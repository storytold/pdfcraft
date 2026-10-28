use super::geometry::reshaped;
use super::*;

/// [`reshaped`] without Shift, as `changing_points` takes it.
fn straight(points: &[[f64; 2]], i: usize, to: [f64; 2]) -> Vec<[f64; 2]> {
    reshaped(points, i, to, false)
}

#[test]
fn freehand_highlight_and_draw_accept_widths_up_to_100() {
    assert_eq!(CommentTool::FreehandHighlight.group(), CommentTool::Highlight.group());
    assert_eq!(GROUPS.len(), 3);
    let mut prefs = CommentPrefs::default();
    assert_eq!(prefs.style(CommentTool::FreehandHighlight).width, 14.0);
    assert_eq!(prefs.style(CommentTool::FreehandHighlight).opacity, 0.35);
    for tool in [CommentTool::Ink, CommentTool::FreehandHighlight] {
        prefs.set_width(tool, 75.0);
        assert_eq!(prefs.style(tool).width, 75.0);
        prefs.set_width(tool, 200.0);
        assert_eq!(prefs.style(tool).width, 100.0);
        prefs.set_width(tool, f64::NAN);
        assert_eq!(prefs.style(tool).width, 100.0);
    }
}

/// A selected line is drawn where it is going: moved by its body (both ends follow the
/// pointer), with an end dragged (that end alone), and as released until the page is drawn.
#[test]
fn a_moved_or_reshaped_line_is_drawn_at_its_new_points() {
    let line = [[10.0, 10.0], [50.0, 10.0]];
    let to_user = |p: Pos2| [f64::from(p.x), f64::from(p.y)];
    let moving = Gesture::Move { page: 0, index: 3, from: pos2(20.0, 10.0) };
    let at = Some(pos2(25.0, 30.0));
    assert_eq!(changing_points(Some(&moving), None, (0, 3), &line, to_user, at, straight), Some(vec![[15.0, 30.0], [55.0, 30.0]]));
    assert_eq!(changing_points(Some(&moving), None, (0, 4), &line, to_user, at, straight), None, "another comment's move");
    let released = Edit::MoveAnnotation { page: 0, index: 3, dx: 5.0, dy: 20.0 };
    assert_eq!(changing_points(None, Some(&released), (0, 3), &line, to_user, None, straight), Some(vec![[15.0, 30.0], [55.0, 30.0]]));
    let dragging = Gesture::Reshape { page: 0, index: 3, point: 1, points: line.to_vec() };
    let to = Some(pos2(60.0, 10.0));
    assert_eq!(changing_points(Some(&dragging), None, (0, 3), &line, to_user, to, straight), Some(vec![[10.0, 10.0], [60.0, 10.0]]));
    assert_eq!(changing_points(None, None, (0, 3), &line, to_user, to, straight), None, "not changing");
    // Released with the rest of the selection: its own move in the batch.
    let together = Edit::Batch {
        label: "Move comments".into(),
        edits: vec![Edit::MoveAnnotation { page: 0, index: 1, dx: 1.0, dy: 1.0 }, Edit::MoveAnnotation { page: 0, index: 3, dx: 5.0, dy: 20.0 }],
    };
    assert_eq!(changing_points(None, Some(&together), (0, 3), &line, to_user, None, straight), Some(vec![[15.0, 30.0], [55.0, 30.0]]));
    assert_eq!(changing_points(None, Some(&together), (0, 2), &line, to_user, None, straight), None, "not in the batch");
}

/// Review on #4094: a callout's text box being resized, and the resize just released, draw its
/// leader line too (to the box where it is going): its points are changing, though they stay.
#[test]
fn a_callout_being_resized_draws_its_leader_line() {
    let lead = [[10.0, 10.0], [40.0, 30.0]];
    let to_user = |p: Pos2| [f64::from(p.x), f64::from(p.y)];
    let resizing = Gesture::Resize { page: 0, index: 3, handle: (1, 1), from: pos2(80.0, 60.0), aspect_ratio: None };
    let at = Some(pos2(95.0, 70.0));
    assert_eq!(changing_points(Some(&resizing), None, (0, 3), &lead, to_user, at, straight), Some(lead.to_vec()));
    assert_eq!(changing_points(Some(&resizing), None, (0, 4), &lead, to_user, at, straight), None, "another comment's resize");
    let released = Edit::ResizeAnnotation { page: 0, index: 3, rect: [50.0, 20.0, 110.0, 70.0] };
    assert_eq!(changing_points(None, Some(&released), (0, 3), &lead, to_user, None, straight), Some(lead.to_vec()));
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
        text_box: None,
        popup: None,
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
            keys: Default::default(),
        };
        assert_eq!(edited_in_view(&cx, 0, viewport), in_view, "no_rotate: {no_rotate}");
    }
}

