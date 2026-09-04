//! The use case behind `claude-stats statusline`: reduce the whole account's
//! state to the one line Claude Code embeds in its own prompt.
//!
//! A Service Layer object in Fowler's sense, and built from the same pieces
//! [`crate::application::blocks_report`] already assembled: the repository
//! answers "every billable response", [`crate::domain::blocks::identify`]
//! cuts them into windows, and [`crate::domain::blocks::BurnRate`] measures
//! the running one. What is new here is the *reduction* -- a dashboard's
//! worth of figures collapsed to a model name, three costs and a context
//! reading -- and the one input none of the other reports have: the hook
//! itself, which already knows some of these answers and is always the
//! better source when it does.
//!
//! The output is a [`StatuslineLine`] view model rather than a string.
//! [`crate::view::statusline::render`] is what turns it into the text Claude
//! Code prints, and keeping the two apart is what makes the assembly here
//! testable by comparing fields rather than by parsing a sentence back apart.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};

use super::ports::{TranscriptTailReader, UsageQuery, UsageRepository};
use crate::domain::blocks::{self, BlockKind, BurnRate, Intensity};
use crate::domain::context::ContextFill;
use crate::domain::entry::Entry;
use crate::domain::model::ModelId;
use crate::domain::money::Usd;
use crate::domain::period::Zone;
use crate::domain::pricing::{CostMode, PriceSheet};
use crate::domain::project::SessionId;

/// What the statusline hook told this run, translated out of the JSON it
/// arrived in.
///
/// The application layer's own shape for the hook's payload, rather than the
/// `serde` structures in [`crate::infrastructure::statusline::hook`]
/// themselves. Nothing above [`crate::infrastructure`] is allowed to depend on
/// it -- the hexagonal rule this whole crate is built on -- so the wire format
/// is translated once, at the boundary, into a value this service can accept
/// without ever importing `serde_json`.
#[derive(Debug, Clone, PartialEq)]
pub struct StatuslineRequest {
    /// The conversation this line is about, when the hook named one.
    pub session_id: Option<SessionId>,
    /// Where that conversation's transcript lives, for the context fallback.
    pub transcript_path: Option<PathBuf>,
    /// The catalogue key of the model in use, for pricing the context window.
    pub model_id: Option<ModelId>,
    /// The name to print. Claude Code's own, not the catalogue's, because a
    /// hook that named a model at all gets to say what it is called.
    pub model_display_name: Option<String>,
    /// The reasoning effort in force, if the hook reported one.
    pub effort: Option<String>,
    /// The session's cost, as Claude Code itself has already totalled it.
    ///
    /// Preferred over this run's own sum whenever it is present -- see
    /// [`StatuslineReport::run`] for why.
    pub hook_session_cost: Option<Usd>,
    /// The context reading, as Claude Code itself has already measured it.
    ///
    /// Preferred over scanning the transcript for the same reason the cost
    /// is: it is the tool that just made the call, and cannot be stale by
    /// even one response the way a figure derived after the fact can be.
    pub hook_context: Option<ContextFill>,
}

/// One rendered statusline, as a view model rather than a string.
///
/// Kept as fields a test can compare rather than a sentence a test would have
/// to parse back apart -- the same reason every other report in this crate
/// hands the view layer a value object instead of assembling text itself.
#[derive(Debug, Clone, PartialEq)]
pub struct StatuslineLine {
    /// The model's display name, plus its effort level where one was given.
    /// Assembling the two into one field is the view's job, not this one's.
    pub model: String,
    /// The reasoning effort in force, printed beside the model name.
    pub effort: Option<String>,
    /// What this conversation has cost so far. `None`, not zero, when there
    /// is nothing to measure it from -- see [`StatuslineRequest::session_id`].
    pub session_cost: Option<Usd>,
    /// What the whole account has cost today, on the reporting calendar.
    pub today_cost: Usd,
    /// The five-hour window presently running, if one is.
    pub block: Option<BlockSegment>,
    /// How fast that window is being spent. Always `None` when
    /// [`Self::block`] is, because there is nothing to measure a rate across.
    pub burn: Option<BurnSegment>,
    /// How full the model's context window is.
    pub context: Option<ContextFill>,
}

