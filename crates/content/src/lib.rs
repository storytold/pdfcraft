//! Content streams (ISO 32000-2 §7.8.2, §8, §9): parse a stream into operators that keep their
//! source spans, write operators back, and the matrix arithmetic content interpretation needs.
//!
//! Layer L2. The parser is tolerant: bytes it cannot read are skipped (and counted in
//! [`Parsed::skipped`]) instead of failing the whole stream, and it never panics. Inline images
//! (`BI … ID … EI`) are kept whole, so a parse → serialize round trip preserves them.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Lexer, Object, PdfString, serialize};

#[cfg(test)]
mod tests;

/// One operator with its operands.
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    /// The operator keyword (`Tj`, `re`, `BI` for inline images…).
    pub op: Vec<u8>,
    pub operands: Vec<Object>,
    /// Inline images: the image dictionary (`operands` is empty) and the data between `ID` and
    /// `EI`.
    pub inline: Option<(Dict, Vec<u8>)>,
    /// The bytes of the source stream this operator came from (operands included).
    pub span: std::ops::Range<usize>,
}

impl Op {
    pub fn new(op: &str, operands: Vec<Object>) -> Self {
        Op { op: op.as_bytes().to_vec(), operands, inline: None, span: 0..0 }
    }

    pub fn is(&self, op: &str) -> bool {
        self.op == op.as_bytes()
    }

    /// Operand `i` as a number.
    pub fn num(&self, i: usize) -> Option<f64> {
        self.operands.get(i).and_then(Object::as_f64)
    }

    /// The last `n` operands as numbers (operators take their operands from the end).
    pub fn nums<const N: usize>(&self) -> Option<[f64; N]> {
        let k = self.operands.len().checked_sub(N)?;
        let mut out = [0.0; N];
        for (o, v) in out.iter_mut().zip(&self.operands[k..]) {
            *o = v.as_f64()?;
        }
        Some(out)
    }

    pub fn name(&self, i: usize) -> Option<&[u8]> {
        self.operands.get(i).and_then(Object::as_name)
    }
}

/// A parsed stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub ops: Vec<Op>,
    /// Bytes that could not be read as operands or operators.
    pub skipped: usize,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Parse a (decoded) content stream.
pub fn parse(data: &[u8]) -> Parsed {
    let mut out = Parsed::default();
    let mut lx = Lexer::new(data, 0);
    let mut operands: Vec<Object> = Vec::new();
    let mut start: Option<usize> = None;
    loop {
        lx.skip_ws();
        let Some(&b) = data.get(lx.pos) else { break };
        let at = lx.pos;
        let operand_start = matches!(b, b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.') || b.is_ascii_digit();
        if operand_start {
            match lx.object() {
                Ok(o) => {
                    start.get_or_insert(at);
                    operands.push(o);
                }
                Err(_) if b == b'(' => {
                    // An unterminated string runs to the end of the stream.
                    out.skipped += data.len() - at;
                    lx.pos = data.len();
                }
                Err(_) => {
                    // Skip the offending byte and carry on.
                    lx.pos = at + 1;
                    out.skipped += 1;
                }
            }
            continue;
        }
        if is_delim(b) {
            // A stray `)`, `>`, `]`, `{`, `}`.
            lx.pos += 1;
            out.skipped += 1;
            continue;
        }
        let kw = lx.token();
        if kw.is_empty() {
            lx.pos = at + 1;
            out.skipped += 1;
            continue;
        }
        match kw {
            b"true" => {
                start.get_or_insert(at);
                operands.push(Object::Bool(true));
            }
            b"false" => {
                start.get_or_insert(at);
                operands.push(Object::Bool(false));
            }
            b"null" => {
                start.get_or_insert(at);
                operands.push(Object::Null);
            }
            b"BI" => {
                let (dict, img, end) = inline_image(data, lx.pos);
                lx.pos = end;
                out.ops.push(Op { op: b"BI".to_vec(), operands: Vec::new(), inline: Some((dict, img)), span: start.take().unwrap_or(at)..end });
                operands.clear();
            }
            _ => {
                let s = start.take().unwrap_or(at);
                out.ops.push(Op { op: kw.to_vec(), operands: std::mem::take(&mut operands), inline: None, span: s..lx.pos });
            }
        }
    }
    out.skipped += operands.len();
    out
}

/// Read an inline image starting after `BI`: (parameters, data, position after `EI`).
fn inline_image(data: &[u8], pos: usize) -> (Dict, Vec<u8>, usize) {
    let mut lx = Lexer::new(data, pos);
    let mut dict = Dict::new();
    loop {
        lx.skip_ws();
        if lx.eat_keyword(b"ID") {
            break;
        }
        let Ok(Object::Name(k)) = lx.object() else {
            return (dict, Vec::new(), data.len());
        };
        let Ok(v) = lx.object() else {
            return (dict, Vec::new(), data.len());
        };
        dict.set(k, v);
    }
    // One whitespace byte separates ID from the data.
    let begin = (lx.pos + 1).min(data.len());
    // The data ends at whitespace + `EI` + (whitespace or end). Search for that pattern.
    let mut i = begin;
    while i + 2 <= data.len() {
        if data[i] == b'E'
            && data.get(i + 1) == Some(&b'I')
            && (i == begin || is_ws(data[i - 1]))
            && data.get(i + 2).is_none_or(|b| is_ws(*b) || is_delim(*b))
        {
            let end_data = if i > begin && is_ws(data[i - 1]) { i - 1 } else { i };
            return (dict, data[begin..end_data].to_vec(), i + 2);
        }
        i += 1;
    }
    (dict, data[begin..].to_vec(), data.len())
}

/// Write one operator (with a trailing newline).
pub fn write_op(op: &Op, out: &mut Vec<u8>) {
    if let Some((dict, img)) = &op.inline {
        out.extend_from_slice(b"BI");
        for (k, v) in dict.iter() {
            out.push(b' ');
            serialize(&Object::Name(k.clone()), out);
            out.push(b' ');
            serialize(v, out);
        }
        out.extend_from_slice(b" ID ");
        out.extend_from_slice(img);
        out.extend_from_slice(b"\nEI\n");
        return;
    }
    for o in &op.operands {
        serialize(o, out);
        out.push(b' ');
    }
    out.extend_from_slice(&op.op);
    out.push(b'\n');
}

/// Write operators back to a content stream.
pub fn serialize_ops(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        write_op(op, &mut out);
    }
    out
}

