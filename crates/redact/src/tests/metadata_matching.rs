//! #2539 review (second round): spellings of the removed text the metadata check has to match.

use super::*;

fn dc_title(title: &str) -> String {
    format!(r#"<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title></rdf:Description>"#)
}

/// A tracked heading, its letters spread by `TJ` numbers rather than character spacing, is
/// collected as "B L U E F I N": its letters and digits still match `/Title (Bluefin)` and the
/// XMP title, while `/Producer` stays.
#[test]
fn a_heading_tracked_with_tj_numbers_matches_the_metadata() {
    let content = b"BT /F1 10 Tf 10 100 Td [(B) -300 (L) -300 (U) -300 (E) -300 (F) -300 (I) -300 (N)] TJ ET";
    let mut doc = with_metadata(content, "<< /Title (Bluefin) /Producer (Test) >>", &dc_title("Bluefin"));
    mark(&mut doc, 0, &[[5.0, 95.0, 200.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 7, "{r:?}");
    assert_eq!(info_keys(&doc), ["Producer"], "{r:?}");
    assert!(!has_xmp(&doc), "the XMP title named it");
    assert_eq!(r.metadata, 2, "{r:?}");
}

/// A value that spells the removed word with a space or a hyphen in it still holds it: "Blue fin"
/// and "BLUE-FIN" go when "BLUEFIN" is redacted, while "Bluefish", which only shares letters
/// with it, stays.
#[test]
fn a_value_spaced_or_hyphenated_otherwise_matches() {
    let info = "<< /Title (Blue fin) /Keywords (tuna, BLUE-FIN) /Subject (Bluefish) /Producer (Test) >>";
    let mut doc = with_metadata(b"BT /F1 10 Tf 10 100 Td (Codename BLUEFIN report) Tj ET", info, &dc_title("Blue Fin"));
    // "BLUEFIN" spans x 55-90.
    mark(&mut doc, 0, &[[55.0, 95.0, 90.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(info_keys(&doc), ["Subject", "Producer"], "{r:?}");
    assert!(!has_xmp(&doc), "the XMP title spelled it with a space");
    assert_eq!(r.metadata, 3, "{r:?}");
}

/// `s` as a UTF-16BE text string in hex, byte order mark first.
fn utf16be(s: &str) -> String {
    format!("<FEFF{}>", s.encode_utf16().map(|u| format!("{u:04X}")).collect::<String>())
}

/// The removed text and the metadata are compared after Unicode compatibility normalization
/// (NFKC) and lower case on both sides: a decomposed "Müller" (u + U+0308) in a UTF-16BE `/Title`
/// matches the composed one removed from the page, a "ﬁ" ligature from a `ToUnicode` map matches
/// "fi", and full-width letters match their ASCII forms.
#[test]
fn unicode_forms_of_the_removed_text_match() {
    let info = format!(
        "<< /Title {} /Subject (Finance plan) /Keywords {} /Author (Ada Lovelace) /Producer (Test) >>",
        utf16be("Mu\u{308}ller"),
        utf16be("\u{FF22}\u{FF2C}\u{FF35}\u{FF25}\u{FF26}\u{FF29}\u{FF2E}")
    );
    let mut doc = with_metadata(b"BT /F1 10 Tf 10 100 Td (x) Tj ET", &info, "");
    let removed = ["M\u{FC}ller".to_string(), "\u{FB01}nance".to_string(), "Bluefin".to_string()];
    let n = metadata::scrub_within(&mut doc, &removed, true, 1 << 28).unwrap().removed;
    assert_eq!(info_keys(&doc), ["Author", "Producer"]);
    assert_eq!(n, 3);
}

/// A value whose normalized form would be far longer than itself (U+FDFA is 18 letters in NFKC)
/// isn't normalized in full: it counts as holding the removed text (fail-closed).
#[test]
fn a_value_that_normalizes_to_far_more_text_counts_as_holding_it() {
    let info = format!("<< /Subject {} /Producer (Test) >>", utf16be(&"\u{FDFA}".repeat(50)));
    let mut doc = with_metadata(b"BT /F1 10 Tf 10 100 Td (x) Tj ET", &info, "");
    assert_eq!(metadata::scrub_within(&mut doc, &["Bluefin".into()], true, 1 << 28).unwrap().removed, 1);
    assert_eq!(info_keys(&doc), ["Producer"]);
}

/// `s` as UTF-16LE bytes in hex, behind an `FF FE` byte order mark.
fn utf16le(s: &str) -> String {
    format!("<FFFE{}>", s.encode_utf16().map(|u| format!("{:02X}{:02X}", u & 0xFF, u >> 8)).collect::<String>())
}

/// ISO 32000-2 §7.9.2.2.1 defines text strings in PDFDocEncoding, UTF-16BE (`FE FF`) and UTF-8
/// (`EF BB BF`) only, but a string behind an `FF FE` mark is UTF-16LE to anyone who reads it as
/// such; read through PDFDocEncoding, Cyrillic or CJK text in it could never match. It is
/// searched as UTF-16LE in `/Info` and in the places applying leaves (here a bookmark title).
#[test]
fn a_utf16le_string_is_searched_as_utf16le() {
    let info = format!("<< /Title {} /Subject {} /Producer (Test) >>", utf16le("Синий план"), utf16le("Годовой отчёт"));
    let mut doc = with_metadata(b"BT /F1 10 Tf 10 100 Td (x) Tj ET", &info, "");
    assert_eq!(metadata::scrub_within(&mut doc, &["СИНИЙ".into()], true, 1 << 28).unwrap().removed, 1);
    assert_eq!(info_keys(&doc), ["Subject", "Producer"]);

    // A bookmark titled the same way is counted as still holding it.
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>".to_vec(),
        b"<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 1 >>".to_vec(),
        format!("<< /Title {} /Parent 4 0 R /Dest [3 0 R /Fit] >>", utf16le("Синий план")).into_bytes(),
    ];
    let doc = Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap();
    assert_eq!(crate::elsewhere::find(&doc, &["СИНИЙ".into()], true), Elsewhere { bookmarks: 1, ..Default::default() });
}
