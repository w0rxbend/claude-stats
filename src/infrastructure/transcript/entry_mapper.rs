//! Turns transcript lines into [`Entry`] values.
//!
//! This is a Data Mapper in Fowler's sense: it sits between the wire shapes in
//! [`super::records`] and the domain's own vocabulary, and neither side knows
//! the other exists. A [`Record`] has no idea what a price sheet is; an
//! [`Entry`] has no idea that JSON, files or Claude Code's storage layout are
//! involved. Every rule about what a transcript *means* -- which lines are
//! billable, what to do about a missing model, who a sub-agent's tokens belong
//! to -- lives here, in one place, instead of being rediscovered by each
//! report that reads a transcript.
//!
//! The mapper is stateful on purpose, and the state is per file. A transcript
//! is not a set of independent lines: a line can inherit the model named
//! several lines earlier, and an anonymous line needs a name unique within the
//! file it came from. That makes one mapper per transcript, fed lines in the
//! order they were written, part of the contract rather than an implementation
//! detail.

use crate::domain::entry::{Entry, EntryId};
use crate::domain::model::ModelId;
use crate::domain::project::{Project, SessionId};

use super::records::Record;

/// Maps the lines of one transcript to billable entries.
///
/// Construct one per file, then feed it every line in order. Anything that is
/// not a charged assistant response is answered with `None`.
pub struct EntryMapper {
    /// Whose session a line belongs to when the line does not say.
    fallback_session: SessionId,
    /// Which project a line belongs to when the line does not say.
    fallback_project: Project,
    /// The last model actually named in this file.
    ///
    /// Starts empty, which prices at the catalogue's fallback rate -- the same
    /// thing the existing scanner does for a transcript whose very first
    /// response omits a model, so entries and the live dashboard cannot
    /// disagree about what such a response cost.
    last_model: ModelId,
    /// How many responses in this file have needed an invented name so far.
    anonymous_seen: u64,
}

impl EntryMapper {
    /// A mapper for one transcript.
    ///
    /// The two fallbacks are what the file itself is known to be about --
    /// normally the session id in its name and the working directory the
    /// catalogue resolved -- and they are used only for lines that do not
    /// carry their own.
    #[must_use]
    pub fn new(fallback_session: &str, fallback_project: &str) -> Self {
        Self {
            fallback_session: SessionId::new(fallback_session),
            fallback_project: Project::new(fallback_project),
            last_model: ModelId::new(""),
            anonymous_seen: 0,
        }
    }

    /// The billable response this line describes, if it describes one.
    ///
    /// Returns `None` for every other kind of line, which is most of them: a
    /// transcript is mostly user turns, tool results and bookkeeping, and none
    /// of those were charged for.
    pub fn map(&mut self, record: &Record) -> Option<Entry> {
        // Only an assistant response was sold. A user turn costs nothing on
        // its own -- it is charged for as part of the *next* response's input
        // counters -- so counting one would be counting the same tokens twice.
        if record.r#type != "assistant" {
            return None;
        }
        let message = record.message.as_ref()?;

        // The model is sticky across the file. Claude Code names it on the
        // first assistant response of a run and leaves it off the rest, so a
        // mapper that read only what each line says would price the whole
        // remainder of a session at the unknown-model fallback rate.
        //
        // This is now the only place that rule is written down. The account
        // scanner used to carry a second copy of it, which is precisely how
        // the live dashboard and a historical report come to quote different
        // figures for one session; it reads entries from here instead, so
        // there is nothing left to diverge from.
        if let Some(named) = stated(message.model.as_ref()) {
            let named = ModelId::new(named);
            // `<synthetic>` is Claude Code answering on its own account: a
            // refusal notice or an interrupt acknowledgement it wrote locally
            // without calling the API. Nothing was sold, so nothing is priced.
            // Its counters are all zero, but the string matches no catalogue
            // key, so an entry for it would be charged at the unknown-model
            // fallback rate as though it were a real paid model. It is also
            // deliberately *not* remembered as the sticky model: doing so
            // would make every following response that omits its model inherit
            // `<synthetic>` and vanish, taking real tokens with it.
            if named.is_synthetic() {
                return None;
            }
            self.last_model = named;
        }

        // No counters or no timestamp means nothing that can be priced or
        // placed in time. A refusal is the common case: the API charges
        // nothing for a request it rejected, and the line is already read
        // elsewhere as a rate-limit event.
        let (Some(usage), Some(at)) = (message.usage, record.timestamp) else {
            return None;
        };

