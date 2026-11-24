//! Word boundaries in a paragraph, as PowerPoint's Ctrl+arrow, Ctrl+Backspace/Delete and
//! double-click see them (observed black-box on PowerPoint for Windows, see
//! `docs/ui-parity.md`, Text editing keys).
//!
//! A paragraph splits into *tokens*. Each token is a run of one class of grapheme clusters plus
//! the spaces that follow it:
//!
//! - **word**: letters, digits and combining marks. An apostrophe (`'` or `’`) joins a word when a
//!   letter or digit follows it (`it's`, `'tis`, but `dogs'` ends before it); a hyphen joins when
//!   a letter or digit comes before it (`a-b`, `1-2`, `a-`); `.`, `,` and `:` join between digits
//!   (`3.14`, `1,000`, `3:45`, but `e.g.` is five tokens).
//! - **punctuation**: everything else that is not a symbol; a run of it is one token (`...`,
//!   `?!`, `),`, `。。`). `_`, `@`, `/`, `#`, quotes and brackets are punctuation.
//! - **symbol**: emoji and pictographs (a run of them is one token, `👍👍`), `©`, `®`, `™`.
//! - **ideograph**: each CJK ideograph, kana or Hangul syllable is a token of its own.
//! - **tab** and **line break**: each is a token of its own, and spaces after one form their own
//!   token instead of joining it.
//! - **space**: U+0020, no-break space, ideographic space and the other fixed-width spaces. Spaces
//!   join the token before them; spaces at the start of a paragraph (or after a tab or line break)
//!   are a token of their own.
//!
//! Positions are character (Unicode scalar) offsets in the paragraph text, where a line break is
//! one `\u{b}` character. A token never splits a grapheme cluster, so the caret never lands
//! inside an emoji sequence or between a letter and its combining marks.

use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Space,
    Tab,
    Break,
    Word,
    Ideo,
    Punct,
    Symbol,
}

/// One grapheme cluster: its first character offset and class.
#[derive(Clone, Copy, Debug)]
struct Cluster {
    start: usize,
    class: Class,
}

/// A space for word movement: absorbed into the token before it.
pub fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// CJK ideographs, kana and Hangul: every one is a word of its own.
fn is_ideo(c: char) -> bool {
    matches!(c,
        '\u{1100}'..='\u{11ff}'
        | '\u{2e80}'..='\u{2fdf}'
        | '\u{3005}'..='\u{3007}'
        | '\u{3021}'..='\u{3029}'
        | '\u{3031}'..='\u{3035}'
        | '\u{3038}'..='\u{303c}'
        | '\u{3040}'..='\u{309f}'
        | '\u{30a0}'..='\u{30ff}'
        | '\u{3100}'..='\u{318f}'
        | '\u{31a0}'..='\u{31ff}'
        | '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{a960}'..='\u{a97f}'
        | '\u{ac00}'..='\u{d7ff}'
        | '\u{f900}'..='\u{faff}'
        | '\u{ff66}'..='\u{ffdc}'
        | '\u{20000}'..='\u{3134f}')
}

/// Emoji, pictographs and the common letter-like symbols.
fn is_symbol(c: char) -> bool {
    matches!(c,
        '\u{a9}' | '\u{ae}' | '\u{2122}'
        | '\u{2190}'..='\u{21ff}'
        | '\u{2300}'..='\u{23ff}'
        | '\u{25a0}'..='\u{27bf}'
        | '\u{2b00}'..='\u{2bff}'
        | '\u{1f000}'..='\u{1faff}')
}

fn base_class(c: char) -> Class {
    match c {
        '\u{b}' | '\n' | '\r' | '\u{2028}' | '\u{2029}' => Class::Break,
        '\t' => Class::Tab,
        c if is_space(c) => Class::Space,
        c if is_ideo(c) => Class::Ideo,
        c if c.is_alphanumeric() => Class::Word,
        c if is_symbol(c) => Class::Symbol,
        // A combining mark with no base (or any other mark) stays with letters.
        c if is_combining_mark(c) => Class::Word,
        _ => Class::Punct,
    }
}

/// Whether `c` is a combining mark (a decomposed accent), which belongs to the letter before it.
pub fn is_combining_mark(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}' | '\u{20d0}'..='\u{20ff}' | '\u{fe20}'..='\u{fe2f}')
}

