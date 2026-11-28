//! Lossless content-stream compaction: page contents, form XObjects and tiling patterns are
//! written again with the fewest separators, numbers in
//! their shortest exact form and no comments, without empty `q Q` pairs, identity `cm`s, or
//! settings that repeat the value already in force. Every operator kept, and every operand by
//! value, is the same when the result is read back; anything this can't be sure of leaves the
//! stream as it is.

use std::collections::BTreeMap;

use pdfcraft_content::{Op, ops as read};
use pdfcraft_cos::{Document, ImageBudget, MAX_PAGE_DECODED, ObjRef, Object, Stream, serialize};
use pdfcraft_filters::{Filter, Params};

use crate::{OptimizeError, Report, Settings, Stage, as_stored, stored_len};

/// Steps of a run's allowance one decoded byte costs: read, written, read back and compressed.
const WORK_PER_BYTE: u64 = 16;
/// Steps one byte costs to compress both the original and the compacted copy, to compare them as stored.
const ENCODE_WORK: u64 = 8;
/// Deepest `q` nesting tracked; a stream nested deeper is left as it is.
const MAX_SAVE_DEPTH: usize = 1024;
/// Deepest array or dictionary operand written.
const MAX_OPERAND_DEPTH: u8 = 32;
/// Most operators, and most operands, read from one stream; a stream with more is left as it is.
const MAX_OPS: usize = 2_000_000;

/// The operators of ISO 32000-2 Annex A. A stream with any other is left as it is.
const KNOWN: &[&[u8]] = &[
    b"b", b"B", b"b*", b"B*", b"BDC", b"BI", b"BMC", b"BT", b"BX", b"c", b"cm", b"CS", b"cs", b"d", b"d0", b"d1", b"Do", b"DP", b"EI", b"EMC", b"ET",
    b"EX", b"f", b"F", b"f*", b"G", b"g", b"gs", b"h", b"i", b"ID", b"j", b"J", b"K", b"k", b"l", b"m", b"M", b"MP", b"n", b"q", b"Q", b"re", b"RG",
    b"rg", b"ri", b"s", b"S", b"SC", b"sc", b"SCN", b"scn", b"sh", b"T*", b"Tc", b"Td", b"TD", b"Tf", b"Tj", b"TJ", b"TL", b"Tm", b"Tr", b"Ts",
    b"Tw", b"Tz", b"v", b"w", b"W", b"W*", b"y", b"'", b"\"",
];

/// Operators that change none of the settings tracked: painting, paths, clipping, text
/// positioning and showing, marked content, and XObjects and shadings, which run in a state of
/// their own. `TD` also sets the leading, so it clears that slot instead (Table 106).
const NEUTRAL: &[&[u8]] = &[
    b"m", b"l", b"c", b"v", b"y", b"h", b"re", b"S", b"s", b"f", b"F", b"f*", b"B", b"B*", b"b", b"b*", b"n", b"W", b"W*", b"BT", b"ET", b"Td",
    b"Tm", b"T*", b"Tj", b"TJ", b"'", b"BMC", b"BDC", b"EMC", b"MP", b"DP", b"Do", b"sh", b"BX", b"EX", b"cm",
];

/// Settings whose operator alone sets one value of the graphics state.
const SETTERS: &[&[u8]] = &[b"w", b"J", b"j", b"M", b"d", b"ri", b"i", b"Tc", b"Tw", b"Tz", b"TL", b"Ts", b"Tr", b"Tf"];
const FILL: &[&[u8]] = &[b"g", b"rg", b"k", b"cs", b"sc", b"scn"];
const STROKE: &[&[u8]] = &[b"G", b"RG", b"K", b"CS", b"SC", b"SCN"];
const FILL_SLOT: usize = SETTERS.len();
const STROKE_SLOT: usize = SETTERS.len() + 1;
const SLOTS: usize = SETTERS.len() + 2;

/// What compacting found, counted in the report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub ops_removed: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

