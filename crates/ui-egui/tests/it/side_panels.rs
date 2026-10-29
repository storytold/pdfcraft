//! Side panels: the Tools pane, the rail and the open side panel.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

const WIDTH: f32 = 1400.0;
const NO_BOOKMARKS: &str = "This document has no bookmarks.";

fn harness_sized(size: egui::Vec2, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(6);
    h
}

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    harness_sized(egui::vec2(WIDTH, 900.0), setup)
}

fn rail(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_role_and_label(Role::Button, "Bookmarks").rect()
}

fn side(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_label(NO_BOOKMARKS).rect()
}

/// The Tools pane's All tools header button.
fn tools(h: &Harness<'static, PdfCraftApp>) -> egui::Rect {
    h.get_by_label("Expand all tools").rect()
}

#[test]
fn rail_is_outermost_then_tools_then_the_side_panel_then_the_page() {
    // Tools on the left: the page between Tools and the side panel, the rail at the right edge.
    let h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    let (r, s, t) = (rail(&h), side(&h), tools(&h));
    assert!(r.right() > WIDTH - 12.0, "{r:?}");
    assert!(s.right() < r.left() && s.left() > WIDTH - 48.0 - 340.0, "{s:?} {r:?}");
    assert!(t.left() < 48.0, "{t:?}");
    // Tools on the right: rail, Tools, side panel, page.
    let h = harness(|app| {
        app.set_option("tools-side", "right").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
    });
    let (r, s, t) = (rail(&h), side(&h), tools(&h));
    assert!(r.right() > WIDTH - 12.0, "{r:?}");
    assert!(t.right() < r.left() && t.left() > WIDTH - 48.0 - 280.0, "{t:?} {r:?}");
    assert!(s.right() < t.left() && s.left() > WIDTH - 48.0 - 280.0 - 340.0, "{s:?} {t:?}");
}

#[test]
fn read_mode_shows_the_side_panel_without_tools() {
    let mut h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    h.state_mut().set_option("mode", "read").unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Expand all tools").is_none());
    assert!(side(&h).right() < rail(&h).left() && side(&h).left() > WIDTH / 2.0);
    // The rail closes and opens it there too.
    h.get_by_role_and_label(Role::Button, "Bookmarks").click();
    h.run_steps(4);
    assert!(h.query_by_label(NO_BOOKMARKS).is_none());
    h.get_by_role_and_label(Role::Button, "Bookmarks").click();
    h.run_steps(4);
    assert!(side(&h).right() < rail(&h).left());
}

/// Drag the side panel's resize edge `by` points to the left (wider).
fn widen_side(h: &mut Harness<'static, PdfCraftApp>, by: f32) {
    let side_left = side(h).left();
    // The side panel's resize edge: the one right of the page, left of the panel's text.
    let edge = h
        .get_all_by_label("Resize panels")
        .map(|n| n.rect().left_center())
        .filter(|c| c.x < side_left && c.x > WIDTH / 2.0)
        .min_by(|a, b| (side_left - a.x).total_cmp(&(side_left - b.x)))
        .expect("the side panel has a resize edge")
        // 2 pt inside the panel: the edge lies right of the border, clear of the page's scrollbar.
        + egui::vec2(2.0, 0.0);
    h.hover_at(edge);
    h.run_steps(1);
    h.drag_at(edge);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(edge - egui::vec2(by * i as f32 / 4.0, 0.0));
        h.run_steps(1);
    }
    h.drop_at(edge - egui::vec2(by, 0.0));
    h.run_steps(3);
}

#[test]
fn a_dragged_width_is_remembered_across_restarts() {
    let mut h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    let before = h.state().side_panels.side_width;
    widen_side(&mut h, 40.0);
    let after = h.state().side_panels.side_width;
    // The pointer was pressed 2 pt inside the panel, and the edge follows the pointer.
    assert!((after - (before + 40.0 - 2.0)).abs() <= 1.0, "{before} -> {after}");
    let saved = h.state().persist();
    let mut restored = PdfCraftApp::new();
    restored.restore(&saved);
    assert_eq!(restored.side_panels.side_width, after);
}

/// Long bookmark titles want room: the side panel widens well past the Tools pane's 520 pt, up to
/// 70 % of the window, so the page stays in view.
#[test]
fn the_side_panel_widens_up_to_most_of_the_window() {
    let mut h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    let before = h.state().side_panels.side_width;
    widen_side(&mut h, 400.0);
    let wide = h.state().side_panels.side_width;
    assert!((wide - (before + 400.0 - 2.0)).abs() <= 1.0, "{before} -> {wide}");
    assert!(wide > 520.0 && (side(&h).width() - wide).abs() <= 2.0, "drawn as wide as set: {wide}, {:?}", side(&h));
    widen_side(&mut h, 600.0);
    let widest = h.state().side_panels.side_width;
    assert!(widest > wide && widest <= 0.7 * WIDTH + 1.0, "{wide} -> {widest}");
}

