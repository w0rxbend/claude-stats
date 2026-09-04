//! The production [`ReportSource`]: the dashboard's Daily/Weekly/Monthly/
//! Blocks tabs, wired to the real transcript corpus on disk.
//!
//! This is deliberately the thinnest possible adapter. It owns nothing that
//! [`PeriodReport`]/[`BlocksReport`] do not already own -- a catalogue, a
//! price sheet, a zone -- and every method below builds a fresh service and
//! asks it once, exactly the sequence `src/main.rs` already drives for the
//! `daily`/`weekly`/`monthly`/`blocks` commands. A figure the dashboard shows
//! in one of these tabs is therefore never able to disagree with what the
//! equivalent CLI command would print for the same corpus: both are folded by
//! the same `UsageReport::build`/`blocks::identify` from entries the same
//! [`FileSystemUsageRepository`] read.
//!
//! # Why a fresh repository is built on every call
//!
//! [`FileSystemUsageRepository`] keeps its own per-file cache internally (see
//! its own module doc), so rebuilding one here does not mean re-reading every
//! transcript from the first byte every time -- only the files that changed
//! since the *previous* [`FileSystemReportSource`] were touched pay that
//! cost, and each call still only reads a file at all if its own bounds could
//! possibly hold something in range. What is genuinely given up by not
//! keeping one repository across calls is the identity-map de-duplication
//! cache staying warm between them; that trade is fine here because
//! `crate::tui::app::App` only reaches for this a handful of times a session
//! -- switching to one of these tabs, or pressing `r` -- rather than once a
//! frame, which is the reasoning the field's own doc comment on the `App`
//! side gives in full.

use chrono::{DateTime, Utc};

use crate::application::blocks_report::{BlockOptions, BlockRow, BlocksReport};
use crate::application::period_report::PeriodReport;
use crate::application::ports::UsageQuery;
use crate::application::report_source::ReportSource;
use crate::domain::period::{AggregationPeriod, GroupingSpec, Zone};
use crate::domain::pricing::{CostMode, PriceSheet};
use crate::domain::report::UsageReport;
use crate::infrastructure::transcript::corpus::FileSystemUsageRepository;
use crate::infrastructure::transcript::locator::FileSystemCatalog;

/// [`ReportSource`] over the real corpus on disk.
pub struct FileSystemReportSource {
    catalog: FileSystemCatalog,
    sheet: PriceSheet,
    zone: Zone,
}

impl FileSystemReportSource {
    /// A source over `catalog`, costing everything at `sheet` and grouping
    /// calendar buckets on `zone`.
    #[must_use]
    pub const fn new(catalog: FileSystemCatalog, sheet: PriceSheet, zone: Zone) -> Self {
        Self {
            catalog,
            sheet,
            zone,
        }
    }

    /// One period report, grouped by `period` over the whole corpus with no
    /// other narrowing -- the same unbounded [`UsageQuery::default`] the
    /// `daily`/`weekly`/`monthly` commands fall back on with no flags of
    /// their own.
    fn period(&self, period: AggregationPeriod) -> anyhow::Result<UsageReport> {
        let mut service = PeriodReport::new(
            FileSystemUsageRepository::new(self.catalog.clone()),
            self.sheet.clone(),
        );
        service.run(
            &UsageQuery::default(),
            &GroupingSpec {
                period: Some(period),
                ..GroupingSpec::default()
            },
            &self.zone,
            CostMode::Auto,
        )
    }
}

impl ReportSource for FileSystemReportSource {
    fn daily(&mut self) -> anyhow::Result<UsageReport> {
        self.period(AggregationPeriod::Day)
    }

    fn weekly(&mut self) -> anyhow::Result<UsageReport> {
        self.period(AggregationPeriod::Week {
            starts_on: AggregationPeriod::DEFAULT_WEEK_START,
        })
    }

    fn monthly(&mut self) -> anyhow::Result<UsageReport> {
        self.period(AggregationPeriod::Month)
    }

    fn blocks(&mut self, now: DateTime<Utc>) -> anyhow::Result<Vec<BlockRow>> {
        let mut service = BlocksReport::new(
            FileSystemUsageRepository::new(self.catalog.clone()),
            self.sheet.clone(),
        );
        service.run(
            &UsageQuery::default(),
            &BlockOptions::default(),
            now,
            CostMode::Auto,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source over a directory with nothing in it produces the same empty
    /// report every command already handles gracefully, rather than an
    /// error -- proving this adapter's plumbing (catalogue, repository,
    /// service, query) fits together the way `main.rs`'s own wiring does,
    /// without needing a real `~/.claude/projects` to run against.
    #[test]
    fn a_source_over_an_empty_directory_answers_empty_reports_rather_than_failing() {
        let dir = std::env::temp_dir().join(format!(
            "claude-stats-report-source-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let catalog = FileSystemCatalog::rooted_at(dir.clone());
        let mut source = FileSystemReportSource::new(catalog, PriceSheet::builtin(), Zone::Utc);

        assert!(
            source
                .daily()
                .expect("no directory to fail on")
                .rows
                .is_empty()
        );
        assert!(
            source
                .weekly()
                .expect("no directory to fail on")
                .rows
                .is_empty()
        );
        assert!(
            source
                .monthly()
                .expect("no directory to fail on")
                .rows
                .is_empty()
        );
        assert!(
            source
                .blocks(Utc::now())
                .expect("no directory to fail on")
                .is_empty()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
