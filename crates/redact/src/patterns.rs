//! Search & Redact patterns (Acrobat's Find Text ▸ Patterns): phone numbers, email addresses,
//! payment cards, bank accounts, network addresses, postcodes, Social Security numbers and dates.
//! Each matcher works on a page's text as characters and returns character ranges; callers map
//! them to glyphs and areas.
//!
//! The matchers are hand-rolled scanners, not regular expressions: every start position does a
//! bounded amount of work, so the whole search is linear in the text and cannot be driven into
//! catastrophic backtracking by hostile page content. Text is normalised first (Unicode digits,
//! full-width forms, soft hyphens, zero-width characters, line breaks), and the ranges reported
//! are mapped back to indices into the text the caller passed. Input beyond [`MAX_INPUT_CHARS`]
//! is refused with an error by [`try_find`] rather than silently truncated.

use std::ops::Range;

/// Most characters of page text one search accepts. Larger input is an error, never truncated.
pub const MAX_INPUT_CHARS: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    /// US/NANP phone numbers (10 digits, or 11 with a leading 1), grouped by separators.
    Phone,
    Email,
    /// 13-19 digit payment card numbers (Luhn-checked), grouped by spaces or hyphens.
    CreditCard,
    Ssn,
    /// Any common date, numeric or with a month name.
    Date,
    /// E.164-style international numbers: `+` or `00`, then 8-15 digits.
    PhoneIntl,
    Ipv4,
    Ipv6,
    /// IBANs of the countries in the registry, length- and mod-97-checked.
    Iban,
    UkPostcode,
    UsZip,
    /// `yyyy-mm-dd`, calendar-checked.
    DateIso,
    /// `m/d/yy` and `m/d/yyyy`, calendar-checked.
    DateUs,
}

pub const PATTERNS: [Pattern; 13] = [
    Pattern::Phone,
    Pattern::Email,
    Pattern::CreditCard,
    Pattern::Ssn,
    Pattern::Date,
    Pattern::PhoneIntl,
    Pattern::Ipv4,
    Pattern::Ipv6,
    Pattern::Iban,
    Pattern::UkPostcode,
    Pattern::UsZip,
    Pattern::DateIso,
    Pattern::DateUs,
];

impl Pattern {
    pub fn label(self) -> &'static str {
        match self {
            Pattern::Phone => "Phone Numbers",
            Pattern::Email => "Email Addresses",
            Pattern::CreditCard => "Credit Cards",
            Pattern::Ssn => "Social Security Numbers",
            Pattern::Date => "Dates",
            Pattern::PhoneIntl => "Phone Numbers (International)",
            Pattern::Ipv4 => "IPv4 Addresses",
            Pattern::Ipv6 => "IPv6 Addresses",
            Pattern::Iban => "IBANs",
            Pattern::UkPostcode => "UK Postcodes",
            Pattern::UsZip => "US ZIP Codes",
            Pattern::DateIso => "Dates (yyyy-mm-dd)",
            Pattern::DateUs => "Dates (m/d/yyyy)",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Pattern::Phone => "phone",
            Pattern::Email => "email",
            Pattern::CreditCard => "credit-card",
            Pattern::Ssn => "ssn",
            Pattern::Date => "date",
            Pattern::PhoneIntl => "phone-intl",
            Pattern::Ipv4 => "ipv4",
            Pattern::Ipv6 => "ipv6",
            Pattern::Iban => "iban",
            Pattern::UkPostcode => "uk-postcode",
            Pattern::UsZip => "us-zip",
            Pattern::DateIso => "date-iso",
            Pattern::DateUs => "date-us",
        }
    }

    /// The pattern with this id; `phone-us` is accepted as an alias of `phone`.
    pub fn from_id(id: &str) -> Option<Self> {
        if id == "phone-us" {
            return Some(Pattern::Phone);
        }
        PATTERNS.into_iter().find(|p| p.id() == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("page text of {len} characters exceeds the {max}-character limit for pattern search")]
    InputTooLarge { len: usize, max: usize },
}

// --- Normalisation -----------------------------------------------------------------------------

/// Zero digits of the Unicode decimal-digit blocks folded to ASCII (each block has ten digits).
const DIGIT_ZEROS: [u32; 21] = [
    0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x17E0, 0x1810,
    0x1D7CE, 0x1D7D8, 0x1D7E2,
];

