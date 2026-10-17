//! UI localisation. Strings in code stay English and are the default lookup keys; a per-language
//! catalog (`*.tsv`, see `ja.tsv` for the format) maps them to display text at render time. Command
//! ids, document text, file names, the control channel, the CLI and MCP never see translated text,
//! so agents and scripts are unaffected. A string without a translation is shown in English, so
//! coverage can grow incrementally.
//!
//! The system is the one PhotoCraft uses (`photocraft/crates/ui-egui/src/i18n`), so catalogs and
//! habits carry over between the two apps.
//!
//! # Adding a language
//! 1. Add `xx.tsv` next to `ja.tsv` (copy its header; translate from the *meaning* of the English
//!    text, clean-room, see `ja.tsv`).
//! 2. Add one row to [`LANGUAGES`] (code, native name, catalog, plural rule).
//!
//! That is all: the Preferences dropdown, the system-locale match and the catalog tests (parse,
//! placeholders, plural forms) pick it up from the registry.
//!
//! # Looking strings up
//! - [`tl!`](crate::tl) / [`t`]: a plain string in the current language. [`tr`]: in a given one.
//! - [`tr_ctx`] (`tl_ctx!` in UI code): when one English word needs different translations.
//! - [`tr_id`]: a command-id keyed string with the English label as fallback (menu items), so a
//!   translation survives rewording of the English text and can differ per command.
//! - [`trn`]: plural-aware (`{n}` is filled in). [`fmt`]: fill `{name}` placeholders after [`tr`];
//!   translators may reorder placeholders freely.

mod catalog;

use std::cell::Cell;
use std::sync::OnceLock;

use catalog::Catalog;

/// The `language` preference value that follows the system locale.
pub const AUTO: &str = "auto";

/// One supported UI language.
pub struct LangInfo {
    /// BCP 47 code, lowercase (`ja`, `zh-hans`, `pt-br`). Also the `language` preference value.
    pub code: &'static str,
    /// The language's name in itself, shown in the Preferences dropdown.
    pub name: &'static str,
    /// Catalog file contents (empty for the built-in English).
    pub source: &'static str,
    /// Plural form index for a count (English: 0 = one, 1 = other; Japanese: always 0). A catalog's
    /// `@plural` entries list one form per index.
    pub plural: fn(u64) -> usize,
    /// The language is written right to left. egui lays text out left to right, so the catalog's
    /// translations are put into display order when it loads ([`crate::bidi::display_rtl`]).
    pub rtl: bool,
    catalog: OnceLock<Catalog>,
}

fn plural_one_other(n: u64) -> usize {
    usize::from(n != 1)
}

fn plural_none(_: u64) -> usize {
    0
}

/// Lithuanian: one (ending in 1 except 11), few (ending in 2–9 except 12–19), other otherwise.
fn plural_lithuanian(n: u64) -> usize {
    match (n % 10, n % 100) {
        (1, 11) => 2,
        (1, _) => 0,
        (2..=9, 12..=19) => 2,
        (2..=9, _) => 1,
        _ => 2,
    }
}

/// Czech: 1 → one, 2–4 → few, everything else (0, 5+) → other.
fn plural_cs(n: u64) -> usize {
    match n {
        1 => 0,
        2..=4 => 1,
        _ => 2,
    }
}

/// Portuguese: 0 and 1 take the singular, everything else the plural.
fn plural_pt(n: u64) -> usize {
    usize::from(n > 1)
}

/// French (CLDR `fr`): 0 and 1 take the singular, everything else the plural.
fn plural_fr(n: u64) -> usize {
    usize::from(n > 1)
}

/// Russian: 1 (but not 11) is `one`, 2–4 (but not 12–14) `few`, everything else `many`.
fn plural_russian(n: u64) -> usize {
    match n % 100 {
        11..=14 => 2,
        _ => match n % 10 {
            1 => 0,
            2..=4 => 1,
            _ => 2,
        },
    }
}

/// Ukrainian (CLDR `uk`, integer counts): one (1, 21), few (2–4, 22–24), many (0, 5–19).
/// Counts ending in 11–14 always take many, regardless of the last digit.
fn plural_ukrainian(n: u64) -> usize {
    match n % 100 {
        11..=14 => 2,
        _ => match n % 10 {
            1 => 0,
            2..=4 => 1,
            _ => 2,
        },
    }
}

/// Arabic (CLDR `ar`, integer counts): zero (0), one (1), two (2), few (3–10, 103–110),
/// many (11–99, 111–199), other (100–102, 200–202).
fn plural_arabic(n: u64) -> usize {
    match (n, n % 100) {
        (0, _) => 0,
        (1, _) => 1,
        (2, _) => 2,
        (_, 3..=10) => 3,
        (_, 11..=99) => 4,
        _ => 5,
    }
}

/// Polish integer counts: 1 is one; endings 2–4 except 12–14 are few; the rest are many.
fn plural_polish(n: u64) -> usize {
    match (n, n % 100, n % 10) {
        (1, _, _) => 0,
        (_, 12..=14, _) => 2,
        (_, _, 2..=4) => 1,
        _ => 2,
    }
}

/// Slovene (CLDR `sl`): one (1, 101, 201 ...) | two (2, 102 ...) | few (3, 4, 103 ...) | other.
fn plural_sl(n: u64) -> usize {
    match n % 100 {
        1 => 0,
        2 => 1,
        3 | 4 => 2,
        _ => 3,
    }
}

