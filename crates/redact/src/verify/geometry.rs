//! The non-text content under the regions: images, paths and shadings, and the placement of the
//! glyphs still shown, checked independently of the interpreter.

use std::collections::HashSet;

use pdfcraft_content::{Matrix, Op, contains, overlaps, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::extract::{appearance_matrix, bbox_of, matrix_of, page_content, res_dict};
use super::util::{Ctx, image_codec};
use crate::stroke::{INITIAL_MITER_LIMIT, flat, join_miters, miter_reach};
use crate::verify::limits::{MAX_CLIP_SUBPATHS, MAX_DEPTH, MAX_GS_STACK, MAX_HOLES, MAX_PIXELS};

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
    pub(super) pen: Pen,
}

/// What a stroke paints around its path: the line width in user space (`w`, an ExtGState's `/LW`),
/// and whether its joins are mitered (`j`, `/LJ`) and up to which limit (`M`, `/ML`).
#[derive(Clone, Copy)]
pub(super) struct Pen {
    pub(super) width: f64,
    pub(super) miters: bool,
    pub(super) miter_limit: f64,
}

/// The pen a page starts with (ISO 32000-2 Table 51): a width of 1, mitered joins, a limit of 10.
const INITIAL_PEN: Pen = Pen { width: 1.0, miters: true, miter_limit: INITIAL_MITER_LIMIT };

/// The least a stroke reaches from its path in device space: the thinnest line still paints a
/// device pixel, half a point at 72 dots an inch.
const MIN_STROKE_REACH: f64 = 0.5;

pub(super) struct Geo<'a> {
    pub(super) doc: &'a Document,
    pub(super) ctx: &'a Ctx,
    pub(super) regions: &'a [[f64; 4]],
    pub(super) found: Vec<(usize, Finding)>,
    /// What `found` already holds: each finding is recorded once per region, so a page that
    /// paints thousands of times cannot grow the report (or the memory) with repeats.
    reported: HashSet<(usize, Finding)>,
    /// Pattern, soft-mask and Type3 resources already searched for images.
    pub(super) searched: HashSet<ObjRef>,
}

