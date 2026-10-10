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
    pub fn label(self) -> &'static str {
        match self {
            Pattern::Phone => "Phone Numbers",
            Pattern::Email => "Email Addresses",
            Pattern::CreditCard => "Credit Cards",
            Pattern::Ssn => "Social Security Numbers",
            Pattern::Date => "Dates",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Pattern::Phone => "phone",
            Pattern::Email => "email",
            Pattern::CreditCard => "credit-card",
            Pattern::Ssn => "ssn",
            Pattern::Date => "date",
        }
    }

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
        } else if !(seps.contains(&c) && (digit(s.get(i + 1)) || s.get(i + 1) == Some(&'(')) || (c == '(' || c == ')') && seps.contains(&c)) {
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
    let plus = s[i] == '+';
    let first = if plus { i + 1 } else { i };
    let (end, n) = digit_run(s, first, &[' ', '-', '.', '(', ')'], 28);
    let run = s.get(first..end)?;
    let seps = run.iter().filter(|c| !c.is_ascii_digit()).count();
    // Always grouped by at least one separator, so plain numbers never match.
    if seps == 0 || digit(s.get(end)) {
        return None;
    }
    let ok = if plus {
        // E.164: a `+` country code and up to 15 digits in all ("+49 2151 123456").
        (8..=15).contains(&n)
    } else {
        // North American: 10 digits, 11 with a country code ("(555) 123-4567").
        let nanp = (10..=11).contains(&n);
        // National with a trunk 0 and 6–14 more digits ("0211 123456", "01 23 45 67 89").
        // The first group needs two digits so decimals ("0.1234567") don't match, and
        // day-first dates ("01.10.2026") are left to the date pattern.
        let mut digits = run.iter().skip_while(|c| !c.is_ascii_digit());
        let trunk_zero = digits.clone().next() == Some(&'0');
        let lead = digits.by_ref().take_while(|c| c.is_ascii_digit()).count();
        let trunk = trunk_zero && (7..=15).contains(&n) && lead >= 2 && date(s, i).is_none();
        nanp || trunk
    };
    ok.then_some(end - i)
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
    if let Some(a) = number(s, i, 1, 4) {
        let sep = s.get(i + a).copied();
        if matches!(sep, Some('/' | '-' | '.'))
            && let Some(b) = number(s, i + a + 1, 1, 2)
            && s.get(i + a + 1 + b).copied() == sep
            && let Some(c) = number(s, i + a + b + 2, if a == 4 { 1 } else { 2 }, if a == 4 { 2 } else { 4 })
            && (a <= 2 || a == 4)
            && !digit(s.get(i + a + b + 2 + c))
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
    if s.get(k2) == Some(&',') {
        k2 += 1;
    }
    k2 += spaces(k2);
    let y = number(s, k2, 4, 4)?;
    Some(k2 + y - i)
}

/// Character ranges of `text` matching `pattern`.
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

/// The most entries a word list keeps (Find Text ▸ Multiple words or phrases).
pub const MAX_WORDS: usize = 1000;
/// The most characters an entry keeps.
pub const MAX_WORD_CHARS: usize = 256;

/// A word list from text with one word or phrase per line: trimmed, without empty lines or
/// repeats, in order, at most [`MAX_WORDS`] entries of at most [`MAX_WORD_CHARS`] characters.
pub fn word_list(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .map(|l| l.trim().chars().take(MAX_WORD_CHARS).collect::<String>())
        .filter(|w| !w.is_empty() && seen.insert(w.clone()))
        .take(MAX_WORDS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_lists_are_trimmed_unique_and_bounded() {
        assert_eq!(word_list("  John Smith \n\n\tACME\r\nJohn Smith\n  \nő ű"), ["John Smith", "ACME", "ő ű"]);
        let long = "é".repeat(MAX_WORD_CHARS + 10);
        assert_eq!(word_list(&long)[0].chars().count(), MAX_WORD_CHARS);
        let many: String = (0..MAX_WORDS + 50).map(|i| format!("w{i}\n")).collect();
        let list = word_list(&many);
        assert_eq!((list.len(), list.last().map(String::as_str)), (MAX_WORDS, Some("w999")));
        assert!(word_list("").is_empty());
    }

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
        // #525: international numbers match whole; "+49" used to stay visible.
        assert_eq!(found(Pattern::Phone, "Tel. +49 2151 123456."), ["+49 2151 123456"]);
        assert_eq!(
            found(Pattern::Phone, "+44 20 7946 0018, +33 1 23 45 67 89, +49 (0) 2151 123456, (0211) 123456, 030 12345678, 01 23 45 67 89"),
            ["+44 20 7946 0018", "+33 1 23 45 67 89", "+49 (0) 2151 123456", "(0211) 123456", "030 12345678", "01 23 45 67 89"]
        );
        // Not phone numbers: day-first dates, decimals, unseparated, too short or too long.
        assert_eq!(
            found(
                Pattern::Phone,
                "on 01.10.2026 or 09-10-2026, ratio 0.1234567, id 0211123456, +4921511234567, +1 234 567, 0211 12, +49 1234 5678 9012 3456"
            ),
            Vec::<String>::new()
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
