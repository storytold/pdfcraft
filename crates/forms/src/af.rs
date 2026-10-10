//! Acrobat form formatting, validation and calculation without a JavaScript engine.
//!
//! Real forms carry these as JavaScript calls to the AF functions that Acrobat's JavaScript API
//! reference documents (`AFNumber_Format`, `AFPercent_Format`, `AFDate_FormatEx`,
//! `AFTime_Format`, `AFSpecial_Format`, `AFSpecial_KeystrokeEx`, `AFRange_Validate`,
//! `AFSimple_Calculate`, and simplified field notation). Acrobat itself writes exactly these
//! calls from the Format, Validate and Calculate tabs. This module recognises them, runs them
//! natively (format for display, keystroke check on commit, validation, calculation), and
//! writes them back when a form is authored. Other scripts are kept untouched and reported as
//! unsupported; they need the JavaScript engine (M6.4).

use std::fmt::Write as _;

/// The Format tab.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Format {
    #[default]
    None,
    /// `AFNumber_Format(dec, sepStyle, negStyle, currStyle, strCurrency, bCurrencyPrepend)`.
    Number { decimals: u8, sep: u8, neg: u8, currency: String, prepend: bool },
    /// `AFPercent_Format(dec, sepStyle)`.
    Percent { decimals: u8, sep: u8 },
    /// `AFDate_FormatEx(cFormat)`.
    Date(String),
    /// `AFTime_FormatEx(cFormat)`.
    Time(String),
    /// `AFSpecial_Format(n)`: 0 zip, 1 zip+4, 2 phone, 3 SSN.
    Special(u8),
    /// `AFSpecial_KeystrokeEx(cMask)`: 9 digit, A letter, O letter or digit, X any.
    Mask(String),
}

/// The Validate tab.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Validate {
    #[default]
    None,
    /// `AFRange_Validate(bGreater, nGreater, bLess, nLess)`.
    Range { min: Option<f64>, max: Option<f64> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcOp {
    Sum,
    Product,
    Average,
    Minimum,
    Maximum,
}

impl CalcOp {
    pub fn code(self) -> &'static str {
        match self {
            CalcOp::Sum => "SUM",
            CalcOp::Product => "PRD",
            CalcOp::Average => "AVG",
            CalcOp::Minimum => "MIN",
            CalcOp::Maximum => "MAX",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CalcOp::Sum => "sum (+)",
            CalcOp::Product => "product (x)",
            CalcOp::Average => "average",
            CalcOp::Minimum => "minimum",
            CalcOp::Maximum => "maximum",
        }
    }

    pub const ALL: [CalcOp; 5] = [CalcOp::Sum, CalcOp::Product, CalcOp::Average, CalcOp::Minimum, CalcOp::Maximum];
}

/// The Calculate tab.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Calculate {
    #[default]
    None,
    /// `AFSimple_Calculate("SUM", new Array("a", "b"))`.
    Simple { op: CalcOp, fields: Vec<String> },
    /// Simplified field notation, e.g. `Price * Quantity`.
    Notation(String),
}

/// A field's recognised scripts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Actions {
    pub format: Format,
    pub validate: Validate,
    pub calculate: Calculate,
    /// Events whose script isn't one of the AF calls (they need the JavaScript engine).
    pub unsupported: Vec<&'static str>,
    /// Those scripts' JavaScript, run through [`crate::Scripts`].
    pub scripts: Scripts,
}

/// Field scripts that aren't AF calls, by event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scripts {
    pub format: Option<String>,
    pub keystroke: Option<String>,
    pub validate: Option<String>,
    pub calculate: Option<String>,
}

impl Actions {
    pub fn is_empty(&self) -> bool {
        self.format == Format::None && self.validate == Validate::None && self.calculate == Calculate::None
    }
}

// ── parsing scripts ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Arg {
    Num(f64),
    Str(String),
    Bool(bool),
    List(Vec<Arg>),
}

impl Arg {
    fn num(&self) -> Option<f64> {
        match self {
            Arg::Num(n) => Some(*n),
            Arg::Bool(b) => Some(f64::from(u8::from(*b))),
            Arg::Str(s) => s.trim().parse().ok(),
            Arg::List(_) => None,
        }
    }

    fn string(&self) -> Option<String> {
        match self {
            Arg::Str(s) => Some(s.clone()),
            Arg::Num(n) => Some(n.to_string()),
            _ => None,
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Arg::Bool(b) => *b,
            Arg::Num(n) => *n != 0.0,
            Arg::Str(s) => !s.is_empty(),
            Arg::List(l) => !l.is_empty(),
        }
    }
}

/// How deeply script arguments and field notation may nest; deeper input is rejected rather
/// than recursed into (a hostile file could otherwise overflow the stack).
const MAX_NESTING: usize = 64;

