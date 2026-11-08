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
            InternalNumber::Real(r) => r as f32,
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
        // PdfCraft patch (37): the whole sign run, however long, so skipping a number
        // covers the token `read_inner` reads or refuses. Two signs used to be the limit,
        // and a skip that stops short of the read puts the reader back where it started
        // (#3630 review, #6601 review).
        let run_start = r.offset();
        let mut has_sign = false;
        while r.forward_if(|b| b == b'+' || b == b'-').is_some() {
            has_sign = true;
        }
        // PdfCraft patch (38): whether the sign run is one the model parser refuses
        // (patch (37)'s rule in `read_inner`), and whether the body turns out dotted;
        // together they decide the trailing-character check below.
        let refused = r.range(run_start..r.offset()).is_some_and(|run| {
            let mut body = run;
            while body.first() == Some(&b'+') {
                body = &body[1..];
            }
            !(body.len() <= 2 && body.iter().all(|b| *b == b'-'))
        });
        let mut dotted = false;


        // Some PDFs have weird trailing minuses, so try to accept those as well.
        match r.peek_byte()? {
            b'.' => {
                r.read_byte()?;
                dotted = true;
                // See PDFJS-9252 - treat a single . as 0.
                r.forward_while(is_digit_or_minus);
            }
            b'0'..=b'9' | b'-' => {
                r.forward_while_1(is_digit_or_minus)?;
                if let Some(()) = r.forward_tag(b".") {
                    dotted = true;
                    r.forward_while(is_digit_or_minus);
                }
            }
            // See PDFJS-bug1753983 - accept just + or - as a zero.
            // ALso see PDFJS-bug1953099, where the sign is followed by a show
            // text string operand, requiring us to allow '<' and '(' as well.
            b if has_sign && (is_white_space_character(b) || matches!(b, b'(' | b'<')) => {}
            _ => return None,
        }

        // See issue 994. Don't accept numbers that are followed by a regular character.
        if r.peek_byte().is_some_and(is_regular_character) {
            // PdfCraft patch (38): a refused sign run over a dotted body reads as
            // zero however the token ends (patch (38) in `read_inner`), so skipping
            // covers it whole too; every other number keeps the refusal above.
            if refused && dotted {
                r.forward_while(is_regular_character);
            } else {
                return None;
            }
        }

        Some(())
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
    // PdfCraft patch (37): the sign prefix follows the model parser's rule, not a free
    // absorption. The COS lexer takes a run of signs as one token, and its number reader
    // tolerates a doubled sign -- `--280` and `+-280` are -280, `++280` is 280 (#3565,
    // #3299) -- but a longer or mixed run (`---280`, `-+280`) is a bad number there.
    // Absorbing every sign read `---280` as -280, so the two parsers disagreed about
    // whole boxes (#6601 review). A refused run is no number here either; read and
    // `Number::skip` still consume the same whole-token extent, so a container holding
    // one keeps its shape.
    let token_start = r.offset();
    while matches!(r.peek_byte(), Some(b'-' | b'+')) {
        r.forward();
    }
    let run = r.range(token_start..r.offset()).unwrap_or(&[]);
    let mut sign_body = run;
    while sign_body.first() == Some(&b'+') {
        sign_body = &sign_body[1..];
    }
    // After any leading '+'s, nothing, one '-' or two is a number's sign.
    let sign_ok = sign_body.len() <= 2 && sign_body.iter().all(|b| *b == b'-');
    let negative = sign_ok && !sign_body.is_empty();
    if !sign_ok {
        // PdfCraft patch (38): over a dotted body a refused run is still a number to
        // the model parser: its real branch collapses the first `--` and takes the
        // longest real prefix, which is empty here, so `---1.5abc` and `---280.0`
        // read as 0.0 there and the container survives (#6601 review). Read zero
        // here too, consuming the whole token as `Number::skip` does.
        let body_start = r.offset();
        r.forward_while(|b| b.is_ascii_digit() || b == b'-' || b == b'.');
        let dotted = r.range(body_start..r.offset()).is_some_and(|t| t.contains(&b'.'));
        r.forward_while(is_regular_character);
        if dotted {
            return Some(Number(InternalNumber::Integer(0)));
        }
        return None;
    }

    // PdfCraft patch: high-precision float mantissa overflow (jsPDF double-precision coordinates).
    // Digits that don't fit in `mantissa` are dropped: past the dot they are only precision;
    // before it, each one scales the value by ten (`extra_int_digits`).
    let mut mantissa: u64 = 0;
    let mut has_dot = false;
    let mut decimal_shift: u32 = 0;
    let mut has_digits = false;
    let mut extra_int_digits: u32 = 0;

    loop {
        match r.peek_byte() {
            Some(b'0'..=b'9') => {
                let d = (r.read_byte().unwrap() - b'0') as u64;
                has_digits = true;
                if let Some(next) = mantissa.checked_mul(10).and_then(|m| m.checked_add(d)) {
                    mantissa = next;
                    if has_dot {
                        decimal_shift += 1;
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
                break;
            }
            _ => break,
        }
    }

    if !has_digits {
        // PdfCraft patch (36): `-inf`, `-nan`, `+Infinity` are one token with no finite value, not
        // a sign (read as 0) followed by a stray `inf` (#6477). `Number::skip` already refused
        // them; `Object` reads them as null (`skip_non_finite`).
        if !has_dot && r.peek_byte().is_some_and(is_regular_character) {
            let word_start = r.offset();
            r.forward_while(is_regular_character);
            if r.range(token_start..r.offset()).is_some_and(has_no_finite_value) {
                return None;
            }
            r.jump(word_start);
        }
        if negative || has_dot {
            // Treat numbers like just `-`, `+` or `-.` as zero.
            return Some(Number(InternalNumber::Integer(0)));
        }
        return None;
    }

    // See issue 994. Don't accept numbers that are followed by a regular character
    // without any white space in-between.
    if r.peek_byte().is_some_and(is_regular_character) {
        return None;
    }

    // PdfCraft patch (36): a token with no finite value is absent at its slot, never clamped
    // (#6477). It used to saturate to `f64::MAX`, where pdfcraft-cos read infinity, so the page
    // size and the redaction geometry built from it disagreed. Only a value whose integer part
    // overflowed `u64` can be that large; whether it is decides from the token itself, exactly
    // as pdfcraft-cos's correctly rounded conversion does, not from the approximation below.
    if extra_int_digits > 0 && r.range(token_start..r.offset()).is_some_and(has_no_finite_value) {
        return None;
    }

    // A value whose integer part overflowed `u64` is a real, scaled by the digits dropped. Tokens
    // with no finite value were refused above. What still overflows here saturates, as before:
    // a finite value just under `f64::MAX` that this approximation overshoots, or a damaged token
    // the rule doesn't cover (digits with a trailing minus), which pdfcraft-cos refuses anyway.
    let scaled = |value: f64| -> f64 {
        let value = if extra_int_digits > 0 { value * powi_f64(10.0, extra_int_digits) } else { value };
        if value.is_finite() { value } else { f64::MAX }
    };

    if !has_dot && extra_int_digits > 0 {
        let value = scaled(mantissa as f64);
        return Some(Number(InternalNumber::Real(if negative { -value } else { value })));
    }

    if !has_dot {
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
        Some(Number(InternalNumber::Integer(value)))
    } else {
        let mut value = scaled(mantissa as f64);

        if decimal_shift > 0 {
            if decimal_shift < POWERS_OF_10.len() as u32 {
                value /= POWERS_OF_10[decimal_shift as usize];
            } else {
                value /= powi_f64(10.0, decimal_shift);
            }
        }

        if negative {
            value = -value;
        }

        Some(Number(InternalNumber::Real(value)))
    }
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
        r.read_without_context::<Number>()
            .map(|n| n.as_f64() as Self)
    }
}

