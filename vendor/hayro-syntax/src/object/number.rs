//! Numbers.

use crate::math::{powi_f64, trunc_f64};
use crate::object::macros::object;
use crate::object::{Object, ObjectLike};
use crate::reader::Reader;
use crate::reader::{Readable, ReaderContext, ReaderExt, Skippable};
use crate::trivia::{is_regular_character, is_white_space_character};
use core::fmt::Debug;

#[rustfmt::skip]
static POWERS_OF_10: [f64; 20] = [
    1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9,
    1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18, 1e19,
];

/// A number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Number(pub(crate) InternalNumber);

impl Number {
    /// The number zero.
    pub const ZERO: Self = Self::from_i32(0);
    /// The number one.
    pub const ONE: Self = Self::from_i32(1);

    /// Returns the number as a f64.
    pub fn as_f64(&self) -> f64 {
        match self.0 {
            InternalNumber::Real(r) => r,
            InternalNumber::Integer(i) => i as f64,
        }
    }

    /// Returns the number as a f32.
    pub fn as_f32(&self) -> f32 {
        match self.0 {
            InternalNumber::Real(r) => r.clamp(f32::MIN as f64, f32::MAX as f64) as f32,
            InternalNumber::Integer(i) => i as f32,
        }
    }

    /// Returns the number as an i64.
    pub fn as_i64(&self) -> i64 {
        match self.0 {
            InternalNumber::Real(r) => {
                let res = r as i64;

                if !(trunc_f64(r) == r) {
                    debug!("float {r} was truncated to {res}");
                }

                res
            }
            InternalNumber::Integer(i) => i,
        }
    }

    /// Create a new `Number` from an f32 number.
    pub const fn from_f32(num: f32) -> Self {
        Self(InternalNumber::Real(num as f64))
    }

    /// Create a new `Number` from an i32 number.
    pub const fn from_i32(num: i32) -> Self {
        Self(InternalNumber::Integer(num as i64))
    }
}

impl Skippable for Number {
    fn skip(r: &mut Reader<'_>, _: bool) -> Option<()> {
        let has_sign = r.forward_if(|b| b == b'+' || b == b'-').is_some();
        let mut has_digits = false;
        let mut had_trailing_minus = false;

        // Some PDFs have weird trailing minuses, so try to accept those as well.
        match r.peek_byte()? {
            b'.' => {
                r.read_byte()?;
                // See PDFJS-9252 - treat a single . as 0.
                while let Some(b) = r.peek_byte() {
                    if is_digit(b) {
                        has_digits = true;
                        r.forward();
                    } else if b == b'-' {
                        had_trailing_minus = true;
                        r.forward();
                    } else {
                        break;
                    }
                }
            }
            b'0'..=b'9' | b'-' => {
                while let Some(b) = r.peek_byte() {
                    if is_digit(b) {
                        has_digits = true;
                        r.forward();
                    } else if b == b'-' {
                        had_trailing_minus = true;
                        r.forward();
                    } else {
                        break;
                    }
                }
                if !had_trailing_minus && let Some(()) = r.forward_tag(b".") {
                    while let Some(b) = r.peek_byte() {
                        if is_digit(b) {
                            has_digits = true;
                            r.forward();
                        } else if b == b'-' {
                            had_trailing_minus = true;
                            r.forward();
                        } else {
                            break;
                        }
                    }
                }
            }
            // See PDFJS-bug1753983 - accept just + or - as a zero.
            // ALso see PDFJS-bug1953099, where the sign is followed by a show
            // text string operand, requiring us to allow '<' and '(' as well.
            b if has_sign && (is_white_space_character(b) || matches!(b, b'(' | b'<')) => {}
            _ => return None,
        }

        // PdfCraft patch: accept scientific-notation exponents (`1e30`, `1E-5`) when preceded
        // by at least one digit and followed by at least one exponent digit.
        if has_digits && !had_trailing_minus {
            skip_exponent(r);
        }

        // See issue 994. Don't accept numbers that are followed by a regular character.
        if r.peek_byte().is_some_and(is_regular_character) {
            return None;
        }

        Some(())
    }
}