        // A response with no `message.id` still spent real tokens, so it is
        // kept rather than dropped. What it cannot do is take part in
        // deduplication: with no id there are no grounds to call it the same
        // response as anything else, so it is given a name no other line in
        // this file will be given. The record's own uuid is preferred where
        // there is one, both because it is stable across two runs over the
        // same unchanged file and because it is unique across files -- the
        // invented name is only unique within one, so two transcripts of the
        // same session that both had to invent one would mint `anon-0` twice.
        // That is the weakest of the three and therefore the last resort.
        let message_id = stated(message.id.as_ref())
            .or_else(|| stated(record.uuid.as_ref()))
            .map_or_else(|| self.next_anonymous_id(), ToOwned::to_owned);

        // The line's own `sessionId` wins over the file it was found in. A
        // sub-agent writes its transcript under the session that spawned it
        // and records that session's id, which is how a helper's tokens end up
        // attributed to the run the user actually started rather than to a
        // session id nobody has ever seen.
        let session = stated(record.session_id.as_ref())
            .map_or_else(|| self.fallback_session.clone(), SessionId::new);

        // Likewise `cwd`: the recorded working directory is the only
        // trustworthy statement of which project this response belongs to,
        // because the directory name Claude Code files the transcript under
        // encodes the path lossily and cannot be decoded back.
        let project =
            stated(record.cwd.as_ref()).map_or_else(|| self.fallback_project.clone(), Project::new);

        Some(Entry {
            id: EntryId {
                message_id,
                // Also `""`-guarded: an empty request id and an absent one
                // hash differently, so one transcript writing the field blank
                // where another omits it would stop the same response
                // recognising itself.
                request_id: stated(record.request_id.as_ref()).map(ToOwned::to_owned),
                session: session.clone(),
            },
            at,
            model: self.last_model.clone(),
            tokens: usage.into(),
            // Always `None`, and not an oversight: no assistant entry in this
            // transcript format carries a `costUSD` or anything like it.
            // Claude Code records counters and leaves the arithmetic to
            // whoever reads them, so every figure this crate prints is derived
            // from the price sheet.
            recorded_cost: None,
            session,
            project,
            is_sidechain: record.is_sidechain,
        })
    }

    /// A name for a response that arrived without one, unique within this
    /// file.
    fn next_anonymous_id(&mut self) -> String {
        let n = self.anonymous_seen;
        self.anonymous_seen += 1;
        format!("anon-{n}")
    }
}

