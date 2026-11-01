//! Ungroup (Edit a PDF): grouped Form artwork becomes page content without changing the page.

use std::sync::Arc;

use super::render_page;
use crate::{DocId, Edit, ImageEdit, Session};

fn pdf(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn stream(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// A page drawn the way CAD exports draw it: one Form XObject, scaled onto the page, holding the
/// text, line work and a nested symbol. Its resource names clash with the page's, it clips its
/// content to a box smaller than what it draws, and it leaves a graphics state open.
fn cad_page() -> (Session, DocId) {
    let page = "q 0.12 0 0 0.12 0.5 0.5 cm /Fm1 Do Q BT /F1 9 Tf 10 10 Td (Page footer) Tj ET 0 0 1 RG 2 w 5 290 m 195 290 l S";
    let form = "0 w 0.03 0 0 RG q 4 3 m 1600 3 l 1600 2400 l 4 2400 l h W n 6 w 0 0 0 RG \
                100 100 m 1500 100 l 1500 2300 l S /G1 gs 200 1800 m 1400 1800 l S \
                BT 60 0 0 60 160 2000 Tm /F1 1.4 Tf (Elektroschema Deckblatt) Tj ET \
                1 0.5 0 rg 300 600 400 300 re f \
                q 1 0 0 1 900 600 cm /Sym Do Q \
                1700 2500 m 1900 2500 l S q 0 0 1 rg";
    let symbol = "0 0 0 RG 8 w 0 0 m 200 200 l S 0 200 m 200 0 l S BT /F1 40 Tf 0 220 Td (K1) Tj ET";
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R >> /ExtGState << /G1 << /LW 0.5 >> >> >> /Contents 4 0 R >>".to_string(),
        stream("", page),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 1650 2450] /Matrix [1 0 0 1 10 20] /Resources << /Font << /F1 7 0 R >> /ExtGState << /G1 << /LW 20 >> >> /XObject << /Sym 8 0 R >> >>",
            form,
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold >>".to_string(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 7 0 R >> >>", symbol),
    ];
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("cad.pdf", None, Arc::new(pdf(&objs)), None).expect("opens");
    (s, id)
}

/// Ungrouping a page-sized group leaves every pixel as it was, makes its paragraphs and its nested
/// symbol separate objects, survives save and reopen, and is one undo step.
#[test]
fn ungrouping_cad_artwork_keeps_every_pixel_and_frees_its_objects() {
    let (mut s, id) = cad_page();
    let before = render_page(&s, id);
    let objects = s.get(id).unwrap().editable_objects(0).unwrap();
    let kinds: Vec<&str> = objects.iter().map(|o| o.content_type).collect();
    assert_eq!(kinds, ["text", "form"], "the page's footer and one page-sized group: {objects:?}");

    s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Ungroup }).unwrap();
    let after = render_page(&s, id);
    assert_eq!((after.width, after.height), (before.width, before.height));
    assert!(after.rgba == before.rgba, "ungrouping changed the page's pixels");

    let doc = s.get(id).unwrap();
    let texts: Vec<String> = doc.text_blocks(0).into_iter().map(|b| b.text).collect();
    assert!(texts.iter().any(|t| t == "Elektroschema Deckblatt"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "Page footer"), "{texts:?}");
    let images = doc.page_images(0);
    assert_eq!(images.len(), 1, "the nested symbol is now a group of its own: {images:?}");
    assert!(images[0].is_form);

    // Saved and reopened, it still looks the same.
    let saved = s.save_bytes(id).unwrap();
    let reopened = s.open("saved.pdf", None, saved, None).unwrap();
    assert!(render_page(&s, reopened).rgba == before.rgba, "the saved file looks different");

    assert_eq!(s.undo(id).unwrap().to_string(), "Ungroup artwork");
    assert_eq!(s.get(id).unwrap().page_images(0).len(), 1);
    assert!(s.get(id).unwrap().page_images(0)[0].rect[2] > 150.0, "the page-sized group is back");
    assert!(render_page(&s, id).rgba == before.rgba);

    // The nested symbol can be ungrouped in turn, still without changing a pixel.
    s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Ungroup }).unwrap();
    s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Ungroup }).unwrap();
    assert!(s.get(id).unwrap().page_images(0).is_empty());
    assert!(s.get(id).unwrap().text_blocks(0).iter().any(|b| b.text == "K1"));
    assert!(render_page(&s, id).rgba == before.rgba);
}

/// A refused ungroup changes nothing and leaves no undo step.
#[test]
fn a_refused_ungroup_changes_nothing() {
    let (mut s, id) = cad_page();
    s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Ungroup }).unwrap();
    s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Ungroup }).unwrap();
    let bytes = s.get(id).unwrap().bytes.clone();
    // No groups are left: index 0 is no longer a form.
    assert!(s.apply(id, Edit::EditPageImage { page: 0, index: 0, change: ImageEdit::Ungroup }).is_err());
    assert_eq!(s.get(id).unwrap().bytes, bytes);
    assert_eq!(s.undo(id).unwrap().to_string(), "Ungroup artwork");
}
