//! Bidirectional text helpers (UAX #9).
//!
//! Follows Unicode Bidirectional Algorithm (UAX #9 rules P2 and P3) for paragraph
//! base direction auto-detection.

/// Does the first strong directional character in `text` belong to a right-to-left script?
///
/// Follows UAX #9 rules P2 and P3: characters with strong bidirectional class AL or R
/// (Arabic, Hebrew, Syriac, Thaana…) give `true`; strong L characters (Latin, Greek,
/// Cyrillic, CJK…) give `false`. Neutral, weak, and whitespace characters are skipped.
pub fn first_strong_is_rtl(text: &str) -> bool {
    for c in text.chars() {
        if c == '\u{200F}' || c == '\u{061C}' {
            // RLM and ALM are strong RTL controls, although neither is alphabetic.
            return true;
        }
        if c == '\u{200E}' {
            // Left-to-Right Mark (LRM)
            return false;
        }
        if c.is_alphabetic() {
            return is_rtl_script(c);
        }
    }
    false
}

/// Helper checking if a char belongs to standard RTL Unicode blocks.
pub fn is_rtl_script(c: char) -> bool {
    matches!(
        c,
        '\u{0590}'..='\u{05FF}' // Hebrew
        | '\u{0600}'..='\u{06FF}' // Arabic
        | '\u{0700}'..='\u{074F}' // Syriac
        | '\u{0750}'..='\u{077F}' // Arabic Supplement
        | '\u{0780}'..='\u{07BF}' // Thaana
        | '\u{0870}'..='\u{089F}' // Arabic Extended-B
        | '\u{08A0}'..='\u{08FF}' // Arabic Extended-A
        | '\u{FB1D}'..='\u{FB4F}' // Hebrew Presentation Forms
        | '\u{FB50}'..='\u{FDFF}' // Arabic Presentation Forms-A
        | '\u{FE70}'..='\u{FEFE}' // Arabic Presentation Forms-B
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_arabic_and_kurdish() {
        assert!(first_strong_is_rtl("مرحبا"));
        assert!(first_strong_is_rtl("من ناوم هێڤاره"));
        assert!(first_strong_is_rtl("سلام"));
    }

    #[test]
    fn detects_hebrew() {
        assert!(first_strong_is_rtl("שלום"));
    }

    #[test]
    fn detects_latin_as_not_rtl() {
        assert!(!first_strong_is_rtl("Hello"));
        assert!(!first_strong_is_rtl("DeckCraft"));
    }

    #[test]
    fn skips_neutrals_numbers_and_whitespace() {
        assert!(first_strong_is_rtl("  123 مرحبا"));
        assert!(first_strong_is_rtl("... (2024) سنة"));
        assert!(!first_strong_is_rtl("  123 Hello"));
        assert!(!first_strong_is_rtl(""));
        assert!(!first_strong_is_rtl("   12345 "));
    }

    #[test]
    fn honours_directional_marks() {
        assert!(first_strong_is_rtl("\u{200F}Hello"));
        assert!(first_strong_is_rtl("\u{061C}Hello"));
        assert!(first_strong_is_rtl("\u{061C}"));
        assert!(!first_strong_is_rtl("Hello\u{061C}"));
        assert!(!first_strong_is_rtl("\u{200E}\u{061C}"));
        assert!(!first_strong_is_rtl("\u{200E}مرحبا"));
    }
}
