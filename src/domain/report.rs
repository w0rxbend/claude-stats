//! The aggregate every period report is built from: rows of usage, one per
//! bucket, and the total underneath them.
//!
//! This is the read model the daily, weekly, monthly, per-project and
//! per-session commands all return. There is exactly one of it, and exactly
//! one function that produces it, for a reason worth stating plainly: the
//! moment a text table and a JSON document each fold over the entries in their
//! own way, they eventually disagree about what a week cost, and a pair of
//! figures that disagree is worse than either of them being wrong alone. A
//! reader can check one number. They cannot check two numbers that contradict
//! each other.
//!
//! In Fowler's vocabulary this is a Transform View's input rather than a
//! Domain Model: nothing here has a lifecycle, nothing is mutated after it is
//! built, and the whole structure exists to be rendered. What keeps it in the
//! domain layer rather than beside the renderer is that the *rules* are here --
//! what a bucket is, what belongs in it, which order the models come out in --
//! and those are the parts that must be identical for every audience.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::entry::Entry;
use super::model::ModelId;
use super::money::Usd;
use super::period::{GroupingSpec, Order, PeriodKey};
use super::pricing::{CostMode, PriceSheet};
use super::project::{Project, SessionId};
use super::tokens::TokenUsage;

/// What one model contributed to one row.
///
/// Present so that a row's total can be taken apart. A figure nobody can break
/// down is a figure nobody can check, and "why was Tuesday four times
/// Monday" is answered by this and by nothing else on the row.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelBreakdown {
    /// The model, as the transcript spelled it.
    pub model: ModelId,
    /// Its share of the row's counters.
    pub tokens: TokenUsage,
    /// Its share of the row's cost.
    pub cost: Usd,
}

/// One bucket of usage: a period, optionally a project, optionally a session.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    /// The calendar bucket, or [`PeriodKey::none`] for a report with no time
    /// axis.
    pub key: PeriodKey,
    /// The working directory this row is about.
    ///
    /// Present for two different reasons that happen to want the same field. A
    /// report split by project carries the directory that *defines* the row; a
    /// per-session report carries the directory its conversation was started
    /// in, which [`remember_home`] notes without splitting anything.
    pub project: Option<Project>,
    /// The conversation, when the report is split by session.
    pub session: Option<SessionId>,
    /// Every token counted into this bucket.
    pub tokens: TokenUsage,
    /// What those tokens cost, under the report's cost mode and price sheet.
    pub cost: Usd,
    /// The models that contributed, in the order they were first seen.
    ///
    /// First-seen rather than sorted, because this is the column a reader
    /// scans down to see *what changed*: a model appearing partway through a
    /// week is visible as a new name arriving at the bottom of a cell, and
    /// sorting would scatter that signal.
    pub models: Vec<ModelId>,
    /// The same models with their figures, dearest first.
    ///
    /// Ordered by cost rather than by name or by first appearance because the
    /// question a breakdown answers is "where did the money go", and the
    /// answer is the first line.
    pub breakdown: Vec<ModelBreakdown>,
    /// The earliest response in the bucket, when the report tracks it.
    pub first_activity_at: Option<DateTime<Utc>>,
    /// The latest response in the bucket, when the report tracks it.
    pub last_activity_at: Option<DateTime<Utc>>,
}

impl UsageRow {
    /// An empty row carrying only its identity.
    fn opened(key: PeriodKey, project: Option<Project>, session: Option<SessionId>) -> Self {
        Self {
            key,
            project,
            session,
            tokens: TokenUsage::ZERO,
            cost: Usd::ZERO,
            models: Vec::new(),
            breakdown: Vec::new(),
            first_activity_at: None,
            last_activity_at: None,
        }
    }

    /// How this row sorts against another when the order is by calendar.
    fn calendar_order(&self, other: &Self) -> std::cmp::Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.project.cmp(&other.project))
            .then_with(|| self.session.cmp(&other.session))
    }
}

/// Rows and their total, ready to be rendered.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageReport {
    /// One row per bucket that had any activity at all.
    ///
    /// A bucket with no activity is **absent**, never present with zeroes.
    /// Silence and zero mean different things to somebody reading a table:
    /// a missing Sunday says "nothing happened, or nothing was recorded", a
    /// Sunday reading `0` says "the tool looked and is certain the answer is
    /// nothing". Only the first of those is true of a corpus that is a pile of
    /// files, any of which may not have been written yet.
    pub rows: Vec<UsageRow>,
    /// The sum of [`Self::rows`], computed rather than restated.
    pub totals: UsageRow,
}

