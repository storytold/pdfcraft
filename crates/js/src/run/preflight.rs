//! Bounded lexical preflight before entering Boa's recursive parser/compiler.
use super::{MAX_ARROW_FUNCTIONS, MAX_BRACKET_DEPTH, MAX_CONDITIONALS, MAX_PREFIX_RUN, MAX_RIGHT_ASSOCIATIVE_OPERATORS, MAX_SCRIPT_BYTES};

/// Active chains live at bracket depths; literal contents never act as statement boundaries.
struct Guard<'a> {
    bytes: &'a [u8],
    i: usize,
    depth: usize,
    levels: [[usize; 4]; MAX_BRACKET_DEPTH + 1],
    template_returns: [bool; MAX_BRACKET_DEPTH + 1],
    template_text: bool,
    operand: bool,
    property: bool,
    prefix: usize,
}
impl<'a> Guard<'a> {
    /// Allocate fixed-size nesting state; no input controls an allocation size here.
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            i: 0,
            depth: 0,
            levels: [[0; 4]; MAX_BRACKET_DEPTH + 1],
            template_returns: [false; MAX_BRACKET_DEPTH + 1],
            template_text: false,
            operand: true,
            property: false,
            prefix: 0,
        }
    }
    /// Consume one exact ASCII byte when present.
    fn eat(&mut self, c: u8) -> bool {
        if self.bytes.get(self.i) == Some(&c) {
            self.i = self.i.saturating_add(1);
            true
        } else {
            false
        }
    }
    /// Reset token context after a value or an operator; comments and whitespace preserve it.
    fn context(&mut self, operand: bool) {
        self.prefix = 0;
        self.property = false;
        self.operand = operand;
    }
    /// Count active operators across enclosing depths without charging completed sibling statements.
    fn count(&mut self, kind: usize, limit: usize) -> bool {
        let Some(n) = self.levels.get_mut(self.depth).and_then(|d| d.get_mut(kind)) else { return false };
        *n = n.saturating_add(1);
        self.levels.iter().fold(0usize, |sum, d| sum.saturating_add(d.get(kind).copied().unwrap_or(0))) <= limit
    }
    /// Enter a bracket or template interpolation while preserving the enclosing expression's counters.
    fn open(&mut self, template: bool) -> bool {
        self.depth = self.depth.saturating_add(1);
        let Some(level) = self.levels.get_mut(self.depth) else { return false };
        *level = [0; 4];
        if let Some(back) = self.template_returns.get_mut(self.depth) {
            *back = template;
        }
        self.context(true);
        true
    }
    /// Complete a bracketed expression and resume template text at its interpolation boundary.
    fn close(&mut self, c: u8) {
        self.template_text = c == b'}' && self.template_returns.get(self.depth).copied().unwrap_or(false);
        if let Some(level) = self.levels.get_mut(self.depth) {
            *level = [0; 4];
        }
        if let Some(back) = self.template_returns.get_mut(self.depth) {
            *back = false;
        }
        self.depth = self.depth.saturating_sub(1);
        self.context(false);
    }
    /// A semicolon/comma ends only this depth's chain, never a surrounding function assignment.
    fn separator(&mut self) {
        if let Some(level) = self.levels.get_mut(self.depth) {
            *level = [0; 4];
        }
        self.context(true);
    }
    /// Skip an ordinary quoted token, including escaped quote bytes.
    fn quoted(&mut self, quote: u8) {
        while let Some(&c) = self.bytes.get(self.i) {
            self.i = self.i.saturating_add(1);
            if c == b'\\' {
                self.i = self.i.saturating_add(1);
            } else if c == quote || c == b'\n' {
                break;
            }
        }
        self.context(false);
    }
    /// Skip a line comment without changing the preceding token context.
    fn line_comment(&mut self) {
        while self.bytes.get(self.i).is_some_and(|c| *c != b'\n') {
            self.i = self.i.saturating_add(1);
        }
    }
    /// Skip a block comment without treating its delimiters as code.
    fn block_comment(&mut self) {
        self.i = self.i.saturating_add(1);
        while self.bytes.get(self.i..).is_some_and(|rest| !rest.is_empty() && !rest.starts_with(b"*/")) {
            self.i = self.i.saturating_add(1);
        }
        self.i = self.i.saturating_add(2);
    }
    /// In operand position, a regexp is one literal token; escaped slashes/classes stay inside it.
    fn regexp(&mut self) {
        let mut class = false;
        while let Some(&c) = self.bytes.get(self.i) {
            self.i = self.i.saturating_add(1);
            match c {
                b'\\' => self.i += 1,
                b'[' => class = true,
                b']' => class = false,
                b'/' if !class => break,
                b'\n' | b'\r' => break,
                _ => {}
            }
        }
        while self.bytes.get(self.i).is_some_and(u8::is_ascii_alphabetic) {
            self.i = self.i.saturating_add(1);
        }
        self.context(false);
    }
    /// Template text is literal, but each interpolation re-enters the bounded code scanner.
    fn template(&mut self, c: u8) -> bool {
        match c {
            b'\\' => self.i += 1,
            b'`' => {
                self.template_text = false;
                self.context(false);
            }
            b'$' if self.eat(b'{') => {
                self.template_text = false;
                return self.open(true);
            }
            _ => {}
        }
        true
    }
    /// Prefix recursion is independent from binary expression chains; postfix ++/-- ends a value.
    fn unary(&mut self, c: u8) -> bool {
        if matches!(c, b'+' | b'-') && !self.operand && self.eat(c) {
            self.context(false);
            return true;
        }
        self.prefix = self.prefix.saturating_add(1);
        self.operand = true;
        self.property = false;
        self.prefix <= MAX_PREFIX_RUN
    }
    /// Scan a keyword/identifier without treating property names such as .return as keywords.
    fn word(&mut self) -> bool {
        let start = self.i.saturating_sub(1);
        while self.bytes.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$')) {
            self.i = self.i.saturating_add(1);
        }
        let token = self.bytes.get(start..self.i);
        let unary = !self.property && matches!(token, Some(b"void" | b"typeof" | b"delete" | b"new"));
        self.operand =
            !self.property && (unary || matches!(token, Some(b"return" | b"throw" | b"case" | b"yield" | b"await" | b"in" | b"instanceof")));
        self.property = false;
        if unary {
            self.prefix = self.prefix.saturating_add(1);
        } else {
            self.prefix = 0;
        }
        self.prefix <= MAX_PREFIX_RUN
    }
    /// Distinguish assignment, arrow, power and ternary tokens from comparisons and optional chaining.
    fn operator(&mut self, c: u8) -> bool {
        self.context(true);
        let op = match c {
            b'=' if self.eat(b'>') => Some((2, MAX_ARROW_FUNCTIONS)),
            b'=' if self.eat(b'=') => {
                self.eat(b'=');
                None
            }
            b'=' => Some((0, MAX_RIGHT_ASSOCIATIVE_OPERATORS)),
            b'!' => {
                self.eat(b'=');
                self.eat(b'=');
                None
            }
            b'<' | b'>' => {
                if self.eat(c) {
                    if c == b'>' {
                        self.eat(c);
                    }
                    self.eat(b'=').then_some((0, MAX_RIGHT_ASSOCIATIVE_OPERATORS))
                } else {
                    self.eat(b'=');
                    None
                }
            }
            b'*' if self.eat(b'*') => Some((if self.eat(b'=') { 0 } else { 1 }, MAX_RIGHT_ASSOCIATIVE_OPERATORS)),
            b'?' if self.eat(b'?') => self.eat(b'=').then_some((0, MAX_RIGHT_ASSOCIATIVE_OPERATORS)),
            b'?' if self.eat(b'.') => {
                self.property = true;
                self.operand = false;
                None
            }
            b'?' => Some((3, MAX_CONDITIONALS)),
            _ => None,
        };
        op.is_none_or(|(kind, limit)| self.count(kind, limit))
    }
    /// Scan one code token without entering the JavaScript parser.
    fn token(&mut self, c: u8) -> bool {
        match c {
            b'`' => {
                self.template_text = true;
                self.prefix = 0;
                self.property = false;
            }
            b'\'' | b'"' => self.quoted(c),
            b'/' if self.bytes.get(self.i) == Some(&b'/') => self.line_comment(),
            b'/' if self.bytes.get(self.i) == Some(&b'*') => self.block_comment(),
            b'/' if self.operand => self.regexp(),
            b'(' | b'[' | b'{' => return self.open(false),
            b')' | b']' | b'}' => self.close(c),
            b';' | b',' => self.separator(),
            b'!' if self.bytes.get(self.i) == Some(&b'=') => return self.operator(c),
            b'!' | b'~' | b'+' | b'-' => return self.unary(c),
            b'=' | b'<' | b'>' | b'*' | b'?' => return self.operator(c),
            c if c.is_ascii_alphabetic() || matches!(c, b'_' | b'$') => return self.word(),
            c if c.is_ascii_whitespace() => {}
            b'.' => {
                self.context(false);
                self.property = true;
            }
            _ => self.context(matches!(c, b'/' | b'%' | b'&' | b'|' | b'^' | b':')),
        }
        true
    }
}

/// Enforce source and active nesting budgets before any parser/compiler work is attempted.
pub(super) fn refuse(source: &str) -> Option<String> {
    if source.len() > MAX_SCRIPT_BYTES {
        return Some(format!("the script is too long ({} KiB; the limit is {} KiB)", source.len() / 1024, MAX_SCRIPT_BYTES / 1024));
    }
    let mut guard = Guard::new(source.as_bytes());
    while let Some(&c) = guard.bytes.get(guard.i) {
        guard.i = guard.i.saturating_add(1);
        let safe = if guard.template_text { guard.template(c) } else { guard.token(c) };
        if !safe {
            return Some("the script is nested too deeply to run safely".into());
        }
    }
    None
}
