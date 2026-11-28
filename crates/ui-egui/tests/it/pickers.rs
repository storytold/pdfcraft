//! Native file pickers never block the frame. A blocking picker ran a nested run loop inside
//! winit's event handler on macOS, which crashed the app when the user clicked File ▸ Open (or
//! any other open, save or folder picker). Commands now return at once, and the chosen files are
//! used on a later frame. The host's `ScriptedFiles` stands in for the native panel and, like it,
//! answers on a later frame.

use crate::support;

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::support::AppHarness as _;
use craft::host::{AppInfo, Host, ManualJobs, PickKind, Picked, ScriptedFiles, Wake};
use craft::platform::clipboard::{Clipboard, EventClipboard};
use craft::platform::open::{FileIntake, IntakeSender};
use craft::platform::watch::FileWatch;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::{AppServices, PdfCraftApp};

/// An `n`-page document with a proper xref table.
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn pages(app: &PdfCraftApp, view: usize) -> usize {
    let doc = app.session.get(app.views[view].id).unwrap();
    PageRenderer::new(doc.bytes.clone(), Default::default()).page_count()
}

/// The app on a host whose files are scripted and whose jobs wait for the test, with the sender
/// of its file intake (what the OS and the browser deliver opens through).
struct Rig {
    h: Harness<'static, PdfCraftApp>,
    files: Arc<ScriptedFiles>,
    jobs: Arc<ManualJobs>,
    intake: IntakeSender,
}

impl Deref for Rig {
    type Target = Harness<'static, PdfCraftApp>;
    fn deref(&self) -> &Self::Target {
        &self.h
    }
}

impl DerefMut for Rig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.h
    }
}

impl Rig {
    /// `n` frames, running the jobs the app has queued before each one.
    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.jobs.run_all();
            self.h.run_steps(1);
        }
    }

    /// Answers the open pick and lets the app use the answer on later frames.
    fn answer(&mut self, picked: Picked) {
        assert!(self.files.answer(picked).is_some(), "a pick was open to answer");
        self.steps(2);
    }

    /// Hands the app a file that arrived as bytes, as a startup URL or a browser read does.
    fn deliver(&self, name: &str, bytes: Vec<u8>) {
        self.intake.open_bytes(name.into(), Arc::from(bytes));
    }
}

/// A shell with `docs` open, each the given number of pages, whose picks stay open until the
/// test answers them.
fn harness(docs: &[usize]) -> Rig {
    rig(docs, ScriptedFiles::held())
}

fn rig(docs: &[usize], files: ScriptedFiles) -> Rig {
    let files = Arc::new(files);
    rig_with_file_service(docs, files.clone(), files)
}

fn rig_with_file_service(docs: &[usize], files: Arc<ScriptedFiles>, file_service: Arc<dyn craft::host::Files>) -> Rig {
    let jobs = Arc::new(ManualJobs::new());
    let host = Arc::new(Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).jobs(jobs.clone()).files(file_service).build());

    let (intake, sender) = FileIntake::new(Wake::none());
    let services = AppServices::new(
        Wake::none(),
        Clipboard::new(EventClipboard::new(|| 0, |_| Ok(0))),
        None,
        None,
        intake,
        FileWatch::new(Wake::none()).inline(),
    );

    let docs = docs.to_vec();
    let run = jobs.clone();
    let h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = PdfCraftApp::with_host(host, services);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            for (i, n) in docs.iter().enumerate() {
                app.open_bytes(&egui::Context::default(), &format!("doc{i}.pdf"), None, fixture(*n)).unwrap();
            }
            run.run_all();
            app
        }
    });
    let mut rig = Rig { h, files, jobs, intake: sender };
    rig.steps(2);
    rig
}

