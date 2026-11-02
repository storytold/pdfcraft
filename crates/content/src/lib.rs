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
    /// For [`Pieces::parse`]: per piece, whether a comment runs into it from the piece before
    /// and whether one runs out of it into the next. Only boundaries are recorded, so the size
    /// depends on the number of pieces, not of comments.
    comment_edges: Vec<(bool, bool)>,
}

fn is_ws(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Parse a (decoded) content stream.
pub fn parse(data: &[u8]) -> Parsed {
    parse_pieces(data, &[])
}

/// Parse `data`, recording which of the piece boundaries `ends` (as in [`Pieces`]) a comment
/// crosses.
fn parse_pieces(data: &[u8], ends: &[usize]) -> Parsed {
    let mut out = Parsed { comment_edges: vec![(false, false); ends.len()], ..Parsed::default() };
    let mut boundary = 0;
    let mut lx = Lexer::new(data, 0);
    let mut operands: Vec<Object> = Vec::new();
    let mut start: Option<usize> = None;
    loop {
        let trivia = lx.pos;
        lx.skip_ws();
        // Only the whitespace the lexer skips here holds top-level comments: a `%` inside a
        // string, a name, an array or dictionary operand or inline image data is not one.
        if boundary < ends.len() {
            comment_edges(data, trivia..lx.pos, ends, &mut boundary, &mut out.comment_edges);
        }
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
            b"BI" => match inline_image(data, lx.pos) {
                Ok((dict, img, end)) => {
                    lx.pos = end;
                    out.ops.push(Op { op: b"BI".to_vec(), operands: Vec::new(), inline: Some((dict, img)), span: start.take().unwrap_or(at)..end });
                    operands.clear();
                }
                Err(end) => {
                    out.skipped += end.saturating_sub(at);
                    lx.pos = end;
                    start = None;
                    operands.clear();
                }
            },
            _ => {
                let s = start.take().unwrap_or(at);
                out.ops.push(Op { op: kw.to_vec(), operands: std::mem::take(&mut operands), inline: None, span: s..lx.pos });
            }
        }
    }
    out.skipped += operands.len();
    out
}

/// Record the piece boundaries crossed by the comments in `trivia` (skipped whitespace and
/// comments). `boundary` is the first boundary not yet passed; boundaries only move forward.
fn comment_edges(data: &[u8], trivia: std::ops::Range<usize>, ends: &[usize], boundary: &mut usize, edges: &mut [(bool, bool)]) {
    let mut at = trivia.start;
    while at < trivia.end {
        if data.get(at) != Some(&b'%') {
            at += 1;
            continue;
        }
        let comment = at;
        while at < trivia.end && !matches!(data.get(at), Some(b'\r' | b'\n')) {
            at += 1;
        }
        // The separator of piece `boundary` is the last byte before its end.
        while let Some(&end) = ends.get(*boundary) {
            let separator = end.saturating_sub(1);
            if separator >= at {
                break;
            }
            if separator >= comment {
                if let Some(edge) = edges.get_mut(*boundary) {
                    edge.1 = true;
                }
                // Includes a comment whose end of line is the next piece's first byte.
                if let Some(edge) = edges.get_mut(boundary.saturating_add(1)) {
                    edge.0 = true;
                }
            }
            *boundary += 1;
        }
    }
}

/// Read an inline image starting after `BI`: (parameters, data, position after `EI`). `Err` holds
/// the position after the `EI` (or the end of the stream) of an image whose parameters can't be read.
fn inline_image(data: &[u8], pos: usize) -> Result<(Dict, Vec<u8>, usize), usize> {
    let mut lx = Lexer::new(data, pos);
    let mut dict = Dict::new();
    loop {
        lx.skip_ws();
        if lx.eat_keyword(b"ID") {
            break;
        }
        let at = lx.pos;
        let entry = match lx.object() {
            Ok(Object::Name(k)) => lx.object().ok().map(|v| (k, v)),
            _ => None,
        };
        let Some((k, v)) = entry else {
            // A stream that ends inside the parameters is cut short, not unreadable: keep what was read.
            if at >= data.len() {
                return Ok((dict, Vec::new(), data.len()));
            }
            // Without an `EI` after it, the image runs to the end of the stream.
            return Err(end_of_image(data, at).map_or(data.len(), |(_, end)| end));
        };
        dict.set(k, v);
    }
    // One whitespace byte separates ID from the data.
    let begin = (lx.pos + 1).min(data.len());
    let end = unfiltered_image_end(data, begin, &dict).or_else(|| end_of_image(data, begin));
    Ok(match end {
        Some((end_data, end)) => (dict, data[begin..end_data].to_vec(), end),
        None => (dict, data[begin..].to_vec(), data.len()),
    })
}

