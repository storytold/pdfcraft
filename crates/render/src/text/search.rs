//! Literal search in normalized page text, retaining the original glyph owners.
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
        let line_break = i.checked_sub(1).and_then(|previous| page.line_of.get(previous)).zip(page.line_of.get(i)).is_some_and(|(a, b)| a != b);
        let gap = page.space_before.get(i).copied().unwrap_or(false) || line_break;
        if i > 0 && gap && chars.last().is_some_and(|c| c.0 != ' ') {
            chars.push((' ', i));
        }
        for c in fold(&glyph.text).chars() {
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
    // A neighbour bounds a word only in a script that puts spaces between words. Han, kana, Thai
    // and the like run their words together, so a character of theirs next to the match is no
    // reason to reject it: Acrobat finds 台北 inside 台北市 with Whole words only on. Digits still
    // bound, in any script: ๑ is not a whole word inside the number ๑๒.
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|c| c.0.is_alphanumeric() && (c.0.is_numeric() || !unspaced_script(c.0)));
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

/// Extend a prefix, falling back through strictly shorter proper prefixes on a mismatch.
fn advance(needle: &[char], prefix: &[usize], mut matched: usize, c: char) -> usize {
    while matched > 0 && needle.get(matched) != Some(&c) {
        matched = prefix.get(matched - 1).copied().unwrap_or_default();
    }
    if needle.get(matched) == Some(&c) { matched.saturating_add(1) } else { 0 }
}

/// Whether `c` is a letter of a script written without spaces between words (CJK ideographs,
/// kana, bopomofo, Thai, Lao, Myanmar, Khmer, Tibetan), so that a whole-word search cannot take a
/// neighbouring letter as a word boundary.
fn unspaced_script(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0E00..=0x0EFF      // Thai, Lao
        | 0x0F00..=0x0FFF    // Tibetan
        | 0x1000..=0x109F    // Myanmar
        | 0x1780..=0x17FF    // Khmer
        | 0x2E80..=0x2FDF    // CJK and Kangxi radicals
        | 0x3005..=0x3007    // 々 〆 〇
        | 0x3031..=0x3035    // vertical kana repeat marks
        | 0x303B..=0x303C    // 〻 〼
        | 0x3040..=0x30FF    // hiragana, katakana
        | 0x3100..=0x312F    // bopomofo
        | 0x31A0..=0x31BF    // bopomofo extended
        | 0x31F0..=0x31FF    // katakana phonetic extensions
        | 0x3400..=0x4DBF    // CJK extension A
        | 0x4E00..=0x9FFF    // CJK unified ideographs
        | 0xA9E0..=0xA9FF    // Myanmar extended-B
        | 0xAA60..=0xAA7F    // Myanmar extended-A
        | 0xF900..=0xFAFF    // CJK compatibility ideographs
        | 0xFF66..=0xFF9F    // halfwidth katakana
        | 0x1AFF0..=0x1B16F  // kana extended-B, kana supplement, kana extended-A, small kana
        // The supplementary and tertiary ideographic planes, all CJK ideographs (extensions B
        // to J, compatibility supplement).
        | 0x20000..=0x3FFFF
    )
}
