use super::*;
use crate::util::testutil::*;
use CellError::{Div0, NA, Num};

fn ev1(name: &str, x: f64) -> Value {
    ev(name, vec![n(x)])
}
fn ev2(name: &str, x: f64, y: f64) -> Value {
    ev(name, vec![n(x), n(y)])
}

#[test]
fn percentof() {
    // SUM(subset) / SUM(all): text, booleans and blanks inside ranges are skipped like SUM.
    let all = arr(vec![vec![nv(10.0)], vec![nv(30.0)], vec![tv("x")], vec![Value::Empty], vec![nv(60.0)]]);
    let part = arr(vec![vec![nv(10.0)], vec![nv(30.0)]]);
    close(ev("PERCENTOF", vec![rf(part), rf(all.clone())]), 0.4);
    close(ev("PERCENTOF", vec![n(25.0), n(200.0)]), 0.125);
    is_err(ev("PERCENTOF", vec![n(1.0), rf(arr(vec![vec![nv(5.0), nv(-5.0)]]))]), Div0);
    is_err(ev("PERCENTOF", vec![rf(arr(vec![vec![Value::Error(NA)]])), rf(all)]), NA);
    is_err(ev("PERCENTOF", vec![t("abc"), n(1.0)]), CellError::Value);
}

#[test]
fn sums() {
    close(ev("SUM", vec![n(1.0), n(2.0), t("3"), b(true)]), 7.0);
    let r = arr(vec![vec![nv(1.0), tv("x"), Value::Bool(true), Value::Empty, nv(4.0)]]);
    close(ev("SUM", vec![rf(r.clone())]), 5.0);
    is_err(ev("SUM", vec![t("abc")]), CellError::Value);
    is_err(ev("SUM", vec![rf(arr(vec![vec![nv(1.0), Value::Error(Div0)]]))]), Div0);
    close(ev("SUM", vec![rf(tv("abc"))]), 0.0);
    close(ev("SUMSQ", vec![n(3.0), n(4.0)]), 25.0);
    close(ev("PRODUCT", vec![rf(r), n(2.0)]), 8.0);
    close(ev("PRODUCT", vec![rf(arr(vec![vec![tv("a")]]))]), 0.0);
    let a1 = arr(vec![vec![nv(3.0), nv(4.0)], vec![nv(8.0), nv(6.0)], vec![nv(1.0), nv(9.0)]]);
    let a2 = arr(vec![vec![nv(2.0), nv(7.0)], vec![nv(6.0), nv(7.0)], vec![nv(5.0), nv(3.0)]]);
    close(ev("SUMPRODUCT", vec![rf(a1.clone()), rf(a2)]), 156.0);
    close(ev("SUMPRODUCT", vec![rf(a1.clone())]), 31.0);
    is_err(ev("SUMPRODUCT", vec![rf(a1), rf(row(&[1.0, 2.0]))]), CellError::Value);
    close(ev("SUMPRODUCT", vec![av(row(&[1.0, 2.0])), av(arr(vec![vec![tv("x"), nv(3.0)]]))]), 6.0);
    let x = row(&[2.0, 3.0, 9.0, 1.0, 8.0, 7.0, 5.0]);
    let y = row(&[6.0, 5.0, 11.0, 7.0, 5.0, 4.0, 4.0]);
    close(ev("SUMX2MY2", vec![av(x.clone()), av(y.clone())]), -55.0);
    close(ev("SUMX2PY2", vec![av(x.clone()), av(y.clone())]), 521.0);
    close(ev("SUMXMY2", vec![av(x.clone()), av(y)]), 79.0);
    is_err(ev("SUMXMY2", vec![av(x), av(row(&[1.0]))]), NA);
}

/// SUMPRODUCT cases observed in Excel (FormulaEvalTestData, row "Sumproduct"): scalars and
/// single-cell references multiply in, text in a scalar argument is `#VALUE!`, sizes must match.
#[test]
fn sumproduct_scalar_arguments() {
    let cases: Vec<(Vec<crate::Arg>, Result<f64, CellError>)> = vec![
        (vec![n(3.0), rf(nv(534.0)), rf(nv(1.1))], Ok(1762.2)),
        (vec![rf(nv(1.1)), n(3.0), rf(nv(534.0)), rf(nv(1.1))], Ok(1938.42)),
        (vec![n(2.0), rf(col(&[1.00001, 2.99999]))], Err(CellError::Value)),
        (vec![rf(tv("4")), rf(nv(2.0))], Err(CellError::Value)),
        (vec![t("4"), n(2.0)], Err(CellError::Value)),
        (vec![rf(nv(3.0)), rf(Value::Error(CellError::Ref))], Err(CellError::Ref)),
        (vec![av(arr(vec![vec![tv("4")]])), n(2.0)], Ok(0.0)),
    ];
    for (args, want) in cases {
        match want {
            Ok(x) => close(ev("SUMPRODUCT", args), x),
            Err(err) => is_err(ev("SUMPRODUCT", args), err),
        }
    }
}

