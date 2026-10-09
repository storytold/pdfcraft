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
