//! What a cell's text is, for the Excel export: a number, an amount, a percentage or a date that
//! Excel can compute with, with a number format that shows it exactly as the PDF does - or text.
//! Anything that could be read two ways stays text.
//!
//! - Leading zeros stay text ("00123", "0123": codes, not quantities).
//! - More than 15 significant digits stay text (card and account numbers: Excel keeps 15).
//! - Plain numbers, thousands groups of three, a decimal mark: a number, its decimals and
//!   grouping kept in the format. Which of `.` and `,` is the decimal mark comes from the
//!   document's language ([`Separators`]); without one, a single `.` or `,` followed by exactly
//!   three digits (`1.234`, `1,234`: a thousand and more, or just over one?) stays text.
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

/// How a document writes its numbers: which of `.` and `,` is the decimal mark (the other groups
/// thousands). From its language with [`Separators::for_language`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Separators {
    /// No language, or one whose numbers could be written either way: a number that reads
    /// differently with each stays text.
    #[default]
    Unknown,
    /// `1,234.5`: English, Chinese, Japanese, Korean, Hebrew, Spanish in the Americas north of the
    /// equator, ...
    DecimalPoint,
    /// `1.234,5`: German, French, Spanish, Italian, Portuguese, Dutch, Russian, Polish, ...
    DecimalComma,
}

impl Separators {
    /// The separators a BCP 47 language tag (`/Lang`, "de-DE", "en") writes numbers with. A
    /// region where the usual one isn't the rule (Switzerland groups with an apostrophe, South
    /// Africa writes a decimal comma in English) reads as [`Separators::Unknown`], as does a
    /// language this doesn't list, or Latin America as a whole (`419`).
    pub fn for_language(tag: Option<&str>) -> Self {
        let Some(tag) = tag.map(str::trim).filter(|t| !t.is_empty()) else { return Self::Unknown };
        let mut parts = tag.split(['-', '_']);
        let lang = parts.next().unwrap_or("").to_ascii_lowercase();
        // The region: two letters or three digits after the language (and any script).
        let region = parts.find(|p| p.len() == 2 || (p.len() == 3 && p.chars().all(|c| c.is_ascii_digit()))).map(str::to_ascii_uppercase);
        let region = region.as_deref();
        // Latin America as a whole (419) holds both: Mexico's decimal point, Argentina's comma.
        if matches!(region, Some("CH" | "LI" | "419")) {
            return Self::Unknown;
        }
        match lang.as_str() {
            "en" if region == Some("ZA") => Self::Unknown,
            "en" | "zh" | "ja" | "ko" | "he" | "th" | "hi" | "ms" | "fil" | "tl" | "ga" | "cy" | "mt" | "sw" => Self::DecimalPoint,
            "es" if matches!(region, Some("MX" | "US" | "PR" | "DO" | "GT" | "HN" | "NI" | "PA" | "SV")) => Self::DecimalPoint,
            "de" | "fr" | "es" | "it" | "pt" | "nl" | "ru" | "pl" | "cs" | "sk" | "sl" | "hr" | "sr" | "bs" | "bg" | "uk" | "be" | "ro" | "hu"
            | "tr" | "el" | "da" | "nb" | "nn" | "no" | "sv" | "fi" | "et" | "lv" | "lt" | "is" | "ca" | "gl" | "eu" | "id" | "vi" | "az" | "kk"
            | "ka" | "hy" | "mk" | "sq" => Self::DecimalComma,
            _ => Self::Unknown,
        }
    }
}

/// [`classify_in`] for a document whose language isn't known.
pub fn classify(text: &str) -> Value {
    classify_in(text, Separators::Unknown)
}

/// What `text` is, in a document that writes its numbers with `separators`.
pub fn classify_in(text: &str, separators: Separators) -> Value {
    let t = text.trim();
    if t.is_empty() || t.len() > 64 {
        return Value::Text;
    }
    if let Some(v) = date(t) {
        return v;
    }
    amount(t, separators).unwrap_or(Value::Text)
}

