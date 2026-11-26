//! Rendering a value with a parsed section.

use std::fmt::Write as _;

use gridcraft_core::DateSystem;
use gridcraft_core::date::{datetime_from_serial, max_serial};

use crate::calendar::Calendar;
use crate::decimal::{decimal_digits, fixed, fixed_from_digits};
use crate::parse::{DatePart, Section, Tok};

/// Text being built plus the first repeat-fill position.
#[derive(Default)]
pub(crate) struct Out {
    pub text: String,
    pub fill: Option<(char, usize)>,
    /// True when every digit shown is zero (a number section that rounded to zero).
    pub zero: bool,
}

impl Out {
    fn push(&mut self, s: &str) {
        self.text.push_str(s);
    }
    fn fill(&mut self, c: char) {
        if self.fill.is_none() {
            self.fill = Some((c, self.text.len()));
        }
    }
    /// Appends what `t` shows when it has no special meaning (no temporary string).
    fn push_tok(&mut self, t: &Tok) {
        t.push_literal(&mut self.text);
    }
    pub(crate) fn prepend(&mut self, s: &str) {
        self.text.insert_str(0, s);
        if let Some((_, p)) = &mut self.fill {
            *p += s.len();
        }
    }
}

/// Renders a text value through a text section (`@` is replaced by the text).
pub(crate) fn render_text(sec: &Section, text: &str) -> Out {
    let mut out = Out::default();
    for t in &sec.toks {
        match t {
            Tok::At => out.push(text),
            Tok::Fill(c) => out.fill(*c),
            other => out.push_tok(other),
        }
    }
    out
}

/// [`render_text`], but `None` once the output passes `max_units` UTF-16 code units (a cell's
/// limit), checked before each piece is appended, so an oversized result is never built.
pub(crate) fn render_text_within(sec: &Section, text: &str, max_units: usize) -> Option<Out> {
    let text_units = text.encode_utf16().count();
    let mut out = Out::default();
    let mut units = 0usize;
    for t in &sec.toks {
        match t {
            Tok::At => {
                units = units.saturating_add(text_units);
                if units > max_units {
                    return None;
                }
                out.push(text);
            }
            Tok::Fill(c) => out.fill(*c),
            other => {
                let literal = other.literal_text();
                units = units.saturating_add(literal.encode_utf16().count());
                if units > max_units {
                    return None;
                }
                out.push(&literal);
            }
        }
    }
    Some(out)
}

/// Renders a number through a General section: `general` (the number already written as General)
/// with the section's literals around it.
pub(crate) fn render_general(sec: &Section, general: &str) -> Out {
    let mut out = Out::default();
    for t in &sec.toks {
        match t {
            Tok::General => out.push(general),
            Tok::Fill(c) => out.fill(*c),
            other => out.push_tok(other),
        }
    }
    out
}

