use super::*;

const JA: fn() -> Lang = || Lang::from_code("ja").expect("ja registered");

/// Skips a test that needs the embedded catalogs when none were embedded (`CRAFT_I18N_DIR`
/// unset): the interface is then English only.
macro_rules! needs_catalogs {
    () => {
        if !needs_catalogs() {
            return;
        }
    };
}

/// The rows of `text` (catalog format) as (context, English source, translation), escapes
/// undone; comments, blank and malformed lines left out.
fn rows(text: &str) -> Vec<(String, String, String)> {
    let unescape = |s: &str| s.replace("\\\\", "\u{0}").replace("\\n", "\n").replace("\\t", "\t").replace('\u{0}', "\\");
    text.lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut c = l.split('\t');
            let (Some(context), Some(source), Some(translation), None) = (c.next(), c.next(), c.next(), c.next()) else { return None };
            Some((unescape(context), unescape(source), unescape(translation)))
        })
        .collect()
}

/// The embedded PdfCraft catalog of `code` (not the shared one), `""` without one.
fn app_catalog(code: &str) -> &'static str {
    CRAFT_I18N.iter().find(|(c, ..)| *c == code).map_or("", |(_, _, app)| *app)
}

#[test]
fn the_macos_bundle_declares_every_interface_language() {
    // AppKit translates its own dialogs (open and save panels) only into the languages the app
    // bundle declares, so `CFBundleLocalizations` must list every entry of `LANGUAGES`.
    let plist = include_str!("../../../../packaging/macos/Info.plist.in");
    let block = plist.split("<key>CFBundleLocalizations</key>").nth(1).expect("the template declares CFBundleLocalizations");
    let block = block.split("</array>").next().expect("the declaration is an array");
    let declared: Vec<String> = block.split("<string>").skip(1).filter_map(|s| s.split("</string>").next()).map(str::to_lowercase).collect();
    let registered: Vec<String> = LANGUAGES.iter().map(|l| l.code.to_lowercase()).collect();
    assert_eq!(declared, registered, "packaging/macos/Info.plist.in and LANGUAGES list different languages");
}

#[test]
fn keyboard_key_names_have_a_separate_context_and_english_fallback() {
    let keys = fixture_lang("x-keys", "key\tDelete\tEntf\n\tDelete\tLöschen\n\tOpen\tÖffnen\n");
    let previous = current();
    set_current(keys);
    assert_eq!(key_name("Delete"), "Entf");
    assert_eq!(t("Delete"), "Löschen");
    assert_eq!(key_name("Open"), "Open", "a missing key name must not borrow a command translation");
    set_current(Lang::EN);
    assert_eq!(key_name("Delete"), "Delete");
    set_current(previous);
}

#[test]
fn shortcuts_are_written_with_the_key_names_of_the_current_language() {
    use pdfcraft_engine::commands::Shortcut;
    let chord = Shortcut::cmd_shift("S");
    let german = fixture_lang("x-chords", "key\tCtrl\tStrg\nkey\tShift\tUmschalt\nkey\tDelete\tEntf\n");
    let previous = current();
    set_current(german);
    assert_eq!(crate::commands::shortcut_text(chord, false), "Strg+Umschalt+S");
    assert_eq!(crate::commands::shortcut_text(Shortcut::cmd("Delete"), false), "Strg+Entf");
    assert_eq!(crate::commands::shortcut_text(chord, true), "⇧⌘S", "macOS keeps its symbols");
    set_current(Lang::EN);
    assert_eq!(crate::commands::shortcut_text(chord, false), "Ctrl+Shift+S");
    set_current(previous);
}

/// Regression test: a language picker uses its native name only when a face can draw it, otherwise its
/// English name.
#[test]
fn picker_names_fall_back_to_english_when_their_script_has_no_face() {
    assert_eq!(Lang::EN.picker_name(|_| false), "English");
    needs_catalogs!();
    let hi = Lang::from_code("hi").expect("hi registered");
    assert_eq!(hi.picker_name(|_| true), "हिन्दी", "the native name is kept when it can be drawn");
    assert_eq!(hi.picker_name(|_| false), "Hindi", "no Devanagari face: an English name, not an unrenderable label");
    for language in Lang::all() {
        assert!(!language.english_name().is_empty(), "{}", language.code());
        assert_ne!(language.english_name(), language.code(), "{} needs an English picker fallback", language.code());
    }
}

