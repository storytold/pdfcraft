use super::format::{js_number, make_number};
use super::script::{MAX_NESTING, Parser, js_str};
use super::*;

// ── renaming fields in calculations ─────────────────────────────────────────────────────────

/// A JavaScript token, as far as finding the field names a calculation reads needs.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Js {
    Ident,
    /// A string literal, its escapes undone.
    Str(String),
    Punct(char),
    /// A number, a regular expression or a template literal.
    Other,
}

/// A [`Js`] token with its byte range in the script.
type Token = (Range<usize>, Js);

/// Words after which `/` starts a regular expression instead of dividing.
const BEFORE_REGEX: [&str; 13] = ["return", "typeof", "case", "do", "else", "in", "of", "new", "delete", "void", "throw", "instanceof", "yield"];

/// Words whose condition, in parentheses, comes before a statement, so a `/` after it starts a
/// regular expression. The `while` that ends a `do` loop counts too: the loop ends at its `)`, so
/// a `/` after it starts the next statement, and can't divide.
const BEFORE_CONDITION: [&str; 4] = ["if", "while", "for", "with"];

/// Whether a line ends at `s[j]`: JavaScript ends one at `\n`, `\r`, U+2028 and U+2029.
fn line_ends_at(s: &[u8], j: usize) -> bool {
    matches!(s.get(j..), Some([b'\n' | b'\r', ..] | [0xE2, 0x80, 0xA8 | 0xA9, ..]))
}

/// JavaScript's tokens with their byte ranges, and apart from them the text of each comment
/// (inside its `//`, or its `/*` and `*/`): enough of the language to tell code from strings,
/// comments and regular expressions. The tokens stop where a string, comment or template literal
/// never ends; a comment that never ends runs to the end of the script.
pub(super) fn js_tokens(js: &str) -> (Vec<Token>, Vec<Range<usize>>) {
    let s = js.as_bytes();
    // Whether `s[j]` is part of a word. Bytes past ASCII are parts of identifiers, except
    // U+2028 and U+2029, which end a line: a range never ends inside a character.
    let word = |j: usize| s.get(j).is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || !b.is_ascii()) && !line_ends_at(s, j);
    let mut out: Vec<Token> = Vec::new();
    let mut comments = Vec::new();
    let mut i = 0;
    // Where a regular expression that never ended stopped being read: every `/` before it
    // divides, so a line of them is read once, not once for each.
    let mut divides_until = 0;
    // For each `(` still open, whether it opened the condition of `if`, `while`, `for` or
    // `with`; and whether the last token is the `)` that closed one.
    let mut conditions: Vec<bool> = Vec::new();
    let mut after_condition = false;
    'scan: while let Some(&c) = s.get(i) {
        let start = i;
        let token = match c {
            _ if c.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            // U+2028 or U+2029 between tokens ends a line, and is skipped whole: read a byte at a
            // time, the rest of it would be read from inside the character.
            0xE2 if line_ends_at(s, i) => {
                i += 3;
                continue;
            }
            b'/' if s.get(i + 1) == Some(&b'/') => {
                i = (i + 2..s.len()).find(|&j| line_ends_at(s, j)).unwrap_or(s.len());
                comments.push(start + 2..i);
                continue;
            }
            b'/' if s.get(i + 1) == Some(&b'*') => match js.get(i + 2..).and_then(|rest| rest.find("*/")) {
                Some(n) => {
                    comments.push(i + 2..i + 2 + n);
                    i += n + 4;
                    continue;
                }
                None => {
                    comments.push(i + 2..s.len());
                    break 'scan;
                }
            },
            b'"' | b'\'' => {
                let mut p = Parser { s, i, depth: 0 };
                let Some(text) = p.string() else { break 'scan };
                i = p.i;
                Js::Str(text)
            }
            b'`' => {
                let mut j = i + 1;
                loop {
                    match s.get(j) {
                        None => break 'scan,
                        Some(b'\\') => j += 2,
                        Some(b'`') => break,
                        Some(_) => j += 1,
                    }
                }
                i = j + 1;
                Js::Other
            }
            b'/' if i >= divides_until && regex_may_follow(js, &out, after_condition) => {
                // A regular expression ends at a `/` outside a class; a line break first means it
                // was a division after all.
                let (mut j, mut class) = (i + 1, false);
                let end = loop {
                    match s.get(j) {
                        None => break None,
                        Some(_) if line_ends_at(s, j) => break None,
                        Some(b'\\') => j += 2,
                        Some(b'/') if !class => break Some(j),
                        Some(b) => {
                            class = match b {
                                b'[' => true,
                                b']' => false,
                                _ => class,
                            };
                            j += 1;
                        }
                    }
                };
                match end {
                    Some(e) => {
                        i = e + 1;
                        while word(i) {
                            i += 1;
                        }
                        Js::Other
                    }
                    None => {
                        divides_until = j;
                        i += 1;
                        Js::Punct('/')
                    }
                }
            }
            _ if c.is_ascii_digit() => {
                while word(i) || s.get(i) == Some(&b'.') {
                    i += 1;
                }
                Js::Other
            }
            _ if word(i) => {
                while word(i) {
                    i += 1;
                }
                Js::Ident
            }
            _ => {
                i += 1;
                Js::Punct(char::from(c))
            }
        };
        after_condition = match token {
            Js::Punct('(') => {
                conditions.push(opens_condition(js, &out));
                false
            }
            Js::Punct(')') => conditions.pop().unwrap_or(false),
            _ => false,
        };
        out.push((start..i, token));
    }
    (out, comments)
}