fn skip_exponent(r: &mut Reader<'_>) {
    let mark = r.offset();
    if r.forward_if(|b| b == b'e' || b == b'E').is_some() {
        r.forward_if(|b| b == b'+' || b == b'-');
        if r.forward_while_1(is_digit).is_none() {
            r.jump(mark);
        }
    }
}

impl Readable<'_> for Number {
    #[inline]
    fn read(r: &mut Reader<'_>, _: &ReaderContext<'_>) -> Option<Self> {
        let old_offset = r.offset();
        read_inner(r).or_else(|| {
            r.jump(old_offset);
            None
        })
    }
}

#[inline(always)]
fn read_inner(r: &mut Reader<'_>) -> Option<Number> {
    let negative = match r.peek_byte()? {
        b'-' => {
            r.forward();
            true
        }
        b'+' => {
            r.forward();
            false
        }
        _ => false,
    };

    // PdfCraft patch: high-precision float mantissa overflow (jsPDF double-precision coordinates)
    // and scientific-notation exponents (`/MediaBox [0 0 1e30 1e30]`, `/UserUnit 1e20`).
    // Digits that don't fit in `mantissa` are dropped: past the dot they are only precision;
    // before it, each one scales the value by ten (`extra_int_digits`).
    let mut mantissa: u64 = 0;
    let mut has_dot = false;
    let mut decimal_shift: u32 = 0;
    let mut has_digits = false;
    let mut extra_int_digits: u32 = 0;
    let mut had_trailing_minus = false;

    loop {
        match r.peek_byte() {
            Some(b'0'..=b'9') => {
                let d = (r.read_byte().unwrap() - b'0') as u64;
                has_digits = true;
                if let Some(next) = mantissa.checked_mul(10).and_then(|m| m.checked_add(d)) {
                    mantissa = next;
                    if has_dot {
                        decimal_shift = decimal_shift.saturating_add(1);
                    }
                } else if !has_dot {
                    extra_int_digits = extra_int_digits.saturating_add(1);
                }
                // If has_dot and adding the digit would overflow u64:
                // Safely ignore excess trailing fractional digits (do not add, do not increment decimal_shift).
            }
            Some(b'.') if !has_dot => {
                r.forward();
                has_dot = true;
            }
            // Some weird PDFs have trailing minus in the fraction of number.
            Some(b'-') if has_digits => {
                r.forward();
                r.forward_while(is_digit_or_minus);
                had_trailing_minus = true;
                break;
            }
            _ => break,
        }
    }

    if !has_digits {
        if (negative || has_dot) && !r.peek_byte().is_some_and(is_regular_character) {
            // Treat numbers like just `-`, `+` or `-.` as zero.
            return Some(Number(InternalNumber::Integer(0)));
        }
        return None;
    }

    let mut exponent: i32 = 0;
    let mut has_exponent = false;
    if !had_trailing_minus && matches!(r.peek_byte(), Some(b'e' | b'E')) {
        let exp_mark = r.offset();
        r.forward();
        let exp_negative = match r.peek_byte() {
            Some(b'-') => {
                r.forward();
                true
            }
            Some(b'+') => {
                r.forward();
                false
            }
            _ => false,
        };
        let mut exp_val: i32 = 0;
        let mut exp_digits = false;
        while let Some(b'0'..=b'9') = r.peek_byte() {
            let Some(byte) = r.read_byte() else { break };
            let d = (byte - b'0') as i32;
            exp_digits = true;
            exp_val = exp_val.saturating_mul(10).saturating_add(d);
        }
        if exp_digits {
            has_exponent = true;
            exponent = if exp_negative { -exp_val } else { exp_val };
        } else {
            r.jump(exp_mark);
        }
    }

    // See issue 994. Don't accept numbers that are followed by a regular character
    // without any white space in-between.
    if r.peek_byte().is_some_and(is_regular_character) {
        return None;
    }

    if !has_dot && !has_exponent && extra_int_digits == 0 {
        let value = if mantissa <= i64::MAX as u64 {
            let i = mantissa as i64;
            if negative { -i } else { i }
        } else if negative && mantissa == (i64::MIN as u64) {
            i64::MIN
        } else {
            let mut val = mantissa as f64;
            if negative {
                val = -val;
            }
            return Some(Number(InternalNumber::Real(val)));
        };
        return Some(Number(InternalNumber::Integer(value)));
    }

    if mantissa == 0 {
        return Some(Number(InternalNumber::Real(if negative { -0.0 } else { 0.0 })));
    }

    // Scale by the combined power of ten and keep overflowed reals finite.
    let net_exp = i64::from(extra_int_digits) - i64::from(decimal_shift) + i64::from(exponent);
    let mut value = mantissa as f64;
    if net_exp > 0 {
        let shift = net_exp as u64;
        if shift < POWERS_OF_10.len() as u64 {
            value *= POWERS_OF_10[shift as usize];
        } else if shift <= 308 {
            value *= powi_f64(10.0, shift as u32);
        } else {
            value = f64::MAX;
        }
        if !value.is_finite() {
            value = f64::MAX;
        }
    } else if net_exp < 0 {
        let shift = (-net_exp) as u64;
        if shift < POWERS_OF_10.len() as u64 {
            value /= POWERS_OF_10[shift as usize];
        } else if shift <= 308 {
            value /= powi_f64(10.0, shift as u32);
        } else if shift <= 350 {
            value = (value / powi_f64(10.0, 300)) / powi_f64(10.0, (shift - 300) as u32);
        } else {
            value = 0.0;
        }
    }

    if negative {
        value = -value;
    }

    Some(Number(InternalNumber::Real(value)))
}

