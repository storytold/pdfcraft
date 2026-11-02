//! Edit ▸ Search a folder…, driven through the dialog as a person would.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

use crate::commands::fixture;
use crate::support;

struct TempFolder(PathBuf);

impl TempFolder {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("pdfcraft-search-folder-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(3);
    h.state_mut().execute("edit.search_folder");
    h.run_steps(4);
    h
}

fn fill(h: &mut Harness<'static, PdfCraftApp>, field: &str, text: &str) {
    h.get_by_role_and_label(Role::TextInput, field).click();
    h.run_steps(2);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
}

fn search(h: &mut Harness<'static, PdfCraftApp>, folder: &str, query: &str) {
    fill(h, "Folder", folder);
    fill(h, "Search", query);
    h.get_by_role_and_label(Role::Button, "Search").click();
    h.run_steps(6);
}

fn replace_query(h: &mut Harness<'static, PdfCraftApp>, text: &str) {
    h.get_by_role_and_label(Role::TextInput, "Search").click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(2);
}

fn result_row<'h>(h: &'h Harness<'static, PdfCraftApp>, text: &str) -> egui_kittest::Node<'h> {
    let text = text.to_owned();
    h.get_by(move |n| n.role() == Role::Button && n.label().is_some_and(|l| l.contains(&text)))
}

#[test]
fn search_waits_for_a_folder_and_something_to_look_for() {
    let mut h = harness();
    assert_eq!(h.state().dialog, Some(Dialog::FolderSearch));
    for field in ["Folder", "Search"] {
        assert!(h.get_by_role_and_label(Role::TextInput, field).rect().width() >= 400.0, "{field} has room for a path");
    }
    let disabled = |h: &Harness<'static, PdfCraftApp>| {
        h.query_by(|n| n.role() == Role::Button && n.label().as_deref() == Some("Search") && n.is_disabled()).is_some()
    };
    assert!(disabled(&h));
    fill(&mut h, "Folder", "/somewhere");
    assert!(disabled(&h), "no text yet");
    fill(&mut h, "Search", "   ");
    assert!(disabled(&h), "spaces are nothing to look for");
    fill(&mut h, "Search", "x");
    assert!(!disabled(&h));
}

#[test]
fn a_result_opens_its_file_at_its_page() {
    let folder = TempFolder::new();
    let file = folder.write("a.pdf", &fixture(3));
    let mut h = harness();
    search(&mut h, &folder.path(), "Page 3");
    h.get_by_label_contains("Matches: 1. Files with matches: 1. PDFs read to the end: 1 of 1.");
    h.get_by_label("a.pdf");
    result_row(&h, "Page 3").click();
    support::wait_until(&mut h, "the file to open", |app| app.views.len() == 1);
    assert_eq!(h.state().dialog, None, "the dialog gets out of the way");
    assert_eq!(h.state().views[0].current, 2);
    assert_eq!(h.state().session.get(h.state().views[0].id).and_then(|d| d.path.clone()), Some(file));
    h.state_mut().execute("edit.search_folder");
    h.run_steps(4);
    result_row(&h, "Page 3");
}

#[test]
fn a_result_in_a_file_already_open_goes_to_its_tab() {
    let folder = TempFolder::new();
    let file = folder.write("a.pdf", &fixture(3));
    let mut h = harness();
    h.state_mut().open_bytes("other.pdf", None, fixture(1)).unwrap();
    h.state_mut().open_path(&file);
    h.state_mut().open_bytes("third.pdf", None, fixture(1)).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 3);
    h.state_mut().execute("edit.search_folder");
    h.run_steps(4);
    search(&mut h, &folder.path(), "Page 2");
    result_row(&h, "Page 2").click();
    h.run_steps(6);
    assert_eq!(h.state().views.len(), 3, "no second tab");
    assert_eq!(h.state().active, Some(1));
    assert_eq!(h.state().views[1].current, 1);
}

