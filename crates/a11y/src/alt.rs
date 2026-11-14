//! Add alternate text: the document's figures one by one, their alternate text, and marking a
//! figure as decorative (its content becomes an artifact and its tag goes away).

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, PdfString, Stream};

/// A Figure element (after the role map).
#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub obj: ObjRef,
    /// 0-based page of its content.
    pub page: Option<usize>,
    pub alt: Option<String>,
    /// Where its content is drawn, in user space (images, forms and paths it marks).
    pub bbox: Option<[f64; 4]>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AltError {
    #[error("the document has no tags")]
    Untagged,
    #[error("object {0} is not a figure")]
    NotAFigure(u32),
    #[error("the page has more content than can be edited")]
    TooLarge,
    #[error("the figure's content may be in a damaged or unreadable content stream, or after the damage, so nothing was changed")]
    Damaged,
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// Why a structure tag cannot safely become an artifact.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ArtifactError {
    #[error("the document has no tags")]
    Untagged,
    #[error("object {0} is not a reachable indirect structure tag")]
    NotATag(u32),
    #[error("the tag has child structure elements; mark a leaf tag instead")]
    HasChildren,
    #[error("the tag has no direct marked content")]
    NoMarkedContent,
    #[error("the tag has no page")]
    NoPage,
    #[error("the tag's marked content was not found on its page")]
    ContentNotFound,
    #[error("the tag has no removable structure-tree parent")]
    NoParent,
    #[error("the parent must list the tag exactly once")]
    AmbiguousParent,
    #[error("couldn't update the parent tree: {0}")]
    ParentTree(String),
    #[error("the page has more content than can be edited")]
    TooLarge,
    #[error("the tag's marked content may be in a damaged or unreadable content stream, or after the damage, so nothing was changed")]
    Damaged,
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

pub(crate) fn tree_root(doc: &Document) -> Option<Dict> {
    let cat = doc.root().and_then(|r| doc.get(r).as_dict().cloned())?;
    cat.get(b"StructTreeRoot").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned())
}

/// The standard type of a structure type through the role map.
pub(crate) fn standard(doc: &Document, role_map: &Dict, s: &[u8]) -> Vec<u8> {
    let mut t = s.to_vec();
    for _ in 0..12 {
        if crate::structure::is_standard(&t) {
            break;
        }
        match role_map.get(&t).map(|o| doc.resolve(o)).and_then(|o| o.as_name().map(<[u8]>::to_vec)) {
            Some(n) if n != t => t = n,
            _ => break,
        }
    }
    t
}

/// An indirect structure element and its nearest indirect parent in tree order.
pub(crate) struct Element {
    pub object: ObjRef,
    pub dict: Dict,
    pub page: Option<ObjRef>,
    pub parent: Option<ObjRef>,
    pub depth: usize,
}

/// Indirect structure elements in document order, with their page and hierarchy.
pub(crate) fn elements(doc: &Document) -> Vec<Element> {
    let Some(root) = tree_root(doc) else { return Vec::new() };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(Object, Option<ObjRef>, Option<ObjRef>, usize)> =
        root.get(b"K").map(|k| vec![(k.clone(), None, None, 0)]).unwrap_or_default();
    while let Some((k, pg, parent, depth)) = stack.pop() {
        if out.len() > 1_000_000 {
            break;
        }
        match k {
            Object::Array(a) => stack.extend(a.into_iter().rev().map(|x| (x, pg, parent, depth))),
            Object::Ref(r) => {
                if !seen.insert(r) {
                    continue;
                }
                let target = doc.get(r);
                if matches!(&*target, Object::Array(_) | Object::Ref(_)) {
                    // An indirect /K array is a container, not an element or a parent.
                    stack.push(((*target).clone(), pg, parent, depth));
                    continue;
                }
                let Some(d) = target.as_dict().cloned() else { continue };
                if d.contains(b"S") {
                    let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
                    if let Some(kids) = d.get(b"K") {
                        stack.push((kids.clone(), pg, Some(r), depth.saturating_add(1)));
                    }
                    out.push(Element { object: r, dict: d, page: pg, parent, depth });
                }
            }
            Object::Dict(d) if d.contains(b"S") => {
                // Direct elements can't be addressed; their indirect children still can.
                let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
                if let Some(kids) = d.get(b"K") {
                    stack.push((kids.clone(), pg, parent, depth));
                }
            }
            _ => {}
        }
    }
    out
}

/// Marked content a structure element owns: the content stream holding it, `None` for the page's
/// own content or else the form XObject (or other stream) an MCR's `/Stm` names, and its
/// marked-content id there. A form numbers its marked content on its own (ISO 32000-2 §14.7.5.1),
/// so the same id in a form and on the page is different content belonging to different elements.
pub(crate) type ContentRef = (Option<ObjRef>, i64);

