//! The content interpreter that applies (or verifies) redaction on one content scope: a page's
//! streams or a form XObject's.
//!
//! It tracks just enough graphics state to place every glyph, image and path in user space:
//! the CTM stack, the text state (font, size, spacing, scaling, rise, leading) and the text
//! matrices. In apply mode it rewrites the operators:
//! - text: glyphs whose boxes overlap a region are cut out of `Tj`/`TJ`/`'`/`"`, replaced by a
//!   `TJ` displacement of the same width so the remaining text stays exactly where it was;
//! - images: fully covered → the `Do` is removed; partly covered → the covered pixels are
//!   cleared in a copy of the image (or the image is removed when its codec can't be re-encoded);
//!   inline images under a region are removed;
//! - vectors: fully covered paths are removed; partly covered ones are clipped so nothing shows
//!   inside the regions; shadings are clipped the same way;
//! - form XObjects under a region are rewritten recursively into new objects (copy-on-write, so
//!   other pages using the original are untouched).
//!
//! In verify mode nothing changes; glyphs and inline images that still overlap a region are
//! counted.

use std::collections::HashMap;
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op, contains, num, overlaps, parse, serialize_ops, string};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::{Report, image};
use pdfcraft_fonts::pdf::Metrics;

const MAX_DEPTH: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Apply,
    Verify,
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    font: Rc<Metrics>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    render_mode: i64,
    scale: f64,
    leading: f64,
    rise: f64,
}

/// What processing a scope produced.
#[derive(Default)]
pub(crate) struct Output {
    /// The rewritten streams (`None` = unchanged).
    pub streams: Vec<Option<Vec<u8>>>,
    /// New XObjects the rewritten content refers to: (resource name, object).
    pub xobjects: Vec<(Vec<u8>, ObjRef)>,
    /// Glyphs / inline images still overlapping a region (verify mode).
    pub residue: usize,
}

pub(crate) struct Scope<'a> {
    pub rects: &'a [[f64; 4]],
    pub mode: Mode,
    pub report: &'a mut Report,
    /// Sanitize: remove hidden text (invisible render modes, or wholly outside this page box).
    pub hidden_text: Option<[f64; 4]>,
    /// Sanitize: optional content groups (and membership dictionaries) that are off; content
    /// marked with them is removed.
    pub hidden_layers: Vec<ObjRef>,
    /// Content removed from hidden layers (blocks and XObjects).
    pub layer_blocks: usize,
    fonts: HashMap<Vec<u8>, Rc<Metrics>>,
    used_names: Vec<Vec<u8>>,
    depth: usize,
}

impl<'a> Scope<'a> {
    pub fn new(rects: &'a [[f64; 4]], mode: Mode, report: &'a mut Report) -> Self {
        Scope {
            rects,
            mode,
            report,
            hidden_text: None,
            hidden_layers: Vec::new(),
            layer_blocks: 0,
            fonts: HashMap::new(),
            used_names: Vec::new(),
            depth: 0,
        }
    }

    /// Sanitizing visits every form XObject (not only those under a region).
    fn everywhere(&self) -> bool {
        self.hidden_text.is_some() || !self.hidden_layers.is_empty()
    }

    /// Is this optional-content reference (an OCG or OCMD) hidden?
    fn layer_hidden(&self, doc: &Document, o: &Object) -> bool {
        if self.hidden_layers.is_empty() {
            return false;
        }
        if let Some(r) = o.as_ref()
            && self.hidden_layers.contains(&r)
        {
            return true;
        }
        // A membership dictionary (default policy AnyOn): hidden when all its groups are.
        let d = doc.resolve(o);
        let Some(d) = d.as_dict() else { return false };
        if d.name(b"Type") != Some(b"OCMD") {
            return false;
        }
        let groups: Vec<ObjRef> = match d.get(b"OCGs").map(|g| (*doc.resolve(g)).clone()) {
            Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
            Some(_) => d.get(b"OCGs").and_then(Object::as_ref).into_iter().collect(),
            None => Vec::new(),
        };
        !groups.is_empty() && groups.iter().all(|g| self.hidden_layers.contains(g))
    }