/// An unfiltered image's samples determine its length, including byte padding per row.
/// Binary samples may themselves contain whitespace + `EI` + whitespace, so use the known
/// length when the actual terminator follows it. Unknown or damaged layouts keep the heuristic.
fn unfiltered_image_end(data: &[u8], begin: usize, dict: &Dict) -> Option<(usize, usize)> {
    for key in [b"Filter".as_slice(), b"F"] {
        if let Some(filter) = dict.get(key)
            && !matches!(filter, Object::Null)
            && !matches!(filter, Object::Array(filters) if filters.is_empty())
        {
            return None;
        }
    }
    for (full, short) in
        [(b"Width".as_slice(), b"W".as_slice()), (b"Height", b"H"), (b"ColorSpace", b"CS"), (b"BitsPerComponent", b"BPC"), (b"ImageMask", b"IM")]
    {
        if let (Some(full), Some(short)) = (dict.get(full), dict.get(short))
            && full != short
        {
            return None;
        }
    }
    let get = |full: &[u8], short: &[u8]| dict.get(full).or_else(|| dict.get(short));
    let width = usize::try_from(get(b"Width", b"W")?.as_int()?).ok().filter(|n| *n > 0)?;
    let height = usize::try_from(get(b"Height", b"H")?.as_int()?).ok().filter(|n| *n > 0)?;
    let mask = match get(b"ImageMask", b"IM") {
        None | Some(Object::Null | Object::Bool(false)) => false,
        Some(Object::Bool(true)) => true,
        _ => return None,
    };
    let bits = match get(b"BitsPerComponent", b"BPC") {
        None | Some(Object::Null) => None,
        Some(value) => Some(value.as_int()?),
    };
    let (components, bits) = if mask {
        if bits.is_some_and(|bits| bits != 1) {
            return None;
        }
        (1usize, 1usize)
    } else {
        let (components, max_bits) = match get(b"ColorSpace", b"CS")? {
            Object::Name(name) => match name.as_slice() {
                b"DeviceGray" | b"G" => (1, 16),
                b"DeviceRGB" | b"RGB" => (3, 16),
                b"DeviceCMYK" | b"CMYK" => (4, 16),
                _ => return None,
            },
            Object::Array(space) if space.len() == 4 && matches!(space.first().and_then(Object::as_name), Some(b"Indexed" | b"I")) => (1, 8),
            _ => return None,
        };
        let bits = match bits? {
            bits @ (1 | 2 | 4 | 8 | 16) => bits as usize,
            _ => return None,
        };
        if bits > max_bits {
            return None;
        }
        (components, bits)
    };
    let row = width.checked_mul(components)?.checked_mul(bits)?.div_ceil(8);
    let end_data = begin.checked_add(row.checked_mul(height)?)?;
    let after = data.get(end_data..)?;
    if !after.first().is_some_and(|b| is_ws(*b)) {
        return None;
    }
    let separator = after.iter().position(|b| !is_ws(*b))?;
    let ei = end_data.checked_add(separator)?;
    let end = ei.checked_add(2)?;
    if data.get(ei..end) == Some(b"EI") && data.get(end).is_none_or(|b| is_ws(*b) || is_delim(*b)) { Some((end_data, end)) } else { None }
}

