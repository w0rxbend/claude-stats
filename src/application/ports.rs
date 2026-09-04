//! The ports the dashboard talks to, expressed as traits.
//!
//! Nothing in [`crate::application`] or [`crate::domain`] names a file path
//! type it did not define, a JSON library, or a terminal. Those live behind
//! these traits and are wired in at the composition root
//! (`main.rs`). The immediate payoff is that the whole application layer is
//! testable against an in-memory fake; the longer-term one is that a future
//! source of session data -- a socket, a database, the sniffer proxy -- plugs
//! in without touching a line of the logic above it.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::domain::entry::Entry;
use crate::domain::limits::AccountUsage;
use crate::domain::pricing::PriceSheet;
use crate::domain::session::SessionSnapshot;
use crate::domain::tokens::TokenUsage;

/// A transcript file the dashboard could attach to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRef {
    /// Where the transcript lives.
    pub path: PathBuf,
    /// The session's UUID, taken from the file name.
    pub session_id: String,
    /// The working directory the session belongs to, decoded from the
    /// directory name Claude Code encodes it into.
    pub project_dir: String,
    /// When the file was last written. This is how "the active session" is
    /// decided, and how the follower notices there is new work to read.
    pub modified_at: DateTime<Utc>,
    /// Size in bytes, used to tell an appended-to file from a rewritten one.
    pub size_bytes: u64,
}

/// Which session the user asked for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionSelector {
    /// The most recently written transcript belonging to the current working
    /// directory, falling back to the most recent one anywhere.
    ///
    /// This is what `claude-stats monitor` does with no arguments, and it is the
    /// behaviour that makes the dashboard useful in a second terminal: open
    /// it next to a session and it attaches to that session.
    #[default]
    Active,
    /// The most recently written transcript belonging to a specific directory.
    Project(PathBuf),
    /// A session whose UUID starts with the given prefix.
    Id(String),
    /// One specific transcript file.
    Path(PathBuf),
}

/// Finds transcripts on whatever medium they are stored on.
pub trait TranscriptCatalog {
    /// Resolves a selector to a single transcript, or `None` if nothing
    /// matches.
    ///
    /// # Errors
    ///
    /// Returns an error only when the underlying store cannot be inspected at
    /// all. "Nothing matched" is `Ok(None)`, because a user who has not
    /// started a session yet has not done anything wrong.
    fn resolve(&self, selector: &SessionSelector) -> anyhow::Result<Option<TranscriptRef>>;

    /// Every session transcript, most recently written first.
    ///
    /// A *session* is a conversation a person started. Sub-agent and workflow
    /// transcripts are excluded, because this is what the session picker and
    /// the `sessions` listing are built from and a user looking for the run
    /// they started ten minutes ago should not have to find it among several
    /// thousand machine-spawned children.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be listed.
    fn list(&self) -> anyhow::Result<Vec<TranscriptRef>>;

    /// Every transcript that carries billable usage, sessions and their
    /// sub-agents alike, most recently written first.
    ///
    /// A sub-agent's tokens are charged to the account exactly like any
    /// others, so anything totalling *spend* must count them. On a machine
    /// that uses sub-agents and workflows heavily they are not a rounding
    /// error -- they routinely outnumber session transcripts thirty to one
    /// and account for the majority of the bill -- so a total that omits them
    /// is not merely approximate, it is wrong.
    ///
    /// Kept separate from [`Self::list`] rather than replacing it because the
    /// two questions genuinely differ: one asks "what did I work on", the
    /// other "what was I charged for".
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be listed.
    fn list_billable(&self) -> anyhow::Result<Vec<TranscriptRef>>;
}

/// Turns a transcript into a [`SessionSnapshot`].
pub trait SessionReader {
    /// Reads the whole transcript and derives a snapshot from it.
    ///
    /// # Errors
    ///
    /// Returns an error when the transcript cannot be read. Individual
    /// malformed lines are skipped rather than failing the whole read: a
    /// transcript is appended to live, so the last line is routinely a
    /// half-written one, and refusing to draw the dashboard because of it
    /// would make the tool useless exactly when it is most wanted.
    fn read(&self, transcript: &TranscriptRef) -> anyhow::Result<SessionSnapshot>;
}

