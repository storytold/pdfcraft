//! Syntax colouring for the script editors: [`highlight`] is a fast, forgiving tokenizer for colouring
//! text as it is typed. It never fails, it only has to be good enough to colour, and it runs neither
//! the script nor the engine's parser, so showing a script from any document reaches neither.

use std::ops::Range;

/// What a stretch of script is, for colouring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// A reserved word: `var`, `function`, `if`, `return`, `this` …
    Keyword,
    /// `true`, `false`, `null`, `undefined`, `NaN`, `Infinity`.
    Literal,
    /// A number.
    Number,
    /// A quoted string.
    String,
    /// A template literal's text (the code in its `${…}` is coloured as code).
    Template,
    /// A regular expression literal.
    Regex,
    /// A comment.
    Comment,
    /// A name from the Acrobat object model: `event`, `app`, `util`, `console`, `getField` …
    Api,
}

/// A coloured stretch of a script: `bytes` (on character boundaries) is `kind`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub bytes: Range<usize>,
}

const KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

const LITERALS: &[&str] = &["true", "false", "null", "undefined", "NaN", "Infinity"];

/// Globals PdfCraft provides from Acrobat's object model (see the crate documentation).
pub(crate) const API: &[&str] = &["app", "color", "console", "display", "event", "getField", "util"];

/// Keywords after which a `/` starts a regular expression rather than dividing.
const BEFORE_REGEX: &[&str] =
    &["return", "typeof", "case", "do", "else", "in", "of", "new", "delete", "void", "throw", "instanceof", "yield", "await"];

/// Deepest `${` nesting of template literals tracked; deeper ones are coloured as template text.
const MAX_TEMPLATE_DEPTH: usize = 64;

/// The coloured stretches of `source`, in order and not overlapping; text between them (names,
/// operators, white space) is plain. Never fails: unclosed strings, comments and templates run
/// to the end of their line or of the source.
pub fn highlight(source: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        s: source,
        i: 0,
        out: Vec::new(),
        regex_ok: true,
        after_dot: false,
        braces: Vec::new(),
        keyword_paren: false,
        parens: Vec::new(),
        line_start: false,
    };
    lexer.run();
    lexer.out
}

