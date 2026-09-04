//! Five-hour billing blocks: when one opened, what it holds, how fast it is
//! being spent, and whether it is on course to outrun a limit.
//!
//! Claude Code meters a subscription in rolling five-hour windows, and the one
//! question a user asks under pressure is "will this block blow the limit
//! before I finish". Answering it needs three things that are all arithmetic
//! over a sorted list of instants: which responses belong to the same window,
//! how quickly that window is being consumed, and where that rate lands by the
//! time the window closes.
//!
//! All three live here, in the domain, with no clock and no files anywhere in
//! sight. `now` is a parameter rather than something this module reads, which
//! is what lets every rule below be pinned by a three-line fixture: an
//! off-by-one in a window boundary is invisible on a screen and obvious in a
//! test, and the two comparisons in [`identify`] are the likeliest place in
//! this crate for one to hide.
//!
//! In Fowler's vocabulary a [`BillingBlock`] is a Value Object -- built once
//! from a run of entries, never mutated afterwards, compared by what it holds
//! rather than by identity -- and [`identify`] is the fold that produces them.
//! Keeping the fold here rather than beside the report is what lets the live
//! dashboard and the printed table read the same blocks, so the two can never
//! disagree about when the current window opened.

use chrono::{DateTime, Duration, Timelike, Utc};

use super::entry::Entry;
use super::model::ModelId;
use super::money::Usd;
use super::pricing::{CostMode, PriceSheet};
use super::tokens::TokenUsage;

/// How long a billing block runs, unless the caller says otherwise.
///
/// Five hours because that is the window Anthropic meters a subscription in;
/// it is a constant here and a flag at the command line so that a change to
/// the plan is a value rather than a release.
pub const DEFAULT_SPAN_HOURS: i64 = 5;

/// What a block in a column of blocks actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// Still running: the window is open and work has been done in it lately.
    Active,
    /// Over, whether because its window elapsed or because the work stopped.
    Closed,
    /// No block at all -- the stretch between two of them, kept so that the
    /// silence is visible rather than inferred.
    Gap,
}

/// One five-hour window of billable work, or the gap between two of them.
///
/// Every field is public because this is a value read by reports rather than
/// an object that guards an invariant: once [`identify`] has produced it there
/// is nothing left to protect, and a reader who cannot see the parts cannot
/// check the whole.
#[derive(Debug, Clone, PartialEq)]
pub struct BillingBlock {
    /// When the window opened, floored to the UTC hour of its first response.
    ///
    /// Floored rather than taken from the response itself because the limit is
    /// anchored to the clock, not to the moment somebody pressed enter. A
    /// window that began with a message at 09:47 runs to 14:00, not to 14:47,
    /// and a report that said otherwise would have a user waiting
    /// three-quarters of an hour for a reset that had already happened.
    pub started_at: DateTime<Utc>,
    /// When the window closes: [`Self::started_at`] plus the span.
    pub ends_at: DateTime<Utc>,
    /// The last response inside it, if it saw any.
    ///
    /// Deliberately not the same thing as [`Self::ends_at`]. A block that saw
    /// one response at nine in the morning and nothing afterwards still runs
    /// its full five hours, and conflating the two would report a window as
    /// finished while it was still consuming the allowance.
    pub last_activity_at: Option<DateTime<Utc>>,
    /// The first response inside it, if it saw any.
    ///
    /// Carried so that [`BurnRate::measure`] can measure across the work
    /// rather than across the window; see its own documentation for why the
    /// distinction changes the number materially.
    pub first_activity_at: Option<DateTime<Utc>>,
    /// Everything counted into the window.
    pub tokens: TokenUsage,
    /// What those tokens cost, under the report's cost mode and price sheet.
    pub cost: Usd,
    /// The models that contributed, in the order they were first seen.
    ///
    /// First-seen rather than sorted, for the same reason the period tables
    /// keep that order: a model arriving partway through a window is a change
    /// worth seeing, and sorting scatters the signal.
    pub models: Vec<ModelId>,
    /// How many responses were counted.
    pub entries: usize,
    /// Whether this is a live window, a finished one, or the silence between
    /// two.
    pub kind: BlockKind,
}

