use super::*;

/// A selected line is drawn where it is going: moved by its body (both ends follow the
/// pointer), with an end dragged (that end alone), and as released until the page is drawn.
#[test]
fn a_moved_or_reshaped_line_is_drawn_at_its_new_points() {
    let line = [[10.0, 10.0], [50.0, 10.0]];
    let to_user = |p: Pos2| [f64::from(p.x), f64::from(p.y)];
    let moving = Gesture::Move { page: 0, index: 3, from: pos2(20.0, 10.0) };
    let at = Some(pos2(25.0, 30.0));
    assert_eq!(changing_points(Some(&moving), None, (0, 3), &line, to_user, at, false), Some(vec![[15.0, 30.0], [55.0, 30.0]]));
    assert_eq!(changing_points(Some(&moving), None, (0, 4), &line, to_user, at, false), None, "another comment's move");
    let released = Edit::MoveAnnotation { page: 0, index: 3, dx: 5.0, dy: 20.0 };
    assert_eq!(changing_points(None, Some(&released), (0, 3), &line, to_user, None, false), Some(vec![[15.0, 30.0], [55.0, 30.0]]));
    let dragging = Gesture::Reshape { page: 0, index: 3, point: 1, points: line.to_vec() };
    let to = Some(pos2(60.0, 10.0));
    assert_eq!(changing_points(Some(&dragging), None, (0, 3), &line, to_user, to, false), Some(vec![[10.0, 10.0], [60.0, 10.0]]));
    assert_eq!(changing_points(None, None, (0, 3), &line, to_user, to, false), None, "not changing");
}

/// A sticky note on a page turned a quarter is drawn upright beside its `/Rect`: an edit
/// composer stays open while the drawn note is partly in view, even with `/Rect` out of it.
#[test]
fn an_upright_note_on_a_turned_page_is_in_view_where_it_is_drawn() {
    let note = |no_rotate| Annotation {
        page: 0,
        subtype: "Text".into(),
        author: None,
        contents: None,
        modified: None,
        name: None,
        in_reply_to: None,
        parent: None,
        grouped: false,
        rect: [40.0, 20.0, 70.0, 40.0],
        color: None,
        index: 0,
        state: None,
        quads: Vec::new(),
        locked: false,
        intent: None,
        fill_sign: false,
        no_rotate,
        points: Vec::new(),
    };
    let page = PageInfo { width: 140.0, height: 200.0, label: String::new(), crop: [0.0, 0.0, 200.0, 140.0], rotation: 90 };
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(140.0, 200.0)), rot: 0, pw: 140.0, ph: 200.0 };
    let prefs = CommentPrefs::default();
    // Screen x is user y here: `/Rect` spans x 20..40, the upright note x 40..70.
    let viewport = Rect::from_min_max(pos2(50.0, 0.0), pos2(140.0, 200.0));
    for (no_rotate, in_view) in [(true, true), (false, false)] {
        let info = DocInfo { pages: vec![page.clone()], annotations: vec![note(no_rotate)], ..Default::default() };
        let cx = PageCx {
            page: 0,
            xf: &xf,
            ptr: &PagePointer::default(),
            info: &info,
            tool: QuickTool::Select,
            prefs: &prefs,
            allowed: true,
            hidden: false,
            redact_mark: ([0.0; 3], 1.0),
        };
        assert_eq!(edited_in_view(&cx, 0, viewport), in_view, "no_rotate: {no_rotate}");
    }
}

use pdfcraft_render::TextGlyph;

/// One glyph per character, `w` points wide, on a line of `em`-point text whose baseline is at `y`, drawn
/// along `direction` (a unit vector in view space, y down).
fn line(chars: &str, x: f32, y: f32, w: f32, em: f32, direction: [f32; 2]) -> PageText {
    let [dx, dy] = direction;
    // "Up" for the glyph: its writing direction turned a quarter counterclockwise (y down).
    let [ux, uy] = [dy, -dx];
    let glyphs = chars
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let start = [x + dx * w * i as f32, y + dy * w * i as f32];
            let at = |along: f32, up: f32| [start[0] + dx * along + ux * up, start[1] + dy * along + uy * up];
            let quad = [at(0.0, -0.2 * em), at(w, -0.2 * em), at(w, 0.8 * em), at(0.0, 0.8 * em)];
            let xs = quad.map(|p| p[0]);
            let ys = quad.map(|p| p[1]);
            let rect = [
                xs.into_iter().fold(f32::INFINITY, f32::min),
                ys.into_iter().fold(f32::INFINITY, f32::min),
                xs.into_iter().fold(f32::NEG_INFINITY, f32::max),
                ys.into_iter().fold(f32::NEG_INFINITY, f32::max),
            ];
            TextGlyph { text: c.to_string(), rect, quad, direction }
        })
        .collect::<Vec<_>>();
    let n = glyphs.len();
    PageText { glyphs, line_of: vec![0; n], space_before: vec![false; n] }
}

