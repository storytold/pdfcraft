//! PDF functions (ISO 32000-2 7.10), read from a document and evaluated, and sampled functions
//! written.
//!
//! Converting colour to CMYK needs them: a spot colour's tint transform and a shading's colour
//! function map their inputs to colour in a space PDF/X-1a doesn't allow. The conversion
//! evaluates them, and writes sampled functions into DeviceCMYK in their place
//! ([`sampled_stream`], [`stitching`]).
//!
//! All four types evaluate: sampled (type 0, with linear interpolation; a cubic `Order` isn't
//! read), exponential (2), stitching (3) and PostScript calculator (4). Reading is bounded (how
//! deep stitching nests, how many functions one holds, how many samples and instructions), and
//! evaluating never panics: an input a function can't take (a negative base to a fractional power,
//! a calculator program that divides by zero or empties its stack) has no value.

use pdfcraft_cos::{Dict, Document, Object, Stream};

/// How deep stitching functions nest.
const MAX_DEPTH: u8 = 16;
/// The most functions one function is made of.
const MAX_NODES: usize = 1024;
/// The most sample values a sampled function holds.
const MAX_SAMPLE_VALUES: usize = 1 << 22;
/// The most inputs a sampled function interpolates over.
const MAX_INPUTS: usize = 16;
/// The most tokens a calculator program holds.
const MAX_TOKENS: usize = 1 << 16;
/// How deep a calculator program's procedures nest.
const MAX_BLOCK_DEPTH: u8 = 64;
/// The calculator's operand stack (ISO 32000-2 7.10.5.1).
const MAX_STACK: usize = 100;

/// One function.
#[derive(Clone, Debug)]
pub(crate) struct Function {
    domain: Vec<(f64, f64)>,
    range: Option<Vec<(f64, f64)>>,
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    Sampled(Sampled),
    Exponential { c0: Vec<f64>, c1: Vec<f64>, n: f64 },
    Stitching { functions: Vec<Function>, bounds: Vec<f64>, encode: Vec<(f64, f64)> },
    Calculator(Vec<Instr>),
}

#[derive(Clone, Debug)]
struct Sampled {
    size: Vec<usize>,
    bits: u32,
    encode: Vec<(f64, f64)>,
    decode: Vec<(f64, f64)>,
    data: Vec<u8>,
}

/// A colour function: one function with all the colour's components as outputs, or one function
/// per component (shadings may have either, ISO 32000-2 8.7.4.5).
#[derive(Clone, Debug)]
pub(crate) struct Functions(Vec<Function>);

impl Functions {
    pub(crate) fn read(doc: &Document, o: &Object) -> Option<Functions> {
        let o = doc.resolve(o);
        match &*o {
            Object::Array(list) => {
                let fs = list.iter().map(|f| Function::read(doc, f)).collect::<Option<Vec<_>>>()?;
                fs.iter().all(|f| f.inputs() == fs.first().map_or(0, Function::inputs) && f.outputs() == Some(1)).then_some(())?;
                (!fs.is_empty()).then_some(Functions(fs))
            }
            _ => Some(Functions(vec![Function::read(doc, &o)?])),
        }
    }

    pub(crate) fn inputs(&self) -> usize {
        self.0.first().map_or(0, Function::inputs)
    }

    pub(crate) fn outputs(&self) -> Option<usize> {
        match self.0.as_slice() {
            [one] => one.outputs(),
            many => Some(many.len()),
        }
    }

    /// The domain of the first function (all of them take the same inputs).
    pub(crate) fn domain(&self) -> Vec<(f64, f64)> {
        self.0.first().map(|f| f.domain.clone()).unwrap_or_default()
    }

    pub(crate) fn eval(&self, x: &[f64]) -> Option<Vec<f64>> {
        match self.0.as_slice() {
            [one] => one.eval(x),
            many => many.iter().map(|f| f.eval(x).and_then(|v| v.first().copied())).collect(),
        }
    }

    /// Where between `lo` and `hi` a one-input function may jump: the bounds of its stitching
    /// functions, sorted, each once.
    pub(crate) fn breaks(&self, lo: f64, hi: f64) -> Vec<f64> {
        let mut out = Vec::new();
        for f in &self.0 {
            f.breaks(lo, hi, &mut out);
        }
        out.retain(|b| b.is_finite() && *b > lo && *b < hi);
        out.sort_by(f64::total_cmp);
        out.dedup();
        out
    }
}

