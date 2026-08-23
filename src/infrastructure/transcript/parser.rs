//! Walks a transcript and builds a [`SessionSnapshot`] out of it.
//!
//! The transcript is append-only, so a single forward pass is enough and the
//! parser never needs to seek. State that spans lines -- which tool calls are
//! still waiting for a result, what the context fill was just before a
//! compaction -- lives in [`ParseState`] rather than in a pile of local
//! variables, so the per-line handlers stay short enough to read in one go.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Utc};

use super::records::{Block, Record};
use crate::application::ports::{SessionReader, TranscriptRef};
use crate::domain::activity::{ToolEvent, ToolKind};
use crate::domain::session::{
    CompactionEvent, EVENT_LOG_CAPACITY, LogEntry, LogLevel, RECENT_TOOL_CAPACITY, ResponseSample,
    SessionPhase, SessionSnapshot, TurnCounters,
};

/// How much of a tool's subject (a command, a search pattern) is kept.
///
/// Long enough to identify the call, short enough that one entry cannot push
/// everything else out of a narrow terminal.
const SUBJECT_MAX_CHARS: usize = 60;

/// How much of an error message is kept for the status line.
const ERROR_MAX_CHARS: usize = 200;

/// Reads transcripts off the local filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct TranscriptParser;

impl SessionReader for TranscriptParser {
    fn read(&self, transcript: &TranscriptRef) -> anyhow::Result<SessionSnapshot> {
        let contents = std::fs::read_to_string(&transcript.path)?;
        Ok(Self::parse(
            &transcript.path,
            &transcript.session_id,
            &contents,
        ))
    }
}

impl TranscriptParser {
    /// Parses transcript text into a snapshot.
    ///
    /// Exposed separately from [`SessionReader::read`] so the parsing rules
    /// can be tested against a string literal without touching the disk.
    #[must_use]
    pub fn parse(path: &Path, session_id: &str, contents: &str) -> SessionSnapshot {
        let mut state = ParseState::new(path.to_path_buf(), session_id.to_owned());
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // A malformed line is skipped, not fatal. The last line of a live
            // transcript is routinely half-written, and a dashboard that
            // refuses to draw because of it would fail exactly when it is
            // most wanted.
            if let Ok(record) = serde_json::from_str::<Record>(line) {
                state.absorb(&record);
            }
        }
        state.finish()
    }
}

/// Everything the parser needs to remember between lines.
struct ParseState {
    snapshot: SessionSnapshot,
    /// The turn number currently being filled in.
    current_turn: u32,
    /// Distinct `tool_use` ids for sub-agent spawns, so re-reads of the same
    /// transcript do not inflate the count.
    subagent_ids: HashSet<String>,
    /// `tool_use` id to a label, for the calls whose *result* is interesting:
    /// sub-agents (which report back) and skills (which end when they return).
    pending_labels: HashMap<String, PendingLabel>,
    /// Prompt tokens of the most recent response, i.e. the live context fill.
    last_context: u64,
    /// Set when a compaction boundary has been seen but the rebuilt context
    /// has not been measured yet. The next assistant usage supplies it.
    awaiting_rebuild: bool,
}

/// What a still-unanswered tool call was.
enum PendingLabel {
    /// A sub-agent, with the description it was spawned under.
    Agent(String),
    /// A skill, with its name.
    Skill(String),
}

impl ParseState {
    fn new(path: std::path::PathBuf, session_id: String) -> Self {
        Self {
            snapshot: SessionSnapshot::empty(path, session_id),
            current_turn: 0,
            subagent_ids: HashSet::new(),
            pending_labels: HashMap::new(),
            last_context: 0,
            awaiting_rebuild: false,
        }
    }

    fn finish(mut self) -> SessionSnapshot {
        self.snapshot.subagents = self.subagent_ids.len() as u32;
        self.snapshot.turn.agents_running = self
            .pending_labels
            .values()
            .filter(|l| matches!(l, PendingLabel::Agent(_)))
            .count() as u32;
        self.snapshot
    }

