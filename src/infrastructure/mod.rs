//! The adapters: concrete implementations of the ports, and the only place in
//! the crate that knows about the filesystem, JSON and the shape of Claude
//! Code's own storage.

pub mod config;
pub mod pricing;
pub mod reports;
pub mod statusline;
pub mod transcript;
