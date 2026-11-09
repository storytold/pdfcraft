//! #2539 review: each way the metadata check fails closed, end to end. When the removed text
//! can't be known in full, every `/Info` string and both the catalog's and the page's XMP go;
//! when one XMP packet can't be read, that packet goes and the rest of the metadata is checked
//! as usual.

use super::*;

const CLEAN_XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Annual figures</dc:title></rdf:Description></rdf:RDF></x:xmpmeta>"#;

/// One page showing `content`, with `/Info` (two strings and a name), catalog XMP (object 7)
/// and page XMP (object 8). None of the metadata mentions anything the tests remove.
fn page_with_metadata(content: &[u8], catalog_xmp: Vec<u8>) -> Document {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Metadata 8 0 R >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        catalog_xmp,
        stream("/Type /Metadata /Subtype /XML", CLEAN_XMP.as_bytes()),
        b"<< /Title (Annual figures) /Producer (Test) /Trapped /False >>".to_vec(),
    ];
    Document::open(Arc::new(Pdf::new().objs(objs).trailer_entry("/Info 9 0 R").build())).unwrap()
}

fn clean_xmp() -> Vec<u8> {
    stream("/Type /Metadata /Subtype /XML", CLEAN_XMP.as_bytes())
}

fn catalog_has_xmp(doc: &Document) -> bool {
    doc.get(doc.root().unwrap()).as_dict().is_some_and(|c| c.contains(b"Metadata"))
}

fn page_has_xmp(doc: &Document) -> bool {
    pdfcraft_model::pages(doc)[0].dict.contains(b"Metadata")
}

/// Mark the line at y 100 across the page and apply.
fn redact_line(doc: &mut Document) -> Report {
    mark(doc, 0, &[[0.0, 95.0, 300.0, 110.0]], "");
    apply(doc, None).unwrap()
}

/// Every `/Info` string and both XMP streams went, though none of them mentions what was removed.
fn assert_cleared(doc: &Document, r: &Report) {
    assert_eq!(r.metadata, 4, "{r:?}");
    assert_eq!(info_keys(doc), ["Trapped"], "strings go, the name stays");
    assert!(!catalog_has_xmp(doc) && !page_has_xmp(doc), "both XMP streams go");
}

/// Control for the tests below: removing a short word that the metadata doesn't mention keeps
/// all of it.
#[test]
fn metadata_that_does_not_mention_the_removed_text_stays() {
    let mut doc = page_with_metadata(b"BT /F1 10 Tf 10 100 Td (Bluefin) Tj ET", clean_xmp());
    let r = redact_line(&mut doc);
    assert_eq!(r.metadata, 0, "{r:?}");
    assert_eq!(info_keys(&doc), ["Title", "Producer", "Trapped"]);
    assert!(catalog_has_xmp(&doc) && page_has_xmp(&doc));
}

/// A single removed run longer than the collection keeps (64 KiB): the part past the limit is
/// never seen. The run is one letter, 70,000 spaces (one space once normalized) and another
/// letter, so what would be collected without the limit is short enough to search, and only
/// the limit itself clears the metadata.
#[test]
fn a_removed_run_over_64_kib_clears_the_metadata() {
    // At 0.004 pt each glyph advances 0.002 pt, so all 70,002 fit in 141 pt of one line.
    let text = format!("A{}B", " ".repeat(70_000));
    let content = format!("BT /F1 0.004 Tf 10 100 Td ({text}) Tj ET");
    let mut doc = page_with_metadata(content.as_bytes(), clean_xmp());
    let r = redact_line(&mut doc);
    assert_eq!(r.glyphs, 70_002, "{r:?}");
    assert_cleared(&doc, &r);
}

/// More than 20,000 distinct terms: each removed run is one word, far enough from the last to
/// start a run of its own, and each word is different.
#[test]
fn more_than_20000_terms_clear_the_metadata() {
    let mut content = b"BT /F1 10 Tf 10 100 Td".to_vec();
    // Words alternate between x 10 and x 70; each is 30 pt wide, so the gap between two is
    // 30 pt, more than a run spans.
    for i in 0..20_001u32 {
        let x = if i % 2 == 0 { 60 } else { -60 };
        content.extend_from_slice(format!(" (q{i:05}) Tj {x} 0 Td").as_bytes());
    }
    content.extend_from_slice(b" ET");
    let mut doc = page_with_metadata(&content, clean_xmp());
    let r = redact_line(&mut doc);
    assert_eq!(r.glyphs, 20_001 * 6, "{r:?}");
    assert_cleared(&doc, &r);
}

