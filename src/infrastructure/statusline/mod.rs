//! Adapters for `claude-stats statusline`: parsing what Claude Code hands the
//! hook on stdin, filling in what it left out by reading the transcript
//! directly, and remembering the answer between one prompt redraw and the
//! next.

pub mod cache;
pub mod hook;
pub mod transcript_tail;
