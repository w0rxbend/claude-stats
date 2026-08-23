//! The usage-tracking use case: keep the account's rolling windows current.
//!
//! Separate from [`crate::application::monitor::Monitor`] because the two
//! answer different questions on different clocks. The monitor follows *one*
//! transcript and wants to notice a change within a frame. This follows *every*
//! transcript and only needs to be roughly right: a five-hour window does not
//! move perceptibly in thirty seconds, and scanning is the more expensive of
//! the two by far.
//!
//! It knows nothing about terminals, and nothing about the filesystem either --
//! the scanning happens behind [`AccountUsageReader`].

use std::time::{Duration, Instant};

use super::ports::{AccountUsageReader, Clock};
use crate::domain::limits::AccountUsage;

/// How often the transcripts are re-scanned.
///
/// The windows this feeds are five hours and seven days wide, so a reading
/// half a minute old is indistinguishable from a fresh one. Scanning harder
/// would cost real I/O to tell the user nothing new.
const SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Keeps an [`AccountUsage`] reading current.
pub struct UsageTracker {
    reader: Box<dyn AccountUsageReader>,
    clock: Box<dyn Clock>,
    usage: AccountUsage,
    last_scan: Option<Instant>,
    /// The most recent scan failure, kept so the dashboard can say the figures
    /// are stale rather than quietly showing old ones.
    last_error: Option<String>,
}

impl UsageTracker {
    /// A tracker that will scan on its first tick.
    ///
    /// Nothing is read yet, for the same reason the monitor attaches lazily: a
    /// dashboard should paint before it touches the disk.
    #[must_use]
    pub fn new(reader: Box<dyn AccountUsageReader>, clock: Box<dyn Clock>) -> Self {
        let now = clock.now();
        Self {
            reader,
            clock,
            usage: AccountUsage::empty(now),
            last_scan: None,
            last_error: None,
        }
    }

    /// The most recent reading.
    #[must_use]
    pub const fn usage(&self) -> &AccountUsage {
        &self.usage
    }

    /// Whether anything has been measured yet.
    #[must_use]
    pub fn has_measured(&self) -> bool {
        self.last_scan.is_some()
    }

    /// The most recent scan failure, if any.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Re-scans if enough time has passed. Returns whether it did.
    ///
    /// Safe to call on every frame: the interval check is the first thing it
    /// does, so a tick with nothing to do costs a clock read.
    pub fn tick(&mut self) -> bool {
        if let Some(last) = self.last_scan
            && last.elapsed() < SCAN_INTERVAL
        {
            return false;
        }
        self.scan();
        true
    }

    /// Re-scans now, whatever the interval says.
    ///
    /// This is what the manual refresh key is wired to.
    pub fn scan(&mut self) {
        self.last_scan = Some(Instant::now());
        match self.reader.usage(self.clock.now()) {
            Ok(usage) => {
                self.usage = usage;
                self.last_error = None;
            }
            // The previous reading is kept rather than blanked, for the same
            // reason a failed transcript re-read keeps its snapshot: numbers a
            // minute stale next to an error beat no numbers at all.
            Err(error) => self.last_error = Some(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use chrono::{DateTime, TimeZone, Utc};

    use super::*;
    use crate::domain::limits::{SessionContribution, UsagePoint};
    use crate::domain::money::Usd;
    use crate::domain::tokens::TokenUsage;

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    struct CountingReader {
        scans: Rc<Cell<u32>>,
        fails: bool,
    }

    impl AccountUsageReader for CountingReader {
        fn usage(&mut self, now: DateTime<Utc>) -> anyhow::Result<AccountUsage> {
            self.scans.set(self.scans.get() + 1);
            if self.fails {
                anyhow::bail!("cannot list the projects directory");
            }
            Ok(AccountUsage::measure(
                now,
                &[SessionContribution {
                    session_id: "a".to_owned(),
                    points: vec![UsagePoint {
                        at: now,
                        tokens: TokenUsage {
                            input: 500,
                            ..TokenUsage::ZERO
                        },
                        cost: Usd::new(2.0),
                    }],
                }],
                Vec::new(),
            ))
        }
    }

    fn tracker(fails: bool) -> (UsageTracker, Rc<Cell<u32>>) {
        let scans = Rc::new(Cell::new(0));
        let now = Utc
            .timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("a time");
        (
            UsageTracker::new(
                Box::new(CountingReader {
                    scans: Rc::clone(&scans),
                    fails,
                }),
                Box::new(FixedClock(now)),
            ),
            scans,
        )
    }

    #[test]
    fn nothing_is_read_until_the_first_tick() {
        let (tracker, scans) = tracker(false);
        assert_eq!(scans.get(), 0, "constructing must not touch the disk");
        assert!(!tracker.has_measured());
    }

    #[test]
    fn the_first_tick_scans_and_the_next_one_does_not() {
        let (mut tracker, scans) = tracker(false);

        assert!(tracker.tick(), "the first tick scans");
        assert_eq!(scans.get(), 1);
        assert_eq!(tracker.usage().session.tokens.total(), 500);

        assert!(!tracker.tick(), "the second is inside the interval");
        assert_eq!(scans.get(), 1);
    }

    #[test]
    fn a_manual_scan_ignores_the_interval() {
        let (mut tracker, scans) = tracker(false);
        tracker.tick();
        tracker.scan();
        assert_eq!(scans.get(), 2);
    }

    #[test]
    fn a_failed_scan_keeps_the_last_good_reading_and_reports_the_error() {
        let (mut tracker, _) = tracker(false);
        tracker.tick();
        assert_eq!(tracker.usage().session.tokens.total(), 500);

        // Swap in a reader that always fails, then force a scan.
        tracker.reader = Box::new(CountingReader {
            scans: Rc::new(Cell::new(0)),
            fails: true,
        });
        tracker.scan();

        assert_eq!(
            tracker.usage().session.tokens.total(),
            500,
            "the stale reading is kept"
        );
        assert_eq!(
            tracker.last_error(),
            Some("cannot list the projects directory")
        );
    }
}
