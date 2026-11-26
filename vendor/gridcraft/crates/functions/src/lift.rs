//! Automatic array lifting: a function given an array where it expects a scalar is applied to
//! each element, broadcasting single rows/columns like Excel's dynamic arrays.

use gridcraft_core::{Array, CellError, Value};

use crate::util::MAX_CELLS;
use crate::{Arg, Ctx, FnSpec};

/// Value-parameter masks (see [`FnSpec::value_args`]), observed in Excel 365: a reference to more
/// than one cell at these positions gives `#VALUE!` in dynamic-array and legacy formulas alike (no
/// implicit intersection), while array constants and computed arrays lift. Range parameters
/// (holidays, schedules, cash flows, coefficients) and the WORKDAY.INTL/NETWORKDAYS.INTL weekend
/// (which lifts a range) are `false`.
pub(crate) const VALUE_ARGS: &[(&str, &[bool])] = &[
    // Date and time.
    ("EOMONTH EDATE YEARFRAC WEEKNUM", &[true]),
    ("WORKDAY NETWORKDAYS", &[true, true, false]),
    ("WORKDAY.INTL NETWORKDAYS.INTL", &[true, true, false, false]),
    // Financial.
    (
        "ACCRINT ACCRINTM AMORDEGRC AMORLINC COUPDAYBS COUPDAYS COUPDAYSNC COUPNCD COUPNUM COUPPCD CUMIPMT CUMPRINC DISC \
         DOLLARDE DOLLARFR DURATION EFFECT INTRATE MDURATION NOMINAL ODDFPRICE ODDFYIELD ODDLPRICE ODDLYIELD PRICE PRICEDISC \
         PRICEMAT RECEIVED TBILLEQ TBILLPRICE TBILLYIELD YIELD YIELDDISC YIELDMAT",
        &[true],
    ),
    ("FVSCHEDULE XNPV", &[true, false]),
    ("XIRR", &[false, false, true]),
    // Math and information.
    ("ISEVEN ISODD QUOTIENT MROUND FACTDOUBLE SQRTPI RANDBETWEEN", &[true]),
    ("SERIESSUM", &[true, true, true, false]),
    // Engineering (IMSUM and IMPRODUCT aggregate, so they aren't here).
    (
        "BIN2DEC BIN2HEX BIN2OCT DEC2BIN DEC2HEX DEC2OCT HEX2BIN HEX2DEC HEX2OCT OCT2BIN OCT2DEC OCT2HEX CONVERT DELTA GESTEP \
         ERF ERF.PRECISE ERFC ERFC.PRECISE BESSELI BESSELJ BESSELK BESSELY COMPLEX IMABS IMAGINARY IMARGUMENT IMCONJUGATE \
         IMCOS IMCOSH IMCOT IMCSC IMCSCH IMDIV IMEXP IMLN IMLOG10 IMLOG2 IMPOWER IMREAL IMSEC IMSECH IMSIN IMSINH IMSQRT IMSUB \
         IMTAN",
        &[true],
    ),
];

/// The value-parameter mask of `name` (empty for functions without value parameters).
pub(crate) fn value_args(name: &str) -> &'static [bool] {
    VALUE_ARGS.iter().find(|(names, _)| names.split_ascii_whitespace().any(|n| n == name)).map_or(&[], |(_, mask)| mask)
}

/// A reference to more than one cell at a value parameter becomes a scalar `#VALUE!`, so it adds
/// no shape: `EOMONTH(A1:A3, {1;2})` is a 2x1 array of `#VALUE!`, as in Excel.
fn reject_multi_cell_refs(spec: &FnSpec, args: &[Arg]) -> Option<Vec<Arg>> {
    let multi = |i: usize, a: &Arg| a.from_ref && spec.is_value_arg(i) && matches!(&a.value, Value::Array(arr) if arr.data.len() > 1);
    if !args.iter().enumerate().any(|(i, a)| multi(i, a)) {
        return None;
    }
    Some(args.iter().enumerate().map(|(i, a)| if multi(i, a) { Arg::val(Value::Error(CellError::Value)) } else { a.clone() }).collect())
}

