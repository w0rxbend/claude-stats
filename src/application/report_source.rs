//! A Separated Interface (Fowler, *`PoEAA`*) over the four report use cases
//! [`super::period_report::PeriodReport`] and [`super::blocks_report::BlocksReport`]
//! already answer for the `daily`, `weekly`, `monthly` and `blocks` commands,
//! for the one caller on the other side of the hexagon that cannot depend on
//! either concretely: the dashboard's own tabs of the same name.
//!
//! `crate::tui::app::App` is generic only over
//! [`super::ports::TranscriptCatalog`], [`super::ports::SessionReader`] and
//! [`super::ports::ChangeSourceFactory`] -- the three ports the session
//! monitor itself needs. Adding a fourth generic parameter for whichever
//! concrete [`super::ports::UsageRepository`] the composition root chose
//! would ripple that type through every one of `App`'s existing test call
//! sites for a capability most of them have no interest in, the same
//! reasoning [`crate::application::usage::UsageTracker`] and
//! `crate::infrastructure::config::ConfigGateway` already follow as fields
//! `App` holds behind a concrete, already-erased type or an `Option`. A
//! trait object kept behind `Option<Box<dyn ReportSource>>` follows that same
//! shape: one optional field, `None` in every test that does not care, and a
//! real adapter wired in only at the composition root.
//!
//! Only the *shape* of what the tabs need is here -- "a report", "some
//! blocks" -- not how it is produced. [`crate::infrastructure::reports::FileSystemReportSource`]
//! is the one production implementation, built from the same
//! [`super::period_report::PeriodReport`]/[`super::blocks_report::BlocksReport`]
//! pipeline `src/main.rs` already wires the CLI commands through, so a figure
//! shown in the dashboard's Daily tab cannot come out different from the one
//! `claude-stats daily` prints for the same corpus.

use chrono::{DateTime, Utc};

use super::blocks_report::BlockRow;
use crate::domain::report::UsageReport;

/// Produces the same period and blocks reports the `daily`/`weekly`/
/// `monthly`/`blocks` commands print.
///
/// Every method takes `&mut self` for the same reason
/// [`super::ports::UsageRepository::entries`] does: an honest implementation
/// remembers what it read, and a signature that hid that would understate
/// the cost of asking twice.
pub trait ReportSource {
    /// Usage over the whole corpus, grouped by calendar day.
    ///
    /// # Errors
    ///
    /// Returns an error only when the corpus cannot be enumerated at all --
    /// see [`super::ports::UsageRepository::entries`] for why a single
    /// unreadable transcript does not fail this on its own.
    fn daily(&mut self) -> anyhow::Result<UsageReport>;

    /// Usage over the whole corpus, grouped by calendar week.
    ///
    /// # Errors
    ///
    /// See [`Self::daily`].
    fn weekly(&mut self) -> anyhow::Result<UsageReport>;

    /// Usage over the whole corpus, grouped by calendar month.
    ///
    /// # Errors
    ///
    /// See [`Self::daily`].
    fn monthly(&mut self) -> anyhow::Result<UsageReport>;

    /// The five-hour billing blocks, as of `now`.
    ///
    /// # Errors
    ///
    /// See [`Self::daily`].
    fn blocks(&mut self, now: DateTime<Utc>) -> anyhow::Result<Vec<BlockRow>>;
}
