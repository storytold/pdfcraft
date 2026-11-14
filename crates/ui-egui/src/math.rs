//! LaTeX math delimiter parser, AST, and 2D layout engine for comment annotations and tooltips.
//!
//! Parses inline math (`$ ... $`) and block math (`$$ ... $$`), builds a tree (AST) of
//! mathematical structures (fractions, binomials, radicals, scripts, symbols), then measures
//! and paints them with egui's `Painter` for true 2D layout—numerators above denominators,
//! scaled parentheses around binomial coefficients, radical signs with overlines, and
//! properly positioned superscripts and subscripts.
//!
//! A text-only fallback (`format_text_with_math`) exists for copy/paste and accessibility.
//!
//! Adheres strictly to the never-crash rule: no panics, no unwrap/expect outside tests,
//! bounded recursion, safe slicing, and graceful fallback to raw text on malformed input.

use egui::{Color32, FontId, Pos2, Response, Ui, Vec2, vec2};

use crate::math_symbols::{self, AtomClass};

/// Maximum source size parsed as math; larger comment bodies remain plain text.
const MAX_MATH_INPUT_BYTES: usize = 64 * 1024;

// ─── Segment parser ───────────────────────────────────────────────────────────

/// A segment of text that is either plain text, inline math, or block math.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MathSegment<'a> {
    /// Plain text without math delimiters.
    PlainText(&'a str),
    /// Inline mathematical formula enclosed in `$ ... $`.
    InlineMath(&'a str),
    /// Block mathematical formula enclosed in `$$ ... $$`.
    BlockMath(&'a str),
}

/// Parses an input string into segments of plain text, inline math (`$ ... $`),
/// and block math (`$$ ... $$`).
///
/// Handles false positives:
/// - Isolated `$` (e.g. `"Costa $20"`) is treated as plain text.
/// - Unclosed delimiters (e.g. `"$x + y"`) are treated as plain text.
/// - Escaped dollars (`\$`) are preserved as literal text.
/// - Inputs over 64 KiB are kept as plain text.
pub fn parse_math_segments(input: &str) -> Vec<MathSegment<'_>> {
    if input.len() > MAX_MATH_INPUT_BYTES {
        return vec![MathSegment::PlainText(input)];
    }

    let mut segments = Vec::new();
    let mut current_start = 0;
    let mut i = 0;
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut inline_closers = Vec::new();
    let mut block_closers = Vec::new();
    let mut paragraph_breaks = Vec::new();
    let mut scan = 0;
    while scan < len {
        if bytes.get(scan) == Some(&b'\\') {
            scan = scan.saturating_add(2);
            continue;
        }
        if bytes.get(scan) == Some(&b'$') {
            if input.get(..scan).and_then(|s| s.chars().next_back()).is_some_and(|c| !c.is_whitespace()) {
                inline_closers.push(scan);
            }
            if bytes.get(scan.saturating_add(1)) == Some(&b'$') {
                block_closers.push(scan);
            }
        }
        if bytes.get(scan) == Some(&b'\n') && bytes.get(scan.saturating_add(1)) == Some(&b'\n') {
            paragraph_breaks.push(scan);
        }
        scan = scan.saturating_add(1);
    }

    while i < len {
        if bytes.get(i) == Some(&b'\\') {
            // Escaped character: skip backslash and next byte so `\$` is not parsed as a delimiter.
            i = i.saturating_add(2);
            continue;
        }

        if bytes.get(i) == Some(&b'$') {
            // Check for block math delimiter "$$"
            if bytes.get(i.saturating_add(1)) == Some(&b'$') {
                let open_start = i;
                let math_start = i.saturating_add(2);
                let close_idx = block_closers.get(block_closers.partition_point(|&close| close < math_start)).copied();

                if let Some(close) = close_idx {
                    if open_start > current_start
                        && let Some(plain) = input.get(current_start..open_start)
                    {
                        segments.push(MathSegment::PlainText(plain));
                    }
                    if let Some(math) = input.get(math_start..close) {
                        segments.push(MathSegment::BlockMath(math));
                    }
                    i = close.saturating_add(2);
                    current_start = i;
                    continue;
                } else {
                    // No matching closing "$$", treat as plain text.
                    i = i.saturating_add(2);
                    continue;
                }
            } else {
                // Potential inline math delimiter "$"
                let open_start = i;
                let math_start = i.saturating_add(1);

                // Opening '$' must not be immediately followed by whitespace or '$'
                let next_char = input.get(math_start..).and_then(|s| s.chars().next());
                let valid_open = next_char.is_some_and(|c| !c.is_whitespace() && c != '$');

                if valid_open {
                    let first_close = inline_closers.partition_point(|&close| close < math_start);
                    let paragraph_end =
                        paragraph_breaks.get(paragraph_breaks.partition_point(|&break_start| break_start < math_start)).copied().unwrap_or(len);
                    let close_idx = inline_closers.get(first_close).copied().filter(|&close| close < paragraph_end);

                    if let Some(close) = close_idx {
                        if open_start > current_start
                            && let Some(plain) = input.get(current_start..open_start)
                        {
                            segments.push(MathSegment::PlainText(plain));
                        }
                        if let Some(math) = input.get(math_start..close) {
                            segments.push(MathSegment::InlineMath(math));
                        }
                        i = close.saturating_add(1);
                        current_start = i;
                        continue;
                    }
                }

                // Not a valid opening '$' or no closing '$' found: advance by 1 byte.
                i = i.saturating_add(1);
                continue;
            }
        }

        i = i.saturating_add(1);
    }

    if current_start < len
        && let Some(plain) = input.get(current_start..)
    {
        segments.push(MathSegment::PlainText(plain));
    }

    segments
}

// ─── AST ──────────────────────────────────────────────────────────────────────

/// A node in the math expression tree.
#[derive(Clone, Debug, PartialEq)]
pub enum MathNode {
    /// A run of text characters (variables, digits, punctuation).
    Text(String),
    /// A known LaTeX symbol mapped to its Unicode representation.
    Symbol { text: String, command: String, class: AtomClass },
    /// A fraction: numerator over denominator separated by a horizontal bar.
    Frac { num: Box<MathNode>, den: Box<MathNode> },
    /// A binomial coefficient: `n` above `k` inside scaled parentheses.
    Binom { n: Box<MathNode>, k: Box<MathNode> },
    /// A square root (or nth root): radical sign with overline.
    Sqrt { degree: Option<Box<MathNode>>, arg: Box<MathNode> },
    /// An accent drawn above its argument.
    Accent { kind: MathAccent, arg: Box<MathNode> },
    /// Super/subscripts on a base.
    Scripts { base: Box<MathNode>, sup: Option<Box<MathNode>>, sub: Option<Box<MathNode>> },
    /// A horizontal sequence of nodes.
    Row(Vec<MathNode>),
    /// A space of a certain width factor (1.0 = normal space).
    Space(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathAccent {
    Vector,
    Hat,
    Bar,
    Overline,
    Underline,
    Tilde,
    Dot,
    DoubleDot,
}

// ─── Tokenizer ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Token<'a> {
    /// A LaTeX command like `\frac`, `\alpha`.
    Command(&'a str),
    /// A `{` character.
    OpenBrace,
    /// A `}` character.
    CloseBrace,
    /// `^` (superscript).
    Caret,
    /// `_` (subscript).
    Underscore,
    /// A literal character.
    Char(char),
    /// A space or whitespace run.
    Whitespace,
}

fn tokenize(input: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = match bytes.get(i) {
            Some(b) => *b,
            None => break,
        };

        if b == b'\\' {
            let next_i = i.saturating_add(1);
            if next_i >= len {
                tokens.push(Token::Char('\\'));
                break;
            }
            let next_b = match bytes.get(next_i) {
                Some(b) => *b,
                None => break,
            };
            if next_b == b'{' {
                tokens.push(Token::Char('{'));
                i = next_i.saturating_add(1);
                continue;
            }
            if next_b == b'}' {
                tokens.push(Token::Char('}'));
                i = next_i.saturating_add(1);
                continue;
            }
            if next_b == b'\\' {
                tokens.push(Token::Char('\n'));
                i = next_i.saturating_add(1);
                continue;
            }
            if next_b == b'$' {
                tokens.push(Token::Char('$'));
                i = next_i.saturating_add(1);
                continue;
            }
            // Spacing commands: \, \: \; \!
            if matches!(next_b, b',' | b':' | b';' | b'!') {
                let cmd = input.get(next_i..next_i.saturating_add(1)).unwrap_or("");
                tokens.push(Token::Command(cmd));
                i = next_i.saturating_add(1);
                continue;
            }
            // Read alphabetic command name
            let mut cmd_end = next_i;
            while cmd_end < len && bytes.get(cmd_end).is_some_and(|c| c.is_ascii_alphabetic()) {
                cmd_end = cmd_end.saturating_add(1);
            }
            if cmd_end > next_i {
                let cmd = input.get(next_i..cmd_end).unwrap_or("");
                tokens.push(Token::Command(cmd));
                i = cmd_end;
            } else {
                // Single non-alphabetic character after backslash
                let cmd = input.get(next_i..next_i.saturating_add(1)).unwrap_or("");
                tokens.push(Token::Command(cmd));
                i = next_i.saturating_add(1);
            }
            continue;
        }

        match b {
            b'{' => {
                tokens.push(Token::OpenBrace);
                i = i.saturating_add(1);
            }
            b'}' => {
                tokens.push(Token::CloseBrace);
                i = i.saturating_add(1);
            }
            b'^' => {
                tokens.push(Token::Caret);
                i = i.saturating_add(1);
            }
            b'_' => {
                tokens.push(Token::Underscore);
                i = i.saturating_add(1);
            }
            _ => {
                if let Some(c) = input.get(i..).and_then(|sub| sub.chars().next()) {
                    if c.is_whitespace() {
                        tokens.push(Token::Whitespace);
                    } else {
                        tokens.push(Token::Char(c));
                    }
                    i = i.saturating_add(c.len_utf8());
                } else {
                    i = i.saturating_add(1);
                }
            }
        }
    }

    tokens
}

// ─── Recursive descent parser ─────────────────────────────────────────────────

/// Maximum recursion depth to prevent stack overflow on pathological input.
const MAX_DEPTH: usize = 32;

struct Parser<'a> {
    tokens: &'a [Token<'a>],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token<'a>]) -> Self {
        Self { tokens, pos: 0, depth: 0 }
    }

    fn peek(&self) -> Option<&Token<'a>> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    /// Parse a complete expression (a row of atoms, possibly with scripts).
    fn parse_row(&mut self) -> Result<MathNode, &'static str> {
        if self.depth > MAX_DEPTH {
            return Err("recursion limit exceeded");
        }
        self.depth = self.depth.saturating_add(1);
        let mut nodes = Vec::new();

        while !self.at_end() {
            // Stop at CloseBrace (handled by caller)
            if matches!(self.peek(), Some(Token::CloseBrace)) {
                break;
            }
            let atom = self.parse_atom()?;
            // Check for scripts after the atom
            let node = self.maybe_scripts(atom)?;
            nodes.push(node);
        }

        self.depth = self.depth.saturating_sub(1);
        Ok(match nodes.len() {
            0 => MathNode::Text(String::new()),
            1 => nodes.remove(0),
            _ => MathNode::Row(nodes),
        })
    }

    /// Parse a single atom (the smallest unit before scripts are attached).
    fn parse_atom(&mut self) -> Result<MathNode, &'static str> {
        match self.peek() {
            Some(Token::Command(cmd)) => {
                let cmd = *cmd;
                self.advance();
                self.parse_command(cmd)
            }
            Some(Token::OpenBrace) => {
                self.advance(); // consume '{'
                let inner = self.parse_row()?;
                self.consume_group_end()?;
                Ok(inner)
            }
            Some(Token::Char(c)) => {
                let c = *c;
                self.advance();
                // Collect consecutive chars into one Text node for efficiency
                let mut text = String::new();
                text.push(c);
                while let Some(Token::Char(c)) = self.peek() {
                    // Don't merge if next token is a relation char (=, <, >)
                    if matches!(*c, '=' | '<' | '>') {
                        break;
                    }
                    text.push(*c);
                    self.advance();
                }
                Ok(MathNode::Text(text))
            }
            Some(Token::Whitespace) => {
                self.advance();
                Ok(MathNode::Space(1.0))
            }
            // Relation characters get their own node with spacing
            Some(Token::Caret) | Some(Token::Underscore) => {
                // Bare script without a base: use empty text as base
                Ok(MathNode::Text(String::new()))
            }
            Some(Token::CloseBrace) => {
                // Should be consumed by caller
                Ok(MathNode::Text(String::new()))
            }
            None => Ok(MathNode::Text(String::new())),
        }
    }

    /// If the current position has `^` or `_`, attach scripts to `base`.
    fn maybe_scripts(&mut self, base: MathNode) -> Result<MathNode, &'static str> {
        let mut sup = None;
        let mut sub = None;

        loop {
            match self.peek() {
                Some(Token::Caret) if sup.is_none() => {
                    self.advance();
                    sup = Some(Box::new(self.parse_script_arg()?));
                }
                Some(Token::Underscore) if sub.is_none() => {
                    self.advance();
                    sub = Some(Box::new(self.parse_script_arg()?));
                }
                _ => break,
            }
        }

        if sup.is_some() || sub.is_some() { Ok(MathNode::Scripts { base: Box::new(base), sup, sub }) } else { Ok(base) }
    }

    /// Parse the argument to a script: either a `{...}` group or a single token.
    fn parse_script_arg(&mut self) -> Result<MathNode, &'static str> {
        match self.peek() {
            Some(Token::OpenBrace) => {
                self.advance();
                let inner = self.parse_row()?;
                self.consume_group_end()?;
                Ok(inner)
            }
            Some(Token::Command(cmd)) => {
                let cmd = *cmd;
                self.advance();
                self.parse_command(cmd)
            }
            Some(Token::Char(c)) => {
                let c = *c;
                self.advance();
                Ok(MathNode::Text(c.to_string()))
            }
            _ => Err("missing script argument"),
        }
    }

    /// Parse a `{...}` group and return its contents as a node.
    fn parse_group(&mut self) -> Result<MathNode, &'static str> {
        // Skip whitespace before the group
        while matches!(self.peek(), Some(Token::Whitespace)) {
            self.advance();
        }
        match self.peek() {
            Some(Token::OpenBrace) => {
                self.advance();
                let inner = self.parse_row()?;
                self.consume_group_end()?;
                Ok(inner)
            }
            Some(Token::Command(cmd)) => {
                let cmd = *cmd;
                self.advance();
                self.parse_command(cmd)
            }
            Some(Token::Char(c)) => {
                let c = *c;
                self.advance();
                Ok(MathNode::Text(c.to_string()))
            }
            _ => Err("missing command argument"),
        }
    }

    fn consume_group_end(&mut self) -> Result<(), &'static str> {
        if matches!(self.peek(), Some(Token::CloseBrace)) {
            self.advance();
            Ok(())
        } else {
            Err("unclosed group")
        }
    }

    fn parse_command(&mut self, cmd: &str) -> Result<MathNode, &'static str> {
        if self.depth > MAX_DEPTH {
            return Err("recursion limit exceeded");
        }
        self.depth = self.depth.saturating_add(1);
        let result = self.parse_command_inner(cmd);
        self.depth = self.depth.saturating_sub(1);
        result
    }

    fn parse_command_inner(&mut self, cmd: &str) -> Result<MathNode, &'static str> {
        match cmd {
            "frac" | "dfrac" | "tfrac" => {
                let num = self.parse_group()?;
                let den = self.parse_group()?;
                Ok(MathNode::Frac { num: Box::new(num), den: Box::new(den) })
            }
            "binom" | "dbinom" | "tbinom" | "choose" => {
                let n = self.parse_group()?;
                let k = self.parse_group()?;
                Ok(MathNode::Binom { n: Box::new(n), k: Box::new(k) })
            }
            "sqrt" => {
                // Check for optional degree argument [n]
                let degree = self.parse_optional_bracket()?;
                let arg = self.parse_group()?;
                Ok(MathNode::Sqrt { degree: degree.map(Box::new), arg: Box::new(arg) })
            }
            "mathbb" => {
                let arg = self.parse_group()?;
                let text = node_to_text(&arg);
                let mut nodes = Vec::new();
                for c in text.chars() {
                    let mapped = match c {
                        'R' => Some('ℝ'),
                        'N' => Some('ℕ'),
                        'Z' => Some('ℤ'),
                        'Q' => Some('ℚ'),
                        'C' => Some('ℂ'),
                        'P' => Some('ℙ'),
                        'E' => Some('𝔼'),
                        'H' => Some('ℍ'),
                        _ => None,
                    };
                    if let Some(mapped) = mapped {
                        nodes.push(MathNode::Symbol { text: mapped.to_string(), command: c.to_string(), class: AtomClass::Ord });
                    } else {
                        nodes.push(MathNode::Text(c.to_string()));
                    }
                }
                Ok(match nodes.len() {
                    0 => MathNode::Text(String::new()),
                    1 => nodes.remove(0),
                    _ => MathNode::Row(nodes),
                })
            }
            "mathcal" | "mathbf" | "mathit" => {
                let arg = self.parse_group()?;
                let source = node_to_text(&arg);
                let style = match cmd {
                    "mathcal" => MathStyle::Calligraphic,
                    "mathbf" => MathStyle::Bold,
                    _ => MathStyle::Italic,
                };
                Ok(styled_text(&source, style))
            }
            "text" | "mathrm" | "mathtt" | "operatorname" => {
                let arg = self.parse_group()?;
                Ok(arg)
            }
            "left" | "right" => match self.peek() {
                Some(Token::Char(delimiter)) => {
                    let delimiter = *delimiter;
                    self.advance();
                    Ok(MathNode::Text(if delimiter == '.' { String::new() } else { delimiter.to_string() }))
                }
                _ => Ok(MathNode::Text(String::new())),
            },
            "vec" | "hat" | "bar" | "overline" | "underline" | "tilde" | "dot" | "ddot" => {
                let arg = self.parse_group()?;
                let kind = match cmd {
                    "vec" => MathAccent::Vector,
                    "hat" => MathAccent::Hat,
                    "bar" => MathAccent::Bar,
                    "overline" => MathAccent::Overline,
                    "underline" => MathAccent::Underline,
                    "tilde" => MathAccent::Tilde,
                    "dot" => MathAccent::Dot,
                    "ddot" => MathAccent::DoubleDot,
                    _ => return Err("unsupported accent"),
                };
                Ok(MathNode::Accent { kind, arg: Box::new(arg) })
            }
            // Spacing commands
            "quad" => Ok(MathNode::Space(4.0)),
            "qquad" => Ok(MathNode::Space(8.0)),
            "," | ":" | ";" => Ok(MathNode::Space(0.5)),
            "!" => Ok(MathNode::Space(-0.17)),
            " " => Ok(MathNode::Space(1.0)),
            _ => match lookup_command(cmd) {
                Some(symbol) => Ok(MathNode::Symbol { text: symbol.glyph.to_string(), command: cmd.to_string(), class: symbol.class }),
                None => Ok(MathNode::Text(format!("{cmd} "))),
            },
        }
    }

    /// Parse an optional `[...]` argument (e.g. for `\sqrt[3]{x}`).
    fn parse_optional_bracket(&mut self) -> Result<Option<MathNode>, &'static str> {
        // Skip whitespace
        while matches!(self.peek(), Some(Token::Whitespace)) {
            self.advance();
        }
        // Check for '[' as a Char token
        if let Some(Token::Char('[')) = self.peek() {
            self.advance(); // consume '['
            let mut text = String::new();
            let mut closed = false;
            while !self.at_end() {
                match self.peek() {
                    Some(Token::Char(']')) => {
                        self.advance();
                        closed = true;
                        break;
                    }
                    Some(Token::Char(c)) => {
                        text.push(*c);
                        self.advance();
                    }
                    Some(Token::Whitespace) => {
                        text.push(' ');
                        self.advance();
                    }
                    _ => break,
                }
            }
            if closed { Ok(Some(MathNode::Text(text))) } else { Err("unclosed optional argument") }
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone, Copy)]
enum MathStyle {
    Calligraphic,
    Bold,
    Italic,
}

