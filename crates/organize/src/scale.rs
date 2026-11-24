//! Scale page contents to a new page size, to fit inside it or to fill it.
//!
//! Each page's visible area (its crop box clipped to its media box, ISO 32000-2 §14.11.2) is
//! mapped uniformly onto a `[0 0 w h]` page by `x' = s·x + dx`, `y' = s·y + dy`. The content
//! streams are wrapped, not rewritten: a stream with `q`, the `cm` and a clip to the old visible
//! area goes before them, and one with `Q` after (§8.4.2, §8.4.4, §8.5.4). Annotation geometry
//! (§12.5.2, §12.5.6), viewports (§12.9.1), article beads (§12.4.3) and explicit destinations
//! (§12.3.2.2) that name a scaled page move with the content; `/Measure` conversion factors are
//! rescaled so measurements still read true (§12.9). Geospatial measures are left as they are
//! and counted. A page's patterns map to its default space, not the current transformation
//! (§8.7.3.1), so the `cm` doesn't reach them: each page gets copies with the placement composed
//! into their `/Matrix`.
//!
//! Adapted from pdfcer (MIT, same author) `pageops/scale.rs` and `EditSession::scale_pages`.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::{OrganizeError, boxes::page_boxes, check, resolved_rotation, walk};

/// How content meets a page size whose proportions differ from the page's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScaleMode {
    /// All of the content shows; the spare room is split evenly on both sides.
    #[default]
    Fit,
    /// The content covers the page; what overflows is outside the new boxes (not removed).
    Fill,
}

/// How the size is turned against each page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScaleOrientation {
    /// Swap width and height when that keeps the page's own orientation (portrait stays
    /// portrait). Square pages and square sizes are never swapped.
    #[default]
    Match,
    /// Use the size as given.
    Exact,
}

/// A new page size, as the page is shown (after `/Rotate`), in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleRequest {
    pub width: f64,
    pub height: f64,
    pub mode: ScaleMode,
    pub orientation: ScaleOrientation,
}

/// What [`scale_pages`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScaleReport {
    /// Each scaled page (0-based, ascending) with the factor its content was scaled by.
    pub pages: Vec<(usize, f64)>,
    /// Annotations whose geometry moved.
    pub annotations: usize,
    /// `/Measure` dictionaries rescaled.
    pub measures: usize,
    /// Explicit destinations naming a scaled page whose coordinates moved.
    pub destinations: usize,
    /// Geospatial `/Measure` dictionaries (`/Subtype /GEO`) left unchanged: they register points
    /// to coordinates rather than carry a factor, and are not rescaled.
    pub geo_measures_unchanged: usize,
    /// Fill patterns left unplaced on a scaled page (an unreadable `/Matrix`, or past the
    /// per-page cap), so they no longer line up with the content.
    pub patterns_unplaced: usize,
}

/// Where one page's content lands, in its unrotated user space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    scale: f64,
    dx: f64,
    dy: f64,
    /// The new page, `[0 0 w h]`.
    size: [f64; 2],
}

/// Most patterns copied for one page.
const MAX_PATTERNS: usize = 4096;

impl Placement {
    fn x(&self, x: f64) -> Option<f64> {
        Some(self.scale * x + self.dx).filter(|v| v.is_finite())
    }

    fn y(&self, y: f64) -> Option<f64> {
        Some(self.scale * y + self.dy).filter(|v| v.is_finite())
    }

    /// Identifies the placement, for copies made per placement.
    fn key(&self) -> [u64; 3] {
        [self.scale, self.dx, self.dy].map(f64::to_bits)
    }

    fn rect(&self, r: [f64; 4]) -> Option<[f64; 4]> {
        let (x0, y0, x1, y1) = (self.x(r[0])?, self.y(r[1])?, self.x(r[2])?, self.y(r[3])?);
        Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
    }
}

