//! The full-screen views the dashboard can show.
//!
//! Each one takes a `Rect` and the data it needs, and draws. None of them own
//! state -- what is selected, how far the log is scrolled, which view is
//! showing -- because that all belongs to [`crate::tui::app::App`], and
//! spreading it across the screens is how a terminal application ends up with
//! two sources of truth about where the cursor is.

pub mod dashboard;
pub mod help;
pub mod log;
pub mod sessions;