/// One stream's result.
enum Outcome {
    /// The stream couldn't be read for certain: its filters, damage, or an inline image whose
    /// end is a guess. A neighbouring piece of the same page contents may then start inside an
    /// operator, so none of that page's pieces is rewritten.
    Unreadable,
    /// Read, but left as it is: nothing to gain, or something this doesn't rewrite.
    Kept,
    Compacted(Stream, Counts),
}

/// Compact every page content stream, form XObject and tiling pattern of `doc`, when
/// [`Settings::optimize_content`] asks for it.
pub(crate) fn run(
    doc: &mut Document,
    pages: &[ObjRef],
    settings: &Settings,
    report: &mut Report,
    progress: &mut dyn FnMut(Stage) -> bool,
    budget: &mut ImageBudget,
) -> Result<(), OptimizeError> {
    if !settings.optimize_content {
        return Ok(());
    }
    let units = units(doc, pages);
    let mut done: BTreeMap<ObjRef, Outcome> = BTreeMap::new();
    let mut vetoed = Vec::new();
    for unit in &units {
        for r in unit {
            if done.contains_key(r) {
                continue;
            }
            if !progress(Stage::CleanUp) {
                return Err(OptimizeError::Cancelled);
            }
            let outcome = match &*doc.get(*r) {
                Object::Stream(s) => compact(s, settings, budget),
                _ => Outcome::Unreadable,
            };
            done.insert(*r, outcome);
        }
        if unit.len() > 1 && unit.iter().any(|r| matches!(done.get(r), Some(Outcome::Unreadable))) {
            vetoed.extend(unit.iter().copied());
        }
    }
    for r in vetoed {
        done.remove(&r);
    }
    for (r, outcome) in done {
        if let Outcome::Compacted(s, c) = outcome {
            doc.set(r, Object::Stream(s));
            report.content_streams = report.content_streams.saturating_add(1);
            report.content_ops_removed = report.content_ops_removed.saturating_add(c.ops_removed);
            report.content_bytes_before = report.content_bytes_before.saturating_add(c.bytes_before);
            report.content_bytes_after = report.content_bytes_after.saturating_add(c.bytes_after);
        }
    }
    Ok(())
}

/// The streams to compact, grouped as they are read: each page's contents (one stream, or the
/// pieces of an array, which are read as one stream, §7.8.2), then each form XObject and tiling
/// pattern on its own.
fn units(doc: &Document, pages: &[ObjRef]) -> Vec<Vec<ObjRef>> {
    let mut units = Vec::new();
    for p in pages {
        let page = doc.get(*p);
        let refs = match page.as_dict().and_then(|d| d.get(b"Contents")) {
            Some(Object::Ref(r)) => match &*doc.get(*r) {
                Object::Stream(_) => vec![*r],
                Object::Array(items) => items.iter().filter_map(Object::as_ref).collect(),
                _ => Vec::new(),
            },
            Some(Object::Array(items)) => items.iter().filter_map(Object::as_ref).collect(),
            _ => Vec::new(),
        };
        if !refs.is_empty() {
            units.push(refs);
        }
    }
    for num in doc.object_numbers() {
        let r = ObjRef { num, generation: doc.generation(num) };
        let drawn = matches!(&*doc.get(r), Object::Stream(s)
            if s.dict.name(b"Subtype") == Some(b"Form") || s.dict.get(b"PatternType") == Some(&Object::Int(1)));
        if drawn {
            units.push(vec![r]);
        }
    }
    units
}