/// Plan where `source` lands on a page shown at `rotate` degrees, `width` × `height` as shown.
/// `None` unless both sizes are finite and positive and the placement is finite and within the
/// range of a PDF real (Annex C).
fn plan(source: [f64; 4], rotate: i64, width: f64, height: f64, req: &ScaleRequest) -> Option<Placement> {
    let (sw, sh) = (source[2] - source[0], source[3] - source[1]);
    let sane = |v: f64| v.is_finite() && v > 0.0;
    if ![sw, sh, width, height].into_iter().all(sane) {
        return None;
    }
    let quarter = rotate.rem_euclid(180) == 90;
    let (shown_w, shown_h) = if quarter { (sh, sw) } else { (sw, sh) };
    let (mut tw, mut th) = (width, height);
    if req.orientation == ScaleOrientation::Match && shown_w != shown_h && tw != th && (shown_w > shown_h) != (tw > th) {
        std::mem::swap(&mut tw, &mut th);
    }
    // Back to unrotated user space: a quarter turn swaps the axes.
    let (uw, uh) = if quarter { (th, tw) } else { (tw, th) };
    let (fx, fy) = (uw / sw, uh / sh);
    let scale = match req.mode {
        ScaleMode::Fit => fx.min(fy),
        ScaleMode::Fill => fx.max(fy),
    };
    let p = Placement { scale, dx: (uw - scale * sw) / 2.0 - scale * source[0], dy: (uh - scale * sh) / 2.0 - scale * source[1], size: [uw, uh] };
    let real = |v: f64| v.is_finite() && v.abs() <= f64::from(f32::MAX);
    [p.scale, p.dx, p.dy].into_iter().all(real).then_some(p).filter(|p| p.scale > 0.0)
}

/// A content-stream number, in plain decimal with every digit needed to read back the same value.
fn num(v: f64) -> String {
    if v == 0.0 { "0".into() } else { format!("{v}") }
}

/// The stream before the page's own: save the state, place, clip to the old visible area.
fn prefix(source: [f64; 4], p: &Placement) -> Vec<u8> {
    let s = num(p.scale);
    let (w, h) = (source[2] - source[0], source[3] - source[1]);
    format!("q\n{s} 0 0 {s} {} {} cm\n{} {} {} {} re W n\n", num(p.dx), num(p.dy), num(source[0]), num(source[1]), num(w), num(h)).into_bytes()
}

/// `m` (a pattern's `/Matrix`, identity when absent) followed by the placement.
fn placed_matrix(doc: &Document, m: Option<&Object>, p: &Placement) -> Option<Object> {
    let m = match m.map(|m| doc.resolve(m)) {
        None => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        Some(m) => {
            let items = m.as_array()?;
            let v: Vec<f64> = items.iter().filter_map(|o| number(doc, o)).collect();
            <[f64; 6]>::try_from(v.as_slice()).ok().filter(|_| items.len() == 6)?
        }
    };
    let s = p.scale;
    let out = [m[0] * s, m[1] * s, m[2] * s, m[3] * s, m[4] * s + p.dx, m[5] * s + p.dy];
    out.iter().all(|v| v.is_finite()).then(|| reals(out))
}

