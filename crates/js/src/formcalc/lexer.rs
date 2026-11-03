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
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err(format!("line {line}: unterminated string")),
                        Some('"') if chars.get(i + 1) == Some(&'"') => {
                            s.push('"');
                            i += 2;
                        }
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some(&ch) => {
                            if ch == '\n' {
                                line += 1;
                            }
                            s.push(ch);
                            i += 1;
                        }
                    }
                    if s.len() > MAX_STRING {
                        return Err(format!("line {line}: a string is too long"));
                    }
                }
                out.push(Token { tok: Tok::Str(s), line });
            }
            c if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1) != Some(&'.'))) {
                    i += 1;
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let mut j = i + 1;
                    if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].is_ascii_digit() {
                        i = j;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let text: String = chars[start..i].iter().collect();
                let n: f64 = text.parse().map_err(|_| format!("line {line}: bad number {text}"))?;
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
