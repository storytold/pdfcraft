//! Acrobat form formatting, validation and calculation without a JavaScript engine.
//!
//! Real forms carry these as JavaScript calls to the AF functions that Acrobat's JavaScript API
//! reference documents (`AFNumber_Format`, `AFPercent_Format`, `AFDate_FormatEx`,
//! `AFTime_Format`, `AFSpecial_Format`, `AFSpecial_KeystrokeEx`, `AFRange_Validate`,
//! `AFSimple_Calculate`, and simplified field notation). Acrobat itself writes exactly these
//! calls from the Format, Validate and Calculate tabs. This module recognises them, runs them
//! natively (format for display, keystroke check on commit, validation, calculation), and
//! writes them back when a form is authored. Other scripts are kept untouched and reported as
//! unsupported; they need the JavaScript engine (M6.4).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::ops::Range;

mod calc;
mod format;
mod script;

#[cfg(test)]
use calc::{Notation, eval_notation, notation_name, notation_tokens, unique};
pub use calc::{calculate, calculation_reads, notation_js, reads_built_names, rename_in_calculation};
pub use format::{DATE_PRESETS, DateTime, TIME_PRESETS, format_date, format_value, js_number, keystroke, make_number, parse_date, validate};
use script::{Arg, call};
pub use script::{NOTATION_NOT_STORABLE, calculate_js, format_js, notation_storable, parse_calculate, parse_format, parse_validate, validate_js};

/// The Format tab.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Format {
    #[default]
    None,
    /// `AFNumber_Format(dec, sepStyle, negStyle, currStyle, strCurrency, bCurrencyPrepend)`.
    Number { decimals: u8, sep: u8, neg: u8, currency: String, prepend: bool },
    /// `AFPercent_Format(dec, sepStyle)`.
    Percent { decimals: u8, sep: u8 },
    /// `AFDate_FormatEx(cFormat)`.
    Date(String),
    /// `AFTime_FormatEx(cFormat)`.
    Time(String),
    /// `AFSpecial_Format(n)`: 0 zip, 1 zip+4, 2 phone, 3 SSN.
    Special(u8),
    /// `AFSpecial_KeystrokeEx(cMask)`: 9 digit, A letter, O letter or digit, X any.
    Mask(String),
}

/// The Validate tab.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Validate {
    #[default]
    None,
    /// `AFRange_Validate(bGreater, nGreater, bLess, nLess)`.
    Range { min: Option<f64>, max: Option<f64> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcOp {
    Sum,
    Product,
    Average,
    Minimum,
    Maximum,
}

impl CalcOp {
    pub fn code(self) -> &'static str {
        match self {
            CalcOp::Sum => "SUM",
            CalcOp::Product => "PRD",
            CalcOp::Average => "AVG",
            CalcOp::Minimum => "MIN",
            CalcOp::Maximum => "MAX",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CalcOp::Sum => "sum (+)",
            CalcOp::Product => "product (x)",
            CalcOp::Average => "average",
            CalcOp::Minimum => "minimum",
            CalcOp::Maximum => "maximum",
        }
    }

    pub const ALL: [CalcOp; 5] = [CalcOp::Sum, CalcOp::Product, CalcOp::Average, CalcOp::Minimum, CalcOp::Maximum];
}

/// The Calculate tab.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Calculate {
    #[default]
    None,
    /// `AFSimple_Calculate("SUM", new Array("a", "b"))`.
    Simple { op: CalcOp, fields: Vec<String> },
    /// Simplified field notation, e.g. `Price * Quantity`.
    Notation(String),
}

/// A field's recognised scripts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Actions {
    pub format: Format,
    pub validate: Validate,
    pub calculate: Calculate,
    /// Events whose script isn't one of the AF calls (they need the JavaScript engine).
    pub unsupported: Vec<&'static str>,
    /// Those scripts' JavaScript, run through [`crate::Scripts`].
    pub scripts: Scripts,
}

/// Field scripts that aren't AF calls, by event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scripts {
    pub format: Option<String>,
    pub keystroke: Option<String>,
    pub validate: Option<String>,
    pub calculate: Option<String>,
}

impl Actions {
    pub fn is_empty(&self) -> bool {
        self.format == Format::None && self.validate == Validate::None && self.calculate == Calculate::None
    }
}

#[cfg(test)]
mod tests;

// ── push buttons ────────────────────────────────────────────────────────────────────────────

