use pdfcraft_render::{PageText, TextGlyph, text::fold_case};
use std::ops::Range;

fn page(parts: &[&str]) -> PageText {
    PageText {
        glyphs: parts.iter().map(|text| TextGlyph { text: (*text).into(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] }).collect(),
        line_of: vec![0; parts.len()],
        space_before: vec![false; parts.len()],
    }
}

/// The letters of the property test's alphabet that come from scripts written without spaces
/// between words: 界 (CJK Unified Ideographs, U+4E00..U+9FFF) and ก (Thai, U+0E00..U+0E7F).
/// 한 (Hangul Syllables) is written with spaces and bounds a word. The reference knows only
/// these three letters, so it does not share the block table in `text/search.rs`; that table is
/// checked against `UNSPACED_BLOCK_SAMPLES` instead.
fn unspaced(c: char) -> bool {
    matches!(c, '界' | 'ก')
}

/// Letters at the edges of each block of a script written without spaces (`true`), and letters
/// just outside them (`false`), named by their block as Unicode's Blocks.txt gives it. Each one is
/// alphabetic and not a digit, so whether it bounds a whole word depends on the table alone.
const UNSPACED_BLOCK_SAMPLES: &[(char, bool, &str)] = &[
    ('\u{0DC6}', false, "Sinhala (0D80..0DFF), before Thai"),
    ('\u{0E01}', true, "Thai (0E00..0E7F), first letter"),
    ('\u{0E46}', true, "Thai, MAIYAMOK"),
    ('\u{0E81}', true, "Lao (0E80..0EFF), first letter"),
    ('\u{0EDF}', true, "Lao, last letter"),
    ('\u{0F00}', true, "Tibetan (0F00..0FFF), first letter"),
    ('\u{0F8C}', true, "Tibetan, last letter"),
    ('\u{1000}', true, "Myanmar (1000..109F), first letter"),
    ('\u{108E}', true, "Myanmar, last letter"),
    ('\u{10A0}', false, "Georgian (10A0..10FF), after Myanmar"),
    ('\u{1770}', false, "Tagbanwa (1760..177F), before Khmer"),
    ('\u{1780}', true, "Khmer (1780..17FF), first letter"),
    ('\u{17DC}', true, "Khmer, last letter"),
    ('\u{1820}', false, "Mongolian (1800..18AF), after Khmer"),
    ('\u{3005}', true, "CJK Symbols and Punctuation (3000..303F), ideographic iteration mark"),
    ('\u{3006}', true, "CJK Symbols and Punctuation, ideographic closing mark"),
    ('\u{3031}', true, "CJK Symbols and Punctuation, vertical kana repeat mark"),
    ('\u{3035}', true, "CJK Symbols and Punctuation, vertical kana repeat mark lower half"),
    ('\u{303B}', true, "CJK Symbols and Punctuation, vertical ideographic iteration mark"),
    ('\u{303C}', true, "CJK Symbols and Punctuation, masu mark"),
    ('\u{3041}', true, "Hiragana (3040..309F), first letter"),
    ('\u{309F}', true, "Hiragana, last letter"),
    ('\u{30A1}', true, "Katakana (30A0..30FF), first letter"),
    ('\u{30FF}', true, "Katakana, last letter"),
    ('\u{3105}', true, "Bopomofo (3100..312F), first letter"),
    ('\u{312F}', true, "Bopomofo, last letter"),
    ('\u{3131}', false, "Hangul Compatibility Jamo (3130..318F), after Bopomofo"),
    ('\u{318E}', false, "Hangul Compatibility Jamo, last letter, before Bopomofo Extended"),
    ('\u{31A0}', true, "Bopomofo Extended (31A0..31BF), first letter"),
    ('\u{31BF}', true, "Bopomofo Extended, last letter"),
    ('\u{31F0}', true, "Katakana Phonetic Extensions (31F0..31FF), first letter"),
    ('\u{31FF}', true, "Katakana Phonetic Extensions, last letter"),
    ('\u{3400}', true, "CJK Unified Ideographs Extension A (3400..4DBF), first"),
    ('\u{4DBF}', true, "CJK Unified Ideographs Extension A, last"),
    ('\u{4E00}', true, "CJK Unified Ideographs (4E00..9FFF), first"),
    ('\u{9FFF}', true, "CJK Unified Ideographs, last"),
    ('\u{A000}', false, "Yi Syllables (A000..A48F), after CJK Unified Ideographs"),
    ('\u{A9CF}', false, "Javanese (A980..A9DF), before Myanmar Extended-B"),
    ('\u{A9E0}', true, "Myanmar Extended-B (A9E0..A9FF), first letter"),
    ('\u{A9FE}', true, "Myanmar Extended-B, last letter"),
    ('\u{AA00}', false, "Cham (AA00..AA5F), after Myanmar Extended-B"),
    ('\u{AA4B}', false, "Cham, before Myanmar Extended-A"),
    ('\u{AA60}', true, "Myanmar Extended-A (AA60..AA7F), first letter"),
    ('\u{AA7F}', true, "Myanmar Extended-A, last letter"),
    ('\u{AA80}', false, "Tai Viet (AA80..AADF), after Myanmar Extended-A"),
    ('\u{D7FB}', false, "Hangul Jamo Extended-B (D7B0..D7FF), before CJK Compatibility Ideographs"),
    ('\u{F900}', true, "CJK Compatibility Ideographs (F900..FAFF), first"),
    ('\u{FAD9}', true, "CJK Compatibility Ideographs, last"),
    ('\u{FB00}', false, "Alphabetic Presentation Forms (FB00..FB4F), after CJK Compatibility Ideographs"),
    ('\u{FF5A}', false, "Halfwidth and Fullwidth Forms (FF00..FFEF), fullwidth z, before halfwidth katakana"),
    ('\u{FF66}', true, "Halfwidth and Fullwidth Forms, first halfwidth katakana"),
    ('\u{FF9F}', true, "Halfwidth and Fullwidth Forms, last halfwidth katakana"),
    ('\u{FFA1}', false, "Halfwidth and Fullwidth Forms, halfwidth Hangul, after halfwidth katakana"),
    ('\u{1AFF0}', true, "Kana Extended-B (1AFF0..1AFFF), first letter"),
    ('\u{1B000}', true, "Kana Supplement (1B000..1B0FF), first letter"),
    ('\u{1B122}', true, "Kana Extended-A (1B100..1B12F), last letter"),
    ('\u{1B132}', true, "Small Kana Extension (1B130..1B16F), first letter"),
    ('\u{1B167}', true, "Small Kana Extension, last letter"),
    ('\u{1B170}', false, "Nushu (1B170..1B2FF), after Small Kana Extension"),
    ('\u{20000}', true, "CJK Unified Ideographs Extension B (20000..2A6DF), first"),
    ('\u{2A6DF}', true, "CJK Unified Ideographs Extension B, last"),
    ('\u{2F800}', true, "CJK Compatibility Ideographs Supplement (2F800..2FA1F), first"),
    ('\u{323AF}', true, "CJK Unified Ideographs Extension H (31350..323AF), last"),
];

