//! Convert colours rewrites each of a page's content streams on its own: a stream keeps its
//! dictionary (its `/PCMark` among it), unchanged streams stay as they are, the colour space
//! set in one stream holds in the next, and a page with an unreadable stream is left alone.

use pdfcraft_cos::{Document, ObjRef, Object, Stream};
use pdfcraft_edit::{ColourTarget, ConvertColours, convert_colours};
use std::sync::Arc;

/// A document of `objs` (object k is `k + 1 0 obj`; object 1 is the catalog). A `{stream:...}`
/// entry is written as a stream holding the text after the colon, with its `/Length`.
fn pdf(objs: &[&str]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        let body = match o.split_once(">>stream:") {
            Some((dict, data)) => format!("{dict} /Length {} >>\nstream\n{data}\nendstream", data.len()),
            None => (*o).to_string(),
        };
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn flat(b: &[u8]) -> String {
    String::from_utf8_lossy(b).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn stream_text(doc: &Document, r: ObjRef) -> String {
    match &*doc.get(r) {
        Object::Stream(s) => flat(&s.decoded().unwrap()),
        _ => panic!("not a stream"),
    }
}

/// The page's `/Contents` references.
fn contents(doc: &Document, page: usize) -> Vec<ObjRef> {
    let p = pdfcraft_model::pages(doc).swap_remove(page);
    match &*doc.resolve(p.dict.get(b"Contents").unwrap()) {
        Object::Array(a) => a.iter().filter_map(Object::as_ref).collect(),
        _ => p.dict.get(b"Contents").and_then(Object::as_ref).into_iter().collect(),
    }
}

fn no_images(_: &Document, _: &Stream, _: ColourTarget) -> Option<Stream> {
    None
}

/// One page drawing `streams`, with an indexed colour space `/CS0` (0 = red, 1 = green).
fn page_with(streams: &[&str]) -> Document {
    let refs: Vec<String> = (0..streams.len()).map(|i| format!("{} 0 R", i + 4)).collect();
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /CS0 [/Indexed /DeviceRGB 1 <FF000000FF00>] >> >> /Contents [{}] >>",
        refs.join(" ")
    );
    let mut objs: Vec<&str> = vec!["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>", &page];
    objs.extend_from_slice(streams);
    pdf(&objs)
}

#[test]
fn each_stream_keeps_its_dictionary_and_only_changed_ones_are_replaced() {
    let mut doc = page_with(&["<< >>stream:0.5 g 0 0 10 10 re f", "<< /PCMark /Redaction >>stream:1 0 0 rg 0 0 10 10 re f"]);
    let before = contents(&doc, 0);
    let done = convert_colours(&mut doc, &[0], &ConvertColours::default(), &mut no_images).unwrap();
    assert_eq!(done.colours, 1);
    let after = contents(&doc, 0);
    assert_eq!(after.len(), 2, "still two streams");
    assert_eq!(after[0], before[0], "the unchanged stream is the same object");
    let mark = match &*doc.get(after[1]) {
        Object::Stream(s) => s.dict.name(b"PCMark").map(<[u8]>::to_vec),
        _ => None,
    };
    assert_eq!(mark.as_deref(), Some(&b"Redaction"[..]), "the mark is kept");
    assert!(stream_text(&doc, after[1]).contains("0.299 g 0 0 10 10 re f"));
}

#[test]
fn the_colour_space_carries_from_one_stream_to_the_next() {
    // Index 0 of the space set in the first stream is red; read as a gray, it would be black.
    let mut doc = page_with(&["<< >>stream:/CS0 cs", "<< >>stream:0 sc 0 0 10 10 re f"]);
    convert_colours(&mut doc, &[0], &ConvertColours::default(), &mut no_images).unwrap();
    let after = contents(&doc, 0);
    assert!(stream_text(&doc, after[1]).contains("0.299 g 0 0 10 10 re f"), "{}", stream_text(&doc, after[1]));
}

#[test]
fn a_page_with_an_unreadable_stream_is_left_alone() {
    let mut doc = page_with(&["<< >>stream:1 0 0 rg 0 0 10 10 re f", "<< /Filter /DCTDecode >>stream:not a picture"]);
    let before = contents(&doc, 0);
    let done = convert_colours(&mut doc, &[0], &ConvertColours::default(), &mut no_images).unwrap();
    assert_eq!(done.colours, 0);
    assert_eq!(contents(&doc, 0), before, "no stream is dropped");
}