/// The page's resources with every pattern replaced by a placed copy, or `None` when it has no
/// patterns. A pattern shared with the same placement is copied once.
fn placed_resources(doc: &mut Document, resources: &Object, p: &Placement, s: &mut Scaling) -> Option<Object> {
    let mut res = doc.resolve(resources).as_dict()?.clone();
    let patterns = doc.resolve(res.get(b"Pattern")?).as_dict()?.clone();
    // A pattern that can't be placed (an unreadable `/Matrix`) is kept as it was, and counted.
    let mut placed = patterns.clone();
    s.report.patterns_unplaced += patterns.len().saturating_sub(MAX_PATTERNS);
    for (name, pattern) in patterns.iter().take(MAX_PATTERNS) {
        let key = pattern.as_ref().map(|r| (r, p.key()));
        if let Some(copy) = key.and_then(|k| s.copies.get(&k)) {
            placed.set(name.clone(), Object::Ref(*copy));
            continue;
        }
        let copy = match &*doc.resolve(pattern) {
            Object::Stream(st) => {
                let mut st = st.clone();
                let Some(m) = placed_matrix(doc, st.dict.get(b"Matrix"), p) else {
                    s.report.patterns_unplaced += 1;
                    continue;
                };
                st.dict.set(b"Matrix".to_vec(), m);
                Object::Stream(st)
            }
            Object::Dict(d) => {
                let mut d = d.clone();
                let Some(m) = placed_matrix(doc, d.get(b"Matrix"), p) else {
                    s.report.patterns_unplaced += 1;
                    continue;
                };
                d.set(b"Matrix".to_vec(), m);
                Object::Dict(d)
            }
            _ => {
                s.report.patterns_unplaced += 1;
                continue;
            }
        };
        let r = doc.add(copy);
        if let Some(k) = key {
            s.copies.insert(k, r);
        }
        placed.set(name.clone(), Object::Ref(r));
    }
    res.set(b"Pattern".to_vec(), Object::Dict(placed));
    Some(Object::Dict(res))
}

/// The stream after the page's own. The leading newline keeps a last token from fusing with `Q`.
const SUFFIX: &[u8] = b"\nQ\n";

fn number(doc: &Document, o: &Object) -> Option<f64> {
    doc.resolve(o).as_f64().filter(|v| v.is_finite())
}

fn reals(v: impl IntoIterator<Item = f64>) -> Object {
    Object::Array(v.into_iter().map(Object::Real).collect())
}

/// A 4-number rectangle, normalized.
fn rect(doc: &Document, o: &Object) -> Option<[f64; 4]> {
    let o = doc.resolve(o);
    match o.as_array()?.as_slice() {
        [a, b, c, d] => {
            let [a, b, c, d] = [number(doc, a)?, number(doc, b)?, number(doc, c)?, number(doc, d)?];
            Some([a.min(c), b.min(d), a.max(c), b.max(d)])
        }
        _ => None,
    }
}

/// Map a flat `[x1 y1 x2 y2 …]` array; `None` if it has an odd length or a non-number.
fn pairs(doc: &Document, o: &Object, p: &Placement) -> Option<Object> {
    let o = doc.resolve(o);
    let items = o.as_array()?;
    if items.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(items.len());
    for [x, y] in items.as_chunks::<2>().0 {
        out.push(p.x(number(doc, x)?)?);
        out.push(p.y(number(doc, y)?)?);
    }
    Some(reals(out))
}

/// `/F` bit 4, NoZoom (§12.5.3): the annotation keeps its size, its upper-left corner fixed. A text
/// annotation always behaves as NoZoom (§12.5.6.4).
fn no_zoom(doc: &Document, annot: &Dict) -> bool {
    annot.name(b"Subtype") == Some(b"Text") || annot.get(b"F").and_then(|f| doc.resolve(f).as_int()).is_some_and(|f| f & 8 != 0)
}

