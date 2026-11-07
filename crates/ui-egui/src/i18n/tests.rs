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

/// Every translation `code`'s embedded catalogs hold, each plural form on its own: the fonts must
/// draw all of them.
fn translations(code: &str) -> String {
    let lang = Lang::from_code(code).expect("registered");
    lang.catalog_texts().flat_map(rows).flat_map(|(_, _, t)| t.split('|').map(str::to_owned).collect::<Vec<_>>()).collect()
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
    // Only the pending Punjabi starter catalog may be absent from a configured checkout.
    let expected: Vec<&str> = listed.iter().copied().filter(|c| *c != "pa-guru" || CRAFT_I18N.iter().any(|(code, ..)| code == c)).collect();
    assert_eq!(all, expected, "all established languages must retain their embedded catalogs");
    assert!(Lang::from_code("pseudo").is_none(), "the pseudo locale is never a setting");
    // craft-i18n has catalogs PdfCraft doesn't offer (yet); they are never offered here.
    for code in CRAFT_I18N.iter().map(|(c, ..)| *c).filter(|c| !listed.contains(c)) {
        assert_eq!(Lang::from_code(code), None, "{code} is not one of PdfCraft's languages");
    }
}

#[test]
fn lookups_fall_back_to_english() {
    assert_eq!(tr(Lang::EN, "File"), "File");
    assert!(!has(Lang::EN, "Save"));
    needs_catalogs!();
    assert_eq!(tr(JA(), "no such label"), "no such label");
    assert_eq!(tr(JA(), "File"), "ファイル");
    assert_eq!(tr(JA(), "日本語の文書.pdf"), "日本語の文書.pdf");
    assert_eq!(tr_id(JA(), "no.such.id", "File"), "ファイル");
    assert_eq!(tr_ctx(JA(), "no such context", "File"), "ファイル");
    assert!(has(JA(), "Save"));
}

#[test]
fn catalog_kinds_are_parsed_and_looked_up() {
    let l = fixture_lang(
        "ja",
        "# c\n\tHello\tこんにちは\n@id\tfile.save\t保存する\nmenu\tWindows\tウィンドウ群\n@plural\t{n} file|{n} files\t{n} 個\n\n",
    );
    assert!(l.i18n().check_all().is_empty(), "{:?}", l.i18n().check_all());
    assert_eq!(tr(l, "Hello"), "こんにちは");
    assert_eq!(tr_id(l, "file.save", "Save"), "保存する");
    assert_eq!(tr_ctx(l, "menu", "Windows"), "ウィンドウ群");
    assert_eq!(tr_ctx(l, "other", "Windows"), "Windows");
    assert_eq!(trn(l, 0, "{n} file", "{n} files"), "0 個");
    assert_eq!(trn(l, 5, "{n} file", "{n} files"), "5 個");
}

/// A bad line is skipped and reported; the rest of the catalog still loads.
#[test]
fn malformed_lines_are_reported_not_fatal() {
    let l = fixture_lang("x-bad", "\tok\tはい\nno tabs here\n\tonly\n\ta\tb\tc\textra\n\t\tempty source\n\tOpen…\tOpen\n");
    assert_eq!(tr(l, "ok"), "はい");
    assert_eq!(tr(l, "Open…"), "Open…", "a row that drops the ellipsis is skipped");
    assert_eq!(l.i18n().check_all().len(), 5, "{:?}", l.i18n().check_all());
}

#[test]
fn catalogs_handle_escapes_crlf_and_distinct_contexts() {
    let l =
        fixture_lang("x-esc", "# comment\r\n\r\n\tLine\\nTab\\tPath\\\\\tOther\\nTab\\tPath\\\\\r\nweight\tLight\tThin\r\ntheme\tLight\tPale\r\n");
    assert!(l.i18n().check_all().is_empty(), "{:?}", l.i18n().check_all());
    assert_eq!(tr(l, "Line\nTab\tPath\\"), "Other\nTab\tPath\\");
    assert_eq!(tr_ctx(l, "weight", "Light"), "Thin");
    assert_eq!(tr_ctx(l, "theme", "Light"), "Pale");
}

#[test]
fn broken_catalog_falls_back_to_english() {
    let broken = fixture_lang("x-broken", "malformed\n\\");
    assert_eq!(tr(broken, "File"), "File");
    assert_eq!(trn(broken, 2, "{n} page", "{n} pages"), "2 pages");
}

#[test]
fn contextual_id_and_plural_lookups_have_english_fallbacks() {
    let l = fixture_lang(
        "de",
        "\tLight\tPlain light\nweight\tLight\tThin\n@id\tfile.open\tOpen dialog…\n@plural\t{n} page|{n} pages\tOne page: {n}|Many pages: {n}\n",
    );
    assert_eq!(tr_ctx(l, "weight", "Light"), "Thin");
    assert_eq!(tr_ctx(l, "theme", "Light"), "Plain light");
    assert_eq!(tr_ctx(l, "theme", "Unknown"), "Unknown");
    assert_eq!(tr_id(l, "file.open", "Reworded English…"), "Open dialog…");
    assert_eq!(tr_id(l, "unknown", "Light"), "Plain light");
    assert_eq!(tr_id(l, "unknown", "Unknown"), "Unknown");
    assert_eq!(trn(l, 1, "{n} page", "{n} pages"), "One page: 1");
    assert_eq!(trn(l, 0, "{n} page", "{n} pages"), "Many pages: 0");
    assert_eq!(trn(l, 1234, "{n} page", "{n} pages"), "Many pages: 1234", "no grouping, as before");
    assert_eq!(trn(l, u64::MAX, "{n} page", "{n} pages"), format!("Many pages: {}", u64::MAX));
}

