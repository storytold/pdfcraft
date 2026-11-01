//! Caller-supplied regular expressions for Search & Redact (`redact_mark regex`).
//!
//! The expression is untrusted input. It runs on the `regex` crate, which matches in time
//! linear in the text (no backtracking, no look-around or back-references), and is compiled
//! under hard limits on source length, nesting and compiled size, so a hostile expression is
//! refused with an error instead of exhausting time or memory. An optional [`Validator`]
//! keeps a match only when its checksum holds.

use std::ops::Range;

use regex::RegexBuilder;
use thiserror::Error;

/// Longest expression accepted, in bytes.
pub const MAX_REGEX_LEN: usize = 1024;
/// Most matches accepted for one page; one more makes [`UserRegex::find`] fail, so a page is never
/// marked only in part.
pub const MAX_MATCHES_PER_TEXT: usize = 10_000;
/// Cap on the compiled program and on the lazy-DFA cache, in bytes.
const SIZE_LIMIT: usize = 1 << 20;
/// Cap on parenthesis nesting.
const NEST_LIMIT: u32 = 32;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegexError {
    #[error("regex is empty")]
    Empty,
    #[error("regex is {0} bytes long; at most {MAX_REGEX_LEN} are allowed")]
    TooLong(usize),
    #[error("invalid or too complex regex: {0}")]
    Invalid(String),
    #[error("regex matches the empty string, so it would mark nothing useful; require at least one character")]
    MatchesEmpty,
    #[error("regex has more than {MAX_MATCHES_PER_TEXT} matches on one page; narrow it so that every match can be marked")]
    TooManyMatches,
}

/// Checksum a match must pass to be kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validator {
    /// Luhn (ISO/IEC 7812): digits only, spaces and hyphens ignored.
    Luhn,
    /// ISO 7064 MOD 97-10: letters count as 10..35 and the number mod 97 must be 1. Letters and
    /// digits only, spaces and hyphens ignored.
    Mod97,
    /// IBAN check (ISO 13616): the first four characters move to the end, then MOD 97-10.
    Iban,
}

/// Ids accepted for `validate`.
pub const VALIDATORS: [Validator; 3] = [Validator::Luhn, Validator::Mod97, Validator::Iban];

impl Validator {
    pub fn id(self) -> &'static str {
        match self {
            Validator::Luhn => "luhn",
            Validator::Mod97 => "mod97",
            Validator::Iban => "iban",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        VALIDATORS.into_iter().find(|v| v.id() == id)
    }

    /// Whether `s` passes the checksum.
    pub fn check(self, s: &str) -> bool {
        match self {
            Validator::Luhn => luhn(s),
            Validator::Mod97 => mod97(s.chars()),
            Validator::Iban => {
                let kept: Vec<char> = s.chars().filter(|c| *c != ' ' && *c != '-').collect();
                let (head, tail) = kept.split_at(kept.len().min(4));
                mod97(tail.iter().chain(head).copied())
            }
        }
    }
}

fn luhn(s: &str) -> bool {
    let (mut sum, mut digits, mut double) = (0u32, 0usize, false);
    for c in s.chars().rev() {
        if c == ' ' || c == '-' {
            continue;
        }
        let Some(d) = c.to_digit(10).filter(|_| c.is_ascii_digit()) else { return false };
        let d = if double { if d * 2 > 9 { d * 2 - 9 } else { d * 2 } } else { d };
        sum = sum.saturating_add(d);
        digits += 1;
        double = !double;
    }
    digits >= 2 && sum % 10 == 0
}

fn mod97(s: impl Iterator<Item = char>) -> bool {
    let (mut rem, mut n) = (0u32, 0usize);
    for c in s {
        if c == ' ' || c == '-' {
            continue;
        }
        if !c.is_ascii_alphanumeric() {
            return false;
        }
        // Digits stay one decimal digit, letters become two (A = 10 .. Z = 35).
        let v = c.to_digit(36).unwrap_or(0);
        rem = if v >= 10 { (rem * 100 + v) % 97 } else { (rem * 10 + v) % 97 };
        n += 1;
    }
    n >= 2 && rem == 1
}

/// Whether `re` can match the empty string, which `is_match("")` alone misses for assertions such
/// as `\b`. Tries a few short texts with letters, digits, space, newline and a non-ASCII letter.
fn can_match_empty(re: &regex::Regex) -> bool {
    ["", " ", "a", "1", "a a", "a\n", "\n", "é", "aa 11"].iter().any(|p| re.find_iter(p).any(|m| m.is_empty()))
}

/// A compiled caller expression with an optional checksum.
#[derive(Debug, Clone)]
pub struct UserRegex {
    re: regex::Regex,
    validator: Option<Validator>,
}

impl UserRegex {
    pub fn new(source: &str, validator: Option<Validator>) -> Result<Self, RegexError> {
        if source.is_empty() {
            return Err(RegexError::Empty);
        }
        if source.len() > MAX_REGEX_LEN {
            return Err(RegexError::TooLong(source.len()));
        }
        let re = RegexBuilder::new(source)
            .size_limit(SIZE_LIMIT)
            .dfa_size_limit(SIZE_LIMIT)
            .nest_limit(NEST_LIMIT)
            .build()
            .map_err(|e| RegexError::Invalid(e.to_string()))?;
        if can_match_empty(&re) {
            return Err(RegexError::MatchesEmpty);
        }
        Ok(Self { re, validator })
    }

