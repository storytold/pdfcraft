//! Edit a PDF ▸ Link: drawing a link, Link Properties, editing and deleting links.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::LinkAction;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness() -> Harness<'static, PdfCraftApp> {
    // 60 fps steps, so two clicks fall inside egui's double-click window.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, a: Pos2, b: Pos2) {
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(3);
}

fn links(h: &Harness<'static, PdfCraftApp>) -> Vec<pdfcraft_engine::LinkItem> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().links().to_vec()
}

#[test]
fn drawing_editing_and_deleting_links() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.link"));
    let (a, b) = (at(&h, 40.0, 300.0), at(&h, 160.0, 320.0));
    drag(&mut h, a, b);
    assert_eq!(h.state().dialog, Some(Dialog::LinkProps));
    h.get_by_label("Create Link");
    h.state_mut().link_draft.as_mut().unwrap().url = "https://example.org".into();
    h.get_by_label("OK").click();
    h.run_steps(3);
    let l = links(&h);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].action, LinkAction::Uri("https://example.org".into()));
    // Double-click opens Link Properties; switch to a page action.
    let c = at(&h, 100.0, 310.0);
    h.hover_at(c);
    h.run_steps(1);
    for _ in 0..2 {
        h.drag_at(c);
        h.step();
        h.drop_at(c);
        h.step();
    }
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::LinkProps));
    h.get_by_label("Link Properties");
    {
        let d = h.state_mut().link_draft.as_mut().unwrap();
        d.web = false;
        d.target = 1;
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(links(&h)[0].action, LinkAction::Page(0));
    // Delete the selected link.
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(links(&h).is_empty());
}

#[test]
fn removing_all_links_and_undo() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.link"));
    let (a, b) = (at(&h, 40.0, 300.0), at(&h, 160.0, 320.0));
    drag(&mut h, a, b);
    h.state_mut().link_draft.as_mut().unwrap().url = "https://example.org".into();
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(links(&h).len(), 1);

    // Execute remove all links command
    assert!(h.state_mut().execute("edit.remove_links"));
    h.run_steps(3);
    assert_eq!(links(&h).len(), 0);

    // Undo restores the link
    h.state_mut().undo();
    h.run_steps(3);
    assert_eq!(links(&h).len(), 1);
}

fn right_click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

#[test]
fn the_link_context_menu_shows_only_for_a_right_click_on_a_link() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.link"));
    let (a, b) = (at(&h, 40.0, 100.0), at(&h, 160.0, 120.0));
    drag(&mut h, a, b);
    h.state_mut().link_draft.as_mut().unwrap().url = "https://example.org".into();
    h.get_by_label("OK").click();
    h.run_steps(3);
    let (on_link, away) = (at(&h, 100.0, 110.0), at(&h, 250.0, 200.0));
    // Select the link with the Link tool, then right-click away from it: the menu there is
    // not the link's.
    h.hover_at(on_link);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: on_link, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: on_link, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    assert!(h.state().views[0].links.selected.is_some());
    right_click(&mut h, away);
    assert!(h.query_by_label("Remove all links").is_none(), "a right-click away from the link got the link menu");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    // A right-click on the link gets it, with the Select tool too.
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Select;
    h.run_steps(2);
    right_click(&mut h, on_link);
    h.get_by_label("Remove all links").click();
    h.run_steps(3);
    assert!(links(&h).is_empty());
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Remove all links"));
}
