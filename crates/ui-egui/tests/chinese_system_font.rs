//! The Chinese interface on a real machine: the full catalogs and Chinese file names, drawn with
//! an installed face rather than a font asset. Runs on every desktop platform — the face comes from
//! [`pdfcraft_fonts::han`], which searches Windows, macOS and Linux.

use std::collections::BTreeSet;

use egui::epaint::text::{Fonts, TextOptions};
use egui::{Color32, FontDefinitions, FontId};
use pdfcraft_ui_egui::theme;

fn families() -> [FontId; 4] {
    [FontId::proportional(13.0), FontId::monospace(13.0), theme::medium(13.0), theme::semibold(17.0)]
}

fn check_glyphs(defs: FontDefinitions, text: &str) {
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    for id in families() {
        let missing: String = text.chars().filter(|c| !fonts.has_glyphs(&id, &c.to_string())).collect();
        assert!(missing.is_empty(), "{id:?} lacks {missing:?}");
    }
}

/// Every non-ASCII, non-space character a catalog writes to the screen.
fn translated_chars(catalog: &str) -> BTreeSet<char> {
    let mut chars = BTreeSet::new();
    for line in catalog.lines().filter(|line| !line.starts_with('#')) {
        if let Some(translation) = line.split('\t').nth(2) {
            chars.extend(translation.chars().filter(|c| !c.is_ascii() && !c.is_whitespace()));
        }
    }
    chars
}

/// True when an installed Chinese face is in play. False when there is none — but a build that
/// ships a Chinese interface is exactly the build that must not pass by skipping, so the release
/// jobs set `PDFCRAFT_TEST_REQUIRE_CJK=1` and the check fails instead of going green.
fn installed_face_or_skip(what: &str) -> bool {
    if theme::installed_font_definitions(true).font_data.contains_key(theme::SYSTEM_CHINESE_FALLBACK) {
        return true;
    }
    assert!(
        std::env::var_os("PDFCRAFT_TEST_REQUIRE_CJK").is_none_or(|value| value != "1"),
        "no installed Chinese face: PDFCRAFT_TEST_REQUIRE_CJK=1 was set, so this build would show {what} broken"
    );
    eprintln!("skipping {what}: no installed Chinese face (or PDFCRAFT_SYSTEM_FONTS=0)");
    false
}

#[test]
fn chinese_catalogs_and_filenames_keep_arabic_fallback() {
    // The installed faces are only ever added on top of the embedded ones.
    assert!(!theme::font_definitions_for(true).font_data.contains_key(theme::SYSTEM_CHINESE_FALLBACK));
    let defs = theme::installed_font_definitions(true);
    if !defs.font_data.contains_key(theme::SYSTEM_CHINESE_FALLBACK) {
        assert!(defs.families.values().all(|stack| !stack.iter().any(|name| name == theme::SYSTEM_CHINESE_FALLBACK)));
        assert!(
            std::env::var_os("PDFCRAFT_TEST_REQUIRE_CJK").is_none_or(|value| value != "1"),
            "no installed Chinese face: PDFCRAFT_TEST_REQUIRE_CJK=1 was set, so this build would show tofu for Chinese text"
        );
        eprintln!("skipping Chinese glyph coverage: no installed Chinese face (or PDFCRAFT_SYSTEM_FONTS=0)");
        return;
    }

    for prefer_hans in [true, false] {
        let defs = theme::installed_font_definitions(prefer_hans);
        let embedded = theme::font_definitions_for(prefer_hans);
        let japanese: Vec<String> = pdfcraft_fonts::ui_japanese_fonts().iter().map(|face| face.name()).collect();
        let chinese: Vec<String> = pdfcraft_fonts::ui_chinese_fonts().iter().map(|face| face.name()).collect();
        for (family, stack) in &defs.families {
            let cjk = stack.iter().position(|name| name == theme::SYSTEM_CHINESE_FALLBACK).expect("checked above");
            assert_eq!(stack.iter().filter(|name| *name == theme::SYSTEM_CHINESE_FALLBACK).count(), 1);
            if prefer_hans {
                // Chinese mode: after any embedded Chinese face, before every Japanese face.
                assert!(stack.iter().enumerate().filter(|(_, name)| japanese.contains(name)).all(|(i, _)| cjk < i), "{family:?}: {stack:?}");
                assert!(stack.iter().enumerate().filter(|(_, name)| chinese.contains(name)).all(|(i, _)| i < cjk), "{family:?}: {stack:?}");
            } else {
                assert_eq!(cjk, embedded.families[family].len(), "non-Chinese UI keeps all embedded faces first");
            }
            if defs.font_data.contains_key(theme::SYSTEM_FALLBACK) {
                assert_eq!(stack.last().map(String::as_str), Some(theme::SYSTEM_FALLBACK));
                assert_eq!(stack.iter().filter(|name| *name == theme::SYSTEM_FALLBACK).count(), 1);
            }
        }
        if defs.font_data.contains_key(theme::SYSTEM_FALLBACK) {
            // The Arabic face stays the last fallback. Hebrew is not asserted: on macOS the
            // installed Arabic face covers Arabic only, and no UI catalog or promise needs Hebrew.
            check_glyphs(defs.clone(), "واحد اثنين");
        }
        check_glyphs(defs, "工程估价_欢迎编辑_报告.pdf");
    }

    let mut total = 0;
    for catalog in [include_str!("../src/i18n/zh-hans.tsv"), include_str!("../src/i18n/zh-hant.tsv")] {
        let chars = translated_chars(catalog);
        total += chars.len();
        let chars: String = chars.iter().collect();
        check_glyphs(theme::installed_font_definitions(true), &chars);
    }
    println!(
        "verified {total} unique non-ASCII translated glyphs (zh-hans + zh-hant) in four UI families; Chinese file names in both font orders; Arabic fallback retained"
    );
}

