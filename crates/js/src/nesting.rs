//! The nesting guard: whether a script nests deeper than boa's parser can take.
//!
//! boa parses by recursive descent with no depth limit of its own, and a stack overflow aborts
//! the process (it is not a panic, so `catch_unwind` can't stop it). So every script is scanned
//! before boa sees it, and one that nests too deeply is refused.
//!
//! A scan has to know which brackets are code, and that depends on where strings, templates,
//! comments and regular expressions begin and end. Most of that follows from the bytes, but a
//! `/` that isn't a comment is a division or the start of a regular expression depending on
//! where the parser is, which a scan can't know. A scan that guesses can be put out of phase:
//! after `/'/`, the quote in the regular expression made the real nesting that followed look
//! like string text (#5079). So the scan doesn't guess. It follows every reading at once,
//! splitting at each such `/` into a division and a regular expression, and counts nesting on
//! each reading. boa's reading is always one of them, so the deepest is at least as deep as
//! boa's. A reading boa would reject is dropped at the byte where boa stops with a syntax error
//! (a string or regular expression running into a line break, a regular expression's groups not
//! matching), which keeps divisions from leaving stray readings behind.

/// Deepest nesting of `(`, `[`, `{` and template substitutions (`${`).
pub(crate) const MAX_BRACKET_DEPTH: usize = 64;

/// Longest run of prefix operators (`!`, `~`, `+`, `-`).
pub(crate) const MAX_PREFIX_RUN: usize = 64;

/// Most `?` (conditional expressions nest without brackets).
pub(crate) const MAX_CONDITIONALS: usize = 4096;

/// Most readings followed at once. Readings that differ only in their counts are merged first,
/// keeping the larger counts; past this, the script is refused.
const MAX_READINGS: usize = 64;

/// Where a reading is in the script.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Mode {
    Code,
    /// In a string that this quote closes.
    Str(u8),
    /// In a template's text: after its backtick, or after a substitution's closing `}`.
    Template,
    /// In a regular expression literal: in a class (`[...]`), and the groups open outside one.
    Regex {
        class: bool,
        groups: usize,
    },
    /// In a `//`, `<!--` or `-->` comment, to the end of the line.
    LineComment,
    /// In a `/* */` comment.
    BlockComment,
}

/// One way of reading the script, and what it has counted so far.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Reading {
    mode: Mode,
    /// Bytes to pass over: the rest of a two-byte token, or what a backslash escapes.
    skip: usize,
    /// For each open template substitution, the braces open inside it, so the `}` that closes
    /// the substitution is told from one that closes a block.
    substitutions: Vec<usize>,
    depth: usize,
    brackets: [u8; MAX_BRACKET_DEPTH + 2],
    regex_only: bool,
    prefix: usize,
    conditionals: usize,
    conditional_levels: [usize; MAX_BRACKET_DEPTH + 2],
}

impl Reading {
    fn in_code() -> Self {
        Self {
            mode: Mode::Code,
            skip: 0,
            substitutions: Vec::new(),
            depth: 0,
            brackets: [0; MAX_BRACKET_DEPTH + 2],
            regex_only: true,
            prefix: 0,
            conditionals: 0,
            conditional_levels: [0; MAX_BRACKET_DEPTH + 2],
        }
    }

    fn with_mode(&self, mode: Mode) -> Self {
        Self { mode, ..self.clone() }
    }

    fn over_limit(&self) -> bool {
        self.depth > MAX_BRACKET_DEPTH || self.prefix > MAX_PREFIX_RUN || self.conditionals > MAX_CONDITIONALS
    }

    fn open(&mut self, kind: u8) {
        self.regex_only = true;
        self.depth = self.depth.saturating_add(1);
        if let Some(slot) = self.brackets.get_mut(self.depth) {
            *slot = kind;
        }
        self.prefix = 0;
    }

    fn close(&mut self) {
        self.regex_only = false;
        if let Some(slot) = self.brackets.get_mut(self.depth) {
            *slot = 0;
        }
        if let Some(count) = self.conditional_levels.get_mut(self.depth) {
            self.conditionals = self.conditionals.saturating_sub(*count);
            *count = 0;
        }
        self.depth = self.depth.saturating_sub(1);
        self.prefix = 0;
    }

    /// Read byte `i` of `b`. A second reading that splits off here goes to `forks`. Returns
    /// false when boa would stop with a syntax error at this byte, ending this reading.
    fn advance(&mut self, b: &[u8], i: usize, forks: &mut Vec<Reading>) -> bool {
        if self.skip > 0 {
            self.skip -= 1;
            return true;
        }
        let Some(&c) = b.get(i) else { return true };
        match self.mode {
            Mode::Code => {
                // Outside a literal JavaScript only permits a backslash in a Unicode
                // identifier escape. Prune an impossible division reading at `\(`, etc.;
                // otherwise escaped regex punctuation falsely accumulates code brackets.
                if c == b'\\' && !at(b, i + 1, b"u") {
                    return false;
                }
                let expected = match c {
                    b')' => Some(b'('),
                    b']' => Some(b'['),
                    b'}' => Some(b'{'),
                    _ => None,
                };
                if let Some(expected) = expected
                    && (self.depth == 0 || self.brackets.get(self.depth).copied() != Some(expected))
                {
                    return false;
                }
                self.code(c, b, i, forks);
                true
            }
            Mode::Str(quote) => self.string(quote, c, b, i),
            Mode::Template => {
                self.template(c, b, i);
                true
            }
            Mode::Regex { class, groups } => self.regex(class, groups, c, b, i),
            Mode::LineComment => {
                let n = line_break(b, i);
                if n > 0 {
                    self.mode = Mode::Code;
                    self.skip = n - 1;
                }
                true
            }
            Mode::BlockComment => {
                if c == b'*' && at(b, i + 1, b"/") {
                    self.mode = Mode::Code;
                    self.skip = 1;
                }
                true
            }
        }
    }

