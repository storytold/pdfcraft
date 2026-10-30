//! Headless integration tests for adding text with custom and Unicode fonts (#4978).

use std::sync::Arc;

use egui_kittest::Harness;
use pdfcraft_engine::{AddedContent, AddedText, Edit, Session};
use pdfcraft_ui_egui::PdfCraftApp;

use crate::gpu;

/// Builds a test harness loaded with the standard single-page form fixture.
fn harness() -> Harness<'static, PdfCraftApp> {
    let bytes = include_bytes!("../data/form.pdf").to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, bytes.clone()).unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

/// Verifies that text added with an explicit embedded/custom font resource can draw Greek and Unicode text (#4978).
#[test]
fn add_text_with_custom_unicode_font_succeeds_and_roundtrips() {
    let _gpu = gpu();
    let mut h = harness();
    let id = h.state().views[0].id;

    // Register Inter-Regular font on page 0.
    let font_bytes = pdfcraft_fonts::INTER_REGULAR.to_vec();
    assert!(h.state_mut().session.apply(id, Edit::RegisterTextFont { page: 0, bytes: Arc::new(font_bytes) }).is_ok());

    let doc = h.state().session.get(id).unwrap();
    let font_entry =
        doc.text_editor_fonts(0).into_iter().find(|f| !f.resource.starts_with("PCApproved")).expect("embedded document font should be listed");
    let resource = font_entry.resource.clone();

    // Greek text containing phi (U+03C6) reported in issue #4978 ("γραφή").
    let greek_text = "γραφή (Greek text with φ)";

    // Standard text without font_resource would be flagged as undrawable by first_undrawable.
    let standard_attempt = AddedText { rect: [40.0, 195.0, 240.0, 220.0], text: greek_text.to_string(), ..Default::default() };
    assert!(pdfcraft_engine::first_undrawable(&standard_attempt).is_some(), "standard font must refuse Greek phi");

    // With custom font_resource, first_undrawable returns None.
    let custom_added =
        AddedText { rect: [40.0, 195.0, 240.0, 220.0], text: greek_text.to_string(), font_resource: Some(resource.clone()), ..Default::default() };
    assert_eq!(pdfcraft_engine::first_undrawable(&custom_added), None, "custom font resource permits Greek text");

    // Apply AddText edit to the page.
    assert!(h.state_mut().apply_edit(Edit::AddText { page: 0, text: custom_added }));
    h.run_steps(3);

    // Save and reload to verify round-trip persistence of font_resource and Greek text.
    let state = h.state();
    let bytes = state.session.save_bytes(state.views[0].id).unwrap();
    let mut reopened = Session::new();
    let open_id = reopened.open("again.pdf", None, bytes, None).unwrap();
    let reopened_doc = reopened.get(open_id).unwrap();
    assert_eq!(reopened_doc.added().len(), 1);

    let AddedContent::Text(saved_text) = &reopened_doc.added()[0].content else {
        panic!("expected added text item");
    };
    assert_eq!(saved_text.text, greek_text);
    assert_eq!(saved_text.font_resource.as_deref(), Some(resource.as_str()));
}

/// Converts normalized page coordinates into screen position for page 0.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> egui::Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

/// Simulates a pointer click at screen position `p`.
fn click(h: &mut Harness<'static, PdfCraftApp>, p: egui::Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// Verifies that typing Greek text into an active Add-text draft with custom font resource is not blocked (#4978).
#[test]
fn add_text_ui_draft_with_custom_font_is_not_blocked() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::QuickTool;

    let _gpu = gpu();
    let mut h = harness();
    let id = h.state().views[0].id;

    // Register Inter-Regular font on page 0.
    let font_bytes = pdfcraft_fonts::INTER_REGULAR.to_vec();
    assert!(h.state_mut().session.apply(id, Edit::RegisterTextFont { page: 0, bytes: Arc::new(font_bytes) }).is_ok());

    let doc = h.state().session.get(id).unwrap();
    let font_entry =
        doc.text_editor_fonts(0).into_iter().find(|f| !f.resource.starts_with("PCApproved")).expect("embedded document font should be listed");
    let resource = font_entry.resource.clone();

    // Set text_style font resource.
    h.state_mut().text_style.font_resource = Some(resource.clone());
    h.state_mut().quick_tool = QuickTool::AddText;
    h.run_steps(2);

    let point = at(&h, 45.0, 195.0);
    click(&mut h, point);

    let draft_style = h.state().views[0].content.draft.as_ref().expect("draft must open").style.clone();
    assert_eq!(draft_style.font_resource.as_deref(), Some(resource.as_str()));

    // Type Greek text with phi.
    h.event(egui::Event::Text("γραφή".into()));
    h.run_steps(3);

    // Blocked check must be None because custom font resource is present.
    let updated_style = h.state().views[0].content.draft.as_ref().expect("draft stays open").style.clone();
    assert_eq!(pdfcraft_engine::first_undrawable(&updated_style), None, "Greek text must not be blocked when custom font is set");

    h.get_by_label("Done adding text").click();
    h.run_steps(3);

    let added = h.state().session.get(id).unwrap().added().to_vec();
    assert_eq!(added.len(), 1);
    let AddedContent::Text(saved) = &added[0].content else {
        panic!("expected added text item");
    };
    assert_eq!(saved.text, "γραφή");
    assert_eq!(saved.font_resource.as_deref(), Some(resource.as_str()));
}