fn styled_text(source: &str, style: MathStyle) -> MathNode {
    let nodes: Vec<MathNode> = source
        .chars()
        .map(|character| {
            let Some(glyph) = styled_letter(character, style) else {
                return MathNode::Text(character.to_string());
            };
            MathNode::Symbol { text: glyph.to_string(), command: character.to_string(), class: AtomClass::Ord }
        })
        .collect();
    match nodes.len() {
        0 => MathNode::Text(String::new()),
        1 => nodes.into_iter().next().unwrap_or_else(|| MathNode::Text(String::new())),
        _ => MathNode::Row(nodes),
    }
}

fn styled_letter(character: char, style: MathStyle) -> Option<char> {
    if matches!(style, MathStyle::Calligraphic) {
        let exceptional = match character {
            'B' => Some('ℬ'),
            'E' => Some('ℰ'),
            'F' => Some('ℱ'),
            'H' => Some('ℋ'),
            'I' => Some('ℐ'),
            'L' => Some('ℒ'),
            'M' => Some('ℳ'),
            'R' => Some('ℛ'),
            'e' => Some('ℯ'),
            'g' => Some('ℊ'),
            _ => None,
        };
        if exceptional.is_some() {
            return exceptional;
        }
    }
    let (start_upper, start_lower) = match style {
        MathStyle::Bold => (0x1D400, 0x1D41A),
        MathStyle::Italic => (0x1D434, 0x1D44E),
        MathStyle::Calligraphic => (0x1D49C, 0x1D4B6),
    };
    match character {
        'A'..='Z' => char::from_u32(start_upper + u32::from(character) - u32::from('A')),
        'a'..='z' => {
            let codepoint = start_lower + u32::from(character) - u32::from('a');
            if matches!(style, MathStyle::Italic) && character == 'h' { Some('\u{210E}') } else { char::from_u32(codepoint) }
        }
        _ => None,
    }
}

/// Parse a LaTeX math expression string into an AST.
///
/// Returns an error when the source exceeds 64 KiB.
pub fn parse_math_ast(input: &str) -> Result<MathNode, &'static str> {
    if input.len() > MAX_MATH_INPUT_BYTES {
        return Err("math input exceeds size limit");
    }

    let tokens = tokenize(input);
    let mut parser = Parser::new(&tokens);
    let node = parser.parse_row()?;
    if parser.at_end() { Ok(node) } else { Err("unexpected closing brace") }
}

// ─── Symbol table ─────────────────────────────────────────────────────────────

fn lookup_command(cmd: &str) -> Option<math_symbols::MathSymbol> {
    math_symbols::lookup(cmd)
}

// ─── AST to text (for flat fallback / copy-paste) ─────────────────────────────

/// Convert a `MathNode` to a flat text representation (used for copy/paste and accessibility).
fn node_to_text(node: &MathNode) -> String {
    match node {
        MathNode::Text(t) => t.clone(),
        MathNode::Symbol { text, .. } => text.clone(),
        MathNode::Space(w) => {
            if *w <= 0.0 {
                String::new()
            } else if *w >= 2.0 {
                "  ".to_string()
            } else {
                " ".to_string()
            }
        }
        MathNode::Frac { num, den } => {
            let n = node_to_text(num);
            let d = node_to_text(den);
            let wrap = |s: &str| {
                if s.len() > 1 && (s.contains(' ') || s.contains('+') || s.contains('-') || s.contains('/')) {
                    format!("({s})")
                } else {
                    s.to_string()
                }
            };
            format!("{}/{}", wrap(&n), wrap(&d))
        }
        MathNode::Binom { n, k } => {
            format!("C({},{})", node_to_text(n), node_to_text(k))
        }
        MathNode::Sqrt { degree, arg } => {
            let a = node_to_text(arg);
            match degree {
                Some(d) => format!("{}√({})", node_to_text(d), a),
                None => {
                    if a.len() <= 1 {
                        format!("√{a}")
                    } else {
                        format!("√({a})")
                    }
                }
            }
        }
        MathNode::Accent { kind, arg } => {
            let mut text = node_to_text(arg);
            text.push(match kind {
                MathAccent::Vector => '⃗',
                MathAccent::Hat => '̂',
                MathAccent::Bar => '̄',
                MathAccent::Overline => '̅',
                MathAccent::Underline => '̲',
                MathAccent::Tilde => '̃',
                MathAccent::Dot => '̇',
                MathAccent::DoubleDot => '̈',
            });
            text
        }
        MathNode::Scripts { base, sup, sub } => {
            let mut result = node_to_text(base);
            if let Some(sup) = sup {
                let s = node_to_text(sup);
                // Try Unicode superscript
                let mut all_ok = true;
                let mut converted = String::new();
                for c in s.chars() {
                    if let Some(sc) = char_to_superscript(c) {
                        converted.push(sc);
                    } else {
                        all_ok = false;
                        break;
                    }
                }
                if all_ok && !converted.is_empty() {
                    result.push_str(&converted);
                } else {
                    result.push_str(&format!("^({s})"));
                }
            }
            if let Some(sub) = sub {
                let s = node_to_text(sub);
                let mut all_ok = true;
                let mut converted = String::new();
                for c in s.chars() {
                    if let Some(sc) = char_to_subscript(c) {
                        converted.push(sc);
                    } else {
                        all_ok = false;
                        break;
                    }
                }
                if all_ok && !converted.is_empty() {
                    result.push_str(&converted);
                } else {
                    result.push_str(&format!("_({s})"));
                }
            }
            result
        }
        MathNode::Row(nodes) => {
            let mut result = String::new();
            for (i, node) in nodes.iter().enumerate() {
                let text = node_to_text(node);
                if matches!(node, MathNode::Symbol { class: AtomClass::Rel | AtomClass::Bin, .. }) {
                    if i > 0 && !matches!(nodes.get(i - 1), Some(MathNode::Space(_))) && !result.is_empty() && !result.ends_with(' ') {
                        result.push(' ');
                    }
                    result.push_str(&text);
                    if i + 1 < nodes.len() && !matches!(nodes.get(i + 1), Some(MathNode::Space(_))) {
                        result.push(' ');
                    }
                } else if matches!(node, MathNode::Space(_))
                    && i > 0
                    && matches!(nodes.get(i - 1), Some(MathNode::Symbol { text, .. }) if matches!(text.as_str(), "∀" | "∃" | "¬"))
                {
                    continue;
                } else {
                    result.push_str(&text);
                }
            }
            result
        }
    }
}