fn hex4(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    bytes.iter().try_fold(0u16, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value.checked_mul(16)?.checked_add(u16::from(digit))
    })
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn string(&mut self) -> Option<String> {
        let q = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        while self.i < self.s.len() {
            let c = self.s[self.i];
            self.i += 1;
            match c {
                b'\\' if self.i < self.s.len() => {
                    let e = self.s[self.i];
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let code = hex4(self.s.get(self.i..self.i.checked_add(4)?)?)?;
                            self.i += 4;
                            let code = if (0xD800..=0xDBFF).contains(&code) && self.s.get(self.i..self.i.checked_add(6)?)?.starts_with(b"\\u") {
                                let low = hex4(self.s.get(self.i.checked_add(2)?..self.i.checked_add(6)?)?)?;
                                if (0xDC00..=0xDFFF).contains(&low) {
                                    self.i += 6;
                                    0x10000 + ((u32::from(code) - 0xD800) << 10) + (u32::from(low) - 0xDC00)
                                } else {
                                    u32::from(code)
                                }
                            } else {
                                u32::from(code)
                            };
                            let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
                            let mut encoded = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                c if c == q => return Some(String::from_utf8_lossy(&out).into_owned()),
                c => out.push(c),
            }
        }
        None
    }

    fn arg(&mut self) -> Option<Arg> {
        self.ws();
        let c = *self.s.get(self.i)?;
        match c {
            b'"' | b'\'' => self.string().map(Arg::Str),
            b'[' => {
                self.i += 1;
                self.nested(b']').map(Arg::List)
            }
            _ if self.s[self.i..].starts_with(b"new Array") => {
                self.i += b"new Array".len();
                self.ws();
                if self.s.get(self.i) != Some(&b'(') {
                    return None;
                }
                self.i += 1;
                self.nested(b')').map(Arg::List)
            }
            _ if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Some(Arg::Bool(true))
            }
            _ if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Some(Arg::Bool(false))
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || matches!(self.s[self.i], b'.' | b'-' | b'+' | b'e' | b'E')) {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok().map(Arg::Num)
            }
        }
    }

    /// A list inside another, at most [`MAX_NESTING`] deep.
    fn nested(&mut self, close: u8) -> Option<Vec<Arg>> {
        if self.depth >= MAX_NESTING {
            return None;
        }
        self.depth += 1;
        let out = self.list(close);
        self.depth -= 1;
        out
    }

    fn list(&mut self, close: u8) -> Option<Vec<Arg>> {
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.s.get(self.i) == Some(&close) {
                self.i += 1;
                return Some(out);
            }
            out.push(self.arg()?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(c) if *c == close => {
                    self.i += 1;
                    return Some(out);
                }
                _ => return None,
            }
        }
    }
}

/// The first call to `name(` in `js` and its arguments.
fn call(js: &str, name: &str) -> Option<Vec<Arg>> {
    let at = js.find(&format!("{name}("))?;
    // Not part of a longer identifier (AFDate_Format vs AFDate_FormatEx).
    if js[..at].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let mut p = Parser { s: js.as_bytes(), i: at + name.len() + 1, depth: 0 };
    p.list(b')')
}

/// Acrobat's AFDate_Format(n) / AFTime_Format(n) presets.
pub const DATE_PRESETS: [&str; 14] = [
    "m/d",
    "m/d/yy",
    "mm/dd/yy",
    "mm/yy",
    "d-mmm",
    "d-mmm-yy",
    "dd-mmm-yy",
    "yy-mm-dd",
    "mmm-yy",
    "mmmm-yy",
    "mmm d, yyyy",
    "mmmm d, yyyy",
    "m/d/yy h:MM tt",
    "m/d/yy HH:MM",
];
pub const TIME_PRESETS: [&str; 4] = ["HH:MM", "h:MM tt", "HH:MM:ss", "h:MM:ss tt"];

pub fn parse_format(js: &str) -> Option<Format> {
    let a = |args: &[Arg], i: usize| args.get(i).and_then(Arg::num).unwrap_or(0.0).clamp(0.0, 255.0) as u8;
    if let Some(args) = call(js, "AFNumber_Format") {
        return Some(Format::Number {
            decimals: a(&args, 0),
            sep: a(&args, 1),
            neg: a(&args, 2),
            currency: args.get(4).and_then(Arg::string).unwrap_or_default(),
            prepend: args.get(5).is_none_or(Arg::truthy),
        });
    }
    if let Some(args) = call(js, "AFPercent_Format") {
        return Some(Format::Percent { decimals: a(&args, 0), sep: a(&args, 1) });
    }
    if let Some(args) = call(js, "AFDate_FormatEx") {
        return Some(Format::Date(args.first().and_then(Arg::string)?));
    }
    if let Some(args) = call(js, "AFDate_Format") {
        return Some(Format::Date(DATE_PRESETS.get(a(&args, 0) as usize)?.to_string()));
    }
    if let Some(args) = call(js, "AFTime_FormatEx") {
        return Some(Format::Time(args.first().and_then(Arg::string)?));
    }
    if let Some(args) = call(js, "AFTime_Format") {
        return Some(Format::Time(TIME_PRESETS.get(a(&args, 0) as usize)?.to_string()));
    }
    if let Some(args) = call(js, "AFSpecial_Format") {
        return Some(Format::Special(a(&args, 0).min(3)));
    }
    if let Some(args) = call(js, "AFSpecial_KeystrokeEx") {
        return Some(Format::Mask(args.first().and_then(Arg::string)?));
    }
    None
}

pub fn parse_validate(js: &str) -> Option<Validate> {
    let args = call(js, "AFRange_Validate")?;
    let flag = |i: usize| args.get(i).is_some_and(Arg::truthy);
    let num = |i: usize| args.get(i).and_then(Arg::num);
    Some(Validate::Range { min: if flag(0) { num(1) } else { None }, max: if flag(2) { num(3) } else { None } })
}

pub fn parse_calculate(js: &str) -> Option<Calculate> {
    // Simplified field notation: Acrobat keeps the expression in a comment.
    if let (Some(a), Some(b)) = (js.find("BVCALC"), js.find("EVCALC")) {
        let expr = js.get(a + 6..b)?.trim().to_string();
        return Some(Calculate::Notation(expr));
    }
    let args = call(js, "AFSimple_Calculate")?;
    let op = match args.first().and_then(Arg::string)?.as_str() {
        "SUM" => CalcOp::Sum,
        "PRD" => CalcOp::Product,
        "AVG" => CalcOp::Average,
        "MIN" => CalcOp::Minimum,
        "MAX" => CalcOp::Maximum,
        _ => return None,
    };
    let fields = match args.get(1)? {
        Arg::List(l) => l.iter().filter_map(Arg::string).collect(),
        Arg::Str(s) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        _ => return None,
    };
    Some(Calculate::Simple { op, fields })
}

