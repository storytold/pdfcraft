//! Spell checking of plain text against a word list.
//!
//! A [`Dictionary`] is a set of words read from a plain word list (one word per line, `#` starts a
//! comment line). PdfCraft ships no dictionary: the list comes from the caller, so any language
//! works and no word list's licence is taken on here. A user dictionary is more words added with
//! [`Dictionary::add_word`].
//!
//! Matching is case-aware the way people expect: a word listed in lower case is also right
//! capitalised or in capitals, a word listed with capitals of its own ("iPhone", "Paris") is right
//! only as written, except that a listed capitalised word is also right in capitals.
//! Text that is not prose is left alone: anything with a digit in it, web and e-mail addresses,
//! and single letters.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeSet;

/// Most words a dictionary holds; further words are refused and [`Dictionary::truncated`] says so.
pub const MAX_WORDS: usize = 2_000_000;
/// Longest word, in characters, that is stored, checked or given suggestions.
pub const MAX_WORD_CHARS: usize = 64;
/// Most suggestions returned for one word.
pub const MAX_SUGGESTIONS: usize = 5;
/// Most characters of one text that are checked; the rest is left unchecked.
pub const MAX_TEXT_CHARS: usize = 1_000_000;

/// A misspelled word and where it is, in characters (not bytes) from the start of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misspelling {
    pub word: String,
    pub start: usize,
    pub end: usize,
    pub suggestions: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Dictionary {
    /// Words listed in lower case.
    lower: BTreeSet<String>,
    /// Words listed with a capital, as written.
    cased: BTreeSet<String>,
    /// Every letter the words use, lower-cased: what suggestions insert and substitute.
    alphabet: BTreeSet<char>,
    truncated: bool,
}

impl Dictionary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a word list: one word per line, blank lines and lines starting with `#` ignored.
    /// Lines that are not a single word (spaces, digits) are skipped.
    pub fn from_word_list(text: &str) -> Self {
        let mut dictionary = Self::new();
        for line in text.lines() {
            let word = line.trim().trim_start_matches('\u{feff}');
            if !word.is_empty() && !word.starts_with('#') {
                dictionary.add_word(word);
            }
        }
        dictionary
    }

    /// Add one word (also a user's own). `false` when it is empty, too long, holds anything but
    /// letters and apostrophes, or the dictionary is full.
    pub fn add_word(&mut self, word: &str) -> bool {
        let word = normalise(word.trim());
        let count = word.chars().count();
        if count == 0 || count > MAX_WORD_CHARS || !word.chars().all(is_word_char) || !word.chars().any(char::is_alphabetic) {
            return false;
        }
        if self.len() >= MAX_WORDS {
            self.truncated = true;
            return false;
        }
        self.alphabet.extend(word.chars().flat_map(char::to_lowercase));
        if word == lowercase(&word) {
            self.lower.insert(word);
        } else {
            self.cased.insert(word);
        }
        true
    }

    pub fn len(&self) -> usize {
        self.lower.len() + self.cased.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Words were refused because the dictionary reached [`MAX_WORDS`].
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Is this one word spelled right? (A word, not a sentence: see [`Dictionary::check`].)
    pub fn knows(&self, word: &str) -> bool {
        let word = normalise(word);
        let lower = lowercase(&word);
        if word == lower {
            return self.lower.contains(&word);
        }
        if self.cased.contains(&word) {
            return true;
        }
        if is_capitalised(&word) {
            return self.lower.contains(&lower);
        }
        if is_all_capitals(&word) {
            return self.lower.contains(&lower) || self.cased.contains(&capitalise(&lower));
        }
        false
    }

    /// Every misspelled word in `text`, in order, with suggestions.
    ///
    /// `complete` is false when characters past [`MAX_TEXT_CHARS`] were not examined.
    pub fn check(&self, text: &str) -> CheckResult {
        let (words, complete) = words_of(text);
        let mut misspellings = Vec::new();
        // An empty dictionary would flag everything: nothing is known, so nothing is claimed.
        if !self.is_empty() {
            for (start, end, word) in words {
                if !self.knows(&word) {
                    let suggestions = self.suggest(&word);
                    misspellings.push(Misspelling { word, start, end, suggestions });
                }
            }
        }
        CheckResult { misspellings, complete }
    }

    /// Words one edit away from `word` (a letter dropped, added, changed or two swapped), in the
    /// case of `word`: alphabetical, as there is no frequency data to rank by.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let word = normalise(word);
        let chars: Vec<char> = lowercase(&word).chars().collect();
        if chars.is_empty() || chars.len() > MAX_WORD_CHARS {
            return Vec::new();
        }
        let mut candidates: BTreeSet<String> = BTreeSet::new();
        for i in 0..=chars.len() {
            let (head, tail) = chars.split_at(i);
            if let Some((_, rest)) = tail.split_first() {
                candidates.insert(head.iter().chain(rest).collect());
                if let [a, b, after @ ..] = tail
                    && a != b
                {
                    candidates.insert(head.iter().chain([b, a]).chain(after).collect());
                }
                for letter in &self.alphabet {
                    candidates.insert(head.iter().chain(std::iter::once(letter)).chain(rest).collect());
                }
            }
            for letter in &self.alphabet {
                candidates.insert(head.iter().chain(std::iter::once(letter)).chain(tail).collect());
            }
        }
        let shape = |s: &str| {
            if is_all_capitals(&word) {
                s.to_uppercase()
            } else if is_capitalised(&word) {
                capitalise(s)
            } else {
                s.to_string()
            }
        };
        let lower_word = lowercase(&word);
        candidates
            .iter()
            .filter(|c| **c != lower_word)
            .filter_map(|c| {
                if self.lower.contains(c) {
                    let shaped = shape(c);
                    return self.knows(&shaped).then_some(shaped);
                }
                // Cased entries ("Paris", "iPhone"): shape like lower-case ones when that form is
                // known (so "PARI" → "PARIS"), otherwise keep the stored spelling ("iPhone").
                self.cased.iter().find(|w| lowercase(w) == *c).map(|stored| {
                    let shaped = shape(c);
                    if self.knows(&shaped) { shaped } else { stored.clone() }
                })
            })
            .take(MAX_SUGGESTIONS)
            .collect()
    }
}