impl Function {
    pub(crate) fn read(doc: &Document, o: &Object) -> Option<Function> {
        let mut budget = MAX_NODES;
        read_at(doc, o, 0, &mut budget)
    }

    pub(crate) fn inputs(&self) -> usize {
        self.domain.len()
    }

    pub(crate) fn outputs(&self) -> Option<usize> {
        if let Some(r) = &self.range {
            return Some(r.len());
        }
        match &self.kind {
            Kind::Sampled(s) => Some(s.decode.len()),
            Kind::Exponential { c0, .. } => Some(c0.len()),
            Kind::Stitching { functions, .. } => functions.first()?.outputs(),
            Kind::Calculator(_) => None,
        }
    }

    pub(crate) fn eval(&self, x: &[f64]) -> Option<Vec<f64>> {
        if x.len() != self.domain.len() || x.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let x: Vec<f64> = x.iter().zip(&self.domain).map(|(v, (lo, hi))| v.clamp(*lo, *hi)).collect();
        let mut y = match &self.kind {
            Kind::Sampled(s) => s.eval(&x, &self.domain)?,
            Kind::Exponential { c0, c1, n } => {
                let v = *x.first()?;
                let p = v.powf(*n);
                if !p.is_finite() {
                    return None;
                }
                c0.iter().zip(c1).map(|(a, b)| a + p * (b - a)).collect()
            }
            Kind::Stitching { functions, bounds, encode } => {
                let v = *x.first()?;
                let (lo, hi) = *self.domain.first()?;
                // The first subdomain whose upper bound lies past `v`; the last takes the rest.
                // When the domain starts at the first bound, that bound belongs to the first.
                let i = if bounds.first() == Some(&lo) && v == lo { 0 } else { bounds.iter().position(|b| v < *b).unwrap_or(bounds.len()) };
                let low = if i == 0 { lo } else { *bounds.get(i - 1)? };
                let high = bounds.get(i).copied().unwrap_or(hi);
                let (e0, e1) = *encode.get(i)?;
                functions.get(i)?.eval(&[interpolate(v, low, high, e0, e1)])?
            }
            Kind::Calculator(program) => calculate(program, &x, self.range.as_ref()?.len())?,
        };
        if let Some(range) = &self.range {
            if y.len() != range.len() {
                return None;
            }
            for (v, (lo, hi)) in y.iter_mut().zip(range) {
                *v = v.clamp(*lo, *hi);
            }
        }
        y.iter().all(|v| v.is_finite()).then_some(y)
    }

    fn breaks(&self, lo: f64, hi: f64, out: &mut Vec<f64>) {
        let Kind::Stitching { functions, bounds, encode } = &self.kind else { return };
        let Some(&(d0, d1)) = self.domain.first() else { return };
        out.extend(bounds.iter().copied());
        for (i, f) in functions.iter().enumerate() {
            let low = if i == 0 { d0 } else { bounds.get(i - 1).copied().unwrap_or(d0) };
            let high = bounds.get(i).copied().unwrap_or(d1);
            let Some(&(e0, e1)) = encode.get(i) else { continue };
            if high <= lo || low >= hi || e0 == e1 {
                continue;
            }
            let mut inner = Vec::new();
            f.breaks(e0.min(e1), e0.max(e1), &mut inner);
            out.extend(inner.into_iter().map(|t| interpolate(t, e0, e1, low, high)));
        }
    }
}

/// `x` mapped linearly from `[x0, x1]` to `[y0, y1]` (ISO 32000-2 7.10.2): `y0` when the first
/// interval is empty.
fn interpolate(x: f64, x0: f64, x1: f64, y0: f64, y1: f64) -> f64 {
    if x1 == x0 { y0 } else { y0 + (x - x0) * (y1 - y0) / (x1 - x0) }
}

fn number(doc: &Document, o: &Object) -> Option<f64> {
    doc.resolve(o).as_f64().filter(|v| v.is_finite())
}

fn numbers(doc: &Document, dict: &Dict, key: &[u8]) -> Option<Vec<f64>> {
    match &*doc.resolve(dict.get(key)?) {
        Object::Array(a) => a.iter().map(|v| number(doc, v)).collect(),
        _ => None,
    }
}