// ── writing scripts (what Acrobat's tabs generate) ─────────────────────────────────────────

fn js_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c.is_ascii() && !c.is_control() => o.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    o.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
    }
    o.push('"');
    o
}

/// The (format, keystroke) scripts for a format.
pub fn format_js(f: &Format) -> Option<(String, String)> {
    Some(match f {
        Format::None => return None,
        Format::Number { decimals, sep, neg, currency, prepend } => {
            let args = format!("{decimals}, {sep}, {neg}, 0, {}, {prepend}", js_str(currency));
            (format!("AFNumber_Format({args});"), format!("AFNumber_Keystroke({args});"))
        }
        Format::Percent { decimals, sep } => (format!("AFPercent_Format({decimals}, {sep});"), format!("AFPercent_Keystroke({decimals}, {sep});")),
        Format::Date(f) => (format!("AFDate_FormatEx({});", js_str(f)), format!("AFDate_KeystrokeEx({});", js_str(f))),
        Format::Time(f) => (format!("AFTime_FormatEx({});", js_str(f)), format!("AFTime_KeystrokeEx({});", js_str(f))),
        Format::Special(n) => (format!("AFSpecial_Format({n});"), format!("AFSpecial_Keystroke({n});")),
        Format::Mask(m) => (String::new(), format!("AFSpecial_KeystrokeEx({});", js_str(m))),
    })
}

pub fn validate_js(v: &Validate) -> Option<String> {
    match v {
        Validate::None => None,
        Validate::Range { min, max } => {
            Some(format!("AFRange_Validate({}, {}, {}, {});", min.is_some(), min.unwrap_or(0.0), max.is_some(), max.unwrap_or(0.0)))
        }
    }
}

pub fn calculate_js(c: &Calculate) -> Option<String> {
    match c {
        Calculate::None => None,
        Calculate::Simple { op, fields } => Some(format!(
            "AFSimple_Calculate({}, new Array({}));",
            js_str(op.code()),
            fields.iter().map(|f| js_str(f)).collect::<Vec<_>>().join(", ")
        )),
        Calculate::Notation(expr) => Some(format!("/** BVCALC {expr} EVCALC **/ event.value = AFMakeNumber(0);")),
    }
}

// ── running them ────────────────────────────────────────────────────────────────────────────

/// AFMakeNumber: a number from typed text (currency symbols, group separators, spaces and a
/// trailing % ignored; a comma decimal separator accepted).
pub fn make_number(s: &str) -> Option<f64> {
    let t: String = s.trim().chars().filter(|c| !c.is_whitespace() && !matches!(c, '$' | '€' | '£' | '¥' | '\'' | '%')).collect();
    if t.is_empty() {
        return None;
    }
    let neg = t.starts_with('(') && t.ends_with(')');
    let t = t.trim_start_matches('(').trim_end_matches(')');
    // "1.234,56" or "1234,56": comma is the decimal separator when it comes last.
    let normalized = match (t.rfind(','), t.rfind('.')) {
        (Some(c), Some(d)) if c > d => t.replace('.', "").replace(',', "."),
        (Some(c), None) if t.len() - c - 1 != 3 => t.replace(',', "."),
        _ => t.replace(',', ""),
    };
    let v: f64 = normalized.parse().ok()?;
    v.is_finite().then_some(if neg { -v } else { v })
}

/// A number as JavaScript prints it (integers without a fraction).
pub fn js_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

