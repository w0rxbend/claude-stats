//! Account-wide usage, asked of the corpus as one question.
//!
//! This used to walk the projects directory itself, decode every line and keep
//! its own cache. All of that now lives in [`super::corpus`], behind the
//! [`UsageRepository`] port, because it is not specific to *this* reading: the
//! per-day, per-project and per-model reports want the same deduplicated
//! stream of entries and would otherwise each have grown a scanner of their
//! own, which is how two reports come to disagree about what a week cost.
//!
//! What is left here is the one decision that genuinely belongs to this
//! reading and to no other: **how far back to look**. That rule is expressed
//! as a [`UsageQuery`], handed to the repository, and the answer is handed
//! straight to [`AccountUsage::measure`]. The adapter is thin on purpose --
//! everything it used to do was either a corpus concern or a domain rule, and
//! neither belongs in a port implementation.

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::corpus::FileSystemUsageRepository;
use crate::application::ports::{AccountUsageReader, TranscriptCatalog, UsageQuery};
use crate::domain::limits::{AccountUsage, previous_month_start};
use crate::domain::period::Zone;
use crate::domain::pricing::PriceSheet;

/// How far back the scanner looks.
///
/// Four weeks, which is deliberately wider than the widest window reported.
/// The seven-day window is drawn against the busiest seven-day window on
/// record, and with only seven days of history the current week would always
/// *be* that record: the bar would sit at either 100% or nothing, for ever.
/// Three prior weeks is enough for the comparison to mean something.
///
/// Anything older is never opened. On a machine with months of history that is
/// most of it, which is what keeps a scan of every session affordable.
const HISTORY: chrono::Duration = chrono::Duration::days(28);

/// Reads account-wide usage out of the corpus, re-reading only what changed.
///
/// Owns its repository rather than borrowing one because the caching is the
/// whole point: a dashboard holds one of these for as long as it runs and asks
/// it again every half a minute, and a repository handed in and thrown away
/// per call would open every transcript on the machine each time.
pub struct IncrementalUsageScanner<C> {
    corpus: FileSystemUsageRepository<C>,
    /// The rates this reading is costed at.
    ///
    /// Held rather than looked up so that the account panel is priced by the
    /// same sheet as every other figure in the run, including a user's own
    /// corrections. Composed once at the composition root and handed in;
    /// nothing here decides what a model costs.
    prices: PriceSheet,
    /// The calendar `AccountUsage::today` is bucketed on.
    ///
    /// Held for the same reason as `prices`: [`AccountUsage::measure`] needs
    /// a [`Zone`] and this is the one place in the crate that is allowed to
    /// decide what a live dashboard's "today" means, because it is the
    /// composition root's own reader rather than a report that took a
    /// `--timezone` flag of its own.
    zone: Zone,
}

impl<C: TranscriptCatalog> IncrementalUsageScanner<C> {
    /// A scanner over `catalog`, costed at `prices` and bucketing "today" on
    /// `zone`, with nothing read yet.
    pub fn new(catalog: C, prices: PriceSheet, zone: Zone) -> Self {
        Self {
            corpus: FileSystemUsageRepository::new(catalog),
            prices,
            zone,
        }
    }

    /// The question this reading asks of the corpus, as of `now`.
    ///
    /// Only the lower bound is set. Everything else is left at its default,
    /// which is to say: every project, every model, every session, and
    /// sub-agent traffic included -- a sub-agent's tokens are charged to the
    /// account exactly like the main thread's, and leaving them out would
    /// under-report the bill several-fold rather than slightly.
    ///
    /// There is deliberately no upper bound. A window is closed by
    /// [`AccountUsage::measure`] itself, and the busiest-window comparison
    /// wants the whole scanned history rather than a truncated tail.
    fn horizon(now: DateTime<Utc>) -> UsageQuery {
        // Whichever reaches further back: the rolling-window history, or the
        // start of the previous calendar month. The month totals need the
        // latter, and near the start of a month it is up to ~62 days ago,
        // which four weeks of history would silently truncate.
        UsageQuery {
            since: Some((now - HISTORY).min(previous_month_start(now))),
            ..UsageQuery::default()
        }
    }
}

