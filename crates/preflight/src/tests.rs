use super::*;

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

/// A page with embedded-font-free text, a note without the print flag, a link with a
/// JavaScript action, an interpolated image, and a document JavaScript.
fn messy() -> Document {
    build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript << /Names [(x) 9 0 R] >> >> /AA << /WC 9 0 R >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im 8 0 R >> >> /Annots [6 0 R 7 0 R] >>",
            "<< /Length 44 >>\nstream\n1 0 0 rg BT /F1 12 Tf 10 10 Td (Hi) Tj ET /Im Do\nendstream",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /P 3 0 R /AP << /N 10 0 R >> >>",
            "<< /Type /Annot /Subtype /Link /Rect [40 10 80 30] /P 3 0 R /F 4 /A 9 0 R >>",
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate true /Length 1 >>\nstream\n\u{0}\nendstream",
            "<< /Type /Action /S /JavaScript /JS (app.alert(1)) >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream",
        ],
        "/Info << /Title (Report) /Producer (Test) /CreationDate (D:20240105093000Z) >>",
    )
}

fn clauses(issues: &[Issue]) -> Vec<&'static str> {
    let mut c: Vec<&str> = issues.iter().map(|i| i.clause).collect();
    c.dedup();
    c
}

#[test]
fn verify_reports_what_breaks_pdfa() {
    let issues = verify(&messy(), Level::A2b);
    let msgs: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
    for want in [
        "The document has no XMP metadata",
        "Device colour is used, but there is no PDF/A output intent",
        "The document has additional actions (/AA)",
        "The document has document-level JavaScript",
        "A JavaScript action isn't allowed",
        "A Text annotation isn't set to print, or is hidden",
        "The font Helvetica isn't embedded",
        "An image asks to be interpolated",
    ] {
        assert!(msgs.contains(&want), "missing {want:?} in {msgs:#?}");
    }
    assert!(issues.iter().find(|i| i.message.starts_with("The font")).is_some_and(|i| !i.fixable));
    assert!(clauses(&issues).contains(&"6.6.2.1"));
}

#[test]
fn convert_fixes_all_but_fonts() {
    let mut doc = messy();
    let r = convert(&mut doc, Level::A2b).unwrap();
    assert!(r.fixed.iter().any(|f| f.contains("output intent")), "{:?}", r.fixed);
    let left: Vec<&str> = r.remaining.iter().map(|i| i.message.as_str()).collect();
    assert_eq!(left, ["The font Helvetica isn't embedded"], "{:#?}", r.remaining);
    let d = declared(&doc);
    assert_eq!(d.pdfa, Some((2, "B".into())));
    assert_eq!(d.output_intents, ["sRGB IEC61966-2.1"]);
    // The metadata carries the document information.
    let x = metadata(&doc).unwrap();
    assert_eq!(xmp::value(&x, "dc:title").as_deref(), Some("Report"));
    assert_eq!(xmp::value(&x, "xmp:CreateDate").as_deref(), Some("2024-01-05T09:30:00Z"));
    // It survives a save.
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let back = Document::open(std::sync::Arc::new(bytes)).unwrap();
    assert_eq!(verify(&back, Level::A2b).len(), 1);
}

#[test]
fn embedded_files_differ_between_parts() {
    let doc = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(a.txt) 4 0 R] >> >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Filespec /F (a.txt) /EF << /F 5 0 R >> >>",
            "<< /Type /EmbeddedFile /Length 2 >>\nstream\nhi\nendstream",
        ],
        "",
    );
    assert!(verify(&doc, Level::A2b).iter().any(|i| i.clause == "6.8" && !i.fixable));
    let mut d3 = doc.clone();
    assert!(verify(&d3, Level::A3b).iter().any(|i| i.message.contains("AFRelationship")));
    let r = convert(&mut d3, Level::A3b).unwrap();
    assert!(r.remaining.is_empty(), "{:#?}", r.remaining);
}
