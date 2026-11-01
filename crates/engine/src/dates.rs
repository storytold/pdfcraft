//! The date format Fill & Sign stamps (Preferences ▸ Date format), kept on the [`Session`] so
//! the app and the automation tools write the same text. Acrobat's Fill & Sign has no such
//! setting (only Acrobat Sign's account settings do); this one is per user and takes any
//! pattern in Acrobat's date letters, not just a fixed list, with month and weekday names in
//! any of [`DATE_LANGUAGES`].
//!
//! Patterns use `util.printd`'s date letters: `yyyy` `yy`, `mmmm` `mmm` `mm` `m`, `dddd` `ddd`
//! `dd` `d`. `\` shows the next character as is (`d \de mmmm` → `8 de October`).

use crate::Session;

/// The format until the user picks one (Acrobat's: US month/day/year).
pub const DEFAULT_DATE_FORMAT: &str = "m/d/yyyy";

/// Ready-made formats for Preferences ▸ Date format. Any other valid pattern works too.
pub const DATE_FORMATS: [&str; 14] = [
    "m/d/yyyy",
    "mm/dd/yyyy",
    "d/m/yyyy",
    "dd/mm/yyyy",
    "dd.mm.yyyy",
    "yyyy-mm-dd",
    "yyyy.mm.dd.",
    // Year first with slashes, as Taiwan, Japan and China write it (Windows' zh-TW short date).
    "yyyy/mm/dd",
    "yyyy/m/d",
    "d mmm yyyy",
    "d mmmm yyyy",
    "mmm d, yyyy",
    "mmmm d, yyyy",
    "dddd, mmmm d, yyyy",
];

/// The longest pattern kept (settings and tool arguments are untrusted).
pub const MAX_DATE_FORMAT_CHARS: usize = 64;