/// A listed comment or reply at `index` on `page`, replying to `parent`.
fn listed(page: usize, index: usize, contents: &str, parent: Option<(usize, usize)>) -> Annotation {
    Annotation {
        page,
        subtype: "Text".into(),
        author: None,
        contents: Some(contents.into()),
        modified: None,
        name: None,
        in_reply_to: None,
        parent,
        grouped: false,
        rect: [0.0, 0.0, 20.0, 20.0],
        color: None,
        index,
        state: None,
        quads: Vec::new(),
        locked: false,
        intent: None,
        fill_sign: false,
        no_rotate: false,
        points: Vec::new(),
        text_box: None,
        popup: None,
    }
}

/// What stays selected or open in the editor after a delete is found where it moved to: the
/// reply's own replies go with it (pop-ups, unlisted, are gone too), and a page that lost
/// something else, or whose annotation in that place reads differently, keeps nothing.
#[test]
fn a_kept_comment_is_found_where_a_delete_moved_it() {
    // Page 0's `/Annots`: reply A, its pop-up (unlisted), the comment, reply B, a reply to A.
    let before = vec![
        listed(0, 0, "A", Some((0, 2))),
        listed(0, 2, "comment", None),
        listed(0, 3, "B", Some((0, 2))),
        listed(0, 4, "to A", Some((0, 0))),
        listed(1, 0, "other page", None),
    ];
    // Deleting A takes its pop-up and the reply to it: the comment and B move up two places.
    let after = vec![listed(0, 0, "comment", None), listed(0, 1, "B", Some((0, 0))), listed(1, 0, "other page", None)];
    let gone = (0, 0);
    assert_eq!(after_delete(&before, &after, gone, (0, 2)), Some((0, 0)), "the comment");
    assert_eq!(after_delete(&before, &after, gone, (0, 3)), Some((0, 1)), "the other reply");
    assert_eq!(after_delete(&before, &after, gone, (0, 4)), None, "the reply to A went with it");
    assert_eq!(after_delete(&before, &after, gone, gone), None, "the deleted one");
    assert_eq!(after_delete(&before, &after, gone, (1, 0)), Some((1, 0)), "another page keeps its places");
    // The page lost more than A and its replies: nothing is guessed.
    assert_eq!(after_delete(&before, &after[..1], gone, (0, 2)), None);
    // Another annotation in the comment's place.
    let changed = vec![listed(0, 0, "something else", None), listed(0, 1, "B", Some((0, 0)))];
    assert_eq!(after_delete(&before, &changed, gone, (0, 2)), None);

    let info = DocInfo { annotations: before.clone(), ..Default::default() };
    let mut view = CommentView::from_info(&info);
    view.selected = Some((0, 2));
    view.editing = Some((0, 3, "typed".into()));
    view.annotation_deleted(&before, &DocInfo { annotations: after.clone(), ..Default::default() }, gone);
    view.document_changed(&DocInfo { annotations: after.clone(), ..Default::default() });
    assert_eq!((view.selected, view.editing), (Some((0, 0)), Some((0, 1, "typed".into()))), "both follow");
    let mut view = CommentView::from_info(&info);
    view.editing = Some((0, 4, "typed".into()));
    view.editor_focus = Some(EditorFocus::Take);
    view.annotation_deleted(&before, &DocInfo { annotations: after.clone(), ..Default::default() }, gone);
    assert_eq!((view.editing, view.editor_focus), (None, None), "the editor closes with the reply it was open on");
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
    PageText { glyphs, line_of: vec![0; n], space_before: vec![false; n], truncated: false }
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
        text_box: None,
        popup: None,
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

/// The grid's columns took egui's 40 point minimum though a swatch is 22 wide, so six of
/// them made a 270 point grid in the 246 points of the Edit text panel. The panel's column
/// widened to fit, and everything under the grid was laid out against an edge the panel
/// clips: right-aligned Arabic lost its first letters. Where there is room, the swatches
/// keep their spacing.
#[test]
fn the_swatch_grid_fits_the_width_it_is_given() {
    for (width, gap) in [(246.0, 22.8), (400.0, 24.0)] {
        let mut h = egui_kittest::Harness::builder().with_size(vec2(width + 40.0, 300.0)).build_ui(|ui| {
            ui.set_max_width(width);
            let _ = swatch_grid(ui, None);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("grid-test"), ui.min_rect().width()));
        });
        h.run();
        let used: f32 = h.ctx.data(|d| d.get_temp(egui::Id::unique("grid-test"))).unwrap_or(f32::INFINITY);
        assert!(used <= width, "a {width} point column holds a {used} point grid");
        use egui_kittest::kittest::Queryable;
        let [first, second] = ["Yellow", "Orange"].map(|name| h.get_by_label(name).rect().left());
        assert!(((second - first) - (22.0 + gap)).abs() < 1.0, "{width}: the swatches are {} apart", second - first);
    }
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
            text_box: None,
            popup: None,
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
                let rights = MenuRights { allowed: true, can_paste: false, can_modify: true };
                if let Some(a) = context_menu(ui, view, &info, &|_, _| true, &prefs, rights) {
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
fn shift_click_selects_the_range_from_the_comment_with_the_handles() {
    let order = [(0, 3), (0, 1), (0, 0), (1, 2), (1, 0)];
    let mut cv = CommentView::default();
    cv.select_only((0, 1));
    cv.select_range(&order, (1, 2));
    assert_eq!(cv.selection(), vec![(0, 1), (0, 0), (1, 2)], "from the anchor down, the anchor keeps the handles");
    // Another ⇧-click from the same anchor changes the range, upwards too.
    cv.select_range(&order, (0, 3));
    assert_eq!(cv.selection(), vec![(0, 1), (0, 3)]);
    // Nothing to start from, or a card not in the list: added as by ⌘/Ctrl-click.
    let mut none = CommentView::default();
    none.select_range(&order, (0, 0));
    assert_eq!(none.selection(), vec![(0, 0)]);
    cv.select_range(&order, (9, 9));
    assert_eq!(cv.selected, Some((9, 9)));
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

#[test]
fn markup_comment_markers_skip_rounded_inverted_clamp_bounds() {
    let prefs = CommentPrefs::default();
    let ptr = PagePointer::default();
    let info = DocInfo {
        pages: vec![PageInfo { width: 12.0, height: 12.0, label: String::new(), crop: [0.0, 0.0, 12.0, 12.0], rotation: 0 }],
        ..Default::default()
    };
    let a = Annotation { rect: [0.0, 0.0, 12.0, 12.0], contents: Some("Review".into()), ..annotation("Highlight", None, false) };
    for min in [pos2(1.0e-7, 0.0), pos2(0.0, 1.0e-7)] {
        let page = Rect::from_min_max(min, pos2(12.0, 12.0));
        assert_eq!(page.size(), vec2(12.0, 12.0));
        let marker_max = page.max - vec2(12.0, 12.0);
        assert!(marker_max.x < page.min.x || marker_max.y < page.min.y);
        let xf = PageXform { rect: page, rot: 0, pw: 12.0, ph: 12.0 };
        let cx = PageCx {
            page: 0,
            xf: &xf,
            ptr: &ptr,
            info: &info,
            tool: QuickTool::Select,
            prefs: &prefs,
            allowed: true,
            hidden: false,
            redact_mark: ([0.0; 3], 1.0),
            keys: crate::page_keys::PageKeys::new(false),
        };
        assert!(page.intersects(cx.screen_rects(&a).last().unwrap()));
        assert!(cx.comment_marker_rect(&a).is_none());
    }
}

#[test]
fn markup_comment_markers_stay_inside_turned_and_zoomed_pages() {
    let prefs = CommentPrefs::default();
    for (page_rotation, rotation) in [(0, 0), (90, 0), (180, 0), (270, 0), (0, 90), (0, 180), (0, 270), (90, 270)] {
        for zoom in [0.25, 1.0, 4.0] {
            let (pw, ph) = if page_rotation % 180 == 0 { (300.0, 200.0) } else { (200.0, 300.0) };
            let (w, h) = if rotation % 180 == 0 { (pw, ph) } else { (ph, pw) };
            let xf = PageXform { rect: Rect::from_min_size(pos2(40.0, 60.0), vec2(w, h) * zoom), rot: rotation, pw, ph };
            let info = DocInfo {
                pages: vec![PageInfo { width: pw, height: ph, label: String::new(), crop: [0.0, 0.0, 300.0, 200.0], rotation: page_rotation }],
                ..Default::default()
            };
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
                keys: crate::page_keys::PageKeys::new(false),
            };
            for rect in [[0.0, 180.0, 30.0, 200.0], [270.0, 180.0, 300.0, 200.0], [270.0, 0.0, 300.0, 20.0], [0.0, 0.0, 30.0, 20.0]] {
                let [x0, y0, x1, y1] = rect;
                let mut a = Annotation {
                    page: 0,
                    subtype: "Highlight".into(),
                    author: Some("Ada".into()),
                    contents: Some("Review".into()),
                    modified: None,
                    name: None,
                    in_reply_to: None,
                    parent: None,
                    grouped: false,
                    rect,
                    color: None,
                    index: 0,
                    state: None,
                    quads: vec![[x0, y1, x1, y1, x0, y0, x1, y0]],
                    locked: false,
                    intent: None,
                    fill_sign: false,
                    no_rotate: false,
                    points: Vec::new(),
                    text_box: None,
                    popup: None,
                };
                let marker = cx.comment_marker_rect(&a).expect("commented markup has a marker");
                assert!((marker.width() - 12.0).abs() < 0.001 && (marker.height() - 12.0).abs() < 0.001, "{marker:?}");
                assert!(xf.rect.contains_rect(marker), "page/view rotation {page_rotation}/{rotation}, zoom {zoom}, anchor {rect:?}: {marker:?}");
                let mut top = a.clone();
                top.index = 1;
                // Inspection order can differ from /Annots order. Picking the marker
                // must select the same topmost comment as hovering its marked text.
                let hit_info = DocInfo { annotations: vec![top, a.clone()], ..info.clone() };
                let hit_cx = PageCx { info: &hit_info, ..cx };
                assert_eq!(hit_cx.comment_at(marker.center()).map(|a| a.index), Some(1));
                a.contents = None;
                assert!(cx.comment_marker_rect(&a).is_none(), "plain markup has no marker");
                a.contents = Some("Review".into());
                a.quads = vec![[1000.0, 1010.0, 1010.0, 1010.0, 1000.0, 1000.0, 1010.0, 1000.0]];
                assert!(cx.comment_marker_rect(&a).is_none(), "off-page markup stays off the page");
            }
        }
    }
}

#[test]
fn stale_comment_state_is_cleared_and_delete_never_hits_non_comment_slots() {
    let annot = |index: usize, name: &str| Annotation {
        page: 0,
        index,
        subtype: "Text".into(),
        rect: [10.0, 10.0, 30.0, 30.0],
        color: None,
        author: None,
        contents: Some(name.into()),
        modified: None,
        in_reply_to: None,
        parent: None,
        grouped: false,
        name: Some(name.into()),
        state: None,
        quads: Vec::new(),
        locked: false,
        intent: None,
        fill_sign: false,
        no_rotate: false,
        points: Vec::new(),
        text_box: None,
        popup: None,
    };
    // Page 0 has a Widget at index 0 (not in `info.annotations`), and Text comments at 1 and 2.
    let info = DocInfo {
        pages: vec![PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }],
        annotations: vec![annot(1, "c1"), annot(2, "c2")],
        ..Default::default()
    };
    let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default());
    view.comments.selected = Some((0, 2));
    view.comments.open_editor(0, 2, "draft".into());
    view.comments.composer = Some(Composer { page: 0, at: [10.0, 10.0], kind: ComposerKind::Edit(2), text: "body".into(), focus: false });
    view.comments.gesture = Some(Gesture::Move { page: 0, index: 2, from: pos2(0.0, 0.0) });

    // Moving/editing in place or appending a comment at index 3 preserves (0, 2) selection/editing/gesture.
    let mut appended = info.clone();
    appended.annotations.push(annot(3, "c3"));
    view.document_changed(&appended);
    assert_eq!(view.comments.selected, Some((0, 2)));
    assert!(view.comments.editing.is_some());
    assert!(view.comments.composer.is_some());
    assert!(view.comments.gesture.is_some());

    // Deleting comment 1 shifts comment 2 down to index 1: stale state at (0, 2) and shifted (0, 1) must clear.
    let mut shifted = info.clone();
    shifted.annotations = vec![annot(1, "c2")];
    view.comments.selected = Some((0, 1));
    view.comments.open_editor(0, 1, "for c1, not c2".into());
    view.comments.composer = Some(Composer { page: 0, at: [10.0, 10.0], kind: ComposerKind::Edit(1), text: "for c1".into(), focus: false });
    view.comments.gesture = Some(Gesture::Move { page: 0, index: 1, from: pos2(0.0, 0.0) });
    view.document_changed(&shifted);
    assert_eq!(view.comments.selected, None);
    assert_eq!(view.comments.editing, None);
    assert_eq!(view.comments.composer, None);
    assert_eq!(view.comments.gesture, None);

    // Pressing Delete when `selected` holds a widget index (0, 0) or stale index (0, 2) emits no edit.
    let ctx = egui::Context::default();
    for bad_index in [0, 2] {
        view.comments.selected = Some((0, bad_index));
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut out = ctx.run_ui(input, |ui| {
            let mut tool = QuickTool::Select;
            keys(ui.ctx(), crate::page_keys::PageKeys::new(false), &mut view, &shifted, &mut tool, true);
        });
        out.textures_delta.clear();
        assert_eq!(view.comments.selected, None);
        assert_eq!(view.pending_edit, None, "index {bad_index} must not emit DeleteAnnotation");
    }
}