impl UsageReport {
    /// Folds `entries` into the report `spec` asks for.
    ///
    /// The single place aggregation happens. Every command that prints a
    /// period table calls this, so two of them can only print different
    /// figures if they were handed different entries.
    ///
    /// Buckets are keyed by the period, the project and the session -- each
    /// included only when [`GroupingSpec`] asked for it -- and a bucket nothing
    /// fell into does not appear. Costs are taken through
    /// [`CostMode::cost_of`] so that the choice of how a cost is arrived at is
    /// made once, at the composition root, rather than differently in each
    /// report.
    ///
    /// Rows come out in calendar order, reversed for [`Order::Descending`],
    /// except for a per-session report: those are always dearest first,
    /// whatever order was asked for. That is not a quiet override for its own
    /// sake. A session table is read to find the run that cost the money, its
    /// keys are opaque UUIDs that sort into no meaningful order, and every
    /// tool these figures get compared against does the same.
    #[must_use]
    pub fn build(
        entries: &[Entry],
        spec: &GroupingSpec,
        zone: &super::period::Zone,
        mode: CostMode,
        sheet: &PriceSheet,
    ) -> Self {
        // An Identity Map over the rows being built: the map answers "have I
        // opened this bucket already" in constant time while the vector keeps
        // the rows in the order they were first opened. A lookup by scanning
        // the vector would turn a month of traffic into a quadratic fold, and
        // sorting the entries first would make the first-seen model order an
        // accident of the sort rather than a property of the data.
        let mut index: HashMap<BucketKey, usize> = HashMap::new();
        let mut rows: Vec<UsageRow> = Vec::new();
        // One breakdown index per row, kept alongside rather than on the row,
        // because it is scaffolding for the fold and nothing a reader of the
        // finished report should have to look at.
        let mut model_index: Vec<HashMap<ModelId, usize>> = Vec::new();

        for entry in entries {
            let bucket = BucketKey {
                key: spec
                    .period
                    .map_or_else(PeriodKey::none, |period| period.key_of(entry.at, zone)),
                project: spec.by_project.then(|| entry.project.clone()),
                session: spec.by_session.then(|| entry.session.clone()),
            };
            let at = *index.entry(bucket.clone()).or_insert_with(|| {
                rows.push(UsageRow::opened(bucket.key, bucket.project, bucket.session));
                model_index.push(HashMap::new());
                rows.len() - 1
            });

            let cost = mode.cost_of(entry, sheet);
            let row = &mut rows[at];
            add_usage(&mut row.tokens, &mut row.cost, entry.tokens, cost);
            // Activity stamps are only carried where a report is going to show
            // them, which today means a per-session table. Stamping every row
            // would cost nothing but would put two columns on a daily table
            // that repeat what its own key already says.
            if spec.by_session {
                stamp_activity(row, entry.at);
                remember_home(row, &entry.project);
            }

            let breakdown = &mut row.breakdown;
            let slot = *model_index[at]
                .entry(entry.model.clone())
                .or_insert_with(|| {
                    breakdown.push(ModelBreakdown {
                        model: entry.model.clone(),
                        tokens: TokenUsage::ZERO,
                        cost: Usd::ZERO,
                    });
                    breakdown.len() - 1
                });
            let share = &mut row.breakdown[slot];
            add_usage(&mut share.tokens, &mut share.cost, entry.tokens, cost);
        }

        for row in &mut rows {
            finish(row);
        }

        // Totalled while the rows are still in the order they were opened, and
        // deliberately before they are sorted. Two reasons, and the second is
        // the one that bites. The models column of the total comes out in the
        // order the corpus first saw them rather than in the order the reader
        // happened to ask the rows for; and the cost is folded over the same
        // sequence whichever way round the table is printed. `f64` addition is
        // not associative, so summing the rows after reversing them can differ
        // in the last bits -- which is a report that prints two different
        // totals for the same traffic depending on a flag that was only ever
        // meant to change the order of the lines.
        let totals = total_of(&rows);

        // Sessions are ranked by spend; everything else reads down the
        // calendar.
        if spec.by_session {
            rows.sort_by(dearest_first);
        } else {
            rows.sort_by(UsageRow::calendar_order);
            if spec.order == Order::Descending {
                rows.reverse();
            }
        }

        Self { rows, totals }
    }
}

