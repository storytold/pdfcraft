//! Shared argument coercion and collection helpers.

use std::borrow::Cow;
use std::sync::Arc;

use gridcraft_core::{Array, CellError, Value};

use crate::Arg;

pub(crate) type R<T> = Result<T, CellError>;

/// Largest array a function may build.
pub(crate) const MAX_CELLS: usize = 10_000_000;
/// Longest text a cell can hold.
pub(crate) const MAX_TEXT: usize = 32_767;

/// Scalar-position lifting tables.
pub(crate) const S: &[bool] = &[true];
/// No lifting (all positions take arrays / ranges).
pub(crate) const A: &[bool] = &[false];

/// Turns an implementation result into a cell value: errors become error values, non-finite
/// numbers become `#NUM!`, text longer than 32,767 characters becomes `#VALUE!`.
pub(crate) fn finish(r: R<Value>) -> Value {
    match r {
        Ok(Value::Number(n)) => Value::number(n),
        Ok(Value::Text(t)) if t.len() > MAX_TEXT && t.chars().count() > MAX_TEXT => Value::Error(CellError::Value),
        Ok(v) => v,
        Err(e) => Value::Error(e),
    }
}

/// A finite number or `#NUM!`.
pub(crate) fn num_val(n: f64) -> R<Value> {
    if n.is_finite() { Ok(Value::Number(if n == 0.0 { 0.0 } else { n })) } else { Err(CellError::Num) }
}

pub(crate) fn text_val(s: impl Into<Arc<str>>) -> R<Value> {
    Ok(Value::Text(s.into()))
}

/// Scalar value of an argument (top-left element for arrays).
pub(crate) fn scalar(a: &Arg) -> Value {
    a.value.scalar()
}

/// Argument `i`, or `#VALUE!` when it is missing.
pub(crate) fn arg(args: &[Arg], i: usize) -> R<&Arg> {
    args.get(i).ok_or(CellError::Value)
}

/// Whether argument `i` was supplied (present and not an empty literal like `F(1,)`).
pub(crate) fn has(args: &[Arg], i: usize) -> bool {
    matches!(args.get(i), Some(a) if !matches!(a.value, Value::Empty) || a.from_ref || a.blank_value)
}

/// Number coercion for a scalar parameter: empty = 0, TRUE = 1, numeric text parses.
pub(crate) fn num(args: &[Arg], i: usize) -> R<f64> {
    let a = arg(args, i)?;
    scalar(a).to_number()
}

/// Optional number: `default` when missing, otherwise coerced (an explicit empty is 0).
pub(crate) fn opt_num(args: &[Arg], i: usize, default: f64) -> R<f64> {
    match args.get(i) {
        None => Ok(default),
        Some(a) => scalar(a).to_number(),
    }
}

/// Integer coercion (truncates toward zero). Values beyond ±2^53 give `#NUM!`.
pub(crate) fn int(args: &[Arg], i: usize) -> R<i64> {
    to_int(num(args, i)?)
}

pub(crate) fn opt_int(args: &[Arg], i: usize, default: i64) -> R<i64> {
    match args.get(i) {
        None => Ok(default),
        Some(_) => int(args, i),
    }
}

pub(crate) fn to_int(n: f64) -> R<i64> {
    if !n.is_finite() || n.abs() > 9.007_199_254_740_992e15 {
        return Err(CellError::Num);
    }
    Ok(n.trunc() as i64)
}

/// Text coercion for a scalar parameter.
pub(crate) fn text(args: &[Arg], i: usize) -> R<String> {
    let a = arg(args, i)?;
    scalar(a).to_text()
}

/// Boolean coercion for a scalar parameter.
pub(crate) fn boolean(args: &[Arg], i: usize) -> R<bool> {
    let a = arg(args, i)?;
    scalar(a).to_bool()
}

pub(crate) fn opt_bool(args: &[Arg], i: usize, default: bool) -> R<bool> {
    match args.get(i) {
        None => Ok(default),
        Some(a) => scalar(a).to_bool(),
    }
}

/// The value viewed as an array (scalars become 1×1).
pub(crate) fn as_array(v: &Value) -> Cow<'_, Array> {
    match v {
        Value::Array(a) => Cow::Borrowed(a.as_ref()),
        other => Cow::Owned(Array::scalar(other.clone())),
    }
}

/// Wraps an array result. Rejects empty and oversized arrays.
pub(crate) fn array_val(rows: usize, cols: usize, data: Vec<Value>) -> R<Value> {
    if rows.checked_mul(cols).is_none_or(|n| n > MAX_CELLS) {
        return Err(CellError::Num);
    }
    Array::new(rows, cols, data).map(Value::from).ok_or(CellError::Calc)
}

/// Checks that a requested result size is acceptable.
pub(crate) fn check_size(rows: f64, cols: f64) -> R<(usize, usize)> {
    if rows.is_nan() || cols.is_nan() || rows < 1.0 || cols < 1.0 || !rows.is_finite() || !cols.is_finite() {
        return Err(CellError::Value);
    }
    let (r, c) = (rows as usize, cols as usize);
    if r > 1_048_576 || c > 16_384 || r.saturating_mul(c) > MAX_CELLS {
        return Err(CellError::Num);
    }
    Ok((r, c))
}

