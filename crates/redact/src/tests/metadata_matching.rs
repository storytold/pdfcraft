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
