//! Syntax colouring for the script editors: [`highlight`] is a fast, forgiving tokenizer for colouring
//! text as it is typed. It never fails, it only has to be good enough to colour, and it runs neither
//! the script nor the engine's parser, so showing a script from any document reaches neither.
//!
//! A wrong colour is only that, but editing mustn't change what a script holds on a guess, so what
//! it asks ([`in_literal`]) stops trusting the colouring at the first thing the lexer can't be sure
//! of: a `/` that could go either way, a `\`, or a character that is no part of a script's code.

use std::borrow::Cow;
use std::ops::Range;

use icu_properties::CodePointSetData;
use icu_properties::props::{IdContinue, IdStart};

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

/// Globals coloured as literals. Unlike `true`, `false` and `null`, they are names, so a script may
/// spell them with escapes.
const VALUES: &[&str] = &["undefined", "NaN", "Infinity"];

/// Globals PdfCraft provides from Acrobat's object model (see the crate documentation).
pub(crate) const API: &[&str] = &["app", "color", "console", "display", "event", "getField", "util"];

/// Deepest `${` nesting of template literals tracked; deeper ones are coloured as template text, and
/// the lexer is unsure past them.
const MAX_TEMPLATE_DEPTH: usize = 64;

/// The coloured stretches of `source`, in order and not overlapping; text between them (names,
/// operators, white space) is plain. Never fails: unclosed strings, comments and templates run
/// to the end of their line or of the source.
pub fn highlight(source: &str) -> Vec<Token> {
    lex(source).0
}

/// Whether byte `at` of `source` may be inside a string or template, where white space is what the
/// script holds. After its closing quote, or between `${` and `}`, it isn't; at the end of one never
/// closed, it is.
///
/// Without a parser, a `/` after a `}`, after a word that may be a name, or at the start of a line,
/// can start a regular expression or divide: `{ valueOf() { return 6; } } / 2` is valid, and so is
/// `var x` with `/re/.test(s)` on the next line. Read the wrong way, it hides what
/// follows on its line, a template's opening `` ` `` among them, or turns a `/*` in a regular
/// expression into a comment to the end. So past the first such `/`, this is always true: it errs
/// towards leaving the text alone rather than guess. So it is past a `\` in code, as the parser
/// refuses many names spelled with escapes (`\u0072eturn`, `\u0020`) and any other `\` there, and past
/// a character that is no part of a script's code (`@`, `²`): what the script means there is a guess
/// too. So is the code in a `${…}` nested deeper than the lexer tracks.
pub fn in_literal(source: &str, at: usize) -> bool {
    let (tokens, doubt) = lex(source);
    doubt.is_some_and(|unsure| unsure < at)
        || tokens.iter().filter(|t| matches!(t.kind, TokenKind::String | TokenKind::Template)).any(|t| {
            let token = source.get(t.bytes.clone()).unwrap_or_default();
            let open_at_its_end = match t.kind {
                // A piece of a template starts with `` ` `` or `}`, and ends with `` ` `` or `${`.
                TokenKind::Template => !(closed_by(token, "${") || closed_by(token, "`")),
                _ => !token.get(..1).is_some_and(|quote| closed_by(token, quote)),
            };
            t.bytes.start < at && (at < t.bytes.end || (at == t.bytes.end && open_at_its_end))
        })
}

/// Whether `token` ends with `end`, when that is neither all it holds nor escaped.
fn closed_by(token: &str, end: &str) -> bool {
    token.strip_suffix(end).is_some_and(|rest| !rest.is_empty() && rest.chars().rev().take_while(|c| *c == '\\').count().is_multiple_of(2))
}

/// The tokens of `source`, and where the lexer first stops being sure of them (see [`Lexer::doubt`]).
fn lex(source: &str) -> (Vec<Token>, Option<usize>) {
    let mut lexer = Lexer {
        s: source,
        i: 0,
        out: Vec::new(),
        slash: Slash::StartsRegex,
        next: Next::Any,
        braces: Vec::new(),
        parens: Vec::new(),
        line_start: false,
        doubt: None,
    };
    lexer.run();
    (lexer.out, lexer.doubt)
}

/// What a `/` is at some point of a script, which only the parser can always tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slash {
    /// It starts a regular expression: what came before it is no value.
    StartsRegex,
    /// It divides the value before it.
    Divides,
    /// It may do either: what came before it may be a value or not, as an object's `}` or a block's,
    /// or `yield` as a name or a keyword. The lexer reads it as starting a regular expression, and is
    /// unsure from it on.
    Either,
}

