use super::*;

/// The editor focused, in a space 400 points wide, for one frame of `events`: how tall it is, and
/// what the frame drew.
fn measured(ctx: &egui::Context, text: &mut String, time: f64, events: Vec<egui::Event>) -> (f32, Vec<String>) {
    let mut h = 0.0;
    let input = egui::RawInput { time: Some(time), events, focused: true, ..Default::default() };
    let output = ctx.run_ui(input, |ui| {
        ui.set_max_width(400.0);
        let id = ui.make_persistent_id("probe");
        ui.memory_mut(|m| m.request_focus(id));
        let top = ui.cursor().top();
        script_editor(ui, text, "probe", 4, 400.0, None);
        h = ui.cursor().top() - top;
    });
    let said: Vec<String> =
        output.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.text().to_owned()) } else { None }).collect();
    output.drop_without_applying_deltas();
    (h, said)
}

#[test]
fn the_editor_keeps_its_height_as_the_box_fills_and_empties() {
    // A box that grew as it filled would move the buttons below it out from under the pointer.
    let ctx = egui::Context::default();
    let mut text = String::new();
    let (empty, _) = measured(&ctx, &mut text, 1.0, Vec::new());
    measured(&ctx, &mut text, 1.1, Vec::new());
    let (typed, _) = measured(&ctx, &mut text, 1.2, vec![egui::Event::Text("v\nw\nx\ny\nz\nv\nw\nx\ny\nz".into())]);
    assert!(text.lines().count() > 8, "more lines than the box shows: {text:?}");
    text.clear();
    let (cleared, _) = measured(&ctx, &mut text, 3.0, Vec::new());
    assert!((empty - typed).abs() < 0.5 && (empty - cleared).abs() < 0.5, "{empty} {typed} {cleared}");
}

#[test]
fn undo_never_brings_back_the_script_shown_before() {
    // One box shows one field's script, then another's (the same box id): Cmd+Z after typing
    // in the second goes back to the second script as it came, not to the first.
    let ctx = egui::Context::default();
    let mut id = egui::Id::NULL;
    let mut show = |text: &mut String, time: f64, events: Vec<egui::Event>| {
        let input = egui::RawInput { time: Some(time), events, focused: true, ..Default::default() };
        let output = ctx.run_ui(input, |ui| {
            id = ui.make_persistent_id("probe");
            ui.memory_mut(|m| m.request_focus(id));
            script_editor(ui, text, "probe", 4, 300.0, None);
        });
        output.drop_without_applying_deltas();
    };
    let typed = |s: &str| vec![egui::Event::Text(s.into())];
    let mut first = String::from("first");
    show(&mut first, 1.0, Vec::new());
    show(&mut first, 1.1, Vec::new());
    show(&mut first, 2.0, typed("x"));
    show(&mut first, 5.0, Vec::new());
    assert_eq!(first, "firstx");

    let mut second = String::from("second");
    show(&mut second, 6.0, Vec::new());
    show(&mut second, 6.1, typed("y"));
    assert_eq!(second, "secondy");
    let undo = egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
    show(&mut second, 6.2, vec![undo]);
    assert_eq!(second, "second");
}

/// The corner grip makes the box taller. It must not widen it too, which would also widen the dialog
/// around it, for good.
#[test]
fn dragging_the_box_s_corner_only_makes_it_taller() {
    let ctx = egui::Context::default();
    let mut text = String::from("var a;");
    let mut time = 1.0;
    let mut frame = |events: Vec<egui::Event>| {
        time += 0.05;
        let mut rect = egui::Rect::NOTHING;
        let output = ctx.run_ui(egui::RawInput { time: Some(time), events, ..Default::default() }, |ui| {
            rect = script_editor(ui, &mut text, "probe", 4, 300.0, None).rect;
        });
        output.drop_without_applying_deltas();
        rect
    };
    frame(Vec::new());
    let before = frame(Vec::new());
    let grip = before.right_bottom() - egui::vec2(4.0, 4.0);
    let to = grip + egui::vec2(60.0, 40.0);
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    frame(vec![egui::Event::PointerMoved(grip)]);
    frame(vec![button(grip, true)]);
    frame(vec![egui::Event::PointerMoved(to)]);
    frame(vec![egui::Event::PointerMoved(to)]);
    frame(vec![button(to, false)]);
    let after = frame(Vec::new());
    assert!(after.height() > before.height() + 20.0, "taller: {before:?} then {after:?}");
    assert_eq!(after.width(), before.width(), "and no wider");
}

fn palette() -> Palette {
    Palette::for_kind(ThemeKind::Light)
}

#[test]
fn the_layout_keeps_every_character_and_colours_the_right_ones() {
    let source = "var s = 'é'; // ok\nif (x {";
    let tokens = pdfcraft_engine::js::highlight(source);
    let job = layout_job(source, &tokens, &palette(), &egui::FontId::monospace(12.0));
    assert_eq!(job.text, source, "nothing lost or doubled");
    let colour_of = |piece: &str| {
        let at = source.find(piece).expect("piece");
        job.sections.iter().find(|s| s.byte_range.start.0 <= at && at < s.byte_range.end.0).map(|s| (s.format.color, s.format.italics))
    };
    let p = palette();
    assert_eq!(colour_of("var"), Some((p.keyword, false)));
    assert_eq!(colour_of("'é'"), Some((p.string, false)));
    assert_eq!(colour_of("// ok"), Some((p.comment, true)));
    assert_eq!(colour_of(" = "), Some((p.text, false)));
    assert_eq!(colour_of("x"), Some((p.text, false)));
}

#[test]
fn every_colour_reads_on_its_background() {
    // WCAG relative luminance contrast.
    fn luminance(c: Color32) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }
    for kind in [ThemeKind::Light, ThemeKind::Dark] {
        let tokens = Tokens::for_kind(kind);
        let (field, card) = (tokens.field, tokens.card);
        let p = Palette::for_kind(kind);
        let on_field =
            [("keyword", p.keyword), ("literal", p.literal), ("string", p.string), ("regex", p.regex), ("comment", p.comment), ("api", p.api)];
        // What the box says when something is wrong is written on the dialog card.
        let checks = on_field.map(|(name, c)| (name, c, field)).into_iter().chain([("error message", p.error, card)]);
        for (name, c, background) in checks {
            let (a, b) = (luminance(c), luminance(background));
            let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(ratio >= 4.5, "{kind:?} {name}: {ratio:.2}:1");
        }
    }
}