#[test]
fn rounding() {
    close(ev2("ROUND", 2.675, 2.0), 2.68);
    close(ev2("ROUND", -1.5, 0.0), -2.0);
    close(ev2("ROUND", 1234.567, -2.0), 1200.0);
    close(ev2("ROUND", 2.15, 1.0), 2.2);
    close(ev2("ROUNDUP", 3.2, 0.0), 4.0);
    close(ev2("ROUNDUP", -3.14259, 1.0), -3.2);
    close(ev2("ROUNDUP", 31415.92654, -2.0), 31500.0);
    close(ev2("ROUNDDOWN", -3.9, 0.0), -3.0);
    close(ev2("ROUNDDOWN", 2.71989, 3.0), 2.719);
    close(ev2("ROUNDDOWN", 31415.92654, -2.0), 31400.0);
    for digits in [-308.0, -309.0, -400.0] {
        for x in [1234.0, -1234.0, 0.5] {
            close(ev2("ROUNDDOWN", x, digits), 0.0);
            close(ev2("TRUNC", x, digits), 0.0);
        }
    }
    assert!(matches!(ev2("ROUNDUP", 1234.0, -309.0), Value::Error(CellError::Num)));
    close(ev2("ROUNDUP", 0.1 + 0.2, 1.0), 0.3);
    close(ev2("TRUNC", 8.987, 2.0), 8.98);
    close(ev1("TRUNC", -8.9), -8.0);
    close(ev1("INT", -8.9), -9.0);
    close(ev1("INT", 8.9), 8.0);
    close(ev2("MROUND", 10.0, 3.0), 9.0);
    close(ev2("MROUND", -10.0, -3.0), -9.0);
    close(ev2("MROUND", 1.3, 0.2), 1.4);
    is_err(ev2("MROUND", 5.0, -2.0), Num);
    close(ev2("CEILING", 2.5, 1.0), 3.0);
    close(ev2("CEILING", -2.5, -2.0), -4.0);
    close(ev2("CEILING", -2.5, 2.0), -2.0);
    close(ev2("CEILING", 1.5, 0.1), 1.5);
    close(ev2("CEILING", 0.234, 0.01), 0.24);
    is_err(ev2("CEILING", 2.5, -1.0), Num);
    // ECMA.CEILING is CEILING under a second name (Excel table from #2776).
    close(ev2("ECMA.CEILING", 1.5, 1.0), 2.0);
    close(ev2("ECMA.CEILING", -2.5, -2.0), -4.0);
    close(ev2("ECMA.CEILING", 2.5, 0.0), 0.0);
    is_err(ev2("ECMA.CEILING", 1.5, -1.0), Num);
    close(ev2("FLOOR", 3.7, 2.0), 2.0);
    close(ev2("FLOOR", -2.5, -2.0), -2.0);
    close(ev2("FLOOR", -2.5, 2.0), -4.0);
    is_err(ev2("FLOOR", 2.5, -2.0), Num);
    is_err(ev2("FLOOR", 2.5, 0.0), Div0);
    close(ev2("FLOOR", 1.58, 0.1), 1.5);
    close(ev2("FLOOR", 0.234, 0.01), 0.23);
    close(ev2("CEILING.MATH", 24.3, 5.0), 25.0);
    close(ev1("CEILING.MATH", 6.7), 7.0);
    close(ev2("CEILING.MATH", -8.1, 2.0), -8.0);
    close(ev("CEILING.MATH", vec![n(-5.5), n(2.0), n(-1.0)]), -6.0);
    close(ev2("FLOOR.MATH", 24.3, 5.0), 20.0);
    close(ev1("FLOOR.MATH", 6.7), 6.0);
    close(ev2("FLOOR.MATH", -8.1, 2.0), -10.0);
    close(ev("FLOOR.MATH", vec![n(-5.5), n(2.0), n(-1.0)]), -4.0);
    close(ev2("CEILING.PRECISE", -4.6, 2.0), -4.0);
    close(ev2("CEILING.PRECISE", 4.3, -2.0), 6.0);
    close(ev2("ISO.CEILING", 4.3, 0.0), 0.0);
    close(ev2("FLOOR.PRECISE", -3.2, -1.0), -4.0);
    close(ev1("FLOOR.PRECISE", 3.2), 3.0);
    close(ev1("EVEN", 1.5), 2.0);
    close(ev1("EVEN", 3.0), 4.0);
    close(ev1("EVEN", 2.0), 2.0);
    close(ev1("EVEN", -1.0), -2.0);
    close(ev1("EVEN", 0.0), 0.0);
    close(ev1("ODD", 1.5), 3.0);
    close(ev1("ODD", 3.0), 3.0);
    close(ev1("ODD", 2.0), 3.0);
    close(ev1("ODD", -1.0), -1.0);
    close(ev1("ODD", -2.0), -3.0);
    close(ev1("ODD", 0.0), 1.0);
}