fn clusters(text: &str) -> Vec<Cluster> {
    let mut out: Vec<Cluster> = Vec::new();
    let mut firsts: Vec<char> = Vec::new();
    let mut pos = 0;
    for g in text.graphemes(true) {
        let n = g.chars().count();
        let first = g.chars().next().unwrap_or(' ');
        // "\r\n" is one grapheme; any other cluster takes the class of its first character.
        out.push(Cluster { start: pos, class: base_class(first) });
        firsts.push(first);
        pos += n;
    }
    // Context: apostrophes, hyphens and number separators that belong to a word.
    let classes: Vec<Class> = out.iter().map(|c| c.class).collect();
    for (i, cl) in out.iter_mut().enumerate() {
        let Some(&c) = firsts.get(i) else { continue };
        let prev = i.checked_sub(1).and_then(|j| firsts.get(j)).copied();
        let next = firsts.get(i + 1).copied();
        let joins = match c {
            '\'' | '\u{2019}' => next.is_some_and(char::is_alphanumeric),
            '-' => prev.is_some_and(char::is_alphanumeric),
            '.' | ',' | ':' => prev.is_some_and(char::is_numeric) && next.is_some_and(char::is_numeric),
            _ => false,
        };
        if joins && classes.get(i) == Some(&Class::Punct) {
            cl.class = Class::Word;
        }
    }
    out
}

/// Does a token start at cluster `cur` (after `prev`)?
fn starts_token(prev: Class, cur: Class) -> bool {
    if cur == Class::Space {
        // Spaces join the token before them, except after a tab or a line break.
        return matches!(prev, Class::Tab | Class::Break);
    }
    prev == Class::Space || prev != cur || matches!(cur, Class::Ideo | Class::Tab | Class::Break)
}

/// The word stops of a paragraph's text in ascending order: 0, the start of every token, and the
/// end of the text. Ctrl+Right goes to the next one.
pub fn stops(text: &str) -> Vec<usize> {
    let cs = clusters(text);
    let mut out = vec![0];
    for w in cs.windows(2) {
        if let [a, b] = w
            && starts_token(a.class, b.class)
        {
            out.push(b.start);
        }
    }
    let n = text.chars().count();
    if out.last() != Some(&n) {
        out.push(n);
    }
    out
}

/// The stops Ctrl+Left uses. PowerPoint's backward scan differs from the forward one in two
/// observed details: a no-break or other non-ASCII space is a stop of its own (unless an ASCII
/// space follows it), and a symbol followed by punctuation (`👍?`) is one token.
pub fn back_stops(text: &str) -> Vec<usize> {
    let cs = clusters(text);
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![0];
    for w in cs.windows(2) {
        if let [a, b] = w {
            let symbol_punct = matches!((a.class, b.class), (Class::Symbol, Class::Punct) | (Class::Punct, Class::Symbol));
            if starts_token(a.class, b.class) && !symbol_punct {
                out.push(b.start);
            }
        }
    }
    for (i, c) in chars.iter().enumerate() {
        if *c != ' ' && is_space(*c) && chars.get(i + 1) != Some(&' ') && i > 0 {
            out.push(i);
        }
    }
    out.sort_unstable();
    out.dedup();
    let n = chars.len();
    if out.last() != Some(&n) {
        out.push(n);
    }
    out
}

/// The next stop after `ch` (Ctrl+Right), or `None` at the end of the text.
pub fn next_stop(text: &str, ch: usize) -> Option<usize> {
    stops(text).into_iter().find(|s| *s > ch)
}

/// The previous stop before `ch` (Ctrl+Left), or `None` at the start.
pub fn prev_stop(text: &str, ch: usize) -> Option<usize> {
    back_stops(text).into_iter().rev().find(|s| *s < ch)
}

/// Is the character at `i` a space (for word purposes)?
fn space_at(chars: &[char], i: usize) -> bool {
    chars.get(i).is_some_and(|c| is_space(*c))
}

/// The end of the token's text (before its trailing spaces) for the token containing `ch`.
fn content_end(text: &str, ch: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let next = next_stop(text, ch).unwrap_or(chars.len());
    let mut e = ch;
    while e < next && !space_at(&chars, e) {
        e += 1;
    }
    // A token made only of spaces (leading spaces) ends at the next stop.
    if e == ch && space_at(&chars, ch) { next } else { e }
}

