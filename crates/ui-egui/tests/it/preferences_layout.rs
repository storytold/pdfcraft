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
    // A setting far down General starts out below the visible settings; scrolling brings it in
    // between the title and the OK row, and OK did not move.
    let ok_before = h.get_by_label("OK").rect();
    let title = h.get_by(|n| n.role() == egui::accesskit::Role::Label && n.value().as_deref() == Some("Preferences")).rect();
    let far = h.get_by_label_contains("Show a notice when a document is secured").rect();
    assert!(far.min.y >= ok_before.min.y, "the notice setting starts out of sight: {far:?} vs OK {ok_before:?}");
    h.get_by_label_contains("Show a notice when a document is secured").scroll_to_me();
    h.run_steps(4);
    let far = h.get_by_label_contains("Show a notice when a document is secured").rect();
    assert!(far.min.y >= title.max.y && far.max.y <= ok_before.min.y, "the setting scrolled in between the title and OK: {far:?}");
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
fn a_very_tall_window_does_not_stretch_the_dialog() {
    // Every category opens at one height, capped, so a tall monitor doesn't get a dialog as tall
    // as itself with General's settings at the top and a blank page below them.
    let mut h = harness(2600.0);
    h.run_steps(6);
    let ok = h.get_by_label("OK").rect();
    let title = h.get_by(|n| n.role() == egui::accesskit::Role::Label && n.value().as_deref() == Some("Preferences")).rect();
    assert!(on_screen(ok, 2600.0) && on_screen(title, 2600.0), "{title:?} {ok:?}");
    assert!(ok.max.y - title.min.y < 760.0, "the dialog is {} pt tall", ok.max.y - title.min.y);
}

/// The category tabs, by their accessible role: the page heading has the same name.
fn tab<'a>(h: &'a Harness<'static, PdfCraftApp>, name: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(egui::accesskit::Role::Tab, name)
}

#[test]
fn the_categories_switch_the_settings_and_the_dialog_keeps_its_size() {
    let mut h = harness(900.0);
    let ok = h.get_by_label("OK").rect();
    assert!(h.query_by_label("Tools panel side").is_some(), "General opens first");
    assert!(h.query_by_label("Enable Acrobat JavaScript").is_none(), "other categories' settings are not drawn");
    for (name, setting) in [
        ("Appearance", "Highlight fields"),
        // Reading colours sit in Appearance, below the interface theme (#2627).
        ("Appearance", "White on black"),
        ("Page display", "Continuous scrolling"),
        ("Accessibility", "Animation effects"),
        ("Commenting", "Name on new comments"),
        ("Fill & Sign", "Flatten Fill & Sign when saving"),
        ("JavaScript", "Enable Acrobat JavaScript"),
        ("General", "Tools panel side"),
    ] {
        tab(&h, name).click();
        h.run_steps(3);
        assert!(h.query_all_by_label_contains(setting).next().is_some(), "{name} shows {setting}");
        assert_eq!(h.get_by_label("OK").rect(), ok, "{name}: the dialog kept its size and place");
    }
    // Preferences opens again on the category last shown.
    tab(&h, "Page display").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("app.preferences"));
    h.run_steps(3);
    assert!(h.query_by_label("Continuous scrolling").is_some());
}

#[test]
fn page_display_and_zoom_are_picture_tiles_that_choose_the_defaults() {
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness(900.0);
    h.state_mut().set_option("dialog", "preferences:page-display").unwrap();
    h.run_steps(3);
    let radio = egui::accesskit::Role::RadioButton;
    h.get_by_role_and_label(radio, "Two-page view").click();
    h.run_steps(2);
    assert_eq!(h.state().view_defaults.layout, PageLayout::TwoUp);
    h.get_by_role_and_label(radio, "Zoom to page level").click();
    h.run_steps(2);
    assert_eq!(h.state().view_defaults.fit, Fit::Page);
    // The chosen tile says so to assistive technology.
    let selected = |h: &Harness<'static, PdfCraftApp>, name: &str| {
        h.query_by(|n| n.role() == radio && n.label().as_deref() == Some(name)).is_some()
            && h.query_by(|n| n.role() == radio && n.label().as_deref() == Some(name) && n.toggled() == Some(egui::accesskit::Toggled::True))
                .is_some()
    };
    assert!(selected(&h, "Two-page view") && !selected(&h, "Single page"));
}