impl TryFrom<Object<'_>> for f32 {
    type Error = ();

    fn try_from(value: Object<'_>) -> Result<Self, Self::Error> {
        match value {
            Object::Number(n) => Ok(n.as_f64() as Self),
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

/// PdfCraft patch (36): whether `token`, a whole token, is a number with no finite value: a word
/// form (`inf`, `infinity` or `nan` in any case) or a decimal number past the range of `f64`
/// (`1` followed by 400 zeros, `1e400`), after any leading signs (#6477).
///
/// Such a token is absent at its slot, never clamped. pdfcraft-cos applies the same rule
/// (`crates/cos/src/parser.rs`, `has_no_finite_value`), so the renderer and the editing crates
/// agree on page sizes and boxes. Keep the two definitions identical.
pub(crate) fn has_no_finite_value(token: &[u8]) -> bool {
    let mut body = token;
    while let Some((b'+' | b'-', rest)) = body.split_first() {
        body = rest;
    }
    const WORDS: [&[u8]; 3] = [b"inf", b"infinity", b"nan"];
    if WORDS.iter().any(|w| body.eq_ignore_ascii_case(w)) {
        return true;
    }
    if body.is_empty() || decimal_len(body) != body.len() {
        return false;
    }
    // `decimal_len` admits ASCII only, which is UTF-8.
    core::str::from_utf8(body)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .is_some_and(|v| !v.is_finite())
}

/// PdfCraft patch (36): length of the longest prefix of `s` that is digits with at most one `.`
/// and then an exponent with digits; zero when the prefix has no digit. The same grammar as
/// pdfcraft-cos's `real_prefix_len`, without its sign.
fn decimal_len(s: &[u8]) -> usize {
    let digits = |from: usize| s.get(from..).map_or(0, |r| r.iter().take_while(|b| b.is_ascii_digit()).count());
    let int = digits(0);
    let mut at = int;
    let mut frac = 0;
    if s.get(at) == Some(&b'.') {
        frac = digits(at + 1);
        at += 1 + frac;
    }
    if int + frac == 0 {
        return 0;
    }
    if matches!(s.get(at), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(s.get(at + 1), Some(b'+' | b'-')));
        let exp = digits(at + 1 + sign);
        if exp > 0 {
            at += 1 + sign + exp;
        }
    }
    at
}

/// PdfCraft patch (36): step over a token with no finite value ([`has_no_finite_value`]),
/// leaving the reader where it was otherwise. An object reads it as null, so the dictionary or
/// array holding it survives with that one slot absent, where a direct `1e400`, `-inf` or `+nan`
/// used to make its whole dictionary unreadable (#6477). Scanning with the reader, not the
/// bytes, keeps the end-of-window flag of streamed content (`Reader::looked_past_end`) honest.
///
/// PdfCraft patch (37): a token whose sign run the model parser refuses
/// ([`has_refused_sign_run`]) steps over the same way, for the same reason: `Number::skip`
/// accepts it as one token, so an object read must yield a slot for it -- null -- or the
/// array iterator, which unwraps what skip accepted, panics (#6601 review).
pub(crate) fn skip_non_finite(r: &mut Reader<'_>) -> Option<()> {
    let start = r.offset();
    r.forward_while(is_regular_character);
    if r.range(start..r.offset()).is_some_and(|t| has_no_finite_value(t) || has_refused_sign_run(t)) {
        Some(())
    } else {
        r.jump(start);
        None
    }
}

/// PdfCraft patch (37): whether `token`'s sign prefix is one the model parser refuses --
/// after any leading '+'s, something other than nothing, one '-' or two -- while the rest
/// of the token is a number's body (digits, '.', '-'). `---280` is such a token; `--280`
/// (a tolerated doubled sign) and `---nan` (no finite value, patch 36) are not.
fn has_refused_sign_run(token: &[u8]) -> bool {
    let run_len = token.iter().take_while(|b| matches!(b, b'+' | b'-')).count();
    if run_len == 0 {
        return false;
    }
    let (run, body) = token.split_at(run_len);
    let mut sign_body = run;
    while sign_body.first() == Some(&b'+') {
        sign_body = &sign_body[1..];
    }
    let refused = !(sign_body.len() <= 2 && sign_body.iter().all(|b| *b == b'-'));
    refused && !body.is_empty() && body.iter().all(|b| b.is_ascii_digit() || *b == b'-' || *b == b'.')
}

#[cfg(test)]
mod tests {
    use crate::object::Number;
    use crate::reader::Reader;
    use crate::reader::ReaderExt;

    /// PdfCraft patch: a doubled sign is one number. `--280` returned 0 and left `-280` to be
    /// read as a second number, so a `cm` carrying it got seven operands and a degenerate matrix
    /// (#3565). The COS lexer reads these the same way since #3299.
    #[test]
    fn a_doubled_sign_is_one_number() {
        for (src, want) in [("--280", -280), ("+-280", -280), ("++280", 280), ("-280", -280), ("+280", 280)] {
            let mut r = Reader::new(src.as_bytes());
            assert_eq!(r.read_without_context::<i32>().unwrap(), want, "{src}");
            assert!(r.at_end(), "{src}: the whole number was read, leaving nothing for a second one");
        }
        // The fraction too, and a lone sign is still zero.
        assert_eq!(Reader::new("--280.5".as_bytes()).read_without_context::<f64>().unwrap(), -280.5);
        assert_eq!(Reader::new("-".as_bytes()).read_without_context::<i32>().unwrap(), 0);

        // Skipping covers what reading accepts: a skip that fails where a read succeeds puts the
        // reader back where it started, so `Object::skip` would leave the number in place
        // (#3630 review).
        for src in ["--280", "+-280", "-+280", "++280", "-280", "+280", "--280.5"] {
            let mut r = Reader::new(src.as_bytes());
            assert!(r.skip::<Number>(false).is_some(), "{src}: not skipped");
            assert!(r.at_end(), "{src}: only part of the number was skipped");
        }
    }

    /// PdfCraft patch (37): a sign run is one token, and its value follows the model
    /// parser: a doubled sign is tolerated (`--280` and `+-280` are -280, `++280` is
    /// 280), a longer or mixed run is no number at all. Read used to absorb every sign,
    /// so `---280` read as -280 where pdfcraft-cos refuses the token (#6601 review).
    /// Either way skip consumes the whole token, so a container holding one keeps its
    /// shape.
    #[test]
    fn sign_runs_follow_the_model_parser() {
        use alloc::format;
        for (src, want) in [("--280", -280), ("+-280", -280), ("++280", 280), ("+++280", 280), ("+280", 280), ("-280", -280)] {
            let mut r = Reader::new(src.as_bytes());
            assert_eq!(r.read_without_context::<i32>().unwrap(), want, "{src}");
            assert!(r.at_end(), "{src}: the whole number was read, leaving nothing for a second one");
            let mut r = Reader::new(src.as_bytes());
            assert!(r.skip::<Number>(false).is_some(), "{src}: not skipped");
            assert!(r.at_end(), "{src}: only part of the number was skipped");
        }
        assert_eq!(Reader::new("--280.5".as_bytes()).read_without_context::<f64>().unwrap(), -280.5);
        for src in ["---280", "----280", "-+-280", "-+280"] {
            let mut r = Reader::new(src.as_bytes());
            assert_eq!(r.read_without_context::<Number>(), None, "{src}: a run the model refuses is no number");
            let mut r = Reader::new(src.as_bytes());
            assert!(r.skip::<Number>(false).is_some(), "{src}: not skipped");
            assert!(r.at_end(), "{src}: only part of the number was skipped");
        }
        // A refused run over a dotted body is the model parser's real branch: zero,
        // the whole token consumed, trailing garbage included (patch (38)). `-+5.5`
        // is the same case the object parser's real-prefix fallback reads as 0.0.
        for src in ["---280.0", "---1.5abc", "-+5.5"] {
            let mut r = Reader::new(src.as_bytes());
            assert_eq!(r.read_without_context::<f64>().unwrap(), 0.0, "{src}");
            assert!(r.at_end(), "{src}: the whole token was read");
            let mut r = Reader::new(src.as_bytes());
            assert!(r.skip::<Number>(false).is_some(), "{src}: not skipped");
            assert!(r.at_end(), "{src}: only part of the token was skipped");
        }
        // The token from the #6601 review: three signs, 400 nines and a trailing minus.
        // Skip accepts it as one token; read refuses it whole (the model parser refuses
        // it too), it does not split it in two.
        let huge = format!("---{}-", "9".repeat(400));
        let mut r = Reader::new(huge.as_bytes());
        assert_eq!(r.read_without_context::<Number>(), None);
        let mut r = Reader::new(huge.as_bytes());
        assert!(r.skip::<Number>(false).is_some(), "the token is skipped");
        assert!(r.at_end(), "the whole token was skipped");
        // A word form after a sign run is one token with no finite value, not 0 plus a stray.
        assert_eq!(Reader::new("---nan".as_bytes()).read_without_context::<Number>(), None);
    }

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
        // Hundreds of digits that still fit in `f64` keep their value.
        let large = format!("1{}", "0".repeat(300));
        let num6 = Reader::new(large.as_bytes()).read_without_context::<f64>().unwrap();
        assert!((num6 / 1e300 - 1.0).abs() < 1e-12, "got {num6}");
        // Past `f64` there is no value: absent, never clamped to `f64::MAX` (patch 36, #6477).
        let huge = "9".repeat(400);
        assert_eq!(Reader::new(huge.as_bytes()).read_without_context::<f64>(), None);
    }
}

