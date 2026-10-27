use super::*;
use egui::{Context, Modifiers, Sense, TouchDeviceId, TouchId, TouchPhase, pos2, vec2};

/// The pages' widget: x 0..200, y 0..200 of a 400×400 screen.
const PAGES: Rect = Rect { min: Pos2::ZERO, max: Pos2 { x: 200.0, y: 200.0 } };
/// An Area drawn over the pages' lower right corner.
const OVER: Rect = Rect { min: Pos2 { x: 120.0, y: 120.0 }, max: Pos2 { x: 260.0, y: 260.0 } };

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

fn touch(phase: TouchPhase, pos: Pos2) -> Event {
    Event::Touch { device_id: TouchDeviceId(1), id: TouchId(1), phase, pos, force: None }
}

/// Runs one frame of the pages' widget (and the Area over it, with `area`) and returns what
/// the pages see.
fn frame(ctx: &Context, events: Vec<Event>, area: bool) -> PagePointer {
    frame_with(ctx, events, area, false)
}

/// [`frame`], with a dialog open (`modal`) whose layer egui has not met.
fn frame_with(ctx: &Context, events: Vec<Event>, area: bool, modal: bool) -> PagePointer {
    let mut seen = PagePointer::default();
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0))), events, ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| {
        let (_, resp) = ui.allocate_exact_size(PAGES.size(), Sense::click_and_drag());
        seen = PagePointer::read(ui, &resp, modal);
        if area {
            egui::Area::new(egui::Id::unique("over")).fixed_pos(OVER.min).show(ui.ctx(), |ui| {
                ui.allocate_exact_size(OVER.size(), Sense::hover());
            });
        }
    });
    // An input-only test has no renderer to take the font texture.
    output.textures_delta.clear();
    seen
}

#[test]
fn a_touch_tap_keeps_its_position_on_the_frame_it_lifts() {
    let ctx = Context::default();
    let p = pos2(50.0, 60.0);
    frame(&ctx, vec![], false);
    let down = frame(&ctx, vec![touch(TouchPhase::Start, p), Event::PointerMoved(p), button(p, PointerButton::Primary, true)], false);
    assert_eq!((down.pos, down.origin, down.pressed()), (Some(p), Some(p), Some(p)));
    // egui-winit ends a touch with the release and `PointerGone` in one frame.
    let up = frame(&ctx, vec![touch(TouchPhase::End, p), button(p, PointerButton::Primary, false), Event::PointerGone], false);
    assert_eq!(ctx.input(|i| i.pointer.hover_pos()), None, "egui's hover position is gone");
    assert_eq!(up.pos, Some(p), "the pages still know where the finger lifted");
    assert_eq!(up.edges, [Edge::Release(p)]);
    assert_eq!(frame(&ctx, vec![], false), PagePointer::default(), "and forget it the frame after");
}

#[test]
fn a_press_and_release_in_one_frame_keep_the_press() {
    let ctx = Context::default();
    let p = pos2(50.0, 60.0);
    frame(&ctx, vec![Event::PointerMoved(p)], false);
    let click = frame(&ctx, vec![button(p, PointerButton::Primary, true), button(p, PointerButton::Primary, false)], false);
    assert_eq!(ctx.input(|i| i.pointer.press_origin()), None, "egui has forgotten the press");
    assert_eq!(click.origin, Some(p));
    assert_eq!(click.edges, [Edge::Press(p), Edge::Release(p)]);
}

