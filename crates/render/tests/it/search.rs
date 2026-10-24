use pdfcraft_render::{PageText, TextGlyph, text::fold_case};
use std::ops::Range;

fn page(parts: &[&str]) -> PageText {
    PageText {
        glyphs: parts.iter().map(|text| TextGlyph { text: (*text).into(), rect: [0.0; 4], quad: [[0.0; 2]; 4], direction: [1.0, 0.0] }).collect(),
        line_of: vec![0; parts.len()],
        space_before: vec![false; parts.len()],
    }
}

// The original straightforward search is an independent oracle for valid page metadata, extended
// with ligature letters and the three readings of a line-end hyphen between letters: as written,
// joined (no hyphen, no break) and kept (hyphen, no break).
fn reference(page: &PageText, query: &str, sensitive: bool, whole: bool, limit: usize) -> Vec<Range<usize>> {
    let letters = |s: &str| {
        s.replace('\u{FB00}', "ff")
            .replace('\u{FB01}', "fi")
            .replace('\u{FB02}', "fl")
            .replace('\u{FB03}', "ffi")
            .replace('\u{FB04}', "ffl")
            .replace(['\u{FB05}', '\u{FB06}'], "st")
    };
    let fold = |s: &str| if sensitive { letters(s) } else { fold_case(&letters(s)) };
    let needle: Vec<char> = fold(query).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    if needle.is_empty() || limit == 0 {
        return Vec::new();
    }
    let texts: Vec<String> = page.glyphs.iter().map(|g| fold(&g.text)).collect();
    let n = texts.len();
    let new_line = |i: usize| i > 0 && page.line_of[i] != page.line_of[i - 1];
    let hyphen = |c: char| matches!(c, '-' | '\u{AD}' | '\u{2010}');
    let divided: Vec<bool> = (0..n)
        .map(|i| {
            let chars: Vec<char> = texts[i].chars().collect();
            let Some(&last) = chars.last() else { return false };
            let before = if chars.len() >= 2 {
                Some(chars[chars.len() - 2])
            } else if i > 0 && !page.space_before[i] && !new_line(i) {
                texts[i - 1].chars().last()
            } else {
                None
            };
            hyphen(last)
                && before.is_some_and(char::is_alphabetic)
                && i + 1 < n
                && new_line(i + 1)
                && texts[i + 1].chars().next().is_some_and(char::is_alphabetic)
        })
        .collect();
    let mut all = Vec::new();
    let readings = if divided.contains(&true) { 3 } else { 1 };
    for reading in 0..readings {
        let mut chars = Vec::new();
        for i in 0..n {
            let joined = reading > 0 && i > 0 && divided[i - 1];
            if i > 0 && (page.space_before[i] || (new_line(i) && !joined)) && chars.last().is_some_and(|&(c, _)| c != ' ') {
                chars.push((' ', i));
            }
            let mut glyph: Vec<char> = texts[i].chars().collect();
            if reading == 1 && divided[i] {
                glyph.pop();
            }
            for c in glyph {
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
        all.push(hits);
    }
    if readings == 1 {
        return all.remove(0);
    }
    // A hit is left out where it overlaps one kept from another reading; one reading's hits are
    // all kept, though two can share a glyph.
    let mut hits: Vec<(Range<usize>, usize)> = all.into_iter().enumerate().flat_map(|(r, hits)| hits.into_iter().map(move |h| (h, r))).collect();
    hits.sort_by_key(|(h, _)| (h.start, std::cmp::Reverse(h.end)));
    let mut out: Vec<(Range<usize>, usize)> = Vec::new();
    for (h, r) in hits {
        let overlaps = out.iter().any(|(k, kr)| *kr != r && h.start < k.end && k.start < h.end);
        if !overlaps && out.len() < limit {
            out.push((h, r));
        }
    }
    out.into_iter().map(|(h, _)| h).collect()
}

/// One glyph per entry of `text`, each on the line given in `lines`.
fn lined(text: &[&str], lines: &[u32]) -> PageText {
    let mut p = page(text);
    p.line_of = lines.to_vec();
    p
}

#[test]
fn a_ligature_matches_its_letters() {
    let text = page(&["o", "\u{FB03}", "c", "e", " ", "o", "f", "\u{FB01}", "c", "e"]);
    assert_eq!(text.find("office"), vec![0..4, 5..10]);
    assert_eq!(text.find("OFFICE"), vec![0..4, 5..10]);
    assert_eq!(text.find_opts("office", true, true), vec![0..4, 5..10], "case-sensitive and whole words too");
    assert_eq!(text.find("o\u{FB03}ce"), vec![0..4, 5..10], "a ligature in the query reads as its letters too");
    assert_eq!(text.find("fi"), vec![1..2, 7..8], "a match inside a ligature takes its glyph");
    assert!(text.find("o\u{FB13}").is_empty(), "no other compatibility folding");
}

#[test]
fn a_word_divided_by_a_line_end_hyphen_matches_whole_and_as_written() {
    // "exam-" ends line 0, "ple" starts line 1; the hyphen is its own glyph.
    let text = lined(&["e", "x", "a", "m", "-", "p", "l", "e"], &[0, 0, 0, 0, 0, 1, 1, 1]);
    assert_eq!(text.find("example"), vec![0..8], "both parts, and the hyphen between them");
    assert_eq!(text.find_opts("example", false, true), vec![0..8], "a whole word");
    assert_eq!(text.find("exam-ple"), vec![0..8], "a query with the hyphen");
    assert_eq!(text.find("exam- ple"), vec![0..8], "and with the line break as a space");
    assert_eq!(text.find("exam"), vec![0..4]);
    // The hyphen in the glyph of the last letter, a soft hyphen and U+2010.
    for hyphen in ["m-", "m\u{AD}", "m\u{2010}"] {
        let text = lined(&["e", "x", "a", hyphen, "p", "l", "e"], &[0, 0, 0, 0, 1, 1, 1]);
        assert_eq!(text.find("example"), vec![0..7], "{hyphen:?}");
    }
    // Not divided: a hyphen inside a line, a dash after a word gap, a hyphen between digits.
    assert!(page(&["e", "x", "a", "m", "-", "p", "l", "e"]).find("example").is_empty(), "inside a line");
    let mut spaced = lined(&["e", "x", "a", "m", "-", "p", "l", "e"], &[0, 0, 0, 0, 0, 1, 1, 1]);
    spaced.space_before[4] = true;
    assert!(spaced.find("example").is_empty(), "a dash after a word gap");
    assert!(spaced.find("exam ple").is_empty(), "a dash after a word gap is never dropped");
    assert!(lined(&["1", "0", "-", "2", "0"], &[0, 0, 0, 1, 1]).find("1020").is_empty(), "between digits");
    assert!(lined(&["a", "-", "2", "0"], &[0, 0, 1, 1]).find("a20").is_empty(), "before a number");
    assert!(lined(&["1", "0", "-", "a", "b"], &[0, 0, 0, 1, 1]).find("10ab").is_empty(), "after a number");
    assert!(lined(&["a", "-"], &[0, 0]).find("a-").len() == 1, "at the end of the page");
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
    // So does a page with a word divided at a line end, which is searched in three readings.
    let divided = lined(&["\u{FB03}", " ", "exam", "-", "ple"], &[0, 0, 0, 0, 1]);
    assert_eq!(divided.find("f"), vec![0..1, 0..1]);
    assert_eq!(divided.find("example"), vec![2..5]);
}

#[test]
fn search_matches_reference_for_unicode_gaps_limits_and_word_boundaries() {
    let alphabet = ["a", "b", " ", "-", "İ", "ı", "ς", "Σ", "\t", "\n", "ffi", "", "界", "e\u{301}", "العربية", "\u{FB01}", "b\u{AD}", "1"];
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
