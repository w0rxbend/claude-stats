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
    let mut out = format!("\nsession {}\n", snapshot.session_id);
    // One helper per section, because the four sections are what a reader
    // actually scans for -- who am I looking at, how full is it, what did it
    // cost, what did it do -- and a single long function hides that shape.
    identity(&mut out, snapshot);
    context(&mut out, snapshot);
    spend(&mut out, snapshot);
    activity(&mut out, snapshot);
    out.push('\n');
    out
}

fn identity(out: &mut String, snapshot: &SessionSnapshot) {
    out.push('\n');
    row(out, "model", snapshot.model_display_name());
    row(
        out,
        "project",
        snapshot.project_dir.as_deref().unwrap_or("-"),
    );
    row(out, "branch", snapshot.git_branch.as_deref().unwrap_or("-"));
    row(
        out,
        "elapsed",
        snapshot
            .duration()
            .map_or_else(|| "-".to_owned(), format::duration),
    );
}

fn context(out: &mut String, snapshot: &SessionSnapshot) {
    let fill = snapshot.context_fill();
    out.push('\n');
    row(
        out,
        "context",
        format!(
            "{} / {}  ({})",
            format::tokens(fill.used()),
            format::tokens(fill.window()),
            format::percent_precise(fill.ratio())
        ),
    );
    row(
        out,
        "until compaction",
        match snapshot.compaction_distance() {
            CompactionDistance::Imminent => "imminent".to_owned(),
            CompactionDistance::Turns(n) => format!("~{n} turns"),
            CompactionDistance::Unknown => "unknown".to_owned(),
        },
    );
    row(out, "compactions", snapshot.compactions.len());
}

fn spend(out: &mut String, snapshot: &SessionSnapshot) {
    out.push('\n');
    row(
        out,
        "cost",
        format!("{}  ({}/turn)", snapshot.cost(), snapshot.cost_per_turn()),
    );
    row(
        out,
        "cache hit ratio",
        snapshot
            .cache_hit_ratio()
            .map_or_else(|| "-".to_owned(), format::percent_precise),
    );
    row(out, "input tokens", format::tokens(snapshot.totals.input));
    row(
        out,
        "cache reads",
        format::tokens(snapshot.totals.cache_read),
    );
    row(
        out,
        "cache writes",
        format::tokens(snapshot.totals.cache_creation),
    );
    row(out, "output tokens", format::tokens(snapshot.totals.output));
}

fn activity(out: &mut String, snapshot: &SessionSnapshot) {
    out.push('\n');
    row(out, "turns", snapshot.turns);
    row(out, "tool calls", snapshot.tool_calls());
    row(out, "tool errors", snapshot.tool_errors);
    row(out, "files touched", snapshot.files_touched());
    row(
        out,
        "lines",
        format!("+{} -{}", snapshot.lines_added, snapshot.lines_removed),
    );
    row(out, "thinking blocks", snapshot.thinking_blocks);
    row(out, "sub-agents", snapshot.subagents);
    row(out, "skills", snapshot.skills);

    if snapshot.tool_counts.is_empty() {
        return;
    }
    out.push_str("\n  top tools\n");
    let mut ranked: Vec<_> = snapshot.tool_counts.iter().collect();
    // Most-used first, then alphabetically, so a tie renders the same way on
    // every run and the report can be diffed.
    ranked.sort_by_key(|(name, count)| (std::cmp::Reverse(**count), (*name).clone()));
    for (name, count) in ranked.iter().take(8) {
        let _ = writeln!(out, "    {name:<18}{count}");
    }
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