/// `s` compacted, if that is certain to draw the same and is smaller as the later passes store
/// both ([`as_stored`]): an unfiltered stream is compared compressed when they compress it, as
/// fewer separators and shorter numbers can compress worse.
fn compact(s: &Stream, settings: &Settings, budget: &mut ImageBudget) -> Outcome {
    let Some(chain) = rewritable_filters(s) else { return Outcome::Unreadable };
    let Ok(data) = budget.decode_chain(&chain, &s.raw, MAX_PAGE_DECODED) else { return Outcome::Unreadable };
    let per_byte = if chain.is_empty() && settings.flate_unencoded { WORK_PER_BYTE + ENCODE_WORK } else { WORK_PER_BYTE };
    if !budget.spend(u64::try_from(data.len()).unwrap_or(u64::MAX).saturating_mul(per_byte)) {
        return Outcome::Unreadable;
    }
    let ops = match read_all(&data, MAX_OPS) {
        Ok(ops) => ops,
        Err(outcome) => return outcome,
    };
    if !rewritable(&ops) || has_odd_number(&data) {
        return Outcome::Kept;
    }
    let Some(keep) = redundant(&ops) else { return Outcome::Kept };
    let kept: Vec<&Op> = ops.iter().zip(&keep).filter(|(_, k)| **k).map(|(op, _)| op).collect();
    let Some(out) = write(&kept, data.first().is_some_and(|b| is_ws(*b))) else { return Outcome::Kept };
    if !reads_back(&out, &kept) {
        return Outcome::Kept;
    }
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    // An unfiltered result stays unfiltered: the later pass compresses it to the bytes compared.
    let new = if chain.is_empty() { Stream::from_raw(dict, out.clone()) } else { Stream::flate(dict, &out) };
    let mut stored = |version: &Stream| match as_stored(version, settings, budget) {
        Some(later) => stored_len(&later),
        None => stored_len(version),
    };
    match (stored(&new), stored(s)) {
        (Some(after), Some(before)) if after < before => {
            let counts = Counts { ops_removed: ops.len().saturating_sub(kept.len()), bytes_before: data.len(), bytes_after: out.len() };
            Outcome::Compacted(new, counts)
        }
        _ => Outcome::Kept,
    }
}

/// Every operator of `data`, unless some bytes can't be read for certain ([`Outcome::Unreadable`])
/// or there are more than `max` operators or operands ([`Outcome::Kept`]). The parser holds
/// operands until their operator comes, so the tokens are counted before anything is read.
fn read_all(data: &[u8], max: usize) -> Result<Vec<Op>, Outcome> {
    if !tokens_at_most(data, max.saturating_mul(2)) {
        return Err(Outcome::Kept);
    }
    let mut reader = read(data);
    let mut ops = Vec::new();
    let mut operands = 0usize;
    for op in reader.by_ref() {
        operands = operands.saturating_add(op.operands.len());
        if ops.len() >= max || operands > max {
            return Err(Outcome::Kept);
        }
        ops.push(op);
    }
    if reader.skipped() > 0 || reader.ambiguous_images() > 0 {
        return Err(Outcome::Unreadable);
    }
    Ok(ops)
}

/// Whether `data` has at most `limit` tokens, counting each run of regular characters and each
/// delimiter as one. That overcounts (inside strings, and `<<` as two), never undercounts.
fn tokens_at_most(data: &[u8], limit: usize) -> bool {
    let mut count = 0usize;
    let mut prev_regular = false;
    for &b in data {
        let regular = is_regular(b);
        if is_delim(b) || (regular && !prev_regular) {
            count = count.saturating_add(1);
            if count > limit {
                return false;
            }
        }
        prev_regular = regular;
    }
    true
}

/// The filters of `s`, when its data is in the file, only general-purpose filters with direct
/// integer parameters encode it, and it is not a stream the writer makes or a reader reads as
/// anything but content.
fn rewritable_filters(s: &Stream) -> Option<Vec<(Filter, Params)>> {
    if s.dict.contains(b"F")
        || matches!(s.dict.name(b"Type"), Some(b"XRef" | b"ObjStm" | b"Metadata" | b"EmbeddedFile"))
        || s.dict.name(b"Subtype") == Some(b"Image")
    {
        return None;
    }
    let names: Vec<&[u8]> = match s.dict.get(b"Filter") {
        None | Some(Object::Null) => Vec::new(),
        Some(Object::Name(n)) => vec![n.as_slice()],
        Some(Object::Array(a)) => a.iter().map(Object::as_name).collect::<Option<_>>()?,
        Some(_) => return None,
    };
    let general = |n: &&[u8]| matches!(Filter::from_name(n), Filter::Flate | Filter::Lzw | Filter::AsciiHex | Filter::Ascii85 | Filter::RunLength);
    if !names.iter().all(general) {
        return None;
    }
    let direct = |p: &Object| match p {
        Object::Null => true,
        Object::Dict(d) => d.iter().all(|(_, v)| matches!(v, Object::Int(_))),
        _ => false,
    };
    let parms_ok = match s.dict.get(b"DecodeParms") {
        None | Some(Object::Null) => true,
        Some(d @ Object::Dict(_)) => names.len() == 1 && direct(d),
        Some(Object::Array(a)) => a.len() == names.len() && a.iter().all(direct),
        Some(_) => false,
    };
    let chain = s.filters();
    (parms_ok && chain.len() == names.len()).then_some(chain)
}

