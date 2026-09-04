//! Deserialising the JSON Claude Code pipes into the statusline hook on
//! stdin.
//!
//! A faithful transcription of the wire format and nothing more, in the same
//! spirit as [`crate::infrastructure::transcript::records`]: every field is
//! optional, unknown fields are ignored, and turning this into something the
//! application layer can act on is [`StatuslineHook::into_request`]'s job,
//! one step over. The format is Claude Code's own and undocumented, so a
//! newer release adding a field must not stop an older `claude-stats` from
//! printing a line.

use std::path::PathBuf;

use serde::Deserialize;

use crate::application::statusline::StatuslineRequest;
use crate::domain::context::ContextFill;
use crate::domain::model::ModelId;
use crate::domain::money::Usd;
use crate::domain::project::SessionId;

/// Printed, and returned as an error, when stdin carried nothing at all.
///
/// This command has no other way to fail this early -- there is no file to
/// be missing, no flag to be malformed -- so the one input it does have is
/// worth naming plainly when it turns out to be empty: a hook invoked by hand
/// rather than by Claude Code is the overwhelmingly likely cause, and the
/// message says so rather than reporting a bare JSON parse failure over
/// nothing.
pub const EMPTY_STDIN_MESSAGE: &str = "no input provided on stdin; claude-stats statusline is meant to be run as a Claude Code \
     statusline hook";

/// The hook payload Claude Code writes to stdin before every prompt redraw.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatuslineHook {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(default)]
    model: Option<HookModel>,
    #[serde(default)]
    cost: Option<HookCost>,
    #[serde(default)]
    context_window: Option<HookContextWindow>,
    #[serde(default)]
    effort: Option<HookEffort>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HookModel {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HookCost {
    #[serde(default)]
    total_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HookContextWindow {
    #[serde(default)]
    total_input_tokens: Option<u64>,
    #[serde(default)]
    context_window_size: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HookEffort {
    #[serde(default)]
    level: Option<String>,
}

impl StatuslineHook {
    /// Reads a hook payload from `input`, the raw text of stdin.
    ///
    /// # Errors
    ///
    /// Returns [`EMPTY_STDIN_MESSAGE`] for blank input -- see its own
    /// documentation -- and a `serde_json` error, naming what it could not
    /// parse, for anything present but malformed.
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(!input.trim().is_empty(), EMPTY_STDIN_MESSAGE);
        Ok(serde_json::from_str(input)?)
    }

    /// Translates the wire payload into the shape
    /// [`crate::application::statusline`] is written against.
    ///
    /// This is the whole of the boundary crossing: everywhere else in the
    /// crate, `serde` structures stay inside [`crate::infrastructure`], and
    /// this is where that rule is kept for the statusline hook. A
    /// [`ContextFill`] is only produced when the hook gave *both* halves of
    /// it -- a used figure with no window, or a window with no used figure,
    /// answers half a question, and the transcript fallback deserves the
    /// chance to answer the whole thing instead of being pre-empted by half
    /// an answer.
    #[must_use]
    pub fn into_request(self) -> StatuslineRequest {
        let hook_context = self.context_window.and_then(|window| {
            match (window.total_input_tokens, window.context_window_size) {
                (Some(used), Some(size)) => Some(ContextFill::new(used, size)),
                _ => None,
            }
        });
        StatuslineRequest {
            session_id: self.session_id.map(SessionId::new),
            transcript_path: self.transcript_path.map(PathBuf::from),
            model_id: self
                .model
                .as_ref()
                .and_then(|model| model.id.clone())
                .map(ModelId::new),
            model_display_name: self.model.and_then(|model| model.display_name),
            effort: self.effort.and_then(|effort| effort.level),
            hook_session_cost: self.cost.and_then(|cost| cost.total_cost_usd).map(Usd::new),
            hook_context,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stdin_explains_that_the_command_is_a_hook_rather_than_panicking() {
        for blank in ["", "   ", "\n\t"] {
            let error = StatuslineHook::parse(blank).expect_err("blank stdin is refused");
            assert_eq!(error.to_string(), EMPTY_STDIN_MESSAGE);
        }
    }

    #[test]
    fn a_full_payload_translates_every_field_into_the_request() {
        let hook = StatuslineHook::parse(
            r#"{ "session_id": "abc123", "transcript_path": "/tmp/t.jsonl",
                 "model": { "id": "claude-opus-5", "display_name": "Opus 5" },
                 "cost": { "total_cost_usd": 0.23 },
                 "context_window": { "total_input_tokens": 25000, "context_window_size": 200000 },
                 "effort": { "level": "high" } }"#,
        )
        .expect("well-formed JSON");
        let request = hook.into_request();

        assert_eq!(request.session_id, Some(SessionId::new("abc123")));
        assert_eq!(request.transcript_path, Some(PathBuf::from("/tmp/t.jsonl")));
        assert_eq!(request.model_id, Some(ModelId::new("claude-opus-5")));
        assert_eq!(request.model_display_name, Some("Opus 5".to_owned()));
        assert_eq!(request.effort, Some("high".to_owned()));
        assert_eq!(request.hook_session_cost, Some(Usd::new(0.23)));
        assert_eq!(
            request.hook_context,
            Some(ContextFill::new(25_000, 200_000))
        );
    }

    #[test]
    fn a_bare_object_translates_to_a_request_of_all_nothing() {
        let request = StatuslineHook::parse("{}")
            .expect("an empty object is still valid JSON")
            .into_request();

        assert_eq!(request.session_id, None);
        assert_eq!(request.model_display_name, None);
        assert_eq!(request.hook_session_cost, None);
        assert_eq!(request.hook_context, None);
    }

    #[test]
    fn an_unknown_field_is_ignored_rather_than_failing_the_whole_payload() {
        // A newer Claude Code adding a field to the hook must not break an
        // older claude-stats.
        let hook = StatuslineHook::parse(r#"{ "session_id": "abc", "aBrandNewField": 42 }"#)
            .expect("unknown fields must not fail the parse");
        assert_eq!(hook.into_request().session_id, Some(SessionId::new("abc")));
    }

    #[test]
    fn a_context_window_missing_either_half_is_left_for_the_transcript_fallback() {
        let used_only =
            StatuslineHook::parse(r#"{ "context_window": { "total_input_tokens": 10 } }"#)
                .expect("well-formed JSON")
                .into_request();
        assert_eq!(used_only.hook_context, None);

        let window_only =
            StatuslineHook::parse(r#"{ "context_window": { "context_window_size": 200000 } }"#)
                .expect("well-formed JSON")
                .into_request();
        assert_eq!(window_only.hook_context, None);
    }
}