fn group(int: &str, sep: char) -> String {
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// A number with `decimals` places in separator style `sep` (0 "1,234.56", 1 "1234.56",
/// 2 "1.234,56", 3 "1234,56", 4 "1'234.56").
fn number_text(v: f64, decimals: u8, sep: u8) -> String {
    // Round half away from zero, as JavaScript's toFixed does for these values.
    let p = 10f64.powi(i32::from(decimals));
    let s = format!("{:.*}", decimals as usize, (v.abs() * p).round() / p);
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
    let (g, d) = match sep {
        1 => (None, '.'),
        2 => (Some('.'), ','),
        3 => (None, ','),
        4 => (Some('\''), '.'),
        _ => (Some(','), '.'),
    };
    let mut out = match g {
        Some(c) => group(int, c),
        None => int.to_string(),
    };
    if !frac.is_empty() {
        out.push(d);
        out.push_str(frac);
    }
    out
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// A date and time read from text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub y: i32,
    pub m: u32,
    pub d: u32,
    pub hh: u32,
    pub mm: u32,
    pub ss: u32,
}

fn days_in(y: i32, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn weekday(y: i32, m: u32, d: u32) -> usize {
    // Sakamoto's method, in i64 so no year or day can overflow it.
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = i64::from(y) - i64::from(m < 3);
    let t = T.get((m as usize).wrapping_sub(1)).copied().unwrap_or(0);
    (y + y / 4 - y / 100 + y / 400 + t + i64::from(d)).rem_euclid(7) as usize
}

/// The order of the year/month/day fields in a format, as letters.
fn order(fmt: &str) -> Vec<char> {
    let mut out = Vec::new();
    // Weekday names (ddd, dddd) are shown, not typed: leave them out.
    let mut cleaned = fmt.replace("dddd", " ");
    cleaned = cleaned.replace("ddd", " ");
    for c in cleaned.chars() {
        let k = match c {
            'y' => 'y',
            'm' => 'm',
            'd' => 'd',
            'H' | 'h' => 'H',
            'M' => 'M',
            's' => 's',
            _ => continue,
        };
        if out.last() != Some(&k) {
            out.push(k);
        }
    }
    out
}

/// Parse typed text against a date/time format (AFDate_KeystrokeEx's leniency: any
/// separators, month names, two-digit years). `None` when it isn't a valid date.
pub fn parse_date(text: &str, fmt: &str) -> Option<DateTime> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_lowercase();
    let pm = lower.contains("pm") || lower.ends_with('p');
    let am = lower.contains("am");
    // Tokens: numbers and month names.
    let mut nums: Vec<u32> = Vec::new();
    let mut month_name: Option<u32> = None;
    let mut cur = String::new();
    let flush = |cur: &mut String, nums: &mut Vec<u32>, month: &mut Option<u32>| {
        if cur.is_empty() {
            return;
        }
        if let Ok(n) = cur.parse::<u32>() {
            nums.push(n);
        } else if cur.len() >= 3
            && let Some(i) = MONTHS.iter().position(|m| m.to_lowercase().starts_with(&cur.to_lowercase()))
        {
            *month = Some(i as u32 + 1);
        }
        cur.clear();
    };
    for c in t.chars() {
        // Digits and letters form separate tokens ("5Mar" is 5 and Mar).
        let same_kind = cur.is_empty() || cur.chars().next().is_some_and(|x| x.is_ascii_digit()) == c.is_ascii_digit();
        if c.is_alphanumeric() && same_kind {
            cur.push(c);
        } else {
            flush(&mut cur, &mut nums, &mut month_name);
            if c.is_alphanumeric() {
                cur.push(c);
            }
        }
    }
    flush(&mut cur, &mut nums, &mut month_name);
    let mut dt = DateTime { y: 0, m: 1, d: 1, hh: 0, mm: 0, ss: 0 };
    let mut it = nums.into_iter();
    let mut have_y = false;
    for k in order(fmt) {
        match k {
            'm' => {
                dt.m = match month_name {
                    Some(m) => m,
                    None => it.next()?,
                }
            }
            'd' => dt.d = it.next()?,
            'y' => {
                let y = i32::try_from(it.next()?).ok()?;
                dt.y = if y < 100 { if y < 50 { 2000 + y } else { 1900 + y } } else { y };
                have_y = true;
            }
            'H' => dt.hh = it.next().unwrap_or(0),
            'M' => dt.mm = it.next().unwrap_or(0),
            's' => dt.ss = it.next().unwrap_or(0),
            _ => {}
        }
    }
    if it.next().is_some() {
        return None;
    }
    if !have_y {
        dt.y = 2000;
    }
    if pm && dt.hh < 12 {
        dt.hh += 12;
    }
    if am && dt.hh == 12 {
        dt.hh = 0;
    }
    let has_date = fmt.contains(['y', 'm', 'd']);
    if has_date && (!(1..=12).contains(&dt.m) || dt.d == 0 || dt.d > days_in(dt.y, dt.m)) {
        return None;
    }
    if dt.hh > 23 || dt.mm > 59 || dt.ss > 59 {
        return None;
    }
    Some(dt)
}

/// Format a date/time with Acrobat's date format letters.
pub fn format_date(dt: DateTime, fmt: &str) -> String {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        let h12 = if dt.hh.is_multiple_of(12) { 12 } else { dt.hh % 12 };
        match (c, n) {
            ('y', 4..) => {
                let _ = write!(out, "{:04}", dt.y);
            }
            ('y', _) => {
                let _ = write!(out, "{:02}", dt.y.rem_euclid(100));
            }
            ('m', 3..) => match MONTHS.get((dt.m as usize).wrapping_sub(1)) {
                Some(name) if n >= 4 => out.push_str(name),
                Some(name) => out.push_str(name.get(..3).unwrap_or(name)),
                None => {
                    let _ = write!(out, "{}", dt.m);
                }
            },
            ('m', 2) => {
                let _ = write!(out, "{:02}", dt.m);
            }
            ('m', _) => {
                let _ = write!(out, "{}", dt.m);
            }
            ('d', 3..) => {
                let name = DAYS.get(weekday(dt.y, dt.m, dt.d)).copied().unwrap_or_default();
                out.push_str(if n >= 4 { name } else { name.get(..3).unwrap_or(name) });
            }
            ('d', 2) => {
                let _ = write!(out, "{:02}", dt.d);
            }
            ('d', _) => {
                let _ = write!(out, "{}", dt.d);
            }
            ('H', 2) => {
                let _ = write!(out, "{:02}", dt.hh);
            }
            ('H', _) => {
                let _ = write!(out, "{}", dt.hh);
            }
            ('h', 2) => {
                let _ = write!(out, "{h12:02}");
            }
            ('h', _) => {
                let _ = write!(out, "{h12}");
            }
            ('M', 2) => {
                let _ = write!(out, "{:02}", dt.mm);
            }
            ('M', _) => {
                let _ = write!(out, "{}", dt.mm);
            }
            ('s', 2) => {
                let _ = write!(out, "{:02}", dt.ss);
            }
            ('s', _) => {
                let _ = write!(out, "{}", dt.ss);
            }
            ('t', 2) => out.push_str(if dt.hh < 12 { "am" } else { "pm" }),
            ('t', _) => out.push(if dt.hh < 12 { 'a' } else { 'p' }),
            (c, n) => out.extend(std::iter::repeat_n(c, n)),
        }
        i += n;
    }
    out
}

const SPECIAL_MASKS: [&str; 4] = ["99999", "99999-9999", "(999) 999-9999", "999-99-9999"];

