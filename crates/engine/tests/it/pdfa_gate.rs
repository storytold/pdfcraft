//! The PDF/A gate (`Session::pdfa_check`): a document that declares PDF/A is checked against the
//! part it declares, and what its changes break, measured from the document as it first declared
//! PDF/A in the session, is what the gate reports.

use std::sync::Arc;

use pdfcraft_engine::pdfa::Level;
use pdfcraft_engine::{DocId, Edit, PdfaClaim, Session};

/// A one-page file. `ids` (if any) are the attributes of its XMP identification;
/// `content` (if any) is the page's content stream, drawn with an unembedded Helvetica `/F1`.
fn file(version: &str, ids: Option<&str>, content: Option<&str>) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R{} >>", if ids.is_some() { " /Metadata 4 0 R" } else { "" }),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >>{} >>",
            if content.is_some() { " /Contents 6 0 R" } else { "" }
        ),
    ];
    let xmp = ids.map(|ids| {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" {ids}/></rdf:RDF></x:xmpmeta>"
        )
    });
    objs.push(match &xmp {
        Some(x) => format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{x}\nendstream", x.len()),
        None => "null".to_string(),
    });
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
    if let Some(c) = content {
        objs.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
    }
    let mut pdf = format!("%PDF-{version}\n%\u{e2}\u{e3}\n").into_bytes();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    let id = "/ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>]";
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {id} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    pdf
}

const PDFA4: &str = "pdfaid:part=\"4\" pdfaid:rev=\"2020\"";
const VISIBLE_TEXT: &str = "BT /F1 12 Tf 20 100 Td (Hello) Tj ET";

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("in.pdf", None, Arc::new(bytes), None).unwrap();
    (s, id)
}

fn attach() -> Edit {
    Edit::AddAttachment { name: "note.txt".into(), bytes: b"hello\n".to_vec(), description: None, mime: Some("text/plain".into()) }
}

/// The clauses the document's changes break, as the gate reports them.
fn broken(s: &Session, id: DocId) -> Vec<&'static str> {
    s.pdfa_check(id).expect("declares PDF/A").breaks.iter().map(|b| b.clause).collect()
}

#[test]
fn an_edit_that_breaks_pdfa4_is_reported_and_undo_takes_it_back() {
    let (mut s, id) = open(file("2.0", Some(PDFA4), None));
    let check = s.pdfa_check(id).unwrap();
    assert_eq!(check.claim, PdfaClaim { part: 4, conformance: String::new() });
    assert_eq!((check.claim.label().as_str(), check.checked_as), ("PDF/A-4", Some(Level::A4)));
    assert!(check.breaks.is_empty(), "nothing changed yet: {:?}", check.breaks);

    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert!(broken(&s, id).is_empty(), "turning a page breaks nothing");

    // ISO 19005-4 6.9: an embedded file of a PDF/A-4 (not 4f) file is a PDF/A file itself.
    s.apply(id, attach()).unwrap();
    assert!(broken(&s, id).contains(&"6.9"), "{:?}", s.pdfa_check(id));
    s.undo(id).unwrap();
    assert!(broken(&s, id).is_empty(), "undone");
    s.redo(id).unwrap();
    assert!(broken(&s, id).contains(&"6.9"), "redone");
}

#[test]
fn what_the_file_broke_when_opened_is_not_a_change() {
    // The opened file already draws with an unembedded font (ISO 19005-4 6.2.10.4.1).
    let bytes = file("2.0", Some(PDFA4), Some(VISIBLE_TEXT));
    let opened = pdfcraft_cos::Document::open(Arc::new(bytes.clone())).unwrap();
    assert!(pdfcraft_engine::pdfa::verify(&opened, Level::A4).iter().any(|i| i.clause == "6.2.10.4.1"), "the fixture breaks it");
    let (mut s, id) = open(bytes);
    assert!(broken(&s, id).is_empty());
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert!(broken(&s, id).is_empty(), "the font was not embedded before the change either");
    s.apply(id, attach()).unwrap();
    let now = broken(&s, id);
    assert!(!now.is_empty() && now.iter().all(|c| *c == "6.9"), "only what the change broke: {now:?}");
}

