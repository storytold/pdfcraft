//! GridCraft number formats: Excel format codes (`#,##0.00;[Red](#,##0.00)`, dates, fractions,
//! scientific…), the General display format and the `TEXT()` worksheet function. en-US only.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod calendar;
mod decimal;
mod parse;
mod render;

use std::sync::Arc;

use gridcraft_core::{CellError, DateSystem, Value};

pub use decimal::format_general_fit;

use parse::{DatePart, SecKind, Section, Tok};
use render::Out;

/// Width (in characters) of General output in a cell.
const GENERAL_WIDTH: usize = 11;
/// What Excel paints when a date is out of range.
const OVERFLOW: &str = "########";

/// Colour from a `[Red]` or `[ColorN]` section tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormatColor {
    Black,
    Blue,
    Cyan,
    Green,
    Magenta,
    Red,
    White,
    Yellow,
    /// `[Color1]`..`[Color56]` (palette index).
    Indexed(u8),
}

impl FormatColor {
    /// RGB value used to render this number-format color.
    pub fn rgb(self) -> Option<[u8; 3]> {
        use FormatColor as F;
        Some(match self {
            F::Black => [0, 0, 0],
            F::Blue => [0, 0, 255],
            F::Cyan => [0, 255, 255],
            F::Green => [0, 255, 0],
            F::Magenta => [255, 0, 255],
            F::Red => [255, 0, 0],
            F::White => [255, 255, 255],
            F::Yellow => [255, 255, 0],
            F::Indexed(index) => return gridcraft_core::color::indexed_color(index),
        })
    }
}

/// Best guess at the ribbon's number-format category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormatKind {
    General,
    Number,
    Currency,
    Accounting,
    Date,
    Time,
    Percentage,
    Fraction,
    Scientific,
    Text,
    /// Zip codes, phone numbers, social security numbers.
    Special,
    Custom,
}

/// A formatted value ready to paint.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Formatted {
    pub text: String,
    pub color: Option<FormatColor>,
    /// Repeat-fill char from `*x` and its byte position in `text`, for the renderer to expand to
    /// the cell width.
    pub fill: Option<(char, usize)>,
    /// True when the result should be right-aligned by default (numbers); General text is left
    /// aligned.
    pub numeric: bool,
}

#[derive(Debug)]
struct Inner {
    code: String,
    general: bool,
    sections: Vec<Section>,
    /// Sections that apply to numbers (a trailing text section is excluded).
    num_sections: usize,
    /// Index of the section that applies to text, if any.
    text_section: Option<usize>,
    /// A code Excel rejects (a misplaced `B1`/`B2` calendar prefix): shown as General, and
    /// `TEXT` returns `#VALUE!`, as for a code that is too long.
    invalid: bool,
}

/// A parsed number format code. Cheap to clone.
#[derive(Clone, Debug)]
pub struct NumberFormat {
    inner: Arc<Inner>,
}

impl PartialEq for NumberFormat {
    fn eq(&self, other: &Self) -> bool {
        self.inner.code == other.inner.code
    }
}

impl Default for NumberFormat {
    fn default() -> Self {
        NumberFormat::general()
    }
}

/// Longest number format code, in characters (not bytes), that is parsed: Excel's own limit.
/// A longer code is invalid: [`NumberFormat::parse`] returns a General format (the cell shows
/// its value unformatted; the stored code is kept as is) and `TEXT` returns `#VALUE!`. The cap
/// also bounds every allocation that scales with the number of placeholders in a code.
pub const MAX_FORMAT_CODE_CHARS: usize = 255;

/// True when `code` is longer than [`MAX_FORMAT_CODE_CHARS`] characters.
fn too_long(code: &str) -> bool {
    // A code of more than 255 bytes can still be short in characters; count only when needed.
    code.len() > MAX_FORMAT_CODE_CHARS && code.chars().count() > MAX_FORMAT_CODE_CHARS
}

impl NumberFormat {
    /// Parses a format code. Never fails: unknown or malformed parts degrade to literal text, and
    /// a code longer than [`MAX_FORMAT_CODE_CHARS`] characters, or one Excel rejects for a
    /// misplaced `B1`/`B2` calendar prefix, degrades to General.
    pub fn parse(code: &str) -> NumberFormat {
        Self::parse_inner(code, false)
    }

