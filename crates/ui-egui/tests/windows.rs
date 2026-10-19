//! Several windows (M14): a tab dragged out of the strip into a window of its own, dragged onto
//! another window, the Window menu's commands, and what closing and quitting ask. The extra
//! windows are embedded in the main one here (egui's fallback where a backend has no windows),
//! at the place and size a real window would get, so the drops aim at real tab rects.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{CloseRequest, PdfCraftApp, WindowId, WindowOp};

const PDF: &[u8] = include_bytes!("data/form.pdf");

fn harness(names: &[&str]) -> Harness<'static, PdfCraftApp> {
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for name in &names {
            app.open_bytes(name, None, PDF.to_vec()).unwrap();
        }
        app
    });
    // Fonts install on frame 1 and apply on frame 2.
    h.run_steps(4);
    h
}

fn names(app: &PdfCraftApp) -> Vec<String> {
    app.views.iter().filter_map(|v| app.session.get(v.id)).map(|d| d.name.clone()).collect()
}

fn names_in(h: &mut Harness<'static, PdfCraftApp>, window: WindowId) -> Vec<String> {
    h.state_mut().with_window(window, |a| names(a)).expect("window exists")
}

fn tab_rect(h: &Harness<'static, PdfCraftApp>, name: &str) -> egui::Rect {
    h.get_by_role_and_label(Role::Tab, name).rect()
}

/// Press on the tab `name`, move to `to` in steps (pressing `escape` on the way when asked),
/// and release there when `release`.
fn drag_tab(h: &mut Harness<'static, PdfCraftApp>, name: &str, to: egui::Pos2, escape: bool, release: bool) {
    let from = tab_rect(h, name).center();
    h.event(egui::Event::PointerMoved(from));
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=6 {
        h.event(egui::Event::PointerMoved(from.lerp(to, k as f32 / 6.0)));
        h.run_steps(1);
        if escape && k == 4 {
            h.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
            h.run_steps(1);
            h.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE });
            h.run_steps(1);
        }
    }
    if release {
        h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(4);
    }
}

fn second_window(h: &Harness<'static, PdfCraftApp>) -> WindowId {
    let ids = h.state().window_ids();
    assert_eq!(ids.len(), 2, "one window besides the main one: {ids:?}");
    ids[1]
}

#[test]
fn dragging_a_tab_below_the_strip_opens_it_in_a_window_of_its_own() {
    let mut h = harness(&["first.pdf", "middle.pdf", "last.pdf"]);
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]));
    let strip = tab_rect(&h, "first.pdf");
    // Well past the strip's 40 pt detach zone, still inside the window.
    let to = egui::pos2(strip.center().x + 200.0, strip.bottom() + 160.0);
    drag_tab(&mut h, "first.pdf", to, false, true);
    assert_eq!(h.state().window_count(), 2);
    assert_eq!(h.state().current_window(), WindowId::ROOT, "between frames the main window is current");
    assert_eq!(names(h.state()), ["middle.pdf", "last.pdf"]);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[2]), "the active document stays active");
    let second = second_window(&h);
    assert_eq!(names_in(&mut h, second), ["first.pdf"]);
    assert_eq!(h.state().window_of_doc(ids[0]), Some(second));
    assert_eq!(h.state().session.docs().len(), 3, "moving a tab closes nothing");
    assert!(h.state().session.docs().iter().all(|d| !d.dirty));
    h.run_steps(2);
    // The new window shows the tab, below the main window's strip (so that strip stays usable).
    let moved = tab_rect(&h, "first.pdf");
    assert!(moved.top() > strip.bottom() + 40.0, "the tab is in the new window: {moved:?} under {strip:?}");
    assert_eq!(h.state().focused_window(), second);
}

#[test]
fn a_tab_dropped_on_another_window_joins_its_strip_where_the_pointer_is() {
    let mut h = harness(&["first.pdf", "middle.pdf", "last.pdf"]);
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    let ctx = h.ctx.clone();
    // first.pdf into a window of its own, low enough that its strip is clear of the main strip.
    h.state_mut().queue_window_op(WindowOp::MoveTab { doc: ids[0], from: WindowId::ROOT, to: None, at: Some(egui::pos2(700.0, 520.0)) });
    h.state_mut().apply_window_ops(&ctx);
    h.run_steps(3);
    let second = second_window(&h);
    let first_tab = tab_rect(&h, "first.pdf");
    assert!(first_tab.top() > 100.0, "the second window's strip lies below the main one's: {first_tab:?}");
    // middle.pdf onto the second window's strip, right of first.pdf: it lands after it.
    let to = first_tab.right_center() + egui::vec2(30.0, 0.0);
    drag_tab(&mut h, "middle.pdf", to, false, true);
    assert_eq!(h.state().window_count(), 2, "no third window");
    assert_eq!(names(h.state()), ["last.pdf"]);
    assert_eq!(names_in(&mut h, second), ["first.pdf", "middle.pdf"]);
    assert_eq!(h.state_mut().with_window(second, |a| a.active), Some(Some(1)), "the dropped tab is the active one there");
    assert_eq!(h.state().focused_window(), second);
    assert_eq!(h.state().session.docs().len(), 3);
    h.run_steps(2);
    // last.pdf onto the second window's document area: after the last tab.
    let body = egui::pos2(first_tab.center().x, first_tab.bottom() + 300.0);
    drag_tab(&mut h, "last.pdf", body, false, true);
    assert_eq!(names_in(&mut h, second), ["first.pdf", "middle.pdf", "last.pdf"]);
    assert!(h.state().views.is_empty() && h.state().active.is_none(), "the main window shows Home");
    assert_eq!(h.state().window_count(), 2, "the main window stays, empty");
}

