use super::bridge::{self, PageMap};
use super::*;
use craft::behavior::kurbo::{Point, Vec2};
use craft::pages::ui::PlacedPage;
use craft::review::annotate::{AnnotationEdit, pick_at};
use craft::review::{AnnotationId, ThreadId};

/// The screen size, in points, of a comment marker's icon.
const ICON_POINTS: f32 = 12.0;

/// The side of a comment marker in page units, as the app feeds `bridge`: the icon's screen size through the page map's
/// own conversion, so a change of the conversion is in the test's path.
fn icon_side(map: &PageMap<'_>) -> f64 {
    map.doc_len(ICON_POINTS)
}

/// The screen distance, in points, within which a press picks a comment's body.
const SLOP: f32 = 4.0;

/// The one placed page of a test: `xf`'s rectangle at `zoom`.
fn placed_page(xf: &PageXform, zoom: f64) -> PlacedPage {
    PlacedPage::new(0, xf.rect, zoom, (0.0, xf.rect.height()))
}

/// The page map the app feeds `bridge` for `placed` pages shown turned `rotation` degrees, in a canvas view whose
/// document space is neither the screen nor the page's units, as the page view's is not.
fn page_map<'a>(placed: &'a [PlacedPage], info: &'a DocInfo, rotation: u16) -> PageMap<'a> {
    let view = craft::behavior::CanvasView::new(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 800.0)), Point::new(250.0, 300.0), 0.75);
    PageMap::new(info, placed, rotation, view)
}

/// A rectangle in page units of `page` on screen, through the page map, for comparing a marker with the page's
/// screen rectangle.
fn screen(map: &PageMap<'_>, page: usize, r: craft::behavior::kurbo::Rect) -> Rect {
    let corner = |x, y| map.to_screen(page, Point::new(x, y)).expect("the page is placed");
    Rect::from_two_pos(corner(r.x0, r.y0), corner(r.x1, r.y1))
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
        let cx = PageCx { page: 0, xf: &xf, info: &info, prefs: &prefs, allowed: true, hidden: false };
        assert_eq!(super::composer::edited_in_view(&cx, 0, viewport), in_view, "no_rotate: {no_rotate}");
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
    let moved = DocInfo { annotations: after.clone(), ..Default::default() };
    let thread = |key: (usize, usize)| ThreadId(bridge::id_of(key.0, key.1).0);
    let mut view = CommentView::from_info(&info);
    view.select_only((0, 2));
    view.panel.editing = Some((thread((0, 3)), 0, "typed".into()));
    view.annotation_deleted(&before, &moved, gone);
    view.document_changed(&moved);
    assert_eq!(view.selection(), vec![(0, 0)], "the selection follows");
    let (editing, _, text) = view.panel.editing.clone().expect("the editor stays open");
    assert_eq!((bridge::key_of(AnnotationId(editing.0)), text.as_str()), ((0, 1), "typed"), "the editor follows");
    let mut view = CommentView::from_info(&info);
    view.panel.editing = Some((thread((0, 4)), 0, "typed".into()));
    view.annotation_deleted(&before, &moved, gone);
    assert_eq!(view.panel.editing, None, "the editor closes with the reply it was open on");
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
    use super::SWATCHES;
    use super::colours::{RecentColours, to_srgba};
    use craft::theme::Srgba;
    let same = |a: &Srgba, b: [f64; 3]| {
        let b = to_srgba(b);
        (a.r - b.r).abs() < 1e-3 && (a.g - b.g).abs() < 1e-3 && (a.b - b.b).abs() < 1e-3
    };
    // #1193: "Other colour…" remembers what it applied, newest first.
    let recent = RecentColours::default();
    assert!(recent.as_srgba().is_empty());
    for i in 0..7 {
        recent.remember([0.1 * f64::from(i), 0.3, 0.7]);
    }
    let list = recent.as_srgba();
    assert_eq!(list.len(), 5, "capped");
    assert!(same(&list[0], [0.6, 0.3, 0.7]), "newest first: {list:?}");
    recent.remember([0.2, 0.3, 0.7]);
    let again = recent.as_srgba();
    assert_eq!(again.len(), 5);
    assert!(same(&again[0], [0.2, 0.3, 0.7]) && again.iter().filter(|c| same(c, [0.2, 0.3, 0.7])).count() == 1, "moved, not repeated");
    // Swatches and unusable values are not remembered; out-of-range ones are clamped.
    recent.remember(SWATCHES[0].1);
    recent.remember([f64::NAN, 0.0, 0.0]);
    assert!(same(&recent.as_srgba()[0], [0.2, 0.3, 0.7]));
    recent.remember([1.7, -0.5, 0.25]);
    assert!(same(&recent.as_srgba()[0], [1.0, 0.0, 0.25]));
}

