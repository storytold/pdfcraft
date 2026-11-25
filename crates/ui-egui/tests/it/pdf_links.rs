//! Edit a PDF ▸ Link: drawing a link, Link Properties, editing and deleting links.

use crate::support::AppHarness as _;
use egui::Pos2;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::LinkAction;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    // 60 fps steps, so two clicks fall inside egui's double-click window.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
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

fn double_click(h: &mut Harness<'static, PdfCraftApp>, c: Pos2) {
    h.hover_at(c);
    h.run_steps(1);
    for _ in 0..2 {
        h.drag_at(c);
        h.step();
        h.drop_at(c);
        h.step();
    }
    h.run_steps(2);
}

fn links(h: &Harness<'static, PdfCraftApp>) -> Vec<pdfcraft_engine::LinkItem> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().links().to_vec()
}

/// Types the address over Link Properties' URL field, which starts with the scheme.
fn type_url(h: &mut Harness<'static, PdfCraftApp>) {
    h.get_by_role_and_label(Role::TextInput, "Link URL").click();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.get_by_role_and_label(Role::TextInput, "Link URL").type_text("https://example.org");
    h.run_steps(2);
}

/// Draws a link over `a`..`b` with the Link tool and confirms the dialog with a web address.
fn draw_web_link(h: &mut Harness<'static, PdfCraftApp>, a: Pos2, b: Pos2) {
    assert!(h.execute("edit.link"));
    drag(h, a, b);
    h.get_by_role_and_label(Role::Dialog, "Create Link");
    type_url(h);
    h.get_by_label("OK").click();
    h.run_steps(3);
}

#[test]
fn drawing_editing_and_deleting_links() {
    let mut h = harness();
    let (a, b) = (at(&h, 40.0, 300.0), at(&h, 160.0, 320.0));
    draw_web_link(&mut h, a, b);
    let l = links(&h);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].action, LinkAction::Uri("https://example.org".into()));
    // Double-click opens Link Properties; switch to a page action.
    let c = at(&h, 100.0, 310.0);
    double_click(&mut h, c);
    h.get_by_role_and_label(Role::Dialog, "Link Properties");
    h.get_by_label("Go to a page view").click();
    h.run_steps(3);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(links(&h)[0].action, LinkAction::Page { page: 0, zoom: pdfcraft_engine::LinkZoom::FitPage });
    // Delete the selected link.
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(links(&h).is_empty());
}

#[test]
fn link_properties_sets_the_page_view_zoom() {
    use pdfcraft_engine::LinkZoom;
    let mut h = harness();
    assert!(h.execute("edit.link"));
    let (a, b) = (at(&h, 40.0, 300.0), at(&h, 160.0, 320.0));
    drag(&mut h, a, b);
    h.get_by_role_and_label(Role::Dialog, "Create Link");
    h.get_by_label("Go to a page view").click();
    h.run_steps(3);
    // The field's caption and the toolbar's zoom carry the name too: the choice is the drop-down.
    h.get_by_role_and_label(Role::ComboBox, "Zoom").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::ListBoxOption, "200%").scroll_to_me();
    h.run_steps(3);
    h.get_by_role_and_label(Role::ListBoxOption, "200%").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(links(&h)[0].action, LinkAction::Page { page: 0, zoom: LinkZoom::Percent(200.0) });
}

/// One page with a link whose destination has a position, which Link Properties doesn't show.
const POSITIONED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Link /Rect [40 40 160 60] /Border [0 0 0] /Dest [3 0 R /XYZ 100 300 2] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn an_appearance_edit_keeps_the_destination_as_written() {
    use pdfcraft_render::{DestView, LinkTarget};
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "positioned.pdf", None, POSITIONED.to_vec()).unwrap();
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            app
        }
    });
    h.run_steps(6);
    let positioned = LinkTarget::Page(0, DestView::Xyz { left: Some(100.0), top: Some(300.0), zoom: Some(2.0) });
    let target = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().info.links[0].target.clone()
    };
    assert_eq!(target(&h), positioned);
    assert!(h.execute("edit.link"));
    h.run_steps(2);
    // The link lies at y 340..360 from the top of the 400 pt page.
    let c = at(&h, 100.0, 350.0);
    double_click(&mut h, c);
    h.get_by_role_and_label(Role::Dialog, "Link Properties");
    h.get_by_role_and_label(Role::ComboBox, "Link Type").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::ListBoxOption, "Visible Rectangle").click();
    h.run_steps(2);
    h.get_by_label("Other colour…").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::TextInput, "Hex color").click();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_role_and_label(Role::TextInput, "Hex color").type_text("FF0000");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(links(&h)[0].style.color, [1.0, 0.0, 0.0], "the edit applied");
    assert_eq!(target(&h), positioned, "the destination is not rewritten without its position");
}

#[test]
fn removing_all_links_and_undo() {
    let mut h = harness();
    let (a, b) = (at(&h, 40.0, 300.0), at(&h, 160.0, 320.0));
    draw_web_link(&mut h, a, b);
    assert_eq!(links(&h).len(), 1);

    // Execute remove all links command
    assert!(h.execute("edit.remove_links"));
    h.run_steps(3);
    assert_eq!(links(&h).len(), 0);

    // Undo restores the link
    h.undo();
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
    let (a, b) = (at(&h, 40.0, 100.0), at(&h, 160.0, 120.0));
    draw_web_link(&mut h, a, b);
    let (on_link, away) = (at(&h, 100.0, 110.0), at(&h, 250.0, 200.0));
    // Select the link with the Link tool, then right-click away from it: the menu there is
    // not the link's.
    h.hover_at(on_link);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: on_link, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: on_link, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    assert!(h.state().views[0].session.links.selected.is_some());
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