    /// `parse` without the length cap, so tests can reach renderer paths a capped code cannot.
    #[cfg(test)]
    fn parse_uncapped(code: &str) -> NumberFormat {
        Self::parse_inner(code, true)
    }

    fn parse_inner(code: &str, uncapped: bool) -> NumberFormat {
        #[cfg(test)]
        parse_cache::count_parse();
        let trimmed = code.trim();
        // `checkbox` marks a checkbox cell (Insert › Checkbox): TRUE/FALSE draw as a box, any other
        // value displays as General.
        let general = |invalid: bool| NumberFormat {
            inner: Arc::new(Inner { code: code.to_string(), general: true, sections: Vec::new(), num_sections: 0, text_section: None, invalid }),
        };
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("general") || trimmed == "checkbox" || (!uncapped && too_long(code)) {
            return general(false);
        }
        let sections = parse::parse_sections(code);
        if sections.iter().any(|s| s.invalid) {
            return general(true);
        }
        let len = sections.len();
        let last_is_text = sections.last().is_some_and(|s| s.kind == SecKind::Text);
        let text_section = if len >= 4 {
            Some(3)
        } else if last_is_text {
            Some(len - 1)
        } else {
            None
        };
        let num_sections = if len >= 4 {
            3
        } else if last_is_text {
            len - 1
        } else {
            len
        };
        NumberFormat { inner: Arc::new(Inner { code: code.to_string(), general: false, sections, num_sections, text_section, invalid: false }) }
    }

    pub fn general() -> NumberFormat {
        NumberFormat::parse("General")
    }

    pub fn code(&self) -> &str {
        &self.inner.code
    }

    pub fn is_general(&self) -> bool {
        self.inner.general
    }

    fn num_sections(&self) -> &[Section] {
        self.inner.sections.get(..self.inner.num_sections).unwrap_or(&[])
    }

    /// True when the code contains date or time tokens.
    pub fn is_date(&self) -> bool {
        self.num_sections().iter().any(|s| s.kind == SecKind::Date)
    }

    /// For a date or time format, whether the section that formats `value` shows calendar parts (year, month,
    /// day) and whether it shows time parts, as `(date, time)`. `None` for any other format and for
    /// elapsed-time codes like `[h]:mm`, whose value is a duration, not a moment.
    pub fn date_shape(&self, value: f64) -> Option<(bool, bool)> {
        let s = self.select(value).map(|(s, _)| s).filter(|s| s.kind == SecKind::Date)?;
        let parts = || s.toks.iter().filter_map(|t| if let Tok::Date(d) = t { Some(*d) } else { None });
        if parts().any(|d| matches!(d, DatePart::ElapsedH(_) | DatePart::ElapsedM(_) | DatePart::ElapsedS(_))) {
            return None;
        }
        Some((parts().any(DatePart::is_calendar), parts().any(|d| !d.is_calendar())))
    }

    pub fn is_percent(&self) -> bool {
        self.num_sections().iter().any(|s| s.kind == SecKind::Number && s.percent > 0)
    }

    /// True for a pure text format such as `@`.
    pub fn is_text(&self) -> bool {
        self.inner.num_sections == 0 && self.inner.text_section.is_some()
    }

    /// Best guess at the format category for the ribbon's dropdown.
    pub fn kind(&self) -> FormatKind {
        if self.is_general() {
            return FormatKind::General;
        }
        if self.is_text() {
            return FormatKind::Text;
        }
        const SPECIAL: [&str; 4] = ["00000", "00000-0000", "[<=9999999]###-####;(###) ###-####", "000-00-0000"];
        if SPECIAL.contains(&self.code()) {
            return FormatKind::Special;
        }
        let Some(s) = self.inner.sections.first() else { return FormatKind::Custom };
        match s.kind {
            SecKind::General => return FormatKind::General,
            SecKind::Text => return FormatKind::Text,
            SecKind::Date => {
                let calendar = s.toks.iter().any(|t| matches!(t, Tok::Date(d) if d.is_calendar()));
                return if calendar { FormatKind::Date } else { FormatKind::Time };
            }
            SecKind::Number => {}
        }
        if s.fraction {
            FormatKind::Fraction
        } else if s.exp {
            FormatKind::Scientific
        } else if s.percent > 0 {
            FormatKind::Percentage
        } else if s.fill && s.has_digits {
            FormatKind::Accounting
        } else if s.currency && s.has_digits {
            FormatKind::Currency
        } else if s.has_digits {
            FormatKind::Number
        } else {
            FormatKind::Custom
        }
    }

