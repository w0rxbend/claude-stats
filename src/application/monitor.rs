//! The monitor use case: attach to a session and keep its snapshot current.
//!
//! This is the piece that turns "there is a transcript somewhere" into "there
//! is an up-to-date [`SessionSnapshot`] on screen". It owns the two decisions
//! that are easy to get wrong and hard to notice: *when* to re-read the
//! transcript, and *when* to give up on the session it is attached to and go
//! looking for a newer one.
//!
//! It knows nothing about terminals. The dashboard calls [`Monitor::tick`] on
//! every frame and reads the result.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::ports::{
    ChangeSource, ChangeSourceFactory, SessionReader, SessionSelector, TranscriptCatalog,
    TranscriptRef,
};
use crate::domain::session::SessionSnapshot;

/// How often to go looking for a session while none is attached.
///
/// Long enough not to hammer the disk on an idle machine, short enough that
/// starting Claude Code in the next terminal feels like it connects at once.
const SEARCH_INTERVAL: Duration = Duration::from_secs(1);

/// How often to check whether a *different* session has become the active one.
///
/// Only relevant when following [`SessionSelector::Active`]. Re-scanning the
/// whole projects directory is the most expensive thing the monitor does, so
/// it happens on its own slow cadence rather than on every frame.
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);

/// What changed on this tick.
///
/// The dashboard uses this to decide whether to redraw and what to say in the
/// status line, so it is an enum rather than a bare `bool`: "we swapped to a
/// different session" and "the current session grew" call for different
/// treatment, and a boolean would flatten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Still no session to attach to.
    Searching,
    /// Attached to a session for the first time, or swapped to another one.
    Attached,
    /// The attached transcript grew and the snapshot was re-read.
    Refreshed,
    /// Nothing happened.
    Idle,
}

/// Keeps one session's snapshot in step with its transcript.
pub struct Monitor<C, R, W> {
    catalog: C,
    reader: R,
    watchers: W,
    selector: SessionSelector,

    attached: Option<TranscriptRef>,
    snapshot: Option<SessionSnapshot>,
    change_source: Option<Box<dyn ChangeSource>>,

    last_search: Option<Instant>,
    last_rescan: Option<Instant>,
    /// The most recent failure to read a transcript, kept so the dashboard can
    /// say what went wrong instead of silently showing stale numbers.
    last_error: Option<String>,
}