/// The ASCII form of `c` the matchers see; `None` for characters that carry no text (soft hyphen,
/// zero-width and bidi controls) and are dropped.
fn fold(c: char) -> Option<char> {
    let u = c as u32;
    Some(match c {
        '\u{AD}' | '\u{34F}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' => {
            return None;
        }
        // Hyphens, dashes and the minus sign.
        '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{FE58}' | '\u{FE63}' => '-',
        // Full-width ASCII.
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(u - 0xFEE0).unwrap_or(c),
        _ => match DIGIT_ZEROS.iter().find_map(|z| u.checked_sub(*z).filter(|d| *d < 10)).and_then(|d| char::from_digit(d, 10)) {
            Some(d) => d,
            None if c.is_whitespace() || c.is_control() => ' ',
            None => c,
        },
    })
}

/// Page text folded for matching, with the original index of every folded character.
struct Normalized {
    chars: Vec<char>,
    origin: Vec<usize>,
}

impl Normalized {
    fn new(text: &[char]) -> Self {
        let mut chars: Vec<char> = Vec::with_capacity(text.len());
        let mut origin: Vec<usize> = Vec::with_capacity(text.len());
        let back = |chars: &[char], n: usize| chars.len().checked_sub(n).and_then(|k| chars.get(k)).copied();
        for (i, &c) in text.iter().enumerate() {
            let Some(c) = fold(c) else { continue };
            if c == ' ' && back(&chars, 1) == Some(' ') {
                continue;
            }
            // "4111- 1111" and "4111 -1111": a hyphen between digits joins them across the break.
            let joins = match c {
                '-' => back(&chars, 1) == Some(' ') && back(&chars, 2).is_some_and(|p| p.is_ascii_digit()),
                d if d.is_ascii_digit() => {
                    back(&chars, 1) == Some(' ') && back(&chars, 2) == Some('-') && back(&chars, 3).is_some_and(|p| p.is_ascii_digit())
                }
                _ => false,
            };
            if joins {
                chars.pop();
                origin.pop();
            }
            chars.push(c);
            origin.push(i);
        }
        Self { chars, origin }
    }

    /// The range of the original text covering a range of the folded text (including characters
    /// dropped inside it).
    fn to_original(&self, r: &Range<usize>) -> Option<Range<usize>> {
        let start = *self.origin.get(r.start)?;
        let end = self.origin.get(r.end.checked_sub(1)?)?.checked_add(1)?;
        Some(start..end)
    }
}

// --- Scanning helpers --------------------------------------------------------------------------

/// The character at `i`, or NUL past either end.
fn ch(s: &[char], i: usize) -> char {
    s.get(i).copied().unwrap_or('\0')
}

fn prev(s: &[char], i: usize) -> char {
    i.checked_sub(1).map_or('\0', |p| ch(s, p))
}

fn dig(s: &[char], i: usize) -> bool {
    ch(s, i).is_ascii_digit()
}

fn word(c: char) -> bool {
    c.is_alphanumeric()
}

/// Number of ASCII digits in a row from `i`, counted up to 32 (no matcher needs more, and the cap
/// keeps long digit runs from costing quadratic time).
fn count_digits(s: &[char], i: usize) -> usize {
    s.get(i..).map_or(0, |r| r.iter().take(32).take_while(|c| c.is_ascii_digit()).count())
}

/// Value of the `n` digits at `i` (the caller has checked they are digits).
fn value(s: &[char], i: usize, n: usize) -> u32 {
    (i..i.saturating_add(n)).fold(0, |v, k| v.saturating_mul(10).saturating_add(ch(s, k).to_digit(10).unwrap_or(0)))
}

/// A number glued to the digits or letters before it, like `12-` before `555-123-4567`.
fn glued_before(s: &[char], i: usize) -> bool {
    let p = prev(s, i);
    word(p) || p == '+' || (matches!(p, '-' | '.' | '/') && word(i.checked_sub(2).map_or('\0', |k| ch(s, k))))
}

