//! Finding transcripts under `~/.claude/projects`.
//!
//! Claude Code stores one directory per project and one JSON Lines file per
//! session inside it. The directory name is the project's absolute path with
//! every `/` replaced by `-`, e.g. `/home/ada/code/app` becomes
//! `-home-ada-code-app`. That encoding is lossy -- a directory legitimately
//! containing a dash is indistinguishable from a path separator -- so the
//! decoding here is only ever used for display, never to open anything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::application::ports::{SessionSelector, TranscriptCatalog, TranscriptRef};

/// The catalogue backed by the real `~/.claude/projects` directory (or
/// directories -- see [`FileSystemCatalog::from_home`]).
#[derive(Debug, Clone)]
pub struct FileSystemCatalog {
    /// Every `projects/` directory this catalogue reads from, in the order
    /// they were discovered.
    ///
    /// More than one root is the normal case for anyone who has ever moved
    /// `CLAUDE_CONFIG_DIR` or set `XDG_CONFIG_HOME`: Claude Code keeps
    /// writing to wherever it last wrote, and a machine that has used both a
    /// standard and an XDG location has sessions genuinely split across the
    /// two. Treating that as one error condition to pick between would lose
    /// history; walking every root that exists does not.
    roots: Vec<PathBuf>,
    /// The one project directory to look in, when a report asked for one.
    ///
    /// See [`FileSystemCatalog::narrowed_to_project`] for why this is an
    /// optimisation rather than the filter itself.
    only: Option<String>,
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
    /// With `CLAUDE_CONFIG_DIR` set, that variable is authoritative and the
    /// defaults below are never consulted -- it may name one directory or
    /// several, comma-separated, and each entry may point either at a config
    /// directory or straight at its `projects/` subdirectory (the latter is
    /// normalised back to its parent, so both spellings work). Only entries
    /// that actually contain a `projects/` directory survive; if none do,
    /// that is a hard error rather than a silent fall-through to a directory
    /// the user did not ask for.
    ///
    /// Otherwise two locations are probed and *both* are kept when both
    /// exist: `$XDG_CONFIG_HOME/claude` (or `~/.config/claude` when that
    /// variable is unset) and `~/.claude`, in that order. Merging them rather
    /// than picking one is what stops a user who has moved between the two
    /// conventions from silently losing whichever half of their history is
    /// not in the one this tool happened to guess.
    ///
    /// # Errors
    ///
    /// Returns an error when `CLAUDE_CONFIG_DIR` is set but names no
    /// directory that actually holds a `projects/` folder, or when the home
    /// directory cannot be determined, which on a normal machine means
    /// something is badly wrong with the environment.
    pub fn from_home() -> Result<Self> {
        Ok(Self {
            roots: Self::discover_roots()?,
            only: None,
        })
    }

    /// The `projects/` directories [`Self::from_home`] should read from.
    fn discover_roots() -> Result<Vec<PathBuf>> {
        if let Some(value) = std::env::var_os("CLAUDE_CONFIG_DIR") {
            if !value.is_empty() {
                return Self::roots_from_env(&value.to_string_lossy());
            }
        }

        let home = dirs::home_dir().context("cannot determine the home directory")?;
        let xdg_base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| home.join(".config"));

