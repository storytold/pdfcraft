//! A file that declares PDF/A-4 keeps its document metadata in XMP: a save, a signature or a
//! Document Properties change writes no `/Info`, unless the catalog has `/PieceInfo`, and then
//! `/ModDate` alone (ISO 19005-4 §6.1.3). Every other file is saved as before.

use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_engine::sign::{Modification, Status};
use pdfcraft_engine::{Edit, EditError, Session, SignOptions};

/// 1_700_000_000 seconds: the time the sessions below save at.
const NOW: &str = "2023-11-14T22:13:20Z";
const PDF_NOW: &str = "D:20231114221320Z";

/// An XMP packet declaring `ids` (attributes of the identification description), with a title.
fn packet(ids: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" {ids}/>\n  \
         <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
         <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Old title</rdf:li></rdf:Alt></dc:title></rdf:Description>\n \
         </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>"
    )
}

const PDFA4: &str = "pdfaid:part=\"4\" pdfaid:rev=\"2020\"";

/// A one-page PDF 2.0 file: `catalog_extra` goes into the catalog, `xmp` (if any) is its
/// metadata stream, and `info` (if any) the trailer's document information dictionary.
fn file(catalog_extra: &str, xmp: Option<&str>, info: Option<&str>) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R{} {catalog_extra} >>", if xmp.is_some() { " /Metadata 4 0 R" } else { "" }),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>".to_string(),
    ];
    if let Some(x) = xmp {
        objs.push(format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{x}\nendstream", x.len()));
    }
    let info_ref = info.map(|i| {
        objs.push(i.to_string());
        objs.len()
    });
    let mut pdf = b"%PDF-2.0\n%\xE2\xE3\xCF\xD3\n".to_vec();
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
    let info_entry = info_ref.map(|n| format!(" /Info {n} 0 R")).unwrap_or_default();
    let id = "/ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>]";
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R{info_entry} {id} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    pdf
}

fn pdfa4() -> Vec<u8> {
    file("", Some(&packet(PDFA4)), None)
}

fn open(bytes: Vec<u8>) -> (Session, pdfcraft_engine::DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("in.pdf", None, Arc::new(bytes), None).unwrap();
    (s, id)
}

/// The saved file's trailer `/Info`, as text (`None`: no `/Info`), and its XMP packet.
fn read(bytes: &Arc<Vec<u8>>) -> (Option<String>, String) {
    let doc = Document::open(bytes.clone()).unwrap();
    let info = doc.trailer().get(b"Info").map(|o| info_text(&doc, o));
    let root = doc.root().unwrap();
    let meta = doc.get(root).as_dict().unwrap().reference(b"Metadata").unwrap();
    let xmp = match &*doc.get(meta) {
        pdfcraft_cos::Object::Stream(s) => String::from_utf8(s.decoded_within(1 << 20).unwrap()).unwrap(),
        other => panic!("not a stream: {other:?}"),
    };
    (info, xmp)
}

/// A document information dictionary as `Key=value;` entries.
fn info_text(doc: &Document, info: &pdfcraft_cos::Object) -> String {
    let info = doc.resolve(info);
    let entries = info.as_dict().unwrap().iter();
    entries.map(|(k, v)| format!("{}={};", String::from_utf8_lossy(k), doc.resolve(v).as_string().map(|s| s.to_text()).unwrap_or_default())).collect()
}

fn dated(xmp: &str) -> bool {
    xmp.contains(&format!("<xmp:ModifyDate>{NOW}</xmp:ModifyDate>")) && xmp.contains(&format!("<xmp:MetadataDate>{NOW}</xmp:MetadataDate>"))
}

#[test]
fn a_pdfa4_save_dates_the_xmp_and_writes_no_info() {
    let (mut s, id) = open(pdfa4());
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    for saved in [s.save_bytes(id).unwrap(), s.save_full_bytes(id).unwrap()] {
        let (info, xmp) = read(&saved);
        assert_eq!(info, None, "no /Info");
        assert!(dated(&xmp), "{xmp}");
        assert!(xmp.contains("pdfaid:part=\"4\"") && xmp.contains("Old title"), "the rest of the packet stays: {xmp}");
    }
    // The open document is not changed by saving it.
    let saved = s.save_bytes(id).unwrap();
    s.mark_saved(id, saved, None).unwrap();
    assert!(!s.get(id).unwrap().dirty);
}

#[test]
fn a_pdfa4_property_change_writes_xmp_only() {
    let (mut s, id) = open(pdfa4());
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "New title".into() }).unwrap();
    s.apply(id, Edit::SetInfo { key: "Author".into(), value: "Ada".into() }).unwrap();
    assert_eq!(s.get(id).unwrap().info_value("Title").as_deref(), Some("New title"));
    let (info, xmp) = read(&s.save_bytes(id).unwrap());
    assert_eq!(info, None, "no /Info");
    assert!(xmp.contains(">New title<") && !xmp.contains("Old title") && xmp.contains("<dc:creator><rdf:Seq><rdf:li>Ada</rdf:li>"), "{xmp}");
    // Undo puts the packet back.
    s.undo(id).unwrap();
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().info_value("Title").as_deref(), Some("Old title"));
}

