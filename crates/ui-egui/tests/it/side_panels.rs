//! Side panels: the Tools pane, the rail and the open side panel.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

const WIDTH: f32 = 1400.0;
const NO_BOOKMARKS: &str = "This document has no bookmarks.";

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(WIDTH, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(6);
    h
}

fn rail(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_role_and_label(Role::Button, "Bookmarks").rect()
}

fn side(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_label(NO_BOOKMARKS).rect()
}

/// The Tools pane's All tools header button.
fn tools(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_label("Expand all tools").rect()
}

#[test]
fn rail_is_outermost_then_tools_then_the_side_panel_then_the_page() {
    // Tools on the left: the page between Tools and the side panel, the rail at the right edge.
    let h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    let (r, s, t) = (rail(&h), side(&h), tools(&h));
    assert!(r.right() > WIDTH - 12.0, "{r:?}");
    assert!(s.right() < r.left() && s.left() > WIDTH - 48.0 - 340.0, "{s:?} {r:?}");
    assert!(t.left() < 48.0, "{t:?}");
    // Tools on the right: rail, Tools, side panel, page.
    let h = harness(|app| {
        app.set_option("tools-side", "right").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
    });
    let (r, s, t) = (rail(&h), side(&h), tools(&h));
    assert!(r.right() > WIDTH - 12.0, "{r:?}");
    assert!(t.right() < r.left() && t.left() > WIDTH - 48.0 - 280.0, "{t:?} {r:?}");
    assert!(s.right() < t.left() && s.left() > WIDTH - 48.0 - 280.0 - 340.0, "{s:?} {t:?}");
}

#[test]
fn read_mode_shows_the_side_panel_without_tools() {
    let mut h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    h.state_mut().set_option("mode", "read").unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Expand all tools").is_none());
    assert!(side(&h).right() < rail(&h).left() && side(&h).left() > WIDTH / 2.0);
    // The rail closes and opens it there too.
    h.get_by_role_and_label(Role::Button, "Bookmarks").click();
    h.run_steps(4);
    assert!(h.query_by_label(NO_BOOKMARKS).is_none());
    h.get_by_role_and_label(Role::Button, "Bookmarks").click();
    h.run_steps(4);
    assert!(side(&h).right() < rail(&h).left());
}

#[test]
fn a_dragged_width_is_remembered_across_restarts() {
    let mut h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    let before = h.state().side_panels.side_width;
    let side_left = side(&h).left();
    // The side panel's resize edge: the one right of the page, left of the panel's text.
    let edge = h
        .get_all_by_label("Resize panels")
        .map(|n| n.rect().center())
        .filter(|c| c.x < side_left && c.x > WIDTH / 2.0)
        .min_by(|a, b| (side_left - a.x).total_cmp(&(side_left - b.x)))
        .expect("the side panel has a resize edge")
        // The pane's half of the edge: the page, drawn after the panes, takes the other half.
        + egui::vec2(2.0, 0.0);
    h.hover_at(edge);
    h.run_steps(1);
    h.drag_at(edge);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(edge - egui::vec2(10.0 * i as f32, 0.0));
        h.run_steps(1);
    }
    h.drop_at(edge - egui::vec2(40.0, 0.0));
    h.run_steps(3);
    let after = h.state().side_panels.side_width;
    // The pointer was pressed 2 pt inside the panel, and the edge follows the pointer.
    assert!((after - (before + 40.0 - 2.0)).abs() <= 1.0, "{before} -> {after}");
    let saved = h.state().persist();
    let mut restored = PdfCraftApp::new();
    restored.restore(&saved);
    assert_eq!(restored.side_panels.side_width, after);
}

#[test]
fn f4_toggles_the_side_panel_and_shift_f4_the_tools_panel() {
    let mut h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    h.get_by_label(NO_BOOKMARKS);
    h.key_press(egui::Key::F4);
    h.run_steps(3);
    assert_eq!(h.state().right, None);
    assert!(h.query_by_label(NO_BOOKMARKS).is_none());
    // F4 again reopens the panel that was open, not the default.
    h.key_press(egui::Key::F4);
    h.run_steps(3);
    h.get_by_label(NO_BOOKMARKS);
    assert!(h.state().left_open);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F4);
    h.run_steps(3);
    assert!(!h.state().left_open);
    assert!(h.query_by_label("Expand all tools").is_none());
    // Shift+F4 leaves the side panel alone.
    h.get_by_label(NO_BOOKMARKS);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F4);
    h.run_steps(3);
    h.get_by_label("Expand all tools");
}