/// Move one annotation's geometry; `None` when it has none recognised. `/Rect`, the point arrays
/// `/QuadPoints`, `/Vertices`, `/L`, `/CL`, the arrays of point arrays `/InkList` and `/Path`, and
/// the lengths `/RD`, `/LL`, `/LLE`, `/LLO`. A NoZoom annotation is moved by its upper-left
/// corner and keeps its size: its points move with that corner and its lengths stay. An
/// appearance follows its `/Rect` by itself (§12.5.5).
fn move_annotation(doc: &Document, annot: &Dict, p: &Placement) -> Option<Dict> {
    let mut out = annot.clone();
    let mut changed = false;
    let fixed = no_zoom(doc, annot);
    let mut points = *p;
    if let Some(r) = annot.get(b"Rect").and_then(|o| rect(doc, o)) {
        let moved = if fixed {
            let (x, y) = (p.x(r[0])?, p.y(r[3])?);
            points = Placement { scale: 1.0, dx: x - r[0], dy: y - r[3], size: p.size };
            Some([x, y - (r[3] - r[1]), x + (r[2] - r[0]), y])
        } else {
            p.rect(r)
        };
        if let Some(m) = moved.filter(|m| m.iter().all(|v| v.is_finite())) {
            out.set(b"Rect".to_vec(), reals(m));
            changed = true;
        }
    } else if fixed {
        return None;
    }
    let p = &points;
    for key in [&b"QuadPoints"[..], b"Vertices", b"L", b"CL"] {
        if let Some(m) = annot.get(key).and_then(|o| pairs(doc, o, p)) {
            out.set(key.to_vec(), m);
            changed = true;
        }
    }
    for key in [&b"InkList"[..], b"Path"] {
        let strokes = annot.get(key).map(|o| doc.resolve(o));
        let mapped: Option<Vec<Object>> = strokes.as_deref().and_then(Object::as_array).and_then(|s| s.iter().map(|s| pairs(doc, s, p)).collect());
        if let Some(m) = mapped {
            out.set(key.to_vec(), Object::Array(m));
            changed = true;
        }
    }
    let scaled = |o: &Object| number(doc, o).map(|v| v * p.scale).filter(|v| v.is_finite());
    let rd = annot.get(b"RD").map(|o| doc.resolve(o));
    if let Some(m) = rd.as_deref().and_then(Object::as_array).and_then(|a| a.iter().map(scaled).collect::<Option<Vec<f64>>>()) {
        out.set(b"RD".to_vec(), reals(m));
        changed = true;
    }
    for key in [&b"LL"[..], b"LLE", b"LLO"] {
        if let Some(v) = annot.get(key).and_then(scaled) {
            out.set(key.to_vec(), Object::Real(v));
            changed = true;
        }
    }
    changed.then_some(out)
}

/// What became of a `/Measure` dictionary.
enum Measured {
    Geo,
    Same,
    Changed(Dict),
}

/// Rescale a rectilinear `/Measure` (§12.9): only the first number format of `/X` and `/Y`
/// converts from user space, so its `/C` is divided by the scale; `/D` and `/A` convert from
/// `/X`'s units and stay, as do angles and slopes. An explicit origin `/O` is a point in user
/// space and moves with the content.
fn rescale_measure(doc: &Document, measure: &Dict, p: &Placement) -> Measured {
    if measure.get(b"Subtype").is_some_and(|s| doc.resolve(s).as_name() == Some(b"GEO")) {
        return Measured::Geo;
    }
    let mut out = measure.clone();
    let mut changed = false;
    let origin = measure.get(b"O").filter(|o| doc.resolve(o).as_array().is_some_and(|a| a.len() == 2));
    if let Some(o) = origin.and_then(|o| pairs(doc, o, p)) {
        out.set(b"O".to_vec(), o);
        changed = true;
    }
    for key in [&b"X"[..], b"Y"] {
        let factor = p.scale;
        let Some(arr) = measure.get(key).map(|o| doc.resolve(o)) else { continue };
        let Some(items) = arr.as_array() else { continue };
        let Some(first) = items.first().map(|o| doc.resolve(o)) else { continue };
        let Some(format) = first.as_dict() else { continue };
        let Some(c) = format.get(b"C").and_then(|o| number(doc, o)).map(|c| c / factor).filter(|c| c.is_finite()) else { continue };
        let mut format = format.clone();
        format.set(b"C".to_vec(), Object::Real(c));
        let mut items = items.clone();
        if let Some(slot) = items.first_mut() {
            *slot = Object::Dict(format);
        }
        out.set(key.to_vec(), Object::Array(items));
        changed = true;
    }
    if changed { Measured::Changed(out) } else { Measured::Same }
}

