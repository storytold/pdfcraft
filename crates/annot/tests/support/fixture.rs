//! Shared helpers compile into every integration-test binary of this crate, and not every
//! binary uses every one of them; `dead_code` would then fail `clippy -D warnings` for reasons
//! that have nothing to do with the test being written.
#![allow(dead_code)]

//! Hand-written PDF skeletons and the save/reopen helpers these tests share.
//!
//! The shape follows `crates/annot/src/tests.rs::fixture`: build the bytes of a small
//! cross-referenced PDF by hand, open it with `pdfcraft_cos::Document::open`, and let
//! `pdfcraft-annot` add comments through its public API. Doing it this way (rather than
//! constructing a `Document` in memory) matters for these tests: the file has a real
//! cross-reference chain, so `write_incremental` takes its genuine incremental path instead of
//! silently falling back to a full rewrite.
//!
//! Every save here goes through one of three writer paths, and the tests replay their assertions
//! over all three:
//!
//! * `Save::Incremental` — the original bytes plus an appended revision and `/Prev`;
//! * `Save::FullTable` — a full rewrite with a classic cross-reference table;
//! * `Save::FullStreams` — a full rewrite that packs objects into compressed object streams and
//!   renumbers everything from 1.
//!
//! The third path is the interesting one for annotation *identity*: object numbers change, so a
//! `/P`, `/Popup` or `/IRT` that survives it has really been re-pointed, not copied.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, write_full, write_incremental};

/// Which writer path a round trip should take.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Save {
    Incremental,
    FullTable,
    FullStreams,
}

/// Every path, in the order tests report them.
pub const ALL_SAVES: [Save; 3] = [Save::Incremental, Save::FullTable, Save::FullStreams];

/// The paths whose output holds plain (uncompressed) objects, so raw-byte assertions can look at
/// them. Object streams are Flate-compressed by `write_full`, so their bytes need decoding
/// first; those cases are checked through the reopened document instead.
pub const UNCOMPRESSED_SAVES: [Save; 2] = [Save::Incremental, Save::FullTable];

/// Objects written as indirect objects plus a classic cross-reference table and trailer. The
/// catalog is object 1, so `objs` starts with it.
pub fn pdf(objs: &[&[u8]]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A document of `pages` blank pages (612×792, no `/Annots` anywhere).
pub fn blank(pages: usize) -> Document {
    let mut kids = String::new();
    for i in 0..pages {
        kids.push_str(&format!("{} 0 R ", 3 + i));
    }
    let mut objs: Vec<Vec<u8>> =
        vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), format!("<</Type /Pages/Kids [{kids}]/Count {pages} >>").into_bytes()];
    for _ in 0..pages {
        objs.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec());
    }
    open(&pdf_refs(&objs))
}

/// Like [`blank`], but page `page` starts with an indirect (and initially empty) `/Annots`
/// array, so writes to it go through `set_annots`' shared-array branch.
pub fn blank_with_indirect_annots(pages: usize, page: usize) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", (0..pages).map(|i| format!("{} 0 R", 3 + i)).collect::<Vec<_>>().join(" "))
            .into_bytes(),
    ];
    for i in 0..pages {
        objs.push(if i == page {
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots {} 0 R >>", 3 + pages).into_bytes()
        } else {
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec()
        });
    }
    objs.push(b"[]".to_vec());
    open(&pdf_refs(&objs))
}

/// [`pdf`] over already-owned object bodies.
fn pdf_refs(objs: &[Vec<u8>]) -> Vec<u8> {
    let borrowed: Vec<&[u8]> = objs.iter().map(Vec::as_slice).collect();
    pdf(&borrowed)
}

fn open(bytes: &[u8]) -> Document {
    Document::open(Arc::new(bytes.to_vec())).expect("the skeleton parses")
}

/// Save one document, then reopen the bytes with our reader. A second, independent parser must
/// accept the same bytes first — that is what makes "it reloads" mean more than "our own writer
/// and reader agree with each other".
pub fn save_and_reopen(doc: &Document, path: Save) -> (Vec<u8>, Document) {
    let bytes = save(doc, path);
    hayro_syntax::Pdf::new(bytes.clone()).unwrap_or_else(|e| panic!("{path:?} output rejected by hayro-syntax: {e:?}"));
    (bytes.clone(), Document::open(Arc::new(bytes)).unwrap_or_else(|e| panic!("{path:?} output rejected by our reader: {e:?}")))
}