/// What the last token makes of the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    /// Nothing in particular.
    Any,
    /// After a `.`, a name is a property's, never a keyword or a global: `x.default`, `this.event`.
    Property,
    /// After `break` or `continue`, a name on their line is a label, after which a `/` starts a
    /// regular expression as after the keyword alone.
    Label,
    /// After `if`, `while`, `for` (or `for await`) or `with`, a `(` opens brackets that a regular
    /// expression may follow (`if (x) /re/.test(s)`), unlike other brackets (`(a + b) / 2`).
    Condition,
}

/// A word of the language, as the lexer reads it.
#[derive(Clone, Copy)]
struct Word {
    colour: Option<TokenKind>,
    /// What a `/` right after it is.
    slash: Slash,
    next: Next,
}

/// What `word` is, spelled plainly: its colour, what a `/` right after it is, and what it makes of the
/// next token. `None` for a name, which is coloured only as a global, and after which a `/` divides.
/// Spelled with escapes, no word is one of these: the parser refuses a keyword so spelled, or reads it
/// as a name, and the same for `true`, `false` and `null`.
fn known(word: &str) -> Option<Word> {
    let keyword = |slash, next| Some(Word { colour: Some(TokenKind::Keyword), slash, next });
    match word {
        // No value ends with these, so a `/` after them starts a regular expression.
        "return" | "typeof" | "case" | "do" | "else" | "in" | "new" | "delete" | "void" | "throw" | "instanceof" | "extends" => {
            keyword(Slash::StartsRegex, Next::Any)
        }
        // A line end ends these, so a `/` on the next line starts a statement; on their own line, it
        // would be a syntax error. After `break` and `continue`, a name on their line is a label.
        "debugger" => keyword(Slash::StartsRegex, Next::Any),
        "break" | "continue" => keyword(Slash::StartsRegex, Next::Label),
        // A statement follows their `(…)`.
        "if" | "while" | "for" | "with" => keyword(Slash::Divides, Next::Condition),
        // Keywords in generators and async functions, and names elsewhere.
        "await" | "yield" => keyword(Slash::Either, Next::Any),
        // A value (`this / 2`) or a name (`let / 2`). A `/` can follow the others only on the next
        // line, where the lexer is unsure of any `/` after a word.
        "async" | "catch" | "class" | "const" | "default" | "enum" | "export" | "finally" | "function" | "import" | "let" | "static" | "super"
        | "switch" | "this" | "try" | "var" => keyword(Slash::Divides, Next::Any),
        "true" | "false" | "null" => Some(Word { colour: Some(TokenKind::Literal), slash: Slash::Divides, next: Next::Any }),
        // A keyword only in `for (x of y)`, so not coloured as one, and a name elsewhere.
        "of" => Some(Word { colour: None, slash: Slash::Either, next: Next::Any }),
        _ => None,
    }
}

/// How the global `name` is coloured, or `None` if it isn't one the lexer colours.
fn global(name: &str) -> Option<TokenKind> {
    if VALUES.contains(&name) {
        Some(TokenKind::Literal)
    } else if API.contains(&name) {
        Some(TokenKind::Api)
    } else {
        None
    }
}

