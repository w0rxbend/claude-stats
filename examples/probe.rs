//! Parses a real transcript and prints the derived metrics, as a sanity check
//! against live data. Run with: `cargo run --example probe -- <path>`
use claude_stats::application::ports::{SessionReader, SessionSelector, TranscriptCatalog};
use claude_stats::infrastructure::transcript::locator::FileSystemCatalog;
use claude_stats::infrastructure::transcript::parser::TranscriptParser;

fn main() -> anyhow::Result<()> {
    let catalog = FileSystemCatalog::from_home()?;
    let selector = std::env::args()
        .nth(1)
        .map_or(SessionSelector::Active, |p| {
            SessionSelector::Path(std::path::PathBuf::from(p))
        });
    let Some(t) = catalog.resolve(&selector)? else {
        println!("no transcript found");
        return Ok(());
    };
    let s = TranscriptParser::default().read(&t)?;
    println!("session      {}", s.session_id);
    println!("project      {:?}", s.project_dir);
    println!("branch       {:?}", s.git_branch);
    println!("model        {} ({})", s.model_display_name(), s.model_id);
    println!("turns/resp   {} / {}", s.turns, s.responses);
    println!("totals       {:?}", s.totals);
    println!(
        "context      {} / {} = {:.1}%",
        s.context_fill().used(),
        s.context_window(),
        s.context_fill().percent()
    );
    println!("severity     {:?}", s.context_fill().severity());
    println!("growth/turn  {:.0}", s.average_context_growth_per_turn());
    println!(
        "compaction   {:?} (seen {})",
        s.compaction_distance(),
        s.compactions.len()
    );
    println!(
        "cost         {} ({}/turn, {:?}/h)",
        s.cost(),
        s.cost_per_turn(),
        s.burn_rate_per_hour().map(|u| u.to_string())
    );
    println!("cache        {:?}", s.cache_hit_ratio());
    println!("efficiency   {:?}", s.efficiency());
    println!(
        "tools        {} calls, {} errors",
        s.tool_calls(),
        s.tool_errors
    );
    println!("kinds        {:?}", s.kind_counts);
    println!(
        "files        {} touched, +{} -{}",
        s.files_touched(),
        s.lines_added,
        s.lines_removed
    );
    println!(
        "thinking     {}  agents {}  skills {}",
        s.thinking_blocks, s.subagents, s.skills
    );
    println!("duration     {:?}", s.duration().map(|d| d.num_minutes()));
    println!("last error   {:?}", s.last_error);
    println!(
        "recent       {:?}",
        s.recent_tools
            .iter()
            .rev()
            .take(5)
            .map(claude_stats::domain::activity::ToolEvent::label)
            .collect::<Vec<_>>()
    );
    println!("events       {}", s.events.len());
    Ok(())
}