/// The Chinese face must sit on the line, not above it.
///
/// egui places a glyph `face ascent + (primary row height - face row height) / 2` below the top of
/// its row, so a face whose row is much taller than Inter's — every Chinese face is — was drawn 3 pt
/// above the Latin beside it at 13 pt, and out of the top of the row box, where a fixed-height
/// widget clipped it. `pdfcraft_fonts::ui_y_offset_factor` cancels that; this measures what the user
/// sees, which is where the ink lands.
#[test]
fn chinese_text_shares_the_interface_baseline() {
    if !installed_face_or_skip("Chinese baseline alignment") {
        return;
    }
    let defs = theme::installed_font_definitions(true);
    for size in [13.0, 17.0, 26.0] {
        let mut fonts = Fonts::new(TextOptions::default(), defs.clone());
        fonts.begin_pass(TextOptions::default());
        let mut view = fonts.with_pixels_per_point(2.0);
        let galley = view.layout("中A".to_owned(), FontId::proportional(size), Color32::BLACK, f32::INFINITY);
        let row = galley.rows.first().expect("one row");
        // Where the pixels actually land: the baseline, plus the glyph image's own box on it.
        let ink = |c: char| {
            row.row
                .glyphs
                .iter()
                .filter(|g| g.chr == c)
                .map(|g| (g.pos.y + g.uv_rect.offset.y, g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y))
                .next()
        };
        let Some((han_top, han_bottom)) = ink('中') else { panic!("中 left no ink at {size} pt: the installed face is not drawing") };
        let Some((latin_top, latin_bottom)) = ink('A') else { panic!("A left no ink at {size} pt") };
        let han_center = (han_top + han_bottom) / 2.0;
        let latin_center = (latin_top + latin_bottom) / 2.0;
        assert!(
            (han_center - latin_center).abs() <= 0.06 * size,
            "{size} pt: Chinese ink centred at {han_center:.2}, Latin at {latin_center:.2} — {} of the size apart",
            (han_center - latin_center).abs() / size
        );
        // A fixed-height widget clips at the row box, so the Chinese has to fit inside it.
        assert!(
            han_top >= -0.5 && han_bottom <= row.row.size.y + 0.5,
            "{size} pt: Chinese ink {han_top:.2}..{han_bottom:.2} spills the 0..{:.2} row box",
            row.row.size.y
        );
    }
}

#[test]
fn publication_opt_out_disables_both_runtime_faces() {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_none_or(|value| value != "0") {
        return;
    }
    let defs = theme::installed_font_definitions(true);
    for name in [theme::SYSTEM_FALLBACK, theme::SYSTEM_CHINESE_FALLBACK] {
        assert!(!defs.font_data.contains_key(name));
        assert!(defs.families.values().all(|stack| !stack.iter().any(|entry| entry == name)));
    }
}
