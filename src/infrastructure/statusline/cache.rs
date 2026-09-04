//! Caching one rendered statusline per session, so a prompt that redraws
//! several times a second does not re-scan the whole corpus on every one of
//! them.
//!
//! This is the only place in the crate that writes to disk outside the
//! transcript corpus itself, and it writes only under the cache directory --
//! never into `~/.claude`, which the rest of this tool treats as read-only
//! for ever. [`StatuslineCache`] is a Gateway in Fowler's sense over that one
//! file: a thin wrapper the rest of the module can read and write through
//! without knowing it is JSON on disk, and [`resolve`] is the policy that
//! decides when to trust what it holds.
//!
//! The failure mode this module exists to prevent is not "the numbers are
//! briefly stale" -- that is the whole trade the cache makes on purpose -- it
//! is "the prompt shows nothing, or shows a stack trace, because a corpus
//! scan tripped over a half-written transcript at exactly the wrong moment".
//! [`resolve`] makes stale numbers the worst case a render failure can
//! produce.

use std::fs;
use std::path::PathBuf;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Printed when there is nothing at all to fall back to: a fresh render
/// failed, and no earlier line was ever cached to reprint instead.
///
/// The only other thing this command ever writes to stdout, and deliberately
/// terse -- there is no room on a single prompt line for a stack trace, and a
/// user who sees this is meant to go looking in `claude-stats stats`, not to
/// read the statusline for detail it was never going to have.
pub const FALLBACK_LINE: &str = "🤖 claude-stats | error";

/// The rendered line, and the two facts that decide whether it is still good
/// enough to show again without re-rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CachedLine {
    line: String,
    rendered_at: DateTime<Utc>,
    /// The transcript's modification time as of the render that produced
    /// [`Self::line`], or `None` when the hook named no transcript at all.
    ///
    /// Compared for exact equality rather than "no older than", because the
    /// question this answers is "has anything happened since", and a
    /// transcript can only have grown or not -- there is no partial credit
    /// for a render that is merely close to current.
    transcript_modified_at: Option<DateTime<Utc>>,
}

/// The one cache file a single statusline session owns.
pub struct StatuslineCache {
    path: PathBuf,
}

impl StatuslineCache {
    /// The cache file for `session_id`, under the XDG cache directory.
    ///
    /// `${XDG_CACHE_HOME:-~/.cache}/claude-stats/statusline/<session id>.json`,
    /// matching the same environment-variable convention
    /// [`crate::infrastructure::pricing::overrides`] already uses for its own
    /// configuration directory.
    ///
    /// # Errors
    ///
    /// Returns an error only when neither `XDG_CACHE_HOME` nor a home
    /// directory can be found, which on a working machine means something is
    /// badly wrong with the environment. Callers are expected to treat that
    /// as "caching is unavailable this run" rather than as a reason to stop
    /// the statusline printing -- see [`resolve`].
    pub fn for_session(session_id: &str) -> Result<Self> {
        let base = match std::env::var_os("XDG_CACHE_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => dirs::home_dir()
                .context("cannot determine the cache directory")?
                .join(".cache"),
        };
        Ok(Self::at(
            base.join("claude-stats")
                .join("statusline")
                .join(format!("{session_id}.json")),
        ))
    }

    /// A cache backed by an arbitrary file. Used by the tests, and by
    /// anything that wants a cache outside the usual XDG location.
    #[must_use]
    pub const fn at(path: PathBuf) -> Self {
        Self { path }
    }

