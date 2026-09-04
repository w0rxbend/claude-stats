//! The catalogue of Claude models: how big their context window is, and what
//! they cost.
//!
//! The transcript records a model as a free-form string such as
//! `"claude-opus-5"`. Both lookups below match a known key as a *substring* of
//! that string, so dated or vendor-prefixed variants (`"claude-opus-5"` on
//! Bedrock arrives as `"anthropic.claude-opus-5"`) resolve to the same entry.
//! Entries are ordered most-specific first, and the first match wins.

use std::fmt;

use super::money::RatePerMillionTokens;

/// The model string exactly as a transcript recorded it.
///
/// Kept as its own type, rather than a bare `String`, because it is a key
/// rather than prose: it is what the catalogue is searched with, what a
/// per-model report groups by, and what decides the price of every response
/// attributed to it. A report that grouped by the *display* name instead would
/// quietly merge two models the catalogue prices differently, and nothing in
/// the output would show it had happened.
///
/// The raw string is preserved rather than normalised. Deployments prefix it
/// (`anthropic.claude-opus-5` on Bedrock), and the catalogue already matches
/// on substrings, so normalising here would throw away information for no gain
/// and would have to be undone by anyone debugging what a transcript actually
/// said.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(String);

impl ModelId {
    /// The literal Claude Code writes in place of a model name when it
    /// answered on its own account rather than calling the API.
    pub const SYNTHETIC: &'static str = "<synthetic>";

    /// Wraps a recorded model string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The string itself, which is what the catalogue lookups take.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this names Claude Code's local stand-in rather than a real
    /// model.
    ///
    /// Claude Code writes `<synthetic>` when *it* produced the message --
    /// a refusal notice, an interrupt acknowledgement -- without asking the
    /// API. Nothing was sold, so nothing should be priced: its counters are
    /// all zero, and because the string matches no catalogue key it would
    /// otherwise be charged at the fallback rate as though it were an unknown
    /// paid model.
    #[must_use]
    pub fn is_synthetic(&self) -> bool {
        self.0 == Self::SYNTHETIC
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The five published prices for one model, in dollars per million tokens.
///
/// There are five rather than four because a cache write is not one price.
/// Anthropic sells two time-to-live options and charges differently for them:
/// a five-minute write costs 1.25x the input rate, a one-hour write costs 2x.
/// Collapsing them into a single "cache write" rate understates the bill by
/// 37.5% for any session that takes the longer lease, and Claude Code takes
/// the longer lease routinely -- so the distinction is not a rounding detail,
/// it is most of the error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    /// Fresh input tokens.
    pub input: RatePerMillionTokens,
    /// Tokens served from the prompt cache.
    pub cache_read: RatePerMillionTokens,
    /// Tokens written into the prompt cache with a five-minute lease.
    pub cache_write_5m: RatePerMillionTokens,
    /// Tokens written into the prompt cache with a one-hour lease.
    pub cache_write_1h: RatePerMillionTokens,
    /// Tokens the model generated.
    pub output: RatePerMillionTokens,
}

impl ModelPricing {
    /// A five-minute cache write costs this multiple of the input rate.
    const WRITE_5M_MULTIPLIER: f64 = 1.25;
    /// A one-hour cache write costs this multiple of the input rate.
    const WRITE_1H_MULTIPLIER: f64 = 2.0;
    /// What a cache read costs, as a multiple of the input rate, for every
    /// model up to and including the 5.0 generation.
    const READ_MULTIPLIER: f64 = 0.1;
    /// The cheaper read multiple introduced with the 5.1 generation.
    const CHEAP_READ_MULTIPLIER: f64 = 0.025;

    /// Derives the whole price sheet from the two headline numbers.
    ///
    /// The cache rates are fixed multiples of the input rate, so spelling all
    /// five out per model would be five chances to typo instead of two.
    ///
    /// Public because a user-written price override states only the two
    /// headline figures too -- Anthropic publishes those and nothing else --
    /// and an override file that had to spell out all five rates would be a
    /// second place for the multiples to be got wrong. Note that an override
    /// for a model the catalogue already knows goes through
    /// [`Self::with_headline`] instead, which keeps that model's own read
    /// multiple rather than assuming this one.
    #[must_use]
    pub const fn from_headline(input: f64, output: f64) -> Self {
        Self::with_read_multiplier(input, output, Self::READ_MULTIPLIER)
    }