/// Whether every operator is one this knows and writes: no inline image or a part of one, no
/// unknown operator, no reference among the operands, and `q` and `Q` without operands.
fn rewritable(ops: &[Op]) -> bool {
    ops.iter().all(|op| {
        op.inline.is_none()
            && !matches!(op.op.as_slice(), b"BI" | b"ID" | b"EI")
            && KNOWN.contains(&op.op.as_slice())
            && !(matches!(op.op.as_slice(), b"q" | b"Q") && !op.operands.is_empty())
            && op.operands.iter().all(|o| !matches!(o, Object::Ref(_) | Object::Stream(_)))
    })
}

/// Whether `data` holds a token that starts like a number but isn't one by §7.3.3 (`-`, `1.2.3`,
/// `1e5`, `.`, `--5`): the parser reads those leniently, so writing them as it read them would
/// change what another reader sees, and reading the result back can't tell. Comments, strings
/// and names are skipped.
fn has_odd_number(data: &[u8]) -> bool {
    let mut i = 0;
    while let Some(&b) = data.get(i) {
        let rest = data.get(i..).unwrap_or_default();
        let len = match b {
            b'%' => rest.iter().position(|c| matches!(c, b'\r' | b'\n')).unwrap_or(rest.len()),
            b'(' => literal_len(rest),
            b'<' if rest.get(1) == Some(&b'<') => 2,
            b'<' => rest.iter().position(|c| *c == b'>').map_or(rest.len(), |p| p.saturating_add(1)),
            b'/' => rest.iter().skip(1).take_while(|c| is_regular(**c)).count().saturating_add(1),
            _ if is_regular(b) => {
                let token = rest.iter().take_while(|c| is_regular(**c)).count();
                if matches!(b, b'+' | b'-' | b'.' | b'0'..=b'9') && !is_number(rest.get(..token).unwrap_or_default()) {
                    return true;
                }
                token
            }
            _ => 1,
        };
        i = i.saturating_add(len.max(1));
    }
    false
}

/// The length of the literal string `s` starts with, balanced parentheses and escapes included.
fn literal_len(s: &[u8]) -> usize {
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(&c) = s.get(i) {
        match c {
            b'\\' => i = i.saturating_add(1),
            b'(' => depth = depth.saturating_add(1),
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i.saturating_add(1);
                }
            }
            _ => {}
        }
        i = i.saturating_add(1);
    }
    s.len()
}

/// Whether `t` is an integer or real as §7.3.3 writes them: a sign at most, digits, and at most
/// one period, with a digit somewhere.
fn is_number(t: &[u8]) -> bool {
    let body = t.strip_prefix(b"+").or_else(|| t.strip_prefix(b"-")).unwrap_or(t);
    body.iter().any(u8::is_ascii_digit) && body.iter().all(|c| c.is_ascii_digit() || *c == b'.') && body.iter().filter(|c| **c == b'.').count() <= 1
}

