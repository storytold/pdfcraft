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

#[test]
fn selected_text_copy_edit_and_context_menus() {
    use egui_kittest::kittest::Queryable;
    let (mut h, at) = harness(|_| {});
    click(&mut h, at);
    click(&mut h, at);
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Hand;
    h.run_steps(2); // Hide floating Copy so this test can only reach menu entries.
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("Edit ⏵").hover();
    h.run_steps(3);
    h.get_by_label_contains("Copy").click();
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

#[test]
fn selected_text_copy_keeps_focused_input_clipboard_ownership() {
    let (mut h, at) = harness(|_| {});
    click(&mut h, at);
    click(&mut h, at);
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "widget café".into();
    h.run_steps(3);
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::new("find-input")));
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
    h.event(Event::MouseWheel { unit, delta: egui::vec2(0.0, dy), phase: TouchPhase::Move, modifiers: Modifiers::NONE });
    h.run_steps(10);
}

/// The top edge of `page` on screen.
fn page_top(h: &Harness<'static, PdfCraftApp>, page: usize) -> f32 {
    h.state().views[0].page_screen_rect(page).expect("page on screen").min.y
}

/// Issue #721: selecting near the bottom of a page with the button held, the wheel keeps
/// scrolling while the pointer crosses the gap onto the next page. The selection stays on its
/// own page (a selection can't span pages yet).
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
    let held = h.state().views[0].selected_text().expect("still selected");
    assert!(held.starts_with("ends") && "Page 1 ends with this line".contains(held.as_str()), "the selection stays on page 1: {held:?}");
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text(), Some(held), "releasing keeps the selection");
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
