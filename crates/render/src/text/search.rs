//! Literal search in normalized page text, retaining the original glyph owners.
//!
//! The page text is read as characters, each owned by the glyph it came from: words and lines are
//! separated by one space, and a Latin compatibility ligature (U+FB00–U+FB06) reads as its letters
//! in the page and in the query, so "office" finds "o\u{FB03}ce". A word divided by a hyphen at
//! the end of a line is also found whole: where a line ends in a hyphen between two letters, the
//! text is searched a second time without the hyphen and the line break ("example" finds
//! "exam-" + "ple") and a third time with the hyphen but without the break ("exam-ple"). A hit in
//! any reading counts; overlapping hits keep the one that starts first (the longer on a tie).
use super::{PageText, fold_case};
use std::ops::Range;

/// How a line ending in a hyphen between two letters is read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hyphen {
    /// As written: the hyphen, then a line break (a space).
    Written,
    /// One word: neither the hyphen nor the break.
    Joined,
    /// A hyphenated compound: the hyphen, but no break.
    Kept,
}

/// The letters of a Latin compatibility ligature (U+FB00–U+FB06), their compatibility decomposition.
fn ligature(c: char) -> Option<&'static str> {
    match c {
        '\u{FB00}' => Some("ff"),
        '\u{FB01}' => Some("fi"),
        '\u{FB02}' => Some("fl"),
        '\u{FB03}' => Some("ffi"),
        '\u{FB04}' => Some("ffl"),
        '\u{FB05}' | '\u{FB06}' => Some("st"),
        _ => None,
    }
}

/// `s` with each Latin compatibility ligature written as its letters. Nothing else is normalised.
fn letters(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match ligature(c) {
            Some(l) => out.push_str(l),
            None => out.push(c),
        }
    }
    out
}

/// A hyphen that can divide a word at a line end: HYPHEN-MINUS, SOFT HYPHEN (ISO 32000-2
/// §14.8.2.3) and HYPHEN.
fn is_hyphen(c: char) -> bool {
    matches!(c, '-' | '\u{AD}' | '\u{2010}')
}

/// Whether a line break separates glyph `i` from the glyph before it.
fn breaks_before(page: &PageText, i: usize) -> bool {
    i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b)
}

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let fold = |s: &str| if sensitive { letters(s) } else { fold_case(&letters(s)) };
    let needle: Vec<char> = fold(query).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let texts: Vec<String> = page.glyphs.iter().map(|g| fold(&g.text)).collect();
    let divided: Vec<bool> = (0..texts.len()).map(|i| divides(page, &texts, i)).collect();
    let readings: &[Hyphen] = if divided.contains(&true) { &[Hyphen::Written, Hyphen::Joined, Hyphen::Kept] } else { &[Hyphen::Written] };
    // KMP reuses a matched prefix instead of comparing a long query again at every position.
    // The table is bounded by the page's already materialized character count.
    let mut prefix = vec![0; needle.len()];
    let mut matched = 0;
    for (i, &c) in needle.iter().enumerate().skip(1) {
        matched = advance(&needle, &prefix, matched, c);
        if let Some(slot) = prefix.get_mut(i) {
            *slot = matched;
        }
    }
    let mut hits = Vec::new();
    for &reading in readings {
        hits.extend(search(&characters(page, &texts, &divided, reading), &needle, &prefix, whole_words, limit));
    }
    if readings.len() == 1 {
        return hits;
    }
    // Each reading's hits are in order and apart; across readings, the earliest start wins.
    hits.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut kept: Vec<Range<usize>> = Vec::new();
    for hit in hits {
        if kept.last().is_none_or(|last| hit.start >= last.end) {
            kept.push(hit);
            if kept.len() == limit {
                break;
            }
        }
    }
    kept
}

/// Whether glyph `i` ends its line with a hyphen that follows a letter of its word, and the next
/// line starts with a letter. `texts` are the glyphs' folded texts.
fn divides(page: &PageText, texts: &[String], i: usize) -> bool {
    let Some(text) = texts.get(i) else { return false };
    let mut back = text.chars().rev();
    if !back.next().is_some_and(is_hyphen) {
        return false;
    }
    let before = match back.next() {
        Some(c) => Some(c),
        // A hyphen glyph of its own: the letter is the end of the glyph before, on its line with no word gap.
        None if !page.space_before.get(i).copied().unwrap_or(false) && !breaks_before(page, i) => {
            i.checked_sub(1).and_then(|p| texts.get(p)).and_then(|t| t.chars().next_back())
        }
        None => None,
    };
    let Some(next) = i.checked_add(1) else { return false };
    before.is_some_and(char::is_alphabetic)
        && breaks_before(page, next)
        && texts.get(next).and_then(|t| t.chars().next()).is_some_and(char::is_alphabetic)
}

/// The page's characters in one `reading`, each with the glyph it came from.
fn characters(page: &PageText, texts: &[String], divided: &[bool], reading: Hyphen) -> Vec<(char, usize)> {
    let mut chars: Vec<(char, usize)> = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        let after_divide = reading != Hyphen::Written && i.checked_sub(1).and_then(|p| divided.get(p)).copied().unwrap_or(false);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || (breaks_before(page, i) && !after_divide);
        if i > 0 && gap && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', i));
        }
        // The divided glyph's last character is its hyphen.
        let last =
            if reading == Hyphen::Joined && divided.get(i).copied().unwrap_or(false) { text.chars().count().saturating_sub(1) } else { usize::MAX };
        for c in text.chars().take(last) {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
                chars.push((c, i));
            }
        }
    }
    chars
}

/// Non-overlapping hits of `needle` (with its KMP `prefix` table) in `chars`, as glyph ranges.
fn search(chars: &[(char, usize)], needle: &[char], prefix: &[usize], whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if needle.len() > chars.len() {
        return Vec::new();
    }
    let mut matched = 0;
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| c.0.is_alphanumeric());
    let mut hits = Vec::new();
    for (i, &(c, owner)) in chars.iter().enumerate() {
        matched = advance(needle, prefix, matched, c);
        if matched != needle.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.saturating_sub(needle.len());
        let bounded = !whole_words || (!word(start.checked_sub(1)) && !word(Some(end)));
        if bounded {
            if let Some((_, first)) = chars.get(start)
                && let Some(last) = owner.checked_add(1)
            {
                hits.push(*first..last);
                if hits.len() == limit {
                    break;
                }
            }
            // Only accepted hits consume their full span: literal search is non-overlapping.
            matched = 0;
        } else {
            // A word-boundary rejection must still allow an overlapping later candidate.
            matched = prefix.last().copied().unwrap_or_default();
        }
    }
    hits
}

/// Extend a prefix, falling back through strictly shorter proper prefixes on a mismatch.
fn advance(needle: &[char], prefix: &[usize], mut matched: usize, c: char) -> usize {
    while matched > 0 && needle.get(matched) != Some(&c) {
        matched = prefix.get(matched - 1).copied().unwrap_or_default();
    }
    if needle.get(matched) == Some(&c) { matched.saturating_add(1) } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::letters;

    #[test]
    fn ligatures_read_as_their_letters() {
        assert_eq!(letters("o\u{FB03}ce \u{FB00}\u{FB01}\u{FB02}\u{FB04}\u{FB05}\u{FB06}"), "office fffiflfflstst");
        assert_eq!(letters("\u{FB13}é\u{FF21}\u{2460}"), "\u{FB13}é\u{FF21}\u{2460}", "nothing else is normalised");
    }
}