#[test]
fn tags_map_to_languages() {
    assert_eq!(lang_from_tag("en_US.UTF-8"), Some(Lang::EN));
    assert_eq!(lang_from_tag("C"), Some(Lang::EN));
    assert_eq!(lang_from_tag("POSIX"), Some(Lang::EN));
    assert_eq!(lang_from_tag(""), None);
    assert_eq!(lang_from_tag("_"), None);
    needs_catalogs!();
    assert_eq!(lang_from_tag("ja_JP.UTF-8"), Some(JA()));
    assert_eq!(lang_from_tag("ja-JP"), Some(JA()));
    assert_eq!(lang_from_tag("fr_FR"), Lang::from_code("fr"));
    assert_eq!(lang_from_tag("fr_CA.UTF-8"), Lang::from_code("fr"));
    assert_eq!(lang_from_tag("fr-BE"), Lang::from_code("fr"));
    assert_eq!(lang_from_tag("zh-TW"), Lang::from_code("zh-hant"));
    assert_eq!(lang_from_tag("zh_CN.UTF-8"), Lang::from_code("zh-hans"));
    assert_eq!(lang_from_tag("zh"), Lang::from_code("zh-hans"));
    assert_eq!(lang_from_tag("zh_HK.UTF-8"), Lang::from_code("zh-hant"));
    assert_eq!(lang_from_tag("zh-Hant-MO"), Lang::from_code("zh-hant"));
    assert_eq!(lang_from_tag("lo_LA.UTF-8"), Lang::from_code("lo"));
    assert_eq!(lang_from_tag("lo-LA"), Lang::from_code("lo"));
    assert_eq!(lang_from_tag("lo"), Lang::from_code("lo"));
}

#[test]
fn candidates_walk_from_specific_to_general() {
    assert_eq!(candidates("pt_BR.UTF-8"), ["pt-br", "pt"]);
    assert_eq!(candidates("pt_PT.UTF-8"), ["pt-pt", "pt-br", "pt"]);
    assert_eq!(candidates("pt"), ["pt-br", "pt"]);
    assert_eq!(candidates("zh_TW"), ["zh-tw", "zh-hant", "zh"]);
    assert_eq!(candidates("zh-CN"), ["zh-cn", "zh-hans", "zh"]);
    assert_eq!(candidates("zh-Hant-HK"), ["zh-hant-hk", "zh-hant", "zh"]);
    assert_eq!(candidates("lo_LA.UTF-8"), ["lo-la", "lo"]);
    assert_eq!(candidates("nb_NO.UTF-8"), ["nb-no", "nb"]);
    assert_eq!(candidates("no_NO.UTF-8"), ["no-no", "no", "nb"]);
    assert_eq!(candidates("nn-NO"), ["nn-no", "nn", "nb"]);
}

/// The locale variables, the first detection step: `LC_ALL` over `LC_MESSAGES` over `LANG`, an
/// unsupported value passed over, and PdfCraft's own mappings (`no`/`nn` → Bokmål, Portuguese →
/// Brazilian, Chinese by region), which the crate's matching doesn't all make.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn locale_variables_pick_the_language_with_pdfcraft_mappings() {
    let env = |vars: &'static [(&'static str, &'static str)]| move |name: &str| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
    assert_eq!(env_lang(env(&[])), None);
    assert_eq!(env_lang(env(&[("LANG", "en_US.UTF-8")])), Some(Lang::EN));
    needs_catalogs!();
    let nb = Lang::from_code("nb");
    assert_eq!(env_lang(env(&[("LC_MESSAGES", "nn_NO.UTF-8"), ("LANG", "en_US.UTF-8")])), nb);
    assert_eq!(env_lang(env(&[("LANG", "no_NO.UTF-8")])), nb);
    assert_eq!(env_lang(env(&[("LC_ALL", "pt_PT.UTF-8"), ("LANG", "ja_JP.UTF-8")])), Lang::from_code("pt-br"));
    assert_eq!(env_lang(env(&[("LC_ALL", "xx_YY.UTF-8"), ("LC_MESSAGES", "zh_HK.UTF-8")])), Lang::from_code("zh-hant"));
    if cfg!(not(target_os = "windows")) {
        // Outside Windows a neutral locale means English and ends the search.
        assert_eq!(env_lang(env(&[("LC_ALL", "C"), ("LANG", "ja_JP.UTF-8")])), Some(Lang::EN));
    }
}

/// The macOS preferred-languages list (`defaults read -g AppleLanguages`), the second step, with
/// the mappings it relies on: Chinese by region, other Portuguese to Brazilian, Norwegian.
#[test]
fn macos_language_list_is_parsed() {
    assert_eq!(first_supported("("), None);
    needs_catalogs!();
    assert_eq!(first_supported("(\n    \"ja-JP\",\n    \"en-US\"\n)\n"), Some(JA()));
    assert_eq!(first_supported("(\n    \"fr-FR\",\n    \"en-US\"\n)\n"), Lang::from_code("fr"));
    assert_eq!(first_supported("(\n    \"xx-YY\",\n    \"zh-HK\"\n)\n"), Lang::from_code("zh-hant"));
    assert_eq!(first_supported("(\n    \"pt-PT\",\n    \"en-US\"\n)\n"), Lang::from_code("pt-br"));
    assert_eq!(first_supported("(\n    \"nn-NO\",\n    \"en-US\"\n)\n"), Lang::from_code("nb"));
}

