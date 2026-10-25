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

/// Romanian (CLDR `ro`): 1 is `one`; 0 and numbers whose last two digits are 1–19 (2, 19, 101,
/// 119) are `few`; everything else (20, 100, 120) is `other`, the form with "de" ("20 de pagini").
fn plural_ro(n: u64) -> usize {
    if n == 1 {
        0
    } else if n == 0 || (1..=19).contains(&(n % 100)) {
        1
    } else {
        2
    }
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
pub static LANGUAGES: [LangInfo; 35] = [
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
    // Central Kurdish (Sorani); every `ckb-*` locale (`ckb-IQ`, `ckb-IR`) resolves here. Right to
    // left like Arabic: see `LangInfo::rtl`. CLDR `ckb` counts have two forms (one, other).
    LangInfo {
        code: "ckb", name: "کوردیی ناوەندی", source: include_str!("ckb.tsv"), plural: plural_one_other, rtl: true, catalog: OnceLock::new()
    },
    // Persian (Farsi); every `fa-*` locale (`fa-IR`, `fa_IR.UTF-8`, `fa-AF` ...) resolves here. Right
    // to left like Arabic: see `LangInfo::rtl`. CLDR `fa` counts have two forms (one: i = 0 or n = 1,
    // other), which for whole counts is the French rule, so `@plural` entries list two forms.
    LangInfo { code: "fa", name: "فارسی", source: include_str!("fa.tsv"), plural: plural_fr, rtl: true, catalog: OnceLock::new() },
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
    // Finnish; every `fi-*` locale (`fi-FI`, `fi_FI.UTF-8`) resolves here. Two plural forms (one, other):
    // the partitive singular follows any number but 1 (`1 sivu`, `2 sivua`).
    LangInfo { code: "fi", name: "Suomi", source: include_str!("fi.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Albanian; every `sq-*` locale (`sq-AL`, `sq_AL.UTF-8`, `sq-XK`) resolves here. One/other plural, like English.
    LangInfo { code: "sq", name: "Shqip", source: include_str!("sq.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Thai; every `th-*` locale (`th-TH`, `th_TH.UTF-8`) resolves here. No grammatical plural.
    LangInfo { code: "th", name: "ไทย", source: include_str!("th.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Catalan; every `ca-*` locale (`ca-ES`, `ca_ES.UTF-8`, `ca_ES@valencia`) resolves here.
    LangInfo { code: "ca", name: "Català", source: include_str!("ca.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Romanian; every `ro-*` locale (`ro-RO`, `ro_RO.UTF-8`, `ro-MD`) resolves here. Three plural forms (one, few, other).
    LangInfo { code: "ro", name: "Română", source: include_str!("ro.tsv"), plural: plural_ro, rtl: false, catalog: OnceLock::new() },
    // Lao; every `lo-*` locale (`lo-LA`, `lo_LA.UTF-8` ...) resolves here.
    LangInfo { code: "lo", name: "ພາສາລາວ", source: include_str!("lo.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Hindi; every `hi-*` locale (`hi-IN`, `hi_IN.UTF-8`) resolves here. CLDR `hi` has "one"
    // (0 and 1) and "other", but the catalog's nouns are the same in both ("{n} पृष्ठ"), so
    // `@plural` entries list a single form.
    LangInfo { code: "hi", name: "हिन्दी", source: include_str!("hi.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Korean; every `ko-*` locale (`ko-KR`, `ko_KR.UTF-8`) resolves here. CLDR `ko` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "ko", name: "한국어", source: include_str!("ko.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Norwegian Bokmål; every `nb-*` locale resolves here, and so do general Norwegian (`no-*`) and
    // Nynorsk (`nn-*`), which have no catalog of their own (see `candidates`).
    LangInfo { code: "nb", name: "Norsk bokmål", source: include_str!("nb.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
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

    /// Every registered language in the order a picker lists them: English first (the source
    /// language and the fallback), then the rest by their own name with accents folded, so
    /// Čeština sits under C rather than after Z. Names in other scripts follow the Latin ones in
    /// code point order: Greek, Cyrillic, Arabic, then the CJK languages.
    pub fn for_picker() -> Vec<Lang> {
        let mut rest: Vec<Lang> = Lang::all().filter(|l| l.code() != Lang::EN.code()).collect();
        rest.sort_by_cached_key(|l| name_sort_key(l.name()));
        std::iter::once(Lang::EN).chain(rest).collect()
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
    if matches!(primary, "no" | "nn") {
        // The only Norwegian catalog is Bokmål. `no` (Norwegian in general) and Nynorsk readers use
        // it rather than English; last, so a catalog of their own would win.
        out.push("nb".to_string());
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

/// The first supported language among BCP 47 tags in preference order, as the browser lists them
/// in `navigator.languages` (#2954). Only the first few tags are looked at: the list is short in
/// practice, and a page can't make Auto walk an unbounded one.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn first_supported_tag<S: AsRef<str>>(tags: impl IntoIterator<Item = S>) -> Option<Lang> {
    tags.into_iter().take(32).find_map(|tag| lang_from_tag(tag.as_ref().trim()))
}

/// The browser's preferred languages, as the desktop builds use the system's: `navigator.languages`
/// in order, then `navigator.language` (kept within the 32 tags `first_supported_tag` reads).
/// English when the browser offers none we have a catalog for.
#[cfg(target_arch = "wasm32")]
fn detect_system_lang() -> Lang {
    let Some(navigator) = web_sys::window().map(|w| w.navigator()) else { return Lang::EN };
    let mut tags: Vec<String> = navigator.languages().iter().take(31).filter_map(|v| v.as_string()).collect();
    tags.extend(navigator.language());
    first_supported_tag(&tags).unwrap_or(Lang::EN)
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
    match current().code() {
        "de" => std::borrow::Cow::Borrowed(label),
        "tr" => std::borrow::Cow::Owned(label.chars().flat_map(lower_turkish).collect()),
        _ => std::borrow::Cow::Owned(label.to_lowercase()),
    }
}

/// The translation of `english` in upper case, for section titles. Rust's case mapping knows no
/// language, and Turkish has a dotted and a dotless i (`i` ↔ `İ`, `ı` ↔ `I`): "Gelişmiş" must
/// become "GELİŞMİŞ", not "GELIŞMIŞ". A label without a Turkish row is still English and keeps
/// the English rule.
pub fn upper_label(english: &str) -> String {
    let lang = current();
    let shown = tr(lang, english);
    if lang.code() == "tr" && has(lang, english) { shown.chars().flat_map(upper_turkish).collect() } else { shown.to_uppercase() }
}

fn upper_turkish(c: char) -> std::char::ToUppercase {
    match c {
        'i' => 'İ'.to_uppercase(),
        'ı' => 'I'.to_uppercase(),
        c => c.to_uppercase(),
    }
}

fn lower_turkish(c: char) -> std::char::ToLowercase {
    match c {
        // `İ`.to_lowercase() is "i" plus a combining dot above.
        'İ' => 'i'.to_lowercase(),
        'I' => 'ı'.to_lowercase(),
        c => c.to_lowercase(),
    }
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

/// `name` lowercased with the Latin accents that occur in language names folded away, so a
/// byte-wise sort reads alphabetically. Other scripts are left as they are.
fn name_sort_key(name: &str) -> String {
    name.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
            'ç' | 'ć' | 'č' => 'c',
            'è'..='ë' | 'ē' | 'ě' | 'ę' | 'ế' | 'ệ' => 'e',
            'ì'..='ï' | 'ī' => 'i',
            'ñ' | 'ń' | 'ň' => 'n',
            'ò'..='ö' | 'ø' | 'ő' => 'o',
            'ś' | 'š' => 's',
            'ù'..='ü' | 'ū' | 'ů' | 'ű' => 'u',
            'ý' | 'ÿ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::catalog::{Entry, parse_entries, placeholders};
    use super::*;

    const JA: fn() -> Lang = || Lang::from_code("ja").expect("ja registered");

    #[test]
    fn keyboard_key_names_have_a_separate_context_and_english_fallback() {
        static KEYS: LangInfo = LangInfo {
            code: "keys",
            name: "Key name test catalog",
            source: "key\tDelete\tEntf\n\tDelete\tLöschen\n\tOpen\tÖffnen\n",
            plural: plural_one_other,
            rtl: false,
            catalog: OnceLock::new(),
        };
        let previous = current();
        set_current(Lang(&KEYS));
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
        assert_eq!(lang_from_tag("lo_LA.UTF-8"), Lang::from_code("lo"));
        assert_eq!(lang_from_tag("lo-LA"), Lang::from_code("lo"));
        assert_eq!(lang_from_tag("lo"), Lang::from_code("lo"));
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
        assert_eq!(candidates("lo_LA.UTF-8"), ["lo-la", "lo"]);
        assert_eq!(candidates("nb_NO.UTF-8"), ["nb-no", "nb"]);
        assert_eq!(candidates("no_NO.UTF-8"), ["no-no", "no", "nb"]);
        assert_eq!(candidates("nn-NO"), ["nn-no", "nn", "nb"]);
    }

    #[test]
    fn macos_language_list_is_parsed() {
        assert_eq!(first_supported("(\n    \"ja-JP\",\n    \"en-US\"\n)\n"), Some(JA()));
        assert_eq!(first_supported("(\n    \"fr-FR\",\n    \"en-US\"\n)\n"), Lang::from_code("fr"));
        assert_eq!(first_supported("("), None);
    }

    /// #2954: the web build's Auto language follows the browser's preferred languages in order,
    /// skipping ones without a catalog, and is English only when none is supported.
    #[test]
    fn browser_language_list_is_followed_in_order() {
        assert_eq!(first_supported_tag(["it-IT", "en-US"]), Lang::from_code("it"));
        assert_eq!(first_supported_tag(["xx-YY", "ja", "fr-FR"]), Some(JA()));
        assert_eq!(first_supported_tag(["zh-TW"]), Lang::from_code("zh-hant"));
        assert_eq!(first_supported_tag(["en-GB", "fr-FR"]), Some(Lang::EN));
        assert_eq!(first_supported_tag([" pt-PT "]), Lang::from_code("pt-br"));
        assert_eq!(first_supported_tag(["", "xx"]), None);
        assert_eq!(first_supported_tag(Vec::<String>::new()), None);
        let long = std::iter::repeat_n("xx", 10_000).chain(["fr"]);
        assert_eq!(first_supported_tag(long), None, "only the first tags are read");
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
        assert_eq!(first_supported("lo-LA\r\n"), Lang::from_code("lo"));
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
        let english = pdfcraft_engine::sign::AppearanceLabels::default();
        for language in LANGUAGES.iter().filter(|language| !language.source.is_empty()) {
            let lang = Lang::from_code(language.code).expect("registered language");
            for (field, label) in english.fields() {
                let said = tr(lang, label);
                if field != "signed_by" && !lang.rtl() {
                    assert!(said.ends_with(' ') || said.ends_with('：'), "{}: the {field} label runs into its value: {said:?}", language.code);
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
        // No catalog has this `@plural` entry: Japanese falls back to the English forms.
        assert_eq!(trn(JA(), 7, "{n} test item", "{n} test items"), "7 test items");
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
        assert_eq!(tr(pt, "Arquivo do usuário.pdf"), "Arquivo do usuário.pdf");
        assert_eq!((0..=3).map(|n| (pt.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 1, 1]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "pt-br").unwrap();
        assert_eq!(app.language, "pt-br");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "pt-br");
    }

    #[test]
    fn spanish_is_registered() {
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

    #[test]
    fn czech_is_registered_and_persists() {
        let cs = Lang::from_code("cs").expect("cs registered");
        assert_eq!(cs.name(), "Čeština");
        assert_eq!(tr(cs, "Žluťoučký kůň.pdf"), "Žluťoučký kůň.pdf");
        assert_eq!((0..=6).map(|n| (cs.0.plural)(n)).collect::<Vec<_>>(), [2, 0, 1, 1, 1, 2, 2]);
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
        let tr_lang = Lang::from_code("tr").expect("tr registered");
        assert_eq!(tr_lang.name(), "Türkçe");
        assert_eq!(tr(tr_lang, "Türkçe Belge.pdf"), "Türkçe Belge.pdf");
        assert_eq!((0..=6).map(|n| (tr_lang.0.plural)(n)).collect::<Vec<_>>(), [0; 7]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "tr").unwrap();
        assert!(app.set_option("language", "tu").is_err());
        assert_eq!(app.language, "tr");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "tr");
    }

    /// The code joins some labels with a space at either end, so these catalogs keep exactly the
    /// source's, and write … rather than three dots.
    #[test]
    fn catalog_rows_keep_the_source_spacing_and_ellipsis() {
        let edges = |s: &str| (s.len() - s.trim_start().len(), s.len() - s.trim_end().len());
        for code in ["cs", "tr", "vi"] {
            let lang = Lang::from_code(code).expect("registered");
            let (entries, errors) = parse_entries(lang.0.source, lang.0.plural_forms());
            assert!(errors.is_empty(), "{code}: {errors:?}");
            for e in &entries {
                assert!(!e.translation.contains("..."), "{code}: use … rather than three dots: {:?}", e.translation);
                assert_eq!(edges(&e.translation), edges(&e.source), "{code}: stray whitespace: {:?}", e.translation);
            }
        }
    }

    #[test]
    fn simplified_chinese_is_registered() {
        let zh = Lang::from_code("zh-hans").expect("zh-hans registered");
        assert_eq!(zh.name(), "简体中文");
        assert_eq!(normalize_pref("zh-Hans"), Some("zh-hans"));
        assert_eq!(normalize_pref("zh"), None, "only exact codes are preferences");
        assert_eq!(tr(zh, "报告.pdf"), "报告.pdf");
    }

    #[test]
    fn traditional_chinese_is_registered() {
        let zh = Lang::from_code("zh-hant").expect("zh-hant registered");
        assert_eq!(zh.name(), "繁體中文");
        assert_eq!(normalize_pref("zh-Hant"), Some("zh-hant"));
        assert_eq!(tr(zh, "報告.pdf"), "報告.pdf");
    }

    #[test]
    fn french_is_registered() {
        let fr = Lang::from_code("fr").expect("fr registered");
        assert_eq!(fr.name(), "Français");
        assert_eq!(normalize_pref("FR"), Some("fr"));
        assert_eq!(lang_from_tag("fr_FR.UTF-8"), Some(fr));
        assert_eq!(lang_from_tag("fr-CA"), Some(fr));
        assert_eq!(first_supported("fr-FR\r\nen-US"), Some(fr));
        assert_eq!(tr(fr, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        assert_eq!((0..=3).map(|n| (fr.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 1, 1]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "fr").unwrap();
        assert_eq!(app.language, "fr");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "fr");
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
        assert_eq!(tr(de, "Bericht des Nutzers.pdf"), "Bericht des Nutzers.pdf");
        assert_eq!((0..=3).map(|n| (de.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
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
        set_current(Lang::from_code("de").expect("de registered"));
        assert_eq!(in_sentence("Textfeld"), "Textfeld");
        set_current(Lang::EN);
    }

    #[test]
    fn turkish_keeps_its_dotted_and_dotless_i_when_changing_case() {
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
        let ru = Lang::from_code("ru").expect("ru registered");
        assert_eq!(ru.name(), "Русский");
        assert_eq!(normalize_pref("RU"), Some("ru"));
        assert_eq!(lang_from_tag("ru_RU.UTF-8"), Some(ru));
        assert_eq!(lang_from_tag("ru-BY"), Some(ru));
        assert_eq!(first_supported("ru-RU\r\nen-US"), Some(ru));
        assert_eq!(tr(ru, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        // one (1), few (2-4), many (0, 5+), with 11-14 exception
        assert_eq!((0..=3).map(|n| (ru.0.plural)(n)).collect::<Vec<_>>(), [2, 0, 1, 1]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "ru").unwrap();
        assert_eq!(app.language, "ru");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "ru");
    }

    #[test]
    fn finnish_is_registered_and_persists() {
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
        assert_eq!((0..=3).map(|n| (nb.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "NB").unwrap();
        assert_eq!(app.language, "nb");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "nb");
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
        assert_eq!((0..=3).map(|n| (vi.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
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
        let ro = Lang::from_code("ro").expect("ro registered");
        for tag in ["ro", "ro-RO", "ro_RO.UTF-8", "RO-ro", "ro-MD"] {
            assert_eq!(lang_from_tag(tag), Some(ro), "{tag}");
        }
        assert_eq!(normalize_pref("Ro"), Some("ro"));
        let info = LANGUAGES.iter().find(|language| language.code == "ro").expect("ro row");
        assert_eq!(info.plural_forms(), 3);
        // one: 1; few: 0 and n % 100 in 1..=19 ("2 pagini", "101 pagini"); other: the "de" form ("20 de pagini").
        for (n, form) in [(1, 0), (0, 1), (2, 1), (19, 1), (101, 1), (119, 1), (20, 2), (100, 2), (120, 2), (1000, 2)] {
            assert_eq!(plural_ro(n), form, "{n}");
        }
    }

    #[test]
    fn catalan_is_registered_and_persists() {
        let ca = Lang::from_code("ca").expect("ca registered");
        assert_eq!(ca.name(), "Català");
        assert_eq!(normalize_pref("CA"), Some("ca"));
        for tag in ["ca", "ca-ES", "ca_ES.UTF-8", "CA-es", "ca_ES@valencia"] {
            assert_eq!(lang_from_tag(tag), Some(ca), "{tag}");
        }
        assert_eq!(first_supported("ca-ES\r\nen-US"), Some(ca));
        // one (1), other (0, 2+), as in English
        assert_eq!((0..=3).map(|n| (ca.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
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
        let sl = Lang::from_code("sl").expect("sl registered");
        assert_eq!(sl.name(), "Slovenščina");
        assert_eq!(normalize_pref("SL"), Some("sl"));
        for tag in ["sl", "sl-SI", "sl_SI.UTF-8", "SL-si", "sl_SI@variant"] {
            assert_eq!(lang_from_tag(tag), Some(sl), "{tag}");
        }
        assert_eq!(first_supported("sl-SI\r\nen-US"), Some(sl));
        assert_eq!(tr(sl, "Poročilo {n}.pdf"), "Poročilo {n}.pdf");
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
    fn albanian_is_registered_and_persists() {
        let sq = Lang::from_code("sq").expect("sq registered");
        assert_eq!(sq.name(), "Shqip");
        assert_eq!(normalize_pref("SQ"), Some("sq"));
        for tag in ["sq", "sq-AL", "sq_AL.UTF-8", "SQ-xk", "sq_XK@variant"] {
            assert_eq!(lang_from_tag(tag), Some(sq), "{tag}");
        }
        assert_eq!(first_supported("sq-AL\r\nen-US"), Some(sq));
        // one (1), other (0, 2+)
        assert_eq!((0..=3).map(|n| (sq.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
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
        let sl = Lang::from_code("sl").expect("sl registered");
        assert_eq!(sl.0.plural_forms(), 4);
        for (counts, form) in [
            (vec![1, 101, 201, 1001, 1101], 0),
            (vec![2, 102, 202, 1002], 1),
            (vec![3, 4, 103, 104, 1003], 2),
            (vec![0, 5, 10, 11, 12, 13, 14, 15, 20, 21, 22, 23, 24, 25, 31, 100, 111, 112, 121, 122, 1000, u64::MAX], 3),
        ] {
            for n in counts {
                assert_eq!((sl.0.plural)(n), form, "{n}");
            }
        }
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
        // One CLDR plural form (CLDR `id`, `vi`): the message reads the same for every count.
        assert_eq!((0..=3).map(|n| (id.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
        assert_eq!((0..=3).map(|n| (vi.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
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
        // One plural form per catalog: the message reads the same for every count.
        assert_eq!((0..=3).map(|n| (hi.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
        assert_eq!((0..=3).map(|n| (ko.0.plural)(n)).collect::<Vec<_>>(), [0, 0, 0, 0]);
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
        let bg = Lang::from_code("bg").expect("bg registered");
        assert_eq!(bg.name(), "Български");
        assert_eq!(normalize_pref("BG"), Some("bg"));
        assert_eq!(lang_from_tag("bg_BG.UTF-8"), Some(bg));
        assert_eq!(lang_from_tag("bg-BG"), Some(bg));
        assert_eq!(first_supported("bg-BG\r\nen-US"), Some(bg));
        assert_eq!(tr(bg, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        // one (1), other (0, 2+)
        assert_eq!((0..=3).map(|n| (bg.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "bg").unwrap();
        assert_eq!(app.language, "bg");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "bg");
    }

    #[test]
    fn hungarian_is_registered() {
        let hu = Lang::from_code("hu").expect("hu registered");
        assert_eq!(hu.name(), "Magyar");
        assert_eq!(normalize_pref("HU"), Some("hu"));
        assert_eq!(lang_from_tag("hu_HU.UTF-8"), Some(hu));
        assert_eq!(lang_from_tag("hu-HU"), Some(hu));
        assert_eq!(first_supported("hu-HU\r\nen-US"), Some(hu));
        assert_eq!(tr(hu, "Rapport de l'utilisateur.pdf"), "Rapport de l'utilisateur.pdf");
        // one (1), other (0, 2+)
        assert_eq!((0..=3).map(|n| (hu.0.plural)(n)).collect::<Vec<_>>(), [1, 0, 1, 1]);
        let mut app = crate::PdfCraftApp::default();
        app.set_option("language", "hu").unwrap();
        assert_eq!(app.language, "hu");
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.language, "hu");
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
        let uk = Lang::from_code("uk").expect("uk registered");
        assert_eq!(uk.0.plural_forms(), 3);
        for (counts, form) in [
            (vec![1, 21, 31, 101, 121, 1001], 0),
            (vec![2, 3, 4, 22, 23, 24, 102, 104, 122, 1002], 1),
            (vec![0, 5, 10, 11, 12, 13, 14, 19, 20, 25, 100, 110, 111, 112, 114, 1000, u64::MAX], 2),
        ] {
            for n in counts {
                assert_eq!((uk.0.plural)(n), form, "{n}");
            }
        }
    }

    fn arabic() -> Lang {
        Lang::from_code("ar").expect("ar registered")
    }

    #[test]
    fn arabic_is_registered_and_persists() {
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
    }

    fn persian() -> Lang {
        Lang::from_code("fa").expect("fa registered")
    }

    #[test]
    fn persian_is_registered_and_persists() {
        let fa = persian();
        assert_eq!(fa.name(), "فارسی");
        assert!(fa.rtl());
        // CLDR `fa` (one: i = 0 or n = 1): 0 and 1 are singular, every other count the plural.
        assert_eq!(fa.0.plural_forms(), 2);
        assert_eq!([0, 1, 2, 11].map(|n| (fa.0.plural)(n)), [0, 0, 1, 1]);
        assert_eq!(normalize_pref("FA"), Some("fa"));
        for tag in ["fa", "fa-IR", "fa_IR.UTF-8", "FA-af", "fa_IR@calendar=persian"] {
            assert_eq!(lang_from_tag(tag), Some(fa), "{tag}");
        }
        assert_eq!(first_supported("fa-IR\r\nen-US"), Some(fa));
        // Lookups return Persian (single words in display order).
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

    /// The translation gaps report (`cargo xtask i18n-report`, a step of `cargo xtask ci`): for
    /// each language, how many English strings it has no translation for, and the first few.
    /// It never fails: a missing string shows in English.
    ///
    /// The English strings are the UI source's literals, the commands, All tools, the engine's
    /// own wordings, and every key of every bundled catalog (which covers contextual, plural and
    /// dynamically looked-up strings the scan can't see).
    #[test]
    fn translation_gaps_report() {
        use std::collections::BTreeSet;
        const SHOWN: usize = 3;
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
        for language in &LANGUAGES {
            let (entries, _) = parse_entries(language.source, language.plural_forms());
            keys.extend(entries.into_iter().filter(|e| e.context != "@id").map(|e| (e.context, e.source)));
        }
        let mut report = format!("Translation gaps: {} English strings\n", keys.len());
        for lang in Lang::all().filter(|l| *l != Lang::EN) {
            let catalog = lang.catalog();
            let missing: Vec<&(String, String)> = keys
                .iter()
                .filter(|(context, source)| match context.as_str() {
                    "" => catalog.plain(source).is_none(),
                    "@plural" => source.split_once('|').is_none_or(|(one, other)| catalog.plural(one, other, 0).is_none()),
                    // A contextual lookup falls back to the plain translation.
                    context => catalog.contextual(context, source).is_none() && catalog.plain(source).is_none(),
                })
                .collect();
            let mut line = format!("  {:<8} {:>5} missing", lang.code(), missing.len());
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
        println!("{REPORT_START}\n{report}{REPORT_END}");
    }

    /// The lines `cargo xtask i18n-report` prints from the test output lie between these.
    const REPORT_START: &str = "=== translation gaps ===";
    const REPORT_END: &str = "=== end of translation gaps ===";

    /// Preferences ▸ Date format ▸ Language offers exactly the interface languages, in order.
    #[test]
    fn date_languages_are_the_interface_languages() {
        let interface: Vec<(&str, &str)> = Lang::all().map(|l| (l.code(), l.name())).collect();
        let dates: Vec<(&str, &str)> = pdfcraft_engine::dates::DATE_LANGUAGES.iter().map(|l| (l.code, l.name)).collect();
        assert_eq!(dates, interface);
    }
}

#[cfg(test)]
mod picker_order_tests {
    use super::*;

    #[test]
    fn picker_lists_english_first_then_names_alphabetically() {
        let order = Lang::for_picker();
        assert_eq!(order.first().map(|l| l.code()), Some(Lang::EN.code()));
        assert_eq!(order.len(), Lang::all().count());
        let keys: Vec<String> = order[1..].iter().map(|l| name_sort_key(l.name())).collect();
        assert!(keys.windows(2).all(|w| w[0] <= w[1]), "{keys:?}");
        assert_eq!(name_sort_key("Čeština"), "cestina");
        assert_eq!(name_sort_key("Türkçe"), "turkce");
    }
}
