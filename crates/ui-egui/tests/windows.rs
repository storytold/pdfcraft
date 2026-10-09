//! Several views of one document: shared undo and unsaved state, labels, and what happens when
//! views close (see `plan/multi-window.md`).

use egui_kittest::Harness;
use pdfcraft_ui_egui::{PdfCraftApp, WindowId};

fn form_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
    }
    h
}

fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

fn value(app: &PdfCraftApp, name: &str) -> Vec<String> {
    let doc = app.session.get(app.views[0].id).unwrap();
    doc.form.iter().find(|f| f.name == name).map(|f| f.value.clone()).unwrap_or_default()
}

/// Assert the window invariants after a step.
fn assert_windows_ok(app: &PdfCraftApp) {
    let problems = app.debug_check_windows();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn closing_the_second_view_of_a_dirty_document_asks_nothing() {
    let mut h = form_harness();
    let doc = h.state().views[0].id;
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    let other = h.state_mut().test_add_parked_view(doc).unwrap();
    assert_eq!(h.state().view_count(doc), 2);
    h.state_mut().with_window(other, |a| a.request_close_tab(0));
    assert!(h.state().close_request.is_none(), "no question: the document stays open in the other view");
    assert_eq!(h.state().view_count(doc), 1);
    let d = h.state().session.get(doc).unwrap();
    assert!(d.dirty, "and keeps its unsaved changes");
    // The last view asks, as before.
    h.state_mut().request_close_tab(0);
    assert!(h.state().close_request.is_some());
}

#[test]
fn views_are_labelled_by_number_once_there_are_two() {
    let mut h = form_harness();
    let doc = h.state().views[0].id;
    let label = |h: &mut Harness<'static, PdfCraftApp>, w: WindowId| {
        h.state_mut().with_window(w, |a| a.views.first().and_then(|v| a.display_label(v))).flatten()
    };
    h.run_steps(2);
    assert_eq!(label(&mut h, WindowId::ROOT).as_deref(), Some("form.pdf"));
    let second = h.state_mut().test_add_parked_view(doc).unwrap();
    h.run_steps(2);
    assert_eq!(label(&mut h, WindowId::ROOT).as_deref(), Some("form.pdf:1"));
    assert_eq!(label(&mut h, second).as_deref(), Some("form.pdf:2"));
    // Closing :1 does not renumber :2.
    h.state_mut().close_tab(0);
    let third = h.state_mut().test_add_parked_view(doc).unwrap();
    h.run_steps(1);
    assert_eq!(label(&mut h, second).as_deref(), Some("form.pdf:2"));
    assert_eq!(label(&mut h, third).as_deref(), Some("form.pdf:3"));
}

#[test]
fn leaving_a_window_takes_over_what_is_typed_and_the_other_view_sees_it() {
    let mut h = form_harness();
    let doc = h.state().views[0].id;
    let other = h.state_mut().test_add_parked_view(doc).unwrap();
    click_field(&mut h, "name");
    h.event(egui::Event::Text("Grace Hopper".into()));
    h.run_steps(2);
    assert!(h.state().views[0].forms.focus.is_some(), "still typing");
    assert!(h.state_mut().with_window(other, |a| a.has_unsaved_work(0)).unwrap(), "typing in the other window is unsaved work of the document");
    h.state_mut().commit_view_inputs(0);
    assert_eq!(value(h.state(), "name"), ["Grace Hopper"]);
    assert!(h.state().views[0].forms.focus.is_none());
    h.run_steps(2);
    // The other view caught up on the change.
    let current = h.state().session.get(doc).unwrap().edit_generation();
    assert!(h.state_mut().with_window(other, |a| a.views[0].seen_edit_generation()).unwrap() >= current);
    assert_windows_ok(h.state());
}

