use super::{Arg, C, CellError, R, Value, arg, array_numbers, array_val, as_array, has, num, num_val, opt_num};

// ---------------------------------------------------------------------------------------------
// Basic helpers over number lists
// ---------------------------------------------------------------------------------------------

pub(super) fn mean(x: &[f64]) -> R<f64> {
    if x.is_empty() {
        return Err(CellError::Div0);
    }
    Ok(x.iter().sum::<f64>() / x.len() as f64)
}

pub(super) fn devsq(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let m = x.iter().sum::<f64>() / x.len() as f64;
    x.iter().map(|v| (v - m) * (v - m)).sum()
}

pub(super) fn var_s(x: &[f64]) -> R<f64> {
    if x.len() < 2 {
        return Err(CellError::Div0);
    }
    Ok(devsq(x) / (x.len() - 1) as f64)
}

pub(super) fn var_p(x: &[f64]) -> R<f64> {
    if x.is_empty() {
        return Err(CellError::Div0);
    }
    Ok(devsq(x) / x.len() as f64)
}

pub(super) fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v
}

/// Sorts `v` if it is already monotone in `total_cmp` order (a descending slice is reversed) and
/// says whether it did. At most two passes, each stopping at the first out-of-order pair, so
/// unordered data costs next to nothing. Needed because the std sort recognises ascending and
/// descending runs in O(n) with a small constant; selection is O(n) too but measured 1.2-1.8x
/// slower on such input.
fn sorted_if_monotone(v: &mut [f64]) -> bool {
    if v.is_sorted_by(|a, b| a.total_cmp(b).is_le()) {
        return true;
    }
    if v.is_sorted_by(|a, b| a.total_cmp(b).is_ge()) {
        v.reverse();
        return true;
    }
    false
}

pub(super) fn median(v: Vec<f64>) -> R<f64> {
    if v.is_empty() {
        return Err(CellError::Num);
    }
    let mut v = v;
    let n = v.len();
    if sorted_if_monotone(&mut v) {
        let hi = v.get(n / 2).copied().unwrap_or(0.0);
        return if n % 2 == 1 { Ok(hi) } else { Ok(v.get(n / 2 - 1).copied().unwrap_or(0.0).midpoint(hi)) };
    }
    let (lower, hi, _) = v.select_nth_unstable_by(n / 2, f64::total_cmp);
    let hi = *hi;
    if n % 2 == 1 { Ok(hi) } else { Ok(lower.iter().copied().max_by(f64::total_cmp).unwrap_or(0.0).midpoint(hi)) }
}

/// Values occurring at least twice, ordered by first occurrence, with their (equal, maximal) count.
fn modes(v: &[f64]) -> Vec<f64> {
    // Index of each distinct number in `counts`, so counting is linear rather than a search of
    // every distinct number so far. Keyed by bits with -0 folded into 0 (they compare equal); NaN
    // never equals anything, so it is never merged.
    let mut index: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for &x in v {
        if x.is_nan() {
            counts.push((x, 1));
            continue;
        }
        let key = if x == 0.0 { 0 } else { x.to_bits() };
        match index.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => {
                if let Some(c) = counts.get_mut(*e.get()) {
                    c.1 += 1;
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(counts.len());
                counts.push((x, 1));
            }
        }
    }
    let best = counts.iter().map(|e| e.1).max().unwrap_or(0);
    if best < 2 {
        return Vec::new();
    }
    counts.into_iter().filter(|e| e.1 == best).map(|e| e.0).collect()
}

pub(super) fn mode_sngl(v: &[f64]) -> R<f64> {
    modes(v).first().copied().ok_or(CellError::NA)
}