impl BillingBlock {
    /// Whether this block is still running as of `now`.
    ///
    /// Two conditions, both strict, and both required. The window must still
    /// be open, *and* something must have happened in it within the last span:
    /// a block whose last response was five hours ago is over even if its own
    /// window has not formally closed, because the allowance it was consuming
    /// has already rolled forward.
    ///
    /// On blocks that [`identify`] builds the second condition can only fire
    /// once the first has, because a block's last response is never earlier
    /// than its hour-floored start. It is kept anyway, and kept public,
    /// because the rule is about the block rather than about how this module
    /// happens to construct one -- the live dashboard holds a window it
    /// assembled itself, and the answer for it must be the same answer.
    ///
    /// A gap is never live: there is nothing in it to be live.
    #[must_use]
    pub fn is_live(&self, now: DateTime<Utc>, span: Duration) -> bool {
        if self.kind == BlockKind::Gap {
            return false;
        }
        let touched_recently = self.last_activity_at.is_some_and(|last| now - last < span);
        now < self.ends_at && touched_recently
    }

    /// The stretch of silence between the block that ended and the one that
    /// followed.
    ///
    /// Rendered rather than dropped. A reader scanning a column of blocks
    /// needs to see that nothing happened; left out, the same fact has to be
    /// inferred from a jump in two timestamps, which is exactly the sort of
    /// arithmetic a table exists to save somebody.
    fn gap(started_at: DateTime<Utc>, ends_at: DateTime<Utc>) -> Self {
        Self {
            started_at,
            ends_at,
            last_activity_at: None,
            first_activity_at: None,
            tokens: TokenUsage::ZERO,
            cost: Usd::ZERO,
            models: Vec::new(),
            entries: 0,
            kind: BlockKind::Gap,
        }
    }
}

/// Folds `entries` into the blocks they were billed in.
///
/// The single place a block boundary is decided. Every rule is stated once
/// here so that the report, the JSON and any future dashboard panel cannot
/// come to disagree about when the current window opened.
///
/// The entries are sorted ascending first, because a corpus is a pile of files
/// and nothing guarantees the order they were read in. A block opens on the
/// first response with [`BillingBlock::started_at`] floored to the UTC hour
/// that response fell in, and closes when either of two things happens:
///
/// * the next response is more than a span after the block *started*, which is
///   what bounds a busy window; or
/// * the next response is more than a span after the *previous* response,
///   which is what stops an overnight break being counted as one long sitting.
///
/// Both comparisons are strictly greater-than, and that is deliberate: a
/// response exactly a span after the start still belongs to the block, and a
/// pause of exactly a span does not close it. The second condition also emits
/// a [`BlockKind::Gap`] between the two real blocks.
///
/// A response carrying no tokens at all still opens a block. It is still
/// activity, and the window it started is still consuming the allowance.
///
/// `span` is expected to be at least an hour, and the flooring above is why.
/// A window shorter than the hour it is anchored to can close before its own
/// first response -- a response at 09:47 in a half-hour window opens a block
/// running 09:00 to 09:30 -- which leaves every response in that hour opening
/// another block stamped 09:00 and none of them able to be the live one.
/// Nothing here panics or clamps on a shorter span, because the domain is not
/// where a mistyped flag is caught; [`crate::cli`] refuses one before a
/// transcript is opened, which is the only place that can say so usefully.
#[must_use]
pub fn identify(
    entries: &[Entry],
    span: Duration,
    now: DateTime<Utc>,
    mode: CostMode,
    sheet: &PriceSheet,
) -> Vec<BillingBlock> {
    // Sorted through references rather than by cloning the entries: a month of
    // traffic is several hundred thousand of them, each holding five heap
    // allocations, and none of that has to be copied to put them in order.
    //
    // The tie-break down to the full identity is not decoration. Transcripts
    // are stamped to the second, so ties are common, and without it the order
    // of two responses in the same second -- and therefore which model the
    // block names first -- would come out of whatever order the repository's
    // hash map happened to yield.
    let mut ordered: Vec<&Entry> = entries.iter().collect();
    ordered.sort_by(|a, b| {
        a.at.cmp(&b.at)
            .then_with(|| a.id.message_id.cmp(&b.id.message_id))
            .then_with(|| a.id.request_id.cmp(&b.id.request_id))
            .then_with(|| a.id.session.cmp(&b.id.session))
    });

    let mut blocks: Vec<BillingBlock> = Vec::new();
    let mut open: Option<OpenBlock> = None;

    for entry in ordered {
        // `take_if` rather than a test followed by a `take().expect(..)`: the
        // two spellings do the same thing, but only this one has no panic in
        // it to reason about, and a dashboard that panics over a timestamp is
        // the failure this module is most careful to avoid.
        if let Some(finished) = open.take_if(|block| !block.admits(entry.at, span)) {
            let quiet_since = finished.last_activity_at;
            blocks.push(finished.sealed());
            if entry.at - quiet_since > span {
                blocks.push(BillingBlock::gap(quiet_since + span, entry.at));
            }
        }
        open.get_or_insert_with(|| OpenBlock::opened(entry.at, span))
            .push(entry, mode.cost_of(entry, sheet));
    }

    if let Some(current) = open {
        blocks.push(current.sealed());
    }
    // Only the last block can be live, because every earlier one was closed by
    // a response that arrived after it. Asking the question of all of them
    // would be the same answer and a slower way of getting it.
    if let Some(last) = blocks.last_mut() {
        if last.is_live(now, span) {
            last.kind = BlockKind::Active;
        }
    }
    blocks
}