/// What one frame of the editor draws for `text`: every text shape.
fn drawn(ctx: &egui::Context, text: &mut String, time: f64) -> Vec<String> {
    let output = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
        script_editor(ui, text, "probe", 4, 300.0, None);
    });
    let said =
        output.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.text().to_owned()) } else { None }).collect();
    output.drop_without_applying_deltas();
    said
}

#[test]
fn a_script_past_the_engine_s_limit_gets_a_note_and_is_not_laid_out() {
    let ctx = egui::Context::default();
    let mut text = "x".repeat(MAX_EDITED_BYTES + 1);
    let shown = text.clone();
    for time in [1.0, 2.0, 3.0] {
        let said = drawn(&ctx, &mut text, time);
        assert!(said.iter().any(|s| s.contains("too long")), "{:?}", said.iter().map(|s| s.len()).collect::<Vec<_>>());
        assert!(said.iter().all(|s| s.len() < 1000), "its text is not laid out: {:?}", said.iter().map(|s| s.len()).collect::<Vec<_>>());
        assert_eq!(text, shown, "and it is left as it is");
    }
    // Replaced by one the box can show, the box is back.
    text = String::from("var (");
    let said = drawn(&ctx, &mut text, 4.0);
    assert!(said.iter().any(|s| s == "var ("), "{said:?}");
    assert!(!said.iter().any(|s| s.contains("too long")), "{said:?}");
}

#[test]
fn a_script_at_the_engine_s_limit_is_still_edited() {
    let ctx = egui::Context::default();
    let mut text = "x".repeat(MAX_EDITED_BYTES);
    let said = drawn(&ctx, &mut text, 1.0);
    assert!(said.iter().any(|s| s.len() == MAX_EDITED_BYTES), "{:?}", said.iter().map(|s| s.len()).collect::<Vec<_>>());
    assert!(!said.iter().any(|s| s.contains("too long")));
}

/// The editor with its box focused, for one frame of `events`; what the frame drew.
fn focused(ctx: &egui::Context, text: &mut String, time: f64, events: Vec<egui::Event>) -> Vec<String> {
    let input = egui::RawInput { time: Some(time), events, focused: true, ..Default::default() };
    let output = ctx.run_ui(input, |ui| {
        let id = ui.make_persistent_id("probe");
        ui.memory_mut(|m| m.request_focus(id));
        script_editor(ui, text, "probe", 4, 300.0, None);
    });
    let said =
        output.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.text().to_owned()) } else { None }).collect();
    output.drop_without_applying_deltas();
    said
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// The accessibility request that replaces the whole value of box `id`.
fn set_value(id: egui::Id, value: &str) -> egui::Event {
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::SetValue,
        target_tree: egui::accesskit::TreeId::ROOT,
        target_node: id.accesskit_id(),
        data: Some(egui::accesskit::ActionData::Value(value.into())),
    })
}

/// Whether the box last refused an edit. A box this long pushes the line that says so off the screen,
/// where it isn't drawn, so the other tests read the line and these read this.
const REFUSED: &str = "Not added";

fn refused(ctx: &egui::Context) -> bool {
    let mut id = egui::Id::NULL;
    ctx.run_ui(egui::RawInput::default(), |ui| id = ui.make_persistent_id("probe")).drop_without_applying_deltas();
    ctx.data(|d| d.get_temp::<Seen>(id.with("script-seen"))).expect("the editor keeps what it saw").refused
}

#[test]
fn every_way_of_adding_text_past_the_limit_is_refused_and_the_line_says_why() {
    let ctx = egui::Context::default();
    let mut box_id = egui::Id::NULL;
    ctx.run_ui(egui::RawInput::default(), |ui| box_id = ui.make_persistent_id("probe")).drop_without_applying_deltas();
    // One byte of room, counted in bytes: "é" is one character and two bytes.
    let full = "x".repeat(MAX_EDITED_BYTES - 1);
    let mut text = full.clone();
    let mut time = 1.0;
    focused(&ctx, &mut text, time, Vec::new());
    focused(&ctx, &mut text, time + 0.1, Vec::new());
    let is_refused = |what: &str, text: &str, said: &[String]| {
        assert!(text == full, "{what}: the script stays as it was: {} bytes", text.len());
        assert!(refused(&ctx), "{what}: the box says so");
        assert!(!said.iter().any(|s| s.contains("too long")), "{what}: the box stays");
    };
    let cases = vec![
        ("a paste", egui::Event::Paste("é".into())),
        ("a character", egui::Event::Text("é".into())),
        ("an input method", egui::Event::Ime(egui::ImeEvent::Commit("é".into()))),
        ("a huge paste", egui::Event::Paste("y".repeat(300 * 1024))),
        ("a middle click", egui::Event::MiddleClickPaste { pos: egui::pos2(40.0, 30.0), text: "é".into() }),
    ];
    for (what, event) in cases {
        time += 1.0;
        // The middle click goes where the pointer is, with or without the box's focus.
        let said = focused(&ctx, &mut text, time, vec![egui::Event::PointerMoved(egui::pos2(40.0, 30.0)), event]);
        is_refused(what, &text, &said);
        // Each case by itself: another script in the box ends the refusal.
        focused(&ctx, &mut String::new(), time + 0.1, Vec::new());
        assert!(!refused(&ctx), "{what}");
        focused(&ctx, &mut text, time + 0.2, Vec::new());
    }
    // Whatever the box replaces it with, assistive technology included.
    time += 1.0;
    let said = focused(&ctx, &mut text, time, vec![set_value(box_id, &"y".repeat(MAX_EDITED_BYTES + 1))]);
    is_refused("assistive technology", &text, &said);

    // What fits goes in, up to the last byte, and the line goes back to the check.
    time += 1.0;
    focused(&ctx, &mut text, time, vec![egui::Event::Paste("y".into())]);
    assert_eq!(text.len(), MAX_EDITED_BYTES, "a paste that fits exactly");
    assert!(!refused(&ctx), "and the box no longer says so");

    // At the limit even the keys that add one byte don't.
    let at_limit = text.clone();
    for (what, event) in [("Enter", key(egui::Key::Enter, egui::Modifiers::NONE)), ("Tab", key(egui::Key::Tab, egui::Modifiers::NONE))] {
        time += 1.0;
        focused(&ctx, &mut text, time, vec![event]);
        assert!(text == at_limit, "{what}: {} bytes", text.len());
        assert!(refused(&ctx), "{what}");
    }
}