#[test]
fn mod_quotient_limit() {
    // Excel 16: #NUM! whenever |number / divisor| >= 1.1259E12.
    for (x, d) in [
        (3.0, 1e-300),
        (-1.0, 1e-300),
        (3.0, -1e-300),
        (-1.0, -1e-300),
        (1e15, 7.0),
        (1e10, 0.001),
        (1_125_900_000_000.0, 1.0),
        (-1_125_900_000_000.0, 1.0),
    ] {
        is_err(ev2("MOD", x, d), Num);
    }
    close(ev2("MOD", 1e12 * 7.0 + 3.0, 7.0), 3.0);
    close(ev2("MOD", 1_125_899_999_999.99, 1.0), 0.989_990_234_375);
}

#[test]
fn arithmetic() {
    close(ev2("MOD", 3.0, 2.0), 1.0);
    close(ev2("MOD", -3.0, 2.0), 1.0);
    close(ev2("MOD", 3.0, -2.0), -1.0);
    close(ev2("MOD", -3.0, -2.0), -1.0);
    close(ev2("MOD", 5.5, 1.0), 0.5);
    is_err(ev2("MOD", 1.0, 0.0), Div0);
    close(ev2("QUOTIENT", 5.0, 2.0), 2.0);
    close(ev2("QUOTIENT", -10.0, 3.0), -3.0);
    is_err(ev2("QUOTIENT", 1.0, 0.0), Div0);
    close(ev1("ABS", -4.0), 4.0);
    close(ev1("SIGN", -0.0001), -1.0);
    close(ev1("SIGN", 0.0), 0.0);
    close(ev1("SQRT", 16.0), 4.0);
    is_err(ev1("SQRT", -16.0), Num);
    close(ev1("SQRTPI", 1.0), 1.772_453_850_905_516);
    close(ev2("POWER", 5.0, 2.0), 25.0);
    close(ev2("POWER", 4.0, 1.25), 5.656_854_249_492_381);
    close(ev2("POWER", -2.0, 3.0), -8.0);
    is_err(ev2("POWER", 0.0, 0.0), Num);
    is_err(ev2("POWER", 0.0, -1.0), Div0);
    is_err(ev2("POWER", -8.0, 1.0 / 3.0), Num);
    is_err(ev2("POWER", 10.0, 400.0), Num);
    close(ev1("EXP", 1.0), std::f64::consts::E);
    close(ev2("LOG", 8.0, 2.0), 3.0);
    close(ev1("LOG", 10.0), 1.0);
    close(ev2("LOG", 81.0, 3.0), 4.0);
    is_err(ev1("LN", 0.0), Num);
    is_err(ev2("LOG", 10.0, 1.0), Div0);
    is_err(ev1("LOG10", -1.0), Num);
    close(ev1("LN", std::f64::consts::E), 1.0);
    close(ev1("LOG10", 1e5), 5.0);
    close(ev("PI", vec![]), PI);
    close(ev("SQRT", vec![t("16")]), 4.0);
    is_err(ev("SQRT", vec![t("x")]), CellError::Value);
}

