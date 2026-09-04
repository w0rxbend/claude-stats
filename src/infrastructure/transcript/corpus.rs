//! The whole corpus of transcripts, answered as one deduplicated collection of
//! billable entries.
//!
//! This is the crate's Repository. Above it, a report asks "every entry
//! matching this query" and gets a list; below it are several thousand JSON
//! Lines files, some of them close to a megabyte, most of them written by
//! sub-agents nobody ever looked at. Putting the whole corpus behind one
//! object is what stops two reports quietly disagreeing about what a week
//! cost, because there is only one place left that could answer.
//!
//! # Why the answer is not simply "read every file and add it up"
//!
//! Two things would make that wrong, and a third would make it too slow to do
//! while a dashboard is running.
//!
//! **Claude Code writes the same response down more than once.** Resume a
//! session and the replayed history is written again; fork it and both
//! branches carry the shared prefix; run a sub-agent and its transcript
//! repeats what it was handed. On this machine one 208-row transcript held
//! only 130 distinct message/request pairs. Adding up rows therefore
//! overstates the bill by something like 60%, which is why every entry passes
//! through an Identity Map on the way out: a second sighting of an
//! [`EntryId`] resolves to the copy already held instead of adding to the
//! total.
//!
//! **The horizon moves backwards.** A cache keyed only on a file's
//! modification time and size looks correct until a caller asks for an earlier
//! month than the last one it asked for. Nothing about the *files* changed, so
//! every cached scan is reused -- and every one of them was trimmed to the
//! later horizon, so the older tail of the month is silently missing. The
//! fix is [`CachedScan::trimmed_to`]: a scan is stale when the file changed
//! **or** when the question now reaches further back than the answer does.
//!
//! **Almost nothing needs to be read.** Three layers of Lazy Load keep a scan
//! of the whole corpus affordable: a file last written before the query's
//! lower bound cannot hold an entry inside it and is never opened at all; a
//! file that has not changed since it was read yields what it yielded last
//! time; and what is kept per file is not the file but the handful of entries
//! it contributed, the prose having been read once and thrown away.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};

use super::entry_mapper::EntryMapper;
use super::records::Record;
use crate::application::ports::{TranscriptCatalog, TranscriptRef, UsageQuery, UsageRepository};
use crate::domain::entry::{Entry, EntryId};
use crate::domain::limits::{LimitEvent, WindowKind};

/// What was learned from one transcript, and the question it was learned for.
#[derive(Debug, Clone)]
struct CachedScan {
    /// The modification time the catalogue reported when this was read.
    modified_at: DateTime<Utc>,
    /// The size it reported. Checked as well as the time, because a file can
    /// be rewritten within the same second its timestamp records.
    size_bytes: u64,
    /// The lower bound this scan was trimmed to.
    ///
    /// Kept because the file's state is only half of what makes a cached scan
    /// usable. The other half is how far back the caller was asking: a scan
    /// trimmed to the first of September answers nothing about August, so the
    /// moment somebody asks about August it has to be read again even though
    /// nothing about the file changed. Without this field that older tail
    /// vanishes from every unchanged transcript at once, which is an
    /// under-report of a whole month that nothing else in the system would
    /// notice.
    ///
    /// The account scanner's own horizon only ever creeps forwards, so it is
    /// not the caller that reaches this. A report asked for an earlier range
    /// than the last one it was asked for is -- `--since last-month` after
    /// `--since today`, over the same repository, which is the ordinary way a
    /// command line gets used.
    trimmed_to: DateTime<Utc>,
    /// The billable responses at or after [`Self::trimmed_to`], in the order
    /// the file wrote them.
    entries: Vec<Entry>,
    /// The refusals recorded in this transcript.
    limit_events: Vec<LimitEvent>,
}

/// The corpus of transcripts on this filesystem, read through a catalogue.
///
/// Built from a [`TranscriptCatalog`] rather than a directory path so that the
/// question "where do transcripts live" stays answered in exactly one place,
/// and so that a test can stand a handful of files in front of it without
/// pretending to be a home directory.
pub struct FileSystemUsageRepository<C> {
    catalog: C,
    cache: HashMap<PathBuf, CachedScan>,
    reads: u64,
}