/// Pairs of numbers, each pair in order.
fn pairs(values: &[f64]) -> Option<Vec<(f64, f64)>> {
    if !values.len().is_multiple_of(2) {
        return None;
    }
    values.as_chunks::<2>().0.iter().map(|&[lo, hi]| (lo <= hi).then_some((lo, hi))).collect()
}

fn read_at(doc: &Document, o: &Object, depth: u8, budget: &mut usize) -> Option<Function> {
    if depth > MAX_DEPTH || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let o = doc.resolve(o);
    let (dict, stream) = match &*o {
        Object::Stream(s) => (&s.dict, Some(s)),
        Object::Dict(d) => (d, None),
        _ => return None,
    };
    let domain = pairs(&numbers(doc, dict, b"Domain")?)?;
    if domain.is_empty() {
        return None;
    }
    let range = match dict.get(b"Range") {
        Some(_) => Some(pairs(&numbers(doc, dict, b"Range")?)?).filter(|r| !r.is_empty()),
        None => None,
    };
    let kind = match doc.resolve(dict.get(b"FunctionType")?).as_int()? {
        0 => Kind::Sampled(read_sampled(doc, dict, stream?, &domain, range.as_deref()?)?),
        2 => {
            if domain.len() != 1 {
                return None;
            }
            let c0 = if dict.get(b"C0").is_some() { numbers(doc, dict, b"C0")? } else { vec![0.0] };
            let c1 = if dict.get(b"C1").is_some() { numbers(doc, dict, b"C1")? } else { vec![1.0] };
            let n = number(doc, dict.get(b"N")?)?;
            if c0.len() != c1.len() || c0.is_empty() {
                return None;
            }
            Kind::Exponential { c0, c1, n }
        }
        3 => {
            let &[(lo, hi)] = domain.as_slice() else { return None };
            let functions = match &*doc.resolve(dict.get(b"Functions")?) {
                Object::Array(a) => a.iter().map(|f| read_at(doc, f, depth.saturating_add(1), budget)).collect::<Option<Vec<_>>>()?,
                _ => return None,
            };
            let bounds = numbers(doc, dict, b"Bounds")?;
            let encode = pairs_any(&numbers(doc, dict, b"Encode")?)?;
            let outputs = functions.first()?.outputs();
            let ordered = bounds.windows(2).all(|w| matches!(w, [a, b] if a <= b));
            if bounds.len().checked_add(1) != Some(functions.len())
                || encode.len() != functions.len()
                || !ordered
                || bounds.iter().any(|b| *b < lo || *b > hi)
                || functions.iter().any(|f| f.inputs() != 1 || f.outputs() != outputs)
            {
                return None;
            }
            Kind::Stitching { functions, bounds, encode }
        }
        4 => {
            range.as_ref()?;
            let text = stream?.decoded_strict().ok()?;
            Kind::Calculator(parse_program(&text)?)
        }
        _ => return None,
    };
    Some(Function { domain, range, kind })
}

/// Pairs of numbers in any order (an `Encode` interval may run backwards).
fn pairs_any(values: &[f64]) -> Option<Vec<(f64, f64)>> {
    if !values.len().is_multiple_of(2) {
        return None;
    }
    Some(values.as_chunks::<2>().0.iter().map(|&[a, b]| (a, b)).collect())
}

fn read_sampled(doc: &Document, dict: &Dict, stream: &Stream, domain: &[(f64, f64)], range: &[(f64, f64)]) -> Option<Sampled> {
    let size: Vec<usize> = match &*doc.resolve(dict.get(b"Size")?) {
        Object::Array(a) => {
            a.iter().map(|v| doc.resolve(v).as_int().and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0)).collect::<Option<_>>()?
        }
        _ => return None,
    };
    if size.len() != domain.len() || size.len() > MAX_INPUTS {
        return None;
    }
    let bits = u32::try_from(doc.resolve(dict.get(b"BitsPerSample")?).as_int()?).ok()?;
    if ![1, 2, 4, 8, 12, 16, 24, 32].contains(&bits) {
        return None;
    }
    // A cubic spline isn't evaluated, and approximating it with straight lines isn't exact.
    if let Some(order) = dict.get(b"Order")
        && doc.resolve(order).as_int() != Some(1)
    {
        return None;
    }
    let encode = match dict.get(b"Encode") {
        Some(_) => pairs_any(&numbers(doc, dict, b"Encode")?)?,
        None => size.iter().map(|n| (0.0, (n.saturating_sub(1)) as f64)).collect(),
    };
    let decode = match dict.get(b"Decode") {
        Some(_) => pairs_any(&numbers(doc, dict, b"Decode")?)?,
        None => range.to_vec(),
    };
    if encode.len() != size.len() || decode.len() != range.len() {
        return None;
    }
    let values = size.iter().try_fold(range.len(), |acc, n| acc.checked_mul(*n))?;
    if values > MAX_SAMPLE_VALUES {
        return None;
    }
    let needed = values.checked_mul(bits as usize)?.div_ceil(8);
    let mut data = stream.decoded_strict().ok()?;
    if data.len() < needed {
        return None;
    }
    data.truncate(needed);
    Some(Sampled { size, bits, encode, decode, data })
}

