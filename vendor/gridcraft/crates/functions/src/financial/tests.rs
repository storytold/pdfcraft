use crate::Arg;
use crate::util::testutil::*;
use gridcraft_core::{CellError, DateSystem, Value, date::serial_from_ymd};

fn d(y: i64, m: i64, dd: i64) -> Arg {
    n(serial_from_ymd(DateSystem::D1900, y, m, dd).unwrap())
}

#[test]
fn tvm() {
    close_tol(ev("PMT", vec![n(0.08 / 12.0), n(10.0), n(10000.0)]), -1037.032089, 1e-9);
    close_tol(ev("PMT", vec![n(0.06 / 12.0), n(216.0), n(0.0), n(50000.0)]), -129.0811609, 1e-9);
    close(ev("PMT", vec![n(0.0), n(10.0), n(1000.0)]), -100.0);
    is_err(ev("PMT", vec![n(0.1), n(0.0), n(1000.0)]), CellError::Num);
    close_tol(ev("FV", vec![n(0.06 / 12.0), n(10.0), n(-200.0), n(-500.0), n(1.0)]), 2581.403374, 1e-9);
    close_tol(ev("FV", vec![n(0.01), n(12.0), n(-1000.0)]), 12682.50301, 1e-9);
    close(ev("FV", vec![n(0.0), n(10.0), n(-100.0)]), 1000.0);
    close_tol(ev("PV", vec![n(0.08 / 12.0), n(240.0), n(500.0)]), -59777.14585, 1e-9);
    close_tol(ev("NPER", vec![n(0.01), n(-100.0), n(-1000.0), n(10000.0), n(1.0)]), 59.6738657, 1e-8);
    close_tol(ev("NPER", vec![n(0.01), n(-100.0), n(-1000.0), n(10000.0)]), 60.0821229, 1e-8);
    close_tol(ev("NPER", vec![n(0.01), n(-100.0), n(-1000.0)]), -9.57859404, 1e-8);
    close_tol(ev("IPMT", vec![n(0.1 / 12.0), n(1.0), n(36.0), n(8000.0)]), -66.66666667, 1e-9);
    close_tol(ev("IPMT", vec![n(0.1), n(3.0), n(3.0), n(8000.0)]), -292.4471299, 1e-9);
    is_err(ev("IPMT", vec![n(0.1), n(4.0), n(3.0), n(8000.0)]), CellError::Num);
    close_tol(ev("PPMT", vec![n(0.1 / 12.0), n(1.0), n(24.0), n(2000.0)]), -75.62318601, 1e-9);
    close_tol(ev("PPMT", vec![n(0.08), n(10.0), n(10.0), n(200000.0)]), -27598.05346, 1e-9);
    close_tol(ev("RATE", vec![n(48.0), n(-200.0), n(8000.0)]), 0.0077014725, 1e-7);
    close_tol(ev("CUMIPMT", vec![n(0.09 / 12.0), n(360.0), n(125000.0), n(13.0), n(24.0), n(0.0)]), -11135.23213, 1e-9);
    close_tol(ev("CUMIPMT", vec![n(0.09 / 12.0), n(360.0), n(125000.0), n(1.0), n(1.0), n(0.0)]), -937.5, 1e-9);
    close_tol(ev("CUMPRINC", vec![n(0.09 / 12.0), n(360.0), n(125000.0), n(13.0), n(24.0), n(0.0)]), -934.1071234, 1e-9);
    close_tol(ev("CUMPRINC", vec![n(0.09 / 12.0), n(360.0), n(125000.0), n(1.0), n(1.0), n(0.0)]), -68.27827118, 1e-9);
    is_err(ev("CUMIPMT", vec![n(0.0), n(360.0), n(125000.0), n(1.0), n(1.0), n(0.0)]), CellError::Num);
    is_err(ev("CUMIPMT", vec![n(0.01), n(360.0), n(125000.0), n(1.0), n(1.0), n(2.0)]), CellError::Num);
}

/// RATE with the given arguments, as a number or an error.
fn rate_of(args: &[f64]) -> Value {
    ev("RATE", args.iter().map(|x| n(*x)).collect())
}

#[track_caller]
fn rate_is(args: &[f64], expected: f64) {
    close_tol(rate_of(args), expected, 1e-15);
}