#[test]
fn a_refused_edit_keeps_the_selection_s_text_and_the_undo_history() {
    let ctx = egui::Context::default();
    let mut id = egui::Id::NULL;
    ctx.run_ui(egui::RawInput::default(), |ui| id = ui.make_persistent_id("probe")).drop_without_applying_deltas();
    let select = |range: std::ops::Range<usize>| {
        let mut state = egui::text_edit::TextEditState::load(&ctx, id).expect("the box has been shown");
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end))));
        state.store(&ctx, id);
    };
    let script = format!("{}abc", "x".repeat(MAX_EDITED_BYTES - 3));
    let mut text = script.clone();
    focused(&ctx, &mut text, 1.0, Vec::new());
    focused(&ctx, &mut text, 1.1, Vec::new());

    // Five bytes over three selected: the box deletes the selection first, and it comes back.
    select(MAX_EDITED_BYTES - 3..MAX_EDITED_BYTES);
    focused(&ctx, &mut text, 2.0, vec![egui::Event::Paste("12345".into())]);
    assert!(text == script, "what was selected is still there: {} bytes", text.len());
    assert!(refused(&ctx));
    let [from, to] = egui::text_edit::TextEditState::load(&ctx, id).and_then(|s| s.cursor.char_range()).expect("a caret").sorted_cursors();
    assert_eq!((from.index.0, to.index.0), (MAX_EDITED_BYTES, MAX_EDITED_BYTES), "the caret is after it");

    // Over a selection that makes room, the same paste fits.
    select(MAX_EDITED_BYTES - 3..MAX_EDITED_BYTES);
    focused(&ctx, &mut text, 3.0, vec![egui::Event::Paste("123".into())]);
    assert_eq!(text.len(), MAX_EDITED_BYTES);
    assert!(text.ends_with("123"), "replaced");
    assert!(!refused(&ctx));

    // And the edit before the refused one can still be undone: there's still a box, and history.
    focused(&ctx, &mut text, 6.0, Vec::new());
    focused(&ctx, &mut text, 6.1, vec![egui::Event::Paste("past the end".into())]);
    focused(&ctx, &mut text, 6.2, vec![key(egui::Key::Z, egui::Modifiers::COMMAND)]);
    assert!(text == script, "Undo goes back past the refusal: {} bytes", text.len());
}

#[test]
fn the_refusal_is_said_until_the_script_changes() {
    let ctx = egui::Context::default();
    let mut text = String::from("var a;");
    focused(&ctx, &mut text, 1.0, Vec::new());
    let said = focused(&ctx, &mut text, 1.1, vec![egui::Event::Paste("y".repeat(MAX_EDITED_BYTES))]);
    assert_eq!(text, "var a;", "a paste bigger than the room left");
    assert!(said.iter().any(|s| s.contains(REFUSED)), "{said:?}");
    assert!(focused(&ctx, &mut text, 1.2, Vec::new()).iter().any(|s| s.contains(REFUSED)), "the next frame too");
    let said = focused(&ctx, &mut text, 1.3, vec![egui::Event::Text("b".into())]);
    assert_eq!(text, "var a;b");
    assert!(!said.iter().any(|s| s.contains(REFUSED)), "{said:?}");
    // An empty box says it too.
    let mut text = String::new();
    let ctx = egui::Context::default();
    focused(&ctx, &mut text, 1.0, Vec::new());
    let said = focused(&ctx, &mut text, 1.1, vec![egui::Event::Paste("y".repeat(MAX_EDITED_BYTES + 1))]);
    assert_eq!(text, "");
    assert!(said.iter().any(|s| s.contains(REFUSED)), "{said:?}");
}

/// An editor driven by key events, as a person types into it.
struct Typing {
    ctx: egui::Context,
    text: String,
    time: f64,
    id: egui::Id,
    said: Vec<String>,
    /// Each text painted: what it says, where, and the rectangle it is clipped to.
    shapes: Vec<(String, egui::Rect, egui::Rect)>,
    /// Room above the box, as a dialog has (the top of the screen would hide what is painted above it).
    above: f32,
}

impl Typing {
    fn new(text: &str) -> Self {
        let mut t = Typing {
            ctx: egui::Context::default(),
            text: text.into(),
            time: 1.0,
            id: egui::Id::NULL,
            said: Vec::new(),
            shapes: Vec::new(),
            above: 0.0,
        };
        t.frame(Vec::new());
        t.ctx.memory_mut(|m| m.request_focus(t.id));
        t.frame(Vec::new());
        t
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 0.05;
        let (mut id, text, above) = (egui::Id::NULL, &mut self.text, self.above);
        let output = self.ctx.run_ui(egui::RawInput { time: Some(self.time), events, ..Default::default() }, |ui| {
            ui.add_space(above);
            id = ui.make_persistent_id("probe");
            script_editor(ui, text, "probe", 6, 400.0, None);
        });
        self.id = id;
        self.shapes = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_owned(), egui::Rect::from_min_size(t.pos, t.galley.size()), c.clip_rect)),
                _ => None,
            })
            .collect();
        self.said = self.shapes.iter().map(|(s, _, _)| s.clone()).collect();
        output.drop_without_applying_deltas();
    }

    /// Put the selection on characters `range`, as clicking or dragging would.
    fn select(&mut self, range: Range<usize>) {
        set_selection_facing(&self.ctx, self.id, range, false);
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        self.frame(vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
    }

    fn typed(&mut self, s: &str) {
        self.frame(vec![egui::Event::Text(s.into())]);
    }

    fn caret(&self) -> Option<Range<usize>> {
        selection(&self.ctx, self.id)
    }

    /// Long enough for egui's undo to take what the box holds as a point to go back to.
    fn idle(&mut self) {
        for _ in 0..2 {
            self.time += 1.1;
            self.frame(Vec::new());
        }
    }

    /// Frames with no input, for what waits for the next frame or is animated.
    fn settle(&mut self) {
        for _ in 0..12 {
            self.frame(Vec::new());
        }
    }
}

