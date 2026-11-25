//! Crop tool and Set Page Boxes in the real shell (egui_kittest).

use crate::support::AppHarness as _;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable};
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

#[test]
fn set_page_boxes_preview_updates_its_physical_dimensions() {
    use crate::support::input::one_frame;
    use egui::{Event, Key, Modifiers};
    use pdfcraft_cos::{Document, Object, write_incremental};
    use pdfcraft_ui_egui::units::Unit;

    // Original synthetic PDF. An inset crop on a turned page separates the shown size from
    // both the unturned crop and the media box, and /UserUnit separates it from raw coordinates.
    let mut doc = Document::open(std::sync::Arc::new(crate::support::fixtures::pdf_pages(1))).unwrap();
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| {
        d.set(b"Rotate".to_vec(), Object::Int(90));
        d.set(b"UserUnit".to_vec(), Object::Int(2));
        d.set(b"CropBox".to_vec(), Object::Array([20, 30, 250, 170].map(Object::Int).to_vec()));
    })
    .unwrap();
    let bytes = write_incremental(&doc, &Default::default()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::control::new_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.units = Unit::Points;
        app.open_bytes(&cc.egui_ctx, "rotated-crop.pdf", None, bytes.clone()).unwrap();
        app
    });
    h.run_steps(6);
    let (width, height) = size(&h, 0);
    let (width, height) = (f64::from(width), f64::from(height));
    assert_eq!((width, height), (280.0, 460.0), "the synthetic CropBox in displayed points");
    let open = |h: &mut Harness<'static, PdfCraftApp>| {
        assert!(h.execute("page.boxes"));
        h.run_steps(4);
        h.get_by_role_and_label(Role::Dialog, "Set Page Boxes");
    };
    let shows = |h: &Harness<'static, PdfCraftApp>, text: &str| {
        h.get_by_role_and_label(Role::Dialog, "Set Page Boxes").query_by_label_contains(text).is_some()
    };
    open(&mut h);
    let original = Unit::Points.size(width, height);
    let original_shown = shows(&h, &original);

    // An extra 20 physical points off the top of the unturned page reduces the displayed width.
    h.get_by_label("Top").click();
    h.run_steps(1);
    let key = |h: &mut Harness<'static, PdfCraftApp>, key, modifiers| {
        for pressed in [true, false] {
            one_frame(h, [Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }]);
        }
    };
    key(&mut h, Key::A, Modifiers::COMMAND);
    one_frame(&mut h, [Event::Text("80".into())]);
    key(&mut h, Key::Tab, Modifiers::NONE);
    let edited = Unit::Points.size(width - 20.0, height);
    assert_eq!(size(&h, 0), (width as f32, height as f32), "the preview has not applied the edit");
    assert!(shows(&h, &edited) && !shows(&h, &original), "after Tab and before OK, the preview should shrink from {original:?} to {edited:?}");
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(size(&h, 0), ((width - 20.0) as f32, height as f32), "the dialog received the margin change");

    h.state_mut().open_preferences();
    h.run_steps(4);
    h.get_by_role_and_label(Role::RadioGroup, "Units").scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(Role::RadioGroup, "Units").get_by_role_and_label(Role::RadioButton, "mm").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().units, Unit::Millimetres);

    open(&mut h);
    let metric = Unit::Millimetres.size(width - 20.0, height);
    assert!(original_shown, "the original preview shows {original:?}");
    assert!(shows(&h, &metric), "the preview follows the Units preference and shows {metric:?}");
    let preview = h.get_by_role_and_label(Role::Image, "Page 1");
    let label = format!("Box size: {metric}");
    let readout = h.get_by_role_and_label(Role::Label, &label);
    let preview = preview.accesskit_node();
    let readout = readout.accesskit_node();
    assert!(preview.description().is_some_and(|description| description.contains(&metric)));
    assert_eq!(readout.live(), egui::accesskit::Live::Polite);
    assert!(readout.bounding_box().unwrap().y0 >= preview.bounding_box().unwrap().y1);
}
