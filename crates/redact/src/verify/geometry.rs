//! The non-text content under the regions: images, paths and shadings, and the placement of the
//! glyphs still shown, checked independently of the interpreter.

use std::cell::Cell;
use std::collections::HashSet;

use pdfcraft_content::{Matrix, contains, overlaps, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use sha2::{Digest, Sha256};

use super::extract::{appearance_matrix, bbox_of, matrix_of, page_content, res_dict};
use super::util::{Ctx, image_codec};
use crate::verify::limits::{MAX_CLIP_SUBPATHS, MAX_DEPTH, MAX_GS_STACK, MAX_HOLES, MAX_PIXELS, MAX_SEARCHED, SEARCH_COST};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Finding {
    /// This many glyphs are placed inside the region.
    Text(usize),
    Image,
    Path,
    Shading,
    Missing,
    Unverifiable(&'static str),
}

#[derive(Clone)]
pub(super) struct Gs {
    pub(super) ctm: Matrix,
    /// Device-space boxes cut out of the clip (even-odd clips with a hole).
    pub(super) holes: Vec<[f64; 4]>,
}

pub(super) struct Geo<'a> {
    pub(super) doc: &'a Document,
    pub(super) ctx: &'a Ctx,
    pub(super) regions: &'a [[f64; 4]],
    pub(super) found: Vec<(usize, Finding)>,
    /// What `found` already holds: each finding is recorded once per region, so a page that
    /// paints thousands of times cannot grow the report (or the memory) with repeats.
    reported: HashSet<(usize, Finding)>,
    /// Patterns, soft-mask groups, Type3 glyphs and the forms they draw already searched for
    /// images, each with where it looked its names up (at most [`MAX_SEARCHED`]).
    pub(super) searched: HashSet<(ObjRef, Within)>,
}

/// Where a stream searched for images looks up the names it draws: in resources of its own, or
/// in the XObjects of the place it is drawn from, told apart by a digest of their names and the
/// objects they name. A Type 3 glyph shared by two fonts, or a pattern or form without resources
/// of its own, can draw an image from one place and not from another, so it is searched for each.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Within {
    Own,
    Caller([u8; 32]),
}

/// The key of a set of XObjects, once worked out: `Err` when the proof's work allowance ran out.
type Key = Result<Option<[u8; 32]>, ()>;

/// The XObjects a stream without resources of its own looks its names up in: a Type 3 font's own
/// (`font`), where they give a stream, then those of the place it is drawn from (`from`), each with
/// a slot for the key, worked out at the first search that needs it.
#[derive(Clone, Copy)]
struct Names<'a> {
    font: Option<(&'a Dict, &'a Cell<Option<Key>>)>,
    from: (&'a Dict, &'a Cell<Option<Key>>),
}

impl<'a> Names<'a> {
    fn get(&self, doc: &Document, name: &[u8]) -> Option<&'a Object> {
        if let Some(x) = self.font.and_then(|(font, _)| font.get(name))
            && matches!(&*doc.resolve(x), Object::Stream(_))
        {
            return Some(x);
        }
        self.from.0.get(name)
    }

    /// The key of these XObjects: that of `from`, or, with a font's, a digest of the key of the
    /// font's own and that of `from`. Each is worked out once, so a font's costs its own entries
    /// and not those of `from` again. `None` when either has none.
    fn key(&self, geo: &Geo<'_>) -> Key {
        let from = once(self.from.1, || geo.xobjects_key(self.from.0))?;
        let Some((font, slot)) = self.font else { return Ok(from) };
        once(slot, || {
            Ok(match (geo.xobjects_key(font)?, from) {
                (Some(own), Some(from)) => Some(Sha256::new().chain_update(own).chain_update(from).finalize().into()),
                _ => None,
            })
        })
    }
}

/// What `slot` holds, or what `work` gives, kept there.
fn once(slot: &Cell<Option<Key>>, work: impl FnOnce() -> Key) -> Key {
    if let Some(k) = slot.get() {
        return k;
    }
    let k = work();
    slot.set(Some(k));
    k
}

/// A dictionary written in place, or given by reference, read where it is held rather than
/// copied.
enum Held<'a> {
    In(&'a Dict),
    At(std::sync::Arc<Object>),
    Not,
}

impl<'a> Held<'a> {
    fn of(doc: &Document, o: Option<&'a Object>) -> Self {
        match o {
            Some(Object::Dict(d)) => Held::In(d),
            Some(o @ Object::Ref(_)) => Held::At(doc.resolve(o)),
            _ => Held::Not,
        }
    }

    fn dict(&self) -> Option<&Dict> {
        match self {
            Held::In(d) => Some(d),
            Held::At(o) => o.as_dict(),
            Held::Not => None,
        }
    }
}