/// Stands for a `/Stm` that isn't a reference: no object has number 0, so content looked up there
/// is never found, rather than being taken for the page's.
const UNREADABLE_STM: ObjRef = ObjRef::new(0, 0);

/// The stream an MCR's `/Stm` names, as written; `None` without one.
pub(crate) fn mcr_stm(m: &Dict) -> Option<ObjRef> {
    match m.get(b"Stm") {
        None | Some(Object::Null) => None,
        Some(o) => Some(o.as_ref().unwrap_or(UNREADABLE_STM)),
    }
}

/// The streams a page's `/Contents` names. An MCR whose `/Stm` names one of them refers to the
/// page's own content.
pub(crate) fn page_streams(doc: &Document, page: Option<&Dict>) -> HashSet<ObjRef> {
    let refs = |items: &[Object]| items.iter().filter_map(Object::as_ref).collect::<HashSet<_>>();
    match page.and_then(|p| p.get(b"Contents")) {
        Some(Object::Ref(r)) => match &*doc.get(*r) {
            Object::Array(a) => refs(a),
            _ => HashSet::from([*r]),
        },
        Some(Object::Array(a)) => refs(a),
        _ => HashSet::new(),
    }
}

/// The stream content an MCR's `stm` names is in, given the page's own `streams`.
pub(crate) fn content_stream(stm: Option<ObjRef>, streams: &HashSet<ObjRef>) -> Option<ObjRef> {
    stm.filter(|r| !streams.contains(r))
}

/// The marked content an element owns directly, on the page whose content `streams` are.
fn mcids(doc: &Document, d: &Dict, streams: &HashSet<ObjRef>) -> Vec<ContentRef> {
    let mut out = Vec::new();
    let mut push = |o: &Object| match &*doc.resolve(o) {
        Object::Int(n) => out.push((None, *n)),
        Object::Dict(m) if m.name(b"Type") == Some(b"MCR") => {
            if let Some(n) = m.get(b"MCID").and_then(|n| doc.resolve(n).as_int()) {
                out.push((content_stream(mcr_stm(m), streams), n))
            }
        }
        _ => {}
    };
    match d.get(b"K") {
        Some(Object::Array(a)) => a.iter().for_each(&mut push),
        Some(o) => push(o),
        None => {}
    }
    out
}

/// The page's named property lists (`/Resources /Properties`), which a `BDC` can name instead of
/// writing its property list inline.
fn page_properties(doc: &Document, page: &Dict) -> Dict {
    let res = page.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    res.get(b"Properties").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()).unwrap_or_default()
}

/// The marked-content id a `BDC` opens, whether its property list is inline or named in `props`.
fn bdc_mcid(doc: &Document, props: &Dict, op: &pdfcraft_content::Op) -> Option<i64> {
    match op.operands.get(1)? {
        Object::Dict(d) => d.get(b"MCID").and_then(Object::as_int),
        Object::Name(n) => props.get(n).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().and_then(|d| d.get(b"MCID").and_then(Object::as_int))),
        _ => None,
    }
}

pub(crate) fn page_content(doc: &Document, page: &Dict) -> Vec<u8> {
    page_content_within(doc, page, &mut DecodeBudget::page())
}

/// The page's content streams, decoded against `budget` and joined with a space. Streams that
/// don't decode, or arrive after the budget is spent, are left out.
pub(crate) fn page_content_within(doc: &Document, page: &Dict, budget: &mut DecodeBudget) -> Vec<u8> {
    let Some(c) = page.get(b"Contents") else { return Vec::new() };
    let mut out = Vec::new();
    for data in budget.decode_contents(doc, c).into_iter().flatten() {
        out.extend_from_slice(&data);
        out.push(b' ');
    }
    out
}

fn union(a: Option<[f64; 4]>, b: [f64; 4]) -> [f64; 4] {
    match a {
        Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
        None => b,
    }
}

/// An open marked-content sequence while bounds are gathered: its id, when it is one of those asked
/// for, and the bounds of what has been drawn in it so far.
struct Mark {
    id: Option<ContentRef>,
    drawn: Option<[f64; 4]>,
}

/// Adds `b` to what the innermost open sequence has drawn.
fn draw(marks: &mut [Mark], b: [f64; 4]) {
    if let Some(m) = marks.last_mut() {
        m.drawn = Some(union(m.drawn, b));
    }
}

/// Closes the innermost open sequence: what it drew counts for its id and for the sequence around it.
fn close(marks: &mut Vec<Mark>, matching: &mut usize, out: &mut HashMap<ContentRef, [f64; 4]>) {
    let Some(m) = marks.pop() else { return };
    if m.id.is_some() {
        *matching = matching.saturating_sub(1);
    }
    let Some(b) = m.drawn else { return };
    if let Some(id) = m.id {
        let e = out.entry(id).or_insert(b);
        *e = union(Some(*e), b);
    }
    draw(marks, b);
}