    /// Picks the number section for `v`. Returns the section and whether the minus sign is
    /// dropped (the default negative section shows the absolute value). `None` = General.
    fn select(&self, v: f64) -> Option<(&Section, bool)> {
        let secs = self.num_sections();
        let k = secs.len();
        let s0 = secs.first()?;
        let c0 = s0.cond;
        let c1 = secs.get(1).and_then(|s| s.cond);
        if c0.is_some() || c1.is_some() {
            let idx = if c0.is_some_and(|c| c.test(v)) {
                0
            } else if c1.is_some_and(|c| c.test(v)) {
                1
            } else if c0.is_some() && c1.is_some() {
                2
            } else if c0.is_none() {
                0
            } else {
                1
            };
            // Excel never adds a minus sign in a multi-section conditional format.
            return secs.get(idx).map(|s| (s, k >= 2));
        }
        if v < 0.0 && k >= 2 {
            return secs.get(1).map(|s| (s, true));
        }
        if v == 0.0 && k >= 3 {
            return secs.get(2).map(|s| (s, false));
        }
        Some((s0, false))
    }

    /// Formats a number. The flag is true when the value cannot be shown (date out of range).
    /// With `full`, General is not narrowed to the cell width but written by [`general_full`].
    fn format_number(&self, n: f64, sys: DateSystem, full: bool) -> (Formatted, bool) {
        let general_text = |n: f64, width: usize| if full { Some(general_full(n)) } else { format_general_fit(n, width) };
        let general =
            |n: f64| Formatted { text: general_text(n, GENERAL_WIDTH).unwrap_or_else(|| OVERFLOW.into()), color: None, fill: None, numeric: true };
        if self.is_general() {
            return (general(n), false);
        }
        let Some((sec, drop_sign)) = self.select(n) else { return (general(n), false) };
        let minus = n < 0.0 && !drop_sign;
        let out: Option<Out> = match sec.kind {
            SecKind::Text => Some(render::render_text(sec, &general_text(n, GENERAL_WIDTH).unwrap_or_default())),
            SecKind::General => {
                let width = if minus { GENERAL_WIDTH - 1 } else { GENERAL_WIDTH };
                let text = general_text(n.abs(), width).unwrap_or_else(|| "#".repeat(width.clamp(1, 11)));
                Some(render::render_general(sec, &text))
            }
            SecKind::Date => {
                if minus {
                    None
                } else {
                    render::render_date(sec, n.abs(), sys)
                }
            }
            SecKind::Number => Some(render::render_number(sec, n.abs())),
        };
        let Some(mut out) = out else {
            return (Formatted { text: OVERFLOW.into(), color: sec.color, fill: None, numeric: true }, true);
        };
        // Excel drops the minus sign when the first section shows a negative number as all zeros.
        if minus && sec.kind != SecKind::Text && !out.zero {
            out.prepend("-");
        }
        (Formatted { text: out.text, color: sec.color, fill: out.fill, numeric: true }, false)
    }

    /// The number `n` as this format displays it: what Excel stores when "Set precision as
    /// displayed" is on. Fixed decimals, percent, thousands scaling (`0.0,,`), scientific and
    /// fraction sections round to what they show; the section is picked as for display, so
    /// `0.00;(0.0)` keeps one decimal for negatives. Observed in Excel (COM,
    /// `Workbook.PrecisionAsDisplayed`), these keep the full value: General, date and time
    /// formats (`mm:ss.0` included), text formats and sections without digits (`"x"`, `;;;`).
    /// The section is picked by [`NumberFormat::rounding_section`], which ignores conditions.
    /// Never returns `-0`.
    pub fn round_as_displayed(&self, n: f64) -> f64 {
        if self.is_general() || !n.is_finite() {
            return n;
        }
        let Some(sec) = self.rounding_section(n) else { return n };
        if sec.kind != SecKind::Number {
            return n;
        }
        let Some(m) = render::shown_value(sec, n) else { return n };
        if m == 0.0 {
            0.0
        } else if n < 0.0 {
            -m
        } else {
            m
        }
    }