    /// Folds one transcript line into the snapshot.
    fn absorb(&mut self, record: &Record) {
        // Sub-agent traffic is a conversation of its own with its own token
        // budget. Counting it here would make the main thread's context fill
        // jump around for work that never entered the main window.
        if record.is_sidechain {
            return;
        }

        if let Some(ts) = record.timestamp {
            if self.snapshot.started_at.is_none() {
                self.snapshot.started_at = Some(ts);
            }
            self.snapshot.last_activity_at = Some(ts);
        }
        if self.snapshot.project_dir.is_none() {
            self.snapshot.project_dir.clone_from(&record.cwd);
        }
        if record.git_branch.is_some() {
            self.snapshot.git_branch.clone_from(&record.git_branch);
        }

        match record.r#type.as_str() {
            "assistant" => self.absorb_assistant(record),
            "user" => self.absorb_user(record),
            "summary" => self.absorb_compaction(record),
            "system" if record.subtype.as_deref() == Some("compact_boundary") => {
                self.absorb_compaction(record);
            }
            _ => {}
        }
    }

    // ── user entries ──────────────────────────────────────────────────

    /// Handles a `user` entry, which is one of two very different things.
    ///
    /// A person typing at the prompt starts a new turn. A tool handing back a
    /// result does not -- it arrives wearing the same `user` type, and
    /// treating it as a turn would multiply the turn count by the number of
    /// tools Claude happened to call.
    fn absorb_user(&mut self, record: &Record) {
        let Some(message) = &record.message else {
            return;
        };

        for block in message.content.blocks() {
            if block.r#type == "tool_result" {
                self.absorb_tool_result(record, block);
            }
        }

        if record.is_meta || record.is_compact_summary || !message.content.has_user_text() {
            return;
        }
        self.begin_turn(record.timestamp);
    }

    fn begin_turn(&mut self, at: Option<DateTime<Utc>>) {
        self.current_turn += 1;
        self.snapshot.turns += 1;
        self.snapshot.turns_since_compaction += 1;
        self.snapshot.last_user_at = at;
        self.snapshot.phase = SessionPhase::Thinking;
        // The live feed and the per-turn counters are about the turn in
        // progress, so a new turn wipes them.
        self.snapshot.recent_tools.clear();
        self.snapshot.turn = TurnCounters::default();
    }

    fn absorb_tool_result(&mut self, record: &Record, block: &Block) {
        let id = block.tool_use_id.clone().unwrap_or_default();

        if block.is_error {
            self.snapshot.tool_errors += 1;
            self.snapshot.turn.tool_errors += 1;
            let text = block.result_text().map_or_else(
                || "tool failed".to_owned(),
                |t| truncate(t.trim(), ERROR_MAX_CHARS),
            );
            self.snapshot.last_error = Some(text.clone());
            self.mark_failed(&id);
            self.log(record.timestamp, LogLevel::Error, format!("error: {text}"));
        }

        match self.pending_labels.remove(&id) {
            Some(PendingLabel::Agent(label)) => {
                self.log(
                    record.timestamp,
                    LogLevel::Notice,
                    format!("agent finished: {label}"),
                );
            }
            Some(PendingLabel::Skill(name)) => {
                self.snapshot.turn.active_skill = None;
                self.log(
                    record.timestamp,
                    LogLevel::Notice,
                    format!("skill finished: /{name}"),
                );
            }
            None => {}
        }
    }

    /// Flags the queued tool call that this failed result answers.
    ///
    /// The call was recorded when it was made, several lines earlier; the feed
    /// shows it in red only once its result comes back as an error.
    fn mark_failed(&mut self, tool_use_id: &str) {
        if let Some(event) = self
            .snapshot
            .recent_tools
            .iter_mut()
            .rev()
            .find(|e| e.id == tool_use_id)
        {
            event.failed = true;
        }
    }

    // ── assistant entries ─────────────────────────────────────────────

    fn absorb_assistant(&mut self, record: &Record) {
        let Some(message) = &record.message else {
            return;
        };

        if self.snapshot.model_id.is_empty() {
            if let Some(model) = &message.model {
                self.snapshot.model_id.clone_from(model);
            }
        }

        self.snapshot.responses += 1;
        self.snapshot.phase = SessionPhase::Idle;

        let mut had_thinking = false;
        for block in message.content.blocks() {
            match block.r#type.as_str() {
                "thinking" => had_thinking = true,
                "tool_use" => self.absorb_tool_use(record, block),
                _ => {}
            }
        }
        if had_thinking {
            self.snapshot.thinking_blocks += 1;
            self.snapshot.turn.thinking_blocks += 1;
        }

        if let Some(usage) = message.usage {
            self.absorb_usage(record, usage.into());
        }
    }

    fn absorb_usage(&mut self, record: &Record, usage: crate::domain::tokens::TokenUsage) {
        self.snapshot.totals += usage;
        let prompt_tokens = usage.prompt_tokens();
        self.last_context = prompt_tokens;

        // The first response after a compaction tells us what the rebuilt
        // conversation cost. That figure is the compaction's real price, so
        // it is written back onto the event rather than being lost.
        if self.awaiting_rebuild {
            self.awaiting_rebuild = false;
            if let Some(event) = self.snapshot.compactions.last_mut() {
                event.context_after = prompt_tokens;
            }
        }

        self.snapshot.samples.push(ResponseSample {
            turn: self.current_turn,
            prompt_tokens,
            output_tokens: usage.output,
            at: record.timestamp.unwrap_or_else(Utc::now),
        });
    }

    fn absorb_tool_use(&mut self, record: &Record, block: &Block) {
        let name = block.name.clone().unwrap_or_else(|| "unknown".to_owned());
        let id = block.id.clone().unwrap_or_default();
        let kind = ToolKind::classify(&name);

        *self.snapshot.tool_counts.entry(name.clone()).or_insert(0) += 1;
        *self.snapshot.turn.tools.entry(name.clone()).or_insert(0) += 1;
        *self.snapshot.kind_counts.entry(kind).or_insert(0) += 1;

        let subject = match kind {
            ToolKind::Agent => self.register_agent(record, block, &id),
            ToolKind::Skill => self.register_skill(record, block, &id),
            ToolKind::Write => self.register_file_edit(block),
            ToolKind::Read => Self::register_file_read(&mut self.snapshot, block),
            _ => Self::describe_tool(block),
        };

        self.log(
            record.timestamp,
            LogLevel::Info,
            if subject.is_empty() {
                name.to_lowercase()
            } else {
                format!("{} {subject}", name.to_lowercase())
            },
        );

        push_capped(
            &mut self.snapshot.recent_tools,
            ToolEvent {
                at: record.timestamp.unwrap_or_else(Utc::now),
                name,
                kind,
                subject,
                failed: false,
                id,
            },
            RECENT_TOOL_CAPACITY,
        );
    }

    fn register_agent(&mut self, _record: &Record, block: &Block, id: &str) -> String {
        let label = block
            .input_str("description")
            .or_else(|| block.input_str("subagent_type"))
            .unwrap_or("subagent")
            .to_owned();
        if !id.is_empty() {
            self.subagent_ids.insert(id.to_owned());
            self.pending_labels
                .insert(id.to_owned(), PendingLabel::Agent(label.clone()));
        }
        self.snapshot.turn.agents_spawned += 1;
        label
    }

    fn register_skill(&mut self, _record: &Record, block: &Block, id: &str) -> String {
        let name = block.input_str("skill").unwrap_or("skill").to_owned();
        self.snapshot.skills += 1;
        self.snapshot.turn.active_skill = Some(name.clone());
        if !id.is_empty() {
            self.pending_labels
                .insert(id.to_owned(), PendingLabel::Skill(name.clone()));
        }
        format!("/{name}")
    }

    fn register_file_edit(&mut self, block: &Block) -> String {
        let Some(path) = block
            .input_str("file_path")
            .or_else(|| block.input_str("path"))
        else {
            return Self::describe_tool(block);
        };
        let file = base_name(path);
        bump(&mut self.snapshot.files_edited, &file);
        bump(&mut self.snapshot.turn.files_edited, &file);
        self.count_changed_lines(block);
        file
    }

    /// Adds up the lines an edit or write touched.
    ///
    /// This is an approximation, and deliberately so: the transcript records
    /// the strings that were swapped, not a diff. Counting newlines in the old
    /// and new text gives a figure that tracks the size of the change well
    /// enough to be worth showing, without re-implementing a diff algorithm
    /// for a number that lives in one corner of one panel.
    fn count_changed_lines(&mut self, block: &Block) {
        if let Some(content) = block.input_str("content") {
            self.snapshot.lines_added += line_count(content);
            return;
        }
        if let (Some(old), Some(new)) =
            (block.input_str("old_string"), block.input_str("new_string"))
        {
            self.snapshot.lines_removed += line_count(old);
            self.snapshot.lines_added += line_count(new);
        }
    }

    fn register_file_read(snapshot: &mut SessionSnapshot, block: &Block) -> String {
        let Some(path) = block
            .input_str("file_path")
            .or_else(|| block.input_str("path"))
        else {
            return Self::describe_tool(block);
        };
        let file = base_name(path);
        bump(&mut snapshot.files_read, &file);
        bump(&mut snapshot.turn.files_read, &file);
        file
    }

    /// A short subject line for a tool that is not about a specific file.
    ///
    /// Tries the argument keys the common tools use, in the order that gives
    /// the most informative answer, and gives up quietly rather than printing
    /// a blob of JSON.
    fn describe_tool(block: &Block) -> String {
        for key in [
            "command",
            "pattern",
            "query",
            "prompt",
            "url",
            "description",
        ] {
            if let Some(value) = block.input_str(key) {
                let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
                if !single_line.is_empty() {
                    return truncate(&single_line, SUBJECT_MAX_CHARS);
                }
            }
        }
        String::new()
    }

    // ── compaction ────────────────────────────────────────────────────

    /// Records an automatic compaction.
    ///
    /// Two things are captured that nothing else can recover afterwards: the
    /// context fill immediately before, which is what the compaction threw
    /// away, and how many turns the finished segment lasted, which is the only
    /// empirical evidence for how long the next one will last.
    fn absorb_compaction(&mut self, record: &Record) {
        self.snapshot.compactions.push(CompactionEvent {
            turn: self.current_turn,
            context_before: self.last_context,
            context_after: 0,
            turns_in_segment: self.snapshot.turns_since_compaction,
            at: record.timestamp.unwrap_or_else(Utc::now),
        });
        self.snapshot.turns_since_compaction = 0;
        self.awaiting_rebuild = true;
        let index = self.snapshot.compactions.len();
        self.log(
            record.timestamp,
            LogLevel::Notice,
            format!("compaction #{index}"),
        );
    }

    fn log(&mut self, at: Option<DateTime<Utc>>, level: LogLevel, text: String) {
        push_capped(
            &mut self.snapshot.events,
            LogEntry {
                at: at.unwrap_or_else(Utc::now),
                level,
                text,
            },
            EVENT_LOG_CAPACITY,
        );
    }
}

