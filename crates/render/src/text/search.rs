//! Literal search in normalized page text, retaining the original glyph owners.
use super::{PageText, for_search, glyph_range, is_redundant_space};
use crate::search_fold::is_combining;
use std::ops::Range;

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let needle = needle(query, sensitive);
    if needle.is_empty() {
        return Vec::new();
    }
    let (chars, owner) = page.search_text(sensitive);
    if needle.len() > chars.len() {
        return Vec::new();
    }
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
    matched = 0;
    // A combining mark continues the word of the letter before it, so a decomposed accent does
    // not end a word where its composed spelling would not.
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| c.is_alphanumeric() || is_combining(*c));
    let mut hits = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        matched = advance(&needle, &prefix, matched, c);
        if matched != needle.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.saturating_sub(needle.len());
        let bounded = !whole_words || (!word(start.checked_sub(1)) && !word(Some(end)));
        if bounded {
            // The needle neither starts nor ends with a space, so a match never starts or ends on
            // an inserted separator: its first and last characters each have a glyph.
            if let Some(glyphs) = glyph_range(&owner, start..end) {
                hits.push(glyphs);
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

/// The query as search compares it: each character as [`for_search`] reads it, without leading,
/// trailing or repeated spaces.
fn needle(query: &str, sensitive: bool) -> Vec<char> {
    let mut scratch = Vec::new();
    let mut needle: Vec<char> = Vec::new();
    for c in query.chars() {
        for_search(c, sensitive, &mut scratch, |c| {
            if !is_redundant_space(c, needle.last()) {
                needle.push(c);
            }
        });
    }
    while needle.last() == Some(&' ') {
        needle.pop();
    }
    needle
}

/// Extend a prefix, falling back through strictly shorter proper prefixes on a mismatch.
fn advance(needle: &[char], prefix: &[usize], mut matched: usize, c: char) -> usize {
    while matched > 0 && needle.get(matched) != Some(&c) {
        matched = prefix.get(matched - 1).copied().unwrap_or_default();
    }
    if needle.get(matched) == Some(&c) { matched.saturating_add(1) } else { 0 }
}