impl<C: TranscriptCatalog> FileSystemUsageRepository<C> {
    /// A repository over `catalog`, with nothing read yet.
    pub fn new(catalog: C) -> Self {
        Self {
            catalog,
            cache: HashMap::new(),
            reads: 0,
        }
    }

    /// How many transcripts this repository has tried to open since it was
    /// built.
    ///
    /// Counts the attempt rather than the success, because a file that turns
    /// out to be unreadable still cost a syscall and still occupies a cache
    /// slot afterwards.
    ///
    /// Public because the three layers of Lazy Load above are the entire
    /// reason re-reading the corpus every half a minute costs almost nothing,
    /// and a cache that stopped working would show up as a gradually slower
    /// dashboard rather than as a wrong number -- which is to say, it would
    /// not show up at all. This is the figure that makes the caching an
    /// assertable property instead of a hope.
    #[must_use]
    pub const fn transcripts_read(&self) -> u64 {
        self.reads
    }

    /// Every entry matching `query`, together with every refusal recorded in
    /// the transcripts the query's horizon let through.
    ///
    /// The two are collected in one pass because they come out of the same
    /// lines of the same files, and reading the corpus twice to separate them
    /// would double the cost of the only expensive thing this crate does.
    /// They are returned as a pair rather than folded together because a
    /// refusal is not an entry: nothing was sold, nothing was charged, and no
    /// project, model or session filter has any business narrowing the list of
    /// moments the account was told to stop.
    ///
    /// # Errors
    ///
    /// Returns an error only when the catalogue cannot be listed. See
    /// [`UsageRepository::entries`] for why a single unreadable transcript is
    /// not one.
    pub fn entries_and_limit_events(
        &mut self,
        query: &UsageQuery,
    ) -> Result<(Vec<Entry>, Vec<LimitEvent>)> {
        // The billable corpus, not the session listing. A sub-agent's tokens
        // are charged to the account like any others, and the nested
        // `<session-id>/subagents/...` transcripts `list` omits are about 97%
        // of the files here and most of the bill.
        let transcripts = self.catalog.list_billable()?;

        // An unbounded query has no floor, so nothing can be pruned and
        // nothing trimmed. Spelling that as the earliest representable instant
        // rather than as an `Option` keeps the two comparisons below free of a
        // special case each.
        let floor = query.since.unwrap_or(DateTime::<Utc>::MIN_UTC);

        let mut identity = EntryIdentityMap::new();
        let mut limit_events = Vec::new();
        // A set rather than the list this used to be: eviction asks "is this
        // path still listed" once per cached path, and a linear scan of
        // several thousand paths inside that loop is quadratic work done on
        // every scan for no reason.
        let mut live: HashSet<PathBuf> = HashSet::with_capacity(transcripts.len());

        for transcript in transcripts {
            // Written before the query's window opened, so nothing in it can
            // fall inside one: a response is recorded when it happens, so a
            // file untouched since August holds nothing from September. Never
            // opened, which on a machine with months of history is nearly
            // every file.
            if transcript.modified_at < floor {
                continue;
            }
            live.insert(transcript.path.clone());

            if self.is_stale(&transcript, floor) {
                let scan = self.read_transcript(&transcript, floor);
                self.cache.insert(transcript.path.clone(), scan);
            }

            let Some(cached) = self.cache.get(&transcript.path) else {
                continue;
            };
            limit_events.extend(cached.limit_events.iter().copied());
            for entry in &cached.entries {
                if query.matches(entry) {
                    identity.offer(entry.clone());
                }
            }
        }

        // Drop transcripts that have aged out of the window, so a dashboard
        // left running for a week does not accumulate them for ever.
        self.cache.retain(|path, _| live.contains(path));

        Ok((identity.into_sorted(), limit_events))
    }

