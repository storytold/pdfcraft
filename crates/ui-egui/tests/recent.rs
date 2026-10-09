//! Clearing the recent-files list (#430): all of it from Home or File ▸ Open Recent, or one entry
//! from its context menu on Home. Pinned folders are a separate list and stay.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, RecentFile};

fn recent(name: &str) -> RecentFile {
    RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 }
}

/// Home (no document open) listing `a.pdf`, `b.pdf` and `c.pdf`, with one pinned folder.
fn harness(folder: &str) -> Harness<'static, PdfCraftApp> {
    let folder = folder.to_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1400.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.pin_folder(&folder);
        app.recent = vec![recent("a.pdf"), recent("b.pdf"), recent("c.pdf")];
        app
    });
    h.run_steps(4);
    assert_eq!(h.state().pinned.folders.len(), 1, "the folder is pinned");
    h
}

fn temp_folder(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("pdfcraft-recent-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

fn names(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    h.state().recent.iter().map(|r| r.name.clone()).collect()
}

fn persisted(h: &Harness<'static, PdfCraftApp>) -> serde_json::Value {
    serde_json::from_str(&h.state().persist()).unwrap()
}

#[test]
fn clear_on_home_empties_the_list_and_keeps_pinned_folders() {
    let folder = temp_folder("clear");
    let mut h = harness(&folder);
    let pinned = h.state().pinned.folders.clone();
    h.get_by_label("Clear recent files").click();
    h.run_steps(3);
    assert!(h.state().recent.is_empty());
    let saved = persisted(&h);
    assert_eq!(saved["recent"], serde_json::json!([]), "the saved settings keep no recent files");
    assert_eq!(saved["pinned_folders"], serde_json::json!(pinned), "pinned folders are not recent files");
    assert!(h.query_by_label("a.pdf").is_none());
    assert!(h.query_by_label("Clear recent files").is_none(), "nothing left to clear");
    h.get_by_label_contains("Files you open in PdfCraft appear here");
}

#[test]
fn remove_from_recent_removes_only_that_row() {
    let folder = temp_folder("remove");
    let mut h = harness(&folder);
    let pinned = h.state().pinned.folders.clone();
    let at = h.get_by_label("b.pdf").rect().center();
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() });
    }
    h.run_steps(2);
    h.get_by_label("Remove from recent").click();
    h.run_steps(3);
    assert_eq!(names(&h), ["a.pdf", "c.pdf"]);
    assert_eq!(h.state().views.len(), 0, "removing an entry does not open it");
    let saved = persisted(&h);
    let paths: Vec<&str> = saved["recent"].as_array().unwrap().iter().filter_map(|r| r["path"].as_str()).collect();
    assert_eq!(paths, ["/nowhere/a.pdf", "/nowhere/c.pdf"]);
    assert_eq!(saved["pinned_folders"], serde_json::json!(pinned));
}

#[test]
fn open_recent_submenu_clears_the_shared_list() {
    let folder = temp_folder("menu");
    let mut h = harness(&folder);
    let pinned = h.state().pinned.folders.clone();
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    let submenu = h.get_by_label("Open Recent ⏵").rect();
    h.get_by_label("Open Recent ⏵").hover();
    h.run_steps(3);
    // Home has its own Clear button; take the submenu's, to the right of Open Recent.
    h.get_all_by_label("Clear recent files").find(|n| n.rect().left() >= submenu.right()).expect("Clear in the Open Recent submenu").click();
    h.run_steps(3);
    assert!(h.state().recent.is_empty());
    assert_eq!(h.state().pinned.folders, pinned);
}

#[test]
fn the_clear_command_runs_headlessly_and_tolerates_an_empty_list() {
    let folder = temp_folder("command");
    let mut h = harness(&folder);
    let pinned = h.state().pinned.folders.clone();
    assert!(h.state_mut().execute("file.clear_recent"));
    assert!(h.state().recent.is_empty());
    // Again, with nothing left: a note, not a failure.
    assert!(h.state_mut().execute("file.clear_recent"));
    h.run_steps(2);
    assert!(h.state().recent.is_empty());
    assert_eq!(h.state().pinned.folders, pinned);
}