/// PdfCraft patch (36): a number token with no finite value is absent at its slot, never
/// clamped, and pdfcraft-cos reads it the same way (#6477).
#[cfg(test)]
mod pdfcraft_non_finite_tests {
    use super::has_no_finite_value;
    use crate::content::UntypedIter;
    use crate::object::{Array, Dict, FromBytes, MaybeRef, Number, Object, Rect};
    use crate::reader::{Reader, ReaderExt};
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// One of each class: word forms, integer overflow, real overflow.
    fn tokens() -> Vec<String> {
        let mut t: Vec<String> = ["+inf", "-inf", "+nan", "-nan", "+Infinity", "-INF", "--nan", "1e400", "-1e400", "1.0e400", ".5E+400"]
            .iter()
            .map(|s| (*s).into())
            .collect();
        t.push(format!("1{}", "0".repeat(400)));
        t.push(format!("-{}", "9".repeat(400)));
        t.push(format!("{}.5", "9".repeat(400)));
        t
    }

    #[test]
    fn the_recogniser_takes_only_tokens_with_no_finite_value() {
        for t in tokens() {
            assert!(has_no_finite_value(t.as_bytes()), "{t}");
        }
        let finite = format!("1{}", "0".repeat(300));
        for t in ["0", "-1.5", "1e5", "1e308", "+", "-", ".", "", "infx", "nan5", "1e400x", "1.2.3", "-inf-", finite.as_str()] {
            assert!(!has_no_finite_value(t.as_bytes()), "{t}");
        }
    }