// ─── Unicode script helpers ───────────────────────────────────────────────────

/// Converts a character to its Unicode superscript representation if available.
pub fn char_to_superscript(c: char) -> Option<char> {
    match c {
        '0' => Some('⁰'),
        '1' => Some('¹'),
        '2' => Some('²'),
        '3' => Some('³'),
        '4' => Some('⁴'),
        '5' => Some('⁵'),
        '6' => Some('⁶'),
        '7' => Some('⁷'),
        '8' => Some('⁸'),
        '9' => Some('⁹'),
        '+' => Some('⁺'),
        '-' => Some('⁻'),
        '=' => Some('⁼'),
        '(' => Some('⁽'),
        ')' => Some('⁾'),
        'n' => Some('ⁿ'),
        'i' => Some('ⁱ'),
        'a' => Some('ᵃ'),
        'b' => Some('ᵇ'),
        'c' => Some('ᶜ'),
        'd' => Some('ᵈ'),
        'e' => Some('ᵉ'),
        'f' => Some('ᶠ'),
        'g' => Some('ᵍ'),
        'h' => Some('ʰ'),
        'j' => Some('ʲ'),
        'k' => Some('ᵏ'),
        'l' => Some('ˡ'),
        'm' => Some('ᵐ'),
        'o' => Some('ᵒ'),
        'p' => Some('ᵖ'),
        'r' => Some('ʳ'),
        's' => Some('ˢ'),
        't' => Some('ᵗ'),
        'u' => Some('ᵘ'),
        'v' => Some('ᵛ'),
        'w' => Some('ʷ'),
        'x' => Some('ˣ'),
        'y' => Some('ʸ'),
        'z' => Some('ᶻ'),
        'T' => Some('ᵀ'),
        _ => None,
    }
}

/// Converts a character to its Unicode subscript representation if available.
pub fn char_to_subscript(c: char) -> Option<char> {
    match c {
        '0' => Some('₀'),
        '1' => Some('₁'),
        '2' => Some('₂'),
        '3' => Some('₃'),
        '4' => Some('₄'),
        '5' => Some('₅'),
        '6' => Some('₆'),
        '7' => Some('₇'),
        '8' => Some('₈'),
        '9' => Some('₉'),
        '+' => Some('₊'),
        '-' => Some('₋'),
        '=' => Some('₌'),
        '(' => Some('₍'),
        ')' => Some('₎'),
        'a' => Some('ₐ'),
        'e' => Some('ₑ'),
        'h' => Some('ₕ'),
        'i' => Some('ᵢ'),
        'j' => Some('ⱼ'),
        'k' => Some('ₖ'),
        'l' => Some('ₗ'),
        'm' => Some('ₘ'),
        'n' => Some('ₙ'),
        'o' => Some('ₒ'),
        'p' => Some('ₚ'),
        'r' => Some('ᵣ'),
        's' => Some('ₛ'),
        't' => Some('ₜ'),
        'u' => Some('ᵤ'),
        'v' => Some('ᵥ'),
        'x' => Some('ₓ'),
        _ => None,
    }
}

// ─── Format helpers (text-only path, for copy/paste) ──────────────────────────

/// Formats a LaTeX math expression into a formatted mathematical string.
///
/// Returns `Err` if a syntax error (such as an unclosed brace) occurs, allowing
/// caller to fall back gracefully to raw text.
pub fn format_latex_math(latex: &str) -> Result<String, &'static str> {
    let ast = parse_math_ast(latex)?;
    let text = node_to_text(&ast);
    Ok(text.trim().to_string())
}

/// Formats text by converting any LaTeX formulas ($ ... $ and $$ ... $$) into
/// formatted mathematical notation.
pub fn format_text_with_math(text: &str) -> String {
    let segments = parse_math_segments(text);
    if segments.is_empty() {
        return String::new();
    }

    let mut out = String::with_capacity(text.len());
    for seg in segments {
        match seg {
            MathSegment::PlainText(s) => out.push_str(s),
            MathSegment::InlineMath(formula) => match format_latex_math(formula) {
                Ok(f) => out.push_str(&f),
                Err(_) => {
                    out.push('$');
                    out.push_str(formula);
                    out.push('$');
                }
            },
            MathSegment::BlockMath(formula) => match format_latex_math(formula) {
                Ok(f) => {
                    out.push('\n');
                    out.push_str(&f);
                    out.push('\n');
                }
                Err(_) => {
                    out.push_str("$$");
                    out.push_str(formula);
                    out.push_str("$$");
                }
            },
        }
    }
    out
}

// ─── 2D Layout & Rendering ────────────────────────────────────────────────────

/// Configuration for a math layout pass.
/// Measured size of a laid-out math node, with baseline information.
#[derive(Clone, Copy, Debug)]
struct MathBox {
    width: f32,
    ascent: f32,
    descent: f32,
    /// Distance that this node's baseline sits above the surrounding text baseline.
    baseline_shift: f32,
}

impl MathBox {
    fn height(&self) -> f32 {
        self.ascent + self.descent
    }

    fn above_baseline(&self) -> f32 {
        self.ascent + self.baseline_shift
    }

    fn below_baseline(&self) -> f32 {
        (self.descent - self.baseline_shift).max(0.0)
    }

    fn size(&self) -> Vec2 {
        vec2(self.width, self.above_baseline() + self.below_baseline())
    }
}

/// Minimum gap between numerator/denominator and the fraction bar.
const FRAC_GAP: f32 = 2.0;
/// Fraction bar thickness.
const FRAC_BAR: f32 = 1.0;
/// Extra padding around fraction content.
const FRAC_PAD: f32 = 1.5;
/// Padding inside binomial parentheses at a 15-point font size.
const BINOM_SIDE: f32 = 14.0;
/// Radical sign width factor.
const RADICAL_WIDTH: f32 = 0.7;
/// Gap above radical overline.
const RADICAL_GAP: f32 = 2.0;
/// Script size factor relative to base.
const SCRIPT_FACTOR: f32 = 0.78;
/// Natural downward shift of a subscript as a fraction of the base ascent.
const SUBSCRIPT_BASELINE_FRACTION: f32 = 0.25;
/// Upward shift of a superscript as a fraction of the base ascent.
const SUPERSCRIPT_BASELINE_FRACTION: f32 = 0.45;
/// Minimum gap between superscript and subscript bounds.
const SCRIPT_CLEARANCE: f32 = 1.0;
/// Space around relation symbols.
const RELATION_SPACE: f32 = 3.0;
/// Space around binary operators.
const BINARY_SPACE: f32 = 1.75;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VectorSymbol {
    Models,
    Vdash,
    NotVdash,
    NotModels,
    DoubleVdash,
    Dashv,
    ForAll,
    Exists,
    NotExists,
    Negation,
    And,
    Or,
    Implies,
    Iff,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SymbolRenderMode {
    Font,
    Vector(VectorSymbol),
    TextFallback,
}

/// Measure a `MathNode` for layout purposes.
fn measure_node(ui: &Ui, node: &MathNode, font: &FontId) -> MathBox {
    match node {
        MathNode::Text(t) => measure_text(ui, t, font),
        MathNode::Symbol { text, command, .. } => measure_symbol(ui, text, command, font),
        MathNode::Space(factor) => {
            let mut metrics = measure_text(ui, " ", font);
            metrics.width *= factor;
            metrics
        }
        MathNode::Frac { num, den } => {
            let num_box = measure_node(ui, num, font);
            let den_box = measure_node(ui, den, font);
            let width = num_box.width.max(den_box.width) + FRAC_PAD * 2.0;
            let ascent = num_box.height() + FRAC_GAP + FRAC_BAR * 0.5;
            let descent = den_box.height() + FRAC_GAP + FRAC_BAR * 0.5;
            MathBox { width, ascent, descent, baseline_shift: row_height(ui, font) * 0.25 }
        }
        MathNode::Binom { n, k } => {
            let n_box = measure_node(ui, n, font);
            let k_box = measure_node(ui, k, font);
            let inner_width = n_box.width.max(k_box.width) + FRAC_PAD * 2.0;
            let width = inner_width + binom_side(font) * 2.0;
            let ascent = n_box.height() + FRAC_GAP;
            let descent = k_box.height() + FRAC_GAP;
            MathBox { width, ascent, descent, baseline_shift: row_height(ui, font) * 0.25 }
        }
        MathNode::Sqrt { degree, arg } => {
            let arg_box = measure_node(ui, arg, font);
            let deg_width = degree
                .as_ref()
                .map(|d| {
                    let script_font = scaled_font(font, SCRIPT_FACTOR);
                    measure_node(ui, d, &script_font).width
                })
                .unwrap_or(0.0);
            let radical_w = (arg_box.height() * RADICAL_WIDTH).max(8.0);
            let width = deg_width + radical_w + arg_box.width + 2.0;
            let ascent = arg_box.ascent + RADICAL_GAP + FRAC_BAR;
            MathBox { width, ascent, descent: arg_box.descent, baseline_shift: arg_box.baseline_shift }
        }
        MathNode::Accent { kind, arg } => {
            let arg_box = measure_node(ui, arg, font);
            let accent_height = (row_height(ui, font) * 0.3).max(3.0);
            MathBox {
                width: arg_box.width.max(4.0),
                ascent: arg_box.ascent + if matches!(kind, MathAccent::Underline) { 0.0 } else { accent_height },
                descent: arg_box.descent + if matches!(kind, MathAccent::Underline) { accent_height } else { 0.0 },
                baseline_shift: arg_box.baseline_shift,
            }
        }
        MathNode::Scripts { base, sup, sub } => {
            let base_box = measure_node(ui, base, font);
            let script_font = scaled_font(font, SCRIPT_FACTOR);
            let sup_box = sup.as_ref().map(|s| measure_node(ui, s, &script_font));
            let sub_box = sub.as_ref().map(|s| measure_node(ui, s, &script_font));
            let (sup_offset, sub_offset) = script_baseline_offsets(base_box, sup_box, sub_box);
            let script_w = sup_box.map_or(0.0, |b| b.width).max(sub_box.map_or(0.0, |b| b.width));
            let width = base_box.width + script_w;
            let sup_ascent = sup_box.zip(sup_offset).map(|(sb, offset)| offset + sb.above_baseline()).unwrap_or(0.0);
            let sub_ascent = sub_box.zip(sub_offset).map(|(sb, offset)| (sb.above_baseline() - offset).max(0.0)).unwrap_or(0.0);
            let ascent = base_box.ascent.max(sup_ascent).max(sub_ascent);
            let sub_descent = sub_box.zip(sub_offset).map(|(sb, offset)| offset + sb.below_baseline()).unwrap_or(0.0);
            let descent = base_box.descent.max(sub_descent);
            MathBox { width, ascent, descent, baseline_shift: base_box.baseline_shift }
        }
        MathNode::Row(nodes) => {
            let mut width = 0.0f32;
            let mut ascent = 0.0f32;
            let mut descent = 0.0f32;
            for (i, child) in nodes.iter().enumerate() {
                let child_box = measure_node(ui, child, font);
                let before_space = atom_space(child);
                let after_space = before_space;
                if i > 0 && before_space > 0.0 && !matches!(nodes.get(i - 1), Some(MathNode::Space(_))) {
                    width += before_space;
                }
                width += child_box.width;
                ascent = ascent.max(child_box.above_baseline());
                descent = descent.max(child_box.below_baseline());
                if i + 1 < nodes.len() && after_space > 0.0 && !matches!(nodes.get(i + 1), Some(MathNode::Space(_))) {
                    width += after_space;
                }
            }
            MathBox { width, ascent, descent, baseline_shift: 0.0 }
        }
    }
}

fn measure_text(ui: &Ui, text: &str, font: &FontId) -> MathBox {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER);
    let mut ascent = 0.0f32;
    let mut descent = 0.0f32;
    for glyph in galley.rows.iter().flat_map(|row| &row.glyphs) {
        ascent = ascent.max(glyph.font_ascent);
        descent = descent.max(glyph.font_height - glyph.font_ascent);
    }
    if ascent == 0.0 && descent == 0.0 {
        let height = row_height(ui, font);
        ascent = height * 0.8;
        descent = height - ascent;
    }
    MathBox { width: galley.size().x, ascent, descent, baseline_shift: 0.0 }
}

