//! Link annotations for auto-tagging (ISO 32000-2 §14.8.4.7.2, Table 368; §14.7.5.3 and §14.7.5.4).
//!
//! Each link annotation on a page becomes a Link structure element that holds the annotation through
//! an object reference (OBJR) and the text drawn inside the link's area. A glyph belongs to a link
//! when the centre of its box falls inside one of the link's areas: its `/QuadPoints`
//! quadrilaterals when they all lie within `/Rect`, else `/Rect` (Table 176). Glyph positions and
//! link areas are both in the page's default user space, so `/Rotate` plays no part. Where areas
//! overlap, the smallest one wins, so a link nested in another keeps its own text.
//!
//! A text-showing operator whose glyphs belong to different owners is split at character-code
//! boundaries into operators of the same kind, which paint the same glyphs at the same places: each
//! part can then be its own marked-content sequence. Glyphs whose position can't be worked out (an
//! unknown font, codes that can't be split reliably) stay with their text object, never with a link.
//!
//! A glyph a clip hides shows nowhere, so it is no link's: a clip that encloses no area hides
//! everything; a clip rectangle, through the CTM, hides a glyph whose centre is outside it; any
//! other clip path hides one whose centre is outside its shape, by the nonzero rule for `W` and
//! the even-odd rule for `W*` (§8.5.3.3), with its curves flattened to [`CURVE_SEGMENTS`] lines
//! each. Past [`MAX_CLIP_QUADS`] turned rectangles in one state a rectangle, and past the
//! document's [`MAX_CLIP_WORK`] any other path, counts by the box around it, so a glyph inside the
//! box is taken as shown; past [`MAX_SAVED`] nested `q`, clips are not followed, and clipping by
//! text (render modes 4 to 7) never is. Hidden text inside a link's text is an artifact: in no
//! element, and no interruption.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};
use pdfcraft_fonts::pdf::Metrics;

use crate::autotag::{AutoTagError, unsupported};
use crate::autotag_roles::Fonts;

/// Link annotations tagged in one document.
const MAX_LINKS: usize = 4_096;
/// `/QuadPoints` numbers one document may read: eight for every area [`MAX_LINK_AREAS`] allows,
/// eight times over.
pub(crate) const MAX_QUAD_VALUES: usize = MAX_LINK_AREAS * 64;
/// Link areas (a quadrilateral, or a `/Rect`) one document may hold: each is kept, with its
/// corners, until the content is written. A link with more than the viewer's
/// [`pdfcraft_render::MAX_LINK_QUADS`] quadrilaterals is its `/Rect`, one area.
pub(crate) const MAX_LINK_AREAS: usize = 65_536;
/// A jump between two glyphs of a link's text, in glyph space (a fraction of the font size, along
/// the line as `Tz` scales it), that the description reads as a space: wider than kerning,
/// narrower than a word space.
const WORD_GAP: f64 = 0.15;
/// Saved graphics states followed; deeper `q` nesting keeps the current state.
const MAX_SAVED: usize = 64;
/// Clip rectangles not aligned with the axes that are followed exactly in one graphics state.
const MAX_CLIP_QUADS: usize = 16;
/// The line segments a Bézier curve of a clip path is followed by.
const CURVE_SEGMENTS: usize = 16;
/// Clip-path work in one document: each line segment a clip path is kept as, and each segment a
/// glyph is tested against. Every glyph inside a clip path's box is tested, near a link or not.
/// Past it, a clip path counts by the box around it.
const MAX_CLIP_WORK: usize = 50_000_000;
/// Characters of a `/Contents` description written from a link's text or address.
const MAX_CONTENTS: usize = 200;
/// Glyph-in-area tests in one document: each glyph is tested against every link area on its page.
pub(crate) const MAX_AREA_TESTS: usize = 50_000_000;

/// What placing a document's glyphs may use, across its pages.
pub(crate) struct Allowance {
    /// Glyph-in-area tests left, from [`MAX_AREA_TESTS`]: past them, the document is refused.
    tests: usize,
    /// Clip-path work left, from [`MAX_CLIP_WORK`]: past it, clip paths count by their boxes.
    clip: usize,
}

impl Default for Allowance {
    fn default() -> Self {
        Allowance { tests: MAX_AREA_TESTS, clip: MAX_CLIP_WORK }
    }
}

/// What a document's links use, across its pages.
pub(crate) struct LinkBudget {
    /// The annotations listed so far.
    seen: HashSet<ObjRef>,
    /// Link areas left, from [`MAX_LINK_AREAS`]; each is charged before it is built.
    areas: usize,
    /// `/QuadPoints` numbers left to read, from [`MAX_QUAD_VALUES`].
    values: usize,
}

impl Default for LinkBudget {
    fn default() -> Self {
        LinkBudget { seen: HashSet::new(), areas: MAX_LINK_AREAS, values: MAX_QUAD_VALUES }
    }
}

/// A link annotation to tag.
pub(crate) struct Link {
    pub annot: ObjRef,
    /// Where the annotation is in the page's `/Annots`, from 1: how a refusal names it.
    pub position: usize,
    /// Where the link is active, in default user space. Empty when the annotation has no usable
    /// `/Rect`: it is still tagged, but owns no text.
    areas: Vec<Area>,
    /// The annotation has a `/Contents` of its own with something in it. An empty or blank one
    /// describes nothing (PDF/UA-1 §7.18.5 asks for a description), so it is filled like a missing
    /// one.
    described: bool,
    /// A `/URI` action's address.
    uri: Option<String>,
}

/// The link annotations of a page, in `/Annots` order. Any other annotation is refused, as is a link
/// this can't tag on its own: one written inline (an OBJR needs an object to refer to), one listed
/// twice (`budget` spans the document) or one that already has a parent tree key.
pub(crate) fn page_links(doc: &Document, page: &Dict, index: usize, budget: &mut LinkBudget) -> Result<Vec<Link>, AutoTagError> {
    let LinkBudget { seen, areas: areas_left, values } = budget;
    let Some(annots) = page.get(b"Annots") else { return Ok(Vec::new()) };
    let annots = doc.resolve(annots);
    let Object::Array(items) = &*annots else { return Err(unsupported(index, "the page's annotations are not an array")) };
    let mut links = Vec::with_capacity(items.len().min(MAX_LINKS));
    for (at, item) in items.iter().enumerate() {
        let Object::Ref(annot) = item else {
            return Err(unsupported(index, "an annotation is written inside /Annots; a Link tag needs it as an object of its own"));
        };
        let object = doc.get(*annot);
        let Some(dict) = object.as_dict() else {
            return Err(unsupported(index, format!("annotation {} in /Annots is missing or not a dictionary", at.saturating_add(1))));
        };
        let subtype = dict.get(b"Subtype").map(|s| doc.resolve(s)).and_then(|s| s.as_name().map(<[u8]>::to_vec));
        if subtype.as_deref() != Some(b"Link".as_slice()) {
            let kind = subtype.map_or_else(|| "untyped".to_string(), |s| String::from_utf8_lossy(&s).into_owned());
            return Err(unsupported(index, format!("{kind} annotations need separate structure tags; Apply tags only links")));
        }
        if dict.contains(b"StructParent") {
            return Err(unsupported(index, "a link annotation already has a structure parent key"));
        }
        if !seen.insert(*annot) {
            return Err(unsupported(index, "a link annotation is listed twice"));
        }
        if seen.len() > MAX_LINKS {
            return Err(unsupported(index, format!("too many link annotations (maximum {MAX_LINKS})")));
        }
        let Some(areas) = areas(doc, dict, areas_left, values) else {
            return Err(unsupported(
                index,
                format!(
                    "the document's links have too many quadrilaterals (maximum {MAX_LINK_AREAS} in all, from {MAX_QUAD_VALUES} /QuadPoints numbers)"
                ),
            ));
        };
        links.push(Link { annot: *annot, position: at.saturating_add(1), areas, described: described(doc, dict), uri: uri(doc, dict) });
    }
    Ok(links)
}

/// A rectangle: an array of four finite numbers, each possibly given by reference. A longer array
/// isn't read (one shared by many links would be read again for each).
fn rect(doc: &Document, value: Option<&Object>) -> Option<[f64; 4]> {
    let value = doc.resolve(value?);
    let [a, b, c, d] = value.as_array()?.as_slice() else { return None };
    let n = |v: &Object| doc.resolve(v).as_f64().filter(|n| n.is_finite());
    Some([n(a)?, n(b)?, n(c)?, n(d)?])
}

/// A convex region where a link is active: its corners counter-clockwise, the box around them,
/// and its area.
#[derive(Clone, Debug)]
struct Area {
    corners: Vec<(f64, f64)>,
    bounds: [f64; 4],
    size: f64,
}

impl Area {
    /// The convex hull of `points`; `None` when it encloses no area. A quadrilateral's corners
    /// come in whatever order its producer wrote them (Table 176 asks for counter-clockwise; many
    /// write top left, top right, bottom left, bottom right), so the hull is what is read: the
    /// union of the four triangles its corners make, which the viewer tests
    /// (`pdfcraft_render::quad_contains`). Whether a quadrilateral is too thin to use is the
    /// viewer's rule, applied before ([`areas`]).
    fn hull(points: &[(f64, f64)]) -> Option<Area> {
        let mut points = points.to_vec();
        points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let turn = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
        let mut corners: Vec<(f64, f64)> = Vec::with_capacity(points.len() * 2);
        // Andrew's monotone chain: the lower hull left to right, then the upper hull back.
        for pass in [points.clone(), points.into_iter().rev().collect()] {
            let start = corners.len();
            for p in pass {
                while corners.len() >= start + 2 && turn(corners[corners.len() - 2], corners[corners.len() - 1], p) <= 0.0 {
                    corners.pop();
                }
                corners.push(p);
            }
            corners.pop();
        }
        let size = corners.iter().zip(corners.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum::<f64>() / 2.0;
        if corners.len() < 3 || !(size.is_finite() && size > 0.0) {
            return None;
        }
        let bounds = corners.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
            [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]
        });
        Some(Area { corners, bounds, size })
    }

    fn holds(&self, (x, y): (f64, f64)) -> bool {
        let b = self.bounds;
        b[0] <= x
            && x <= b[2]
            && b[1] <= y
            && y <= b[3]
            && self.corners.iter().zip(self.corners.iter().cycle().skip(1)).all(|(a, c)| (c.0 - a.0) * (y - a.1) - (c.1 - a.1) * (x - a.0) >= 0.0)
    }
}

/// The link's areas as the viewer reads them ([`pdfcraft_render::link_quads`], Table 176): its
/// `/QuadPoints` quadrilaterals when they are usable and every corner lies within `/Rect`, else
/// `/Rect`. The numbers read are charged to `values` before they are read (an array can be
/// shared by many links, and is read again for each), and each area to `left` before it is built;
/// `None` when too few are left.
fn areas(doc: &Document, annot: &Dict, left: &mut usize, values: &mut usize) -> Option<Vec<Area>> {
    let Some([a, b, c, d]) = rect(doc, annot.get(b"Rect")) else { return Some(Vec::new()) };
    let (x0, y0, x1, y1) = (a.min(c), b.min(d), a.max(c), b.max(d));
    // In the viewer's single precision; an array longer than it reads is not read.
    let array = annot.get(b"QuadPoints").map(|q| doc.resolve(q));
    let quads = match array.as_deref().and_then(Object::as_array) {
        Some(q) if q.len() <= pdfcraft_render::MAX_LINK_QUADS.saturating_mul(8) => {
            *values = values.checked_sub(q.len())?;
            let read: Vec<Option<f32>> = q.iter().map(|v| doc.resolve(v).as_f64().map(|n| n as f32)).collect();
            pdfcraft_render::link_quads(&read, [x0, y0, x1, y1].map(|v| v as f32))
        }
        _ => Vec::new(),
    };
    *left = left.checked_sub(quads.len().max(1))?;
    if quads.is_empty() {
        return Some(Area::hull(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)]).into_iter().collect());
    }
    let corners = |q: &[f32; 8]| -> Vec<(f64, f64)> { q.as_chunks::<2>().0.iter().map(|&[x, y]| (f64::from(x), f64::from(y))).collect() };
    Some(quads.iter().filter_map(|q| Area::hull(&corners(q))).collect())
}

fn described(doc: &Document, annot: &Dict) -> bool {
    annot.get(b"Contents").map(|c| doc.resolve(c)).and_then(|c| c.as_string().map(|s| !s.to_text().trim().is_empty())).unwrap_or(false)
}

fn uri(doc: &Document, annot: &Dict) -> Option<String> {
    let action = doc.resolve(annot.get(b"A")?);
    let action = action.as_dict()?;
    if action.get(b"S").map(|s| doc.resolve(s)).and_then(|s| s.as_name().map(<[u8]>::to_vec)).as_deref() != Some(b"URI".as_slice()) {
        return None;
    }
    let address = doc.resolve(action.get(b"URI")?);
    // A URI is 7-bit ASCII (§12.6.4.8); anything else is shown as it decodes.
    let address = String::from_utf8_lossy(&address.as_string()?.bytes).trim().to_string();
    (!address.is_empty()).then_some(address)
}

impl Link {
    /// The `/Contents` to write: none when the annotation has its own, else the text it covers, else
    /// its address.
    fn description(&self, text: &str) -> Option<String> {
        if self.described {
            return None;
        }
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let chosen = if text.is_empty() { self.uri.clone()? } else { text };
        Some(chosen.chars().take(MAX_CONTENTS).collect())
    }
}

/// A page's operators with text-showing operators split where their glyphs change owner.
pub(crate) struct Placed {
    pub ops: Vec<Op>,
    /// For each text-showing operator that shows a glyph: the link its glyphs belong to, or `None`
    /// for the text object's own text.
    pub owners: HashMap<usize, Option<usize>>,
    /// The text-showing operators whose glyphs all read as nothing ([`blank`]): whitespace and
    /// control characters.
    pub blank: HashSet<usize>,
    /// The text-showing operators of no link whose glyphs are all without text with codes that
    /// are control codes (below 32) of a one-byte font: a control character the writer copied
    /// from its text (standard encodings have no glyph there), such as a newline shown off the
    /// page after each label. They join a link only when they lie inside its text ([`settle`]).
    pub quiet: HashSet<usize>,
    /// The text-showing operators whose glyphs are all clipped out of view: shown nowhere, so no
    /// link's. They never interrupt a link; inside a link's text they are artifacts ([`settle`]).
    pub hidden: HashSet<usize>,
    /// The `/Contents` to write on each link, by index into the page's links.
    pub descriptions: Vec<Option<String>>,
}

