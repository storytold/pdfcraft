//! UI localisation. Strings in code stay English and are the default lookup keys; the craft-i18n
//! catalogs map them to display text at render time. Command ids, document text, file names, the
//! control channel, the CLI and MCP never see translated text, so agents and scripts are
//! unaffected. A string without a translation is shown in English, so coverage can grow
//! incrementally, and a missing translation never fails a build, a test or a release.
//!
//! The catalogs live in [storytold/craft-i18n](https://github.com/storytold/craft-i18n)
//! (`locales/<code>/common.tsv`, shared by the Crafting Apps, and `pdfcraft.tsv`), not in this
//! repository. `build.rs` embeds them from `CRAFT_I18N_DIR` (CI and releases check out the commit
//! in `craft-i18n.rev`); without it the build is English only. Lookup, fallback (PdfCraft's rows
//! over shared ones), plural forms (CLDR) and language names are the `craft-i18n` crate's
//! (craft-libs). Which languages are offered, how the system locale picks one, right to left and
//! `{name}` placeholders stay here.
//!
//! # Adding or changing a translation
//! In craft-i18n: add rows to `locales/<code>/pdfcraft.tsv` (its `docs/integration.md`). They show
//! here once `craft-i18n.rev` moves to a commit that has them. A feature PR here adds the English
//! only.
//!
//! # Adding a language
//! 1. craft-i18n gets `locales/<code>/pdfcraft.tsv` with its `@id language.name` row.
//! 2. Add one row to [`LANGUAGES`] (code, right to left), and its English name to
//!    [`Lang::english_name`].
//!
//! The Preferences dropdown and the system-locale match pick it up from there.
//!
//! # Looking strings up
//! - `tl!` / [`t`]: a plain string in the current language. [`tr`]: in a given one.
//! - [`tr_ctx`] (`tl_ctx!` in UI code): when one English word needs different translations.
//! - [`tr_id`]: a command-id keyed string with the English label as fallback (menu items), so a
//!   translation survives rewording of the English text and can differ per command.
//! - [`trn`]: plural-aware (`{n}` is filled in). [`fmt`]: fill `{name}` placeholders after [`tr`];
//!   translators may reorder placeholders freely.

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::{LazyLock, OnceLock};

use craft_i18n::I18n;

include!(concat!(env!("OUT_DIR"), "/craft_i18n.rs"));

/// The embedded catalogs, offering exactly the languages in [`LANGUAGES`]. Built on first use; a
/// locale is parsed on its first lookup.
static I18N: LazyLock<I18n> = LazyLock::new(|| {
    let offered: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
    let (i18n, problems) = I18n::new(CRAFT_I18N, &offered);
    for problem in problems {
        log::warn!("interface catalogs: {problem}");
    }
    i18n
});

/// The `language` preference value that follows the system locale.
pub const AUTO: &str = "auto";

/// One UI language PdfCraft offers, when its catalogs are embedded: what the app needs beyond
/// them. Its name and translations come from craft-i18n.
pub struct LangInfo {
    /// BCP 47 code, lowercase (`ja`, `zh-hans`, `pt-br`). Also the `language` preference value and
    /// the craft-i18n locale.
    pub code: &'static str,
    /// The language is written right to left. Catalogs stay in logical order: the egui 0.37 fork
    /// owns bidi shaping/reordering (E5/E12), so preordering here would reverse Arabic twice.
    pub rtl: bool,
}

/// How many languages [`LANGUAGES`] lists.
const LANGUAGE_COUNT: usize = 36;

