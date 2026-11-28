//! Edit mode's picture menu sets an image's opacity.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::support::AppHarness as _;

#[test]
fn the_picture_menu_sets_the_image_opacity() {
    let mut png = Vec::new();
    image::RgbImage::from_pixel(80, 40, image::Rgb([200, 40, 40])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes(&egui::Context::default(), "picture.png", None, png.clone()).expect("opens");
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    assert!(h.execute("edit.edit_text"));
    h.run_steps(2);
    let c = h.state().views[0].page_screen_rect(0).expect("on screen").center();
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    assert!(h.state().views[0].session.image_selection.is_some(), "selected");
    let before = h.state().session.get(id).unwrap().page_image_opacities(0);
    assert!(before.first().is_some_and(|o| (o - 1.0).abs() < 1e-6), "opaque to start: {before:?}");

    h.hover_at(c);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: c, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: c, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    h.get_by_label("Opacity").click();
    h.run_steps(2);
    h.get_by_label("50%").click();
    h.run_steps(3);

    let after = h.state().session.get(id).unwrap().page_image_opacities(0);
    assert!(after.first().is_some_and(|o| (o - 0.5).abs() < 1e-6), "half opaque after the menu: {after:?}");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Set opacity"), "one undoable edit");
}
