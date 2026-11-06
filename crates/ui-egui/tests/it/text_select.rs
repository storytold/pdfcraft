//! Issue #295: quick clicks on text widen the selection, as in Acrobat: two clicks select a
//! word, three the line, four all the text on the page.

use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// One 300×200 page with two lines of Helvetica text.
fn two_lines() -> Vec<u8> {
    text_pdf("BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj 0 -24 Td (jumps over the dog) Tj ET")
}

fn text_pdf(content: &str) -> Vec<u8> {
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes()
}

/// The page on screen and the pointer over the "u" of "quick". Frames are 1/60 s: a click takes
/// two frames (press, release), and egui counts a double-click only within 0.3 s.
fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> (Harness<'static, PdfCraftApp>, Pos2) {
    harness_with(two_lines(), setup)
}

/// `harness` with another document; the pointer ends over glyph 5 of its first page.
fn harness_with(pdf: Vec<u8>, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> (Harness<'static, PdfCraftApp>, Pos2) {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("lines.pdf", None, pdf).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        setup(&mut app);
        app
    });
    // Wait for the text layer itself (#1277), not for a fixed second.
    crate::support::wait_until(&mut h, "the page's text layer", |app| app.views[0].glyph_screen_pos(0, 5).is_some() && !app.render_pending());
    let at = h.state().views[0].glyph_screen_pos(0, 5).expect("text layer loaded");
    h.hover_at(at);
    h.run_steps(1);
    (h, at)
}

/// Press and release without moving or leaving the page.
fn click(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2) {
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(1);
}

#[test]
fn quick_clicks_select_a_word_then_the_line_then_the_page() {
    let (mut h, at) = harness(|_| {});
    let page = "The quick brown fox\njumps over the dog";
    for (n, want) in [None, Some("quick"), Some("The quick brown fox"), Some(page), Some(page)].into_iter().enumerate() {
        click(&mut h, at);
        assert_eq!(h.state().views[0].selected_text().as_deref(), want, "after {} clicks", n + 1);
    }
    // A pause ends the run: the next double-click selects a word again.
    h.run_steps(60);
    click(&mut h, at);
    click(&mut h, at);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
}

#[test]
fn quick_clicks_with_the_highlighter_mark_only_the_word() {
    let (mut h, at) = harness(|app| assert!(app.execute("comment.highlight")));
    for _ in 0..4 {
        click(&mut h, at);
        // The double-click's highlight re-renders the page; give its text layer up to 8 frames
        // to come back (still well inside egui's 0.3 s multi-click window at 1/60 s a frame), so
        // a loaded machine doesn't make the later clicks land on a page with no text yet.
        for _ in 0..8 {
            if !h.state().render_pending() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.run_steps(1);
        }
    }
    h.run_steps(3);
    // Under load the mark can land later; wait for it (egui's clock is fixed per frame, so
    // waiting can't turn the clicks into a different run). By state, not for a fixed second
    // (#1277).
    crate::support::wait_until(&mut h, "the highlight and the page after it", |app| {
        app.session.get(app.views[0].id).is_some_and(|d| !d.info.annotations.is_empty()) && !app.render_pending()
    });
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let marks: Vec<(&str, usize)> = doc.info.annotations.iter().map(|a| (a.subtype.as_str(), a.quads.len())).collect();
    assert_eq!(marks, [("Highlight", 1)], "one highlight, on the double-clicked word");
    assert!(app.views[0].selected_text().is_none());
}

/// Press at `from`, move to `to` and release there.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
}

/// Click at `pos` with Shift held.
fn shift_click(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2) {
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.hover_at(pos);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT });
    }
    h.run_steps(1);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