fn annotation(subtype: &str, contents: Option<&str>, fill_sign: bool) -> Annotation {
    Annotation {
        page: 0,
        subtype: subtype.into(),
        author: None,
        contents: contents.map(str::to_string),
        modified: None,
        name: None,
        in_reply_to: None,
        parent: None,
        grouped: false,
        rect: [0.0; 4],
        color: None,
        index: 0,
        state: None,
        quads: Vec::new(),
        locked: false,
        intent: None,
        fill_sign,
        no_rotate: false,
        points: Vec::new(),
    }
}

#[test]
fn the_hover_tip_skips_fill_and_sign_marks_replies_and_bare_highlights_when_asked() {
    let prefs = CommentPrefs::default();
    assert_eq!(hover_tip(&annotation("FreeText", Some("Hello"), false), &prefs).as_deref(), Some("FreeText\nHello"));
    assert_eq!(hover_tip(&annotation("FreeText", Some("Hello"), true), &prefs), None);
    let reply = Annotation { in_reply_to: Some("1 0 R".into()), parent: Some((0, 1)), ..annotation("Text", Some("Agreed"), false) };
    assert_eq!(hover_tip(&reply, &prefs), None);
    let bare = Annotation { author: Some("Ana".into()), ..annotation("Highlight", None, false) };
    assert_eq!(hover_tip(&bare, &prefs).as_deref(), Some("Ana"));
    let hide = CommentPrefs { hide_empty_highlight_hover: true, ..CommentPrefs::default() };
    assert_eq!(hover_tip(&bare, &hide), None);
    assert_eq!(hover_tip(&annotation("Highlight", Some("Check this"), false), &hide).as_deref(), Some("Highlight\nCheck this"));
}

fn near(a: Option<([f32; 2], f32)>, b: ([f32; 2], f32)) -> bool {
    a.is_some_and(|([x, y], s)| (x - b.0[0]).abs() < 1e-3 && (y - b.0[1]).abs() < 1e-3 && (s - b.1).abs() < 1e-3)
}

#[test]
fn the_caret_snaps_to_the_nearer_edge_on_the_baseline() {
    let text = line("abcd", 20.0, 100.0, 10.0, 14.0, [1.0, 0.0]);
    // "c" spans 40..50: its left half snaps before it, its right half after it.
    assert!(near(caret_snap(&text, 43.0, 95.0), ([40.0, 100.0], 7.0)));
    assert!(near(caret_snap(&text, 47.0, 92.0), ([50.0, 100.0], 7.0)));
    // Just past the last character, on the line: after it.
    assert!(near(caret_snap(&text, 63.0, 99.0), ([60.0, 100.0], 7.0)));
}

#[test]
fn the_snapped_caret_follows_the_page_into_user_space() {
    let text = line("abcd", 20.0, 100.0, 10.0, 14.0, [1.0, 0.0]);
    let page = |rotation, user_unit: f32| PageInfo {
        width: 300.0 * user_unit,
        height: 200.0 * user_unit,
        label: "1".into(),
        crop: [0.0, 0.0, 300.0, 200.0],
        rotation,
    };
    // An unrotated page: view y = 100 is user y = 100 (y up from the bottom of a 200 pt page).
    let (tip, side) = snapped_caret(&page(0, 1.0), &text, 43.0, 95.0).expect("snaps");
    assert!((tip[0] - 40.0).abs() < 1e-3 && (tip[1] - 100.0).abs() < 1e-3 && (side - 7.0).abs() < 1e-3);
    // /UserUnit 2: the view is twice as large, so the same text is half as many user units.
    let doubled = line("abcd", 40.0, 200.0, 20.0, 28.0, [1.0, 0.0]);
    let (tip, side) = snapped_caret(&page(0, 2.0), &doubled, 86.0, 190.0).expect("snaps");
    assert!((tip[0] - 40.0).abs() < 1e-3 && (tip[1] - 100.0).abs() < 1e-3 && (side - 7.0).abs() < 1e-3, "{tip:?} {side}");
    // A rotated page: the caret would lie on its side, so it stays at the click.
    for rotation in [90, 180, 270] {
        assert_eq!(snapped_caret(&page(rotation, 1.0), &text, 43.0, 95.0), None, "/Rotate {rotation}");
    }
}