/// Where each glyph of `ops` falls, against `links`. `resources` and `states` are the page's `/Font`
/// and `/ExtGState` resource dictionaries and `fonts` the document's font cache. With no link areas the operators come back as
/// they are and no glyph has an owner. `allowance` is what the document has left; `None` when
/// placing this page's glyphs would take more glyph-in-area tests than it has.
pub(crate) fn place(
    doc: &Document,
    fonts: &mut Fonts,
    resources: &Dict,
    states: &Dict,
    ops: Vec<Op>,
    links: &[Link],
    allowance: &mut Allowance,
) -> Option<Placed> {
    // Each usable area once, with the link it belongs to: what every glyph near them is tested
    // against, and what it is charged for.
    let areas: Vec<(usize, &Area)> = links.iter().enumerate().flat_map(|(i, l)| l.areas.iter().map(move |a| (i, a))).collect();
    if areas.is_empty() {
        let descriptions = links.iter().map(|l| l.description("")).collect();
        return Some(Placed { ops, owners: HashMap::new(), blank: HashSet::new(), quiet: HashSet::new(), hidden: HashSet::new(), descriptions });
    }
    let mut walk = Walk {
        state: State::default(),
        saved: Vec::new(),
        unsaved: 0,
        tm: Matrix::IDENTITY,
        tlm: Matrix::IDENTITY,
        lost: false,
        tests: allowance.tests,
        clip_work: allowance.clip,
        exhausted: false,
        bounds: areas.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, (_, a)| {
            [b[0].min(a.bounds[0]), b[1].min(a.bounds[1]), b[2].max(a.bounds[2]), b[3].max(a.bounds[3])]
        }),
        areas,
        texts: vec![Covered::default(); links.len()],
        path: Path::default(),
        clip: None,
    };
    let mut out = Vec::with_capacity(ops.len());
    let (mut owners, mut blank, mut quiet, mut hidden) = (HashMap::new(), HashSet::new(), HashSet::new(), HashSet::new());
    for op in ops {
        match op.op.as_slice() {
            b"Tf" => {
                // Looked up first, whatever it is, so the fonts the document's allowance measures
                // are the same as with no links; a vertical font's glyphs are then left unplaced.
                walk.state.metrics =
                    op.name(0).and_then(|name| fonts.named(doc, resources, name).filter(|_| !resources.get(name).is_some_and(|f| vertical(doc, f))));
            }
            b"gs" => {
                if let Some(set) = op.name(0).and_then(|name| fonts.state(doc, states, name)) {
                    walk.state.metrics = set.metrics.filter(|_| !vertical(doc, &set.font));
                    walk.state.size = set.size;
                }
            }
            _ => {}
        }
        let runs = walk.step(&op);
        // Checked before anything else: once spent, later operators would place nothing at all.
        if walk.exhausted {
            return None;
        }
        let Some(runs) = runs else {
            out.push(op);
            continue;
        };
        if runs.len() <= 1 {
            if let Some(run) = runs.first() {
                owners.insert(out.len(), run.owner);
                match run.reads {
                    _ if run.hidden => hidden.insert(out.len()),
                    Reads::Nothing => blank.insert(out.len()),
                    Reads::Unknown => quiet.insert(out.len()),
                    Reads::Text => false,
                };
            }
            out.push(op);
            continue;
        }
        for (i, run) in runs.into_iter().enumerate() {
            owners.insert(out.len(), run.owner);
            match run.reads {
                _ if run.hidden => hidden.insert(out.len()),
                Reads::Nothing => blank.insert(out.len()),
                Reads::Unknown => quiet.insert(out.len()),
                Reads::Text => false,
            };
            out.push(split_op(&op, i == 0, run.items));
        }
    }
    (allowance.tests, allowance.clip) = (walk.tests, walk.clip_work);
    let descriptions = links.iter().zip(&walk.texts).map(|(l, t)| l.description(&t.text)).collect();
    Some(Placed { ops: out, owners, blank, quiet, hidden, descriptions })
}

/// Whether `font` writes vertically (WMode 1, §9.7.4.3): its glyphs advance down the page, which
/// the metrics don't follow, so its glyphs are left unplaced.
fn vertical(doc: &Document, font: &Object) -> bool {
    let font = doc.resolve(font);
    let Some(encoding) = font.as_dict().and_then(|f| f.get(b"Encoding")).map(|e| doc.resolve(e)) else { return false };
    match &*encoding {
        Object::Name(name) => name.ends_with(b"-V"),
        Object::Stream(cmap) => cmap.dict.get(b"WMode").map(|w| doc.resolve(w)).and_then(|w| w.as_int()) == Some(1),
        _ => false,
    }
}

/// The part of a text-showing operator that shows `items`. The first part keeps the operator, so
/// `'` and `"` still move to the next line (and `"` still sets the spacing); later parts show from
/// where the previous one ended, with `Tj` or, for `TJ`, `TJ`.
fn split_op(op: &Op, first: bool, items: Vec<Object>) -> Op {
    if op.op == b"TJ" {
        return Op::new("TJ", vec![Object::Array(items)]);
    }
    let string = items.into_iter().next().unwrap_or(Object::String(PdfString::literal(Vec::new())));
    if first && op.op != b"Tj" {
        let mut operands = op.operands.clone();
        if let Some(last) = operands.last_mut() {
            *last = string;
        }
        return Op { op: op.op.clone(), operands, inline: None, span: 0..0 };
    }
    Op::new("Tj", vec![string])
}

/// The graphics and text state that places glyphs.
#[derive(Clone)]
struct State {
    ctm: Matrix,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    /// `Tz` / 100.
    scale: f64,
    leading: f64,
    rise: f64,
    metrics: Option<Rc<Metrics>>,
    /// The clipping path encloses no area (`0 0 0 0 re W n`): nothing drawn until it is restored
    /// shows, so no glyph is placed.
    hidden: bool,
    /// The clips with area that are active: a glyph whose centre is outside them shows nowhere.
    clip: Clip,
}

/// Clips that enclose some area, as far as they are followed: a rectangle (`re`, the only
/// subpath of its path) exactly, through the CTM; any other path by its shape, through the CTM,
/// while the document's clip work lasts, and past it by its bounding box, which holds the path, so
/// a glyph outside the box is hidden, and one inside it is taken as shown. Each clip narrows the
/// one before (§8.5.4), so they are kept together. In default user space.
#[derive(Clone, Default)]
struct Clip {
    /// The intersection of the clips' bounding boxes, `[x0, y0, x1, y1]`; `None` until a clip
    /// sets it. It can be empty (`x0 > x1`): then no glyph shows. It is tested first: a glyph
    /// outside it costs no clip work.
    bounds: Option<[f64; 4]>,
    /// Rectangles not aligned with the axes (a rotated or skewed CTM), each a parallelogram: the
    /// first [`MAX_CLIP_QUADS`], past which a rectangle counts by its bounding box alone.
    quads: Vec<[(f64, f64); 4]>,
    /// The other clip paths, followed by their shapes, the latest first: shared, so saving the
    /// graphics state copies none of them.
    paths: Option<Rc<ClipPath>>,
}

/// A clip path that is not one rectangle, as line segments in default user space.
struct ClipPath {
    /// Its subpaths, each closed from its last point back to its first. With each point, the
    /// curve the line ending there was flattened from, by its place in [`ClipPath::bends`] from 1
    /// (0 for a straight line).
    polygons: Vec<Polygon>,
    /// The curves its lines were flattened from ([`Bend`]).
    bends: Vec<Bend>,
    /// `W*`: the even-odd rule; else nonzero winding.
    even_odd: bool,
    /// Its segments: what a glyph test against it costs.
    segments: usize,
    /// The clip path it narrows.
    under: Option<Rc<ClipPath>>,
}

/// A subpath as a polygon: each point with the curve the line ending there was flattened from, by
/// its place in a [`Bend`] list from 1 (0 for a straight line).
type Polygon = Vec<((f64, f64), usize)>;

/// How far the lines of one curve can stray from it, and the box around its control points,
/// which holds the curve: a point within the slack of one of its lines and inside the box is
/// taken as inside, so flattening a curve hides no glyph that shows.
#[derive(Clone, Copy)]
struct Bend {
    slack: f64,
    bounds: [f64; 4],
}

impl Drop for ClipPath {
    /// One clip at a time: a long chain dropped recursively would overflow the stack.
    fn drop(&mut self) {
        let mut under = self.under.take();
        while let Some(path) = under {
            under = Rc::try_unwrap(path).ok().and_then(|mut p| p.under.take());
        }
    }
}

impl ClipPath {
    /// Whether `(x, y)` is inside the path by its rule, or on its edge or within the slack of a
    /// curve's line. A side too large to measure (an overflow) is taken as inside.
    fn holds(&self, (x, y): (f64, f64)) -> bool {
        let mut winding = 0i64;
        for polygon in &self.polygons {
            for (i, &(a, _)) in polygon.iter().enumerate() {
                // The line back to the first point, closing the subpath, is straight.
                let (b, bend) = polygon.get(i + 1).copied().unwrap_or((polygon.first().map_or(a, |f| f.0), 0));
                // Where the point is from the edge's line: left of it, going from `a` to `b`, when
                // positive.
                let side = (b.0 - a.0) * (y - a.1) - (x - a.0) * (b.1 - a.1);
                if !side.is_finite() {
                    return true;
                }
                if side == 0.0 && a.0.min(b.0) <= x && x <= a.0.max(b.0) && a.1.min(b.1) <= y && y <= a.1.max(b.1) {
                    return true;
                }
                if let Some(Bend { slack, bounds: k }) = bend.checked_sub(1).and_then(|i| self.bends.get(i)).copied()
                    && k[0] <= x
                    && x <= k[2]
                    && k[1] <= y
                    && y <= k[3]
                {
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let along = ((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy);
                    let t = if along.is_finite() { along.clamp(0.0, 1.0) } else { 0.0 };
                    let near = (x - a.0 - t * dx).hypot(y - a.1 - t * dy);
                    if !near.is_finite() || near <= slack {
                        return true;
                    }
                }
                if a.1 <= y {
                    if b.1 > y && side > 0.0 {
                        winding += 1;
                    }
                } else if b.1 <= y && side < 0.0 {
                    winding -= 1;
                }
            }
        }
        if self.even_odd { winding % 2 != 0 } else { winding != 0 }
    }
}

impl Clip {
    /// Narrows the clip to `path`'s, when `path` has points and they are all finite. `even_odd`
    /// is its rule; `work` is the document's clip work left, which keeping a path's segments
    /// spends.
    fn narrow(&mut self, path: &Path, even_odd: bool, work: &mut usize) {
        let Some(b) = path.bounds.filter(|_| path.finite) else { return };
        match path.shape {
            Shape::Rect(q) => {
                if !(q[0].1 == q[1].1 && q[1].0 == q[2].0 || q[0].0 == q[1].0 && q[1].1 == q[2].1) && self.quads.len() < MAX_CLIP_QUADS {
                    self.quads.push(q);
                }
            }
            Shape::Other => {
                let segments = path.segments();
                if let Some(left) = work.checked_sub(segments) {
                    *work = left;
                    let (polygons, bends) = path.polygons();
                    self.paths = Some(Rc::new(ClipPath { polygons, bends, even_odd, segments, under: self.paths.take() }));
                }
            }
            Shape::Empty => {}
        }
        self.bounds = Some(match self.bounds {
            None => b,
            Some(a) => [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])],
        });
    }

    /// Whether a glyph centred at `p` shows: inside, or on the edge of, every clip. A point that
    /// isn't finite has no place to test, and is taken as shown. Each clip path it is tested
    /// against spends `work`; once `work` can't pay for one, it and the clip paths it narrows
    /// count by their boxes.
    fn shows(&self, (x, y): (f64, f64), work: &mut usize) -> bool {
        if !(x.is_finite() && y.is_finite()) {
            return true;
        }
        if let Some(b) = self.bounds
            && !(b[0] <= x && x <= b[2] && b[1] <= y && y <= b[3])
        {
            return false;
        }
        let mut path = self.paths.as_deref();
        while let Some(p) = path {
            let Some(left) = work.checked_sub(p.segments) else { break };
            *work = left;
            if !p.holds((x, y)) {
                return false;
            }
            path = p.under.as_deref();
        }
        self.quads.iter().all(|q| {
            let side = |i: usize| {
                let (a, b) = (q[i], q[(i + 1) % 4]);
                (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
            };
            let sides = [side(0), side(1), side(2), side(3)];
            // A side too large to measure (an overflow) is taken as shown.
            sides.iter().any(|s| !s.is_finite()) || sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0)
        })
    }
}

/// Whether a path is one rectangle.
#[derive(Clone, Copy, Default)]
enum Shape {
    #[default]
    Empty,
    /// One `re` and nothing else: its corners in default user space, in order around it.
    Rect([(f64, f64); 4]),
    Other,
}

impl Default for State {
    fn default() -> Self {
        State {
            ctm: Matrix::IDENTITY,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            metrics: None,
            hidden: false,
            clip: Clip::default(),
        }
    }
}

/// The path being built, in device space: for the current subpath its first point, a second
/// point apart from it, and whether any point lies off the line through them; whether any subpath
/// so far does; the box around its points; whether it is one rectangle; and its subpaths, for a
/// clip to follow its shape. For the area and the box, curves are followed by their control
/// points, whose hull holds the curve.
struct Path {
    first: Option<(f64, f64)>,
    second: Option<(f64, f64)>,
    area: bool,
    points: bool,
    bounds: Option<[f64; 4]>,
    finite: bool,
    shape: Shape,
    /// Each subpath from its first point, as it was drawn.
    subpaths: Vec<Vec<Segment>>,
    /// The last subpath was closed (`h`, or `re`): a line or curve after it starts a new subpath
    /// at its first point.
    closed: bool,
}

