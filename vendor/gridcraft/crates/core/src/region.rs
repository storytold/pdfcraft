//! Regional formats: how a region writes numbers, dates, times and money.
//!
//! Excel takes these from the operating system's regional settings; they are independent of the
//! interface language and of the formula language. A [`Region`] changes what people *type* into
//! a cell (`1.234,56`, `25/09/2026`) and what cells *show* (separators, month and day names,
//! AM/PM, the system short and long dates). It never changes what is stored: values are numbers,
//! and the number format codes saved in a file stay in their canonical en-US spelling.
//!
//! [`EN_US`] is the default and reproduces GridCraft's original behaviour exactly.
//!
//! # Where the values come from
//! The separators, month and weekday names, AM/PM words, currency symbols and the order of day,
//! month and year are the Unicode CLDR conventions (`unicode-org/cldr-json`:
//! `cldr-numbers-full/main/<locale>/numbers.json` and `cldr-dates-full/main/<locale>/ca-gregorian.json`),
//! which is also what macOS, Android and ICU ship. They were read from those files for en-GB,
//! en-IN, cs and ru, read from ICU's CLDR 48 data for the Swiss regions and nl-NL, and written
//! from the same conventions for the other regions. CLDR data is under the Unicode License v3,
//! reproduced in `NOTICE` (row in `ATTRIBUTION.md`).
//! Deliberate choices where operating systems differ from CLDR:
//! - Short dates use a four-digit year (`dd/mm/yyyy`, where CLDR has `dd/MM/y` or `dd/MM/yy`):
//!   Excel's short date is the system's, Windows writes four digits, and a two-digit year would
//!   not read back to the same date.
//! - The long date is CLDR's *full* date (with the weekday), like Windows en-US, except where
//!   Windows' long date differs: Russian, Japanese and Chinese have no weekday (`23 августа
//!   2005 г.`, `2005年8月23日`), as in Microsoft's National Language Support tables.
//! - Korean writes the short date `yyyy-mm-dd` (CLDR: `yy. M. d.`), also as in Microsoft's
//!   National Language Support tables.
//! - Month names for `mmm` are CLDR's abbreviated names (so en-GB has `Sept`), except
//!   Portuguese, which drops CLDR's period (`jan`, not `jan.`) as Excel in Brazil shows it.
//! - French and Portuguese (Portugal) group digits with U+00A0 (Windows), not CLDR's U+202F.
//!   Parsing accepts every space-like grouping character in those regions.
//! - The Swiss regions (en-CH, de-CH, fr-CH, it-CH; Liechtenstein uses de-CH) group digits with
//!   CLDR's ASCII apostrophe (`1'234.50`); parsing also accepts the typographic `’` that glibc
//!   uses. French (Switzerland) writes a decimal point like the other Swiss regions and glibc's
//!   `fr_CH`, where CLDR has a comma.
//!
//! Regions this module doesn't list (`de-AT`, `es-MX`, `en-CA`…) resolve to the main region of
//! their language through [`lookup`]; their own conventions (a Mexican decimal point) are not
//! modelled.

/// How digits are grouped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grouping {
    /// Groups of three (`1,234,567`).
    Thousands,
    /// The Indian system: three digits, then groups of two (`12,34,567`).
    Indian,
}

/// The order of the parts of a short date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateOrder {
    Mdy,
    Dmy,
    Ymd,
}

