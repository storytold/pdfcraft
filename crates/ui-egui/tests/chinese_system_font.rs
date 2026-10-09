//! Windows UI regression: the full Chinese catalog and file names, without font assets.

use std::collections::BTreeSet;

use egui::epaint::text::{Fonts, TextOptions};
use egui::{FontDefinitions, FontId};
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

#[test]
fn chinese_catalog_and_filenames_keep_arabic_fallback() {
    assert!(!theme::font_definitions_for(true).font_data.contains_key(theme::SYSTEM_CHINESE_FALLBACK));
    let defs = theme::installed_font_definitions(true);
    if !defs.font_data.contains_key(theme::SYSTEM_CHINESE_FALLBACK) {
        assert!(defs.families.values().all(|stack| !stack.iter().any(|name| name == theme::SYSTEM_CHINESE_FALLBACK)));
        if cfg!(windows) && std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_none_or(|value| value != "0") {
            assert!(std::env::var_os("PDFCRAFT_TEST_REQUIRE_CJK").is_none_or(|value| value != "1"), "required installed Chinese font was not loaded");
        }
        eprintln!("skipping Chinese glyph coverage: no installed Windows Chinese face (or PDFCRAFT_SYSTEM_FONTS=0)");
        return;
    }

    let catalog = include_str!("../src/i18n/zh-hans.tsv");
    assert!(
        catalog.contains("\tJavaScript is turned off (Preferences ▸ JavaScript).\tJavaScript 已关闭（首选项 › JavaScript）。"),
        "English lookup key must remain unchanged"
    );
    let mut chars = BTreeSet::new();
    for line in catalog.lines().filter(|line| !line.starts_with('#')) {
        if let Some(translation) = line.split('\t').nth(2) {
            chars.extend(translation.chars().filter(|c| !c.is_ascii() && !c.is_whitespace()));
        }
    }
    let catalog: String = chars.iter().collect();
    for prefer_hans in [true, false] {
        let defs = theme::installed_font_definitions(prefer_hans);
        let embedded = theme::font_definitions_for(prefer_hans);
        let japanese: Vec<String> = pdfcraft_fonts::ui_japanese_fonts().iter().map(|face| face.name()).collect();
        let chinese: Vec<String> = pdfcraft_fonts::ui_chinese_fonts().iter().map(|face| face.name()).collect();
        for (family, stack) in &defs.families {
            let cjk = stack.iter().position(|name| name == theme::SYSTEM_CHINESE_FALLBACK).unwrap();
            assert_eq!(stack.iter().filter(|name| *name == theme::SYSTEM_CHINESE_FALLBACK).count(), 1);
            if prefer_hans {
                assert!(stack.iter().enumerate().filter(|(_, name)| japanese.contains(name)).all(|(i, _)| cjk < i));
                assert!(stack.iter().enumerate().filter(|(_, name)| chinese.contains(name)).all(|(i, _)| i < cjk));
            } else {
                assert_eq!(cjk, embedded.families[family].len(), "non-Chinese UI keeps all embedded faces first");
            }
            if defs.font_data.contains_key(theme::SYSTEM_FALLBACK) {
                assert_eq!(stack.last().map(String::as_str), Some(theme::SYSTEM_FALLBACK));
                assert_eq!(stack.iter().filter(|name| *name == theme::SYSTEM_FALLBACK).count(), 1);
            }
        }
        if defs.font_data.contains_key(theme::SYSTEM_FALLBACK) {
            check_glyphs(defs.clone(), "واحد اثنين אבג");
        }
        check_glyphs(defs, "工程估价_欢迎编辑_报告.pdf");
    }
    check_glyphs(defs, &catalog);
    println!(
        "verified {} unique non-ASCII translated glyphs in four UI families; Chinese file names in both font orders; Arabic fallback retained",
        chars.len()
    );
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