// The original straightforward search is an independent oracle for valid page metadata.
fn reference(page: &PageText, query: &str, sensitive: bool, whole: bool, limit: usize) -> Vec<Range<usize>> {
    let fold = |s: &str| if sensitive { s.to_owned() } else { fold_case(s) };
    let needle: Vec<char> = fold(query).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut chars = Vec::new();
    for (i, glyph) in page.glyphs.iter().enumerate() {
        if i > 0 && (page.space_before[i] || page.line_of[i] != page.line_of[i - 1]) && chars.last().is_some_and(|&(c, _)| c != ' ') {
            chars.push((' ', i));
        }
        for c in fold(&glyph.text).chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|&(last, _)| last != ' ') {
                chars.push((c, i));
            }
        }
    }
    // A neighbour bounds a word only in a script written with spaces between words, or when it
    // is a digit.
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|&(c, _)| c.is_alphanumeric() && (c.is_numeric() || !unspaced(c)));
    let mut hits = Vec::new();
    let mut start = 0;
    while start + needle.len() <= chars.len() {
        let end = start + needle.len();
        if (!whole || (!word(start.checked_sub(1)) && !word(Some(end)))) && chars[start..end].iter().map(|&(c, _)| c).eq(needle.iter().copied()) {
            hits.push(chars[start].1..chars[end - 1].1 + 1);
            if hits.len() == limit {
                break;
            }
            start = end;
        } else {
            start += 1;
        }
    }
    hits
}