/// One region's conventions. See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    /// BCP 47 tag, `pt-BR`.
    pub tag: &'static str,
    /// The region's name in its own language.
    pub name: &'static str,
    /// Windows locale id, used by `[$-409]`-style format tags.
    pub lcid: u16,
    pub decimal: char,
    pub group: char,
    pub grouping: Grouping,
    pub date_order: DateOrder,
    pub date_sep: char,
    pub time_sep: char,
    /// The system short date as a canonical format code (`dd/mm/yyyy`): built-in format 14.
    pub short_date: &'static str,
    /// The system long date (`[$-F800]`).
    pub long_date: &'static str,
    /// The system short time. It contains `AM/PM` in regions that use a 12-hour clock.
    pub short_time: &'static str,
    /// The system long time (`[$-F400]`).
    pub long_time: &'static str,
    /// Built-in format 22: the short date and a 24-hour time.
    pub datetime: &'static str,
    pub months: [&'static str; 12],
    pub months_abbr: [&'static str; 12],
    /// The month names a date with a day uses (Russian `1 января`, Czech `1. ledna`), where they
    /// differ from `months`.
    pub months_genitive: Option<[&'static str; 12]>,
    /// Sunday first.
    pub weekdays: [&'static str; 7],
    pub weekdays_abbr: [&'static str; 7],
    pub am_pm: [&'static str; 2],
    pub currency: &'static str,
    /// The currency format with the currency's usual decimals (two, or none for the yen and the
    /// won), as a canonical code. Where it starts with a quote the symbol comes before the number,
    /// otherwise after.
    pub currency_format: &'static str,
    /// The same with no decimals (for a typed whole amount).
    pub currency_format_int: &'static str,
}

const EN_MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const EN_MONTHS_ABBR: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const EN_DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const EN_DAYS_ABBR: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const AM_PM: [&str; 2] = ["AM", "PM"];

/// English (United States): the default. Identical to the formats before regions existed.
pub static EN_US: Region = Region {
    tag: "en-US",
    name: "English (United States)",
    lcid: 0x0409,
    decimal: '.',
    group: ',',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Mdy,
    date_sep: '/',
    time_sep: ':',
    short_date: "m/d/yyyy",
    long_date: "dddd, mmmm dd, yyyy",
    short_time: "h:mm AM/PM",
    long_time: "h:mm:ss AM/PM",
    datetime: "m/d/yyyy h:mm",
    months: EN_MONTHS,
    months_abbr: EN_MONTHS_ABBR,
    months_genitive: None,
    weekdays: EN_DAYS,
    weekdays_abbr: EN_DAYS_ABBR,
    am_pm: AM_PM,
    currency: "$",
    currency_format: "\"$\"#,##0.00",
    currency_format_int: "\"$\"#,##0",
};

static EN_GB: Region = Region {
    tag: "en-GB",
    name: "English (United Kingdom)",
    lcid: 0x0809,
    decimal: '.',
    group: ',',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '/',
    time_sep: ':',
    short_date: "dd/mm/yyyy",
    long_date: "dddd, d mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd/mm/yyyy hh:mm",
    months: EN_MONTHS,
    months_abbr: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sept", "Oct", "Nov", "Dec"],
    months_genitive: None,
    weekdays: EN_DAYS,
    weekdays_abbr: EN_DAYS_ABBR,
    am_pm: AM_PM,
    currency: "£",
    currency_format: "\"£\"#,##0.00",
    currency_format_int: "\"£\"#,##0",
};

static EN_IN: Region = Region {
    tag: "en-IN",
    name: "English (India)",
    lcid: 0x4009,
    decimal: '.',
    group: ',',
    grouping: Grouping::Indian,
    date_order: DateOrder::Dmy,
    date_sep: '/',
    time_sep: ':',
    short_date: "dd/mm/yyyy",
    long_date: "dddd, d mmmm yyyy",
    short_time: "h:mm AM/PM",
    long_time: "h:mm:ss AM/PM",
    datetime: "dd/mm/yyyy h:mm",
    months: EN_MONTHS,
    months_abbr: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sept", "Oct", "Nov", "Dec"],
    months_genitive: None,
    weekdays: EN_DAYS,
    weekdays_abbr: EN_DAYS_ABBR,
    am_pm: AM_PM,
    currency: "₹",
    currency_format: "\"₹\"#,##0.00",
    currency_format_int: "\"₹\"#,##0",
};

static EN_CH: Region = Region {
    tag: "en-CH",
    name: "English (Switzerland)",
    // English (Switzerland) has no LCID of its own: Windows reports LOCALE_CUSTOM_UNSPECIFIED.
    lcid: 0x1000,
    decimal: '.',
    group: '\'',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '.',
    time_sep: ':',
    short_date: "dd.mm.yyyy",
    long_date: "dddd, d mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd.mm.yyyy hh:mm",
    months: EN_MONTHS,
    months_abbr: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sept", "Oct", "Nov", "Dec"],
    months_genitive: None,
    weekdays: EN_DAYS,
    weekdays_abbr: EN_DAYS_ABBR,
    am_pm: AM_PM,
    currency: "CHF",
    currency_format: "\"CHF\" #,##0.00",
    currency_format_int: "\"CHF\" #,##0",
};

static ES_ES: Region = Region {
    tag: "es-ES",
    name: "Español (España)",
    lcid: 0x0C0A,
    decimal: ',',
    group: '.',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '/',
    time_sep: ':',
    short_date: "dd/mm/yyyy",
    long_date: "dddd, d \"de\" mmmm \"de\" yyyy",
    short_time: "h:mm",
    long_time: "h:mm:ss",
    datetime: "dd/mm/yyyy h:mm",
    months: ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
    months_abbr: ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"],
    months_genitive: None,
    weekdays: ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"],
    weekdays_abbr: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    am_pm: ["a. m.", "p. m."],
    currency: "€",
    currency_format: "#,##0.00 \"€\"",
    currency_format_int: "#,##0 \"€\"",
};

static DE_DE: Region = Region {
    tag: "de-DE",
    name: "Deutsch (Deutschland)",
    lcid: 0x0407,
    decimal: ',',
    group: '.',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '.',
    time_sep: ':',
    short_date: "dd.mm.yyyy",
    long_date: "dddd, d. mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd.mm.yyyy hh:mm",
    months: ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    months_abbr: ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
    months_genitive: None,
    weekdays: ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"],
    weekdays_abbr: ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"],
    am_pm: AM_PM,
    currency: "€",
    currency_format: "#,##0.00 \"€\"",
    currency_format_int: "#,##0 \"€\"",
};

static DE_CH: Region = Region {
    tag: "de-CH",
    name: "Deutsch (Schweiz)",
    lcid: 0x0807,
    decimal: '.',
    group: '\'',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '.',
    time_sep: ':',
    short_date: "dd.mm.yyyy",
    long_date: "dddd, d. mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd.mm.yyyy hh:mm",
    months: ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    months_abbr: ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
    months_genitive: None,
    weekdays: ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"],
    weekdays_abbr: ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"],
    am_pm: AM_PM,
    currency: "CHF",
    currency_format: "\"CHF\" #,##0.00",
    currency_format_int: "\"CHF\" #,##0",
};

static FR_FR: Region = Region {
    tag: "fr-FR",
    name: "Français (France)",
    lcid: 0x040C,
    decimal: ',',
    group: '\u{a0}',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '/',
    time_sep: ':',
    short_date: "dd/mm/yyyy",
    long_date: "dddd d mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd/mm/yyyy hh:mm",
    months: ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    months_abbr: ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    months_genitive: None,
    weekdays: ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"],
    weekdays_abbr: ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."],
    am_pm: AM_PM,
    currency: "€",
    currency_format: "#,##0.00 \"€\"",
    currency_format_int: "#,##0 \"€\"",
};

static FR_CH: Region = Region {
    tag: "fr-CH",
    name: "Français (Suisse)",
    lcid: 0x100C,
    decimal: '.',
    group: '\'',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '.',
    time_sep: ':',
    short_date: "dd.mm.yyyy",
    long_date: "dddd, d mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd.mm.yyyy hh:mm",
    months: ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    months_abbr: ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    months_genitive: None,
    weekdays: ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"],
    weekdays_abbr: ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."],
    am_pm: AM_PM,
    currency: "CHF",
    currency_format: "#,##0.00 \"CHF\"",
    currency_format_int: "#,##0 \"CHF\"",
};

static IT_IT: Region = Region {
    tag: "it-IT",
    name: "Italiano (Italia)",
    lcid: 0x0410,
    decimal: ',',
    group: '.',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '/',
    time_sep: ':',
    short_date: "dd/mm/yyyy",
    long_date: "dddd d mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd/mm/yyyy hh:mm",
    months: ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
    months_abbr: ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
    months_genitive: None,
    weekdays: ["domenica", "lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato"],
    weekdays_abbr: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    am_pm: AM_PM,
    currency: "€",
    currency_format: "#,##0.00 \"€\"",
    currency_format_int: "#,##0 \"€\"",
};

static IT_CH: Region = Region {
    tag: "it-CH",
    name: "Italiano (Svizzera)",
    lcid: 0x0810,
    decimal: '.',
    group: '\'',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '.',
    time_sep: ':',
    short_date: "dd.mm.yyyy",
    long_date: "dddd, d mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd.mm.yyyy hh:mm",
    months: ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
    months_abbr: ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
    months_genitive: None,
    weekdays: ["domenica", "lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato"],
    weekdays_abbr: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    am_pm: AM_PM,
    currency: "CHF",
    currency_format: "\"CHF\" #,##0.00",
    currency_format_int: "\"CHF\" #,##0",
};

static NL_NL: Region = Region {
    tag: "nl-NL",
    name: "Nederlands (Nederland)",
    lcid: 0x0413,
    decimal: ',',
    group: '.',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '-',
    time_sep: ':',
    short_date: "dd-mm-yyyy",
    long_date: "dddd d mmmm yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd-mm-yyyy hh:mm",
    months: ["januari", "februari", "maart", "april", "mei", "juni", "juli", "augustus", "september", "oktober", "november", "december"],
    months_abbr: ["jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec"],
    months_genitive: None,
    weekdays: ["zondag", "maandag", "dinsdag", "woensdag", "donderdag", "vrijdag", "zaterdag"],
    weekdays_abbr: ["zo", "ma", "di", "wo", "do", "vr", "za"],
    am_pm: AM_PM,
    currency: "€",
    currency_format: "\"€\" #,##0.00",
    currency_format_int: "\"€\" #,##0",
};

static PT_BR: Region = Region {
    tag: "pt-BR",
    name: "Português (Brasil)",
    lcid: 0x0416,
    decimal: ',',
    group: '.',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '/',
    time_sep: ':',
    short_date: "dd/mm/yyyy",
    long_date: "dddd, d \"de\" mmmm \"de\" yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd/mm/yyyy hh:mm",
    months: ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"],
    months_abbr: ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"],
    months_genitive: None,
    weekdays: ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"],
    weekdays_abbr: ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"],
    am_pm: AM_PM,
    currency: "R$",
    currency_format: "\"R$\" #,##0.00",
    currency_format_int: "\"R$\" #,##0",
};

static PT_PT: Region = Region {
    tag: "pt-PT",
    name: "Português (Portugal)",
    lcid: 0x0816,
    decimal: ',',
    group: '\u{a0}',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '/',
    time_sep: ':',
    short_date: "dd/mm/yyyy",
    long_date: "dddd, d \"de\" mmmm \"de\" yyyy",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd/mm/yyyy hh:mm",
    months: ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"],
    months_abbr: ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"],
    months_genitive: None,
    weekdays: ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"],
    weekdays_abbr: ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"],
    am_pm: AM_PM,
    currency: "€",
    currency_format: "#,##0.00 \"€\"",
    currency_format_int: "#,##0 \"€\"",
};

static RU_RU: Region = Region {
    tag: "ru-RU",
    name: "Русский (Россия)",
    lcid: 0x0419,
    decimal: ',',
    group: '\u{a0}',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '.',
    time_sep: ':',
    short_date: "dd.mm.yyyy",
    long_date: "d mmmm yyyy\" г.\"",
    short_time: "hh:mm",
    long_time: "hh:mm:ss",
    datetime: "dd.mm.yyyy hh:mm",
    months: ["январь", "февраль", "март", "апрель", "май", "июнь", "июль", "август", "сентябрь", "октябрь", "ноябрь", "декабрь"],
    months_abbr: ["янв.", "февр.", "март", "апр.", "май", "июнь", "июль", "авг.", "сент.", "окт.", "нояб.", "дек."],
    months_genitive: Some(["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"]),
    weekdays: ["воскресенье", "понедельник", "вторник", "среда", "четверг", "пятница", "суббота"],
    weekdays_abbr: ["вс", "пн", "вт", "ср", "чт", "пт", "сб"],
    am_pm: AM_PM,
    currency: "₽",
    currency_format: "#,##0.00\u{a0}\"₽\"",
    currency_format_int: "#,##0\u{a0}\"₽\"",
};

static CS_CZ: Region = Region {
    tag: "cs-CZ",
    name: "Čeština (Česko)",
    lcid: 0x0405,
    decimal: ',',
    group: '\u{a0}',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Dmy,
    date_sep: '.',
    time_sep: ':',
    short_date: "dd.mm.yyyy",
    long_date: "dddd d. mmmm yyyy",
    short_time: "h:mm",
    long_time: "h:mm:ss",
    datetime: "dd.mm.yyyy h:mm",
    months: ["leden", "únor", "březen", "duben", "květen", "červen", "červenec", "srpen", "září", "říjen", "listopad", "prosinec"],
    months_abbr: ["led", "úno", "bře", "dub", "kvě", "čvn", "čvc", "srp", "zář", "říj", "lis", "pro"],
    months_genitive: Some(["ledna", "února", "března", "dubna", "května", "června", "července", "srpna", "září", "října", "listopadu", "prosince"]),
    weekdays: ["neděle", "pondělí", "úterý", "středa", "čtvrtek", "pátek", "sobota"],
    weekdays_abbr: ["ne", "po", "út", "st", "čt", "pá", "so"],
    am_pm: ["dop.", "odp."],
    currency: "Kč",
    currency_format: "#,##0.00 \"Kč\"",
    currency_format_int: "#,##0 \"Kč\"",
};

static JA_JP: Region = Region {
    tag: "ja-JP",
    name: "日本語 (日本)",
    lcid: 0x0411,
    decimal: '.',
    group: ',',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Ymd,
    date_sep: '/',
    time_sep: ':',
    short_date: "yyyy/m/d",
    long_date: "yyyy\"年\"m\"月\"d\"日\"",
    short_time: "h:mm",
    long_time: "h:mm:ss",
    datetime: "yyyy/m/d h:mm",
    months: ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"],
    months_abbr: ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"],
    months_genitive: None,
    weekdays: ["日曜日", "月曜日", "火曜日", "水曜日", "木曜日", "金曜日", "土曜日"],
    weekdays_abbr: ["日", "月", "火", "水", "木", "金", "土"],
    am_pm: ["午前", "午後"],
    currency: "¥",
    currency_format: "\"¥\"#,##0",
    currency_format_int: "\"¥\"#,##0",
};

static KO_KR: Region = Region {
    tag: "ko-KR",
    name: "한국어 (대한민국)",
    lcid: 0x0412,
    decimal: '.',
    group: ',',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Ymd,
    date_sep: '-',
    time_sep: ':',
    short_date: "yyyy-mm-dd",
    long_date: "yyyy\"년\" m\"월\" d\"일\" dddd",
    short_time: "AM/PM h:mm",
    long_time: "AM/PM h:mm:ss",
    datetime: "yyyy-mm-dd hh:mm",
    months: ["1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월"],
    months_abbr: ["1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월"],
    months_genitive: None,
    weekdays: ["일요일", "월요일", "화요일", "수요일", "목요일", "금요일", "토요일"],
    weekdays_abbr: ["일", "월", "화", "수", "목", "금", "토"],
    am_pm: ["오전", "오후"],
    currency: "₩",
    currency_format: "\"₩\"#,##0",
    currency_format_int: "\"₩\"#,##0",
};

static ZH_CN: Region = Region {
    tag: "zh-CN",
    name: "中文 (中国)",
    lcid: 0x0804,
    decimal: '.',
    group: ',',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Ymd,
    date_sep: '/',
    time_sep: ':',
    short_date: "yyyy/m/d",
    long_date: "yyyy\"年\"m\"月\"d\"日\"",
    short_time: "h:mm",
    long_time: "h:mm:ss",
    datetime: "yyyy/m/d h:mm",
    months: ["一月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "十一月", "十二月"],
    months_abbr: ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"],
    months_genitive: None,
    weekdays: ["星期日", "星期一", "星期二", "星期三", "星期四", "星期五", "星期六"],
    weekdays_abbr: ["周日", "周一", "周二", "周三", "周四", "周五", "周六"],
    am_pm: ["上午", "下午"],
    currency: "¥",
    currency_format: "\"¥\"#,##0.00",
    currency_format_int: "\"¥\"#,##0",
};

static ZH_TW: Region = Region {
    tag: "zh-TW",
    name: "中文 (台灣)",
    lcid: 0x0404,
    decimal: '.',
    group: ',',
    grouping: Grouping::Thousands,
    date_order: DateOrder::Ymd,
    date_sep: '/',
    time_sep: ':',
    short_date: "yyyy/m/d",
    long_date: "yyyy\"年\"m\"月\"d\"日\"",
    short_time: "AM/PM hh:mm",
    long_time: "AM/PM hh:mm:ss",
    datetime: "yyyy/m/d hh:mm",
    months: ["一月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "十一月", "十二月"],
    months_abbr: ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"],
    months_genitive: None,
    weekdays: ["星期日", "星期一", "星期二", "星期三", "星期四", "星期五", "星期六"],
    weekdays_abbr: ["週日", "週一", "週二", "週三", "週四", "週五", "週六"],
    am_pm: ["上午", "下午"],
    currency: "NT$",
    currency_format: "\"NT$\"#,##0.00",
    currency_format_int: "\"NT$\"#,##0",
};

static ALL: [&Region; 20] = [
    &EN_US, &EN_GB, &EN_IN, &EN_CH, &ES_ES, &DE_DE, &DE_CH, &FR_FR, &FR_CH, &IT_IT, &IT_CH, &NL_NL, &PT_BR, &PT_PT, &RU_RU, &CS_CZ, &JA_JP, &KO_KR,
    &ZH_CN, &ZH_TW,
];

/// Every region, English (United States) first.
pub fn all() -> &'static [&'static Region] {
    &ALL
}

/// The default region, English (United States).
pub fn default_region() -> &'static Region {
    &EN_US
}

/// The region with exactly this tag (`pt-BR`; case and `_` don't matter).
pub fn find(tag: &str) -> Option<&'static Region> {
    let t = tag.trim().replace('_', "-");
    ALL.iter().copied().find(|r| r.tag.eq_ignore_ascii_case(&t))
}

/// The region for a tag as operating systems and browsers write it: `pt-BR`, `pt_BR.UTF-8`,
/// `de`, `en-AU`, `zh-Hant`, `sr_RS@latin`. An exact match wins; otherwise the language's main
/// region (with the usual variants: `en-AU`→`en-GB`, `pt-AO`→`pt-PT`, `zh-Hant`→`zh-TW`).
/// An unknown language is English (United States).
pub fn lookup(tag: &str) -> &'static Region {
    if let Some(r) = find(tag) {
        return r;
    }
    let base = tag.split(['.', '@']).next().unwrap_or("");
    let norm = base.trim().replace('_', "-");
    if let Some(r) = find(&norm) {
        return r;
    }
    let mut parts = norm.split('-').filter(|p| !p.is_empty());
    let lang = parts.next().unwrap_or("").to_ascii_lowercase();
    let mut script = String::new();
    let mut country = String::new();
    for p in parts {
        // An extension or private-use singleton (`-u-ca-gregory`, `-x-…`) ends the region part.
        if p.len() == 1 {
            break;
        }
        if p.len() == 4 && p.chars().all(|c| c.is_ascii_alphabetic()) {
            script = p.to_ascii_lowercase();
        } else if (p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic())) || (p.len() == 3 && p.chars().all(|c| c.is_ascii_digit())) {
            country = p.to_ascii_uppercase();
        }
    }
    let tag = match lang.as_str() {
        "en" => match country.as_str() {
            "GB" | "AU" | "NZ" | "IE" => "en-GB",
            "IN" => "en-IN",
            "CH" | "LI" => "en-CH",
            _ => "en-US",
        },
        "pt" => match country.as_str() {
            "AO" | "CH" | "CV" | "FR" | "GQ" | "GW" | "LU" | "MO" | "MZ" | "ST" | "TL" | "PT" => "pt-PT",
            _ => "pt-BR",
        },
        "zh" => match country.as_str() {
            // An explicit territory wins over the script (`zh-Hant-CN` is China's conventions).
            "CN" | "SG" => "zh-CN",
            "TW" | "HK" | "MO" => "zh-TW",
            _ if script == "hant" => "zh-TW",
            _ => "zh-CN",
        },
        "es" => "es-ES",
        "de" => match country.as_str() {
            "CH" | "LI" => "de-CH",
            _ => "de-DE",
        },
        "fr" => match country.as_str() {
            "CH" | "LI" => "fr-CH",
            _ => "fr-FR",
        },
        "it" => match country.as_str() {
            "CH" | "LI" => "it-CH",
            _ => "it-IT",
        },
        "nl" => "nl-NL",
        "ru" => "ru-RU",
        "cs" => "cs-CZ",
        "ja" => "ja-JP",
        "ko" => "ko-KR",
        _ => "en-US",
    };
    find(tag).unwrap_or(&EN_US)
}

impl Region {
    /// True for the characters that separate groups of digits in this region. A region that
    /// groups with a space also accepts the other space-like characters (NBSP, narrow NBSP, thin
    /// space), as people type and paste any of them; one that groups with an apostrophe accepts
    /// both `'` and `’`.
    pub fn is_group(&self, c: char) -> bool {
        c == self.group || (is_space_like(self.group) && is_space_like(c)) || (is_apostrophe_like(self.group) && is_apostrophe_like(c))
    }

    /// True when the currency symbol comes before the number.
    pub fn currency_first(&self) -> bool {
        self.currency_format.starts_with('"')
    }

    /// Month number (1–12) for a month name as typed: the full name, the abbreviation (with or
    /// without its period) or any prefix of the full name of three or more letters (`Sept`),
    /// ignoring case. `word` is one word: the genitive form used after a day (`января` in
    /// `1 января`) matches too, and the caller reads the day itself.
    pub fn month_from_name(&self, word: &str) -> Option<u32> {
        let w = word.trim().to_lowercase();
        // Where the abbreviations end in a period (`сент.`, `janv.`), any abbreviation may be typed
        // with one (`мар.`, `juin.`). Elsewhere a period is not part of a month name, so en-US
        // `Mar. 4, 2026` stays text as before.
        let dotted = self.months_abbr.iter().any(|a| a.ends_with('.'));
        let stem = if dotted { w.trim_end_matches('.') } else { w.as_str() };
        if stem.is_empty() {
            return None;
        }
        let long_enough = stem.chars().count() >= 3;
        for (i, full) in self.months.iter().enumerate() {
            let full_l = full.to_lowercase();
            let abbr_l = self.months_abbr.get(i).map(|a| a.to_lowercase()).unwrap_or_default();
            let gen_l = self.months_genitive.as_ref().and_then(|g| g.get(i)).map(|g| g.to_lowercase());
            let hit = w == full_l
                || w == abbr_l
                || stem == abbr_l.trim_end_matches('.')
                || gen_l.as_deref() == Some(w.as_str())
                || (long_enough && full_l.starts_with(stem))
                || (long_enough && gen_l.as_deref().is_some_and(|g| g.starts_with(stem)));
            if hit {
                return Some(i as u32 + 1);
            }
        }
        None
    }

    /// The name of month `month` (1–12) for `mmmm`; `with_day` selects the genitive form where
    /// the region has one.
    pub fn month_name(&self, month: u32, with_day: bool) -> &'static str {
        let i = (month as usize).wrapping_sub(1);
        if with_day && let Some(g) = &self.months_genitive {
            return g.get(i).copied().unwrap_or("");
        }
        self.months.get(i).copied().unwrap_or("")
    }

    /// True when the short time uses a 12-hour clock.
    pub fn is_12_hour(&self) -> bool {
        self.short_time.contains("AM/PM")
    }
}

fn is_space_like(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\u{2009}')
}

fn is_apostrophe_like(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_accepts_system_and_browser_tags() {
        let cases = [
            ("pt-BR", "pt-BR"),
            ("pt_BR.UTF-8", "pt-BR"),
            ("pt", "pt-BR"),
            ("pt-PT", "pt-PT"),
            ("pt-AO", "pt-PT"),
            ("de", "de-DE"),
            ("de-AT", "de-DE"),
            ("de_CH.UTF-8", "de-CH"),
            ("de-LI", "de-CH"),
            ("fr-CH", "fr-CH"),
            ("fr-LI", "fr-CH"),
            ("fr-BE", "fr-FR"),
            ("it_CH.UTF-8", "it-CH"),
            ("it-LI", "it-CH"),
            ("en-CH", "en-CH"),
            ("nl_NL.UTF-8", "nl-NL"),
            ("nl", "nl-NL"),
            ("en-AU", "en-GB"),
            ("en_GB.UTF-8", "en-GB"),
            ("en-IN", "en-IN"),
            ("en-CA", "en-US"),
            ("zh-Hant", "zh-TW"),
            ("zh-HK", "zh-TW"),
            ("zh", "zh-CN"),
            // Extensions don't replace the region or script; an explicit territory beats the script.
            ("en-GB-u-ca-gregory", "en-GB"),
            ("zh-Hant-u-nu-latn", "zh-TW"),
            ("zh-Hant-CN", "zh-CN"),
            ("zh-Hans-TW", "zh-TW"),
            ("cs_CZ.UTF-8", "cs-CZ"),
            ("C", "en-US"),
            ("", "en-US"),
            ("xx-YY", "en-US"),
        ];
        for (tag, expected) in cases {
            assert_eq!(lookup(tag).tag, expected, "{tag}");
        }
        assert_eq!(find("DE-de").map(|r| r.tag), Some("de-DE"));
        assert!(find("xx").is_none());
    }

    #[test]
    fn every_region_is_consistent() {
        let mut tags = std::collections::HashSet::new();
        for r in all() {
            assert!(tags.insert(r.tag), "duplicate {}", r.tag);
            assert_ne!(r.decimal, r.group, "{}", r.tag);
            assert!(r.currency_format.contains(r.currency), "{}", r.tag);
            assert!(r.months.iter().all(|m| !m.is_empty()) && r.months_abbr.iter().all(|m| !m.is_empty()), "{}", r.tag);
            assert!(r.weekdays.iter().all(|m| !m.is_empty()) && r.weekdays_abbr.iter().all(|m| !m.is_empty()), "{}", r.tag);
            assert_eq!(find(r.tag).map(|x| x.tag), Some(r.tag));
        }
    }

    #[test]
    fn month_names_match_loosely() {
        let de = find("de-DE").unwrap();
        assert_eq!(de.month_from_name("März"), Some(3));
        assert_eq!(de.month_from_name("mär"), Some(3));
        assert_eq!(de.month_from_name("DEZ"), Some(12));
        assert_eq!(de.month_from_name("de"), None);
        let gb = find("en-GB").unwrap();
        assert_eq!(gb.month_from_name("Sept"), Some(9));
        assert_eq!(gb.month_from_name("sep"), Some(9));
        let ru = find("ru-RU").unwrap();
        assert_eq!(ru.month_from_name("сентября"), Some(9));
        assert_eq!(ru.month_from_name("сент."), Some(9));
        // Abbreviations typed with a period the table doesn't carry.
        assert_eq!(ru.month_from_name("мар."), Some(3));
        assert_eq!(ru.month_from_name("июн."), Some(6));
        assert_eq!(find("fr-FR").unwrap().month_from_name("juin."), Some(6));
        assert_eq!(EN_US.month_from_name("Mar."), None);
        assert_eq!(de.month_from_name("."), None);
        assert_eq!(EN_US.month_from_name("Ma"), None);
        assert_eq!(EN_US.month_from_name("May"), Some(5));
    }

    #[test]
    fn space_like_and_apostrophe_groups_are_interchangeable() {
        let fr = find("fr-FR").unwrap();
        assert!(fr.is_group('\u{a0}') && fr.is_group(' ') && fr.is_group('\u{202f}'));
        assert!(!find("de-DE").unwrap().is_group(' '));
        let ch = find("de-CH").unwrap();
        assert!(ch.is_group('\'') && ch.is_group('\u{2019}'));
        assert!(!ch.is_group(' ') && !find("de-DE").unwrap().is_group('\''));
    }
}