#[test]
fn formatting_reorders_parameters_without_reinterpreting_user_text() {
    let args = [("file", "日本語-{n}.pdf"), ("n", "2")];
    assert_eq!(fmt("{n}: {file}; {unknown}", &args), "2: 日本語-{n}.pdf; {unknown}");
    assert_eq!(fmt("{file} / {file}", &args), "日本語-{n}.pdf / 日本語-{n}.pdf");
    assert_eq!(fmt("text {unfinished", &args), "text {unfinished");
    assert_eq!(fmt("}{n}{", &args), "}2{");
    assert_eq!(fmt("unchanged", &[]), "unchanged");
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
                // A non-breaking space also separates the value; catalogs use it to retain
                // a trailing separator without a trailing ASCII blank in the TSV file.
                assert!(
                    said.ends_with(' ') || said.ends_with('\u{a0}') || said.ends_with('：'),
                    "{}: the {field} label runs into its value: {said:?}",
                    lang.code()
                );
            }
        }
    }
}

/// With craft-fonts, every Japanese translation has glyphs: with all interface faces (desktop)
/// and with BIZ UDPGothic Regular alone (the web build's only Japanese face).
#[test]
fn japanese_labels_have_glyphs() {
    let craft = pdfcraft_fonts::ui_japanese_fonts();
    let Some(web) = craft.iter().find(|f| f.family == "BIZ UDPGothic" && f.style == "Regular") else {
        eprintln!("skipping Japanese glyph checks: build with CRAFT_FONTS_DIR to run them");
        return;
    };
    needs_catalogs!();
    let labels: String = translations("ja").chars().chain(JA().name().chars()).filter(|c| !c.is_control()).collect();
    let web_name = web.name();
    let mut web_only = crate::theme::font_definitions();
    for family in web_only.families.values_mut() {
        family.retain(|name| !pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.name() == *name) || *name == web_name);
    }
    use egui::epaint::text::{Fonts, TextOptions};
    for (build, defs) in [("desktop", crate::theme::font_definitions()), ("web", web_only)] {
        let mut fonts = Fonts::new(TextOptions::default(), defs);
        for id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0), crate::theme::medium(13.0), crate::theme::semibold(17.0)] {
            assert!(fonts.has_glyphs(&id, &labels), "{build}: {id:?} lacks a Japanese label glyph");
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
fn current_language_is_per_thread() {
    set_current(fixture_lang("x-thread", "\tFile\tファイル\n"));
    assert_eq!(t("File"), "ファイル");
    std::thread::spawn(|| assert_eq!(current(), Lang::EN)).join().unwrap();
    set_current(Lang::EN);
    assert_eq!(t("File"), "File");
}

#[test]
fn plurals_and_placeholders() {
    assert_eq!(trn(Lang::EN, 1, "{n} item", "{n} items"), "1 item");
    assert_eq!(trn(Lang::EN, 0, "{n} item", "{n} items"), "0 items");
    assert_eq!(trn(Lang::EN, 7, "{n} item", "{n} items"), "7 items");
    assert_eq!(trn(Lang::EN, 12345, "{n} item", "{n} items"), "12345 items");
    assert_eq!(fmt("{b} before {a}", &[("a", "x"), ("b", "y"), ("c", "z")]), "y before x");
    assert_eq!(fmt("{missing}", &[]), "{missing}");
    needs_catalogs!();
    // No catalog has this `@plural` entry: Japanese falls back to the English forms.
    assert_eq!(trn(JA(), 7, "{n} test item", "{n} test items"), "7 test items");
}

/// PdfCraft's plural rules before craft-i18n (form index by count), kept to show the crate's
/// CLDR categories pick the same form of the same rows.
fn pdfcraft_plural_rule(code: &str) -> fn(u64) -> usize {
    fn one_other(n: u64) -> usize {
        usize::from(n != 1)
    }
    fn none(_: u64) -> usize {
        0
    }
    fn zero_one_other(n: u64) -> usize {
        usize::from(n > 1)
    }
    fn ro(n: u64) -> usize {
        if n == 1 {
            0
        } else if n == 0 || (1..=19).contains(&(n % 100)) {
            1
        } else {
            2
        }
    }
    fn lt(n: u64) -> usize {
        match (n % 10, n % 100) {
            (1, 11) => 2,
            (1, _) => 0,
            (2..=9, 12..=19) => 2,
            (2..=9, _) => 1,
            _ => 2,
        }
    }
    fn cs(n: u64) -> usize {
        match n {
            1 => 0,
            2..=4 => 1,
            _ => 2,
        }
    }
    fn ru_uk(n: u64) -> usize {
        match n % 100 {
            11..=14 => 2,
            _ => match n % 10 {
                1 => 0,
                2..=4 => 1,
                _ => 2,
            },
        }
    }
    fn ar(n: u64) -> usize {
        match (n, n % 100) {
            (0, _) => 0,
            (1, _) => 1,
            (2, _) => 2,
            (_, 3..=10) => 3,
            (_, 11..=99) => 4,
            _ => 5,
        }
    }
    fn pl(n: u64) -> usize {
        match (n, n % 100, n % 10) {
            (1, _, _) => 0,
            (_, 12..=14, _) => 2,
            (_, _, 2..=4) => 1,
            _ => 2,
        }
    }
    fn sl(n: u64) -> usize {
        match n % 100 {
            1 => 0,
            2 => 1,
            3 | 4 => 2,
            _ => 3,
        }
    }
    match code {
        "ja" | "zh-hans" | "zh-hant" | "tr" | "id" | "vi" | "th" | "lo" | "hi" | "ko" => none,
        "cs" => cs,
        "pt-br" | "fr" | "fa" => zero_one_other,
        "ru" | "uk" => ru_uk,
        "ar" => ar,
        "pl" => pl,
        "sl" => sl,
        "lt" => lt,
        "ro" => ro,
        _ => one_other,
    }
}

/// The counts the plural checks try: 0..=200, then large ones with every ending.
fn plural_counts() -> impl Iterator<Item = u64> {
    (0..=200).chain([1000, 1001, 1002, 1003, 1011, 1012, 1021, 1100, 1_000_000, 1_000_001, 2_000_000, 123_456_789, u64::MAX - 13, u64::MAX])
}

/// craft-i18n picks plural forms by CLDR category, PdfCraft by its own rules: for every language
/// the two choose the same form wherever they list as many forms. Where CLDR lists more (Hindi
/// and Turkish have two where PdfCraft had one), the catalog repeats the form, so the text is the
/// same; the embedded rows are checked for that and any that differ are reported.
#[test]
fn plural_forms_follow_the_rules_pdfcraft_had() {
    let mut split = Vec::new();
    // English has no catalog: its own two forms, `one` for 1 alone, as PdfCraft's rule had it.
    assert_eq!(plural_counts().map(|n| trn(Lang::EN, n, "1", "x")).filter(|s| s == "1").count(), 1);
    // Punjabi has no legacy PdfCraft plural rule to compare against.
    for info in LANGUAGES.iter().filter(|l| !matches!(l.code, "en" | "pa-guru")) {
        let rule = pdfcraft_plural_rule(info.code);
        let pdfcraft_forms = (0..=1000).map(rule).max().unwrap_or(0) + 1;
        // How many forms CLDR gives this language: the first form count whose row loads.
        let fixture = |forms: usize| {
            let row = (0..forms).map(|i| format!("F{i} {{n}}")).collect::<Vec<_>>().join("|");
            fixture_lang(info.code, Box::leak(format!("@plural\t{{n}} x|{{n}} xs\t{row}\n").into_boxed_str()))
        };
        let (forms, lang) =
            (1..=6).map(|f| (f, fixture(f))).find(|(_, l)| trn(*l, 5, "{n} x", "{n} xs").starts_with('F')).expect("a form count loads");
        if forms != pdfcraft_forms {
            split.push((info.code, pdfcraft_forms, forms));
            assert_eq!(pdfcraft_forms, 1, "{}: PdfCraft listed {pdfcraft_forms} forms, CLDR {forms}", info.code);
            continue;
        }
        for n in plural_counts() {
            assert_eq!(trn(lang, n, "{n} x", "{n} xs"), format!("F{} {n}", rule(n)), "{}: count {n}", info.code);
        }
    }
    let codes: Vec<&str> = split.iter().map(|(c, ..)| *c).collect();
    assert_eq!(codes, ["tr", "hi"], "the languages CLDR gives more forms than PdfCraft had");
    if !needs_catalogs() {
        return;
    }
    for (code, ..) in split {
        let lang = Lang::from_code(code).expect("registered");
        for (_, source, forms) in lang.catalog_texts().flat_map(rows).filter(|(c, ..)| c == "@plural") {
            let distinct: std::collections::BTreeSet<&str> = forms.split('|').collect();
            if distinct.len() > 1 {
                println!("{code}: {source:?} has forms that differ, so PdfCraft's single form is no longer the only text: {forms:?}");
            }
        }
    }
}

#[test]
fn language_persists_and_invalid_input_keeps_current_language() {
    let mut app = crate::PdfCraftApp::default();
    assert_eq!(app.language, AUTO);
    assert!(app.set_option("language", "xx").is_err());
    // Settings written before the system-locale default (no key) follow the system.
    let mut legacy = crate::PdfCraftApp::default();
    legacy.restore("{}");
    assert_eq!(legacy.language, AUTO);
    needs_catalogs!();
    app.set_option("language", "JA").unwrap();
    assert_eq!(app.language, "ja");
    assert!(app.set_option("language", "xx").is_err());
    assert_eq!(app.language, "ja");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "ja");
    restored.restore(r#"{"language":"xx"}"#);
    assert_eq!(restored.language, "ja");
    app.set_option("language", AUTO).unwrap();
    assert_eq!(app.language, AUTO);
}

#[test]
fn brazilian_portuguese_is_registered() {
    needs_catalogs!();
    let pt = Lang::from_code("pt-br").expect("pt-br registered");
    assert_eq!(pt.name(), "Português (Brasil)");
    assert_eq!(Lang::from_code("PT-BR"), Some(pt));
    assert_eq!(normalize_pref("pt-BR"), Some("pt-br"));
    assert_eq!(normalize_pref("pt"), None, "only exact codes are preferences");
    assert_eq!(lang_from_tag("pt_BR.UTF-8"), Some(pt));
    assert_eq!(lang_from_tag("pt_PT"), Some(pt));
    assert_eq!(tr(pt, "Arquivo do usuário.pdf"), "Arquivo do usuário.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "pt-br").unwrap();
    assert_eq!(app.language, "pt-br");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "pt-br");
}

#[test]
fn spanish_is_registered() {
    needs_catalogs!();
    let es = Lang::from_code("es").expect("es registered");
    assert_eq!(es.name(), "Español");
    assert_eq!(normalize_pref("ES"), Some("es"));
    assert_eq!(lang_from_tag("es_ES.UTF-8"), Some(es));
    assert_eq!(lang_from_tag("es-MX"), Some(es));
    assert_eq!(first_supported("es-ES\r\nen-US"), Some(es));
    assert_eq!(tr(es, "Informe del usuario.pdf"), "Informe del usuario.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "es").unwrap();
    assert_eq!(app.language, "es");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "es");
}

/// Each sentence the engine words for itself keeps its holes when it is said in Spanish, so
/// the document's own words still have somewhere to go: a translation that drops `{p}` or
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

#[test]
fn telugu_is_registered() {
    needs_catalogs!();
    let te = Lang::from_code("te").expect("te registered");
    assert_eq!(te.name(), "తెలుగు");
    assert_eq!(normalize_pref("TE"), Some("te"));
    assert_eq!(lang_from_tag("te_IN.UTF-8"), Some(te));
    assert_eq!(first_supported("te-IN\r\nen-US"), Some(te));
    assert_eq!(tr(te, "నివేదిక.pdf"), "నివేదిక.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "te").unwrap();
    assert_eq!(app.language, "te");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "te");
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

/// A label that reads differently by use has its own contextual entry where a language needs
/// one; a language without it falls back to the plain translation.
#[test]
fn contextual_labels_fall_back_to_the_plain_translation() {
    needs_catalogs!();
    let te = Lang::from_code("te").expect("te registered");
    assert_eq!(tr_ctx(te, "signature pad", "Type"), "టైప్ చేయి");
    assert_eq!(tr(te, "Type"), "రకం");
    // A context no catalog defines, so the fallback stays tested as catalogs fill their gaps.
    let es = Lang::from_code("es").expect("es registered");
    assert_eq!(tr_ctx(es, "no such context", "Type"), tr(es, "Type"));
}

/// With a craft-fonts Telugu face, every Telugu translation has glyphs: with all interface
/// faces (desktop) and with the Telugu face as the only craft-fonts face (the web build keeps
/// it; Telugu labels must not need a Japanese or Arabic face).
#[test]
fn telugu_labels_have_glyphs() {
    let telugu: Vec<String> = pdfcraft_fonts::ui_telugu_fonts().iter().map(|f| f.name()).collect();
    if telugu.is_empty() {
        eprintln!("skipping Telugu glyph checks: build with CRAFT_FONTS_DIR and a Telugu face to run them");
        return;
    }
    needs_catalogs!();
    let te = Lang::from_code("te").expect("te registered");
    let labels: String = translations("te").chars().chain(te.name().chars()).filter(|c| !c.is_control()).collect();
    let mut telugu_only = crate::theme::font_definitions();
    for family in telugu_only.families.values_mut() {
        family.retain(|name| !pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.name() == *name) || telugu.contains(name));
    }
    use egui::epaint::text::{Fonts, TextOptions};
    for (build, defs) in [("all faces", crate::theme::font_definitions()), ("Telugu face only", telugu_only)] {
        let mut fonts = Fonts::new(TextOptions::default(), defs);
        for id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0), crate::theme::medium(13.0), crate::theme::semibold(17.0)] {
            assert!(fonts.has_glyphs(&id, &labels), "{build}: {id:?} lacks a Telugu label glyph");
        }
    }
}

