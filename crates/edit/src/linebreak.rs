//! Where a row of rewrapped text may end besides a space: between two characters when either is
//! written without spaces between words (Chinese and Japanese ideographs and kana), following the
//! simple cases of Unicode line breaking (UAX #14, class ID). The row must not then begin with a
//! character that closes or follows (closing brackets and quotes, commas and full stops, small
//! kana, the prolonged sound mark, iteration marks) or end with one that opens (opening brackets
//! and quotes, currency signs before an amount): the line-start and line-end prohibitions of
//! JIS X 4051 ("kinsoku"), which UAX #14 gives as classes CL, CP, EX, IS, NS and OP, PR.
//!
//! Korean is written with spaces between words and keeps breaking only there; so does Thai, whose
//! words need a dictionary to find.

/// Writing with no spaces between words, where a row may end between two characters (UAX #14 ID).
fn ideographic(c: char) -> bool {
    matches!(c,
        '\u{2e80}'..='\u{2fdf}' // CJK radicals, Kangxi radicals
        | '\u{3000}'..='\u{303f}' // CJK symbols and punctuation
        | '\u{3040}'..='\u{30ff}' // hiragana, katakana
        | '\u{3190}'..='\u{31ff}' // kanbun, CJK strokes, katakana phonetic extensions
        | '\u{3400}'..='\u{4dbf}' // CJK extension A
        | '\u{4e00}'..='\u{9fff}' // CJK unified ideographs
        | '\u{f900}'..='\u{faff}' // CJK compatibility ideographs
        | '\u{fe30}'..='\u{fe4f}' // CJK compatibility forms
        | '\u{ff00}'..='\u{ffef}' // half-width and full-width forms
        | '\u{20000}'..='\u{3fffd}' // supplementary ideographic planes
    )
}

/// Characters that must not begin a row: closing brackets and quotes, and the punctuation, small
/// kana and marks that belong to the character before them (UAX #14 CL, CP, EX, IS, NS). The
/// ideographic space breaks after itself, not before (BA): it is Japanese spacing and indentation.
fn no_start(c: char) -> bool {
    matches!(c,
        ')' | ']' | '}' | '»' | '’' | '”' | '!' | '?' | ',' | '.' | ':' | ';' | '%' | '‰' | '°' | '℃'
        | '‐' | '–' | '〜' | '‼' | '⁇' | '⁈' | '⁉' | '・' | '\u{3000}'
        | '、' | '。' | '〃' | '〆' | '々' | '〻' | '〕' | '〉' | '》' | '」' | '』' | '】' | '〙' | '〗' | '〟' | '〞'
        | 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'っ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゎ' | 'ゕ' | 'ゖ'
        | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ッ' | 'ャ' | 'ュ' | 'ョ' | 'ヮ' | 'ヵ' | 'ヶ'
        | '゛' | '゜' | 'ゝ' | 'ゞ' | '゠' | 'ー' | 'ヽ' | 'ヾ'
        | '\u{31f0}'..='\u{31ff}' // small katakana for Ainu
        | '！' | '％' | '）' | '，' | '．' | '：' | '；' | '？' | '］' | '｝' | '｠' | '～'
        | '｡' | '｣' | '､' | '･' | 'ｰ' | 'ﾞ' | 'ﾟ'
        | '\u{ff67}'..='\u{ff6f}' // small half-width katakana
    )
}

/// Characters that must not end a row: opening brackets and quotes, and currency signs written
/// before an amount (UAX #14 OP, PR).
fn no_end(c: char) -> bool {
    matches!(
        c,
        '(' | '['
            | '{'
            | '«'
            | '‘'
            | '“'
            | '$'
            | '£'
            | '¥'
            | '€'
            | '〈'
            | '《'
            | '「'
            | '『'
            | '【'
            | '〔'
            | '〖'
            | '〘'
            | '〝'
            | '（'
            | '［'
            | '｛'
            | '｟'
            | '｢'
            | '＄'
            | '￡'
            | '￥'
    )
}

/// Whether a row may end between `before` and `after`, neither of them a space.
pub(crate) fn between(before: char, after: char) -> bool {
    (ideographic(before) || ideographic(after)) && !no_start(after) && !no_end(before)
}

/// `word` (text without spaces) cut where a row may end inside it; the pieces join with nothing.
pub(crate) fn pieces(word: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<char> = None;
    for (at, c) in word.char_indices() {
        if let Some(before) = prev
            && between(before, c)
        {
            out.extend(word.get(start..at));
            start = at;
        }
        prev = Some(c);
    }
    out.extend(word.get(start..).filter(|rest| !rest.is_empty()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ideographs_and_kana_break_between_characters_except_around_kinsoku_marks() {
        assert_eq!(pieces("日本語の文章"), ["日", "本", "語", "の", "文", "章"]);
        // Closing punctuation and small kana stay with the character before them; an opening
        // bracket stays with the one after it.
        assert_eq!(pieces("ちょっと、「はい」。"), ["ちょっ", "と、", "「は", "い」。"]);
        assert_eq!(pieces("コーヒー"), ["コー", "ヒー"]);
        assert_eq!(pieces("（例）です"), ["（例）", "で", "す"]);
        // Latin text inside a Japanese sentence breaks before and after it, never inside it.
        assert_eq!(pieces("PDFを編集"), ["PDF", "を", "編", "集"]);
        assert_eq!(pieces("価格は￥100です"), ["価", "格", "は", "￥100", "で", "す"]);
        // Words of space-separated writing are never cut.
        assert_eq!(pieces("wrapping"), ["wrapping"]);
        assert_eq!(pieces("한국어"), ["한국어"]);
        assert_eq!(pieces(""), Vec::<&str>::new());
        assert!(!between('a', 'b') && between('字', 'a') && !between('字', '。') && !between('「', '字'));
    }
}