/// An unnamed comment has no object name to tell it from the one that slid into its index.
/// Deleting the earlier one still drops selection, editing, the composer and a gesture that
/// pointed at that index.
#[test]
fn an_unnamed_shift_clears_state_for_the_old_index() {
    let annot = |index: usize| Annotation {
        page: 0,
        index,
        subtype: "Text".into(),
        rect: [10.0, 10.0, 30.0, 30.0],
        color: None,
        author: None,
        contents: None,
        modified: None,
        in_reply_to: None,
        parent: None,
        grouped: false,
        name: None,
        state: None,
        quads: Vec::new(),
        locked: false,
        intent: None,
        fill_sign: false,
        no_rotate: false,
        points: Vec::new(),
        text_box: None,
        popup: None,
    };
    let info = DocInfo {
        pages: vec![
            PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 },
            PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 },
        ],
        annotations: vec![annot(1), annot(2)],
        ..Default::default()
    };
    let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default());
    view.comments.selected = Some((0, 1));
    view.comments.open_editor(0, 1, "the first note".into());
    view.comments.composer = Some(Composer { page: 0, at: [10.0, 10.0], kind: ComposerKind::Edit(1), text: "the first note".into(), focus: false });
    view.comments.gesture = Some(Gesture::Move { page: 0, index: 1, from: pos2(0.0, 0.0) });
    let mut shifted = info.clone();
    shifted.annotations = vec![annot(1)];
    view.document_changed(&shifted);
    assert_eq!(view.comments.selected, None);
    assert_eq!(view.comments.editing, None);
    assert_eq!(view.comments.composer, None);
    assert_eq!(view.comments.gesture, None);
}

