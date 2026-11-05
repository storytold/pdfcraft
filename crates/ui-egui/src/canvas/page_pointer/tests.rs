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

/// A click is located where it came up even after a stall's many presses and releases, past the
/// cap on a frame's edges. Read from the edges kept, it was an earlier release's.
#[test]
fn a_frames_last_release_is_kept_past_the_cap_on_its_edges() {
    let ctx = Context::default();
    let (p, q) = (pos2(50.0, 60.0), pos2(80.0, 90.0));
    frame(&ctx, vec![Event::PointerMoved(p)], false);
    let mut events: Vec<Event> = (0..2 * MAX_EDGES).map(|i| button(p, PointerButton::Primary, i % 2 == 0)).collect();
    events.extend([button(q, PointerButton::Primary, true), button(q, PointerButton::Primary, false)]);
    let stall = frame(&ctx, events, false);
    assert_eq!(stall.edges.len(), MAX_EDGES);
    assert_eq!(stall.released, Some(q));
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
const PAGE_MODULES: [&str; 37] = [
    "canvas.rs",
    "canvas/page_input.rs",
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
    "tools/stamps_ui.rs",
];

/// What a file that acts on the pages uses: the pointer, a tool's or the comments' page context,
/// what a press is to a page, the router's hits and owners, or the router itself. Such a file is a
/// page module. One that acts on the pages through the page view's response and a bare point alone
/// uses none of them (`canvas/image_pick.rs`): PAGE_MODULES lists it by hand.
const PAGE_MODULE_MARKS: [&str; 6] = ["PagePointer", "PageToolContext", "PageCx", "PagePress", "page_router::", ".router."];

/// The router's own files, which define what page modules use.
const ROUTER_FILES: [&str; 3] = ["canvas/page_pointer.rs", "canvas/page_router.rs", "canvas/page_cursor.rs"];

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
const NOT_PAGE_INPUT: [(&str, &str, &str); 12] = [
    ("canvas/area.rs", "zoom_anchor", "a pinch zooms the whole view, and is read before the pages' widget exists"),
    ("canvas/area.rs", "drawing_edge_pointer", "scrolls the view at its edges, read before the pages' widget exists"),
    ("canvas/area.rs", "glide_interrupted", "a press anywhere stops the touchpad's glide"),
    ("canvas/area.rs", "presentation_click", "full screen's click to turn the page, read before the pages' widget exists"),
    ("canvas/bars.rs", "page_size_hint", "the page size hint sits in the view's corner, not on a page"),
    ("canvas/organize.rs", "organize_grid", "Organize Pages shows thumbnails, not pages"),
    ("tools/forms_ui.rs", "overlay", "the field editor is an Area of its own over the page"),
    ("tools/forms_ui.rs", "choice_list", "a drop-down's open list is an Area of its own over the page"),
    ("tools/forms_ui.rs", "scroll_bar", "the scroll bar of a drop-down's open list, in its Area over the page"),
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

/// Ways to set egui's cursor.
const CURSOR_WRITES: [&str; 4] = ["set_cursor_icon(", ".cursor_icon = ", "on_hover_cursor(", "on_hover_and_drag_cursor("];

/// Functions in the page modules that set a cursor other than the pages': (file, function, why).
const NOT_PAGE_CURSOR: [(&str, &str, &str); 3] = [
    ("tools/fill_sign/scan.rs", "scan_tab", "the signature dialog's Scan tab"),
    ("tools/edit_text_ui.rs", "overlay", "the paragraph editor is an Area of its own over the page, drawn after the pages' cursor is set"),
    ("tools/calc_order_ui.rs", "panel", "the Calculation order panel, whose rows are dragged to reorder them"),
];

/// Whether `line` uses `name`, and not a longer name that ends in it (a field, after its `.`, is
/// never part of a longer name).
fn uses(line: &str, name: &str) -> bool {
    line.match_indices(name).any(|(at, _)| name.starts_with('.') || !line[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
}

/// The lines of `source` outside its test modules, numbered from 1. Tests drive egui's input and
/// read its output themselves. A test module (`#[cfg(test)]` then `mod name {`) ends at the first
/// line that is a lone `}`, as rustfmt lays a top-level item out, so the code after a test module
/// in the middle of a file is still read; `#[cfg(test)]` then `mod name;` is one line.
fn outside_test_modules(source: &str) -> Vec<(usize, &str)> {
    let mut kept = Vec::new();
    let mut lines = source.lines().enumerate().map(|(n, line)| (n + 1, line)).peekable();
    while let Some((n, line)) = lines.next() {
        if line == "#[cfg(test)]" && lines.peek().is_some_and(|(_, next)| next.starts_with("mod ")) {
            if lines.next().is_some_and(|(_, module)| module.ends_with('{')) {
                lines.by_ref().find(|(_, line)| *line == "}");
            }
            continue;
        }
        kept.push((n, line));
    }
    kept
}

/// The lines of code in `source` outside its test modules, numbered from 1, each with the
/// top-level function or `impl` method it is in; comment lines are left out. A line rustfmt
/// continued (a method chain split as `view` / `.router` / `.owner(i)`) is read with the one it
/// continues, under that one's number.
fn code_lines(source: &str) -> Vec<(usize, Option<&str>, String)> {
    let (mut function, mut in_impl) = (None, false);
    let mut lines: Vec<(usize, Option<&str>, String)> = Vec::new();
    for (n, line) in outside_test_modules(source) {
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
        match lines.last_mut() {
            Some((_, _, text)) if line.trim_start().starts_with('.') => text.push_str(line.trim_start()),
            _ => lines.push((n, function, line.to_owned())),
        }
    }
    lines
}

/// Each name `find` finds in a line of the page modules' code (their tests left out), as (file,
/// the top-level function or `impl` method it is in, line, name) ([`code_lines`]).
fn page_module_matches(find: impl Fn(&str) -> Vec<String>) -> Vec<(&'static str, Option<String>, usize, String)> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for file in PAGE_MODULES {
        let source = std::fs::read_to_string(src.join(file)).expect("page module").replace("\r\n", "\n");
        for (n, function, text) in code_lines(&source) {
            for name in find(&text) {
                found.push((file, function.map(str::to_owned), n, name));
            }
        }
    }
    found
}

/// Each hit test `source` imports under a name that is not a hit test's (`use …::hit as under;`),
/// as (name, alias), from each `use` statement, from its first line to its `;`, wherever it is.
fn aliased_hit_tests(source: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut statement: Option<String> = None;
    for line in source.lines().map(str::trim) {
        if statement.is_none() && ["use ", "pub use ", "pub(crate) use ", "pub(super) use "].iter().any(|start| line.starts_with(start)) {
            statement = Some(String::new());
        }
        let Some(text) = statement.as_mut() else { continue };
        text.push_str(line);
        text.push(' ');
        if !line.ends_with(';') {
            continue;
        }
        for part in text.split([',', '{', '}', ';']) {
            let Some((path, alias)) = part.split_once(" as ") else { continue };
            let (name, alias) = (path.trim().rsplit("::").next().unwrap_or_default(), alias.trim());
            if is_hit_test(name) && !is_hit_test(alias) {
                found.push((name.to_owned(), alias.to_owned()));
            }
        }
        statement = None;
    }
    found
}

/// Each hit test a page module imports under a name that is not a hit test's, which the checks by
/// name would not see ([`aliased_hit_tests`]).
fn hit_test_aliases() -> Vec<String> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for file in PAGE_MODULES {
        let source = std::fs::read_to_string(src.join(file)).expect("page module");
        for (name, alias) in aliased_hit_tests(&source) {
            found.push(format!(
                "src/{file}: `{name} as {alias}` imports a hit test under another name, which the checks by name miss: name \
                 it `hit` or `…_hit`."
            ));
        }
    }
    found
}

/// Each use of one of `names` in the page modules' code (their tests left out).
fn page_module_uses(names: &[&str]) -> Vec<(&'static str, Option<String>, usize, String)> {
    page_module_matches(|line| names.iter().filter(|name| uses(line, name)).map(|name| (*name).to_owned()).collect())
}

/// Each use of one of `names` in the page modules' code (their tests left out) outside the
/// functions `allowed` to, as "src/file:line: `name` why"; then each allowed function that uses
/// none of them any more, as "src/file: fn function unused".
fn misuses(names: &[&str], allowed: &[(&str, &str, &str)], why: &str, unused: &str) -> Vec<String> {
    misuses_in(page_module_uses(names), allowed, why, unused)
}

/// [`misuses`], of what was `found`.
fn misuses_in(found: Vec<(&str, Option<String>, usize, String)>, allowed: &[(&str, &str, &str)], why: &str, unused: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut needed = vec![false; allowed.len()];
    for (file, function, n, name) in found {
        match allowed.iter().position(|(f, function_name, _)| *f == file && function.as_deref() == Some(*function_name)) {
            Some(k) => needed[k] = true,
            None => problems.push(format!("src/{file}:{n}: `{name}` {why}")),
        }
    }
    for ((file, name, _), needed) in allowed.iter().zip(needed) {
        if !needed {
            problems.push(format!("src/{file}: fn {name} {unused}"));
        }
    }
    problems
}

/// Page modules read the pointer only through `PagePointer`. egui's raw state misses what is
/// drawn over the pages and where a touch lifted, so a raw read in a page module brings back
/// the bugs `PagePointer` fixed: a cursor or a press through a dialog, a tap that does nothing.
#[test]
fn page_modules_read_the_pointer_only_through_page_pointer() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut problems = misuses(
        &RAW_READS,
        &NOT_PAGE_INPUT,
        "reads egui's pointer state in a page module. Read `PagePointer` (src/canvas/page_pointer.rs) instead: it \
         knows what is drawn over the pages and where a touch lifted. Not page input? Add the function to \
         NOT_PAGE_INPUT there, with why.",
        "reads no pointer state any more: remove it from NOT_PAGE_INPUT.",
    );
    // A module that acts on the pages is a page module, so it is checked too.
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
            let marks = PAGE_MODULE_MARKS.iter().filter(|mark| source.contains(**mark)).collect::<Vec<_>>();
            if !ROUTER_FILES.contains(&file.as_str()) && !marks.is_empty() && !PAGE_MODULES.contains(&file.as_str()) {
                problems.push(format!("src/{file} uses {marks:?}, so it acts on the pages: add it to PAGE_MODULES."));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Page modules claim the cursor and never set it. Where each set it, the last write of a frame
/// won, so the order of the per-page chain decided the cursor and not what a press does: a
/// link's hand over every tool, the I-beam over a comment that a drag moves.
#[test]
fn page_modules_claim_the_cursor_and_never_set_it() {
    let problems = misuses(
        &CURSOR_WRITES,
        &NOT_PAGE_CURSOR,
        "sets the cursor in a page module, where the last write of a frame would win. Claim it instead, on the layer \
         of what is under the pointer (`view.cursor.claim`, src/canvas/page_cursor.rs): `document_area` sets it once, \
         from the claim on the highest layer the active tool reaches. Not the pages' cursor? Add the function to \
         NOT_PAGE_CURSOR in src/canvas/page_pointer/tests.rs, with why.",
        "sets no cursor any more: remove it from NOT_PAGE_CURSOR.",
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// What decides, keeps or drops the press the pages hold.
const ROUTER_WRITES: [&str; 8] = [
    "router.begin(",
    "router.press(",
    "router.settle(",
    "router.cancel(",
    "PageRouter::begin(",
    "PageRouter::press(",
    "PageRouter::settle(",
    "PageRouter::cancel(",
];

/// The functions that decide, keep or drop the press the pages hold: (file, function, why).
const ROUTER_WRITERS: [(&str, &str, &str); 3] = [
    ("canvas/page_input.rs", "page_tool_input", "the hits at the press point decide its owner, once per press"),
    ("canvas.rs", "draw_pages", "the press is over once the button is up and its release seen, and a new one drops the last owner"),
    ("canvas.rs", "cancel_pointer_gestures", "every pointer gesture's cancel: Escape, automation's cancel"),
];

/// Ways to make a `Hit`.
const HIT_BUILDS: [&str; 2] = ["Hit::new(", "Hit {"];

/// Whether `name` is a hit test's: `hit`, or `…_hit`.
fn is_hit_test(name: &str) -> bool {
    name == "hit" || name.ends_with("_hit")
}

/// The hit tests `line` calls: by path, bare or as a method.
fn hit_test_calls(line: &str) -> Vec<String> {
    line.match_indices("hit(")
        .filter_map(|(at, _)| {
            let end = at + "hit".len();
            let start = line[..end].char_indices().rev().find(|(_, c)| !(c.is_alphanumeric() || *c == '_')).map_or(0, |(i, c)| i + c.len_utf8());
            let (before, name) = (&line[..start], &line[start..end]);
            (is_hit_test(name) && !before.ends_with("fn ")).then(|| name.to_owned())
        })
        .collect()
}

/// The functions that call a hit test: (file, function, why). Hit tests are found by name (a
/// function named `hit` or `…_hit`, called by path, bare or as a method). The comments' point test
/// has a list of its own (`COMMENT_POINT_TESTERS`); the others by other names (added content's,
/// Edit text's paragraphs', Redact's text test, the text layer's links) are not listed yet, and
/// most are PR 4b's. `PageHits::hits` is the one: the hits
/// at the press point decide its owner, and those under the pointer give its cursor, its tip, hover
/// painting and a right-click.
const HIT_TESTERS: [(&str, &str, &str); 1] = [(
    "canvas/page_input.rs",
    "hits",
    "`PageHits::hits`: every module's hits at a point, for the pointer's cursor and tip, a right-click and, once, the press's owner",
)];

/// The page view's asks for every module's hits at a point (`PageHits::hits`, and `at` and
/// `context_at`, which wrap it).
const PAGE_HITS_ASKS: [&str; 6] = [".hits(", ".at(", ".context_at(", "PageHits::hits(", "PageHits::at(", "PageHits::context_at("];

/// The functions that ask the page view's hits at a point: (file, function, why). `PageHits` is
/// private to `page_input.rs`, so this keeps a new function there from asking; a second ask inside
/// a listed function is for review to catch. The method names are matched in every page module,
/// so an `at` of another type there would need them narrowed.
const PAGE_HITS_ASKERS: [(&str, &str, &str); 5] = [
    ("canvas/page_input.rs", "at", "`PageHits::at`: the hits at a point, as a press there would take them"),
    ("canvas/page_input.rs", "context_at", "`PageHits::context_at`: what a right-click at a point is on"),
    (
        "canvas/page_input.rs",
        "page_tool_input",
        "the hits at the press point decide its owner, once per press; those under the pointer before the tools act, once a \
         frame, serve a click's release, a right-click and Fill & Sign's preview",
    ),
    ("canvas/page_input.rs", "claim_under", "what lies under the pointer after the tools act: its cursor, its tip and hover painting"),
    ("canvas/page_input.rs", "right_click", "what a right-click is on"),
];

/// The comments' point test, which their hit tests are built on: which comment lies under a point.
const COMMENT_POINT_TESTS: [&str; 2] = [".comment_at(", "PageCx::comment_at("];

/// The functions that ask which comment lies under a point: (file, function, why).
const COMMENT_POINT_TESTERS: [(&str, &str, &str); 3] = [
    ("tools/comments/input.rs", "hit", "the comments' hit test"),
    ("tools/comments/input.rs", "context_hit", "the comments' hit test for a right-click"),
    ("tools/comments/paint.rs", "paint_page", "a redaction mark shows its fill under the pointer with every tool, as comments' tips show"),
];

/// What reads who holds the press the pages hold.
const ROUTER_READS: [&str; 6] =
    ["router.press_for(", "router.owner(", "router.keeps_drag(", "PageRouter::press_for(", "PageRouter::owner(", "PageRouter::keeps_drag("];

/// The functions that read who holds the press: (file, function, why). The page view asks once per
/// page and tells each module what the press is to its page (`PagePress`); the Edit text group, the
/// text layer and link following still ask, until they are told too (PR 4b), and an unused entry
/// fails the test.
const ROUTER_READERS: [(&str, &str, &str); 8] = [
    (
        "canvas/page_input.rs",
        "page_tool_input",
        "the page view: what the press is to each page (`press_for`); Add text's paragraph and the pictures until 4b",
    ),
    ("canvas/page_input.rs", "update_selection_on_press", "the text layer selects no text under a press held by what keeps its drag (4b)"),
    ("canvas/page_input.rs", "finish_text_pointer_gesture", "a click held by what keeps its drag leaves the text selection alone (4b)"),
    ("canvas/page_input.rs", "page_overlays", "a click follows the link that holds its press (4b)"),
    ("tools/content_ui.rs", "page_input", "added text and pictures, until they are told (4b)"),
    ("tools/edit_text_ui.rs", "page_input", "Edit text's paragraphs, until they are told (4b)"),
    ("tools/edit_text_ui/image.rs", "image_input", "Edit text's pictures, until they are told (4b)"),
    ("tools/object_ui.rs", "input", "the objects selected together, until they are told (4b)"),
];

/// What ignores the press the pages hold until the button is up.
const PRESS_IGNORES: [&str; 1] = ["press_cancelled = true"];

/// The one function that ignores a press until the button is up: (file, function, why).
const PRESS_IGNORERS: [(&str, &str, &str); 1] =
    [("canvas/page_input.rs", "cancel_on_escape", "Escape abandons the gesture the press drives, whichever tool's")];

/// The module hit tests, called by path: a press handler calls none of them (HIT_TESTERS says who
/// may).
const PAGE_HIT_CALLS: [&str; 12] = [
    "link_ui::hit(",
    "prepare::hit(",
    "forms_ui::hit(",
    "comments::hit(",
    "comments::context_hit(",
    "camera_ui::hit(",
    "edit_text_ui::hit(",
    "edit_text_ui::picture_hit(",
    "edit_text_ui::picture_handle_hit(",
    "edit_text_ui::crop_hit(",
    "object_ui::hit(",
    "content_ui::hit(",
];

/// Whether a press handler's `code` acts on the owner the router captured, and calls no hit test
/// of its own to choose one.
fn routed_press_handler(code: &str) -> bool {
    let code = code.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    code.contains("router.owner(")
        && !PAGE_HIT_CALLS.iter().any(|call| code.contains(call))
        && !uses(&code, "hit(")
        && !uses(&code, "body_hit(")
        && !uses(&code, "handle_hit(")
}

/// Page modules act on a press only when the router says it is theirs. The hits at the press
/// point decide its owner once, in one place, and a `Hit` comes only from a module's hit test,
/// so what the cursor promised is what the press does. Where each module tested the press
/// point again, or asked another whether it wanted the press, the answers disagreed: a press on
/// one check box released over the next ticked the second, and a drag could be taken over
/// halfway. Only `PageHits::hits` asks the hit tests. Escape is the one way to
/// drop a press and ignore it until the button is up.
#[test]
fn page_modules_act_on_a_press_only_through_the_router() {
    let mut problems = misuses(
        &ROUTER_WRITES,
        &ROUTER_WRITERS,
        "decides, keeps or drops the press the pages hold. Only the hits at the press point decide its owner, once \
         (src/canvas/page_router.rs), and the page view tells each module what the press is to its page (`PagePress`). Drop a press with \
         `DocView::cancel_pointer_gestures`. A new place that must? Add the function to ROUTER_WRITERS in \
         src/canvas/page_pointer/tests.rs, with why.",
        "no longer decides, keeps or drops the press: remove it from ROUTER_WRITERS.",
    );
    problems.extend(misuses(
        &ROUTER_READS,
        &ROUTER_READERS,
        "reads who holds the press the pages hold. The page view asks the router once per page and tells each module \
         what the press is to its page: take a `PagePress` (src/canvas/page_router.rs) instead. A new place that must? \
         Add the function to ROUTER_READERS in src/canvas/page_pointer/tests.rs, with why.",
        "no longer reads who holds the press: remove it from ROUTER_READERS.",
    ));
    // A router method the lists above don't name yet is the router's all the same: only their
    // functions use it.
    let known: Vec<&str> = ROUTER_WRITES.iter().chain(ROUTER_READS.iter()).copied().collect();
    let unlisted = page_module_matches(|line| {
        let router = uses(line, ".router.") || uses(line, "PageRouter::");
        if router && !known.iter().any(|name| uses(line, name)) { vec!["router".to_owned()] } else { Vec::new() }
    });
    problems.extend(
        unlisted
            .into_iter()
            .filter(|(file, function, _, _)| {
                !ROUTER_WRITERS.iter().chain(ROUTER_READERS.iter()).any(|(f, name, _)| f == file && function.as_deref() == Some(*name))
            })
            .map(|(file, _, n, _)| {
                format!(
                    "src/{file}:{n}: uses the router by a method ROUTER_WRITES and ROUTER_READS don't name. The page view \
                     decides the press and tells each module what it is to its page (`PagePress`): add the method to \
                     ROUTER_WRITES or ROUTER_READS in src/canvas/page_pointer/tests.rs, and the function to their lists \
                     only if it must."
                )
            }),
    );
    problems.extend(misuses(
        &COMMENT_POINT_TESTS,
        &COMMENT_POINT_TESTERS,
        "asks which comment lies under a point. The comments' hit tests answer that for a press and a right-click: \
         act on what the page view tells you, rather than testing the point again. A new place that must? Add the \
         function to COMMENT_POINT_TESTERS in src/canvas/page_pointer/tests.rs, with why.",
        "no longer asks which comment lies under a point: remove it from COMMENT_POINT_TESTERS.",
    ));
    problems.extend(misuses(
        &PAGE_HITS_ASKS,
        &PAGE_HITS_ASKERS,
        "asks the page view's hits at a point. The hits at the press point decide its owner once, and those under the \
         pointer are taken once before the tools act and once after: use those (`under`, `PageInput.hover`). A new \
         place that must ask? Add the function to PAGE_HITS_ASKERS in src/canvas/page_pointer/tests.rs, with why.",
        "no longer asks the page view's hits: remove it from PAGE_HITS_ASKERS.",
    ));
    problems.extend(misuses(
        &PRESS_IGNORES,
        &PRESS_IGNORERS,
        "ignores the press the pages hold. Escape cancels a press, in `cancel_on_escape`: drop a gesture with \
         `DocView::cancel_pointer_gestures` instead.",
        "no longer ignores a press: remove it from PRESS_IGNORERS.",
    ));
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for (file, function) in [
        ("tools/content_ui.rs", "page_input"),
        ("tools/edit_text_ui.rs", "page_input"),
        ("tools/edit_text_ui/image.rs", "image_input"),
        ("tools/object_ui.rs", "input"),
    ] {
        let source = std::fs::read_to_string(src.join(file)).expect("page input module");
        let body = source.lines().skip_while(|line| declares(line) != Some(function)).take_while(|line| *line != "}").collect::<Vec<_>>().join("\n");
        if !routed_press_handler(&body) {
            problems.push(format!("src/{file}: {function} must consume the captured router owner without repeating a hit test"));
        }
    }
    // A hit test is a function named `hit` or `…_hit`: one answer to "what of mine is here?",
    // which gives the module's cursor, its tip and its press.
    problems.extend(misuses_in(
        page_module_matches(hit_test_calls),
        &HIT_TESTERS,
        "calls a hit test. The hits at the press point decide its owner once, in `PageHits::hits`: act on the press when \
         the page view tells you it is yours (`PagePress::Mine`), rather than testing its point again. What lies under \
         the pointer comes from the page view's own hits: hover painting and tips (`PageInput.hover`), a click released \
         over what it was pressed on (`released_over`). A new place that must ask? Add the function to HIT_TESTERS in \
         src/canvas/page_pointer/tests.rs, with why.",
        "calls no hit test any more: remove it from HIT_TESTERS.",
    ));
    problems.extend(hit_test_aliases());
    // An entry no page module calls any more no longer guards anything.
    let called = page_module_uses(&PAGE_HIT_CALLS);
    problems.extend(
        PAGE_HIT_CALLS
            .iter()
            .filter(|call| !called.iter().any(|(_, _, _, name)| name == *call))
            .map(|call| format!("no page module calls `{call}` any more: remove it from PAGE_HIT_CALLS.")),
    );
    let builds = page_module_uses(&HIT_BUILDS);
    assert!(!builds.is_empty(), "no page module makes a `Hit` any more: update HIT_BUILDS");
    problems.extend(builds.into_iter().filter(|(_, function, _, _)| !function.as_deref().is_some_and(is_hit_test)).map(|(file, _, n, name)| {
        format!(
            "src/{file}:{n}: `{name}` makes a hit outside a hit test. A page module answers \"what of mine is \
                     here?\" in one function, named `hit` or `…_hit`, so its cursor, its tip and its press agree."
        )
    }));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// A test module in the middle of a file hides only itself from the page guards: the code after
/// it is read too. The guards used to stop at the first test module.
#[test]
fn the_page_guards_read_past_a_test_module_in_the_middle_of_a_file() {
    let source = "fn before() {}\n#[cfg(test)]\nmod tests {\n    fn inside() {}\n}\n\nfn after() {}\n#[cfg(test)]\nmod more;\nfn last() {}\n";
    let kept: Vec<(usize, &str)> = outside_test_modules(source);
    assert_eq!(kept, [(1, "fn before() {}"), (6, ""), (7, "fn after() {}"), (10, "fn last() {}")]);
}

/// The guard's readers: hit tests by name, the function a line declares, a call rustfmt split,
/// and a hit test imported as another name.
#[test]
fn the_page_guards_read_hit_tests_split_calls_and_aliases() {
    assert_eq!(hit_test_calls("let h = crate::forms_ui::hit(xf, p);"), ["hit"]);
    assert_eq!(hit_test_calls("cx.comment_hit(p) && picture_hit(p)"), ["comment_hit", "picture_hit"]);
    assert!(hit_test_calls("pub(crate) fn hit(xf: &PageXform) -> Option<Hit> {").is_empty());
    assert!(hit_test_calls("let all = self.hits(view, p, false);").is_empty());
    assert_eq!(declares("pub(crate) const fn crop_hit<'a>(view: &DocView) {"), Some("crop_hit"));
    assert_eq!(declares("let hit = 1;"), None);
    let source = "impl PageHits<'_> {\n    fn at(&self) {\n        self\n            .hits(view, p, false)\n    }\n}\nfn after() {\n    // view.router.owner(0)\n    let _ = view\n        .router\n        .owner(0);\n}\n";
    assert_eq!(
        code_lines(source),
        [
            (1, None, "impl PageHits<'_> {".to_owned()),
            (2, Some("at"), "    fn at(&self) {".to_owned()),
            (3, Some("at"), "        self.hits(view, p, false)".to_owned()),
            (5, Some("at"), "    }".to_owned()),
            (6, None, "}".to_owned()),
            (7, Some("after"), "fn after() {".to_owned()),
            (9, Some("after"), "    let _ = view.router.owner(0);".to_owned()),
            (12, None, "}".to_owned()),
        ]
    );
    let source =
        "fn a() {}\n\n#[cfg(any())]\nuse crate::camera_ui::hit as corner_under;\nuse crate::x::{\n    hit as x_hit,\n    picture_hit as under,\n};\n";
    let aliases: Vec<(String, String)> = [("hit", "corner_under"), ("picture_hit", "under")].map(|(a, b)| (a.to_owned(), b.to_owned())).to_vec();
    assert_eq!(aliased_hit_tests(source), aliases);
}

#[test]
fn the_page_press_guard_rejects_missing_owners_and_second_hit_tests() {
    assert!(routed_press_handler("let owner = view.router.owner(page);"));
    assert!(!routed_press_handler("// view.router.owner(page)\nlet owner = None;"));
    assert!(!routed_press_handler("let owner = view.router.owner(page);\nlet again = cx.hit(origin);"));
    for call in PAGE_HIT_CALLS {
        assert!(!routed_press_handler(&format!("let owner = view.router.owner(page);\n{call}origin);")), "{call}");
    }
}
