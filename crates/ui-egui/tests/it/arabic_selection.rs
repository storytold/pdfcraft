//! Canvas selection must use logical glyph clusters inside Arabic ActualText, rather than
//! treating a complete line as one selectable glyph. These tests copy through the real shell.

use crate::support;

use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use pdfcraft_engine::{AddedText, Edit};
use pdfcraft_render::{PageRenderer, PageText, RenderRequest, RequestKind};
use pdfcraft_ui_egui::PdfCraftApp;

fn added_pdf(text: &str) -> Vec<u8> {
    let mut app = PdfCraftApp::new();
    let bytes = app.session.create_blank(600.0, 300.0, 1).unwrap();
    app.open_bytes("selection.pdf", None, bytes.as_ref().clone()).unwrap();
    let id = app.views[0].id;
    app.session
        .apply(
            id,
            Edit::AddText { page: 0, text: AddedText { text: text.into(), rect: [30.0, 70.0, 570.0, 110.0], size: 24.0, ..Default::default() } },
        )
        .unwrap();
    app.session.save_bytes(id).unwrap().as_ref().clone()
}

fn harness(pdf: Vec<u8>, language: &'static str) -> (Harness<'static, PdfCraftApp>, PageText) {
    let mut renderer = PageRenderer::new(std::sync::Arc::new(pdf.clone()), Default::default());
    let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, ..Default::default() });
    assert!(rendered.error.is_none(), "text extraction failed: {:?}", rendered.error);
    let text = rendered.text.unwrap().as_ref().clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", language).unwrap();
        app.open_bytes("selection.pdf", None, pdf).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    support::wait_until(&mut h, "the Arabic canvas text layer", |app| app.views[0].glyph_screen_pos(0, 0).is_some() && !app.render_pending());
    (h, text)
}

fn click(h: &mut Harness<'static, PdfCraftApp>, at: Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(1);
}

fn assert_copied(h: &mut Harness<'static, PdfCraftApp>, expected: &str) {
    h.event(Event::Copy);
    h.step();
    let copied: Vec<_> = h
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(copied, [expected], "canvas Copy emits only the logical selection");
}

fn select_word(h: &mut Harness<'static, PdfCraftApp>, text: &PageText, word: &str) {
    let range = text.find(word).into_iter().next().expect("word in text layer");
    let at = h.state().views[0].glyph_screen_pos(0, range.start).unwrap();
    h.run_steps(30); // End the previous multi-click sequence before double-clicking another word.
    click(h, at);
    click(h, at);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some(word), "double-click selects only {word:?}");
    assert_copied(h, word);
    assert_eq!(text.text_of(range.clone()), word, "search and the canvas retain the same logical cluster range");
    let selected_width: f32 = text.line_rects(range).iter().map(|r| r[2] - r[0]).sum();
    let line_width: f32 = text.line_rects(0..text.glyphs.len()).iter().map(|r| r[2] - r[0]).sum();
    assert!(selected_width < line_width * 0.6, "a selected word must not highlight the complete line: {selected_width} / {line_width}");
}

#[test]
fn arabic_canvas_double_click_and_copy_select_one_word_in_both_ui_languages() {
    for language in ["en", "ar"] {
        let (mut h, text) = harness(added_pdf("البداية سلام النهاية"), language);
        assert_eq!(text.plain_text(), "البداية سلام النهاية");
        select_word(&mut h, &text, "سلام");
    }
}

#[test]
fn mixed_arabic_canvas_keeps_english_digits_and_substring_selection_precise() {
    let line = "مرحبا English 123 عالم ٤٥٦ النهاية";
    let (mut h, text) = harness(added_pdf(line), "en");
    assert_eq!(text.plain_text(), line);
    for word in ["English", "123", "عالم", "٤٥٦"] {
        select_word(&mut h, &text, word);
    }
    let range = text.find("ngli").into_iter().next().unwrap();
    let from = h.state().views[0].glyph_screen_pos(0, range.start).unwrap();
    let to = h.state().views[0].glyph_screen_pos(0, range.end - 1).unwrap();
    h.run_steps(30);
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for step in 1..=4 {
        h.hover_at(from + (to - from) * (step as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("ngli"));
    assert_copied(&mut h, "ngli");
}

#[test]
fn english_canvas_word_selection_and_copy_remain_precise() {
    let (mut h, text) = harness(added_pdf("The quick brown fox"), "en");
    assert_eq!(text.plain_text(), "The quick brown fox");
    select_word(&mut h, &text, "quick");
}
