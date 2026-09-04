//! The user's own price corrections, read from an optional file.
//!
//! A Gateway in Fowler's sense: one object that owns every detail of talking
//! to an external resource -- where the file lives, what its JSON looks like,
//! which of its fields may be left out -- so that nothing above it knows the
//! resource is a file at all. What comes back is a
//! [`PriceSheet`], the same type the compiled-in rates arrive as, which is
//! what lets the two be composed with a single call.
//!
//! # The file
//!
//! `${XDG_CONFIG_HOME:-~/.config}/claude-stats/prices.json`, shaped like this:
//!
//! ```json
//! {
//!   "models": {
//!     "claude-opus-5": {
//!       "input": 5.0,
//!       "output": 25.0,
//!       "cache_read": 0.5,
//!       "cache_write_5m": 6.25,
//!       "cache_write_1h": 10.0,
//!       "context_window": 1000000,
//!       "display": "Opus 5"
//!     }
//!   }
//! }
//! ```
//!
//! Only `input` and `output` are required, because those are the two figures
//! Anthropic publishes. Everything else is defaulted from the multiples of the
//! input rate that the catalogue's own row for that id already uses -- for
//! almost every model a cache read at a tenth, a five-minute write at 1.25x
//! and a one-hour write at 2x, but a *fortieth* for the read on the 5.1
//! generation. Taking the multiples from the row rather than from a constant
//! is what stops a user who corrects Fable 5.1's headline price from silently
//! quadrupling what its cache reads cost, which on traffic that is
//! overwhelmingly cache reads would be most of their bill. An id the catalogue
//! has never heard of gets the usual multiples, which is all anyone knows
//! about it.
//!
//! So the common case -- correcting a headline rate that changed between
//! releases -- is two numbers rather than five.
//!
//! # Missing versus malformed
//!
//! A missing file is `Ok(None)`. Almost nobody will ever write one, and
//! treating its absence as a problem would make the normal case an error path.
//!
//! A file that exists but cannot be parsed *is* an error, named by path and
//! carrying the parse failure. The alternative -- shrugging and using the
//! built-in rates -- is worse than refusing: somebody sat down and wrote that
//! file on purpose, and a report that ignored it would go on printing figures
//! they believe they have corrected. A typo in a key is caught for the same
//! reason: unknown fields are rejected rather than skipped, so `cache_write5m`
//! is a message about a misspelling instead of a rate that silently did
//! nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::domain::model::{CatalogRow, ModelCatalog};
use crate::domain::money::RatePerMillionTokens;
use crate::domain::pricing::{PriceRow, PriceSheet, Provenance};

/// The whole override file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverrideFile {
    /// One entry per model id, keyed exactly as the sheet matches: a substring
    /// of whatever the transcript recorded.
    ///
    /// A [`BTreeMap`] rather than a hash map so that two runs over the same
    /// file compose the sheet in the same order. Hash-map order would leak
    /// into which of two overlapping override ids is reached first, which is
    /// a figure that changes between runs for no reason a user could explain.
    #[serde(default)]
    models: BTreeMap<String, ModelOverride>,
}

/// One model's corrected rates, as the user wrote them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelOverride {
    /// Dollars per million fresh input tokens. Required.
    input: f64,
    /// Dollars per million generated tokens. Required.
    output: f64,
    /// Dollars per million tokens served from the cache. Defaults to whatever
    /// multiple of the input rate the catalogue's row for this id uses: a
    /// tenth for most models, a fortieth for the 5.1 generation.
    cache_read: Option<f64>,
    /// Dollars per million tokens written with a five-minute lease. Defaults
    /// to 1.25x `input`.
    cache_write_5m: Option<f64>,
    /// Dollars per million tokens written with a one-hour lease. Defaults to
    /// 2x `input`.
    cache_write_1h: Option<f64>,
    /// Context window in tokens. Defaults to the window of the catalogue row
    /// this id *is*, and to 200k for an id the catalogue has no row for --
    /// the same conservative guess made everywhere else, so an unknown model's
    /// context bar reads too full rather than promising head-room that is not
    /// there.
    context_window: Option<u64>,
    /// Short name for a header or a table. Defaults to the catalogue's name
    /// for this id, or the id with `claude-` trimmed off.
    display: Option<String>,
}