fn press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// A key pressed and let go, so that the next press of it is a press and no repeat.
fn tap(key: egui::Key) -> [egui::Event; 2] {
    let up = egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE };
    [press(key, egui::Modifiers::NONE), up]
}

#[test]
fn enter_keeps_the_indentation_tab_indents_lines_and_a_brace_outdents() {
    let mut t = Typing::new("if (a) {");
    t.select(8..8);
    t.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(t.text, "if (a) {\n  ");
    assert_eq!(t.caret(), Some(11..11));
    t.typed("x;");
    t.key(egui::Key::Enter, egui::Modifiers::NONE);
    t.typed("}");
    assert_eq!(t.text, "if (a) {\n  x;\n}", "the brace goes back to the block's level");
    // Tab on two selected lines indents both; Shift+Tab takes it back.
    t.select(0..13);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "  if (a) {\n    x;\n}");
    t.select(0..17);
    t.key(egui::Key::Tab, egui::Modifiers::SHIFT);
    assert_eq!(t.text, "if (a) {\n  x;\n}");
    // Tab at a caret types the script's indentation, not a tab character.
    t.select(0..0);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "  if (a) {\n  x;\n}");
}

/// Line numbers show only beside the lines in view: a scrolled box painted the numbers of the
/// lines scrolled out over whatever was above it.
#[test]
fn line_numbers_stay_inside_a_scrolled_box() {
    let text: String = (1..=40).map(|i| format!("var a{i} = {i};\n")).collect();
    let end = text.chars().count();
    let mut t = Typing::new(&text);
    t.above = 300.0;
    t.select(end..end);
    t.typed("x");
    // The box scrolls to the caret over a few frames.
    for _ in 0..20 {
        t.frame(Vec::new());
    }
    // The script shows between these heights (its clip, inside the box's margins).
    let shown = t.shapes.iter().find(|(s, _, _)| s.starts_with("var a1 = 1;")).map(|(_, _, clip)| clip.y_range()).expect("the script is painted");
    let numbers: Vec<_> = t.shapes.iter().filter(|(s, _, _)| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).collect();
    assert!(numbers.iter().any(|(s, rect, clip)| s == "41" && clip.intersects(*rect)), "the caret's line is numbered: {numbers:?}");
    for (s, rect, clip) in numbers {
        if clip.intersects(*rect) {
            let visible = clip.intersect(*rect);
            // The box's 6-point margins: a number may show in them, beside a line cut by the edge.
            assert!(
                visible.top() >= shown.min - 6.5 && visible.bottom() <= shown.max + 6.5,
                "line {s} shows outside the box ({shown:?}): {rect:?} clipped to {clip:?}"
            );
        }
    }
}

/// In a dialog, the box is never taller than the dialog can be and still fit in the window: a box
/// dragged taller pushed the dialog's buttons off the screen, and kept that height when reopened.
#[test]
fn the_box_keeps_its_dialog_inside_the_window() {
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0));
    let mut text = String::new();
    for frame in 0..6 {
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(1.0 + f64::from(frame) * 0.05), ..Default::default() };
        let output = ctx.run_ui(input, |ui| {
            crate::widgets::show_modal(ui.ctx(), "probe-dialog", |ui| {
                ui.label("Title");
                // 80 rows: far more than the window has room for.
                script_editor(ui, &mut text, "probe", 80, 300.0, None);
                ui.label("Buttons");
            });
        });
        output.drop_without_applying_deltas();
    }
    let card: egui::Rect = ctx.data(|d| d.get_temp(egui::Id::unique("probe-dialog").with("card"))).expect("the dialog was shown");
    assert!(card.top() >= 0.0 && card.bottom() <= screen.bottom(), "the dialog leaves the window: {card:?}");
}

#[test]
fn tab_on_whole_lines_indents_them_and_never_types_over_them() {
    // A selection from a line's start to the next line's start (Shift+Down), and one whole line.
    let mut t = Typing::new("a\nb");
    t.select(0..2);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "  a\nb");
    let mut t = Typing::new("  x;");
    t.select(0..4);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "    x;");
}

#[test]
fn a_brace_typed_with_other_text_goes_in_with_it_in_order() {
    let mut t = Typing::new("if (a) {\n    ");
    t.select(13..13);
    t.frame(vec![egui::Event::Text("a".into()), egui::Event::Text("}".into())]);
    assert_eq!(t.text, "if (a) {\n    a}");
}

#[test]
fn lines_ended_by_a_lone_carriage_return_are_lines_and_numbered() {
    // Acrobat's own line end: egui breaks rows only at `\n`, so the box lays a lone `\r` out as one.
    let mut t = Typing::new("var a = 1;\rvar b = 2;\r\nvar c = 3;");
    t.frame(Vec::new());
    let numbers: Vec<&str> = t.said.iter().map(String::as_str).filter(|s| s.chars().all(|c| c.is_ascii_digit())).collect();
    assert_eq!(numbers, ["1", "2", "3"]);
    assert_eq!(t.text, "var a = 1;\rvar b = 2;\r\nvar c = 3;", "the script keeps its own line ends");
    let job = layout_job("a\rb\r\nc", &[], &palette(), &egui::FontId::monospace(12.0));
    assert_eq!(job.text, "a\nb\r\nc");
}

