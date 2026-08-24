//! The composition root.
//!
//! This is the only place in the project where a concrete adapter meets an
//! abstract port. Everything above it -- the domain, the use cases, the
//! widgets -- is written against traits, so swapping the filesystem catalogue
//! for something else would be a change to this file and nothing else.

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use claude_stats::application::monitor::Monitor;
use claude_stats::application::ports::{
    AccountUsageReader, SessionReader, SessionSelector, SystemClock, TranscriptCatalog,
    TranscriptRef,
};
use claude_stats::application::usage::UsageTracker;
use claude_stats::cli::{Cli, Command};
use claude_stats::infrastructure::transcript::locator::FileSystemCatalog;
use claude_stats::infrastructure::transcript::parser::TranscriptParser;
use claude_stats::infrastructure::transcript::usage::IncrementalUsageScanner;
use claude_stats::infrastructure::transcript::watcher::FileSystemWatchFactory;
use claude_stats::report;
use claude_stats::tui::runtime;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let selector = cli.selection.selector();
    let catalog = FileSystemCatalog::from_home()?;

    match cli.command.unwrap_or(Command::Monitor) {
        Command::Monitor => monitor(catalog, selector),
        Command::Stats { json } => stats(&catalog, &selector, json),
        Command::Sessions { limit } => sessions(&catalog, limit),
        Command::Models => {
            print!("{}", report::models());
            Ok(())
        }
    }
}

/// Runs the live dashboard.
fn monitor(catalog: FileSystemCatalog, selector: SessionSelector) -> Result<()> {
    // The usage tracker gets a catalogue of its own rather than sharing the
    // monitor's. They ask different questions of it on different clocks, and
    // a second one costs nothing: a catalogue is a path and no state.
    let usage = UsageTracker::new(
        Box::new(IncrementalUsageScanner::new(FileSystemCatalog::from_home()?)),
        Box::new(SystemClock),
    );
    // Refusing early with a clear message beats emitting a screenful of escape
    // sequences into whatever the output was redirected to.
    anyhow::ensure!(
        runtime::is_interactive(),
        "the dashboard needs a terminal; try `claude-stats stats` to print a report instead"
    );
    runtime::run(
        Monitor::new(catalog, TranscriptParser, FileSystemWatchFactory, selector),
        usage,
    )
}

/// Prints a one-shot report.
fn stats(catalog: &FileSystemCatalog, selector: &SessionSelector, as_json: bool) -> Result<()> {
    let transcript = require_session(catalog, selector)?;
    let snapshot = TranscriptParser.read(&transcript)?;

    // Account-wide usage is a bonus here, not the point of the command: a
    // report about one session is still worth printing when the scan of every
    // other session fails, so a failure is dropped rather than propagated.
    let usage = FileSystemCatalog::from_home()
        .map(IncrementalUsageScanner::new)
        .and_then(|mut scanner| scanner.usage(Utc::now()))
        .ok();

    if as_json {
        println!("{}", report::json(&snapshot, usage.as_ref()));
    } else {
        print!("{}", report::text(&snapshot, usage.as_ref()));
    }
    Ok(())
}

/// Lists the sessions on this machine.
fn sessions(catalog: &FileSystemCatalog, limit: usize) -> Result<()> {
    print!("{}", report::sessions(&catalog.list()?, limit));
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
