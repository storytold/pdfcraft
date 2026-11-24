use super::*;

#[test]
fn format_colors_resolve_named_and_indexed_rgb_safely() {
    assert_eq!(FormatColor::Green.rgb(), Some([0, 255, 0]));
    assert_eq!(FormatColor::Black.rgb(), Some([0, 0, 0]));
    assert_eq!(FormatColor::Indexed(1).rgb(), Some([0, 0, 0]));
    assert_eq!(FormatColor::Indexed(10).rgb(), Some([0, 128, 0]));
    assert_eq!(FormatColor::Indexed(46).rgb(), Some([255, 102, 0]));
    assert_eq!(FormatColor::Indexed(56).rgb(), Some([51, 51, 51]));
    for index in [0, 57, 255] {
        assert_eq!(FormatColor::Indexed(index).rgb(), None);
    }
}

const S: DateSystem = DateSystem::D1900;

/// Built-in format 44 (accounting with a currency symbol, two decimals).
const ACCOUNTING_2DP: &str = "_(\"$\"* #,##0.00_);_(\"$\"* \\(#,##0.00\\);_(\"$\"* \"-\"??_);_(@_)";

fn f(code: &str, n: f64) -> String {
    format_value(&Value::Number(n), &NumberFormat::parse(code), S).text
}

fn full(code: &str, v: Value) -> Formatted {
    format_value(&v, &NumberFormat::parse(code), S)
}

fn t(code: &str, s: &str) -> String {
    format_value(&Value::from(s), &NumberFormat::parse(code), S).text
}

#[test]
fn numbers() {
    assert_eq!(f("0.00", 1234.567), "1234.57");
    assert_eq!(f("#,##0", 1234567.0), "1,234,567");
    assert_eq!(f("#,##0", -1234567.89), "-1,234,568");
    assert_eq!(f("#,##0.00", 0.0), "0.00");
    assert_eq!(f("0", 0.5), "1");
    assert_eq!(f("0", -2.5), "-3");
    assert_eq!(f("0.00", 2.675), "2.68");
    assert_eq!(f("0.00", 1.005), "1.01");
    assert_eq!(f("0.0", -0.04), "0.0");
    assert_eq!(f("00000", 123.0), "00123");
    assert_eq!(f("000-00-0000", 123456789.0), "123-45-6789");
    assert_eq!(f("#.##", 0.5), ".5");
    assert_eq!(f("0.##", 1.0), "1.");
    assert_eq!(f("#", 0.0), "");
    assert_eq!(f("0.0?", 1.5), "1.5 ");
    assert_eq!(f("??0.0", 1.5), "  1.5");
    assert_eq!(f(".00", 12.5), "12.50");
    assert_eq!(f("0", 1e20), "100000000000000000000");
    assert_eq!(f("0.0", 1e-20), "0.0");
    assert_eq!(f("0,000", 5.0), "0,005");
    assert_eq!(f("#,##0.000", 1234.5), "1,234.500");
}

#[test]
fn scaling_and_percent() {
    assert_eq!(f("#,##0,", 1234567.0), "1,235");
    assert_eq!(f("0.0,,", 12345678.0), "12.3");
    assert_eq!(f("0,\"K\"", 12345.0), "12K");
    assert_eq!(f("0%", 0.256), "26%");
    assert_eq!(f("0.00%", 0.12345), "12.35%");
    assert_eq!(f("0%%", 0.5), "5000%%");
    assert_eq!(f("0%", -0.5), "-50%");
    assert!(NumberFormat::parse("0.0%").is_percent());
    assert!(!NumberFormat::parse("0.0").is_percent());
}

#[test]
fn sections_and_colors() {
    let r = full("#,##0.00;[Red](#,##0.00)", Value::Number(-5.0));
    assert_eq!(r.text, "(5.00)");
    assert_eq!(r.color, Some(FormatColor::Red));
    assert!(r.numeric);
    let r = full("#,##0.00;[Red](#,##0.00)", Value::Number(5.0));
    assert_eq!((r.text.as_str(), r.color), ("5.00", None));
    assert_eq!(f("0;-0;;@", 0.0), "");
    assert_eq!(t("0;-0;;@", "hi"), "hi");
    assert_eq!(f("0;-0;\"zero\"", 0.0), "zero");
    assert_eq!(f("0;-0;\"zero\"", 0.001), "0");
    assert_eq!(f("0_);(0)", 5.0), "5 ");
    assert_eq!(f("0_);(0)", -5.0), "(5)");
    assert_eq!(f(";;;", 5.0), "");
    assert_eq!(t(";;;", "x"), "");
    assert_eq!(full("[Color10]0", Value::Number(1.0)).color, Some(FormatColor::Indexed(10)));
    assert_eq!(full("[blue]0", Value::Number(1.0)).color, Some(FormatColor::Blue));
    assert_eq!(full("[Color99]0", Value::Number(1.0)).color, None);
}

#[test]
fn conditions() {
    let c = "[>=100]\"big\";[<0]\"neg\";\"small\"";
    assert_eq!(f(c, 150.0), "big");
    assert_eq!(f(c, -5.0), "neg");
    assert_eq!(f(c, 50.0), "small");
    assert_eq!(f("[<1]0.00;0", 0.5), "0.50");
    assert_eq!(f("[<1]0.00;0", 5.0), "5");
    // Like Excel, a multi-section format never adds the minus sign itself.
    assert_eq!(f("[<1]0.00;0", -5.0), "5.00");
    assert_eq!(f("[>=1000]0,\"K\";0", -5.0), "5");
    assert_eq!(f("[>=1000]0,\"K\"", -5.0), "-5");
    let phone = "[<=9999999]###-####;(###) ###-####";
    assert_eq!(f(phone, 5551234.0), "555-1234");
    assert_eq!(f(phone, 2125551234.0), "(212) 555-1234");
    assert_eq!(full("[Red][<=100]0;[Blue][>100]0", Value::Number(150.0)).color, Some(FormatColor::Blue));
}

#[test]
fn scientific() {
    assert_eq!(f("0.00E+00", 12345.0), "1.23E+04");
    assert_eq!(f("0.00E+00", 0.000123), "1.23E-04");
    assert_eq!(f("0.00E+00", 0.0), "0.00E+00");
    assert_eq!(f("0.00E-00", 12345.0), "1.23E04");
    assert_eq!(f("0.00e+00", 12345.0), "1.23e+04");
    assert_eq!(f("##0.0E+0", 12345.0), "12.3E+3");
    assert_eq!(f("0.0E+0", 0.00001), "1.0E-5");
    assert_eq!(f("0.000E+00", 1e-300), "1.000E-300");
    assert_eq!(f("0.00E+00", 9.999), "1.00E+01");
    assert_eq!(f("0.00E+00", -12345.0), "-1.23E+04");
}

/// A negative number that rounds to zero in the first section shows no minus sign; a negative
/// section of its own keeps its text (observed Excel results).
#[test]
fn negative_rounding_to_zero_drops_the_sign() {
    let cases: &[(&str, f64, &str)] = &[
        ("0.00", -0.0033, "0.00"),
        ("0.00", -2.89e-9, "0.00"),
        ("0.00", -0.005, "-0.01"),
        ("0", -0.4, "0"),
        ("0", -0.5, "-1"),
        ("0.0", -0.004, "0.0"),
        ("#,##0.00", -0.004, "0.00"),
        ("0.00%", -2.89e-9, "0.00%"),
        ("0.00%", -0.0033, "-0.33%"),
        ("0.00;(0.00)", -0.0033, "(0.00)"),
        ("0.00;-0.00;\"zero\"", -0.0033, "-0.00"),
        ("0.00E+00", -2.89e-9, "-2.89E-09"),
    ];
    for &(code, n, want) in cases {
        assert_eq!(f(code, n), want, "{code} with {n}");
    }
}

#[test]
fn fractions() {
    assert_eq!(f("# ?/?", 1.5), "1 1/2");
    assert_eq!(f("# ?/?", 0.25), " 1/4");
    assert_eq!(f("# ?/?", 2.0), "2    ");
    assert_eq!(f("# ?/?", 0.0), "0    ");
    assert_eq!(f("# ?/?", -1.5), "-1 1/2");
    assert_eq!(f("# ??/??", std::f64::consts::PI), "3 14/99");
    assert_eq!(f("# ??/??", 1.0 / 3.0), "  1/3 ");
    assert_eq!(f("?/8", 0.5), "4/8");
    assert_eq!(f("# ?/8", 2.125), "2 1/8");
    assert_eq!(f("?/?", 1.5), "3/2");
    assert_eq!(f("# ?/?", 0.99), "1    ");
    assert_eq!(f("# ???/???", 0.123), " 100/813");
    assert_eq!(f("# ???/???", 0.5), "   1/2  ");
}

