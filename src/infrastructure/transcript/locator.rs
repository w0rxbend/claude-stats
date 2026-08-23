//! Finding transcripts under `~/.claude/projects`.
//!
//! Claude Code stores one directory per project and one JSON Lines file per
//! session inside it. The directory name is the project's absolute path with
//! every `/` replaced by `-`, e.g. `/home/ada/code/app` becomes
//! `-home-ada-code-app`. That encoding is lossy -- a directory legitimately
//! containing a dash is indistinguishable from a path separator -- so the
//! decoding here is only ever used for display, never to open anything.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::application::ports::{SessionSelector, TranscriptCatalog, TranscriptRef};

/// The catalogue backed by the real `~/.claude/projects` directory.
#[derive(Debug, Clone)]
pub struct FileSystemCatalog {
    projects_dir: PathBuf,
}

impl FileSystemCatalog {
    /// Points the catalogue at `~/.claude/projects`.
    ///
    /// # Errors
    ///
    /// Returns an error when the home directory cannot be determined, which on
    /// a normal machine means something is badly wrong with the environment.
    pub fn from_home() -> Result<Self> {
        let home = dirs::home_dir().context("cannot determine the home directory")?;
        Ok(Self::rooted_at(home.join(".claude").join("projects")))
    }

    /// Points the catalogue at an arbitrary directory. Used by the tests.
    #[must_use]
    pub fn rooted_at(projects_dir: PathBuf) -> Self {
        Self { projects_dir }
    }

    /// The directory Claude Code would store `project_dir`'s sessions in.
    #[must_use]
    pub fn encode_project_dir(project_dir: &Path) -> String {
        let text = project_dir.to_string_lossy().replace('/', "-");
        format!("-{}", text.trim_start_matches('-'))
    }

    /// Best-effort inverse of [`Self::encode_project_dir`], for display only.
    fn decode_project_dir(encoded: &str) -> String {
        format!("/{}", encoded.trim_start_matches('-').replace('-', "/"))
    }

    fn describe(path: &Path, project_dir: &str) -> Option<TranscriptRef> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified_at: DateTime<Utc> = metadata.modified().ok()?.into();
        Some(TranscriptRef {
            session_id: path.file_stem()?.to_string_lossy().into_owned(),
            path: path.to_path_buf(),
            project_dir: project_dir.to_owned(),
            modified_at,
            size_bytes: metadata.len(),
        })
    }

    /// Every transcript in one project directory.
    fn transcripts_in(dir: &Path) -> Vec<TranscriptRef> {
        let project_dir = dir
            .file_name()
            .map_or_else(String::new, |n| Self::decode_project_dir(&n.to_string_lossy()));
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "jsonl"))
            .filter_map(|p| Self::describe(&p, &project_dir))
            .collect()
    }
}

impl TranscriptCatalog for FileSystemCatalog {
    fn list(&self) -> Result<Vec<TranscriptRef>> {
        let Ok(entries) = std::fs::read_dir(&self.projects_dir) else {
            // No projects directory means Claude Code has never run here. That
            // is an empty list, not a failure.
            return Ok(Vec::new());
        };
        let mut all: Vec<TranscriptRef> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .flat_map(|dir| Self::transcripts_in(&dir))
            .collect();
        all.sort_by_key(|t| std::cmp::Reverse(t.modified_at));
        Ok(all)
    }

    fn resolve(&self, selector: &SessionSelector) -> Result<Option<TranscriptRef>> {
        match selector {
            SessionSelector::Path(path) => {
                Ok(Self::describe(path, &path.parent().map_or_else(
                    String::new,
                    |p| p.to_string_lossy().into_owned(),
                )))
            }
            SessionSelector::Id(prefix) => Ok(self
                .list()?
                .into_iter()
                .find(|t| t.session_id.starts_with(prefix))),
            SessionSelector::Project(dir) => Ok(self.newest_for_project(dir)),
            // "The active session" means the newest transcript for the
            // directory the dashboard was launched from -- that is what makes
            // it useful in a second terminal next to a running session. If
            // this directory has no sessions, fall back to the newest anywhere,
            // so the dashboard still shows something rather than an empty
            // screen the user cannot act on.
            SessionSelector::Active => {
                let cwd = std::env::current_dir().unwrap_or_default();
                if let Some(found) = self.newest_for_project(&cwd) {
                    return Ok(Some(found));
                }
                Ok(self.list()?.into_iter().next())
            }
        }
    }
}

impl FileSystemCatalog {
    fn newest_for_project(&self, project_dir: &Path) -> Option<TranscriptRef> {
        let dir = self.projects_dir.join(Self::encode_project_dir(project_dir));
        let mut found = Self::transcripts_in(&dir);
        found.sort_by_key(|t| std::cmp::Reverse(t.modified_at));
        found.into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_path_is_encoded_the_way_claude_code_encodes_it() {
        assert_eq!(
            FileSystemCatalog::encode_project_dir(Path::new("/home/ada/code/app")),
            "-home-ada-code-app"
        );
    }

    #[test]
    fn encoding_is_idempotent_for_a_path_that_already_starts_with_a_slash() {
        let once = FileSystemCatalog::encode_project_dir(Path::new("/a/b"));
        assert_eq!(once, "-a-b");
        assert_eq!(FileSystemCatalog::decode_project_dir(&once), "/a/b");
    }

    #[test]
    fn a_missing_projects_directory_lists_nothing_rather_than_failing() {
        let catalog = FileSystemCatalog::rooted_at(PathBuf::from("/nonexistent/claude/projects"));
        assert!(catalog.list().expect("must not fail").is_empty());
    }
}