/// The page-space box a `Do` operator paints: a form's transformed `/BBox`, or the unit square for an image.
/// `None` when the operand is not a resolvable XObject or the form's box is malformed.
fn xobject_bbox(doc: &Document, xobjects: &Dict, op: &pdfcraft_content::Op, top: &Matrix) -> Option<[f64; 4]> {
    let r = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref)?;
    Some(match &*doc.get(r) {
        Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form") => {
            let bb: Vec<f64> =
                s.dict.get(b"BBox").and_then(|b| b.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default();
            let m = Matrix::from_operands(s.dict.get(b"Matrix").and_then(|m| m.as_array()).map(Vec::as_slice).unwrap_or(&[]))
                .unwrap_or(Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
            match bb[..] {
                [a, b, c, d] => m.then(top).bbox([a, b, c, d]),
                _ => return None,
            }
        }
        _ => top.bbox([0.0, 0.0, 1.0, 1.0]),
    })
}

/// How deeply forms drawn from forms are followed when locating marked content in them.
const MAX_FORM_DEPTH: usize = 16;

fn sub_dict(doc: &Document, d: &Dict, key: &[u8]) -> Dict {
    d.get(key).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
}

/// Where each of the marked content `ids` is drawn on a page (images, forms and paths), in one pass
/// over the page. What a sequence draws counts for every sequence of `ids` around it too. Content a
/// form XObject holds (an id with a stream) is found by following the form's `Do` on the page, with
/// its `/Matrix` and the transformation in force there, through at most [`MAX_FORM_DEPTH`] forms
/// and the page's decode and form allowance.
pub(crate) fn content_bboxes(doc: &Document, page: &Dict, ids: &HashSet<ContentRef>) -> HashMap<ContentRef, [f64; 4]> {
    let res = sub_dict(doc, page, b"Resources");
    #[cfg(test)]
    crate::BOUNDS_PASSES.with(|c| c.set(c.get() + 1));
    let mut bounds =
        Bounds { doc, ids, in_forms: ids.iter().any(|(s, _)| s.is_some()), budget: DecodeBudget::page(), open: Vec::new(), out: HashMap::new() };
    bounds.walk(&page_content(doc, page), &res, None, Matrix::IDENTITY);
    bounds.out
}

/// One pass gathering where marked content is drawn, into forms as well as on the page.
struct Bounds<'a> {
    doc: &'a Document,
    ids: &'a HashSet<ContentRef>,
    /// Whether any of `ids` is in a stream of its own, so forms are worth entering.
    in_forms: bool,
    budget: DecodeBudget,
    /// The forms being walked, outermost first: a form drawing itself isn't entered again.
    open: Vec<ObjRef>,
    out: HashMap<ContentRef, [f64; 4]>,
}