impl Sampled {
    /// Sample value `index` (counting every output of every point), as stored.
    fn raw(&self, index: usize) -> Option<u64> {
        let bits = self.bits as usize;
        let start = index.checked_mul(bits)?;
        if bits.is_multiple_of(8) {
            let bytes = self.data.get(start / 8..(start / 8).checked_add(bits / 8)?)?;
            return Some(bytes.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b)));
        }
        let mut v = 0u64;
        for bit in start..start.checked_add(bits)? {
            let byte = *self.data.get(bit / 8)?;
            v = (v << 1) | u64::from((byte >> (7 - bit % 8)) & 1);
        }
        Some(v)
    }

    fn eval(&self, x: &[f64], domain: &[(f64, f64)]) -> Option<Vec<f64>> {
        let n = self.decode.len();
        // Each input's position among the samples: a whole part and a fraction.
        let mut at = Vec::with_capacity(x.len());
        for ((v, (d0, d1)), ((e0, e1), size)) in x.iter().zip(domain).zip(self.encode.iter().zip(&self.size)) {
            let last = size.saturating_sub(1) as f64;
            let e = interpolate(*v, *d0, *d1, *e0, *e1).clamp(0.0, last);
            let whole = (e.floor() as usize).min(size.saturating_sub(2));
            at.push((whole, e - whole as f64));
        }
        let max = ((1u64 << self.bits) - 1) as f64;
        let mut out = vec![0.0; n];
        // Every corner of the cell around the point, weighted (inputs at a sample take one).
        let moving: Vec<usize> = at.iter().enumerate().filter(|(_, (_, f))| *f > 0.0).map(|(i, _)| i).collect();
        for corner in 0..(1usize << moving.len()) {
            let mut weight = 1.0;
            let mut offset = 0usize;
            let mut stride = 1usize;
            for (i, ((whole, frac), size)) in at.iter().zip(&self.size).enumerate() {
                let up = moving.iter().position(|m| *m == i).is_some_and(|bit| corner & (1 << bit) != 0);
                let index = if up { whole + 1 } else { *whole };
                if moving.contains(&i) {
                    weight *= if up { *frac } else { 1.0 - frac };
                }
                offset = offset.checked_add(index.checked_mul(stride)?)?;
                stride = stride.checked_mul(*size)?;
            }
            if weight == 0.0 {
                continue;
            }
            for (j, o) in out.iter_mut().enumerate() {
                let raw = self.raw(offset.checked_mul(n)?.checked_add(j)?)? as f64;
                *o += weight * raw;
            }
        }
        Some(out.iter().zip(&self.decode).map(|(s, (d0, d1))| interpolate(*s, 0.0, max, *d0, *d1)).collect())
    }
}

/// A sampled function (type 0) from `domain` into `values.len() / Π size` outputs between 0 and 1:
/// `values` holds each point's outputs, the first input varying fastest, written at 16 bits.
pub(crate) fn sampled_stream(domain: &[(f64, f64)], size: &[usize], values: &[f64]) -> Stream {
    let points = size.iter().try_fold(1usize, |acc, n| acc.checked_mul(*n)).unwrap_or(0);
    let outputs = values.len().checked_div(points).unwrap_or(0);
    let mut dict = Dict::new();
    dict.set(b"FunctionType".to_vec(), Object::Int(0));
    dict.set(b"Domain".to_vec(), Object::Array(domain.iter().flat_map(|(a, b)| [Object::Real(*a), Object::Real(*b)]).collect()));
    dict.set(b"Range".to_vec(), Object::Array((0..outputs).flat_map(|_| [Object::Int(0), Object::Int(1)]).collect()));
    dict.set(b"Size".to_vec(), Object::Array(size.iter().map(|n| Object::Int(i64::try_from(*n).unwrap_or(i64::MAX))).collect()));
    dict.set(b"BitsPerSample".to_vec(), Object::Int(16));
    let mut data = Vec::with_capacity(values.len().saturating_mul(2));
    for v in values {
        let q = if v.is_finite() { (v.clamp(0.0, 1.0) * 65535.0).round() as u16 } else { 0 };
        data.extend(q.to_be_bytes());
    }
    Stream::flate(dict, &data)
}

