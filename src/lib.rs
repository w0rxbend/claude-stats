//! `claudetui` -- a live dashboard for Claude Code sessions.
//!
//! The crate is split the way a hexagonal architecture suggests, because the
//! interesting logic here is arithmetic on token counts and it deserves to be
//! testable without a terminal:
//!
//! * [`domain`] -- what a session, a token, a dollar and a context window are.
//!   Pure data and arithmetic; no I/O, no ratatui, no serde.
//! * [`application`] -- the use cases, written against traits (`ports`) rather
//!   than against files.
//! * [`infrastructure`] -- the adapters that implement those traits by reading
//!   Claude Code's own storage.
//! * [`tui`] -- the terminal presentation. It reads the domain and never
//!   computes anything the domain could compute for it.
//!
//! Dependencies point inwards only: `tui` and `infrastructure` know about
//! `domain`, and `domain` knows about nobody.

pub mod application;
pub mod domain;
pub mod infrastructure;