    fn hits(&self, b: [f64; 4]) -> bool {
        self.rects.iter().any(|r| overlaps(*r, b, 0.0))
    }

    fn covered(&self, b: [f64; 4]) -> bool {
        self.rects.iter().any(|r| contains(*r, b, 0.01))
    }

    /// Does a glyph box count as under a region? Any real overlap does (more than a fifth of
    /// the glyph or a point, whichever is less, in both directions), so neighbours that only
    /// touch an edge survive while partly covered glyphs go. Degenerate boxes (zero-size text)
    /// count when their origin is inside.
    fn glyph_hit(&self, b: [f64; 4], origin: (f64, f64)) -> bool {
        let (gw, gh) = (b[2] - b[0], b[3] - b[1]);
        if gw < 0.01 || gh < 0.01 {
            return self.rects.iter().any(|r| origin.0 >= r[0] && origin.0 <= r[2] && origin.1 >= r[1] && origin.1 <= r[3]);
        }
        let (mx, my) = ((gw * 0.2).min(1.0), (gh * 0.2).min(1.0));
        self.rects.iter().any(|r| {
            let ox = b[2].min(r[2]) - b[0].max(r[0]);
            let oy = b[3].min(r[3]) - b[1].max(r[1]);
            ox > mx && oy > my
        })
    }
}

fn res_dict(doc: &Document, res: &Dict, key: &[u8]) -> Dict {
    res.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default()
}

fn fresh_name(scope: &mut Scope<'_>, xobjects: &Dict) -> Vec<u8> {
    let mut i = scope.used_names.len() + 1;
    loop {
        let n = format!("PCRedacted{i}").into_bytes();
        if !xobjects.contains(&n) && !scope.used_names.contains(&n) {
            scope.used_names.push(n.clone());
            return n;
        }
        i += 1;
    }
}

/// The clip that hides everything inside each region: one even-odd clip per region (a big
/// rectangle with the region cut out), in the current user space. Returns `None` when the CTM
/// can't be inverted (nothing is painted then anyway).
fn clip_out(rects: &[[f64; 4]], ctm: &Matrix, around: [f64; 4]) -> Option<Vec<Op>> {
    let inv = ctm.invert()?;
    let mut ops = Vec::new();
    let big = [around[0] - 10.0, around[1] - 10.0, around[2] + 10.0, around[3] + 10.0];
    for r in rects.iter().filter(|r| overlaps(**r, around, 0.0)) {
        let outer = [(big[0], big[1]), (big[2], big[1]), (big[2], big[3]), (big[0], big[3])];
        let inner = [(r[0], r[1]), (r[0], r[3]), (r[2], r[3]), (r[2], r[1])];
        for poly in [outer, inner] {
            for (k, (x, y)) in poly.iter().enumerate() {
                let (u, v) = inv.apply(*x, *y);
                ops.push(Op::new(if k == 0 { "m" } else { "l" }, vec![num(u), num(v)]));
            }
            ops.push(Op::new("h", vec![]));
        }
        ops.push(Op::new("W*", vec![]));
        ops.push(Op::new("n", vec![]));
    }
    Some(ops)
}

/// One glyph of a shown string.
struct Glyph {
    bytes: std::ops::Range<usize>,
    /// Advance in unscaled text space (before `Th`).
    advance: f64,
    hit: bool,
}