#[test]
fn the_options_reach_the_search() {
    let folder = TempFolder::new();
    folder.write("a.pdf", &fixture(1));
    std::fs::create_dir(folder.0.join("sub")).unwrap();
    folder.write("sub/b.pdf", &fixture(1));
    let mut h = harness();
    search(&mut h, &folder.path(), "page 1");
    h.get_by_label_contains("Matches: 2.");
    h.get_by_role_and_label(Role::CheckBox, "Case-sensitive").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, "Search").click();
    h.run_steps(6);
    h.get_by_label_contains("Matches: 0.");
    h.get_by_role_and_label(Role::CheckBox, "Case-sensitive").click();
    h.get_by_role_and_label(Role::CheckBox, "Include subfolders").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, "Search").click();
    h.run_steps(6);
    h.get_by_label_contains("Matches: 1.");
    replace_query(&mut h, "pag");
    h.get_by_role_and_label(Role::Button, "Search").click();
    h.run_steps(6);
    h.get_by_label_contains("Matches: 1.");
    h.get_by_role_and_label(Role::CheckBox, "Whole words only").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, "Search").click();
    h.run_steps(6);
    h.get_by_label_contains("Matches: 0.");
}

#[test]
fn enter_in_the_search_field_starts_the_search() {
    let folder = TempFolder::new();
    folder.write("a.pdf", &fixture(1));
    let mut h = harness();
    fill(&mut h, "Folder", &folder.path());
    fill(&mut h, "Search", "Page 1");
    h.key_press(egui::Key::Enter);
    h.run_steps(6);
    h.get_by_label_contains("Matches: 1.");
}

#[test]
fn what_was_not_searched_is_listed_with_why() {
    let folder = TempFolder::new();
    folder.write("a.pdf", &fixture(1));
    let broken = folder.write("broken.pdf", b"not a PDF");
    let mut h = harness();
    search(&mut h, &folder.path(), "Page 1");
    h.get_by_label_contains("PDFs read to the end: 1 of 2.");
    h.get_by_label("Not searched: 1").click();
    h.run_steps(8);
    h.get_by_label(&broken);
    h.get_by_label_contains("Couldn't be read:");
}

#[test]
fn a_folder_that_is_not_there_is_said_and_nothing_else_happens() {
    let folder = TempFolder::new();
    let gone = format!("{}/gone", folder.path());
    let mut h = harness();
    search(&mut h, &gone, "needle");
    h.get_by_label_contains(&gone);
    assert_eq!(h.state().dialog, Some(Dialog::FolderSearch));
    assert!(h.state().views.is_empty());
}

#[test]
fn a_new_search_starts_clean() {
    let folder = TempFolder::new();
    folder.write("a.pdf", &fixture(1));
    let mut h = harness();
    search(&mut h, &folder.path(), "Page 1");
    h.get_by_label("a.pdf");
    replace_query(&mut h, "nothing like this");
    h.get_by_role_and_label(Role::Button, "Search").click();
    h.run_steps(6);
    h.get_by_label_contains("Matches: 0.");
    assert!(h.query_by_label("a.pdf").is_none(), "the last search's results are gone");
}

#[test]
fn browse_fills_in_the_folder_it_chose() {
    let folder = TempFolder::new();
    folder.write("a.pdf", &fixture(1));
    let mut h = harness();
    h.state_mut().pick_override = Some(vec![folder.path()]);
    h.get_by_role_and_label(Role::Button, "Browse…").click();
    h.run_steps(6);
    fill(&mut h, "Search", "Page 1");
    h.get_by_role_and_label(Role::Button, "Search").click();
    h.run_steps(6);
    h.get_by_label_contains("Matches: 1.");
}

#[test]
fn the_results_stay_on_screen_in_the_smallest_window() {
    let folder = TempFolder::new();
    folder.write("a.pdf", &fixture(30));
    let mut h = Harness::builder().with_size(egui::vec2(820.0, 520.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(3);
    h.state_mut().execute("edit.search_folder");
    h.run_steps(4);
    search(&mut h, &folder.path(), "Page");
    h.run_steps(6);
    let dialog = h.ctx.memory(|m| m.area_rect(egui::Id::unique("dialog"))).unwrap();
    assert!(dialog.min.y >= 0.0 && dialog.max.y <= 520.0, "all of the dialog is in the window: {dialog:?}");
    let first = result_row(&h, "Page 1…").rect();
    assert!(first.max.y <= 520.0, "the first result shows: {first:?}");
}

#[test]
fn close_ends_the_dialog() {
    let mut h = harness();
    // The window's own close button comes first.
    h.query_all_by_role_and_label(Role::Button, "Close").last().unwrap().click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
}

#[test]
fn the_edit_menu_offers_it_with_no_document_open() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    support::open_menu(&mut h, "Edit");
    support::menu_row(&h, "Search a folder…").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, Some(Dialog::FolderSearch));
}
