//! PDF/A-3 6.8: every embedded file is an associated file, its file specification listed in the
//! `/AF` array of the document's catalog or of one of its parts (ISO 32000-2 14.13): a page, a
//! structure element, an XObject, a DPart, an annotation, a metadata stream, or a sequence of
//! content. That last is a marked-content sequence `BDC` begins with the tag `AF` and the name of a
//! property list in a `/Properties` resource dictionary, whose value is then itself the array of
//! file specifications (14.13.5).
//!
//! The arrays are read in every indirect object and the dictionaries and arrays it holds directly,
//! as deep as [`MAX_DEPTH`]. An `/AF` given by reference is followed to its array. A property list
//! that is an array counts when some content stream names it, by that name, with the tag `AF`:
//! the content is read for those names only, and stops being read once each has been found.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How deep the direct dictionaries and arrays of an object are read.
const MAX_DEPTH: u8 = 8;
/// The decoded content, across the document's content streams, read for `AF` sequences.
const MAX_CONTENT_BYTES: usize = 256 << 20;

/// The file specifications `objs` list as associated files. `content` are the content streams
/// only a reference identifies, read for the property lists `AF` sequences name.
pub(crate) fn associated(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> HashSet<ObjRef> {
    let reader = Reader { doc };
    let mut found = Found::default();
    for (_, o) in objs {
        match &**o {
            Object::Dict(d) => reader.dict(d, 0, &mut found),
            Object::Stream(s) => reader.dict(&s.dict, 0, &mut found),
            Object::Array(a) => reader.array(a, 0, &mut found),
            _ => {}
        }
    }
    let Found { mut associated, mut lists } = found;
    // The lists whose files are all associated already needn't be looked for.
    lists.retain(|_, files| files.iter().any(|f| !associated.contains(f)));
    if !lists.is_empty() {
        crate::contents::each(doc, objs, content, MAX_CONTENT_BYTES, &mut |data| {
            // Only content that begins a marked-content sequence with a property list is parsed. The
            // tag is looked for in the parsed name, which may be written with escapes (`/A#46`).
            if data.windows(3).any(|w| w == b"BDC") {
                for op in pdfcraft_content::ops(data) {
                    if op.op == b"BDC"
                        && op.operands.first().and_then(Object::as_name) == Some(b"AF".as_slice())
                        && let Some(name) = op.operands.get(1).and_then(Object::as_name)
                        && let Some(files) = lists.remove(name)
                    {
                        associated.extend(files);
                    }
                }
            }
            !lists.is_empty()
        });
    }
    associated
}

/// What the objects say: the files their `/AF` arrays list, and the property lists that are
/// arrays of file specifications, by name.
#[derive(Default)]
struct Found {
    associated: HashSet<ObjRef>,
    lists: HashMap<Vec<u8>, HashSet<ObjRef>>,
}

struct Reader<'a> {
    doc: &'a Document,
}

impl Reader<'_> {
    /// The references in an `/AF` value or a property list: an array, or a reference to one.
    fn listed(&self, value: &Object) -> Vec<ObjRef> {
        let value = self.doc.resolve(value);
        value.as_array().map(|files| files.iter().filter_map(Object::as_ref).collect()).unwrap_or_default()
    }

    fn dict(&self, d: &Dict, depth: u8, found: &mut Found) {
        if depth > MAX_DEPTH {
            return;
        }
        if let Some(af) = d.get(b"AF") {
            found.associated.extend(self.listed(af));
        }
        if let Some(properties) = d.get(b"Properties").map(|p| self.doc.resolve(p))
            && let Some(properties) = properties.as_dict()
        {
            for (name, list) in properties.iter() {
                let files = self.listed(list);
                if !files.is_empty() {
                    found.lists.entry(name.clone()).or_default().extend(files);
                }
            }
        }
        for (_, v) in d.iter() {
            match v {
                Object::Dict(inner) => self.dict(inner, depth.saturating_add(1), found),
                Object::Array(a) => self.array(a, depth.saturating_add(1), found),
                _ => {}
            }
        }
    }

    fn array(&self, a: &[Object], depth: u8, found: &mut Found) {
        if depth > MAX_DEPTH {
            return;
        }
        for v in a {
            match v {
                Object::Dict(inner) => self.dict(inner, depth.saturating_add(1), found),
                Object::Array(inner) => self.array(inner, depth.saturating_add(1), found),
                _ => {}
            }
        }
    }
}