/// #2954: the web build's Auto language follows the browser's preferred languages in order,
/// skipping ones without a catalog, and is English only when none is supported.
#[test]
fn browser_language_list_is_followed_in_order() {
    assert_eq!(first_supported_tag(["", "xx"]), None);
    assert_eq!(first_supported_tag(Vec::<String>::new()), None);
    needs_catalogs!();
    assert_eq!(first_supported_tag(["it-IT", "en-US"]), Lang::from_code("it"));
    assert_eq!(first_supported_tag(["xx-YY", "ja", "fr-FR"]), Some(JA()));
    assert_eq!(first_supported_tag(["zh-TW"]), Lang::from_code("zh-hant"));
    assert_eq!(first_supported_tag(["en-GB", "fr-FR"]), Some(Lang::EN));
    assert_eq!(first_supported_tag([" pt-PT "]), Lang::from_code("pt-br"));
    let long = std::iter::repeat_n("xx", 10_000).chain(["fr"]);
    assert_eq!(first_supported_tag(long), None, "only the first tags are read");
}

/// The Windows display languages (`sys_locale`), the last step: one tag per line, with the same
/// mappings.
#[test]
fn windows_display_language_resolves_chinese_scripts() {
    assert_eq!(first_supported("\r\n"), None);
    needs_catalogs!();
    for tag in ["zh-CN", "zh-SG", "zh-Hans", "zh-Hans-CN"] {
        assert_eq!(first_supported(&format!("{tag}\r\n")), Lang::from_code("zh-hans"));
    }
    for tag in ["zh-TW", "zh-HK", "zh-MO", "zh-Hant", "zh-Hant-HK"] {
        assert_eq!(first_supported(&format!("{tag}\r\n")), Lang::from_code("zh-hant"));
    }
    assert_eq!(first_supported("en-US\r\n"), Some(Lang::EN));
    assert_eq!(first_supported("fr-FR\r\n"), Lang::from_code("fr"));
    assert_eq!(first_supported("lo-LA\r\n"), Lang::from_code("lo"));
    assert_eq!(first_supported("pt-PT\r\nen-US\r\n"), Lang::from_code("pt-br"));
    assert_eq!(first_supported("no-NO\r\nen-US\r\n"), Lang::from_code("nb"));
}

#[cfg(target_os = "windows")]
#[test]
fn neutral_locale_does_not_override_display_language() {
    needs_catalogs!();
    let fr = Lang::from_code("fr").unwrap();
    for tag in ["C", "C.UTF-8", "POSIX", "posix.UTF-8", "", "  C.UTF-8  ", "unsupported"] {
        assert_eq!(language_override(tag), None, "{tag}");
        assert_eq!(language_override(tag).or_else(|| first_supported("unsupported-XY\nfr-FR\nen-US")), Some(fr));
    }
    assert_eq!(["C.UTF-8", "POSIX", "fr_FR.UTF-8"].into_iter().find_map(language_override), Some(fr));
    assert_eq!(language_override("fr_FR.UTF-8"), Some(fr));
    assert_eq!(language_override("en_US.UTF-8"), Some(Lang::EN));
    assert_eq!(language_override("ja_JP.UTF-8"), Some(JA()));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_display_language_query_returns_a_locale_tag() {
    // One tag per line, as `first_supported` reads them: a machine with several display
    // languages reports them all, so each line is checked rather than the list as a whole.
    let list = windows_ui_language().expect("Windows should report a display language");
    let tags: Vec<&str> = list.lines().map(str::trim).filter(|t| !t.is_empty()).collect();
    assert!(!tags.is_empty(), "no display language in {list:?}");
    for tag in tags {
        assert!(tag.split('-').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric())), "not a locale tag: {tag:?}");
    }
}

#[test]
fn preferences_resolve_with_fallback() {
    assert_eq!(Lang::from_pref("en"), Lang::EN);
    // `auto` and unknown codes follow the system (English under test).
    assert_eq!(Lang::from_pref(AUTO), Lang::EN);
    assert_eq!(Lang::from_pref("xx-unknown"), Lang::EN);
    assert_eq!(normalize_pref("Auto"), Some(AUTO));
    assert_eq!(normalize_pref("xx"), None);
    needs_catalogs!();
    assert_eq!(Lang::from_pref("ja"), JA());
    assert_eq!(Lang::from_pref("JA"), JA());
    assert_eq!(normalize_pref("JA"), Some("ja"));
}