/// The languages PdfCraft offers. English first: it is the fallback and the source language.
pub static LANGUAGES: [LangInfo; LANGUAGE_COUNT] = [
    LangInfo { code: "en", rtl: false },
    LangInfo { code: "ja", rtl: false },
    // Simplified Chinese; `zh`, `zh-CN`, `zh-SG` and `zh-Hans-*` locales resolve here (see `candidates`).
    LangInfo { code: "zh-hans", rtl: false },
    // Traditional Chinese in the vocabulary used in Taiwan; `zh-TW`, `zh-HK`, `zh-MO` and `zh-Hant-*`
    // locales all resolve here (see `candidates`).
    LangInfo { code: "zh-hant", rtl: false },
    LangInfo { code: "cs", rtl: false },
    // Brazilian Portuguese; `pt`, `pt-BR` and `pt-PT` locales all resolve here (see `candidates`).
    LangInfo { code: "pt-br", rtl: false },
    // German (informal "du"); every `de-*` locale (`de-DE`, `de-AT`, `de-CH` ...) resolves here.
    LangInfo { code: "de", rtl: false },
    // Spanish (European vocabulary); every `es-*` locale (`es-ES`, `es-MX`, `es-419` ...) resolves here.
    LangInfo { code: "es", rtl: false },
    // French; every `fr-*` locale (`fr-FR`, `fr-CA`, `fr-BE` ...) resolves here.
    LangInfo { code: "fr", rtl: false },
    // Russian; every `ru-*` locale (`ru-RU`, `ru-BY`, `ru-KZ` ...) resolves here.
    LangInfo { code: "ru", rtl: false },
    // Kazakh; every `kk-*` locale (`kk-KZ`, `kk-KZ.UTF-8`) resolves here.
    LangInfo { code: "kk", rtl: false },
    // Bulgarian; every `bg-*` locale (`bg-BG`) resolves here.
    LangInfo { code: "bg", rtl: false },
    // Telugu; every `te-*` locale (`te-IN`) resolves here.
    LangInfo { code: "te", rtl: false },
    // Hungarian; every `hu-*` locale (`hu-HU`) resolves here.
    LangInfo { code: "hu", rtl: false },
    // Ukrainian; every `uk-*` locale (`uk-UA`, `uk_UA.UTF-8`) resolves here.
    LangInfo { code: "uk", rtl: false },
    // Italian; every `it-*` locale (`it-IT`) resolves here.
    LangInfo { code: "it", rtl: false },
    // Arabic (Modern Standard, Western digits); every `ar-*` locale (`ar-MA`, `ar-EG`, `ar-SA` ...)
    // resolves here. Right to left: see `LangInfo::rtl`.
    LangInfo { code: "ar", rtl: true },
    // Central Kurdish (Sorani); every `ckb-*` locale (`ckb-IQ`, `ckb-IR`) resolves here. Right to
    // left like Arabic: see `LangInfo::rtl`. CLDR `ckb` counts have two forms (one, other).
    LangInfo { code: "ckb", rtl: true },
    // Persian (Farsi); every `fa-*` locale (`fa-IR`, `fa_IR.UTF-8`, `fa-AF` ...) resolves here. Right
    // to left like Arabic: see `LangInfo::rtl`. CLDR `fa` counts have two forms (one: i = 0 or n = 1,
    // other), which for whole counts is the French rule, so `@plural` entries list two forms.
    LangInfo { code: "fa", rtl: true },
    // Polish; every `pl-*` locale (`pl-pl`, `pl_pl.UTF-8`) resolves here.
    LangInfo { code: "pl", rtl: false },
    // Slovene; every `sl-*` locale (`sl-SI`) resolves here. Four plural forms (one, two, few, other).
    LangInfo { code: "sl", rtl: false },
    // Bosnian (Latin script, Ijekavian); every `bs-*` locale (`bs-BA`, `bs_BA.UTF-8`, `bs-Latn-BA`)
    // resolves here. Three plural forms (one, few, other).
    LangInfo { code: "bs", rtl: false },
    LangInfo { code: "el", rtl: false },
    // Turkish; every `tr-*` locale (`tr-TR`, `tr_TR.UTF-8`) resolves here. CLDR `tr` has two forms
    // (one, other); the catalog's counted nouns read the same in both, so its `@plural` rows repeat
    // the one form.
    LangInfo { code: "tr", rtl: false },
    // Lithuanian; every `lt-*` locale (`lt-LT`, `lt_LT.UTF-8`) resolves here. Three plural forms (one, few, other).
    LangInfo { code: "lt", rtl: false },
    // Indonesian; every `id-*` locale (`id-ID`, `id_ID.UTF-8`) resolves here. CLDR `id` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "id", rtl: false },
    // Vietnamese; every `vi-*` locale (`vi-VN`, `vi_VN.UTF-8`) resolves here. CLDR `vi` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "vi", rtl: false },
    // Finnish; every `fi-*` locale (`fi-FI`, `fi_FI.UTF-8`) resolves here. Two plural forms (one, other):
    // the partitive singular follows any number but 1 (`1 sivu`, `2 sivua`).
    LangInfo { code: "fi", rtl: false },
    // Albanian; every `sq-*` locale (`sq-AL`, `sq_AL.UTF-8`, `sq-XK`) resolves here. One/other plural, like English.
    LangInfo { code: "sq", rtl: false },
    // Thai; every `th-*` locale (`th-TH`, `th_TH.UTF-8`) resolves here. No grammatical plural.
    LangInfo { code: "th", rtl: false },
    // Catalan; every `ca-*` locale (`ca-ES`, `ca_ES.UTF-8`, `ca_ES@valencia`) resolves here.
    LangInfo { code: "ca", rtl: false },
    // Romanian; every `ro-*` locale (`ro-RO`, `ro_RO.UTF-8`, `ro-MD`) resolves here. Three plural forms (one, few, other).
    LangInfo { code: "ro", rtl: false },
    // Lao; every `lo-*` locale (`lo-LA`, `lo_LA.UTF-8` ...) resolves here.
    LangInfo { code: "lo", rtl: false },
    // Hindi; every `hi-*` locale (`hi-IN`, `hi_IN.UTF-8`) resolves here. CLDR `hi` has "one"
    // (0 and 1) and "other", but the catalog's nouns are the same in both ("{n} पृष्ठ"), so its
    // `@plural` rows repeat the one form.
    LangInfo { code: "hi", rtl: false },
    // Korean; every `ko-*` locale (`ko-KR`, `ko_KR.UTF-8`) resolves here. CLDR `ko` counts have
    // one form ("other"), so `@plural` entries list a single form.
    LangInfo { code: "ko", rtl: false },
    // Norwegian Bokmål; every `nb-*` locale resolves here, and so do general Norwegian (`no-*`) and
    // Nynorsk (`nn-*`), which have no catalog of their own (see `candidates`).
    LangInfo { code: "nb", rtl: false },
];

