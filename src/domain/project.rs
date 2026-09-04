//! Who a billable response belongs to: which working directory it was made
//! from, and which conversation it was part of.
//!
//! Both are strings, and both are wrapped rather than passed around bare. A
//! session id and a project path are never interchangeable, they are never
//! compared to one another, and a report that groups by one when it meant the
//! other would produce a plausible-looking table that is silently wrong. Two
//! distinct types cost a line of code each and make that mistake a compile
//! error, which is exactly the trade a Value Object is for.

use std::fmt;

/// The working directory a session was run from, as the transcript recorded
/// it.
///
/// This is always the absolute path the transcript itself wrote into its
/// `cwd` field, and it is never reconstructed from the directory name Claude
/// Code stores the transcript under. That encoding is lossy: it replaces both
/// `/` and `.` with `-`, so `~/.claude/projects/-home-ada-Projects--github`
/// could have come from `/home/ada/Projects/.github` or from
/// `/home/ada/Projects/-github`, and nothing in the name says which. Once two
/// different paths can produce the same name, the mapping has no inverse, and
/// a decoder that guesses is a decoder that is sometimes wrong. The recorded
/// `cwd` has no such ambiguity, so it is the only source used.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Project(String);

impl Project {
    /// Wraps a recorded working directory.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// The path itself, for grouping and for a table wide enough to show it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment of the path, for tables too narrow for the whole
    /// thing.
    ///
    /// A terminal is eighty columns on a bad day and `/home/ada/Projects/api`
    /// is mostly prefix shared with every other row, so the segment that
    /// actually distinguishes one project from another is the one worth the
    /// space. Trailing separators are trimmed first so that a path recorded
    /// as `/home/ada/api/` still shortens to `api` rather than to nothing.
    #[must_use]
    pub fn display_name(&self) -> &str {
        let trimmed = self.0.trim_end_matches('/');
        match trimmed.rsplit_once('/') {
            // A path that is nothing but separators, such as `/`, leaves an
            // empty tail; showing the raw path beats showing blank.
            Some((_, tail)) if !tail.is_empty() => tail,
            _ if trimmed.is_empty() => &self.0,
            _ => trimmed,
        }
    }
}

impl fmt::Display for Project {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The identifier of one Claude Code conversation.
///
/// Claude Code names a transcript after this id, and a sub-agent's transcript
/// lives in a directory named after the session that spawned it -- so the same
/// id is what ties a run's own responses to the responses of every helper it
/// started. Anything that counts "how many sessions" counts distinct values of
/// this type, never files, because one session routinely writes many.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    /// Wraps a recorded session id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id itself.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_the_recorded_working_directory_not_the_encoded_directory_name() {
        // Claude Code would store this session under the directory name
        // `-home-ada-Projects--github`, having replaced every `/` and every
        // `.` with a `-`. Reading that name back cannot tell you whether the
        // final segment was `.github` or `-github`, so the project is taken
        // from the `cwd` the transcript recorded and the encoded name is never
        // decoded at all.
        let project = Project::new("/home/ada/Projects/.github");
        assert_eq!(project.as_str(), "/home/ada/Projects/.github");
        assert_eq!(project.to_string(), "/home/ada/Projects/.github");
        assert_eq!(project.display_name(), ".github");
    }

    #[test]
    fn a_trailing_separator_does_not_shorten_a_project_to_nothing() {
        assert_eq!(Project::new("/home/ada/api/").display_name(), "api");
        assert_eq!(Project::new("/").display_name(), "/");
        assert_eq!(Project::new("api").display_name(), "api");
    }

    #[test]
    fn two_sessions_with_the_same_id_are_the_same_session() {
        assert_eq!(SessionId::new("abc"), SessionId::new("abc"));
        assert_ne!(SessionId::new("abc"), SessionId::new("abd"));
        assert_eq!(SessionId::new("abc").as_str(), "abc");
    }
}