#[test]
fn f4_toggles_the_side_panel_and_shift_f4_the_tools_panel() {
    let mut h = harness(|app| app.set_option("panel", "bookmarks").unwrap());
    h.get_by_label(NO_BOOKMARKS);
    h.key_press(egui::Key::F4);
    h.run_steps(3);
    assert_eq!(h.state().right, None);
    assert!(h.query_by_label(NO_BOOKMARKS).is_none());
    // F4 again reopens the panel that was open, not the default.
    h.key_press(egui::Key::F4);
    h.run_steps(3);
    h.get_by_label(NO_BOOKMARKS);
    assert!(h.state().left_open);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F4);
    h.run_steps(3);
    assert!(!h.state().left_open);
    assert!(h.query_by_label("Expand all tools").is_none());
    // Shift+F4 leaves the side panel alone.
    h.get_by_label(NO_BOOKMARKS);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F4);
    h.run_steps(3);
    h.get_by_label("Expand all tools");
}

#[test]
fn scrollbar_and_panel_resize_handle_do_not_collide_on_either_dock_side() {
    // 1. Left docking: tools panel on the left (resize handle on the right, scrollbar on the right).
    let mut h = harness_sized(egui::vec2(1200.0, 450.0), |app| {
        app.all_tools_expanded = true;
    });
    h.run_steps(4);

    let splitters: Vec<_> = h.get_all_by_label("Resize panels").collect();
    assert!(!splitters.is_empty(), "Splitter handle should exist");
    let splitter_rect = splitters[0].rect();

    // Find the scrollbar belonging to the Tools panel (left side of the window).
    let sb_node = h.get_all_by_role(Role::ScrollBar).find(|s| s.rect().max.x < 300.0).expect("Tools panel scrollbar should exist");
    let sb_rect = sb_node.rect();

    // The scrollbar must sit strictly inside the panel, with clear gutter before the splitter.
    assert!(sb_rect.max.x < splitter_rect.min.x);
    assert!(splitter_rect.min.x - sb_rect.max.x >= 12.0, "Gutter must be at least 12px");

    // Hovering directly over the scrollbar must NOT trigger panel resize cursor.
    for x in [sb_rect.min.x, sb_rect.center().x, sb_rect.max.x - 0.5] {
        h.hover_at(egui::pos2(x, sb_rect.center().y));
        h.run_steps(1);
        let cursor = h.output().platform_output.cursor_icon;
        assert_ne!(cursor, egui::CursorIcon::ResizeHorizontal, "Hovering scrollbar at x={x} should not trigger resize handle");
    }

    // Hovering directly on the splitter handle MUST trigger panel resize cursor.
    h.hover_at(splitter_rect.center());
    h.run_steps(1);
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeHorizontal);

    // Dragging the scrollbar down scrolls the content up without resizing the panel.
    let initial_width = h.state().side_panels.tools_width;
    let first_row_before = h.get_by_label("Edit a PDF").rect().top();
    h.hover_at(sb_rect.center());
    h.run_steps(1);
    h.drag_at(sb_rect.center());
    h.run_steps(1);
    h.hover_at(sb_rect.center() + egui::vec2(0.0, 60.0));
    h.run_steps(1);
    h.drop_at(sb_rect.center() + egui::vec2(0.0, 60.0));
    h.run_steps(2);
    let first_row_after = h.get_by_label("Edit a PDF").rect().top();
    assert!(first_row_after < first_row_before, "Content should scroll up when dragging scrollbar down");
    assert_eq!(h.state().side_panels.tools_width, initial_width, "Dragging scrollbar must not resize the panel");

    // Dragging the splitter smoothly resizes the panel.
    h.hover_at(splitter_rect.center());
    h.run_steps(1);
    h.drag_at(splitter_rect.center());
    h.run_steps(1);
    h.hover_at(splitter_rect.center() + egui::vec2(40.0, 0.0));
    h.run_steps(1);
    h.drop_at(splitter_rect.center() + egui::vec2(40.0, 0.0));
    h.run_steps(2);
    let after_width = h.state().side_panels.tools_width;
    assert!(after_width > initial_width);
    assert!((after_width - (initial_width + 40.0)).abs() <= 5.0);

    // 2. Right docking: tools panel on the right (resize handle on the left), next to the page's
    // scrollbar (#5730).
    let mut h_right = harness_sized(egui::vec2(1200.0, 450.0), |app| {
        app.set_option("tools-side", "right").unwrap();
        app.all_tools_expanded = true;
    });
    h_right.run_steps(4);

    let splitters_right: Vec<_> = h_right.get_all_by_label("Resize panels").collect();
    assert!(!splitters_right.is_empty());
    let right_splitter_rect = splitters_right[0].rect();
    let bars: Vec<egui::Rect> = h_right.get_all_by_role(Role::ScrollBar).map(|s| s.rect()).filter(|r| r.height() > 0.0).collect();
    // The page's scrollbar ends where the pane begins; the pane's own one is at its far side.
    let page_bar = bars
        .iter()
        .copied()
        .filter(|r| r.min.x < right_splitter_rect.center().x)
        .max_by(|a, b| a.max.x.total_cmp(&b.max.x))
        .expect("the page has a scrollbar");
    assert!(
        (right_splitter_rect.center().x - page_bar.max.x).abs() < 6.0,
        "the page's scrollbar is beside the edge: {page_bar:?} {right_splitter_rect:?}"
    );
    let tools_bar = bars.iter().copied().find(|r| r.min.x > right_splitter_rect.center().x).expect("Tools panel scrollbar should exist");
    let initial_right_width = h_right.state().side_panels.tools_width;

    // All of either scrollbar is the scrollbar, not the resize edge.
    for bar in [page_bar, tools_bar] {
        for x in [bar.min.x + 0.5, bar.center().x, bar.max.x - 0.5] {
            h_right.hover_at(egui::pos2(x, bar.center().y));
            h_right.run_steps(1);
            let cursor = h_right.output().platform_output.cursor_icon;
            assert_ne!(cursor, egui::CursorIcon::ResizeHorizontal, "Hovering scrollbar {bar:?} at x={x} should not trigger resize handle");
        }
    }

    // Dragging either scrollbar by its right edge scrolls its content and leaves the width alone.
    let drag = |h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2| {
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.hover_at(at + egui::vec2(0.0, 60.0));
        h.run_steps(1);
        h.drop_at(at + egui::vec2(0.0, 60.0));
        h.run_steps(2);
    };
    let page_top = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].page_screen_rect(0).expect("page on screen").min.y;
    let before = page_top(&h_right);
    drag(&mut h_right, egui::pos2(page_bar.max.x - 1.0, page_bar.center().y));
    assert!(page_top(&h_right) < before, "dragging the page's scrollbar scrolls the page");
    assert_eq!(h_right.state().side_panels.tools_width, initial_right_width, "Dragging scrollbar must not resize the panel");
    let before = h_right.get_by_label("Edit a PDF").rect().top();
    drag(&mut h_right, egui::pos2(tools_bar.max.x - 1.0, tools_bar.center().y));
    assert!(h_right.get_by_label("Edit a PDF").rect().top() < before, "Content should scroll up when dragging scrollbar down");
    assert_eq!(h_right.state().side_panels.tools_width, initial_right_width, "Dragging scrollbar must not resize the panel");

    // Splitter is at the left boundary of the right-docked pane.
    h_right.hover_at(right_splitter_rect.center());
    h_right.run_steps(1);
    assert_eq!(h_right.output().platform_output.cursor_icon, egui::CursorIcon::ResizeHorizontal);

    // Dragging the splitter on right-docked pane smoothly resizes.
    h_right.hover_at(right_splitter_rect.center());
    h_right.run_steps(1);
    h_right.drag_at(right_splitter_rect.center());
    h_right.run_steps(1);
    // Moving left increases width of right-docked pane
    h_right.hover_at(right_splitter_rect.center() - egui::vec2(30.0, 0.0));
    h_right.run_steps(1);
    h_right.drop_at(right_splitter_rect.center() - egui::vec2(30.0, 0.0));
    h_right.run_steps(2);
    let after_right_width = h_right.state().side_panels.tools_width;
    assert!(after_right_width > initial_right_width);
    assert!((after_right_width - (initial_right_width + 30.0)).abs() <= 5.0);

    // Inside the right-docked panel (e.g. 15px inside from the splitter), cursor is not resize.
    h_right.hover_at(right_splitter_rect.center() + egui::vec2(15.0, 0.0));
    h_right.run_steps(1);
    assert_ne!(h_right.output().platform_output.cursor_icon, egui::CursorIcon::ResizeHorizontal);
}