/// What clicking a push button does (its mouse-up action), as far as PdfCraft can run it
/// without a JavaScript engine.
#[derive(Clone, Debug, PartialEq)]
pub enum ButtonAction {
    /// Reset fields: the listed ones, or all but them (`exclude`), or all when empty.
    Reset {
        fields: Vec<String>,
        exclude: bool,
    },
    /// A named action: Print, NextPage, PrevPage, FirstPage, LastPage, …
    Named(String),
    Uri(String),
    /// Go to a page (0-based) in this document.
    GoTo(usize),
    /// A hide action (§12.6.4.11): hide the listed fields (and their kids), or show them when
    /// `hide` is false.
    ShowHide {
        fields: Vec<String>,
        hide: bool,
    },
    /// A set-layer-visibility action (`SetOCGState`, §12.6.4.13): each change in order, naming
    /// the layer by its optional content group (object number, generation). With `preserve_rb`,
    /// a layer turned on turns off the other layers of its radio-button groups.
    SetLayers {
        changes: Vec<(LayerOp, (u32, u16))>,
        preserve_rb: bool,
    },
    /// `app.alert("…")`.
    Alert(String),
    /// Submit the form to a URL (not sent: PdfCraft never posts form data on its own).
    Submit(String),
    /// `event.target.buttonImportIcon()`: choose an image for the button (an image field).
    ImportIcon,
    /// A script PdfCraft can't run yet.
    Script(String),
}

/// What a set-layer-visibility action does to a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerOp {
    On,
    Off,
    Toggle,
}

/// Recognise the common one-line button scripts.
pub fn button_script(js: &str) -> ButtonAction {
    let t = js.trim();
    // Print takes an options object whose contents don't change what PdfCraft does.
    if t.starts_with("this.print(") || t.starts_with("print(") || t.contains(";this.print(") || t.contains("; this.print(") {
        return ButtonAction::Named("Print".into());
    }
    if let Some(args) = call(t, "this.resetForm").or_else(|| call(t, "resetForm")) {
        let fields = match args.first() {
            Some(Arg::List(l)) => l.iter().filter_map(Arg::string).collect(),
            Some(Arg::Str(s)) => vec![s.clone()],
            _ => Vec::new(),
        };
        return ButtonAction::Reset { fields, exclude: false };
    }
    if let Some(m) = call(t, "app.alert").and_then(|args| args.first().and_then(Arg::string)) {
        return ButtonAction::Alert(m);
    }
    if let Some(u) = call(t, "app.launchURL").and_then(|args| args.first().and_then(Arg::string)) {
        return ButtonAction::Uri(u);
    }
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.trim_end_matches(';') == "event.target.buttonImportIcon()" {
        return ButtonAction::ImportIcon;
    }
    match compact.trim_end_matches(';') {
        "this.pageNum++" | "pageNum++" => ButtonAction::Named("NextPage".into()),
        "this.pageNum--" | "pageNum--" => ButtonAction::Named("PrevPage".into()),
        "this.pageNum=0" => ButtonAction::Named("FirstPage".into()),
        _ => ButtonAction::Script(t.to_string()),
    }
}

#[cfg(test)]
mod button_tests {
    use super::*;

    #[test]
    fn common_button_scripts_are_recognised() {
        assert_eq!(button_script("this.print({bUI: true});"), ButtonAction::Named("Print".into()));
        assert_eq!(button_script("this.resetForm([\"a\", \"b\"]);"), ButtonAction::Reset { fields: vec!["a".into(), "b".into()], exclude: false });
        assert_eq!(button_script("this.resetForm();"), ButtonAction::Reset { fields: vec![], exclude: false });
        assert_eq!(button_script("app.alert('Thanks!');"), ButtonAction::Alert("Thanks!".into()));
        assert_eq!(button_script("app.launchURL(\"https://example.org\", true);"), ButtonAction::Uri("https://example.org".into()));
        assert_eq!(button_script("this.pageNum++;"), ButtonAction::Named("NextPage".into()));
        assert_eq!(button_script("event.target.buttonImportIcon();"), ButtonAction::ImportIcon);
        assert!(matches!(button_script("var x = 1; doStuff(x);"), ButtonAction::Script(_)));
    }
}

#[cfg(test)]
mod never_crash_tests {
    use super::*;

    /// A field value or typed text with an absurd year used to overflow the weekday arithmetic.
    #[test]
    fn huge_years_format_without_panicking() {
        let f = Format::Date("dddd, mmmm d, yyyy".into());
        for v in ["1/1/2147483647", "1/1/4294967295", "12/31/3000000000"] {
            let _ = format_value(&f, v);
            let _ = keystroke(&f, "date", v);
        }
    }

    /// Deeply nested arrays in a field's format script used to recurse until the stack overflowed.
    #[test]
    fn deeply_nested_script_arguments_are_rejected() {
        let n = 200_000;
        let js = format!("AFNumber_Format({}0{});", "[".repeat(n), "]".repeat(n));
        assert_eq!(parse_format(&js), None);
        let js = format!("AFNumber_Format({}0{});", "new Array(".repeat(n), ")".repeat(n));
        assert_eq!(parse_format(&js), None);
    }