/// Per language of [`LANGUAGES`], the (context, source) keys of PdfCraft's own catalog as written
/// (escaped), read on first use ([`Lang::has_own`]).
static OWN_ROWS: [OnceLock<HashSet<(&'static str, &'static str)>>; LANGUAGE_COUNT] = [const { OnceLock::new() }; LANGUAGE_COUNT];

/// A language the UI can be shown in: English, or one of [`LANGUAGES`] whose catalogs are
/// embedded.
#[derive(Clone, Copy)]
pub struct Lang {
    lang: craft_i18n::Lang,
    /// The catalogs it is looked up in: the embedded ones ([`I18N`]), or a test fixture's.
    catalogs: Option<&'static I18n>,
}

impl std::fmt::Debug for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lang({})", self.code())
    }
}

impl PartialEq for Lang {
    fn eq(&self, other: &Self) -> bool {
        self.code() == other.code()
    }
}

impl Eq for Lang {}

impl Lang {
    pub const EN: Lang = Lang { lang: craft_i18n::Lang::EN, catalogs: None };

    fn i18n(self) -> &'static I18n {
        self.catalogs.unwrap_or_else(|| LazyLock::force(&I18N))
    }

    fn info(self) -> Option<&'static LangInfo> {
        let code = self.code();
        LANGUAGES.iter().find(|l| l.code == code)
    }

    pub fn code(self) -> &'static str {
        self.i18n().code(self.lang)
    }

    /// A language by its exact code (any case): one of [`LANGUAGES`] whose catalogs are embedded.
    pub fn from_code(code: &str) -> Option<Lang> {
        let info = LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code))?;
        I18N.from_code(info.code).map(|lang| Lang { lang, catalogs: None })
    }

    /// Resolve the `language` preference: a language code, or [`AUTO`] (and anything unknown,
    /// e.g. a code from a newer version) to follow the system locale.
    pub fn from_pref(pref: &str) -> Lang {
        Lang::from_code(pref).unwrap_or_else(system_lang)
    }

    /// Every language the interface can be shown in: [`LANGUAGES`] in order, those whose catalogs
    /// are embedded (English alone in a build without craft-i18n).
    pub fn all() -> impl Iterator<Item = Lang> {
        LANGUAGES.iter().filter_map(|l| Lang::from_code(l.code))
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

    /// The language's name in itself (its `@id language.name` row), shown in the Preferences
    /// dropdown.
    pub fn name(self) -> &'static str {
        self.i18n().name(self.lang)
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
            "bs" => "Bosnian",
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
            code => code,
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
        self.info().is_some_and(|l| l.rtl)
    }

    /// Whether PdfCraft's own catalog (`pdfcraft.tsv`, not the shared `common.tsv`) has the row
    /// (`context`, `source`). The shared catalog's `@id` rows are keyed by the other apps' command
    /// ids, which can name another command (in Bulgarian and Indonesian its `file.close` row holds
    /// an id rather than a label), so command-id lookups ask this; every `@id` row PdfCraft had is
    /// in its own catalog. A test fixture has no shared catalog: its rows are its own.
    fn has_own(self, context: &str, source: &str) -> bool {
        if let Some(fixture) = self.catalogs {
            return fixture.get(self.lang, context, source).is_some();
        }
        let code = self.code();
        let Some(slot) = LANGUAGES.iter().position(|l| l.code == code).and_then(|i| OWN_ROWS.get(i)) else { return false };
        let rows: &HashSet<(&str, &str)> = slot.get_or_init(|| {
            let app = CRAFT_I18N.iter().find(|(c, ..)| c.eq_ignore_ascii_case(code)).map_or("", |(_, _, app)| *app);
            app.lines()
                .filter(|l| !l.starts_with('#'))
                .filter_map(|l| {
                    let mut columns = l.split('\t');
                    Some((columns.next()?, columns.next()?))
                })
                .collect()
        });
        let escape = |s: &str| s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t");
        rows.contains(&(escape(context).as_str(), escape(source).as_str()))
    }

    /// The embedded catalog texts this language reads, the shared one then PdfCraft's; none for
    /// English. For checks that the interface fonts draw every translation.
    pub fn catalog_texts(self) -> impl Iterator<Item = &'static str> {
        let code = self.code();
        CRAFT_I18N.iter().filter(move |(c, ..)| c.eq_ignore_ascii_case(code)).flat_map(|(_, shared, app)| [*shared, *app])
    }
}

