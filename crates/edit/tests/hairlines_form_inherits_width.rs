//! Fix hairlines measures a form's strokes with the line width in effect where the form is
//! drawn (a form inherits the graphics state of its `Do`), and puts that width back after a
//! stroke it widens, in forms drawn by forms too.

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

const CATALOG: (&str, Option<&[u8]>) = ("<< /Type /Catalog /Pages 2 0 R >>", None);
const PAGES: (&str, Option<&[u8]>) = ("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>", None);
const PAGE: (&str, Option<&[u8]>) = ("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>", None);
const FORM: &str = "/Type /XObject /Subtype /Form /BBox [0 0 100 100]";

#[test]
fn a_form_without_its_own_width_is_measured_with_the_width_at_its_do() {
    // 0.1 pt is in effect where the form is drawn; the form sets no width of its own.
    let mut doc = pdf(&[CATALOG, PAGES, PAGE, ("", Some(b"0.1 w /Fm0 Do")), (FORM, Some(b"0 0 m 10 0 l S"))]);
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    assert_eq!(flat(&doc, ObjRef { num: 5, generation: 0 }), "0.72 w 0 0 m 10 0 l S 0.1 w");
}

#[test]
fn a_widened_stroke_in_a_form_gets_the_inherited_width_back() {
    // 0.4 pt at half scale is 0.2 pt on the page; after the stroke the form's later strokes
    // must draw with the 0.4 they inherited, not the 1 a fresh graphics state has.
    let mut doc = pdf(&[CATALOG, PAGES, PAGE, ("", Some(b"0.5 0 0 0.5 0 0 cm 0.4 w /Fm0 Do")), (FORM, Some(b"0 0 m 1 0 l S"))]);
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    assert_eq!(flat(&doc, ObjRef { num: 5, generation: 0 }), "1.44 w 0 0 m 1 0 l S 0.4 w");
}

#[test]
fn a_form_drawn_by_a_form_inherits_the_width_too() {
    let fm0 = "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /XObject << /Fm1 6 0 R >> >>";
    let mut doc = pdf(&[CATALOG, PAGES, PAGE, ("", Some(b"0.1 w /Fm0 Do")), (fm0, Some(b"/Fm1 Do")), (FORM, Some(b"0 0 m 10 0 l S"))]);
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    assert_eq!(flat(&doc, ObjRef { num: 6, generation: 0 }), "0.72 w 0 0 m 10 0 l S 0.1 w");
}
