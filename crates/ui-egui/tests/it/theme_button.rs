//! The title bar's theme button: a click cycles System, Light and Dark; a right-click opens the
//! menu with the appearance settings.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(6);
    h
}

fn preference(h: &Harness<'static, PdfCraftApp>) -> String {
    format!("{:?}", h.state().theme_preference)
}

#[test]
fn a_click_cycles_the_theme_and_a_right_click_opens_the_menu() {
    let mut h = harness();
    assert_eq!(preference(&h), "Light", "new installs start Light");
    for (label, next) in
        [("Display theme: Light gray", "Dark"), ("Display theme: Dark gray", "System"), ("Display theme: Use system setting", "Light")]
    {
        h.get_by_label(label).click();
        h.run_steps(4);
        assert_eq!(preference(&h), next);
    }
    assert!(h.query_by_label("Appearance settings…").is_none(), "a plain click opens no menu");

    h.get_by_label("Display theme: Light gray").click_secondary();
    h.run_steps(4);
    h.get_by_label("Appearance settings…").click();
    h.run_steps(4);
    h.run_steps(10);
    assert!(h.query_by_label("Light theme").is_some(), "Preferences opens on Appearance");
}
