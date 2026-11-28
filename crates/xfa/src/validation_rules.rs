//! When a field's value fails its validation's null or format test (XFA 3.3 p. 880-881), and
//! what is said then. Each choice is made in one function here, so it can follow what readers
//! are seen to do.

use crate::picture::alternatives;

/// A validation test's setting (`nullTest`, `formatTest`, `scriptTest`: `disabled`, `warning`,
/// `error`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Test {
    #[default]
    Disabled,
    Warning,
    Error,
}

impl Test {
    /// The setting `s` names; anything else is the attribute's default `default`.
    pub fn parse(s: Option<&str>, default: Test) -> Test {
        match s.map(str::trim) {
            Some("disabled") => Test::Disabled,
            Some("warning") => Test::Warning,
            Some("error") => Test::Error,
            _ => default,
        }
    }
}

/// Whether the format test fails for the (unformatted, p. 382) `value` against the validation's
/// picture clause (XFA 3.3 p. 880, formatTest), checked when the user leaves the field
/// (p. 396): the test is on, there is a value (an empty one is not tested) and a picture, and
/// the value is not one the picture describes ([`conforms`]). A picture this can't read
/// passes. The null test is not run then, as readers are seen not to (p. 396 leaves it to
/// `execValidate` and the events before submitting, printing and saving).
pub fn format_fails(test: Test, picture: Option<&str>, value: &str) -> bool {
    test != Test::Disabled && !value.trim().is_empty() && picture.is_some_and(|p| conforms(p, value) == Some(false))
}

/// What is said when the format test fails (both `warning` and `error` say it, p. 880; the
/// value stays as typed, as readers are seen to keep it): the validation's message for the
/// test, else this.
pub fn format_message(given: Option<&str>, name: &str) -> String {
    match given.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.to_string(),
        None => format!("{name}: the value is not in the expected format."),
    }
}

/// The work one format test may do over all of a picture's alternatives together, in
/// characters looked at. The test runs as the user leaves a field, outside any script's
/// limits, and a picture and a value may each be long; a dozen alternatives against a value of
/// a hundred characters are a few thousand.
const MATCH_WORK: usize = 1 << 20;

/// Whether `value` is one picture clause `picture` describes: any of its alternatives (`|`)
/// does. `None` when that can't be told: no alternative describes the value and one of them is
/// not one this reads (`text{…}`, `num{…}`, `date{…}` with digits only, or a bare clause read
/// as text; a `time{…}` alternative may be the one the value is for), or matching would take
/// more than [`MATCH_WORK`].
pub fn conforms(picture: &str, value: &str) -> Option<bool> {
    let mut work = MATCH_WORK;
    let mut unread = false;
    for clause in alternatives(picture) {
        // Reading the clause and the value against it, once each.
        work = work.checked_sub(clause.len().saturating_add(value.len()).saturating_add(1))?;
        match clause_conforms(clause.trim(), value, &mut work) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unread = true,
        }
    }
    (!unread).then_some(false)
}

/// One clause: its category (`text`, `num`, `date`; a locale in parentheses is ignored) and its
/// pattern between the braces. `work` is what is left of [`MATCH_WORK`].
fn clause_conforms(clause: &str, value: &str, work: &mut usize) -> Option<bool> {
    let (category, pattern) = match clause.find('{') {
        Some(open) => {
            let cat = clause.get(..open)?;
            let cat = cat.split('(').next().unwrap_or(cat).trim();
            let pat = clause.get(open.saturating_add(1)..)?.strip_suffix('}')?;
            (cat, pat)
        }
        None => ("text", clause),
    };
    let symbols = tokens(pattern);
    match category {
        // A text picture counts every character as entered (`X` takes a space); numbers and
        // dates are read without the spaces around them.
        "text" | "" => Some(text_conforms(&symbols, value)),
        "num" => num_conforms(&symbols, value.trim(), work),
        "date" => date_conforms(&symbols, value.trim()),
        _ => None,
    }
}

/// A pattern's symbols: a quoted literal (`'…'`, `''` a quote) is a literal of its own.
#[derive(Debug, PartialEq)]
enum Sym {
    Char(char),
    Literal(String),
}

