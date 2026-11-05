//! Add a stamp: the palette and placing stamps on the page.

use egui::{Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

#[test]
fn choosing_and_placing_stamps() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    assert!(h.state_mut().execute("comment.stamp"));
    h.run_steps(2);
    h.get_by_label("Add a stamp");
    h.get_all_by_label("APPROVED").next().expect("dynamic approved").click();
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Stamp(pdfcraft_engine::StampKind::DynApproved));
    let p = {
        let r = h.state().views[0].page_screen_rect(0).unwrap();
        r.min + egui::vec2(r.width() * 0.5, r.height() * 0.85)
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let stamps: Vec<_> = doc.info.annotations.iter().filter(|a| a.subtype == "Stamp").collect();
    assert_eq!(stamps.len(), 1);
    assert_eq!(s.quick_tool, QuickTool::Select, "back to selecting after one stamp");
    for _ in 0..30 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if let Ok(dir) = std::env::var("PDFCRAFT_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(format!("{dir}/stamps.png")).unwrap();
    }
}

#[test]
fn editing_the_second_custom_stamp_keeps_both_library_entries() {
    use egui::accesskit::Role;

    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app
    });
    h.run_steps(5);
    assert!(h.state_mut().execute("comment.stamp"));
    h.run_steps(2);
    for (file, name, byte) in [("first.png", "First", 1), ("second.png", "Second", 2)] {
        h.state_mut().start_custom_stamp(file.into(), vec![byte]);
        h.state_mut().stamp_draft.category = "Tools".into();
        h.state_mut().stamp_draft.name = name.into();
        h.run_steps(1);
        h.get_by_label("OK").click();
        h.run_steps(2);
    }
    let first_id = h.state().custom_stamps[0].id;
    let second_id = h.state().custom_stamps[1].id;
    let at = h.get_by_label("Second").rect().center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::default() });
    h.event(egui::Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::default() });
    h.run_steps(2);
    h.get_by_label("Rename / change category…").click();
    h.run_steps(2);
    let category = h.get_by_role_and_label(Role::TextInput, "Category:");
    category.click();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role_and_label(Role::TextInput, "Category:").type_text("Archive");
    let name = h.get_by_role_and_label(Role::TextInput, "Name:");
    name.click();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role_and_label(Role::TextInput, "Name:").type_text("Second edited");
    h.run_steps(1);
    h.get_by_label("Save").click();
    h.run_steps(2);

    assert_eq!(h.state().custom_stamps.len(), 2);
    assert_eq!(
        (h.state().custom_stamps[0].id, h.state().custom_stamps[0].name.as_str(), h.state().custom_stamps[0].category.as_str()),
        (first_id, "First", "Tools")
    );
    assert_eq!(
        (h.state().custom_stamps[1].id, h.state().custom_stamps[1].name.as_str(), h.state().custom_stamps[1].category.as_str()),
        (second_id, "Second edited", "Archive")
    );
}

#[test]
fn natural_image_stamps_keep_displayed_bounds_on_rotated_pages() {
    for degrees in [0, 90, 180, 270] {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
            let id = app.views[0].id;
            app.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees }).unwrap();
            app.set_option("zoom", "100").unwrap();
            app
        });
        h.run_steps(6);
        assert!(h.state_mut().execute("comment.stamp"));
        let mut png = Vec::new();
        image::RgbImage::from_pixel(80, 40, image::Rgb([20, 120, 220]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        h.state_mut().start_custom_stamp("rectangle.png".into(), png);
        h.run_steps(2);
        h.get_by_label("OK").click();
        h.run_steps(2);
        let page = h.state().views[0].page_screen_rect(0).unwrap();
        let at = page.min + egui::vec2(page.width() * 0.5, page.height() * 0.6);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(4);
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(app.quick_tool, QuickTool::Select);
        let stamp = doc.info.annotations.iter().find(|a| a.subtype == "Stamp").unwrap();
        let info = &doc.info.pages[0];
        let a = info.user_to_view(stamp.rect[0], stamp.rect[1]);
        let b = info.user_to_view(stamp.rect[2], stamp.rect[3]);
        assert!(((a[0] - b[0]).abs() - 80.0).abs() < 0.001, "rotation {degrees}: {a:?}, {b:?}");
        assert!(((a[1] - b[1]).abs() - 40.0).abs() < 0.001, "rotation {degrees}: {a:?}, {b:?}");
        assert_eq!(doc.can_undo(), Some("Add stamp"));
    }
}

#[test]
fn creating_placing_and_keeping_custom_stamps() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    h.run_steps(6);
    assert!(h.state_mut().execute("comment.stamp"));
    h.run_steps(2);
    // An image becomes a stamp: Create ▸ category and name.
    let mut png = Vec::new();
    image::RgbImage::from_pixel(60, 20, image::Rgb([20, 120, 220])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    h.state_mut().start_custom_stamp("logo.png".into(), png);
    h.run_steps(2);
    h.get_by_label("Create Custom Stamp");
    h.state_mut().stamp_draft.category = "Company".into();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::CustomStamp(h.state().custom_stamps[0].id));
    h.get_by_label("Company");
    h.get_by_label("logo");
    let stamp_id = h.state().custom_stamps[0].id;
    let stamp_data = h.state().custom_stamps[0].data.clone();
    assert_eq!(h.state().custom_stamps[0].id, stamp_id);
    assert_eq!(h.state().custom_stamps[0].name, "logo");
    assert_eq!(h.state().custom_stamps[0].category, "Company");
    assert_eq!(h.state().custom_stamps[0].data, stamp_data);
    // Place it.
    let p = {
        let r = h.state().views[0].page_screen_rect(0).unwrap();
        r.min + egui::vec2(r.width() * 0.5, r.height() * 0.85)
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let stamps: Vec<_> = doc.info.annotations.iter().filter(|a| a.subtype == "Stamp").collect();
    assert_eq!(stamps.len(), 1);
    let r = stamps[0].rect;
    assert!(((r[2] - r[0]) / (r[3] - r[1]) - 3.0).abs() < 0.05, "keeps the picture's shape: {r:?}");
    assert_eq!(doc.can_undo(), Some("Add stamp"));
    // The edited library is kept with the app's settings.
    let saved = h.state().persist();
    let mut again = PdfCraftApp::new();
    again.set_option("language", "en").unwrap();
    again.restore(&saved);
    assert_eq!(again.custom_stamps.len(), 1);
    assert_eq!(
        (again.custom_stamps[0].id, again.custom_stamps[0].category.as_str(), again.custom_stamps[0].name.as_str()),
        (stamp_id, "Company", "logo")
    );
    assert_eq!(again.custom_stamps[0].data, stamp_data);
}