/// Lookups whose every call searches a lookup vector from its start, so a call lifted over `n`
/// needles does `n` x (vector length) work. Their search arguments don't lift.
const SCANS: &[&str] = &["VLOOKUP", "HLOOKUP", "LOOKUP", "MATCH", "XLOOKUP", "XMATCH"];

/// Work budget of a lifted [`SCANS`] lookup: needle cells times the length of the longest vector
/// it searches (the longest dimension of its largest non-lifted array). Each needle re-reads and
/// scans the vector (about 8 ns per entry), so without a cap a one-cell formula such as
/// `XMATCH(SEQUENCE(1048576), -SEQUENCE(1048576))` runs for hours and hangs opening the file.
/// Past the budget the call is `#NUM!`, as MMULT is past 2e9 multiply-adds. 5e8 keeps the worst
/// case to a few seconds and still admits 5,000 needles over a 100,000-row table.
///
/// The same budget covers all the work of a lifted call added up across its elements: criteria
/// aggregates rescanning their ranges, period loops, and array results built per element.
const LIFTED_SCAN_BUDGET: usize = 500_000_000;

/// Criteria aggregates: every call tests each cell of its ranges against its criteria, so a call
/// lifted over `n` criteria does `n` x (range cells) work, like the [`SCANS`] lookups.
const CRITERIA: &[&str] = &["COUNTIF", "COUNTIFS", "SUMIF", "SUMIFS", "AVERAGEIF", "AVERAGEIFS", "MAXIFS", "MINIFS"];

/// Cumulative TVM functions that step through every period from `start_period` (argument 3) to
/// `end_period` (argument 4) on each call.
const PERIOD_LOOPS: &[&str] = &["CUMIPMT", "CUMPRINC"];

/// Budget units one period of a [`PERIOD_LOOPS`] call costs: each period computes a payment and a
/// balance with powers, many times a scanned cell.
const PERIOD_COST: usize = 16;

/// The work every lifted call of `spec` repeats over its non-lifted arguments: the longest vector
/// a [`SCANS`] lookup searches, or every cell a [`CRITERIA`] aggregate tests. 0 for other
/// functions.
fn scan_len(spec: &FnSpec, args: &[Arg]) -> usize {
    let scans = SCANS.contains(&spec.name);
    if !scans && !CRITERIA.contains(&spec.name) {
        return 0;
    }
    // SUMIF and AVERAGEIF read the optional third range only at the coordinates of the first.
    let first = args.first().and_then(|a| if let Value::Array(arr) = &a.value { Some(arr.data.len()) } else { None });
    let lens = args.iter().enumerate().filter(|(i, _)| !spec.is_scalar_arg(*i)).filter_map(|(i, a)| match &a.value {
        Value::Array(arr) if scans => Some(arr.rows.max(arr.cols)),
        Value::Array(arr) if i == 2 && matches!(spec.name, "SUMIF" | "AVERAGEIF") => Some(arr.data.len().min(first.unwrap_or(usize::MAX))),
        Value::Array(arr) => Some(arr.data.len()),
        _ => None,
    });
    if scans { lens.max().unwrap_or(0) } else { lens.fold(0, usize::saturating_add) }
}

/// The work of one lifted call that depends on its own (lifted) scalar arguments: the number of
/// periods a [`PERIOD_LOOPS`] function steps through. 0 for other functions.
fn call_cost(spec: &FnSpec, cell_args: &[Arg]) -> usize {
    if !PERIOD_LOOPS.contains(&spec.name) {
        return 0;
    }
    // Coercing numeric text reads all of it, even in a call the function then refuses, so each
    // character is charged as well: of the arguments it coerces in order, up to the first that fails.
    let mut parse = 0usize;
    for (i, a) in cell_args.iter().enumerate().take(6) {
        if let Value::Text(t) = &a.value {
            parse = parse.saturating_add(t.len());
        }
        if crate::util::num(cell_args, i).is_err() {
            break;
        }
    }
    // The function's own validation and coercion (text like "10000000" counts), so a call it
    // refuses before its loop costs nothing and no loop goes uncharged.
    let periods = crate::financial::cumulative_args(cell_args).map_or(0, |(.., start, end, _)| (end - start + 1.0) as usize);
    parse.saturating_add(periods.saturating_mul(PERIOD_COST))
}