/// Misspellings found in a text, and whether the whole text was examined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub misspellings: Vec<Misspelling>,
    /// False when characters past [`MAX_TEXT_CHARS`] were skipped.
    pub complete: bool,
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || c == '\''
}

/// Typographic apostrophes are the plain one.
fn normalise(word: &str) -> String {
    word.replace(['\u{2019}', '\u{2018}', '\u{02bc}'], "'")
}

fn lowercase(word: &str) -> String {
    word.to_lowercase()
}

fn is_all_capitals(word: &str) -> bool {
    word.chars().filter(|c| c.is_alphabetic()).count() > 1 && word == word.to_uppercase()
}

fn is_capitalised(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(char::is_uppercase) && chars.as_str() == lowercase(chars.as_str())
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The words to check in `text` with their character range. Whitespace-separated pieces that are
/// not prose (a digit, `@`, `://`, `www.`) are skipped whole; elsewhere a word is a run of
/// letters, with apostrophes only between letters ("don't", not "'quoted'").
///
/// Stops after [`MAX_TEXT_CHARS`] characters. Each piece is consumed only up to the remaining
/// budget, plus [`MAX_WORD_CHARS`] of look-ahead (so a word that starts inside the budget and
/// would cross it is detected and left out), plus one character to see that the piece continues.
/// The non-prose test and lowercasing see only the budgeted prefix. Words that end inside the
/// budget keep their ranges. The second return value is false when any character past the limit
/// was not examined.
fn words_of(text: &str) -> (Vec<(usize, usize, String)>, bool) {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut complete = true;
    for piece in text.split_inclusive(char::is_whitespace) {
        if offset >= MAX_TEXT_CHARS {
            complete = false;
            break;
        }
        let remaining = MAX_TEXT_CHARS - offset;
        let scan_limit = remaining.saturating_add(MAX_WORD_CHARS);
        // Only the budgeted prefix plus look-ahead are kept; one further character detects that a
        // space-free piece continues, without allocating the rest of the run.
        let mut piece_chars = Vec::with_capacity(scan_limit.min(piece.len()));
        let mut chars = piece.chars();
        for _ in 0..scan_limit {
            match chars.next() {
                Some(c) => piece_chars.push(c),
                None => break,
            }
        }
        let piece_continues = chars.next().is_some();
        if piece_continues || piece_chars.len() > remaining {
            complete = false;
        }
        let budget_end = remaining.min(piece_chars.len());
        let scan: String = piece_chars.iter().collect();
        let budgeted: String = piece_chars[..budget_end].iter().collect();
        let trimmed = budgeted.trim_end();
        let lower = lowercase(trimmed);
        let prose = !trimmed.chars().any(|c| c.is_ascii_digit() || c == '@') && !lower.contains("://") && !lower.starts_with("www.");
        if prose {
            collect_words(&scan, offset, MAX_TEXT_CHARS, &mut out);
        }
        if piece_continues {
            // The rest of this piece (and later pieces) lie past what we examine.
            break;
        }
        offset = offset.saturating_add(piece_chars.len());
    }
    (out, complete)
}

fn collect_words(piece: &str, base: usize, limit: usize, out: &mut Vec<(usize, usize, String)>) {
    let chars: Vec<char> = piece.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let abs = base + i;
        if abs >= limit {
            break;
        }
        if !chars.get(i).is_some_and(|c| c.is_alphabetic()) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while let Some(&c) = chars.get(end) {
            let inner_apostrophe = matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '\u{02bc}')
                && chars.get(end + 1).is_some_and(|n| n.is_alphabetic());
            if c.is_alphabetic() || inner_apostrophe {
                end += 1;
            } else {
                break;
            }
        }
        let length = end - start;
        let abs_start = base + start;
        let abs_end = base + end;
        if length > 1 && length <= MAX_WORD_CHARS && abs_end <= limit {
            out.push((abs_start, abs_end, chars.iter().skip(start).take(length).collect()));
        }
        i = end.max(i + 1);
    }
}

#[cfg(test)]
mod tests;
