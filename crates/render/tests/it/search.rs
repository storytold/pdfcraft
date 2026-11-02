use pdfcraft_render::{
    PageText, TextGlyph,
    text::{FindOptions, fold_accents, fold_case},
};
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

/// A single expected match, glyphs `start..end`.
fn one(start: usize, end: usize) -> Vec<Range<usize>> {
    std::iter::once(start..end).collect()
}

fn matches(p: &PageText, query: &str, options: FindOptions) -> Vec<Range<usize>> {
    p.find_options(query, options, usize::MAX)
}

#[test]
fn accents_come_off_latin_letters_and_nothing_else() {
    for (text, want) in [
        ("café", "cafe"),
        ("Crème Brûlée", "Creme Brulee"),
        ("ăâîșț ĂÂÎȘȚ", "aaist AAIST"),
        ("šşś žźż çćč ñň", "sss zzz ccc nn"),
        ("Việt Nam, ế ề ể ễ ệ", "Viet Nam, e e e e e"),
        ("über Résumé naïve", "uber Resume naive"),
        // Written as a letter and a separate mark.
        ("cafe\u{301} Cre\u{300}me", "cafe Creme"),
        ("a\u{301}\u{302}", "a"),
    ] {
        assert_eq!(fold_accents(text), want, "{text:?}");
    }
    // Not accents: letters of their own, other scripts, and marks that belong to no Latin letter.
    for same in ["ł ø đ ħ ß æ œ", "\u{438}\u{306} й ё", "が か ガ", "ά έ ή", "한국어", "\u{301}", "1 + 2 = 3", ""] {
        assert_eq!(fold_accents(same), same, "{same:?}");
    }
    // The Turkish dotted capital is handled by fold_case first, as Find does it.
    assert_eq!(fold_accents(&fold_case("İstanbul ırmak")), "istanbul irmak");
}

#[test]
fn find_ignores_accents_by_default_and_matches_them_when_asked() {
    let p = page(&["Un café ", "très ", "cher, ", "la cafeteria"]);
    let default = FindOptions::default();
    // `cafe` finds café and the start of cafeteria; whole words only finds the first.
    assert_eq!(matches(&p, "cafe", default), [0..1, 3..4]);
    assert_eq!(matches(&p, "cafe", FindOptions { whole_words: true, ..default }), one(0, 1));
    // The accent can be typed too: `café` finds `cafe` as well.
    assert_eq!(matches(&p, "café", default), [0..1, 3..4]);
    assert_eq!(matches(&p, "tres", default), one(1, 2));
    // Accent-sensitive: the letters must match as written.
    let exact = FindOptions { accent_sensitive: true, ..default };
    assert_eq!(matches(&p, "cafe", exact), one(3, 4));
    assert_eq!(matches(&p, "café", exact), one(0, 1));
    assert!(matches(&p, "tres", exact).is_empty());
    // Case and accents are independent: case-sensitive still ignores accents.
    let cs = FindOptions { case_sensitive: true, ..default };
    assert_eq!(matches(&p, "Cafe", cs), Vec::<Range<usize>>::new());
    assert_eq!(matches(&p, "cafe", cs), [0..1, 3..4]);
    let q = page(&["Café"]);
    assert_eq!(matches(&q, "Cafe", cs), one(0, 1));
    assert!(matches(&q, "cafe", cs).is_empty());
    // The older entry points still match accents as written.
    assert!(p.find("cafe").len() == 1 && p.find_opts("cafe", false, false).len() == 1);
}

#[test]
fn a_match_over_a_dropped_accent_covers_the_glyphs_it_came_from() {
    // The accent is a glyph of its own, in the middle of the word and at its end.
    let p = page(&["caf", "e", "\u{301}", "s ", "ok"]);
    assert_eq!(matches(&p, "cafes", FindOptions::default()), one(0, 4), "the mark glyph is inside the match");
    assert_eq!(matches(&p, "cafe", FindOptions::default()), one(0, 3), "a match that ends on the letter takes its accent glyph too");
    // Typed with the accent, matched exactly, the mark is part of the text and does not match.
    assert!(matches(&p, "cafes", FindOptions { accent_sensitive: true, ..Default::default() }).is_empty());
    // Accents inside one glyph's text, precomposed or not.
    let q = page(&["re\u{301}sume\u{301}", " ", "résumé"]);
    assert_eq!(matches(&q, "resume", FindOptions::default()), [0..1, 2..3]);
    // A word is still a word: `cafe` is not found inside `cafes` with whole words only.
    assert!(matches(&p, "cafe", FindOptions { whole_words: true, ..Default::default() }).is_empty());
}