#[test]
fn something_drawn_over_the_pages_hides_the_pointer() {
    let ctx = Context::default();
    // The Area and the widget rects egui hit-tests against come from the frame before.
    frame(&ctx, vec![], true);
    frame(&ctx, vec![], true);
    let beside = pos2(100.0, 100.0);
    assert_eq!(frame(&ctx, vec![Event::PointerMoved(beside)], true).pos, Some(beside));
    // Inside the Area, and just inside its edge where egui's interact radius still finds the
    // pages' widget.
    for p in [pos2(160.0, 160.0), pos2(122.0, 150.0)] {
        assert_eq!(frame(&ctx, vec![Event::PointerMoved(p)], true).pos, None, "{p:?}");
    }
    // Off the pages altogether.
    assert_eq!(frame(&ctx, vec![Event::PointerMoved(pos2(300.0, 50.0))], true).pos, None);
    // A press on the Area is not the pages' press, though the button is down all the same.
    let p = pos2(160.0, 160.0);
    let press = frame(&ctx, vec![button(p, PointerButton::Primary, true)], true);
    assert_eq!(press, PagePointer { primary_down: true, any_down: true, ..PagePointer::default() });
}

#[test]
fn a_press_on_the_pages_keeps_the_pointer_until_its_release() {
    let ctx = Context::default();
    frame(&ctx, vec![], true);
    frame(&ctx, vec![], true);
    let start = pos2(40.0, 40.0);
    frame(&ctx, vec![Event::PointerMoved(start), button(start, PointerButton::Primary, true)], true);
    // Dragged over the Area and off the pages: still theirs.
    for p in [pos2(160.0, 160.0), pos2(300.0, 50.0)] {
        let held = frame(&ctx, vec![Event::PointerMoved(p)], true);
        assert_eq!((held.pos, held.origin, held.primary_down), (Some(p), Some(start), true), "{p:?}");
    }
    let end = pos2(300.0, 60.0);
    let up = frame(&ctx, vec![Event::PointerMoved(end), button(end, PointerButton::Primary, false)], true);
    assert_eq!((up.pos, up.edges.as_slice()), (Some(end), [Edge::Release(end)].as_slice()), "the release is the pages' too");
    assert!(!up.primary_down && !up.any_down, "and the button is up");
    assert_eq!(frame(&ctx, vec![], true).pos, None, "then off the pages, the pointer is not theirs");
}

#[test]
fn a_dialog_hides_the_pointer_and_its_clicks_but_not_a_drag_the_pages_have() {
    let ctx = Context::default();
    let p = pos2(50.0, 50.0);
    frame(&ctx, vec![Event::PointerMoved(p)], false);
    assert_eq!(frame(&ctx, vec![], false).pos, Some(p));
    assert_eq!(frame_with(&ctx, vec![], false, true).pos, None, "no pointer under a dialog, before egui knows its layer too");
    let click = frame_with(&ctx, vec![button(p, PointerButton::Primary, true), button(p, PointerButton::Primary, false)], false, true);
    assert_eq!((click.pos, click.origin, click.edges.as_slice()), (None, None, [].as_slice()), "nor a click egui gives the pages then");
    // A drag the pages already have when a dialog opens goes on until its release.
    frame(&ctx, vec![button(p, PointerButton::Primary, true)], false);
    frame(&ctx, vec![Event::PointerMoved(pos2(80.0, 50.0))], false);
    let q = pos2(110.0, 50.0);
    let dragging = frame_with(&ctx, vec![Event::PointerMoved(q)], false, true);
    assert_eq!((dragging.pos, dragging.origin), (Some(q), Some(p)), "a drag the pages have keeps its pointer under a dialog");
    let up = frame_with(&ctx, vec![button(q, PointerButton::Primary, false)], false, true);
    assert_eq!(up.edges.as_slice(), [Edge::Release(q)].as_slice(), "and its release");
}

#[test]
fn a_secondary_press_is_some_button_down_but_no_primary_edge() {
    let ctx = Context::default();
    let p = pos2(50.0, 60.0);
    frame(&ctx, vec![Event::PointerMoved(p)], false);
    let right = frame(&ctx, vec![button(p, PointerButton::Secondary, true)], false);
    assert_eq!((right.pos, right.any_down, right.primary_down, right.edges.len()), (Some(p), true, false, 0));
}