#[test]
fn czech_is_registered_and_persists() {
    needs_catalogs!();
    let cs = Lang::from_code("cs").expect("cs registered");
    assert_eq!(cs.name(), "Čeština");
    assert_eq!(tr(cs, "Žluťoučký kůň.pdf"), "Žluťoučký kůň.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "cs").unwrap();
    assert!(app.set_option("language", "cz").is_err());
    assert_eq!(app.language, "cs");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "cs");
}

#[test]
fn turkish_is_registered_and_persists() {
    needs_catalogs!();
    let tr_lang = Lang::from_code("tr").expect("tr registered");
    assert_eq!(tr_lang.name(), "Türkçe");
    assert_eq!(tr(tr_lang, "Türkçe Belge.pdf"), "Türkçe Belge.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "tr").unwrap();
    assert!(app.set_option("language", "tu").is_err());
    assert_eq!(app.language, "tr");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "tr");
}

/// The code joins some labels with a space at either end, so these catalogs (PdfCraft's rows in
/// craft-i18n) keep exactly the source's, and write … rather than three dots.
#[test]
fn catalog_rows_keep_the_source_spacing_and_ellipsis() {
    needs_catalogs!();
    let edges = |s: &str| (s.len() - s.trim_start().len(), s.len() - s.trim_end().len());
    for code in ["cs", "tr", "vi"] {
        for (_, source, translation) in rows(app_catalog(code)) {
            assert!(!translation.contains("..."), "{code}: use … rather than three dots: {translation:?}");
            assert_eq!(edges(&translation), edges(&source), "{code}: stray whitespace: {translation:?}");
        }
    }
}