/// A fraction without a whole part puts the whole value in the numerator; large values must
/// not saturate at u64::MAX.
#[test]
fn fraction_numerator_beyond_u64() {
    assert_eq!(f("?/1000000000", 2e10), "20000000000000000000/1000000000");
    assert_eq!(f("?/?", 1e20), "100000000000000000000/1");
    assert_eq!(f("?/100", 1e18), "100000000000000000000/100");
    assert_eq!(f("??/??", 1e25), "10000000000000000000000000/1 ");
    assert!(!f("?/10", 1e300).contains("18446744073709551615"));
    assert_eq!(f("?/1000000000", 2.5), "2500000000/1000000000");
}

/// Fraction sections with literal text, as Excel renders them (observed `TEXT()` results).
#[test]
fn fractions_with_literals() {
    let cases: &[(&str, f64, &str)] = &[
        // Literals between the parts and around the bar.
        (r"|#\:#/#|", 23.75, "|23:3/4|"),
        (r"|#\:#=/=#|", 23.75, "|23:3=/=4|"),
        (r"|#\:#=/=#|", -23.75, "-|23:3=/=4|"),
        (r"|#\:? ?=/=#|", 23.75, "|2:3 3=/=4|"),
        (r"|#-#-#\:#/#|", 23.75, "|-2-3:3/4|"),
        // A whole part without digits drops its separator (spaces next to a `?`).
        (r"|#\:#/#|", 0.75, "|3/4|"),
        (r"|#\:#/#|", -0.75, "-|3/4|"),
        (r"|#-#-#\:#/#|", 0.75, "|--3/4|"),
        (r"|#\:#=/=#|", 0.75, "|3=/=4|"),
        (r"|#\:0=/=?|", 0.75, "|3=/=4|"),
        (r"|#\:?=/=#|", 0.75, "| 3=/=4|"),
        (r"|?\:#=/=#|", 0.75, "|  3=/=4|"),
        (r"|?\:0=/=0|", -0.75, "-|  3=/=4|"),
        (r"|?\:?0=/=#|", 0.75, "|   3=/=4|"),
        (r"|0\:#=/=#|", 0.75, "|0:3=/=4|"),
        // A zero fraction hides separator, numerator, bar and denominator (spaces if any `?`).
        (r"|#\:#/#|", 0.0, "|0|"),
        (r"|#\:#/#|", 1.0, "|1|"),
        (r"|#\:#/#|", -1.0, "-|1|"),
        (r"|#-#-#\:#/#|", 0.0, "|--0|"),
        (r"|#-#-#\:#/#|", 1.0, "|--1|"),
        (r"|#\:#=/=0|", 0.0, "|0|"),
        (r"|0\:#=/=#|", 1.0, "|1|"),
        (r"|#\:#=/=?|", 0.0, "|0      |"),
        (r"|#\:?=/=0|", 1.0, "|1      |"),
        (r"|?\:#=/=#|", 0.0, "|0      |"),
        (r"|?\:#=/=#|", -1.0, "-|1      |"),
        (r"|#\:? ?=/=#|", 0.0, "|:0      |"),
        (r"|#\:? ?=/=?|", 1.0, "|:1      |"),
        // ... unless the numerator has a `0` placeholder: then it shows 0/1.
        (r"|#\:0=/=#|", 0.0, "|0=/=1|"),
        (r"|?\:0=/=?|", 0.0, "|0:0=/=1|"),
        (r"|0\:0=/=0|", 0.0, "|0:0=/=1|"),
        (r"|#\:0=/=#|", 1.0, "|1:0=/=1|"),
        (r"|?\:0=/=#|", -1.0, "-|1:0=/=1|"),
        (r"|#\:? ?0#/000", 0.0, "|:0  00/001"),
        (r"|#\:? ?0#/000", 1.0, "|:1  00/001"),
        // `0` denominator placeholders zero pad on the left.
        (r"|#\:? ?0#/000", 23.75, "|2:3  03/004"),
        (r"|#\:? ?#/#|", 23.75, "|2:3  3/4|"),
        // Literal text before the bar: still an improper fraction.
        (r"|#_#/#|", 3.75, "|15 /4|"),
        (r"|#_#/#|", -0.75, "-|3 /4|"),
        (r"|#_#/#|", 0.0, "|0 /1|"),
        (r"|#_#/#|", -1.0, "-|1 /1|"),
        (r"|#_#=/=?|", 3.75, "|15 =/=4|"),
        (r"|#_?=/=0|", 0.0, "|0 =/=1|"),
        (r"|#_?=/=?|", 1.0, "|1 =/=1|"),
    ];
    for &(code, v, want) in cases {
        assert_eq!(f(code, v), want, "{code} with {v}");
    }
}

#[test]
fn question_mark_pads_thousands_separator() {
    let cases: &[(&str, f64, &str)] = &[
        ("?,???????", 1234567.0, " 1,234,567"),
        ("?,????????", 1234567.0, "  1,234,567"),
        ("?,?????????", 1234567.0, "    1,234,567"),
        ("?,?????????", -1234567.0, "-    1,234,567"),
        ("?,??????", 1234567.0, "1,234,567"),
        ("?,????", 12345.0, "12,345"),
        ("0,000000000", 1234567.0, "0,001,234,567"),
    ];
    for &(code, v, want) in cases {
        assert_eq!(f(code, v), want, "{code} with {v}");
    }
}

#[test]
fn literals_and_fill() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(f("\"$\"#,##0.00", 3.0), "$3.00");
    assert_eq!(f("$#,##0.00", -3.0), "-$3.00");
    assert_eq!(f("[$€-407]#,##0.00", 1234.5), "€1,234.50");
    assert_eq!(f("0\" units\"", 5.0), "5 units");
    assert_eq!(f("\\A0", 5.0), "A5");
    assert_eq!(f("(0)", 5.0), "(5)");
    let r = full("*-0", Value::Number(5.0));
    assert_eq!((r.text.as_str(), r.fill), ("5", Some(('-', 0))));
    let acct = ACCOUNTING_2DP;
    let r = full(acct, Value::Number(0.0));
    assert_eq!((r.text.as_str(), r.fill), (" $-   ", Some((' ', 2))));
    let r = full(acct, Value::Number(1234.5));
    assert_eq!((r.text.as_str(), r.fill), (" $1,234.50 ", Some((' ', 2))));
    let r = full(acct, Value::Number(-1234.5));
    assert_eq!((r.text.as_str(), r.fill), (" $(1,234.50)", Some((' ', 2))));
    let r = full(acct, Value::from("abc"));
    assert_eq!((r.text.as_str(), r.numeric), (" abc ", false));
    let r = full("-* 0", Value::Number(-5.0));
    assert_eq!((r.text.as_str(), r.fill), ("--5", Some((' ', 2))));
    Ok(())
}

#[test]
fn dates_and_times() {
    assert_eq!(f("m/d/yyyy", 46302.0), "10/7/2026");
    assert_eq!(f("dddd, mmmm d, yyyy", 46302.0), "Wednesday, October 7, 2026");
    assert_eq!(f("ddd mmm yy", 46302.0), "Wed Oct 26");
    assert_eq!(f("mmmmm", 46302.0), "O");
    assert_eq!(f("dd/mm/yyyy", 46302.0), "07/10/2026");
    assert_eq!(f("d-mmm-yy", 46302.0), "7-Oct-26");
    assert_eq!(f("e", 46302.0), "2026");
    assert_eq!(f("YYYY-MM-DD", 46302.0), "2026-10-07");
}