/// PdfCraft offers exactly the languages in `LANGUAGES` when craft-i18n is embedded, and English
/// alone when it isn't: a missing catalog never fails, it shows English.
#[test]
fn the_languages_offered_are_the_embedded_ones_of_the_list() {
    let all: Vec<&str> = Lang::all().map(Lang::code).collect();
    if !catalogs_embedded() {
        assert_eq!(all, ["en"]);
        assert_eq!(Lang::from_code("fr"), None);
        assert_eq!(Lang::from_pref("fr"), Lang::EN, "a saved language without a catalog shows English");
        assert_eq!(normalize_pref("fr"), None);
        assert_eq!(tr(Lang::EN, "File"), "File");
        return;
    }
    let listed: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
    assert_eq!(all, listed, "every language PdfCraft lists has its catalog in craft-i18n");
    assert!(Lang::from_code("pseudo").is_none(), "the pseudo locale is never a setting");
    // craft-i18n has catalogs PdfCraft doesn't offer (yet); they are never offered here.
    for code in CRAFT_I18N.iter().map(|(c, ..)| *c).filter(|c| !listed.contains(c)) {
        assert_eq!(Lang::from_code(code), None, "{code} is not one of PdfCraft's languages");
    }
}

/// Every language in `LANGUAGES` is registered, its bundled catalog loads, and its code survives
/// a save and a restore into a fresh app. A language added to the list without its catalog or
/// without a saved code fails here.
#[test]
fn every_language_is_registered_loads_its_catalog_and_persists() {
    needs_catalogs!();
    for info in &LANGUAGES {
        let lang = Lang::from_code(info.code).unwrap_or_else(|| panic!("{} is not registered", info.code));
        assert_eq!(lang.code(), info.code);
        if lang != Lang::EN {
            assert!(lang.catalog_texts().any(|text| !rows(text).is_empty()), "{}: its catalog holds no rows", info.code);
        }
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.set_option(&egui::Context::default(), "language", info.code).unwrap();
        assert_eq!(app.language, info.code);
        let mut restored = crate::PdfCraftApp::for_unit_tests();
        restored.apply_settings(&app.settings_snapshot());
        assert_eq!(restored.language, info.code, "{}: the saved code is restored", info.code);
    }
}

/// Every literal passed to `tl!(…)` or `crate::i18n::t(…)` in the UI source (test modules
/// aside), unescaped, including literals on a line of their own.
fn ui_literals() -> std::collections::BTreeSet<String> {
    let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    let mut literals = std::collections::BTreeSet::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).expect("UI source directory") {
            let path = entry.expect("UI source entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "i18n") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n");
                let source = source.replace("crate::i18n::t(", "tl!(");
                let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                while let Some((_, after)) = rest.split_once("tl!(") {
                    let after = after.trim_start();
                    let Some(after) = after.strip_prefix('"') else {
                        rest = after;
                        continue;
                    };
                    let mut escaped = false;
                    let end = after
                        .char_indices()
                        .find_map(|(i, c)| {
                            if c == '"' && !escaped {
                                return Some(i);
                            }
                            escaped = c == '\\' && !escaped;
                            None
                        })
                        .expect("closed tl! literal");
                    let (raw, tail) = after.split_at(end);
                    let argument_end = tail.strip_prefix('"').expect("closing quote").trim_start();
                    let literal_call =
                        argument_end.starts_with(')') || argument_end.strip_prefix(',').is_some_and(|rest| rest.trim_start().starts_with(')'));
                    if literal_call {
                        let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                        literals.insert(label);
                    }
                    rest = tail.strip_prefix('"').expect("closing quote");
                }
            }
        }
    }
    literals
}

/// Each translated label of a visible signature that runs into its value keeps a separator
/// at its end.
#[test]
fn signature_appearance_labels_keep_their_separator() {
    needs_catalogs!();
    let english = pdfcraft_engine::sign::AppearanceLabels::default();
    for lang in Lang::all().filter(|l| *l != Lang::EN) {
        for (field, label) in english.fields() {
            let said = tr(lang, label);
            if field != "signed_by" && !lang.rtl() {
                assert!(said.ends_with(' ') || said.ends_with('：'), "{}: the {field} label runs into its value: {said:?}", lang.code());
            }
        }
    }
}

