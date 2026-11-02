//! Literal search in normalized page text, retaining the original glyph owners.
use super::{FindOptions, PageText, fold_accents_from, fold_case};
use std::ops::Range;

pub(super) fn find(page: &PageText, query: &str, options: FindOptions, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let FindOptions { case_sensitive, whole_words, accent_sensitive } = options;
    // Each glyph's text is folded on its own, so a fold that drops characters (an accent mark)
    // still leaves every kept character owned by the glyph it came from. `attached` says the
    // text continues a Latin letter with no space or line break between, so a mark at its start
    // is that letter's accent however the text was split into glyphs.
    let fold = |s: &str, attached: bool| {
        let s = if case_sensitive { s.to_owned() } else { fold_case(s) };
        if accent_sensitive { s } else { fold_accents_from(&s, attached) }
    };
    let needle: Vec<char> = fold(query, false).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut chars: Vec<(char, usize)> = Vec::new();
    // Where a match ending in glyph `i` ends: just after it, or after the glyphs that hold only
    // accents drawn onto its letter (decomposed text), which the match covers too.
    let mut reach: Vec<usize> = (1..=page.glyphs.len()).collect();
    for (i, glyph) in page.glyphs.iter().enumerate() {
        let line_break = i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || line_break;
        if i > 0 && gap && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', i));
        }
        // After a space or line break the last character is that space, so this is false then.
        let attached = chars.last().is_some_and(|c| c.0.is_ascii_alphabetic());
        let before = chars.len();
        for c in fold(&glyph.text, attached).chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
                chars.push((c, i));
            }
        }
        // A glyph of nothing but accents on the letter before it adds no characters; the match
        // that ends on that letter takes the glyph with it. (A glyph that holds an accent and
        // other text is not taken: a range is whole glyphs, and the match would then also cover
        // letters that were not searched for.)
        if attached
            && chars.len() == before
            && !glyph.text.is_empty()
            && let Some(&(_, owner)) = chars.last()
            && let Some(end) = reach.get_mut(owner)
        {
            *end = i.saturating_add(1);
        }
    }
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
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| c.0.is_alphanumeric());
    let mut hits = Vec::new();
    for (i, &(c, owner)) in chars.iter().enumerate() {
        matched = advance(&needle, &prefix, matched, c);
        if matched != needle.len() {
            continue;
        }
        let end = i.saturating_add(1);
        let start = end.saturating_sub(needle.len());
        let bounded = !whole_words || (!word(start.checked_sub(1)) && !word(Some(end)));
        if bounded {
            if let Some((_, first)) = chars.get(start)
                && let Some(&last) = reach.get(owner)
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
