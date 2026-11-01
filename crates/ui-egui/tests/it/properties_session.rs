//! Properties drafts belong to the document revision on which the user opened them.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Edit;
use pdfcraft_ui_egui::PdfCraftApp;

/// Open the real Properties dialog and type a title that has not entered document history.
fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("properties.pdf", None, bytes.as_ref().clone()).unwrap();
        app.set_option("dialog", "properties").unwrap();
        app
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
    if h.state().dialog.is_some() {
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
    assert!(h.state().dialog.is_none());
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
    assert!(h.state().dialog.is_none());
}

#[test]
/// Inspection refresh must not rebase accepted edits or Custom input awaiting Add.
fn properties_session_inspection_tabs_do_not_rebase_pending_input() {
    for unaccepted_custom in [false, true] {
        let mut h = harness();
        let id = h.state().views[0].id;
        if unaccepted_custom {
            // Name/Value are typed input, even before Add has accepted a property.
            let app = h.state_mut();
            app.props_draft.as_mut().unwrap().1[0].clear();
            let custom = &mut app.custom_draft.as_mut().unwrap().1;
            custom.name = "Project".into();
            custom.value = "Unaccepted".into();
        }
        h.state_mut().set_option("dialog", "space-audit").unwrap();
        h.run_steps(3);
        h.state_mut().session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Newer title".into() }).unwrap();
        let generation = h.state().session.get(id).unwrap().edit_generation();
        h.run_steps(3);
        assert!(h.state().dialog.is_none());
        assert!(h.state().props_draft.is_none());
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.info_value("Title").as_deref(), Some("Newer title"));
        assert_eq!(doc.edit_generation(), generation);
        assert!(doc.custom_info().is_empty());
    }
}