/// The running billing block, reduced to what a single line has room for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockSegment {
    /// What the block has cost so far.
    pub cost: Usd,
    /// How long until it closes.
    pub remaining: Duration,
}

/// How fast the running block is being spent, reduced the same way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurnSegment {
    /// What an hour at the current rate would cost.
    pub cost_per_hour: Usd,
    /// Which band that rate falls in.
    pub intensity: Intensity,
}

/// Printed in place of a model name the hook never gave.
///
/// A statusline hook that supplies no model at all is not a failure worth
/// stopping the prompt over -- there is still a session cost and a context
/// reading worth showing -- so the line degrades to naming what it does not
/// know rather than refusing to render.
const UNKNOWN_MODEL: &str = "unknown model";

/// Produces the [`StatuslineLine`] behind `claude-stats statusline`.
///
/// Holds the repository, the tail reader and the sheet for the reason its
/// neighbours do: a repository remembers which transcripts it has read, and a
/// sheet composed twice in one run could be composed differently.
pub struct StatuslineReport<R: UsageRepository, T: TranscriptTailReader> {
    repository: R,
    tail: T,
    sheet: PriceSheet,
}

impl<R: UsageRepository, T: TranscriptTailReader> StatuslineReport<R, T> {
    /// A service over `repository`, filling in what the hook omitted from
    /// `tail`, costing everything at `sheet`.
    pub const fn new(repository: R, tail: T, sheet: PriceSheet) -> Self {
        Self {
            repository,
            tail,
            sheet,
        }
    }

    /// The line to print for `request`, as of `now`.
    ///
    /// `now` and `zone` are parameters rather than something read here, for
    /// the reason every other report in this crate takes them as parameters:
    /// every answer below depends on them, and a service that read the clock
    /// or the environment itself could only be tested by waiting.
    ///
    /// # Errors
    ///
    /// Returns an error when the repository cannot enumerate the corpus at
    /// all, or when the transcript named by [`StatuslineRequest::transcript_path`]
    /// cannot be opened for the context fallback. Both are the caller's cue to
    /// fall back to the last good line rather than print either failure into
    /// somebody's prompt.
    pub fn run(
        &mut self,
        request: &StatuslineRequest,
        zone: &Zone,
        now: DateTime<Utc>,
        mode: CostMode,
    ) -> anyhow::Result<StatuslineLine> {
        let entries = self.repository.entries(&UsageQuery::default())?;

        let session_cost = self.session_cost(request, &entries, mode);
        let today_cost = Self::today_cost(&entries, zone, now, mode, &self.sheet);

        let span = Duration::hours(blocks::DEFAULT_SPAN_HOURS);
        let identified = blocks::identify(&entries, span, now, mode, &self.sheet);
        // At most one block can be active, and only the last one identify
        // produces is ever a candidate -- see its own documentation -- so
        // finding the first is finding the only one there could be.
        let active = identified
            .into_iter()
            .find(|block| block.kind == BlockKind::Active);

        let block = active.as_ref().map(|block| BlockSegment {
            cost: block.cost,
            remaining: block.ends_at - now,
        });
        // The burn segment is derived from the same block the block segment
        // is, rather than from a second search, so the two can never disagree
        // about which window they are describing.
        let burn = active
            .as_ref()
            .and_then(BurnRate::measure)
            .map(|rate| BurnSegment {
                cost_per_hour: rate.cost_per_hour,
                intensity: rate.intensity(),
            });

        let context = self.context_fill(request)?;

        Ok(StatuslineLine {
            model: request
                .model_display_name
                .clone()
                .unwrap_or_else(|| UNKNOWN_MODEL.to_owned()),
            effort: request.effort.clone(),
            session_cost,
            today_cost,
            block,
            burn,
            context,
        })
    }