#[test]
fn time_formats() {
    assert_eq!(f("yyyy-mm-dd hh:mm:ss", 46302.5), "2026-10-07 12:00:00");
    assert_eq!(f("h:mm AM/PM", 0.75), "6:00 PM");
    assert_eq!(f("h AM/PM", 0.0), "12 AM");
    assert_eq!(f("h a/p", 0.25), "6 a");
    assert_eq!(f("hh:mm", 0.75), "18:00");
    assert_eq!(f("[h]:mm", 1.5), "36:00");
    assert_eq!(f("[h]:mm:ss", 2.25), "54:00:00");
    assert_eq!(f("[mm]:ss", 1.0 / 24.0), "60:00");
    assert_eq!(f("[s]", 1.0 / 1440.0), "60");
    assert_eq!(f("mm:ss.0", 61.2 / 86400.0), "01:01.2");
    assert_eq!(f("mm:ss.00", 61.25 / 86400.0), "01:01.25");
}

#[test]
fn date_serial_boundaries() {
    assert_eq!(f("h:mm:ss", 0.5 + 59.6 / 86400.0), "12:01:00");
    assert_eq!(f("h:mm", 0.5 + 40.0 / 86400.0), "12:00");
    assert_eq!(f("m/d/yyyy", 60.0), "2/29/1900");
    assert_eq!(f("m/d/yyyy", 0.0), "1/0/1900");
    assert_eq!(format_value(&Value::Number(0.0), &NumberFormat::parse("m/d/yyyy"), DateSystem::D1904).text, "1/1/1904");
    let fmt = NumberFormat::parse("m/d/yyyy");
    assert_eq!(format_value(&Value::Number(2957003.5), &fmt, DateSystem::D1904).text, "12/31/9999");
    assert_eq!(format_value(&Value::Number(2957004.0), &fmt, DateSystem::D1904).text, "########");
    assert_eq!(format_value(&Value::Number(2958465.0), &fmt, DateSystem::D1900).text, "12/31/9999");
    assert_eq!(format_value(&Value::Number(2958466.0), &fmt, DateSystem::D1900).text, "########");
    assert_eq!(f("m/d/yyyy", -1.0), "########");
    assert_eq!(f("m/d/yyyy", 3e6), "########");
    assert_eq!(f("[$-409]m/d/yyyy", 46302.0), "10/7/2026");
    assert_eq!(f("m/d/yyyy h:mm", 46302.75), "10/7/2026 18:00");
    assert_eq!(f("mmss.0", 0.0), "0000.0");
    assert!(NumberFormat::parse("h:mm").is_date());
    assert!(!NumberFormat::parse("0.00").is_date());
}

#[test]
fn text_and_other_values() {
    assert_eq!(t("@", "hello"), "hello");
    assert_eq!(t("\"<\"@\">\"", "x"), "<x>");
    assert_eq!(t("0.00", "abc"), "abc");
    assert_eq!(f("@", 5.0), "5");
    assert!(NumberFormat::parse("@").is_text());
    assert!(!NumberFormat::parse("0;@").is_text());
    let g = NumberFormat::general();
    assert_eq!(format_value(&Value::Bool(true), &g, S).text, "TRUE");
    assert_eq!(format_value(&Value::Bool(false), &NumberFormat::parse("0.00"), S).text, "FALSE");
    assert_eq!(format_value(&Value::Error(CellError::Div0), &g, S).text, "#DIV/0!");
    assert_eq!(format_value(&Value::Empty, &g, S), Formatted::default());
    let r = format_value(&Value::from("x"), &g, S);
    assert!(!r.numeric);
}

#[test]
fn general() {
    assert_eq!(f("General", 1e11), "1E+11");
    assert_eq!(f("General", 12345678901.0), "12345678901");
    assert_eq!(f("General", 0.1 + 0.2), "0.3");
    assert_eq!(f("General", -1234.5), "-1234.5");
    assert_eq!(f("", 1.0 / 3.0), "0.333333333");
    assert_eq!(f("general", 123456.789012), "123456.789");
    assert_eq!(f("General\" units\"", 5.0), "5 units");
    assert_eq!(f("General;[Red]-General", -5.0), "-5");
    assert!(NumberFormat::parse("General").is_general());
    assert!(NumberFormat::default().is_general());
    assert_eq!(NumberFormat::parse("0.0").code(), "0.0");
}

#[test]
fn kinds() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(NumberFormat::general().kind(), FormatKind::General);
    assert_eq!(NumberFormat::parse("0.00").kind(), FormatKind::Number);
    assert_eq!(NumberFormat::parse("#,##0.00;[Red](#,##0.00)").kind(), FormatKind::Number);
    assert_eq!(NumberFormat::parse("\"$\"#,##0.00").kind(), FormatKind::Currency);
    assert_eq!(NumberFormat::parse(ACCOUNTING_2DP).kind(), FormatKind::Accounting);
    assert_eq!(NumberFormat::parse("m/d/yyyy").kind(), FormatKind::Date);
    assert_eq!(NumberFormat::parse("h:mm AM/PM").kind(), FormatKind::Time);
    assert_eq!(NumberFormat::parse("0%").kind(), FormatKind::Percentage);
    assert_eq!(NumberFormat::parse("# ?/?").kind(), FormatKind::Fraction);
    assert_eq!(NumberFormat::parse("0.00E+00").kind(), FormatKind::Scientific);
    assert_eq!(NumberFormat::parse("@").kind(), FormatKind::Text);
    assert_eq!(NumberFormat::parse("00000").kind(), FormatKind::Special);
    assert_eq!(NumberFormat::parse("\"hello\"").kind(), FormatKind::Custom);
    Ok(())
}

#[test]
fn builtin_codes_render() {
    // Built-in ids 5, 12 and 47 (the id tables live with the file formats, in gridcraft-xlsx).
    assert_eq!(f("\"$\"#,##0_);\\(\"$\"#,##0\\)", -1234.0), "($1,234)");
    assert_eq!(f("# ?/?", 0.5), " 1/2");
    assert_eq!(f("mmss.0", 61.2 / 86400.0), "0101.2");
}

#[test]
fn text_fn() {
    assert_eq!(text_function(&Value::Number(1234.5), "$#,##0.00", S), Ok("$1,234.50".into()));
    assert_eq!(text_function(&Value::from("5"), "0.00", S), Ok("5.00".into()));
    assert_eq!(text_function(&Value::from("abc"), "0.00", S), Ok("abc".into()));
    // Each `@` repeats the text: up to a cell's 32,767 characters, then #VALUE!.
    let at = "@".repeat(255);
    assert_eq!(text_function(&Value::from("x".repeat(128).as_str()), &at, S).map(|s| s.len()), Ok(128 * 255));
    assert_eq!(text_function(&Value::from("x".repeat(32_000).as_str()), &at, S), Err(CellError::Value));
    // The limit counts UTF-16 units: 16,384 emoji are 32,768 of them, one too many.
    assert_eq!(text_function(&Value::from("\u{1F600}".repeat(16_383).as_str()), "@", S).map(|s| s.chars().count()), Ok(16_383));
    assert_eq!(text_function(&Value::from("\u{1F600}".repeat(16_384).as_str()), "@", S), Err(CellError::Value));
    assert_eq!(text_function(&Value::Error(CellError::NA), "0", S), Err(CellError::NA));
    assert_eq!(text_function(&Value::Number(46302.0), "yyyy", S), Ok("2026".into()));
    assert_eq!(text_function(&Value::Number(0.5), "General", S), Ok("0.5".into()));
    assert_eq!(text_function(&Value::Number(1.0 / 3.0), "0.000", S), Ok("0.333".into()));
    assert_eq!(text_function(&Value::Empty, "0.00", S), Ok("0.00".into()));
    assert_eq!(text_function(&Value::Number(5.0), "", S), Ok("".into()));
    assert_eq!(text_function(&Value::Number(-1.0), "m/d/yyyy", S), Err(CellError::Value));
    assert_eq!(text_function(&Value::Number(2957003.0), "m/d/yyyy", DateSystem::D1904), Ok("12/31/9999".into()));
    assert_eq!(text_function(&Value::Number(2957004.0), "m/d/yyyy", DateSystem::D1904), Err(CellError::Value));
    assert_eq!(text_function(&Value::Bool(true), "0", S), Ok("TRUE".into()));
    assert_eq!(text_function(&Value::Number(-0.0033), "0", S), Ok("0".into()));
    assert_eq!(text_function(&Value::Number(-0.4), "0", S), Ok("0".into()));
    // Booleans go through the text section as TRUE/FALSE (observed Excel results).
    let cases: &[(bool, &str, &str)] = &[
        (true, ";;;@", "TRUE"),
        (true, ";;;\"hi\"", "hi"),
        (true, ";;;\"h\"\"I\"", "hI"),
        (false, ";;;\\h\\i", "hi"),
        (true, ";;;\\@@\\@", "@TRUE@"),
        (false, ";;;-@-", "-FALSE-"),
        (true, ";;;@-", "TRUE-"),
        (false, ";;;-@-@-", "-FALSE-FALSE-"),
        (true, "0.00", "TRUE"),
    ];
    for &(b, code, want) in cases {
        assert_eq!(text_function(&Value::Bool(b), code, S), Ok(want.into()), "{code} with {b}");
    }
}