fn atom_space(node: &MathNode) -> f32 {
    let MathNode::Symbol { class, .. } = node else {
        return 0.0;
    };
    match class {
        AtomClass::Rel => RELATION_SPACE,
        AtomClass::Bin | AtomClass::Op => BINARY_SPACE,
        AtomClass::Ord | AtomClass::Open | AtomClass::Close => 0.0,
    }
}

fn script_baseline_offsets(base: MathBox, sup: Option<MathBox>, sub: Option<MathBox>) -> (Option<f32>, Option<f32>) {
    let sup_offset = sup.map(|_| base.ascent * SUPERSCRIPT_BASELINE_FRACTION);
    let sub_offset = sub.map(|sub| {
        let natural_offset = base.ascent * SUBSCRIPT_BASELINE_FRACTION;
        let clearance_offset = match (sup, sup_offset) {
            (Some(sup), Some(sup_offset)) => -sup_offset + sup.below_baseline() + SCRIPT_CLEARANCE + sub.above_baseline(),
            _ => natural_offset,
        };
        natural_offset.max(clearance_offset)
    });
    (sup_offset, sub_offset)
}

fn binom_side(font: &FontId) -> f32 {
    BINOM_SIDE * font.size / 15.0
}

/// Draw a `MathNode` at position `pos` (top-left of its bounding box), with `baseline_y`.
fn draw_node(ui: &Ui, node: &MathNode, pos: Pos2, baseline_y: f32, font: &FontId, color: Color32) {
    let painter = ui.painter();

    match node {
        MathNode::Text(t) => {
            if !t.is_empty() {
                draw_text_on_baseline(ui, t, pos.x, baseline_y, font, color);
            }
        }
        MathNode::Symbol { text, command, .. } => draw_symbol(ui, text, command, pos, baseline_y, font, color),
        MathNode::Space(_) => { /* Just horizontal space, nothing to draw */ }
        MathNode::Frac { num, den } => {
            let num_box = measure_node(ui, num, font);
            let den_box = measure_node(ui, den, font);
            let total_w = num_box.width.max(den_box.width) + FRAC_PAD * 2.0;

            // Fraction bar at baseline
            let bar_y = baseline_y;
            let bar_stroke = egui::Stroke::new(FRAC_BAR, color);
            painter.line_segment([Pos2::new(pos.x, bar_y), Pos2::new(pos.x + total_w, bar_y)], bar_stroke);

            // Numerator: centered above the bar
            let num_x = pos.x + (total_w - num_box.width) * 0.5;
            let num_baseline = bar_y - FRAC_GAP - num_box.descent;
            let num_top_y = num_baseline - num_box.ascent;
            draw_node(ui, num, Pos2::new(num_x, num_top_y), num_baseline, font, color);

            // Denominator: centered below the bar
            let den_x = pos.x + (total_w - den_box.width) * 0.5;
            let den_baseline = bar_y + FRAC_GAP + FRAC_BAR + den_box.ascent;
            let den_top_y = den_baseline - den_box.ascent;
            draw_node(ui, den, Pos2::new(den_x, den_top_y), den_baseline, font, color);
        }
        MathNode::Binom { n, k } => {
            let n_box = measure_node(ui, n, font);
            let k_box = measure_node(ui, k, font);
            let side = binom_side(font);
            let inner_w = n_box.width.max(k_box.width) + FRAC_PAD * 2.0;
            // Draw scaled parentheses
            let paren_stroke = egui::Stroke::new(1.5, color);
            let left_x = pos.x + side;
            let right_x = pos.x + side + inner_w;
            let top_y = baseline_y - n_box.height() - FRAC_GAP;
            let bottom_y = baseline_y + k_box.height() + FRAC_GAP;

            // Left parenthesis as a Bezier-like set of line segments
            draw_paren(painter, left_x, top_y, bottom_y, side, true, paren_stroke);
            // Right parenthesis
            draw_paren(painter, right_x, top_y, bottom_y, side, false, paren_stroke);

            // N (top, centered)
            let n_x = pos.x + side + (inner_w - n_box.width) * 0.5;
            let n_baseline = baseline_y - FRAC_GAP - n_box.descent;
            draw_node(ui, n, Pos2::new(n_x, n_baseline - n_box.ascent), n_baseline, font, color);

            // K (bottom, centered)
            let k_x = pos.x + side + (inner_w - k_box.width) * 0.5;
            let k_baseline = baseline_y + FRAC_GAP + k_box.ascent;
            draw_node(ui, k, Pos2::new(k_x, k_baseline - k_box.ascent), k_baseline, font, color);
        }
        MathNode::Sqrt { degree, arg } => {
            let arg_box = measure_node(ui, arg, font);
            let script_font = scaled_font(font, SCRIPT_FACTOR);
            let deg_w = degree.as_ref().map(|d| measure_node(ui, d, &script_font).width).unwrap_or(0.0);
            let radical_w = (arg_box.height() * RADICAL_WIDTH).max(8.0);
            let stroke = egui::Stroke::new(FRAC_BAR, color);

            let content_left = pos.x + deg_w + radical_w;
            let arg_baseline = baseline_y;
            let overline_y = arg_baseline - arg_box.ascent - RADICAL_GAP;
            let bottom_y = arg_baseline + arg_box.descent;

            // Radical sign: checkmark shape
            let tick_start = Pos2::new(pos.x + deg_w, arg_baseline);
            let tick_bottom = Pos2::new(pos.x + deg_w + radical_w * 0.4, bottom_y);
            let tick_top = Pos2::new(content_left, overline_y);
            painter.line_segment([tick_start, tick_bottom], stroke);
            painter.line_segment([tick_bottom, tick_top], stroke);

            // Overline above the content
            let overline_end = Pos2::new(content_left + arg_box.width + 2.0, overline_y);
            painter.line_segment([tick_top, overline_end], stroke);

            // Draw the degree if present
            if let Some(d) = degree {
                let d_box = measure_node(ui, d, &script_font);
                let d_y = tick_start.y - d_box.height();
                draw_node(ui, d, Pos2::new(pos.x, d_y), d_y + d_box.ascent, &script_font, color);
            }

            // Draw argument
            draw_node(ui, arg, Pos2::new(content_left, arg_baseline - arg_box.ascent), arg_baseline, font, color);
        }
        MathNode::Accent { kind, arg } => {
            let arg_box = measure_node(ui, arg, font);
            let accent_width = arg_box.width.max(4.0);
            let accent_y = if matches!(kind, MathAccent::Underline) { baseline_y + arg_box.descent + 2.0 } else { baseline_y - arg_box.ascent - 2.0 };
            let stroke = egui::Stroke::new(FRAC_BAR, color);
            draw_node(ui, arg, pos, baseline_y, font, color);
            match kind {
                MathAccent::Vector => {
                    let tip = Pos2::new(pos.x + accent_width - 2.0, accent_y);
                    painter.line_segment([Pos2::new(pos.x, accent_y), tip], stroke);
                    painter.line_segment([tip - vec2(3.0, 2.5), tip], stroke);
                    painter.line_segment([tip - vec2(3.0, -2.5), tip], stroke);
                }
                MathAccent::Hat => {
                    let peak = Pos2::new(pos.x + accent_width * 0.5, accent_y - 2.0);
                    painter.line_segment([Pos2::new(pos.x, accent_y + 2.0), peak], stroke);
                    painter.line_segment([peak, Pos2::new(pos.x + accent_width, accent_y + 2.0)], stroke);
                }
                MathAccent::Bar | MathAccent::Overline | MathAccent::Underline => {
                    painter.line_segment([Pos2::new(pos.x, accent_y), Pos2::new(pos.x + accent_width, accent_y)], stroke);
                }
                MathAccent::Tilde => {
                    let mut previous = Pos2::new(pos.x, accent_y);
                    for i in 1..=6 {
                        let t = i as f32 / 6.0;
                        let next = Pos2::new(pos.x + accent_width * t, accent_y + (t * std::f32::consts::TAU).sin() * 1.5);
                        painter.line_segment([previous, next], stroke);
                        previous = next;
                    }
                }
                MathAccent::Dot => {
                    painter.circle_filled(Pos2::new(pos.x + accent_width * 0.5, accent_y), 1.2, color);
                }
                MathAccent::DoubleDot => {
                    painter.circle_filled(Pos2::new(pos.x + accent_width * 0.35, accent_y), 1.2, color);
                    painter.circle_filled(Pos2::new(pos.x + accent_width * 0.65, accent_y), 1.2, color);
                }
            }
        }
        MathNode::Scripts { base, sup, sub } => {
            let base_box = measure_node(ui, base, font);
            let script_font = scaled_font(font, SCRIPT_FACTOR);
            let sup_box = sup.as_ref().map(|s| measure_node(ui, s, &script_font));
            let sub_box = sub.as_ref().map(|s| measure_node(ui, s, &script_font));
            let (sup_offset, sub_offset) = script_baseline_offsets(base_box, sup_box, sub_box);

            // Draw base
            draw_node(ui, base, pos, baseline_y, font, color);

            let script_x = pos.x + base_box.width;

            // Draw superscript
            if let (Some(s), Some(s_box), Some(offset)) = (sup.as_ref(), sup_box, sup_offset) {
                let sup_baseline = baseline_y - offset - s_box.baseline_shift;
                let sup_top = sup_baseline - s_box.ascent;
                draw_node(ui, s, Pos2::new(script_x, sup_top), sup_baseline, &script_font, color);
            }

            // Draw subscript
            if let (Some(s), Some(s_box), Some(offset)) = (sub.as_ref(), sub_box, sub_offset) {
                let sub_baseline = baseline_y + offset - s_box.baseline_shift;
                let sub_top = sub_baseline - s_box.ascent;
                draw_node(ui, s, Pos2::new(script_x, sub_top), sub_baseline, &script_font, color);
            }
        }
        MathNode::Row(nodes) => {
            let mut x = pos.x;
            for (i, child) in nodes.iter().enumerate() {
                let before_space = atom_space(child);
                let after_space = before_space;
                if i > 0 && before_space > 0.0 && !matches!(nodes.get(i - 1), Some(MathNode::Space(_))) {
                    x += before_space;
                }
                let child_box = measure_node(ui, child, font);
                let child_baseline = baseline_y - child_box.baseline_shift;
                let child_top = child_baseline - child_box.ascent;
                draw_node(ui, child, Pos2::new(x, child_top), child_baseline, font, color);
                x += child_box.width;
                if i + 1 < nodes.len() && after_space > 0.0 && !matches!(nodes.get(i + 1), Some(MathNode::Space(_))) {
                    x += after_space;
                }
            }
        }
    }
}

/// Draw a parenthesis from `top_y` to `bottom_y` at `x`. `left` = true for `(`, false for `)`.
fn draw_paren(painter: &egui::Painter, x: f32, top_y: f32, bottom_y: f32, max_bulge: f32, left: bool, stroke: egui::Stroke) {
    let height = bottom_y - top_y;
    let bulge = (height * 0.15).clamp(0.0, max_bulge);
    let sign: f32 = if left { -1.0 } else { 1.0 };

    // Simple 3-segment approximation of a parenthesis curve
    let segments = 8usize;
    let mut points = Vec::with_capacity(segments.saturating_add(1));
    for s in 0..=segments {
        let t = s as f32 / segments as f32;
        let y = top_y + height * t;
        // Parabolic bulge: maximum at the middle
        let offset = bulge * 4.0 * t * (1.0 - t);
        points.push(Pos2::new(x + sign * offset, y));
    }

    for pair in points.windows(2) {
        if let [a, b] = pair {
            painter.line_segment([*a, *b], stroke);
        }
    }
}

/// Get the row height for a font.
fn row_height(ui: &Ui, font: &FontId) -> f32 {
    ui.painter().layout_no_wrap(String::from("M"), font.clone(), Color32::PLACEHOLDER).size().y
}