    /// These rates restated around a different pair of headline figures.
    ///
    /// The three cache rates are re-derived from the multiples *this* row
    /// already uses rather than from the standard ones, and that distinction
    /// is the whole reason the method exists. The standard tenth-of-input
    /// cache read is not universal: the 5.1 generation reads at a fortieth.
    /// Somebody correcting Fable 5.1's headline price through
    /// [`Self::from_headline`] would get the tenth back and so quadruple what
    /// a cache read costs -- and since real traffic is overwhelmingly cache
    /// reads, that is most of the bill rather than a rounding detail.
    ///
    /// A row whose input rate is zero has no multiples worth preserving --
    /// every cache rate derived from it would be zero, or worse, not a number
    /// at all -- so the standard multiples are used for it instead.
    #[must_use]
    pub fn with_headline(self, input: f64, output: f64) -> Self {
        let previous = self.input.dollars_per_million();
        if previous <= 0.0 {
            return Self::from_headline(input, output);
        }
        let rescaled = |rate: RatePerMillionTokens| {
            RatePerMillionTokens::new(input * rate.dollars_per_million() / previous)
        };
        Self {
            input: RatePerMillionTokens::new(input),
            cache_read: rescaled(self.cache_read),
            cache_write_5m: rescaled(self.cache_write_5m),
            cache_write_1h: rescaled(self.cache_write_1h),
            output: RatePerMillionTokens::new(output),
        }
    }

    /// The same, for the 5.1-generation models whose reads are four times
    /// cheaper.
    ///
    /// The tenth-of-input read price held for every model Anthropic had
    /// shipped until the 5.1 generation cut it to a fortieth. Since real
    /// traffic is overwhelmingly cache reads, applying the old multiple to a
    /// 5.1 model overstates its bill several-fold -- which is why this is a
    /// separate constructor rather than an argument callers might forget.
    const fn from_headline_with_cheap_reads(input: f64, output: f64) -> Self {
        Self::with_read_multiplier(input, output, Self::CHEAP_READ_MULTIPLIER)
    }

    const fn with_read_multiplier(input: f64, output: f64, read: f64) -> Self {
        Self {
            input: RatePerMillionTokens::new(input),
            cache_read: RatePerMillionTokens::new(input * read),
            cache_write_5m: RatePerMillionTokens::new(input * Self::WRITE_5M_MULTIPLIER),
            cache_write_1h: RatePerMillionTokens::new(input * Self::WRITE_1H_MULTIPLIER),
            output: RatePerMillionTokens::new(output),
        }
    }
}

/// One row of the catalogue.
struct CatalogEntry {
    /// Matched as a substring of the transcript's model string.
    key: &'static str,
    /// Short name for the dashboard header, e.g. `"Opus 5"`.
    display: &'static str,
    /// Context window in tokens.
    context_window: u64,
    pricing: ModelPricing,
}

/// One catalogue row, as seen from outside the module.
///
/// [`CatalogEntry`] itself stays private so that no caller can build a row,
/// reorder the table or hold a reference into it -- the ordering is
/// load-bearing and the table is the single source of truth for what a model
/// costs. This is the read-only projection of a row: enough to print a pricing
/// table or to walk every known model in a test, and not enough to change one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CatalogRow {
    /// The key, which is matched as a substring of a transcript's model
    /// string.
    pub id: &'static str,
    /// Short name for a header or a table, e.g. `"Opus 5"`.
    pub display: &'static str,
    /// Context window in tokens.
    pub context_window: u64,
    /// The five published rates for this model.
    pub pricing: ModelPricing,
}

