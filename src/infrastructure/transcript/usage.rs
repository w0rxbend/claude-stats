//! Adds up account-wide usage by reading every transcript, cheaply enough to
//! do it while a dashboard is running.
//!
//! The naive version of this re-reads 190-odd files on a timer, some of them
//! close to a megabyte. Two things make that unnecessary:
//!
//! * **A transcript older than the widest window cannot contribute to it.** A
//!   file last written eight days ago cannot hold a response from the last
//!   seven days, because a response is written when it happens. Those files
//!   are skipped without being opened at all, which on a machine with months
//!   of history is nearly all of them.
//! * **A transcript that has not changed yields what it yielded last time.**
//!   Anything that has been read once is kept, keyed by the modification time
//!   and size the catalogue reported; a file whose transcript grows is re-read,
//!   and one that is untouched is not.
//!
//! What is kept per file is not the file: it is the handful of numbers per
//! response that a usage window needs, already filtered to the window. A
//! transcript's worth of prose is read once and thrown away.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};

use super::records::Record;
use crate::application::ports::{AccountUsageReader, TranscriptCatalog};
use crate::domain::limits::{
    AccountUsage, LimitEvent, SessionContribution, UsagePoint, WindowKind,
};
use crate::domain::model::ModelCatalog;

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

/// What was learned from one transcript, and the file state it was learned at.
#[derive(Debug, Clone)]
struct CachedScan {
    /// The modification time the catalogue reported when this was read.
    modified_at: DateTime<Utc>,
    /// The size it reported. Checked as well as the time, because a file can
    /// be rewritten within the same second its timestamp records.
    size_bytes: u64,
    /// The responses, already trimmed to the history window.
    points: Vec<UsagePoint>,
    /// The refusals recorded in this transcript.
    limit_events: Vec<LimitEvent>,
}

/// Scans every transcript on the filesystem, re-reading only what changed.
pub struct IncrementalUsageScanner<C> {
    catalog: C,
    cache: HashMap<PathBuf, CachedScan>,
}

impl<C: TranscriptCatalog> IncrementalUsageScanner<C> {
    /// A scanner over `catalog`, with nothing read yet.
    pub fn new(catalog: C) -> Self {
        Self {
            catalog,
            cache: HashMap::new(),
        }
    }

    /// Reads one transcript for the numbers a usage window needs.
    ///
    /// Everything that is not a priced assistant response or a refusal is
    /// skipped, including sub-agent traffic: a sub-agent's tokens are billed
    /// to the account like any others, so unlike the per-session view they are
    /// counted here rather than excluded.
    fn scan_file(path: &Path, since: DateTime<Utc>) -> (Vec<UsagePoint>, Vec<LimitEvent>) {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return (Vec::new(), Vec::new());
        };

        let mut points = Vec::new();
        let mut events = Vec::new();
        let mut model = String::new();

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Record>(line) else {
                continue;
            };

            if let Some(event) = limit_event(&record) {
                events.push(event);
            }

            if record.r#type != "assistant" {
                continue;
            }
            let Some(message) = &record.message else {
                continue;
            };
            if let Some(named) = &message.model {
                model.clone_from(named);
            }
            let (Some(usage), Some(at)) = (message.usage, record.timestamp) else {
                continue;
            };
            if at < since {
                continue;
            }
            let tokens = usage.into();
            points.push(UsagePoint {
                at,
                tokens,
                cost: tokens.cost(ModelCatalog::pricing_for(&model)),
            });
        }
        (points, events)
    }
}

impl<C: TranscriptCatalog> AccountUsageReader for IncrementalUsageScanner<C> {
    fn usage(&mut self, now: DateTime<Utc>) -> Result<AccountUsage> {
        let since = now - HISTORY;
        let transcripts = self.catalog.list()?;

        let mut contributions = Vec::new();
        let mut limit_events = Vec::new();
        let mut seen = Vec::with_capacity(transcripts.len());

        for transcript in transcripts {
            // Written before the history window opened, so nothing in it can
            // fall inside one. Never opened.
            if transcript.modified_at < since {
                continue;
            }
            seen.push(transcript.path.clone());

            let fresh = self.cache.get(&transcript.path).is_none_or(|cached| {
                cached.modified_at != transcript.modified_at
                    || cached.size_bytes != transcript.size_bytes
            });
            if fresh {
                let (points, events) = Self::scan_file(&transcript.path, since);
                self.cache.insert(
                    transcript.path.clone(),
                    CachedScan {
                        modified_at: transcript.modified_at,
                        size_bytes: transcript.size_bytes,
                        points,
                        limit_events: events,
                    },
                );
            }

            let Some(cached) = self.cache.get(&transcript.path) else {
                continue;
            };
            limit_events.extend(cached.limit_events.iter().copied());
            if !cached.points.is_empty() {
                contributions.push(SessionContribution {
                    session_id: transcript.session_id.clone(),
                    points: cached.points.clone(),
                });
            }
        }

        // Drop transcripts that have aged out of the window, so a dashboard
        // left running for a week does not accumulate them for ever.
        self.cache.retain(|path, _| seen.contains(path));

        // Collapse a run of refusals into the one limit period they belong to.
        // Being refused ten times in the twenty minutes before a reset is one
        // limit being hit, not ten -- every one of those records carries the
        // same reset instant, which is what identifies the period. The
        // earliest refusal is kept, because that is when the limit began to
        // bite.
        limit_events.sort_by_key(|e| (e.resets_at, e.at));
        limit_events.dedup_by_key(|e| (e.resets_at, e.kind));

        Ok(AccountUsage::measure(now, &contributions, limit_events))
    }
}