/// Numbers for MODE / MODE.SNGL / MODE.MULT. Inside multi-cell ranges and arrays only numbers
/// count, like [`numbers`]. Scalar arguments differ from SUM: in Excel 365, text and logical
/// values are skipped whether typed or referenced (even numeric text such as `"1"`, so
/// `MODE(1,"1",2,3)` is `#N/A`, and `MODE(1,TRUE,TRUE,2)` is `#N/A`), but a single-cell reference
/// to a blank cell is `#VALUE!` (`MODE(B9,2,D7)` with D7 blank). Arguments are processed left to
/// right, so the first error wins. The results cached in POI's FormulaEvalTestData come from an
/// older Excel that answered `#VALUE!` for typed text; they are not the reference here.
pub(super) fn mode_numbers(args: &[Arg]) -> R<Vec<f64>> {
    let mut out = Vec::new();
    for a in args {
        match &a.value {
            Value::Array(arr) => {
                for v in arr.iter() {
                    match v {
                        Value::Number(n) => out.push(*n),
                        Value::Error(e) => return Err(*e),
                        _ => {}
                    }
                }
            }
            Value::Number(n) => out.push(*n),
            Value::Error(e) => return Err(*e),
            Value::Text(_) | Value::Bool(_) => {}
            Value::Empty => return Err(CellError::Value),
        }
    }
    Ok(out)
}

fn product(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().product() }
}

pub(super) fn min_of(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().copied().fold(f64::INFINITY, f64::min) }
}

pub(super) fn max_of(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().copied().fold(f64::NEG_INFINITY, f64::max) }
}

/// k-th largest (k is rounded up like Excel).
pub(super) fn kth(v: Vec<f64>, k: f64, largest: bool) -> R<f64> {
    let k = k.ceil();
    if k.is_nan() || k < 1.0 || k > v.len() as f64 {
        return Err(CellError::Num);
    }
    let mut v = v;
    let k = k as usize;
    let idx = if largest { v.len() - k } else { k - 1 };
    // Selection instead of a full sort (monotone input is read directly); same total order, so the
    // same bit pattern as `sorted(v)[idx]`.
    if sorted_if_monotone(&mut v) {
        return v.get(idx).copied().ok_or(CellError::Num);
    }
    Ok(*v.select_nth_unstable_by(idx, f64::total_cmp).1)
}

/// Value at fractional rank `pos` of the sorted data, linearly interpolated, found by selection.
fn interp(mut v: Vec<f64>, pos: f64) -> R<f64> {
    let i = pos.floor();
    let frac = pos - i;
    let i = i as usize;
    if i >= v.len() {
        return Err(CellError::Num);
    }
    if sorted_if_monotone(&mut v) {
        let lo = v.get(i).copied().ok_or(CellError::Num)?;
        return match v.get(i + 1) {
            Some(hi) if frac > 0.0 => Ok(lo + frac * (hi - lo)),
            _ => Ok(lo),
        };
    }
    let (_, lo, upper) = v.select_nth_unstable_by(i, f64::total_cmp);
    let lo = *lo;
    // The next order statistic is the minimum of the upper partition.
    if frac > 0.0
        && let Some(hi) = upper.iter().copied().min_by(f64::total_cmp)
    {
        return Ok(lo + frac * (hi - lo));
    }
    Ok(lo)
}

pub(super) fn percentile_inc(v: Vec<f64>, k: f64) -> R<f64> {
    if v.is_empty() || !(0.0..=1.0).contains(&k) {
        return Err(CellError::Num);
    }
    let pos = k * (v.len() - 1) as f64;
    interp(v, pos)
}

pub(super) fn percentile_exc(v: Vec<f64>, k: f64) -> R<f64> {
    let n = v.len() as f64;
    if v.is_empty() || !(k > 0.0 && k < 1.0) {
        return Err(CellError::Num);
    }
    let pos = k * (n + 1.0);
    if pos < 1.0 || pos > n {
        return Err(CellError::Num);
    }
    interp(v, pos - 1.0)
}

pub(super) fn quartile_inc(v: Vec<f64>, q: f64) -> R<f64> {
    let q = q.trunc();
    if !(0.0..=4.0).contains(&q) {
        return Err(CellError::Num);
    }
    percentile_inc(v, q / 4.0)
}

