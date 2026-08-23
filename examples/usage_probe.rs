//! Prints the account-usage reading the dashboard would show, without a
//! terminal. Handy for checking the scanner against a real machine's history.

use chrono::Utc;
use claude_stats::application::ports::AccountUsageReader;
use claude_stats::domain::limits::WindowUsage;
use claude_stats::infrastructure::transcript::locator::FileSystemCatalog;
use claude_stats::infrastructure::transcript::usage::IncrementalUsageScanner;

fn main() -> anyhow::Result<()> {
    let mut scanner = IncrementalUsageScanner::new(FileSystemCatalog::from_home()?);

    let started = std::time::Instant::now();
    let usage = scanner.usage(Utc::now())?;
    let cold = started.elapsed();

    let started = std::time::Instant::now();
    let _ = scanner.usage(Utc::now())?;
    let warm = started.elapsed();

    show("last 5 hours", &usage.session);
    show("last 7 days", &usage.week);

    println!("\nlimit events recorded: {}", usage.limit_events.len());
    for event in &usage.limit_events {
        println!(
            "  {} {:?} -> resets {}  (active now: {})",
            event.at.format("%Y-%m-%d %H:%M"),
            event.kind,
            event.resets_at.format("%Y-%m-%d %H:%M"),
            event.is_active_at(usage.measured_at),
        );
    }
    println!("\ncold scan {cold:?}, warm scan {warm:?}");
    Ok(())
}

fn show(label: &str, window: &WindowUsage) {
    println!(
        "\n{label}\n  tokens   {}\n  cost     {}\n  sessions {}\n  peak     {:?}\n  vs peak  {:?}",
        window.tokens.total(),
        window.cost,
        window.sessions,
        window.peak,
        window.share_of_peak(),
    );
}