    fn code(&mut self, c: u8, b: &[u8], i: usize, forks: &mut Vec<Reading>) {
        match c {
            b'\'' | b'"' => {
                self.regex_only = false;
                self.mode = Mode::Str(c);
                self.prefix = 0;
            }
            b'`' => {
                self.regex_only = false;
                self.mode = Mode::Template;
                self.prefix = 0;
            }
            b'/' if at(b, i + 1, b"/") => self.mode = Mode::LineComment,
            b'/' if at(b, i + 1, b"*") => {
                self.mode = Mode::BlockComment;
                self.skip = 1;
            }
            b'/' => {
                // A division, or the start of a regular expression: the parser decides, so
                // follow both.
                // At these expression starts a slash cannot be division. Unlike a scan of
                // preceding bytes, this state survives comments without reading their text
                // as punctuation. Ambiguous positions still follow both alternatives.
                if self.regex_only {
                    self.mode = Mode::Regex { class: false, groups: 0 };
                } else {
                    forks.push(self.with_mode(Mode::Regex { class: false, groups: 0 }));
                }
                self.prefix = 0;
            }
            b'(' | b'[' => self.open(c),
            b'{' => {
                if let Some(open) = self.substitutions.last_mut() {
                    *open = open.saturating_add(1);
                }
                self.open(c);
            }
            b')' | b']' => self.close(),
            b'}' => {
                self.close();
                if self.substitutions.last() == Some(&0) {
                    self.substitutions.pop();
                    self.mode = Mode::Template;
                } else if let Some(open) = self.substitutions.last_mut() {
                    *open = open.saturating_sub(1);
                }
            }
            b'!' | b'~' | b'+' | b'-' => {
                self.regex_only = matches!(c, b'!' | b'~');
                // `-->` begins a comment at the start of a line (Annex B); followed anywhere.
                if c == b'-' && at(b, i + 1, b"->") {
                    forks.push(self.with_mode(Mode::LineComment));
                }
                self.prefix = self.prefix.saturating_add(1);
            }
            b'?' => {
                self.regex_only = true;
                self.conditionals = self.conditionals.saturating_add(1);
                if let Some(count) = self.conditional_levels.get_mut(self.depth) {
                    *count = count.saturating_add(1);
                }
                self.prefix = 0;
            }
            // A semicolon cannot occur inside an unfinished conditional at this depth.
            // Keep outer levels: a function body's statements cannot erase its caller's chain.
            b';' => {
                self.regex_only = true;
                if let Some(count) = self.conditional_levels.get_mut(self.depth) {
                    self.conditionals = self.conditionals.saturating_sub(*count);
                    *count = 0;
                }
                self.prefix = 0;
            }
            b'=' | b':' | b',' => {
                self.regex_only = true;
                self.prefix = 0;
            }
            // `<!--` begins a comment (Annex B, in scripts); followed both ways.
            b'<' if at(b, i + 1, b"!--") => {
                forks.push(self.with_mode(Mode::LineComment));
                self.prefix = 0;
            }
            // Whitespace and line breaks, and any non-ASCII byte, which may be part of either,
            // leave a run of prefix operators going.
            _ if c.is_ascii_whitespace() || !c.is_ascii() => {}
            _ => {
                self.regex_only = false;
                self.prefix = 0;
            }
        }
    }

    /// In a string closed by `quote`. False at a line break boa rejects.
    fn string(&mut self, quote: u8, c: u8, b: &[u8], i: usize) -> bool {
        if c == quote {
            self.mode = Mode::Code;
            self.prefix = 0;
        } else if c == b'\\' {
            // An escape, or a line continuation (`\` and a line break, CR LF included).
            self.skip = line_break(b, i + 1).max(1);
        } else if c == b'\n' || c == b'\r' {
            // U+2028 and U+2029 may stand in a string; LF and CR may not.
            return false;
        }
        true
    }

    fn template(&mut self, c: u8, b: &[u8], i: usize) {
        if c == b'`' {
            self.mode = Mode::Code;
            self.prefix = 0;
        } else if c == b'\\' {
            self.skip = 1;
        } else if c == b'$' && at(b, i + 1, b"{") {
            self.substitutions.push(0);
            self.open(b'{');
            self.mode = Mode::Code;
            self.skip = 1;
        }
    }