        let mut found = Vec::new();
        for candidate in [xdg_base.join("claude"), home.join(".claude")] {
            if candidate.join("projects").is_dir() {
                found.push(candidate);
            }
        }
        // Neither location exists yet on a machine Claude Code has never run
        // on. `~/.claude` is still the answer in that case -- `list()`
        // already treats a missing `projects/` directory as "no sessions"
        // rather than an error, so defaulting to it costs nothing and keeps
        // this function infallible on a normal machine.
        if found.is_empty() {
            found.push(home.join(".claude"));
        }
        Ok(found.into_iter().map(|dir| dir.join("projects")).collect())
    }

    /// Parses a `CLAUDE_CONFIG_DIR` value into the `projects/` directories it
    /// names.
    ///
    /// # Errors
    ///
    /// Returns an error naming the variable when nothing in it survives --
    /// silently falling back to the default location would hide a typo
    /// behind what looks like an empty account rather than a broken setting.
    fn roots_from_env(value: &str) -> Result<Vec<PathBuf>> {
        let mut seen = HashSet::new();
        let roots: Vec<PathBuf> = value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(Self::normalize_claude_config_path)
            .filter(|dir| dir.join("projects").is_dir())
            .filter(|dir| seen.insert(dir.clone()))
            .map(|dir| dir.join("projects"))
            .collect();
        anyhow::ensure!(
            !roots.is_empty(),
            "No valid Claude data directories found in CLAUDE_CONFIG_DIR \
             ({value:?}). Each comma-separated entry must be a directory \
             that contains, or is itself, a `projects` directory."
        );
        Ok(roots)
    }

    /// Accepts either a config directory or its `projects` subdirectory.
    ///
    /// A user pointing `CLAUDE_CONFIG_DIR` at the exact place the transcripts
    /// live is a reasonable reading of the variable's name; normalising it
    /// back to the parent means both spellings land on the same catalogue
    /// instead of one of them quietly finding nothing.
    fn normalize_claude_config_path(entry: &str) -> PathBuf {
        let path = Self::expand_leading_tilde(entry);
        if path.file_name().is_some_and(|name| name == "projects") && path.is_dir() {
            path.parent().map_or(path.clone(), Path::to_path_buf)
        } else {
            path
        }
    }

    /// A minimal `~` expansion for the one place a user is likely to type it
    /// by hand -- the start of a `CLAUDE_CONFIG_DIR` entry. Nothing else in
    /// this tool reads a path from free text, so a fuller expansion (`~user`,
    /// `~` in the middle of a string) is not worth the extra surface.
    fn expand_leading_tilde(entry: &str) -> PathBuf {
        entry.strip_prefix("~/").map_or_else(
            || PathBuf::from(entry),
            |rest| dirs::home_dir().map_or_else(|| PathBuf::from(entry), |home| home.join(rest)),
        )
    }

    /// Points the catalogue at an arbitrary directory. Used by the tests.
    #[must_use]
    pub fn rooted_at(projects_dir: PathBuf) -> Self {
        Self {
            roots: vec![projects_dir],
            only: None,
        }
    }

    /// The same catalogue, reading only the directory `project` names, when
    /// that directory can be worked out safely and actually exists.
    ///
    /// This is an *optimisation and nothing else*. The authoritative project
    /// filter is [`crate::application::ports::ProjectFilter`], applied to every
    /// entry after it has been read, and it stays applied whether or not this
    /// narrowing takes effect. That separation is deliberate: the directory
    /// name Claude Code stores a project under is its path with every `/`
    /// turned into `-`, an encoding with no inverse, and a session that changed
    /// working directory records a `cwd` the directory name does not match. If
    /// pruning were the filter, either of those would silently produce an empty
    /// report where a correct one was available.
    ///
    /// So it narrows only when it is certain. A name with no separator in it --
    /// `api` -- is a final path segment, and there is no way to know which of
    /// several parents it belongs to, so nothing is pruned. A full path is
    /// encoded, checked for being a single harmless path segment (the encoding
    /// cannot produce a `/`, but a name that came out as `.` or `..` would walk
    /// out of the corpus), and used only if the directory is really there.
    ///
    /// The payoff is worth the care. Without it a `--project` report opens
    /// every transcript on the machine -- several thousand files on a corpus
    /// that uses sub-agents -- and throws away all but one directory's worth.
    #[must_use]
    pub fn narrowed_to_project(mut self, project: Option<&Path>) -> Self {
        let Some(project) = project else {
            return self;
        };
        if project.components().count() < 2 {
            return self;
        }
        let encoded = Self::encode_project_dir(project);
        if encoded.contains(std::path::MAIN_SEPARATOR) || encoded == "." || encoded == ".." {
            return self;
        }
        if self.roots.iter().any(|root| root.join(&encoded).is_dir()) {
            self.only = Some(encoded);
        }
        self
    }

    /// The project directories this catalogue is willing to walk into,
    /// across every root.
    fn project_dirs(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .flat_map(|root| {
                // A missing `projects/` directory under one root means
                // Claude Code never wrote there -- an empty contribution
                // from that root, not a failure of the whole catalogue.
                std::fs::read_dir(root).into_iter().flatten().flatten()
            })
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .filter(|path| {
                self.only.as_ref().is_none_or(|wanted| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy() == wanted.as_str())
                })
            })
            .collect()
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

    /// Every session transcript in one project directory.
    ///
    /// Only the files sitting directly in the project directory. Claude Code
    /// nests a session's sub-agent and workflow transcripts in a subdirectory
    /// named after it, and those are not sessions a person started.
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

    /// Every transcript beneath one project directory, at any depth.
    ///
    /// Sub-agent and workflow transcripts live in nested directories --
    /// `<session-id>/subagents/` and `<session-id>/subagents/workflows/<run>/`
    /// -- and each carries its own billable `message.usage`. The walk is
    /// depth-unbounded rather than hardcoded to those two shapes so that a
    /// future nesting Claude Code invents is counted without a code change:
    /// the cost of guessing wrong here is a silently understated bill.
    fn billable_transcripts_in(dir: &Path) -> Vec<TranscriptRef> {
        let project_dir = dir.file_name().map_or_else(String::new, |n| {
            Self::decode_project_dir(&n.to_string_lossy())
        });
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                // A directory directly under the project is named after the
                // session that owns everything inside it.
                let owner = path.file_name().map(|n| n.to_string_lossy().into_owned());
                Self::walk(&path, &project_dir, owner.as_deref(), &mut found);
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Some(described) = Self::describe(&path, &project_dir) {
                    found.push(described);
                }
            }
        }
        found
    }

    /// Collects every `.jsonl` under `dir`, recursing into subdirectories.
    ///
    /// Everything found is attributed to `owner`, the session that spawned it,
    /// rather than to itself. A sub-agent is not a session someone started: it
    /// is work done on behalf of one, and counting each of a session's several
    /// hundred sub-agents as a separate session would turn "you ran 3 sessions
    /// today" into "you ran 1,200".
    ///
    /// Iterative rather than recursive so that a pathologically deep tree
    /// cannot overflow the stack of a tool whose whole job is to keep running
    /// in the background.
    fn walk(dir: &Path, project_dir: &str, owner: Option<&str>, found: &mut Vec<TranscriptRef>) {
        let mut pending = vec![dir.to_path_buf()];
        while let Some(current) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for path in entries.flatten().map(|e| e.path()) {
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                    if let Some(mut described) = Self::describe(&path, project_dir) {
                        if let Some(owner) = owner {
                            owner.clone_into(&mut described.session_id);
                        }
                        found.push(described);
                    }
                }
            }
        }
    }
}

