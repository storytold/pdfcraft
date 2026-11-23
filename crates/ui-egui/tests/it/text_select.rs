//! Issue #295: quick clicks on text widen the selection, as in Acrobat: two clicks select a
//! word, three the line, four all the text on the page.

use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

use crate::support::AppHarness as _;
use crate::support::RecordingClipboard;

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
    harness_clip(pdf, RecordingClipboard::default(), setup)
}

/// `harness_with` on the injected `clipboard`, whose recorded writes a test reads.
fn harness_clip(
    pdf: Vec<u8>,
    clipboard: RecordingClipboard,
    setup: impl FnOnce(&mut PdfCraftApp) + 'static,
) -> (Harness<'static, PdfCraftApp>, Pos2) {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_on(clipboard);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "lines.pdf", None, pdf).expect("opens");
            app.set_option(&egui::Context::default(), "left", "closed").unwrap();
            app.set_option(&egui::Context::default(), "author", "Tester").unwrap();
            setup(&mut app);
            app
        }
    });
    // Wait for the text layer itself (#1277), not for a fixed second.
    crate::support::wait_until(&mut h, "the page's text layer", |app| app.views[0].glyph_screen_pos(0, 5).is_some() && !app.render_pending());
    let at = h.state().views[0].glyph_screen_pos(0, 5).expect("text layer loaded");
    h.hover_at(at);
    h.run_steps(1);
    (h, at)
}

/// The app on the injected `clipboard`.
fn app_on(clipboard: RecordingClipboard) -> PdfCraftApp {
    // PLAN: assumed support::app_with_clipboard(RecordingClipboard) -> PdfCraftApp (pdf-chrome-w9: `PdfCraftApp::with_host` over `AppServices` holding the clipboard)
    crate::support::app_with_clipboard(clipboard)
}

// Keep the word under test clear of the floating quick toolbar.
fn readable_zoom(app: &mut PdfCraftApp) {
    app.set_option(&egui::Context::default(), "zoom", "100%").unwrap();
}

/// What Copy with formatting wrote to the clipboard: `(html, plain text)` per write that had HTML.
fn html_sets(clipboard: &RecordingClipboard) -> Vec<(String, String)> {
    // PLAN: assumed RecordingClipboard::writes() -> Vec<Vec<Flavor>> recording every attempt, with `mime` and `data`.
    clipboard
        .writes()
        .iter()
        .filter_map(|flavors| {
            let part = |mime: &str| flavors.iter().find(|f| f.mime.to_string() == mime).map(|f| String::from_utf8_lossy(&f.data).into_owned());
            Some((part("text/html")?, part("text/plain").unwrap_or_default()))
        })
        .collect()
}

/// Press and release without moving or leaving the page.
fn click(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2) {
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(1);
}