/// A piece of a subpath, in device space: a line to a point (the subpath's first point, for its
/// first piece), or a Bézier curve's two control points and its end.
#[derive(Clone, Copy)]
enum Segment {
    Line((f64, f64)),
    Curve([(f64, f64); 3]),
}

impl Default for Path {
    fn default() -> Self {
        Path {
            first: None,
            second: None,
            area: false,
            points: false,
            bounds: None,
            finite: true,
            shape: Shape::Empty,
            subpaths: Vec::new(),
            closed: false,
        }
    }
}

impl Path {
    /// `m` or `re`: a new subpath.
    fn start(&mut self) {
        (self.first, self.second) = (None, None);
    }

    fn add(&mut self, p: (f64, f64)) {
        self.points = true;
        self.finite &= p.0.is_finite() && p.1.is_finite();
        self.bounds = Some(match self.bounds {
            None => [p.0, p.1, p.0, p.1],
            Some(b) => [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)],
        });
        match (self.first, self.second) {
            _ if self.area => {}
            (None, _) => self.first = Some(p),
            (Some(a), None) => {
                if p != a {
                    self.second = Some(p);
                }
            }
            // Not `== 0.0`: a NaN counts as area, so it hides nothing.
            (Some(a), Some(b)) => self.area = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0) != 0.0,
        }
    }

    /// The path has points, and each subpath's lie on one line.
    fn flat(&self) -> bool {
        self.points && !self.area
    }

    /// Adds a drawn piece: `start` begins a new subpath with it (`m`, `re`); otherwise it goes on
    /// the current subpath, or on a new one from where a closed one began.
    fn draw(&mut self, start: bool, segment: Segment) {
        let reopen = if self.closed && !start { self.subpaths.last().and_then(|s| s.first()).copied() } else { None };
        self.closed = false;
        if start || reopen.is_some() || self.subpaths.is_empty() {
            self.subpaths.push(reopen.into_iter().collect());
        }
        let Some(subpath) = self.subpaths.last_mut() else { return };
        match segment {
            // A curve with no point to start from (no `m` before it) starts the subpath at its end.
            Segment::Curve([_, _, end]) if subpath.is_empty() => subpath.push(Segment::Line(end)),
            _ => subpath.push(segment),
        }
    }

    /// The current point: where the last piece ended, or where a closed subpath began.
    fn current(&self) -> Option<(f64, f64)> {
        let subpath = self.subpaths.last()?;
        let segment = if self.closed { subpath.first() } else { subpath.last() };
        segment.map(|&(Segment::Line(p) | Segment::Curve([_, _, p]))| p)
    }

    /// `h`: the current subpath is closed.
    fn close(&mut self) {
        self.closed = !self.subpaths.is_empty();
    }

    /// The line segments [`Path::polygons`] gives.
    fn segments(&self) -> usize {
        self.subpaths
            .iter()
            .flatten()
            .map(|s| match s {
                Segment::Line(_) => 1,
                Segment::Curve(_) => CURVE_SEGMENTS,
            })
            .fold(0usize, usize::saturating_add)
    }

    /// Each subpath as the points of a polygon, its curves flattened to [`CURVE_SEGMENTS`] lines,
    /// each point with the curve the line ending there came from; and those curves. A line of a
    /// curve strays at most an eighth of the curve's largest second derivative times the square
    /// of the line's share of it: 3/4 of the largest second difference of its points, over the
    /// lines squared.
    fn polygons(&self) -> (Vec<Polygon>, Vec<Bend>) {
        let mut bends: Vec<Bend> = Vec::new();
        let polygons = self
            .subpaths
            .iter()
            .map(|subpath| {
                let mut points: Polygon = Vec::new();
                for &segment in subpath {
                    match segment {
                        Segment::Line(p) => points.push((p, 0)),
                        Segment::Curve([b, c, d]) => {
                            let a = points.last().map_or(d, |l| l.0);
                            let second = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| (p.0 - 2.0 * q.0 + r.0).hypot(p.1 - 2.0 * q.1 + r.1);
                            let n = CURVE_SEGMENTS as f64;
                            let slack = 0.75 * second(a, b, c).max(second(b, c, d)) / (n * n);
                            let (xs, ys) = ([a.0, b.0, c.0, d.0], [a.1, b.1, c.1, d.1]);
                            let bounds = [
                                xs.into_iter().fold(f64::INFINITY, f64::min),
                                ys.into_iter().fold(f64::INFINITY, f64::min),
                                xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
                                ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
                            ];
                            bends.push(Bend { slack, bounds });
                            let bend = bends.len();
                            for k in 1..=CURVE_SEGMENTS {
                                let t = k as f64 / CURVE_SEGMENTS as f64;
                                let u = 1.0 - t;
                                let (wa, wb, wc, wd) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                                points.push(((wa * a.0 + wb * b.0 + wc * c.0 + wd * d.0, wa * a.1 + wb * b.1 + wc * c.1 + wd * d.1), bend));
                            }
                        }
                    }
                }
                points
            })
            .collect();
        (polygons, bends)
    }
}

/// Consecutive glyphs of one operator with one owner, and the operands that show them.
struct Run {
    owner: Option<usize>,
    /// The glyphs are clipped out of view: shown nowhere, so no link's.
    hidden: bool,
    items: Vec<Object>,
    /// What its glyphs read as, together.
    reads: Reads,
}

/// What glyphs read as, from least to most: a run reads as its most.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reads {
    /// Whitespace and control characters ([`blank`]).
    Nothing,
    /// No link's, and no text with a control code ([`Placed::quiet`]).
    Unknown,
    Text,
}

/// The text inside a link's areas, for its description: every glyph there, whichever link owns it,
/// so a link with another inside it is described by all of its text.
#[derive(Clone, Default)]
struct Covered {
    /// At most [`MAX_CONTENTS`] characters, with whitespace as the description writes it (none
    /// before the first word, one space between words): only what the description can use is
    /// kept, and counted.
    text: String,
    chars: usize,
    /// Whitespace, or a jump, came after the last character kept.
    space: bool,
    /// Where the last glyph taken ended on its baseline, in default user space.
    end: Option<(f64, f64)>,
}

impl Covered {
    /// Adds a glyph's text; a space first when the glyph doesn't start near where the last one
    /// ended (a word gap in `TJ`, a line move, another text object elsewhere). `base` maps the
    /// glyph's space, without the text rise, to default user space: the jump is measured back in
    /// glyph space, so it is in units of the font size whatever the scaling, and a rise isn't a
    /// jump. `start` and `end` are the glyph's ends on its baseline, in default user space.
    fn take(&mut self, base: &Matrix, start: (f64, f64), end: (f64, f64), text: &str) {
        let jumped = self.end.is_some_and(|last| {
            let [a, b, c, d, _, _] = base.0;
            let (ux, uy, det) = (start.0 - last.0, start.1 - last.1, a * d - b * c);
            let (x, y) = ((d * ux - c * uy) / det, (a * uy - b * ux) / det);
            x.hypot(y) > WORD_GAP
        });
        self.end = Some(end);
        self.space |= jumped;
        for c in text.chars() {
            if c.is_whitespace() {
                self.space = true;
                continue;
            }
            let gap = self.space && !self.text.is_empty();
            if self.chars.saturating_add(usize::from(gap)) >= MAX_CONTENTS {
                return;
            }
            if gap {
                self.text.push(' ');
                self.chars += 1;
            }
            self.text.push(c);
            self.chars += 1;
            self.space = false;
        }
    }
}

struct Walk<'a> {
    state: State,
    saved: Vec<State>,
    unsaved: usize,
    tm: Matrix,
    tlm: Matrix,
    /// The text matrix is unknown: a string was shown whose glyphs couldn't be measured. Positions
    /// are known again after the next line move or `Tm`.
    lost: bool,
    /// Glyph-in-area tests left in the document's allowance; a glyph near the links costs one per
    /// area.
    tests: usize,
    /// The page's link areas, each with the index of its link.
    areas: Vec<(usize, &'a Area)>,
    /// Each link's text, for its description.
    texts: Vec<Covered>,
    /// A glyph was left unplaced because the allowance was spent.
    exhausted: bool,
    /// The box around all the page's link areas: a glyph outside it is tested against none of them,
    /// and costs nothing.
    bounds: [f64; 4],
    /// Clip-path work left in the document's allowance ([`MAX_CLIP_WORK`]).
    clip_work: usize,
    /// The current path, and whether `W` (`Some(false)`) or `W*` (`Some(true)`) makes it a clip
    /// when it is ended.
    path: Path,
    clip: Option<bool>,
}