/// Deterministic xorshift for fuzzing.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

#[test]
fn fuzz_no_panic() {
    let pieces = [
        "0",
        "#",
        "?",
        ".",
        ",",
        "%",
        "E+",
        "E-",
        "e",
        "/",
        "8",
        "16",
        "\"",
        "\\",
        "_",
        "*",
        "@",
        ";",
        "[",
        "]",
        "[Red]",
        "[>=100]",
        "[<0]",
        "[Color7]",
        "[$€-407]",
        "[$-409]",
        "[h]",
        "[mm]",
        "[s]",
        "d",
        "dd",
        "ddd",
        "dddd",
        "m",
        "mm",
        "mmm",
        "mmmmm",
        "yy",
        "yyyy",
        "h",
        "hh",
        "s",
        "ss",
        ".000",
        "AM/PM",
        "A/P",
        "General",
        " ",
        "-",
        "(",
        ")",
        "$",
        "é",
        "€",
        "💥",
        "b",
        "g",
        "[",
        "[>",
        "[=",
        "\"abc",
        "*",
        "_",
        "00000000000000000000",
        "##########",
        ",,,,",
        "%%%%%%",
        "E+0000",
        "?/?????????",
        "/0",
    ];
    let values = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        1e-300,
        -1e-300,
        1e300,
        -1e300,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        2958465.999999,
        2958466.0,
        46302.75,
        123_456_789.123_456_78,
        -987654.321,
        1.0 / 3.0,
        std::f64::consts::PI,
        1e15,
        1e16,
        0.999999999,
        f64::NAN,
        f64::INFINITY,
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..3000 {
        let n = (rng.next() % 8) as usize;
        let mut code = String::new();
        for _ in 0..n {
            code.push_str(rng.pick(&pieces));
        }
        let fmt = NumberFormat::parse(&code);
        let _ = (fmt.kind(), fmt.is_date(), fmt.is_percent(), fmt.is_text(), fmt.code());
        for &v in &values {
            let r = format_value(&Value::Number(v), &fmt, S);
            if let Some((_, pos)) = r.fill {
                assert!(r.text.is_char_boundary(pos), "{code:?} {v}");
            }
            let _ = format_value(&Value::Number(v), &fmt, DateSystem::D1904);
            let _ = text_function(&Value::Number(v), &code, S);
        }
        let _ = format_value(&Value::from("text"), &fmt, S);
        let _ = text_function(&Value::from("12"), &code, S);
    }
    // Random bytes as codes.
    for _ in 0..500 {
        let len = (rng.next() % 24) as usize;
        let code: String = (0..len).map(|_| char::from_u32((rng.next() % 0x3000) as u32).unwrap_or('x')).collect();
        let fmt = NumberFormat::parse(&code);
        for _ in 0..5 {
            let v = f64::from_bits(rng.next());
            let _ = format_value(&Value::Number(v), &fmt, S);
        }
    }
    for w in 0..20 {
        for &v in &values {
            if let Some(s) = format_general_fit(v, w) {
                assert!(s.chars().count() <= w, "{v} {w} {s}");
            }
        }
    }
}

// ---- 15 significant digits: an exact tie at the 16th digit goes toward zero ----
// Windows Excel 16.0.20430 TEXT() over the exact doubles below. An odd 15th digit is where
// round-half-even and toward-zero differ; an even one is where they agree.

#[test]
fn an_exact_tie_at_the_sixteenth_digit_goes_toward_zero() {
    for (v, whole) in [
        (1234567890123405.0, "1234567890123400"),
        (1234567890123415.0, "1234567890123410"),
        (1234567890123425.0, "1234567890123420"),
        (1234567890123435.0, "1234567890123430"),
        (1234567890123455.0, "1234567890123450"),
        (1234567890123475.0, "1234567890123470"),
        (1234567890123495.0, "1234567890123490"),
        (6631679258820775.0, "6631679258820770"),
        (8052991635850695.0, "8052991635850690"),
    ] {
        assert_eq!(f("0", v), whole, "0 over {v}");
        assert_eq!(f("0.00", v), format!("{whole}.00"), "0.00 over {v}");
        assert_eq!(f("0", -v), format!("-{whole}"), "0 over -{v}");
    }
    assert_eq!(f("#,##0", 1234567890123455.0), "1,234,567,890,123,450");
    assert_eq!(f("#,##0", -3267527853107975.0), "-3,267,527,853,107,970");
    // A fraction is a tie as well, when it is exact in binary.
    assert_eq!(f("0", 123456789012345.5), "123456789012345");
    assert_eq!(f("0.00", 123456789012345.5), "123456789012345.00");
    // Excel observed: no carry after fifteen 9s.
    assert_eq!(f("0", 999999999999999.5), "999999999999999");
    assert_eq!(f("0", -999999999999999.5), "-999999999999999");
    assert_eq!(f("0", 4999999999999995.0), "4999999999999990");
    assert_eq!(f("#,##0", -4999999999999995.0), "-4,999,999,999,999,990");
}

#[test]
fn only_exact_ties_go_toward_zero() {
    // Just above and just below a tie round to the nearer 15-digit value (Excel observed).
    assert_eq!(f("0", 1234567890123455.0 + 0.25), "1234567890123460");
    assert_eq!(f("0", 1234567890123455.5), "1234567890123460");
    assert_eq!(f("0", 1234567890123455.0 - 0.25), "1234567890123450");
    // 16 digits that are not a tie, and the ordinary decimal cases, are untouched.
    assert_eq!(f("0", 1234567890123456.0), "1234567890123460");
    assert_eq!(f("0", 1234567890123454.0), "1234567890123450");
    assert_eq!(f("0.00", 2.675), "2.68");
    assert_eq!(f("0.00", 1.005), "1.01");
    assert_eq!(f("0.0", 0.285), "0.3");
    assert_eq!(f("0.0", 99999999999999.95), "100000000000000.0");
    // Ten digits and a half are not a 16th-digit tie.
    assert_eq!(f("0", 0.5), "1");
    assert_eq!(f("0", 2.5), "3");
}

