//! Where a row of rewrapped text may end besides a space: between two characters when either is
//! written without spaces between words (Chinese and Japanese ideographs and kana), following the
//! simple cases of Unicode line breaking (UAX #14, class ID). The row must not then begin with a
//! character that closes or follows (closing brackets and quotes, commas and full stops, small
//! kana, the prolonged sound mark, iteration marks) or end with one that opens (opening brackets
//! and quotes, currency signs before an amount): the line-start and line-end prohibitions of
//! JIS X 4051 ("kinsoku"), which UAX #14 gives as classes CL, CP, EX, IS, NS and OP, PR.
//!
//! Thai is also written without spaces between words, but a row may not end between any two of its
//! characters: its words need a dictionary to find. They come from ICU4X's dictionary line
//! segmenter (UAX #14 with dictionary breaking for class SA), kept only where a Thai character is on
//! either side. A row never begins with a following vowel, a tone or other mark, Mai Yamok (ๆ) or
//! Paiyannoi (ฯ), nor ends with a leading vowel (เ แ โ ใ ไ), whatever the dictionary says.
//!
//! Korean is written with spaces between words and keeps breaking only there.

use std::sync::LazyLock;

use icu_segmenter::options::LineBreakOptions;
use icu_segmenter::{LineSegmenter, LineSegmenterBorrowed};

/// The dictionary segmenter, built once: its data is compiled in, so building it cannot fail.
static SEGMENTER: LazyLock<LineSegmenterBorrowed<'static>> = LazyLock::new(|| LineSegmenter::new_dictionary(LineBreakOptions::default()));

/// Letters, vowels, marks, digits and signs of the Thai block.
fn thai(c: char) -> bool {
    matches!(c, '\u{0e01}'..='\u{0e5b}')
}

/// Thai that belongs to the character before it: Paiyannoi, the following vowels, Lakkhangyao,
/// Mai Yamok and the tone and other marks.
fn thai_no_start(c: char) -> bool {
    matches!(c, '\u{0e2f}' | '\u{0e30}'..='\u{0e3a}' | '\u{0e45}'..='\u{0e4e}')
}

/// The leading vowels, written before the consonant they follow in speech.
fn thai_no_end(c: char) -> bool {
    matches!(c, '\u{0e40}'..='\u{0e44}')
}

/// Byte offsets where the dictionary lets a row of Thai end inside `word`.
fn thai_breaks(word: &str) -> Vec<usize> {
    if !word.chars().any(thai) {
        return Vec::new();
    }
    SEGMENTER
        .segment_str(word)
        .filter(|&at| {
            let before = word.get(..at).and_then(|s| s.chars().next_back());
            let after = word.get(at..).and_then(|s| s.chars().next());
            match (before, after) {
                (Some(b), Some(a)) => (thai(b) || thai(a)) && !thai_no_start(a) && !thai_no_end(b) && !no_start(a) && !no_end(b),
                // The start and end of the word are not breaks inside it.
                _ => false,
            }
        })
        .collect()
}

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

/// Whether a row may end between `before` and `after` in Chinese or Japanese, neither of them a
/// space. Thai needs the whole word ([`breaks`]).
fn between(before: char, after: char) -> bool {
    (ideographic(before) || ideographic(after)) && !no_start(after) && !no_end(before) && !thai_no_start(after) && !thai_no_end(before)
}

/// Byte offsets inside `word` (text without spaces) where a row may end, in order: never 0 or the
/// word's length, always on a character boundary.
pub(crate) fn breaks(word: &str) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut prev: Option<char> = None;
    for (at, c) in word.char_indices() {
        if let Some(before) = prev
            && between(before, c)
        {
            out.push(at);
        }
        prev = Some(c);
    }
    out.extend(thai_breaks(word));
    out.sort_unstable();
    out.dedup();
    out
}