/// Issue #527: ⇧-click extends the text selection to the click, keeping its anchor.
#[test]
fn shift_click_extends_the_selection_from_its_anchor() {
    let (mut h, _) = harness(|_| {});
    let glyph = |h: &Harness<'static, PdfCraftApp>, g: usize| h.state().views[0].glyph_screen_pos(0, g).expect("glyph on screen");
    // Drag over "quick" (glyphs 4..=8).
    let (q, k) = (glyph(&h, 4), glyph(&h, 8));
    drag(&mut h, q, k);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
    h.run_steps(60);
    // ⇧-click on the "n" of "brown": the selection runs from the anchor to it.
    let n = glyph(&h, 14);
    shift_click(&mut h, n);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick brown"));
    h.run_steps(60);
    // ⇧-click before the anchor: the anchor stays, the selection now runs back to the click.
    let t = glyph(&h, 0);
    shift_click(&mut h, t);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("The q"));
    h.run_steps(60);
    // A plain click still clears it.
    click(&mut h, n);
    assert_eq!(h.state().views[0].selected_text(), None);
    h.run_steps(60);
    // ⇧-click with nothing selected selects nothing.
    shift_click(&mut h, n);
    assert_eq!(h.state().views[0].selected_text(), None);
}

/// Issue #1024: multi-click selection triggers immediately on pointer press, before release.
#[test]
fn multiclick_selects_on_pointer_press_before_release() {
    let (mut h, at) = harness(|_| {});
    // First click: press and release.
    click(&mut h, at);
    // Second click: press and HOLD (do not release).
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"), "word highlights on press");
    // Release the second click: still selected.
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"), "word stays selected on release");

    // Third click: press and HOLD.
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("The quick brown fox"), "line highlights on press");
    // Release third click.
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("The quick brown fox"), "line stays selected on release");
}

// Issue #1225: page-text Copy belongs in both menus and observes PDF permissions.
fn copied(h: &Harness<'static, PdfCraftApp>, want: &str) -> bool {
    h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == want))
}

/// Check that both the Edit menu and the context menu copy the selected page text.
#[test]
fn selected_text_copy_edit_and_context_menus() {
    use egui_kittest::kittest::Queryable;
    let (mut h, at) = harness(|_| {});
    click(&mut h, at);
    click(&mut h, at);
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Hand;
    h.run_steps(2); // Hide floating Copy so this test can only reach menu entries.
    crate::support::open_menu(&mut h, "Edit");
    crate::support::menu_row(&h, "Copy").click();
    let mut did_copy = false;
    for _ in 0..4 {
        h.step();
        did_copy |= copied(&h, "quick");
    }
    assert!(did_copy, "Edit Copy emits the selected text");
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Hand;
    h.run_steps(2); // Hide the floating bar: the remaining Copy must be in the context menu.
    h.event(Event::PointerMoved(at));
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    h.get_by_label_contains("Copy").click();
    let mut did_copy = false;
    for _ in 0..4 {
        h.step();
        did_copy |= copied(&h, "quick");
    }
    assert!(did_copy, "context Copy emits the selected text");
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
}

#[test]
fn escape_closes_an_open_menu_before_it_clears_the_selection() {
    // #3535: one Escape closed the menu and dropped the selection with it.
    let (mut h, at) = harness(|_| {});
    click(&mut h, at);
    click(&mut h, at);
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
    crate::support::open_menu(&mut h, "View");
    assert!(egui::Popup::is_any_open(&h.ctx), "the menu is open");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!egui::Popup::is_any_open(&h.ctx), "Escape closes the menu");
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"), "and keeps the selection");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text(), None, "a second Escape clears it");
}

#[test]
fn selected_text_copy_command_requires_selection() {
    let (mut h, at) = harness(|_| {});
    assert!(!h.state_mut().execute("edit.copy"));
    click(&mut h, at);
    click(&mut h, at);
    assert!(h.state_mut().execute("edit.copy"));
    h.step();
    assert!(copied(&h, "quick"));
    let mut app = PdfCraftApp::new();
    assert!(!app.execute("edit.copy"));
}