fn symbol_render_mode(ui: &Ui, text: &str, command: &str, font: &FontId) -> SymbolRenderMode {
    // Eframe's WebRunner installs a Canvas fallback, which has_glyphs does not query.
    #[cfg(target_arch = "wasm32")]
    let has_glyph = true;
    #[cfg(not(target_arch = "wasm32"))]
    let has_glyph = ui.ctx().fonts_mut(|fonts| fonts.has_glyphs(font, text));

    let mode = symbol_render_mode_for(has_glyph, command);
    if mode == SymbolRenderMode::TextFallback {
        let font_stack = ui.ctx().fonts(|fonts| fonts.definitions().families.get(&font.family).cloned()).unwrap_or_default();
        let codepoints = text.chars().map(|character| format!("U+{:04X}", character as u32)).collect::<Vec<_>>().join(", ");
        log::debug!("math symbol fallback: command=\\{command}, codepoints={codepoints}, font stack={font_stack:?}");
    }
    mode
}

fn symbol_render_mode_for(has_glyph: bool, command: &str) -> SymbolRenderMode {
    if has_glyph {
        SymbolRenderMode::Font
    } else if let Some(symbol) = vector_symbol(command) {
        SymbolRenderMode::Vector(symbol)
    } else {
        SymbolRenderMode::TextFallback
    }
}

fn vector_symbol(command: &str) -> Option<VectorSymbol> {
    match command {
        "models" | "vDash" => Some(VectorSymbol::Models),
        "vdash" => Some(VectorSymbol::Vdash),
        "nvdash" => Some(VectorSymbol::NotVdash),
        "nvDash" => Some(VectorSymbol::NotModels),
        "Vdash" => Some(VectorSymbol::DoubleVdash),
        "dashv" => Some(VectorSymbol::Dashv),
        "forall" => Some(VectorSymbol::ForAll),
        "exists" => Some(VectorSymbol::Exists),
        "nexists" => Some(VectorSymbol::NotExists),
        "neg" | "lnot" => Some(VectorSymbol::Negation),
        "land" | "wedge" => Some(VectorSymbol::And),
        "lor" | "vee" => Some(VectorSymbol::Or),
        "implies" | "Rightarrow" => Some(VectorSymbol::Implies),
        "iff" | "Leftrightarrow" => Some(VectorSymbol::Iff),
        "top" => Some(VectorSymbol::Top),
        "bot" => Some(VectorSymbol::Bottom),
        _ => None,
    }
}

fn vector_symbol_lines(symbol: VectorSymbol) -> &'static [(f32, f32, f32, f32)] {
    const MODELS: &[(f32, f32, f32, f32)] = &[(0.18, -0.72, 0.18, 0.1), (0.18, -0.55, 0.84, -0.55), (0.18, -0.18, 0.84, -0.18)];
    const VDASH: &[(f32, f32, f32, f32)] = &[(0.18, -0.72, 0.18, 0.1), (0.18, -0.18, 0.84, -0.18)];
    const NOT_VDASH: &[(f32, f32, f32, f32)] = &[(0.18, -0.72, 0.18, 0.1), (0.18, -0.18, 0.84, -0.18), (0.03, 0.08, 0.88, -0.75)];
    const NOT_MODELS: &[(f32, f32, f32, f32)] =
        &[(0.18, -0.72, 0.18, 0.1), (0.18, -0.55, 0.84, -0.55), (0.18, -0.18, 0.84, -0.18), (0.03, 0.08, 0.88, -0.75)];
    const DOUBLE_VDASH: &[(f32, f32, f32, f32)] = &[(0.14, -0.72, 0.14, 0.1), (0.28, -0.72, 0.28, 0.1), (0.28, -0.18, 0.84, -0.18)];
    const DASHV: &[(f32, f32, f32, f32)] = &[(0.82, -0.72, 0.82, 0.1), (0.16, -0.18, 0.82, -0.18)];
    const FORALL: &[(f32, f32, f32, f32)] = &[(0.08, 0.08, 0.31, -0.72), (0.31, -0.72, 0.68, -0.72), (0.68, -0.72, 0.91, 0.08)];
    const EXISTS: &[(f32, f32, f32, f32)] =
        &[(0.18, -0.72, 0.84, -0.72), (0.34, -0.31, 0.84, -0.31), (0.18, 0.08, 0.84, 0.08), (0.84, -0.72, 0.84, 0.08)];
    const NOT_EXISTS: &[(f32, f32, f32, f32)] =
        &[(0.18, -0.72, 0.84, -0.72), (0.34, -0.31, 0.84, -0.31), (0.18, 0.08, 0.84, 0.08), (0.84, -0.72, 0.84, 0.08), (0.05, 0.08, 0.91, -0.75)];
    const NEGATION: &[(f32, f32, f32, f32)] = &[(0.12, -0.4, 0.88, -0.4), (0.88, -0.4, 0.88, 0.08)];
    const AND: &[(f32, f32, f32, f32)] = &[(0.1, 0.08, 0.5, -0.7), (0.5, -0.7, 0.9, 0.08)];
    const OR: &[(f32, f32, f32, f32)] = &[(0.1, -0.7, 0.5, 0.08), (0.5, 0.08, 0.9, -0.7)];
    const IMPLIES: &[(f32, f32, f32, f32)] =
        &[(0.05, -0.38, 0.72, -0.38), (0.05, -0.12, 0.72, -0.12), (0.55, -0.58, 0.88, -0.25), (0.88, -0.25, 0.55, 0.08)];
    const IFF: &[(f32, f32, f32, f32)] = &[
        (0.18, -0.38, 0.82, -0.38),
        (0.18, -0.12, 0.82, -0.12),
        (0.18, -0.38, 0.42, -0.62),
        (0.18, -0.38, 0.42, -0.14),
        (0.82, -0.12, 0.58, -0.36),
        (0.82, -0.12, 0.58, 0.12),
    ];
    const TOP: &[(f32, f32, f32, f32)] = &[(0.12, -0.68, 0.88, -0.68), (0.5, -0.68, 0.5, 0.1)];
    const BOTTOM: &[(f32, f32, f32, f32)] = &[(0.12, 0.08, 0.88, 0.08), (0.5, -0.68, 0.5, 0.08)];

    match symbol {
        VectorSymbol::Models => MODELS,
        VectorSymbol::Vdash => VDASH,
        VectorSymbol::NotVdash => NOT_VDASH,
        VectorSymbol::NotModels => NOT_MODELS,
        VectorSymbol::DoubleVdash => DOUBLE_VDASH,
        VectorSymbol::Dashv => DASHV,
        VectorSymbol::ForAll => FORALL,
        VectorSymbol::Exists => EXISTS,
        VectorSymbol::NotExists => NOT_EXISTS,
        VectorSymbol::Negation => NEGATION,
        VectorSymbol::And => AND,
        VectorSymbol::Or => OR,
        VectorSymbol::Implies => IMPLIES,
        VectorSymbol::Iff => IFF,
        VectorSymbol::Top => TOP,
        VectorSymbol::Bottom => BOTTOM,
    }
}

fn measure_symbol(ui: &Ui, text: &str, command: &str, font: &FontId) -> MathBox {
    match symbol_render_mode(ui, text, command, font) {
        SymbolRenderMode::Font => measure_text(ui, text, font),
        SymbolRenderMode::Vector(_) => {
            let metrics = measure_text(ui, "M", font);
            MathBox {
                width: font.size * 0.95,
                ascent: metrics.ascent.max(font.size * 0.75),
                descent: metrics.descent.max(font.size * 0.12),
                baseline_shift: 0.0,
            }
        }
        SymbolRenderMode::TextFallback => measure_text(ui, command, font),
    }
}

fn draw_symbol(ui: &Ui, text: &str, command: &str, pos: Pos2, baseline_y: f32, font: &FontId, color: Color32) {
    match symbol_render_mode(ui, text, command, font) {
        SymbolRenderMode::Font => draw_text_on_baseline(ui, text, pos.x, baseline_y, font, color),
        SymbolRenderMode::Vector(symbol) => draw_vector_symbol(ui, symbol, pos, baseline_y, font, color),
        SymbolRenderMode::TextFallback => draw_text_on_baseline(ui, command, pos.x, baseline_y, font, color),
    }
}

fn draw_vector_symbol(ui: &Ui, symbol: VectorSymbol, pos: Pos2, baseline_y: f32, font: &FontId, color: Color32) {
    let size = font.size;
    let stroke = egui::Stroke::new((size * 0.075).max(0.8), color);
    for &(x1, y1, x2, y2) in vector_symbol_lines(symbol) {
        ui.painter()
            .line_segment([Pos2::new(pos.x + x1 * size, baseline_y + y1 * size), Pos2::new(pos.x + x2 * size, baseline_y + y2 * size)], stroke);
    }
}

fn draw_text_on_baseline(ui: &Ui, text: &str, x: f32, baseline_y: f32, font: &FontId, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), color);
    let baseline_offset =
        galley.rows.iter().find_map(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y)).unwrap_or_else(|| row_height(ui, font) * 0.8);
    ui.painter().galley(Pos2::new(x, baseline_y - baseline_offset), galley, color);
}

/// Create a font scaled by `factor`, preserving the requested ratio for nested scripts.
fn scaled_font(base: &FontId, factor: f32) -> FontId {
    FontId::new(base.size * factor, base.family.clone())
}

// ─── Public rendering API ─────────────────────────────────────────────────────

/// Scale factor for math formulas in tooltips relative to base text.
const MATH_TOOLTIP_SCALE: f32 = 1.2;
/// Display math vertical margin (points).
const DISPLAY_MATH_MARGIN: f32 = 6.0;
/// Maximum natural width of a math note or hover tooltip before wrapping.
pub const MAX_TOOLTIP_WIDTH: f32 = 440.0;

enum InlineContent {
    Text(String),
    Math(MathNode),
}

struct InlineItem {
    content: InlineContent,
    font: FontId,
    metrics: MathBox,
    is_math: bool,
    is_whitespace: bool,
}

struct InlineLine {
    items: Vec<InlineItem>,
    width: f32,
    ascent: f32,
    descent: f32,
    gap_after: f32,
}

enum RenderBlock {
    Inline(Vec<InlineLine>),
    Display(MathNode, MathBox, FontId),
    DisplayText(String, MathBox, FontId),
}

fn append_plain_pieces(plain: &str, pieces: &mut Vec<InlineContent>) {
    let mut run = String::new();
    let mut run_is_whitespace = None;
    for character in plain.chars() {
        if character == '\n' {
            if !run.is_empty() {
                pieces.push(InlineContent::Text(std::mem::take(&mut run)));
            }
            run_is_whitespace = None;
            pieces.push(InlineContent::Text("\n".to_owned()));
            continue;
        }
        let is_whitespace = character.is_whitespace();
        if run_is_whitespace.is_some_and(|previous| previous != is_whitespace) && !run.is_empty() {
            pieces.push(InlineContent::Text(std::mem::take(&mut run)));
        }
        run_is_whitespace = Some(is_whitespace);
        run.push(character);
    }
    if !run.is_empty() {
        pieces.push(InlineContent::Text(run));
    }
}

fn make_inline_item(ui: &Ui, content: InlineContent, font: &FontId, math_font: &FontId, max_width: f32) -> Vec<InlineItem> {
    match content {
        InlineContent::Text(text) if text == "\n" => Vec::new(),
        InlineContent::Text(text) => {
            let whitespace = text.chars().all(char::is_whitespace);
            let metrics = measure_text(ui, &text, font);
            if !whitespace && metrics.width > max_width {
                text.chars()
                    .map(|character| {
                        let text = character.to_string();
                        let metrics = measure_text(ui, &text, font);
                        InlineItem {
                            is_whitespace: character.is_whitespace(),
                            content: InlineContent::Text(text),
                            font: font.clone(),
                            metrics,
                            is_math: false,
                        }
                    })
                    .collect()
            } else {
                vec![InlineItem { content: InlineContent::Text(text), font: font.clone(), metrics, is_math: false, is_whitespace: whitespace }]
            }
        }
        InlineContent::Math(node) => {
            let mut item_font = math_font.clone();
            let mut metrics = measure_node(ui, &node, &item_font);
            for _ in 0..4 {
                if metrics.width <= max_width || metrics.width <= 0.0 {
                    break;
                }
                item_font.size *= (max_width / metrics.width).clamp(0.1, 0.95);
                metrics = measure_node(ui, &node, &item_font);
            }
            vec![InlineItem { content: InlineContent::Math(node), font: item_font, metrics, is_math: true, is_whitespace: false }]
        }
    }
}