#[test]
fn an_edit_the_editor_makes_scrolls_the_box_to_the_caret() {
    // Six rows of box, twelve lines of script, the caret at the end: Enter is the editor's own
    // edit, which the text box does not see, so the editor scrolls it there.
    let script: String = (1..=12).map(|i| format!("x{i} = {i};")).collect::<Vec<_>>().join("\n");
    let mut t = Typing::new(&script);
    let end = script.chars().count();
    t.select(end..end);
    t.frame(Vec::new());
    let top = |t: &Typing| t.shapes.iter().find(|(s, _, _)| s.starts_with("x1 =")).map(|(_, rect, clip)| (rect.top(), clip.top()));
    let (before, clip) = top(&t).expect("the script is drawn");
    t.key(egui::Key::Enter, egui::Modifiers::NONE);
    // egui animates the scroll.
    for _ in 0..10 {
        t.frame(Vec::new());
    }
    let (after, _) = top(&t).expect("the script is drawn");
    assert!(after < clip && after < before, "scrolled down to the new line: {before} → {after} (box top {clip})");
}

#[test]
fn a_caret_inside_a_windows_line_end_is_before_it() {
    // End puts egui's caret after the `\r` of a `\r\n`: Enter split the line end there, and Tab
    // on just the `\n` doubled the `\r`.
    let mut t = Typing::new("  a;\r\n  b;");
    t.select(5..5);
    t.key(egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(t.text, "  a;\r\n  \r\n  b;");
    let mut t = Typing::new("  a;\r\n  b;");
    t.select(5..6);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "    a;\r\n  b;");
}

#[test]
fn the_arrows_cross_a_windows_line_end() {
    // egui stops between the `\r` and the `\n`; moved back there every frame, Right never got past.
    let mut t = Typing::new("a;\r\nb;");
    t.select(2..2);
    t.key(egui::Key::ArrowRight, egui::Modifiers::NONE);
    t.key(egui::Key::ArrowRight, egui::Modifiers::NONE);
    t.frame(Vec::new());
    assert_eq!(t.caret(), Some(4..4));
    t.select(2..2);
    t.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
    t.key(egui::Key::ArrowRight, egui::Modifiers::SHIFT);
    t.frame(Vec::new());
    assert_eq!(t.caret(), Some(2..4));
    // Typing with the caret inside it goes before the line end.
    t.select(3..3);
    t.typed("x");
    assert_eq!(t.text, "a;x\r\nb;");
}

#[test]
fn ctrl_tab_leaves_the_selection_alone() {
    let mut t = Typing::new("a\nb");
    t.select(0..3);
    t.key(egui::Key::Tab, egui::Modifiers::CTRL);
    assert_eq!(t.text, "a\nb");
}

#[test]
fn an_edit_keeps_a_backward_selection_backward() {
    let mut t = Typing::new("a\nb\nc");
    set_selection_facing(&t.ctx, t.id, 0..3, true);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "  a\n  b\nc");
    assert!(selected_backward(&t.ctx, t.id), "the caret stays at the start");
}

#[test]
fn backspace_and_delete_inside_a_windows_line_end_act_on_the_whole_of_it() {
    // End puts the caret after the `\r`: Backspace removed only that and Delete only the `\n`, so
    // nothing showed, or half a line end was left behind.
    let mut t = Typing::new("a;\r\nb;");
    t.select(3..3);
    t.key(egui::Key::Backspace, egui::Modifiers::NONE);
    assert_eq!(t.text, "a\r\nb;");
    let mut t = Typing::new("a;\r\nb;");
    t.select(3..3);
    t.key(egui::Key::Delete, egui::Modifiers::NONE);
    assert_eq!(t.text, "a;b;");
}

#[test]
fn copy_takes_no_half_of_a_windows_line_end() {
    // A selection up to End stops after the `\r`; pasted elsewhere, a lone `\r` is a line break.
    let mut t = Typing::new("ab\r\nc");
    t.select(0..3);
    t.frame(vec![egui::Event::Copy]);
    assert_eq!(t.caret(), Some(0..2));
}

/// The box this branch bounds and scrolls keeps both things under it working: the note in place of
/// a script past the limit, with its Clear button for one under it.
#[test]
fn a_full_box_keeps_its_height_and_a_long_script_gets_only_the_note() {
    let ctx = egui::Context::default();
    // Past the limit: the note and the button, and no box.
    let mut text = "x".repeat(MAX_EDITED_BYTES + 1);
    let (note, said) = measured(&ctx, &mut text, 1.0, Vec::new());
    assert!(note < 120.0, "{note}");
    assert!(said.iter().any(|s| s.contains("too long")) && said.iter().any(|s| s == "Clear script"), "{said:?}");
    // At it, on one line: no taller than an empty box.
    let ctx = egui::Context::default();
    let (empty, _) = measured(&ctx, &mut String::new(), 1.0, Vec::new());
    let ctx = egui::Context::default();
    let mut text = "x".repeat(MAX_EDITED_BYTES);
    measured(&ctx, &mut text, 1.0, Vec::new());
    let (full, said) = measured(&ctx, &mut text, 1.1, Vec::new());
    assert!((full - empty).abs() < 0.5, "{empty} then {full}");
    assert!(!said.iter().any(|s| s.contains("too long")), "the box, not the note");
}