/// Whether a `/` after `tokens` starts a regular expression: after an operator, an opening
/// bracket, a keyword such as `return` or the condition of `if`, `while`, `for` or `with`, not
/// after a value. `after_condition` says the last token is the `)` that ends such a condition.
fn regex_may_follow(js: &str, tokens: &[Token], after_condition: bool) -> bool {
    match tokens.last() {
        None => true,
        Some((range, Js::Ident)) => js.get(range.clone()).is_some_and(|w| BEFORE_REGEX.contains(&w)),
        Some((_, Js::Punct(')'))) => after_condition,
        Some((_, Js::Punct(c))) => !matches!(c, ']' | '}'),
        Some((_, Js::Str(_) | Js::Other)) => false,
    }
}

/// Whether a `(` after `tokens` opens the condition of `if`, `while`, `for` or `with`, and not
/// the arguments of a method that has one of those names (`o.if(x)`).
fn opens_condition(js: &str, tokens: &[Token]) -> bool {
    let mut back = tokens.iter().rev();
    let Some((range, Js::Ident)) = back.next() else { return false };
    js.get(range.clone()).is_some_and(|w| BEFORE_CONDITION.contains(&w)) && !matches!(back.next(), Some((_, Js::Punct('.'))))
}

/// Where a Calculate script keeps its simplified field notation: between `BVCALC` and the
/// `EVCALC` after it, in the first of `comments` that holds both, as Acrobat writes it
/// (`/** BVCALC a + b EVCALC **/`). A string or code that spells the markers is not the notation.
pub(super) fn notation_range(js: &str, comments: &[Range<usize>]) -> Option<Range<usize>> {
    comments.iter().find_map(|comment| {
        let text = js.get(comment.clone())?;
        let a = text.find("BVCALC")? + 6;
        let b = a + text.get(a..)?.find("EVCALC")?;
        Some(comment.start + a..comment.start + b)
    })
}

/// A place where a Calculate script names a field.
#[derive(Clone, Debug, PartialEq)]
enum Site {
    /// A string literal holding one name: `getField("a")`, or an item of an
    /// `AFSimple_Calculate` list.
    Literal(String),
    /// An `AFSimple_Calculate` list written as one string of comma-separated names.
    Commas(String),
    /// A name in the simplified field notation of a `BVCALC` comment.
    Notation(String),
    /// A field read by a name built at run time: a `getField` whose argument is more than one
    /// string literal (`getField("row" + i)`), or an `AFSimple_Calculate` list that isn't
    /// written out. Which field it is isn't known.
    Built,
}

impl Site {
    fn names(&self) -> Vec<&str> {
        match self {
            Site::Literal(name) | Site::Notation(name) => vec![name.as_str()],
            Site::Commas(list) => list.split(',').map(str::trim).filter(|n| !n.is_empty()).collect(),
            Site::Built => Vec::new(),
        }
    }

