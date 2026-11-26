//! Preferences ▸ Recent files to keep (#5572): how many files the recent list holds, from none
//! up to 50, on every path that adds to it, shows it or saves it.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
// PLAN: assumed `pdfcraft_ui_egui::settings::import_json(&str) -> (PdfCraftSettings, Vec<SectionError>)` (pdf-shell-w9)
use pdfcraft_ui_egui::settings::import_json;
use pdfcraft_ui_egui::{PdfCraftApp, RecentFile};

use crate::support;
use crate::support::AppHarness as _;

/// Settings as a settings file gives them, read the way the app reads them.
fn restore(app: &mut PdfCraftApp, json: &str) {
    app.apply_settings(&import_json(json).0);
}

/// The recent list the app would write to its settings.
fn saved_recent(app: &PdfCraftApp) -> Vec<RecentFile> {
    app.settings_snapshot().recent.recent.as_deref().map(|text| serde_json::from_str(text).expect("the saved list reads back")).unwrap_or_default()
}

const PDF: &[u8] = include_bytes!("../data/form.pdf");

fn recent_file(name: &str) -> RecentFile {
    RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 }
}

fn names(app: &PdfCraftApp) -> Vec<&str> {
    app.recent.iter().map(|r| r.name.as_str()).collect()
}

fn app_with(files: &[&str]) -> PdfCraftApp {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app.recent.extend(files.iter().map(|n| recent_file(n)));
    app
}

/// Tall enough that the whole of Home is on screen without scrolling.
fn harness(limit: usize, files: &[&str]) -> Harness<'static, PdfCraftApp> {
    let files: Vec<String> = files.iter().map(ToString::to_string).collect();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1600.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_with(&files.iter().map(String::as_str).collect::<Vec<_>>());
            app.recent_limit = limit;
            app
        }
    });
    h.run_steps(4);
    h
}

/// Open a document with a file behind it, as a file on disk is recorded.
fn open(app: &mut PdfCraftApp, name: &str) {
    app.open_bytes(&egui::Context::default(), name, Some(format!("/nowhere/{name}")), PDF.to_vec()).unwrap();
}

#[test]
fn opening_a_file_keeps_the_newest_up_to_the_limit() {
    let mut app = app_with(&[]);
    app.recent_limit = 2;
    for name in ["first.pdf", "second.pdf", "third.pdf"] {
        open(&mut app, name);
    }
    assert_eq!(names(&app), ["third.pdf", "second.pdf"]);
    open(&mut app, "second.pdf");
    assert_eq!(names(&app), ["second.pdf", "third.pdf"], "a file opened again moves up and loses nothing");
}

#[test]
fn at_zero_opening_a_file_records_nothing() {
    let mut app = app_with(&[]);
    app.recent_limit = 0;
    open(&mut app, "first.pdf");
    assert!(app.recent.is_empty());
    assert!(saved_recent(&app).is_empty(), "and the saved list is empty");
}

#[test]
fn at_zero_home_has_no_recent_section() {
    let h = harness(0, &[]);
    assert!(h.query_by_label("Recent").is_none(), "no heading");
    assert!(h.query_by_label_contains("Files you open in PdfCraft appear here").is_none(), "and no promise of a list");
    h.get_by_label_contains("PdfCraft works offline");
}

#[test]
fn above_zero_an_empty_list_still_says_what_it_is_for() {
    let h = harness(1, &[]);
    h.get_by_role_and_label(egui::accesskit::Role::Heading, "Recent");
    h.get_by_label_contains("Files you open in PdfCraft appear here");
}

#[test]
fn at_zero_the_menu_and_the_palette_have_nothing_to_open() {
    let mut h = harness(0, &[]);
    support::open_menu(&mut h, "File");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Open Recent") && n.is_disabled()).is_some(), "Open Recent is disabled");
    assert!(h.execute("file.open_recent"));
    h.step();
    assert!(support::shown_notice(&h).is_some_and(|m| m.contains("No recent files")));
    assert!(h.state().views.is_empty(), "and nothing opened");
}

#[test]
fn the_longest_list_scrolls_in_the_smallest_window_down_to_clear() {
    let files: Vec<String> = (0..50).map(|n| format!("{n}.pdf")).collect();
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 520.0)).build_eframe({
        let files = files.clone();
        move |cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            let mut app = app_with(&files.iter().map(String::as_str).collect::<Vec<_>>());
            app.recent_limit = 50;
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, PDF.to_vec()).unwrap();
            app
        }
    });
    h.run_steps(4);
    support::open_menu(&mut h, "File");
    support::menu_row(&h, "Open Recent").hover();
    h.run_steps(3);
    let foot = |h: &Harness<'static, PdfCraftApp>| support::menu_row(h, "Clear Recent Files").rect().max.y;
    assert!(foot(&h) > 520.0, "fifty rows run past the smallest window");
    support::menu_row(&h, "1.pdf").hover();
    h.run_steps(2);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -5000.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(8);
    assert!(foot(&h) <= 520.0, "the wheel brings the foot of the menu into view");
    support::menu_row(&h, "Clear Recent Files").click();
    h.run_steps(3);
    assert!(h.state().recent.is_empty());
}