#[test]
fn quick_clicks_with_the_highlighter_mark_only_the_word() {
    let (mut h, at) = harness(|app| assert!(app.execute(&egui::Context::default(), "comment.highlight")));
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

/// Wait for the page a mark just re-rendered to have its text layer back, glyph `g` of page 1
/// on screen, within the multi-click window: at most 12 frames of 1/60 s, with real time between
/// them for the render to finish.
fn text_back(h: &mut Harness<'static, PdfCraftApp>, g: usize) {
    for _ in 0..12 {
        if !h.state().render_pending() && h.state().views[0].glyph_screen_pos(0, g).is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
        h.run_steps(1);
    }
    panic!("the text layer did not come back within the multi-click window");
}

/// Review of #5017: with a markup tool, a third quick press after a double-click starts over
/// from one click, so its drag selects and marks what it moves over, even though the
/// double-click's mark has used up the selection.
#[test]
fn a_markup_drag_right_after_a_double_click_marks_what_it_drags_over() {
    let (mut h, at) = harness(|app| assert!(app.execute(&egui::Context::default(), "comment.underline")));
    click(&mut h, at);
    click(&mut h, at);
    // A glyph inside "jumps over the dog", on the second line.
    text_back(&mut h, 23);
    let below = h.state().views[0].glyph_screen_pos(0, 23).expect("glyph on screen");
    press_and_move(&mut h, at, below);
    h.event(Event::PointerButton { pos: below, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    crate::support::wait_until(&mut h, "the dragged underline", |app| {
        app.session.get(app.views[0].id).is_some_and(|d| d.info.annotations.len() == 2) && !app.render_pending()
    });
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let marks: Vec<(&str, usize)> = doc.info.annotations.iter().map(|a| (a.subtype.as_str(), a.quads.len())).collect();
    assert_eq!(marks, [("Underline", 1), ("Underline", 2)], "the double-clicked word, then the two lines dragged over");
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

// Issue #1225: page-text Copy belongs in both menus and observes PDF permissions.
/// Whether the clipboard the app writes through holds `want`, as the last copy put it there.
fn copied(clipboard: &RecordingClipboard, want: &str) -> bool {
    clipboard.copied().as_deref() == Some(want)
}

/// Check that both the Edit menu and the context menu copy the selected page text.
#[test]
fn selected_text_copy_edit_and_context_menus() {
    use egui_kittest::kittest::Queryable;
    let clipboard = RecordingClipboard::default();
    let (mut h, at) = harness_clip(two_lines(), clipboard.clone(), readable_zoom);
    click(&mut h, at);
    click(&mut h, at);
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Hand;
    h.run_steps(2); // Hide floating Copy so this test can only reach menu entries.
    crate::support::open_menu(&mut h, "Edit");
    crate::support::menu_row(&h, "Copy").click();
    for _ in 0..4 {
        h.step();
    }
    assert!(copied(&clipboard, "quick"), "Edit Copy emits the selected text");
    let writes = clipboard.writes().len();
    h.state_mut().quick_tool = pdfcraft_ui_egui::QuickTool::Hand;
    h.run_steps(2); // Hide the floating bar: the remaining Copy must be in the context menu.
    h.event(Event::PointerMoved(at));
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(2);
    h.get_by_label_contains("Copy").click();
    for _ in 0..4 {
        h.step();
    }
    assert!(clipboard.writes().len() > writes && copied(&clipboard, "quick"), "context Copy emits the selected text");
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
}

#[test]
fn selected_text_copy_command_requires_selection() {
    let clipboard = RecordingClipboard::default();
    let (mut h, at) = harness_clip(two_lines(), clipboard.clone(), readable_zoom);
    assert!(!h.execute("edit.copy"));
    click(&mut h, at);
    click(&mut h, at);
    assert!(h.execute("edit.copy"));
    h.step();
    assert!(copied(&clipboard, "quick"));
    let mut app = crate::control::new_app();
    assert!(!app.execute(&egui::Context::default(), "edit.copy"));
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
        let clipboard = RecordingClipboard::default();
        let app_clipboard = clipboard.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
            crate::support::install_runtime(&cc.egui_ctx);
            {
                let mut app = app_on(app_clipboard);
                app.set_option(&egui::Context::default(), "language", "en").unwrap();
                app.open_bytes(&egui::Context::default(), "no-copy.pdf", None, bytes).unwrap();
                app.submit_password(&egui::Context::default(), Some(password.into()));
                app.set_option(&egui::Context::default(), "left", "closed").unwrap();
                app
            }
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
        for _ in 0..4 {
            h.step();
        }
        let did_copy = copied(&clipboard, "The quick brown fox\njumps over the dog");
        assert_eq!(did_copy, password == "owner", "floating Copy with {password} password");
        assert_eq!(h.execute("edit.copy"), password == "owner");
    }
}

fn any_copy_text(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_)))
}

/// Copy with formatting: Copy puts the selection as HTML (font, size) and its plain text on the
/// system clipboard in one set, and doesn't also copy the text through egui, which would
/// replace the HTML. A new copy replaces it.
#[test]
fn selected_text_copy_keeps_the_formatting_as_html() {
    let clipboard = RecordingClipboard::default();
    let (mut h, at) = harness_clip(two_lines(), clipboard.clone(), readable_zoom);
    click(&mut h, at);
    click(&mut h, at);
    assert!(h.execute("edit.copy"));
    h.step();
    assert_eq!(
        html_sets(&clipboard).as_slice(),
        [("<p style=\"margin:0\"><span style=\"font-family:'Helvetica';font-size:14pt\">quick</span></p>\n".to_owned(), "quick".to_owned())]
    );
    assert!(!any_copy_text(&h), "egui's own text copy would replace the HTML");
    // Three clicks: the line, a new copy.
    h.run_steps(60);
    for _ in 0..3 {
        click(&mut h, at);
    }
    assert!(h.execute("edit.copy"));
    let last = html_sets(&clipboard).last().cloned().unwrap_or_default();
    assert!(last.0.contains(">The quick brown fox</span>"), "{last:?}");
    assert_eq!(last.1, "The quick brown fox");
}

