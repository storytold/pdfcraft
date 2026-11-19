//! Crop tool, Set Page Boxes and Duplicate pages in the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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
    crop_middle_half(&mut h);
}

#[test]
fn quick_toolbar_crop_crops_the_page_and_undo_restores_it() {
    let mut h = harness();
    h.get_by_label("Crop pages").click();
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Crop);
    assert!(!h.state().views[0].organize);
    crop_middle_half(&mut h);
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    assert_eq!(size(&h, 0), (300.0, 400.0));
}

#[test]
fn quick_toolbar_crop_is_disabled_when_page_assembly_is_forbidden() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(FIXTURE.to_vec())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: 0b0100,
        encrypt_metadata: true,
        seed: [5; 32],
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("restricted.pdf", None, bytes).unwrap();
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(6);
    assert!(h.get_by_label("Crop pages").accesskit_node().is_disabled());
    assert!(!h.state_mut().execute("page.crop"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
    assert_eq!(size(&h, 0), (300.0, 400.0));
}

#[test]
fn quick_toolbar_crop_is_hidden_on_home_and_in_read_mode() {
    let mut h = harness();
    h.state_mut().execute("view.read_mode");
    h.run_steps(3);
    assert!(h.query_by_label("Crop pages").is_none());
    h.state_mut().execute("view.read_mode");
    h.run_steps(3);
    h.get_by_label("Crop pages");
    h.state_mut().active = None;
    h.run_steps(3);
    assert!(h.query_by_label("Crop pages").is_none());
}

fn crop_middle_half(h: &mut Harness<'static, PdfCraftApp>) {
    drag_middle_half(h);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    let (w, hh) = size(h, 0);
    assert!((w - 150.0).abs() < 2.0 && (hh - 200.0).abs() < 2.0, "{w}×{hh}");
    assert_eq!(size(h, 1), (300.0, 400.0));
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
    drag_middle_half(&mut h);
    h.get_by_label("OK").click();
    h.run_steps(4);
    let (w, hh) = size(&h, 0);
    assert!((w - 150.0).abs() < 2.0 && (hh - 200.0).abs() < 2.0, "{w}×{hh}");
    assert_eq!(size(&h, 1), (300.0, 400.0));
}

/// Drags a crop rectangle over the middle half of page 0 and checks that Set Page Boxes opens
/// with its margins, for that page alone, and that nothing is cropped yet (#5695).
fn drag_middle_half(h: &mut Harness<'static, PdfCraftApp>) {
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
    assert_eq!(h.state().dialog, Some(Dialog::PageBoxes), "a drawn rectangle opens Set Page Boxes");
    h.get_by_label("Set Page Boxes");
    assert_eq!(size(h, 0), (300.0, 400.0), "nothing is cropped before OK");
    let d = &h.state().boxes_draft;
    // The crop box of page 0 alone, of the two.
    let edit = d.edit(0, 2, &[]);
    assert!(matches!(&edit, pdfcraft_engine::Edit::SetPageBox { pages, which: pdfcraft_engine::PageBox::Crop, .. } if pages == &[0]), "{edit:?}");
    // Left, bottom, right, top: a quarter of the 300 × 400 page in from each side.
    for (m, want) in d.margins.iter().zip([75.0, 100.0, 75.0, 100.0]) {
        assert!((m - want).abs() < 2.0, "{:?}", d.margins);
    }
}

#[test]
fn a_drawn_crop_rectangle_can_be_fine_tuned_before_it_is_applied() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.crop"));
    h.run_steps(2);
    drag_middle_half(&mut h);
    // Keep the left edge: a zero margin leaves it at the media box (#5695).
    h.state_mut().boxes_draft.margins[0] = 0.0;
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    let (w, hh) = size(&h, 0);
    assert!((w - 225.0).abs() < 2.0 && (hh - 200.0).abs() < 2.0, "{w}×{hh}");
    assert_eq!(size(&h, 1), (300.0, 400.0), "only the page the rectangle was drawn on");
}

#[test]
fn a_crop_rectangle_drawn_on_a_page_mostly_scrolled_away_crops_that_page() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.crop"));
    h.run_steps(2);
    // Scroll until page 1 fills most of the view and is current, with the bottom of page 0
    // still on screen above it.
    let middle = h.state().views[0].page_screen_rect(0).unwrap().center();
    for _ in 0..40 {
        if h.state().views[0].current == 1 {
            break;
        }
        h.hover_at(middle);
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -40.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.run_steps(3);
    }
    assert_eq!(h.state().views[0].current, 1, "scrolled to page 1");
    // A strip across the bottom of page 0, the part of it left on screen. While the pointer is
    // down, page 1 shows the most and goes back to being current.
    let r0 = h.state().views[0].page_screen_rect(0).unwrap();
    let a = egui::pos2(r0.min.x + r0.width() * 0.25, r0.max.y - 40.0);
    let b = egui::pos2(r0.min.x + r0.width() * 0.75, r0.max.y - 10.0);
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
    assert_eq!(h.state().dialog, Some(Dialog::PageBoxes));
    assert_eq!(h.state().views[0].current, 1, "the page shown most stays current");
    assert_eq!(h.state().boxes_draft.page, Some(0), "the dialog is for the page the rectangle was drawn on");
    assert_eq!(h.state().boxes_draft.seeded, Some((0, pdfcraft_engine::PageBox::Crop)), "and keeps its margins");
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(size(&h, 1), (300.0, 400.0), "the current page is left alone");
    // The middle half across and a strip of the bottom: page 0, not the page shown most.
    let (w, hh) = size(&h, 0);
    assert!((w - 150.0).abs() < 2.0 && (5.0..=25.0).contains(&hh), "page 0 is cropped to the strip: {w}×{hh}");
}

#[test]
fn cancelling_set_page_boxes_after_a_drawn_rectangle_crops_nothing() {
    let mut h = harness();
    assert!(h.state_mut().execute("page.crop"));
    h.run_steps(2);
    drag_middle_half(&mut h);
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(size(&h, 0), (300.0, 400.0));
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), None);
    // Set Page Boxes opened afterwards starts from the page's own box, not the old rectangle.
    assert!(h.state_mut().execute("page.boxes"));
    h.run_steps(3);
    assert_eq!(h.state().boxes_draft.margins, [0.0; 4]);
}
