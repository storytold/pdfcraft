//! The content side: `cs`/`CS` and `sc`/`scn`/`SC`/`SCN` under a converted space become `k`/`K`,
//! through page content, forms, tiling patterns and annotation appearances, and paints whose
//! overprint does not survive are reported (ISO 32000-1 §8.6.7).
//!
//! A form is drawn with its caller's graphics state (§8.10.1), so it is rewritten for the state
//! of the first place that draws it; a later place drawing it under a different state is
//! reported. Only the operators that change are replaced: the rest of each stream keeps its bytes.

use std::collections::BTreeMap;
use std::ops::Range;

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, ObjRef, Object, Stream};

use super::{Spaces, SpotsConverted, Work, real};
use crate::page_list;

/// Forms, patterns and appearances nest at most this deep.
const MAX_DEPTH: usize = 32;
/// Streams walked in all, counting a form once per time it is drawn.
const MAX_VISITS: usize = 4096;
/// `q` nesting tracked.
const MAX_SAVES: usize = 512;

/// The graphics state this rewrite tracks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct GState {
    /// The converted fill space, by index into `Spaces::conversions`.
    fill: Option<usize>,
    stroke: Option<usize>,
    /// `/OP`.
    op_stroke: bool,
    /// `/op`, or `/OP` when `/op` is absent (§8.6.7).
    op_fill: bool,
}

/// A stream's new decoded bytes and how many colour operators they rewrite.
type Rewrite = (Vec<u8>, usize);

struct Rewriter<'a, 'd> {
    spaces: &'a mut Spaces<'d>,
    report: &'a mut SpotsConverted,
    active: Vec<ObjRef>,
    visits: usize,
    /// Each stream's new decoded bytes and the colour operators rewritten in them, from the
    /// first time it was drawn; `None` when it needed no change.
    out: BTreeMap<ObjRef, Option<Rewrite>>,
}

/// Rewrite every page's content and its annotations' appearances.
pub(super) fn rewrite_pages(spaces: &mut Spaces<'_>, work: &mut Work<'_>, report: &mut SpotsConverted) {
    let doc = spaces.doc;
    let mut r = Rewriter { spaces, report, active: Vec::new(), visits: 0, out: BTreeMap::new() };
    for page in page_list(doc) {
        let resources = dict_at(doc, page.dict.get(b"Resources"));
        r.page(&page.dict, &resources);
        for ap in appearances(doc, &page.dict) {
            r.form(ap, &Dict::new(), GState::default(), 0);
        }
    }
    for (at, rewrite) in r.out {
        let Some((bytes, paints)) = rewrite else { continue };
        r.report.paints += paints;
        let Object::Stream(s) = work.current(at) else { continue };
        let mut dict = s.dict;
        for k in [&b"Length"[..], b"Filter", b"DecodeParms", b"DL"] {
            dict.remove(k);
        }
        work.objects.insert(at, Object::Stream(Stream::flate(dict, &bytes)));
    }
}

fn dict_at(doc: &pdfcraft_cos::Document, o: Option<&Object>) -> Dict {
    o.map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default()
}

/// Every appearance stream of a page's annotations: `/N`, `/R` and `/D`, and their states.
fn appearances(doc: &pdfcraft_cos::Document, page: &Dict) -> Vec<ObjRef> {
    let annots = page.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
    let mut out = Vec::new();
    for a in annots {
        let annot = doc.resolve(&a);
        let ap = dict_at(doc, annot.as_dict().and_then(|d| d.get(b"AP")));
        for (_, v) in ap.iter() {
            match &*doc.resolve(v) {
                Object::Stream(_) => out.extend(v.as_ref()),
                Object::Dict(states) => out.extend(states.iter().filter_map(|(_, s)| s.as_ref())),
                _ => {}
            }
        }
    }
    out
}

/// The entry `name` of resource category `kind`.
fn resource(doc: &pdfcraft_cos::Document, resources: &Dict, kind: &[u8], name: &[u8]) -> Option<Object> {
    dict_at(doc, resources.get(kind)).get(name).cloned()
}

