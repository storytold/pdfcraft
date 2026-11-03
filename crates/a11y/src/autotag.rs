//! Create a valid, reviewable structure tree for PDFs whose painted content is text only.
//!
//! The writer refuses mixed content and existing tags. Each text object becomes a paragraph,
//! which can then be retagged with the existing tag-property tool after human review.

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

const MAX_PAGES: usize = 1_000;
const MAX_PAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_OPERATORS: usize = 100_000;
const MAX_TEXT_OBJECTS: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoTagResult {
    pub pages: usize,
    pub paragraphs: usize,
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
}

fn unsupported(page: usize, reason: impl Into<String>) -> AutoTagError {
    AutoTagError::Unsupported { page: page + 1, reason: reason.into() }
}

fn content_bytes(doc: &Document, page: &pdfcraft_model::Page, index: usize) -> Result<Vec<u8>, AutoTagError> {
    let Some(contents) = page.dict.get(b"Contents") else { return Ok(Vec::new()) };
    let resolved = doc.resolve(contents);
    let pieces: Vec<Object> = match &*resolved {
        Object::Array(items) => items.clone(),
        Object::Stream(_) => vec![contents.clone()],
        _ => return Err(unsupported(index, "contents are not a stream or array of streams")),
    };
    let mut all = Vec::new();
    for item in pieces {
        let value = doc.resolve(&item);
        let Object::Stream(stream) = &*value else { return Err(unsupported(index, "contents include a non-stream object")) };
        let remaining = MAX_PAGE_BYTES.saturating_sub(all.len().saturating_add(1));
        if remaining == 0 {
            return Err(unsupported(index, "decoded content exceeds 16 MiB"));
        }
        let chain = stream.filters();
        let decoded = if chain.is_empty() {
            if stream.raw.len() > remaining {
                return Err(unsupported(index, "decoded content exceeds 16 MiB"));
            }
            stream.raw.to_vec()
        } else {
            pdfcraft_filters::decode(&chain, &stream.raw, remaining)
                .map_err(|e| unsupported(index, format!("content cannot be decoded strictly: {e}")))?
        };
        all.extend_from_slice(&decoded);
        all.push(b' ');
    }
    Ok(all)
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
    )
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

/// The `(BT, ET)` operator index pairs of each text object that shows text, or why the page cannot be tagged.
fn text_spans(ops: &[Op], index: usize) -> Result<Vec<(usize, usize)>, AutoTagError> {
    let mut start = None;
    let mut has_text = false;
    let mut spans = Vec::new();
    for (at, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"BT" if start.is_none() => {
                start = Some(at);
                has_text = false;
            }
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
            name if start.is_none() && harmless_outside(name) => {}
            _ => return Err(unsupported(index, format!("unsupported or painted operator {}", String::from_utf8_lossy(&op.op)))),
        }
    }
    if start.is_some() {
        return Err(unsupported(index, "unterminated BT text object"));
    }
    Ok(spans)
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
    for (index, page) in pages.iter().enumerate() {
        if page.dict.contains(b"StructParents") {
            return Err(unsupported(index, "page already has a structure parent key"));
        }
        if page.dict.get(b"Annots").is_some_and(|a| match &*doc.resolve(a) {
            Object::Array(items) => !items.is_empty(),
            _ => true,
        }) {
            return Err(unsupported(index, "annotations need separate structure tags"));
        }
        let bytes = content_bytes(doc, page, index)?;
        let parsed = parse(&bytes);
        if parsed.skipped > 0 || parsed.ops.len() > MAX_OPERATORS {
            return Err(unsupported(index, "content has malformed or too many operators"));
        }
        let spans = text_spans(&parsed.ops, index)?;
        total = total.saturating_add(spans.len());
        out.push(Plan { page: page.obj, ops: parsed.ops, spans });
    }
    if total == 0 {
        return Err(AutoTagError::NoText);
    }
    Ok(out)
}

/// Tag every text object in an untagged, text-only PDF as a paragraph. The full structure tree,
/// parent tree and marked-content IDs are written together. Mixed content is refused before any
/// mutation. On success the caller can use tag-property edits to assign reviewed heading roles.
pub fn autotag_text(doc: &mut Document) -> Result<AutoTagResult, AutoTagError> {
    let plans = plan(doc)?;
    let mut next = doc.clone();
    let catalog = next.root().ok_or(AutoTagError::NoCatalog)?;
    let root = next.add(Object::Null);
    let document = next.add(Object::Null);
    let mut children = Vec::new();
    let mut numbers = Vec::new();
    let mut tagged_pages = 0usize;
    let mut paragraphs = 0usize;
    for page in plans {
        if page.spans.is_empty() {
            continue;
        }
        let key = i64::try_from(tagged_pages).map_err(|_| AutoTagError::TooManyPages)?;
        let mut parents = Vec::with_capacity(page.spans.len());
        let mut starts = std::collections::HashMap::new();
        let mut ends = std::collections::HashSet::new();
        for (mcid, (first, last)) in page.spans.iter().enumerate() {
            let mut element = Dict::new();
            element.set(b"Type".to_vec(), Object::name("StructElem"));
            element.set(b"S".to_vec(), Object::name("P"));
            element.set(b"P".to_vec(), Object::Ref(document));
            element.set(b"Pg".to_vec(), Object::Ref(page.page));
            element.set(b"K".to_vec(), Object::Int(i64::try_from(mcid).map_err(|_| AutoTagError::TooManyPages)?));
            let reference = next.add(element);
            parents.push(Object::Ref(reference));
            children.push(Object::Ref(reference));
            starts.insert(*first, mcid);
            ends.insert(*last);
            paragraphs = paragraphs.saturating_add(1);
        }
        let mut marked = Vec::with_capacity(page.ops.len().saturating_add(page.spans.len().saturating_mul(2)));
        for (at, op) in page.ops.into_iter().enumerate() {
            if let Some(mcid) = starts.get(&at) {
                let mut properties = Dict::new();
                properties.set(b"MCID".to_vec(), Object::Int(i64::try_from(*mcid).map_err(|_| AutoTagError::TooManyPages)?));
                marked.push(Op::new("BDC", vec![Object::name("P"), Object::Dict(properties)]));
            }
            marked.push(op);
            if ends.contains(&at) {
                marked.push(Op::new("EMC", Vec::new()));
            }
        }
        let stream = next.add(Stream::flate(Dict::new(), &serialize_ops(&marked)));
        next.update_dict(page.page, |d| {
            d.set(b"Contents".to_vec(), Object::Ref(stream));
            d.set(b"StructParents".to_vec(), Object::Int(key));
        })?;
        numbers.push(Object::Int(key));
        numbers.push(Object::Array(parents));
        tagged_pages += 1;
    }
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
    *doc = next;
    Ok(AutoTagResult { pages: tagged_pages, paragraphs })
}