#[test]
fn escape_cancels_a_tab_drag_and_a_short_drag_only_reorders() {
    let mut h = harness(&["first.pdf", "middle.pdf", "last.pdf"]);
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    let strip = tab_rect(&h, "first.pdf");
    // Dropped just under the strip (inside the detach zone), past middle.pdf: a reorder.
    let to = egui::pos2(tab_rect(&h, "middle.pdf").right() + 10.0, strip.bottom() + 20.0);
    drag_tab(&mut h, "first.pdf", to, false, true);
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(h.state().views.iter().map(|v| v.id).collect::<Vec<_>>(), [ids[1], ids[0], ids[2]]);
    // Escape on the way out: releasing far below the strip changes nothing.
    let far = egui::pos2(strip.center().x, strip.bottom() + 200.0);
    drag_tab(&mut h, "last.pdf", far, true, true);
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(h.state().views.iter().map(|v| v.id).collect::<Vec<_>>(), [ids[1], ids[0], ids[2]]);
    assert!(!h.state().tab_drag_in_progress(), "the drag is over");
}

#[test]
fn closing_a_window_with_unsaved_changes_asks_first() {
    let mut h = harness(&["a.pdf", "b.pdf"]);
    let ctx = h.ctx.clone();
    let b = h.state().views[1].id;
    h.state_mut().queue_window_op(WindowOp::MoveTab { doc: b, from: WindowId::ROOT, to: None, at: None });
    h.state_mut().apply_window_ops(&ctx);
    h.run_steps(2);
    let second = second_window(&h);
    h.state_mut().session.apply(b, pdfcraft_engine::Edit::InsertBlankPage { at: 0, width: 300.0, height: 400.0 }).unwrap();
    assert!(h.state().any_unsaved_work());
    assert!(h.state().first_dirty().is_none(), "the main window's own tab is clean");
    // Window ▸ Close window from the second window: the question first.
    h.state_mut().with_window(second, |a| assert!(a.execute("window.close")));
    h.state_mut().apply_window_ops(&ctx);
    assert_eq!(h.state().window_count(), 2, "the window stays while it asks");
    assert_eq!(h.state_mut().with_window(second, |a| a.close_request), Some(Some(CloseRequest::Window)));
    // Cancel keeps everything.
    h.state_mut().with_window(second, |a| a.resolve_close(&ctx, None));
    h.state_mut().apply_window_ops(&ctx);
    assert_eq!(h.state().window_count(), 2);
    assert!(h.state().session.get(b).is_some_and(|d| d.dirty));
    // Don't save: the tab goes, and the window with it.
    h.state_mut().with_window(second, |a| {
        a.close_request = Some(CloseRequest::Window);
        a.resolve_close(&ctx, Some(false));
    });
    h.state_mut().apply_window_ops(&ctx);
    assert_eq!(h.state().window_count(), 1);
    assert!(h.state().session.get(b).is_none(), "b.pdf closed with its window");
    assert_eq!(names(h.state()), ["a.pdf"]);
    assert_eq!(h.state().focused_window(), WindowId::ROOT);
}

#[test]
fn quitting_asks_about_unsaved_documents_in_every_window() {
    let mut h = harness(&["a.pdf", "b.pdf", "c.pdf"]);
    let ctx = h.ctx.clone();
    let (b, c) = (h.state().views[1].id, h.state().views[2].id);
    h.state_mut().queue_window_op(WindowOp::MoveTab { doc: b, from: WindowId::ROOT, to: None, at: None });
    h.state_mut().queue_window_op(WindowOp::MoveTab { doc: c, from: WindowId::ROOT, to: None, at: None });
    h.state_mut().apply_window_ops(&ctx);
    h.run_steps(2);
    assert_eq!(h.state().window_count(), 3);
    h.state_mut().session.apply(c, pdfcraft_engine::Edit::InsertBlankPage { at: 0, width: 300.0, height: 400.0 }).unwrap();
    assert!(!h.state_mut().request_quit(&ctx), "there is something to ask about");
    // Every tab is back in the main window, so the questions can bring each one forward.
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(names(h.state()), ["a.pdf", "b.pdf", "c.pdf"]);
    assert_eq!(h.state().close_request, Some(CloseRequest::Quit));
    assert_eq!(h.state().first_dirty(), Some(2));
    h.run_steps(2);
    h.get_by_label_contains("Save changes to “c.pdf”");
}

