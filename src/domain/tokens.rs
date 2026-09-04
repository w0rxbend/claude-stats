//! How a single API call's token usage is counted and priced.

use std::ops::{Add, AddAssign};

use super::model::ModelPricing;
use super::money::Usd;

/// The token counters the Claude API reports for one assistant response.
///
/// The transcript records these under `message.usage`. Every metric on the
/// dashboard -- context fill, cost, cache ratio -- is derived from these
/// numbers, so they are modelled once here rather than being passed around as
/// a loose tuple.
///
/// The distinction that matters most:
///
/// * `input` -- fresh tokens sent to the model, charged at full price.
/// * `cache_read` -- tokens served from the prompt cache, charged at a tenth
///   of the input price, or a fortieth on the newest models. A high share here
///   is *good*; it means the conversation prefix is being reused instead of
///   re-sent.
/// * `cache_write_5m` and `cache_write_1h` -- tokens written into the prompt
///   cache, charged at 1.25x and 2x the input price respectively. You pay a
///   premium now to get the cheap reads later, and the premium depends on how
///   long a lease you took.
/// * `output` -- tokens the model generated, the most expensive of them all.
///
/// The two write counters are kept apart rather than summed because they are
/// priced 60% apart, and which one a session uses is not a detail: a run that
/// takes one-hour leases throughout costs materially more than the same run
/// on five-minute leases, and a single blended counter cannot tell you that.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
}

impl TokenUsage {
    /// Nothing used yet.
    pub const ZERO: Self = Self {
        input: 0,
        cache_read: 0,
        cache_write_5m: 0,
        cache_write_1h: 0,
        output: 0,
    };

    /// Every token written into the cache, whichever lease it took.
    ///
    /// The two leases are priced differently but occupy the context window
    /// identically, so anything measuring *size* rather than *cost* wants
    /// this sum and should not have to know the split exists.
    #[must_use]
    pub const fn cache_creation(self) -> u64 {
        self.cache_write_5m + self.cache_write_1h
    }

    /// Every token involved in this call, whatever its kind.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.input + self.cache_read + self.cache_creation() + self.output
    }

    /// The tokens that were *sent to* the model, excluding what it generated.
    ///
    /// This is the number that fills the context window. Output tokens do not
    /// occupy the window of the call that produced them -- they occupy the
    /// window of the *next* call, by which point they have been folded into
    /// the input or cache counters.
    #[must_use]
    pub const fn prompt_tokens(self) -> u64 {
        self.input + self.cache_read + self.cache_creation()
    }

    /// The share of prompt tokens that came from the cache, in `0.0..=1.0`.
    ///
    /// Returns `None` when there were no prompt tokens at all, so the caller
    /// can render a placeholder instead of a misleading `0%`.
    #[must_use]
    pub fn cache_hit_ratio(self) -> Option<f64> {
        let prompt = self.prompt_tokens();
        if prompt == 0 {
            return None;
        }
        Some(self.cache_read as f64 / prompt as f64)
    }

    /// What this usage costs at the given model's published prices.
    #[must_use]
    pub fn cost(self, pricing: ModelPricing) -> Usd {
        pricing.input.charge_for(self.input)
            + pricing.cache_read.charge_for(self.cache_read)
            + pricing.cache_write_5m.charge_for(self.cache_write_5m)
            + pricing.cache_write_1h.charge_for(self.cache_write_1h)
            + pricing.output.charge_for(self.output)
    }
}

impl Add for TokenUsage {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            input: self.input + other.input,
            cache_read: self.cache_read + other.cache_read,
            cache_write_5m: self.cache_write_5m + other.cache_write_5m,
            cache_write_1h: self.cache_write_1h + other.cache_write_1h,
            output: self.output + other.output,
        }
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::ModelCatalog;

    fn sample() -> TokenUsage {
        TokenUsage {
            input: 100,
            cache_read: 700,
            cache_write_5m: 120,
            cache_write_1h: 80,
            output: 50,
        }
    }

    #[test]
    fn prompt_tokens_exclude_what_the_model_generated() {
        assert_eq!(sample().prompt_tokens(), 1_000);
        assert_eq!(sample().total(), 1_050);
    }

    #[test]
    fn the_cache_ratio_is_reads_over_prompt_tokens() {
        assert_eq!(sample().cache_hit_ratio(), Some(0.7));
    }

    #[test]
    fn an_empty_usage_has_no_cache_ratio_rather_than_a_misleading_zero() {
        assert_eq!(TokenUsage::ZERO.cache_hit_ratio(), None);
    }

    #[test]
    fn each_token_kind_is_charged_at_its_own_published_rate() {
        let pricing = ModelCatalog::pricing_for("claude-opus-5");
        // 100 input @ $5, 700 reads @ $0.50, 120 five-minute writes @ $6.25,
        // 80 one-hour writes @ $10, 50 output @ $25
        // = 0.0005 + 0.00035 + 0.00075 + 0.0008 + 0.00125 = 0.00365
        let cost = sample().cost(pricing).dollars();
        assert!((cost - 0.003_65).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn a_one_hour_cache_write_costs_more_than_a_five_minute_one() {
        // The bug this guards against was a single blended write rate, which
        // charged the cheaper lease for both and understated any session that
        // took the longer one.
        let pricing = ModelCatalog::pricing_for("claude-opus-5");
        let short = TokenUsage {
            cache_write_5m: 1_000_000,
            ..TokenUsage::ZERO
        };
        let long = TokenUsage {
            cache_write_1h: 1_000_000,
            ..TokenUsage::ZERO
        };
        assert!((short.cost(pricing).dollars() - 6.25).abs() < 1e-9);
        assert!((long.cost(pricing).dollars() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn the_two_write_leases_occupy_the_context_window_identically() {
        // They are priced apart but sized the same, so anything measuring the
        // window must see their sum.
        assert_eq!(sample().cache_creation(), 200);
        assert_eq!(sample().prompt_tokens(), 1_000);
        assert_eq!(sample().total(), 1_050);
    }
}