/// A number operand, written as an integer when it is one.
pub fn num(v: f64) -> Object {
    if v.fract() == 0.0 && v.abs() < 1e15 { Object::Int(v as i64) } else { Object::Real((v * 10_000.0).round() / 10_000.0) }
}

pub fn string(bytes: Vec<u8>) -> Object {
    Object::String(PdfString::literal(bytes))
}

/// An affine matrix `[a b c d e f]` (PDF order: a point maps to `(a·x + c·y + e, b·x + d·y + f)`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix(pub [f64; 6]);

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(x: f64, y: f64) -> Self {
        Matrix([1.0, 0.0, 0.0, 1.0, x, y])
    }

    pub fn from_operands(v: &[Object]) -> Option<Self> {
        if v.len() != 6 {
            return None;
        }
        let mut m = [0.0; 6];
        for (o, x) in m.iter_mut().zip(v) {
            *o = x.as_f64()?;
        }
        Some(Matrix(m))
    }

    /// `self` then `then` (`self × then` in PDF's row-vector convention).
    pub fn then(&self, then: &Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = then.0;
        Matrix([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    pub fn invert(&self) -> Option<Matrix> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Some(Matrix([ia, ib, ic, id, -(e * ia + f * ic), -(e * ib + f * id)]))
    }

    /// The bounding box of a rectangle `[x0 y0 x1 y1]` after this transform.
    pub fn bbox(&self, r: [f64; 4]) -> [f64; 4] {
        let pts = [self.apply(r[0], r[1]), self.apply(r[2], r[1]), self.apply(r[0], r[3]), self.apply(r[2], r[3])];
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (x, y) in pts {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        b
    }
}

/// Do two rectangles overlap by more than `eps` in both directions?
pub fn overlaps(a: [f64; 4], b: [f64; 4], eps: f64) -> bool {
    a[0].max(b[0]) + eps < a[2].min(b[2]) && a[1].max(b[1]) + eps < a[3].min(b[3])
}

/// Is `inner` inside `outer` (within `eps`)?
pub fn contains(outer: [f64; 4], inner: [f64; 4], eps: f64) -> bool {
    inner[0] >= outer[0] - eps && inner[1] >= outer[1] - eps && inner[2] <= outer[2] + eps && inner[3] <= outer[3] + eps
}
