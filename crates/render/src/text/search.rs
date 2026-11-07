//! Literal search in normalized page text, retaining the original glyph owners.
//!
//! Words and lines are separated by one space, except where a line of Chinese or Japanese runs on
//! to the next: a break between two such characters is no word break, so it reads as nothing and
//! "検知機構" is found when the line ends after "検知".
use super::{PageText, fold_case};
use std::ops::Range;

pub(super) fn find(page: &PageText, query: &str, sensitive: bool, whole_words: bool, limit: usize) -> Vec<Range<usize>> {
    if limit == 0 {
        return Vec::new();
    }
    let fold = |s: &str| if sensitive { s.to_owned() } else { fold_case(s) };
    let needle: Vec<char> = fold(query).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut chars: Vec<(char, usize)> = Vec::new();
    for (i, glyph) in page.glyphs.iter().enumerate() {
        let text = fold(&glyph.text);
        let line_break = i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        let joined = line_break
            && chars.last().zip(text.chars().next()).is_some_and(|(before, after)| unspaced(before.0) && unspaced(after))
            && wraps(page, i);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || (line_break && !joined);
        if i > 0 && gap && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', i));
        }
        for c in text.chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|last| last.0 != ' ') {
                chars.push((c, i));
            }
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

/// Whether `c` is set solid, with no spaces between words, in text that a line break may divide
/// anywhere: East Asian Width Wide, Fullwidth or Halfwidth (Unicode Standard Annex #11) Han,
/// kana, bopomofo, CJK symbols and punctuation and fullwidth forms. Hangul is left out: Korean
/// separates its words with spaces, so a line end there is a word break. A line break between
/// two such characters is the CSS segment-break rule's case for removing the break, not turning
/// it into a space (CSS Text Module, segment break transformation).
fn unspaced(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FFF // CJK radicals, Kangxi radicals, ideographic description characters
        | 0x3001..=0x303E // CJK symbols and punctuation (U+3000 is a space; U+303F is narrow)
        | 0x3041..=0x30FF // Hiragana, Katakana
        | 0x3105..=0x312F // Bopomofo
        | 0x3190..=0x31FF // Kanbun, Bopomofo extended, CJK strokes, Katakana phonetic extensions
        | 0x3200..=0x4DBF // Enclosed CJK letters, CJK compatibility, Han extension A
        | 0x4E00..=0x9FFF // Han
        | 0xF900..=0xFAFF // CJK compatibility ideographs
        | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F // vertical, compatibility and small forms
        | 0xFF01..=0xFF60 | 0xFFE0..=0xFFE6 // fullwidth forms
        | 0xFF61..=0xFF9F // halfwidth CJK punctuation and Katakana (not halfwidth Hangul)
        | 0x16FE0..=0x16FFF | 0x1AFF0..=0x1B2FF // ideographic symbols, kana supplements and extensions
        | 0x20000..=0x3FFFD // Han extensions B and later, compatibility supplement
    ) && !matches!(c as u32, 0x3200..=0x321E | 0x3260..=0x327E) // parenthesised and circled Hangul
}

/// Whether glyph `i` starts the line that the text of glyph `i - 1` runs on to: it sits past that
/// glyph across the lines, by more than half the glyph's line height. Text set side by side on one
/// line (table cells, labels) and a column that starts back at the top keep their word break.
fn wraps(page: &PageText, i: usize) -> bool {
    let Some((before, after)) = i.checked_sub(1).and_then(|p| page.glyphs.get(p)).zip(page.glyphs.get(i)) else { return false };
    let centre = |q: &[[f32; 2]; 4]| [q.iter().map(|p| p[0]).sum::<f32>() / 4.0, q.iter().map(|p| p[1]).sum::<f32>() / 4.0];
    let (a, b) = (centre(&before.quad), centre(&after.quad));
    // Towards the next line: the writing direction turned a quarter clockwise in y-down view space.
    let [dx, dy] = before.direction;
    let across = (b[0] - a[0]) * -dy + (b[1] - a[1]) * dx;
    let [near, far] = [before.quad[0], before.quad[3]];
    let height = (near[0] - far[0]).hypot(near[1] - far[1]);
    // NaN compares false: no join.
    across > height * 0.5
}

/// Extend a prefix, falling back through strictly shorter proper prefixes on a mismatch.
fn advance(needle: &[char], prefix: &[usize], mut matched: usize, c: char) -> usize {
    while matched > 0 && needle.get(matched) != Some(&c) {
        matched = prefix.get(matched - 1).copied().unwrap_or_default();
    }
    if needle.get(matched) == Some(&c) { matched.saturating_add(1) } else { 0 }
}