#[test]
fn selected_text_copy_floating_button_honours_permissions() {
    use egui_kittest::kittest::Queryable;
    for password in ["pw", "owner"] {
        let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(two_lines())).unwrap();
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "pw",
            owner_password: "owner",
            permissions: -1 ^ 0b1_0000,
            encrypt_metadata: true,
            seed: [4; 32],
        })
        .unwrap();
        let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("no-copy.pdf", None, bytes).unwrap();
            app.submit_password(Some(password.into()));
            app.set_option("left", "closed").unwrap();
            app
        });
        let mut loaded = false;
        for _ in 0..200 {
            h.run_steps(2);
            if h.state_mut().views[0].select_all() {
                loaded = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(loaded);
        h.run_steps(2);
        h.get_by_label("Copy").click();
        let mut did_copy = false;
        for _ in 0..4 {
            h.step();
            did_copy |= copied(&h, "The quick brown fox\njumps over the dog");
        }
        assert_eq!(did_copy, password == "owner", "floating Copy with {password} password");
        assert_eq!(h.state_mut().execute("edit.copy"), password == "owner");
    }
}

#[test]
fn selected_text_copy_preserves_unicode_and_page_clipboard() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.system_clipboard = false;
        app.open_bytes("unicode.pdf", None, text_pdf(r"BT /F1 14 Tf 20 150 Td (caf\351) Tj ET")).unwrap();
        app
    });
    let mut loaded = false;
    for _ in 0..200 {
        h.run_steps(2);
        if h.state_mut().views[0].select_all() {
            loaded = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(loaded);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("café"));
    assert!(h.state_mut().execute("edit.copy"));
    h.step();
    assert!(copied(&h, "café"));
    assert!(h.state().page_clipboard.is_none());
    assert!(h.state_mut().execute("page.organize"));
    h.run_steps(2);
    assert!(!h.state_mut().execute("edit.copy"), "Organize retains page clipboard ownership");
    assert!(h.state_mut().execute("page.copy"));
    assert_eq!(h.state().page_clipboard.as_ref().map(|c| c.pages.clone()), Some(vec![0]));
}

/// Check that a focused search input owns Copy and the selected page text does not also consume it.
#[test]
fn selected_text_copy_keeps_focused_input_clipboard_ownership() {
    let (mut h, at) = harness(|_| {});
    click(&mut h, at);
    click(&mut h, at);
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "widget café".into();
    h.run_steps(3);
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::unique("find-input")));
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.run_steps(2);
    h.event(Event::Copy);
    let mut widget_copy = false;
    let mut page_copy = false;
    for _ in 0..4 {
        h.step();
        widget_copy |= copied(&h, "widget café");
        page_copy |= copied(&h, "quick");
    }
    assert!(widget_copy, "the focused text input copies its selected text");
    assert!(!page_copy, "page text must not also consume the Copy event");
}

/// `n` pages of 300×200, each ending with one line of Helvetica text near its bottom edge.
fn bottom_lines(n: usize) -> Vec<u8> {
    let mut objects = String::new();
    let mut kids = Vec::new();
    for k in 0..n {
        let (page, contents) = (4 + 2 * k, 5 + 2 * k);
        kids.push(format!("{page} 0 R"));
        let content = format!("BT /F1 14 Tf 20 20 Td (Page {} ends with this line) Tj ET", k + 1);
        objects.push_str(&format!(
            "{page} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents {contents} 0 R /Resources << /Font << /F1 3 0 R >> >> >> endobj\n\
             {contents} 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n",
            content.len()
        ));
    }
    format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{}] /Count {n} >> endobj\n\
         3 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n{objects}trailer << /Root 1 0 R >>\n%%EOF",
        kids.join(" ")
    )
    .into_bytes()
}

/// Press at `from` and move to `to`, keeping the button down.
fn press_and_move(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
}

