//! Link annotations for auto-tagging (ISO 32000-2 §14.8.4.7.2, Table 368; §14.7.5.3 and §14.7.5.4).
//!
//! Each link annotation on a page becomes a Link structure element that holds the annotation through
//! an object reference (OBJR) and the text drawn inside the link's area. A glyph belongs to a link
//! when the centre of its box falls inside one of the link's areas: its `/QuadPoints` boxes, clipped to
//! `/Rect`, else `/Rect`. Glyph positions and link areas are both in the page's default user space, so
//! `/Rotate` plays no part. Where areas overlap, the smallest one wins, so a link nested in another
//! keeps its own text.
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
/// `/QuadPoints` quadrilaterals read per link.
const MAX_QUADS: usize = 64;
/// Saved graphics states followed; deeper `q` nesting keeps the current state.
const MAX_SAVED: usize = 64;
/// Characters of a `/Contents` description written from a link's text or address.
const MAX_CONTENTS: usize = 200;
/// Glyph-in-area tests in one document: each glyph is tested against every link area on its page.
pub(crate) const MAX_AREA_TESTS: usize = 50_000_000;

/// A link annotation to tag.
pub(crate) struct Link {
    pub annot: ObjRef,
    /// Boxes in default user space, `[x0, y0, x1, y1]` with `x0 < x1` and `y0 < y1`. Empty when the
    /// annotation has no usable `/Rect`: it is still tagged, but owns no text.
    areas: Vec<[f64; 4]>,
    /// The annotation has a non-empty `/Contents` of its own.
    described: bool,
    /// A `/URI` action's address.
    uri: Option<String>,
}

/// The link annotations of a page, in `/Annots` order. Any other annotation is refused, as is a link
/// this can't tag on its own: one written inline (an OBJR needs an object to refer to), one listed
/// twice (`seen` spans the document) or one that already has a parent tree key.
pub(crate) fn page_links(doc: &Document, page: &Dict, index: usize, seen: &mut HashSet<ObjRef>) -> Result<Vec<Link>, AutoTagError> {
    let Some(annots) = page.get(b"Annots") else { return Ok(Vec::new()) };
    let annots = doc.resolve(annots);
    let Object::Array(items) = &*annots else { return Err(unsupported(index, "the page's annotations are not an array")) };
    let mut links = Vec::with_capacity(items.len().min(MAX_LINKS));
    for item in items {
        let Object::Ref(annot) = item else {
            return Err(unsupported(index, "an annotation is written inside /Annots; a Link tag needs it as an object of its own"));
        };
        let object = doc.get(*annot);
        let Some(dict) = object.as_dict() else { return Err(unsupported(index, "an annotation is not a dictionary")) };
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
        links.push(Link { annot: *annot, areas: areas(doc, dict), described: described(doc, dict), uri: uri(doc, dict) });
    }
    Ok(links)
}

/// An array of finite numbers, each possibly given by reference.
fn numbers(doc: &Document, value: Option<&Object>) -> Option<Vec<f64>> {
    let value = doc.resolve(value?);
    value.as_array()?.iter().map(|v| doc.resolve(v).as_f64().filter(|n| n.is_finite())).collect()
}

fn areas(doc: &Document, annot: &Dict) -> Vec<[f64; 4]> {
    let Some([a, b, c, d]) = numbers(doc, annot.get(b"Rect")).and_then(|r| <[f64; 4]>::try_from(r).ok()) else { return Vec::new() };
    let rect = [a.min(c), b.min(d), a.max(c), b.max(d)];
    if rect[0] >= rect[2] || rect[1] >= rect[3] {
        return Vec::new();
    }
    // Quadrilaterals outside the rectangle are ignored (Table 176); each is read as its bounding box.
    // More quadrilaterals than are read: the rectangle, so no part of the link loses its text.
    let quads: Vec<[f64; 4]> = numbers(doc, annot.get(b"QuadPoints"))
        .filter(|q| !q.is_empty() && q.len() % 8 == 0 && q.len() / 8 <= MAX_QUADS)
        .map(|q| {
            q.as_chunks::<8>()
                .0
                .iter()
                .take(MAX_QUADS)
                .filter_map(|p| {
                    let xs = [p[0], p[2], p[4], p[6]];
                    let ys = [p[1], p[3], p[5], p[7]];
                    let lo = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
                    let hi = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
                    let quad = [lo(xs).max(rect[0]), lo(ys).max(rect[1]), hi(xs).min(rect[2]), hi(ys).min(rect[3])];
                    (quad[0] < quad[2] && quad[1] < quad[3]).then_some(quad)
                })
                .collect()
        })
        .unwrap_or_default();
    if quads.is_empty() { vec![rect] } else { quads }
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
    /// The `/Contents` to write on each link, by index into the page's links.
    pub descriptions: Vec<Option<String>>,
}