fn tokens(pattern: &str) -> Vec<Sym> {
    let mut out = Vec::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            out.push(Sym::Char(c));
            continue;
        }
        let mut lit = String::new();
        while let Some(d) = chars.next() {
            if d == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    lit.push('\'');
                    continue;
                }
                break;
            }
            lit.push(d);
        }
        out.push(Sym::Literal(if lit.is_empty() { "'".into() } else { lit }));
    }
    out
}

/// A text pattern: `9` a digit, `A` a letter, `O` and `0` a letter or digit, `X` any
/// character, anything else itself; the value has exactly as many characters.
fn text_conforms(symbols: &[Sym], value: &str) -> bool {
    let mut v = value.chars();
    for s in symbols {
        match s {
            Sym::Literal(l) => {
                for c in l.chars() {
                    if v.next() != Some(c) {
                        return false;
                    }
                }
            }
            Sym::Char(c) => {
                let Some(x) = v.next() else { return false };
                let ok = match c {
                    '9' => x.is_ascii_digit(),
                    'A' => x.is_alphabetic(),
                    'O' | '0' => x.is_alphanumeric(),
                    'X' => true,
                    other => x == *other,
                };
                if !ok {
                    return false;
                }
            }
        }
    }
    v.next().is_none()
}

/// A numeric pattern: `9` a digit, `Z` a digit or a space, `z` a digit or nothing, `.`, `v`
/// and `V` the decimal point, `,` an optional grouping comma, `s`, `-` and `+` an optional
/// sign, `$` itself; `None` for symbols this doesn't read (currencies by locale, `E`, `CR`),
/// and when following the pattern would take more than is left of `work`.
fn num_conforms(symbols: &[Sym], value: &str, work: &mut usize) -> Option<bool> {
    /// What one step of the pattern takes: one character of a kind, or one or none.
    #[derive(Clone, Copy)]
    enum Takes {
        Digit,
        DigitOrSpace,
        Sign,
        Char(char),
    }
    let mut steps: Vec<(Takes, bool)> = Vec::new();
    for s in symbols {
        match s {
            Sym::Literal(l) => steps.extend(l.chars().map(|c| (Takes::Char(c), false))),
            Sym::Char(c) => steps.push(match c {
                '9' => (Takes::Digit, false),
                'Z' => (Takes::DigitOrSpace, false),
                'z' => (Takes::Digit, true),
                '.' => (Takes::Char('.'), false),
                // The radix that may be implied on input (XFA 3.3 p. 1178): `99V99` takes
                // `10.50` and `1050`.
                'v' | 'V' => (Takes::Char('.'), true),
                ',' => (Takes::Char(','), true),
                's' | '-' | '+' => (Takes::Sign, true),
                'E' | 'C' | 'R' | 'D' | 'B' | '(' | ')' | '%' | '8' => return None,
                other => (Takes::Char(*other), false),
            }),
        }
    }
    let v: Vec<char> = value.chars().collect();
    // Which value positions the steps so far can have reached: optional steps make the match
    // ambiguous (`zzz9` against `12`), so every way is followed at once. Too long a pair is not
    // read.
    let Some(left) = work.checked_sub(steps.len().saturating_mul(v.len().saturating_add(1))) else {
        // Nothing is left for the alternatives after it either.
        *work = 0;
        return None;
    };
    *work = left;
    let fits = |t: Takes, c: char| match t {
        Takes::Digit => c.is_ascii_digit(),
        Takes::DigitOrSpace => c.is_ascii_digit() || c == ' ',
        Takes::Sign => c == '-' || c == '+',
        Takes::Char(x) => c == x,
    };
    let mut reached = vec![false; v.len().saturating_add(1)];
    if let Some(first) = reached.first_mut() {
        *first = true;
    }
    for (t, optional) in steps {
        let mut next = vec![false; reached.len()];
        for (i, _) in reached.iter().enumerate().filter(|(_, r)| **r) {
            if optional && let Some(n) = next.get_mut(i) {
                *n = true;
            }
            if v.get(i).is_some_and(|c| fits(t, *c))
                && let Some(n) = next.get_mut(i.saturating_add(1))
            {
                *n = true;
            }
        }
        reached = next;
    }
    Some(reached.last().copied().unwrap_or(false))
}