/// The registry. English first: it is the fallback and the source language.
pub static LANGUAGES: [LangInfo; 30] = [
    LangInfo { code: "en", name: "English", source: "", plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    LangInfo { code: "ja", name: "日本語", source: include_str!("ja.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Simplified Chinese; `zh`, `zh-CN`, `zh-SG` and `zh-Hans-*` locales resolve here (see `candidates`).
    LangInfo {
        code: "zh-hans", name: "简体中文", source: include_str!("zh-hans.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new()
    },
    // Traditional Chinese in the vocabulary used in Taiwan; `zh-TW`, `zh-HK`, `zh-MO` and `zh-Hant-*`
    // locales all resolve here (see `candidates`).
    LangInfo {
        code: "zh-hant", name: "繁體中文", source: include_str!("zh-hant.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new()
    },
    LangInfo { code: "cs", name: "Čeština", source: include_str!("cs.tsv"), plural: plural_cs, rtl: false, catalog: OnceLock::new() },
    // Brazilian Portuguese; `pt`, `pt-BR` and `pt-PT` locales all resolve here (see `candidates`).
    LangInfo {
        code: "pt-br",
        name: "Português (Brasil)",
        source: include_str!("pt-br.tsv"),
        plural: plural_pt,
        rtl: false,
        catalog: OnceLock::new(),
    },
    // German (informal "du"); every `de-*` locale (`de-DE`, `de-AT`, `de-CH` ...) resolves here.
    LangInfo { code: "de", name: "Deutsch", source: include_str!("de.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Spanish (European vocabulary); every `es-*` locale (`es-ES`, `es-MX`, `es-419` ...) resolves here.
    LangInfo { code: "es", name: "Español", source: include_str!("es.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // French; every `fr-*` locale (`fr-FR`, `fr-CA`, `fr-BE` ...) resolves here.
    LangInfo { code: "fr", name: "Français", source: include_str!("fr.tsv"), plural: plural_fr, rtl: false, catalog: OnceLock::new() },
    // Russian; every `ru-*` locale (`ru-RU`, `ru-BY`, `ru-KZ` ...) resolves here.
    LangInfo { code: "ru", name: "Русский", source: include_str!("ru.tsv"), plural: plural_russian, rtl: false, catalog: OnceLock::new() },
    // Kazakh; every `kk-*` locale (`kk-KZ`, `kk-KZ.UTF-8`) resolves here.
    LangInfo { code: "kk", name: "Қазақша", source: include_str!("kk.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Bulgarian; every `bg-*` locale (`bg-BG`) resolves here.
    LangInfo {
        code: "bg", name: "Български", source: include_str!("bg.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new()
    },
    // Telugu; every `te-*` locale (`te-IN`) resolves here.
    LangInfo {
        code: "te", name: "తెలుగు", source: include_str!("te.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new()
    },
    // Hungarian; every `hu-*` locale (`hu-HU`) resolves here.
    LangInfo { code: "hu", name: "Magyar", source: include_str!("hu.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Ukrainian; every `uk-*` locale (`uk-UA`, `uk_UA.UTF-8`) resolves here.
    LangInfo {
        code: "uk", name: "Українська", source: include_str!("uk.tsv"), plural: plural_ukrainian, rtl: false, catalog: OnceLock::new()
    },
    // Italian; every `it-*` locale (`it-IT`) resolves here.
    LangInfo { code: "it", name: "Italiano", source: include_str!("it.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Arabic (Modern Standard, Western digits); every `ar-*` locale (`ar-MA`, `ar-EG`, `ar-SA` ...)
    // resolves here. Right to left: see `LangInfo::rtl`.
    LangInfo { code: "ar", name: "العربية", source: include_str!("ar.tsv"), plural: plural_arabic, rtl: true, catalog: OnceLock::new() },
    // Polish; every `pl-*` locale (`pl-pl`, `pl_pl.UTF-8`) resolves here.
    LangInfo { code: "pl", name: "Polski", source: include_str!("pl.tsv"), plural: plural_polish, rtl: false, catalog: OnceLock::new() },
    // Slovene; every `sl-*` locale (`sl-SI`) resolves here. Four plural forms (one, two, few, other).
    LangInfo { code: "sl", name: "Slovenščina", source: include_str!("sl.tsv"), plural: plural_sl, rtl: false, catalog: OnceLock::new() },
    LangInfo { code: "el", name: "Ελληνικά", source: include_str!("el.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Turkish; every `tr-*` locale (`tr-TR`, `tr_TR.UTF-8`) resolves here.
    LangInfo { code: "tr", name: "Türkçe", source: include_str!("tr.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Lithuanian; every `lt-*` locale (`lt-LT`, `lt_LT.UTF-8`) resolves here. Three plural forms (one, few, other).
    LangInfo { code: "lt", name: "Lietuvių", source: include_str!("lt.tsv"), plural: plural_lithuanian, rtl: false, catalog: OnceLock::new() },
    // Indonesian; every `id-*` locale (`id-ID`, `id_ID.UTF-8`) resolves here. CLDR `id` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "id", name: "Bahasa Indonesia", source: include_str!("id.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Vietnamese; every `vi-*` locale (`vi-VN`, `vi_VN.UTF-8`) resolves here. CLDR `vi` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "vi", name: "Tiếng Việt", source: include_str!("vi.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Hindi; every `hi-*` locale (`hi-IN`, `hi_IN.UTF-8`) resolves here. CLDR `hi` has "one"
    // (0 and 1) and "other", but the catalog's nouns are the same in both ("{n} पृष्ठ"), so
    // `@plural` entries list a single form.
    LangInfo { code: "hi", name: "हिन्दी", source: include_str!("hi.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Korean; every `ko-*` locale (`ko-KR`, `ko_KR.UTF-8`) resolves here. CLDR `ko` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "ko", name: "한국어", source: include_str!("ko.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Dutch; every `nl-*` locale (`nl-NL`, `nl-BE`) resolves here.
    LangInfo { code: "nl", name: "Nederlands", source: include_str!("nl.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Danish; every `da-*` locale (`da-DK`, `da_DK.UTF-8`) resolves here.
    LangInfo { code: "da", name: "Dansk", source: include_str!("da.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Swedish; every `sv-*` locale (`sv-SE`, `sv-FI`) resolves here.
    LangInfo { code: "sv", name: "Svenska", source: include_str!("sv.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Finnish; every `fi-*` locale (`fi-FI`, `fi_FI.UTF-8`) resolves here.
    LangInfo { code: "fi", name: "Suomi", source: include_str!("fi.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
];

impl LangInfo {
    /// How many plural forms the language's `@plural` entries list.
    fn plural_forms(&self) -> usize {
        (0..=1000).map(self.plural).max().unwrap_or(0).saturating_add(1)
    }

    fn catalog(&self) -> &Catalog {
        self.catalog.get_or_init(|| {
            let (mut catalog, errors) = Catalog::parse(self.source, self.plural_forms());
            for error in errors {
                log::warn!("{} interface catalog: {error}; that entry shows in English", self.code);
            }
            if self.rtl {
                catalog.map_translations(crate::bidi::display_rtl);
            }
            catalog
        })
    }
}

/// A language the UI can be shown in (a handle into [`LANGUAGES`]).
#[derive(Clone, Copy)]
pub struct Lang(&'static LangInfo);

impl std::fmt::Debug for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lang({})", self.0.code)
    }
}

impl PartialEq for Lang {
    fn eq(&self, other: &Self) -> bool {
        self.0.code == other.0.code
    }
}

impl Eq for Lang {}

impl Lang {
    pub const EN: Lang = Lang(&LANGUAGES[0]);

    pub fn code(self) -> &'static str {
        self.0.code
    }

    /// A language by its exact code.
    pub fn from_code(code: &str) -> Option<Lang> {
        LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code)).map(Lang)
    }

    /// Resolve the `language` preference: a language code, or [`AUTO`] (and anything unknown,
    /// e.g. a code from a newer version) to follow the system locale.
    pub fn from_pref(pref: &str) -> Lang {
        Lang::from_code(pref).unwrap_or_else(system_lang)
    }

    /// Every registered language.
    pub fn all() -> impl Iterator<Item = Lang> {
        LANGUAGES.iter().map(Lang)
    }

    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// Whether the language is written right to left. Its translations come back from the
    /// lookups in display order (see [`LangInfo::rtl`]).
    pub fn rtl(self) -> bool {
        self.0.rtl
    }

    fn catalog(self) -> &'static Catalog {
        self.0.catalog()
    }
}

/// The canonical `language` preference for user input: [`AUTO`] or a registered code (any case).
/// `None` for anything else, so callers can keep the current setting.
pub fn normalize_pref(pref: &str) -> Option<&'static str> {
    if pref.eq_ignore_ascii_case(AUTO) {
        return Some(AUTO);
    }
    Lang::from_code(pref).map(Lang::code)
}

/// Candidate language codes for a locale tag, most specific first: `zh_TW.UTF-8` →
/// `zh-tw`, `zh-hant`, `zh`.
fn candidates(tag: &str) -> Vec<String> {
    let base = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-").to_ascii_lowercase();
    let parts: Vec<&str> = base.split('-').filter(|p| !p.is_empty()).collect();
    let Some(&primary) = parts.first() else { return Vec::new() };
    let mut out = Vec::new();
    for n in (1..=parts.len()).rev() {
        out.push(parts.get(..n).unwrap_or_default().join("-"));
    }
    if primary == "zh" && !parts.iter().any(|p| matches!(*p, "hans" | "hant")) {
        // Chinese by region when no script is given.
        let script = if parts.iter().any(|p| matches!(*p, "tw" | "hk" | "mo")) { "zh-hant" } else { "zh-hans" };
        out.insert(out.len().saturating_sub(1), script.to_string());
    }
    if primary == "pt" && !out.iter().any(|c| c == "pt-br") {
        // The only Portuguese catalog is Brazilian; other regions use it rather than English.
        out.insert(out.len().saturating_sub(1), "pt-br".to_string());
    }
    out
}

/// The registered language for a locale tag such as `ja_JP.UTF-8`, `ja-JP`; `None` if the
/// language isn't supported. `C`/`POSIX` mean English.
pub fn lang_from_tag(tag: &str) -> Option<Lang> {
    let cands = candidates(tag);
    if matches!(cands.first().map(String::as_str), Some("c" | "posix")) {
        return Some(Lang::EN);
    }
    cands.iter().find_map(|c| Lang::from_code(c))
}

/// The system language (cached). English when it can't be determined.
pub fn system_lang() -> Lang {
    // Tests drive the UI by its English labels whatever the developer's locale is. This covers
    // the unit tests; the integration tests link the library as it ships, where `cfg!(test)` is
    // off, so each of their harnesses pins the `language` option to `en` instead.
    if cfg!(test) {
        return Lang::EN;
    }
    static SYSTEM: OnceLock<Lang> = OnceLock::new();
    *SYSTEM.get_or_init(detect_system_lang)
}

#[cfg(not(target_arch = "wasm32"))]
fn detect_system_lang() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(l) = std::env::var(var).ok().and_then(|v| language_override(&v)) {
            return l;
        }
    }
    // Apps started from the Finder don't inherit LANG: use the macOS preferred-languages list.
    // The absolute path keeps a `defaults` earlier on PATH from running; any failure means English.
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleLanguages"]).output()
        && out.status.success()
        && let Some(l) = first_supported(&String::from_utf8_lossy(&out.stdout))
    {
        return l;
    }
    #[cfg(target_os = "windows")]
    if let Some(l) = windows_ui_language().as_deref().and_then(first_supported) {
        return l;
    }
    Lang::EN
}

/// A named locale can override the display language. On Windows, the neutral
/// C/POSIX locale inherited from a launcher says nothing about the user's UI language.
#[cfg(not(target_arch = "wasm32"))]
fn language_override(tag: &str) -> Option<Lang> {
    let tag = tag.trim();
    if cfg!(target_os = "windows") && matches!(candidates(tag).first().map(String::as_str), Some("c" | "posix")) {
        return None;
    }
    lang_from_tag(tag)
}

/// The Windows display languages in preference order, one tag per line, queried once when Auto
/// first resolves. `sys-locale` asks Windows directly (`GetUserPreferredUILanguages`), so no
/// process is started.
#[cfg(target_os = "windows")]
fn windows_ui_language() -> Option<String> {
    let tags: Vec<String> = sys_locale::get_locales().collect();
    (!tags.is_empty()).then(|| tags.join("\n"))
}

/// The first supported language in a macOS `defaults read` list or Windows UI-culture output.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn first_supported(list: &str) -> Option<Lang> {
    list.split(['(', ')', ',', '"', '\n']).map(str::trim).filter(|s| !s.is_empty()).find_map(lang_from_tag)
}

#[cfg(target_arch = "wasm32")]
fn detect_system_lang() -> Lang {
    Lang::EN
}

thread_local! {
    // Per thread, as in PhotoCraft: independent app and test threads (kittest harnesses run in
    // parallel) must not change each other's drawing language.
    static CURRENT: Cell<Lang> = const { Cell::new(Lang::EN) };
}

/// Set the UI language for drawing (the shell calls this once per frame from the preference), so
/// widgets can translate without every call site carrying a language around.
pub fn set_current(lang: Lang) {
    CURRENT.set(lang);
}

/// The language the UI is drawn in.
/// `label` as it reads inside a sentence ("add a text box"): lowercased, except in languages
/// that capitalise nouns (German), where lowercasing would misspell it.
pub fn in_sentence(label: &str) -> std::borrow::Cow<'_, str> {
    if current().code() == "de" { std::borrow::Cow::Borrowed(label) } else { std::borrow::Cow::Owned(label.to_lowercase()) }
}

pub fn current() -> Lang {
    CURRENT.get()
}

/// Does `lang` have a catalog entry for this plain string? (English never does: it is the source.)
pub fn has(lang: Lang, s: &str) -> bool {
    lang.catalog().plain(s).is_some()
}

/// Translate an English UI string into the current language ([`tr`] with [`current`]).
pub fn t(s: &str) -> &str {
    tr(current(), s)
}

/// The name of a keyboard key, distinct from a command such as Delete. Missing key-name
/// translations use the English key name, never a plain command translation.
pub fn key_name(s: &str) -> &str {
    current().catalog().contextual("key", s).unwrap_or(s)
}

/// Translate an English UI string; unknown strings come back unchanged.
pub fn tr(lang: Lang, s: &str) -> &str {
    lang.catalog().plain(s).unwrap_or(s)
}

/// How the layers below say their own sentences in the current language.
///
/// The checker, the preflight profiles and the signature panel hand over a whole English wording
/// — holes and all — and the document's own words apart from it, so this is all they need of us:
/// `f.said(&i18n::words)` says the sentence here, and `in_english` says it in English. An unknown
/// wording comes back as it stands, which is the English, so a catalog that is missing one is a
/// sentence in English rather than a gap.
pub fn words(wording: &str) -> String {
    t(wording).to_owned()
}

/// Like [`tr`], for an English string that needs a disambiguating `context`.
pub fn tr_ctx<'a>(lang: Lang, context: &str, s: &'a str) -> &'a str {
    lang.catalog().contextual(context, s).unwrap_or_else(|| tr(lang, s))
}

/// A string keyed by its command id, falling back to the translation of the English `label`.
pub fn tr_id<'a>(lang: Lang, id: &str, label: &'a str) -> &'a str {
    lang.catalog().id(id).unwrap_or_else(|| tr(lang, label))
}

/// Fill `{name}` placeholders in one pass. Unknown placeholders are left as written, and inserted
/// values are never read as templates again, so a file name containing `{n}` stays intact.
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after_open)) = rest.split_once('{') {
        out.push_str(before);
        let Some((name, after_close)) = after_open.split_once('}') else {
            out.push('{');
            out.push_str(after_open);
            return out;
        };
        match args.iter().find(|(key, _)| *key == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = after_close;
    }
    out.push_str(rest);
    out
}

/// A generated history label (the action part of "Undo …") in the current language. Captured
/// values (file and field names) are inserted as they are; unknown labels are looked up whole.
pub fn action_label(text: &str) -> String {
    for (prefix, template) in [
        ("Insert pages from ", "Insert pages from {name}"),
        ("Fill in ", "Fill in {name}"),
        ("Set the image of ", "Set the image of {name}"),
        ("Edit script of ", "Edit script of {name}"),
        ("Import ", "Import {name}"),
    ] {
        if let Some(value) = text.strip_prefix(prefix) {
            return fmt(t(template), &[("name", value)]);
        }
    }
    if let Some(key) = text.strip_prefix("Change ")
        && ["Title", "Author", "Subject", "Keywords", "Creator", "Producer"].contains(&key)
    {
        return fmt(t("Change {key}"), &[("key", t(key))]);
    }
    if let Some(n) = text.strip_prefix("Delete ").and_then(|rest| rest.strip_suffix(" fields")).and_then(|n| n.parse::<u64>().ok()) {
        return trn(current(), n, "Delete {n} field", "Delete {n} fields");
    }
    t(text).to_owned()
}

/// A history label in the current language, out of the engine's own two parts: the wording is
/// looked up whole and the document's own word goes into its `{}` afterwards, so a file or field
/// name is never read as part of the wording. This is exact where [`action_label`] has to guess
/// from the English text, and is what the engine's own labels go through.
pub fn edit_label(label: &pdfcraft_engine::Label) -> String {
    // A batch's wording is only the hole: it carries its name as text, which from below L7 is one
    // of `BATCH_LABELS` and so is looked up too.
    if let Some(name) = label.value.as_deref().filter(|_| label.wording == "{batch}") {
        return t(name).to_owned();
    }
    label.said(t(label.wording))
}

/// A command label such as "Undo Insert pages from a.pdf" in the current language. A catalog
/// whose word order doesn't put the verb first translates "Undo {action}" and "Redo {action}"
/// as whole phrases; otherwise the translated verb goes before the action.
pub fn command_label(text: &str) -> String {
    for (prefix, template) in [("Undo", "Undo {action}"), ("Redo", "Redo {action}")] {
        if let Some(action) = text.strip_prefix(prefix).and_then(|tail| tail.strip_prefix(' ')) {
            let action = action_label(action);
            if has(current(), template) {
                return fmt(t(template), &[("action", &action)]);
            }
            return format!("{} {action}", t(prefix));
        }
    }
    t(text).to_owned()
}

/// A registered command's menu label: its `@id` catalog entry if there is one, otherwise
/// [`command_label`] of the current English label.
pub fn menu_label(id: &str, label: &str) -> String {
    match current().catalog().id(id) {
        Some(translated) => translated.to_owned(),
        None => command_label(label),
    }
}

/// A plural-aware message: `one`/`other` are the English forms (with `{n}` where the count goes).
pub fn trn(lang: Lang, n: u64, one: &str, other: &str) -> String {
    let idx = (lang.0.plural)(n);
    let text = lang.catalog().plural(one, other, idx).unwrap_or(if n == 1 { one } else { other });
    fmt(text, &[("n", &n.to_string())])
}

#[cfg(test)]
mod tests {
    use super::catalog::{Entry, parse_entries, placeholders};
    use super::*;

    const JA: fn() -> Lang = || Lang::from_code("ja").expect("ja registered");
    // These physical key-cap names remain English in several keyboard layouts. Only their
    // dedicated key context may use them unchanged; ordinary command labels still translate.
    const KEY_CAP_NAMES: &[&str] = &["Ctrl", "Shift", "Delete", "Esc", "Home", "End"];

    #[test]
    fn keyboard_key_names_have_a_separate_context_and_english_fallback() {
        for lang in Lang::all().filter(|l| *l != Lang::EN) {
            for key in ["Ctrl", "Shift", "Delete", "Esc", "Home", "End", "Space"] {
                assert!(lang.catalog().contextual("key", key).is_some(), "{}: {key}", lang.code());
            }
        }
        let previous = current();
        set_current(Lang::from_code("de").unwrap());
        assert_eq!(key_name("Delete"), "Entf");
        assert_eq!(t("Delete"), "Löschen");
        assert_eq!(key_name("Open"), "Open", "a missing key name must not borrow a command translation");
        set_current(Lang::EN);
        assert_eq!(key_name("Delete"), "Delete");
        set_current(previous);
    }

    #[test]
    fn tags_map_to_languages() {
        assert_eq!(lang_from_tag("ja_JP.UTF-8"), Some(JA()));
        assert_eq!(lang_from_tag("ja-JP"), Some(JA()));
        assert_eq!(lang_from_tag("en_US.UTF-8"), Some(Lang::EN));
        assert_eq!(lang_from_tag("C"), Some(Lang::EN));
        assert_eq!(lang_from_tag("POSIX"), Some(Lang::EN));
        assert_eq!(lang_from_tag("fr_FR"), Lang::from_code("fr"));
        assert_eq!(lang_from_tag("fr_CA.UTF-8"), Lang::from_code("fr"));
        assert_eq!(lang_from_tag("fr-BE"), Lang::from_code("fr"));
        assert_eq!(lang_from_tag("zh-TW"), Lang::from_code("zh-hant"));
        assert_eq!(lang_from_tag("zh_CN.UTF-8"), Lang::from_code("zh-hans"));
        assert_eq!(lang_from_tag("zh"), Lang::from_code("zh-hans"));
        assert_eq!(lang_from_tag("zh_HK.UTF-8"), Lang::from_code("zh-hant"));
        assert_eq!(lang_from_tag("zh-Hant-MO"), Lang::from_code("zh-hant"));
        assert_eq!(lang_from_tag(""), None);
        assert_eq!(lang_from_tag("_"), None);
    }

    #[test]
    fn candidates_walk_from_specific_to_general() {
        assert_eq!(candidates("pt_BR.UTF-8"), ["pt-br", "pt"]);
        assert_eq!(candidates("pt_PT.UTF-8"), ["pt-pt", "pt-br", "pt"]);
        assert_eq!(candidates("pt"), ["pt-br", "pt"]);
        assert_eq!(candidates("zh_TW"), ["zh-tw", "zh-hant", "zh"]);
        assert_eq!(candidates("zh-CN"), ["zh-cn", "zh-hans", "zh"]);
        assert_eq!(candidates("zh-Hant-HK"), ["zh-hant-hk", "zh-hant", "zh"]);
    }

    #[test]
    fn macos_language_list_is_parsed() {
        assert_eq!(first_supported("(\n    \"ja-JP\",\n    \"en-US\"\n)\n"), Some(JA()));
        assert_eq!(first_supported("(\n    \"fr-FR\",\n    \"en-US\"\n)\n"), Lang::from_code("fr"));
        assert_eq!(first_supported("("), None);
    }

    #[test]
    fn windows_display_language_resolves_chinese_scripts() {
        for tag in ["zh-CN", "zh-SG", "zh-Hans", "zh-Hans-CN"] {
            assert_eq!(first_supported(&format!("{tag}\r\n")), Lang::from_code("zh-hans"));
        }
        for tag in ["zh-TW", "zh-HK", "zh-MO", "zh-Hant", "zh-Hant-HK"] {
            assert_eq!(first_supported(&format!("{tag}\r\n")), Lang::from_code("zh-hant"));
        }
        assert_eq!(first_supported("en-US\r\n"), Some(Lang::EN));
        assert_eq!(first_supported("fr-FR\r\n"), Lang::from_code("fr"));
        assert_eq!(first_supported("\r\n"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn neutral_locale_does_not_override_display_language() {
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
        assert_eq!(Lang::from_pref("ja"), JA());
        assert_eq!(Lang::from_pref("JA"), JA());
        assert_eq!(Lang::from_pref("en"), Lang::EN);
        // `auto` and unknown codes follow the system (English under test).
        assert_eq!(Lang::from_pref(AUTO), Lang::EN);
        assert_eq!(Lang::from_pref("xx-unknown"), Lang::EN);
        assert_eq!(normalize_pref("JA"), Some("ja"));
        assert_eq!(normalize_pref("Auto"), Some(AUTO));
        assert_eq!(normalize_pref("xx"), None);
    }

    #[test]
    fn lookups_fall_back_to_english() {
        assert_eq!(tr(JA(), "no such label"), "no such label");
        assert_eq!(tr(Lang::EN, "File"), "File");
        assert_eq!(tr(JA(), "File"), "ファイル");
        assert_eq!(tr(JA(), "日本語の文書.pdf"), "日本語の文書.pdf");
        assert_eq!(tr_id(JA(), "no.such.id", "File"), "ファイル");
        assert_eq!(tr_ctx(JA(), "no such context", "File"), "ファイル");
        assert!(has(JA(), "Save") && !has(Lang::EN, "Save"));
    }

    #[test]
    fn catalog_kinds_are_parsed_and_looked_up() {
        let (c, errors) = Catalog::parse(
            "# c\n\tHello\tこんにちは\n@id\tfile.save\t保存する\nmenu\tWindows\tウィンドウ群\n@plural\t{n} file|{n} files\t{n} 個\n\n",
            1,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(c.plain("Hello"), Some("こんにちは"));
        assert_eq!(c.id("file.save"), Some("保存する"));
        assert_eq!(c.contextual("menu", "Windows"), Some("ウィンドウ群"));
        assert_eq!(c.contextual("other", "Windows"), None);
        assert_eq!(c.plural("{n} file", "{n} files", 0), Some("{n} 個"));
        assert_eq!(c.plural("{n} file", "{n} files", 5), Some("{n} 個"), "an index past the forms clamps");
    }

    #[test]
    fn malformed_lines_are_reported_not_fatal() {
        let (entries, errors) = parse_entries("\tok\tはい\nno tabs here\n\tonly\n\ta\tb\tc\textra\n\t\tempty source\n", 1);
        assert_eq!(entries.len(), 1);
        assert_eq!(errors.len(), 4, "{errors:?}");
        let (entries, errors) = parse_entries("\ta\\tb\tx\\ny\\\\z\n", 1);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(entries, [Entry { context: String::new(), source: "a\tb".into(), translation: "x\ny\\z".into() }]);
    }

    #[test]
    fn strict_validation_skips_only_the_bad_line() {
        for (text, forms, error) in [
            ("\tFile", 1, "context<TAB>"),
            ("\tFile\t", 1, "nonempty"),
            ("\tFile\\q\tFiles", 1, "unknown escape"),
            ("\tFile\tFiles\\", 1, "trailing backslash"),
            ("\tOpen…\tOpen", 1, "ellipsis"),
            ("\tOpen {file}\tOpen {name}", 1, "placeholders"),
            ("@plural\tpage\tPages", 1, "one|other"),
            ("@plural\t{n} page|{n} pages\t{n} pages", 2, "2 nonempty"),
            ("@plural\t{n} page|{n} pages\tPages", 1, "placeholders"),
            ("@unknown\tFile\tFiles", 1, "reserved context"),
        ] {
            // The bad line comes first; the good line after it still loads.
            let (catalog, errors) = Catalog::parse(&format!("{text}\n\tSave\t保存\n"), forms);
            assert_eq!(errors.len(), 1, "{text:?}: {errors:?}");
            assert!(errors[0].starts_with("line 1: ") && errors[0].contains(error), "{errors:?}");
            assert_eq!(catalog.plain("Save"), Some("保存"), "{text:?}");
        }
        let (catalog, errors) = Catalog::parse("\tFile\tファイル\n\tFile\t別\n", 1);
        assert!(errors.len() == 1 && errors[0].contains("line 2: duplicate"), "{errors:?}");
        assert_eq!(catalog.plain("File"), Some("ファイル"), "the first entry wins");
    }

    #[test]
    fn catalogs_handle_escapes_crlf_and_distinct_contexts() {
        let (catalog, errors) =
            Catalog::parse("# comment\r\n\r\n\tLine\\nTab\\tPath\\\\\tOther\\nTab\\tPath\\\\\r\nweight\tLight\tThin\r\ntheme\tLight\tPale\r\n", 1);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(catalog.plain("Line\nTab\tPath\\"), Some("Other\nTab\tPath\\"));
        assert_eq!(catalog.contextual("weight", "Light"), Some("Thin"));
        assert_eq!(catalog.contextual("theme", "Light"), Some("Pale"));
    }

    #[test]
    fn broken_catalog_falls_back_to_english() {
        static BROKEN: LangInfo = LangInfo {
            code: "broken",
            name: "Broken test catalog",
            source: "malformed\n\\",
            plural: plural_none,
            rtl: false,
            catalog: OnceLock::new(),
        };
        assert_eq!(tr(Lang(&BROKEN), "File"), "File");
        assert_eq!(trn(Lang(&BROKEN), 2, "{n} page", "{n} pages"), "2 pages");
    }

    #[test]
    fn contextual_id_and_plural_lookups_have_english_fallbacks() {
        static TEST: LangInfo = LangInfo {
            code: "test",
            name: "Test catalog",
            source: "\tLight\tPlain light\nweight\tLight\tThin\n@id\tfile.open\tOpen dialog…\n@plural\t{n} page|{n} pages\tOne page: {n}|Many pages: {n}\n",
            plural: plural_one_other,
            rtl: false,
            catalog: OnceLock::new(),
        };
        let l = Lang(&TEST);
        assert_eq!(tr_ctx(l, "weight", "Light"), "Thin");
        assert_eq!(tr_ctx(l, "theme", "Light"), "Plain light");
        assert_eq!(tr_ctx(l, "theme", "Unknown"), "Unknown");
        assert_eq!(tr_id(l, "file.open", "Reworded English…"), "Open dialog…");
        assert_eq!(tr_id(l, "unknown", "Light"), "Plain light");
        assert_eq!(tr_id(l, "unknown", "Unknown"), "Unknown");
        assert_eq!(trn(l, 1, "{n} page", "{n} pages"), "One page: 1");
        assert_eq!(trn(l, 0, "{n} page", "{n} pages"), "Many pages: 0");
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

    /// #103: the missing-OCR-models messages are translated under their current English text
    /// (catalogs once keyed an older `PRINTCRAFT_MODELS` wording, which never matched).
    #[test]
    fn missing_ocr_models_messages_are_translated() {
        let error = pdfcraft_engine::ocr::OcrError::NoModels.to_string();
        let notice = "Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them.";
        assert!(include_str!("../ocr_ui.rs").contains(notice), "keep in step with ocr_ui.rs");
        for code in ["bg", "cs", "de", "es", "fr", "hu", "ja", "ru", "te", "uk", "zh-hans", "zh-hant"] {
            let lang = Lang::from_code(code).expect("registered");
            assert!(has(lang, notice) && has(lang, &error), "{code}");
        }
        for lang in &LANGUAGES {
            assert!(!lang.source.contains("PRINTCRAFT"), "{}", lang.code);
        }
    }

    /// Japanese translates every registered command and every All tools group, section and item.
    #[test]
    fn japanese_covers_commands_and_catalogue() {
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(JA(), command.label), "missing command: {}", command.label);
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(JA(), group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(JA(), section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(JA(), item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Every `tl!("…")` literal in the UI source (test modules aside), unescaped.
    /// Every label Shared Field Properties shows, including those it passes through a variable
    /// (flag labels, errors, the font label), exists in each full catalog.
    #[test]
    fn bulk_field_properties_labels_are_in_every_full_catalog() {
        let source = include_str!("../bulk_fields.rs");
        // The string literal starting right after `at`, unescaped.
        fn literal(rest: &str) -> Option<String> {
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => return Some(out),
                    '\\' => out.push(chars.next()?),
                    c => out.push(c),
                }
            }
            None
        }
        let mut labels = std::collections::BTreeSet::new();
        for marker in ["tl!(\"", "Err(\"", "ok_or(\"", "|| \"", "MISSING: &str = \"", "i18n::t(\"", "_ => \""] {
            for (at, _) in source.match_indices(marker) {
                labels.extend(source.get(at + marker.len()..).and_then(literal));
            }
        }
        // Flag labels: `(field_flags::NAME, "Label", inverted)`.
        for (at, _) in source.match_indices("(field_flags::") {
            let line = source.get(at..).and_then(|s| s.lines().next()).unwrap_or_default();
            if let Some(q) = line.find(", \"") {
                labels.extend(line.get(q + 3..).and_then(literal));
            }
        }
        labels.retain(|l| l.chars().any(char::is_alphabetic) && !l.contains("credits") && !l.starts_with("bulk"));
        for label in ["Mixed", "Custom font", "The document is no longer open."] {
            labels.insert(label.to_string());
        }
        assert!(labels.len() > 20 && labels.contains("Comb of characters"), "the scan found the dialog's labels: {labels:?}");
        for code in ["bg", "cs", "de", "es", "fr", "hu", "it", "ja", "pt-br", "ru", "te", "tr", "uk", "zh-hans", "zh-hant"] {
            let lang = Lang::from_code(code).expect("registered");
            let missing: Vec<_> = labels.iter().filter(|l| !has(lang, l)).collect();
            assert!(missing.is_empty(), "{code} lacks Shared Field Properties labels: {missing:#?}");
        }
    }

    fn ui_literals() -> std::collections::BTreeSet<String> {
        scan_ui_literals(false)
    }

    // Lithuanian also checks direct lookup calls and multiline literal arguments.
    fn scan_ui_literals(expanded: bool) -> std::collections::BTreeSet<String> {
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
                    let source = if expanded { source.replace("crate::i18n::t(", "tl!(") } else { source };
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once(if expanded { "tl!(" } else { "tl!(\"" }) {
                        let after = if expanded {
                            let after = after.trim_start();
                            let Some(after) = after.strip_prefix('"') else {
                                rest = after;
                                continue;
                            };
                            after
                        } else {
                            after
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
                        if tail.starts_with("\")") || (expanded && literal_call) {
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

    /// Partial catalogs also translate every label of the absolute local GoToR consent prompt.
    #[test]
    fn local_gotor_confirmation_is_translated_in_all_supported_languages() {
        for language in LANGUAGES.iter().filter(|language| !language.source.is_empty()) {
            let lang = Lang::from_code(language.code).expect("registered language");
            for label in [
                "Open this local PDF?",
                "This document is trying to open a local file:",
                "Only allow this if you trust the source document.",
                "Allow",
                "Cancel",
            ] {
                assert!(has(lang, label), "{} lacks GoToR confirmation label: {label}", language.code);
                assert_ne!(tr(lang, label), label, "{} falls back to English for {label}", language.code);
            }
        }
    }

    /// Every catalog says the labels of a visible signature, which are looked up by the sign
    /// crate's own English words (so the `tl!` scan can't see them), and the note shown when they
    /// fall back to English. Each label that runs into its value keeps a separator at its end.
    #[test]
    fn signature_appearance_labels_are_in_every_catalog() {
        let english = pdfcraft_engine::sign::AppearanceLabels::default();
        let note = "The labels are in English: the signature's font can't draw them in this language.";
        for language in LANGUAGES.iter().filter(|language| !language.source.is_empty()) {
            let lang = Lang::from_code(language.code).expect("registered language");
            assert!(has(lang, note), "{} lacks the labels note", language.code);
            for (field, label) in english.fields() {
                assert!(has(lang, label), "{} lacks the signature label {field}: {label:?}", language.code);
                let said = tr(lang, label);
                if field != "signed_by" && !lang.rtl() {
                    assert!(said.ends_with(' ') || said.ends_with('：'), "{}: the {field} label runs into its value: {said:?}", language.code);
                }
            }
        }
    }

    /// Japanese translates every `tl!("…")` literal in the UI source, so a new string can't ship in
    /// English by accident (the same check French, German, Russian and Simplified Chinese have).
    #[test]
    fn japanese_covers_ui_literals() {
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(JA(), label)).collect();
        assert!(missing.is_empty(), "untranslated Japanese UI literals: {missing:#?}");
    }

    /// Italian translates every registered command and every All tools group, section and item.
    #[test]
    fn italian_covers_commands_and_catalogue() {
        let it = Lang::from_code("it").expect("it registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(it, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(it, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(it, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(it, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(it, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English in Italian.
    #[test]
    fn italian_covers_ui_literals() {
        let it = Lang::from_code("it").expect("it registered");
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(it, label)).collect();
        assert!(missing.is_empty(), "untranslated Italian UI literals: {missing:#?}");
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
        let (entries, _) = parse_entries(JA().0.source, 1);
        let labels: String = entries.iter().flat_map(|e| e.translation.chars()).chain(JA().name().chars()).filter(|c| !c.is_control()).collect();
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
        set_current(JA());
        assert_eq!(action_label("Insert pages from Save {e}.pdf"), "Save {e}.pdf からページを挿入");
        assert_eq!(command_label("Undo Insert pages from Save {e}.pdf"), "取り消し Save {e}.pdf からページを挿入");
        assert_eq!(action_label("Untranslated custom action"), "Untranslated custom action");
        assert_eq!(menu_label("file.open", "Open…"), "開く…");
        set_current(Lang::EN);
        assert_eq!(command_label("Undo Fill in Save {e}"), "Undo Fill in Save {e}");
    }

    #[test]
    fn current_language_is_per_thread() {
        set_current(JA());
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
        // No `@plural` entry yet: Japanese falls back to the English forms.
        assert_eq!(trn(JA(), 7, "{n} item", "{n} items"), "7 items");
        assert_eq!(fmt("{b} before {a}", &[("a", "x"), ("b", "y"), ("c", "z")]), "y before x");
        assert_eq!(fmt("{missing}", &[]), "{missing}");
        assert_eq!(placeholders("a {x} b {y} {"), ["x", "y"]);
    }

    #[test]
    fn language_persists_and_invalid_input_keeps_current_language() {
        let mut app = crate::PdfCraftApp::default();
        assert_eq!(app.language, AUTO);
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
        // Settings written before the system-locale default (no key) follow the system.
        let mut legacy = crate::PdfCraftApp::default();
        legacy.restore("{}");
        assert_eq!(legacy.language, AUTO);
    }

    #[test]
    fn brazilian_portuguese_is_registered() {
        let pt = Lang::from_code("pt-br").expect("pt-br registered");
        assert_eq!(pt.name(), "Português (Brasil)");
        assert_eq!(Lang::from_code("PT-BR"), Some(pt));
        assert_eq!(normalize_pref("pt-BR"), Some("pt-br"));
        assert_eq!(normalize_pref("pt"), None, "only exact codes are preferences");
        assert_eq!(lang_from_tag("pt_BR.UTF-8"), Some(pt));
        assert_eq!(lang_from_tag("pt_PT"), Some(pt));
        assert_eq!(tr(pt, "File"), "Arquivo");
        assert_eq!(tr(pt, "Save as…"), "Salvar como…");
        assert_eq!(tr(pt, "Arquivo do usuário.pdf"), "Arquivo do usuário.pdf");
        assert_eq!((0..=3).map(|n| (pt.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 1, 1]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "pt-br").unwrap();
        assert_eq!(app.language, "pt-br");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "pt-br");
    }

    /// Brazilian Portuguese translates every registered command and every All tools group, section and item.
    #[test]
    fn brazilian_portuguese_covers_commands_and_catalogue() {
        let pt = Lang::from_code("pt-br").expect("pt-br registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(pt, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(pt, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(pt, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(pt, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(pt, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Check direct lookups and multiline literals as well as ordinary tl!("literal") calls,
    /// following the complete-catalog check for Simplified Chinese.
    #[test]
    fn brazilian_portuguese_covers_ui_literals() {
        let pt = Lang::from_code("pt-br").expect("pt-br registered");
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
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n").replace("crate::i18n::t(", "tl!(");
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
                        let closing = tail.strip_prefix('"').expect("closing quote").trim_start();
                        let closing = closing.strip_prefix(',').unwrap_or(closing).trim_start();
                        if closing.starts_with(')') {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(pt, label)).collect();
        assert!(missing.is_empty(), "untranslated Brazilian Portuguese UI literals: {missing:#?}");
    }

    #[test]
    fn spanish_is_registered() {
        let es = Lang::from_code("es").expect("es registered");
        assert_eq!(es.name(), "Español");
        assert_eq!(normalize_pref("ES"), Some("es"));
        assert_eq!(lang_from_tag("es_ES.UTF-8"), Some(es));
        assert_eq!(lang_from_tag("es-MX"), Some(es));
        assert_eq!(first_supported("es-ES\r\nen-US"), Some(es));
        assert_eq!(tr(es, "File"), "Archivo");
        assert_eq!(tr(es, "Save as…"), "Guardar como…");
        assert_eq!(tr(es, "Informe del usuario.pdf"), "Informe del usuario.pdf");
        assert_eq!(trn(es, 1, "{n} page", "{n} pages"), "1 página");
        assert_eq!(trn(es, 3, "{n} page", "{n} pages"), "3 páginas");
        // The signature pad's Type is the verb (type your name), not the noun "Tipo".
        assert_eq!(tr_ctx(es, "signature pad", "Type"), "Escribir");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "es").unwrap();
        assert_eq!(app.language, "es");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "es");
    }

    /// Spanish translates every registered command and every All tools group, section and item.
    #[test]
    fn spanish_covers_commands_and_catalogue() {
        let es = Lang::from_code("es").expect("es registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(es, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(es, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(es, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(es, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(es, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English in Spanish.
    #[test]
    fn spanish_covers_ui_literals() {
        let es = Lang::from_code("es").expect("es registered");
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(es, label)).collect();
        assert!(missing.is_empty(), "untranslated Spanish UI literals: {missing:#?}");
    }

    /// Spanish says everything the layers below word for themselves: what the accessibility
    /// checker finds and what its saved report calls things, what the PDF/A verifier reports, what
    /// a signature's details say and the changes they name, what can be wrong with the pages to
    /// print, and what an edit is called in the history.
    ///
    /// These lists are the whole point of the engine handing the wording over apart from the
    /// document's own words: a catalog that covers them says those sentences in its own language.
    /// A sentence missing here is a sentence in English on a Spanish screen.
    #[test]
    fn spanish_covers_what_the_engine_says() {
        let es = Lang::from_code("es").expect("es registered");
        let lists: [(&str, &[&str]); 8] = [
            ("accessibility finding", pdfcraft_engine::a11y::WORDINGS),
            ("accessibility report", pdfcraft_engine::a11y::REPORT_WORDS),
            ("PDF/A problem", pdfcraft_engine::pdfa::WORDINGS),
            ("signature detail", pdfcraft_engine::sign::pdf::WORDINGS),
            ("change after signing", pdfcraft_engine::sign::pdf::kind::ALL),
            ("pages to print", pdfcraft_engine::print::WORDINGS),
            ("edit label", pdfcraft_engine::EDIT_LABELS),
            ("batch label", pdfcraft_engine::BATCH_LABELS),
        ];
        for (what, list) in lists {
            assert!(!list.is_empty(), "no {what} wordings to cover");
            for wording in list {
                assert!(has(es, wording), "missing {what}: {wording}");
            }
        }
    }

    /// Each of those sentences keeps its holes when it is said in Spanish, so the document's own
    /// words still have somewhere to go: a translation that drops `{p}` or `{}` would read as a
    /// sentence about nothing in particular.
    #[test]
    fn spanish_keeps_the_holes_in_what_the_engine_says() {
        let es = Lang::from_code("es").expect("es registered");
        let lists: [&[&str]; 4] = [
            pdfcraft_engine::a11y::WORDINGS,
            pdfcraft_engine::pdfa::WORDINGS,
            pdfcraft_engine::sign::pdf::WORDINGS,
            pdfcraft_engine::print::WORDINGS,
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
        let te = Lang::from_code("te").expect("te registered");
        assert_eq!(te.name(), "తెలుగు");
        assert_eq!(normalize_pref("TE"), Some("te"));
        assert_eq!(lang_from_tag("te_IN.UTF-8"), Some(te));
        assert_eq!(first_supported("te-IN\r\nen-US"), Some(te));
        assert_eq!(tr(te, "File"), "ఫైల్");
        assert_eq!(tr(te, "Save as…"), "వేరే పేరుతో సేవ్ చేయి…");
        assert_eq!(tr(te, "నివేదిక.pdf"), "నివేదిక.pdf");
        assert_eq!(trn(te, 1, "{n} page", "{n} pages"), "1 పేజీ");
        assert_eq!(trn(te, 3, "{n} page", "{n} pages"), "3 పేజీలు");
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
        let te = Lang::from_code("te").expect("te registered");
        assert_eq!(tr_ctx(te, "signature pad", "Type"), "టైప్ చేయి");
        assert_eq!(tr(te, "Type"), "రకం");
        // A context no catalog defines, so the fallback stays tested as catalogs fill their gaps.
        let es = Lang::from_code("es").expect("es registered");
        assert_eq!(tr_ctx(es, "no such context", "Type"), tr(es, "Type"));
    }

    /// Telugu translates every registered command and every All tools group, section and item.
    #[test]
    fn telugu_covers_commands_and_catalogue() {
        let te = Lang::from_code("te").expect("te registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(te, command.label), "missing command: {}", command.label);
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(te, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(te, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(te, item.label), "missing item: {}", item.label);
                }
            }
        }
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
        let te = Lang::from_code("te").expect("te registered");
        let (entries, _) = parse_entries(te.0.source, te.0.plural_forms());
        let labels: String = entries.iter().flat_map(|e| e.translation.chars()).chain(te.name().chars()).filter(|c| !c.is_control()).collect();
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

    /// Czech covers every registered menu title and menu command label (#68), keeps ellipses, and
    /// only leaves untranslated what is deliberately the same in Czech.
    #[test]
    fn czech_covers_every_menu_label() {
        let cs = Lang::from_code("cs").expect("cs registered");
        assert_eq!(cs.name(), "Čeština");
        assert_eq!(tr(cs, "File"), "Soubor");
        assert_eq!(tr(cs, "Žluťoučký kůň.pdf"), "Žluťoučký kůň.pdf");
        assert_eq!((0..=6).map(|n| (cs.0.plural)(n)).collect::<Vec<_>>(), [2, 0, 1, 1, 1, 2, 2]);
        for spec in pdfcraft_engine::commands::COMMANDS {
            let Some(menu) = spec.menu else { continue };
            assert!(has(cs, menu), "Czech lacks the menu title {menu:?}");
            assert!(has(cs, spec.label), "Czech lacks the {menu} menu label {:?} ({})", spec.label, spec.id);
        }
        // Spelled the same in Czech: product and format names, abbreviations, key caps, words
        // Czech shares with English, and templates with no English words.
        const KEEP_AS_IS: &[&str] = &[
            "OK",
            "PDF/A…",
            "Microsoft Word (.docx)",
            "PostScript / EPS",
            "Home / End",
            "JavaScript",
            "ZIP",
            "Text",
            "Index",
            "Metadata",
            "Model",
            "{field}: {e}",
            "{0}: {1}",
            "{month} {y}",
            "+LOC",
            "−LOC",
            "ΔLOC",
            "+Bin",
            "−Bin",
            // The distinguished name of a certificate, abbreviated as in X.500.
            "DN: ",
        ];
        let (entries, errors) = parse_entries(cs.0.source, cs.0.plural_forms());
        assert!(errors.is_empty(), "{errors:?}");
        // The code joins some labels with a space at either end: keep exactly the source's.
        let edges = |s: &str| (s.len() - s.trim_start().len(), s.len() - s.trim_end().len());
        for e in &entries {
            let key_cap = e.context == "key" && KEY_CAP_NAMES.contains(&e.source.as_str());
            assert!(e.source != e.translation || KEEP_AS_IS.contains(&e.source.as_str()) || key_cap, "{:?} is untranslated", e.source);
            assert!(!e.translation.contains("..."), "use … rather than three dots: {:?}", e.translation);
            assert_eq!(edges(&e.translation), edges(&e.source), "stray whitespace: {:?}", e.translation);
        }
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "cs").unwrap();
        assert!(app.set_option("language", "cz").is_err());
        assert_eq!(app.language, "cs");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "cs");
    }

    /// Czech translates every registered command, menu and All tools label.
    #[test]
    fn czech_covers_commands_and_catalogue() {
        let cs = Lang::from_code("cs").expect("cs registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(cs, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(cs, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(cs, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(cs, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(cs, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Czech translates every `tl!("…")` literal in the UI source.
    #[test]
    fn czech_covers_ui_literals() {
        let cs = Lang::from_code("cs").expect("cs registered");
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(cs, label)).collect();
        assert!(missing.is_empty(), "untranslated Czech UI literals: {missing:#?}");
    }

    /// Czech counts take three forms (1; 2–4; 0 and 5+), dates use the genitive month after the
    /// day, and history labels keep captured names.
    #[test]
    fn czech_plurals_dates_and_history_labels() {
        let cs = Lang::from_code("cs").expect("cs registered");
        assert_eq!(trn(cs, 1, "{n} page", "{n} pages"), "1 stránka");
        assert_eq!(trn(cs, 3, "{n} page", "{n} pages"), "3 stránky");
        assert_eq!(trn(cs, 5, "{n} page", "{n} pages"), "5 stránek");
        assert_eq!(trn(cs, 0, "{n} field", "{n} fields"), "0 polí");
        assert_eq!(trn(cs, 1, "{n} byte", "{n} bytes"), "1 bajt");
        assert_eq!(trn(cs, 512, "{n} byte", "{n} bytes"), "512 bajtů");
        assert_eq!(trn(cs, 3, "{n} image optimized", "{n} images optimized"), "3 obrázky optimalizovány");
        assert_eq!(tr(cs, "October"), "Říjen");
        let month = tr_ctx(cs, "calendar date", "October");
        assert_eq!(fmt(tr(cs, "{month} {day}, {y}"), &[("month", month), ("day", "9"), ("y", "2026")]), "9. října 2026");
        set_current(cs);
        assert_eq!(command_label("Undo Insert pages from Zpráva {n}.pdf"), "Zpět: Vložit stránky ze souboru Zpráva {n}.pdf");
        assert_eq!(command_label("Redo Fill in Kontakt {key}"), "Znovu: Vyplnit Kontakt {key}");
        assert_eq!(action_label("Change Title"), "Změnit: Název");
        assert_eq!(action_label("Delete 3 fields"), "Odstranit 3 pole");
        assert_eq!(action_label("Delete 12 fields"), "Odstranit 12 polí");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "Vybráno stránek: 3");
        set_current(Lang::EN);
    }

    /// Turkish covers every registered menu title and menu command label, keeps ellipses, and
    /// persists in preferences.
    #[test]
    fn turkish_covers_every_menu_label() {
        let tr_lang = Lang::from_code("tr").expect("tr registered");
        assert_eq!(tr_lang.name(), "Türkçe");
        assert_eq!(tr(tr_lang, "File"), "Dosya");
        assert_eq!(tr(tr_lang, "Türkçe Belge.pdf"), "Türkçe Belge.pdf");
        assert_eq!((0..=6).map(|n| (tr_lang.0.plural)(n)).collect::<Vec<_>>(), [0; 7]);
        for spec in pdfcraft_engine::commands::COMMANDS {
            let Some(menu) = spec.menu else { continue };
            assert!(has(tr_lang, menu), "Turkish lacks the menu title {menu:?}");
            assert!(has(tr_lang, spec.label), "Turkish lacks the {menu} menu label {:?} ({})", spec.label, spec.id);
        }
        const KEEP_AS_IS: &[&str] = &[
            "OK",
            "PDF/A…",
            "Microsoft Word (.docx)",
            "PostScript / EPS",
            "Preflight",
            "JavaScript",
            "ZIP",
            "{n} {kind}",
            "{rule} - {status}",
            "Poster",
            "{field}: {e}",
            "`{command}` {when}",
            "{0}: {1}",
            "{month} {y}",
            "+LOC",
            "−LOC",
            "ΔLOC",
            "+Bin",
            "−Bin",
            "Model",
            "Home / End",
            // The distinguished name of a certificate, abbreviated as in X.500.
            "DN: ",
        ];
        let (entries, errors) = parse_entries(tr_lang.0.source, tr_lang.0.plural_forms());
        assert!(errors.is_empty(), "Turkish catalog errors: {errors:?}");
        for e in &entries {
            assert!(
                e.source != e.translation
                    || KEEP_AS_IS.contains(&e.source.as_str())
                    || (e.context == "key" && KEY_CAP_NAMES.contains(&e.source.as_str())),
                "{:?} is untranslated",
                e.source
            );
            assert!(!e.translation.contains("..."), "use … rather than three dots: {:?}", e.translation);
            assert_eq!(e.source.starts_with(' '), e.translation.starts_with(' '), "leading space mismatch: {:?}", e.translation);
            assert_eq!(e.source.ends_with(' '), e.translation.ends_with(' '), "trailing space mismatch: {:?}", e.translation);
        }
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "tr").unwrap();
        assert!(app.set_option("language", "tu").is_err());
        assert_eq!(app.language, "tr");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "tr");
    }

    /// Turkish translates every registered command, menu and All tools label.
    #[test]
    fn turkish_covers_commands_and_catalogue() {
        let tr_lang = Lang::from_code("tr").expect("tr registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(tr_lang, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(tr_lang, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(tr_lang, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(tr_lang, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(tr_lang, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Turkish translates every `tl!("…")` literal in the UI source.
    #[test]
    fn turkish_covers_ui_literals() {
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let tr_lang = Lang::from_code("tr").expect("tr registered");
        let missing: Vec<_> = literals.iter().filter(|label| !has(tr_lang, label)).collect();
        assert!(missing.is_empty(), "untranslated Turkish UI literals: {missing:#?}");
    }

    #[test]
    fn simplified_chinese_is_registered() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        assert_eq!(zh.name(), "简体中文");
        assert_eq!(normalize_pref("zh-Hans"), Some("zh-hans"));
        assert_eq!(normalize_pref("zh"), None, "only exact codes are preferences");
        assert_eq!(tr(zh, "File"), "文件");
        assert_eq!(tr(zh, "报告.pdf"), "报告.pdf");
        for menu in ["File", "Edit", "Pages", "View", "Help", "Preferences…", "Interface language"] {
            assert!(has(zh, menu), "missing {menu}");
        }
    }

    /// Like PhotoCraft's complete-catalog gates, derive coverage from the UI, not another language.
    #[test]
    fn simplified_chinese_covers_commands_and_catalogue() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(zh, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(zh, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(zh, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(zh, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(zh, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    #[test]
    fn simplified_chinese_covers_about_credit_labels() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        for label in ["About", "Contributors", "Models"].into_iter().chain(crate::credits::MODEL_COLUMNS) {
            assert!(has(zh, label), "missing About label: {label}");
        }
        for mode in crate::credits::NameMode::ALL {
            assert!(has(zh, mode.label()), "missing name mode: {}", mode.label());
        }
        for key in crate::credits::SortKey::ALL {
            let (label, header) = key.label();
            for text in [label, header] {
                assert!(has(zh, text), "missing contributor sort/header: {text}");
            }
        }
    }

    /// Direct tl! and i18n::t labels must not silently fall back to English in the complete catalog.
    #[test]
    fn simplified_chinese_covers_ui_literals() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut literals = std::collections::BTreeSet::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).expect("UI source directory") {
                let path = entry.expect("UI source entry").path();
                if path.is_dir() {
                    // Catalog implementation and its test-only lookup examples are not UI labels.
                    if path.file_name().is_some_and(|name| name != "i18n") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n").replace("crate::i18n::t(", "tl!(");
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
                        let closing = tail.strip_prefix('"').expect("closing quote").trim_start();
                        let closing = closing.strip_prefix(',').unwrap_or(closing).trim_start();
                        if closing.starts_with(')') {
                            // Current UI literals use the shared Rust/JSON string escapes.
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(zh, label)).collect();
        assert!(missing.is_empty(), "untranslated Simplified Chinese UI literals: {missing:#?}");
    }

    #[test]
    fn simplified_chinese_history_and_diagnostics_preserve_user_values() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        set_current(zh);
        assert_eq!(command_label("Undo Insert pages from 报告 {n}.pdf"), "撤销 从 报告 {n}.pdf 插入页面");
        assert_eq!(command_label("Redo Fill in 联系人 {key}"), "重做 填写 联系人 {key}");
        assert_eq!(action_label("Change Title"), "更改标题");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]), "无法显示此页面。\nOS error {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "已选择 3 页");
        assert_eq!(tr(zh, "CheckBox"), "复选框");
        assert_eq!(tr(zh, "pages"), "页");
        assert_eq!(trn(zh, 1, "{n} page", "{n} pages"), "1 页");
        assert_eq!(trn(zh, 0, "{n} field", "{n} fields"), "0 个字段");
        assert_eq!(trn(zh, 2, "{n} field", "{n} fields"), "2 个字段");
        assert_eq!(fmt(t("{n} contributors · {c} commits"), &[("n", "2"), ("c", "1,234")]), "2 位贡献者 · 1,234 次提交");
        let contributor = crate::credits::Contributor {
            login: "reader{n}",
            display_name: Some("Save"),
            real_name: None,
            prs: 2,
            commits: 3,
            lines_added: 10,
            lines_deleted: 4,
            binary_added: 1,
            binary_deleted: 0,
            first_commit: "2026-10-01T00:00:00Z",
            last_commit: "2026-10-09T00:00:00Z",
        };
        assert_eq!(
            contributor.summary(),
            "@reader{n}：2 个 PR，3 次提交，新增 10 行 / 删除 4 行（净增 +6 行），新增 1 个 / 删除 0 个二进制资源，2026-10-01 – 2026-10-09"
        );
        assert_eq!(contributor.name(crate::credits::NameMode::DisplayName), "Save");
        set_current(Lang::EN);
    }

    #[test]
    fn traditional_chinese_is_registered() {
        let zh = Lang::from_code("zh-hant").expect("zh-hant registered");
        assert_eq!(zh.name(), "繁體中文");
        assert_eq!(normalize_pref("zh-Hant"), Some("zh-hant"));
        assert_eq!(tr(zh, "File"), "檔案");
        assert_eq!(tr(zh, "報告.pdf"), "報告.pdf");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(zh, command.label), "missing command: {}", command.label);
        }
    }

    /// Report statuses and rule descriptions must retain their meaning in Taiwan Chinese.
    #[test]
    fn traditional_chinese_accessibility_meaning() {
        use pdfcraft_engine::a11y::{Rule, Status};
        let zh = Lang::from_code("zh-hant").expect("zh-hant registered");
        assert_eq!(tr(zh, Status::Passed.label()), "通過");
        assert_eq!(tr(zh, Status::Failed.label()), "未通過");
        assert_eq!(tr(zh, Rule::Scripts.description()), "沒有妨礙無障礙操作的指令碼");
        assert_eq!(tr(zh, Rule::TableRows.name()), "列");
        assert_eq!(tr(zh, Rule::TableRows.explanation()), "表格列應直接位於表格或其表頭、主體、表尾內。請修正表格標記。");
        assert_eq!(tr(zh, Rule::TableRegularity.description()), "表格每列的欄數必須相同，每欄的列數也必須相同");
        assert_eq!(tr(zh, Rule::TableCells.explanation()), "表格儲存格應位於列內。請修正表格標記。");
        assert_eq!(tr(zh, Rule::TableHeaders.explanation()), "表頭儲存格（TH）讓聽者知道每欄或每列的內容。請標記表頭儲存格。");
    }

    /// A choice in a form is an item, not a project; field notices preserve user-supplied names.
    #[test]
    fn traditional_chinese_form_vocabulary() {
        let zh = Lang::from_code("zh-hant").expect("zh-hant registered");
        assert_eq!(tr(zh, "Item:"), "項目：");
        assert_eq!(tr(zh, "Item List"), "項目清單");
        assert_eq!(tr(zh, "Sort items"), "排序項目");
        assert_eq!(tr(zh, "Read Only"), "唯讀");
        assert_eq!(fmt(tr(zh, "{name} is read-only"), &[("name", "Field{n}")]), "Field{n} 為唯讀");
    }

    #[test]
    fn french_is_registered() {
        let fr = Lang::from_code("fr").expect("fr registered");
        assert_eq!(fr.name(), "Français");
        assert_eq!(normalize_pref("FR"), Some("fr"));
        assert_eq!(lang_from_tag("fr_FR.UTF-8"), Some(fr));
        assert_eq!(lang_from_tag("fr-CA"), Some(fr));
        assert_eq!(first_supported("fr-FR\r\nen-US"), Some(fr));
        assert_eq!(tr(fr, "File"), "Fichier");
        assert_eq!(tr(fr, "Save as…"), "Enregistrer sous…");
        assert_eq!(tr(fr, "Bookmarks"), "Signets");
        assert_eq!(tr(fr, "Layers"), "Calques");
        assert_eq!(tr(fr, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        assert_eq!((0..=3).map(|n| (fr.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 1, 1]);
        assert_eq!(trn(fr, 0, "{n} page", "{n} pages"), "0 page");
        assert_eq!(trn(fr, 1, "{n} page", "{n} pages"), "1 page");
        assert_eq!(trn(fr, 2, "{n} page", "{n} pages"), "2 pages");
        assert_eq!(trn(fr, 1, "{n} field", "{n} fields"), "1 champ");
        assert_eq!(trn(fr, 2, "{n} field", "{n} fields"), "2 champs");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "fr").unwrap();
        assert_eq!(app.language, "fr");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "fr");
    }

    /// French translates every registered command and every All tools group, section and item.
    #[test]
    fn french_covers_commands_and_catalogue() {
        let fr = Lang::from_code("fr").expect("fr registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(fr, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(fr, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(fr, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(fr, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(fr, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn french_covers_ui_literals() {
        let fr = Lang::from_code("fr").expect("fr registered");
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(fr, label)).collect();
        assert!(missing.is_empty(), "untranslated French UI literals: {missing:#?}");
    }

    #[test]
    fn french_history_and_diagnostics_preserve_user_values() {
        let fr = Lang::from_code("fr").expect("fr registered");
        set_current(fr);
        assert_eq!(command_label("Undo Insert pages from Rapport {n}.pdf"), "Annuler Insérer des pages depuis Rapport {n}.pdf");
        assert_eq!(command_label("Redo Fill in Contact {key}"), "Rétablir Remplir Contact {key}");
        assert_eq!(action_label("Change Title"), "Modifier Titre");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]), "Impossible d'afficher cette page.\nOS error {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "3 pages sélectionnées");
        assert_eq!(tr(fr, "CheckBox"), "Case à cocher");
        set_current(Lang::EN);
    }

    #[test]
    fn german_is_registered() {
        let de = Lang::from_code("de").expect("de registered");
        assert_eq!(de.name(), "Deutsch");
        assert_eq!(normalize_pref("DE"), Some("de"));
        assert_eq!(lang_from_tag("de_DE.UTF-8"), Some(de));
        assert_eq!(lang_from_tag("de-AT"), Some(de));
        assert_eq!(lang_from_tag("de_CH"), Some(de));
        assert_eq!(first_supported("de-DE\r\nen-US"), Some(de));
        assert_eq!(tr(de, "File"), "Datei");
        assert_eq!(tr(de, "Save as…"), "Speichern unter…");
        assert_eq!(tr(de, "Bookmarks"), "Lesezeichen");
        assert_eq!(tr(de, "Layers"), "Ebenen");
        assert_eq!(tr(de, "Bericht des Nutzers.pdf"), "Bericht des Nutzers.pdf");
        assert_eq!((0..=3).map(|n| (de.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        assert_eq!(trn(de, 0, "{n} page", "{n} pages"), "0 Seiten");
        assert_eq!(trn(de, 1, "{n} page", "{n} pages"), "1 Seite");
        assert_eq!(trn(de, 2, "{n} page", "{n} pages"), "2 Seiten");
        assert_eq!(trn(de, 1, "{n} field", "{n} fields"), "1 Feld");
        assert_eq!(trn(de, 2, "{n} field", "{n} fields"), "2 Felder");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "de").unwrap();
        assert_eq!(app.language, "de");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "de");
    }

    /// German translates every registered command and every All tools group, section and item.
    #[test]
    fn german_covers_commands_and_catalogue() {
        let de = Lang::from_code("de").expect("de registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(de, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(de, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(de, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(de, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(de, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn german_covers_ui_literals() {
        let de = Lang::from_code("de").expect("de registered");
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(de, label)).collect();
        assert!(missing.is_empty(), "untranslated German UI literals: {missing:#?}");
    }

    #[test]
    fn tool_names_inside_sentences_are_lowercased_except_in_german() {
        set_current(Lang::EN);
        assert_eq!(in_sentence("Text Box"), "text box");
        set_current(Lang::from_code("de").expect("de registered"));
        assert_eq!(in_sentence("Textfeld"), "Textfeld");
        set_current(Lang::EN);
    }

    #[test]
    fn german_certificate_subject_has_a_distinct_context() {
        let de = Lang::from_code("de").expect("de registered");
        assert_eq!(tr(de, "Subject"), "Thema");
        assert_eq!(tr_ctx(de, "certificate", "Subject"), "Zertifikatsinhaber");
        assert_eq!(tr_ctx(de, "certificate", "Issuer"), tr(de, "Issuer"));
        assert_eq!(tr_ctx(Lang::EN, "certificate", "Subject"), "Subject");
    }

    #[test]
    fn german_history_and_diagnostics_preserve_user_values() {
        let de = Lang::from_code("de").expect("de registered");
        set_current(de);
        assert_eq!(command_label("Undo Insert pages from Rapport {n}.pdf"), "Seiten aus Rapport {n}.pdf einfügen rückgängig machen");
        assert_eq!(command_label("Redo Fill in Contact {key}"), "Contact {key} ausfüllen wiederholen");
        assert_eq!(action_label("Change Title"), "Titel ändern");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(
            fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]),
            "Diese Seite konnte nicht angezeigt werden.\nOS error {n}"
        );
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "3 Seiten ausgewählt");
        assert_eq!(tr(de, "CheckBox"), "Kontrollkästchen");
        set_current(Lang::EN);
    }

    #[test]
    fn russian_is_registered() {
        let ru = Lang::from_code("ru").expect("ru registered");
        assert_eq!(ru.name(), "Русский");
        assert_eq!(normalize_pref("RU"), Some("ru"));
        assert_eq!(lang_from_tag("ru_RU.UTF-8"), Some(ru));
        assert_eq!(lang_from_tag("ru-BY"), Some(ru));
        assert_eq!(first_supported("ru-RU\r\nen-US"), Some(ru));
        assert_eq!(tr(ru, "File"), "Файл");
        assert_eq!(tr(ru, "Save as…"), "Сохранить как…");
        assert_eq!(tr(ru, "Bookmarks"), "Закладки");
        assert_eq!(tr(ru, "Layers"), "Слои");
        assert_eq!(tr(ru, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        // one (1), few (2-4), many (0, 5+), with 11-14 exception
        assert_eq!((0..=3).map(|n| (ru.0.plural)(n)).collect::<Vec<_>>(), [2, 0, 1, 1]);
        assert_eq!(trn(ru, 0, "{n} page", "{n} pages"), "0 страниц");
        assert_eq!(trn(ru, 1, "{n} page", "{n} pages"), "1 страница");
        assert_eq!(trn(ru, 2, "{n} page", "{n} pages"), "2 страницы");
        assert_eq!(trn(ru, 5, "{n} page", "{n} pages"), "5 страниц");
        assert_eq!(trn(ru, 11, "{n} page", "{n} pages"), "11 страниц");
        assert_eq!(trn(ru, 21, "{n} page", "{n} pages"), "21 страница");
        assert_eq!(trn(ru, 22, "{n} page", "{n} pages"), "22 страницы");
        assert_eq!(trn(ru, 1, "{n} field", "{n} fields"), "1 поле");
        assert_eq!(trn(ru, 2, "{n} field", "{n} fields"), "2 поля");
        assert_eq!(trn(ru, 5, "{n} field", "{n} fields"), "5 полей");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "ru").unwrap();
        assert_eq!(app.language, "ru");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "ru");
    }

    /// Russian translates every registered command and every All tools group, section and item.
    #[test]
    fn russian_covers_commands_and_catalogue() {
        let ru = Lang::from_code("ru").expect("ru registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(ru, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(ru, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(ru, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(ru, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(ru, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn russian_covers_ui_literals() {
        let ru = Lang::from_code("ru").expect("ru registered");
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(ru, label)).collect();
        assert!(missing.is_empty(), "untranslated Russian UI literals: {missing:#?}");
    }

    #[test]
    fn russian_history_and_diagnostics_preserve_user_values() {
        let ru = Lang::from_code("ru").expect("ru registered");
        set_current(ru);
        assert_eq!(command_label("Undo Insert pages from Rapport {n}.pdf"), "Отменить Вставить страницы из Rapport {n}.pdf");
        assert_eq!(command_label("Redo Fill in Contact {key}"), "Повторить Заполнить Contact {key}");
        assert_eq!(action_label("Change Title"), "Изменить заголовок");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]), "Не удалось отобразить эту страницу.\nOS error {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "Выбрано 3 страницы");
        assert_eq!(tr(ru, "CheckBox"), "Флажок");
        set_current(Lang::EN);
    }

    #[test]
    fn lithuanian_is_registered_and_persists() {
        let lt = Lang::from_code("lt").expect("lt registered");
        assert_eq!(lt.name(), "Lietuvių");
        assert_eq!(normalize_pref("LT"), Some("lt"));
        for tag in ["lt", "lt-LT", "lt_LT.UTF-8", "LT-lt"] {
            assert_eq!(lang_from_tag(tag), Some(lt), "{tag}");
        }
        assert_eq!(tr(lt, "File"), "Failas");
        assert_eq!(tr(lt, "Edit"), "Redaguoti");
        assert_eq!(tr(lt, "Save"), "Įrašyti");
        assert_eq!(tr(lt, "Interface language"), "Sąsajos kalba");
        assert_eq!(trn(lt, 1, "{n} page", "{n} pages"), "1 puslapis");
        assert_eq!(trn(lt, 2, "{n} page", "{n} pages"), "2 puslapiai");
        assert_eq!(trn(lt, 10, "{n} page", "{n} pages"), "10 puslapių");
        assert_eq!(trn(lt, 11, "{n} page", "{n} pages"), "11 puslapių");
        assert_eq!(trn(lt, 21, "{n} page", "{n} pages"), "21 puslapis");
        assert_eq!(trn(lt, 22, "{n} page", "{n} pages"), "22 puslapiai");
        assert_eq!(trn(lt, 12, "{n} object selected", "{n} objects selected"), "Pažymėta 12 objektų");
        assert_eq!(trn(lt, 22, "{n} object selected", "{n} objects selected"), "Pažymėti 22 objektai");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "LT").unwrap();
        assert_eq!(app.language, "lt");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "lt");
        assert!(restored.set_option("language", "xx").is_err());
    }

    #[test]
    fn lithuanian_covers_commands_and_catalogue() {
        let lt = Lang::from_code("lt").expect("lt registered");
        let mut missing = std::collections::BTreeSet::new();
        for command in pdfcraft_engine::commands::COMMANDS {
            if !has(lt, command.label) {
                missing.insert(command.label);
            }
            if let Some(menu) = command.menu
                && !has(lt, menu)
            {
                missing.insert(menu);
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            if !has(lt, group.label) {
                missing.insert(group.label);
            }
            for section in group.sections {
                if !has(lt, section.title) {
                    missing.insert(section.title);
                }
                for item in section.items {
                    if !has(lt, item.label) {
                        missing.insert(item.label);
                    }
                }
            }
        }
        assert!(missing.is_empty(), "Lithuanian misses commands/tools: {missing:#?}");
    }

    #[test]
    fn lithuanian_covers_ui_literals() {
        let lt = Lang::from_code("lt").expect("lt registered");
        let literals = scan_ui_literals(true);
        assert!(literals.contains("Float panel"), "direct lookup literals are included");
        assert!(literals.contains("@{login}: {prs} PRs, {commits} commits, +{added} / −{deleted} lines (Δ {delta}), +{bin_added} / −{bin_deleted} binary assets, {first} – {last}"), "multiline literal arguments are included");
        let missing: Vec<_> = literals.iter().filter(|label| !has(lt, label)).collect();
        assert!(missing.is_empty(), "Untranslated Lithuanian UI labels: {missing:#?}");
    }

    #[test]
    fn lithuanian_covers_contextual_calendar_certificate_and_preference_labels() {
        let lt = Lang::from_code("lt").expect("lt registered");
        assert_eq!(tr_ctx(lt, "signature pad", "Type"), "Tekstu");
        for label in ["All tools", "Read", "E-Sign", "Tools", "View more", "View less"] {
            assert!(has(lt, label), "shell label remains untranslated: {label}");
        }
        assert_eq!(["Slow", "Normal", "Fast"].map(|label| tr_ctx(lt, "scroll speed", label)), ["Lėtas", "Įprastas", "Greitas"]);
        let months = [
            ("January", "sausis", "sausio"),
            ("February", "vasaris", "vasario"),
            ("March", "kovas", "kovo"),
            ("April", "balandis", "balandžio"),
            ("May", "gegužė", "gegužės"),
            ("June", "birželis", "birželio"),
            ("July", "liepa", "liepos"),
            ("August", "rugpjūtis", "rugpjūčio"),
            ("September", "rugsėjis", "rugsėjo"),
            ("October", "spalis", "spalio"),
            ("November", "lapkritis", "lapkričio"),
            ("December", "gruodis", "gruodžio"),
        ];
        for (source, standalone, in_date) in months {
            assert_eq!(tr(lt, source), standalone);
            assert_eq!(tr_ctx(lt, "calendar date", source), in_date);
        }
        assert_eq!(
            ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"].map(|day| tr_ctx(lt, "calendar weekday", day)),
            ["Sk", "Pr", "An", "Tr", "Kt", "Pn", "Št"]
        );
        assert_eq!(
            fmt(tr(lt, "{month} {day}, {y}"), &[("month", tr_ctx(lt, "calendar date", "March")), ("day", "9"), ("y", "2026")]),
            "2026 m. kovo 9 d."
        );
        assert_eq!(tr_ctx(lt, "certificate", "Subject"), "Sertifikato subjektas");
        for field in [
            "Issued to",
            "Issued by",
            "Valid from",
            "Valid to",
            "Intended usage",
            "Serial number",
            "Issuer",
            "Validity starts",
            "Validity ends",
            "Public key",
            "Basic constraints",
            "Key usage",
            "Self-signed",
            "SHA-1 digest",
            "SHA-256 digest",
        ] {
            assert_ne!(tr_ctx(lt, "certificate", field), field, "certificate field remains untranslated: {field}");
        }
    }

    #[test]
    fn lithuanian_dialog_wording_preserves_resolution_and_selection_counts() {
        let lt = Lang::from_code("lt").expect("lt registered");
        assert_eq!(tr(lt, "for images above"), "kai raiška viršija");
        assert_eq!(
            fmt(tr(lt, "Checking Options ({on} of 32 in all categories)"), &[("on", "31")]),
            "Tikrinimo parinktys (visose kategorijose: 31 iš 32)"
        );
    }

    #[test]
    fn lithuanian_covers_bundled_catalog_keys_including_dynamic_dialog_labels() {
        // Compare English keys, not other languages' translations. Literal scans alone
        // miss labels passed through helpers, enum methods and conditional expressions.
        let lt = Lang::from_code("lt").expect("lt registered");
        let mut missing = std::collections::BTreeSet::new();
        for reference in LANGUAGES.iter().filter(|language| language.code != "lt") {
            let (entries, errors) = catalog::parse_entries(reference.source, reference.plural_forms());
            assert!(errors.is_empty(), "{}: invalid reference catalog: {errors:?}", reference.code);
            for entry in entries {
                let translated = match entry.context.as_str() {
                    "" => lt.catalog().plain(&entry.source),
                    "@id" => lt.catalog().id(&entry.source),
                    "@plural" => entry.source.split_once('|').and_then(|(one, other)| lt.catalog().plural(one, other, 0)),
                    context => lt.catalog().contextual(context, &entry.source),
                };
                if translated.is_none() {
                    missing.insert((entry.context, entry.source));
                }
            }
        }
        assert!(missing.is_empty(), "Lithuanian misses bundled catalog keys: {missing:#?}");
    }

    #[test]
    fn lithuanian_covers_credits_labels() {
        let lt = Lang::from_code("lt").expect("lt registered");
        let labels = crate::credits::NameMode::ALL
            .iter()
            .map(|mode| mode.label())
            .chain(crate::credits::SortKey::ALL.iter().flat_map(|key| {
                let (menu, heading) = key.label();
                [menu, heading]
            }))
            .chain(crate::credits::MODEL_COLUMNS.iter().copied());
        let missing: Vec<_> = labels.filter(|label| !has(lt, label)).collect();
        assert!(missing.is_empty(), "Lithuanian misses credits labels: {missing:#?}");
    }

    #[test]
    fn lithuanian_wording_fixes_keep_context_and_meaning() {
        let lt = Lang::from_code("lt").expect("lt registered");
        assert_eq!(tr(lt, "of {name}"), "iš {name}");
        assert_eq!(tr(lt, "Active"), "Aktyvi", "the adjective describes a theme (tema)");
        for n in [1, 2, 12] {
            let renamed = trn(
                lt,
                n,
                "{n} field name collides with earlier files and will be renamed: {list}",
                "{n} field names collide with earlier files and will be renamed: {list}",
            );
            assert!(renamed.contains("ankstesniuose failuose esančių laukų pavadinimais"));
            assert!(renamed.contains("bus pervadint"));
            assert!(renamed.contains("{list}"));
        }
        assert_eq!(tr(lt, "Two-Up (Facing)"), "Du atgręžti puslapiai");
        assert_eq!(tr(lt, "Windows store  ·  "), "„Windows“ sertifikatų saugykla  ·  ");
        assert_eq!(
            tr(lt, "Adds invisible text over each word; the page image is not changed"),
            "Virš kiekvieno žodžio prideda nematomą tekstą; puslapio vaizdas nesikeičia"
        );
        assert!(
            tr(lt, "The fonts PdfCraft writes with can't draw “{c}” ({code}), so it shows as ? in every PDF viewer. The text is saved.")
                .contains("visose PDF žiūryklėse")
        );
    }

    #[test]
    fn vietnamese_is_registered_and_persists() {
        let vi = Lang::from_code("vi").expect("vi registered");
        assert_eq!(vi.name(), "Tiếng Việt");
        assert_eq!(normalize_pref("Vi"), Some("vi"));
        for tag in ["vi", "vi-VN", "vi_VN.UTF-8", "VI-vn"] {
            assert_eq!(lang_from_tag(tag), Some(vi), "{tag}");
        }
        assert_eq!(first_supported("vi-VN\r\nen-US"), Some(vi));
        assert_eq!(tr(vi, "File"), "Tệp");
        assert_eq!(tr(vi, "Edit"), "Chỉnh sửa");
        assert_eq!(tr(vi, "Save"), "Lưu");
        assert_eq!(tr(vi, "Save as…"), "Lưu thành…");
        assert_eq!(tr(vi, "Bookmarks"), "Dấu trang");
        assert_eq!(tr(vi, "Fill & Sign"), "Điền & Ký");
        assert_eq!(tr(vi, "Interface language"), "Ngôn ngữ giao diện");
        assert_eq!((0..=3).map(|n| (vi.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
        for n in [0, 1, 2, 5] {
            assert_eq!(trn(vi, n, "{n} page", "{n} pages"), format!("{n} trang"));
            assert_eq!(trn(vi, n, "{n} field", "{n} fields"), format!("{n} trường"));
        }
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
    fn vietnamese_covers_commands_and_catalogue() {
        let vi = Lang::from_code("vi").expect("vi registered");
        let mut missing = std::collections::BTreeSet::new();
        for command in pdfcraft_engine::commands::COMMANDS {
            if !has(vi, command.label) {
                missing.insert(command.label);
            }
            if let Some(menu) = command.menu
                && !has(vi, menu)
            {
                missing.insert(menu);
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            if !has(vi, group.label) {
                missing.insert(group.label);
            }
            for section in group.sections {
                if !has(vi, section.title) {
                    missing.insert(section.title);
                }
                for item in section.items {
                    if !has(vi, item.label) {
                        missing.insert(item.label);
                    }
                }
            }
        }
        assert!(missing.is_empty(), "Vietnamese misses commands/tools: {missing:#?}");
    }

    #[test]
    fn vietnamese_covers_ui_literals() {
        let vi = Lang::from_code("vi").expect("vi registered");
        let literals = scan_ui_literals(true);
        let missing: Vec<_> = literals.iter().filter(|label| !has(vi, label)).collect();
        assert!(missing.is_empty(), "Untranslated Vietnamese UI labels: {missing:#?}");
    }

    #[test]
    fn vietnamese_covers_contextual_calendar_certificate_and_preference_labels() {
        let vi = Lang::from_code("vi").expect("vi registered");
        assert_eq!(tr_ctx(vi, "comment menu", "Edit"), "Sửa");
        assert_eq!(tr_ctx(vi, "signature pad", "Type"), "Nhập chữ");
        assert_eq!(tr_ctx(vi, "action wizard", "Start"), "Bắt đầu");
        assert_eq!(["Slow", "Normal", "Fast"].map(|label| tr_ctx(vi, "scroll speed", label)), ["Chậm", "Bình thường", "Nhanh"]);
        let months = [
            ("January", "Tháng 1", "tháng 1"),
            ("February", "Tháng 2", "tháng 2"),
            ("March", "Tháng 3", "tháng 3"),
            ("April", "Tháng 4", "tháng 4"),
            ("May", "Tháng 5", "tháng 5"),
            ("June", "Tháng 6", "tháng 6"),
            ("July", "Tháng 7", "tháng 7"),
            ("August", "Tháng 8", "tháng 8"),
            ("September", "Tháng 9", "tháng 9"),
            ("October", "Tháng 10", "tháng 10"),
            ("November", "Tháng 11", "tháng 11"),
            ("December", "Tháng 12", "tháng 12"),
        ];
        for (source, standalone, in_date) in months {
            assert_eq!(tr(vi, source), standalone);
            assert_eq!(tr_ctx(vi, "calendar date", source), in_date);
        }
        assert_eq!(
            ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"].map(|day| tr_ctx(vi, "calendar weekday", day)),
            ["CN", "T2", "T3", "T4", "T5", "T6", "T7"]
        );
        assert_eq!(tr_ctx(vi, "certificate", "Subject"), "Chủ thể chứng chỉ");
        assert_eq!(tr(vi, "Subject"), "Chủ đề");
        // Appearance: object/comment/signature look vs Preferences theme section.
        assert_eq!(tr(vi, "Appearance"), "Diện mạo");
        assert_eq!(tr_ctx(vi, "theme", "Appearance"), "Giao diện");
        // PDF form Field — never the visual-box sense "ô".
        assert_eq!(tr(vi, "Field"), "Trường");
        assert_eq!(tr(vi, "Fields"), "Các trường");
        assert_eq!(tr(vi, "Form fields"), "Các trường biểu mẫu");
        assert_eq!(tr(vi, "Text field"), "Trường văn bản");
        assert_eq!(tr(vi, "Date field"), "Trường ngày");
        assert_eq!(tr(vi, "Image field"), "Trường hình ảnh");
        assert_eq!(tr(vi, "Field properties"), "Thuộc tính trường");
        assert_eq!(tr(vi, "Field list"), "Danh sách trường");
        assert_eq!(tr(vi, "{kind} field"), "Trường {kind}");
        // Redaction: permanent removal, not temporary cover.
        assert_eq!(tr(vi, "Mark for redaction"), "Đánh dấu để xóa");
        assert_eq!(tr(vi, "Apply redactions"), "Xóa vĩnh viễn nội dung đã đánh dấu");
        assert_eq!(tr(vi, "Redact text and images"), "Xóa nội dung nhạy cảm trong văn bản và hình ảnh");
        assert_eq!(tr(vi, "Clear form"), "Xóa dữ liệu biểu mẫu");
        // Fill & Sign tool section title — fill-in, not paint fill.
        assert_eq!(tr(vi, "Fill"), "Điền");
        assert_eq!(tr(vi, "Fill & Sign"), "Điền & Ký");
        assert_eq!(
            tr(vi, "Remove \"{name}\" from this list? The file itself is not deleted."),
            "Gỡ \"{name}\" khỏi danh sách này? Bản thân tệp không bị xóa."
        );
        for field in [
            "Issued to",
            "Issued by",
            "Valid from",
            "Valid to",
            "Intended usage",
            "Serial number",
            "Issuer",
            "Validity starts",
            "Validity ends",
            "Public key",
            "Basic constraints",
            "Key usage",
            "Self-signed",
            "SHA-1 digest",
            "SHA-256 digest",
        ] {
            assert_ne!(tr_ctx(vi, "certificate", field), field, "certificate field remains untranslated: {field}");
        }
    }

    #[test]
    fn vietnamese_covers_bundled_catalog_keys() {
        let vi = Lang::from_code("vi").expect("vi registered");
        let mut missing = std::collections::BTreeSet::new();
        for reference in LANGUAGES.iter().filter(|language| language.code != "vi") {
            let (entries, errors) = catalog::parse_entries(reference.source, reference.plural_forms());
            assert!(errors.is_empty(), "{}: invalid reference catalog: {errors:?}", reference.code);
            for entry in entries {
                let translated = match entry.context.as_str() {
                    "" => vi.catalog().plain(&entry.source),
                    "@id" => vi.catalog().id(&entry.source),
                    "@plural" => entry.source.split_once('|').and_then(|(one, other)| vi.catalog().plural(one, other, 0)),
                    context => vi.catalog().contextual(context, &entry.source),
                };
                if translated.is_none() {
                    missing.insert((entry.context, entry.source));
                }
            }
        }
        assert!(missing.is_empty(), "Vietnamese misses bundled catalog keys: {missing:#?}");
    }

    #[test]
    fn vietnamese_covers_credits_labels() {
        let vi = Lang::from_code("vi").expect("vi registered");
        let labels = crate::credits::NameMode::ALL
            .iter()
            .map(|mode| mode.label())
            .chain(crate::credits::SortKey::ALL.iter().flat_map(|key| {
                let (menu, heading) = key.label();
                [menu, heading]
            }))
            .chain(crate::credits::MODEL_COLUMNS.iter().copied());
        let missing: Vec<_> = labels.filter(|label| !has(vi, label)).collect();
        assert!(missing.is_empty(), "Vietnamese misses credits labels: {missing:#?}");
    }

    #[test]
    fn vietnamese_leaves_no_unapproved_english_fallbacks() {
        let vi = Lang::from_code("vi").expect("vi registered");
        // Authoritative allowlist for intentional English-identical Vietnamese rows.
        const KEEP_AS_IS: &[&str] = &[
            "OK",
            "Digital ID",
            "ZIP",
            "JavaScript",
            "Microsoft Word (.docx)",
            "PDF/A…",
            "PostScript / EPS",
            "Rich Text Format (.rtf)",
            "CheckBox",
            "Combo",
            "Home / End",
            "PushButton",
            "Radio",
            "Standard Business",
            "Keychain  ·  ",
            "{0}: {1}",
            "{field}: {e}",
            "{month} {y}",
            "{n} {kind}",
            "{rule} - {status}",
            "+LOC",
            "−LOC",
            "ΔLOC",
            "+Bin",
            "−Bin",
            "`{command}` {when}",
            "English",
        ];
        const KEY_CAPS: &[&str] = &["Ctrl", "Shift", "Delete", "Esc", "Home", "End", "Space"];
        let (entries, errors) = catalog::parse_entries(vi.0.source, vi.0.plural_forms());
        assert!(errors.is_empty(), "{errors:?}");
        let edges = |s: &str| (s.len() - s.trim_start().len(), s.len() - s.trim_end().len());
        let mut unchanged = Vec::new();
        for e in &entries {
            let key_cap = e.context == "key" && KEY_CAPS.contains(&e.source.as_str());
            if e.source == e.translation && !KEEP_AS_IS.contains(&e.source.as_str()) && !key_cap {
                unchanged.push((e.context.clone(), e.source.clone()));
            }
            assert!(!e.translation.contains("..."), "use … rather than three dots: {:?}", e.translation);
            assert_eq!(edges(&e.translation), edges(&e.source), "stray whitespace: {:?}", e.translation);
            if e.context == "@plural" {
                assert!(!e.translation.contains('|'), "Vietnamese @plural is one form: {:?}", e.translation);
            }
        }
        assert!(unchanged.is_empty(), "unapproved English Vietnamese catalog entries: {unchanged:#?}");
    }

    #[test]
    fn slovene_is_registered_and_persists() {
        let sl = Lang::from_code("sl").expect("sl registered");
        assert_eq!(sl.name(), "Slovenščina");
        assert_eq!(normalize_pref("SL"), Some("sl"));
        for tag in ["sl", "sl-SI", "sl_SI.UTF-8", "SL-si", "sl_SI@variant"] {
            assert_eq!(lang_from_tag(tag), Some(sl), "{tag}");
        }
        assert_eq!(first_supported("sl-SI\r\nen-US"), Some(sl));
        assert_eq!(tr(sl, "File"), "Datoteka");
        assert_eq!(tr(sl, "Save as…"), "Shrani kot…");
        assert_eq!(tr(sl, "Bookmarks"), "Zaznamki");
        assert_eq!(tr(sl, "Layers"), "Plasti");
        assert_eq!(tr(sl, "Poročilo {n}.pdf"), "Poročilo {n}.pdf");
        assert_eq!(tr_ctx(sl, "comment menu", "Edit"), "Uredi");
        assert_eq!(tr_ctx(sl, "signature pad", "Type"), "Vtipkaj");
        assert_eq!(tr_ctx(sl, "action wizard", "Start"), "Začni");
        // one (1), two (2), few (3, 4), other (0, 5+)
        assert_eq!((0..=5).map(|n| (sl.0.plural)(n)).collect::<Vec<_>>(), [3, 0, 1, 2, 2, 3]);
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
    fn slovene_plurals_cover_dual_few_teens_and_large_counts() {
        let sl = Lang::from_code("sl").expect("sl registered");
        for (counts, page, field) in [
            (vec![1, 101, 201, 1001, 1101], "stran", "polje"),
            (vec![2, 102, 202, 1002], "strani", "polji"),
            (vec![3, 4, 103, 104, 1003], "strani", "polja"),
            (vec![0, 5, 10, 11, 12, 13, 14, 15, 20, 21, 22, 23, 24, 25, 31, 100, 111, 112, 121, 122, 1000, u64::MAX], "strani", "polj"),
        ] {
            for n in counts {
                assert_eq!(trn(sl, n, "{n} page", "{n} pages"), format!("{n} {page}"));
                assert_eq!(trn(sl, n, "{n} field", "{n} fields"), format!("{n} {field}"));
            }
        }
    }

    #[test]
    fn slovene_diagnostics_preserve_user_values() {
        let sl = Lang::from_code("sl").expect("sl registered");
        set_current(sl);
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "Izbranih strani: 3");
        assert_eq!(tr(sl, "CheckBox"), "Potrditveno polje");
        set_current(Lang::EN);
    }

    /// Slovene translates every registered command and every All tools group, section and item.
    #[test]
    fn slovene_covers_commands_and_catalogue() {
        let sl = Lang::from_code("sl").expect("sl registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(sl, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(sl, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(sl, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(sl, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(sl, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn slovene_covers_ui_literals() {
        let sl = Lang::from_code("sl").expect("sl registered");
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(sl, label)).collect();
        assert!(missing.is_empty(), "untranslated Slovene UI literals: {missing:#?}");
    }

    #[test]
    fn indonesian_and_vietnamese_are_registered_and_persist() {
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
        assert_eq!(tr(id, "Save"), "Simpan");
        assert_eq!(tr(id, "Save as…"), "Simpan sebagai…");
        assert_eq!(tr(id, "Bookmarks"), "Bookmark");
        assert_eq!(tr(id, "Fill & Sign"), "Isi & Tanda Tangani");
        assert_eq!(tr(vi, "Save"), "Lưu");
        assert_eq!(tr(vi, "Save as…"), "Lưu thành…");
        assert_eq!(tr(vi, "Bookmarks"), "Dấu trang");
        assert_eq!(tr(vi, "Fill & Sign"), "Điền & Ký");
        assert_eq!(tr(vi, "Interface language"), "Ngôn ngữ giao diện");
        assert_eq!(tr(id, "Interface language"), "Bahasa antarmuka");
        // One CLDR plural form (CLDR `id`, `vi`): the message reads the same for every count.
        assert_eq!((0..=3).map(|n| (id.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
        assert_eq!((0..=3).map(|n| (vi.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
        for n in [0, 1, 2, 5] {
            assert_eq!(trn(id, n, "{n} page", "{n} pages"), format!("{n} halaman"));
            assert_eq!(trn(vi, n, "{n} page", "{n} pages"), format!("{n} trang"));
            assert_eq!(trn(id, n, "{n} field", "{n} fields"), format!("{n} kolom"));
            assert_eq!(trn(vi, n, "{n} field", "{n} fields"), format!("{n} trường"));
        }
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

    /// Indonesian and Vietnamese translate every registered command and every All tools group,
    /// section and item.
    #[test]
    fn indonesian_and_vietnamese_cover_commands_and_catalogue() {
        for lang in [Lang::from_code("id").expect("id registered"), Lang::from_code("vi").expect("vi registered")] {
            for command in pdfcraft_engine::commands::COMMANDS {
                assert!(has(lang, command.label), "{} missing command: {}", lang.code(), command.label);
                if let Some(menu) = command.menu {
                    assert!(has(lang, menu), "{} missing menu: {menu}", lang.code());
                }
            }
            for group in pdfcraft_engine::catalog::TOOL_GROUPS {
                assert!(has(lang, group.label), "{} missing group: {}", lang.code(), group.label);
                for section in group.sections {
                    assert!(has(lang, section.title), "{} missing section: {}", lang.code(), section.title);
                    for item in section.items {
                        assert!(has(lang, item.label), "{} missing item: {}", lang.code(), item.label);
                    }
                }
            }
        }
    }

    /// Every `tl!("…")` literal in the UI source is translated in Indonesian and Vietnamese, so a
    /// new label cannot silently fall back to English in either.
    #[test]
    fn indonesian_and_vietnamese_cover_ui_literals() {
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        for lang in [Lang::from_code("id").expect("id registered"), Lang::from_code("vi").expect("vi registered")] {
            let missing: Vec<_> = literals.iter().filter(|label| !has(lang, label)).collect();
            assert!(missing.is_empty(), "untranslated {} UI literals: {missing:#?}", lang.code());
        }
    }

    #[test]
    fn hindi_and_korean_are_registered_and_persist() {
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
        assert_eq!(tr(hi, "Save"), "सहेजें");
        assert_eq!(tr(hi, "Save as…"), "इस रूप में सहेजें…");
        assert_eq!(tr(hi, "Bookmarks"), "बुकमार्क");
        assert_eq!(tr(hi, "Home"), "होम");
        assert_eq!(tr(hi, "Interface language"), "इंटरफ़ेस भाषा");
        assert_eq!(tr(ko, "Save"), "저장");
        assert_eq!(tr(ko, "Save as…"), "다른 이름으로 저장…");
        assert_eq!(tr(ko, "Bookmarks"), "책갈피");
        assert_eq!(tr(ko, "Home"), "홈");
        assert_eq!(tr(ko, "Interface language"), "인터페이스 언어");
        // One plural form per catalog: the message reads the same for every count.
        assert_eq!((0..=3).map(|n| (hi.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
        assert_eq!((0..=3).map(|n| (ko.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
        for n in [0, 1, 2, 5] {
            assert_eq!(trn(hi, n, "{n} page", "{n} pages"), format!("{n} पृष्ठ"));
            assert_eq!(trn(ko, n, "{n} page", "{n} pages"), format!("{n}페이지"));
            assert_eq!(trn(ko, n, "{n} field", "{n} fields"), format!("필드 {n}개"));
        }
        // The broken-line regression: a `\n` escape stays one catalog line.
        assert_eq!(tr(ko, "This page couldn't be displayed.\n{e}"), "이 페이지를 표시할 수 없습니다.\n{e}");
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

    /// Redaction first marks a region; applying it irreversibly removes marked content.
    /// Korean must not present either stage as censorship ("검열") or imply that
    /// clearing un-applied marks deletes PDF content.
    #[test]
    fn korean_redaction_labels_distinguish_marks_and_permanent_removal() {
        let ko = Lang::from_code("ko").expect("ko registered");
        for english in [
            "Redact",
            "Redact a PDF",
            "Redact text and images",
            "Redact pages…",
            "Mark for redaction",
            "Apply redactions",
            "Apply redactions…",
            "Clear redaction marks",
            "Redact all",
        ] {
            assert!(!tr(ko, english).contains("검열"), "Korean label misstates redaction: {english}");
        }
        assert_eq!(tr(ko, "Mark for redaction"), "제거할 영역 표시");
        assert_eq!(tr(ko, "Apply redactions"), "표시된 정보 영구 제거");
        assert_eq!(tr(ko, "Clear redaction marks"), "제거 대상 표시 지우기");
        let warning = concat!(
            "You are about to apply 1 redaction mark. ",
            "Text, images and drawings under the marks, and comments ",
            "and form fields that overlap them, are removed permanently."
        );
        assert!(tr(ko, warning).contains("영구적으로 제거됩니다"));
    }

    /// Hindi and Korean translate every registered command and every All tools group, section
    /// and item.
    #[test]
    fn hindi_and_korean_cover_commands_and_catalogue() {
        for lang in [Lang::from_code("hi").expect("hi registered"), Lang::from_code("ko").expect("ko registered")] {
            for command in pdfcraft_engine::commands::COMMANDS {
                assert!(has(lang, command.label), "{} missing command: {}", lang.code(), command.label);
                if let Some(menu) = command.menu {
                    assert!(has(lang, menu), "{} missing menu: {menu}", lang.code());
                }
            }
            for group in pdfcraft_engine::catalog::TOOL_GROUPS {
                assert!(has(lang, group.label), "{} missing group: {}", lang.code(), group.label);
                for section in group.sections {
                    assert!(has(lang, section.title), "{} missing section: {}", lang.code(), section.title);
                    for item in section.items {
                        assert!(has(lang, item.label), "{} missing item: {}", lang.code(), item.label);
                    }
                }
            }
        }
    }

    /// Every `tl!("…")` literal in the UI source is translated in Hindi and Korean, so a new
    /// label cannot silently fall back to English in either.
    #[test]
    fn hindi_and_korean_cover_ui_literals() {
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        for lang in [Lang::from_code("hi").expect("hi registered"), Lang::from_code("ko").expect("ko registered")] {
            let missing: Vec<_> = literals.iter().filter(|label| !has(lang, label)).collect();
            assert!(missing.is_empty(), "untranslated {} UI literals: {missing:#?}", lang.code());
        }
    }

    #[test]
    fn dutch_danish_swedish_and_finnish_are_registered_and_persist() {
        let nl = Lang::from_code("nl").expect("nl registered");
        let da = Lang::from_code("da").expect("da registered");
        let sv = Lang::from_code("sv").expect("sv registered");
        let fi = Lang::from_code("fi").expect("fi registered");
        assert_eq!(nl.name(), "Nederlands");
        assert_eq!(da.name(), "Dansk");
        assert_eq!(sv.name(), "Svenska");
        assert_eq!(fi.name(), "Suomi");
        assert_eq!(normalize_pref("NL"), Some("nl"));
        assert_eq!(normalize_pref("DA"), Some("da"));
        assert_eq!(normalize_pref("SV"), Some("sv"));
        assert_eq!(normalize_pref("FI"), Some("fi"));
        for tag in ["nl", "nl-NL", "nl_BE.UTF-8", "NL-nl"] {
            assert_eq!(lang_from_tag(tag), Some(nl), "{tag}");
        }
        for tag in ["da", "da-DK", "da_DK.UTF-8", "DA-dk"] {
            assert_eq!(lang_from_tag(tag), Some(da), "{tag}");
        }
        for tag in ["sv", "sv-SE", "sv_FI.UTF-8", "SV-se"] {
            assert_eq!(lang_from_tag(tag), Some(sv), "{tag}");
        }
        for tag in ["fi", "fi-FI", "fi_FI.UTF-8", "FI-fi"] {
            assert_eq!(lang_from_tag(tag), Some(fi), "{tag}");
        }
        assert_eq!(first_supported("nl-NL\r\nen-US"), Some(nl));
        assert_eq!(first_supported("da-DK\r\nen-US"), Some(da));
        assert_eq!(first_supported("sv-SE\r\nen-US"), Some(sv));
        assert_eq!(first_supported("fi-FI\r\nen-US"), Some(fi));
        assert_eq!(tr(nl, "Save"), "Opslaan");
        assert_eq!(tr(nl, "Save as…"), "Opslaan als…");
        assert_eq!(tr(nl, "Bookmarks"), "Bladwijzers");
        assert_eq!(tr(nl, "Interface language"), "Interfacetaal");
        assert_eq!(tr(da, "Save"), "Gem");
        assert_eq!(tr(da, "Save as…"), "Gem som…");
        assert_eq!(tr(da, "Bookmarks"), "Bogmærker");
        assert_eq!(tr(da, "Interface language"), "Grænsefladesprog");
        assert_eq!(tr(sv, "Save"), "Spara");
        assert_eq!(tr(sv, "Save as…"), "Spara som…");
        assert_eq!(tr(sv, "Bookmarks"), "Bokmärken");
        assert_eq!(tr(sv, "Interface language"), "Gränssnittsspråk");
        assert_eq!(tr(fi, "Save"), "Tallenna");
        assert_eq!(tr(fi, "Save as…"), "Tallenna nimellä…");
        assert_eq!(tr(fi, "Bookmarks"), "Kirjanmerkit");
        assert_eq!(tr(fi, "Interface language"), "Käyttöliittymän kieli");
        // Two plural forms (one, other) as in English: only 1 takes the first.
        for lang in [&nl, &da, &sv, &fi] {
            assert_eq!((0..=3).map(|n| (lang.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        }
        for n in [0, 1, 2, 5] {
            assert_eq!(trn(nl, n, "{n} page", "{n} pages"), if n == 1 { format!("{n} pagina") } else { format!("{n} pagina's") });
            assert_eq!(trn(da, n, "{n} page", "{n} pages"), if n == 1 { format!("{n} side") } else { format!("{n} sider") });
            assert_eq!(trn(sv, n, "{n} page", "{n} pages"), if n == 1 { format!("{n} sida") } else { format!("{n} sidor") });
            assert_eq!(trn(fi, n, "{n} page", "{n} pages"), if n == 1 { format!("{n} sivu") } else { format!("{n} sivua") });
            assert_eq!(trn(fi, n, "{n} field", "{n} fields"), if n == 1 { format!("{n} kenttä") } else { format!("{n} kenttää") });
        }
        // The broken-line regression: a `\n` escape stays one catalog line.
        assert_eq!(tr(nl, "This page couldn't be displayed.\n{e}"), "Deze pagina kon niet worden weergegeven.\n{e}");
        assert_eq!(tr(da, "This page couldn't be displayed.\n{e}"), "Denne side kunne ikke vises.\n{e}");
        assert_eq!(tr(sv, "This page couldn't be displayed.\n{e}"), "Den här sidan kunde inte visas.\n{e}");
        assert_eq!(tr(fi, "This page couldn't be displayed.\n{e}"), "Tätä sivua ei voitu näyttää.\n{e}");
        for code in ["nl", "da", "sv", "fi"] {
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

    /// Dutch, Danish, Swedish and Finnish translate every registered command and every All tools
    /// group, section and item.
    #[test]
    fn dutch_danish_swedish_and_finnish_cover_commands_and_catalogue() {
        for lang in ["nl", "da", "sv", "fi"].map(|code| Lang::from_code(code).expect("registered")) {
            for command in pdfcraft_engine::commands::COMMANDS {
                assert!(has(lang, command.label), "{} missing command: {}", lang.code(), command.label);
                if let Some(menu) = command.menu {
                    assert!(has(lang, menu), "{} missing menu: {menu}", lang.code());
                }
            }
            for group in pdfcraft_engine::catalog::TOOL_GROUPS {
                assert!(has(lang, group.label), "{} missing group: {}", lang.code(), group.label);
                for section in group.sections {
                    assert!(has(lang, section.title), "{} missing section: {}", lang.code(), section.title);
                    for item in section.items {
                        assert!(has(lang, item.label), "{} missing item: {}", lang.code(), item.label);
                    }
                }
            }
        }
    }

    /// Every `tl!("…")` literal in the UI source is translated in Dutch, Danish, Swedish and
    /// Finnish, so a new label cannot silently fall back to English in any of them.
    #[test]
    fn dutch_danish_swedish_and_finnish_cover_ui_literals() {
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        for lang in ["nl", "da", "sv", "fi"].map(|code| Lang::from_code(code).expect("registered")) {
            let missing: Vec<_> = literals.iter().filter(|label| !has(lang, label)).collect();
            assert!(missing.is_empty(), "untranslated {} UI literals: {missing:#?}", lang.code());
        }
    }

    #[test]
    fn bulgarian_is_registered() {
        let bg = Lang::from_code("bg").expect("bg registered");
        assert_eq!(bg.name(), "Български");
        assert_eq!(normalize_pref("BG"), Some("bg"));
        assert_eq!(lang_from_tag("bg_BG.UTF-8"), Some(bg));
        assert_eq!(lang_from_tag("bg-BG"), Some(bg));
        assert_eq!(first_supported("bg-BG\r\nen-US"), Some(bg));
        assert_eq!(tr(bg, "File"), "Файл");
        assert_eq!(tr(bg, "Save as…"), "Запиши като…");
        assert_eq!(tr(bg, "Bookmarks"), "Отметки");
        assert_eq!(tr(bg, "Layers"), "Слоеве");
        assert_eq!(tr(bg, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        // one (1), other (0, 2+)
        assert_eq!((0..=3).map(|n| (bg.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        assert_eq!(trn(bg, 0, "{n} page", "{n} pages"), "0 страници");
        assert_eq!(trn(bg, 1, "{n} page", "{n} pages"), "1 страница");
        assert_eq!(trn(bg, 2, "{n} page", "{n} pages"), "2 страници");
        assert_eq!(trn(bg, 21, "{n} page", "{n} pages"), "21 страници");
        assert_eq!(trn(bg, 1, "{n} field", "{n} fields"), "1 поле");
        assert_eq!(trn(bg, 5, "{n} field", "{n} fields"), "5 полета");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "bg").unwrap();
        assert_eq!(app.language, "bg");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "bg");
    }

    /// Bulgarian translates every registered command and every All tools group, section and item.
    #[test]
    fn bulgarian_covers_commands_and_catalogue() {
        let bg = Lang::from_code("bg").expect("bg registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(bg, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(bg, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(bg, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(bg, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(bg, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn bulgarian_covers_ui_literals() {
        let bg = Lang::from_code("bg").expect("bg registered");
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(bg, label)).collect();
        assert!(missing.is_empty(), "untranslated Bulgarian UI literals: {missing:#?}");
    }

    #[test]
    fn bulgarian_diagnostics_preserve_user_values() {
        let bg = Lang::from_code("bg").expect("bg registered");
        set_current(bg);
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "Избрани страници: 3");
        assert_eq!(tr(bg, "CheckBox"), "Квадратче за отметка");
        set_current(Lang::EN);
    }

    #[test]
    fn hungarian_is_registered() {
        let hu = Lang::from_code("hu").expect("hu registered");
        assert_eq!(hu.name(), "Magyar");
        assert_eq!(normalize_pref("HU"), Some("hu"));
        assert_eq!(lang_from_tag("hu_HU.UTF-8"), Some(hu));
        assert_eq!(lang_from_tag("hu-HU"), Some(hu));
        assert_eq!(first_supported("hu-HU\r\nen-US"), Some(hu));
        assert_eq!(tr(hu, "File"), "Fájl");
        assert_eq!(tr(hu, "Save as…"), "Mentés másként…");
        assert_eq!(tr(hu, "Bookmarks"), "Könyvjelzők");
        assert_eq!(tr(hu, "Layers"), "Rétegek");
        assert_eq!(tr(hu, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        assert_eq!(tr_ctx(hu, "signature pad", "Type"), "Gépelés");
        assert_eq!(tr_ctx(hu, "action wizard", "Start"), "Indítás");
        assert_eq!(tr(hu, "Windows store  ·  "), "Windows-tároló  ·  ");
        assert_eq!(tr(hu, "Place saved {what}"), "Mentett {what} elhelyezése");
        assert_eq!(tr(hu, "Remove saved {what}"), "Mentett {what} eltávolítása");
        assert_eq!(tr(hu, "Squiggly"), "Hullámos aláhúzás");
        // one (1), other (0, 2+)
        assert_eq!((0..=3).map(|n| (hu.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        assert_eq!(trn(hu, 0, "{n} page", "{n} pages"), "0 oldal");
        assert_eq!(trn(hu, 1, "{n} page", "{n} pages"), "1 oldal");
        assert_eq!(trn(hu, 2, "{n} page", "{n} pages"), "2 oldal");
        assert_eq!(trn(hu, 1, "{n} field", "{n} fields"), "1 mező");
        assert_eq!(trn(hu, 5, "{n} field", "{n} fields"), "5 mező");
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "hu").unwrap();
        assert_eq!(app.language, "hu");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "hu");
    }

    /// Hungarian translates every registered command and every All tools group, section and item.
    #[test]
    fn hungarian_covers_commands_and_catalogue() {
        let hu = Lang::from_code("hu").expect("hu registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(hu, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(hu, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(hu, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(hu, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(hu, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// New tl!("literal") labels must not silently fall back to English.
    #[test]
    fn hungarian_covers_ui_literals() {
        let hu = Lang::from_code("hu").expect("hu registered");
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
                    let mut rest = source.split("#[cfg(test)]\nmod ").next().unwrap_or_default();
                    while let Some((_, after)) = rest.split_once("tl!(\"") {
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
                        if tail.starts_with("\")") {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(hu, label)).collect();
        assert!(missing.is_empty(), "untranslated Hungarian UI literals: {missing:#?}");
    }

    #[test]
    fn hungarian_history_and_diagnostics_preserve_user_values() {
        let hu = Lang::from_code("hu").expect("hu registered");
        set_current(hu);
        assert_eq!(command_label("Undo Insert pages from Rapport {n}.pdf"), "Oldalak beszúrása ebből: Rapport {n}.pdf visszavonása");
        assert_eq!(command_label("Redo Fill in Contact {key}"), "Contact {key} kitöltése megismétlése");
        assert_eq!(action_label("Change Title"), "Cím módosítása");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(
            fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]),
            "Ezt az oldalt nem sikerült megjeleníteni.\nOS error {n}"
        );
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "3 oldal kijelölve");
        assert_eq!(tr(hu, "CheckBox"), "Jelölőnégyzet");
        set_current(Lang::EN);
    }

    #[test]
    fn ukrainian_is_registered_and_persists() {
        let uk = Lang::from_code("uk").expect("uk registered");
        assert_eq!(uk.name(), "Українська");
        assert_eq!(normalize_pref("UK"), Some("uk"));
        for tag in ["uk", "uk-UA", "uk_UA.UTF-8", "UK-ua", "uk_UA@variant"] {
            assert_eq!(lang_from_tag(tag), Some(uk), "{tag}");
        }
        assert_eq!(first_supported("uk-UA\r\nen-US"), Some(uk));
        assert_eq!(first_supported("(\n    \"uk-UA\",\n    \"en-US\"\n)\n"), Some(uk));
        assert_eq!(tr(uk, "File"), "Файл");
        assert_eq!(tr(uk, "Save as…"), "Зберегти як…");
        assert_eq!(tr(uk, "Bookmarks"), "Закладки");
        assert_eq!(tr(uk, "Layers"), "Шари");
        assert_eq!(tr(uk, "Звіт {n}.pdf"), "Звіт {n}.pdf");
        assert_eq!(tr_ctx(uk, "comment menu", "Edit"), "Редагувати");
        assert_eq!(tr_ctx(uk, "signature pad", "Type"), "Ввести");
        assert_eq!(tr_ctx(uk, "action wizard", "Start"), "Почати");
        assert_eq!(tr(uk, "Subject"), "Тема");
        assert_eq!(tr_ctx(uk, "certificate", "Subject"), "Власник сертифіката");
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
        let uk = Lang::from_code("uk").expect("uk registered");
        for (counts, page, field) in [
            (vec![1, 21, 31, 101, 121, 1001], "сторінка", "поле"),
            (vec![2, 3, 4, 22, 23, 24, 102, 104, 122, 1002], "сторінки", "поля"),
            (vec![0, 5, 10, 11, 12, 13, 14, 19, 20, 25, 100, 110, 111, 112, 114, 1000, u64::MAX], "сторінок", "полів"),
        ] {
            for n in counts {
                assert_eq!(trn(uk, n, "{n} page", "{n} pages"), format!("{n} {page}"));
                assert_eq!(trn(uk, n, "{n} field", "{n} fields"), format!("{n} {field}"));
            }
        }
    }

    #[test]
    fn ukrainian_calendar_uses_month_case_for_dates() {
        let uk = Lang::from_code("uk").expect("uk registered");
        for (source, standalone, date) in [
            ("January", "Січень", "січня"),
            ("February", "Лютий", "лютого"),
            ("March", "Березень", "березня"),
            ("April", "Квітень", "квітня"),
            ("May", "Травень", "травня"),
            ("June", "Червень", "червня"),
            ("July", "Липень", "липня"),
            ("August", "Серпень", "серпня"),
            ("September", "Вересень", "вересня"),
            ("October", "Жовтень", "жовтня"),
            ("November", "Листопад", "листопада"),
            ("December", "Грудень", "грудня"),
        ] {
            assert_eq!(tr(uk, source), standalone);
            let month = tr_ctx(uk, "calendar date", source);
            assert_eq!(month, date);
            assert_eq!(fmt(tr(uk, "{month} {day}, {y}"), &[("month", month), ("day", "9"), ("y", "2026")]), format!("9 {date} 2026"));
            // Catalogs without this context retain their existing month translation.
            assert_eq!(tr_ctx(Lang::EN, "calendar date", source), source);
            assert_eq!(tr_ctx(JA(), "calendar date", source), tr(JA(), source));
        }
    }

    /// Preserve only product/format names, abbreviations and templates with no English words.
    #[test]
    fn ukrainian_catalog_does_not_leave_english_ui_text() {
        let uk = Lang::from_code("uk").expect("uk registered");
        let (entries, errors) = parse_entries(uk.0.source, uk.0.plural_forms());
        assert!(errors.is_empty(), "{errors:?}");
        let keep = [
            "OK",
            "PDF/A…",
            "Microsoft Word (.docx)",
            "PostScript / EPS",
            // Key-cap names, printed in English on Ukrainian keyboards.
            "Home / End",
            "JavaScript",
            "ZIP",
            "{n} {kind}",
            "{rule} - {status}",
            "{field}: {e}",
            "`{command}` {when}",
            "{0}: {1}",
            "{month} {y}",
            "+LOC",
            "−LOC",
            "ΔLOC",
            "+Bin",
            "−Bin",
            // The distinguished name of a certificate, abbreviated as in X.500.
            "DN: ",
        ];
        let unchanged: Vec<_> = entries
            .iter()
            .filter(|e| {
                let key_cap = e.context == "key" && KEY_CAP_NAMES.contains(&e.source.as_str());
                e.source == e.translation && !keep.contains(&e.source.as_str()) && !key_cap
            })
            .collect();
        assert!(unchanged.is_empty(), "untranslated Ukrainian catalog entries: {unchanged:#?}");
    }

    /// Ukrainian translates every registered command and every All tools label.
    #[test]
    fn ukrainian_covers_commands_and_catalogue() {
        let uk = Lang::from_code("uk").expect("uk registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(uk, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(uk, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(uk, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(uk, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(uk, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Kazakh translates every registered command and every All tools label.
    #[test]
    fn kazakh_covers_commands_and_catalogue() {
        let kk = Lang::from_code("kk").expect("kk registered");
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(kk, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(kk, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(kk, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(kk, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(kk, item.label), "missing item: {}", item.label);
                }
            }
        }
    }
    /// Preserve only product/format names, abbreviations and templates with no English words.
    #[test]
    fn kazakh_catalog_does_not_leave_english_ui_text() {
        let kk = Lang::from_code("kk").expect("kk registered");
        let (entries, errors) = parse_entries(kk.0.source, kk.0.plural_forms());
        assert!(errors.is_empty(), "{errors:?}");
        let keep = [
            "OK",
            "PDF/A…",
            "Microsoft Word (.docx)",
            "PostScript / EPS",
            // Key-cap names, printed in English on Kazakh keyboards.
            "Home / End",
            "JavaScript",
            "ZIP",
            "{n} {kind}",
            "{rule} - {status}",
            "{field}: {e}",
            "`{command}` {when}",
            "{0}: {1}",
            "{month} {y}",
            "+LOC",
            "−LOC",
            "ΔLOC",
            "+Bin",
            "−Bin",
            // The distinguished name of a certificate, abbreviated as in X.500.
            "DN: ",
        ];
        let unchanged: Vec<_> = entries
            .iter()
            .filter(|e| {
                let key_cap = e.context == "key" && KEY_CAP_NAMES.contains(&e.source.as_str());
                e.source == e.translation && !keep.contains(&e.source.as_str()) && !key_cap
            })
            .collect();
        assert!(unchanged.is_empty(), "untranslated Kazakh catalog entries: {unchanged:#?}");
    }
    /// Kazakh translates every tl!("literal") in the UI source, including direct lookups and
    /// multiline literals, following the complete-catalog check for Simplified Chinese.
    #[test]
    fn kazakh_covers_ui_literals() {
        let kk = Lang::from_code("kk").expect("kk registered");
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
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n").replace("crate::i18n::t(", "tl!(");
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
                        let closing = tail.strip_prefix('"').expect("closing quote").trim_start();
                        let closing = closing.strip_prefix(',').unwrap_or(closing).trim_start();
                        if closing.starts_with(')') {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(kk, label)).collect();
        assert!(missing.is_empty(), "untranslated Kazakh UI literals: {missing:#?}");
    }
    /// Check direct lookups and multiline literals as well as ordinary tl!("literal") calls,
    /// following the complete-catalog check for Simplified Chinese.
    #[test]
    fn ukrainian_covers_ui_literals() {
        let uk = Lang::from_code("uk").expect("uk registered");
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
                    let source = std::fs::read_to_string(path).expect("UI source file").replace("\r\n", "\n").replace("crate::i18n::t(", "tl!(");
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
                        let closing = tail.strip_prefix('"').expect("closing quote").trim_start();
                        let closing = closing.strip_prefix(',').unwrap_or(closing).trim_start();
                        if closing.starts_with(')') {
                            let label: String = serde_json::from_str(&format!("\"{raw}\"")).expect("UI literal escapes");
                            literals.insert(label);
                        }
                        rest = tail.strip_prefix('"').expect("closing quote");
                    }
                }
            }
        }
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(uk, label)).collect();
        assert!(missing.is_empty(), "untranslated Ukrainian UI literals: {missing:#?}");
    }

    #[test]
    fn ukrainian_history_and_diagnostics_preserve_user_values() {
        let uk = Lang::from_code("uk").expect("uk registered");
        set_current(uk);
        assert_eq!(command_label("Undo Insert pages from Звіт {n}.pdf"), "Скасувати: Вставити сторінки з Звіт {n}.pdf");
        assert_eq!(command_label("Redo Fill in Контакт {key}"), "Повторити: Заповнити Контакт {key}");
        assert_eq!(action_label("Change Title"), "Змінити: Заголовок");
        assert_eq!(action_label("Custom action {n}"), "Custom action {n}");
        assert_eq!(fmt(t("This page couldn't be displayed.\n{e}"), &[("e", "OS error {n}")]), "Не вдалося відобразити цю сторінку.\nOS error {n}");
        assert_eq!(fmt(t("{n} pages selected"), &[("n", "3")]), "Вибрано сторінок: 3");
        assert_eq!(fmt(t("{n} contributors · {c} commits"), &[("n", "2"), ("c", "1,234")]), "Учасників: 2 · Комітів: 1,234");
        assert_eq!(tr(uk, "CheckBox"), "Прапорець");
        let contributor = crate::credits::Contributor {
            login: "reader{n}",
            display_name: Some("Save"),
            real_name: None,
            prs: 2,
            commits: 3,
            lines_added: 10,
            lines_deleted: 4,
            binary_added: 1,
            binary_deleted: 0,
            first_commit: "2026-10-01T00:00:00Z",
            last_commit: "2026-10-09T00:00:00Z",
        };
        assert_eq!(
            contributor.summary(),
            "@reader{n}: PR: 2, комітів: 3, +10 / −4 рядків (Δ +6), +1 / −0 бінарних ресурсів, 2026-10-01 – 2026-10-09"
        );
        assert_eq!(contributor.name(crate::credits::NameMode::DisplayName), "Save");
        set_current(Lang::EN);
    }

    fn arabic() -> Lang {
        Lang::from_code("ar").expect("ar registered")
    }

    #[test]
    fn arabic_is_registered_and_persists() {
        let ar = arabic();
        assert_eq!(ar.name(), "العربية");
        assert!(ar.rtl());
        assert!(Lang::all().filter(|l| l.rtl()).eq([ar]), "Arabic is the only right-to-left language");
        assert_eq!(normalize_pref("AR"), Some("ar"));
        for tag in ["ar", "ar-MA", "ar_EG.UTF-8", "AR-sa", "ar_DZ@variant"] {
            assert_eq!(lang_from_tag(tag), Some(ar), "{tag}");
        }
        assert_eq!(first_supported("ar-MA\r\nen-US"), Some(ar));
        assert_eq!(tr(ar, "File"), "ملف");
        assert_eq!(tr(ar, "Bookmarks"), "المرجعية الإشارات");
        assert_eq!(tr_ctx(ar, "comment menu", "Edit"), "تحرير");
        assert_eq!(tr_ctx(ar, "certificate", "Subject"), "الشهادة صاحب");
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

    /// Lookups return Arabic in display order: egui draws the words left to right as given, and
    /// joins the letters of each word itself.
    #[test]
    fn arabic_lookups_are_in_display_order() {
        let ar = arabic();
        // "حفظ باسم…" as typed; the ellipsis ends the phrase on the left.
        assert_eq!(tr(ar, "Save as…"), "…باسم حفظ");
        // Latin names keep their own direction inside the phrase.
        assert_eq!(tr(ar, "About PdfCraft"), "PdfCraft حول");
        // A caption's colon stays beside the widget on its right.
        assert_eq!(tr(ar, "Name:"), "الاسم:");
        assert_eq!(tr(ar, "Issued by: "), "عن صادرة: ");
        // Placeholders survive and are filled in place.
        assert_eq!(tr(ar, "Couldn't open {name}: {e}"), "{name}: {e} فتح تعذّر");
        assert_eq!(fmt(tr(ar, "Couldn't open {name}: {e}"), &[("name", "a {e}.pdf"), ("e", "x")]), "a {e}.pdf: x فتح تعذّر");
        assert_eq!(fmt(tr(ar, "Page {p} of {n}"), &[("p", "3"), ("n", "12")]), "12 من 3 الصفحة");
        set_current(ar);
        assert_eq!(command_label("Undo Insert pages from a.pdf"), "a.pdf من صفحات إدراج عن التراجع");
        assert_eq!(menu_label("file.saveAs", "Save as…"), "…باسم حفظ");
        set_current(Lang::EN);
    }

    #[test]
    fn arabic_plurals_follow_the_six_cldr_forms() {
        let ar = arabic();
        assert_eq!(ar.0.plural_forms(), 6);
        let form = |n| (ar.0.plural)(n);
        assert_eq!([0, 1, 2].map(form), [0, 1, 2]);
        assert_eq!([3, 10, 103, 110, 1003].map(form), [3; 5]);
        assert_eq!([11, 99, 111, 199, 1011].map(form), [4; 5]);
        assert_eq!([100, 101, 102, 200, 1000, u64::MAX - 13].map(form), [5; 6]);
        assert_eq!(trn(ar, 1, "{n} page", "{n} pages"), "صفحة 1");
        assert_eq!(trn(ar, 2, "{n} page", "{n} pages"), "صفحتان 2");
        assert_eq!(trn(ar, 5, "{n} page", "{n} pages"), "صفحات 5");
        assert_eq!(trn(ar, 11, "{n} field", "{n} fields"), "حقلًا 11");
        assert_eq!(trn(ar, 100, "{n} field", "{n} fields"), "حقل 100");
    }

    /// Arabic translates every registered command, menu and All tools label.
    #[test]
    fn arabic_covers_commands_and_catalogue() {
        let ar = arabic();
        for command in pdfcraft_engine::commands::COMMANDS {
            assert!(has(ar, command.label), "missing command: {}", command.label);
            if let Some(menu) = command.menu {
                assert!(has(ar, menu), "missing menu: {menu}");
            }
        }
        for group in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(has(ar, group.label), "missing group: {}", group.label);
            for section in group.sections {
                assert!(has(ar, section.title), "missing section: {}", section.title);
                for item in section.items {
                    assert!(has(ar, item.label), "missing item: {}", item.label);
                }
            }
        }
    }

    /// Arabic translates every `tl!("…")` literal in the UI source.
    #[test]
    fn arabic_covers_ui_literals() {
        let literals = ui_literals();
        assert!(literals.len() > 900, "source scan found only {} literals", literals.len());
        let missing: Vec<_> = literals.iter().filter(|label| !has(arabic(), label)).collect();
        assert!(missing.is_empty(), "untranslated Arabic UI literals: {missing:#?}");
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
        let ar = arabic();
        let (entries, _) = parse_entries(ar.0.source, ar.0.plural_forms());
        let labels: String = entries.iter().flat_map(|e| e.translation.chars()).chain(ar.name().chars()).filter(|c| !c.is_control()).collect();
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

    /// Every bundled catalog is well-formed and consistent with its sources.
    #[test]
    fn bundled_catalogs_are_consistent() {
        let mut codes = std::collections::HashSet::new();
        for l in &LANGUAGES {
            assert!(codes.insert(l.code), "duplicate code {}", l.code);
            assert!(l.code == l.code.to_ascii_lowercase() && !l.name.is_empty(), "{}", l.code);
            let (entries, errors) = parse_entries(l.source, l.plural_forms());
            assert!(errors.is_empty(), "{}: {errors:?}", l.code);
            for e in entries.iter().filter(|e| e.context == "@id") {
                let command = pdfcraft_engine::commands::command(&e.source);
                let Some(command) = command else { panic!("{}: unknown command id {:?}", l.code, e.source) };
                assert_eq!(placeholders(&e.translation), placeholders(command.label), "{}: {}", l.code, e.source);
                assert_eq!(e.translation.ends_with('…'), command.label.ends_with('…'), "{}: ellipsis mismatch: {}", l.code, e.source);
            }
        }
    }

    /// Preferences ▸ Date format ▸ Language offers exactly the interface languages, in order.
    #[test]
    fn date_languages_are_the_interface_languages() {
        let interface: Vec<(&str, &str)> = Lang::all().map(|l| (l.code(), l.name())).collect();
        let dates: Vec<(&str, &str)> = pdfcraft_engine::dates::DATE_LANGUAGES.iter().map(|l| (l.code, l.name)).collect();
        assert_eq!(dates, interface);
    }
}
