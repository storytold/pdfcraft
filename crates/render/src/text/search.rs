//! Literal search in normalized page text, retaining the original glyph owners.
use super::{PageText, fold_case};
use std::ops::Range;

/// Whether `c` is a hyphen character used for word hyphenation at line breaks.
fn is_hyphen(c: char) -> bool {
    matches!(c, '-' | '\u{00AD}' | '\u{2010}' | '\u{2011}')
}

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let fold = |s: &str| if sensitive { s.to_owned() } else { fold_case(s) };
    let needle: Vec<char> = fold(query).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let (chars_dehyphenated, has_hyphen_break) = collect_chars(page, &fold, true);
    let mut hits = search_chars(&chars_dehyphenated, &needle, whole_words, limit);
    if hits.is_empty() && has_hyphen_break {
        let (chars_preserved, _) = collect_chars(page, &fold, false);
        hits = search_chars(&chars_preserved, &needle, whole_words, limit);
    }
    hits
}

/// Flatten glyphs into a character sequence, optionally removing hyphens at line breaks.
fn collect_chars(page: &PageText, fold: &impl Fn(&str) -> String, dehyphenate: bool) -> (Vec<(char, usize)>, bool) {
    let mut chars: Vec<(char, usize)> = Vec::new();
    let mut has_hyphen_break = false;
    for (i, glyph) in page.glyphs.iter().enumerate() {
        let line_break = i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        let is_hyphen_break = line_break && {
            let prev_idx = i.saturating_sub(1);
            let prev_text = page.glyphs.get(prev_idx).map_or("", |g| g.text.as_str());
            let prev_ends_hyphen = prev_text.chars().last().is_some_and(is_hyphen);
            let prev_has_alpha = prev_text.chars().rev().nth(1).is_some_and(|c| c.is_alphanumeric())
                || (prev_text.chars().count() <= 1
                    && prev_idx
                        .checked_sub(1)
                        .and_then(|p2| page.glyphs.get(p2))
                        .is_some_and(|g| g.text.chars().last().is_some_and(|c| c.is_alphanumeric())));
            let next_has_alpha = glyph.text.chars().next().is_some_and(|c| c.is_alphanumeric());
            prev_ends_hyphen && prev_has_alpha && next_has_alpha
        };
        if is_hyphen_break {
            has_hyphen_break = true;
        }
        let gap = page.space_before.get(i).copied().unwrap_or(false) || (line_break && !is_hyphen_break);
        if i > 0 && gap && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', i));
        }
        let folded = fold(&glyph.text);
        let char_count = folded.chars().count();
        for (ci, c) in folded.chars().enumerate() {
            if dehyphenate && ci + 1 == char_count && is_hyphen(c) {
                let next_is_break = page.line_of.get(i).zip(page.line_of.get(i.saturating_add(1))).is_some_and(|(a, b)| a != b);
                if next_is_break {
                    let next_has_alpha =
                        page.glyphs.get(i.saturating_add(1)).is_some_and(|g| g.text.chars().next().is_some_and(|c| c.is_alphanumeric()));
                    let has_alpha_before = ci > 0
                        || i.checked_sub(1)
                            .and_then(|p| page.glyphs.get(p))
                            .is_some_and(|g| g.text.chars().last().is_some_and(|c| c.is_alphanumeric()));
                    if next_has_alpha && has_alpha_before {
                        has_hyphen_break = true;
                        continue;
                    }
                }
            }
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
                chars.push((c, i));
            }
        }
    }
    (chars, has_hyphen_break)
}

/// Scan characters using the Knuth-Morris-Pratt algorithm and return matching glyph ranges.
fn search_chars(chars: &[(char, usize)], needle: &[char], whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if needle.len() > chars.len() {
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
