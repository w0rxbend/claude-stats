//! The composition root.
//!
//! This is the only place in the project where a concrete adapter meets an
//! abstract port. Everything above it -- the domain, the use cases, the
//! widgets -- is written against traits, so swapping the filesystem catalogue
//! for something else would be a change to this file and nothing else.

use std::fmt::Write as _;

use anyhow::{Context, Result};
use clap::Parser;
use claude_stats::application::monitor::Monitor;
use claude_stats::application::ports::{
    SessionReader, SessionSelector, TranscriptCatalog, TranscriptRef,
};
use claude_stats::cli::{Cli, Command};
use claude_stats::domain::model::ModelCatalog;
use claude_stats::infrastructure::transcript::locator::FileSystemCatalog;
use claude_stats::infrastructure::transcript::parser::TranscriptParser;
use claude_stats::infrastructure::transcript::watcher::FileSystemWatchFactory;
use claude_stats::report;
use claude_stats::tui::{format, runtime};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let selector = cli.selection.selector();
    let catalog = FileSystemCatalog::from_home()?;

    match cli.command.unwrap_or(Command::Monitor) {
        Command::Monitor => monitor(catalog, selector),
        Command::Stats { json } => stats(&catalog, &selector, json),
        Command::Sessions { limit } => sessions(&catalog, limit),
        Command::Models => {
            print!("{}", model_table());
            Ok(())
        }
    }
}

/// Runs the live dashboard.
fn monitor(catalog: FileSystemCatalog, selector: SessionSelector) -> Result<()> {
    // Refusing early with a clear message beats emitting a screenful of escape
    // sequences into whatever the output was redirected to.
    anyhow::ensure!(
        runtime::is_interactive(),
        "the dashboard needs a terminal; try `claude-stats stats` to print a report instead"
    );
    runtime::run(Monitor::new(
        catalog,
        TranscriptParser,
        FileSystemWatchFactory,
        selector,
    ))
}

/// Prints a one-shot report.
fn stats(catalog: &FileSystemCatalog, selector: &SessionSelector, as_json: bool) -> Result<()> {
    let transcript = require_session(catalog, selector)?;
    let snapshot = TranscriptParser
        .read(&transcript)
        .with_context(|| format!("cannot read {}", transcript.path.display()))?;

    if as_json {
        println!("{}", report::json(&snapshot));
    } else {
        print!("{}", report::text(&snapshot));
    }
    Ok(())
}

/// Lists the sessions on this machine.
fn sessions(catalog: &FileSystemCatalog, limit: usize) -> Result<()> {
    let all = catalog.list()?;
    if all.is_empty() {
        println!(
            "no Claude Code sessions found under ~/.claude/projects \
             (set CLAUDE_CONFIG_DIR if Claude Code stores its state elsewhere)"
        );
        return Ok(());
    }
    println!(
        "{:<10}  {:<16}  {:>8}  PROJECT",
        "SESSION", "MODIFIED", "SIZE"
    );
    for session in all.iter().take(limit) {
        println!(
            "{:<10}  {:<16}  {:>7}K  {}",
            format::session_id(&session.session_id),
            session.modified_at.format("%Y-%m-%d %H:%M"),
            session.size_bytes / 1_024,
            session.project_dir,
        );
    }
    if all.len() > limit {
        println!("\n... and {} more (use --limit)", all.len() - limit);
    }
    Ok(())
}

/// Resolves a selector, turning "nothing matched" into an actionable message.
fn require_session(
    catalog: &FileSystemCatalog,
    selector: &SessionSelector,
) -> Result<TranscriptRef> {
    catalog.resolve(selector)?.with_context(|| match selector {
        SessionSelector::Active => {
            "no active session found; start Claude Code, or pass --session".to_owned()
        }
        SessionSelector::Id(prefix) => format!("no session whose id starts with {prefix:?}"),
        SessionSelector::Project(dir) => format!("no sessions for {}", dir.display()),
        SessionSelector::Path(path) => format!("cannot read {}", path.display()),
    })
}

/// The model catalogue as a table.
///
/// Built as a string rather than printed directly so it can be asserted on in
/// a test without capturing stdout.
fn model_table() -> String {
    const MODELS: &[&str] = &[
        "claude-fable-5",
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-haiku-4-5",
    ];

    let mut out = format!(
        "\n{:<14}{:>12}{:>10}{:>12}{:>13}{:>10}\n",
        "MODEL", "CONTEXT", "INPUT", "CACHE READ", "CACHE WRITE", "OUTPUT"
    );
    let _ = writeln!(out, "{}", "-".repeat(71));
    for id in MODELS {
        let pricing = ModelCatalog::pricing_for(id);
        let _ = writeln!(
            out,
            "{:<14}{:>11}k{:>9.2}{:>12.2}{:>13.2}{:>10.2}",
            ModelCatalog::display_name_for(id),
            ModelCatalog::context_window_for(id) / 1_000,
            pricing.input.dollars_per_million(),
            pricing.cache_read.dollars_per_million(),
            pricing.cache_write.dollars_per_million(),
            pricing.output.dollars_per_million(),
        );
    }
    out.push_str("\nprices are US dollars per million tokens\n\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_table_lists_every_headline_model_with_its_window() {
        let table = model_table();
        assert!(table.contains("Opus 5"));
        assert!(table.contains("Haiku 4.5"));
        assert!(table.contains("1000k"), "the 1M window should be shown");
        assert!(table.contains("200k"), "the 200k window should be shown");
    }
}
