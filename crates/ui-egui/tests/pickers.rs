//! Native file pickers never block the frame. A blocking picker ran a nested run loop inside
//! winit's event handler on macOS, which crashed the app when the user clicked File ▸ Open (or
//! any other open, save or folder picker). Commands now return at once, and the chosen files are
//! used on a later frame. `pick_override` stands in for the native panel and, like it, answers
//! on a later frame.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::PageRenderer;
use pdfcraft_ui_egui::PdfCraftApp;

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

/// A shell with `docs` open, each the given number of pages.
fn harness(docs: &[usize]) -> Harness<'static, PdfCraftApp> {
    let docs = docs.to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for (i, n) in docs.iter().enumerate() {
            app.open_bytes(&format!("doc{i}.pdf"), None, fixture(*n)).unwrap();
        }
        app
    });
    h.run_steps(2);
    h
}

#[test]
fn file_open_returns_at_once_and_opens_the_pick_on_the_next_frame() {
    let dir = std::env::temp_dir().join(format!("printcraft-pickers-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("picked.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[]);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("file.open"));
    assert!(h.state().views.is_empty(), "the command must not wait for the picker");
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1, "the picked file opens on a later frame");
    assert_eq!(pages(h.state(), 0), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn insert_from_file_uses_the_pick_on_the_next_frame() {
    let dir = std::env::temp_dir().join(format!("printcraft-pickers-insert-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("more.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[3]);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("page.insert"));
    assert_eq!(pages(h.state(), 0), 3, "nothing is inserted until the pick arrives");
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pick_for_a_document_that_is_no_longer_active_is_dropped() {
    let dir = std::env::temp_dir().join(format!("printcraft-pickers-switch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("more.pdf");
    std::fs::write(&file, fixture(2)).unwrap();

    let mut h = harness(&[3, 4]);
    h.state_mut().active = Some(0);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("page.insert"));
    // The user switches tabs before the pick arrives.
    h.state_mut().active = Some(1);
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 3, "the document the pick was for is unchanged");
    assert_eq!(pages(h.state(), 1), 4, "the newly active document is unchanged");
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("document changed"), "the user is told why nothing happened: {toast:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scratch folder for one test.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-pickers-{name}-{}", std::process::id()));
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
    app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
}

