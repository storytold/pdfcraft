use super::descriptive::{kth, median, percentile_exc, percentile_inc, sorted};
use super::{MAX_REGRESSORS, aggregate_values, aggregate_values_k, check_regression_size};
use crate::util::R;
use crate::util::testutil::*;
use gridcraft_core::{CellError, Value};

const D7: f64 = 1e-6;

fn c(xs: &[f64]) -> crate::Arg {
    av(col(xs))
}

// The two middle values are finite and their midpoint is representable even when
// adding them first would overflow. Exercise both callers of the shared helper.
#[test]
fn median_finite_midpoint_does_not_overflow() {
    for (values, expected) in [
        (vec![1e308, 1e308], 1e308),
        (vec![-1e308, -1e308], -1e308),
        (vec![1.6e308, 1e308], 1.3e308),
        (vec![-1e308, -1.6e308], -1.3e308),
        (vec![f64::MAX, f64::MAX], f64::MAX),
        (vec![-f64::MAX, -f64::MAX], -f64::MAX),
        (vec![-f64::MAX, f64::MAX], 0.0),
        (vec![1.7e308, -1.7e308, 1e308, 1.6e308], 1.3e308),
    ] {
        close(ev("MEDIAN", values.iter().copied().map(n).collect()), expected);
        close(ev("MEDIAN", vec![rf(col(&values))]), expected);
        close(aggregate_values(12, &values.iter().copied().map(nv).collect::<Vec<_>>()), expected);
    }
}

#[test]
fn median_midpoint_keeps_small_values_and_argument_semantics() {
    // Halving each operand unconditionally can lose the smallest numbers. Use exact
    // equality here: the usual aggregate-test tolerance would accept zero.
    for value in [f64::MIN_POSITIVE, -f64::MIN_POSITIVE, f64::from_bits(1), -f64::from_bits(1)] {
        assert_eq!(ev("MEDIAN", vec![n(value), n(value)]), nv(value));
        assert_eq!(aggregate_values(12, &[nv(value), nv(value)]), nv(value));
    }
    close(ev("MEDIAN", vec![c(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])]), 3.5);
    close(ev("MEDIAN", vec![c(&[1e308, -1e308, 3.0])]), 3.0);
    close(ev("MEDIAN", vec![n(1e308)]), 1e308);
    close(ev("MEDIAN", vec![t("4"), b(true)]), 2.5);
    close(ev("MEDIAN", vec![rf(arr(vec![vec![nv(1.0), tv("ignored"), Value::Empty, Value::Bool(true), nv(6.0)]]))]), 3.5);
    is_err(ev("MEDIAN", vec![t("invalid")]), CellError::Value);
    is_err(ev("MEDIAN", vec![e(CellError::NA), n(1e308)]), CellError::NA);
    is_err(ev("MEDIAN", vec![rf(arr(vec![vec![tv("ignored"), Value::Empty]]))]), CellError::Num);
    is_err(aggregate_values(12, &[]), CellError::Num);
    is_err(aggregate_values(12, &[nv(1e308), Value::Error(CellError::Div0)]), CellError::Div0);
}

#[test]
fn averages_and_counts() {
    close(ev("AVERAGE", vec![n(1.0), n(2.0), n(6.0)]), 3.0);
    is_err(ev("AVERAGE", vec![rf(arr(vec![vec![tv("a")]]))]), CellError::Div0);
    let mixed = arr(vec![vec![nv(1.0), tv("x"), Value::Bool(true), Value::Empty, nv(3.0)]]);
    close(ev("AVERAGE", vec![rf(mixed.clone())]), 2.0);
    close(ev("AVERAGEA", vec![rf(mixed.clone())]), 1.25);
    close(ev("AVERAGE", vec![t("4"), b(true)]), 2.5);
    is_err(ev("AVERAGE", vec![t("abc")]), CellError::Value);
    is_err(ev("AVERAGE", vec![rf(arr(vec![vec![nv(1.0), Value::Error(CellError::NA)]]))]), CellError::NA);
    close(ev("COUNT", vec![rf(mixed.clone()), n(4.0), t("5"), t("x")]), 4.0);
    close(ev("COUNT", vec![e(CellError::Div0)]), 0.0);
    close(ev("COUNTA", vec![rf(mixed.clone())]), 4.0);
    close(ev("COUNTA", vec![n(1.0), t("")]), 2.0);
    let blanks = arr(vec![vec![Value::Empty, tv(""), nv(0.0), tv(" ")]]);
    close(ev("COUNTBLANK", vec![rf(blanks)]), 2.0);
    close(ev("MEDIAN", vec![c(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])]), 3.5);
    close(ev("MEDIAN", vec![c(&[3.0, 1.0, 2.0])]), 2.0);
    is_err(ev("MEDIAN", vec![rf(arr(vec![vec![tv("a")]]))]), CellError::Num);
    close(ev("MODE.SNGL", vec![c(&[5.6, 4.0, 4.0, 3.0, 2.0, 4.0])]), 4.0);
    close(ev("MODE", vec![c(&[1.0, 2.0, 2.0, 1.0])]), 1.0);
    is_err(ev("MODE", vec![c(&[1.0, 2.0, 3.0])]), CellError::NA);
    assert_eq!(rows_of(&ev("MODE.MULT", vec![c(&[1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0, 2.0, 3.0])])), vec![vec![nv(2.0)], vec![nv(3.0)]]);
}

/// MODE argument semantics in Excel 365, on the inputs of FormulaEvalTestData (row "Mode"):
/// text is skipped, typed or referenced, but a single-cell reference to a blank cell is
/// `#VALUE!`; the first error from the left wins. The values cached in POI's file come from an
/// older Excel and say `#VALUE!` for `MODE(C9,E16,"1",B8)`, `MODE(B8,"1",2,3)` and
/// `MODE("1",1,#DIV/0!,2,3)`; Excel 365 gives `#N/A`, `#N/A` and `#DIV/0!`.
#[test]
fn mode_scalar_arguments() {
    let data = arr(vec![vec![tv(""), nv(0.0), nv(1.0)], vec![tv("x"), nv(1.0), nv(1.001)], vec![tv("y"), nv(9e9), nv(2.3456)]]);
    let cases: Vec<(Vec<crate::Arg>, Result<f64, CellError>)> = vec![
        (vec![n(1.0), n(1.0), n(2.0), n(3.0)], Ok(1.0)),
        (vec![rf(data)], Ok(1.0)),
        (vec![rf(arr(vec![vec![nv(-1.1), Value::Empty, tv("1.1")]])), n(-1.1)], Ok(-1.1)),
        // MODE(C9,E16,"1",B8): E16 holds the text "1.1".
        (vec![rf(nv(1.1)), rf(tv("1.1")), t("1"), rf(nv(1.0))], Err(CellError::NA)),
        (vec![rf(nv(1.0)), t("1"), n(2.0), n(3.0)], Err(CellError::NA)),
        (vec![t("1"), n(1.0), e(CellError::Div0), n(2.0), n(3.0)], Err(CellError::Div0)),
        // MODE(B9,2,D7) with D7 blank.
        (vec![rf(nv(2.0)), n(2.0), rf(Value::Empty)], Err(CellError::Value)),
        (vec![n(1.0), n(1.0), e(CellError::Div0), t("1")], Err(CellError::Div0)),
    ];
    for name in ["MODE", "MODE.SNGL"] {
        for (args, want) in &cases {
            match want {
                Ok(x) => close(ev(name, args.clone()), *x),
                Err(err) => is_err(ev(name, args.clone()), *err),
            }
        }
    }
    is_err(ev("MODE.MULT", vec![rf(nv(1.0)), t("1"), rf(Value::Empty)]), CellError::Value);
}

/// Text and logical values typed as arguments are skipped, not coerced (Excel 365, #736).
#[test]
fn mode_skips_typed_text_and_logicals() {
    let cases: Vec<(Vec<crate::Arg>, Result<f64, CellError>)> = vec![
        (vec![n(1.0), n(1.0), t("x")], Ok(1.0)),
        (vec![n(1.0), n(2.0), t("x")], Err(CellError::NA)),
        (vec![n(1.0), t("1"), n(2.0), n(3.0)], Err(CellError::NA)),
        (vec![t("1"), n(1.0), n(2.0), n(3.0)], Err(CellError::NA)),
        (vec![t("1"), n(1.0), e(CellError::Div0), n(2.0), n(3.0)], Err(CellError::Div0)),
        (vec![n(1.0), b(true), b(true), n(2.0)], Err(CellError::NA)),
        (vec![n(1.0), n(1.0), b(true)], Ok(1.0)),
    ];
    for name in ["MODE", "MODE.SNGL"] {
        for (args, want) in &cases {
            match want {
                Ok(x) => close(ev(name, args.clone()), *x),
                Err(err) => is_err(ev(name, args.clone()), *err),
            }
        }
    }
    assert_eq!(rows_of(&ev("MODE.MULT", vec![n(1.0), t("1"), n(1.0), t("x")])), vec![vec![nv(1.0)]]);
    is_err(ev("MODE.MULT", vec![n(1.0), b(true), b(true), n(2.0)]), CellError::NA);
}

#[test]
fn spread() {
    let d = [1345.0, 1301.0, 1368.0, 1322.0, 1310.0, 1370.0, 1318.0, 1350.0, 1303.0, 1299.0];
    close_tol(ev("STDEV.S", vec![c(&d)]), 27.46391572, 1e-9);
    close_tol(ev("STDEV", vec![c(&d)]), 27.46391572, 1e-9);
    close_tol(ev("STDEV.P", vec![c(&d)]), 26.05455814, 1e-9);
    close_tol(ev("STDEVP", vec![c(&d)]), 26.05455814, 1e-9);
    close_tol(ev("VAR.S", vec![c(&d)]), 754.2666667, 1e-9);
    close(ev("VAR.P", vec![c(&d)]), 678.84);
    close(ev("VARP", vec![c(&d)]), 678.84);
    is_err(ev("STDEV.S", vec![n(1.0)]), CellError::Div0);
    close(ev("STDEV.P", vec![n(1.0)]), 0.0);
    let mixed = arr(vec![vec![nv(2.0), Value::Bool(true), tv("x")]]);
    close(ev("VARA", vec![rf(mixed.clone())]), 1.0);
    close(ev("VARPA", vec![rf(mixed.clone())]), 2.0 / 3.0);
    close(ev("STDEVA", vec![rf(mixed.clone())]), 1.0);
    close(ev("STDEVPA", vec![rf(mixed)]), (2.0f64 / 3.0).sqrt());
    let s = [4.0, 5.0, 6.0, 7.0, 5.0, 4.0, 3.0];
    close(ev("DEVSQ", vec![c(&[4.0, 5.0, 8.0, 7.0, 11.0, 4.0, 3.0])]), 48.0);
    close_tol(ev("AVEDEV", vec![c(&s)]), 1.020408163, 1e-9);
    let g = [4.0, 5.0, 8.0, 7.0, 11.0, 4.0, 3.0];
    close_tol(ev("GEOMEAN", vec![c(&g)]), 5.476986969, 1e-9);
    close_tol(ev("HARMEAN", vec![c(&g)]), 5.028375962, 1e-9);
    let text_values = arr(vec![vec![tv("1"), tv("2")]]);
    let bool_values = arr(vec![vec![Value::Bool(true), Value::Bool(false)]]);
    let mixed_values = arr(vec![vec![nv(2.0), tv("ignored"), Value::Bool(true)]]);
    is_err(ev("HARMEAN", vec![av(text_values.clone())]), CellError::NA);
    is_err(ev("HARMEAN", vec![av(bool_values.clone())]), CellError::NA);
    is_err(ev("HARMEAN", vec![rf(text_values)]), CellError::NA);
    is_err(ev("HARMEAN", vec![rf(bool_values)]), CellError::NA);
    close(ev("HARMEAN", vec![av(mixed_values.clone())]), 2.0);
    close(ev("HARMEAN", vec![rf(mixed_values)]), 2.0);
    close(ev("HARMEAN", vec![b(true)]), 1.0);
    close(ev("HARMEAN", vec![n(2.0), n(4.0)]), 8.0 / 3.0);
    is_err(ev("HARMEAN", vec![av(arr(vec![vec![nv(0.0)]]))]), CellError::Num);
    is_err(ev("HARMEAN", vec![av(arr(vec![vec![nv(-1.0)]]))]), CellError::Num);
    is_err(ev("GEOMEAN", vec![c(&[1.0, 0.0])]), CellError::Num);
    close_tol(ev("TRIMMEAN", vec![c(&[4.0, 5.0, 6.0, 7.0, 2.0, 3.0, 4.0, 5.0, 1.0, 2.0, 3.0]), n(0.2)]), 3.777777778, 1e-9);
    is_err(ev("TRIMMEAN", vec![c(&[1.0]), n(1.0)]), CellError::Num);
    let k = [3.0, 4.0, 5.0, 2.0, 3.0, 4.0, 5.0, 6.0, 4.0, 7.0];
    close_tol(ev("KURT", vec![c(&k)]), -0.151799637, 1e-8);
    close_tol(ev("SKEW", vec![c(&k)]), 0.359543071, 1e-8);
    close_tol(ev("SKEW.P", vec![c(&k)]), 0.303193339, 1e-8);
    is_err(ev("KURT", vec![c(&[1.0, 2.0, 3.0])]), CellError::Div0);
    close_tol(ev("STANDARDIZE", vec![n(42.0), n(40.0), n(1.5)]), 1.333333333, 1e-9);
    is_err(ev("STANDARDIZE", vec![n(42.0), n(40.0), n(0.0)]), CellError::Num);
}