#[test]
fn a_comment_note_being_edited_is_saved_when_the_window_is_left() {
    let mut h = form_harness();
    let edit = pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
        page: 0,
        shape: pdfcraft_engine::Shape::Rectangle { rect: [10.0, 10.0, 60.0, 60.0] },
        style: pdfcraft_engine::Style { color: [1.0, 0.0, 0.0], opacity: 1.0, width: 2.0, fill: None },
        contents: String::new(),
        author: "t".into(),
    });
    assert!(h.state_mut().apply_edit(edit));
    let index = h.state().session.get(h.state().views[0].id).unwrap().info.annotations[0].index;
    h.state_mut().views[0].comments.editing = Some((0, index, "Check this".into()));
    h.state_mut().commit_view_inputs(0);
    assert!(h.state().views[0].comments.editing.is_none());
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert_eq!(doc.info.annotations.first().and_then(|a| a.contents.as_deref()), Some("Check this"));
}

// --- child windows ---

use egui_kittest::kittest::Queryable;

fn two_docs_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("one.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        app.open_bytes("two.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn new_view_opens_second_window() {
    let mut h = form_harness();
    let doc = h.state().views[0].id;
    assert!(h.state_mut().execute("window.new_view"));
    h.run_steps(4);
    assert_windows_ok(h.state());
    assert_eq!(h.state().window_count(), 2);
    let child = h.state().window_ids()[1];
    assert_eq!(h.state().view_count(doc), 2);
    assert_eq!(h.state_mut().with_window(child, |a| a.views.first().map(|v| v.id)).flatten(), Some(doc), "the same document, not a copy");
    // Each window shows its own number, and has its own zoom.
    h.get_by_label("form.pdf:1");
    h.get_by_label("form.pdf:2");
    h.state_mut().views[0].set_zoom(2.0);
    h.state_mut().with_window(child, |a| a.views[0].set_zoom(0.5));
    h.run_steps(2);
    assert!((h.state().views[0].zoom - 2.0).abs() < 1e-3);
    assert!((h.state_mut().with_window(child, |a| a.views[0].zoom).unwrap() - 0.5).abs() < 1e-3);
    assert_windows_ok(h.state());
}

#[test]
fn the_new_window_starts_where_the_old_one_is() {
    let mut h = form_harness();
    h.state_mut().views[0].set_zoom(1.75);
    assert!(h.state_mut().execute("window.new_view"));
    h.run_steps(3);
    let child = h.state().window_ids()[1];
    let zoom = h.state_mut().with_window(child, |a| a.views[0].zoom).unwrap();
    assert!((zoom - 1.75).abs() < 1e-3, "{zoom}");
}

#[test]
fn move_tab_new_moves() {
    let mut h = two_docs_harness();
    let (one, two) = (h.state().views[0].id, h.state().views[1].id);
    h.state_mut().active = Some(1);
    assert!(h.state_mut().execute("window.move_tab_new"));
    h.run_steps(4);
    assert_windows_ok(h.state());
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(h.state().views[0].id, one);
    let child = h.state().window_ids()[1];
    assert_eq!(
        h.state_mut().with_window(child, |a| a.views.iter().map(|v| v.id).collect::<Vec<_>>()).unwrap(),
        vec![two],
        "no reload: the same DocId"
    );
    // With one tab left, the command is not available.
    assert!(!h.state_mut().execute("window.move_tab_new"));
}

#[test]
fn merge_all_dedupes_views() {
    let mut h = two_docs_harness();
    let one = h.state().views[0].id;
    h.state_mut().active = Some(0);
    assert!(h.state_mut().execute("window.new_view"));
    h.run_steps(3);
    assert!(h.state_mut().execute("window.new_view"));
    h.run_steps(3);
    assert_eq!(h.state().window_count(), 3);
    assert_eq!(h.state().view_count(one), 3);
    assert!(h.state_mut().execute("window.merge_all"));
    h.run_steps(3);
    assert_windows_ok(h.state());
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(h.state().views.len(), 2, "one tab per document");
    assert_eq!(h.state().view_count(one), 1);
}

#[test]
fn last_tab_closed_closes_child() {
    let mut h = form_harness();
    assert!(h.state_mut().execute("window.new_view"));
    h.run_steps(3);
    let child = h.state().window_ids()[1];
    h.state_mut().with_window(child, |a| a.request_close_tab(0));
    h.state_mut().pending_window_ops_for_test();
    h.run_steps(3);
    assert_eq!(h.state().window_count(), 1, "the empty window is gone");
    assert_windows_ok(h.state());
}

#[test]
fn the_window_shortcut_opens_a_window() {
    let mut h = form_harness();
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::ALT, egui::Key::N);
    h.run_steps(4);
    assert_eq!(h.state().window_count(), 2);
    assert_windows_ok(h.state());
}

