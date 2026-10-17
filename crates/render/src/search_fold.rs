//! How search reads one character: ligatures spelled out, dashes as a hyphen, invisible format
//! characters dropped, white space as a space, precomposed Latin letters decomposed, and case
//! folded. [`crate::text`] puts the query and the page text through the same steps, so a term is
//! found the way it reads.

/// [`fold_case`](crate::text::fold_case) of one character.
pub(crate) fn fold_char(c: char) -> std::char::ToLowercase {
    match c {
        'I' | 'i' | '\u{130}' | '\u{131}' => 'i',
        'Σ' | 'σ' | 'ς' => 'σ',
        _ => c,
    }
    .to_lowercase()
}

/// What search sees for one page or query character, pushed onto `out`: compatibility
/// ligatures spelled out, dash variants as `-`, invisible format characters dropped, white
/// space as a space, and a precomposed Latin letter as its base letter plus combining mark (its
/// canonical decomposition), so the composed and decomposed spellings compare equal. The needle
/// and the page text both go through this, so a term is found the way it reads. (Unicode
/// Standard Annex #15 and UnicodeData.txt are the source; the tables are written by hand for
/// the characters text set in PDFs actually carries.)
pub(crate) fn search_chars(c: char, out: &mut Vec<char>) {
    match c {
        '\u{FB00}' => out.extend(['f', 'f']),
        '\u{FB01}' => out.extend(['f', 'i']),
        '\u{FB02}' => out.extend(['f', 'l']),
        '\u{FB03}' => out.extend(['f', 'f', 'i']),
        '\u{FB04}' => out.extend(['f', 'f', 'l']),
        '\u{FB05}' | '\u{FB06}' => out.extend(['s', 't']),
        // Soft hyphen; zero width space, non-joiner and joiner; word joiner; zero width
        // no-break space.
        '\u{AD}' | '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' => {}
        // Hyphen, non-breaking hyphen, figure dash, en dash, em dash, horizontal bar; minus
        // sign; small em dash, small hyphen-minus; fullwidth hyphen-minus.
        '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{FE58}' | '\u{FE63}' | '\u{FF0D}' => out.push('-'),
        _ if c.is_whitespace() => out.push(' '),
        _ => match DECOMPOSE.binary_search_by_key(&c, |e| e.0).ok().and_then(|i| DECOMPOSE.get(i)) {
            Some(&(_, base, mark)) => out.extend([base, mark]),
            None => out.push(c),
        },
    }
}

/// Is `c` a combining mark (U+0300 to U+036F)? It continues the word of the letter before it.
pub(crate) fn is_combining(c: char) -> bool {
    ('\u{300}'..='\u{36F}').contains(&c)
}

