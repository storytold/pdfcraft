//! Split view: typing in a field of one pane goes into the document when the next click goes to
//! the other pane, even when that click changes the document too.

use egui::{Key, Modifiers, Pos2};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::panes::Place;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        // The whole 300×400 pt page in either pane.
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(4);
    h
}

/// The centre of field `name`'s first widget in tab `view`, once that tab has drawn its page.
fn field_center(h: &Harness<'static, PdfCraftApp>, view: usize, name: &str) -> Option<Pos2> {
    let s = h.state();
    let doc = s.session.get(s.views[view].id)?;
    let f = doc.form.iter().find(|f| f.name == name)?;
    pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[view], &doc.info, f, 0).map(|r| r.center())
}

fn value(h: &Harness<'static, PdfCraftApp>, name: &str) -> Vec<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

/// A press and release at `p`.
fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// Typing in the second pane's field, then clicking a check box in the first pane: the click
/// makes the first pane active before the second is drawn, and the field commits as it loses the
/// focus, so its edit is queued on the tab that is no longer active. It used to wait there until
/// the check box's edit made that tab start afresh next frame, and the typing was gone.
#[test]
fn typing_in_one_pane_survives_an_edit_in_the_other() {
    let mut h = harness();
    // ⌘\ splits the document: a second view of it in the second pane, the active one.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(4);
    let (first, second) = (0, 1);
    assert_eq!(h.state().views.iter().map(|v| v.place()).collect::<Vec<_>>(), [Place::Main, Place::Second]);
    assert_eq!(h.state().views[first].id, h.state().views[second].id, "one document");
    for _ in 0..100 {
        if field_center(&h, first, "agree").is_some() && field_center(&h, second, "name").is_some() {
            break;
        }
        h.run_steps(1);
    }
    let name = field_center(&h, second, "name").expect("the second pane shows the field");
    let agree = field_center(&h, first, "agree").expect("the first pane shows the check box");
    assert!(h.state().views[second].viewport_rect().contains(name), "{name:?}");
    assert!(h.state().views[first].viewport_rect().contains(agree), "{agree:?}");

    click(&mut h, name);
    assert_eq!(h.state().active, Some(second));
    assert!(h.state().views[second].forms.focus.is_some(), "the editor opened");
    h.event(egui::Event::Text("Typed in the second pane".into()));
    h.run_steps(2);
    assert!(value(&h, "name").is_empty(), "not committed yet");

    click(&mut h, agree);
    h.run_steps(3);
    assert_eq!(h.state().active, Some(first), "the click made the first pane active");
    assert_eq!(value(&h, "agree"), ["Yes"], "the click's own edit");
    assert_eq!(value(&h, "name"), ["Typed in the second pane"], "the typing in the other pane went into the document");
    assert!(h.state().views[second].pending_edit.is_none(), "nothing left queued");
}
