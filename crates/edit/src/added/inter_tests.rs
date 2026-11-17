//! Greek, Cyrillic and extended Latin in added text: drawn from the bundled Inter face (present in
//! every build) instead of being refused as characters the standard fonts can't encode.

use super::*;

fn item(text: &str) -> AddedText {
    AddedText { text: text.into(), rect: [72.0, 600.0, 540.0, 700.0], ..AddedText::default() }
}

#[test]
fn greek_and_cyrillic_added_text_is_drawable() {
    for text in ["Γειά σου κόσμε", "Ελλάδα 2026: γ, δ, ς", "Привет", "Dvořák", "ΆΈΉ ἀγαθός"] {
        assert_eq!(first_undrawable(&item(text)), None, "{text}");
    }
    // What Inter lacks is still refused rather than written as `?`.
    assert_eq!(first_undrawable(&item("γ 日本")), Some('日'));
}

#[test]
fn greek_added_text_draws_inter_glyphs_and_keeps_its_text() {
    let mut doc = Document::new_empty();
    let t = item("Καλημέρα Semra");
    let (content, res) = draw::draw(&mut doc, &Content::Text(t), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &Dict::new(), false).unwrap();
    let content = String::from_utf8_lossy(&content);
    // The line keeps its typed text for copy and search.
    let logical: String = "Καλημέρα Semra".encode_utf16().map(|u| format!("{u:04X}")).collect();
    assert!(content.contains(&logical), "{content}");
    let fonts = match res.get(b"Font") {
        Some(Object::Dict(f)) => f.clone(),
        other => panic!("no fonts: {other:?}"),
    };
    let type3 = fonts.iter().filter(|(_, f)| doc.dict(f).as_ref().is_some_and(|d| d.name(b"Subtype") == Some(b"Type3"))).count();
    assert!(type3 >= 1, "Greek is drawn with a generated Type 3 font");
    // The Latin word stays in the item's standard font.
    assert!(content.contains("(Semra) Tj") || content.contains("( Semra) Tj"), "{content}");
}