#[test]
fn simplified_chinese_is_registered() {
    needs_catalogs!();
    let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
    assert_eq!(zh.name(), "简体中文");
    assert_eq!(normalize_pref("zh-Hans"), Some("zh-hans"));
    assert_eq!(normalize_pref("zh"), None, "only exact codes are preferences");
    assert_eq!(tr(zh, "报告.pdf"), "报告.pdf");
}

#[test]
fn traditional_chinese_is_registered() {
    needs_catalogs!();
    let zh = Lang::from_code("zh-hant").expect("zh-hant registered");
    assert_eq!(zh.name(), "繁體中文");
    assert_eq!(normalize_pref("zh-Hant"), Some("zh-hant"));
    assert_eq!(tr(zh, "報告.pdf"), "報告.pdf");
}

#[test]
fn french_is_registered() {
    needs_catalogs!();
    let fr = Lang::from_code("fr").expect("fr registered");
    assert_eq!(fr.name(), "Français");
    assert_eq!(normalize_pref("FR"), Some("fr"));
    assert_eq!(lang_from_tag("fr_FR.UTF-8"), Some(fr));
    assert_eq!(lang_from_tag("fr-CA"), Some(fr));
    assert_eq!(first_supported("fr-FR\r\nen-US"), Some(fr));
    assert_eq!(tr(fr, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "fr").unwrap();
    assert_eq!(app.language, "fr");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "fr");
}