fn mask_fits(mask: &str, text: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let m: Vec<char> = mask.chars().collect();
    t.len() == m.len()
        && m.iter().zip(&t).all(|(mc, tc)| match mc {
            '9' => tc.is_ascii_digit(),
            'A' => tc.is_alphabetic(),
            'O' => tc.is_alphanumeric(),
            'X' => true,
            other => other == tc,
        })
}

/// Fill a mask from the typed characters that fit its slots.
fn apply_mask(mask: &str, text: &str) -> Option<String> {
    let mut src = text.chars().filter(|c| c.is_alphanumeric());
    let mut out = String::new();
    for mc in mask.chars() {
        match mc {
            '9' | 'A' | 'O' | 'X' => {
                let c = src.next()?;
                let ok = match mc {
                    '9' => c.is_ascii_digit(),
                    'A' => c.is_alphabetic(),
                    _ => true,
                };
                if !ok {
                    return None;
                }
                out.push(c);
            }
            other => out.push(other),
        }
    }
    src.next().is_none().then_some(out)
}

/// The text shown for a value (the Format event).
pub fn format_value(f: &Format, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    match f {
        Format::None => value.to_string(),
        Format::Number { decimals, sep, neg, currency, prepend } => {
            let Some(v) = make_number(value) else { return value.to_string() };
            let body = number_text(v, *decimals, *sep);
            let body = match (currency.is_empty(), *prepend) {
                (true, _) => body,
                (false, true) => format!("{currency}{body}"),
                (false, false) => format!("{body}{currency}"),
            };
            match (v < 0.0, neg) {
                (false, _) => body,
                // Styles 2 and 3 use parentheses (3 is also red, which the appearance can't
                // know here); 0 and 1 a minus sign.
                (true, 2 | 3) => format!("({body})"),
                (true, _) => format!("-{body}"),
            }
        }
        Format::Percent { decimals, sep } => match make_number(value) {
            Some(v) => format!(
                "{}%",
                if v < 0.0 { format!("-{}", number_text(v * 100.0, *decimals, *sep)) } else { number_text(v * 100.0, *decimals, *sep) }
            ),
            None => value.to_string(),
        },
        Format::Date(fmt) | Format::Time(fmt) => parse_date(value, fmt).map_or_else(|| value.to_string(), |dt| format_date(dt, fmt)),
        Format::Special(n) => apply_mask(SPECIAL_MASKS[(*n).min(3) as usize], value).unwrap_or_else(|| value.to_string()),
        Format::Mask(m) => apply_mask(m, value).unwrap_or_else(|| value.to_string()),
    }
}

/// The Keystroke event on commit: does the value fit the format? Returns the value to store
/// (numbers are normalised), or Acrobat's message.
pub fn keystroke(f: &Format, field: &str, value: &str) -> Result<String, String> {
    let bad = || format!("The value entered does not match the format of the field [ {field} ]");
    if value.trim().is_empty() {
        return Ok(String::new());
    }
    match f {
        Format::None => Ok(value.to_string()),
        Format::Number { .. } => make_number(value).map(js_number).ok_or_else(bad),
        Format::Percent { .. } => {
            // Typed "12.5%" or "12.5" both mean 0.125 once stored? Acrobat stores the number
            // as typed and shows it ×100: keep the typed number.
            make_number(value).map(js_number).ok_or_else(bad)
        }
        Format::Date(fmt) | Format::Time(fmt) => parse_date(value, fmt)
            .map(|_| value.trim().to_string())
            .ok_or_else(|| format!("Invalid date/time: please ensure that the date/time exists. Field [ {field} ] should match format {fmt}")),
        Format::Special(n) => {
            let m = SPECIAL_MASKS[(*n).min(3) as usize];
            apply_mask(m, value).filter(|s| mask_fits(m, s)).map(|_| value.trim().to_string()).ok_or_else(bad)
        }
        Format::Mask(m) => apply_mask(m, value).map(|_| value.trim().to_string()).ok_or_else(bad),
    }
}

/// The Validate event: `Err` with Acrobat's message when the value is out of range.
pub fn validate(v: &Validate, value: &str) -> Result<(), String> {
    let Validate::Range { min, max } = v else { return Ok(()) };
    if value.trim().is_empty() {
        return Ok(());
    }
    let Some(x) = make_number(value) else { return Ok(()) };
    let fmt = |n: f64| js_number(n);
    let ok = min.is_none_or(|m| x >= m) && max.is_none_or(|m| x <= m);
    if ok {
        return Ok(());
    }
    Err(match (min, max) {
        (Some(a), Some(b)) => format!("Invalid value: must be greater than or equal to {} and less than or equal to {}.", fmt(*a), fmt(*b)),
        (Some(a), None) => format!("Invalid value: must be greater than or equal to {}.", fmt(*a)),
        (None, Some(b)) => format!("Invalid value: must be less than or equal to {}.", fmt(*b)),
        (None, None) => String::new(),
    })
}

