//! The rendered output of every command that is not the dashboard: the text
//! and JSON reports behind `claude-stats stats`, the session listing behind
//! `sessions`, and the price table behind `models`.
//!
//! Every function here returns a `String` rather than printing, so the output
//! can be asserted on in a test without capturing stdout. The composition root
//! does the printing.
//!
//! Separate from the dashboard because the audiences are different. The
//! dashboard is watched; a report is read once, pasted into an issue, or piped
//! into `jq`. Both read the same [`SessionSnapshot`], so they cannot disagree
//! about what the session cost.

use std::fmt::Write as _;

use serde_json::json;

use crate::application::ports::TranscriptRef;
use crate::domain::context::CompactionDistance;
use crate::domain::limits::{AccountUsage, WindowUsage};
use crate::domain::model::ModelCatalog;
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
pub fn text(snapshot: &SessionSnapshot, usage: Option<&AccountUsage>) -> String {
    let mut out = format!("\nsession {}\n", snapshot.session_id);
    // One helper per section, because the four sections are what a reader
    // actually scans for -- who am I looking at, how full is it, what did it
    // cost, what did it do -- and a single long function hides that shape.
    identity(&mut out, snapshot);
    context(&mut out, snapshot);
    spend(&mut out, snapshot);
    activity(&mut out, snapshot);
    if let Some(usage) = usage {
        account(&mut out, usage);
    }
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

/// The account-wide windows, which are about every session rather than this
/// one. Printed only when a reading was taken, so `stats` on a machine where
/// scanning failed stays quiet rather than printing zeroes.
fn account(out: &mut String, usage: &AccountUsage) {
    out.push('\n');
    for window in [&usage.session, &usage.week] {
        row(
            out,
            &format!("last {}", window.kind.span_label()),
            format!(
                "{} tokens  {}  ({} session{}{})",
                format::tokens(window.tokens.total()),
                window.cost,
                window.sessions,
                if window.sessions == 1 { "" } else { "s" },
                match window.limit_periods {
                    0 => String::new(),
                    1 => ", 1 limit hit".to_owned(),
                    n => format!(", {n} limits hit"),
                }
            ),
        );
    }
    if let Some(limit) = usage.active_limit() {
        row(
            out,
            "rate limited",
            format!(
                "yes, resets in {}",
                limit
                    .time_until_reset(usage.measured_at)
                    .map_or_else(|| "any moment".to_owned(), format::duration)
            ),
        );
    }
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
pub fn json(snapshot: &SessionSnapshot, usage: Option<&AccountUsage>) -> String {
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
    let mut value = value;
    if let Some(usage) = usage {
        value["account"] = json!({
            "measured_at": usage.measured_at,
            "last_5h": window_json(&usage.session),
            "last_7d": window_json(&usage.week),
            "rate_limited_until": usage.active_limit().map(|l| l.resets_at),
            "limit_periods": usage.limit_events.len(),
        });
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned())
}

/// One rolling window, as raw numbers.
fn window_json(window: &WindowUsage) -> serde_json::Value {
    json!({
        "since": window.since,
        "tokens": window.tokens.total(),
        "cost_usd": window.cost.dollars(),
        "sessions": window.sessions,
        "limit_periods": window.limit_periods,
        "last_limit_at": window.last_limit_at,
        // Named for what it is. There is deliberately no "share of limit"
        // here: that number is not knowable from a transcript.
        "peak_comparable_window_tokens": window.peak,
        "share_of_peak": window.share_of_peak(),
    })
}

/// The session listing behind `claude-stats sessions`.
///
/// `limit` caps the rows shown; anything beyond it is reported as a count so
/// the reader knows the list was cut rather than assuming it was complete.
#[must_use]
pub fn sessions(all: &[TranscriptRef], limit: usize) -> String {
    if all.is_empty() {
        return "no Claude Code sessions found under ~/.claude/projects \
                (set CLAUDE_CONFIG_DIR if Claude Code stores its state elsewhere)\n"
            .to_owned();
    }

    let mut out = format!(
        "{:<10}  {:<16}  {:>8}  PROJECT\n",
        "SESSION", "MODIFIED", "SIZE"
    );
    for session in all.iter().take(limit) {
        let _ = writeln!(
            out,
            "{:<10}  {:<16}  {:>7}K  {}",
            format::session_id(&session.session_id),
            session.modified_at.format("%Y-%m-%d %H:%M"),
            session.size_bytes / 1_024,
            session.project_dir,
        );
    }
    if all.len() > limit {
        let _ = writeln!(out, "\n... and {} more (use --limit)", all.len() - limit);
    }
    out
}

/// The model catalogue and its prices, behind `claude-stats models`.
#[must_use]
pub fn models() -> String {
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
    use chrono::DateTime;

    #[test]
    fn the_model_table_lists_every_headline_model_with_its_window() {
        let table = models();
        assert!(table.contains("Opus 5"));
        assert!(table.contains("Haiku 4.5"));
        assert!(table.contains("1000k"), "the 1M window should be shown");
        assert!(table.contains("200k"), "the 200k window should be shown");
    }

    fn transcript(id: &str, project: &str, size_bytes: u64) -> TranscriptRef {
        TranscriptRef {
            path: format!("/tmp/{id}.jsonl").into(),
            session_id: id.to_owned(),
            project_dir: project.to_owned(),
            modified_at: DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
            size_bytes,
        }
    }

    #[test]
    fn an_empty_session_list_says_where_it_looked() {
        let out = sessions(&[], 10);
        assert!(out.contains("no Claude Code sessions found"));
        assert!(
            out.contains("CLAUDE_CONFIG_DIR"),
            "a user with a relocated config needs the way out: {out:?}"
        );
    }

    #[test]
    fn the_session_list_truncates_ids_and_reports_sizes_in_kibibytes() {
        let out = sessions(
            &[transcript("0f3a9c21-1b2c-4d5e", "/home/me/work", 5_120)],
            10,
        );
        assert!(out.contains("SESSION"), "the header is missing: {out:?}");
        assert!(out.contains("0f3a9c21"), "the id should be cut to eight");
        assert!(
            !out.contains("0f3a9c21-1b2c"),
            "the full id should not be shown: {out:?}"
        );
        assert!(out.contains("5K"), "5120 bytes is 5K: {out:?}");
        assert!(out.contains("/home/me/work"));
    }

    #[test]
    fn a_truncated_session_list_says_how_many_it_hid() {
        let all = [
            transcript("aaaaaaaa-1", "/a", 1_024),
            transcript("bbbbbbbb-2", "/b", 1_024),
            transcript("cccccccc-3", "/c", 1_024),
        ];

        let out = sessions(&all, 1);

        assert!(out.contains("aaaaaaaa"));
        assert!(
            !out.contains("bbbbbbbb"),
            "the limit should have cut this row: {out:?}"
        );
        assert!(
            out.contains("and 2 more"),
            "a cut list must say it was cut: {out:?}"
        );
    }

    fn snapshot() -> SessionSnapshot {
        let mut s = SessionSnapshot::empty("/tmp/t.jsonl".into(), "abc123".to_owned());
        s.model_id = "claude-opus-5".to_owned();
        s.turns = 3;
        s.totals.output = 1_000;
        s
    }

    #[test]
    fn the_text_report_names_the_session_and_the_model() {
        let out = text(&snapshot(), None);
        assert!(out.contains("abc123"));
        assert!(out.contains("Opus 5"));
    }

    #[test]
    fn the_json_report_carries_raw_numbers_not_abbreviations() {
        let out = json(&snapshot(), None);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(parsed["tokens"]["output"], 1_000);
        assert_eq!(parsed["turns"], 3);
    }

    fn account_usage() -> AccountUsage {
        use crate::domain::limits::{SessionContribution, UsagePoint};
        use crate::domain::money::Usd;
        use crate::domain::tokens::TokenUsage;

        let now = chrono::Utc::now();
        AccountUsage::measure(
            now,
            &[SessionContribution {
                session_id: "a".to_owned(),
                points: vec![UsagePoint {
                    at: now,
                    tokens: TokenUsage {
                        input: 250_000,
                        ..TokenUsage::ZERO
                    },
                    cost: Usd::new(7.5),
                }],
            }],
            Vec::new(),
        )
    }

    #[test]
    fn the_report_carries_the_account_windows_when_they_were_measured() {
        let usage = account_usage();
        let out = text(&snapshot(), Some(&usage));

        assert!(out.contains("last 5h"));
        assert!(out.contains("last 7d"));
        assert!(out.contains("250.0k tokens"));
    }

    #[test]
    fn without_a_reading_the_report_says_nothing_about_the_account() {
        let out = text(&snapshot(), None);
        assert!(!out.contains("last 5h"), "no zeroes invented");
    }

    #[test]
    fn the_json_account_block_never_claims_a_share_of_any_limit() {
        let usage = account_usage();
        let out = json(&snapshot(), Some(&usage));
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");

        assert_eq!(parsed["account"]["last_5h"]["tokens"], 250_000);
        assert!(parsed["account"]["rate_limited_until"].is_null());
        // The only ceiling named anywhere is the user's own peak window.
        assert!(!out.contains("limit_share"));
        assert!(parsed["account"]["last_5h"]["peak_comparable_window_tokens"].is_null());
    }

    #[test]
    fn a_session_with_no_activity_still_produces_a_report() {
        let empty = SessionSnapshot::empty("/tmp/t.jsonl".into(), "none".to_owned());
        assert!(text(&empty, None).contains("none"));
        assert!(serde_json::from_str::<serde_json::Value>(&json(&empty, None)).is_ok());
    }
}