struct Lexer<'a> {
    s: &'a str,
    /// The next byte to read; always on a character boundary.
    i: usize,
    out: Vec<Token>,
    /// Whether a `/` here starts a regular expression.
    regex_ok: bool,
    /// Whether the last token was a `.` (a name after it is a property, never a keyword).
    after_dot: bool,
    /// For each open `${`, the `{` opened inside it and not yet closed.
    braces: Vec<usize>,
    /// Whether the last token was `if`, `while`, `for` or `with`, whose `(…)` a regular
    /// expression may follow (`if (x) /re/.test(s)`), unlike other brackets' (`(a + b) / 2`).
    keyword_paren: bool,
    /// For each open `(`, whether it was such a keyword's.
    parens: Vec<bool>,
    /// Whether only white space and comments stand between here and a line end before it, where
    /// `-->` starts a comment. Not on the script's first line, where the parser reads `--` and `>`.
    line_start: bool,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i..).and_then(|r| r.chars().next())
    }

    fn peek2(&self) -> Option<char> {
        self.s.get(self.i..).and_then(|r| r.chars().nth(1))
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i = self.i.saturating_add(c.len_utf8());
        Some(c)
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        if self.i > start {
            self.out.push(Token { kind, bytes: start..self.i });
        }
    }

    fn at(&self, text: &str) -> bool {
        self.s.get(self.i..).is_some_and(|r| r.starts_with(text))
    }

    fn run(&mut self) {
        if self.at("#!") {
            // A hashbang, only at the very start. The parser takes the line end with it, so a
            // `-->` on the next line is not at a line start to it.
            self.skip_to_line_end();
            self.push(TokenKind::Comment, 0);
            self.bump();
        }
        while let Some(c) = self.peek() {
            let start = self.i;
            if c == '/' && self.peek2() == Some('/') {
                self.skip_to_line_end();
                self.push(TokenKind::Comment, start);
            } else if c == '/' && self.peek2() == Some('*') {
                self.i = self
                    .s
                    .get(start.saturating_add(2)..)
                    .and_then(|r| r.find("*/"))
                    .map_or(self.s.len(), |end| start.saturating_add(2).saturating_add(end).saturating_add(2));
                self.push(TokenKind::Comment, start);
                // A comment that spans lines ends a line.
                self.line_start |= self.s.get(start..self.i).is_some_and(|comment| comment.contains(is_line_end));
            } else if self.at("<!--") || (self.line_start && self.at("-->")) {
                // HTML-like comments, as the parser reads a script (ECMAScript Annex B.1.1).
                self.skip_to_line_end();
                self.push(TokenKind::Comment, start);
            } else if c == '/' && self.regex_ok {
                self.regex();
                self.push(TokenKind::Regex, start);
                self.after(false, false);
            } else if c == '\'' || c == '"' {
                self.bump();
                self.string(c);
                self.push(TokenKind::String, start);
                self.after(false, false);
            } else if c == '`' {
                self.bump();
                self.template(start);
            } else if self.braces.last() == Some(&0) && c == '}' {
                // The end of a template's `${…}`: back into its text.
                self.braces.pop();
                self.bump();
                self.template(start);
            } else if c.is_ascii_digit() || (c == '.' && self.peek2().is_some_and(|d| d.is_ascii_digit())) {
                self.number();
                self.push(TokenKind::Number, start);
                self.after(false, false);
            } else if is_name_start(c) {
                self.word(start);
            } else if matches!(c, '+' | '-') && self.peek2() == Some(c) {
                // An increment leaves a `/` meaning what it did before it: after a value it
                // divides (`i++ / 2`), and before one it starts a regular expression.
                self.bump();
                self.bump();
                self.after(self.regex_ok, false);
            } else {
                self.bump();
                // Whether a `/` after `c` starts a regular expression: not after a value's
                // closing bracket, but after a keyword's `(…)` and after a block's `}`.
                let mut regex_ok = c != ']';
                match c {
                    '{' => {
                        if let Some(open) = self.braces.last_mut() {
                            *open = open.saturating_add(1);
                        }
                    }
                    '}' => {
                        if let Some(open) = self.braces.last_mut() {
                            *open = open.saturating_sub(1);
                        }
                    }
                    '(' => self.parens.push(self.keyword_paren),
                    ')' => regex_ok = self.parens.pop().unwrap_or(false),
                    _ => {}
                }
                if is_line_end(c) {
                    self.line_start = true;
                } else if !c.is_whitespace() {
                    self.after(regex_ok, c == '.');
                }
            }
        }
    }

    fn after(&mut self, regex_ok: bool, after_dot: bool) {
        self.regex_ok = regex_ok;
        self.after_dot = after_dot;
        self.keyword_paren = false;
        self.line_start = false;
    }

    fn skip_to_line_end(&mut self) {
        while self.peek().is_some_and(|c| !is_line_end(c)) {
            self.bump();
        }
    }

    /// The rest of a string opened by `quote`, to its close or the end of the line. U+2028 and
    /// U+2029 end a line but not a string (ECMAScript 2019).
    fn string(&mut self, quote: char) {
        while let Some(c) = self.peek() {
            if matches!(c, '\n' | '\r') {
                return;
            }
            self.bump();
            if c == '\\' {
                // An escaped character, or a line continuation (`\r\n` counts as one).
                if self.bump() == Some('\r') && self.peek() == Some('\n') {
                    self.bump();
                }
            } else if c == quote {
                return;
            }
        }
    }

    /// A regular expression: to its closing `/` outside a class, then its flags.
    fn regex(&mut self) {
        self.bump();
        let mut class = false;
        while let Some(c) = self.peek() {
            if is_line_end(c) {
                return;
            }
            self.bump();
            match c {
                '\\' => {
                    if self.peek().is_some_and(|c| !is_line_end(c)) {
                        self.bump();
                    }
                }
                '[' => class = true,
                ']' => class = false,
                '/' if !class => break,
                _ => {}
            }
        }
        while self.peek().is_some_and(is_name_part) {
            self.bump();
        }
    }

    /// Template text from `start` (just past its opening `` ` `` or `}`): to the closing `` ` ``,
    /// or to a `${`, after which the lexer reads code until the matching `}`.
    fn template(&mut self, start: usize) {
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    self.bump();
                }
                '`' => {
                    self.push(TokenKind::Template, start);
                    self.after(false, false);
                    return;
                }
                '$' if self.peek() == Some('{') && self.braces.len() < MAX_TEMPLATE_DEPTH => {
                    self.bump();
                    self.push(TokenKind::Template, start);
                    self.braces.push(0);
                    self.after(true, false);
                    return;
                }
                _ => {}
            }
        }
        self.push(TokenKind::Template, start);
    }

    fn number(&mut self) {
        let hex = self.peek() == Some('0') && matches!(self.peek2(), Some('x' | 'X'));
        let mut last = '\0';
        while let Some(c) = self.peek() {
            let sign = matches!(c, '+' | '-') && matches!(last, 'e' | 'E') && !hex;
            if !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || sign) {
                break;
            }
            last = c;
            self.bump();
        }
    }

    fn word(&mut self, start: usize) {
        while self.peek().is_some_and(is_name_part) {
            self.bump();
        }
        let word = self.s.get(start..self.i).unwrap_or_default();
        if self.after_dot {
            // A property: `x.default`, `this.event` …
            self.after(false, false);
            return;
        }
        let kind = if KEYWORDS.contains(&word) {
            Some(TokenKind::Keyword)
        } else if LITERALS.contains(&word) {
            Some(TokenKind::Literal)
        } else if API.contains(&word) {
            Some(TokenKind::Api)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.push(kind, start);
        }
        self.after(BEFORE_REGEX.contains(&word), false);
        self.keyword_paren = matches!(word, "if" | "while" | "for" | "with");
    }
}

fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_name_start(c: char) -> bool {
    c == '$' || c == '_' || c.is_alphabetic()
}

fn is_name_part(c: char) -> bool {
    is_name_start(c) || c.is_alphanumeric() || c == '\u{200C}' || c == '\u{200D}'
}

#[cfg(test)]
mod tests;