#[test]
fn decimal_digits_keeps_fifteen_digits_and_truncates_a_tie() {
    use crate::decimal::decimal_digits;
    let (d, e) = decimal_digits(1234567890123455.0);
    assert_eq!((d, e), ([1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1, 2, 3, 4, 5], 15));
    let (d, e) = decimal_digits(1234567890123456.0);
    assert_eq!((d, e), ([1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1, 2, 3, 4, 6], 15));
    let (d, e) = decimal_digits(123456789012345.5);
    assert_eq!((d, e), ([1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 1, 2, 3, 4, 5], 14));
    // Excel observed: a tie after fifteen 9s does not carry (999999999999999.5 shows 999999999999999).
    let (d, e) = decimal_digits(999999999999999.5);
    assert_eq!((d, e), ([9; 15], 14));
    let (d, e) = decimal_digits(4999999999999995.0);
    assert_eq!((d, e), ([4, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9], 15));
    // A 16th digit above 5 carries as usual.
    let (d, e) = decimal_digits(999999999999999.6);
    assert_eq!((d, e), ([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 15));
}

// ---- parse cache ----

/// `TEXT()` as it was before the cache: parse the code, then format.
fn text_uncached(v: &Value, code: &str, sys: DateSystem) -> Result<String, CellError> {
    if code.is_empty() {
        return Ok(String::new());
    }
    let fmt = NumberFormat::parse(code);
    let n = match v.scalar() {
        Value::Error(e) => return Err(e),
        Value::Empty => 0.0,
        Value::Number(n) => n,
        Value::Bool(b) => return Ok(fmt.format_text(if b { "TRUE" } else { "FALSE" }).text),
        Value::Text(t) => match gridcraft_core::parse::parse_number_text(&t) {
            Some(n) => n,
            None => return Ok(fmt.format_text(&t).text),
        },
        Value::Array(_) => return Err(CellError::Value),
    };
    if !n.is_finite() {
        return Err(CellError::Num);
    }
    let (f, overflow) = fmt.format_number(n, sys, false);
    if overflow { Err(CellError::Value) } else { Ok(f.text) }
}

const CACHE_CODES: &[&str] = &[
    "0",
    "0.00",
    "#,##0.00",
    "#,##0.00;[Red](#,##0.00)",
    "$#,##0.00",
    "0%",
    "0.0%",
    "0.00E+00",
    "##0.0E+0",
    "General",
    "general",
    "checkbox",
    "@",
    "0;@",
    "0;-0;;@",
    "00000",
    "# ?/?",
    "# ??/16",
    "?/?",
    "m/d/yyyy",
    "yyyy-mm-dd hh:mm:ss",
    "dddd, mmmm d, yyyy",
    "h:mm AM/PM",
    "[h]:mm:ss",
    "mm:ss.000",
    "[Red]0;[Blue]-0",
    "[>100]0.0;[<5]0.000;0",
    "[<=9999999]###-####;(###) ###-####",
    "[$-409]d mmm yy",
    "\"abc\"0",
    "\\x0",
    "*-0",
    "_(* #,##0_)",
    "0.0.0",
    "[",
    "\"abc",
    "0;0;0;0;0",
    "#,##0.00;[Red",
    "[$-]0",
    "  0.00  ",
    "[Color99]0",
    "0.00_);(0.00)",
    "#,,\"M\"",
    "yyyy",
    "",
];

fn cache_values() -> Vec<Value> {
    vec![
        Value::Number(0.0),
        Value::Number(1234.5678),
        Value::Number(-42.5),
        Value::Number(0.5),
        Value::Number(45000.75),
        Value::Number(1e15),
        Value::Number(1e-7),
        Value::Number(-0.0001),
        Value::Number(f64::INFINITY),
        Value::from("abc"),
        Value::from("12.5"),
        Value::Bool(true),
        Value::Empty,
        Value::Error(CellError::NA),
    ]
}

#[test]
fn parse_cached_gives_the_same_results_cold_and_warm() {
    assert!(CACHE_CODES.len() >= 40);
    parse_cache::clear();
    let values = cache_values();
    for sys in [DateSystem::D1900, DateSystem::D1904] {
        // Cold on the first system, warm on the second.
        for code in CACHE_CODES {
            let direct = NumberFormat::parse(code);
            let cached = NumberFormat::parse_cached(code);
            assert_eq!(cached.code(), direct.code(), "{code:?}");
            assert_eq!(
                (cached.kind(), cached.is_date(), cached.is_text(), cached.is_general()),
                (direct.kind(), direct.is_date(), direct.is_text(), direct.is_general())
            );
            for v in &values {
                let want = text_uncached(v, code, sys);
                assert_eq!(text_function(v, code, sys), want, "TEXT({v:?}, {code:?})");
                // Again, now certainly warm.
                assert_eq!(text_function(v, code, sys), want, "warm TEXT({v:?}, {code:?})");
                assert_eq!(format_value(v, &cached, sys), format_value(v, &direct, sys), "{v:?} {code:?}");
            }
        }
    }
}

#[test]
fn parse_cached_parses_each_code_once() {
    parse_cache::clear();
    let before = parse_cache::parses();
    for i in 0..10_000 {
        assert!(text_function(&Value::Number(f64::from(i) + 0.5), "0.00", S).is_ok());
    }
    assert_eq!(parse_cache::parses() - before, 1);

    let before = parse_cache::parses();
    for i in 0..10_000 {
        let code = if i % 2 == 0 { "0.00" } else { "#,##0.0" };
        assert!(text_function(&Value::Number(f64::from(i)), code, S).is_ok());
    }
    // "0.00" is still cached from above: only the second code is new.
    assert_eq!(parse_cache::parses() - before, 1);

    parse_cache::clear();
    let before = parse_cache::parses();
    for i in 0..10_000 {
        let code = if i % 2 == 0 { "0.00" } else { "#,##0.0" };
        assert!(text_function(&Value::Number(f64::from(i)), code, S).is_ok());
    }
    assert_eq!(parse_cache::parses() - before, 2);
}

#[test]
fn parse_cache_is_bounded() {
    parse_cache::clear();
    // More distinct codes than the capacity: all still format correctly...
    let n = parse_cache::CAPACITY + 10;
    for i in 0..n {
        let c = format!("0.00\"{i}\"");
        assert_eq!(text_function(&Value::Number(1.0), &c, S), Ok(format!("1.00{i}")));
    }
    // ...the first ones were evicted (parsed again), the newest is still cached.
    let before = parse_cache::parses();
    let newest = format!("0.00\"{}\"", n - 1);
    assert!(text_function(&Value::Number(1.0), &newest, S).is_ok());
    assert_eq!(parse_cache::parses() - before, 0);
    assert!(text_function(&Value::Number(1.0), "0.00\"0\"", S).is_ok());
    assert_eq!(parse_cache::parses() - before, 1);

    // A code too long (in bytes) for the cache is parsed on every call and not stored. Two-byte
    // characters keep it under the 255-character cap that TEXT() refuses past.
    let long = format!("0\"{}\"", "é".repeat(parse_cache::MAX_CODE_LEN / 2 + 8));
    let before = parse_cache::parses();
    for _ in 0..3 {
        assert!(text_function(&Value::Number(1.0), &long, S).is_ok());
    }
    assert_eq!(parse_cache::parses() - before, 3);
}

#[test]
fn fraction_denominator_with_65536_zeros_does_not_panic() {
    // `{:0>n$}` panics for a width above u16::MAX; the cap keeps this out of reach of `parse`,
    // so the renderer is driven with the uncapped parser.
    let code = format!("# ?/{}", "0".repeat(65_536));
    let fmt = NumberFormat::parse_uncapped(&code);
    let text = format_value(&Value::Number(0.5), &fmt, S).text;
    assert!(text.starts_with(" 1/"), "{text}");
    // And a denominator with fewer digits than zeros is still zero padded.
    assert_eq!(f("# ?/000", 0.5), " 1/002");
}

#[test]
fn format_codes_are_capped_at_255_characters() {
    let ok = format!("0{}", "0".repeat(254));
    let long = format!("0{}", "0".repeat(255));
    assert_eq!(ok.chars().count(), MAX_FORMAT_CODE_CHARS);
    assert!(text_function(&Value::Number(1.0), &ok, S).is_ok());
    assert_eq!(text_function(&Value::Number(1.0), &long, S), Err(CellError::Value));
    // Capped codes display as General.
    assert_eq!(f(&long, 0.5), "0.5");
    // Characters are counted, not bytes: 255 two-byte characters are fine, 256 are not.
    let multi_ok = "é".repeat(255);
    assert!(multi_ok.len() > 255);
    assert!(text_function(&Value::Number(1.0), &multi_ok, S).is_ok());
    assert_eq!(text_function(&Value::Number(1.0), &"é".repeat(256), S), Err(CellError::Value));
    // 65536 zeros through the public API: no panic, TEXT is #VALUE!.
    let huge = format!("# ?/{}", "0".repeat(65_536));
    assert_eq!(text_function(&Value::Number(0.5), &huge, S), Err(CellError::Value));
    assert_eq!(f(&huge, 0.5), "0.5");
}

/// Parses and renders `code` on a worker thread; errors if that takes more than 20 s, so a quadratic
/// parser fails the test instead of hanging CI. Not a benchmark: at this size an O(n^2) pass takes
/// minutes and an O(n) pass milliseconds, so the margin is decades wide.
fn finishes_in_time(code: String) -> Result<String, std::sync::mpsc::RecvTimeoutError> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Uncapped: `parse` turns a code past 255 characters into General before the parser sees it.
        let _ = tx.send(format_value(&Value::Number(1234.5), &NumberFormat::parse_uncapped(&code), S).text);
    });
    rx.recv_timeout(std::time::Duration::from_secs(20))
}