object!(Number, Number);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum InternalNumber {
    Real(f64),
    Integer(i64),
}

macro_rules! int_num {
    ($i:ident) => {
        impl Skippable for $i {
            fn skip(r: &mut Reader<'_>, _: bool) -> Option<()> {
                r.forward_if(|b| b == b'+' || b == b'-');
                r.forward_while_1(is_digit)?;

                // We have a float instead of an integer.
                if r.peek_byte() == Some(b'.') {
                    return None;
                }

                // See issue 994. Don't accept numbers that are followed by a regular character
                // without any white space in-between.
                if r.peek_byte().is_some_and(is_regular_character) {
                    return None;
                }

                Some(())
            }
        }

        impl<'a> Readable<'a> for $i {
            fn read(r: &mut Reader<'a>, ctx: &ReaderContext<'a>) -> Option<$i> {
                r.read::<Number>(ctx)
                    .map(|n| n.as_i64())
                    .and_then(|n| n.try_into().ok())
            }
        }

        impl TryFrom<Object<'_>> for $i {
            type Error = ();

            fn try_from(value: Object<'_>) -> core::result::Result<Self, Self::Error> {
                match value {
                    Object::Number(n) => n.as_i64().try_into().ok().ok_or(()),
                    _ => Err(()),
                }
            }
        }

        impl<'a> ObjectLike<'a> for $i {}
    };
}

int_num!(i32);
int_num!(i64);
int_num!(u32);
int_num!(u16);
int_num!(usize);
int_num!(u8);

impl Skippable for f32 {
    fn skip(r: &mut Reader<'_>, is_content_stream: bool) -> Option<()> {
        r.skip::<Number>(is_content_stream).map(|_| {})
    }
}

impl Readable<'_> for f32 {
    fn read(r: &mut Reader<'_>, _: &ReaderContext<'_>) -> Option<Self> {
        r.read_without_context::<Number>().map(|n| n.as_f32())
    }
}

impl TryFrom<Object<'_>> for f32 {
    type Error = ();

    fn try_from(value: Object<'_>) -> Result<Self, Self::Error> {
        match value {
            Object::Number(n) => Ok(n.as_f32()),
            _ => Err(()),
        }
    }
}

impl ObjectLike<'_> for f32 {}

impl Skippable for f64 {
    fn skip(r: &mut Reader<'_>, is_content_stream: bool) -> Option<()> {
        r.skip::<Number>(is_content_stream).map(|_| {})
    }
}

impl Readable<'_> for f64 {
    fn read(r: &mut Reader<'_>, _: &ReaderContext<'_>) -> Option<Self> {
        r.read_without_context::<Number>().map(|n| n.as_f64())
    }
}

impl TryFrom<Object<'_>> for f64 {
    type Error = ();

    fn try_from(value: Object<'_>) -> Result<Self, Self::Error> {
        match value {
            Object::Number(n) => Ok(n.as_f64()),
            _ => Err(()),
        }
    }
}