/// Process a scope's content streams (state flows from one stream to the next).
pub(crate) fn process(doc: &mut Document, scope: &mut Scope<'_>, streams: &[Vec<u8>], resources: &Dict, ctm: Matrix) -> Output {
    let mut out = Output::default();
    let fallback = Rc::new(Metrics::fallback());
    let mut gs =
        Gs { ctm, font: fallback.clone(), size: 0.0, char_spacing: 0.0, word_spacing: 0.0, render_mode: 0, scale: 1.0, leading: 0.0, rise: 0.0 };
    let mut stack: Vec<Gs> = Vec::new();
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let fonts_res = res_dict(doc, resources, b"Font");
    let mut xobjects = res_dict(doc, resources, b"XObject");
    // The current path: its operators, its bounding box in user space, and whether it clips.
    let mut path: Vec<Op> = Vec::new();
    let mut path_box: Option<[f64; 4]> = None;
    let mut clip = false;
    let mut forced_change = false;

    for data in streams {
        let mut parsed = parse(data);
        if !scope.hidden_layers.is_empty() {
            let props = res_dict(doc, resources, b"Properties");
            let (kept, removed) = strip_hidden_layers(doc, scope, parsed.ops, &props);
            parsed.ops = kept;
            if removed > 0 {
                scope.layer_blocks += removed;
                forced_change = true;
            }
        }
        let mut ops: Vec<Op> = Vec::with_capacity(parsed.ops.len());
        let mut changed = std::mem::take(&mut forced_change);
        for op in parsed.ops {
            let o = op.op.as_slice();
            // Path construction.
            if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
                let pts: Vec<(f64, f64)> = match o {
                    b"re" => op.nums::<4>().map(|[x, y, w, h]| vec![(x, y), (x + w, y), (x, y + h), (x + w, y + h)]).unwrap_or_default(),
                    b"h" | b"W" | b"W*" => Vec::new(),
                    _ => op.operands.iter().filter_map(Object::as_f64).collect::<Vec<_>>().as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect(),
                };
                for (x, y) in pts {
                    let (u, v) = gs.ctm.apply(x, y);
                    path_box = Some(match path_box {
                        None => [u, v, u, v],
                        Some(b) => [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)],
                    });
                }
                clip |= matches!(o, b"W" | b"W*");
                path.push(op);
                continue;
            }
            if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                let body = std::mem::take(&mut path);
                let bbox = path_box.take();
                let clips = std::mem::replace(&mut clip, false);
                let hit = scope.mode == Mode::Apply && o != b"n" && bbox.is_some_and(|b| scope.hits(b));
                if !hit {
                    ops.extend(body);
                    ops.push(op);
                    continue;
                }
                let b = bbox.unwrap_or_default();
                changed = true;
                if scope.covered(b) {
                    scope.report.paths_removed += 1;
                    if clips {
                        // Keep the clipping, drop the painting.
                        ops.extend(body);
                        ops.push(Op::new("n", vec![]));
                    }
                } else if clips {
                    // A clip-and-paint path can't be wrapped in q/Q without losing its clip:
                    // paint it clipped, then set its clip with a no-op path.
                    scope.report.paths_clipped += 1;
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, b) {
                        let plain: Vec<Op> = body.iter().filter(|p| !p.is("W") && !p.is("W*")).cloned().collect();
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.extend(plain);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                    }
                    ops.extend(body);
                    ops.push(Op::new("n", vec![]));
                } else {
                    scope.report.paths_clipped += 1;
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, b) {
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.extend(body);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                    }
                }
                continue;
            }
            if !path.is_empty() {
                // A path not ended by a painting operator: pass it through.
                ops.append(&mut path);
                path_box = None;
                clip = false;
            }
            match o {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        gs.ctm = Matrix(m).then(&gs.ctm);
                    }
                }
                b"BT" => {
                    tm = Matrix::IDENTITY;
                    tlm = Matrix::IDENTITY;
                }
                b"Tf" => {
                    gs.size = op.num(1).unwrap_or(gs.size);
                    if let Some(name) = op.name(0) {
                        gs.font = match scope.fonts.get(name) {
                            Some(f) => f.clone(),
                            None => {
                                let m = fonts_res
                                    .get(name)
                                    .and_then(|f| doc.resolve(f).as_dict().cloned())
                                    .map(|d| Rc::new(Metrics::from_dict(doc, &d)))
                                    .unwrap_or_else(|| fallback.clone());
                                scope.fonts.insert(name.to_vec(), m.clone());
                                m
                            }
                        };
                    }
                }
                b"Tc" => gs.char_spacing = op.num(0).unwrap_or(0.0),
                b"Tw" => gs.word_spacing = op.num(0).unwrap_or(0.0),
                b"Tz" => gs.scale = op.num(0).unwrap_or(100.0) / 100.0,
                b"TL" => gs.leading = op.num(0).unwrap_or(0.0),
                b"Ts" => gs.rise = op.num(0).unwrap_or(0.0),
                b"Tr" => gs.render_mode = op.num(0).unwrap_or(0.0) as i64,
                b"Td" | b"TD" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        if o == b"TD" {
                            gs.leading = -y;
                        }
                        tlm = Matrix::translate(x, y).then(&tlm);
                        tm = tlm;
                    }
                }
                b"Tm" => {
                    if let Some(m) = op.nums::<6>() {
                        tlm = Matrix(m);
                        tm = tlm;
                    }
                }
                b"T*" => {
                    tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
                    tm = tlm;
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if o == b"\"" {
                        gs.word_spacing = op.num(0).unwrap_or(gs.word_spacing);
                        gs.char_spacing = op.num(1).unwrap_or(gs.char_spacing);
                    }
                    if matches!(o, b"'" | b"\"") {
                        tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
                        tm = tlm;
                    }
                    let items: Vec<Object> = match o {
                        b"TJ" => op.operands.first().and_then(Object::as_array).cloned().unwrap_or_default(),
                        _ => op.operands.last().cloned().into_iter().collect(),
                    };
                    let (new_items, removed) = show(scope, &gs, &mut tm, &items);
                    if removed == 0 {
                        ops.push(op);
                        continue;
                    }
                    if scope.mode == Mode::Verify {
                        out.residue += removed;
                        ops.push(op);
                        continue;
                    }
                    changed = true;
                    scope.report.glyphs += removed;
                    match o {
                        b"\"" => {
                            ops.push(Op::new("Tw", vec![num(gs.word_spacing)]));
                            ops.push(Op::new("Tc", vec![num(gs.char_spacing)]));
                            ops.push(Op::new("T*", vec![]));
                        }
                        b"'" => ops.push(Op::new("T*", vec![])),
                        _ => {}
                    }
                    ops.push(Op::new("TJ", vec![Object::Array(new_items)]));
                    continue;
                }
                b"BI" => {
                    let b = gs.ctm.bbox([0.0, 0.0, 1.0, 1.0]);
                    if scope.hits(b) {
                        if scope.mode == Mode::Verify {
                            out.residue += 1;
                        } else {
                            scope.report.images_removed += 1;
                            changed = true;
                            continue;
                        }
                    }
                }
                b"sh" if scope.mode == Mode::Apply => {
                    // A shading fills the current clip: clip the regions out.
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, [-1e6, -1e6, 1e6, 1e6]) {
                        changed = true;
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                        continue;
                    }
                }
                b"Do" => {
                    if let Some(new) = xobject(doc, scope, &op, &gs, resources, &mut xobjects, &mut out) {
                        changed = true;
                        if let Some(n) = new {
                            ops.push(n);
                        }
                        continue;
                    }
                }
                _ => {}
            }
            ops.push(op);
        }
        if !path.is_empty() {
            ops.append(&mut path);
        }
        out.streams.push(changed.then(|| serialize_ops(&ops)));
    }
    out
}

