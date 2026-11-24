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
/// Characters of a `/Contents` description written from a link's text or address.
const MAX_CONTENTS: usize = 200;
/// Glyph-in-area tests in one document: each glyph is tested against every link area on its page.
pub(crate) const MAX_AREA_TESTS: usize = 50_000_000;

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
    /// The text-showing operators of no link that show only glyphs without text whose codes are
    /// control codes (below 32) of a one-byte font: a control character the writer copied from
    /// its text (standard encodings have no glyph there), such as a newline shown off the page
    /// after each label. They join a link only when they lie inside its text ([`settle`]).
    pub quiet: HashSet<usize>,
    /// The `/Contents` to write on each link, by index into the page's links.
    pub descriptions: Vec<Option<String>>,
}

/// Where each glyph of `ops` falls, against `links`. `resources` and `states` are the page's `/Font`
/// and `/ExtGState` resource dictionaries and `fonts` the document's font cache. With no link areas the operators come back as
/// they are and no glyph has an owner. `allowance` is the glyph-in-area tests the document has left
/// (from [`MAX_AREA_TESTS`]); `None` when placing this page's glyphs would take more.
pub(crate) fn place(
    doc: &Document,
    fonts: &mut Fonts,
    resources: &Dict,
    states: &Dict,
    ops: Vec<Op>,
    links: &[Link],
    allowance: &mut usize,
) -> Option<Placed> {
    // Each usable area once, with the link it belongs to: what every glyph near them is tested
    // against, and what it is charged for.
    let areas: Vec<(usize, &Area)> = links.iter().enumerate().flat_map(|(i, l)| l.areas.iter().map(move |a| (i, a))).collect();
    if areas.is_empty() {
        let descriptions = links.iter().map(|l| l.description("")).collect();
        return Some(Placed { ops, owners: HashMap::new(), blank: HashSet::new(), quiet: HashSet::new(), descriptions });
    }
    let mut walk = Walk {
        state: State::default(),
        saved: Vec::new(),
        unsaved: 0,
        tm: Matrix::IDENTITY,
        tlm: Matrix::IDENTITY,
        lost: false,
        tests: *allowance,
        exhausted: false,
        bounds: areas.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, (_, a)| {
            [b[0].min(a.bounds[0]), b[1].min(a.bounds[1]), b[2].max(a.bounds[2]), b[3].max(a.bounds[3])]
        }),
        areas,
        texts: vec![Covered::default(); links.len()],
    };
    let mut out = Vec::with_capacity(ops.len());
    let (mut owners, mut blank, mut quiet) = (HashMap::new(), HashSet::new(), HashSet::new());
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
                Reads::Nothing => blank.insert(out.len()),
                Reads::Unknown => quiet.insert(out.len()),
                Reads::Text => false,
            };
            out.push(split_op(&op, i == 0, run.items));
        }
    }
    *allowance = walk.tests;
    let descriptions = links.iter().zip(&walk.texts).map(|(l, t)| l.description(&t.text)).collect();
    Some(Placed { ops: out, owners, blank, quiet, descriptions })
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
}

impl Default for State {
    fn default() -> Self {
        State { ctm: Matrix::IDENTITY, size: 0.0, char_spacing: 0.0, word_spacing: 0.0, scale: 1.0, leading: 0.0, rise: 0.0, metrics: None }
    }
}

/// Consecutive glyphs of one operator with one owner, and the operands that show them.
struct Run {
    owner: Option<usize>,
    items: Vec<Object>,
    /// What its glyphs read as, together.
    reads: Reads,
}