/// What a field says, when the transcript actually said something.
///
/// A field written as `""` is an absent value dressed up as a present one, and
/// nothing downstream can tell the difference once it has been wrapped. An
/// empty `sessionId` or `cwd` would defeat the fallbacks and file a response
/// under a nameless session or a blank project. An empty `message.id` is worse
/// still, because it is an *identity*: every response in a session that
/// carried one would share it, collapse into a single entry during
/// deduplication, and take the rest of their tokens with them -- the same
/// class of error as double-counting, only in the direction that hides money
/// rather than inventing it.
fn stated(field: Option<&String>) -> Option<&str> {
    field.map(String::as_str).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::ModelCatalog;

    fn mapper() -> EntryMapper {
        EntryMapper::new("session-in-the-file-name", "/home/ada/api")
    }

    fn line(json: &str) -> Record {
        serde_json::from_str(json).expect("a well-formed transcript line")
    }

    #[test]
    fn a_synthetic_refusal_produces_no_entry_rather_than_a_free_one() {
        let mut on_its_own = mapper();
        let entry = on_its_own.map(&line(
            r#"{"type":"assistant","timestamp":"2026-09-01T12:00:00Z",
                "message":{"id":"msg_01","model":"<synthetic>","content":[],
                           "usage":{"input_tokens":0,"output_tokens":0}}}"#,
        ));
        assert!(entry.is_none(), "nothing was sold, so nothing is billable");

        // And it must not have poisoned the sticky model: the next response,
        // which names no model of its own, would otherwise inherit
        // `<synthetic>` and disappear along with its real tokens.
        let mut across_a_synthetic = mapper();
        across_a_synthetic.map(&line(
            r#"{"type":"assistant","timestamp":"2026-09-01T12:00:00Z",
                "message":{"id":"msg_00","model":"claude-opus-5","content":[],
                           "usage":{"input_tokens":10,"output_tokens":1}}}"#,
        ));
        across_a_synthetic.map(&line(
            r#"{"type":"assistant","timestamp":"2026-09-01T12:01:00Z",
                "message":{"id":"msg_01","model":"<synthetic>","content":[],
                           "usage":{"input_tokens":0,"output_tokens":0}}}"#,
        ));
        let after = across_a_synthetic
            .map(&line(
                r#"{"type":"assistant","timestamp":"2026-09-01T12:02:00Z",
                    "message":{"id":"msg_02","content":[],
                               "usage":{"input_tokens":20,"output_tokens":2}}}"#,
            ))
            .expect("a real response after the synthetic one");
        assert_eq!(after.model, ModelId::new("claude-opus-5"));
    }

    #[test]
    fn a_response_that_omits_the_model_inherits_the_last_one_seen() {
        // Claude Code names the model on the first assistant response of a run
        // and leaves it off the rest. Reading each line in isolation would
        // price everything after the first at the unknown-model fallback of
        // $3/$15 instead of Opus 5's $5/$25.
        let mut mapper = mapper();
        let first = mapper
            .map(&line(
                r#"{"type":"assistant","timestamp":"2026-09-01T12:00:00Z",
                    "message":{"id":"msg_01","model":"claude-opus-5","content":[],
                               "usage":{"input_tokens":1000000,"output_tokens":0}}}"#,
            ))
            .expect("the first response");
        let later = mapper
            .map(&line(
                r#"{"type":"assistant","timestamp":"2026-09-01T12:05:00Z",
                    "message":{"id":"msg_02","content":[],
                               "usage":{"input_tokens":1000000,"output_tokens":0}}}"#,
            ))
            .expect("a response that names no model");

        assert_eq!(later.model, first.model);
        let cost = later
            .cost(ModelCatalog::pricing_for(later.model.as_str()))
            .dollars();
        assert!((cost - 5.0).abs() < 1e-9, "priced as Opus 5, got {cost}");
    }

    #[test]
    fn a_response_with_no_message_id_is_still_kept_rather_than_dropped() {
        let mut mapper = mapper();
        let with_uuid = mapper
            .map(&line(
                r#"{"type":"assistant","uuid":"row-7","timestamp":"2026-09-01T12:00:00Z",
                    "message":{"model":"claude-opus-5","content":[],
                               "usage":{"input_tokens":30,"output_tokens":3}}}"#,
            ))
            .expect("tokens were spent, so the entry is kept");
        assert_eq!(with_uuid.id.message_id, "row-7");
        assert_eq!(with_uuid.tokens.total(), 33);

        // With neither a message id nor a uuid there is nothing to name it
        // after, so it gets a name unique to this file. Unique is the point:
        // no identity means no grounds to merge it with anything.
        let anonymous: Vec<_> = (0..2)
            .map(|_| {
                mapper
                    .map(&line(
                        r#"{"type":"assistant","timestamp":"2026-09-01T12:01:00Z",
                            "message":{"model":"claude-opus-5","content":[],
                                       "usage":{"input_tokens":5,"output_tokens":0}}}"#,
                    ))
                    .expect("an anonymous response is still billable")
            })
            .collect();
        assert_eq!(anonymous[0].id.message_id, "anon-0");
        assert_eq!(anonymous[1].id.message_id, "anon-1");
        assert_ne!(anonymous[0].id, anonymous[1].id);
    }

    #[test]
    fn a_nested_transcript_entry_is_attributed_to_the_session_that_spawned_it() {
        // A sub-agent's transcript lives under the session that started it, so
        // the file's own name is the parent's id. When a line names a session
        // of its own that line wins, which is how a helper's tokens land on
        // the run the user actually started.
        let mut mapper = EntryMapper::new("parent-session", "/home/ada/api");
        let named = mapper
            .map(&line(
                r#"{"type":"assistant","sessionId":"parent-session","isSidechain":true,
                    "cwd":"/home/ada/api","timestamp":"2026-09-01T12:00:00Z",
                    "message":{"id":"msg_01","model":"claude-opus-5","content":[],
                               "usage":{"input_tokens":10,"output_tokens":1}}}"#,
            ))
            .expect("a sub-agent response");
        assert_eq!(named.session, SessionId::new("parent-session"));
        assert!(named.is_sidechain, "it was a helper, not the main thread");

        // A line that names no session at all falls back to the file's, which
        // is the same parent id -- so either way the tokens are billed to the
        // conversation the user started.
        let unnamed = mapper
            .map(&line(
                r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-09-01T12:01:00Z",
                    "message":{"id":"msg_02","model":"claude-opus-5","content":[],
                               "usage":{"input_tokens":10,"output_tokens":1}}}"#,
            ))
            .expect("a sub-agent response with no session id of its own");
        assert_eq!(unnamed.session, SessionId::new("parent-session"));
        assert_eq!(unnamed.id.session, SessionId::new("parent-session"));
    }

    #[test]
    fn the_recorded_working_directory_wins_over_the_file_it_was_found_in() {
        let mut mapper = mapper();
        let entry = mapper
            .map(&line(
                r#"{"type":"assistant","cwd":"/home/ada/Projects/.github",
                    "timestamp":"2026-09-01T12:00:00Z",
                    "message":{"id":"msg_01","model":"claude-opus-5","content":[],
                               "usage":{"input_tokens":10,"output_tokens":1}}}"#,
            ))
            .expect("a response");
        assert_eq!(entry.project, Project::new("/home/ada/Projects/.github"));
        assert_eq!(entry.project.display_name(), ".github");
    }

    #[test]
    fn a_user_turn_and_an_unpriced_refusal_are_not_billable_entries() {
        let mut mapper = mapper();
        assert!(
            mapper
                .map(&line(
                    r#"{"type":"user","timestamp":"2026-09-01T12:00:00Z",
                        "message":{"content":"do the thing"}}"#
                ))
                .is_none(),
            "a user turn is charged as the next response's input, not on its own"
        );
        assert!(
            mapper
                .map(&line(
                    r#"{"type":"assistant","timestamp":"2026-09-01T12:00:00Z",
                        "quotaLimits":{"status":"rejected","rateLimitType":"five_hour"}}"#
                ))
                .is_none(),
            "a refused request has no usage block and was not charged for"
        );
        assert!(
            mapper
                .map(&line(
                    r#"{"type":"assistant",
                        "message":{"id":"msg_01","model":"claude-opus-5","content":[],
                                   "usage":{"input_tokens":10,"output_tokens":1}}}"#
                ))
                .is_none(),
            "a response with no timestamp cannot be placed in any window"
        );
    }

    #[test]
    fn a_field_written_as_an_empty_string_counts_as_not_written_at_all() {
        // An empty `message.id` is the dangerous one: shared by every response
        // that carried one, it would give them all the same identity and
        // collapse a whole session's tokens into a single entry. The empty
        // `sessionId` and `cwd` are the same mistake in a milder form -- they
        // would file the response under a nameless session and a blank
        // project instead of the ones the file is known to be about.
        let mut mapper = mapper();
        let first = mapper
            .map(&line(
                r#"{"type":"assistant","sessionId":"","cwd":"",
                    "timestamp":"2026-09-01T12:00:00Z",
                    "message":{"id":"","model":"claude-opus-5","content":[],
                               "usage":{"input_tokens":10,"output_tokens":1}}}"#,
            ))
            .expect("real tokens were spent");
        let second = mapper
            .map(&line(
                r#"{"type":"assistant","sessionId":"","cwd":"",
                    "timestamp":"2026-09-01T12:01:00Z",
                    "message":{"id":"","model":"claude-opus-5","content":[],
                               "usage":{"input_tokens":20,"output_tokens":2}}}"#,
            ))
            .expect("real tokens were spent");

        assert_ne!(first.id, second.id, "two responses, two identities");
        assert_eq!(first.id.message_id, "anon-0");
        assert_eq!(second.id.message_id, "anon-1");
        assert_eq!(first.session, SessionId::new("session-in-the-file-name"));
        assert_eq!(first.project, Project::new("/home/ada/api"));
    }

    #[test]
    fn the_request_id_travels_with_the_entrys_identity() {
        let mut mapper = mapper();
        let entry = mapper
            .map(&line(
                r#"{"type":"assistant","requestId":"req_01XYZ",
                    "timestamp":"2026-09-01T12:00:00Z",
                    "message":{"id":"msg_01","model":"claude-opus-5","content":[],
                               "usage":{"input_tokens":10,"output_tokens":1}}}"#,
            ))
            .expect("a response");
        assert_eq!(entry.id.request_id.as_deref(), Some("req_01XYZ"));
    }
}