    /// Deeply nested simplified field notation (from a field's calculate script) likewise.
    #[test]
    fn deeply_nested_notation_is_rejected() {
        let n = 200_000;
        let deep = format!("{}1{}", "(".repeat(n), ")".repeat(n));
        assert_eq!(eval_notation(&deep, &|_| 0.0), None);
        let negated = format!("{}1", "-".repeat(n));
        assert_eq!(eval_notation(&negated, &|_| 0.0), None);
        assert_eq!(eval_notation("((1 + 2)) * -(3)", &|_| 0.0), Some(-9.0));
    }

    /// `DateTime` is public: an out-of-range month or day must not index past the name tables.
    #[test]
    fn out_of_range_dates_format_without_panicking() {
        for m in [0, 13, u32::MAX] {
            let dt = DateTime { y: 2024, m, d: 1, hh: 0, mm: 0, ss: 0 };
            let _ = format_date(dt, "dddd mmmm mmm d yyyy");
        }
        let dt = DateTime { y: i32::MAX, m: 1, d: u32::MAX, hh: 0, mm: 0, ss: 0 };
        let _ = format_date(dt, "dddd ddd");
    }
}

#[cfg(test)]
mod pm_marker_tests {
    use super::*;

    /// A month name ending in "p" ("Sep") is not a PM marker; only a "p" attached to a time is.
    #[test]
    fn month_name_ending_in_p_is_not_pm() {
        let dt = parse_date("2026 05 Sep", "yyyy dd mmm HH:MM").expect("valid date");
        assert_eq!((dt.m, dt.d, dt.hh, dt.mm), (9, 5, 0, 0));
        let f = Format::Date("yyyy dd mmm HH:MM".into());
        assert_eq!(format_value(&f, "2026 05 Sep"), "2026 05 Sep 00:00");
        assert!(keystroke(&f, "date", "2026 05 Sep").is_ok());
    }

    #[test]
    fn bare_p_after_a_time_is_still_pm() {
        assert_eq!(parse_date("05/09/2026 5:30p", "mm/dd/yyyy h:MM").map(|d| d.hh), Some(17));
        assert_eq!(parse_date("05/09/2026 5:30 p", "mm/dd/yyyy h:MM").map(|d| d.hh), Some(17));
        assert_eq!(parse_date("05/09/2026 5:30 pm", "mm/dd/yyyy h:MM").map(|d| d.hh), Some(17));
    }
}

#[cfg(test)]
mod undelimited_tests {
    use super::*;

    fn ymd(dt: Option<DateTime>) -> Option<(i32, u32, u32)> {
        dt.map(|d| (d.y, d.m, d.d))
    }

    /// Fields written side by side in the format ("yyyymmdd") take their fixed widths from one
    /// run of typed digits. Every such date used to be refused, because the run was read as a
    /// single number.
    #[test]
    fn digits_typed_without_separators_fill_adjacent_fields() {
        assert_eq!(ymd(parse_date("20261018", "yyyymmdd")), Some((2026, 10, 18)));
        assert_eq!(ymd(parse_date("261018", "yymmdd")), Some((2026, 10, 18)));
        assert_eq!(ymd(parse_date("10182026", "mmddyyyy")), Some((2026, 10, 18)));
        assert_eq!(ymd(parse_date("18102026", "ddmmyyyy")), Some((2026, 10, 18)));
        let f = Format::Date("yyyymmdd".into());
        assert_eq!(keystroke(&f, "Date", "20261018"), Ok("20261018".into()));
        assert_eq!(format_value(&f, "20261018"), "20261018");
    }

    #[test]
    fn separators_are_still_accepted_in_an_undelimited_format() {
        assert_eq!(ymd(parse_date("2026-10-18", "yyyymmdd")), Some((2026, 10, 18)));
        assert_eq!(ymd(parse_date("2026 1018", "yyyymmdd")), Some((2026, 10, 18)));
    }

    /// Separators mean one number per field, as before: a four-digit year typed with them stays
    /// whole in a two-digit-year format. Only a run covering several fields is split.
    #[test]
    fn a_run_is_split_only_when_it_has_to_fill_several_fields() {
        assert_eq!(ymd(parse_date("2026-10-18", "yymmdd")), Some((2026, 10, 18)));
        assert_eq!(ymd(parse_date("2026-10", "yymm")), Some((2026, 10, 1)));
        assert_eq!(ymd(parse_date("2610", "yymm")), Some((2026, 10, 1)));
        assert_eq!(ymd(parse_date("202610 18", "yyyymmdd")), Some((2026, 10, 18)));
        assert_eq!(ymd(parse_date("18Oct2026", "ddmmmyyyy")), Some((2026, 10, 18)));
    }