/// What glyphs read as, from least to most: a run reads as its most.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reads {
    /// Whitespace and control characters ([`blank`]).
    Nothing,
    /// No link's, no text, and a control code ([`Placed::quiet`]).
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
            _ => {}
        }
        None
    }

    fn next_line(&mut self) {
        self.tlm = Matrix::translate(0.0, -self.state.leading).then(&self.tlm);
        (self.tm, self.lost) = (self.tlm, false);
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
            for (owner, bytes, reads) in self.glyphs(string) {
                let piece = Object::String(PdfString { bytes, hex: string.hex });
                match runs.last_mut() {
                    Some(run) if run.owner == owner => {
                        run.items.push(piece);
                        run.reads = run.reads.max(reads);
                    }
                    _ => {
                        let mut items = std::mem::take(&mut pending);
                        items.push(piece);
                        runs.push(Run { owner, items, reads });
                    }
                }
            }
        }
        if let Some(run) = runs.last_mut() {
            run.items.append(&mut pending);
        }
        (!runs.is_empty()).then_some(runs)
    }

    /// The glyphs of `string` grouped by owner, in order: `(owner, bytes, what they read as)`. A
    /// string whose glyphs can't be placed is one group with no owner, read as text. Each glyph's
    /// text goes to every link it is in.
    fn glyphs(&mut self, string: &PdfString) -> Vec<(Option<usize>, Vec<u8>, Reads)> {
        let bytes = &string.bytes;
        if bytes.is_empty() {
            return Vec::new();
        }
        let s = &self.state;
        let usable = s.metrics.as_ref().filter(|m| !m.codespaces_dropped && !m.past_allowance);
        let codes = usable
            .map(|m| m.codes(bytes))
            .filter(|c| c.iter().map(|&(_, len)| len).sum::<usize>() == bytes.len() && c.iter().all(|&(_, len)| len > 0));
        let (Some(m), Some(codes), false) = (usable, codes, self.lost) else {
            self.lost = true;
            return vec![(None, bytes.clone(), Reads::Text)];
        };
        let m = Rc::clone(m);
        let mut out: Vec<(Option<usize>, Vec<u8>, Reads)> = Vec::new();
        let mut at = 0usize;
        for (code, len) in codes {
            let s = &self.state;
            let width = m.width(code);
            let glyph = Matrix([s.size * s.scale, 0.0, 0.0, s.size, 0.0, s.rise]).then(&self.tm).then(&s.ctm);
            let centre = glyph.apply(width / 2.0, (m.ascent + m.descent) / 2.0);
            let b = self.bounds;
            let near = b[0] <= centre.0 && centre.0 <= b[2] && b[1] <= centre.1 && centre.1 <= b[3];
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
                Some(last) if last.0 == owner => {
                    last.1.extend_from_slice(piece);
                    last.2 = last.2.max(reads);
                }
                _ => out.push((owner, piece.to_vec(), reads)),
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

/// The marked-content sequences of the text object `(first, last)` (its `BT` and `ET`): the whole
/// object, with no link, when none of its glyphs belongs to one. Otherwise one sequence per run of
/// glyph-showing operators with the same owner, which together cover everything between `BT` and
/// `ET`: an operator that shows no glyph (`() Tj`, `[-250] TJ`) is still text-showing content, and
/// must be in a sequence too.
pub(crate) fn sequences((first, last): (usize, usize), owners: &HashMap<usize, Option<usize>>) -> Vec<(usize, usize, Option<usize>)> {
    if !(first..=last).any(|at| matches!(owners.get(&at), Some(Some(_)))) {
        return vec![(first, last, None)];
    }
    let mut out: Vec<(usize, usize, Option<usize>)> = Vec::new();
    for at in first..=last {
        let Some(&owner) = owners.get(&at) else { continue };
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
/// link's text, and so does an operator of [`Placed::quiet`] control codes. `spans` are the
/// page's text objects; a text object that joins a link is keyed by its `BT` in `owners`, and
/// whatever joins a link is added to `blank`.
pub(crate) fn settle(spans: &[(usize, usize)], owners: &mut HashMap<usize, Option<usize>>, blank: &mut HashSet<usize>, quiet: &HashSet<usize>) {
    // Each glyph-showing operator, or a text object with none, in order: (key, owner, reads as
    // nothing, joins only a link it lies inside).
    let mut items: Vec<(usize, Option<usize>, bool, bool)> = Vec::new();
    for &(first, last) in spans {
        let before = items.len();
        items.extend(
            (first..=last).filter_map(|at| owners.get(&at).map(|&o| (at, o, blank.contains(&at) || quiet.contains(&at), quiet.contains(&at)))),
        );
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
        if chosen != owner {
            owners.insert(at, chosen);
            blank.insert(at);
        }
    }
}

/// The first link whose text something else interrupts, in the order the page shows its text:
/// `(link, what came between)`, where `None` is text that is no link's. A Link element holds its
/// text in one piece; text of a link after an interruption could stay in the Link only by being
/// read out of order, and could keep its order only by leaving the Link. A link inside another
/// interrupts it unless it covers the start or end of the other's text.
pub(crate) fn interrupted(spans: &[(usize, usize)], owners: &HashMap<usize, Option<usize>>) -> Option<(usize, Option<usize>)> {
    // Each link whose text has ended, and what came right after it.
    let mut ended: HashMap<usize, Option<usize>> = HashMap::new();
    let mut last: Option<Option<usize>> = None;
    for &span in spans {
        for (_, _, owner) in sequences(span, owners) {
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