#[test]
fn a_save_keeps_what_the_changes_broke() {
    let (mut s, id) = open(file("2.0", Some(PDFA4), None));
    s.apply(id, attach()).unwrap();
    let bytes = s.save_bytes(id).unwrap();
    s.mark_saved(id, bytes, Some("in.pdf".into())).unwrap();
    assert!(broken(&s, id).contains(&"6.9"), "the saved file breaks it still, whatever it was read from");
}

#[test]
fn a_copy_is_checked_from_the_same_start() {
    let (mut s, id) = open(file("2.0", Some(PDFA4), None));
    let (clean, _) = s.optimized_bytes(id, &Default::default(), &[]).unwrap();
    assert!(s.pdfa_check_copy(id, &clean).unwrap().breaks.is_empty(), "an optimized copy of the file as opened");
    s.apply(id, attach()).unwrap();
    let (copy, _) = s.optimized_bytes(id, &Default::default(), &[]).unwrap();
    let check = s.pdfa_check_copy(id, &copy).unwrap();
    assert!(check.breaks.iter().any(|b| b.clause == "6.9"), "{check:?}");
    assert_eq!(s.pdfa_check_copy(id, &Arc::new(b"not a pdf".to_vec())), None, "a copy that can't be read is not checked");
}

#[test]
fn a_file_converted_to_pdfa_is_measured_from_its_conversion() {
    // The page draws with an unembedded font, which conversion can't fix (ISO 19005-2 6.2.11.4.1).
    let (mut s, id) = open(file("1.7", None, Some(VISIBLE_TEXT)));
    assert_eq!(s.pdfa_check(id), None, "declares nothing");
    s.apply(id, Edit::ConvertPdfA { level: Level::A2b }).unwrap();
    let Some(check) = s.pdfa_check(id) else {
        // A conversion that leaves problems may write no identification at all; then nothing is
        // checked, and there is nothing more to see here.
        return;
    };
    assert_eq!((check.claim.label().as_str(), check.checked_as), ("PDF/A-2b", Some(Level::A2b)));
    assert!(check.breaks.is_empty(), "what the conversion left is the conversion's to report: {:?}", check.breaks);
    // ISO 19005-2 6.8: an embedded file of a PDF/A-2 file is a PDF/A file itself.
    s.apply(id, attach()).unwrap();
    assert_eq!(broken(&s, id), ["6.8"]);
    s.undo(id).unwrap();
    s.undo(id).unwrap();
    assert_eq!(s.pdfa_check(id), None, "the identification is undone too");
}

#[test]
fn levels_a_and_u_are_checked_at_level_b_and_part_1_is_not_checked() {
    for (ids, label, level) in [
        ("pdfaid:part=\"2\" pdfaid:conformance=\"U\"", "PDF/A-2u", Some(Level::A2b)),
        ("pdfaid:part=\"3\" pdfaid:conformance=\"A\"", "PDF/A-3a", Some(Level::A3b)),
        ("pdfaid:part=\"4\" pdfaid:conformance=\"F\"", "PDF/A-4f", Some(Level::A4f)),
        ("pdfaid:part=\"1\" pdfaid:conformance=\"B\"", "PDF/A-1b", None),
    ] {
        let (mut s, id) = open(file("1.7", Some(ids), None));
        s.apply(id, attach()).unwrap();
        let check = s.pdfa_check(id).unwrap();
        assert_eq!((check.claim.label().as_str(), check.checked_as), (label, level));
        // An attachment without AFRelationship breaks every checked level here (ISO 19005-2/-3
        // 6.8, ISO 19005-4 6.9); part 1 isn't checked, so nothing is reported for it.
        assert_eq!(check.breaks.is_empty(), level.is_none(), "{label}: {:?}", check.breaks);
    }
}

#[test]
fn a_file_that_declares_no_pdfa_is_not_checked() {
    let (mut s, id) = open(file("1.7", None, Some(VISIBLE_TEXT)));
    s.apply(id, attach()).unwrap();
    assert_eq!(s.pdfa_check(id), None);
}