/// The catalogue row this override id names outright, if there is one.
///
/// Deliberately an exact comparison where [`ModelCatalog::display_name_for`]
/// and [`ModelCatalog::context_window_for`] match on substrings. Those two
/// answer a different question -- "which row would price a transcript that
/// said this" -- and asking them here gives an override adding
/// `claude-opus-5-turbo`, a model nothing in the catalogue knows, the name and
/// the window of plain Opus 5. The price table would then show two rows both
/// called "Opus 5" at two different prices, which tells a reader nothing about
/// which is which.
fn catalogued_as(id: &str) -> Option<CatalogRow> {
    ModelCatalog::entries().find(|row| row.id == id)
}

impl ModelOverride {
    /// The row this override contributes for `id`.
    ///
    /// Built by restating the catalogue's own row for `id` around the two
    /// headline figures the user gave, then replacing only the rates they
    /// actually wrote. Going through
    /// [`ModelPricing::with_headline`](crate::domain::model::ModelPricing::with_headline) rather
    /// than spelling the multiples out again means the defaults here cannot
    /// drift away from the ones the catalogue uses -- including the fortieth
    /// that the 5.1 generation reads at, which a fixed tenth would quietly
    /// undo.
    fn into_row(self, id: &str) -> PriceRow {
        let mut pricing = ModelCatalog::pricing_for(id).with_headline(self.input, self.output);
        if let Some(rate) = self.cache_read {
            pricing.cache_read = RatePerMillionTokens::new(rate);
        }
        if let Some(rate) = self.cache_write_5m {
            pricing.cache_write_5m = RatePerMillionTokens::new(rate);
        }
        if let Some(rate) = self.cache_write_1h {
            pricing.cache_write_1h = RatePerMillionTokens::new(rate);
        }
        let catalogued = catalogued_as(id);
        PriceRow {
            id: id.to_owned(),
            display: self.display.unwrap_or_else(|| {
                catalogued.map_or_else(
                    || ModelCatalog::fallback_display_name(id),
                    |row| row.display.to_owned(),
                )
            }),
            context_window: self.context_window.unwrap_or_else(|| {
                catalogued.map_or(ModelCatalog::DEFAULT_CONTEXT_WINDOW, |row| {
                    row.context_window
                })
            }),
            pricing,
        }
    }
}

/// The user's price corrections, wherever they keep them.
#[derive(Debug, Clone)]
pub struct PriceOverrides {
    path: PathBuf,
}

impl PriceOverrides {
    /// The directory the file lives in, under whichever config root applies.
    const DIRECTORY: &'static str = "claude-stats";
    /// The file itself.
    const FILE: &'static str = "prices.json";

