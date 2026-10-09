//! Issue #295: quick clicks on text widen the selection, as in Acrobat: two clicks select a
//! word, three the line, four all the text on the page.

use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// One 300×200 page with two lines of Helvetica text.
fn two_lines() -> Vec<u8> {
    let content = "BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj 0 -24 Td (jumps over the dog) Tj ET";
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
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

/// The page on screen and the pointer over the "u" of "quick". Frames are 1/60 s: a click takes
/// two frames (press, release), and egui counts a double-click only within 0.3 s.
fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> (Harness<'static, PdfCraftApp>, Pos2) {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("lines.pdf", None, two_lines()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        setup(&mut app);
        app
    });
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
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
    }
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let marks: Vec<(&str, usize)> = doc.info.annotations.iter().map(|a| (a.subtype.as_str(), a.quads.len())).collect();
    assert_eq!(marks, [("Highlight", 1)], "one highlight, on the double-clicked word");
    assert!(app.views[0].selected_text().is_none());
}