/// Lay out a shown string (or `TJ` array) glyph by glyph, advancing `tm`. Returns the `TJ`
/// items with the glyphs under a region replaced by displacements, and how many were removed.
fn show(scope: &Scope<'_>, gs: &Gs, tm: &mut Matrix, items: &[Object]) -> (Vec<Object>, usize) {
    let f = &gs.font;
    let size = gs.size;
    let mut out: Vec<Object> = Vec::new();
    let mut removed = 0;
    let push_num = |out: &mut Vec<Object>, v: f64| {
        if v == 0.0 {
            return;
        }
        if let Some(last) = out.last_mut()
            && let Some(prev) = last.as_f64()
        {
            *last = num(prev + v);
            return;
        }
        out.push(num(v));
    };
    for item in items {
        match item {
            Object::String(s) => {
                let bytes = &s.bytes;
                let mut glyphs = Vec::new();
                let mut pos = 0;
                for (code, len) in f.codes(bytes) {
                    let w0 = f.width(code);
                    let spacing = gs.char_spacing + if f.is_space(code, len) { gs.word_spacing } else { 0.0 };
                    let trm = Matrix([size * gs.scale, 0.0, 0.0, size, 0.0, gs.rise]).then(tm).then(&gs.ctm);
                    let b = trm.bbox([0.0, f.descent, w0, f.ascent]);
                    let origin = trm.apply(0.0, 0.0);
                    let advance = w0 * size + spacing;
                    let hit = match scope.hidden_text {
                        // Invisible (Tr 3) and clip-only (Tr 7) text, or text wholly off the page.
                        Some(page) => {
                            let degenerate = b[2] - b[0] < 0.01 && b[3] - b[1] < 0.01;
                            matches!(gs.render_mode, 3 | 7) || !(overlaps(page, b, 0.0) || degenerate)
                        }
                        None => scope.glyph_hit(b, origin),
                    };
                    glyphs.push(Glyph { bytes: pos..pos + len, advance, hit });
                    *tm = Matrix::translate(advance * gs.scale, 0.0).then(tm);
                    pos += len;
                }
                let mut run: Vec<u8> = Vec::new();
                for g in &glyphs {
                    if g.hit {
                        removed += 1;
                        if !run.is_empty() {
                            out.push(string(std::mem::take(&mut run)));
                        }
                        // A TJ number moves by -n/1000 × size (× Th): the removed advance.
                        if size != 0.0 {
                            push_num(&mut out, -g.advance * 1000.0 / size);
                        }
                    } else {
                        run.extend_from_slice(&bytes[g.bytes.clone()]);
                    }
                }
                if !run.is_empty() {
                    out.push(string(run));
                }
            }
            other => {
                if let Some(v) = other.as_f64() {
                    *tm = Matrix::translate(-v / 1000.0 * size * gs.scale, 0.0).then(tm);
                    push_num(&mut out, v);
                }
            }
        }
    }
    (out, removed)
}