#[test]
fn tools_round_trip_through_their_commands() {
    icons::register();
    // PLAN: assumed `super::annotate_icons() -> AnnotateIcons`, the app's CM-C31 supply built from `CommentTool::icon`.
    let supplied = super::annotate_icons();
    for t in ALL {
        assert_eq!(CommentTool::from_command(t.command()), Some(t));
        assert!(pdfcraft_engine::commands::command(t.command()).is_some(), "{} is registered", t.command());
        let icon = supplied.icon(craft::review::ui::annotate::AnnotateCommand::of(t.tool())).expect("the app names every tool's icon");
        assert!(craft::ui::primitives::has_icon(&craft::behavior::IconId::from(std::borrow::Cow::Owned(icon.to_string()))), "icon {icon}");
        assert!(GROUPS[t.group()].contains(&t));
    }
}

/// A saved arrow without its head compiles and passes every other check: `Create` maps an Arrow to a line ending in an
/// open arrow, and a plain Line to one with no end.
#[test]
fn an_arrow_is_created_as_a_line_with_an_open_arrow_end() {
    let page = PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 };
    let info = DocInfo { pages: vec![page], ..Default::default() };
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(300.0, 400.0)), rot: 0, pw: 300.0, ph: 400.0 };
    let placed = [placed_page(&xf, 1.0)];
    let map = page_map(&placed, &info, 0);
    let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default(), false);
    let prefs = CommentPrefs::default();
    let mut created = |kind| {
        let drawn = craft::review::Annotation {
            id: bridge::id_of(0, 0),
            page: 0,
            kind,
            rect: craft::behavior::kurbo::Rect::new(10.0, 10.0, 60.0, 10.0),
            points: vec![Point::new(10.0, 10.0), Point::new(60.0, 10.0)],
            strokes: Vec::new(),
            color: [255, 0, 0],
            opacity: 1.0,
            stroke_width: 2.0,
            text: String::new(),
            locked: false,
            aspect: None,
            handle_rule: Default::default(),
        };
        bridge::edit(AnnotationEdit::Create(drawn), &map, &mut view.session.comments, &info, &prefs)
    };
    assert!(matches!(
        created(craft::review::AnnotationKind::Arrow),
        Some(Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            shape: Shape::Line { start: LineEnding::None, end: LineEnding::OpenArrow, .. },
            ..
        }))
    ));
    assert!(matches!(
        created(craft::review::AnnotationKind::Line),
        Some(Edit::AddAnnotation(pdfcraft_engine::NewAnnotation { shape: Shape::Line { end: LineEnding::None, .. }, .. }))
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
    assert_eq!(cv.primary(), Some((9, 9)));
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

/// A commented markup's marker lies inside its page for every page and view rotation and zoom, picking the marker takes
/// the topmost comment, and plain or off-page markup has none.
#[test]
fn markup_comment_markers_stay_inside_turned_and_zoomed_pages() {
    let tolerance_of = |map: &PageMap<'_>| map.doc_len(SLOP);
    for (page_rotation, rotation) in [(0, 0), (90, 0), (180, 0), (270, 0), (0, 90), (0, 180), (0, 270), (90, 270)] {
        for zoom in [0.25, 1.0, 4.0] {
            let (pw, ph) = if page_rotation % 180 == 0 { (300.0, 200.0) } else { (200.0, 300.0) };
            let (w, h) = if rotation % 180 == 0 { (pw, ph) } else { (ph, pw) };
            let xf = PageXform { rect: Rect::from_min_size(pos2(40.0, 60.0), vec2(w, h) * zoom), rot: rotation, pw, ph };
            let info = DocInfo {
                pages: vec![PageInfo { width: pw, height: ph, label: String::new(), crop: [0.0, 0.0, 300.0, 200.0], rotation: page_rotation }],
                ..Default::default()
            };
            let placed = [placed_page(&xf, f64::from(zoom))];
            let map = page_map(&placed, &info, rotation as u16);
            let side = icon_side(&map);
            let with = |annotations: Vec<Annotation>| DocInfo { annotations, ..info.clone() };
            for rect in [[0.0, 180.0, 30.0, 200.0], [270.0, 180.0, 300.0, 200.0], [270.0, 0.0, 300.0, 20.0], [0.0, 0.0, 30.0, 20.0]] {
                let [x0, y0, x1, y1] = rect;
                let a = Annotation {
                    page: 0,
                    subtype: "Highlight".into(),
                    author: Some("Ada".into()),
                    contents: Some("Review".into()),
                    rect,
                    quads: vec![[x0, y1, x1, y1, x0, y0, x1, y0]],
                    ..annotation("Highlight", None, false)
                };
                // Inspection order can differ from /Annots order: picking the marker must select the same topmost
                // comment as hovering its marked text.
                let top = Annotation { index: 1, ..a.clone() };
                let both = with(vec![top, a.clone()]);
                let markers = bridge::markers(&both, &map, side);
                let (_, marker) = *markers.last().expect("commented markup has a marker");
                let inside = screen(&map, 0, marker);
                assert!(xf.rect.contains_rect(inside), "page/view rotation {page_rotation}/{rotation}, zoom {zoom}, anchor {rect:?}: {inside:?}");
                let annotations = bridge::annotations(&both, &map, QuickTool::Select, true);
                let picked = pick_at(&annotations, marker.center(), tolerance_of(&map), side, &markers);
                assert_eq!(picked.map(|id| bridge::key_of(id).1), Some(1));
                let plain = Annotation { contents: None, ..a.clone() };
                assert!(bridge::markers(&with(vec![plain]), &map, side).is_empty(), "plain markup has no marker");
                let off = Annotation { quads: vec![[1000.0, 1010.0, 1010.0, 1010.0, 1000.0, 1000.0, 1010.0, 1000.0]], ..a };
                assert!(bridge::markers(&with(vec![off]), &map, side).is_empty(), "off-page markup stays off the page");
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
        contents: Some(name.into()),
        name: Some(name.into()),
        ..annotation("Text", None, false)
    };
    // Page 0 has a Widget at index 0 (not in `info.annotations`), and Text comments at 1 and 2.
    let info = DocInfo {
        pages: vec![PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }],
        annotations: vec![annot(1, "c1"), annot(2, "c2")],
        ..Default::default()
    };
    let thread = |index: usize| ThreadId(bridge::id_of(0, index).0);
    let composer =
        |index: usize, text: &str| Composer { page: 0, at: [10.0, 10.0], kind: ComposerKind::Edit(index), text: text.into(), focus: false };
    let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default(), false);
    view.session.comments.select_only((0, 2));
    view.session.comments.panel.editing = Some((thread(2), 0, "draft".into()));

    view.session.comments.composer = Some(composer(2, "body"));

    // Moving/editing in place or appending a comment at index 3 preserves the (0, 2) selection, editor and composer.
    let mut appended = info.clone();
    appended.annotations.push(annot(3, "c3"));
    view.document_changed(&egui::Context::default(), &appended);
    assert_eq!(view.session.comments.selection(), vec![(0, 2)]);
    assert!(view.session.comments.panel.editing.is_some());
    assert!(view.session.comments.composer.is_some());

    // Deleting comment 1 shifts comment 2 down to index 1: stale state at (0, 2) and shifted (0, 1) must clear.
    let mut shifted = info.clone();
    shifted.annotations = vec![annot(1, "c2")];
    view.session.comments.select_only((0, 1));
    view.session.comments.panel.editing = Some((thread(1), 0, "for c1, not c2".into()));
    view.session.comments.composer = Some(composer(1, "for c1"));
    view.document_changed(&egui::Context::default(), &shifted);
    assert!(view.session.comments.selection().is_empty());
    assert_eq!(view.session.comments.panel.editing, None);
    assert_eq!(view.session.comments.composer, None);

    // Deleting a widget slot (0, 0) or a stale slot (0, 2) asks the engine for nothing.
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(300.0, 400.0)), rot: 0, pw: 300.0, ph: 400.0 };
    let placed = [placed_page(&xf, 1.0)];
    let map = page_map(&placed, &shifted, 0);
    let prefs = CommentPrefs::default();
    for bad_index in [0, 2] {
        let edit = bridge::edit(AnnotationEdit::Delete(vec![bridge::id_of(0, bad_index)]), &map, &mut view.session.comments, &shifted, &prefs);
        assert_eq!(edit, None, "index {bad_index} must not emit DeleteAnnotation");
    }
}

