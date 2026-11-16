//! Fix hairlines gives a thin stroke a width of its own: set before its path begins (a `w`
//! inside a path is not allowed) and put back after it is painted, so the `w` other strokes
//! share, and strokes under another transformation, keep their width.

use pdfcraft_cos::{Document, Object};
use pdfcraft_edit::{Hairlines, fix_hairlines};
use std::sync::Arc;

/// A one-page document drawing `content`.
fn one_page(content: &[u8]) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
    ];
    let mut s = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    s.extend_from_slice(content);
    s.extend_from_slice(b"\nendstream");
    objs.push(s);
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

/// The page's content, its tokens joined by single spaces.
fn content_text(doc: &Document) -> String {
    let page = pdfcraft_model::pages(doc).swap_remove(0);
    let list: Vec<Object> = match &*doc.resolve(page.dict.get(b"Contents").unwrap()) {
        Object::Array(a) => a.clone(),
        _ => vec![page.dict.get(b"Contents").unwrap().clone()],
    };
    let mut text = String::new();
    for e in list {
        if let Object::Stream(s) = &*doc.resolve(&e) {
            text.push_str(&String::from_utf8_lossy(&s.decoded().unwrap()));
            text.push(' ');
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn a_thin_stroke_does_not_change_the_width_other_strokes_share() {
    // The first line is 0.1 pt on the page; the second, under a tenfold scale, is 1 pt.
    let mut doc = one_page(b"0.1 w 0 0 m 10 0 l S 10 0 0 10 0 0 cm 0 1 m 1 1 l S");
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 1);
    let t = content_text(&doc);
    assert!(t.starts_with("0.1 w 0.72 w 0 0 m 10 0 l S 0.1 w 10 0 0 10 0 0 cm"), "the shared w is kept: {t}");
    assert!(!t.contains("7.2"), "the scaled stroke keeps its 1 pt: {t}");
}

#[test]
fn a_stroke_inside_q_does_not_change_the_outer_width() {
    let mut doc = one_page(b"0.1 w q 0 0 m 1 0 l S Q 0 1 m 1 1 l S");
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines::default()).unwrap();
    assert_eq!(fixed.lines, 2);
    let t = content_text(&doc);
    assert!(t.starts_with("0.1 w q 0.72 w 0 0 m 1 0 l S 0.1 w Q 0.72 w 0 1 m 1 1 l S 0.1 w"), "{t}");
}

#[test]
fn the_new_width_is_set_before_the_path_not_inside_it() {
    // No `w` before the stroke: the default 1 pt at half scale is 0.5 pt on the page.
    let mut doc = one_page(b"0.5 0 0 0.5 0 0 cm 0 0 m 10 0 l S 5 5 10 10 re f");
    let fixed = fix_hairlines(&mut doc, &[0], &Hairlines { narrower_than: 0.6, width: 1.0, ..Hairlines::default() }).unwrap();
    assert_eq!(fixed.lines, 1);
    let t = content_text(&doc);
    assert!(t.contains("cm 2 w 0 0 m 10 0 l S 1 w 5 5 10 10 re f"), "{t}");
    assert!(!t.contains("l 2 w S"), "no w inside the path: {t}");
}