/// `Do`: `None` = leave the operator alone; `Some(None)` = drop it; `Some(Some(op))` = replace it.
fn xobject(
    doc: &mut Document,
    scope: &mut Scope<'_>,
    op: &Op,
    gs: &Gs,
    resources: &Dict,
    xobjects: &mut Dict,
    out: &mut Output,
) -> Option<Option<Op>> {
    let name = op.name(0)?.to_vec();
    let r = xobjects.get(&name)?.as_ref()?;
    let obj = doc.get(r);
    let Object::Stream(s) = &*obj else { return None };
    match s.dict.name(b"Subtype") {
        _ if s.dict.get(b"OC").is_some_and(|oc| scope.layer_hidden(doc, oc)) => {
            if scope.mode == Mode::Verify {
                return None;
            }
            scope.layer_blocks += 1;
            Some(None)
        }
        Some(b"Image") => {
            let b = gs.ctm.bbox([0.0, 0.0, 1.0, 1.0]);
            if !scope.hits(b) || scope.mode == Mode::Verify {
                return None;
            }
            if scope.covered(b) {
                scope.report.images_removed += 1;
                return Some(None);
            }
            match image::clear(doc, s, &gs.ctm, scope.rects) {
                Ok(None) => None,
                Ok(Some(new)) => {
                    let nr = doc.add(Object::Stream(new));
                    let n = fresh_name(scope, xobjects);
                    xobjects.set(n.clone(), Object::Ref(nr));
                    out.xobjects.push((n.clone(), nr));
                    scope.report.images_cleared += 1;
                    Some(Some(Op::new("Do", vec![Object::Name(n)])))
                }
                Err(()) => {
                    scope.report.images_removed += 1;
                    Some(None)
                }
            }
        }
        Some(b"Form") => {
            let m = s.dict.get(b"Matrix").and_then(|m| Matrix::from_operands(doc.resolve(m).as_array()?)).unwrap_or_default();
            let fctm = m.then(&gs.ctm);
            let bbox = s.dict.get(b"BBox").and_then(|b| {
                let v: Vec<f64> = doc.resolve(b).as_array()?.iter().filter_map(Object::as_f64).collect();
                (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
            });
            if let Some(bb) = bbox
                && !scope.everywhere()
                && !scope.hits(fctm.bbox(bb))
            {
                return None;
            }
            if scope.depth >= MAX_DEPTH {
                if scope.mode == Mode::Verify {
                    return None;
                }
                scope.report.forms_removed += 1;
                return Some(None);
            }
            let data = match s.decoded() {
                Ok(d) => d,
                Err(_) if scope.mode == Mode::Apply => {
                    scope.report.forms_removed += 1;
                    return Some(None);
                }
                Err(_) => return None,
            };
            let own = s.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned());
            let res = own.clone().unwrap_or_else(|| resources.clone());
            let saved_fonts = std::mem::take(&mut scope.fonts);
            scope.depth += 1;
            let inner = process(doc, scope, &[data], &res, fctm);
            scope.depth -= 1;
            scope.fonts = saved_fonts;
            out.residue += inner.residue;
            if scope.mode == Mode::Verify {
                return None;
            }
            let new_data = inner.streams.into_iter().next().flatten();
            if new_data.is_none() && inner.xobjects.is_empty() {
                return None;
            }
            let mut dict = s.dict.clone();
            dict.remove(b"Length");
            if !inner.xobjects.is_empty() {
                let mut res = res;
                let mut xo = res_dict(doc, &res, b"XObject");
                for (n, r) in &inner.xobjects {
                    xo.set(n.clone(), Object::Ref(*r));
                }
                res.set(b"XObject".to_vec(), Object::Dict(xo));
                dict.set(b"Resources".to_vec(), Object::Dict(res));
            }
            let stream = match new_data {
                Some(d) => Stream::flate(dict, &d),
                None => Stream { dict, raw: s.raw.clone() },
            };
            let nr = doc.add(Object::Stream(stream));
            let n = fresh_name(scope, xobjects);
            xobjects.set(n.clone(), Object::Ref(nr));
            out.xobjects.push((n.clone(), nr));
            scope.report.forms_rewritten += 1;
            Some(Some(Op::new("Do", vec![Object::Name(n)])))
        }
        _ => None,
    }
}