/// Every value of every argument, flattened row-major (scalars as themselves).
pub(crate) fn flatten(args: &[Arg]) -> Vec<Value> {
    let mut out = Vec::new();
    for a in args {
        match &a.value {
            Value::Array(arr) => out.extend(arr.data.iter().cloned()),
            v => out.push(v.clone()),
        }
    }
    out
}

/// Numbers for aggregate functions (SUM, AVERAGE, MIN, STDEV…): inside references and arrays
/// only numbers count (text, booleans and blanks are skipped); direct scalar arguments are
/// coerced ("3" → 3, TRUE → 1, other text → `#VALUE!`). Errors propagate.
pub(crate) fn numbers(args: &[Arg]) -> R<Vec<f64>> {
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
            Value::Error(e) => return Err(*e),
            Value::Number(n) => out.push(*n),
            _ if a.from_ref => {}
            Value::Empty => {}
            v => out.push(v.to_number()?),
        }
    }
    Ok(out)
}

/// Numbers for the "A" aggregates (AVERAGEA, MAXA, STDEVA…): inside references and arrays text
/// counts as 0 and booleans as 1/0, blanks are skipped; direct arguments coerce as in
/// [`numbers`].
pub(crate) fn numbers_a(args: &[Arg]) -> R<Vec<f64>> {
    let mut out = Vec::new();
    for a in args {
        match &a.value {
            Value::Array(arr) => {
                for v in arr.iter() {
                    match v {
                        Value::Number(n) => out.push(*n),
                        Value::Bool(b) => out.push(if *b { 1.0 } else { 0.0 }),
                        Value::Text(_) => out.push(0.0),
                        Value::Error(e) => return Err(*e),
                        _ => {}
                    }
                }
            }
            Value::Error(e) => return Err(*e),
            Value::Empty => {}
            Value::Text(_) if a.from_ref => out.push(0.0),
            v => out.push(v.to_number()?),
        }
    }
    Ok(out)
}

/// Numbers of a single array-like argument where only numbers count (text, booleans, blanks
/// skipped), errors propagate. Used for data arrays of statistical functions.
pub(crate) fn array_numbers(v: &Value) -> R<Vec<f64>> {
    let mut out = Vec::new();
    match v {
        Value::Array(arr) => {
            for x in arr.iter() {
                match x {
                    Value::Number(n) => out.push(*n),
                    Value::Error(e) => return Err(*e),
                    _ => {}
                }
            }
        }
        Value::Number(n) => out.push(*n),
        Value::Error(e) => return Err(*e),
        Value::Empty => {}
        other => out.push(other.to_number()?),
    }
    Ok(out)
}

/// Rounds half away from zero at `digits` decimals using the 15-significant-digit decimal
/// representation of `x`, so ROUND(2.675, 2) = 2.68 like Excel.
pub(crate) fn round_half_away(x: f64, digits: i32) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let digits = digits.clamp(-400, 400) as i64;
    let s = format!("{:.14e}", x.abs());
    let Some((mant, exp)) = s.split_once('e') else { return x };
    let Ok(e) = exp.parse::<i64>() else { return x };
    let mut d: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    // value = 0.d1d2…d15 × 10^(e+1); keep the first `keep` digits.
    let keep = e + 1 + digits;
    if keep >= d.len() as i64 {
        // Nothing to round off, but Excel still keeps only 15 significant digits, which drops binary noise
        // such as 0.1 + 0.2 = 0.30000000000000004.
        return to_sig_digits(x, d.len());
    }
    if keep < 0 {
        return 0.0;
    }
    let keep = keep as usize;
    let round_up = d.get(keep).is_some_and(|&v| v >= 5);
    d.truncate(keep);
    let mut exp10 = e + 1;
    if round_up {
        let mut i = keep;
        loop {
            if i == 0 {
                d.insert(0, 1);
                exp10 += 1;
                break;
            }
            i -= 1;
            if let Some(v) = d.get_mut(i) {
                if *v == 9 {
                    *v = 0;
                } else {
                    *v += 1;
                    break;
                }
            }
        }
    }
    if d.is_empty() {
        return 0.0;
    }
    let digits_str: String = d.iter().map(|v| char::from(b'0' + v)).collect();
    let text = format!("0.{digits_str}e{exp10}");
    let r: f64 = text.parse().unwrap_or(x);
    if x < 0.0 { -r } else { r }
}

/// `x` rounded to `sig` significant decimal digits (via decimal formatting).
pub(crate) fn to_sig_digits(x: f64, sig: usize) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let s = format!("{:.*e}", sig.saturating_sub(1).min(30), x);
    s.parse().unwrap_or(x)
}

#[cfg(test)]
pub(crate) mod testutil {
    use gridcraft_core::{Array, CellError, DateSystem, Value};

    use crate::{Arg, Ctx};