#[test]
fn history_labels_keep_captured_names() {
    set_current(Lang::EN);
    assert_eq!(command_label("Undo Fill in Save {e}"), "Undo Fill in Save {e}");
    needs_catalogs!();
    set_current(JA());
    assert_eq!(action_label("Insert pages from Save {e}.pdf"), "Save {e}.pdf からページを挿入");
    assert_eq!(command_label("Undo Insert pages from Save {e}.pdf"), "取り消し Save {e}.pdf からページを挿入");
    assert_eq!(action_label("Untranslated custom action"), "Untranslated custom action");
    assert_eq!(menu_label("file.open", "Open…"), "開く…");
    set_current(Lang::EN);
}

/// An `@id` row that drops the English label's trailing `…` or changes its `{placeholders}` is not
/// used, by the menus or by `tr_id`: the label's translation shows instead. craft-i18n checks
/// neither for `@id` rows, whose source is the command id.
#[test]
fn an_id_row_that_changes_the_label_s_shape_is_not_used() {
    let l = fixture_lang(
        "x-shape",
        "@id\tfile.open\tÖffnen\n@id\tfile.close\tSchließen {name}\n@id\tfile.save\tSichern…\n\tOpen…\tÖffnen…\n\tClose\tSchließen\n",
    );
    assert_eq!(tr_id(l, "file.open", "Open…"), "Öffnen…", "the row drops the ellipsis");
    assert_eq!(tr_id(l, "file.close", "Close"), "Schließen", "the row adds a placeholder");
    assert_eq!(tr_id(l, "file.save", "Save…"), "Sichern…", "a row that keeps the shape is used");
    set_current(l);
    assert_eq!(menu_label("file.open", "Open…"), "Öffnen…");
    assert_eq!(menu_label("file.close", "Close"), "Schließen");
    assert_eq!(menu_label("file.save", "Save…"), "Sichern…");
    set_current(Lang::EN);
    assert!(same_shape("{b} und {a}", "{a} and {b}"), "placeholders may be reordered");
    assert!(!same_shape("Öffnen {name…", "Open…"), "a placeholder left open");
    assert!(!same_shape("Öffnen name}…", "Open…"), "a closing brace without its opening one");
    assert!(!same_shape("{name}} öffnen", "Open {name}"), "a second closing brace");
}

/// Command ids go by PdfCraft's own `@id` rows only: the shared catalog's are keyed by the other
/// apps' command ids (in Bulgarian its `file.close` row is the id itself). The Undo/Redo phrases
/// are used only as a pair: a shared "Undo {action}" comes without its "Redo {action}".
#[test]
fn command_ids_and_undo_phrases_come_from_pdfcraft_s_own_rows() {
    let fx = fixture_lang(
        "x-own",
        "@id\tfile.close\tClose-FX\n\tUndo {action}\tFX-undo: {action}\n\tClose file\tClose file-FX\n\tUndo\tFX-U\n\tRedo\tFX-R\n",
    );
    assert_eq!(tr_id(fx, "file.close", "Close file"), "Close-FX", "a row of its own is used");
    set_current(fx);
    assert_eq!(menu_label("file.close", "Close file"), "Close-FX");
    assert_eq!(command_label("Undo Something"), "FX-U Something", "one phrase without its pair is not used");
    let pair = fixture_lang("x-pair", "\tUndo {action}\tFX-undo: {action}\n\tRedo {action}\tFX-redo: {action}\n");
    set_current(pair);
    assert_eq!(command_label("Undo Something"), "FX-undo: Something");
    assert_eq!(command_label("Redo Something"), "FX-redo: Something");
    set_current(Lang::EN);
    needs_catalogs!();
    for code in ["bg", "id"] {
        let lang = Lang::from_code(code).expect("registered");
        if lang.has_own("@id", "file.close") {
            continue;
        }
        assert_eq!(tr_id(lang, "file.close", "Close file"), tr(lang, "Close file"), "{code}: file.close is not a shared row's");
        set_current(lang);
        assert_eq!(menu_label("file.close", "Close file"), command_label("Close file"), "{code}");
    }
    for code in ["ja", "zh-hans", "zh-hant", "ru"] {
        let lang = Lang::from_code(code).expect("registered");
        if has(lang, "Redo {action}") {
            continue;
        }
        set_current(lang);
        assert_eq!(command_label("Undo Something"), format!("{} Something", t("Undo")), "{code}");
        assert_eq!(command_label("Redo Something"), format!("{} Something", t("Redo")), "{code}");
    }
    set_current(Lang::EN);
}