/// What still paints inside `regions` on a page of the output (the overlay boxes excepted):
/// images with pixels there, paths and shadings not clipped away from there.
pub(super) fn check_geometry(doc: &Document, page: &Dict, regions: &[[f64; 4]], ctx: &Ctx) -> Vec<(usize, Finding)> {
    let unverifiable = |why: &'static str| (0..regions.len()).map(|i| (i, Finding::Unverifiable(why))).collect();
    // The redaction boxes are painted on purpose.
    let Ok(data) = page_content(doc, page, ctx, true) else { return unverifiable("the page content can't be read in full") };
    let mut geo = Geo { doc, ctx, regions, found: Vec::new(), reported: HashSet::new(), searched: HashSet::new() };
    let res = page.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    geo.run(&data, &res, Matrix::IDENTITY, Vec::new(), 0);
    geo.appearances(page, &res);
    geo.found
}

impl Geo<'_> {
    /// Record a finding for region `i`, once: a page that paints over a region a thousand
    /// times is one finding, not a thousand.
    fn report(&mut self, i: usize, f: Finding) {
        if self.reported.insert((i, f.clone())) {
            self.found.push((i, f));
        }
    }

    /// The appearances of the annotations still on the page, checked like the page itself: their
    /// content is placed with the appearance matrix (ISO 32000-2 §12.5.5) and swept for images,
    /// paths and shadings under the regions. An appearance that has content but can't be placed
    /// makes every region unverifiable: the redaction refuses such documents, so one in the
    /// output means what it paints can't be vouched for.
    fn appearances(&mut self, page: &Dict, res: &Dict) {
        let doc = self.doc;
        for a in crate::annots_of(doc, page) {
            // A direct dictionary has no reference of its own; it is checked like an indirect
            // one (what its appearance paints does not depend on how it is stored).
            let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
            // A redaction mark's appearance outlines the marked area on purpose (and applying
            // removes the mark whole): it is exempt like the boxes drawn over the regions.
            if crate::subtype(doc, &d).as_deref() == Some(b"Redact") {
                continue;
            }
            let Some(ap) = d.get(b"AP").and_then(|x| doc.resolve(x).as_dict().cloned()) else { continue };
            let rect = crate::rect_of(doc, d.get(b"Rect"));
            for (_, state) in ap.iter() {
                let s = doc.resolve(state);
                let streams: Vec<Stream> = match &*s {
                    Object::Stream(st) => vec![st.clone()],
                    Object::Dict(sub) => sub
                        .iter()
                        .filter_map(|(_, v)| match &*doc.resolve(v) {
                            Object::Stream(st) => Some(st.clone()),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                for st in streams {
                    // A stream shared by several annotations is checked at each annotation's
                    // own placement: where it lands comes from each `/Rect`, so the first
                    // placement tells nothing about the others. Repeated decodes spend the
                    // same bounded budget as everything else.
                    let Ok(data) = self.ctx.decode(self.doc, &st) else {
                        self.unverifiable("an annotation appearance can't be read in full");
                        continue;
                    };
                    if data.is_empty() {
                        continue;
                    }
                    let Some(ctm) = rect.as_ref().and_then(|r| appearance_matrix(doc, &st, r)) else {
                        self.unverifiable("an annotation appearance can't be placed, so what it paints can't be checked");
                        continue;
                    };
                    let own = st.dict.get(b"Resources").and_then(|x| doc.resolve(x).as_dict().cloned());
                    self.run(&data, own.as_ref().unwrap_or(res), ctm, Vec::new(), 1);
                }
            }
        }
    }

    fn touching(&self, b: [f64; 4]) -> Vec<usize> {
        (0..self.regions.len()).filter(|&i| self.regions.get(i).is_some_and(|r| overlaps(*r, b, 0.0))).collect()
    }

    /// Does something painting over `within` reach region `i` through the clip? Only the part of
    /// the region the painting could touch has to be cut out.
    fn unclipped(&self, i: usize, within: [f64; 4], gs: &Gs) -> bool {
        let Some(r) = self.regions.get(i) else { return false };
        let reach = [r[0].max(within[0]), r[1].max(within[1]), r[2].min(within[2]), r[3].min(within[3])];
        !gs.holes.iter().any(|h| contains(*h, reach, 0.01))
    }

    fn run(&mut self, data: &[u8], res: &Dict, ctm: Matrix, holes: Vec<[f64; 4]>, depth: usize) {
        if depth > MAX_DEPTH {
            for i in 0..self.regions.len() {
                self.report(i, Finding::Unverifiable("forms are nested too deep to check"));
            }
            return;
        }
        let xobjects = res_dict(self.doc, res, b"XObject");
        self.hidden_images(res, depth);
        let mut gs = Gs { ctm, holes };
        let mut stack: Vec<Gs> = Vec::new();
        // The current path in device space: its subpaths, and the clip operator it carries.
        let mut sub: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut clip: Option<bool> = None;
        for op in parse(data).ops {
            let o = op.op.as_slice();
            if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
                extend_path(&op, &gs.ctm, &mut sub, &mut clip);
                continue;
            }
            if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                self.paint_path(o, &mut gs, &sub, clip);
                sub.clear();
                clip = None;
                continue;
            }
            sub.clear();
            clip = None;
            match o {
                b"q" => {
                    // The saved state carries the holes with it, so an unbounded stack of them
                    // is unbounded memory: past the cap the walk says so instead of growing.
                    if stack.len() >= MAX_GS_STACK {
                        return self.unverifiable("the content stacks graphics states too deeply to check");
                    }
                    stack.push(gs.clone());
                }
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
                b"sh" => {
                    for i in 0..self.regions.len() {
                        if self.unclipped(i, [f64::MIN, f64::MIN, f64::MAX, f64::MAX], &gs) {
                            self.report(i, Finding::Shading);
                        }
                    }
                }
                b"BI" => {
                    for i in self.touching(gs.ctm.bbox([0.0, 0.0, 1.0, 1.0])) {
                        self.report(i, Finding::Image);
                    }
                }
                b"Do" => {
                    let Some(entry) = op.last_name().and_then(|n| xobjects.get(n)) else { continue };
                    let obj = self.doc.resolve(entry);
                    let Object::Stream(s) = &*obj else { continue };
                    match crate::subtype(self.doc, &s.dict).as_deref() {
                        Some(b"Image") => {
                            for i in self.touching(gs.ctm.bbox([0.0, 0.0, 1.0, 1.0])) {
                                let Some(region) = self.regions.get(i) else { continue };
                                match image_cleared(self.doc, self.ctx, s, &gs.ctm, *region) {
                                    Ok(true) => {}
                                    Ok(false) => self.report(i, Finding::Image),
                                    Err(()) => self.report(i, Finding::Unverifiable("an image under the region can't be inspected")),
                                }
                            }
                        }
                        Some(b"Form") => self.form(s, &gs, res, depth),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// A path-painting operator `o` ends the current path: report what it paints over a region,
    /// and let an even-odd clip add its holes to `gs`.
    fn paint_path(&mut self, o: &[u8], gs: &mut Gs, sub: &[Vec<(f64, f64)>], clip: Option<bool>) {
        let pts: Vec<(f64, f64)> = sub.iter().flatten().copied().collect();
        let bbox = pts.iter().fold(None, |b: Option<[f64; 4]>, &(u, v)| {
            Some(match b {
                None => [u, v, u, v],
                Some(b) => [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)],
            })
        });
        if o != b"n"
            && let Some(b) = bbox
        {
            for i in self.touching(b) {
                if self.unclipped(i, b, gs) {
                    self.report(i, Finding::Path);
                }
            }
        }
        // An even-odd clip whose subpaths are all axis-aligned rectangles lets through what an
        // odd number of the inner rectangles covers. Where a region lies under the outer box
        // and an odd number of inner ones, it is cut out of the clip; under an even number it
        // is painted and must be reported. Anything that is not a stock of disjoint rectangles
        // (a triangle, a rotated box, overlapping inners, too many of them) cannot be told
        // cheaply, so no hole is cut: what the clip lets through is then reported as paint
        // (fail closed). This is how redaction itself clips regions out of what it leaves.
        if clip != Some(true) {
            return;
        }
        let Some(rects) = sub.iter().map(|s| axis_aligned_rect(s)).collect::<Option<Vec<[f64; 4]>>>() else { return };
        if rects.len() < 2 || rects.len() > MAX_CLIP_SUBPATHS {
            return;
        }
        let (outer, inners) = (rects[0], &rects[1..]);
        // Inner rectangles must be disjoint, or their overlaps would change the parity the
        // reader draws with.
        let disjoint = inners.iter().enumerate().all(|(x, a)| inners.iter().skip(x + 1).all(|b| !overlaps(*a, *b, 0.0)));
        let regions: Vec<[f64; 4]> = self.regions.to_vec();
        let mut cut = Vec::new();
        for r in regions {
            if contains(outer, r, 0.0) && disjoint && inners.iter().filter(|b| contains(**b, r, 0.0)).count() % 2 == 1 {
                cut.push(r);
            }
        }
        if gs.holes.len() + cut.len() > MAX_HOLES {
            return self.unverifiable("the content cuts clips with too many pieces to check");
        }
        gs.holes.extend(cut);
    }

    fn unverifiable(&mut self, why: &'static str) {
        for i in 0..self.regions.len() {
            self.report(i, Finding::Unverifiable(why));
        }
    }

    fn form(&mut self, s: &Stream, gs: &Gs, parent: &Dict, depth: usize) {
        let (Ok(m), Ok(bbox)) = (matrix_of(self.doc, &s.dict), bbox_of(self.doc, &s.dict)) else {
            self.unverifiable("a form under the region has a malformed /Matrix or /BBox");
            return;
        };
        let fctm = m.then(&gs.ctm);
        if let Some(bb) = bbox
            && self.touching(fctm.bbox(bb)).is_empty()
        {
            return;
        }
        let Ok(data) = self.ctx.decode(self.doc, s) else {
            self.unverifiable("a form under the region can't be decoded in full");
            return;
        };
        let own = s.dict.get(b"Resources").and_then(|r| self.doc.resolve(r).as_dict().cloned());
        self.run(&data, own.as_ref().unwrap_or(parent), fctm, gs.holes.clone(), depth + 1);
    }

    /// Images drawn from tiling patterns, soft-mask groups and Type3 glyph procedures can't be
    /// placed or cleared by this check: if `res` reaches any, every region is unverifiable. Each is
    /// searched as it is found, its dictionaries read where they are held.
    fn hidden_images(&mut self, res: &Dict, depth: usize) {
        let doc = self.doc;
        let none = Dict::new();
        let xobjects = Held::of(doc, res.get(b"XObject"));
        let from_key = Cell::new(None);
        let from = (xobjects.dict().unwrap_or(&none), &from_key);
        let patterns = Held::of(doc, res.get(b"Pattern"));
        for (_, p) in patterns.dict().unwrap_or(&none).iter() {
            self.search(p, Names { font: None, from }, depth);
        }
        let states = Held::of(doc, res.get(b"ExtGState"));
        for (_, g) in states.dict().unwrap_or(&none).iter() {
            let state = Held::of(doc, Some(g));
            let mask = Held::of(doc, state.dict().and_then(|g| g.get(b"SMask")));
            if let Some(group) = mask.dict().and_then(|m| m.get(b"G")) {
                self.search(group, Names { font: None, from }, depth);
            }
        }
        let fonts = Held::of(doc, res.get(b"Font"));
        for (_, f) in fonts.dict().unwrap_or(&none).iter() {
            let font = Held::of(doc, Some(f));
            let Some(font) = font.dict() else { continue };
            if crate::subtype(doc, font).as_deref() != Some(b"Type3") {
                continue;
            }
            // A glyph looks its names up in its font's /Resources (ISO 32000-2 §9.6.4). The renderer
            // falls back from these to `res` for a name the font's lack or give no stream for (null, a
            // missing object, a dictionary), and for every name when the font has none.
            let own = Held::of(doc, font.get(b"Resources"));
            let own = own.dict().map(|r| Held::of(doc, r.get(b"XObject")));
            let font_key = Cell::new(None);
            let names = Names { font: own.as_ref().map(|x| (x.dict().unwrap_or(&none), &font_key)), from };
            let procs = Held::of(doc, font.get(b"CharProcs"));
            for (_, p) in procs.dict().unwrap_or(&none).iter() {
                self.search(p, names, depth);
            }
        }
    }

    /// Search the stream `o` gives, if it gives one, for images, and say what is found.
    fn search(&mut self, o: &Object, names: Names<'_>, depth: usize) {
        let held = self.doc.resolve(o);
        let Object::Stream(s) = &*held else { return };
        match self.has_image(o.as_ref(), s, names, depth + 1) {
            Ok(false) => {}
            Ok(true) => self.unverifiable("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"),
            Err(()) => self.unverifiable("a pattern, soft mask or Type3 glyph can't be read in full"),
        }
    }

    /// Does the content of `s` (forms it draws included) draw an image? `id` is `s`'s reference. A
    /// stream without resources of its own looks its names up in `names`, so it is searched once
    /// for each set of XObjects it is drawn with; one with resources of its own, once. Each search
    /// is charged [`SEARCH_COST`] of the proof's work allowance.
    fn has_image(&mut self, id: Option<ObjRef>, s: &Stream, names: Names<'_>, depth: usize) -> Result<bool, ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        let doc = self.doc;
        let own = Held::of(doc, s.dict.get(b"Resources"));
        let own = own.dict();
        let within = match own {
            Some(_) => Some(Within::Own),
            None => names.key(self)?.map(Within::Caller),
        };
        if let (Some(r), Some(w)) = (id, within) {
            if self.searched.contains(&(r, w)) {
                return Ok(false);
            }
            if self.searched.len() >= MAX_SEARCHED {
                return Err(());
            }
            self.searched.insert((r, w));
        }
        if !self.ctx.charge(SEARCH_COST) {
            return Err(());
        }
        let data = self.ctx.decode(doc, s)?;
        let none = Dict::new();
        let xobjects = own.map(|r| Held::of(doc, r.get(b"XObject")));
        let own_key = Cell::new(None);
        // Where the names drawn here are looked up, a form drawn without resources of its own
        // included: this stream's own XObjects, or `names`.
        let here = match &xobjects {
            Some(x) => Names { font: None, from: (x.dict().unwrap_or(&none), &own_key) },
            None => names,
        };
        for op in parse(&data).ops {
            match op.op.as_slice() {
                b"BI" => return Ok(true),
                b"Do" => {
                    let Some(entry) = op.last_name().and_then(|n| here.get(doc, n)) else { continue };
                    let held = doc.resolve(entry);
                    let Object::Stream(x) = &*held else { continue };
                    match crate::subtype(doc, &x.dict).as_deref() {
                        Some(b"Image") => return Ok(true),
                        Some(b"Form") if self.has_image(entry.as_ref(), x, here, depth + 1)? => return Ok(true),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }

    /// A key of `xobjects`, telling apart the places a stream without resources of its own is
    /// drawn from: two places that give the same names to the same objects draw alike. `None`
    /// when one of them is a stream written in place, which has no object number to tell it by.
    /// Each entry is charged to the proof's work allowance before it is digested, so a digest stops
    /// where the allowance does, with an error.
    fn xobjects_key(&self, xobjects: &Dict) -> Result<Option<[u8; 32]>, ()> {
        let mut h = Sha256::new();
        let mut direct = false;
        for (name, x) in xobjects.iter() {
            if !self.ctx.charge(name.len().saturating_add(16)) {
                return Err(());
            }
            h.update((name.len() as u64).to_be_bytes());
            h.update(name);
            match x {
                Object::Ref(r) => {
                    h.update([1]);
                    h.update(r.num.to_be_bytes());
                    h.update(r.generation.to_be_bytes());
                }
                Object::Stream(_) => direct = true,
                _ => h.update([0]),
            }
        }
        Ok((!direct).then(|| h.finalize().into()))
    }
}

/// Extend the current path (`sub`: its subpaths in device space; `clip`: the clip operator it
/// carries) by one construction operator.
fn extend_path(op: &pdfcraft_content::Op, ctm: &Matrix, sub: &mut Vec<Vec<(f64, f64)>>, clip: &mut Option<bool>) {
    let o = op.op.as_slice();
    let at = |x: f64, y: f64| ctm.apply(x, y);
    // Each takes its numbers from the end, as the renderer reads them: a surplus before them is
    // ignored, not read as a point.
    let points: Option<Vec<(f64, f64)>> = match o {
        b"l" => op.nums::<2>().map(|[x, y]| vec![at(x, y)]),
        b"c" => op.nums::<6>().map(|[x1, y1, x2, y2, x3, y3]| vec![at(x1, y1), at(x2, y2), at(x3, y3)]),
        b"v" | b"y" => op.nums::<4>().map(|[x1, y1, x2, y2]| vec![at(x1, y1), at(x2, y2)]),
        _ => None,
    };
    match o {
        b"m" => {
            if let Some([x, y]) = op.nums::<2>() {
                sub.push(vec![at(x, y)]);
            }
        }
        b"l" | b"c" | b"v" | b"y" => {
            if let (Some(cur), Some(points)) = (sub.last_mut(), points) {
                cur.extend(points);
            }
        }
        b"re" => {
            if let Some([x, y, w, h]) = op.nums::<4>() {
                sub.push(vec![at(x, y), at(x + w, y), at(x + w, y + h), at(x, y + h)]);
            }
        }
        b"W" => *clip = Some(false),
        b"W*" => *clip = Some(true),
        _ => {}
    }
}

/// The axis-aligned rectangle a four-corner subpath draws, in device space: `None` for anything
/// else (open paths, more or fewer corners, skewed or rotated boxes, non-finite coordinates).
fn axis_aligned_rect(pts: &[(f64, f64)]) -> Option<[f64; 4]> {
    // `re` leaves its rectangle open: the four corners are the whole subpath. A rectangle drawn
    // by hand closes by repeating the first point at the end. Both spell the same four corners.
    let pts = match pts {
        [a, _, _, _, e] if a == e => &pts[..4],
        _ => pts,
    };
    let [p0, p1, p2, p3] = pts else { return None };
    let finite = |p: &(f64, f64)| p.0.is_finite() && p.1.is_finite();
    if !finite(p0) || !finite(p1) || !finite(p2) || !finite(p3) {
        return None;
    }
    // The edges alternate between horizontal and vertical, whichever corner they start from and
    // whichever way the rectangle is wound.
    let flat = |a: &(f64, f64), b: &(f64, f64)| a.1 == b.1 && a.0 != b.0;
    let steep = |a: &(f64, f64), b: &(f64, f64)| a.0 == b.0 && a.1 != b.1;
    let even = flat(p0, p1) && steep(p1, p2) && flat(p2, p3) && steep(p3, p0);
    let odd = steep(p0, p1) && flat(p1, p2) && steep(p2, p3) && flat(p3, p0);
    if !even && !odd {
        return None;
    }
    let (x0, x1) = (p0.0.min(p2.0), p0.0.max(p2.0));
    let (y0, y1) = (p0.1.min(p2.1), p0.1.max(p2.1));
    (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
}

/// Are the samples of image `s` whose centres fall in `region` all clear (zero; for image masks,
/// the sample that does not paint)? `Err` when the image can't be inspected.
#[allow(clippy::result_unit_err)]
pub(super) fn image_cleared(doc: &Document, ctx: &Ctx, s: &Stream, ctm: &Matrix, region: [f64; 4]) -> Result<bool, ()> {
    if image_codec(&s.dict) {
        // Still encoded pixels: nothing was cleared.
        return Ok(false);
    }
    let (w, h) = (s.dict.int(b"Width").ok_or(())?, s.dict.int(b"Height").ok_or(())?);
    if w <= 0 || h <= 0 || (w as u64).saturating_mul(h as u64) > MAX_PIXELS {
        return Err(());
    }
    let (w, h) = (usize::try_from(w).map_err(|_| ())?, usize::try_from(h).map_err(|_| ())?);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let (ncomp, bpc) = if mask {
        (1, 1)
    } else {
        let cs = doc.resolve(s.dict.get(b"ColorSpace").ok_or(())?);
        let name = match &*cs {
            Object::Name(n) => n.clone(),
            Object::Array(a) => a.first().and_then(Object::as_name).ok_or(())?.to_vec(),
            _ => return Err(()),
        };
        let n = match name.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
            b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
            b"DeviceCMYK" | b"CMYK" => 4,
            // One component per colorant name.
            b"DeviceN" => doc.resolve(cs.as_array().and_then(|a| a.get(1)).ok_or(())?).as_array().ok_or(())?.len(),
            b"ICCBased" => match &*doc.resolve(cs.as_array().and_then(|a| a.get(1)).ok_or(())?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N").ok_or(())?).map_err(|_| ())?,
                _ => return Err(()),
            },
            _ => return Err(()),
        };
        (n, usize::try_from(s.dict.int(b"BitsPerComponent").ok_or(())?).map_err(|_| ())?)
    };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) || ncomp == 0 || ncomp > 32 {
        return Err(());
    }
    let data = ctx.decode(doc, s)?;
    let bits = ncomp.checked_mul(bpc).ok_or(())?;
    let row = w.checked_mul(bits).ok_or(())?.div_ceil(8);
    if data.len() < row.checked_mul(h).ok_or(())? {
        return Err(());
    }
    let inverted = s.dict.get(b"Decode").is_some_and(|d| {
        let d = doc.resolve(d);
        d.as_array().and_then(|a| a.first()).is_some_and(|first| doc.resolve(first).as_f64() == Some(1.0))
    });
    let want = u8::from(mask && !inverted);
    for j in 0..h {
        let v = 1.0 - (j as f64 + 0.5) / h as f64;
        // Rows that don't reach the region hold nothing to check.
        let ((ax, ay), (bx, by)) = (ctm.apply(0.0, v), ctm.apply(1.0, v));
        if !overlaps(region, [ax.min(bx), ay.min(by) - 0.01, ax.max(bx), ay.max(by) + 0.01], 0.0) {
            continue;
        }
        for i in 0..w {
            let (x, y) = ctm.apply((i as f64 + 0.5) / w as f64, v);
            if x < region[0] || x > region[2] || y < region[1] || y > region[3] {
                continue;
            }
            let start = j.checked_mul(row).and_then(|v| v.checked_mul(8)).and_then(|v| v.checked_add(i.checked_mul(bits)?)).ok_or(())?;
            for bit in start..start.checked_add(bits).ok_or(())? {
                let byte = data.get(bit / 8).ok_or(())?;
                if (byte >> (7 - bit % 8)) & 1 != want {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashSet;

    use pdfcraft_cos::{Dict, ObjRef, Object, Stream};

    use super::{Geo, Names, Within};
    use crate::tests_verify::{open, pdf_bytes, stream};
    use crate::verify::limits::{MAX_SEARCHED, MAX_TOTAL_DECODED, MAX_WORK, SEARCH_COST};
    use crate::verify::util::Ctx;

    /// The search for images reads at most [`MAX_SEARCHED`] streams in a page's check, each once
    /// for each place it is drawn from: one more is an error, which leaves the page unverifiable,
    /// and a stream already searched where it is drawn is still passed over.
    #[test]
    fn the_search_for_images_stops_at_its_allowance() {
        let objects = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec(), stream("", b"0 0 d0")];
        let doc = open(pdf_bytes(objects, ""));
        let glyph = ObjRef::new(3, 0);
        let held = doc.get(glyph);
        let Object::Stream(s) = &*held else { panic!("the glyph program is a stream") };
        let ctx = Ctx::with_decode_left(MAX_TOTAL_DECODED);
        let (none, key) = (Dict::new(), Cell::new(Some(Ok(Some([7; 32])))));
        let here = Names { font: None, from: (&none, &key) };
        for (searched, expected) in [(MAX_SEARCHED - 1, Ok(false)), (MAX_SEARCHED, Err(()))] {
            let others = (0..searched).map(|i| (ObjRef::new(100 + i as u32, 0), Within::Own)).collect();
            let mut geo = Geo { doc: &doc, ctx: &ctx, regions: &[], found: Vec::new(), reported: HashSet::new(), searched: others };
            assert_eq!(geo.has_image(Some(glyph), s, here, 0), expected, "{searched} searched before");
            geo.searched.insert((glyph, Within::Caller([7; 32])));
            assert_eq!(geo.has_image(Some(glyph), s, here, 0), Ok(false), "{searched} searched before, then this one");
        }
    }

    /// A stream with resources of its own is searched once, wherever it is drawn from: from
    /// another place it doesn't count again towards [`MAX_SEARCHED`].
    #[test]
    fn a_stream_with_resources_of_its_own_is_searched_once() {
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec(),
            stream("/Resources << >>", b"0 0 5 5 re f"),
        ];
        let doc = open(pdf_bytes(objects, ""));
        let tile = ObjRef::new(3, 0);
        let held = doc.get(tile);
        let Object::Stream(s) = &*held else { panic!("the pattern is a stream") };
        let ctx = Ctx::with_decode_left(MAX_TOTAL_DECODED);
        let others = (0..MAX_SEARCHED - 1).map(|i| (ObjRef::new(100 + i as u32, 0), Within::Own)).collect();
        let mut geo = Geo { doc: &doc, ctx: &ctx, regions: &[], found: Vec::new(), reported: HashSet::new(), searched: others };
        let none = Dict::new();
        let (one, two) = (Cell::new(Some(Ok(Some([1; 32])))), Cell::new(Some(Ok(Some([2; 32])))));
        assert_eq!(geo.has_image(Some(tile), s, Names { font: None, from: (&none, &one) }, 0), Ok(false));
        assert_eq!(geo.has_image(Some(tile), s, Names { font: None, from: (&none, &two) }, 0), Ok(false), "drawn from another place");
    }

    /// Each search for images is charged [`SEARCH_COST`] of the proof's work allowance, past which
    /// it is an error, whatever little it decodes.
    #[test]
    fn each_search_for_images_is_charged() {
        let objects = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec(), stream("", b"")];
        let doc = open(pdf_bytes(objects, ""));
        let glyph = ObjRef::new(3, 0);
        let held = doc.get(glyph);
        let Object::Stream(s) = &*held else { panic!("the glyph program is a stream") };
        let ctx = Ctx::with_decode_left(MAX_TOTAL_DECODED);
        let mut geo = Geo { doc: &doc, ctx: &ctx, regions: &[], found: Vec::new(), reported: HashSet::new(), searched: HashSet::new() };
        assert!(ctx.charge(MAX_WORK as usize - SEARCH_COST));
        let none = Dict::new();
        let (one, two) = (Cell::new(Some(Ok(Some([1; 32])))), Cell::new(Some(Ok(Some([2; 32])))));
        assert_eq!(geo.has_image(Some(glyph), s, Names { font: None, from: (&none, &one) }, 0), Ok(false), "within what is left");
        assert_eq!(geo.has_image(Some(glyph), s, Names { font: None, from: (&none, &two) }, 0), Err(()), "past it");
    }

    /// The key of a Type 3 font's XObjects over those of the place its glyphs are drawn from costs
    /// the font's own entries: those of the place are digested once for all its fonts. Fonts with
    /// the same own XObjects have the same key, and fonts with other ones, or none, another.
    #[test]
    fn the_key_of_a_fonts_xobjects_costs_its_own() {
        let doc = open(pdf_bytes(vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec()], ""));
        let ctx = Ctx::with_decode_left(MAX_TOTAL_DECODED);
        let geo = Geo { doc: &doc, ctx: &ctx, regions: &[], found: Vec::new(), reported: HashSet::new(), searched: HashSet::new() };
        let mut page = Dict::new();
        for i in 0..1000u32 {
            page.set(format!("X{i:04}").into_bytes(), Object::Ref(ObjRef::new(100 + i, 0)));
        }
        let mut own = Dict::new();
        own.set(b"Im0".to_vec(), Object::Ref(ObjRef::new(9, 0)));
        let mut other = Dict::new();
        other.set(b"Im0".to_vec(), Object::Ref(ObjRef::new(11, 0)));
        // 1,000 entries of 5 bytes for the page, then 8 fonts of one 3-byte entry.
        assert!(ctx.charge(MAX_WORK as usize - (1000 * 21 + 8 * 19)));
        let from = Cell::new(None);
        let slots: Vec<_> = (0..9).map(|_| Cell::new(None)).collect();
        let keys: Vec<_> = slots[..8]
            .iter()
            .enumerate()
            .map(|(i, slot)| Names { font: Some((if i == 7 { &other } else { &own }, slot)), from: (&page, &from) }.key(&geo))
            .collect();
        assert!(keys.iter().all(|k| matches!(k, Ok(Some(_)))), "{keys:?}");
        assert_eq!(keys[0], keys[6], "the same own XObjects");
        assert_ne!(keys[0], keys[7], "other own XObjects");
        assert_ne!(keys[0], Names { font: None, from: (&page, &from) }.key(&geo), "none of its own");
        assert_eq!(Names { font: Some((&own, &slots[8])), from: (&page, &from) }.key(&geo), Err(()), "past the allowance");
        // The same font drawn from another place.
        let ctx = Ctx::with_decode_left(MAX_TOTAL_DECODED);
        let geo = Geo { doc: &doc, ctx: &ctx, regions: &[], found: Vec::new(), reported: HashSet::new(), searched: HashSet::new() };
        let elsewhere = Dict::new();
        let key = |from: &Dict| Names { font: Some((&own, &Cell::new(None))), from: (from, &Cell::new(None)) }.key(&geo);
        assert_eq!(key(&page), keys[0]);
        assert_ne!(key(&page), key(&elsewhere), "drawn from another place");
    }

    /// The key of a set of XObjects tells apart two that give a name to another object, or an
    /// object another name; a stream written in place gives none; and past the proof's work
    /// allowance it is an error.
    #[test]
    fn the_key_of_a_set_of_xobjects() {
        let doc = open(pdf_bytes(vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec()], ""));
        let ctx = Ctx::with_decode_left(MAX_TOTAL_DECODED);
        let geo = Geo { doc: &doc, ctx: &ctx, regions: &[], found: Vec::new(), reported: HashSet::new(), searched: HashSet::new() };
        let xobjects = |entries: &[(&[u8], Object)]| {
            let mut d = Dict::new();
            for (name, x) in entries {
                d.set(name.to_vec(), x.clone());
            }
            d
        };
        let at = |n| Object::Ref(ObjRef::new(n, 0));
        let key = |d: &Dict| geo.xobjects_key(d).ok().flatten();
        let one = key(&xobjects(&[(b"Im0", at(9))]));
        assert!(one.is_some());
        assert_eq!(one, key(&xobjects(&[(b"Im0", at(9))])), "the same objects under the same names");
        assert_ne!(one, key(&xobjects(&[(b"Im0", at(11))])), "another object");
        assert_ne!(one, key(&xobjects(&[(b"Im1", at(9))])), "another name");
        assert_ne!(one, key(&xobjects(&[(b"Im0", at(9)), (b"X", Object::Null)])), "a name more");
        let written_in_place = Object::Stream(Stream { dict: Dict::new(), raw: Default::default() });
        assert_eq!(geo.xobjects_key(&xobjects(&[(b"Im0", written_in_place)])), Ok(None));
        // Each entry costs its name's length and 16 of the proof's work allowance.
        let ctx = Ctx::with_decode_left(MAX_TOTAL_DECODED);
        let geo = Geo { doc: &doc, ctx: &ctx, regions: &[], found: Vec::new(), reported: HashSet::new(), searched: HashSet::new() };
        assert!(ctx.charge(MAX_WORK as usize - 19));
        assert!(geo.xobjects_key(&xobjects(&[(b"Im0", at(9))])).is_ok(), "within what is left");
        assert_eq!(geo.xobjects_key(&xobjects(&[(b"Im0", at(9))])), Err(()), "past it");
        // With the allowance spent, a digest stops at its first entry, not after the last.
        let many: Vec<(Vec<u8>, Object)> = (0..1000).map(|i| (format!("X{i:04}").into_bytes(), at(100 + i))).collect();
        let many = xobjects(&many.iter().map(|(n, x)| (n.as_slice(), x.clone())).collect::<Vec<_>>());
        let before = ctx.work.get();
        assert_eq!(geo.xobjects_key(&many), Err(()));
        assert_eq!(ctx.work.get() - before, 21, "one entry of five bytes charged");
    }
}