impl Bounds<'_> {
    /// Walk the content of `stream` (`None` for the page's), drawn with `base` as its starting
    /// transformation.
    fn walk(&mut self, content: &[u8], res: &Dict, stream: Option<ObjRef>, base: Matrix) {
        let doc = self.doc;
        let xobjects = sub_dict(doc, res, b"XObject");
        let props = sub_dict(doc, res, b"Properties");
        let mut ctm = vec![base];
        // The open marked-content sequences, and how many of them are one of `ids`.
        let mut marks: Vec<Mark> = Vec::new();
        let mut matching = 0usize;
        let mut path: Vec<(f64, f64)> = Vec::new();
        for op in pdfcraft_content::parse(content).ops {
            crate::count_mark_reads(1);
            let inside = matching > 0;
            let top = ctm.last().copied().unwrap_or(base);
            match op.op.as_slice() {
                b"q" => ctm.push(top),
                b"Q" if ctm.len() > 1 => {
                    ctm.pop();
                }
                b"cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands)
                        && let Some(t) = ctm.last_mut()
                    {
                        *t = m.then(&top);
                    }
                }
                b"BMC" => marks.push(Mark { id: None, drawn: None }),
                b"BDC" => {
                    let id = bdc_mcid(doc, &props, &op).map(|n| (stream, n)).filter(|id| self.ids.contains(id));
                    matching += usize::from(id.is_some());
                    marks.push(Mark { id, drawn: None });
                }
                b"EMC" => close(&mut marks, &mut matching, &mut self.out),
                b"m" | b"l" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        path.push(top.apply(x, y));
                    }
                }
                b"c" => {
                    if let Some([_, _, _, _, x, y]) = op.nums::<6>() {
                        path.push(top.apply(x, y));
                    }
                }
                b"re" => {
                    if let Some([x, y, w, h]) = op.nums::<4>() {
                        let b = top.bbox([x, y, x + w, y + h]);
                        path.push((b[0], b[1]));
                        path.push((b[2], b[3]));
                    }
                }
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"n" => {
                    if inside && !matches!(op.op.as_slice(), b"n") {
                        for (x, y) in &path {
                            draw(&mut marks, [*x, *y, *x, *y]);
                        }
                    }
                    path.clear();
                }
                b"Do" => {
                    if inside && let Some(unit) = xobject_bbox(doc, &xobjects, &op, &top) {
                        draw(&mut marks, unit);
                    }
                    self.enter(&xobjects, &op, &top, res);
                }
                b"BI" if inside => draw(&mut marks, top.bbox([0.0, 0.0, 1.0, 1.0])),
                _ => {}
            }
        }
        while !marks.is_empty() {
            close(&mut marks, &mut matching, &mut self.out);
        }
    }

    /// Walk the form a `Do` draws, when marked content asked for may be in a form. A form without
    /// `/Resources` takes those of the content drawing it.
    fn enter(&mut self, xobjects: &Dict, op: &pdfcraft_content::Op, top: &Matrix, res: &Dict) {
        if !self.in_forms || self.open.len() >= MAX_FORM_DEPTH {
            return;
        }
        let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { return };
        if self.open.contains(&r) {
            return;
        }
        let target = self.doc.get(r);
        let Object::Stream(s) = &*target else { return };
        if s.dict.name(b"Subtype") != Some(b"Form") || !self.budget.enter_form() {
            return;
        }
        let Ok(data) = self.budget.decode(s) else { return };
        let m = Matrix::from_operands(s.dict.get(b"Matrix").and_then(|m| m.as_array()).map(Vec::as_slice).unwrap_or(&[])).unwrap_or(Matrix::IDENTITY);
        let form_res = match s.dict.get(b"Resources") {
            Some(_) => sub_dict(self.doc, &s.dict, b"Resources"),
            None => res.clone(),
        };
        self.open.push(r);
        self.walk(&data, &form_res, Some(r), m.then(top));
        self.open.pop();
    }
}

/// The figures, in document order.
pub fn figures(doc: &Document) -> Vec<Figure> {
    let Some(root) = tree_root(doc) else { return Vec::new() };
    let role_map = root.get(b"RoleMap").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let pages = pdfcraft_model::pages(doc);
    let index: HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let mut streams: HashMap<usize, HashSet<ObjRef>> = HashMap::new();
    let figures: Vec<(Element, Option<usize>, Vec<ContentRef>)> = elements(doc)
        .into_iter()
        .filter(|e| e.dict.name(b"S").is_some_and(|s| standard(doc, &role_map, s) == b"Figure"))
        .map(|e| {
            let page = e.page.and_then(|p| index.get(&p).copied());
            let owned = match page {
                Some(p) => {
                    let streams = streams.entry(p).or_insert_with(|| page_streams(doc, pages.get(p).map(|x| &x.dict)));
                    mcids(doc, &e.dict, streams)
                }
                None => Vec::new(),
            };
            (e, page, owned)
        })
        .collect();
    let mut wanted: HashMap<usize, HashSet<ContentRef>> = HashMap::new();
    for (_, page, owned) in &figures {
        if let Some(p) = page {
            wanted.entry(*p).or_default().extend(owned.iter().copied());
        }
    }
    let bounds: HashMap<usize, HashMap<ContentRef, [f64; 4]>> =
        wanted.into_iter().filter_map(|(p, ids)| Some((p, content_bboxes(doc, &pages.get(p)?.dict, &ids)))).collect();
    figures
        .into_iter()
        .map(|(e, page, owned)| {
            let alt = e.dict.get(b"Alt").map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text()));
            let on_page = page.and_then(|p| bounds.get(&p));
            let bbox = on_page.and_then(|b| owned.iter().filter_map(|id| b.get(id)).fold(None, |acc, b| Some(union(acc, *b))));
            Figure { obj: e.object, page, alt, bbox }
        })
        .collect()
}

fn check_figure(doc: &Document, obj: ObjRef) -> Result<Dict, AltError> {
    let root = tree_root(doc).ok_or(AltError::Untagged)?;
    let role_map = root.get(b"RoleMap").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let d = doc.get(obj).as_dict().cloned().ok_or(AltError::NotAFigure(obj.num))?;
    match d.name(b"S") {
        Some(s) if standard(doc, &role_map, s) == b"Figure" => Ok(d),
        _ => Err(AltError::NotAFigure(obj.num)),
    }
}