#[test]
fn window_commands_know_when_they_apply() {
    let mut h = form_harness();
    assert!(!h.state_mut().execute("window.merge_all"), "one window: nothing to merge");
    assert!(!h.state_mut().execute("window.close"));
    assert!(h.state().window_count() == 1);
}

// --- the tab's context menu ---

#[test]
fn the_tab_menu_opens_a_new_window() {
    let mut h = two_docs_harness();
    h.get_by_label("one.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("In new window").click();
    h.run_steps(4);
    assert_eq!(h.state().window_count(), 2);
    assert_windows_ok(h.state());
    // The tab stays; the second view is numbered.
    assert_eq!(h.state().views.len(), 2);
    h.get_by_label("one.pdf:1");
    h.get_by_label("one.pdf:2");
}

#[test]
fn closing_other_tabs_keeps_a_document_with_unsaved_changes_and_asks() {
    let mut h = two_docs_harness();
    h.state_mut().active = Some(1);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    let dirty = h.state().views[1].id;
    h.get_by_label("one.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("Close other tabs").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 2, "it stays");
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Tab(dirty)), "and asks");
}

#[test]
fn closing_other_tabs_closes_the_clean_ones() {
    let mut h = two_docs_harness();
    h.get_by_label("one.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("Close other tabs").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1);
    assert!(h.state().close_request.is_none());
}

#[test]
fn copy_path_puts_the_path_on_the_clipboard() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-tabmenu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("menu.pdf");
    std::fs::write(&path, include_bytes!("data/form.pdf")).unwrap();
    let file = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe({
        let file = file.clone();
        move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_path(&file);
            app
        }
    });
    h.run_steps(4);
    h.get_by_label("menu.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("Copy path").click();
    // The frame that applies the choice reports the command; look at every frame.
    let mut copied: Vec<String> = Vec::new();
    for _ in 0..4 {
        h.run_steps(1);
        copied.extend(
            h.output().platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::CopyText(t) = c { Some(t.clone()) } else { None }),
        );
    }
    assert!(copied.contains(&file), "{copied:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tab_without_a_file_offers_no_path_entries() {
    let mut h = two_docs_harness();
    h.get_by_label("one.pdf").click_secondary();
    h.run_steps(2);
    assert!(h.query_by_label("Copy path").is_none());
    assert!(h.query_by_label("Merge all windows").is_none(), "one window: nothing to merge");
    h.get_by_label("Close");
}

// --- closing windows, quitting, promoting ---

fn rotate_in(h: &mut Harness<'static, PdfCraftApp>, window: WindowId) {
    assert!(h.state_mut().with_window(window, |a| a.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 })).unwrap());
}

fn root_close_requested(h: &mut Harness<'static, PdfCraftApp>) {
    h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
}

/// The close request was delivered: stop sending it.
fn close_delivered(h: &mut Harness<'static, PdfCraftApp>) {
    if let Some(root) = h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT) {
        root.events.clear();
    }
}

fn cancelled_close(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.output()
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .is_some_and(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::CancelClose)))
}

/// Root shows one.pdf; a second window shows two.pdf.
fn root_and_child() -> (Harness<'static, PdfCraftApp>, WindowId) {
    let mut h = two_docs_harness();
    h.state_mut().active = Some(1);
    assert!(h.state_mut().execute("window.move_tab_new"));
    h.run_steps(4);
    let child = h.state().window_ids()[1];
    (h, child)
}

#[test]
fn closing_a_child_asks_about_what_only_it_shows() {
    let (mut h, child) = root_and_child();
    rotate_in(&mut h, child);
    let ctx = h.ctx.clone();
    h.state_mut().test_close_window(child, &ctx);
    assert!(h.state_mut().with_window(child, |a| a.close_request.is_some()).unwrap(), "the question is in the child");
    assert!(h.state().close_request.is_none(), "and not in the main window");
    h.run_steps(3);
    // Cancel: the window stays.
    h.state_mut().with_window(child, |a| a.resolve_close(&ctx, None));
    h.run_steps(3);
    assert_eq!(h.state().window_count(), 2);
    assert_windows_ok(h.state());
    // Don't save: window and document go.
    h.state_mut().test_close_window(child, &ctx);
    h.state_mut().with_window(child, |a| a.resolve_close(&ctx, Some(false)));
    h.run_steps(4);
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(h.state().session.docs().len(), 1, "the document closed with its last view");
    assert_windows_ok(h.state());
}

