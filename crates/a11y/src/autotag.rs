//! Create a valid, reviewable structure tree for untagged PDFs.
//!
//! Each text object becomes a paragraph, or a heading when its size or weight sets it apart from
//! the body text ([`crate::autotag_roles`]); either can be retagged with the existing tag-property
//! tool after human review. Running headers, footers and page numbers, found by repetition across
//! the pages ([`crate::autotag_running`]), are wrapped in
//! `/Artifact <</Type /Pagination /Subtype /Header>> BDC … EMC` (or `/Footer`) instead of being
//! tagged. Painted paths, shadings and images outside text objects
//! are wrapped in `/Artifact BMC … EMC` so the resulting page passes the tagged-content check.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::{Op, Pieces, serialize_ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use crate::autotag_roles::{self, Evidence};
use crate::autotag_running::{self, Edge, Frame, PageText};

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
    /// Text objects marked as running header artifacts.
    pub headers: usize,
    /// Text objects marked as running footer artifacts, page numbers included.
    pub footers: usize,
}

/// A text object auto-tag marks as a running header or footer instead of tagging it.
#[derive(Clone, Debug, PartialEq)]
pub struct RunningText {
    /// 0-based page index.
    pub page: usize,
    /// `"Header"` or `"Footer"`, the artifact subtype written.
    pub subtype: &'static str,
    /// The text shown, at most 256 characters.
    pub text: String,
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
    frame: Frame,
}