#[test]
fn german_is_registered() {
    needs_catalogs!();
    let de = Lang::from_code("de").expect("de registered");
    assert_eq!(de.name(), "Deutsch");
    assert_eq!(normalize_pref("DE"), Some("de"));
    assert_eq!(lang_from_tag("de_DE.UTF-8"), Some(de));
    assert_eq!(lang_from_tag("de-AT"), Some(de));
    assert_eq!(lang_from_tag("de_CH"), Some(de));
    assert_eq!(first_supported("de-DE\r\nen-US"), Some(de));
    assert_eq!(tr(de, "Bericht des Nutzers.pdf"), "Bericht des Nutzers.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "de").unwrap();
    assert_eq!(app.language, "de");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "de");
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

#[test]
fn russian_is_registered() {
    needs_catalogs!();
    let ru = Lang::from_code("ru").expect("ru registered");
    assert_eq!(ru.name(), "Русский");
    assert_eq!(normalize_pref("RU"), Some("ru"));
    assert_eq!(lang_from_tag("ru_RU.UTF-8"), Some(ru));
    assert_eq!(lang_from_tag("ru-BY"), Some(ru));
    assert_eq!(first_supported("ru-RU\r\nen-US"), Some(ru));
    assert_eq!(tr(ru, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "ru").unwrap();
    assert_eq!(app.language, "ru");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "ru");
}

#[test]
fn finnish_is_registered_and_persists() {
    needs_catalogs!();
    let fi = Lang::from_code("fi").expect("fi registered");
    assert_eq!(fi.name(), "Suomi");
    assert_eq!(normalize_pref("FI"), Some("fi"));
    for tag in ["fi", "fi-FI", "fi_FI.UTF-8", "FI-fi"] {
        assert_eq!(lang_from_tag(tag), Some(fi), "{tag}");
    }
    // Filipino (`fil`) and Swedish in Finland (`sv-FI`) don't pick Finnish.
    assert_ne!(lang_from_tag("fil"), Some(fi));
    assert_ne!(lang_from_tag("sv-FI"), Some(fi));
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "FI").unwrap();
    assert_eq!(app.language, "fi");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "fi");
}

#[test]
fn lithuanian_is_registered_and_persists() {
    needs_catalogs!();
    let lt = Lang::from_code("lt").expect("lt registered");
    assert_eq!(lt.name(), "Lietuvių");
    assert_eq!(normalize_pref("LT"), Some("lt"));
    for tag in ["lt", "lt-LT", "lt_LT.UTF-8", "LT-lt"] {
        assert_eq!(lang_from_tag(tag), Some(lt), "{tag}");
    }
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "LT").unwrap();
    assert_eq!(app.language, "lt");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "lt");
    assert!(restored.set_option("language", "xx").is_err());
}

#[test]
fn norwegian_bokmal_is_registered_and_persists() {
    needs_catalogs!();
    let nb = Lang::from_code("nb").expect("nb registered");
    assert_eq!(nb.name(), "Norsk bokmål");
    assert_eq!(normalize_pref("NB"), Some("nb"));
    assert_eq!(normalize_pref("no"), None, "only exact codes are preferences");
    // Current systems say `nb`; older ones and some Linux setups say `no`. Nynorsk is `nn`.
    for tag in ["nb", "nb-NO", "nb_NO.UTF-8", "no", "no_NO.UTF-8", "NO-no", "nn", "nn_NO.UTF-8"] {
        assert_eq!(lang_from_tag(tag), Some(nb), "{tag}");
    }
    assert_eq!(first_supported("(\n    \"nb-NO\",\n    \"en-US\"\n)\n"), Some(nb));
    assert_eq!(first_supported("nn-NO\r\nen-US\r\n"), Some(nb));
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "NB").unwrap();
    assert_eq!(app.language, "nb");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "nb");
}

#[test]
fn vietnamese_is_registered_and_persists() {
    needs_catalogs!();
    let vi = Lang::from_code("vi").expect("vi registered");
    assert_eq!(vi.name(), "Tiếng Việt");
    assert_eq!(normalize_pref("Vi"), Some("vi"));
    for tag in ["vi", "vi-VN", "vi_VN.UTF-8", "VI-vn"] {
        assert_eq!(lang_from_tag(tag), Some(vi), "{tag}");
    }
    assert_eq!(first_supported("vi-VN\r\nen-US"), Some(vi));
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "vi").unwrap();
    assert_eq!(app.language, "vi");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "vi");
    assert!(restored.set_option("language", "xx").is_err());
    assert_eq!(restored.language, "vi");
}

#[test]
fn romanian_is_registered_with_three_plural_forms() {
    // one: 1; few: 0 and n % 100 in 1..=19 ("2 pagini", "101 pagini"); other: the "de" form ("20 de pagini").
    let form = plural_fixture("ro", 3);
    for (n, f) in [(1, 0), (0, 1), (2, 1), (19, 1), (101, 1), (119, 1), (20, 2), (100, 2), (120, 2), (1000, 2)] {
        assert_eq!(form(n), f, "{n}");
    }
    needs_catalogs!();
    let ro = Lang::from_code("ro").expect("ro registered");
    for tag in ["ro", "ro-RO", "ro_RO.UTF-8", "RO-ro", "ro-MD"] {
        assert_eq!(lang_from_tag(tag), Some(ro), "{tag}");
    }
    assert_eq!(normalize_pref("Ro"), Some("ro"));
}

#[test]
fn catalan_is_registered_and_persists() {
    needs_catalogs!();
    let ca = Lang::from_code("ca").expect("ca registered");
    assert_eq!(ca.name(), "Català");
    assert_eq!(normalize_pref("CA"), Some("ca"));
    for tag in ["ca", "ca-ES", "ca_ES.UTF-8", "CA-es", "ca_ES@valencia"] {
        assert_eq!(lang_from_tag(tag), Some(ca), "{tag}");
    }
    assert_eq!(first_supported("ca-ES\r\nen-US"), Some(ca));
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "CA").unwrap();
    assert_eq!(app.language, "ca");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "ca");
    assert!(restored.set_option("language", "xx").is_err());
    assert_eq!(restored.language, "ca");
}

#[test]
fn slovene_is_registered_and_persists() {
    needs_catalogs!();
    let sl = Lang::from_code("sl").expect("sl registered");
    assert_eq!(sl.name(), "Slovenščina");
    assert_eq!(normalize_pref("SL"), Some("sl"));
    for tag in ["sl", "sl-SI", "sl_SI.UTF-8", "SL-si", "sl_SI@variant"] {
        assert_eq!(lang_from_tag(tag), Some(sl), "{tag}");
    }
    assert_eq!(first_supported("sl-SI\r\nen-US"), Some(sl));
    assert_eq!(tr(sl, "Poročilo {n}.pdf"), "Poročilo {n}.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "SL").unwrap();
    assert_eq!(app.language, "sl");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "sl");
    assert!(restored.set_option("language", "xx").is_err());
    assert_eq!(restored.language, "sl");
}

