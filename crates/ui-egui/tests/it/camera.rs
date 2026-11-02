//! Scan & OCR ▸ Enhance ▸ Camera image in the real shell (egui_kittest): drag a corner, then
//! Enhance page; Cancel and Esc leave the page as it was.

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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "100").unwrap();
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

fn undo_label(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().can_undo().map(str::to_owned)
}

#[test]
fn dragging_a_corner_then_enhance_page_straightens_the_page() {
    let mut h = harness();
    assert!(h.state_mut().execute("ocr.enhance_camera"));
    assert_eq!(h.state().quick_tool, QuickTool::EnhanceCamera);
    h.run_steps(3);
    let quad = h.state().views[0].camera.expect("the corners start on the current page");
    assert_eq!(quad.page, 0);
    assert_eq!(quad.corners, [[15.0, 20.0], [285.0, 20.0], [285.0, 380.0], [15.0, 380.0]], "inset by 5%");

    // Drag the top-left corner to (60, 80) on the page.
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let (a, b) = (r.min + r.size() * 0.05, r.min + r.size() * 0.2);
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(2);
    let c = h.state().views[0].camera.unwrap().corners[0];
    assert!((c[0] - 60.0).abs() < 1.0 && (c[1] - 80.0).abs() < 1.0, "{c:?}");
    assert_eq!(undo_label(&h), None, "dragging changes nothing yet");

    h.get_by_label("Enhance page").click();
    h.run_steps(4);
    // The longer of each pair of opposite sides: 270 × 360 pt.
    let (w, hh) = size(&h, 0);
    assert!((w - 270.0).abs() < 2.0 && (hh - 360.0).abs() < 2.0, "{w}×{hh}");
    assert_eq!(size(&h, 1), (300.0, 400.0), "only the page with the corners changes");
    assert_eq!(undo_label(&h).as_deref(), Some("Enhance camera image"));
    // The tool stays open for the enhanced page: turn it, then Done.
    assert_eq!(h.state().quick_tool, QuickTool::EnhanceCamera);
    assert!(h.state().views[0].camera.is_none());
    assert_eq!(h.state().views[0].camera_done.map(|d| d.page), Some(0));
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(4);
    let (w, hh) = size(&h, 0);
    assert!((w - 360.0).abs() < 2.0 && (hh - 270.0).abs() < 2.0, "{w}×{hh}");
    assert_eq!(undo_label(&h).as_deref(), Some("Rotate page"));
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(h.state().views[0].camera_done, None);
}

#[test]
fn the_clean_up_level_enhances_again_from_the_photo_and_keeps_the_turn() {
    let mut h = harness();
    assert!(h.state_mut().execute("ocr.enhance_camera"));
    h.run_steps(3);
    h.get_by_label("Enhance page").click();
    h.run_steps(4);
    let done = h.state().views[0].camera_done.unwrap();
    assert_eq!((done.look, done.level, done.steps), (pdfcraft_engine::camera_clean::Look::Colour, 50, 1), "enhanced in colour at the default level");
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(4);
    let turned = size(&h, 0);
    assert!(turned.0 > turned.1, "{turned:?}");

    // Let go of the slider at 100.
    let view = &mut h.state_mut().views[0];
    if let Some(d) = view.camera_done.as_mut() {
        d.level = 100;
    }
    view.pending_action = Some(pdfcraft_ui_egui::canvas::ViewAction::CameraLevel);
    h.run_steps(4);
    let done = h.state().views[0].camera_done.unwrap();
    assert_eq!((done.applied.1, done.steps, done.turns), (100, 2, 90));
    assert_eq!(size(&h, 0), turned, "still turned");
    // The history holds one enhancement and one turn, not a pile of them.
    let s = h.state_mut();
    let id = s.views[0].id;
    assert_eq!(s.session.undo(id).unwrap().as_str(), "Rotate page");
    assert_eq!(s.session.undo(id).unwrap().as_str(), "Enhance camera image");
    assert_eq!(s.session.get(id).unwrap().can_undo(), None);
    let p = &s.session.get(id).unwrap().info.pages[0];
    assert_eq!((p.width, p.height), (300.0, 400.0), "back to the photo");
}