#[test]
fn edges_are_primary_finite_in_order_and_capped() {
    let p = pos2(1.0, 2.0);
    let q = pos2(3.0, 4.0);
    let events = vec![
        button(p, PointerButton::Secondary, true),
        button(p, PointerButton::Primary, true),
        Event::PointerMoved(q),
        button(pos2(f32::NAN, 0.0), PointerButton::Primary, false),
        button(q, PointerButton::Primary, false),
    ];
    assert_eq!(edges(&events), [Edge::Press(p), Edge::Release(q)]);
    let many: Vec<Event> = (0..100).map(|i| button(p, PointerButton::Primary, i % 2 == 0)).collect();
    assert_eq!(edges(&many).len(), MAX_EDGES);
    assert_eq!(first_press(&[Edge::Release(q), Edge::Press(p), Edge::Press(q)]), Some(p));
}

/// The page modules: the per-page chain in `canvas::document_area` and what it calls.
const PAGE_MODULES: [&str; 35] = [
    "canvas.rs",
    "canvas/area.rs",
    "canvas/bars.rs",
    "canvas/cache.rs",
    "canvas/find.rs",
    "canvas/navigation.rs",
    "canvas/organize.rs",
    "canvas/selection.rs",
    "canvas/xform.rs",
    "canvas/zoom.rs",
    "tools/comments.rs",
    "tools/comments/composer.rs",
    "tools/comments/geometry.rs",
    "tools/comments/input.rs",
    "tools/comments/menu.rs",
    "tools/comments/paint.rs",
    "tools/comments/state.rs",
    "tools/forms_ui.rs",
    "tools/prepare.rs",
    "tools/fill_sign.rs",
    "tools/fill_sign/scan.rs",
    "tools/sign_ui.rs",
    "canvas/crop.rs",
    "canvas/zoom_snap.rs",
    "tools/camera_ui.rs",
    "tools/measure_ui.rs",
    "tools/redact_ui.rs",
    "tools/link_ui.rs",
    "tools/content_ui.rs",
    "tools/edit_text_ui.rs",
    "tools/edit_text_ui/image.rs",
    "tools/object_ui.rs",
    "canvas/image_pick.rs",
    "canvas/signature_drag.rs",
    "tools/calc_order_ui.rs",
];

/// egui's raw pointer state: where the pointer is, and which buttons are down or went down.
const RAW_READS: [&str; 21] = [
    "hover_pos()",
    "interact_pos()",
    "latest_pos()",
    "press_origin()",
    "pointer_hover_pos()",
    "pointer_interact_pos()",
    "pointer_latest_pos()",
    "primary_down()",
    "primary_pressed()",
    "primary_released()",
    "primary_clicked()",
    "any_down()",
    "any_pressed()",
    "any_released()",
    "any_click()",
    // These take the button, so they match up to the parenthesis.
    "button_down(",
    "button_pressed(",
    "button_released(",
    "button_clicked(",
    "button_double_clicked(",
    "button_triple_clicked(",
];

/// Functions in the page modules that are not page input and read egui's pointer state
/// themselves: (file, function, why).
const NOT_PAGE_INPUT: [(&str, &str, &str); 11] = [
    ("canvas/area.rs", "zoom_anchor", "a pinch zooms the whole view, and is read before the pages' widget exists"),
    ("canvas/area.rs", "drawing_edge_pointer", "scrolls the view at its edges, read before the pages' widget exists"),
    ("canvas/area.rs", "glide_interrupted", "a press anywhere stops the touchpad's glide"),
    ("canvas/area.rs", "presentation_click", "full screen's click to turn the page, read before the pages' widget exists"),
    ("canvas/bars.rs", "page_size_hint", "the page size hint sits in the view's corner, not on a page"),
    ("canvas/organize.rs", "organize_grid", "Organize Pages shows thumbnails, not pages"),
    ("tools/comments/input.rs", "keys", "keyboard handling after the pages: a drag cancelled with Escape ends once no button is down"),
    ("tools/forms_ui.rs", "overlay", "the field editor is an Area of its own over the page"),
    ("tools/content_ui.rs", "editor", "the added-text editor is an Area of its own over the page"),
    ("tools/edit_text_ui.rs", "overlay", "the paragraph editor is an Area of its own over the page"),
    ("tools/fill_sign/scan.rs", "scan_tab", "the signature dialog's Scan tab"),
];