#[test]
fn away_from_upright_horizontal_text_the_caret_stays_at_the_click() {
    let text = line("abcd", 20.0, 100.0, 10.0, 14.0, [1.0, 0.0]);
    assert_eq!(caret_snap(&text, 200.0, 100.0), None, "far to the right");
    assert_eq!(caret_snap(&text, 40.0, 40.0), None, "far above the line");
    assert_eq!(caret_snap(&PageText::default(), 40.0, 100.0), None, "no text");
    for (direction, what) in [([0.0, 1.0], "vertical"), ([-1.0, 0.0], "upside down"), ([0.995, 0.0998], "sloped")] {
        let turned = line("abcd", 20.0, 100.0, 10.0, 14.0, direction);
        let [x0, y0, x1, y1] = turned.glyphs[2].rect;
        assert_eq!(caret_snap(&turned, (x0 + x1) / 2.0, (y0 + y1) / 2.0), None, "{what}");
    }
}

#[test]
fn recent_custom_colours_dedupe_cap_and_skip_swatches() {
    // #1193: "Other colour…" remembers what it applied, newest first.
    let ctx = egui::Context::default();
    assert!(recent_custom_colours(&ctx).is_empty());
    for i in 0..7 {
        remember_custom_colour(&ctx, [0.1 * f64::from(i), 0.3, 0.7]);
    }
    let recent = recent_custom_colours(&ctx);
    assert_eq!(recent.len(), RECENT_CUSTOM, "capped");
    assert!(same_colour(&recent[0], &[0.6, 0.3, 0.7]), "newest first: {recent:?}");
    remember_custom_colour(&ctx, [0.2, 0.3, 0.7]);
    let again = recent_custom_colours(&ctx);
    assert_eq!(again.len(), RECENT_CUSTOM);
    assert!(
        same_colour(&again[0], &[0.2, 0.3, 0.7]) && again.iter().filter(|c| same_colour(c, &[0.2, 0.3, 0.7])).count() == 1,
        "moved, not repeated"
    );
    // Swatches and unusable values are not remembered; out-of-range ones are clamped.
    remember_custom_colour(&ctx, SWATCHES[0].1);
    remember_custom_colour(&ctx, [f64::NAN, 0.0, 0.0]);
    assert!(same_colour(&recent_custom_colours(&ctx)[0], &[0.2, 0.3, 0.7]));
    remember_custom_colour(&ctx, [1.7, -0.5, 0.25]);
    assert!(same_colour(&recent_custom_colours(&ctx)[0], &[1.0, 0.0, 0.25]));
}

#[test]
fn other_colour_applies_the_picked_colour_once() {
    // #1193: the swatch grid has a full picker; Apply returns its colour and remembers it.
    use egui_kittest::kittest::Queryable;
    let current = [0.2, 0.4, 0.6];
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, picked: &mut Vec<Rgb>| {
            if let Some(c) = swatch_grid(ui, Some(current)) {
                picked.push(c);
            }
        },
        Vec::new(),
    );
    h.run();
    assert!(h.query_by_label("Apply").is_none(), "the picker starts closed");
    assert!(h.query_by_label("Recent colours").is_none());
    h.get_by_label("Other colour…").click();
    h.run();
    h.get_by_label("Apply").click();
    h.run();
    let picked = h.state().clone();
    assert_eq!(picked.len(), 1, "one choice: {picked:?}");
    assert!(same_colour(&picked[0], &current), "the picker opens on the current colour: {picked:?}");
    assert!(h.query_by_label("Apply").is_none(), "applying closes the picker");
    assert!(h.query_by_label("Recent colours").is_some(), "the colour is offered again");
}