#[test]
fn language_persists_and_invalid_input_keeps_current_language() {
    let ctx = egui::Context::default();
    let mut app = crate::PdfCraftApp::for_unit_tests();
    assert_eq!(app.language, AUTO);
    assert!(app.set_option(&ctx, "language", "xx").is_err());
    // Settings written before the system-locale default (no key) follow the system.
    let mut legacy = crate::PdfCraftApp::for_unit_tests();
    legacy.apply_settings(&crate::settings::import_json("{}").0);
    assert_eq!(legacy.language, AUTO);
    needs_catalogs!();
    app.set_option(&ctx, "language", "JA").unwrap();
    assert_eq!(app.language, "ja");
    assert!(app.set_option(&ctx, "language", "xx").is_err());
    assert_eq!(app.language, "ja");
    let mut restored = crate::PdfCraftApp::for_unit_tests();
    restored.apply_settings(&app.settings_snapshot());
    assert_eq!(restored.language, "ja");
    restored.apply_settings(&crate::settings::import_json(r#"{"language":"xx"}"#).0);
    assert_eq!(restored.language, "ja");
    app.set_option(&ctx, "language", AUTO).unwrap();
    assert_eq!(app.language, AUTO);
}

/// `{}` would read as a sentence about nothing in particular.
#[test]
fn spanish_keeps_the_holes_in_what_the_engine_says() {
    needs_catalogs!();
    let es = Lang::from_code("es").expect("es registered");
    let lists: [&[&str]; 5] = [
        pdfcraft_engine::a11y::WORDINGS,
        pdfcraft_engine::pdfa::WORDINGS,
        pdfcraft_engine::sign::pdf::WORDINGS,
        pdfcraft_engine::print::WORDINGS,
        pdfcraft_engine::field_table::WORDINGS,
    ];
    for wording in lists.into_iter().flatten() {
        let said = tr(es, wording);
        // `{}` is one hole; `{name}` holes are named, and each name must survive by itself.
        assert_eq!(wording.matches("{}").count(), said.matches("{}").count(), "the Spanish of {wording:?} loses a hole: {said:?}");
        for hole in wording.split('{').skip(1).filter_map(|rest| rest.split_once('}')).map(|(name, _)| name).filter(|n| !n.is_empty()) {
            assert!(said.contains(&format!("{{{hole}}}")), "the Spanish of {wording:?} loses {{{hole}}}: {said:?}");
        }
    }
}

/// Telugu puts the verb last, so it translates "Undo {action}" as a whole phrase; a catalog
/// without that entry (Spanish) keeps the verb in front of the action.
#[test]
fn undo_and_redo_labels_use_the_catalog_phrase_when_there_is_one() {
    needs_catalogs!();
    let te = Lang::from_code("te").expect("te registered");
    set_current(te);
    assert_eq!(command_label("Undo"), "రద్దు చేయి");
    assert_eq!(command_label("Undo Untranslated custom action"), "రద్దు చేయి: Untranslated custom action");
    assert_eq!(command_label("Redo Untranslated custom action"), "మళ్లీ చేయి: Untranslated custom action");
    let es = Lang::from_code("es").expect("es registered");
    set_current(es);
    assert_eq!(command_label("Undo Untranslated custom action"), format!("{} Untranslated custom action", tr(es, "Undo")));
    set_current(Lang::EN);
    assert_eq!(command_label("Undo Untranslated custom action"), "Undo Untranslated custom action");
}

#[test]
fn tool_names_inside_sentences_are_lowercased_except_in_german() {
    set_current(Lang::EN);
    assert_eq!(in_sentence("Text Box"), "text box");
    needs_catalogs!();
    set_current(Lang::from_code("de").expect("de registered"));
    assert_eq!(in_sentence("Textfeld"), "Textfeld");
    set_current(Lang::EN);
}

#[test]
fn turkish_keeps_its_dotted_and_dotless_i_when_changing_case() {
    needs_catalogs!();
    set_current(Lang::from_code("tr").expect("tr registered"));
    assert_eq!(upper_label("Advanced"), "GELİŞMİŞ");
    assert_eq!(upper_label("Signatures"), "İMZALAR");
    // A label with no Turkish row is still English and keeps the English rule.
    assert_eq!(upper_label("Untranslated title"), "UNTRANSLATED TITLE");
    assert_eq!(in_sentence("İmza Alanı"), "imza alanı");
    assert_eq!(in_sentence("Işık"), "ışık");
    set_current(Lang::EN);
    assert_eq!(upper_label("Advanced"), "ADVANCED");
    assert_eq!(in_sentence("Image"), "image");
}

fn arabic() -> Lang {
    Lang::from_code("ar").expect("ar registered")
}

/// Catalog lookups stay in logical order; the egui 0.37 fork owns bidi shaping and display
/// reordering (E5/E12), so preordering here would reverse Arabic twice. Accessibility names,
/// copied labels and search therefore see the source string.
#[test]
fn arabic_catalog_stays_logical() {
    needs_catalogs!();
    let ar = arabic();
    assert_eq!(tr(ar, "Save as…"), "حفظ باسم…");
    assert_eq!(tr(ar, "About PdfCraft"), "حول PdfCraft");
    assert_eq!(tr(ar, "Name:"), "الاسم:");
    assert_eq!(tr(ar, "Issued by: "), "صادرة عن: ");
    // Placeholders survive in their logical positions.
    assert_eq!(tr(ar, "Couldn't open {name}: {e}"), "تعذّر فتح {name}: {e}");
    assert_eq!(fmt(tr(ar, "Couldn't open {name}: {e}"), &[("name", "a {e}.pdf"), ("e", "x")]), "تعذّر فتح a {e}.pdf: x");
    assert_eq!(fmt(tr(ar, "Page {p} of {n}"), &[("p", "3"), ("n", "12")]), "الصفحة 3 من 12");
    set_current(ar);
    let command = command_label("Undo Insert pages from a.pdf");
    assert!(command.contains("a.pdf") && command.contains("إدراج صفحات"), "logical command label: {command:?}");
    assert_eq!(menu_label("file.saveAs", "Save as…"), "حفظ باسم…");
    set_current(Lang::EN);
}

/// The language list is well-formed. Problems in the embedded catalogs are not failed: craft-i18n
/// checks its rows, a row PdfCraft can't use shows in English, and `translation_gaps_report`
/// lists the `@id` rows PdfCraft doesn't use ([`unused_id_rows`]).
#[test]
fn bundled_catalogs_are_consistent() {
    let mut codes = std::collections::HashSet::new();
    for l in &LANGUAGES {
        assert!(codes.insert(l.code), "duplicate code {}", l.code);
        assert!(l.code == l.code.to_ascii_lowercase() && !l.code.is_empty(), "{}", l.code);
    }
    assert_eq!(LANGUAGES.first().map(|l| l.code), Some("en"), "English first: the source and the fallback");
    if !needs_catalogs() {
        return;
    }
    let problems = I18N.check_all();
    if !problems.is_empty() {
        println!("{} catalog rows are skipped (they show in English):\n{problems:#?}", problems.len());
    }
}

/// The `@id` rows of the embedded PdfCraft catalogs that are never shown, for the translation gaps
/// report: a row for a command PdfCraft doesn't have, and one that doesn't keep its label's
/// placeholders or trailing `…` ([`same_shape`]), whose label's translation shows instead. One
/// line each under a heading; empty when there are none.
fn unused_id_rows() -> String {
    let mut out = Vec::new();
    for lang in Lang::all().filter(|l| *l != Lang::EN) {
        for (_, id, translation) in rows(app_catalog(lang.code())).into_iter().filter(|(c, ..)| c == "@id") {
            if id == "language.name" {
                continue;
            }
            let Some(command) = pdfcraft_engine::commands::command(&id) else {
                out.push(format!("{}: @id {id:?} names a command PdfCraft doesn't have", lang.code()));
                continue;
            };
            if !same_shape(&translation, command.label) {
                let label = command.label;
                out.push(format!("{}: @id {id:?} doesn't keep the placeholders or ellipsis of {label:?}: {translation:?}", lang.code()));
            }
        }
    }
    if out.is_empty() {
        return String::new();
    }
    format!("{} @id rows not used (their label's translation shows):\n{}", out.len(), out.iter().map(|row| format!("  {row}\n")).collect::<String>())
}

/// The (context, English source) keys PdfCraft looks up: the UI source's literals, the commands,
/// All tools, the engine's own wordings, and every key of every embedded PdfCraft catalog (which
/// covers contextual, plural and dynamically looked-up strings the scan can't see).
fn english_keys() -> std::collections::BTreeSet<(String, String)> {
    use std::collections::BTreeSet;
    let mut plain: BTreeSet<String> = ui_literals();
    let mut add = |s: &str| {
        plain.insert(s.to_string());
    };
    for command in pdfcraft_engine::commands::COMMANDS {
        add(command.label);
        command.menu.into_iter().for_each(&mut add);
    }
    for group in pdfcraft_engine::catalog::TOOL_GROUPS {
        add(group.label);
        for section in group.sections {
            add(section.title);
            section.items.iter().for_each(|item| add(item.label));
        }
    }
    pdfcraft_engine::catalog::TOOL_CATEGORIES.iter().for_each(|category| add(category.label));
    let engine: [&[&str]; 9] = [
        pdfcraft_engine::a11y::WORDINGS,
        pdfcraft_engine::a11y::REPORT_WORDS,
        pdfcraft_engine::pdfa::WORDINGS,
        pdfcraft_engine::sign::pdf::WORDINGS,
        pdfcraft_engine::sign::pdf::kind::ALL,
        pdfcraft_engine::print::WORDINGS,
        pdfcraft_engine::EDIT_LABELS,
        pdfcraft_engine::BATCH_LABELS,
        pdfcraft_engine::field_table::WORDINGS,
    ];
    engine.into_iter().flatten().for_each(|s| add(s));
    pdfcraft_engine::sign::AppearanceLabels::default().fields().into_iter().for_each(|(_, label)| add(label));
    // (context, source) keys; `@id` rows only reword a command label, which is in `plain`.
    let mut keys: BTreeSet<(String, String)> = plain.into_iter().map(|s| (String::new(), s)).collect();
    for (_, _, app) in CRAFT_I18N {
        keys.extend(rows(app).into_iter().filter(|(context, ..)| context != "@id").map(|(context, source, _)| (context, source)));
    }
    keys
}

/// The translation gaps report (`cargo xtask i18n-report`, a step of `cargo xtask ci`): for
/// each language, how many English strings it has no translation for, and the first few, and how
/// many are kept in English (a row whose translation is the English itself). It never fails: a
/// missing string shows in English, and translators fill it in craft-i18n. Without embedded
/// catalogs it says so.
///
/// The English strings are the UI source's literals, the commands, All tools, the engine's
/// own wordings, and every key of every embedded PdfCraft catalog (which covers contextual,
/// plural and dynamically looked-up strings the scan can't see).
#[test]
fn translation_gaps_report() {
    const SHOWN: usize = 3;
    if !catalogs_embedded() {
        println!("{REPORT_START}\nTranslation gaps: skipped, built without craft-i18n (set CRAFT_I18N_DIR to a craft-i18n checkout)\n{REPORT_END}");
        return;
    }
    let keys = english_keys();
    let problems = I18N.check_all();
    let mut report = format!("Translation gaps: {} English strings; {} catalog rows skipped (they show in English)\n", keys.len(), problems.len());
    for lang in Lang::all().filter(|l| *l != Lang::EN) {
        let found = |context: &str, source: &str| lang.i18n().get(lang.lang, context, source).map(str::to_owned);
        let missing: Vec<&(String, String)> = keys
            .iter()
            .filter(|(context, source)| match context.as_str() {
                "" => found("", source).is_none(),
                // A contextual lookup falls back to the plain translation.
                context => found(context, source).is_none() && (context == "@plural" || found("", source).is_none()),
            })
            .collect();
        let kept = keys.iter().filter(|(context, source)| context.is_empty() && found("", source).as_ref() == Some(source)).count();
        let mut line = format!("  {:<8} {:>5} missing, {kept:>4} kept in English", lang.code(), missing.len());
        for (i, (context, source)) in missing.iter().take(SHOWN).enumerate() {
            let mut shown: String = source.chars().take(48).collect();
            if shown.len() < source.len() {
                shown.push('…');
            }
            let key = if context.is_empty() { format!("{shown:?}") } else { format!("{context} / {shown:?}") };
            line.push_str(if i == 0 { ": " } else { ", " });
            line.push_str(&key);
        }
        if missing.len() > SHOWN {
            line.push_str(", …");
        }
        report.push_str(&line);
        report.push('\n');
    }
    report.push_str(&unused_id_rows());
    println!("{REPORT_START}\n{report}{REPORT_END}");
}

/// The lines `cargo xtask i18n-report` prints from the test output lie between these.
const REPORT_START: &str = "=== translation gaps ===";
const REPORT_END: &str = "=== end of translation gaps ===";

/// Preferences ▸ Date format ▸ Language offers exactly the interface languages, in order, with
/// the interface's names for them when the catalogs are embedded.
#[test]
fn date_languages_are_the_interface_languages() {
    let interface: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
    let dates: Vec<&str> = pdfcraft_engine::dates::DATE_LANGUAGES.iter().map(|l| l.code).collect();
    assert_eq!(dates, interface);
    needs_catalogs!();
    let interface: Vec<(&str, &str)> = Lang::all().map(|l| (l.code(), l.name())).collect();
    let dates: Vec<(&str, &str)> = pdfcraft_engine::dates::DATE_LANGUAGES.iter().map(|l| (l.code, l.name)).collect();
    assert_eq!(dates, interface);
}

/// The stale-draft notice remains discoverable after the UI modules are reorganized.
#[test]
fn properties_stale_notice_is_in_translation_scan() {
    assert!(ui_literals().contains("The document changed. Reopen Document Properties to edit the current version."));
}