/// Where each glyph of `ops` falls, against `links`. `resources` is the page's `/Font` resource
/// dictionary and `fonts` the document's font cache. With no link areas the operators come back as
/// they are and no glyph has an owner. `allowance` is the glyph-in-area tests the document has left
/// (from [`MAX_AREA_TESTS`]); `None` when placing this page's glyphs would take more.
pub(crate) fn place(doc: &Document, fonts: &mut Fonts, resources: &Dict, ops: Vec<Op>, links: &[Link], allowance: &mut usize) -> Option<Placed> {
    let mut texts = vec![String::new(); links.len()];
    let areas = links.iter().map(|l| l.areas.len()).fold(0usize, usize::saturating_add);
    if areas == 0 {
        let descriptions = links.iter().map(|l| l.description("")).collect();
        return Some(Placed { ops, owners: HashMap::new(), descriptions });
    }
    let mut walk = Walk {
        state: State::default(),
        saved: Vec::new(),
        unsaved: 0,
        tm: Matrix::IDENTITY,
        tlm: Matrix::IDENTITY,
        lost: false,
        tests: *allowance,
        cost: areas,
        exhausted: false,
        bounds: links.iter().flat_map(|l| l.areas.iter()).fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, a| {
            [b[0].min(a[0]), b[1].min(a[1]), b[2].max(a[2]), b[3].max(a[3])]
        }),
    };
    let mut by_name: HashMap<Vec<u8>, Option<Rc<Metrics>>> = HashMap::new();
    let mut out = Vec::with_capacity(ops.len());
    let mut owners = HashMap::new();
    // The text object each link's text was last taken from, to put a space between objects.
    let (mut object, mut last_object) = (0usize, vec![None; links.len()]);
    for op in ops {
        match op.op.as_slice() {
            b"BT" => object = object.saturating_add(1),
            b"Tf" => {
                walk.state.metrics = op.name(0).and_then(|name| {
                    by_name
                        .entry(name.to_vec())
                        .or_insert_with(|| resources.get(name).filter(|f| !vertical(doc, f)).and_then(|f| fonts.get(doc, f)))
                        .clone()
                });
            }
            _ => {}
        }
        let runs = walk.step(&op, links);
        // Checked before anything else: once spent, later operators would place nothing at all.
        if walk.exhausted {
            return None;
        }
        let Some(runs) = runs else {
            out.push(op);
            continue;
        };
        for (owner, text) in runs.iter().filter_map(|r| r.owner.map(|o| (o, &r.text))) {
            if let (Some(t), Some(last)) = (texts.get_mut(owner), last_object.get_mut(owner)) {
                if last.is_some_and(|l| l != object) {
                    t.push(' ');
                }
                // Only what the description can use is kept, however much text the link covers.
                let room = MAX_CONTENTS.saturating_sub(t.chars().count());
                t.extend(text.chars().take(room));
                *last = Some(object);
            }
        }
        if runs.len() <= 1 {
            if let Some(run) = runs.first() {
                owners.insert(out.len(), run.owner);
            }
            out.push(op);
            continue;
        }
        for (i, run) in runs.into_iter().enumerate() {
            owners.insert(out.len(), run.owner);
            out.push(split_op(&op, i == 0, run.items));
        }
    }
    *allowance = walk.tests;
    let descriptions = links.iter().zip(&texts).map(|(l, t)| l.description(t)).collect();
    Some(Placed { ops: out, owners, descriptions })
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
    text: String,
}

struct Walk {
    state: State,
    saved: Vec<State>,
    unsaved: usize,
    tm: Matrix,
    tlm: Matrix,
    /// The text matrix is unknown: a string was shown whose glyphs couldn't be measured. Positions
    /// are known again after the next line move or `Tm`.
    lost: bool,
    /// Glyph-in-area tests left in the document's allowance, and what one glyph costs: the page's
    /// number of link areas.
    tests: usize,
    cost: usize,
    /// A glyph was left unplaced because the allowance was spent.
    exhausted: bool,
    /// The box around all the page's link areas: a glyph outside it is tested against none of them,
    /// and costs nothing.
    bounds: [f64; 4],
}