/// The canonical decompositions of the precomposed letters of Latin-1 Supplement and Latin
/// Extended-A (UnicodeData.txt field 5): `(letter, base letter, combining mark)`, in code point
/// order for binary search. `İ` (U+0130) is left out: [`fold_case`](crate::text::fold_case)
/// folds it to `i`.
const DECOMPOSE: &[(char, char, char)] = &[
    ('À', 'A', '\u{300}'),
    ('Á', 'A', '\u{301}'),
    ('Â', 'A', '\u{302}'),
    ('Ã', 'A', '\u{303}'),
    ('Ä', 'A', '\u{308}'),
    ('Å', 'A', '\u{30A}'),
    ('Ç', 'C', '\u{327}'),
    ('È', 'E', '\u{300}'),
    ('É', 'E', '\u{301}'),
    ('Ê', 'E', '\u{302}'),
    ('Ë', 'E', '\u{308}'),
    ('Ì', 'I', '\u{300}'),
    ('Í', 'I', '\u{301}'),
    ('Î', 'I', '\u{302}'),
    ('Ï', 'I', '\u{308}'),
    ('Ñ', 'N', '\u{303}'),
    ('Ò', 'O', '\u{300}'),
    ('Ó', 'O', '\u{301}'),
    ('Ô', 'O', '\u{302}'),
    ('Õ', 'O', '\u{303}'),
    ('Ö', 'O', '\u{308}'),
    ('Ù', 'U', '\u{300}'),
    ('Ú', 'U', '\u{301}'),
    ('Û', 'U', '\u{302}'),
    ('Ü', 'U', '\u{308}'),
    ('Ý', 'Y', '\u{301}'),
    ('à', 'a', '\u{300}'),
    ('á', 'a', '\u{301}'),
    ('â', 'a', '\u{302}'),
    ('ã', 'a', '\u{303}'),
    ('ä', 'a', '\u{308}'),
    ('å', 'a', '\u{30A}'),
    ('ç', 'c', '\u{327}'),
    ('è', 'e', '\u{300}'),
    ('é', 'e', '\u{301}'),
    ('ê', 'e', '\u{302}'),
    ('ë', 'e', '\u{308}'),
    ('ì', 'i', '\u{300}'),
    ('í', 'i', '\u{301}'),
    ('î', 'i', '\u{302}'),
    ('ï', 'i', '\u{308}'),
    ('ñ', 'n', '\u{303}'),
    ('ò', 'o', '\u{300}'),
    ('ó', 'o', '\u{301}'),
    ('ô', 'o', '\u{302}'),
    ('õ', 'o', '\u{303}'),
    ('ö', 'o', '\u{308}'),
    ('ù', 'u', '\u{300}'),
    ('ú', 'u', '\u{301}'),
    ('û', 'u', '\u{302}'),
    ('ü', 'u', '\u{308}'),
    ('ý', 'y', '\u{301}'),
    ('ÿ', 'y', '\u{308}'),
    ('Ā', 'A', '\u{304}'),
    ('ā', 'a', '\u{304}'),
    ('Ă', 'A', '\u{306}'),
    ('ă', 'a', '\u{306}'),
    ('Ą', 'A', '\u{328}'),
    ('ą', 'a', '\u{328}'),
    ('Ć', 'C', '\u{301}'),
    ('ć', 'c', '\u{301}'),
    ('Ĉ', 'C', '\u{302}'),
    ('ĉ', 'c', '\u{302}'),
    ('Ċ', 'C', '\u{307}'),
    ('ċ', 'c', '\u{307}'),
    ('Č', 'C', '\u{30C}'),
    ('č', 'c', '\u{30C}'),
    ('Ď', 'D', '\u{30C}'),
    ('ď', 'd', '\u{30C}'),
    ('Ē', 'E', '\u{304}'),
    ('ē', 'e', '\u{304}'),
    ('Ĕ', 'E', '\u{306}'),
    ('ĕ', 'e', '\u{306}'),
    ('Ė', 'E', '\u{307}'),
    ('ė', 'e', '\u{307}'),
    ('Ę', 'E', '\u{328}'),
    ('ę', 'e', '\u{328}'),
    ('Ě', 'E', '\u{30C}'),
    ('ě', 'e', '\u{30C}'),
    ('Ĝ', 'G', '\u{302}'),
    ('ĝ', 'g', '\u{302}'),
    ('Ğ', 'G', '\u{306}'),
    ('ğ', 'g', '\u{306}'),
    ('Ġ', 'G', '\u{307}'),
    ('ġ', 'g', '\u{307}'),
    ('Ģ', 'G', '\u{327}'),
    ('ģ', 'g', '\u{327}'),
    ('Ĥ', 'H', '\u{302}'),
    ('ĥ', 'h', '\u{302}'),
    ('Ĩ', 'I', '\u{303}'),
    ('ĩ', 'i', '\u{303}'),
    ('Ī', 'I', '\u{304}'),
    ('ī', 'i', '\u{304}'),
    ('Ĭ', 'I', '\u{306}'),
    ('ĭ', 'i', '\u{306}'),
    ('Į', 'I', '\u{328}'),
    ('į', 'i', '\u{328}'),
    ('Ĵ', 'J', '\u{302}'),
    ('ĵ', 'j', '\u{302}'),
    ('Ķ', 'K', '\u{327}'),
    ('ķ', 'k', '\u{327}'),
    ('Ĺ', 'L', '\u{301}'),
    ('ĺ', 'l', '\u{301}'),
    ('Ļ', 'L', '\u{327}'),
    ('ļ', 'l', '\u{327}'),
    ('Ľ', 'L', '\u{30C}'),
    ('ľ', 'l', '\u{30C}'),
    ('Ń', 'N', '\u{301}'),
    ('ń', 'n', '\u{301}'),
    ('Ņ', 'N', '\u{327}'),
    ('ņ', 'n', '\u{327}'),
    ('Ň', 'N', '\u{30C}'),
    ('ň', 'n', '\u{30C}'),
    ('Ō', 'O', '\u{304}'),
    ('ō', 'o', '\u{304}'),
    ('Ŏ', 'O', '\u{306}'),
    ('ŏ', 'o', '\u{306}'),
    ('Ő', 'O', '\u{30B}'),
    ('ő', 'o', '\u{30B}'),
    ('Ŕ', 'R', '\u{301}'),
    ('ŕ', 'r', '\u{301}'),
    ('Ŗ', 'R', '\u{327}'),
    ('ŗ', 'r', '\u{327}'),
    ('Ř', 'R', '\u{30C}'),
    ('ř', 'r', '\u{30C}'),
    ('Ś', 'S', '\u{301}'),
    ('ś', 's', '\u{301}'),
    ('Ŝ', 'S', '\u{302}'),
    ('ŝ', 's', '\u{302}'),
    ('Ş', 'S', '\u{327}'),
    ('ş', 's', '\u{327}'),
    ('Š', 'S', '\u{30C}'),
    ('š', 's', '\u{30C}'),
    ('Ţ', 'T', '\u{327}'),
    ('ţ', 't', '\u{327}'),
    ('Ť', 'T', '\u{30C}'),
    ('ť', 't', '\u{30C}'),
    ('Ũ', 'U', '\u{303}'),
    ('ũ', 'u', '\u{303}'),
    ('Ū', 'U', '\u{304}'),
    ('ū', 'u', '\u{304}'),
    ('Ŭ', 'U', '\u{306}'),
    ('ŭ', 'u', '\u{306}'),
    ('Ů', 'U', '\u{30A}'),
    ('ů', 'u', '\u{30A}'),
    ('Ű', 'U', '\u{30B}'),
    ('ű', 'u', '\u{30B}'),
    ('Ų', 'U', '\u{328}'),
    ('ų', 'u', '\u{328}'),
    ('Ŵ', 'W', '\u{302}'),
    ('ŵ', 'w', '\u{302}'),
    ('Ŷ', 'Y', '\u{302}'),
    ('ŷ', 'y', '\u{302}'),
    ('Ÿ', 'Y', '\u{308}'),
    ('Ź', 'Z', '\u{301}'),
    ('ź', 'z', '\u{301}'),
    ('Ż', 'Z', '\u{307}'),
    ('ż', 'z', '\u{307}'),
    ('Ž', 'Z', '\u{30C}'),
    ('ž', 'z', '\u{30C}'),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::fold_case;

    #[test]
    fn decomposition_table_is_sorted_and_canonical() {
        for w in DECOMPOSE.windows(2) {
            assert!(w[0].0 < w[1].0, "{:?} before {:?}", w[0].0, w[1].0);
        }
        for (c, base, mark) in DECOMPOSE {
            assert!(base.is_ascii_alphabetic(), "{c}");
            assert!(is_combining(*mark), "{c}");
            assert_ne!(*c, '\u{130}', "İ is folded by fold_case, not decomposed");
            // The decomposition round-trips through the standard library's view of the letter:
            // its lowercase form decomposes to the lowercase base letter with the same mark.
            let lower: String = c.to_lowercase().collect();
            if let Some(&(_, lb, lm)) = lower.chars().next().and_then(|l| DECOMPOSE.iter().find(|e| e.0 == l)) {
                assert_eq!((lb.to_ascii_lowercase(), lm), (base.to_ascii_lowercase(), *mark), "{c}");
            }
        }
        assert_eq!(fold_case("İIıi"), "iiii", "fold_case is unchanged");
    }
}