/// Read-only lookup over the known Claude models.
///
/// A Registry in Fowler's sense: a well-known object that answers questions
/// about globally shared reference data. It fits here because the price sheet
/// is not per-session state and has no lifecycle -- it is a fact about the
/// world, identical for every caller, so threading it through every function
/// that needs a price would be ceremony that buys nothing. What keeps that
/// honest is that the registry is strictly read-only: there is no way to add,
/// remove or reorder a row at runtime, so no part of the program can change
/// what another part will be charged.
pub struct ModelCatalog;

impl ModelCatalog {
    /// The window assumed for a model that is not in the catalogue.
    ///
    /// 200k is the conservative choice: every Claude model has at least this
    /// much, so an unknown model's context bar reads *pessimistically* (too
    /// full) rather than optimistically (plenty of room left when there is
    /// none).
    pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

    /// Head-room Claude Code keeps free before it auto-compacts.
    ///
    /// Compaction does not wait for the window to be literally full; it fires
    /// once the remaining space drops below this buffer. The dashboard's
    /// "compaction distance" is measured against this threshold, not against
    /// the raw window size, so it predicts the event that actually happens.
    pub const COMPACTION_BUFFER: u64 = 33_000;

    /// Prices used when the model string matches nothing known.
    ///
    /// The middle of the published range: an unknown model is as likely to be
    /// cheaper than Opus as it is to be dearer than Haiku, and a mid estimate
    /// is wrong by less than either extreme. What it is emphatically not is
    /// free: a model nobody has catalogued yet is still being charged for, and
    /// pricing it at nothing would quietly subtract a whole model from every
    /// total on the machine of whoever adopts it first.
    ///
    /// Public so that [`crate::domain::pricing::PriceSheet`], which is built
    /// from this table, charges an unknown model the same figure the catalogue
    /// would have -- one fallback rate, not two that can drift apart.
    pub const FALLBACK_PRICING: ModelPricing = ModelPricing::from_headline(3.0, 15.0);

