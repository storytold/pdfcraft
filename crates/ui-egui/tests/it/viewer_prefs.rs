//! Viewer preferences applied when a document opens (ISO 32000-2 §12.2, Table 147): hiding the
//! toolbars, menu and window controls in its tab, with F8 to bring them back; fitting and
//! centring the window; and the zoom a one-page-at-a-time layout opens at.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::canvas::Fit;
use pdfcraft_ui_egui::viewer_prefs::HiddenInterface;

/// Two 200 × 300 pages; `catalog` is added to the catalog dictionary.
fn fixture(catalog: &str) -> Vec<u8> {
    let objs = [
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

const HIDE_ALL: &str = "/ViewerPreferences << /HideToolbar true /HideMenubar true /HideWindowUI true >>";

/// A plain document in the first tab and `files` after it, the last one active.
fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static, files: &'static [&'static str]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        for (i, catalog) in files.iter().enumerate() {
            app.open_bytes(&format!("{i}.pdf"), None, fixture(catalog)).unwrap();
        }
        app
    });
    settle(&mut h);
    h
}

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..3000 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

/// Which of the toolbar (the command search), the menu (the Menu button) and the window controls
/// (the rail's fit button) are on screen.
fn shown(h: &Harness<'static, PdfCraftApp>) -> (bool, bool, bool) {
    (
        h.query_by_label("Find tools and commands").is_some(),
        h.query_by_label("Menu").is_some(),
        h.query_by_label("Toggle fit page / fit width").is_some(),
    )
}

#[test]
fn a_document_that_hides_the_interface_opens_without_it_and_f8_brings_it_back() {
    let mut h = harness(|_| {}, &["", HIDE_ALL]);
    assert_eq!(shown(&h), (false, false, false), "the toolbar, menu and rail are hidden");
    assert_eq!(h.state().hidden_interface(), HiddenInterface::ALL);
    // The quick-action bar is a toolbar too.
    assert!(h.query_by_label_contains("Hand (H)").is_none());
    // The tab strip stays: the tabs and Home are still there.
    h.get_by_label("Home");

    h.key_press(egui::Key::F8);
    h.run_steps(3);
    assert_eq!(shown(&h), (true, true, true), "F8 shows them");
    h.get_by_label_contains("Hand (H)");
    h.key_press(egui::Key::F8);
    h.run_steps(3);
    assert_eq!(shown(&h), (false, false, false), "and hides them again");

    // Another tab, whose document asks for nothing, has the usual interface.
    h.state_mut().active = Some(0);
    h.run_steps(3);
    assert_eq!(shown(&h), (true, true, true));
    h.state_mut().active = Some(1);
    h.run_steps(3);
    assert_eq!(shown(&h), (false, false, false));

    // The command (palette, menus, automation) does what F8 does.
    assert!(h.state_mut().execute("view.toggle_interface"));
    h.run_steps(3);
    assert_eq!(shown(&h), (true, true, true));
}

#[test]
fn each_preference_hides_its_own_part() {
    let top = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].viewport_rect().top();
    let mut h = harness(|_| {}, &["/ViewerPreferences << /HideToolbar true >>"]);
    assert_eq!(shown(&h), (false, true, true), "HideToolbar");
    // Shown and hidden again, F8 hides only what the document asked for.
    h.key_press(egui::Key::F8);
    h.run_steps(3);
    assert_eq!(shown(&h), (true, true, true));
    h.key_press(egui::Key::F8);
    h.run_steps(3);
    assert_eq!(shown(&h), (false, true, true));
    let h = harness(|_| {}, &["/ViewerPreferences << /HideMenubar true >>"]);
    assert_eq!(shown(&h), (true, false, true), "HideMenubar");
    let h = harness(|_| {}, &["/ViewerPreferences << /HideWindowUI true >>"]);
    assert_eq!(shown(&h), (true, true, false), "HideWindowUI");
    assert!(h.state().hidden_interface().window_ui);
    // False is the default, and is nothing to hide.
    let h = harness(|_| {}, &["/ViewerPreferences << /HideToolbar false /HideMenubar false /HideWindowUI false >>"]);
    assert_eq!(shown(&h), (true, true, true));
    let with_bar = top(&h);
    // With both the tools and the menu hidden, the mode bar goes and the document moves up.
    let h = harness(|_| {}, &["/ViewerPreferences << /HideToolbar true /HideMenubar true >>"]);
    assert_eq!(shown(&h), (false, false, true));
    assert!(top(&h) < with_bar - 40.0, "{} against {with_bar}", top(&h));
}

