use pdfcraft_cos::{Document, ObjRef, Object};
use pdfcraft_preflight::{Level, convert, verify};

fn build(objs: &[&str], trailer_extra: &str) -> Document {
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(std::sync::Arc::new(out)).unwrap()
}

/// One page whose `/Annots` lists a Sound annotation (forbidden by clause 6.3.1) and a link.
fn page_with_a_forbidden_sound_annotation() -> Document {
    build(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
            "<< /Type /Annot /Subtype /Sound /Rect [10 10 30 30] /P 3 0 R /F 4 >>",
            "<< /Type /Annot /Subtype /Link /Rect [40 40 60 60] /P 3 0 R /F 4 /Border [0 0 0] >>",
        ],
        "",
    )
}

/// The forbidden annotation is reported before `convert` and by neither `convert` nor a later
/// `verify` afterwards: the object it took off the page is no longer an annotation.
#[test]
fn convert_removes_the_forbidden_annotation_clause_6_3_1() {
    let mut doc = page_with_a_forbidden_sound_annotation();
    let before = verify(&doc, Level::A2b);
    assert!(before.iter().any(|i| i.clause == "6.3.1"), "the fixture starts with the finding: {before:#?}");
    let report = convert(&mut doc, Level::A2b).unwrap();
    assert!(!report.remaining.iter().any(|i| i.clause == "6.3.1"), "6.3.1 remains after convert: {:#?}", report.remaining);
    let after = verify(&doc, Level::A2b);
    assert!(!after.iter().any(|i| i.clause == "6.3.1"), "6.3.1 is reported after convert: {after:#?}");
}

/// The page keeps its other annotation: only the forbidden one leaves `/Annots`.
#[test]
fn convert_keeps_the_other_annotations_on_the_page() {
    let mut doc = page_with_a_forbidden_sound_annotation();
    convert(&mut doc, Level::A2b).unwrap();
    let page = doc.get(ObjRef::new(3, 0));
    let dict = page.as_dict().expect("the page is a dictionary");
    let annots = doc.resolve(dict.get(b"Annots").expect("the page has /Annots"));
    let kept: Vec<ObjRef> = annots.as_array().expect("the page's /Annots is an array").iter().filter_map(Object::as_ref).collect();
    assert_eq!(kept, vec![ObjRef::new(5, 0)]);
}