#[test]
fn min_max_order() {
    close(ev("MIN", vec![c(&[3.0, -1.0, 2.0]), n(0.0)]), -1.0);
    close(ev("MAX", vec![c(&[3.0, -1.0, 2.0]), n(10.0)]), 10.0);
    close(ev("MIN", vec![]), 0.0);
    close(ev("MAX", vec![rf(arr(vec![vec![tv("a")]]))]), 0.0);
    close(ev("MAXA", vec![rf(arr(vec![vec![nv(-1.0), Value::Bool(true)]]))]), 1.0);
    close(ev("MINA", vec![rf(arr(vec![vec![nv(1.0), tv("x")]]))]), 0.0);
    let d = c(&[3.0, 5.0, 3.0, 5.0, 4.0, 4.0, 2.0, 4.0, 6.0, 7.0]);
    close(ev("LARGE", vec![d.clone(), n(3.0)]), 5.0);
    close(ev("SMALL", vec![d.clone(), n(4.0)]), 4.0);
    is_err(ev("LARGE", vec![d.clone(), n(0.0)]), CellError::Num);
    is_err(ev("SMALL", vec![d.clone(), n(11.0)]), CellError::Num);
    assert_eq!(rows_of(&ev("LARGE", vec![d, av(row(&[1.0, 2.0]))])), vec![vec![nv(7.0), nv(6.0)]]);
    let r = c(&[89.0, 88.0, 92.0, 101.0, 94.0, 97.0, 95.0]);
    close(ev("RANK.AVG", vec![n(94.0), r.clone()]), 4.0);
    close(ev("RANK.EQ", vec![n(101.0), r.clone()]), 1.0);
    is_err(ev("RANK", vec![n(1.0), r]), CellError::NA);
    let r2 = c(&[7.0, 3.5, 3.5, 1.0, 2.0]);
    close(ev("RANK.EQ", vec![n(7.0), r2.clone(), n(1.0)]), 5.0);
    close(ev("RANK.EQ", vec![n(3.5), r2.clone(), n(1.0)]), 3.0);
    close(ev("RANK.AVG", vec![n(3.5), r2, n(1.0)]), 3.5);
}

// Reference implementations: the previous full-sort versions, as the oracle for the selection ones.
fn old_interp(s: &[f64], pos: f64) -> R<f64> {
    let i = pos.floor();
    let frac = pos - i;
    let i = i as usize;
    let lo = *s.get(i).ok_or(CellError::Num)?;
    match s.get(i + 1) {
        Some(hi) if frac > 0.0 => Ok(lo + frac * (hi - lo)),
        _ => Ok(lo),
    }
}

fn old_median(v: Vec<f64>) -> R<f64> {
    if v.is_empty() {
        return Err(CellError::Num);
    }
    let s = sorted(v);
    let n = s.len();
    let hi = s.get(n / 2).copied().unwrap_or(0.0);
    if n % 2 == 1 { Ok(hi) } else { Ok(s.get(n / 2 - 1).copied().unwrap_or(0.0).midpoint(hi)) }
}

fn old_kth(v: Vec<f64>, k: f64, largest: bool) -> R<f64> {
    let k = k.ceil();
    if k.is_nan() || k < 1.0 || k > v.len() as f64 {
        return Err(CellError::Num);
    }
    let s = sorted(v);
    let k = k as usize;
    let idx = if largest { s.len() - k } else { k - 1 };
    s.get(idx).copied().ok_or(CellError::Num)
}

fn old_percentile_inc(v: Vec<f64>, k: f64) -> R<f64> {
    if v.is_empty() || !(0.0..=1.0).contains(&k) {
        return Err(CellError::Num);
    }
    let s = sorted(v);
    old_interp(&s, k * (s.len() - 1) as f64)
}

fn old_percentile_exc(v: Vec<f64>, k: f64) -> R<f64> {
    let n = v.len() as f64;
    if v.is_empty() || !(k > 0.0 && k < 1.0) {
        return Err(CellError::Num);
    }
    let pos = k * (n + 1.0);
    if pos < 1.0 || pos > n {
        return Err(CellError::Num);
    }
    let s = sorted(v);
    old_interp(&s, pos - 1.0)
}

/// Same Ok bit pattern or same error.
fn same(a: R<f64>, b: R<f64>) -> bool {
    match (a, b) {
        (Ok(x), Ok(y)) => x.to_bits() == y.to_bits(),
        (Err(x), Err(y)) => x == y,
        _ => false,
    }
}

#[test]
fn selection_matches_full_sort_bit_for_bit() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let specials = [0.0, -0.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN, f64::MAX, f64::MIN_POSITIVE, 1.0, -1.0];
    for len in (0..=40).chain([63, 64, 65, 100, 257, 1000, 4096]) {
        for round in 0..24 {
            let mut v: Vec<f64> = (0..len)
                .map(|_| match (round % 4, next() % 8) {
                    // Few distinct values: many ties.
                    (0, r) => (r % 3) as f64,
                    // Specials, including both zeros and NaNs.
                    (1, r) => specials[(next() as usize).wrapping_add(r as usize) % specials.len()],
                    // Wide random range.
                    (2, _) => (next() as i64 as f64) / 1e3,
                    // Integers with some specials mixed in.
                    (_, 0) => specials[next() as usize % specials.len()],
                    _ => (next() % 50) as f64,
                })
                .collect();
            // Rounds 12..24 repeat the data as already ascending, already descending and nearly
            // ascending (ends swapped): the monotone shortcut and its fallback to selection.
            match round / 4 {
                3 => v.sort_by(f64::total_cmp),
                4 => {
                    v.sort_by(f64::total_cmp);
                    v.reverse();
                }
                5 => {
                    v.sort_by(f64::total_cmp);
                    if let Some(last) = v.len().checked_sub(1) {
                        v.swap(0, last);
                    }
                }
                _ => {}
            }
            assert!(same(median(v.clone()), old_median(v.clone())), "median len={len} round={round}");
            let ks = [-1.0, 0.0, 0.2, 1.0, 1.5, 2.0, len as f64 / 2.0, len as f64 - 0.5, len as f64, len as f64 + 1.0, f64::NAN];
            for &k in &ks {
                for largest in [true, false] {
                    assert!(same(kth(v.clone(), k, largest), old_kth(v.clone(), k, largest)), "kth len={len} k={k}");
                }
            }
            let ps = [-0.1, 0.0, 0.01, 0.1, 0.25, 0.3, 0.5, 0.75, 0.9, 0.99, 1.0, 1.1, f64::NAN, (next() % 1001) as f64 / 1000.0];
            for &p in &ps {
                assert!(same(percentile_inc(v.clone(), p), old_percentile_inc(v.clone(), p)), "pinc len={len} p={p}");
                assert!(same(percentile_exc(v.clone(), p), old_percentile_exc(v.clone(), p)), "pexc len={len} p={p}");
            }
        }
    }
}

