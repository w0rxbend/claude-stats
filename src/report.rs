//! The rendered output of every command that is not the dashboard: the text
//! and JSON reports behind `claude-stats stats`, the session listing behind
//! `sessions`, the price table behind `models`, the period tables behind
//! `daily`, `weekly`, `monthly` and `session`, and the five-hour windows behind
//! `blocks`.
//!
//! Every function here returns a `String` rather than printing, so the output
//! can be asserted on in a test without capturing stdout. The composition root
//! does the printing, and it is the only thing that knows which of stdout and
//! stderr a given sentence belongs on.
//!
//! Separate from the dashboard because the audiences are different. The
//! dashboard is watched; a report is read once, pasted into an issue, or piped
//! into `jq`. Both read the same [`SessionSnapshot`], so they cannot disagree
//! about what the session cost, and the period tables and their JSON are both
//! folded from the same [`UsageReport`] for the same reason.

use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::application::blocks_report::BlockRow;
use crate::application::ports::TranscriptRef;
use crate::domain::blocks::{BlockKind, BurnRate, LimitStanding, Projection};
use crate::domain::context::CompactionDistance;
use crate::domain::limits::{AccountUsage, WindowUsage};
use crate::domain::model::ModelId;
use crate::domain::period::Zone;
use crate::domain::pricing::{CostMode, PriceSheet};
use crate::domain::project::Project;
use crate::domain::report::{ModelBreakdown, UsageReport, UsageRow};
use crate::domain::session::SessionSnapshot;
use crate::view::{blocks_view, format, usage_view};

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
        format::tokens(snapshot.totals.cache_creation()),
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
    for (label, month) in [
        ("this month", &usage.this_month),
        ("last month", &usage.last_month),
    ] {
        // A month with nothing in it is skipped rather than printed as zero:
        // near the start of the scanned history "last month $0.00" would more
        // often mean "not scanned" than "nothing spent".
        if month.tokens.total() == 0 {
            continue;
        }
        row(
            out,
            label,
            format!(
                "{} tokens  {}  ({})",
                format::tokens(month.tokens.total()),
                month.cost,
                month.name(),
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
            "cache_creation": snapshot.totals.cache_creation(),
            "cache_creation_5m": snapshot.totals.cache_write_5m,
            "cache_creation_1h": snapshot.totals.cache_write_1h,
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
            "this_month": month_json(&usage.this_month),
            "last_month": month_json(&usage.last_month),
            "rate_limited_until": usage.active_limit().map(|l| l.resets_at),
            "limit_periods": usage.limit_events.len(),
        });
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned())
}

/// One calendar month, as raw numbers.
fn month_json(month: &crate::domain::limits::MonthUsage) -> serde_json::Value {
    json!({
        "starts": month.starts,
        "tokens": month.tokens.total(),
        "cost_usd": month.cost.dollars(),
        "sessions": month.sessions,
    })
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

// ---------------------------------------------------------------------------
// The period reports: `daily`, `weekly`, `monthly` and `session`.
//
// Two renderings of one [`UsageReport`], and deliberately two rather than one
// with a flag. The table rounds, drops columns on a narrow terminal and puts a
// short name where the transcript wrote a dated model id, all of which are the
// right things to do for a person reading it once. The JSON does none of them,
// because the thing on the other end of a pipe is going to do arithmetic. What
// they share is the aggregate: neither folds the entries itself, so the two
// cannot come to disagree about what a week cost.
// ---------------------------------------------------------------------------

/// What the table says when nothing matched.
///
/// It names the way out rather than merely reporting the absence, mirroring
/// the empty session listing above. The two reasons a range comes back empty
/// are that it really is empty and that the corpus is somewhere else, and only
/// one of those is something the reader can do anything about.
const EMPTY_REPORT: &str = "no usage found in that range (try widening --since/--until, \
     or set CLAUDE_CONFIG_DIR if Claude Code stores its state elsewhere)";

/// The heading every period table carries.
const REPORT_HEADING: &str = "Claude Code Token Usage Report";

/// Renders a period report as a table.
///
/// `title` names the report -- `Daily`, `Weekly`, `Monthly`, `By Session` --
/// and becomes the second half of the heading. `first_column` is the leftmost
/// column's own heading, `breakdown` adds a sub-row per model, and `width` is
/// how many columns the output has room for; below
/// [`usage_view::COMPACT_BELOW_COLUMNS`] the cache and total-token columns are
/// dropped rather than the table being wrapped.
///
/// The last two arguments are what the footer is made of, and they are here
/// rather than read from a global for the reason the footer exists at all: two
/// runs a release apart, or one with a price override file and one without,
/// print different figures for the same traffic, and a reader who cannot tell
/// which sheet produced a number cannot check it. By the time a report reaches
/// a renderer the sheet is long out of scope unless something hands it over.
#[must_use]
pub fn usage_table(
    report: &UsageReport,
    title: &str,
    first_column: &str,
    breakdown: bool,
    width: usize,
    mode: CostMode,
    prices: &PriceSheet,
) -> String {
    if report.rows.is_empty() {
        return format!("{EMPTY_REPORT}\n");
    }

    // The view module titles a table for whoever renders it; this command has
    // a heading of its own that a ccusage reader already recognises, so it is
    // substituted rather than printed above the one already there. `title` is
    // a public field precisely so a caller can name its own output.
    let mut view = usage_view::table(report, first_column, breakdown, width);
    view.title = format!("{REPORT_HEADING} - {title}");

    let mut out = view.render();
    let _ = writeln!(
        out,
        "\npriced with the {}, mode: {}",
        prices.provenance(),
        mode_name(mode)
    );
    out
}

/// What a cost mode is called in a footer.
///
/// Spelled here rather than derived from the enum's `Debug` because the footer
/// is output somebody reads and, in a `--json`-adjacent world, may well grep
/// for; a rename of a variant should not silently change it.
const fn mode_name(mode: CostMode) -> &'static str {
    match mode {
        CostMode::Auto => "auto",
        CostMode::Calculate => "calculate",
        CostMode::Display => "display",
    }
}