#[test]
fn ignoring_accents_changes_nothing_on_text_that_has_none() {
    let p = page(&["The quick ", "brown fox ", "jumps over ", "the lazy dog. ", "THE END"]);
    for query in ["the", "THE", "o", "quick brown", "dog. the", "x", "  ", "zz"] {
        for (case_sensitive, whole_words) in [(false, false), (true, false), (false, true), (true, true)] {
            let opts = FindOptions { case_sensitive, whole_words, accent_sensitive: false };
            assert_eq!(matches(&p, query, opts), p.find_opts(query, case_sensitive, whole_words), "{query:?} {case_sensitive} {whole_words}");
        }
    }
}

#[test]
fn an_accent_glyph_does_not_hide_a_line_break_or_a_space_before_it() {
    // The mark starts the next line: it is not the accent of the letter before the break.
    let mut p = page(&["a", "\u{301}", "b"]);
    p.line_of = vec![0, 1, 1];
    assert!(matches(&p, "ab", FindOptions::default()).is_empty(), "a, then a line break, then the rest: not one word");
    assert_eq!(matches(&p, "a", FindOptions { whole_words: true, ..Default::default() }), one(0, 1), "a is a word of its own");
    // The same after a space.
    let mut q = page(&["a", "\u{301}", "b"]);
    q.space_before = vec![false, true, false];
    assert!(matches(&q, "ab", FindOptions::default()).is_empty());
    assert_eq!(matches(&q, "a", FindOptions { whole_words: true, ..Default::default() }), one(0, 1));
    // Where it does follow the letter closely, it belongs to it.
    let r = page(&["a", "\u{301}", "b"]);
    assert_eq!(matches(&r, "ab", FindOptions::default()), one(0, 3));
    // An accent with no letter before it is only a mark, and stays one.
    let lone = page(&["\u{301}", "e"]);
    assert_eq!(matches(&lone, "e", FindOptions::default()), one(1, 2));
    assert!(matches(&lone, "\u{301}e", FindOptions { accent_sensitive: true, ..Default::default() }).len() == 1);
}

#[test]
fn folding_does_not_depend_on_how_decomposed_text_is_split_into_glyphs() {
    // "éx" as e + (accent and x) in the next glyph, as e + accent + x, and written whole: the same
    // text, so the same matches for the plain and the accented query.
    for glyphs in [vec!["e", "\u{301}x"], vec!["e", "\u{301}", "x"], vec!["e\u{301}x"], vec!["\u{e9}x"], vec!["\u{e9}", "x"]] {
        let p = page(&glyphs);
        let all = glyphs.len();
        for query in ["ex", "\u{e9}x", "e\u{301}x"] {
            assert_eq!(matches(&p, query, FindOptions::default()), one(0, all), "{glyphs:?} {query:?}");
        }
        // Accent-sensitive, only a query written the same way matches; precomposed and decomposed
        // text are different characters to it.
        assert!(matches(&p, "ex", FindOptions { accent_sensitive: true, ..Default::default() }).is_empty(), "{glyphs:?}");
    }
}

#[test]
fn a_match_ending_on_a_letter_covers_the_accent_glyphs_after_it() {
    // The text_find result and the highlight come from this range, so it must include the accent.
    let p = page(&["caf", "e", "\u{301}"]);
    assert_eq!(matches(&p, "cafe", FindOptions::default()), one(0, 3));
    assert_eq!(matches(&p, "caf", FindOptions::default()), one(0, 1), "ending earlier does not reach it");
    // Several stacked accent glyphs, and a following word.
    let q = page(&["e", "\u{301}", "\u{302}", " ", "x"]);
    assert_eq!(matches(&q, "e", FindOptions::default()), one(0, 3));
    assert_eq!(matches(&q, "e x", FindOptions::default()), one(0, 5));
}

#[test]
fn a_glyph_holding_an_accent_and_more_text_is_not_taken_to_cover_the_accent() {
    // Ranges are whole glyphs. The accent shares a glyph with the "x", so a match on the "e"
    // stops before it rather than highlighting a letter that was not searched for.
    let p = page(&["e", "\u{301}x"]);
    assert_eq!(matches(&p, "e", FindOptions::default()), one(0, 1));
    assert_eq!(matches(&p, "\u{e9}", FindOptions::default()), one(0, 1));
    assert_eq!(matches(&p, "ex", FindOptions::default()), one(0, 2));
}