// ── small helpers ─────────────────────────────────────────────────────

/// Appends to a ring of bounded length, dropping the oldest entry when full.
fn push_capped<T>(queue: &mut std::collections::VecDeque<T>, item: T, capacity: usize) {
    if queue.len() == capacity {
        queue.pop_front();
    }
    queue.push_back(item);
}

fn bump(counter: &mut BTreeMap<String, u32>, key: &str) {
    *counter.entry(key.to_owned()).or_insert(0) += 1;
}

/// The last path segment, which is what a narrow panel has room for.
fn base_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Lines in a block of text, counting a final line with no newline.
fn line_count(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    text.lines().count() as u64
}

/// Shortens `text` to `max_chars` characters, with an ellipsis if it was cut.
///
/// Counts characters rather than bytes: cutting a UTF-8 string at a byte
/// offset would panic on the first path containing a non-ASCII character.
fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().nth(max_chars).is_some() {
        out.push('\u{2026}');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(lines: &[&str]) -> SessionSnapshot {
        TranscriptParser::parse(Path::new("/tmp/s.jsonl"), "session-1", &lines.join("\n"))
    }

    const USER: &str = r#"{"type":"user","timestamp":"2026-08-23T10:00:00Z","message":{"content":"do the thing"}}"#;

    fn assistant(input: u64, cache_read: u64, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-08-23T10:00:01Z","message":{{"model":"claude-opus-5","content":[],"usage":{{"input_tokens":{input},"cache_read_input_tokens":{cache_read},"output_tokens":{output}}}}}}}"#
        )
    }

    #[test]
    fn a_half_written_final_line_does_not_lose_the_lines_before_it() {
        let snapshot = parse(&[USER, &assistant(10, 90, 5), r#"{"type":"assist"#]);
        assert_eq!(snapshot.turns, 1);
        assert_eq!(snapshot.responses, 1);
    }

    #[test]
    fn tool_results_do_not_inflate_the_turn_count() {
        let tool_result = r#"{"type":"user","timestamp":"2026-08-23T10:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#;
        let snapshot = parse(&[USER, &assistant(10, 90, 5), tool_result, tool_result]);
        assert_eq!(snapshot.turns, 1, "only the typed message is a turn");
    }

    #[test]
    fn sidechain_entries_are_left_out_of_the_main_threads_totals() {
        let sidechain = r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-08-23T10:00:03Z","message":{"model":"claude-opus-5","content":[],"usage":{"input_tokens":999999}}}"#;
        let snapshot = parse(&[USER, &assistant(10, 90, 5), sidechain]);
        assert_eq!(snapshot.totals.input, 10);
    }

    #[test]
    fn context_fill_follows_the_latest_response() {
        let snapshot = parse(&[USER, &assistant(10, 90, 5), &assistant(20, 380, 5)]);
        assert_eq!(snapshot.context_fill().used(), 400);
    }

    #[test]
    fn a_failed_tool_result_marks_the_call_that_made_it() {
        let call = r#"{"type":"assistant","timestamp":"2026-08-23T10:00:01Z","message":{"model":"claude-opus-5","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"false"}}]}}"#;
        let failure = r#"{"type":"user","timestamp":"2026-08-23T10:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"boom"}]}}"#;
        let snapshot = parse(&[USER, call, failure]);
        assert_eq!(snapshot.tool_errors, 1);
        assert_eq!(snapshot.last_error.as_deref(), Some("boom"));
        assert!(snapshot.recent_tools.back().expect("one call").failed);
    }

    #[test]
    fn a_compaction_records_what_it_threw_away_and_what_the_rebuild_cost() {
        let boundary =
            r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-08-23T10:00:05Z"}"#;
        let snapshot = parse(&[
            USER,
            &assistant(0, 900_000, 5),
            boundary,
            &assistant(0, 40_000, 5),
        ]);
        let event = snapshot.compactions.first().expect("one compaction");
        assert_eq!(event.context_before, 900_000);
        assert_eq!(event.context_after, 40_000);
        assert_eq!(event.turns_in_segment, 1);
        assert_eq!(snapshot.turns_since_compaction, 0);
    }

    #[test]
    fn a_new_turn_clears_the_live_feed_but_keeps_the_session_totals() {
        let call = r#"{"type":"assistant","timestamp":"2026-08-23T10:00:01Z","message":{"model":"claude-opus-5","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a/b/money.rs"}}]}}"#;
        let snapshot = parse(&[USER, call, USER]);
        assert!(snapshot.recent_tools.is_empty(), "feed is per turn");
        assert_eq!(snapshot.tool_counts.get("Read"), Some(&1), "totals persist");
        assert_eq!(snapshot.files_read.get("money.rs"), Some(&1));
    }

    #[test]
    fn distinct_subagents_are_counted_once_each() {
        let spawn = r#"{"type":"assistant","timestamp":"2026-08-23T10:00:01Z","message":{"model":"claude-opus-5","content":[{"type":"tool_use","id":"a1","name":"Task","input":{"description":"search the docs"}}]}}"#;
        let snapshot = parse(&[USER, spawn, spawn]);
        assert_eq!(snapshot.subagents, 1, "same id twice is one agent");
        assert_eq!(snapshot.turn.agents_running, 1, "no result came back");
    }

    #[test]
    fn a_subject_is_cut_on_a_character_boundary_not_a_byte_one() {
        let long = "\u{e4}".repeat(200);
        assert_eq!(
            truncate(&long, 5).chars().count(),
            6,
            "five chars plus the ellipsis"
        );
    }
}