struct Lexer<'a> {
    s: &'a str,
    /// The next byte to read; always on a character boundary.
    i: usize,
    out: Vec<Token>,
    /// What a `/` here is.
    slash: Slash,
    /// What the last token makes of the next.
    next: Next,
    /// For each open `${`, the `{` opened inside it and not yet closed.
    braces: Vec<usize>,
    /// For each open `(`, whether it was a condition's (see [`Next::Condition`]).
    parens: Vec<bool>,
    /// Whether only white space and comments stand between here and a line end before it, where
    /// `-->` starts a comment, a `++` is the next value's, and a `/` after a value may start a regular
    /// expression. Not on the script's first line, where the parser reads `--` and `>`.
    line_start: bool,
    /// Where the lexer first stops being sure of what it reads: a `/` that could go either way, a `\`,
    /// or a character that is no part of a script's code.
    doubt: Option<usize>,
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
            } else if c == '/' && self.slash != Slash::Divides {
                if self.slash == Slash::Either {
                    self.unsure(start);
                }
                self.regex();
                self.push(TokenKind::Regex, start);
                self.after(Slash::Divides);
            } else if c == '\'' || c == '"' {
                self.bump();
                self.string(c);
                self.push(TokenKind::String, start);
                self.after(Slash::Divides);
            } else if c == '`' {
                self.bump();
                self.template(start);
            } else if self.braces.last() == Some(&0) && c == '}' {
                // The end of a template's `${…}`: back into its text.
                self.braces.pop();
                self.bump();
                self.template(start);
            } else if self.at("...") || self.at("<<") {
                // Read whole, as the parser does: the `.1` in `...1.` is no number, and the `<!--` in
                // `<<!--` no comment. A value follows either.
                for _ in 0..if c == '.' { 3 } else { 2 } {
                    self.bump();
                }
                self.after(Slash::StartsRegex);
            } else if c.is_ascii_digit() || (c == '.' && self.peek2().is_some_and(|d| d.is_ascii_digit())) {
                self.number();
                self.push(TokenKind::Number, start);
                self.after(Slash::Divides);
            } else if is_name_start(c) || self.at("\\u") {
                self.word(start);
            } else if c == '#' && self.peek2().is_some_and(is_name_start) {
                // A private name, `#count`: never a keyword or a global.
                self.bump();
                while self.name_part() {}
                self.after(Slash::Divides);
            } else if matches!(c, '+' | '-') && self.peek2() == Some(c) {
                // An increment on the line of a value is that value's, and leaves a `/` dividing
                // (`i++ / 2`). Past a line end it is the next value's, since none may come between a
                // value and its `++`, and so is it where no value came before: a `/` after it starts a
                // regular expression (`++/re/.lastIndex`).
                self.bump();
                self.bump();
                self.after(if self.line_start { Slash::StartsRegex } else { self.slash });
            } else if is_line_end(c) {
                self.bump();
                self.line_start = true;
            } else if is_space(c) {
                self.bump();
            } else {
                self.bump();
                let slash = match c {
                    '{' => {
                        if let Some(open) = self.braces.last_mut() {
                            *open = open.saturating_add(1);
                        }
                        Slash::StartsRegex
                    }
                    // A block's or an object's: `{}\n/re/.test(s)` and `x = {} / 2` are both valid.
                    '}' => {
                        if let Some(open) = self.braces.last_mut() {
                            *open = open.saturating_sub(1);
                        }
                        Slash::Either
                    }
                    '(' => {
                        self.parens.push(self.next == Next::Condition);
                        Slash::StartsRegex
                    }
                    // A statement follows a condition's brackets, and an operator other brackets.
                    ')' => {
                        if self.parens.pop().unwrap_or(false) {
                            Slash::StartsRegex
                        } else {
                            Slash::Divides
                        }
                    }
                    ']' => Slash::Divides,
                    // Dividing, as far as the lexer can tell. But past a line end, the parser ends a
                    // statement before a `/` that can't continue it, which then starts a regular
                    // expression (`var x`, and `/re/.test(s)` on the next line), and the engine does so
                    // after `let`, `async` and `(() => {})` too: there, the lexer is unsure from it on.
                    '/' => {
                        if self.line_start {
                            self.unsure(start);
                        }
                        Slash::StartsRegex
                    }
                    '[' | ';' | ',' | '.' | '<' | '>' | '=' | '!' | '+' | '-' | '*' | '%' | '&' | '|' | '^' | '~' | '?' | ':' => Slash::StartsRegex,
                    // Outside a literal or a comment, a `\` is only a name's `\u` escape, and anything else
                    // is no part of a script's code.
                    _ => {
                        self.unsure(start);
                        Slash::StartsRegex
                    }
                };
                self.after(slash);
                if c == '.' {
                    self.next = Next::Property;
                }
            }
        }
    }

    /// Records what the token just read leaves: what a `/` after it is, and nothing in particular for
    /// the next one.
    fn after(&mut self, slash: Slash) {
        self.slash = slash;
        self.next = Next::Any;
        self.line_start = false;
    }

    /// Records that the lexer stops being sure at byte `at`, unless it already has.
    fn unsure(&mut self, at: usize) {
        self.doubt = self.doubt.or(Some(at));
    }

    /// Takes a character of a name, or a `\u` escape spelling one (`\u0061`, `\u{61}`), and says
    /// whether it did. The parser refuses many names spelled with escapes (`\u0072eturn`, `\u0020`),
    /// and the lexer doesn't tell which: it is unsure past any escape.
    fn name_part(&mut self) -> bool {
        if self.at("\\u") {
            self.unsure(self.i);
            self.bump();
            self.bump();
            // `\u0061` needs nothing more, its digits being name characters too.
            if self.peek() == Some('{') {
                self.bump();
                while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                    self.bump();
                }
                if self.peek() == Some('}') {
                    self.bump();
                }
            }
            true
        } else if self.peek().is_some_and(is_name_part) {
            self.bump();
            true
        } else {
            false
        }
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
                    self.after(Slash::Divides);
                    return;
                }
                '$' if self.peek() == Some('{') && self.braces.len() < MAX_TEMPLATE_DEPTH => {
                    self.bump();
                    self.push(TokenKind::Template, start);
                    self.braces.push(0);
                    self.after(Slash::StartsRegex);
                    return;
                }
                // Deeper than tracked, the code in a `${…}` is read as the template's text: a guess.
                '$' if self.peek() == Some('{') => self.unsure(self.i.saturating_sub(1)),
                _ => {}
            }
        }
        self.push(TokenKind::Template, start);
    }

    /// A number, to where the parser ends it: `1.5e+3`, `.5`, `0x1F`, `10n`, `1_000`. A `.` after a
    /// fraction, an exponent, a radix's digits, a BigInt's `n` or an old script's octal number is a
    /// property's: `1.5.toFixed(2)`, `1e3.toFixed(1)`, `0x1F.toString()`, `10n.toString()`, `017.x`.
    fn number(&mut self) {
        let radix = self.peek() == Some('0') && matches!(self.peek2(), Some('x' | 'X' | 'o' | 'O' | 'b' | 'B'));
        // Whether what it has taken is a `0` and octal digits only, as `017` is.
        let mut octal = self.peek() == Some('0');
        let (mut taken, mut last, mut dot, mut letter) = (0_usize, '\0', false, false);
        while let Some(c) = self.peek() {
            let sign = matches!(c, '+' | '-') && matches!(last, 'e' | 'E') && !radix;
            let fraction = c == '.' && !dot && !letter && !(octal && taken > 1);
            if !(c.is_ascii_alphanumeric() || c == '_' || fraction || sign) {
                break;
            }
            octal &= taken == 0 || matches!(c, '0'..='7');
            dot |= c == '.';
            letter |= c.is_ascii_alphabetic();
            (taken, last) = (taken.saturating_add(1), c);
            self.bump();
        }
    }

    fn word(&mut self, start: usize) {
        while self.name_part() {}
        if self.next == Next::Property {
            // A property: `x.default`, `this.event` …
            self.after(Slash::Divides);
            return;
        }
        let word = self.s.get(start..self.i).unwrap_or_default();
        let label = self.next == Next::Label && !self.line_start;
        let for_await = self.next == Next::Condition && word == "await";
        let Word { colour, slash, next } = known(word).unwrap_or_else(|| Word {
            // A name spelled with escapes is coloured as the name it spells: `\u0061pp` is `app`.
            colour: unescaped(word).and_then(|name| global(&name)),
            slash: Slash::Divides,
            next: Next::Any,
        });
        if let Some(kind) = colour {
            self.push(kind, start);
        }
        self.after(if label { Slash::StartsRegex } else { slash });
        self.next = if for_await { Next::Condition } else { next };
    }
}