/// A date pattern with numeric fields only: `D`/`DD` a day, `M`/`MM` a month, `YY`/`YYYY` a
/// year (one or two digits for the single letters, exactly as many for the repeated ones; a
/// day is 1 to 31 and a month 1 to 12, whatever the month and year), anything else itself;
/// `None` for names (`MMM`, `EEE`), Julian days and other letters.
fn date_conforms(symbols: &[Sym], value: &str) -> Option<bool> {
    let v: Vec<char> = value.chars().collect();
    let mut i = 0usize;
    let mut k = 0usize;
    while let Some(s) = symbols.get(k) {
        match s {
            Sym::Literal(l) => {
                for c in l.chars() {
                    if v.get(i) != Some(&c) {
                        return Some(false);
                    }
                    i = i.saturating_add(1);
                }
                k = k.saturating_add(1);
            }
            Sym::Char(c) if matches!(c, 'D' | 'M' | 'Y') => {
                let mut run = 1usize;
                while symbols.get(k.saturating_add(run)) == Some(&Sym::Char(*c)) {
                    run = run.saturating_add(1);
                }
                let (min, max) = match (c, run) {
                    ('D' | 'M', 1) => (1, 2),
                    ('D' | 'M', 2) => (2, 2),
                    ('Y', 2) => (2, 2),
                    ('Y', 4) => (4, 4),
                    _ => return None,
                };
                let digits = v.get(i..).unwrap_or_default().iter().take(max).take_while(|d| d.is_ascii_digit()).count();
                if digits < min {
                    return Some(false);
                }
                // At most four digits, so the number fits.
                let number = v
                    .get(i..i.saturating_add(digits))
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|d| d.to_digit(10))
                    .fold(0u32, |n, d| n.saturating_mul(10).saturating_add(d));
                let in_range = match c {
                    'D' => (1..=31).contains(&number),
                    'M' => (1..=12).contains(&number),
                    _ => true,
                };
                if !in_range {
                    return Some(false);
                }
                i = i.saturating_add(digits);
                k = k.saturating_add(run);
            }
            Sym::Char(c) if c.is_ascii_alphabetic() => return None,
            Sym::Char(c) => {
                if v.get(i) != Some(c) {
                    return Some(false);
                }
                i = i.saturating_add(1);
                k = k.saturating_add(1);
            }
        }
    }
    Some(i == v.len())
}

/// Whether a failed format test of setting `t` leaves the field's script test unreported: an
/// error may skip the remaining validations (XFA 3.3 p. 385), and this one message is enough;
/// after a warning the script test still reports.
pub fn later_tests_skipped(t: Test) -> bool {
    t == Test::Error
}