/// When the system clipboard refuses the HTML, Copy still copies the plain text.
#[test]
fn selected_text_copy_falls_back_to_plain_text_when_html_is_refused() {
    // PLAN: assumed RecordingClipboard::refusing(mime) refuses a write that has that flavour, and records the attempt.
    let clipboard = RecordingClipboard::refusing("text/html");
    let (mut h, at) = harness_clip(two_lines(), clipboard.clone(), readable_zoom);
    click(&mut h, at);
    click(&mut h, at);
    assert!(h.execute("edit.copy"));
    h.step();
    assert_eq!(html_sets(&clipboard).len(), 1, "the HTML was offered");
    assert!(copied(&clipboard, "quick"), "the plain text goes out the usual way");
}

/// A document that withholds copying gives neither the text nor its HTML.
#[test]
fn selected_text_copy_with_formatting_honours_permissions() {
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
    let clipboard = RecordingClipboard::default();
    let (mut h, _) = harness_clip(bytes, clipboard.clone(), |app| app.submit_password(&egui::Context::default(), Some("pw".into())));
    h.set_option("zoom", "100%").unwrap();
    h.run_steps(2);
    let at = h.state().views[0].glyph_screen_pos(0, 5).expect("text layer loaded");
    h.hover_at(at);
    h.run_steps(1);
    click(&mut h, at);
    click(&mut h, at);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
    assert!(!h.execute("edit.copy"));
    h.step();
    assert!(!any_copy_text(&h));
    assert!(html_sets(&clipboard).is_empty());
}

#[test]
fn selected_text_copy_preserves_unicode_and_page_clipboard() {
    let clipboard = RecordingClipboard::default();
    let app_clipboard = clipboard.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_on(app_clipboard);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "unicode.pdf", None, text_pdf(r"BT /F1 14 Tf 20 150 Td (caf\351) Tj ET")).unwrap();
            app
        }
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
    assert!(h.execute("edit.copy"));
    h.step();
    assert!(copied(&clipboard, "café"));
    assert!(h.state().page_clipboard.is_none());
    assert!(h.execute("page.organize"));
    h.run_steps(2);
    assert!(!h.execute("edit.copy"), "Organize retains page clipboard ownership");
    assert!(h.execute("page.copy"));
    assert_eq!(h.state().page_clipboard.as_ref().map(|c| c.pages.clone()), Some(vec![0]));
}

/// Check that a focused search input owns Copy and the selected page text does not also consume it.
#[test]
fn selected_text_copy_keeps_focused_input_clipboard_ownership() {
    let clipboard = RecordingClipboard::default();
    let (mut h, at) = harness_clip(two_lines(), clipboard.clone(), |_| {});
    click(&mut h, at);
    click(&mut h, at);
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "widget café".into();
    h.run_steps(3);
    {
        use egui_kittest::kittest::Queryable;
        h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Find text").click();
    }
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    h.run_steps(2);
    h.event(Event::Copy);
    // The text input copies through egui, as every framework text input does; the page's text through the app's
    // clipboard.
    let mut widget_copy = false;
    for _ in 0..4 {
        h.step();
        widget_copy |= h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "widget café"));
    }
    let page_copy = !clipboard.writes().is_empty();
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
        app.set_option(&egui::Context::default(), "layout", "continuous").unwrap();
        app.set_option(&egui::Context::default(), "zoom", "200").unwrap();
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
        app.set_option(&egui::Context::default(), "layout", "continuous").unwrap();
        app.set_option(&egui::Context::default(), "zoom", "75").unwrap();
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
    assert!(h.state().views[0].session.comments.primary().is_some_and(|(page, _)| page == 1), "its last part is selected, as one markup is");
    h.undo();
    h.run_steps(3);
    assert_eq!(marks(&h), [], "one undo removes both");
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
    h.state_mut().set_full_screen(true);
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
    assert_eq!(h.state().views[0].current(), 2, "the link's page, and no turn after it");
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
    h.state_mut().set_full_screen(true);
    crate::support::wait_until(&mut h, "the page in full screen", |app| app.views[0].glyph_screen_pos(0, 5).is_some() && !app.render_pending());
    let at = h.state().views[0].glyph_screen_pos(0, 5).expect("text layer");
    h.hover_at(at);
    h.run_steps(1);

    // A double-click selects "quick" and stays on page 1, also once the double-click time is over.
    click(&mut h, at);
    click(&mut h, at);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
    h.run_steps(30);
    assert_eq!(h.state().views[0].current(), 0, "the double-click didn't turn the page");

    // A single click turns it once the double-click time is over.
    // Entering full screen again clears the selection.
    h.state_mut().set_full_screen(false);
    h.state_mut().set_full_screen(true);
    h.run_steps(30);
    click(&mut h, at);
    h.run_steps(30);
    assert_eq!(h.state().views[0].current(), 1, "a click goes forward");

    // A click right before leaving full screen doesn't turn the page after it.
    h.state_mut().views[0].go_to_page(0);
    h.run_steps(30);
    click(&mut h, at);
    h.state_mut().set_full_screen(false);
    h.run_steps(30);
    assert_eq!(h.state().views[0].current(), 0, "no page turn after leaving");
}