    /// The site written again with field `old` named `new`, or `None` when it doesn't name `old`.
    fn renamed(&self, old: &str, new: &str) -> Option<String> {
        match self {
            Site::Literal(name) => (name == old).then(|| js_str(new)),
            Site::Notation(name) => (name == old).then(|| notation_name(new)),
            Site::Commas(list) => {
                let parts: Vec<&str> = list.split(',').collect();
                parts.iter().any(|p| p.trim() == old).then(|| {
                    // The spaces around each name stay as they were.
                    let parts: Vec<String> = parts.iter().map(|p| if p.trim() == old { p.replacen(old, new, 1) } else { (*p).to_owned() }).collect();
                    js_str(&parts.join(","))
                })
            }
            Site::Built => None,
        }
    }
}

/// Where a Calculate script names fields, in order: the simplified field notation in its comment
/// (see [`notation_range`]; [`parse_calculate`] reads it there too), the field list of
/// `AFSimple_Calculate`, and every `getField` whose whole argument is one string literal. A name
/// built at run time (`getField("row" + i)`) is a [`Site::Built`], at its `getField` or
/// `AFSimple_Calculate`.
fn calculation_sites(js: &str) -> Vec<(Range<usize>, Site)> {
    let mut sites = Vec::new();
    let (tokens, comments) = js_tokens(js);
    if let Some(at) = notation_range(js, &comments)
        && let Some(expr) = js.get(at.clone())
    {
        for (range, token) in notation_tokens(expr) {
            if let Notation::Name(name) = token {
                sites.push((at.start + range.start..at.start + range.end, Site::Notation(name)));
            }
        }
    }
    let ident = |k: usize, word: &str| tokens.get(k).is_some_and(|(range, t)| *t == Js::Ident && js.get(range.clone()) == Some(word));
    let punct = |k: usize, c: char| tokens.get(k).is_some_and(|(_, t)| *t == Js::Punct(c));
    let string = |k: usize| match tokens.get(k) {
        Some((range, Js::Str(text))) => Some((range.clone(), text.clone())),
        _ => None,
    };
    let built = |k: usize| tokens.get(k).map(|(range, _)| (range.clone(), Site::Built));
    for k in 0..tokens.len() {
        if ident(k, "getField") && punct(k + 1, '(') {
            match string(k + 2) {
                Some((range, name)) if punct(k + 3, ')') => sites.push((range, Site::Literal(name))),
                _ => sites.extend(built(k)),
            }
        }
        if !(ident(k, "AFSimple_Calculate") && punct(k + 1, '(')) {
            continue;
        }
        if !(string(k + 2).is_some() && punct(k + 3, ',')) {
            sites.extend(built(k));
            continue;
        }
        // One string of comma-separated names, when it is the whole list (not `"row" + suffix`).
        // An argument after it leaves the list as written, as after an array: its names are read,
        // and what follows is a name built at run time.
        if let Some((range, list)) = string(k + 4) {
            if punct(k + 5, ')') || punct(k + 5, ',') {
                sites.push((range, Site::Commas(list)));
            }
            if !punct(k + 5, ')') {
                sites.extend(built(k));
            }
            continue;
        }
        // `new Array("a", "b")` or `["a", "b"]`: each item that is one string, and the list has
        // to end the call. An item that is more than a string (`"row" + i`), or anything after
        // the list (`.concat(more)`), is a name built at run time. Nothing else is read, so no
        // token is read for two calls.
        let (first, close) = if punct(k + 4, '[') {
            (k + 5, ']')
        } else if ident(k + 4, "new") && ident(k + 5, "Array") && punct(k + 6, '(') {
            (k + 7, ')')
        } else {
            sites.extend(built(k));
            continue;
        };
        let mut j = first;
        loop {
            match tokens.get(j) {
                // An item ends at a comma or the list's end, or where a script cut short ends.
                Some((range, Js::Str(name))) if punct(j + 1, ',') || punct(j + 1, close) || tokens.get(j + 1).is_none() => {
                    sites.push((range.clone(), Site::Literal(name.clone())));
                    j += 1;
                }
                Some((_, Js::Punct(','))) => j += 1,
                Some((_, Js::Punct(c))) if *c == close => {
                    if !punct(j + 1, ')') {
                        sites.extend(built(k));
                    }
                    break;
                }
                _ => {
                    sites.extend(built(k));
                    break;
                }
            }
        }
    }
    sites.sort_by_key(|(range, _)| range.start);
    sites
}