/// The word a double-click at `ch` selects: the token under the caret without its trailing
/// spaces; on a space just after a token, that token; in a run of spaces, the spaces.
pub fn word_at(text: &str, ch: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return (0, 0);
    }
    let ch = ch.min(n);
    let on_space = ch >= n || space_at(&chars, ch);
    let after_token = ch > 0 && !space_at(&chars, ch - 1);
    let at = if on_space && after_token { ch - 1 } else { ch.min(n - 1) };
    if space_at(&chars, at) {
        let mut s = at;
        while s > 0 && space_at(&chars, s - 1) {
            s -= 1;
        }
        let mut e = at;
        while e < n && space_at(&chars, e) {
            e += 1;
        }
        return (s, e);
    }
    let s = stops(text).into_iter().rev().find(|s| *s <= at).unwrap_or(0);
    (s, content_end(text, s).max(at + 1).min(n))
}

/// What Ctrl+Delete removes from `ch` (before smart spacing): inside a token, the rest of its
/// text; on spaces right after a token, those spaces and the next token's text; anywhere else up
/// to the next stop. `None` at the end of the text.
pub fn delete_forward_end(text: &str, ch: usize) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if ch >= n {
        return None;
    }
    let st = stops(text);
    let at_stop = st.contains(&ch);
    if !at_stop && !space_at(&chars, ch) {
        return Some(content_end(text, ch));
    }
    let prev_is_text = ch > 0 && chars.get(ch - 1).is_some_and(|c| !is_space(*c) && !matches!(base_class(*c), Class::Tab | Class::Break));
    if space_at(&chars, ch) && prev_is_text {
        let mut j = ch;
        while j < n && space_at(&chars, j) {
            j += 1;
        }
        if j >= n {
            return Some(n);
        }
        return Some(content_end(text, j));
    }
    st.into_iter().find(|s| *s > ch)
}

/// Closing punctuation that a smart delete pulls its word's spaces up to (`ab cd, ef` → Ctrl+Delete
/// on `cd` gives `ab, ef`).
pub fn is_closing_punct(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '%' | '\u{2026}' | '\u{201d}' | '\u{2019}' | '\u{bb}')
}

