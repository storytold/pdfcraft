//! The model's predicates (`fn:…`), read and evaluated.
//!
//! The grammar is `INTERNAL_GRAMMAR.md` of the Arlington model: predicates nest, take up to two
//! arguments, and combine with `&&`, `||`, comparisons and a little arithmetic. `@Key` is the
//! value of a key, a bare `Key` that it is present, and `parent::` and `trailer::` start paths
//! elsewhere in the document.
//!
//! Evaluation is three-valued. A predicate this validator does not implement, a key that is
//! missing, or a value of the wrong kind gives *unknown* (`None`), never a guess; the caller then
//! reports the rule as not checked. Unimplemented predicates are recorded by name.

use crate::Object;
use crate::model::{Version, call};

/// Expressions nest at most this deep; deeper text is not evaluated.
const MAX_DEPTH: usize = 48;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expr {
    Func(String, Vec<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Cmp(Cmp, Box<Expr>, Box<Expr>),
    Arith(Arith, Box<Expr>, Box<Expr>),
    Int(i64),
    Real(f64),
    Str(Vec<u8>),
    Bool(bool),
    Null,
    /// `@path`: the value at a path.
    Value(Vec<String>),
    /// A bare word: a key path when a predicate wants one, otherwise a name.
    Word(Vec<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arith {
    Add,
    Sub,
    Mul,
    Mod,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Fn(String),
    Open,
    Close,
    Comma,
    And,
    Or,
    Cmp(Cmp),
    Plus,
    Str(Vec<u8>),
    Word(String),
}

fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '*' | ':' | '@' | '#')
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            ' ' | '\t' => {}
            '(' => out.push(Tok::Open),
            ')' => out.push(Tok::Close),
            ',' => out.push(Tok::Comma),
            '+' => out.push(Tok::Plus),
            '&' | '|' => {
                chars.next_if(|(_, d)| *d == c)?;
                out.push(if c == '&' { Tok::And } else { Tok::Or });
            }
            '=' | '!' | '<' | '>' => {
                let eq = chars.next_if(|(_, d)| *d == '=').is_some();
                out.push(Tok::Cmp(match (c, eq) {
                    ('=', true) => Cmp::Eq,
                    ('!', true) => Cmp::Ne,
                    ('<', false) => Cmp::Lt,
                    ('<', true) => Cmp::Le,
                    ('>', false) => Cmp::Gt,
                    ('>', true) => Cmp::Ge,
                    _ => return None,
                }));
            }
            '\'' => {
                let mut bytes = Vec::new();
                loop {
                    match chars.next() {
                        Some((_, '\'')) => break,
                        Some((_, d)) => {
                            let mut buf = [0u8; 4];
                            bytes.extend_from_slice(d.encode_utf8(&mut buf).as_bytes());
                        }
                        None => return None,
                    }
                }
                out.push(Tok::Str(bytes));
            }
            c if word_char(c) => {
                let mut end = i + c.len_utf8();
                while let Some((j, d)) = chars.peek().copied() {
                    if !word_char(d) {
                        break;
                    }
                    end = j + d.len_utf8();
                    chars.next();
                }
                let w = s.get(i..end)?;
                if let Some(name) = w.strip_prefix("fn:") {
                    out.push(Tok::Fn(name.to_string()));
                } else {
                    out.push(Tok::Word(w.to_string()));
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Parse one expression; `None` for text outside the grammar.
pub(crate) fn parse(s: &str) -> Option<Expr> {
    let toks = lex(s)?;
    let mut p = Parser { toks, at: 0, depth: 0 };
    let e = p.logic()?;
    (p.at == p.toks.len()).then_some(e)
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn enter(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= MAX_DEPTH).then_some(())
    }

    /// `&&` and `||` at one level, left to right: the model brackets every mix of the two.
    fn logic(&mut self) -> Option<Expr> {
        self.enter()?;
        let mut left = self.comparison()?;
        loop {
            match self.peek() {
                Some(Tok::And) => {
                    self.bump();
                    left = Expr::And(Box::new(left), Box::new(self.comparison()?));
                }
                Some(Tok::Or) => {
                    self.bump();
                    left = Expr::Or(Box::new(left), Box::new(self.comparison()?));
                }
                _ => break,
            }
        }
        self.depth -= 1;
        Some(left)
    }

    fn comparison(&mut self) -> Option<Expr> {
        let left = self.sum()?;
        if let Some(Tok::Cmp(op)) = self.peek().cloned() {
            self.bump();
            let right = self.sum()?;
            return Some(Expr::Cmp(op, Box::new(left), Box::new(right)));
        }
        Some(left)
    }

    fn sum(&mut self) -> Option<Expr> {
        let mut left = self.product()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => Arith::Add,
                Some(Tok::Word(w)) if w == "-" => Arith::Sub,
                _ => break,
            };
            self.bump();
            left = Expr::Arith(op, Box::new(left), Box::new(self.product()?));
        }
        Some(left)
    }

    fn product(&mut self) -> Option<Expr> {
        let mut left = self.atom()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Word(w)) if w == "*" => Arith::Mul,
                Some(Tok::Word(w)) if w == "mod" => Arith::Mod,
                _ => break,
            };
            self.bump();
            left = Expr::Arith(op, Box::new(left), Box::new(self.atom()?));
        }
        Some(left)
    }

    fn atom(&mut self) -> Option<Expr> {
        match self.bump()? {
            Tok::Open => {
                let e = self.logic()?;
                (self.bump()? == Tok::Close).then_some(e)
            }
            Tok::Fn(name) => {
                if self.bump()? != Tok::Open {
                    return None;
                }
                let mut args = Vec::new();
                if self.peek() == Some(&Tok::Close) {
                    self.bump();
                } else {
                    loop {
                        args.push(self.logic()?);
                        match self.bump()? {
                            Tok::Comma => {}
                            Tok::Close => break,
                            _ => return None,
                        }
                    }
                }
                Some(Expr::Func(name, args))
            }
            Tok::Str(s) => Some(Expr::Str(s)),
            Tok::Word(w) => Some(word(&w)),
            _ => None,
        }
    }
}

