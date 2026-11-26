//! Decimal-string rounding and the General format.
//!
//! Excel works with 15 significant digits, so numbers are first written with 15 significant
//! digits and then rounded half away from zero on that decimal string. That makes 2.675 with
//! "0.00" show 2.68 and 1.005 show 1.01, as in Excel, even though neither is exact in binary.

use std::fmt::Write as _;

/// Most decimals we ever compute; placeholders beyond this are padded with zeros.
pub(crate) const MAX_DECIMALS: usize = 400;

/// The 15 significant digits of a positive finite number and the power of ten of the first
/// digit: `a ≈ d0.d1d2… × 10^exp`.
///
/// The value is rounded to the nearest 15-digit decimal. An exact tie at the 16th digit (the exact
/// decimal expansion is 15 digits, a 5, then only zeros, as for 1234567890123455 or
/// 123456789012345.5) goes toward zero, as Excel does: `1234567890123455` shows `1234567890123450`,
/// not `…460`. Excel observed (Windows 16.0.20430, TEXT()).
pub(crate) fn decimal_digits(a: f64) -> ([u8; 15], i32) {
    let mut digits = [0u8; 15];
    if !a.is_finite() || a <= 0.0 {
        return (digits, 0);
    }
    // 16 digits, rounded. Unless the 16th is a 5 it decides the 15th exactly (4 or less rounds
    // down, 6 or more up), so one format serves almost every number.
    let mut d16 = [0u8; 16];
    let exp = significant_digits(a, &mut d16);
    for (slot, d) in digits.iter_mut().zip(&d16) {
        *slot = *d;
    }
    match d16.get(15).copied().unwrap_or(0) {
        0..=4 => (digits, exp),
        5 if is_exact_tie(a) => (digits, exp),
        5 => {
            // Not a tie: round the exact value (the 16-digit rounding above may be a double rounding).
            let mut out = [0u8; 15];
            let exp15 = significant_digits(a, &mut out);
            (out, exp15)
        }
        _ => {
            let mut exp = exp;
            let mut carry = true;
            for d in digits.iter_mut().rev() {
                if *d == 9 {
                    *d = 0;
                } else {
                    *d += 1;
                    carry = false;
                    break;
                }
            }
            if carry {
                digits = [0; 15];
                digits[0] = 1;
                exp += 1;
            }
            (digits, exp)
        }
    }
}

/// Writes `out.len()` significant digits of `a` (rounded half to even on the exact binary value)
/// into `out` and returns the power of ten of the first.
///
/// Up to 32 digits are formatted in a stack buffer; more (the 800 of the tie check) use a string.
fn significant_digits(a: f64, out: &mut [u8]) -> i32 {
    fn parse(s: &str, out: &mut [u8]) -> i32 {
        let (mant, exp) = s.split_once('e').unwrap_or((s, "0"));
        let digits = mant.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0');
        for (slot, d) in out.iter_mut().zip(digits) {
            *slot = d;
        }
        exp.parse().unwrap_or(0)
    }
    let prec = out.len().saturating_sub(1);
    if out.len() <= 32 {
        let mut buf = StackStr::default();
        if write!(buf, "{:.*e}", prec, a).is_ok() {
            return parse(buf.as_str(), out);
        }
    }
    parse(&format!("{:.*e}", prec, a), out)
}

/// A fixed-size string buffer for short number formatting; a write that does not fit fails.
struct StackStr {
    buf: [u8; 64],
    len: usize,
}

impl Default for StackStr {
    fn default() -> Self {
        StackStr { buf: [0; 64], len: 0 }
    }
}