/// Which operators to keep, or `None` when `q` nests deeper than this tracks.
fn redundant(ops: &[Op]) -> Option<Vec<bool>> {
    let mut keep = vec![true; ops.len()];
    let mut state: Vec<Option<&Op>> = vec![None; SLOTS];
    let mut saved: Vec<Vec<Option<&Op>>> = Vec::new();
    // The bare `q`s still open, and whether anything kept has come after each.
    let mut open: Vec<(usize, bool)> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" => {
                if saved.len() >= MAX_SAVE_DEPTH {
                    return None;
                }
                saved.push(state.clone());
                open.push((i, false));
                continue;
            }
            b"Q" => {
                state = saved.pop().unwrap_or_else(|| vec![None; SLOTS]);
                match open.pop() {
                    Some((q, false)) => {
                        for k in [q, i] {
                            if let Some(slot) = keep.get_mut(k) {
                                *slot = false;
                            }
                        }
                    }
                    Some(_) => mark(&mut open),
                    None => {}
                }
                continue;
            }
            b"cm" if is_identity(op) => {
                if let Some(slot) = keep.get_mut(i) {
                    *slot = false;
                }
                continue;
            }
            _ => {}
        }
        if let Some(slot) = slot_of(&op.op) {
            if state.get(slot).copied().flatten().is_some_and(|held| held.op == op.op && same_all(&held.operands, &op.operands)) {
                if let Some(k) = keep.get_mut(i) {
                    *k = false;
                }
                continue;
            }
            if let Some(s) = state.get_mut(slot) {
                *s = Some(op);
            }
        } else if op.op == b"TD" {
            if let Some(s) = SETTERS.iter().position(|o| *o == b"TL").and_then(|tl| state.get_mut(tl)) {
                *s = None;
            }
        } else if !NEUTRAL.contains(&op.op.as_slice()) {
            state.iter_mut().for_each(|s| *s = None);
        }
        mark(&mut open);
    }
    Some(keep)
}

/// Something kept came after the innermost open `q`.
fn mark(open: &mut [(usize, bool)]) {
    if let Some(top) = open.last_mut() {
        top.1 = true;
    }
}

fn slot_of(op: &[u8]) -> Option<usize> {
    if let Some(i) = SETTERS.iter().position(|o| *o == op) {
        Some(i)
    } else if FILL.contains(&op) {
        Some(FILL_SLOT)
    } else if STROKE.contains(&op) {
        Some(STROKE_SLOT)
    } else {
        None
    }
}

fn is_identity(op: &Op) -> bool {
    op.nums::<6>().is_some_and(|m| m == [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]) && op.operands.len() == 6
}

/// `ops` written with a separator only where two tokens would otherwise run together. A first
/// line break is kept when `lead` (the stream began with white space) and a last one always: the
/// pieces of page contents are read as one stream.
fn write(ops: &[&Op], lead: bool) -> Option<Vec<u8>> {
    let mut out = Out { buf: Vec::new(), after_op: false };
    if lead {
        out.buf.push(b'\n');
    }
    for op in ops {
        for o in &op.operands {
            out.object(o, 0)?;
        }
        out.token(&op.op);
        out.after_op = true;
    }
    out.buf.push(b'\n');
    Some(out.buf)
}

struct Out {
    buf: Vec<u8>,
    /// The last token written was an operator: a separator after it is a line break.
    after_op: bool,
}

impl Out {
    fn token(&mut self, t: &[u8]) {
        if let (Some(&last), Some(&first)) = (self.buf.last(), t.first())
            && is_regular(last)
            && is_regular(first)
        {
            self.buf.push(if self.after_op { b'\n' } else { b' ' });
        }
        self.buf.extend_from_slice(t);
        self.after_op = false;
    }

    fn object(&mut self, o: &Object, depth: u8) -> Option<()> {
        if depth > MAX_OPERAND_DEPTH {
            return None;
        }
        match o {
            Object::Null => self.token(b"null"),
            Object::Bool(b) => self.token(if *b { b"true" } else { b"false" }),
            Object::Int(i) => self.token(i.to_string().as_bytes()),
            Object::Real(r) => self.token(real(*r)?.as_bytes()),
            Object::String(_) | Object::Name(_) => {
                let mut t = Vec::new();
                serialize(o, &mut t);
                self.token(&t);
            }
            Object::Array(items) => {
                self.token(b"[");
                for item in items {
                    self.object(item, depth.saturating_add(1))?;
                }
                self.token(b"]");
            }
            Object::Dict(d) => {
                self.token(b"<<");
                for (k, v) in d.iter() {
                    self.object(&Object::Name(k.clone()), depth)?;
                    self.object(v, depth.saturating_add(1))?;
                }
                self.token(b">>");
            }
            Object::Ref(_) | Object::Stream(_) => return None,
        }
        Some(())
    }
}