/// Evaluate simplified field notation: + - * / and parentheses over numbers and field names
/// (`\` escapes spaces and operators in names, as Acrobat writes them).
fn eval_notation(src: &str, value_of: &dyn Fn(&str) -> f64) -> Option<f64> {
    #[derive(Debug, Clone, PartialEq)]
    enum T {
        N(f64),
        Op(char),
    }
    let mut toks = Vec::new();
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if "+-*/()".contains(c) {
            toks.push(T::Op(c));
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let s = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            toks.push(T::N(cs[s..i].iter().collect::<String>().parse().ok()?));
        } else {
            let mut name = String::new();
            while i < cs.len() && !(cs[i].is_whitespace() || "+-*/()".contains(cs[i])) {
                if cs[i] == '\\' && i + 1 < cs.len() {
                    i += 1;
                }
                name.push(cs[i]);
                i += 1;
            }
            toks.push(T::N(value_of(&name)));
        }
    }
    // Recursive descent, `d` levels deep (at most MAX_NESTING).
    fn expr(t: &[T], i: &mut usize, d: usize) -> Option<f64> {
        let mut v = term(t, i, d)?;
        while let Some(T::Op(o @ ('+' | '-'))) = t.get(*i) {
            *i += 1;
            let r = term(t, i, d)?;
            v = if *o == '+' { v + r } else { v - r };
        }
        Some(v)
    }
    fn term(t: &[T], i: &mut usize, d: usize) -> Option<f64> {
        let mut v = factor(t, i, d)?;
        while let Some(T::Op(o @ ('*' | '/'))) = t.get(*i) {
            *i += 1;
            let r = factor(t, i, d)?;
            v = if *o == '*' { v * r } else { v / r };
        }
        Some(v)
    }
    fn factor(t: &[T], i: &mut usize, d: usize) -> Option<f64> {
        if d > MAX_NESTING {
            return None;
        }
        match t.get(*i)? {
            T::N(n) => {
                *i += 1;
                Some(*n)
            }
            T::Op('-') => {
                *i += 1;
                factor(t, i, d + 1).map(|v| -v)
            }
            T::Op('(') => {
                *i += 1;
                let v = expr(t, i, d + 1)?;
                (t.get(*i) == Some(&T::Op(')'))).then(|| *i += 1)?;
                Some(v)
            }
            _ => None,
        }
    }
    let mut i = 0;
    let v = expr(&toks, &mut i, 0)?;
    (i == toks.len() && v.is_finite()).then_some(v)
}