/// Expected values observed in Excel 365 with these synthetic inputs.
#[test]
fn rate_follows_excel_iteration() {
    // No payments and a zero future value: the answer is -100%, which the iteration
    // approaches without reaching. Excel returns the last iterate, bit for bit.
    rate_is(&[5.0, 0.0, -100.0, 0.0], -0.999_999_409_145_182_5);
    rate_is(&[3.0, 0.0, -50.0, 0.0], -0.999_999_744_782_979_9);
    rate_is(&[10.0, 0.0, -1.0, 0.0], -0.999_998_741_101_371_3);
    rate_is(&[5.0, 0.0, -100.0, 0.0, 0.0, 0.5], -0.999_999_408_783_809_4);
    rate_is(&[5.0, 0.0, -100.0, 0.0, 0.0, -0.99], -0.999_999_411_730_630_3);
    // Second starting point goes the other way when the equation is positive at the guess.
    rate_is(&[5.0, 0.0, 100.0, 0.0], -0.999_999_409_810_240_3);
    rate_is(&[5.0, 0.0, -100.0, 1e-29], -0.999_999_260_333_075_4);
    rate_is(&[5.0, 0.0, -100.0, 1e4], 1.511_886_431_510_563_3);
    rate_is(&[4.0, 0.0, -1.0, 100.0], 2.162_277_660_168_009);
    // Within 1e-7 by step but not by value: Excel keeps going.
    rate_is(&[5.0, 0.0, -1e25, 0.0], -0.999_999_628_525_605_1);
    // 200 steps without converging: the last iterate when it is within 1e-4, else #NUM!.
    rate_is(&[20.0, 0.0, -1.0, 0.0], -0.999_112_071_912_539_3);
    rate_is(&[36.0, 0.0, -1.0, 0.0, 0.0, -0.994], -0.999_875_315_309_626_9);
    is_err(rate_of(&[12.0, 0.0, -1.0, 0.0]), CellError::Num);
    is_err(rate_of(&[25.0, 0.0, -1.0, 0.0]), CellError::Num);
    is_err(rate_of(&[20.0, 0.0, -1e60, 0.0]), CellError::Num);
    // Large rates the iteration can't reach are #NUM! in Excel.
    is_err(rate_of(&[4.0, 0.0, -0.0001, 100.0]), CellError::Num);
    is_err(rate_of(&[5.0, 0.0, -100.0, 1e5]), CellError::Num);
    is_err(rate_of(&[10.0, -300.0, 1000.0]), CellError::Num);
    // ...but are found from a closer guess.
    rate_is(&[4.0, 0.0, -0.1, 100.0, 0.0, 4.0], 4.623_413_251_903_489);
    close_tol(rate_of(&[10.0, -300.0, 1000.0, 0.0, 0.0, 0.3]), 0.273_198_424_105_560_4, 1e-12);
    // Converging on a rate at or below -100% (or within 1e-7 of it) is #NUM!.
    is_err(rate_of(&[1.0, 0.0, -1.0, 0.0]), CellError::Num);
    is_err(rate_of(&[10.0, -1000.0, 1000.0]), CellError::Num);
    is_err(rate_of(&[1.0, 0.0, -100.0, 1e-5]), CellError::Num);
    rate_is(&[1.0, 0.0, -100.0, 2e-5], -0.999_999_8);
    is_err(rate_of(&[5.0, 0.0, -1e30, 0.0]), CellError::Num);
    is_err(rate_of(&[0.5, 0.0, -1.0, 0.0]), CellError::Num);
    // A value that never gets near zero is #NUM!, however small the step.
    is_err(rate_of(&[4.0, 0.0, 100.0, 100.0]), CellError::Num);
    is_err(rate_of(&[4.0, 0.0, -1e-12, 1e-6]), CellError::Num);
    rate_is(&[4.0, 0.0, -1e-14, 1e-8], 0.099_000_000_283_235_72);
    // Subnormal steps are flushed to zero.
    rate_is(&[3.0, 0.0, -1e-305, 0.0], 0.101);
    is_err(rate_of(&[4.0, 0.0, 0.0, 100.0]), CellError::Num);
    // Guess at or below -1 is #VALUE!; type is 0 or anything else (1).
    is_err(rate_of(&[10.0, -100.0, 1000.0, 0.0, 0.0, -1.0]), CellError::Value);
    is_err(rate_of(&[0.0, -100.0, 1000.0]), CellError::Num);
    // Excel does not step the second point back to -1: a fractional nper with a guess near -1 is #NUM!
    // even though a root exists at -0.99, and the same case from a guess of -0.99 finds it.
    is_err(rate_of(&[0.5, 0.0, -1.0, 0.1, 0.0, -0.9995]), CellError::Num);
    is_err(rate_of(&[0.5, 0.0, -1.0, 0.1, 0.0, -0.5]), CellError::Num);
    close_tol(rate_of(&[0.5, 0.0, -1.0, 0.1, 0.0, -0.99]), -0.99, 1e-7);
    assert_eq!(rate_of(&[10.0, -100.0, 1000.0, 0.0, 2.0]), rate_of(&[10.0, -100.0, 1000.0, 0.0, 1.0]));
    // Ordinary loans, and large amounts that can't get within 1e-7 of zero.
    close_tol(rate_of(&[360.0, -1000.0, 150000.0]), 0.005_850_253_376_759_637, 1e-11);
    close_tol(rate_of(&[360.0, -1000.0, 150000.0, 0.0, 1.0]), 0.005_898_953_457_098_463, 1e-11);
    close_tol(rate_of(&[1000.0, -1e6, 1e8]), 0.009_999_522_655_994_865, 1e-12);
    close_tol(rate_of(&[360.0, -1e9, 1.5e11]), 0.005_850_253_376_759_668, 1e-12);
    close_tol(rate_of(&[10.0, -50.0, 1000.0]), -0.109_560_293_684_682_5, 1e-14);
    // Roots at 0% come out as rounding noise in both, so only the absolute size matters.
    match rate_of(&[10.0, -100.0, 1000.0]) {
        Value::Number(x) => assert!(x.abs() < 1e-8, "{x}"),
        other => panic!("{other:?}"),
    }
}