/// Renders a date/time. `None` when the serial is outside the supported range.
pub(crate) fn render_date(sec: &Section, v: f64, sys: DateSystem) -> Option<Out> {
    if !v.is_finite() || !(0.0..max_serial(sys) as f64 + 1.0).contains(&v) {
        return None;
    }
    let unit: i64 = 10i64.pow(3 - u32::from(sec.sub_digits.min(3)));
    let mut day = v.floor() as i64;
    let mut ms = (((v - v.floor()) * 86_400_000.0) / unit as f64).round() as i64 * unit;
    if ms >= 86_400_000 {
        ms -= 86_400_000;
        day += 1;
    }
    let total_ms = day.saturating_mul(86_400_000).saturating_add(ms);
    // Under `B2`, Excel shows the 1900 system's fictitious 29 February (serial 60) as `#VALUE!`
    // whenever a clock or calendar part is shown, even for `hh:mm` (`[h]` alone still works).
    if sec.calendar == Calendar::Hijri
        && sys == DateSystem::D1900
        && day == 60
        && sec.toks.iter().any(|t| matches!(t, Tok::Date(d) if !d.is_elapsed()))
    {
        return None;
    }
    let dt = if sec.needs_date { datetime_from_serial(sys, day as f64)? } else { datetime_from_serial(sys, 0.0).unwrap_or_default() };
    let ampm = sec.ampm;
    let hour = ms / 3_600_000;
    let minute = (ms / 60_000) % 60;
    let second = (ms / 1000) % 60;
    let milli = ms % 1000;
    // No date in the section's calendar (serial 60 under `B2`): shown like an out-of-range date.
    let date = sec.calendar.date(&dt)?;
    let mut out = Out::default();
    for t in &sec.toks {
        match t {
            Tok::Date(d) => {
                let text = &mut out.text;
                match *d {
                    DatePart::Year(n) => {
                        let _ = if n <= 2 { write!(text, "{:02}", date.year.rem_euclid(100)) } else { write!(text, "{:04}", date.year) };
                    }
                    DatePart::Month(n) => match n {
                        1 | 2 => push_padded(text, date.month as i64, n),
                        3 => text.push_str(date.month_short),
                        5 => text.extend(date.month_name.chars().take(1)),
                        _ => text.push_str(date.month_name),
                    },
                    DatePart::Day(n) => match n {
                        1 | 2 => push_padded(text, date.day as i64, n),
                        3 => text.push_str(date.day_short),
                        _ => text.push_str(date.day_name),
                    },
                    DatePart::Hour(n) => {
                        let h = if ampm {
                            let h = hour % 12;
                            if h == 0 { 12 } else { h }
                        } else {
                            hour
                        };
                        push_padded(text, h, n);
                    }
                    DatePart::Minute(n) => push_padded(text, minute, n),
                    DatePart::Second(n) => push_padded(text, second, n),
                    DatePart::SubSec(n) => {
                        // `.` and the first `n` (at most 3) of the three millisecond digits, then
                        // zeros for the digits beyond milliseconds.
                        text.push('.');
                        for (k, div) in [100, 10, 1].into_iter().enumerate() {
                            if k < n as usize {
                                text.push(char::from(b'0' + (milli / div % 10) as u8));
                            }
                        }
                        for _ in 3..n {
                            text.push('0');
                        }
                    }
                    DatePart::ElapsedH(n) => push_padded(text, total_ms / 3_600_000, n),
                    DatePart::ElapsedM(n) => push_padded(text, total_ms / 60_000, n),
                    DatePart::ElapsedS(n) => push_padded(text, total_ms / 1000, n),
                    DatePart::AmPm => text.push_str(if hour >= 12 { "PM" } else { "AM" }),
                    DatePart::AP(a, p) => text.push(if hour >= 12 { p } else { a }),
                }
            }
            Tok::Fill(c) => out.fill(*c),
            other => out.push_tok(other),
        }
    }
    Some(out)
}

/// Appends `n`, zero padded to `w` digits (at most 20) when `w` is 2 or more.
fn push_padded(out: &mut String, n: i64, w: u8) {
    let _ = if w >= 2 { write!(out, "{:0w$}", n, w = w.min(20) as usize) } else { write!(out, "{n}") };
}

/// Scratch text of one number rendering: every token's own output (`piece`, replaces the token's
/// text), the text inserted ahead of a token (`before`) and the replacement of a literal
/// (`lit_spaces`: fractions blank the literals of a hidden part) live in ONE buffer, addressed
/// by byte ranges, instead of one string per token. Writing a piece again appends the new text
/// and points the slot at it.
struct Parts {
    buf: String,
    slots: Vec<Slot>,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    piece: (usize, usize),
    before: (usize, usize),
    /// `Some(k)`: show `k` spaces instead of the literal.
    lit_spaces: Option<usize>,
}

impl Parts {
    fn new(n: usize) -> Parts {
        Parts { buf: String::new(), slots: vec![Slot::default(); n + 1] }
    }
    /// Start of a piece written straight into the buffer; finish it with [`Parts::end_piece`].
    fn mark(&self) -> usize {
        self.buf.len()
    }
    fn end_piece(&mut self, ti: usize, start: usize) {
        let end = self.buf.len();
        if let Some(sl) = self.slots.get_mut(ti) {
            sl.piece = (start, end);
        }
    }
    fn set_piece(&mut self, ti: usize, text: &str) {
        let start = self.mark();
        self.buf.push_str(text);
        self.end_piece(ti, start);
    }
    fn set_spaces(&mut self, ti: usize, k: usize) {
        let start = self.mark();
        self.buf.extend(std::iter::repeat_n(' ', k));
        self.end_piece(ti, start);
    }
    fn set_before(&mut self, ti: usize, text: &str) {
        let start = self.mark();
        self.buf.push_str(text);
        let end = self.buf.len();
        if let Some(sl) = self.slots.get_mut(ti) {
            sl.before = (start, end);
        }
    }
    fn set_lit_spaces(&mut self, ti: usize, k: usize) {
        if let Some(sl) = self.slots.get_mut(ti) {
            sl.lit_spaces = Some(k);
        }
    }
    fn text(&self, (a, b): (usize, usize)) -> &str {
        self.buf.get(a..b).unwrap_or("")
    }
    fn piece(&self, ti: usize) -> &str {
        self.slots.get(ti).map_or("", |sl| self.text(sl.piece))
    }
    fn before(&self, ti: usize) -> &str {
        self.slots.get(ti).map_or("", |sl| self.text(sl.before))
    }
}

