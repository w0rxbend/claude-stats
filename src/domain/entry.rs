//! One billable assistant response, as a value the reports can group,
//! deduplicate and attribute.
//!
//! Everything historical this crate reports -- spend per day, per project, per
//! model, per five-hour block -- is a fold over a stream of these. The type
//! deliberately carries more than a cost: a figure you cannot break down is a
//! figure you cannot check, and a total nobody can check is a total nobody
//! should trust.
//!
//! Cost is the one thing an [`Entry`] does *not* store. Prices change, the
//! catalogue is corrected, and a stored figure would then be a permanent
//! record of a rate that was never charged. [`Entry::cost`] recomputes from
//! the counters and the price sheet every time, so a correction to the
//! catalogue corrects every report at once.

use chrono::{DateTime, Utc};

use super::model::{ModelId, ModelPricing};
use super::money::Usd;
use super::project::{Project, SessionId};
use super::tokens::TokenUsage;

/// What makes two recorded responses the same response.
///
/// Claude Code does not write an assistant message exactly once. The same
/// response is copied into every transcript that replays the conversation it
/// belongs to: resume a session and the replayed history is written again,
/// fork it and both branches carry the shared prefix, run a sub-agent and its
/// log repeats what it was handed. On real data this is not a rare edge --
/// one 208-row transcript on this machine held only 130 distinct
/// message/request pairs, so a total that simply adds every row up overstates
/// the bill by roughly 60%.
///
/// The identity is a triple rather than the message id alone because the
/// message id is not guaranteed unique across conversations. A gateway or
/// proxy that recycles ids would otherwise let one session's response cancel
/// out another's, and those really were two responses that were really
/// charged for twice. Including the session means the only rows that collapse
/// are rows that describe the same response in the same conversation, which is
/// the only case where collapsing them is right.
///
/// The request id is part of the triple for the opposite reason: one message
/// id can span several API requests when a response is retried or continued,
/// and each request was billed. Keeping it distinguishes "the same response
/// written down twice" from "two charges that happen to share a message id".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryId {
    /// The API's own `msg_...` id, or a stand-in when the record carried
    /// none.
    pub message_id: String,
    /// The `requestId` the transcript recorded, when it recorded one.
    pub request_id: Option<String>,
    /// The conversation the response belongs to.
    pub session: SessionId,
}

/// One assistant response that was charged for.
///
/// This is the atom every historical report is built from. It is a value
/// object with no lifecycle of its own: the mapper that reads a transcript
/// produces them, aggregations fold over them, and nothing mutates one after
/// it exists.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// What makes this response distinct from every other one.
    pub id: EntryId,
    /// When the response was recorded, in UTC.
    pub at: DateTime<Utc>,
    /// The model that produced it, which decides what it cost.
    pub model: ModelId,
    /// The five token counters the API reported.
    pub tokens: TokenUsage,
    /// A cost the transcript stated outright, if it ever states one.
    ///
    /// Always `None` for entries read from a Claude Code transcript, which
    /// records counters and never a dollar figure. The field exists so that a
    /// source that *does* state a price -- an exported billing report, say --
    /// can be preferred over our own arithmetic rather than silently
    /// recomputed.
    pub recorded_cost: Option<Usd>,
    /// The conversation this response was part of.
    pub session: SessionId,
    /// The working directory that conversation was run from.
    pub project: Project,
    /// Whether a sub-agent produced this rather than the main thread.
    ///
    /// Sub-agent traffic is billed to the account exactly like any other, so
    /// this never excludes an entry from a spend total. It is kept so a report
    /// can say how much of the bill was spent by helpers the user never saw.
    pub is_sidechain: bool,
}

impl Entry {
    /// What this response cost at the given price sheet.
    ///
    /// Derived rather than stored -- see the module comment -- and delegated
    /// to [`TokenUsage::cost`] so that the five-rate arithmetic lives in
    /// exactly one place.
    #[must_use]
    pub fn cost(&self, pricing: ModelPricing) -> Usd {
        self.tokens.cost(pricing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::ModelCatalog;

    fn identity(message: &str, request: Option<&str>, session: &str) -> EntryId {
        EntryId {
            message_id: message.to_owned(),
            request_id: request.map(ToOwned::to_owned),
            session: SessionId::new(session),
        }
    }

    fn entry(tokens: TokenUsage) -> Entry {
        Entry {
            id: identity("msg_01", Some("req_01"), "session-a"),
            at: "2026-09-01T12:00:00Z".parse().expect("a valid timestamp"),
            model: ModelId::new("claude-opus-5"),
            tokens,
            recorded_cost: None,
            session: SessionId::new("session-a"),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    #[test]
    fn two_responses_with_the_same_message_and_request_id_have_the_same_identity() {
        // This is the duplicate the deduplication exists to remove: one
        // response written into two transcripts because the session was
        // resumed. Counting it twice is what inflates a real total by about
        // 60%.
        let written_once = identity("msg_01ABC", Some("req_01XYZ"), "session-a");
        let replayed = identity("msg_01ABC", Some("req_01XYZ"), "session-a");
        assert_eq!(written_once, replayed);
    }

    #[test]
    fn the_same_message_id_in_two_different_sessions_is_two_identities() {
        // Two genuinely different conversations that happen to share a message
        // id are two responses that were really sold, so collapsing them would
        // undercount rather than deduplicate.
        let here = identity("msg_01ABC", Some("req_01XYZ"), "session-a");
        let elsewhere = identity("msg_01ABC", Some("req_01XYZ"), "session-b");
        assert_ne!(here, elsewhere);
    }

    #[test]
    fn a_retry_under_the_same_message_id_is_a_second_charge() {
        // One message id can cover several API requests when a response is
        // retried, and each request was billed.
        let first = identity("msg_01ABC", Some("req_01XYZ"), "session-a");
        let retried = identity("msg_01ABC", Some("req_02XYZ"), "session-a");
        assert_ne!(first, retried);
    }

    #[test]
    fn an_entry_is_priced_from_its_counters_rather_than_from_a_stored_figure() {
        let response = entry(TokenUsage {
            input: 1_000_000,
            ..TokenUsage::ZERO
        });
        assert_eq!(response.recorded_cost, None);
        let cost = response
            .cost(ModelCatalog::pricing_for(response.model.as_str()))
            .dollars();
        assert!((cost - 5.0).abs() < 1e-9, "got {cost}");
    }
}