/// Review on #4358: a Replace Text group is selected by its caret, the group's primary (ISO
/// 32000-2 §12.5.6.2), and Make Current Properties Default on it updates the Replace Text tool. A
/// caret of its own (Insert Text) still updates the Insert Text tool.
#[test]
fn a_replace_text_group_makes_replace_text_defaults() {
    let annot = |index: usize, subtype: &str, intent: Option<&str>, parent: Option<(usize, usize)>| Annotation {
        page: 0,
        subtype: subtype.into(),
        author: None,
        contents: None,
        modified: None,
        name: None,
        in_reply_to: None,
        parent,
        grouped: parent.is_some(),
        rect: [10.0, 10.0, 20.0, 20.0],
        color: None,
        index,
        state: None,
        quads: Vec::new(),
        locked: false,
        intent: intent.map(Into::into),
        fill_sign: false,
        no_rotate: false,
        points: Vec::new(),
        text_box: None,
        popup: None,
    };
    let all =
        vec![annot(0, "Caret", Some("Replace"), None), annot(1, "StrikeOut", Some("StrikeOutTextEdit"), Some((0, 0))), annot(2, "Caret", None, None)];
    assert_eq!(tool_for_default(&all, &all[0], None), Some(CommentTool::ReplaceText));
    assert_eq!(tool_for_default(&all, &all[2], None), Some(CommentTool::Caret));
}