#[test]
fn hiding_the_window_controls_leaves_the_document_the_whole_width() {
    // The document also asks for the page thumbnails panel, which is hidden with the rest.
    let mut h = harness(|_| {}, &["/PageMode /UseThumbs /ViewerPreferences << /HideWindowUI true >>"]);
    let width = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].viewport_rect().width();
    assert!(width(&h) > 1390.0, "{}", width(&h));
    h.key_press(egui::Key::F8);
    settle(&mut h);
    // The rail and the Pages panel are back beside the document.
    assert!(width(&h) < 1400.0 - 48.0 - 200.0, "{}", width(&h));
}

#[test]
fn the_preference_to_ignore_document_views_keeps_the_interface() {
    let h = harness(|app| app.always_default_view = true, &[HIDE_ALL]);
    assert_eq!(shown(&h), (true, true, true));
    assert_eq!(h.state().hidden_interface(), HiddenInterface::NONE);
}

#[test]
fn f8_hides_the_interface_of_any_document_and_shows_it_again() {
    let mut h = harness(|_| {}, &[""]);
    assert_eq!(shown(&h), (true, true, true));
    h.key_press(egui::Key::F8);
    h.run_steps(3);
    assert_eq!(shown(&h), (false, false, false));
    h.get_by_label_contains("Press F8 to show them");
    h.key_press(egui::Key::F8);
    h.run_steps(3);
    assert_eq!(shown(&h), (true, true, true));
}

#[test]
fn hiding_the_interface_is_never_saved() {
    let h = harness(|_| {}, &[HIDE_ALL]);
    let settings = h.state().persist();
    assert!(!settings.contains("interface"), "{settings}");
}

/// The fit a document with `catalog` opens at, with the user's defaults (fit width).
fn opening_fit(catalog: &str, own_view: bool) -> Fit {
    let mut app = PdfCraftApp::new();
    app.always_default_view = own_view;
    app.open_bytes("x.pdf", None, fixture(catalog)).unwrap();
    app.views[0].fit
}

#[test]
fn one_page_at_a_time_opens_at_fit_page() {
    let default = opening_fit("", false);
    assert_eq!(default, Fit::Width, "the default zoom");
    for layout in ["SinglePage", "TwoPageLeft", "TwoPageRight"] {
        assert_eq!(opening_fit(&format!("/PageLayout /{layout}"), false), Fit::Page, "{layout}");
        // The user's preference keeps their zoom.
        assert_eq!(opening_fit(&format!("/PageLayout /{layout}"), true), default, "{layout}");
    }
    // Scrolling layouts keep the default zoom.
    for layout in ["OneColumn", "TwoColumnLeft", "TwoColumnRight"] {
        assert_eq!(opening_fit(&format!("/PageLayout /{layout}"), false), default, "{layout}");
    }
    // A zoom the document names wins.
    assert_eq!(opening_fit("/PageLayout /SinglePage /OpenAction [3 0 R /FitH 300]", false), Fit::Width);
    let mut app = PdfCraftApp::new();
    app.open_bytes("x.pdf", None, fixture("/PageLayout /SinglePage /OpenAction [3 0 R /XYZ 0 300 2]")).unwrap();
    assert_eq!((app.views[0].fit, app.views[0].zoom), (Fit::None, 2.0));
}

/// The last `InnerSize` and `OuterPosition` asked for over `frames` frames.
fn window_commands(h: &mut Harness<'static, PdfCraftApp>, frames: usize) -> (Option<egui::Vec2>, Option<egui::Pos2>) {
    let (mut size, mut at) = (None, None);
    for _ in 0..frames {
        h.run_steps(1);
        for c in &h.output().viewport_output[&egui::ViewportId::ROOT].commands {
            match c {
                egui::ViewportCommand::InnerSize(s) => size = Some(*s),
                egui::ViewportCommand::OuterPosition(p) => at = Some(*p),
                _ => {}
            }
        }
    }
    (size, at)
}