/// Set (or, with `None` or blank text, remove) a figure's alternate text.
pub fn set_alt(doc: &mut Document, obj: ObjRef, alt: Option<&str>) -> Result<(), AltError> {
    check_figure(doc, obj)?;
    let alt = alt.map(str::trim).filter(|a| !a.is_empty());
    doc.update_dict(obj, |d| match alt {
        Some(a) => d.set(b"Alt".to_vec(), Object::String(PdfString::text(a))),
        None => {
            d.remove(b"Alt");
        }
    })?;
    Ok(())
}

/// The page's content entries: one stream, or the elements of a contents array.
fn content_entries(doc: &Document, page: &Dict) -> Vec<Object> {
    let Some(c) = page.get(b"Contents") else { return Vec::new() };
    match &*doc.resolve(c) {
        Object::Array(a) => a.clone(),
        Object::Stream(_) => vec![c.clone()],
        _ => Vec::new(),
    }
}

/// How much of a content stream decoded.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Read {
    Whole,
    /// Damaged partway: the data is what decoded before the damage. What follows it is unknown,
    /// the syntax the later streams start in too.
    Damaged,
    /// Not at all (a filter this reader lacks, say): the data is empty.
    Unread,
}

/// A `/Contents` entry and, for a stream, the stream, its decoded data and how much of it decoded.
type DecodedStream = (Object, Option<(Stream, Vec<u8>, Read)>);

/// Decode the page's content streams within the page decode budget. `None` when a stream is
/// rejected because the budget is exhausted.
fn decode_page_streams(doc: &Document, page: &Dict) -> Option<Vec<DecodedStream>> {
    let entries = content_entries(doc, page);
    let mut streams = Vec::with_capacity(entries.len());
    let mut budget = DecodeBudget::page();
    for entry in entries {
        let decoded = match &*doc.resolve(&entry) {
            Object::Stream(s) => {
                let (data, read) = match budget.decode_with_damage(doc, s, usize::MAX) {
                    Ok((data, damaged)) => (data, if damaged { Read::Damaged } else { Read::Whole }),
                    // Content past the budget may hold more of the tag: editing the rest
                    // would leave that part marked as a tag the tree no longer has.
                    Err(_) if budget.exhausted() => return None,
                    Err(_) => (Vec::new(), Read::Unread),
                };
                Some((s.clone(), data, read))
            }
            _ => None,
        };
        streams.push((entry, decoded));
    }
    Some(streams)
}

fn marked_id(doc: &Document, props: &Dict, op: &pdfcraft_content::Op, ids: &HashSet<i64>) -> Option<i64> {
    (op.op == b"BDC").then(|| bdc_mcid(doc, props, op)).flatten().filter(|n| ids.contains(n))
}

/// What [`artifact_marked_content`] did.
#[derive(Debug, PartialEq)]
enum Marked {
    /// The streams holding the content were rewritten.
    Done,
    /// None of the content is on the page, or with `require_all` not all of it: nothing changed.
    NotFound,
    /// Some of it is, or may be, in a stream damaged partway or one that doesn't decode, neither of
    /// which can be rewritten without losing what wasn't read, or after the damage: nothing changed.
    Damaged,
}

/// Turn the tag's marked-content blocks into artifacts, rewriting only the streams that
/// contain them. Other content streams, and unknown entries on a rewritten stream, stay.
/// With `require_all`, every id in `ids` must appear on the page before any stream is rewritten.
fn artifact_marked_content(
    doc: &mut Document,
    page: &pdfcraft_model::Page,
    streams: Vec<DecodedStream>,
    ids: &HashSet<i64>,
    require_all: bool,
) -> Result<Marked, pdfcraft_cos::CosError> {
    let props = page_properties(doc, &page.dict);
    // The pieces are the streams, in order.
    let decoded: Vec<&[u8]> = streams.iter().filter_map(|(_, s)| s.as_ref().map(|(_, bytes, _)| bytes.as_slice())).collect();
    let read: Vec<Read> = streams.iter().filter_map(|(_, s)| s.as_ref().map(|(_, _, read)| *read)).collect();
    let damage = read.iter().position(|r| *r == Read::Damaged);
    let pieces = pdfcraft_content::Pieces::join(&decoded);
    let mut found = HashSet::new();
    let mut edits = Vec::new();
    for op in pieces.parse().ops {
        if let Some(id) = marked_id(doc, &props, &op, ids) {
            // What follows the damage is unknown: a string or comment the damage left open runs
            // on into the next stream, so what reads as an operator after it may not be one. An
            // operator whose operands start before a stream not read whole and whose keyword comes
            // after it takes them from around what wasn't read: that stream may end them itself.
            let (first, last) = pieces.pieces_of(&op);
            if damage.is_some_and(|d| last > d) || read.get(first..last).is_some_and(|r| r.iter().any(|r| *r != Read::Whole)) {
                return Ok(Marked::Damaged);
            }
            found.insert(id);
            edits.push((op.span, b"/Artifact BMC".to_vec()));
        }
    }
    // Content not found may be past the damage, or in a stream that doesn't decode: whether it
    // is on the page isn't known.
    if found != *ids && read.iter().any(|r| *r != Read::Whole) {
        return Ok(Marked::Damaged);
    }
    if if require_all { found != *ids } else { found.is_empty() } {
        return Ok(Marked::NotFound);
    }
    let spliced = pieces.splice(edits);
    if read.iter().zip(&decoded).zip(&spliced).any(|((read, old), new)| *read != Read::Whole && *old != new.as_slice()) {
        return Ok(Marked::Damaged);
    }
    let mut spliced = spliced.into_iter();
    let mut next = Vec::with_capacity(streams.len());
    for (entry, stream) in streams {
        let Some((stream, old, _)) = stream else {
            next.push(entry);
            continue;
        };
        let Some(new) = spliced.next() else {
            next.push(entry);
            continue;
        };
        if new == old {
            next.push(entry);
            continue;
        }
        let mut dict = stream.dict;
        dict.remove(b"Length");
        dict.remove(b"Filter");
        dict.remove(b"DecodeParms");
        let stream = Object::Stream(Stream::flate(dict, &new));
        // Splicing depends on this occurrence's neighbours. The original stream may also
        // appear elsewhere in this array or on another page, where it must stay unchanged.
        next.push(Object::Ref(doc.add(stream)));
    }
    let contents = match next.as_slice() {
        [one] => one.clone(),
        _ => Object::Array(next),
    };
    doc.update_dict(page.obj, |p| p.set(b"Contents".to_vec(), contents))?;
    Ok(Marked::Done)
}