pub(super) fn quartile_exc(v: Vec<f64>, q: f64) -> R<f64> {
    let q = q.trunc();
    if !(1.0..=3.0).contains(&q) {
        return Err(CellError::Num);
    }
    percentile_exc(v, q / 4.0)
}

pub(super) fn arg_nums(a: &[Arg], i: usize) -> R<Vec<f64>> {
    array_numbers(&arg(a, i)?.value)
}

/// Pairs of numbers at matching positions of two arrays (pairs with a non-number skipped).
/// Different sizes give `#N/A`; errors propagate.
fn pairs(x: &Value, y: &Value) -> R<(Vec<f64>, Vec<f64>)> {
    let xa = as_array(x);
    let ya = as_array(y);
    if xa.data.len() != ya.data.len() {
        return Err(CellError::NA);
    }
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (a, b) in xa.data.iter().zip(ya.data.iter()) {
        if let Value::Error(e) = a {
            return Err(*e);
        }
        if let Value::Error(e) = b {
            return Err(*e);
        }
        if let (Value::Number(p), Value::Number(q)) = (a, b) {
            xs.push(*p);
            ys.push(*q);
        }
    }
    Ok((xs, ys))
}

pub(super) fn arg_pairs(a: &[Arg], i: usize, j: usize) -> R<(Vec<f64>, Vec<f64>)> {
    pairs(&arg(a, i)?.value, &arg(a, j)?.value)
}

/// (sum of cross deviations, devsq x, devsq y, mean x, mean y).
pub(super) fn moments(x: &[f64], y: &[f64]) -> R<(f64, f64, f64, f64, f64)> {
    let mx = mean(x)?;
    let my = mean(y)?;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for (a, b) in x.iter().zip(y) {
        sxy += (a - mx) * (b - my);
        sxx += (a - mx) * (a - mx);
        syy += (b - my) * (b - my);
    }
    Ok((sxy, sxx, syy, mx, my))
}

pub(super) fn correl(x: &[f64], y: &[f64]) -> R<f64> {
    if x.len() < 2 {
        return Err(CellError::Div0);
    }
    let (sxy, sxx, syy, _, _) = moments(x, y)?;
    if sxx == 0.0 || syy == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(sxy / (sxx * syy).sqrt())
}

pub(super) fn slope(y: &[f64], x: &[f64]) -> R<f64> {
    let (sxy, sxx, _, _, _) = moments(x, y)?;
    if sxx == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(sxy / sxx)
}

pub(super) fn intercept(y: &[f64], x: &[f64]) -> R<f64> {
    let (sxy, sxx, _, mx, my) = moments(x, y)?;
    if sxx == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(my - sxy / sxx * mx)
}

fn trunc_digits(r: f64, sig: f64) -> f64 {
    let f = 10f64.powi(sig as i32);
    if !f.is_finite() {
        return r;
    }
    ((r * f) + 1e-9).floor() / f
}

fn col_value(v: Vec<f64>) -> R<Value> {
    let n = v.len();
    array_val(n, 1, v.into_iter().map(Value::Number).collect())
}

// ---------------------------------------------------------------------------------------------
// Aggregates
// ---------------------------------------------------------------------------------------------

pub(super) fn count_impl(a: &[Arg]) -> f64 {
    let mut c = 0usize;
    for x in a {
        match &x.value {
            Value::Array(arr) => c += arr.iter().filter(|v| v.is_number()).count(),
            Value::Number(_) => c += 1,
            _ if x.from_ref => {}
            Value::Bool(_) => c += 1,
            Value::Text(t) if gridcraft_core::parse::parse_number_text(t).is_some() => c += 1,
            _ => {}
        }
    }
    c as f64
}