/// The name `word` spells, its `\u` escapes (`\u0061`, `\u{61}`) decoded, or `None` if one of them
/// spells no character.
fn unescaped(word: &str) -> Option<Cow<'_, str>> {
    if !word.contains('\\') {
        return Some(Cow::Borrowed(word));
    }
    let mut out = String::with_capacity(word.len());
    let mut rest = word;
    while let Some(at) = rest.find('\\') {
        out.push_str(rest.get(..at)?);
        let escape = rest.get(at..)?.strip_prefix("\\u")?;
        let (digits, after) = match escape.strip_prefix('{') {
            Some(braced) => {
                let end = braced.find('}')?;
                (braced.get(..end)?, braced.get(end..)?.strip_prefix('}')?)
            }
            None => (escape.get(..4)?, escape.get(4..)?),
        };
        // `from_str_radix` would take a sign too.
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        out.push(char::from_u32(u32::from_str_radix(digits, 16).ok()?)?);
        rest = after;
    }
    out.push_str(rest);
    Some(Cow::Owned(out))
}

fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Whether `c` is white space to the parser, other than a line end: Unicode's White_Space but for
/// U+0085, which ECMAScript doesn't count, and with the byte order mark, which it does.
fn is_space(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{85}' && !is_line_end(c))
}

/// Whether a name may start with `c`: ECMAScript's IdentifierStartChar, by Unicode's ID_Start, as the
/// parser reads it (`\u00b2` can't start a name, `\u2118` can).
fn is_name_start(c: char) -> bool {
    c == '$' || c == '_' || CodePointSetData::new::<IdStart>().contains(c)
}

/// Whether a name may hold `c` after its first character: ECMAScript's IdentifierPartChar, by
/// Unicode's ID_Continue (`_`, digits and combining marks among them).
fn is_name_part(c: char) -> bool {
    c == '$' || c == '\u{200C}' || c == '\u{200D}' || CodePointSetData::new::<IdContinue>().contains(c)
}

#[cfg(test)]
mod tests;