/// Where inline image data starting at `begin` ends: (end of the data, position after `EI`). The
/// data ends at whitespace + `EI` + (whitespace, a delimiter or the end).
fn end_of_image(data: &[u8], begin: usize) -> Option<(usize, usize)> {
    let mut i = begin;
    while i + 2 <= data.len() {
        if data[i] == b'E'
            && data.get(i + 1) == Some(&b'I')
            && (i == begin || is_ws(data[i - 1]))
            && data.get(i + 2).is_none_or(|b| is_ws(*b) || is_delim(*b))
        {
            let end_data = if i > begin && is_ws(data[i - 1]) { i - 1 } else { i };
            return Some((end_data, i + 2));
        }
        i += 1;
    }
    None
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

/// A page's content streams joined into the one stream they are. Writers may split it between
/// any two tokens (ISO 32000-2 §7.8.2), so an operator's operands can end one piece and its
/// keyword start the next: parse the pieces joined ([`Pieces::parse`]) and map positions back
/// with [`Pieces::piece_of`]. A space separates the pieces, as renderers join them (hayro,
/// MuPDF and Poppler alike): tokens never run together, and a comment at the end of a piece
/// runs on into the next one, up to its first end of line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pieces {
    data: Vec<u8>,
    /// Where each piece ends in `data`, after its separator.
    ends: Vec<usize>,
}

impl Pieces {
    pub fn join<D: AsRef<[u8]>>(pieces: &[D]) -> Self {
        let mut data = Vec::new();
        let mut ends = Vec::with_capacity(pieces.len());
        for p in pieces {
            data.extend_from_slice(p.as_ref());
            data.push(b' ');
            ends.push(data.len());
        }
        Pieces { data, ends }
    }

    /// The joined bytes (the spans of [`Pieces::parse`]'s operators index into them).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// How many pieces there are.
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Parse the pieces as one stream.
    pub fn parse(&self) -> Parsed {
        parse_pieces(&self.data, &self.ends)
    }

    /// Per piece, from `parsed` (this piece set's [`Pieces::parse`]): whether a comment runs
    /// into it from the piece before, and whether one runs out of it into the next. A writer
    /// that rewrites a piece without its comments must keep both: end the incoming comment
    /// before its first operator, and leave a comment open at its end.
    pub fn comment_edges(&self, parsed: &Parsed) -> Vec<(bool, bool)> {
        let mut edges = parsed.comment_edges.clone();
        edges.resize(self.len(), (false, false));
        edges
    }

    /// The piece holding joined position `at` (the last piece for positions past the end).
    pub fn piece_of(&self, at: usize) -> usize {
        self.ends.partition_point(|&e| e <= at).min(self.ends.len().saturating_sub(1))
    }

    /// The pieces an operator spans: (the one its first token is in, the one its keyword is in).
    pub fn pieces_of(&self, op: &Op) -> (usize, usize) {
        (self.piece_of(op.span.start), self.piece_of(op.span.end.saturating_sub(1)))
    }

    /// The joined range of piece `i`, without its separator.
    fn range(&self, i: usize) -> std::ops::Range<usize> {
        let start = match i.checked_sub(1) {
            None => 0,
            Some(p) => self.ends.get(p).copied().unwrap_or(self.data.len()),
        };
        let end = self.ends.get(i).map_or(self.data.len(), |e| e.saturating_sub(1)).max(start);
        start..end
    }

    /// Rebuild every piece after replacing joined byte ranges: `edits` are (range, bytes) in
    /// ascending, non-overlapping order. A replacement goes into the piece its range starts in,
    /// and the rest of a range that runs on into later pieces is removed from them, so an
    /// operator split across pieces is replaced or removed whole. Everything else is copied byte
    /// for byte. Returns every piece's new bytes (unchanged pieces come back equal).
    pub fn splice(&self, edits: impl IntoIterator<Item = (std::ops::Range<usize>, Vec<u8>)>) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = (0..self.len()).map(|i| Vec::with_capacity(self.range(i).len())).collect();
        let copy = |out: &mut Vec<Vec<u8>>, from: usize, to: usize| {
            for (i, piece) in out.iter_mut().enumerate() {
                let r = self.range(i);
                let (a, b) = (from.max(r.start), to.min(r.end));
                if a < b {
                    piece.extend_from_slice(self.data.get(a..b).unwrap_or_default());
                }
            }
        };
        let mut at = 0;
        for (range, bytes) in edits {
            let start = range.start.clamp(at, self.data.len());
            copy(&mut out, at, start);
            if !bytes.is_empty()
                && let Some(piece) = out.get_mut(self.piece_of(start))
            {
                piece.extend_from_slice(&bytes);
            }
            at = range.end.clamp(start, self.data.len());
        }
        copy(&mut out, at, self.data.len());
        out
    }
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