impl TranscriptCatalog for FileSystemCatalog {
    fn list(&self) -> Result<Vec<TranscriptRef>> {
        let mut all: Vec<TranscriptRef> = self
            .project_dirs()
            .iter()
            .flat_map(|dir| Self::transcripts_in(dir))
            .collect();
        all.sort_by_key(|t| std::cmp::Reverse(t.modified_at));
        Ok(all)
    }

    fn list_billable(&self) -> Result<Vec<TranscriptRef>> {
        let mut all: Vec<TranscriptRef> = self
            .project_dirs()
            .iter()
            .flat_map(|dir| Self::billable_transcripts_in(dir))
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
        let encoded = Self::encode_project_dir(project_dir);
        self.roots
            .iter()
            .flat_map(|root| Self::transcripts_in(&root.join(&encoded)))
            .max_by_key(|t| t.modified_at)
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
        let dir = std::env::temp_dir().join(format!("claude-stats-cwd-{}", std::process::id()));
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
    fn sub_agent_transcripts_are_billable_but_are_not_sessions() {
        // Claude Code nests a session's sub-agent and workflow transcripts
        // beneath it. They carry billable usage, so a spend total must see
        // them; they are not conversations anyone started, so the session
        // picker must not.
        let root = std::env::temp_dir().join(format!("claude-stats-nested-{}", std::process::id()));
        let project = root.join("-home-ada-app");
        let nested = project
            .join("session-1")
            .join("subagents")
            .join("workflows");
        std::fs::create_dir_all(&nested).expect("temp dirs");
        std::fs::write(project.join("session-1.jsonl"), b"{}\n").expect("write");
        std::fs::write(nested.join("agent-abc.jsonl"), b"{}\n").expect("write");

        let catalog = FileSystemCatalog::rooted_at(root.clone());
        let sessions = catalog.list().expect("list");
        let billable = catalog.list_billable().expect("list_billable");

        assert_eq!(sessions.len(), 1, "only the session a person started");
        assert_eq!(sessions[0].session_id, "session-1");
        assert_eq!(billable.len(), 2, "the sub-agent is charged for too");
        // Both are attributed to the session that owns them, so a window
        // reports one session rather than two.
        assert!(billable.iter().all(|t| t.session_id == "session-1"));
        assert!(billable.iter().any(|t| t.path.ends_with("agent-abc.jsonl")));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_projects_directory_lists_nothing_rather_than_failing() {
        let catalog = FileSystemCatalog::rooted_at(PathBuf::from("/nonexistent/claude/projects"));
        assert!(catalog.list().expect("must not fail").is_empty());
    }

    #[test]
    fn sessions_from_two_roots_are_merged_rather_than_one_root_winning() {
        // A user who has moved between a standard and an XDG location has
        // history genuinely split across two `projects/` directories. Both
        // must be walked, not just the first one found.
        let root_a = std::env::temp_dir().join(format!(
            "claude-stats-root-a-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let root_b = std::env::temp_dir().join(format!(
            "claude-stats-root-b-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let dir_a = root_a.join("-home-ada-api");
        let dir_b = root_b.join("-home-ada-web");
        std::fs::create_dir_all(&dir_a).expect("temp dir a");
        std::fs::create_dir_all(&dir_b).expect("temp dir b");
        std::fs::write(dir_a.join("api-1.jsonl"), b"{}\n").expect("write a");
        std::fs::write(dir_b.join("web-1.jsonl"), b"{}\n").expect("write b");

        let catalog = FileSystemCatalog {
            roots: vec![root_a.clone(), root_b.clone()],
            only: None,
        };
        let sessions = catalog.list().expect("list");

        assert_eq!(sessions.len(), 2, "both roots' sessions are present");
        assert!(sessions.iter().any(|t| t.session_id == "api-1"));
        assert!(sessions.iter().any(|t| t.session_id == "web-1"));

        std::fs::remove_dir_all(&root_a).ok();
        std::fs::remove_dir_all(&root_b).ok();
    }

    #[test]
    fn the_newest_session_for_a_project_is_found_whichever_root_it_lives_under() {
        let root_a = std::env::temp_dir().join(format!(
            "claude-stats-newest-a-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let root_b = std::env::temp_dir().join(format!(
            "claude-stats-newest-b-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        // Only the second root has this project at all.
        let dir_b = root_b.join("-home-ada-api");
        std::fs::create_dir_all(&dir_b).expect("temp dir b");
        std::fs::write(dir_b.join("api-1.jsonl"), b"{}\n").expect("write");

        let catalog = FileSystemCatalog {
            roots: vec![root_a.clone(), root_b.clone()],
            only: None,
        };
        let found = catalog
            .resolve(&SessionSelector::Project(PathBuf::from("/home/ada/api")))
            .expect("resolve")
            .expect("a session in the second root");
        assert_eq!(found.session_id, "api-1");

        std::fs::remove_dir_all(&root_a).ok();
        std::fs::remove_dir_all(&root_b).ok();
    }

    #[test]
    fn comma_separated_claude_config_dir_entries_are_all_kept() {
        let root_a = std::env::temp_dir().join(format!(
            "claude-stats-env-a-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let root_b = std::env::temp_dir().join(format!(
            "claude-stats-env-b-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root_a.join("projects")).expect("temp dir a");
        std::fs::create_dir_all(root_b.join("projects")).expect("temp dir b");

        let value = format!("{} , {}", root_a.display(), root_b.display());
        let roots = FileSystemCatalog::roots_from_env(&value).expect("both entries are valid");

        assert_eq!(
            roots,
            vec![root_a.join("projects"), root_b.join("projects")]
        );

        std::fs::remove_dir_all(&root_a).ok();
        std::fs::remove_dir_all(&root_b).ok();
    }

    #[test]
    fn an_entry_naming_no_projects_directory_is_dropped_rather_than_kept_empty() {
        let root = std::env::temp_dir().join(format!(
            "claude-stats-env-good-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("projects")).expect("temp dir");

        let value = format!("/nonexistent/definitely-not-here,{}", root.display());
        let roots = FileSystemCatalog::roots_from_env(&value).expect("the good entry survives");

        assert_eq!(roots, vec![root.join("projects")]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn claude_config_dir_naming_nothing_valid_is_a_hard_error() {
        let err = FileSystemCatalog::roots_from_env("/nonexistent/one,/nonexistent/two")
            .expect_err("neither entry has a projects directory");
        assert!(
            err.to_string().contains("CLAUDE_CONFIG_DIR"),
            "the message should name the variable that was misconfigured: {err}"
        );
    }

    #[test]
    fn duplicate_claude_config_dir_entries_are_not_read_twice() {
        let root = std::env::temp_dir().join(format!(
            "claude-stats-env-dup-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("projects")).expect("temp dir");

        let value = format!("{},{}", root.display(), root.display());
        let roots = FileSystemCatalog::roots_from_env(&value).expect("one entry, twice over");

        assert_eq!(roots, vec![root.join("projects")]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_claude_config_dir_entry_already_pointing_at_projects_is_normalised_to_its_parent() {
        // Pointing the variable at the `projects` directory itself is a
        // reasonable reading of what it is for; it must land on the same
        // catalogue as pointing it at the parent config directory.
        let root = std::env::temp_dir().join(format!(
            "claude-stats-env-normalise-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("projects")).expect("temp dir");

        let via_parent = FileSystemCatalog::normalize_claude_config_path(&root.to_string_lossy());
        let via_projects = FileSystemCatalog::normalize_claude_config_path(
            &root.join("projects").to_string_lossy(),
        );
        assert_eq!(via_parent, root);
        assert_eq!(via_projects, root, "the trailing `projects` is stripped");

        std::fs::remove_dir_all(&root).ok();
    }

    /// Two projects, one transcript each, under a directory of this test's own.
    fn two_projects(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "claude-stats-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        for (project, session) in [("-home-ada-api", "api-1"), ("-home-ada-web", "web-1")] {
            let dir = root.join(project);
            std::fs::create_dir_all(&dir).expect("temp dirs");
            std::fs::write(dir.join(format!("{session}.jsonl")), b"{}\n").expect("write");
        }
        root
    }

    #[test]
    fn a_full_project_path_narrows_the_walk_to_the_one_directory_it_names() {
        let root = two_projects("narrow");
        let narrowed = FileSystemCatalog::rooted_at(root.clone())
            .narrowed_to_project(Some(Path::new("/home/ada/web")));

        let billable = narrowed.list_billable().expect("list_billable");
        assert_eq!(billable.len(), 1, "only the named project was opened");
        assert_eq!(billable[0].session_id, "web-1");
        assert_eq!(narrowed.list().expect("list").len(), 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_bare_project_name_narrows_nothing_because_it_names_no_directory() {
        // `api` is a final path segment, and the directory a project is stored
        // under is its *whole* path with the separators rewritten. There is no
        // way to work out which parent `api` belongs to, so pruning on it would
        // have to guess -- and a wrong guess here is an empty report where a
        // correct one was available. The filter on the query still narrows the
        // result; only the walk stays wide.
        let root = two_projects("bare");
        let catalog =
            FileSystemCatalog::rooted_at(root.clone()).narrowed_to_project(Some(Path::new("api")));

        assert_eq!(
            catalog.list_billable().expect("list_billable").len(),
            2,
            "both projects are still walked"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_project_directory_that_is_not_there_leaves_the_walk_alone() {
        // A session that changed working directory records a `cwd` the
        // directory name does not match, so the encoded name can legitimately
        // be absent while the traffic is not. Narrowing onto a directory that
        // does not exist would turn that into a silently empty report; leaving
        // the walk wide lets the query's own project filter answer correctly.
        let root = two_projects("absent");
        let catalog = FileSystemCatalog::rooted_at(root.clone())
            .narrowed_to_project(Some(Path::new("/home/ada/moved-since")));

        assert_eq!(catalog.list_billable().expect("list_billable").len(), 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_project_name_cannot_walk_out_of_the_corpus() {
        // The encoding turns every separator into a dash and prefixes another,
        // so it cannot produce a name that escapes the projects directory. This
        // pins that, because the value being encoded arrives from the command
        // line and the encoded name is joined onto a path.
        for hostile in ["/../..", "/..", "/a/../../etc"] {
            let encoded = FileSystemCatalog::encode_project_dir(Path::new(hostile));
            assert!(
                !encoded.contains(std::path::MAIN_SEPARATOR),
                "{hostile:?} encoded to {encoded:?}, which is more than one segment"
            );
            assert!(encoded.starts_with('-'), "{encoded:?}");
        }
    }
}