/// Assigns integer digits right-to-left to the placeholders at `idxs`; the leftmost placeholder
/// takes all remaining digits. `force_zero` shows `0` in the units placeholder when there are no
/// digits. A blank `?` pads with a space, and so does the thousands separator it would carry.
///
/// `digits` is ASCII (digit strings made by this crate), so bytes are characters. Each
/// placeholder's text is written left to right: a digit, then the separator that follows it when
/// its position counted from the right is a multiple of 3.
fn fill_int(toks: &[Tok], idxs: &[usize], digits: &str, thousands: bool, force_zero: bool, parts: &mut Parts) {
    let ds = digits.as_bytes();
    let mut rem = ds.len();
    let mut pos = 0usize;
    let count = idxs.len();
    let sep = |p: usize| thousands && p > 0 && p.is_multiple_of(3);
    for (k, &ti) in idxs.iter().rev().enumerate() {
        let leftmost = k + 1 == count;
        let ph = match toks.get(ti) {
            Some(Tok::Digit(c)) => *c,
            _ => '0',
        };
        let start = parts.mark();
        let take = if leftmost { rem } else { rem.min(1) };
        if take == 0 {
            match ph {
                '0' => {
                    parts.buf.push('0');
                    if sep(pos) {
                        parts.buf.push(',');
                    }
                    pos += 1;
                }
                _ if force_zero && k == 0 => {
                    parts.buf.push('0');
                    if sep(pos) {
                        parts.buf.push(',');
                    }
                    pos += 1;
                }
                '?' => {
                    parts.buf.push(' ');
                    if sep(pos) {
                        parts.buf.push(' ');
                    }
                    pos += 1;
                }
                _ => {}
            }
        } else {
            // The `take` digits just left of `rem`, most significant first; the digit `s` places
            // from the right end of the run sits at position `pos + s`.
            for s in (0..take).rev() {
                let d = ds.get(rem.saturating_sub(1 + s)).copied().unwrap_or(b'0');
                parts.buf.push(char::from(d));
                if sep(pos + s) {
                    parts.buf.push(',');
                }
            }
            rem -= take;
            pos += take;
        }
        parts.end_piece(ti, start);
    }
}

fn with_commas(digits: &str, thousands: bool) -> String {
    if !thousands {
        return digits.to_string();
    }
    let n = digits.chars().count();
    let mut s = String::with_capacity(n + n / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            s.push(',');
        }
        s.push(c);
    }
    s
}

/// Fraction-digit placeholders: trailing zeros show as `0`, nothing (`#`) or a space (`?`).
/// `frac` is ASCII digits.
fn fill_frac(toks: &[Tok], idxs: &[usize], frac: &str, parts: &mut Parts) {
    let fd = frac.as_bytes();
    let last_sig = fd.iter().rposition(|&c| c != b'0');
    for (i, &ti) in idxs.iter().enumerate() {
        let ph = match toks.get(ti) {
            Some(Tok::Digit(c)) => *c,
            _ => '0',
        };
        let start = parts.mark();
        if last_sig.is_some_and(|l| i <= l) {
            parts.buf.push(char::from(fd.get(i).copied().unwrap_or(b'0')));
        } else {
            match ph {
                '0' => parts.buf.push('0'),
                '?' => parts.buf.push(' '),
                _ => {}
            }
        }
        parts.end_piece(ti, start);
    }
}

fn digit_indices(toks: &[Tok], range: std::ops::Range<usize>) -> Vec<usize> {
    range.filter(|&i| matches!(toks.get(i), Some(Tok::Digit(_)))).collect()
}

/// Where a number section's placeholders are: the decimal point, the exponent, and the digit
/// placeholders before the point (or exponent) and between the point and the exponent.
struct NumberLayout {
    point: Option<usize>,
    exp: Option<usize>,
    int_idx: Vec<usize>,
    frac_idx: Vec<usize>,
}