/// Digits grouped by separators, from `start`: (end after the last digit, digit count). At most
/// two separator characters may follow each other (`") "`) and the run never ends on one.
fn grouped_digits(s: &[char], start: usize, seps: &[char], max_len: usize) -> (usize, usize) {
    let (mut end, mut n, mut run) = (start, 0, 0);
    for (k, &c) in s.iter().enumerate().skip(start).take(max_len) {
        if c.is_ascii_digit() {
            n += 1;
            run = 0;
            end = k + 1;
        } else if seps.contains(&c) && run < 2 {
            run += 1;
        } else {
            break;
        }
    }
    (end, n)
}

fn digit_values(s: &[char], range: Range<usize>) -> Vec<u32> {
    s.get(range).map_or_else(Vec::new, |r| r.iter().filter_map(|c| c.to_digit(10)).collect())
}

// --- Phone -------------------------------------------------------------------------------------

/// Whether `i` follows a `+` country code other than 1 and a space, like the `7911` in `+44 7911`.
fn after_foreign_code(s: &[char], i: usize) -> bool {
    let mut j = i;
    if prev(s, j) != ' ' {
        return false;
    }
    j -= 1;
    let digits = (1..=3).take_while(|k| j.checked_sub(*k).is_some_and(|p| dig(s, p))).count();
    let sign = j.checked_sub(digits + 1).map_or('\0', |p| ch(s, p));
    sign == '+' && digits > 0 && !(digits == 1 && j.checked_sub(1).is_some_and(|p| ch(s, p) == '1'))
}

fn phone(s: &[char], i: usize) -> Option<usize> {
    let c = ch(s, i);
    if !(c.is_ascii_digit() || matches!(c, '+' | '(') && dig(s, i + 1)) || glued_before(s, i) || after_foreign_code(s, i) {
        return None;
    }
    let plus = c == '+';
    let first = if plus { i + 1 } else { i };
    let (end, n) = grouped_digits(s, first, &[' ', '-', '.', '(', ')'], 24);
    let digits = digit_values(s, first..end);
    let grouped = s.get(first..end).is_some_and(|r| r.iter().any(|c| !c.is_ascii_digit()));
    // 10 digits, or 11 with the country code 1, grouped by at least one separator.
    let count_ok = n == 10 || n == 11 && digits.first() == Some(&1);
    if !count_ok || !(grouped || plus) || dig(s, end) {
        return None;
    }
    Some(end - i)
}

fn phone_intl(s: &[char], i: usize) -> Option<usize> {
    let (first, plus) = match (ch(s, i), ch(s, i + 1)) {
        ('+', d) if d.is_ascii_digit() => (i + 1, true),
        ('0', '0') => (i + 2, false),
        _ => return None,
    };
    if glued_before(s, i) || !matches!(ch(s, first), '1'..='9') {
        return None;
    }
    let (end, n) = grouped_digits(s, first, &[' ', '-', '.', '(', ')'], 32);
    let grouped = s.get(first..end).is_some_and(|r| r.iter().any(|c| !c.is_ascii_digit()));
    // E.164 allows 15 digits at most; without a `+` demand grouping so plain numbers don't match.
    ((8..=15).contains(&n) && (plus || grouped) && !dig(s, end)).then_some(end - i)
}

// --- Email -------------------------------------------------------------------------------------

fn email(s: &[char], i: usize) -> Option<usize> {
    let local = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
    // The local part may start with `_` or `+` too (`_svc@`, `+tag@`), but not with a dot.
    if !(ch(s, i).is_ascii_alphanumeric() || matches!(ch(s, i), '_' | '+')) || local(prev(s, i)) {
        return None;
    }
    let mut j = i;
    while local(ch(s, j)) {
        j += 1;
    }
    if ch(s, j) != '@' {
        return None;
    }
    let d0 = j + 1;
    let mut k = d0;
    while ch(s, k).is_ascii_alphanumeric() || ch(s, k) == '-' || ch(s, k) == '.' && ch(s, k + 1).is_ascii_alphanumeric() {
        k += 1;
    }
    // A trailing hyphen is punctuation, not part of the domain.
    while k > d0 && ch(s, k - 1) == '-' {
        k -= 1;
    }
    let domain: String = s.get(d0..k)?.iter().collect();
    let tld = domain.rsplit('.').next().unwrap_or("");
    (domain.contains('.') && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())).then_some(k - i)
}

// --- Credit cards ------------------------------------------------------------------------------

fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(k, d)| {
            if k % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                *d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Card numbers start with 2-6 (Mastercard 2/5, Amex/JCB/Diners 3, Visa 4, Discover/UnionPay 6),
/// and a run of one repeated digit is a placeholder, not a card.
fn plausible_card(digits: &[u32]) -> bool {
    digits.first().is_some_and(|d| (2..=6).contains(d)) && digits.iter().any(|d| Some(d) != digits.first())
}

fn credit_card(s: &[char], i: usize) -> Option<usize> {
    if !dig(s, i) || prev(s, i).is_ascii_digit() || prev(s, i) == '-' && i.checked_sub(2).is_some_and(|k| dig(s, k)) {
        return None;
    }
    // Digit groups joined by one space or hyphen: (end, length) of each, at most 19 digits in all.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let (mut pos, mut total) = (i, 0);
    loop {
        let g = count_digits(s, pos);
        total += g;
        if g == 0 || total > 19 {
            break;
        }
        groups.push((pos + g, g));
        pos += g;
        if matches!(ch(s, pos), ' ' | '-') && dig(s, pos + 1) {
            pos += 1;
        } else {
            break;
        }
    }
    // The longest run of leading groups that is a plausible, checksum-valid card number.
    for k in (1..=groups.len()).rev() {
        let (end, _) = *groups.get(k - 1)?;
        let sizes: Vec<usize> = groups.iter().take(k).map(|g| g.1).collect();
        let n: usize = sizes.iter().sum();
        let shaped = k == 1 || sizes.iter().take(k - 1).all(|g| (4..=6).contains(g)) && sizes.last().is_some_and(|g| (1..=6).contains(g));
        if !(13..=19).contains(&n) || !shaped {
            continue;
        }
        let digits = digit_values(s, i..end);
        if luhn(&digits) && plausible_card(&digits) {
            return Some(end - i);
        }
    }
    None
}

// --- Social Security numbers -------------------------------------------------------------------

/// Whether the text just before `i` is the label "SSN" (with optional `:`/`#`/space after it).
fn ssn_label(s: &[char], i: usize) -> bool {
    let mut j = i;
    while j > 0 && matches!(prev(s, j), ' ' | ':' | '#' | '.') {
        j -= 1;
    }
    let tag: String = (j.saturating_sub(3)..j).map(|k| ch(s, k).to_ascii_lowercase()).collect();
    tag == "ssn" && !word(j.checked_sub(4).map_or('\0', |k| ch(s, k)))
}

fn ssn(s: &[char], i: usize) -> Option<usize> {
    if !dig(s, i) || prev(s, i).is_ascii_digit() || prev(s, i) == '-' && i.checked_sub(2).is_some_and(|k| dig(s, k)) {
        return None;
    }
    let sep = |k: usize| matches!(ch(s, k), '-' | ' ');
    let area = value(s, i, 3);
    let (group, serial, len) = if count_digits(s, i) == 3 && sep(i + 3) && count_digits(s, i + 4) == 2 && sep(i + 6) && count_digits(s, i + 7) == 4 {
        (value(s, i + 4, 2), value(s, i + 7, 4), 11)
    } else if count_digits(s, i) == 9 && ssn_label(s, i) {
        // Nine bare digits only after an explicit "SSN" label.
        (value(s, i + 3, 2), value(s, i + 5, 4), 9)
    } else {
        return None;
    };
    // Never issued: area 000, 666 and 900-999, group 00, serial 0000.
    let valid = area != 0 && area != 666 && area < 900 && group != 0 && serial != 0;
    (valid && !dig(s, i + len) && !(ch(s, i + len) == '-' && dig(s, i + len + 1))).then_some(len)
}

// --- Dates -------------------------------------------------------------------------------------

const MONTHS: [&str; 12] = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];

/// A month name or its abbreviation (three letters, `Sept` for September; optionally with a full
/// stop) at `i`. Names in other languages are not recognised.
fn month(s: &[char], i: usize) -> Option<usize> {
    if word(prev(s, i)) {
        return None;
    }
    let rest: String = s.get(i..s.len().min(i + 10))?.iter().collect::<String>().to_lowercase();
    for m in MONTHS {
        let sept = (m == "september").then_some("sept");
        for cand in [Some(m), sept, m.get(..3)].into_iter().flatten() {
            if rest.starts_with(cand) && !rest.get(cand.len()..).is_some_and(|t| t.starts_with(|c: char| c.is_alphabetic())) {
                let mut n = cand.chars().count();
                if cand.len() < m.len() && ch(s, i + n) == '.' {
                    n += 1;
                }
                return Some(n);
            }
        }
    }
    None
}

