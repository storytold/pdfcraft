use super::*;

fn page(text: &str) -> PageText {
    let glyphs: Vec<_> = text.chars().enumerate().map(|(i, c)| g(&c.to_string(), i as f32 * 6.0, 10.0, i as f32 * 6.0 + 6.0)).collect();
    let len = glyphs.len();
    PageText { truncated: false, glyphs, line_of: vec![0; len], space_before: vec![false; len] }
}

fn options(sensitive: bool, whole: bool) -> FindOptions {
    FindOptions { case_sensitive: sensitive, whole_words: whole, language: SearchLanguage::Turkish }
}

fn one(range: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    std::iter::once(range).collect()
}

#[test]
fn turkish_search_keeps_dotted_and_dotless_pairs_separate_and_general_unchanged() {
    let p = page("I ı İ i");
    assert_eq!(p.find_with_options("i", options(false, true), 10), [4..5, 6..7]);
    assert_eq!(p.find_with_options("ı", options(false, true), 10), [0..1, 2..3]);
    assert_eq!(p.find_with_options("İ", options(true, true), 10), one(4..5));
    assert_eq!(p.find("i"), [0..1, 2..3, 4..5, 6..7], "general mode remains compatible");
    assert_eq!(p.find_with_options("i", FindOptions::default(), 10), p.find("i"));
}

#[test]
fn turkish_search_matches_words_in_both_case_directions() {
    let p = page("IĞDIR ığdır İSTANBUL istanbul ISPARTA isparta");
    for (query, expected) in
        [("ığdır", vec!["IĞDIR", "ığdır"]), ("İSTANBUL", vec!["İSTANBUL", "istanbul"]), ("ısparta", vec!["ISPARTA"]), ("isparta", vec!["isparta"])]
    {
        let hits = p.find_with_options(query, options(false, true), 10);
        assert_eq!(hits.into_iter().map(|r| p.text_of(r)).collect::<Vec<_>>(), expected, "{query}");
    }
}

#[test]
fn turkish_search_composes_across_glyphs_but_preserves_the_original_text() {
    let source = "I\u{307}STANBUL S\u{327}is\u{327}li G\u{306} U\u{308}";
    let p = page(source);
    let before = p.clone();
    let first = p.find_with_options("istanbul", options(false, true), 10);
    assert_eq!(first, one(0..9), "include the separate dot-above glyph");
    assert_eq!(p.text_of(first[0].clone()), "I\u{307}STANBUL");
    for query in ["Şişli", "ğ", "ü"] {
        assert_eq!(p.find_with_options(query, options(false, true), 10).len(), 1, "{query}");
    }
    assert_eq!(p.find_with_options("İSTANBUL", options(true, true), 10), one(0..9));
    assert_eq!(p, before, "normalization must never rewrite glyph text or geometry");
    assert_eq!(p.plain_text(), source);
}

#[test]
fn turkish_search_reorders_marks_and_retains_every_original_owner() {
    let p = page("I\u{307}\u{323}");
    assert_eq!(p.find_with_options("ị", options(false, true), 10), one(0..3));
    let p = page("A\u{301}\u{323}");
    assert_eq!(p.find_with_options("Ạ\u{301}", options(true, true), 10), one(0..3));
    let p = page("가");
    assert_eq!(p.find_with_options("가", options(true, true), 10), one(0..2));
}

#[test]
fn turkish_search_does_not_drop_accents_or_cross_a_blocking_mark() {
    let p = page("I I\u{307} I\u{301}\u{307} ı i i\u{307} Çağrı ①");
    let hits = p.find_with_options("i", options(false, true), 20);
    assert_eq!(hits.into_iter().map(|r| p.text_of(r)).collect::<Vec<_>>(), ["I\u{307}", "i"]);
    let hits = p.find_with_options("ı", options(false, true), 20);
    assert_eq!(hits.into_iter().map(|r| p.text_of(r)).collect::<Vec<_>>(), ["I", "ı"]);
    assert!(p.find_with_options("Cagri", options(false, true), 10).is_empty());
    assert!(p.find_with_options("1", options(false, true), 10).is_empty(), "canonical, not compatibility normalization");
}

#[test]
fn turkish_search_handles_ligatures_limits_and_long_combining_runs() {
    let mut p = PageText {
        truncated: false,
        glyphs: vec![g("I\u{307}", 0.0, 0.0, 6.0), g("STANBUL", 6.0, 0.0, 48.0)],
        line_of: vec![0, 0],
        space_before: vec![false, false],
    };
    assert_eq!(p.find_with_options("istanbul", options(false, true), 1), one(0..2));
    assert!(p.find_with_options("istanbul", options(false, true), 0).is_empty());
    p = page(&format!("I{}\u{307}", "\u{323}".repeat(10_000)));
    let source = p.plain_text();
    let query = format!("i{}", "\u{323}".repeat(10_000));
    assert_eq!(p.find_with_options(&query, options(false, true), 1), one(0..10_002));
    assert_eq!(p.plain_text(), source);
}