#[test]
fn combinatorics() {
    close(ev1("FACT", 5.0), 120.0);
    close(ev1("FACT", 1.9), 1.0);
    close(ev1("FACT", 0.0), 1.0);
    is_err(ev1("FACT", -1.0), Num);
    is_err(ev1("FACT", 171.0), Num);
    close(ev1("FACTDOUBLE", 6.0), 48.0);
    close(ev1("FACTDOUBLE", 7.0), 105.0);
    close(ev1("FACTDOUBLE", -1.0), 1.0);
    is_err(ev1("FACTDOUBLE", -2.0), Num);
    close(ev2("COMBIN", 8.0, 2.0), 28.0);
    close(ev2("COMBIN", 8.9, 2.9), 28.0);
    is_err(ev2("COMBIN", 2.0, 3.0), Num);
    close(ev2("COMBIN", 60.0, 30.0), 118_264_581_564_861_424.0);
    close(ev2("COMBINA", 4.0, 3.0), 20.0);
    close(ev2("COMBINA", 10.0, 3.0), 220.0);
    close(ev2("PERMUT", 100.0, 3.0), 970_200.0);
    close(ev2("PERMUT", 3.0, 2.0), 6.0);
    is_err(ev2("PERMUT", 2.0, 3.0), Num);
    close(ev2("PERMUTATIONA", 3.0, 2.0), 9.0);
    close(ev2("GCD", 24.0, 36.0), 12.0);
    close(ev2("GCD", 5.0, 0.0), 5.0);
    close(ev2("GCD", 7.0, 1.0), 1.0);
    is_err(ev2("GCD", -1.0, 5.0), Num);
    close(ev("GCD", vec![av(row(&[12.0, 18.0, 30.0]))]), 6.0);
    close(ev2("LCM", 24.0, 36.0), 72.0);
    close(ev2("LCM", 5.0, 2.0), 10.0);
    close(ev2("LCM", 5.0, 0.0), 0.0);
    close(ev("MULTINOMIAL", vec![n(2.0), n(3.0), n(4.0)]), 1260.0);
    is_err(ev("MULTINOMIAL", vec![n(-1.0), n(2.0)]), Num);
}

#[test]
fn gcd_and_lcm_reject_negative_fractions_before_truncation() {
    for name in ["GCD", "LCM"] {
        for x in [-0.1, -0.5, -0.999] {
            is_err(ev2(name, x, 2.0), Num);
            is_err(ev(name, vec![rf(row(&[2.0, x]))]), Num);
            // A zero elsewhere must not hide an invalid negative argument.
            is_err(ev2(name, 0.0, x), Num);
        }
        close(ev2(name, -0.0, 2.0), if name == "GCD" { 2.0 } else { 0.0 });
        close(ev2(name, 12.9, 18.9), if name == "GCD" { 6.0 } else { 36.0 });
    }
}

#[test]
fn trig() {
    close(ev1("SIN", PI / 2.0), 1.0);
    close(ev1("COS", 0.0), 1.0);
    close(ev1("TAN", PI / 4.0), 1.0);
    is_err(ev1("SIN", 1e10), Num);
    close(ev1("ASIN", 1.0), PI / 2.0);
    is_err(ev1("ACOS", 1.5), Num);
    close(ev1("ACOT", 2.0), 0.463_647_609_000_806);
    close(ev1("ACOTH", 6.0), 0.168_236_118_310_606);
    is_err(ev1("ACOTH", 0.5), Num);
    close(ev1("COT", 30.0), -0.156_119_952_161_659);
    is_err(ev1("COT", 0.0), Div0);
    close(ev1("COTH", 2.0), 1.037_314_720_727_548);
    close(ev1("CSC", 15.0), 1.537_780_562_208_209);
    close(ev1("CSCH", 1.5), 0.469_642_440_595_653);
    close(ev1("SEC", 45.0), 1.903_594_407_014_6);
    close_tol(ev1("SECH", 45.0), 5.725_037_161_098_787e-20, 1e-9);
    close(ev2("ATAN2", 1.0, 1.0), PI / 4.0);
    close(ev2("ATAN2", -1.0, -1.0), -3.0 * PI / 4.0);
    is_err(ev2("ATAN2", 0.0, 0.0), Div0);
    close(ev1("ASINH", -2.5), -1.647_231_146_371_096);
    close(ev1("ACOSH", 10.0), 2.993_222_846_126_381);
    is_err(ev1("ACOSH", 0.5), Num);
    close(ev1("ATANH", 0.5), 0.549_306_144_334_055);
    is_err(ev1("ATANH", 1.0), Num);
    close(ev1("SINH", 1.0), 1.175_201_193_643_801);
    close(ev1("DEGREES", PI), 180.0);
    close(ev1("RADIANS", 270.0), 4.712_388_980_384_69);
}

