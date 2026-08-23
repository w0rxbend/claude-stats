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

use crate::domain::session::SessionSnapshot;

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

    /// Every known transcript, most recently written first.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be listed.
    fn list(&self) -> anyhow::Result<Vec<TranscriptRef>>;
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
