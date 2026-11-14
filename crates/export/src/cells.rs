//! What a cell's text is, for the Excel export: a number, an amount, a percentage or a date that
//! Excel can compute with, with a number format that shows it exactly as the PDF does - or text.
//! Anything that could be read two ways stays text.
//!
//! - Leading zeros stay text ("00123", "0123": codes, not quantities).
//! - More than 15 significant digits stay text (card and account numbers: Excel keeps 15).
//! - Plain numbers, thousands groups of three, a decimal point: a number, its decimals and
//!   grouping kept in the format.
//! - Amounts: `$`, `€`, `£` before or after the number, with or without a space; negatives with a
//!   minus in front, in parentheses, or with a trailing minus - a number with a format that shows
//!   the same text.
//! - Percentages ("12.5%", "12 %"): the fraction (0.125) with a percent format.
//! - Dates: ISO `yyyy-mm-dd`, or a date written with an English month name in Excel's own
//!   spelling ("14 Oct 2026", "October 14, 2026", "14-Oct-2026") - a real date with the format
//!   that shows it the same way. Numeric dates such as `03/04/2026` (March or April?) stay text.

/// What a cell holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text,
    /// A number and the Excel number format that shows it as the PDF did.
    Number {
        value: f64,
        format: String,
    },
    /// A date as Excel's serial day number (1900 date system) and its date format.
    Date {
        serial: f64,
        format: String,
    },
}

/// The most significant digits Excel keeps.
const EXCEL_DIGITS: usize = 15;

pub fn classify(text: &str) -> Value {
    let t = text.trim();
    if t.is_empty() || t.len() > 64 {
        return Value::Text;
    }
    if let Some(v) = date(t) {
        return v;
    }
    amount(t).unwrap_or(Value::Text)
}

/// A number written with an optional thousands grouping and decimal point: (value, decimals,
/// grouped). None for leading zeros, more than 15 significant digits, or anything else.
fn bare_number(s: &str) -> Option<(f64, usize, bool)> {
    if s.is_empty() || !s.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    if let Some(f) = frac
        && (f.is_empty() || !f.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    if !int.chars().all(|c| c.is_ascii_digit() || c == ',') {
        return None;
    }
    let groups: Vec<&str> = int.split(',').collect();
    let grouped = groups.len() > 1;
    if grouped && (groups[0].is_empty() || groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3)) {
        return None;
    }
    let digits: String = int.chars().filter(|c| *c != ',').collect();
    // A code, not a quantity: "00123", "0123" (a lone "0", and "0.5", are numbers).
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    let all: String = digits.chars().chain(frac.unwrap_or("").chars()).collect();
    let significant = all.trim_start_matches('0').trim_end_matches('0').len().max(1);
    if significant > EXCEL_DIGITS || digits.len() > EXCEL_DIGITS {
        return None;
    }
    let plain = match frac {
        Some(f) => format!("{digits}.{f}"),
        None => digits,
    };
    let value: f64 = plain.parse().ok().filter(|v: &f64| v.is_finite())?;
    Some((value, frac.map_or(0, str::len), grouped))
}

/// The number part of a format: `0`, `0.00`, `#,##0.00`.
fn number_format(decimals: usize, grouped: bool) -> String {
    let mut f = if grouped { "#,##0".to_string() } else { "0".to_string() };
    if decimals > 0 {
        f.push('.');
        f.push_str(&"0".repeat(decimals.min(30)));
    }
    f
}

/// A literal in a number format: quoted (`"$ "`).
fn literal(s: &str) -> String {
    if s.is_empty() { String::new() } else { format!("\"{s}\"") }
}

const SYMBOLS: [char; 3] = ['$', '€', '£'];