/// One wheel event, with time to scroll (egui spreads a wheel line over a few frames).
fn wheel(h: &mut Harness<'static, PdfCraftApp>, unit: MouseWheelUnit, dy: f32) {
    h.event(Event::MouseWheel {
        unit,
        delta: egui::vec2(0.0, dy),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(10);
}

/// The top edge of `page` on screen.
fn page_top(h: &Harness<'static, PdfCraftApp>, page: usize) -> f32 {
    h.state().views[0].page_screen_rect(page).expect("page on screen").min.y
}

/// Issue #721: selecting near the bottom of a page with the button held, the wheel keeps
/// scrolling while the pointer crosses the gap onto the next page, and the selection follows the
/// pointer onto that page.
#[test]
fn a_selection_drag_keeps_scrolling_over_the_gap_and_the_next_page() {
    let (mut h, _) = harness_with(bottom_lines(3), |app| {
        app.set_option("layout", "continuous").unwrap();
        app.set_option("zoom", "200").unwrap();
    });
    let glyph = |h: &Harness<'static, PdfCraftApp>, g: usize| h.state().views[0].glyph_screen_pos(0, g).expect("glyph on screen");
    // "ends" (glyphs 7..=10) in "Page 1 ends with this line".
    let (from, to) = (glyph(&h, 7), glyph(&h, 10));
    press_and_move(&mut h, from, to);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("ends"));
    let (mut over_gap, mut over_next) = (false, false);
    for _ in 0..100 {
        let before = page_top(&h, 0);
        wheel(&mut h, MouseWheelUnit::Line, -0.1);
        assert!(page_top(&h, 0) < before - 0.5, "the wheel scrolls during the drag (over the gap yet: {over_gap})");
        let view = &h.state().views[0];
        let on = |page| view.page_screen_rect(page).is_some_and(|r| r.contains(to));
        over_gap |= !on(0) && !on(1);
        over_next = on(1);
        if over_next {
            break;
        }
    }
    assert!(over_gap && over_next, "the pointer crossed the gap ({over_gap}) onto the next page ({over_next})");
    // The next page's text layer may still be on its way: the selection reaches it once read.
    crate::support::wait_until(&mut h, "page 2's text layer", |app| app.views[0].glyph_screen_pos(1, 0).is_some());
    h.hover_at(to);
    h.run_steps(2);
    let held = h.state().views[0].selected_text().expect("still selected");
    assert!(held.starts_with("ends with this line\nPage 2"), "the selection runs on onto page 2: {held:?}");
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text(), Some(held), "releasing keeps the selection");
}

/// `bottom_lines(3)` at 75 %, every page on screen with its text layer read.
fn three_pages() -> Harness<'static, PdfCraftApp> {
    let (mut h, _) = harness_with(bottom_lines(3), |app| {
        app.set_option("layout", "continuous").unwrap();
        app.set_option("zoom", "75").unwrap();
    });
    crate::support::wait_until(&mut h, "every page's text layer", |app| {
        (0..3).all(|p| app.views[0].glyph_screen_pos(p, 10).is_some()) && !app.render_pending()
    });
    let viewport = h.state().views[0].viewport_rect();
    assert!((0..3).all(|p| h.state().views[0].page_screen_rect(p).is_some_and(|r| viewport.contains_rect(r))), "every page in view");
    h
}

fn glyph_on(h: &Harness<'static, PdfCraftApp>, page: usize, g: usize) -> Pos2 {
    h.state().views[0].glyph_screen_pos(page, g).expect("glyph on screen")
}

/// A drag from one page onto the next selects to the end of the first page and from the start
/// of the next, as in Acrobat, and Copy has both parts in reading order. Dragging the other way
/// selects the same text.
#[test]
fn a_drag_selects_text_across_pages() {
    let mut h = three_pages();
    // From "ends" on page 1 (glyph 7) to the "s" of "ends" on page 2 (glyph 10).
    let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
    drag(&mut h, from, to);
    let want = "ends with this line\nPage 2 ends";
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some(want), "dragging down");
    h.run_steps(60);
    drag(&mut h, to, from);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some(want), "dragging up");
    // Over three pages: the middle one is selected whole.
    h.run_steps(60);
    let page3 = glyph_on(&h, 2, 3);
    drag(&mut h, from, page3);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("ends with this line\nPage 2 ends with this line\nPage"));
    // ⌘C copies all of it.
    h.event(Event::Copy);
    let mut did_copy = false;
    for _ in 0..4 {
        h.step();
        did_copy |= copied(&h, "ends with this line\nPage 2 ends with this line\nPage");
    }
    assert!(did_copy, "Copy has the text of every page");
}