#[test]
fn choosing_a_look_enhances_again() {
    use pdfcraft_engine::camera_clean::Look;
    let mut h = harness();
    assert!(h.state_mut().execute("ocr.enhance_camera"));
    h.run_steps(3);
    h.get_by_label("Enhance page").click();
    h.run_steps(4);
    for (label, look) in [("Black and white", Look::BlackWhite), ("Greyscale", Look::Greyscale), ("Original", Look::Original)] {
        h.get_by_label(label).click();
        h.run_steps(4);
        let done = h.state().views[0].camera_done.unwrap();
        assert_eq!((done.applied.0, done.steps), (look, 1), "{label}");
        let s = h.state();
        assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Enhance camera image"), "{label}");
    }
}

#[test]
fn after_enhance_page_crop_and_organize_take_over() {
    for (button, then) in [("Crop pages", QuickTool::Crop), ("Organize pages", QuickTool::Select)] {
        let mut h = harness();
        assert!(h.state_mut().execute("ocr.enhance_camera"));
        h.run_steps(3);
        h.get_by_label("Enhance page").click();
        h.run_steps(4);
        assert_eq!(h.state().views[0].camera_done.map(|d| d.page), Some(0));
        h.get_by_label(button).click();
        h.run_steps(3);
        assert_eq!(h.state().quick_tool, then, "{button}");
        assert_eq!(h.state().views[0].camera_done, None, "{button}");
        assert_eq!(h.state().views[0].organize, button == "Organize pages");
    }
}

#[test]
fn cancel_and_escape_leave_the_page_alone() {
    let mut h = harness();
    assert!(h.state_mut().execute("ocr.enhance_camera"));
    h.run_steps(3);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert!(h.state().views[0].camera.is_none());

    assert!(h.state_mut().execute("ocr.enhance_camera"));
    h.run_steps(3);
    assert!(h.state().views[0].camera.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert!(h.state().views[0].camera.is_none());
    assert_eq!(size(&h, 0), (300.0, 400.0));
    assert_eq!(undo_label(&h), None);
}

/// A turn made outside the tool after the enhancement breaks its run in the history: changing the
/// level then leaves the page and the history as they are instead of enhancing the straightened
/// page a second time.
#[test]
fn the_level_leaves_the_page_alone_when_another_edit_came_in_between() {
    let mut h = harness();
    assert!(h.state_mut().execute("ocr.enhance_camera"));
    h.run_steps(3);
    h.get_by_label("Enhance page").click();
    h.run_steps(4);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(4);
    // Another "Rotate page" from outside the tool, with the same history label.
    {
        let s = h.state_mut();
        let id = s.views[0].id;
        s.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    }
    h.run_steps(2);
    let before = size(&h, 0);
    let applied = h.state().views[0].camera_done.unwrap().applied;

    let view = &mut h.state_mut().views[0];
    if let Some(d) = view.camera_done.as_mut() {
        d.level = 100;
    }
    view.pending_action = Some(pdfcraft_ui_egui::canvas::ViewAction::CameraLevel);
    h.run_steps(4);
    assert_eq!(size(&h, 0), before, "the page is not warped again");
    let done = h.state().views[0].camera_done.unwrap();
    assert_eq!((done.level, done.applied), (applied.1, applied), "the slider goes back");
    let s = h.state_mut();
    let id = s.views[0].id;
    assert_eq!(s.session.undo(id).unwrap().as_str(), "Rotate page");
    assert_eq!(s.session.undo(id).unwrap().as_str(), "Rotate page");
    assert_eq!(s.session.undo(id).unwrap().as_str(), "Enhance camera image");
    assert_eq!(s.session.get(id).unwrap().can_undo(), None);
}