impl Walk<'_> {
    /// Follows `op`; for a text-showing operator that shows at least one glyph, its glyphs grouped
    /// into runs by owner.
    fn step(&mut self, op: &Op) -> Option<Vec<Run>> {
        let s = &mut self.state;
        match op.op.as_slice() {
            b"q" if self.saved.len() < MAX_SAVED => self.saved.push(s.clone()),
            b"q" => self.unsaved = self.unsaved.saturating_add(1),
            b"Q" if self.unsaved > 0 => self.unsaved -= 1,
            b"Q" => {
                if let Some(saved) = self.saved.pop() {
                    *s = saved;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    s.ctm = Matrix(m).then(&s.ctm);
                }
            }
            b"Tf" => s.size = op.num(1).unwrap_or(s.size),
            b"Tc" => s.char_spacing = op.num(0).unwrap_or(s.char_spacing),
            b"Tw" => s.word_spacing = op.num(0).unwrap_or(s.word_spacing),
            b"Tz" => s.scale = op.num(0).map_or(s.scale, |z| z / 100.0),
            b"TL" => s.leading = op.num(0).unwrap_or(s.leading),
            b"Ts" => s.rise = op.num(0).unwrap_or(s.rise),
            b"BT" => (self.tm, self.tlm, self.lost) = (Matrix::IDENTITY, Matrix::IDENTITY, false),
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.op == b"TD" {
                        s.leading = -y;
                    }
                    self.tlm = Matrix::translate(x, y).then(&self.tlm);
                    (self.tm, self.lost) = (self.tlm, false);
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    (self.tm, self.tlm, self.lost) = (Matrix(m), Matrix(m), false);
                }
            }
            b"T*" => self.next_line(),
            b"Tj" | b"'" | b"\"" | b"TJ" => return self.show(op),
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" => self.path_points(op),
            b"h" => self.path.close(),
            b"W" | b"W*" => self.clip = Some(op.op == b"W*"),
            // A clip only narrows until `Q`, so once it encloses nothing it stays so. Past
            // `MAX_SAVED`, a `Q` would not restore it, so text is then taken as shown.
            b"n" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" => {
                if let Some(even_odd) = self.clip.take()
                    && self.unsaved == 0
                {
                    if self.path.flat() {
                        self.state.hidden = true;
                    } else {
                        self.state.clip.narrow(&self.path, even_odd, &mut self.clip_work);
                    }
                }
                self.path = Path::default();
            }
            _ => {}
        }
        None
    }

    fn next_line(&mut self) {
        self.tlm = Matrix::translate(0.0, -self.state.leading).then(&self.tlm);
        (self.tm, self.lost) = (self.tlm, false);
    }

    /// A path-building operator's points, in device space.
    fn path_points(&mut self, op: &Op) {
        let ctm = self.state.ctm;
        let points: Vec<(f64, f64)> = match op.op.as_slice() {
            b"m" | b"l" => op.nums::<2>().map(|[x, y]| vec![(x, y)]),
            b"c" => op.nums::<6>().map(|[a, b, c, d, e, f]| vec![(a, b), (c, d), (e, f)]),
            b"v" | b"y" => op.nums::<4>().map(|[a, b, c, d]| vec![(a, b), (c, d)]),
            b"re" => op.nums::<4>().map(|[x, y, w, h]| vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)]),
            _ => None,
        }
        .unwrap_or_default();
        // An operator whose operands don't read is skipped, as the renderer skips it: it starts
        // no subpath.
        if points.is_empty() {
            return;
        }
        let start = matches!(op.op.as_slice(), b"m" | b"re");
        if start {
            self.path.start();
        }
        let points: Vec<(f64, f64)> = points.into_iter().map(|(x, y)| ctm.apply(x, y)).collect();
        self.path.shape = match (self.path.shape, points.as_slice()) {
            (Shape::Empty, &[a, b, c, d]) if op.op == b"re" => Shape::Rect([a, b, c, d]),
            _ => Shape::Other,
        };
        // `v` puts its curve's first control point at the current point, and `y` its second at
        // the curve's end.
        let at = self.path.current();
        match (op.op.as_slice(), points.as_slice()) {
            (b"re", &[a, b, c, d]) => {
                for (i, p) in [a, b, c, d].into_iter().enumerate() {
                    self.path.draw(i == 0, Segment::Line(p));
                }
                self.path.close();
            }
            (b"c", &[b, c, d]) => self.path.draw(false, Segment::Curve([b, c, d])),
            (b"v", &[c, d]) => self.path.draw(false, Segment::Curve([at.unwrap_or(c), c, d])),
            (b"y", &[b, d]) => self.path.draw(false, Segment::Curve([b, d, d])),
            (_, &[p]) => self.path.draw(start, Segment::Line(p)),
            _ => {}
        }
        for p in points {
            self.path.add(p);
        }
    }

    fn show(&mut self, op: &Op) -> Option<Vec<Run>> {
        if op.op == b"\"" {
            self.state.word_spacing = op.num(0).unwrap_or(self.state.word_spacing);
            self.state.char_spacing = op.num(1).unwrap_or(self.state.char_spacing);
        }
        if op.op != b"Tj" && op.op != b"TJ" {
            self.next_line();
        }
        let items: Vec<Object> = match op.op.as_slice() {
            // A surplus operand is malformed; the operator takes the one right before it, as the
            // other readers here do.
            b"TJ" => op.operands.last().and_then(Object::as_array).cloned().unwrap_or_default(),
            _ => op.operands.last().cloned().into_iter().collect(),
        };
        let mut runs: Vec<Run> = Vec::new();
        // Adjustments before the first glyph go with the first run.
        let mut pending: Vec<Object> = Vec::new();
        for item in items {
            let Object::String(string) = &item else {
                let dx = -item.as_f64().unwrap_or(0.0) / 1000.0 * self.state.size * self.state.scale;
                self.tm = Matrix::translate(dx, 0.0).then(&self.tm);
                match runs.last_mut() {
                    Some(run) => run.items.push(item),
                    None => pending.push(item),
                }
                continue;
            };
            for Glyphs { owner, hidden, bytes, reads } in self.glyphs(string) {
                let piece = Object::String(PdfString { bytes, hex: string.hex });
                match runs.last_mut() {
                    Some(run) if run.owner == owner && run.hidden == hidden => {
                        run.items.push(piece);
                        run.reads = run.reads.max(reads);
                    }
                    _ => {
                        let mut items = std::mem::take(&mut pending);
                        items.push(piece);
                        runs.push(Run { owner, hidden, items, reads });
                    }
                }
            }
        }
        if let Some(run) = runs.last_mut() {
            run.items.append(&mut pending);
        }
        // Hidden glyphs matter only beside a link's ([`settle`]): an operator with none of a link's
        // and visible text is split nowhere, since that text stays its own wherever it is. With
        // only blank or quiet text visible, the hidden part is kept apart, to be an artifact.
        if runs.len() > 1 && runs.iter().all(|r| r.owner.is_none()) && runs.iter().any(|r| !r.hidden && r.reads == Reads::Text) {
            let items = runs.into_iter().flat_map(|r| r.items).collect();
            runs = vec![Run { owner: None, hidden: false, items, reads: Reads::Text }];
        }
        (!runs.is_empty()).then_some(runs)
    }

    /// The glyphs of `string` grouped by owner and by whether they are clipped out of view, in
    /// order. A string whose glyphs can't be placed is one group with no owner, read as text and
    /// taken as shown, unless a clip that encloses no area hides everything. Each glyph's text
    /// goes to every link it is in.
    fn glyphs(&mut self, string: &PdfString) -> Vec<Glyphs> {
        let bytes = &string.bytes;
        if bytes.is_empty() {
            return Vec::new();
        }
        // Clipped out of view: shown nowhere, so in no link's area; it never interrupts a link
        // ([`settle`]).
        if self.state.hidden {
            return vec![Glyphs { owner: None, hidden: true, bytes: bytes.clone(), reads: Reads::Unknown }];
        }
        let s = &self.state;
        let usable = s.metrics.as_ref().filter(|m| !m.codespaces_dropped && !m.past_allowance);
        let codes = usable
            .map(|m| m.codes(bytes))
            .filter(|c| c.iter().map(|&(_, len)| len).sum::<usize>() == bytes.len() && c.iter().all(|&(_, len)| len > 0));
        let (Some(m), Some(codes), false) = (usable, codes, self.lost) else {
            self.lost = true;
            return vec![Glyphs { owner: None, hidden: false, bytes: bytes.clone(), reads: Reads::Text }];
        };
        let m = Rc::clone(m);
        let mut out: Vec<Glyphs> = Vec::new();
        let mut at = 0usize;
        for (code, len) in codes {
            let s = &self.state;
            let width = m.width(code);
            let glyph = Matrix([s.size * s.scale, 0.0, 0.0, s.size, 0.0, s.rise]).then(&self.tm).then(&s.ctm);
            let centre = glyph.apply(width / 2.0, (m.ascent + m.descent) / 2.0);
            let hidden = !s.clip.shows(centre, &mut self.clip_work);
            let b = self.bounds;
            let near = !hidden && b[0] <= centre.0 && centre.0 <= b[2] && b[1] <= centre.1 && centre.1 <= b[3];
            let (owner, inside) = if near {
                let Some(left) = self.tests.checked_sub(self.areas.len()) else {
                    self.exhausted = true;
                    break;
                };
                self.tests = left;
                links_at(&self.areas, centre)
            } else {
                (None, Vec::new())
            };
            let piece = bytes.get(at..at.saturating_add(len)).unwrap_or_default();
            at = at.saturating_add(len);
            // A glyph with no text isn't known to read as nothing: a font with no Unicode mapping
            // still shows text, a link's or a paragraph's. Only a control code is set apart.
            let reads = match m.text_of(code) {
                Some(t) if blank(&t) => Reads::Nothing,
                None if owner.is_none() && !m.composite && len == 1 && code < 32 => Reads::Unknown,
                _ => Reads::Text,
            };
            match out.last_mut() {
                Some(last) if last.owner == owner && last.hidden == hidden => {
                    last.bytes.extend_from_slice(piece);
                    last.reads = last.reads.max(reads);
                }
                _ => out.push(Glyphs { owner, hidden, bytes: piece.to_vec(), reads }),
            }
            let spacing = if m.is_space(code, len) { s.word_spacing } else { 0.0 };
            let advance = (width * s.size + s.char_spacing + spacing) * s.scale;
            let base = Matrix([s.size * s.scale, 0.0, 0.0, s.size, 0.0, 0.0]).then(&self.tm).then(&s.ctm);
            self.tm = Matrix::translate(advance, 0.0).then(&self.tm);
            if !inside.is_empty() {
                let s = &self.state;
                let end = Matrix([s.size * s.scale, 0.0, 0.0, s.size, 0.0, 0.0]).then(&self.tm).then(&s.ctm).apply(0.0, 0.0);
                let text = m.text_of(code).map(|t| t.into_owned()).unwrap_or_default();
                for link in inside {
                    if let Some(covered) = self.texts.get_mut(link) {
                        covered.take(&base, base.apply(0.0, 0.0), end, &text);
                    }
                }
            }
        }
        out
    }
}

/// Consecutive glyphs of one string with one owner, all clipped out of view or none, and their
/// codes.
struct Glyphs {
    owner: Option<usize>,
    hidden: bool,
    bytes: Vec<u8>,
    reads: Reads,
}

/// The link whose smallest area holds `point` (its owner), and every link with an area holding it.
fn links_at(areas: &[(usize, &Area)], point: (f64, f64)) -> (Option<usize>, Vec<usize>) {
    if !(point.0.is_finite() && point.1.is_finite()) {
        return (None, Vec::new());
    }
    let (mut owner, mut inside): (Option<(usize, f64)>, Vec<usize>) = (None, Vec::new());
    for &(link, area) in areas.iter().filter(|(_, a)| a.holds(point)) {
        if owner.is_none_or(|(_, size)| area.size < size) {
            owner = Some((link, area.size));
        }
        // A link's areas are next to each other in `areas`.
        if inside.last() != Some(&link) {
            inside.push(link);
        }
    }
    (owner.map(|(link, _)| link), inside)
}

/// What holds a marked-content sequence of a text object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    /// The text object's own element.
    Own,
    /// The link's Link element.
    Link(usize),
    /// No element: text clipped out of view inside a link's text ([`settle`]).
    Artifact,
}

impl Part {
    pub(crate) fn link(self) -> Option<usize> {
        match self {
            Part::Link(link) => Some(link),
            _ => None,
        }
    }
}

/// The marked-content sequences of the text object `(first, last)` (its `BT` and `ET`): the whole
/// object, its own, when none of its glyphs belongs to a link or is an artifact. Otherwise one
/// sequence per run of glyph-showing operators with the same [`Part`], which together cover
/// everything between `BT` and `ET`: an operator that shows no glyph (`() Tj`, `[-250] TJ`) is
/// still text-showing content, and must be in a sequence too. `artifacts` are the operators that
/// [`settle`] made artifacts.
pub(crate) fn sequences(
    (first, last): (usize, usize),
    owners: &HashMap<usize, Option<usize>>,
    artifacts: &HashSet<usize>,
) -> Vec<(usize, usize, Part)> {
    let part = |at: usize| {
        let owner = owners.get(&at)?;
        Some(if artifacts.contains(&at) { Part::Artifact } else { owner.map_or(Part::Own, Part::Link) })
    };
    if !(first..=last).any(|at| part(at).is_some_and(|p| p != Part::Own)) {
        return vec![(first, last, Part::Own)];
    }
    let mut out: Vec<(usize, usize, Part)> = Vec::new();
    for at in first..=last {
        let Some(owner) = part(at) else { continue };
        match out.last_mut() {
            Some(run) if run.2 == owner => run.1 = at,
            _ => out.push((at, at, owner)),
        }
    }
    // Each sequence runs on to the one after it; the first starts after BT, the last ends before ET.
    let starts: Vec<usize> = out.iter().skip(1).map(|r| r.0).collect();
    for (run, next) in out.iter_mut().zip(starts.into_iter().map(Some).chain(std::iter::once(None))) {
        run.1 = next.map_or(last.saturating_sub(1), |n| n.saturating_sub(1));
    }
    if let Some(run) = out.first_mut() {
        run.0 = first.saturating_add(1);
    }
    out
}

/// Whether `text` reads as nothing: only whitespace and control characters.
fn blank(text: &str) -> bool {
    text.chars().all(|c| c.is_whitespace() || c.is_control())
}

/// Gives the text that reads as nothing an owner from the text around it, in the order the page
/// shows it, so that it never decides whether a link's text is in one piece: a space at the end
/// of a line before a link goes on, or a text object of spaces (or of no glyph at all) between two
/// parts of one link, joins that link. Other blank text goes with the text before it (or, at the
/// start, after it); a text object that shows no glyph stays its own unless it lies inside one
/// link's text, and so does an operator of [`Placed::quiet`] control codes. Text clipped out of
/// view ([`Placed::hidden`]) stays its own too, and inside one link's text is an artifact, in no
/// element: it isn't the link's text, and doesn't interrupt it. `spans` are the page's text
/// objects; a text object that joins a link is keyed by its `BT` in `owners`, whatever joins a
/// link is added to `blank`, and the artifacts are returned.
pub(crate) fn settle(
    spans: &[(usize, usize)],
    owners: &mut HashMap<usize, Option<usize>>,
    blank: &mut HashSet<usize>,
    quiet: &HashSet<usize>,
    hidden: &HashSet<usize>,
) -> HashSet<usize> {
    let mut artifacts = HashSet::new();
    // Each glyph-showing operator, or a text object with none, in order: (key, owner, reads as
    // nothing, joins only a link it lies inside).
    let mut items: Vec<(usize, Option<usize>, bool, bool)> = Vec::new();
    for &(first, last) in spans {
        let before = items.len();
        items.extend((first..=last).filter_map(|at| {
            let only_inside = quiet.contains(&at) || hidden.contains(&at);
            owners.get(&at).map(|&o| (at, o, only_inside || blank.contains(&at), only_inside))
        }));
        if items.len() == before {
            items.push((first, None, true, true));
        }
    }
    let mut after: Vec<Option<Option<usize>>> = vec![None; items.len()];
    let mut next = None;
    for (slot, item) in after.iter_mut().zip(&items).rev() {
        *slot = next;
        if !item.2 {
            next = Some(item.1);
        }
    }
    let mut before: Option<Option<usize>> = None;
    for (&(at, owner, is_blank, only_inside), next) in items.iter().zip(after) {
        if !is_blank {
            before = Some(owner);
            continue;
        }
        let inside = before.filter(|b| b.is_some() && Some(*b) == next).flatten();
        // A whole text object, or a quiet operator, has no owner of its own to keep.
        let chosen = if only_inside { inside } else { before.or(next).unwrap_or(owner) };
        // Blank text before any text that reads joins what comes after it, and so is what comes
        // before the blank and hidden text after it.
        if before.is_none() && !only_inside && next.is_some() {
            before = Some(chosen);
        }
        if chosen.is_some() && hidden.contains(&at) {
            artifacts.insert(at);
        } else if chosen != owner {
            owners.insert(at, chosen);
            blank.insert(at);
        }
    }
    artifacts
}

/// The first link whose text something else interrupts, in the order the page shows its text:
/// `(link, what came between)`, where `None` is text that is no link's. A Link element holds its
/// text in one piece; text of a link after an interruption could stay in the Link only by being
/// read out of order, and could keep its order only by leaving the Link. A link inside another
/// interrupts it unless it covers the start or end of the other's text.
/// An artifact interrupts nothing.
pub(crate) fn interrupted(
    spans: &[(usize, usize)],
    owners: &HashMap<usize, Option<usize>>,
    artifacts: &HashSet<usize>,
) -> Option<(usize, Option<usize>)> {
    // Each link whose text has ended, and what came right after it.
    let mut ended: HashMap<usize, Option<usize>> = HashMap::new();
    let mut last: Option<Option<usize>> = None;
    for &span in spans {
        for (_, _, part) in sequences(span, owners, artifacts) {
            if part == Part::Artifact {
                continue;
            }
            let owner = part.link();
            if last == Some(owner) {
                continue;
            }
            if let Some(Some(link)) = last {
                ended.entry(link).or_insert(owner);
            }
            if let Some(link) = owner
                && let Some(&by) = ended.get(&link)
            {
                return Some((link, by));
            }
            last = Some(owner);
        }
    }
    None
}