#[test]
fn percentiles() {
    close(ev("PERCENTILE.INC", vec![c(&[1.0, 3.0, 2.0, 4.0]), n(0.3)]), 1.9);
    close(ev("PERCENTILE", vec![c(&[1.0, 3.0, 2.0, 4.0]), n(1.0)]), 4.0);
    is_err(ev("PERCENTILE.INC", vec![c(&[1.0]), n(1.5)]), CellError::Num);
    let e9 = c(&[1.0, 2.0, 3.0, 6.0, 6.0, 6.0, 7.0, 8.0, 9.0]);
    close(ev("PERCENTILE.EXC", vec![e9.clone(), n(0.25)]), 2.5);
    is_err(ev("PERCENTILE.EXC", vec![e9.clone(), n(0.0)]), CellError::Num);
    is_err(ev("PERCENTILE.EXC", vec![e9.clone(), n(0.01)]), CellError::Num);
    close(ev("QUARTILE.INC", vec![c(&[1.0, 2.0, 4.0, 7.0, 8.0, 9.0, 10.0, 12.0]), n(1.0)]), 3.5);
    close(ev("QUARTILE", vec![c(&[1.0, 2.0, 4.0, 7.0, 8.0, 9.0, 10.0, 12.0]), n(4.0)]), 12.0);
    let q = c(&[6.0, 7.0, 15.0, 36.0, 39.0, 40.0, 41.0, 42.0, 43.0, 47.0, 49.0]);
    close(ev("QUARTILE.EXC", vec![q.clone(), n(1.0)]), 15.0);
    close(ev("QUARTILE.EXC", vec![q.clone(), n(3.0)]), 43.0);
    is_err(ev("QUARTILE.EXC", vec![q, n(4.0)]), CellError::Num);
    let p = c(&[13.0, 12.0, 11.0, 8.0, 4.0, 3.0, 2.0, 1.0, 1.0, 1.0]);
    close(ev("PERCENTRANK.INC", vec![p.clone(), n(2.0)]), 0.333);
    close(ev("PERCENTRANK.INC", vec![p.clone(), n(4.0)]), 0.555);
    close(ev("PERCENTRANK.INC", vec![p.clone(), n(8.0)]), 0.666);
    close(ev("PERCENTRANK", vec![p.clone(), n(5.0)]), 0.583);
    close(ev("PERCENTRANK.INC", vec![p.clone(), n(5.0), n(1.0)]), 0.5);
    is_err(ev("PERCENTRANK.INC", vec![p.clone(), n(0.5)]), CellError::NA);
    is_err(ev("PERCENTRANK.INC", vec![p, n(5.0), n(0.0)]), CellError::Num);
    close(ev("PERCENTRANK.EXC", vec![e9.clone(), n(7.0)]), 0.7);
    close(ev("PERCENTRANK.EXC", vec![e9.clone(), n(5.43)]), 0.381);
    close(ev("PERCENTRANK.EXC", vec![e9, n(5.43), n(1.0)]), 0.3);
    let t = c(&[1.0, 2.0, 3.0]);
    // Excel 16: significance 309 and up is #DIV/0!, 308 still works.
    for name in ["PERCENTRANK", "PERCENTRANK.INC", "PERCENTRANK.EXC"] {
        is_err(ev(name, vec![t.clone(), n(2.0), n(309.0)]), CellError::Div0);
        is_err(ev(name, vec![t.clone(), n(1.0), n(400.0)]), CellError::Div0);
        close(ev(name, vec![t.clone(), n(2.0), n(308.0)]), 0.5);
    }
    // From significance 15 up the result has 15 significant digits.
    let k = c(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    for sig in [15.0, 16.0, 308.0] {
        close_tol(ev("PERCENTRANK.INC", vec![k.clone(), n(2.123_456_789), n(sig)]), 0.187_242_798_166_667, 1e-16);
    }
    close_tol(ev("PERCENTRANK.INC", vec![k.clone(), n(2.123_456_789), n(14.0)]), 0.187_242_798_166_66, 1e-15);
    close(ev("PERCENTRANK.EXC", vec![k, n(2.123_456_789), n(16.0)]), 0.265_432_098_625);
}

#[test]
fn pairs_and_regression() {
    close_tol(ev("CORREL", vec![c(&[3.0, 2.0, 4.0, 5.0, 6.0]), c(&[9.0, 7.0, 12.0, 15.0, 17.0])]), 0.997054486, 1e-9);
    close_tol(ev("PEARSON", vec![c(&[9.0, 7.0, 5.0, 3.0, 1.0]), c(&[10.0, 6.0, 1.0, 5.0, 3.0])]), 0.699379, D7);
    is_err(ev("CORREL", vec![c(&[1.0, 2.0]), c(&[1.0, 2.0, 3.0])]), CellError::NA);
    is_err(ev("CORREL", vec![c(&[1.0, 1.0]), c(&[1.0, 2.0])]), CellError::Div0);
    let y = c(&[2.0, 3.0, 9.0, 1.0, 8.0, 7.0, 5.0]);
    let x = c(&[6.0, 5.0, 11.0, 7.0, 5.0, 4.0, 4.0]);
    close_tol(ev("SLOPE", vec![y.clone(), x.clone()]), 0.305555556, 1e-8);
    close_tol(ev("RSQ", vec![y.clone(), x.clone()]), 0.057950192, 1e-8);
    close_tol(ev("STEYX", vec![y.clone(), x.clone()]), 3.305718950, 1e-8);
    close_tol(ev("INTERCEPT", vec![c(&[2.0, 3.0, 9.0, 1.0, 8.0]), c(&[6.0, 5.0, 11.0, 7.0, 5.0])]), 0.048387097, 1e-8);
    close_tol(ev("FORECAST", vec![n(30.0), c(&[6.0, 7.0, 9.0, 15.0, 21.0]), c(&[20.0, 28.0, 31.0, 38.0, 40.0])]), 10.607253, D7);
    close_tol(ev("FORECAST.LINEAR", vec![n(30.0), c(&[6.0, 7.0, 9.0, 15.0, 21.0]), c(&[20.0, 28.0, 31.0, 38.0, 40.0])]), 10.607253, D7);
    close(ev("COVARIANCE.P", vec![c(&[3.0, 2.0, 4.0, 5.0, 6.0]), c(&[9.0, 7.0, 12.0, 15.0, 17.0])]), 5.2);
    close(ev("COVAR", vec![c(&[3.0, 2.0, 4.0, 5.0, 6.0]), c(&[9.0, 7.0, 12.0, 15.0, 17.0])]), 5.2);
    close(ev("COVARIANCE.S", vec![c(&[2.0, 4.0, 8.0]), c(&[5.0, 11.0, 12.0])]), 29.0 / 3.0);
    // Pairs with text are skipped.
    let xs = arr(vec![vec![nv(1.0)], vec![tv("x")], vec![nv(3.0)]]);
    close(ev("SLOPE", vec![c(&[2.0, 100.0, 6.0]), rf(xs)]), 2.0);
}

#[test]
fn linest_family() {
    let r = rows_of(&ev("LINEST", vec![c(&[1.0, 9.0, 5.0, 7.0]), c(&[0.0, 4.0, 2.0, 3.0])]));
    assert_eq!(r.len(), 1);
    close(r[0][0].clone(), 2.0);
    close(r[0][1].clone(), 1.0);
    // Statistics for a simple fit: y = 1,2,4 on x = 1,2,3.
    let s = rows_of(&ev("LINEST", vec![c(&[1.0, 2.0, 4.0]), c(&[1.0, 2.0, 3.0]), b(true), b(true)]));
    assert_eq!((s.len(), s[0].len()), (5, 2));
    close(s[0][0].clone(), 1.5);
    close(s[0][1].clone(), -2.0 / 3.0);
    close(s[2][0].clone(), 0.9642857142857143); // r²
    close(s[3][1].clone(), 1.0); // df
    close(s[4][0].clone(), 4.5); // ssreg
    close(s[4][1].clone(), 1.0 / 6.0); // ssresid
    close(s[2][1].clone(), (1.0f64 / 6.0).sqrt()); // sey
    close(s[3][0].clone(), 27.0); // F
    close(s[1][0].clone(), (1.0f64 / 6.0 / 2.0).sqrt()); // se slope
    // No constant.
    let z = rows_of(&ev("LINEST", vec![c(&[2.0, 4.0, 6.0]), c(&[1.0, 2.0, 3.0]), b(false)]));
    close(z[0][0].clone(), 2.0);
    close(z[0][1].clone(), 0.0);
    // Two predictors: y = 1 + 2a + 3b.
    let xs = arr(vec![vec![nv(1.0), nv(0.0)], vec![nv(0.0), nv(1.0)], vec![nv(1.0), nv(1.0)], vec![nv(2.0), nv(1.0)], vec![nv(3.0), nv(5.0)]]);
    let ys: Vec<f64> = [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (2.0, 1.0), (3.0, 5.0)].iter().map(|(a, b)| 1.0 + 2.0 * a + 3.0 * b).collect();
    let m = rows_of(&ev("LINEST", vec![c(&ys), av(xs)]));
    close(m[0][0].clone(), 3.0);
    close(m[0][1].clone(), 2.0);
    close(m[0][2].clone(), 1.0);
    let gy = [33100.0, 47300.0, 69000.0, 102000.0, 150000.0, 220000.0];
    let gx = [11.0, 12.0, 13.0, 14.0, 15.0, 16.0];
    let l = rows_of(&ev("LOGEST", vec![c(&gy), c(&gx)]));
    close_tol(l[0][0].clone(), 1.463275628, 1e-9);
    close_tol(l[0][1].clone(), 495.3047702, 1e-9);
    let g = rows_of(&ev("GROWTH", vec![c(&gy), c(&gx), c(&[17.0, 18.0])]));
    close_tol(g[0][0].clone(), 320196.7184, 1e-9);
    close_tol(g[1][0].clone(), 468536.0539, 1e-9);
    let ty = [133890.0, 135000.0, 135790.0, 137300.0, 138130.0, 139100.0, 139900.0, 141120.0, 141890.0, 143230.0, 144000.0, 145290.0];
    let tx: Vec<f64> = (1..=12).map(f64::from).collect();
    let t1 = ev("TREND", vec![c(&ty), c(&tx), n(13.0)]);
    close_tol(Value::Number(rows_of(&t1)[0][0].as_f64().unwrap().round()), 146172.0, 1e-9);
    let fitted = rows_of(&ev("TREND", vec![c(&[1.0, 9.0, 5.0, 7.0]), c(&[0.0, 4.0, 2.0, 3.0])]));
    close(fitted[1][0].clone(), 9.0);
    is_err(ev("LOGEST", vec![c(&[1.0, -2.0])]), CellError::Num);
    is_err(ev("LINEST", vec![c(&[1.0, 2.0]), c(&[1.0, 2.0, 3.0])]), CellError::Ref);
}

/// LINEST on columns of predictors, `stats` on, as rows of numbers (`None` for an error cell).
fn linest_rows(y: &[f64], xs: &[Vec<f64>], constant: bool) -> Vec<Vec<Option<f64>>> {
    let x = arr((0..y.len()).map(|i| xs.iter().map(|col| nv(col[i])).collect()).collect());
    let r = rows_of(&ev("LINEST", vec![c(y), av(x), b(constant), b(true)]));
    r.iter().map(|row| row.iter().map(Value::as_f64).collect()).collect()
}

fn rel_close(got: Option<f64>, want: f64, tol: f64, what: &str) {
    let got = got.unwrap_or(f64::NAN);
    assert!((got - want).abs() <= tol * want.abs().max(1e-300), "{what}: got {got:e}, want {want:e}, rel tol {tol:e}");
}

// B2: LINEST solved the normal equations, whose condition number is the square of the data's.
// Expected values below come from exact rational arithmetic: the normal equations solved with
// Python `fractions.Fraction` (no rounding at all), rounded once to f64 (script in the PR text).
#[test]
fn linest_ill_conditioned_polynomial_is_exact() {
    // y = 1 + x + x² + x³ + x⁴ + x⁵ on x = 1..12: every coefficient is exactly 1. The normal
    // equations were off by up to 2.6e-6.
    let xs: Vec<f64> = (1..=12).map(f64::from).collect();
    let ys: Vec<f64> = xs.iter().map(|x| (0..=5).map(|j| x.powi(j)).sum()).collect();
    let cols: Vec<Vec<f64>> = (1..=5).map(|j| xs.iter().map(|x| x.powi(j)).collect()).collect();
    let r = linest_rows(&ys, &cols, true);
    for (i, v) in r[0].iter().enumerate() {
        rel_close(*v, 1.0, 1e-9, &format!("coefficient {i}"));
    }
}

#[test]
fn linest_large_offset_regressors_recover_the_line() {
    // y = 3x + 7 exactly with x = 1e6 + i and regressors [x, x²]: the true fit has x² coefficient
    // 0, x coefficient 3, intercept 7. The normal equations returned 2.99976 and 251.14.
    let xs: Vec<f64> = (0..12).map(|i| 1_000_000.0 + f64::from(i)).collect();
    let ys: Vec<f64> = xs.iter().map(|x| 3.0 * x + 7.0).collect();
    let r = linest_rows(&ys, &[xs.clone(), xs.iter().map(|x| x * x).collect()], true);
    assert!(r[0][0].unwrap().abs() < 1e-12, "x² coefficient {:?}", r[0][0]);
    rel_close(r[0][1], 3.0, 1e-9, "x coefficient");
    // The intercept subtracts 3e6 from 3e6 + 7 and carries the x² coefficient times 1e12, so
    // it is good to about 1e-4 in absolute terms, not better.
    assert!((r[0][2].unwrap() - 7.0).abs() < 1e-3, "intercept {:?}", r[0][2]);
    // One regressor: exact.
    let one = linest_rows(&ys, std::slice::from_ref(&xs), true);
    rel_close(one[0][0], 3.0, 1e-12, "slope");
    rel_close(one[0][1], 7.0, 1e-9, "intercept");
}

#[test]
fn linest_noisy_fits_match_exact_rational_least_squares() {
    // Two predictors with noise, constant fitted. Order of each row: x2, x1, intercept.
    let x1 = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let x2 = [2.0, 1.0, 4.0, 3.0, 7.0, 5.0, 9.0, 6.0];
    let y = [5.0, 6.0, 11.0, 12.0, 20.0, 19.0, 30.0, 28.0];
    let r = linest_rows(&y, &[x1.to_vec(), x2.to_vec()], true);
    let (coef, se) = ([1.4270152505446623, 2.452069716775599, -1.2592592592592593], [0.3376442444771937, 0.3679392851360571, 1.0847489958915801]);
    for i in 0..3 {
        rel_close(r[0][i], coef[i], 1e-12, "coef");
        rel_close(r[1][i], se[i], 1e-12, "se");
    }
    rel_close(r[2][0], 0.985070139277778, 1e-12, "r2");
    rel_close(r[2][1], 1.367057173604725, 1e-12, "sey");
    rel_close(r[3][0], 164.94965318256004, 1e-11, "F");
    assert_eq!(r[3][1], Some(5.0));
    rel_close(r[4][0], 616.5307734204792, 1e-12, "ssreg");
    rel_close(r[4][1], 9.344226579520697, 1e-11, "ssresid");
    assert_eq!(r[2][2], None);
    // const = FALSE: through the origin, R² and F from the uncentred sums.
    let r = linest_rows(&y, &[x1.to_vec(), x2.to_vec()], false);
    rel_close(r[0][0], 1.3529411764705883, 1e-12, "coef");
    rel_close(r[0][1], 2.303921568627451, 1e-12, "coef");
    assert_eq!(r[0][2], Some(0.0));
    rel_close(r[1][0], 0.34102994374364876, 1e-12, "se");
    rel_close(r[1][1], 0.35495521934550084, 1e-12, "se");
    assert_eq!(r[1][2], None);
    rel_close(r[2][0], 0.995718966041848, 1e-12, "r2");
    rel_close(r[2][1], 1.4061024795535124, 1e-12, "sey");
    rel_close(r[3][0], 697.7652892561983, 1e-11, "F");
    assert_eq!(r[3][1], Some(6.0));
    rel_close(r[4][0], 2759.1372549019607, 1e-12, "ssreg");
    rel_close(r[4][1], 11.862745098039216, 1e-11, "ssresid");
    // Cubic and quintic with noise on x = 1..12; order x^k .. x, intercept.
    let xs: Vec<f64> = (1..=12).map(f64::from).collect();
    let noise = [3.0, -2.0, 4.0, -1.0, 2.0, -3.0, 1.0, -4.0, 2.0, 3.0, -2.0, 1.0];
    let cub: Vec<f64> = xs.iter().zip(noise).map(|(x, e)| 1.0 + x + x * x + x.powi(3) + e).collect();
    let r = linest_rows(&cub, &(1..=3).map(|j| xs.iter().map(|x| x.powi(j)).collect()).collect::<Vec<Vec<f64>>>(), true);
    let coef = [0.9955969955969955, 1.1452991452991452, -0.33514633514633513, 4.373737373737374];
    let se = [0.02755463331301788, 0.5434124199147191, 3.102549978874372, 4.851885058051787];
    for i in 0..4 {
        rel_close(r[0][i], coef[i], 1e-11, "cubic coef");
        rel_close(r[1][i], se[i], 1e-10, "cubic se");
    }
    rel_close(r[2][0], 0.9999837292113533, 1e-12, "cubic r2");
    rel_close(r[4][1], 70.35586635586635, 1e-9, "cubic ssresid");
    let quint: Vec<f64> = xs.iter().zip(noise).map(|(x, e)| (0..=5).map(|j| x.powi(j)).sum::<f64>() + 50.0 * e).collect();
    let r = linest_rows(&quint, &(1..=5).map(|j| xs.iter().map(|x| x.powi(j)).collect()).collect::<Vec<Vec<f64>>>(), true);
    let coef = [0.9330693815987934, 3.022273070067188, -20.97398190045249, 108.20489681886741, -250.24940010969422, 271.45454545454544];
    let se = [0.20046539191103213, 6.538253746916523, 78.39635356777087, 421.2608646012634, 976.3504976093745, 736.5866411764556];
    for i in 0..6 {
        rel_close(r[0][i], coef[i], 1e-8, "quintic coef");
        rel_close(r[1][i], se[i], 1e-8, "quintic se");
    }
    rel_close(r[2][1], 168.58174051770763, 1e-9, "quintic sey");
    rel_close(r[4][1], 170518.81941587824, 1e-7, "quintic ssresid");
}

#[test]
fn linest_drops_collinear_columns() {
    // x2 = 2·x1: the later column is dropped (coefficient and standard error 0), the fit is the
    // one on x1 alone, and the degrees of freedom count only the kept column.
    let x1 = [1.0, 2.0, 3.0, 4.0, 5.0];
    let y = [2.0, 4.0, 5.0, 4.0, 5.0];
    let x2: Vec<f64> = x1.iter().map(|v| 2.0 * v).collect();
    let r = linest_rows(&y, &[x1.to_vec(), x2], true);
    // Exact: slope 0.6, intercept 2.2, ssresid 2.4, ssreg 3.6, df 3 (Fraction arithmetic).
    assert_eq!(r[0][0], Some(0.0));
    rel_close(r[0][1], 0.6, 1e-12, "slope");
    rel_close(r[0][2], 2.2, 1e-12, "intercept");
    assert_eq!(r[1][0], Some(0.0));
    rel_close(r[1][1], (0.8f64 / 10.0).sqrt(), 1e-12, "se slope");
    assert_eq!(r[3][1], Some(3.0));
    rel_close(r[4][0], 3.6, 1e-12, "ssreg");
    rel_close(r[4][1], 2.4, 1e-12, "ssresid");
    // A constant column is collinear with the intercept and dropped as well.
    let r = linest_rows(&y, &[vec![7.0; 5], x1.to_vec()], true);
    assert_eq!(r[0][1], Some(0.0));
    assert_eq!(r[1][1], Some(0.0));
    rel_close(r[0][0], 0.6, 1e-12, "slope");
    rel_close(r[0][2], 2.2, 1e-12, "intercept");
    // A constant column whose mean does not centre to exactly 0 (the mean of three 0.1 is
    // 0.10000000000000002) is just as collinear with the intercept: dropped, not scaled up to a
    // unit vector. Some length hits a rounding mean for each constant.
    for c in [0.1, 1.1, 1e6 + 0.3] {
        for n in 3..=12 {
            let xs: Vec<f64> = (1..=n).map(f64::from).collect();
            let ys: Vec<f64> = (1..=n).map(|i| f64::from(i * i % 7) + 0.5 * f64::from(i)).collect();
            let r = linest_rows(&ys, &[vec![c; n as usize], xs], true);
            assert_eq!(r[0][1], Some(0.0), "coefficient of the constant {c}, n={n}");
            assert_eq!(r[1][1], Some(0.0), "standard error of the constant {c}, n={n}");
            assert_eq!(r[3][1], Some(f64::from(n) - 2.0), "df with the constant {c}, n={n}");
        }
    }
    // A column that merely sits far from zero is not noise and is kept.
    let off: Vec<f64> = x1.iter().map(|v| 1e9 + v).collect();
    let r = linest_rows(&y, &[off], true);
    rel_close(r[0][0], 0.6, 1e-6, "slope on an offset column");
    assert_eq!(r[3][1], Some(3.0));
    // Distinct values far from zero, whose spread is below 1e-12 of the offset, still vary.
    let r = linest_rows(&[0.0, 1.0, 2.0], &[vec![1e13, 1e13 + 1.0, 1e13 + 2.0]], true);
    rel_close(r[0][0], 1.0, 1e-9, "slope on a predictor with a 1e13 offset");
    assert_eq!(r[3][1], Some(1.0), "df: the predictor is kept");
    // Same at 1e15, where the spread is only a few ulps of the offset: still distinct doubles.
    let x15 = [1e15, 1e15 + 1.0, 1e15 + 2.0];
    let r = linest_rows(&[0.0, 1.0, 2.0], &[x15.to_vec()], true);
    rel_close(r[0][0], 1.0, 1e-9, "slope on a predictor with a 1e15 offset");
    rel_close(r[0][1], -1e15, 1e-9, "intercept on a predictor with a 1e15 offset");
    assert_eq!(r[3][1], Some(1.0), "df: the 1e15 predictor is kept");
    assert!(r[4][1].is_some_and(|v| v.abs() < 1e-9), "ssresid: the 1e15 line is exact, got {:?}", r[4][1]);
    let fitted = rows_of(&ev("TREND", vec![c(&[0.0, 1.0, 2.0]), c(&x15)]));
    close(fitted[2][0].clone(), 2.0);
    // More predictors than observations: the surplus columns drop, no panic.
    let r = linest_rows(&[1.0, 2.0], &[vec![1.0, 2.0], vec![2.0, 1.0], vec![0.0, 5.0]], true);
    assert_eq!(r[0].len(), 4);
}

#[test]
fn logest_is_linest_of_ln_y() {
    // y = 5·2^x: m = 2, b = 5.
    let x = [0.0, 1.0, 2.0, 3.0, 4.0];
    let y: Vec<f64> = x.iter().map(|v| 5.0 * 2f64.powf(*v)).collect();
    let l = rows_of(&ev("LOGEST", vec![c(&y), c(&x)]));
    close_tol(l[0][0].clone(), 2.0, 1e-12);
    close_tol(l[0][1].clone(), 5.0, 1e-12);
    // The statistics are those of the fit on ln y, untransformed.
    let ln: Vec<f64> = y.iter().map(|v| v.ln()).collect();
    let ls = rows_of(&ev("LOGEST", vec![c(&y), c(&x), b(true), b(true)]));
    let ns = rows_of(&ev("LINEST", vec![c(&ln), c(&x), b(true), b(true)]));
    assert_eq!(ls[1..], ns[1..]);
    close_tol(ls[0][0].clone(), ns[0][0].as_f64().unwrap().exp(), 1e-14);
}

// #2177: one predictor row of 16,384 columns built three 16,384² matrices (6 GB) and an O(k³)
// inversion; a wider TOROW aborted the process. It is now refused before allocating.
#[test]
fn linest_family_caps_regressor_count() {
    let wide = |k: usize| av(arr(vec![(0..k).map(|i| nv(i as f64)).collect()]));
    let tall = |k: usize| av(arr((0..k).map(|i| vec![nv(i as f64), nv(1.0)]).collect()));
    for f in ["LINEST", "LOGEST", "TREND", "GROWTH"] {
        // y is one cell, so every column of x is a predictor.
        is_err(ev(f, vec![n(1.0), wide(16_384)]), CellError::Value);
        // y is a row, so every row of x is a predictor.
        is_err(ev(f, vec![av(arr(vec![vec![nv(1.0), nv(2.0)]])), tall(16_384)]), CellError::Value);
    }
    is_err(ev("LINEST", vec![n(1.0), wide(MAX_REGRESSORS + 1)]), CellError::Value);
    // At the cap the fit still runs (one observation: coefficients are zero, not an error).
    assert_eq!(rows_of(&ev("LINEST", vec![n(1.0), wide(MAX_REGRESSORS)]))[0].len(), MAX_REGRESSORS + 1);
    // The rows × k² work bound.
    assert_eq!(check_regression_size(1_000, MAX_REGRESSORS), Ok(()));
    assert_eq!(check_regression_size(1_000_000, 100), Err(CellError::Num));
    assert_eq!(check_regression_size(usize::MAX, 2), Err(CellError::Num));
    assert_eq!(check_regression_size(10_000_000, 1), Ok(()));
}

#[test]
fn frequency_prob() {
    let f = ev("FREQUENCY", vec![c(&[79.0, 85.0, 78.0, 85.0, 50.0, 81.0, 95.0, 88.0, 97.0]), c(&[70.0, 79.0, 89.0])]);
    assert_eq!(rows_of(&f), vec![vec![nv(1.0)], vec![nv(2.0)], vec![nv(4.0)], vec![nv(2.0)]]);
    let x = c(&[0.0, 1.0, 2.0, 3.0]);
    let p = c(&[0.2, 0.3, 0.1, 0.4]);
    close(ev("PROB", vec![x.clone(), p.clone(), n(2.0)]), 0.1);
    close(ev("PROB", vec![x.clone(), p, n(1.0), n(3.0)]), 0.8);
    is_err(ev("PROB", vec![x.clone(), c(&[0.5, 0.5, 0.5, 0.5]), n(1.0)]), CellError::Num);
    // Excel: any probability <= 0 is #NUM!, even when it sums to one and lies outside the limits.
    is_err(ev("PROB", vec![x.clone(), c(&[0.5, 0.0, 0.25, 0.25]), n(0.0), n(3.0)]), CellError::Num);
    is_err(ev("PROB", vec![x, c(&[0.5, 0.0, 0.25, 0.25]), n(2.0), n(3.0)]), CellError::Num);
}

#[test]
fn normal() {
    close_tol(ev("NORM.DIST", vec![n(42.0), n(40.0), n(1.5), b(true)]), 0.9087888, D7);
    close_tol(ev("NORM.DIST", vec![n(42.0), n(40.0), n(1.5), b(false)]), 0.10934005, D7);
    close_tol(ev("NORMDIST", vec![n(42.0), n(40.0), n(1.5), b(true)]), 0.9087888, D7);
    is_err(ev("NORM.DIST", vec![n(42.0), n(40.0), n(0.0), b(true)]), CellError::Num);
    close_tol(ev("NORM.INV", vec![n(0.908789), n(40.0), n(1.5)]), 42.000002, D7);
    close_tol(ev("NORMINV", vec![n(0.908789), n(40.0), n(1.5)]), 42.000002, D7);
    is_err(ev("NORM.INV", vec![n(1.0), n(0.0), n(1.0)]), CellError::Num);
    close_tol(ev("NORM.S.DIST", vec![n(1.333333), b(true)]), 0.908788726, 1e-8);
    close_tol(ev("NORM.S.DIST", vec![n(0.0), b(false)]), 0.398942280, 1e-8);
    close_tol(ev("NORMSDIST", vec![n(1.333333)]), 0.908788726, 1e-8);
    close_tol(ev("NORM.S.INV", vec![n(0.908789)]), 1.3333347, D7);
    close_tol(ev("NORMSINV", vec![n(0.975)]), 1.959963985, 1e-9);
    close_tol(ev("PHI", vec![n(0.75)]), 0.301137432, 1e-8);
    close_tol(ev("GAUSS", vec![n(2.0)]), 0.477249868, 1e-8);
    close_tol(ev("FISHER", vec![n(0.75)]), 0.972955075, 1e-8);
    close_tol(ev("FISHERINV", vec![n(0.972955)]), 0.75, D7);
    is_err(ev("FISHER", vec![n(1.0)]), CellError::Num);
    close_tol(ev("CONFIDENCE.NORM", vec![n(0.05), n(2.5), n(50.0)]), 0.692951912, 1e-8);
    close_tol(ev("CONFIDENCE", vec![n(0.05), n(2.5), n(50.0)]), 0.692951912, 1e-8);
    close_tol(ev("CONFIDENCE.T", vec![n(0.05), n(1.0), n(50.0)]), 0.284196855, 1e-8);
    is_err(ev("CONFIDENCE.T", vec![n(0.05), n(1.0), n(1.0)]), CellError::Div0);
    let z = c(&[3.0, 6.0, 7.0, 8.0, 6.0, 5.0, 4.0, 2.0, 1.0, 9.0]);
    close_tol(ev("Z.TEST", vec![z.clone(), n(4.0)]), 0.090574, 1e-5);
    close_tol(ev("ZTEST", vec![z, n(6.0)]), 0.863043, 1e-5);
    close_tol(ev("LOGNORM.DIST", vec![n(4.0), n(3.5), n(1.2), b(true)]), 0.0390836, D7);
    close_tol(ev("LOGNORM.DIST", vec![n(4.0), n(3.5), n(1.2), b(false)]), 0.0176176, D7);
    close_tol(ev("LOGNORMDIST", vec![n(4.0), n(3.5), n(1.2)]), 0.0390836, D7);
    close_tol(ev("LOGNORM.INV", vec![n(0.039084), n(3.5), n(1.2)]), 4.0000252, D7);
    close_tol(ev("LOGINV", vec![n(0.039084), n(3.5), n(1.2)]), 4.0000252, D7);
}

#[test]
fn t_chi_f() {
    close_tol(ev("T.DIST", vec![n(0.0), n(6e307), b(false)]), 0.3989422804, 1e-10);
    close_tol(ev("T.DIST", vec![n(60.0), n(1.0), b(true)]), 0.99469533, D7);
    close_tol(ev("T.DIST", vec![n(8.0), n(3.0), b(false)]), 0.00073691, 1e-5);
    close_tol(ev("T.DIST.2T", vec![n(1.959999998), n(60.0)]), 0.054644930, D7);
    close_tol(ev("T.DIST.RT", vec![n(1.959999998), n(60.0)]), 0.027322465, D7);
    close_tol(ev("TDIST", vec![n(1.959999998), n(60.0), n(2.0)]), 0.054644930, D7);
    is_err(ev("TDIST", vec![n(-1.0), n(60.0), n(2.0)]), CellError::Num);
    is_err(ev("T.DIST", vec![n(1.0), n(0.0), b(true)]), CellError::Num);
    close_tol(ev("T.INV", vec![n(0.75), n(2.0)]), 0.8164966, D7);
    close_tol(ev("T.INV", vec![n(0.25), n(2.0)]), -0.8164966, D7);
    close_tol(ev("T.INV.2T", vec![n(0.546449), n(60.0)]), 0.606533, D7);
    close_tol(ev("T.INV.2T", vec![n(0.05), n(10.0)]), 2.228138852, 1e-9);
    close_tol(ev("TINV", vec![n(0.05), n(10.0)]), 2.228138852, 1e-9);
    let a1 = c(&[3.0, 4.0, 5.0, 8.0, 9.0, 1.0, 2.0, 4.0, 5.0]);
    let a2 = c(&[6.0, 19.0, 3.0, 2.0, 14.0, 4.0, 5.0, 17.0, 1.0]);
    close_tol(ev("T.TEST", vec![a1.clone(), a2.clone(), n(2.0), n(1.0)]), 0.196016, 1e-5);
    close_tol(ev("TTEST", vec![a1.clone(), a2.clone(), n(2.0), n(1.0)]), 0.196016, 1e-5);
    close_tol(ev("T.TEST", vec![a1.clone(), a2.clone(), n(2.0), n(2.0)]), 0.191995887, 1e-6);
    close_tol(ev("T.TEST", vec![a1.clone(), a2.clone(), n(1.0), n(3.0)]), 0.1012, 1e-3);
    is_err(ev("T.TEST", vec![a1, a2, n(3.0), n(1.0)]), CellError::Num);
    close_tol(ev("CHISQ.DIST", vec![n(0.5), n(1.0), b(true)]), 0.52049988, D7);
    close_tol(ev("CHISQ.DIST", vec![n(2.0), n(3.0), b(false)]), 0.20755375, D7);
    close_tol(ev("CHISQ.DIST.RT", vec![n(18.307), n(10.0)]), 0.0500006, 1e-5);
    close_tol(ev("CHIDIST", vec![n(18.307), n(10.0)]), 0.0500006, 1e-5);
    close_tol(ev("CHISQ.INV", vec![n(0.93), n(1.0)]), 3.283020287, 1e-8);
    close_tol(ev("CHISQ.INV", vec![n(0.6), n(2.0)]), 1.832581464, 1e-8);
    close_tol(ev("CHISQ.INV.RT", vec![n(0.050001), n(10.0)]), 18.30697, D7);
    close_tol(ev("CHIINV", vec![n(0.050001), n(10.0)]), 18.30697, D7);
    is_err(ev("CHISQ.DIST", vec![n(-1.0), n(1.0), b(true)]), CellError::Num);
    let act = arr(vec![vec![nv(58.0), nv(35.0)], vec![nv(11.0), nv(25.0)], vec![nv(10.0), nv(23.0)]]);
    let exp = arr(vec![vec![nv(45.35), nv(47.65)], vec![nv(17.56), nv(18.44)], vec![nv(16.09), nv(16.91)]]);
    close_tol(ev("CHISQ.TEST", vec![av(act.clone()), av(exp.clone())]), 0.0003082, 1e-3);
    close_tol(ev("CHITEST", vec![av(act), av(exp)]), 0.0003082, 1e-3);
    close_tol(ev("F.DIST", vec![n(15.2069), n(6.0), n(4.0), b(true)]), 0.99, 1e-6);
    close_tol(ev("F.DIST", vec![n(15.2069), n(6.0), n(4.0), b(false)]), 0.0012238, 1e-5);
    close_tol(ev("F.DIST.RT", vec![n(15.2069), n(6.0), n(4.0)]), 0.01, 1e-6);
    close_tol(ev("FDIST", vec![n(15.2069), n(6.0), n(4.0)]), 0.01, 1e-6);
    close_tol(ev("F.INV", vec![n(0.01), n(6.0), n(4.0)]), 0.10930991, D7);
    close_tol(ev("F.INV", vec![n(0.99), n(6.0), n(4.0)]), 15.20686, D7);
    close_tol(ev("F.INV.RT", vec![n(0.01), n(6.0), n(4.0)]), 15.20686, D7);
    close_tol(ev("FINV", vec![n(0.01), n(6.0), n(4.0)]), 15.20686, D7);
    close_tol(ev("F.TEST", vec![c(&[6.0, 7.0, 9.0, 15.0, 21.0]), c(&[20.0, 28.0, 31.0, 38.0, 40.0])]), 0.64831785, D7);
    close_tol(ev("FTEST", vec![c(&[6.0, 7.0, 9.0, 15.0, 21.0]), c(&[20.0, 28.0, 31.0, 38.0, 40.0])]), 0.64831785, D7);
    is_err(ev("F.DIST", vec![n(1.0), n(0.0), n(4.0), b(true)]), CellError::Num);
}

#[test]
fn f_distributions_accept_ten_billion_degrees() {
    close_tol(ev("F.DIST", vec![n(0.0), n(10_000_000_000.0), n(1.0), b(false)]), 0.0, 0.0);
    close_tol(ev("F.DIST.RT", vec![n(0.0), n(1.0), n(10_000_000_000.0)]), 1.0, 0.0);
    close_tol(ev("F.DIST", vec![n(0.01), n(1_000_000_000.0), n(1.0), b(true)]), 1.52397e-23, 1e-28);
    close_tol(ev("F.DIST", vec![n(1.0), n(10_000_000_000.0), n(10_000_000_000.0), b(true)]), 0.5, 0.0);
    close_tol(ev("F.DIST", vec![n(1_000_000.0), n(10_000_000_000.0), n(1.0), b(true)]), 0.999202115, 1e-8);
    close_tol(ev("F.INV", vec![n(0.5), n(10_000_000_000.0), n(1.0)]), 2.198110953542527, 2e-6);
    close_tol(ev("F.DIST.RT", vec![n(0.0), n(9_999_999_999.0), n(1.0)]), 1.0, 0.0);
    is_err(ev("F.DIST.RT", vec![n(0.0), n(10_000_000_001.0), n(1.0)]), CellError::Num);
    is_err(ev("F.DIST.RT", vec![n(0.0), n(1.0), n(10_000_000_001.0)]), CellError::Num);
}

#[test]
fn t_dist_zero_degrees_error_depends_on_mode() {
    for df in [0.0, 0.9, -0.1] {
        is_err(ev("T.DIST", vec![n(0.0), n(df), b(false)]), CellError::Div0);
        is_err(ev("T.DIST", vec![n(0.0), n(df), b(true)]), CellError::Num);
    }
    is_err(ev("T.DIST", vec![n(0.0), n(-1.0), b(false)]), CellError::Num);
    close_tol(ev("T.DIST", vec![n(0.0), n(1.99), b(false)]), 1.0 / std::f64::consts::PI, 1e-15);
    close_tol(ev("T.DIST", vec![n(0.0), n(1.0), b(false)]), 1.0 / std::f64::consts::PI, 1e-15);
}

#[test]
fn t_density_is_stable_for_large_degrees() {
    close_tol(ev("T.DIST", vec![n(0.0), n(100_000_000.0), b(false)]), 0.398942279404077, 1e-12);
    close_tol(ev("T.DIST", vec![n(0.0), n(10_000_000_000.0), b(false)]), 0.3989422803914591, 1e-12);
    close_tol(ev("T.DIST", vec![n(1.0), n(10_000_000_000_000_000.0), b(false)]), 0.24197072451914337, 1e-12);
}

#[test]
fn t_inv_zero_probability_returns_div0() {
    is_err(ev("T.INV", vec![n(-0.1), n(1.0)]), CellError::Num);
    is_err(ev("T.INV", vec![n(0.0), n(1.0)]), CellError::Div0);
    close_tol(ev("T.INV", vec![n(0.75), n(2.0)]), 0.8164966, D7);
    is_err(ev("T.INV", vec![n(1.0), n(1.0)]), CellError::Num);
    is_err(ev("T.INV", vec![n(1.1), n(1.0)]), CellError::Num);
    is_err(ev("T.INV", vec![n(0.0), n(0.0)]), CellError::Num);
    is_err(ev("T.INV.2T", vec![n(0.0), n(1.0)]), CellError::Num);
}

#[test]
fn binom_inv_huge_trials_returns() {
    // Above 2^53 neighbouring f64 values are more than 1 apart; the bisection must still end.
    let cases: [(&str, f64, f64, f64); 3] = [("BINOM.INV", 1e17, 0.5, 0.5), ("CRITBINOM", 1e300, 1.0, 0.5), ("BINOM.INV", 1e300, 0.5, 0.5)];
    for (name, trials, p, alpha) in cases {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = ev(name, vec![n(trials), n(p), n(alpha)]);
            let _ = tx.send(r);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Value::Number(_)) | Ok(Value::Error(CellError::Num)) => {}
            Ok(other) => panic!("{name}({trials},{p},{alpha}) returned {other:?}"),
            Err(_) => panic!("{name}({trials},{p},{alpha}) did not return within 5 s"),
        }
    }
}

