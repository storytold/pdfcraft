//! Split view: each pane's find field has its own id, so two find bars open at once do not clash
//! and do not share the focus; Find and Close act on the pane in use; and AltGr, which Windows
//! reports as Ctrl+Alt, does not run ⌘\ while it types a backslash.

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

/// With the same document in both panes, Find and Close act on the second pane, the one in use.
#[test]
fn find_and_close_act_on_the_second_pane() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(4);
    assert_eq!(h.state().active, Some(1), "the new pane is the one in use");

    h.key_press_modifiers(Modifiers::COMMAND, Key::F);
    h.run_steps(4);
    assert!(h.state().views[1].find.is_some(), "Find opens in the pane in use");
    assert!(h.state().views[0].find.is_none(), "and not in the other");

    let (kept, id) = (h.state().views[0].view_key(), h.state().views[0].id);
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1, "Close closes that pane's tab");
    assert_eq!(h.state().views[0].view_key(), kept, "and not the first pane's");
    assert!(h.state().session.get(id).is_some(), "the document stays open");
}

/// German and Spanish keyboards type `\` with AltGr, which reaches us as Ctrl+Alt: that is typing,
/// not ⌘\. The real Ctrl+\ still splits.
#[test]
fn altgr_backslash_does_not_split() {
    let mut h = harness();
    let altgr = Modifiers { alt: true, ctrl: true, command: true, ..Modifiers::NONE };
    h.key_press_modifiers(altgr, Key::Backslash);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1, "AltGr+key types a backslash");
    assert!(h.state().panes.split.is_none());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(4);
    assert!(h.state().panes.split.is_some(), "Ctrl+backslash splits");
}