    /// Points at `${XDG_CONFIG_HOME:-~/.config}/claude-stats/prices.json`.
    ///
    /// `XDG_CONFIG_HOME` is honoured because a user who has relocated their
    /// configuration has done so precisely so that tools stop writing to and
    /// reading from `~/.config`, and one that ignored it would look in a
    /// directory they have deliberately emptied.
    ///
    /// # Errors
    ///
    /// Returns an error only when `XDG_CONFIG_HOME` is unset *and* the home
    /// directory cannot be determined, which on a normal machine means
    /// something is badly wrong with the environment.
    pub fn from_config_dir() -> Result<Self> {
        let config_dir = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => dirs::home_dir()
                .context("cannot determine the home directory")?
                .join(".config"),
        };
        Ok(Self::at(config_dir.join(Self::DIRECTORY).join(Self::FILE)))
    }

    /// Points at an arbitrary file. Used by the tests.
    #[must_use]
    pub const fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where this gateway is looking.
    ///
    /// Public so that an error a user has to act on can name the file they
    /// need to go and fix.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The user's corrections as a sheet, or `None` if they have written none.
    ///
    /// The returned sheet is *only* the override rows; it is meant to be laid
    /// over [`PriceSheet::builtin`] rather than used on its own, and it carries
    /// the [`Provenance::Overridden`] that the composed sheet will inherit.
    ///
    /// A file that exists but lists no models still returns `Some`. The rates
    /// are unchanged, but the run did consult the user's file, and a footer
    /// that hid that would be hiding exactly the thing the provenance is there
    /// to show -- not least because the file will not stay empty for ever.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read, or exists and
    /// is not a valid override file. Both name the path, because the only
    /// useful response to either is to go and look at it.
    pub fn load(&self) -> Result<Option<PriceSheet>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            // The overwhelmingly common case, and not a problem: almost nobody
            // needs to correct a published rate.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("cannot read the price overrides at {}", self.path.display())
                });
            }
        };

        let file: OverrideFile = serde_json::from_str(&text).with_context(|| {
            format!("{} is not a valid price override file", self.path.display())
        })?;

        let rows = file
            .models
            .into_iter()
            .map(|(id, model)| model.into_row(&id))
            .collect();
        Ok(Some(PriceSheet::from_rows(
            rows,
            Provenance::Overridden {
                source: self.path.display().to_string(),
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::model::ModelId;

    /// A directory that deletes itself, so a test can write a real file
    /// without leaving one behind. The process id and a counter keep two tests
    /// running at once out of each other's way.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "claude-stats-prices-{name}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("a writable temporary directory");
            Self(dir)
        }

        /// Writes `contents` to `prices.json` inside the directory and returns
        /// a gateway pointed at it.
        fn holding(&self, contents: &str) -> PriceOverrides {
            let path = self.0.join("prices.json");
            std::fs::write(&path, contents).expect("a writable file");
            PriceOverrides::at(path)
        }

        fn missing_file(&self) -> PriceOverrides {
            PriceOverrides::at(self.0.join("nothing-here.json"))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_malformed_override_file_is_refused_by_name_rather_than_quietly_ignored() {
        // Somebody sat down and wrote this file on purpose. Shrugging and using
        // the built-in rates would leave them reading figures they believe
        // they have corrected, with nothing on screen to say otherwise.
        let dir = TempDir::new("malformed");
        let overrides = dir.holding("{ \"models\": { \"claude-opus-5\": { \"input\": ");

        let error = overrides.load().expect_err("a truncated file is refused");
        let message = format!("{error:#}");
        assert!(
            message.contains("prices.json"),
            "the message must name the file to go and fix: {message}"
        );
        assert!(
            message.contains("not a valid price override file"),
            "and say what was wrong with it: {message}"
        );
    }

    #[test]
    fn a_misspelled_rate_is_a_refusal_rather_than_a_setting_that_does_nothing() {
        // `cache_write5m` is a plausible typo for `cache_write_5m`. Skipping
        // unknown fields would leave the user with a file that looks right and
        // changes nothing.
        let dir = TempDir::new("typo");
        let overrides = dir.holding(
            r#"{ "models": { "claude-opus-5": { "input": 5.0, "output": 25.0, "cache_write5m": 1.0 } } }"#,
        );

        let error = overrides.load().expect_err("an unknown field is refused");
        let message = format!("{error:#}");
        assert!(message.contains("cache_write5m"), "{message}");
    }

    #[test]
    fn the_rates_a_user_leaves_out_are_the_usual_multiples_of_the_input_rate() {
        // Anthropic publishes two figures, so correcting a rate should take
        // two figures. The other three follow from the multiples the catalogue
        // itself uses.
        let dir = TempDir::new("defaults");
        let overrides =
            dir.holding(r#"{ "models": { "claude-opus-5": { "input": 8.0, "output": 40.0 } } }"#);

        let sheet = overrides
            .load()
            .expect("a well-formed file")
            .expect("a file that is present");
        let pricing = sheet
            .pricing_for(&ModelId::new("claude-opus-5"))
            .expect("the overriding row");

        assert!((pricing.input.dollars_per_million() - 8.0).abs() < 1e-9);
        assert!((pricing.output.dollars_per_million() - 40.0).abs() < 1e-9);
        assert!(
            (pricing.cache_read.dollars_per_million() - 0.8).abs() < 1e-9,
            "a tenth of the input rate"
        );
        assert!(
            (pricing.cache_write_5m.dollars_per_million() - 10.0).abs() < 1e-9,
            "1.25x the input rate"
        );
        assert!(
            (pricing.cache_write_1h.dollars_per_million() - 16.0).abs() < 1e-9,
            "2x the input rate"
        );
        assert_eq!(
            sheet.display_name_for(&ModelId::new("claude-opus-5")),
            "Opus 5",
            "and the catalogue's own name, which the user did not have to retype"
        );
        assert_eq!(
            sheet.context_window_for(&ModelId::new("claude-opus-5")),
            1_000_000
        );
    }

    #[test]
    fn a_model_the_catalogue_has_never_heard_of_is_not_named_after_the_one_it_shadows() {
        // `claude-opus-5-turbo` contains `claude-opus-5`, so asking the
        // catalogue what to call it answers "Opus 5". The price table would
        // then carry two rows called "Opus 5" at two different prices with
        // nothing to tell them apart, and the context bar would promise the
        // 1M window of a model this is not.
        let dir = TempDir::new("unheard-of");
        let overrides = dir.holding(
            r#"{ "models": { "claude-opus-5-turbo": { "input": 20.0, "output": 100.0 } } }"#,
        );

        let sheet = overrides
            .load()
            .expect("a well-formed file")
            .expect("a file that is present");
        let turbo = ModelId::new("claude-opus-5-turbo");

        assert_eq!(sheet.display_name_for(&turbo), "opus-5-turbo");
        assert_eq!(
            sheet.context_window_for(&turbo),
            200_000,
            "the conservative guess, since nobody has said otherwise"
        );
    }

    #[test]
    fn correcting_a_headline_rate_keeps_that_models_own_cache_read_multiple() {
        // Fable 5.1 reads at a fortieth of its input rate where every model
        // before it read at a tenth. Defaulting the unwritten rates from the
        // tenth would turn a 20% correction of the headline price into a
        // near-fivefold rise in the price of a cache read -- and cache reads
        // are the overwhelming majority of real traffic, so that is most of
        // the bill rather than a rounding detail.
        let dir = TempDir::new("cheap-reads");
        let overrides = dir
            .holding(r#"{ "models": { "claude-fable-5-1": { "input": 12.0, "output": 60.0 } } }"#);

        let sheet = overrides
            .load()
            .expect("a well-formed file")
            .expect("a file that is present");
        let pricing = sheet
            .pricing_for(&ModelId::new("claude-fable-5-1"))
            .expect("the overriding row");

        assert!(
            (pricing.cache_read.dollars_per_million() - 0.3).abs() < 1e-9,
            "a fortieth of $12, not the tenth every earlier model reads at: got {}",
            pricing.cache_read.dollars_per_million()
        );
        assert!(
            (pricing.cache_write_5m.dollars_per_million() - 15.0).abs() < 1e-9,
            "the write multiples are the usual ones and stay so"
        );
        assert!((pricing.cache_write_1h.dollars_per_million() - 24.0).abs() < 1e-9);
    }

    #[test]
    fn every_field_a_user_does_write_is_taken_verbatim() {
        let dir = TempDir::new("explicit");
        let overrides = dir.holding(
            r#"{ "models": { "claude-zephyr-9": {
                "input": 4.0, "output": 20.0, "cache_read": 0.1,
                "cache_write_5m": 4.5, "cache_write_1h": 9.0,
                "context_window": 500000, "display": "Zephyr 9"
            } } }"#,
        );

        let sheet = overrides
            .load()
            .expect("a well-formed file")
            .expect("a file that is present");
        let zephyr = ModelId::new("claude-zephyr-9-20270101");
        let pricing = sheet.pricing_for(&zephyr).expect("the added row");

        assert!((pricing.cache_read.dollars_per_million() - 0.1).abs() < 1e-9);
        assert!((pricing.cache_write_5m.dollars_per_million() - 4.5).abs() < 1e-9);
        assert!((pricing.cache_write_1h.dollars_per_million() - 9.0).abs() < 1e-9);
        assert_eq!(sheet.context_window_for(&zephyr), 500_000);
        assert_eq!(sheet.display_name_for(&zephyr), "Zephyr 9");
    }

    #[test]
    fn a_file_nobody_wrote_is_not_a_problem_to_report() {
        let dir = TempDir::new("absent");
        assert!(
            dir.missing_file()
                .load()
                .expect("a missing file is not an error")
                .is_none()
        );
    }

    #[test]
    fn an_override_file_names_itself_so_a_surprising_figure_can_be_traced() {
        let dir = TempDir::new("provenance");
        let overrides = dir.holding(r#"{ "models": {} }"#);
        let sheet = overrides
            .load()
            .expect("a well-formed file")
            .expect("a file that is present even though it lists nothing");

        assert_eq!(
            sheet.provenance().to_string(),
            format!("overridden from {}", overrides.path().display())
        );
    }
}