/// What a text object's span is wrapped in.
#[derive(Clone, Copy)]
enum Mark {
    Tag(usize, &'static str),
    Running(Edge),
}

fn unsupported(page: usize, reason: impl Into<String>) -> AutoTagError {
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

fn page_fonts(doc: &Document, page: &Dict) -> Dict {
    page.get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|r| r.get(b"Font").map(|x| doc.resolve(x))))
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
) -> Result<Plan, AutoTagError> {
    if page.dict.contains(b"StructParents") {
        return Err(unsupported(index, "page already has a structure parent key"));
    }
    if page.dict.get(b"Annots").is_some_and(|a| match &*doc.resolve(a) {
        Object::Array(items) => !items.is_empty(),
        _ => true,
    }) {
        return Err(unsupported(index, "annotations need separate structure tags"));
    }
    let pieces = content_pieces(doc, page, index)?;
    totals.0 = totals.0.saturating_add(pieces.data().len());
    let parsed = pieces.parse();
    if parsed.skipped > 0 || parsed.ambiguous_images > 0 || parsed.ops.len() > MAX_OPERATORS {
        return Err(unsupported(index, "content has malformed or too many operators"));
    }
    totals.1 = totals.1.saturating_add(parsed.ops.len());
    if totals.0 > MAX_TOTAL_BYTES || totals.1 > MAX_TOTAL_OPERATORS {
        return Err(unsupported(index, "the document has too much content to auto-tag in one edit"));
    }
    let xobjects = page_xobjects(doc, &page.dict);
    let (mut start, mut has_text, mut spans, mut artifacts) = (None, false, Vec::new(), HashMap::new());
    // Marked-content boundaries must enclose the whole path, not just its paint operator.
    let mut path = None;
    for (at, op) in parsed.ops.iter().enumerate() {
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
    let resources = page_fonts(doc, &page.dict);
    let evidence = autotag_roles::evidence(doc, fonts, &resources, &parsed.ops, &spans);
    let frame = Frame { rotation: page.rotation(doc), crop: page.crop(doc) };
    Ok(Plan { page: page.obj, ops: parsed.ops, spans, evidence, artifacts, frame })
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
    for (index, page) in pages.iter().enumerate() {
        let planned = plan_page(doc, page, index, &mut totals, &mut fonts)?;
        total = total.saturating_add(planned.spans.len());
        out.push(planned);
    }
    if total == 0 {
        return Err(AutoTagError::NoText);
    }
    Ok(out)
}

fn mark_ops(ops: Vec<Op>, starts: &HashMap<usize, Mark>, ends: &HashSet<usize>, artifacts: &HashMap<usize, usize>) -> Result<Vec<Op>, AutoTagError> {
    let artifact_ends: HashSet<usize> = artifacts.values().copied().collect();
    let extra = starts.len().saturating_add(artifacts.len()).saturating_mul(2);
    let mut marked = Vec::with_capacity(ops.len().saturating_add(extra));
    for (at, op) in ops.into_iter().enumerate() {
        let is_artifact = artifacts.contains_key(&at);
        if let Some(&mark) = starts.get(&at) {
            let mut properties = Dict::new();
            let tag = match mark {
                Mark::Tag(mcid, role) => {
                    properties.set(b"MCID".to_vec(), Object::Int(i64::try_from(mcid).map_err(|_| AutoTagError::TooManyPages)?));
                    role
                }
                Mark::Running(edge) => {
                    properties.set(b"Type".to_vec(), Object::name("Pagination"));
                    properties.set(b"Subtype".to_vec(), Object::name(edge.subtype()));
                    "Artifact"
                }
            };
            marked.push(Op::new("BDC", vec![Object::name(tag), Object::Dict(properties)]));
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
    tagged_pages: usize,
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
    structure.set(b"ParentTreeNextKey".to_vec(), Object::Int(i64::try_from(tagged_pages).map_err(|_| AutoTagError::TooManyPages)?));
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

/// The edge each planned text object repeats at, for running text, per page and span.
fn running(plans: &[Plan]) -> Vec<Vec<Option<Edge>>> {
    let pages: Vec<PageText<'_>> = plans.iter().map(|p| PageText { frame: p.frame, objects: &p.evidence }).collect();
    autotag_running::find(&pages, autotag_roles::body_size(plans.iter().flat_map(|p| &p.evidence)))
}

/// The running headers, footers and page numbers [`autotag_text`] would mark as artifacts, in page
/// order. Refuses what `autotag_text` refuses, and changes nothing.
pub fn running_text(doc: &Document) -> Result<Vec<RunningText>, AutoTagError> {
    let plans = plan(doc)?;
    let mut out = Vec::new();
    for (page, (plan, edges)) in plans.iter().zip(running(&plans)).enumerate() {
        for (evidence, edge) in plan.evidence.iter().zip(edges) {
            if let Some(edge) = edge {
                out.push(RunningText { page, subtype: edge.subtype(), text: evidence.text.clone() });
            }
        }
    }
    Ok(out)
}

/// What [`autotag_text`] has written so far.
struct Written {
    children: Vec<Object>,
    numbers: Vec<Object>,
    result: AutoTagResult,
}

/// Tag or mark one page's text objects, and write its marked content.
fn tag_page(
    next: &mut Document,
    document: ObjRef,
    page: Plan,
    edges: &[Option<Edge>],
    roles: &mut impl Iterator<Item = &'static str>,
    written: &mut Written,
) -> Result<(), AutoTagError> {
    let (mut starts, mut ends, mut parents) = (HashMap::new(), HashSet::new(), Vec::new());
    let key = i64::try_from(written.result.pages).map_err(|_| AutoTagError::TooManyPages)?;
    for (at, &(first, last)) in page.spans.iter().enumerate() {
        ends.insert(last);
        if let Some(edge) = edges.get(at).copied().flatten() {
            starts.insert(first, Mark::Running(edge));
            let count = if edge == Edge::Top { &mut written.result.headers } else { &mut written.result.footers };
            *count = count.saturating_add(1);
            continue;
        }
        let role = roles.next().unwrap_or("P");
        let mcid = parents.len();
        let mut element = Dict::new();
        element.set(b"Type".to_vec(), Object::name("StructElem"));
        element.set(b"S".to_vec(), Object::name(role));
        element.set(b"P".to_vec(), Object::Ref(document));
        element.set(b"Pg".to_vec(), Object::Ref(page.page));
        element.set(b"K".to_vec(), Object::Int(i64::try_from(mcid).map_err(|_| AutoTagError::TooManyPages)?));
        let reference = next.add(element);
        parents.push(Object::Ref(reference));
        written.children.push(Object::Ref(reference));
        starts.insert(first, Mark::Tag(mcid, role));
        let count = if role == "P" { &mut written.result.paragraphs } else { &mut written.result.headings };
        *count = count.saturating_add(1);
    }
    if !parents.is_empty() {
        next.update_dict(page.page, |d| d.set(b"StructParents".to_vec(), Object::Int(key)))?;
        written.numbers.extend([Object::Int(key), Object::Array(parents)]);
        written.result.pages = written.result.pages.saturating_add(1);
    }
    let marked = mark_ops(page.ops, &starts, &ends, &page.artifacts)?;
    let stream = next.add(Stream::flate(Dict::new(), &serialize_ops(&marked)));
    next.update_dict(page.page, |d| d.set(b"Contents".to_vec(), Object::Ref(stream)))?;
    Ok(())
}

/// Tag every text object in an untagged PDF as a paragraph or a heading, mark running headers,
/// footers and page numbers as pagination artifacts, and mark painted paths, shadings and
/// images outside text objects as artifacts. The full structure tree, parent tree and
/// marked-content IDs are written together. Unsupported operators are refused before any
/// mutation. On success the caller can use tag-property edits to correct the roles after review.
pub fn autotag_text(doc: &mut Document) -> Result<AutoTagResult, AutoTagError> {
    let plans = plan(doc)?;
    let running = running(&plans);
    // Running text is left out of the body style and the heading levels, as it is out of the tags.
    let tagged: Vec<Evidence> = plans
        .iter()
        .zip(&running)
        .flat_map(|(p, edges)| p.evidence.iter().zip(edges).filter(|(_, edge)| edge.is_none()).map(|(e, _)| e.clone()))
        .collect();
    let mut roles = autotag_roles::roles(&tagged).into_iter();
    let mut next = doc.clone();
    let catalog = next.root().ok_or(AutoTagError::NoCatalog)?;
    let (root, document) = (next.add(Object::Null), next.add(Object::Null));
    let result = AutoTagResult { pages: 0, paragraphs: 0, headings: 0, headers: 0, footers: 0 };
    let mut written = Written { children: Vec::new(), numbers: Vec::new(), result };
    for (page, edges) in plans.into_iter().zip(running) {
        if page.spans.is_empty() && page.artifacts.is_empty() {
            continue;
        }
        tag_page(&mut next, document, page, &edges, &mut roles, &mut written)?;
    }
    let Written { children, numbers, result } = written;
    write_structure(&mut next, catalog, root, document, children, numbers, result.pages)?;
    *doc = next;
    Ok(result)
}
