//! Non-negative decimal inputs with at most twelve integral and six fractional digits.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Number of internal integer units representing one whole decimal unit.
pub(crate) const SCALE: u128 = 1_000_000;

/// An exact decimal, serialized as a string; never converted through floating point.
///
/// Accepts ASCII digits and an optional dot with 1–6 fractional digits. No sign,
/// exponent, grouping, whitespace or locale-dependent separator is accepted.
/// Display and JSON serialization remove redundant leading and fractional zeros.
///
/// ```
/// # fn main() -> Result<(), String> {
/// use pdfcraft_invoice::Decimal;
/// let price: Decimal = "0012.345600".parse()?;
/// assert_eq!(price.to_string(), "12.3456");
/// assert!("12,34".parse::<Decimal>().is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Decimal(pub(crate) u128);

impl FromStr for Decimal {
    type Err = String;

    /// Parse bounded ASCII decimal notation without rounding or accepting locale separators.
    /// Returns a format error for unsupported syntax or precision before numeric conversion.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || "expected a non-negative decimal string with 1–12 integral and at most 6 fractional digits".to_owned();
        if text.is_empty() || text.len() > 19 {
            return Err(invalid());
        }
        let (whole, fraction) = match text.split_once('.') {
            Some((whole, fraction)) if !fraction.is_empty() && fraction.len() <= 6 => (whole, fraction),
            Some(_) => return Err(invalid()),
            None => (text, ""),
        };
        if whole.is_empty() || whole.len() > 12 || !whole.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        let integral = whole.parse::<u128>().map_err(|_| invalid())?;
        let fractional = if fraction.is_empty() { 0 } else { fraction.parse::<u128>().map_err(|_| invalid())? };
        // Fraction length is checked above, so the exponent is in 0..=6.
        let factor = 10_u128.pow(6 - fraction.len() as u32);
        let value = integral.checked_mul(SCALE).and_then(|w| fractional.checked_mul(factor).and_then(|f| w.checked_add(f))).ok_or_else(invalid)?;
        Ok(Self(value))
    }
}

impl TryFrom<String> for Decimal {
    type Error = String;

    /// Apply the same strict parser to owned strings, including serde input.
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Decimal> for String {
    /// Produce canonical decimal notation for serde without a floating-point intermediate.
    fn from(value: Decimal) -> Self {
        value.to_string()
    }
}

impl fmt::Display for Decimal {
    /// Write a dot-separated decimal with no unnecessary integral or fractional zeros.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / SCALE;
        let fraction = self.0 % SCALE;
        if fraction == 0 {
            write!(f, "{whole}")
        } else {
            let digits = format!("{fraction:06}");
            write!(f, "{whole}.{}", digits.trim_end_matches('0'))
        }
    }
}

/// Round a non-negative rational to the nearest integer, with ties rounded up.
/// Returns `None` for a zero denominator or an unrepresentable rounded result.
pub(crate) fn rounded(numerator: u128, denominator: u128) -> Option<u128> {
    if denominator == 0 {
        return None;
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    // Compare without doubling a possibly large remainder or overflowing numerator + half.
    quotient.checked_add(u128::from(remainder >= denominator - remainder))
}

/// Format already-rounded EUR cents with exactly two fractional digits.
pub(crate) fn money(cents: u128) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Pin the accepted grammar and ensure JSON numbers cannot bypass string parsing.
    #[test]
    fn decimals_reject_ambiguous_or_unbounded_inputs() {
        for text in ["", "-1", "+1", "1,25", " 1", "1 ", ".5", "1.", "1e2", "NaN", "1.0000001", "1000000000000", "１", "1.2.3"] {
            assert!(text.parse::<Decimal>().is_err(), "{text:?}");
        }
        assert_eq!("0001.230000".parse::<Decimal>().unwrap().to_string(), "1.23");
        assert_eq!("999999999999.999999".parse::<Decimal>().unwrap().to_string(), "999999999999.999999");
        assert_eq!(serde_json::from_str::<Decimal>("0.1").unwrap_err().classify(), serde_json::error::Category::Data);
        assert_eq!(serde_json::to_string(&"0.1".parse::<Decimal>().unwrap()).unwrap(), "\"0.1\"");
    }

    /// Exercise tie handling and integer limits without overflowing an intermediate sum.
    #[test]
    fn rational_rounding_handles_ties_zero_and_the_integer_limit() {
        assert_eq!(rounded(1, 2), Some(1));
        assert_eq!(rounded(149, 100), Some(1));
        assert_eq!(rounded(150, 100), Some(2));
        assert_eq!(rounded(u128::MAX, 1), Some(u128::MAX));
        assert_eq!(rounded(u128::MAX, u128::MAX), Some(1));
        assert_eq!(rounded(0, 0), None);
    }

    proptest! {
        /// Preserve exact micro-units across canonical display and serde round trips.
        #[test]
        fn micro_units_survive_string_and_json_round_trips(units in 0_u64..1_000_000_000_000_000_000) {
            let value = Decimal(u128::from(units));
            prop_assert_eq!(value.to_string().parse::<Decimal>().unwrap(), value);
            prop_assert_eq!(serde_json::from_str::<Decimal>(&serde_json::to_string(&value).unwrap()).unwrap(), value);
        }

        /// Require arbitrary Unicode text to produce a value or error without panicking.
        #[test]
        fn arbitrary_text_is_a_total_parser(text in ".{0,100}") {
            let _ = text.parse::<Decimal>();
        }
    }
}