/// The marked content an element owns, split into the ids in the page's own content and, for each
/// other stream an MCR's `/Stm` names, the ids in that stream (in the order first referenced).
fn split_by_stream(owned: &[ContentRef]) -> (HashSet<i64>, Vec<(ObjRef, HashSet<i64>)>) {
    let mut page = HashSet::new();
    let mut forms: Vec<(ObjRef, HashSet<i64>)> = Vec::new();
    for (stream, id) in owned {
        match stream {
            None => {
                page.insert(*id);
            }
            Some(s) => match forms.iter_mut().find(|(f, _)| f == s) {
                Some((_, ids)) => {
                    ids.insert(*id);
                }
                None => forms.push((*s, HashSet::from([*id]))),
            },
        }
    }
    (page, forms)
}

/// What making a tag's marked content in a form XObject an artifact would write.
enum FormArtifact {
    /// The form's new stream, and its `/StructParents` key, when it has one.
    Ready(Stream, Option<i64>),
    /// None of the content is in the form, or with `require_all` not all of it.
    NotFound,
    /// The form is damaged partway or doesn't decode: rewriting it would lose what wasn't read.
    Damaged,
    /// Decoding the form would take more than a page's allowance.
    TooLarge,
}

/// The stream `form` (which an MCR's `/Stm` names) with its marked content `ids` made artifacts,
/// without changing the document. Its named property lists are its own `/Resources`, or the
/// page's when it has none.
fn artifact_form(doc: &Document, form: ObjRef, page: Option<&Dict>, ids: &HashSet<i64>, require_all: bool) -> FormArtifact {
    let target = doc.get(form);
    let Object::Stream(s) = &*target else { return FormArtifact::NotFound };
    let mut budget = DecodeBudget::page();
    let data = match budget.decode_with_damage(doc, s, usize::MAX) {
        Ok((data, false)) => data,
        Ok((_, true)) => return FormArtifact::Damaged,
        Err(_) if budget.exhausted() => return FormArtifact::TooLarge,
        Err(_) => return FormArtifact::Damaged,
    };
    let props = match s.dict.get(b"Resources") {
        Some(_) => sub_dict(doc, &sub_dict(doc, &s.dict, b"Resources"), b"Properties"),
        None => page.map(|p| page_properties(doc, p)).unwrap_or_default(),
    };
    let pieces = pdfcraft_content::Pieces::join(&[data.as_slice()]);
    let mut found = HashSet::new();
    let mut edits = Vec::new();
    for op in pieces.parse().ops {
        if let Some(id) = marked_id(doc, &props, &op, ids) {
            found.insert(id);
            edits.push((op.span, b"/Artifact BMC".to_vec()));
        }
    }
    if if require_all { found != *ids } else { found.is_empty() } {
        return FormArtifact::NotFound;
    }
    let Some(new) = pieces.splice(edits).into_iter().next() else { return FormArtifact::NotFound };
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    dict.remove(b"Filter");
    dict.remove(b"DecodeParms");
    let key = s.dict.get(b"StructParents").and_then(|o| doc.resolve(o).as_int());
    FormArtifact::Ready(Stream::flate(dict, &new), key)
}