fn word(w: &str) -> Expr {
    match w {
        "true" => return Expr::Bool(true),
        "false" => return Expr::Bool(false),
        "null" => return Expr::Null,
        _ => {}
    }
    let numeric = |s: &str| {
        let s = s.strip_prefix('-').unwrap_or(s);
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || b == b'.') && s.bytes().filter(|b| *b == b'.').count() <= 1
    };
    if numeric(w) {
        if let Ok(i) = w.parse::<i64>() {
            return Expr::Int(i);
        }
        if let Ok(r) = w.parse::<f64>()
            && r.is_finite()
        {
            return Expr::Real(r);
        }
    }
    let segments: Vec<String> = w.split("::").map(str::to_string).collect();
    match segments.last().and_then(|l| l.strip_prefix('@')) {
        Some(last) => {
            let mut path = segments.clone();
            if let Some(end) = path.last_mut() {
                *end = last.to_string();
            }
            Expr::Value(path)
        }
        None => Expr::Word(segments),
    }
}

/// A value a predicate computes or reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Val {
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(Vec<u8>),
    Str(Vec<u8>),
    /// An array, dictionary or stream: present, but not a value to compare.
    Other,
}

impl Val {
    fn num(&self) -> Option<f64> {
        match self {
            Val::Int(i) => Some(*i as f64),
            Val::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub(crate) fn of(o: &Object) -> Option<Val> {
        Some(match o {
            Object::Bool(b) => Val::Bool(*b),
            Object::Int(i) => Val::Int(*i),
            Object::Real(r) => Val::Real(*r),
            Object::Name(n) => Val::Name(n.clone()),
            Object::String { bytes, .. } => Val::Str(bytes.clone()),
            Object::Array(_) | Object::Dict(_) | Object::Stream(_) => Val::Other,
            Object::Null | Object::Ref(_) => return None,
        })
    }
}

/// What a path leads to.
pub(crate) enum Found {
    Absent,
    Present(Object),
    /// The path goes somewhere this validator cannot follow (`*`, a missing parent).
    Unknown,
}

/// What an evaluation can see of the document.
pub(crate) trait Env {
    fn version(&self) -> Version;
    /// Follow a path from the object holding the row (or `parent::`, `trailer::`); references
    /// are resolved, and a null value counts as absent (ISO 32000-2 §7.3.7).
    fn find(&self, path: &[String]) -> Found;
    /// The row's own value, for the bit predicates.
    fn current(&self) -> Option<&Object>;
    /// The object holding the row.
    fn holder(&self) -> Option<&Object>;
    fn file_size(&self) -> Option<u64>;
    fn page_count(&self) -> Option<usize>;
    /// Note a predicate this validator does not evaluate.
    fn unsupported(&self, name: &str);
}

/// How a version predicate with a statement reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// In a condition (Required, IndirectReference): "in version v, and statement".
    Condition,
    /// In an assertion (SpecialCase, PossibleValues): "in version v, statement holds"; in other
    /// versions it says nothing, so it holds.
    Assertion,
}

/// The standard 14 font names (ISO 32000-2 §9.6.2.2), for `fn:NotStandard14Font`.
const STANDARD_14: &[&[u8]] = &[
    b"Times-Roman",
    b"Times-Bold",
    b"Times-Italic",
    b"Times-BoldItalic",
    b"Helvetica",
    b"Helvetica-Bold",
    b"Helvetica-Oblique",
    b"Helvetica-BoldOblique",
    b"Courier",
    b"Courier-Bold",
    b"Courier-Oblique",
    b"Courier-BoldOblique",
    b"Symbol",
    b"ZapfDingbats",
];

pub(crate) struct Eval<'e> {
    pub env: &'e dyn Env,
    pub mode: Mode,
}

