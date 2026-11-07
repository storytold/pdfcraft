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
    let alphabet = ["a", "b", " ", "-", "İ", "ı", "ς", "Σ", "\t", "\n", "ffi", "", "界", "e\u{301}", "العربية"];
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

/// Horizontal glyphs of `size` points, one per character, each line `lead` points below the
/// last and starting at `x`: (x, first line's top, text) per line.
fn laid_out(lines: &[(f32, f32, &str)], size: f32) -> PageText {
    let mut glyphs = Vec::new();
    for &(x0, top, text) in lines {
        for (k, c) in text.chars().enumerate() {
            let x = x0 + size * k as f32;
            let (y0, y1) = (top, top + size);
            glyphs.push(TextGlyph {
                text: c.into(),
                rect: [x, y0, x + size, y1],
                quad: [[x, y1], [x + size, y1], [x + size, y0], [x, y0]],
                direction: [1.0, 0.0],
            });
        }
    }
    pdfcraft_render::text::layout(glyphs)
}

#[test]
fn a_chinese_or_japanese_word_divided_by_a_line_end_is_found_on_both_lines() {
    // "検知機構" wraps after "検知" (pdf.js #15340); "ソレノイド" after "ソレ".
    let text = laid_out(&[(10.0, 10.0, "装置の検知"), (10.0, 25.0, "機構です"), (10.0, 40.0, "ソレ"), (10.0, 55.0, "ノイド")], 10.0);
    assert_eq!(text.line_of.iter().copied().max(), Some(3));
    let hits = text.find("検知機構");
    assert_eq!(hits, vec![3..7]);
    // One highlight per line, the end of the first line and the start of the second.
    assert_eq!(text.line_rects(hits[0].clone()), vec![[40.0, 10.0, 60.0, 20.0], [10.0, 25.0, 30.0, 35.0]]);
    assert_eq!(text.find("ソレノイド"), vec![9..14]);
    // The query typed with a space still finds nothing across the joined break.
    assert!(text.find("検知 機構").is_empty());
    // Copied text keeps the line break.
    assert_eq!(text.text_of(3..7), "検知\n機構");
}

#[test]
fn a_line_break_stays_a_space_beside_latin_or_hangul_and_between_side_by_side_text() {
    // Latin and Korean separate words with spaces: the line end is one.
    let latin = laid_out(&[(10.0, 10.0, "abc"), (10.0, 25.0, "def")], 10.0);
    assert_eq!(latin.find("abc def"), vec![0..6]);
    assert!(latin.find("abcdef").is_empty());
    let mixed = laid_out(&[(10.0, 10.0, "PDF"), (10.0, 25.0, "検知")], 10.0);
    assert_eq!(mixed.find("PDF 検知"), vec![0..5]);
    assert!(mixed.find("PDF検知").is_empty());
    let hangul = laid_out(&[(10.0, 10.0, "한국"), (10.0, 25.0, "어")], 10.0);
    assert_eq!(hangul.find("한국 어"), vec![0..3]);
    assert!(hangul.find("한국어").is_empty());
    // Two cells on one row read as two lines, but the second does not continue the first.
    let row = laid_out(&[(10.0, 10.0, "東京"), (200.0, 10.0, "大阪")], 10.0);
    assert_eq!(row.line_of, vec![0, 0, 1, 1]);
    assert!(row.find("東京大阪").is_empty());
    assert_eq!(row.find("東京 大阪"), vec![0..4]);
}
