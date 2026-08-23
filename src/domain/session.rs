//! The session snapshot: everything the dashboard knows about one Claude Code
//! conversation at one moment in time.
//!
//! A transcript is an append-only log. Reading it start to finish yields a
//! [`SessionSnapshot`], which is a *read model*: a plain aggregate of derived
//! facts with no behaviour of its own beyond computing metrics from the
//! numbers it already holds. Nothing outside this module mutates it once the
//! parser has handed it over, which is why every field is public -- there are
//! no invariants left to protect at that point.

use std::collections::{BTreeMap, VecDeque};

use chrono::{DateTime, Duration, Utc};

use super::activity::{ToolEvent, ToolKind};
use super::context::{CompactionDistance, ContextFill};
use super::model::ModelCatalog;
use super::money::Usd;
use super::tokens::TokenUsage;

/// How many recent tool calls the live feed keeps.
///
/// The feed is a window onto the present, not a history: older calls are
/// already summarised in the per-tool counters, so keeping them here would
/// only grow memory for a session that runs all day.
pub const RECENT_TOOL_CAPACITY: usize = 64;

/// How many log lines the scrollable event log keeps.
pub const EVENT_LOG_CAPACITY: usize = 512;

/// One assistant response, kept so the dashboard can draw trends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseSample {
    /// Which user turn this response belongs to.
    pub turn: u32,
    /// Prompt tokens in flight for this call -- i.e. the context fill at the
    /// moment it was made.
    pub prompt_tokens: u64,
    /// Tokens the model generated.
    pub output_tokens: u64,
    /// When the response was recorded.
    pub at: DateTime<Utc>,
}

/// One auto-compaction, and what it cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionEvent {
    /// The turn during which compaction fired.
    pub turn: u32,
    /// Context fill immediately before the compaction, in tokens.
    pub context_before: u64,
    /// Context fill immediately after the conversation was rebuilt.
    pub context_after: u64,
    /// How many turns the previous segment lasted.
    pub turns_in_segment: u32,
    /// When it happened.
    pub at: DateTime<Utc>,
}

/// The severity of a line in the event log, used only for colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// A normal tool call or message.
    Info,
    /// Something noteworthy: a compaction, a sub-agent finishing.
    Notice,
    /// A tool returned an error.
    Error,
}

/// One line of the scrollable event log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub at: DateTime<Utc>,
    pub level: LogLevel,
    pub text: String,
}

/// What the session is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// A user message is in and the assistant has not replied yet.
    Thinking,
    /// The assistant has replied and Claude Code is waiting on the human.
    Idle,
}

/// Counters that are reset every time the user sends a new message.
///
/// The dashboard shows both "this session" and "this turn" figures; keeping
/// the per-turn ones in their own struct means resetting them is a single
/// assignment rather than a list of fields somebody will forget to extend.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TurnCounters {
    pub tools: BTreeMap<String, u32>,
    pub tool_errors: u32,
    pub files_read: BTreeMap<String, u32>,
    pub files_edited: BTreeMap<String, u32>,
    pub thinking_blocks: u32,
    pub agents_spawned: u32,
    pub agents_running: u32,
    pub active_skill: Option<String>,
}

impl TurnCounters {
    /// Total tool calls made during this turn.
    #[must_use]
    pub fn tool_calls(&self) -> u32 {
        self.tools.values().sum()
    }
}

/// Everything known about one session, derived from its transcript.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    /// Absolute path of the transcript that produced this snapshot.
    pub transcript_path: std::path::PathBuf,
    /// The session's UUID, as taken from the transcript file name.
    pub session_id: String,
    /// The working directory the session was started in.
    pub project_dir: Option<String>,
    /// The git branch recorded on the transcript's entries, if any.
    pub git_branch: Option<String>,
    /// The model string as the API reported it, e.g. `"claude-opus-5"`.
    pub model_id: String,

    /// Timestamp of the first entry.
    pub started_at: Option<DateTime<Utc>>,
    /// Timestamp of the most recent entry.
    pub last_activity_at: Option<DateTime<Utc>>,
    /// Timestamp of the most recent user message.
    pub last_user_at: Option<DateTime<Utc>>,
    /// Whether the assistant is mid-reply.
    pub phase: SessionPhase,

    /// User messages that started a turn.
    pub turns: u32,
    /// Assistant messages recorded.
    pub responses: u32,

    /// Token usage summed over every response in the session.
    pub totals: TokenUsage,
    /// What the session has cost, accumulated response by response at the
    /// price of the model that answered each one.
    ///
    /// Kept rather than derived from [`Self::totals`] because a session can be
    /// switched between models mid-conversation, and pricing the whole total
    /// at any one model's rates would then be wrong for every other one.
    pub cost_accrued: Usd,
    /// Per-response history, oldest first.
    pub samples: Vec<ResponseSample>,

    /// Every auto-compaction seen, oldest first.
    pub compactions: Vec<CompactionEvent>,
    /// Turns completed since the last compaction (or since the start).
    pub turns_since_compaction: u32,

    /// Session-wide call count per tool name.
    pub tool_counts: BTreeMap<String, u32>,
    /// Session-wide call count per tool kind, for the activity mix chart.
    pub kind_counts: BTreeMap<ToolKind, u32>,
    /// The most recent tool calls, newest last, capped at
    /// [`RECENT_TOOL_CAPACITY`].
    pub recent_tools: VecDeque<ToolEvent>,

    /// How often each file was read, keyed by base name.
    pub files_read: BTreeMap<String, u32>,
    /// How often each file was written, keyed by base name.
    pub files_edited: BTreeMap<String, u32>,
    /// Lines added across every edit and write.
    pub lines_added: u64,
    /// Lines removed across every edit.
    pub lines_removed: u64,

    /// Assistant messages that contained a thinking block.
    pub thinking_blocks: u32,
    /// Distinct sub-agents spawned.
    pub subagents: u32,
    /// Skills loaded.
    pub skills: u32,

    /// Tool results that came back as errors.
    pub tool_errors: u32,
    /// The most recent error message, truncated for display.
    pub last_error: Option<String>,

    /// Counters for the turn currently in progress.
    pub turn: TurnCounters,
    /// The scrollable event log, newest last, capped at
    /// [`EVENT_LOG_CAPACITY`].
    pub events: VecDeque<LogEntry>,
}

