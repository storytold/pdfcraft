//! Visual and logical order for Indic scripts in Edit text (Devanagari, Bengali, Gurmukhi, Gujarati, Oriya, Tamil, Malayalam).
//!
//! Legacy PDF producers often output glyphs in *visual* display order when using fonts without
//! OpenType GSUB layout tables. In visual order, pre-base vowel signs (such as Devanagari short 'i'
//! `ि` U+093F) appear in the stream *before* the consonant cluster they modify, matching their visual
//! placement to the left of the glyph.
//!
//! Standard Unicode text is *logical* (typing order): the pre-base vowel sign follows the consonant
//! cluster. The editor presents and edits text in logical order. When a source font is flagged with
//! `visual_order`, [`to_logical`] converts extracted stream text to logical order for editing, and
//! [`to_visual`] converts logical edits back into the visual glyph sequence the font expects.

/// Whether `c` is an Indic consonant across covered scripts.
pub fn is_indic_consonant(c: char) -> bool {
    matches!(
        c,
        '\u{0915}'..='\u{0939}' | '\u{0958}'..='\u{095F}' | '\u{0978}'..='\u{097F}' // Devanagari
        | '\u{0995}'..='\u{09B9}' | '\u{09DC}'..='\u{09DF}' | '\u{09F0}'..='\u{09F1}' // Bengali / Assamese
        | '\u{0A15}'..='\u{0A39}' | '\u{0A59}'..='\u{0A5E}' // Gurmukhi
        | '\u{0A95}'..='\u{0AB9}' | '\u{0AF9}' // Gujarati
        | '\u{0B15}'..='\u{0B39}' | '\u{0B5C}'..='\u{0B5F}' | '\u{0B71}' // Oriya
        | '\u{0B95}'..='\u{0BB9}' // Tamil
        | '\u{0C15}'..='\u{0C39}' | '\u{0C58}'..='\u{0C5A}' // Telugu
        | '\u{0C95}'..='\u{0CB9}' | '\u{0CDE}' // Kannada
        | '\u{0D15}'..='\u{0D3A}' // Malayalam
    )
}

/// Whether `c` is an Indic nukta (diacritic dot modifying a consonant).
pub fn is_indic_nukta(c: char) -> bool {
    matches!(
        c,
        '\u{093C}' // Devanagari
        | '\u{09BC}' // Bengali
        | '\u{0A3C}' // Gurmukhi
        | '\u{0ABC}' // Gujarati
        | '\u{0B3C}' // Oriya
        | '\u{0CBC}' // Kannada
    )
}

/// Whether `c` is an Indic virama (halant / vowel killer).
pub fn is_indic_virama(c: char) -> bool {
    matches!(
        c,
        '\u{094D}' // Devanagari
        | '\u{09CD}' // Bengali
        | '\u{0A4D}' // Gurmukhi
        | '\u{0ACD}' // Gujarati
        | '\u{0B4D}' // Oriya
        | '\u{0BCD}' // Tamil
        | '\u{0C4D}' // Telugu
        | '\u{0CCD}' // Kannada
        | '\u{0D4D}' // Malayalam
    )
}

/// Whether `c` is an Indic pre-base vowel sign that appears visually to the left of its consonant.
pub fn is_indic_pre_base_vowel(c: char) -> bool {
    matches!(
        c,
        '\u{093F}' // Devanagari short I
        | '\u{09BF}' | '\u{09C7}' | '\u{09C8}' // Bengali short I, E, AI
        | '\u{0A3F}' // Gurmukhi short I
        | '\u{0ABF}' // Gujarati short I
        | '\u{0B3F}' // Oriya short I
        | '\u{0BC6}' | '\u{0BC7}' | '\u{0BC8}' // Tamil E, EE, AI
        | '\u{0D46}' | '\u{0D47}' | '\u{0D48}' // Malayalam E, EE, AI
    )
}

