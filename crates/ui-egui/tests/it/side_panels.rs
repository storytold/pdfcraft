//! Side panels: the Tools panel, the rail and the open side panel.

use crate::support::AppHarness as _;
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

fn harness_sized(size: egui::Vec2, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(size).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, FIXTURE.to_vec()).unwrap();
            setup(&mut app);
            app
        }
    });
    h.run_steps(6);
    h
}

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    harness_sized(egui::vec2(WIDTH, 900.0), setup)
}

/// The rail's Bookmarks button.
fn rail(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_role_and_label(Role::Button, "Bookmarks").rect()
}

fn side(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_label(NO_BOOKMARKS).rect()
}

/// The Tools panel's All tools header button.
fn tools(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_label("Expand all tools").rect()
}

/// The x of the dock divider next to the side panel, on its page side.
fn divider_x(h: &Harness<'static, PdfCraftApp>) -> f32 {
    let side_left = side(h).left();
    h.get_all_by_label("Resize panels")
        .map(|n| n.rect().center().x)
        .filter(|x| *x < side_left && *x > WIDTH / 2.0)
        .max_by(f32::total_cmp)
        .expect("the side panel has a divider")
}

#[test]
fn read_mode_shows_the_side_panel_without_tools() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "panel", "bookmarks").unwrap());
    h.set_option("mode", "read").unwrap();
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
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "panel", "bookmarks").unwrap());
    let before = divider_x(&h);
    let edge = egui::pos2(before, side(&h).center().y);
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
    let dragged = divider_x(&h);
    assert!(dragged < before - 20.0, "dragging the divider left widens the panel: {before} -> {dragged}");
    let snapshot = h.state().settings_snapshot();
    let restored = harness(move |app| app.apply_settings(&snapshot));
    let after = divider_x(&restored);
    assert!((after - dragged).abs() <= 1.0, "{dragged} -> {after}");
}

#[test]
fn f4_toggles_the_side_panel_and_shift_f4_the_tools_panel() {
    let mut h = harness(|app| app.set_option(&egui::Context::default(), "panel", "bookmarks").unwrap());
    h.get_by_label(NO_BOOKMARKS);
    h.key_press(egui::Key::F4);
    h.run_steps(3);
    assert!(h.query_by_label(NO_BOOKMARKS).is_none());
    // F4 again reopens the panel that was open, not the default.
    h.key_press(egui::Key::F4);
    h.run_steps(3);
    h.get_by_label(NO_BOOKMARKS);
    assert!(tools(&h).left() < WIDTH / 2.0);
    // Shift+F4 closes Tools and leaves the side panel alone.
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F4);
    h.run_steps(3);
    assert!(h.query_by_label("Expand all tools").is_none());
    h.get_by_label(NO_BOOKMARKS);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F4);
    h.run_steps(3);
    h.get_by_label("Expand all tools");
}
