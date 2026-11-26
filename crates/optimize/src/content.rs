//! Lossless content-stream compaction: page contents, form XObjects and tiling patterns are
//! written again with the fewest separators, numbers in
//! their shortest exact form and no comments, without empty `q Q` pairs, identity `cm`s, or
//! settings that repeat the value already in force. Every operator kept, and every operand by
//! value, is the same when the result is read back; anything this can't be sure of leaves the
//! stream as it is.

use std::collections::BTreeMap;

use pdfcraft_content::{Op, parse};
use pdfcraft_cos::{Document, ImageBudget, MAX_PAGE_DECODED, ObjRef, Object, Stream, serialize};
use pdfcraft_filters::{Filter, Params};

use crate::{OptimizeError, Report, Settings, Stage, stored_len};

/// Steps of a run's allowance one decoded byte costs: read, written, read back and compressed.
const WORK_PER_BYTE: u64 = 16;
/// Deepest `q` nesting tracked; a stream nested deeper is left as it is.
const MAX_SAVE_DEPTH: usize = 1024;
/// Deepest array or dictionary operand written.
const MAX_OPERAND_DEPTH: u8 = 32;

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
                Object::Stream(s) => compact(s, budget),
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

/// `s` compacted, if that is certain to draw the same and is smaller.
fn compact(s: &Stream, budget: &mut ImageBudget) -> Outcome {
    let Some(chain) = rewritable_filters(s) else { return Outcome::Unreadable };
    let Ok(data) = budget.decode_chain(&chain, &s.raw, MAX_PAGE_DECODED) else { return Outcome::Unreadable };
    if !budget.spend(u64::try_from(data.len()).unwrap_or(u64::MAX).saturating_mul(WORK_PER_BYTE)) {
        return Outcome::Unreadable;
    }
    let parsed = parse(&data);
    if parsed.skipped > 0 || parsed.ambiguous_images > 0 {
        return Outcome::Unreadable;
    }
    if !rewritable(&parsed.ops) || has_lone_sign(&data) {
        return Outcome::Kept;
    }
    let Some(keep) = redundant(&parsed.ops) else { return Outcome::Kept };
    let kept: Vec<&Op> = parsed.ops.iter().zip(&keep).filter(|(_, k)| **k).map(|(op, _)| op).collect();
    let Some(out) = write(&kept, data.first().is_some_and(|b| is_ws(*b))) else { return Outcome::Kept };
    if !reads_back(&out, &kept) {
        return Outcome::Kept;
    }
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    let new = if chain.is_empty() { Stream::from_raw(dict, out.clone()) } else { Stream::flate(dict, &out) };
    match (stored_len(&new), stored_len(s)) {
        (Some(after), Some(before)) if after < before => {
            let counts = Counts { ops_removed: parsed.ops.len().saturating_sub(kept.len()), bytes_before: data.len(), bytes_after: out.len() };
            Outcome::Compacted(new, counts)
        }
        _ => Outcome::Kept,
    }
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

/// Whether every operator is one this knows and writes: no inline image, no unknown operator, no
/// reference among the operands, and `q` and `Q` without operands.
fn rewritable(ops: &[Op]) -> bool {
    ops.iter().all(|op| {
        op.inline.is_none()
            && KNOWN.contains(&op.op.as_slice())
            && !(matches!(op.op.as_slice(), b"q" | b"Q") && !op.operands.is_empty())
            && op.operands.iter().all(|o| !matches!(o, Object::Ref(_) | Object::Stream(_)))
    })
}

/// Whether `data` may hold a token of signs alone (`-`, `+-`), which the parser reads as 0 and
/// other readers may not. Strings and names are not told apart, so this may refuse a stream that
/// has none.
fn has_lone_sign(data: &[u8]) -> bool {
    let boundary = |b: Option<&u8>| b.is_none_or(|b| is_ws(*b) || is_delim(*b));
    let mut i = 0;
    while let Some(&b) = data.get(i) {
        if matches!(b, b'-' | b'+') && boundary(i.checked_sub(1).and_then(|p| data.get(p))) {
            let run = data.get(i..).map_or(0, |rest| rest.iter().take_while(|c| matches!(c, b'-' | b'+')).count());
            if boundary(data.get(i.saturating_add(run))) {
                return true;
            }
            i = i.saturating_add(run);
        } else {
            i = i.saturating_add(1);
        }
    }
    false
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
/// whole number in the integer range as an integer, others without the leading zero.
fn real(r: f64) -> Option<String> {
    if !r.is_finite() {
        return None;
    }
    if r.fract() == 0.0 && r.abs() <= f64::from(i32::MAX) {
        // Whole and within ±2³¹−1, so the conversion is exact.
        return Some(format!("{}", r as i64));
    }
    let s = r.to_string();
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
    let again = parse(out);
    again.skipped == 0
        && again.ambiguous_images == 0
        && again.ops.len() == kept.len()
        && again.ops.iter().zip(kept).all(|(a, b)| a.op == b.op && a.inline.is_none() && same_all(&a.operands, &b.operands))
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
        let ops = parse(src.as_bytes()).ops;
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
        for r in [0.5, -0.25, 0.1, 1.0 / 3.0, 612.000_1, -0.000_001, 3.0, -0.0, 1e12 + 0.5, 4e9] {
            let text = real(r).unwrap();
            let mut full = Vec::new();
            serialize(&Object::Real(r), &mut full);
            assert!(text.len() <= full.len(), "{r}: {text}");
            assert!(!text.contains('e'), "{text}");
            let back = parse(format!("{text} w").as_bytes()).ops.first().and_then(|o| o.num(0));
            assert_eq!(back, Some(r), "{text}");
        }
        assert!(real(0.5).unwrap().len() < "0.5".len(), "the leading zero goes");
        assert!(real(f64::NAN).is_none());
    }

    #[test]
    fn a_spent_allowance_leaves_the_stream_as_it_is() {
        let s = Stream::from_raw(Dict::new(), b"q   Q   0   0   m   10   10   l   S  ".to_vec());
        // Decoding draws on the same allowance; the pass must still ask for its own share.
        let cost = u64::try_from(s.raw.len()).unwrap() * WORK_PER_BYTE;
        let least = (0..=4 * cost).find(|w| matches!(compact(&s, &mut ImageBudget::with_work(*w)), Outcome::Compacted(..)));
        assert!(least.is_some_and(|w| w >= cost), "{least:?}");
        assert!(matches!(compact(&s, &mut ImageBudget::with_work(cost - 1)), Outcome::Unreadable));
    }
}