#[test]
fn closing_a_child_whose_document_is_open_elsewhere_asks_nothing() {
    let mut h = form_harness();
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    assert!(h.state_mut().execute("window.new_view"));
    h.run_steps(3);
    let child = h.state().window_ids()[1];
    let ctx = h.ctx.clone();
    h.state_mut().test_close_window(child, &ctx);
    assert!(h.state_mut().with_window(child, |a| a.close_request.is_none()).unwrap());
    h.run_steps(3);
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(h.state().session.docs().len(), 1);
    assert!(h.state().session.docs()[0].dirty, "still open, still unsaved");
}

#[test]
fn root_close_promotes_child() {
    let (mut h, child) = root_and_child();
    let two = h.state_mut().with_window(child, |a| a.views[0].id).unwrap();
    let ctx = h.ctx.clone();
    let _ = ctx;
    root_close_requested(&mut h);
    h.run_steps(1);
    close_delivered(&mut h);
    assert!(cancelled_close(&h), "the app stays open");
    h.run_steps(4);
    assert_eq!(h.state().window_count(), 1, "no child windows left");
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(h.state().views[0].id, two, "the main window shows the tabs of the one that took over");
    assert_eq!(h.state().session.docs().len(), 1, "the old window's document closed, nothing was asked");
    assert_windows_ok(h.state());
}

#[test]
fn closing_the_main_window_asks_about_documents_only_it_shows() {
    let (mut h, child) = root_and_child();
    rotate_in(&mut h, WindowId::ROOT);
    root_close_requested(&mut h);
    h.run_steps(1);
    close_delivered(&mut h);
    assert!(cancelled_close(&h));
    assert!(matches!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Window(w)) if w == WindowId::ROOT));
    let ctx = h.ctx.clone();
    h.state_mut().resolve_close(&ctx, None);
    h.run_steps(3);
    assert_eq!(h.state().window_count(), 2, "cancelled");
    root_close_requested(&mut h);
    h.run_steps(1);
    close_delivered(&mut h);
    h.state_mut().resolve_close(&ctx, Some(false));
    h.run_steps(4);
    assert_eq!(h.state().window_count(), 1);
    let _ = child;
    assert_windows_ok(h.state());
}

#[test]
fn quitting_asks_in_each_window_in_turn() {
    let (mut h, child) = root_and_child();
    rotate_in(&mut h, WindowId::ROOT);
    rotate_in(&mut h, child);
    let ctx = h.ctx.clone();
    h.state_mut().request_quit(&ctx);
    root_close_requested(&mut h);
    h.run_steps(1);
    close_delivered(&mut h);
    assert!(cancelled_close(&h));
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Quit), "the main window's document first");
    assert!(h.state_mut().with_window(child, |a| a.close_request.is_none()).unwrap());
    h.state_mut().resolve_close(&ctx, Some(false));
    h.run_steps(3);
    assert!(h.state().close_request.is_none());
    assert_eq!(h.state_mut().with_window(child, |a| a.close_request).unwrap(), Some(pdfcraft_ui_egui::CloseRequest::Quit), "then the other window's");
    h.state_mut().with_window(child, |a| a.resolve_close(&ctx, Some(false)));
    h.run_steps(3);
    assert!(h.state().session.docs().is_empty(), "both closed");
}

// --- input, OS events and pickers ---

#[derive(Debug)]
struct DroppedPdf(std::path::PathBuf);