/// The two lines printed when a narrow terminal forced columns out of a table.
///
/// Returned as a string rather than printed so that the composition root can
/// decide where it goes -- which is **stderr**, always. A notice on stdout
/// would end up inside a redirected table or, worse, at the top of a JSON
/// document, and a document with a sentence of English in front of it is not a
/// document any longer.
#[must_use]
pub fn compact_notice(width: usize) -> String {
    format!(
        "the terminal is {width} columns wide, so the cache and total-token columns were dropped\n\
         widen it to at least {} columns, or pass --compact to keep this layout everywhere\n",
        usage_view::COMPACT_BELOW_COLUMNS
    )
}

/// The JSON key each report puts its rows under.
///
/// These four strings are a public interface: a script billing against this
/// output indexes by them, so they are named constants rather than literals
/// scattered through the module.
pub mod json_root {
    /// `claude-stats daily --json`.
    pub const DAILY: &str = "daily";
    /// `claude-stats weekly --json`.
    pub const WEEKLY: &str = "weekly";
    /// `claude-stats monthly --json`.
    pub const MONTHLY: &str = "monthly";
    /// `claude-stats session --json`.
    pub const SESSIONS: &str = "sessions";
}

/// Renders a period report as JSON.
///
/// `root` is the key the rows go under: one of the four in [`json_root`].
///
/// Built from the aggregate rather than from the [`view::table::TableView`] the
/// text report renders, which is the one design decision in this function
/// worth defending. Reading the table would be less code and would guarantee
/// the two agreed -- but it would also inherit everything the table does for a
/// human reader and a machine does not want: figures rounded to the cent and
/// grouped with commas, cache columns silently absent on a narrow terminal, and
/// short display names in place of the model ids a caller needs to match on.
/// The two renderings agree because they are folded from the same
/// [`UsageReport`], not because one is parsed out of the other.
///
/// Numbers are emitted exactly as they were computed. Rounding is a
/// presentation decision and belongs in the table.
///
/// [`view::table::TableView`]: crate::view::table::TableView
#[must_use]
pub fn usage_json(report: &UsageReport, root: &str) -> String {
    let rows: Vec<Value> = report
        .rows
        .iter()
        .map(|row| usage_row_json(row, root))
        .collect();

    let mut document = serde_json::Map::new();
    document.insert(root.to_owned(), Value::Array(rows));
    document.insert(
        "totals".to_owned(),
        Value::Object(figures_json(&report.totals)),
    );
    serde_json::to_string_pretty(&Value::Object(document)).unwrap_or_else(|_| "{}".to_owned())
}

/// Which field carries a row's identity, given the report it belongs to.
///
/// A daily report says `date`, a weekly one `week`, and so on. Falling back to
/// `date` for an unrecognised root rather than inventing a fifth name: the four
/// callers are all in this crate, and a document with a key nobody expects is
/// harder to diagnose than one with a familiar key in the wrong report.
fn key_field(root: &str) -> &'static str {
    match root {
        json_root::WEEKLY => "week",
        json_root::MONTHLY => "month",
        json_root::SESSIONS => "sessionId",
        _ => "date",
    }
}

/// One row, as the object a script indexes into.
fn usage_row_json(row: &UsageRow, root: &str) -> Value {
    let mut object = figures_json(row);
    let by_session = root == json_root::SESSIONS;

    let identity = if by_session {
        row.session
            .as_ref()
            .map_or_else(String::new, |session| session.as_str().to_owned())
    } else {
        row.key.to_string()
    };
    object.insert(key_field(root).to_owned(), Value::String(identity));

    if by_session {
        // A session is the one grouping where "which directory" and "when did
        // it run" are facts about the row rather than about the report, so
        // they are carried. A caller reconciling these figures against a
        // calendar has nothing else to reconcile them with.
        object.insert(
            "projectPath".to_owned(),
            json!(row.project.as_ref().map(Project::as_str)),
        );
        object.insert(
            "firstActivity".to_owned(),
            json!(stamp(row.first_activity_at)),
        );
        object.insert(
            "lastActivity".to_owned(),
            json!(stamp(row.last_activity_at)),
        );
    } else if let Some(project) = &row.project {
        // Only `daily --instances` splits a period by directory, so this
        // field is present exactly when the report was asked to produce it.
        object.insert("project".to_owned(), json!(project.as_str()));
    }

    Value::Object(object)
}