/// How [`Scaling::move_list`] treats an indirect item that other pages may share.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shared {
    /// Moved in place, once: annotations and beads, which other objects point at.
    InPlace,
    /// Replaced on this page by a copy per placement: viewports, which only `/VP` arrays hold.
    Copied,
}

/// The edit in progress: objects already moved (shared ones move once), the copies made per
/// placement, and the counts.
struct Scaling {
    seen: HashSet<ObjRef>,
    /// Placed copies of shared patterns, measures and viewports, by original and placement.
    copies: HashMap<(ObjRef, [u64; 3]), ObjRef>,
    report: ScaleReport,
}

impl Scaling {
    /// Rescale the `/Measure` held in `holder`. A direct one is replaced in `holder`; an indirect
    /// one is left for the pages that still use it, and `holder` gets a copy per placement.
    fn measure(&mut self, doc: &mut Document, holder: &mut Dict, p: &Placement) {
        let Some(m) = holder.get(b"Measure").cloned() else { return };
        let key = m.as_ref().map(|r| (r, p.key()));
        if let Some(copy) = key.and_then(|k| self.copies.get(&k)) {
            holder.set(b"Measure".to_vec(), Object::Ref(*copy));
            return;
        }
        let resolved = doc.resolve(&m);
        let Object::Dict(md) = &*resolved else { return };
        match rescale_measure(doc, md, p) {
            Measured::Geo => {
                if m.as_ref().is_none_or(|r| self.seen.insert(r)) {
                    self.report.geo_measures_unchanged += 1;
                }
            }
            Measured::Same => {}
            Measured::Changed(new) => {
                self.report.measures += 1;
                let new = match key {
                    Some(k) => {
                        let copy = doc.add(Object::Dict(new));
                        self.copies.insert(k, copy);
                        Object::Ref(copy)
                    }
                    None => Object::Dict(new),
                };
                holder.set(b"Measure".to_vec(), new);
            }
        }
    }

    /// Move each dictionary in the array at `page[key]` with `f`. An indirect item is moved in
    /// place or copied, as `shared` says; a changed direct item or a copy goes into the array,
    /// and a changed array onto this page alone.
    fn move_list(
        &mut self,
        doc: &mut Document,
        page: &mut Dict,
        key: &[u8],
        p: &Placement,
        shared: Shared,
        f: fn(&Document, &Dict, &Placement) -> Option<Dict>,
    ) -> Result<usize, OrganizeError> {
        let Some(held) = page.get(key).cloned() else { return Ok(0) };
        let Some(mut items) = doc.resolve(&held).as_array().cloned() else { return Ok(0) };
        let (mut moved, mut rewired) = (0, false);
        for item in &mut items {
            let r = item.as_ref();
            if shared == Shared::InPlace && r.is_some_and(|r| !self.seen.insert(r)) {
                continue;
            }
            if let Some(copy) = r.and_then(|r| self.copies.get(&(r, p.key()))) {
                *item = Object::Ref(*copy);
                rewired = true;
                continue;
            }
            let resolved = doc.resolve(item);
            let Object::Dict(d) = &*resolved else { continue };
            let mut out = f(doc, d, p);
            if out.is_some() {
                moved += 1;
            }
            let mut holder = out.take().unwrap_or_else(|| d.clone());
            self.measure(doc, &mut holder, p);
            if holder == *d {
                continue;
            }
            match r {
                Some(r) if shared == Shared::InPlace => doc.set_existing(r, Object::Dict(holder))?,
                Some(r) => {
                    let copy = doc.add(Object::Dict(holder));
                    self.copies.insert((r, p.key()), copy);
                    *item = Object::Ref(copy);
                    rewired = true;
                }
                None => {
                    *item = Object::Dict(holder);
                    rewired = true;
                }
            }
        }
        if rewired {
            page.set(key.to_vec(), Object::Array(items));
        }
        Ok(moved)
    }
}

