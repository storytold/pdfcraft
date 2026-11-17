use super::*;

fn english() -> Dictionary {
    Dictionary::from_word_list("# test list\nthe\ncat\nsat\non\nmat\ndon't\nParis\niPhone\nNASA\n\nplease\ncheck\nthis\n")
}

#[test]
fn a_misspelled_word_is_found_with_its_character_range_and_a_suggestion() {
    let found = english().check("The cat sta on the mat");
    assert_eq!(found, vec![Misspelling { word: "sta".into(), start: 8, end: 11, suggestions: vec!["sat".into()] }]);
}

#[test]
fn ranges_count_characters_not_bytes() {
    let found = english().check("Ünï the caat");
    assert_eq!(found.iter().map(|m| (m.word.as_str(), m.start, m.end)).collect::<Vec<_>>(), vec![("Ünï", 0, 3), ("caat", 8, 12)]);
}

#[test]
fn case_follows_how_a_word_is_listed() {
    let d = english();
    assert!(d.knows("cat") && d.knows("Cat") && d.knows("CAT"));
    assert!(d.knows("Paris") && d.knows("PARIS") && !d.knows("paris"));
    assert!(d.knows("iPhone") && !d.knows("Iphone") && !d.knows("iphone"));
    assert!(d.knows("NASA") && !d.knows("Nasa"));
}

#[test]
fn apostrophes_join_letters_and_quotes_do_not() {
    let d = english();
    assert!(d.check("don\u{2019}t 'cat' the").is_empty());
    assert_eq!(d.check("dont").len(), 1);
}

#[test]
fn text_that_is_not_prose_is_left_alone() {
    let d = english();
    assert!(d.check("the https://example.org/zzz on zzq@example.org, item7 b2b www.zzz.org x").is_empty());
}

#[test]
fn a_user_word_makes_a_word_right() {
    let mut d = english();
    assert_eq!(d.check("check Zorblax").len(), 1);
    assert!(d.add_word("Zorblax"));
    assert!(d.check("check Zorblax").is_empty());
    assert!(!d.add_word("two words") && !d.add_word("") && !d.add_word("abc1") && !d.add_word(&"a".repeat(MAX_WORD_CHARS + 1)));
}

#[test]
fn an_empty_dictionary_claims_nothing() {
    assert!(Dictionary::new().check("anything at all").is_empty());
}

#[test]
fn suggestions_keep_the_case_and_are_bounded() {
    let d = english();
    assert_eq!(d.suggest("Caat"), vec!["Cat".to_string()]);
    assert_eq!(d.suggest("THEE"), vec!["THE".to_string()]);
    assert!(d.suggest(&"x".repeat(MAX_WORD_CHARS + 1)).is_empty());
    assert!(d.suggest("").is_empty());
}

#[test]
fn hostile_text_does_not_panic() {
    let d = english();
    let text = "\u{0}\u{feff}'''\u{2019}a\u{2019}\u{2019}b é\u{301}\u{301} 𝔘𝔫𝔦 ".repeat(1000);
    let _ = d.check(&text);
    let _ = d.check(&"x".repeat(100_000));
}