/// Plan the artifact rewrite of every form holding some of a tag's content. Nothing changes.
fn plan_forms(
    doc: &Document,
    page: Option<&Dict>,
    forms: &[(ObjRef, HashSet<i64>)],
    require_all: bool,
) -> Result<Vec<(ObjRef, Stream, Option<i64>)>, FormArtifact> {
    let mut ready = Vec::with_capacity(forms.len());
    for (form, ids) in forms {
        match artifact_form(doc, *form, page, ids, require_all) {
            FormArtifact::Ready(stream, key) => ready.push((*form, stream, key)),
            FormArtifact::NotFound if !require_all => {}
            other => return Err(other),
        }
    }
    Ok(ready)
}

/// Write the planned form rewrites, each in place (the MCR names that object, wherever it is
/// drawn), and take `obj` out of each form's parent-tree entry.
fn write_forms(doc: &mut Document, ready: Vec<(ObjRef, Stream, Option<i64>)>, obj: ObjRef) -> Result<(), AltError> {
    for (form, stream, key) in ready {
        doc.set(form, Object::Stream(stream));
        if let Some(key) = key {
            clear_parent_tree(doc, key, obj)?;
        }
    }
    Ok(())
}

/// Decorative figure: its marked content becomes an artifact, on the page and in any form XObject
/// an MCR's `/Stm` names, and the element leaves the structure tree (and the parent tree). Nothing
/// changes when some of it is in a stream that can't be rewritten whole.
pub fn mark_decorative(doc: &mut Document, obj: ObjRef) -> Result<(), AltError> {
    let d = check_figure(doc, obj)?;
    let pages = pdfcraft_model::pages(doc);
    let pg = d.get(b"Pg").and_then(Object::as_ref).or_else(|| elements(doc).into_iter().find(|e| e.object == obj).and_then(|e| e.page));
    let page = pg.and_then(|p| pages.iter().find(|x| x.obj == p));
    let owned = mcids(doc, &d, &page_streams(doc, page.map(|p| &p.dict)));
    let (ids, forms) = split_by_stream(&owned);
    let forms = match plan_forms(doc, page.map(|p| &p.dict), &forms, false) {
        Ok(ready) => ready,
        Err(FormArtifact::TooLarge) => return Err(AltError::TooLarge),
        Err(_) => return Err(AltError::Damaged),
    };
    if let Some(page) = page.filter(|_| !ids.is_empty()) {
        let streams = decode_page_streams(doc, &page.dict).ok_or(AltError::TooLarge)?;
        match artifact_marked_content(doc, page, streams, &ids, false)? {
            // The parent tree no longer points those ids at the figure.
            Marked::Done => {
                if let Some(key) = page.dict.get(b"StructParents").and_then(|o| doc.resolve(o).as_int()) {
                    clear_parent_tree(doc, key, obj)?;
                }
            }
            Marked::NotFound => {}
            Marked::Damaged => return Err(AltError::Damaged),
        }
    }
    write_forms(doc, forms, obj)?;
    // Out of its parent's kids.
    if let Some(parent) = d.get(b"P").and_then(Object::as_ref) {
        let kids = doc.get(parent).as_dict().and_then(|p| p.get(b"K").cloned());
        let keep = |o: &Object| o.as_ref() != Some(obj);
        let new = match kids {
            Some(Object::Array(a)) => Some(Object::Array(a.into_iter().filter(keep).collect())),
            Some(Object::Ref(r)) if r == obj => Some(Object::Array(Vec::new())),
            _ => None,
        };
        if let Some(k) = new {
            doc.update_dict(parent, |p| p.set(b"K".to_vec(), k))?;
        }
    }
    Ok(())
}