/// A stream's content, decoded; `None` when it is not a stream or can't be read.
fn decoded(doc: &pdfcraft_cos::Document, at: ObjRef) -> Option<(Dict, Vec<u8>)> {
    match &*doc.get(at) {
        Object::Stream(s) => s.decoded().ok().map(|d| (s.dict.clone(), d)),
        _ => None,
    }
}

impl Rewriter<'_, '_> {
    /// A page's content streams, which share one graphics state.
    fn page(&mut self, page: &Dict, resources: &Dict) {
        let doc = self.spaces.doc;
        let refs: Vec<Object> = match page.get(b"Contents").map(|c| doc.resolve(c)) {
            None => Vec::new(),
            Some(c) => match &*c {
                Object::Array(a) => a.clone(),
                _ => page.get(b"Contents").cloned().into_iter().collect(),
            },
        };
        let mut stack = vec![GState::default()];
        let mut results = Vec::new();
        for o in &refs {
            let Some((at, (_, data))) = o.as_ref().and_then(|at| decoded(doc, at).map(|d| (at, d))) else {
                let why = "a page's content cannot be read, so none of that page was converted".into();
                self.report.refuse(o.as_ref(), b"", why);
                return;
            };
            let bytes = self.walk(at, &data, resources, &mut stack, 0);
            results.push((at, bytes));
        }
        for (at, bytes) in results {
            self.record(at, bytes);
        }
    }

    /// Keep a stream's first rewrite; report a later, different one.
    fn record(&mut self, at: ObjRef, bytes: Option<Rewrite>) {
        match self.out.get(&at) {
            None => {
                self.out.insert(at, bytes);
            }
            Some(first) if *first != bytes => {
                let why = "it is drawn under different colour states; it was converted for the first".into();
                self.report.refuse(Some(at), b"", why);
            }
            Some(_) => {}
        }
    }

    /// A form, tiling pattern or appearance, drawn with `caller`'s state.
    fn form(&mut self, at: ObjRef, outer: &Dict, caller: GState, depth: usize) {
        if depth >= MAX_DEPTH || self.active.contains(&at) {
            return;
        }
        self.visits += 1;
        if self.visits > MAX_VISITS {
            self.report.disclose(format!("The document draws more than {MAX_VISITS} nested streams; the rest were not converted."));
            return;
        }
        let doc = self.spaces.doc;
        let Some((dict, data)) = decoded(doc, at) else {
            self.report.refuse(Some(at), b"", "its content cannot be read".into());
            return;
        };
        let own = dict.get(b"Resources").map(|o| dict_at(doc, Some(o)));
        self.active.push(at);
        let mut stack = vec![caller];
        let bytes = self.walk(at, &data, own.as_ref().unwrap_or(outer), &mut stack, depth);
        self.active.pop();
        self.record(at, bytes);
    }

    /// Walk one stream; its new bytes when an operator changed.
    fn walk(&mut self, at: ObjRef, data: &[u8], resources: &Dict, stack: &mut Vec<GState>, depth: usize) -> Option<Rewrite> {
        let parsed = parse(data);
        let mut edits: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
        let mut too_deep = false;
        for op in &parsed.ops {
            if let Some((params, _)) = &op.inline {
                self.inline_image(at, params, resources);
                continue;
            }
            match op.op.as_slice() {
                b"q" => {
                    let top = stack.last().copied().unwrap_or_default();
                    if stack.len() < MAX_SAVES {
                        stack.push(top);
                    } else {
                        too_deep = true;
                    }
                }
                b"Q" if stack.len() > 1 => {
                    stack.pop();
                }
                _ => {
                    let mut state = stack.last().copied().unwrap_or_default();
                    if let Some(new) = self.operator(at, op, resources, &mut state, depth) {
                        edits.push((op.span.clone(), serialize_ops(&[new])));
                    }
                    match stack.last_mut() {
                        Some(top) => *top = state,
                        None => stack.push(state),
                    }
                }
            }
        }
        if edits.is_empty() {
            return None;
        }
        if parsed.ambiguous_images > 0 {
            let why = "an inline image in its content can't be read with certainty, so where its operators end is a guess".into();
            self.report.refuse(Some(at), b"", why);
            return None;
        }
        if too_deep {
            let why = format!("its graphics state is saved more than {MAX_SAVES} deep, so its colours can't be followed");
            self.report.refuse(Some(at), b"", why);
            return None;
        }
        Some((splice(data, &edits), edits.len()))
    }

