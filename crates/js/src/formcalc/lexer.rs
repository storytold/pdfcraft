use super::MAX_STRING;

// ── lexer ───────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(&'static str),
    Newline,
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Token {
    pub(super) tok: Tok,
    pub(super) line: usize,
}

const OPS: [&str; 20] = ["<=", ">=", "==", "<>", "..", "(", ")", "[", "]", ".", ",", "=", "<", ">", "+", "-", "*", "/", "&", "|"];

pub(super) fn lex(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut depth = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
                // Newlines inside brackets continue the expression.
                if depth == 0 {
                    out.push(Token { tok: Tok::Newline, line });
                }
            }
            ' ' | '\t' | '\r' => i += 1,
            ';' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                let tok_line = line;
                let s = lex_string(&chars, &mut i, &mut line)?;
                out.push(Token { tok: Tok::Str(s), line: tok_line });
            }
            c if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) => {
                let n = lex_number(&chars, &mut i, line)?;
                out.push(Token { tok: Tok::Num(n), line });
            }
            '!' => {
                out.push(Token { tok: Tok::Ident("!".into()), line });
                i += 1;
            }
            c if c.is_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                out.push(Token { tok: Tok::Ident(text), line });
            }
            _ => {
                let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
                let op = OPS.iter().find(|o| rest.starts_with(*o)).copied().ok_or_else(|| format!("line {line}: unexpected character {c:?}"))?;
                match op {
                    "(" | "[" => depth += 1,
                    ")" | "]" => depth = depth.saturating_sub(1),
                    _ => {}
                }
                out.push(Token { tok: Tok::Op(op), line });
                i += op.len();
            }
        }
    }
    out.push(Token { tok: Tok::Newline, line });
    out.push(Token { tok: Tok::Eof, line });
    Ok(out)
}

fn hex_u32(digits: &[char]) -> Option<u32> {
    let mut v = 0u32;
    for &c in digits {
        v = (v << 4) | c.to_digit(16)?;
    }
    Some(v)
}

/// FormCalc string escape (`\uXXXX` or `\UXXXXXXXX`, XFA 3.3 part 8 p. 1054); any other `\` stays literal.
fn unicode_escape(chars: &[char], at: usize) -> Option<(char, usize)> {
    if chars.get(at) != Some(&'\\') {
        return None;
    }
    match chars.get(at + 1)? {
        'u' => {
            let hi = hex_u32(chars.get(at + 2..at + 6)?)?;
            if (0xD800..=0xDBFF).contains(&hi)
                && chars.get(at + 6..at + 8) == Some(&['\\', 'u'])
                && let Some(lo) = chars.get(at + 8..at + 12).and_then(hex_u32).filter(|lo| (0xDC00..=0xDFFF).contains(lo))
            {
                let scalar = 0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                return Some((char::from_u32(scalar)?, 12));
            }
            Some((char::from_u32(hi)?, 6))
        }
        'U' => {
            let code = hex_u32(chars.get(at + 2..at + 10)?)?;
            Some((char::from_u32(code)?, 10))
        }
        _ => None,
    }
}

fn lex_string(chars: &[char], i: &mut usize, line: &mut usize) -> Result<String, String> {
    let mut s = String::new();
    *i += 1;
    loop {
        match chars.get(*i) {
            None => return Err(format!("line {line}: unterminated string")),
            Some('"') if chars.get(*i + 1) == Some(&'"') => {
                s.push('"');
                *i += 2;
            }
            Some('"') => {
                *i += 1;
                return Ok(s);
            }
            Some(&ch) => {
                if let Some((decoded, step)) = unicode_escape(chars, *i) {
                    s.push(decoded);
                    *i += step;
                } else {
                    if ch == '\n' {
                        *line += 1;
                    }
                    s.push(ch);
                    *i += 1;
                }
            }
        }
        if s.len() > MAX_STRING {
            return Err(format!("line {line}: a string is too long"));
        }
    }
}

/// Whether `c` may start a FormCalc identifier.
fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

/// End index of a numeric exponent starting at `at` (`e`/`E`, optional sign, digits), or `None`
/// if no valid exponent starts there or if identifier characters immediately follow the digits.
fn exponent_end(chars: &[char], at: usize) -> Option<usize> {
    if !matches!(chars.get(at), Some('e' | 'E')) {
        return None;
    }
    let mut j = at + 1;
    if matches!(chars.get(j), Some('+' | '-')) {
        j += 1;
    }
    if !chars.get(j).is_some_and(char::is_ascii_digit) {
        return None;
    }
    while chars.get(j).is_some_and(char::is_ascii_digit) {
        j += 1;
    }
    if chars.get(j).is_some_and(|&c| c.is_alphanumeric() || c == '_' || c == '$') {
        return None;
    }
    Some(j)
}

/// Scan a FormCalc number literal starting at `*i`, stopping before `.member` or `..` while
/// rejecting malformed multi-dot literals such as `1.2.3`.
fn lex_number(chars: &[char], i: &mut usize, line: usize) -> Result<f64, String> {
    let start = *i;
    let mut seen_dot = false;
    while *i < chars.len() {
        let next = chars.get(*i + 1).copied();
        let next_is_member = next.is_some_and(is_ident_start) && exponent_end(chars, *i + 1).is_none();
        if chars[*i].is_ascii_digit() {
            *i += 1;
        } else if chars[*i] == '.' && !seen_dot && next != Some('.') && !next_is_member {
            seen_dot = true;
            *i += 1;
        } else if chars[*i] == '.' && seen_dot && next.is_some_and(|c| c.is_ascii_digit()) {
            return Err(format!("line {line}: bad number"));
        } else {
            break;
        }
    }
    if let Some(end) = exponent_end(chars, *i) {
        *i = end;
    }
    let text: String = chars[start..*i].iter().collect();
    text.parse().map_err(|_| format!("line {line}: bad number {text}"))
}
