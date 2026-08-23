//! Live tool activity: what Claude is doing right now, and what it just did.

use chrono::{DateTime, Utc};

/// The kind of thing a tool call did, for colouring and grouping.
///
/// The transcript only gives a tool *name* (`"Edit"`, `"Bash"`, a long
/// `"mcp__..."` string). Classifying once, here, keeps every widget from
/// re-deriving "is this a write?" out of a string comparison.
/// The ordering is the display order in the activity mix chart: the kinds a
/// reader most wants to see first come first, and `Ord` is what puts them in
/// that order when they are collected into a `BTreeMap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolKind {
    /// Read a file into context.
    Read,
    /// Changed a file on disk.
    Write,
    /// Searched the codebase.
    Search,
    /// Ran a shell command.
    Shell,
    /// Delegated work to a sub-agent.
    Agent,
    /// Loaded a skill.
    Skill,
    /// Reached out to the network.
    Network,
    /// Anything else, including MCP tools.
    Other,
}

impl ToolKind {
    /// Classifies a raw tool name from the transcript.
    #[must_use]
    pub fn classify(tool_name: &str) -> Self {
        match tool_name {
            "Read" | "NotebookRead" => Self::Read,
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Self::Write,
            "Grep" | "Glob" | "LS" => Self::Search,
            "Bash" | "BashOutput" | "KillShell" => Self::Shell,
            "Task" | "Agent" => Self::Agent,
            "Skill" => Self::Skill,
            "WebFetch" | "WebSearch" => Self::Network,
            other if other.starts_with("mcp__") => Self::Network,
            _ => Self::Other,
        }
    }

    /// A single glyph identifying the kind in dense layouts.
    ///
    /// Plain Unicode, not a Nerd Font: the dashboard has to look right in a
    /// terminal that has no patched font installed.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Read => "\u{25c8}",    // a filled lozenge
            Self::Write => "\u{270e}",   // a pencil
            Self::Search => "\u{2315}",  // a magnifier
            Self::Shell => "\u{25b6}",   // a play triangle
            Self::Agent => "\u{2726}",   // a four-pointed star
            Self::Skill => "\u{2698}",   // a flower
            Self::Network => "\u{2601}", // a cloud
            Self::Other => "\u{2022}",   // a bullet
        }
    }
}

/// One tool call, as it appears in the live activity feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEvent {
    /// When the assistant message containing the call was written.
    pub at: DateTime<Utc>,
    /// The raw tool name, e.g. `"Edit"`.
    pub name: String,
    /// What the call was classified as.
    pub kind: ToolKind,
    /// A short human-readable subject: a file name, the first word of a shell
    /// command, a search pattern, a sub-agent's description.
    pub subject: String,
    /// Whether the matching tool result came back as an error.
    ///
    /// Starts `false` and is flipped when the corresponding `tool_result`
    /// block is parsed, which is always a later line in the transcript.
    pub failed: bool,
    /// The `tool_use` id, used to pair the call with its result.
    pub id: String,
}

impl ToolEvent {
    /// A one-line label for the activity feed, e.g. `edit money.rs`.
    #[must_use]
    pub fn label(&self) -> String {
        if self.subject.is_empty() {
            self.name.to_lowercase()
        } else {
            format!("{} {}", self.name.to_lowercase(), self.subject)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_tools_are_classified_as_network_calls() {
        assert_eq!(ToolKind::classify("mcp__github__list_prs"), ToolKind::Network);
    }

    #[test]
    fn every_file_mutating_tool_lands_in_the_write_bucket() {
        for name in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
            assert_eq!(ToolKind::classify(name), ToolKind::Write, "{name}");
        }
    }

    #[test]
    fn an_unknown_tool_falls_back_to_other_rather_than_panicking() {
        assert_eq!(ToolKind::classify("SomeNewTool"), ToolKind::Other);
    }
}