#[test]
fn save_as_writes_on_a_later_frame_including_edits_made_meanwhile() {
    let dir = scratch("save-as");
    let out = dir.join("saved.pdf");
    let mut h = harness(&[3]);
    h.state_mut().pick_override = Some(vec![out.to_string_lossy().into_owned()]);
    assert!(h.state_mut().execute("file.save_as"));
    assert!(!out.exists(), "the command must not wait for the picker");
    // The user keeps working while the panel is open.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    h.run_steps(2);
    assert_eq!(page_count(std::fs::read(&out).unwrap()), 2, "the edit made while choosing is saved too");
    assert!(!dirty(h.state(), 0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_on_close_closes_the_tab_only_once_the_file_is_written() {
    let dir = scratch("save-close");
    let out = dir.join("closed.pdf");
    let mut h = harness(&[2]);
    rotate_first_page(h.state_mut());
    h.state_mut().request_close_tab(0);
    assert!(h.state().close_request.is_some(), "a dirty document asks first");
    h.state_mut().pick_override = Some(vec![out.to_string_lossy().into_owned()]);
    let ctx = h.ctx.clone();
    h.state_mut().resolve_close(&ctx, Some(true));
    assert_eq!(h.state().views.len(), 1, "the tab stays open until the save is written");
    h.run_steps(2);
    assert!(out.exists());
    assert!(h.state().views.is_empty(), "then it closes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_save_on_close_keeps_the_tab_open() {
    let mut h = harness(&[2]);
    rotate_first_page(h.state_mut());
    h.state_mut().request_close_tab(0);
    h.state_mut().pick_override = Some(vec![]); // the user cancels the save panel
    let ctx = h.ctx.clone();
    h.state_mut().resolve_close(&ctx, Some(true));
    h.run_steps(2);
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
    h.state_mut().close_all();
    let ctx = h.ctx.clone();
    for (n, name) in ["first.pdf", "second.pdf"].iter().enumerate() {
        assert!(h.state().close_request.is_some(), "asks about document {n}");
        h.state_mut().pick_override = Some(vec![dir.join(name).to_string_lossy().into_owned()]);
        h.state_mut().resolve_close(&ctx, Some(true));
        h.run_steps(2);
    }
    assert!(h.state().views.is_empty());
    assert_eq!(page_count(std::fs::read(dir.join("first.pdf")).unwrap()), 2);
    assert_eq!(page_count(std::fs::read(dir.join("second.pdf")).unwrap()), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_and_delete_deletes_the_pages_only_after_their_files_are_written() {
    let dir = scratch("extract");
    let mut h = harness(&[3]);
    h.state_mut().extract_draft.separate = true;
    h.state_mut().extract_draft.delete = true;
    h.state_mut().pick_override = Some(vec![dir.to_string_lossy().into_owned()]);
    h.state_mut().extract_selection();
    assert_eq!(pages(h.state(), 0), 3, "nothing is deleted while the folder picker is open");
    h.run_steps(2);
    assert!(dir.join("doc0 (page 1).pdf").exists());
    assert_eq!(pages(h.state(), 0), 2, "the extracted page is deleted once written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_folder_pick_extracts_and_deletes_nothing() {
    let mut h = harness(&[3]);
    h.state_mut().extract_draft.separate = true;
    h.state_mut().extract_draft.delete = true;
    h.state_mut().pick_override = Some(vec![]);
    h.state_mut().extract_selection();
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 3);
}

#[test]
fn an_image_pick_for_a_document_that_is_no_longer_active_changes_nothing() {
    let dir = scratch("image-switch");
    let file = dir.join("picture.png");
    std::fs::write(&file, b"not really a png").unwrap();
    let mut h = harness(&[1, 1]);
    h.state_mut().active = Some(0);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    h.state_mut().add_image_dialog();
    h.state_mut().active = Some(1);
    h.run_steps(2);
    assert!(!dirty(h.state(), 0) && !dirty(h.state(), 1), "neither document changed");
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("document changed"), "the user is told why nothing happened: {toast:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_comments_writes_on_a_later_frame() {
    let dir = scratch("export-data");
    let out = dir.join("data.xfdf");
    let mut h = harness(&[1]);
    h.state_mut().pick_override = Some(vec![out.to_string_lossy().into_owned()]);
    h.state_mut().export_data_dialog(true, false);
    assert!(!out.exists(), "the command must not wait for the picker");
    h.run_steps(2);
    assert!(std::fs::read_to_string(&out).unwrap().contains("xfdf"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extracted_pages_are_not_deleted_if_the_document_was_edited_while_choosing_the_folder() {
    let dir = scratch("extract-edited");
    let mut h = harness(&[3]);
    h.state_mut().extract_draft.separate = true;
    h.state_mut().extract_draft.delete = true;
    h.state_mut().pick_override = Some(vec![dir.to_string_lossy().into_owned()]);
    h.state_mut().extract_selection();
    // While the folder picker is open the user deletes page 1 themselves: the page that would
    // now be deleted is the original page 2, which was never written out.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    h.run_steps(2);
    assert!(dir.join("doc0 (page 1).pdf").exists(), "the extracted file is still written");
    assert_eq!(pages(h.state(), 0), 2, "nothing more is deleted");
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("not deleted"), "the user is told: {toast:?}");
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
    h.state_mut().request_close_tab(0);
    h.state_mut().pick_override = Some(vec![dir.join("a.pdf").to_string_lossy().into_owned()]);
    let ctx = h.ctx.clone();
    h.state_mut().resolve_close(&ctx, Some(true));
    // ...when the user asks to close the second one.
    h.state_mut().request_close_tab(1);
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 2, "the first tab closed once saved");
    // Don't Save: the prompt was about the second document, which is now the first tab.
    h.state_mut().resolve_close(&ctx, Some(false));
    let open: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    assert_eq!(open, vec![c], "the second document closed, not the third: {b:?} vs {c:?}");
    assert!(dirty(h.state(), 0), "the third document keeps its unsaved changes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_image_pick_is_dropped_if_the_document_was_edited_meanwhile() {
    let dir = scratch("image-edited");
    let file = dir.join("picture.png");
    std::fs::write(&file, b"not really a png").unwrap();
    let mut h = harness(&[2]);
    h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
    // Replace image 0 on page 2...
    h.state_mut().replace_image_dialog(1, 0);
    // ...but page 1 is deleted while choosing: "page 2" is now a different page.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 1);
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("document changed"), "the pick is dropped and the user told why: {toast:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_watermark_file_from_browse_lands_only_in_the_dialog_that_asked() {
    let dir = scratch("browse");
    let file = dir.join("mark.pdf");
    std::fs::write(&file, fixture(1)).unwrap();
    for keep_open in [true, false] {
        let mut h = harness(&[1]);
        assert!(h.state_mut().execute("edit.watermark"));
        h.state_mut().marks_draft.use_file = true;
        h.state_mut().pick_override = Some(vec![file.to_string_lossy().into_owned()]);
        h.run_steps(2);
        h.get_by_label("Browse…").click();
        h.run_steps(1);
        if !keep_open {
            h.state_mut().dialog = None; // closed before the pick arrives
        }
        h.run_steps(2);
        let got = h.state().marks_draft.file.as_ref().map(|f| f.0.clone());
        if keep_open {
            assert_eq!(got.as_deref(), Some("mark.pdf"), "the open dialog gets the file");
        } else {
            assert_eq!(got, None, "a closed dialog's pick is dropped");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Browsers read picked files asynchronously and queue them on `requests` (#167): the request
/// is made when the picker opens, so it is bound to the document active then.
fn browser_insert(h: &Harness<'static, PdfCraftApp>, pages: usize) -> pdfcraft_ui_egui::FileRequest {
    h.state().file_request(pdfcraft_ui_egui::FilePurpose::InsertPages, vec![("b.pdf".into(), fixture(pages))])
}

#[test]
fn a_browser_insert_goes_into_the_document_it_was_started_on() {
    let mut h = harness(&[3, 1]);
    h.state_mut().active = Some(0);
    let request = browser_insert(&h, 1);
    h.state_mut().requests.lock().unwrap().push(request);
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 4);
    assert_eq!(pages(h.state(), 1), 1);
}

#[test]
fn a_browser_insert_that_arrives_after_switching_documents_changes_neither() {
    // Issue #167: insert into A, open C before the file is read: the pages went into C.
    let mut h = harness(&[3, 1]);
    h.state_mut().active = Some(0);
    let request = browser_insert(&h, 1);
    h.state_mut().active = Some(1);
    h.state_mut().requests.lock().unwrap().push(request);
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 3, "A is unchanged");
    assert_eq!(pages(h.state(), 1), 1, "C is unchanged");
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("document changed"), "the user is told why nothing happened: {toast:?}");
}

#[test]
fn a_browser_insert_into_a_document_edited_meanwhile_is_refused() {
    let mut h = harness(&[3]);
    let request = browser_insert(&h, 1);
    h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    h.state_mut().requests.lock().unwrap().push(request);
    h.run_steps(2);
    assert_eq!(pages(h.state(), 0), 2, "only the user's own edit applied");
}

#[test]
fn replace_pages_applies_only_to_the_document_its_dialog_was_opened_on() {
    for switch in [false, true] {
        let mut h = harness(&[3, 3]);
        h.state_mut().active = Some(0);
        h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::ReplacePages, vec![("r.pdf".into(), fixture(1))]);
        h.run_steps(2);
        assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::ReplacePages));
        if switch {
            // A file opened meanwhile (a browser read finishing) becomes the active document.
            h.state_mut().active = Some(1);
            h.run_steps(2);
        }
        h.get_by_label("OK").click();
        h.run_steps(2);
        if switch {
            assert!(!dirty(h.state(), 0) && !dirty(h.state(), 1), "neither document is changed");
            let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
            assert!(toast.contains("document changed"), "the user is told why: {toast:?}");
        } else {
            assert!(dirty(h.state(), 0) && !dirty(h.state(), 1), "the first document's page is replaced");
        }
    }
}

/// A file that fails to arrive asynchronously (the browser's `?file=` URL answering HTTP 404) is
/// reported in the app on the next frame, not only in the console (#173).
#[test]
fn a_failed_startup_url_is_reported_in_the_app() {
    let mut h = harness(&[]);
    let failed = h.state().failed_inbox.clone();
    failed.lock().unwrap().push(("missing.pdf".into(), "HTTP 404".into()));
    h.run_steps(2);
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("missing.pdf") && toast.contains("HTTP 404"), "{toast:?}");
    assert!(h.state().views.is_empty(), "no document was opened");
    assert!(failed.lock().unwrap().is_empty(), "each failure is reported once");
}

fn deliver_startup(h: &Harness<'static, PdfCraftApp>, name: &str, bytes: Vec<u8>) {
    h.state().startup_inbox.lock().unwrap().push((name.into(), bytes));
}

#[test]
fn a_startup_url_opens_in_front_when_the_user_has_not_started_working() {
    let mut h = harness(&[]);
    deliver_startup(&h, "startup.pdf", fixture(2));
    h.run_steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(pages(h.state(), 0), 2);
    assert!(h.state().startup_inbox.lock().unwrap().is_empty());
    h.run_steps(2);
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
    h.state_mut().apply_edit(Edit::DeletePages { pages: vec![1] });
    h.state_mut().mode = Mode::Edit;
    h.state_mut().right = Some(RightPanel::Pages);
    h.state_mut().left = LeftPanel::AllTools;
    h.state_mut().left_open = false;
    h.state_mut().quick_tool = QuickTool::Hand;
    deliver_startup(&h, "late.pdf", late);
    h.run_steps(2);

    assert_eq!(h.state().active_ids().unwrap().1, active, "Save still targets the user's document");
    assert_eq!(h.state().mode, Mode::Edit);
    assert_eq!(h.state().right, Some(RightPanel::Pages));
    assert_eq!(h.state().left, LeftPanel::AllTools);
    assert!(!h.state().left_open);
    assert_eq!(h.state().quick_tool, QuickTool::Hand);
    assert!(dirty(h.state(), 0));
    assert_eq!(h.state().views.len(), 2, "the startup file remains available in its own tab");
    assert_eq!(h.state().views[1].zoom, 1.75);

    let dir = scratch("startup-save");
    let path = dir.join("saved.pdf");
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    assert!(h.state_mut().execute("file.save"));
    h.run_steps(2);
    let saved = PageRenderer::new(std::sync::Arc::new(std::fs::read(&path).unwrap()), Default::default());
    assert_eq!(saved.page_count(), 2, "the saved file contains the local deletion");
    assert!(!dirty(h.state(), 0), "the local document was saved, not the startup document");

    h.get_by_label("late.pdf").click();
    h.run_steps(2);
    assert_eq!(h.state().active, Some(1), "the user can choose the downloaded tab");
    h.get_by_label("saved.pdf").click();
    h.run_steps(2);
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
            h.run_steps(2);
        }
        assert_eq!(h.state().active, None);
        deliver_startup(&h, "late.pdf", fixture(1));
        h.run_steps(2);
        assert_eq!(h.state().active, None, "Home stays selected (closed tabs: {close})");
        h.get_by_label_contains("Welcome to PdfCraft");
        h.get_by_label("late.pdf").click();
        h.run_steps(2);
        assert!(h.state().active.is_some());
    }
}

#[test]
fn deliberate_browser_file_opens_still_activate_the_new_tab() {
    let mut h = harness(&[3]);
    h.state().inbox.lock().unwrap().push(("picked.pdf".into(), fixture(2)));
    deliver_startup(&h, "late.pdf", fixture(1));
    h.run_steps(2);
    assert_eq!(h.state().active, Some(1), "a deliberate Open wins even on the same frame");
    assert_eq!(pages(h.state(), 1), 2);
    assert_eq!(h.state().views.len(), 3);
}

#[test]
fn opening_a_file_picker_supersedes_the_startup_url_even_if_the_picker_is_cancelled() {
    let mut h = harness(&[]);
    h.state_mut().pick_override = Some(vec![]);
    h.state_mut().execute("file.open");
    deliver_startup(&h, "late.pdf", fixture(1));
    h.run_steps(2);
    assert_eq!(h.state().active, None, "the startup file doesn't replace the user's newer Open action");
    assert_eq!(h.state().views.len(), 1, "it is still available as a background tab");
}

#[test]
fn a_broken_late_startup_file_reports_its_error_without_changing_the_active_document() {
    let mut h = harness(&[3]);
    deliver_startup(&h, "broken.pdf", b"not a PDF".to_vec());
    h.run_steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 1);
    let toast = &h.state().toast.as_ref().unwrap().0;
    assert!(toast.contains("broken.pdf"), "{toast}");
    assert!(h.state().startup_inbox.lock().unwrap().is_empty());
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
        h.state_mut().open_bytes("local-encrypted.pdf", None, encrypted_fixture()).unwrap();
        deliver_startup(&h, "late.pdf", fixture(2));
        h.run_steps(2);
        assert_eq!(h.state().password_prompt.as_ref().unwrap().name, "local-encrypted.pdf");
        assert!(h.state().views.is_empty());
        assert_eq!(h.state().startup_inbox.lock().unwrap().len(), 1, "startup bytes wait for the prompt");
        h.state_mut().submit_password((!cancel).then(|| "test-password".into()));
        h.run_steps(2);
        assert!(h.state().password_prompt.is_none());
        assert!(h.state().startup_inbox.lock().unwrap().is_empty());
        assert_eq!(h.state().active, if cancel { None } else { Some(0) });
        assert_eq!(h.state().views.len(), if cancel { 1 } else { 2 });
    }
}

#[test]
fn unlocking_a_late_startup_url_keeps_it_in_the_background() {
    let mut h = harness(&[3]);
    deliver_startup(&h, "late-encrypted.pdf", encrypted_fixture());
    h.run_steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().password_prompt.as_ref().unwrap().name, "late-encrypted.pdf");
    h.state_mut().submit_password(Some("wrong-password".into()));
    h.run_steps(2);
    assert!(h.state().password_prompt.as_ref().unwrap().error.is_some());
    assert_eq!(h.state().active, Some(0));
    h.state_mut().submit_password(Some("test-password".into()));
    h.run_steps(2);
    assert!(h.state().password_prompt.is_none());
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 2);
    h.get_by_label("late-encrypted.pdf").click();
    h.run_steps(2);
    assert_eq!(h.state().active, Some(1));
}