    /// The section whose digits "Set precision as displayed" rounds `n` by. Observed in Excel
    /// (COM), this differs from the section that displays `n`:
    /// - Conditions are ignored; the sign picks the section, the first for positives and the
    ///   second for negatives. `[<1]0.00;0` shows 2.333 as `2` but stores 2.33, and stores -10/3
    ///   as -3 although it shows `-3.00`. `[>1]0.0;0.000` stores 1/3 as 0.3.
    /// - A format with a condition and a single number section keeps negatives at full value
    ///   (`[<1]0.00`, `[>=-100]0.0`); with an `@` section added (`[<1]0.00;@`) negatives use the
    ///   first section, as every format without a condition does.
    /// - A completely empty section (no characters, not even a colour) passes to the next one:
    ///   `0.0;;0.000` stores -2.333 as -2.333, `;0.0` stores 2.333 as 2.3, `;;0.0` too. A section
    ///   with only a colour or a literal (`0.0;[Red];0.000`, `0.0;" "`) has no digits and keeps
    ///   the full value.
    ///
    /// `None` keeps the full value.
    fn rounding_section(&self, n: f64) -> Option<&Section> {
        let secs = self.num_sections();
        let conditional = secs.iter().any(|s| s.cond.is_some());
        let slot = if n < 0.0 && secs.len() >= 2 {
            1
        } else if n < 0.0 && conditional && self.inner.text_section.is_none() {
            return None;
        } else {
            0
        };
        let empty = |s: &Section| s.toks.is_empty() && s.color.is_none() && s.cond.is_none();
        secs.get(slot..)?.iter().find(|s| !empty(s))
    }

    fn format_text(&self, t: &str) -> Formatted {
        match self.inner.text_section.and_then(|i| self.inner.sections.get(i)) {
            Some(sec) => {
                let out = render::render_text(sec, t);
                Formatted { text: out.text, color: sec.color, fill: out.fill, numeric: false }
            }
            None => Formatted { text: t.to_string(), color: None, fill: None, numeric: false },
        }
    }

    /// [`Self::format_text`]'s text, or `None` once it passes `max_units` UTF-16 code units.
    fn format_text_within(&self, t: &str, max_units: usize) -> Option<String> {
        match self.inner.text_section.and_then(|i| self.inner.sections.get(i)) {
            Some(sec) => render::render_text_within(sec, t, max_units).map(|out| out.text),
            None => (t.encode_utf16().count() <= max_units).then(|| t.to_string()),
        }
    }
}

/// Formats a cell value for display.
pub fn format_value(v: &Value, fmt: &NumberFormat, sys: DateSystem) -> Formatted {
    format_value_with(v, fmt, sys, false)
}

/// Formats a cell value as Save As CSV/Text writes it: like [`format_value`], except that a number
/// shown as General (a General or Text section, with or without colour and literals) is not
/// narrowed to the cell width but written by `general_full`: `124412140710`, not `1.24412E+11`.
/// Excel writes the cell display here; GridCraft keeps the digits.
pub fn format_value_full(v: &Value, fmt: &NumberFormat, sys: DateSystem) -> Formatted {
    format_value_with(v, fmt, sys, true)
}

/// General for Save As CSV/Text: the text `&` gives (15 significant digits), but never with an
/// exponent, because reopening the file would give the cell a scientific format that rounds it
/// on the next save. Where `&` writes an exponent, a number of 1 or more is written as the
/// shortest digits that read back as the same value (every integer f64 holds below 1e16 exactly,
/// so 16-digit card numbers survive), and a smaller one as its 15 significant digits in full.
fn general_full(n: f64) -> String {
    let text = gridcraft_core::number_to_text(n);
    if !text.contains('E') {
        text
    } else if n.abs() >= 1.0 {
        format!("{n}")
    } else {
        let mag: i32 = format!("{n:.14e}").split_once('e').and_then(|(_, e)| e.parse().ok()).unwrap_or(0);
        gridcraft_core::value::trim_decimal(&format!("{:.*}", (14 - mag).max(0) as usize, n))
    }
}

fn format_value_with(v: &Value, fmt: &NumberFormat, sys: DateSystem, full: bool) -> Formatted {
    match v {
        Value::Empty => Formatted::default(),
        Value::Number(n) if !n.is_finite() => Formatted { text: CellError::Num.as_str().into(), ..Formatted::default() },
        Value::Number(n) => fmt.format_number(*n, sys, full).0,
        Value::Text(t) => fmt.format_text(t),
        Value::Bool(b) => Formatted { text: if *b { "TRUE" } else { "FALSE" }.into(), ..Formatted::default() },
        Value::Error(e) => Formatted { text: e.as_str().into(), ..Formatted::default() },
        Value::Array(a) => match a.data.first() {
            Some(Value::Array(_)) | None => Formatted::default(),
            Some(first) => format_value_with(first, fmt, sys, full),
        },
    }
}

