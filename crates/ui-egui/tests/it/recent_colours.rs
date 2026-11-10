//! The colour pickers' Recent colours row is kept across restarts (#6577), and saved settings
//! are held to the same rules as a colour picked in the session.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::comments::{recent_custom_colours, remember_custom_colour};

/// A fresh shell that starts from `saved` settings, as the next launch would.
fn harness(saved: Option<String>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        if let Some(saved) = saved {
            app.restore(&saved);
        }
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    h
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

#[test]
fn recent_colours_survive_a_restart() {
    let (pink, blue) = ([0.96, 0.42, 0.71], [0.25, 0.55, 0.95]);
    let h = harness(None);
    remember_custom_colour(&h.ctx, pink);
    remember_custom_colour(&h.ctx, blue);
    let saved = h.state().persist();

    let again = harness(Some(saved.clone()));
    let recent = recent_custom_colours(&again.ctx);
    assert_eq!(recent.len(), 2, "both colours come back: {recent:?}");
    assert!(near(recent[0], blue) && near(recent[1], pink), "newest first: {recent:?}");
    // Saved again as they came back, so a second restart keeps them too.
    let resaved: serde_json::Value = serde_json::from_str(&again.state().persist()).unwrap();
    assert_eq!(resaved["recent_colours"].as_array().map(Vec::len), Some(2));
    let third = harness(Some(again.state().persist()));
    assert_eq!(recent_custom_colours(&third.ctx), recent);
}

#[test]
fn colours_picked_after_a_restart_are_kept_and_saved() {
    let (pink, blue, green) = ([0.96, 0.42, 0.71], [0.25, 0.55, 0.95], [0.3, 0.8, 0.4]);
    let h = harness(None);
    remember_custom_colour(&h.ctx, pink);
    remember_custom_colour(&h.ctx, blue);
    let mut again = harness(Some(h.state().persist()));
    // A new pick goes first and later frames don't put the restored row back over it.
    remember_custom_colour(&again.ctx, green);
    again.run_steps(3);
    let recent = recent_custom_colours(&again.ctx);
    assert!(recent.len() == 3 && near(recent[0], green) && near(recent[1], blue) && near(recent[2], pink), "{recent:?}");
    // Picking a restored colour again moves it first, once.
    remember_custom_colour(&again.ctx, pink);
    again.run_steps(3);
    let recent = recent_custom_colours(&again.ctx);
    assert!(recent.len() == 3 && near(recent[0], pink) && near(recent[1], green) && near(recent[2], blue), "{recent:?}");
    // And that is what is saved.
    let saved: serde_json::Value = serde_json::from_str(&again.state().persist()).unwrap();
    assert_eq!(saved["recent_colours"].as_array().map(Vec::len), Some(3));
    assert_eq!(recent_custom_colours(&harness(Some(again.state().persist())).ctx), recent);
}

#[test]
fn a_colour_that_clamps_to_a_swatch_is_not_a_recent_colour() {
    let h = harness(None);
    // Black and Red are swatches, the first only once clamped.
    remember_custom_colour(&h.ctx, [-1.0, 0.0, 0.0]);
    remember_custom_colour(&h.ctx, [0.89, 0.13, 0.13]);
    assert!(recent_custom_colours(&h.ctx).is_empty(), "{:?}", recent_custom_colours(&h.ctx));
}

#[test]
fn saved_recent_colours_are_untrusted() {
    // Entries that are not three finite numbers are dropped, values are clamped, a repeat is
    // kept once, and only the five newest usable colours stay.
    let mut list = vec![
        serde_json::json!("pink"),
        serde_json::json!([0.5, 0.5]),
        serde_json::json!(7),
        serde_json::json!([1.7, -0.5, 0.25]),
        serde_json::json!([0.1, 0.2, 0.3]),
        serde_json::json!([0.1, 0.2, 0.3]),
    ];
    for i in 0..1000 {
        list.push(serde_json::json!([0.4, 0.6, f64::from(i % 10) / 10.0]));
    }
    let saved = serde_json::json!({ "recent_colours": list }).to_string();
    let h = harness(Some(saved));
    let recent = recent_custom_colours(&h.ctx);
    assert_eq!(recent.len(), 5, "capped: {recent:?}");
    assert!(near(recent[0], [1.0, 0.0, 0.25]), "clamped, newest first: {recent:?}");
    assert!(near(recent[1], [0.1, 0.2, 0.3]), "kept once: {recent:?}");
    assert_eq!(recent.iter().filter(|c| near(**c, [0.1, 0.2, 0.3])).count(), 1);
    assert!(recent.iter().flatten().all(|v| (0.0..=1.0).contains(v)));

    // Swatches are not repeated in the row, from settings either.
    let saved = serde_json::json!({ "recent_colours": [[0.0, 0.0, 0.0], [0.2, 0.4, 0.6], [0.89, 0.13, 0.13]] }).to_string();
    let recent = recent_custom_colours(&harness(Some(saved)).ctx);
    assert!(recent.len() == 1 && near(recent[0], [0.2, 0.4, 0.6]), "no swatches: {recent:?}");

    // Only the first 64 entries are read: usable colours after them are not.
    let mut list = vec![serde_json::json!("x"); 64];
    list.push(serde_json::json!([0.2, 0.4, 0.6]));
    let saved = serde_json::json!({ "recent_colours": list }).to_string();
    assert!(recent_custom_colours(&harness(Some(saved)).ctx).is_empty(), "read up to a bound");

    // A settings file of another shape leaves the row empty and doesn't fail.
    for odd in [r#"{"recent_colours": "red"}"#, r#"{"recent_colours": {"0": [1, 0, 0]}}"#, r#"{"recent_colours": null}"#] {
        let h = harness(Some(odd.to_string()));
        assert!(recent_custom_colours(&h.ctx).is_empty(), "{odd}");
    }
}

#[test]
fn recent_colours_read_back_are_saved_before_the_first_frame() {
    // Settings saved again before any frame has run (no egui context yet) keep the row.
    let mut app = PdfCraftApp::new();
    app.restore(r#"{"recent_colours": [[0.2, 0.4, 0.6], [1.5, 0.0, 0.0]]}"#);
    let saved: serde_json::Value = serde_json::from_str(&app.persist()).unwrap();
    assert_eq!(saved["recent_colours"], serde_json::json!([[0.2, 0.4, 0.6], [1.0, 0.0, 0.0]]));
}
