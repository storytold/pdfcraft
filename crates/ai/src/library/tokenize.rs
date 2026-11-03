//! Library tokens: NFKC, lowercase, Unicode word boundaries (UAX #29), and Chinese, Japanese and
//! Korean as each character plus each overlapping pair, since those scripts don't separate words
//! with spaces: pairs rank a word match, single characters let a one-character query match.

use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// Longer tokens (a hash, a run of base64) are left out: nobody searches for them.
pub const MAX_TOKEN_BYTES: usize = 64;

/// Append the tokens of `text` to `out`.
pub fn tokens(text: &str, out: &mut Vec<String>) {
    let normalized = text.nfkc().collect::<String>().to_lowercase();
    let mut cjk: Vec<char> = Vec::new();
    for segment in normalized.split_word_bounds() {
        if !segment.is_empty() && segment.chars().all(is_cjk) {
            // UAX #29 gives each ideograph its own segment: join adjacent ones into one run.
            cjk.extend(segment.chars());
            continue;
        }
        flush_cjk(&mut cjk, out);
        if !segment.chars().any(char::is_alphanumeric) {
            continue;
        }
        // A word that mixes scripts: CJK stretches become bigrams, the rest stays one token.
        let mut word = String::new();
        for c in segment.chars() {
            if is_cjk(c) {
                push_word(&mut word, out);
                cjk.push(c);
            } else {
                flush_cjk(&mut cjk, out);
                word.push(c);
            }
        }
        push_word(&mut word, out);
        flush_cjk(&mut cjk, out);
    }
    flush_cjk(&mut cjk, out);
}

fn push_word(word: &mut String, out: &mut Vec<String>) {
    let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
    if !trimmed.is_empty() && trimmed.len() <= MAX_TOKEN_BYTES {
        out.push(trimmed.to_owned());
    }
    word.clear();
}

fn flush_cjk(run: &mut Vec<char>, out: &mut Vec<String>) {
    for (i, c) in run.iter().enumerate() {
        out.push(c.to_string());
        if let Some(next) = run.get(i + 1) {
            out.push([*c, *next].iter().collect());
        }
    }
    run.clear();
}

/// Han, Hiragana, Katakana and Hangul.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF      // Hangul Jamo
        | 0x3040..=0x30FF    // Hiragana, Katakana
        | 0x3130..=0x318F    // Hangul Compatibility Jamo
        | 0x31F0..=0x31FF    // Katakana Phonetic Extensions
        | 0x3400..=0x4DBF    // CJK Extension A
        | 0x4E00..=0x9FFF    // CJK Unified Ideographs
        | 0xAC00..=0xD7AF    // Hangul Syllables
        | 0xF900..=0xFAFF    // CJK Compatibility Ideographs
        | 0x20000..=0x3134F  // CJK Extensions B–G
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        tokens(s, &mut out);
        out
    }

    #[test]
    fn words_are_normalized() {
        assert_eq!(toks("The PDF's Ｆｕｌｌｗｉｄｔｈ ﬁle, café — 3.14!"), ["the", "pdf's", "fullwidth", "file", "café", "3.14"]);
        assert_eq!(toks("Straße STRASSE"), ["straße", "strasse"]);
        assert_eq!(toks("  ...  "), Vec::<String>::new());
        assert!(toks(&"a".repeat(MAX_TOKEN_BYTES + 1)).is_empty());
    }

    #[test]
    fn cjk_becomes_characters_and_pairs() {
        assert_eq!(toks("東京都"), ["東", "東京", "京", "京都", "都"]);
        assert_eq!(toks("日本語のPDF"), ["日", "日本", "本", "本語", "語", "語の", "の", "pdf"]);
        assert_eq!(toks("中。国"), ["中", "国"]);
        assert_eq!(toks("한국어 문서"), ["한", "한국", "국", "국어", "어", "문", "문서", "서"]);
        // Halfwidth katakana is folded to fullwidth by NFKC first.
        assert_eq!(toks("ｶﾀｶﾅ"), toks("カタカナ"));
    }

    proptest::proptest! {
        #[test]
        fn tokens_are_total(s in "\\PC*") {
            for t in toks(&s) {
                proptest::prop_assert!(!t.is_empty() && t.len() <= MAX_TOKEN_BYTES);
            }
        }
    }
}
