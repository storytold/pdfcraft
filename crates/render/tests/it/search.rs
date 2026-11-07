use pdfcraft_render::{PageText, TextGlyph, text::fold_case};
use std::ops::Range;

fn page(parts: &[&str]) -> PageText {
    PageText {
        glyphs: parts.iter().map(|text| TextGlyph { text: (*text).into(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] }).collect(),
        line_of: vec![0; parts.len()],
        space_before: vec![false; parts.len()],
    }
}

// The original straightforward search is an independent oracle for valid page metadata.
fn reference(page: &PageText, query: &str, sensitive: bool, whole: bool, limit: usize) -> Vec<Range<usize>> {
    let fold = |s: &str| if sensitive { s.to_owned() } else { fold_case(s) };
    // Soft hyphens and zero width characters draw nothing and are skipped on both sides.
    let visible =
        |s: String| s.chars().filter(|c| !matches!(c, '\u{AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}')).collect::<String>();
    let needle: Vec<char> = visible(fold(query)).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut chars = Vec::new();
    for (i, glyph) in page.glyphs.iter().enumerate() {
        // A line that ends in a soft hyphen runs on to the next.
        let line_break = i > 0 && page.line_of[i] != page.line_of[i - 1] && !page.glyphs[i - 1].text.ends_with('\u{AD}');
        if i > 0 && (page.space_before[i] || line_break) && chars.last().is_some_and(|&(c, _)| c != ' ') {
            chars.push((' ', i));
        }
        for c in visible(fold(&glyph.text)).chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|&(last, _)| last != ' ') {
                chars.push((c, i));
            }
        }
    }
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|&(c, _)| c.is_alphanumeric());
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

#[test]
fn search_matches_reference_for_unicode_gaps_limits_and_word_boundaries() {
    let alphabet = ["a", "b", " ", "-", "İ", "ı", "ς", "Σ", "\t", "\n", "ffi", "", "界", "e\u{301}", "العربية", "\u{AD}", "a\u{200B}"];
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

/// pdf.js issue 13226 stores "Mitarbeiterinnen" with soft hyphens at its hyphenation points, and
/// issue 9186 puts zero width spaces next to the gaps between words. Find skips both characters
/// (and the other invisible joiners) in the page and in the query, and a match still starts and
/// ends on visible letters.
#[test]
fn find_skips_soft_hyphens_and_zero_width_characters() {
    let word = page(&["M", "i", "t", "\u{AD}", "a", "r", "b", "e", "i", "\u{AD}", "t", "e", "r", "i", "n", "n", "e", "n"]);
    assert_eq!(word.find("Mitarbeiterinnen"), vec![0..18]);
    assert_eq!(word.find_opts("Mitarbeiterinnen", true, true), vec![0..18]);
    assert_eq!(word.find("arbei"), vec![4..9]);
    // Inside one word, a piece of it is not a whole word.
    assert!(word.find_opts("terinnen", false, true).is_empty());
    // A query that carries the soft hyphens still matches.
    assert_eq!(word.find("Mit\u{AD}arbei\u{AD}terinnen"), vec![0..18]);

    let mut gaps = page(&["is", "\u{200B}", "\u{200B}", "made", "\u{2060}", "as"]);
    gaps.space_before = vec![false, false, true, false, false, true];
    assert_eq!(gaps.find("is made"), vec![0..4]);
    assert_eq!(gaps.find_opts("made as", false, true), vec![3..6]);
    for joiner in ['\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}'] {
        assert_eq!(page(&["ab", &joiner.to_string(), "cd"]).find("abcd"), vec![0..3], "{joiner:?}");
    }
    assert!(page(&["\u{200B}"]).find("\u{200B}").is_empty());
}

/// A soft hyphen at a line end is where the word was divided (ISO 32000-2 §14.8.2.3): the break
/// after it is no word break, so "Mitar\u{AD}" ending one line and "beiterinnen" starting the next
/// are found as one word, highlighted on both lines. A line break after any other glyph still
/// separates words.
#[test]
fn find_joins_a_word_divided_at_a_line_end_soft_hyphen() {
    let parts: Vec<String> = "Mitar\u{AD}beiterinnen".chars().map(String::from).collect();
    let mut text = page(&parts.iter().map(String::as_str).collect::<Vec<_>>());
    for (i, glyph) in text.glyphs.iter_mut().enumerate() {
        let x = i as f32 * 10.0;
        glyph.rect = [x, 0.0, x + 10.0, 10.0];
    }
    // "Mitar" and the soft hyphen on the first line, "beiterinnen" on the second.
    for line in text.line_of.iter_mut().skip(6) {
        *line = 1;
    }
    let hits = text.find("Mitarbeiterinnen");
    assert_eq!(hits, vec![0..17]);
    assert_eq!(text.line_rects(hits[0].clone()).len(), 2);
    assert_eq!(text.find_opts("Mitarbeiterinnen", true, true), vec![0..17]);
    assert!(text.find("Mitar beiterinnen").is_empty());

    // The soft hyphen as the last character of a longer glyph works the same way.
    let mut text = page(&["Mitar\u{AD}", "beiterinnen"]);
    text.line_of = vec![0, 1];
    assert_eq!(text.find("Mitarbeiterinnen"), vec![0..2]);

    // Without the soft hyphen the line break is a word break.
    let mut text = page(&["Mitar", "beiterinnen"]);
    text.line_of = vec![0, 1];
    assert!(text.find("Mitarbeiterinnen").is_empty());
    assert_eq!(text.find("Mitar beiterinnen"), vec![0..2]);
}
