//! The use case behind every period table: load the entries, fold them, hand
//! the result back.
//!
//! A Service Layer object in Fowler's sense, and a deliberately thin one. It
//! owns no rules at all -- which responses count is
//! [`UsageQuery`]'s business, how they pile up is
//! [`GroupingSpec`]'s, what they cost is [`CostMode`]'s and
//! [`crate::domain::pricing::PriceSheet`]'s. What it owns is the *sequence*:
//! that the repository is asked once, that the aggregate is built from exactly
//! what came back, and that nobody in between gets a chance to filter, sort or
//! re-price. A layer that only forwards would be layering theatre; a layer
//! that fixes the order of two steps that must not be reordered is the
//! smallest useful boundary there is.
//!
//! # Why daily, weekly and monthly are one use case
//!
//! They differ in exactly one value: which [`AggregationPeriod`] the caller
//! puts in the spec. Everything else -- the query, the deduplication, the
//! pricing, the ordering, the totals -- is identical. Three services would be
//! three copies of the same eight lines, and the first correction to any of
//! them would land in one copy.
//!
//! [`AggregationPeriod`]: crate::domain::period::AggregationPeriod

use super::ports::{UsageQuery, UsageRepository};
use crate::domain::period::{GroupingSpec, Zone};
use crate::domain::pricing::{CostMode, PriceSheet};
use crate::domain::report::UsageReport;

/// Produces a [`UsageReport`] from the corpus.
///
/// Holds the repository and the sheet rather than taking them per call because
/// both are expensive to arrive at and neither should differ between two
/// reports in one run. A repository remembers which transcripts it has already
/// read, so handing it over once is what makes a second report on the same
/// process nearly free; a sheet composed twice could be composed differently
/// if the user's override file changed in between, and two tables in one run
/// priced by two sheets is exactly the confusion the sheet exists to prevent.
pub struct PeriodReport<R: UsageRepository> {
    repository: R,
    sheet: PriceSheet,
}

impl<R: UsageRepository> PeriodReport<R> {
    /// A service over `repository`, costing everything at `sheet`.
    pub const fn new(repository: R, sheet: PriceSheet) -> Self {
        Self { repository, sheet }
    }

    /// The report `query` and `spec` describe.
    ///
    /// Takes `&mut self` because the repository does: it is expected to
    /// remember what it read, and a signature that hid that would be a lie
    /// about the cost of calling this twice.
    ///
    /// # Errors
    ///
    /// Only what the repository fails with -- the corpus could not be
    /// enumerated at all. A single unreadable or half-written transcript is
    /// skipped there rather than failing the report, for the reason given on
    /// [`UsageRepository::entries`].
    pub fn run(
        &mut self,
        query: &UsageQuery,
        spec: &GroupingSpec,
        zone: &Zone,
        mode: CostMode,
    ) -> anyhow::Result<UsageReport> {
        let entries = self.repository.entries(query)?;
        Ok(UsageReport::build(&entries, spec, zone, mode, &self.sheet))
    }

    /// The sheet the figures were produced with, for a report's footer.
    ///
    /// A total is only comparable with another total if both can say which
    /// rates produced them, and by the time a figure reaches a renderer the
    /// sheet is long out of scope unless something hands it over.
    #[must_use]
    pub const fn sheet(&self) -> &PriceSheet {
        &self.sheet
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::domain::entry::{Entry, EntryId};
    use crate::domain::model::ModelId;
    use crate::domain::period::{AggregationPeriod, Order};
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
    /// asked, so a test can prove the service loads once and folds once.
    struct InMemory {
        entries: Vec<Entry>,
        loads: u32,
        fails: bool,
    }

    impl UsageRepository for InMemory {
        fn entries(&mut self, query: &UsageQuery) -> anyhow::Result<Vec<Entry>> {
            self.loads += 1;
            if self.fails {
                anyhow::bail!("cannot list the projects directory");
            }
            Ok(self
                .entries
                .iter()
                .filter(|entry| query.matches(entry))
                .cloned()
                .collect())
        }
    }

    fn service(fails: bool) -> PeriodReport<InMemory> {
        PeriodReport::new(
            InMemory {
                entries: vec![
                    entry("a", "2026-09-01T09:00:00Z", 1_000_000),
                    entry("b", "2026-09-02T09:00:00Z", 2_000_000),
                ],
                loads: 0,
                fails,
            },
            PriceSheet::builtin(),
        )
    }

    fn daily() -> GroupingSpec {
        GroupingSpec {
            period: Some(AggregationPeriod::Day),
            by_project: false,
            by_session: false,
            order: Order::Ascending,
        }
    }

    #[test]
    fn the_service_reads_the_repository_once_and_folds_what_it_got() {
        let mut service = service(false);
        let report = service
            .run(&UsageQuery::default(), &daily(), &Zone::Utc, CostMode::Auto)
            .expect("the fake cannot fail");

        assert_eq!(service.repository.loads, 1, "one query, one read");
        assert_eq!(report.rows.len(), 2);
        assert_eq!(report.totals.tokens.input, 3_000_000);
        assert_eq!(
            report.totals.cost.to_cents(),
            1_500,
            "three million Opus 5 input tokens at $5 per million"
        );
    }

    #[test]
    fn the_query_narrows_what_the_report_covers_rather_than_the_service_doing_it() {
        // The service owns no filtering of its own: everything it reports on
        // is what the repository handed back for the query it was given.
        let mut service = service(false);
        let query = UsageQuery {
            since: Some(at("2026-09-02T00:00:00Z")),
            ..UsageQuery::default()
        };
        let report = service
            .run(&query, &daily(), &Zone::Utc, CostMode::Auto)
            .expect("the fake cannot fail");

        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].key.as_str(), "2026-09-02");
    }

    #[test]
    fn one_service_answers_daily_weekly_and_monthly_from_the_same_entries() {
        // The whole reason there is one use case rather than three: the only
        // thing that differs is the period in the spec.
        let mut service = service(false);
        let periods = [
            (AggregationPeriod::Day, 2, "2026-09-01"),
            (
                AggregationPeriod::Week {
                    starts_on: AggregationPeriod::DEFAULT_WEEK_START,
                },
                1,
                "2026-08-30",
            ),
            (AggregationPeriod::Month, 1, "2026-09"),
        ];
        for (period, rows, first_key) in periods {
            let spec = GroupingSpec {
                period: Some(period),
                ..daily()
            };
            let report = service
                .run(&UsageQuery::default(), &spec, &Zone::Utc, CostMode::Auto)
                .expect("the fake cannot fail");
            assert_eq!(report.rows.len(), rows, "{period:?}");
            assert_eq!(report.rows[0].key.as_str(), first_key, "{period:?}");
            assert_eq!(
                report.totals.tokens.input, 3_000_000,
                "{period:?} regroups the same traffic rather than changing it"
            );
        }
    }

    #[test]
    fn a_corpus_that_cannot_be_read_fails_the_report_rather_than_reporting_nothing() {
        // Reporting an empty table for a directory that could not be listed
        // would be indistinguishable from a genuinely quiet week.
        let mut service = service(true);
        let failure = service
            .run(&UsageQuery::default(), &daily(), &Zone::Utc, CostMode::Auto)
            .expect_err("the fake always fails");
        assert!(failure.to_string().contains("projects directory"));
    }
}