impl<C, R, W> Monitor<C, R, W>
where
    C: TranscriptCatalog,
    R: SessionReader,
    W: ChangeSourceFactory,
{
    /// Creates a monitor that will follow whatever `selector` resolves to.
    ///
    /// Nothing is read yet; the first [`Monitor::tick`] does the attaching, so
    /// that a dashboard can paint its "searching" splash immediately rather
    /// than blocking on the disk before the first frame.
    pub fn new(catalog: C, reader: R, watchers: W, selector: SessionSelector) -> Self {
        Self {
            catalog,
            reader,
            watchers,
            selector,
            attached: None,
            snapshot: None,
            change_source: None,
            last_search: None,
            last_rescan: None,
            last_error: None,
        }
    }

    /// The snapshot as of the last successful read, if there is one.
    #[must_use]
    pub fn snapshot(&self) -> Option<&SessionSnapshot> {
        self.snapshot.as_ref()
    }

    /// The transcript currently being followed.
    #[must_use]
    pub fn attached(&self) -> Option<&TranscriptRef> {
        self.attached.as_ref()
    }

    /// The most recent read failure, if any.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Advances the monitor by one frame.
    ///
    /// Safe and cheap to call at the display refresh rate: the expensive parts
    /// are rate-limited internally, and a tick with nothing to do costs one
    /// `stat`.
    pub fn tick(&mut self) -> Tick {
        if self.attached.is_none() {
            return self.search();
        }
        if self.should_rescan() && self.swap_to_newer_session() {
            return Tick::Attached;
        }
        if self
            .change_source
            .as_mut()
            .is_some_and(|source| source.has_changed())
        {
            self.reload();
            return Tick::Refreshed;
        }
        Tick::Idle
    }

    /// Points the monitor at a different session immediately.
    ///
    /// Used by the session picker. Unlike the automatic swap, this ignores the
    /// rescan interval, because a user who just chose a session should not
    /// wait five seconds to see it.
    ///
    /// # Errors
    ///
    /// Returns an error when the chosen transcript cannot be read.
    pub fn attach_to(&mut self, transcript: TranscriptRef) -> Result<()> {
        self.change_source = Some(self.watchers.watch(&transcript.path));
        self.attached = Some(transcript);
        self.last_rescan = Some(Instant::now());
        self.reload();
        match self.last_error.take() {
            Some(message) => Err(anyhow::anyhow!(message)),
            None => Ok(()),
        }
    }

    /// Looks for a session to attach to, no more often than every second.
    fn search(&mut self) -> Tick {
        if let Some(last) = self.last_search
            && last.elapsed() < SEARCH_INTERVAL
        {
            return Tick::Searching;
        }
        self.last_search = Some(Instant::now());

        match self.catalog.resolve(&self.selector) {
            Ok(Some(transcript)) => {
                self.change_source = Some(self.watchers.watch(&transcript.path));
                self.attached = Some(transcript);
                self.last_rescan = Some(Instant::now());
                self.reload();
                Tick::Attached
            }
            Ok(None) => Tick::Searching,
            Err(error) => {
                self.last_error = Some(error.to_string());
                Tick::Searching
            }
        }
    }

    fn should_rescan(&self) -> bool {
        // Only the "active session" selector is allowed to change its mind.
        // If the user named a session, following a different one behind their
        // back would be a bug, not a feature.
        if self.selector != SessionSelector::Active {
            return false;
        }
        self.last_rescan
            .is_none_or(|last| last.elapsed() >= RESCAN_INTERVAL)
    }

    /// Swaps to a newer session if one has appeared. Returns whether it did.
    fn swap_to_newer_session(&mut self) -> bool {
        self.last_rescan = Some(Instant::now());
        let Ok(Some(candidate)) = self.catalog.resolve(&self.selector) else {
            return false;
        };
        if self.attached.as_ref().is_some_and(|a| a.path == candidate.path) {
            return false;
        }
        self.change_source = Some(self.watchers.watch(&candidate.path));
        self.attached = Some(candidate);
        self.reload();
        true
    }

    /// Re-reads the attached transcript, keeping the previous snapshot on
    /// failure.
    ///
    /// A read can fail transiently -- the file is being rotated, a permission
    /// is briefly wrong -- and blanking the dashboard for a moment would be
    /// worse than showing numbers that are one second stale next to an error
    /// message saying so.
    fn reload(&mut self) {
        let Some(transcript) = &self.attached else {
            return;
        };
        match self.reader.read(transcript) {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.last_error = None;
            }
            Err(error) => self.last_error = Some(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use chrono::Utc;

    use super::*;

    /// A catalogue that hands back whatever the test put in it.
    struct FakeCatalog {
        transcripts: Vec<TranscriptRef>,
    }

    impl FakeCatalog {
        fn with(paths: &[&str]) -> Self {
            Self {
                transcripts: paths
                    .iter()
                    .map(|p| TranscriptRef {
                        path: PathBuf::from(p),
                        session_id: (*p).to_owned(),
                        project_dir: "/project".to_owned(),
                        modified_at: Utc::now(),
                        size_bytes: 0,
                    })
                    .collect(),
            }
        }
    }

    impl TranscriptCatalog for FakeCatalog {
        fn resolve(&self, _selector: &SessionSelector) -> Result<Option<TranscriptRef>> {
            Ok(self.transcripts.first().cloned())
        }

        fn list(&self) -> Result<Vec<TranscriptRef>> {
            Ok(self.transcripts.clone())
        }
    }

    /// A reader that counts how often it was asked to read.
    struct CountingReader {
        reads: Rc<Cell<u32>>,
    }

    impl SessionReader for CountingReader {
        fn read(&self, transcript: &TranscriptRef) -> Result<SessionSnapshot> {
            self.reads.set(self.reads.get() + 1);
            Ok(SessionSnapshot::empty(
                transcript.path.clone(),
                transcript.session_id.clone(),
            ))
        }
    }

    /// A change source the test drives by hand.
    struct ScriptedChanges {
        pending: Rc<Cell<u32>>,
    }

    impl ChangeSource for ScriptedChanges {
        fn has_changed(&mut self) -> bool {
            if self.pending.get() == 0 {
                return false;
            }
            self.pending.set(self.pending.get() - 1);
            true
        }
    }

    struct ScriptedFactory {
        pending: Rc<Cell<u32>>,
    }

    impl ChangeSourceFactory for ScriptedFactory {
        fn watch(&self, _path: &Path) -> Box<dyn ChangeSource> {
            Box::new(ScriptedChanges {
                pending: Rc::clone(&self.pending),
            })
        }
    }

    fn monitor(
        paths: &[&str],
        reads: &Rc<Cell<u32>>,
        pending: &Rc<Cell<u32>>,
    ) -> Monitor<FakeCatalog, CountingReader, ScriptedFactory> {
        Monitor::new(
            FakeCatalog::with(paths),
            CountingReader {
                reads: Rc::clone(reads),
            },
            ScriptedFactory {
                pending: Rc::clone(pending),
            },
            SessionSelector::Active,
        )
    }

    #[test]
    fn the_first_tick_attaches_and_reads_once() {
        let reads = Rc::new(Cell::new(0));
        let pending = Rc::new(Cell::new(0));
        let mut m = monitor(&["/a.jsonl"], &reads, &pending);

        assert_eq!(m.tick(), Tick::Attached);
        assert_eq!(reads.get(), 1);
        assert!(m.snapshot().is_some());
    }

    #[test]
    fn an_idle_tick_does_not_re_read_the_transcript() {
        let reads = Rc::new(Cell::new(0));
        let pending = Rc::new(Cell::new(0));
        let mut m = monitor(&["/a.jsonl"], &reads, &pending);
        m.tick();

        assert_eq!(m.tick(), Tick::Idle);
        assert_eq!(reads.get(), 1, "the file did not change");
    }

    #[test]
    fn a_reported_change_triggers_exactly_one_re_read() {
        let reads = Rc::new(Cell::new(0));
        let pending = Rc::new(Cell::new(0));
        let mut m = monitor(&["/a.jsonl"], &reads, &pending);
        m.tick();

        pending.set(1);
        assert_eq!(m.tick(), Tick::Refreshed);
        assert_eq!(reads.get(), 2);
        assert_eq!(m.tick(), Tick::Idle);
        assert_eq!(reads.get(), 2);
    }

    #[test]
    fn with_nothing_to_attach_to_the_monitor_keeps_searching() {
        let reads = Rc::new(Cell::new(0));
        let pending = Rc::new(Cell::new(0));
        let mut m = monitor(&[], &reads, &pending);

        assert_eq!(m.tick(), Tick::Searching);
        assert_eq!(reads.get(), 0);
        assert!(m.snapshot().is_none());
    }

    #[test]
    fn searching_is_rate_limited_so_an_idle_machine_is_not_hammered() {
        let reads = Rc::new(Cell::new(0));
        let pending = Rc::new(Cell::new(0));
        let mut m = monitor(&[], &reads, &pending);

        m.tick();
        let before = m.last_search;
        m.tick();
        assert_eq!(m.last_search, before, "the second tick was skipped");
    }

    #[test]
    fn a_named_session_is_never_swapped_out_from_under_the_user() {
        let reads = Rc::new(Cell::new(0));
        let pending = Rc::new(Cell::new(0));
        let mut m = Monitor::new(
            FakeCatalog::with(&["/a.jsonl"]),
            CountingReader {
                reads: Rc::clone(&reads),
            },
            ScriptedFactory {
                pending: Rc::clone(&pending),
            },
            SessionSelector::Id("chosen".to_owned()),
        );
        m.tick();
        assert!(!m.should_rescan());
    }
}
