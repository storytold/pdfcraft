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

#[test]
fn segmented_pill_controls_switch_modes_and_layouts() {
    let mut h = harness(900.0);
    // Switch workspace mode via segmented pill
    h.get_by_role_and_label(egui::accesskit::Role::RadioButton, "Convert").scroll_to_me();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::RadioButton, "Convert").click();
    h.run_steps(2);
    assert_eq!(h.state().default_mode, pdfcraft_ui_egui::Mode::Convert);

    let continuous_before = h.state().view_defaults.continuous;

    // Switch page layout via segmented pill
    h.get_by_label("Two-page view").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Two-page view").click();
    h.run_steps(2);
    assert_eq!(h.state().view_defaults.layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp);
    assert_eq!(h.state().view_defaults.continuous, continuous_before, "Two-page view retains continuous scrolling");

    // Switch continuous scrolling pill (can be combined with Two-page view)
    h.get_by_label("Continuous scrolling").click();
    h.run_steps(2);
    assert_eq!(h.state().view_defaults.continuous, !continuous_before);
    assert_eq!(h.state().view_defaults.layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp, "two-page continuous scrolling is supported");

    // Switch to single page mode (continuous setting remains independent)
    h.get_by_label("Single page").click();
    h.run_steps(2);
    assert_eq!(h.state().view_defaults.layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    assert_eq!(h.state().view_defaults.continuous, !continuous_before);

    // Switch tools panel side via segmented pill
    h.get_by_label("Left").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Left").click();
    h.run_steps(2);
    assert_eq!(h.state().side_panels.tools_side, pdfcraft_ui_egui::side_panels::ToolsSide::Left);

    // Switch measurement units via segmented pill
    h.get_by_label("pt").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("pt").click();
    h.run_steps(2);
    assert_eq!(h.state().units, pdfcraft_ui_egui::units::Unit::Points);
}

#[test]
fn search_settings_scans_and_highlights_in_situ_without_hiding_cards() {
    let mut h = harness(900.0);
    // Initially all card titles exist
    assert!(h.query_by_label("General & Interface").is_some());
    assert!(h.query_by_label("Comment").is_some());
    assert!(h.query_by_label("JavaScript").is_some());

    // Type "Hide hover" in search box
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").click();
    h.run_steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").type_text("Hide hover");
    h.run_steps(4);

    // In browser-style search, cards are NOT hidden
    assert!(h.query_by_label("General & Interface").is_some());
    assert!(h.query_by_label("Comment").is_some());
    assert!(h.query_by_label("JavaScript").is_some());

    // The setting "Hide hover previews for highlights without comment text" is present and matched
    assert!(h.query_by_label("Hide hover previews for highlights without comment text").is_some());
    // Match badge displays 1 match
    assert!(h.query_by_label("1 match").is_some());

    // Clearing search clears match badge
    h.get_by_label("Clear search").click();
    h.run_steps(4);
    assert!(h.query_by_label("1 match").is_none());

    // Search for "javascript"
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").click();
    h.run_steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").type_text("javascript");
    h.run_steps(4);

    // Cards remain visible and multiple matches are reported
    assert!(h.query_by_label("JavaScript").is_some());
    assert!(h.query_by_label("General & Interface").is_some());
    assert!(h.query_by_label_contains("matches").is_some());

    // Clearing search shows everything again
    h.get_by_label("Clear search").click();
    h.run_steps(4);
    assert!(h.query_by_label("General & Interface").is_some());
    assert!(h.query_by_label("Comment").is_some());
    assert!(h.query_by_label("JavaScript").is_some());
}

#[test]
fn preferences_dialog_scrollbar_spacing_and_controls() {
    let h = harness(600.0);
    let title = h.get_by(|n| n.role() == egui::accesskit::Role::Label && n.value().as_deref() == Some("Preferences")).rect();
    let search = h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").rect();
    let ok = h.get_by_label("OK").rect();
    assert!(on_screen(ok, 600.0));
    // Header title and search box are vertically aligned
    assert!((title.center().y - search.center().y).abs() <= 2.0);
    // Search box right edge is aligned within the dialog
    assert!(search.max.x <= ok.max.x);
}

#[test]
fn search_scrolls_option_only_and_theme_card_matches_into_visible_viewport() {
    let mut h = harness(480.0);
    let title = h.get_by(|n| n.role() == egui::accesskit::Role::Label && n.value().as_deref() == Some("Preferences")).rect();
    let ok = h.get_by_label("OK").rect();

    // "Two-page view" is an option in Card 3 (Documents and view).
    // In a 480 pt window, it starts out below the visible scroll viewport.
    let opt_before = h.get_by_label("Two-page view").rect();
    assert!(opt_before.min.y >= ok.min.y, "Two-page option starts out below the visible viewport: {opt_before:?} vs OK {ok:?}");

    // Searching "Two-page" (which only occurs inside that segmented pill option)
    // auto-scrolls the matching option into the visible viewport between title and OK.
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").click();
    h.run_steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").type_text("Two-page");
    h.run_steps(4);

    let opt_after = h.get_by_label("Two-page view").rect();
    assert!(
        opt_after.min.y >= title.max.y && opt_after.max.y <= ok.min.y,
        "Two-page option scrolled into visible scroll viewport: {opt_after:?} between title {title:?} and OK {ok:?}"
    );

    // Clear search and test a theme-card-only match
    h.get_by_label("Clear search").click();
    h.run_steps(4);

    // Search for "Dark theme" (which occurs only in the dark theme card in Card 2).
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").click();
    h.run_steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").type_text("Dark theme");
    h.run_steps(4);

    let dark_card = h.get_by_label("Dark theme").rect();
    assert!(
        dark_card.min.y >= title.max.y && dark_card.max.y <= ok.min.y,
        "Dark theme card scrolled into visible viewport: {dark_card:?} between title {title:?} and OK {ok:?}"
    );
}

#[test]
fn search_settings_finds_the_discord_preference() {
    let mut h = harness(900.0);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").click();
    h.run_steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Search settings…").type_text("Discord");
    h.run_steps(4);
    assert!(h.query_by_label("Show the Discord button in the title bar and the community card on Home").is_some());
    assert!(h.query_by_label("1 match").is_some());
}
