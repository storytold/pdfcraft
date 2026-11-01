//! A large file opens from disk with only its structure in memory (`core.lazy-loading`): it can
//! be read and copied, and one click reads all of it so that it can be edited.

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{OpenOutcome, PdfCraftApp, SaveTarget};

use crate::support;

/// A two-page file whose content streams are long enough to stay on disk.
fn large() -> Vec<u8> {
    let content = |page: usize| format!("BT /F 24 Tf 20 150 Td (Page {page}) Tj ET % {}", "x".repeat(30_000));
    pdfcraft_testkit::Pdf::new()
        .obj("<</Type/Catalog/Pages 2 0 R>>")
        .obj("<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2/MediaBox[0 0 200 300]>>")
        .obj("<</Type/Page/Parent 2 0 R/Contents 5 0 R/Resources<</Font<</F 7 0 R>>>>>>")
        .obj("<</Type/Page/Parent 2 0 R/Contents 6 0 R/Resources<</Font<</F 7 0 R>>>>>>")
        .stream("", content(1))
        .stream("", content(2))
        .obj("<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>")
        .build()
}

/// A file with more pages than fit on screen, so that a copy of all of it has pages to read.
fn long() -> Vec<u8> {
    let pages = 40;
    let kids: String = (0..pages).map(|page| format!("{} 0 R ", 4 + 2 * page)).collect();
    let mut pdf = pdfcraft_testkit::Pdf::new()
        .obj("<</Type/Catalog/Pages 2 0 R>>")
        .obj(format!("<</Type/Pages/Kids[{kids}]/Count {pages}/MediaBox[0 0 200 300]>>"))
        .obj("<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>");
    for page in 0..pages {
        pdf = pdf
            .obj(format!("<</Type/Page/Parent 2 0 R/Contents {} 0 R/Resources<</Font<</F 3 0 R>>>>>>", 5 + 2 * page))
            .stream("", format!("BT /F 24 Tf 20 150 Td (Page {page}) Tj ET % {}", "x".repeat(30_000)));
    }
    pdf.build()
}

/// A file in its own folder, gone when the test ends.
struct Temp {
    dir: PathBuf,
    path: String,
}

impl Temp {
    fn new(test: &str, bytes: &[u8]) -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-large-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("large.pdf").to_string_lossy().into_owned();
        std::fs::write(&path, bytes).unwrap();
        Self { dir, path }
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn harness(inline: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = inline;
        // Any file counts as large here.
        app.session = pdfcraft_engine::Session::default().with_on_disk_from(0);
        app
    });
    h.run_steps(2);
    h
}

fn on_disk(app: &PdfCraftApp) -> bool {
    app.views.first().and_then(|v| app.session.get(v.id)).is_some_and(|d| d.on_disk.is_some())
}

#[test]
fn a_large_file_opens_from_disk_and_a_click_makes_it_editable() {
    let file = Temp::new("load", &large());
    let mut h = harness(true);
    assert_eq!(h.state_mut().open_path(&file.path), OpenOutcome::Opened);
    h.run_steps(4);
    assert!(on_disk(h.state()), "only the structure is in memory");
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().info.pages.len(), 2);
    assert!(h.state().recent.first().is_some_and(|r| r.size == large().len()), "recent files show its real size");
    let edit = pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 };
    assert!(!h.state_mut().apply_edit(edit.clone()), "not editable from disk");

    h.get_by_label("Load to edit").click();
    h.run_steps(4);
    assert!(!on_disk(h.state()), "all of it is in memory now");
    assert_eq!(h.state().views.len(), 1, "in the same tab");
    assert_eq!(h.state().views[0].id, id);
    assert!(h.state_mut().apply_edit(edit), "and it edits");
    assert!(h.query_by_label("Load to edit").is_none(), "the offer is gone");
}

