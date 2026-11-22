use super::*;

fn english() -> Dictionary {
    Dictionary::from_word_list("# test list\nthe\ncat\nsat\non\nmat\ndon't\nParis\niPhone\nNASA\n\nplease\ncheck\nthis\n")
}

#[test]
fn a_misspelled_word_is_found_with_its_character_range_and_a_suggestion() {
    let found = english().check("The cat sta on the mat");
    assert!(found.complete);
    assert_eq!(found.misspellings, vec![Misspelling { word: "sta".into(), start: 8, end: 11, suggestions: vec!["sat".into()] }]);
}

#[test]
fn ranges_count_characters_not_bytes() {
    let found = english().check("Ünï the caat");
    assert!(found.complete);
    assert_eq!(found.misspellings.iter().map(|m| (m.word.as_str(), m.start, m.end)).collect::<Vec<_>>(), vec![("Ünï", 0, 3), ("caat", 8, 12)]);
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
    assert!(d.check("don\u{2019}t 'cat' the").misspellings.is_empty());
    assert_eq!(d.check("dont").misspellings.len(), 1);
}

#[test]
fn text_that_is_not_prose_is_left_alone() {
    let d = english();
    assert!(d.check("the https://example.org/zzz on zzq@example.org, item7 b2b www.zzz.org x").misspellings.is_empty());
}

#[test]
fn a_user_word_makes_a_word_right() {
    let mut d = english();
    assert_eq!(d.check("check Zorblax").misspellings.len(), 1);
    assert!(d.add_word("Zorblax"));
    assert!(d.check("check Zorblax").misspellings.is_empty());
    assert!(!d.add_word("two words") && !d.add_word("") && !d.add_word("abc1") && !d.add_word(&"a".repeat(MAX_WORD_CHARS + 1)));
}

#[test]
fn an_empty_dictionary_claims_nothing() {
    assert!(Dictionary::new().check("anything at all").misspellings.is_empty());
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

#[test]
fn mixed_case_dictionary_words_are_suggested() {
    let d = english();
    assert_eq!(d.suggest("iPhon"), vec!["iPhone".to_string()]);
    assert_eq!(d.suggest("IPHON"), vec!["iPhone".to_string()]);
    assert_eq!(d.suggest("iphon"), vec!["iPhone".to_string()]);
    let mut mcdonald = Dictionary::new();
    assert!(mcdonald.add_word("McDonald"));
    assert_eq!(mcdonald.suggest("McDonld"), vec!["McDonald".to_string()]);
    assert_eq!(mcdonald.suggest("MCDONLD"), vec!["McDonald".to_string()]);
    assert_eq!(mcdonald.suggest("mcdonld"), vec!["McDonald".to_string()]);
}

#[test]
fn all_caps_typos_of_capitalised_words_are_suggested_in_capitals() {
    // A capitalised dictionary entry is also right in capitals (`knows`), so suggestions for an
    // all-caps typo follow the typed case when that shaped form is known.
    let mut paris = Dictionary::new();
    assert!(paris.add_word("Paris"));
    assert!(paris.knows("PARIS"));
    assert_eq!(paris.suggest("PARI"), vec!["PARIS".to_string()]);
    assert_eq!(paris.suggest("Pari"), vec!["Paris".to_string()]);
    // Lower-case typed form is not known for a capitalised entry: keep the stored spelling.
    assert_eq!(paris.suggest("pari"), vec!["Paris".to_string()]);

    let mut cat = Dictionary::new();
    assert!(cat.add_word("Cat"));
    assert!(cat.knows("CAT"));
    assert_eq!(cat.suggest("CAAT"), vec!["CAT".to_string()]);
    assert_eq!(cat.suggest("Caat"), vec!["Cat".to_string()]);
}

#[test]
fn lower_case_dictionary_suggestions_keep_typed_case_shaping() {
    let mut d = Dictionary::new();
    assert!(d.add_word("cat"));
    assert!(d.add_word("the"));
    assert_eq!(d.suggest("Caat"), vec!["Cat".to_string()]);
    assert_eq!(d.suggest("THEE"), vec!["THE".to_string()]);
    assert_eq!(d.suggest("CAAT"), vec!["CAT".to_string()]);
}

#[test]
fn text_past_the_character_budget_is_left_unchecked() {
    let d = english();
    // A long run of periods, then a misspelling past MAX_TEXT_CHARS: the misspelling is not reported.
    let text = format!("{}zz", ".".repeat(MAX_TEXT_CHARS));
    let found = d.check(&text);
    assert!(found.misspellings.is_empty(), "{:?}", found.misspellings);
    assert!(!found.complete);
    // A word that starts inside the budget and would cross it is also left out.
    let text = format!("{}hello", " ".repeat(MAX_TEXT_CHARS - 2));
    let found = d.check(&text);
    assert!(found.misspellings.iter().all(|m| m.end <= MAX_TEXT_CHARS), "{:?}", found.misspellings);
    assert!(!found.complete);
    // And a misspelling wholly inside the budget is still found, with complete false when more follows.
    let text = format!("caat{}", " ".repeat(MAX_TEXT_CHARS));
    let found = d.check(&text);
    assert_eq!(found.misspellings[0].word, "caat");
    assert!(!found.complete);
    // A single whitespace-free piece longer than the budget: only the budgeted prefix is examined,
    // words that would cross the limit are omitted, and complete is false.
    let text = format!("{}zz", "a".repeat(MAX_TEXT_CHARS));
    let found = d.check(&text);
    assert!(found.misspellings.iter().all(|m| m.end <= MAX_TEXT_CHARS), "{:?}", found.misspellings);
    assert!(!found.complete);
    // Past the look-ahead window inside one unbroken piece, a later misspelling stays unchecked.
    let text = format!("{}{}", "a".repeat(MAX_TEXT_CHARS + MAX_WORD_CHARS + 1), "zz");
    let found = d.check(&text);
    assert!(found.misspellings.is_empty(), "{:?}", found.misspellings);
    assert!(!found.complete);
}

