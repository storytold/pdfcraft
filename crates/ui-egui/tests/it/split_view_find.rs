//! Split view: each pane's find field has its own id, so two find bars open at once do not clash
//! and do not share the focus.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn each_panes_find_input_has_its_own_id() {
    let mut h = harness();
    // ⌘\ splits the document: a second view of it in the second pane.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 2);
    let (first, second) = (h.state().views[0].find_input_id(), h.state().views[1].find_input_id());
    assert_ne!(first, second, "one find field id per pane");

    // Find open in both panes, the second one asking for the focus last.
    h.state_mut().views[0].open_find();
    h.state_mut().views[1].open_find();
    h.run_steps(4);
    let focused = h.ctx.memory(|m| m.focused());
    assert_eq!(focused, Some(second), "the focus is in the pane that asked last");
    assert!(!h.ctx.memory(|m| m.has_focus(first)), "the other pane's field does not share it");

    // Typing goes to one query only.
    h.event(egui::Event::Text("abc".into()));
    h.run_steps(2);
    assert_eq!(h.state().views[1].find.as_ref().unwrap().query, "abc");
    assert!(h.state().views[0].find.as_ref().unwrap().query.is_empty());
}
