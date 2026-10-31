//! Fix hairlines rewrites a form for the scale it is drawn under: a form a page not being
//! fixed also draws, or one drawn under two scales, is rewritten as a copy for the use that
//! needs it, so the other uses keep drawing it as it was.

use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_edit::{Hairlines, fix_hairlines};
use std::sync::Arc;

/// A document of `objects` (object `i + 1` is the `i`th: a dictionary, and stream data when
/// it is a stream); object 1 is the catalog.
fn pdf(objects: &[(&str, Option<&[u8]>)]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, (dict, data)) in objects.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        match data {
            Some(data) => {
                out.extend_from_slice(format!("<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes());
                out.extend_from_slice(data);
                out.extend_from_slice(b"\nendstream");
            }
            None => out.extend_from_slice(dict.as_bytes()),
        }
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objects.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// A stream's data, its tokens joined by single spaces.
fn flat(doc: &Document, r: ObjRef) -> String {
    match &*doc.get(r) {
        Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).split_whitespace().collect::<Vec<_>>().join(" "),
        _ => panic!("not a stream"),
    }
}

/// Page `i`'s content (a single stream), flattened.
fn page_text(doc: &Document, i: usize) -> String {
    let page = pdfcraft_model::pages(doc).swap_remove(i);
    flat(doc, page.dict.get(b"Contents").and_then(Object::as_ref).unwrap())
}

/// The object page `i`'s resources draw as `name`.
fn xobject(doc: &Document, i: usize, name: &[u8]) -> ObjRef {
    let page = pdfcraft_model::pages(doc).swap_remove(i);
    let res = doc.resolve(page.dict.get(b"Resources").unwrap());
    let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
    xo.as_dict().unwrap().get(name).and_then(Object::as_ref).unwrap()
}

const FORM: &str = "/Type /XObject /Subtype /Form /BBox [0 0 100 100]";
const ORIGINAL: &str = "0.4 w 0 0 m 1 0 l S";

#[test]
fn a_form_another_page_draws_is_fixed_as_a_copy() {
    // Both pages share one resource dictionary; the first draws the form at half scale (its
    // 0.4 pt line is 0.2 pt there), the second at full size.
    let mut doc = pdf(&[
        ("<< /Type /Catalog /Pages 2 0 R >>", None),
        ("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 600 800] >>", None),
        ("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources 7 0 R >>", None),
        ("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources 7 0 R >>", None),
        ("", Some(b"0.5 0 0 0.5 0 0 cm /Fm0 Do")),
        ("", Some(b"/Fm0 Do")),
        ("<< /XObject << /Fm0 8 0 R >> >>", None),
        (FORM, Some(ORIGINAL.as_bytes())),
    ]);
    let form = ObjRef { num: 8, generation: 0 };
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    assert_eq!(flat(&doc, form), ORIGINAL, "the form the second page draws is unchanged");
    assert_eq!(xobject(&doc, 1, b"Fm0"), form, "the second page still draws it");
    let copy = xobject(&doc, 0, b"Fm0");
    assert_ne!(copy, form, "the first page draws a copy");
    assert_eq!(flat(&doc, copy), "0.4 w 1.44 w 0 0 m 1 0 l S 0.4 w");
}

#[test]
fn a_form_drawn_under_two_scales_gets_one_object_for_each() {
    let mut doc = pdf(&[
        ("<< /Type /Catalog /Pages 2 0 R >>", None),
        ("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>", None),
        ("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>", None),
        ("", Some(b"q 0.5 0 0 0.5 0 0 cm /Fm0 Do Q /Fm0 Do")),
        (FORM, Some(ORIGINAL.as_bytes())),
    ]);
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    let text = page_text(&doc, 0);
    let words: Vec<&str> = text.split(' ').collect();
    let drawn: Vec<&str> = words.windows(2).filter(|w| w[1] == "Do").map(|w| w[0]).collect();
    assert_eq!(drawn.len(), 2, "{text}");
    assert_eq!(drawn[0], "/Fm0", "{text}");
    assert_ne!(drawn[1], "/Fm0", "the full-size use draws another name: {text}");
    let half = xobject(&doc, 0, b"Fm0");
    let full = xobject(&doc, 0, drawn[1].trim_start_matches('/').as_bytes());
    assert_ne!(half, full);
    assert_eq!(flat(&doc, half), "0.4 w 1.44 w 0 0 m 1 0 l S 0.4 w");
    assert_eq!(flat(&doc, full), ORIGINAL, "at full size the 0.4 pt line is left as it was");
}