fn layout_inline(ui: &Ui, pieces: Vec<InlineContent>, font: &FontId, math_font: &FontId, max_width: f32) -> Vec<InlineLine> {
    let mut lines = Vec::new();
    let base_height = measure_text(ui, "M", font).height();
    let mut line = InlineLine { items: Vec::new(), width: 0.0, ascent: 0.0, descent: 0.0, gap_after: 0.0 };
    for piece in pieces {
        if matches!(&piece, InlineContent::Text(text) if text == "\n") {
            if line.items.is_empty() {
                line.ascent = base_height * 0.8;
                line.descent = base_height * 0.2;
            }
            lines.push(line);
            line = InlineLine { items: Vec::new(), width: 0.0, ascent: 0.0, descent: 0.0, gap_after: 0.0 };
            continue;
        }
        for item in make_inline_item(ui, piece, font, math_font, max_width) {
            if item.is_whitespace && line.width + item.metrics.width > max_width {
                continue;
            }
            if !item.is_whitespace && !line.items.is_empty() && line.width + item.metrics.width > max_width {
                lines.push(line);
                line = InlineLine { items: Vec::new(), width: 0.0, ascent: 0.0, descent: 0.0, gap_after: 0.0 };
            }
            line.width += item.metrics.width;
            line.ascent = line.ascent.max(item.metrics.above_baseline());
            line.descent = line.descent.max(item.metrics.below_baseline());
            if item.is_math && item.metrics.size().y > base_height * 1.25 {
                line.gap_after = 2.0;
            }
            line.items.push(item);
        }
    }
    if !line.items.is_empty() || lines.is_empty() {
        if line.items.is_empty() {
            line.ascent = base_height * 0.8;
            line.descent = base_height * 0.2;
        }
        lines.push(line);
    }
    lines
}