/// Month and weekday names in one language. Weekdays start on Sunday.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DateLanguage {
    /// The interface-language code where there is one (`en`, `cs`, `pt-br`, `zh-hans`, …).
    pub code: &'static str,
    /// Its name in itself, for pickers.
    pub name: &'static str,
    pub months: [&'static str; 12],
    /// The month after a day number (`mmmm` when the pattern also shows `d` or `dd`), where
    /// the language changes it: Czech `8. října`, Russian `8 октября`.
    pub months_with_day: Option<[&'static str; 12]>,
    pub months_short: [&'static str; 12],
    pub days: [&'static str; 7],
    pub days_short: [&'static str; 7],
}

const CJK_MONTHS: [&str; 12] = ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"];
const ZH_MONTHS: [&str; 12] = ["一月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "十一月", "十二月"];
const ZH_DAYS: [&str; 7] = ["星期日", "星期一", "星期二", "星期三", "星期四", "星期五", "星期六"];

const AR_MONTHS: [&str; 12] = ["يناير", "فبراير", "مارس", "أبريل", "مايو", "يونيو", "يوليو", "أغسطس", "سبتمبر", "أكتوبر", "نوفمبر", "ديسمبر"];
const AR_DAYS: [&str; 7] = ["الأحد", "الاثنين", "الثلاثاء", "الأربعاء", "الخميس", "الجمعة", "السبت"];

const CKB_MONTHS: [&str; 12] =
    ["کانوونی دووەم", "شوبات", "ئازار", "نیسان", "ئایار", "حوزەیران", "تەممووز", "ئاب", "ئەیلوول", "تشرینی یەکەم", "تشرینی دووەم", "کانوونی یەکەم"];
const CKB_DAYS: [&str; 7] = ["یەکشەممە", "دووشەممە", "سێشەممە", "چوارشەممە", "پێنجشەممە", "هەینی", "شەممە"];

/// The languages month and weekday names can be written in (Preferences ▸ Date format ▸ Language):
/// the interface languages, in the same order and with the same names.
pub static DATE_LANGUAGES: [DateLanguage; 35] = [
    DateLanguage {
        code: "en",
        name: "English",
        months: ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
        months_with_day: None,
        months_short: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
        days: ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"],
        days_short: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    },
    DateLanguage {
        code: "ja",
        name: "日本語",
        months: CJK_MONTHS,
        months_with_day: None,
        months_short: CJK_MONTHS,
        days: ["日曜日", "月曜日", "火曜日", "水曜日", "木曜日", "金曜日", "土曜日"],
        days_short: ["日", "月", "火", "水", "木", "金", "土"],
    },
    DateLanguage {
        code: "zh-hans",
        name: "简体中文",
        months: ZH_MONTHS,
        months_with_day: None,
        months_short: CJK_MONTHS,
        days: ZH_DAYS,
        days_short: ["周日", "周一", "周二", "周三", "周四", "周五", "周六"],
    },
    DateLanguage {
        code: "zh-hant",
        name: "繁體中文",
        months: ZH_MONTHS,
        months_with_day: None,
        months_short: CJK_MONTHS,
        days: ZH_DAYS,
        days_short: ["週日", "週一", "週二", "週三", "週四", "週五", "週六"],
    },
    DateLanguage {
        code: "cs",
        name: "Čeština",
        months: ["leden", "únor", "březen", "duben", "květen", "červen", "červenec", "srpen", "září", "říjen", "listopad", "prosinec"],
        months_with_day: Some([
            "ledna",
            "února",
            "března",
            "dubna",
            "května",
            "června",
            "července",
            "srpna",
            "září",
            "října",
            "listopadu",
            "prosince",
        ]),
        months_short: ["led", "úno", "bře", "dub", "kvě", "čvn", "čvc", "srp", "zář", "říj", "lis", "pro"],
        days: ["neděle", "pondělí", "úterý", "středa", "čtvrtek", "pátek", "sobota"],
        days_short: ["ne", "po", "út", "st", "čt", "pá", "so"],
    },
    DateLanguage {
        code: "pt-br",
        name: "Português (Brasil)",
        months: ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"],
        months_with_day: None,
        months_short: ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"],
        days: ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"],
        days_short: ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"],
    },
    DateLanguage {
        code: "de",
        name: "Deutsch",
        months: ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
        months_with_day: None,
        months_short: ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."],
        days: ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"],
        days_short: ["So.", "Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa."],
    },
    DateLanguage {
        code: "es",
        name: "Español",
        months: ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
        months_with_day: None,
        months_short: ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"],
        days: ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"],
        days_short: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    },
    DateLanguage {
        code: "fr",
        name: "Français",
        months: ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
        months_with_day: None,
        months_short: ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
        days: ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"],
        days_short: ["dim.", "lun.", "mar.", "mer.", "jeu.", "ven.", "sam."],
    },
    DateLanguage {
        code: "ru",
        name: "Русский",
        months: ["январь", "февраль", "март", "апрель", "май", "июнь", "июль", "август", "сентябрь", "октябрь", "ноябрь", "декабрь"],
        months_with_day: Some(["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"]),
        months_short: ["янв.", "февр.", "март", "апр.", "май", "июнь", "июль", "авг.", "сент.", "окт.", "нояб.", "дек."],
        days: ["воскресенье", "понедельник", "вторник", "среда", "четверг", "пятница", "суббота"],
        days_short: ["вс", "пн", "вт", "ср", "чт", "пт", "сб"],
    },
    DateLanguage {
        code: "kk",
        name: "Қазақша",
        months: ["қаңтар", "ақпан", "наурыз", "сәуір", "мамыр", "маусым", "шілде", "тамыз", "қыркүйек", "қазан", "қараша", "желтоқсан"],
        months_with_day: None,
        months_short: ["қаң", "ақп", "нау", "сәу", "мам", "мау", "шіл", "там", "қыр", "қаз", "қар", "жел"],
        days: ["жексенбі", "дүйсенбі", "сейсенбі", "сәрсенбі", "бейсенбі", "жұма", "сенбі"],
        days_short: ["жк", "дс", "сс", "ср", "бс", "жм", "сб"],
    },
    DateLanguage {
        code: "bg",
        name: "Български",
        months: ["януари", "февруари", "март", "април", "май", "юни", "юли", "август", "септември", "октомври", "ноември", "декември"],
        months_with_day: None,
        months_short: ["яну", "фев", "март", "апр", "май", "юни", "юли", "авг", "сеп", "окт", "ное", "дек"],
        days: ["неделя", "понеделник", "вторник", "сряда", "четвъртък", "петък", "събота"],
        days_short: ["нд", "пн", "вт", "ср", "чт", "пт", "сб"],
    },
    DateLanguage {
        code: "te",
        name: "తెలుగు",
        months: ["జనవరి", "ఫిబ్రవరి", "మార్చి", "ఏప్రిల్", "మే", "జూన్", "జులై", "ఆగస్టు", "సెప్టెంబర్", "అక్టోబర్", "నవంబర్", "డిసెంబర్"],
        months_with_day: None,
        months_short: ["జన", "ఫిబ్ర", "మార్చి", "ఏప్రి", "మే", "జూన్", "జులై", "ఆగ", "సెప్టెం", "అక్టో", "నవం", "డిసెం"],
        days: ["ఆదివారం", "సోమవారం", "మంగళవారం", "బుధవారం", "గురువారం", "శుక్రవారం", "శనివారం"],
        days_short: ["ఆది", "సోమ", "మంగళ", "బుధ", "గురు", "శుక్ర", "శని"],
    },
    DateLanguage {
        code: "hu",
        name: "Magyar",
        months: [
            "január",
            "február",
            "március",
            "április",
            "május",
            "június",
            "július",
            "augusztus",
            "szeptember",
            "október",
            "november",
            "december",
        ],
        months_with_day: None,
        months_short: ["jan.", "febr.", "márc.", "ápr.", "máj.", "jún.", "júl.", "aug.", "szept.", "okt.", "nov.", "dec."],
        days: ["vasárnap", "hétfő", "kedd", "szerda", "csütörtök", "péntek", "szombat"],
        days_short: ["V", "H", "K", "Sze", "Cs", "P", "Szo"],
    },
    DateLanguage {
        code: "uk",
        name: "Українська",
        months: ["січень", "лютий", "березень", "квітень", "травень", "червень", "липень", "серпень", "вересень", "жовтень", "листопад", "грудень"],
        months_with_day: Some([
            "січня",
            "лютого",
            "березня",
            "квітня",
            "травня",
            "червня",
            "липня",
            "серпня",
            "вересня",
            "жовтня",
            "листопада",
            "грудня",
        ]),
        months_short: ["січ.", "лют.", "бер.", "квіт.", "трав.", "черв.", "лип.", "серп.", "вер.", "жовт.", "лист.", "груд."],
        days: ["неділя", "понеділок", "вівторок", "середа", "четвер", "пʼятниця", "субота"],
        days_short: ["нд", "пн", "вт", "ср", "чт", "пт", "сб"],
    },
    DateLanguage {
        code: "it",
        name: "Italiano",
        months: ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
        months_with_day: None,
        months_short: ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
        days: ["domenica", "lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato"],
        days_short: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    },
    // Arabic has no shorter written forms of its month and weekday names.
    DateLanguage {
        code: "ar",
        name: "العربية",
        months: AR_MONTHS,
        months_with_day: None,
        months_short: AR_MONTHS,
        days: AR_DAYS,
        days_short: AR_DAYS,
    },
    // Central Kurdish (Sorani) has no shorter written forms of its month and weekday names either.
    DateLanguage {
        code: "ckb",
        name: "کوردیی ناوەندی",
        months: CKB_MONTHS,
        months_with_day: None,
        months_short: CKB_MONTHS,
        days: CKB_DAYS,
        days_short: CKB_DAYS,
    },
    // Persian (Farsi); Gregorian month and weekday names, right to left like Arabic and Kurdish.
    // Persian does not change the month form after a day number, so `months_with_day` is `None`.
    DateLanguage {
        code: "fa",
        name: "فارسی",
        months: ["ژانویه", "فوریه", "مارس", "آوریل", "مه", "ژوئن", "ژوئیه", "اوت", "سپتامبر", "اکتبر", "نوامبر", "دسامبر"],
        months_with_day: None,
        months_short: ["ژانویه", "فوریه", "مارس", "آوریل", "مه", "ژوئن", "ژوئیه", "اوت", "سپتامبر", "اکتبر", "نوامبر", "دسامبر"],
        days: ["یکشنبه", "دوشنبه", "سه‌شنبه", "چهارشنبه", "پنج‌شنبه", "جمعه", "شنبه"],
        days_short: ["یک", "دو", "سه", "چه", "پن", "جم", "شن"],
    },
    DateLanguage {
        code: "pl",
        name: "Polski",
        months: ["styczeń", "luty", "marzec", "kwiecień", "maj", "czerwiec", "lipiec", "sierpień", "wrzesień", "październik", "listopad", "grudzień"],
        months_with_day: Some([
            "stycznia",
            "lutego",
            "marca",
            "kwietnia",
            "maja",
            "czerwca",
            "lipca",
            "sierpnia",
            "września",
            "października",
            "listopada",
            "grudnia",
        ]),
        months_short: ["sty", "lut", "mar", "kwi", "maj", "cze", "lip", "sie", "wrz", "paź", "lis", "gru"],
        days: ["niedziela", "poniedziałek", "wtorek", "środa", "czwartek", "piątek", "sobota"],
        days_short: ["niedz.", "pon.", "wt.", "śr.", "czw.", "pt.", "sob."],
    },
    DateLanguage {
        code: "sl",
        name: "Slovenščina",
        months: ["januar", "februar", "marec", "april", "maj", "junij", "julij", "avgust", "september", "oktober", "november", "december"],
        months_with_day: Some([
            "januarja",
            "februarja",
            "marca",
            "aprila",
            "maja",
            "junija",
            "julija",
            "avgusta",
            "septembra",
            "oktobra",
            "novembra",
            "decembra",
        ]),
        months_short: ["jan.", "feb.", "mar.", "apr.", "maj", "jun.", "jul.", "avg.", "sep.", "okt.", "nov.", "dec."],
        days: ["nedelja", "ponedeljek", "torek", "sreda", "četrtek", "petek", "sobota"],
        days_short: ["ned.", "pon.", "tor.", "sre.", "čet.", "pet.", "sob."],
    },
    DateLanguage {
        code: "el",
        name: "Ελληνικά",
        months: [
            "Ιανουάριος",
            "Φεβρουάριος",
            "Μάρτιος",
            "Απρίλιος",
            "Μάιος",
            "Ιούνιος",
            "Ιούλιος",
            "Αύγουστος",
            "Σεπτέμβριος",
            "Οκτώβριος",
            "Νοέμβριος",
            "Δεκέμβριος",
        ],
        months_with_day: Some([
            "Ιανουαρίου",
            "Φεβρουαρίου",
            "Μαρτίου",
            "Απριλίου",
            "Μαΐου",
            "Ιουνίου",
            "Ιουλίου",
            "Αυγούστου",
            "Σεπτεμβρίου",
            "Οκτωβρίου",
            "Νοεμβρίου",
            "Δεκεμβρίου",
        ]),
        months_short: ["Ιαν", "Φεβ", "Μαρ", "Απρ", "Μάι", "Ιουν", "Ιουλ", "Αυγ", "Σεπ", "Οκτ", "Νοε", "Δεκ"],
        days: ["Κυριακή", "Δευτέρα", "Τρίτη", "Τετάρτη", "Πέμπτη", "Παρασκευή", "Σάββατο"],
        days_short: ["Κυρ", "Δευ", "Τρί", "Τετ", "Πέμ", "Παρ", "Σάβ"],
    },
    DateLanguage {
        code: "tr",
        name: "Türkçe",
        months: ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"],
        months_with_day: None,
        months_short: ["Oca", "Şub", "Mar", "Nis", "May", "Haz", "Tem", "Ağu", "Eyl", "Eki", "Kas", "Ara"],
        days: ["Pazar", "Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi"],
        days_short: ["Paz", "Pzt", "Sal", "Çar", "Per", "Cum", "Cmt"],
    },
    DateLanguage {
        code: "lt",
        name: "Lietuvių",
        months: ["sausis", "vasaris", "kovas", "balandis", "gegužė", "birželis", "liepa", "rugpjūtis", "rugsėjis", "spalis", "lapkritis", "gruodis"],
        months_with_day: Some([
            "sausio",
            "vasario",
            "kovo",
            "balandžio",
            "gegužės",
            "birželio",
            "liepos",
            "rugpjūčio",
            "rugsėjo",
            "spalio",
            "lapkričio",
            "gruodžio",
        ]),
        months_short: ["saus.", "vas.", "kov.", "bal.", "geg.", "birž.", "liep.", "rugpj.", "rugs.", "spal.", "lapkr.", "gruod."],
        days: ["sekmadienis", "pirmadienis", "antradienis", "trečiadienis", "ketvirtadienis", "penktadienis", "šeštadienis"],
        days_short: ["sk", "pr", "an", "tr", "kt", "pn", "št"],
    },
    // Indonesian. Month and weekday names are the same with and after a day number, so
    // `months_with_day` is `None`.
    DateLanguage {
        code: "id",
        name: "Bahasa Indonesia",
        months: ["Januari", "Februari", "Maret", "April", "Mei", "Juni", "Juli", "Agustus", "September", "Oktober", "November", "Desember"],
        months_with_day: None,
        months_short: ["Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des"],
        days: ["Minggu", "Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu"],
        days_short: ["Min", "Sen", "Sel", "Rab", "Kam", "Jum", "Sab"],
    },
    // Vietnamese: numeric month forms (`tháng 1` with a day, `Tháng 1` standing alone).
    // Fill & Sign still refuses any format whose final text fails the existing WinAnsi check
    // (for example weekday names); `dd.mm.yyyy` remains writable.
    DateLanguage {
        code: "vi",
        name: "Tiếng Việt",
        months: [
            "Tháng 1",
            "Tháng 2",
            "Tháng 3",
            "Tháng 4",
            "Tháng 5",
            "Tháng 6",
            "Tháng 7",
            "Tháng 8",
            "Tháng 9",
            "Tháng 10",
            "Tháng 11",
            "Tháng 12",
        ],
        months_with_day: Some([
            "tháng 1",
            "tháng 2",
            "tháng 3",
            "tháng 4",
            "tháng 5",
            "tháng 6",
            "tháng 7",
            "tháng 8",
            "tháng 9",
            "tháng 10",
            "tháng 11",
            "tháng 12",
        ]),
        months_short: ["Thg 1", "Thg 2", "Thg 3", "Thg 4", "Thg 5", "Thg 6", "Thg 7", "Thg 8", "Thg 9", "Thg 10", "Thg 11", "Thg 12"],
        days: ["Chủ Nhật", "Thứ Hai", "Thứ Ba", "Thứ Tư", "Thứ Năm", "Thứ Sáu", "Thứ Bảy"],
        days_short: ["CN", "T2", "T3", "T4", "T5", "T6", "T7"],
    },
    DateLanguage {
        code: "fi",
        name: "Suomi",
        months: [
            "tammikuu",
            "helmikuu",
            "maaliskuu",
            "huhtikuu",
            "toukokuu",
            "kesäkuu",
            "heinäkuu",
            "elokuu",
            "syyskuu",
            "lokakuu",
            "marraskuu",
            "joulukuu",
        ],
        // After a day number the month takes the partitive: `14. lokakuuta 2026`.
        months_with_day: Some([
            "tammikuuta",
            "helmikuuta",
            "maaliskuuta",
            "huhtikuuta",
            "toukokuuta",
            "kesäkuuta",
            "heinäkuuta",
            "elokuuta",
            "syyskuuta",
            "lokakuuta",
            "marraskuuta",
            "joulukuuta",
        ]),
        months_short: ["tammi", "helmi", "maalis", "huhti", "touko", "kesä", "heinä", "elo", "syys", "loka", "marras", "joulu"],
        days: ["sunnuntai", "maanantai", "tiistai", "keskiviikko", "torstai", "perjantai", "lauantai"],
        days_short: ["su", "ma", "ti", "ke", "to", "pe", "la"],
    },
    DateLanguage {
        code: "sq",
        name: "Shqip",
        months: ["janar", "shkurt", "mars", "prill", "maj", "qershor", "korrik", "gusht", "shtator", "tetor", "nëntor", "dhjetor"],
        months_with_day: None,
        months_short: ["jan", "shk", "mar", "pri", "maj", "qer", "korr", "gush", "sht", "tet", "nën", "dhj"],
        days: ["e diel", "e hënë", "e martë", "e mërkurë", "e enjte", "e premte", "e shtunë"],
        days_short: ["die", "hën", "mar", "mër", "enj", "pre", "sht"],
    },
    DateLanguage {
        code: "th",
        name: "ไทย",
        months: ["มกราคม", "กุมภาพันธ์", "มีนาคม", "เมษายน", "พฤษภาคม", "มิถุนายน", "กรกฎาคม", "สิงหาคม", "กันยายน", "ตุลาคม", "พฤศจิกายน", "ธันวาคม"],
        months_with_day: None,
        months_short: ["ม.ค.", "ก.พ.", "มี.ค.", "เม.ย.", "พ.ค.", "มิ.ย.", "ก.ค.", "ส.ค.", "ก.ย.", "ต.ค.", "พ.ย.", "ธ.ค."],
        days: ["วันอาทิตย์", "วันจันทร์", "วันอังคาร", "วันพุธ", "วันพฤหัสบดี", "วันศุกร์", "วันเสาร์"],
        days_short: ["อา.", "จ.", "อ.", "พ.", "พฤ.", "ศ.", "ส."],
    },
    DateLanguage {
        code: "ca",
        name: "Català",
        months: ["gener", "febrer", "març", "abril", "maig", "juny", "juliol", "agost", "setembre", "octubre", "novembre", "desembre"],
        // After a day the month carries its own preposition, elided before a vowel: `7 d'abril`.
        months_with_day: Some([
            "de gener",
            "de febrer",
            "de març",
            "d'abril",
            "de maig",
            "de juny",
            "de juliol",
            "d'agost",
            "de setembre",
            "d'octubre",
            "de novembre",
            "de desembre",
        ]),
        months_short: ["gen.", "febr.", "març", "abr.", "maig", "juny", "jul.", "ag.", "set.", "oct.", "nov.", "des."],
        days: ["diumenge", "dilluns", "dimarts", "dimecres", "dijous", "divendres", "dissabte"],
        days_short: ["dg.", "dl.", "dt.", "dx.", "dj.", "dv.", "ds."],
    },
    // Romanian: month and weekday names are lowercase, also standing alone (`noiembrie 2023`).
    DateLanguage {
        code: "ro",
        name: "Română",
        months: [
            "ianuarie",
            "februarie",
            "martie",
            "aprilie",
            "mai",
            "iunie",
            "iulie",
            "august",
            "septembrie",
            "octombrie",
            "noiembrie",
            "decembrie",
        ],
        months_with_day: None,
        months_short: ["ian.", "feb.", "mar.", "apr.", "mai", "iun.", "iul.", "aug.", "sept.", "oct.", "nov.", "dec."],
        days: ["duminică", "luni", "marți", "miercuri", "joi", "vineri", "sâmbătă"],
        days_short: ["dum.", "lun.", "mar.", "mie.", "joi", "vin.", "sâm."],
    },
    DateLanguage {
        code: "lo",
        name: "ພາສາລາວ",
        months: ["ມັງກອນ", "ກຸມພາ", "ມີນາ", "ເມສາ", "ພຶດສະພາ", "ມິຖຸນາ", "ກໍລະກົດ", "ສິງຫາ", "ກັນຍາ", "ຕຸລາ", "ພະຈິກ", "ທັນວາ"],
        months_with_day: None,
        months_short: ["ມ.ກ.", "ກ.ພ.", "ມ.ນ.", "ມ.ສ.", "ພ.ພ.", "ມິ.ຖ.", "ກ.ລ.", "ສ.ຫ.", "ກ.ຍ.", "ຕ.ລ.", "ພ.ຈ.", "ທ.ວ."],
        days: ["ວັນອາທິດ", "ວັນຈັນ", "ວັນອັງຄານ", "ວັນພຸດ", "ວັນພະຫັດ", "ວັນສຸກ", "ວັນເສົາ"],
        days_short: ["ອາທິດ", "ຈັນ", "ອັງຄານ", "ພຸດ", "ພະຫັດ", "ສຸກ", "ເສົາ"],
    },
    // Hindi (Devanagari). Month and weekday names are the same with and after a day number.
    DateLanguage {
        code: "hi",
        name: "हिन्दी",
        months: ["जनवरी", "फ़रवरी", "मार्च", "अप्रैल", "मई", "जून", "जुलाई", "अगस्त", "सितंबर", "अक्तूबर", "नवंबर", "दिसंबर"],
        months_with_day: None,
        months_short: ["जन॰", "फ़र॰", "मार्च", "अप्रैल", "मई", "जून", "जुला॰", "अग॰", "सित॰", "अक्तू॰", "नव॰", "दिस॰"],
        days: ["रविवार", "सोमवार", "मंगलवार", "बुधवार", "गुरुवार", "शुक्रवार", "शनिवार"],
        days_short: ["रवि", "सोम", "मंगल", "बुध", "गुरु", "शुक्र", "शनि"],
    },
    // Korean: months are numbered (`3월`) and weekdays end in 요일; no names change after a day.
    DateLanguage {
        code: "ko",
        name: "한국어",
        months: ["1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월"],
        months_with_day: None,
        months_short: ["1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월", "9월", "10월", "11월", "12월"],
        days: ["일요일", "월요일", "화요일", "수요일", "목요일", "금요일", "토요일"],
        days_short: ["일", "월", "화", "수", "목", "금", "토"],
    },
    DateLanguage {
        code: "nb",
        name: "Norsk bokmål",
        months: ["januar", "februar", "mars", "april", "mai", "juni", "juli", "august", "september", "oktober", "november", "desember"],
        months_with_day: None,
        months_short: ["jan.", "feb.", "mar.", "apr.", "mai", "jun.", "jul.", "aug.", "sep.", "okt.", "nov.", "des."],
        days: ["søndag", "mandag", "tirsdag", "onsdag", "torsdag", "fredag", "lørdag"],
        days_short: ["søn.", "man.", "tir.", "ons.", "tor.", "fre.", "lør."],
    },
];

/// The date language for `code` (any case), or `None` when there's none.
pub fn date_language(code: &str) -> Option<&'static DateLanguage> {
    DATE_LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code.trim()))
}

/// The characters of `text` that Fill & Sign can't write into a PDF yet, each once. Its text is
/// drawn in Helvetica with WinAnsi encoding (Western European letters), so anything else (`ř`,
/// Japanese, Chinese) would become `?` in the file.
pub fn unwritable(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let mut buf = [0u8; 4];
        if c != '?' && pdfcraft_fonts::win_ansi(c.encode_utf8(&mut buf)) == b"?" && !out.contains(c) {
            out.push(c);
        }
    }
    out
}

/// `fmt` trimmed, when it's a usable date pattern: not too long, showing at least a year,
/// month or day, and no time letters (`H h M s t`) unless escaped with `\`.
pub fn check_date_format(fmt: &str) -> Result<&str, String> {
    let fmt = fmt.trim();
    if fmt.is_empty() {
        return Err("the date format is empty".into());
    }
    if fmt.chars().count() > MAX_DATE_FORMAT_CHARS {
        return Err(format!("the date format is longer than {MAX_DATE_FORMAT_CHARS} characters"));
    }
    let letters: String = unescaped(fmt).filter_map(|(c, escaped)| (!escaped).then_some(c)).collect();
    if let Some(c) = letters.chars().find(|c| matches!(c, 'H' | 'h' | 'M' | 's' | 't')) {
        return Err(format!("`{c}` is a time letter (H h M s t); put \\ before it to show it as is"));
    }
    if !letters.chars().any(|c| matches!(c, 'y' | 'm' | 'd')) {
        return Err("the date format shows no year (y), month (m) or day (d)".into());
    }
    // Only whole codes: `yyy` or `mmmmm` would silently show something else.
    for (c, n, _) in runs(fmt).into_iter().filter(|r| !r.2) {
        let fits = match c {
            'y' => n == 2 || n == 4,
            'm' | 'd' => (1..=4).contains(&n),
            _ => true,
        };
        if !fits {
            let code: String = std::iter::repeat_n(c, n).collect();
            let use_ = match c {
                'y' => "yy or yyyy",
                'm' => "m, mm, mmm or mmmm",
                _ => "d, dd, ddd or dddd",
            };
            return Err(format!("`{code}` is not a date code; use {use_}"));
        }
    }
    Ok(fmt)
}

/// `fmt` as runs of the same character with whether it's escaped: `dd.\mm` → (`d`, 2, no),
/// (`.`, 1, no), (`m`, 1, yes), (`m`, 1, no). Escaped characters are never grouped.
fn runs(fmt: &str) -> Vec<(char, usize, bool)> {
    let mut runs: Vec<(char, usize, bool)> = Vec::new();
    for (c, escaped) in unescaped(fmt) {
        match runs.last_mut() {
            Some((last, n, false)) if !escaped && *last == c => *n += 1,
            _ => runs.push((c, 1, escaped)),
        }
    }
    runs
}

/// The day `(year, month, day)` written in `fmt` with `lang`'s names. Anything but the date
/// letters is copied as is; a month or day out of range shows as its number.
pub fn format_day(fmt: &str, (y, m, d): (i64, u32, u32), lang: &DateLanguage) -> String {
    let runs = runs(fmt);
    let with_day = runs.iter().any(|&(c, n, escaped)| !escaped && c == 'd' && n <= 2);
    let month = (m as usize).checked_sub(1);
    let weekday = weekday(y, m, d);
    let mut out = String::new();
    for (c, n, escaped) in runs {
        match (c, n) {
            _ if escaped => out.push(c),
            ('y', 4..) => out.push_str(&format!("{:04}", y.clamp(0, 9999))),
            ('y', _) => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            ('m', 3..) => {
                let names = if n >= 4 { lang.months_with_day.filter(|_| with_day).unwrap_or(lang.months) } else { lang.months_short };
                match month.and_then(|i| names.get(i)) {
                    Some(name) => out.push_str(name),
                    None => out.push_str(&m.to_string()),
                }
            }
            ('m', 2) => out.push_str(&format!("{m:02}")),
            ('m', _) => out.push_str(&m.to_string()),
            ('d', 3..) => {
                let names = if n >= 4 { lang.days } else { lang.days_short };
                match weekday.and_then(|w| names.get(w)) {
                    Some(name) => out.push_str(name),
                    None => out.push_str(&d.to_string()),
                }
            }
            ('d', 2) => out.push_str(&format!("{d:02}")),
            ('d', _) => out.push_str(&d.to_string()),
            _ => out.extend(std::iter::repeat_n(c, n)),
        }
    }
    out
}

/// The weekday (0 = Sunday) of a Gregorian date in years 1–9999, or `None` for an invalid one.
fn weekday(y: i64, m: u32, d: u32) -> Option<usize> {
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    if !(1..=9999).contains(&y) || !(1..=31).contains(&d) {
        return None;
    }
    let t = *T.get((m as usize).checked_sub(1)?)?;
    let y = y - i64::from(m < 3);
    Some((y + y / 4 - y / 100 + y / 400 + t + i64::from(d)).rem_euclid(7) as usize)
}

/// The characters of `fmt`, each with whether a `\` before it makes it literal.
fn unescaped(fmt: &str) -> impl Iterator<Item = (char, bool)> + '_ {
    let mut chars = fmt.chars();
    std::iter::from_fn(move || match chars.next()? {
        '\\' => Some(chars.next().map_or(('\\', true), |c| (c, true))),
        c => Some((c, false)),
    })
}