#[test]
fn a_running_app_opens_and_loads_it_on_its_worker() {
    let file = Temp::new("worker", &large());
    let mut h = harness(false);
    assert_eq!(h.state_mut().open_path(&file.path), OpenOutcome::Pending);
    support::wait_until(&mut h, "the file opens", |app| !app.views.is_empty());
    assert!(on_disk(h.state()));
    assert!(h.state().recent.first().is_some_and(|r| r.size == large().len()));
    h.run_steps(2);
    h.get_by_label("Load to edit").click();
    support::wait_until(&mut h, "the whole file loads", |app| !on_disk(app));
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn save_as_copies_the_file_and_saving_in_place_changes_nothing() {
    let pdf = large();
    let file = Temp::new("save", &pdf);
    let mut h = harness(true);
    h.state_mut().open_path(&file.path);
    h.run_steps(4);
    let copy = file.dir.join("copy.pdf").to_string_lossy().into_owned();
    h.state_mut().save_override = Some(copy.clone());
    assert!(h.state_mut().save_active(SaveTarget::As));
    assert_eq!(std::fs::read(&copy).unwrap(), pdf, "the file itself, not what is in memory");
    let app = h.state();
    assert_eq!(app.session.get(app.views[0].id).unwrap().path.as_deref(), Some(file.path.as_str()), "it keeps reading its own file");

    h.state_mut().save_override = None;
    assert!(h.state_mut().save_active(SaveTarget::InPlace));
    assert_eq!(std::fs::read(&file.path).unwrap(), pdf);
    assert_ne!(h.state().session.get(h.state().views[0].id).map(|d| d.dirty), Some(true));
}

#[test]
fn saving_in_place_refuses_a_file_that_has_gone_and_save_as_still_copies_it() {
    let pdf = large();
    let file = Temp::new("deleted", &pdf);
    let mut h = harness(true);
    h.state_mut().open_path(&file.path);
    h.run_steps(4);
    assert!(on_disk(h.state()));
    std::fs::remove_file(&file.path).unwrap();
    assert!(!h.state_mut().save_active(SaveTarget::InPlace));
    assert!(!std::path::Path::new(&file.path).exists());
    let copy = file.dir.join("copy.pdf").to_string_lossy().into_owned();
    h.state_mut().save_override = Some(copy.clone());
    assert!(h.state_mut().save_active(SaveTarget::As));
    assert_eq!(std::fs::read(&copy).unwrap(), pdf, "the tab still reads the file it opened");
}

#[test]
fn saving_in_place_leaves_a_file_that_took_its_name() {
    let pdf = large();
    let mut same_size = pdf.clone();
    same_size[0] ^= 1;
    for bytes in [b"another program's newer file".to_vec(), same_size] {
        let file = Temp::new("replaced", &pdf);
        let mut h = harness(true);
        h.state_mut().open_path(&file.path);
        h.run_steps(4);
        assert!(on_disk(h.state()));
        let newer = file.dir.join("newer.pdf");
        std::fs::write(&newer, &bytes).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&newer).unwrap().set_modified(later).unwrap();
        std::fs::rename(&newer, &file.path).unwrap();
        assert!(!h.state_mut().save_active(SaveTarget::InPlace));
        assert_eq!(std::fs::read(&file.path).unwrap(), bytes);
    }
}

#[test]
fn print_says_to_load_the_file_and_can_not_be_clicked() {
    let file = Temp::new("print", &large());
    let mut h = harness(true);
    h.state_mut().open_path(&file.path);
    h.run_steps(4);
    assert!(on_disk(h.state()));
    h.state_mut().open_print();
    h.run_steps(4);
    assert!(h.query_by_label_contains("To print this large file").is_some());
    let print =
        h.query_by(|n| n.role() == egui::accesskit::Role::Button && matches!(n.label().as_deref(), Some("Print" | "Save as PDF")) && n.is_disabled());
    assert!(print.is_some());
}

#[test]
fn a_search_marks_nothing_it_can_not_apply() {
    let file = Temp::new("redact", &large());
    let mut h = harness(true);
    h.state_mut().open_path(&file.path);
    h.run_steps(4);
    assert!(on_disk(h.state()));
    h.state_mut().redact_search.text = "Page".into();
    assert_eq!(h.state_mut().redact_search(), 0);
}

#[test]
fn copying_all_of_a_file_cut_short_says_so_and_copies_nothing() {
    let file = Temp::new("copy", &long());
    let mut h = harness(true);
    h.state_mut().open_path(&file.path);
    h.run_steps(4);
    assert!(on_disk(h.state()));
    let mut selected = false;
    for _ in 0..200 {
        h.run_steps(2);
        selected = h.state_mut().views[0].select_all();
        if selected {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(selected, "the pages shown have their text");
    std::fs::File::options().write(true).open(&file.path).unwrap().set_len(1_000).unwrap();
    h.state_mut().toast = None;
    h.event(egui::Event::Copy);
    let (mut copied, mut said) = (false, None);
    for _ in 0..200 {
        h.step();
        copied |= h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_)));
        said = h.state().toast.as_ref().map(|toast| toast.0.clone());
        if copied || said.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!copied, "the text would be short of the pages left unread");
    assert!(said.as_ref().is_some_and(|s| s.contains(pdfcraft_render::PART_UNREAD)), "{said:?}");
}

#[test]
fn a_file_that_can_not_stay_on_disk_opens_as_before() {
    let mut damaged = large();
    let tail = damaged.windows(9).rposition(|w| w == b"startxref").unwrap();
    damaged.truncate(tail);
    let file = Temp::new("damaged", &damaged);
    let mut h = harness(true);
    assert_eq!(h.state_mut().open_path(&file.path), OpenOutcome::Opened);
    h.run_steps(4);
    assert!(!on_disk(h.state()));
    assert!(h.query_by_label("Load to edit").is_none());
}
