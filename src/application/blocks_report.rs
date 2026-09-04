//! The use case behind `claude-stats blocks`: load the entries, cut them into
//! billing blocks, and work out what each block is on course to do.
//!
//! A Service Layer object in Fowler's sense, and as thin as its neighbour
//! [`crate::application::period_report`]. It owns no arithmetic: which
//! responses count is [`UsageQuery`]'s business, where a block begins and ends
//! is [`blocks::identify`]'s, and the rates and projections belong to the
//! domain types that produce them. What it owns is the *sequence* -- the
//! repository is asked once, the blocks are cut from exactly what came back,
//! the ceiling is resolved from all of them and only then is the view narrowed
//! -- and the order of those four steps is the one thing here that cannot be
//! got wrong quietly.
//!
//! That last ordering is worth stating outright, because it is the subtle one:
//! a `--token-limit max` asked *after* `--active` had thrown the other blocks
//! away would compare the running block against itself and report a
//! comfortable 100% forever.

use chrono::{DateTime, Duration, Utc};

use super::ports::{UsageQuery, UsageRepository};
use crate::domain::blocks::{self, BillingBlock, BlockKind, BurnRate, LimitStanding, Projection};
use crate::domain::pricing::{CostMode, PriceSheet};

/// How many days back `--recent` reaches.
///
/// Three rather than one, because the question `--recent` answers is "what
/// have I been doing lately" and a Monday morning wants to see Friday
/// afternoon. It is a constant rather than a flag because a second knob for
/// the same idea as `--since` would be two ways of spelling one filter.
pub const DEFAULT_RECENT_DAYS: i64 = 3;

/// The ceiling a block's projection is judged against.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TokenLimit {
    /// No ceiling, so no percentages and no standing.
    #[default]
    None,
    /// The busiest block already finished.
    ///
    /// The only ceiling this tool can honestly discover for itself. The real
    /// limit lives on Anthropic's side and is never written to disk, so the
    /// next best thing is the most a comparable window has actually held --
    /// which is a number the user produced themselves and can therefore check.
    Max,
    /// A ceiling the user stated outright.
    Exact(u64),
}

impl TokenLimit {
    /// The ceiling in tokens, given every block that was identified.
    ///
    /// [`Self::Max`] deliberately looks only at *finished* blocks. Including
    /// the running one would let it set its own ceiling: the moment it became
    /// the busiest window on record it would be at exactly 100% of the limit
    /// and stay there, and the projection -- the entire reason the column
    /// exists -- could never exceed anything.
    ///
    /// A gap holds nothing, so it can never win and needs no special case
    /// beyond not being a finished block of work.
    #[must_use]
    pub fn resolve(self, blocks: &[BillingBlock]) -> Option<u64> {
        match self {
            Self::None => None,
            Self::Exact(limit) => Some(limit),
            Self::Max => blocks
                .iter()
                .filter(|block| block.kind == BlockKind::Closed)
                .map(|block| block.tokens.total())
                .max(),
        }
    }
}

/// What the caller asked the blocks report for.
///
/// A Query Object's other half: [`UsageQuery`] narrows which responses are
/// counted, and this narrows what is done with them once they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockOptions {
    /// How long a block runs.
    pub span: Duration,
    /// Show only the block that is still running.
    pub active_only: bool,
    /// Keep only blocks that started within this many days, if any.
    pub recent_days: Option<i64>,
    /// The ceiling to judge a projection against.
    pub token_limit: TokenLimit,
}

impl Default for BlockOptions {
    /// Every block, over the standard five-hour window, judged against
    /// nothing.
    fn default() -> Self {
        Self {
            span: Duration::hours(blocks::DEFAULT_SPAN_HOURS),
            active_only: false,
            recent_days: None,
            token_limit: TokenLimit::None,
        }
    }
}