#[test]
fn settings_saved_while_preferences_is_open_already_leave_the_rest_out() {
    let mut app = app_with(&["first.pdf", "second.pdf"]);
    app.recent_limit = 1;
    let saved: Vec<String> = saved_recent(&app).iter().map(|r| r.name.clone()).collect();
    assert_eq!(saved, ["first.pdf"]);
}

#[test]
fn the_limit_is_saved_and_restored_and_cuts_a_longer_list() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-recent-limit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<String> = ["a.pdf", "b.pdf", "c.pdf"]
        .iter()
        .map(|n| {
            let path = dir.join(n);
            std::fs::write(&path, PDF).unwrap();
            path.to_string_lossy().into_owned()
        })
        .collect();
    let entries: Vec<RecentFile> = paths.iter().map(|p| RecentFile { name: "x.pdf".into(), path: p.clone(), pages: 1, size: 0 }).collect();
    let json = serde_json::json!({ "recent": entries, "recent_limit": 2 }).to_string();
    let mut restored = crate::control::new_app();
    restore(&mut restored, &json);
    assert_eq!(restored.recent_limit, 2);
    let kept: Vec<&str> = restored.recent.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(kept, [paths[0].as_str(), paths[1].as_str()]);
    let mut again = crate::control::new_app();
    again.apply_settings(&restored.settings_snapshot());
    assert_eq!(again.recent_limit, 2);
    assert_eq!(again.recent.len(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// #5682: restoring settings never asks the file system about recent files, which can block on a
/// mount that has stopped answering. A gone file leaves the list once frames run, and a file that
/// is there stays.
#[test]
fn restoring_settings_does_not_check_recent_files_and_a_frame_drops_the_gone_ones() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-recent-restore-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let here = dir.join("here.pdf");
    std::fs::write(&here, PDF).unwrap();
    let here = here.to_string_lossy().into_owned();
    let gone = dir.join("gone.pdf").to_string_lossy().into_owned();
    let entries: Vec<RecentFile> =
        [&here, &gone].iter().map(|p| RecentFile { name: "x.pdf".into(), path: (*p).clone(), pages: 1, size: 0 }).collect();
    let json = serde_json::json!({ "recent": entries }).to_string();

    let mut restored = crate::control::new_app();
    restore(&mut restored, &json);
    let paths: Vec<&str> = restored.recent.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, [here.as_str(), gone.as_str()], "restoring keeps every entry without asking whether its file exists");

    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1600.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = support::inline_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            restore(&mut app, &json);
            app
        }
    });
    h.run_steps(2);
    let paths: Vec<&str> = h.state().recent.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, [here.as_str()], "the gone file leaves once the window is up");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_settings_file_can_not_ask_for_a_limit_that_is_not_one() {
    for (saved, want) in [
        ("{}", 12),
        ("{\"recent_limit\": null}", 12),
        ("{\"recent_limit\": \"3\"}", 12),
        ("{\"recent_limit\": -1}", 12),
        ("{\"recent_limit\": 2.5}", 12),
        ("{\"recent_limit\": 0}", 0),
        ("{\"recent_limit\": 50}", 50),
        ("{\"recent_limit\": 51}", 50),
        ("{\"recent_limit\": 18446744073709551615}", 50),
    ] {
        let mut app = crate::control::new_app();
        restore(&mut app, saved);
        assert_eq!(app.recent_limit, want, "{saved}");
    }
}

#[test]
fn the_control_channel_sets_the_limit_and_refuses_what_it_can_not_be() {
    let mut app = app_with(&["first.pdf", "second.pdf"]);
    for bad in ["51", "-1", "x", "", "1.5"] {
        let error = app.set_option(&egui::Context::default(), "recent-files", bad).unwrap_err();
        assert!(error.contains("between 0 and 50"), "{bad}: {error}");
    }
    assert_eq!((app.recent_limit, app.recent.len()), (12, 2), "a refused value changes nothing");
    app.set_option(&egui::Context::default(), "recent-files", "50").unwrap();
    assert_eq!((app.recent_limit, app.recent.len()), (50, 2));
    app.set_option(&egui::Context::default(), "recent-files", "1").unwrap();
    assert_eq!(names(&app), ["first.pdf"], "cut at once, with no dialog to wait for");
    app.set_option(&egui::Context::default(), "recent-files", "0").unwrap();
    assert!(app.recent.is_empty());
}
