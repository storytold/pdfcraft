use pdfcraft_render::{PageText, TextGlyph, text::fold_case};
use std::ops::Range;

fn page(parts: &[&str]) -> PageText {
    PageText {
        glyphs: parts.iter().map(|text| TextGlyph { text: (*text).into(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] }).collect(),
        line_of: vec![0; parts.len()],
        space_before: vec![false; parts.len()],
    }
}

fn is_hyphen(c: char) -> bool {
    matches!(c, '-' | '\u{00AD}' | '\u{2010}' | '\u{2011}')
}

fn collect_ref_chars(page: &PageText, fold: &impl Fn(&str) -> String, dehyphenate: bool) -> (Vec<(char, usize)>, bool) {
    let mut chars = Vec::new();
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
        if i > 0 && gap && chars.last().is_some_and(|&(c, _)| c != ' ') {
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
            if c != ' ' || chars.last().is_none_or(|&(last, _)| last != ' ') {
                chars.push((c, i));
            }
        }
    }
    (chars, has_hyphen_break)
}

fn scan(chars: &[(char, usize)], needle: &[char], whole: bool, limit: usize) -> Vec<Range<usize>> {
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

// The original straightforward search is an independent oracle for valid page metadata.
fn reference(page: &PageText, query: &str, sensitive: bool, whole: bool, limit: usize) -> Vec<Range<usize>> {
    let fold = |s: &str| if sensitive { s.to_owned() } else { fold_case(s) };
    let needle: Vec<char> = fold(query).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() || limit == 0 {
        return Vec::new();
    }
    let (chars_dehyphenated, has_hyphen_break) = collect_ref_chars(page, &fold, true);
    let mut hits = scan(&chars_dehyphenated, &needle, whole, limit);
    if hits.is_empty() && has_hyphen_break {
        let (chars_preserved, _) = collect_ref_chars(page, &fold, false);
        hits = scan(&chars_preserved, &needle, whole, limit);
    }
    hits
}

#[test]
fn search_matches_words_hyphenated_across_line_breaks() {
    let text = PageText {
        glyphs: vec![
            TextGlyph { text: "docu-".into(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] },
            TextGlyph { text: "ment".into(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] },
        ],
        line_of: vec![0, 1],
        space_before: vec![false, false],
    };
    assert_eq!(text.find("document"), vec![0..2]);
    assert_eq!(text.find_opts("document", false, true), vec![0..2]);
    assert_eq!(text.find("docu-ment"), vec![0..2]);

    let text2 = PageText {
        glyphs: ["d", "o", "c", "u", "-", "m", "e", "n", "t"]
            .iter()
            .map(|t| TextGlyph { text: (*t).into(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] })
            .collect(),
        line_of: vec![0, 0, 0, 0, 0, 1, 1, 1, 1],
        space_before: vec![false; 9],
    };
    assert_eq!(text2.find("document"), vec![0..9]);
    assert_eq!(text2.find_opts("document", false, true), vec![0..9]);
    assert_eq!(text2.find("docu-ment"), vec![0..9]);

    let text3 = PageText {
        glyphs: ["d", "o", "c", "u", "\u{2010}", "m", "e", "n", "t"]
            .iter()
            .map(|t| TextGlyph { text: (*t).into(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] })
            .collect(),
        line_of: vec![0, 0, 0, 0, 0, 1, 1, 1, 1],
        space_before: vec![false; 9],
    };
    assert_eq!(text3.find("document"), vec![0..9]);
    assert_eq!(text3.find_opts("document", false, true), vec![0..9]);
    assert_eq!(text3.find("docu\u{2010}ment"), vec![0..9]);
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