/// A block still being filled.
///
/// Separate from [`BillingBlock`] because the two have different shapes: while
/// a block is open its first and last responses are known rather than
/// optional, and its kind is not decided yet. Modelling that as a
/// half-populated `BillingBlock` would mean every reader of the finished type
/// had to wonder which of its fields were meaningful.
struct OpenBlock {
    started_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    first_activity_at: DateTime<Utc>,
    last_activity_at: DateTime<Utc>,
    tokens: TokenUsage,
    cost: Usd,
    models: Vec<ModelId>,
    entries: usize,
}

impl OpenBlock {
    /// Opens a window on a response recorded at `at`.
    fn opened(at: DateTime<Utc>, span: Duration) -> Self {
        let started_at = floored_to_the_hour(at);
        Self {
            started_at,
            ends_at: started_at + span,
            first_activity_at: at,
            last_activity_at: at,
            tokens: TokenUsage::ZERO,
            cost: Usd::ZERO,
            models: Vec::new(),
            entries: 0,
        }
    }

    /// Whether a response recorded at `at` still belongs to this block.
    ///
    /// Both comparisons are strict, so a response landing exactly a span after
    /// the start, or exactly a span after the one before it, stays inside.
    fn admits(&self, at: DateTime<Utc>, span: Duration) -> bool {
        at - self.started_at <= span && at - self.last_activity_at <= span
    }

    /// Counts one response into the block.
    fn push(&mut self, entry: &Entry, cost: Usd) {
        self.last_activity_at = entry.at;
        self.tokens += entry.tokens;
        // Through `Usd::total` rather than `+=`, so that every sum in the
        // crate is the same deliberate left fold and two reports over the same
        // traffic cannot differ in the last bits.
        self.cost = Usd::total([self.cost, cost]);
        self.entries += 1;
        if !self.models.contains(&entry.model) {
            self.models.push(entry.model.clone());
        }
    }

    /// The finished block, still to be told whether it is the live one.
    fn sealed(self) -> BillingBlock {
        BillingBlock {
            started_at: self.started_at,
            ends_at: self.ends_at,
            last_activity_at: Some(self.last_activity_at),
            first_activity_at: Some(self.first_activity_at),
            tokens: self.tokens,
            cost: self.cost,
            models: self.models,
            entries: self.entries,
            kind: BlockKind::Closed,
        }
    }
}

/// The top of the UTC hour `at` fell in.
///
/// Falls back to `at` itself if the clock arithmetic ever refuses, which it
/// cannot for a real instant: every hour has a zeroth minute. Written as a
/// fallback rather than an `expect` because a dashboard that panics over a
/// timestamp is worse than one that reports a window a few minutes late.
fn floored_to_the_hour(at: DateTime<Utc>) -> DateTime<Utc> {
    at.with_minute(0)
        .and_then(|at| at.with_second(0))
        .and_then(|at| at.with_nanosecond(0))
        .unwrap_or(at)
}

/// How fast a block is being consumed.
///
/// Two rates rather than one, and the second is the interesting one. See
/// [`Self::measure`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurnRate {
    /// Every token, cache reads included, per minute of work.
    pub tokens_per_minute: f64,
    /// Fresh input and output only, per minute of work.
    pub indicator_tokens_per_minute: f64,
    /// What an hour at this rate costs.
    pub cost_per_hour: Usd,
}

impl BurnRate {
    /// The rate `block` was consumed at, or `None` when there is no rate to
    /// measure.
    ///
    /// Measured across the stretch between the block's **first and last
    /// response**, not across its five hours. A block that saw a burst of work
    /// and then went quiet was genuinely burning fast during the burst;
    /// averaging that over four idle hours would report a calm number for the
    /// busiest half-hour of somebody's day, which is precisely the moment the
    /// figure is being looked at.
    ///
    /// `None` for an empty block, a gap, a single response, or any stretch
    /// that is not positive. Each of those is a division by zero waiting to
    /// happen, and a rate of infinity on a dashboard is worse than no rate at
    /// all.
    ///
    /// [`Self::indicator_tokens_per_minute`] excludes cache traffic on
    /// purpose. A cached read costs a tenth of a fresh input token -- a
    /// fortieth on the newest models -- so a replay of a long conversation
    /// moves millions of tokens while spending almost nothing. Letting those
    /// drive the indicator would paint every resumed session as a crisis.
    #[must_use]
    pub fn measure(block: &BillingBlock) -> Option<Self> {
        if block.kind == BlockKind::Gap || block.entries < 2 {
            return None;
        }
        let first = block.first_activity_at?;
        let last = block.last_activity_at?;
        let minutes = (last - first).num_seconds() as f64 / SECONDS_PER_MINUTE;
        if minutes <= 0.0 {
            return None;
        }
        let indicator = block.tokens.input + block.tokens.output;
        Some(Self {
            tokens_per_minute: block.tokens.total() as f64 / minutes,
            indicator_tokens_per_minute: indicator as f64 / minutes,
            cost_per_hour: Usd::new(block.cost.dollars() / minutes * MINUTES_PER_HOUR),
        })
    }