/// ⇧-click on another page extends the selection there from its anchor, either way.
#[test]
fn shift_click_extends_the_selection_onto_another_page() {
    let mut h = three_pages();
    // "ends" on page 2.
    let (from, to) = (glyph_on(&h, 1, 7), glyph_on(&h, 1, 10));
    drag(&mut h, from, to);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("ends"));
    h.run_steps(60);
    let page3 = glyph_on(&h, 2, 3);
    shift_click(&mut h, page3);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("ends with this line\nPage"));
    h.run_steps(60);
    // Before the anchor, on page 1: from the click to the anchor.
    // Glyph 22 is the "l" of "line" (spaces are glyphs too).
    let line = glyph_on(&h, 0, 22);
    shift_click(&mut h, line);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("line\nPage 2 e"));
}

/// Marking a selection that runs across pages puts one markup on each page, in one undo step.
#[test]
fn a_selection_across_pages_is_marked_on_every_page_in_one_step() {
    use egui_kittest::kittest::Queryable;
    let mut h = three_pages();
    let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
    drag(&mut h, from, to);
    h.run_steps(3);
    h.get_by_label("Underline").click();
    h.run_steps(3);
    let marks = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        doc.info.annotations.iter().map(|a| (a.page, a.subtype.clone())).collect::<Vec<_>>()
    };
    assert_eq!(marks(&h), [(0, "Underline".to_string()), (1, "Underline".to_string())]);
    assert!(h.state().views[0].selected_text().is_none(), "the selection is consumed");
    assert!(h.state().views[0].comments.selected.is_some_and(|(page, _)| page == 1), "its last part is selected, as one markup is");
    h.state_mut().undo();
    h.run_steps(3);
    assert_eq!(marks(&h), [], "one undo removes both");
}

/// One 300×600 page with a line of Helvetica text at its top and another at its bottom.
fn tall_page() -> Vec<u8> {
    let content = "BT /F1 14 Tf 20 570 Td (Top line) Tj 0 -550 Td (Bottom line) Tj ET";
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 600] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes()
}