impl egui::DroppedFile for DroppedPdf {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn scratch_pdf(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-windows-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.pdf"));
    std::fs::write(&path, include_bytes!("data/form.pdf")).unwrap();
    path
}

#[test]
fn a_file_dropped_on_the_second_window_opens_there() {
    let (mut h, child) = root_and_child();
    let path = scratch_pdf("dropped");
    let ctx = h.ctx.clone();
    h.state_mut().with_window(child, |a| a.handle_drops(vec![std::sync::Arc::new(DroppedPdf(path.clone()))], &ctx));
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "the main window is as it was");
    let names = h.state_mut().with_window(child, |a| a.views.len()).unwrap();
    assert_eq!(names, 2, "the file is a tab of the window it was dropped on");
    assert_windows_ok(h.state());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn an_open_request_from_the_system_shows_a_file_that_is_open_already() {
    let path = scratch_pdf("already");
    let file = path.to_string_lossy().into_owned();
    let go = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe({
        let (file, go) = (file.clone(), go.clone());
        move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_path(&file);
            app.open_bytes("other.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
            app.os_events = Some(Box::new(move || {
                if go.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    vec![pdfcraft_ui_egui::OsEvent::Open(vec![file.clone()])]
                } else {
                    Vec::new()
                }
            }));
            app
        }
    });
    h.run_steps(2);
    h.state_mut().active = Some(1);
    go.store(true, std::sync::atomic::Ordering::SeqCst);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 2, "no second copy");
    assert_eq!(h.state().active, Some(0), "it is shown");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_file_open_elsewhere_is_brought_forward_there() {
    let path = scratch_pdf("elsewhere");
    let file = path.to_string_lossy().into_owned();
    let mut h = two_docs_harness();
    h.state_mut().open_path(&file);
    h.state_mut().active = Some(2);
    assert!(h.state_mut().execute("window.move_tab_new"));
    h.run_steps(4);
    let child = h.state().window_ids()[1];
    h.state_mut().open_recent(&file);
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 2, "not opened a second time");
    assert_eq!(h.state_mut().with_window(child, |a| a.views.len()).unwrap(), 1);
    assert_windows_ok(h.state());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_picked_file_opens_in_the_window_that_asked() {
    let (mut h, child) = root_and_child();
    let path = scratch_pdf("picked");
    h.state_mut().pick_override = Some(vec![path.to_string_lossy().into_owned()]);
    h.state_mut().with_window(child, |a| a.open_dialog());
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1, "not in the main window");
    assert_eq!(h.state_mut().with_window(child, |a| a.views.len()).unwrap(), 2, "but in the window that asked");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

// --- the last session ---

fn two_files(dir: &str) -> (String, String) {
    let a = scratch_pdf(&format!("{dir}-a"));
    let b = scratch_pdf(&format!("{dir}-b"));
    (a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned())
}

#[test]
fn the_last_session_remembers_every_window_and_a_shared_file_opens_once() {
    let (a, b) = two_files("session");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe({
        let (a, b) = (a.clone(), b.clone());
        move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.reopen_last_session = true;
            app.open_path(&a);
            app.open_path(&b);
            app
        }
    });
    h.run_steps(3);
    // The second window shows a.pdf again (a second view) and b.pdf.
    h.state_mut().active = Some(0);
    assert!(h.state_mut().execute("window.new_view"));
    h.run_steps(3);
    let settings = h.state().persist();
    let value: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(value["last_session"]["windows"].as_array().map(Vec::len), Some(1), "{value}");

    let mut app = PdfCraftApp::new();
    app.restore(&settings);
    app.reopen_last_files(&[]);
    assert_eq!(app.views.len(), 2, "the main window's tabs");
    assert_eq!(app.window_count(), 2);
    assert_eq!(app.session.docs().len(), 2, "each file is open once");
    let doc_a = app.views[0].id;
    assert_eq!(app.view_count(doc_a), 2, "a.pdf has a view in each window");
    assert!(app.debug_check_windows().is_empty(), "{:?}", app.debug_check_windows());
    for f in [a, b] {
        let _ = std::fs::remove_dir_all(std::path::Path::new(&f).parent().unwrap());
    }
}

#[test]
fn a_session_with_a_broken_window_still_restores() {
    let (a, _b) = two_files("broken");
    let settings = serde_json::json!({
        "reopen_last_session": true,
        "last_session": {"files": [{"path": a}], "windows": [{"files": [{"path": a}], "rect": [f64::NAN, 1e300, -4, 0]}, {"files": [{"path": "/does/not/exist.pdf"}]}]},
    })
    .to_string();
    let mut app = PdfCraftApp::new();
    app.restore(&settings);
    app.reopen_last_files(&[]);
    assert_eq!(app.window_count(), 2, "the window with the missing file is not left empty");
    assert!(app.debug_check_windows().is_empty());
    let _ = std::fs::remove_dir_all(std::path::Path::new(&a).parent().unwrap());
}