/// A viewport's `/BBox` keeps its corner order, which sets the measuring axes' directions (§12.9).
fn move_viewport(doc: &Document, d: &Dict, p: &Placement) -> Option<Dict> {
    let bbox = d.get(b"BBox")?;
    if doc.resolve(bbox).as_array()?.len() != 4 {
        return None;
    }
    let mut out = d.clone();
    out.set(b"BBox".to_vec(), pairs(doc, bbox, p)?);
    Some(out)
}

fn move_bead(doc: &Document, d: &Dict, p: &Placement) -> Option<Dict> {
    let r = p.rect(rect(doc, d.get(b"R")?)?)?;
    let mut out = d.clone();
    out.set(b"R".to_vec(), reals(r));
    Some(out)
}

/// The page's content streams (a stream, an array, or a reference to an array, §7.7.3.3),
/// wrapped between the placing stream and the restoring one. `None` for a page without content.
fn wrapped_contents(doc: &mut Document, page: &Dict, source: [f64; 4], p: &Placement) -> Option<Object> {
    let existing = match page.get(b"Contents")? {
        Object::Array(items) => items.clone(),
        c @ Object::Ref(_) => match &*doc.resolve(c) {
            Object::Array(items) => items.clone(),
            Object::Stream(_) => vec![c.clone()],
            _ => return None,
        },
        _ => return None,
    };
    if existing.is_empty() {
        return None;
    }
    let pre = doc.add(Object::Stream(Stream::flate(Dict::new(), &prefix(source, p))));
    let post = doc.add(Object::Stream(Stream::flate(Dict::new(), SUFFIX)));
    let mut array = Vec::with_capacity(existing.len().saturating_add(2));
    array.push(Object::Ref(pre));
    array.extend(existing);
    array.push(Object::Ref(post));
    Some(Object::Array(array))
}

/// Explicit destination kinds and, for each operand after the kind, its axis (§12.3.2.2).
fn dest_axes(kind: &[u8]) -> Option<&'static [u8]> {
    match kind {
        b"XYZ" => Some(b"xy"),
        b"FitH" | b"FitBH" => Some(b"y"),
        b"FitV" | b"FitBV" => Some(b"x"),
        b"FitR" => Some(b"xyxy"),
        _ => None,
    }
}

/// An array shaped as an explicit destination: a page reference, then a destination kind.
fn is_destination(doc: &Document, items: &[Object]) -> bool {
    let kind = items.get(1).map(|k| doc.resolve(k));
    let fits = |k: &[u8]| dest_axes(k).is_some() || matches!(k, b"Fit" | b"FitB");
    items.first().and_then(Object::as_ref).is_some() && kind.as_deref().and_then(Object::as_name).is_some_and(fits)
}

/// Move an explicit destination's coordinates, read through references and written in place in
/// the array; `null` operands and `/XYZ`'s zoom stay.
fn move_destination(doc: &Document, items: &[Object], p: &Placement) -> Option<Vec<Object>> {
    let kind = doc.resolve(items.get(1)?);
    let axes = dest_axes(kind.as_name()?)?;
    let mut out = items.to_vec();
    let mut changed = false;
    for (slot, axis) in out.iter_mut().skip(2).zip(axes) {
        let Some(v) = number(doc, slot) else { continue };
        if let Some(m) = if *axis == b'x' { p.x(v) } else { p.y(v) } {
            *slot = Object::Real(m);
            changed = true;
        }
    }
    changed.then_some(out)
}

/// How deep [`DestScan::walk`] looks inside one object.
const MAX_DEST_DEPTH: usize = 32;

/// What a value is, by where it sits (§12.3.2).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// Looked through for the entries below.
    Other,
    /// A destination: a `/Dest`, a GoTo `/D`, the `/OpenAction`, or a named destination's value.
    Dest,
    /// The catalog's `/Dests` dictionary, or a node of the `/Dests` name tree.
    Dests,
}

