//! The catalogue of Claude models: how big their context window is, and what
//! they cost.
//!
//! The transcript records a model as a free-form string such as
//! `"claude-opus-5"`. Both lookups below match a known key as a *substring* of
//! that string, so dated or vendor-prefixed variants (`"claude-opus-5"` on
//! Bedrock arrives as `"anthropic.claude-opus-5"`) resolve to the same entry.
//! Entries are ordered most-specific first, and the first match wins.

use super::money::RatePerMillionTokens;

/// The four published prices for one model, in dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    /// Fresh input tokens.
    pub input: RatePerMillionTokens,
    /// Tokens served from the prompt cache -- about a tenth of `input`.
    pub cache_read: RatePerMillionTokens,
    /// Tokens written into the prompt cache -- about 1.25x `input`.
    pub cache_write: RatePerMillionTokens,
    /// Tokens the model generated.
    pub output: RatePerMillionTokens,
}

impl ModelPricing {
    /// Derives the whole price sheet from the two headline numbers.
    ///
    /// Anthropic's cache prices are a fixed multiple of the input price -- a
    /// read costs a tenth, a five-minute write costs 1.25x -- so spelling all
    /// four out per model would be four chances to typo instead of two.
    const fn from_headline(input: f64, output: f64) -> Self {
        Self {
            input: RatePerMillionTokens::new(input),
            cache_read: RatePerMillionTokens::new(input * 0.1),
            cache_write: RatePerMillionTokens::new(input * 1.25),
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

/// Read-only lookup over the known Claude models.
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
    /// Sonnet-tier, i.e. the middle of the range: an unknown model is as
    /// likely to be cheaper than Opus as it is to be dearer than Haiku, and a
    /// mid estimate is wrong by less than either extreme.
    const FALLBACK_PRICING: ModelPricing = ModelPricing::from_headline(3.0, 15.0);

    /// Every model the dashboard knows about, most specific key first.
    ///
    /// Sonnet 5 carries an introductory rate of $2/$10 through 2026-08-31; the
    /// durable standard rate is listed instead so the figure stays right once
    /// the introductory window closes.
    const ENTRIES: &'static [CatalogEntry] = &[
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
        CatalogEntry {
            key: "claude-sonnet-5",
            display: "Sonnet 5",
            context_window: 1_000_000,
            pricing: ModelPricing::from_headline(3.0, 15.0),
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
            || {
                let trimmed = model_id.trim_start_matches("claude-");
                if trimmed.is_empty() {
                    "unknown".to_owned()
                } else {
                    trimmed.to_owned()
                }
            },
            |e| e.display.to_owned(),
        )
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
        assert!((p.cache_write.dollars_per_million() - 6.25).abs() < 1e-9);
    }

    #[test]
    fn an_unknown_model_still_gets_a_readable_name() {
        assert_eq!(
            ModelCatalog::display_name_for("claude-zephyr-9"),
            "zephyr-9"
        );
    }
}