    #[test]
    fn impossible_undelimited_dates_are_refused() {
        let f = Format::Date("yyyymmdd".into());
        assert!(keystroke(&f, "Date", "20261340").is_err(), "no month 13");
        assert!(keystroke(&f, "Date", "20260230").is_err(), "no 30 February");
        assert!(keystroke(&f, "Date", "2026101899").is_err(), "digits left over make no day");
        assert!(keystroke(&f, "Date", "2026").is_err(), "a year alone is not a date");
    }

    /// What is left of a split run fills its field exactly: "202610018" leaves "018" for the
    /// day, one digit too many, and used to pass as 18 October. Numbers typed on their own
    /// keep their leeway.
    #[test]
    fn the_rest_of_a_split_run_must_fill_its_field_exactly() {
        assert_eq!(parse_date("202610018", "yyyymmdd"), None, "one digit too many");
        assert_eq!(parse_date("2026101", "yyyymmdd"), None, "one digit too few");
        assert_eq!(parse_date("2026101814305", "yyyymmddHHMM"), None, "one digit too many in the time");
        assert_eq!(ymd(parse_date("2026 10 018", "yyyymmdd")), Some((2026, 10, 18)), "typed on its own, 018 is still a day");
        assert_eq!(parse_date("202610181430", "yyyymmddHHMM").map(|d| (d.d, d.hh, d.mm)), Some((18, 14, 30)));
    }

    /// Times may be left out, so a number typed on its own isn't split for them: "018" is the
    /// day in "2026 10 018" even when the format goes on to HHMM.
    #[test]
    fn a_number_typed_on_its_own_is_split_only_when_the_rest_fills_the_next_field() {
        let dt = parse_date("2026 10 018", "yyyymmddHHMM").expect("a date with the time left out");
        assert_eq!((dt.y, dt.m, dt.d, dt.hh, dt.mm), (2026, 10, 18, 0, 0));
        assert_eq!(ymd(parse_date("20261018", "yyyymmddHHMM")), Some((2026, 10, 18)));
        assert_eq!(parse_date("2026 10 18 1430", "yyyymmdd HHMM").map(|d| (d.hh, d.mm)), Some((14, 30)));
    }

    /// The date's runs are matched against the date's fields, the time's against the time's:
    /// optional times left out don't make a separated four-digit year look like it needs
    /// splitting, and a time typed in one run still fills HHMM.
    #[test]
    fn date_and_time_fields_are_counted_apart() {
        let dt = parse_date("2026-10-18", "yymmddHHMM").expect("a date with the time left out");
        assert_eq!((dt.y, dt.m, dt.d, dt.hh, dt.mm), (2026, 10, 18, 0, 0));
        assert_eq!(parse_date("20261018 1430", "yyyymmddHHMM").map(|d| (d.y, d.m, d.d, d.hh, d.mm)), Some((2026, 10, 18, 14, 30)));
        assert_eq!(parse_date("2026-10-18 1430", "yyyy-mm-dd HHMM").map(|d| (d.hh, d.mm)), Some((14, 30)));
        assert_eq!(parse_date("20261018143005", "yyyymmddHHMMss").map(|d| (d.d, d.hh, d.mm, d.ss)), Some((18, 14, 30, 5)));
    }

    /// A run of more digits than fit in a u32 ("yyyymmddHHMMss") is split, not dropped.
    #[test]
    fn long_runs_of_digits_split_into_dates_and_times() {
        let dt = parse_date("20261018143005", "yyyymmddHHMMss").expect("valid date and time");
        assert_eq!((dt.y, dt.m, dt.d, dt.hh, dt.mm, dt.ss), (2026, 10, 18, 14, 30, 5));
        assert_eq!(parse_date("1430", "HHMM").map(|d| (d.hh, d.mm)), Some((14, 30)));
        assert_eq!(parse_date("99999999999999999999", "yyyymmdd"), None);
        assert_eq!(parse_date("2026 99999999999", "yyyy"), None, "an unreadable number is left over, not ignored");
    }

    #[test]
    fn huge_finite_numbers_do_not_print_inf() {
        let n = format_value(&Format::Number { decimals: 2, sep: 1, neg: 0, currency: String::new(), prepend: true }, "1e307");
        assert!(n.len() > 300 && n.ends_with(".00") && !n.contains("inf"), "got {n}");
        let p = format_value(&Format::Percent { decimals: 2, sep: 1 }, "1e306");
        assert!(!p.contains("inf"), "got {p}");
    }
}
