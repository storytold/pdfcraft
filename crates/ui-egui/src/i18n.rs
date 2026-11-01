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
    /// The language is written right to left. Catalogs stay in logical order: the egui 0.37 fork
    /// owns bidi shaping/reordering (E5/E12), so preordering here would reverse Arabic twice.
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
    LangInfo { code: "ja", name: "日本語", source: include_str!("i18n/ja.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Simplified Chinese; `zh`, `zh-CN`, `zh-SG` and `zh-Hans-*` locales resolve here (see `candidates`).
    LangInfo {
        code: "zh-hans",
        name: "简体中文",
        source: include_str!("i18n/zh-hans.tsv"),
        plural: plural_none,
        rtl: false,
        catalog: OnceLock::new(),
    },
    // Traditional Chinese in the vocabulary used in Taiwan; `zh-TW`, `zh-HK`, `zh-MO` and `zh-Hant-*`
    // locales all resolve here (see `candidates`).
    LangInfo {
        code: "zh-hant",
        name: "繁體中文",
        source: include_str!("i18n/zh-hant.tsv"),
        plural: plural_none,
        rtl: false,
        catalog: OnceLock::new(),
    },
    LangInfo { code: "cs", name: "Čeština", source: include_str!("i18n/cs.tsv"), plural: plural_cs, rtl: false, catalog: OnceLock::new() },
    // Brazilian Portuguese; `pt`, `pt-BR` and `pt-PT` locales all resolve here (see `candidates`).
    LangInfo {
        code: "pt-br",
        name: "Português (Brasil)",
        source: include_str!("i18n/pt-br.tsv"),
        plural: plural_pt,
        rtl: false,
        catalog: OnceLock::new(),
    },
    // German (informal "du"); every `de-*` locale (`de-DE`, `de-AT`, `de-CH` ...) resolves here.
    LangInfo { code: "de", name: "Deutsch", source: include_str!("i18n/de.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Spanish (European vocabulary); every `es-*` locale (`es-ES`, `es-MX`, `es-419` ...) resolves here.
    LangInfo { code: "es", name: "Español", source: include_str!("i18n/es.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // French; every `fr-*` locale (`fr-FR`, `fr-CA`, `fr-BE` ...) resolves here.
    LangInfo { code: "fr", name: "Français", source: include_str!("i18n/fr.tsv"), plural: plural_fr, rtl: false, catalog: OnceLock::new() },
    // Russian; every `ru-*` locale (`ru-RU`, `ru-BY`, `ru-KZ` ...) resolves here.
    LangInfo {
        code: "ru", name: "Русский", source: include_str!("i18n/ru.tsv"), plural: plural_russian, rtl: false, catalog: OnceLock::new()
    },
    // Kazakh; every `kk-*` locale (`kk-KZ`, `kk-KZ.UTF-8`) resolves here.
    LangInfo {
        code: "kk", name: "Қазақша", source: include_str!("i18n/kk.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new()
    },
    // Bulgarian; every `bg-*` locale (`bg-BG`) resolves here.
    LangInfo {
        code: "bg", name: "Български", source: include_str!("i18n/bg.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new()
    },
    // Telugu; every `te-*` locale (`te-IN`) resolves here.
    LangInfo {
        code: "te", name: "తెలుగు", source: include_str!("i18n/te.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new()
    },
    // Hungarian; every `hu-*` locale (`hu-HU`) resolves here.
    LangInfo { code: "hu", name: "Magyar", source: include_str!("i18n/hu.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Ukrainian; every `uk-*` locale (`uk-UA`, `uk_UA.UTF-8`) resolves here.
    LangInfo {
        code: "uk", name: "Українська", source: include_str!("i18n/uk.tsv"), plural: plural_ukrainian, rtl: false, catalog: OnceLock::new()
    },
    // Italian; every `it-*` locale (`it-IT`) resolves here.
    LangInfo { code: "it", name: "Italiano", source: include_str!("i18n/it.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Arabic (Modern Standard, Western digits); every `ar-*` locale (`ar-MA`, `ar-EG`, `ar-SA` ...)
    // resolves here. Right to left: see `LangInfo::rtl`.
    LangInfo { code: "ar", name: "العربية", source: include_str!("i18n/ar.tsv"), plural: plural_arabic, rtl: true, catalog: OnceLock::new() },
    // Central Kurdish (Sorani); every `ckb-*` locale (`ckb-IQ`, `ckb-IR`) resolves here. Right to
    // left like Arabic: see `LangInfo::rtl`. CLDR `ckb` counts have two forms (one, other).
    LangInfo {
        code: "ckb",
        name: "کوردیی ناوەندی",
        source: include_str!("i18n/ckb.tsv"),
        plural: plural_one_other,
        rtl: true,
        catalog: OnceLock::new(),
    },
    // Persian (Farsi); every `fa-*` locale (`fa-IR`, `fa_IR.UTF-8`, `fa-AF` ...) resolves here. Right
    // to left like Arabic: see `LangInfo::rtl`. CLDR `fa` counts have two forms (one: i = 0 or n = 1,
    // other), which for whole counts is the French rule, so `@plural` entries list two forms.
    LangInfo { code: "fa", name: "فارسی", source: include_str!("i18n/fa.tsv"), plural: plural_fr, rtl: true, catalog: OnceLock::new() },
    // Polish; every `pl-*` locale (`pl-pl`, `pl_pl.UTF-8`) resolves here.
    LangInfo { code: "pl", name: "Polski", source: include_str!("i18n/pl.tsv"), plural: plural_polish, rtl: false, catalog: OnceLock::new() },
    // Slovene; every `sl-*` locale (`sl-SI`) resolves here. Four plural forms (one, two, few, other).
    LangInfo { code: "sl", name: "Slovenščina", source: include_str!("i18n/sl.tsv"), plural: plural_sl, rtl: false, catalog: OnceLock::new() },
    LangInfo {
        code: "el", name: "Ελληνικά", source: include_str!("i18n/el.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new()
    },
    // Turkish; every `tr-*` locale (`tr-TR`, `tr_TR.UTF-8`) resolves here.
    LangInfo { code: "tr", name: "Türkçe", source: include_str!("i18n/tr.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Lithuanian; every `lt-*` locale (`lt-LT`, `lt_LT.UTF-8`) resolves here. Three plural forms (one, few, other).
    LangInfo { code: "lt", name: "Lietuvių", source: include_str!("i18n/lt.tsv"), plural: plural_lithuanian, rtl: false, catalog: OnceLock::new() },
    // Indonesian; every `id-*` locale (`id-ID`, `id_ID.UTF-8`) resolves here. CLDR `id` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "id", name: "Bahasa Indonesia", source: include_str!("i18n/id.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Vietnamese; every `vi-*` locale (`vi-VN`, `vi_VN.UTF-8`) resolves here. CLDR `vi` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "vi", name: "Tiếng Việt", source: include_str!("i18n/vi.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Finnish; every `fi-*` locale (`fi-FI`, `fi_FI.UTF-8`) resolves here. Two plural forms (one, other):
    // the partitive singular follows any number but 1 (`1 sivu`, `2 sivua`).
    LangInfo { code: "fi", name: "Suomi", source: include_str!("i18n/fi.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Albanian; every `sq-*` locale (`sq-AL`, `sq_AL.UTF-8`, `sq-XK`) resolves here. One/other plural, like English.
    LangInfo { code: "sq", name: "Shqip", source: include_str!("i18n/sq.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Thai; every `th-*` locale (`th-TH`, `th_TH.UTF-8`) resolves here. No grammatical plural.
    LangInfo { code: "th", name: "ไทย", source: include_str!("i18n/th.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Catalan; every `ca-*` locale (`ca-ES`, `ca_ES.UTF-8`, `ca_ES@valencia`) resolves here.
    LangInfo { code: "ca", name: "Català", source: include_str!("i18n/ca.tsv"), plural: plural_one_other, rtl: false, catalog: OnceLock::new() },
    // Romanian; every `ro-*` locale (`ro-RO`, `ro_RO.UTF-8`, `ro-MD`) resolves here. Three plural forms (one, few, other).
    LangInfo { code: "ro", name: "Română", source: include_str!("i18n/ro.tsv"), plural: plural_ro, rtl: false, catalog: OnceLock::new() },
    // Lao; every `lo-*` locale (`lo-LA`, `lo_LA.UTF-8` ...) resolves here.
    LangInfo {
        code: "lo", name: "ພາສາລາວ", source: include_str!("i18n/lo.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new()
    },
    // Hindi; every `hi-*` locale (`hi-IN`, `hi_IN.UTF-8`) resolves here. CLDR `hi` has "one"
    // (0 and 1) and "other", but the catalog's nouns are the same in both ("{n} पृष्ठ"), so
    // `@plural` entries list a single form.
    LangInfo {
        code: "hi", name: "हिन्दी", source: include_str!("i18n/hi.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new()
    },
    // Korean; every `ko-*` locale (`ko-KR`, `ko_KR.UTF-8`) resolves here. CLDR `ko` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "ko", name: "한국어", source: include_str!("i18n/ko.tsv"), plural: plural_none, rtl: false, catalog: OnceLock::new() },
    // Norwegian Bokmål; every `nb-*` locale resolves here, and so do general Norwegian (`no-*`) and
    // Nynorsk (`nn-*`), which have no catalog of their own (see `candidates`).
    LangInfo {
        code: "nb",
        name: "Norsk bokmål",
        source: include_str!("i18n/nb.tsv"),
        plural: plural_one_other,
        rtl: false,
        catalog: OnceLock::new(),
    },
];

impl LangInfo {
    /// How many plural forms the language's `@plural` entries list.
    fn plural_forms(&self) -> usize {
        (0..=1000).map(self.plural).max().unwrap_or(0).saturating_add(1)
    }

    /// Return the parsed catalogue, building it on first use and warning about entries that fall back to
    /// English.
    fn catalog(&self) -> &Catalog {
        self.catalog.get_or_init(|| {
            let (catalog, errors) = Catalog::parse(self.source, self.plural_forms());
            for error in errors {
                log::warn!("{} interface catalog: {error}; that entry shows in English", self.code);
            }
            // Catalogs keep logical text: the fork shapes and reorders each run once. Accessibility
            // names, copied text and search therefore see the same logical string as the source.
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
        Self::for_picker_by(Lang::name)
    }

    /// [`for_picker`](Self::for_picker), ordered by the names the picker actually shows.
    ///
    /// `shown` must return each language's displayed name. Pass [`Lang::picker_name`] to keep a
    /// Latin-script fallback with the other Latin names when a script's face is missing, rather
    /// than sorting it into a block of unrenderable labels.
    pub fn for_picker_by(mut shown: impl FnMut(Lang) -> &'static str) -> Vec<Lang> {
        let mut rest: Vec<Lang> = Lang::all().filter(|l| l.code() != Lang::EN.code()).collect();
        rest.sort_by_cached_key(|l| name_sort_key(shown(*l)));
        std::iter::once(Lang::EN).chain(rest).collect()
    }

    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// The language's name in English, used by [`picker_name`](Self::picker_name) when this
    /// build's fonts cannot draw [`name`](Self::name).
    pub fn english_name(self) -> &'static str {
        match self.code() {
            "en" => "English",
            "ja" => "Japanese",
            "zh-hans" => "Simplified Chinese",
            "zh-hant" => "Traditional Chinese",
            "cs" => "Czech",
            "pt-br" => "Portuguese (Brazil)",
            "de" => "German",
            "es" => "Spanish",
            "fr" => "French",
            "ru" => "Russian",
            "kk" => "Kazakh",
            "bg" => "Bulgarian",
            "te" => "Telugu",
            "hu" => "Hungarian",
            "uk" => "Ukrainian",
            "it" => "Italian",
            "ar" => "Arabic",
            "ckb" => "Central Kurdish",
            "fa" => "Persian",
            "pl" => "Polish",
            "sl" => "Slovenian",
            "el" => "Greek",
            "tr" => "Turkish",
            "lt" => "Lithuanian",
            "id" => "Indonesian",
            "vi" => "Vietnamese",
            "fi" => "Finnish",
            "nb" => "Norwegian Bokmål",
            "sq" => "Albanian",
            "th" => "Thai",
            "ca" => "Catalan",
            "ro" => "Romanian",
            "lo" => "Lao",
            "hi" => "Hindi",
            "ko" => "Korean",
            // Only reached if a language is added to `LANGUAGES` without an English name; the
            // code still tells the reader which entry it is.
            _ => self.0.code,
        }
    }

    /// The name to show in the language picker: the native name when the current fonts can draw
    /// it, otherwise the English name. With egui 0.37's default `MissingGlyphPolicy::Panic`, a
    /// native name whose script has no face (for example Hindi in a build without a Devanagari
    /// face and with installed fonts turned off) would otherwise abort while the list opens.
    pub fn picker_name(self, drawable: impl FnOnce(&str) -> bool) -> &'static str {
        if drawable(self.name()) { self.name() } else { self.english_name() }
    }

    /// Whether the language is written right to left. Lookups stay logical; the egui fork
    /// shapes and reorders each run for display (see [`LangInfo::rtl`]).
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
mod tests;

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