// --- never crash ---

#[test]
fn window_operations_on_unknown_windows_and_documents_do_nothing() {
    use pdfcraft_ui_egui::WindowOp;
    let mut h = two_docs_harness();
    let doc = h.state().views[0].id;
    let ghost_doc = pdfcraft_engine::DocId(987_654);
    for op in [
        WindowOp::NewView { doc: ghost_doc, from: WindowId::ROOT },
        WindowOp::NewView { doc, from: WindowId(77) },
        WindowOp::MoveTab { doc: ghost_doc, from: WindowId::ROOT, to: None },
        WindowOp::MoveTab { doc, from: WindowId(77), to: None },
        WindowOp::MoveTab { doc, from: WindowId::ROOT, to: Some(WindowId(77)) },
        WindowOp::MoveTab { doc, from: WindowId::ROOT, to: Some(WindowId::ROOT) },
        WindowOp::Close(WindowId(77)),
        WindowOp::Close(WindowId::ROOT),
        WindowOp::Promote(WindowId(77)),
        WindowOp::Promote(WindowId::ROOT),
        WindowOp::Focus(WindowId(u32::MAX)),
    ] {
        h.state_mut().queue_window_op(op);
    }
    h.run_steps(3);
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(h.state().views.len(), 2);
    assert_windows_ok(h.state());
}

#[test]
fn closing_a_window_and_moving_a_tab_into_it_in_one_frame() {
    use pdfcraft_ui_egui::WindowOp;
    let (mut h, child) = root_and_child();
    let one = h.state().views[0].id;
    h.state_mut().queue_window_op(WindowOp::Close(child));
    h.state_mut().queue_window_op(WindowOp::MoveTab { doc: one, from: WindowId::ROOT, to: Some(child) });
    h.state_mut().queue_window_op(WindowOp::MoveTab { doc: one, from: WindowId::ROOT, to: None });
    h.run_steps(4);
    assert_windows_ok(h.state());
    assert!(h.state().session.docs().iter().all(|d| h.state().view_count(d.id) >= 1), "no document lost its views");
}

#[test]
fn many_new_windows_stop_with_a_notice() {
    let mut h = form_harness();
    let doc = h.state().views[0].id;
    for _ in 0..70 {
        h.state_mut().execute("window.new_view");
        h.run_steps(1);
    }
    h.run_steps(3);
    assert!(h.state().window_count() <= pdfcraft_ui_egui::windows::MAX_WINDOWS);
    assert!(h.state().view_count(doc) <= pdfcraft_ui_egui::windows::MAX_VIEWS_PER_DOCUMENT);
    assert!(h.state().toast.is_some(), "the user is told why no more windows open");
    assert_windows_ok(h.state());
}

/// A small, fixed pseudo-random generator, so a failure repeats.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

