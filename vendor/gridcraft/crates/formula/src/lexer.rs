//! Tokenizer for Excel formulas (A1 style, canonical or selected UI separators).
//!
//! Token spans are byte offsets into the source, which the editor uses to colour references.

use gridcraft_core::CellError;

use crate::ast::TrimMode;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Number(f64),
    Text(String),
    Bool(bool),
    Error(CellError),
    /// A reference-looking or name-looking word, possibly sheet-qualified: `A1`, `$B$2`, `A:A`
    /// is lexed as Word(A) Colon Word(A); `Sheet1!A1` → `Sheet(Some("Sheet1"))` then `Word`.
    Word(String),
    /// Sheet prefix `Sheet1!`, `'My Sheet'!`, `Sheet1:Sheet3!` (first, last).
    Sheet(String, Option<String>),
    /// Function name (word immediately followed by `(`), the `(` is consumed.
    Func(String),
    /// Structured reference body after a table name or bare: the raw text inside `[...]`.
    Struct(String),
    Op(&'static str),
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Colon,
    /// Trim reference range operator `.:`, `:.` or `.:.`.
    Trim(TrimMode),
    Space,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{msg} at {pos}")]
pub struct LexError {
    pub msg: String,
    pub pos: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\\' | '?')
}

/// `A1`, `$A$1`, `A`, `$A`, `1`, `$1`: a word that can end a range, so `A1.:` is a trim
/// reference and not a name ending in a dot.
fn is_range_end(w: &str) -> bool {
    let s = w.strip_prefix('$').unwrap_or(w);
    let (letters, rest) = s.split_at(s.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(s.len()));
    let digits = if letters.is_empty() { rest } else { rest.strip_prefix('$').unwrap_or(rest) };
    letters.len() <= 3 && if digits.is_empty() { !letters.is_empty() && rest.is_empty() } else { digits.bytes().all(|b| b.is_ascii_digit()) }
}

/// Whether `c` can start the second end of a range after `:.` (`A1:.E10`, `A:.$C`, `1:.5`).
fn starts_range_end(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '$')
}

fn byte_pos(src: &str, chars: &[(usize, char)], index: usize) -> usize {
    chars.get(index).map(|c| c.0).unwrap_or(src.len())
}

fn char_at(chars: &[(usize, char)], index: usize) -> Option<char> {
    chars.get(index).map(|c| c.1)
}

fn emit(src: &str, chars: &[(usize, char)], out: &mut Vec<Token>, tok: Tok, start: usize, end_index: usize) {
    out.push(Token { tok, start, end: byte_pos(src, chars, end_index) });
}

fn quoted(src: &str, chars: &[(usize, char)], i: usize, start: usize, out: &mut Vec<Token>) -> Result<usize, LexError> {
    let quote = char_at(chars, i).unwrap_or_default();
    let mut value = String::new();
    let mut j = i + 1;
    loop {
        match char_at(chars, j) {
            None => {
                let msg = if quote == '"' { "unterminated text" } else { "unterminated sheet name" };
                return Err(LexError { msg: msg.into(), pos: start });
            }
            Some(c) if c == quote && char_at(chars, j + 1) == Some(quote) => {
                value.push(quote);
                j += 2;
            }
            Some(c) if c == quote => {
                j += 1;
                break;
            }
            Some(c) => {
                value.push(c);
                j += 1;
            }
        }
    }
    if quote == '"' {
        emit(src, chars, out, Tok::Text(value), start, j);
        return Ok(j);
    }
    if char_at(chars, j) != Some('!') {
        return Err(LexError { msg: "expected ! after sheet name".into(), pos: byte_pos(src, chars, j) });
    }
    let (first, last) = match value.split_once(':') {
        Some((first, last)) => (first.to_string(), Some(last.to_string())),
        None => (value, None),
    };
    emit(src, chars, out, Tok::Sheet(first, last), start, j + 1);
    Ok(j + 1)
}

fn error_literal(src: &str, chars: &[(usize, char)], i: usize, start: usize, out: &mut Vec<Token>) -> Result<usize, LexError> {
    let rest = src.get(start..).unwrap_or("");
    let error = CellError::ALL
        .iter()
        .copied()
        .find(|error| rest.get(..error.as_str().len()).is_some_and(|text| text.eq_ignore_ascii_case(error.as_str())))
        .ok_or_else(|| LexError { msg: "unknown error literal".into(), pos: start })?;
    let end = i + error.as_str().chars().count();
    emit(src, chars, out, Tok::Error(error), start, end);
    Ok(end)
}