#[test]
fn two_grids_in_one_parent_open_their_pickers_separately() {
    // Redaction properties and Prepare a form draw two grids in one parent ui.
    use egui_kittest::kittest::Queryable;
    let mut h = egui_kittest::Harness::new_ui(|ui| {
        let _ = swatch_grid(ui, Some([0.2, 0.4, 0.6]));
        let _ = swatch_grid(ui, Some([0.6, 0.4, 0.2]));
    });
    h.run();
    let plus = h.query_all_by_label("Other colour…").collect::<Vec<_>>();
    assert_eq!(plus.len(), 2);
    plus[0].click();
    h.run();
    assert_eq!(h.query_all_by_label("Apply").count(), 1, "only the first grid's picker opens");
}

#[test]
fn other_colour_opens_the_picker_inside_the_quick_bar_popup() {
    use egui_kittest::kittest::Queryable;
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, prefs: &mut CommentPrefs| {
            let _ = quick_bar_controls(ui, CommentTool::Highlight, prefs);
        },
        CommentPrefs::default(),
    );
    h.run();
    h.get_by_label("Colour").click();
    h.run();
    h.get_by_label("Other colour…").click();
    h.run();
    assert!(h.query_by_label("Apply").is_some(), "the popup stays open with the picker showing");
    h.get_by_label("Apply").click();
    h.run();
    assert!(h.query_by_label("Other colour…").is_none(), "applying a colour closes the popup");
}

#[test]
fn other_colour_opens_the_picker_inside_the_quick_bar_fill_popup() {
    use egui_kittest::kittest::Queryable;
    let prefs = CommentPrefs::default();
    let fill_label = if prefs.style(CommentTool::Rectangle).fill.is_some() { "Fill colour" } else { "Fill colour: none" };
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, prefs: &mut CommentPrefs| {
            let _ = quick_bar_controls(ui, CommentTool::Rectangle, prefs);
        },
        prefs,
    );
    h.ctx.set_fonts(theme::font_definitions());
    h.run();
    h.get_by_label(fill_label).click();
    h.run();
    h.get_by_label("Other colour…").click();
    h.run();
    assert!(h.query_by_label("Apply").is_some(), "the popup stays open with the picker showing");
    h.get_by_label("Apply").click();
    h.run();
    assert!(h.query_by_label("No fill").is_none(), "applying a colour closes the popup");
    assert!(h.state().style(CommentTool::Rectangle).fill.is_some());
}

#[test]
fn context_menu_colour_submenu_keeps_its_picker_open_and_a_swatch_closes_the_menu() {
    use egui_kittest::kittest::Queryable;
    let info = DocInfo {
        pages: vec![PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }],
        annotations: vec![Annotation {
            page: 0,
            subtype: "Square".into(),
            author: None,
            contents: None,
            modified: None,
            name: None,
            in_reply_to: None,
            parent: None,
            grouped: false,
            rect: [10.0, 10.0, 50.0, 50.0],
            color: None,
            index: 0,
            state: None,
            quads: Vec::new(),
            locked: false,
            intent: None,
            fill_sign: false,
            points: Vec::new(),
            no_rotate: false,
        }],
        ..Default::default()
    };
    let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default());
    view.comments.selected = Some((0, 0));
    let prefs = CommentPrefs::default();
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, (view, actions): &mut (DocView, Vec<CanvasAction>)| {
            ui.button("Page").context_menu(|ui| {
                if let Some(a) = context_menu(ui, view, &info, &prefs, true, true) {
                    actions.push(a);
                }
            });
        },
        (view, Vec::new()),
    );
    h.run();
    h.get_by_label("Page").click_secondary();
    h.run();
    h.get_by_label("Colour").click();
    h.run();
    h.get_by_label("Other colour…").click();
    h.run();
    assert!(h.query_by_label("Apply").is_some(), "the submenu stays open with the picker showing");
    h.get_by_label("Red").click();
    h.run();
    assert!(h.query_by_label("Edit text…").is_none(), "picking a swatch closes the whole menu");
    let actions = &h.state().1;
    assert!(
        matches!(actions.as_slice(), [CanvasAction::Edit(e)] if matches!(**e, Edit::StyleAnnotation { page: 0, index: 0, color: Some(_), .. })),
        "{actions:?}"
    );
}