/// The hover outline goes round the comment a press under the pointer would take (the top hit
/// there): not a highlight under a form field, which takes the press.
#[test]
fn the_hover_outline_goes_round_what_a_press_would_take() {
    use super::paint::hover_outline;
    use crate::page_cursor::Layer;
    use crate::page_router::{CommentPart, Hit, Owner};
    let page = PageInfo { width: 200.0, height: 200.0, label: String::new(), crop: [0.0, 0.0, 200.0, 200.0], rotation: 0 };
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0)), rot: 0, pw: 200.0, ph: 200.0 };
    let highlight = Annotation { rect: [50.0, 50.0, 150.0, 150.0], ..annotation("Highlight", None, false) };
    let info = DocInfo { pages: vec![page], annotations: vec![highlight], ..Default::default() };
    let prefs = CommentPrefs::default();
    // The pointer is over the highlight.
    let ptr = PagePointer { pos: Some(pos2(100.0, 100.0)), ..PagePointer::default() };
    let cx = PageCx {
        page: 0,
        xf: &xf,
        ptr: &ptr,
        info: &info,
        tool: QuickTool::Select,
        prefs: &prefs,
        allowed: true,
        hidden: false,
        redact_mark: ([0.0; 3], 1.0),
        keys: Default::default(),
    };
    let on = |owner, layer| Hit::new(owner, layer, egui::CursorIcon::PointingHand);
    let comment = on(Owner::Comment { index: 0, part: CommentPart::Body }, Layer::Comment);
    let field = on(Owner::Field { field: 0, widget: 0 }, Layer::Field);
    let cv = CommentView::default();
    assert!(hover_outline(&cx, &cv, Some(&comment)).is_some(), "the highlight a press there selects");
    assert!(hover_outline(&cx, &cv, Some(&field)).is_none(), "not under a field that takes the press");
    assert!(hover_outline(&cx, &cv, None).is_none(), "nor where nothing would take it");
    let selected = CommentView { selected: Some((0, 0)), ..CommentView::default() };
    assert!(hover_outline(&cx, &selected, Some(&comment)).is_none(), "the selected comment has its own frame");
}

