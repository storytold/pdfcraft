use super::*;

/// `wrap` with a width of `n` characters.
fn wrap_at(text: &str, n: usize) -> Vec<Line> {
    wrap(text, |s| s.chars().count() <= n)
}

fn rtl(text: &str) -> Line {
    Line { text: text.to_owned(), rtl: true }
}

fn ltr(text: &str) -> Line {
    Line { text: text.to_owned(), rtl: false }
}

/// Check that a Persian paragraph, including words joined by a zero-width non-joiner, reaches epaint as
/// one logical right-to-left run.
#[test]
fn persian_paragraph_keeps_logical_order_for_epaint() {
    // The line stays in reading order; epaint shapes and reorders it (see `on_screen`).
    assert_eq!(wrap_at("من به مدرسه می‌روم", 80), [rtl("من به مدرسه می‌روم")]);
    // The zero-width non-joiner keeps a word whole, so the shaper gets it as one run.
    assert_eq!(wrap_at("کتاب‌ها", 80), [rtl("کتاب‌ها")]);
}

/// Check that Arabic text wraps into lines in logical reading order rather than visual order.
#[test]
fn arabic_lines_break_in_reading_order() {
    // The first words go on the first line, in logical order for epaint to reorder.
    assert_eq!(wrap_at("واحد اثنين ثلاثة أربعة", 11), [rtl("واحد اثنين"), rtl("ثلاثة أربعة")]);
    assert_eq!(wrap_at("واحد اثنين ثلاثة أربعة", 80), [rtl("واحد اثنين ثلاثة أربعة")]);
}

/// Check that each paragraph takes its base direction from its first strong character, not from leading
/// numbers or neutral text.
#[test]
fn base_direction_comes_from_the_first_strong_character() {
    // Latin first: a left-to-right paragraph with an Arabic phrase inside it.
    assert_eq!(wrap_at("See واحد اثنين here", 80), [ltr("See واحد اثنين here")]);
    // Persian first: right to left, with the Latin words and number in reading order.
    assert_eq!(wrap_at("فایل PDF نسخه 2 است", 80), [rtl("فایل PDF نسخه 2 است")]);
    assert_eq!(wrap_at("سلام Hello World", 80), [rtl("سلام Hello World")]);
    // Leading neutrals and numbers don't decide it.
    assert_eq!(wrap_at("12. واحد اثنين", 80), [rtl("12. واحد اثنين")]);
    // Each paragraph finds its own direction.
    assert_eq!(wrap_at("واحد اثنين\nOK then\n\nשלום עולם", 80), [rtl("واحد اثنين"), ltr("OK then"), ltr(""), rtl("שלום עולם")]);
}

/// Check that right-to-left punctuation and brackets stay in logical order for epaint to mirror when
/// displaying.
#[test]
fn rtl_punctuation_and_brackets() {
    // Logical text: epaint mirrors punctuation and brackets for display.
    assert_eq!(wrap_at("سلام دنیا.", 80), [rtl("سلام دنیا.")]);
    assert_eq!(wrap_at("واحد (اثنين) ثلاثة", 80), [rtl("واحد (اثنين) ثلاثة")]);
    assert_eq!(wrap_at("آیا آماده است؟", 80), [rtl("آیا آماده است؟")]);
}

/// Check that Persian and Arabic-Indic digits stay in logical order inside a right-to-left paragraph.
#[test]
fn persian_digits_go_to_the_shaper_reversed() {
    // Persian and Arabic-Indic digits stay in logical order; epaint's shaper preserves their
    // left-to-right reading inside the right-to-left paragraph.
    assert_eq!(wrap_at("سال ۱۴۰۲ شمسی", 80), [rtl("سال ۱۴۰۲ شمسی")]);
    assert_eq!(wrap_at("صفحه ١٢", 80), [rtl("صفحه ١٢")]);
    assert_eq!(wrap_at("ب۱۲", 80), [rtl("ب۱۲")]);
}