    /// Every model the dashboard knows about, most specific key first.
    ///
    /// Order is load-bearing, because a key matches as a *substring*: the 5.1
    /// entries must precede their 5.0 namesakes, or `claude-fable-5-1` would
    /// match `claude-fable-5` and be priced with reads four times dearer than
    /// it actually charges.
    const ENTRIES: &'static [CatalogEntry] = &[
        CatalogEntry {
            key: "claude-fable-5-1",
            display: "Fable 5.1",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline_with_cheap_reads(10.0, 50.0),
        },
        CatalogEntry {
            key: "claude-mythos-5-1",
            display: "Mythos 5.1",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline_with_cheap_reads(10.0, 50.0),
        },
        CatalogEntry {
            key: "claude-fable-5",
            display: "Fable 5",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(10.0, 50.0),
        },
        CatalogEntry {
            key: "claude-mythos",
            display: "Mythos 5",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(10.0, 50.0),
        },
        CatalogEntry {
            key: "claude-opus-5",
            display: "Opus 5",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(5.0, 25.0),
        },
        CatalogEntry {
            key: "claude-opus-4-8",
            display: "Opus 4.8",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(5.0, 25.0),
        },
        CatalogEntry {
            key: "claude-opus-4-7",
            display: "Opus 4.7",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(5.0, 25.0),
        },
        CatalogEntry {
            key: "claude-opus-4-6",
            display: "Opus 4.6",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(5.0, 25.0),
        },
        CatalogEntry {
            key: "claude-opus-4-5",
            display: "Opus 4.5",
            context_window: 200_000,
            pricing: ModelPricing::from_headline(5.0, 25.0),
        },
        // Sonnet 5 was to rise to $3/$15 on 2026-09-01, and this entry once
        // listed that rate so the figure would stay right when it did. The
        // increase was cancelled and $2/$10 is now the standard price, so the
        // anticipated rate has become simply wrong.
        CatalogEntry {
            key: "claude-sonnet-5",
            display: "Sonnet 5",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(2.0, 10.0),
        },
        CatalogEntry {
            key: "claude-sonnet-4-6",
            display: "Sonnet 4.6",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(3.0, 15.0),
        },
        CatalogEntry {
            key: "claude-sonnet-4",
            display: "Sonnet 4",
            context_window: 200_000,
            pricing: ModelPricing::from_headline(3.0, 15.0),
        },
        CatalogEntry {
            key: "claude-haiku-4-5",
            display: "Haiku 4.5",
            context_window: 200_000,
            pricing: ModelPricing::from_headline(1.0, 5.0),
        },
        CatalogEntry {
            key: "claude-haiku-3-5",
            display: "Haiku 3.5",
            context_window: 200_000,
            pricing: ModelPricing::from_headline(0.8, 4.0),
        },
    ];

    fn lookup(model_id: &str) -> Option<&'static CatalogEntry> {
        Self::ENTRIES.iter().find(|e| model_id.contains(e.key))
    }

    /// Every known model, in the order the table declares them.
    ///
    /// Declaration order is handed out rather than a sorted or arbitrary one
    /// because it *is* the matching rule: keys match as substrings and the
    /// first hit wins, so the order is the difference between `claude-fable-5-1`
    /// being priced as itself and being priced as `claude-fable-5`. A caller
    /// walking this iterator sees precisely the sequence a lookup walks, which
    /// is what lets a test prove the table has not been shuffled.
    pub fn entries() -> impl Iterator<Item = CatalogRow> {
        Self::ENTRIES.iter().map(|e| CatalogRow {
            id: e.key,
            display: e.display,
            context_window: e.context_window,
            pricing: e.pricing,
        })
    }

    /// The context window for `model_id`, in tokens.
    #[must_use]
    pub fn context_window_for(model_id: &str) -> u64 {
        Self::lookup(model_id).map_or(Self::DEFAULT_CONTEXT_WINDOW, |e| e.context_window)
    }

    /// The price sheet for `model_id`.
    #[must_use]
    pub fn pricing_for(model_id: &str) -> ModelPricing {
        Self::lookup(model_id).map_or(Self::FALLBACK_PRICING, |e| e.pricing)
    }

    /// A short human-facing name for `model_id`.
    ///
    /// Falls back to the raw string with the `claude-` prefix trimmed, so an
    /// unrecognised model still shows something recognisable in the header
    /// rather than the word "unknown".
    #[must_use]
    pub fn display_name_for(model_id: &str) -> String {
        Self::lookup(model_id).map_or_else(
            || Self::fallback_display_name(model_id),
            |e| e.display.to_owned(),
        )
    }

    /// What to call a model no table has a row for.
    ///
    /// Named and public rather than inlined into [`Self::display_name_for`]
    /// because [`crate::domain::pricing::PriceSheet`] has to answer the same
    /// question and must answer it the same way. Two independent spellings of
    /// "what do we call an unrecognised model" would show up as the dashboard
    /// header and the price table disagreeing about a model neither of them
    /// knows, which is a confusing way to learn that a new model has shipped.
    #[must_use]
    pub fn fallback_display_name(model_id: &str) -> String {
        let trimmed = model_id.trim_start_matches("claude-");
        if trimmed.is_empty() {
            "unknown".to_owned()
        } else {
            trimmed.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vendor_prefixed_model_id_still_resolves() {
        assert_eq!(
            ModelCatalog::context_window_for("anthropic.claude-opus-5"),
            1_000_000
        );
    }

    #[test]
    fn the_more_specific_key_wins_over_the_broader_one() {
        // "claude-sonnet-4-6" is listed before "claude-sonnet-4", and only the
        // former has the 1M window.
        assert_eq!(
            ModelCatalog::context_window_for("claude-sonnet-4-6"),
            1_000_000
        );
        assert_eq!(ModelCatalog::context_window_for("claude-sonnet-4"), 200_000);
    }

    #[test]
    fn an_unknown_model_gets_the_conservative_window() {
        assert_eq!(
            ModelCatalog::context_window_for("some-future-model"),
            ModelCatalog::DEFAULT_CONTEXT_WINDOW
        );
    }

    #[test]
    fn cache_rates_are_derived_from_the_input_rate() {
        let p = ModelCatalog::pricing_for("claude-opus-5");
        assert!((p.cache_read.dollars_per_million() - 0.5).abs() < 1e-9);
        assert!((p.cache_write_5m.dollars_per_million() - 6.25).abs() < 1e-9);
        assert!((p.cache_write_1h.dollars_per_million() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn every_catalogue_key_resolves_to_its_own_entry_and_no_other() {
        // The table is ordered most-specific first because a key matches as a
        // substring, so a row inserted in the wrong place does not fail
        // loudly -- it silently prices one model at another's rate. This walks
        // every row and insists it still resolves to itself, which is the only
        // cheap way to keep that ordering honest as rows are added.
        for row in ModelCatalog::entries() {
            assert_eq!(
                ModelCatalog::context_window_for(row.id),
                row.context_window,
                "{} resolved to a different row's context window",
                row.id
            );
            assert_eq!(
                ModelCatalog::pricing_for(row.id),
                row.pricing,
                "{} was priced from a different row",
                row.id
            );
            assert_eq!(
                ModelCatalog::display_name_for(row.id),
                row.display,
                "{} was named after a different row",
                row.id
            );
        }
    }

    #[test]
    fn restating_a_row_around_a_new_headline_price_keeps_its_own_multiples() {
        // The 5.1 generation reads at a fortieth of its input rate; everything
        // before it reads at a tenth. Anyone correcting a headline price is
        // correcting one number, not opting the model back into the older
        // generation's cache economics.
        let cheap_reads = ModelCatalog::pricing_for("claude-fable-5-1").with_headline(12.0, 60.0);
        assert!(
            (cheap_reads.cache_read.dollars_per_million() - 0.3).abs() < 1e-9,
            "a fortieth of the new input rate"
        );
        assert!((cheap_reads.cache_write_5m.dollars_per_million() - 15.0).abs() < 1e-9);
        assert!((cheap_reads.cache_write_1h.dollars_per_million() - 24.0).abs() < 1e-9);
        assert!((cheap_reads.output.dollars_per_million() - 60.0).abs() < 1e-9);

        // And a model on the usual multiples comes out exactly as
        // `from_headline` would have built it, so the two ways of stating a
        // price cannot disagree for the models where both apply.
        assert_eq!(
            ModelCatalog::pricing_for("claude-opus-5").with_headline(7.5, 37.5),
            ModelPricing::from_headline(7.5, 37.5)
        );
    }

    #[test]
    fn a_row_with_no_input_rate_falls_back_to_the_usual_multiples() {
        // Nothing in the catalogue is free, but a row that were would have no
        // multiples to preserve: every rate derived from a zero input rate is
        // either zero or not a number at all.
        let free = ModelPricing::from_headline(0.0, 0.0).with_headline(4.0, 20.0);
        assert_eq!(free, ModelPricing::from_headline(4.0, 20.0));
    }

    #[test]
    fn a_five_minute_and_a_one_hour_write_are_still_priced_apart() {
        // A single blended cache-write rate charged the cheaper lease for
        // both and understated every session that took the longer one. Claude
        // Code takes the longer one routinely, so this was most of a
        // several-fold error rather than a rounding detail. The two rates are
        // pinned here, at the top of the price sheet, so that a later
        // simplification of `ModelPricing` cannot quietly reintroduce it.
        let opus = ModelCatalog::pricing_for("claude-opus-5");
        assert!(
            (opus.cache_write_1h.dollars_per_million() - 10.0).abs() < 1e-9,
            "a one-hour write costs twice the input rate"
        );
        assert!(
            (opus.cache_write_5m.dollars_per_million() - 6.25).abs() < 1e-9,
            "a five-minute write costs 1.25x the input rate"
        );
    }

    #[test]
    fn claude_codes_own_stand_in_is_recognised_as_not_a_real_model() {
        assert!(ModelId::new("<synthetic>").is_synthetic());
        assert!(!ModelId::new("claude-opus-5").is_synthetic());
        assert_eq!(ModelId::new("claude-opus-5").as_str(), "claude-opus-5");
        assert_eq!(ModelId::new("claude-opus-5").to_string(), "claude-opus-5");
    }

    #[test]
    fn an_unknown_model_still_gets_a_readable_name() {
        assert_eq!(
            ModelCatalog::display_name_for("claude-zephyr-9"),
            "zephyr-9"
        );
    }
}
