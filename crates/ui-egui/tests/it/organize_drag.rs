//! A document edit cancels an Organize drag before old page indices can be used.

use egui::{Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{AddedText, Edit, Session};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::PdfCraftApp;

fn document(prefix: &str) -> std::sync::Arc<Vec<u8>> {
    // Use the engine's synthetic document builder rather than another PDF serializer.
    let mut session = Session::new();
    let bytes = session.create_blank(200.0, 300.0, 4).unwrap();
    let id = session.open("pages.pdf", None, bytes, None).unwrap();
    for page in 0..4 {
        session
            .apply(
                id,
                Edit::AddText {
                    page,
                    text: AddedText { text: format!("{prefix} {}", page + 1), rect: [20.0, 120.0, 180.0, 150.0], ..Default::default() },
                },
            )
            .unwrap();
    }
    session.save_bytes(id).unwrap()
}

fn organize() -> Harness<'static, PdfCraftApp> {
    let bytes = document("Page");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, bytes.as_ref().clone()).unwrap();
        app.set_option("organize", "on").unwrap();
        app
    });
    h.run_steps(4);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3", "Page 4"]);
    h
}

fn page_texts(app: &PdfCraftApp) -> Vec<String> {
    let doc = app.session.get(app.views[0].id).unwrap();
    let mut renderer = PageRenderer::new(doc.bytes.clone(), Default::default());
    (0..renderer.page_count())
        .map(|page| {
            renderer
                .render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() })
                .text
                .unwrap()
                .plain_text()
                .trim()
                .to_string()
        })
        .collect()
}

fn copy_modifier(h: &Harness<'static, PdfCraftApp>, copying: bool) -> Modifiers {
    if !copying {
        Modifiers::NONE
    } else if h.ctx.os() == egui::os::OperatingSystem::Mac {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    }
}

fn start_drag(h: &mut Harness<'static, PdfCraftApp>, copying: bool) -> (Pos2, Modifiers) {
    let from = h.get_by_label("Page 1").rect().center();
    let to = h.get_by_label("Page 3").rect().right_center() - egui::vec2(10.0, 0.0);
    let modifiers = copy_modifier(h, copying);
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(from);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers });
    h.run_steps(1);
    for step in 1..=5 {
        h.hover_at(from + (to - from) * (step as f32 / 5.0));
        h.run_steps(1);
    }
    (to, modifiers)
}

fn release(h: &mut Harness<'static, PdfCraftApp>, to: Pos2, modifiers: Modifiers) {
    h.event(egui::Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(4);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

fn assert_saved_order(h: &Harness<'static, PdfCraftApp>, expected: &[&str]) {
    let app = h.state();
    let bytes = app.session.save_bytes(app.views[0].id).unwrap();
    let mut reopened = PdfCraftApp::new();
    reopened.open_bytes("saved.pdf", None, bytes.as_ref().clone()).unwrap();
    assert_eq!(page_texts(&reopened), expected, "save/reopen preserves the post-edit order");
}

fn check_undo_cancels_drag(copying: bool) {
    let mut h = organize();
    assert!(h.state_mut().apply_edit(Edit::MovePages { pages: vec![0], to: 3 }));
    h.run_steps(2);
    assert_eq!(page_texts(h.state()), ["Page 2", "Page 3", "Page 4", "Page 1"]);
    let (to, modifiers) = start_drag(&mut h, copying);
    // The source index now belongs to Page 2. Undo changes it back to Page 1.
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3", "Page 4"]);
    h.hover_at(to - egui::vec2(1.0, 0.0));
    h.run_steps(2);
    release(&mut h, to, modifiers);
    let expected = ["Page 1", "Page 2", "Page 3", "Page 4"];
    assert_eq!(page_texts(h.state()), expected, "releasing the old drag must not move or copy a different page");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_redo(), Some("Move page"), "the cancelled gesture leaves Redo available");
    assert_saved_order(&h, &expected);
    h.state_mut().redo();
    h.run_steps(2);
    assert_eq!(page_texts(h.state()), ["Page 2", "Page 3", "Page 4", "Page 1"], "the preserved Redo still applies the original edit");
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(page_texts(h.state()), expected);

    // Cancellation must not disable the next ordinary drag or its history.
    let (to, modifiers) = start_drag(&mut h, copying);
    release(&mut h, to, modifiers);
    let changed = if copying { vec!["Page 1", "Page 2", "Page 3", "Page 1", "Page 4"] } else { vec!["Page 2", "Page 3", "Page 1", "Page 4"] };
    assert_eq!(page_texts(h.state()), changed);
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(page_texts(h.state()), expected);
    h.state_mut().redo();
    h.run_steps(2);
    assert_eq!(page_texts(h.state()), changed);
    assert_saved_order(&h, &changed);
}

#[test]
fn undo_cancels_an_organize_move_before_pointer_release() {
    check_undo_cancels_drag(false);
}

#[test]
fn undo_cancels_an_organize_copy_before_pointer_release() {
    check_undo_cancels_drag(true);
}

#[test]
fn redo_cancels_an_organize_drag_before_pointer_release() {
    let mut h = organize();
    assert!(h.state_mut().apply_edit(Edit::MovePages { pages: vec![0], to: 3 }));
    h.state_mut().undo();
    h.run_steps(2);
    let (to, modifiers) = start_drag(&mut h, false);
    h.state_mut().redo();
    h.run_steps(2);
    release(&mut h, to, modifiers);
    let expected = ["Page 2", "Page 3", "Page 4", "Page 1"];
    assert_eq!(page_texts(h.state()), expected, "Redo cancels the old drag even though its source index remains valid");
    assert_saved_order(&h, &expected);
}

#[test]
fn deleting_a_page_cancels_an_organize_drag_before_pointer_release() {
    let mut h = organize();
    let (to, modifiers) = start_drag(&mut h, false);
    assert!(h.state_mut().apply_edit(Edit::DeletePages { pages: vec![0] }));
    h.run_steps(2);
    release(&mut h, to, modifiers);
    let expected = ["Page 2", "Page 3", "Page 4"];
    assert_eq!(page_texts(h.state()), expected, "a page edit cancels the old drag before indices shift");
    assert_saved_order(&h, &expected);
}

/// Replacement already starts a fresh view; valid old indices must not survive its reset.
#[test]
fn replacing_a_document_cancels_an_organize_drag_before_pointer_release() {
    let mut h = organize();
    let (to, modifiers) = start_drag(&mut h, false);
    let bytes = document("Replacement");
    let app = h.state_mut();
    let id = app.views[0].id;
    app.session.reload(id, bytes).unwrap();
    let info = &app.session.get(id).unwrap().info;
    app.views[0].document_replaced(info);
    let expected = ["Replacement 1", "Replacement 2", "Replacement 3", "Replacement 4"];
    assert_eq!(page_texts(h.state()), expected, "the new content replaces all four pages before release");
    h.run_steps(2);
    release(&mut h, to, modifiers);
    assert_eq!(page_texts(h.state()), expected, "releasing the old drag must not reorder replacement pages");
    assert_saved_order(&h, &expected);
}