/// L-318 (#5302): Esc closes one thing at a time. In full screen with a text selection the first Esc clears the
/// selection and full screen stays; the second leaves it.
#[test]
fn full_screen_escape_clears_the_text_selection_before_it_leaves() {
    let (mut h, _) = harness_with(two_lines(), |_| {});
    h.state_mut().set_full_screen(true);
    crate::support::wait_until(&mut h, "the page in full screen", |app| app.views[0].glyph_screen_pos(0, 5).is_some() && !app.render_pending());
    let at = h.state().views[0].glyph_screen_pos(0, 5).expect("text layer");
    h.hover_at(at);
    h.run_steps(1);
    click(&mut h, at);
    click(&mut h, at);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
    h.run_steps(30);

    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().views[0].selected_text().is_none(), "the first Esc clears the selection");
    assert!(h.state().full_screen(), "and full screen stays");

    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!h.state().full_screen(), "the second Esc leaves full screen");
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

/// Review of #5017: a flick back up onto the page before, let go on the frame after the drag
/// starts, selects up to where it was let go. The earlier page is drawn first, so the release
/// frame is the only one that sees the pointer over it with the selection started.
#[test]
fn a_flick_back_onto_the_page_before_selects_to_the_release() {
    let mut h = three_pages();
    let (from, to) = (glyph_on(&h, 1, 10), glyph_on(&h, 0, 7));
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
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
        assert!(h.execute(command));
        h.run_steps(2);
        let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
        drag(&mut h, from, to);
        h.run_steps(3);
        assert_eq!(marks(&h), both(kind), "{command}: dragged with the tool");
        h.undo();
        h.run_steps(3);
        assert_eq!(marks(&h), [], "{command}: one undo step");
        // Selecting first, then picking the tool.
        // (The tool opened the Comments panel above, which moved the pages.)
        let mut h = three_pages();
        let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
        drag(&mut h, from, to);
        assert!(h.state().views[0].selected_text().is_some());
        assert!(h.execute(command));
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
    assert!(h.execute("comment.highlight"));
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
    assert!(h.execute("comment.highlight"));
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let on_page_2 = doc.info.annotations.iter().find(|a| a.page == 1 && a.subtype == "Highlight").map(|a| a.index).expect("a highlight on page 2");
    assert_eq!(app.views[0].session.comments.primary(), Some((1, on_page_2)), "the part where the selection ends is selected");
    assert!(
        app.views[0]
            .session
            .comments
            .panel
            .editing
            .as_ref()
            .is_some_and(|(thread, _, _)| *thread == pdfcraft_ui_egui::comments::thread_id(1, on_page_2)),
        "its note is open"
    );
}

/// Review of #5017: dragged back up, from page 2 to page 1, the selection ends on page 1: the
/// highlight's part there is the one selected, with its note open.
#[test]
fn a_highlight_dragged_back_across_pages_selects_its_part_where_the_drag_ended() {
    let mut h = three_pages();
    let (from, to) = (glyph_on(&h, 0, 7), glyph_on(&h, 1, 10));
    drag(&mut h, to, from);
    h.run_steps(3);
    assert!(h.execute("comment.highlight"));
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let on_page_1 = doc.info.annotations.iter().find(|a| a.page == 0 && a.subtype == "Highlight").map(|a| a.index).expect("a highlight on page 1");
    assert_eq!(app.views[0].session.comments.primary(), Some((0, on_page_1)), "the part where the drag ended is selected");
    assert!(
        app.views[0]
            .session
            .comments
            .panel
            .editing
            .as_ref()
            .is_some_and(|(thread, _, _)| *thread == pdfcraft_ui_egui::comments::thread_id(0, on_page_1)),
        "its note is open"
    );
}

/// Review of #5017: only markup on a text selection across pages is followed up as one markup.
/// Redact ▸ Mark Page Range, also one comment per page in one batch, selects nothing and leaves
/// the comment editor alone, as before.
#[test]
fn a_page_range_redaction_is_not_followed_up_as_a_selection() {
    let mut h = three_pages();
    let ctx_h = h.ctx.clone();
    h.state_mut().redact_pages(&ctx_h, &[0, 1, 2]);
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.redaction_marks(), 3, "a mark on every page");
    assert_eq!(app.views[0].session.comments.primary(), None, "no mark selected");
    assert!(app.views[0].session.comments.panel.editing.is_none(), "no editor opened");
}