pub(super) fn counta_impl(a: &[Arg]) -> f64 {
    let mut c = 0usize;
    for x in a {
        match &x.value {
            Value::Array(arr) => c += arr.iter().filter(|v| !v.is_empty()).count(),
            Value::Empty if x.from_ref => {}
            _ => c += 1,
        }
    }
    c as f64
}

pub(super) fn countblank(a: &[Arg], _: C) -> R<Value> {
    let v = &arg(a, 0)?.value;
    let blank = |x: &Value| matches!(x, Value::Empty) || matches!(x, Value::Text(t) if t.is_empty());
    let c = match v {
        Value::Array(arr) => arr.iter().filter(|x| blank(x)).count(),
        x => usize::from(blank(x)),
    };
    num_val(c as f64)
}

pub(super) fn mode_mult(a: &[Arg], _: C) -> R<Value> {
    let v = mode_numbers(a)?;
    let m = modes(&v);
    if m.is_empty() {
        return Err(CellError::NA);
    }
    col_value(m)
}

pub(super) fn large(a: &[Arg], _: C) -> R<Value> {
    num_val(kth(arg_nums(a, 0)?, num(a, 1)?, true)?)
}

pub(super) fn small(a: &[Arg], _: C) -> R<Value> {
    num_val(kth(arg_nums(a, 0)?, num(a, 1)?, false)?)
}

pub(super) fn rank_impl(a: &[Arg], avg: bool) -> R<Value> {
    let x = num(a, 0)?;
    let v = arg_nums(a, 1)?;
    let asc = opt_num(a, 2, 0.0)? != 0.0;
    let eq = v.iter().filter(|&&y| y == x).count();
    if eq == 0 {
        return Err(CellError::NA);
    }
    let before = v.iter().filter(|&&y| if asc { y < x } else { y > x }).count();
    let r = before as f64 + 1.0;
    num_val(if avg { r + (eq as f64 - 1.0) / 2.0 } else { r })
}

pub(super) fn percentrank_impl(a: &[Arg], exc: bool) -> R<Value> {
    let s = sorted(arg_nums(a, 0)?);
    let x = num(a, 1)?;
    let sig = opt_num(a, 2, 3.0)?.trunc();
    if sig < 1.0 || s.is_empty() {
        return Err(CellError::Num);
    }
    // Excel: 10^significance overflows from 309 up.
    if sig >= 309.0 {
        return Err(CellError::Div0);
    }
    let (first, last) = (s.first().copied().unwrap_or(0.0), s.last().copied().unwrap_or(0.0));
    if x < first || x > last {
        return Err(CellError::NA);
    }
    let n = s.len() as f64;
    let rank_of = |v: f64| -> f64 {
        let less = s.iter().filter(|&&y| y < v).count() as f64;
        if exc {
            (less + 1.0) / (n + 1.0)
        } else if n <= 1.0 {
            1.0
        } else {
            less / (n - 1.0)
        }
    };
    let r = if s.contains(&x) {
        rank_of(x)
    } else {
        let lo = s.iter().copied().filter(|&y| y < x).fold(f64::NEG_INFINITY, f64::max);
        let hi = s.iter().copied().filter(|&y| y > x).fold(f64::INFINITY, f64::min);
        let (rl, rh) = (rank_of(lo), rank_of(hi));
        rl + (x - lo) / (hi - lo) * (rh - rl)
    };
    // From significance 15 up Excel returns 15 significant digits, rounded.
    if sig >= 15.0 {
        return num_val(format!("{r:.14e}").parse().unwrap_or(r));
    }
    num_val(trunc_digits(r, sig))
}

pub(super) fn trimmean(a: &[Arg], _: C) -> R<Value> {
    let v = arg_nums(a, 0)?;
    let p = num(a, 1)?;
    if !(0.0..1.0).contains(&p) || v.is_empty() {
        return Err(CellError::Num);
    }
    let n = v.len();
    let per_side = ((n as f64 * p) / 2.0).floor() as usize;
    let s = sorted(v);
    let kept = s.get(per_side..n - per_side).unwrap_or(&[]);
    num_val(mean(kept)?)
}