/// The context of the value under `key` in a dictionary whose own context is `parent`.
fn child(parent: Ctx, key: &[u8]) -> Ctx {
    match key {
        b"Dest" | b"D" | b"OpenAction" => Ctx::Dest,
        b"Dests" => Ctx::Dests,
        b"Kids" | b"Names" if parent == Ctx::Dests => Ctx::Dests,
        b"Limits" => Ctx::Other,
        _ if parent == Ctx::Dests => Ctx::Dest,
        _ => Ctx::Other,
    }
}

/// Finds the explicit destinations naming a scaled page, by context: arrays elsewhere, such as
/// private application data, are left alone.
struct DestScan<'a> {
    doc: &'a Document,
    pages: &'a HashMap<ObjRef, Placement>,
    count: usize,
    /// Objects referenced from a destination context, to be read in it.
    pending: Vec<(ObjRef, Ctx)>,
}

impl DestScan<'_> {
    /// `obj` with the destinations in it moved, or `None` when nothing changed.
    fn walk(&mut self, obj: &Object, ctx: Ctx, depth: usize) -> Option<Object> {
        if depth > MAX_DEST_DEPTH {
            return None;
        }
        match obj {
            Object::Ref(r) if ctx != Ctx::Other => {
                self.pending.push((*r, ctx));
                None
            }
            Object::Array(items) if ctx != Ctx::Other && is_destination(self.doc, items) => {
                let p = items.first().and_then(Object::as_ref).and_then(|r| self.pages.get(&r))?;
                let out = move_destination(self.doc, items, p)?;
                self.count += 1;
                Some(Object::Array(out))
            }
            Object::Array(_) if ctx == Ctx::Dest => None,
            Object::Array(items) => {
                let mut out: Option<Vec<Object>> = None;
                for (i, item) in items.iter().enumerate() {
                    if let Some(new) = self.walk(item, ctx, depth + 1)
                        && let Some(slot) = out.get_or_insert_with(|| items.clone()).get_mut(i)
                    {
                        *slot = new;
                    }
                }
                out.map(Object::Array)
            }
            Object::Dict(d) => {
                let mut out: Option<Dict> = None;
                for (key, value) in d.iter() {
                    if let Some(new) = self.walk(value, child(ctx, key), depth + 1) {
                        out.get_or_insert_with(|| d.clone()).set(key.clone(), new);
                    }
                }
                out.map(Object::Dict)
            }
            _ => None,
        }
    }
}

/// Scale the content of `pages` (0-based) to `request`'s size: to fit inside it or to fill it,
/// centred. Every page box present on a page is set to the new page; the crop box is set when
/// the page has or inherits one. Sizes are in points as the page is shown, so a turned page and
/// a `/UserUnit` page get the size asked for. Everything is checked before anything is written.
pub fn scale_pages(doc: &mut Document, pages: &[usize], request: ScaleRequest) -> Result<ScaleReport, OrganizeError> {
    if ![request.width, request.height].iter().all(|v| v.is_finite() && *v >= 1.0) {
        return Err(OrganizeError::InvalidBox("the new page size must be at least a point each way".into()));
    }
    let all = walk(doc)?;
    check(pages, all.len())?;
    let boxes = page_boxes(doc)?;
    let mut targets = pages.to_vec();
    targets.sort_unstable();
    targets.dedup();
    let mut plans = Vec::with_capacity(targets.len());
    for &i in &targets {
        let (page, inherited) = &all[i];
        let source = boxes[i][1];
        let own = doc.get(*page);
        let unit = own.as_dict().and_then(|d| d.get(b"UserUnit")).and_then(|u| number(doc, u)).filter(|u| *u > 0.0).unwrap_or(1.0);
        let placement = plan(source, resolved_rotation(doc, inherited), request.width / unit, request.height / unit, &request)
            .ok_or_else(|| OrganizeError::InvalidBox(format!("page {} can't be scaled to that size", i + 1)))?;
        plans.push((i, *page, inherited.contains(b"CropBox"), inherited.get(b"Resources").cloned(), source, placement));
    }
    let mut s = Scaling { seen: HashSet::new(), copies: HashMap::new(), report: ScaleReport::default() };
    let mut placed = HashMap::new();
    for (i, page, cropped, resources, source, p) in &plans {
        let (i, page, p) = (*i, *page, *p);
        scale_page(doc, page, *cropped, resources.as_ref(), *source, &p, &mut s)?;
        placed.insert(page, p);
        s.report.pages.push((i, p.scale));
    }
    s.report.destinations = rewrite_destinations(doc, &placed)?;
    Ok(s.report)
}

