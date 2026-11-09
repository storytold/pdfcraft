//! #2539 review: the PDF/A and PDF/UA identification of a catalog XMP packet the metadata check
//! removes is written back in a packet of its own, or reported lost.

use super::*;
use crate::{Claim, Identification};

const DOC_XMP_HEAD: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#;
const DOC_XMP_TAIL: &str = r#"</rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;

/// One page showing "Codename BLUEFIN report", whose catalog XMP is `xmp` (raw bytes).
fn page(xmp: &[u8]) -> Document {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (Codename BLUEFIN report) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("/Type /Metadata /Subtype /XML", xmp),
        b"<< /Producer (Test) >>".to_vec(),
    ];
    Document::open(Arc::new(Pdf::new().objs(objs).trailer_entry("/Info 8 0 R").build())).unwrap()
}

/// Apply a mark over "BLUEFIN" and return the report and the catalog's XMP text afterwards.
fn redact(doc: &mut Document) -> (Report, Option<String>) {
    mark(doc, 0, &[[55.0, 95.0, 90.0, 110.0]], "");
    let r = apply(doc, None).unwrap();
    let xmp = doc.get(doc.root().unwrap()).as_dict().and_then(|c| c.get(b"Metadata").cloned());
    let text = xmp.and_then(|x| crate::metadata::xmp_text(doc, &x));
    (r, text)
}

/// The packet that names the redacted word goes, and one holding only its PDF/A and PDF/UA
/// identification takes its place: the PDF/A properties written as attributes, PDF/UA's as an
/// element under a prefix of its own, both found by namespace. Nothing else is copied.
#[test]
fn the_identification_of_a_removed_packet_is_written_back() {
    let body = r#"<rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="2" pdfaid:conformance="B"/><rdf:Description rdf:about="" xmlns:ua="http://www.aiim.org/pdfua/ns/id/"><ua:part>1</ua:part></rdf:Description><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Bluefin report</dc:title><dc:creator>Ada Lovelace</dc:creator></rdf:Description>"#;
    let mut doc = page(format!("{DOC_XMP_HEAD}{body}{DOC_XMP_TAIL}").as_bytes());
    let (r, text) = redact(&mut doc);
    assert_eq!(r.identification, Identification { pdfa: Claim::Kept, pdfua: Claim::Kept }, "{r:?}");
    assert_eq!(r.metadata, 1, "{r:?}");
    let text = text.expect("a packet is written back");
    assert!(!text.to_lowercase().contains("bluefin") && !text.contains("Lovelace"), "{text}");
    let xml = roxmltree::Document::parse(&text).expect("well-formed");
    let value = |ns: &str, local: &str| {
        xml.descendants().find(|n| n.tag_name().namespace() == Some(ns) && n.tag_name().name() == local).and_then(|n| n.text()).map(str::to_string)
    };
    let a = "http://www.aiim.org/pdfa/ns/id/";
    let ua = "http://www.aiim.org/pdfua/ns/id/";
    assert_eq!((value(a, "part").as_deref(), value(a, "conformance").as_deref(), value(ua, "part").as_deref()), (Some("2"), Some("B"), Some("1")));
    // PDF/A-2 accepts pdfuaid only through an extension schema describing it.
    assert_eq!(value("http://www.aiim.org/pdfa/ns/schema#", "prefix").as_deref(), Some("pdfuaid"), "{text}");
}

/// A packet without identification is removed as before, and nothing is said about it.
#[test]
fn a_packet_without_identification_is_only_removed() {
    let body = r#"<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Bluefin report</dc:title></rdf:Description>"#;
    let mut doc = page(format!("{DOC_XMP_HEAD}{body}{DOC_XMP_TAIL}").as_bytes());
    let (r, text) = redact(&mut doc);
    assert_eq!((r.identification, text), (Identification::default(), None), "{r:?}");
}