/// Issue #1670: holding a selection drag at the bottom edge of the view scrolls it, and the
/// selection follows the pointer down to text that was off screen.
#[test]
fn a_selection_drag_held_at_the_bottom_edge_scrolls_the_view() {
    let (mut h, _) = harness_with(tall_page(), |app| {
        app.set_option("layout", "continuous").unwrap();
        app.set_option("zoom", "200").unwrap();
    });
    let view = &h.state().views[0];
    let viewport = view.viewport_rect();
    assert!(view.page_screen_rect(0).expect("page on screen").bottom() > viewport.bottom() + 100.0, "the page must not fit");
    // "Top" (glyphs 0..=2).
    let (from, to) = (view.glyph_screen_pos(0, 0).expect("glyph on screen"), view.glyph_screen_pos(0, 2).expect("glyph on screen"));
    press_and_move(&mut h, from, to);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Top"));
    let edge = egui::pos2(to.x, viewport.bottom() - 2.0);
    let before = page_top(&h, 0);
    h.hover_at(edge);
    h.run_steps(120);
    assert!(page_top(&h, 0) < before - 100.0, "the view scrolls while the pointer rests at its edge ({before} -> {})", page_top(&h, 0));
    let held = h.state().views[0].selected_text().expect("still selected");
    // The pointer stays above the "t" of "Top", so the selection ends at the "t" of "Bottom".
    assert_eq!(held, "Top line\nBot", "the selection reaches the bottom line");
    h.event(Event::PointerButton { pos: edge, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text(), Some(held), "releasing keeps the selection");
}

/// Click at `at`, then press there again and drag to `to` before releasing: a double-click drag.
fn double_click_drag(h: &mut Harness<'static, PdfCraftApp>, at: Pos2, to: Pos2) {
    click(h, at);
    press_and_move(h, at, to);
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
}

/// Issue #1685: dragging after a double-click extends the selection a word at a time, and the
/// double-clicked word stays selected whichever way the drag goes.
#[test]
fn a_double_click_drag_extends_by_whole_words_both_ways() {
    let (mut h, _) = harness(|_| {});
    let glyph = |h: &Harness<'static, PdfCraftApp>, g: usize| h.state().views[0].glyph_screen_pos(0, g).expect("glyph on screen");
    // "The quick brown fox": the "u" of "quick" is glyph 5, the "o" of "brown" glyph 12.
    let (u, o) = (glyph(&h, 5), glyph(&h, 12));
    double_click_drag(&mut h, u, o);
    let right = h.state().views[0].selected_text();
    h.run_steps(60);
    double_click_drag(&mut h, o, u);
    let left = h.state().views[0].selected_text();
    assert_eq!([right.as_deref(), left.as_deref()], [Some("quick brown"); 2], "dragging right, then left");
}

/// Issue #1685: after a triple-click the drag extends a whole line at a time.
#[test]
fn a_triple_click_drag_extends_by_whole_lines() {
    let (mut h, at) = harness(|_| {});
    // A glyph inside "jumps over the dog", on the second line.
    let below = h.state().views[0].glyph_screen_pos(0, 23).expect("glyph on screen");
    click(&mut h, at);
    double_click_drag(&mut h, at, below);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("The quick brown fox\njumps over the dog"));
}

/// Coalesced down/up keeps the same word/line selection as separate press and release passes.
#[test]
fn coalesced_primary_clicks_select_line_then_spaced_click_clears() {
    for coalesced in [true, false] {
        let (mut h, at) = harness(|_| {});
        h.ctx.options_mut(|o| o.max_passes = 1.try_into().unwrap());
        let id = h.state().views[0].id;
        let saved = h.state().session.get(id).unwrap().saved_bytes();
        assert!(h.state().views[0].selected_text().is_none());
        assert!(!h.state().session.get(id).unwrap().dirty);
        for want in [None, Some("quick"), Some("The quick brown fox")] {
            if coalesced {
                // Harness::event steps each queued event separately; RawInput keeps this pair together.
                h.input_mut().events.extend([true, false].map(|pressed| Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }));
                h.step();
            } else {
                click(&mut h, at);
            }
            assert_eq!(h.state().views[0].selected_text().as_deref(), want, "coalesced={coalesced}");
        }
        h.run_steps(60);
        if coalesced {
            h.input_mut().events.extend([true, false].map(|pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }));
            h.step();
        } else {
            click(&mut h, at);
        }
        assert_eq!(h.state().views[0].selected_text(), None, "a spaced click clears either gesture path");
        assert_eq!(h.state().views[0].id, id);
        let doc = h.state().session.get(id).unwrap();
        assert!(!doc.dirty);
        assert_eq!((doc.can_undo(), doc.can_redo()), (None, None));
        assert_eq!(doc.saved_bytes(), saved);
    }
}