/// Cmd+Z restores a caret inside a `\r\n` (where End leaves it). Moved off it before egui compared
/// it with the undo point, every further Cmd+Z went back to that same point, and redo was lost.
#[test]
fn undo_and_redo_go_all_the_way_on_a_windows_script() {
    let mut t = Typing::new("a;\r\nb;");
    t.select(0..0);
    t.key(egui::Key::End, egui::Modifiers::NONE);
    assert_eq!(t.caret(), Some(3..3), "End stops inside the line end");
    t.idle();
    t.typed("x");
    t.idle();
    t.key(egui::Key::End, egui::Modifiers::NONE);
    t.idle();
    t.typed("y");
    t.idle();
    assert_eq!(t.text, "a;xy\r\nb;");
    let mut seen = Vec::new();
    for _ in 0..8 {
        t.key(egui::Key::Z, egui::Modifiers::COMMAND);
        seen.push(t.text.clone());
        t.idle();
    }
    assert_eq!(t.text, "a;\r\nb;", "each Cmd+Z goes further back: {seen:?}");
    let redo = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    for _ in 0..8 {
        t.key(egui::Key::Z, redo);
        t.idle();
    }
    assert_eq!(t.text, "a;xy\r\nb;", "and redo brings it all back");
    // Cmd+Y as well.
    t.key(egui::Key::Z, egui::Modifiers::COMMAND);
    t.idle();
    t.key(egui::Key::Y, egui::Modifiers::COMMAND);
    assert_eq!(t.text, "a;xy\r\nb;");
}

/// Beside typing in one frame, the caret would be moved off the line end for the typing before Undo
/// looked at it. Undo would then put it back where the typing goes in.
#[test]
fn undo_and_typing_in_one_frame_leave_a_windows_line_end_whole() {
    let mut t = Typing::new("a;\r\nb;");
    t.select(0..0);
    t.key(egui::Key::End, egui::Modifiers::NONE);
    t.idle();
    t.typed("x");
    t.idle();
    t.key(egui::Key::End, egui::Modifiers::NONE);
    t.idle();
    assert_eq!(t.caret(), Some(4..4), "inside the line end");
    t.frame(vec![press(egui::Key::Z, egui::Modifiers::COMMAND), egui::Event::Text("y".into())]);
    t.settle();
    assert_eq!(t.text, "a;xy\r\nb;");
}

#[test]
fn tab_keeps_its_size_however_often_it_is_pressed() {
    // Worked out again after each press, the indentation just added made the next one bigger.
    let mut t = Typing::new("x");
    t.select(0..0);
    let mut widths = Vec::new();
    for _ in 0..5 {
        t.key(egui::Key::Tab, egui::Modifiers::NONE);
        widths.push(t.text.len() - 1);
    }
    assert_eq!(widths, [2, 4, 6, 8, 10]);
    for _ in 0..5 {
        t.key(egui::Key::Tab, egui::Modifiers::SHIFT);
    }
    assert_eq!(t.text, "x");
}

#[test]
fn tab_then_shift_tab_gives_the_script_back() {
    for (script, selection) in [("if (a) {\n  x;\n}", 11..11), ("ab", 1..1), ("  ab", 3..3), ("", 0..0), ("\tx", 1..1), ("    a\n    b", 2..8)] {
        let mut t = Typing::new(script);
        t.select(selection.clone());
        t.key(egui::Key::Tab, egui::Modifiers::NONE);
        assert_ne!(t.text, script, "{script:?}");
        t.key(egui::Key::Tab, egui::Modifiers::SHIFT);
        assert_eq!(t.text, script, "at {selection:?}");
        assert_eq!(t.caret(), Some(selection), "{script:?}");
    }
}

#[test]
fn the_indentation_is_worked_out_again_for_a_new_script_or_a_paste() {
    let mut t = Typing::new("a\n  b");
    t.select(0..0);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "  a\n  b");
    // Another field's script, in the same box.
    t.text = "a\n    b".into();
    t.frame(Vec::new());
    t.select(0..0);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "    a\n    b");
    // Code pasted in, with its own indentation.
    let mut t = Typing::new("a");
    t.select(1..1);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "a  ");
    t.key(egui::Key::Tab, egui::Modifiers::SHIFT);
    t.select(0..1);
    t.frame(vec![egui::Event::Paste("b\n\tc".into())]);
    assert_eq!(t.text, "b\n\tc");
    t.select(0..0);
    t.key(egui::Key::Tab, egui::Modifiers::NONE);
    assert_eq!(t.text, "\tb\n\tc");
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

    /// In the box, where the indentation it adds could change what it takes away. Tab over a selection
    /// inside a line types over it, which only Undo gives back, so that one is taken as a caret.
    #[test]
    fn tab_then_shift_tab_gives_any_script_back(
        parts in proptest::collection::vec(proptest::sample::select(&["a", "b;", "{", "}", " ", "  ", "    ", "\t", "\n", "\r", "\r\n", "é", "x = 1;"][..]), 0..9),
        from in proptest::prelude::any::<proptest::sample::Index>(),
        to in proptest::prelude::any::<proptest::sample::Index>(),
    ) {
        let script = parts.concat();
        let positions = script.chars().count() + 1;
        let snapped = |i: usize| if edit::inside_crlf(&script, i) { i - 1 } else { i };
        let (a, b) = (snapped(from.index(positions)), snapped(to.index(positions)));
        let selection = a.min(b)..a.max(b);
        let selection = if edit::indents_lines(&script, &selection) { selection } else { selection.start..selection.start };
        let mut t = Typing::new(&script);
        t.select(selection.clone());
        t.key(egui::Key::Tab, egui::Modifiers::NONE);
        t.key(egui::Key::Tab, egui::Modifiers::SHIFT);
        proptest::prop_assert_eq!(&t.text, &script);
        proptest::prop_assert_eq!(t.caret(), Some(selection));
    }
}

/// What a box holding `start`, with the caret at `caret`, ends up holding after `events` come in one
/// frame, as they do when frames are slow.
fn after_one_frame(start: &str, caret: usize, events: Vec<egui::Event>) -> String {
    let mut t = Typing::new(start);
    t.select(caret..caret);
    t.frame(events);
    t.settle();
    t.text
}