#[test]
fn discrete() {
    close_tol(ev("BINOM.DIST", vec![n(6.0), n(10.0), n(0.5), b(false)]), 0.205078125, 1e-12);
    close_tol(ev("BINOM.DIST", vec![n(6.0), n(10.0), n(0.5), b(true)]), 0.828125, 1e-12);
    close_tol(ev("BINOMDIST", vec![n(6.0), n(10.0), n(0.5), b(false)]), 0.205078125, 1e-12);
    close(ev("BINOM.DIST", vec![n(1.0), n(2_147_483_646.0), n(0.0), b(false)]), 0.0);
    close(ev("BINOM.DIST", vec![n(1.0), n(2_147_483_646.9), n(0.0), b(false)]), 0.0);
    is_err(ev("BINOM.DIST", vec![n(1.0), n(2_147_483_647.0), n(0.0), b(false)]), CellError::Num);
    is_err(ev("BINOM.DIST", vec![n(1.0), n(2_147_483_647.9), n(0.0), b(false)]), CellError::Num);
    is_err(ev("BINOM.DIST", vec![n(11.0), n(10.0), n(0.5), b(false)]), CellError::Num);
    close(ev("BINOM.DIST.RANGE", vec![n(2_147_483_646.0), n(0.0), n(1.0)]), 0.0);
    close(ev("BINOM.DIST.RANGE", vec![n(2_147_483_646.9), n(0.0), n(1.0)]), 0.0);
    is_err(ev("BINOM.DIST.RANGE", vec![n(2_147_483_647.0), n(0.0), n(1.0)]), CellError::Num);
    is_err(ev("BINOM.DIST.RANGE", vec![n(2_147_483_647.9), n(0.0), n(1.0)]), CellError::Num);
    close_tol(ev("BINOM.DIST.RANGE", vec![n(60.0), n(0.75), n(48.0)]), 0.083974967, 1e-6);
    close_tol(ev("BINOM.DIST.RANGE", vec![n(60.0), n(0.75), n(45.0), n(50.0)]), 0.523629793, 1e-6);
    close(ev("BINOM.INV", vec![n(6.0), n(0.5), n(0.75)]), 4.0);
    close(ev("CRITBINOM", vec![n(6.0), n(0.5), n(0.75)]), 4.0);
    close(ev("BINOM.INV", vec![n(1e9), n(0.5), n(0.5)]), 5e8);
    close_tol(ev("POISSON.DIST", vec![n(2.0), n(5.0), b(true)]), 0.124652, 1e-5);
    close_tol(ev("POISSON.DIST", vec![n(2.0), n(5.0), b(false)]), 0.084224, 1e-5);
    close_tol(ev("POISSON", vec![n(2.0), n(5.0), b(false)]), 0.084224, 1e-5);
    is_err(ev("POISSON.DIST", vec![n(-1.0), n(5.0), b(false)]), CellError::Num);
    close_tol(ev("HYPGEOM.DIST", vec![n(1.0), n(4.0), n(8.0), n(20.0), b(true)]), 0.4654, 1e-4);
    close_tol(ev("HYPGEOM.DIST", vec![n(1.0), n(4.0), n(8.0), n(20.0), b(false)]), 0.3633, 1e-4);
    close_tol(ev("HYPGEOMDIST", vec![n(1.0), n(4.0), n(8.0), n(20.0)]), 0.363261094, 1e-8);
    is_err(ev("HYPGEOMDIST", vec![n(5.0), n(4.0), n(8.0), n(20.0)]), CellError::Num);
    close_tol(ev("NEGBINOM.DIST", vec![n(10.0), n(5.0), n(0.25), b(true)]), 0.3135141, D7);
    close_tol(ev("NEGBINOM.DIST", vec![n(10.0), n(5.0), n(0.25), b(false)]), 0.0550487, D7);
    close_tol(ev("NEGBINOMDIST", vec![n(10.0), n(5.0), n(0.25)]), 0.0550487, D7);
}

