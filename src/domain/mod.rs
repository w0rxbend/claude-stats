//! The domain layer: what a Claude Code session *is*, expressed without any
//! reference to files, JSON, terminals or ratatui.
//!
//! Every type here is either a value object (money, token counts, a context
//! fill reading, one billable [`entry::Entry`]) or part of the
//! [`session::SessionSnapshot`] aggregate. None of them know where their data
//! came from, which is what lets the parser, the dashboard and the tests all
//! agree on the same vocabulary.

pub mod activity;
pub mod blocks;
pub mod context;
pub mod entry;
pub mod limits;
pub mod model;
pub mod money;
pub mod period;
pub mod pricing;
pub mod project;
pub mod report;
pub mod session;
pub mod tokens;