#[test]
fn keys_of_one_frame_take_effect_in_their_order() {
    let none = egui::Modifiers::NONE;
    let enter = || press(egui::Key::Enter, none);
    let text = |s: &str| egui::Event::Text(s.into());
    assert_eq!(after_one_frame("", 0, vec![text("x"), enter(), text("y")]), "x\ny");
    assert_eq!(after_one_frame("ab", 2, vec![press(egui::Key::Backspace, none), enter()]), "a\n");
    assert_eq!(after_one_frame("a", 1, [tap(egui::Key::Enter), tap(egui::Key::Enter)].concat()), "a\n\n");
    assert_eq!(after_one_frame("a", 1, [tap(egui::Key::Tab), tap(egui::Key::Tab)].concat()), "a    ");
    assert_eq!(
        after_one_frame("", 0, [tap(egui::Key::Enter).to_vec(), vec![text("x")], tap(egui::Key::Enter).to_vec(), vec![text("y")]].concat()),
        "\nx\ny"
    );
    assert_eq!(after_one_frame("ab", 2, vec![press(egui::Key::ArrowLeft, none), enter()]), "a\nb");
    assert_eq!(
        after_one_frame(
            "a",
            1,
            [vec![text("b")], tap(egui::Key::Tab).to_vec(), vec![press(egui::Key::Tab, egui::Modifiers::SHIFT), text("c")]].concat()
        ),
        "abc"
    );
    assert_eq!(after_one_frame("ab", 2, vec![text("c"), press(egui::Key::Z, egui::Modifiers::COMMAND), text("d")]), "abd", "Undo in its place");
    // A `}` after something else that edits is typed, in its place: the level stays.
    assert_eq!(after_one_frame("if (a) {\n    ", 13, vec![press(egui::Key::Backspace, none), text("}")]), "if (a) {\n   }");
}

#[test]
fn keys_waiting_for_the_next_frame_ask_for_it() {
    let mut t = Typing::new("a");
    t.select(1..1);
    t.frame(Vec::new());
    t.frame([tap(egui::Key::Enter), tap(egui::Key::Enter), tap(egui::Key::Enter), tap(egui::Key::Enter), tap(egui::Key::Enter)].concat());
    // Input gets the frames after it from egui itself, two of them.
    t.frame(Vec::new());
    t.frame(Vec::new());
    assert_eq!(t.text, "a\n\n\n");
    // At once: other things ask for a frame later (the check, the caret's blink), which would do too.
    let soonest = Arc::new(std::sync::Mutex::new(std::time::Duration::MAX));
    let asked = soonest.clone();
    t.ctx.set_request_repaint_callback(move |info| {
        let mut soonest = asked.lock().expect("no test thread panics holding it");
        *soonest = (*soonest).min(info.delay);
    });
    t.frame(Vec::new());
    assert_eq!(t.text, "a\n\n\n\n");
    assert_eq!(*soonest.lock().expect("no test thread panics holding it"), std::time::Duration::ZERO, "no input may come for a while");
    t.frame(Vec::new());
    assert_eq!(t.text, "a\n\n\n\n\n");
}

#[test]
fn a_key_held_down_does_not_pile_up_behind_slow_frames() {
    // Repeats as they come while a frame takes long: one for each frame, none left when it is up.
    let mut t = Typing::new("a");
    t.select(1..1);
    t.frame(Vec::new());
    let enter = || press(egui::Key::Enter, egui::Modifiers::NONE);
    t.frame(vec![enter(), enter(), enter(), enter()]);
    t.frame(Vec::new());
    assert_eq!(t.text, "a\n");
    // Up and Down are the box's, and stay together: nothing waits for them.
    let mut t = Typing::new("a\nb\nc");
    t.select(0..0);
    t.frame(Vec::new());
    let down = || press(egui::Key::ArrowDown, egui::Modifiers::NONE);
    t.frame(vec![
        down(),
        down(),
        egui::Event::Key { key: egui::Key::ArrowDown, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE },
    ]);
    assert_eq!(t.caret(), Some(4..4));
}

#[test]
fn keys_waiting_are_dropped_with_the_focus() {
    // They were meant for the box. Typed there later, they would be a surprise.
    let mut t = Typing::new("a");
    t.select(1..1);
    t.frame(vec![press(egui::Key::Enter, egui::Modifiers::NONE), egui::Event::Text("x".into())]);
    t.ctx.memory_mut(|m| m.surrender_focus(t.id));
    t.frame(Vec::new());
    t.ctx.memory_mut(|m| m.request_focus(t.id));
    t.settle();
    assert_eq!(t.text, "a\n");
}

/// `paint_gutter` and `layout_job` lean on this: a line ended by U+2028 or U+2029 stays on the row of
/// the line after it, as nothing here installs a line breaker that marks them, and egui breaks rows
/// only where one says so. If that changes, the parity note's "not yet" is out of date too.
#[test]
fn a_line_separator_does_not_start_a_row() {
    for separator in ['\u{2028}', '\u{2029}'] {
        let script = format!("var a;{separator}var b;\nvar c;");
        let mut t = Typing::new(&script);
        t.frame(Vec::new());
        let numbers: Vec<&str> = t.said.iter().map(String::as_str).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).collect();
        assert_eq!(numbers, ["1", "2"], "{separator:?}");
    }
}

/// A paste that brings no indentation of its own leaves the size the editor has been using: worked
/// out again from what the editor added, Tab grew.
#[test]
fn a_paste_with_no_indented_line_keeps_the_indentation() {
    let tab = egui::Key::Tab;
    let none = egui::Modifiers::NONE;
    let mut t = Typing::new("x");
    t.select(0..0);
    t.key(tab, none);
    t.key(tab, none);
    assert_eq!(t.text, "    x");
    t.frame(vec![egui::Event::Paste("y".into())]);
    t.key(tab, none);
    assert_eq!(t.text, "    y  x");
    // More lines, none indented.
    t.frame(vec![egui::Event::Paste("a\nb\r\n\n".into())]);
    t.key(tab, none);
    assert_eq!(t.text, "    y  a\nb\r\n\n  x");
    // A line that is indented sets it, with any line end before it and after a blank one.
    for paste in ["a\n    b", "a\r    b", "    b", "\n    b"] {
        let mut t = Typing::new("x");
        t.select(0..0);
        t.key(tab, none);
        t.select(0..3);
        t.frame(vec![egui::Event::Paste(paste.into())]);
        t.select(0..0);
        t.key(tab, none);
        assert_eq!(t.text, format!("    {paste}"), "{paste:?}");
    }
    // White space alone is no indentation: worked out again, the size would come from the first line.
    let mut t = Typing::new("x\ny");
    t.select(0..0);
    t.key(tab, none);
    t.key(tab, none);
    assert_eq!(t.text, "    x\ny");
    t.select(7..7);
    t.frame(vec![egui::Event::Paste("  \n".into())]);
    t.key(tab, none);
    assert_eq!(t.text, "    x\ny  \n  ");
}