    /// One operator other than `q`/`Q`: update `state`, and return its replacement when it is
    /// rewritten.
    fn operator(&mut self, at: ObjRef, op: &Op, resources: &Dict, state: &mut GState, depth: usize) -> Option<Op> {
        match op.op.as_slice() {
            b"cs" | b"CS" => {
                let stroke = op.is("CS");
                let hit = self.set_space(at, op, resources);
                *if stroke { &mut state.stroke } else { &mut state.fill } = hit;
                let cmyk = self.spaces.conversions.get(hit?)?.initial()?;
                Some(cmyk_op(cmyk, stroke))
            }
            b"sc" | b"scn" | b"SC" | b"SCN" => {
                let stroke = op.op.first() == Some(&b'S');
                match if stroke { state.stroke } else { state.fill } {
                    Some(c) => self.set_colour(at, op, c, stroke),
                    None => {
                        self.pattern(op, resources, depth);
                        None
                    }
                }
            }
            b"g" | b"rg" | b"k" => {
                state.fill = None;
                None
            }
            b"G" | b"RG" | b"K" => {
                state.stroke = None;
                None
            }
            b"gs" => {
                self.ext_gstate(op, resources, state);
                None
            }
            b"Do" => {
                self.xobject(at, op, resources, *state, depth);
                None
            }
            b"sh" => {
                self.shading(at, op, resources, *state);
                None
            }
            _ => {
                self.paint(at, op, *state);
                None
            }
        }
    }

    /// `cs`/`CS`: the converted space it selects, if any.
    fn set_space(&mut self, at: ObjRef, op: &Op, resources: &Dict) -> Option<usize> {
        let doc = self.spaces.doc;
        let object = resource(doc, resources, b"ColorSpace", op.name(0)?)?;
        let resolved = doc.resolve(&object);
        if let Some(items) = resolved.as_array()
            && items.first().map(|h| doc.resolve(h)).and_then(|h| h.as_name().map(<[u8]>::to_vec)).as_deref() == Some(b"Pattern")
        {
            if let Some(base) = items.get(1)
                && let Some(c) = self.spaces.lookup(base, Some(at), self.report)
            {
                let ink = self.spaces.conversions.get(c).map(|v| v.ink().to_vec()).unwrap_or_default();
                self.report.refuse(Some(at), &ink, "an uncoloured pattern paints in it".into());
            }
            return None;
        }
        self.spaces.lookup(&object, Some(at), self.report)
    }

    /// `sc`/`scn` under converted space `c`.
    fn set_colour(&mut self, at: ObjRef, op: &Op, c: usize, stroke: bool) -> Option<Op> {
        let conv = self.spaces.conversions.get(c)?;
        let tints: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
        match conv.cmyk(&tints).filter(|_| tints.len() == op.operands.len()) {
            Some(cmyk) => Some(cmyk_op(cmyk, stroke)),
            None => {
                let ink = conv.ink().to_vec();
                let initial = conv.initial()?;
                let why = format!("a colour operator has {} operands it cannot use; it was given the initial colour", op.operands.len());
                self.report.refuse(Some(at), &ink, why);
                Some(cmyk_op(initial, stroke))
            }
        }
    }

    fn ext_gstate(&self, op: &Op, resources: &Dict, state: &mut GState) {
        let doc = self.spaces.doc;
        let Some(entry) = op.name(0).and_then(|n| resource(doc, resources, b"ExtGState", n)) else {
            return;
        };
        let d = dict_at(doc, Some(&entry));
        let flag = |k: &[u8]| match d.get(k).map(|o| doc.resolve(o)).as_deref() {
            Some(Object::Bool(b)) => Some(*b),
            _ => None,
        };
        if let Some(on) = flag(b"OP") {
            state.op_stroke = on;
            state.op_fill = on;
        }
        if let Some(fill) = flag(b"op") {
            state.op_fill = fill;
        }
    }

