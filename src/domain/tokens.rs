//! How a single API call's token usage is counted and priced.

use std::ops::{Add, AddAssign};

use super::money::Usd;
use super::model::ModelPricing;

/// The four token counters the Claude API reports for one assistant response.
///
/// The transcript records these under `message.usage`. Every metric on the
/// dashboard -- context fill, cost, cache ratio -- is derived from these four
/// numbers, so they are modelled once here rather than being passed around as
/// a loose tuple.
///
/// The distinction that matters most:
///
/// * `input` -- fresh tokens sent to the model, charged at full price.
/// * `cache_read` -- tokens served from the prompt cache, charged at roughly a
///   tenth of the input price. A high share here is *good*; it means the
///   conversation prefix is being reused instead of re-sent.
/// * `cache_creation` -- tokens written into the prompt cache, charged at
///   roughly 1.25x the input price. You pay a small premium now to get the
///   cheap reads later.
/// * `output` -- tokens the model generated, the most expensive of the four.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub output: u64,
}

impl TokenUsage {
    /// Nothing used yet.
    pub const ZERO: Self = Self {
        input: 0,
        cache_read: 0,
        cache_creation: 0,
        output: 0,
    };

    /// Every token involved in this call, whatever its kind.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.input + self.cache_read + self.cache_creation + self.output
    }

    /// The tokens that were *sent to* the model, excluding what it generated.
    ///
    /// This is the number that fills the context window. Output tokens do not
    /// occupy the window of the call that produced them -- they occupy the
    /// window of the *next* call, by which point they have been folded into
    /// the input or cache counters.
    #[must_use]
    pub const fn prompt_tokens(self) -> u64 {
        self.input + self.cache_read + self.cache_creation
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
            + pricing.cache_write.charge_for(self.cache_creation)
            + pricing.output.charge_for(self.output)
    }
}

impl Add for TokenUsage {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            input: self.input + other.input,
            cache_read: self.cache_read + other.cache_read,
            cache_creation: self.cache_creation + other.cache_creation,
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
            cache_creation: 200,
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
        // 100 input @ $5, 700 reads @ $0.50, 200 writes @ $6.25, 50 output @ $25
        // = 0.0005 + 0.00035 + 0.00125 + 0.00125 = 0.00335
        let cost = sample().cost(pricing).dollars();
        assert!((cost - 0.003_35).abs() < 1e-9, "got {cost}");
    }
}
