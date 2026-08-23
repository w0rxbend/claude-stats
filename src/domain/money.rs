//! The money value object used everywhere a cost is reported.
//!
//! Anthropic publishes prices as "dollars per one million tokens". Doing that
//! arithmetic ad hoc invites two classes of mistake: forgetting the factor of a
//! million, and adding a per-million *rate* to an absolute *amount* because
//! both are bare `f64`s. `Usd` and [`RatePerMillionTokens`] are separate types
//! precisely so the compiler rejects the second mistake, and the conversion
//! between them lives in exactly one place so the first one can only be made
//! once.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign};

/// An absolute amount of United States dollars.
#[derive(Debug, Default, Clone, Copy, PartialEq, PartialOrd)]
pub struct Usd(f64);

impl Usd {
    /// Nothing spent.
    pub const ZERO: Self = Self(0.0);

    /// Wraps a plain dollar amount.
    #[must_use]
    pub const fn new(dollars: f64) -> Self {
        Self(dollars)
    }

    /// The amount as a plain `f64`, for charts and ratios.
    #[must_use]
    pub const fn dollars(self) -> f64 {
        self.0
    }

    /// Divides the amount into `parts` equal shares.
    ///
    /// Returns [`Usd::ZERO`] when `parts` is zero, which is what callers want:
    /// "cost per turn" of a session with no turns is nothing, not a crash.
    #[must_use]
    pub fn per(self, parts: u32) -> Self {
        if parts == 0 {
            return Self::ZERO;
        }
        Self(self.0 / f64::from(parts))
    }
}

impl Add for Usd {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}

impl AddAssign for Usd {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl Sum for Usd {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

impl fmt::Display for Usd {
    /// Renders the amount at a precision a human can act on.
    ///
    /// Sub-cent amounts get four decimals, because during the first few turns
    /// of a session `$0.00` would look like the meter is broken. Everything
    /// else gets the usual two.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 > 0.0 && self.0 < 0.01 {
            write!(f, "${:.4}", self.0)
        } else {
            write!(f, "${:.2}", self.0)
        }
    }
}

/// A published price, in dollars per one million tokens.
///
/// This is a *rate*, not an amount. The only way to turn it into an [`Usd`] is
/// [`RatePerMillionTokens::charge_for`], which is where the division by a
/// million happens.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct RatePerMillionTokens(f64);

impl RatePerMillionTokens {
    /// One million: the number of tokens a published rate is quoted against.
    const TOKENS_PER_UNIT: f64 = 1_000_000.0;

    /// Wraps a published dollars-per-million-tokens figure.
    #[must_use]
    pub const fn new(dollars_per_million: f64) -> Self {
        Self(dollars_per_million)
    }

    /// What `tokens` tokens cost at this rate.
    #[must_use]
    pub fn charge_for(self, tokens: u64) -> Usd {
        Usd(self.0 * (tokens as f64) / Self::TOKENS_PER_UNIT)
    }

    /// The published figure itself, for display in a pricing table.
    #[must_use]
    pub const fn dollars_per_million(self) -> f64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_times_exactly_one_million_tokens_is_the_rate_itself() {
        let rate = RatePerMillionTokens::new(5.0);
        assert!((rate.charge_for(1_000_000).dollars() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn sub_cent_amounts_keep_four_decimals_so_they_do_not_look_like_zero() {
        assert_eq!(Usd::new(0.0031).to_string(), "$0.0031");
        assert_eq!(Usd::new(1.5).to_string(), "$1.50");
    }

    #[test]
    fn dividing_a_cost_over_zero_turns_yields_zero_rather_than_infinity() {
        assert_eq!(Usd::new(9.0).per(0), Usd::ZERO);
    }
}
