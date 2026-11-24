//! Create a valid, reviewable structure tree for untagged PDFs.
//!
//! Each text object becomes a paragraph, or a heading when its size or weight sets it apart from
//! the body text ([`crate::autotag_roles`]); either can be retagged with the existing tag-property
//! tool after human review. Painted paths, shadings and images outside text objects
//! are wrapped in `/Artifact BMC … EMC` so the resulting page passes the tagged-content check.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::{Op, Pieces, serialize_ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use crate::autotag_links::{self, Link};
use crate::autotag_roles::{self, Evidence};

const MAX_PAGES: usize = 1_000;
const MAX_PAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_OPERATORS: usize = 100_000;
/// What all pages together may decode and keep: pages can share one stream, so the per-page limits
/// alone let a small file retain a plan for every page. Real documents stay far under these.
const MAX_TOTAL_BYTES: usize = 128 * 1024 * 1024;
const MAX_TOTAL_OPERATORS: usize = 2_000_000;
const MAX_TEXT_OBJECTS: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoTagResult {
    pub pages: usize,
    pub paragraphs: usize,
    pub headings: usize,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AutoTagError {
    #[error("the document already has a structure tree; review its existing tags instead")]
    AlreadyTagged,
    #[error("the document has no text objects to tag")]
    NoText,
    #[error("the document has too many pages to auto-tag in one edit (maximum {MAX_PAGES})")]
    TooManyPages,
    #[error("page {page}: {reason}")]
    Unsupported { page: usize, reason: String },
    #[error("the PDF has no document catalog")]
    NoCatalog,
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

struct Plan {
    page: ObjRef,
    ops: Vec<Op>,
    spans: Vec<(usize, usize)>,
    evidence: Vec<Evidence>,
    artifacts: HashMap<usize, usize>,
    links: Vec<Link>,
    /// See [`autotag_links::Placed`].
    owners: HashMap<usize, Option<usize>>,
    descriptions: Vec<Option<String>>,
}

pub(crate) fn unsupported(page: usize, reason: impl Into<String>) -> AutoTagError {
    AutoTagError::Unsupported { page: page + 1, reason: reason.into() }
}

fn content_pieces(doc: &Document, page: &pdfcraft_model::Page, index: usize) -> Result<Pieces, AutoTagError> {
    let Some(contents) = page.dict.get(b"Contents") else { return Ok(Pieces::join::<&[u8]>(&[])) };
    let resolved = doc.resolve(contents);
    let items: Vec<Object> = match &*resolved {
        Object::Array(items) => items.clone(),
        Object::Stream(_) => vec![contents.clone()],
        _ => return Err(unsupported(index, "contents are not a stream or array of streams")),
    };
    let mut pieces = Vec::with_capacity(items.len());
    let mut total = 0usize;
    for item in items {
        let value = doc.resolve(&item);
        let Object::Stream(stream) = &*value else { return Err(unsupported(index, "contents include a non-stream object")) };
        let remaining = MAX_PAGE_BYTES.saturating_sub(total.saturating_add(1));
        if remaining == 0 {
            return Err(unsupported(index, "decoding the content takes more than 16 MiB"));
        }
        // Strictly, and with `/DecodeParms` given by reference: the content is written back from it.
        let mut budget = DecodeBudget::new(remaining, 0);
        let decoded = budget.decode_strict(doc, stream).map_err(|e| {
            // The allowance counts what every filter stage decodes, as a page's does.
            if budget.remaining() == 0 {
                unsupported(index, "decoding the content takes more than 16 MiB")
            } else {
                // The decoder's own reason: `CosError::Filter`'s text would say again that the data
                // couldn't be decoded.
                let why = match e {
                    pdfcraft_cos::CosError::Filter(why) => why,
                    other => other.to_string(),
                };
                unsupported(index, format!("content cannot be decoded strictly: {why}"))
            }
        })?;
        total = total.saturating_add(decoded.len()).saturating_add(1);
        pieces.push(decoded);
    }
    Ok(Pieces::join(&pieces))
}

fn page_xobjects(doc: &Document, page: &Dict) -> Dict {
    page.get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|r| r.get(b"XObject").map(|x| doc.resolve(x))))
        .and_then(|x| x.as_dict().cloned())
        .unwrap_or_default()
}