impl StackStr {
    fn as_str(&self) -> &str {
        self.buf.get(..self.len).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl std::fmt::Write for StackStr {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(std::fmt::Error)?;
        self.buf.get_mut(self.len..end).ok_or(std::fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// True when `a`'s exact decimal expansion has a 5 as its 16th significant digit and only zeros after.
fn is_exact_tie(a: f64) -> bool {
    let is_tie = |ds: &[u8]| ds.get(15) == Some(&5) && ds.get(16..).is_some_and(|t| t.iter().all(|&d| d == 0));
    // A cheap screen first (30 digits), then the exact expansion: a double has at most 767
    // significant digits and Rust prints every one of them.
    let mut screen = [0u8; 30];
    significant_digits(a, &mut screen);
    if !is_tie(&screen) {
        return false;
    }
    let mut exact = vec![0u8; 800];
    significant_digits(a, &mut exact);
    is_tie(&exact)
}

/// Rounds the number `digits × 10^exp` (see [`decimal_digits`]) to `decimals` places, half away
/// from zero. Returns the integer digits ("" when the integer part is zero) and exactly
/// `decimals` fraction digits.
pub(crate) fn fixed_from_digits(digits: &[u8; 15], exp: i32, decimals: usize) -> (String, String) {
    let (mut int, mut frac) = (String::new(), String::new());
    fixed_into(digits, exp, decimals, &mut int, &mut frac);
    (int, frac)
}

/// [`fixed_from_digits`] into caller-owned strings (cleared first), so a loop can reuse them.
///
/// The rounded digits are `lead` (at most 16, a carry can add a leading 1) followed by `zeros`
/// zero digits (placeholders beyond the 15 known digits), which is all a `Vec` of them held.
fn fixed_into(digits: &[u8; 15], exp: i32, decimals: usize, int: &mut String, frac: &mut String) {
    int.clear();
    frac.clear();
    let decimals_calc = decimals.min(MAX_DECIMALS);
    let keep = exp as i64 + decimals_calc as i64 + 1;
    let mut head = [0u8; 16];
    let mut head_len = 0usize;
    let mut zeros = 0usize;
    if keep <= 0 {
        if keep == 0 && digits.first().copied().unwrap_or(0) >= 5 {
            if let Some(h) = head.first_mut() {
                *h = 1;
            }
            head_len = 1;
        }
    } else if keep as usize >= digits.len() {
        for (h, d) in head.iter_mut().zip(digits) {
            *h = *d;
        }
        head_len = digits.len();
        zeros = keep as usize - digits.len();
    } else {
        let keep = keep as usize;
        for (h, d) in head.iter_mut().zip(digits.iter().take(keep)) {
            *h = *d;
        }
        head_len = keep;
        if digits.get(keep).copied().unwrap_or(0) >= 5 {
            let mut carry = true;
            for d in head.iter_mut().take(keep).rev() {
                if *d == 9 {
                    *d = 0;
                } else {
                    *d += 1;
                    carry = false;
                    break;
                }
            }
            if carry {
                head.copy_within(0..keep, 1);
                if let Some(h) = head.first_mut() {
                    *h = 1;
                }
                head_len += 1;
            }
        }
    }
    let total = head_len + zeros;
    let mut lead = head.iter().take(head_len).take_while(|&&d| d == 0).count();
    if lead == head_len {
        lead = total; // all zeros
    }
    let len = total - lead;
    // Digit `k` of the rounded number after its leading zeros.
    let digit_at = |k: usize| {
        let pos = lead + k;
        if pos < head_len { head.get(pos).map_or('0', |&d| char::from(b'0' + d)) } else { '0' }
    };
    if len <= decimals_calc {
        frac.extend(std::iter::repeat_n('0', decimals_calc - len));
        frac.extend((0..len).map(digit_at));
    } else {
        let split = len - decimals_calc;
        int.extend((0..split).map(digit_at));
        frac.extend((split..len).map(digit_at));
    }
    if decimals > decimals_calc {
        frac.extend(std::iter::repeat_n('0', decimals - decimals_calc));
    }
}

/// Rounds a non-negative number to `decimals` places (see [`fixed_from_digits`]).
pub(crate) fn fixed(a: f64, decimals: usize) -> (String, String) {
    if !a.is_finite() || a <= 0.0 {
        return (String::new(), "0".repeat(decimals.min(MAX_DECIMALS * 4)));
    }
    let (d, e) = decimal_digits(a);
    fixed_from_digits(&d, e, decimals)
}

/// General format squeezed into `max_chars` characters. Excel shows fewer decimals, then
/// scientific notation, then "####" when a column is narrow; `None` means the caller paints "####".
pub fn format_general_fit(n: f64, max_chars: usize) -> Option<String> {
    // The widest fixed rendering of a double is about 330 characters; a larger request only
    // pads zero decimals the trim below throws away.
    let max_chars = max_chars.min(1024);
    if !n.is_finite() {
        return None;
    }
    if n == 0.0 {
        return (max_chars >= 1).then(|| "0".to_string());
    }
    let sign = if n < 0.0 { "-" } else { "" };
    let (digits, x) = decimal_digits(n.abs());
    let int_len = sign.len() + (x.max(0) as usize) + 1;
    // Fixed notation for magnitudes from 1E-4 up to what fits. The 15 digits are computed once;
    // each width only re-rounds them, into strings that are reused.
    let (mut i, mut f, mut s) = (String::new(), String::new(), String::new());
    if x >= -4 && int_len <= max_chars {
        let avail = max_chars - int_len;
        // Zeros past the 400th decimal are trimmed below anyway, so a huge width needs no more.
        let mut decimals = (if avail >= 2 { avail - 1 } else { 0 }).min(MAX_DECIMALS);
        loop {
            fixed_into(&digits, x, decimals, &mut i, &mut f);
            let ft = f.trim_end_matches('0');
            if i.is_empty() && ft.is_empty() {
                break; // rounded away to zero: use scientific
            }
            s.clear();
            s.push_str(sign);
            s.push_str(if i.is_empty() { "0" } else { &i });
            if !ft.is_empty() {
                s.push('.');
                s.push_str(ft);
            }
            if s.chars().count() <= max_chars {
                return Some(s);
            }
            if decimals == 0 {
                break;
            }
            decimals -= 1;
        }
    }
    for md in (0..=max_chars.min(14)).rev() {
        let mut e = x;
        fixed_into(&digits, 0, md, &mut i, &mut f);
        if i.len() > 1 {
            e += 1;
            fixed_into(&digits, -1, md, &mut i, &mut f);
        }
        let ft = f.trim_end_matches('0');
        s.clear();
        s.push_str(sign);
        s.push_str(if i.is_empty() { "0" } else { &i });
        if !ft.is_empty() {
            s.push('.');
            s.push_str(ft);
        }
        let _ = write!(s, "E{}{:02}", if e < 0 { '-' } else { '+' }, e.unsigned_abs());
        if s.chars().count() <= max_chars {
            return Some(s);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding() {
        assert_eq!(fixed(2.675, 2), ("2".into(), "68".into()));
        assert_eq!(fixed(1.005, 2), ("1".into(), "01".into()));
        assert_eq!(fixed(0.5, 0), ("1".into(), "".into()));
        assert_eq!(fixed(0.04, 1), ("".into(), "0".into()));
        assert_eq!(fixed(0.05, 1), ("".into(), "1".into()));
        assert_eq!(fixed(9.999, 2), ("10".into(), "00".into()));
        assert_eq!(fixed(0.0004, 3), ("".into(), "000".into()));
        assert_eq!(fixed(0.0005, 3), ("".into(), "001".into()));
        assert_eq!(fixed(1e20, 0).0, "100000000000000000000");
        assert_eq!(fixed(0.0, 2), ("".into(), "00".into()));
        // Past 400 decimals only zeros are added; huge exponents keep all their digits.
        assert_eq!(fixed(1.5, 450), ("1".into(), format!("5{}", "0".repeat(449))));
        assert_eq!(fixed(1e300, 1), (format!("1{}", "0".repeat(300)), "0".into()));
        assert_eq!(fixed(123456789012345678.0, 0).0, "123456789012346000");
    }

    #[test]
    fn general_fit() {
        assert_eq!(format_general_fit(1e11, 11).as_deref(), Some("1E+11"));
        assert_eq!(format_general_fit(12345678901.0, 11).as_deref(), Some("12345678901"));
        // A huge width neither allocates width-sized padding nor changes the text.
        assert_eq!(format_general_fit(1.5, usize::MAX).as_deref(), Some("1.5"));
        assert_eq!(format_general_fit(123456789012.0, 11).as_deref(), Some("1.23457E+11"));
        assert_eq!(format_general_fit(0.1 + 0.2, 11).as_deref(), Some("0.3"));
        assert_eq!(format_general_fit(1.0 / 3.0, 11).as_deref(), Some("0.333333333"));
        assert_eq!(format_general_fit(123456.789012, 11).as_deref(), Some("123456.789"));
        assert_eq!(format_general_fit(1e-10, 11).as_deref(), Some("1E-10"));
        assert_eq!(format_general_fit(0.0000123456789, 11).as_deref(), Some("1.23457E-05"));
        assert_eq!(format_general_fit(0.0001, 11).as_deref(), Some("0.0001"));
        assert_eq!(format_general_fit(-5.0, 11).as_deref(), Some("-5"));
        assert_eq!(format_general_fit(0.0, 11).as_deref(), Some("0"));
        assert_eq!(format_general_fit(1234.5678, 6).as_deref(), Some("1234.6"));
        assert_eq!(format_general_fit(1234.5678, 4).as_deref(), Some("1235"));
        assert_eq!(format_general_fit(123456.0, 5).as_deref(), Some("1E+05"));
        assert_eq!(format_general_fit(123456.0, 3), None);
        assert_eq!(format_general_fit(9.99999999999, 5).as_deref(), Some("10"));
        assert_eq!(format_general_fit(f64::NAN, 11), None);
        assert_eq!(format_general_fit(1.7976931348623157e308, 11).as_deref(), Some("1.7977E+308"));
    }
}
