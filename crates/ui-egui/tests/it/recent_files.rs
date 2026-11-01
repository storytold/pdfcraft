//! Preferences ▸ Recent files to keep (#5572): how many files the recent list holds, from none
//! up to 50, on every path that adds to it, shows it or saves it.

use egui::Key;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, RECENT_FILES_DEFAULT, RecentFile};

use crate::support;

const PDF: &[u8] = include_bytes!("../data/form.pdf");

fn recent_file(name: &str) -> RecentFile {
    RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 }
}

fn names(app: &PdfCraftApp) -> Vec<&str> {
    app.recent.iter().map(|r| r.name.as_str()).collect()
}

fn app_with(files: &[&str]) -> PdfCraftApp {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.recent.extend(files.iter().map(|n| recent_file(n)));
    app
}

/// Tall enough that the whole of Home is on screen without scrolling.
fn harness(limit: usize, files: &[&str]) -> Harness<'static, PdfCraftApp> {
    let files: Vec<String> = files.iter().map(ToString::to_string).collect();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1600.0)).build_eframe(move |_cc| {
        let mut app = app_with(&files.iter().map(String::as_str).collect::<Vec<_>>());
        app.recent_limit = limit;
        app
    });
    h.run_steps(4);
    h
}

/// Open a document with a file behind it, as a file on disk is recorded.
fn open(app: &mut PdfCraftApp, name: &str) {
    app.open_bytes(name, Some(format!("/nowhere/{name}")), PDF.to_vec()).unwrap();
}

/// Typed the way a person does: click the field, type, Enter.
fn type_limit(h: &mut Harness<'static, PdfCraftApp>, value: &str) {
    h.get_by_role_and_label(Role::SpinButton, "Recent files to keep").click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text(value.into()));
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
}

#[test]
fn the_list_keeps_twelve_files_until_preferences_says_otherwise() {
    let mut app = app_with(&[]);
    assert_eq!(app.recent_limit, 12);
    for n in 0..=RECENT_FILES_DEFAULT {
        open(&mut app, &format!("{n}.pdf"));
    }
    assert_eq!(app.recent.len(), 12);
    assert_eq!(app.recent[0].name, "12.pdf", "newest first");
    assert_eq!(app.recent[11].name, "1.pdf", "the oldest of the thirteen went");
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
    let saved: serde_json::Value = serde_json::from_str(&app.persist()).unwrap();
    assert_eq!(saved["recent"], serde_json::json!([]), "and the saved list is empty");
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
    h.get_by_label("Recent");
    h.get_by_label_contains("Files you open in PdfCraft appear here");
}

#[test]
fn at_zero_the_menu_and_the_palette_have_nothing_to_open() {
    let mut h = harness(0, &[]);
    support::open_menu(&mut h, "File");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Open Recent") && n.is_disabled()).is_some(), "Open Recent is disabled");
    assert!(h.state_mut().execute("file.open_recent"));
    assert!(h.state().toast.as_ref().is_some_and(|(m, _)| m.contains("No recent files")));
    assert!(h.state().views.is_empty(), "and nothing opened");
}

#[test]
fn the_longest_list_scrolls_in_the_smallest_window_down_to_clear() {
    let files: Vec<String> = (0..50).map(|n| format!("{n}.pdf")).collect();
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 520.0)).build_eframe({
        let files = files.clone();
        move |_cc| {
            let mut app = app_with(&files.iter().map(String::as_str).collect::<Vec<_>>());
            app.recent_limit = 50;
            app.open_bytes("doc.pdf", None, PDF.to_vec()).unwrap();
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
fn preferences_cuts_the_list_when_it_closes_and_not_before() {
    let mut h = harness(12, &["first.pdf", "second.pdf", "third.pdf"]);
    h.state_mut().dialog = Some(Dialog::Preferences);
    h.run_steps(4);
    type_limit(&mut h, "0");
    assert_eq!(h.state().recent_limit, 0);
    assert_eq!(h.state().recent.len(), 3, "0 on the way to another number forgets nothing");
    assert!(h.query_by_label("first.pdf").is_none(), "though Home already leaves them out");
    type_limit(&mut h, "2");
    assert_eq!(h.state().recent.len(), 3);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(names(h.state()), ["first.pdf", "second.pdf"], "the newest two stay");
}

#[test]
fn escape_closes_preferences_with_the_same_result_as_ok() {
    let mut h = harness(12, &["first.pdf", "second.pdf"]);
    h.state_mut().dialog = Some(Dialog::Preferences);
    h.run_steps(4);
    type_limit(&mut h, "1");
    h.key_press(Key::Escape);
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(names(h.state()), ["first.pdf"]);
}

#[test]
fn the_field_does_not_go_past_fifty() {
    let mut h = harness(12, &[]);
    h.state_mut().dialog = Some(Dialog::Preferences);
    h.run_steps(4);
    type_limit(&mut h, "999");
    assert_eq!(h.state().recent_limit, 50);
}

#[test]
fn settings_saved_while_preferences_is_open_already_leave_the_rest_out() {
    let mut app = app_with(&["first.pdf", "second.pdf"]);
    app.recent_limit = 1;
    let saved = app.persist();
    assert!(saved.contains("first.pdf"));
    assert!(!saved.contains("second.pdf"));
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
    let mut restored = PdfCraftApp::new();
    restored.restore(&json);
    assert_eq!(restored.recent_limit, 2);
    let kept: Vec<&str> = restored.recent.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(kept, [paths[0].as_str(), paths[1].as_str()]);
    let mut again = PdfCraftApp::new();
    again.restore(&restored.persist());
    assert_eq!(again.recent_limit, 2);
    assert_eq!(again.recent.len(), 2);
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
        let mut app = PdfCraftApp::new();
        app.restore(saved);
        assert_eq!(app.recent_limit, want, "{saved}");
    }
}

#[test]
fn the_control_channel_sets_the_limit_and_refuses_what_it_can_not_be() {
    let mut app = app_with(&["first.pdf", "second.pdf"]);
    for bad in ["51", "-1", "x", "", "1.5"] {
        let error = app.set_option("recent-files", bad).unwrap_err();
        assert!(error.contains("between 0 and 50"), "{bad}: {error}");
    }
    assert_eq!((app.recent_limit, app.recent.len()), (12, 2), "a refused value changes nothing");
    app.set_option("recent-files", "50").unwrap();
    assert_eq!((app.recent_limit, app.recent.len()), (50, 2));
    app.set_option("recent-files", "1").unwrap();
    assert_eq!(names(&app), ["first.pdf"], "cut at once, with no dialog to wait for");
    app.set_option("recent-files", "0").unwrap();
    assert!(app.recent.is_empty());
}