/// A number written with an optional thousands grouping and decimal mark: (value, decimals,
/// grouped). None for leading zeros, more than 15 significant digits, a number `separators`
/// leave two ways to read, or anything else.
fn bare_number(s: &str, separators: Separators) -> Option<(f64, usize, bool)> {
    if s.is_empty() || !s.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (point, group) = match separators {
        Separators::DecimalComma => (',', '.'),
        Separators::DecimalPoint | Separators::Unknown => ('.', ','),
    };
    if separators == Separators::Unknown && two_ways(s) {
        return None;
    }
    let (int, frac) = match s.split_once(point) {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    if let Some(f) = frac
        && (f.is_empty() || !f.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    if !int.chars().all(|c| c.is_ascii_digit() || c == group) {
        return None;
    }
    let groups: Vec<&str> = int.split(group).collect();
    let grouped = groups.len() > 1;
    if grouped && (groups[0].is_empty() || groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3)) {
        return None;
    }
    let digits: String = int.chars().filter(|c| *c != group).collect();
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

/// Whether `s` reads as a different number with a decimal point than with a decimal comma: one
/// `.` or `,`, one to three digits before it (not a leading zero) and exactly three after.
fn two_ways(s: &str) -> bool {
    let mut marks = s.match_indices(['.', ',']);
    let (Some((i, _)), None) = (marks.next(), marks.next()) else { return false };
    let (int, frac) = (&s[..i], &s[i + 1..]);
    (1..=3).contains(&int.len()) && !int.starts_with('0') && frac.len() == 3 && int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit())
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
fn amount(t: &str, separators: Separators) -> Option<Value> {
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
    let (mut value, decimals, grouped) = bare_number(number, separators)?;
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

    fn num_in(t: &str, separators: Separators) -> (f64, String) {
        match classify_in(t, separators) {
            Value::Number { value, format } => (value, format),
            other => panic!("{t:?} is {other:?} with {separators:?}"),
        }
    }

    #[test]
    fn a_number_that_reads_two_ways_follows_the_documents_language_or_stays_text() {
        use Separators::{DecimalComma, DecimalPoint, Unknown};
        // Without a language, one separator and exactly three digits could be either.
        for t in ["1.234", "1.234 €", "1,234", "€1,234", "(1.234)", "1,234%"] {
            assert_eq!(classify_in(t, Unknown), Value::Text, "{t:?}");
        }
        // Only one reading, whatever the language: two groups, both marks, other digit counts.
        assert_eq!(num_in("1,234,567", Unknown), (1_234_567.0, "#,##0".into()));
        assert_eq!(num_in("1,234.56", Unknown), (1234.56, "#,##0.00".into()));
        assert_eq!(num_in("0.125", Unknown), (0.125, "0.000".into()), "a leading zero groups nothing");
        assert_eq!(num_in("12.50 €", Unknown), (12.5, "0.00\" €\"".into()));
        // German (or French): "." groups thousands and "," is the decimal mark.
        assert_eq!(num_in("1.234", DecimalComma), (1234.0, "#,##0".into()));
        assert_eq!(num_in("1.234 €", DecimalComma), (1234.0, "#,##0\" €\"".into()));
        assert_eq!(num_in("1,234", DecimalComma), (1.234, "0.000".into()));
        assert_eq!(num_in("12,50 €", DecimalComma), (12.5, "0.00\" €\"".into()));
        assert_eq!(num_in("1.234,56 €", DecimalComma), (1234.56, "#,##0.00\" €\"".into()));
        assert_eq!(num_in("12,5 %", DecimalComma), (0.125, "0.0\" \"%".into()));
        assert_eq!(classify_in("1,234.56", DecimalComma), Value::Text, "English grouping on a German page");
        // English: as before.
        assert_eq!(num_in("1.234", DecimalPoint), (1.234, "0.000".into()));
        assert_eq!(num_in("1,234", DecimalPoint), (1234.0, "#,##0".into()));
        assert_eq!(classify_in("12,50 €", DecimalPoint), Value::Text);
        assert_eq!(classify_in("1.234,56 €", DecimalPoint), Value::Text);
    }

    #[test]
    fn the_language_tag_says_which_mark_is_the_decimal_one() {
        use Separators::{DecimalComma, DecimalPoint, Unknown};
        for (tag, want) in [
            (Some("de-DE"), DecimalComma),
            (Some("fr"), DecimalComma),
            (Some("es-ES"), DecimalComma),
            (Some("pt_BR"), DecimalComma),
            (Some("sr-Latn-RS"), DecimalComma),
            (Some("en-US"), DecimalPoint),
            (Some("EN"), DecimalPoint),
            (Some("zh-Hant-TW"), DecimalPoint),
            (Some("es-MX"), DecimalPoint),
            (Some("es-PR"), DecimalPoint),
            (Some("es-419"), Unknown),
            (Some("es-AR"), DecimalComma),
            (Some("de-CH"), Unknown),
            (Some("fr-CH"), Unknown),
            (Some("en-ZA"), Unknown),
            (Some("x-klingon"), Unknown),
            (Some(" "), Unknown),
            (None, Unknown),
        ] {
            assert_eq!(Separators::for_language(tag), want, "{tag:?}");
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
