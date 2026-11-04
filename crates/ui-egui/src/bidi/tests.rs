use super::*;

#[test]
fn text_without_rtl_is_borrowed_unchanged() {
    for s in ["", "report.pdf", "日本語の文字.pdf", "a (b) [c]", "caf\u{00E9} \u{200E}x"] {
        assert!(matches!(visual(s), Cow::Borrowed(v) if v == s), "{s:?}");
    }
}

#[test]
fn arabic_words_swap_and_the_extension_stays_last() {
    // The reported name: "one two.pdf" in Arabic.
    assert_eq!(visual("واحد اثنين.pdf"), "اثنين واحد.pdf");
    assert_eq!(visual("ملف.pdf"), "ملف.pdf");
    assert_eq!(visual("تقرير نهائي جدا.pdf"), "جدا نهائي تقرير.pdf");
}

#[test]
fn latin_and_numbers_keep_their_own_order() {
    assert_eq!(visual("report واحد اثنين final.pdf"), "report اثنين واحد final.pdf");
    // A number inside right-to-left text reads left to right, to the left of what precedes it.
    assert_eq!(visual("ملف 2024.pdf"), "2024 ملف.pdf");
    assert_eq!(visual(r"C:\Users\me\واحد اثنين\a.pdf"), r"C:\Users\me\اثنين واحد\a.pdf");
}

#[test]
fn hebrew_and_brackets() {
    assert_eq!(visual("שלום עולם.pdf"), "עולם שלום.pdf");
    // Brackets between right-to-left words flip with the direction.
    assert_eq!(visual("واحد (اثنين) ثلاثة.pdf"), "ثلاثة (اثنين) واحد.pdf");
}

#[test]
fn direction_controls_are_not_drawn() {
    assert_eq!(visual("\u{202B}واحد اثنين\u{202C}.pdf"), "اثنين واحد.pdf");
    assert_eq!(visual("\u{200F}واحد\u{061C}"), "واحد");
}

#[test]
fn rtl_labels_read_right_to_left() {
    // No right-to-left characters: unchanged.
    for s in ["", "Save as…", "{n} pages", "日本語"] {
        assert_eq!(display_rtl(s), s);
    }
    assert_eq!(display_rtl("حفظ"), "حفظ");
    // "Save as…": the second word and the ellipsis end up on the left.
    assert_eq!(display_rtl("حفظ باسم…"), "…باسم حفظ");
    // Latin words and numbers keep their own order inside the line.
    assert_eq!(display_rtl("إنشاء PDF من ملف"), "ملف من PDF إنشاء");
    assert_eq!(display_rtl("تصدير إلى Microsoft Word الآن"), "الآن Microsoft Word إلى تصدير");
    assert_eq!(display_rtl("صالح لمدة 5 سنوات"), "سنوات 5 لمدة صالح");
    // Keyboard shortcuts are not reversed.
    assert_eq!(display_rtl("طباعة (⌘P)"), "(⌘P) طباعة");
    assert_eq!(display_rtl("تدوير العرض (⇧⌘+)"), "(⇧⌘+) العرض تدوير");
    assert_eq!(display_rtl("التمرير مع ⌘ أيضًا"), "أيضًا ⌘ مع التمرير");
    assert_eq!(display_rtl("نص (عادي)"), "(عادي) نص");
}

#[test]
fn rtl_labels_keep_placeholders_whole() {
    // Two placeholders around a colon stay together, left to right: a file name and its error.
    assert_eq!(display_rtl("تعذّر فتح {name}: {e}"), "{name}: {e} فتح تعذّر");
    assert_eq!(display_rtl("فشل {label}، السبب {e}"), "{e} السبب ،{label} فشل");
    assert_eq!(display_rtl("{n} صفحات"), "صفحات {n}");
    assert_eq!(display_rtl("الصفحة {p} من {n}"), "{n} من {p} الصفحة");
    // Adjacent placeholders keep their order, as numbers around a slash would.
    assert_eq!(display_rtl("جارٍ البحث… {s}/{p}"), "{s}/{p} …البحث جارٍ");
    // Braces that are not a placeholder are ordinary punctuation.
    assert_eq!(display_rtl("حرف } واحد"), "واحد { حرف");
    let many = format!("ا {}", "{a} ".repeat(200));
    assert_eq!(display_rtl(&many).matches("{a}").count(), 200);
}

#[test]
fn rtl_labels_keep_their_edges() {
    // Edge spaces and separator dots stay where the code concatenates.
    assert_eq!(display_rtl(" (صفحة {p})"), " ({p} صفحة)");
    assert_eq!(display_rtl(" · محمي بكلمة مرور"), " · مرور بكلمة محمي");
    assert_eq!(display_rtl("سلسلة المفاتيح  ·  "), "المفاتيح سلسلة  ·  ");
    // A caption's colon stays next to its widget; a sentence's colon ends the sentence.
    assert_eq!(display_rtl("الاسم:"), "الاسم:");
    assert_eq!(display_rtl("صادرة عن: "), "عن صادرة: ");
    assert_eq!(display_rtl("اختر المعرّف الرقمي الذي تريد استخدامه:"), ":استخدامه تريد الذي الرقمي المعرّف اختر");
    // Each line is its own paragraph.
    assert_eq!(display_rtl("تعذّر عرض الصفحة.\n{e}"), ".الصفحة عرض تعذّر\n{e}");
}