/// Numbers, amounts and percentages, with their negatives.
fn amount(t: &str) -> Option<Value> {
    // How a negative is written: a leading minus (Excel's own way with a one-part format),
    // parentheses, or a trailing minus.
    #[derive(PartialEq)]
    enum Neg {
        None,
        Minus,
        Parens,
        Trailing,
    }
    let (neg, body) = if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        (Neg::Parens, inner)
    } else if let Some(rest) = t.strip_prefix('-') {
        (Neg::Minus, rest)
    } else if let Some(rest) = t.strip_suffix('-') {
        (Neg::Trailing, rest)
    } else {
        (Neg::None, t)
    };
    if body.is_empty() || body.starts_with(['-', '(', ' ']) || body.ends_with(['-', ')', ' ']) {
        return None;
    }
    // A percentage: the number, then "%" with or without a space.
    let (body, percent) = match body.strip_suffix('%') {
        Some(b) => match b.strip_suffix(' ') {
            Some(b) => (b, Some(" ")),
            None => (b, Some("")),
        },
        None => (body, None),
    };
    // A currency symbol before or after, with or without a space (not with a percentage).
    let (prefix, number, suffix) = if percent.is_some() {
        (String::new(), body, String::new())
    } else if let Some(c) = body.chars().next().filter(|c| SYMBOLS.contains(c)) {
        let rest = &body[c.len_utf8()..];
        match rest.strip_prefix(' ') {
            Some(r) => (format!("{c} "), r, String::new()),
            None => (c.to_string(), rest, String::new()),
        }
    } else if let Some(c) = body.chars().last().filter(|c| SYMBOLS.contains(c)) {
        let rest = &body[..body.len() - c.len_utf8()];
        match rest.strip_suffix(' ') {
            Some(r) => (String::new(), r, format!(" {c}")),
            None => (String::new(), rest, c.to_string()),
        }
    } else {
        (String::new(), body, String::new())
    };
    let (mut value, decimals, grouped) = bare_number(number)?;
    let core = number_format(decimals, grouped);
    let positive = match percent {
        Some(space) => {
            value /= 100.0;
            format!("{core}{}%", literal(space))
        }
        None => format!("{}{core}{}", literal(&prefix), literal(&suffix)),
    };
    let format = match neg {
        Neg::None => positive,
        Neg::Minus => {
            value = -value;
            positive
        }
        Neg::Parens => {
            value = -value;
            format!("{positive};({positive})")
        }
        Neg::Trailing => {
            value = -value;
            format!("{positive};{positive}-")
        }
    };
    Some(Value::Number { value, format })
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// A month written as Excel writes it: (month 1-12, full name?).
fn month(word: &str) -> Option<(u32, bool)> {
    for (i, m) in MONTHS.iter().enumerate() {
        if word == *m {
            return Some((i as u32 + 1, true));
        }
        if word == &m[..3] {
            return Some((i as u32 + 1, false));
        }
    }
    None
}

/// A day as written: (day, two digits?).
fn day(s: &str) -> Option<(u32, bool)> {
    if s.is_empty() || s.len() > 2 || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let d: u32 = s.parse().ok()?;
    // "4" is d; "04" is dd; "14" fits both, and d shows it the same.
    Some((d, s.len() == 2 && s.starts_with('0')))
}

fn year(s: &str) -> Option<i64> {
    (s.len() == 4 && s.chars().all(|c| c.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Excel's serial day for a date (1900 system: 1 = 1900-01-01, with its 1900 leap-day quirk, so
/// dates before 1900-03-01 are left as text).
fn serial(y: i64, m: u32, d: u32) -> Option<f64> {
    if !(1900..=9999).contains(&y) || !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) || (y == 1900 && m < 3) {
        return None;
    }
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil), then from Excel's 1899-12-30.
    let (yy, mm) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days + 25_569) as f64)
}