fn number_layout(toks: &[Tok]) -> NumberLayout {
    let n = toks.len();
    let point = toks.iter().position(|t| matches!(t, Tok::Point));
    let exp = toks.iter().position(|t| matches!(t, Tok::Exp { .. }));
    let int_end = point.or(exp).unwrap_or(n);
    let frac_end = exp.unwrap_or(n);
    let int_idx = digit_indices(toks, 0..int_end);
    let frac_idx = match point {
        Some(p) => digit_indices(toks, p + 1..frac_end),
        None => Vec::new(),
    };
    NumberLayout { point, exp, int_idx, frac_idx }
}

/// The number a section works on: each `%` multiplies by 100, each scaling comma divides by 1000.
fn scaled(sec: &Section, mut x: f64) -> f64 {
    // Observed: `%` in a scientific section is only a literal (`#%e+#` shows 123456.789 as
    // `1%e+5`).
    if sec.percent > 0 && !sec.exp {
        x *= 100f64.powi(sec.percent.min(20));
    }
    if sec.scale > 0 {
        x /= 1000f64.powi(sec.scale.min(110));
    }
    x
}

/// Scientific notation of a non-negative `x` with `int_count` integer and `frac_count` fraction
/// placeholders: the exponent (a multiple of the integer placeholder count, engineering style)
/// and the mantissa's integer and fraction digits.
fn scientific_parts(x: f64, int_count: usize, frac_count: usize) -> (i32, String, String) {
    if x == 0.0 {
        // Observed: zero shows `0` in every integer placeholder, `#` and `?` too
        // (`#.#E+#` gives `0.E+0`, `####.####E+#` gives `0000.E+0`).
        return (0, "0".repeat(int_count), "0".repeat(frac_count));
    }
    let step = int_count.max(1) as i32;
    let (digits, xe) = decimal_digits(x);
    let mut e = xe - xe.rem_euclid(step);
    let (mut i, mut f) = fixed_from_digits(&digits, xe - e, frac_count);
    if i.len() > step as usize {
        e += step;
        (i, f) = fixed_from_digits(&digits, xe - e, frac_count);
    }
    (e, i, f)
}

/// The number a number section shows for a non-negative `v`, as a value: what "Set precision as
/// displayed" stores. `None` when the section shows no digits (`"x"`) or the result isn't finite.
/// It uses the display's own rounding, so 1.005 with `0.00` gives 1.01, and 0.123456 with
/// `0.00%` gives 0.1235 (the nearest double to the decimal shown).
pub(crate) fn shown_value(sec: &Section, v: f64) -> Option<f64> {
    if !sec.has_digits || !v.is_finite() {
        return None;
    }
    let x = scaled(sec, v.abs());
    if !x.is_finite() {
        return None;
    }
    if x == 0.0 {
        return Some(0.0);
    }
    // `%` and scaling are undone by moving the decimal point in the digits shown, so the result
    // is the nearest double to that decimal rather than a product with its own rounding error.
    let pct = if sec.exp { 0 } else { sec.percent.min(20) };
    let shift = 3 * sec.scale.clamp(0, 110) - 2 * pct;
    let shown = if sec.fraction {
        let lay = fraction_layout(&sec.toks)?;
        let (whole, num, den) = fraction_parts(&lay, x);
        let den = den.max(1) as f64;
        (whole * den + num) / den * 10f64.powi(shift)
    } else {
        let NumberLayout { exp, frac_idx, .. } = number_layout(&sec.toks);
        let (e, int_s, frac_s) = match exp {
            // Observed: the stored value keeps one digit plus the fraction digits, whatever the
            // integer placeholders (`##0.0E+0` stores 123456.789 as 120000 though it shows
            // `123.5E+3`; `###.00E+0` keeps three digits). A `%` in a scientific section stores a
            // value that matches neither the display nor this rule: left as is.
            Some(_) if sec.percent > 0 => return None,
            Some(_) => scientific_parts(x, 1, frac_idx.len()),
            None => {
                let (i, f) = fixed(x, frac_idx.len());
                (0, i, f)
            }
        };
        format!("0{int_s}.{frac_s}0e{}", i64::from(e) + i64::from(shift)).parse::<f64>().ok()?
    };
    shown.is_finite().then_some(shown)
}