    /// Report a fill or stroke that overprints in a converted colour.
    fn paint(&mut self, at: ObjRef, op: &Op, state: GState) {
        let name = op.op.as_slice();
        let fills = matches!(name, b"f" | b"F" | b"f*" | b"b" | b"b*" | b"B" | b"B*" | b"Tj" | b"TJ" | b"'" | b"\"");
        let strokes = matches!(name, b"S" | b"s" | b"b" | b"b*" | b"B" | b"B*");
        let lost = (fills && state.fill.is_some() && state.op_fill) || (strokes && state.stroke.is_some() && state.op_stroke);
        if lost {
            self.report.overprinted(at);
        }
    }

    fn xobject(&mut self, at: ObjRef, op: &Op, resources: &Dict, state: GState, depth: usize) {
        let doc = self.spaces.doc;
        let Some(x) = op.name(0).and_then(|n| resource(doc, resources, b"XObject", n)).and_then(|o| o.as_ref()) else {
            return;
        };
        let (subtype, cs) = match &*doc.get(x) {
            Object::Stream(s) => (s.dict.name(b"Subtype").map(<[u8]>::to_vec), s.dict.get(b"ColorSpace").cloned()),
            _ => return,
        };
        match subtype.as_deref() {
            Some(b"Form") => self.form(x, resources, state, depth + 1),
            Some(b"Image") => {
                if let Some(cs) = cs
                    && self.spaces.lookup(&cs, Some(x), self.report).is_some()
                    && state.op_fill
                {
                    self.report.overprinted(at);
                }
            }
            _ => {}
        }
    }

    fn shading(&mut self, at: ObjRef, op: &Op, resources: &Dict, state: GState) {
        let doc = self.spaces.doc;
        let cs = op.name(0).and_then(|n| resource(doc, resources, b"Shading", n)).and_then(|s| dict_at(doc, Some(&s)).get(b"ColorSpace").cloned());
        if let Some(cs) = cs
            && self.spaces.lookup(&cs, None, self.report).is_some()
            && state.op_fill
        {
            self.report.overprinted(at);
        }
    }

    /// `scn` naming a tiling pattern: walk its cell.
    fn pattern(&mut self, op: &Op, resources: &Dict, depth: usize) {
        let doc = self.spaces.doc;
        let Some(p) = op.last_name().and_then(|n| resource(doc, resources, b"Pattern", n)).and_then(|o| o.as_ref()) else {
            return;
        };
        if matches!(&*doc.get(p), Object::Stream(_)) {
            self.form(p, resources, GState::default(), depth + 1);
        }
    }

    /// An inline image in a converted space is refused: its samples are part of the content.
    fn inline_image(&mut self, at: ObjRef, params: &Dict, resources: &Dict) {
        let doc = self.spaces.doc;
        let Some(cs) = params.get(b"CS").or(params.get(b"ColorSpace")) else {
            return;
        };
        let object = match cs.as_name() {
            Some(n) => resource(doc, resources, b"ColorSpace", n),
            None => Some(cs.clone()),
        };
        if let Some(c) = object.and_then(|o| self.spaces.lookup(&o, Some(at), self.report)) {
            let ink = self.spaces.conversions.get(c).map(|v| v.ink().to_vec()).unwrap_or_default();
            self.report.refuse(Some(at), &ink, "an inline image paints in it; its samples were not converted".into());
        }
    }
}

/// `c m y k k`, or `K` for a stroke.
fn cmyk_op(cmyk: [f64; 4], stroke: bool) -> Op {
    Op::new(if stroke { "K" } else { "k" }, cmyk.iter().map(|v| real(*v)).collect())
}

/// `data` with each range replaced; `edits` are in stream order and don't overlap.
fn splice(data: &[u8], edits: &[(Range<usize>, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len().saturating_add(edits.len().saturating_mul(16)));
    let mut at = 0usize;
    for (range, text) in edits {
        if range.start < at {
            continue;
        }
        out.extend_from_slice(data.get(at..range.start).unwrap_or_default());
        out.push(b' ');
        out.extend_from_slice(text.trim_ascii());
        out.push(b' ');
        at = range.end;
    }
    out.extend_from_slice(data.get(at..).unwrap_or_default());
    out
}
