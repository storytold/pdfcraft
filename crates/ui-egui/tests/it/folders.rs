//! Pinned folders on Home: File ▸ Pin folder to Home…, the newest PDFs listed per folder,
//! opening one, unpinning, and the setting surviving a restart.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::support::AppHarness as _;
use craft::host::{ManualJobs, Picked, ScriptedFiles};
use craft::ui::shell::PINNED_FILES_SHOWN;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::folders_ui::{MAX_PINNED, folder_name, location};

/// A one-page PDF.
const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A fresh, empty folder under the temp dir, removed when dropped.
struct TempFolder(PathBuf);

impl TempFolder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-pinned-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    /// Write `name` with `bytes`, last modified `age` ago.
    fn file(&self, name: &str, bytes: &[u8], age: Duration) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
        path
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app on a host whose jobs wait in `jobs` until a test runs them, and whose pickers answer from `files`.
struct Session {
    h: Harness<'static, PdfCraftApp>,
    jobs: Arc<ManualJobs>,
}

impl Session {
    /// Runs the waiting listing jobs and shows their result.
    fn settle(&mut self) {
        self.h.run_steps(2);
        self.jobs.run_all();
        self.h.run_steps(2);
    }
}

fn session(files: ScriptedFiles, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Session {
    let jobs = Arc::new(ManualJobs::new());
    let host_jobs = jobs.clone();
    let files = Arc::new(files);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1400.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::test_app(crate::support::host_with(host_jobs, files));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            setup(&mut app);
            app
        }
    });
    // Fonts install on frame 1 and apply on frame 2.
    h.run_steps(4);
    let mut s = Session { h, jobs };
    s.settle();
    s
}