impl SessionSnapshot {
    /// An empty snapshot for a transcript that has not been read yet.
    #[must_use]
    pub fn empty(transcript_path: std::path::PathBuf, session_id: String) -> Self {
        Self {
            transcript_path,
            session_id,
            project_dir: None,
            git_branch: None,
            model_id: String::new(),
            started_at: None,
            last_activity_at: None,
            last_user_at: None,
            phase: SessionPhase::Idle,
            turns: 0,
            responses: 0,
            totals: TokenUsage::ZERO,
            cost_accrued: Usd::ZERO,
            samples: Vec::new(),
            compactions: Vec::new(),
            turns_since_compaction: 0,
            tool_counts: BTreeMap::new(),
            kind_counts: BTreeMap::new(),
            recent_tools: VecDeque::new(),
            files_read: BTreeMap::new(),
            files_edited: BTreeMap::new(),
            lines_added: 0,
            lines_removed: 0,
            thinking_blocks: 0,
            subagents: 0,
            skills: 0,
            tool_errors: 0,
            last_error: None,
            turn: TurnCounters::default(),
            events: VecDeque::new(),
        }
    }

    /// The context window of the session's model, in tokens.
    #[must_use]
    pub fn context_window(&self) -> u64 {
        ModelCatalog::context_window_for(&self.model_id)
    }

    /// A short name for the model, for the header.
    #[must_use]
    pub fn model_display_name(&self) -> String {
        if self.model_id.is_empty() {
            "waiting".to_owned()
        } else {
            ModelCatalog::display_name_for(&self.model_id)
        }
    }

    /// How full the context window is, as of the latest response.
    ///
    /// Reads the prompt tokens of the last sample rather than summing the
    /// session: the context window holds the *current* conversation, and
    /// summing every call would count the same cached prefix once per turn.
    #[must_use]
    pub fn context_fill(&self) -> ContextFill {
        let used = self.samples.last().map_or(0, |s| s.prompt_tokens);
        ContextFill::new(used, self.context_window())
    }

    /// Mean growth in prompt tokens per turn since the last compaction.
    ///
    /// Only samples after the last compaction count: a compaction resets the
    /// conversation, so including pre-compaction growth would average across a
    /// discontinuity and predict the next compaction far too early.
    #[must_use]
    pub fn average_context_growth_per_turn(&self) -> f64 {
        let segment = self.current_segment();
        let (Some(first), Some(last)) = (segment.first(), segment.last()) else {
            return 0.0;
        };
        let turns_elapsed = last.turn.saturating_sub(first.turn);
        if turns_elapsed == 0 {
            return 0.0;
        }
        let grown = last.prompt_tokens.saturating_sub(first.prompt_tokens);
        grown as f64 / f64::from(turns_elapsed)
    }

    /// The responses recorded since the most recent compaction.
    #[must_use]
    pub fn current_segment(&self) -> &[ResponseSample] {
        let Some(last_compaction) = self.compactions.last() else {
            return &self.samples;
        };
        let start = self.samples.partition_point(|s| s.at <= last_compaction.at);
        &self.samples[start..]
    }

    /// How many turns are left before the next auto-compaction.
    #[must_use]
    pub fn compaction_distance(&self) -> CompactionDistance {
        CompactionDistance::estimate(self.context_fill(), self.average_context_growth_per_turn())
    }

    /// What the session has cost so far.
    #[must_use]
    pub const fn cost(&self) -> Usd {
        self.cost_accrued
    }

    /// Average cost of one user turn.
    #[must_use]
    pub fn cost_per_turn(&self) -> Usd {
        self.cost().per(self.turns)
    }