    /// In a regular expression literal. False where boa rejects it: a line break, or groups
    /// that don't match (boa checks the pattern as it reads the literal).
    fn regex(&mut self, class: bool, groups: usize, c: u8, b: &[u8], i: usize) -> bool {
        if line_break(b, i) > 0 || (c == b'\\' && line_break(b, i + 1) > 0) {
            return false;
        }
        match (c, class) {
            (b'\\', _) => self.skip = 1,
            (b']', true) => self.mode = Mode::Regex { class: false, groups },
            (b'[', false) => self.mode = Mode::Regex { class: true, groups },
            (b'(', false) => self.mode = Mode::Regex { class, groups: groups.saturating_add(1) },
            (b')', false) if groups == 0 => return false,
            (b')', false) => self.mode = Mode::Regex { class, groups: groups - 1 },
            (b'/', false) if groups > 0 => return false,
            (b'/', false) => {
                self.mode = Mode::Code;
                self.regex_only = false;
                self.prefix = 0;
            }
            _ => {}
        }
        true
    }
}

/// Whether `s` stands in `b` at `i`.
fn at(b: &[u8], i: usize, s: &[u8]) -> bool {
    b.get(i..).is_some_and(|rest| rest.starts_with(s))
}

/// How many bytes the line break at `i` takes: CR LF, LF, CR, U+2028 or U+2029. 0 if none.
fn line_break(b: &[u8], i: usize) -> usize {
    let rest = b.get(i..).unwrap_or_default();
    if rest.starts_with(b"\r\n") {
        2
    } else if rest.starts_with(b"\n") || rest.starts_with(b"\r") {
        1
    } else if rest.starts_with("\u{2028}".as_bytes()) || rest.starts_with("\u{2029}".as_bytes()) {
        3
    } else {
        0
    }
}

/// Whether any reading of `source` nests brackets deeper than [`MAX_BRACKET_DEPTH`], runs more
/// than [`MAX_PREFIX_RUN`] prefix operators together, or has more than [`MAX_CONDITIONALS`]
/// conditionals; or the script has more ambiguous readings than the scan follows.
pub(crate) fn too_deep(source: &str) -> bool {
    refusal(source).is_some()
}
pub(crate) fn refusal(source: &str) -> Option<(usize, &'static str)> {
    let b = source.as_bytes();
    // A leading `#!` is a hashbang comment: boa parses scripts and eval code from a goal that
    // allows one. Read as code it could only be a syntax error (`#` must start a private name).
    let first = if b.starts_with(b"#!") { Mode::LineComment } else { Mode::Code };
    let mut readings = vec![Reading::in_code().with_mode(first)];
    let mut next = Vec::with_capacity(MAX_READINGS + 1);
    for i in 0..b.len() {
        let mut ended: Option<Reading> = None;
        for mut r in readings.drain(..) {
            if r.advance(b, i, &mut next) {
                next.push(r);
            } else {
                ended = Some(match ended {
                    Some(e) => merged(e, &r),
                    None => r,
                });
            }
        }
        if next.is_empty() {
            // Every reading ends in a syntax error here, so boa stops by this byte. Carry on as
            // code anyway, with the largest counts, rather than stop checking.
            let ended = ended.unwrap_or_else(Reading::in_code);
            next.push(Reading { mode: Mode::Code, skip: 0, ..ended });
        }
        if let Some(reading) = next.iter().find(|r| r.over_limit()) {
            return Some((
                i,
                if reading.depth > MAX_BRACKET_DEPTH {
                    "brackets"
                } else if reading.prefix > MAX_PREFIX_RUN {
                    "prefix operators"
                } else {
                    "conditional expressions"
                },
            ));
        }
        if next.len() > 1 && !settle(&mut next) {
            return Some((i, "ambiguous regular expressions"));
        }
        std::mem::swap(&mut readings, &mut next);
    }
    None
}

/// `a` with the larger of each count in `a` and `b`.
fn merged(a: Reading, b: &Reading) -> Reading {
    let conditional_levels =
        std::array::from_fn(|i| a.conditional_levels.get(i).copied().unwrap_or(0).max(b.conditional_levels.get(i).copied().unwrap_or(0)));
    let conditionals = conditional_levels.iter().copied().fold(0usize, usize::saturating_add);
    Reading {
        depth: a.depth.max(b.depth),
        prefix: a.prefix.max(b.prefix),
        conditionals,
        conditional_levels,
        regex_only: a.regex_only && b.regex_only,
        ..a
    }
}

/// Drop duplicate readings, then merge those in the same place if there are too many. False if
/// there are still too many to follow.
fn settle(readings: &mut Vec<Reading>) -> bool {
    readings.sort_unstable();
    readings.dedup();
    if readings.len() > MAX_READINGS {
        // Sorted, readings in the same place are next to each other. Merging keeps the larger
        // counts, so it can only refuse more.
        readings.dedup_by(|r, kept| {
            let same = r.mode == kept.mode && r.skip == kept.skip && r.substitutions == kept.substitutions && r.brackets == kept.brackets;
            if same {
                *kept = merged(kept.clone(), r);
            }
            same
        });
    }
    readings.len() <= MAX_READINGS
}