#[test]
fn number_systems() {
    is_text(ev1("ROMAN", 499.0), "CDXCIX");
    is_text(ev2("ROMAN", 499.0, 1.0), "LDVLIV");
    is_text(ev2("ROMAN", 499.0, 2.0), "XDIX");
    is_text(ev2("ROMAN", 499.0, 3.0), "VDIV");
    is_text(ev2("ROMAN", 499.0, 4.0), "ID");
    is_text(ev("ROMAN", vec![n(499.0), b(false)]), "ID");
    is_text(ev1("ROMAN", 1999.0), "MCMXCIX");
    is_text(ev1("ROMAN", 3999.0), "MMMCMXCIX");
    is_text(ev1("ROMAN", 0.0), "");
    is_err(ev1("ROMAN", 4000.0), CellError::Value);
    is_err(ev1("ROMAN", -1.0), CellError::Value);
    close(ev("ARABIC", vec![t("LVII")]), 57.0);
    close(ev("ARABIC", vec![t("mcmxii")]), 1912.0);
    close(ev("ARABIC", vec![t("")]), 0.0);
    close(ev("ARABIC", vec![t("-XIV")]), -14.0);
    is_err(ev("ARABIC", vec![t("ABC")]), CellError::Value);
    is_text(ev2("BASE", 7.0, 2.0), "111");
    is_text(ev2("BASE", 100.0, 16.0), "64");
    is_text(ev("BASE", vec![n(15.0), n(2.0), n(10.0)]), "0000001111");
    is_text(ev2("BASE", 0.0, 36.0), "0");
    is_err(ev2("BASE", 7.0, 1.0), Num);
    is_err(ev2("BASE", -7.0, 2.0), Num);
    close(ev("DECIMAL", vec![t("FF"), n(16.0)]), 255.0);
    close(ev("DECIMAL", vec![t("111"), n(2.0)]), 7.0);
    close(ev("DECIMAL", vec![t("zap"), n(36.0)]), 45745.0);
    is_err(ev("DECIMAL", vec![t("12"), n(2.0)]), Num);
}