/// An unnamed comment has no object name to tell it from the one that slid into its index.
/// Deleting the earlier one still drops selection, editing and the composer that pointed at that index.
#[test]
fn an_unnamed_shift_clears_state_for_the_old_index() {
    let annot = |index: usize| Annotation { page: 0, index, rect: [10.0, 10.0, 30.0, 30.0], ..annotation("Text", None, false) };
    let page = || PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 };
    let info = DocInfo { pages: vec![page(), page()], annotations: vec![annot(1), annot(2)], ..Default::default() };
    let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default(), false);
    view.session.comments.select_only((0, 1));
    view.session.comments.panel.editing = Some((ThreadId(bridge::id_of(0, 1).0), 0, "the first note".into()));
    view.session.comments.composer =
        Some(Composer { page: 0, at: [10.0, 10.0], kind: ComposerKind::Edit(1), text: "the first note".into(), focus: false });
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(300.0, 400.0)), rot: 0, pw: 300.0, ph: 400.0 };
    let placed = [placed_page(&xf, 1.0)];
    let map = page_map(&placed, &info, 0);
    let deleted = bridge::edit(AnnotationEdit::Delete(vec![bridge::id_of(0, 1)]), &map, &mut view.session.comments, &info, &CommentPrefs::default());
    assert!(deleted.is_some(), "the unnamed comment is deletable");
    let mut shifted = info.clone();
    shifted.annotations = vec![annot(1)];
    view.document_changed(&egui::Context::default(), &shifted);
    assert!(view.session.comments.selection().is_empty());
    assert_eq!(view.session.comments.panel.editing, None);
    assert_eq!(view.session.comments.composer, None);
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