/// PMT within about 4 ULPs of Excel's (Value2 of synthetic probes; libm differs by an ULP across
/// platforms). The old `(1 + rate)^nper` form was 25 to 91 ULPs off on these, enough to flip a loan schedule's final `IF(p - SUM(r) > 0, ...)`.
#[test]
fn pmt_matches_excel_within_a_few_ulps() {
    for (rate, nper, pv, fv, typ, excel) in [
        (0.141 / 12.0, 12.0, -4653300.0, 0.0, 0.0, 418025.37230211904),
        (0.005716666666666666, 26.0, 42678411.0, -26010.62, 0.0, -1770235.3828507778),
        (0.012164, 29.0, 16722649.0, 0.0, 1.0, -679516.0784762775),
        (0.009916666666666666, 5.0, 8590842.8, 0.0, 0.0, -1769620.32624324),
        (0.011158333333333333, 36.0, 21457108.0, 49218.26, 1.0, -720095.3912155787),
        (-0.5, 2.5, 100.0, 0.0, 0.0, -10.736861692729645),
    ] {
        let got = ev("PMT", vec![n(rate), n(nper), n(pv), n(fv), n(typ)]);
        close_tol(got, excel, 1e-15);
    }
}

/// Edge cases observed in Excel: a rate of -1 or less is #NUM!, a huge nper leaves -pv*rate, and a
/// tiny rate no longer loses its low digits in 1 + rate.
#[test]
fn pmt_edge_cases_follow_excel() {
    is_err(ev("PMT", vec![n(-2.0), n(3.0), n(100.0)]), CellError::Num);
    is_err(ev("PMT", vec![n(-1.0), n(3.0), n(100.0)]), CellError::Num);
    is_err(ev("PMT", vec![n(-2.0), n(2.5), n(100.0)]), CellError::Num);
    assert_eq!(ev("PMT", vec![n(0.1), n(10000.0), n(100.0)]), Value::Number(-10.0));
    assert_eq!(ev("PMT", vec![n(5.0), n(1000.0), n(100.0)]), Value::Number(-500.0));
    assert_eq!(ev("PMT", vec![n(1e-300), n(10.0), n(100.0)]), Value::Number(-10.0));
    close_tol(ev("PMT", vec![n(1e-12), n(10.0), n(100.0)]), -10.000000000055001, 1e-15);
    close_tol(ev("PMT", vec![n(1e-300), n(10.0), n(1e12)]), -1e11, 1e-15);
    close_tol(ev("PMT", vec![n(-0.999), n(5.0), n(100.0)]), -9.990_000_000_000_092e-14, 1e-15);
    close_tol(ev("PMT", vec![n(0.05), n(-10.0), n(100.0)]), 7.950_457_496_545_667, 1e-15);
    close_tol(ev("PMT", vec![n(-1e-17), n(1.0), n(1e18), n(-1e18)]), 10.0, 1e-9);
    close_tol(ev("PMT", vec![n(-0.5), n(60.0), n(1e18)]), -0.433_680_868_994_201_8, 1e-6);
    close_tol(ev("PMT", vec![n(0.05), n(10.0), n(100.0), n(50.0), n(1.0)]), -16.119701185541427, 1e-15);
}

