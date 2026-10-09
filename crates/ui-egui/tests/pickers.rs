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