#[test]
fn comment_marker_wins_over_an_overlapping_body_for_pick_and_hover() {
    let page = PageInfo { width: 200.0, height: 200.0, label: String::new(), crop: [0.0, 0.0, 200.0, 200.0], rotation: 0 };
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0)), rot: 0, pw: 200.0, ph: 200.0 };
    let markup = Annotation { rect: [50.0, 50.0, 80.0, 65.0], index: 0, ..annotation("Highlight", Some("Anchor"), false) };
    let body = Annotation { rect: [0.0, 0.0, 200.0, 200.0], index: 1, ..annotation("Square", Some("Body"), false) };
    let info = DocInfo { pages: vec![page], annotations: vec![body, markup], ..Default::default() };
    let prefs = CommentPrefs::default();
    let ptr = PagePointer::default();
    let cx = PageCx {
        page: 0,
        xf: &xf,
        ptr: &ptr,
        info: &info,
        tool: QuickTool::Select,
        prefs: &prefs,
        allowed: true,
        hidden: false,
        redact_mark: ([0.0; 3], 1.0),
        keys: crate::page_keys::PageKeys::new(false),
    };
    let marker = cx.comment_marker_rect(&info.annotations[1]).unwrap();
    assert_eq!(cx.comment_at(marker.center()).map(|a| a.index), Some(0));
    let ptr = PagePointer { pos: Some(marker.center()), ..PagePointer::default() };
    let cx = PageCx { ptr: &ptr, ..cx };
    let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default());
    super::input::describe(&cx, &mut view);
    let ctx = egui::Context::default();
    assert_eq!(view.cursor.resolve(&ctx, QuickTool::Select).as_deref(), Some("Highlight\nAnchor"));
    let hidden = PageCx { hidden: true, ..cx };
    assert!(hidden.comment_at(marker.center()).is_none());
    super::input::describe(&hidden, &mut view);
    assert!(view.cursor.resolve(&ctx, QuickTool::Select).is_none());
}