/// An instant as RFC 3339, or `null`.
///
/// `Z` rather than `+00:00`, and seconds rather than nanoseconds, because
/// that is the spelling every JSON date parser handles without being told and
/// the extra precision is not a fact the transcript records anyway.
fn stamp(at: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    at.map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// The eight fields every row and the totals row share.
///
/// The totals row is exactly this and nothing else. It has no identity to
/// carry -- no date, no session, no single directory, no one stretch of
/// activity -- so inventing fields for it with `null` in them would offer a
/// caller a key that can never hold anything.
fn figures_json(row: &UsageRow) -> serde_json::Map<String, Value> {
    let mut object = serde_json::Map::new();
    object.insert("inputTokens".to_owned(), json!(row.tokens.input));
    object.insert("outputTokens".to_owned(), json!(row.tokens.output));
    object.insert(
        "cacheCreationTokens".to_owned(),
        json!(row.tokens.cache_creation()),
    );
    object.insert("cacheReadTokens".to_owned(), json!(row.tokens.cache_read));
    object.insert("totalTokens".to_owned(), json!(row.tokens.total()));
    object.insert("totalCost".to_owned(), json!(row.cost.dollars()));
    object.insert(
        "modelsUsed".to_owned(),
        json!(row.models.iter().map(ModelId::as_str).collect::<Vec<_>>()),
    );
    object.insert(
        "modelBreakdowns".to_owned(),
        Value::Array(row.breakdown.iter().map(breakdown_json).collect()),
    );
    object
}

/// One model's share of a row.
///
/// The model is named by the id the transcript recorded, not by the short name
/// the table prints. `Opus 5` is a label for a reader; `claude-opus-5` is what
/// a caller can group by, price against and match to its own records.
fn breakdown_json(share: &ModelBreakdown) -> Value {
    json!({
        "modelName": share.model.as_str(),
        "inputTokens": share.tokens.input,
        "outputTokens": share.tokens.output,
        "cacheCreationTokens": share.tokens.cache_creation(),
        "cacheReadTokens": share.tokens.cache_read,
        "cost": share.cost.dollars(),
    })
}

// ---------------------------------------------------------------------------
// The blocks report: `claude-stats blocks`.
//
// Two renderings of one list of [`BlockRow`]s, split for the same reason the
// period reports are: the table rounds figures, drops columns on a narrow
// terminal and writes a block's start on the reader's own clock, all of which
// are right for somebody reading it once and wrong for a script. The JSON does
// none of them. Neither cuts the blocks itself -- both are handed what
// [`BlocksReport::run`] produced -- so the two cannot come to disagree about
// where a window opened.
//
// [`BlocksReport::run`]: crate::application::blocks_report::BlocksReport::run
// ---------------------------------------------------------------------------

/// What the blocks table says when nothing matched.
///
/// The wording every neighbouring tool uses for the same situation, so that a
/// script grepping for it keeps working. It is deliberately not the period
/// reports' message: those are always asked about a range and can suggest
/// widening it, whereas `blocks` with no flags at all covers the whole corpus
/// and there is nothing to widen.
const EMPTY_BLOCKS: &str = "no Claude usage data found";

/// The key the blocks JSON document puts its rows under.
///
/// In [`json_root`]'s company because it is the same kind of thing: a string a
/// caller indexes by, and therefore part of the interface rather than a
/// literal.
pub const BLOCKS_JSON_ROOT: &str = "blocks";

/// Renders the billing blocks as a table.
///
/// `zone` is the calendar the report was asked for, and it is what the `Block
/// Start` column is written on -- the same calendar `--since` and `--until`
/// were read on, so that one table is not quietly measured on two.
///
/// `width` is how many columns the output has room for; below
/// [`usage_view::COMPACT_BELOW_COLUMNS`] the `Tokens` and `[%]` columns are
/// dropped rather than the table being wrapped.
///
/// The footer carries the price sheet's provenance and the cost mode for the
/// same reason the period tables' does: two runs a release apart print
/// different figures for the same traffic, and a reader who cannot tell which
/// sheet produced a number cannot check it.
#[must_use]
pub fn blocks_table(
    rows: &[BlockRow],
    zone: &Zone,
    width: usize,
    mode: CostMode,
    prices: &PriceSheet,
) -> String {
    if rows.is_empty() {
        return format!("{EMPTY_BLOCKS}\n");
    }

    let mut out = blocks_view::table(rows, zone, width).render();
    let _ = writeln!(
        out,
        "\npriced with the {}, mode: {}",
        prices.provenance(),
        mode_name(mode)
    );
    out
}

/// The two lines printed when a narrow terminal forced columns out of the
/// blocks table.
///
/// Its own function rather than [`compact_notice`], because the two tables drop
/// different columns and a notice that named the wrong ones would send somebody
/// looking for figures that were never there. Both go on **stderr**, for the
/// reason given on the other one.
#[must_use]
pub fn blocks_compact_notice(width: usize) -> String {
    format!(
        "the terminal is {width} columns wide, so the token and percentage columns were dropped\n\
         widen it to at least {} columns, or pass --compact to keep this layout everywhere\n",
        usage_view::COMPACT_BELOW_COLUMNS
    )
}

/// Renders the billing blocks as JSON.
///
/// The document is `{"blocks": [...]}`, one object per block in the order they
/// happened, gaps included. A gap is carried rather than dropped for the same
/// reason the table prints a row for it: a caller charting a day's windows
/// needs to know that the silence was silence, not a block that failed to
/// serialise.
///
/// An empty result is an empty array rather than the sentence the table
/// prints. A document is what the other end of a pipe is parsing, and a
/// sentence of English is not one.
///
/// Numbers are emitted exactly as they were computed. Rounding is a
/// presentation decision and belongs in the table.
#[must_use]
pub fn blocks_json(rows: &[BlockRow]) -> String {
    let blocks: Vec<Value> = rows.iter().map(block_json).collect();
    let mut document = serde_json::Map::new();
    document.insert(BLOCKS_JSON_ROOT.to_owned(), Value::Array(blocks));
    serde_json::to_string_pretty(&Value::Object(document)).unwrap_or_else(|_| "{}".to_owned())
}

/// One block, as the object a script indexes into.
fn block_json(row: &BlockRow) -> Value {
    let block = &row.block;
    let is_gap = block.kind == BlockKind::Gap;
    let started = stamp(Some(block.started_at)).unwrap_or_default();

    let mut object = serde_json::Map::new();
    // The id is the start instant, which is unique among real blocks because
    // two of them cannot open in the same hour. A gap can share its start with
    // the block that follows it -- a gap opens a span after the last response,
    // so a window whose last response fell on the hour hands the next block the
    // same instant -- and it is prefixed rather than left to collide, because a
    // caller keying a map by this field would otherwise silently lose one of
    // the two.
    object.insert(
        "id".to_owned(),
        json!(if is_gap {
            format!("{GAP_ID_PREFIX}{started}")
        } else {
            started.clone()
        }),
    );
    object.insert("startTime".to_owned(), json!(started));
    object.insert("endTime".to_owned(), json!(stamp(Some(block.ends_at))));
    // Deliberately distinct from `endTime`. A window that saw one response at
    // nine and nothing afterwards still runs its full five hours, and a caller
    // measuring how long the work took needs the response rather than the
    // window.
    object.insert(
        "actualEndTime".to_owned(),
        json!(stamp(block.last_activity_at)),
    );
    object.insert(
        "isActive".to_owned(),
        json!(block.kind == BlockKind::Active),
    );
    object.insert("isGap".to_owned(), json!(is_gap));
    object.insert("entries".to_owned(), json!(block.entries));
    object.insert("tokenCounts".to_owned(), token_counts_json(block));
    object.insert("costUSD".to_owned(), json!(block.cost.dollars()));
    object.insert(
        "models".to_owned(),
        json!(block.models.iter().map(ModelId::as_str).collect::<Vec<_>>()),
    );

    // The three derived objects are absent rather than null when there is
    // nothing to derive: a single-response block has no rate, a finished one
    // has nothing to project into, and without a ceiling there is nothing to
    // stand against. A zero in any of them would read as measured.
    if let Some(rate) = row.rate {
        object.insert("burnRate".to_owned(), burn_rate_json(rate));
    }
    if let Some(projection) = row.projection {
        object.insert("projection".to_owned(), projection_json(&projection));
    }
    if let Some((standing, limit)) = row.standing.zip(row.limit) {
        if let Some(projection) = row.projection {
            object.insert(
                "tokenLimitStatus".to_owned(),
                limit_status_json(&projection, standing, limit),
            );
        }
    }
    Value::Object(object)
}

/// What marks a gap's id apart from the id of a block that opened in the same
/// hour.
const GAP_ID_PREFIX: &str = "gap-";

/// A block's five counters, under the names the neighbouring tools use.
///
/// The two cache-write leases are added together here, because the five-minute
/// and one-hour split is a pricing distinction rather than a reporting one and
/// no caller has a use for the halves. What it costs is already in `costUSD`,
/// charged at each lease's own rate.
fn token_counts_json(block: &crate::domain::blocks::BillingBlock) -> Value {
    json!({
        "inputTokens": block.tokens.input,
        "outputTokens": block.tokens.output,
        "cacheCreationInputTokens": block.tokens.cache_creation(),
        "cacheReadInputTokens": block.tokens.cache_read,
    })
}

/// How fast a block was consumed.
fn burn_rate_json(rate: BurnRate) -> Value {
    json!({
        "tokensPerMinute": rate.tokens_per_minute,
        "tokensPerMinuteForIndicator": rate.indicator_tokens_per_minute,
        "costPerHour": rate.cost_per_hour.dollars(),
    })
}

/// Where the running block lands if its rate holds.
fn projection_json(projection: &Projection) -> Value {
    json!({
        "totalTokens": projection.total_tokens,
        "totalCost": projection.cost.dollars(),
    })
}

/// How that projection sits against the ceiling.
///
/// `percentUsed` is emitted unrounded and uncapped: a projection of 140% is the
/// whole point of the figure, and a caller that wants it clamped can clamp it,
/// whereas one handed a clamped number can never get the real one back.
fn limit_status_json(projection: &Projection, standing: LimitStanding, limit: u64) -> Value {
    json!({
        "limit": limit,
        "projectedUsage": projection.total_tokens,
        "percentUsed": projection.total_tokens as f64 / limit as f64 * 100.0,
        "status": standing_name(standing),
    })
}

/// What a standing is called in the JSON.
///
/// Spelled out rather than derived from the enum's `Debug`, for the reason
/// [`mode_name`] is: a caller branches on these strings, so a rename of a
/// variant must not silently change them.
const fn standing_name(standing: LimitStanding) -> &'static str {
    match standing {
        LimitStanding::Ok => "ok",
        LimitStanding::Warning => "warning",
        LimitStanding::Exceeds => "exceeds",
    }
}