#[test]
fn a_late_startup_url_preserves_the_combine_files_tab() {
    let mut h = harness(&[]);
    h.state_mut().open_combine_tab();
    h.run_steps(2);
    deliver_startup(&h, "late.pdf", fixture(1));
    h.run_steps(2);
    assert!(h.state().combine_showing());
    assert_eq!(h.state().active, None);
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn a_late_startup_url_waits_for_an_existing_dialog_to_close() {
    let mut h = harness(&[3]);
    h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::Preferences);
    deliver_startup(&h, "late.pdf", fixture(1));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::Preferences));
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(h.state().startup_inbox.lock().unwrap().len(), 1);
    h.state_mut().dialog = None;
    h.run_steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 2);
    assert!(h.state().startup_inbox.lock().unwrap().is_empty());
}

#[test]
fn a_late_startup_text_file_is_still_converted_without_stealing_focus() {
    let mut h = harness(&[3]);
    deliver_startup(&h, "late.txt", b"A downloaded text file".to_vec());
    h.run_steps(2);
    assert_eq!(h.state().active, Some(0));
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(pages(h.state(), 1), 1);
}

#[test]
fn a_background_startup_forms_script_notice_waits_until_its_tab_is_selected() {
    let template = pdfcraft_xfa::fixtures::scripted_template()
        .replace("if (qty.rawValue === null) qty.rawValue = 2;", r#"xfa.host.messageBox("Late form opened");"#);
    let mut h = harness(&[3]);
    deliver_startup(&h, "late-form.pdf", pdfcraft_xfa::fixtures::shell(&template));
    h.run_steps(2);
    assert_eq!(h.state().active, Some(0));
    assert!(h.state().toast.is_none(), "the background form must not interrupt the current document");
    h.get_by_label("late-form.pdf").click();
    h.run_steps(2);
    assert_eq!(h.state().toast.as_ref().unwrap().0, "Late form opened");
    h.state_mut().toast = None;
    h.run_steps(2);
    assert!(h.state().toast.is_none(), "the notice is delivered once");
}