impl BlockOptions {
    /// Whether `block` survives the filters.
    ///
    /// The active block is kept by `--recent` whatever its start says. A
    /// window that opened four days ago and is still being worked in is the
    /// single most relevant row on the table, and dropping it for being old
    /// would answer "what am I doing now" with everything except that.
    fn keeps(&self, block: &BillingBlock, now: DateTime<Utc>) -> bool {
        let live = block.kind == BlockKind::Active;
        if self.active_only && !live {
            return false;
        }
        match self.recent_days {
            Some(days) => live || block.started_at >= now - Duration::days(days),
            None => true,
        }
    }
}

/// One block with everything derived from it.
///
/// The rate, the projection and the standing are all `Option` because all
/// three genuinely may not exist: a single-response block has no rate, a
/// finished block has nothing to project into, and without a ceiling there is
/// nothing to stand against. Reporting a zero for any of them would be a
/// figure that reads as measured when it was invented.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockRow {
    /// The window itself.
    pub block: BillingBlock,
    /// How fast it was consumed, where that could be measured.
    pub rate: Option<BurnRate>,
    /// Where it lands if the rate holds, for the running block only.
    pub projection: Option<Projection>,
    /// How that projection sits against the ceiling.
    pub standing: Option<LimitStanding>,
    /// The ceiling the standing and the percentages are measured against.
    ///
    /// Carried on the row rather than passed beside it because a row is
    /// rendered on its own: the table prints `(assuming N token limit)` and
    /// three percentages, and the JSON carries a `tokenLimitStatus`, none of
    /// which can be produced from a [`LimitStanding`] alone.
    pub limit: Option<u64>,
}

impl BlockRow {
    /// Everything derivable about `block` as of `now`.
    ///
    /// Assembled here rather than inside [`BlocksReport::run`] so that the
    /// derivation is one named thing a test can reach with a hand-built block,
    /// instead of three lines living in the middle of a loop.
    #[must_use]
    pub fn of(block: BillingBlock, now: DateTime<Utc>, limit: Option<u64>) -> Self {
        let rate = BurnRate::measure(&block);
        let projection = rate.and_then(|rate| Projection::of(&block, rate, now));
        let standing = projection
            .zip(limit)
            .map(|(projection, limit)| LimitStanding::of(projection.total_tokens, limit));
        Self {
            block,
            rate,
            projection,
            standing,
            limit,
        }
    }
}

/// Produces the block rows behind `claude-stats blocks`.
///
/// Holds the repository and the sheet for the same reasons its neighbour does:
/// a repository remembers which transcripts it has read, and a sheet composed
/// twice in one run could be composed differently.
pub struct BlocksReport<R: UsageRepository> {
    repository: R,
    sheet: PriceSheet,
}

impl<R: UsageRepository> BlocksReport<R> {
    /// A service over `repository`, costing everything at `sheet`.
    pub const fn new(repository: R, sheet: PriceSheet) -> Self {
        Self { repository, sheet }
    }

    /// The blocks `query` and `options` describe, as of `now`.
    ///
    /// `now` is a parameter rather than something read here, because every
    /// answer this service gives depends on it -- which block is running, how
    /// much of its window is left, what the projection comes to -- and a
    /// service that read the clock itself could only be tested by waiting.
    ///
    /// # Errors
    ///
    /// Only what the repository fails with: the corpus could not be enumerated
    /// at all. A single unreadable transcript is skipped there rather than
    /// failing the report.
    pub fn run(
        &mut self,
        query: &UsageQuery,
        options: &BlockOptions,
        now: DateTime<Utc>,
        mode: CostMode,
    ) -> anyhow::Result<Vec<BlockRow>> {
        let entries = self.repository.entries(query)?;
        let identified = blocks::identify(&entries, options.span, now, mode, &self.sheet);

        // Resolved before the filters run, so that narrowing the view cannot
        // move the ceiling; and a ceiling of zero is treated as no ceiling,
        // because every percentage measured against it would be infinite.
        let limit = options
            .token_limit
            .resolve(&identified)
            .filter(|limit| *limit > 0);

        Ok(identified
            .into_iter()
            .filter(|block| options.keeps(block, now))
            .map(|block| BlockRow::of(block, now, limit))
            .collect())
    }