pub(super) fn kurt(v: &[f64]) -> R<f64> {
    let n = v.len() as f64;
    if v.len() < 4 {
        return Err(CellError::Div0);
    }
    let m = mean(v)?;
    let s = var_s(v)?.sqrt();
    if s == 0.0 {
        return Err(CellError::Div0);
    }
    let sum4: f64 = v.iter().map(|x| ((x - m) / s).powi(4)).sum();
    Ok(n * (n + 1.0) / ((n - 1.0) * (n - 2.0) * (n - 3.0)) * sum4 - 3.0 * (n - 1.0).powi(2) / ((n - 2.0) * (n - 3.0)))
}

pub(super) fn skew(v: &[f64]) -> R<f64> {
    let n = v.len() as f64;
    if v.len() < 3 {
        return Err(CellError::Div0);
    }
    let m = mean(v)?;
    let s = var_s(v)?.sqrt();
    if s == 0.0 {
        return Err(CellError::Div0);
    }
    let sum3: f64 = v.iter().map(|x| ((x - m) / s).powi(3)).sum();
    Ok(n / ((n - 1.0) * (n - 2.0)) * sum3)
}

pub(super) fn skew_p(v: &[f64]) -> R<f64> {
    let m = mean(v)?;
    let s = var_p(v)?.sqrt();
    if s == 0.0 {
        return Err(CellError::Div0);
    }
    Ok(v.iter().map(|x| ((x - m) / s).powi(3)).sum::<f64>() / v.len() as f64)
}

pub(super) fn geomean(v: &[f64]) -> R<f64> {
    if v.is_empty() || v.iter().any(|&x| x <= 0.0) {
        return Err(CellError::Num);
    }
    Ok((v.iter().map(|x| x.ln()).sum::<f64>() / v.len() as f64).exp())
}

pub(super) fn harmean(v: &[f64]) -> R<f64> {
    if v.is_empty() {
        return Err(CellError::NA);
    }
    if v.iter().any(|&x| x <= 0.0) {
        return Err(CellError::Num);
    }
    Ok(v.len() as f64 / v.iter().map(|x| 1.0 / x).sum::<f64>())
}

pub(super) fn avedev(v: &[f64]) -> R<f64> {
    if v.is_empty() {
        return Err(CellError::Num);
    }
    let m = mean(v)?;
    Ok(v.iter().map(|x| (x - m).abs()).sum::<f64>() / v.len() as f64)
}

pub(super) fn frequency(a: &[Arg], _: C) -> R<Value> {
    let data = arg_nums(a, 0)?;
    let bins = arg_nums(a, 1)?;
    if bins.is_empty() {
        return col_value(vec![data.len() as f64]);
    }
    // a NaN bin equals no value (and would break the ascending order searched below)
    let mut order: Vec<usize> = (0..bins.len()).filter(|&i| bins.get(i).is_some_and(|b| !b.is_nan())).collect();
    order.sort_by(|&i, &j| bins.get(i).copied().unwrap_or(0.0).total_cmp(&bins.get(j).copied().unwrap_or(0.0)));
    let mut counts = vec![0.0; bins.len() + 1];
    for x in data {
        // `order` sorts the bins ascending, so the bins `x` exceeds come first: binary search for
        // the first bin at or above `x` (the first of equal bins, as a linear scan would find).
        let pos = if x.is_nan() { order.len() } else { order.partition_point(|&i| bins.get(i).copied().unwrap_or(f64::INFINITY) < x) };
        let slot = order.get(pos).copied().unwrap_or(bins.len());
        if let Some(c) = counts.get_mut(slot) {
            *c += 1.0;
        }
    }
    col_value(counts)
}