/// The refusal a record describes, if it describes one.
fn limit_event(record: &Record) -> Option<LimitEvent> {
    let quota = record.quota_limits.as_ref()?;
    if quota.status != "rejected" {
        return None;
    }
    let resets_at = Utc.timestamp_opt(quota.resets_at?, 0).single()?;
    let kind = match quota.rate_limit_type.as_deref()? {
        "five_hour" => WindowKind::Session,
        "weekly" => WindowKind::Week,
        // An unfamiliar limit type is ignored rather than guessed at. Showing
        // a countdown under the wrong heading would be worse than showing
        // none.
        _ => return None,
    };
    Some(LimitEvent {
        at: record.timestamp?,
        resets_at,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;
    use crate::application::ports::{SessionSelector, TranscriptRef};

    /// A catalogue over real files in a temporary directory, which is the only
    /// way to exercise the mtime/size caching the scanner is built around.
    struct DirCatalog {
        dir: PathBuf,
        lists: Rc<Cell<u32>>,
    }

    impl TranscriptCatalog for DirCatalog {
        fn resolve(&self, _s: &SessionSelector) -> Result<Option<TranscriptRef>> {
            Ok(None)
        }

        fn list(&self) -> Result<Vec<TranscriptRef>> {
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

    fn response(at: &str, input: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","message":{{"model":"claude-opus-5","content":[],"usage":{{"input_tokens":{input},"output_tokens":0}}}}}}"#
        )
    }

    fn scanner(dir: &Path) -> (IncrementalUsageScanner<DirCatalog>, Rc<Cell<u32>>) {
        let lists = Rc::new(Cell::new(0));
        (
            IncrementalUsageScanner::new(DirCatalog {
                dir: dir.to_path_buf(),
                lists: Rc::clone(&lists),
            }),
            lists,
        )
    }

    #[test]
    fn usage_is_summed_across_every_session_not_just_one() {
        let dir = temp_dir("across");
        let now = Utc::now();
        let stamp = now.to_rfc3339();
        std::fs::write(dir.join("a.jsonl"), response(&stamp, 100)).expect("write");
        std::fs::write(dir.join("b.jsonl"), response(&stamp, 250)).expect("write");

        let (mut scanner, _) = scanner(&dir);
        let usage = scanner.usage(now).expect("a scan");

        assert_eq!(usage.session.tokens.total(), 350, "both sessions counted");
        assert_eq!(usage.session.sessions, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unchanged_transcript_is_not_read_a_second_time() {
        let dir = temp_dir("cache");
        let now = Utc::now();
        let path = dir.join("a.jsonl");
        std::fs::write(&path, response(&now.to_rfc3339(), 100)).expect("write");

        let (mut scanner, _) = scanner(&dir);
        scanner.usage(now).expect("first scan");

        // Replace the file's contents with a different number of the same
        // byte length, then put the modification time back. The catalogue now
        // reports exactly what it reported before, so as far as the cache key
        // is concerned nothing changed -- and the stale figure proves the file
        // was not opened again.
        let before = std::fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("mtime");
        std::fs::write(&path, response(&now.to_rfc3339(), 999)).expect("rewrite");
        filetime_set(&path, before);

        let usage = scanner.usage(now).expect("second scan");
        assert_eq!(
            usage.session.tokens.total(),
            100,
            "the cached figure was reused rather than the file re-read"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transcript_older_than_the_history_window_is_never_opened() {
        let dir = temp_dir("ancient");
        let now = Utc::now();
        let path = dir.join("old.jsonl");
        std::fs::write(&path, response(&now.to_rfc3339(), 100)).expect("write");
        // Backdate the file itself. Its contents claim to be from now, so if
        // the scanner opened it the tokens would be counted; the modification
        // time is what must keep it out.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24 * 30);
        filetime_set(&path, old);

        let (mut scanner, _) = scanner(&dir);
        let usage = scanner.usage(now).expect("a scan");

        assert_eq!(usage.week.tokens.total(), 0);
        assert!(scanner.cache.is_empty(), "it was never even cached");
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
            format!(
                "{}
{}",
                refusal(10),
                refusal(5)
            ),
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