/// The Calculate event: the new value, given the current values of the other fields
/// (`values(name)` returns every field named `name` or below it, for groups).
pub fn calculate(c: &Calculate, values: &dyn Fn(&str) -> Vec<String>) -> Option<String> {
    let num = |s: &String| make_number(s).unwrap_or(0.0);
    match c {
        Calculate::None => None,
        Calculate::Simple { op, fields } => {
            let xs: Vec<f64> = fields.iter().flat_map(|f| values(f)).map(|v| num(&v)).collect();
            let v = match op {
                CalcOp::Sum => xs.iter().sum(),
                CalcOp::Product => xs.iter().product(),
                CalcOp::Average => {
                    if xs.is_empty() {
                        0.0
                    } else {
                        xs.iter().sum::<f64>() / xs.len() as f64
                    }
                }
                CalcOp::Minimum => xs.iter().copied().fold(f64::INFINITY, f64::min),
                CalcOp::Maximum => xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            };
            Some(if v.is_finite() { js_number(v) } else { "0".into() })
        }
        Calculate::Notation(expr) => {
            let v = eval_notation(expr, &|name| values(name).first().map(num).unwrap_or(0.0))?;
            Some(js_number(v))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_recognised_and_written_back() {
        assert_eq!(
            parse_format("AFNumber_Format(2, 0, 0, 0, \"$\", true);"),
            Some(Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true })
        );
        assert_eq!(parse_format("AFPercent_Format(1, 0);"), Some(Format::Percent { decimals: 1, sep: 0 }));
        assert_eq!(parse_format("AFDate_FormatEx(\"dd-mmm-yyyy\");"), Some(Format::Date("dd-mmm-yyyy".into())));
        assert_eq!(parse_format("AFDate_Format(2);"), Some(Format::Date("mm/dd/yy".into())));
        assert_eq!(parse_format("AFTime_Format(1);"), Some(Format::Time("h:MM tt".into())));
        assert_eq!(parse_format("AFSpecial_Format(2);"), Some(Format::Special(2)));
        assert_eq!(parse_format("AFSpecial_KeystrokeEx(\"AA-9999\");"), Some(Format::Mask("AA-9999".into())));
        assert_eq!(parse_format("app.alert('hi')"), None);
        assert_eq!(parse_validate("AFRange_Validate(true, 0, true, 100);"), Some(Validate::Range { min: Some(0.0), max: Some(100.0) }));
        assert_eq!(parse_validate("AFRange_Validate(false, 0, true, 10);"), Some(Validate::Range { min: None, max: Some(10.0) }));
        assert_eq!(
            parse_calculate("AFSimple_Calculate(\"SUM\", new Array (\"a\", \"b.c\"));"),
            Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["a".into(), "b.c".into()] })
        );
        assert_eq!(parse_calculate("AFSimple_Calculate('AVG', ['x']);"), Some(Calculate::Simple { op: CalcOp::Average, fields: vec!["x".into()] }));
        assert_eq!(
            parse_calculate("/** BVCALC Price * Qty EVCALC **/ event.value = AFMakeNumber(getField(\"Price\").value) * …"),
            Some(Calculate::Notation("Price * Qty".into()))
        );
        // Round trips through what the tabs write.
        for f in [
            Format::Number { decimals: 2, sep: 2, neg: 2, currency: "€".into(), prepend: false },
            Format::Percent { decimals: 0, sep: 0 },
            Format::Date("mmmm d, yyyy".into()),
            Format::Special(3),
        ] {
            let (fj, _) = format_js(&f).unwrap();
            assert_eq!(parse_format(&fj), Some(f));
        }
        let euro = format_js(&Format::Number { decimals: 2, sep: 2, neg: 0, currency: "€".into(), prepend: true }).unwrap().0;
        assert!(euro.contains("\\u20AC"), "{euro}");
        assert!(!euro.contains('€'), "{euro}");
        let pound = format_js(&Format::Number { decimals: 2, sep: 0, neg: 0, currency: "£".into(), prepend: true }).unwrap().0;
        assert!(pound.contains("\\u00A3"), "{pound}");
        assert!(!pound.contains('£'), "{pound}");
        let c = Calculate::Simple { op: CalcOp::Product, fields: vec!["q".into(), "p".into()] };
        assert_eq!(parse_calculate(&calculate_js(&c).unwrap()), Some(c));
        let c = Calculate::Notation("a + b".into());
        assert_eq!(parse_calculate(&calculate_js(&c).unwrap()), Some(c));
    }

    #[test]
    fn numbers_format_like_acrobat() {
        let n = |d, s, ng, c: &str, p| Format::Number { decimals: d, sep: s, neg: ng, currency: c.into(), prepend: p };
        assert_eq!(format_value(&n(2, 0, 0, "$", true), "1234.5"), "$1,234.50");
        assert_eq!(format_value(&n(0, 1, 0, "", true), "1234567"), "1234567");
        assert_eq!(format_value(&n(2, 2, 0, " €", false), "1234.5"), "1.234,50 €");
        assert_eq!(format_value(&n(2, 0, 2, "", true), "-12"), "(12.00)");
        assert_eq!(format_value(&n(1, 0, 0, "", true), "-0.25"), "-0.3");
        assert_eq!(format_value(&Format::Percent { decimals: 1, sep: 0 }, "0.125"), "12.5%");
        assert_eq!(make_number("$1,234.50"), Some(1234.5));
        assert_eq!(make_number("1.234,5"), Some(1234.5));
        assert_eq!(make_number("(7)"), Some(-7.0));
        assert_eq!(make_number("abc"), None);
        assert_eq!(keystroke(&n(2, 0, 0, "$", true), "Total", "$1,000"), Ok("1000".into()));
        assert_eq!(
            keystroke(&n(2, 0, 0, "$", true), "Total", "ten"),
            Err("The value entered does not match the format of the field [ Total ]".into())
        );
    }

    #[test]
    fn dates_times_and_masks() {
        let d = Format::Date("mm/dd/yyyy".into());
        assert_eq!(format_value(&d, "1/2/24"), "01/02/2024");
        assert_eq!(format_value(&Format::Date("mmmm d, yyyy".into()), "10/1/2026"), "October 1, 2026");
        assert_eq!(format_value(&Format::Date("dddd, mmm d, yyyy".into()), "10/1/2026"), "Thursday, Oct 1, 2026");
        assert_eq!(format_value(&Format::Date("dd-mmm-yy".into()), "5-Mar-21"), "05-Mar-21", "month names are read");
        assert!(keystroke(&d, "Due", "2/30/2024").is_err(), "no 30 February");
        assert!(keystroke(&d, "Due", "13/1/2024").is_err());
        assert_eq!(format_value(&Format::Time("h:MM tt".into()), "14:05"), "2:05 pm");
        assert_eq!(format_value(&Format::Time("HH:MM".into()), "2:05 pm"), "14:05");
        assert_eq!(format_value(&Format::Special(2), "5551234567"), "(555) 123-4567");
        assert_eq!(format_value(&Format::Special(3), "123-45-6789"), "123-45-6789");
        assert!(keystroke(&Format::Special(0), "Zip", "1234").is_err());
        assert_eq!(format_value(&Format::Mask("AA-9999".into()), "ab1234"), "ab-1234");
        assert!(keystroke(&Format::Mask("AA-9999".into()), "Code", "a11234").is_err());
    }

    #[test]
    fn validation_and_calculation() {
        let r = Validate::Range { min: Some(0.0), max: Some(100.0) };
        assert!(validate(&r, "50").is_ok() && validate(&r, "").is_ok());
        assert_eq!(validate(&r, "120"), Err("Invalid value: must be greater than or equal to 0 and less than or equal to 100.".into()));
        let vals = |name: &str| -> Vec<String> {
            match name {
                "a" => vec!["2".into()],
                "b" => vec!["$3.50".into()],
                "row" => vec!["1".into(), "2".into(), "3".into()],
                "Unit Price" => vec!["4".into()],
                _ => vec![],
            }
        };
        let simple = |op, f: &[&str]| calculate(&Calculate::Simple { op, fields: f.iter().map(|s| s.to_string()).collect() }, &vals);
        assert_eq!(simple(CalcOp::Sum, &["a", "b"]), Some("5.5".into()));
        assert_eq!(simple(CalcOp::Product, &["a", "b"]), Some("7".into()));
        assert_eq!(simple(CalcOp::Average, &["row"]), Some("2".into()), "groups count every child");
        assert_eq!(simple(CalcOp::Maximum, &["a", "row"]), Some("3".into()));
        assert_eq!(simple(CalcOp::Sum, &["missing"]), Some("0".into()));
        assert_eq!(calculate(&Calculate::Notation("(a + b) * 2 - Unit\\ Price / 4".into()), &vals), Some("10".into()));
        assert_eq!(calculate(&Calculate::Notation("a +".into()), &vals), None);
    }
}

// ── push buttons ────────────────────────────────────────────────────────────────────────────