    /// Whether what is cached for `transcript` can still answer a query whose
    /// lower bound is `floor`.
    fn is_stale(&self, transcript: &TranscriptRef, floor: DateTime<Utc>) -> bool {
        self.cache.get(&transcript.path).is_none_or(|cached| {
            cached.modified_at != transcript.modified_at
                || cached.size_bytes != transcript.size_bytes
                // The horizon has moved backwards past what this answer
                // covers, so the answer is missing its own older tail.
                || floor < cached.trimmed_to
        })
    }

    /// Reads one transcript into the entries and refusals it contributes.
    ///
    /// A file that cannot be read at all, and a line that cannot be parsed,
    /// are both skipped rather than reported. The common cause is neither
    /// corruption nor a bug: it is a session running right now, whose last
    /// line is half written because the process is in the middle of appending
    /// it.
    fn read_transcript(&mut self, transcript: &TranscriptRef, floor: DateTime<Utc>) -> CachedScan {
        self.reads += 1;
        let mut scan = CachedScan {
            modified_at: transcript.modified_at,
            size_bytes: transcript.size_bytes,
            trimmed_to: floor,
            entries: Vec::new(),
            limit_events: Vec::new(),
        };
        let Ok(contents) = std::fs::read_to_string(&transcript.path) else {
            return scan;
        };

        // One mapper per file, fed lines in written order: the model is sticky
        // across a transcript and an unnamed response needs a name unique
        // within the file it came from, so sharing a mapper between files
        // would both misprice and collide.
        let mut mapper = EntryMapper::new(&transcript.session_id, &transcript.project_dir);
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Record>(line) else {
                continue;
            };
            if let Some(event) = limit_event(&record) {
                scan.limit_events.push(event);
            }
            if let Some(entry) = mapper.map(&record) {
                // Trimming here rather than at query time is what makes the
                // cache small: a year-old transcript that is still being
                // appended to is read in full and kept as the handful of
                // entries inside the horizon.
                if entry.at >= floor {
                    scan.entries.push(entry);
                }
            }
        }
        scan
    }
}

impl<C: TranscriptCatalog> UsageRepository for FileSystemUsageRepository<C> {
    fn entries(&mut self, query: &UsageQuery) -> Result<Vec<Entry>> {
        self.entries_and_limit_events(query)
            .map(|(entries, _)| entries)
    }
}

/// One copy of each distinct response, however many transcripts recorded it.
///
/// Fowler's Identity Map, scoped to a single query rather than to the process.
/// That scope is deliberate: a process-wide map would grow to hold the whole
/// corpus and never shrink, which is exactly the wrong shape for a dashboard
/// that is meant to sit in a terminal for days. The cost of rebuilding it per
/// query is paid by the per-file memoisation above, which means rebuilding it
/// is cloning entries out of a cache rather than reading files again.
struct EntryIdentityMap {
    /// Where each identity's chosen copy sits in [`Self::entries`].
    seen: HashMap<EntryId, usize>,
    /// The chosen copies, in the order their identities were first offered.
    entries: Vec<Entry>,
}

impl EntryIdentityMap {
    fn new() -> Self {
        Self {
            seen: HashMap::new(),
            entries: Vec::new(),
        }
    }

    /// Offers a copy of a response, keeping whichever copy is the better
    /// record of it.
    fn offer(&mut self, entry: Entry) {
        if let Some(&index) = self.seen.get(&entry.id) {
            // Seen before. Whichever of the two copies is the better record of
            // the response wins; the count does not go up either way.
            if supersedes(&entry, &self.entries[index]) {
                self.entries[index] = entry;
            }
        } else {
            self.seen.insert(entry.id.clone(), self.entries.len());
            self.entries.push(entry);
        }
    }

    /// The chosen copies, oldest first.
    ///
    /// Sorted by identity as well as by time so that two responses recorded in
    /// the same instant -- which happens, transcripts are stamped to the
    /// second -- always come out in the same order. Without the tie-break the
    /// order would be the order the hash map happened to iterate in, and a
    /// figure that depends on a hash seed is a figure that changes between
    /// runs for no reason a user could ever explain.
    fn into_sorted(self) -> Vec<Entry> {
        let mut entries = self.entries;
        entries.sort_by(|a, b| {
            a.at.cmp(&b.at)
                .then_with(|| a.id.message_id.cmp(&b.id.message_id))
                .then_with(|| a.id.request_id.cmp(&b.id.request_id))
                .then_with(|| a.id.session.cmp(&b.id.session))
        });
        entries
    }
}