impl<C: TranscriptCatalog> AccountUsageReader for IncrementalUsageScanner<C> {
    fn usage(&mut self, now: DateTime<Utc>) -> Result<AccountUsage> {
        // Entries and refusals in one pass, because they come out of the same
        // lines of the same files and reading the corpus twice to separate
        // them would double the cost of the only expensive thing this crate
        // does.
        let (entries, limit_events) = self.corpus.entries_and_limit_events(&Self::horizon(now))?;

        // The refusals go over raw. Collapsing them into distinct limit
        // periods is a rule about what a limit period is, so it belongs to the
        // domain -- see `LimitEvent::collapse_periods`, which `measure` calls.
        Ok(AccountUsage::measure(
            now,
            &entries,
            limit_events,
            &self.prices,
            &self.zone,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use super::*;
    use crate::application::ports::{SessionSelector, TranscriptRef};
    use crate::domain::limits::WindowKind;

    /// A catalogue over real files in a temporary directory, which is the only
    /// way to exercise the modification-time caching the reading is built
    /// around.
    struct DirCatalog {
        dir: PathBuf,
        lists: Rc<Cell<u32>>,
    }

    impl TranscriptCatalog for DirCatalog {
        fn resolve(&self, _s: &SessionSelector) -> Result<Option<TranscriptRef>> {
            Ok(None)
        }

        fn list(&self) -> Result<Vec<TranscriptRef>> {
            self.list_billable()
        }

        // The repository asks for the billable corpus, so the call counter the
        // caching tests assert on has to live here rather than in `list`.
        fn list_billable(&self) -> Result<Vec<TranscriptRef>> {
            self.lists.set(self.lists.get() + 1);
            let mut out = Vec::new();
            for entry in std::fs::read_dir(&self.dir)? {
                let entry = entry?;
                let meta = entry.metadata()?;
                out.push(TranscriptRef {
                    session_id: entry.file_name().to_string_lossy().into_owned(),
                    path: entry.path(),
                    project_dir: "/project".to_owned(),
                    modified_at: meta.modified()?.into(),
                    size_bytes: meta.len(),
                });
            }
            Ok(out)
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("claude-stats-usage-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    /// One assistant response, spelled the way a transcript spells it.
    ///
    /// Both the session and the message id are written out rather than left to
    /// the file name and a stand-in, because together they are what decides
    /// whether two recorded rows are one charge or two. A fixture that left
    /// either off would be exercising the fallbacks rather than the sum.
    fn response(session: &str, id: &str, at: &str, input: u64) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"{session}","requestId":"req-{id}","timestamp":"{at}","message":{{"id":"{id}","model":"claude-opus-5","content":[],"usage":{{"input_tokens":{input},"output_tokens":0}}}}}}"#
        )
    }

    fn scanner(dir: &Path) -> (IncrementalUsageScanner<DirCatalog>, Rc<Cell<u32>>) {
        let lists = Rc::new(Cell::new(0));
        (
            IncrementalUsageScanner::new(
                DirCatalog {
                    dir: dir.to_path_buf(),
                    lists: Rc::clone(&lists),
                },
                PriceSheet::builtin(),
                Zone::Utc,
            ),
            lists,
        )
    }

    #[test]
    fn usage_is_summed_across_every_session_not_just_one() {
        let dir = temp_dir("across");
        let now = Utc::now();
        let stamp = now.to_rfc3339();
        std::fs::write(
            dir.join("a.jsonl"),
            response("session-a", "msg_a", &stamp, 100),
        )
        .expect("write");
        std::fs::write(
            dir.join("b.jsonl"),
            response("session-b", "msg_b", &stamp, 250),
        )
        .expect("write");

        let (mut scanner, _) = scanner(&dir);
        let usage = scanner.usage(now).expect("a scan");

        assert_eq!(usage.session.tokens.total(), 350, "both sessions counted");
        assert_eq!(usage.session.sessions, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_response_recorded_in_two_transcripts_is_charged_for_once() {
        // The correction this reading exists to carry: Claude Code copies a
        // response into every transcript that replays the conversation it
        // belongs to, so adding rows up counts one charge several times.
        let dir = temp_dir("dedup");
        let now = Utc::now();
        let stamp = now.to_rfc3339();
        // The same conversation, written down by two transcripts: one
        // response, charged for once.
        std::fs::write(
            dir.join("a.jsonl"),
            response("session-a", "msg_01", &stamp, 100),
        )
        .expect("write");
        std::fs::write(
            dir.join("b.jsonl"),
            response("session-a", "msg_01", &stamp, 100),
        )
        .expect("write");

        let (mut scanner, _) = scanner(&dir);
        let usage = scanner.usage(now).expect("a scan");

        assert_eq!(
            usage.session.tokens.total(),
            100,
            "one response, however many transcripts recorded it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unchanged_transcript_is_not_read_a_second_time() {
        let dir = temp_dir("cache");
        let now = Utc::now();
        let path = dir.join("a.jsonl");
        std::fs::write(
            &path,
            response("session-a", "msg_01", &now.to_rfc3339(), 100),
        )
        .expect("write");

        let (mut scanner, _) = scanner(&dir);
        scanner.usage(now).expect("first scan");
        assert_eq!(scanner.corpus.transcripts_read(), 1);

        // Replace the file's contents with a different number of the same byte
        // length, then put the modification time back. The catalogue now
        // reports exactly what it reported before, so as far as the cache key
        // is concerned nothing changed -- and the stale figure proves the file
        // was not opened again.
        let before = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime");
        std::fs::write(
            &path,
            response("session-a", "msg_01", &now.to_rfc3339(), 999),
        )
        .expect("rewrite");
        filetime_set(&path, before);

        let usage = scanner.usage(now).expect("second scan");
        assert_eq!(
            usage.session.tokens.total(),
            100,
            "the cached figure was reused rather than the file re-read"
        );
        assert_eq!(scanner.corpus.transcripts_read(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transcript_older_than_the_history_window_is_never_opened() {
        let dir = temp_dir("ancient");
        let now = Utc::now();
        let path = dir.join("old.jsonl");
        std::fs::write(
            &path,
            response("session-a", "msg_01", &now.to_rfc3339(), 100),
        )
        .expect("write");
        // Backdate the file itself. Its contents claim to be from now, so if
        // the scanner opened it the tokens would be counted; the modification
        // time is what must keep it out.
        // Seventy days: past the four-week rolling history *and* past the
        // start of the previous calendar month, which is at most ~62 days ago.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24 * 70);
        filetime_set(&path, old);

        let (mut scanner, _) = scanner(&dir);
        let usage = scanner.usage(now).expect("a scan");

        assert_eq!(usage.week.tokens.total(), 0);
        assert_eq!(
            scanner.corpus.transcripts_read(),
            0,
            "it was never even opened"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rate_limit_refusal_becomes_an_event_with_the_servers_own_reset_time() {
        let dir = temp_dir("limit");
        let now = Utc::now();
        let resets = now.timestamp() + 3_600;
        let line = format!(
            r#"{{"type":"assistant","timestamp":"{}","error":"rate_limit","quotaLimits":{{"status":"rejected","resetsAt":{resets},"rateLimitType":"five_hour"}}}}"#,
            now.to_rfc3339()
        );
        std::fs::write(dir.join("a.jsonl"), line).expect("write");

        let (mut scanner, _) = scanner(&dir);
        let usage = scanner.usage(now).expect("a scan");

        let active = usage.active_limit().expect("a limit still in force");
        assert_eq!(active.kind, WindowKind::Session);
        assert_eq!(active.resets_at.timestamp(), resets);
        assert_eq!(
            active.time_until_reset(now).map(|d| d.num_minutes()),
            Some(59),
            "just under the hour, since a moment has passed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_of_refusals_against_one_reset_is_a_single_limit_period() {
        let dir = temp_dir("period");
        let now = Utc::now();
        let resets = now.timestamp() + 1_800;
        // Being refused repeatedly while waiting out one limit is one period,
        // however many times the request was retried, and however many
        // transcripts recorded it.
        let refusal = |offset: i64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{}","quotaLimits":{{"status":"rejected","resetsAt":{resets},"rateLimitType":"five_hour"}}}}"#,
                (now - chrono::Duration::minutes(offset)).to_rfc3339()
            )
        };
        std::fs::write(
            dir.join("a.jsonl"),
            format!("{}\n{}", refusal(10), refusal(5)),
        )
        .expect("write");
        std::fs::write(dir.join("b.jsonl"), refusal(7)).expect("write");

        let (mut scanner, _) = scanner(&dir);
        let usage = scanner.usage(now).expect("a scan");

        assert_eq!(usage.limit_events.len(), 1, "three refusals, one period");
        assert_eq!(
            usage.limit_events[0].at,
            usage
                .limit_events
                .iter()
                .map(|e| e.at)
                .min()
                .expect("an earliest"),
            "the period starts when it first bit"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unfamiliar_limit_type_is_ignored_rather_than_guessed_at() {
        let dir = temp_dir("unknown");
        let now = Utc::now();
        let line = format!(
            r#"{{"type":"assistant","timestamp":"{}","quotaLimits":{{"status":"rejected","resetsAt":{},"rateLimitType":"monthly"}}}}"#,
            now.to_rfc3339(),
            now.timestamp() + 60
        );
        std::fs::write(dir.join("a.jsonl"), line).expect("write");

        let (mut scanner, _) = scanner(&dir);
        let usage = scanner.usage(now).expect("a scan");

        assert!(usage.limit_events.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_horizon_reaches_back_to_the_previous_month_when_that_is_further() {
        // Early in a month, the start of the previous one is further back than
        // four weeks, and the month totals would be missing their older half
        // if the rolling history alone decided the floor.
        let second_of_the_month = "2026-09-02T00:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("a valid timestamp");
        let query = IncrementalUsageScanner::<DirCatalog>::horizon(second_of_the_month);

        assert_eq!(
            query.since,
            Some(previous_month_start(second_of_the_month)),
            "the first of August, not five days into it"
        );
        assert_eq!(
            query.until, None,
            "a window is closed by the domain, not here"
        );
        assert!(
            query.include_sidechains,
            "a sub-agent's tokens are charged to the account like any others"
        );
    }

    /// Sets a file's modification time.
    ///
    /// Written by hand rather than pulling in the `filetime` crate for one
    /// call in one test.
    fn filetime_set(path: &Path, when: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("the file to backdate");
        file.set_modified(when).expect("a settable timestamp");
    }
}