/// Whether this build embeds the craft-i18n catalogs (`CRAFT_I18N_DIR` was set). Without them the
/// interface is English only; tests that need a translation skip with a note.
pub fn catalogs_embedded() -> bool {
    !CRAFT_I18N.is_empty()
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
    if let Some(l) = env_lang(|var| std::env::var(var).ok()) {
        return l;
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

/// The language the locale variables name, `LC_ALL` first, then `LC_MESSAGES`, then `LANG`;
/// `var` reads one.
#[cfg(not(target_arch = "wasm32"))]
fn env_lang(var: impl Fn(&str) -> Option<String>) -> Option<Lang> {
    ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().find_map(|name| var(name).and_then(|v| language_override(&v)))
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
    lang.i18n().get(lang.lang, "", s).is_some()
}

/// Translate an English UI string into the current language ([`tr`] with [`current`]).
pub fn t(s: &str) -> &str {
    tr(current(), s)
}

/// The name of a keyboard key, distinct from a command such as Delete. Missing key-name
/// translations use the English key name, never a plain command translation.
pub fn key_name(s: &str) -> &str {
    let lang = current();
    lang.i18n().get(lang.lang, "key", s).unwrap_or(s)
}

/// Translate an English UI string; unknown strings come back unchanged.
pub fn tr(lang: Lang, s: &str) -> &str {
    lang.i18n().t(lang.lang, s)
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
    lang.i18n().tc(lang.lang, context, s)
}

/// A string keyed by its command id, falling back to the translation of the English `label`. Only
/// PdfCraft's own `@id` rows count ([`own_id`]).
pub fn tr_id<'a>(lang: Lang, id: &str, label: &'a str) -> &'a str {
    if own_id(lang, id, label) { lang.i18n().tid(lang.lang, id, label) } else { tr(lang, label) }
}