pub(crate) fn call_lifted(spec: &FnSpec, args: &[Arg], ctx: &mut dyn Ctx) -> Value {
    call_lifted_within(spec, args, ctx, LIFTED_SCAN_BUDGET)
}

/// Sets the lifted arguments of `cell_args` to their elements at (`r`, `c`).
fn element_args(spec: &FnSpec, args: &[Arg], r: usize, c: usize, cell_args: &mut [Arg]) {
    for (i, a) in args.iter().enumerate() {
        if spec.is_scalar_arg(i)
            && let Value::Array(arr) = &a.value
            && let Some(slot) = cell_args.get_mut(i)
        {
            let v = arr.get_broadcast(r, c);
            slot.value = if let Value::Array(inner) = &v { inner.data.first().cloned().unwrap_or(Value::Empty) } else { v };
            // A blank element of a written array argument is given, as a scalar blank is.
            slot.blank_value = !slot.from_ref && matches!(slot.value, Value::Empty);
        }
    }
}

/// [`call_lifted`] with every call's work added up against `budget` (see [`LIFTED_SCAN_BUDGET`]).
fn call_lifted_within(spec: &FnSpec, args: &[Arg], ctx: &mut dyn Ctx, budget: usize) -> Value {
    let rejected = if spec.value_args.is_empty() { None } else { reject_multi_cell_refs(spec, args) };
    let args = rejected.as_deref().unwrap_or(args);
    let mut rows = 0usize;
    let mut cols = 0usize;
    let mut any = false;
    for (i, a) in args.iter().enumerate() {
        if spec.is_scalar_arg(i)
            && let Value::Array(arr) = &a.value
        {
            any = true;
            rows = rows.max(arr.rows);
            cols = cols.max(arr.cols);
        }
    }
    if !any {
        return (spec.imp)(args, ctx);
    }
    let needles = rows.saturating_mul(cols);
    // Every call is charged its repeated scan (at least 1), its own period loop, and the cells of
    // an array result (built in full, though only its first cell is kept), so lifting a scalar
    // argument can't multiply what one call may do into hours of work:
    // `COUNTIF(big_range, SEQUENCE(1e4))`, `CUMIPMT(0.1, 1e7, 1, 1, SEQUENCE(1e4,,1e7,0), 0)`,
    // `MUNIT(SEQUENCE(1e4,,3000,0))`.
    let per_call = scan_len(spec, args).max(1);
    let mut cell_args: Vec<Arg> = args.to_vec();
    let mut spent = needles.saturating_mul(per_call);
    if needles > MAX_CELLS || spent > budget {
        return Value::Error(CellError::Num);
    }
    // Period loops are known before any call runs, so a call over budget does none of them.
    if PERIOD_LOOPS.contains(&spec.name) {
        for r in 0..rows {
            for c in 0..cols {
                element_args(spec, args, r, c, &mut cell_args);
                spent = spent.saturating_add(call_cost(spec, &cell_args));
                if spent > budget {
                    return Value::Error(CellError::Num);
                }
            }
        }
    }
    let mut data = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            element_args(spec, args, r, c, &mut cell_args);
            let out = (spec.imp)(&cell_args, ctx);
            data.push(match out {
                Value::Array(a) => {
                    spent = spent.saturating_add(a.data.len());
                    if spent > budget {
                        return Value::Error(CellError::Num);
                    }
                    a.data.first().cloned().unwrap_or(Value::Empty)
                }
                v => v,
            });
        }
    }
    if rows == 1 && cols == 1 {
        return data.pop().unwrap_or(Value::Empty);
    }
    Array::new(rows, cols, data).map(Value::from).unwrap_or(Value::Error(CellError::Calc))
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellError, Value};

    use crate::util::testutil::*;
    use crate::util::{R, num};
    use crate::{Arg, Ctx, FnSpec};

    fn add(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
        Ok(Value::Number(num(a, 0)? + num(a, 1)?))
    }

    fn spec() -> FnSpec {
        f!("ADD2", 2, 2, MathTrig, &[true, true], "ADD2(a, b)", "Adds two numbers.", add)
    }

    #[test]
    fn broadcasts_row_and_column() {
        let s = spec();
        let out = crate::call(&s, &[av(row(&[1.0, 2.0, 3.0])), av(col(&[10.0, 20.0]))], &mut TestCtx::default());
        assert_eq!(rows_of(&out), vec![vec![nv(11.0), nv(12.0), nv(13.0)], vec![nv(21.0), nv(22.0), nv(23.0)]]);
    }

    #[test]
    fn mismatched_sizes_pad_with_na() {
        let s = spec();
        let out = crate::call(&s, &[av(row(&[1.0, 2.0, 3.0])), av(row(&[1.0, 1.0]))], &mut TestCtx::default());
        assert_eq!(rows_of(&out), vec![vec![nv(2.0), nv(3.0), Value::Error(CellError::NA)]]);
    }

    #[test]
    fn single_cell_unwraps_and_arity() {
        let s = spec();
        let out = crate::call(&s, &[rf(col(&[4.0])), n(1.0)], &mut TestCtx::default());
        assert_eq!(out, nv(5.0));
        is_err(crate::call(&s, &[n(1.0)], &mut TestCtx::default()), CellError::Value);
        let huge = crate::call(
            &s,
            &[av(gridcraft_core::Array::filled(1, 100_000, nv(1.0)).into()), av(gridcraft_core::Array::filled(100_000, 1, nv(1.0)).into())],
            &mut TestCtx::default(),
        );
        is_err(huge, CellError::Num);
    }

    /// Security: a lifted lookup searches its whole lookup vector once per needle, so needles x
    /// vector length past `LIFTED_SCAN_BUDGET` is `#NUM!` at once.
    /// `XMATCH(SEQUENCE(1048576), -SEQUENCE(1048576))` used to run for hours (and hung opening
    /// the 1.6 KB file holding it).
    #[test]
    fn lifted_lookups_stop_past_their_work_budget() {
        use gridcraft_core::Array;
        let tall = |n: usize| av(Array::filled(n, 1, nv(-1.0)).into());
        let wide = |n: usize| av(Array::filled(1, n, nv(-1.0)).into());
        is_err(ev("XMATCH", vec![tall(1_048_576), tall(1_048_576)]), CellError::Num);
        // Just past the budget, for every lookup: 501 needles over 1,000,000 entries.
        assert_eq!(super::LIFTED_SCAN_BUDGET, 500 * 1_000_000);
        is_err(ev("XMATCH", vec![tall(501), tall(1_000_000)]), CellError::Num);
        is_err(ev("MATCH", vec![tall(501), tall(1_000_000), n(0.0)]), CellError::Num);
        is_err(ev("XLOOKUP", vec![tall(501), tall(1_000_000), tall(1_000_000)]), CellError::Num);
        is_err(ev("VLOOKUP", vec![tall(501), tall(1_000_000), n(1.0), b(false)]), CellError::Num);
        is_err(ev("HLOOKUP", vec![tall(501), wide(1_000_000), n(1.0), b(false)]), CellError::Num);
        is_err(ev("LOOKUP", vec![tall(501), tall(1_000_000)]), CellError::Num);
        // Within the budget lifted lookups work as before.
        let keys = || av(col(&[10.0, 20.0, 30.0]));
        let pos = ev("XMATCH", vec![av(col(&[30.0, 10.0, 99.0])), keys()]);
        assert_eq!(rows_of(&pos), vec![vec![nv(3.0)], vec![nv(1.0)], vec![Value::Error(CellError::NA)]]);
        let found = ev("XLOOKUP", vec![av(row(&[20.0, 30.0])), keys(), av(col(&[1.0, 2.0, 3.0]))]);
        assert_eq!(rows_of(&found), vec![vec![nv(2.0), nv(3.0)]]);
        // Other lifted functions aren't metered: INDEX reads one entry per position.
        let picks: Vec<f64> = (1..=1000u32).map(f64::from).collect();
        let out = ev("INDEX", vec![tall(1_048_576), av(col(&picks))]);
        assert_eq!(rows_of(&out), vec![vec![nv(-1.0)]; 1000]);
    }

    /// Security: lifting a criteria aggregate rescans its range for every criterion, so criteria
    /// x range cells past the budget is `#NUM!` before any scan runs.
    #[test]
    fn lifted_criteria_aggregates_stop_past_their_work_budget() {
        use gridcraft_core::Array;
        let tall = |n: usize| av(Array::filled(n, 1, nv(-1.0)).into());
        is_err(ev("COUNTIF", vec![tall(1_000_000), tall(501)]), CellError::Num);
        is_err(ev("SUMIF", vec![tall(1_000_000), tall(501)]), CellError::Num);
        is_err(ev("COUNTIFS", vec![tall(1_000_000), tall(501)]), CellError::Num);
        // Two ranges of 250,000 cells each count together.
        is_err(ev("SUMIFS", vec![tall(250_000), tall(250_000), tall(1001)]), CellError::Num);
        // SUMIF reads its sum range only where the criteria range has cells: a one-cell range over a
        // million-cell sum range is one cell of work per criterion.
        let one = av(Array::filled(1, 1, nv(-1.0)).into());
        assert_eq!(rows_of(&ev("SUMIF", vec![one, tall(501), tall(1_000_000)])).len(), 501);
        let counts = ev("COUNTIF", vec![av(col(&[1.0, 2.0, 2.0])), av(col(&[2.0, 1.0, 5.0]))]);
        assert_eq!(rows_of(&counts), vec![vec![nv(2.0)], vec![nv(1.0)], vec![nv(0.0)]]);
        for name in super::CRITERIA.iter().chain(super::PERIOD_LOOPS) {
            assert!(crate::lookup(name).is_some(), "{name} isn't registered");
        }
    }

    /// Security: CUMIPMT and CUMPRINC loop over their periods once per lifted element; the
    /// periods of all elements are added up first, so an over-budget call runs none of them.
    #[test]
    fn lifted_period_loops_stop_past_their_work_budget() {
        use gridcraft_core::Array;
        let ends = |len: usize, end: f64| av(Array::filled(len, 1, nv(end)).into());
        let args = |len: usize, end: f64| vec![n(0.01), n(1e7), n(1000.0), n(1.0), ends(len, end), n(0.0)];
        // 51 elements of 1e7 periods each is past 5e8.
        is_err(ev("CUMIPMT", args(51, 1e7)), CellError::Num);
        is_err(ev("CUMPRINC", args(51, 1e7)), CellError::Num);
        // A period costs more than a scanned cell, so fewer elements are enough.
        is_err(ev("CUMIPMT", args(49, 1e7)), CellError::Num);
        // Numeric text is coerced by the function, so its periods are charged the same.
        let text_ends = av(Array::filled(51, 1, Value::text("10000000")).into());
        is_err(ev("CUMIPMT", vec![n(0.01), n(1e7), n(1000.0), n(1.0), text_ends, n(0.0)]), CellError::Num);
        let out = ev("CUMIPMT", args(2, 12.0));
        assert_eq!(rows_of(&out).len(), 2);
        assert!(matches!(rows_of(&out).first().and_then(|r| r.first()), Some(Value::Number(x)) if *x < 0.0));
    }

    /// Security: numeric text is parsed once per lifted element even when the call is refused; the
    /// characters read are charged, so a long text repeated down a column can't cost gigabytes of parsing.
    #[test]
    fn lifted_period_loops_charge_the_text_they_parse() {
        use gridcraft_core::Array;
        // 20,000 elements x 32,000 characters is past 5e8 though no call reaches its loop (rate is negative).
        let long = Value::text(format!("{}5", "0".repeat(32_000)));
        let ends = av(Array::filled(20_000, 1, long).into());
        is_err(ev("CUMIPMT", vec![n(-0.01), n(10.0), n(1000.0), n(1.0), ends, n(0.0)]), CellError::Num);
        // A short text period costs next to nothing.
        let ends = av(Array::filled(3, 1, Value::text("12")).into());
        assert_eq!(rows_of(&ev("CUMIPMT", vec![n(0.01), n(20.0), n(1000.0), n(1.0), ends, n(0.0)])).len(), 3);
    }

    /// Text after the first argument that fails to coerce is never read, so it is not charged: the
    /// elements keep their own errors instead of one budget `#NUM!`.
    #[test]
    fn lifted_period_loops_charge_only_the_text_they_reach() {
        use gridcraft_core::Array;
        let rates = av(Array::filled(20_000, 1, Value::text("x")).into());
        let long = Value::text(format!("{}5", "0".repeat(32_000)));
        let out = ev("CUMIPMT", vec![rates, n(10.0), n(1000.0), n(1.0), av(long), n(0.0)]);
        assert!(rows_of(&out).iter().all(|r| r.first() == Some(&Value::Error(CellError::Value))));
    }

    /// A call the function refuses before its loop (here: end period past the period count) is
    /// not charged, so rejected elements can't push a valid one over the budget.
    #[test]
    fn lifted_period_loops_do_not_charge_rejected_calls() {
        use gridcraft_core::Array;
        let mut ends = vec![nv(1e7 + 1.0); 60];
        ends.push(nv(12.0));
        let ends = av(Array::new(ends.len(), 1, ends).unwrap_or_else(|| panic!("shape")).into());
        let out = ev("CUMIPMT", vec![n(0.01), n(1e7), n(1000.0), n(1.0), ends, n(0.0)]);
        let rows = rows_of(&out);
        assert_eq!(rows.len(), 61);
        assert!(matches!(rows.last().and_then(|r| r.first()), Some(Value::Number(x)) if *x < 0.0));
    }

    /// Security: an array-producing function lifted over a computed array builds each element's
    /// whole result before keeping one cell; their cells count against the budget.
    #[test]
    fn lifted_array_results_count_against_the_budget() {
        use gridcraft_core::Array;
        let spec = crate::lookup("MUNIT").unwrap_or_else(|| panic!("MUNIT isn't registered"));
        let dims = |n: usize| [av(Array::filled(n, 1, nv(10.0)).into())];
        // Ten 10x10 identity matrices are 1,000 cells.
        let out = super::call_lifted_within(spec, &dims(10), &mut TestCtx::default(), 1_010);
        assert_eq!(rows_of(&out), vec![vec![nv(1.0)]; 10]);
        is_err(super::call_lifted_within(spec, &dims(11), &mut TestCtx::default(), 1_010), CellError::Num);
    }

    #[test]
    fn value_params_reject_multi_cell_references() {
        let s = FnSpec { value_args: &[true, false], ..spec() };
        let mut ctx = TestCtx::default();
        // A multi-cell reference is #VALUE!; a one-cell reference and a computed array are fine.
        is_err(crate::call(&s, &[rf(col(&[1.0, 2.0, 3.0])), n(1.0)], &mut ctx), CellError::Value);
        assert_eq!(crate::call(&s, &[rf(col(&[4.0])), n(1.0)], &mut ctx), nv(5.0));
        assert_eq!(rows_of(&crate::call(&s, &[av(col(&[1.0, 2.0])), n(1.0)], &mut ctx)), vec![vec![nv(2.0)], vec![nv(3.0)]]);
        // The rejected reference adds no shape: the other argument still lifts.
        let v = Value::Error(CellError::Value);
        assert_eq!(rows_of(&crate::call(&s, &[rf(col(&[1.0, 2.0, 3.0])), av(col(&[1.0, 2.0]))], &mut ctx)), vec![vec![v.clone()], vec![v]]);
        // A position that isn't a value parameter still lifts the reference.
        assert_eq!(rows_of(&crate::call(&s, &[n(1.0), rf(row(&[1.0, 2.0]))], &mut ctx)), vec![vec![nv(2.0), nv(3.0)]]);
    }

    #[test]
    fn value_arg_table_names_registered_scalar_positions() {
        for (names, mask) in super::VALUE_ARGS {
            assert!(!mask.is_empty());
            for name in names.split_ascii_whitespace() {
                let Some(s) = crate::lookup(name) else { panic!("{name} isn't registered") };
                assert_eq!(s.value_args, *mask, "{name}");
                for i in 0..s.max_args.min(12) {
                    assert!(!s.is_value_arg(i) || s.is_scalar_arg(i), "{name} argument {i} is a value parameter but doesn't lift");
                }
            }
        }
        assert!(crate::lookup("WORKDAY").is_some_and(|s| s.is_value_arg(1) && !s.is_value_arg(2)));
        assert!(crate::lookup("WORKDAY.INTL").is_some_and(|s| !s.is_value_arg(2) && s.is_scalar_arg(2)));
        assert!(crate::lookup("SUM").is_some_and(|s| s.value_args.is_empty()));
        assert!(crate::lookup("ISOWEEKNUM").is_some_and(|s| s.value_args.is_empty()));
    }
}