    /// The sheet the figures were produced with, for a report's footer.
    #[must_use]
    pub const fn sheet(&self) -> &PriceSheet {
        &self.sheet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entry::{Entry, EntryId};
    use crate::domain::model::ModelId;
    use crate::domain::project::{Project, SessionId};
    use crate::domain::tokens::TokenUsage;

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    fn entry(id: &str, when: &str, input: u64) -> Entry {
        Entry {
            id: EntryId {
                message_id: id.to_owned(),
                request_id: Some(format!("req_{id}")),
                session: SessionId::new("session-a"),
            },
            at: at(when),
            model: ModelId::new("claude-opus-5"),
            tokens: TokenUsage {
                input,
                ..TokenUsage::ZERO
            },
            recorded_cost: None,
            session: SessionId::new("session-a"),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    /// A repository that answers from memory and counts how often it was
    /// asked, so a test can prove the service loads once.
    struct InMemory {
        entries: Vec<Entry>,
        loads: u32,
    }

    impl UsageRepository for InMemory {
        fn entries(&mut self, query: &UsageQuery) -> anyhow::Result<Vec<Entry>> {
            self.loads += 1;
            Ok(self
                .entries
                .iter()
                .filter(|entry| query.matches(entry))
                .cloned()
                .collect())
        }
    }

    /// A quiet morning block, a long silence, and a busy live one.
    ///
    /// The first block holds two responses of a million input tokens each; the
    /// second, still running, holds five million. So the busiest *finished*
    /// window is two million and the running one is larger than it, which is
    /// the arrangement `max` can get wrong.
    fn service() -> BlocksReport<InMemory> {
        BlocksReport::new(
            InMemory {
                entries: vec![
                    entry("a", "2026-09-01T09:10:00Z", 1_000_000),
                    entry("b", "2026-09-01T09:40:00Z", 1_000_000),
                    entry("c", "2026-09-05T09:10:00Z", 2_500_000),
                    entry("d", "2026-09-05T09:40:00Z", 2_500_000),
                ],
                loads: 0,
            },
            PriceSheet::builtin(),
        )
    }

    /// The instant every test below reads the corpus at: half an hour into the
    /// second block's window, which runs from 09:00 to 14:00.
    fn now() -> DateTime<Utc> {
        at("2026-09-05T10:00:00Z")
    }

    fn run(options: &BlockOptions) -> Vec<BlockRow> {
        service()
            .run(&UsageQuery::default(), options, now(), CostMode::Calculate)
            .expect("the fake cannot fail")
    }

    #[test]
    fn a_token_limit_of_max_is_the_busiest_previous_block_rather_than_this_one() {
        // The running block is the biggest window on record. Letting it set
        // its own ceiling would pin it at 100% for ever and make the
        // projection -- the whole reason the column exists -- unable to exceed
        // anything.
        let rows = run(&BlockOptions {
            token_limit: TokenLimit::Max,
            ..BlockOptions::default()
        });

        let live = rows.last().expect("the running block");
        assert_eq!(live.block.kind, BlockKind::Active);
        assert_eq!(live.block.tokens.total(), 5_000_000);
        assert_eq!(
            live.limit,
            Some(2_000_000),
            "the ceiling is the quiet morning, not the busy afternoon"
        );
        assert_eq!(
            live.standing,
            Some(LimitStanding::Exceeds),
            "five million already, against a two million ceiling"
        );

        // Stated outright, the ceiling is whatever was stated. Five hundred
        // million is comfortably clear of the forty-five million this block is
        // projected to reach.
        let exact = run(&BlockOptions {
            token_limit: TokenLimit::Exact(500_000_000),
            ..BlockOptions::default()
        });
        assert_eq!(
            exact.last().expect("the running block").limit,
            Some(500_000_000)
        );
        assert_eq!(
            exact.last().expect("the running block").standing,
            Some(LimitStanding::Ok)
        );
    }

    #[test]
    fn with_no_limit_a_row_carries_no_standing_at_all() {
        // Nothing to stand against, so no percentage is invented for the
        // column to print.
        let rows = run(&BlockOptions::default());
        let live = rows.last().expect("the running block");

        assert_eq!(live.limit, None);
        assert_eq!(live.standing, None);
        assert!(live.projection.is_some(), "the projection needs no ceiling");
    }

    #[test]
    fn the_service_reads_the_repository_once_and_cuts_what_it_got_into_blocks() {
        let mut service = service();
        let rows = service
            .run(
                &UsageQuery::default(),
                &BlockOptions::default(),
                now(),
                CostMode::Calculate,
            )
            .expect("the fake cannot fail");

        assert_eq!(service.repository.loads, 1, "one query, one read");
        assert_eq!(rows.len(), 3, "two blocks with the gap between them");
        assert_eq!(rows[1].block.kind, BlockKind::Gap);
        assert_eq!(rows[2].block.kind, BlockKind::Active);
        // Seven million Opus 5 input tokens at $5 a million, across the two
        // real blocks.
        assert_eq!(rows[0].block.cost.to_cents(), 1_000);
        assert_eq!(rows[2].block.cost.to_cents(), 2_500);
    }

    #[test]
    fn asking_for_the_active_block_leaves_the_ceiling_where_it_was() {
        // The filters run after the ceiling is resolved. Reversed, `--active
        // --token-limit max` would compare the running block against itself.
        let rows = run(&BlockOptions {
            active_only: true,
            token_limit: TokenLimit::Max,
            ..BlockOptions::default()
        });

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].block.kind, BlockKind::Active);
        assert_eq!(rows[0].limit, Some(2_000_000));
    }

    #[test]
    fn recent_keeps_the_running_block_however_long_ago_it_opened() {
        // Three days back from 5 September is the 2nd, so the block of the 1st
        // falls away along with the gap that preceded it.
        let rows = run(&BlockOptions {
            recent_days: Some(DEFAULT_RECENT_DAYS),
            ..BlockOptions::default()
        });

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].block.started_at, at("2026-09-05T09:00:00Z"));

