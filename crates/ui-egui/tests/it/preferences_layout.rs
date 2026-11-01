//! The Preferences dialog keeps OK in reach in a short window (#3557): the settings scroll, the
//! OK row does not.

use egui::Rect;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness(height: f32) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, height)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("dialog", "preferences").unwrap();
        app
    });
    h.run_steps(6);
    h
}

fn on_screen(rect: Rect, height: f32) -> bool {
    rect.min.x >= 0.0 && rect.min.y >= 0.0 && rect.max.x <= 1000.0 && rect.max.y <= height
}

#[test]
fn ok_is_on_screen_without_scrolling_at_any_window_height() {
    // From a short laptop window to a tall monitor, the settings never fit the short ones. 173 is
    // the smallest window (520 pt) at the largest interface size (300%), 208 is 250%.
    for height in [173.0, 208.0, 260.0, 420.0, 480.0, 560.0, 700.0, 900.0, 1100.0] {
        let h = harness(height);
        let ok = h.get_by_label("OK").rect();
        assert!(on_screen(ok, height), "{height}: OK at {ok:?} is off the {height} pt window");
        let title = h.get_by(|n| n.role() == egui::accesskit::Role::Label && n.value().as_deref() == Some("Preferences")).rect();
        assert!(on_screen(title, height), "{height}: the title at {title:?} is off the window");
        // The OK row sits below the scrolled settings, never over them: the settings' last visible
        // row ends above it. The title is above everything.
        assert!(title.max.y < ok.min.y, "{height}: title {title:?} above OK {ok:?}");
    }
}

#[test]
fn ok_closes_the_dialog_in_a_short_window_and_the_settings_still_scroll() {
    let mut h = harness(480.0);
    // A setting far down the list starts out below the visible settings; scrolling brings it in
    // between the title and the OK row, and OK did not move.
    let ok_before = h.get_by_label("OK").rect();
    let title = h.get_by(|n| n.role() == egui::accesskit::Role::Label && n.value().as_deref() == Some("Preferences")).rect();
    let far = h.get_by_label_contains("Enable Acrobat JavaScript").rect();
    assert!(far.min.y >= ok_before.min.y, "JavaScript setting starts out of sight: {far:?} vs OK {ok_before:?}");
    h.get_by_label_contains("Enable Acrobat JavaScript").scroll_to_me();
    h.run_steps(4);
    let far = h.get_by_label_contains("Enable Acrobat JavaScript").rect();
    assert!(far.min.y >= title.max.y && far.max.y <= ok_before.min.y, "JavaScript setting scrolled in between the title and OK: {far:?}");
    assert_eq!(h.get_by_label("OK").rect(), ok_before, "OK stays put while the settings scroll");
    assert!(h.state().dialog.is_some());
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "OK closes Preferences");
}

#[test]
fn the_dialog_opens_at_its_full_size_and_does_not_grow_afterwards() {
    // With OK outside the scroll, the scroll area started from what the modal offered, which
    // grew some 45 pt a frame for ten frames: the dialog opened small and stretched, and clicks
    // in its first frames landed on controls that were still moving.
    for height in [260.0, 560.0, 900.0] {
        let mut h = harness(height);
        let settled = (
            h.get_by(|n| n.role() == egui::accesskit::Role::Label && n.value().as_deref() == Some("Preferences")).rect(),
            h.get_by_label("OK").rect(),
        );
        h.run_steps(8);
        let later = (
            h.get_by(|n| n.role() == egui::accesskit::Role::Label && n.value().as_deref() == Some("Preferences")).rect(),
            h.get_by_label("OK").rect(),
        );
        assert_eq!(settled, later, "{height}: the dialog moved after it had been shown for six frames");
    }
    // And it is in place within a few frames of opening, for the first click: a frame or two of
    // settling is fine, ten frames of stretching is not.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::Preferences);
    h.run_steps(3);
    let early = h.get_by_label("Right").rect();
    h.run_steps(8);
    let late = h.get_by_label("Right").rect();
    assert!((early.min.y - late.min.y).abs() <= 2.0 && (early.min.x - late.min.x).abs() <= 2.0, "{early:?} then {late:?}");
}

#[test]
fn a_very_tall_window_does_not_leave_a_blank_gap_above_ok() {
    // The settings are some 1 400 pt tall; in a taller window the scroll area is not stretched
    // past them.
    let mut h = harness(2600.0);
    h.run_steps(6);
    let ok = h.get_by_label("OK").rect();
    let last = h.get_by_label_contains("Enable Acrobat JavaScript").rect();
    assert!(on_screen(ok, 2600.0) && on_screen(last, 2600.0), "{last:?} {ok:?}");
    assert!(ok.min.y - last.max.y < 120.0, "a gap of {} pt between the last setting and OK", ok.min.y - last.max.y);
}