#[test]
fn hostile_codes_parse_in_linear_time() -> Result<(), std::sync::mpsc::RecvTimeoutError> {
    let n = 1_000_000;
    // Unclosed brackets, a run of commas after a digit, a run of fraction bars.
    assert!(finishes_in_time("[".repeat(n))?.starts_with('['));
    finishes_in_time(format!("0{}", ",".repeat(n)))?;
    finishes_in_time(format!("0{}", "/".repeat(n)))?;
    finishes_in_time(format!("0{}0", ",".repeat(n)))?;
    // Unclosed brackets mixed with semicolons: each is a literal and a section separator stays one.
    finishes_in_time("[;".repeat(n / 2))?;
    Ok(())
}

#[test]
fn unclosed_bracket_is_a_literal_in_both_passes() {
    // Sections still split after an unclosed `[`.
    assert_eq!(f("[0;0", 5.0), "[5");
    assert_eq!(f("0;[0", -5.0), "[5");
    assert_eq!(f("0;-0;[", 0.0), "[");
    assert_eq!(parse::parse_sections("0;[Red-0").len(), 2);
    assert_eq!(parse::parse_sections("[0;0").len(), 2);
}

#[test]
fn closed_brackets_still_hold_their_semicolons() {
    assert_eq!(f("[Red]0;[Blue]-0", -5.0), "-5");
    assert_eq!(f("[>=100]0.0;0", 150.0), "150.0");
    assert_eq!(f("[$-409]0.00", 2.5), "2.50");
    assert_eq!(f("[$€-2] #,##0", 1234.0), "€ 1,234");
    assert_eq!(parse::parse_sections("[Red]0;[Blue]0").len(), 2);
    assert_eq!(parse::parse_sections("[unknown;x]0;0").len(), 2);
    assert_eq!(f("0\\[0", 5.0), "0[5");
    assert_eq!(f("\"[\"0;0", 5.0), "[5");
}

const JAN_1_2001: f64 = 36892.0;

#[test]
fn b1_prefix_is_hidden_and_gregorian() {
    assert_eq!(f("B1dd/mm/yyyy", 46314.0), "19/10/2026");
    assert_eq!(f("b1dddd, mmmm d, yyyy", 46314.0), "Monday, October 19, 2026");
    assert_eq!(f("B1yyyy-mm-dd hh:mm", 46314.5), "2026-10-19 12:00");
}

#[test]
fn b2_prefix_renders_hijri_date() {
    assert_eq!(f("B2dd/mm/yyyy", JAN_1_2001), "06/10/1421");
    assert_eq!(f("b2d/m/yy", JAN_1_2001), "6/10/21");
    assert_eq!(f("B2dddd dd/mm/yyyy hh:mm", JAN_1_2001 + 0.75), "الإثنين 06/10/1421 18:00");
    assert_eq!(text_function(&Value::Number(46314.0), "B2dd/mm/yyyy", S).as_deref(), Ok("08/05/1448"));
    assert_eq!(f("B2e", 46314.0), "1448");
    assert_eq!(f("B2ee", 46314.0), "1448");
    assert_eq!(f("B2d/m/yyyy;@", 46314.0), "8/5/1448");
}

/// Under `B2`, Excel shows Arabic weekday names, the full name for `ddd` as well as `dddd`.
#[test]
fn b2_prefix_names_weekdays_in_arabic() {
    // Monday 19 and Tuesday 20 October 2026.
    assert_eq!(f("B2dddd", 46314.0), "الإثنين");
    assert_eq!(f("B2ddd", 46314.0), "الإثنين");
    assert_eq!(f("B2dddd", 46315.0), "الثلاثاء");
    assert_eq!(f("B1dddd", 46314.0), "Monday");
    assert_eq!(f("B1ddd", 46315.0), "Tue");
    // Every weekday has a name.
    for v in 46314..46321 {
        assert!(!f("B2dddd", v as f64).is_empty());
    }
}

/// `B2` on a General or text section changes nothing: Excel shows the plain serial.
#[test]
fn b2_prefix_on_general_and_text_sections() {
    assert_eq!(f("B2General", 46314.0), "46314");
    assert_eq!(f("B2@", 46314.0), "46314");
}

#[test]
fn b2_prefix_names_hijri_months() {
    assert_eq!(f("B2d mmmm yyyy", JAN_1_2001), "6 شوال 1421");
    assert_eq!(f("B2mmm", 37034.0), "ربيع\u{a0}الأول");
    assert_eq!(f("B2mmmm", JAN_1_2001 + 23.0), "شوال");
    assert_eq!(f("B2mmmm", JAN_1_2001 + 24.0), "ذو\u{a0}القعدة");
    assert_eq!(f("B2mmmmm", JAN_1_2001 + 24.0), "ذ");
}

/// All twelve month names and seven weekday names as Excel 16.0 (build 20527, en-US) shows them,
/// read with TEXT and a cell format: two-word month names are joined by a no-break space.
#[test]
fn b2_names_match_excel() {
    let months = [
        (46219.0, "صفر"),
        (46248.0, "ربيع\u{a0}الأول"),
        (46278.0, "ربيع\u{a0}الثاني"),
        (46307.0, "جمادى\u{a0}الأولى"),
        (46337.0, "جمادى\u{a0}الثانية"),
        (46366.0, "رجب"),
        (46396.0, "شعبان"),
        (46425.0, "رمضان"),
        (46455.0, "شوال"),
        (46484.0, "ذو\u{a0}القعدة"),
        (46514.0, "ذو\u{a0}الحجة"),
        (46543.0, "محرم"),
    ];
    for (serial, name) in months {
        assert_eq!(f("B2dd", serial), "01", "{serial}");
        assert_eq!(f("B2mmmm", serial), name, "{serial}");
        assert_eq!(f("B2mmm", serial), name, "{serial}");
        assert_eq!(f("B2mmmmmm", serial), name, "{serial}");
    }
    // Sunday 18 to Saturday 24 October 2026.
    let days = ["الأحد", "الإثنين", "الثلاثاء", "الأربعاء", "الخميس", "الجمعة", "السبت"];
    for (k, name) in days.iter().enumerate() {
        let serial = 46313.0 + k as f64;
        assert_eq!(f("B2dddd", serial), *name, "{serial}");
        assert_eq!(f("B2ddd", serial), *name, "{serial}");
    }
    assert_eq!(f("B2yy", 46314.0), "48");
    assert_eq!(f("B2y", 46314.0), "48");
    assert_eq!(f("B2yyy", 46314.0), "1448");
    assert_eq!(f("B2yyyyy", 46314.0), "1448");
}

#[test]
fn b2_month_and_year_rollover() {
    assert_eq!(f("B2dd/mm/yyyy", 36915.0), "29/10/1421");
    assert_eq!(f("B2dd/mm/yyyy", 36916.0), "01/11/1421");
    assert_eq!(f("B2dd/mm/yyyy", 36974.0), "29/12/1421");
    assert_eq!(f("B2dd/mm/yyyy", 36975.0), "01/01/1422");
    assert_eq!(f("B2dd/mm/yyyy", 37683.0), "30/12/1423");
    assert_eq!(f("B2dd/mm/yyyy", 37684.0), "01/01/1424");
}

/// Cycle years 15 (1425, leap) and 16 (1426, not leap): Excel's leap year is 15, not 16. The
/// 2 March 2005 and 29 January 2006 values were read from Excel by a reviewer; the year
/// boundaries follow from them and the 354/355-day years. The same comparison covered every
/// serial from 36,000 to 49,000 with no disagreement.
#[test]
fn b2_leap_year_is_cycle_year_15() {
    assert_eq!(f("B2yyyy-mm-dd", 38413.0), "1426-01-21"); // 2005-03-02, from Excel
    assert_eq!(f("B2yyyy-mm-dd", 38746.0), "1426-12-29"); // 2006-01-29, from Excel
    assert_eq!(f("B2dd/mm/yyyy", 38037.0), "29/12/1424"); // 2004-02-20
    assert_eq!(f("B2dd/mm/yyyy", 38038.0), "01/01/1425"); // 2004-02-21
    assert_eq!(f("B2dd/mm/yyyy", 38392.0), "30/12/1425"); // 2005-02-09
    assert_eq!(f("B2dd/mm/yyyy", 38393.0), "01/01/1426"); // 2005-02-10
    assert_eq!(f("B2dd/mm/yyyy", 38747.0), "01/01/1427"); // 2006-01-30
    // Anchors the reviewer read from Excel.
    assert_eq!(f("B2dd/mm/yyyy", 46314.0), "08/05/1448"); // 2026-10-19
    assert_eq!(f("B2dd/mm/yyyy", 46313.0), "07/05/1448"); // 2026-10-18
    assert_eq!(f("B2dd/mm/yyyy", JAN_1_2001), "06/10/1421");
    assert_eq!(f("B2dd/mm/yyyy", 61.0), "29/10/1317"); // 1900-03-01
    assert_eq!(f("B2dd/mm/yyyy", 64970.0), "01/01/1501"); // 2077-11-16
    assert_eq!(f("B2dd/mm/yyyy", 64971.0), "02/01/1501"); // 2077-11-17
    assert_eq!(f("B2dd/mm/yyyy", 0.0), "29/08/1317");
    assert_eq!(f("B2dd/mm/yyyy", 2958465.0), "03/04/9666"); // 9999-12-31
}