    #[test]
    fn a_number_with_no_finite_value_is_not_read_as_a_number() {
        for t in tokens() {
            let mut r = Reader::new(t.as_bytes());
            assert_eq!(r.read_without_context::<Number>(), None, "{t}");
            assert_eq!(r.offset(), 0, "{t}: a failed read leaves the reader where it was");
            assert_eq!(Reader::new(t.as_bytes()).read_without_context::<f32>(), None, "{t}");
        }
    }

    #[test]
    fn an_object_reads_it_as_null_and_skipping_covers_the_whole_token() {
        for t in tokens() {
            let mut r = Reader::new(t.as_bytes());
            let o = r.read_without_context::<Object<'_>>();
            assert!(matches!(o, Some(Object::Null(_))), "{t}: {o:?}");
            assert!(r.at_end(), "{t}: the whole token was read");
            let mut r = Reader::new(t.as_bytes());
            assert!(r.skip::<Object<'_>>(false).is_some(), "{t}: not skipped");
            assert!(r.at_end(), "{t}: only part of the token was skipped");
        }
    }

    #[test]
    fn the_dictionary_holding_it_keeps_its_other_entries() {
        for t in tokens() {
            let src = format!("<< /UserUnit {t} /Rotate 90 >>");
            let d = Dict::from_bytes(src.as_bytes()).unwrap_or_else(|| panic!("{t}: the dictionary is readable"));
            assert_eq!(d.get::<f64>(b"UserUnit"), None, "{t}: absent at its slot");
            assert!(matches!(d.get::<Object<'_>>(b"UserUnit"), Some(Object::Null(_))), "{t}");
            assert_eq!(d.get::<i32>(b"Rotate"), Some(90), "{t}");
            // Listing the entries reads each one again; it must not meet a token it can't read.
            assert_eq!(d.entries().count(), 2, "{t}");
        }
    }

    #[test]
    fn a_box_holding_it_is_not_a_box_and_the_array_reads_null_there() {
        for t in tokens() {
            let src = format!("[0 0 {t} 792]");
            assert!(Reader::new(src.as_bytes()).read_without_context::<Rect>().is_none(), "{t}");
            let a = Array::from_bytes(src.as_bytes()).unwrap_or_else(|| panic!("{t}: the array is readable"));
            let items: Vec<_> = a.raw_iter().collect();
            assert_eq!(items.len(), 4, "{t}");
            assert!(matches!(items[2], MaybeRef::NotRef(Object::Null(_))), "{t}: {:?}", items[2]);
            assert!(matches!(items[3], MaybeRef::NotRef(Object::Number(_))), "{t}");
        }
    }

    #[test]
    fn a_content_operand_with_no_finite_value_is_null_in_its_place() {
        // `-nan` used to be 0 and then an operator `nan`; `+nan` an operator that cleared the
        // operands before it. pdfcraft-content reads null in that place, as here.
        for t in tokens() {
            let src = format!("1 {t} 3 4 re");
            let mut iter = UntypedIter::new(src.as_bytes());
            let instruction = iter.next().unwrap_or_else(|| panic!("{t}: an instruction"));
            assert_eq!(instruction.operator.as_ref(), b"re", "{t}");
            let operands: Vec<_> = instruction.operands().collect();
            assert_eq!(operands.len(), 4, "{t}: {operands:?}");
            assert!(matches!(operands[1], Object::Null(_)), "{t}: {operands:?}");
        }
    }
}