    fn read(&self) -> Option<CachedLine> {
        let text = fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// The cached line, if one exists, is no older than `max_age` as of
    /// `now`, and was rendered against the transcript state described by
    /// `transcript_modified_at`.
    ///
    /// A cached line whose `rendered_at` is *after* `now` is also refused --
    /// which only a clock running backward could produce -- rather than
    /// treated as fresh by an age comparison that would otherwise be
    /// negative and satisfy every bound by accident.
    #[must_use]
    pub fn fresh(
        &self,
        max_age: StdDuration,
        transcript_modified_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Option<String> {
        let cached = self.read()?;
        let age = now.signed_duration_since(cached.rendered_at);
        if age < chrono::Duration::zero() {
            return None;
        }
        match age.to_std() {
            Ok(age) if age <= max_age => {}
            _ => return None,
        }
        if cached.transcript_modified_at != transcript_modified_at {
            return None;
        }
        Some(cached.line)
    }

    /// The most recently cached line, whatever its age -- the fallback
    /// [`resolve`] reprints when a fresh render fails.
    #[must_use]
    pub fn stale(&self) -> Option<String> {
        self.read().map(|cached| cached.line)
    }

    /// Records `line` as the answer for right now.
    ///
    /// Best-effort and silent about failure: a read-only cache directory or a
    /// full disk must never turn into a statusline that refuses to print. The
    /// cost of that silence is a cache that never helps, which is exactly the
    /// state a machine with no cache at all is already in -- not a
    /// regression, just no improvement.
    pub fn store(
        &self,
        line: &str,
        transcript_modified_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) {
        let cached = CachedLine {
            line: line.to_owned(),
            rendered_at: now,
            transcript_modified_at,
        };
        let Ok(text) = serde_json::to_string(&cached) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&self.path, text);
    }
}

/// Produces the line to print for this run.
///
/// Three outcomes, tried in order: the cached line, when `cache` holds one
/// that is still fresh by [`StatuslineCache::fresh`]'s rule; a freshly
/// rendered line, when `render` succeeds, which is also written back to
/// `cache` for the next redraw; and the most recently cached line, or
/// [`FALLBACK_LINE`] when there has never been one, when `render` fails.
///
/// `render` is a closure rather than this function reaching into the
/// repository and the price sheet itself, so the caching *policy* -- what
/// counts as fresh, what happens on failure -- can be tested here without a
/// corpus, a price sheet, or any of the I/O a real render needs. `cache`
/// being `None` disables caching outright: every call renders fresh, and a
/// failure falls straight through to [`FALLBACK_LINE`] because there is
/// nothing stale to fall back to.
#[must_use]
pub fn resolve(
    cache: Option<&StatuslineCache>,
    max_age: StdDuration,
    transcript_modified_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    render: impl FnOnce() -> Result<String>,
) -> String {
    if let Some(fresh) = cache.and_then(|cache| cache.fresh(max_age, transcript_modified_at, now)) {
        return fresh;
    }
    match render() {
        Ok(line) => {
            if let Some(cache) = cache {
                cache.store(&line, transcript_modified_at, now);
            }
            line
        }
        Err(_) => cache
            .and_then(StatuslineCache::stale)
            .unwrap_or_else(|| FALLBACK_LINE.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cache file under a directory this test owns, deleted when it ends.
    struct TempCache {
        cache: StatuslineCache,
        dir: PathBuf,
    }

    impl TempCache {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "claude-stats-statusline-cache-{name}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("a writable temp directory");
            let path = dir.join("session.json");
            Self {
                cache: StatuslineCache::at(path),
                dir,
            }
        }
    }

    impl Drop for TempCache {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    #[test]
    fn a_cached_line_is_reused_while_the_transcript_has_not_moved() {
        let temp = TempCache::new("reused");
        let rendered_at = at("2026-09-03T09:00:00Z");
        let transcript = Some(at("2026-09-03T08:59:00Z"));
        temp.cache.store("🤖 first render", transcript, rendered_at);

        // Half a second later, well inside a one-second refresh interval, and
        // the transcript's mtime has not changed.
        let asked_at = at("2026-09-03T09:00:00.500Z");
        let served = temp
            .cache
            .fresh(StdDuration::from_secs(1), transcript, asked_at);

        assert_eq!(served, Some("🤖 first render".to_owned()));
    }

    #[test]
    fn a_grown_transcript_invalidates_the_cached_line() {
        let temp = TempCache::new("invalidated");
        let rendered_at = at("2026-09-03T09:00:00Z");
        temp.cache.store(
            "🤖 stale reading",
            Some(at("2026-09-03T08:59:00Z")),
            rendered_at,
        );

        // Asked again an instant later -- well inside the refresh interval --
        // but the transcript has been written to since the cached render.
        let asked_at = at("2026-09-03T09:00:00.010Z");
        let grown_transcript = Some(at("2026-09-03T09:00:00Z"));
        let served = temp
            .cache
            .fresh(StdDuration::from_secs(1), grown_transcript, asked_at);

        assert_eq!(
            served, None,
            "new work since the cached render must not be hidden behind it"
        );
    }

    #[test]
    fn a_line_older_than_the_refresh_interval_is_not_reused_even_with_no_new_work() {
        let temp = TempCache::new("aged-out");
        let transcript = Some(at("2026-09-03T08:59:00Z"));
        temp.cache
            .store("🤖 old", transcript, at("2026-09-03T09:00:00Z"));

        let two_seconds_later = at("2026-09-03T09:00:02Z");
        let served = temp
            .cache
            .fresh(StdDuration::from_secs(1), transcript, two_seconds_later);

        assert_eq!(served, None);
    }

    #[test]
    fn a_missing_cache_file_is_simply_not_fresh_rather_than_an_error() {
        let temp = TempCache::new("missing");
        assert_eq!(
            temp.cache
                .fresh(StdDuration::from_secs(60), None, at("2026-09-03T09:00:00Z")),
            None
        );
        assert_eq!(temp.cache.stale(), None);
    }

    #[test]
    fn a_failed_render_reprints_the_last_good_line_rather_than_an_error() {
        let temp = TempCache::new("failed-render");
        let now = at("2026-09-03T09:00:00Z");
        temp.cache.store("🤖 last good line", None, now);

        // The cache is stale by the time this render is asked for -- an hour
        // has passed -- so `resolve` must attempt a fresh render, which fails.
        let later = now + chrono::Duration::hours(1);
        let output = resolve(
            Some(&temp.cache),
            StdDuration::from_secs(1),
            None,
            later,
            || anyhow::bail!("the repository could not enumerate the corpus"),
        );

        assert_eq!(output, "🤖 last good line");
    }

    #[test]
    fn a_failed_render_with_nothing_cached_prints_the_fixed_error_line() {
        let temp = TempCache::new("no-fallback");
        let now = at("2026-09-03T09:00:00Z");

        let output = resolve(
            Some(&temp.cache),
            StdDuration::from_secs(1),
            None,
            now,
            || anyhow::bail!("no corpus, and nothing was ever cached"),
        );

        assert_eq!(output, FALLBACK_LINE);
    }

    #[test]
    fn a_successful_render_is_written_back_for_the_next_call() {
        let temp = TempCache::new("write-back");
        let now = at("2026-09-03T09:00:00Z");

        let first = resolve(
            Some(&temp.cache),
            StdDuration::from_secs(1),
            Some(at("2026-09-03T08:59:00Z")),
            now,
            || Ok("🤖 freshly rendered".to_owned()),
        );
        assert_eq!(first, "🤖 freshly rendered");

        // A moment later, well inside the refresh interval and with no new
        // transcript activity: the render just written must be served
        // without the closure being called again.
        let second = resolve(
            Some(&temp.cache),
            StdDuration::from_secs(1),
            Some(at("2026-09-03T08:59:00Z")),
            now + chrono::Duration::milliseconds(200),
            || panic!("must not re-render while the cached line is still fresh"),
        );
        assert_eq!(second, "🤖 freshly rendered");
    }

    #[test]
    fn with_caching_disabled_every_call_renders_fresh() {
        let now = at("2026-09-03T09:00:00Z");
        let mut calls = 0;

        for _ in 0..3 {
            let output = resolve(None, StdDuration::from_secs(60), None, now, || {
                calls += 1;
                Ok(format!("🤖 render {calls}"))
            });
            assert_eq!(output, format!("🤖 render {calls}"));
        }
        assert_eq!(calls, 3, "no cache means nothing is ever reused");
    }
}