/// Modifiers that attach to an Indic vowel sign (such as candrabindu, anusvara, or visarga).
fn is_indic_vowel_modifier(c: char) -> bool {
    matches!(
        c,
        '\u{0900}'..='\u{0903}' // Devanagari candrabindu, anusvara, visarga
        | '\u{0981}'..='\u{0983}' // Bengali
        | '\u{0A01}'..='\u{0A03}' | '\u{0A70}'..='\u{0A71}' // Gurmukhi (including tippi, addak)
        | '\u{0A81}'..='\u{0A83}' // Gujarati
        | '\u{0B01}'..='\u{0B03}' // Oriya
        | '\u{0B82}'..='\u{0B83}' // Tamil
        | '\u{0C00}'..='\u{0C04}' // Telugu
        | '\u{0C80}'..='\u{0C83}' // Kannada
        | '\u{0D00}'..='\u{0D03}' // Malayalam
    )
}

/// The character at `i`, if any.
fn at(chars: &[char], i: usize) -> Option<char> {
    chars.get(i).copied()
}

/// The end of the consonant cluster starting at `start` (consonants with their nuktas, joined by
/// viramas and optional ZWNJ/ZWJ); `start` itself when no consonant starts there.
fn cluster_end(chars: &[char], start: usize) -> usize {
    let mut end = start;
    while at(chars, end).is_some_and(is_indic_consonant) {
        end += 1;
        if at(chars, end).is_some_and(is_indic_nukta) {
            end += 1;
        }
        if at(chars, end).is_some_and(is_indic_virama) {
            end += 1;
            if at(chars, end).is_some_and(|c| matches!(c, '\u{200C}' | '\u{200D}')) {
                end += 1;
            }
            if at(chars, end).is_some_and(is_indic_consonant) {
                continue;
            }
        }
        break;
    }
    end
}

/// The end of the vowel-modifier run (candrabindu, anusvara, visarga) starting at `start`.
fn modifiers_end(chars: &[char], start: usize) -> usize {
    let mut end = start;
    while at(chars, end).is_some_and(is_indic_vowel_modifier) {
        end += 1;
    }
    end
}