fn on_monitor(h: &mut Harness<'static, PdfCraftApp>) {
    let vp = h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default();
    vp.monitor_size = Some(egui::vec2(1920.0, 1080.0));
    vp.inner_rect = Some(egui::Rect::from_min_size(egui::pos2(50.0, 70.0), egui::vec2(1400.0, 900.0)));
    vp.outer_rect = Some(egui::Rect::from_min_size(egui::pos2(50.0, 40.0), egui::vec2(1400.0, 930.0)));
}

/// A harness whose window is on a 1920 × 1080 monitor, with one document opened in the first
/// frames.
fn windowed(files: &'static [&'static str]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    on_monitor(&mut h);
    h.run_steps(2);
    for (i, catalog) in files.iter().enumerate() {
        h.state_mut().open_bytes(&format!("{i}.pdf"), None, fixture(catalog)).unwrap();
    }
    h
}

#[test]
fn fit_window_sizes_the_window_to_the_first_page() {
    let mut h = windowed(&["/ViewerPreferences << /FitWindow true >> /OpenAction [3 0 R /Fit]"]);
    let (size, at) = window_commands(&mut h, 12);
    let size = size.expect("the window is resized");
    assert_eq!(at, None, "not moved");
    // A 2:3 portrait page fitted to the window's height makes a narrower window.
    assert!(size.x < 1000.0 && (size.y - 900.0).abs() < 1.0, "{size:?}");
    // Asked once.
    assert_eq!(window_commands(&mut h, 6), (None, None));
    // The page now fills the document area's height and nearly its width.
    let v = &h.state().views[0];
    let (page, area) = (v.page_screen_rect(0).expect("shown"), v.viewport_rect());
    assert!(area.width() - page.width() < 160.0, "page {page:?} in {area:?}");
}

#[test]
fn center_window_centres_the_window_on_the_monitor() {
    let mut h = windowed(&["/ViewerPreferences << /CenterWindow true >>"]);
    // 1400 × 930 on 1920 × 1080.
    assert_eq!(window_commands(&mut h, 12), (None, Some(egui::pos2(260.0, 75.0))));
    let mut h = windowed(&["/ViewerPreferences << /FitWindow true /CenterWindow true >>"]);
    let (size, at) = window_commands(&mut h, 12);
    let (size, at) = (size.expect("resized"), at.expect("moved"));
    assert!(((1920.0 - size.x) / 2.0 - at.x).abs() <= 0.5, "{size:?} at {at:?}");
}

#[test]
fn a_maximized_window_or_a_second_tab_is_left_alone() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| PdfCraftApp::new());
    on_monitor(&mut h);
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().maximized = Some(true);
    h.run_steps(2);
    h.state_mut().open_bytes("0.pdf", None, fixture("/ViewerPreferences << /FitWindow true /CenterWindow true >>")).unwrap();
    assert_eq!(window_commands(&mut h, 12), (None, None), "maximized");

    let mut h = windowed(&["", "/ViewerPreferences << /FitWindow true /CenterWindow true >>"]);
    assert_eq!(window_commands(&mut h, 12), (None, None), "beside another document");
}

#[test]
fn the_user_is_told_which_key_brings_the_interface_back() {
    let mut h = windowed(&[HIDE_ALL]);
    h.run_steps(3);
    h.get_by_label_contains("This document hides the toolbars and panels. Press F8 to show them.");
    // Not when nothing is hidden.
    let mut h = windowed(&["/ViewerPreferences << /HideToolbar false /HideMenubar false /HideWindowUI false >>"]);
    h.run_steps(3);
    assert!(h.query_by_label_contains("Press F8").is_none());
    // Nor when the user opens documents their own way.
    let mut h = windowed(&[]);
    h.state_mut().always_default_view = true;
    h.state_mut().open_bytes("0.pdf", None, fixture(HIDE_ALL)).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Press F8").is_none());
}
