//! Crop tool and Set Page Boxes in the real shell (egui_kittest).

use crate::support::AppHarness as _;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, FIXTURE.to_vec()).unwrap();
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "zoom", "150").unwrap();
            app
        }
    });
    h.run_steps(6);
    h
}

fn size(h: &Harness<'static, PdfCraftApp>, page: usize) -> (f32, f32) {
    let s = h.state();
    let p = &s.session.get(s.views[0].id).unwrap().info.pages[page];
    (p.width, p.height)
}

#[test]
fn the_crop_tool_crops_to_the_dragged_rectangle_then_returns_to_select() {
    let mut h = harness();
    assert!(h.execute("page.crop"));
    assert_eq!(h.state().quick_tool, QuickTool::Crop);
    h.run_steps(2);
    crop_middle_half(&mut h);
}

#[test]
fn quick_toolbar_crop_is_hidden_on_home_and_in_read_mode() {
    let mut h = harness();
    h.execute("view.read_mode");
    h.run_steps(3);
    assert!(h.query_by_label("Crop pages").is_none());
    h.execute("view.read_mode");
    h.run_steps(3);
    h.get_by_label("Crop pages");
    h.state_mut().active = None;
    h.run_steps(3);
    assert!(h.query_by_label("Crop pages").is_none());
}

fn crop_middle_half(h: &mut Harness<'static, PdfCraftApp>) {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    // The middle half of the page in both directions.
    let (a, b) = (r.min + r.size() * 0.25, r.min + r.size() * 0.75);
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    let (w, hh) = size(h, 0);
    assert!((w - 150.0).abs() < 2.0 && (hh - 200.0).abs() < 2.0, "{w}×{hh}");
    assert_eq!(size(h, 1), (300.0, 400.0));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Crop page"));
}

#[test]
fn set_page_boxes_dialog_applies_margins_to_the_chosen_pages() {
    let mut h = harness();
    assert!(h.execute("page.boxes"));
    h.run_steps(3);
    h.get_by_role_and_label(Role::Dialog, "Set Page Boxes");
    // One inch off every side (the units default to inches).
    for side in ["Top", "Bottom", "Left", "Right"] {
        h.get_by_label(side).click();
        h.run_steps(1);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_label(side).type_text("1");
        h.key_press(egui::Key::Tab);
        h.run_steps(2);
    }
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert!(h.query_by_label("Set Page Boxes").is_none(), "OK closes the dialog");
    assert_eq!(size(&h, 0), (156.0, 256.0));
    assert_eq!(size(&h, 1), (156.0, 256.0), "All pages by default");
    // Set to zero resets the box.
    h.execute("page.boxes");
    h.run_steps(3);
    h.get_by_label("Set to zero").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(size(&h, 0), (300.0, 400.0));
}

#[test]
fn the_crop_tool_leaves_the_page_grid_so_a_page_can_be_cropped() {
    let mut h = harness();
    // The Organize panel's thumbnail grid is on screen for this view: choosing
    // "Crop pages" from it must leave the grid so the canvas handles a drag on
    // a page instead of returning early to organize_grid.
    h.state_mut().views[0].organize = true;
    assert!(h.execute("page.crop"));
    assert_eq!(h.state().quick_tool, QuickTool::Crop);
    assert!(!h.state().views[0].organize, "\"page.crop\" must clear view.organize");
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    // The middle half of the page in both directions.
    let (a, b) = (r.min + r.size() * 0.25, r.min + r.size() * 0.75);
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    let (w, hh) = size(&h, 0);
    assert!((w - 150.0).abs() < 2.0 && (hh - 200.0).abs() < 2.0, "{w}×{hh}");
    assert_eq!(size(&h, 1), (300.0, 400.0));
}
