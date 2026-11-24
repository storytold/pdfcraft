//! Headless integration tests for adding text with custom and Unicode fonts (#4978).

use std::sync::Arc;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{AddedContent, AddedText, Edit, Session};
use pdfcraft_ui_egui::{FilePurpose, PdfCraftApp, QuickTool};

use crate::gpu;
use crate::support::AppHarness as _;

/// Builds a test harness loaded with the standard single-page form fixture.
fn harness() -> Harness<'static, PdfCraftApp> {
    let bytes = include_bytes!("../data/form.pdf").to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(0.25).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::control::new_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.open_bytes(&cc.egui_ctx, "form.pdf", None, bytes.clone()).unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

/// Registers the bundled Inter-Regular face on page 0 and returns its `(resource, face)` pair.
fn register_inter(h: &mut Harness<'static, PdfCraftApp>) -> (String, String) {
    let id = h.state().views[0].id;
    let font_bytes = pdfcraft_fonts::INTER_REGULAR.to_vec();
    assert!(h.state_mut().session.apply(id, Edit::RegisterTextFont { page: 0, bytes: Arc::new(font_bytes) }).is_ok());
    let doc = h.state().session.get(id).unwrap();
    let font_entry = doc
        .text_editor_fonts(0)
        .into_iter()
        .find(|f| !f.resource.starts_with("PCApproved") && !f.choice_only)
        .expect("embedded document font should be listed");
    (font_entry.resource, font_entry.face)
}

/// Verifies that text added with an explicit embedded/custom font resource can draw Greek and Unicode text (#4978).
#[test]
fn add_text_with_custom_unicode_font_succeeds_and_roundtrips() {
    let _gpu = gpu();
    let mut h = harness();
    let (resource, _) = register_inter(&mut h);

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
    let ctx = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx, Edit::AddText { page: 0, text: custom_added }));
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

/// Verifies that loading a font file for Add text applies it and lets a draft commit Greek text without being blocked (#4978).
#[test]
fn add_text_ui_draft_with_custom_font_is_not_blocked() {
    let _gpu = gpu();
    let mut h = harness();
    let id = h.state().views[0].id;

    // Load Inter-Regular through the TextFont workflow while Add text is armed.
    assert!(h.execute("edit.text"));
    h.run_steps(2);
    let ctx = h.ctx.clone();
    h.state_mut().use_files(&ctx, FilePurpose::TextFont { page: 0 }, vec![("Inter-Regular.ttf".into(), pdfcraft_fonts::INTER_REGULAR.to_vec())]);
    h.run_steps(2);

    let resource = h.state().text_style.font_resource.clone().expect("loaded font applies to armed text style");
    assert_eq!(h.state().quick_tool, QuickTool::AddText);

    let point = at(&h, 45.0, 195.0);
    click(&mut h, point);

    let draft_style = h.state().views[0].session.content.draft.as_ref().expect("draft must open").style.clone();
    assert_eq!(draft_style.font_resource.as_deref(), Some(resource.as_str()));

    // Type Greek text with phi.
    h.event(egui::Event::Text("γραφή".into()));
    h.run_steps(3);

    // Blocked check must be None because custom font resource is present.
    let updated_style = h.state().views[0].session.content.draft.as_ref().expect("draft stays open").style.clone();
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

/// Verifies that the Format text font picker lists registered page fonts for added text and applies one as an undoable edit (#4978).
#[test]
fn format_text_font_picker_applies_document_font_to_added_text() {
    let _gpu = gpu();
    let mut h = harness();
    let id = h.state().views[0].id;
    let (resource, face) = register_inter(&mut h);

    // Add standard text, select it, and pick the registered document font in Format text.
    let ctx = h.ctx.clone();
    assert!(h.state_mut().apply_edit(
        &ctx,
        Edit::AddText { page: 0, text: AddedText { rect: [40.0, 195.0, 200.0, 220.0], text: "Label".into(), ..Default::default() } },
    ));
    assert!(h.execute("edit.edit_text"));
    h.state_mut().views[0].session.content.selected = Some((0, 0));
    h.run_steps(4);

    let font_node = h.get_by_role_and_label(Role::ComboBox, "Font family");
    font_node.click();
    h.run_steps(2);
    h.query_all_by_label(&face).next().expect("document font row should appear in Format text").click();
    h.run_steps(4);

    let added = h.state().session.get(id).unwrap().added().to_vec();
    let AddedContent::Text(updated) = &added[0].content else {
        panic!("expected added text item");
    };
    assert_eq!(updated.font_resource.as_deref(), Some(resource.as_str()));
    assert_eq!(h.state().text_style.font_resource.as_deref(), Some(resource.as_str()));
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Edit content"));
}

/// Verifies that adding multi-paragraph text with a custom font respects paragraph alignment and persists.
#[test]
fn add_text_with_custom_font_and_paragraph_alignment() {
    let _gpu = gpu();
    let mut h = harness();
    let (resource, _) = register_inter(&mut h);

    let text = "Paragraph 1 line\nParagraph 2 line";
    let custom_added = AddedText {
        rect: [40.0, 150.0, 240.0, 220.0],
        text: text.to_string(),
        font_resource: Some(resource.clone()),
        align: pdfcraft_engine::TextAlign::Left,
        aligns: vec![Some(pdfcraft_engine::TextAlign::Right)],
        ..Default::default()
    };

    let ctx = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx, Edit::AddText { page: 0, text: custom_added }));
    h.run_steps(3);

    let state = h.state();
    let bytes = state.session.save_bytes(state.views[0].id).unwrap();
    let mut reopened = Session::new();
    let open_id = reopened.open("reopened.pdf", None, bytes, None).unwrap();
    let reopened_doc = reopened.get(open_id).unwrap();
    assert_eq!(reopened_doc.added().len(), 1);

    let AddedContent::Text(saved_text) = &reopened_doc.added()[0].content else {
        panic!("expected added text item");
    };
    assert_eq!(saved_text.text, text);
    assert_eq!(saved_text.font_resource.as_deref(), Some(resource.as_str()));
    assert_eq!(saved_text.align, pdfcraft_engine::TextAlign::Left);
    assert_eq!(saved_text.aligns, vec![Some(pdfcraft_engine::TextAlign::Right)]);
}