#[test]
fn search_tolerates_missing_layout_metadata() {
    let mut text = page(&["a", "b", "c"]);
    text.line_of.clear();
    text.space_before.clear();
    assert_eq!(text.find("abc"), vec![0..3]);
    text.space_before = vec![false, true];
    assert_eq!(text.find("a bc"), vec![0..3]);
    text.line_of = vec![0, 1];
    assert_eq!(text.find("a bc"), vec![0..3]);
}

#[test]
fn whole_word_rejection_keeps_overlapping_candidates() {
    let text = page(&["xaa-aa-aa"]);
    assert_eq!(text.find_opts("aa-aa", true, true), vec![0..1]);
    // Accepted matches still do not overlap, even within one multi-character glyph.
    assert_eq!(page(&["aaaaa"]).find("aaa"), vec![0..1]);
    assert_eq!(page(&["aaaaa"]).find("aa"), vec![0..1, 0..1]);
}

/// A letter from a block in `UNSPACED_BLOCK_SAMPLES` next to a whole-word match leaves it standing
/// when the block's script is written without spaces, and bounds it otherwise, on either side.
#[test]
fn whole_words_follow_the_unicode_blocks_of_unspaced_scripts() {
    for &(c, unspaced, block) in UNSPACED_BLOCK_SAMPLES {
        let (neighbour, cp) = (c.to_string(), u32::from(c));
        assert!(c.is_alphanumeric() && !c.is_numeric(), "U+{cp:04X} ({block}) is a letter, so the sample tests the table");
        // Case-sensitive, so that case folding does not move a neighbour into another block
        // (Georgian U+10A0 folds to U+2D00).
        let (before, after) = if unspaced { (Some(1..2), Some(0..1)) } else { (None, None) };
        assert_eq!(page(&[&neighbour, "PDF"]).find_opts("PDF", true, true), Vec::from_iter(before), "U+{cp:04X} ({block}) before the match");
        assert_eq!(page(&["PDF", &neighbour]).find_opts("PDF", true, true), Vec::from_iter(after), "U+{cp:04X} ({block}) after the match");
    }
}

#[test]
fn search_matches_reference_for_unicode_gaps_limits_and_word_boundaries() {
    let alphabet = ["a", "b", " ", "-", "İ", "ı", "ς", "Σ", "\t", "\n", "ffi", "", "界", "ก", "한", "e\u{301}", "العربية"];
    let mut seed = 1234567u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 32) as usize
    };
    for case in 0..2000 {
        let parts: Vec<_> = (0..10 + case % 70).map(|_| alphabet[next() % alphabet.len()]).collect();
        let query = if case % 2 == 0 {
            parts.iter().skip(case % 8).take(1 + case % 9).copied().collect::<String>()
        } else {
            (0..1 + case % 9).map(|_| alphabet[next() % alphabet.len()]).collect::<String>()
        };
        let mut text = page(&parts);
        for i in 1..parts.len() {
            text.space_before[i] = next() % 11 == 0;
            text.line_of[i] = text.line_of[i - 1] + u32::from(next() % 17 == 0);
        }
        for sensitive in [false, true] {
            for whole in [false, true] {
                for limit in [0, 1, 2, usize::MAX] {
                    assert_eq!(
                        text.find_opts_limited(&query, sensitive, whole, limit),
                        reference(&text, &query, sensitive, whole, limit),
                        "case {case}: {query:?}, sensitive={sensitive}, whole={whole}, limit={limit}"
                    );
                }
            }
        }
    }
}

#[test]
fn long_repeated_prefix_search_preserves_the_last_match_and_limit() {
    let mut parts = vec!["a"; 131072];
    parts.push("b");
    let text = page(&parts);
    let query = format!("{}b", "a".repeat(4095));
    assert_eq!(text.find_opts_limited(&query, false, false, 1), vec![126977..131073]);
    assert!(text.find(&format!("{}c", "a".repeat(4095))).is_empty());
}