/// Review on #2070: a click on the page in full screen waits out the double-click time before
/// it turns the page, and a link clicked meanwhile used to leave that turn pending: the link
/// jumped to page 3, then the page moved on to page 4.
#[test]
fn full_screen_link_click_cancels_the_pending_page_turn() {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>";
    let four_pages = String::from_utf8(two_lines())
        .unwrap()
        .replace("/Kids [3 0 R] /Count 1", "/Kids [3 0 R 6 0 R 7 0 R 8 0 R] /Count 4")
        .replace("/Contents 4 0 R /Resources", "/Annots [9 0 R] /Contents 4 0 R /Resources")
        .replace(
            "trailer",
            &format!(
                "6 0 obj {page} endobj\n7 0 obj {page} endobj\n8 0 obj {page} endobj\n9 0 obj << /Type /Annot /Subtype /Link /Rect [200 20 280 60] /Border [0 0 0] /Dest [7 0 R /Fit] >> endobj\ntrailer"
            ),
        )
        .into_bytes();
    let (mut h, _) = harness_with(four_pages, |_| {});
    h.state_mut().apply_full_screen(true);
    crate::support::wait_until(&mut h, "the page in full screen", |app| app.views[0].glyph_screen_pos(0, 5).is_some() && !app.render_pending());
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    let at = |x: f32, y: f32| egui::pos2(r.left() + x / 300.0 * r.width(), r.top() + (200.0 - y) / 200.0 * r.height());
    // A click on blank page area, then, within the double-click time, a click on the link.
    h.hover_at(at(150.0, 90.0));
    h.run_steps(1);
    click(&mut h, at(150.0, 90.0));
    h.run_steps(3);
    h.hover_at(at(240.0, 40.0));
    h.run_steps(1);
    click(&mut h, at(240.0, 40.0));
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 2, "the link's page, and no turn after it");
}

/// Review on #2070: in full screen a click goes forward a page, but a double-click selects the
/// word and stays: the click waits out the double-click time before turning. Turning on the first
/// click's release had sent the second click to the next page. Leaving full screen meanwhile
/// drops the pending turn.
#[test]
fn full_screen_double_click_selects_a_word_and_stays_on_the_page() {
    // The same page twice.
    let two_pages = String::from_utf8(two_lines())
        .unwrap()
        .replace("/Kids [3 0 R] /Count 1", "/Kids [3 0 R 6 0 R] /Count 2")
        .replace(
            "trailer",
            "6 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\ntrailer",
        )
        .into_bytes();
    let (mut h, _) = harness_with(two_pages, |_| {});
    h.state_mut().apply_full_screen(true);
    crate::support::wait_until(&mut h, "the page in full screen", |app| app.views[0].glyph_screen_pos(0, 5).is_some() && !app.render_pending());
    let at = h.state().views[0].glyph_screen_pos(0, 5).expect("text layer");
    h.hover_at(at);
    h.run_steps(1);

    // A double-click selects "quick" and stays on page 1, also once the double-click time is over.
    click(&mut h, at);
    click(&mut h, at);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 0, "the double-click didn't turn the page");

    // A single click turns it once the double-click time is over.
    // Entering full screen again clears the selection.
    h.state_mut().apply_full_screen(false);
    h.state_mut().apply_full_screen(true);
    h.run_steps(30);
    click(&mut h, at);
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 1, "a click goes forward");

    // A click right before leaving full screen doesn't turn the page after it.
    h.state_mut().views[0].go_to_page(0);
    h.run_steps(30);
    click(&mut h, at);
    h.state_mut().apply_full_screen(false);
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 0, "no page turn after leaving");
}

/// A quick drag can be off its page by the frame egui calls it a drag, in the gap or on the next
/// page: the selection still starts where the button went down and runs on to the pointer.
#[test]
fn a_flick_off_the_page_still_selects_from_the_press() {
    let mut h = three_pages();
    let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    let gap =
        egui::pos2(from.x, (h.state().views[0].page_screen_rect(0).unwrap().bottom() + h.state().views[0].page_screen_rect(1).unwrap().top()) / 2.0);
    h.hover_at(gap);
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("e"), "started at the press, on page 1; the gap moves nothing");
    h.hover_at(to);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("ends with this line\nPage 2 ends"));
}

