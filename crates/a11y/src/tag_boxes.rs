//! Where a tag's content, and its descendants' content, is drawn: what the Tags panel outlines.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::alt::{self, union};
use crate::glyphs::MAX_SAVED;
use crate::tags::{TagError, tag_type_name};

/// How deep the tag's subtree may nest, counting arrays and references.
const MAX_DEPTH: usize = 512;
/// How many `/K` entries the walk visits before giving up.
const MAX_ITEMS: usize = 65_536;

/// One box a tag's content is drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct TagBox {
    /// Zero-based page.
    pub page: usize,
    /// `[x0, y0, x1, y1]` in the page's user space.
    pub rect: [f64; 4],
}

/// What the subtree of a tag refers to.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Item {
    /// Marked content in the page's own content.
    Mark { page: ObjRef, mcid: i64 },
    /// Marked content in a form XObject (`/Stm`): outlined where the form is placed.
    Stream { page: ObjRef, stream: ObjRef, mcid: Option<i64> },
    /// An object reference: an annotation, or an XObject placed on the page.
    Object { page: Option<ObjRef>, object: ObjRef },
}

/// Where tag `object`'s content and its descendants' content is drawn, one box per marked-content
/// sequence or referenced object, in page order and then structure order.
///
/// Marked content on a page counts what it paints (glyph boxes from the font's advance, ascent and
/// descent; paths; images; forms). Marked content inside a form XObject (`/Stm`) counts the whole
/// form where the page places it, and an object reference counts an annotation's `/Rect` or an
/// XObject's placement. Returns `NotATag` for an object that isn't a reachable indirect tag, and
/// `ContentLimit` beyond 65,536 entries or 512 levels.
pub fn tag_boxes(doc: &Document, object: ObjRef) -> Result<Vec<TagBox>, TagError> {
    // Only what the tag listing lists: a tag type that is a name.
    let Some(element) = alt::elements(doc).into_iter().find(|e| e.object == object && tag_type_name(doc, &e.dict).is_some()) else {
        return Err(TagError::NotATag(object));
    };
    let items = subtree_items(doc, object, &element.dict, element.page)?;
    let mut by_page: HashMap<ObjRef, Vec<Item>> = HashMap::new();
    let mut kept = HashSet::new();
    for item in items {
        if let Some(page) = item_page(doc, &item) {
            // One annotation reached with and without `/Pg` is one box on its page.
            let item = match item {
                Item::Object { object, .. } => Item::Object { page: Some(page), object },
                other => other,
            };
            if kept.insert(item) {
                by_page.entry(page).or_default().push(item);
            }
        }
    }
    let mut out = Vec::new();
    for (index, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        // A page listed twice in the page tree is still read once.
        if let Some(here) = by_page.remove(&page.obj) {
            page_boxes(doc, &page.dict, &here, |rect| out.push(TagBox { page: index, rect }));
        }
    }
    Ok(out)
}

/// Each page's union of `boxes` (as [`tag_boxes`] returns them, in page order): the one outline
/// per page the Tags panel draws.
pub fn tag_page_bounds(boxes: &[TagBox]) -> Vec<TagBox> {
    let mut out: Vec<TagBox> = Vec::new();
    for b in boxes {
        match out.last_mut() {
            Some(last) if last.page == b.page => last.rect = union(Some(last.rect), b.rect),
            _ => out.push(b.clone()),
        }
    }
    out
}

/// The page an item is drawn on; an object reference without `/Pg` falls back to an annotation's `/P`.
fn item_page(doc: &Document, item: &Item) -> Option<ObjRef> {
    match *item {
        Item::Mark { page, .. } | Item::Stream { page, .. } => Some(page),
        Item::Object { page: Some(page), .. } => Some(page),
        Item::Object { page: None, object } => doc.get(object).as_dict().and_then(|d| d.get(b"P")).and_then(Object::as_ref),
    }
}

fn is_type(doc: &Document, d: &Dict, name: &[u8]) -> bool {
    d.get(b"Type").map(|t| doc.resolve(t)).as_deref().and_then(Object::as_name) == Some(name)
}

/// Everything the tag's subtree refers to, in structure order without repeats. Child tags are
/// walked through, whether indirect or inline, with `/Pg` inherited from the nearest one that has it.
fn subtree_items(doc: &Document, root: ObjRef, dict: &Dict, page: Option<ObjRef>) -> Result<Vec<Item>, TagError> {
    let mut seen = HashSet::from([root]);
    let mut found = HashSet::new();
    let mut out = Vec::new();
    let mut remaining = MAX_ITEMS;
    let mut stack: Vec<(Object, Option<ObjRef>, usize)> = dict.get(b"K").map(|k| vec![(k.clone(), page, 0)]).unwrap_or_default();
    while let Some((k, pg, depth)) = stack.pop() {
        remaining = remaining.checked_sub(1).ok_or(TagError::ContentLimit)?;
        if depth > MAX_DEPTH {
            return Err(TagError::ContentLimit);
        }
        let item = match k {
            Object::Int(mcid) => pg.map(|page| Item::Mark { page, mcid }),
            Object::Array(a) => {
                if a.len() > remaining.saturating_sub(stack.len()) {
                    return Err(TagError::ContentLimit);
                }
                stack.extend(a.into_iter().rev().map(|x| (x, pg, depth + 1)));
                None
            }
            Object::Ref(r) if seen.insert(r) => {
                stack.push(((*doc.get(r)).clone(), pg, depth + 1));
                None
            }
            Object::Dict(d) if is_type(doc, &d, b"MCR") => mcr_item(doc, &d, pg),
            Object::Dict(d) if is_type(doc, &d, b"OBJR") => {
                let page = d.get(b"Pg").and_then(Object::as_ref).or(pg);
                d.get(b"Obj").and_then(Object::as_ref).map(|object| Item::Object { page, object })
            }
            Object::Dict(d) if d.contains(b"S") => {
                let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
                if let Some(kids) = d.get(b"K") {
                    stack.push((kids.clone(), pg, depth + 1));
                }
                None
            }
            _ => None,
        };
        if let Some(item) = item
            && found.insert(item)
        {
            out.push(item);
        }
    }
    Ok(out)
}