/// Remove marked-content blocks of hidden layers (`/OC /name BDC … EMC`, nested blocks
/// included). Returns the remaining operators and how many blocks went.
fn strip_hidden_layers(doc: &Document, scope: &Scope<'_>, ops: Vec<Op>, props: &Dict) -> (Vec<Op>, usize) {
    let mut out = Vec::with_capacity(ops.len());
    // For each open marked-content block: does it hide its contents?
    let mut stack: Vec<bool> = Vec::new();
    let mut removed = 0;
    for op in ops {
        let hidden_now = stack.last().copied().unwrap_or(false);
        match op.op.as_slice() {
            b"BDC" | b"BMC" => {
                let this = op.is("BDC")
                    && op.name(0) == Some(b"OC")
                    && match op.operands.get(1) {
                        Some(Object::Name(n)) => props.get(n).is_some_and(|o| scope.layer_hidden(doc, o)),
                        Some(o) => scope.layer_hidden(doc, o),
                        None => false,
                    };
                if this && !hidden_now {
                    removed += 1;
                }
                stack.push(hidden_now || this);
                if !(hidden_now || this) {
                    out.push(op);
                }
            }
            b"EMC" => {
                let was = stack.pop().unwrap_or(false);
                if !was {
                    out.push(op);
                }
            }
            _ => {
                if !hidden_now {
                    out.push(op);
                }
            }
        }
    }
    (out, removed)
}