/// Which projects a query is about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectFilter {
    /// Every project on the machine.
    #[default]
    All,
    /// Only these projects.
    ///
    /// A name matches either the whole recorded working directory
    /// (`/home/ada/Projects/api`) or its final segment (`api`), because those
    /// are the two spellings a person actually has to hand: the one their
    /// shell prompt shows them, and the one the reports print.
    Named(Vec<String>),
}

/// Which models a query is about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelFilter {
    /// Every model.
    #[default]
    All,
    /// Only models whose id contains one of these fragments.
    ///
    /// Substring rather than equality, and deliberately so: model ids carry a
    /// date suffix that nobody remembers, and the price catalogue already
    /// matches its rows the same way, so `opus` here selects exactly the rows
    /// `opus` would have been priced by.
    Named(Vec<String>),
}

/// Which sessions a query is about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionFilter {
    /// Every session.
    #[default]
    All,
    /// Only the session whose id starts with this prefix.
    ///
    /// A prefix rather than the whole id, matching [`SessionSelector::Id`], so
    /// that the eight characters the `sessions` listing prints are enough to
    /// name one.
    Id(String),
}

/// What a caller wants counted.
///
/// This is Fowler's Query Object: the criteria of a search, made into a value
/// that can be built, passed around, defaulted and compared, instead of eight
/// parameters threaded through every signature. The payoff is concrete here.
/// The command line, the dashboard and the tests all construct the *same*
/// value, so a report can only ever disagree with another report about what a
/// week cost if they were genuinely asked different questions; and the next
/// filter somebody needs is a field with a default rather than an edit to
/// every function that takes one.
///
/// [`Default`] is an unbounded query: no ends, no projects excluded, no models
/// excluded, no session singled out, and **sidechains included**. That last
/// default is the one worth stating out loud. A sub-agent's tokens are charged
/// to the account exactly like the main thread's, and on a machine that uses
/// sub-agents at all they are roughly four fifths of the spend -- so a report
/// that quietly left them out would not be slightly low, it would be wrong by
/// a factor of about five. Excluding them is therefore something a caller has
/// to ask for, and the only honest reason to ask is to answer "how much of
/// this did helpers spend", never "what did this cost".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageQuery {
    /// The earliest instant that counts, inclusive. `None` reaches back as far
    /// as the corpus goes.
    pub since: Option<DateTime<Utc>>,
    /// The latest instant that counts, inclusive. `None` runs to the end of
    /// the corpus.
    pub until: Option<DateTime<Utc>>,
    /// Which projects to count.
    pub projects: ProjectFilter,
    /// Which models to count.
    pub models: ModelFilter,
    /// Which sessions to count.
    pub sessions: SessionFilter,
    /// Whether sub-agent traffic counts. See the type's own documentation for
    /// why this defaults to `true` and why turning it off is nearly always the
    /// wrong thing to do.
    pub include_sidechains: bool,
}

impl Default for UsageQuery {
    fn default() -> Self {
        Self {
            since: None,
            until: None,
            projects: ProjectFilter::All,
            models: ModelFilter::All,
            sessions: SessionFilter::All,
            include_sidechains: true,
        }
    }
}

impl UsageQuery {
    /// Whether `entry` is one of the ones asked for.
    ///
    /// Both ends of the range are inclusive, which is what makes a query for
    /// "the first of the month to the last instant of it" mean what a reader
    /// expects. A `since` later than its `until` therefore selects nothing at
    /// all rather than everything: an empty range is empty, and answering an
    /// impossible question with the whole corpus is how a typo becomes a
    /// wrong number on a screen.
    #[must_use]
    pub fn matches(&self, entry: &Entry) -> bool {
        if self.since.is_some_and(|since| entry.at < since) {
            return false;
        }
        if self.until.is_some_and(|until| entry.at > until) {
            return false;
        }
        if !self.include_sidechains && entry.is_sidechain {
            return false;
        }
        if let ProjectFilter::Named(wanted) = &self.projects {
            let path = entry.project.as_str();
            let leaf = entry.project.display_name();
            if !wanted.iter().any(|name| name == path || name == leaf) {
                return false;
            }
        }
        if let ModelFilter::Named(wanted) = &self.models {
            let model = entry.model.as_str();
            if !wanted
                .iter()
                .any(|fragment| model.contains(fragment.as_str()))
            {
                return false;
            }
        }
        if let SessionFilter::Id(prefix) = &self.sessions {
            if !entry.session.as_str().starts_with(prefix.as_str()) {
                return false;
            }
        }
        true
    }
}

