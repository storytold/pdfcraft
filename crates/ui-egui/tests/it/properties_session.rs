//! Properties drafts belong to the document revision on which the user opened them.

use crate::support::AppHarness as _;
use craft::ui::dialog::dialog_id;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Edit;
use pdfcraft_ui_egui::PdfCraftApp;
use serde_json::{Value, json};

/// Set one row of the open Properties dialog, as automation does.
fn set_field(h: &mut Harness<'static, PdfCraftApp>, field: &str, value: Value) {
    let ctx = h.ctx.clone();
    h.state_mut().dialogs.set_json_field(&ctx, dialog_id("file.properties"), field, value).unwrap();
}

/// Type into the labelled field of the shown tab.
fn type_into(h: &mut Harness<'static, PdfCraftApp>, label: &str, text: &str) {
    let field = h.get_by_role_and_label(Role::TextInput, label);
    field.focus();
    field.type_text(text);
    h.run_steps(2);
}

/// Open the real Properties dialog and type a title that has not entered document history.
fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
            app.open_bytes(&egui::Context::default(), "properties.pdf", None, bytes.as_ref().clone()).unwrap();
            app.set_option(&egui::Context::default(), "dialog", "properties").unwrap();
            app
        }
    });
    h.run_steps(4);
    h.get_by_role_and_label(Role::TextInput, "Title").focus();
    h.get_by_role_and_label(Role::TextInput, "Title").type_text("Old draft");
    h.run_steps(2);
    h
}

/// Reproduce the baseline stale commit; a guarded dialog has already dismissed itself.
fn confirm_if_still_open(h: &mut Harness<'static, PdfCraftApp>) {
    // On the baseline this commits the stale text. A guarded dialog has already closed.
    if !h.state().dialogs.is_empty() {
        h.get_by_label("OK").click();
        h.run_steps(3);
    }
}

#[test]
/// A pending title must never replace metadata written by a later document edit.
fn properties_session_does_not_overwrite_a_newer_document_edit() {
    let mut h = harness();
    let id = h.state().views[0].id;
    h.state_mut().session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Newer title".into() }).unwrap();
    let generation = h.state().session.get(id).unwrap().edit_generation();
    h.run_steps(2);
    confirm_if_still_open(&mut h);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.info_value("Title").as_deref(), Some("Newer title"));
    assert_eq!(doc.edit_generation(), generation, "stale confirmation issues no edit");
    assert!(h.state().dialogs.is_empty());
}

#[test]
/// Returning to the original value via Undo must still invalidate the draft and preserve Redo.
fn properties_session_rejects_undo_back_to_the_original_value() {
    let mut h = harness();
    let id = h.state().views[0].id;
    h.state_mut().session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Temporary".into() }).unwrap();
    h.state_mut().session.undo(id).unwrap();
    let generation = h.state().session.get(id).unwrap().edit_generation();
    h.run_steps(2);
    confirm_if_still_open(&mut h);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.info_value("Title"), None);
    assert_eq!(doc.edit_generation(), generation);
    assert!(doc.can_redo().is_some(), "stale confirmation preserves Redo");
    assert!(h.state().dialogs.is_empty());
}

#[test]
/// Inspection refresh must not rebase accepted edits or Custom input awaiting Add.
fn properties_session_inspection_tabs_do_not_rebase_pending_input() {
    for unaccepted_custom in [false, true] {
        let mut h = harness();
        let id = h.state().views[0].id;
        if unaccepted_custom {
            // Name/Value are typed input, even before Add has accepted a property.
            set_field(&mut h, "title", json!(""));
            h.set_option("dialog", "custom").unwrap();
            h.run_steps(3);
            type_into(&mut h, "Name:", "Project");
            type_into(&mut h, "Value:", "Unaccepted");
        }
        h.set_option("dialog", "space-audit").unwrap();
        h.run_steps(3);
        h.state_mut().session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Newer title".into() }).unwrap();
        let generation = h.state().session.get(id).unwrap().edit_generation();
        h.run_steps(3);
        assert!(h.state().dialogs.is_empty());
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.info_value("Title").as_deref(), Some("Newer title"));
        assert_eq!(doc.edit_generation(), generation);
        assert!(doc.custom_info().is_empty());
    }
}

#[test]
/// A clean inspection tab keeps the dialog open and follows a same-document edit.
fn properties_session_clean_inspection_tab_follows_a_newer_edit() {
    let mut h = harness();
    let id = h.state().views[0].id;
    // No pending input: the typed title is cleared again before switching tabs.
    set_field(&mut h, "title", json!(""));
    h.set_option("dialog", "space-audit").unwrap();
    h.run_steps(3);
    h.state_mut().session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Newer title".into() }).unwrap();
    let generation = h.state().session.get(id).unwrap().edit_generation();
    h.run_steps(3);
    assert!(!h.state().dialogs.is_empty(), "a clean inspection tab stays open");
    h.set_option("dialog", "properties").unwrap();
    h.run_steps(3);
    let title = h.get_by_role_and_label(Role::TextInput, "Title").value();
    assert_eq!(title.as_deref(), Some("Newer title"), "the draft is reseeded from the newer revision");
    assert_eq!(h.state().session.get(id).unwrap().edit_generation(), generation, "refreshing issues no edit");
}