/// The price sheet, behind `claude-stats models`.
///
/// Walks `prices` itself rather than naming the models it means to show. It
/// used to carry a hand-written list of seven ids, which went stale the moment
/// a row was added to the catalogue: by the time this was changed it was
/// printing six of the fourteen models the tool actually prices, and nothing
/// in the output hinted that eight were missing. A table generated from the
/// sheet cannot drift away from the sheet.
///
/// The footer names the sheet's provenance, so that a figure somebody cannot
/// reproduce can be traced to the file that produced it.
#[must_use]
pub fn models(prices: &PriceSheet) -> String {
    let mut out = format!(
        "\n{:<14}{:>12}{:>10}{:>12}{:>12}{:>12}{:>10}\n",
        "MODEL", "CONTEXT", "INPUT", "CACHE READ", "WRITE 5M", "WRITE 1H", "OUTPUT"
    );
    let _ = writeln!(out, "{}", "-".repeat(82));
    for row in prices.rows() {
        let pricing = row.pricing;
        let _ = writeln!(
            out,
            "{:<14}{:>11}k{:>10.2}{:>12.2}{:>12.2}{:>12.2}{:>10.2}",
            row.display,
            row.context_window / 1_000,
            pricing.input.dollars_per_million(),
            pricing.cache_read.dollars_per_million(),
            pricing.cache_write_5m.dollars_per_million(),
            pricing.cache_write_1h.dollars_per_million(),
            pricing.output.dollars_per_million(),
        );
    }
    out.push_str("\nprices are US dollars per million tokens\n");
    out.push_str(
        "a cache write is charged by how long a lease it takes: five minutes or one hour\n",
    );
    let _ = writeln!(out, "rates: {}\n", prices.provenance());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    #[test]
    fn the_model_table_lists_every_headline_model_with_its_window() {
        let table = models(&PriceSheet::builtin());
        assert!(table.contains("Opus 5"));
        assert!(table.contains("Haiku 4.5"));
        assert!(table.contains("1000k"), "the 1M window should be shown");
        assert!(table.contains("200k"), "the 200k window should be shown");
    }

    #[test]
    fn a_model_added_to_the_catalogue_appears_in_the_model_table() {
        // The table used to name seven models by hand and had drifted to
        // showing six of the fourteen the tool prices. A reader has no way of
        // telling a short list from a complete one, so the only safe table is
        // one generated from the sheet itself.
        let sheet = PriceSheet::builtin();
        let table = models(&sheet);

        for row in sheet.rows() {
            assert!(
                table.contains(row.display.as_str()),
                "{} is priced but not listed",
                row.display
            );
        }
    }

    #[test]
    fn the_model_table_says_which_sheet_it_printed() {
        // Two runs a release apart print different rates. The footer is what
        // lets a reader tell "the price changed" from "my override file is
        // doing something I forgot about".
        assert!(models(&PriceSheet::builtin()).contains("rates: built-in price sheet"));
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
        use crate::domain::entry::{Entry, EntryId};
        use crate::domain::model::ModelId;
        use crate::domain::project::SessionId;
        use crate::domain::tokens::TokenUsage;

        let now = chrono::Utc::now();
        AccountUsage::measure(
            now,
            &[Entry {
                id: EntryId {
                    message_id: "msg_01".to_owned(),
                    request_id: Some("req_01".to_owned()),
                    session: SessionId::new("a"),
                },
                at: now,
                model: ModelId::new("claude-opus-5"),
                tokens: TokenUsage {
                    input: 250_000,
                    ..TokenUsage::ZERO
                },
                recorded_cost: None,
                session: SessionId::new("a"),
                project: Project::new("/home/ada/api"),
                is_sidechain: false,
            }],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
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

    // -----------------------------------------------------------------------
    // The period reports.
    // -----------------------------------------------------------------------

    use crate::domain::entry::{Entry, EntryId};
    use crate::domain::model::ModelId;
    use crate::domain::period::{AggregationPeriod, GroupingSpec, Zone};
    use crate::domain::pricing::CostMode;
    use crate::domain::project::SessionId;
    use crate::domain::report::UsageReport;
    use crate::domain::tokens::TokenUsage;

    fn at(stamp: &str) -> DateTime<chrono::Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    fn response(id: &str, when: &str, model: &str, session: &str, tokens: TokenUsage) -> Entry {
        Entry {
            id: EntryId {
                message_id: id.to_owned(),
                request_id: Some(format!("req_{id}")),
                session: SessionId::new(session),
            },
            at: at(when),
            model: ModelId::new(model),
            tokens,
            recorded_cost: None,
            session: SessionId::new(session),
            project: Project::new("/home/ada/Projects/api"),
            is_sidechain: false,
        }
    }

    /// Two fixed days of traffic whose every figure can be checked by hand.
    ///
    /// Opus 5 charges $5.00 per million input, $0.50 per million cache read,
    /// $6.25 per million five-minute writes and $25.00 per million output, so
    /// the first day is $5.00 + $1.00 + $2.50 + $5.00 = $13.50. Sonnet 5
    /// charges $2.00 / $0.20 / $2.50 / $10.00 against a fifth of the traffic,
    /// so the second is $1.00 + $0.20 + $0.25 + $0.50 = $1.95. Together
    /// $15.45, which is what the totals carry.
    fn two_days() -> UsageReport {
        let entries = [
            response(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-opus-5",
                "0f3a9c21-1b2c",
                TokenUsage {
                    input: 1_000_000,
                    cache_read: 2_000_000,
                    cache_write_5m: 400_000,
                    cache_write_1h: 0,
                    output: 200_000,
                },
            ),
            response(
                "b",
                "2026-09-02T17:30:00Z",
                "claude-sonnet-5",
                "0f3a9c21-1b2c",
                TokenUsage {
                    input: 500_000,
                    cache_read: 1_000_000,
                    cache_write_5m: 100_000,
                    cache_write_1h: 0,
                    output: 50_000,
                },
            ),
        ];
        UsageReport::build(
            &entries,
            &GroupingSpec {
                period: Some(AggregationPeriod::Day),
                ..GroupingSpec::default()
            },
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        )
    }

    fn empty_report() -> UsageReport {
        UsageReport::build(
            &[],
            &GroupingSpec {
                period: Some(AggregationPeriod::Day),
                ..GroupingSpec::default()
            },
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        )
    }

    #[test]
    fn the_period_table_is_headed_the_way_a_ccusage_reader_expects() {
        let out = usage_table(
            &two_days(),
            "Daily",
            "Date",
            false,
            200,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        assert!(
            out.starts_with("Claude Code Token Usage Report - Daily\n"),
            "the heading is missing or renamed: {out}"
        );
        assert!(out.contains("2026-09-01"));
        assert!(out.contains("$13.50"), "the day's own cost: {out}");
        assert!(out.contains("$15.45"), "and the total under it: {out}");
    }

    #[test]
    fn an_empty_report_says_where_it_looked_rather_than_printing_a_bare_header() {
        // A header with nothing under it says "there is nothing", which is one
        // of two possible truths. The other is that the corpus is somewhere
        // this run never looked, and that is the one the reader can fix.
        let out = usage_table(
            &empty_report(),
            "Daily",
            "Date",
            false,
            200,
            CostMode::Auto,
            &PriceSheet::builtin(),
        );

        assert!(out.contains("no usage found in that range"));
        assert!(
            out.contains("--since/--until"),
            "the first way out is a wider range: {out}"
        );
        assert!(
            out.contains("CLAUDE_CONFIG_DIR"),
            "the second is a relocated corpus: {out}"
        );
        assert!(
            !out.contains("Claude Code Token Usage Report"),
            "there is no table, so there is no heading: {out}"
        );
    }

    #[test]
    fn a_since_after_its_until_is_reported_as_an_empty_range_rather_than_as_everything() {
        // The query object already refuses to match anything for a reversed
        // range. What this pins is that the refusal reaches the reader as the
        // empty-report message rather than as a table of zeroes.
        let backwards = crate::application::ports::UsageQuery {
            since: Some(at("2026-09-30T00:00:00Z")),
            until: Some(at("2026-09-01T00:00:00Z")),
            ..crate::application::ports::UsageQuery::default()
        };
        let entry = response(
            "a",
            "2026-09-15T09:00:00Z",
            "claude-opus-5",
            "s",
            TokenUsage {
                input: 1_000,
                ..TokenUsage::ZERO
            },
        );
        assert!(!backwards.matches(&entry));

        let out = usage_table(
            &empty_report(),
            "Daily",
            "Date",
            false,
            200,
            CostMode::Auto,
            &PriceSheet::builtin(),
        );
        assert!(out.contains("no usage found in that range"));
    }

    #[test]
    fn the_footer_names_the_cost_mode_and_the_price_sheet_it_used() {
        // Two runs a release apart, or one with an override file and one
        // without, print different figures for the same traffic. The footer is
        // what lets a reader tell "the price changed" from "my figures are
        // wrong".
        let out = usage_table(
            &two_days(),
            "Daily",
            "Date",
            false,
            200,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        assert!(
            out.contains("priced with the built-in price sheet, mode: calculate"),
            "the footer is missing or reworded: {out}"
        );

        let automatic = usage_table(
            &two_days(),
            "Daily",
            "Date",
            false,
            200,
            CostMode::Auto,
            &PriceSheet::builtin(),
        );
        assert!(
            automatic.contains("mode: auto"),
            "the footer must report the mode it was given: {automatic}"
        );
    }

    #[test]
    fn asking_for_json_emits_no_compact_mode_notice_on_stdout() {
        // The notice belongs on stderr, and the way to be sure of that is for
        // neither thing printed to stdout to be able to contain it. A sentence
        // of English at the top of a JSON document is not a document any
        // longer, and a script that stops parsing there fails a long way from
        // the terminal that was too narrow.
        let narrow = usage_view::COMPACT_BELOW_COLUMNS - 20;
        let document = usage_json(&two_days(), json_root::DAILY);
        let table = usage_table(
            &two_days(),
            "Daily",
            "Date",
            false,
            narrow,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        let notice = compact_notice(narrow);

        assert!(
            serde_json::from_str::<Value>(&document).is_ok(),
            "the document must parse: {document}"
        );
        for line in notice.lines() {
            assert!(
                !document.contains(line),
                "the JSON carried the notice: {line}"
            );
            assert!(
                !table.contains(line),
                "the table carried the notice: {line}"
            );
        }
        // The notice itself still says both the width it found and the way out.
        assert!(notice.contains(&narrow.to_string()));
        assert!(notice.contains("--compact"));
        assert_eq!(notice.lines().count(), 2, "two lines, as documented");
    }

    #[test]
    fn a_narrow_table_drops_columns_rather_than_wrapping_them() {
        let narrow = usage_table(
            &two_days(),
            "Daily",
            "Date",
            false,
            usage_view::COMPACT_BELOW_COLUMNS - 1,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        assert!(!narrow.contains("Cache Read"), "{narrow}");
        assert!(
            narrow.contains("$15.45"),
            "the money survives the narrowing: {narrow}"
        );
    }

    #[test]
    fn the_json_report_is_stable_field_for_field() {
        // A golden document rather than a handful of field probes, because the
        // thing this test protects is somebody else's script: a renamed key or
        // a number that started arriving rounded has to fail here rather than
        // in their invoice. Every figure in it is the arithmetic spelled out on
        // `two_days` above.
        assert_eq!(usage_json(&two_days(), json_root::DAILY), GOLDEN_DAILY_JSON);
    }

    /// The daily document, spelled out.
    ///
    /// Keys come out in alphabetical order because `serde_json` stores an
    /// object in a `BTreeMap`, which is what makes a golden literal possible at
    /// all: the order is a property of the library rather than of the order
    /// this crate happened to insert them in.
    const GOLDEN_DAILY_JSON: &str = r#"{
  "daily": [
    {
      "cacheCreationTokens": 400000,
      "cacheReadTokens": 2000000,
      "date": "2026-09-01",
      "inputTokens": 1000000,
      "modelBreakdowns": [
        {
          "cacheCreationTokens": 400000,
          "cacheReadTokens": 2000000,
          "cost": 13.5,
          "inputTokens": 1000000,
          "modelName": "claude-opus-5",
          "outputTokens": 200000
        }
      ],
      "modelsUsed": [
        "claude-opus-5"
      ],
      "outputTokens": 200000,
      "totalCost": 13.5,
      "totalTokens": 3600000
    },
    {
      "cacheCreationTokens": 100000,
      "cacheReadTokens": 1000000,
      "date": "2026-09-02",
      "inputTokens": 500000,
      "modelBreakdowns": [
        {
          "cacheCreationTokens": 100000,
          "cacheReadTokens": 1000000,
          "cost": 1.95,
          "inputTokens": 500000,
          "modelName": "claude-sonnet-5",
          "outputTokens": 50000
        }
      ],
      "modelsUsed": [
        "claude-sonnet-5"
      ],
      "outputTokens": 50000,
      "totalCost": 1.95,
      "totalTokens": 1650000
    }
  ],
  "totals": {
    "cacheCreationTokens": 500000,
    "cacheReadTokens": 3000000,
    "inputTokens": 1500000,
    "modelBreakdowns": [
      {
        "cacheCreationTokens": 400000,
        "cacheReadTokens": 2000000,
        "cost": 13.5,
        "inputTokens": 1000000,
        "modelName": "claude-opus-5",
        "outputTokens": 200000
      },
      {
        "cacheCreationTokens": 100000,
        "cacheReadTokens": 1000000,
        "cost": 1.95,
        "inputTokens": 500000,
        "modelName": "claude-sonnet-5",
        "outputTokens": 50000
      }
    ],
    "modelsUsed": [
      "claude-opus-5",
      "claude-sonnet-5"
    ],
    "outputTokens": 250000,
    "totalCost": 15.45,
    "totalTokens": 5250000
  }
}"#;

    #[test]
    fn each_period_names_its_own_key_field() {
        // A script reading `daily` indexes rows by `date` and a script reading
        // `monthly` by `month`. Getting these wrong is invisible in a table and
        // fatal in a pipeline.
        for (root, field) in [
            (json_root::DAILY, "date"),
            (json_root::WEEKLY, "week"),
            (json_root::MONTHLY, "month"),
        ] {
            let parsed: Value =
                serde_json::from_str(&usage_json(&two_days(), root)).expect("valid json");
            assert!(parsed[root].is_array(), "{root} is missing its rows");
            assert!(
                parsed[root][0][field].is_string(),
                "{root} rows must carry {field}"
            );
        }
    }

    #[test]
    fn a_session_row_carries_its_project_and_the_hours_it_ran() {
        let report = UsageReport::build(
            &[
                response(
                    "a",
                    "2026-09-01T09:00:00Z",
                    "claude-opus-5",
                    "0f3a9c21-1b2c",
                    TokenUsage {
                        input: 1_000_000,
                        ..TokenUsage::ZERO
                    },
                ),
                response(
                    "b",
                    "2026-09-01T17:45:00Z",
                    "claude-sonnet-5",
                    "0f3a9c21-1b2c",
                    TokenUsage {
                        input: 500_000,
                        ..TokenUsage::ZERO
                    },
                ),
            ],
            &GroupingSpec {
                period: None,
                by_project: true,
                by_session: true,
                order: crate::domain::period::Order::Ascending,
            },
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );

        let parsed: Value =
            serde_json::from_str(&usage_json(&report, json_root::SESSIONS)).expect("valid json");
        let row = &parsed["sessions"][0];

        assert_eq!(row["sessionId"], "0f3a9c21-1b2c");
        assert_eq!(row["projectPath"], "/home/ada/Projects/api");
        assert_eq!(row["firstActivity"], "2026-09-01T09:00:00Z");
        assert_eq!(row["lastActivity"], "2026-09-01T17:45:00Z");
        assert_eq!(row["modelsUsed"][0], "claude-opus-5");
        assert!(
            parsed["totals"]["sessionId"].is_null(),
            "the total belongs to no one session"
        );
    }

    #[test]
    fn only_a_report_split_by_project_carries_a_project_on_its_rows() {
        let split = UsageReport::build(
            &[response(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-opus-5",
                "s",
                TokenUsage {
                    input: 1_000,
                    ..TokenUsage::ZERO
                },
            )],
            &GroupingSpec {
                period: Some(AggregationPeriod::Day),
                by_project: true,
                by_session: false,
                order: crate::domain::period::Order::Ascending,
            },
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        let parsed: Value =
            serde_json::from_str(&usage_json(&split, json_root::DAILY)).expect("valid json");
        assert_eq!(parsed["daily"][0]["project"], "/home/ada/Projects/api");

        let plain: Value =
            serde_json::from_str(&usage_json(&two_days(), json_root::DAILY)).expect("valid json");
        assert!(
            plain["daily"][0]["project"].is_null(),
            "a plain daily report was never asked which directory a day was spent in"
        );
    }

    #[test]
    fn the_json_carries_raw_model_ids_where_the_table_carries_short_names() {
        // The table is for a reader and says `Opus 5`. A caller has to match
        // these figures against its own records, and `claude-opus-5` is the
        // only spelling both sides have.
        let document = usage_json(&two_days(), json_root::DAILY);
        let table = usage_table(
            &two_days(),
            "Daily",
            "Date",
            true,
            200,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );

        assert!(document.contains("claude-opus-5"));
        assert!(!document.contains("Opus 5"));
        assert!(table.contains("Opus 5"));
        assert!(
            table.contains("\u{2514}\u{2500} Opus 5"),
            "--breakdown draws a sub-row: {table}"
        );
    }

    #[test]
    fn an_empty_report_is_still_a_document_a_script_can_read() {
        // The prose message is for a person reading a table. A pipeline gets
        // an empty list, because `jq '.daily | length'` returning zero is
        // something a script can act on and a sentence of English is not.
        let out = usage_json(&empty_report(), json_root::DAILY);
        let parsed: Value = serde_json::from_str(&out).expect("valid json");

        assert_eq!(parsed["daily"].as_array().map(Vec::len), Some(0));
        assert_eq!(parsed["totals"]["totalTokens"], 0);
        assert!(!out.contains("no usage found"));
    }

    // -----------------------------------------------------------------------
    // The blocks report.
    // -----------------------------------------------------------------------

    /// A finished window, a night's silence, and a running window.
    ///
    /// Every figure below is checkable by hand, and the two stretches are
    /// sixteen and thirty-two minutes long on purpose: dividing a whole number
    /// of dollars by a power of two is exact in binary, so the rates and the
    /// projection land on figures a golden literal can spell rather than on a
    /// trail of digits.
    ///
    /// The finished block holds two Opus 5 responses of 800,000 input tokens,
    /// sixteen minutes apart: 1,600,000 tokens at $5 a million is $8.00,
    /// 100,000 tokens a minute and $30.00 an hour. The running one holds two of
    /// 400,000 tokens, thirty-two minutes apart: 800,000 tokens for $4.00,
    /// 25,000 a minute and $7.50 an hour. Read at 10:00 it has 240 minutes of
    /// its window left, so it projects to 800,000 + 25,000 x 240 = 6,800,000
    /// tokens and $4.00 + $30.00 = $34.00 -- 85% of the 8,000,000 ceiling,
    /// which is past four fifths of it and therefore a warning.
    fn block_rows(limit: Option<u64>) -> Vec<BlockRow> {
        use crate::domain::entry::{Entry, EntryId};
        use crate::domain::project::{Project, SessionId};
        use crate::domain::tokens::TokenUsage;

        let at = |stamp: &str| -> chrono::DateTime<chrono::Utc> {
            stamp.parse().expect("a valid timestamp")
        };
        let entry = |id: &str, when: &str, input: u64| Entry {
            id: EntryId {
                message_id: id.to_owned(),
                request_id: Some(format!("req_{id}")),
                session: SessionId::new("session-a"),
            },
            at: at(when),
            model: ModelId::new("claude-opus-5"),
            tokens: TokenUsage {
                input,
                ..TokenUsage::ZERO
            },
            recorded_cost: None,
            session: SessionId::new("session-a"),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        };

        let now = at("2026-09-02T10:00:00Z");
        crate::domain::blocks::identify(
            &[
                entry("a", "2026-09-01T09:30:00Z", 800_000),
                entry("b", "2026-09-01T09:46:00Z", 800_000),
                entry("c", "2026-09-02T09:20:00Z", 400_000),
                entry("d", "2026-09-02T09:52:00Z", 400_000),
            ],
            chrono::Duration::hours(crate::domain::blocks::DEFAULT_SPAN_HOURS),
            now,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        )
        .into_iter()
        .map(|block| BlockRow::of(block, now, limit))
        .collect()
    }

    #[test]
    fn a_corpus_with_no_blocks_says_so_in_the_words_the_other_tools_use() {
        let out = blocks_table(
            &[],
            &Zone::Utc,
            200,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        assert_eq!(out, "no Claude usage data found\n");
    }

    #[test]
    fn the_blocks_table_says_which_sheet_priced_it() {
        // The same footer the period tables carry, for the same reason: two
        // runs a release apart print different figures for one window, and a
        // reader who cannot tell which sheet produced a number cannot check it.
        let out = blocks_table(
            &block_rows(Some(8_000_000)),
            &Zone::Utc,
            200,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );

        assert!(out.contains("Claude Code Token Usage Report - Session Blocks"));
        assert!(out.contains("ACTIVE"));
        assert!(out.contains("(inactive)"), "the gap is rendered: {out}");
        assert!(out.contains("PROJECTED"));
        assert!(out.contains("priced with the built-in price sheet, mode: calculate"));
    }

    #[test]
    fn the_blocks_json_is_stable_field_for_field() {
        // A golden document rather than a handful of probes, because what this
        // protects is somebody else's script: a renamed key, a gap that stopped
        // being emitted or a percentage that started arriving rounded has to
        // fail here rather than in their alerting. Every figure in it is the
        // arithmetic spelled out on `block_rows` above.
        assert_eq!(
            blocks_json(&block_rows(Some(8_000_000))),
            GOLDEN_BLOCKS_JSON
        );
    }

    #[test]
    fn a_block_with_nothing_to_derive_carries_no_invented_figures() {
        // A gap has no rate, no projection and no standing; a finished block
        // has a rate but nothing to project into. Emitting a zero for any of
        // them would put a figure on the page that reads as measured.
        let document: Value =
            serde_json::from_str(&blocks_json(&block_rows(None))).expect("valid json");
        let blocks = document[BLOCKS_JSON_ROOT]
            .as_array()
            .expect("an array of blocks");

        let gap = &blocks[1];
        assert_eq!(gap["isGap"], true);
        assert_eq!(gap["isActive"], false);
        assert_eq!(gap["entries"], 0);
        assert_eq!(gap["actualEndTime"], Value::Null);
        assert!(gap["burnRate"].is_null(), "a gap has no rate: {gap}");
        assert!(gap["projection"].is_null());
        assert!(
            gap["id"].as_str().is_some_and(|id| id.starts_with("gap-")),
            "a gap's id is marked apart from the block that ended in the same hour: {gap}"
        );

        let finished = &blocks[0];
        assert!(finished["burnRate"].is_object(), "{finished}");
        assert!(
            finished["projection"].is_null(),
            "a window that has closed cannot grow: {finished}"
        );

        // With no ceiling nothing stands against anything, however live the
        // block is.
        let live = &blocks[2];
        assert_eq!(live["isActive"], true);
        assert!(live["projection"].is_object());
        assert!(live["tokenLimitStatus"].is_null(), "{live}");
    }

    /// The blocks document, spelled out.
    ///
    /// Keys come out alphabetically because `serde_json` stores an object in a
    /// `BTreeMap`, which is what makes a golden literal possible at all.
    const GOLDEN_BLOCKS_JSON: &str = r#"{
  "blocks": [
    {
      "actualEndTime": "2026-09-01T09:46:00Z",
      "burnRate": {
        "costPerHour": 30.0,
        "tokensPerMinute": 100000.0,
        "tokensPerMinuteForIndicator": 100000.0
      },
      "costUSD": 8.0,
      "endTime": "2026-09-01T14:00:00Z",
      "entries": 2,
      "id": "2026-09-01T09:00:00Z",
      "isActive": false,
      "isGap": false,
      "models": [
        "claude-opus-5"
      ],
      "startTime": "2026-09-01T09:00:00Z",
      "tokenCounts": {
        "cacheCreationInputTokens": 0,
        "cacheReadInputTokens": 0,
        "inputTokens": 1600000,
        "outputTokens": 0
      }
    },
    {
      "actualEndTime": null,
      "costUSD": 0.0,
      "endTime": "2026-09-02T09:20:00Z",
      "entries": 0,
      "id": "gap-2026-09-01T14:46:00Z",
      "isActive": false,
      "isGap": true,
      "models": [],
      "startTime": "2026-09-01T14:46:00Z",
      "tokenCounts": {
        "cacheCreationInputTokens": 0,
        "cacheReadInputTokens": 0,
        "inputTokens": 0,
        "outputTokens": 0
      }
    },
    {
      "actualEndTime": "2026-09-02T09:52:00Z",
      "burnRate": {
        "costPerHour": 7.5,
        "tokensPerMinute": 25000.0,
        "tokensPerMinuteForIndicator": 25000.0
      },
      "costUSD": 4.0,
      "endTime": "2026-09-02T14:00:00Z",
      "entries": 2,
      "id": "2026-09-02T09:00:00Z",
      "isActive": true,
      "isGap": false,
      "models": [
        "claude-opus-5"
      ],
      "projection": {
        "totalCost": 34.0,
        "totalTokens": 6800000
      },
      "startTime": "2026-09-02T09:00:00Z",
      "tokenCounts": {
        "cacheCreationInputTokens": 0,
        "cacheReadInputTokens": 0,
        "inputTokens": 800000,
        "outputTokens": 0
      },
      "tokenLimitStatus": {
        "limit": 8000000,
        "percentUsed": 85.0,
        "projectedUsage": 6800000,
        "status": "warning"
      }
    }
  ]
}"#;
}