impl Eval<'_> {
    /// Evaluate to a truth value, if one can be told.
    pub(crate) fn truth(&self, e: &Expr) -> Option<bool> {
        match self.value(e, 0)? {
            Val::Bool(b) => Some(b),
            _ => None,
        }
    }

    fn version_arg(&self, e: &Expr) -> Option<Version> {
        match e {
            Expr::Real(r) => Version::parse(&format!("{r:.1}")),
            Expr::Int(i) => Version::parse(&format!("{i}.0")),
            _ => None,
        }
    }

    /// A key path argument: a bare word (or an `@`value whose key is meant).
    fn path_arg<'a>(&self, e: &'a Expr) -> Option<&'a [String]> {
        match e {
            Expr::Word(p) | Expr::Value(p) => Some(p),
            _ => None,
        }
    }

    fn found(&self, e: &Expr) -> Option<Option<Object>> {
        match self.env.find(self.path_arg(e)?) {
            Found::Absent => Some(None),
            Found::Present(o) => Some(Some(o)),
            Found::Unknown => None,
        }
    }

    fn value(&self, e: &Expr, depth: usize) -> Option<Val> {
        if depth > MAX_DEPTH {
            return None;
        }
        let d = depth + 1;
        match e {
            Expr::Int(i) => Some(Val::Int(*i)),
            Expr::Real(r) => Some(Val::Real(*r)),
            Expr::Str(s) => Some(Val::Str(s.clone())),
            Expr::Bool(b) => Some(Val::Bool(*b)),
            Expr::Null => None,
            Expr::Value(p) => match self.env.find(p) {
                Found::Present(o) => Val::of(&o),
                Found::Absent | Found::Unknown => None,
            },
            // A bare word standing alone as a truth value says the key is present.
            Expr::Word(p) => match self.env.find(p) {
                Found::Present(_) => Some(Val::Bool(true)),
                Found::Absent => Some(Val::Bool(false)),
                Found::Unknown => None,
            },
            Expr::And(a, b) => {
                let (x, y) = (self.bool_of(a, d), self.bool_of(b, d));
                match (x, y) {
                    (Some(false), _) | (_, Some(false)) => Some(Val::Bool(false)),
                    (Some(true), Some(true)) => Some(Val::Bool(true)),
                    _ => None,
                }
            }
            Expr::Or(a, b) => {
                let (x, y) = (self.bool_of(a, d), self.bool_of(b, d));
                match (x, y) {
                    (Some(true), _) | (_, Some(true)) => Some(Val::Bool(true)),
                    (Some(false), Some(false)) => Some(Val::Bool(false)),
                    _ => None,
                }
            }
            Expr::Cmp(op, a, b) => self.compare(*op, a, b, d),
            Expr::Arith(op, a, b) => {
                let (x, y) = (self.value(a, d)?, self.value(b, d)?);
                arith(*op, &x, &y)
            }
            Expr::Func(name, args) => self.func(name, args, d),
        }
    }

    fn bool_of(&self, e: &Expr, depth: usize) -> Option<bool> {
        match self.value(e, depth)? {
            Val::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// An operand of a comparison: a bare word there is a name, as in `@Filter==JPXDecode`.
    fn operand(&self, e: &Expr, depth: usize) -> Option<Val> {
        match e {
            Expr::Word(p) if p.len() == 1 => p.first().map(|w| Val::Name(w.as_bytes().to_vec())),
            _ => self.value(e, depth),
        }
    }

    fn compare(&self, op: Cmp, a: &Expr, b: &Expr, depth: usize) -> Option<Val> {
        let (x, y) = (self.operand(a, depth)?, self.operand(b, depth)?);
        let ord = match (&x, &y) {
            (Val::Int(i), Val::Int(j)) => i.cmp(j),
            _ if x.num().is_some() && y.num().is_some() => x.num()?.partial_cmp(&y.num()?)?,
            (Val::Name(m), Val::Name(n)) | (Val::Str(m), Val::Str(n)) if matches!(op, Cmp::Eq | Cmp::Ne) => m.cmp(n),
            (Val::Bool(m), Val::Bool(n)) if matches!(op, Cmp::Eq | Cmp::Ne) => m.cmp(n),
            // Different kinds of value: nothing can be told.
            _ => return None,
        };
        use std::cmp::Ordering::*;
        Some(Val::Bool(match op {
            Cmp::Eq => ord == Equal,
            Cmp::Ne => ord != Equal,
            Cmp::Lt => ord == Less,
            Cmp::Le => ord != Greater,
            Cmp::Gt => ord == Greater,
            Cmp::Ge => ord != Less,
        }))
    }

    /// A version predicate: `v` alone is whether it holds; with a statement it depends on the mode.
    fn versioned(&self, holds: bool, statement: Option<&Expr>, depth: usize) -> Option<Val> {
        let Some(s) = statement else { return Some(Val::Bool(holds)) };
        match (self.mode, holds) {
            (_, true) => self.value(s, depth),
            (Mode::Condition, false) => Some(Val::Bool(false)),
            (Mode::Assertion, false) => Some(Val::Bool(true)),
        }
    }

    fn func(&self, name: &str, args: &[Expr], depth: usize) -> Option<Val> {
        let version = self.env.version();
        let arg = |i: usize| args.get(i);
        match (name, args.len()) {
            ("Eval" | "IsRequired", 1) => self.value(arg(0)?, depth),
            ("Not", 1) => Some(Val::Bool(!self.bool_of(arg(0)?, depth)?)),
            ("SinceVersion", 1 | 2) => self.versioned(version >= self.version_arg(arg(0)?)?, arg(1), depth),
            ("BeforeVersion", 1 | 2) => self.versioned(version < self.version_arg(arg(0)?)?, arg(1), depth),
            ("IsPDFVersion", 1 | 2) => self.versioned(version == self.version_arg(arg(0)?)?, arg(1), depth),
            ("Deprecated", 2) => self.value(arg(1)?, depth),
            ("IsPresent", 1) if self.path_arg(arg(0)?).is_some() => Some(Val::Bool(self.found(arg(0)?)?.is_some())),
            // "When the key is present, the condition holds": only an assertion reads that way.
            ("IsPresent", 2) if self.mode == Mode::Assertion && self.path_arg(arg(0)?).is_some() => {
                if self.found(arg(0)?)?.is_some() {
                    self.value(arg(1)?, depth)
                } else {
                    Some(Val::Bool(true))
                }
            }
            ("ArrayLength", 1) => match self.found(arg(0)?)?? {
                Object::Array(a) => i64::try_from(a.len()).ok().map(Val::Int),
                _ => None,
            },
            ("StringLength", 1) => match self.found(arg(0)?)?? {
                Object::String { bytes, .. } => i64::try_from(bytes.len()).ok().map(Val::Int),
                _ => None,
            },
            ("RectWidth" | "RectHeight", 1) => {
                let Object::Array(a) = self.found(arg(0)?)?? else { return None };
                let n: Vec<f64> = a.iter().map(number).collect::<Option<_>>()?;
                let [x0, y0, x1, y1] = n.as_slice() else { return None };
                Some(Val::Real(if name == "RectWidth" { (x1 - x0).abs() } else { (y1 - y0).abs() }))
            }
            ("Contains", 2) => {
                let wanted = self.operand(arg(1)?, depth)?;
                match self.found(arg(0)?)?? {
                    Object::Array(items) => {
                        let mut unknown = false;
                        for item in &items {
                            match Val::of(item) {
                                Some(v) if v == wanted => return Some(Val::Bool(true)),
                                Some(_) => {}
                                None => unknown = true,
                            }
                        }
                        if unknown { None } else { Some(Val::Bool(false)) }
                    }
                    other => Some(Val::Bool(Val::of(&other)? == wanted)),
                }
            }
            ("BitSet" | "BitClear", 1) => {
                let bit = self.bit(arg(0)?)?;
                let set = self.bits()? & (1u32 << (bit - 1)) != 0;
                Some(Val::Bool(set == (name == "BitSet")))
            }
            ("BitsSet" | "BitsClear", 2) => {
                let (lo, hi) = (self.bit(arg(0)?)?, self.bit(arg(1)?)?);
                if lo > hi {
                    return None;
                }
                let bits = self.bits()?;
                let all = (lo..=hi).all(|b| (bits & (1u32 << (b - 1)) != 0) == (name == "BitsSet"));
                Some(Val::Bool(all))
            }
            ("FileSize", 0) => self.env.file_size().and_then(|s| i64::try_from(s).ok()).map(Val::Int),
            ("NumberOfPages", 0) => self.env.page_count().and_then(|s| i64::try_from(s).ok()).map(Val::Int),
            ("IsPDFTagged", 0) => {
                let path: Vec<String> = ["trailer", "Root", "MarkInfo", "Marked"].iter().map(|s| s.to_string()).collect();
                match self.env.find(&path) {
                    Found::Present(Object::Bool(b)) => Some(Val::Bool(b)),
                    Found::Absent => Some(Val::Bool(false)),
                    _ => None,
                }
            }
            ("NotStandard14Font", 0) => {
                let (Object::Dict(d) | Object::Stream(d)) = self.env.holder()? else { return None };
                let get = |k: &[u8]| d.iter().find(|(n, _)| n.as_slice() == k).map(|(_, v)| v);
                match (get(b"Subtype"), get(b"BaseFont")) {
                    (Some(Object::Name(s)), Some(Object::Name(base))) if s.as_slice() == b"Type1" => {
                        Some(Val::Bool(!STANDARD_14.contains(&base.as_slice())))
                    }
                    (Some(Object::Name(s)), _) if s.as_slice() != b"Type1" => Some(Val::Bool(true)),
                    _ => None,
                }
            }
            ("IsHexString", 0) => match self.env.current()? {
                Object::String { hex, .. } => Some(Val::Bool(*hex)),
                _ => None,
            },
            _ => {
                self.env.unsupported(name);
                None
            }
        }
    }

    fn bit(&self, e: &Expr) -> Option<u32> {
        match e {
            Expr::Int(i) => u32::try_from(*i).ok().filter(|b| (1..=32).contains(b)),
            _ => None,
        }
    }

    /// The row's own value as 32 bits; values that don't fit are not judged.
    fn bits(&self) -> Option<u32> {
        match self.env.current()? {
            Object::Int(i) => u32::try_from(*i).ok().or_else(|| i32::try_from(*i).ok().map(|v| v as u32)),
            _ => None,
        }
    }
}

fn number(o: &Object) -> Option<f64> {
    match o {
        Object::Int(i) => Some(*i as f64),
        Object::Real(r) if r.is_finite() => Some(*r),
        _ => None,
    }
}

fn arith(op: Arith, x: &Val, y: &Val) -> Option<Val> {
    if let (Val::Int(a), Val::Int(b)) = (x, y) {
        let r = match op {
            Arith::Add => a.checked_add(*b),
            Arith::Sub => a.checked_sub(*b),
            Arith::Mul => a.checked_mul(*b),
            Arith::Mod => a.checked_rem_euclid(*b),
        };
        if let Some(r) = r {
            return Some(Val::Int(r));
        }
    }
    let (a, b) = (x.num()?, y.num()?);
    let r = match op {
        Arith::Add => a + b,
        Arith::Sub => a - b,
        Arith::Mul => a * b,
        Arith::Mod if b != 0.0 => a.rem_euclid(b),
        Arith::Mod => return None,
    };
    r.is_finite().then_some(Val::Real(r))
}

/// The names of the predicates in `text`, for reporting what was not checked.
pub(crate) fn predicate_names(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices("fn:").filter_map(move |(at, _)| {
        let rest = text.get(at + 3..)?;
        let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
        rest.get(..end).filter(|n| !n.is_empty())
    })
}

/// `fn:F(args)` when `s` is a single call to `F`.
pub(crate) fn single_call<'a>(s: &'a str, f: &str) -> Option<&'a str> {
    call(s).filter(|(name, _)| *name == f).map(|(_, args)| args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct TestEnv {
        version: Version,
        holder: Object,
        current: Option<Object>,
        unsupported: RefCell<Vec<String>>,
    }

    impl Env for TestEnv {
        fn version(&self) -> Version {
            self.version
        }
        fn find(&self, path: &[String]) -> Found {
            let [key] = path else { return Found::Unknown };
            let Object::Dict(d) = &self.holder else { return Found::Unknown };
            match d.iter().find(|(k, _)| k.as_slice() == key.as_bytes()) {
                Some((_, Object::Null)) | None => Found::Absent,
                Some((_, v)) => Found::Present(v.clone()),
            }
        }
        fn current(&self) -> Option<&Object> {
            self.current.as_ref()
        }
        fn holder(&self) -> Option<&Object> {
            Some(&self.holder)
        }
        fn file_size(&self) -> Option<u64> {
            Some(1000)
        }
        fn page_count(&self) -> Option<usize> {
            Some(3)
        }
        fn unsupported(&self, name: &str) {
            self.unsupported.borrow_mut().push(name.to_string());
        }
    }

    fn env(entries: &[(&str, Object)]) -> TestEnv {
        TestEnv {
            version: Version(17),
            holder: Object::Dict(entries.iter().map(|(k, v)| (k.as_bytes().to_vec(), v.clone())).collect()),
            current: None,
            unsupported: RefCell::new(Vec::new()),
        }
    }

    fn truth(env: &TestEnv, mode: Mode, text: &str) -> Option<bool> {
        let e = parse(text).unwrap_or_else(|| panic!("parses: {text}"));
        Eval { env, mode }.truth(&e)
    }

    fn name(s: &str) -> Object {
        Object::Name(s.as_bytes().to_vec())
    }

    #[test]
    fn parses_the_model_grammar() {
        assert_eq!(parse("@Size>0"), Some(Expr::Cmp(Cmp::Gt, Box::new(Expr::Value(vec!["Size".into()])), Box::new(Expr::Int(0)))));
        assert!(parse("fn:IsRequired(fn:SinceVersion(1.4,fn:IsPresent(trailer::Catalog::PieceInfo)))").is_some());
        assert!(parse("fn:Eval((fn:ArrayLength(RD)==4) && (RD::@0>=0) && ((RD::@1+RD::@3)<fn:RectHeight(Rect)))").is_some());
        assert!(parse("fn:Eval((@Length>=40) && (@Length<=128) && ((@Length mod 8)==0))").is_some());
        assert!(parse("fn:Eval(fn:StreamLength(DL)==(@Width * @Height))").is_some());
        assert!(parse("@SubFilter==adbe.x509.rsa_sha1").is_some());
        assert_eq!(parse("@A==-1"), Some(Expr::Cmp(Cmp::Eq, Box::new(Expr::Value(vec!["A".into()])), Box::new(Expr::Int(-1)))));
        for bad in ["", "(", "fn:A(", "@A==", "a & b", "fn:A(b))", "'open"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        // Nesting past the limit is refused rather than recursed into.
        let deep = format!("{}1{}", "(".repeat(200), ")".repeat(200));
        assert_eq!(parse(&deep), None);
    }

    #[test]
    fn values_compare_by_kind() {
        let e = env(&[("Size", Object::Int(5)), ("Filter", name("JPXDecode")), ("CA", Object::Real(0.5))]);
        assert_eq!(truth(&e, Mode::Assertion, "fn:Eval(@Size>0)"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "@Filter==JPXDecode"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "@Filter!=JPXDecode"), Some(false));
        assert_eq!(truth(&e, Mode::Assertion, "fn:Eval((@CA>=0.0) && (@CA<=1.0))"), Some(true));
        // A name compared with a number, or a missing key: unknown, not false.
        assert_eq!(truth(&e, Mode::Assertion, "@Filter==5"), None);
        assert_eq!(truth(&e, Mode::Assertion, "@Missing>0"), None);
        assert_eq!(truth(&e, Mode::Assertion, "fn:Eval((@Size mod 2)==1)"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "fn:Eval((@Size mod 0)==1)"), None);
    }

    #[test]
    fn unknown_does_not_decide_and_or() {
        let e = env(&[("A", Object::Int(1))]);
        assert_eq!(truth(&e, Mode::Condition, "(@A==1) || (@B==2)"), Some(true));
        assert_eq!(truth(&e, Mode::Condition, "(@A==2) && (@B==2)"), Some(false));
        assert_eq!(truth(&e, Mode::Condition, "(@A==1) && (@B==2)"), None);
        assert_eq!(truth(&e, Mode::Condition, "fn:Not(fn:IsPresent(B))"), Some(true));
    }

    #[test]
    fn version_predicates_read_by_mode() {
        let e = env(&[("RT", name("R"))]);
        // In a condition: "in 1.6 or later, and RT is present".
        assert_eq!(truth(&e, Mode::Condition, "fn:SinceVersion(1.6,fn:IsPresent(RT))"), Some(true));
        assert_eq!(truth(&e, Mode::Condition, "fn:SinceVersion(2.0,fn:IsPresent(RT))"), Some(false));
        assert_eq!(truth(&e, Mode::Condition, "fn:IsPDFVersion(1.3,fn:IsPresent(RT))"), Some(false));
        // In an assertion a statement for another version says nothing about this file.
        assert_eq!(truth(&e, Mode::Assertion, "fn:IsPDFVersion(1.3,fn:IsPresent(Nope))"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "fn:SinceVersion(1.7,fn:IsPresent(Nope))"), Some(false));
        assert_eq!(truth(&e, Mode::Condition, "fn:BeforeVersion(2.0)"), Some(true));
    }

    #[test]
    fn unimplemented_predicates_are_unknown_and_recorded() {
        let e = env(&[]);
        assert_eq!(truth(&e, Mode::Condition, "fn:InNameTree(trailer::Catalog::Names::Pages)"), None);
        assert_eq!(truth(&e, Mode::Condition, "fn:Extension(PDF_VT2)"), None);
        assert_eq!(*e.unsupported.borrow(), ["InNameTree", "Extension"]);
    }

    #[test]
    fn bits_arrays_strings_and_fonts() {
        let mut e = env(&[
            ("Filter", Object::Array(vec![name("FlateDecode"), name("DCTDecode")])),
            ("Rect", Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(10), Object::Real(5.0)])),
            ("Subtype", name("Type1")),
            ("BaseFont", name("Helvetica")),
        ]);
        e.current = Some(Object::Int(0b101));
        assert_eq!(truth(&e, Mode::Assertion, "fn:BitSet(1)"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "fn:BitClear(2)"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "fn:BitsClear(4,32)"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "fn:BitsClear(1,32)"), Some(false));
        assert_eq!(truth(&e, Mode::Assertion, "fn:BitSet(33)"), None);
        assert_eq!(truth(&e, Mode::Assertion, "fn:Contains(@Filter,DCTDecode)"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "fn:Contains(@Filter,JPXDecode)"), Some(false));
        assert_eq!(truth(&e, Mode::Assertion, "fn:Eval(fn:ArrayLength(Filter)==2)"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "fn:Eval(fn:RectWidth(Rect)>0)"), Some(true));
        assert_eq!(truth(&e, Mode::Assertion, "fn:Eval(fn:ArrayLength(Subtype)==2)"), None);
        assert_eq!(truth(&e, Mode::Condition, "fn:NotStandard14Font()"), Some(false));
        assert_eq!(truth(&e, Mode::Assertion, "fn:Eval(fn:FileSize()>999)"), Some(true));
    }

    #[test]
    fn predicate_names_are_listed() {
        let names: Vec<_> = predicate_names("fn:IsRequired(fn:InNameTree(x) || fn:Not(y))").collect();
        assert_eq!(names, ["IsRequired", "InNameTree", "Not"]);
        assert_eq!(single_call("fn:Eval(@A>0)", "Eval"), Some("@A>0"));
        assert_eq!(single_call("fn:Ignore()", "Eval"), None);
    }
}