    /// This conversation's cost, preferring what Claude Code already
    /// measured over this run's own sum.
    ///
    /// The hook's own figure is Claude Code's, produced by the process that
    /// just made the call and therefore never stale by even the response that
    /// prompted this redraw. This run's sum, by contrast, is only as fresh as
    /// the corpus scan above it, and disagrees with the hook's figure only
    /// when the two were measured moments apart -- which is why the hook
    /// wins whenever it has an opinion at all.
    ///
    /// `None`, not [`Usd::ZERO`], when there is nothing to answer from: an
    /// absent session id and an absent hook figure both mean "cannot say",
    /// and zero would claim to be an answer instead.
    fn session_cost(
        &self,
        request: &StatuslineRequest,
        entries: &[Entry],
        mode: CostMode,
    ) -> Option<Usd> {
        if let Some(cost) = request.hook_session_cost {
            return Some(cost);
        }
        let session_id = request.session_id.as_ref()?;
        Some(Usd::total(
            entries
                .iter()
                .filter(|entry| &entry.session == session_id)
                .map(|entry| mode.cost_of(entry, &self.sheet)),
        ))
    }

    /// The whole account's cost so far today, on `zone`'s calendar.
    ///
    /// Bounded through [`Zone::day_bounds`] rather than a bare date
    /// comparison, so "today" agrees with the same half-open rule every other
    /// calendar bucket in this crate uses.
    fn today_cost(
        entries: &[Entry],
        zone: &Zone,
        now: DateTime<Utc>,
        mode: CostMode,
        sheet: &PriceSheet,
    ) -> Usd {
        let (start, end) = zone.day_bounds(zone.local_date(now));
        Usd::total(
            entries
                .iter()
                .filter(|entry| entry.at >= start && entry.at < end)
                .map(|entry| mode.cost_of(entry, sheet)),
        )
    }

