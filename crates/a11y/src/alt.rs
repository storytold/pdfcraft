//! Add alternate text: the document's figures one by one, their alternate text, and marking a
//! figure as decorative (its content becomes an artifact and its tag goes away).

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

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
                let Some(d) = doc.get(r).as_dict().cloned() else { continue };
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

/// The marked-content ids an element owns directly.
fn mcids(d: &Dict) -> Vec<i64> {
    let mut out = Vec::new();
    let mut push = |o: &Object| match o {
        Object::Int(n) => out.push(*n),
        Object::Dict(m) if m.name(b"Type") == Some(b"MCR") => {
            if let Some(n) = m.get(b"MCID").and_then(Object::as_int) {
                out.push(n)
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
    let Some(c) = page.get(b"Contents") else { return Vec::new() };
    match &*doc.resolve(c) {
        Object::Stream(s) => s.decoded().unwrap_or_default(),
        Object::Array(a) => a
            .iter()
            .flat_map(|x| match &*doc.resolve(x) {
                Object::Stream(s) => {
                    let mut v = s.decoded().unwrap_or_default();
                    v.push(b'\n');
                    v
                }
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn union(a: Option<[f64; 4]>, b: [f64; 4]) -> [f64; 4] {
    match a {
        Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
        None => b,
    }
}

/// Where the marked content `ids` is drawn on a page (images, forms and paths).
fn content_bbox(doc: &Document, page: &Dict, ids: &HashSet<i64>) -> Option<[f64; 4]> {
    let res = page.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let xobjects = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let props = page_properties(doc, page);
    let mut ctm = vec![Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])];
    let mut marks: Vec<bool> = Vec::new();
    let mut bbox = None;
    let mut path: Vec<(f64, f64)> = Vec::new();
    for op in pdfcraft_content::parse(&page_content(doc, page)).ops {
        let inside = marks.iter().any(|m| *m);
        let top = ctm.last().copied().unwrap_or(Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
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
            b"BMC" => marks.push(false),
            b"BDC" => marks.push(bdc_mcid(doc, &props, &op).is_some_and(|n| ids.contains(&n))),
            b"EMC" => {
                marks.pop();
            }
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
                        bbox = Some(union(bbox, [*x, *y, *x, *y]));
                    }
                }
                path.clear();
            }
            b"Do" if inside => {
                let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { continue };
                let unit = match &*doc.get(r) {
                    Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form") => {
                        let bb: Vec<f64> =
                            s.dict.get(b"BBox").and_then(|b| b.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default();
                        let m = Matrix::from_operands(s.dict.get(b"Matrix").and_then(|m| m.as_array()).map(Vec::as_slice).unwrap_or(&[]))
                            .unwrap_or(Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
                        match bb[..] {
                            [a, b, c, d] => m.then(&top).bbox([a, b, c, d]),
                            _ => continue,
                        }
                    }
                    _ => top.bbox([0.0, 0.0, 1.0, 1.0]),
                };
                bbox = Some(union(bbox, unit));
            }
            b"BI" if inside => bbox = Some(union(bbox, top.bbox([0.0, 0.0, 1.0, 1.0]))),
            _ => {}
        }
    }
    bbox
}

/// The figures, in document order.
pub fn figures(doc: &Document) -> Vec<Figure> {
    let Some(root) = tree_root(doc) else { return Vec::new() };
    let role_map = root.get(b"RoleMap").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let pages = pdfcraft_model::pages(doc);
    let index: HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    elements(doc)
        .into_iter()
        .filter(|e| e.dict.name(b"S").is_some_and(|s| standard(doc, &role_map, s) == b"Figure"))
        .map(|e| {
            let page = e.page.and_then(|p| index.get(&p).copied());
            let alt = e.dict.get(b"Alt").map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text()));
            let ids: HashSet<i64> = mcids(&e.dict).into_iter().collect();
            let bbox = page.and_then(|p| content_bbox(doc, &pages[p].dict, &ids));
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
fn content_entries(doc: &Document, page: &Dict) -> Option<Vec<Object>> {
    let c = page.get(b"Contents")?;
    Some(match &*doc.resolve(c) {
        Object::Array(a) => a.clone(),
        Object::Stream(_) => vec![c.clone()],
        _ => return None,
    })
}

fn marked(doc: &Document, props: &Dict, op: &pdfcraft_content::Op, ids: &HashSet<i64>) -> bool {
    op.op == b"BDC" && bdc_mcid(doc, props, op).is_some_and(|n| ids.contains(&n))
}

/// Turn the figure's marked-content blocks into artifacts, rewriting only the streams that
/// contain them. Other content streams, and unknown entries on a rewritten stream, stay.
fn artifact_marked_content(doc: &mut Document, page: &pdfcraft_model::Page, ids: &HashSet<i64>) -> Result<bool, AltError> {
    let Some(entries) = content_entries(doc, &page.dict) else { return Ok(false) };
    let props = page_properties(doc, &page.dict);
    let mut next = Vec::with_capacity(entries.len());
    let mut changed = false;
    for entry in entries {
        let Some(stream) = (match &*doc.resolve(&entry) {
            Object::Stream(s) => Some(s.clone()),
            _ => None,
        }) else {
            next.push(entry);
            continue;
        };
        let mut ops = pdfcraft_content::parse(&stream.decoded().unwrap_or_default()).ops;
        let mut ours = false;
        for op in &mut ops {
            if marked(doc, &props, op, ids) {
                *op = pdfcraft_content::Op::new("BMC", vec![Object::name("Artifact")]);
                ours = true;
            }
        }
        if !ours {
            next.push(entry);
            continue;
        }
        changed = true;
        let mut dict = stream.dict;
        dict.remove(b"Length");
        dict.remove(b"Filter");
        dict.remove(b"DecodeParms");
        let stream = Object::Stream(Stream::flate(dict, &pdfcraft_content::serialize_ops(&ops)));
        let stored = match entry {
            Object::Ref(r) => {
                doc.set(r, stream);
                Object::Ref(r)
            }
            _ => Object::Ref(doc.add(stream)),
        };
        next.push(stored);
    }
    if changed {
        let contents = match next.as_slice() {
            [one] => one.clone(),
            _ => Object::Array(next),
        };
        doc.update_dict(page.obj, |p| p.set(b"Contents".to_vec(), contents))?;
    }
    Ok(changed)
}

/// Decorative figure: its marked content on the page becomes an artifact, and the element
/// leaves the structure tree (and the parent tree).
pub fn mark_decorative(doc: &mut Document, obj: ObjRef) -> Result<(), AltError> {
    let d = check_figure(doc, obj)?;
    let ids: HashSet<i64> = mcids(&d).into_iter().collect();
    let pages = pdfcraft_model::pages(doc);
    let pg = d.get(b"Pg").and_then(Object::as_ref).or_else(|| elements(doc).into_iter().find(|e| e.object == obj).and_then(|e| e.page));
    if let Some(page) = pg.and_then(|p| pages.iter().find(|x| x.obj == p)).filter(|_| !ids.is_empty())
        && artifact_marked_content(doc, page, &ids)?
    {
        // The parent tree no longer points those ids at the figure.
        if let Some(key) = page.dict.get(b"StructParents").and_then(Object::as_int) {
            clear_parent_tree(doc, key, obj)?;
        }
    }
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
    let ids: HashSet<i64> = mcids(&element.dict).into_iter().collect();
    if ids.is_empty() {
        return Err(ArtifactError::NoMarkedContent);
    }
    let pages = pdfcraft_model::pages(doc);
    let page_ref = element.page.ok_or(ArtifactError::NoPage)?;
    let page = pages.iter().find(|page| page.obj == page_ref).ok_or(ArtifactError::NoPage)?;
    let mut ops = pdfcraft_content::parse(&page_content(doc, &page.dict)).ops;
    let mut found = HashSet::new();
    for op in &mut ops {
        let id = (op.op == b"BDC")
            .then(|| match op.operands.get(1) {
                Some(Object::Dict(mark)) => mark.get(b"MCID").and_then(Object::as_int),
                _ => None,
            })
            .flatten()
            .filter(|id| ids.contains(id));
        if let Some(id) = id {
            *op = pdfcraft_content::Op::new("BMC", vec![Object::name("Artifact")]);
            found.insert(id);
        }
    }
    if found != ids {
        return Err(ArtifactError::ContentNotFound);
    }
    let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), &pdfcraft_content::serialize_ops(&ops))));
    doc.update_dict(page.obj, |p| p.set(b"Contents".to_vec(), Object::Ref(stream)))?;
    if let Some(key) = page.dict.get(b"StructParents").and_then(Object::as_int) {
        clear_parent_tree(doc, key, obj).map_err(|e| ArtifactError::ParentTree(e.to_string()))?;
    }
    let parent = element.dict.get(b"P").and_then(Object::as_ref).ok_or(ArtifactError::NoParent)?;
    remove_from_parent(doc, parent, obj)?;
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

fn remove_from_parent(doc: &mut Document, parent: ObjRef, obj: ObjRef) -> Result<(), ArtifactError> {
    let kids = doc.get(parent).as_dict().and_then(|p| p.get(b"K").cloned()).ok_or(ArtifactError::NoParent)?;
    let replacement = match kids {
        Object::Array(items) => {
            if items.iter().filter(|item| item.as_ref() == Some(obj)).count() != 1 {
                return Err(ArtifactError::AmbiguousParent);
            }
            Object::Array(items.into_iter().filter(|item| item.as_ref() != Some(obj)).collect())
        }
        Object::Ref(r) if r == obj => Object::Array(Vec::new()),
        _ => return Err(ArtifactError::NoParent),
    };
    doc.update_dict(parent, |p| p.set(b"K".to_vec(), replacement))?;
    Ok(())
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