/// How a refusal names `link`.
pub(crate) fn name(links: &[Link], link: usize) -> String {
    links.get(link).map_or_else(
        || "a link annotation".to_string(),
        |l| format!("link annotation {} in /Annots (object {} {})", l.position, l.annot.num, l.annot.generation),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{Document, ObjRef, Object};

    use crate::{AutoTagError, Options, Rule, Status, autotag_text, check, tag_content, tags};

    /// A PDF from object bodies (object n = bodies[n - 1]).
    fn pdf(bodies: &[String]) -> Document {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, b) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    fn stream(data: &str) -> String {
        format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
    }

    /// One page (object 3) with `content`, Courier as /F1 (each glyph 0.6 em wide), an image as /Im1
    /// and the annotations in `annots` as objects 7 onwards. `page` adds entries to the page.
    const COURIER: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>";

    fn page_with(content: &str, annots: &[&str], page: &str) -> Document {
        page_in(content, annots, page, COURIER)
    }

    fn page_in(content: &str, annots: &[&str], page: &str, font: &str) -> Document {
        page_res(content, annots, page, font, "")
    }

    /// As [`page_in`], with `resources` added to the page's resource dictionary.
    fn page_res(content: &str, annots: &[&str], page: &str, font: &str, resources: &str) -> Document {
        let refs: Vec<String> = (0..annots.len()).map(|i| format!("{} 0 R", i + 7)).collect();
        let mut bodies = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> {resources} >> /Annots [{}] {page} >>",
                refs.join(" ")
            ),
            stream(content),
            font.into(),
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream"
                .into(),
        ];
        bodies.extend(annots.iter().map(|a| a.to_string()));
        pdf(&bodies)
    }

    fn page(content: &str, annots: &[&str]) -> Document {
        page_with(content, annots, "")
    }

    fn link(rect: &str) -> String {
        format!("<< /Type /Annot /Subtype /Link /Rect [{rect}] /Border [0 0 0] /A << /S /URI /URI (https://example.org/) >> >>")
    }

    /// Courier at 10 pt from x = 20 on the line at y = 300: character `i` spans 20 + 6i to 26 + 6i.
    /// The rectangle around characters `from..to` of that line.
    fn over(from: usize, to: usize) -> String {
        format!("{} 296 {} 312", 20 + 6 * from, 20 + 6 * to)
    }

    const LINE: &str = "BT /F1 10 Tf 20 300 Td (See the guide here) Tj ET";

    /// Every tag as (kind, its parent's kind, the text of the content it owns directly).
    fn tree(doc: &Document) -> Vec<(String, String, Vec<String>)> {
        let all = tags(doc);
        let kind = |r: Option<ObjRef>| all.iter().find(|t| Some(t.object) == r).map(|t| t.kind.clone()).unwrap_or_default();
        all.iter().map(|t| (t.kind.clone(), kind(t.parent), tag_content(doc, t.object).unwrap().into_iter().map(|c| c.text).collect())).collect()
    }

    /// The text of the marked content in structure order: the tree read depth first, as a screen
    /// reader reads it.
    fn reading(doc: &Document) -> String {
        fn walk(doc: &Document, at: ObjRef, text: &std::collections::HashMap<i64, String>, out: &mut String) {
            let element = doc.get(at);
            let kids = match element.as_dict().and_then(|d| d.get(b"K")).cloned() {
                Some(Object::Array(kids)) => kids,
                Some(kid) => vec![kid],
                None => Vec::new(),
            };
            for kid in kids {
                match kid {
                    Object::Int(mcid) => out.push_str(&text[&mcid]),
                    Object::Ref(r) => walk(doc, r, text, out),
                    _ => {}
                }
            }
        }
        let all = tags(doc);
        let text = all.iter().flat_map(|t| tag_content(doc, t.object).unwrap()).map(|c| (c.mcid, c.text)).collect();
        let mut out = String::new();
        walk(doc, all.iter().find(|t| t.kind == "Document").unwrap().object, &text, &mut out);
        out
    }

    fn links(doc: &Document) -> Vec<(Vec<String>, String)> {
        tree(doc).into_iter().filter(|t| t.0 == "Link").map(|t| (t.2, t.1)).collect()
    }

    /// The structure element the parent tree gives for an annotation's `/StructParent`, if that
    /// element holds the annotation through an OBJR.
    fn owner_of(doc: &Document, annot: ObjRef) -> Option<ObjRef> {
        let key = doc.get(annot).as_dict()?.int(b"StructParent")?;
        let catalog = doc.get(doc.root()?);
        let root = doc.get(catalog.as_dict()?.reference(b"StructTreeRoot")?);
        let tree = doc.get(root.as_dict()?.reference(b"ParentTree")?);
        let nums = tree.as_dict()?.get(b"Nums")?.as_array()?.clone();
        let element = nums.chunks(2).find(|p| p[0].as_int() == Some(key)).and_then(|p| match p[1] {
            Object::Ref(r) => Some(r),
            _ => None,
        })?;
        let dict = doc.get(element);
        let kids = dict.as_dict()?.get(b"K")?.as_array()?.clone();
        kids.iter().any(|k| k.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"OBJR") && d.reference(b"Obj") == Some(annot))).then_some(element)
    }

    fn passes(doc: &Document) {
        let rules = [Rule::TaggedPdf, Rule::TaggedContent, Rule::TaggedAnnotations, Rule::TabOrder];
        let report = check(doc, &Options { rules: rules.into(), pages: None });
        for rule in rules {
            assert_eq!(report.result(rule).unwrap().status, Status::Passed, "{rule:?}");
        }
    }

    /// The page's text-showing operators' strings, joined: what the page shows, in order.
    fn shown(doc: &Document) -> Vec<u8> {
        let page = &pdfcraft_model::pages(doc)[0];
        let content = doc.resolve(page.dict.get(b"Contents").unwrap());
        let Object::Stream(content) = &*content else { panic!("page content") };
        let mut out = Vec::new();
        for op in pdfcraft_content::parse(&content.decoded_strict().unwrap()).ops {
            let items = match op.op.as_slice() {
                b"TJ" => op.operands[0].as_array().unwrap().clone(),
                b"Tj" | b"'" | b"\"" => vec![op.operands.last().unwrap().clone()],
                _ => continue,
            };
            for item in items {
                if let Object::String(s) = item {
                    out.extend(s.bytes);
                }
            }
        }
        out
    }

    fn annot_dict(doc: &Document, n: u32) -> pdfcraft_cos::Dict {
        doc.get(ObjRef::new(n, 0)).as_dict().unwrap().clone()
    }

    #[test]
    fn a_link_over_a_whole_line_owns_its_text_and_its_annotation() {
        let mut doc = page("BT /F1 10 Tf 20 300 Td (Read the guide) Tj ET", &[&link(&over(0, 14))]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["Read the guide".to_string()], "P".to_string())]);
        let element = owner_of(&doc, ObjRef::new(7, 0)).expect("the annotation's parent tree entry is its Link tag");
        assert!(tags(&doc).iter().any(|t| t.object == element && t.kind == "Link"));
        let page = &pdfcraft_model::pages(&doc)[0];
        assert_eq!(page.dict.name(b"Tabs"), Some(b"S".as_slice()));
        assert_eq!(annot_dict(&doc, 7).get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text()).as_deref(), Some("Read the guide"));
        passes(&doc);
        let bytes = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let reopened = Document::open(Arc::new(bytes)).unwrap();
        assert!(owner_of(&reopened, ObjRef::new(7, 0)).is_some());
        passes(&reopened);
    }

    #[test]
    fn a_link_over_part_of_a_string_owns_only_that_part_and_the_page_shows_the_same_text() {
        let mut doc = page(LINE, &[&link(&over(8, 13))]);
        autotag_text(&mut doc).unwrap();
        let tree = tree(&doc);
        let p = tree.iter().find(|t| t.0 == "P").unwrap();
        assert_eq!(p.2, ["See the ", " here"]);
        assert_eq!(links(&doc), [(vec!["guide".to_string()], "P".to_string())]);
        assert_eq!(shown(&doc), b"See the guide here");
        passes(&doc);
    }

    #[test]
    fn two_links_on_one_line_each_own_their_words() {
        let mut doc = page(LINE, &[&link(&over(0, 3)), &link(&over(14, 18))]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["See".to_string()], "P".to_string()), (vec!["here".to_string()], "P".to_string())]);
        let p = tree(&doc).into_iter().find(|t| t.0 == "P").unwrap();
        assert_eq!(p.2, [" the guide "]);
        assert!(owner_of(&doc, ObjRef::new(7, 0)).is_some() && owner_of(&doc, ObjRef::new(8, 0)).is_some());
        assert_ne!(owner_of(&doc, ObjRef::new(7, 0)), owner_of(&doc, ObjRef::new(8, 0)));
        passes(&doc);
    }

    #[test]
    fn a_link_over_no_text_or_over_an_image_holds_only_its_annotation() {
        for content in [LINE.to_string(), format!("{LINE} q 40 0 0 40 300 100 cm /Im1 Do Q")] {
            let mut doc = page(&content, &[&link("300 100 340 140")]);
            autotag_text(&mut doc).unwrap();
            assert_eq!(links(&doc), [(Vec::<String>::new(), "Document".to_string())]);
            assert!(owner_of(&doc, ObjRef::new(7, 0)).is_some());
            // No text to describe it: the address does.
            assert_eq!(
                annot_dict(&doc, 7).get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text()).as_deref(),
                Some("https://example.org/")
            );
            passes(&doc);
        }
    }

    /// Refused before any change, with `why` in the reason.
    fn refused(mut doc: Document, why: &str) {
        let before = doc.clone();
        match autotag_text(&mut doc) {
            Err(AutoTagError::Unsupported { reason, .. }) => assert!(reason.contains(why), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert!(doc.get(ObjRef::new(3, 0)) == before.get(ObjRef::new(3, 0)) && tags(&doc).is_empty());
    }

    /// A Link tag holds its link's text in one piece. A link whose text something else comes
    /// between (another link inside it, text outside it on the same line or the rest of a line
    /// before it goes on) is refused, naming the link; a link inside another at either end of its
    /// text is tagged, and the tree reads in the order the page shows its text.
    #[test]
    fn a_link_whose_text_is_interrupted_is_refused_and_one_at_the_edge_of_another_is_tagged() {
        // "guide" inside a link over the whole line.
        refused(
            page(LINE, &[&link(&over(0, 18)), &link(&over(8, 13))]),
            "link annotation 1 in /Annots (object 7 0) is interrupted by link annotation 2",
        );
        // One link over "See" and "guide", with " the " between.
        let quads = format!("QuadPoints [20 296 38 296 38 312 20 312 {} 296 {} 296 {} 312 {} 312]", 20 + 48, 20 + 78, 20 + 78, 20 + 48);
        refused(page(LINE, &[&format!("<< /Type /Annot /Subtype /Link /Rect [{}] /{quads} >>", over(0, 18))]), "interrupted by text outside it");
        // A link running on into the next line after text that isn't its own.
        let content = "BT /F1 10 Tf 20 300 Td (Read the more) Tj ET BT /F1 10 Tf 20 288 Td (guide now) Tj ET";
        let quads = "QuadPoints [49 312 69 312 49 296 69 296 18 300 52 300 18 284 52 284]";
        refused(page(content, &[&format!("<< /Type /Annot /Subtype /Link /Rect [18 284 69 312] /{quads} >>")]), "interrupted by text outside it");
        // A link inside another at the end, and at the start, of its text.
        for (outer, inner, texts) in [(over(0, 13), over(8, 13), ["See the ", "guide"]), (over(0, 18), over(0, 3), ["See", " the guide here"])] {
            let mut doc = page(LINE, &[&link(&outer), &link(&inner)]);
            let shown = String::from_utf8(shown(&doc)).unwrap();
            autotag_text(&mut doc).unwrap();
            assert_eq!(reading(&doc), shown);
            let found: Vec<Vec<String>> = links(&doc).into_iter().map(|l| l.0).collect();
            assert_eq!(found, texts.map(|t| vec![t.to_string()]));
            assert!(owner_of(&doc, ObjRef::new(7, 0)).is_some() && owner_of(&doc, ObjRef::new(8, 0)).is_some());
            passes(&doc);
        }
        // The outer link is described by all the text inside it.
        let mut doc = page(LINE, &[&link(&over(0, 13)), &link(&over(8, 13))]);
        autotag_text(&mut doc).unwrap();
        let described = |n| annot_dict(&doc, n).get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text());
        assert_eq!((described(7).as_deref(), described(8).as_deref()), (Some("See the guide"), Some("guide")));
    }

    /// Text that reads as nothing doesn't interrupt a link: a space left outside a link's area at
    /// the end of a line, a text object of spaces or of no glyph between two parts of one link, a
    /// control character a writer shows far off the page after each label, and the space an outer
    /// link reaches past an inner one at its end. Each is tagged, and the tree reads in the order
    /// the page shows its text.
    #[test]
    fn text_that_reads_as_nothing_does_not_interrupt_a_link() {
        let per_line =
            "<< /Type /Annot /Subtype /Link /Rect [18 284 110 312] /QuadPoints [74 296 104 296 104 312 74 312 18 284 38 284 38 300 18 300] >>";
        let mut cases = vec![(page("BT /F1 10 Tf 12 TL 20 300 Td (Read the guide ) Tj T* (now please) Tj ET", &[per_line]), 1)];
        for between in ["( ) Tj", "() Tj", "[-250] TJ"] {
            let content =
                format!("BT /F1 10 Tf 20 300 Td (See the gu) Tj ET BT /F1 10 Tf 300 100 Td {between} ET BT /F1 10 Tf 80 300 Td (ide here) Tj ET");
            cases.push((page(&content, &[&link(&over(8, 13))]), 1));
        }
        let labels = "BT /F1 10 Tf 20 300 Td (U1) Tj 2314 2304 Td (\n) ' ET BT /F1 10 Tf 20 280 Td (GND) Tj 2314 2304 Td (\n) ' ET";
        cases.push((page(labels, &[&link("18 276 60 312")]), 1));
        cases.push((page(LINE, &[&link(&over(0, 14)), &link(&over(8, 13))]), 2));
        for (mut doc, count) in cases {
            let shown = String::from_utf8(shown(&doc)).unwrap();
            autotag_text(&mut doc).unwrap();
            let read = reading(&doc);
            // Marked-content sequences are read one after another, so compare without the spaces.
            assert_eq!(read.split_whitespace().collect::<String>(), shown.split_whitespace().collect::<String>());
            assert_eq!(links(&doc).len(), count);
            assert!(links(&doc).iter().all(|l| !l.0.is_empty()), "{:?}", links(&doc));
            passes(&doc);
        }
    }

    /// A clip of a circle drawn as four Bézier curves.
    fn circle((x, y): (f64, f64), r: f64) -> String {
        let k = 0.5523 * r;
        format!(
            "{} {y} m {} {} {} {} {x} {} c {} {} {} {} {} {y} c {} {} {} {} {x} {} c {} {} {} {} {} {y} c h W n",
            x + r,
            x + r,
            y + k,
            x + k,
            y + r,
            y + r,
            x - k,
            y + r,
            x - r,
            y + k,
            x - r,
            x - r,
            y - k,
            x - k,
            y - r,
            y - r,
            x + k,
            y - r,
            x + r,
            y - k,
            x + r
        )
    }

    /// Text drawn where a clip hides it shows nowhere, so it is in no link's area: a hidden `v` a
    /// writer draws where a citation's `[5]` is shown stays in its paragraph, and is not in the
    /// link's description; hidden text between two parts of one link is an artifact, neither in
    /// the Link nor interrupting it. A clip hides a glyph when it encloses no area, or when the
    /// glyph's centre is outside its shape through the CTM: a rectangle, the intersection of
    /// several clips, or any other path by its rule (nonzero for `W`, even-odd for `W*`), its
    /// curves followed too. A clip around the glyph, or one restored by `Q`, hides nothing.
    #[test]
    fn text_clipped_out_of_view_is_in_no_link() {
        let cite = |clip: &str| format!("q {clip} BT /F1 10 Tf 20 300 Td (v) Tj ET Q BT /F1 10 Tf 20 300 Td ([5]) Tj ET");
        let described = |doc: &Document| annot_dict(doc, 7).get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text());
        // The `v` is centred near (23, 302). A pentagram around it, drawn as one self-crossing
        // subpath: the glyph is where it winds twice, inside by the nonzero rule, outside by
        // even-odd.
        let star = |rule: &str| {
            let corner = |k: usize| {
                let a = std::f64::consts::FRAC_PI_2 + (k * 2 % 5) as f64 * std::f64::consts::TAU / 5.0;
                format!("{:.3} {:.3}", 23.0 + 60.0 * a.cos(), 302.0 + 60.0 * a.sin())
            };
            format!("{} m {} l {} l {} l {} l h {rule} n", corner(0), corner(1), corner(2), corner(3), corner(4))
        };
        let hidden = [
            "0 0 0 0 re W n",
            "0 0 0 10 re W* n",
            "20 300 m 60 300 l W n",
            "0 0 m 10 10 l 20 20 10 10 30 30 c h W f",
            "0 0 100 100 re f 0 0 0 0 re W n",
            "0 0 m 10 0 l 0 5 m 0 10 l W n",
            // A rectangle away from the glyph, with either rule, and one ended by a paint.
            "0 0 10 10 re W n",
            "0 0 10 10 re W* n",
            "0 0 10 10 re W f",
            // A rectangle away from the glyph, then one around it: the clip is where both are.
            "0 0 10 400 re W n 0 0 400 400 re W n",
            // A rectangle around the glyph moved away by `cm`, and one skewed off it whose box
            // holds it.
            "1 0 0 1 200 0 cm 10 290 100 30 re W n 1 0 0 1 -200 0 cm",
            "1 0 1 1 0 0 cm 0 0 10 400 re W n 1 0 -1 1 0 0 cm",
            // A triangle whose box is away from the glyph, and one whose box holds it.
            "100 100 m 200 100 l 150 200 l h W n",
            "0 280 m 400 280 l 400 400 l h W n",
            // A concave clip whose box holds the glyph, with a notch where the glyph is; and with a
            // wide curve far from it, whose slack is no straight edge's.
            "0 280 m 100 280 l 100 330 l 40 330 l 40 295 l 10 295 l 10 330 l 0 330 l h W n",
            "0 280 m 100 280 l 100 330 l 40 330 l 40 295 l 10 295 l 10 330 l 0 330 l h 300 280 m 300 2000 400 -1500 400 280 c h W n",
            // And with a curve whose lines' slack reaches the glyph from far off, though the curve,
            // inside the box around its control points, stays away from it.
            "0 280 m 100 280 l 100 330 l 40 330 l 40 295 l 10 295 l 10 330 l 0 330 l h 300 0 m 300 100000 400 -100000 400 0 c h W n",
            // A triangle that holds the glyph, turned by `cm` (a reflection) so that it misses it
            // while its box still holds it.
            "-1 0 0 1 400 0 cm 0 280 m 400 280 l 0 400 l h W n -1 0 0 1 400 0 cm",
            // That triangle, narrowed by a larger one around the glyph: the clip is where both are.
            "0 280 m 400 280 l 400 400 l h W n 0 0 m 400 0 l 0 400 l h W n",
            // A curve drawn by `v` (its first control point at the current point) and by `y` (its
            // second at its end), each passing below the glyph; with both control points at the
            // given one, it would pass above.
            "0 220 m 100 220 l 0 370 0 220 v h W n",
            "0 220 m 100 220 l 0 370 0 220 y h W n",
        ]
        .map(str::to_string)
        .into_iter()
        // The pentagram by the even-odd rule, and a circle whose box holds the glyph.
        .chain([star("W*"), circle((63.0, 342.0), 50.0)]);
        for clip in hidden {
            let mut doc = page(&cite(&clip), &[&link(&over(0, 3))]);
            autotag_text(&mut doc).unwrap();
            assert_eq!(links(&doc), [(vec!["[5]".to_string()], "P".to_string())], "{clip}");
            assert_eq!(described(&doc).as_deref(), Some("[5]"), "{clip}");
            passes(&doc);
        }
        // A clip around the glyph; a path that isn't a clip; a clip already restored; a triangle
        // around the glyph that is flat only in user space, its last point moved by `cm`.
        // Also: a clip of an area and a flat subpath; a path ended without `W` after a clip; a
        // repeated first point; an `re` with too few operands, which the renderer skips.
        let around = [
            "10 290 100 30 re W n",
            "0 0 0 0 re n",
            "q 0 0 0 0 re W n Q",
            "0 0 m 400 0 l 0 400 l W n",
            "10 290 100 30 re 0 0 0 0 re W n",
            "10 290 100 30 re W n 0 0 0 0 re n",
            "0 0 m 0 0 l 400 0 l 0 400 l W n",
            "0 0 m 400 0 l 1 2 3 re 0 400 l W n",
        ];
        let moved = "0 0 m 400 0 l 1 0 0 1 0 400 cm 0 0 l h W n 1 0 0 1 0 -400 cm";
        // A rectangle skewed by `cm` that still holds the glyph; nested rectangles around it; the
        // concave clip with its notch away from the glyph; the triangle before `cm` turns it; the
        // pentagram by the nonzero rule; and a circle around the glyph, off its centre so that
        // the glyph is outside the circle's four corners (a curve taken as a line would miss it).
        let skewed = "1 0 1 1 0 0 cm -300 290 50 30 re W n 1 0 -1 1 0 0 cm";
        let nested = "0 0 400 400 re W n 10 290 100 30 re W* n";
        let concave = "0 280 m 100 280 l 100 330 l 80 330 l 80 295 l 50 295 l 50 330 l 0 330 l h W n";
        let unturned = "0 280 m 400 280 l 0 400 l h W n";
        // A wide arch whose top passes 6 above the glyph, where its flattened lines pass 6 below:
        // a glyph within the lines' slack is taken as inside.
        let arch = "1 0 0 1 -1790.4766 -2680.0619 cm 0 0 m 0 4000 4000 4000 4000 0 c h W n 1 0 0 1 1790.4766 2680.0619 cm";
        // A rectangle turned and reflected (wound the other way) around the glyph, and a turned
        // one too large to measure.
        let reflected = "0.6 0.8 0.8 -0.6 0 0 cm 240 -180 30 30 re W n 0.6 0.8 0.8 -0.6 0 0 cm";
        let big = format!("1{}", "0".repeat(160));
        let huge = format!("0.6 0.8 -0.8 0.6 0 0 cm -{big} -{big} 2{big} 2{big} re W n 0.6 -0.8 0.8 0.6 0 0 cm");
        let around = around
            .into_iter()
            .map(str::to_string)
            .chain([moved, skewed, nested, concave, unturned, arch, reflected].map(str::to_string))
            .chain([huge, star("W"), circle((11.0, 290.0), 20.0)]);
        for content in around.map(|c| cite(&c)) {
            let mut doc = page(&content, &[&link(&over(0, 3))]);
            autotag_text(&mut doc).unwrap();
            assert_eq!(described(&doc).as_deref(), Some("v [5]"), "{content}");
        }
        // Past the saved states a walk follows, a `Q` could not restore the clip: the text is
        // taken as shown, and the link keeps its own.
        let deep = format!("{} {} {}", "q ".repeat(super::MAX_SAVED), cite("0 0 0 0 re W n"), "Q ".repeat(super::MAX_SAVED));
        let mut doc = page(&deep, &[&link(&over(0, 3))]);
        autotag_text(&mut doc).unwrap();
        assert!(links(&doc)[0].0.contains(&"[5]".to_string()), "{:?}", links(&doc));
        // Hidden text right after a link and before other text stays out of the link.
        let after = "BT /F1 10 Tf 20 300 Td ([5]) Tj ET q 0 0 0 0 re W n BT /F1 10 Tf 20 300 Td (v) Tj ET Q BT /F1 10 Tf 20 200 Td (Next) Tj ET";
        let mut doc = page(after, &[&link(&over(0, 3))]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["[5]".to_string()], "P".to_string())]);
        // Between two parts of one link it is an artifact: the Link holds only the link's text,
        // in one piece, the page shows the same, and the tree reads without it.
        for clip in ["0 0 0 0 re W n", "0 0 10 10 re W n"] {
            let content = format!(
                "BT /F1 10 Tf 20 300 Td (See the gu) Tj ET q {clip} BT /F1 10 Tf 300 100 Td (hidden) Tj ET Q BT /F1 10 Tf 80 300 Td (ide here) Tj ET"
            );
            let mut doc = page(&content, &[&link(&over(8, 13))]);
            let before = shown(&doc);
            autotag_text(&mut doc).unwrap();
            assert_eq!(links(&doc).iter().map(|l| l.0.concat()).collect::<Vec<_>>(), ["guide"], "{clip}");
            assert_eq!(reading(&doc), "See the guide here", "{clip}");
            assert_eq!(shown(&doc), before, "{clip}");
            assert_eq!(described(&doc).as_deref(), Some("guide"), "{clip}");
            passes(&doc);
        }
        // So it is after a text object of spaces that leads into the link.
        let leading = "BT /F1 10 Tf 300 100 Td ( ) Tj ET q 0 0 0 0 re W n BT /F1 10 Tf 300 50 Td (x) Tj ET Q BT /F1 10 Tf 20 300 Td (See) Tj ET";
        let mut doc = page(leading, &[&link(&over(0, 3))]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc).iter().map(|l| l.0.concat().trim().to_string()).collect::<Vec<_>>(), ["See"]);
        assert!(!reading(&doc).contains('x'));
        passes(&doc);
        // Hidden text and a visible space in one operator between two parts of a link: the space
        // joins the link and the hidden text is an artifact, as when they are two operators.
        for between in ["(x ) Tj", "(x) Tj ( ) Tj"] {
            let content = format!(
                "BT /F1 10 Tf 20 300 Td (See the gu) Tj ET q 305 0 100 400 re W n BT /F1 10 Tf 300 100 Td {between} ET Q BT /F1 10 Tf 80 300 Td (ide here) Tj ET"
            );
            let mut doc = page(&content, &[&link(&over(8, 13))]);
            autotag_text(&mut doc).unwrap();
            assert_eq!(links(&doc).iter().map(|l| l.0.concat().replace(' ', "")).collect::<Vec<_>>(), ["guide"], "{between}");
            passes(&doc);
        }
        // Blank text with nothing readable after it keeps the owners it was placed with.
        let mut doc = page("BT /F1 10 Tf 300 100 Td ( ) Tj ET BT /F1 10 Tf 20 300 Td ( ) Tj ET", &[&link(&over(0, 1))]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec![" ".to_string()], "P".to_string())]);
        // A clip's edge splits no text-showing operator that is in no link.
        let shows = |doc: &Document| {
            let page = &pdfcraft_model::pages(doc)[0];
            let content = doc.resolve(page.dict.get(b"Contents").unwrap());
            let Object::Stream(content) = &*content else { panic!("page content") };
            pdfcraft_content::parse(&content.decoded_strict().unwrap()).ops.iter().filter(|o| o.op == b"Tj").count()
        };
        let edge = "q 0 0 23 400 re W n BT /F1 10 Tf 20 300 Td (ab) Tj ET Q BT /F1 10 Tf 20 200 Td (Go) Tj ET";
        let mut doc = page(edge, &[&link("18 196 40 212")]);
        let before = shows(&doc);
        autotag_text(&mut doc).unwrap();
        assert_eq!(shows(&doc), before);
        passes(&doc);
    }

    /// A link's text that runs on into a later text object sits in the first object's element: a
    /// link from a paragraph into a heading, or from a heading into a paragraph, is refused rather
    /// than reading the heading as a paragraph (and the headings after it a level too deep).
    #[test]
    fn a_link_from_a_paragraph_into_a_heading_is_refused_and_one_inside_a_heading_keeps_its_role() {
        // "Read the" and a body line at 10 pt, "Title" at 24 pt (x 20 to 92) and "Heading" at 18 pt.
        let content = "BT /F1 10 Tf 20 300 Td (Read the) Tj ET BT /F1 24 Tf 20 260 Td (Title) Tj ET \
                       BT /F1 18 Tf 20 200 Td (Heading) Tj ET BT /F1 10 Tf 20 150 Td (Body text of the page, long enough to set the body size.) Tj ET";
        let across = |quads: &str| format!("<< /Type /Annot /Subtype /Link /Rect [18 250 95 312] /QuadPoints [{quads}] >>");
        // "the" and "Title"; then "Title" and the end of the line before it, the other way round.
        let into_heading = across("49 312 69 312 49 296 69 296 18 255 95 255 18 290 95 290");
        refused(page(content, &[&into_heading]), "is in text tagged P and text tagged H1");
        let content_up = "BT /F1 24 Tf 20 300 Td (Title) Tj ET BT /F1 10 Tf 20 260 Td (Read the) Tj ET \
                          BT /F1 18 Tf 20 200 Td (Heading) Tj ET BT /F1 10 Tf 20 150 Td (Body text of the page, long enough to set the body size.) Tj ET";
        let into_paragraph = across("18 295 95 295 18 330 95 330 20 256 50 256 20 272 50 272");
        refused(page(content_up, &[&into_paragraph.replace("250 95 312", "250 95 330")]), "is in text tagged H1 and text tagged P");
        // A heading drawn in two text objects with a text object of spaces between them (a P,
        // with no text of its own), under one link: the space has no role to keep.
        let split = "BT /F1 24 Tf 20 300 Td (Ti) Tj ET BT /F1 10 Tf 300 100 Td ( ) Tj ET BT /F1 24 Tf 48.8 300 Td (tle) Tj ET \
                     BT /F1 10 Tf 20 150 Td (Body text of the page, long enough to set the body size.) Tj ET";
        let mut doc = page(split, &[&link("18 295 100 330")]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc).iter().map(|l| l.1.as_str()).collect::<Vec<_>>(), ["H1"]);
        // A text object of spaces before a linked heading, which joins the link: the Link sits in
        // the heading, and the heading keeps its role.
        let leading = "BT /F1 10 Tf 300 100 Td ( ) Tj ET BT /F1 24 Tf 20 300 Td (Title) Tj ET \
                       BT /F1 10 Tf 20 150 Td (Body text of the page, long enough to set the body size.) Tj ET";
        let mut doc = page(leading, &[&link("18 295 100 330")]);
        let shown = String::from_utf8(shown(&doc)).unwrap();
        autotag_text(&mut doc).unwrap();
        let kinds: Vec<String> = tree(&doc).into_iter().map(|t| t.0).collect();
        assert_eq!(kinds, ["Document", "H1", "Link", "P"]);
        assert_eq!(links(&doc).iter().map(|l| (l.0.concat(), l.1.as_str())).collect::<Vec<_>>(), [(" Title".to_string(), "H1")]);
        assert_eq!(reading(&doc), shown);
        passes(&doc);
        // A link over the heading alone: the heading keeps its role, and holds the link.
        let mut doc = page(content, &[&across("18 255 95 255 18 290 95 290")]);
        autotag_text(&mut doc).unwrap();
        let kinds: Vec<String> = tree(&doc).into_iter().map(|t| t.0).collect();
        assert_eq!(kinds, ["Document", "P", "H1", "Link", "H2", "P"]);
        assert_eq!(links(&doc), [(vec!["Title".to_string()], "H1".to_string())]);
        passes(&doc);
    }

    /// The link areas a document may hold are counted before any is built: past them the
    /// document is refused, while a link with more quadrilaterals than the viewer reads is still
    /// its `/Rect`.
    #[test]
    fn the_documents_link_quadrilaterals_are_bounded_and_a_link_with_too_many_is_its_rectangle() {
        use pdfcraft_render::MAX_LINK_QUADS;
        let quads = |n: usize| "20 296 38 296 38 312 20 312 ".repeat(n);
        let many = |n: usize| format!("<< /Type /Annot /Subtype /Link /Rect [{}] /QuadPoints [{}] >>", over(0, 18), quads(n));
        // Over two pages: the allowance is the document's, not each page's.
        // The first `linked` objects are the links, listed half on each page.
        let two_pages = |annots: &[String], linked: usize| {
            let half = linked / 2;
            let refs = |r: std::ops::Range<usize>| r.map(|i| format!("{} 0 R", i + 7)).collect::<Vec<_>>().join(" ");
            let page = |annots: String| {
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> /Annots [{annots}] >>"
                )
            };
            let mut bodies = vec![
                "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
                "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
                page(refs(0..half)),
                page(refs(half..linked)),
                stream(LINE),
                COURIER.into(),
            ];
            bodies.extend(annots.iter().cloned());
            pdf(&bodies)
        };
        let annots: Vec<String> = (0..=super::MAX_LINK_AREAS / MAX_LINK_QUADS).map(|_| many(MAX_LINK_QUADS)).collect();
        refused(two_pages(&annots, annots.len()), "too many quadrilaterals");
        // Many links sharing one array that isn't used (a corner outside /Rect), so each is its
        // /Rect: each still reads the array.
        let readers = super::MAX_QUAD_VALUES / (8 * MAX_LINK_QUADS) + 1;
        let mut annots: Vec<String> =
            (0..readers).map(|_| format!("<< /Type /Annot /Subtype /Link /Rect [{}] /QuadPoints {} 0 R >>", over(0, 3), readers + 7)).collect();
        annots.push(format!("[{}]", "0 0 9 0 9 9 0 9 ".repeat(MAX_LINK_QUADS)));
        refused(two_pages(&annots, readers), "too many quadrilaterals");
        let mut doc = page(LINE, &[&many(MAX_LINK_QUADS + 1)]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["See the guide here".to_string()], "P".to_string())]);
    }

    /// A glyph belongs to a quadrilateral only when its centre is inside it, not just inside the
    /// box around it; and a quadrilateral with a corner outside `/Rect` gives way to `/Rect`
    /// (Table 176).
    #[test]
    fn a_link_area_is_its_quadrilaterals_only_when_they_lie_within_its_rectangle() {
        // A diamond whose lowest corner touches the line under "i": the box around it spans
        // "guide". Glyph centres are about 2.4 above the baseline.
        let diamond = "<< /Type /Annot /Subtype /Link /Rect [71 300 95 324] /QuadPoints [83 300 95 312 83 324 71 312] >>";
        let mut doc = page(LINE, &[diamond]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["i".to_string()], "P".to_string())]);
        // A quadrilateral over "See" with a corner left of the rectangle, which spans the line.
        let outside = format!("<< /Type /Annot /Subtype /Link /Rect [{}] /QuadPoints [10 312 38 312 10 296 38 296] >>", over(0, 18));
        let mut doc = page(LINE, &[&outside]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["See the guide here".to_string()], "P".to_string())]);
        passes(&doc);
        // A corner outside by no more than rounding still counts as inside, as in the viewer.
        let rounded = format!("<< /Type /Annot /Subtype /Link /Rect [{}] /QuadPoints [20 312.005 38 312.005 20 296 38 296] >>", over(0, 18));
        let mut doc = page(LINE, &[&rounded]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["See".to_string()], "P".to_string())]);
        // A quadrilateral thinner than that encloses nothing: the link is its rectangle.
        let thin = format!("<< /Type /Annot /Subtype /Link /Rect [{}] /QuadPoints [20 300 38 300 20 300.007 38 300.007] >>", over(0, 18));
        let mut doc = page(LINE, &[&thin]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["See the guide here".to_string()], "P".to_string())]);
        // Thick enough by the viewer's rule (one of the triangles its corners make is), though its
        // hull is thinner than that: the link is that sliver, over no glyph's centre.
        let sliver = format!("<< /Type /Annot /Subtype /Link /Rect [{}] /QuadPoints [20 300 21 300 20.001 300.014 20 300.014] >>", over(0, 18));
        let mut doc = page(LINE, &[&sliver]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(Vec::<String>::new(), "Document".to_string())]);
    }

    /// Past the document's clip work, a clip path counts by the box around it, as it did before
    /// its shape was followed: the text is still placed, not refused. A clip that misses the
    /// glyph, with its box around it, then shows it.
    #[test]
    fn past_the_documents_clip_work_a_clip_path_counts_by_its_box() {
        let described = |clip: &str, work: usize| {
            let content = format!("q {clip} BT /F1 10 Tf 20 300 Td (v) Tj ET Q BT /F1 10 Tf 20 300 Td ([5]) Tj ET");
            let doc = page(&content, &[&link(&over(0, 3))]);
            let page = &pdfcraft_model::pages(&doc)[0];
            let links = super::page_links(&doc, &page.dict, 0, &mut super::LinkBudget::default()).unwrap();
            let stream = doc.resolve(page.dict.get(b"Contents").unwrap());
            let Object::Stream(stream) = &*stream else { panic!("page content") };
            let ops = pdfcraft_content::parse(&stream.decoded_strict().unwrap()).ops;
            let resources = doc.resolve(page.dict.get(b"Resources").unwrap());
            let fonts = doc.resolve(resources.as_dict().unwrap().get(b"Font").unwrap());
            let mut allowance = super::Allowance { clip: work, ..Default::default() };
            let mut cache = crate::autotag_roles::Fonts::default();
            let placed = super::place(&doc, &mut cache, fonts.as_dict().unwrap(), &pdfcraft_cos::Dict::default(), ops, &links, &mut allowance);
            placed.expect("placed").descriptions.remove(0)
        };
        let triangle = "0 280 m 400 280 l 400 400 l h W n";
        assert_eq!(described(triangle, super::MAX_CLIP_WORK).as_deref(), Some("[5]"));
        // Enough to keep the triangle's three sides, but not to test the glyph against them.
        assert_eq!(described(triangle, 3).as_deref(), Some("v [5]"));
        // A curve costs its lines: a clip of two lines and a curve that misses the glyph, with
        // work for it as three lines, counts by its box.
        assert_eq!(described("0 220 m 100 220 l 0 370 0 220 v h W n", 8).as_deref(), Some("v [5]"));
        // The triangle narrowed by a circle around the glyph, with work to keep both and to test
        // the triangle but not the circle: once the latest clip can't be paid for, the ones it
        // narrows count by their boxes too, so the work spent on a glyph is bounded.
        let both = format!("{triangle} {}", circle((23.0, 302.0), 30.0));
        assert_eq!(described(&both, 3 + 65 + 3).as_deref(), Some("v [5]"));
    }

    /// A long chain of clip paths (two rectangles each, so not one rectangle) is freed one at a
    /// time, on a small stack.
    #[test]
    fn many_nested_clip_paths_are_freed_without_overflowing_the_stack() {
        let clips = "0 0 400 400 re 0 0 400 400 re W n ".repeat(24_000);
        let content = format!("q {clips} BT /F1 10 Tf 20 300 Td (v) Tj ET Q BT /F1 10 Tf 20 300 Td ([5]) Tj ET");
        let tagged = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut doc = page(&content, &[&link(&over(0, 3))]);
                autotag_text(&mut doc).is_ok()
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(tagged);
    }

    /// `gs` with `/Font` sets the font and size as `Tf` does (Table 57).
    #[test]
    fn a_font_set_by_a_graphics_state_places_the_text_after_it() {
        // 10 pt from Tf, then 20 pt Courier from /GS1: "guide" at 20 pt spans x = 68 to 128.
        let content = "BT /F1 10 Tf 20 300 Td (See) Tj ET /GS1 gs BT 20 270 Td (the guide here) Tj ET";
        let resources = "/ExtGState << /GS1 << /Type /ExtGState /Font [5 0 R 20] >> >>";
        let mut doc = page_res(content, &[&link("66 266 130 300")], "", COURIER, resources);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["guide".to_string()], "P".to_string())]);
        passes(&doc);
    }

    /// Placing glyphs against links reads the page's fonts; that must not use up the font
    /// allowance the roles are worked out from.
    #[test]
    fn a_link_does_not_change_the_role_of_any_text_on_its_page() {
        // Every text object in a font of its own, written in the page's resources.
        let fonts = crate::autotag_roles::MAX_FONTS;
        let names: String = (0..fonts).map(|i| format!("/F{i} << /Type /Font /Subtype /Type1 /BaseFont /Courier >> ")).collect();
        let mut content = String::from("BT /F0 24 Tf 20 380 Td (Course title) Tj ET");
        for i in 1..fonts {
            content.push_str(&format!(" BT /F{i} 10 Tf 20 {} Td (Body text of the course) Tj ET", 360 - i));
        }
        let roles = |annots: &str| {
            let mut doc = pdf(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
                format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Contents 4 0 R /Resources << /Font << {names}>> >> {annots} >>"),
                stream(&content),
                link("20 376 92 400"),
            ]);
            autotag_text(&mut doc).unwrap();
            tags(&doc).into_iter().map(|t| t.kind).filter(|k| k != "Link").collect::<Vec<_>>()
        };
        let without = roles("");
        assert!(without.get(1).is_some_and(|k| k.starts_with('H')), "{without:?}");
        assert_eq!(roles("/Annots [5 0 R]"), without);
    }

    /// Which fonts the document's allowance measures doesn't depend on where the links are: a
    /// vertical font is measured in its turn, though its glyphs are never placed.
    #[test]
    fn a_link_does_not_change_which_fonts_are_measured_when_a_vertical_font_comes_first() {
        let fonts = crate::autotag_roles::MAX_FONTS;
        let cid = "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /V /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>";
        let mut names = format!("/F0 << /Type /Font /Subtype /Type0 /BaseFont /V /Encoding /Identity-V /DescendantFonts [{cid}] >> ");
        names.extend((1..=fonts).map(|i| format!("/F{i} << /Type /Font /Subtype /Type1 /BaseFont /Courier >> ")));
        // The vertical font first, then a font of its own for each body line, and the title last.
        let mut content = String::from("BT /F0 10 Tf 380 380 Td <0001> Tj ET");
        for i in 1..fonts {
            content.push_str(&format!(" BT /F{i} 10 Tf 20 {} Td (Body text of the course) Tj ET", 360 - i));
        }
        content.push_str(&format!(" BT /F{fonts} 24 Tf 20 380 Td (Course title) Tj ET"));
        let roles = |annots: &str| {
            let mut doc = pdf(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
                format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Contents 4 0 R /Resources << /Font << {names}>> >> {annots} >>"),
                stream(&content),
                link("18 376 200 400"),
            ]);
            autotag_text(&mut doc).unwrap();
            tags(&doc).into_iter().map(|t| t.kind).filter(|k| k != "Link").collect::<Vec<_>>()
        };
        assert_eq!(roles("/Annots [5 0 R]"), roles(""));
    }

    /// Text whose font comes only from a graphics state's `/Font` reads back in that font: with no
    /// `Tf` at all, and after a `Tf` that set another one.
    #[test]
    fn text_in_a_font_set_by_a_graphics_state_reads_back_in_that_font() {
        // /GS1 sets Courier that shows code 65 ("A") as "B"; /F1 is plain Courier.
        let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding << /Type /Encoding /Differences [65 /B] >> >>";
        let resources = format!("/ExtGState << /GS1 << /Type /ExtGState /Font [{font} 10] >> >>");
        // Glyphs land under the link, and the TJ gap moves the text, only at the size /GS1 sets.
        for content in ["/GS1 gs BT 20 300 Td [(Go) -600 (AAA)] TJ ET", "BT /F1 1 Tf ET /GS1 gs BT 20 300 Td [(Go) -600 (AAA)] TJ ET"] {
            let mut doc = page_res(content, &[&link(&over(0, 2))], "", COURIER, &resources);
            autotag_text(&mut doc).unwrap();
            assert_eq!(links(&doc), [(vec!["Go".to_string()], "P".to_string())], "{content}");
            let mut doc = page_res(content, &[&link("0 0 400 400")], "", COURIER, &resources);
            autotag_text(&mut doc).unwrap();
            assert_eq!(links(&doc), [(vec!["Go BBB".to_string()], "P".to_string())], "{content}");
        }
    }

    #[test]
    fn link_areas_and_text_are_matched_in_default_user_space_whatever_the_page_rotation_or_text_matrix() {
        // Text turned a quarter turn by its matrix, on a page shown turned: the link's rectangle is
        // where the glyphs are in default user space.
        let content = "BT /F1 10 Tf 0 1 -1 0 300 20 Tm (Read the guide) Tj ET BT /F1 10 Tf 20 300 Td (elsewhere) Tj ET";
        let mut doc = page_with(content, &[&link("290 18 310 110")], "/Rotate 90");
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["Read the guide".to_string()], "P".to_string())]);
        passes(&doc);
    }

    #[test]
    fn kerning_and_line_operators_are_followed_and_split_text_shows_the_same_bytes() {
        // `"` sets word and character spacing: after "a " the next glyph starts 6 + 1 + 6 + 1 + 2 = 16
        // to the right.
        let content = "BT /F1 10 Tf 12 TL 20 312 Td [(See) -200 (the guide) 100 (here)] TJ (and more guide text) ' 2 1 (a guide) \" ET";
        let mut doc = page(content, &[&link("63 308 95 324"), &link(&over(9, 14)), &link("35 284 70 300")]);
        autotag_text(&mut doc).unwrap();
        let found = links(&doc);
        assert_eq!(found.iter().map(|f| f.0.clone()).collect::<Vec<_>>(), [["guide"], ["guide"], ["guide"]], "{found:?}");
        assert_eq!(shown(&doc), b"Seethe guidehereand more guide texta guide");
        passes(&doc);
    }

    #[test]
    fn a_link_across_two_lines_is_one_tag_in_the_first_line() {
        let content = "BT /F1 10 Tf 20 300 Td (Read the) Tj ET BT /F1 10 Tf 20 288 Td (guide now) Tj ET";
        let quads = "QuadPoints [49 312 69 312 49 296 69 296 20 300 50 300 20 284 50 284]";
        let annot = format!("<< /Type /Annot /Subtype /Link /Rect [20 284 69 312] /{quads} /A << /S /URI /URI (https://example.org/) >> >>");
        let mut doc = page(content, &[&annot]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["the".to_string(), "guide".to_string()], "P".to_string())]);
        assert_eq!(annot_dict(&doc, 7).get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text()).as_deref(), Some("the guide"));
        passes(&doc);
    }

    /// Operators that show no glyph still sit inside marked content once a text object is split.
    #[test]
    fn shows_without_glyphs_stay_in_marked_content_around_a_link() {
        let content = "BT /F1 10 Tf 20 300 Td [-100] TJ (See) Tj () Tj ( the guide) Tj [-100] TJ ET";
        let mut doc = page(content, &[&link(&over(0, 3))]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(links(&doc), [(vec!["See".to_string()], "P".to_string())]);
        passes(&doc);
    }

    /// A second line that is all link text adds to the link and leaves no empty paragraph behind.
    #[test]
    fn a_line_wholly_inside_a_link_that_started_earlier_adds_no_empty_tag() {
        let content = "BT /F1 10 Tf 20 300 Td (Read the) Tj ET BT /F1 10 Tf 20 288 Td (guide) Tj ET";
        let quads = "QuadPoints [49 312 69 312 49 296 69 296 18 300 52 300 18 284 52 284]";
        let annot = format!("<< /Type /Annot /Subtype /Link /Rect [18 284 69 312] /{quads} >>");
        let mut doc = page(content, &[&annot]);
        let result = autotag_text(&mut doc).unwrap();
        assert_eq!(result.paragraphs, 1);
        let kinds: Vec<String> = tree(&doc).into_iter().map(|t| t.0).collect();
        assert_eq!(kinds, ["Document", "P", "Link"]);
        assert_eq!(links(&doc), [(vec!["the".to_string(), "guide".to_string()], "P".to_string())]);
        passes(&doc);
    }

    #[test]
    fn a_link_keeps_its_own_description() {
        let annot = "<< /Type /Annot /Subtype /Link /Rect [18 296 130 312] /Contents (Course guide) /A << /S /URI /URI (https://example.org/) >> >>";
        let mut doc = page(LINE, &[annot]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(annot_dict(&doc, 7).get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text()).as_deref(), Some("Course guide"));
        // A blank one describes nothing, and is filled like a missing one.
        let mut doc = page(LINE, &[&annot.replace("(Course guide)", "( )")]);
        autotag_text(&mut doc).unwrap();
        assert_eq!(annot_dict(&doc, 7).get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text()).as_deref(), Some("See the guide here"));
    }

    /// Words a writer spaces by moving (a `TJ` gap, `Td`, a new line) rather than with a space
    /// glyph are still separate words in the description; kerning doesn't split a word.
    #[test]
    fn a_description_has_a_space_wherever_its_text_jumps() {
        let cases = [
            ("BT /F1 10 Tf 20 300 Td [(Click)-600(here)] TJ ET", "Click here"),
            ("BT /F1 10 Tf 20 300 Td (Click) Tj 36 0 Td (here) Tj ET", "Click here"),
            ("BT /F1 10 Tf 12 TL 20 300 Td (Read the) Tj T* (guide now) Tj ET", "Read the guide now"),
            ("BT /F1 10 Tf 20 300 Td [(Cl)-20(ick)] TJ ET", "Click"),
            // The jump is in units of the font size as the text is scaled, and a rise isn't one.
            ("BT /F1 10 Tf 50 Tz 20 300 Td [(Click)-250(here)] TJ ET", "Click here"),
            ("q 2 0 0 1 0 0 cm BT /F1 10 Tf 10 300 Td [(Cl)-100(ick)] TJ ET Q", "Click"),
            ("q 1 0 0 0.5 0 0 cm BT /F1 20 Tf 20 600 Td [(Cl)-100(ick)] TJ ET Q", "Click"),
            ("BT /F1 10 Tf 20 300 Td (H) Tj -2 Ts (2) Tj 0 Ts (O) Tj ET", "H2O"),
        ];
        // Whitespace counts toward the length limit only as the description writes it.
        let spaces = " ".repeat(300);
        let spaced = [(format!("BT /F1 1 Tf 20 300 Td ({spaces}Go) Tj ET"), "Go"), (format!("BT /F1 1 Tf 20 300 Td (Go{spaces}on) Tj ET"), "Go on")];
        for (content, description) in cases.map(|(c, d)| (c.to_string(), d)).into_iter().chain(spaced) {
            let mut doc = page(&content, &[&link("0 0 400 400")]);
            autotag_text(&mut doc).unwrap();
            assert_eq!(annot_dict(&doc, 7).get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text()).as_deref(), Some(description));
        }
    }

    /// A two-byte font's string splits between codes, never inside one.
    #[test]
    fn a_two_byte_string_splits_between_its_codes() {
        // Identity-H with every glyph 1 em wide: at 10 pt from x = 20, glyph i spans 20 + 10i to
        // 30 + 10i; the link covers glyphs 1 and 2.
        let cid = "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /H /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>";
        let font = format!("<< /Type /Font /Subtype /Type0 /BaseFont /H /Encoding /Identity-H /DescendantFonts [{cid}] >>");
        let mut doc = page_in("BT /F1 10 Tf 20 300 Td <0001000200030004> Tj ET", &[&link("28 290 52 320")], "", &font);
        autotag_text(&mut doc).unwrap();
        assert_eq!(shown(&doc), [0, 1, 0, 2, 0, 3, 0, 4]);
        let page = &pdfcraft_model::pages(&doc)[0];
        let content = doc.resolve(page.dict.get(b"Contents").unwrap());
        let Object::Stream(content) = &*content else { panic!("page content") };
        let strings: Vec<_> = pdfcraft_content::parse(&content.decoded_strict().unwrap())
            .ops
            .into_iter()
            .filter(|op| op.op == b"Tj")
            .filter_map(|op| op.operands.last().and_then(|o| o.as_string().map(|s| s.bytes.clone())))
            .collect();
        assert_eq!(strings, [vec![0, 1], vec![0, 2, 0, 3], vec![0, 4]]);
        let element = owner_of(&doc, ObjRef::new(7, 0)).unwrap();
        assert_eq!(tag_content(&doc, element).unwrap().len(), 1);
        passes(&doc);
    }

    /// Glyphs with no Unicode text still show text: a link over them owns them, and a link in such
    /// a font takes no text from the text objects around it.
    #[test]
    fn glyphs_with_no_text_stay_with_their_own_link_or_paragraph() {
        let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding << /Type /Encoding /Differences [65 /uniXXXX /uniYYYY] >> >>";
        let mut doc = page_in("BT /F1 10 Tf 20 300 Td (See AB here) Tj ET", &[&link(&over(4, 6))], "", font);
        autotag_text(&mut doc).unwrap();
        let element = owner_of(&doc, ObjRef::new(7, 0)).unwrap();
        assert_eq!(tag_content(&doc, element).unwrap().len(), 1);
        passes(&doc);
        // Every glyph without text: the link over "AB" holds only that.
        let content = "BT /F1 10 Tf 20 300 Td (AABB) Tj ET BT /F1 24 Tf 20 250 Td (BBAA) Tj ET BT /F1 10 Tf 20 200 Td (ABAB) Tj ET";
        let mut doc = page_in(content, &[&link(&over(1, 3))], "", font);
        autotag_text(&mut doc).unwrap();
        let element = owner_of(&doc, ObjRef::new(7, 0)).unwrap();
        assert_eq!(tag_content(&doc, element).unwrap().len(), 1);
        assert_eq!(tags(&doc).iter().filter(|t| t.kind != "Link").count(), 4, "Document and a tag for each text object");
        passes(&doc);
        // A control code after a link's text, with no more of the link after it, stays the
        // paragraph's.
        let mut doc = page("BT /F1 10 Tf 20 300 Td (U1) Tj 2314 2304 Td (\n) ' ET", &[&link(&over(0, 2))]);
        autotag_text(&mut doc).unwrap();
        let p = tags(&doc).into_iter().find(|t| t.kind == "P").unwrap();
        assert_eq!(tag_content(&doc, p.object).unwrap().len(), 1);
        // A text object of such glyphs between two parts of one link is text, and interrupts it.
        let quads = "QuadPoints [26 296 38 296 38 312 26 312 26 196 38 196 38 212 26 212]";
        refused(
            page_in(content, &[&format!("<< /Type /Annot /Subtype /Link /Rect [18 190 60 312] /{quads} >>")], "", font),
            "interrupted by text outside it",
        );
    }

    /// A font that isn't there, and one that writes vertically, whose glyphs would land under the
    /// link if they advanced across the line.
    #[test]
    fn text_in_a_font_that_cannot_be_measured_stays_with_its_paragraph() {
        let mut missing = page("BT /F9 10 Tf 20 300 Td (See the guide here) Tj ET", &[&link(&over(0, 18))]);
        let cid = "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /V /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>";
        let font = format!("<< /Type /Font /Subtype /Type0 /BaseFont /V /Encoding /Identity-V /DescendantFonts [{cid}] >>");
        let mut vertical = page_in("BT /F1 10 Tf 20 300 Td <0001000200030004> Tj ET", &[&link("18 280 80 312")], "", &font);
        for doc in [&mut missing, &mut vertical] {
            autotag_text(doc).unwrap();
            assert_eq!(links(doc), [(Vec::<String>::new(), "Document".to_string())]);
            passes(doc);
        }
    }

    #[test]
    fn other_annotations_and_links_it_cannot_tag_are_refused_without_a_change() {
        let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 60 120 80] >>";
        let tagged = "<< /Type /Annot /Subtype /Link /Rect [20 60 120 80] /StructParent 4 >>";
        let cases: [(Vec<String>, &str); 2] = [
            (vec![link(&over(0, 3)), widget.into()], "Widget annotations need separate structure tags"),
            (vec![tagged.into()], "already has a structure parent key"),
        ];
        for (annots, why) in cases {
            let annots: Vec<&str> = annots.iter().map(String::as_str).collect();
            let mut doc = page(LINE, &annots);
            let before: Vec<_> = (3..=8).map(|n| doc.get(ObjRef::new(n, 0)).as_ref().clone()).collect();
            match autotag_text(&mut doc) {
                Err(AutoTagError::Unsupported { reason, .. }) => assert!(reason.contains(why), "{reason}"),
                other => panic!("{other:?}"),
            }
            let after: Vec<_> = (3..=8).map(|n| doc.get(ObjRef::new(n, 0)).as_ref().clone()).collect();
            assert!(before == after && tags(&doc).is_empty());
        }
        // A page with only an image and a link: the image would be an artifact, leaving nothing to
        // read but the link.
        let mut doc = page("q 40 0 0 40 300 100 cm /Im1 Do Q", &[&link("300 100 340 140")]);
        assert_eq!(autotag_text(&mut doc), Err(AutoTagError::NoText));
        // One annotation listed twice, and one written inline.
        let twice = Object::Array(vec![Object::Ref(ObjRef::new(7, 0)), Object::Ref(ObjRef::new(7, 0))]);
        let inline = Object::Array(vec![Object::Dict(annot_dict(&page(LINE, &[&link("0 0 9 9")]), 7))]);
        for (annots, why) in [(twice, "listed twice"), (inline, "inside /Annots")] {
            let mut doc = page(LINE, &[&link(&over(0, 3))]);
            let at = pdfcraft_model::pages(&doc)[0].obj;
            doc.update_dict(at, |d| d.set(b"Annots".to_vec(), annots)).unwrap();
            assert!(matches!(autotag_text(&mut doc), Err(AutoTagError::Unsupported { reason, .. }) if reason.contains(why)));
        }
    }
}