/// Renders text with math formulas into an `egui::Ui`.
///
/// Plain text and inline math flow together. Block formulas (`$$ ... $$`)
/// are displayed centered on their own line with vertical padding.
///
/// The 2D layout engine draws fractions, binomials, radicals, and scripts
/// with proper mathematical typography using `egui::Painter`.
///
/// Inherits the given `font` and `color`, ensuring proper contrast and scaling across
/// light popup boxes and dark hover tooltips.
pub fn render_math_text(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    let segments = parse_math_segments(text);
    let text_scale = (font.size / 12.5 * ui.ctx().zoom_factor()).max(0.5);
    let viewport_limit = (ui.ctx().content_rect().width() - 32.0).max(1.0);
    let max_width = (MAX_TOOLTIP_WIDTH * text_scale).min(viewport_limit);
    ui.set_max_width(max_width);
    if segments.is_empty() {
        return ui.allocate_response(Vec2::ZERO, egui::Sense::hover());
    }

    let has_math = segments.iter().any(|s| matches!(s, MathSegment::InlineMath(_) | MathSegment::BlockMath(_)));
    if !has_math {
        return ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).wrap());
    }

    let available_width = ui.available_width().min(max_width).max(1.0);
    let math_font = FontId::new(font.size * MATH_TOOLTIP_SCALE, egui::FontFamily::Name("math".into()));
    let mut blocks = Vec::new();
    let mut inline_pieces = Vec::new();
    let flush_inline = |pieces: &mut Vec<InlineContent>, blocks: &mut Vec<RenderBlock>| {
        if !pieces.is_empty() {
            blocks.push(RenderBlock::Inline(layout_inline(ui, std::mem::take(pieces), &font, &math_font, available_width)));
        }
    };

    for segment in segments {
        match segment {
            MathSegment::PlainText(plain) => append_plain_pieces(plain, &mut inline_pieces),
            MathSegment::InlineMath(formula) => match parse_math_ast(formula) {
                Ok(node) => inline_pieces.push(InlineContent::Math(node)),
                Err(_) => append_plain_pieces(&format!("${formula}$"), &mut inline_pieces),
            },
            MathSegment::BlockMath(formula) => {
                flush_inline(&mut inline_pieces, &mut blocks);
                match parse_math_ast(formula) {
                    Ok(node) => {
                        let mut display_font = math_font.clone();
                        let mut measured = measure_node(ui, &node, &display_font);
                        for _ in 0..5 {
                            if measured.width <= available_width || measured.width <= 0.0 {
                                break;
                            }
                            display_font.size *= (available_width / measured.width).clamp(0.1, 0.95);
                            measured = measure_node(ui, &node, &display_font);
                        }
                        blocks.push(RenderBlock::Display(node, measured, display_font));
                    }
                    Err(_) => {
                        let fallback = format!("$${formula}$$");
                        let mut fallback_font = font.clone();
                        let mut measured = measure_text(ui, &fallback, &fallback_font);
                        for _ in 0..5 {
                            if measured.width <= available_width || measured.width <= 0.0 {
                                break;
                            }
                            fallback_font.size *= (available_width / measured.width).clamp(0.1, 0.95);
                            measured = measure_text(ui, &fallback, &fallback_font);
                        }
                        blocks.push(RenderBlock::DisplayText(fallback, measured, fallback_font));
                    }
                }
            }
        }
    }
    flush_inline(&mut inline_pieces, &mut blocks);

    let content_width = blocks
        .iter()
        .map(|block| match block {
            RenderBlock::Inline(lines) => lines.iter().map(|line| line.width).fold(0.0f32, f32::max),
            RenderBlock::Display(_, metrics, _) | RenderBlock::DisplayText(_, metrics, _) => metrics.width,
        })
        .fold(0.0f32, f32::max)
        .min(max_width);
    let total_height = blocks
        .iter()
        .map(|block| match block {
            RenderBlock::Inline(lines) => lines.iter().map(|line| line.ascent + line.descent + line.gap_after).sum::<f32>(),
            RenderBlock::Display(_, metrics, _) | RenderBlock::DisplayText(_, metrics, _) => {
                metrics.size().y + DISPLAY_MATH_MARGIN * text_scale * 2.0
            }
        })
        .sum::<f32>();
    let (rect, response) = ui.allocate_exact_size(vec2(content_width, total_height), egui::Sense::hover());
    let accessible_text = format_text_with_math(text);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &accessible_text));
    let mut y = rect.min.y;
    for block in blocks {
        match block {
            RenderBlock::Inline(lines) => {
                for line in lines {
                    let baseline = y + line.ascent;
                    let mut x = rect.min.x;
                    for item in line.items {
                        match item.content {
                            InlineContent::Text(text) => draw_text_on_baseline(ui, &text, x, baseline, &item.font, color),
                            InlineContent::Math(node) => {
                                let node_baseline = baseline - item.metrics.baseline_shift;
                                draw_node(ui, &node, Pos2::new(x, node_baseline - item.metrics.ascent), node_baseline, &item.font, color);
                            }
                        }
                        x += item.metrics.width;
                    }
                    y += line.ascent + line.descent + line.gap_after;
                }
            }
            RenderBlock::Display(node, metrics, display_font) => {
                y += DISPLAY_MATH_MARGIN * text_scale;
                let offset_x = ((content_width - metrics.width) / 2.0).max(0.0);
                let baseline = y + metrics.ascent;
                draw_node(ui, &node, Pos2::new(rect.min.x + offset_x, y), baseline, &display_font, color);
                y += metrics.size().y + DISPLAY_MATH_MARGIN * text_scale;
            }
            RenderBlock::DisplayText(value, metrics, display_font) => {
                y += DISPLAY_MATH_MARGIN * text_scale;
                let offset_x = ((content_width - metrics.width) / 2.0).max(0.0);
                let baseline = y + metrics.ascent;
                draw_text_on_baseline(ui, &value, rect.min.x + offset_x, baseline, &display_font, color);
                y += metrics.size().y + DISPLAY_MATH_MARGIN * text_scale;
            }
        }
    }
    response
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Segment parser tests ──────────────────────────────────────────

    #[test]
    fn single_inline_formula_issue_7223() {
        let input = "($KB \\models \\alpha$)";
        let segments = parse_math_segments(input);
        assert_eq!(segments, vec![MathSegment::PlainText("("), MathSegment::InlineMath("KB \\models \\alpha"), MathSegment::PlainText(")")]);
        let formatted = format_text_with_math(input);
        assert_eq!(formatted, "(KB ⊨ α)");
    }

    #[test]
    fn mixed_text_and_formulas() {
        let input = "Given $KB \\models \\alpha$, we conclude that $\\alpha$ is valid.";
        let segments = parse_math_segments(input);
        assert_eq!(
            segments,
            vec![
                MathSegment::PlainText("Given "),
                MathSegment::InlineMath("KB \\models \\alpha"),
                MathSegment::PlainText(", we conclude that "),
                MathSegment::InlineMath("\\alpha"),
                MathSegment::PlainText(" is valid."),
            ]
        );
        let formatted = format_text_with_math(input);
        assert_eq!(formatted, "Given KB ⊨ α, we conclude that α is valid.");
    }

    #[test]
    fn multiple_formulas_in_same_note() {
        let input = "First $x = 1$, second $y = 2$, third $z = 3$.";
        let segments = parse_math_segments(input);
        assert_eq!(segments.len(), 7);
        let formatted = format_text_with_math(input);
        assert_eq!(formatted, "First x = 1, second y = 2, third z = 3.");
    }

    #[test]
    fn isolated_dollar_is_plain_text() {
        let input = "The price is $20 for one.";
        let segments = parse_math_segments(input);
        assert_eq!(segments, vec![MathSegment::PlainText("The price is $20 for one.")]);
        assert_eq!(format_text_with_math(input), input);
    }

    #[test]
    fn isolated_dollars_with_spaces() {
        let input = "Between $10 and $20 total.";
        let segments = parse_math_segments(input);
        assert_eq!(segments, vec![MathSegment::PlainText("Between $10 and $20 total.")]);
        assert_eq!(format_text_with_math(input), input);
    }

    #[test]
    fn many_unclosed_inline_candidates_are_scanned_once() {
        let input = "$x ".repeat(10_000);
        assert_eq!(parse_math_segments(&input), vec![MathSegment::PlainText(&input)]);
    }

    #[test]
    fn unclosed_delimiter_is_plain_text() {
        let input = "This is $x + y with no end";
        let segments = parse_math_segments(input);
        assert_eq!(segments, vec![MathSegment::PlainText("This is $x + y with no end")]);
        assert_eq!(format_text_with_math(input), input);
    }

    #[test]
    fn oversized_math_input_stays_plain_text() {
        let input = format!("${}$", "x".repeat(MAX_MATH_INPUT_BYTES));
        assert_eq!(parse_math_segments(&input), vec![MathSegment::PlainText(&input)]);
        assert_eq!(format_text_with_math(&input), input);
        assert_eq!(parse_math_ast(&input), Err("math input exceeds size limit"));
    }

    #[test]
    fn block_math_delimiters() {
        let input = "Theorem:\n$$E = mc^2$$\nend of proof.";
        let segments = parse_math_segments(input);
        assert_eq!(
            segments,
            vec![MathSegment::PlainText("Theorem:\n"), MathSegment::BlockMath("E = mc^2"), MathSegment::PlainText("\nend of proof.")]
        );
        let formatted = format_text_with_math(input);
        assert_eq!(formatted, "Theorem:\n\nE = mc²\n\nend of proof.");
    }

    #[test]
    fn escaped_dollar_is_plain_text() {
        let input = "Price: \\$20 and \\$30.";
        let segments = parse_math_segments(input);
        assert_eq!(segments, vec![MathSegment::PlainText("Price: \\$20 and \\$30.")]);
    }

    #[test]
    fn malformed_latex_graceful_fallback() {
        let input = "Formula $\\frac{unclosed$ remains plain.";
        let segments = parse_math_segments(input);
        assert_eq!(
            segments,
            vec![MathSegment::PlainText("Formula "), MathSegment::InlineMath("\\frac{unclosed"), MathSegment::PlainText(" remains plain.")]
        );
        // The AST parser will handle the unclosed brace gracefully
        let formatted = format_text_with_math(input);
        // Should contain the partial parse, not crash
        assert!(!formatted.is_empty());
    }

    #[test]
    fn greek_and_relations() {
        let input = "$\\alpha + \\beta \\le \\gamma \\implies \\delta \\neq \\epsilon$";
        let formatted = format_text_with_math(input);
        assert_eq!(formatted, "α + β ≤ γ ⟹ δ ≠ ε");
    }

    #[test]
    fn superscripts_and_subscripts() {
        let input = "$x^2 + y_1 = z_{ij}$";
        let formatted = format_text_with_math(input);
        assert_eq!(formatted, "x² + y₁ = zᵢⱼ");
    }

    #[test]
    fn logic_symbols() {
        let input = "$P \\land Q \\lor \\neg R \\vdash S$";
        let formatted = format_text_with_math(input);
        assert_eq!(formatted, "P ∧ Q ∨ ¬R ⊢ S");
    }

    #[test]
    fn blackboard_bold_and_quantifiers() {
        let input = "$\\forall x \\in \\mathbb{R}, \\exists y \\in \\mathbb{N}$";
        let formatted = format_text_with_math(input);
        assert_eq!(formatted, "∀x ∈ ℝ, ∃y ∈ ℕ");
    }

    // ── AST parser tests ──────────────────────────────────────────────

    #[test]
    fn ast_frac_basic() {
        let ast = parse_math_ast("\\frac{a}{b}").unwrap();
        assert!(matches!(ast, MathNode::Frac { .. }));
        let text = node_to_text(&ast);
        assert_eq!(text, "a/b");
    }

    #[test]
    fn ast_frac_complex() {
        let ast = parse_math_ast("\\frac{a + b}{c}").unwrap();
        let text = node_to_text(&ast);
        assert_eq!(text, "(a + b)/c");
    }

    #[test]
    fn ast_nested_denominators_are_parenthesized() {
        assert_eq!(format_latex_math("\\frac{a}{\\frac{b}{c}}"), Ok("a/(b/c)".into()));
        assert_eq!(format_latex_math("\\frac{a}{bc}"), Ok("a/bc".into()));
    }

    #[test]
    fn unbraced_nested_commands_obey_the_recursion_limit() {
        let input = format!("{}x", "\\sqrt".repeat(MAX_DEPTH * 2));
        assert_eq!(parse_math_ast(&input), Err("recursion limit exceeded"));
    }

    #[test]
    fn ast_binom_basic() {
        let ast = parse_math_ast("\\binom{n}{2}").unwrap();
        assert!(matches!(ast, MathNode::Binom { .. }));
        let text = node_to_text(&ast);
        assert_eq!(text, "C(n,2)");
    }

    #[test]
    fn ast_binom_does_not_corrupt() {
        // The old parser turned \binom{n}{2} into \binomn2.
        // The AST parser must not corrupt it.
        let ast = parse_math_ast("\\binom{n}{2}").unwrap();
        let text = node_to_text(&ast);
        assert!(!text.contains("\\binomn2"), "binom must not be corrupted: got {text}");
        assert!(text.contains('n'), "must contain 'n': got {text}");
        assert!(text.contains('2'), "must contain '2': got {text}");
    }

    #[test]
    fn ast_sqrt_basic() {
        let ast = parse_math_ast("\\sqrt{x}").unwrap();
        assert!(matches!(ast, MathNode::Sqrt { degree: None, .. }));
        let text = node_to_text(&ast);
        assert_eq!(text, "√x");
    }

    #[test]
    fn ast_sqrt_with_degree() {
        let ast = parse_math_ast("\\sqrt[3]{x}").unwrap();
        assert!(matches!(ast, MathNode::Sqrt { degree: Some(_), .. }));
        let text = node_to_text(&ast);
        assert_eq!(text, "3√(x)");
    }

    #[test]
    fn ast_nested_frac_in_binom() {
        let input = "\\frac{\\binom{n}{k}}{2^n}";
        let ast = parse_math_ast(input).unwrap();
        let text = node_to_text(&ast);
        assert!(text.contains("C(n,k)"), "must contain binom: got {text}");
        assert!(text.contains("2ⁿ"), "must contain 2^n: got {text}");
    }

    #[test]
    fn ast_complex_display_math() {
        let input = "\\binom{n}{2} = \\frac{n(n-1)}{2}";
        let ast = parse_math_ast(input).unwrap();
        let text = node_to_text(&ast);
        assert!(text.contains("C(n,2)"), "must contain binom: got {text}");
        assert!(text.contains("n(n-1)"), "must contain numerator: got {text}");
    }

    #[test]
    fn ast_logic_expression() {
        let input = "KB \\models \\alpha \\iff (KB \\land \\neg \\alpha) \\models \\bot";
        let ast = parse_math_ast(input).unwrap();
        let text = node_to_text(&ast);
        assert!(text.contains("⊨"), "must contain ⊨: got {text}");
        assert!(text.contains("⟺"), "must contain ⟺: got {text}");
        assert!(text.contains("¬"), "must contain ¬: got {text}");
        assert!(text.contains("⊥"), "must contain ⊥: got {text}");
    }

    #[test]
    fn ast_quantifiers() {
        let input = "\\forall x \\in S, \\exists y : x \\subseteq y";
        let ast = parse_math_ast(input).unwrap();
        let text = node_to_text(&ast);
        assert!(text.contains("∀"), "must contain ∀: got {text}");
        assert!(text.contains("∈"), "must contain ∈: got {text}");
        assert!(text.contains("∃"), "must contain ∃: got {text}");
        assert!(text.contains("⊆"), "must contain ⊆: got {text}");
    }

    #[test]
    fn ast_unknown_command_graceful() {
        let input = "\\foo{x}";
        let ast = parse_math_ast(input).unwrap();
        assert_eq!(node_to_text(&ast), "foo x");
        assert_eq!(format_text_with_math(&format!("${input}$")), "foo x");
    }

    #[test]
    fn ast_deeply_nested() {
        // Test bounded recursion: deeply nested braces should not crash
        let input = "\\frac{\\frac{\\frac{a}{b}}{c}}{d}";
        let ast = parse_math_ast(input).unwrap();
        let text = node_to_text(&ast);
        assert!(text.contains('a'), "must contain 'a': got {text}");
        assert!(text.contains('d'), "must contain 'd': got {text}");
    }

    #[test]
    fn ast_empty_input() {
        let ast = parse_math_ast("").unwrap();
        let text = node_to_text(&ast);
        assert!(text.is_empty());
    }

    #[test]
    fn ast_single_character() {
        let ast = parse_math_ast("x").unwrap();
        let text = node_to_text(&ast);
        assert_eq!(text, "x");
    }

    #[test]
    fn ast_scripts_combined() {
        let input = "x^{2}_{i}";
        let ast = parse_math_ast(input).unwrap();
        let text = node_to_text(&ast);
        assert_eq!(text, "x²ᵢ");
    }

    #[test]
    fn ast_fractions_and_square_roots() {
        let input = "\\frac{1}{2} + \\sqrt{x} = \\frac{a + b}{c}";
        let formatted = format_latex_math(input).unwrap_or_default();
        assert!(formatted.contains("1/2"), "must contain 1/2: got {formatted}");
        assert!(formatted.contains("√"), "must contain √: got {formatted}");
        assert!(formatted.contains("(a + b)/c"), "must contain (a + b)/c: got {formatted}");
    }

    #[test]
    fn ast_recursion_limit() {
        // Build deeply nested input that should hit the recursion limit
        let mut input = String::from("a");
        for _ in 0..40 {
            input = format!("\\frac{{{input}}}{{b}}");
        }
        assert_eq!(parse_math_ast(&input), Err("recursion limit exceeded"));
    }

    #[test]
    fn required_nested_math_examples_keep_their_ast_structure() {
        let examples = [
            ("\\binom{n}{2} = \\frac{n(n-1)}{2}", true, true),
            ("\\frac{\\binom{n}{k}}{2^n}", true, true),
            ("KB \\models \\alpha \\iff (KB \\land \\neg \\alpha) \\models \\bot", false, false),
            ("\\forall x \\in S, \\exists y : x \\subseteq y", false, false),
        ];
        for (input, fraction, binom) in examples {
            let ast = parse_math_ast(input).unwrap();
            let text = node_to_text(&ast);
            assert_eq!(contains_node(&ast, |node| matches!(node, MathNode::Frac { .. })), fraction, "{input}");
            assert_eq!(contains_node(&ast, |node| matches!(node, MathNode::Binom { .. })), binom, "{input}");
            assert!(!text.contains("\\binomn2"), "{input}: {text}");
            assert!(!text.contains('{') && !text.contains('}'), "{input}: {text}");
        }
    }

    #[test]
    fn malformed_groups_fall_back_instead_of_dropping_source() {
        for input in ["\\frac{a}{b", "\\sqrt[3{x}", "a}b"] {
            assert!(parse_math_ast(input).is_err(), "{input}");
        }
        let input = "before $\\frac{a}{b$ after";
        assert_eq!(format_text_with_math(input), input);
    }

    #[test]
    fn mapped_symbols_select_a_drawable_rendering_path() {
        let symbols = [
            ("models", "⊨"),
            ("vdash", "⊢"),
            ("top", "⊤"),
            ("bot", "⊥"),
            ("land", "∧"),
            ("lor", "∨"),
            ("neg", "¬"),
            ("implies", "⟹"),
            ("iff", "⟺"),
            ("forall", "∀"),
            ("exists", "∃"),
            ("in", "∈"),
            ("notin", "∉"),
            ("subset", "⊂"),
            ("subseteq", "⊆"),
            ("cup", "∪"),
            ("cap", "∩"),
            ("emptyset", "∅"),
            ("neq", "≠"),
            ("leq", "≤"),
            ("geq", "≥"),
            ("approx", "≈"),
            ("equiv", "≡"),
            ("times", "×"),
            ("cdot", "·"),
            ("pm", "±"),
            ("infty", "∞"),
            ("partial", "∂"),
            ("nabla", "∇"),
        ];
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(13.0, egui::FontFamily::Name("math".into()));
            for (command, symbol) in symbols {
                let mode = symbol_render_mode(ui, symbol, command, &font);
                match mode {
                    SymbolRenderMode::Font => assert!(ui.ctx().fonts_mut(|fonts| fonts.has_glyphs(&font, symbol))),
                    SymbolRenderMode::Vector(shape) => assert_eq!(vector_symbol(command), Some(shape)),
                    SymbolRenderMode::TextFallback => {
                        assert!(ui.ctx().fonts_mut(|fonts| fonts.has_glyphs(&font, command)), "fallback for \\{command} has no glyphs");
                    }
                }
                if let Some(shape) = vector_symbol(command) {
                    assert_ne!(mode, SymbolRenderMode::TextFallback, "\\{command} must not fall back to its command name");
                    if let SymbolRenderMode::Vector(selected) = mode {
                        assert_eq!(selected, shape);
                    }
                }
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn every_symbol_has_a_glyph_safe_display_fallback() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::installed_font_definitions(crate::theme::CjkPreference::Default));
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(13.0, egui::FontFamily::Name("math".into()));
            for symbol in math_symbols::SYMBOLS {
                let mode = symbol_render_mode(ui, symbol.glyph, symbol.command, &font);
                match mode {
                    SymbolRenderMode::Font => {
                        #[cfg(not(target_arch = "wasm32"))]
                        assert!(ui.ctx().fonts_mut(|fonts| fonts.has_glyphs(&font, symbol.glyph)));
                    }
                    SymbolRenderMode::Vector(shape) => assert_eq!(vector_symbol(symbol.command), Some(shape)),
                    SymbolRenderMode::TextFallback => {
                        assert!(
                            ui.ctx().fonts_mut(|fonts| fonts.has_glyphs(&font, symbol.command)),
                            "fallback for \\{} has no glyphs",
                            symbol.command
                        );
                    }
                }
                if let Some(shape) = vector_symbol(symbol.command) {
                    assert_ne!(mode, SymbolRenderMode::TextFallback, "\\{} must not fall back to text", symbol.command);
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let has_glyph = ui.ctx().fonts_mut(|fonts| fonts.has_glyphs(&font, symbol.glyph));
                        assert_eq!(mode, if has_glyph { SymbolRenderMode::Font } else { SymbolRenderMode::Vector(shape) });
                    }
                }
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn missing_symbol_glyph_uses_the_readable_fallback() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(13.0, egui::FontFamily::Name("math".into()));
            let missing_glyph = "\u{10FFFF}";
            assert!(!ui.ctx().fonts_mut(|fonts| fonts.has_glyphs(&font, missing_glyph)));
            assert_eq!(symbol_render_mode(ui, missing_glyph, "missing", &font), SymbolRenderMode::TextFallback);
            assert!(ui.ctx().fonts_mut(|fonts| fonts.has_glyphs(&font, "missing")));
        });
        output.textures_delta.clear();
    }

    #[test]
    fn logical_symbol_vector_fallbacks_keep_unicode_and_baseline_metrics() {
        for (command, shape) in [
            ("models", VectorSymbol::Models),
            ("vDash", VectorSymbol::Models),
            ("vdash", VectorSymbol::Vdash),
            ("forall", VectorSymbol::ForAll),
            ("exists", VectorSymbol::Exists),
            ("neg", VectorSymbol::Negation),
            ("land", VectorSymbol::And),
            ("lor", VectorSymbol::Or),
            ("implies", VectorSymbol::Implies),
            ("iff", VectorSymbol::Iff),
            ("top", VectorSymbol::Top),
            ("bot", VectorSymbol::Bottom),
        ] {
            assert_eq!(vector_symbol(command), Some(shape));
            assert!(!vector_symbol_lines(shape).is_empty());
            assert_eq!(symbol_render_mode_for(false, command), SymbolRenderMode::Vector(shape));
            assert_eq!(symbol_render_mode_for(true, command), SymbolRenderMode::Font);
        }
    }

    #[test]
    fn vector_models_fallback_paints_strokes_at_the_requested_baseline() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(20.0, egui::FontFamily::Name("math".into()));
            let pos = Pos2::new(10.0, 20.0);
            let baseline = 35.0;
            draw_vector_symbol(ui, VectorSymbol::Models, pos, baseline, &font, Color32::BLACK);
        });
        let line_segments = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::LineSegment { points, .. } => Some(*points),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(line_segments.len(), vector_symbol_lines(VectorSymbol::Models).len());
        let vertical_stem = line_segments.iter().find(|points| (points[0].x - points[1].x).abs() < f32::EPSILON).expect("models has a vertical stem");
        assert!((vertical_stem[0].y - 20.6).abs() < 0.01);
        assert!((vertical_stem[1].y - 37.0).abs() < 0.01);
        output.textures_delta.clear();
    }

    #[test]
    fn models_renders_as_a_symbol_on_light_and_dark_comment_surfaces() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::installed_font_definitions(crate::theme::CjkPreference::Default));
        let mut mode = SymbolRenderMode::TextFallback;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let math_font = FontId::new(15.0, egui::FontFamily::Name("math".into()));
            mode = symbol_render_mode(ui, "⊨", "models", &math_font);
            assert_ne!(mode, SymbolRenderMode::TextFallback);

            render_math_text(ui, "KB $\\models$ α", crate::theme::regular(12.5), Color32::BLACK);
            render_math_text(ui, "KB $\\models$ α", crate::theme::regular(12.5), Color32::WHITE);
        });
        let text_shapes = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(text_shapes.iter().all(|text| !text.contains("models")));
        if mode == SymbolRenderMode::Font {
            assert!(text_shapes.iter().any(|text| text.contains('⊨')));
        } else {
            let line_count = output.shapes.iter().filter(|shape| matches!(&shape.shape, egui::Shape::LineSegment { .. })).count();
            assert!(line_count >= vector_symbol_lines(VectorSymbol::Models).len());
        }
        output.textures_delta.clear();
    }

    #[test]
    fn math_nodes_keep_their_baselines_and_grow_on_the_correct_side() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(15.0, egui::FontFamily::Name("math".into()));
            let base = measure_node(ui, &parse_math_ast("n").unwrap(), &font);
            let superscript = measure_node(ui, &parse_math_ast("n^2").unwrap(), &font);
            let subscript = measure_node(ui, &parse_math_ast("X_i").unwrap(), &font);
            let fraction = measure_node(ui, &parse_math_ast("\\frac{a}{b}").unwrap(), &font);
            let binomial = measure_node(ui, &parse_math_ast("\\binom{n}{2}").unwrap(), &font);

            assert_eq!(superscript.baseline_shift, base.baseline_shift);
            assert!(superscript.ascent > base.ascent);
            assert_eq!(subscript.baseline_shift, base.baseline_shift);
            assert!(subscript.descent > base.descent);
            let math_axis = row_height(ui, &font) * 0.25;
            assert!((fraction.baseline_shift - math_axis).abs() < 0.01);
            assert!((binomial.baseline_shift - math_axis).abs() < 0.01);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn subscript_paint_overlaps_the_base_baseline_within_measured_bounds() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let baseline = 100.0;
        let mut measured = None;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(15.0, egui::FontFamily::Name("math".into()));
            let node = parse_math_ast("X_i").unwrap();
            measured = Some(measure_node(ui, &node, &font));
            draw_node(ui, &node, Pos2::ZERO, baseline, &font, Color32::BLACK);
        });
        let bounds = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text.visual_bounding_rect()),
                    _ => None,
                })
                .expect("script text")
        };
        let subscript = bounds("i");
        let measured = measured.expect("measured script");
        let painted_bounds = bounds("X").union(subscript);
        assert!(subscript.top() < baseline && subscript.bottom() > baseline, "subscript straddles the base baseline: {subscript:?}");
        assert!(painted_bounds.top() >= baseline - measured.ascent, "painted top exceeds measured ascent: {painted_bounds:?}, {measured:?}");
        assert!(painted_bounds.bottom() <= baseline + measured.descent, "painted bottom exceeds measured descent: {painted_bounds:?}, {measured:?}");
        output.textures_delta.clear();
    }

    #[test]
    fn paired_scripts_keep_clearance_and_preserve_the_superscript_position() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(15.0, egui::FontFamily::Name("math".into()));
            let node = parse_math_ast("X^2_i").unwrap();
            draw_node(ui, &node, Pos2::ZERO, 100.0, &font, Color32::BLACK);
        });
        let bounds = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text.visual_bounding_rect()),
                    _ => None,
                })
                .expect("script text")
        };
        let superscript = bounds("2");
        let subscript = bounds("i");
        assert!(superscript.bottom() < subscript.top(), "scripts have clear separation: {superscript:?}, {subscript:?}");
        output.textures_delta.clear();
    }

    #[test]
    fn compound_subscript_baseline_shift_stays_within_measured_bounds() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let baseline = 100.0;
        let mut measured = None;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(15.0, egui::FontFamily::Name("math".into()));
            let node = parse_math_ast("X_{\\frac{a}{b}}").unwrap();
            measured = Some(measure_node(ui, &node, &font));
            draw_node(ui, &node, Pos2::ZERO, baseline, &font, Color32::BLACK);
        });
        let painted_bounds = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.visual_bounding_rect()),
                egui::Shape::LineSegment { points, .. } => Some(egui::Rect::from_two_pos(points[0], points[1])),
                _ => None,
            })
            .reduce(egui::Rect::union)
            .expect("compound script paint");
        let measured = measured.expect("measured compound script");
        assert!(painted_bounds.top() >= baseline - measured.ascent, "painted top exceeds measured ascent: {painted_bounds:?}, {measured:?}");
        assert!(painted_bounds.bottom() <= baseline + measured.descent, "painted bottom exceeds measured descent: {painted_bounds:?}, {measured:?}");
        output.textures_delta.clear();
    }

    #[test]
    fn nested_fraction_children_stay_on_their_side_of_the_outer_bar() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::new(15.0, egui::FontFamily::Name("math".into()));
            let node = parse_math_ast("\\frac{\\frac{a}{b}}{\\frac{c}{d}}").unwrap();
            draw_node(ui, &node, Pos2::ZERO, 100.0, &font, Color32::BLACK);
        });
        let outer_bar_y = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::LineSegment { points, .. } if (points[0].y - points[1].y).abs() < 0.01 => {
                    Some((points[0].y, (points[1].x - points[0].x).abs()))
                }
                _ => None,
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(y, _)| y)
            .expect("outer fraction bar");
        let text_bounds = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.visual_bounding_rect())),
                _ => None,
            })
            .collect::<Vec<_>>();
        let b = text_bounds.iter().find(|(text, _)| text == "b").map(|(_, rect)| *rect).expect("numerator denominator");
        let c = text_bounds.iter().find(|(text, _)| text == "c").map(|(_, rect)| *rect).expect("denominator numerator");
        assert!(b.bottom() < outer_bar_y, "nested numerator stays above the outer bar: {b:?}, bar {outer_bar_y}");
        assert!(c.top() > outer_bar_y, "nested denominator stays below the outer bar: {c:?}, bar {outer_bar_y}");
        output.textures_delta.clear();
    }

    #[test]
    fn display_math_stays_within_the_tooltip_width() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut rendered_width = None;
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 500.0))), ..Default::default() },
            |ui| {
                let formula = format!("$$\\frac{{{}}}{{2}}$$", "n + ".repeat(120));
                rendered_width = Some(render_math_text(ui, &formula, crate::theme::regular(12.5), Color32::BLACK).rect.width());
            },
        );
        output.textures_delta.clear();
        assert!(rendered_width.is_some_and(|width| width <= MAX_TOOLTIP_WIDTH), "tooltip width: {rendered_width:?}");
    }

    #[test]
    fn display_binomial_paint_stays_within_the_tooltip_width() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let formula = format!("$$\\binom{{{}}}{{k}}$$", "n + ".repeat(120));
        let mut response_rect = None;
        let mut output = ctx
            .run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 500.0))), ..Default::default() }, |ui| {
                response_rect = Some(render_math_text(ui, &formula, crate::theme::regular(12.5), Color32::BLACK).rect)
            });
        let painted_bounds = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.visual_bounding_rect()),
                egui::Shape::LineSegment { points, .. } => Some(egui::Rect::from_two_pos(points[0], points[1])),
                _ => None,
            })
            .reduce(egui::Rect::union)
            .expect("painted formula");
        output.textures_delta.clear();
        let rect = response_rect.expect("rendered formula");
        assert!(rect.width() <= MAX_TOOLTIP_WIDTH, "response width: {}", rect.width());
        assert!(painted_bounds.width() <= MAX_TOOLTIP_WIDTH + 1.0, "painted bounds {painted_bounds:?} exceed tooltip width from {rect:?}");
    }

    #[test]
    fn command_table_maps_relations_styles_and_operator_names() {
        let relation = parse_math_ast("\\models").unwrap();
        assert!(matches!(relation, MathNode::Symbol { text, class: AtomClass::Rel, .. } if text == "⊨"));
        assert_eq!(node_to_text(&parse_math_ast("\\mathcal{O}(n^2)").unwrap()), "𝒪(n²)");
        assert_eq!(node_to_text(&parse_math_ast("\\sin x").unwrap()), "sin x");
        assert!(matches!(
            parse_math_ast("a \\land b").unwrap(),
            MathNode::Row(nodes) if nodes.iter().any(|node| matches!(node, MathNode::Symbol { class: AtomClass::Bin, .. }))
        ));
    }

    fn contains_node(node: &MathNode, predicate: impl Fn(&MathNode) -> bool + Copy) -> bool {
        if predicate(node) {
            return true;
        }
        match node {
            MathNode::Frac { num, den } => contains_node(num, predicate) || contains_node(den, predicate),
            MathNode::Binom { n, k } => contains_node(n, predicate) || contains_node(k, predicate),
            MathNode::Sqrt { degree, arg } => degree.as_deref().is_some_and(|d| contains_node(d, predicate)) || contains_node(arg, predicate),
            MathNode::Accent { arg, .. } => contains_node(arg, predicate),
            MathNode::Scripts { base, sup, sub } => {
                contains_node(base, predicate)
                    || sup.as_deref().is_some_and(|s| contains_node(s, predicate))
                    || sub.as_deref().is_some_and(|s| contains_node(s, predicate))
            }
            MathNode::Row(nodes) => nodes.iter().any(|child| contains_node(child, predicate)),
            MathNode::Text(_) | MathNode::Symbol { .. } | MathNode::Space(_) => false,
        }
    }

    #[test]
    fn no_stray_braces_in_output() {
        // Ensure that braces used as grouping delimiters don't leak into the text output
        let cases = ["\\frac{a+b}{c-d}", "\\binom{n}{k}", "\\sqrt{x+y}", "x^{2n}", "y_{ij}", "{a+b}"];
        for input in cases {
            let text = format_latex_math(input).unwrap_or_default();
            assert!(!text.contains('{'), "braces leaked in '{input}': got '{text}'");
            assert!(!text.contains('}'), "braces leaked in '{input}': got '{text}'");
        }
    }

    #[test]
    fn accents_and_left_right_delimiters_keep_their_structure() {
        let vector = parse_math_ast("\\vec{x}").unwrap();
        assert!(matches!(vector, MathNode::Accent { kind: MathAccent::Vector, .. }));
        assert!(matches!(parse_math_ast("\\overline{AB}").unwrap(), MathNode::Accent { kind: MathAccent::Overline, .. }));
        assert!(matches!(parse_math_ast("\\underline{x}").unwrap(), MathNode::Accent { kind: MathAccent::Underline, .. }));
        let delimited = parse_math_ast("\\left(x\\right)").unwrap();
        assert_eq!(node_to_text(&delimited), "(x)");
    }

    #[test]
    fn inline_math_stays_on_the_text_line() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut response_rect = None;
        let mut output = ctx
            .run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(500.0, 100.0))), ..Default::default() }, |ui| {
                response_rect = Some(render_math_text(ui, "before $x$ after", crate::theme::regular(12.5), Color32::BLACK).rect)
            });
        output.textures_delta.clear();
        assert!(response_rect.is_some_and(|rect| rect.height() < 30.0), "inline math should share a line with surrounding text");
    }

    #[test]
    fn rendered_math_exposes_its_plain_text_to_accessibility() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.set_fonts(crate::theme::font_definitions());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            render_math_text(ui, "KB $\\models \\alpha$", crate::theme::regular(12.5), Color32::BLACK);
        });
        let update = output.platform_output.accesskit_update.take().expect("math label should be exposed to accessibility");
        assert!(update.nodes.iter().any(|(_, node)| { node.role() == egui::accesskit::Role::Label && node.value() == Some("KB ⊨ α") }));
        output.textures_delta.clear();
    }
}