/// The page's resource dictionary of kind `kind` (`/Font`, `/ExtGState`), or an empty one.
fn page_resources(doc: &Document, page: &Dict, kind: &[u8]) -> Dict {
    page.get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|r| r.get(kind).map(|x| doc.resolve(x))))
        .and_then(|x| x.as_dict().cloned())
        .unwrap_or_default()
}

fn harmless_outside(op: &[u8]) -> bool {
    matches!(
        op,
        b"q" | b"Q"
            | b"cm"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"sc"
            | b"SCN"
            | b"scn"
            | b"m"
            | b"l"
            | b"c"
            | b"v"
            | b"y"
            | b"h"
            | b"re"
            | b"W"
            | b"W*"
            | b"n"
    )
}

fn painted_outside(doc: &Document, xobjects: &Dict, op: &Op) -> bool {
    match op.op.as_slice() {
        b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"sh" | b"BI" => true,
        b"Do" => op
            .name(0)
            .and_then(|n| xobjects.get(n))
            .map(|o| doc.resolve(o))
            .and_then(|o| match &*o {
                Object::Stream(s) => s.dict.name(b"Subtype").map(<[u8]>::to_vec),
                _ => None,
            })
            .is_some_and(|st| st == b"Image"),
        _ => false,
    }
}

fn harmless_inside(op: &[u8]) -> bool {
    matches!(
        op,
        b"Tf"
            | b"Tc"
            | b"Tw"
            | b"Tz"
            | b"TL"
            | b"Tr"
            | b"Ts"
            | b"Td"
            | b"TD"
            | b"Tm"
            | b"T*"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"sc"
            | b"SCN"
            | b"scn"
    )
}

/// `totals` is the decoded bytes and operators of the pages planned so far, bounded by
/// `MAX_TOTAL_BYTES` and `MAX_TOTAL_OPERATORS`.
fn plan_page(
    doc: &Document,
    page: &pdfcraft_model::Page,
    index: usize,
    totals: &mut (usize, usize),
    fonts: &mut autotag_roles::Fonts,
    seen: &mut HashSet<ObjRef>,
    allowance: &mut usize,
) -> Result<Plan, AutoTagError> {
    if page.dict.contains(b"StructParents") {
        return Err(unsupported(index, "page already has a structure parent key"));
    }
    let links = autotag_links::page_links(doc, &page.dict, index, seen)?;
    let pieces = content_pieces(doc, page, index)?;
    totals.0 = totals.0.saturating_add(pieces.data().len());
    let parsed = pieces.parse();
    if parsed.skipped > 0 || parsed.ambiguous_images > 0 || parsed.ops.len() > MAX_OPERATORS {
        return Err(unsupported(index, "content has malformed or too many operators"));
    }
    let (resources, states) = (page_resources(doc, &page.dict, b"Font"), page_resources(doc, &page.dict, b"ExtGState"));
    fonts.new_page();
    // Splitting text at a link's edge adds operators, so the operator limits count the split content.
    let placed = autotag_links::place(doc, fonts, &resources, &states, parsed.ops, &links, allowance)
        .ok_or_else(|| unsupported(index, "the document has too much text under too many links to place"))?;
    if placed.ops.len() > MAX_OPERATORS {
        return Err(unsupported(index, "splitting text at link edges takes too many operators"));
    }
    let ops = placed.ops;
    totals.1 = totals.1.saturating_add(ops.len());
    if totals.0 > MAX_TOTAL_BYTES || totals.1 > MAX_TOTAL_OPERATORS {
        return Err(unsupported(index, "the document has too much content to auto-tag in one edit"));
    }
    let xobjects = page_xobjects(doc, &page.dict);
    let (mut start, mut has_text, mut spans, mut artifacts) = (None, false, Vec::new(), HashMap::new());
    // Marked-content boundaries must enclose the whole path, not just its paint operator.
    let mut path = None;
    for (at, op) in ops.iter().enumerate() {
        if path.is_some() && matches!(op.op.as_slice(), b"BT" | b"Do" | b"BI" | b"sh") {
            return Err(unsupported(index, "painted content interrupts an unfinished path"));
        }
        match op.op.as_slice() {
            b"BT" if start.is_none() => (start, has_text) = (Some(at), false),
            b"ET" => {
                let Some(first) = start.take() else { return Err(unsupported(index, "unmatched ET operator")) };
                if has_text {
                    spans.push((first, at));
                    if spans.len() > MAX_TEXT_OBJECTS {
                        return Err(unsupported(index, "too many text objects (maximum 4096)"));
                    }
                }
            }
            b"Tj" | b"TJ" | b"'" | b"\"" if start.is_some() => has_text = true,
            name if start.is_some() && harmless_inside(name) => {}
            name if start.is_none() && harmless_outside(name) => match name {
                b"m" | b"re" => {
                    path.get_or_insert(at);
                }
                b"n" => path = None,
                _ => {}
            },
            _ if start.is_none() && painted_outside(doc, &xobjects, op) => {
                let first = if matches!(op.op.as_slice(), b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s") {
                    path.take().unwrap_or(at)
                } else {
                    at
                };
                artifacts.insert(first, at);
            }
            _ => return Err(unsupported(index, format!("unsupported or painted operator {}", String::from_utf8_lossy(&op.op)))),
        }
    }
    if start.is_some() {
        return Err(unsupported(index, "unterminated BT text object"));
    }
    if path.is_some() {
        return Err(unsupported(index, "unterminated path object"));
    }
    let evidence = autotag_roles::evidence(doc, fonts, &resources, &states, &ops, &spans);
    Ok(Plan { page: page.obj, ops, spans, evidence, artifacts, links, owners: placed.owners, descriptions: placed.descriptions })
}