/// A stitching function (type 3) over `domain` made of `functions`, each over `[0, 1]`, between
/// `bounds`.
pub(crate) fn stitching(domain: (f64, f64), bounds: &[f64], functions: Vec<Object>) -> Object {
    let mut dict = Dict::new();
    dict.set(b"FunctionType".to_vec(), Object::Int(3));
    dict.set(b"Domain".to_vec(), Object::Array(vec![Object::Real(domain.0), Object::Real(domain.1)]));
    dict.set(b"Encode".to_vec(), Object::Array(functions.iter().flat_map(|_| [Object::Int(0), Object::Int(1)]).collect()));
    dict.set(b"Bounds".to_vec(), Object::Array(bounds.iter().map(|b| Object::Real(*b)).collect()));
    dict.set(b"Functions".to_vec(), Object::Array(functions));
    Object::Dict(dict)
}

// The PostScript calculator (ISO 32000-2 7.10.5).

#[derive(Clone, Copy, Debug, PartialEq)]
enum Val {
    Int(i64),
    Real(f64),
    Bool(bool),
}

impl Val {
    fn num(self) -> Option<f64> {
        match self {
            Val::Int(i) => Some(i as f64),
            Val::Real(r) => Some(r),
            Val::Bool(_) => None,
        }
    }

    fn real(v: f64) -> Option<Val> {
        v.is_finite().then_some(Val::Real(v))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Abs,
    Add,
    Atan,
    Ceiling,
    Cos,
    Cvi,
    Cvr,
    Div,
    Exp,
    Floor,
    Idiv,
    Ln,
    Log,
    Mod,
    Mul,
    Neg,
    Round,
    Sin,
    Sqrt,
    Sub,
    Truncate,
    And,
    Bitshift,
    Eq,
    Ge,
    Gt,
    Le,
    Lt,
    Ne,
    Not,
    Or,
    Xor,
    Copy,
    Dup,
    Exch,
    Index,
    Pop,
    Roll,
}

#[derive(Clone, Debug)]
enum Instr {
    Push(Val),
    Op(Op),
    If(Vec<Instr>),
    IfElse(Vec<Instr>, Vec<Instr>),
}

fn operator(word: &[u8]) -> Option<Instr> {
    Some(match word {
        b"true" => Instr::Push(Val::Bool(true)),
        b"false" => Instr::Push(Val::Bool(false)),
        b"abs" => Instr::Op(Op::Abs),
        b"add" => Instr::Op(Op::Add),
        b"atan" => Instr::Op(Op::Atan),
        b"ceiling" => Instr::Op(Op::Ceiling),
        b"cos" => Instr::Op(Op::Cos),
        b"cvi" => Instr::Op(Op::Cvi),
        b"cvr" => Instr::Op(Op::Cvr),
        b"div" => Instr::Op(Op::Div),
        b"exp" => Instr::Op(Op::Exp),
        b"floor" => Instr::Op(Op::Floor),
        b"idiv" => Instr::Op(Op::Idiv),
        b"ln" => Instr::Op(Op::Ln),
        b"log" => Instr::Op(Op::Log),
        b"mod" => Instr::Op(Op::Mod),
        b"mul" => Instr::Op(Op::Mul),
        b"neg" => Instr::Op(Op::Neg),
        b"round" => Instr::Op(Op::Round),
        b"sin" => Instr::Op(Op::Sin),
        b"sqrt" => Instr::Op(Op::Sqrt),
        b"sub" => Instr::Op(Op::Sub),
        b"truncate" => Instr::Op(Op::Truncate),
        b"and" => Instr::Op(Op::And),
        b"bitshift" => Instr::Op(Op::Bitshift),
        b"eq" => Instr::Op(Op::Eq),
        b"ge" => Instr::Op(Op::Ge),
        b"gt" => Instr::Op(Op::Gt),
        b"le" => Instr::Op(Op::Le),
        b"lt" => Instr::Op(Op::Lt),
        b"ne" => Instr::Op(Op::Ne),
        b"not" => Instr::Op(Op::Not),
        b"or" => Instr::Op(Op::Or),
        b"xor" => Instr::Op(Op::Xor),
        b"copy" => Instr::Op(Op::Copy),
        b"dup" => Instr::Op(Op::Dup),
        b"exch" => Instr::Op(Op::Exch),
        b"index" => Instr::Op(Op::Index),
        b"pop" => Instr::Op(Op::Pop),
        b"roll" => Instr::Op(Op::Roll),
        _ => return None,
    })
}

#[derive(Debug, PartialEq)]
enum Token<'a> {
    Open,
    Close,
    Word(&'a [u8]),
}

fn tokens(text: &[u8]) -> Option<Vec<Token<'_>>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = text.get(i) {
        match c {
            b'{' => {
                out.push(Token::Open);
                i += 1;
            }
            b'}' => {
                out.push(Token::Close);
                i += 1;
            }
            b'%' => {
                while text.get(i).is_some_and(|c| *c != b'\n' && *c != b'\r') {
                    i += 1;
                }
            }
            c if c.is_ascii_whitespace() || c == 0 => i += 1,
            _ => {
                let start = i;
                while text.get(i).is_some_and(|c| !c.is_ascii_whitespace() && !b"{}%".contains(c) && *c != 0) {
                    i += 1;
                }
                out.push(Token::Word(text.get(start..i)?));
            }
        }
        if out.len() > MAX_TOKENS {
            return None;
        }
    }
    Some(out)
}