#[test]
fn the_control_channel_opens_a_category_and_rejects_an_unknown_one() {
    let mut app = PdfCraftApp::new();
    app.set_option("dialog", "preferences:fill-sign").unwrap();
    assert_eq!(app.preferences_page, pdfcraft_ui_egui::PrefsPage::FillSign);
    assert_eq!(app.dialog, Some(pdfcraft_ui_egui::Dialog::Preferences));
    let err = app.set_option("dialog", "preferences:colours").unwrap_err();
    assert!(err.contains("page-display"), "the error lists the categories: {err}");
    assert_eq!(app.preferences_page, pdfcraft_ui_egui::PrefsPage::FillSign, "a typo leaves the category alone");
}

/// Every category in languages whose words run long (German, Finnish, Hungarian), in Cyrillic and
/// Greek: nothing the dialog draws reaches past its card, so a translation wraps instead of
/// running off the edge.
#[test]
fn long_translations_stay_inside_the_dialog_in_every_category() {
    for language in ["en", "de", "fi", "hu", "pt-br", "uk", "el", "lt"] {
        for page in pdfcraft_ui_egui::PrefsPage::ALL {
            let mut h = Harness::builder().with_size(egui::vec2(1000.0, 900.0)).build_eframe(move |_cc| {
                let mut app = PdfCraftApp::new();
                app.set_option("language", language).unwrap();
                app
            });
            h.run_steps(4);
            // What the app draws behind the dialog, so only the dialog's own nodes are checked.
            let key = |n: &egui_kittest::kittest::AccessKitNode<'_>| format!("{:?} {:?} {:?}", n.role(), n.label(), n.bounding_box());
            let behind = std::cell::RefCell::new(std::collections::HashSet::new());
            let _ = h.query_all_by(|n| behind.borrow_mut().insert(key(n)) && false).count();
            h.state_mut().set_option("dialog", &format!("preferences:{}", page.slug())).unwrap();
            h.run_steps(6);
            let card = h.ctx.data(|d| d.get_temp::<Rect>(egui::Id::unique("dialog").with("card"))).expect("Preferences is shown");
            // The card is measured from what it holds, so a line that doesn't wrap widens it
            // rather than running past it: it must keep the width the dialog sets (860 pt and
            // the padding) and stay in the window.
            assert!(card.width() <= 900.5 && card.max.x <= 1000.0, "{language} {}: the dialog grew to {card:?}", page.slug());
            let behind = behind.into_inner();
            let outside = std::cell::RefCell::new(Vec::new());
            let _ = h
                .query_all_by(|n| {
                    if let Some(r) = n.bounding_box()
                        && !behind.contains(&key(n))
                    {
                        let r = Rect::from_min_max(egui::pos2(r.x0 as f32, r.y0 as f32), egui::pos2(r.x1 as f32, r.y1 as f32));
                        let inside = r.min.x >= card.min.x - 0.5 && r.max.x <= card.max.x + 0.5;
                        if r.width() > 0.0 && !inside && n.role() != egui::accesskit::Role::Window {
                            outside.borrow_mut().push(format!("{:?} {:?} at {r:?}", n.role(), n.label()));
                        }
                    }
                    false
                })
                .count();
            let outside = outside.into_inner();
            assert!(outside.is_empty(), "{language} {}: past the dialog {card:?}: {outside:#?}", page.slug());
        }
    }
}

#[test]
fn in_arabic_the_categories_sit_on_the_right() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "ar").unwrap();
        app.set_option("dialog", "preferences:general").unwrap();
        app
    });
    h.run_steps(6);
    let tabs: Vec<Rect> = h.query_all_by_role(egui::accesskit::Role::Tab).map(|n| n.rect()).collect();
    let tiles: Vec<Rect> = h.query_all_by_role(egui::accesskit::Role::RadioButton).map(|n| n.rect()).collect();
    assert_eq!(tabs.len(), pdfcraft_ui_egui::PrefsPage::ALL.len());
    let list_left = tabs.iter().map(|r| r.min.x).fold(f32::MAX, f32::min);
    assert!(tiles.iter().all(|r| r.max.x < list_left), "the settings are left of the list: {tiles:?} vs {list_left}");
}