/// Whether a value's `override` attribute (`0` or `1`, p. 883) says the user overrode its
/// calculation.
pub fn is_override(s: Option<&str>) -> bool {
    s.map(str::trim) == Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Picture clauses (XFA 3.3 ch. 24) as the format test reads them: text, numeric and date
    /// patterns, quoted literals, alternatives; a pattern this can't read is not a failure.
    /// `v` and `V` are the radix the input may leave out (XFA 3.3 p. 1178); `.` must be there.
    #[test]
    fn an_implied_radix_may_be_left_out() {
        assert_eq!(conforms("num{99V99}", "1050"), Some(true));
        assert_eq!(conforms("num{99v99}", "10.50"), Some(true));
        assert_eq!(conforms("num{99.99}", "1050"), Some(false));
        assert_eq!(conforms("num{99.99}", "10.50"), Some(true));
    }

    /// A text picture takes the value as entered, spaces counted (`X` is any character); a
    /// number or a date is read without the spaces around it.
    #[test]
    fn text_pictures_count_spaces_and_numbers_skip_them() {
        assert_eq!(conforms("text{XX}", "a "), Some(true));
        assert_eq!(conforms("text{X}", " a"), Some(false));
        assert_eq!(conforms("num{99}", " 12 "), Some(true));
        assert_eq!(conforms("date{YYYYMMDD}", " 20261112 "), Some(true));
    }

    #[test]
    fn values_are_matched_against_picture_clauses() {
        let cases: &[(&str, &str, Option<bool>)] = &[
            ("text{9999}", "1234", Some(true)),
            ("text{9999}", "abc", Some(false)),
            ("text{9999}", "12345", Some(false)),
            ("text{A9A 9A9}", "K1A 0B1", Some(true)),
            ("text{OOO-XX}", "a1B-?!", Some(true)),
            ("text{'('999')' 999}", "(613) 555", Some(true)),
            ("text{'it''s' 9}", "it's 5", Some(true)),
            ("999", "123", Some(true)),
            ("num{zzz9.99}", "12.50", Some(true)),
            ("num{zzz9.99}", "-12.50", Some(false)),
            ("num{s9.9}", "-1.5", Some(true)),
            ("num{z,zz9}", "1,234", Some(true)),
            ("num{z,zz9}", "1234", Some(true)),
            ("num{9}", "x", Some(false)),
            ("num{$9.99}", "$1.50", Some(true)),
            ("num{9E}", "1", None),
            ("date{YYYY-MM-DD}", "2026-10-27", Some(true)),
            ("date{YYYY-MM-DD}", "26-10-27", Some(false)),
            ("date{D/M/YY}", "7/10/26", Some(true)),
            ("date{MMM D, YYYY}", "Oct 7, 2026", None),
            ("date(en_US){MM/DD/YYYY}", "10/27/2026", Some(true)),
            ("time{HH:MM}", "10:00", None),
            ("text{9999}|text{AAAA}", "abcd", Some(true)),
            // An alternative this can't read may take the value: no failure is said.
            ("text{9999}|time{HH}", "abcd", None),
            ("text{9999}|time{HH:MM}", "10:00", None),
            ("time{HH}|text{9999}", "1234", Some(true)),
            ("date{YYYY-MM-DD}", "2026-13-01", Some(false)),
            ("date{YYYY-MM-DD}", "2026-12-32", Some(false)),
            ("date{YYYY-MM-DD}", "2026-00-10", Some(false)),
            ("date{YYYY-MM-DD}", "2026-10-00", Some(false)),
            ("date{YYYY-MM-DD}", "2026-12-31", Some(true)),
            ("date{D/M/YY}", "31/12/26", Some(true)),
            ("date{D/M/YY}", "32/1/26", Some(false)),
            ("date{D/M/YY}", "0/1/26", Some(false)),
            ("time{HH}|date{EEE}", "x", None),
        ];
        for (picture, value, want) in cases {
            assert_eq!(conforms(picture, value), *want, "{picture} {value}");
        }
    }

    /// One format test's work is bounded over all of a picture's alternatives together, not
    /// for each: past the bound the picture is not read (no failure is said), however many
    /// alternatives follow. Each alternative here is within the bound by itself.
    #[test]
    fn matching_work_is_bounded_over_all_alternatives() {
        let value = "1".repeat(10_000);
        let few = vec!["num{9}"; 50].join("|");
        assert_eq!(conforms(&few, &value), Some(false), "50 alternatives are within the bound");
        let many = vec!["num{9}"; 200].join("|");
        assert_eq!(conforms(&many, &value), None, "200 are past it");
        // A long pattern and value are past it by themselves, and nothing after them is read.
        let long = format!("num{{{}}}|text{{9}}", "z".repeat(2_000));
        assert_eq!(conforms(&long, &"1".repeat(1_000)), None);
        assert_eq!(conforms(&long, "1"), Some(true));
    }

    /// The format test is off, or passes, for empty values and pictures it can't read; both
    /// `warning` and `error` fail a value the picture doesn't describe. The message is the
    /// form's when it has one.
    #[test]
    fn the_format_test_and_its_message() {
        assert!(format_fails(Test::Error, Some("text{99}"), "abc"));
        assert!(format_fails(Test::Warning, Some("text{99}"), "abc"));
        assert!(!format_fails(Test::Disabled, Some("text{99}"), "abc"));
        assert!(!format_fails(Test::Error, Some("text{99}"), "  "));
        assert!(!format_fails(Test::Error, None, "abc"));
        assert!(!format_fails(Test::Error, Some("time{HH}"), "abc"));
        assert_eq!(format_message(Some(" Custom "), "V2"), "Custom");
        assert_eq!(format_message(Some(""), "V2"), "V2: the value is not in the expected format.");
        assert_eq!(Test::parse(Some("error"), Test::Warning), Test::Error);
        assert_eq!(Test::parse(Some("bogus"), Test::Warning), Test::Warning);
        assert!(later_tests_skipped(Test::Error) && !later_tests_skipped(Test::Warning));
    }

    /// `override="1"` marks a value as one calculations leave alone.
    #[test]
    fn an_overridden_value_is_marked() {
        assert!(is_override(Some(" 1 ")) && !is_override(Some("0")) && !is_override(None));
    }
}