/// Whether `candidate` is a better record of a response than `held`.
///
/// Two transcripts holding the same response do not always hold it equally
/// well, so the choice is made by rule rather than by whichever file the
/// directory listing happened to return first.
///
/// 1. **The parent copy beats the sidechain copy.** A sub-agent's transcript
///    repeats what it was handed; the transcript of the conversation that
///    handed it over is the canonical record of it.
/// 2. **More tokens beats fewer.** The only way one copy of a response can
///    report fewer tokens than another is if it was written down before the
///    response finished, so the larger figure is the complete one and the
///    smaller is a truncated write.
/// 3. **Otherwise the copy already held stays.** Two indistinguishable copies
///    are indistinguishable; swapping them would only make the result depend
///    on listing order.
fn supersedes(candidate: &Entry, held: &Entry) -> bool {
    if candidate.is_sidechain != held.is_sidechain {
        return !candidate.is_sidechain;
    }
    candidate.tokens.total() > held.tokens.total()
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
    use std::path::Path;
    use std::rc::Rc;

    use super::*;
    use crate::application::ports::{SessionSelector, TranscriptRef};
    use crate::infrastructure::transcript::locator::FileSystemCatalog;

    /// A catalogue over real files in a temporary directory, which is the only
    /// way to exercise the modification-time caching the repository is built
    /// around.
    ///
    /// Lists in file-name order, or reversed on request, so that a test can
    /// say what order the corpus was read in -- `read_dir` will not.
    struct DirCatalog {
        dir: PathBuf,
        reversed: bool,
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
                    project_dir: "/home/ada/api".to_owned(),
                    modified_at: meta.modified()?.into(),
                    size_bytes: meta.len(),
                });
            }
            out.sort_by(|a, b| a.path.cmp(&b.path));
            if self.reversed {
                out.reverse();
            }
            Ok(out)
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("claude-stats-corpus-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    fn repository(dir: &Path) -> FileSystemUsageRepository<DirCatalog> {
        FileSystemUsageRepository::new(DirCatalog {
            dir: dir.to_path_buf(),
            reversed: false,
            lists: Rc::new(Cell::new(0)),
        })
    }

    /// A fixed instant, `minutes` after 2026-09-01T00:00:00Z.
    fn at(minutes: i64) -> DateTime<Utc> {
        "2026-09-01T00:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("a valid timestamp")
            + chrono::Duration::minutes(minutes)
    }

    /// One assistant response, spelled the way a transcript spells it.
    fn response(
        session: &str,
        message_id: &str,
        request_id: &str,
        at: DateTime<Utc>,
        input: u64,
    ) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"{session}","requestId":"{request_id}","timestamp":"{}","message":{{"id":"{message_id}","model":"claude-opus-5","content":[],"usage":{{"input_tokens":{input},"output_tokens":0}}}}}}"#,
            at.to_rfc3339()
        )
    }

    /// The same, written by a sub-agent rather than the main thread.
    fn sidechain_response(
        session: &str,
        message_id: &str,
        request_id: &str,
        at: DateTime<Utc>,
        input: u64,
    ) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":true,"sessionId":"{session}","requestId":"{request_id}","timestamp":"{}","message":{{"id":"{message_id}","model":"claude-opus-5","content":[],"usage":{{"input_tokens":{input},"output_tokens":0}}}}}}"#,
            at.to_rfc3339()
        )
    }

    fn total(entries: &[Entry]) -> u64 {
        entries.iter().map(|e| e.tokens.total()).sum()
    }

    /// Sets a file's modification time.
    ///
    /// Written by hand rather than pulling in the `filetime` crate for two
    /// calls in two tests.
    fn set_mtime(path: &Path, when: DateTime<Utc>) {
        let seconds = u64::try_from(when.timestamp()).expect("an instant after the epoch");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("the file to restamp");
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
            .expect("a settable timestamp");
    }

    #[test]
    fn two_rows_sharing_a_message_id_and_request_id_are_counted_once() {
        // The duplicate the whole Identity Map exists for: one response
        // written into the transcript twice because the session was resumed
        // and its history replayed.
        let dir = temp_dir("same-id");
        std::fs::write(
            dir.join("a.jsonl"),
            format!(
                "{}\n{}\n",
                response("session-a", "msg_01", "req_01", at(0), 100),
                response("session-a", "msg_01", "req_01", at(0), 100),
            ),
        )
        .expect("write");

        let entries = repository(&dir)
            .entries(&UsageQuery::default())
            .expect("a scan");

        assert_eq!(
            entries.len(),
            1,
            "one response, however many rows recorded it"
        );
        assert_eq!(total(&entries), 100, "not 200");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_message_retried_under_a_new_request_id_is_counted_twice_because_it_was_billed_twice()
     {
        // One message id can span several API requests when a response is
        // retried or continued. Each request was charged for, so collapsing
        // them would hide money rather than stop inventing it.
        let dir = temp_dir("retry");
        std::fs::write(
            dir.join("a.jsonl"),
            format!(
                "{}\n{}\n",
                response("session-a", "msg_01", "req_01", at(0), 100),
                response("session-a", "msg_01", "req_02", at(1), 100),
            ),
        )
        .expect("write");

        let entries = repository(&dir)
            .entries(&UsageQuery::default())
            .expect("a scan");

        assert_eq!(entries.len(), 2);
        assert_eq!(total(&entries), 200);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_duplicate_pair_prefers_the_parent_copy_over_the_sidechain_one() {
        // The sidechain copy is given the *larger* token count on purpose: if
        // the rules were applied in the other order the bigger figure would
        // win and the test would pass for the wrong reason.
        let dir = temp_dir("parent-wins");
        std::fs::write(
            dir.join("a-parent.jsonl"),
            response("session-a", "msg_01", "req_01", at(0), 100),
        )
        .expect("write");
        std::fs::write(
            dir.join("b-child.jsonl"),
            sidechain_response("session-a", "msg_01", "req_01", at(0), 999),
        )
        .expect("write");

        let entries = repository(&dir)
            .entries(&UsageQuery::default())
            .expect("a scan");

        assert_eq!(entries.len(), 1);
        assert!(
            !entries[0].is_sidechain,
            "the parent transcript is canonical"
        );
        assert_eq!(total(&entries), 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn between_two_copies_the_one_with_more_tokens_wins() {
        // A copy can only report fewer tokens than another by having been
        // written before the response finished, so the larger figure is the
        // complete record and the smaller one is a truncated write.
        let dir = temp_dir("more-tokens");
        std::fs::write(
            dir.join("a.jsonl"),
            response("session-a", "msg_01", "req_01", at(0), 40),
        )
        .expect("write");
        std::fs::write(
            dir.join("b.jsonl"),
            response("session-a", "msg_01", "req_01", at(0), 400),
        )
        .expect("write");

        let entries = repository(&dir)
            .entries(&UsageQuery::default())
            .expect("a scan");

        assert_eq!(entries.len(), 1);
        assert_eq!(total(&entries), 400);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deduplication_does_not_depend_on_the_order_the_files_were_read() {
        // Whichever file the directory listing happens to return first, the
        // account was charged the same amount. A total that moves when the
        // filesystem reorders itself is a total nobody can check.
        let dir = temp_dir("order");
        std::fs::write(
            dir.join("a.jsonl"),
            format!(
                "{}\n{}\n",
                response("session-a", "msg_01", "req_01", at(0), 40),
                response("session-a", "msg_02", "req_02", at(5), 7),
            ),
        )
        .expect("write");
        std::fs::write(
            dir.join("b.jsonl"),
            sidechain_response("session-a", "msg_01", "req_01", at(0), 400),
        )
        .expect("write");

        let mut forwards = FileSystemUsageRepository::new(DirCatalog {
            dir: dir.clone(),
            reversed: false,
            lists: Rc::new(Cell::new(0)),
        });
        let mut backwards = FileSystemUsageRepository::new(DirCatalog {
            dir: dir.clone(),
            reversed: true,
            lists: Rc::new(Cell::new(0)),
        });

        let one_way = forwards.entries(&UsageQuery::default()).expect("a scan");
        let other_way = backwards.entries(&UsageQuery::default()).expect("a scan");

        assert_eq!(one_way, other_way, "same corpus, same answer, either order");
        assert_eq!(
            total(&one_way),
            47,
            "the parent copy of msg_01, plus msg_02"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cached_scan_is_re_read_when_the_horizon_reaches_further_back_than_it_did() {
        // Two calls straddling a month rollover, against a file that does not
        // change between them. Keyed on file state alone the second call
        // reuses a scan trimmed to September and reports nothing for August,
        // which is how a whole month of spend disappears without a single
        // thing looking wrong.
        let dir = temp_dir("horizon");
        let path = dir.join("a.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                response("session-a", "msg_aug", "req_aug", at(-60 * 24 * 20), 200),
                response("session-a", "msg_sep", "req_sep", at(60), 100),
            ),
        )
        .expect("write");
        set_mtime(&path, at(120));

        let mut repository = repository(&dir);

        let september = repository
            .entries(&UsageQuery {
                since: Some(at(0)),
                ..UsageQuery::default()
            })
            .expect("a scan");
        assert_eq!(total(&september), 100, "August is outside this horizon");
        assert_eq!(repository.transcripts_read(), 1);

        let both_months = repository
            .entries(&UsageQuery {
                since: Some(at(-60 * 24 * 40)),
                ..UsageQuery::default()
            })
            .expect("a second scan");
        assert_eq!(
            total(&both_months),
            300,
            "the older tail came back rather than staying trimmed away"
        );
        assert_eq!(
            repository.transcripts_read(),
            2,
            "the unchanged file was re-read, because the question changed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unchanged_transcript_is_not_read_a_second_time() {
        let dir = temp_dir("cache");
        std::fs::write(
            dir.join("a.jsonl"),
            response("session-a", "msg_01", "req_01", at(0), 100),
        )
        .expect("write");

        let mut repository = repository(&dir);
        let query = UsageQuery {
            since: Some(at(-60)),
            ..UsageQuery::default()
        };
        let first = repository.entries(&query).expect("a scan");
        let second = repository.entries(&query).expect("a second scan");

        assert_eq!(first, second);
        assert_eq!(
            repository.transcripts_read(),
            1,
            "the same question about the same file was answered from memory"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transcript_older_than_the_query_is_never_opened() {
        let dir = temp_dir("ancient");
        let path = dir.join("old.jsonl");
        // The contents claim to be recent, so opening the file would count the
        // tokens. Only the modification time can keep it out.
        std::fs::write(&path, response("session-a", "msg_01", "req_01", at(0), 100))
            .expect("write");
        set_mtime(&path, at(-60 * 24 * 90));

        let mut repository = repository(&dir);
        let entries = repository
            .entries(&UsageQuery {
                since: Some(at(-60 * 24 * 7)),
                ..UsageQuery::default()
            })
            .expect("a scan");

        assert!(entries.is_empty());
        assert_eq!(repository.transcripts_read(), 0, "it was never opened");
        assert!(repository.cache.is_empty(), "nor even cached");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nested_subagent_transcript_is_scanned_because_its_tokens_are_billed_too() {
        // Read through the real catalogue rather than the test double, because
        // the thing being asserted is that the repository asks for the
        // *billable* corpus: `list` stops at the top of a project directory
        // and would miss this file entirely.
        let projects = temp_dir("nested");
        let project = projects.join("-home-ada-api");
        let nested = project.join("sid-1/subagents/workflows/wf_x");
        std::fs::create_dir_all(&nested).expect("the nested directories");
        std::fs::write(
            project.join("sid-1.jsonl"),
            response("sid-1", "msg_01", "req_01", at(0), 10),
        )
        .expect("write");
        // The helper's own line names no session, so the transcript's owning
        // directory is what attributes it -- which is the point.
        std::fs::write(
            nested.join("agent-y.jsonl"),
            format!(
                r#"{{"type":"assistant","isSidechain":true,"requestId":"req_02","timestamp":"{}","message":{{"id":"msg_02","model":"claude-opus-5","content":[],"usage":{{"input_tokens":90,"output_tokens":0}}}}}}"#,
                at(1).to_rfc3339()
            ),
        )
        .expect("write");

        let mut repository =
            FileSystemUsageRepository::new(FileSystemCatalog::rooted_at(projects.clone()));
        let entries = repository.entries(&UsageQuery::default()).expect("a scan");

        assert_eq!(entries.len(), 2, "the sub-agent's response counts too");
        assert_eq!(total(&entries), 100);
        let helper = entries
            .iter()
            .find(|e| e.is_sidechain)
            .expect("the sub-agent's entry");
        assert_eq!(
            helper.session,
            crate::domain::project::SessionId::new("sid-1"),
            "billed to the session that spawned it, not to a name nobody has seen"
        );
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn a_half_written_final_line_does_not_lose_the_lines_before_it() {
        // A transcript is appended to while a session runs, so the last line
        // of the file somebody is working in right now is routinely a partial
        // one. Refusing the whole file over it would empty the report at
        // exactly the moment it is being read.
        let dir = temp_dir("truncated");
        let complete = response("session-a", "msg_01", "req_01", at(0), 100);
        std::fs::write(
            dir.join("a.jsonl"),
            format!("{complete}\n{{\"type\":\"assistant\",\"timest"),
        )
        .expect("write");

        let entries = repository(&dir)
            .entries(&UsageQuery::default())
            .expect("a scan, not a failure");

        assert_eq!(total(&entries), 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_directory_holding_no_transcripts_is_skipped_rather_than_failing() {
        let projects = temp_dir("empty-project");
        std::fs::create_dir_all(projects.join("-home-ada-nothing-here")).expect("an empty project");
        let busy = projects.join("-home-ada-api");
        std::fs::create_dir_all(&busy).expect("a project");
        std::fs::write(
            busy.join("sid-1.jsonl"),
            response("sid-1", "msg_01", "req_01", at(0), 100),
        )
        .expect("write");

        let mut repository =
            FileSystemUsageRepository::new(FileSystemCatalog::rooted_at(projects.clone()));
        let entries = repository
            .entries(&UsageQuery::default())
            .expect("an empty directory is not a failure");

        assert_eq!(total(&entries), 100);
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn a_refusal_is_reported_alongside_the_entries_rather_than_filtered_with_them() {
        let dir = temp_dir("refusal");
        let resets = at(60).timestamp();
        std::fs::write(
            dir.join("a.jsonl"),
            format!(
                r#"{{"type":"assistant","timestamp":"{}","quotaLimits":{{"status":"rejected","resetsAt":{resets},"rateLimitType":"five_hour"}}}}"#,
                at(0).to_rfc3339()
            ),
        )
        .expect("write");

        let (entries, events) = repository(&dir)
            .entries_and_limit_events(&UsageQuery::default())
            .expect("a scan");

        assert!(entries.is_empty(), "a refused request was not charged for");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, WindowKind::Session);
        assert_eq!(events[0].resets_at.timestamp(), resets);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transcript_that_is_no_longer_listed_is_dropped_from_the_cache() {
        let dir = temp_dir("evict");
        let path = dir.join("a.jsonl");
        std::fs::write(&path, response("session-a", "msg_01", "req_01", at(0), 100))
            .expect("write");

        let mut repository = repository(&dir);
        repository.entries(&UsageQuery::default()).expect("a scan");
        assert_eq!(repository.cache.len(), 1);

        std::fs::remove_file(&path).expect("remove");
        repository
            .entries(&UsageQuery::default())
            .expect("a second scan");

        assert!(
            repository.cache.is_empty(),
            "a dashboard left running for a week must not hoard what has aged out"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