impl ObjectLike<'_> for f64 {}

pub(crate) fn is_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

pub(crate) fn is_digit_or_minus(byte: u8) -> bool {
    is_digit(byte) || byte == b'-'
}

#[cfg(test)]
mod tests {
    use crate::object::Number;
    use crate::reader::Reader;
    use crate::reader::ReaderExt;

    #[test]
    fn int_1() {
        assert_eq!(
            Reader::new("0".as_bytes())
                .read_without_context::<i32>()
                .unwrap(),
            0
        );
    }

    #[test]
    fn int_3() {
        assert_eq!(
            Reader::new("+32".as_bytes())
                .read_without_context::<i32>()
                .unwrap(),
            32
        );
    }

    #[test]
    fn int_4() {
        assert_eq!(
            Reader::new("-32".as_bytes())
                .read_without_context::<i32>()
                .unwrap(),
            -32
        );
    }

    #[test]
    fn int_6() {
        assert_eq!(
            Reader::new("98349".as_bytes())
                .read_without_context::<i32>()
                .unwrap(),
            98349
        );
    }

    #[test]
    fn int_7() {
        assert_eq!(
            Reader::new("003245".as_bytes())
                .read_without_context::<i32>()
                .unwrap(),
            3245
        );
    }

    #[test]
    fn int_min_does_not_panic() {
        assert_eq!(
            Reader::new("-9223372036854775808".as_bytes())
                .read_without_context::<i64>()
                .unwrap(),
            i64::MIN
        );
    }