/// Mark a leaf structure tag's directly owned marked content as an artifact.
///
/// This deliberately refuses a tag with child structure elements. Converting a parent would
/// otherwise silently discard semantic descendants. The visible page content is preserved: each
/// matching marked-content sequence changes from `BDC` with an MCID to `/Artifact BMC`.
pub fn mark_tag_artifact(doc: &mut Document, obj: ObjRef) -> Result<(), ArtifactError> {
    tree_root(doc).ok_or(ArtifactError::Untagged)?;
    let elements = elements(doc);
    let Some(element) = elements.iter().find(|e| e.object == obj) else { return Err(ArtifactError::NotATag(obj.num)) };
    if has_child_structure(doc, &element.dict) {
        return Err(ArtifactError::HasChildren);
    }
    let pages = pdfcraft_model::pages(doc);
    let page = element.page.and_then(|r| pages.iter().find(|page| page.obj == r));
    let owned = mcids(doc, &element.dict, &page_streams(doc, page.map(|p| &p.dict)));
    if owned.is_empty() {
        return Err(ArtifactError::NoMarkedContent);
    }
    // Content in the page's own streams needs the page; content in a form only the form.
    let (ids, forms) = split_by_stream(&owned);
    let page = if ids.is_empty() { page } else { Some(page.ok_or(ArtifactError::NoPage)?) };
    let parent = element.dict.get(b"P").and_then(Object::as_ref).ok_or(ArtifactError::NoParent)?;
    let replacement = replacement_kids(doc, parent, obj)?;
    // Every form is checked before anything is written, so a form missing some of the content, or
    // one that can't be rewritten whole, leaves the document as it was.
    let forms = match plan_forms(doc, page.map(|p| &p.dict), &forms, true) {
        Ok(ready) => ready,
        Err(FormArtifact::TooLarge) => return Err(ArtifactError::TooLarge),
        Err(FormArtifact::Damaged) => return Err(ArtifactError::Damaged),
        Err(_) => return Err(ArtifactError::ContentNotFound),
    };
    if let Some(page) = page.filter(|_| !ids.is_empty()) {
        let streams = decode_page_streams(doc, &page.dict).ok_or(ArtifactError::TooLarge)?;
        match artifact_marked_content(doc, page, streams, &ids, true)? {
            Marked::Done => {}
            Marked::NotFound => return Err(ArtifactError::ContentNotFound),
            Marked::Damaged => return Err(ArtifactError::Damaged),
        }
        if let Some(key) = page.dict.get(b"StructParents").and_then(|o| doc.resolve(o).as_int()) {
            clear_parent_tree(doc, key, obj).map_err(|e| ArtifactError::ParentTree(e.to_string()))?;
        }
    }
    write_forms(doc, forms, obj).map_err(|e| ArtifactError::ParentTree(e.to_string()))?;
    doc.update_dict(parent, |p| p.set(b"K".to_vec(), replacement))?;
    // Keep the retired object valid if another object holds an out-of-tree reference to it.
    doc.update_dict(obj, |d| {
        d.remove(b"P");
    })?;
    Ok(())
}

fn has_child_structure(doc: &Document, tag: &Dict) -> bool {
    fn child(doc: &Document, item: &Object) -> bool {
        match item {
            Object::Array(items) => items.iter().any(|item| child(doc, item)),
            Object::Ref(r) => doc.get(*r).as_dict().is_some_and(|d| d.contains(b"S")),
            Object::Dict(d) => d.contains(b"S"),
            _ => false,
        }
    }
    tag.get(b"K").is_some_and(|items| child(doc, items))
}

fn replacement_kids(doc: &Document, parent: ObjRef, obj: ObjRef) -> Result<Object, ArtifactError> {
    let kids = doc.get(parent).as_dict().and_then(|p| p.get(b"K").cloned()).ok_or(ArtifactError::NoParent)?;
    match kids {
        Object::Array(items) => {
            if items.iter().filter(|item| item.as_ref() == Some(obj)).count() != 1 {
                return Err(ArtifactError::AmbiguousParent);
            }
            Ok(Object::Array(items.into_iter().filter(|item| item.as_ref() != Some(obj)).collect()))
        }
        Object::Ref(r) if r == obj => Ok(Object::Array(Vec::new())),
        _ => Err(ArtifactError::NoParent),
    }
}

/// Replace `obj` by null in the page's parent-tree array (`key`), in a flat /Nums tree.
fn clear_parent_tree(doc: &mut Document, key: i64, obj: ObjRef) -> Result<(), AltError> {
    let Some(pt) = tree_root(doc).and_then(|r| r.get(b"ParentTree").and_then(Object::as_ref)) else { return Ok(()) };
    let Some(nums) = doc.get(pt).as_dict().and_then(|d| d.get(b"Nums").and_then(|n| n.as_array().cloned())) else { return Ok(()) };
    let mut nums = nums;
    // Whole pairs only: a trailing key without a value maps nothing, so it is left as it is.
    for [k, v] in nums.as_chunks_mut::<2>().0 {
        if k.as_int() == Some(key) {
            match &*v {
                // The entry is the element itself rather than an array of them.
                Object::Ref(r) if *r == obj => *v = Object::Null,
                Object::Array(a) => *v = Object::Array(a.iter().map(|x| if x.as_ref() == Some(obj) { Object::Null } else { x.clone() }).collect()),
                Object::Ref(r) => {
                    if let Some(a) = doc.get(*r).as_array().cloned() {
                        let a: Vec<Object> = a.iter().map(|x| if x.as_ref() == Some(obj) { Object::Null } else { x.clone() }).collect();
                        doc.set(*r, Object::Array(a));
                    }
                }
                _ => {}
            }
        }
    }
    doc.update_dict(pt, |d| d.set(b"Nums".to_vec(), Object::Array(nums)))?;
    Ok(())
}