/// Save with the path's options. `/Info /ModDate` is left unset so a full save adds nothing that
/// would make byte comparisons depend on the clock.
pub fn save(doc: &Document, path: Save) -> Vec<u8> {
    let opts = SaveOptions { mod_date: None, id_seed: 0x5052_494E_5443_5241, object_streams: path == Save::FullStreams };
    match path {
        Save::Incremental => write_incremental(doc, &opts),
        Save::FullTable | Save::FullStreams => write_full(doc, &opts),
    }
    .expect("saves")
}

/// The leaf pages, in `/Kids` order. Local so these tests do not depend on `annot::page_refs`
/// about a page tree they are only incidentally exercising.
pub fn page_object_numbers(doc: &Document) -> Vec<u32> {
    let root = doc.root().expect("a catalog");
    let pages = doc.get(root).as_dict().and_then(|d| d.reference(b"Pages")).expect("/Pages");
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    collect_pages(doc, pages, &mut seen, &mut out);
    out
}

fn collect_pages(doc: &Document, node: pdfcraft_cos::ObjRef, seen: &mut std::collections::HashSet<pdfcraft_cos::ObjRef>, out: &mut Vec<u32>) {
    // `seen` bounds the walk, so a cyclic page tree visits each node once and cannot recurse forever.
    if !seen.insert(node) || seen.len() > 10_000 {
        return;
    }
    let obj = doc.get(node);
    let Some(d) = obj.as_dict() else { return };
    if d.name(b"Type") == Some(b"Page") {
        out.push(node.num);
        return;
    }
    let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) else { return };
    for k in kids {
        // The array element itself is the reference; resolving it would give the page dictionary.
        if let Object::Ref(r) = k {
            collect_pages(doc, r, seen, out);
        }
    }
}

/// The raw entries of page `page`'s `/Annots`, in array order (references or inline dictionaries).
pub fn annot_entries(doc: &Document, page: usize) -> Vec<Object> {
    let num = *page_object_numbers(doc).get(page).expect("the page exists");
    let d = doc.get(pdfcraft_cos::ObjRef::new(num, 0));
    let Some(a) = d.as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a))) else { return Vec::new() };
    a.as_array().cloned().unwrap_or_default()
}

/// The dictionary an `/Annots` entry names (a stream's dictionary counts as one).
pub fn dict_of(doc: &Document, entry: &Object) -> Option<Dict> {
    doc.resolve(entry).as_dict().cloned()
}

/// A dictionary's number array with every element resolved, as PDF numbers. An entry that is not
/// a number (a name, a string, a dictionary) yields `None` here rather than 0.0, so tests can
/// tell "absent or wrong type" apart from "present and zero".
pub fn nums(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<f64>> {
    let a = doc.resolve(d.get(key)?).as_array()?.clone();
    a.iter().map(|x| doc.resolve(x).as_f64()).collect()
}

/// A single number entry (`/CA`, `/BS /W`), resolving an indirect value.
pub fn num(doc: &Document, d: &Dict, key: &[u8]) -> Option<f64> {
    doc.resolve(d.get(key)?).as_f64()
}

/// A dictionary's text entry, decoded with our reader (`None` when absent or not a string).
pub fn str_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    doc.resolve(d.get(key)?).as_string().map(|s| s.to_text())
}

/// A dictionary's name entry as a `&str` where it is ASCII, for readable assertion messages.
pub fn name_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    let o = doc.resolve(d.get(key)?);
    Some(String::from_utf8_lossy(o.as_name()?).into_owned())
}

/// Annotation flag bit masks, ISO 32000-2 §12.5.3 Table 165 (the spec numbers bits from 1).
/// `pdfcraft-annot` keeps these private, so they are restated here from the standard: this is
/// deliberately *not* read back from the crate, otherwise a wrong bit would be tested against
/// itself.
pub const FLAG_INVALID: i64 = 1 << 0; // bit 1
pub const FLAG_HIDDEN: i64 = 1 << 1; // bit 2
pub const FLAG_PRINT: i64 = 1 << 2; // bit 3
pub const FLAG_NO_ZOOM: i64 = 1 << 3; // bit 4
pub const FLAG_NO_ROTATE: i64 = 1 << 4; // bit 5
pub const FLAG_NO_VIEW: i64 = 1 << 5; // bit 6
pub const FLAG_READONLY: i64 = 1 << 7; // bit 8 (`/ReadOnly`, PdfCraft's Locked)
