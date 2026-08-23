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
    /// How many lines to read looking for the recorded working directory.
    ///
    /// The first line of a session is often a bookkeeping entry with no `cwd`,
    /// so one line is not always enough; a handful always is, and it keeps
    /// listing 190 sessions to a few kilobytes of reads.
    const CWD_SCAN_LINES: usize = 8;

    /// Points the catalogue at wherever Claude Code keeps its projects.
    ///
    /// That is `~/.claude/projects`, unless `CLAUDE_CONFIG_DIR` is set --
    /// which relocates Claude Code's whole state directory, so a user who has
    /// set it would otherwise be told they have no sessions at all.
    ///
    /// # Errors
    ///
    /// Returns an error when `CLAUDE_CONFIG_DIR` is unset and the home
    /// directory cannot be determined, which on a normal machine means
    /// something is badly wrong with the environment.
    pub fn from_home() -> Result<Self> {
        let config_dir = match std::env::var_os("CLAUDE_CONFIG_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => dirs::home_dir()
                .context("cannot determine the home directory")?
                .join(".claude"),
        };
        Ok(Self::rooted_at(config_dir.join("projects")))
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
    ///
    /// Only used when the transcript itself does not say where it came from.
    /// The encoding cannot be inverted correctly -- a directory named
    /// `claude-stats` and a path `claude/stats` encode identically -- so
    /// [`Self::recorded_project_dir`] is tried first and this is the fallback.
    fn decode_project_dir(encoded: &str) -> String {
        format!("/{}", encoded.trim_start_matches('-').replace('-', "/"))
    }

    /// The working directory the transcript itself records.
    ///
    /// Every entry Claude Code writes carries a `cwd` field, so the first few
    /// lines are enough and there is no need to parse the whole file. This is
    /// authoritative where the directory-name encoding is only a guess.
    fn recorded_project_dir(path: &Path) -> Option<String> {
        use std::io::{BufRead, BufReader};

        let file = std::fs::File::open(path).ok()?;
        BufReader::new(file)
            .lines()
            .take(Self::CWD_SCAN_LINES)
            .map_while(Result::ok)
            .find_map(|line| {
                let record: super::records::Record = serde_json::from_str(&line).ok()?;
                record.cwd
            })
    }

    fn describe(path: &Path, fallback_project_dir: &str) -> Option<TranscriptRef> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified_at: DateTime<Utc> = metadata.modified().ok()?.into();
        Some(TranscriptRef {
            session_id: path.file_stem()?.to_string_lossy().into_owned(),
            project_dir: Self::recorded_project_dir(path)
                .unwrap_or_else(|| fallback_project_dir.to_owned()),
            path: path.to_path_buf(),
            modified_at,
            size_bytes: metadata.len(),
        })
    }

    /// Every transcript in one project directory.
    fn transcripts_in(dir: &Path) -> Vec<TranscriptRef> {
        let project_dir = dir.file_name().map_or_else(String::new, |n| {
            Self::decode_project_dir(&n.to_string_lossy())
        });
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
            SessionSelector::Path(path) => Ok(Self::describe(
                path,
                &path
                    .parent()
                    .map_or_else(String::new, |p| p.to_string_lossy().into_owned()),
            )),
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
        let dir = self
            .projects_dir
            .join(Self::encode_project_dir(project_dir));
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
    fn the_transcripts_own_cwd_beats_the_lossy_directory_name() {
        // "-home-ada-claude-stats" decodes to "/home/ada/claude/stats", which
        // is wrong for a directory actually named "claude-stats". The
        // transcript knows the truth.
        let dir = std::env::temp_dir().join(format!("claudetui-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("abc.jsonl");
        std::fs::write(
            &path,
            b"{\"type\":\"bridge-session\"}\n{\"type\":\"user\",\"cwd\":\"/home/ada/claude-stats\"}\n",
        )
        .expect("write");

        let described =
            FileSystemCatalog::describe(&path, "/home/ada/claude/stats").expect("described");
        assert_eq!(described.project_dir, "/home/ada/claude-stats");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_projects_directory_lists_nothing_rather_than_failing() {
        let catalog = FileSystemCatalog::rooted_at(PathBuf::from("/nonexistent/claude/projects"));
        assert!(catalog.list().expect("must not fail").is_empty());
    }
}