#[test]
fn cash_flows() {
    close_tol(ev("NPV", vec![n(0.1), n(-10000.0), n(3000.0), n(4200.0), n(6800.0)]), 1188.443412, 1e-9);
    close_tol(ev("NPV", vec![n(0.08), rf(col(&[8000.0, 9200.0, 10000.0, 12000.0, 14500.0]))]), 41922.06155, 1e-9);
    is_err(ev("NPV", vec![n(-1.0), n(1.0)]), CellError::Div0);
    let flows = col(&[-70000.0, 12000.0, 15000.0, 18000.0, 21000.0]);
    close_tol(ev("IRR", vec![rf(flows)]), -0.021244848, 1e-7);
    close_tol(ev("IRR", vec![rf(col(&[-70000.0, 12000.0, 15000.0, 18000.0, 21000.0, 26000.0]))]), 0.086630948, 1e-7);
    close_tol(ev("IRR", vec![rf(col(&[-70000.0, 12000.0, 15000.0])), n(-0.1)]), -0.44350694, 1e-7);
    is_err(ev("IRR", vec![rf(col(&[1.0, 2.0]))]), CellError::Num);
    let vals = col(&[-10000.0, 2750.0, 4250.0, 3250.0, 2750.0]);
    let dates = col(&[39448.0, 39508.0, 39751.0, 39859.0, 39904.0]);
    close_tol(ev("XNPV", vec![n(0.09), rf(vals.clone()), rf(dates.clone())]), 2086.647602, 1e-9);
    close_tol(ev("XIRR", vec![rf(vals.clone()), rf(dates)]), 0.373362535, 1e-7);
    is_err(ev("XNPV", vec![n(0.09), rf(vals), rf(col(&[1.0, 2.0]))]), CellError::Num);
    let m = col(&[-120000.0, 39000.0, 30000.0, 21000.0, 37000.0, 46000.0]);
    close_tol(ev("MIRR", vec![rf(m), n(0.1), n(0.12)]), 0.1260943, 1e-6);
    close_tol(ev("MIRR", vec![rf(col(&[-120000.0, 39000.0, 30000.0, 21000.0])), n(0.1), n(0.12)]), -0.048044655, 1e-7);
    is_err(ev("MIRR", vec![rf(col(&[1.0, 2.0])), n(0.1), n(0.1)]), CellError::Div0);
    // Excel 365 gives -1 when every flow is negative: the positive flows are worth nothing.
    close_tol(ev("MIRR", vec![rf(col(&[-10000.0, -3400.0, -6500.0, -1000.0])), n(0.07), n(0.01)]), -1.0, 1e-12);
    close(ev("FVSCHEDULE", vec![n(1.0), av(row(&[0.09, 0.11, 0.1]))]), 1.33089);
    close(ev("EFFECT", vec![n(0.0525), n(4.0)]), 0.0535426673707584);
    is_err(ev("EFFECT", vec![n(-0.1), n(4.0)]), CellError::Num);
    close_tol(ev("NOMINAL", vec![n(0.053543), n(4.0)]), 0.052500319, 1e-8);
    close_tol(ev("ISPMT", vec![n(0.1 / 12.0), n(0.0), n(36.0), n(8000000.0)]), -66666.66667, 1e-9);
    is_err(ev("ISPMT", vec![n(0.1), n(1.0), n(0.0), n(1.0)]), CellError::Div0);
    close_tol(ev("PDURATION", vec![n(0.025), n(2000.0), n(2200.0)]), 3.859866163, 1e-9);
    is_err(ev("PDURATION", vec![n(0.0), n(2000.0), n(2200.0)]), CellError::Num);
    close_tol(ev("RRI", vec![n(96.0), n(10000.0), n(11000.0)]), 0.0009933, 1e-4);
    close(ev("DOLLARDE", vec![n(1.02), n(16.0)]), 1.125);
    close(ev("DOLLARDE", vec![n(1.1), n(32.0)]), 1.3125);
    close(ev("DOLLARFR", vec![n(1.125), n(16.0)]), 1.02);
    close(ev("DOLLARFR", vec![n(1.125), n(32.0)]), 1.04);
    is_err(ev("DOLLARDE", vec![n(1.02), n(0.0)]), CellError::Div0);
    is_err(ev("DOLLARDE", vec![n(1.02), n(-1.0)]), CellError::Num);
}

#[test]
fn depreciation() {
    close(ev("SLN", vec![n(30000.0), n(7500.0), n(10.0)]), 2250.0);
    is_err(ev("SLN", vec![n(30000.0), n(7500.0), n(0.0)]), CellError::Div0);
    close_tol(ev("SYD", vec![n(30000.0), n(7500.0), n(10.0), n(1.0)]), 4090.909091, 1e-9);
    close_tol(ev("SYD", vec![n(30000.0), n(7500.0), n(10.0), n(10.0)]), 409.0909091, 1e-9);
    let dbv = |p: f64| ev("DB", vec![n(1000000.0), n(100000.0), n(6.0), n(p), n(7.0)]);
    close_tol(dbv(1.0), 186083.3333, 1e-9);
    close_tol(dbv(2.0), 259639.4167, 1e-9);
    close_tol(dbv(3.0), 176814.4428, 1e-9);
    close_tol(dbv(7.0), 15845.09847, 1e-9);
    is_err(dbv(8.0), CellError::Num);
    close_tol(ev("DDB", vec![n(2400.0), n(300.0), n(3650.0), n(1.0)]), 1.315068493, 1e-8);
    close(ev("DDB", vec![n(2400.0), n(300.0), n(120.0), n(1.0), n(2.0)]), 40.0);
    close(ev("DDB", vec![n(2400.0), n(300.0), n(10.0), n(1.0), n(2.0)]), 480.0);
    close_tol(ev("DDB", vec![n(2400.0), n(300.0), n(10.0), n(2.0), n(1.5)]), 306.0, 1e-12);
    close_tol(ev("DDB", vec![n(2400.0), n(300.0), n(10.0), n(10.0)]), 22.1225472, 1e-8);
    is_err(ev("DDB", vec![n(2400.0), n(300.0), n(10.0), n(11.0)]), CellError::Num);
    close_tol(ev("VDB", vec![n(2400.0), n(300.0), n(3650.0), n(0.0), n(1.0)]), 1.315068493, 1e-8);
    close(ev("VDB", vec![n(2400.0), n(300.0), n(120.0), n(0.0), n(1.0)]), 40.0);
    close(ev("VDB", vec![n(2400.0), n(300.0), n(10.0), n(0.0), n(1.0)]), 480.0);
    close_tol(ev("VDB", vec![n(2400.0), n(300.0), n(120.0), n(6.0), n(18.0)]), 396.3060533, 1e-8);
    close_tol(ev("VDB", vec![n(2400.0), n(300.0), n(120.0), n(6.0), n(18.0), n(1.5)]), 311.8089367, 1e-8);
    close(ev("VDB", vec![n(2400.0), n(300.0), n(10.0), n(0.0), n(0.875), n(1.5)]), 315.0);
    // Over the whole life the total depreciation is cost - salvage.
    close(ev("VDB", vec![n(2400.0), n(300.0), n(10.0), n(0.0), n(10.0)]), 2100.0);
    is_err(ev("VDB", vec![n(2400.0), n(300.0), n(10.0), n(5.0), n(4.0)]), CellError::Num);
    close_tol(ev("AMORLINC", vec![n(2400.0), d(2008, 8, 19), d(2008, 12, 31), n(300.0), n(1.0), n(0.15), n(1.0)]), 360.0, 1e-9);
    close(ev("AMORDEGRC", vec![n(2400.0), d(2008, 8, 19), d(2008, 12, 31), n(300.0), n(1.0), n(0.15), n(1.0)]), 776.0);
}

