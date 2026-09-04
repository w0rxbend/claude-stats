//! The presentation layer that has nothing to do with a terminal.
//!
//! Everything a report has to decide before it can be drawn lives here: how a
//! number is written down, which columns a table has, what goes in each cell.
//! Nothing in it imports ratatui, crossterm or `std::io`, which is the point.
//!
//! It exists because the alternative had already gone wrong. The text reports
//! were importing [`crate::tui`] for a duration formatter, which made a report
//! depend on the dashboard -- the only import in the crate pointing inwards
//! from a delivery mechanism, and the one that would have had to be untangled
//! the first time anything but a terminal wanted a table. Moving the shared
//! half here leaves `tui` free to be about ratatui, `report` free to be about
//! text, and both of them reading the same numbers the same way.

pub mod blocks_view;
pub mod dashboard_view;
pub mod format;
pub mod statusline;
pub mod table;
pub mod usage_view;