/// Excel honours `B2` after a colour tag, but a `[$-...]` locale tag before it keeps the date
/// Gregorian (the prefix is still hidden); `B2[$-401]` is Hijri.
#[test]
fn calendar_prefix_and_bracket_tags() {
    assert_eq!(f("[Red]B2dd/mm/yyyy", 46314.0), "08/05/1448");
    assert_eq!(f("[<0]0;B2dd/mm/yyyy", JAN_1_2001), "06/10/1421");
    assert_eq!(f("B2[$-401]dd/mm/yyyy", 46314.0), "08/05/1448");
    for code in ["[$-401]B2dd/mm/yyyy", "[$-409]B2dd/mm/yyyy", "[$-1010401]B2dd/mm/yyyy", "[$-ar-SA]B2dd/mm/yyyy"] {
        assert_eq!(f(code, 46314.0), "19/10/2026", "{code}");
        assert_eq!(text_function(&Value::Number(46314.0), code, S).as_deref(), Ok("19/10/2026"), "{code}");
    }
    assert_eq!(f("[Red][$-401]B1dd/mm/yyyy", JAN_1_2001), "01/01/2001");
    // Checked in Excel 16.0 (build 20527, en-US): a currency tag cancels `B2` like a locale tag,
    // a tag after the prefix does not, and a repeated prefix is accepted.
    assert_eq!(f("[$€-407]B2dd/mm/yyyy", 46314.0), "€19/10/2026");
    assert_eq!(f("[$$-409]B2dd/mm/yyyy", 46314.0), "$19/10/2026");
    assert_eq!(f("[$-401][Red]B2dd/mm/yyyy", 46314.0), "19/10/2026");
    assert_eq!(f("B2[Red]dd/mm/yyyy", 46314.0), "08/05/1448");
    for code in ["B2B2dd/mm/yyyy", "B2B2B2dd/mm/yyyy", "B2b2dd/mm/yyyy", "[Red]B2B2dd/mm/yyyy"] {
        assert_eq!(f(code, 46314.0), "08/05/1448", "{code}");
    }
    assert_eq!(f("B1B1dd/mm/yyyy", 46314.0), "19/10/2026");
    assert_eq!(f("B2 dd/mm/yyyy", 46314.0), " 08/05/1448");
    assert_eq!(f("B2\"x\"dd", 46314.0), "x08");
    for code in ["B1\"x\"", "B2\"x\""] {
        assert_eq!(f(code, 46314.0), "x", "{code}");
    }
    for code in ["B1General", "B1@", "0;0;0;B2@", "0;B2dd/mm/yyyy"] {
        assert_eq!(f(code, 46314.0), "46314", "{code}");
    }
}

/// A quoted `B2` is literal text; any other `B` and digit makes Excel reject the code (`TEXT` is
/// `#VALUE!`), as does a prefix before a number format. A rejected code shows as General. All
/// checked in Excel 16.0 (build 20527, en-US), with TEXT and with a cell format.
#[test]
fn misplaced_calendar_prefix_is_rejected() {
    assert_eq!(f("\"B2\"dd/mm/yyyy", 46314.0), "B219/10/2026");
    assert_eq!(f("dd/\"B2\"/yyyy", 46314.0), "19/B2/2026");
    for code in [
        "B0dd/mm/yyyy",
        "B3dd/mm/yyyy",
        "B4dd/mm/yyyy",
        "B9dd/mm/yyyy",
        "B02dd",
        "B12dd",
        "B21dd",
        "B1B2dd/mm/yyyy",
        "B2B1dd/mm/yyyy",
        "B2[Red]B2dd/mm/yyyy",
        "B2 B2dd",
        "dd/mm/yyyyB2",
        "dd/mm/yyyyB1",
        "dd B2 mm",
        "dd b2 mm",
        "dd \\B2 yyyy",
        "B2\\B2dd",
        "B20.00",
        "B2#,##0",
        "B10.00",
        "B1#,##0",
        "0;B3dd",
    ] {
        assert_eq!(text_function(&Value::Number(46314.0), code, S), Err(CellError::Value), "{code}");
        assert_eq!(f(code, 46314.0), "46314", "{code}");
        assert!(NumberFormat::parse(code).is_general(), "{code}");
    }
}

#[test]
fn b2_edges_of_the_date_range() {
    // Serial 60 is the 1900 system's fictitious 29 February: Excel has no Hijri date for it.
    assert_eq!(text_function(&Value::Number(60.0), "B2dd/mm/yyyy", S), Err(CellError::Value));
    assert_eq!(f("B2dd/mm/yyyy", 60.0), "########");
    assert_eq!(f("B1dd/mm/yyyy", 60.0), "29/02/1900");
    assert_eq!(f("B2dd/mm/yyyy", 59.0), "28/10/1317");
    assert_eq!(f("B2dd/mm/yyyy", 61.0), "29/10/1317");
    // So does any clock part (Excel 16.0, build 20527, en-US), but not an elapsed `[h]`, a literal,
    // or a locale tag that cancels `B2`.
    for code in ["B2hh:mm", "B2hh", "B2ss", "B2am/pm", "B2hh:mm AM/PM", "B2[$-401]hh:mm", "B2dd/mm/yyyy;\"x\""] {
        assert_eq!(text_function(&Value::Number(60.0), code, S), Err(CellError::Value), "{code}");
        assert_eq!(text_function(&Value::Number(60.5), code, S), Err(CellError::Value), "{code}");
    }
    assert_eq!(f("B2[h]", 60.0), "1440");
    assert_eq!(f("B2\"x\"", 60.0), "x");
    assert_eq!(f("[$-401]B2hh:mm", 60.0), "00:00");
    assert_eq!(f("B1hh:mm", 60.0), "00:00");
    assert_eq!(f("B2hh:mm", 61.0), "00:00");
    // Serial 0 shows as serial 1.
    assert_eq!(f("B2dd/mm/yyyy", 0.0), "29/08/1317");
    assert_eq!(f("B2dd/mm/yyyy", 1.0), "29/08/1317");
    // No upper limit short of 31 December 9999: 16 and 17 November 2077, 31 December 9999.
    assert_eq!(f("B2dd/mm/yyyy", 64970.0), "01/01/1501");
    assert_eq!(f("B2dd/mm/yyyy", 64971.0), "02/01/1501");
    assert_eq!(f("B2dd/mm/yyyy", 2958465.0), "03/04/9666");
    // In the 1904 system, serial 60 is a real day.
    assert_ne!(format_value(&Value::Number(60.0), &NumberFormat::parse("B2dd/mm/yyyy"), DateSystem::D1904).text, "########");
}

#[test]
fn b2_out_of_range_does_not_panic() {
    for v in [-1.0, 0.0, 60.0, 60.5, 2958465.0, 2958466.0, 1e300, f64::NAN] {
        let _ = f("B2dd/mm/yyyy mmmm dddd", v);
        let _ = text_function(&Value::Number(v), "B2dd/mm/yyyy mmmm dddd", S);
    }
    assert_eq!(f("B2dd/mm/yyyy", -1.0), f("dd/mm/yyyy", -1.0));
    assert_eq!(f("B2dd/mm/yyyy", 2958466.0), f("dd/mm/yyyy", 2958466.0));
    // Before 1 Muharram 1 AH there is no Hijri date (no serial reaches it).
    let before_epoch = gridcraft_core::date::DateTime { year: 600, month: 3, day: 9, ..Default::default() };
    assert!(calendar::Calendar::Hijri.date(&before_epoch).is_none());
}