fn parse_program(text: &[u8]) -> Option<Vec<Instr>> {
    let tokens = tokens(text)?;
    let mut at = 0;
    if tokens.first() != Some(&Token::Open) {
        return None;
    }
    at += 1;
    let program = parse_block(&tokens, &mut at, 0)?;
    // Nothing but white space after the program.
    (at == tokens.len()).then_some(program)
}

/// The procedure starting after an opening brace, up to and past its closing one.
fn parse_block(tokens: &[Token<'_>], at: &mut usize, depth: u8) -> Option<Vec<Instr>> {
    if depth > MAX_BLOCK_DEPTH {
        return None;
    }
    let mut out = Vec::new();
    let mut pending: Vec<Vec<Instr>> = Vec::new();
    loop {
        let token = tokens.get(*at)?;
        *at += 1;
        match token {
            Token::Close => return pending.is_empty().then_some(out),
            Token::Open => {
                if pending.len() >= 2 {
                    return None;
                }
                pending.push(parse_block(tokens, at, depth.saturating_add(1))?);
            }
            Token::Word(b"if") => {
                let [then] = <[Vec<Instr>; 1]>::try_from(std::mem::take(&mut pending)).ok()?;
                out.push(Instr::If(then));
            }
            Token::Word(b"ifelse") => {
                let [then, otherwise] = <[Vec<Instr>; 2]>::try_from(std::mem::take(&mut pending)).ok()?;
                out.push(Instr::IfElse(then, otherwise));
            }
            Token::Word(w) => {
                if !pending.is_empty() {
                    return None;
                }
                out.push(match operator(w) {
                    Some(i) => i,
                    None => Instr::Push(literal(w)?),
                });
            }
        }
    }
}

fn literal(word: &[u8]) -> Option<Val> {
    let text = std::str::from_utf8(word).ok()?;
    if let Ok(i) = text.parse::<i64>() {
        return Some(Val::Int(i));
    }
    // Reals are digits with a point or an exponent (no names that Rust also reads, like `inf`).
    if !text.bytes().all(|c| c.is_ascii_digit() || b"+-.eE".contains(&c)) {
        return None;
    }
    Val::real(text.parse::<f64>().ok()?)
}

fn calculate(program: &[Instr], x: &[f64], outputs: usize) -> Option<Vec<f64>> {
    let mut stack: Vec<Val> = x.iter().map(|v| Val::Real(*v)).collect();
    run(program, &mut stack)?;
    let start = stack.len().checked_sub(outputs)?;
    stack.get(start..)?.iter().map(|v| v.num()).collect()
}

fn run(program: &[Instr], stack: &mut Vec<Val>) -> Option<()> {
    for instr in program {
        match instr {
            Instr::Push(v) => stack.push(*v),
            Instr::Op(op) => apply(*op, stack)?,
            Instr::If(then) => {
                if let Val::Bool(b) = stack.pop()? {
                    if b {
                        run(then, stack)?;
                    }
                } else {
                    return None;
                }
            }
            Instr::IfElse(then, otherwise) => match stack.pop()? {
                Val::Bool(true) => run(then, stack)?,
                Val::Bool(false) => run(otherwise, stack)?,
                _ => return None,
            },
        }
        if stack.len() > MAX_STACK {
            return None;
        }
    }
    Some(())
}

fn apply(op: Op, stack: &mut Vec<Val>) -> Option<()> {
    let result = match op {
        Op::Dup | Op::Pop | Op::Exch | Op::Copy | Op::Index | Op::Roll => return stack_op(op, stack),
        Op::Eq | Op::Ne | Op::Ge | Op::Gt | Op::Le | Op::Lt | Op::And | Op::Or | Op::Xor | Op::Not | Op::Bitshift => logic(op, stack)?,
        Op::Sqrt | Op::Sin | Op::Cos | Op::Atan | Op::Exp | Op::Ln | Op::Log => math(op, stack)?,
        _ => arithmetic(op, stack)?,
    };
    stack.push(result);
    Some(())
}

fn one(stack: &mut Vec<Val>) -> Option<Val> {
    stack.pop()
}

fn two(stack: &mut Vec<Val>) -> Option<(Val, Val)> {
    let b = stack.pop()?;
    let a = stack.pop()?;
    Some((a, b))
}

fn int(v: Val) -> Option<i64> {
    match v {
        Val::Int(i) => Some(i),
        _ => None,
    }
}

/// `add` to `cvr`: integers stay integers while they fit.
fn arithmetic(op: Op, stack: &mut Vec<Val>) -> Option<Val> {
    use Val::{Bool, Int, Real};
    Some(match op {
        Op::Add | Op::Sub | Op::Mul => {
            let (a, b) = two(stack)?;
            match (a, b) {
                (Int(x), Int(y)) => {
                    let exact = match op {
                        Op::Add => x.checked_add(y),
                        Op::Sub => x.checked_sub(y),
                        _ => x.checked_mul(y),
                    };
                    match exact {
                        Some(v) => Int(v),
                        None => Val::real(arith(op, x as f64, y as f64))?,
                    }
                }
                (a, b) => Val::real(arith(op, a.num()?, b.num()?))?,
            }
        }
        Op::Div => {
            let (a, b) = two(stack)?;
            let d = b.num()?;
            if d == 0.0 {
                return None;
            }
            Val::real(a.num()? / d)?
        }
        Op::Idiv | Op::Mod => {
            let (a, b) = two(stack)?;
            let (x, y) = (int(a)?, int(b)?);
            Int(if op == Op::Idiv { x.checked_div(y)? } else { x.checked_rem(y)? })
        }
        Op::Abs | Op::Neg => match one(stack)? {
            Int(i) => match if op == Op::Abs { i.checked_abs() } else { i.checked_neg() } {
                Some(v) => Int(v),
                None => Val::real(-(i as f64))?,
            },
            Real(r) => Real(if op == Op::Abs { r.abs() } else { -r }),
            Bool(_) => return None,
        },
        Op::Ceiling | Op::Floor | Op::Round | Op::Truncate => match one(stack)? {
            Int(i) => Int(i),
            Real(r) => Real(match op {
                Op::Ceiling => r.ceil(),
                Op::Floor => r.floor(),
                // PostScript rounds halves up.
                Op::Round => (r + 0.5).floor(),
                _ => r.trunc(),
            }),
            Bool(_) => return None,
        },
        Op::Cvi => {
            let v = one(stack)?.num()?.trunc();
            if v < i64::MIN as f64 || v >= i64::MAX as f64 {
                return None;
            }
            Int(v as i64)
        }
        _ => Val::real(one(stack)?.num()?)?,
    })
}

/// `sqrt` to `log`: real results, angles in degrees.
fn math(op: Op, stack: &mut Vec<Val>) -> Option<Val> {
    match op {
        Op::Sqrt => {
            let v = one(stack)?.num()?;
            if v < 0.0 {
                return None;
            }
            Val::real(v.sqrt())
        }
        Op::Sin => Val::real(one(stack)?.num()?.to_radians().sin()),
        Op::Cos => Val::real(one(stack)?.num()?.to_radians().cos()),
        Op::Atan => {
            let (num, den) = two(stack)?;
            let (n, d) = (num.num()?, den.num()?);
            if n == 0.0 && d == 0.0 {
                return None;
            }
            let deg = n.atan2(d).to_degrees();
            Val::real(if deg < 0.0 { deg + 360.0 } else { deg })
        }
        Op::Exp => {
            let (base, exponent) = two(stack)?;
            Val::real(base.num()?.powf(exponent.num()?))
        }
        _ => {
            let v = one(stack)?.num()?;
            if v <= 0.0 {
                return None;
            }
            Val::real(if op == Op::Ln { v.ln() } else { v.log10() })
        }
    }
}

/// The relational, boolean and bitwise operators.
fn logic(op: Op, stack: &mut Vec<Val>) -> Option<Val> {
    use Val::{Bool, Int, Real};
    Some(match op {
        Op::Eq | Op::Ne => {
            let (a, b) = two(stack)?;
            let same = match (a, b) {
                (Bool(x), Bool(y)) => x == y,
                (Bool(_), _) | (_, Bool(_)) => false,
                (x, y) => x.num()? == y.num()?,
            };
            Bool(same == (op == Op::Eq))
        }
        Op::Ge | Op::Gt | Op::Le | Op::Lt => {
            let (a, b) = two(stack)?;
            let (x, y) = (a.num()?, b.num()?);
            Bool(match op {
                Op::Ge => x >= y,
                Op::Gt => x > y,
                Op::Le => x <= y,
                _ => x < y,
            })
        }
        Op::And | Op::Or | Op::Xor => match two(stack)? {
            (Bool(x), Bool(y)) => Bool(match op {
                Op::And => x & y,
                Op::Or => x | y,
                _ => x ^ y,
            }),
            (Int(x), Int(y)) => Int(match op {
                Op::And => x & y,
                Op::Or => x | y,
                _ => x ^ y,
            }),
            _ => return None,
        },
        Op::Not => match one(stack)? {
            Bool(b) => Bool(!b),
            Int(i) => Int(!i),
            Real(_) => return None,
        },
        _ => {
            let (a, b) = two(stack)?;
            let (x, shift) = (int(a)?, int(b)?);
            // Bits shifted out are lost, and those shifted in are zero.
            Int(if shift >= 0 {
                u32::try_from(shift).ok().and_then(|s| x.checked_shl(s)).unwrap_or(0)
            } else {
                u32::try_from(shift.unsigned_abs()).ok().and_then(|s| (x as u64).checked_shr(s)).map_or(0, |v| v as i64)
            })
        }
    })
}

/// The operators that move what is on the stack.
fn stack_op(op: Op, stack: &mut Vec<Val>) -> Option<()> {
    match op {
        Op::Dup => {
            let top = *stack.last()?;
            stack.push(top);
        }
        Op::Pop => {
            stack.pop()?;
        }
        Op::Exch => {
            let (a, b) = two(stack)?;
            stack.push(b);
            stack.push(a);
        }
        Op::Copy => {
            let n = usize::try_from(int(one(stack)?)?).ok()?;
            let start = stack.len().checked_sub(n)?;
            if stack.len().saturating_add(n) > MAX_STACK {
                return None;
            }
            let copied: Vec<Val> = stack.get(start..)?.to_vec();
            stack.extend(copied);
        }
        Op::Index => {
            let n = usize::try_from(int(one(stack)?)?).ok()?;
            let at = stack.len().checked_sub(n.checked_add(1)?)?;
            let v = *stack.get(at)?;
            stack.push(v);
        }
        _ => {
            let (n, j) = two(stack)?;
            let (n, j) = (usize::try_from(int(n)?).ok()?, int(j)?);
            let start = stack.len().checked_sub(n)?;
            if n > 0 {
                let by = usize::try_from(j.rem_euclid(i64::try_from(n).ok()?)).ok()?;
                stack.get_mut(start..)?.rotate_right(by);
            }
        }
    }
    Some(())
}

fn arith(op: Op, a: f64, b: f64) -> f64 {
    match op {
        Op::Add => a + b,
        Op::Sub => a - b,
        _ => a * b,
    }
}

#[cfg(test)]
mod tests;