#[test]
fn continuous() {
    close_tol(ev("EXPON.DIST", vec![n(0.2), n(10.0), b(true)]), 0.86466472, D7);
    close_tol(ev("EXPON.DIST", vec![n(0.2), n(10.0), b(false)]), 1.35335283, D7);
    close_tol(ev("EXPONDIST", vec![n(0.2), n(10.0), b(true)]), 0.86466472, D7);
    is_err(ev("EXPON.DIST", vec![n(0.2), n(0.0), b(true)]), CellError::Num);
    close_tol(ev("GAMMA.DIST", vec![n(10.00001131), n(9.0), n(2.0), b(false)]), 0.032639, 1e-5);
    close_tol(ev("GAMMA.DIST", vec![n(10.00001131), n(9.0), n(2.0), b(true)]), 0.068094, 1e-5);
    close_tol(ev("GAMMADIST", vec![n(10.00001131), n(9.0), n(2.0), b(true)]), 0.068094, 1e-5);
    close_tol(ev("GAMMA.INV", vec![n(0.068094), n(9.0), n(2.0)]), 10.0000112, 1e-6);
    close_tol(ev("GAMMAINV", vec![n(0.068094), n(9.0), n(2.0)]), 10.0000112, 1e-6);
    close_tol(ev("GAMMALN", vec![n(4.0)]), 1.791759469, 1e-9);
    close_tol(ev("GAMMALN.PRECISE", vec![n(4.5)]), 2.453736571, 1e-9);
    is_err(ev("GAMMALN", vec![n(0.0)]), CellError::Num);
    close_tol(ev("GAMMA", vec![n(2.5)]), 1.329340388, 1e-9);
    close_tol(ev("GAMMA", vec![n(-0.75)]), -4.834146544, 1e-8);
    is_err(ev("GAMMA", vec![n(-1.0)]), CellError::Num);
    is_err(ev("GAMMA", vec![n(0.0)]), CellError::Num);
    close_tol(ev("BETA.DIST", vec![n(2.0), n(8.0), n(10.0), b(true), n(1.0), n(3.0)]), 0.6854706, D7);
    close_tol(ev("BETA.DIST", vec![n(2.0), n(8.0), n(10.0), b(false), n(1.0), n(3.0)]), 1.4837646, D7);
    is_err(ev("BETA.DIST", vec![n(0.0), n(1.0), n(1.0), b(false)]), CellError::Num);
    is_err(ev("BETA.DIST", vec![n(1.0), n(1.0), n(1.0), b(false)]), CellError::Num);
    is_err(ev("BETA.DIST", vec![n(0.0), n(0.5), n(2.0), b(false)]), CellError::Num);
    is_err(ev("BETA.DIST", vec![n(1.0), n(2.0), n(0.5), b(false)]), CellError::Num);
    close(ev("BETA.DIST", vec![n(0.0), n(2.0), n(1.0), b(false)]), 0.0);
    close(ev("BETA.DIST", vec![n(1.0), n(1.0), n(2.0), b(false)]), 0.0);
    close(ev("BETA.DIST", vec![n(0.0), n(1.0), n(1.0), b(true)]), 0.0);
    close(ev("BETA.DIST", vec![n(1.0), n(1.0), n(1.0), b(true)]), 1.0);
    is_err(ev("BETA.DIST", vec![n(2.0), n(1.0), n(2.0), b(false), n(2.0), n(6.0)]), CellError::Num);
    is_err(ev("BETA.DIST", vec![n(6.0), n(2.0), n(1.0), b(false), n(2.0), n(6.0)]), CellError::Num);
    close(ev("BETA.DIST", vec![n(4.0), n(1.0), n(1.0), b(false), n(2.0), n(6.0)]), 0.25);
    close(ev("BETA.DIST", vec![n(2.0), n(1.0), n(1.0), b(true), n(2.0), n(6.0)]), 0.0);
    close(ev("BETA.DIST", vec![n(6.0), n(1.0), n(1.0), b(true), n(2.0), n(6.0)]), 1.0);
    close_tol(ev("BETADIST", vec![n(2.0), n(8.0), n(10.0), n(1.0), n(3.0)]), 0.6854706, D7);
    close_tol(ev("BETA.INV", vec![n(0.685470581), n(8.0), n(10.0), n(1.0), n(3.0)]), 2.0, D7);
    close_tol(ev("BETAINV", vec![n(0.685470581), n(8.0), n(10.0), n(1.0), n(3.0)]), 2.0, D7);
    is_err(ev("BETA.DIST", vec![n(4.0), n(8.0), n(10.0), b(true), n(1.0), n(3.0)]), CellError::Num);
    close_tol(ev("WEIBULL.DIST", vec![n(105.0), n(20.0), n(100.0), b(true)]), 0.929581, 1e-5);
    close_tol(ev("WEIBULL.DIST", vec![n(105.0), n(20.0), n(100.0), b(false)]), 0.035589, 1e-4);
    close_tol(ev("WEIBULL", vec![n(105.0), n(20.0), n(100.0), b(true)]), 0.929581, 1e-5);
    // Lifting over an array of x values.
    let r = rows_of(&ev("NORM.S.DIST", vec![av(row(&[0.0, 1.0])), b(true)]));
    close(r[0][0].clone(), 0.5);
    close_tol(r[0][1].clone(), 0.841344746, 1e-8);
}

