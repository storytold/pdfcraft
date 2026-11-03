//! Page text split into passages of about [`TARGET_CHARS`] characters on sentence boundaries,
//! each starting with the last sentence of the one before, so a fact that straddles two passages
//! is whole in one of them. A last sentence longer than half a passage ([`MAX_OVERLAP_CHARS`]) is
//! not repeated: it is a passage's worth of text on its own, and repeating it would double it.

use unicode_segmentation::UnicodeSegmentation;

/// The size a passage grows to before a new one starts.
pub const TARGET_CHARS: usize = 1_000;
/// A single sentence longer than this is cut, at a space where there is one.
pub const MAX_SENTENCE_CHARS: usize = 2_000;
/// The longest last sentence repeated at the start of the next passage.
pub const MAX_OVERLAP_CHARS: usize = TARGET_CHARS / 2;

pub fn split(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    for sentence in text.split_sentence_bounds() {
        cut_long(sentence, &mut pieces);
    }
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut chars = 0usize;
    // Sentences in `current` that aren't the overlap carried over from the passage before.
    let mut fresh = 0usize;
    for piece in pieces {
        let n = piece.chars().count();
        if piece.trim().is_empty() {
            current.push(piece);
            chars = chars.saturating_add(n);
            continue;
        }
        if fresh > 0 && chars.saturating_add(n) > TARGET_CHARS {
            emit(&current, &mut out);
            let overlap = current.iter().rev().find(|s| !s.trim().is_empty()).copied();
            current.clear();
            chars = 0;
            fresh = 0;
            if let Some(last) = overlap {
                let len = last.chars().count();
                if len <= MAX_OVERLAP_CHARS {
                    current.push(last);
                    chars = len;
                }
            }
        }
        current.push(piece);
        chars = chars.saturating_add(n);
        fresh += 1;
    }
    if fresh > 0 {
        emit(&current, &mut out);
    }
    out
}

fn emit(parts: &[&str], out: &mut Vec<String>) {
    let text = parts.concat();
    let text = text.trim();
    if !text.is_empty() {
        out.push(text.to_owned());
    }
}

/// Push `s` as pieces of at most [`MAX_SENTENCE_CHARS`] characters.
fn cut_long<'a>(mut s: &'a str, out: &mut Vec<&'a str>) {
    while let Some((limit, _)) = s.char_indices().nth(MAX_SENTENCE_CHARS) {
        let head = s.get(..limit).unwrap_or(s);
        // Cut after the last space in the window, or at the limit when there is none.
        let cut = match head.rfind(char::is_whitespace) {
            Some(i) if i > 0 => head.get(i..).and_then(|rest| rest.chars().next()).map_or(limit, |c| i + c.len_utf8()),
            _ => limit,
        };
        let (piece, rest) = s.split_at_checked(cut).unwrap_or((s, ""));
        out.push(piece);
        s = rest;
    }
    if !s.is_empty() {
        out.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_passage() {
        assert_eq!(split("  One. Two.  "), ["One. Two."]);
        assert!(split(" \n\t ").is_empty());
        assert!(split("").is_empty());
    }

    #[test]
    fn passages_overlap_by_one_sentence() {
        let sentence = |i: usize| format!("Sentence number {i} says something about the topic at some length here. ");
        let text: String = (0..40).map(sentence).collect();
        let passages = split(&text);
        assert!(passages.len() > 2, "{passages:?}");
        for p in &passages {
            assert!(p.chars().count() <= TARGET_CHARS + sentence(0).len(), "{}", p.len());
        }
        for pair in passages.windows(2) {
            let last = pair[0].rsplit(". ").next().unwrap_or_default();
            assert!(pair[1].starts_with(last.trim_end_matches('.')), "{:?} then {:?}", pair[0], pair[1]);
        }
        // Every sentence is in some passage.
        for i in 0..40 {
            assert!(passages.iter().any(|p| p.contains(&format!("number {i} "))), "{i}");
        }
    }

    #[test]
    fn a_long_last_sentence_is_not_repeated() {
        let short = |i: usize| format!("Short sentence {i} about the topic. ");
        let long = format!("{}. ", "Long ".repeat(120).trim_end());
        assert!(long.chars().count() > MAX_OVERLAP_CHARS && long.chars().count() < TARGET_CHARS);
        let text = format!("{}{long}{}", (0..10).map(short).collect::<String>(), (10..40).map(short).collect::<String>());
        let passages = split(&text);
        let holding: Vec<&String> = passages.iter().filter(|p| p.contains("Long Long")).collect();
        assert_eq!(holding.len(), 1, "{passages:?}");
        // The sentence after it still overlaps as usual.
        assert!(passages.windows(2).any(|w| w[0].contains("Long Long") && !w[1].contains("Long Long")));
    }

    #[test]
    fn a_huge_sentence_is_cut() {
        let words = "word ".repeat(MAX_SENTENCE_CHARS);
        let passages = split(&words);
        assert!(passages.len() >= 2);
        assert!(passages.iter().all(|p| p.chars().count() <= MAX_SENTENCE_CHARS));
        let blob = "日".repeat(MAX_SENTENCE_CHARS * 2 + 7);
        let passages = split(&blob);
        assert_eq!(passages.iter().map(|p| p.chars().count()).sum::<usize>(), MAX_SENTENCE_CHARS * 2 + 7);
    }

    proptest::proptest! {
        #[test]
        fn split_is_total(s in "\\PC{0,3000}") {
            for p in split(&s) {
                proptest::prop_assert!(!p.trim().is_empty());
                proptest::prop_assert!(p.chars().count() <= MAX_SENTENCE_CHARS + TARGET_CHARS);
            }
        }
    }
}