#[test]
fn albanian_is_registered_and_persists() {
    needs_catalogs!();
    let sq = Lang::from_code("sq").expect("sq registered");
    assert_eq!(sq.name(), "Shqip");
    assert_eq!(normalize_pref("SQ"), Some("sq"));
    for tag in ["sq", "sq-AL", "sq_AL.UTF-8", "SQ-xk", "sq_XK@variant"] {
        assert_eq!(lang_from_tag(tag), Some(sq), "{tag}");
    }
    assert_eq!(first_supported("sq-AL\r\nen-US"), Some(sq));
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "SQ").unwrap();
    assert_eq!(app.language, "sq");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "sq");
    assert!(restored.set_option("language", "xx").is_err());
    assert_eq!(restored.language, "sq");
}

#[test]
fn slovene_plurals_cover_dual_few_teens_and_large_counts() {
    let form = plural_fixture("sl", 4);
    for (counts, f) in [
        (vec![1, 101, 201, 1001, 1101], 0),
        (vec![2, 102, 202, 1002], 1),
        (vec![3, 4, 103, 104, 1003], 2),
        (vec![0, 5, 10, 11, 12, 13, 14, 15, 20, 21, 22, 23, 24, 25, 31, 100, 111, 112, 121, 122, 1000, u64::MAX], 3),
    ] {
        for n in counts {
            assert_eq!(form(n), f, "{n}");
        }
    }
}

#[test]
fn indonesian_and_vietnamese_are_registered_and_persist() {
    needs_catalogs!();
    let id = Lang::from_code("id").expect("id registered");
    let vi = Lang::from_code("vi").expect("vi registered");
    assert_eq!(id.name(), "Bahasa Indonesia");
    assert_eq!(vi.name(), "Tiếng Việt");
    assert_eq!(normalize_pref("ID"), Some("id"));
    assert_eq!(normalize_pref("Vi"), Some("vi"));
    for tag in ["id", "id-ID", "id_ID.UTF-8", "ID-id"] {
        assert_eq!(lang_from_tag(tag), Some(id), "{tag}");
    }
    for tag in ["vi", "vi-VN", "vi_VN.UTF-8", "VI-vn"] {
        assert_eq!(lang_from_tag(tag), Some(vi), "{tag}");
    }
    assert_eq!(first_supported("id-ID\r\nen-US"), Some(id));
    assert_eq!(first_supported("vi-VN\r\nen-US"), Some(vi));
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "ID").unwrap();
    assert_eq!(app.language, "id");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "id");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "vi").unwrap();
    assert_eq!(app.language, "vi");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "vi");
    assert!(restored.set_option("language", "xx").is_err());
    assert_eq!(restored.language, "vi");
}

#[test]
fn hindi_and_korean_are_registered_and_persist() {
    needs_catalogs!();
    let hi = Lang::from_code("hi").expect("hi registered");
    let ko = Lang::from_code("ko").expect("ko registered");
    assert_eq!(hi.name(), "हिन्दी");
    assert_eq!(ko.name(), "한국어");
    assert_eq!(normalize_pref("HI"), Some("hi"));
    assert_eq!(normalize_pref("Ko"), Some("ko"));
    for tag in ["hi", "hi-IN", "hi_IN.UTF-8", "HI-in"] {
        assert_eq!(lang_from_tag(tag), Some(hi), "{tag}");
    }
    for tag in ["ko", "ko-KR", "ko_KR.UTF-8", "KO-kr"] {
        assert_eq!(lang_from_tag(tag), Some(ko), "{tag}");
    }
    assert_eq!(first_supported("hi-IN\r\nen-US"), Some(hi));
    assert_eq!(first_supported("ko-KR\r\nen-US"), Some(ko));
    for code in ["hi", "ko"] {
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", &code.to_uppercase()).unwrap();
        assert_eq!(app.language, code);
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, code);
        assert!(restored.set_option("language", "xx").is_err());
        assert_eq!(restored.language, code);
    }
}

#[test]
fn bulgarian_is_registered() {
    needs_catalogs!();
    let bg = Lang::from_code("bg").expect("bg registered");
    assert_eq!(bg.name(), "Български");
    assert_eq!(normalize_pref("BG"), Some("bg"));
    assert_eq!(lang_from_tag("bg_BG.UTF-8"), Some(bg));
    assert_eq!(lang_from_tag("bg-BG"), Some(bg));
    assert_eq!(first_supported("bg-BG\r\nen-US"), Some(bg));
    assert_eq!(tr(bg, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "bg").unwrap();
    assert_eq!(app.language, "bg");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "bg");
}

#[test]
fn hungarian_is_registered() {
    needs_catalogs!();
    let hu = Lang::from_code("hu").expect("hu registered");
    assert_eq!(hu.name(), "Magyar");
    assert_eq!(normalize_pref("HU"), Some("hu"));
    assert_eq!(lang_from_tag("hu_HU.UTF-8"), Some(hu));
    assert_eq!(lang_from_tag("hu-HU"), Some(hu));
    assert_eq!(first_supported("hu-HU\r\nen-US"), Some(hu));
    assert_eq!(tr(hu, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "hu").unwrap();
    assert_eq!(app.language, "hu");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "hu");
}

#[test]
fn ukrainian_is_registered_and_persists() {
    needs_catalogs!();
    let uk = Lang::from_code("uk").expect("uk registered");
    assert_eq!(uk.name(), "Українська");
    assert_eq!(normalize_pref("UK"), Some("uk"));
    for tag in ["uk", "uk-UA", "uk_UA.UTF-8", "UK-ua", "uk_UA@variant"] {
        assert_eq!(lang_from_tag(tag), Some(uk), "{tag}");
    }
    assert_eq!(first_supported("uk-UA\r\nen-US"), Some(uk));
    assert_eq!(first_supported("(\n    \"uk-UA\",\n    \"en-US\"\n)\n"), Some(uk));
    assert_eq!(tr(uk, "Звіт {n}.pdf"), "Звіт {n}.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "UK").unwrap();
    assert_eq!(app.language, "uk");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "uk");
    assert!(restored.set_option("language", "xx").is_err());
    assert_eq!(restored.language, "uk");
}

