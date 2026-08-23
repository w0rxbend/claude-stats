//! Noticing that a transcript has changed.
//!
//! Two mechanisms, on purpose. The filesystem watcher gives near-instant
//! wake-ups so the dashboard feels live, and a metadata poll acts as a safety
//! net for the cases where inotify and friends quietly do not deliver: network
//! filesystems, containers with an exhausted watch limit, and the file being
//! replaced rather than appended to. Either one alone would be wrong some of
//! the time; together they are right all of the time, and the poll is cheap
//! because it is one `stat` per redraw interval.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::SystemTime;

use anyhow::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// A cheap identity for a file's contents: when it was last written, and how
/// long it is.
///
/// Modification time alone is not enough. Filesystems commonly hold only
/// second-granularity timestamps, so two appends within the same second are
/// indistinguishable -- and during an active session that is the normal case.
/// Pairing the timestamp with the length catches those.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    modified: Option<SystemTime>,
    length: u64,
}

impl FileFingerprint {
    fn of(path: &Path) -> Self {
        let metadata = std::fs::metadata(path).ok();
        Self {
            modified: metadata.as_ref().and_then(|m| m.modified().ok()),
            length: metadata.map_or(0, |m| m.len()),
        }
    }
}

/// Watches one transcript file for changes.
pub struct TranscriptWatcher {
    path: PathBuf,
    fingerprint: FileFingerprint,
    /// Kept alive for its side effect: dropping it stops the watch.
    _watcher: Option<RecommendedWatcher>,
    notifications: Option<Receiver<()>>,
}

impl TranscriptWatcher {
    /// Starts watching `path`.
    ///
    /// The watch is registered on the *parent directory* rather than the file.
    /// Editors and log rotators frequently write a new file and rename it over
    /// the old one, which silently invalidates a watch bound to the original
    /// inode; a directory watch survives that.
    ///
    /// If the platform watcher cannot be started at all, the watcher degrades
    /// to polling rather than failing. A slightly less responsive dashboard is
    /// a much better outcome than no dashboard.
    #[must_use]
    pub fn new(path: &Path) -> Self {
        let fingerprint = FileFingerprint::of(path);
        let (watcher, notifications) = Self::try_watch(path).unwrap_or((None, None));
        Self {
            path: path.to_path_buf(),
            fingerprint,
            _watcher: watcher,
            notifications,
        }
    }

    fn try_watch(path: &Path) -> Result<(Option<RecommendedWatcher>, Option<Receiver<()>>)> {
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            if result.is_ok() {
                // The event's details do not matter: any activity in the
                // directory is a reason to re-check the fingerprint, and the
                // fingerprint is what actually decides whether to re-read.
                let _ = tx.send(());
            }
        })?;
        let dir = path.parent().unwrap_or(Path::new("."));
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        Ok((Some(watcher), Some(rx)))
    }

    /// Whether the transcript has changed since the last call.
    ///
    /// Drains any pending filesystem notifications first, then compares the
    /// fingerprint. Returning `true` only on a fingerprint change means an
    /// unrelated file in the same directory cannot trigger a re-parse.
    pub fn has_changed(&mut self) -> bool {
        if let Some(rx) = &self.notifications {
            while rx.try_recv().is_ok() {}
        }
        let current = FileFingerprint::of(&self.path);
        if current == self.fingerprint {
            return false;
        }
        self.fingerprint = current;
        true
    }

    /// The file being watched.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl crate::application::ports::ChangeSource for TranscriptWatcher {
    fn has_changed(&mut self) -> bool {
        Self::has_changed(self)
    }
}

/// Creates real filesystem watchers.
#[derive(Debug, Default, Clone, Copy)]
pub struct FileSystemWatchFactory;

impl crate::application::ports::ChangeSourceFactory for FileSystemWatchFactory {
    fn watch(&self, path: &Path) -> Box<dyn crate::application::ports::ChangeSource> {
        Box::new(TranscriptWatcher::new(path))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn appending_to_the_file_is_detected_even_within_the_same_second() {
        let dir = std::env::temp_dir().join(format!("claudetui-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("session.jsonl");
        std::fs::write(&path, b"one\n").expect("write");

        let mut watcher = TranscriptWatcher::new(&path);
        assert!(!watcher.has_changed(), "nothing has happened yet");

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        file.write_all(b"two\n").expect("append");
        file.flush().expect("flush");

        // The length changed, so this holds regardless of timestamp
        // granularity -- which is the whole point of the paired fingerprint.
        assert!(watcher.has_changed());
        assert!(!watcher.has_changed(), "the change is reported once");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_does_not_exist_yet_simply_reports_no_change() {
        let mut watcher = TranscriptWatcher::new(Path::new("/nonexistent/session.jsonl"));
        assert!(!watcher.has_changed());
    }
}