/// The shortest text that reads back as `r` exactly, without an exponent (§7.3.3 has none): a
/// whole number in the integer range as an integer, others as reals, without the leading zero.
/// A whole number beyond that range keeps a period, so no reader takes it for an integer.
fn real(r: f64) -> Option<String> {
    if !r.is_finite() {
        return None;
    }
    if r.fract() == 0.0 && r.abs() <= f64::from(i32::MAX) {
        // Whole and within ±2³¹−1, so the conversion is exact.
        return Some(format!("{}", r as i64));
    }
    let mut s = r.to_string();
    if !s.contains('.') {
        s.push('.');
    }
    Some(if let Some(rest) = s.strip_prefix("0.") {
        format!(".{rest}")
    } else if let Some(rest) = s.strip_prefix("-0.") {
        format!("-.{rest}")
    } else {
        s
    })
}

/// Whether `out` reads back as exactly the operators `kept`, operands compared by value.
fn reads_back(out: &[u8], kept: &[&Op]) -> bool {
    !has_odd_number(out)
        && read_all(out, MAX_OPS).is_ok_and(|again| {
            again.len() == kept.len() && again.iter().zip(kept).all(|(a, b)| a.op == b.op && a.inline.is_none() && same_all(&a.operands, &b.operands))
        })
}

fn same_all(a: &[Object], b: &[Object]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
}