/// The one part the page registers at the pointer is for the comment a press there would take (the same `pick_at`
/// annotation), and none where nothing would take it.
#[test]
fn the_hover_outline_goes_round_what_a_press_would_take() {
    let page = PageInfo { width: 200.0, height: 200.0, label: String::new(), crop: [0.0, 0.0, 200.0, 200.0], rotation: 0 };
    let xf = PageXform { rect: Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0)), rot: 0, pw: 200.0, ph: 200.0 };
    let highlight = Annotation { rect: [50.0, 50.0, 150.0, 150.0], ..annotation("Highlight", None, false) };
    let info = DocInfo { pages: vec![page], annotations: vec![highlight], ..Default::default() };
    let prefs = CommentPrefs::default();
    let placed = [placed_page(&xf, 1.0)];
    let map = page_map(&placed, &info, 0);
    let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default(), false);
    let tolerance = map.doc_len(SLOP);
    let side = icon_side(&map);
    let over = Point::new(100.0, 100.0);
    let annotations = bridge::annotations(&info, &map, QuickTool::Select, true);
    let markers = bridge::markers(&info, &map, side);
    let at = map.to_page(0, over).expect("the page is placed");
    assert_eq!(pick_at(&annotations, at, tolerance, side, &markers), Some(bridge::id_of(0, 0)), "the highlight a press there selects");
    // PLAN: the outline shape itself is the overlay's `HoverOutline` (pdf-comments-w8); a request to w8 for an `overlay` helper
    // the test can compare against the part's annotation. The part registered at the pointer is asserted here.
    let parts =
        |view: &DocView, pointer| bridge::parts(&info, &map, &prefs, QuickTool::Select, pointer, &view.session.comments.tool, tolerance, side, false);
    assert_eq!(parts(&view, Some(over)).len(), 1, "one part for the comment under the pointer");
    assert!(parts(&view, None).is_empty(), "no pointer, no part");
    assert!(parts(&view, Some(Point::new(5.0, 5.0))).is_empty(), "nor where nothing would take the press");
    // A selected comment keeps its part, so its tip shows as production's did; the overlay draws no outline round it
    // (the handles are what is there), and a drag on it still passes to the tool.
    view.session.comments.select_only((0, 0));
    assert_eq!(parts(&view, Some(over)).len(), 1, "the selected comment is still pressed and tipped where it is");
}