    /// Share of prompt tokens served from the cache, in `0.0..=1.0`.
    #[must_use]
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        self.totals.cache_hit_ratio()
    }

    /// Wall-clock time from the first entry to the most recent one.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        Some(self.last_activity_at? - self.started_at?)
    }

    /// Spend rate in dollars per hour, over the session's wall-clock time.
    ///
    /// Returns `None` for a session shorter than a minute, where dividing by a
    /// tiny elapsed time would produce a spectacular and meaningless number.
    #[must_use]
    pub fn burn_rate_per_hour(&self) -> Option<Usd> {
        let elapsed = self.duration()?;
        let minutes = elapsed.num_seconds() as f64 / 60.0;
        if minutes < 1.0 {
            return None;
        }
        Some(Usd::new(self.cost().dollars() * 60.0 / minutes))
    }

    /// Total tool calls made in the session.
    #[must_use]
    pub fn tool_calls(&self) -> u32 {
        self.tool_counts.values().sum()
    }

    /// Distinct files touched, whether read or written.
    #[must_use]
    pub fn files_touched(&self) -> usize {
        self.files_read
            .keys()
            .chain(self.files_edited.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    /// Output tokens per response, oldest first, for the sparkline.
    #[must_use]
    pub fn output_series(&self) -> Vec<u64> {
        self.samples.iter().map(|s| s.output_tokens).collect()
    }

    /// Indices into [`Self::output_series`] where a compaction occurred.
    ///
    /// The sparkline marks these so a sudden drop in context reads as "the
    /// conversation was compacted" rather than "something went wrong".
    #[must_use]
    pub fn compaction_marker_indices(&self) -> Vec<usize> {
        self.compactions
            .iter()
            .map(|c| self.samples.partition_point(|s| s.at <= c.at))
            .collect()
    }

    /// Tokens spent rebuilding context after compactions.
    ///
    /// Every compaction throws away a conversation that was paid for and pays
    /// again for a summary of it. This adds up the summaries, which is the
    /// part of the bill that bought nothing new.
    #[must_use]
    pub fn tokens_wasted_on_compaction(&self) -> u64 {
        self.compactions.iter().map(|c| c.context_after).sum()
    }

    /// The share of tokens that did useful work, in `0.0..=1.0`.
    ///
    /// One minus the compaction rebuild cost over everything sent. A session
    /// that never compacts scores 1.0.
    #[must_use]
    pub fn efficiency(&self) -> Option<f64> {
        let sent = self.totals.prompt_tokens();
        if sent == 0 {
            return None;
        }
        let wasted = self.tokens_wasted_on_compaction().min(sent);
        Some(1.0 - (wasted as f64 / sent as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("valid timestamp")
    }

    fn snapshot_with(samples: Vec<ResponseSample>) -> SessionSnapshot {
        let mut s = SessionSnapshot::empty("/tmp/t.jsonl".into(), "abc".to_owned());
        s.model_id = "claude-opus-5".to_owned();
        s.samples = samples;
        s
    }

    fn sample(turn: u32, prompt: u64, seconds: i64) -> ResponseSample {
        ResponseSample {
            turn,
            prompt_tokens: prompt,
            output_tokens: 100,
            at: at(seconds),
        }
    }

    #[test]
    fn context_fill_reads_the_latest_call_not_the_session_total() {
        let s = snapshot_with(vec![sample(1, 10_000, 1), sample(2, 25_000, 2)]);
        assert_eq!(s.context_fill().used(), 25_000);
    }

    #[test]
    fn growth_is_measured_only_within_the_segment_after_the_last_compaction() {
        let mut s = snapshot_with(vec![
            sample(1, 10_000, 1),
            sample(2, 900_000, 2),
            sample(3, 40_000, 4),
            sample(4, 60_000, 5),
        ]);
        s.compactions.push(CompactionEvent {
            turn: 3,
            context_before: 900_000,
            context_after: 40_000,
            turns_in_segment: 2,
            at: at(3),
        });
        // Only turns 3 and 4 count: 20k over one turn, not the 890k jump that
        // the compaction erased.
        assert!((s.average_context_growth_per_turn() - 20_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_single_sample_gives_no_growth_rate_rather_than_dividing_by_zero() {
        let s = snapshot_with(vec![sample(1, 10_000, 1)]);
        assert!((s.average_context_growth_per_turn() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_session_that_never_compacted_is_fully_efficient() {
        let mut s = snapshot_with(vec![sample(1, 10_000, 1)]);
        s.totals.input = 10_000;
        assert_eq!(s.efficiency(), Some(1.0));
    }

    #[test]
    fn a_session_under_a_minute_reports_no_burn_rate() {
        let mut s = snapshot_with(vec![sample(1, 10_000, 1)]);
        s.started_at = Some(at(0));
        s.last_activity_at = Some(at(30));
        assert_eq!(s.burn_rate_per_hour(), None);
    }
}