/// The function a `fn` line declares, laid out as rustfmt does (`pub`, `const` and the like
/// before `fn`).
fn declares(line: &str) -> Option<&str> {
    let mut rest = line;
    loop {
        let before = rest;
        for prefix in ["pub(crate) ", "pub(super) ", "pub ", "const ", "async ", "unsafe "] {
            rest = rest.strip_prefix(prefix).unwrap_or(rest);
        }
        if rest == before {
            break;
        }
    }
    let rest = rest.strip_prefix("fn ")?;
    rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|end| &rest[..end])
}

/// Whether `line` calls `read`, and not a longer name that ends in it.
fn reads(line: &str, read: &str) -> bool {
    line.match_indices(read).any(|(at, _)| !line[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
}

/// Page modules read the pointer only through `PagePointer`. egui's raw state misses what is
/// drawn over the pages and where a touch lifted, so a raw read in a page module brings back
/// the bugs `PagePointer` fixed: a cursor or a press through a dialog, a tap that does nothing.
#[test]
fn page_modules_read_the_pointer_only_through_page_pointer() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut problems = Vec::new();
    let mut needed = vec![false; NOT_PAGE_INPUT.len()];
    for file in PAGE_MODULES {
        let source = std::fs::read_to_string(src.join(file)).expect("page module").replace("\r\n", "\n");
        // Tests drive egui's input themselves.
        let code = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
        // The top-level function, or method of an `impl`, that each line is in.
        let (mut function, mut in_impl) = (None, false);
        for (n, line) in code.lines().enumerate() {
            if line.starts_with("impl") {
                (function, in_impl) = (None, true);
            } else if line == "}" {
                (function, in_impl) = (None, false);
            } else if let Some(name) = declares(line).or_else(|| line.strip_prefix("    ").filter(|_| in_impl).and_then(declares)) {
                function = Some(name);
            }
            if line.trim_start().starts_with("//") {
                continue;
            }
            for read in RAW_READS.into_iter().filter(|read| reads(line, read)) {
                match NOT_PAGE_INPUT.iter().position(|(f, name, _)| *f == file && function == Some(*name)) {
                    Some(k) => needed[k] = true,
                    None => problems.push(format!(
                        "src/{file}:{}: `{read}` reads egui's pointer state in a page module. Read `PagePointer` \
                             (src/page_pointer.rs) instead: it knows what is drawn over the pages and where a touch \
                             lifted. Not page input? Add the function to NOT_PAGE_INPUT there, with why.",
                        n + 1
                    )),
                }
            }
        }
    }
    for ((file, name, _), needed) in NOT_PAGE_INPUT.iter().zip(needed) {
        if !needed {
            problems.push(format!("src/{file}: fn {name} reads no pointer state any more: remove it from NOT_PAGE_INPUT."));
        }
    }
    // A module that takes the page pointer is a page module, so it is checked too.
    let mut dirs = vec![src.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).expect("source directory") {
            let path = entry.expect("source entry").path();
            // Tests drive egui's input themselves, in test modules and in test files.
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if stem == "tests" || stem.ends_with("_tests") {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let file =
                path.strip_prefix(&src).expect("under src").components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            let source = std::fs::read_to_string(&path).expect("source file");
            if file != "canvas/page_pointer.rs" && source.contains("PagePointer") && !PAGE_MODULES.contains(&file.as_str()) {
                problems.push(format!("src/{file} takes the page pointer: add it to PAGE_MODULES."));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