/// `word` (text without spaces) cut where a row may end inside it; the pieces join with nothing.
pub(crate) fn pieces(word: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for at in breaks(word) {
        out.extend(word.get(start..at));
        start = at;
    }
    out.extend(word.get(start..).filter(|rest| !rest.is_empty()));
    out
}

/// For each character of `word` (text without spaces), whether a row may end just before it.
pub(crate) fn breaks_before_chars(word: &str) -> Vec<bool> {
    let cuts = breaks(word);
    word.char_indices().map(|(at, _)| cuts.binary_search(&at).is_ok()).collect()
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

    #[test]
    fn thai_breaks_between_dictionary_words() {
        assert_eq!(pieces("ภาษาไทยไม่มีช่องว่างระหว่างคำ"), ["ภาษา", "ไทย", "ไม่มี", "ช่อง", "ว่าง", "ระหว่าง", "คำ"]);
        assert_eq!(pieces("น้ำประปาสะอาด"), ["น้ำ", "ประปา", "สะอาด"]);
        assert_eq!(pieces("การประปาส่วนภูมิภาค"), ["การ", "ประปา", "ส่วน", "ภูมิภาค"]);
    }

    #[test]
    fn thai_vowels_marks_and_repetition_stay_with_the_word_before_them() {
        // The dictionary alone would start a row with Mai Yamok, with the Sara Aa of a decomposed
        // Sara Am, or with a closing bracket.
        assert_eq!(pieces("เด็กๆชอบกินขนม"), ["เด็กๆ", "ชอบ", "กิน", "ขนม"]);
        assert_eq!(pieces("ท\u{0e4d}\u{0e32}งาน"), ["ท\u{0e4d}\u{0e32}", "งาน"]);
        assert_eq!(pieces("กรุงเทพฯเป็นเมืองหลวง"), ["กรุงเทพฯ", "เป็น", "เมือง", "หลวง"]);
        assert_eq!(pieces("สวัสดีครับ(ทดสอบ)"), ["สวัสดี", "ครับ", "(ทดสอบ)"]);
    }

    #[test]
    fn thai_keeps_numbers_and_latin_whole() {
        assert_eq!(pieces("ราคา100บาทต่อเดือน"), ["ราคา", "100บาท", "ต่อ", "เดือน"]);
        assert_eq!(pieces("พ.ศ.2569"), ["พ.ศ.2569"]);
        assert_eq!(pieces("PDF"), ["PDF"]);
        assert_eq!(breaks("ไทย"), Vec::<usize>::new());
    }

    #[test]
    fn breaks_before_chars_marks_the_first_character_of_each_later_piece() {
        let flags = breaks_before_chars("น้ำประปา");
        let starts: Vec<usize> = flags.iter().enumerate().filter_map(|(i, &b)| b.then_some(i)).collect();
        // น ้ ำ | ป ร ะ ป า
        assert_eq!(starts, [3]);
        assert_eq!(breaks_before_chars(""), Vec::<bool>::new());
    }

    #[test]
    fn pieces_of_arbitrary_thai_rejoin_to_the_input_and_never_start_with_a_mark() {
        // A deterministic generator over Thai, Latin, digits and CJK, including mark-only runs and
        // unusual orders that no dictionary word has.
        let alphabet: Vec<char> = ('\u{0e01}'..='\u{0e5b}').chain(['a', '1', '(', ')', '字', 'の', '.']).collect();
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..2000 {
            let mut word = String::new();
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let len = (state >> 59) as usize + 1;
            for _ in 0..len {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let pick = (state >> 33) as usize % alphabet.len();
                word.extend(alphabet.get(pick));
            }
            let parts = pieces(&word);
            assert_eq!(parts.concat(), word);
            assert!(parts.iter().all(|p| !p.is_empty()), "{word:?} gave {parts:?}");
            for part in parts.iter().skip(1) {
                let first = part.chars().next().unwrap_or(' ');
                assert!(!thai_no_start(first) && !no_start(first), "{word:?} gave {parts:?}");
            }
            assert_eq!(breaks_before_chars(&word).len(), word.chars().count());
        }
    }
}