/// Whether `lang` has PdfCraft's own `@id` row for `id` ([`Lang::has_own`]: command ids are each
/// app's own) and it keeps the `{placeholders}` and trailing `…` of the English `label`. craft-i18n
/// can't check that, because an `@id` row's source is the id, so a row that doesn't is skipped
/// here and the label's translation shows instead.
fn own_id(lang: Lang, id: &str, label: &str) -> bool {
    lang.has_own("@id", id) && lang.i18n().get(lang.lang, "@id", id).is_some_and(|translated| same_shape(translated, label))
}

/// Whether `translated` has the `{placeholders}` (in any order) and the trailing `…` of `english`,
/// the check craft-i18n makes on every row whose source is English text, and no `{` left open or
/// `}` without its `{`.
fn same_shape(translated: &str, english: &str) -> bool {
    let placeholders = |text: &str| -> Option<Vec<String>> {
        let mut names = text.split('{').skip(1).map(|p| p.split_once('}').map(|(name, _)| name.to_owned())).collect::<Option<Vec<_>>>()?;
        if text.matches('}').count() != names.len() {
            return None;
        }
        names.sort_unstable();
        Some(names)
    };
    translated.ends_with('…') == english.ends_with('…') && placeholders(translated).is_some_and(|names| Some(names) == placeholders(english))
}

/// Fill `{name}` placeholders in one pass. Unknown placeholders are left as written, and inserted
/// values are never read as templates again, so a file name containing `{n}` stays intact. Unlike
/// the crate's `fmt` it adds no bidi isolates: the egui fork orders each run for display.
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
/// as whole phrases, both of them; otherwise the translated verb goes before the action. (The
/// shared catalog has "Undo {action}" without "Redo {action}" in some languages, where one phrase
/// alone would word Undo and Redo differently.)
pub fn command_label(text: &str) -> String {
    for (prefix, template) in [("Undo", "Undo {action}"), ("Redo", "Redo {action}")] {
        if let Some(action) = text.strip_prefix(prefix).and_then(|tail| tail.strip_prefix(' ')) {
            let action = action_label(action);
            if has(current(), "Undo {action}") && has(current(), "Redo {action}") {
                return fmt(t(template), &[("action", &action)]);
            }
            return format!("{} {action}", t(prefix));
        }
    }
    t(text).to_owned()
}

/// A registered command's menu label: its `@id` entry in PdfCraft's own catalog if there is one
/// that keeps the label's shape ([`own_id`]), otherwise [`command_label`] of the current English
/// label.
pub fn menu_label(id: &str, label: &str) -> String {
    let lang = current();
    match own_id(lang, id, label).then(|| lang.i18n().get(lang.lang, "@id", id)).flatten() {
        Some(translated) => translated.to_owned(),
        None => command_label(label),
    }
}

/// A plural-aware message: `one`/`other` are the English forms (with `{n}` where the count goes).
/// The form is the language's CLDR category; `{n}` is written in `0`–`9` without grouping or
/// isolate marks, as PdfCraft always has.
pub fn trn(lang: Lang, n: u64, one: &str, other: &str) -> String {
    lang.i18n().trn_plain(lang.lang, n, one, other)
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

/// A test language whose PdfCraft catalog is `source` (catalog format), so tests of lookup code
/// don't depend on the embedded catalogs' wording. `code` picks the CLDR plural rules.
#[cfg(test)]
pub(crate) fn fixture_lang(code: &'static str, source: &'static str) -> Lang {
    let embedded: &'static craft_i18n::Embedded = Box::leak(Box::new([(code, "", source)]));
    let (i18n, _) = I18n::new(embedded, &[code]);
    let i18n: &'static I18n = Box::leak(Box::new(i18n));
    let lang = i18n.from_code(code).unwrap_or(craft_i18n::Lang::EN);
    Lang { lang, catalogs: Some(i18n) }
}

/// For a test that needs the embedded catalogs: `false`, with a note, when none were embedded
/// (`CRAFT_I18N_DIR` unset), so the test can skip; the interface is then English only.
#[cfg(test)]
pub(crate) fn needs_catalogs() -> bool {
    if !catalogs_embedded() {
        eprintln!("skipped: built without craft-i18n (set CRAFT_I18N_DIR to a craft-i18n checkout); the interface is English only");
    }
    catalogs_embedded()
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