/// A removed run longer than 4,096 characters can't be searched for whole.
#[test]
fn a_removed_run_over_4096_characters_clears_the_metadata() {
    // 4,100 letters at 0.05 pt advance 0.025 pt each: 103 pt in all, one run, 4 KiB.
    let content = format!("BT /F1 0.05 Tf 10 100 Td ({}) Tj ET", "Z".repeat(4_100));
    let mut doc = page_with_metadata(content.as_bytes(), clean_xmp());
    let r = redact_line(&mut doc);
    assert_eq!(r.glyphs, 4_100, "{r:?}");
    assert_cleared(&doc, &r);
}

/// The catalog's packet can't be read: that packet goes, unread, while the page's packet and
/// `/Info`, which don't mention the removed word, stay.
fn assert_only_the_catalog_packet_went(catalog_xmp: Vec<u8>) {
    let mut doc = page_with_metadata(b"BT /F1 10 Tf 10 100 Td (Bluefin) Tj ET", catalog_xmp);
    let r = redact_line(&mut doc);
    assert_eq!(r.metadata, 1, "{r:?}");
    assert!(!catalog_has_xmp(&doc), "the packet that couldn't be read goes");
    assert!(page_has_xmp(&doc), "the readable packet is checked, and stays");
    assert_eq!(info_keys(&doc), ["Title", "Producer", "Trapped"]);
}

/// XMP whose Flate data is corrupt can't be decoded, so it can't be searched.
#[test]
fn an_xmp_stream_whose_filter_data_is_corrupt_goes() {
    assert_only_the_catalog_packet_went(stream("/Type /Metadata /Subtype /XML /Filter /FlateDecode", b"this is not deflate data at all"));
}

/// A valid Flate stream cut off half way decodes, leniently, to the first part of the packet;
/// the rest was never seen, so the packet isn't searched but goes.
#[test]
fn an_xmp_stream_whose_filter_data_is_cut_short_goes() {
    let full = Stream::flate(Dict::new(), CLEAN_XMP.as_bytes());
    let cut = full.raw.get(..full.raw.len() / 2).unwrap();
    assert_only_the_catalog_packet_went(stream("/Type /Metadata /Subtype /XML /Filter /FlateDecode", cut));
}

/// XMP that decodes to more than 16 MiB isn't read at all.
#[test]
fn an_xmp_stream_over_16_mib_goes() {
    // A valid packet, padded with whitespace past the limit.
    let padded = CLEAN_XMP.replace("</x:xmpmeta>", &format!("{}</x:xmpmeta>", " ".repeat(16 * 1024 * 1024)));
    assert_only_the_catalog_packet_went(stream("/Type /Metadata /Subtype /XML", padded.as_bytes()));
}

/// XMP that is neither UTF-8 nor UTF-16 (here Latin-1) isn't searched lossily.
#[test]
fn an_xmp_stream_in_another_encoding_goes() {
    let latin1 = CLEAN_XMP.replace("Annual figures", "Ann\u{e9}e").replace('\u{e9}', "\u{0}");
    let bytes: Vec<u8> = latin1.bytes().map(|b| if b == 0 { 0xE9 } else { b }).collect();
    assert!(std::str::from_utf8(&bytes).is_err());
    assert_only_the_catalog_packet_went(stream("/Type /Metadata /Subtype /XML", &bytes));
}

/// XMP behind two filters has no strict decoder here, so it isn't read either. The first filter
/// alone decodes to valid ASCII (the packet in hex), which must not be searched as the packet.
#[test]
fn an_xmp_stream_behind_two_filters_goes() {
    let hex: String = CLEAN_XMP.bytes().map(|b| format!("{b:02x}")).chain(std::iter::once(">".to_string())).collect();
    let Stream { raw, .. } = Stream::flate(Dict::new(), hex.as_bytes());
    assert_only_the_catalog_packet_went(stream("/Type /Metadata /Subtype /XML /Filter [/FlateDecode /ASCIIHexDecode]", &raw));
}

/// A form XObject under a mark whose content can't be decoded is removed without being read, so
/// what it showed is unknown (the deep-form case is `a_form_removed_unread_clears_the_metadata`).
#[test]
fn a_form_whose_content_cant_be_read_clears_the_metadata() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Fm 9 0 R >> >> /Metadata 8 0 R >>".to_vec(),
        stream("", b"q 1 0 0 1 0 0 cm /Fm Do Q"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        clean_xmp(),
        clean_xmp(),
        // 9: a form whose data no filter here decodes.
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Filter /NoSuchDecode", b"BT /F1 10 Tf 10 100 Td (Bluefin) Tj ET"),
        b"<< /Title (Annual figures) /Producer (Test) /Trapped /False >>".to_vec(),
    ];
    let mut doc = Document::open(Arc::new(Pdf::new().objs(objs).trailer_entry("/Info 10 0 R").build())).unwrap();
    let r = redact_line(&mut doc);
    assert_eq!(r.forms_removed, 1, "{r:?}");
    assert_cleared(&doc, &r);
}
