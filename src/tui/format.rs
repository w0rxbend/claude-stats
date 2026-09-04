//! Kept as a re-export so that the widgets go on saying `format::tokens(..)`.
//!
//! The formatting helpers moved to [`crate::view::format`] when the text
//! reports started needing them too. A report reaching sideways into the
//! terminal layer for a thousands separator was the crate's only
//! inward-pointing import, and one line here removes it without touching a
//! single widget.

pub use crate::view::format::*;