/// What makes two entries belong in the same row.
///
/// Each part is `None`-shaped when the report did not ask to split on it, so
/// a daily report has one bucket per day and a daily-per-project report has
/// one per day per directory, from the same code.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BucketKey {
    key: PeriodKey,
    project: Option<Project>,
    session: Option<SessionId>,
}

/// Adds one response's counters and cost into a running pair.
///
/// The cost goes through [`Usd::total`] rather than `+=` so that every sum in
/// the crate is the same deliberate left fold. Folding an accumulator with the
/// new amount is arithmetically identical to folding the whole sequence at the
/// end -- adding [`Usd::ZERO`] first cannot change an `f64` -- and it costs no
/// memory, which matters when the sequence is several hundred thousand long.
fn add_usage(tokens: &mut TokenUsage, cost: &mut Usd, added: TokenUsage, added_cost: Usd) {
    *tokens += added;
    *cost = Usd::total([*cost, added_cost]);
}

/// Notes which directory a session belongs to, the first time it is asked.
///
/// A session row is expected to name a project, and until now the only way it
/// could was to be split by one -- which is precisely what it must not be. A
/// session's helpers do not all run where the conversation started: Claude Code
/// gives a workflow its own git worktree, so one session on this machine
/// recorded sixty-five different working directories and would come out as
/// sixty-five rows, each carrying the same `sessionId` and a fraction of the
/// spend. The dearest conversation on that corpus cost $1,144 and appeared
/// fourth in the ranking at $573.
///
/// So the directory is remembered *beside* the bucket rather than folded into
/// it. The one kept is the first offered, and since the repository hands its
/// entries over oldest first that is the directory the conversation was
/// started in -- the parent's own working directory, not a worktree some
/// sub-agent was handed twenty minutes later. A row already carrying a project
/// is left alone, so a report genuinely split by directory keeps the one that
/// defines its bucket.
fn remember_home(row: &mut UsageRow, project: &Project) {
    if row.project.is_none() {
        row.project = Some(project.clone());
    }
}

/// Records `at` as the row's earliest or latest activity, whichever it is.
fn stamp_activity(row: &mut UsageRow, at: DateTime<Utc>) {
    row.first_activity_at = Some(row.first_activity_at.map_or(at, |first| first.min(at)));
    row.last_activity_at = Some(row.last_activity_at.map_or(at, |last| last.max(at)));
}

/// Fixes a row's model columns once nothing more will be added to it.
///
/// [`UsageRow::models`] is taken *before* the breakdown is sorted, which is
/// how the two columns come to hold the same models in two different orders on
/// purpose: first-seen for the column a reader scans, dearest-first for the
/// breakdown that answers where the money went.
fn finish(row: &mut UsageRow) {
    row.models = row
        .breakdown
        .iter()
        .map(|share| share.model.clone())
        .collect();
    row.breakdown.sort_by(|a, b| {
        b.cost
            .dollars()
            .total_cmp(&a.cost.dollars())
            .then_with(|| a.model.cmp(&b.model))
    });
}

/// Ranks two rows by spend, dearest first.
///
/// The tie-break down to the identity is not decoration: two sessions that
/// cost exactly the same are common in a corpus full of tiny sub-agent runs,
/// and without it their order would come out of a hash map and change between
/// runs for no reason a user could explain.
fn dearest_first(a: &UsageRow, b: &UsageRow) -> std::cmp::Ordering {
    b.cost
        .dollars()
        .total_cmp(&a.cost.dollars())
        .then_with(|| a.calendar_order(b))
}