/// A silent transform error would carry into every saved comment: the user point survives the trip through the document
/// space on a turned page, a turned view and a zoomed page, and a move by one document unit moves the annotation by the
/// same distance in user space.
#[test]
fn an_annotation_round_trips_through_the_page_map_on_turned_and_zoomed_pages() {
    for (page_rotation, rotation, zoom) in [(90, 0, 1.0), (0, 90, 1.0), (0, 0, 4.0)] {
        let (pw, ph) = if page_rotation % 180 == 0 { (300.0, 200.0) } else { (200.0, 300.0) };
        let (w, h) = if rotation % 180 == 0 { (pw, ph) } else { (ph, pw) };
        let xf = PageXform { rect: Rect::from_min_size(pos2(40.0, 60.0), vec2(w, h) * zoom), rot: rotation, pw, ph };
        let square = Annotation { rect: [40.0, 60.0, 100.0, 100.0], ..annotation("Square", Some("Box"), false) };
        let info = DocInfo {
            pages: vec![PageInfo { width: pw, height: ph, label: String::new(), crop: [0.0, 0.0, 300.0, 200.0], rotation: page_rotation }],
            annotations: vec![square],
            ..Default::default()
        };
        let placed = [placed_page(&xf, f64::from(zoom))];
        let map = page_map(&placed, &info, rotation as u16);
        let case = format!("page/view rotation {page_rotation}/{rotation}, zoom {zoom}");
        for user in [[10.0, 20.0], [250.0, 150.0]] {
            let back = map.to_user(0, map.to_doc(0, user));
            assert!((back[0] - user[0]).abs() < 1e-6 && (back[1] - user[1]).abs() < 1e-6, "{case}: {user:?} came back as {back:?}");
        }
        let mut view = DocView::new(pdfcraft_engine::DocId(1), &info, crate::canvas::ViewDefaults::default(), false);
        let delta = Vec2::new(1.0, 0.0);
        let moved = bridge::edit(
            AnnotationEdit::Move { ids: vec![bridge::id_of(0, 0)], delta },
            &map,
            &mut view.session.comments,
            &info,
            &CommentPrefs::default(),
        );
        let Some(Edit::MoveAnnotation { dx, dy, .. }) = moved else { panic!("{case}: a move is one MoveAnnotation, got {moved:?}") };
        let origin = map.to_doc(0, [50.0, 80.0]);
        let (from, to) = (map.to_user(0, origin), map.to_user(0, origin + delta));
        assert!((dx - (to[0] - from[0])).abs() < 1e-6 && (dy - (to[1] - from[1])).abs() < 1e-6, "{case}: moved ({dx}, {dy})");
    }
}

/// A hand-written format boundary: a wrong zone shifts every shown age and date.
#[test]
fn a_pdf_date_with_a_zone_reads_as_the_same_instant_in_unix_seconds() {
    let zoned = bridge::pdf_date_seconds("D:20240102030405+02'00'");
    assert!(zoned.is_some(), "a zoned date reads");
    assert_eq!(zoned, bridge::pdf_date_seconds("D:20240102010405Z"), "the same instant in UTC");
    let day = bridge::pdf_date_seconds("D:20240102");
    assert!(day.is_some());
    assert_eq!(day, bridge::pdf_date_seconds("D:20240102000000Z"), "no time part reads midnight UTC");
    assert_eq!(bridge::pdf_date_seconds("D:20240103").zip(day).map(|(next, this)| next - this), Some(86_400));
    assert_eq!(bridge::pdf_date_seconds("not a date"), None);
}
