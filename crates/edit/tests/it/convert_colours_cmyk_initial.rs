//! Convert colours: `/DeviceCMYK cs` starts at black (0 0 0 1, ISO 32000-1 8.6.5.5), so the
//! converted operator is black too, not the white that four zeros would be.

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

fn page_with(content: &str) -> Document {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>",
        &format!("<< >>stream:{content}"),
    ])
}

#[test]
fn device_cmyk_starts_at_black() {
    let mut doc = page_with("/DeviceCMYK cs 0 0 10 10 re f /DeviceCMYK CS 0 0 m 10 10 l S");
    let s = ConvertColours { preserve_black: false, ..ConvertColours::default() };
    let done = convert_colours(&mut doc, &[0], &s, &mut no_images).unwrap();
    assert_eq!(done.colours, 2);
    let t = stream_text(&doc, contents(&doc, 0)[0]);
    assert!(t.contains("0 g 0 0 10 10 re f") && t.contains("0 G 0 0 m 10 10 l S"), "black, not white: {t}");
}