/// Answers "every billable response matching these criteria" over the whole
/// corpus.
///
/// A Repository in Fowler's sense, and a Separated Interface: the trait is
/// declared here, in the layer that needs it, and implemented outward in
/// [`crate::infrastructure`]. Everything above it -- the reports, the
/// dashboard, the use cases -- gets to think in terms of a collection of
/// [`Entry`] values it can query, and never learns that behind the collection
/// are several thousand JSON Lines files, a modification-time cache and a
/// deduplication pass.
///
/// One repository, one answer. The reason the crate has exactly one of these
/// rather than a scan per report is that two scans eventually disagree, and a
/// pair of totals that disagree about what a week cost is worse than either of
/// them being wrong on its own.
pub trait UsageRepository {
    /// Every entry matching `query`, oldest first and already deduplicated.
    ///
    /// Takes `&mut self` for the same reason [`AccountUsageReader::usage`]
    /// does: an implementation is expected to remember what it read, and a
    /// signature that hid that would be a lie about the cost of calling it
    /// twice.
    ///
    /// # Errors
    ///
    /// Returns an error only when the corpus cannot be enumerated at all --
    /// no projects directory, no permission to look. A single transcript that
    /// cannot be read, or that ends in a half-written line, is skipped and the
    /// rest are returned. That is not sloppiness: a transcript is appended to
    /// while a session runs, so the last line of the file somebody is working
    /// in right now is routinely a partial one, and refusing the whole report
    /// over it would break the tool at exactly the moment it is being used.
    fn entries(&mut self, query: &UsageQuery) -> anyhow::Result<Vec<Entry>>;
}

/// Supplies the price sheet a run is costed against.
///
/// A Separated Interface, declared here and implemented in
/// [`crate::infrastructure`], for a reason that is worth being explicit about
/// because the obvious one is wrong. It is *not* here so that prices can be
/// fetched from somewhere: nothing in this crate fetches anything, and nothing
/// is going to. It is here so that the composition root decides which sheet a
/// run uses -- compiled-in, user-corrected, or a fixed one a test hands over --
/// without a single line of the domain or the reports learning that the
/// question has more than one answer. That separation is the entire benefit,
/// and it costs one trait to have.
pub trait PriceSheetSource {
    /// The sheet to price this run with.
    ///
    /// Takes `&mut self` because an implementation is expected to compose the
    /// sheet once and remember it: reading a user's override file afresh for
    /// every report would be a lie about the cost of asking twice, and worse,
    /// two reports in one run could then be priced by two different sheets if
    /// the file changed between them.
    ///
    /// # Errors
    ///
    /// Never for want of a network, because nothing here reaches for one.
    /// Only when the user's own override file exists but cannot be read or
    /// cannot be parsed -- a file somebody wrote by hand and got wrong, which
    /// is refused by name rather than quietly ignored.
    fn sheet(&mut self) -> anyhow::Result<PriceSheet>;
}

