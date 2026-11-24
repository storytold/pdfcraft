//! The Header & Footer date through PPTX: an automatic date's format comes back on import, so the
//! dialog opens on the deck's own choice; a fixed date still comes back as its text.

use deckcraft_model::{Presentation, defaults};
use serde_json::json;

/// A deck whose Date placeholders are set up by the engine, exported and imported again.
fn round_trip(args: serde_json::Value) -> Presentation {
    let mut s = deckcraft_engine::Session::with_new();
    s.execute("design.headerFooter", &args).expect("headerFooter");
    let p = s.doc().expect("doc").doc.clone();
    let bytes = deckcraft_pptx::export(&p).expect("export");
    deckcraft_pptx::import(&bytes).expect("import")
}

#[test]
fn an_automatic_dates_format_survives_a_round_trip() {
    // A chosen format comes back as the remembered choice.
    let q = round_trip(json!({"date": true, "dateFormat": "datetime5", "all": true}));
    assert!(q.header_footer.date, "the date placeholder is there");
    assert_eq!(q.header_footer.date_format, "datetime5", "the format is remembered");
    assert_eq!(q.header_footer.date_text, "", "an automatic date has no fixed text");

    // A different one, so the test cannot pass by always reading datetime5.
    let q = round_trip(json!({"date": true, "dateFormat": "datetime11", "all": true}));
    assert_eq!(q.header_footer.date_format, "datetime11");

    // The default: no format asked for, so nothing to remember beyond the field itself.
    let q = round_trip(json!({"date": true, "all": true}));
    assert_eq!(q.header_footer.date_format, "datetime1", "the placeholder's own field is datetime1");

    // A fixed date is text, not a field: its text comes back and no format is claimed.
    let q = round_trip(json!({"date": true, "dateText": "4 July 1776", "all": true}));
    assert_eq!(q.header_footer.date_text, "4 July 1776");
    assert_eq!(q.header_footer.date_format, "", "a fixed date has no format");
}

#[test]
fn a_deck_without_a_date_placeholder_claims_no_format() {
    let p = defaults::new_presentation(None);
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let q = deckcraft_pptx::import(&bytes).expect("import");
    assert!(!q.header_footer.date);
    assert_eq!(q.header_footer.date_format, "");
}