fn plan(doc: &Document) -> Result<Vec<Plan>, AutoTagError> {
    let root = doc.root().ok_or(AutoTagError::NoCatalog)?;
    let catalog = doc.get(root);
    let catalog = catalog.as_dict().ok_or(AutoTagError::NoCatalog)?;
    if catalog.contains(b"StructTreeRoot") {
        return Err(AutoTagError::AlreadyTagged);
    }
    if catalog.contains(b"AcroForm") {
        return Err(unsupported(0, "form fields need their own tags; this text-only action cannot tag them"));
    }
    let pages = pdfcraft_model::pages(doc);
    if pages.len() > MAX_PAGES {
        return Err(AutoTagError::TooManyPages);
    }
    let mut out = Vec::with_capacity(pages.len());
    let mut total = 0usize;
    let mut totals = (0usize, 0usize);
    let mut fonts = autotag_roles::Fonts::default();
    let mut seen = HashSet::new();
    let mut allowance = autotag_links::MAX_AREA_TESTS;
    for (index, page) in pages.iter().enumerate() {
        let planned = plan_page(doc, page, index, &mut totals, &mut fonts, &mut seen, &mut allowance)?;
        total = total.saturating_add(planned.spans.len());
        out.push(planned);
    }
    if total == 0 {
        return Err(AutoTagError::NoText);
    }
    Ok(out)
}

