//! The dashboard's widgets.
//!
//! Two of these -- [`spinner`] and the glyph set in [`crate::tui::icons`] --
//! exist because the crates that were meant to supply them (`ratatui-spinner`
//! and `ratatui-icons`) are unpublished name reservations on crates.io. The
//! two third-party widget crates that are real are used where they earn their
//! place: `tui-big-text` in [`banner`] and `tui-piechart` in [`token_mix`].
//!
//! Everything here follows the same rule: a widget renders the domain, it
//! never computes it. If a number needs deriving, it is derived in
//! [`crate::domain`] where it can be tested without a terminal.

pub mod banner;
pub mod gauge;
pub mod meter;
pub mod sparkline;
pub mod spinner;
pub mod stat_tile;
pub mod token_mix;
pub mod tool_feed;
pub mod usage_windows;