fn number(s: &[char], i: usize, min: usize, max: usize) -> Option<usize> {
    let n = count_digits(s, i);
    (n >= min && n <= max).then_some(n)
}

fn date(s: &[char], i: usize) -> Option<usize> {
    if word(prev(s, i)) {
        return None;
    }
    let spaces = |k: usize| s.get(k..).map_or(0, |r| r.iter().take(8).take_while(|c| **c == ' ').count());
    // Numeric: m/d/yy(yy), m-d-yyyy, d.m.yyyy, yyyy-mm-dd.
    if let Some(a) = number(s, i, 1, 4) {
        let sep = ch(s, i + a);
        if matches!(sep, '/' | '-' | '.')
            && let Some(b) = number(s, i + a + 1, 1, 2)
            && ch(s, i + a + 1 + b) == sep
            && let Some(c) = number(s, i + a + b + 2, if a == 4 { 1 } else { 2 }, if a == 4 { 2 } else { 4 })
            && (a <= 2 || a == 4)
            && !dig(s, i + a + b + 2 + c)
        {
            return Some(a + b + c + 2);
        }
        // "5 January 2024".
        if a <= 2
            && let k = i + a + spaces(i + a)
            && k > i + a
            && let Some(m) = month(s, k)
        {
            let k2 = k + m + spaces(k + m);
            if let Some(y) = number(s, k2, 4, 4) {
                return Some(k2 + y - i);
            }
        }
        return None;
    }
    // "January 5, 2024" / "Jan. 5 2024".
    let m = month(s, i)?;
    let k = i + m + spaces(i + m);
    let d = number(s, k, 1, 2)?;
    let mut k2 = k + d;
    if ch(s, k2) == ',' {
        k2 += 1;
    }
    k2 += spaces(k2);
    let y = number(s, k2, 4, 4)?;
    Some(k2 + y - i)
}