/// A marked-content reference: in the page's content, or in the form XObject `/Stm` names.
fn mcr_item(doc: &Document, d: &Dict, pg: Option<ObjRef>) -> Option<Item> {
    let page = d.get(b"Pg").and_then(Object::as_ref).or(pg)?;
    match d.get(b"Stm").and_then(Object::as_ref) {
        Some(stream) => Some(Item::Stream { page, stream, mcid: mcid(doc, d) }),
        None => mcid(doc, d).map(|mcid| Item::Mark { page, mcid }),
    }
}

fn mcid(doc: &Document, d: &Dict) -> Option<i64> {
    d.get(b"MCID").map(|m| doc.resolve(m)).and_then(|m| m.as_int())
}

/// Calls `f` with each item's box on one page, in the items' order. Items drawn nowhere are skipped.
fn page_boxes(doc: &Document, page: &Dict, items: &[Item], mut f: impl FnMut([f64; 4])) {
    let ids: HashSet<i64> = items.iter().filter_map(|i| if let Item::Mark { mcid, .. } = i { Some(*mcid) } else { None }).collect();
    let objects: HashSet<ObjRef> = items
        .iter()
        .filter_map(|i| match *i {
            Item::Stream { stream, .. } => Some(stream),
            Item::Object { object, .. } => Some(object),
            Item::Mark { .. } => None,
        })
        .collect();
    let mut marks = if ids.is_empty() { HashMap::new() } else { mark_bounds(doc, page, &ids) };
    let placed = if objects.is_empty() { HashMap::new() } else { placements(doc, page, &objects) };
    for item in items {
        let rect = match *item {
            Item::Mark { mcid, .. } => marks.remove(&mcid),
            Item::Stream { stream, .. } => placed.get(&stream).copied(),
            Item::Object { object, .. } => annotation_rect(doc, object).or_else(|| placed.get(&object).copied()),
        };
        // Within `f32`, which the screen and the headless tool draw and report in.
        if let Some(rect) = rect.filter(|r| r.iter().all(|v| v.is_finite() && v.abs() <= f64::from(f32::MAX))) {
            f(rect);
        }
    }
}

/// What each of `ids` paints on the page: its paths, images and forms, and its glyphs' boxes.
fn mark_bounds(doc: &Document, page: &Dict, ids: &HashSet<i64>) -> HashMap<i64, [f64; 4]> {
    let mut out = alt::content_bboxes(doc, page, ids);
    crate::glyphs::for_each_glyph(doc, page, |g| {
        // A vertical font's glyphs are placed as if horizontal here: no box rather than a wrong one.
        if let Some(id) = g.id.filter(|id| !g.vertical && ids.contains(id)) {
            let b = g.bbox();
            let e = out.entry(id).or_insert(b);
            *e = union(Some(*e), b);
        }
    });
    out
}

/// An annotation's `/Rect`, normalised so the first corner is the lower left.
fn annotation_rect(doc: &Document, object: ObjRef) -> Option<[f64; 4]> {
    let target = doc.get(object);
    let d = target.as_dict()?;
    // Every entry must be a number: skipping one would shift the others into the wrong corners.
    let r: Vec<f64> =
        d.get(b"Rect").map(|r| doc.resolve(r)).and_then(|r| r.as_array().and_then(|a| a.iter().map(|v| doc.resolve(v).as_f64()).collect()))?;
    match r[..] {
        [a, b, c, e] => Some([a.min(c), b.min(e), a.max(c), b.max(e)]),
        _ => None,
    }
}

/// Where the page's own content places each XObject of `objects` (a form's `/BBox` under its
/// `/Matrix` and the CTM, an image's unit square), united over every placement.
fn placements(doc: &Document, page: &Dict, objects: &HashSet<ObjRef>) -> HashMap<ObjRef, [f64; 4]> {
    let res = page.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let xobjects = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let mut ctm = Matrix::IDENTITY;
    let mut saved: Vec<Matrix> = Vec::new();
    let mut unsaved = 0usize;
    let mut out = HashMap::new();
    for op in pdfcraft_content::parse(&alt::page_content(doc, page)).ops {
        match op.op.as_slice() {
            b"q" if saved.len() < MAX_SAVED => saved.push(ctm),
            b"q" => unsaved += 1,
            b"Q" if unsaved > 0 => unsaved -= 1,
            b"Q" => ctm = saved.pop().unwrap_or(ctm),
            b"cm" => {
                if let Some(m) = Matrix::from_operands(&op.operands) {
                    ctm = m.then(&ctm);
                }
            }
            b"Do" => {
                let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref).filter(|r| objects.contains(r)) else {
                    continue;
                };
                if let Some(b) = alt::xobject_bbox(doc, &xobjects, &op, &ctm) {
                    let e = out.entry(r).or_insert(b);
                    *e = union(Some(*e), b);
                }
            }
            _ => {}
        }
    }
    out
}