#[test]
fn shift_keeps_the_current_proportions_unless_the_box_has_its_own() {
    let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(196.0, 132.0));
    assert_eq!(corner_ratio(r, None, false), None);
    assert_eq!(corner_ratio(r, None, true), Some(3.0));
    assert_eq!(corner_ratio(r, Some(2.0), false), Some(2.0));
    assert_eq!(corner_ratio(r, Some(2.0), true), Some(2.0));
    // A degenerate box has no proportions to keep (no division by zero).
    let flat = Rect::from_min_max(pos2(100.0, 100.0), pos2(196.0, 100.0));
    assert_eq!(corner_ratio(flat, None, true), None);
    // Dragging a corner with Shift held scales the box instead of stretching it.
    let out = resized(r, (1, 1), vec2(48.0, 0.0), corner_ratio(r, None, true));
    assert_eq!(out.min, r.min);
    assert!((out.width() / out.height() - 3.0).abs() < 0.001);
}

#[test]
fn proportional_corners_keep_the_opposite_anchor_and_edges_stretch_one_axis() {
    let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(196.0, 132.0));
    for (hx, hy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
        for (dx, dy) in [(24.0, 0.0), (0.0, 16.0), (-48.0, -20.0), (-200.0, -200.0)] {
            let out = resized(r, (hx, hy), vec2(f32::from(hx) * dx, f32::from(hy) * dy), Some(3.0));
            assert_eq!(handle_pos(out, (-hx, -hy)), handle_pos(r, (-hx, -hy)));
            assert!((out.width() / out.height() - 3.0).abs() < 0.001);
            assert!(out.width() >= 4.0 && out.height() >= 4.0);
        }
    }
    for handle in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        let out = resized(r, handle, vec2(24.0, 16.0), Some(3.0));
        assert_eq!(out, resized(r, handle, vec2(24.0, 16.0), None));
        assert_eq!(if handle.0 == 0 { out.width() } else { out.height() }, if handle.0 == 0 { r.width() } else { r.height() });
    }
    // Ordinary comment corners keep their existing independent width/height behavior.
    let free = resized(r, (1, 1), vec2(24.0, 0.0), None);
    assert_eq!(free.size(), vec2(120.0, 32.0));
}

#[test]
fn tools_round_trip_through_their_commands() {
    for t in ALL {
        assert_eq!(CommentTool::from_command(t.command()), Some(t));
        assert!(pdfcraft_engine::commands::command(t.command()).is_some(), "{} is registered", t.command());
        assert!(icons::exists(t.icon()), "icon {}", t.icon());
        assert!(GROUPS[t.group()].contains(&t));
    }
}

#[test]
fn drawn_shapes_ignore_slips() {
    assert_eq!(drawn_shape(CommentTool::Rectangle, &[[0.0, 0.0], [1.0, 1.0]]), None);
    assert_eq!(drawn_shape(CommentTool::Rectangle, &[[10.0, 0.0], [0.0, 10.0]]), Some(Shape::Rectangle { rect: [0.0, 0.0, 10.0, 10.0] }));
    assert_eq!(drawn_shape(CommentTool::Ink, &[[0.0, 0.0]]), Some(Shape::Ink { strokes: vec![vec![[0.0, 0.0]]] }));
    assert!(matches!(
        drawn_shape(CommentTool::Arrow, &[[0.0, 0.0], [5.0, 0.0]]),
        Some(Shape::Line { start: LineEnding::None, end: LineEnding::OpenArrow, .. })
    ));
}

#[test]
fn text_boxes_fit_their_text() {
    let r = text_box_rect([100.0, 500.0], "Hi", 12.0, 0);
    assert_eq!((r[0], r[3]), (100.0, 500.0));
    assert!(r[2] - r[0] < 60.0);
    let long = text_box_rect([0.0, 500.0], &"word ".repeat(80), 12.0, 0);
    assert_eq!(long[2], 300.0);
    assert!(long[3] - long[1] > 50.0);
}
