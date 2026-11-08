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
    // Each character, its glyph, and whether a skipped zero width space came right before it.
    let mut chars: Vec<(char, usize, bool)> = Vec::new();
    let mut after_zwsp = false;
    for (i, glyph) in page.glyphs.iter().enumerate() {
        // A line that ends in a soft hyphen runs on to the next.
        let line_break = i > 0 && page.line_of[i] != page.line_of[i - 1] && !page.glyphs[i - 1].text.ends_with('\u{AD}');
        if i > 0 && (page.space_before[i] || line_break) && chars.last().is_some_and(|&(c, ..)| c != ' ') {
            chars.push((' ', i, false));
            after_zwsp = false;
        }
        for c in fold(&glyph.text).chars() {
            after_zwsp |= c == '\u{200B}';
            if visible(c.to_string()).is_empty() {
                continue;
            }
            let c = if c.is_whitespace() { ' ' } else { c };
            if c != ' ' || chars.last().is_none_or(|&(last, ..)| last != ' ') {
                chars.push((c, i, after_zwsp && c != ' '));
                after_zwsp = false;
            }
        }
    }
    let word = |i: Option<usize>| i.and_then(|i| chars.get(i)).is_some_and(|&(c, ..)| c.is_alphanumeric());
    // A word ends at a skipped zero width space as it does at a space.
    let edge = |i: usize| chars.get(i).is_some_and(|&(.., zwsp)| zwsp);
    let mut hits = Vec::new();
    let mut start = 0;
    while start + needle.len() <= chars.len() {
        let end = start + needle.len();
        let bounded = (edge(start) || !word(start.checked_sub(1))) && (edge(end) || !word(Some(end)));
        if (!whole || bounded) && chars[start..end].iter().map(|&(c, ..)| c).eq(needle.iter().copied()) {
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
        // The same character typed in the query is skipped too.
        assert_eq!(page(&["ab", "cd"]).find_opts(&format!("ab{joiner}cd"), true, false), vec![0..2], "{joiner:?}");
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
    // Nor does a cell set lower and further along the row: a wrapped line starts back behind the
    // end of the line before it.
    let staggered = laid_out(&[(10.0, 10.0, "東京"), (200.0, 17.0, "大阪")], 10.0);
    assert_eq!(staggered.line_of, vec![0, 0, 1, 1]);
    assert!(staggered.find("東京大阪").is_empty());
    assert_eq!(staggered.find("東京 大阪"), vec![0..4]);
    // Fullwidth Latin letters and digits are Latin text set wide: their line end is a word break.
    let wide = laid_out(&[(10.0, 10.0, "ＡＢ"), (10.0, 25.0, "ＣＤ"), (10.0, 40.0, "１２"), (10.0, 55.0, "３４")], 10.0);
    assert!(wide.find_opts("ＡＢＣＤ", true, false).is_empty());
    assert_eq!(wide.find_opts("ＡＢ ＣＤ", true, false), vec![0..4]);
    assert!(wide.find("１２３４").is_empty());
    assert_eq!(wide.find("１２ ３４"), vec![4..8]);
}

/// Vertical glyphs (WMode 1) of `size` points, one per character, written downwards: each
/// column starts at `top` with its left edge at `x`: (x, top, text) per column, in reading order.
/// Each column is one line, as the text device groups a vertical flow.
fn columns(cols: &[(f32, f32, &str)], size: f32) -> PageText {
    let mut text = PageText::default();
    for (line, &(x, top, chars)) in (0u32..).zip(cols) {
        for (k, c) in chars.chars().enumerate() {
            let y = top + size * k as f32;
            // Written downwards: the em box's edge towards the next line is its left one.
            text.glyphs.push(TextGlyph {
                text: c.into(),
                rect: [x, y, x + size, y + size],
                quad: [[x, y], [x, y + size], [x + size, y + size], [x + size, y]],
                direction: [0.0, 1.0],
            });
            text.line_of.push(line);
            text.space_before.push(false);
        }
    }
    text
}

#[test]
fn a_word_divided_between_vertical_columns_is_found_in_both() {
    // Japanese set vertically: columns run top to bottom and follow each other to the left.
    // "検知機構" breaks after "検知" at the foot of the right column.
    let text = columns(&[(40.0, 10.0, "装置の検知"), (25.0, 10.0, "機構です")], 10.0);
    let hits = text.find("検知機構");
    assert_eq!(hits, vec![3..7]);
    // One highlight per column: the foot of the right one and the head of the left one.
    assert_eq!(text.line_rects(hits[0].clone()), vec![[40.0, 40.0, 50.0, 60.0], [25.0, 10.0, 35.0, 30.0]]);
    assert_eq!(
        text.text_of(3..7),
        "検知
機構"
    );

    // A column that stands to the right of the one before does not continue it.
    let back = columns(&[(40.0, 10.0, "装置の検知"), (55.0, 10.0, "機構です")], 10.0);
    assert!(back.find("検知機構").is_empty());
    assert_eq!(back.find("検知 機構"), vec![3..7]);
}

/// A zero width space draws nothing and is skipped, but UAX #29 breaks words on both sides of it:
/// Thai and Khmer write it between words with no visible gap, so each word is still a whole word.
/// The joiners and the soft hyphen join, so they are no boundary.
#[test]
fn a_zero_width_space_is_a_word_boundary_for_whole_words() {
    let thai = page(&["สวัสดี", "\u{200B}", "ครับ"]);
    assert_eq!(thai.find_opts("ครับ", false, true), vec![2..3]);
    assert_eq!(thai.find_opts("สวัสดี", false, true), vec![0..1]);
    // The words read together, as typed with no space, still match.
    assert_eq!(thai.find_opts("สวัสดีครับ", false, true), vec![0..3]);
    // Inside one glyph's text too.
    assert_eq!(page(&["ab\u{200B}cd"]).find_opts("cd", false, true), vec![0..1]);
    for joiner in ["\u{AD}", "\u{200C}", "\u{200D}", "\u{2060}", "\u{FEFF}"] {
        let text = page(&["ab", joiner, "cd"]);
        assert!(text.find_opts("cd", false, true).is_empty(), "{joiner:?}");
        assert!(text.find_opts("ab", false, true).is_empty(), "{joiner:?}");
        assert_eq!(text.find_opts("abcd", false, true), vec![0..3], "{joiner:?}");
    }
}

/// Every range `unspaced` lists joins a line it ends to the next one, on either side of the break;
/// Hangul, parenthesised and circled Hangul, fullwidth Latin and digits, Yi and ASCII keep a space.
#[test]
fn each_unspaced_range_joins_and_spaced_scripts_do_not() {
    let joined = [
        '\u{2E80}',
        '\u{2F00}', // CJK radicals, Kangxi radicals
        '\u{3001}',
        '\u{3006}', // CJK symbols and punctuation
        '\u{3042}',
        '\u{30A2}', // Hiragana, Katakana
        '\u{3105}', // Bopomofo
        '\u{3190}',
        '\u{31A0}',
        '\u{31C0}',
        '\u{31F0}', // Kanbun, Bopomofo extended, strokes, Katakana extensions
        '\u{3220}',
        '\u{3300}',
        '\u{3400}', // enclosed ideograph, CJK compatibility, Han extension A
        '\u{4E2D}', // Han
        '\u{F900}', // CJK compatibility ideographs
        '\u{FE10}',
        '\u{FE30}',
        '\u{FE50}', // vertical, compatibility and small forms
        '\u{FF01}',
        '\u{FF1A}',
        '\u{FF3B}',
        '\u{FF5B}',
        '\u{FFE0}', // fullwidth punctuation and signs
        '\u{FF61}',
        '\u{FF71}', // halfwidth CJK punctuation, halfwidth Katakana
        '\u{16FE0}',
        '\u{1AFF0}',
        '\u{1B000}', // ideographic symbols, kana extended and supplement
        '\u{20000}',
        '\u{2F800}',
        '\u{30000}', // Han extension B, compatibility supplement, extension G
    ];
    for c in joined {
        let text = laid_out(&[(10.0, 10.0, &format!("中{c}")), (10.0, 25.0, &format!("{c}中"))], 10.0);
        assert_eq!(text.find_opts(&format!("中{c}{c}中"), true, false), vec![0..4], "U+{:04X}", c as u32);
    }
    let spaced = ['\u{AC00}', '\u{3200}', '\u{3260}', '\u{FF10}', '\u{FF21}', '\u{FF41}', '\u{FFA1}', '\u{A000}', 'a'];
    for c in spaced {
        let text = laid_out(&[(10.0, 10.0, &format!("中{c}")), (10.0, 25.0, &format!("{c}中"))], 10.0);
        assert!(text.find_opts(&format!("中{c}{c}中"), true, false).is_empty(), "U+{:04X}", c as u32);
        assert_eq!(text.find_opts(&format!("中{c} {c}中"), true, false), vec![0..4], "U+{:04X}", c as u32);
    }
}

/// The two line-end rules meet: a soft hyphen after a Chinese or Japanese character, a line of
/// Chinese that runs on, a German word divided by a soft hyphen, and a line that opens with a zero
/// width space, all on one page. Each break reads as nothing, and plain Latin line ends stay spaces.
#[test]
fn a_soft_hyphen_and_a_chinese_or_japanese_line_end_combine() {
    let text = laid_out(
        &[
            (10.0, 10.0, "装置の検\u{AD}"),
            (10.0, 25.0, "知機構の"),
            (10.0, 40.0, "ソレ"),
            (10.0, 55.0, "\u{200B}ノイド"),
            (10.0, 70.0, "Mitar\u{AD}"),
            (10.0, 85.0, "beiterinnen"),
            (10.0, 100.0, "abc"),
            (10.0, 115.0, "def"),
        ],
        10.0,
    );
    // Both rules fire at the first break: one hit, highlighted on both lines.
    let hits = text.find("検知機構");
    assert_eq!(hits, vec![3..8]);
    assert_eq!(text.line_rects(hits[0].clone()).len(), 2);
    assert!(text.find("検 知機構").is_empty());
    // A line of Chinese or Japanese with no soft hyphen, and one that opens with a zero width space.
    assert_eq!(text.find("機構のソレノイド"), vec![6..15]);
    // A soft hyphen between Latin letters.
    assert_eq!(text.find_opts("Mitarbeiterinnen", true, true), vec![15..32]);
    // Latin with neither: still a word break.
    assert_eq!(text.find("abc def"), vec![32..38]);
    assert!(text.find("abcdef").is_empty());
}