/// What clicking a push button does (its mouse-up action), as far as PdfCraft can run it
/// without a JavaScript engine.
#[derive(Clone, Debug, PartialEq)]
pub enum ButtonAction {
    /// Reset fields: the listed ones, or all but them (`exclude`), or all when empty.
    Reset {
        fields: Vec<String>,
        exclude: bool,
    },
    /// A named action: Print, NextPage, PrevPage, FirstPage, LastPage, …
    Named(String),
    Uri(String),
    /// Go to a page (0-based) in this document.
    GoTo(usize),
    /// A hide action (§12.6.4.11): hide the listed fields (and their kids), or show them when
    /// `hide` is false.
    ShowHide {
        fields: Vec<String>,
        hide: bool,
    },
    /// A set-layer-visibility action (`SetOCGState`, §12.6.4.13): each change in order, naming
    /// the layer by its optional content group (object number, generation). With `preserve_rb`,
    /// a layer turned on turns off the other layers of its radio-button groups.
    SetLayers {
        changes: Vec<(LayerOp, (u32, u16))>,
        preserve_rb: bool,
    },
    /// `app.alert("…")`.
    Alert(String),
    /// Submit the form to a URL (not sent: PdfCraft never posts form data on its own).
    Submit(String),
    /// `event.target.buttonImportIcon()`: choose an image for the button (an image field).
    ImportIcon,
    /// A script PdfCraft can't run yet.
    Script(String),
}

/// What a set-layer-visibility action does to a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerOp {
    On,
    Off,
    Toggle,
}

/// Recognise the common one-line button scripts.
pub fn button_script(js: &str) -> ButtonAction {
    let t = js.trim();
    // Print takes an options object whose contents don't change what PdfCraft does.
    if t.starts_with("this.print(") || t.starts_with("print(") || t.contains(";this.print(") || t.contains("; this.print(") {
        return ButtonAction::Named("Print".into());
    }
    if let Some(args) = call(t, "this.resetForm").or_else(|| call(t, "resetForm")) {
        let fields = match args.first() {
            Some(Arg::List(l)) => l.iter().filter_map(Arg::string).collect(),
            Some(Arg::Str(s)) => vec![s.clone()],
            _ => Vec::new(),
        };
        return ButtonAction::Reset { fields, exclude: false };
    }
    if let Some(m) = call(t, "app.alert").and_then(|args| args.first().and_then(Arg::string)) {
        return ButtonAction::Alert(m);
    }
    if let Some(u) = call(t, "app.launchURL").and_then(|args| args.first().and_then(Arg::string)) {
        return ButtonAction::Uri(u);
    }
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.trim_end_matches(';') == "event.target.buttonImportIcon()" {
        return ButtonAction::ImportIcon;
    }
    match compact.trim_end_matches(';') {
        "this.pageNum++" | "pageNum++" => ButtonAction::Named("NextPage".into()),
        "this.pageNum--" | "pageNum--" => ButtonAction::Named("PrevPage".into()),
        "this.pageNum=0" => ButtonAction::Named("FirstPage".into()),
        _ => ButtonAction::Script(t.to_string()),
    }
}

#[cfg(test)]
mod button_tests {
    use super::*;

    #[test]
    fn common_button_scripts_are_recognised() {
        assert_eq!(button_script("this.print({bUI: true});"), ButtonAction::Named("Print".into()));
        assert_eq!(button_script("this.resetForm([\"a\", \"b\"]);"), ButtonAction::Reset { fields: vec!["a".into(), "b".into()], exclude: false });
        assert_eq!(button_script("this.resetForm();"), ButtonAction::Reset { fields: vec![], exclude: false });
        assert_eq!(button_script("app.alert('Thanks!');"), ButtonAction::Alert("Thanks!".into()));
        assert_eq!(button_script("app.launchURL(\"https://example.org\", true);"), ButtonAction::Uri("https://example.org".into()));
        assert_eq!(button_script("this.pageNum++;"), ButtonAction::Named("NextPage".into()));
        assert_eq!(button_script("event.target.buttonImportIcon();"), ButtonAction::ImportIcon);
        assert!(matches!(button_script("var x = 1; doStuff(x);"), ButtonAction::Script(_)));
    }
}

#[cfg(test)]
mod never_crash_tests {
    use super::*;

    /// A field value or typed text with an absurd year used to overflow the weekday arithmetic.
    #[test]
    fn huge_years_format_without_panicking() {
        let f = Format::Date("dddd, mmmm d, yyyy".into());
        for v in ["1/1/2147483647", "1/1/4294967295", "12/31/3000000000"] {
            let _ = format_value(&f, v);
            let _ = keystroke(&f, "date", v);
        }
    }

    /// Deeply nested arrays in a field's format script used to recurse until the stack overflowed.
    #[test]
    fn deeply_nested_script_arguments_are_rejected() {
        let n = 200_000;
        let js = format!("AFNumber_Format({}0{});", "[".repeat(n), "]".repeat(n));
        assert_eq!(parse_format(&js), None);
        let js = format!("AFNumber_Format({}0{});", "new Array(".repeat(n), ")".repeat(n));
        assert_eq!(parse_format(&js), None);
    }

    /// Deeply nested simplified field notation (from a field's calculate script) likewise.
    #[test]
    fn deeply_nested_notation_is_rejected() {
        let n = 200_000;
        let deep = format!("{}1{}", "(".repeat(n), ")".repeat(n));
        assert_eq!(eval_notation(&deep, &|_| 0.0), None);
        let negated = format!("{}1", "-".repeat(n));
        assert_eq!(eval_notation(&negated, &|_| 0.0), None);
        assert_eq!(eval_notation("((1 + 2)) * -(3)", &|_| 0.0), Some(-9.0));
    }

    /// `DateTime` is public: an out-of-range month or day must not index past the name tables.
    #[test]
    fn out_of_range_dates_format_without_panicking() {
        for m in [0, 13, u32::MAX] {
            let dt = DateTime { y: 2024, m, d: 1, hh: 0, mm: 0, ss: 0 };
            let _ = format_date(dt, "dddd mmmm mmm d yyyy");
        }
        let dt = DateTime { y: i32::MAX, m: 1, d: u32::MAX, hh: 0, mm: 0, ss: 0 };
        let _ = format_date(dt, "dddd ddd");
    }
}