/// A Calculate script with field `old` named `new` everywhere it reads it (see
/// [`calculation_sites`]), the rest kept as written; `None` when it doesn't read `old`. Also
/// `None` when the new name would read as more than a name there (one holding `EVCALC` would end
/// the notation early), so a script is never left meaning something else.
pub fn rename_in_calculation(js: &str, old: &str, new: &str) -> Option<String> {
    let sites = calculation_sites(js);
    let edits: Vec<(Range<usize>, String)> = sites.iter().filter_map(|(range, site)| Some((range.clone(), site.renamed(old, new)?))).collect();
    if edits.is_empty() {
        return None;
    }
    let out = splice(js, edits);
    let expected: Vec<&str> = sites.iter().flat_map(|(_, site)| site.names()).map(|n| if n == old { new } else { n }).collect();
    let written = calculation_sites(&out);
    let got: Vec<&str> = written.iter().flat_map(|(_, site)| site.names()).collect();
    (got == expected).then_some(out)
}

/// The fields a Calculate script reads by name (see [`calculation_sites`]), each once, in the
/// order it first names them.
pub fn calculation_reads(js: &str) -> Vec<String> {
    let sites = calculation_sites(js);
    unique(sites.iter().flat_map(|(_, site)| site.names()).map(str::to_owned))
}

/// Whether a Calculate script also reads fields by names it builds at run time
/// (`getField("row" + i)`), which [`calculation_reads`] can't list.
pub fn reads_built_names(js: &str) -> bool {
    calculation_sites(js).iter().any(|(_, site)| *site == Site::Built)
}

impl Calculate {
    /// The fields it reads, each once, in the order it first names them. A name also stands for
    /// the fields under it (`row` for `row.0` and `row.1`), as [`calculate`] reads them.
    pub fn reads(&self) -> Vec<String> {
        match self {
            Calculate::None => Vec::new(),
            Calculate::Simple { fields, .. } => unique(fields.iter().cloned()),
            Calculate::Notation(expr) => unique(notation_tokens(expr).into_iter().filter_map(|(_, token)| match token {
                Notation::Name(name) => Some(name),
                _ => None,
            })),
        }
    }
}

/// `names` without repeats, in order.
pub(super) fn unique(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names.into_iter().filter(|n| seen.insert(n.clone())).collect()
}

/// A token of simplified field notation.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Notation {
    /// A number, as written.
    Number,
    /// `+ - * / ( )`.
    Op(char),
    /// A field name, its `\` escapes undone.
    Name(String),
}

/// Simplified field notation's tokens, each with its byte range in `src`. Whitespace separates
/// them, and `\` escapes a space or an operator inside a name, as Acrobat writes them.
pub(super) fn notation_tokens(src: &str) -> Vec<(Range<usize>, Notation)> {
    let breaks = |c: char| c.is_whitespace() || "+-*/()".contains(c);
    let mut out = Vec::new();
    let mut cs = src.char_indices().peekable();
    while let Some(&(start, c)) = cs.peek() {
        let mut end = start + c.len_utf8();
        if c.is_whitespace() {
            cs.next();
        } else if breaks(c) {
            cs.next();
            out.push((start..end, Notation::Op(c)));
        } else if c.is_ascii_digit() || c == '.' {
            while let Some((i, d)) = cs.next_if(|(_, d)| d.is_ascii_digit() || *d == '.') {
                end = i + d.len_utf8();
            }
            out.push((start..end, Notation::Number));
        } else {
            let mut name = String::new();
            while let Some((i, n)) = cs.next_if(|(_, n)| !breaks(*n)) {
                end = i + n.len_utf8();
                if n == '\\'
                    && let Some((j, next)) = cs.next()
                {
                    end = j + next.len_utf8();
                    name.push(next);
                } else {
                    name.push(n);
                }
            }
            out.push((start..end, Notation::Name(name)));
        }
    }
    out
}