/// Punctuation that a smart delete keeps a space after when a word follows (`cd...ef gh` →
/// Ctrl+Delete on `ef` gives `cd... gh`).
pub fn is_sentence_punct(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\u{2026}')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stops drawn as `|` in the text, for readable assertions.
    fn marked(text: &str, st: &[usize]) -> String {
        let mut out = String::new();
        for (i, c) in text.chars().enumerate() {
            if st.contains(&i) {
                out.push('|');
            }
            out.push(match c {
                '\u{b}' => '↵',
                '\t' => '→',
                c => c,
            });
        }
        if st.contains(&text.chars().count()) {
            out.push('|');
        }
        out
    }
    fn fwd(text: &str) -> String {
        marked(text, &stops(text))
    }
    fn back(text: &str) -> String {
        marked(text, &back_stops(text))
    }

    // Every expected string below is a stop trace observed in PowerPoint for Windows.
    #[test]
    fn punctuation_and_spaces() {
        let t = "Hello, world!  Foo...bar baz-qux it's 3.14 e.g. a_b x@y.com (end) \"q\" end.";
        let want = "|Hello|, |world|!  |Foo|...|bar |baz-qux |it's |3.14 |e|.|g|. |a|_|b |x|@|y|.|com |(|end|) |\"|q|\" |end|.|";
        assert_eq!(fwd(t), want);
        assert_eq!(back(t), want);
    }

    #[test]
    fn hyphens_apostrophes_numbers() {
        let t = "a-b -b a--b a - b a- b dogs' x 'q' 1,000.5 3:45 v1.2.3 $5 50% a/b x&y #tag a+b c++ end";
        assert_eq!(
            fwd(t),
            "|a-b |-|b |a-|-|b |a |- |b |a- |b |dogs|' |x |'q|' |1,000.5 |3:45 |v1.2.3 |$|5 |50|% |a|/|b |x|&|y |#|tag |a|+|b |c|++ |end|"
        );
        let t = "it’s x a?!b a.)b a),b 1.a a1.b 1.5.x 1-2 a1-b A-1 —x x— a's 'tis o'clock ‘q’ end";
        assert_eq!(fwd(t), "|it’s |x |a|?!|b |a|.)|b |a|),|b |1|.|a |a1|.|b |1.5|.|x |1-2 |a1-b |A-1 |—|x |x|— |a's |'tis |o'clock |‘|q|’ |end|");
    }

    #[test]
    fn marks_emoji_symbols() {
        let t = "café e\u{301}te 👍🏽 x 👨\u{200d}👩\u{200d}👧 y ½ ®z «q» “w” ¿no? ok—em dash–en x";
        let want = "|café |e\u{301}te |👍🏽 |x |👨\u{200d}👩\u{200d}👧 |y |½ |®|z |«|q|» |“|w|” |¿|no|? |ok|—|em |dash|–|en |x|";
        assert_eq!(fwd(t), want);
        assert_eq!(back(t), want);
        // A run of emoji is one token; emoji then punctuation is two going right, one going left.
        assert_eq!(fwd("👍👍 x 👍? x👍y"), "|👍👍 |x |👍|? |x|👍|y|");
        assert_eq!(back("👍👍 x 👍? x👍y"), "|👍👍 |x |👍? |x|👍|y|");
    }

    #[test]
    fn cjk_each_character() {
        let t = "日本語のテキストです。カタカナ漢字 abc 中文字符，标点。한국어 문장 a。。b";
        assert_eq!(fwd(t), "|日|本|語|の|テ|キ|ス|ト|で|す|。|カ|タ|カ|ナ|漢|字 |abc |中|文|字|符|，|标|点|。|한|국|어 |문|장 |a|。。|b|");
        assert_eq!(back(t), fwd(t));
    }

    #[test]
    fn rtl_words_in_logical_order() {
        let t = "Hello مرحبا بالعالم world שלום עולם end";
        assert_eq!(fwd(t), "|Hello |مرحبا |بالعالم |world |שלום |עולם |end|");
    }

    #[test]
    fn tabs_breaks_and_other_spaces() {
        assert_eq!(fwd("  lead x   mid  "), "|  |lead |x   |mid  |");
        assert_eq!(fwd("Five\u{b}six seven\tTab   x"), "|Five|↵|six |seven|→|Tab   |x|");
        assert_eq!(fwd("a\t  b x  \u{b}  y q\u{b}\u{b}z"), "|a|→|  |b |x  |↵|  |y |q|↵|↵|z|");
        let t = "a\u{a0}\u{a0}b a\u{3000}b a\u{2003}b a \u{a0}b a\t\tb a \tb a\u{a0} b end";
        assert_eq!(fwd(t), "|a\u{a0}\u{a0}|b |a\u{3000}|b |a\u{2003}|b |a \u{a0}|b |a|→|→|b |a |→|b |a\u{a0} |b |end|");
        assert_eq!(back(t), "|a|\u{a0}|\u{a0}|b |a|\u{3000}|b |a|\u{2003}|b |a |\u{a0}|b |a|→|→|b |a |→|b |a\u{a0} |b |end|");
    }

    #[test]
    fn next_and_prev() {
        let t = "  lead x   mid  ";
        assert_eq!(next_stop(t, 1), Some(2));
        assert_eq!(next_stop(t, 9), Some(11));
        assert_eq!(next_stop(t, 16), None);
        assert_eq!(prev_stop(t, 1), Some(0));
        assert_eq!(prev_stop(t, 9), Some(7));
        assert_eq!(prev_stop(t, 15), Some(11));
        assert_eq!(prev_stop(t, 0), None);
        assert_eq!(next_stop("", 0), None);
        assert_eq!(prev_stop("", 0), None);
    }

    #[test]
    fn double_click_word() {
        let t = "Hello world,  foo bar";
        assert_eq!(word_at(t, 7), (6, 11));
        assert_eq!(word_at(t, 15), (14, 17));
        assert_eq!(word_at(t, 11), (11, 12));
        assert_eq!(word_at(t, 12), (11, 12));
        assert_eq!(word_at(t, 13), (12, 14));
        assert_eq!(word_at(t, 21), (18, 21));
        assert_eq!(word_at("日本語のテキスト abc", 5), (5, 6));
        assert_eq!(word_at("", 3), (0, 0));
        assert_eq!(word_at("x👨\u{200d}👩\u{200d}👧y", 2), (1, 6));
    }

    #[test]
    fn delete_forward_extent() {
        let t = "ab   cd...ef gh";
        let ends: Vec<Option<usize>> = (0..=t.len()).map(|c| delete_forward_end(t, c)).collect();
        // Before smart spacing: PowerPoint's results after it are tested in `edit`.
        assert_eq!(
            ends,
            vec![
                Some(5),
                Some(2),
                Some(7),
                Some(5),
                Some(5),
                Some(7),
                Some(7),
                Some(10),
                Some(10),
                Some(10),
                Some(13),
                Some(12),
                Some(15),
                Some(15),
                Some(15),
                None
            ]
        );
        assert_eq!(delete_forward_end("ab\t cd", 2), Some(3));
        assert_eq!(delete_forward_end("ab  ", 2), Some(4));
    }
}