/// A small per-thread cache of parsed format codes for callers that format many values with the
/// same code (`TEXT` over a range, chart labels). A parsed format depends on the code text only
/// (the date system and values are applied when formatting), so the code is the whole key.
mod parse_cache {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::NumberFormat;

    /// Most codes kept per thread. A full cache is emptied, so memory stays bounded.
    pub(super) const CAPACITY: usize = 64;
    /// Longer codes are parsed every time and never stored.
    pub(super) const MAX_CODE_LEN: usize = 256;

    thread_local! {
        static CACHE: RefCell<HashMap<String, NumberFormat>> = RefCell::new(HashMap::new());
    }

    pub(super) fn get(code: &str) -> NumberFormat {
        if code.len() > MAX_CODE_LEN {
            return NumberFormat::parse(code);
        }
        // Not usable (thread shutting down, re-entered): parse without the cache.
        let cached = CACHE
            .try_with(|c| {
                let mut c = c.try_borrow_mut().ok()?;
                if let Some(f) = c.get(code) {
                    return Some(f.clone());
                }
                let f = NumberFormat::parse(code);
                if c.len() >= CAPACITY {
                    c.clear();
                }
                c.insert(code.to_string(), f.clone());
                Some(f)
            })
            .ok()
            .flatten();
        cached.unwrap_or_else(|| NumberFormat::parse(code))
    }

    #[cfg(test)]
    thread_local! {
        static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[cfg(test)]
    pub(super) fn count_parse() {
        let _ = PARSES.try_with(|p| p.set(p.get() + 1));
    }

    /// Parses done on this thread so far (tests only).
    #[cfg(test)]
    pub(super) fn parses() -> usize {
        PARSES.try_with(std::cell::Cell::get).unwrap_or(0)
    }

    /// Empties this thread's cache (tests only).
    #[cfg(test)]
    pub(super) fn clear() {
        let _ = CACHE.try_with(|c| c.borrow_mut().clear());
    }
}

impl NumberFormat {
    /// Like [`NumberFormat::parse`], but remembers recently parsed codes (per thread, bounded) so
    /// formatting many values with one code parses it once. Same result as `parse`.
    pub fn parse_cached(code: &str) -> NumberFormat {
        parse_cache::get(code)
    }
}

/// The `TEXT()` worksheet function: formats a value with a format code. Numeric text is
/// converted to a number first; other text and booleans (as `TRUE`/`FALSE`) go through the text
/// section, or are returned as is.
/// Longest text a cell holds, in UTF-16 code units (Excel's 32,767 characters, which XLSB also
/// stores that way), and so the longest `TEXT` result.
const MAX_CELL_TEXT_CHARS: usize = 32_767;

pub fn text_function(v: &Value, code: &str, sys: DateSystem) -> Result<String, CellError> {
    if code.is_empty() {
        return Ok(String::new());
    }
    if too_long(code) {
        return Err(CellError::Value);
    }
    let fmt = NumberFormat::parse_cached(code);
    if fmt.inner.invalid {
        return Err(CellError::Value);
    }
    let n = match v.scalar() {
        Value::Error(e) => return Err(e),
        Value::Empty => 0.0,
        Value::Number(n) => n,
        Value::Bool(b) => return Ok(fmt.format_text(if b { "TRUE" } else { "FALSE" }).text),
        Value::Text(t) => match gridcraft_core::parse::parse_number_text(&t) {
            Some(n) => n,
            // Every `@` repeats the text: a result past a cell's limit is `#VALUE!`, so nested
            // TEXT calls can't multiply it further.
            None => return fmt.format_text_within(&t, MAX_CELL_TEXT_CHARS).ok_or(CellError::Value),
        },
        Value::Array(_) => return Err(CellError::Value),
    };
    if !n.is_finite() {
        return Err(CellError::Num);
    }
    let (f, overflow) = fmt.format_number(n, sys, false);
    if overflow { Err(CellError::Value) } else { Ok(f.text) }
}

#[cfg(test)]
mod tests;
