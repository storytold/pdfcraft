//! Equality tests for the render/General cleanup: output over a large grid of format codes x
//! values (format_value and text_function in both date systems, then General at widths 1..=30 and a few larger ones)
//! is compared with golden digests recorded from the implementation BEFORE the cleanup
//! (`tests/golden/*.txt`, one FNV-1a digest per format code / per value, so the files stay
//! small). A failure names the code or value that differs; set `NUMFMT_DUMP_DIR` to write the
//! full text of the current output for diffing against a dump from the old code. To re-record
//! after an intended behaviour change run with `NUMFMT_BLESS=1`.

#![allow(clippy::approx_constant, clippy::excessive_precision)] // values are test data, not constants

use std::fmt::Write as _;

use gridcraft_core::{DateSystem, Value};
use gridcraft_numfmt::{NumberFormat, format_general_fit, format_value, text_function};

// Placeholders far beyond the 15 significant digits.
const LONG_DECIMALS: &str = "0.0000000000000000000000000";
#[rustfmt::skip]
const CODES: &[&str] = &[
    LONG_DECIMALS, "#,###,###,###,###,###,###,###,##0.00", "0.0000000000000000000000000E+00", "# ?????/?????",
    // numbers
    "General", "0", "0.00", "#,##0", "#,##0.00", "#.##", "0.0##", "00000", "000.000", "#,###", "#,##0,", "#,##0,,", "0.0,,\"M\"", "?.??", "##0.0", "# ", "#", ".00",
    ",0", "0,0", "0.00,", "\"$\"#,##0.00", "\"$\"#,##0_);\\(\"$\"#,##0\\)", "#,##0.00;[Red](#,##0.00)", "#,##0 ;(#,##0)", "0;-0;;@", "0.00;;", ";;;",
    "0_);(0)", "0\" units\"", "\\A0", "(0)", "00-00", "000-00-0000", "[<=9999999]###-####;(###) ###-####", "[$€-407]#,##0.00",
    // currency/accounting with fill
    "_(* #,##0_);_(* \\(#,##0\\);_(* \"-\"_);_(@_)", "_(\"$\"* #,##0.00_);_(\"$\"* \\(#,##0.00\\);_(\"$\"* \"-\"??_);_(@_)", "*-0", "0*x", "*0 0.0", "@*_",
    // percent
    "0%", "0.00%", "#,##0%", "0.0%%", "%0", "0%;[Red]-0%",
    // scientific
    "0.00E+00", "##0.0E+0", "0E+0", "0.0e-0", "0.00E+000", "E+0", "0.00E+", "#.##E+00", "0.0E+00 \"x\"",
    // fractions
    "# ?/?", "# ??/??", "# ???/???", "?/?", "# ?/8", "# ??/16", "0/0", "# #/#", "#/#", "?/100", "# ?/?;-# ?/?", "0 0/0",
    // dates/times
    "m/d/yyyy", "d-mmm-yy", "d-mmm", "mmm-yy", "h:mm AM/PM", "h:mm:ss AM/PM", "h:mm", "h:mm:ss", "m/d/yyyy h:mm", "mm:ss", "[h]:mm:ss", "[h]:mm", "[m]:ss", "[s]", "mmss.0",
    "yyyy-mm-dd", "dddd, mmmm d, yyyy", "ddd mmm dd yy", "mmmmm", "yy", "h:mm:ss.000", "h:mm:ss.0000", "hh:mm:ss A/P", "[hh]:mm:ss", "d", "dd", "dddd", "m", "mm", "s.00",
    "yyyy-mm-dd hh:mm:ss.00", "h AM/PM", "[$-409]d-mmm-yy", "yyyy\"年\"m\"月\"", "[h]\" hrs\"", "mmmm yyyy;@",
    // text sections, conditions, colours
    "@", "\"x\"@", "0;\"neg\"0;\"zero\";\"txt: \"@", "General;General;General;@", "[Red]0.0;[Blue]-0.0", "[Green]0;[Magenta]0;[Cyan]0;[Yellow]@", "[Color3]0.00",
    "[>100]\"big\"0;[<-100]\"small\"0;0.0", "[=0]\"zero\";0.0", "[>=1000]#,##0,\"K\";0", "[<1]0.00%;0.0", "[Red][>5]0;0.00", "General\" kg\"", "\"v\"General",
    "[Red]General", "0.0;[Red]General",
    // invalid / odd
    "[", "0.0.0", "0\"", "\\", "[h", "[$", "yyyy-mm-dd;", ";", "0;0;0;0;0", "\"abc", "[>x]0", "ggg", "#?#0", "0E", "E0", "/", "0/", "/0", "*", "_", "_0", "[Red", "0.00.00%", "mm:ss.0.0",
    "[ABC]0", "[=1]0;[=2]0;0", "0 ??/??;0",
];