fn structured_ref(src: &str, chars: &[(usize, char)], i: usize, start: usize, out: &mut Vec<Token>) -> Result<usize, LexError> {
    let mut depth = 0;
    let mut j = i;
    loop {
        match char_at(chars, j) {
            None => return Err(LexError { msg: "unbalanced [".into(), pos: start }),
            Some('\'') => j += 2,
            Some('[') => {
                depth += 1;
                j += 1;
            }
            Some(']') => {
                depth -= 1;
                j += 1;
                if depth == 0 {
                    break;
                }
            }
            Some(_) => j += 1,
        }
    }
    let inner = src.get(byte_pos(src, chars, i + 1)..byte_pos(src, chars, j.saturating_sub(1))).unwrap_or("").to_string();
    emit(src, chars, out, Tok::Struct(inner), start, j);
    Ok(j)
}

fn number(src: &str, chars: &[(usize, char)], locale: crate::FormulaLocale, i: usize, start: usize, out: &mut Vec<Token>) -> Result<usize, LexError> {
    let ch = |index| char_at(chars, index);
    let mut j = i;
    while ch(j).is_some_and(|c| c.is_ascii_digit()) {
        j += 1;
    }
    let is_row_ref = {
        let mut k = j;
        if ch(k) == Some('.') && ch(k + 1) == Some(':') {
            k += 1;
        }
        ch(k) == Some(':') && {
            k += 1;
            if ch(k) == Some('.') {
                k += 1;
            }
            if ch(k) == Some('$') {
                k += 1;
            }
            ch(k).is_some_and(|c| c.is_ascii_digit())
        }
    };
    if is_row_ref {
        let word = chars.get(i..j).map(|s| s.iter().map(|c| c.1).collect()).unwrap_or_default();
        emit(src, chars, out, Tok::Word(word), start, j);
        return Ok(j);
    }
    if ch(j) == Some(locale.decimal_separator()) {
        j += 1;
        while ch(j).is_some_and(|c| c.is_ascii_digit()) {
            j += 1;
        }
    }
    if matches!(ch(j), Some('e' | 'E')) {
        let mut exponent = j + 1;
        if matches!(ch(exponent), Some('+' | '-')) {
            exponent += 1;
        }
        if ch(exponent).is_some_and(|c| c.is_ascii_digit()) {
            while ch(exponent).is_some_and(|c| c.is_ascii_digit()) {
                exponent += 1;
            }
            j = exponent;
        }
    }
    let text = src.get(start..byte_pos(src, chars, j)).unwrap_or("");
    let value: f64 =
        text.replace(locale.decimal_separator(), ".").parse().map_err(|_| LexError { msg: format!("bad number `{text}`"), pos: start })?;
    if !value.is_finite() {
        return Err(LexError { msg: format!("number too large `{text}`"), pos: start });
    }
    emit(src, chars, out, Tok::Number(value), start, j);
    Ok(j)
}

fn word(src: &str, chars: &[(usize, char)], locale: crate::FormulaLocale, array_depth: usize, i: usize, start: usize, out: &mut Vec<Token>) -> usize {
    let ch = |index| char_at(chars, index);
    let mut j = i;
    while ch(j).is_some_and(|c| is_word_char(c) && !(array_depth > 0 && c == locale.array_column_separator())) {
        j += 1;
    }
    let value: String = chars.get(i..j).map(|s| s.iter().map(|c| c.1).collect()).unwrap_or_default();
    if ch(j) == Some('!') {
        emit(src, chars, out, Tok::Sheet(value, None), start, j + 1);
        j + 1
    } else if ch(j) == Some(':') && sheet_range_end(chars, locale, array_depth, &value, j, out) {
        let mut end = j + 1;
        while ch(end).is_some_and(|c| is_word_char(c) && !(array_depth > 0 && c == locale.array_column_separator())) {
            end += 1;
        }
        let last: String = chars.get(j + 1..end).map(|s| s.iter().map(|c| c.1).collect()).unwrap_or_default();
        emit(src, chars, out, Tok::Sheet(value, Some(last)), start, end + 1);
        end + 1
    } else if ch(j) == Some('(') {
        emit(src, chars, out, Tok::Func(value), start, j + 1);
        j + 1
    } else if value.eq_ignore_ascii_case("TRUE") && ch(j) != Some('[') {
        emit(src, chars, out, Tok::Bool(true), start, j);
        j
    } else if value.eq_ignore_ascii_case("FALSE") && ch(j) != Some('[') {
        emit(src, chars, out, Tok::Bool(false), start, j);
        j
    } else if value.len() > 1 && value.ends_with('.') && ch(j) == Some(':') && is_range_end(value.get(..value.len() - 1).unwrap_or("")) {
        emit(src, chars, out, Tok::Word(value[..value.len() - 1].to_string()), start, j - 1);
        j - 1
    } else {
        emit(src, chars, out, Tok::Word(value), start, j);
        j
    }
}