#[test]
fn random_window_operations_keep_the_windows_consistent() {
    use pdfcraft_ui_egui::WindowOp;
    for seed in 1..=6u64 {
        let mut rng = Lcg(seed);
        let mut h = two_docs_harness();
        h.state_mut().open_bytes("three.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        let ctx = h.ctx.clone();
        for step in 0..120 {
            let windows = h.state().window_ids();
            let window = windows[rng.next(windows.len())];
            let docs: Vec<_> = h.state().session.docs().iter().map(|d| d.id).collect();
            let doc = docs.get(rng.next(docs.len())).copied();
            match rng.next(11) {
                0 | 1 => {
                    if let Some(doc) = doc {
                        h.state_mut().queue_window_op(WindowOp::NewView { doc, from: window });
                    }
                }
                2 => {
                    if let Some(doc) = doc {
                        h.state_mut().queue_window_op(WindowOp::MoveTab { doc, from: window, to: None });
                    }
                }
                3 => {
                    let to = windows[rng.next(windows.len())];
                    if let Some(doc) = doc {
                        h.state_mut().queue_window_op(WindowOp::MoveTab { doc, from: window, to: Some(to) });
                    }
                }
                4 => h.state_mut().queue_window_op(WindowOp::Close(window)),
                5 => h.state_mut().queue_window_op(WindowOp::MergeAll),
                6 => {
                    h.state_mut().with_window(window, |a| {
                        let n = a.views.len();
                        if n > 0 {
                            a.request_close_tab(0);
                        }
                    });
                }
                7 => {
                    h.state_mut().with_window(window, |a| {
                        a.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
                    });
                }
                8 => {
                    h.state_mut().with_window(window, |a| a.undo());
                }
                9 => {
                    h.state_mut().test_close_window(window, &ctx);
                }
                _ => {
                    h.state_mut().with_window(window, |a| {
                        a.close_request = None;
                    });
                }
            }
            h.run_steps(1 + rng.next(2));
            let problems = h.state().debug_check_windows();
            // A window that lost its last tab is closed with the next frame's repair.
            h.run_steps(1);
            let problems_after = h.state().debug_check_windows();
            assert!(problems_after.is_empty(), "seed {seed} step {step}: {problems:?} then {problems_after:?}");
            assert!(h.state().window_count() <= pdfcraft_ui_egui::windows::MAX_WINDOWS);
        }
    }
}

#[test]
fn a_page_deleted_in_one_window_leaves_the_other_window_drawing_without_a_stale_selection_or_editor() {
    let mut h = form_harness();
    let doc = h.state().views[0].id;
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 200.0, height: 200.0 }));
    let other = h.state_mut().test_add_parked_view(doc).unwrap();
    h.run_steps(3);
    h.state_mut().with_window(other, |a| {
        let v = &mut a.views[0];
        v.comments.selected = Some((1, 0));
        v.links.selected = Some((1, 0));
        v.content.selected = Some((1, 0));
        v.test_select_image(1, 0);
        v.test_open_line_editor(1, 0, "kept");
        v.comments.test_open_composer(1, "also kept");
    });
    h.state_mut().views[0].select_pages(&[1]);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![1] }));
    h.run_steps(6);
    h.state_mut().with_window(other, |a| {
        let v = &a.views[0];
        assert!(v.comments.selected.is_none() && v.links.selected.is_none() && v.content.selected.is_none() && v.image_selection.is_none());
        assert_eq!(v.line_editor.as_ref().map(|e| e.text.as_str()), Some("kept"), "typed text stays");
        assert_eq!(v.comments.composer.as_ref().map(|c| c.text.as_str()), Some("also kept"));
    });
    assert_windows_ok(h.state());
}

fn four_docs_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for name in ["a.pdf", "b.pdf", "c.pdf", "d.pdf"] {
            app.open_bytes(name, None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        }
        // Three of them have unsaved changes; d.pdf (the one that stays) is clean.
        for i in 0..3 {
            app.active = Some(i);
            assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        }
        app.active = Some(3);
        app
    });
    h.run_steps(4);
    h
}

fn close_other_tabs_of_d(h: &mut Harness<'static, PdfCraftApp>) {
    h.get_by_label("d.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("Close other tabs").click();
    h.run_steps(3);
}

fn asked_about(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    match h.state().close_request {
        Some(pdfcraft_ui_egui::CloseRequest::Tab(id)) => h.state().session.get(id).map(|d| d.name.clone()),
        _ => None,
    }
}

#[test]
fn closing_other_tabs_asks_about_every_unsaved_document_in_turn() {
    let mut h = four_docs_harness();
    close_other_tabs_of_d(&mut h);
    assert_eq!(asked_about(&h).as_deref(), Some("a.pdf"));
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert_eq!(asked_about(&h).as_deref(), Some("b.pdf"));
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert_eq!(asked_about(&h).as_deref(), Some("c.pdf"));
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert!(h.state().close_request.is_none());
    assert_eq!(h.state().views.len(), 1, "only the tab that was kept is left");
    assert_windows_ok(h.state());
}

#[test]
fn cancelling_one_question_about_other_tabs_ends_the_run() {
    let mut h = four_docs_harness();
    close_other_tabs_of_d(&mut h);
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert_eq!(asked_about(&h).as_deref(), Some("b.pdf"));
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.state().close_request.is_none(), "no more questions");
    assert_eq!(h.state().views.len(), 3, "b, c and the kept tab stay open");
}

