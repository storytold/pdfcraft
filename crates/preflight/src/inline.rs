//! PDF/A-2 and -3 6.1.10: an inline image's filters (its `F` or `Filter` entry, ISO 32000-2 8.9.7,
//! Table 91) are the standard filters of ISO 32000-1 7.4.1, Table 6, or the abbreviations an
//! inline image may use for them (Table 94: `AHx`, `A85`, `Fl`, `RL`, `CCF`, `DCT`), and none is LZW
//! (`LZWDecode`, `LZW`) or `Crypt`. A single name and an array of names are read alike.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// The decoded content, across the document's content streams, the scan reads.
const MAX_CONTENT_BYTES: usize = 256 << 20;
/// How many filter names are kept, and how many bytes of each.
const MAX_NAMES: usize = 8;
const MAX_NAME_BYTES: usize = 64;

/// The filters of ISO 32000-1 Table 6 that 6.1.10 allows, and the abbreviations of Table 94.
const ALLOWED: &[&[u8]] = &[
    b"ASCIIHexDecode",
    b"ASCII85Decode",
    b"FlateDecode",
    b"RunLengthDecode",
    b"CCITTFaxDecode",
    b"JBIG2Decode",
    b"DCTDecode",
    b"JPXDecode",
    b"AHx",
    b"A85",
    b"Fl",
    b"RL",
    b"CCF",
    b"DCT",
];

/// The filters an inline image dictionary names that 6.1.10 doesn't allow.
fn forbidden(image: &Dict, out: &mut Vec<String>) {
    let names: Vec<&Object> = match image.get(b"F").or_else(|| image.get(b"Filter")) {
        Some(Object::Array(a)) => a.iter().collect(),
        Some(other) => vec![other],
        None => Vec::new(),
    };
    for name in names {
        if out.len() >= MAX_NAMES {
            return;
        }
        let Some(name) = name.as_name() else { continue };
        if ALLOWED.contains(&name) {
            continue;
        }
        let shown = String::from_utf8_lossy(name.get(..MAX_NAME_BYTES).unwrap_or(name)).into_owned();
        if !out.contains(&shown) {
            out.push(shown);
        }
    }
}

/// The filters the inline images of `doc` name that 6.1.10 doesn't allow, each once, at most
/// [`MAX_NAMES`]. `objs` are its objects and `content` the content streams only a reference
/// identifies.
pub(crate) fn forbidden_filters(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> Vec<String> {
    let mut out = Vec::new();
    crate::contents::each(doc, objs, content, MAX_CONTENT_BYTES, &mut |data| {
        // Only content with the `BI` operator holds inline images: the rest isn't parsed.
        if data.windows(2).any(|w| w == b"BI") {
            for op in pdfcraft_content::ops(data) {
                if let Some((image, _)) = &op.inline {
                    forbidden(image, &mut out);
                }
            }
        }
        out.len() < MAX_NAMES
    });
    out
}