    /// Character ranges of `text` the expression matches (non-overlapping, left to right).
    ///
    /// Fails with [`RegexError::TooManyMatches`] when more than [`MAX_MATCHES_PER_TEXT`] matches
    /// are kept, instead of returning a partial list the caller would take for complete.
    pub fn find(&self, text: &[char]) -> Result<Vec<Range<usize>>, RegexError> {
        let s: String = text.iter().collect();
        // Char index of every byte offset that starts a char (and of the end).
        let mut byte_to_char = vec![usize::MAX; s.len().saturating_add(1)];
        for (ci, (bi, _)) in s.char_indices().enumerate() {
            if let Some(slot) = byte_to_char.get_mut(bi) {
                *slot = ci;
            }
        }
        if let Some(slot) = byte_to_char.get_mut(s.len()) {
            *slot = text.len();
        }
        let mut out = Vec::new();
        for m in self.re.find_iter(&s) {
            if m.is_empty() || self.validator.is_some_and(|v| !v.check(m.as_str())) {
                continue;
            }
            if let (Some(&a), Some(&b)) = (byte_to_char.get(m.start()), byte_to_char.get(m.end()))
                && a < b
                && b != usize::MAX
            {
                if out.len() >= MAX_MATCHES_PER_TEXT {
                    return Err(RegexError::TooManyMatches);
                }
                out.push(a..b);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn finds_matches_as_char_ranges_with_multibyte_text() {
        let r = UserRegex::new(r"\d{2}( \d{2}){2}", None).unwrap();
        let t = chars("Ré 85 05 78 et 91 12 34 fin");
        let found: Vec<String> = r.find(&t).unwrap().into_iter().map(|g| t[g].iter().collect()).collect();
        assert_eq!(found, ["85 05 78", "91 12 34"]);
    }

    #[test]
    fn validators_filter_matches() {
        let t = chars("ok 4539 1488 0343 6467 bad 4539 1488 0343 6468");
        let r = UserRegex::new(r"\d{4}( \d{4}){3}", Some(Validator::Luhn)).unwrap();
        assert_eq!(r.find(&t).unwrap().len(), 1);
        let t = chars("GB82 WEST 1234 5698 7654 32 and GB83 WEST 1234 5698 7654 32");
        let r = UserRegex::new(r"GB\d\d WEST( \d{4}){3} \d\d", Some(Validator::Iban)).unwrap();
        assert_eq!(r.find(&t).unwrap().len(), 1);
        assert!(Validator::Mod97.check("98") && !Validator::Mod97.check("99"));
        assert!(!Validator::Luhn.check("12a4") && !Validator::Mod97.check("é1") && !Validator::Luhn.check("0"));
    }

    #[test]
    fn hostile_expressions_are_refused_or_stay_linear() {
        assert_eq!(UserRegex::new("", None).unwrap_err(), RegexError::Empty);
        assert!(matches!(UserRegex::new(&"a".repeat(MAX_REGEX_LEN + 1), None), Err(RegexError::TooLong(_))));
        assert!(matches!(UserRegex::new("(unclosed", None), Err(RegexError::Invalid(_))));
        assert!(matches!(UserRegex::new(r"(a)\1", None), Err(RegexError::Invalid(_))), "no back-references");
        for empty in ["a*", r"\b", "(?m)^", r"\B", "(?m)$", r"\d*|x"] {
            assert_eq!(UserRegex::new(empty, None).unwrap_err(), RegexError::MatchesEmpty, "{empty}");
        }
        assert!(UserRegex::new(r"\bfoo\b", None).is_ok());
        assert!(matches!(UserRegex::new(r"\p{L}{1000}{1000}", None), Err(RegexError::Invalid(_))), "compiled size is capped");
        assert!(matches!(UserRegex::new(&format!("{}a{}", "(".repeat(100), ")".repeat(100)), None), Err(RegexError::Invalid(_))));
        // Classic catastrophic pattern finishes at once on a long non-matching input.
        let r = UserRegex::new(r"(a+)+$", None).unwrap();
        let mut t = chars(&"a".repeat(50_000));
        t.push('!');
        assert!(r.find(&t).unwrap().is_empty());
    }

    #[test]
    fn more_than_the_cap_of_matches_is_an_error_not_a_partial_list() {
        let r = UserRegex::new("x", None).unwrap();
        let at_cap = chars(&"x ".repeat(MAX_MATCHES_PER_TEXT));
        assert_eq!(r.find(&at_cap).unwrap().len(), MAX_MATCHES_PER_TEXT);
        let over = chars(&"x ".repeat(MAX_MATCHES_PER_TEXT + 1));
        assert_eq!(r.find(&over).unwrap_err(), RegexError::TooManyMatches);
    }
}