/// Write one planned page: its boxes, patterns, annotations, viewports and beads, and its contents.
fn scale_page(
    doc: &mut Document,
    page: ObjRef,
    cropped: bool,
    inherited: Option<&Object>,
    source: [f64; 4],
    p: &Placement,
    s: &mut Scaling,
) -> Result<(), OrganizeError> {
    let Some(mut d) = doc.get(page).as_dict().cloned() else { return Ok(()) };
    if let Some(res) = d.get(b"Resources").or(inherited).cloned()
        && let Some(placed) = placed_resources(doc, &res, p, s)
    {
        d.set(b"Resources".to_vec(), placed);
    }
    let target = reals([0.0, 0.0, p.size[0], p.size[1]]);
    d.set(b"MediaBox".to_vec(), target.clone());
    if cropped || d.contains(b"CropBox") {
        d.set(b"CropBox".to_vec(), target.clone());
    }
    for key in [&b"BleedBox"[..], b"TrimBox", b"ArtBox"] {
        if d.contains(key) {
            d.set(key.to_vec(), target.clone());
        }
    }
    s.report.annotations += s.move_list(doc, &mut d, b"Annots", p, Shared::InPlace, move_annotation)?;
    s.move_list(doc, &mut d, b"VP", p, Shared::Copied, move_viewport)?;
    s.move_list(doc, &mut d, b"B", p, Shared::InPlace, move_bead)?;
    if let Some(contents) = wrapped_contents(doc, &d, source, p) {
        d.set(b"Contents".to_vec(), contents);
    }
    doc.update_dict(page, |x| *x = d)?;
    Ok(())
}

/// Move the destinations naming a scaled page in every object but streams. Returns how many.
/// An object referenced from a destination context is read again in that context, and that
/// reading replaces the first.
fn rewrite_destinations(doc: &mut Document, placed: &HashMap<ObjRef, Placement>) -> Result<usize, OrganizeError> {
    let mut changed: HashMap<ObjRef, (Object, usize)> = HashMap::new();
    {
        let view: &Document = doc;
        let mut scan = DestScan { doc: view, pages: placed, count: 0, pending: Vec::new() };
        let mut read = |scan: &mut DestScan, r: ObjRef, o: &Object, ctx: Ctx| {
            let before = scan.count;
            if let Some(new) = scan.walk(o, ctx, 0) {
                changed.insert(r, (new, scan.count.saturating_sub(before)));
            }
        };
        for (r, o) in view.scan_objects().filter(|(_, o)| !matches!(&**o, Object::Stream(_))) {
            read(&mut scan, r, &o, Ctx::Other);
        }
        let mut done = HashSet::new();
        while let Some((r, ctx)) = scan.pending.pop() {
            let o = view.get(r);
            if done.insert(r) && !matches!(&*o, Object::Stream(_)) {
                read(&mut scan, r, &o, ctx);
            }
        }
    }
    let count = changed.values().map(|(_, n)| n).sum();
    for (r, (new, _)) in changed {
        doc.set_existing(r, new)?;
    }
    Ok(count)
}

#[cfg(test)]
#[path = "scale_tests.rs"]
mod tests;