/// Convert Indic text from visual glyph order (pre-base vowels before their consonant cluster)
/// to standard Unicode logical order.
pub fn to_logical(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while let Some(ch) = at(&chars, i) {
        if is_indic_pre_base_vowel(ch) {
            let v_end = modifiers_end(&chars, i + 1);
            let c_end = cluster_end(&chars, v_end);
            if c_end > v_end {
                out.extend_from_slice(chars.get(v_end..c_end).unwrap_or_default());
                out.extend_from_slice(chars.get(i..v_end).unwrap_or_default());
                i = c_end;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out.into_iter().collect()
}

/// Convert Indic text from standard Unicode logical order to visual display order for fonts
/// that expect pre-base vowels before their consonant cluster.
pub fn to_visual(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while let Some(ch) = at(&chars, i) {
        if is_indic_consonant(ch) {
            let c_end = cluster_end(&chars, i);
            if at(&chars, c_end).is_some_and(is_indic_pre_base_vowel) {
                let v_end = modifiers_end(&chars, c_end + 1);
                out.extend_from_slice(chars.get(c_end..v_end).unwrap_or_default());
                out.extend_from_slice(chars.get(i..c_end).unwrap_or_default());
                i = v_end;
            } else {
                out.extend_from_slice(chars.get(i..c_end).unwrap_or_default());
                i = c_end;
            }
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_words_roundtrip() {
        // किताब (kitab): k + i + t + aa + b
        let visual = "\u{093F}\u{0915}\u{0924}\u{093E}\u{092C}";
        let logical = "\u{0915}\u{093F}\u{0924}\u{093E}\u{092C}";
        assert_eq!(to_logical(visual), logical);
        assert_eq!(to_visual(logical), visual);

        // दिन (din): d + i + n
        let visual_din = "\u{093F}\u{0926}\u{0928}";
        let logical_din = "\u{0926}\u{093F}\u{0928}";
        assert_eq!(to_logical(visual_din), logical_din);
        assert_eq!(to_visual(logical_din), visual_din);
    }

    #[test]
    fn decomposed_nukta_with_pre_base_vowel() {
        // Decomposed ड़ि: DDA (U+0921) + NUKTA (U+093C) + Short I (U+093F)
        let logical = "\u{0921}\u{093C}\u{093F}";
        let visual = "\u{093F}\u{0921}\u{093C}";
        assert_eq!(to_logical(visual), logical);
        assert_eq!(to_visual(logical), visual);

        // Decomposed ढ़ि: DDHA (U+0922) + NUKTA (U+093C) + Short I (U+093F)
        let logical_ddhi = "\u{0922}\u{093C}\u{093F}";
        let visual_ddhi = "\u{093F}\u{0922}\u{093C}";
        assert_eq!(to_logical(visual_ddhi), logical_ddhi);
        assert_eq!(to_visual(logical_ddhi), visual_ddhi);
    }

    #[test]
    fn precomposed_nukta_with_pre_base_vowel() {
        // Precomposed ड़ि (U+095C + U+093F)
        let logical = "\u{095C}\u{093F}";
        let visual = "\u{093F}\u{095C}";
        assert_eq!(to_logical(visual), logical);
        assert_eq!(to_visual(logical), visual);
    }

    #[test]
    fn full_words_with_nukta_and_pre_base_vowel() {
        // बढ़िया (badhiya): ba + ddha + nukta + short_i + ya + aa
        let logical = "\u{092C}\u{0922}\u{093C}\u{093F}\u{092F}\u{093E}";
        let visual = "\u{092C}\u{093F}\u{0922}\u{093C}\u{092F}\u{093E}";
        assert_eq!(to_logical(visual), logical);
        assert_eq!(to_visual(logical), visual);

        // खिड़की (khirki): kha + short_i + dda + nukta + ka + long_ii
        let logical_khirki = "\u{0916}\u{093F}\u{0921}\u{093C}\u{0915}\u{0940}";
        let visual_khirki = "\u{093F}\u{0916}\u{0921}\u{093C}\u{0915}\u{0940}";
        assert_eq!(to_logical(visual_khirki), logical_khirki);
        assert_eq!(to_visual(logical_khirki), visual_khirki);
    }

    #[test]
    fn complex_conjuncts_with_pre_base_vowel() {
        // स्थिति (sthiti): sa + virama + tha + short_i + ta + short_i
        let logical = "\u{0938}\u{094D}\u{0925}\u{093F}\u{0924}\u{093F}";
        let visual = "\u{093F}\u{0938}\u{094D}\u{0925}\u{093F}\u{0924}";
        assert_eq!(to_logical(visual), logical);
        assert_eq!(to_visual(logical), visual);
    }

    #[test]
    fn other_indic_scripts_pre_base_vowels() {
        // Bengali কি (ki): KA (U+0995) + I (U+09BF)
        let b_logical = "\u{0995}\u{09BF}";
        let b_visual = "\u{09BF}\u{0995}";
        assert_eq!(to_logical(b_visual), b_logical);
        assert_eq!(to_visual(b_logical), b_visual);

        // Gurmukhi ਕਿ (ki): KA (U+0A15) + I (U+0A3F)
        let g_logical = "\u{0A15}\u{0A3F}";
        let g_visual = "\u{0A3F}\u{0A15}";
        assert_eq!(to_logical(g_visual), g_logical);
        assert_eq!(to_visual(g_logical), g_visual);
    }

    #[test]
    fn non_indic_text_is_untouched() {
        for s in ["", "Hello world!", "PDF 123", "日本語テキスト", "123.45"] {
            assert_eq!(to_logical(s), s);
            assert_eq!(to_visual(s), s);
        }
    }

    #[test]
    fn gurmukhi_tippi_addak_and_southern_vowel_modifiers_reorder_with_pre_base_vowel() {
        // Gurmukhi ਸਿੰਘ (Singh): SA (U+0A38) + I (U+0A3F) + TIPPI (U+0A70) + GHA (U+0A18)
        let singh_logical = "\u{0A38}\u{0A3F}\u{0A70}\u{0A18}";
        let singh_visual = "\u{0A3F}\u{0A70}\u{0A38}\u{0A18}";
        assert_eq!(to_logical(singh_visual), singh_logical);
        assert_eq!(to_visual(singh_logical), singh_visual);

        // Gurmukhi ਇੱਕ / ਸਿੱਖ with Addak (U+0A71): SA (U+0A38) + I (U+0A3F) + ADDAK (U+0A71) + KHA (U+0A16)
        let sikh_logical = "\u{0A38}\u{0A3F}\u{0A71}\u{0A16}";
        let sikh_visual = "\u{0A3F}\u{0A71}\u{0A38}\u{0A16}";
        assert_eq!(to_logical(sikh_visual), sikh_logical);
        assert_eq!(to_visual(sikh_logical), sikh_visual);

        // Tamil and Malayalam pre-base vowel with vowel modifier (U+0B82, U+0D02)
        let tamil_logical = "\u{0B95}\u{0BC6}\u{0B82}";
        let tamil_visual = "\u{0BC6}\u{0B82}\u{0B95}";
        assert_eq!(to_logical(tamil_visual), tamil_logical);
        assert_eq!(to_visual(tamil_logical), tamil_visual);

        let malayalam_logical = "\u{0D15}\u{0D46}\u{0D02}";
        let malayalam_visual = "\u{0D46}\u{0D02}\u{0D15}";
        assert_eq!(to_logical(malayalam_visual), malayalam_logical);
        assert_eq!(to_visual(malayalam_logical), malayalam_visual);

        // Devanagari inverted candrabindu (U+0900)
        let dev_logical = "\u{0915}\u{093F}\u{0900}";
        let dev_visual = "\u{093F}\u{0900}\u{0915}";
        assert_eq!(to_logical(dev_visual), dev_logical);
        assert_eq!(to_visual(dev_logical), dev_visual);

        // Telugu (U+0C00..=U+0C04) and Kannada (U+0C80..=U+0C83) vowel modifiers
        for m in ['\u{0C00}', '\u{0C01}', '\u{0C04}'] {
            let logical = format!("\u{0C15}\u{0BC6}{m}");
            let visual = format!("\u{0BC6}{m}\u{0C15}");
            assert_eq!(to_logical(&visual), logical);
            assert_eq!(to_visual(&logical), visual);
        }
        for m in ['\u{0C80}', '\u{0C81}', '\u{0C83}'] {
            let logical = format!("\u{0C95}\u{0BC6}{m}");
            let visual = format!("\u{0BC6}{m}\u{0C95}");
            assert_eq!(to_logical(&visual), logical);
            assert_eq!(to_visual(&logical), visual);
        }
    }

    #[test]
    fn assamese_oriya_gujarati_consonants_and_kannada_nukta_are_recognized() {
        // Assamese ৰি (ri) and ৱি (wi): U+09F0 / U+09F1 + U+09BF
        for cons in ['\u{09F0}', '\u{09F1}'] {
            let logical = format!("{cons}\u{09BF}");
            let visual = format!("\u{09BF}{cons}");
            assert_eq!(to_logical(&visual), logical);
            assert_eq!(to_visual(&logical), visual);
        }

        // Oriya ୱି (wi): U+0B71 + U+0B3F, Gujarati ૹિ (zhi): U+0AF9 + U+0ABF
        assert_eq!(to_logical("\u{0B3F}\u{0B71}"), "\u{0B71}\u{0B3F}");
        assert_eq!(to_visual("\u{0B71}\u{0B3F}"), "\u{0B3F}\u{0B71}");
        assert_eq!(to_logical("\u{0ABF}\u{0AF9}"), "\u{0AF9}\u{0ABF}");
        assert_eq!(to_visual("\u{0AF9}\u{0ABF}"), "\u{0ABF}\u{0AF9}");

        // Kannada consonant (U+0C95) + nukta (U+0CBC) + pre-base vowel reorders as one cluster
        assert!(is_indic_nukta('\u{0CBC}'));
        let kn_logical = "\u{0C95}\u{0CBC}\u{0BC6}";
        let kn_visual = "\u{0BC6}\u{0C95}\u{0CBC}";
        assert_eq!(to_logical(kn_visual), kn_logical);
        assert_eq!(to_visual(kn_logical), kn_visual);
    }
}