/// What still paints inside `regions` on a page of the output (the overlay boxes excepted):
/// images with pixels there, paths and shadings not clipped away from there.
pub(super) fn check_geometry(doc: &Document, page: &Dict, regions: &[[f64; 4]], ctx: &Ctx) -> Vec<(usize, Finding)> {
    let unverifiable = |why: &'static str| (0..regions.len()).map(|i| (i, Finding::Unverifiable(why))).collect();
    // The redaction boxes are painted on purpose.
    let Ok(data) = page_content(doc, page, ctx, true) else { return unverifiable("the page content can't be read in full") };
    let mut geo = Geo { doc, ctx, regions, found: Vec::new(), reported: HashSet::new(), searched: HashSet::new() };
    let res = page.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    geo.run(&data, &res, Matrix::IDENTITY, Vec::new(), INITIAL_PEN, 0);
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
            if d.name(b"Subtype") == Some(b"Redact") {
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
                    self.run(&data, own.as_ref().unwrap_or(res), ctm, Vec::new(), INITIAL_PEN, 1);
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

    fn run(&mut self, data: &[u8], res: &Dict, ctm: Matrix, holes: Vec<[f64; 4]>, pen: Pen, depth: usize) {
        if depth > MAX_DEPTH {
            for i in 0..self.regions.len() {
                self.report(i, Finding::Unverifiable("forms are nested too deep to check"));
            }
            return;
        }
        let xobjects = res_dict(self.doc, res, b"XObject");
        let states = res_dict(self.doc, res, b"ExtGState");
        self.hidden_images(res, depth);
        let mut gs = Gs { ctm, holes, pen };
        let mut stack: Vec<Gs> = Vec::new();
        // The current path in device space: its subpaths, and the clip operator it carries; the
        // box of what it paints, and the last move's point, which that takes once something draws
        // from it (`draw`); and the operators that make it, whose joins a stroke paints at.
        let mut sub: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut clip: Option<bool> = None;
        let mut drawn: Option<[f64; 4]> = None;
        let mut moved: Option<(f64, f64)> = None;
        let mut path: Vec<Op> = Vec::new();
        for op in parse(data).ops {
            let o = op.op.as_slice();
            if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
                draw(&op, &gs.ctm, !sub.is_empty(), &mut drawn, &mut moved);
                extend_path(&op, &gs.ctm, &mut sub, &mut clip);
                path.push(op);
                continue;
            }
            if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                // `s`, `b` and `b*` close the last subpath: a move left waiting is then a point
                // closed, which a round cap paints.
                if matches!(o, b"s" | b"b" | b"b*") {
                    drawn = union(drawn, moved.map(|(u, v)| [u, v, u, v]));
                }
                self.paint_path(o, &mut gs, &sub, &path, clip, drawn);
                sub.clear();
                clip = None;
                (drawn, moved) = (None, None);
                path.clear();
                continue;
            }
            sub.clear();
            clip = None;
            (drawn, moved) = (None, None);
            path.clear();
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
                // Read as the renderer reads them, the last operand; a negative width at its size.
                b"w" => {
                    if let Some([width]) = op.nums::<1>() {
                        gs.pen.width = width.abs();
                    }
                }
                b"j" => {
                    if let Some([join]) = op.nums::<1>() {
                        gs.pen.miters = join_miters(join);
                    }
                }
                b"M" => {
                    if let Some([limit]) = op.nums::<1>() {
                        gs.pen.miter_limit = limit;
                    }
                }
                b"gs" => {
                    let state = op.last_name().and_then(|n| states.get(n)).map(|s| self.doc.resolve(s));
                    let state = state.as_deref().and_then(Object::as_dict);
                    let number = |key: &[u8]| state.and_then(|d| d.get(key)).and_then(|v| self.doc.resolve(v).as_f64());
                    if let Some(width) = number(b"LW") {
                        gs.pen.width = width.abs();
                    }
                    if let Some(join) = number(b"LJ") {
                        gs.pen.miters = join_miters(join);
                    }
                    if let Some(limit) = number(b"ML") {
                        gs.pen.miter_limit = limit;
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
                    let Some(entry) = op.name(0).and_then(|n| xobjects.get(n)) else { continue };
                    let obj = self.doc.resolve(entry);
                    let Object::Stream(s) = &*obj else { continue };
                    match s.dict.name(b"Subtype") {
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

    /// A path-painting operator `o` ends the current path, whose painted points lie in `bbox`:
    /// report what it paints over a region, and let an even-odd clip add its holes to `gs`.
    fn paint_path(&mut self, o: &[u8], gs: &mut Gs, sub: &[Vec<(f64, f64)>], ops: &[Op], clip: Option<bool>, bbox: Option<[f64; 4]>) {
        // A stroke paints around its path: half the width on each side, `√2` times that at a
        // projecting cap's corner or further at its sharpest mitered corner (`miter_reach`),
        // through the CTM, and at least `MIN_STROKE_REACH`. A fill of a
        // path with no width or height (or none to the renderer, `flat`) still paints the pixels it
        // touches (ISO 32000-2 §10.7.4):
        // `MIN_STROKE_REACH` across there.
        let strokes = matches!(o, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
        let paints = bbox.map(|b| {
            if !strokes {
                let across = if flat(b[0], b[2]) { MIN_STROKE_REACH } else { 0.0 };
                let up = if flat(b[1], b[3]) { MIN_STROKE_REACH } else { 0.0 };
                return [b[0] - across, b[1] - up, b[2] + across, b[3] + up];
            }
            let [a, bb, c, d, _, _] = gs.ctm.0;
            let miter = if gs.pen.miters { miter_reach(ops, matches!(o, b"s" | b"b" | b"b*"), gs.pen.miter_limit) } else { 1.0 };
            let half = gs.pen.width / 2.0 * miter.max(std::f64::consts::SQRT_2);
            let (across, up) = ((half * a.hypot(c)).max(MIN_STROKE_REACH), (half * bb.hypot(d)).max(MIN_STROKE_REACH));
            [b[0] - across, b[1] - up, b[2] + across, b[3] + up]
        });
        if o != b"n"
            && let Some(b) = paints
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
        self.run(&data, own.as_ref().unwrap_or(parent), fctm, gs.holes.clone(), gs.pen, depth + 1);
    }

    /// Images drawn from tiling patterns, soft-mask groups and Type3 glyph procedures can't be
    /// placed or cleared by this check: if `res` reaches any, every region is unverifiable.
    fn hidden_images(&mut self, res: &Dict, depth: usize) {
        let doc = self.doc;
        let mut streams: Vec<(Option<ObjRef>, Stream)> = Vec::new();
        let take = |o: &Object, streams: &mut Vec<(Option<ObjRef>, Stream)>| {
            if let Object::Stream(s) = &*doc.resolve(o) {
                streams.push((o.as_ref(), s.clone()));
            }
        };
        for (_, p) in res_dict(doc, res, b"Pattern").iter() {
            take(p, &mut streams);
        }
        for (_, g) in res_dict(doc, res, b"ExtGState").iter() {
            if let Some(Object::Dict(sm)) = doc.resolve(g).as_dict().and_then(|g| g.get(b"SMask")).map(|m| (*doc.resolve(m)).clone()).as_ref()
                && let Some(group) = sm.get(b"G")
            {
                take(group, &mut streams);
            }
        }
        for (_, f) in res_dict(doc, res, b"Font").iter() {
            let Some(font) = doc.resolve(f).as_dict().cloned() else { continue };
            if font.name(b"Subtype") != Some(b"Type3") {
                continue;
            }
            if let Some(procs) = font.get(b"CharProcs").and_then(|c| doc.resolve(c).as_dict().cloned()) {
                for (_, p) in procs.iter() {
                    take(p, &mut streams);
                }
            }
        }
        for (id, s) in streams {
            if let Some(r) = id
                && !self.searched.insert(r)
            {
                continue;
            }
            match self.has_image(&s, res, depth + 1) {
                Ok(false) => {}
                Ok(true) => self.unverifiable("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"),
                Err(()) => self.unverifiable("a pattern, soft mask or Type3 glyph can't be read in full"),
            }
        }
    }

    /// Does the content of `s` (forms it draws included) draw an image?
    fn has_image(&mut self, s: &Stream, parent: &Dict, depth: usize) -> Result<bool, ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        let data = self.ctx.decode(self.doc, s)?;
        let own = s.dict.get(b"Resources").and_then(|r| self.doc.resolve(r).as_dict().cloned());
        let res = own.as_ref().unwrap_or(parent);
        let xobjects = res_dict(self.doc, res, b"XObject");
        for op in parse(&data).ops {
            match op.op.as_slice() {
                b"BI" => return Ok(true),
                b"Do" => {
                    let Some(entry) = op.name(0).and_then(|n| xobjects.get(n)) else { continue };
                    let Object::Stream(x) = &*self.doc.resolve(entry) else { continue };
                    match x.dict.name(b"Subtype") {
                        Some(b"Image") => return Ok(true),
                        Some(b"Form") => {
                            if let Some(r) = entry.as_ref()
                                && !self.searched.insert(r)
                            {
                                continue;
                            }
                            if self.has_image(x, res, depth + 1)? {
                                return Ok(true);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }
}

/// Extend the current path (`sub`: its subpaths in device space; `clip`: the clip operator it
/// carries) by one construction operator.
fn extend_path(op: &pdfcraft_content::Op, ctm: &Matrix, sub: &mut Vec<Vec<(f64, f64)>>, clip: &mut Option<bool>) {
    let o = op.op.as_slice();
    let nums: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
    let at = |x: f64, y: f64| ctm.apply(x, y);
    match o {
        b"m" => {
            if let [x, y] = nums[..] {
                sub.push(vec![at(x, y)]);
            }
        }
        b"l" | b"c" | b"v" | b"y" => {
            if let Some(cur) = sub.last_mut() {
                cur.extend(nums.as_chunks::<2>().0.iter().map(|p| at(p[0], p[1])));
            }
        }
        b"re" => {
            if let [x, y, w, h] = nums[..] {
                sub.push(vec![at(x, y), at(x + w, y), at(x + w, y + h), at(x, y + h)]);
            }
        }
        b"W" => *clip = Some(false),
        b"W*" => *clip = Some(true),
        _ => {}
    }
}

/// Grow `drawn`, the device-space box of what the current path paints, by one construction
/// operator (`current`: the path has a subpath for a segment to extend, as [`extend_path`] has it).
/// A move's point (`moved`) counts once a segment or `h` follows it, or the painting operator
/// closes it: a subpath that is only a move paints nothing, and nor does a move the next one
/// overrides (ISO 32000-2 §8.5.3.2, and as the renderer draws). One whose operands don't read is
/// skipped, leaving the move before it. After `re` the renderer strokes a segment from the point
/// before it, so a move waits across one.
fn draw(op: &Op, ctm: &Matrix, current: bool, drawn: &mut Option<[f64; 4]>, moved: &mut Option<(f64, f64)>) {
    let o = op.op.as_slice();
    let nums: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
    let at = |x: f64, y: f64| ctm.apply(x, y);
    let mut pts: Vec<(f64, f64)> = match (o, &nums[..]) {
        (b"m", &[x, y]) => {
            *moved = Some(at(x, y));
            return;
        }
        (b"l" | b"c" | b"v" | b"y", _) if current => nums.as_chunks::<2>().0.iter().map(|p| at(p[0], p[1])).collect(),
        (b"re", &[x, y, w, h]) => vec![at(x, y), at(x + w, y), at(x + w, y + h), at(x, y + h)],
        _ => Vec::new(),
    };
    if matches!(o, b"l" | b"c" | b"v" | b"y" | b"h") {
        pts.extend(moved.take());
    }
    for (u, v) in pts {
        *drawn = union(*drawn, Some([u, v, u, v]));
    }
}

/// The box covering `a` and `b`, either of which may be none.
fn union(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
        (a, b) => a.or(b),
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
