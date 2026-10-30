//! Added text drawn with the face an embedding app supplies (`set_document_korean_font`). The face
//! is process-wide, so this check runs in its own test binary. Inter stands in for a Korean face:
//! it draws characters the standard fonts lack (here U+0150), which take the same path as Hangul.

use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_full};
use pdfcraft_edit::{AddedText, Content, add_content, first_undrawable, list_added};

fn one_page() -> Document {
    let objs =
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>"];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

#[test]
fn supplied_face_draws_what_the_standard_fonts_cannot() {
    let text = AddedText { rect: [20.0, 20.0, 280.0, 60.0], text: "\u{150} Kim".into(), size: 14.0, ..AddedText::default() };
    // Without a face the character is refused, as before.
    assert_eq!(first_undrawable(&text), Some('\u{150}'));
    assert!(!pdfcraft_fonts::korean_has('\u{150}'));

    assert!(!pdfcraft_fonts::set_document_korean_font(b"not a font"), "unreadable bytes are not taken");
    assert!(pdfcraft_fonts::set_document_korean_font(include_bytes!("../../../assets/fonts/Inter-Regular.ttf")));
    assert!(pdfcraft_fonts::korean_has('\u{150}'));
    assert_eq!(first_undrawable(&text), None);

    let mut doc = one_page();
    add_content(&mut doc, 0, &Content::Text(text.clone())).unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    assert!(bytes.windows(5).any(|w| w == b"Type3"), "the character is drawn from the face's outlines");
    let back = Document::open(Arc::new(bytes)).unwrap();
    let added = list_added(&back);
    assert_eq!(added.len(), 1);
    assert!(matches!(&added[0].content, Content::Text(t) if t.text == text.text), "{:?}", added[0].content);
}