#[test]
fn rtl_labels_never_panic_on_odd_input() {
    let long = "ا {n} ".repeat(20_000);
    for s in [
        "ا{",
        "ا}",
        "ا{}",
        "ا{{{",
        "}ا{",
        "ا\u{E000}{n}",
        "⌘",
        "ا ⌘",
        "⌘ا⌘{",
        "ا (⇧⌘",
        "\u{064B}:",
        ":",
        "ا:",
        " : ا : ",
        "ا\n\n\nب",
        "\u{202E}ا{n}",
        long.as_str(),
    ] {
        let v = display_rtl(s);
        assert!(v.chars().count() <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
    }
}

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
    ctx.set_fonts(crate::theme::font_definitions());
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

/// A file name keeps its extension at the visual end even when the first strong word is
/// Arabic: with an explicit `LeftToRight` base, `.pdf` is right of every Arabic glyph.
#[test]
fn file_names_are_laid_out_left_to_right() {
    if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
        return;
    }
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions());
    let mut positions: Vec<(char, f32)> = Vec::new();
    ctx.run_ui(egui::RawInput::default(), |ui| {
        let galley = layout_ltr(ui, "واحد اثنين.pdf", egui::FontId::proportional(13.0), egui::Color32::WHITE);
        positions = galley.rows.iter().flat_map(|row| row.glyphs.iter().map(|g| (g.chr, g.pos.x))).collect();
    })
    .drop_without_applying_deltas();
    let rightmost_arabic = positions.iter().filter(|(c, _)| is_rtl_script(*c)).map(|(_, x)| *x).fold(f32::MIN, f32::max);
    let leftmost_extension = positions.iter().filter(|(c, _)| matches!(*c, '.' | 'p' | 'd' | 'f')).map(|(_, x)| *x).fold(f32::MAX, f32::min);
    assert!(rightmost_arabic < leftmost_extension, "the extension must stay to the right: {positions:?}");
    let last = positions.iter().filter(|(c, _)| !c.is_whitespace()).max_by(|a, b| a.1.total_cmp(&b.1));
    assert_eq!(last.map(|(c, _)| *c), Some('f'), "the painted line ends in the extension");
}

/// Arabic/Kurdish labels get an explicit right-to-left paragraph, even when their first
/// strong character is Latin. With English the same text stays left to right.
#[test]
fn arabic_ui_labels_use_an_explicit_paragraph_direction() {
    if pdfcraft_fonts::ui_arabic_fonts().is_empty() {
        return;
    }
    let previous = crate::i18n::current();
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::font_definitions());
    let read = |lang: &str| {
        crate::i18n::set_current(crate::i18n::Lang::from_code(lang).expect("registered language"));
        let mut got = None;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = layout_ui(ui, "PDF واحد", egui::FontId::proportional(13.0), egui::Color32::WHITE);
            let positions: Vec<_> = galley.rows.iter().flat_map(|row| row.glyphs.iter().map(|g| (g.chr, g.pos.x))).collect();
            got = Some((galley.job.direction, positions));
        })
        .drop_without_applying_deltas();
        got.expect("one layout")
    };
    let (direction, positions) = read("ar");
    crate::i18n::set_current(previous);
    assert_eq!(direction, egui::epaint::text::TextDirection::RightToLeft);
    let p = positions.iter().find(|(c, _)| *c == 'P').map(|(_, x)| *x).expect("a Latin P");
    let rightmost_arabic = positions.iter().filter(|(c, _)| is_rtl_script(*c)).map(|(_, x)| *x).fold(f32::MIN, f32::max);
    assert!(p > rightmost_arabic, "in an RTL paragraph the first Latin run is at the right: {positions:?}");

    let (direction, positions) = read("en");
    crate::i18n::set_current(previous);
    assert_eq!(direction, egui::epaint::text::TextDirection::Auto);
    let p = positions.iter().find(|(c, _)| *c == 'P').map(|(_, x)| *x).expect("a Latin P");
    let leftmost_arabic = positions.iter().filter(|(c, _)| is_rtl_script(*c)).map(|(_, x)| *x).fold(f32::MAX, f32::min);
    assert!(p < leftmost_arabic, "with English the Latin run stays at the left: {positions:?}");
}

#[test]
fn single_line_with_its_own_direction() {
    assert!(matches!(visual_auto("Ada Lovelace"), Cow::Borrowed("Ada Lovelace")));
    assert_eq!(visual_auto("علی رضایی"), "رضایی علی");
    assert_eq!(visual_auto("علی 2"), "2 علی");
    assert_eq!(visual_auto("Team علی رضایی"), "Team رضایی علی");
}

#[test]
fn odd_input_never_panics() {
    let long = "ا ب ".repeat(20_000);
    let odd = [
        "\u{064B}",
        "\u{064B}\u{064C} \u{0301}",
        "\u{202E}\u{202E}\u{202E}ا",
        "\u{2067}ا\u{2066}b",
        "ا\nب\r\nج\u{2029}د",
        "\u{FEFF}ا\u{0000}ب",
        "ا\u{10FFFF}\u{E000}ب",
        "((((ا]]]]",
        long.as_str(),
    ];
    for s in odd {
        let v = visual(s);
        assert!(v.chars().count() <= s.chars().count(), "{:?}", s.chars().take(12).collect::<String>());
    }
}