#[test]
fn a_pdfa4_file_refuses_what_xmp_cant_hold_here() {
    let (mut s, id) = open(pdfa4());
    for key in ["Project", "Trapped", "CreationDate", "ModDate"] {
        let err = s.apply(id, Edit::SetInfo { key: key.into(), value: "v".into() }).unwrap_err();
        assert!(matches!(&err, EditError::Organize(e) if e.to_string().contains("PDF/A-4")), "{key}: {err}");
    }
    assert!(!s.get(id).unwrap().dirty, "nothing changed");
    // Clearing what isn't there changes nothing either, and is no error.
    s.apply(id, Edit::SetInfo { key: "Project".into(), value: String::new() }).unwrap();
    let (info, _) = read(&s.save_bytes(id).unwrap());
    assert_eq!(info, None);
}

#[test]
fn an_earlier_info_loses_the_entry_a_change_replaces_and_nothing_else() {
    let info = "<< /Title (Info title) /Producer (Old writer) >>";
    let (mut s, id) = open(file("", Some(&packet(PDFA4)), Some(info)));
    // A save stamps no /ModDate into it.
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let (kept, xmp) = read(&s.save_bytes(id).unwrap());
    let kept = kept.unwrap();
    assert!(kept.contains("Info title") && kept.contains("Old writer") && !kept.contains("ModDate"), "{kept}");
    assert!(dated(&xmp), "{xmp}");
    // A change takes the entry it replaces out; the rest stays.
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "New title".into() }).unwrap();
    let (kept, xmp) = read(&s.save_bytes(id).unwrap());
    let kept = kept.unwrap();
    assert!(!kept.contains("Info title") && kept.contains("Old writer"), "{kept}");
    assert!(xmp.contains(">New title<"), "{xmp}");
    // Clearing the last entry takes the empty /Info away.
    s.apply(id, Edit::SetInfo { key: "Producer".into(), value: String::new() }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    assert_eq!(read(&saved).0, None);
    // Incrementally: the original revision is still there, untouched.
    let original = file("", Some(&packet(PDFA4)), Some(info));
    assert_eq!(&saved[..original.len()], &original[..]);
}

#[test]
fn with_piece_info_a_pdfa4_save_keeps_only_mod_date_in_info() {
    let (mut s, id) = open(file("/PieceInfo << /PdfCraftTest << /LastModified (D:20200101000000Z) /Private 1 >> >>", Some(&packet(PDFA4)), None));
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    let (info, xmp) = read(&s.save_bytes(id).unwrap());
    let info = info.unwrap();
    assert_eq!(info, format!("ModDate={PDF_NOW};"));
    assert!(dated(&xmp), "{xmp}");
}

#[test]
fn other_files_are_saved_as_before() {
    for ids in [None, Some("pdfaid:part=\"2\" pdfaid:conformance=\"B\""), Some("pdfaid:part=\"3\" pdfaid:conformance=\"U\"")] {
        let xmp = ids.map(packet);
        let (mut s, id) = open(file("", xmp.as_deref(), None));
        s.apply(id, Edit::SetInfo { key: "Title".into(), value: "New title".into() }).unwrap();
        let saved = s.save_bytes(id).unwrap();
        let doc = Document::open(saved.clone()).unwrap();
        let info = info_text(&doc, doc.trailer().get(b"Info").unwrap());
        assert_eq!(info, format!("Title=New title;ModDate={PDF_NOW};"), "{ids:?}");
        if xmp.is_some() {
            let (_, packet) = read(&saved);
            assert!(!packet.contains("ModifyDate"), "the packet isn't dated: {packet}");
        }
    }
}

#[test]
fn signing_a_pdfa4_file_writes_no_info() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = pdfcraft_engine::sign::pkcs12::open(&p12, "test").unwrap();
    // After the test certificate's start date (2026-10-02).
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("in.pdf", None, Arc::new(pdfa4()), None).unwrap();
    let signed = s.sign(id, &digital_id, SignOptions::default()).unwrap();
    let (info, xmp) = read(&signed);
    assert_eq!(info, None, "no /Info");
    // The signing time, with the session's offset from UTC: 2027-01-15T08:00:00Z.
    assert!(xmp.contains("<xmp:ModifyDate>2027-01-1"), "{xmp}");
    s.mark_signed(id, signed, None).unwrap();
    let sig = s.get(id).unwrap().signatures[0].clone();
    assert_eq!((sig.status == Status::Invalid, &sig.modification), (false, &Modification::None), "{sig:?}");
    // A later save dates the packet again: a metadata change, which every signature permits.
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "After signing".into() }).unwrap();
    let saved = s.save_bytes(id).unwrap();
    assert_eq!(read(&saved).0, None, "no /Info");
    s.mark_saved(id, saved, None).unwrap();
    let sig = s.get(id).unwrap().signatures[0].clone();
    assert!(sig.status != Status::Invalid && matches!(sig.modification, Modification::Allowed(_)), "{sig:?}");
}

#[test]
fn an_optimized_pdfa4_copy_has_no_info() {
    let (s, id) = open(pdfa4());
    let job = s.optimize_job(id, &pdfcraft_engine::optimize::Settings::default(), &[]).unwrap();
    let (bytes, _) = job.run(|_| true).unwrap();
    let (info, xmp) = read(&bytes);
    assert_eq!(info, None, "no /Info");
    assert!(dated(&xmp), "{xmp}");
}