/// Every number where Excel's forms 2 and 3 differ from a plain ratio limit (issue #733):
/// form 2 subtracts I from C and X from M, form 3 also V from M.
#[test]
fn roman_forms_two_and_three_match_excel() {
    const HUNDREDS: [&str; 10] = ["", "C", "CC", "CCC", "CD", "D", "DC", "DCC", "DCCC", "CM"];
    const ONES: [&str; 10] = ["", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX"];
    let mut cases = Vec::new();
    for thousands in 0..4usize {
        let m = "M".repeat(thousands);
        for (h, hundreds) in HUNDREDS.iter().enumerate() {
            if h != 4 && h != 9 {
                cases.push((thousands * 1000 + h * 100 + 99, 2.0, format!("{m}{hundreds}IC")));
            }
        }
        for (o, ones) in ONES.iter().enumerate() {
            cases.push((thousands * 1000 + 990 + o, 2.0, format!("{m}XM{ones}")));
        }
        for (o, ones) in ONES.iter().enumerate().take(5) {
            cases.push((thousands * 1000 + 995 + o, 3.0, format!("{m}VM{ones}")));
        }
    }
    assert_eq!(cases.len(), 92);
    for (number, form, want) in &cases {
        is_text(ev2("ROMAN", *number as f64, *form), want);
    }
    // Neighbours that already matched: form 2 still has no VD or ID, form 3 no ID.
    is_text(ev2("ROMAN", 495.0, 2.0), "XDV");
    is_text(ev2("ROMAN", 495.0, 3.0), "VD");
    is_text(ev2("ROMAN", 1499.0, 2.0), "MXDIX");
    is_text(ev2("ROMAN", 999.0, 0.0), "CMXCIX");
    is_text(ev2("ROMAN", 999.0, 1.0), "LMVLIV");
    is_text(ev2("ROMAN", 999.0, 4.0), "IM");
    is_text(ev2("ROMAN", 995.0, 4.0), "VM");
    is_text(ev2("ROMAN", 99.0, 1.0), "VCIV");
    is_text(ev2("ROMAN", 99.0, 4.0), "IC");
}

#[test]
fn minverse_scale_relative() {
    let big = ev("MINVERSE", vec![av(arr(vec![vec![nv(1e20), nv(0.0)], vec![nv(0.0), nv(1e20)]]))]);
    let r = rows_of(&big);
    close_tol(r[0][0].clone(), 1e-20, 1e-25);
    close_tol(r[1][1].clone(), 1e-20, 1e-25);
    let small = ev("MINVERSE", vec![av(arr(vec![vec![nv(1e-20), nv(0.0)], vec![nv(0.0), nv(1e-20)]]))]);
    let r = rows_of(&small);
    close_tol(r[0][0].clone(), 1e20, 1e10);
}

#[test]
fn matrices() {
    let a = arr(vec![vec![nv(1.0), nv(2.0)], vec![nv(3.0), nv(4.0)]]);
    let b2 = arr(vec![vec![nv(5.0), nv(6.0)], vec![nv(7.0), nv(8.0)]]);
    assert_eq!(rows_of(&ev("MMULT", vec![av(a.clone()), av(b2)])), vec![vec![nv(19.0), nv(22.0)], vec![nv(43.0), nv(50.0)]]);
    is_err(ev("MMULT", vec![av(a.clone()), av(row(&[1.0, 2.0]))]), CellError::Value);
    is_err(ev("MMULT", vec![av(arr(vec![vec![tv("x")]])), av(row(&[1.0]))]), CellError::Value);
    close(ev("MDETERM", vec![av(a.clone())]), -2.0);
    let m3 = arr(vec![vec![nv(3.0), nv(6.0), nv(1.0)], vec![nv(1.0), nv(1.0), nv(0.0)], vec![nv(3.0), nv(10.0), nv(2.0)]]);
    close(ev("MDETERM", vec![av(m3)]), 1.0);
    let m4 = arr(vec![
        vec![nv(1.0), nv(3.0), nv(8.0), nv(5.0)],
        vec![nv(1.0), nv(3.0), nv(6.0), nv(1.0)],
        vec![nv(1.0), nv(1.0), nv(1.0), nv(0.0)],
        vec![nv(7.0), nv(3.0), nv(10.0), nv(2.0)],
    ]);
    close(ev("MDETERM", vec![av(m4)]), 88.0);
    is_err(ev("MDETERM", vec![av(row(&[1.0, 2.0]))]), CellError::Value);
    let inv = ev("MINVERSE", vec![av(arr(vec![vec![nv(4.0), nv(-1.0)], vec![nv(2.0), nv(0.0)]]))]);
    let r = rows_of(&inv);
    close(r[0][0].clone(), 0.0);
    close(r[0][1].clone(), 0.5);
    close(r[1][0].clone(), -1.0);
    close(r[1][1].clone(), 2.0);
    is_err(ev("MINVERSE", vec![av(arr(vec![vec![nv(1.0), nv(2.0)], vec![nv(2.0), nv(4.0)]]))]), Num);
    assert_eq!(rows_of(&ev1("MUNIT", 2.0)), vec![vec![nv(1.0), nv(0.0)], vec![nv(0.0), nv(1.0)]]);
    is_err(ev1("MUNIT", 0.0), CellError::Value);
    is_err(ev1("MUNIT", 1e6), Num);
    let coeffs = row(&[1.0, -0.5, 1.0 / 24.0, -1.0 / 720.0]);
    close_tol(ev("SERIESSUM", vec![n(PI / 4.0), n(0.0), n(2.0), av(coeffs)]), 0.707_103_214_823, 1e-9);
}

#[test]
fn random() {
    let mut ctx = TestCtx::default();
    let spec = crate::lookup("RAND").unwrap();
    assert!(spec.volatile);
    for _ in 0..50 {
        let v = crate::call(spec, &[], &mut ctx);
        let x = v.as_f64().unwrap();
        assert!((0.0..1.0).contains(&x));
    }
    let spec = crate::lookup("RANDBETWEEN").unwrap();
    for _ in 0..50 {
        let x = crate::call(spec, &[n(1.0), n(6.0)], &mut ctx).as_f64().unwrap();
        assert!((1.0..=6.0).contains(&x) && x.fract() == 0.0);
    }
    is_err(ev2("RANDBETWEEN", 5.0, 1.0), Num);
}

#[test]
fn lifting() {
    assert_eq!(rows_of(&ev("ABS", vec![av(row(&[-1.0, 2.0, -3.0]))])), vec![vec![nv(1.0), nv(2.0), nv(3.0)]]);
    let v = ev("POWER", vec![av(col(&[1.0, 2.0])), av(row(&[2.0, 3.0]))]);
    assert_eq!(rows_of(&v), vec![vec![nv(1.0), nv(1.0)], vec![nv(4.0), nv(8.0)]]);
    close(ev("ROUND", vec![av(arr(vec![vec![nv(1.25)]])), n(1.0)]), 1.3);
}

fn data() -> (Value, Value) {
    let names = arr(vec![vec![tv("apple")], vec![tv("banana")], vec![tv("Apple")], vec![Value::Empty], vec![tv("cherry")]]);
    let qty = col(&[10.0, 20.0, 30.0, 40.0, 50.0]);
    (names, qty)
}

#[test]
fn criteria_functions() {
    let (names, qty) = data();
    close(ev("COUNTIF", vec![rf(names.clone()), t("apple")]), 2.0);
    close(ev("COUNTIF", vec![rf(names.clone()), t("a*")]), 2.0);
    close(ev("COUNTIF", vec![rf(names.clone()), t("<>apple")]), 3.0);
    close(ev("COUNTIF", vec![rf(names.clone()), t("")]), 1.0);
    close(ev("COUNTIF", vec![rf(qty.clone()), t(">20")]), 3.0);
    close(ev("COUNTIF", vec![rf(qty.clone()), n(30.0)]), 1.0);
    let multi = ev("COUNTIF", vec![rf(names.clone()), av(arr(vec![vec![tv("apple"), tv("cherry")]]))]);
    assert_eq!(rows_of(&multi), vec![vec![nv(2.0), nv(1.0)]]);
    close(ev("SUMIF", vec![rf(names.clone()), t("apple"), rf(qty.clone())]), 40.0);
    close(ev("SUMIF", vec![rf(qty.clone()), t(">=30")]), 120.0);
    close(ev("SUMIF", vec![rf(names.clone()), t("?????"), rf(col(&[1.0]))]), 1.0);
    close(ev("AVERAGEIF", vec![rf(qty.clone()), t(">20")]), 40.0);
    is_err(ev("AVERAGEIF", vec![rf(qty.clone()), t(">100")]), Div0);
    close(ev("SUMIFS", vec![rf(qty.clone()), rf(names.clone()), t("apple"), rf(qty.clone()), t(">15")]), 30.0);
    close(ev("COUNTIFS", vec![rf(names.clone()), t("*a*"), rf(qty.clone()), t("<25")]), 2.0);
    close(ev("AVERAGEIFS", vec![rf(qty.clone()), rf(names.clone()), t("<>banana")]), 32.5);
    close(ev("MAXIFS", vec![rf(qty.clone()), rf(names.clone()), t("apple")]), 30.0);
    close(ev("MINIFS", vec![rf(qty.clone()), rf(names.clone()), t("apple")]), 10.0);
    close(ev("MAXIFS", vec![rf(qty.clone()), rf(names.clone()), t("zzz")]), 0.0);
    is_err(ev("SUMIFS", vec![rf(qty.clone()), rf(row(&[1.0, 2.0])), t("1")]), CellError::Value);
    is_err(ev("COUNTIFS", vec![rf(qty.clone()), t(">1"), rf(qty)]), CellError::Value);
    is_err(ev("SUMIF", vec![rf(col(&[1.0, 2.0])), t(">0"), rf(arr(vec![vec![Value::Error(NA)], vec![nv(1.0)]]))]), NA);
}

#[test]
fn criteria_over_255_chars_are_value_errors_with_one_pair_and_truncated_with_more() {
    let ok = "x".repeat(255);
    let long = "x".repeat(256);
    let cells = arr(vec![vec![tv(&ok)], vec![tv(&long)]]);
    let qty = col(&[1.0, 2.0]);
    close(ev("COUNTIF", vec![rf(cells.clone()), t(&ok)]), 1.0);
    is_err(ev("COUNTIF", vec![rf(cells.clone()), t(&long)]), CellError::Value);
    close(ev("SUMIFS", vec![rf(qty.clone()), rf(cells.clone()), t(&ok)]), 1.0);
    is_err(ev("SUMIFS", vec![rf(qty.clone()), rf(cells.clone()), t(&long)]), CellError::Value);
    is_err(ev("COUNTIFS", vec![rf(cells.clone()), t(&long)]), CellError::Value);
    // With two or more pairs Excel matches the first 255 characters: the 255-character cell
    // (quantity 1), not the 256-character one (quantity 2). Checked on Excel 2016 and 365.
    let over = format!("{ok}y");
    close(ev("COUNTIFS", vec![rf(cells.clone()), t(&long), rf(qty.clone()), t(">0")]), 1.0);
    close(ev("SUMIFS", vec![rf(qty.clone()), rf(cells.clone()), t(&long), rf(qty.clone()), t(">0")]), 1.0);
    close(ev("SUMIFS", vec![rf(qty.clone()), rf(qty.clone()), t(">0"), rf(cells.clone()), t(&over)]), 1.0);
    close(ev("AVERAGEIFS", vec![rf(qty.clone()), rf(cells.clone()), t(&long), rf(qty.clone()), t(">0")]), 1.0);
    close(ev("MAXIFS", vec![rf(qty.clone()), rf(cells.clone()), t(&"x".repeat(300)), rf(qty), t(">0")]), 1.0);
}

#[test]
fn criteria_over_255_utf16_units_follow_excel() {
    // The tester's sheet, identical on Excel 2016 and Microsoft 365: A1:A6 hold these
    // strings, B is the row number, and each row uses its own string as the criterion.
    let e = "😀";
    let strings =
        [e.repeat(127), e.repeat(128), format!("{}{e}", "a".repeat(254)), format!("{}{e}", "a".repeat(253)), e.repeat(200), "я".repeat(256)];
    let cells = arr(strings.iter().map(|s| vec![tv(s)]).collect());
    let rows = col(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    // COUNTIF, SUMIF, COUNTIFS with one pair, SUMIFS with a second pair `B,">0"`.
    let expected: [Option<[f64; 4]>; 6] = [Some([1.0, 1.0, 1.0, 1.0]), None, None, Some([1.0, 4.0, 1.0, 4.0]), None, None];
    for (s, want) in strings.iter().zip(expected) {
        let countif = ev("COUNTIF", vec![rf(cells.clone()), t(s)]);
        let sumif = ev("SUMIF", vec![rf(cells.clone()), t(s), rf(rows.clone())]);
        let countifs = ev("COUNTIFS", vec![rf(cells.clone()), t(s)]);
        let sumifs = ev("SUMIFS", vec![rf(rows.clone()), rf(cells.clone()), t(s), rf(rows.clone()), t(">0")]);
        match want {
            Some([a, b, c, d]) => {
                close(countif, a);
                close(sumif, b);
                close(countifs, c);
                close(sumifs, d);
            }
            // Over 255 units: `#VALUE!` with one pair; cut to 255 units with two, which
            // matches no cell (for emoji it splits a surrogate pair).
            None => {
                is_err(countif, CellError::Value);
                is_err(sumif, CellError::Value);
                is_err(countifs, CellError::Value);
                close(sumifs, 0.0);
            }
        }
    }
}

#[test]
fn criteria_cut_inside_a_surrogate_pair() {
    let e = "😀";
    let cells = arr(vec![vec![tv(&e.repeat(127))], vec![tv(&e.repeat(128))], vec![tv("zzz")], vec![Value::Empty]]);
    let rows = col(&[1.0, 2.0, 3.0, 4.0]);
    let sumifs = |crit: String| ev("SUMIFS", vec![rf(rows.clone()), rf(cells.clone()), t(&crit), rf(rows.clone()), t(">0")]);
    // "=" + 😀×128 is 257 units: the cut after 255 keeps "=" and 127 whole emoji.
    close(sumifs(format!("={}", e.repeat(128))), 1.0);
    // With a two-unit prefix the cut keeps 126 emoji and half of the 127th. Such a criterion
    // equals no cell text: "=" matches nothing and "<>" everything, blanks included.
    close(sumifs(format!("=a{}", e.repeat(128))), 0.0);
    close(sumifs(format!("<>{}", e.repeat(128))), 10.0);
    // Comparisons treat it as sorting just above the 126 kept emoji.
    close(sumifs(format!(">={}", e.repeat(128))), 1.0 + 2.0);
    close(sumifs(format!("<={}", e.repeat(128))), 3.0);
}