    /// Which band this rate falls in.
    ///
    /// Read off [`Self::indicator_tokens_per_minute`], for the reason given on
    /// [`Self::measure`]: the band is meant to say how hard the account is
    /// being worked, and cache reads say almost nothing about that.
    #[must_use]
    pub fn intensity(self) -> Intensity {
        if self.indicator_tokens_per_minute >= Intensity::HIGH_FROM {
            Intensity::High
        } else if self.indicator_tokens_per_minute >= Intensity::MODERATE_FROM {
            Intensity::Moderate
        } else {
            Intensity::Normal
        }
    }
}

/// Seconds in a minute, named so the arithmetic reads as arithmetic.
const SECONDS_PER_MINUTE: f64 = 60.0;

/// Minutes in an hour, likewise.
const MINUTES_PER_HOUR: f64 = 60.0;

/// How hard an account is being worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intensity {
    /// Ordinary work.
    Normal,
    /// Busy, and worth a glance at the projection.
    Moderate,
    /// Fast enough that a five-hour window is likely to be a problem.
    High,
}

impl Intensity {
    /// Where [`Intensity::Moderate`] begins, in indicator tokens per minute.
    pub const MODERATE_FROM: f64 = 2_000.0;
    /// Where [`Intensity::High`] begins, in indicator tokens per minute.
    pub const HIGH_FROM: f64 = 5_000.0;
}

/// Where a live block lands if it carries on at its current rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// Tokens the block is expected to hold by the time its window closes.
    pub total_tokens: u64,
    /// What that is expected to cost.
    pub cost: Usd,
}

impl Projection {
    /// Where `block` lands at `rate`, or `None` when there is nothing left to
    /// project into.
    ///
    /// `None` for a closed block and for a gap. A finished window cannot grow,
    /// and projecting one forward would put a number on a table that describes
    /// a future that has already happened.
    #[must_use]
    pub fn of(block: &BillingBlock, rate: BurnRate, now: DateTime<Utc>) -> Option<Self> {
        if block.kind != BlockKind::Active {
            return None;
        }
        // Clamped at zero rather than trusted: a block is only active while
        // `now` is inside its window, but a caller holding a block it built
        // itself deserves a projection of "nothing more", not a negative one.
        let remaining = (block.ends_at - now).num_minutes().max(0) as f64;
        Some(Self {
            total_tokens: (block.tokens.total() as f64 + rate.tokens_per_minute * remaining).round()
                as u64,
            cost: Usd::new(
                block.cost.dollars() + rate.cost_per_hour.dollars() / MINUTES_PER_HOUR * remaining,
            ),
        })
    }
}

/// How a projection sits against a token limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitStanding {
    /// Comfortably inside it.
    Ok,
    /// Past four fifths of it, which is the point at which a plan is worth
    /// changing rather than abandoning.
    Warning,
    /// Expected to go over.
    Exceeds,
}

impl LimitStanding {
    /// Where four fifths of the limit sits, as a fraction.
    ///
    /// A warning band rather than a single line, because the useful moment to
    /// tell somebody is while there is still time to finish the thought.
    const WARNING_FROM: f64 = 0.8;