    /// How full the model's context window is, preferring the hook's own
    /// reading over the transcript fallback.
    fn context_fill(&self, request: &StatuslineRequest) -> anyhow::Result<Option<ContextFill>> {
        if let Some(fill) = request.hook_context {
            return Ok(Some(fill));
        }
        let Some(path) = &request.transcript_path else {
            return Ok(None);
        };
        let Some(usage) = self.tail.last_turn_usage(path)? else {
            return Ok(None);
        };
        // Prompt tokens, not the response's total: this is what fills the
        // window for the *next* call, and the model's own output tokens have
        // already been folded into the input or cache counters by the time
        // they would occupy it.
        let used = usage.prompt_tokens();
        // No model id at all is one further step back than an unrecognised
        // one: [`PriceSheet::context_window_for`] already falls back to the
        // catalogue's own default for the latter, so the same figure is used
        // here rather than a second constant that could drift from it.
        let window = request.model_id.as_ref().map_or(
            crate::domain::model::ModelCatalog::DEFAULT_CONTEXT_WINDOW,
            |model| self.sheet.context_window_for(model),
        );
        Ok(Some(ContextFill::new(used, window)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entry::EntryId;
    use crate::domain::model::ModelId;
    use crate::domain::project::Project;
    use crate::domain::tokens::TokenUsage;

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    fn entry(session: &str, when: &str, tokens: TokenUsage) -> Entry {
        Entry {
            id: EntryId {
                message_id: format!("msg-{when}-{session}"),
                request_id: None,
                session: SessionId::new(session),
            },
            at: at(when),
            model: ModelId::new("claude-opus-5"),
            tokens,
            recorded_cost: None,
            session: SessionId::new(session),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    fn input(tokens: u64) -> TokenUsage {
        TokenUsage {
            input: tokens,
            ..TokenUsage::ZERO
        }
    }

    /// A repository that answers from memory.
    struct InMemory(Vec<Entry>);

    impl UsageRepository for InMemory {
        fn entries(&mut self, query: &UsageQuery) -> anyhow::Result<Vec<Entry>> {
            Ok(self
                .0
                .iter()
                .filter(|entry| query.matches(entry))
                .cloned()
                .collect())
        }
    }

    /// A tail reader that never has anything to say -- every test below
    /// gives the context reading through the hook or not at all.
    struct NoTail;

    impl TranscriptTailReader for NoTail {
        fn last_turn_usage(&self, _path: &std::path::Path) -> anyhow::Result<Option<TokenUsage>> {
            Ok(None)
        }
    }

    fn bare_request() -> StatuslineRequest {
        StatuslineRequest {
            session_id: None,
            transcript_path: None,
            model_id: None,
            model_display_name: Some("Opus 5".to_owned()),
            effort: None,
            hook_session_cost: None,
            hook_context: None,
        }
    }

    #[test]
    fn the_hook_recorded_cost_is_preferred_over_our_own_sum_when_it_is_present() {
        // Two million Opus 5 input tokens would price at $10.00 through the
        // sheet, but the hook says the session cost $7.50 -- Claude Code's own
        // figure, measured moments after the call it describes, which is
        // exactly why it must win.
        let mut service = StatuslineReport::new(
            InMemory(vec![entry(
                "session-a",
                "2026-09-03T09:00:00Z",
                input(2_000_000),
            )]),
            NoTail,
            PriceSheet::builtin(),
        );
        let request = StatuslineRequest {
            session_id: Some(SessionId::new("session-a")),
            hook_session_cost: Some(Usd::new(7.50)),
            ..bare_request()
        };

        let line = service
            .run(
                &request,
                &Zone::Utc,
                at("2026-09-03T09:30:00Z"),
                CostMode::Auto,
            )
            .expect("the fake cannot fail");

        assert_eq!(line.session_cost, Some(Usd::new(7.50)));
    }

    #[test]
    fn with_no_hook_figure_the_session_cost_is_summed_from_the_corpus() {
        let mut service = StatuslineReport::new(
            InMemory(vec![
                entry("session-a", "2026-09-03T09:00:00Z", input(1_000_000)),
                entry("session-b", "2026-09-03T09:00:00Z", input(9_000_000)),
            ]),
            NoTail,
            PriceSheet::builtin(),
        );
        let request = StatuslineRequest {
            session_id: Some(SessionId::new("session-a")),
            ..bare_request()
        };

        let line = service
            .run(
                &request,
                &Zone::Utc,
                at("2026-09-03T09:30:00Z"),
                CostMode::Auto,
            )
            .expect("the fake cannot fail");

        // One million Opus 5 input tokens at $5 a million, and nothing from
        // the other session.
        assert_eq!(line.session_cost, Some(Usd::new(5.0)));
    }

    #[test]
    fn with_neither_a_hook_figure_nor_a_session_id_there_is_no_session_cost_to_report() {
        let mut service =
            StatuslineReport::new(InMemory(Vec::new()), NoTail, PriceSheet::builtin());

        let line = service
            .run(
                &bare_request(),
                &Zone::Utc,
                at("2026-09-03T09:30:00Z"),
                CostMode::Auto,
            )
            .expect("the fake cannot fail");

        assert_eq!(
            line.session_cost, None,
            "nothing to answer from, so nothing is claimed"
        );
    }

    #[test]
    fn todays_cost_is_measured_in_the_reporting_timezone_not_in_utc() {
        // 21:30 in Tokyo on the 2nd is 12:30 UTC the same day, but 09:30 in
        // Tokyo on the 3rd is 00:30 UTC on the *same* day too -- so a response
        // at 00:30 UTC on the 3rd is "today" measured from Tokyo's midday on
        // the 3rd, but was still yesterday evening in Tokyo when it happened.
        let tokyo = Zone::parse("Asia/Tokyo").expect("a real zone");
        let entries = InMemory(vec![
            // 09:30 JST on the 3rd: today in Tokyo, and also 00:30 UTC on the
            // 3rd -- today in UTC as well, so this one cannot distinguish the
            // two calendars on its own.
            entry("s", "2026-09-03T00:30:00Z", input(200_000)),
            // 23:30 JST on the 2nd is 14:30 UTC on the 2nd: yesterday on
            // *both* calendars, and excluded either way.
            entry("s", "2026-09-02T14:30:00Z", input(300_000)),
        ]);
        let now = at("2026-09-03T10:00:00Z"); // 19:00 JST on the 3rd

        let mut service = StatuslineReport::new(entries, NoTail, PriceSheet::builtin());
        let tokyo_line = service
            .run(&bare_request(), &tokyo, now, CostMode::Auto)
            .expect("the fake cannot fail");
        // Two hundred thousand Opus 5 input tokens at $5 a million.
        assert_eq!(tokyo_line.today_cost, Usd::new(1.0));

        // The same entries measured on UTC's calendar instead: 00:30 UTC on
        // the 3rd is still today, so the total does not move for this
        // fixture -- the point below is the boundary case that does.
        let mut boundary = StatuslineReport::new(
            InMemory(vec![entry(
                "s",
                "2026-09-02T16:00:00Z", // 01:00 JST on the 3rd: today in Tokyo
                input(400_000),
            )]),
            NoTail,
            PriceSheet::builtin(),
        );
        let tokyo_only = boundary
            .run(&bare_request(), &tokyo, now, CostMode::Auto)
            .expect("the fake cannot fail")
            .today_cost;
        let utc_only = boundary
            .run(&bare_request(), &Zone::Utc, now, CostMode::Auto)
            .expect("the fake cannot fail")
            .today_cost;
        assert_eq!(
            tokyo_only,
            Usd::new(2.0),
            "01:00 JST on the 3rd counts as today in Tokyo"
        );
        assert_eq!(
            utc_only,
            Usd::ZERO,
            "16:00 UTC on the 2nd is still yesterday by UTC's own clock"
        );
    }

    #[test]
    fn a_session_with_no_active_block_has_neither_a_block_nor_a_burn_segment() {
        // The one entry is five hours in the past, so its window has closed.
        let mut service = StatuslineReport::new(
            InMemory(vec![entry("s", "2026-09-03T00:00:00Z", input(1_000))]),
            NoTail,
            PriceSheet::builtin(),
        );

        let line = service
            .run(
                &bare_request(),
                &Zone::Utc,
                at("2026-09-03T10:00:00Z"),
                CostMode::Auto,
            )
            .expect("the fake cannot fail");

        assert_eq!(line.block, None);
        assert_eq!(
            line.burn, None,
            "there is nothing left to measure a rate across"
        );
    }

    #[test]
    fn the_burn_intensity_uses_the_indicator_rate_so_a_cache_heavy_block_is_not_high() {
        // A resumed conversation replaying a long prefix moves millions of
        // cache-read tokens for a few thousand of fresh work. Banding that on
        // the raw rate would paint every resumed session as a crisis; the
        // indicator excludes cache traffic precisely so it does not.
        let replay = |input_tokens: u64| TokenUsage {
            input: input_tokens,
            cache_read: 900_000,
            ..TokenUsage::ZERO
        };
        let mut service = StatuslineReport::new(
            InMemory(vec![
                entry("s", "2026-09-03T09:00:00Z", replay(500)),
                entry("s", "2026-09-03T09:10:00Z", replay(500)),
            ]),
            NoTail,
            PriceSheet::builtin(),
        );

        let line = service
            .run(
                &bare_request(),
                &Zone::Utc,
                at("2026-09-03T09:15:00Z"),
                CostMode::Auto,
            )
            .expect("the fake cannot fail");

        let burn = line.burn.expect("two responses ten minutes apart");
        assert_eq!(
            burn.intensity,
            Intensity::Normal,
            "a thousand fresh tokens in ten minutes is quiet, whatever the cache moved"
        );
    }

    #[test]
    fn a_missing_model_prints_a_named_placeholder_rather_than_an_empty_line() {
        let mut service =
            StatuslineReport::new(InMemory(Vec::new()), NoTail, PriceSheet::builtin());
        let request = StatuslineRequest {
            model_display_name: None,
            ..bare_request()
        };

        let line = service
            .run(
                &request,
                &Zone::Utc,
                at("2026-09-03T09:15:00Z"),
                CostMode::Auto,
            )
            .expect("the fake cannot fail");

        assert_eq!(line.model, UNKNOWN_MODEL);
    }

    #[test]
    fn the_hook_context_reading_is_preferred_over_the_transcript_fallback() {
        struct FailingTail;
        impl TranscriptTailReader for FailingTail {
            fn last_turn_usage(
                &self,
                _path: &std::path::Path,
            ) -> anyhow::Result<Option<TokenUsage>> {
                anyhow::bail!("must not be called when the hook already answered")
            }
        }

        let mut service =
            StatuslineReport::new(InMemory(Vec::new()), FailingTail, PriceSheet::builtin());
        let request = StatuslineRequest {
            transcript_path: Some(PathBuf::from("/tmp/whatever.jsonl")),
            hook_context: Some(ContextFill::new(25_000, 200_000)),
            ..bare_request()
        };

        let line = service
            .run(
                &request,
                &Zone::Utc,
                at("2026-09-03T09:15:00Z"),
                CostMode::Auto,
            )
            .expect("the tail reader must not have been asked");

        assert_eq!(line.context, Some(ContextFill::new(25_000, 200_000)));
    }
}