impl Session {
    /// Preferences ▸ Date format: the pattern Fill & Sign dates use. Rejects unusable patterns
    /// (see [`check_date_format`]) and keeps the previous one.
    pub fn set_date_format(&mut self, fmt: &str) -> Result<(), String> {
        let fmt = check_date_format(fmt)?;
        self.date_format = (fmt != DEFAULT_DATE_FORMAT).then(|| fmt.to_string());
        Ok(())
    }

    pub fn date_format(&self) -> &str {
        self.date_format.as_deref().unwrap_or(DEFAULT_DATE_FORMAT)
    }

    /// Preferences ▸ Date format ▸ Language: the language of month and weekday names, or
    /// `None` to follow the interface language. Rejects codes not in [`DATE_LANGUAGES`].
    pub fn set_date_language(&mut self, code: Option<&str>) -> Result<(), String> {
        self.date_language = match code {
            None => None,
            Some(c) => Some(date_language(c).map(|l| l.code.to_string()).ok_or_else(|| {
                let codes: Vec<&str> = DATE_LANGUAGES.iter().map(|l| l.code).collect();
                format!("unknown date language {c:?} (use one of {})", codes.join(", "))
            })?),
        };
        Ok(())
    }

    pub fn date_language(&self) -> Option<&str> {
        self.date_language.as_deref()
    }

