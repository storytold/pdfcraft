//! PDF/A-3 6.8: every embedded file is an associated file, its file specification listed in the
//! `/AF` array of the document's catalog or of one of its parts (ISO 32000-2 14.13): a page, a
//! structure element, an XObject, a DPart, an annotation, a metadata stream, or the property list
//! a marked-content sequence tagged `AF` names in a `/Properties` resource dictionary (14.13.5),
//! whose value is itself the array of file specifications.
//!
//! The arrays are read in every indirect object and the dictionaries and arrays it holds directly,
//! as deep as [`MAX_DEPTH`]. An `/AF` given by reference is followed to its array.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How deep the direct dictionaries and arrays of an object are read.
const MAX_DEPTH: u8 = 8;

/// The file specifications `objs` list as associated files.
pub(crate) fn associated(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> HashSet<ObjRef> {
    let mut out = HashSet::new();
    for (_, o) in objs {
        match &**o {
            Object::Dict(d) => dict(doc, d, 0, &mut out),
            Object::Stream(s) => dict(doc, &s.dict, 0, &mut out),
            Object::Array(a) => array(doc, a, 0, &mut out),
            _ => {}
        }
    }
    out
}

/// The references in an `/AF` value: an array, or a reference to one.
fn listed(doc: &Document, value: &Object, out: &mut HashSet<ObjRef>) {
    let value = doc.resolve(value);
    if let Some(files) = value.as_array() {
        out.extend(files.iter().filter_map(Object::as_ref));
    }
}

fn dict(doc: &Document, d: &Dict, depth: u8, out: &mut HashSet<ObjRef>) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(af) = d.get(b"AF") {
        listed(doc, af, out);
    }
    if let Some(properties) = d.get(b"Properties").map(|p| doc.resolve(p))
        && let Some(properties) = properties.as_dict()
    {
        for (_, list) in properties.iter() {
            listed(doc, list, out);
        }
    }
    for (_, v) in d.iter() {
        match v {
            Object::Dict(inner) => dict(doc, inner, depth.saturating_add(1), out),
            Object::Array(a) => array(doc, a, depth.saturating_add(1), out),
            _ => {}
        }
    }
}

fn array(doc: &Document, a: &[Object], depth: u8, out: &mut HashSet<ObjRef>) {
    if depth > MAX_DEPTH {
        return;
    }
    for v in a {
        match v {
            Object::Dict(inner) => dict(doc, inner, depth.saturating_add(1), out),
            Object::Array(inner) => array(doc, inner, depth.saturating_add(1), out),
            _ => {}
        }
    }
}