/// Renders a non-negative number through a number section.
pub(crate) fn render_number(sec: &Section, v: f64) -> Out {
    let mut x = scaled(sec, v.abs());
    if !x.is_finite() {
        x = f64::MAX;
    }
    let toks = &sec.toks;
    let n = toks.len();
    // Pieces replace the token's own output; `before` is inserted ahead of a token; literal
    // replacements blank the literals of a hidden fraction part (see [`Parts`]).
    let mut parts = Parts::new(n);
    let mut zero = false;
    if sec.fraction {
        render_fraction(sec, x, &mut parts);
    } else {
        let NumberLayout { point, exp, int_idx, frac_idx } = number_layout(toks);
        let (int_s, frac_s) = if let Some(e_at) = exp {
            let exp_idx = digit_indices(toks, e_at + 1..n);
            let (mut e, int_s, frac_s) = scientific_parts(x, int_idx.len(), frac_idx.len());
            // Exponent text.
            let (upper, plus) = match toks.get(e_at) {
                Some(Tok::Exp { upper, plus }) => (*upper, *plus),
                _ => (true, true),
            };
            if int_s.is_empty() && frac_s.chars().all(|c| c == '0') {
                e = 0;
            }
            let sign = if e < 0 {
                "-"
            } else if plus {
                "+"
            } else {
                ""
            };
            let digits = e.unsigned_abs().to_string();
            // The sign sits right before the exponent digits, after any literal that follows the
            // `E` (observed: `|0|e+|0|` shows `|1|e|+5|`); `?` pads the digits with spaces.
            match exp_idx.first() {
                Some(&first) => {
                    parts.set_piece(e_at, if upper { "E" } else { "e" });
                    parts.set_before(first, sign);
                    fill_int(toks, &exp_idx, &digits, false, false, &mut parts);
                }
                None => {
                    let start = parts.mark();
                    parts.buf.push(if upper { 'E' } else { 'e' });
                    parts.buf.push_str(sign);
                    parts.buf.push_str(&digits);
                    parts.end_piece(e_at, start);
                }
            }
            (int_s, frac_s)
        } else {
            fixed(x, frac_idx.len())
        };
        zero = int_s.chars().chain(frac_s.chars()).all(|c| c == '0');
        fill_int(toks, &int_idx, &int_s, sec.thousands, false, &mut parts);
        if int_idx.is_empty()
            && !int_s.is_empty()
            && let Some(p) = point.or(exp)
        {
            parts.set_before(p, &with_commas(&int_s, sec.thousands));
        }
        fill_frac(toks, &frac_idx, &frac_s, &mut parts);
        if let Some(p) = point {
            parts.set_piece(p, ".");
        }
    }
    let mut out = Out::default();
    for (i, t) in toks.iter().enumerate() {
        out.push(parts.before(i));
        match t {
            Tok::Lit(s) => match parts.slots.get(i).and_then(|sl| sl.lit_spaces) {
                Some(k) => out.text.extend(std::iter::repeat_n(' ', k)),
                None => out.push(s),
            },
            Tok::Percent => out.push("%"),
            Tok::Fill(c) => out.fill(*c),
            // Every other token shows its piece. `At` (`0@`) is reachable here, `Comma`, `General`
            // and `Date` are not (analysis turns commas into literals and routes sections with
            // dates or General elsewhere); none of them is ever given a piece, so they print "".
            _ => out.push(parts.piece(i)),
        }
    }
    out.zero = zero;
    out
}

/// Best rational approximation of `x` with denominator at most `max_den` (continued fractions).
fn approximate(x: f64, max_den: u64) -> (u64, u64) {
    if x.is_nan() || x <= 0.0 || !x.is_finite() {
        return (0, 1);
    }
    if x >= 1e15 {
        return (x.round() as u64, 1);
    }
    let (mut p0, mut q0, mut p1, mut q1) = (0u64, 1u64, 1u64, 0u64);
    let mut r = x;
    for _ in 0..64 {
        let a_f = r.floor();
        if a_f > 1e15 {
            break;
        }
        let a = a_f as u64;
        let Some(q2) = a.checked_mul(q1).and_then(|v| v.checked_add(q0)) else { break };
        if q2 > max_den {
            break;
        }
        let Some(p2) = a.checked_mul(p1).and_then(|v| v.checked_add(p0)) else { break };
        (p0, q0, p1, q1) = (p1, q1, p2, q2);
        let frac = r - a_f;
        if frac < 1e-12 {
            break;
        }
        r = 1.0 / frac;
    }
    if q1 == 0 {
        return (x.round() as u64, 1);
    }
    // Semiconvergent bound.
    let k = (max_den - q0) / q1;
    let (pb, qb) = (p0.saturating_add(k.saturating_mul(p1)), q0.saturating_add(k.saturating_mul(q1)));
    let err_a = (p1 as f64 / q1 as f64 - x).abs();
    let err_b = if qb > 0 { (pb as f64 / qb as f64 - x).abs() } else { f64::INFINITY };
    if err_b < err_a { (pb, qb) } else { (p1, q1) }
}