#[test]
fn gamma_density_at_zero_shape_one_is_num() {
    for scale in [0.1, 0.9, 1.0, 1.9, 2.0, 10_000_000_000.0] {
        is_err(ev("GAMMA.DIST", vec![n(0.0), n(1.0), n(scale), b(false)]), CellError::Num);
    }
    is_err(ev("GAMMA.DIST", vec![n(0.0), n(0.5), n(1.0), b(false)]), CellError::Num);
    close_tol(ev("GAMMA.DIST", vec![n(0.0), n(2.0), n(1.0), b(false)]), 0.0, 0.0);
    close_tol(ev("GAMMA.DIST", vec![n(0.0), n(1.0), n(2.0), b(true)]), 0.0, 0.0);
    close_tol(ev("GAMMA.DIST", vec![n(2.0), n(1.0), n(2.0), b(false)]), 0.5 * (-1.0f64).exp(), 1e-15);
    close_tol(ev("GAMMA.DIST", vec![n(2.0), n(1.0), n(2.0), b(true)]), 1.0 - (-1.0f64).exp(), 1e-15);
    is_err(ev("GAMMA.DIST", vec![n(-0.1), n(1.0), n(2.0), b(false)]), CellError::Num);
}

#[test]
fn weibull_density_at_zero_is_zero() {
    for shape in [0.5, 1.0, 2.0] {
        close_tol(ev("WEIBULL.DIST", vec![n(0.0), n(shape), n(1.0), b(false)]), 0.0, 0.0);
        close_tol(ev("WEIBULL.DIST", vec![n(0.0), n(shape), n(1.0), b(true)]), 0.0, 0.0);
    }
    close_tol(ev("WEIBULL.DIST", vec![n(1.0), n(1.0), n(1.0), b(false)]), (-1.0f64).exp(), 1e-15);
    close_tol(ev("WEIBULL.DIST", vec![n(1.0), n(1.0), n(1.0), b(true)]), 1.0 - (-1.0f64).exp(), 1e-15);
    is_err(ev("WEIBULL.DIST", vec![n(-0.1), n(1.0), n(1.0), b(false)]), CellError::Num);
}

