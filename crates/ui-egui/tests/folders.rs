//! Pinned folders on Home: File ▸ Pin folder to Home…, the newest PDFs listed per folder,
//! opening one, unpinning, and the setting surviving a restart.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::folders_ui::{MAX_PINNED, SHOWN};

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

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1400.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // List folders on the frame that asks, not on a worker thread.
        app.run_inline = true;
        setup(&mut app);
        app
    });
    // Fonts install on frame 1 and apply on frame 2.
    h.run_steps(4);
    h
}

fn listed(h: &Harness<'static, PdfCraftApp>, folder: &str) -> Vec<String> {
    h.state().pinned.listing(folder).map(|l| l.files.iter().map(|f| f.name.clone()).collect()).unwrap_or_default()
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
    let pick = folder.clone();
    let mut h = harness(move |app| app.pick_override = Some(vec![pick]));
    h.get_by_label("Pin a folder…");
    assert!(h.state_mut().execute("file.pin_folder"));
    h.run_steps(4);
    assert_eq!(h.state().pinned.folders, std::slice::from_ref(&folder));
    assert_eq!(listed(&h, &folder), ["newer.PDF", "older.pdf"], "newest first; no text files, hidden files or folders");
    assert_eq!(h.state().pinned.listing(&folder).map(|l| l.total), Some(2));
    h.get_by_label("newer.PDF");
    h.get_by_label("older.pdf");
    assert!(h.query_by_label("notes.txt").is_none());
    assert!(h.query_by_label(".hidden.pdf").is_none());
}

#[test]
fn clicking_a_pinned_pdf_opens_it_and_adds_it_to_recent() {
    let dir = TempFolder::new("open");
    let path = dir.file("scan.pdf", FIXTURE, Duration::from_secs(10));
    let folder = dir.path();
    let mut h = harness(move |app| {
        app.pin_folder(&folder);
    });
    h.run_steps(2);
    h.get_by_label("scan.pdf").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the PDF opens in a tab");
    assert_eq!(app.recent.first().map(|r| r.path.clone()), Some(path.to_string_lossy().into_owned()));
}

#[test]
fn new_scans_appear_without_reopening_home() {
    let dir = TempFolder::new("rescan");
    dir.file("first.pdf", FIXTURE, Duration::from_secs(600));
    let folder = dir.path();
    let pin = folder.clone();
    let mut h = harness(move |app| {
        app.pin_folder(&pin);
    });
    h.run_steps(2);
    assert_eq!(listed(&h, &folder), ["first.pdf"]);
    dir.file("second.pdf", FIXTURE, Duration::from_secs(5));
    // The listing is refreshed every few seconds while Home shows.
    h.state_mut().refresh_pinned(1.0e6);
    assert_eq!(listed(&h, &folder), ["second.pdf", "first.pdf"]);
}

#[test]
fn unpinning_removes_the_folder_but_not_its_files() {
    let dir = TempFolder::new("unpin");
    let path = dir.file("keep.pdf", FIXTURE, Duration::from_secs(10));
    let folder = dir.path();
    let pin = folder.clone();
    let mut h = harness(move |app| {
        app.pin_folder(&pin);
    });
    h.run_steps(2);
    let name = pdfcraft_ui_egui::folders_ui::folder_name(&folder);
    h.get_by_label(&format!("Unpin {name}")).click();
    h.run_steps(2);
    assert!(h.state().pinned.folders.is_empty());
    assert!(h.query_by_label("keep.pdf").is_none());
    assert!(path.exists(), "unpinning never touches the files");
}

#[test]
fn pinned_folders_survive_a_restart() {
    let dir = TempFolder::new("restart");
    let folder = dir.path();
    let mut a = PdfCraftApp::new();
    a.set_option("language", "en").unwrap();
    assert!(a.pin_folder(&folder));
    let mut b = PdfCraftApp::new();
    b.set_option("language", "en").unwrap();
    b.restore(&a.persist());
    assert_eq!(b.pinned.folders, [folder]);
    b.restore("{\"pinned_folders\": [5, \"relative/path\"]}");
    assert!(b.pinned.folders.is_empty(), "only absolute folder paths are restored");
    b.restore("{not json");
    assert!(b.pinned.folders.is_empty());
}

#[test]
fn an_unavailable_folder_says_so_and_stays_pinned() {
    let missing = std::env::temp_dir().join(format!("pdfcraft-pinned-gone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    let folder = missing.to_string_lossy().into_owned();
    let pin = folder.clone();
    let mut h = harness(move |app| {
        app.pin_folder(&pin);
    });
    h.run_steps(2);
    h.get_by_label_contains("isn't available right now");
    assert_eq!(h.state().pinned.folders, std::slice::from_ref(&folder));
    // It comes back by itself once the folder (or its synced drive) is there again.
    std::fs::create_dir_all(&missing).unwrap();
    std::fs::write(missing.join("back.pdf"), FIXTURE).unwrap();
    h.state_mut().refresh_pinned(1.0e6);
    h.run_steps(2);
    let _ = std::fs::remove_dir_all(&missing);
    assert_eq!(listed(&h, &folder), ["back.pdf"]);
}

#[test]
fn show_more_reveals_the_older_pdfs() {
    let dir = TempFolder::new("more");
    for i in 0..SHOWN + 2 {
        dir.file(&format!("scan{i:02}.pdf"), FIXTURE, Duration::from_secs(60 * (i as u64 + 1)));
    }
    let folder = dir.path();
    let mut h = harness(move |app| {
        app.pin_folder(&folder);
    });
    h.run_steps(2);
    h.get_by_label("scan00.pdf");
    assert!(h.query_by_label("scan09.pdf").is_none(), "only the newest {SHOWN} show at first");
    h.get_by_label("Show 2 more").click();
    h.run_steps(2);
    h.get_by_label("scan09.pdf");
    h.get_by_label("Show less");
}

#[test]
fn a_long_pdf_name_is_cut_before_its_date_and_size() {
    let dir = TempFolder::new("long");
    // Short enough for Windows' 260-character paths under the temp folder.
    let name = format!("{}.pdf", "Signed contract with a long name ".repeat(4));
    dir.file(&name, FIXTURE, Duration::from_secs(10));
    let folder = dir.path();
    let mut h = harness(move |app| {
        app.pin_folder(&folder);
    });
    h.run_steps(2);
    let text: Vec<(String, bool, egui::Rect)> = h
        .output()
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.elided, t.visual_bounding_rect())),
            _ => None,
        })
        .collect();
    let (_, cut, rect) = text.iter().find(|(s, ..)| *s == name).expect("the PDF is listed");
    let detail = text.iter().find(|(s, _, r)| s.contains('·') && r.y_range().contains(rect.center().y)).expect("its row shows the date and size").2;
    assert!(*cut, "the name is cut short");
    assert!(rect.right() < detail.left(), "the name ends before the date and size");
}

#[test]
fn pinning_is_limited_and_never_duplicates() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    assert!(!app.pin_folder("relative/scans"), "a relative path is refused");
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let folders: Vec<String> = (0..MAX_PINNED).map(|i| Path::new(root).join(format!("pinned-{i}")).to_string_lossy().into_owned()).collect();
    for f in &folders {
        assert!(app.pin_folder(f));
    }
    assert!(app.pin_folder(&folders[0]), "pinning again is fine");
    assert_eq!(app.pinned.folders.len(), MAX_PINNED, "and adds nothing");
    assert!(!app.pin_folder(&Path::new(root).join("one-too-many").to_string_lossy()));
    assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("Unpin one first")), "the user is told");
}
