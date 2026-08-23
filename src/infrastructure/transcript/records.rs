//! Serde shapes for the lines of a Claude Code transcript.
//!
//! A transcript is JSON Lines: one self-describing object per line, appended
//! as the session runs. The format is not versioned and not documented, so
//! every field here is optional and unknown fields are ignored. That is a
//! deliberate choice, not laziness -- a transcript written by a newer Claude
//! Code must still parse, and the alternative is a dashboard that refuses to
//! start after an unrelated upgrade.
//!
//! These types are a faithful transcription of the wire format and nothing
//! more. Turning them into domain concepts is the parser's job, one module
//! over.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

/// One line of the transcript.
#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    /// The entry kind: `assistant`, `user`, `system`, `summary`, and a dozen
    /// bookkeeping kinds the dashboard ignores.
    #[serde(default)]
    pub r#type: String,

    /// Present on `system` entries; `compact_boundary` is the one that matters.
    #[serde(default)]
    pub subtype: Option<String>,

    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,

    /// The assistant or user message body.
    #[serde(default)]
    pub message: Option<Message>,

    /// Set on entries Claude Code injected rather than the user typing them.
    #[serde(default, rename = "isMeta")]
    pub is_meta: bool,

    /// Set on the synthetic user message that carries a compaction summary.
    #[serde(default, rename = "isCompactSummary")]
    pub is_compact_summary: bool,

    /// Set on entries belonging to a sub-agent rather than the main thread.
    #[serde(default, rename = "isSidechain")]
    pub is_sidechain: bool,

    #[serde(default)]
    pub cwd: Option<String>,

    #[serde(default, rename = "gitBranch")]
    pub git_branch: Option<String>,

    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
}

/// The `message` object of an `assistant` or `user` entry.
#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    /// The model that produced the message. Only assistant entries carry it.
    #[serde(default)]
    pub model: Option<String>,

    /// Either a bare string (a plain user message) or a list of typed blocks.
    #[serde(default)]
    pub content: Content,

    #[serde(default)]
    pub usage: Option<Usage>,
}

/// A message body, which the API allows in two shapes.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Content {
    /// A plain user message typed at the prompt.
    Text(String),
    /// A list of content blocks.
    Blocks(Vec<Block>),
}

impl Default for Content {
    fn default() -> Self {
        Self::Blocks(Vec::new())
    }
}

impl Content {
    /// The blocks of this body, or an empty slice for a plain-string body.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        match self {
            Self::Text(_) => &[],
            Self::Blocks(blocks) => blocks,
        }
    }

    /// Whether this body contains anything the user actually said.
    ///
    /// A `user` entry is not necessarily a human turn: tool results come back
    /// as user entries too. Only a non-empty string body, or a body with a
    /// `text` block in it, means a person typed something.
    #[must_use]
    pub fn has_user_text(&self) -> bool {
        match self {
            Self::Text(s) => !s.trim().is_empty(),
            Self::Blocks(blocks) => blocks
                .iter()
                .any(|b| b.r#type == "text" && b.text.as_deref().is_some_and(|t| !t.trim().is_empty())),
        }
    }
}

/// One content block inside a message.
///
/// The API's block types have disjoint field sets, so rather than an enum with
/// a variant per type -- which would have to be extended for every new block
/// type Anthropic ships -- this is one flat struct where each field is
/// populated only for the types that carry it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Block {
    #[serde(default)]
    pub r#type: String,

    /// The `tool_use` id, or the `tool_result` it answers.
    #[serde(default)]
    pub id: Option<String>,

    #[serde(default, rename = "tool_use_id")]
    pub tool_use_id: Option<String>,

    /// The tool's name, on `tool_use` blocks.
    #[serde(default)]
    pub name: Option<String>,

    /// The tool's arguments, on `tool_use` blocks. Left as raw JSON because
    /// every tool has a different schema and the parser only ever reaches for
    /// a handful of well-known keys.
    #[serde(default)]
    pub input: Option<Value>,

    /// Set on a `tool_result` block whose tool failed.
    #[serde(default, rename = "is_error")]
    pub is_error: bool,

    /// The text of a `text` block.
    #[serde(default)]
    pub text: Option<String>,

    /// The payload of a `tool_result` block, which the API allows to be
    /// either a bare string or a list of blocks. Kept as raw JSON and
    /// flattened by [`Block::result_text`].
    #[serde(default)]
    pub content: Option<Value>,
}

impl Block {
    /// Looks up a string argument of a `tool_use` block.
    #[must_use]
    pub fn input_str(&self, key: &str) -> Option<&str> {
        self.input.as_ref()?.get(key)?.as_str()
    }

    /// The human-readable text of a `tool_result` block.
    ///
    /// The API returns a result payload either as a bare string or as a list
    /// of blocks; both shapes turn up in real transcripts, so both are
    /// flattened here rather than at each call site.
    #[must_use]
    pub fn result_text(&self) -> Option<String> {
        if let Some(text) = &self.text {
            return Some(text.clone());
        }
        match self.content.as_ref()? {
            Value::String(s) => Some(s.clone()),
            Value::Array(items) => items
                .iter()
                .find_map(|item| item.get("text")?.as_str())
                .map(ToOwned::to_owned),
            _ => None,
        }
    }
}

/// The `usage` object of an assistant message.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

impl From<Usage> for crate::domain::tokens::TokenUsage {
    fn from(u: Usage) -> Self {
        Self {
            input: u.input_tokens,
            cache_read: u.cache_read_input_tokens,
            cache_creation: u.cache_creation_input_tokens,
            output: u.output_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_entry_kind_parses_instead_of_failing_the_whole_line() {
        let line = r#"{"type":"some-future-kind","brandNewField":42}"#;
        let record: Record = serde_json::from_str(line).expect("unknown kinds must parse");
        assert_eq!(record.r#type, "some-future-kind");
    }

    #[test]
    fn a_plain_string_body_and_a_block_body_both_parse() {
        let plain: Message = serde_json::from_str(r#"{"content":"hello"}"#).unwrap();
        assert!(plain.content.has_user_text());

        let blocks: Message =
            serde_json::from_str(r#"{"content":[{"type":"text","text":"hi"}]}"#).unwrap();
        assert!(blocks.content.has_user_text());
    }

    #[test]
    fn a_tool_result_body_is_not_counted_as_something_the_user_typed() {
        let msg: Message =
            serde_json::from_str(r#"{"content":[{"type":"tool_result","content":"ok"}]}"#).unwrap();
        assert!(!msg.content.has_user_text());
    }

    #[test]
    fn a_tool_result_payload_is_flattened_from_either_shape() {
        let bare: Block = serde_json::from_str(r#"{"type":"tool_result","content":"oops"}"#).unwrap();
        assert_eq!(bare.result_text().as_deref(), Some("oops"));

        let listed: Block =
            serde_json::from_str(r#"{"type":"tool_result","content":[{"type":"text","text":"oops"}]}"#)
                .unwrap();
        assert_eq!(listed.result_text().as_deref(), Some("oops"));
    }

    #[test]
    fn a_missing_usage_counter_defaults_to_zero_rather_than_failing() {
        let usage: Usage = serde_json::from_str(r#"{"input_tokens":5}"#).unwrap();
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 0);
    }
}