#[test]
fn the_window_commands_move_merge_and_close() {
    let mut h = harness(&["a.pdf", "b.pdf"]);
    let ctx = h.ctx.clone();
    let (a, b) = (h.state().views[0].id, h.state().views[1].id);
    h.state_mut().active = Some(0);
    assert!(h.state_mut().execute("window.move_tab_new"));
    h.state_mut().apply_window_ops(&ctx);
    h.run_steps(2);
    assert_eq!(h.state().window_count(), 2);
    let second = second_window(&h);
    assert_eq!(h.state().window_of_doc(a), Some(second));
    assert_eq!(names(h.state()), ["b.pdf"]);
    // The only tab of the second window may not leave it for a third window.
    assert!(!h.state_mut().with_window(second, |a| a.execute("window.move_tab_new")).unwrap(), "disabled: that tab is the window");
    h.state_mut().apply_window_ops(&ctx);
    assert_eq!(h.state().window_count(), 2);
    // Merge all windows from the second window: everything comes home.
    assert!(h.state_mut().with_window(second, |a| a.execute("window.merge_all")).unwrap());
    h.state_mut().apply_window_ops(&ctx);
    assert_eq!(h.state().window_count(), 1);
    assert_eq!(names(h.state()), ["b.pdf", "a.pdf"]);
    assert_eq!(h.state().session.docs().len(), 2);
    assert!(!h.state_mut().execute("window.merge_all"), "nothing to merge: disabled");
    // Close window on a clean second window closes it and its document.
    h.state_mut().queue_window_op(WindowOp::MoveTab { doc: b, from: WindowId::ROOT, to: None, at: None });
    h.state_mut().apply_window_ops(&ctx);
    h.run_steps(2);
    let second = second_window(&h);
    assert!(h.state_mut().with_window(second, |a| a.execute("window.close")).unwrap());
    h.state_mut().apply_window_ops(&ctx);
    assert_eq!(h.state().window_count(), 1);
    assert!(h.state().session.get(b).is_none());
    assert_eq!(names(h.state()), ["a.pdf"]);
    h.run_steps(2);
    assert_eq!(h.query_all_by_role_and_label(Role::Tab, "b.pdf").count(), 0);
}

/// The reading-colours preference reaches the documents of every window, not only the current
/// one's.
#[test]
fn reading_colours_apply_to_the_documents_of_every_window() {
    let mut h = harness(&["a.pdf", "b.pdf"]);
    let ctx = h.ctx.clone();
    let (a, b) = (h.state().views[0].id, h.state().views[1].id);
    h.state_mut().active = Some(0);
    assert!(h.state_mut().execute("window.move_tab_new"));
    h.state_mut().apply_window_ops(&ctx);
    h.run_steps(2);
    assert_eq!(h.state().window_of_doc(a), Some(second_window(&h)));
    h.state_mut().set_reading_colors(pdfcraft_render::ReadingColors::YELLOW_ON_BLACK);
    h.run_steps(1);
    for id in [a, b] {
        assert_eq!(h.state().session.reading_colors(id), Some(pdfcraft_render::ReadingColors::YELLOW_ON_BLACK));
    }
    assert_eq!(h.state().current_window(), WindowId::ROOT, "the main window's state is back in place");
}

/// A file chosen in a picker asked from a window opens in that window, and the pick's follow-up
/// runs with that window's state, although the main window has the focus when the choice
/// arrives.
#[test]
fn a_file_picked_from_a_window_opens_there() {
    let mut h = harness(&["a.pdf", "b.pdf"]);
    let ctx = h.ctx.clone();
    h.state_mut().active = Some(0);
    assert!(h.state_mut().execute("window.move_tab_new"));
    h.state_mut().apply_window_ops(&ctx);
    h.run_steps(2);
    let second = second_window(&h);
    let dir = std::env::temp_dir().join(format!("pdfcraft-window-pick-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("c.pdf");
    std::fs::write(&path, PDF).unwrap();
    // File ▸ Open from the second window; the choice arrives on a later frame, with the main
    // window's state loaded.
    h.state_mut()
        .with_window(second, |a| {
            a.pick_override = Some(vec![path.to_string_lossy().into_owned()]);
            assert!(a.execute("file.open"));
            a.pick_override = None;
        })
        .expect("second window");
    assert_eq!(h.state().current_window(), WindowId::ROOT);
    for _ in 0..300 {
        h.run_steps(1);
        if h.state().session.docs().len() == 3 {
            break;
        }
    }
    assert_eq!(h.state().session.docs().len(), 3, "c.pdf opened");
    h.run_steps(2);
    assert_eq!(names_in(&mut h, second), ["a.pdf", "c.pdf"], "in the window that asked");
    assert_eq!(names(h.state()), ["b.pdf"], "not in the main window");
    let _ = std::fs::remove_dir_all(dir);
}