pub(super) fn prob(a: &[Arg], _: C) -> R<Value> {
    let (xs, ps) = arg_pairs(a, 0, 1)?;
    let lo = num(a, 2)?;
    let hi = if has(a, 3) { num(a, 3)? } else { lo };
    if ps.iter().any(|&p| p <= 0.0 || p > 1.0) {
        return Err(CellError::Num);
    }
    let total: f64 = ps.iter().sum();
    if (total - 1.0).abs() > 1e-7 {
        return Err(CellError::Num);
    }
    num_val(xs.iter().zip(&ps).filter(|(x, _)| **x >= lo && **x <= hi).map(|(_, p)| p).sum())
}

pub(super) fn standardize(a: &[Arg], _: C) -> R<Value> {
    let (x, m, sd) = (num(a, 0)?, num(a, 1)?, num(a, 2)?);
    if sd <= 0.0 {
        return Err(CellError::Num);
    }
    num_val((x - m) / sd)
}

// ---------------------------------------------------------------------------------------------
// SUBTOTAL / AGGREGATE support
// ---------------------------------------------------------------------------------------------

/// Evaluates SUBTOTAL/AGGREGATE function `function_num` over already-filtered cell values.
///
/// Codes 1–11 (and 101–111) are AVERAGE, COUNT, COUNTA, MAX, MIN, PRODUCT, STDEV.S, STDEV.P,
/// SUM, VAR.S, VAR.P; 12 = MEDIAN, 13 = MODE.SNGL. Codes 14–19 need `k`: use
/// [`aggregate_values_k`] (they give `#VALUE!` here). Values that are not numbers are ignored
/// (COUNTA counts non-empty values); the first error value present is returned.
pub fn aggregate_values(function_num: u32, values: &[Value]) -> Value {
    crate::util::finish(aggregate_impl(function_num, values, None))
}

/// AGGREGATE codes 14 = LARGE, 15 = SMALL, 16 = PERCENTILE.INC, 17 = QUARTILE.INC,
/// 18 = PERCENTILE.EXC, 19 = QUARTILE.EXC with their `k` argument; codes 1–13 delegate to
/// [`aggregate_values`].
pub fn aggregate_values_k(function_num: u32, values: &[Value], k: &Value) -> Value {
    crate::util::finish(aggregate_impl(function_num, values, Some(k)))
}

fn aggregate_impl(function_num: u32, values: &[Value], k: Option<&Value>) -> R<Value> {
    let code = if (101..=111).contains(&function_num) { function_num - 100 } else { function_num };
    let mut xs = Vec::new();
    let mut nonempty = 0usize;
    let mut push = |v: &Value| -> R<()> {
        match v {
            Value::Number(n) => xs.push(*n),
            Value::Error(e) => return Err(*e),
            _ => {}
        }
        if !v.is_empty() {
            nonempty += 1;
        }
        Ok(())
    };
    for v in values {
        match v {
            Value::Array(arr) => arr.iter().try_for_each(&mut push)?,
            other => push(other)?,
        }
    }
    let k = match (code, k) {
        (14..=19, Some(k)) => k.scalar().to_number()?,
        (14..=19, None) => return Err(CellError::Value),
        _ => 0.0,
    };
    let r = match code {
        1 => mean(&xs)?,
        2 => xs.len() as f64,
        3 => nonempty as f64,
        4 => max_of(&xs),
        5 => min_of(&xs),
        6 => product(&xs),
        7 => var_s(&xs)?.sqrt(),
        8 => var_p(&xs)?.sqrt(),
        9 => xs.iter().sum(),
        10 => var_s(&xs)?,
        11 => var_p(&xs)?,
        12 => median(xs)?,
        13 => mode_sngl(&xs)?,
        14 => kth(xs, k, true)?,
        15 => kth(xs, k, false)?,
        16 => percentile_inc(xs, k)?,
        17 => quartile_inc(xs, k)?,
        18 => percentile_exc(xs, k)?,
        19 => quartile_exc(xs, k)?,
        _ => return Err(CellError::Value),
    };
    num_val(r)
}