#[test]
fn coupons() {
    let args = |f: &str| ev(f, vec![d(2011, 1, 25), d(2011, 11, 15), n(2.0), n(1.0)]);
    close(args("COUPDAYBS"), 71.0);
    close(args("COUPDAYS"), 181.0);
    close(args("COUPDAYSNC"), 110.0);
    close(args("COUPNCD"), 40678.0);
    close(args("COUPPCD"), 40497.0);
    close(args("COUPNUM"), 2.0);
    close(ev("COUPDAYS", vec![d(2011, 1, 25), d(2011, 11, 15), n(2.0), n(0.0)]), 180.0);
    close(ev("COUPDAYS", vec![d(2011, 1, 25), d(2011, 11, 15), n(4.0), n(3.0)]), 91.25);
    close(ev("COUPNUM", vec![d(2007, 1, 25), d(2008, 11, 15), n(2.0), n(1.0)]), 4.0);
    is_err(ev("COUPNUM", vec![d(2012, 1, 25), d(2011, 11, 15), n(2.0)]), CellError::Num);
    is_err(ev("COUPNUM", vec![d(2011, 1, 25), d(2011, 11, 15), n(3.0)]), CellError::Num);
    is_err(ev("COUPNUM", vec![d(2011, 1, 25), d(2011, 11, 15), n(2.0), n(5.0)]), CellError::Num);
    // End-of-month maturity keeps month-end coupon dates.
    close(ev("COUPPCD", vec![d(2011, 3, 15), d(2011, 11, 30), n(2.0)]), serial_from_ymd(DateSystem::D1900, 2010, 11, 30).unwrap());
    close(ev("COUPNCD", vec![d(2011, 3, 15), d(2011, 11, 30), n(2.0)]), serial_from_ymd(DateSystem::D1900, 2011, 5, 31).unwrap());
}