/// The markup and Redact tools mark a drag across pages on every page it covers, in one step,
/// and so does picking a markup tool with such a selection already made.
#[test]
fn markup_tools_mark_a_selection_across_pages_on_every_page() {
    let marks = |h: &Harness<'static, PdfCraftApp>| {
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        doc.info.annotations.iter().map(|a| (a.page, a.subtype.clone())).collect::<Vec<_>>()
    };
    let both = |kind: &str| vec![(0, kind.to_string()), (1, kind.to_string())];
    for (command, kind) in [("comment.strikeout", "StrikeOut"), ("redact.mark", "Redact")] {
        // Dragging with the tool.
        let mut h = three_pages();
        assert!(h.state_mut().execute(command));
        h.run_steps(2);
        let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
        drag(&mut h, from, to);
        h.run_steps(3);
        assert_eq!(marks(&h), both(kind), "{command}: dragged with the tool");
        h.state_mut().undo();
        h.run_steps(3);
        assert_eq!(marks(&h), [], "{command}: one undo step");
        // Selecting first, then picking the tool.
        // (The tool opened the Comments panel above, which moved the pages.)
        let mut h = three_pages();
        let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
        drag(&mut h, from, to);
        assert!(h.state().views[0].selected_text().is_some());
        assert!(h.state_mut().execute(command));
        h.run_steps(3);
        assert_eq!(marks(&h), both(kind), "{command}: picked after selecting");
    }
}

/// Review of #5017: an area highlight whose drag ends in the same frame as Escape (which drops
/// the Highlight tool for Select) is still a highlight, never a redaction: the queued box keeps
/// the tool that drew it.
#[test]
fn an_area_highlight_ended_with_escape_stays_a_highlight() {
    let mut h = three_pages();
    assert!(h.state_mut().execute("comment.highlight"));
    h.run_steps(2);
    // A box in the empty top of page 1 (its text is at the bottom).
    let page = h.state().views[0].page_screen_rect(0).expect("page on screen");
    let (from, to) = (page.min + egui::vec2(20.0, 15.0), page.min + egui::vec2(120.0, 60.0));
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    // Release and Escape in one frame.
    h.input_mut().events.extend([
        Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE },
    ]);
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let marks: Vec<&str> = doc.info.annotations.iter().map(|a| a.subtype.as_str()).collect();
    assert_eq!(marks, ["Highlight"], "the box is a highlight");
    assert_eq!(doc.redaction_marks(), 0, "nothing marked for redaction");
}

/// Review of #5017: a highlight across pages is followed up as one on a single page is: the
/// comment where the selection ends is selected and its note opens for typing.
#[test]
fn a_highlight_across_pages_selects_its_last_part_and_opens_its_note() {
    let mut h = three_pages();
    let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
    drag(&mut h, from, to);
    h.run_steps(3);
    assert!(h.state_mut().execute("comment.highlight"));
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let on_page_2 = doc.info.annotations.iter().find(|a| a.page == 1 && a.subtype == "Highlight").map(|a| a.index).expect("a highlight on page 2");
    assert_eq!(app.views[0].comments.selected, Some((1, on_page_2)), "the part where the selection ends is selected");
    assert!(app.views[0].comments.editing.as_ref().is_some_and(|(p, i, _)| (*p, *i) == (1, on_page_2)), "its note is open");
}

/// Review of #5017: only markup on a text selection across pages is followed up as one markup.
/// Redact ▸ Mark Page Range, also one comment per page in one batch, selects nothing and leaves
/// the comment editor alone, as before.
#[test]
fn a_page_range_redaction_is_not_followed_up_as_a_selection() {
    let mut h = three_pages();
    h.state_mut().redact_pages_draft = pdfcraft_ui_egui::RedactPagesDraft { current: false, from: 1, to: 3 };
    h.state_mut().redact_pages();
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.redaction_marks(), 3, "a mark on every page");
    assert_eq!(app.views[0].comments.selected, None, "no mark selected");
    assert!(app.views[0].comments.editing.is_none(), "no editor opened");
}