/// Display width of a token in a fraction section, for blanking a hidden part with spaces.
fn shown_width(t: &Tok) -> usize {
    match t {
        Tok::Lit(s) | Tok::FixedDenom(s) => s.chars().count(),
        Tok::Digit(_) | Tok::Slash => 1,
        _ => 0,
    }
}

/// Replaces the literals in `range` with spaces (`spaces`) or nothing; placeholders and the bar
/// in it get the same treatment as pieces.
fn blank_range(toks: &[Tok], range: std::ops::Range<usize>, spaces: bool, parts: &mut Parts) {
    for i in range {
        let Some(t) = toks.get(i) else { break };
        let width = if spaces { shown_width(t) } else { 0 };
        match t {
            Tok::Lit(_) => parts.set_lit_spaces(i, width),
            Tok::Digit(_) | Tok::Slash | Tok::FixedDenom(_) => parts.set_spaces(i, width),
            _ => {}
        }
    }
}

/// Where a fraction section's parts are (see [`render_fraction`]).
struct FractionLayout {
    slash: usize,
    num_start: usize,
    whole_idx: Vec<usize>,
    num_idx: Vec<usize>,
    sep_start: usize,
    fixed_den: Option<u64>,
    den_start: usize,
    den_end: usize,
    den_idx: Vec<usize>,
}

fn fraction_layout(toks: &[Tok]) -> Option<FractionLayout> {
    let is_digit = |i: usize| matches!(toks.get(i), Some(Tok::Digit(_)));
    let slash = toks.iter().position(|t| matches!(t, Tok::Slash))?;
    let num_last = (0..slash).rev().find(|&i| is_digit(i))?;
    let mut num_start = num_last;
    while num_start > 0 && is_digit(num_start - 1) {
        num_start -= 1;
    }
    let whole_idx = digit_indices(toks, 0..num_start);
    let num_idx = digit_indices(toks, num_start..num_last + 1);
    let sep_start = whole_idx.last().map_or(num_start, |&w| w + 1);
    let (fixed_den, den_start): (Option<u64>, usize) = match toks.get(slash + 1) {
        Some(Tok::FixedDenom(s)) => (Some(s.parse::<u64>().unwrap_or(u64::MAX).clamp(1, 1_000_000_000)), slash + 1),
        _ => {
            let mut i = slash + 1;
            while matches!(toks.get(i), Some(Tok::Lit(_))) {
                i += 1;
            }
            (None, i)
        }
    };
    let mut den_end = den_start;
    if fixed_den.is_some() {
        den_end += 1;
    } else {
        while is_digit(den_end) {
            den_end += 1;
        }
    }
    let den_idx = digit_indices(toks, den_start..den_end);
    Some(FractionLayout { slash, num_start, whole_idx, num_idx, sep_start, fixed_den, den_start, den_end, den_idx })
}

/// The whole part, numerator and denominator a fraction section shows for a non-negative `x`.
fn fraction_parts(lay: &FractionLayout, x: f64) -> (f64, f64, u64) {
    let has_whole = !lay.whole_idx.is_empty();
    let (mut whole, f) = if has_whole { (x.floor(), x - x.floor()) } else { (0.0, x) };
    // Without a whole part the numerator carries the whole value and can pass u64::MAX, so it
    // stays an f64 (a float-to-u64 cast saturates).
    let (mut num, den): (f64, u64) = match lay.fixed_den {
        Some(d) => ((f * d as f64).round(), d),
        None if f >= 1e15 => (f.round(), 1),
        None => {
            let digits = lay.den_idx.len().clamp(1, 7) as u32;
            let (n, d) = approximate(f, 10u64.pow(digits) - 1);
            (n as f64, d)
        }
    };
    if has_whole && num >= den as f64 && den > 0 {
        // The fractional part is below 1, so the numerator is at most the denominator here.
        let n = num as u64;
        whole += (n / den) as f64;
        num = (n % den) as f64;
    }
    (whole, num, den)
}