/// What the editor adds itself meets the same limit as what the box types.
#[test]
fn what_the_editor_adds_is_refused_past_the_limit_too() {
    let none = egui::Modifiers::NONE;
    let refused = |t: &Typing| t.ctx.data(|d| d.get_temp::<Seen>(t.id.with("script-seen"))).is_some_and(|c| c.refused);
    // Enter with the indentation it keeps, one byte short of the room for it.
    let script = format!("  {}", "x".repeat(MAX_EDITED_BYTES - 4));
    let mut t = Typing::new(&script);
    let end = script.chars().count();
    t.select(end..end);
    t.key(egui::Key::Enter, none);
    assert!(t.text == script, "{} bytes", t.text.len());
    assert!(refused(&t));
    assert!(t.said.iter().any(|s| s.contains(REFUSED)), "the line under the box says why: {:?}", t.said.iter().map(String::len).collect::<Vec<_>>());
    t.select(0..0);
    t.key(egui::Key::Tab, none);
    assert!(t.text == format!("  {script}"), "Tab fits, in the two bytes left: {} bytes", t.text.len());
    assert!(!refused(&t), "and it ends the refusal");
    t.select(0..0);
    t.key(egui::Key::Tab, none);
    assert_eq!(t.text.len(), MAX_EDITED_BYTES, "at the limit");
    assert!(refused(&t), "a Tab past it is refused");
    t.key(egui::Key::Tab, egui::Modifiers::SHIFT);
    assert!(t.text.len() < MAX_EDITED_BYTES, "Shift+Tab gives room");
    assert!(!refused(&t));
}

/// However egui comes to edit next to a `\r\n`, it stays whole. `snap_off_crlf` can't cover these: the
/// caret gets there in the same frame, the paste brings its own place, or the delete ends there.
#[test]
fn nothing_the_box_does_splits_a_windows_line_end() {
    let none = egui::Modifiers::NONE;
    // End and typing in one frame.
    let mut t = Typing::new("a;\r\nb;");
    t.select(0..0);
    t.frame(vec![press(egui::Key::End, none), egui::Event::Text("x".into())]);
    assert_eq!(t.text, "a;x\r\nb;");
    assert_eq!(t.caret(), Some(3..3), "after what was typed");
    // Typing with the caret left inside by End, in the frame the caret is moved off.
    let mut t = Typing::new("a;\r\nb;");
    t.select(0..0);
    t.key(egui::Key::End, none);
    assert_eq!(t.caret(), Some(3..3), "egui's End leaves it inside: the case this is about");
    t.typed("x");
    assert_eq!(t.text, "a;x\r\nb;");
    // A delete to the end of the line.
    let mut t = Typing::new("ab\r\ncd");
    t.select(1..1);
    t.key(egui::Key::K, egui::Modifiers::CTRL);
    assert_eq!(t.text, "a\r\ncd", "the line end stays as it was");
    let mut t = Typing::new("ab\rcd\r\nef");
    t.select(4..4);
    t.key(egui::Key::K, egui::Modifiers::CTRL);
    assert_eq!(t.text, "ab\rc\r\nef");
    // A delete of the whole line end still goes.
    let mut t = Typing::new("a\r\nb");
    t.select(1..1);
    t.key(egui::Key::Delete, none);
    assert_eq!(t.text, "ab");
    // Middle-click paste past the end of a line: it brings its own place.
    let mut t = Typing::new("a;\r\nb;");
    t.frame(Vec::new());
    let row = t.shapes.iter().find(|(s, _, _)| s.starts_with("a;")).map(|(_, r, _)| *r).expect("the script is drawn");
    let pos = egui::pos2(row.left() + 200.0, row.top() + 4.0);
    t.frame(vec![egui::Event::PointerMoved(pos)]);
    t.frame(vec![egui::Event::MiddleClickPaste { pos, text: "X".into() }]);
    t.settle();
    assert_eq!(t.text, "a;X\r\nb;");
}

/// Copy with no selection copies nothing, so it must not move a caret Undo put back, or Redo is lost.
#[test]
fn copy_with_no_selection_leaves_the_redo() {
    let none = egui::Modifiers::NONE;
    let mut t = Typing::new("a;\r\nb;");
    t.select(0..0);
    t.key(egui::Key::End, none);
    t.idle();
    t.typed("x");
    t.idle();
    t.key(egui::Key::Z, egui::Modifiers::COMMAND);
    t.settle();
    assert_eq!(t.text, "a;\r\nb;");
    assert_eq!(t.caret(), Some(3..3), "Undo put the caret back inside the line end");
    t.frame(vec![egui::Event::Copy]);
    t.key(egui::Key::Z, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
    t.settle();
    assert_eq!(t.text, "a;x\r\nb;", "Redo still works");
}

/// Each token is a section egui shapes on its own, so past a limit the rest of a script is plain.
#[test]
fn a_very_long_script_is_coloured_only_so_far() {
    let source = "1,".repeat(100_000);
    let tokens = pdfcraft_engine::js::highlight(&source);
    assert!(tokens.len() > MAX_COLOURED_TOKENS, "{}", tokens.len());
    let job = layout_job(&source, &tokens, &palette(), &egui::FontId::monospace(12.0));
    assert_eq!(job.text, source, "nothing lost");
    assert!(job.sections.len() <= MAX_COLOURED_TOKENS * 2 + 1, "{} sections", job.sections.len());
    let last = job.sections.last().expect("a section");
    assert_eq!(last.format.color, palette().text, "the rest is plain");
    assert_eq!(last.byte_range.end.0, source.len());
}
