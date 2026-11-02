//! Fix hairlines follows the line width an ExtGState's `/LW` sets through `gs`: a stroke it
//! leaves thin is found, and the width put back after a widened stroke is that one.

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

/// A one-page document drawing `content`, with `/GS0` setting `/LW lw` and `/GS1` no width.
fn with_gstate(lw: &str, content: &[u8]) -> Document {
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS0 << /Type /ExtGState /LW {lw} >> /GS1 << /Type /ExtGState /CA 1 >> >> >> >>"
    );
    pdf(&[
        ("<< /Type /Catalog /Pages 2 0 R >>", None),
        ("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>", None),
        (page.as_str(), None),
        ("", Some(content)),
    ])
}

#[test]
fn the_width_put_back_is_the_one_gs_set() {
    // 2 pt under a tenth scale is 0.2 pt on the page: both strokes are widened, and each gets
    // the 2 pt back after it.
    let mut doc = with_gstate("2", b"/GS0 gs 0.1 0 0 0.1 0 0 cm 0 0 m 1 0 l S 0 1 m 1 1 l S");
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 2);
    let t = page_text(&doc, 0);
    assert_eq!(t.matches("l S 2 w").count(), 2, "{t}");
    assert!(!t.contains(" 1 w"), "no stroke is put back to the default width: {t}");
}

#[test]
fn a_zero_width_set_by_gs_is_a_hairline() {
    let mut doc = with_gstate("0", b"/GS0 gs 0 0 m 1 0 l S");
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    assert_eq!(page_text(&doc, 0), "/GS0 gs 0.72 w 0 0 m 1 0 l S 0 w");
}

#[test]
fn a_gs_without_a_width_keeps_the_one_in_effect() {
    let mut doc = with_gstate("2", b"0.1 w /GS1 gs 0 0 m 1 0 l S");
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    assert_eq!(page_text(&doc, 0), "0.1 w /GS1 gs 0.72 w 0 0 m 1 0 l S 0.1 w");
}