/// The identification is lost, and said to be, when its value isn't the code it must be (a
/// conformance level that is a word) or when the packet can't be read (here it isn't UTF-8).
#[test]
fn an_identification_that_cannot_be_kept_is_reported_lost() {
    let wordy = r#"<rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="2" pdfaid:conformance="Bluefin"/>"#;
    let mut doc = page(format!("{DOC_XMP_HEAD}{wordy}{DOC_XMP_TAIL}").as_bytes());
    let (r, text) = redact(&mut doc);
    assert_eq!((r.identification, text), (Identification { pdfa: Claim::Lost, pdfua: Claim::None }, None), "{r:?}");

    let latin1 = [
        DOC_XMP_HEAD.as_bytes(),
        br#"<rdf:Description rdf:about="" xmlns:pdfuaid="http://www.aiim.org/pdfua/ns/id/" pdfuaid:part="1" dc:title="Bluefin caf\xE9"/>"#,
        DOC_XMP_TAIL.as_bytes(),
    ]
    .concat();
    let mut doc = page(&latin1);
    let (r, text) = redact(&mut doc);
    assert_eq!((r.identification, text), (Identification { pdfa: Claim::None, pdfua: Claim::Lost }, None), "{r:?}");
    assert!(r.identification.lost());
}

/// When the removed text couldn't be searched for, every catalog packet goes, but a part number
/// and a conformance letter, one letter or digit each, can't hold it and are written back; a
/// revision year could, and isn't.
#[test]
fn unsearched_text_keeps_only_one_character_codes() {
    let body = r#"<rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="4" pdfaid:rev="2020"/><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Annual figures</dc:title></rdf:Description>"#;
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (ABC) Tj ET"),
        // Glyph names outside the glyph list: the removed text isn't known.
        FONT.replace("95 0 R", "6 0 R").replace(" /Widths", " /Encoding << /Differences [65 /g17 /g18 /g19] >> /Widths").into_bytes(),
        widths(),
        stream("/Type /Metadata /Subtype /XML", format!("{DOC_XMP_HEAD}{body}{DOC_XMP_TAIL}").as_bytes()),
    ];
    let mut doc = Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap();
    mark(&mut doc, 0, &[[5.0, 95.0, 200.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.identification, Identification { pdfa: Claim::Kept, pdfua: Claim::None }, "{r:?}");
    let xmp = doc.get(doc.root().unwrap()).as_dict().and_then(|c| c.get(b"Metadata").cloned()).unwrap();
    let text = crate::metadata::xmp_text(&doc, &xmp).unwrap();
    assert!(text.contains("<pdfaid:part>4</pdfaid:part>") && !text.contains("2020") && !text.contains("Annual"), "{text}");
}

/// The extension schema PDF/A-2 needs for pdfuaid is fixed text, but if the redacted words are
/// among its words ("schema" here) it isn't written, and the PDF/UA claim is reported lost.
#[test]
fn the_extension_schema_is_left_out_when_it_uses_a_redacted_word() {
    let body = r#"<rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="2" pdfaid:conformance="U"/><rdf:Description rdf:about="" xmlns:pdfuaid="http://www.aiim.org/pdfua/ns/id/" pdfuaid:part="1"/><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Schema notes</dc:title></rdf:Description>"#;
    let mut doc = page(format!("{DOC_XMP_HEAD}{body}{DOC_XMP_TAIL}").as_bytes());
    let s = crate::metadata::scrub_within(&mut doc, &["SCHEMA".into()], true, 1 << 28).unwrap();
    assert_eq!(s.identification, Identification { pdfa: Claim::Kept, pdfua: Claim::Lost });
    let xmp = doc.get(doc.root().unwrap()).as_dict().and_then(|c| c.get(b"Metadata").cloned()).unwrap();
    let text = crate::metadata::xmp_text(&doc, &xmp).unwrap();
    assert!(!text.to_lowercase().contains("schema") && text.contains("<pdfaid:conformance>U</pdfaid:conformance>"), "{text}");
}
