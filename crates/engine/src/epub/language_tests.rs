//! The document's /Lang reaches the book only when it is a language tag.
use super::language_tag;

#[test]
fn real_language_tags_are_kept() {
    for tag in ["en", "en-US", "es-419", "zh-Hant-TW", "fil"] {
        assert!(language_tag(tag), "{tag}");
    }
}

#[test]
fn text_that_is_not_a_language_tag_is_dropped() {
    for tag in ["x", "en-1", "-", "en-", "-US", "en--US", "1en", "en-toolongsubtag", "e n", "e1"] {
        assert!(!language_tag(tag), "{tag}");
    }
}
