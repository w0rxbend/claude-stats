//! The price sheet this run will use, composed once.

use anyhow::Result;

use super::overrides::PriceOverrides;
use crate::application::ports::PriceSheetSource;
use crate::domain::pricing::PriceSheet;

/// Composes the run's price sheet from the compiled-in rates and the user's
/// optional corrections.
///
/// The only implementation of [`PriceSheetSource`] there is, and named for
/// what it is built on rather than for how it fetches, because it does not
/// fetch: see the module documentation in [`super`] for why the tool never
/// opens a socket.
///
/// The composed sheet is memoised. Two things follow from that, and both
/// matter. Reading the override file once per run rather than once per report
/// keeps the cost of asking honest -- but more importantly, it guarantees that
/// every report in one run is priced by the *same* sheet. Re-reading the file
/// each time would mean a user editing it while a dashboard is open could get
/// two panels of the same screen costed at two different rates, with nothing
/// to explain the difference.
pub struct BuiltinPriceSource {
    overrides: PriceOverrides,
    /// The composed sheet, once somebody has asked for it. Fowler's Lazy Load:
    /// a run that never prices anything -- `claude-stats sessions`, say --
    /// never touches the user's file at all.
    composed: Option<PriceSheet>,
}

impl BuiltinPriceSource {
    /// A source that looks for corrections in the usual place.
    ///
    /// # Errors
    ///
    /// Returns an error when the config directory cannot be determined. Note
    /// that this is *not* where a missing file is noticed: nothing is read
    /// until [`PriceSheetSource::sheet`] is called.
    pub fn from_config_dir() -> Result<Self> {
        Ok(Self::over(PriceOverrides::from_config_dir()?))
    }

    /// A source that reads corrections from a particular gateway.
    ///
    /// The seam the tests use, and the seam a future gateway would plug into.
    #[must_use]
    pub const fn over(overrides: PriceOverrides) -> Self {
        Self {
            overrides,
            composed: None,
        }
    }
}

impl PriceSheetSource for BuiltinPriceSource {
    fn sheet(&mut self) -> Result<PriceSheet> {
        if let Some(sheet) = &self.composed {
            return Ok(sheet.clone());
        }
        // The built-in sheet is the base in both branches, so a user's file
        // never has to be complete: correcting one rate leaves the other
        // thirteen models exactly as this release shipped them.
        let composed = match self.overrides.load()? {
            Some(corrections) => PriceSheet::builtin().overlaid_with(corrections),
            None => PriceSheet::builtin(),
        };
        self.composed = Some(composed.clone());
        Ok(composed)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::model::ModelId;
    use crate::domain::pricing::Provenance;

    /// A directory that deletes itself when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "claude-stats-source-{name}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("a writable temporary directory");
            Self(dir)
        }

        fn path(&self, file: &str) -> PathBuf {
            self.0.join(file)
        }

        fn holding(&self, contents: &str) -> PathBuf {
            let path = self.path("prices.json");
            std::fs::write(&path, contents).expect("a writable file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_override_file_yields_the_builtin_sheet_rather_than_an_error() {
        // Almost nobody will ever write one. Treating its absence as a problem
        // would make the normal case an error path, and would mean a tool that
        // refuses to tell you what you spent because you have not written a
        // configuration file you have never heard of.
        let dir = TempDir::new("absent");
        let mut source =
            BuiltinPriceSource::over(PriceOverrides::at(dir.path("nothing-here.json")));

        let sheet = source.sheet().expect("no file is not a failure");

        assert_eq!(sheet, PriceSheet::builtin());
        assert_eq!(sheet.provenance(), &Provenance::Builtin);
    }

    #[test]
    fn a_correction_is_laid_over_the_builtin_rates_rather_than_replacing_them() {
        let dir = TempDir::new("overlay");
        let path =
            dir.holding(r#"{ "models": { "claude-opus-5": { "input": 9.0, "output": 45.0 } } }"#);
        let mut source = BuiltinPriceSource::over(PriceOverrides::at(path.clone()));

        let sheet = source.sheet().expect("a well-formed file");

        let opus = sheet
            .pricing_for(&ModelId::new("claude-opus-5"))
            .expect("the corrected row");
        assert!((opus.input.dollars_per_million() - 9.0).abs() < 1e-9);

        let haiku = sheet
            .pricing_for(&ModelId::new("claude-haiku-4-5"))
            .expect("a model the user said nothing about");
        assert!(
            (haiku.input.dollars_per_million() - 1.0).abs() < 1e-9,
            "the other thirteen models ship as this release shipped them"
        );
        assert_eq!(
            sheet.rows().count(),
            PriceSheet::builtin().rows().count(),
            "a correction replaces a row rather than adding one"
        );
        assert_eq!(
            sheet.provenance(),
            &Provenance::Overridden {
                source: path.display().to_string(),
            }
        );
    }

    #[test]
    fn the_override_file_is_read_once_however_often_the_sheet_is_asked_for() {
        // Every report in one run must be priced by the same sheet. Re-reading
        // the file each time would let a user editing it while a dashboard is
        // open see two panels of one screen costed at two different rates.
        let dir = TempDir::new("memoised");
        let path =
            dir.holding(r#"{ "models": { "claude-opus-5": { "input": 9.0, "output": 45.0 } } }"#);
        let mut source = BuiltinPriceSource::over(PriceOverrides::at(path.clone()));

        let first = source.sheet().expect("a well-formed file");
        std::fs::write(&path, "not json at all").expect("a writable file");
        let second = source.sheet().expect("the memoised sheet, not a re-read");

        assert_eq!(first, second);
    }

    #[test]
    fn a_malformed_file_is_refused_rather_than_silently_replaced_by_the_builtin_sheet() {
        let dir = TempDir::new("refused");
        let path = dir.holding("{ this is not json }");
        let mut source = BuiltinPriceSource::over(PriceOverrides::at(path));

        let error = source.sheet().expect_err("a malformed file is refused");
        assert!(format!("{error:#}").contains("prices.json"));
    }
}