#[test]
fn aggregate_helper() {
    let vals = vec![nv(1.0), nv(2.0), tv("x"), Value::Empty, Value::Bool(true), nv(6.0)];
    close(aggregate_values(1, &vals), 3.0);
    close(aggregate_values(2, &vals), 3.0);
    close(aggregate_values(3, &vals), 5.0);
    close(aggregate_values(4, &vals), 6.0);
    close(aggregate_values(105, &vals), 1.0);
    close(aggregate_values(6, &vals), 12.0);
    close(aggregate_values(9, &vals), 9.0);
    close(aggregate_values(109, &vals), 9.0);
    close(aggregate_values(11, &vals), 14.0 / 3.0);
    close(aggregate_values(10, &vals), 7.0);
    close(aggregate_values(12, &vals), 2.0);
    is_err(aggregate_values(13, &vals), CellError::NA);
    is_err(aggregate_values(14, &vals), CellError::Value);
    is_err(aggregate_values(99, &vals), CellError::Value);
    is_err(aggregate_values(9, &[nv(1.0), Value::Error(CellError::Div0)]), CellError::Div0);
    close(aggregate_values_k(14, &vals, &nv(1.0)), 6.0);
    close(aggregate_values_k(15, &vals, &nv(2.0)), 2.0);
    close(aggregate_values_k(16, &vals, &nv(0.5)), 2.0);
    close(aggregate_values_k(17, &vals, &nv(4.0)), 6.0);
    close(aggregate_values_k(18, &vals, &nv(0.5)), 2.0);
    close(aggregate_values_k(19, &vals, &nv(2.0)), 2.0);
    close(aggregate_values_k(9, &vals, &nv(2.0)), 9.0);
    is_err(aggregate_values_k(14, &vals, &nv(10.0)), CellError::Num);
}

// B1: BINOM.DIST and POISSON.DIST went through ln_gamma differences (relative error 1.1e-13 at
// n = 100, 1.5e-11 at n = 10,000). Reference values: 70-digit Decimal arithmetic on the exact
// double `p` (mass by the neighbour-ratio recurrence from q^n, cdf as the running sum), rounded
// once to f64. Columns: k, n, p, mass, cdf.
#[rustfmt::skip]
const BINOM_REF: &[(f64, f64, f64, f64, f64)] = &[
    (0.0, 10.0, 0.5, 0.0009765625, 0.0009765625),
    (1.0, 10.0, 0.5, 0.009765625, 0.0107421875),
    (3.0, 10.0, 0.5, 0.1171875, 0.171875),
    (5.0, 10.0, 0.5, 0.24609375, 0.623046875),
    (7.0, 10.0, 0.5, 0.1171875, 0.9453125),
    (9.0, 10.0, 0.5, 0.009765625, 0.9990234375),
    (0.0, 10.0, 0.3, 0.028247524900000005, 0.028247524900000005),
    (1.0, 10.0, 0.3, 0.12106082100000001, 0.14930834590000003),
    (2.0, 10.0, 0.3, 0.23347444050000002, 0.3827827864),
    (3.0, 10.0, 0.3, 0.266827932, 0.6496107184000001),
    (4.0, 10.0, 0.3, 0.200120949, 0.8497316674000001),
    (5.0, 10.0, 0.3, 0.10291934519999998, 0.9526510126),
    (7.0, 10.0, 0.3, 0.009001691999999999, 0.9984096136),
    (9.0, 10.0, 0.3, 0.00013778099999999996, 0.9999940951),
    (0.0, 10.0, 0.999, 1.0000000000000088e-30, 1.0000000000000088e-30),
    (1.0, 10.0, 0.999, 9.99000000000008e-27, 9.99100000000008e-27),
    (5.0, 10.0, 0.999, 2.507425174812609e-13, 2.50951798425701e-13),
    (9.0, 10.0, 0.999, 0.00991035916125875, 0.009955119790251798),
    (0.0, 100.0, 0.5, 7.888609052210118e-31, 7.888609052210118e-31),
    (1.0, 100.0, 0.5, 7.888609052210118e-29, 7.967495142732219e-29),
    (35.0, 100.0, 0.5, 0.0008638556657416528, 0.0017588208614850792),
    (45.0, 100.0, 0.5, 0.048474296626430755, 0.18410080866334813),
    (50.0, 100.0, 0.5, 0.07958923738717877, 0.5397946186935894),
    (55.0, 100.0, 0.5, 0.048474296626430755, 0.8643734879630827),
    (65.0, 100.0, 0.5, 0.0008638556657416528, 0.9991050348042566),
    (99.0, 100.0, 0.5, 7.888609052210118e-29, 1.0),
    (0.0, 100.0, 0.01, 0.3660323412732295, 0.3660323412732295),
    (1.0, 100.0, 0.01, 0.3697296376497268, 0.7357619789229563),
    (2.0, 100.0, 0.01, 0.1848648188248634, 0.9206267977478196),
    (4.0, 100.0, 0.01, 0.014941714856895142, 0.9965676784122455),
    (50.0, 100.0, 0.01, 6.103987557173001e-72, 1.0),
    (99.0, 100.0, 0.01, 9.900000000000021e-197, 1.0),
    (0.0, 100.0, 0.7, 5.15377520732019e-53, 5.15377520732019e-53),
    (1.0, 100.0, 0.7, 1.2025475483747105e-50, 1.207701323582031e-50),
    (50.0, 100.0, 0.7, 1.302622713144541e-05, 2.2060913327166086e-05),
    (56.0, 100.0, 0.7, 0.001028871216633018, 0.0021146172813184595),
    (65.0, 100.0, 0.7, 0.04677968235272982, 0.16285828837178737),
    (70.0, 100.0, 0.7, 0.08678386475342809, 0.5376602639846381),
    (75.0, 100.0, 0.7, 0.04955992276217012, 0.8864298182958186),
    (84.0, 100.0, 0.7, 0.0005638655029474704, 0.9995950004580563),
    (99.0, 100.0, 0.7, 1.3862042184106021e-14, 0.9999999999999997),
    (0.0, 1000.0, 0.5, 9.332636185032189e-302, 9.332636185032189e-302),
    (1.0, 1000.0, 0.5, 9.332636185032189e-299, 9.341968821217221e-299),
    (453.0, 1000.0, 0.5, 0.00030353498700958767, 0.00162629990658169),
    (484.0, 1000.0, 0.5, 0.015123665829158384, 0.1634683412157977),
    (500.0, 1000.0, 0.5, 0.0252250181783608, 0.5126125090891804),
    (516.0, 1000.0, 0.5, 0.015123665829158384, 0.8516553246133607),
    (547.0, 1000.0, 0.5, 0.00030353498700958767, 0.9986772350804279),
    (999.0, 1000.0, 0.5, 9.332636185032189e-299, 1.0),
    (0.0, 1000.0, 0.3, 1.2532566399657381e-155, 1.2532566399657381e-155),
    (1.0, 1000.0, 0.3, 5.371099885567449e-153, 5.383632451967106e-153),
    (257.0, 1000.0, 0.3, 0.00030959977077928017, 0.0014835731103312242),
    (286.0, 1000.0, 0.3, 0.0174263984921581, 0.1759288949498673),
    (300.0, 1000.0, 0.3, 0.027521003821268385, 0.5155935198141203),
    (314.0, 1000.0, 0.3, 0.017109129819617666, 0.8414952343056957),
    (343.0, 1000.0, 0.3, 0.00036310112605077133, 0.9985016810663147),
    (500.0, 1000.0, 0.3, 3.4791609391626546e-40, 1.0),
    (999.0, 1000.0, 0.3, 0.0, 1.0),
    (0.0, 1000.0, 0.001, 0.36769542477096406, 0.36769542477096406),
    (1.0, 1000.0, 0.001, 0.36806348825922325, 0.7357589130301873),
    (2.0, 1000.0, 0.001, 0.18403174412961162, 0.9197906571597989),
    (4.0, 1000.0, 0.001, 0.01528995542083862, 0.9963631219704782),
    (500.0, 1000.0, 0.001, 0.0, 1.0),
    (999.0, 1000.0, 0.001, 0.0, 1.0),
    (0.0, 10000.0, 0.5, 0.0, 0.0),
    (1.0, 10000.0, 0.5, 0.0, 0.0),
    (4850.0, 10000.0, 0.5, 8.861480551702583e-05, 0.0013939837588563594),
    (4950.0, 10000.0, 0.5, 0.004839495142316416, 0.161087099897656),
    (5000.0, 10000.0, 0.5, 0.007978646139382154, 0.5039893230696911),
    (5050.0, 10000.0, 0.5, 0.004839495142316416, 0.8437523952446604),
    (5150.0, 10000.0, 0.5, 8.861480551702583e-05, 0.9986946310466607),
    (9999.0, 10000.0, 0.5, 0.0, 1.0),
    (0.0, 10000.0, 0.3, 0.0, 0.0),
    (1.0, 10000.0, 0.3, 0.0, 0.0),
    (2863.0, 10000.0, 0.3, 9.71726742023389e-05, 0.0013927715709303387),
    (2954.0, 10000.0, 0.3, 0.0052756073908582085, 0.16038574844431697),
    (3000.0, 10000.0, 0.3, 0.008705361365067056, 0.504932983819298),
    (3046.0, 10000.0, 0.3, 0.005244996814931252, 0.8448693061622872),
    (3137.0, 10000.0, 0.3, 0.00010232641238302388, 0.9986013755801396),
    (5000.0, 10000.0, 0.3, 0.0, 1.0),
    (9999.0, 10000.0, 0.3, 0.0, 1.0),
    (0.0, 10000.0, 0.0001, 0.3678610464329299, 0.3678610464329299),
    (1.0, 10000.0, 0.0001, 0.3678978362165516, 0.7357588826494815),
    (2.0, 10000.0, 0.0001, 0.1839489181082758, 0.9197078007577573),
    (4.0, 10000.0, 0.0001, 0.015324477632794846, 0.996342452182816),
    (5000.0, 10000.0, 0.0001, 0.0, 1.0),
    (9999.0, 10000.0, 0.0001, 0.0, 1.0),
    (0.0, 10000.0, 0.999, 0.0, 0.0),
    (1.0, 10000.0, 0.999, 0.0, 0.0),
    (5000.0, 10000.0, 0.999, 0.0, 0.0),
    (9981.0, 10000.0, 0.999, 0.0037205945263045544, 0.007158161342006954),
    (9987.0, 10000.0, 0.999, 0.07292252318084555, 0.20834867055478343),
    (9990.0, 10000.0, 0.999, 0.12517263665023892, 0.5421328760209826),
    (9993.0, 10000.0, 0.999, 0.09007019224850273, 0.8699847268232276),
    (9999.0, 10000.0, 0.999, 0.00045218564541589875, 0.9999548266540229),
    (0.0, 100000.0, 0.5, 0.0, 0.0),
    (1.0, 100000.0, 0.5, 0.0, 0.0),
    (49526.0, 100000.0, 0.5, 2.821098952769392e-05, 0.00137358872189177),
    (49842.0, 100000.0, 0.5, 0.0015314620321580088, 0.15959653953914368),
    (50000.0, 100000.0, 0.5, 0.00252312621419674, 0.5012615631070984),
    (50158.0, 100000.0, 0.5, 0.0015314620321580088, 0.8419349224930143),
    (50474.0, 100000.0, 0.5, 2.821098952769392e-05, 0.9986546222676359),
    (99999.0, 100000.0, 0.5, 0.0, 1.0),
    (0.0, 100000.0, 0.3, 0.0, 0.0),
    (1.0, 100000.0, 0.3, 0.0, 0.0),
    (29565.0, 100000.0, 0.3, 3.016598258904529e-05, 0.0013408650955754554),
    (29855.0, 100000.0, 0.3, 0.0016703029966719045, 0.15934769308464344),
    (30000.0, 100000.0, 0.3, 0.002752954648397428, 0.5015600059172672),
    (30145.0, 100000.0, 0.3, 0.0016672321665529399, 0.8423208126454166),
    (30435.0, 100000.0, 0.3, 3.067097885268188e-05, 0.9986570625478413),
    (50000.0, 100000.0, 0.3, 0.0, 1.0),
    (99999.0, 100000.0, 0.3, 0.0, 1.0),
    (0.0, 100000.0, 0.99, 0.0, 0.0),
    (1.0, 100000.0, 0.99, 0.0, 0.0),
    (50000.0, 100000.0, 0.99, 0.0, 0.0),
    (98906.0, 100000.0, 0.99, 0.00015962501729101418, 0.001680685850467731),
    (98969.0, 100000.0, 0.99, 0.007723834924565287, 0.1660975153530963),
    (99000.0, 100000.0, 0.99, 0.012678161323544589, 0.5042686879721229),
    (99031.0, 100000.0, 0.99, 0.007885853372194001, 0.8416321140066325),
    (99094.0, 100000.0, 0.99, 0.00013276363518518057, 0.9988439452168577),
    (99999.0, 100000.0, 0.99, 0.0, 1.0),
    // Small k where (1 - p)^(n - k) underflows although the mass is a normal number.
    (10.0, 1100.0, 0.5, 5.050689010687844e-308, 5.097367491037505e-308),
    (14.0, 1100.0, 0.5, 2.9513324041838846e-300, 2.989803284040754e-300),
    (15.0, 1100.0, 0.5, 2.1367646606291324e-298, 2.16666269346954e-298),
    (16.0, 1100.0, 0.5, 1.4489935354891303e-296, 1.4706601624238258e-296),
    (20.0, 1100.0, 0.5, 1.711087104904223e-289, 1.7433098849775326e-289),
    (25.0, 1100.0, 0.5, 3.906996229178926e-281, 3.999839082741416e-281),
];

