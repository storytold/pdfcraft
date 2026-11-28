//! Fix hairlines rewrites each of a page's content streams on its own: a stream keeps its
//! dictionary (its `/PCMark` among it), streams that did not change stay as they are, the
//! graphics state carries from one stream to the next, and a page with a stream that cannot
//! be read is left alone instead of losing that stream.

use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_edit::{Hairlines, fix_hairlines};
use std::sync::Arc;

/// A one-page document whose `/Contents` lists `streams` (dictionary entries, data).
fn one_page(streams: &[(&str, &[u8])]) -> Document {
    let refs: Vec<String> = (0..streams.len()).map(|i| format!("{} 0 R", i + 4)).collect();
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /Contents [{}] >>", refs.join(" ")).into_bytes(),
    ];
    for (dict, data) in streams {
        let mut s = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        s.extend_from_slice(data);
        s.extend_from_slice(b"\nendstream");
        objs.push(s);
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// The page's `/Contents` references.
fn contents(doc: &Document) -> Vec<ObjRef> {
    let page = pdfcraft_model::pages(doc).swap_remove(0);
    match &*doc.resolve(page.dict.get(b"Contents").unwrap()) {
        Object::Array(a) => a.iter().filter_map(Object::as_ref).collect(),
        _ => page.dict.get(b"Contents").and_then(Object::as_ref).into_iter().collect(),
    }
}

fn text_of(doc: &Document, r: ObjRef) -> String {
    match &*doc.get(r) {
        Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).split_whitespace().collect::<Vec<_>>().join(" "),
        _ => panic!("not a stream"),
    }
}

#[test]
fn each_stream_keeps_its_dictionary_and_only_changed_ones_are_replaced() {
    let mut doc = one_page(&[("", b"2 w 0 0 m 1 1 l S"), ("/PCMark /Redaction", b"0 w 0 0 m 1 1 l S")]);
    let before = contents(&doc);
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    let after = contents(&doc);
    assert_eq!(after.len(), 2, "still two streams");
    assert_eq!(after[0], before[0], "the unchanged stream is the same object");
    assert_ne!(after[1], before[1]);
    let mark = match &*doc.get(after[1]) {
        Object::Stream(s) => s.dict.name(b"PCMark").map(<[u8]>::to_vec),
        _ => None,
    };
    assert_eq!(mark.as_deref(), Some(&b"Redaction"[..]), "the mark is kept");
    assert!(text_of(&doc, after[1]).contains("0.72 w 0 0 m 1 1 l S"));
}

#[test]
fn the_graphics_state_carries_from_one_stream_to_the_next() {
    // 0.3 pt under the first stream's half scale is 0.15 pt on the page.
    let mut doc = one_page(&[("", b"0.5 0 0 0.5 0 0 cm"), ("", b"0.3 w 0 0 m 1 1 l S")]);
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    let after = contents(&doc);
    assert!(text_of(&doc, after[1]).contains("1.44 w 0 0 m 1 1 l S"));
}

#[test]
fn a_page_with_an_unreadable_stream_is_left_alone() {
    let mut doc = one_page(&[("", b"0 w 0 0 m 1 1 l S"), ("/Filter /DCTDecode", b"not a picture")]);
    let before = contents(&doc);
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 0);
    assert_eq!(contents(&doc), before, "no stream is dropped");
}