#[test]
fn securities() {
    close_tol(ev("DISC", vec![d(2007, 1, 25), d(2007, 6, 15), n(97.975), n(100.0), n(1.0)]), 0.052420213, 1e-8);
    close_tol(ev("PRICEDISC", vec![d(2008, 2, 16), d(2008, 3, 1), n(0.0525), n(100.0), n(2.0)]), 99.79583333, 1e-9);
    close_tol(ev("YIELDDISC", vec![d(2008, 2, 16), d(2008, 3, 1), n(99.795), n(100.0), n(2.0)]), 0.052823, 1e-5);
    close_tol(ev("INTRATE", vec![d(2008, 2, 15), d(2008, 5, 15), n(1000000.0), n(1014420.0), n(2.0)]), 0.05768, 1e-9);
    close_tol(ev("RECEIVED", vec![d(2008, 2, 15), d(2008, 5, 15), n(1000000.0), n(0.0575), n(2.0)]), 1014584.654, 1e-9);
    is_err(ev("DISC", vec![d(2008, 2, 15), d(2008, 1, 15), n(97.0), n(100.0)]), CellError::Num);
    close_tol(ev("TBILLPRICE", vec![d(2008, 3, 31), d(2008, 6, 1), n(0.09)]), 98.45, 1e-12);
    close_tol(ev("TBILLYIELD", vec![d(2008, 3, 31), d(2008, 6, 1), n(98.45)]), 0.091417, 1e-6);
    close_tol(ev("TBILLEQ", vec![d(2008, 3, 31), d(2008, 6, 1), n(0.0914)]), 0.094151, 1e-6);
    is_err(ev("TBILLPRICE", vec![d(2008, 3, 31), d(2009, 6, 1), n(0.09)]), CellError::Num);
    close_tol(ev("ACCRINTM", vec![d(2008, 4, 1), d(2008, 6, 15), n(0.1), n(1000.0), n(3.0)]), 20.54794521, 1e-9);
    is_err(ev("ACCRINTM", vec![d(2008, 6, 15), d(2008, 4, 1), n(0.1)]), CellError::Num);
    close_tol(ev("ACCRINT", vec![d(2008, 3, 1), d(2008, 8, 31), d(2008, 5, 1), n(0.1), n(1000.0), n(2.0), n(0.0)]), 16.66666667, 1e-9);
    close_tol(ev("ACCRINT", vec![d(2008, 3, 5), d(2008, 8, 31), d(2008, 5, 1), n(0.1), n(1000.0), n(2.0), n(0.0), b(false)]), 15.55555556, 1e-9);
    close_tol(ev("PRICEMAT", vec![d(2008, 2, 15), d(2008, 4, 13), d(2007, 11, 11), n(0.061), n(0.061), n(0.0)]), 99.98449888, 1e-9);
    close_tol(ev("YIELDMAT", vec![d(2008, 3, 15), d(2008, 11, 3), d(2007, 11, 8), n(0.0625), n(100.0123), n(0.0)]), 0.060954, 1e-6);
    close_tol(ev("PRICE", vec![d(2008, 2, 15), d(2017, 11, 15), n(0.0575), n(0.065), n(100.0), n(2.0), n(0.0)]), 94.63436, 1e-7);
    close_tol(ev("YIELD", vec![d(2008, 2, 15), d(2016, 11, 15), n(0.0575), n(95.04287), n(100.0), n(2.0), n(0.0)]), 0.065, 1e-6);
    is_err(ev("PRICE", vec![d(2008, 2, 15), d(2017, 11, 15), n(-0.01), n(0.065), n(100.0), n(2.0)]), CellError::Num);
    close_tol(ev("DURATION", vec![d(2018, 7, 1), d(2048, 1, 1), n(0.08), n(0.09), n(2.0), n(1.0)]), 10.9191453, 1e-8);
    close_tol(ev("MDURATION", vec![d(2008, 1, 1), d(2016, 1, 1), n(0.08), n(0.09), n(2.0), n(1.0)]), 5.73567, 1e-6);
    // A one-coupon bond uses the closed form for yield; PRICE and YIELD invert each other.
    let p = ev("PRICE", vec![d(2008, 2, 15), d(2008, 6, 15), n(0.05), n(0.04), n(100.0), n(2.0)]);
    let pv = match p {
        Value::Number(x) => x,
        _ => panic!(),
    };
    close_tol(ev("YIELD", vec![d(2008, 2, 15), d(2008, 6, 15), n(0.05), n(pv), n(100.0), n(2.0)]), 0.04, 1e-9);
}

fn number(v: Value) -> f64 {
    match v {
        Value::Number(x) => x,
        other => panic!("expected a number, got {other:?}"),
    }
}

#[test]
fn odd_periods() {
    // The worked examples on Microsoft's function pages (results as printed there).
    let first = |x: f64, rest: &[f64]| {
        let mut a = vec![d(2008, 11, 11), d(2021, 3, 1), d(2008, 10, 15), d(2009, 3, 1), n(x)];
        a.extend(rest.iter().map(|v| n(*v)));
        a
    };
    close_tol(ev("ODDFPRICE", first(0.0785, &[0.0625, 100.0, 2.0, 1.0])), 113.60, 0.005 / 113.6);
    close_tol(ev("ODDFYIELD", first(0.0575, &[84.50, 100.0, 2.0, 0.0])), 0.0772, 0.00005);
    let lp = vec![d(2008, 2, 7), d(2008, 6, 15), d(2007, 10, 15), n(0.0375), n(0.0405), n(100.0), n(2.0), n(0.0)];
    close_tol(ev("ODDLPRICE", lp), 99.88, 0.005 / 99.88);
    let ly = vec![d(2008, 4, 20), d(2008, 6, 15), d(2007, 12, 24), n(0.0375), n(99.875), n(100.0), n(2.0), n(0.0)];
    close_tol(ev("ODDLYIELD", ly), 0.04519, 0.000005);

    // Yield → price → yield, for a short and a long odd first period and a long odd last period.
    for (issue, basis) in [(d(2008, 10, 15), 0.0), (d(2007, 1, 20), 1.0), (d(2008, 1, 31), 3.0)] {
        let args = |x: f64| vec![d(2008, 11, 11), d(2021, 3, 1), issue.clone(), d(2009, 3, 1), n(0.06), n(x), n(100.0), n(2.0), n(basis)];
        let p = number(ev("ODDFPRICE", args(0.05)));
        close_tol(ev("ODDFYIELD", args(p)), 0.05, 1e-9);
    }
    let last = |x: f64| vec![d(2009, 2, 1), d(2009, 12, 15), d(2008, 3, 15), n(0.04), n(x), n(100.0), n(4.0), n(1.0)];
    let p = number(ev("ODDLPRICE", last(0.07)));
    close_tol(ev("ODDLYIELD", last(p)), 0.07, 1e-9);

    // Argument checks: frequency, basis, date order, negative yield, non-positive price.
    is_err(ev("ODDFPRICE", first(0.0785, &[0.0625, 100.0, 3.0, 1.0])), CellError::Num);
    is_err(ev("ODDFPRICE", first(0.0785, &[0.0625, 100.0, 2.0, 5.0])), CellError::Num);
    is_err(ev("ODDFPRICE", first(0.0785, &[-0.01, 100.0, 2.0, 1.0])), CellError::Num);
    is_err(ev("ODDFYIELD", first(0.0575, &[0.0, 100.0, 2.0, 0.0])), CellError::Num);
    is_err(ev("ODDFPRICE", vec![d(2009, 4, 1), d(2021, 3, 1), d(2008, 10, 15), d(2009, 3, 1), n(0.07), n(0.06), n(100.0), n(2.0)]), CellError::Num);
    is_err(ev("ODDLPRICE", vec![d(2007, 10, 1), d(2008, 6, 15), d(2007, 10, 15), n(0.0375), n(0.0405), n(100.0), n(2.0)]), CellError::Num);
    is_err(ev("ODDLYIELD", vec![d(2008, 4, 20), d(2008, 6, 15), d(2007, 12, 24), n(0.0375), n(-1.0), n(100.0), n(2.0)]), CellError::Num);
    is_err(
        ev("ODDFPRICE", first(0.0785, &[0.0625, 100.0, 2.0, 1.0]).into_iter().take(4).chain([t("x"), n(0.06), n(100.0), n(2.0)]).collect()),
        CellError::Value,
    );
}