impl Walk {
    /// Follows `op`; for a text-showing operator that shows at least one glyph, its glyphs grouped
    /// into runs by owner.
    fn step(&mut self, op: &Op, links: &[Link]) -> Option<Vec<Run>> {
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
            b"Tj" | b"'" | b"\"" | b"TJ" => return self.show(op, links),
            _ => {}
        }
        None
    }

    fn next_line(&mut self) {
        self.tlm = Matrix::translate(0.0, -self.state.leading).then(&self.tlm);
        (self.tm, self.lost) = (self.tlm, false);
    }

    fn show(&mut self, op: &Op, links: &[Link]) -> Option<Vec<Run>> {
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
            for (owner, bytes, text) in self.glyphs(string, links) {
                let piece = Object::String(PdfString { bytes, hex: string.hex });
                match runs.last_mut() {
                    Some(run) if run.owner == owner => {
                        run.text.push_str(&text);
                        run.items.push(piece);
                    }
                    _ => {
                        let mut items = std::mem::take(&mut pending);
                        items.push(piece);
                        runs.push(Run { owner, items, text });
                    }
                }
            }
        }
        if let Some(run) = runs.last_mut() {
            run.items.append(&mut pending);
        }
        (!runs.is_empty()).then_some(runs)
    }

    /// The glyphs of `string` grouped by owner, in order: `(owner, bytes, text)`. A string whose
    /// glyphs can't be placed is one group with no owner.
    fn glyphs(&mut self, string: &PdfString, links: &[Link]) -> Vec<(Option<usize>, Vec<u8>, String)> {
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
            return vec![(None, bytes.clone(), String::new())];
        };
        let m = Rc::clone(m);
        let mut out: Vec<(Option<usize>, Vec<u8>, String)> = Vec::new();
        let mut at = 0usize;
        for (code, len) in codes {
            let s = &self.state;
            let width = m.width(code);
            let glyph = Matrix([s.size * s.scale, 0.0, 0.0, s.size, 0.0, s.rise]).then(&self.tm).then(&s.ctm);
            let centre = glyph.apply(width / 2.0, (m.ascent + m.descent) / 2.0);
            let b = self.bounds;
            let near = b[0] <= centre.0 && centre.0 <= b[2] && b[1] <= centre.1 && centre.1 <= b[3];
            let owner = if near {
                let Some(left) = self.tests.checked_sub(self.cost) else {
                    self.exhausted = true;
                    break;
                };
                self.tests = left;
                owner(links, centre)
            } else {
                None
            };
            let piece = bytes.get(at..at.saturating_add(len)).unwrap_or_default();
            at = at.saturating_add(len);
            let text = m.text_of(code).map(|t| t.into_owned()).unwrap_or_default();
            match out.last_mut() {
                Some(last) if last.0 == owner => {
                    last.1.extend_from_slice(piece);
                    last.2.push_str(&text);
                }
                _ => out.push((owner, piece.to_vec(), text)),
            }
            let spacing = if m.is_space(code, len) { s.word_spacing } else { 0.0 };
            let advance = (width * s.size + s.char_spacing + spacing) * s.scale;
            self.tm = Matrix::translate(advance, 0.0).then(&self.tm);
        }
        out
    }
}

/// The link whose smallest area holds `point`.
fn owner(links: &[Link], (x, y): (f64, f64)) -> Option<usize> {
    if !(x.is_finite() && y.is_finite()) {
        return None;
    }
    links
        .iter()
        .enumerate()
        .flat_map(|(i, l)| l.areas.iter().map(move |a| (i, a)))
        .filter(|(_, a)| a[0] <= x && x <= a[2] && a[1] <= y && y <= a[3])
        .min_by(|(_, a), (_, b)| ((a[2] - a[0]) * (a[3] - a[1])).total_cmp(&((b[2] - b[0]) * (b[3] - b[1]))))
        .map(|(i, _)| i)
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
    fn page_with(content: &str, annots: &[&str], page: &str) -> Document {
        page_in(content, annots, page, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>")
    }

    fn page_in(content: &str, annots: &[&str], page: &str, font: &str) -> Document {
        let refs: Vec<String> = (0..annots.len()).map(|i| format!("{} 0 R", i + 7)).collect();
        let mut bodies = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> >> /Annots [{}] {page} >>",
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

    #[test]
    fn a_link_inside_another_keeps_its_own_text() {
        let mut doc = page(LINE, &[&link(&over(0, 18)), &link(&over(8, 13))]);
        autotag_text(&mut doc).unwrap();
        let found = links(&doc);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, ["See the ", " here"]);
        assert_eq!(found[1].0, ["guide"]);
        passes(&doc);
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
