//! PDF/A-3 6.8: every embedded file is an associated file, its file specification listed in the
//! `/AF` array of the document's catalog or of one of its parts (ISO 32000-2 14.13): a page, a
//! structure element, an XObject, a DPart, an annotation, a metadata stream, or a sequence of
//! content. That last is a marked-content sequence `BDC` begins with the tag `AF` and the name of a
//! property list in a `/Properties` resource dictionary, whose value is then itself the array of
//! file specifications (14.13.5).
//!
//! The arrays are read in every indirect object and the dictionaries and arrays it holds directly,
//! as deep as [`MAX_DEPTH`]. An `/AF` given by reference is followed to its array. A property list
//! counts when some content stream names it, by that name, with the tag `AF`.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How deep the direct dictionaries and arrays of an object are read.
const MAX_DEPTH: u8 = 8;
/// The decoded content, across the document's content streams, read for `AF` sequences.
const MAX_CONTENT_BYTES: usize = 256 << 20;
/// How many property list names tagged `AF` are kept.
const MAX_TAGGED: usize = 4096;

/// The file specifications `objs` list as associated files. `content` are the content streams
/// only a reference identifies, read for the property lists `AF` sequences name.
pub(crate) fn associated(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> HashSet<ObjRef> {
    let reader = Reader { doc, tagged: tagged(doc, objs, content) };
    let mut out = HashSet::new();
    for (_, o) in objs {
        match &**o {
            Object::Dict(d) => reader.dict(d, 0, &mut out),
            Object::Stream(s) => reader.dict(&s.dict, 0, &mut out),
            Object::Array(a) => reader.array(a, 0, &mut out),
            _ => {}
        }
    }
    out
}

/// The names of the property lists marked-content sequences tagged `AF` begin with: `/AF /Name
/// BDC` (14.13.5).
fn tagged(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> HashSet<Vec<u8>> {
    let mut out = HashSet::new();
    crate::contents::each(doc, objs, content, MAX_CONTENT_BYTES, &mut |data| {
        // Only content that begins a marked-content sequence with a property list is parsed. The tag
        // is looked for in the parsed name, which may be written with escapes (`/A#46`).
        if data.windows(3).any(|w| w == b"BDC") {
            for op in pdfcraft_content::ops(data) {
                if op.op == b"BDC"
                    && op.operands.first().and_then(Object::as_name) == Some(b"AF".as_slice())
                    && let Some(name) = op.operands.get(1).and_then(Object::as_name)
                    && out.len() < MAX_TAGGED
                {
                    out.insert(name.to_vec());
                }
            }
        }
        out.len() < MAX_TAGGED
    });
    out
}

struct Reader<'a> {
    doc: &'a Document,
    /// The property list names `AF` sequences use.
    tagged: HashSet<Vec<u8>>,
}

impl Reader<'_> {
    /// The references in an `/AF` value or a property list: an array, or a reference to one.
    fn listed(&self, value: &Object, out: &mut HashSet<ObjRef>) {
        let value = self.doc.resolve(value);
        if let Some(files) = value.as_array() {
            out.extend(files.iter().filter_map(Object::as_ref));
        }
    }

    fn dict(&self, d: &Dict, depth: u8, out: &mut HashSet<ObjRef>) {
        if depth > MAX_DEPTH {
            return;
        }
        if let Some(af) = d.get(b"AF") {
            self.listed(af, out);
        }
        if !self.tagged.is_empty()
            && let Some(properties) = d.get(b"Properties").map(|p| self.doc.resolve(p))
            && let Some(properties) = properties.as_dict()
        {
            for (name, list) in properties.iter() {
                if self.tagged.contains(name.as_slice()) {
                    self.listed(list, out);
                }
            }
        }
        for (_, v) in d.iter() {
            match v {
                Object::Dict(inner) => self.dict(inner, depth.saturating_add(1), out),
                Object::Array(a) => self.array(a, depth.saturating_add(1), out),
                _ => {}
            }
        }
    }

    fn array(&self, a: &[Object], depth: u8, out: &mut HashSet<ObjRef>) {
        if depth > MAX_DEPTH {
            return;
        }
        for v in a {
            match v {
                Object::Dict(inner) => self.dict(inner, depth.saturating_add(1), out),
                Object::Array(inner) => self.array(inner, depth.saturating_add(1), out),
                _ => {}
            }
        }
    }
}
