//! Split view (`view.split-window`): a tab's menu splits right or down, ⌘\ splits the current
//! document and ends split view again, each pane keeps its own zoom, the same document in both
//! panes is one document (one edit, one Undo), the pane last clicked is the one menus and keys act
//! on, and a tab dropped on the right half of the page splits right with it.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::panes::Place;

/// `pages` portrait pages (300 × 400) with a line of text each.
fn fixture(pages: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..pages).map(|k| format!("{} 0 R", 4 + 2 * k)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 300 400] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for k in 0..pages {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * k));
        let content = format!("BT /F1 18 Tf 40 300 Td (Page {}) Tj ET", k + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
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

fn harness(names: &'static [&'static str]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.set_option("language", "en").unwrap();
        for name in names {
            app.open_bytes(name, None, fixture(3)).unwrap();
        }
        app
    });
    h.run_steps(4);
    h
}

fn index(h: &Harness<'static, PdfCraftApp>, name: &str) -> usize {
    let s = h.state();
    s.views.iter().position(|v| s.session.get(v.id).is_some_and(|d| d.name == name)).expect("tab open")
}

fn places(h: &Harness<'static, PdfCraftApp>) -> Vec<Place> {
    h.state().views.iter().map(|v| v.place()).collect()
}

/// Run frames until `ready`, rendering in between (renders arrive between frames).
fn wait(h: &mut Harness<'static, PdfCraftApp>, what: &str, ready: impl Fn(&PdfCraftApp) -> bool) {
    for _ in 0..600 {
        h.run_steps(1);
        if ready(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

/// A press and release at `at` (a click on the page area).
fn click_at(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn drag_tab(h: &mut Harness<'static, PdfCraftApp>, name: &str, to: egui::Pos2) {
    let from = h.get_by_role_and_label(Role::Tab, name).rect().center();
    h.event(egui::Event::PointerMoved(from));
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=8 {
        h.event(egui::Event::PointerMoved(from.lerp(to, k as f32 / 8.0)));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn split_right_from_the_tab_menu_shows_two_panes_each_with_its_own_zoom() {
    let mut h = harness(&["quote.pdf", "order.pdf"]);
    // The menu of quote.pdf, not the tab the main pane shows (order.pdf, opened last): it moves
    // into the second pane.
    h.get_by_role_and_label(Role::Tab, "quote.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("Split right").click();
    h.run_steps(4);
    let (quote, order) = (index(&h, "quote.pdf"), index(&h, "order.pdf"));
    let split = h.state().panes.split.expect("split view is on");
    assert!(!split.down);
    assert_eq!(h.state().views[order].place(), Place::Main);
    assert_eq!(h.state().views[quote].place(), Place::Second);
    // Side by side: two page areas, the second to the right of the first.
    let (left, right) = (h.state().views[order].viewport_rect(), h.state().views[quote].viewport_rect());
    assert!(left.width() > 200.0 && right.width() > 200.0, "{left:?} {right:?}");
    assert!(right.left() >= left.right(), "{left:?} {right:?}");
    // The second pane has its own strip with its tab; the main strip no longer shows it.
    assert_eq!(h.query_all_by_role_and_label(Role::Tab, "quote.pdf").count(), 1);
    assert_eq!(h.state().active, Some(quote), "the tab split into the pane is active");
    // ⌘+ zooms the active pane only.
    let before = (h.state().views[order].zoom, h.state().views[quote].zoom);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Plus);
    h.run_steps(3);
    assert_eq!(h.state().views[order].zoom, before.0, "the other pane keeps its zoom");
    assert!(h.state().views[quote].zoom > before.1, "the active pane zoomed in");
    // Split down turns the panes.
    h.get_by_role_and_label(Role::Tab, "quote.pdf").click_secondary();
    h.run_steps(2);
    h.get_by_label("Split down").click();
    h.run_steps(4);
    assert!(h.state().panes.split.is_some_and(|s| s.down));
    let (top, bottom) = (h.state().views[order].viewport_rect(), h.state().views[quote].viewport_rect());
    assert!(bottom.top() >= top.bottom(), "{top:?} {bottom:?}");
}

#[test]
fn the_same_document_in_both_panes_is_one_document_with_one_undo() {
    let mut h = harness(&["contract.pdf"]);
    // ⌘\ splits the current document: a second view of it in the second pane.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(4);
    assert_eq!(places(&h), [Place::Main, Place::Second]);
    let (first, second) = (0, 1);
    assert_eq!(h.state().views[first].id, h.state().views[second].id, "one document");
    assert_eq!(h.state().session.docs().len(), 1);
    // Both panes draw the portrait page.
    let portrait = |s: &PdfCraftApp, i: usize| s.views[i].drawn_page_size(0).is_some_and(|[w, hgt]| hgt > w);
    let landscape = |s: &PdfCraftApp, i: usize| s.views[i].drawn_page_size(0).is_some_and(|[w, hgt]| w > hgt);
    wait(&mut h, "both panes drawn", |s| portrait(s, first) && portrait(s, second));
    // Each pane scrolls on its own: the second goes to page 3.
    h.state_mut().views[second].go_to_page(2);
    h.run_steps(4);
    assert_eq!(h.state().views[first].current, 0);
    assert_eq!(h.state().views[second].current, 2);
    // An edit made in the first pane shows in the second pane too. (The second view of the
    // document is the one the app's own edit code doesn't find by the document's id.)
    h.state_mut().views[second].go_to_page(0);
    h.run_steps(3);
    let at = h.state().views[first].viewport_rect().center();
    click_at(&mut h, at);
    assert_eq!(h.state().active, Some(first), "a click makes the pane active");
    assert!(h.state_mut().execute("page.rotate"));
    wait(&mut h, "the rotated page in both panes", |s| landscape(s, first) && landscape(s, second));
    let id = h.state().views[first].id;
    assert!(h.state().session.get(id).unwrap().dirty);
    // One Undo, from the other pane, puts it back in both.
    let at = h.state().views[second].viewport_rect().center();
    click_at(&mut h, at);
    assert_eq!(h.state().active, Some(second));
    assert!(h.state_mut().execute("edit.undo"));
    wait(&mut h, "the page upright again in both panes", |s| portrait(s, first) && portrait(s, second));
    assert!(h.state().session.get(id).unwrap().can_undo().is_none(), "it was one step");
    // The other pane redraws after a change that leaves its page's size alone too (a comment
    // added in the first pane): its page goes out of date at once, then comes back.
    wait(&mut h, "both panes drawn", |s| portrait(s, first) && portrait(s, second));
    let at = h.state().views[first].viewport_rect().center();
    click_at(&mut h, at);
    assert_eq!(h.state().active, Some(first));
    wait(&mut h, "both panes drawn", |s| portrait(s, first) && portrait(s, second));
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
    let shape = Shape::Rectangle { rect: [40.0, 40.0, 200.0, 200.0] };
    let comment = NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: String::new() };
    assert!(h.state_mut().apply_edit(Edit::AddAnnotation(comment)));
    h.run_steps(1);
    assert!(h.state().views[second].drawn_page_size(0).is_none(), "the second pane's page is being drawn again");
    wait(&mut h, "both panes drawn again", |s| portrait(s, first) && portrait(s, second));
}

#[test]
fn split_again_or_closing_a_panes_last_tab_ends_split_view() {
    let mut h = harness(&["a.pdf", "b.pdf"]);
    // ⌘\ twice: split, then back to one pane; the second view of the document closes, the
    // document stays open.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(3);
    assert!(h.state().panes.split.is_some());
    assert_eq!(h.state().views.len(), 3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(3);
    assert!(h.state().panes.split.is_none());
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(places(&h), [Place::Main, Place::Main]);
    assert_eq!(h.state().session.docs().len(), 2);
    // Split right with a.pdf (not the tab shown): it moves to the second pane. Closing it there,
    // the pane's last tab, ends split view and closes only that tab.
    let a = index(&h, "a.pdf");
    h.state_mut().split_tab(a, false);
    h.run_steps(3);
    assert_eq!(h.state().views[index(&h, "a.pdf")].place(), Place::Second);
    let a = index(&h, "a.pdf");
    h.state_mut().request_close_tab(a);
    h.run_steps(3);
    assert!(h.state().panes.split.is_none());
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(places(&h), [Place::Main]);
    // The same document in both panes: closing one view keeps the document and its edits.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(3);
    assert!(h.state_mut().execute("page.rotate"));
    h.run_steps(2);
    let second = h.state().views.iter().position(|v| v.place() == Place::Second).unwrap();
    h.state_mut().request_close_tab(second);
    h.run_steps(3);
    assert!(h.state().close_request.is_none(), "no save prompt: the document is still open");
    assert!(h.state().panes.split.is_none());
    assert_eq!(h.state().views.len(), 1);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().dirty, "its edit is still there");
    // Typing a view holds of its own (not in the document yet) is asked about, even with the
    // document open in the other pane; Don't save closes that view, not the other.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(3);
    let second = h.state().views.iter().position(|v| v.place() == Place::Second).unwrap();
    let (first_key, id) = (h.state().views[0].view_key(), h.state().views[0].id);
    let typed = pdfcraft_ui_egui::fill_sign::TypeBox { page: 0, at: [40.0, 40.0], text: "typed".into(), size: 10.0, focus: false };
    h.state_mut().views[second].fill_text = Some(typed);
    h.state_mut().request_close_tab(second);
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Tab(id)), "the typing is asked about");
    let ctx = h.ctx.clone();
    h.state_mut().resolve_close(&ctx, Some(false));
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(h.state().views[0].view_key(), first_key, "the view asked about closed, the other stayed");
    assert!(h.state().session.get(id).unwrap().dirty, "the document and its edit are still open");
}

#[test]
fn the_pane_last_clicked_is_the_one_menus_and_keys_act_on() {
    // left.pdf, opened last, is the tab shown; right.pdf goes to the second pane.
    let mut h = harness(&["right.pdf", "left.pdf"]);
    let right = index(&h, "right.pdf");
    h.state_mut().split_tab(right, false);
    h.run_steps(4);
    let (left, right) = (index(&h, "left.pdf"), index(&h, "right.pdf"));
    let (lid, rid) = (h.state().views[left].id, h.state().views[right].id);
    // A click in the left pane makes it active; a command from the menu acts on its document.
    let at = h.state().views[left].viewport_rect().center();
    click_at(&mut h, at);
    assert_eq!(h.state().active, Some(left));
    h.state_mut().run_command("page.rotate");
    h.run_steps(2);
    assert!(h.state().session.get(lid).unwrap().dirty);
    assert!(!h.state().session.get(rid).unwrap().dirty);
    // Then the right pane: the active tab follows the click, and so does the next command.
    let at = h.state().views[right].viewport_rect().center();
    click_at(&mut h, at);
    assert_eq!(h.state().active, Some(right));
    h.state_mut().run_command("page.rotate");
    h.run_steps(2);
    assert!(h.state().session.get(rid).unwrap().dirty);
    // The main strip still marks the tab its pane shows, and the window title names it.
    assert_eq!(h.get_by_role_and_label(Role::Tab, "left.pdf (edited)").accesskit_node().is_selected(), Some(true));
    assert!(h.state().window_title.starts_with("left.pdf"), "{}", h.state().window_title);
}

#[test]
fn a_tab_dropped_on_the_right_half_of_the_page_splits_right_with_it() {
    let mut h = harness(&["one.pdf", "two.pdf"]);
    let page = h.state().views[index(&h, "two.pdf")].viewport_rect();
    drag_tab(&mut h, "one.pdf", egui::pos2(page.right() - 120.0, page.center().y));
    assert!(h.state().panes.split.is_some(), "dropped on the right half: split view");
    assert_eq!(h.state().views[index(&h, "one.pdf")].place(), Place::Second);
    assert_eq!(h.state().views[index(&h, "two.pdf")].place(), Place::Main);
    // Dragged back onto the main strip, it rejoins it and split view ends.
    let strip = h.get_by_role_and_label(Role::Tab, "two.pdf").rect();
    drag_tab(&mut h, "one.pdf", strip.right_center() + egui::vec2(40.0, 0.0));
    assert!(h.state().panes.split.is_none());
    assert_eq!(places(&h), [Place::Main, Place::Main]);
    // Let go outside the window, a tab moves along the strip as before split view (here to the
    // front), and nothing splits.
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    drag_tab(&mut h, "two.pdf", egui::pos2(-200.0, 300.0));
    assert!(h.state().panes.split.is_none());
    assert_eq!(h.state().views.iter().map(|v| v.id).collect::<Vec<_>>(), [ids[1], ids[0]]);
}

/// Regression: closing the second pane's tab from its own strip, with the same document in both
/// panes, drew that pane with the closed tab's index ("index out of bounds").
#[test]
fn closing_the_second_panes_tab_from_its_strip_keeps_the_program_running() {
    let mut h = harness(&["a.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    h.run_steps(3);
    assert_eq!(places(&h), [Place::Main, Place::Second]);
    let main_right = h.state().views[0].viewport_rect().right();
    let tab = h.get_all_by_role_and_label(Role::Tab, "a.pdf").find(|t| t.rect().left() >= main_right).expect("the second pane's tab");
    tab.click_secondary();
    h.run_steps(2);
    h.get_by_label("Close Tab").click();
    h.run_steps(3);
    assert!(h.state().panes.split.is_none());
    assert_eq!(h.state().views.len(), 1);
}