    /// How `projected_tokens` stands against `limit`.
    ///
    /// Both comparisons are strict, so a projection landing exactly on the
    /// limit is a warning rather than an exceedance, and one landing exactly
    /// on four fifths of it is still [`Self::Ok`].
    #[must_use]
    pub fn of(projected_tokens: u64, limit: u64) -> Self {
        if projected_tokens > limit {
            Self::Exceeds
        } else if projected_tokens as f64 > limit as f64 * Self::WARNING_FROM {
            Self::Warning
        } else {
            Self::Ok
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entry::EntryId;
    use crate::domain::project::{Project, SessionId};

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    /// The span every test below is written against.
    fn five_hours() -> Duration {
        Duration::hours(DEFAULT_SPAN_HOURS)
    }

    /// One response at `when`, carrying `tokens` fresh input tokens.
    fn entry(id: &str, when: &str, input: u64) -> Entry {
        priced(
            id,
            when,
            TokenUsage {
                input,
                ..TokenUsage::ZERO
            },
        )
    }

    /// One response at `when`, carrying whatever counters a test needs.
    fn priced(id: &str, when: &str, tokens: TokenUsage) -> Entry {
        Entry {
            id: EntryId {
                message_id: id.to_owned(),
                request_id: Some(format!("req_{id}")),
                session: SessionId::new("session-a"),
            },
            at: at(when),
            model: ModelId::new("claude-opus-5"),
            tokens,
            recorded_cost: None,
            session: SessionId::new("session-a"),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    /// The blocks `entries` fall into, as of `now`.
    fn blocks(entries: &[Entry], now: &str) -> Vec<BillingBlock> {
        identify(
            entries,
            five_hours(),
            at(now),
            CostMode::Calculate,
            &PriceSheet::builtin(),
        )
    }

    #[test]
    fn a_block_starts_at_the_hour_the_first_entry_fell_in_not_at_the_entry_itself() {
        // The limit window is anchored to the clock. A block that began with a
        // message at 09:47 resets at 14:00, and a report claiming 14:47 would
        // have somebody waiting three-quarters of an hour for a reset that had
        // already happened.
        let found = blocks(
            &[entry("a", "2026-09-01T09:47:31.250Z", 1_000)],
            "2026-09-01T10:00:00Z",
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].started_at, at("2026-09-01T09:00:00Z"));
        assert_eq!(found[0].ends_at, at("2026-09-01T14:00:00Z"));
        assert_eq!(
            found[0].last_activity_at,
            Some(at("2026-09-01T09:47:31.250Z")),
            "the window is rounded, the activity stamp is not"
        );
    }

    #[test]
    fn an_entry_exactly_on_the_hour_starts_the_block_at_that_hour() {
        // The flooring must be idempotent: an instant already on the hour is
        // its own floor, not the hour before it.
        let found = blocks(
            &[entry("a", "2026-09-01T09:00:00Z", 1_000)],
            "2026-09-01T10:00:00Z",
        );

        assert_eq!(found[0].started_at, at("2026-09-01T09:00:00Z"));
        assert_eq!(found[0].ends_at, at("2026-09-01T14:00:00Z"));
    }

    #[test]
    fn an_entry_exactly_five_hours_after_the_start_still_belongs_to_it() {
        // The first of the two strict comparisons. The block starts at 09:00
        // and ends at 14:00; a response at exactly 14:00 is five hours after
        // the start, which is not *more* than five hours, so it stays in.
        let found = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 1_000),
                entry("b", "2026-09-01T14:00:00Z", 1_000),
            ],
            "2026-09-01T14:05:00Z",
        );

        assert_eq!(found.len(), 1, "one block, not two");
        assert_eq!(found[0].entries, 2);
        assert_eq!(found[0].last_activity_at, Some(at("2026-09-01T14:00:00Z")));

        // One second later and it is a second block, which is what makes the
        // assertion above about the boundary rather than about the arithmetic.
        let split = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 1_000),
                entry("b", "2026-09-01T14:00:01Z", 1_000),
            ],
            "2026-09-01T14:05:00Z",
        );
        assert_eq!(split.len(), 2);
    }

    #[test]
    fn a_gap_of_exactly_five_hours_does_not_close_the_block() {
        // The second strict comparison, isolated from the first by starting
        // the block on the hour so that both boundaries fall on the same
        // instant. Five hours of silence exactly is not *more* than five
        // hours, so the block survives it.
        let found = blocks(
            &[
                entry("a", "2026-09-01T09:00:00Z", 1_000),
                entry("b", "2026-09-01T14:00:00Z", 1_000),
            ],
            "2026-09-01T14:05:00Z",
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].entries, 2);
    }

    #[test]
    fn a_gap_of_more_than_five_hours_closes_the_block_even_early_in_it() {
        // The silence starts at the very first response of the window rather
        // than deep inside it, which is the case worth pinning: the second
        // condition is measured from the response before, so it has to fire on
        // a block holding a single entry as readily as on a busy one. What it
        // buys is the gap row -- the first condition would have closed this
        // block anyway, but only the second says the hours in between were
        // silence rather than work, and an overnight break counted as one long
        // sitting would report a burn rate averaged over the time somebody was
        // asleep.
        let overnight = [
            entry("a", "2026-09-01T09:00:00Z", 1_000),
            entry("b", "2026-09-01T14:00:01Z", 1_000),
        ];
        let found = blocks(&overnight, "2026-09-01T15:00:00Z");

        assert_eq!(found.len(), 3, "two blocks with the gap between them");
        assert_eq!(found[0].entries, 1);
        assert_eq!(found[2].entries, 1);
        assert_eq!(
            found[2].started_at,
            at("2026-09-01T14:00:00Z"),
            "the second block is anchored to its own hour"
        );
    }

    #[test]
    fn a_break_in_work_is_recorded_as_a_gap_block_rather_than_silently_skipped() {
        // A reader scanning a column of blocks needs to see that nothing
        // happened between Tuesday evening and Wednesday morning. Left out,
        // that fact has to be worked out by subtracting two timestamps, which
        // is exactly the arithmetic a table exists to save somebody.
        let found = blocks(
            &[
                entry("a", "2026-09-01T18:30:00Z", 1_000),
                entry("b", "2026-09-02T09:15:00Z", 1_000),
            ],
            "2026-09-02T10:00:00Z",
        );

        assert_eq!(found.len(), 3);
        let gap = &found[1];
        assert_eq!(gap.kind, BlockKind::Gap);
        assert_eq!(
            gap.started_at,
            at("2026-09-01T23:30:00Z"),
            "the gap opens a span after the last response, not at the block's end"
        );
        assert_eq!(gap.ends_at, at("2026-09-02T09:15:00Z"));
        assert_eq!(gap.tokens, TokenUsage::ZERO);
        assert_eq!(gap.cost, Usd::ZERO);
        assert_eq!(gap.entries, 0);
        assert!(gap.models.is_empty());
        assert_eq!(gap.last_activity_at, None);
    }

    #[test]
    fn entries_arriving_out_of_order_are_sorted_before_they_are_blocked() {
        // A corpus is a pile of files and nothing promises the order they were
        // read in. Blocking an unsorted stream would open a new window at
        // every step backwards in time and invent gaps that never happened.
        let chronological = [
            entry("a", "2026-09-01T09:10:00Z", 1_000),
            entry("b", "2026-09-01T11:20:00Z", 2_000),
            entry("c", "2026-09-02T09:30:00Z", 3_000),
        ];
        let expected = blocks(&chronological, "2026-09-02T10:00:00Z");

        let mut reversed = chronological.clone();
        reversed.reverse();
        assert_eq!(blocks(&reversed, "2026-09-02T10:00:00Z"), expected);

        let shuffled = [
            chronological[1].clone(),
            chronological[2].clone(),
            chronological[0].clone(),
        ];
        assert_eq!(blocks(&shuffled, "2026-09-02T10:00:00Z"), expected);
        assert_eq!(expected.len(), 3, "two blocks and the gap between them");
    }

    #[test]
    fn a_block_whose_five_hours_have_elapsed_is_finished_however_recent_the_last_entry() {
        // The window closes on the clock. Work at 13:59 does not extend a
        // block that started at 09:00 past 14:00, and reporting it as still
        // running would have somebody believe an allowance was being consumed
        // that had already rolled forward.
        let found = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 1_000),
                entry("b", "2026-09-01T13:59:00Z", 1_000),
            ],
            "2026-09-01T14:00:00Z",
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, BlockKind::Closed);

        // One minute earlier and the same block is live, so the assertion
        // above is about the boundary rather than about the fixture.
        let live = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 1_000),
                entry("b", "2026-09-01T13:59:00Z", 1_000),
            ],
            "2026-09-01T13:59:59Z",
        );
        assert_eq!(live[0].kind, BlockKind::Active);
    }

    #[test]
    fn a_block_untouched_for_five_hours_is_finished_even_before_its_window_closes() {
        // Asked of the rule directly, because a block this module builds
        // cannot be in this state: its start is floored back to the hour, so
        // its window always closes before its last response is a full span
        // old. The rule is about the block rather than about how it was
        // assembled -- the live dashboard holds a window it built itself --
        // and the answer must be the same answer.
        let stale = BillingBlock {
            started_at: at("2026-09-01T09:00:00Z"),
            ends_at: at("2026-09-01T20:00:00Z"),
            last_activity_at: Some(at("2026-09-01T09:30:00Z")),
            first_activity_at: Some(at("2026-09-01T09:30:00Z")),
            tokens: TokenUsage::ZERO,
            cost: Usd::ZERO,
            models: Vec::new(),
            entries: 1,
            kind: BlockKind::Closed,
        };

        assert!(
            !stale.is_live(at("2026-09-01T14:30:00Z"), five_hours()),
            "five hours of silence finishes it, though the window runs to 20:00"
        );
        assert!(
            stale.is_live(at("2026-09-01T14:29:59Z"), five_hours()),
            "one second short of five hours it is still running"
        );
    }

    #[test]
    fn a_gap_block_is_never_active() {
        // Nothing happened in it, so there is nothing in it to still be
        // happening -- however recent the gap and however open its window.
        let found = blocks(
            &[
                entry("a", "2026-09-01T09:00:00Z", 1_000),
                entry("b", "2026-09-02T09:00:00Z", 1_000),
            ],
            "2026-09-02T09:30:00Z",
        );

        let gap = &found[1];
        assert_eq!(gap.kind, BlockKind::Gap);
        assert!(!gap.is_live(at("2026-09-01T15:00:00Z"), five_hours()));
        assert_eq!(
            found[2].kind,
            BlockKind::Active,
            "the block after it is the live one"
        );
    }

    #[test]
    fn the_burn_rate_is_measured_across_the_entries_not_across_the_block() {
        // Two responses twenty minutes apart in a block that started at 09:00:
        // the work took twenty minutes, and measuring it against the window
        // instead would divide by fifty and report a third of the real rate,
        // calmly, at the busiest moment of somebody's morning.
        let found = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 100_000),
                entry("b", "2026-09-01T09:50:00Z", 100_000),
            ],
            "2026-09-01T10:00:00Z",
        );
        let rate = BurnRate::measure(&found[0]).expect("two responses twenty minutes apart");

        assert!(
            (rate.tokens_per_minute - 10_000.0).abs() < 1e-9,
            "200,000 tokens over twenty minutes: {}",
            rate.tokens_per_minute
        );
        // Opus 5 charges $5 per million input, so 200,000 tokens is $1.00 and
        // twenty minutes of it is $3.00 an hour.
        assert!(
            (rate.cost_per_hour.dollars() - 3.0).abs() < 1e-9,
            "got {}",
            rate.cost_per_hour.dollars()
        );
    }

    #[test]
    fn a_block_with_one_entry_has_no_burn_rate_rather_than_dividing_by_zero() {
        // There is no stretch to measure across, and a rate of infinity on a
        // dashboard is worse than an empty cell.
        let single = blocks(
            &[entry("a", "2026-09-01T09:30:00Z", 100_000)],
            "2026-09-01T10:00:00Z",
        );
        assert_eq!(BurnRate::measure(&single[0]), None);

        // Two responses in the same second are the same problem wearing a
        // different hat.
        let simultaneous = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 100_000),
                entry("b", "2026-09-01T09:30:00Z", 100_000),
            ],
            "2026-09-01T10:00:00Z",
        );
        assert_eq!(BurnRate::measure(&simultaneous[0]), None);

        let gap = blocks(
            &[
                entry("a", "2026-09-01T09:00:00Z", 1_000),
                entry("b", "2026-09-02T09:00:00Z", 1_000),
            ],
            "2026-09-02T09:30:00Z",
        );
        assert_eq!(BurnRate::measure(&gap[1]), None, "a gap has no rate either");
    }

    #[test]
    fn the_burn_indicator_ignores_cache_traffic_so_it_tracks_real_work() {
        // A resumed conversation replays its whole prefix out of the cache:
        // millions of tokens, a fortieth of the price, and no more work being
        // done than usual. Letting those drive the indicator would paint every
        // resumed session as a crisis.
        let replay = [
            priced(
                "a",
                "2026-09-01T09:30:00Z",
                TokenUsage {
                    input: 2_500,
                    cache_read: 495_000,
                    cache_write_5m: 0,
                    cache_write_1h: 0,
                    output: 2_500,
                },
            ),
            priced(
                "b",
                "2026-09-01T09:40:00Z",
                TokenUsage {
                    input: 2_500,
                    cache_read: 495_000,
                    cache_write_5m: 0,
                    cache_write_1h: 0,
                    output: 2_500,
                },
            ),
        ];
        let found = blocks(&replay, "2026-09-01T10:00:00Z");
        let rate = BurnRate::measure(&found[0]).expect("two responses ten minutes apart");

        assert!(
            (rate.tokens_per_minute - 100_000.0).abs() < 1e-9,
            "a million tokens over ten minutes: {}",
            rate.tokens_per_minute
        );
        assert!(
            (rate.indicator_tokens_per_minute - 1_000.0).abs() < 1e-9,
            "ten thousand of them were fresh: {}",
            rate.indicator_tokens_per_minute
        );
        assert_eq!(
            rate.intensity(),
            Intensity::Normal,
            "99% cache reads is a quiet block, not a crisis"
        );
    }

    #[test]
    fn the_burn_bands_line_up_with_the_documented_thresholds() {
        // Asserted on the boundaries themselves, because a band is a promise
        // about exactly two numbers and every other value in it is a promise
        // about nothing.
        let banded = |indicator: f64| {
            BurnRate {
                tokens_per_minute: indicator,
                indicator_tokens_per_minute: indicator,
                cost_per_hour: Usd::ZERO,
            }
            .intensity()
        };

        assert_eq!(banded(0.0), Intensity::Normal);
        assert_eq!(banded(1_999.999), Intensity::Normal);
        assert_eq!(banded(2_000.0), Intensity::Moderate);
        assert_eq!(banded(4_999.999), Intensity::Moderate);
        assert_eq!(banded(5_000.0), Intensity::High);
        assert_eq!(banded(50_000.0), Intensity::High);
    }

    #[test]
    fn a_finished_block_is_not_projected_forward() {
        // A window that has closed cannot grow, and a projection on it would
        // be a figure describing a future that has already happened.
        let found = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 100_000),
                entry("b", "2026-09-01T09:50:00Z", 100_000),
            ],
            "2026-09-01T20:00:00Z",
        );
        let rate = BurnRate::measure(&found[0]).expect("two responses twenty minutes apart");

        assert_eq!(found[0].kind, BlockKind::Closed);
        assert_eq!(
            Projection::of(&found[0], rate, at("2026-09-01T20:00:00Z")),
            None
        );

        // The same block while it was still running does project.
        let live = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 100_000),
                entry("b", "2026-09-01T09:50:00Z", 100_000),
            ],
            "2026-09-01T10:00:00Z",
        );
        let projected = Projection::of(&live[0], rate, at("2026-09-01T10:00:00Z"))
            .expect("four hours of window left");
        // 200,000 tokens so far, plus 10,000 a minute for the 240 minutes to
        // 14:00, is 2,600,000; the cost follows the same four hours at $3.00
        // an hour on top of the $1.00 already spent.
        assert_eq!(projected.total_tokens, 2_600_000);
        assert!(
            (projected.cost.dollars() - 13.0).abs() < 1e-9,
            "got {}",
            projected.cost.dollars()
        );
    }

    #[test]
    fn a_projection_past_the_limit_is_reported_as_exceeding_rather_than_capped() {
        // The whole point of the figure is that it can be larger than the
        // allowance. Capping it at the limit would turn the one number that
        // answers "will this run out" into a number that never can.
        assert_eq!(LimitStanding::of(1_400, 1_000), LimitStanding::Exceeds);
        assert_eq!(LimitStanding::of(1_001, 1_000), LimitStanding::Exceeds);
        // Exactly on the limit is a warning, not an exceedance: it has not
        // gone over.
        assert_eq!(LimitStanding::of(1_000, 1_000), LimitStanding::Warning);
        assert_eq!(LimitStanding::of(801, 1_000), LimitStanding::Warning);
        // And exactly four fifths of it is still comfortable, which is the
        // boundary most likely to be written the wrong way round.
        assert_eq!(LimitStanding::of(800, 1_000), LimitStanding::Ok);
        assert_eq!(LimitStanding::of(0, 1_000), LimitStanding::Ok);
    }

    #[test]
    fn a_response_carrying_no_tokens_still_opens_a_block() {
        // It is still activity, and the window it started is still consuming
        // the allowance. Skipping it would report the block as beginning at
        // whatever hour the next response fell in, up to five hours late.
        let found = blocks(
            &[entry("a", "2026-09-01T09:30:00Z", 0)],
            "2026-09-01T10:00:00Z",
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].entries, 1);
        assert_eq!(found[0].started_at, at("2026-09-01T09:00:00Z"));
    }

    #[test]
    fn a_block_names_its_models_in_the_order_they_first_appeared() {
        // Two responses on one model and one on another, so the assertion is
        // about first appearance rather than about the order they happen to
        // hash into.
        let mut sonnet = entry("b", "2026-09-01T09:40:00Z", 1_000);
        sonnet.model = ModelId::new("claude-sonnet-5");
        let found = blocks(
            &[
                entry("a", "2026-09-01T09:30:00Z", 1_000),
                sonnet,
                entry("c", "2026-09-01T09:50:00Z", 1_000),
            ],
            "2026-09-01T10:00:00Z",
        );

        assert_eq!(
            found[0].models,
            vec![
                ModelId::new("claude-opus-5"),
                ModelId::new("claude-sonnet-5"),
            ]
        );
        assert_eq!(found[0].entries, 3);
    }

    #[test]
    fn an_empty_corpus_produces_no_blocks_rather_than_an_empty_one() {
        assert!(blocks(&[], "2026-09-01T10:00:00Z").is_empty());
    }
}