    /// Fixed clock (2026-10-07 12:00) and a deterministic random generator.
    pub struct TestCtx {
        pub state: u64,
        pub sys: DateSystem,
    }
    impl Default for TestCtx {
        fn default() -> Self {
            TestCtx { state: 0x2545_F491_4F6C_DD1D, sys: DateSystem::D1900 }
        }
    }
    impl Ctx for TestCtx {
        fn date_system(&self) -> DateSystem {
            self.sys
        }
        fn now_serial(&self) -> f64 {
            46302.5
        }
        fn random(&mut self) -> f64 {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            (self.state >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Calls a registered function by name with literal arguments.
    pub fn ev(name: &str, args: Vec<Arg>) -> Value {
        let spec = crate::lookup(name).unwrap_or_else(|| panic!("function {name} not registered"));
        crate::call(spec, &args, &mut TestCtx::default())
    }
    /// Literal number argument.
    pub fn n(x: f64) -> Arg {
        Arg::val(x)
    }
    /// Literal text argument.
    pub fn t(s: &str) -> Arg {
        Arg::val(s)
    }
    /// Literal boolean argument.
    pub fn b(x: bool) -> Arg {
        Arg::val(x)
    }
    /// Literal error argument.
    pub fn e(x: CellError) -> Arg {
        Arg::val(x)
    }
    /// Empty literal argument (as in `F(1,)`).
    pub fn empty() -> Arg {
        Arg::val(Value::Empty)
    }
    /// Array value from rows of values.
    pub fn arr(rows: Vec<Vec<Value>>) -> Value {
        let r = rows.len();
        let c = rows.first().map_or(0, |x| x.len());
        Value::from(Array::new(r, c, rows.into_iter().flatten().collect()).expect("rectangular"))
    }
    /// Column of numbers as an array value.
    pub fn col(xs: &[f64]) -> Value {
        arr(xs.iter().map(|x| vec![Value::Number(*x)]).collect())
    }
    /// Row of numbers as an array value.
    pub fn row(xs: &[f64]) -> Value {
        arr(vec![xs.iter().map(|x| Value::Number(*x)).collect()])
    }
    /// Range argument (from a reference).
    pub fn rf(v: Value) -> Arg {
        Arg::reference(v)
    }
    /// Array-constant argument.
    pub fn av(v: Value) -> Arg {
        Arg::val(v)
    }
    pub fn tv(s: &str) -> Value {
        Value::from(s)
    }
    pub fn nv(x: f64) -> Value {
        Value::Number(x)
    }
    /// Asserts a numeric result within 1e-9 relative (or absolute near zero) tolerance.
    #[track_caller]
    pub fn close(v: Value, expected: f64) {
        match v {
            Value::Number(x) => {
                let tol = 1e-9 * expected.abs().max(1.0);
                assert!((x - expected).abs() <= tol, "got {x}, expected {expected}");
            }
            other => panic!("expected number {expected}, got {other:?}"),
        }
    }
    /// Like [`close`] with a custom relative tolerance.
    #[track_caller]
    pub fn close_tol(v: Value, expected: f64, tol: f64) {
        match v {
            Value::Number(x) => {
                let t = tol * expected.abs().max(1.0);
                assert!((x - expected).abs() <= t, "got {x}, expected {expected}");
            }
            other => panic!("expected number {expected}, got {other:?}"),
        }
    }
    #[track_caller]
    pub fn is_err(v: Value, err: CellError) {
        assert_eq!(v, Value::Error(err));
    }
    #[track_caller]
    pub fn is_text(v: Value, s: &str) {
        assert_eq!(v, Value::from(s));
    }
    /// Array result as rows of values.
    #[track_caller]
    pub fn rows_of(v: &Value) -> Vec<Vec<Value>> {
        match v {
            Value::Array(a) => (0..a.rows).map(|r| (0..a.cols).map(|c| a.get(r, c).cloned().unwrap_or(Value::Empty)).collect()).collect(),
            other => vec![vec![other.clone()]],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding() {
        assert_eq!(round_half_away(2.675, 2), 2.68);
        assert_eq!(round_half_away(-2.5, 0), -3.0);
        assert_eq!(round_half_away(1234.5678, -2), 1200.0);
        assert_eq!(round_half_away(0.285, 2), 0.29);
        assert_eq!(round_half_away(1.005, 2), 1.01);
        assert_eq!(round_half_away(0.1 + 0.2, 15), 0.3);
        assert_eq!(round_half_away(0.1 + 0.2, 20), 0.3);
    }

    #[test]
    fn aggregate_numbers() {
        let arr = Value::from(Array::new(1, 4, vec![Value::Number(1.0), Value::from("2"), Value::Bool(true), Value::Empty]).unwrap());
        assert_eq!(numbers(&[Arg::reference(arr.clone())]).unwrap(), vec![1.0]);
        assert_eq!(numbers_a(&[Arg::reference(arr)]).unwrap(), vec![1.0, 0.0, 1.0]);
        assert_eq!(numbers(&[Arg::val("3"), Arg::val(true)]).unwrap(), vec![3.0, 1.0]);
        assert_eq!(numbers(&[Arg::val("x")]), Err(CellError::Value));
        assert_eq!(numbers(&[Arg::reference("x")]).unwrap(), Vec::<f64>::new());
    }
}