        // A window that opened a week ago and is still being worked in is the
        // most relevant row there is, so it survives its own start date.
        let stale = BlocksReport::new(
            InMemory {
                entries: vec![
                    entry("a", "2026-09-05T09:10:00Z", 1_000),
                    entry("b", "2026-09-05T09:40:00Z", 1_000),
                ],
                loads: 0,
            },
            PriceSheet::builtin(),
        )
        .run(
            &UsageQuery::default(),
            &BlockOptions {
                recent_days: Some(1),
                ..BlockOptions::default()
            },
            at("2026-09-05T10:00:00Z"),
            CostMode::Calculate,
        )
        .expect("the fake cannot fail");
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].block.kind, BlockKind::Active);
    }

    #[test]
    fn a_ceiling_of_zero_is_no_ceiling_rather_than_a_division_by_zero() {
        // `max` over a corpus whose only block is the running one finds no
        // finished window at all, and a stated zero is the same answer typed
        // by hand. Either way every percentage against it would be infinite.
        let rows = run(&BlockOptions {
            token_limit: TokenLimit::Exact(0),
            ..BlockOptions::default()
        });
        assert_eq!(rows.last().expect("the running block").limit, None);

        let only_live = BlocksReport::new(
            InMemory {
                entries: vec![entry("a", "2026-09-05T09:10:00Z", 1_000)],
                loads: 0,
            },
            PriceSheet::builtin(),
        )
        .run(
            &UsageQuery::default(),
            &BlockOptions {
                token_limit: TokenLimit::Max,
                ..BlockOptions::default()
            },
            now(),
            CostMode::Calculate,
        )
        .expect("the fake cannot fail");
        assert_eq!(only_live[0].limit, None);
        assert_eq!(only_live[0].standing, None);
    }
}