/// Runs frames, and the jobs between them, until `ready` holds.
fn settle(r: &mut Rig, what: &str, ready: impl Fn(&PdfCraftApp) -> bool) {
    for _ in 0..500 {
        r.steps(1);
        if ready(r.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// The unsaved-changes question's "save it" answer, in production's words on every platform.
fn save_answer() -> String {
    "Save".to_string()
}

/// The unsaved-changes question's "don't save" answer, in production's words on every platform.
fn dont_save_answer() -> String {
    "Don't save".to_string()
}

/// Presses the button called `label` of the dialog on top (the last such node drawn).
fn press(r: &mut Rig, label: &str) {
    r.get_all_by_label(label).last().unwrap_or_else(|| panic!("no {label:?} button on screen")).click();
    r.steps(2);
}

#[test]
fn file_open_returns_at_once_and_opens_the_pick_when_preparation_finishes() {
    let dir = scratch("open");
    let file = dir.join("picked.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[]);
    assert!(h.execute("file.open"));
    assert!(h.state().views.is_empty(), "the command must not wait for the picker");
    assert!(matches!(h.files.requests().first().map(|r| r.kind), Some(PickKind::Open | PickKind::OpenMany)));
    h.answer(Picked::Paths(vec![file.clone()]));
    settle(&mut h, "picked document", |app| app.views.len() == 1 && app.password_prompt.is_none());
    assert_eq!(h.state().views.len(), 1, "the picked file opens on a later frame");
    assert_eq!(pages(h.state(), 0), 2);
    let (_, id) = h.state().active_ids().unwrap();
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.path.as_deref(), Some(file.to_string_lossy().as_ref()));
    assert_eq!(doc.bytes.as_slice(), fixture(2).as_slice());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn insert_from_file_uses_the_pick_on_the_next_frame() {
    let dir = scratch("insert");
    let file = dir.join("more.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[3]);
    assert!(h.execute("page.insert"));
    h.steps(2);
    assert_eq!(pages(h.state(), 0), 3, "nothing is inserted until the pick arrives");
    h.answer(Picked::Paths(vec![file]));
    assert_eq!(pages(h.state(), 0), 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pick_for_a_document_that_is_no_longer_active_is_dropped() {
    let dir = scratch("switch");
    let file = dir.join("more.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[3, 4]);
    h.state_mut().active = Some(0);
    assert!(h.execute("page.insert"));
    // The user switches tabs before the pick arrives.
    h.state_mut().active = Some(1);
    h.answer(Picked::Paths(vec![file]));
    assert_eq!(pages(h.state(), 0), 3, "the document the pick was for is unchanged");
    assert_eq!(pages(h.state(), 1), 4, "the newly active document is unchanged");
    assert!(support::shown_notice(&h).is_some(), "the user is told why nothing happened");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scratch folder for one test, of its own for each call: the tests share one process and run
/// in parallel, so two that picked the same name would delete each other's folder (#4186).
fn scratch(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("pdfcraft-pickers-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn page_count(bytes: Vec<u8>) -> usize {
    PageRenderer::new(std::sync::Arc::new(bytes), Default::default()).page_count()
}

fn dirty(app: &PdfCraftApp, view: usize) -> bool {
    app.session.get(app.views[view].id).is_some_and(|d| d.dirty)
}

fn rotate_first_page(app: &mut PdfCraftApp) {
    app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
}

#[test]
fn save_as_writes_on_a_later_frame_including_edits_made_meanwhile() {
    let dir = scratch("save-as");
    let out = dir.join("saved.pdf");
    let mut h = harness(&[3]);
    assert!(h.execute("file.save_as"));
    h.steps(2);
    assert!(h.files.file(&out).is_none(), "the command must not wait for the picker");
    assert_eq!(h.files.open(), 1, "the Save picker is open");
    // The user keeps working while the panel is open.
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    h.answer(Picked::Paths(vec![out.clone()]));
    assert_eq!(page_count(h.files.file(&out).unwrap()), 2, "the edit made while choosing is saved too");
    assert!(!dirty(h.state(), 0));
    assert!(matches!(h.files.requests().first().map(|r| r.kind), Some(PickKind::Save)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_on_close_closes_the_tab_only_once_the_file_is_written() {
    let dir = scratch("save-close");
    let out = dir.join("closed.pdf");
    let mut h = harness(&[2]);
    rotate_first_page(h.state_mut());
    assert!(h.execute("file.close"));
    h.steps(2);
    assert_eq!(h.state().views.len(), 1, "a dirty document asks first");
    press(&mut h, &save_answer());
    assert_eq!(h.state().views.len(), 1, "the tab stays open until the save is written");
    assert_eq!(h.files.open(), 1, "the Save picker is open");
    h.answer(Picked::Paths(vec![out.clone()]));
    assert!(h.files.file(&out).is_some());
    assert!(h.state().views.is_empty(), "then it closes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_save_on_close_keeps_the_tab_open() {
    let mut h = harness(&[2]);
    rotate_first_page(h.state_mut());
    assert!(h.execute("file.close"));
    h.steps(2);
    press(&mut h, &save_answer());
    h.answer(Picked::Cancelled); // the user cancels the save panel
    assert_eq!(h.state().views.len(), 1);
    assert!(dirty(h.state(), 0), "nothing was saved or lost");
}

#[test]
fn close_all_saves_each_untitled_document_in_turn() {
    let dir = scratch("close-all");
    let mut h = harness(&[2, 3]);
    for i in 0..2 {
        h.state_mut().active = Some(i);
        rotate_first_page(h.state_mut());
    }
    assert!(h.execute("file.close_all"));
    h.steps(2);
    for (n, name) in ["first.pdf", "second.pdf"].iter().enumerate() {
        press(&mut h, &save_answer());
        assert_eq!(h.files.open(), 1, "one Save pick for document {n}");
        h.answer(Picked::Paths(vec![dir.join(name)]));
    }
    assert!(h.state().views.is_empty());
    let asked = h.files.requests();
    assert_eq!(asked.len(), 2, "one Save pick per untitled document");
    assert!(asked.iter().all(|r| r.kind == PickKind::Save));
    assert_eq!(page_count(h.files.file(&dir.join("first.pdf")).unwrap()), 2);
    assert_eq!(page_count(h.files.file(&dir.join("second.pdf")).unwrap()), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sets the checkbox called `label` of the open dialog to `want`.
fn set_checked(r: &mut Rig, label: &str, want: bool) {
    let on = r.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True);
    if on != want {
        r.get_by_label(label).click();
        r.steps(2);
    }
}

/// Replaces what the text field called `label` holds with `text`.
fn type_into(r: &mut Rig, label: &str, text: &str) {
    r.get_by_role_and_label(egui::accesskit::Role::TextInput, label).click();
    r.steps(1);
    r.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    r.event(egui::Event::Text(text.into()));
    r.steps(2);
}

/// What the Extract dialog is filled with; `None` leaves a field as the dialog opened it.
#[derive(Default)]
struct Extract<'a> {
    name: Option<&'a str>,
    pages: Option<&'a str>,
    separate: bool,
    delete: bool,
}

/// Opens Extract pages through its command and fills it in; leaves the dialog open.
fn open_extract(r: &mut Rig, e: Extract) {
    assert!(r.execute("page.extract"));
    r.steps(2);
    if let Some(pages) = e.pages {
        type_into(r, "Pages", pages);
    }
    if let Some(name) = e.name {
        type_into(r, "File name", name);
    }
    set_checked(r, "Extract pages as separate files", e.separate);
    set_checked(r, "Delete pages after extracting", e.delete);
}

/// The dialog's Extract button.
fn confirm_extract(r: &mut Rig) {
    press(r, "Extract");
}

/// The tab name of the active document.
fn active_name(app: &PdfCraftApp) -> Option<String> {
    app.active.and_then(|i| app.views.get(i)).and_then(|v| app.session.get(v.id)).map(|d| d.name.clone())
}

#[test]
fn extract_and_delete_deletes_the_pages_only_after_their_files_are_written() {
    let dir = scratch("extract");
    let mut h = harness(&[3]);
    open_extract(&mut h, Extract { separate: true, delete: true, ..Default::default() });
    confirm_extract(&mut h);
    assert_eq!(h.files.open(), 1, "the folder picker is open");
    assert_eq!(pages(h.state(), 0), 3, "nothing is deleted while the folder picker is open");
    h.answer(Picked::Paths(vec![dir.clone()]));
    assert!(h.files.file(&dir.join("doc0 (page 1).pdf")).is_some());
    assert_eq!(pages(h.state(), 0), 2, "the extracted page is deleted once written");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #737: Extract pages names the files after the name typed in the dialog, which starts out
/// as the document's name; left as it was (or with nothing usable in it), the names are the
/// ones Extract always used.
#[test]
fn extract_pages_saves_under_the_file_name_typed_in_the_dialog() {
    let dir = scratch("extract-named");
    let mut h = harness(&[3]);
    let separate = |name| Extract { name, separate: true, ..Default::default() };

    // The dialog starts with the document's name, which gives the usual names.
    open_extract(&mut h, separate(None));
    assert_eq!(
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "File name").value().as_deref(),
        Some("doc0"),
        "the dialog starts with the document's name"
    );
    confirm_extract(&mut h);
    h.answer(Picked::Paths(vec![dir.clone()]));
    let written = |h: &Rig, name: &str| h.files.file(&dir.join(name)).is_some();
    assert!(written(&h, "doc0 (page 1).pdf"));
    // A name with nothing usable falls back to the document's.
    h.state_mut().views[0].select_pages(&[1]);
    open_extract(&mut h, separate(Some("  ..  ")));
    confirm_extract(&mut h);
    h.answer(Picked::Paths(vec![dir.clone()]));
    assert!(written(&h, "doc0 (page 2).pdf"));
    // Several pages as separate files: each is "<name> (page N).pdf".
    h.state_mut().views[0].select_pages(&[0, 2]);
    open_extract(&mut h, separate(Some("Invoices.pdf")));
    confirm_extract(&mut h);
    h.answer(Picked::Paths(vec![dir.clone()]));
    assert!(written(&h, "Invoices (page 1).pdf") && written(&h, "Invoices (page 3).pdf"));
    // One renamed page as a separate file is saved under exactly that name; characters a file
    // name can't hold become "_".
    h.state_mut().views[0].select_pages(&[1]);
    open_extract(&mut h, separate(Some(" March: 2026/receipt ")));
    confirm_extract(&mut h);
    h.answer(Picked::Paths(vec![dir.clone()]));
    assert!(written(&h, "March_ 2026_receipt.pdf"));
    // The writes go through the host's file service, not to disk.
    let names: std::collections::BTreeSet<_> = h.files.written().into_iter().filter(|p| p.starts_with(&dir)).collect();
    assert_eq!(names.len(), 5, "nothing written outside those names: {names:?}");
    // Into a new tab: the tab, and so Save As, has the typed name.
    open_extract(&mut h, Extract { name: Some("Chapter 2"), ..Default::default() });
    confirm_extract(&mut h);
    assert_eq!(active_name(h.state()).as_deref(), Some("Chapter 2.pdf"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The text of each page of document `view`.
fn texts_of(app: &PdfCraftApp, view: usize) -> Vec<String> {
    let doc = app.session.get(app.views[view].id).unwrap();
    let mut r = PageRenderer::new(doc.bytes.clone(), Default::default());
    (0..r.page_count())
        .map(|p| {
            let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            out.text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
}

/// #3593: Extract pages takes page ranges typed in its Pages field, in the order typed, each page
/// once; left empty, it takes the selected pages as before. A range it can't use is refused, with
/// the Extract button off.
#[test]
fn extract_pages_takes_the_page_ranges_typed_in_the_dialog() {
    let mut h = harness(&[6]);
    h.state_mut().views[0].select_pages(&[1]);
    assert!(h.execute("page.extract"));
    h.steps(2);
    assert!(h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Pages").value().unwrap_or_default().is_empty(), "opens on the selection");
    press(&mut h, "Cancel");
    // Ranges, a backward one and a repeat: in that order, each page once.
    open_extract(&mut h, Extract { pages: Some("5-6, 3-2, 6"), ..Default::default() });
    confirm_extract(&mut h);
    assert_eq!(h.state().views.len(), 2, "the pages open in a new tab");
    assert_eq!(texts_of(h.state(), 1), ["Page 5", "Page 6", "Page 3", "Page 2"]);
    // A page that isn't there: Extract is off and extracts nothing.
    h.state_mut().active = Some(0);
    open_extract(&mut h, Extract { pages: Some("2, 9"), ..Default::default() });
    assert!(h.get_all_by_label("Extract").last().unwrap().accesskit_node().is_disabled(), "Extract is off");
    assert!(h.query_all_by_label("page 9 is out of range (1–6)").next().is_some(), "the invalid range is explained");
    press(&mut h, "Cancel");
    assert_eq!(h.state().views.len(), 2, "nothing extracted");
    // Extract and delete takes the typed pages out of the document.
    h.state_mut().active = Some(0);
    open_extract(&mut h, Extract { pages: Some("6, 1-2"), delete: true, ..Default::default() });
    confirm_extract(&mut h);
    assert_eq!(texts_of(h.state(), 2), ["Page 6", "Page 1", "Page 2"]);
    assert_eq!(texts_of(h.state(), 0), ["Page 3", "Page 4", "Page 5"]);
}

#[test]
fn a_cancelled_folder_pick_extracts_and_deletes_nothing() {
    let mut h = harness(&[3]);
    open_extract(&mut h, Extract { separate: true, delete: true, ..Default::default() });
    confirm_extract(&mut h);
    h.answer(Picked::Cancelled);
    assert_eq!(pages(h.state(), 0), 3);
}

#[test]
fn import_comments_takes_the_comments_of_another_pdf() {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (from the review) /P 3 0 R >>",
    ];
    let mut review = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(review.len());
        review.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = review.len();
    review.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        review.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    review.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let dir = scratch("import-pdf-comments");
    let path = dir.join("review.pdf");
    // The import reads the picked file through the host's file service.
    let mut h = rig(&[1], ScriptedFiles::held().with_file(&path, review));
    assert!(h.execute("comment.import"));
    h.steps(2);
    h.answer(Picked::Paths(vec![path]));
    let comments = |app: &PdfCraftApp| -> Vec<Option<String>> {
        app.session.get(app.views[0].id).unwrap().info.annotations.iter().map(|a| a.contents.clone()).collect()
    };
    assert_eq!(comments(h.state()), [Some("from the review".to_string())], "the review's comment is imported once");
    assert!(h.execute("edit.undo"));
    h.steps(1);
    assert!(comments(h.state()).is_empty(), "the import undoes in one step");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extracted_pages_are_not_deleted_if_the_document_was_edited_while_choosing_the_folder() {
    let dir = scratch("extract-edited");
    let mut h = harness(&[3]);
    open_extract(&mut h, Extract { separate: true, delete: true, ..Default::default() });
    confirm_extract(&mut h);
    // While the folder picker is open the user deletes page 1 themselves: the page that would
    // now be deleted is the original page 2, which was never written out.
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    h.answer(Picked::Paths(vec![dir.clone()]));
    assert!(h.files.file(&dir.join("doc0 (page 1).pdf")).is_some(), "the extracted file is still written");
    assert_eq!(pages(h.state(), 0), 2, "nothing more is deleted");
    assert!(support::shown_notice(&h).is_some(), "the user is told");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_close_prompt_still_means_its_own_document_after_a_pending_save_closes_another_tab() {
    let dir = scratch("close-shift");
    let mut h = harness(&[1, 2, 3]);
    for i in 0..3 {
        h.state_mut().active = Some(i);
        rotate_first_page(h.state_mut());
    }
    let (b, c) = (h.state().views[1].id, h.state().views[2].id);
    // Save-and-close the first tab; its save panel is still open...
    h.state_mut().active = Some(0);
    assert!(h.execute("file.close"));
    h.steps(2);
    press(&mut h, &save_answer());
    assert_eq!(h.files.open(), 1, "the first tab's save panel is open");
    // ...when the user asks to close the second one.
    h.state_mut().active = Some(1);
    assert!(h.execute("file.close"));
    h.steps(2);
    h.answer(Picked::Paths(vec![dir.join("a.pdf")]));
    assert_eq!(h.state().views.len(), 2, "the first tab closed once saved");
    // Don't Save: the prompt was about the second document, which is now the first tab.
    press(&mut h, &dont_save_answer());
    let open: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    assert_eq!(open, vec![c], "the second document closed, not the third: {b:?} vs {c:?}");
    assert!(dirty(h.state(), 0), "the third document keeps its unsaved changes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_watermark_file_from_browse_lands_only_in_the_dialog_that_asked() {
    let dir = scratch("browse");
    let file = dir.join("mark.pdf");
    for keep_open in [true, false] {
        // The dialog reads the picked file through the host's file service.
        let mut h = rig(&[1], ScriptedFiles::held().with_file(&file, fixture(1)));
        assert!(h.execute("edit.watermark"));
        h.steps(2);
        h.get_by_role_and_label(egui::accesskit::Role::RadioButton, "File").click();
        h.steps(2);
        h.get_by_label("Browse…").click();
        h.steps(1);
        assert_eq!(h.files.open(), 1, "Browse opened the picker");
        if !keep_open {
            h.key_press(egui::Key::Escape); // closed before the pick arrives
            h.steps(2);
            assert!(h.query_by_label("Browse…").is_none(), "the dialog is closed");
        }
        h.answer(Picked::Paths(vec![file.clone()]));
        // The picked file is the File box's text.
        let file_box = |h: &Rig| {
            h.query_all_by(|n| n.role() == egui::accesskit::Role::TextInput && n.value().is_some_and(|v| v.contains("mark.pdf"))).next().is_some()
        };
        if keep_open {
            assert!(file_box(&h), "the open dialog gets the file");
        } else {
            assert!(!file_box(&h), "a closed dialog's pick is dropped");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_browser_insert_that_arrives_after_switching_documents_changes_neither() {
    // Issue #167: insert into A, open C before the file is read: the pages went into C.
    let mut h = harness(&[3, 1]);
    h.state_mut().active = Some(0);
    assert!(h.execute("page.insert"));
    h.steps(2);
    h.state_mut().active = Some(1);
    h.answer(Picked::Contents(vec![("b.pdf".into(), fixture(1))]));
    assert_eq!(pages(h.state(), 0), 3, "A is unchanged");
    assert_eq!(pages(h.state(), 1), 1, "C is unchanged");
    assert!(support::shown_notice(&h).is_some(), "the user is told why nothing happened");
}

#[test]
fn replace_pages_applies_only_to_the_document_its_dialog_was_opened_on() {
    let dir = scratch("replace");
    let file = dir.join("r.pdf");
    std::fs::write(&file, fixture(1)).unwrap();
    for switch in [false, true] {
        let mut h = harness(&[3, 3]);
        h.state_mut().active = Some(0);
        assert!(h.execute("page.replace"));
        h.steps(2);
        h.answer(Picked::Paths(vec![file.clone()]));
        h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Replace Pages");
        if switch {
            // A file opened meanwhile (a browser read finishing) becomes the active document.
            h.state_mut().active = Some(1);
            h.steps(2);
        }
        press(&mut h, "OK");
        if switch {
            assert!(!dirty(h.state(), 0) && !dirty(h.state(), 1), "neither document is changed");
            assert!(support::shown_notice(&h).is_some(), "the user is told why");
        } else {
            assert!(dirty(h.state(), 0) && !dirty(h.state(), 1), "the first document's page is replaced");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file that fails to arrive usably (the browser's `?file=` URL answering with an error page)
/// is reported in the app on the next frame, not only in the console (#173).
#[test]
fn a_failed_startup_url_is_reported_in_the_app() {
    let mut h = harness(&[]);
    h.deliver("missing.pdf", b"HTTP 404".to_vec());
    h.steps(2);
    assert!(support::shown_notice(&h).is_some(), "the failure is reported");
    assert!(h.state().views.is_empty(), "no document was opened");
}

#[test]
fn a_startup_url_opens_in_front_when_the_user_has_not_started_working() {
    let mut h = harness(&[]);
    h.deliver("startup.pdf", fixture(2));
    h.steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(pages(h.state(), 0), 2);
    h.steps(2);
    assert_eq!(h.state().views.len(), 1, "the arrival opens once");
}

#[test]
fn a_late_startup_url_preserves_the_current_edit_workspace_and_save_target() {
    use pdfcraft_engine::{Edit, InitialView, Magnification, Navigation, Session};
    use pdfcraft_ui_egui::{LeftPanel, Mode, QuickTool, RightPanel};

    // A downloaded PDF can ask to show a different panel and magnification. Its own view
    // should honour those settings without taking the user's workspace away.
    let mut s = Session::new();
    let id = s.open("late.pdf", None, std::sync::Arc::new(fixture(1)), None).unwrap();
    let initial = InitialView { navigation: Navigation::Bookmarks, magnification: Magnification::Percent(175.0), ..Default::default() };
    s.apply(id, Edit::SetInitialView(Box::new(initial))).unwrap();
    let late = s.save_bytes(id).unwrap().as_ref().clone();

    let mut h = harness(&[3]);
    let active = h.state().active_ids().unwrap().1;
    let ctx_h = h.ctx.clone();
    h.state_mut().apply_edit(&ctx_h, Edit::DeletePages { pages: vec![1] });
    h.state_mut().mode = Mode::Edit;
    h.set_option("panel", "pages").unwrap();
    h.state_mut().left = LeftPanel::AllTools;
    h.state_mut().set_left_open(false);
    h.state_mut().quick_tool = QuickTool::Hand;
    h.deliver("late.pdf", late);
    h.steps(2);

    assert_eq!(h.state().active_ids().unwrap().1, active, "Save still targets the user's document");
    assert_eq!(h.state().mode, Mode::Edit);
    assert_eq!(crate::support::right(h.state()), Some(RightPanel::Pages));
    assert_eq!(h.state().left, LeftPanel::AllTools);
    assert!(!h.state().left_open());
    assert_eq!(h.state().quick_tool, QuickTool::Hand);
    assert!(dirty(h.state(), 0));
    assert_eq!(h.state().views.len(), 2, "the startup file remains available in its own tab");
    assert_eq!(h.state().views[1].zoom(), 1.75);

    let dir = scratch("startup-save");
    let path = dir.join("saved.pdf");
    assert!(h.execute("file.save"));
    h.steps(2);
    h.answer(Picked::Paths(vec![path.clone()]));
    let saved = PageRenderer::new(std::sync::Arc::new(h.files.file(&path).unwrap()), Default::default());
    assert_eq!(saved.page_count(), 2, "the saved file contains the local deletion");
    assert!(!dirty(h.state(), 0), "the local document was saved, not the startup document");

    h.get_by_label("late.pdf").click();
    h.steps(2);
    assert_eq!(h.state().active, Some(1), "the user can choose the downloaded tab");
    h.get_by_label("saved.pdf").click();
    h.steps(2);
    assert_eq!(pages(h.state(), 0), 2, "the edit survives switching back");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_late_startup_url_preserves_home_even_after_all_other_tabs_are_closed() {
    for close in [false, true] {
        let mut h = harness(&[3]);
        if close {
            h.state_mut().close_tab(0);
        } else {
            h.get_by_label("Home").click();
            h.steps(2);
        }
        assert_eq!(h.state().active, None);
        h.deliver("late.pdf", fixture(1));
        h.steps(2);
        assert_eq!(h.state().active, None, "Home stays selected (closed tabs: {close})");
        h.get_by_label_contains("Welcome to PdfCraft");
        h.get_by_label("late.pdf").click();
        h.steps(2);
        assert!(h.state().active.is_some());
    }
}

#[test]
fn deliberate_browser_file_opens_still_activate_the_new_tab() {
    let dir = scratch("deliberate");
    let picked = dir.join("picked.pdf");
    std::fs::write(&picked, fixture(2)).unwrap();
    let mut h = harness(&[3]);
    // The user's own Open, answered on the same frame the startup file arrives.
    assert!(h.execute("file.open"));
    h.deliver("late.pdf", fixture(1));
    h.answer(Picked::Paths(vec![picked]));
    settle(&mut h, "both files", |app| app.views.len() == 3);
    assert_eq!(h.state().active, Some(1), "a deliberate Open wins even on the same frame");
    assert_eq!(pages(h.state(), 1), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_a_file_picker_supersedes_the_startup_url_even_if_the_picker_is_cancelled() {
    let mut h = harness(&[]);
    h.execute("file.open");
    h.deliver("late.pdf", fixture(1));
    h.answer(Picked::Cancelled);
    assert_eq!(h.state().active, None, "the startup file doesn't replace the user's newer Open action");
    assert_eq!(h.state().views.len(), 1, "it is still available as a background tab");
}

#[test]
fn a_broken_late_startup_file_reports_its_error_without_changing_the_active_document() {
    let mut h = harness(&[3]);
    h.deliver("broken.pdf", b"not a PDF".to_vec());
    h.steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 1);
    let notice = support::shown_notice(&h).expect("the failure is reported");
    assert!(notice.contains("broken.pdf"), "{notice}");
}

fn encrypted_fixture() -> Vec<u8> {
    use pdfcraft_engine::{Edit, Protection, Session};
    let mut s = Session::new();
    let id = s.open("encrypted.pdf", None, std::sync::Arc::new(fixture(1)), None).unwrap();
    s.apply(id, Edit::Protect(Protection { open_password: Some("test-password".into()), ..Default::default() })).unwrap();
    s.save_bytes(id).unwrap().as_ref().clone()
}

#[test]
fn a_startup_url_does_not_replace_another_documents_password_prompt() {
    for cancel in [false, true] {
        let mut h = harness(&[]);
        let local_bytes = encrypted_fixture();
        let late_bytes = fixture(2);
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, "local-encrypted.pdf", None, local_bytes.clone()).unwrap();
        h.deliver("late.pdf", late_bytes.clone());
        h.steps(2);
        assert_eq!(h.state().password_prompt.as_ref().unwrap().name, "local-encrypted.pdf");
        assert!(h.state().views.is_empty());
        assert!(h.query_by_label("late.pdf").is_none(), "the startup file waits for the prompt");
        h.submit_password((!cancel).then(|| "test-password".into()));
        settle(&mut h, "local password completion and startup open", |app| {
            app.password_prompt.is_none() && app.views.len() == if cancel { 1 } else { 2 }
        });
        assert!(h.state().password_prompt.is_none());
        let app = h.state();
        let late: Vec<_> = app.session.docs().iter().filter(|doc| doc.name == "late.pdf").collect();
        assert_eq!(late.len(), 1);
        assert_eq!(late[0].bytes.as_slice(), late_bytes.as_slice());
        assert_eq!(late[0].info.pages.len(), 2);
        if cancel {
            assert_eq!(app.active, None);
            assert_eq!(app.session.docs().len(), 1);
            assert!(!app.session.docs().iter().any(|doc| doc.name == "local-encrypted.pdf"));
        } else {
            let (_, id) = app.active_ids().expect("the authenticated local document is active");
            let local = app.session.get(id).unwrap();
            assert_eq!(local.name, "local-encrypted.pdf");
            assert_eq!(local.bytes.as_slice(), local_bytes.as_slice());
            assert_eq!(local.info.pages.len(), 1);
            assert_eq!(app.session.docs().len(), 2);
        }
        assert_eq!(h.state().views.len(), if cancel { 1 } else { 2 });
    }
}

#[test]
fn unlocking_a_late_startup_url_keeps_it_in_the_background() {
    let mut h = harness(&[3]);
    let original = h.state().views[0].id;
    let late_bytes = encrypted_fixture();
    h.deliver("late-encrypted.pdf", late_bytes.clone());
    h.steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().password_prompt.as_ref().unwrap().name, "late-encrypted.pdf");
    h.submit_password(Some("wrong-password".into()));
    settle(&mut h, "startup wrong-password reply", |app| {
        app.password_prompt.as_ref().is_some_and(|p| p.name == "late-encrypted.pdf" && p.error.as_deref() == Some("Incorrect password. Try again."))
    });
    assert!(h.state().password_prompt.as_ref().unwrap().error.is_some());
    assert_eq!(h.state().active, Some(0));
    h.submit_password(Some("test-password".into()));
    settle(&mut h, "unlocked startup document", |app| app.password_prompt.is_none() && app.views.len() == 2);
    assert!(h.state().password_prompt.is_none());
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(original));
    assert_eq!(h.state().session.get(original).unwrap().bytes.as_slice(), fixture(3).as_slice());
    let late = h.state().session.docs().iter().find(|doc| doc.name == "late-encrypted.pdf").unwrap();
    assert_eq!(late.bytes.as_slice(), late_bytes.as_slice());
    h.get_by_label("late-encrypted.pdf").click();
    h.steps(2);
    assert_eq!(h.state().active, Some(1));
}

#[test]
fn a_late_startup_url_preserves_the_combine_files_tab() {
    let mut h = harness(&[]);
    h.state_mut().open_combine_tab();
    h.steps(2);
    h.deliver("late.pdf", fixture(1));
    h.steps(2);
    assert!(h.state().combine_showing());
    assert_eq!(h.state().active, None);
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn a_late_startup_url_waits_for_an_existing_dialog_to_close() {
    let mut h = harness(&[3]);
    assert!(h.execute("app.preferences"));
    h.steps(2);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Dialog, "Preferences").is_some(), "the dialog is open");
    h.deliver("late.pdf", fixture(1));
    h.steps(2);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Dialog, "Preferences").is_some(), "the dialog is still open");
    assert_eq!(h.state().views.len(), 1);
    h.key_press(egui::Key::Escape);
    h.steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 2);
}

#[test]
fn a_late_startup_text_file_is_still_converted_without_stealing_focus() {
    let mut h = harness(&[3]);
    h.deliver("late.txt", b"A downloaded text file".to_vec());
    h.steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(pages(h.state(), 1), 1);
}

#[test]
fn a_background_startup_forms_script_notice_waits_until_its_tab_is_selected() {
    let template = pdfcraft_xfa::fixtures::scripted_template()
        .replace("if (qty.rawValue === null) qty.rawValue = 2;", r#"xfa.host.messageBox("Late form opened");"#);
    let mut h = harness(&[3]);
    h.deliver("late-form.pdf", pdfcraft_xfa::fixtures::shell(&template));
    h.steps(2);
    assert_eq!(h.state().active, Some(0));
    assert!(support::shown_notice(&h).is_none(), "the background form must not interrupt the current document");
    h.get_by_label("late-form.pdf").click();
    h.steps(2);
    let notice = support::shown_notice(&h).expect("the form's message shows once its tab is selected");
    assert!(notice.contains("Late form opened"), "{notice}");
}

/// A form opened behind the current document that turns field highlighting on gets it at
/// once, and so does the next document: only requests that act on the current document (a
/// print, a save, a message) wait for its tab.
#[test]
fn a_background_forms_highlight_request_applies_at_once() {
    let mut h = harness(&[3]);
    // Highlighting is on by default: start from a user who turned it off.
    h.state_mut().apply_settings(&pdfcraft_ui_egui::settings::import_json(r#"{"highlight_fields": false}"#).0);
    h.state_mut().views[0].session.highlight_fields = false;
    h.steps(1);
    h.deliver("late-form.pdf", pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::layout_ready_template()));
    h.steps(2);
    assert_eq!(h.state().active, Some(0), "it opened behind the current document");
    assert!(h.state().views[1].session.highlight_fields, "the background form is highlighted");
    assert!(!h.state().views[0].session.highlight_fields, "the current document is left as it was");
    assert_eq!(h.state().settings_snapshot().behavior.highlight_fields, Some(true), "the next document opens highlighted");
}

/// Embedded bytes and the PDF's original /UF name stay independent of the Save suggestion.
fn attachment_fixture() -> Vec<u8> {
    attachment_pdf("invoice\u{202e}fdp.exe", b"hello notes")
}

/// A one-page PDF with one embedded file whose /UF name is `name`.
fn attachment_pdf(name: &str, data: &[u8]) -> Vec<u8> {
    let uf = std::iter::once(0xfeffu16).chain(name.encode_utf16()).map(|c| format!("{c:04X}")).collect::<String>();
    let objs: [Vec<u8>; 5] = [
        b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(notes.txt) 5 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>".to_vec(),
        [format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n", data.len()).as_bytes(), data, b"\nendstream"].concat(),
        format!("<< /Type /Filespec /F (notes.txt) /UF <{uf}> /EF << /F 4 0 R >> >>").into_bytes(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

struct AttachmentDir(std::path::PathBuf);

impl AttachmentDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let path = std::env::temp_dir().join(format!("pdfcraft-attachment-save-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("creating attachment Save test directory: {error}"),
            }
        }
        panic!("no unused attachment Save test directory after 128 attempts");
    }
}

impl Drop for AttachmentDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn attachment_save_preserves_bytes_metadata_and_selected_destination() {
    if crate::support::skip_without_glyphs("attachment_save_preserves_bytes_metadata_and_selected_destination", "en", "user chosen 資料.bin") {
        return;
    }
    let dir = AttachmentDir::new();
    let chosen = dir.0.join("user chosen 資料.bin");
    let original = attachment_fixture();
    let mut h = harness(&[]);
    let ctx_h = h.ctx.clone();
    h.state_mut().open_bytes(&ctx_h, "attachment.pdf", None, original.clone()).unwrap();
    h.steps(2);
    let id = h.state().views[0].id;
    let raw_name = "invoice\u{202e}fdp.exe";
    let index = h.state().session.get(id).unwrap().info.attachments.iter().position(|a| a.name == raw_name).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().attachment_action(&ctx_h, id, index, false);
    h.answer(Picked::Paths(vec![chosen.clone()]));
    assert_eq!(h.files.file(&chosen).as_deref(), Some(b"hello notes".as_slice()));
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.info.attachments[index].name, raw_name);
    assert_eq!(doc.bytes.as_ref(), &original);
    assert_eq!(h.files.written(), vec![chosen.clone()], "only the chosen destination was written");
    let ctx_h = h.ctx.clone();
    h.state_mut().attachment_action(&ctx_h, id, index, false);
    h.answer(Picked::Cancelled);
    assert_eq!(h.files.written(), vec![chosen], "cancel did not write anything");
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0, "the scripted file service must not fall back to native writes");
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.info.attachments[index].name, raw_name);
    assert_eq!(doc.bytes.as_ref(), &original);
}

/// Windows: a saved attachment keeps the Mark-of-the-Web of the PDF it came from, and gets the
/// Internet zone when that PDF has none or isn't a file (#3990).
#[cfg(windows)]
#[test]
fn a_saved_attachment_carries_the_documents_zone() {
    let dir = AttachmentDir::new();
    let zone_of = |p: &std::path::Path| std::fs::read(format!("{}:Zone.Identifier", p.display())).ok();
    let mail = dir.0.join("from mail.pdf");
    std::fs::write(&mail, attachment_fixture()).unwrap();
    let zone = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://mail.example/invoice.pdf\r\n";
    std::fs::write(format!("{}:Zone.Identifier", mail.display()), zone).unwrap();
    let local = dir.0.join("local.pdf");
    std::fs::write(&local, attachment_fixture()).unwrap();
    // Hold the native picker at the host boundary but save through the real filesystem,
    // so the production callback can attach an actual NTFS alternate data stream.
    struct DiskWrites(Arc<ScriptedFiles>);
    impl craft::host::Files for DiskWrites {
        fn pick(&self, request: craft::host::PickRequest) -> craft::host::PickId {
            self.0.pick(request)
        }

        fn save(&self, request: craft::host::PickRequest, bytes: Vec<u8>) -> craft::host::PickId {
            self.0.save(request, bytes)
        }

        fn can_pick(&self) -> bool {
            self.0.can_pick()
        }

        fn browser_saves(&self) -> bool {
            self.0.browser_saves()
        }

        fn take_answer(&self, id: craft::host::PickId) -> Option<Picked> {
            self.0.take_answer(id)
        }

        fn forget(&self, id: craft::host::PickId) {
            self.0.forget(id);
        }

        fn pick_pending(&self) -> bool {
            self.0.pick_pending()
        }

        fn read(&self, path: &std::path::Path) -> Result<Vec<u8>, craft::host::FileError> {
            craft::host::NoPickFiles::default().read(path)
        }

        fn write(&self, path: &std::path::Path, bytes: &[u8], intent: craft::host::WriteIntent) -> Result<bool, craft::host::FileError> {
            craft::host::NoPickFiles::default().write(path, bytes, intent)
        }

        fn last_folder(&self, purpose: &str) -> Option<std::path::PathBuf> {
            self.0.last_folder(purpose)
        }
    }

    let files = Arc::new(ScriptedFiles::held());
    let mut h = rig_with_file_service(&[], files.clone(), Arc::new(DiskWrites(files)));
    let ctx = h.ctx.clone();
    let opened = [(Some(&mail), zone), (Some(&local), pdfcraft_platform::zone::INTERNET_ZONE), (None, pdfcraft_platform::zone::INTERNET_ZONE)];
    for (i, (path, expected)) in opened.into_iter().enumerate() {
        h.state_mut().open_bytes(&ctx, &format!("doc{i}.pdf"), path.map(|p| p.to_string_lossy().into_owned()), attachment_fixture()).unwrap();
        h.steps(2);
        let id = h.state().views.last().unwrap().id;
        let chosen = dir.0.join(format!("saved{i}.exe"));
        h.state_mut().attachment_action(&ctx, id, 0, false);
        h.answer(Picked::Paths(vec![chosen.clone()]));
        assert_eq!(std::fs::read(&chosen).unwrap(), b"hello notes");
        assert_eq!(zone_of(&chosen).as_deref(), Some(expected.as_bytes()), "document {i}");
    }
}

/// Every file the host's file service wrote anywhere, once each, sorted: the writes go through it, not to disk.
fn files_written(h: &Rig) -> Vec<std::path::PathBuf> {
    let mut out = h.files.written();
    out.sort();
    out.dedup();
    out
}

#[test]
fn files_written_from_an_attachment_tab_stay_inside_the_chosen_folder() {
    let dir = AttachmentDir::new();
    let absolute = dir.0.join("elsewhere").join("x.pdf").to_string_lossy().into_owned();
    for name in ["../../x.pdf", absolute.as_str()] {
        let out = dir.0.join("a").join("b").join("out");
        std::fs::create_dir_all(&out).unwrap();
        let mut h = harness(&[]);
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, "outer.pdf", None, attachment_pdf(name, &fixture(2))).unwrap();
        h.steps(2);
        let id = h.state().views[0].id;
        let ctx_h = h.ctx.clone();
        h.state_mut().attachment_action(&ctx_h, id, 0, true);
        h.steps(2);
        assert_eq!(h.state().views.len(), 2, "{name}: the attachment opens as a tab");
        let ctx_h = h.ctx.clone();
        h.state_mut().split_active(&ctx_h, &pdfcraft_ui_egui::SplitPlan::By(pdfcraft_engine::SplitBy::PageCount(1)));
        h.answer(Picked::Paths(vec![out.clone()]));
        let written = files_written(&h);
        assert_eq!(written.len(), 2, "{name}: two split parts, got {written:?}");
        assert!(written.iter().all(|p| p.parent() == Some(out.as_path())), "{name}: {written:?}");
        // Extract writes page 1 under the name Split gave it.
        open_extract(&mut h, Extract { separate: true, ..Default::default() });
        confirm_extract(&mut h);
        h.answer(Picked::Paths(vec![out.clone()]));
        assert!(support::shown_notice(&h).is_some(), "{name}: Extract reports what it wrote");
        assert_eq!(files_written(&h), written, "{name}");
        std::fs::remove_dir_all(dir.0.join("a")).unwrap();
    }
}
