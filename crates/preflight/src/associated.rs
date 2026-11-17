//! PDF/A-3 6.8: every embedded file is an associated file, its file specification listed in the
//! `/AF` array of the document's catalog or of one of its parts (ISO 32000-2 14.13): a page, a
//! structure element, an XObject, a DPart, an annotation, a metadata stream, or a sequence of
//! content. That last is a marked-content sequence `BDC` begins with the tag `AF` and the name of a
//! property list in the `/Properties` of the content's resource dictionary, whose value is then
//! itself the array of file specifications (14.13.5).
//!
//! The `/AF` arrays are read in every indirect object and the dictionaries and arrays it holds
//! directly, as deep as [`MAX_DEPTH`]; an `/AF` given by reference is followed to its array. A
//! property list counts when the content its resource dictionary serves names it with the tag
//! `AF`: a page's `/Contents`, joined as one, with the page's resources, inherited or its own; and
//! a form XObject, pattern, appearance or Type 3 glyph procedure with its own. Content without
//! resources of its own takes them from where it is drawn, which isn't followed here: its names
//! are looked up in every property list of the document, so that none of them is missed. Only
//! content whose property lists hold a file not yet associated is read.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};

/// How deep the direct dictionaries and arrays of an object are read.
const MAX_DEPTH: u8 = 8;
/// The decoded content, across the document's content streams, read for `AF` sequences.
const MAX_CONTENT_BYTES: usize = 256 << 20;

/// Property lists that are arrays of file specifications, by name.
type Lists = HashMap<Vec<u8>, HashSet<ObjRef>>;

/// The file specifications `objs` list as associated files. `content` are the content streams
/// only a reference identifies (Type 3 glyph procedures).
pub(crate) fn associated(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> HashSet<ObjRef> {
    let mut associated = HashSet::new();
    let mut everywhere = Lists::new();
    for (_, o) in objs {
        match &**o {
            Object::Dict(d) => walk_dict(doc, d, 0, &mut associated, &mut everywhere),
            Object::Stream(s) => walk_dict(doc, &s.dict, 0, &mut associated, &mut everywhere),
            Object::Array(a) => walk_array(doc, a, 0, &mut associated, &mut everywhere),
            _ => {}
        }
    }
    everywhere.retain(|_, files| files.iter().any(|f| !associated.contains(f)));
    if everywhere.is_empty() {
        return associated;
    }
    let mut budget = DecodeBudget::new(MAX_CONTENT_BYTES, 0);
    let mut read: HashSet<ObjRef> = HashSet::new();
    for page in pdfcraft_model::pages(doc) {
        let refs = crate::limits::page_contents(doc, &page.dict);
        let lists = own_lists(doc, &page.dict, &associated);
        if !lists.is_empty() {
            let mut joined = Vec::new();
            for r in &refs {
                if let Object::Stream(s) = &*doc.get(*r)
                    && let Ok(data) = budget.decode(s)
                {
                    joined.extend_from_slice(&data);
                    joined.push(b' ');
                }
            }
            tagged(&joined, &lists, &mut associated);
        }
        read.extend(refs);
    }
    for (r, o) in objs {
        if budget.remaining() == 0 {
            break;
        }
        let Object::Stream(s) = &**o else { continue };
        if read.contains(r) || !(content.contains(r) || crate::is_content(&s.dict)) {
            continue;
        }
        // Content that names no resources of its own: every list of the document may be the one.
        let lists = if s.dict.get(b"Resources").is_some() { own_lists(doc, &s.dict, &associated) } else { everywhere.clone() };
        if !lists.is_empty()
            && let Ok(data) = budget.decode(s)
        {
            tagged(&data, &lists, &mut associated);
        }
    }
    associated
}

/// The property lists of the resource dictionary of `holder` (a page or a stream's dictionary)
/// that hold a file not in `associated`.
fn own_lists(doc: &Document, holder: &Dict, associated: &HashSet<ObjRef>) -> Lists {
    let mut out = Lists::new();
    let resources = holder.get(b"Resources").map(|r| doc.resolve(r));
    if let Some(properties) = resources.as_deref().and_then(Object::as_dict).and_then(|r| r.get(b"Properties")).map(|p| doc.resolve(p))
        && let Some(properties) = properties.as_dict()
    {
        for (name, list) in properties.iter() {
            let files = listed(doc, list);
            if files.iter().any(|f| !associated.contains(f)) {
                out.entry(name.clone()).or_default().extend(files);
            }
        }
    }
    out
}

/// Associates the files of the lists in `lists` that the content `data` begins an `AF` sequence
/// with. Only content holding `BDC` is parsed; the tag is compared as parsed, so `/A#46` is `AF`.
fn tagged(data: &[u8], lists: &Lists, associated: &mut HashSet<ObjRef>) {
    if !data.windows(3).any(|w| w == b"BDC") {
        return;
    }
    for op in pdfcraft_content::ops(data) {
        if op.op == b"BDC"
            && op.operands.first().and_then(Object::as_name) == Some(b"AF".as_slice())
            && let Some(files) = op.operands.get(1).and_then(Object::as_name).and_then(|name| lists.get(name))
        {
            associated.extend(files.iter().copied());
        }
    }
}

/// The references in an `/AF` value or a property list: an array, or a reference to one.
fn listed(doc: &Document, value: &Object) -> Vec<ObjRef> {
    let value = doc.resolve(value);
    value.as_array().map(|files| files.iter().filter_map(Object::as_ref).collect()).unwrap_or_default()
}

/// The `/AF` arrays of `d` and what it holds directly, into `associated`, and its `/Properties`
/// lists into `everywhere`.
fn walk_dict(doc: &Document, d: &Dict, depth: u8, associated: &mut HashSet<ObjRef>, everywhere: &mut Lists) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(af) = d.get(b"AF") {
        associated.extend(listed(doc, af));
    }
    if let Some(properties) = d.get(b"Properties").map(|p| doc.resolve(p))
        && let Some(properties) = properties.as_dict()
    {
        for (name, list) in properties.iter() {
            let files = listed(doc, list);
            if !files.is_empty() {
                everywhere.entry(name.clone()).or_default().extend(files);
            }
        }
    }
    for (_, v) in d.iter() {
        match v {
            Object::Dict(inner) => walk_dict(doc, inner, depth.saturating_add(1), associated, everywhere),
            Object::Array(a) => walk_array(doc, a, depth.saturating_add(1), associated, everywhere),
            _ => {}
        }
    }
}

fn walk_array(doc: &Document, a: &[Object], depth: u8, associated: &mut HashSet<ObjRef>, everywhere: &mut Lists) {
    if depth > MAX_DEPTH {
        return;
    }
    for v in a {
        match v {
            Object::Dict(inner) => walk_dict(doc, inner, depth.saturating_add(1), associated, everywhere),
            Object::Array(inner) => walk_array(doc, inner, depth.saturating_add(1), associated, everywhere),
            _ => {}
        }
    }
}