/// Reads the token usage of the most recent assistant turn out of a
/// transcript, without loading the whole file into memory.
///
/// A Separated Interface for the one piece of disk access
/// [`crate::application::statusline`] needs that none of the existing ports
/// already gave it. [`UsageRepository::entries`] answers "every billable
/// response", which is the wrong shape for "the very last one" -- turning
/// that into an answer would mean holding the whole corpus in memory to read
/// off a single number the statusline hook usually supplies itself, on every
/// redraw of somebody's prompt.
pub trait TranscriptTailReader {
    /// The usage of the last `assistant` line in `path` that carries one, if
    /// any.
    ///
    /// # Errors
    ///
    /// Returns an error only when `path` cannot be opened or read at all. A
    /// line that fails to parse is skipped rather than treated as a failure,
    /// for the same reason every other transcript reader in this crate does
    /// that: the file is being appended to live, so its last line is
    /// routinely a half-written one.
    fn last_turn_usage(&self, path: &std::path::Path) -> anyhow::Result<Option<TokenUsage>>;
}

/// Adds up usage across every session on the account.
///
/// Separate from [`SessionReader`] because the question is a different shape:
/// a reader answers "what is in this transcript", while this answers "what
/// have I spent lately", which no single transcript can answer. Behind a trait
/// so the dashboard can be driven from a fake in a test rather than from a
/// directory of files.
pub trait AccountUsageReader {
    /// Usage as of `now`.
    ///
    /// Takes `&mut self` because an implementation is expected to cache: the
    /// honest signature for something that remembers what it read last time.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying store cannot be listed.
    fn usage(&mut self, now: DateTime<Utc>) -> anyhow::Result<AccountUsage>;
}

/// Tells the caller whether the thing it is watching has changed.
///
/// Behind a trait so the refresh logic can be driven deterministically in a
/// test instead of by writing files and hoping the platform notices.
pub trait ChangeSource {
    /// Whether a change has occurred since the previous call.
    ///
    /// Reports each change exactly once: a second call with nothing new in
    /// between returns `false`.
    fn has_changed(&mut self) -> bool;
}

/// Starts watching a path.
pub trait ChangeSourceFactory {
    /// Begins watching `path` for changes.
    fn watch(&self, path: &std::path::Path) -> Box<dyn ChangeSource>;
}

/// Reports the current time.
///
/// Behind a trait so that "how long has this turn been running" is testable
/// without sleeping.
pub trait Clock {
    /// The current instant, in UTC.
    fn now(&self) -> DateTime<Utc>;
}