#[test]
fn ukrainian_integer_plurals_cover_teens_and_large_counts() {
    let form = plural_fixture("uk", 3);
    for (counts, f) in [
        (vec![1, 21, 31, 101, 121, 1001], 0),
        (vec![2, 3, 4, 22, 23, 24, 102, 104, 122, 1002], 1),
        (vec![0, 5, 10, 11, 12, 13, 14, 19, 20, 25, 100, 110, 111, 112, 114, 1000, u64::MAX], 2),
    ] {
        for n in counts {
            assert_eq!(form(n), f, "{n}");
        }
    }
}

fn arabic() -> Lang {
    Lang::from_code("ar").expect("ar registered")
}

#[test]
fn arabic_is_registered_and_persists() {
    needs_catalogs!();
    let ar = arabic();
    assert_eq!(ar.name(), "العربية");
    assert!(ar.rtl());
    let rtl: Vec<&str> = Lang::all().filter(|l| l.rtl()).map(|l| l.code()).collect();
    assert_eq!(rtl, ["ar", "ckb", "fa"], "Arabic, Kurdish and Persian are the right-to-left languages");
    assert_eq!(normalize_pref("AR"), Some("ar"));
    for tag in ["ar", "ar-MA", "ar_EG.UTF-8", "AR-sa", "ar_DZ@variant"] {
        assert_eq!(lang_from_tag(tag), Some(ar), "{tag}");
    }
    assert_eq!(first_supported("ar-MA\r\nen-US"), Some(ar));
    // Unknown strings and user text are never reordered by a lookup.
    assert_eq!(tr(ar, "تقرير نهائي {n}.pdf"), "تقرير نهائي {n}.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "AR").unwrap();
    assert_eq!(app.language, "ar");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "ar");
    assert!(restored.set_option("language", "xx").is_err());
    assert_eq!(restored.language, "ar");
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

#[test]
fn arabic_plurals_follow_the_six_cldr_forms() {
    let form = plural_fixture("ar", 6);
    assert_eq!([0, 1, 2].map(&form), [0, 1, 2]);
    assert_eq!([3, 10, 103, 110, 1003].map(&form), [3; 5]);
    assert_eq!([11, 99, 111, 199, 1011].map(&form), [4; 5]);
    assert_eq!([100, 101, 102, 200, 1000, u64::MAX - 13].map(&form), [5; 6]);
}

fn persian() -> Lang {
    Lang::from_code("fa").expect("fa registered")
}

#[test]
fn persian_is_registered_and_persists() {
    needs_catalogs!();
    let fa = persian();
    assert_eq!(fa.name(), "فارسی");
    assert!(fa.rtl());
    // CLDR `fa` (one: i = 0 or n = 1): 0 and 1 are singular, every other count the plural.
    let form = plural_fixture("fa", 2);
    assert_eq!([0, 1, 2, 11].map(&form), [0, 0, 1, 1]);
    assert_eq!(normalize_pref("FA"), Some("fa"));
    for tag in ["fa", "fa-IR", "fa_IR.UTF-8", "FA-af", "fa_IR@calendar=persian"] {
        assert_eq!(lang_from_tag(tag), Some(fa), "{tag}");
    }
    assert_eq!(first_supported("fa-IR\r\nen-US"), Some(fa));
    // Lookups return Persian in logical order; epaint handles visual ordering.
    assert_eq!(tr(fa, "Save"), "ذخیره");
    assert_eq!(tr(fa, "Cancel"), "لغو");
    assert_eq!(tr(fa, "File"), "فایل");
    // Unknown strings and user text are never reordered by a lookup.
    assert_eq!(tr(fa, "گزارش نهایی {n}.pdf"), "گزارش نهایی {n}.pdf");
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "FA").unwrap();
    assert_eq!(app.language, "fa");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "fa");
    assert!(restored.set_option("language", "xx").is_err());
    assert_eq!(restored.language, "fa");
}

/// A form picker for `code`: a fixture row with `forms` plural forms, and for a count the form
/// index the crate's CLDR rules choose.
fn plural_fixture(code: &'static str, forms: usize) -> impl Fn(u64) -> usize {
    let row = (0..forms).map(|i| format!("F{i} {{n}}")).collect::<Vec<_>>().join("|");
    let lang = fixture_lang(code, Box::leak(format!("@plural\t{{n}} x|{{n}} xs\t{row}\n").into_boxed_str()));
    move |n| {
        let said = trn(lang, n, "{n} x", "{n} xs");
        said.strip_prefix('F')
            .and_then(|s| s.split(' ').next())
            .and_then(|i| i.parse().ok())
            .unwrap_or_else(|| panic!("{code}: no form for {n}: {said:?}"))
    }
}

/// With a craft-fonts Arabic face, every Arabic translation has glyphs: with all interface
/// faces (desktop) and with the Arabic face as the only craft-fonts face (the web build keeps
/// it; Arabic labels must not need a Japanese face).
#[test]
fn arabic_labels_have_glyphs() {
    let faces: Vec<String> = pdfcraft_fonts::ui_arabic_fonts().iter().map(|f| f.name()).collect();
    if faces.is_empty() {
        eprintln!("skipping Arabic glyph checks: build with CRAFT_FONTS_DIR and an Arabic face to run them");
        return;
    }
    needs_catalogs!();
    let ar = arabic();
    let labels: String = translations("ar").chars().chain(ar.name().chars()).filter(|c| !c.is_control()).collect();
    let mut arabic_only = crate::theme::font_definitions();
    for family in arabic_only.families.values_mut() {
        family.retain(|name| !pdfcraft_fonts::CRAFT_FONTS.iter().any(|face| face.name() == *name) || faces.contains(name));
    }
    use egui::epaint::text::{Fonts, TextOptions};
    for (build, defs) in [("all faces", crate::theme::font_definitions()), ("Arabic face only", arabic_only)] {
        let mut fonts = Fonts::new(TextOptions::default(), defs);
        for id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0), crate::theme::medium(13.0), crate::theme::semibold(17.0)] {
            let missing: std::collections::BTreeSet<char> = labels.chars().filter(|c| !fonts.has_glyphs(&id, &c.to_string())).collect();
            assert!(missing.is_empty(), "{build}: {id:?} lacks Arabic label glyphs {missing:?}");
        }
    }
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
    crate::credits::NameMode::ALL.iter().for_each(|mode| add(mode.label()));
    for key in crate::credits::SortKey::ALL {
        let (label, header) = key.label();
        add(label);
        add(header);
    }
    crate::credits::MODEL_COLUMNS.iter().for_each(|s| add(s));
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
    let dates: Vec<(&str, &str)> = pdfcraft_engine::dates::DATE_LANGUAGES
        .iter()
        .filter(|l| l.code != "pa-guru" || Lang::from_code(l.code).is_some())
        .map(|l| (l.code, l.name))
        .collect();
    assert_eq!(dates, interface);
}

