//! The Smooth images display preference is app-wide, persistent and never a PDF edit.

use pdfcraft_ui_egui::PdfCraftApp;

#[test]
fn smooth_images_preference_round_trips_and_ignores_invalid_settings() {
    let mut app = PdfCraftApp::new();
    let enabled = |app: &PdfCraftApp| serde_json::from_str::<serde_json::Value>(&app.persist()).unwrap()["smooth_images"].clone();
    assert_eq!(enabled(&app), true);
    app.restore("{}");
    assert_eq!(enabled(&app), true);
    for value in [false, true] {
        app.set_option("smooth_images", if value { "true" } else { "false" }).unwrap();
        assert_eq!(enabled(&app), value);
        let mut restored = PdfCraftApp::new();
        restored.restore(&app.persist());
        assert_eq!(enabled(&restored), value);
    }
    for invalid in ["null", "0", "\"false\"", "[]", "{}"] {
        app.restore(&format!("{{\"smooth_images\":{invalid}}}"));
        assert_eq!(enabled(&app), true);
    }
    app.restore("not json");
    assert_eq!(enabled(&app), true);
    assert!(app.set_option("smooth_images", "maybe").is_err());
    assert_eq!(enabled(&app), true);

    // The same invalid values from a false preference must leave it false, not reset it to the
    // default on.
    app.set_option("smooth_images", "false").unwrap();
    for invalid in ["null", "0", "\"true\"", "[]", "{}"] {
        app.restore(&format!("{{\"smooth_images\":{invalid}}}"));
        assert_eq!(enabled(&app), false);
    }
    app.restore("not json");
    assert_eq!(enabled(&app), false);
    assert!(app.set_option("smooth_images", "maybe").is_err());
    assert_eq!(enabled(&app), false);
}