fn mark_ops(
    ops: Vec<Op>,
    starts: &HashMap<usize, (usize, &'static str)>,
    ends: &HashSet<usize>,
    artifacts: &HashMap<usize, usize>,
) -> Result<Vec<Op>, AutoTagError> {
    let artifact_ends: HashSet<usize> = artifacts.values().copied().collect();
    let extra = starts.len().saturating_add(artifacts.len()).saturating_mul(2);
    let mut marked = Vec::with_capacity(ops.len().saturating_add(extra));
    for (at, op) in ops.into_iter().enumerate() {
        let is_artifact = artifacts.contains_key(&at);
        if let Some(&(mcid, role)) = starts.get(&at) {
            let mut properties = Dict::new();
            properties.set(b"MCID".to_vec(), Object::Int(i64::try_from(mcid).map_err(|_| AutoTagError::TooManyPages)?));
            marked.push(Op::new("BDC", vec![Object::name(role), Object::Dict(properties)]));
        } else if is_artifact {
            marked.push(Op::new("BMC", vec![Object::name("Artifact")]));
        }
        marked.push(op);
        if ends.contains(&at) || artifact_ends.contains(&at) {
            marked.push(Op::new("EMC", Vec::new()));
        }
    }
    Ok(marked)
}

fn write_structure(
    next: &mut Document,
    catalog: ObjRef,
    root: ObjRef,
    document: ObjRef,
    children: Vec<Object>,
    numbers: Vec<Object>,
    next_key: usize,
) -> Result<(), AutoTagError> {
    let mut tree = Dict::new();
    tree.set(b"Nums".to_vec(), Object::Array(numbers));
    let parent_tree = next.add(tree);
    let mut doc_element = Dict::new();
    doc_element.set(b"Type".to_vec(), Object::name("StructElem"));
    doc_element.set(b"S".to_vec(), Object::name("Document"));
    doc_element.set(b"P".to_vec(), Object::Ref(root));
    doc_element.set(b"K".to_vec(), Object::Array(children));
    next.set(document, doc_element);
    let mut structure = Dict::new();
    structure.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
    structure.set(b"K".to_vec(), Object::Ref(document));
    structure.set(b"ParentTree".to_vec(), Object::Ref(parent_tree));
    structure.set(b"ParentTreeNextKey".to_vec(), Object::Int(i64::try_from(next_key).map_err(|_| AutoTagError::TooManyPages)?));
    next.set(root, structure);
    let mark_info = next.get(catalog).as_dict().and_then(|d| d.get(b"MarkInfo")).map(|m| next.resolve(m)).and_then(|m| m.as_dict().cloned());
    let mut mark_info = mark_info.unwrap_or_default();
    mark_info.set(b"Marked".to_vec(), Object::Bool(true));
    next.update_dict(catalog, |d| {
        d.set(b"StructTreeRoot".to_vec(), Object::Ref(root));
        d.set(b"MarkInfo".to_vec(), Object::Dict(mark_info));
    })?;
    Ok(())
}

/// Tag every text object in an untagged PDF as a paragraph or a heading and mark painted paths, shadings and
/// images outside text objects as artifacts. Link annotations become Link tags with the text under
/// them ([`crate::autotag_links`]); any other annotation is refused. The full structure tree, parent
/// tree and marked-content IDs are written together. Unsupported operators are refused before any
/// mutation. On success the caller can use tag-property edits to correct the roles after review.
pub fn autotag_text(doc: &mut Document) -> Result<AutoTagResult, AutoTagError> {
    let plans = plan(doc)?;
    let all: Vec<Evidence> = plans.iter().flat_map(|p| p.evidence.iter().copied()).collect();
    let mut roles = autotag_roles::roles(&all).into_iter();
    let mut next = doc.clone();
    let catalog = next.root().ok_or(AutoTagError::NoCatalog)?;
    let (root, document) = (next.add(Object::Null), next.add(Object::Null));
    let (mut children, mut numbers, mut tagged_pages, mut paragraphs, mut headings) = (Vec::new(), Vec::new(), 0usize, 0usize, 0usize);
    // Each link annotation's Link element and the `/Contents` to give it, keyed after the pages.
    let mut annotations: Vec<(ObjRef, ObjRef, Option<String>)> = Vec::new();
    for page in plans {
        if page.spans.is_empty() && page.artifacts.is_empty() && page.links.is_empty() {
            continue;
        }
        let (mut starts, mut ends) = (HashMap::new(), HashSet::new());
        // Each link's element once its text is found, the element it sits in, and its MCIDs.
        let mut link_elements: Vec<Option<(ObjRef, ObjRef)>> = vec![None; page.links.len()];
        let mut link_kids: Vec<Vec<Object>> = vec![Vec::new(); page.links.len()];
        // The link the page's last marked-content sequence went to. A link's text joins its element
        // only while nothing else has come between, so the tree reads in the order the page shows
        // its text; text of a link after an interruption (another link inside it, or the rest of
        // a line) is its text object's own.
        let mut previous: Option<usize> = None;
        if !page.spans.is_empty() {
            let key = i64::try_from(tagged_pages).map_err(|_| AutoTagError::TooManyPages)?;
            let mut parents = Vec::with_capacity(page.spans.len());
            for &span in &page.spans {
                let role = roles.next().unwrap_or("P");
                // Made with its first kid: a text object whose only text continues an earlier
                // object's link gets no element of its own.
                let mut own: Option<ObjRef> = None;
                let mut kids = Vec::new();
                for (first, last, owner) in autotag_links::sequences(span, &page.owners) {
                    let mcid = parents.len();
                    let id = Object::Int(i64::try_from(mcid).map_err(|_| AutoTagError::TooManyPages)?);
                    let owner = owner.filter(|&l| link_elements.get(l).is_some_and(|e| e.is_none() || previous == Some(l)));
                    previous = owner;
                    let slot = owner.and_then(|l| link_elements.get_mut(l).zip(link_kids.get_mut(l)));
                    if let Some((element, link_mcids)) = slot {
                        // A link whose text runs on into a later text object stays in the first one.
                        let link = match *element {
                            Some((link, _)) => link,
                            None => {
                                let parent = *own.get_or_insert_with(|| next.add(Object::Null));
                                let link = next.add(Object::Null);
                                kids.push(Object::Ref(link));
                                *element = Some((link, parent));
                                link
                            }
                        };
                        link_mcids.push(id);
                        parents.push(Object::Ref(link));
                        starts.insert(first, (mcid, "Link"));
                    } else {
                        let reference = *own.get_or_insert_with(|| next.add(Object::Null));
                        kids.push(id);
                        parents.push(Object::Ref(reference));
                        starts.insert(first, (mcid, role));
                    }
                    ends.insert(last);
                }
                let Some(reference) = own else { continue };
                let mut element = Dict::new();
                element.set(b"Type".to_vec(), Object::name("StructElem"));
                element.set(b"S".to_vec(), Object::name(role));
                element.set(b"P".to_vec(), Object::Ref(document));
                element.set(b"Pg".to_vec(), Object::Ref(page.page));
                let only = if kids.len() == 1 { kids.first().filter(|k| matches!(k, Object::Int(_))).cloned() } else { None };
                element.set(b"K".to_vec(), only.unwrap_or(Object::Array(kids)));
                next.set(reference, element);
                children.push(Object::Ref(reference));
                if role == "P" {
                    paragraphs = paragraphs.saturating_add(1);
                } else {
                    headings = headings.saturating_add(1);
                }
            }
            next.update_dict(page.page, |d| d.set(b"StructParents".to_vec(), Object::Int(key)))?;
            numbers.extend([Object::Int(key), Object::Array(parents)]);
            tagged_pages += 1;
        }
        for ((link, placed), (mut kids, description)) in page.links.iter().zip(link_elements).zip(link_kids.into_iter().zip(page.descriptions)) {
            // A link over no text follows the page's text.
            let (element, parent) = placed.unwrap_or_else(|| {
                let element = next.add(Object::Null);
                children.push(Object::Ref(element));
                (element, document)
            });
            let mut objr = Dict::new();
            objr.set(b"Type".to_vec(), Object::name("OBJR"));
            objr.set(b"Pg".to_vec(), Object::Ref(page.page));
            objr.set(b"Obj".to_vec(), Object::Ref(link.annot));
            kids.push(Object::Dict(objr));
            let mut tag = Dict::new();
            tag.set(b"Type".to_vec(), Object::name("StructElem"));
            tag.set(b"S".to_vec(), Object::name("Link"));
            tag.set(b"P".to_vec(), Object::Ref(parent));
            tag.set(b"Pg".to_vec(), Object::Ref(page.page));
            tag.set(b"K".to_vec(), Object::Array(kids));
            next.set(element, tag);
            annotations.push((link.annot, element, description));
        }
        if !page.links.is_empty() {
            // Tab through the annotations in structure order (PDF/UA-1 §7.18.3).
            next.update_dict(page.page, |d| d.set(b"Tabs".to_vec(), Object::name("S")))?;
        }
        let marked = mark_ops(page.ops, &starts, &ends, &page.artifacts)?;
        let stream = next.add(Stream::flate(Dict::new(), &serialize_ops(&marked)));
        next.update_dict(page.page, |d| d.set(b"Contents".to_vec(), Object::Ref(stream)))?;
    }
    // Annotation keys follow the pages' keys, so the parent tree's numbers stay in order.
    let mut next_key = tagged_pages;
    for (annot, element, description) in annotations {
        let key = i64::try_from(next_key).map_err(|_| AutoTagError::TooManyPages)?;
        numbers.extend([Object::Int(key), Object::Ref(element)]);
        next.update_dict(annot, |d| {
            d.set(b"StructParent".to_vec(), Object::Int(key));
            if let Some(text) = description {
                d.set(b"Contents".to_vec(), Object::String(pdfcraft_cos::PdfString::text(&text)));
            }
        })?;
        next_key = next_key.saturating_add(1);
    }
    write_structure(&mut next, catalog, root, document, children, numbers, next_key)?;
    *doc = next;
    Ok(AutoTagResult { pages: tagged_pages, paragraphs, headings })
}
