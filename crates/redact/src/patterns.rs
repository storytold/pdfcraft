//! Search & Redact patterns (Acrobat's Find Text ▸ Patterns): phone numbers, email addresses,
//! credit card numbers, US Social Security numbers and dates. Each matcher works on a page's
//! text as characters and returns character ranges; callers map them to glyphs and areas.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    Phone,
    Email,
    CreditCard,
    Ssn,
    Date,
}

pub const PATTERNS: [Pattern; 5] = [Pattern::Phone, Pattern::Email, Pattern::CreditCard, Pattern::Ssn, Pattern::Date];

impl Pattern {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Pattern::Phone => "Phone Numbers",
            Pattern::Email => "Email Addresses",
            Pattern::CreditCard => "Credit Cards",
            Pattern::Ssn => "Social Security Numbers",
            Pattern::Date => "Dates",
        }
    }

    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Pattern::Phone => "phone",
            Pattern::Email => "email",
            Pattern::CreditCard => "credit-card",
            Pattern::Ssn => "ssn",
            Pattern::Date => "date",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        PATTERNS.into_iter().find(|p| p.id() == id)
    }
}

fn digit(c: Option<&char>) -> bool {
    c.is_some_and(char::is_ascii_digit)
}

fn word(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric())
}

/// Digits and the separators allowed between them, from `start`: (end, digit count).
fn digit_run(s: &[char], start: usize, seps: &[char], max_len: usize) -> (usize, usize) {
    let (mut i, mut n) = (start, 0);
    while i < s.len() && i - start < max_len {
        let c = s[i];
        if c.is_ascii_digit() {
            n += 1;
        } else if !(seps.contains(&c) && digit(s.get(i + 1)) || (c == '(' || c == ')') && seps.contains(&c)) {
            break;
        }
        i += 1;
    }
    // Never end on a separator.
    while i > start && !s[i - 1].is_ascii_digit() {
        i -= 1;
    }
    (i, n)
}

fn phone(s: &[char], i: usize) -> Option<usize> {
    let start_ok = s[i] == '+' && digit(s.get(i + 1)) || s[i] == '(' && digit(s.get(i + 1)) || s[i].is_ascii_digit();
    if !start_ok || word(i.checked_sub(1).and_then(|p| s.get(p))) || s.get(i.wrapping_sub(1)) == Some(&'+') {
        return None;
    }
    let first = if s[i] == '+' { i + 1 } else { i };
    let (end, n) = digit_run(s, first, &[' ', '-', '.', '(', ')'], 20);
    let seps = s[first..end].iter().filter(|c| !c.is_ascii_digit()).count();
    // 10 digits (11 with a country code), grouped by at least one separator.
    if !(10..=11).contains(&n) || seps == 0 || digit(s.get(end)) {
        return None;
    }
    Some(end - i)
}

fn email(s: &[char], i: usize) -> Option<usize> {
    let local = |c: &char| c.is_ascii_alphanumeric() || "._%+-".contains(*c);
    if !s[i].is_ascii_alphanumeric() || i.checked_sub(1).and_then(|p| s.get(p)).is_some_and(local) {
        return None;
    }
    let mut j = i;
    while j < s.len() && local(&s[j]) {
        j += 1;
    }
    if s.get(j) != Some(&'@') {
        return None;
    }
    let d0 = j + 1;
    let mut k = d0;
    while k < s.len() && (s[k].is_ascii_alphanumeric() || s[k] == '-' || s[k] == '.' && s.get(k + 1).is_some_and(char::is_ascii_alphanumeric)) {
        k += 1;
    }
    let domain: String = s[d0..k].iter().collect();
    let tld = domain.rsplit('.').next().unwrap_or("");
    (domain.contains('.') && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())).then_some(k - i)
}

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

fn credit_card(s: &[char], i: usize) -> Option<usize> {
    if !s[i].is_ascii_digit() || digit(i.checked_sub(1).and_then(|p| s.get(p))) {
        return None;
    }
    let (end, n) = digit_run(s, i, &[' ', '-'], 23);
    if !(13..=19).contains(&n) || digit(s.get(end)) {
        return None;
    }
    let digits: Vec<u32> = s[i..end].iter().filter_map(|c| c.to_digit(10)).collect();
    luhn(&digits).then_some(end - i)
}

fn ssn(s: &[char], i: usize) -> Option<usize> {
    if digit(i.checked_sub(1).and_then(|p| s.get(p))) {
        return None;
    }
    let g = |from: usize, n: usize| (from..from + n).all(|k| digit(s.get(k)));
    let sep = |k: usize| matches!(s.get(k), Some('-' | ' '));
    if !(g(i, 3) && sep(i + 3) && g(i + 4, 2) && sep(i + 6) && g(i + 7, 4)) || digit(s.get(i + 11)) {
        return None;
    }
    let area: String = s[i..i + 3].iter().collect();
    (area != "000" && area != "666" && !area.starts_with('9')).then_some(11)
}

const MONTHS: [&str; 12] = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];