    /// Today's local date in `fmt` (or the session's date format) with the names of `lang`
    /// (or the session's date language; English when neither is set or known).
    pub fn today_text(&self, fmt: Option<&str>, lang: Option<&str>) -> Result<String, String> {
        let fmt = match fmt {
            Some(f) => check_date_format(f)?,
            None => self.date_format(),
        };
        let lang = lang.or(self.date_language()).and_then(date_language).unwrap_or(&DATE_LANGUAGES[0]);
        Ok(format_day(fmt, self.today(), lang))
    }

    /// [`Session::today_text`] for writing into a PDF: refused when the date has characters
    /// Fill & Sign can't write yet (see [`unwritable`]), rather than saving them as `?`.
    pub fn today_text_for_pdf(&self, fmt: Option<&str>, lang: Option<&str>) -> Result<String, String> {
        let text = self.today_text(fmt, lang)?;
        match unwritable(&text) {
            bad if bad.is_empty() => Ok(text),
            bad => Err(format!(
                "\"{text}\" can't be written into the PDF yet: Fill & Sign text is Western European only ({bad}); pick a numeric format such as dd.mm.yyyy"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: (i64, u32, u32) = (2026, 3, 7);
    const EN: &DateLanguage = &DATE_LANGUAGES[0];

    fn in_lang(fmt: &str, code: &str) -> String {
        format_day(fmt, DAY, date_language(code).unwrap())
    }

    #[test]
    fn presets_write_the_day_in_their_own_order() {
        let shown: Vec<String> = DATE_FORMATS.iter().map(|f| format_day(f, DAY, EN)).collect();
        assert_eq!(
            shown,
            [
                "3/7/2026",
                "03/07/2026",
                "7/3/2026",
                "07/03/2026",
                "07.03.2026",
                "2026-03-07",
                "2026.03.07.",
                "2026/03/07",
                "2026/3/7",
                "7 Mar 2026",
                "7 March 2026",
                "Mar 7, 2026",
                "March 7, 2026",
                "Saturday, March 7, 2026",
            ]
        );
        assert!(DATE_FORMATS.iter().all(|f| check_date_format(f).is_ok()));
        assert_eq!(DATE_FORMATS[0], DEFAULT_DATE_FORMAT);
    }

    #[test]
    fn escaped_letters_are_shown_as_is() {
        assert_eq!(format_day("d \\de mmmm \\de yyyy", DAY, EN), "7 de March de 2026");
        assert_eq!(format_day("yyyy\\", DAY, EN), "2026\\");
        assert_eq!(check_date_format("d \\de mmmm"), Ok("d \\de mmmm"));
    }

    #[test]
    fn unusable_patterns_are_rejected() {
        assert!(check_date_format("   ").is_err());
        assert!(check_date_format("Date: dd/mm").unwrap_err().contains("`t`"));
        assert!(check_date_format("mm/dd HH:MM").is_err());
        assert!(check_date_format("Year").is_err(), "no date letters");
        assert!(check_date_format(&"d".repeat(MAX_DATE_FORMAT_CHARS + 1)).is_err());
        assert_eq!(check_date_format("  yyyy-mm-dd "), Ok("yyyy-mm-dd"));
    }

    #[test]
    fn only_whole_date_codes_are_accepted() {
        for (fmt, code) in [("yyyyyyyy", "`yyyyyyyy`"), ("yyy", "`yyy`"), ("d/m/y", "`y`"), ("mmmmm d", "`mmmmm`"), ("ddddd", "`ddddd`")] {
            let e = check_date_format(fmt).unwrap_err();
            assert!(e.contains(code) && e.contains("is not a date code"), "{fmt}: {e}");
        }
        for fmt in ["yy", "yyyy", "m", "mm", "mmm", "mmmm", "d", "dd", "ddd", "dddd", r"\y\y\y yyyy"] {
            assert!(check_date_format(fmt).is_ok(), "{fmt}");
        }
    }

    #[test]
    fn hostile_days_and_patterns_do_not_panic() {
        for fmt in ["mmmm", "mmm", "dddd", "ddd", "\\", "yyyyyyyyy", "é d ü", "d. mmmm"] {
            for day in [(i64::MAX, u32::MAX, u32::MAX), (i64::MIN, 0, 0), (2026, 13, 40), (0, 2, 29)] {
                for lang in &DATE_LANGUAGES {
                    let _ = format_day(fmt, day, lang);
                }
            }
        }
        assert_eq!(format_day("mmmm dddd", (2026, 13, 40), EN), "13 40", "out of range shows numbers");
    }

    #[test]
    fn the_session_stamps_today_in_its_format() {
        // 2023-11-14 (UTC) on the injected clock.
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        assert_eq!(s.date_format(), DEFAULT_DATE_FORMAT);
        assert_eq!(s.today_text(None, None).as_deref(), Ok("11/14/2023"));
        s.set_date_format("dd.mm.yyyy").unwrap();
        assert_eq!(s.today_text(None, None).as_deref(), Ok("14.11.2023"));
        assert_eq!(s.today_text(Some("yyyy-mm-dd"), None).as_deref(), Ok("2023-11-14"));
        assert!(s.today_text(Some("HH"), None).is_err());
        assert!(s.set_date_format("nothing").is_err());
        assert_eq!(s.date_format(), "dd.mm.yyyy", "a rejected pattern keeps the previous one");
    }

    /// 21:30 on 14 Oct 2026 in São Paulo (UTC-3) is 00:30Z on the 15th. The date Fill & Sign
    /// stamps is the wall-clock day (14th); the time the file records (`/M`, `/ModDate`) is UTC.
    /// The test shortcut (`with_clock`) takes the injected time as local and says the 15th, which
    /// is why it cannot serve a host that knows both the UTC time and the zone.
    #[test]
    fn a_wall_clock_gives_the_local_day_and_keeps_utc_for_the_file() {
        let sao_paulo = Session::new().with_wall_clock(|| 1_792_024_200, || -3 * 3600);
        assert_eq!(sao_paulo.today(), (2026, 10, 14));
        assert_eq!(sao_paulo.today_text(None, None).as_deref(), Ok("10/14/2026"));
        assert_eq!(sao_paulo.signing_date(), "D:20261014213000-03'00'");
        assert_eq!(sao_paulo.stamp_by_line("Ada"), "By Ada at 9:30 pm, Oct 14, 2026");
        assert_eq!(sao_paulo.now_secs(), 1_792_024_200, "the file's instant stays in UTC");

        let utc = Session::new().with_wall_clock(|| 1_792_024_200, || 0);
        assert_eq!(utc.today(), (2026, 10, 15));

        let clock_only = Session::new().with_clock(|| 1_792_024_200);
        assert_eq!(clock_only.today(), (2026, 10, 15), "on its own, an injected clock counts as local time");
    }

    #[test]
    fn names_follow_the_date_language() {
        assert_eq!(in_lang("dddd, d \\de mmmm \\de yyyy", "es"), "sábado, 7 de marzo de 2026");
        assert_eq!(in_lang("ddd d mmm", "pt-br"), "sáb 7 mar");
        assert_eq!(in_lang("dddd d mmmm yyyy", "fr"), "samedi 7 mars 2026");
        assert_eq!(in_lang("dddd d mmmm yyyy", "it"), "sabato 7 marzo 2026");
        assert_eq!(in_lang("ddd d mmm", "it"), "sab 7 mar");
        assert_eq!(in_lang("dddd, d mmmm", "te"), "శనివారం, 7 మార్చి");
        assert_eq!(in_lang("dddd, d mmmm", "ar"), "السبت, 7 مارس");
        assert_eq!(in_lang("yyyy\\年mmmmd\\日 dddd", "ja"), "2026年3月7日 土曜日");
        assert_eq!(in_lang("yyyy\\年mmmmd\\日 dddd", "zh-hans"), "2026年三月7日 星期六");
        assert_eq!(in_lang("ddd", "zh-hant"), "週六");
        assert_eq!(in_lang("dddd, d mmmm yyyy", "sq"), "e shtunë, 7 mars 2026");
        assert_eq!(in_lang("ddd d mmm", "sq"), "sht 7 mar");
        assert_eq!(in_lang("dddd d. mmmm yyyy", "nb"), "lørdag 7. mars 2026");
        assert_eq!(in_lang("ddd d. mmm", "nb"), "lør. 7. mar.");
        assert_eq!(date_language("PT-BR").map(|l| l.code), Some("pt-br"));
        assert!(date_language("xx").is_none());
        for l in &DATE_LANGUAGES {
            assert!(l.months.iter().chain(&l.months_short).chain(&l.days).chain(&l.days_short).all(|n| !n.is_empty()), "{}", l.code);
        }
    }

    #[test]
    fn czech_and_russian_months_change_after_a_day() {
        assert_eq!(in_lang("d. mmmm yyyy", "cs"), "7. března 2026");
        assert_eq!(in_lang("mmmm yyyy", "cs"), "březen 2026");
        assert_eq!(in_lang("d mmmm yyyy", "ru"), "7 марта 2026");
        assert_eq!(in_lang("mmmm yyyy", "ru"), "март 2026");
        // A weekday (dddd) is not a day number.
        assert_eq!(in_lang("dddd, mmmm", "cs"), "sobota, březen");
    }

    /// Indonesian month and weekday names are the same with and after a day number. (Vietnamese
    /// has its own test upstream: its numeric month forms are writable, weekday names are not.)
    #[test]
    fn indonesian_month_and_weekday_names() {
        assert_eq!(in_lang("d mmmm yyyy", "id"), "7 Maret 2026");
        assert_eq!(in_lang("mmmm yyyy", "id"), "Maret 2026");
        assert_eq!(in_lang("dddd, d mmmm yyyy", "id"), "Sabtu, 7 Maret 2026");
        // Indonesian month names are Latin, so they reach a PDF.
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        s.set_date_language(Some("id")).unwrap();
        assert_eq!(s.today_text_for_pdf(Some("d mmmm yyyy"), None).as_deref(), Ok("14 November 2023"));
    }

    /// Hindi and Korean names are the same with and after a day number. Neither is Western
    /// European, so the date-text path refuses them with what to use instead.
    #[test]
    fn hindi_and_korean_month_and_weekday_names() {
        assert_eq!(in_lang("d mmmm yyyy", "hi"), "7 मार्च 2026");
        assert_eq!(in_lang("dddd, mmmm", "hi"), "शनिवार, मार्च");
        assert_eq!(in_lang("ddd, d mmm", "hi"), "शनि, 7 मार्च");
        assert_eq!(in_lang("yyyy\\년 mmmm d\\일 dddd", "ko"), "2026년 3월 7일 토요일");
        assert_eq!(in_lang("ddd", "ko"), "토");
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        for code in ["hi", "ko"] {
            s.set_date_language(Some(code)).unwrap();
            assert!(s.today_text_for_pdf(Some("d mmmm yyyy"), None).unwrap_err().contains("dd.mm.yyyy"), "{code}");
        }
    }

    #[test]
    fn catalan_months_elide_the_preposition_after_a_day() {
        assert_eq!(in_lang("dddd d mmmm \\de yyyy", "ca"), "dissabte 7 de març de 2026");
        assert_eq!(in_lang("mmmm yyyy", "ca"), "març 2026");
        let ca = date_language("ca").unwrap();
        assert_eq!(format_day("d mmmm \\de yyyy", (2026, 4, 23), ca), "23 d'abril de 2026");
        assert_eq!(format_day("d mmmm", (2026, 8, 1), ca), "1 d'agost");
        assert_eq!(format_day("d mmmm", (2026, 10, 8), ca), "8 d'octubre");
        assert_eq!(format_day("mmmm", (2026, 10, 8), ca), "octubre", "no preposition without a day");
    }

    #[test]
    fn the_session_writes_names_in_its_date_language() {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        s.set_date_format("dddd d mmmm").unwrap();
        assert_eq!(s.today_text(None, None).as_deref(), Ok("Tuesday 14 November"), "English by default");
        assert_eq!(s.today_text(None, Some("es")).as_deref(), Ok("martes 14 noviembre"), "a caller's language");
        s.set_date_language(Some("PT-BR")).unwrap();
        assert_eq!(s.date_language(), Some("pt-br"));
        assert_eq!(s.today_text(None, None).as_deref(), Ok("terça-feira 14 novembro"));
        assert_eq!(s.today_text(Some("d mmmm"), Some("uk")).as_deref(), Ok("14 листопада"), "Ukrainian months after a day");
        assert_eq!(s.today_text(Some("dddd d mmmm"), Some("de")).as_deref(), Ok("Dienstag 14 November"));
        assert!(s.set_date_language(Some("xx")).unwrap_err().contains("zh-hant"), "only the interface languages");
        assert_eq!(s.date_language(), Some("pt-br"), "an unknown language keeps the previous one");
        s.set_date_language(Some("LT")).unwrap();
        assert_eq!(s.date_language(), Some("lt"));
        assert_eq!(s.today_text(Some("dddd d mmmm"), None).as_deref(), Ok("antradienis 14 lapkričio"));
        s.set_date_language(Some("IT")).unwrap();
        assert_eq!(s.date_language(), Some("it"));
        assert_eq!(s.today_text_for_pdf(None, None).as_deref(), Ok("martedì 14 novembre"), "Italian names and accents can be written into a PDF");
        s.set_date_language(None).unwrap();
        assert_eq!(s.date_language(), None);
    }

    #[test]
    fn romanian_month_and_weekday_names() {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        s.set_date_language(Some("ro")).unwrap();
        assert_eq!(s.date_language(), Some("ro"));
        s.set_date_format("d mmmm yyyy").unwrap();
        assert_eq!(s.today_text(None, None).as_deref(), Ok("14 noiembrie 2023"));
        s.set_date_format("mmmm yyyy").unwrap();
        assert_eq!(s.today_text(None, None).as_deref(), Ok("noiembrie 2023"));
        s.set_date_format("ddd, d mmm").unwrap();
        assert_eq!(s.today_text(None, None).as_deref(), Ok("mar., 14 nov."));
        // `14 noiembrie 2023` is WinAnsi; the weekday `marți` (ț, comma below) is not, so Fill & Sign refuses that format.
        assert_eq!(s.today_text_for_pdf(Some("d mmmm yyyy"), Some("ro")).as_deref(), Ok("14 noiembrie 2023"));
        let e = s.today_text_for_pdf(Some("dddd d mmmm yyyy"), Some("ro")).unwrap_err();
        assert!(e.contains("can't be written into the PDF yet"), "{e}");
    }

    #[test]
    fn vietnamese_month_and_weekday_names() {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        s.set_date_language(Some("vi")).unwrap();
        assert_eq!(s.date_language(), Some("vi"));
        s.set_date_format("d mmmm yyyy").unwrap();
        assert_eq!(s.today_text(None, None).as_deref(), Ok("14 tháng 11 2023"));
        s.set_date_format("mmmm yyyy").unwrap();
        assert_eq!(s.today_text(None, None).as_deref(), Ok("Tháng 11 2023"));
        s.set_date_format("ddd, d mmm").unwrap();
        assert_eq!(s.today_text(None, None).as_deref(), Ok("T3, 14 Thg 11"));
        // Encoding follows the spelled text: `14 tháng 11 2023` is WinAnsi-representable;
        // a weekday name such as `Thứ Ba` is not, so Fill & Sign refuses that format.
        assert_eq!(s.today_text_for_pdf(Some("d mmmm yyyy"), Some("vi")).as_deref(), Ok("14 tháng 11 2023"));
        let e = s.today_text_for_pdf(Some("dddd d mmmm yyyy"), Some("vi")).unwrap_err();
        assert!(e.contains("can't be written into the PDF yet") && e.contains("dd.mm.yyyy"), "{e}");
        assert!(s.today_text_for_pdf(Some("dd.mm.yyyy"), Some("vi")).is_ok());
    }

    #[test]
    fn dates_the_pdf_cannot_hold_are_refused_not_damaged() {
        assert_eq!(unwritable("8. října 2026"), "ř");
        assert_eq!(unwritable("2026年10月8日"), "年月日");
        assert_eq!(unwritable("sábado, 7 de março – 2026?"), "", "Western European, dashes and ? are fine");
        assert_eq!(unwritable("lørdag 7. mars, søn. 8. mai (æøå ÆØÅ)"), "", "Norwegian letters are Western European");
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        s.set_date_format("d. mmmm yyyy").unwrap();
        assert_eq!(s.today_text_for_pdf(None, Some("es")).as_deref(), Ok("14. noviembre 2023"));
        assert_eq!(s.today_text_for_pdf(None, Some("cs")).as_deref(), Ok("14. listopadu 2023"), "no letter outside WinAnsi");
        let e = s.today_text_for_pdf(Some("dddd"), Some("ja")).unwrap_err();
        assert!(e.contains("火曜日") && e.contains("can't be written into the PDF yet"), "{e}");
        assert!(s.today_text_for_pdf(Some("dd.mm.yyyy"), Some("ja")).is_ok(), "numbers are fine in any language");
        assert!(s.today_text_for_pdf(Some("yyyy\\年mm\\月"), Some("ja")).unwrap_err().contains("年月"));
    }
}