#[test]
fn odd_periods_survive_hostile_input() {
    // Quarterly quasi-periods across the whole date range, huge rates and yields: a value or
    // an error, never a panic or a hang.
    let wide = |x: f64, y: f64| vec![n(2_957_000.0), n(2_958_465.0), n(400.0), n(2_957_900.0), n(x), n(y), n(1e300), n(4.0), n(1.0)];
    for (x, y) in [(0.05, 0.05), (1e300, 1e300), (0.0, 1e-300)] {
        let _ = ev("ODDFPRICE", wide(x, y));
        let _ = ev("ODDFYIELD", wide(x, y));
    }
    // The long walk itself works: settled early in an odd first period of ~32,000 quarters,
    // a small yield round-trips.
    let long = |y: f64| vec![n(500.0), n(2_958_465.0), n(400.0), n(2_957_900.0), n(0.05), n(y), n(100.0), n(4.0), n(1.0)];
    let p = number(ev("ODDFPRICE", long(0.0001)));
    close_tol(ev("ODDFYIELD", long(p)), 0.0001, 1e-9);
    let last = |x: f64, y: f64| vec![n(2_958_000.0), n(2_958_465.0), n(400.0), n(x), n(y), n(1e300), n(4.0), n(1.0)];
    for (x, y) in [(0.05, 0.05), (1e300, 1e300), (0.0, f64::MAX)] {
        let _ = ev("ODDLPRICE", last(x, y));
        let _ = ev("ODDLYIELD", last(x, y));
    }
    is_err(ev("ODDFPRICE", vec![n(-5.0), n(1e20), n(0.0), n(1e10), n(0.05), n(0.05), n(100.0), n(2.0)]), CellError::Num);
}

#[test]
fn euroconvert() {
    // The examples on Microsoft's EUROCONVERT page.
    let ec = |x: f64, s: &str, tg: &str, rest: Vec<Arg>| {
        let mut a = vec![n(x), t(s), t(tg)];
        a.extend(rest);
        ev("EUROCONVERT", a)
    };
    close(ec(1.2, "DEM", "EUR", vec![]), 0.61);
    close(ec(1.0, "FRF", "EUR", vec![b(true), n(3.0)]), 0.152);
    close(ec(1.0, "FRF", "EUR", vec![b(false), n(3.0)]), 0.15);
    close(ec(1.0, "FRF", "DEM", vec![b(true), n(3.0)]), 0.29728616);
    // Euro to a member currency rounds to the target's calculation precision (lira: 0).
    close(ec(10.0, "eur", "ITL", vec![]), 19363.0);
    close(ec(100.0, "EUR", "IEP", vec![b(true)]), 78.7564);
    // Same currency returns the number untouched.
    close(ec(1.23456, "DEM", "DEM", vec![]), 1.23456);
    // Unknown codes and a triangulation precision below 3 are #VALUE!.
    is_err(ec(1.0, "USD", "EUR", vec![]), CellError::Value);
    is_err(ec(1.0, "FRF", "DEM", vec![b(true), n(2.0)]), CellError::Value);
    // Hostile magnitudes give a number or #NUM!, never a panic.
    is_err(ec(f64::MAX, "EUR", "ITL", vec![]), CellError::Num);
    close(ec(1.0, "FRF", "DEM", vec![b(true), n(1e300)]), 1.95583 / 6.55957);
}