#[test]
fn a_tab_closed_meanwhile_is_skipped_by_the_run() {
    let mut h = four_docs_harness();
    close_other_tabs_of_d(&mut h);
    // c.pdf is closed some other way while the first question is open.
    let c = h.state().views[2].id;
    h.state_mut().close_document_everywhere_for_test(c);
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert_eq!(asked_about(&h).as_deref(), Some("b.pdf"));
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert!(h.state().close_request.is_none(), "c.pdf was skipped, nothing is left to ask");
    assert_eq!(h.state().views.len(), 1);
}

#[test]
fn quitting_with_the_question_answered_in_a_child_window_remembers_every_window() {
    let (a, b) = two_files("quit-in-child");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe({
        let (a, b) = (a.clone(), b.clone());
        move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.reopen_last_session = true;
            app.open_path(&a);
            app.open_path(&b);
            app
        }
    });
    h.run_steps(3);
    h.state_mut().active = Some(1);
    assert!(h.state_mut().execute("window.move_tab_new"));
    h.run_steps(3);
    let child = h.state().window_ids()[1];
    // Only the child's document has unsaved changes.
    rotate_in(&mut h, child);
    let ctx = h.ctx.clone();
    h.state_mut().request_quit(&ctx);
    root_close_requested(&mut h);
    h.run_steps(1);
    close_delivered(&mut h);
    assert!(h.state_mut().with_window(child, |a| a.close_request).unwrap() == Some(pdfcraft_ui_egui::CloseRequest::Quit), "asked in the child");
    h.state_mut().with_window(child, |a| a.resolve_close(&ctx, Some(false)));
    h.run_steps(3);
    let value: serde_json::Value = serde_json::from_str(&h.state().persist()).unwrap();
    let paths = |v: &serde_json::Value| v.as_array().map(|f| f.iter().filter_map(|f| f["path"].as_str().map(str::to_owned)).collect::<Vec<_>>());
    let main_files = paths(&value["last_session"]["files"]).unwrap_or_default();
    let child_files = paths(&value["last_session"]["windows"][0]["files"]).unwrap_or_default();
    assert!(main_files.iter().any(|p| p.ends_with("quit-in-child-a.pdf")), "{value}");
    assert!(child_files.iter().any(|p| p.ends_with("quit-in-child-b.pdf")), "the child's file is remembered too: {value}");
    for f in [a, b] {
        let _ = std::fs::remove_dir_all(std::path::Path::new(&f).parent().unwrap());
    }
}

fn combine_names(app: &PdfCraftApp) -> Vec<String> {
    app.combine_draft.iter().map(|f| f.name.clone()).collect()
}

fn fixture_pdf() -> Vec<u8> {
    include_bytes!("data/form.pdf").to_vec()
}

#[test]
fn the_combine_tab_of_the_main_window_survives_its_promotion() {
    let (mut h, _child) = root_and_child();
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("x.pdf".into(), fixture_pdf()), ("y.pdf".into(), fixture_pdf())]);
    h.run_steps(2);
    assert!(h.state().combine_tab.open);
    root_close_requested(&mut h);
    h.run_steps(1);
    close_delivered(&mut h);
    h.run_steps(4);
    assert_eq!(h.state().window_count(), 1);
    assert!(h.state().combine_tab.open, "the tab came along");
    assert_eq!(combine_names(h.state()), ["x.pdf", "y.pdf"]);
    assert_windows_ok(h.state());
}

#[test]
fn promoting_a_window_with_its_own_combine_list_adds_the_old_files_to_it() {
    let (mut h, child) = root_and_child();
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("x.pdf".into(), fixture_pdf())]);
    h.state_mut().with_window(child, |a| a.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("z.pdf".into(), fixture_pdf())]));
    h.run_steps(2);
    root_close_requested(&mut h);
    h.run_steps(1);
    close_delivered(&mut h);
    h.run_steps(4);
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(combine_names(h.state()), ["z.pdf", "x.pdf"]);
    let mut ids: Vec<u64> = h.state().combine_draft.iter().map(|f| f.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), 2, "every row keeps a row id of its own");
    assert_windows_ok(h.state());
}
