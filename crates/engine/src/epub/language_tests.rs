//! The document's /Lang reaches the book only when it is a language tag.
use super::language_tag;

#[test]
fn real_language_tags_are_kept() {
    for tag in [
        "en",
        "en-US",
        "es-419",
        "zh-Hant-TW",
        "fil",
        "en-a-bbb",
        "en-a-bbb-x-a-ccc",
        "de-CH-1901",
        "sl-rozaj-biske",
        "zh-yue-HK",
        "x-private",
        "en-US-u-islamcal",
        "ja-Latn-JP-u-ca-japanese-x-hte",
        "i-klingon",
        "I-Klingon",
        "en-GB-oed",
        "zh-min-nan",
        "art-lojban",
        "sgn-BE-FR",
        "de-DE-u-co-phonebk",
        "en-t-es-t0-hybrid",
        "hy-Latn-IT-arevela",
    ] {
        assert!(language_tag(tag), "{tag}");
    }
}

#[test]
fn text_that_is_not_a_language_tag_is_dropped() {
    for tag in [
        "x",
        "en-1",
        "-",
        "en-",
        "-US",
        "en--US",
        "1en",
        "en-toolongsubtag",
        "e n",
        "e1",
        "x",
        "x-",
        "en-a",
        "en-a-",
        "en-x",
        "en-US-x",
        "en-a-b",
        "i",
        "i-nonsuch",
        "en-a-bbb-",
        "abcdefghi",
        "en-x-toolongprivate",
        "en-US-u-ca-x",
    ] {
        assert!(!language_tag(tag), "{tag}");
    }
}