#[test]
fn wrapping_handles_odd_widths_and_input() {
    assert_eq!(wrap_at("", 10), Vec::<Line>::new());
    // A word wider than the line is broken between characters.
    assert_eq!(wrap_at("abcdefghij واحد", 4), [ltr("abcd"), ltr("efgh"), ltr("ij"), ltr("واحد")]);
    // Extra spaces between words don't make empty lines; \r\n is one break.
    assert_eq!(wrap_at("واحد    اثنين\r\nب", 5), [rtl("واحد"), rtl("اثنين"), rtl("ب")]);
    // Nothing fits: one character per line, never an endless loop.
    assert_eq!(wrap("واحد", |_| false).len(), 4);
    let long = "ا ب ۱ (ج) ".repeat(5_000);
    for s in ["\u{202E}ا\u{202C}b", "\u{064B}", "\n\n", "\u{2029}ا", long.as_str()] {
        let lines = wrap_at(s, 7);
        let shown: usize = lines.iter().map(|l| l.text.chars().count()).sum();
        assert!(shown <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
    }
}

/// The characters egui puts on screen for each line of `text`, left to right, zero-width
/// ones left out, with the interface fonts.
fn on_screen(text: &str) -> Vec<String> {
    let ctx = egui::Context::default();
    crate::fonts::ui_fonts(crate::i18n::Lang::EN).install(&ctx);
    let mut shown = Vec::new();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        for line in wrap(text, |_| true) {
            let galley = ui.painter().layout_no_wrap(line.text, egui::FontId::proportional(13.0), egui::Color32::WHITE);
            let atlas = ui.fonts_mut(|fonts| fonts.image());
            let mut visual = String::new();
            for row in &galley.rows {
                // 0.37 keeps glyphs in logical order for selection. Read their actual
                // positions to test visual order instead of assuming vector order.
                let mut glyphs: Vec<_> = row.glyphs.iter().filter(|g| g.advance_width > 0.0).collect();
                glyphs.sort_by(|a, b| a.pos.x.total_cmp(&b.pos.x));
                visual.extend(glyphs.into_iter().map(|g| match g.chr {
                    // `chr` remains logical even when the shaper mirrors the painted
                    // bracket. Read the bitmap's middle-row ink to check what is visible.
                    '(' | ')' => {
                        let [x0, y0] = g.uv_rect.min.map(usize::from);
                        let [x1, y1] = g.uv_rect.max.map(usize::from);
                        let y = usize::midpoint(y0, y1);
                        let ink: Vec<f32> = (x0..x1).map(|x| f32::from(atlas[(x, y)].a())).collect();
                        let total: f32 = ink.iter().sum();
                        assert!(total > 0.0 && !ink.is_empty(), "a bracket must have visible ink");
                        let center = ink.iter().enumerate().map(|(i, alpha)| i as f32 * alpha).sum::<f32>() / total;
                        if center < (ink.len() - 1) as f32 / 2.0 { '(' } else { ')' }
                    }
                    _ => g.chr,
                }));
            }
            shown.push(visual);
        }
    });
    output.drop_without_applying_deltas();
    shown
}

fn rev(s: &str) -> String {
    s.chars().rev().collect()
}

#[test]
fn egui_draws_lines_in_reading_order() {
    // Only with an Arabic-script face (the craft-fonts build input, as in CI); without one
    // egui has no shaper run to check.
    if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
        return;
    }
    assert_eq!(on_screen("من به مدرسه می\u{200C}روم"), [rev("من به مدرسه میروم")]);
    assert_eq!(on_screen("فایل PDF نسخه 2 است"), [format!("{} 2 {} PDF {}", rev("است"), rev("نسخه"), rev("فایل"))]);
    assert_eq!(on_screen("سال ۱۴۰۲ شمسی"), [format!("{} ۱۴۰۲ {}", rev("شمسی"), rev("سال"))]);
    assert_eq!(on_screen("ب۱۲"), ["۱۲ب"]);
    assert_eq!(on_screen("See واحد اثنين here"), [format!("See {} {} here", rev("اثنين"), rev("واحد"))]);
    assert_eq!(on_screen("واحد (اثنين)."), [format!(".({}) {}", rev("اثنين"), rev("واحد"))]);
}