/// The system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entry::EntryId;
    use crate::domain::model::ModelId;
    use crate::domain::project::{Project, SessionId};
    use crate::domain::tokens::TokenUsage;

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(minutes * 60, 0).expect("a valid timestamp")
    }

    /// One billable response, with everything a filter can look at spelled out
    /// so that a test can vary exactly one of them.
    fn entry(minutes: i64, session: &str, project: &str, model: &str, sidechain: bool) -> Entry {
        Entry {
            id: EntryId {
                message_id: format!("msg-{minutes}"),
                request_id: None,
                session: SessionId::new(session),
            },
            at: at(minutes),
            model: ModelId::new(model),
            tokens: TokenUsage {
                input: 100,
                ..TokenUsage::ZERO
            },
            recorded_cost: None,
            session: SessionId::new(session),
            project: Project::new(project),
            is_sidechain: sidechain,
        }
    }

    /// The plainest entry there is, for the tests that vary only the query.
    fn ordinary(minutes: i64) -> Entry {
        entry(
            minutes,
            "session-a",
            "/home/ada/Projects/api",
            "claude-opus-5",
            false,
        )
    }

    #[test]
    fn an_open_ended_query_filters_nothing_away() {
        let query = UsageQuery::default();

        assert!(query.matches(&ordinary(0)));
        assert!(query.matches(&ordinary(-60 * 24 * 365)), "years ago");
        assert!(query.matches(&ordinary(60 * 24 * 365)), "and years hence");
        assert!(
            query.matches(&entry(0, "s", "/anywhere", "some-unlisted-model", true)),
            "including a sub-agent's, whose tokens the account was charged for"
        );
    }

    #[test]
    fn a_range_includes_both_of_its_ends() {
        // Half-open would be defensible arithmetic and indefensible prose: a
        // caller asking for "the first of the month to the last instant of it"
        // means the last instant of it.
        let query = UsageQuery {
            since: Some(at(100)),
            until: Some(at(200)),
            ..UsageQuery::default()
        };

        assert!(query.matches(&ordinary(100)), "the lower end is inside");
        assert!(query.matches(&ordinary(150)));
        assert!(query.matches(&ordinary(200)), "and so is the upper one");
        assert!(!query.matches(&ordinary(99)));
        assert!(!query.matches(&ordinary(201)));
    }

    #[test]
    fn a_since_after_its_until_selects_nothing_rather_than_everything() {
        // An empty range is empty. Answering an impossible question with the
        // whole corpus is how a typo becomes a wrong number on a screen.
        let query = UsageQuery {
            since: Some(at(200)),
            until: Some(at(100)),
            ..UsageQuery::default()
        };

        for minutes in [0, 100, 150, 200, 300] {
            assert!(
                !query.matches(&ordinary(minutes)),
                "nothing falls inside a range that runs backwards"
            );
        }
    }

    #[test]
    fn a_project_is_named_either_by_its_whole_path_or_by_its_last_segment() {
        // Both spellings are ones a person has to hand: the one their shell
        // prompt shows them, and the one the reports print.
        let entry = ordinary(0);

        for name in ["/home/ada/Projects/api", "api"] {
            let query = UsageQuery {
                projects: ProjectFilter::Named(vec![name.to_owned()]),
                ..UsageQuery::default()
            };
            assert!(query.matches(&entry), "{name} names this project");
        }

        let elsewhere = UsageQuery {
            projects: ProjectFilter::Named(vec!["web".to_owned()]),
            ..UsageQuery::default()
        };
        assert!(!elsewhere.matches(&entry));
    }

    #[test]
    fn a_model_is_named_by_a_fragment_because_nobody_remembers_the_date_suffix() {
        let entry = entry(
            0,
            "session-a",
            "/home/ada/api",
            "claude-opus-5-20260901",
            false,
        );
        let query = UsageQuery {
            models: ModelFilter::Named(vec!["opus".to_owned()]),
            ..UsageQuery::default()
        };

        assert!(
            query.matches(&entry),
            "the same substring match the price catalogue uses"
        );

        let other = UsageQuery {
            models: ModelFilter::Named(vec!["haiku".to_owned()]),
            ..UsageQuery::default()
        };
        assert!(!other.matches(&entry));
    }

    #[test]
    fn a_session_is_named_by_the_prefix_the_listing_prints() {
        let entry = entry(
            0,
            "abc12345-dead-beef",
            "/home/ada/api",
            "claude-opus-5",
            false,
        );
        let query = UsageQuery {
            sessions: SessionFilter::Id("abc12345".to_owned()),
            ..UsageQuery::default()
        };

        assert!(
            query.matches(&entry),
            "eight characters are enough to name one"
        );

        let wrong = UsageQuery {
            sessions: SessionFilter::Id("def".to_owned()),
            ..UsageQuery::default()
        };
        assert!(!wrong.matches(&entry));
    }

    #[test]
    fn excluding_sidechains_is_something_a_caller_has_to_ask_for() {
        let helper = entry(0, "session-a", "/home/ada/api", "claude-opus-5", true);
        let main_thread = ordinary(0);

        assert!(
            UsageQuery::default().matches(&helper),
            "four fifths of the spend on a machine that uses sub-agents at all"
        );

        let without = UsageQuery {
            include_sidechains: false,
            ..UsageQuery::default()
        };
        assert!(!without.matches(&helper));
        assert!(without.matches(&main_thread));
    }

    #[test]
    fn every_criterion_has_to_agree_before_an_entry_counts() {
        // The filters narrow rather than widen: matching the project is not
        // enough if the model is wrong.
        let entry = ordinary(150);
        let query = UsageQuery {
            since: Some(at(100)),
            until: Some(at(200)),
            projects: ProjectFilter::Named(vec!["api".to_owned()]),
            models: ModelFilter::Named(vec!["haiku".to_owned()]),
            sessions: SessionFilter::Id("session-a".to_owned()),
            include_sidechains: true,
        };

        assert!(!query.matches(&entry), "the model alone rules it out");
    }
}