fn values() -> Vec<f64> {
    let mut v = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        0.25,
        1.005,
        2.675,
        999.9995,
        0.9995,
        9.995,
        99.5,
        0.05,
        0.005,
        0.0005,
        1e-5,
        1e-10,
        1e-300,
        5e-324,
        123456789.0,
        1234.5678,
        12345678901.0,
        1e11,
        1e15,
        123456789012345.0,
        1234567890123455.0,
        1e20,
        1e100,
        1.7976931348623157e308,
        -1234.5,
        -0.004,
        -1e-10,
        1.0 / 3.0,
        2.0 / 3.0,
        0.1 + 0.2,
        100.0,
        1000.0,
        59.0,
        60.0,
        61.0,
        36526.0,
        36526.75,
        43831.5,
        45000.123456,
        2958465.0,
        2958465.999999,
        2958466.0,
        -5.0,
        0.99999999,
        0.000999,
        41.99999999999,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.041666666666666664,
        0.9999999999,
        1.0000000001,
        12.3456789,
        3.14159265358979,
        65535.0,
        1e5,
        123456.0,
        4.5,
        0.375,
        1e300,
        1.5e-8,
        999999999999999.0,
        99999999999999.99,
        0.00001234,
        123456789012345678.0,
        1e22,
        9.999999999999999e22,
        0.1234567890123456,
    ];
    v.push(61.2 / 86400.0);
    v
}

fn fmt_formatted(f: &gridcraft_numfmt::Formatted) -> String {
    format!("{:?}|{:?}|{:?}|{}", f.text, f.color, f.fill, f.numeric)
}

fn dump_codes() -> Vec<(String, String)> {
    assert!(CODES.len() >= 80 && values().len() >= 40);
    let mut rows = Vec::new();
    let others = [Value::Text("abc".into()), Value::Text("12.5".into()), Value::Bool(true), Value::Empty];
    for code in CODES {
        let mut out = String::new();
        let nf = NumberFormat::parse(code);
        let systems: &[(DateSystem, &str)] =
            if nf.is_date() { &[(DateSystem::D1900, "1900"), (DateSystem::D1904, "1904")] } else { &[(DateSystem::D1900, "1900")] };
        for (sys, name) in systems {
            for n in values() {
                let v = Value::Number(n);
                let _ =
                    writeln!(out, "{code:?} {name} {n:e} => {} || {:?}", fmt_formatted(&format_value(&v, &nf, *sys)), text_function(&v, code, *sys));
            }
        }
        for v in &others {
            let _ = writeln!(
                out,
                "{code:?} {v:?} => {} || {:?}",
                fmt_formatted(&format_value(v, &nf, DateSystem::D1900)),
                text_function(v, code, DateSystem::D1900)
            );
        }
        rows.push((format!("{code:?}"), out));
    }
    rows
}

fn dump_general() -> Vec<(String, String)> {
    let mut vals = values();
    let mut x = 0.7f64;
    // Deterministic spread over many magnitudes and digit patterns (no RNG dependency).
    for i in 0..240 {
        x = (x * 997.0 + 0.123_456_789).fract() + 1e-3;
        let e = (i % 41) - 20;
        let m = x * 10f64.powi(e);
        vals.push(if i % 3 == 0 { -m } else { m });
        vals.push((m * 1000.0).round() / 1000.0);
    }
    assert!(vals.len() >= 300);
    let mut rows = Vec::new();
    for n in vals {
        let mut out = String::new();
        let _ = write!(out, "{n:e} =>");
        for w in (1..=30usize).chain([31, 40, 100, 399, 400, 401, 500, 1000]) {
            let _ = write!(out, " {:?}", format_general_fit(n, w));
        }
        out.push('\n');
        rows.push((format!("{n:e}"), out));
    }
    rows
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

fn check(name: &str, rows: &[(String, String)], golden: &str, path: &str) {
    if let Some(dir) = std::env::var_os("NUMFMT_DUMP_DIR") {
        let all: String = rows.iter().map(|(_, b)| b.as_str()).collect();
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.txt")), all).expect("write dump");
    }
    let actual: String = rows.iter().map(|(k, b)| format!("{:016x} {} {k}\n", fnv1a(b), b.lines().count())).collect();
    if std::env::var_os("NUMFMT_BLESS").is_some() {
        std::fs::write(path, &actual).expect("write golden");
        return;
    }
    let bad: Vec<String> = rows
        .iter()
        .zip(actual.lines().zip(golden.lines()))
        .filter(|(_, (a, g))| a != g)
        .take(3)
        .map(|((_, body), (a, g))| {
            format!("  new: {a}\n  old: {g}\n  new output of that row (first 12 lines):\n{}", body.lines().take(12).collect::<Vec<_>>().join("\n"))
        })
        .collect();
    assert!(
        actual.lines().count() == golden.lines().count() && bad.is_empty(),
        "{name}: output differs from the recorded golden ({} vs {} rows). If the change is intended, re-record with\n  NUMFMT_BLESS=1 cargo test -p gridcraft-numfmt --test render_equality\n{}",
        actual.lines().count(),
        golden.lines().count(),
        bad.join("\n")
    );
}

#[test]
fn format_value_and_text_function_match_golden() {
    check("codes", &dump_codes(), include_str!("golden/codes.txt"), concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/codes.txt"));
}

#[test]
fn general_fit_matches_golden() {
    check("general", &dump_general(), include_str!("golden/general.txt"), concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/general.txt"));
}
