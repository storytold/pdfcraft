//! Keep AppKit's bundle language selection in step with the interface catalogs (#2668, item 1).

use std::collections::BTreeSet;

use pdfcraft_ui_egui::i18n::LANGUAGES;

#[test]
fn macos_bundle_declares_ui_languages_and_english_fallback() {
    // Inspect the shipped template, excluding comments: a commented-out declaration cannot
    // satisfy the check. plist syntax itself is also checked by plutil when packaging on macOS.
    let mut rest = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/macos/Info.plist.in"));
    let mut plist = String::new();
    while let Some((before, comment)) = rest.split_once("<!--") {
        plist.push_str(before);
        rest = comment.split_once("-->").expect("closed XML comment").1;
    }
    plist.push_str(rest);

    let array = plist
        .split_once("<key>CFBundleLocalizations</key>")
        .expect("declare manually translated languages so AppKit can localize native panels")
        .1
        .trim_start()
        .strip_prefix("<array>")
        .expect("localizations array")
        .split_once("</array>")
        .expect("closed localizations array")
        .0;
    let declared: Vec<String> = array
        .split("<string>")
        .skip(1)
        .map(|entry| entry.split_once("</string>").expect("localization string").0.trim().to_ascii_lowercase())
        .collect();
    let actual: BTreeSet<_> = declared.iter().map(String::as_str).collect();
    let mut expected: BTreeSet<_> = LANGUAGES.iter().map(|lang| lang.code).collect();
    // The UI resolves all Portuguese locales to pt-br. AppKit needs the base language too
    // to avoid selecting English for pt-PT when only pt-BR is declared.
    if expected.contains("pt-br") {
        expected.insert("pt");
    }
    assert_eq!(actual.len(), declared.len(), "duplicate bundle language");
    assert_eq!(actual, expected, "AppKit must see the same languages as the interface");

    let fallback = plist
        .split_once("<key>CFBundleDevelopmentRegion</key>")
        .expect("development language")
        .1
        .trim_start()
        .strip_prefix("<string>")
        .expect("development language string")
        .split_once("</string>")
        .expect("closed development language")
        .0;
    assert_eq!(fallback, "en", "unsupported system languages retain English fallback");
}