/// A report, not a check: the notifications pasting an image and pinning a tool show, in each
/// language that has no translation for them yet (they show in English).
#[test]
fn image_paste_and_home_notifications_are_translated() {
    for language in Lang::all().filter(|l| *l != Lang::EN) {
        for source in ["Search…", "Image pasted", "No image on clipboard to paste", "Pinned {tool} to Home", "Unpinned {tool} from Home"] {
            if !has(language, source) {
                println!("{}: no translation for {source:?} (shows in English)", language.code());
            }
        }
    }
}

/// Punjabi selection must never infer Gurmukhi from Shahmukhi/Pakistan or bare Punjabi.
#[test]
fn punjabi_routing_keeps_scripts_separate() {
    for tag in ["pa-Guru", "pa-Guru-IN", "pa-IN", "pa_IN.UTF-8", "pa-Guru-PK"] {
        assert!(candidates(tag).iter().any(|c| c == "pa-guru"), "{tag}");
        assert_eq!(lang_from_tag(tag), Lang::from_code("pa-guru"), "{tag}");
    }
    for tag in ["pa", "pa-Arab", "pa-Arab-PK", "pa-PK", "pa-Latn-IN", "pa_Arab_IN.UTF-8"] {
        assert!(!candidates(tag).iter().any(|c| c == "pa-guru"), "{tag}");
        assert_eq!(lang_from_tag(tag), None, "{tag}");
    }
}

/// A partial catalog can be stored; absent catalogs must not offer Punjabi.
#[test]
fn punjabi_catalog_and_picker_fallback() {
    let Some(lang) = Lang::from_code("pa-guru") else {
        assert!(!Lang::all().any(|l| l.code() == "pa-guru"));
        assert_eq!(normalize_pref("pa-guru"), None);
        return;
    };
    assert_eq!(normalize_pref("PA-GURU"), Some("pa-guru"));
    assert!(!lang.rtl());
    assert_eq!(lang.name(), "ਪੰਜਾਬੀ");
    assert_eq!(lang.picker_name(|_| false), "Punjabi (Gurmukhi)");
    assert_eq!(lang.picker_name(|_| true), "ਪੰਜਾਬੀ");
    assert_eq!(tr(lang, "File"), "ਫ਼ਾਈਲ");
    assert_eq!(tr(lang, "Untranslated synthetic label"), "Untranslated synthetic label");
    assert_eq!(first_supported("pa-IN\r\nen-US"), Some(lang));
    assert_eq!(first_supported("pa-Arab-PK\r\nen-US"), Some(Lang::EN));
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "PA-GURU").expect("Punjabi option");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "pa-guru");
}

/// Punjabi uses the runtime CLDR rules, not a fabricated pre-migration rule.
#[test]
fn punjabi_plural_rules_come_from_runtime() {
    let lang = fixture_lang("pa-guru", "@plural\t{n} item|{n} items\tONE {n}|OTHER {n}\n");
    for (n, expected) in [(0, "ONE 0"), (1, "ONE 1"), (2, "OTHER 2")] {
        assert_eq!(trn(lang, n, "{n} item", "{n} items"), expected);
    }
}

/// Exercise the installed Windows fallback through egui; glyph coverage alone is insufficient.
#[cfg(target_os = "windows")]
#[test]
fn punjabi_egui_layout_and_missing_font_picker() {
    let Some(lang) = Lang::from_code("pa-guru") else { return };
    use egui::epaint::text::{Fonts, TextOptions};
    let id = egui::FontId::proportional(17.0);
    let mut embedded = Fonts::new(TextOptions::default(), crate::theme::font_definitions());
    if !embedded.has_glyphs(&id, lang.name()) {
        assert_eq!(lang.picker_name(|name| embedded.has_glyphs(&id, name)), "Punjabi (Gurmukhi)");
    }
    let defs = crate::theme::installed_font_definitions(crate::theme::CjkPreference::Default);
    let mut installed = Fonts::new(TextOptions::default(), defs.clone());
    let strings = ["ਪੰਜਾਬੀ ਭਾਸ਼ਾ", "ਸਤ ਸ੍ਰੀ ਅਕਾਲ", "ਸਿੱਖਿਆ", "ਅੱਖਰ"];
    if !strings.iter().all(|s| installed.has_glyphs(&id, s)) {
        eprintln!("skipped installed Gurmukhi layout: no suitable runtime face");
        return;
    }
    let ctx = egui::Context::default();
    ctx.set_fonts(defs);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        for text in strings {
            let galley = ui.painter().layout_no_wrap(text.to_owned(), id.clone(), egui::Color32::BLACK);
            assert!(galley.size().x.is_finite() && galley.size().x > 0.0);
            ui.label(text);
        }
    });
    assert!(!ctx.tessellate(output.shapes, output.pixels_per_point).is_empty());
    // This headless test has no GPU consumer; explicitly acknowledge unused texture updates.
    output.textures_delta.clear();
}
