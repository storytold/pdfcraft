//! Literal search in normalized page text, retaining the original glyph owners, and the same
//! search over a string (find and replace matches a paragraph's text with it).
use super::{PageText, fold_case};
use std::ops::Range;

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let needle = needle(query, sensitive);
    if needle.is_empty() {
        return Vec::new();
    }
    let mut chars: Vec<(char, usize)> = Vec::new();
    for (i, glyph) in page.glyphs.iter().enumerate() {
        let line_break = i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || line_break;
        if i > 0 && gap && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', i));
        }
        push(&mut chars, &glyph.text, sensitive, i);
    }
    search(&chars, &needle, whole_words, limit).into_iter().filter_map(|(first, last)| Some(first..last.checked_add(1)?)).collect()
}

/// [`super::find_in_text`]: the same search over a string, the owner of each character being the
/// byte where it starts, so a match comes back as the byte range of the characters it covers.
pub(super) fn find_in_text(text: &str, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let needle = needle(query, sensitive);
    if needle.is_empty() {
        return Vec::new();
    }
    let mut chars: Vec<(char, usize)> = Vec::new();
    for (at, c) in text.char_indices() {
        push(&mut chars, c.encode_utf8(&mut [0; 4]), sensitive, at);
    }
    search(&chars, &needle, whole_words, limit)
        .into_iter()
        .filter_map(|(first, last)| {
            // `last` is where the last matched character starts; the range ends after it.
            let end = last.checked_add(text.get(last..)?.chars().next()?.len_utf8())?;
            (first < end).then_some(first..end)
        })
        .collect()
}

/// The query as it is searched for: folded unless `sensitive`, whitespace runs as one space, no
/// space at either end.
fn needle(query: &str, sensitive: bool) -> Vec<char> {
    let folded = if sensitive { query.to_owned() } else { fold_case(query) };
    folded.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect()
}

/// Append `s`'s characters as they are searched (folded unless `sensitive`, any whitespace as one
/// space, a run of it collapsed as the query's is), each owned by `owner`.
fn push(chars: &mut Vec<(char, usize)>, s: &str, sensitive: bool, owner: usize) {
    let folded = if sensitive { s.to_owned() } else { fold_case(s) };
    for c in folded.chars() {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
            chars.push((c, owner));
        }
    }
}

/// Where `needle` occurs in `chars`, as the owners of its first and last characters, in order and
/// not overlapping; with `whole_words` no letter or digit may adjoin a match. At most `limit`.
fn search(chars: &[(char, usize)], needle: &[char], whole_words: bool, limit: usize) -> Vec<(usize, usize)> {
    if limit == 0 || needle.is_empty() || needle.len() > chars.len() {
        return Vec::new();
    }
    // KMP reuses a matched prefix instead of comparing a long query again at every position.
    // The table is bounded by the page's already materialized character count.
    let mut prefix = vec![0; needle.len()];
    let mut matched = 0;
    for (i, &c) in needle.iter().enumerate().skip(1) {
        matched = advance(needle, &prefix, matched, c);
        if let Some(slot) = prefix.get_mut(i) {
            *slot = matched;
        }
    }
    matched = 0;
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| c.0.is_alphanumeric());
    let mut hits = Vec::new();
    for (i, &(c, owner)) in chars.iter().enumerate() {
        matched = advance(needle, &prefix, matched, c);
        if matched != needle.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.saturating_sub(needle.len());
        let bounded = !whole_words || (!word(start.checked_sub(1)) && !word(Some(end)));
        if bounded {
            if let Some((_, first)) = chars.get(start) {
                hits.push((*first, owner));
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