fn plain(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Session {
    session(ScriptedFiles::new(), setup)
}

fn listed(s: &Session, folder: &str) -> Vec<String> {
    s.h.state().pinned.listing(folder).map(|l| l.files.iter().map(|f| f.name.clone()).collect()).unwrap_or_default()
}

#[test]
fn a_pinned_folder_lists_its_newest_pdfs_on_home() {
    let dir = TempFolder::new("newest");
    dir.file("older.pdf", FIXTURE, Duration::from_secs(2 * 86_400));
    dir.file("newer.PDF", FIXTURE, Duration::from_secs(300));
    dir.file("notes.txt", b"not a pdf", Duration::from_secs(60));
    dir.file(".hidden.pdf", FIXTURE, Duration::from_secs(60));
    std::fs::create_dir_all(dir.0.join("inner.pdf")).unwrap();
    let folder = dir.path();
    let picked = Picked::Paths(vec![PathBuf::from(&folder)]);
    let mut s = session(ScriptedFiles::answering([picked]), |_| {});
    s.h.get_by_label("Pin Folder…");
    assert!(s.h.execute("file.pin_folder"));
    s.settle();
    assert_eq!(s.h.state().pinned.folders, std::slice::from_ref(&folder));
    assert_eq!(listed(&s, &folder), ["newer.PDF", "older.pdf"], "newest first; no text files, hidden files or folders");
    assert_eq!(s.h.state().pinned.listing(&folder).map(|l| l.total), Some(2));
    s.h.get_by_label_contains("newer.PDF");
    s.h.get_by_label_contains("older.pdf");
    assert!(s.h.query_by_label_contains("notes.txt").is_none());
    assert!(s.h.query_by_label_contains(".hidden.pdf").is_none());
}

#[test]
fn clicking_a_pinned_pdf_opens_it_and_adds_it_to_recent() {
    let dir = TempFolder::new("open");
    let path = dir.file("scan.pdf", FIXTURE, Duration::from_secs(10));
    let folder = dir.path();
    let mut s = plain(move |app| {
        app.pin_folder(&folder);
    });
    s.settle();
    s.h.get_by_label_contains("scan.pdf").click();
    // The file is read on a job, as every open is.
    s.settle();
    // The tab strip names the open document.
    s.h.get_by_role_and_label(egui::accesskit::Role::Tab, "scan.pdf");
    // The settings keep the recent files, newest first, as JSON text.
    let saved = serde_json::to_value(s.h.state().settings_snapshot()).unwrap();
    let recent: serde_json::Value = serde_json::from_str(saved["recent"]["recent"].as_str().expect("the recent files are saved")).unwrap();
    assert_eq!(recent[0]["path"].as_str(), Some(path.to_string_lossy().as_ref()));
}

#[test]
fn new_scans_appear_without_reopening_home() {
    let dir = TempFolder::new("rescan");
    dir.file("first.pdf", FIXTURE, Duration::from_secs(600));
    let folder = dir.path();
    let pin = folder.clone();
    let mut s = plain(move |app| {
        app.pin_folder(&pin);
    });
    s.settle();
    assert_eq!(listed(&s, &folder), ["first.pdf"]);
    dir.file("second.pdf", FIXTURE, Duration::from_secs(5));
    // The listing is refreshed every few seconds while Home shows.
    s.h.state_mut().refresh_pinned(1.0e6);
    s.settle();
    assert_eq!(listed(&s, &folder), ["second.pdf", "first.pdf"]);
}

#[test]
fn unpinning_removes_the_folder_but_not_its_files() {
    let dir = TempFolder::new("unpin");
    let path = dir.file("keep.pdf", FIXTURE, Duration::from_secs(10));
    let folder = dir.path();
    let pin = folder.clone();
    let mut s = plain(move |app| {
        app.pin_folder(&pin);
    });
    s.settle();
    let name = folder_name(&folder);
    s.h.get_by_label(&format!("Unpin {name}")).click();
    s.h.run_steps(2);
    assert!(s.h.state().pinned.folders.is_empty());
    assert!(s.h.query_by_label_contains("keep.pdf").is_none());
    assert!(path.exists(), "unpinning never touches the files");
}

/// #5295: a pinned folder folds down to its heading and back, the other folders stay as they
/// are, and the folded state is kept across a restart.
#[test]
fn a_pinned_folder_collapses_to_its_heading_and_stays_collapsed_after_a_restart() {
    let (first, second) = (TempFolder::new("fold-a"), TempFolder::new("fold-b"));
    first.file("alpha.pdf", FIXTURE, Duration::from_secs(10));
    second.file("beta.pdf", FIXTURE, Duration::from_secs(10));
    let (a, b) = (first.path(), second.path());
    let pins = (a.clone(), b.clone());
    let mut s = plain(move |app| {
        app.pin_folder(&pins.0);
        app.pin_folder(&pins.1);
    });
    s.settle();
    let name = folder_name(&a);
    // PLAN: assumed the disclosure is named "Collapse {name}" / "Expand {name}" (ShellWords, pdf-chrome-w5).
    s.h.get_by_label(&format!("Collapse {name}")).click();
    s.h.run_steps(2);
    assert!(s.h.query_by_label_contains("alpha.pdf").is_none(), "a collapsed folder lists nothing");
    s.h.get_by_label_contains("beta.pdf");
    assert_eq!(s.h.state().pinned.collapsed, std::slice::from_ref(&a));

    let mut restarted = crate::control::new_app();
    restarted.apply_settings(&s.h.state().settings_snapshot());
    assert_eq!(restarted.pinned.collapsed, std::slice::from_ref(&a));

    s.h.get_by_label(&format!("Expand {name}")).click();
    s.h.run_steps(2);
    s.h.get_by_label_contains("alpha.pdf");
    assert!(s.h.state().pinned.collapsed.is_empty());

    // Settings are untrusted: only pinned folders, each once.
    let settings = serde_json::json!({ "pinned_folders": [a, b], "pinned_folders_collapsed": [b, b, "/not/pinned", 5] });
    restarted.apply_settings(&pdfcraft_ui_egui::settings::import_json(&settings.to_string()).0);
    assert_eq!(restarted.pinned.collapsed, std::slice::from_ref(&b));
    restarted.unpin_folder(&b);
    assert!(restarted.pinned.collapsed.is_empty(), "unpinning forgets the folded state");
}

/// Two pinned folders of the same name: their parts say where each one is, so a screen reader
/// can tell them apart.
#[test]
fn folders_of_the_same_name_have_buttons_that_tell_them_apart() {
    let root = TempFolder::new("same-name");
    let (a, b) = (root.0.join("work").join("Scans"), root.0.join("home").join("Scans"));
    // A file in each, so each heading folds (a folder with nothing listed has nothing to fold).
    for folder in [&a, &b] {
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(folder.join("scan.pdf"), FIXTURE).unwrap();
    }
    let (a, b) = (a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned());
    let pins = (a.clone(), b.clone());
    let mut s = plain(move |app| {
        app.pin_folder(&pins.0);
        app.pin_folder(&pins.1);
    });
    s.settle();
    for (folder, other) in [(&a, &b), (&b, &a)] {
        let named = format!("Scans ({})", location(folder));
        assert_ne!(named, format!("Scans ({})", location(other)));
        s.h.get_by_label(&format!("Unpin {named}"));
    }
    s.h.get_by_label(&format!("Collapse Scans ({})", location(&a))).click();
    s.h.run_steps(2);
    assert_eq!(s.h.state().pinned.collapsed, std::slice::from_ref(&a), "the part folds its own folder");
}

#[test]
fn pinned_folders_survive_a_restart() {
    let dir = TempFolder::new("restart");
    let folder = dir.path();
    let mut a = crate::control::new_app();
    a.set_option(&egui::Context::default(), "language", "en").unwrap();
    assert!(a.pin_folder(&folder));
    let mut b = crate::control::new_app();
    b.set_option(&egui::Context::default(), "language", "en").unwrap();
    b.apply_settings(&a.settings_snapshot());
    assert_eq!(b.pinned.folders, [folder]);
    for hostile in ["{\"pinned_folders\": [5, \"relative/path\"]}", "{not json"] {
        b.apply_settings(&pdfcraft_ui_egui::settings::import_json(hostile).0);
        assert!(b.pinned.folders.is_empty(), "only absolute folder paths are restored: {hostile}");
    }
}

#[test]
fn an_unavailable_folder_says_so_and_stays_pinned() {
    let missing = std::env::temp_dir().join(format!("pdfcraft-pinned-gone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    let folder = missing.to_string_lossy().into_owned();
    let pin = folder.clone();
    let mut s = plain(move |app| {
        app.pin_folder(&pin);
    });
    s.settle();
    assert_eq!(s.h.state().pinned.folders, std::slice::from_ref(&folder));
    assert!(s.h.state().pinned.listing(&folder).is_some_and(|l| l.unavailable));
    // It comes back by itself once the folder (or its synced drive) is there again.
    std::fs::create_dir_all(&missing).unwrap();
    std::fs::write(missing.join("back.pdf"), FIXTURE).unwrap();
    s.h.state_mut().refresh_pinned(1.0e6);
    s.settle();
    let _ = std::fs::remove_dir_all(&missing);
    assert_eq!(listed(&s, &folder), ["back.pdf"]);
}

#[test]
fn show_more_reveals_the_older_pdfs() {
    let dir = TempFolder::new("more");
    for i in 0..PINNED_FILES_SHOWN + 2 {
        dir.file(&format!("scan{i:02}.pdf"), FIXTURE, Duration::from_secs(60 * (i as u64 + 1)));
    }
    let folder = dir.path();
    let pin = folder.clone();
    let mut s = plain(move |app| {
        app.pin_folder(&pin);
    });
    s.settle();
    s.h.get_by_label_contains("scan00.pdf");
    assert!(s.h.query_by_label_contains("scan09.pdf").is_none(), "only the newest {PINNED_FILES_SHOWN} show at first");
    s.h.get_by_label("Show more").click();
    s.h.run_steps(2);
    s.h.get_by_label_contains("scan09.pdf");
    assert_eq!(s.h.state().pinned.expanded, [folder]);
}

#[test]
fn pinning_is_limited_and_never_duplicates() {
    let mut s = plain(|_| {});
    let app = s.h.state_mut();
    assert!(!app.pin_folder("relative/scans"), "a relative path is refused");
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let folders: Vec<String> = (0..MAX_PINNED).map(|i| Path::new(root).join(format!("pinned-{i}")).to_string_lossy().into_owned()).collect();
    for f in &folders {
        assert!(app.pin_folder(f));
    }
    assert!(app.pin_folder(&folders[0]), "pinning again is fine");
    assert_eq!(app.pinned.folders.len(), MAX_PINNED, "and adds nothing");
    assert!(!app.pin_folder(&Path::new(root).join("one-too-many").to_string_lossy()));
    s.h.run_steps(2);
    // PLAN: assumed `support::shown_notice(&Harness<PdfCraftApp>) -> Option<String>` (pdf-chrome-w9).
    assert!(crate::support::shown_notice(&s.h).is_some_and(|m| m.contains("Unpin one first")), "the user is told");
}