/// "Set precision as displayed": the value each format stores. Expected values observed in Excel
/// (COM, `Workbook.PrecisionAsDisplayed = True`) with synthetic values.
#[test]
fn round_as_displayed_matches_excel() {
    let r = |code: &str, n: f64| NumberFormat::parse(code).round_as_displayed(n);
    let cases: &[(&str, f64, f64)] = &[
        ("0", 527.91, 528.0),
        ("0", 2.5, 3.0),
        ("0", -2.5, -3.0),
        ("0.00", 1.005, 1.01),
        ("0.00", 1.23456, 1.23),
        ("#.##", 1.23456, 1.23),
        ("#.##", 1.2, 1.2),
        ("?.??", 1.23456, 1.23),
        ("0.0??", 1.23456, 1.235),
        ("0%", 0.123456, 0.12),
        ("0.0%", 0.123456, 0.123),
        ("0.00%", 0.123456, 0.1235),
        ("$#,##0.00", 1234.5678, 1234.57),
        ("#,##0", 1234.5678, 1235.0),
        ("0.0,,", 1234567.891, 1200000.0),
        ("#,##0,", 1234567.891, 1235000.0),
        ("0.0,", 1234567.891, 1234600.0),
        ("# ?/?", 0.3333333, 1.0 / 3.0),
        ("# ??/??", 0.3333333, 1.0 / 3.0),
        ("# ?/8", 0.3, 0.25),
        ("# ?/4", 2.71829, 2.75),
        ("?/?", 0.123, 0.125),
        ("0.00E+00", 123456.789, 123000.0),
        ("##0.0E+0", 123456.789, 120000.0),
        ("0.0E+0", 0.000123456, 0.00012),
        ("##0.E+0", 98765.4, 100000.0),
        ("###.00E+0", 98765.4, 98800.0),
        ("000.0E+0", 0.0123456, 0.012),
        ("0.00E+0\" u\"", 98765.4, 98800.0),
        ("0.0%E+0", 12345.0, 12345.0),
        ("0.00;(0.00)", -1.23456, -1.23),
        ("0.00;(0.0)", -1.23456, -1.2),
        ("0.0 \"kg\"", 1.23456, 1.2),
        ("0.00;-0.00;\"zero\"", 0.001, 0.0),
        ("0.00_);(0.00)", 1.23456, 1.23),
        ("#,##0.0_);[Red](#,##0.0)", 2.345, 2.3),
        // Kept at full precision.
        ("General", 1.0 / 3.0, 1.0 / 3.0),
        ("m/d/yyyy", 45000.75, 45000.75),
        ("h:mm", 45000.123456, 45000.123456),
        ("h:mm:ss", 45000.123456, 45000.123456),
        ("[h]:mm", 1.2345678, 1.2345678),
        ("mm:ss.0", 0.0001234567, 0.0001234567),
        ("d", 45000.6, 45000.6),
        ("@", 1.23456, 1.23456),
        ("\"x\"", 1.23456, 1.23456),
        (";;;", 1.23456, 1.23456),
        // Conditions are ignored: the sign picks the section (first for positives, second for
        // negatives), whatever section displays the number.
        ("[<1]0.00;0", 1.0 / 3.0, 0.33),
        ("[<1]0.00;0", 2.0 + 1.0 / 3.0, 2.33),
        ("[<1]0.00;0", 12345.678, 12345.68),
        ("[<1]0.00;0", -10.0 / 3.0, -3.0),
        ("[<1]0.00;0", -1.0 / 3.0, 0.0),
        ("[>1]0.0;0.000", 1.0 / 3.0, 0.3),
        ("[>1]0.0;0.000", -10.0 / 3.0, -3.333),
        ("[>=100]0;0.000", 2.0 + 1.0 / 3.0, 2.0),
        ("[>=100]0;0.000", -200.0 - 1.0 / 3.0, -200.333),
        ("[<0]0.0;0.000", -2.0 - 1.0 / 3.0, -2.333),
        ("[>100]0;[<-100]0.0;0.000", -2.0 - 1.0 / 3.0, -2.3),
        ("[<=-1]0.0;[>=1]0.000;0.00000", -2.0 - 1.0 / 3.0, -2.333),
        ("0.0;[<-5]0.000", -2.0 - 1.0 / 3.0, -2.333),
        ("[<1]0.00%;0", 2.0 + 1.0 / 3.0, 2.3333),
        ("[<1]0.00;0.0,", -2345.6789, -2300.0),
        ("[<1]0.0E+0;0", 1.0 / 3.0, 0.33),
        ("[<1]# ?/?;0", 2.3, 16.0 / 7.0),
        ("[<1]0.00;?/8", -2.0 - 1.0 / 3.0, -2.375),
        ("[<1]0.00;0.0;0.000;@", -2.0 - 1.0 / 3.0, -2.3),
        ("[<1]\"x\";0.0", 2.0 + 1.0 / 3.0, 2.0 + 1.0 / 3.0),
        ("[<1]\"x\";0.0", -2.0 - 1.0 / 3.0, -2.3),
        ("[<1]m/d/yyyy;0.0", 2.0 + 1.0 / 3.0, 2.0 + 1.0 / 3.0),
        ("[<1]0.0;m/d/yyyy", -2.0 - 1.0 / 3.0, -2.0 - 1.0 / 3.0),
        // One section with a condition: negatives keep the full value, unless an `@` section
        // follows (then the first section applies, as without a condition).
        ("[<1]0.00", 2.0 + 1.0 / 3.0, 2.33),
        ("[<1]0.00", -2.0 - 1.0 / 3.0, -2.0 - 1.0 / 3.0),
        ("[>=-100]0.0", -2.0 - 1.0 / 3.0, -2.0 - 1.0 / 3.0),
        ("[<1]0.00;@", -2.0 - 1.0 / 3.0, -2.33),
        ("[<1]0.0%;@", -2.0 - 1.0 / 3.0, -2.333),
        ("0.00;@", -2.0 - 1.0 / 3.0, -2.33),
        // A completely empty section passes to the next; a colour or a literal doesn't.
        ("0.0;;0.000", -2.0 - 1.0 / 3.0, -2.333),
        ("[<1]0.0;;0.000", -2.0 - 1.0 / 3.0, -2.333),
        ("[<1]0.0;;0.0%", -2.0 - 1.0 / 3.0, -2.333),
        (";0.0", 2.0 + 1.0 / 3.0, 2.3),
        (";;0.0", 2.0 + 1.0 / 3.0, 2.3),
        (";;0.0", -2.0 - 1.0 / 3.0, -2.3),
        ("0.0;;", -2.0 - 1.0 / 3.0, -2.0 - 1.0 / 3.0),
        ("0.0;;@", -2.0 - 1.0 / 3.0, -2.0 - 1.0 / 3.0),
        ("[<1]0.00;", -2.0 - 1.0 / 3.0, -2.0 - 1.0 / 3.0),
        ("0.0;[Red];0.000", -2.0 - 1.0 / 3.0, -2.0 - 1.0 / 3.0),
        ("[<1]0.0;\" \";0.000", -2.0 - 1.0 / 3.0, -2.0 - 1.0 / 3.0),
        ("[<1];0.0;0.00", 2.0 + 1.0 / 3.0, 2.0 + 1.0 / 3.0),
    ];
    for &(code, n, want) in cases {
        assert_eq!(r(code, n), want, "{code} with {n}");
    }
    // A negative that rounds to zero stores 0, not -0.
    let z = r("0.00", -0.004);
    assert!(z == 0.0 && z.is_sign_positive());
    // Non-finite and extreme inputs come back unchanged or finite, never a panic.
    assert!(r("0.00", f64::NAN).is_nan());
    assert_eq!(r("0.00", f64::INFINITY), f64::INFINITY);
    assert_eq!(r("0", 1e300 * 10.0), 1e301);
    assert_eq!(r("0.0,,,,,,,,,,", 1.234e35), 1.234e35);
    assert_eq!(r("0.000000000000000000000000000000", 1.0 / 3.0), 0.333333333333333);
    assert_eq!(r("0%%%%%%%%%%%%%%%%%%%%%%%%", f64::MAX), f64::MAX);
    assert_eq!(r("0", f64::MAX), f64::MAX);
    assert_eq!(r("0.0E+0", f64::MIN_POSITIVE), 2.2e-308);
}
