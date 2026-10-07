use super::catalog::{parse_entries, placeholders};
use super::*;

fn japanese() -> Language {
    Language::parse("ja").unwrap()
}

#[test]
fn translations_are_unique_and_preserve_unknown_text() {
    let entries = parse_entries(japanese().0.source, 1).unwrap();
    for entry in &entries {
        assert_eq!(Language::EN.tr(&entry.source), entry.source);
        assert_eq!(japanese().tr(&entry.source), entry.translation);
    }
    assert_eq!(japanese().tr("File"), "ファイル");
    assert_eq!(japanese().tr("日本語の文書.pdf"), "日本語の文書.pdf");
    assert_eq!(Language::parse("xx"), None);
}

#[test]
fn language_persists_and_invalid_input_keeps_current_language() {
    let mut app = crate::PrintCraftApp::default();
    app.set_option("language", "ja").unwrap();
    assert_eq!(app.language, japanese());
    assert!(app.set_option("language", "xx").unwrap_err().contains(&Language::codes()));
    assert_eq!(app.language, japanese());
    let mut restored = crate::PrintCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, japanese());
    restored.restore(r#"{"language":"xx"}"#);
    assert_eq!(restored.language, japanese());
    let mut legacy = crate::PrintCraftApp::default();
    legacy.restore("{}");
    assert_eq!(legacy.language, Language::EN);
}

#[test]
fn registry_and_serialized_codes_agree() {
    let mut codes = std::collections::BTreeSet::new();
    for language in Language::all() {
        assert!(codes.insert(language.code()));
        assert!(!language.name().is_empty());
        assert_eq!(language.code(), language.code().to_ascii_lowercase());
        let json = serde_json::to_string(&language).unwrap();
        assert_eq!(json, format!("\"{}\"", language.code()));
        assert_eq!(serde_json::from_str::<Language>(&json).unwrap(), language);
    }
    assert!(codes.contains(Language::EN.code()));
    for invalid in [r#""xx""#, "null", "[]", "4"] {
        assert!(serde_json::from_str::<Language>(invalid).is_err());
    }
}

#[test]
fn bundled_catalogs_are_consistent() {
    for language in Language::all() {
        let entries = parse_entries(language.0.source, language.0.plural.forms()).unwrap();
        for entry in entries.iter().filter(|entry| entry.context == "@id") {
            let command = printcraft_engine::commands::command(&entry.source).expect("catalog command exists");
            assert_eq!(placeholders(&entry.translation), placeholders(command.label), "{}: {}", language.code(), entry.source);
            assert_eq!(entry.translation.ends_with('…'), command.label.ends_with('…'));
        }
    }
}

#[test]
fn contextual_and_id_lookups_have_english_fallbacks() {
    static TEST_LANGUAGE: LanguageInfo = LanguageInfo {
        code: "test",
        name: "Test catalog",
        source: "\tLight\tPlain light\nweight\tLight\tThin\n@id\tfile.open\tOpen dialog…\n@plural\t{n} page|{n} pages\tOne page: {n}|Many pages: {n}\n",
        plural: PluralRule::OneOther,
        catalog: OnceLock::new(),
    };
    let language = Language(&TEST_LANGUAGE);
    assert_eq!(language.tr_ctx("weight", "Light"), "Thin");
    assert_eq!(language.tr_ctx("theme", "Light"), "Plain light");
    assert_eq!(language.tr_ctx("theme", "Unknown"), "Unknown");
    assert_eq!(language.tr_id("file.open", "Reworded English…"), "Open dialog…");
    assert_eq!(language.tr_id("unknown", "Light"), "Plain light");
    assert_eq!(language.tr_id("unknown", "Unknown"), "Unknown");
    assert_eq!(japanese().tr_id("file.open", "Open…"), "開く…");
    assert_eq!(language.trn(1, "{n} page", "{n} pages"), "One page: 1");
    assert_eq!(language.trn(0, "{n} page", "{n} pages"), "Many pages: 0");
}

#[test]
fn formatting_reorders_parameters_without_reinterpreting_user_text() {
    let args = [("file", "日本語-{n}.pdf"), ("n", "2")];
    assert_eq!(fmt("{n}: {file}; {unknown}", &args), "2: 日本語-{n}.pdf; {unknown}");
    assert_eq!(fmt("{file} / {file}", &args), "日本語-{n}.pdf / 日本語-{n}.pdf");
    assert_eq!(fmt("text {unfinished", &args), "text {unfinished");
    assert_eq!(fmt("unchanged", &[]), "unchanged");
}

#[test]
fn plural_messages_use_language_rules_and_fall_back_to_english() {
    static TEST_LANGUAGE: LanguageInfo = LanguageInfo {
        code: "test",
        name: "Test catalog",
        source: "@plural\t{n} page|{n} pages\tPages: {n}\n",
        plural: PluralRule::Single,
        catalog: OnceLock::new(),
    };
    let language = Language(&TEST_LANGUAGE);
    for n in [0, 1, 2, u64::MAX] {
        assert_eq!(language.trn(n, "{n} page", "{n} pages"), format!("Pages: {n}"));
    }
    assert_eq!(Language::EN.trn(1, "{n} page", "{n} pages"), "1 page");
    assert_eq!(Language::EN.trn(2, "{n} page", "{n} pages"), "2 pages");
    assert_eq!(japanese().trn(2, "{n} page", "{n} pages"), "2 pages");
    assert_eq!(PluralRule::OneOther.select(0), 1);
    assert_eq!(PluralRule::OneOther.select(1), 0);
}

#[test]
fn malformed_catalogs_produce_errors_and_runtime_falls_back() {
    for (text, forms, error) in [
        ("\tFile", 1, "context<TAB>"),
        ("\tFile\t", 1, "nonempty"),
        ("\tFile\tFiles\n\tFile\tOther\n", 1, "duplicate"),
        ("\tFile\\q\tFiles", 1, "unknown escape"),
        ("\tFile\tFiles\\", 1, "trailing backslash"),
        ("\tOpen…\tOpen", 1, "ellipsis"),
        ("\tOpen {file}\tOpen {name}", 1, "placeholders"),
        ("@plural\tpage\tPages", 1, "one|other"),
        ("@plural\t{n} page|{n} pages\t{n} pages", 2, "2 nonempty"),
        ("@plural\t{n} page|{n} pages\tPages", 1, "placeholders"),
        ("@unknown\tFile\tFiles", 1, "reserved context"),
    ] {
        let message = Catalog::parse(text, forms).unwrap_err();
        assert!(message.contains("line 1") || message.contains("line 2"));
        assert!(message.contains(error), "{message}");
    }
    static BROKEN: LanguageInfo =
        LanguageInfo { code: "broken", name: "Broken test catalog", source: "malformed", plural: PluralRule::Single, catalog: OnceLock::new() };
    assert_eq!(Language(&BROKEN).tr("File"), "File");
    assert!(Catalog::parse("", 0).is_err());
}

#[test]
fn catalogs_handle_escapes_crlf_and_distinct_contexts() {
    let catalog =
        Catalog::parse("# comment\r\n\r\n\tLine\\nTab\\tPath\\\\\tOther\\nTab\\tPath\\\\\r\nweight\tLight\tThin\r\ntheme\tLight\tPale\r\n", 1)
            .unwrap();
    assert_eq!(catalog.plain("Line\nTab\tPath\\"), Some("Other\nTab\tPath\\"));
    assert_eq!(catalog.contextual("weight", "Light"), Some("Thin"));
    assert_eq!(catalog.contextual("theme", "Light"), Some("Pale"));
}
