//! The one-shot text and JSON reports behind `claudetui stats`.
//!
//! Separate from the dashboard because the audiences are different. The
//! dashboard is watched; a report is read once, pasted into an issue, or piped
//! into `jq`. Both read the same [`SessionSnapshot`], so they cannot disagree
//! about what the session cost.

use std::fmt::Write as _;

use serde_json::json;

use crate::domain::context::CompactionDistance;
use crate::domain::session::SessionSnapshot;
use crate::tui::format;

/// Renders the human-readable report.
/// One aligned `label  value` line.
///
/// `write!` into the buffer rather than `push_str(&format!(..))`: the latter
/// allocates a throwaway `String` for every one of the thirty-odd rows.
fn row(out: &mut String, label: &str, value: impl std::fmt::Display) {
    let _ = writeln!(out, "  {label:<20}{value}");
}

#[must_use]
pub fn text(snapshot: &SessionSnapshot) -> String {
    let fill = snapshot.context_fill();
    let mut out = String::new();

    let _ = write!(out, "\nsession {}\n\n", snapshot.session_id);
    row(&mut out, "model", snapshot.model_display_name());
    row(&mut out, 
        "project",
        snapshot.project_dir.clone().unwrap_or_else(|| "-".to_owned()),
    );
    row(&mut out, 
        "branch",
        snapshot.git_branch.clone().unwrap_or_else(|| "-".to_owned()),
    );
    row(&mut out, 
        "elapsed",
        snapshot
            .duration()
            .map_or_else(|| "-".to_owned(), format::duration),
    );

    out.push('\n');
    row(&mut out, 
        "context",
        format!(
            "{} / {}  ({})",
            format::tokens(fill.used()),
            format::tokens(fill.window()),
            format::percent_precise(fill.ratio())
        ),
    );
    row(&mut out, 
        "until compaction",
        match snapshot.compaction_distance() {
            CompactionDistance::Imminent => "imminent".to_owned(),
            CompactionDistance::Turns(n) => format!("~{n} turns"),
            CompactionDistance::Unknown => "unknown".to_owned(),
        },
    );
    row(&mut out, "compactions", snapshot.compactions.len().to_string());

    out.push('\n');
    row(&mut out, 
        "cost",
        format!("{}  ({}/turn)", snapshot.cost(), snapshot.cost_per_turn()),
    );
    row(&mut out, 
        "cache hit ratio",
        snapshot
            .cache_hit_ratio()
            .map_or_else(|| "-".to_owned(), format::percent_precise),
    );
    row(&mut out, "input tokens", format::tokens(snapshot.totals.input));
    row(&mut out, "cache reads", format::tokens(snapshot.totals.cache_read));
    row(&mut out, "cache writes", format::tokens(snapshot.totals.cache_creation));
    row(&mut out, "output tokens", format::tokens(snapshot.totals.output));

    out.push('\n');
    row(&mut out, "turns", snapshot.turns.to_string());
    row(&mut out, "tool calls", snapshot.tool_calls().to_string());
    row(&mut out, "tool errors", snapshot.tool_errors.to_string());
    row(&mut out, "files touched", snapshot.files_touched().to_string());
    row(&mut out, 
        "lines",
        format!("+{} -{}", snapshot.lines_added, snapshot.lines_removed),
    );
    row(&mut out, "thinking blocks", snapshot.thinking_blocks.to_string());
    row(&mut out, "sub-agents", snapshot.subagents.to_string());
    row(&mut out, "skills", snapshot.skills.to_string());

    if !snapshot.tool_counts.is_empty() {
        out.push_str("\n  top tools\n");
        let mut ranked: Vec<_> = snapshot.tool_counts.iter().collect();
        ranked.sort_by_key(|(name, count)| (std::cmp::Reverse(**count), (*name).clone()));
        for (name, count) in ranked.iter().take(8) {
                let _ = writeln!(out, "    {name:<18}{count}");
        }
    }

    out.push('\n');
    out
}

/// Renders the machine-readable report.
///
/// Raw token counts and dollar amounts, not the abbreviated strings the
/// dashboard shows: anything consuming this will want to do its own
/// arithmetic, and `953.4k` is not a number.
#[must_use]
pub fn json(snapshot: &SessionSnapshot) -> String {
    let fill = snapshot.context_fill();
    let value = json!({
        "session_id": snapshot.session_id,
        "transcript": snapshot.transcript_path,
        "model": snapshot.model_id,
        "project_dir": snapshot.project_dir,
        "git_branch": snapshot.git_branch,
        "started_at": snapshot.started_at,
        "last_activity_at": snapshot.last_activity_at,
        "context": {
            "used_tokens": fill.used(),
            "window_tokens": fill.window(),
            "ratio": fill.ratio(),
            "tokens_until_compaction": fill.tokens_until_compaction(),
            "growth_per_turn": snapshot.average_context_growth_per_turn(),
            "compactions": snapshot.compactions.len(),
        },
        "tokens": {
            "input": snapshot.totals.input,
            "cache_read": snapshot.totals.cache_read,
            "cache_creation": snapshot.totals.cache_creation,
            "output": snapshot.totals.output,
        },
        "cost_usd": snapshot.cost().dollars(),
        "cost_per_turn_usd": snapshot.cost_per_turn().dollars(),
        "cache_hit_ratio": snapshot.cache_hit_ratio(),
        "efficiency": snapshot.efficiency(),
        "turns": snapshot.turns,
        "responses": snapshot.responses,
        "tool_calls": snapshot.tool_calls(),
        "tool_errors": snapshot.tool_errors,
        "tools": snapshot.tool_counts,
        "files_touched": snapshot.files_touched(),
        "lines_added": snapshot.lines_added,
        "lines_removed": snapshot.lines_removed,
        "thinking_blocks": snapshot.thinking_blocks,
        "subagents": snapshot.subagents,
        "skills": snapshot.skills,
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> SessionSnapshot {
        let mut s = SessionSnapshot::empty("/tmp/t.jsonl".into(), "abc123".to_owned());
        s.model_id = "claude-opus-5".to_owned();
        s.turns = 3;
        s.totals.output = 1_000;
        s
    }

    #[test]
    fn the_text_report_names_the_session_and_the_model() {
        let out = text(&snapshot());
        assert!(out.contains("abc123"));
        assert!(out.contains("Opus 5"));
    }

    #[test]
    fn the_json_report_carries_raw_numbers_not_abbreviations() {
        let out = json(&snapshot());
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(parsed["tokens"]["output"], 1_000);
        assert_eq!(parsed["turns"], 3);
    }

    #[test]
    fn a_session_with_no_activity_still_produces_a_report() {
        let empty = SessionSnapshot::empty("/tmp/t.jsonl".into(), "none".to_owned());
        assert!(text(&empty).contains("none"));
        assert!(serde_json::from_str::<serde_json::Value>(&json(&empty)).is_ok());
    }
}