/// A fraction section is `[whole] [sep] num [lit] / [lit] den`: the numerator is the run of
/// placeholders ending at the last one before the bar, the whole part is every placeholder
/// before it (literals between them stay), `sep` the literals between the two parts.
///
/// Observed Excel behaviour: when the whole part shows no digit its separator disappears (or
/// becomes spaces when the whole part or the numerator has a `?`); when the fraction is zero and
/// the numerator has no `0` placeholder, the separator, numerator, bar and denominator disappear
/// (or become spaces when any part has a `?`) and the whole part shows at least `0`.
fn render_fraction(sec: &Section, x: f64, parts: &mut Parts) {
    let toks = &sec.toks;
    let Some(lay) = fraction_layout(toks) else { return };
    let (whole, num, den) = fraction_parts(&lay, x);
    let FractionLayout { slash, num_start, whole_idx, num_idx, sep_start, fixed_den, den_start, den_end, den_idx } = lay;
    let has_q = |idx: &[usize]| idx.iter().any(|&i| toks.get(i) == Some(&Tok::Digit('?')));
    let has_whole = !whole_idx.is_empty();
    let whole_s = if whole >= 1.0 { fixed(whole, 0).0 } else { String::new() };
    let num_zero_ph = num_idx.iter().any(|&i| toks.get(i) == Some(&Tok::Digit('0')));
    if has_whole && num == 0.0 && !num_zero_ph {
        fill_int(toks, &whole_idx, &whole_s, sec.thousands, true, parts);
        let spaces = has_q(&whole_idx) || has_q(&num_idx) || has_q(&den_idx);
        blank_range(toks, sep_start..den_end, spaces, parts);
        return;
    }
    if has_whole {
        // A zero value shows `0` in a `?` or `0` units place, nothing in a `#` one.
        let units_hash = whole_idx.last().is_some_and(|&i| toks.get(i) == Some(&Tok::Digit('#')));
        let force = whole_s.is_empty() && num == 0.0 && !units_hash;
        fill_int(toks, &whole_idx, &whole_s, sec.thousands, force, parts);
        let whole_shown = whole_idx.iter().any(|&i| parts.piece(i).chars().any(|c| c.is_ascii_digit()));
        if !whole_shown {
            let spaces = has_q(&whole_idx) || has_q(&num_idx);
            blank_range(toks, sep_start..num_start, spaces, parts);
        }
    }
    let num_s = if num < 1e15 { (num as u64).to_string() } else { fixed(num, 0).0 };
    fill_int(toks, &num_idx, &num_s, false, true, parts);
    parts.set_piece(slash, "/");
    let den_s = den.to_string();
    if fixed_den.is_some() {
        parts.set_piece(den_start, &den_s);
    } else {
        // Denominator digits are zero padded to the `0` placeholders, then left aligned; `?`
        // pads with spaces after them.
        let zeros = den_idx.iter().filter(|&&i| toks.get(i) == Some(&Tok::Digit('0'))).count();
        // No `{:0>zeros$}`: a format width above u16::MAX panics.
        let mut padded = "0".repeat(zeros.saturating_sub(den_s.chars().count()));
        padded.push_str(&den_s);
        let dc: Vec<char> = padded.chars().collect();
        let count = den_idx.len();
        for (k, &ti) in den_idx.iter().enumerate() {
            let s = if k + 1 == count {
                dc.get(k..).map(|r| r.iter().collect::<String>()).unwrap_or_default()
            } else {
                dc.get(k).map(ToString::to_string).unwrap_or_default()
            };
            let s = if s.is_empty() {
                match toks.get(ti) {
                    Some(Tok::Digit('?')) => " ".into(),
                    Some(Tok::Digit('0')) => "0".into(),
                    _ => String::new(),
                }
            } else {
                s
            };
            parts.set_piece(ti, &s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approximations() {
        assert_eq!(approximate(0.5, 9), (1, 2));
        assert_eq!(approximate(0.333333, 9), (1, 3));
        assert_eq!(approximate(std::f64::consts::PI - 3.0, 99), (14, 99));
        assert_eq!(approximate(std::f64::consts::PI - 3.0, 9), (1, 7));
        assert_eq!(approximate(0.0, 9), (0, 1));
        assert_eq!(approximate(0.999, 9), (1, 1));
    }
}