/// Equal as values: numbers by number, strings by their bytes however they are written.
pub(crate) fn same(a: &Object, b: &Object) -> bool {
    match (a, b) {
        (Object::Int(x), Object::Int(y)) => x == y,
        (Object::Real(x), Object::Real(y)) => x == y,
        (Object::Int(i), Object::Real(r)) | (Object::Real(r), Object::Int(i)) => {
            r.fract() == 0.0 && r.abs() <= f64::from(i32::MAX) && *i == *r as i64
        }
        (Object::String(x), Object::String(y)) => x.bytes == y.bytes,
        (Object::Array(x), Object::Array(y)) => same_all(x, y),
        (Object::Dict(x), Object::Dict(y)) => x.iter().count() == y.iter().count() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

fn is_ws(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn is_regular(b: u8) -> bool {
    !is_ws(b) && !is_delim(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::Dict;

    fn kept(src: &str) -> Vec<Vec<u8>> {
        let ops = read_all(src.as_bytes(), MAX_OPS).ok().unwrap();
        let keep = redundant(&ops).unwrap();
        ops.iter().zip(keep).filter(|(_, k)| *k).map(|(o, _)| o.op.clone()).collect()
    }

    fn ops(names: &[&str]) -> Vec<Vec<u8>> {
        names.iter().map(|n| n.as_bytes().to_vec()).collect()
    }

    #[test]
    fn what_draws_nothing_goes_and_everything_else_stays() {
        // Empty pairs, nested ones too, and an identity matrix.
        assert_eq!(kept("q Q q q Q Q 1 0 0 1 0 0 cm 0 0 m S"), ops(&["m", "S"]));
        // A pair around something drawn stays; so does a matrix that moves.
        assert_eq!(kept("q 0 0 m S Q 1 0 0 1 5 0 cm"), ops(&["q", "m", "S", "Q", "cm"]));
        // A repeat of the value in force goes, by value (0.5 and .50 are one number)...
        assert_eq!(kept("0.5 g 0 0 m f .50 g 1 w 1.0 w"), ops(&["g", "m", "f", "w"]));
        // ... but not after `Q` restores another value, nor after an operator that may change it.
        assert_eq!(kept("1 g q 2 g Q 2 g"), ops(&["g", "q", "g", "Q", "g"]));
        assert_eq!(kept("1 w /GS0 gs 1 w"), ops(&["w", "gs", "w"]));
        // Colour space and colour share one slot: setting the space again resets the colour.
        assert_eq!(kept("/CS0 cs 1 sc /CS0 cs"), ops(&["cs", "sc", "cs"]));
    }

    #[test]
    fn a_td_sets_the_leading_so_the_next_tl_stays() {
        assert_eq!(kept("BT 5 TL 0 -12 TD 5 TL T* ET"), ops(&["BT", "TL", "TD", "TL", "T*", "ET"]));
    }

    #[test]
    fn numbers_are_written_shorter_and_read_back_exactly() {
        for r in [0.5, -0.25, 0.1, 1.0 / 3.0, 612.000_1, -0.000_001, 3.0, -0.0, 1e12 + 0.5, 4e9, -1e15] {
            let text = real(r).unwrap();
            let mut full = Vec::new();
            serialize(&Object::Real(r), &mut full);
            assert!(text.len() <= full.len(), "{r}: {text}");
            assert!(!text.contains('e'), "{text}");
            assert!(is_number(text.as_bytes()), "{text}");
            let back = read(format!("{text} w").as_bytes()).next().and_then(|o| o.num(0));
            assert_eq!(back, Some(r), "{text}");
        }
        assert_eq!(real(4e9).as_deref(), Some("4000000000."), "beyond the integer range it stays a real");
        assert!(real(0.5).unwrap().len() < "0.5".len(), "the leading zero goes");
        assert!(real(f64::NAN).is_none());
    }

    #[test]
    fn tokens_that_only_look_like_numbers_are_found_outside_strings_and_names() {
        for odd in ["1.2.3", "1e5", "1.5e3", ".", "-.", "--5", "1.5-3", "-", "+-", "4E9"] {
            assert!(has_odd_number(format!("0 0 m {odd} w").as_bytes()), "{odd}");
        }
        let fine = "% 1.2.3 in a comment\n(1e5 (2.3.4) \\) .) Tj <0A1e5> Tj /1.2.3 Do << /K -.5 >> BDC 1. .5 -3 +4 w d0 T* EMC";
        assert!(!has_odd_number(fine.as_bytes()));
    }

    #[test]
    fn a_stream_with_more_operators_than_the_cap_is_left_as_it_is() {
        assert!(read_all(b"n n n", 3).is_ok());
        assert!(matches!(read_all(b"n n n n", 3), Err(Outcome::Kept)));
        assert!(matches!(read_all(b"1 2 3 4 m", 3), Err(Outcome::Kept)), "operands count too");
        assert!(matches!(read_all(b"n ) n", 3), Err(Outcome::Unreadable)));
        // Operands with no operator after them are held until the end: counted before reading.
        assert!(matches!(read_all(b"1 1 1 1 1 1 1", 3), Err(Outcome::Kept)));
        assert!(matches!(read_all(b"[1 1 1 1 1 1] n", 3), Err(Outcome::Kept)), "array elements count");
    }

    #[test]
    fn a_spent_allowance_leaves_the_stream_as_it_is() {
        let s = Stream::from_raw(Dict::new(), b"q   Q   0   0   m   10   10   l   S  ".to_vec());
        // Decoding draws on the same allowance; the pass must still ask for its own share, and
        // for compressing the original too when the later pass compresses unfiltered streams.
        let settings = Settings::default();
        let cost = u64::try_from(s.raw.len()).unwrap() * (WORK_PER_BYTE + ENCODE_WORK);
        let least = (0..=4 * cost).find(|w| matches!(compact(&s, &settings, &mut ImageBudget::with_work(*w)), Outcome::Compacted(..)));
        assert!(least.is_some_and(|w| w >= cost), "{least:?}");
        assert!(matches!(compact(&s, &settings, &mut ImageBudget::with_work(cost - 1)), Outcome::Unreadable));
    }
}