fn days_in_month(month: u32, year: Option<u32>) -> u32 {
    match month {
        2 => match year {
            Some(y) if !(y.is_multiple_of(4) && !y.is_multiple_of(100) || y.is_multiple_of(400)) => 28,
            _ => 29,
        },
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A calendar-valid month and day (`year` unknown: February 29 allowed).
fn calendar(month: u32, day: u32, year: Option<u32>) -> bool {
    (1..=12).contains(&month) && (1..=days_in_month(month, year)).contains(&day)
}

/// Whether a date at `i..end` is glued to neighbouring digits or `-`/`/`/`.` chains.
fn date_glued(s: &[char], i: usize, end: usize, seps: &[char]) -> bool {
    let p = prev(s, i);
    let n = ch(s, end);
    word(p) && p != '\0' || seps.contains(&p) && word(i.checked_sub(2).map_or('\0', |k| ch(s, k))) || word(n) || seps.contains(&n) && dig(s, end + 1)
}

fn date_iso(s: &[char], i: usize) -> Option<usize> {
    if count_digits(s, i) != 4 || ch(s, i + 4) != '-' || count_digits(s, i + 5) != 2 || ch(s, i + 7) != '-' || count_digits(s, i + 8) != 2 {
        return None;
    }
    let (y, m, d) = (value(s, i, 4), value(s, i + 5, 2), value(s, i + 8, 2));
    let end = i + 10;
    // "2026-10-01T09:30" is a timestamp whose date part still counts.
    let stamp = ch(s, end) == 'T' && dig(s, end + 1);
    let glued = if stamp { word(prev(s, i)) } else { date_glued(s, i, end, &['-', '/', '.']) };
    ((1000..=2999).contains(&y) && calendar(m, d, Some(y)) && !glued).then_some(10)
}

fn date_us(s: &[char], i: usize) -> Option<usize> {
    let a = number(s, i, 1, 2)?;
    if ch(s, i + a) != '/' {
        return None;
    }
    let b = number(s, i + a + 1, 1, 2)?;
    if ch(s, i + a + 1 + b) != '/' {
        return None;
    }
    let y0 = i + a + b + 2;
    let c = count_digits(s, y0);
    if c != 2 && c != 4 {
        return None;
    }
    let year = value(s, y0, c);
    let year = if c == 4 { Some(year) } else { None };
    let end = y0 + c;
    let valid = calendar(value(s, i, a), value(s, i + a + 1, b), year) && year.is_none_or(|y| (1000..=2999).contains(&y));
    (valid && !date_glued(s, i, end, &['/', '-', '.'])).then_some(end - i)
}

// --- Network addresses -------------------------------------------------------------------------

/// Four dot-separated octets (0-255, no leading zeros) exactly filling `s`.
fn is_ipv4(s: &[char]) -> bool {
    let mut parts = 0;
    for part in s.split(|c| *c == '.') {
        let v = value(part, 0, part.len());
        let plain = !part.is_empty() && part.len() <= 3 && part.iter().all(char::is_ascii_digit) && (part.len() == 1 || part.first() != Some(&'0'));
        if !plain || v > 255 {
            return false;
        }
        parts += 1;
    }
    parts == 4
}

fn ipv4(s: &[char], i: usize) -> Option<usize> {
    if !dig(s, i) || glued_before(s, i) {
        return None;
    }
    let mut end = i;
    for k in 0..4 {
        end += number(s, end, 1, 3)?;
        if k < 3 {
            if ch(s, end) != '.' {
                return None;
            }
            end += 1;
        }
    }
    let glued = word(ch(s, end)) || ch(s, end) == '.' && dig(s, end + 1);
    (!glued && is_ipv4(s.get(i..end)?)).then_some(end - i)
}

/// Whether `s` is exactly one IPv6 address: eight groups, or fewer with one `::`, optionally
/// ending in a dotted IPv4 address.
fn is_ipv6(s: &[char]) -> bool {
    let (mut groups, mut compressed, mut pos) = (0, false, 0);
    if s.starts_with(&[':', ':']) {
        compressed = true;
        pos = 2;
    }
    while pos < s.len() {
        let end = s.get(pos..).and_then(|r| r.iter().position(|c| *c == ':')).map_or(s.len(), |q| pos + q);
        let Some(seg) = s.get(pos..end) else { return false };
        if seg.contains(&'.') {
            if end != s.len() || !is_ipv4(seg) {
                return false;
            }
            groups += 2;
        } else if (1..=4).contains(&seg.len()) && seg.iter().all(char::is_ascii_hexdigit) {
            groups += 1;
        } else {
            return false;
        }
        pos = end;
        if pos == s.len() {
            break;
        }
        pos += 1;
        if ch(s, pos) == ':' {
            if compressed {
                return false;
            }
            compressed = true;
            pos += 1;
        } else if pos == s.len() {
            return false;
        }
    }
    // At least two groups (or loopback-style `::1`) and a decimal digit, so `a::b` in code or
    // prose is not an address.
    s.iter().any(char::is_ascii_digit) && (groups >= 2 || s.starts_with(&[':', ':']) && groups == 1) && (compressed && groups <= 7 || groups == 8)
}

fn ipv6(s: &[char], i: usize) -> Option<usize> {
    let c = ch(s, i);
    if !(c.is_ascii_hexdigit() || c == ':' && ch(s, i + 1) == ':') || word(prev(s, i)) || prev(s, i) == ':' {
        return None;
    }
    let mut end = i;
    while end - i < 45 && (ch(s, end).is_ascii_hexdigit() || matches!(ch(s, end), ':' | '.')) {
        end += 1;
    }
    // Sentence punctuation after the address is not part of it.
    while end > i + 1 && (ch(s, end - 1) == '.' || ch(s, end - 1) == ':' && ch(s, end - 2) != ':') {
        end -= 1;
    }
    (!word(ch(s, end)) && ch(s, end) != ':' && is_ipv6(s.get(i..end)?)).then_some(end - i)
}

// --- IBAN --------------------------------------------------------------------------------------

/// Total IBAN length per country (ISO 13616 registry).
const IBAN_LENGTHS: [(&str, usize); 78] = [
    ("AD", 24),
    ("AE", 23),
    ("AL", 28),
    ("AT", 20),
    ("AZ", 28),
    ("BA", 20),
    ("BE", 16),
    ("BG", 22),
    ("BH", 22),
    ("BR", 29),
    ("BY", 28),
    ("CH", 21),
    ("CR", 22),
    ("CY", 28),
    ("CZ", 24),
    ("DE", 22),
    ("DK", 18),
    ("DO", 28),
    ("EE", 20),
    ("EG", 29),
    ("ES", 24),
    ("FI", 18),
    ("FO", 18),
    ("FR", 27),
    ("GB", 22),
    ("GE", 22),
    ("GI", 23),
    ("GL", 18),
    ("GR", 27),
    ("GT", 28),
    ("HR", 21),
    ("HU", 28),
    ("IE", 22),
    ("IL", 23),
    ("IQ", 23),
    ("IS", 26),
    ("IT", 27),
    ("JO", 30),
    ("KW", 30),
    ("KZ", 20),
    ("LB", 28),
    ("LC", 32),
    ("LI", 21),
    ("LT", 20),
    ("LU", 20),
    ("LV", 21),
    ("LY", 25),
    ("MC", 27),
    ("MD", 24),
    ("ME", 22),
    ("MK", 19),
    ("MR", 27),
    ("MT", 31),
    ("MU", 30),
    ("NL", 18),
    ("NO", 15),
    ("PK", 24),
    ("PL", 28),
    ("PS", 29),
    ("PT", 25),
    ("QA", 29),
    ("RO", 24),
    ("RS", 22),
    ("SA", 24),
    ("SC", 31),
    ("SE", 24),
    ("SI", 19),
    ("SK", 24),
    ("SM", 27),
    ("ST", 25),
    ("SV", 28),
    ("TL", 23),
    ("TN", 24),
    ("TR", 26),
    ("UA", 29),
    ("VA", 22),
    ("VG", 24),
    ("XK", 20),
];

/// ISO 7064 mod 97-10: the rearranged IBAN, letters as 10-35, must leave remainder 1.
fn iban_checksum(iban: &[char]) -> bool {
    let mut rem = 0u32;
    for c in iban.iter().skip(4).chain(iban.iter().take(4)) {
        rem = match c.to_digit(36) {
            Some(d) if d < 10 => (rem * 10 + d) % 97,
            Some(d) => (rem * 100 + d) % 97,
            None => return false,
        };
    }
    rem == 1
}

fn iban(s: &[char], i: usize) -> Option<usize> {
    let (a, b) = (ch(s, i).to_ascii_uppercase(), ch(s, i + 1).to_ascii_uppercase());
    if !a.is_ascii_uppercase() || !b.is_ascii_uppercase() || !dig(s, i + 2) || !dig(s, i + 3) {
        return None;
    }
    let (_, len) = *IBAN_LENGTHS.iter().find(|(c, _)| c.chars().eq([a, b]))?;
    // The checksum makes a stray match vanishingly unlikely, so only the end needs a boundary.
    // Printed IBANs are usually grouped in fours by single spaces.
    let (mut j, mut iban) = (i, Vec::with_capacity(len));
    while iban.len() < len {
        let c = ch(s, j);
        if c.is_ascii_alphanumeric() {
            iban.push(c.to_ascii_uppercase());
        } else if !(c == ' ' && !iban.is_empty() && ch(s, j + 1).is_ascii_alphanumeric()) {
            return None;
        }
        j += 1;
    }
    (!word(ch(s, j)) && iban_checksum(&iban)).then_some(j - i)
}

// --- Postal codes ------------------------------------------------------------------------------

fn uk_postcode(s: &[char], i: usize) -> Option<usize> {
    if !ch(s, i).is_ascii_uppercase() || word(prev(s, i)) {
        return None;
    }
    let upper = |k: usize| ch(s, k).is_ascii_uppercase();
    let inward = |k: usize| dig(s, k) && "ABDEFGHJLNPQRSTUWXYZ".contains(ch(s, k + 1)) && "ABDEFGHJLNPQRSTUWXYZ".contains(ch(s, k + 2));
    // The one special code.
    if s.get(i..i + 7).is_some_and(|r| r.iter().collect::<String>() == "GIR 0AA") {
        return (!word(ch(s, i + 7))).then_some(7);
    }
    // Area: one or two letters (not Q, V, X first; not I, J, Z second), then the district.
    let letters = if upper(i + 1) { 2 } else { 1 };
    if "QVX".contains(ch(s, i)) || letters == 2 && "IJZ".contains(ch(s, i + 1)) {
        return None;
    }
    let d = i + letters;
    if !dig(s, d) {
        return None;
    }
    // District: a digit, optionally followed by another digit or a letter (A9A, AA9A, A99, AA99).
    for extra in [true, false] {
        let mut p = d + 1;
        if extra {
            if !(dig(s, p) || upper(p)) {
                continue;
            }
            p += 1;
        }
        let q = if ch(s, p) == ' ' { p + 1 } else { p };
        if inward(q) && !word(ch(s, q + 3)) {
            return Some(q + 3 - i);
        }
    }
    None
}

fn us_zip(s: &[char], i: usize) -> Option<usize> {
    let p = prev(s, i);
    if count_digits(s, i) != 5
        || word(p)
        || matches!(p, '$' | '#' | '-' | '/' | '.' | '+' | '€' | '£')
        || p == ',' && i.checked_sub(2).is_some_and(|k| dig(s, k))
    {
        return None;
    }
    let zip4 = ch(s, i + 5) == '-' && count_digits(s, i + 6) == 4;
    let end = if zip4 { i + 10 } else { i + 5 };
    let n = ch(s, end);
    let glued = word(n) || matches!(n, '%' | '/') || matches!(n, '-' | '.' | ',') && dig(s, end + 1);
    (!glued && value(s, i, 5) != 0).then_some(end - i)
}

// --- Entry points ------------------------------------------------------------------------------

fn matcher(pattern: Pattern) -> fn(&[char], usize) -> Option<usize> {
    match pattern {
        Pattern::Phone => phone,
        Pattern::Email => email,
        Pattern::CreditCard => credit_card,
        Pattern::Ssn => ssn,
        Pattern::Date => date,
        Pattern::PhoneIntl => phone_intl,
        Pattern::Ipv4 => ipv4,
        Pattern::Ipv6 => ipv6,
        Pattern::Iban => iban,
        Pattern::UkPostcode => uk_postcode,
        Pattern::UsZip => us_zip,
        Pattern::DateIso => date_iso,
        Pattern::DateUs => date_us,
    }
}

/// Ranges of `pattern` in already-normalised text, indices into that text.
fn scan(pattern: Pattern, s: &[char]) -> Vec<Range<usize>> {
    let f = matcher(pattern);
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        match f(s, i) {
            Some(n) if n > 0 => {
                out.push(i..i + n);
                i += n;
            }
            _ => i += 1,
        }
    }
    out
}

/// Character ranges of `text` matching any of `patterns`, as `(pattern, range)` in text order per
/// pattern. Ranges index `text` itself, covering characters the matcher looked through (soft
/// hyphens, zero-width characters, line breaks) so a mark built from them covers the whole match.
/// Fails, without searching, when `text` is longer than [`MAX_INPUT_CHARS`].
pub fn try_find_many(patterns: &[Pattern], text: &[char]) -> Result<Vec<(Pattern, Range<usize>)>, PatternError> {
    if text.len() > MAX_INPUT_CHARS {
        return Err(PatternError::InputTooLarge { len: text.len(), max: MAX_INPUT_CHARS });
    }
    let norm = Normalized::new(text);
    let mut out = Vec::new();
    for &p in patterns {
        out.extend(scan(p, &norm.chars).iter().filter_map(|r| norm.to_original(r)).map(|r| (p, r)));
    }
    Ok(out)
}

/// Character ranges of `text` matching `pattern`; see [`try_find_many`].
pub fn try_find(pattern: Pattern, text: &[char]) -> Result<Vec<Range<usize>>, PatternError> {
    Ok(try_find_many(&[pattern], text)?.into_iter().map(|(_, r)| r).collect())
}

/// Character ranges of `text` matching `pattern`. Text over [`MAX_INPUT_CHARS`] cannot be
/// searched, so the whole of it is reported as one match (over-marking, never a missed match);
/// use [`try_find`] to get the error instead.
pub fn find(pattern: Pattern, text: &[char]) -> Vec<Range<usize>> {
    try_find(pattern, text).unwrap_or_else(|_| std::iter::once(0..text.len()).collect())
}
