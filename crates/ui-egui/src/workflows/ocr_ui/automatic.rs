//! Conservative text-shape check for automatic Edit OCR, never for explicit recognition.

/// Minimum share of alphanumeric characters in words with a run of at least three letters or
/// numeric characters. Calibrated for #6311 with the attributed ocrs models: 34 synthetic non-text
/// images and 12 text scans. The largest negative Q was 0.50; the mixed paragraph scored 0.745,
/// and a single HELLO or 2024 scored 1.0. This is a heuristic, not a text classifier: a lone false
/// three-letter token also scores 1.0. Short-word-only scans and short decimals need explicit OCR;
/// sideways scans also failed the existing model without orientation preprocessing.
pub(super) const MIN_TEXT_SHARE: f64 = 0.60;

/// Q: the share of all alphanumeric characters belonging to OCR words with an alphabetic or
/// numeric run of length >= 3. Words are considered independently, using Unicode predicates;
/// punctuation breaks a run. An empty/no-alphanumeric result returns None: there is nothing to
/// judge, so the caller keeps the existing no-result behavior. Counting allocates nothing and
/// saturates rather than overflowing even for hostile recognition output.
pub(super) fn text_share<'a>(words: impl IntoIterator<Item = &'a str>) -> Option<f64> {
    let (mut total, mut eligible) = (0_u64, 0_u64);
    for word in words {
        let (mut letters, mut numbers, mut count) = (0_u8, 0_u8, 0_u64);
        let mut qualifies = false;
        for c in word.chars() {
            letters = if c.is_alphabetic() { letters.saturating_add(1).min(3) } else { 0 };
            numbers = if c.is_numeric() { numbers.saturating_add(1).min(3) } else { 0 };
            qualifies |= letters == 3 || numbers == 3;
            if c.is_alphanumeric() {
                count = count.saturating_add(1);
            }
        }
        total = total.saturating_add(count);
        if qualifies {
            eligible = eligible.saturating_add(count);
        }
    }
    if total == 0 { None } else { Some(eligible as f64 / total as f64) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrated_token_lists_separate_texture_from_longer_text() {
        assert_eq!(text_share(["T", "-", "5S", "T", "n", "M", "7", "T"]), Some(0.0));
        // Rectangle seed 3: letter-only share was 14/27 = 0.519; Q includes its digits too.
        let texture = ["I", "LUTCHIA", "5", "1", "?", "TE", "5", "FFAM", "T", "S7", "M", "T", "E", "1", "ILT", "A--R", "UI", "T", "-", "1"];
        assert!((text_share(texture).unwrap() - 14.0 / 33.0).abs() < 1e-10);
        assert!(text_share(texture).unwrap() < MIN_TEXT_SHARE);
        let paragraph = ["Room", "2024", "is", "OK.", "Hi", "Sam,", "meet", "at", "15.3", "degrees.", "The", "total", "is", "125", "items."];
        assert!((text_share(paragraph).unwrap() - 38.0 / 51.0).abs() < 1e-10);
        assert!(text_share(paragraph).unwrap() >= MIN_TEXT_SHARE);
        assert_eq!(text_share(["2024"]), Some(1.0));
        assert_eq!(text_share(["HELLO"]), Some(1.0));
        assert_eq!(text_share(["OK", "Hi", "15.3"]), Some(0.0));
        // Deliberate limitation: a single false long token cannot be separated by token shape.
        assert_eq!(text_share(["CAT"]), Some(1.0));
    }

    #[test]
    fn scoring_is_per_word_unicode_aware_and_total() {
        assert_eq!(text_share([]), None);
        assert_eq!(text_share(["", "-", "?!"]), None);
        assert_eq!(text_share(["a", "b", "c", "12", "34"]), Some(0.0));
        assert_eq!(text_share(["世界你好", "١٢٣"]), Some(1.0));
        assert_eq!(text_share(["ab-c", "12.3"]), Some(0.0));
        assert_eq!(text_share(["abc", "x", "y"]), Some(MIN_TEXT_SHARE), "inclusive threshold");
    }
}