// Columns: x, mean, mass, cdf. The last block has x > 1000, which takes the incomplete-gamma branch.
#[rustfmt::skip]
const POISSON_REF: &[(f64, f64, f64, f64)] = &[
    (0.0, 0.5, 0.6065306597126334, 0.6065306597126334),
    (1.0, 0.5, 0.3032653298563167, 0.9097959895689501),
    (3.0, 0.5, 0.012636055410679864, 0.9982483774437092),
    (0.0, 5.0, 0.006737946999085467, 0.006737946999085467),
    (2.0, 5.0, 0.08422433748856833, 0.12465201948308115),
    (5.0, 5.0, 0.1754673697678507, 0.6159606548330632),
    (10.0, 5.0, 0.018132788707821874, 0.986304731401617),
    (25.0, 5.0, 1.294589320478486e-10, 0.9999999999695003),
    (70.0, 100.0, 0.00031056089812001654, 0.0009714440282773882),
    (100.0, 100.0, 0.039860996809147134, 0.5265621985299984),
    (130.0, 100.0, 0.000575252683815463, 0.9982931596294985),
    (900.0, 1000.0, 7.516954352125952e-05, 0.0006977673277963068),
    (1000.0, 1000.0, 0.012614611348721499, 0.508409367168506),
    (1100.0, 1000.0, 9.498944242299507e-05, 0.9991323590365564),
    (1050.0, 1000.0, 0.003599490404803648, 0.9439711616363267),
    (1200.0, 1000.0, 7.992642848843571e-11, 0.9999999996115061),
    (1500.0, 2000.0, 1.878037542328086e-32, 7.468034373687017e-32),
    (2000.0, 2000.0, 0.00892024889598624, 0.5059467004517837),
    (2100.0, 2000.0, 0.0007442234118916615, 0.9872092799080792),
    (2300.0, 2000.0, 4.011826753210622e-12, 0.999999999973895),
    (4800.0, 5000.0, 9.987733294132511e-05, 0.002269403878343364),
    (5000.0, 5000.0, 0.005641801804664022, 0.5037611677708467),
];

fn rel_err(got: Value, want: f64) -> f64 {
    // An error or a non-finite result is the worst possible miss, not a skipped one.
    let Some(g) = got.as_f64().filter(|g| g.is_finite()) else { return f64::INFINITY };
    if want == 0.0 { g.abs() } else { ((g - want) / want).abs() }
}

/// Error bound the grid below must meet: 1e-13, plus 2e-16 per unit of |ln value| for far-tail
/// results (an exp of a sum of large logs cannot do better in double precision). Measured: at
/// most 9e-14 on the grid (the n = 100,000 cumulative cases), 2e-14 for n <= 10,000 away from the
/// far tails, 2e-15 on the Excel-sized central cases. The log-gamma code was off by 1.5e-11.
fn allowed(want: f64) -> f64 {
    // A reference that underflows to 0 must come back as (almost) 0, not as any finite value.
    if want == 0.0 {
        return 1e-300;
    }
    1e-13 + 2e-16 * want.abs().ln().abs()
}

#[test]
fn binom_and_poisson_match_exact_decimal_references() {
    // Worst error relative to the allowance (above 1 is a failure) and plain worst error.
    let (mut worst, mut worst_plain) = (0.0f64, 0.0f64);
    let mut check = |what: &str, got: Value, want: f64| {
        let e = rel_err(got, want);
        let ratio = e / allowed(want);
        if ratio > 1.0 {
            println!("{what}: error {e:e}, allowed {:e}", allowed(want));
        }
        worst = worst.max(ratio);
        if want > 1e-3 {
            worst_plain = worst_plain.max(e);
        }
    };
    for &(k, nn, p, pmf, cdf) in BINOM_REF {
        check(&format!("BINOM.DIST({k},{nn},{p},FALSE)"), ev("BINOM.DIST", vec![n(k), n(nn), n(p), b(false)]), pmf);
        check(&format!("BINOM.DIST({k},{nn},{p},TRUE)"), ev("BINOM.DIST", vec![n(k), n(nn), n(p), b(true)]), cdf);
    }
    for &(x, m, pmf, cdf) in POISSON_REF {
        check(&format!("POISSON.DIST({x},{m},FALSE)"), ev("POISSON.DIST", vec![n(x), n(m), b(false)]), pmf);
        check(&format!("POISSON.DIST({x},{m},TRUE)"), ev("POISSON.DIST", vec![n(x), n(m), b(true)]), cdf);
    }
    println!("worst error / allowance {worst:.3}; worst relative error where the result is above 1e-3: {worst_plain:e}");
    assert!(worst <= 1.0);
    assert!(worst_plain < 1e-13);
}

#[test]
fn binom_and_poisson_edge_cases() {
    for cum in [false, true] {
        let pm = |k: f64, nn: f64, p: f64| ev("BINOM.DIST", vec![n(k), n(nn), n(p), b(cum)]);
        // p = 0 and p = 1
        close(pm(0.0, 50.0, 0.0), 1.0);
        close(pm(3.0, 50.0, 1.0), 0.0);
        close(pm(50.0, 50.0, 1.0), 1.0);
        // n = 0
        close(pm(0.0, 0.0, 0.3), 1.0);
        // k = n is certain when cumulative and p^n otherwise
        close(pm(20.0, 20.0, 0.5), if cum { 1.0 } else { 0.5f64.powi(20) });
        close_tol(pm(10_000.0, 10_000.0, 0.999), if cum { 1.0 } else { 0.999f64.powi(10_000) }, 1e-12);
        // k = 0 is q^n
        close_tol(pm(0.0, 1000.0, 0.001), 0.999f64.powi(1000), 1e-12);
        close_tol(pm(0.0, 100_000.0, 0.00001), (1.0f64 - 1e-5).powi(100_000), 1e-9);
    }
    close(ev("POISSON.DIST", vec![n(0.0), n(0.0), b(false)]), 1.0);
    close(ev("POISSON.DIST", vec![n(4.0), n(0.0), b(false)]), 0.0);
    close(ev("POISSON.DIST", vec![n(4.0), n(0.0), b(true)]), 1.0);
    close_tol(ev("POISSON.DIST", vec![n(0.0), n(3.0), b(true)]), (-3.0f64).exp(), 1e-15);
    // Fractions truncate as before.
    close(ev("BINOM.DIST", vec![n(6.9), n(10.9), n(0.5), b(false)]), 0.205078125);
    // The 10,000 trial switch of the method no longer shows: neighbours agree to 1e-14.
    let lo = ev("BINOM.DIST", vec![n(5000.0), n(10_000.0), n(0.5), b(true)]).as_f64().unwrap();
    let hi = ev("BINOM.DIST", vec![n(5000.0), n(10_001.0), n(0.5), b(true)]).as_f64().unwrap();
    let pmf = ev("BINOM.DIST", vec![n(5000.0), n(10_000.0), n(0.5), b(false)]).as_f64().unwrap();
    // cdf(5000; 10,001) = cdf(5000; 10,000) - p * mass(5000; 10,000) by the recurrence on n
    assert!(((hi - (lo - 0.5 * pmf)) / hi).abs() < 1e-13, "{lo} {hi}");
    // Inverse stays consistent: the smallest k with cdf >= alpha.
    close(ev("BINOM.INV", vec![n(10_000.0), n(0.5), n(0.5)]), 5000.0);
}

/// Security: FREQUENCY finds each value's bin by binary search, and MODE counts distinct numbers
/// through an index, so neither is quadratic in its inputs. Results are as before: unsorted and
/// duplicate bins keep their positions (a value goes to the first of equal bins), and tied modes
/// come in first-occurrence order.
#[test]
fn frequency_and_mode_scale_with_their_inputs() {
    let f = ev("FREQUENCY", vec![c(&[1.0, 2.0, 2.0, 3.0, 5.0, 9.0, -1.0]), c(&[3.0, 2.0, 2.0, 0.0])]);
    assert_eq!(rows_of(&f), vec![vec![nv(1.0)], vec![nv(3.0)], vec![nv(0.0)], vec![nv(1.0)], vec![nv(2.0)]]);
    // NaN data goes to the overflow bin, and a NaN bin takes nothing
    let f = ev("FREQUENCY", vec![c(&[f64::NAN, 0.5, 2.0]), c(&[f64::NAN, 1.0])]);
    assert_eq!(rows_of(&f), vec![vec![nv(0.0)], vec![nv(1.0)], vec![nv(2.0)]]);
    let data: Vec<f64> = (0..200_000).map(|i| f64::from(i) + 1e6).collect();
    let bins: Vec<f64> = (0..200_000).map(f64::from).collect();
    let f = ev("FREQUENCY", vec![c(&data), c(&bins)]);
    assert_eq!(rows_of(&f).last(), Some(&vec![nv(200_000.0)]));
    let distinct = ev("MODE", vec![c(&data)]);
    is_err(distinct, CellError::NA);
    assert_eq!(rows_of(&ev("MODE.MULT", vec![c(&[3.0, 0.0, 1.0, -0.0, 3.0, 1.0])])), vec![vec![nv(3.0)], vec![nv(0.0)], vec![nv(1.0)]]);
}
