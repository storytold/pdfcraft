//! #2539 review: what applying leaves as it is but still holds the removed text is counted in
//! [`Report::still_holding`], and nothing of it is changed.

use super::*;

const XMP_BLUEFIN: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:description>Photo for Bluefin</dc:description></rdf:Description></rdf:RDF></x:xmpmeta>"#;
const XMP_OTHER: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Annual figures</dc:title></rdf:Description></rdf:RDF></x:xmpmeta>"#;

/// The objects whose text the check reads, by number.
const HELD: [u32; 20] = [7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26];

/// A page showing "Codename BLUEFIN report" (BLUEFIN at x 55–90) with `font`, and the word, or
/// other text, in a bookmark, attachments, comments and XMP the mark doesn't touch:
/// - bookmarks: "Bluefin plans", "Annual figures", and under it "BLUEFIN appendix";
/// - attachments: one named `bluefin.txt`, one described "About Bluefin", one with neither, and
///   a file attachment annotation for `Bluefin.pdf`;
/// - comments: one whose text mentions it, one by "Bluefin Team", one whose rich text does, one
///   that doesn't, and a pop-up repeating it (pop-ups aren't counted);
/// - XMP: an image's mentions it, a font's doesn't.
fn page_with_places(font: &str) -> Document {
    let annot = |rest: &str| format!("<< /Type /Annot /Rect [200 200 220 220] {rest} >>").into_bytes();
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 7 0 R /Names << /EmbeddedFiles 11 0 R >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im 24 0 R >> >> /Annots [16 0 R 17 0 R 18 0 R 19 0 R 20 0 R 21 0 R] >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td (Codename BLUEFIN report) Tj ET q 10 0 0 10 200 250 cm /Im Do Q"),
        format!("{} /Metadata 26 0 R >>", font.replace("95 0 R", "6 0 R").strip_suffix(">>").unwrap()).into_bytes(),
        widths(),
        // 7–10: the outline.
        b"<< /Type /Outlines /First 8 0 R /Last 9 0 R /Count 3 >>".to_vec(),
        b"<< /Title (Bluefin plans) /Parent 7 0 R /Next 9 0 R >>".to_vec(),
        b"<< /Title (Annual figures) /Parent 7 0 R /Prev 8 0 R /First 10 0 R /Last 10 0 R /Count 1 >>".to_vec(),
        b"<< /Title (BLUEFIN appendix) /Parent 9 0 R >>".to_vec(),
        // 11–15: the attachments' name tree and file specifications.
        b"<< /Kids [12 0 R] >>".to_vec(),
        b"<< /Limits [(a.txt) (bluefin.txt)] /Names [(a.txt) 13 0 R (b.txt) 14 0 R (bluefin.txt) 15 0 R] >>".to_vec(),
        b"<< /Type /Filespec /F (a.txt) /UF (a.txt) >>".to_vec(),
        b"<< /Type /Filespec /F (b.txt) /Desc (About Bluefin) >>".to_vec(),
        b"<< /Type /Filespec /F (c.txt) >>".to_vec(),
        // 16–21: the annotations.
        annot("/Subtype /Text /Contents (Ask about bluefin) /T (Ada)"),
        annot("/Subtype /Text /Contents (Seen) /T (Bluefin Team)"),
        annot("/Subtype /FreeText /Contents (plain) /RC (<body><p>BLUEFIN</p></body>) /DA (/F1 10 Tf)"),
        annot("/Subtype /Text /Contents (Looks good) /T (Ada) /Popup 21 0 R"),
        annot("/Subtype /FileAttachment /Contents (Attached) /T (Ada) /FS << /Type /Filespec /UF (Bluefin.pdf) >>"),
        annot("/Subtype /Popup /Parent 19 0 R /Contents (bluefin)"),
        // 22–23: unused.
        b"null".to_vec(),
        b"null".to_vec(),
        // 24–26: an image with XMP that mentions it, and the two packets.
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Metadata 25 0 R", b"\x80"),
        stream("/Type /Metadata /Subtype /XML", XMP_BLUEFIN.as_bytes()),
        stream("/Type /Metadata /Subtype /XML", XMP_OTHER.as_bytes()),
    ];
    pdf(objs)
}

fn held(doc: &Document) -> Vec<Object> {
    HELD.iter().map(|&n| (*doc.get(ObjRef::new(n, 0))).clone()).collect()
}

#[test]
fn places_apply_leaves_that_still_hold_the_removed_text_are_counted_and_kept() {
    let mut doc = page_with_places(FONT);
    let before = held(&doc);
    mark(&mut doc, 0, &[[55.0, 95.0, 90.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 7, "{r:?}");
    assert_eq!(r.still_holding, Elsewhere { bookmarks: 2, attachments: 3, comments: 3, xmp: 1, unsearched: false }, "{r:?}");
    assert_eq!(r.still_holding.total(), 9);
    assert_eq!(held(&doc), before, "counting changes nothing");
    let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    assert!(catalog.contains(b"Outlines") && catalog.contains(b"Names"));
    assert_eq!(annots_of(&doc, &pdfcraft_model::pages(&doc)[0].dict).len(), 6, "the comments stay");
}

/// Removed text that is unrelated to every place counts nothing.
#[test]
fn places_without_the_removed_text_are_not_counted() {
    let mut doc = page_with_places(FONT);
    // "Codename" alone, at x 10–50.
    mark(&mut doc, 0, &[[8.0, 95.0, 50.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 8, "{r:?}");
    assert_eq!(r.still_holding, Elsewhere::default(), "{r:?}");
}

/// When the removed text can't be known (a removed glyph whose text isn't known), every place
/// that holds any text counts, and says why.
#[test]
fn places_count_whenever_they_hold_text_if_the_removed_text_is_unknown() {
    let unnamed = FONT.replace(" /Widths", " /Encoding << /Differences [66 /g17 /g18 /g19 /g20 /g21 /g22 /g23 /g24 /g25 /g26 /g27 /g28 /g29 /g30 /g31 /g32 /g33 /g34 /g35 /g36 /g37 /g38 /g39 /g40 /g41 /g42] >> /Widths");
    let mut doc = page_with_places(&unnamed);
    let before = held(&doc);
    mark(&mut doc, 0, &[[55.0, 95.0, 90.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    // Every bookmark title, every attachment (each has a name), every comment but the pop-up,
    // and both XMP packets (the font's too).
    assert_eq!(r.still_holding, Elsewhere { bookmarks: 3, attachments: 4, comments: 5, xmp: 2, unsearched: true }, "{r:?}");
    assert_eq!(held(&doc), before);
}

/// A bookmark list that loops back on itself is walked once.
#[test]
fn a_looping_outline_is_read_once() {
    let mut doc = page_with_places(FONT);
    // The last item's /Next goes back to the first.
    doc.update_dict(ObjRef::new(9, 0), |d| d.set(b"Next".to_vec(), Object::Ref(ObjRef::new(8, 0)))).unwrap();
    doc.update_dict(ObjRef::new(10, 0), |d| d.set(b"First".to_vec(), Object::Ref(ObjRef::new(7, 0)))).unwrap();
    mark(&mut doc, 0, &[[55.0, 95.0, 90.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.still_holding.bookmarks, 2, "{r:?}");
}