    #[test]
    fn real_1() {
        assert_eq!(
            Reader::new("3".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            3.0
        );
    }

    #[test]
    fn real_3() {
        assert_eq!(
            Reader::new("+32".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            32.0
        );
    }

    #[test]
    fn real_4() {
        assert_eq!(
            Reader::new("-32".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            -32.0
        );
    }

    #[test]
    fn real_5() {
        assert_eq!(
            Reader::new("-32.01".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            -32.01
        );
    }

    #[test]
    fn real_6() {
        assert_eq!(
            Reader::new("-.345".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            -0.345
        );
    }

    #[test]
    fn real_7() {
        assert_eq!(
            Reader::new("-.00143".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            -0.00143
        );
    }

    #[test]
    fn real_8() {
        assert_eq!(
            Reader::new("-12.0013".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            -12.0013
        );
    }

    #[test]
    fn real_9() {
        assert_eq!(
            Reader::new("98349.432534".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            98_349.43
        );
    }

    #[test]
    fn real_10() {
        assert_eq!(
            Reader::new("-34534656.34".as_bytes())
                .read_without_context::<f32>()
                .unwrap(),
            -34534656.34
        );
    }

    #[test]
    fn real_failing() {
        assert!(
            Reader::new("+abc".as_bytes())
                .read_without_context::<f32>()
                .is_none()
        );
    }

    #[test]
    fn number_1() {
        assert_eq!(
            Reader::new("+32".as_bytes())
                .read_without_context::<Number>()
                .unwrap()
                .as_f64() as f32,
            32.0
        );
    }

    #[test]
    fn number_2() {
        assert_eq!(
            Reader::new("-32.01".as_bytes())
                .read_without_context::<Number>()
                .unwrap()
                .as_f64() as f32,
            -32.01
        );
    }

    #[test]
    fn number_3() {
        assert_eq!(
            Reader::new("-.345".as_bytes())
                .read_without_context::<Number>()
                .unwrap()
                .as_f64() as f32,
            -0.345
        );
    }

    #[test]
    fn large_number() {
        assert_eq!(
            Reader::new("38359922".as_bytes())
                .read_without_context::<Number>()
                .unwrap()
                .as_i64(),
            38359922
        );
    }

    #[test]
    fn large_number_2() {
        assert_eq!(
            Reader::new("4294966260".as_bytes())
                .read_without_context::<u32>()
                .unwrap(),
            4294966260
        );
    }

    #[test]
    fn high_precision_float_does_not_overflow_mantissa() {
        let num1 = Reader::new("3874.9606299212600788".as_bytes())
            .read_without_context::<f64>()
            .unwrap();
        assert!((num1 - 3874.96062992126).abs() < 1e-6, "got {num1}");

        let num2 = Reader::new("5493.5433070866147318".as_bytes())
            .read_without_context::<f64>()
            .unwrap();
        assert!((num2 - 5493.543307086615).abs() < 1e-6, "got {num2}");

        // Verify extreme fractional precision (50 decimal places) truncates safely
        let num3 = Reader::new("0.12345678901234567890123456789012345678901234567890".as_bytes())
            .read_without_context::<f64>()
            .unwrap();
        assert!((num3 - 0.12345678901234568).abs() < 1e-15, "got {num3}");

        // Integer digits past `u64` scale the value instead of being dropped.
        let num4 = Reader::new("123456789012345678901.5".as_bytes())
            .read_without_context::<f64>()
            .unwrap();
        assert!((num4 / 1.2345678901234568e20 - 1.0).abs() < 1e-12, "got {num4}");
        let num5 = Reader::new("-123456789012345678901".as_bytes())
            .read_without_context::<f64>()
            .unwrap();
        assert!((num5 / -1.2345678901234568e20 - 1.0).abs() < 1e-12, "got {num5}");
        // Hundreds of digits stay finite.
        let huge = "9".repeat(400);
        let num6 = Reader::new(huge.as_bytes()).read_without_context::<f64>().unwrap();
        assert!(num6.is_finite() && num6 > 1e300, "got {num6}");
        let num6_f32 = Reader::new(huge.as_bytes()).read_without_context::<f32>().unwrap();
        assert_eq!(num6_f32, f32::MAX);
    }

    #[test]
    fn scientific_notation_and_extreme_exponents() {
        for (input, expected) in [
            ("1e30", 1e30_f64),
            ("1e20", 1e20),
            ("1E+5", 1e5),
            ("1.5e-3", 1.5e-3),
            ("-.25e2", -25.0),
            ("+2.e3", 2000.0),
            ("0e300", 0.0),
            ("-0.0e400", -0.0),
            ("0e-400", 0.0),
            ("1e-320", 1e-320),
            ("1e-400", 0.0),
            ("1234567890123456789012345e-5", 1.2345678901234568e19),
            ("0.0000000000000000000000001e25", 1.0),
        ] {
            let mut r = Reader::new(input.as_bytes());
            assert!(r.skip::<Number>(false).is_some(), "skip {input}");
            assert!(r.at_end(), "skip {input} consumed all bytes");
            let val = Reader::new(input.as_bytes()).read_without_context::<f64>().unwrap_or_else(|| panic!("read {input}"));
            assert!(val.is_finite(), "{input}: {val}");
            if expected == 0.0 {
                assert_eq!(val, 0.0, "{input}");
            } else {
                assert!(((val - expected) / expected).abs() < 1e-12, "{input}: got {val}, expected {expected}");
            }
        }

        for (input, expected_f64, expected_f32) in [
            ("1e40", 1e40, f32::MAX),
            ("-1e40", -1e40, f32::MIN),
            ("1e400", f64::MAX, f32::MAX),
            ("-1e400", -f64::MAX, f32::MIN),
            ("1e99999999999999999999", f64::MAX, f32::MAX),
        ] {
            let mut r = Reader::new(input.as_bytes());
            assert!(r.skip::<Number>(false).is_some(), "skip {input}");
            let v64 = Reader::new(input.as_bytes()).read_without_context::<f64>().unwrap();
            let v32 = Reader::new(input.as_bytes()).read_without_context::<f32>().unwrap();
            let obj32 = f32::try_from(Reader::new(input.as_bytes()).read_without_context::<crate::object::Object<'_>>().unwrap()).unwrap();
            assert_eq!(v64, expected_f64, "{input} f64");
            assert_eq!(v32, expected_f32, "{input} f32");
            assert_eq!(obj32, expected_f32, "{input} Object->f32");
        }

        for invalid in ["123endobj", "1e", "1E", "1e+", "1e-", "1e+abc", ".e5", "-.e5", "+e5", "-e5", "--e5", "1e30abc", "1e3.5", "12-3e4"] {
            assert!(Reader::new(invalid.as_bytes()).skip::<Number>(false).is_none(), "skip must reject {invalid}");
            assert!(Reader::new(invalid.as_bytes()).read_without_context::<Number>().is_none(), "read must reject {invalid}");
        }
    }
}
