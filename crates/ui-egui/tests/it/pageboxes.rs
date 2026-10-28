//! Crop tool, Set Page Boxes and Duplicate pages in the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, QuickTool};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
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
    assert!(h.state_mut().execute("page.crop"));
    assert_eq!(h.state().quick_tool, QuickTool::Crop);
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
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Crop page"));
}

#[test]
fn set_page_boxes_dialog_applies_margins_to_the_chosen_pages() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.boxes"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::PageBoxes));
    h.get_by_label("Set Page Boxes");
    // One inch off every side (the units default to inches; margins are stored in points).
    h.state_mut().boxes_draft.margins = [72.0; 4];
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(size(&h, 0), (156.0, 256.0));
    assert_eq!(size(&h, 1), (156.0, 256.0), "All pages by default");
    // Set to zero resets the box.
    h.state_mut().execute("page.boxes");
    h.run_steps(3);
    h.get_by_label("Set to zero").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(size(&h, 0), (300.0, 400.0));
}

#[test]
fn bulk_crop_dialog_applies_margins_across_pages() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.bulk_crop"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::BulkCrop));
    h.get_by_label("Bulk Crop Pages");
    h.state_mut().bulk_crop_draft.blended_margins = [20.0; 4];
    h.run_steps(2);
    h.get_by_label("Crop Pages").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(size(&h, 0), (260.0, 360.0));
    assert_eq!(size(&h, 1), (260.0, 360.0));
}

#[test]
fn bulk_crop_dialog_subsequent_crop_preserves_untouched_pages() {
    let mut h = harness();
    // 1. Initial crop of 20pt on all pages
    assert!(h.state_mut().execute("page.bulk_crop"));
    h.run_steps(3);
    h.state_mut().bulk_crop_draft.blended_margins = [20.0; 4];
    h.run_steps(2);
    h.get_by_label("Crop Pages").click();
    h.run_steps(4);
    assert_eq!(size(&h, 0), (260.0, 360.0));
    assert_eq!(size(&h, 1), (260.0, 360.0));

    // 2. Open bulk crop again: existing 20pt insets are loaded into handles
    assert!(h.state_mut().execute("page.bulk_crop"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::BulkCrop));
    assert_eq!(h.state().bulk_crop_draft.blended_margins, [20.0; 4]);

    // 3. Switch to Page-by-Page mode and edit only page 1
    h.state_mut().bulk_crop_draft.mode = pdfcraft_ui_egui::bulk_crop::BulkCropMode::PageByPage;
    h.state_mut().bulk_crop_draft.edited_pages.insert(1);
    h.state_mut().bulk_crop_draft.page_margins[1] = [30.0; 4];
    h.run_steps(2);
    h.get_by_label("Crop Pages").click();
    h.run_steps(4);

    // Page 0 must preserve its existing crop and NOT reset to 300x400
    assert_eq!(size(&h, 0), (260.0, 360.0));
    // Page 1 gets the updated 30pt crop: 300 - 60 = 240, 400 - 60 = 340
    assert_eq!(size(&h, 1), (240.0, 340.0));
}

#[test]
fn bulk_crop_dialog_clear_crop_and_reset() {
    let mut h = harness();
    // 1. Initial crop of 20pt
    assert!(h.state_mut().execute("page.bulk_crop"));
    h.run_steps(3);
    h.state_mut().bulk_crop_draft.blended_margins = [20.0; 4];
    h.run_steps(2);
    h.get_by_label("Crop Pages").click();
    h.run_steps(4);
    assert_eq!(size(&h, 0), (260.0, 360.0));

    // 2. Open bulk crop again
    assert!(h.state_mut().execute("page.bulk_crop"));
    h.run_steps(3);
    assert_eq!(h.state().bulk_crop_draft.blended_margins, [20.0; 4]);

    // 3. Click Clear crop: removes crop to full sheet
    h.get_by_label("Clear crop").click();
    h.run_steps(2);
    assert_eq!(h.state().bulk_crop_draft.blended_margins, [0.0; 4]);

    // 4. Click Reset: restores initial 20pt crop
    h.get_by_label("Reset").click();
    h.run_steps(2);
    assert_eq!(h.state().bulk_crop_draft.blended_margins, [20.0; 4]);

    // 5. Clear crop and confirm
    h.get_by_label("Clear crop").click();
    h.run_steps(2);
    h.get_by_label("Crop Pages").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(size(&h, 0), (300.0, 400.0));
    assert_eq!(size(&h, 1), (300.0, 400.0));
}

#[test]
fn bulk_crop_dialog_clear_crop_then_edit_margins_applies_new_crop() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.bulk_crop"));
    h.run_steps(3);
    // Click Clear crop
    h.get_by_label("Clear crop").click();
    h.run_steps(2);
    assert_eq!(h.state().bulk_crop_draft.blended_margins, [0.0; 4]);

    // Set new margins after Clear crop
    h.state_mut().bulk_crop_draft.blended_margins = [15.0; 4];
    h.run_steps(2);
    h.get_by_label("Crop Pages").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    // Margins must apply (300 - 30 = 270, 400 - 30 = 370), not be discarded as Remove
    assert_eq!(size(&h, 0), (270.0, 370.0));
    assert_eq!(size(&h, 1), (270.0, 370.0));
}

#[test]
fn bulk_crop_dialog_switching_to_page_by_page_marks_pages_edited() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.bulk_crop"));
    h.run_steps(3);
    h.state_mut().bulk_crop_draft.blended_margins = [25.0; 4];
    h.run_steps(2);

    // Switch mode to Page by page: radio button click triggers the transition
    h.get_by_label("Page by page").click();
    h.run_steps(2);
    assert_eq!(h.state().bulk_crop_draft.mode, pdfcraft_ui_egui::bulk_crop::BulkCropMode::PageByPage);

    // Confirm: carried-over margins must be applied to the pages
    h.get_by_label("Crop Pages").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(size(&h, 0), (250.0, 350.0));
    assert_eq!(size(&h, 1), (250.0, 350.0));
}

#[test]
fn duplicate_pages_copies_the_current_page() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.duplicate"));
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().info.pages.len(), 3);
}

#[test]
fn the_crop_tool_leaves_the_page_grid_so_a_page_can_be_cropped() {
    let mut h = harness();
    // The Organize panel's thumbnail grid is on screen for this view: choosing
    // "Crop pages" from it must leave the grid so the canvas handles a drag on
    // a page instead of returning early to organize_grid.
    h.state_mut().views[0].organize = true;
    assert!(h.state_mut().execute("page.crop"));
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