/// A month name or its three-letter abbreviation (optionally with a full stop) at `i`.
fn month(s: &[char], i: usize) -> Option<usize> {
    if word(i.checked_sub(1).and_then(|p| s.get(p))) {
        return None;
    }
    let rest: String = s[i..s.len().min(i + 10)].iter().collect::<String>().to_lowercase();
    for m in MONTHS {
        for cand in [m, &m[..3]] {
            if rest.starts_with(cand) && !rest[cand.len()..].starts_with(|c: char| c.is_alphabetic()) {
                let mut n = cand.chars().count();
                if cand.len() == 3 && s.get(i + n) == Some(&'.') {
                    n += 1;
                }
                return Some(n);
            }
        }
    }
    None
}

fn number(s: &[char], i: usize, min: usize, max: usize) -> Option<usize> {
    let n = s[i.min(s.len())..].iter().take(max + 1).take_while(|c| c.is_ascii_digit()).count();
    (n >= min && n <= max).then_some(n)
}

fn date(s: &[char], i: usize) -> Option<usize> {
    if word(i.checked_sub(1).and_then(|p| s.get(p))) {
        return None;
    }
    let spaces = |k: usize| s[k.min(s.len())..].iter().take_while(|c| **c == ' ').count();
    // Numeric: m/d/yy(yy), m-d-yyyy, d.m.yyyy, yyyy-mm-dd.
    if let Some(month_len) = number(s, i, 1, 4) {
        let sep = s.get(i + month_len).copied();
        if matches!(sep, Some('/' | '-' | '.'))
            && let Some(sep_len) = number(s, i + month_len + 1, 1, 2)
            && s.get(i + month_len + 1 + sep_len).copied() == sep
            && let Some(year_len) = number(s, i + month_len + sep_len + 2, if month_len == 4 { 1 } else { 2 }, if month_len == 4 { 2 } else { 4 })
            && (month_len <= 2 || month_len == 4)
            && !digit(s.get(i + month_len + sep_len + 2 + year_len))
        {
            return Some(month_len + sep_len + year_len + 2);
        }
        // "5 January 2024".
        if month_len <= 2
            && let after = i + month_len + spaces(i + month_len)
            && after > i + month_len
            && let Some(name_len) = month(s, after)
        {
            let after_name = after + name_len + spaces(after + name_len);
            if let Some(year) = number(s, after_name, 4, 4) {
                return Some(after_name + year - i);
            }
        }
        return None;
    }
    // "January 5, 2024" / "Jan. 5 2024".
    let name_len = month(s, i)?;
    let day_at = i + name_len + spaces(i + name_len);
    let day = number(s, day_at, 1, 2)?;
    let mut year_at = day_at + day;
    if s.get(year_at) == Some(&',') {
        year_at += 1;
    }
    year_at += spaces(year_at);
    let year = number(s, year_at, 4, 4)?;
    Some(year_at + year - i)
}

/// Character ranges of `text` matching `pattern`.
#[must_use]
pub fn find(pattern: Pattern, text: &[char]) -> Vec<Range<usize>> {
    let f: fn(&[char], usize) -> Option<usize> = match pattern {
        Pattern::Phone => phone,
        Pattern::Email => email,
        Pattern::CreditCard => credit_card,
        Pattern::Ssn => ssn,
        Pattern::Date => date,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        match f(text, i) {
            Some(n) if n > 0 => {
                out.push(i..i + n);
                i += n;
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(p: Pattern, s: &str) -> Vec<String> {
        let c: Vec<char> = s.chars().collect();
        find(p, &c).into_iter().map(|r| c[r].iter().collect()).collect()
    }

    #[test]
    fn phone_numbers() {
        assert_eq!(
            found(Pattern::Phone, "Call (555) 123-4567 or 555.987.6543, +1 555 222 3333; not 12345 or 5551234567890"),
            ["(555) 123-4567", "555.987.6543", "+1 555 222 3333"]
        );
    }

    #[test]
    fn emails() {
        assert_eq!(
            found(Pattern::Email, "Write to ada.lovelace+pdf@example.co.uk, or bob@host (no TLD) or x@y.z."),
            ["ada.lovelace+pdf@example.co.uk"]
        );
    }

    #[test]
    fn credit_cards_need_a_valid_checksum() {
        assert_eq!(
            found(Pattern::CreditCard, "Visa 4111 1111 1111 1111, bad 4111 1111 1111 1112, amex 3782-822463-10005"),
            ["4111 1111 1111 1111", "3782-822463-10005"]
        );
    }

    #[test]
    fn social_security_numbers() {
        assert_eq!(
            found(Pattern::Ssn, "SSN 123-45-6789 and 123 45 6789; invalid 000-12-3456, 666-12-3456, 912-12-3456, 1234-56-7890"),
            ["123-45-6789", "123 45 6789"]
        );
    }

    #[test]
    fn dates() {
        assert_eq!(
            found(Pattern::Date, "Due 10/01/2026, 2026-10-01, 1.10.26, March 5, 2024, Jan. 7 2025 and 5 June 2023; not 10/2026 or Mayday 12"),
            ["10/01/2026", "2026-10-01", "1.10.26", "March 5, 2024", "Jan. 7 2025", "5 June 2023"]
        );
    }
}
