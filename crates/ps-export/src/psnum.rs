//! PostScript real-number formatting.
//!
//! Every number operand the emitter writes goes through [`num`]: PostScript reals are shortest
//! when trailing zeros are gone, integers render bare, and `-0` (which parses but reads wrong)
//! never appears. Non-finite values have no PostScript operand at all — callers replace them
//! with `0` and record a fidelity note.

// Foundation for the later Layer A emitter tasks; until they call these, only the tests do.
#![allow(dead_code)]

/// Format `v` as a PostScript number operand, or `None` if it has no operand form.
///
/// `None` covers NaN and ±inf; callers replace those with `0` and record the note
/// "non-finite coordinate replaced with 0". Finite values keep at most 4 decimals
/// (`format!("{:.4}", …)` then trim): `1.5` not `1.5000`, `2` not `2.0000`, and never
/// `-0` — which also catches tiny negatives that round away entirely.
pub(crate) fn num(v: f64) -> Option<String> {
    if !v.is_finite() {
        return None;
    }
    let mut s = format!("{:.4}", v);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".to_string();
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::num;

    /// Finite values render exactly: no trailing zeros, integers bare.
    #[test]
    fn finite_exact_strings() {
        for (v, want) in [(0.0, "0"), (1.0, "1"), (1.5, "1.5"), (-2.25, "-2.25"), (0.1, "0.1"), (1000.0, "1000")] {
            assert_eq!(num(v), Some(want.to_string()), "num({v})");
        }
    }

    /// Values needing more than four decimals are rounded, then trimmed.
    #[test]
    fn rounds_to_four_decimals() {
        assert_eq!(num(123.456789), Some("123.4568".into()));
        assert_eq!(num(0.99996), Some("1".into()));
        assert_eq!(num(0.00004), Some("0".into()));
    }

    /// `-0` and tiny negatives that round away never render as `-0`.
    #[test]
    fn negative_zero_becomes_zero() {
        assert_eq!(num(-0.0), Some("0".into()));
        assert_eq!(num(-0.00004), Some("0".into()));
    }

    /// NaN and ±inf have no PostScript operand; callers replace them with 0 and note it.
    #[test]
    fn non_finite_is_none() {
        assert_eq!(num(f64::NAN), None);
        assert_eq!(num(f64::INFINITY), None);
        assert_eq!(num(f64::NEG_INFINITY), None);
    }
}