fn date(t: &str) -> Option<Value> {
    let made = |y: i64, m: u32, d: u32, format: String| serial(y, m, d).map(|serial| Value::Date { serial, format });
    // ISO: yyyy-mm-dd.
    let b = t.as_bytes();
    if b.len() == 10 && b[4] == b'-' && b[7] == b'-' && t.chars().filter(|c| c.is_ascii_digit()).count() == 8 {
        let (y, m, d) = (t[..4].parse().ok()?, t[5..7].parse().ok()?, t[8..].parse().ok()?);
        return made(y, m, d, "yyyy-mm-dd".into());
    }
    let dfmt = |two: bool| if two { "dd" } else { "d" };
    let mfmt = |full: bool| if full { "mmmm" } else { "mmm" };
    // "14-Oct-2026".
    let dashed: Vec<&str> = t.split('-').collect();
    if dashed.len() == 3 {
        let (d, two) = day(dashed[0])?;
        let (m, full) = month(dashed[1])?;
        let y = year(dashed[2])?;
        return made(y, m, d, format!("{}-{}-yyyy", dfmt(two), mfmt(full)));
    }
    let words: Vec<&str> = t.split(' ').collect();
    match words.as_slice() {
        // "14 Oct 2026", "14 October 2026".
        [d, m, y] if d.chars().all(|c| c.is_ascii_digit()) => {
            let (d, two) = day(d)?;
            let (m, full) = month(m)?;
            made(year(y)?, m, d, format!("{} {} yyyy", dfmt(two), mfmt(full)))
        }
        // "Oct 14, 2026", "October 14, 2026".
        [m, d, y] => {
            let (m, full) = month(m)?;
            let (d, two) = day(d.strip_suffix(',')?)?;
            made(year(y)?, m, d, format!("{} {}, yyyy", mfmt(full), dfmt(two)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(t: &str) -> (f64, String) {
        match classify(t) {
            Value::Number { value, format } => (value, format),
            other => panic!("{t:?} is {other:?}"),
        }
    }

    fn date_of(t: &str) -> (f64, String) {
        match classify(t) {
            Value::Date { serial, format } => (serial, format),
            other => panic!("{t:?} is {other:?}"),
        }
    }

    #[test]
    fn leading_zeros_and_long_numbers_stay_text() {
        for t in ["00123", "0123", "007", "4111111111111111", "1234567890123456", "4111 1111 1111 1111", "1,234,567,890,123,456"] {
            assert_eq!(classify(t), Value::Text, "{t:?}");
        }
        assert_eq!(num("0"), (0.0, "0".into()));
        assert_eq!(num("0.50"), (0.5, "0.00".into()));
        assert_eq!(num("123456789012345"), (123_456_789_012_345.0, "0".into()), "15 digits still fit");
    }

    #[test]
    fn plain_numbers_keep_their_decimals_and_grouping() {
        assert_eq!(num("37414.00"), (37414.0, "0.00".into()));
        assert_eq!(num("1,250.5"), (1250.5, "#,##0.0".into()));
        assert_eq!(num("-12"), (-12.0, "0".into()));
        for t in ["1,25", "12,3456", "1.2.3", "12a", "1 000", "--5", "+5"] {
            assert_eq!(classify(t), Value::Text, "{t:?}");
        }
    }

    #[test]
    fn amounts_in_dollars_euros_and_pounds() {
        assert_eq!(num("$1,234.50"), (1234.5, "\"$\"#,##0.00".into()));
        assert_eq!(num("$ 12"), (12.0, "\"$ \"0".into()));
        assert_eq!(num("12.00 €"), (12.0, "0.00\" €\"".into()));
        assert_eq!(num("£7"), (7.0, "\"£\"0".into()));
        assert_eq!(num("-$5.00"), (-5.0, "\"$\"0.00".into()), "Excel shows a one-part format's negative with a leading minus");
        assert_eq!(num("($1,234.50)"), (-1234.5, "\"$\"#,##0.00;(\"$\"#,##0.00)".into()));
        assert_eq!(num("(400)"), (-400.0, "0;(0)".into()));
        assert_eq!(num("1,000.00-"), (-1000.0, "#,##0.00;#,##0.00-".into()));
        for t in ["$", "$$5", "US$5", "5$5", "$-5.00", "( 5 )", "(5"] {
            assert_eq!(classify(t), Value::Text, "{t:?}");
        }
    }

    #[test]
    fn percentages_are_fractions_with_a_percent_format() {
        assert_eq!(num("12.5%"), (0.125, "0.0%".into()));
        assert_eq!(num("12 %"), (0.12, "0\" \"%".into()));
        assert_eq!(num("-3%"), (-0.03, "0%".into()));
        assert_eq!(num("(3%)"), (-0.03, "0%;(0%)".into()));
        assert_eq!(classify("%"), Value::Text);
    }

    #[test]
    fn unambiguous_dates_become_dates_and_the_rest_stay_text() {
        assert_eq!(date_of("2026-10-14"), (46309.0, "yyyy-mm-dd".into()));
        assert_eq!(date_of("1900-03-01"), (61.0, "yyyy-mm-dd".into()), "Excel's serial, 1900 quirk included");
        assert_eq!(date_of("14 Oct 2026"), (46309.0, "d mmm yyyy".into()));
        assert_eq!(date_of("04 October 2026").1, "dd mmmm yyyy");
        assert_eq!(date_of("October 14, 2026"), (46309.0, "mmmm d, yyyy".into()));
        assert_eq!(date_of("Oct 4, 2026").1, "mmm d, yyyy");
        assert_eq!(date_of("14-Oct-2026").1, "d-mmm-yyyy");
        assert_eq!(date_of("2024-02-29").1, "yyyy-mm-dd", "leap day");
        for t in [
            "03/04/2026",
            "3.4.2026",
            "2026/03/04",
            "2023-02-29",
            "2026-13-01",
            "14 oct 2026",
            "14 OCT 2026",
            "14 Sept 2026",
            "14 de octubre de 2026",
            "Tuesday, October 14, 2026",
            "14-Oct-26",
            "1899-12-31",
            "1900-02-28",
        ] {
            assert!(!matches!(classify(t), Value::Date { .. }), "{t:?} must not become a date");
        }
    }
}