/// The row underneath the table.
///
/// Summed from the rows rather than from the entries a second time. Two folds
/// over the same data are two chances to fold differently, and a total that
/// does not tie out to the column above it is the single fastest way to lose a
/// reader's trust in every other figure on the page.
fn total_of(rows: &[UsageRow]) -> UsageRow {
    let mut totals = UsageRow::opened(PeriodKey::none(), None, None);
    let mut model_index: HashMap<ModelId, usize> = HashMap::new();

    for row in rows {
        totals.tokens += row.tokens;
        if let Some(first) = row.first_activity_at {
            totals.first_activity_at = Some(
                totals
                    .first_activity_at
                    .map_or(first, |held| held.min(first)),
            );
        }
        if let Some(last) = row.last_activity_at {
            totals.last_activity_at =
                Some(totals.last_activity_at.map_or(last, |held| held.max(last)));
        }
        // The models column of the total is the union of the rows' own, still
        // in first-seen order, so a model that only ever appeared on one day
        // is still visible at the bottom of the table.
        //
        // Walked down `models` rather than down `breakdown`, because by this
        // point `finish` has already sorted each row's breakdown by cost.
        // Reading the shares out of it would hand the total the *dearest*
        // model first and quietly contradict the sentence above -- the two
        // columns are meant to hold the same models in two different orders on
        // purpose, and the total is meant to agree with the rows about which
        // order is which.
        for share in row
            .models
            .iter()
            .filter_map(|model| row.breakdown.iter().find(|share| &share.model == model))
        {
            let slot = *model_index.entry(share.model.clone()).or_insert_with(|| {
                totals.breakdown.push(ModelBreakdown {
                    model: share.model.clone(),
                    tokens: TokenUsage::ZERO,
                    cost: Usd::ZERO,
                });
                totals.breakdown.len() - 1
            });
            let into = &mut totals.breakdown[slot];
            into.tokens += share.tokens;
            into.cost = Usd::total([into.cost, share.cost]);
        }
    }
    totals.cost = Usd::total(rows.iter().map(|row| row.cost));
    finish(&mut totals);
    totals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entry::EntryId;
    use crate::domain::period::{AggregationPeriod, Zone};

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    /// One response, spelled out so a test can vary exactly one thing.
    fn entry(id: &str, when: &str, model: &str, session: &str, tokens: TokenUsage) -> Entry {
        Entry {
            id: EntryId {
                message_id: id.to_owned(),
                request_id: Some(format!("req_{id}")),
                session: SessionId::new(session),
            },
            at: at(when),
            model: ModelId::new(model),
            tokens,
            recorded_cost: None,
            session: SessionId::new(session),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    fn input(count: u64) -> TokenUsage {
        TokenUsage {
            input: count,
            ..TokenUsage::ZERO
        }
    }

    fn daily() -> GroupingSpec {
        GroupingSpec {
            period: Some(AggregationPeriod::Day),
            ..GroupingSpec::default()
        }
    }

    fn report(entries: &[Entry], spec: &GroupingSpec) -> UsageReport {
        UsageReport::build(
            entries,
            spec,
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        )
    }

    #[test]
    fn a_day_with_no_activity_is_absent_rather_than_present_with_zeroes() {
        // Monday and Wednesday were worked; Tuesday was not. A Tuesday row
        // reading zero would claim the tool looked and is sure nothing
        // happened, which is a stronger statement than a pile of files can
        // support.
        let entries = [
            entry("a", "2026-09-01T09:00:00Z", "claude-opus-5", "s1", input(1)),
            entry("b", "2026-09-03T09:00:00Z", "claude-opus-5", "s1", input(1)),
        ];
        let report = report(&entries, &daily());

        assert_eq!(report.rows.len(), 2);
        assert_eq!(report.rows[0].key.as_str(), "2026-09-01");
        assert_eq!(report.rows[1].key.as_str(), "2026-09-03");
        assert!(
            !report
                .rows
                .iter()
                .any(|row| row.key.as_str() == "2026-09-02"),
            "an empty Tuesday must not be invented"
        );
    }

    #[test]
    fn the_totals_row_is_the_sum_of_the_rows_above_it() {
        let entries = [
            entry(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-opus-5",
                "s1",
                input(1_000),
            ),
            entry(
                "b",
                "2026-09-02T09:00:00Z",
                "claude-sonnet-5",
                "s2",
                input(2_500),
            ),
            entry(
                "c",
                "2026-09-02T10:00:00Z",
                "claude-opus-5",
                "s2",
                input(500),
            ),
        ];
        let report = report(&entries, &daily());

        // Computed from the fixture rather than restated: if the fixture
        // changes, this still asserts the relationship rather than a stale
        // literal.
        let summed_tokens: u64 = report.rows.iter().map(|row| row.tokens.total()).sum();
        let summed_cost = Usd::total(report.rows.iter().map(|row| row.cost));

        assert_eq!(report.totals.tokens.total(), summed_tokens);
        assert_eq!(report.totals.cost.to_cents(), summed_cost.to_cents());
        assert_eq!(
            report.totals.tokens.total(),
            4_000,
            "and the fixture really does hold four thousand tokens"
        );
    }

    #[test]
    fn a_model_breakdown_is_ordered_by_cost_not_by_first_appearance() {
        // Sonnet 5 is seen first but Opus 5 is five times the price, so a
        // single Opus million outspends two Sonnet ones. The breakdown must
        // put the money at the top.
        let entries = [
            entry(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-sonnet-5",
                "s1",
                input(2_000_000),
            ),
            entry(
                "b",
                "2026-09-01T10:00:00Z",
                "claude-opus-5",
                "s1",
                input(1_000_000),
            ),
        ];
        let report = report(&entries, &daily());
        let row = &report.rows[0];

        assert_eq!(row.breakdown[0].model.as_str(), "claude-opus-5");
        assert_eq!(row.breakdown[1].model.as_str(), "claude-sonnet-5");
        assert_eq!(row.breakdown[0].cost.to_cents(), 500, "$5.00 for a million");
        assert_eq!(row.breakdown[1].cost.to_cents(), 400, "$4.00 for two");
    }

    #[test]
    fn the_models_column_keeps_first_seen_order() {
        // The same fixture as above. The two columns hold the same models in
        // two different orders on purpose: this one is scanned to see what
        // arrived when.
        let entries = [
            entry(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-sonnet-5",
                "s1",
                input(2_000_000),
            ),
            entry(
                "b",
                "2026-09-01T10:00:00Z",
                "claude-opus-5",
                "s1",
                input(1_000_000),
            ),
        ];
        let row = &report(&entries, &daily()).rows[0];

        assert_eq!(
            row.models.iter().map(ModelId::as_str).collect::<Vec<_>>(),
            vec!["claude-sonnet-5", "claude-opus-5"]
        );
        assert_eq!(
            row.breakdown[0].model.as_str(),
            "claude-opus-5",
            "while the breakdown beside it is still dearest first"
        );
    }

    #[test]
    fn a_session_report_is_ordered_by_cost_descending_whatever_order_was_asked_for() {
        let entries = [
            entry(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-opus-5",
                "cheap",
                input(1_000),
            ),
            entry(
                "b",
                "2026-09-01T10:00:00Z",
                "claude-opus-5",
                "dear",
                input(1_000_000),
            ),
        ];
        for order in [Order::Ascending, Order::Descending] {
            let spec = GroupingSpec {
                period: None,
                by_project: false,
                by_session: true,
                order,
            };
            let report = report(&entries, &spec);
            assert_eq!(
                report.rows[0].session.as_ref().map(SessionId::as_str),
                Some("dear"),
                "asked for {order:?}, the dearest session still comes first"
            );
            assert_eq!(
                report.rows[1].session.as_ref().map(SessionId::as_str),
                Some("cheap")
            );
        }
    }

    #[test]
    fn a_session_whose_helpers_ran_in_worktrees_is_still_one_row() {
        // Claude Code gives a workflow a git worktree of its own, so one
        // conversation legitimately records several working directories. It is
        // still one conversation and one bill. Splitting on the directory as
        // well turned the dearest session on a real corpus into sixty-five rows
        // sharing an id, each holding a slice of the money, and dropped it from
        // first place in the ranking to fourth.
        let mut started = entry(
            "a",
            "2026-09-01T09:00:00Z",
            "claude-opus-5",
            "s1",
            input(10),
        );
        started.project = Project::new("/home/ada/glyphora");
        let mut helper = entry(
            "b",
            "2026-09-01T10:00:00Z",
            "claude-opus-5",
            "s1",
            input(90),
        );
        helper.project = Project::new("/home/ada/glyphora/.claude/worktrees/wf_1");

        let spec = GroupingSpec {
            period: None,
            by_project: false,
            by_session: true,
            order: Order::Ascending,
        };
        let report = report(&[started, helper], &spec);

        assert_eq!(report.rows.len(), 1, "one conversation, one row");
        assert_eq!(report.rows[0].tokens.total(), 100, "and the whole bill");
        assert_eq!(
            report.rows[0].project.as_ref().map(Project::as_str),
            Some("/home/ada/glyphora"),
            "named for where the conversation started, not for a worktree a \
             helper was handed an hour later"
        );
    }

    #[test]
    fn splitting_by_project_still_splits_a_session_that_asked_for_it() {
        // The rule above must not reach a report that genuinely wants one row
        // per directory: `daily --instances` is exactly that, and a session
        // report is the only place the directory is a note rather than a key.
        let mut here = entry(
            "a",
            "2026-09-01T09:00:00Z",
            "claude-opus-5",
            "s1",
            input(10),
        );
        here.project = Project::new("/home/ada/api");
        let mut there = entry(
            "b",
            "2026-09-01T10:00:00Z",
            "claude-opus-5",
            "s1",
            input(90),
        );
        there.project = Project::new("/home/ada/web");

        let report = report(
            &[here, there],
            &GroupingSpec {
                period: None,
                by_project: true,
                by_session: true,
                order: Order::Ascending,
            },
        );

        assert_eq!(report.rows.len(), 2, "two directories were asked for");
        assert_eq!(report.totals.tokens.total(), 100);
    }

    #[test]
    fn a_session_row_carries_the_first_and_last_response_it_saw() {
        let entries = [
            entry("a", "2026-09-01T09:00:00Z", "claude-opus-5", "s1", input(1)),
            entry("b", "2026-09-01T17:30:00Z", "claude-opus-5", "s1", input(1)),
            entry("c", "2026-09-01T12:00:00Z", "claude-opus-5", "s1", input(1)),
        ];
        let spec = GroupingSpec {
            period: None,
            by_project: false,
            by_session: true,
            order: Order::Ascending,
        };
        let row = &report(&entries, &spec).rows[0];

        assert_eq!(row.first_activity_at, Some(at("2026-09-01T09:00:00Z")));
        assert_eq!(
            row.last_activity_at,
            Some(at("2026-09-01T17:30:00Z")),
            "out-of-order arrival must not move the last stamp backwards"
        );
    }

    #[test]
    fn the_totals_row_is_the_same_whichever_order_the_rows_were_asked_for() {
        // Reversing the rows is a presentation choice. It must not reach the
        // figure underneath them -- not in the models column, and not in the
        // last bits of the cost, which is what happens when a sum is folded
        // over whichever order the rows happen to be sitting in. `f64`
        // addition is not associative, so a month of small amounts summed
        // forwards and backwards really can land on two different values, and
        // a total that changes when a sort flag does is a total nobody can
        // reconcile.
        let mut entries = Vec::new();
        for day in 1..=28_u64 {
            entries.push(entry(
                &format!("x{day}"),
                &format!("2026-09-{day:02}T09:00:00Z"),
                if day % 2 == 0 {
                    "claude-sonnet-5"
                } else {
                    "claude-opus-5"
                },
                "s1",
                input(1 + day * 7_919),
            ));
        }
        let ascending = report(&entries, &daily());
        let descending = report(
            &entries,
            &GroupingSpec {
                period: Some(AggregationPeriod::Day),
                order: Order::Descending,
                ..GroupingSpec::default()
            },
        );

        assert_eq!(
            ascending.totals.cost.dollars().to_bits(),
            descending.totals.cost.dollars().to_bits(),
            "the same traffic must add up to the very same number both ways"
        );
        assert_eq!(ascending.totals.models, descending.totals.models);
        assert_eq!(ascending.totals.tokens, descending.totals.tokens);
    }

    #[test]
    fn the_totals_models_column_agrees_with_the_rows_about_first_seen_order() {
        // Sonnet is seen first and Opus is dearer, so the two orders disagree
        // and the total has to pick the same one the rows above it picked.
        // Reading the total out of the already-sorted breakdown put the
        // dearest model first here while the row beside it still said Sonnet.
        let entries = [
            entry(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-sonnet-5",
                "s1",
                input(2_000_000),
            ),
            entry(
                "b",
                "2026-09-01T10:00:00Z",
                "claude-opus-5",
                "s1",
                input(1_000_000),
            ),
        ];
        let report = report(&entries, &daily());

        assert_eq!(
            report.totals.models, report.rows[0].models,
            "one row, so the total's models are that row's models"
        );
        assert_eq!(
            report
                .totals
                .models
                .iter()
                .map(ModelId::as_str)
                .collect::<Vec<_>>(),
            vec!["claude-sonnet-5", "claude-opus-5"]
        );
        assert_eq!(
            report.totals.breakdown[0].model.as_str(),
            "claude-opus-5",
            "while the total's breakdown is still dearest first"
        );
    }

    #[test]
    fn a_descending_daily_report_is_the_ascending_one_backwards() {
        let entries = [
            entry("a", "2026-09-01T09:00:00Z", "claude-opus-5", "s1", input(1)),
            entry("b", "2026-09-02T09:00:00Z", "claude-opus-5", "s1", input(2)),
            entry("c", "2026-09-03T09:00:00Z", "claude-opus-5", "s1", input(3)),
        ];
        let ascending = report(&entries, &daily());
        let descending = report(
            &entries,
            &GroupingSpec {
                period: Some(AggregationPeriod::Day),
                order: Order::Descending,
                ..GroupingSpec::default()
            },
        );

        let mut reversed = ascending.rows.clone();
        reversed.reverse();
        assert_eq!(descending.rows, reversed);
        assert_eq!(
            descending.totals.tokens.total(),
            ascending.totals.tokens.total(),
            "reversing the rows cannot change what they add up to"
        );
    }

    #[test]
    fn grouping_by_project_and_day_splits_a_day_into_one_row_per_directory() {
        let mut here = entry(
            "a",
            "2026-09-01T09:00:00Z",
            "claude-opus-5",
            "s1",
            input(10),
        );
        here.project = Project::new("/home/ada/api");
        let mut there = entry(
            "b",
            "2026-09-01T10:00:00Z",
            "claude-opus-5",
            "s2",
            input(20),
        );
        there.project = Project::new("/home/ada/web");

        let spec = GroupingSpec {
            period: Some(AggregationPeriod::Day),
            by_project: true,
            by_session: false,
            order: Order::Ascending,
        };
        let report = report(&[here, there], &spec);

        assert_eq!(report.rows.len(), 2);
        assert_eq!(
            report.rows[0].project.as_ref().map(Project::as_str),
            Some("/home/ada/api")
        );
        assert_eq!(
            report.rows[1].project.as_ref().map(Project::as_str),
            Some("/home/ada/web")
        );
        assert_eq!(report.totals.tokens.total(), 30);
    }

    #[test]
    fn a_report_over_nothing_has_no_rows_and_a_total_of_nothing() {
        let report = report(&[], &daily());
        assert!(report.rows.is_empty());
        assert_eq!(report.totals.tokens.total(), 0);
        assert_eq!(report.totals.cost, Usd::ZERO);
        assert!(report.totals.models.is_empty());
    }

    #[test]
    fn the_report_buckets_on_the_zone_it_was_given_rather_than_on_utc() {
        // 23:30 UTC on the 1st is already the 2nd in Tokyo, so the same two
        // entries make one row or two depending on whose calendar is used.
        let entries = [
            entry("a", "2026-09-01T13:00:00Z", "claude-opus-5", "s1", input(1)),
            entry("b", "2026-09-01T23:30:00Z", "claude-opus-5", "s1", input(1)),
        ];
        let tokyo = Zone::parse("Asia/Tokyo").expect("a real zone");
        let in_tokyo = UsageReport::build(
            &entries,
            &daily(),
            &tokyo,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );

        assert_eq!(report(&entries, &daily()).rows.len(), 1, "one UTC day");
        assert_eq!(in_tokyo.rows.len(), 2, "two Tokyo days");
        assert_eq!(
            in_tokyo.totals.tokens.total(),
            report(&entries, &daily()).totals.tokens.total(),
            "the zone moves the rows about but cannot change the total"
        );
    }

    #[test]
    fn the_display_cost_mode_reports_only_what_the_source_stated() {
        // The transcript format states no costs, so this mode is honest about
        // the gap rather than filling it. Worth pinning here because the
        // report is where a reader would notice.
        let entries = [entry(
            "a",
            "2026-09-01T09:00:00Z",
            "claude-opus-5",
            "s1",
            input(1_000_000),
        )];
        let stated = UsageReport::build(
            &entries,
            &daily(),
            &Zone::Utc,
            CostMode::Display,
            &PriceSheet::builtin(),
        );
        assert_eq!(stated.totals.cost, Usd::ZERO);
        assert_eq!(
            report(&entries, &daily()).totals.cost.to_cents(),
            500,
            "while calculating from the counters gives $5.00"
        );
    }
}
