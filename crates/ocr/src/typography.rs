//! Language-specific repairs of characters the recognizer reliably confuses. Each rule needs
//! letters around the repaired character, so names, numbers and literal text keep what was read.
use crate::Word;

/// Repair one line's words read in `language`.
pub(crate) fn correct(mut words: Vec<Word>, language: &str) -> Vec<Word> {
    if matches!(language, "ja" | "zh-Hant" | "zh-Hans") {
        return words;
    }
    // A hyphen standing alone between two words is a dash, which the model reads as "-".
    for i in 1..words.len().saturating_sub(1) {
        if words[i].text == "-" {
            words[i].text = "–".into();
        }
    }
    for word in &mut words {
        let mut text = std::mem::take(&mut word.text);
        if language == "de" {
            text = german_quotes(&text);
        }
        if matches!(language, "fr" | "it" | "ca") {
            text = elided_l(&text);
        }
        if language == "ca" {
            text = geminated_l(&text);
        }
        word.text = text;
    }
    words
}

/// German opening quotes sit low („) and closing ones high (“); the model reads both as `"`,
/// and the low one also as `,,`.
fn german_quotes(text: &str) -> String {
    let mut text = text.to_owned();
    for open in ["\"", ",,"] {
        if text.strip_prefix(open).and_then(|rest| rest.chars().next()).is_some_and(char::is_alphanumeric) {
            text = format!("„{}", &text[open.len()..]);
        }
    }
    // The closing quote may come before or after the sentence's punctuation; after a digit it
    // is an inch mark.
    const PUNCTUATION: [char; 7] = ['.', ',', '!', '?', ';', ':', '…'];
    let body = text.trim_end_matches(PUNCTUATION);
    if let Some(rest) = body.strip_suffix('"')
        && rest.trim_end_matches(PUNCTUATION).chars().last().is_some_and(char::is_alphabetic)
    {
        text = format!("{rest}“{}", &text[body.len()..]);
    }
    text
}

/// The elided article `l’`/`l'` read as `1’`/`1'` (French, Italian, Catalan): a one before an
/// apostrophe that starts a word and is followed by a letter.
fn elided_l(text: &str) -> String {
    let mut chars = text.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some('1'), Some(apostrophe @ ('\'' | '’')), Some(next)) if next.is_alphabetic() => {
            format!("l{apostrophe}{}", &text[1 + apostrophe.len_utf8()..])
        }
        _ => text.to_owned(),
    }
}

/// Catalan's geminated `l·l`, whose middle dot the model reads as `-`, `.` or `•`: only
/// between two `l` of the same case with letters on both sides, as in `col·legi`, `IL·LUSIÓ`.
fn geminated_l(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = chars.clone();
    for i in 2..chars.len().saturating_sub(2) {
        let (before, l1, dot, l2, after) = (chars[i - 2], chars[i - 1], chars[i], chars[i + 1], chars[i + 2]);
        let same_l = (l1 == 'l' && l2 == 'l') || (l1 == 'L' && l2 == 'L');
        if same_l && matches!(dot, '-' | '.' | '•' | '∙') && before.is_alphabetic() && after.is_alphabetic() {
            out[i] = '·';
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(words: &[&str], language: &str) -> Vec<String> {
        let words = words.iter().map(|text| Word::new(*text, [0.0, 0.0, 1.0, 1.0], "ocrs")).collect();
        correct(words, language).into_iter().map(|w| w.text).collect()
    }

    #[test]
    fn a_lone_hyphen_between_words_is_a_dash() {
        assert_eq!(line(&["Paris", "-", "Berlin"], "en"), ["Paris", "–", "Berlin"]);
        assert_eq!(line(&["-", "item"], "en"), ["-", "item"], "a list bullet stays");
        assert_eq!(line(&["well-known", "-5"], "de"), ["well-known", "-5"]);
        assert_eq!(line(&["東京", "-", "大阪"], "ja"), ["東京", "-", "大阪"]);
    }

    #[test]
    fn german_quotes_open_low_and_close_high() {
        assert_eq!(line(&["\"Guten", "Tag\""], "de"), ["„Guten", "Tag“"]);
        assert_eq!(line(&[",,Hallo\"."], "de"), ["„Hallo“."]);
        assert_eq!(line(&["\"Hallo!\""], "de"), ["„Hallo!“"]);
        assert_eq!(line(&["5\"", "\"", ",,"], "de"), ["5\"", "\"", ",,"], "an inch mark and stray quotes stay");
        assert_eq!(line(&["\"Hello\""], "en"), ["\"Hello\""]);
    }

    #[test]
    fn a_one_before_an_apostrophe_is_the_elided_article() {
        assert_eq!(line(&["1'été", "1’école"], "fr"), ["l'été", "l’école"]);
        assert_eq!(line(&["1'anno"], "it"), ["l'anno"]);
        assert_eq!(line(&["1'home"], "ca"), ["l'home"]);
        assert_eq!(line(&["1'", "1'5", "21'été"], "fr"), ["1'", "1'5", "21'été"]);
        assert_eq!(line(&["1'été"], "en"), ["1'été"]);
    }

    #[test]
    fn catalan_geminated_l_gets_its_middle_dot() {
        assert_eq!(line(&["col-legi", "il.lusió", "intel•ligent", "IL-LUSIÓ"], "ca"), ["col·legi", "il·lusió", "intel·ligent", "IL·LUSIÓ"]);
        assert_eq!(line(&["l-l", "al-l", "Il-La", "ball-2", "www.ll.cat"], "ca"), ["l-l", "al-l", "Il-La", "ball-2", "www.ll.cat"]);
        assert_eq!(line(&["col-legi"], "es"), ["col-legi"]);
    }

    #[test]
    fn repairs_are_drawable_by_the_text_layer_font() {
        assert_eq!(pdfcraft_fonts::first_undrawable("„“–·’"), None);
    }
}