fn sheet_range_end(chars: &[(usize, char)], locale: crate::FormulaLocale, array_depth: usize, value: &str, colon: usize, out: &[Token]) -> bool {
    let ch = |index| char_at(chars, index);
    let mut end = colon + 1;
    while ch(end).is_some_and(|c| is_word_char(c) && !(array_depth > 0 && c == locale.array_column_separator())) {
        end += 1;
    }
    let repeats_sheet = is_range_end(value)
        && matches!(out.last(), Some(Token { tok: Tok::Sheet(previous, None), .. })
            if chars.get(colon + 1..end).is_some_and(|s| crate::same_name(&s.iter().map(|c| c.1).collect::<String>(), previous)));
    end > colon + 1 && ch(end) == Some('!') && !repeats_sheet
}

/// Tokenizes formula text (without the leading `=`).
pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    tokenize_locale(src, crate::FormulaLocale::En)
}

/// Tokenizes UI syntax while retaining byte spans in the original text.
pub fn tokenize_locale(src: &str, locale: crate::FormulaLocale) -> Result<Vec<Token>, LexError> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let n = chars.len();
    let pos_of = |i: usize| chars.get(i).map(|c| c.0).unwrap_or(src.len());
    let ch = |i: usize| chars.get(i).map(|c| c.1);
    let mut out = Vec::new();
    let mut i = 0;
    let mut array_depth = 0usize;
    while i < n {
        let start = pos_of(i);
        let Some(c) = ch(i) else { break };
        let push = |out: &mut Vec<Token>, tok: Tok, end_i: usize| out.push(Token { tok, start, end: pos_of(end_i) });
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                let mut j = i;
                while matches!(ch(j), Some(' ' | '\t' | '\n' | '\r')) {
                    j += 1;
                }
                push(&mut out, Tok::Space, j);
                i = j;
            }
            '"' | '\'' => i = quoted(src, &chars, i, start, &mut out)?,
            '#' if out.last().is_some_and(|t: &Token| t.end == start && matches!(t.tok, Tok::Word(_))) => {
                // Spill range operator right after a reference: `A1#`.
                push(&mut out, Tok::Op("#"), i + 1);
                i += 1;
            }
            '#' => {
                i = error_literal(src, &chars, i, start, &mut out)?;
            }
            '[' => {
                i = structured_ref(src, &chars, i, start, &mut out)?;
            }
            '.' if ch(i + 1) == Some(':') => {
                // Trim reference `A1.:E10` / `A1.:.E10` (the word before stopped at the dot).
                if ch(i + 2) == Some('.') && starts_range_end(ch(i + 3)) {
                    push(&mut out, Tok::Trim(TrimMode::All), i + 3);
                    i += 3;
                } else {
                    push(&mut out, Tok::Trim(TrimMode::Leading), i + 2);
                    i += 2;
                }
            }
            c if c.is_ascii_digit() || (c == locale.decimal_separator() && ch(i + 1).is_some_and(|d| d.is_ascii_digit())) => {
                i = number(src, &chars, locale, i, start, &mut out)?;
            }
            '(' => {
                push(&mut out, Tok::LParen, i + 1);
                i += 1;
            }
            ')' => {
                push(&mut out, Tok::RParen, i + 1);
                i += 1;
            }
            '{' => {
                array_depth += 1;
                push(&mut out, Tok::LBrace, i + 1);
                i += 1;
            }
            '}' => {
                array_depth = array_depth.saturating_sub(1);
                push(&mut out, Tok::RBrace, i + 1);
                i += 1;
            }
            c if (array_depth == 0 && c == locale.list_separator()) || (array_depth > 0 && c == locale.array_column_separator()) => {
                push(&mut out, Tok::Comma, i + 1);
                i += 1;
            }
            ';' => {
                push(&mut out, Tok::Semicolon, i + 1);
                i += 1;
            }
            ':' => {
                if ch(i + 1) == Some('.') && starts_range_end(ch(i + 2)) {
                    push(&mut out, Tok::Trim(TrimMode::Trailing), i + 2);
                    i += 2;
                } else {
                    push(&mut out, Tok::Colon, i + 1);
                    i += 1;
                }
            }
            '+' | '-' | '*' | '/' | '^' | '&' | '%' | '@' | '=' => {
                let op = match c {
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    '^' => "^",
                    '&' => "&",
                    '%' => "%",
                    '@' => "@",
                    _ => "=",
                };
                push(&mut out, Tok::Op(op), i + 1);
                i += 1;
            }
            '<' | '>' => {
                let two = ch(i + 1);
                let (op, len) = match (c, two) {
                    ('<', Some('=')) => ("<=", 2),
                    ('<', Some('>')) => ("<>", 2),
                    ('>', Some('=')) => (">=", 2),
                    ('<', _) => ("<", 1),
                    _ => (">", 1),
                };
                push(&mut out, Tok::Op(op), i + len);
                i += len;
            }
            c if is_word_char(c) => i = word(src, &chars, locale, array_depth, i, start, &mut out),
            other => return Err(LexError { msg: format!("unexpected character `{other}`"), pos: start }),
        }
    }
    out.push(Token { tok: Tok::Eof, start: src.len(), end: src.len() });
    Ok(out)
}