/// A field name as simplified field notation writes it: `\` before a space, an operator or a
/// `\`, and before a first character that would start a number.
pub(super) fn notation_name(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_whitespace() || "+-*/()\\".contains(c) || (i == 0 && (c.is_ascii_digit() || c == '.')) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `src` with each range replaced. The ranges come in order and don't overlap; one that does
/// is left out rather than written over another.
fn splice(src: &str, edits: impl IntoIterator<Item = (Range<usize>, String)>) -> String {
    let mut out = String::with_capacity(src.len());
    let mut done = 0;
    for (range, with) in edits {
        let Some(kept) = src.get(done..range.start).filter(|_| range.start <= range.end) else { continue };
        out.push_str(kept);
        out.push_str(&with);
        done = range.end;
    }
    out.push_str(src.get(done..).unwrap_or_default());
    out
}

/// Simplified field notation as the JavaScript Acrobat writes after its `BVCALC` comment:
/// each field name becomes `AFMakeNumber(this.getField("name").value)`, the rest stays.
pub fn notation_js(expr: &str) -> String {
    let names = notation_tokens(expr).into_iter().filter_map(|(range, t)| match t {
        Notation::Name(name) => Some((range, format!("AFMakeNumber(this.getField({}).value)", js_str(&name)))),
        _ => None,
    });
    splice(expr, names)
}

/// Evaluate simplified field notation: + - * / and parentheses over numbers and field names.
pub(super) fn eval_notation(src: &str, value_of: &dyn Fn(&str) -> f64) -> Option<f64> {
    #[derive(Debug, Clone, PartialEq)]
    enum T {
        N(f64),
        Op(char),
    }
    let toks = notation_tokens(src)
        .into_iter()
        .map(|(range, t)| match t {
            Notation::Number => src.get(range)?.parse().ok().map(T::N),
            Notation::Op(c) => Some(T::Op(c)),
            Notation::Name(name) => Some(T::N(value_of(&name))),
        })
        .collect::<Option<Vec<T>>>()?;
    // Recursive descent, `d` levels deep (at most MAX_NESTING).
    fn expr(t: &[T], i: &mut usize, d: usize) -> Option<f64> {
        let mut v = term(t, i, d)?;
        while let Some(T::Op(o @ ('+' | '-'))) = t.get(*i) {
            *i += 1;
            let r = term(t, i, d)?;
            v = if *o == '+' { v + r } else { v - r };
        }
        Some(v)
    }
    fn term(t: &[T], i: &mut usize, d: usize) -> Option<f64> {
        let mut v = factor(t, i, d)?;
        while let Some(T::Op(o @ ('*' | '/'))) = t.get(*i) {
            *i += 1;
            let r = factor(t, i, d)?;
            v = if *o == '*' { v * r } else { v / r };
        }
        Some(v)
    }
    fn factor(t: &[T], i: &mut usize, d: usize) -> Option<f64> {
        if d > MAX_NESTING {
            return None;
        }
        match t.get(*i)? {
            T::N(n) => {
                *i += 1;
                Some(*n)
            }
            T::Op('-') => {
                *i += 1;
                factor(t, i, d + 1).map(|v| -v)
            }
            T::Op('(') => {
                *i += 1;
                let v = expr(t, i, d + 1)?;
                (t.get(*i) == Some(&T::Op(')'))).then(|| *i += 1)?;
                Some(v)
            }
            _ => None,
        }
    }
    let mut i = 0;
    let v = expr(&toks, &mut i, 0)?;
    (i == toks.len() && v.is_finite()).then_some(v)
}

/// The Calculate event: the new value, given the current values of the other fields
/// (`values(name)` returns every field named `name` or below it, for groups).
pub fn calculate(c: &Calculate, values: &dyn Fn(&str) -> Vec<String>) -> Option<String> {
    let num = |s: &String| make_number(s).unwrap_or(0.0);
    match c {
        Calculate::None => None,
        Calculate::Simple { op, fields } => {
            let xs: Vec<f64> = fields.iter().flat_map(|f| values(f)).map(|v| num(&v)).collect();
            let v = match op {
                CalcOp::Sum => xs.iter().sum(),
                CalcOp::Product => xs.iter().product(),
                CalcOp::Average => {
                    if xs.is_empty() {
                        0.0
                    } else {
                        xs.iter().sum::<f64>() / xs.len() as f64
                    }
                }
                CalcOp::Minimum => xs.iter().copied().fold(f64::INFINITY, f64::min),
                CalcOp::Maximum => xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            };
            Some(if v.is_finite() { js_number(v) } else { "0".into() })
        }
        Calculate::Notation(expr) => {
            let v = eval_notation(expr, &|name| values(name).first().map(num).unwrap_or(0.0))?;
            Some(js_number(v))
        }
    }
}