#[test]
fn lifting() {
    let v = ev("SLN", vec![av(row(&[100.0, 200.0])), n(0.0), n(10.0)]);
    assert_eq!(rows_of(&v), vec![vec![nv(10.0), nv(20.0)]]);
    is_err(ev("PMT", vec![t("abc"), n(1.0), n(1.0)]), CellError::Value);
    is_err(ev("PMT", vec![e(CellError::NA), n(1.0), n(1.0)]), CellError::NA);
}

#[test]
fn securities_read_dates_in_the_1904_system() {
    let d4 = |y, m, dd| n(serial_from_ymd(DateSystem::D1904, y, m, dd).unwrap());
    let ev4 = |name: &str, args: Vec<Arg>| {
        let mut ctx = TestCtx { sys: DateSystem::D1904, ..TestCtx::default() };
        crate::call(crate::lookup(name).unwrap(), &args, &mut ctx)
    };
    let (s, m) = (d4(2011, 3, 15), d4(2011, 11, 30));
    close(ev4("COUPNCD", vec![s.clone(), m.clone(), n(2.0)]), 39232.0);
    close(ev4("COUPPCD", vec![s.clone(), m.clone(), n(4.0), n(0.0)]), 39140.0);
    close(ev4("COUPDAYBS", vec![s.clone(), m.clone(), n(4.0), n(0.0)]), 15.0);
    let accrint = vec![d4(2008, 3, 1), d4(2008, 8, 31), d4(2008, 5, 1), n(0.1), n(1000.0), n(2.0), n(1.0)];
    close_tol(ev4("ACCRINT", accrint), 16.576086956522, 1e-9);
    // Results that already matched Excel keep matching.
    let semi = |name| ev4(name, vec![s.clone(), m.clone(), n(2.0), n(0.0)]);
    close(semi("COUPPCD"), 39050.0);
    close(semi("COUPDAYS"), 180.0);
    close(semi("COUPDAYBS"), 105.0);
    close(semi("COUPDAYSNC"), 75.0);
    close(semi("COUPNUM"), 2.0);
    let price = vec![s.clone(), m.clone(), n(0.0575), n(0.065), n(100.0), n(2.0), n(0.0)];
    close_tol(ev4("PRICE", price), 99.47768286339101, 1e-9);
    close_tol(ev4("DURATION", vec![s, m, n(0.08), n(0.09), n(2.0), n(0.0)]), 0.6890136809022, 1e-9);
    let odd = vec![d4(2008, 11, 11), d4(2021, 3, 1), d4(2008, 10, 15), d4(2009, 3, 1), n(0.0785), n(0.0625), n(100.0), n(2.0), n(1.0)];
    close_tol(ev4("ODDFPRICE", odd), 113.597717474079, 1e-9);
}

/// Excel truncates ACCRINTM's dates to whole days and accrues nothing when issue equals
/// settlement, in every basis and both date systems; only issue after settlement is #NUM! (#4096).
#[test]
fn accrintm_equal_dates_accrue_zero() {
    let day = serial_from_ymd(DateSystem::D1900, 2025, 1, 15).unwrap();
    for basis in 0..=4 {
        close(ev("ACCRINTM", vec![n(day), n(day), n(0.05), n(1000.0), n(f64::from(basis))]), 0.0);
    }
    // Distinct times on the same day truncate to equal dates.
    close(ev("ACCRINTM", vec![n(day + 0.1), n(day + 0.9), n(0.05), n(1000.0), n(2.0)]), 0.0);
    close_tol(ev("ACCRINTM", vec![n(day), n(day + 1.0), n(0.05), n(1000.0), n(2.0)]), 0.1388888888888889, 1e-15);
    is_err(ev("ACCRINTM", vec![n(day + 1.0), n(day), n(0.05), n(1000.0), n(2.0)]), CellError::Num);
    // Equal dates do not excuse a zero rate or par, or an invalid basis.
    is_err(ev("ACCRINTM", vec![n(day), n(day), n(0.0), n(1000.0), n(2.0)]), CellError::Num);
    is_err(ev("ACCRINTM", vec![n(day), n(day), n(0.05), n(0.0), n(2.0)]), CellError::Num);
    is_err(ev("ACCRINTM", vec![n(day), n(day), n(0.05), n(1000.0), n(5.0)]), CellError::Num);

    let day4 = serial_from_ymd(DateSystem::D1904, 2025, 1, 15).unwrap();
    for basis in 0..=4 {
        let mut ctx = TestCtx { sys: DateSystem::D1904, ..TestCtx::default() };
        let args = vec![n(day4), n(day4), n(0.05), n(1000.0), n(f64::from(basis))];
        close(crate::call(crate::lookup("ACCRINTM").unwrap(), &args, &mut ctx), 0.0);
    }
}
