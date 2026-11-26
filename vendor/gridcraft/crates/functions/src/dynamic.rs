//! Dynamic-array functions: FILTER, SORT, SORTBY, UNIQUE, SEQUENCE, RANDARRAY, VSTACK, HSTACK,
//! TOROW, TOCOL, WRAPROWS, WRAPCOLS, TAKE, DROP, CHOOSEROWS, CHOOSECOLS, EXPAND, GROUPBY,
//! PIVOTBY.
//!
//! GROUPBY / PIVOTBY take their aggregation as a function name ("SUM", "AVERAGE"…) or a
//! SUBTOTAL-style code (1–11): the host resolves eta-reduced lambdas such as `SUM` to the name
//! text before calling. Custom LAMBDA aggregations are not supported here.

use std::cmp::Ordering;
use std::collections::HashMap;

use gridcraft_core::{Array, CellError, Value, sort_compare};

use crate::util::{A, MAX_CELLS, R, arg, array_val, as_array, has, num, opt_bool, scalar};
use crate::{Arg, Ctx, FnSpec, VAR};

// ---------------------------------------------------------------------------------------------
// Helpers

/// Argument `i` as an owned array; error values propagate.
fn arr_arg(args: &[Arg], i: usize) -> R<Array> {
    let v = &arg(args, i)?.value;
    if let Value::Error(e) = v {
        return Err(*e);
    }
    Ok(as_array(v).into_owned())
}

fn rows_of(a: &Array) -> Vec<Vec<Value>> {
    (0..a.rows).map(|r| (0..a.cols).map(|c| a.get(r, c).cloned().unwrap_or(Value::Empty)).collect()).collect()
}

/// `#NUM!` when an `h` x `w` output would exceed the function cell budget.
fn check_output_size(h: usize, w: usize) -> R<()> {
    if h.saturating_mul(w) > MAX_CELLS { Err(CellError::Num) } else { Ok(()) }
}

/// Builds an array from rows (all rows padded to the widest with `pad`). Empty → `#CALC!`.
fn from_rows(rows: Vec<Vec<Value>>, pad: &Value) -> R<Value> {
    let h = rows.len();
    let w = rows.iter().map(Vec::len).max().unwrap_or(0);
    if h == 0 || w == 0 {
        return Err(CellError::Calc);
    }
    check_output_size(h, w)?;
    let mut data = Vec::with_capacity(h * w);
    for mut r in rows {
        r.resize(w, pad.clone());
        data.extend(r);
    }
    array_val(h, w, data)
}

/// An optional integer argument where a missing or blank argument means `None`.
fn opt_count(args: &[Arg], i: usize) -> R<Option<i64>> {
    if !has(args, i) {
        return Ok(None);
    }
    let n = num(args, i)?;
    if !n.is_finite() || n.abs() > 1e15 {
        return Err(CellError::Num);
    }
    Ok(Some(n.trunc() as i64))
}

/// Pad value for WRAPROWS/EXPAND…: `#N/A` when omitted.
fn pad_arg(args: &[Arg], i: usize) -> Value {
    match args.get(i) {
        Some(a) if has(args, i) => scalar(a),
        _ => Value::Error(CellError::NA),
    }
}

/// Hashable key for case-insensitive equality of values.
fn key_of(v: &Value) -> String {
    match v {
        Value::Empty => "e".into(),
        Value::Number(n) => format!("n{}", if *n == 0.0 { 0u64 } else { n.to_bits() }),
        Value::Text(t) => format!("t{}", t.to_lowercase()),
        Value::Bool(b) => format!("b{b}"),
        Value::Error(e) => format!("x{}", e.code()),
        Value::Array(a) => key_of(a.data.first().unwrap_or(&Value::Empty)),
    }
}

fn row_key(r: &[Value]) -> String {
    let mut s = String::new();
    for v in r {
        let k = key_of(v);
        // Length prefix keeps the encoding unambiguous even when a text value contains the separator.
        s.push_str(&k.len().to_string());
        s.push(':');
        s.push_str(&k);
    }
    s
}

fn truthy(v: &Value) -> R<bool> {
    match v {
        Value::Error(e) => Err(*e),
        Value::Empty => Ok(false),
        Value::Text(_) => Err(CellError::Value),
        other => other.to_bool(),
    }
}

// ---------------------------------------------------------------------------------------------
// FILTER / SORT / SORTBY / UNIQUE

fn filter(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let a = arr_arg(args, 0)?;
    let inc = arr_arg(args, 1)?;
    let keep_rows = inc.cols == 1 && inc.rows == a.rows;
    let keep_cols = !keep_rows && inc.rows == 1 && inc.cols == a.cols;
    if !keep_rows && !keep_cols {
        return Err(CellError::Value);
    }
    let flags: Vec<bool> = inc.data.iter().map(truthy).collect::<R<_>>()?;
    let grid = rows_of(&a);
    let out: Vec<Vec<Value>> = if keep_rows {
        grid.into_iter().zip(flags.iter()).filter(|(_, f)| **f).map(|(r, _)| r).collect()
    } else {
        grid.into_iter().map(|r| r.into_iter().zip(flags.iter()).filter(|(_, f)| **f).map(|(v, _)| v).collect::<Vec<_>>()).collect()
    };
    if out.is_empty() || out.iter().all(Vec::is_empty) {
        if has(args, 2) {
            return Ok(arg(args, 2)?.value.clone());
        }
        return Err(CellError::Calc);
    }
    from_rows(out, &Value::Empty)
}

/// Compares two key vectors with per-key orders (1 ascending, -1 descending).
fn cmp_keys(a: &[Value], b: &[Value], orders: &[i64]) -> Ordering {
    for (i, ord) in orders.iter().enumerate() {
        let (Some(x), Some(y)) = (a.get(i), b.get(i)) else { continue };
        let o = sort_compare(x, y);
        let o = if *ord < 0 {
            // Blanks stay last even when descending.
            match (x, y) {
                (Value::Empty, Value::Empty) => Ordering::Equal,
                (Value::Empty, _) => Ordering::Greater,
                (_, Value::Empty) => Ordering::Less,
                _ => o.reverse(),
            }
        } else {
            o
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

fn int_list(v: &Value) -> R<Vec<i64>> {
    let a = as_array(v);
    a.data
        .iter()
        .map(|x| {
            let n = x.to_number()?;
            if !n.is_finite() || n.abs() > 1e15 { Err(CellError::Value) } else { Ok(n.trunc() as i64) }
        })
        .collect()
}

fn sort(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let a = arr_arg(args, 0)?;
    let by_col = opt_bool(args, 3, false)?;
    let a = if by_col { a.transpose() } else { a };
    let idx = if has(args, 1) { int_list(&arg(args, 1)?.value)? } else { vec![1] };
    let ord = if has(args, 2) { int_list(&arg(args, 2)?.value)? } else { vec![1] };
    if idx.is_empty() || idx.iter().any(|&i| i < 1 || i > a.cols as i64) || ord.iter().any(|&o| o != 1 && o != -1) {
        return Err(CellError::Value);
    }
    if ord.len() > 1 && ord.len() != idx.len() {
        return Err(CellError::Value);
    }
    let orders: Vec<i64> = (0..idx.len()).map(|i| *ord.get(i).or(ord.first()).unwrap_or(&1)).collect();
    // A column repeated in sort_index can't break a tie its first use left, so only first uses
    // stay: the keys copied per row are then at most the row itself, however long sort_index is.
    let mut seen = std::collections::HashSet::new();
    let (idx, orders): (Vec<i64>, Vec<i64>) = idx.into_iter().zip(orders).filter(|(i, _)| seen.insert(*i)).unzip();
    let mut lines: Vec<(Vec<Value>, Vec<Value>)> =
        rows_of(&a).into_iter().map(|r| (idx.iter().map(|&i| r.get(i as usize - 1).cloned().unwrap_or(Value::Empty)).collect(), r)).collect();
    lines.sort_by(|x, y| cmp_keys(&x.0, &y.0, &orders));
    let out = Array::new(a.rows, a.cols, lines.into_iter().flat_map(|(_, r)| r).collect()).ok_or(CellError::Calc)?;
    Ok(Value::from(if by_col { out.transpose() } else { out }))
}

fn sortby(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let a = arr_arg(args, 0)?;
    let mut keys: Vec<Array> = Vec::new();
    let mut orders: Vec<i64> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        keys.push(arr_arg(args, i)?);
        let o = if has(args, i + 1) { num(args, i + 1)?.trunc() as i64 } else { 1 };
        if o != 1 && o != -1 {
            return Err(CellError::Value);
        }
        orders.push(o);
        i += 2;
    }
    let first = keys.first().ok_or(CellError::Value)?;
    let by_rows = first.cols == 1 && first.rows == a.rows;
    let by_cols = !by_rows && first.rows == 1 && first.cols == a.cols;
    if !by_rows && !by_cols {
        return Err(CellError::Value);
    }
    for k in &keys {
        let ok = if by_rows { k.cols == 1 && k.rows == a.rows } else { k.rows == 1 && k.cols == a.cols };
        if !ok {
            return Err(CellError::Value);
        }
    }
    let base = if by_rows { a } else { a.transpose() };
    let mut lines: Vec<(Vec<Value>, Vec<Value>)> = rows_of(&base)
        .into_iter()
        .enumerate()
        .map(|(r, row)| (keys.iter().map(|k| k.data.get(r).cloned().unwrap_or(Value::Empty)).collect(), row))
        .collect();
    lines.sort_by(|x, y| cmp_keys(&x.0, &y.0, &orders));
    let out = Array::new(base.rows, base.cols, lines.into_iter().flat_map(|(_, r)| r).collect()).ok_or(CellError::Calc)?;
    Ok(Value::from(if by_rows { out } else { out.transpose() }))
}

fn unique(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let a = arr_arg(args, 0)?;
    let by_col = opt_bool(args, 1, false)?;
    let once = opt_bool(args, 2, false)?;
    let base = if by_col { a.transpose() } else { a };
    let lines = rows_of(&base);
    let mut counts: HashMap<String, usize> = HashMap::new();
    for l in &lines {
        *counts.entry(row_key(l)).or_insert(0) += 1;
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for l in lines {
        let k = row_key(&l);
        let c = counts.get(&k).copied().unwrap_or(0);
        if once && c != 1 {
            continue;
        }
        if seen.insert(k) {
            out.push(l);
        }
    }
    if out.is_empty() {
        return Err(CellError::Calc);
    }
    let v = from_rows(out, &Value::Empty)?;
    if by_col && let Value::Array(arr) = &v {
        return Ok(Value::from(arr.transpose()));
    }
    Ok(v)
}

// ---------------------------------------------------------------------------------------------
// SEQUENCE / RANDARRAY

/// Result dimensions for SEQUENCE/RANDARRAY: blank means 1; 0 → `#CALC!`; negative → `#VALUE!`.
fn dims(args: &[Arg], ri: usize, ci: usize) -> R<(usize, usize)> {
    let r = if has(args, ri) { num(args, ri)?.trunc() } else { 1.0 };
    let c = if has(args, ci) { num(args, ci)?.trunc() } else { 1.0 };
    if r < 0.0 || c < 0.0 || !r.is_finite() || !c.is_finite() {
        return Err(CellError::Value);
    }
    if r == 0.0 || c == 0.0 {
        return Err(CellError::Calc);
    }
    if r > 1_048_576.0 || c > 16_384.0 || r * c > MAX_CELLS as f64 {
        return Err(CellError::Num);
    }
    Ok((r as usize, c as usize))
}

fn sequence(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (r, c) = dims(args, 0, 1)?;
    let start = if has(args, 2) { num(args, 2)? } else { 1.0 };
    let step = if has(args, 3) { num(args, 3)? } else { 1.0 };
    let data = (0..r * c).map(|i| Value::number(start + step * i as f64)).collect();
    array_val(r, c, data)
}

fn randarray(args: &[Arg], ctx: &mut dyn Ctx) -> R<Value> {
    let (r, c) = dims(args, 0, 1)?;
    let lo = if has(args, 2) { num(args, 2)? } else { 0.0 };
    let hi = if has(args, 3) { num(args, 3)? } else { 1.0 };
    let whole = if has(args, 4) { opt_bool(args, 4, false)? } else { false };
    if lo > hi {
        return Err(CellError::Value);
    }
    let mut data = Vec::with_capacity(r * c);
    if whole {
        let (lo, hi) = (lo.ceil(), hi.floor());
        if lo > hi {
            return Err(CellError::Value);
        }
        for _ in 0..r * c {
            let x = (lo + (ctx.random() * (hi - lo + 1.0)).floor()).min(hi);
            data.push(Value::number(x));
        }
    } else {
        for _ in 0..r * c {
            data.push(Value::number(lo + ctx.random() * (hi - lo)));
        }
    }
    array_val(r, c, data)
}

// ---------------------------------------------------------------------------------------------
// VSTACK / HSTACK / TOROW / TOCOL / WRAPROWS / WRAPCOLS

fn vstack(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let mut rows = Vec::new();
    let mut total = 0usize;
    for i in 0..args.len() {
        let a = arr_arg(args, i)?;
        total = total.saturating_add(a.data.len());
        if total > MAX_CELLS {
            return Err(CellError::Num);
        }
        rows.extend(rows_of(&a));
    }
    from_rows(rows, &Value::Error(CellError::NA))
}

fn hstack(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let mut arrays = Vec::new();
    let mut h = 0usize;
    let mut w = 0usize;
    let mut total = 0usize;
    for i in 0..args.len() {
        let a = arr_arg(args, i)?;
        h = h.max(a.rows);
        w = w.saturating_add(a.cols);
        total = total.saturating_add(a.data.len());
        if total > MAX_CELLS {
            return Err(CellError::Num);
        }
        arrays.push(a);
    }
    // The padded rectangle can be far larger than the inputs; refuse it before building any rows.
    check_output_size(h, w)?;
    let mut rows = vec![Vec::new(); h];
    for a in &arrays {
        for (r, row) in rows.iter_mut().enumerate() {
            for c in 0..a.cols {
                row.push(a.get(r, c).cloned().unwrap_or(Value::Error(CellError::NA)));
            }
        }
    }
    from_rows(rows, &Value::Error(CellError::NA))
}

/// Flattened values for TOROW/TOCOL with the ignore filter applied.
fn flat_filtered(args: &[Arg]) -> R<Vec<Value>> {
    let a = arr_arg(args, 0)?;
    let ignore = if has(args, 1) { num(args, 1)?.trunc() as i64 } else { 0 };
    if !(0..=3).contains(&ignore) {
        return Err(CellError::Value);
    }
    let by_col = opt_bool(args, 2, false)?;
    let a = if by_col { a.transpose() } else { a };
    Ok(a.data
        .into_iter()
        .filter(|v| match v {
            Value::Empty => ignore & 1 == 0,
            Value::Error(_) => ignore & 2 == 0,
            _ => true,
        })
        .collect())
}

fn torow(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = flat_filtered(args)?;
    if v.is_empty() {
        return Err(CellError::Calc);
    }
    array_val(1, v.len(), v)
}

fn tocol(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = flat_filtered(args)?;
    if v.is_empty() {
        return Err(CellError::Calc);
    }
    array_val(v.len(), 1, v)
}

fn wrap(args: &[Arg], rows: bool) -> R<Value> {
    let a = arr_arg(args, 0)?;
    if a.rows != 1 && a.cols != 1 {
        return Err(CellError::Value);
    }
    let n = num(args, 1)?.trunc();
    if n < 1.0 || !n.is_finite() {
        return Err(CellError::Num);
    }
    let pad = pad_arg(args, 2);
    let len = a.data.len();
    let n = (n as usize).min(len.max(1));
    let lines = len.div_ceil(n);
    let mut out = Vec::with_capacity(lines);
    for l in 0..lines {
        let mut line: Vec<Value> = a.data.iter().skip(l * n).take(n).cloned().collect();
        line.resize(n, pad.clone());
        out.push(line);
    }
    let v = from_rows(out, &pad)?;
    if !rows && let Value::Array(arr) = &v {
        return Ok(Value::from(arr.transpose()));
    }
    Ok(v)
}

fn wraprows(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    wrap(args, true)
}

fn wrapcols(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    wrap(args, false)
}

// ---------------------------------------------------------------------------------------------
// TAKE / DROP / CHOOSEROWS / CHOOSECOLS / EXPAND

/// Range of kept indices for TAKE (`take = true`) or DROP along an axis of length `len`.
fn span(len: usize, n: Option<i64>, take: bool) -> R<(usize, usize)> {
    let Some(n) = n else { return Ok((0, len)) };
    let k = (n.unsigned_abs() as usize).min(len);
    if take {
        if n == 0 {
            return Err(CellError::Calc);
        }
        Ok(if n > 0 { (0, k) } else { (len - k, len) })
    } else {
        let r = if n >= 0 { (k, len) } else { (0, len - k) };
        if r.0 >= r.1 {
            return Err(CellError::Calc);
        }
        Ok(r)
    }
}

fn take_drop(args: &[Arg], take: bool) -> R<Value> {
    let a = arr_arg(args, 0)?;
    let (r0, r1) = span(a.rows, opt_count(args, 1)?, take)?;
    let (c0, c1) = span(a.cols, opt_count(args, 2)?, take)?;
    let rows: Vec<Vec<Value>> = (r0..r1).map(|r| (c0..c1).map(|c| a.get(r, c).cloned().unwrap_or(Value::Empty)).collect()).collect();
    from_rows(rows, &Value::Empty)
}

fn take_fn(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    take_drop(args, true)
}

fn drop_fn(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    take_drop(args, false)
}

fn choose_lines(args: &[Arg], rows: bool) -> R<Value> {
    let a = arr_arg(args, 0)?;
    let base = if rows { a } else { a.transpose() };
    let grid = rows_of(&base);
    let mut out = Vec::new();
    for i in 1..args.len() {
        for k in int_list(&arg(args, i)?.value)? {
            let len = grid.len() as i64;
            let idx = if k > 0 && k <= len {
                k - 1
            } else if k < 0 && -k <= len {
                len + k
            } else {
                return Err(CellError::Value);
            };
            out.push(grid.get(idx as usize).cloned().ok_or(CellError::Value)?);
            if out.len().saturating_mul(base.cols) > MAX_CELLS {
                return Err(CellError::Num);
            }
        }
    }
    let v = from_rows(out, &Value::Empty)?;
    if !rows && let Value::Array(arr) = &v {
        return Ok(Value::from(arr.transpose()));
    }
    Ok(v)
}

fn chooserows(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    choose_lines(args, true)
}

fn choosecols(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    choose_lines(args, false)
}

fn expand(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let a = arr_arg(args, 0)?;
    let r = opt_count(args, 1)?.unwrap_or(a.rows as i64);
    let c = opt_count(args, 2)?.unwrap_or(a.cols as i64);
    if r < a.rows as i64 || c < a.cols as i64 {
        return Err(CellError::Value);
    }
    if r > 1_048_576 || c > 16_384 || (r as usize).saturating_mul(c as usize) > MAX_CELLS {
        return Err(CellError::Num);
    }
    let pad = pad_arg(args, 3);
    let (r, c) = (r as usize, c as usize);
    let mut data = Vec::with_capacity(r * c);
    for i in 0..r {
        for j in 0..c {
            data.push(a.get(i, j).cloned().unwrap_or_else(|| pad.clone()));
        }
    }
    array_val(r, c, data)
}

// ---------------------------------------------------------------------------------------------
// GROUPBY / PIVOTBY

#[derive(Clone, Copy, Debug, PartialEq)]
enum Agg {
    Sum,
    Average,
    Count,
    CountA,
    Max,
    Min,
    Product,
    Median,
    StdevS,
    StdevP,
    VarS,
    VarP,
}

fn parse_agg(v: &Value) -> R<Agg> {
    match v {
        Value::Error(e) => Err(*e),
        Value::Number(n) => Ok(match n.trunc() as i64 {
            1 | 101 => Agg::Average,
            2 | 102 => Agg::Count,
            3 | 103 => Agg::CountA,
            4 | 104 => Agg::Max,
            5 | 105 => Agg::Min,
            6 | 106 => Agg::Product,
            7 | 107 => Agg::StdevS,
            8 | 108 => Agg::StdevP,
            9 | 109 => Agg::Sum,
            10 | 110 => Agg::VarS,
            11 | 111 => Agg::VarP,
            12 => Agg::Median,
            _ => return Err(CellError::Value),
        }),
        Value::Text(t) => {
            let u = t.trim().to_ascii_uppercase();
            let u = u.strip_prefix("_XLFN.").unwrap_or(&u).to_string();
            Ok(match u.as_str() {
                "SUM" => Agg::Sum,
                "AVERAGE" => Agg::Average,
                "COUNT" => Agg::Count,
                "COUNTA" => Agg::CountA,
                "MAX" => Agg::Max,
                "MIN" => Agg::Min,
                "PRODUCT" => Agg::Product,
                "MEDIAN" => Agg::Median,
                "STDEV.S" | "STDEV" => Agg::StdevS,
                "STDEV.P" | "STDEVP" => Agg::StdevP,
                "VAR.S" | "VAR" => Agg::VarS,
                "VAR.P" | "VARP" => Agg::VarP,
                _ => return Err(CellError::Value),
            })
        }
        Value::Array(a) => parse_agg(a.data.first().unwrap_or(&Value::Empty)),
        _ => Err(CellError::Value),
    }
}

/// The function argument of GROUPBY/PIVOTBY: a built-in aggregation, or `None` for a LAMBDA (called
/// through the context).
fn agg_arg(args: &[Arg], i: usize, c: &dyn Ctx) -> R<Option<Agg>> {
    if c.has_lambda() { Ok(None) } else { parse_agg(&arg(args, i)?.value).map(Some) }
}

/// Aggregates one group with `f`, or with the LAMBDA (given the group's values as a column) when `None`.
fn aggregate_with(f: Option<Agg>, vals: &[&Value], c: &mut dyn Ctx) -> Value {
    match f {
        Some(f) => aggregate(f, vals),
        None => {
            let col = Value::from(Array::column(vals.iter().map(|v| (*v).clone()).collect()));
            c.call_lambda(col).unwrap_or(Value::Error(CellError::Calc))
        }
    }
}

fn aggregate(f: Agg, vals: &[&Value]) -> Value {
    if f == Agg::CountA {
        return Value::Number(vals.iter().filter(|v| !matches!(v, Value::Empty)).count() as f64);
    }
    let mut xs = Vec::new();
    for v in vals {
        match v {
            Value::Number(n) => xs.push(*n),
            Value::Error(e) if f != Agg::Count => return Value::Error(*e),
            _ => {}
        }
    }
    let n = xs.len() as f64;
    let sum: f64 = xs.iter().sum();
    let var = |ddof: f64| -> Value {
        if n - ddof <= 0.0 {
            return Value::Error(CellError::Div0);
        }
        let m = sum / n;
        Value::number(xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - ddof))
    };
    match f {
        Agg::Sum => Value::number(sum),
        Agg::Count => Value::Number(n),
        Agg::Average => {
            if xs.is_empty() {
                Value::Error(CellError::Div0)
            } else {
                Value::number(sum / n)
            }
        }
        Agg::Max => Value::number(if xs.is_empty() { 0.0 } else { xs.iter().copied().fold(f64::NEG_INFINITY, f64::max) }),
        Agg::Min => Value::number(if xs.is_empty() { 0.0 } else { xs.iter().copied().fold(f64::INFINITY, f64::min) }),
        Agg::Product => Value::number(if xs.is_empty() { 0.0 } else { xs.iter().product() }),
        Agg::Median => {
            if xs.is_empty() {
                return Value::Error(CellError::Num);
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
            let m = xs.len() / 2;
            let v = if xs.len() % 2 == 1 {
                xs.get(m).copied().unwrap_or(0.0)
            } else {
                (xs.get(m - 1).copied().unwrap_or(0.0) + xs.get(m).copied().unwrap_or(0.0)) / 2.0
            };
            Value::number(v)
        }
        Agg::VarS => var(1.0),
        Agg::VarP => var(0.0),
        Agg::StdevS => match var(1.0) {
            Value::Number(x) => Value::number(x.sqrt()),
            o => o,
        },
        Agg::StdevP => match var(0.0) {
            Value::Number(x) => Value::number(x.sqrt()),
            o => o,
        },
        Agg::CountA => Value::Number(0.0),
    }
}

/// Shared input preparation for GROUPBY/PIVOTBY.
struct Prepared {
    /// Header text for each row field (or generated) when headers are shown.
    field_headers: Vec<Value>,
    value_headers: Vec<Value>,
    show_headers: bool,
    /// Indices of data rows that pass the filter.
    rows: Vec<usize>,
}

/// field_headers: 0 none, 1 present but hidden, 2 generate, 3 present and shown; missing =
/// automatic (headers when the first value row is all text and the next is not).
fn prepare(fields: &[&Array], values: &Array, fh: Option<i64>, filter: Option<&Array>) -> R<Prepared> {
    let n = values.rows;
    for f in fields {
        if f.rows != n {
            return Err(CellError::Value);
        }
    }
    let first_text = (0..values.cols).all(|c| matches!(values.get(0, c), Some(Value::Text(_))));
    let second_not = n > 1 && (0..values.cols).any(|c| !matches!(values.get(1, c), Some(Value::Text(_))));
    let mode = match fh {
        Some(m @ 0..=3) => m,
        Some(_) => return Err(CellError::Value),
        None => {
            if first_text && second_not {
                3
            } else {
                0
            }
        }
    };
    let has_hdr = mode == 1 || mode == 3;
    let show = mode >= 2;
    let mut field_headers = Vec::new();
    let mut k = 0;
    for f in fields {
        for c in 0..f.cols {
            k += 1;
            field_headers.push(if has_hdr { f.get(0, c).cloned().unwrap_or(Value::Empty) } else { Value::from(format!("Field {k}")) });
        }
    }
    let value_headers = (0..values.cols)
        .map(|c| if has_hdr { values.get(0, c).cloned().unwrap_or(Value::Empty) } else { Value::from(format!("Value {}", c + 1)) })
        .collect();
    let start = usize::from(has_hdr);
    let mut rows = Vec::new();
    for r in start..n {
        if let Some(fa) = filter {
            let v = fa.data.get(r).cloned().unwrap_or(Value::Empty);
            if !truthy(&v)? {
                continue;
            }
        }
        rows.push(r);
    }
    Ok(Prepared { field_headers, value_headers, show_headers: show, rows })
}

fn key_row(a: &Array, r: usize) -> Vec<Value> {
    (0..a.cols).map(|c| a.get(r, c).cloned().unwrap_or(Value::Empty)).collect()
}

/// Distinct keys in first-seen order, then sorted (by `sort` = ±column index within the key; 0
/// keeps all keys ascending).
fn distinct_keys(keys: &[Vec<Value>], sort: i64) -> Vec<Vec<Value>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for k in keys {
        if seen.insert(row_key(k)) {
            out.push(k.clone());
        }
    }
    let width = out.first().map_or(0, Vec::len);
    let mut orders = vec![1i64; width];
    if sort != 0 {
        let i = sort.unsigned_abs() as usize;
        if i >= 1 && i <= width {
            if let Some(o) = orders.get_mut(i - 1) {
                *o = sort.signum();
            }
            // Sort key column first.
            out.sort_by(|a, b| {
                let ka = vec![a.get(i - 1).cloned().unwrap_or(Value::Empty)];
                let kb = vec![b.get(i - 1).cloned().unwrap_or(Value::Empty)];
                cmp_keys(&ka, &kb, &[sort.signum()]).then_with(|| cmp_keys(a, b, &vec![1; width]))
            });
            return out;
        }
    }
    out.sort_by(|a, b| cmp_keys(a, b, &orders));
    out
}

fn opt_arr(args: &[Arg], i: usize) -> R<Option<Array>> {
    if has(args, i) { Ok(Some(arr_arg(args, i)?)) } else { Ok(None) }
}

fn groupby(args: &[Arg], c: &mut dyn Ctx) -> R<Value> {
    let rf = arr_arg(args, 0)?;
    let vals = arr_arg(args, 1)?;
    let f = agg_arg(args, 2, c)?;
    let fh = opt_count(args, 3)?;
    let total = opt_count(args, 4)?.unwrap_or(1);
    let sort = opt_count(args, 5)?.unwrap_or(1);
    let filter = opt_arr(args, 6)?;
    let relationship = opt_count(args, 7)?.unwrap_or(0);
    // A total depth of N adds subtotals for the first N - 1 key columns, so it can't exceed the
    // key columns, and a flat table (field_relationship 1) has no levels to subtotal.
    let depth = total.unsigned_abs() as usize;
    if depth > rf.cols.max(1) || (depth > 1 && relationship == 1) {
        return Err(CellError::Value);
    }
    let p = prepare(&[&rf], &vals, fh, filter.as_ref())?;
    let keys: Vec<Vec<Value>> = p.rows.iter().map(|&r| key_row(&rf, r)).collect();
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (k, &r) in keys.iter().zip(p.rows.iter()) {
        groups.entry(row_key(k)).or_default().push(r);
    }
    let sort_in_keys = if sort.unsigned_abs() as usize <= rf.cols { sort } else { 0 };
    let mut ordered = distinct_keys(&keys, sort_in_keys);
    let mut agg_row = |rows: &[usize]| -> Vec<Value> {
        (0..vals.cols)
            .map(|col| {
                let vs: Vec<&Value> = rows.iter().filter_map(|&r| vals.get(r, col)).collect();
                aggregate_with(f, &vs, c)
            })
            .collect()
    };
    let mut lines: Vec<Vec<Value>> = ordered
        .drain(..)
        .map(|k| {
            let rows = groups.get(&row_key(&k)).cloned().unwrap_or_default();
            let mut line = k;
            line.extend(agg_row(&rows));
            line
        })
        .collect();
    // Sorting by a value column.
    if sort.unsigned_abs() as usize > rf.cols {
        let ci = sort.unsigned_abs() as usize - 1;
        lines
            .sort_by(|a, b| cmp_keys(&[a.get(ci).cloned().unwrap_or(Value::Empty)], &[b.get(ci).cloned().unwrap_or(Value::Empty)], &[sort.signum()]));
    }
    if depth > 1 {
        let mut rows_of = |keys: &[Value]| groups.get(&row_key(keys)).cloned().unwrap_or_default();
        // The header and total lines count too, so the whole result is checked before it is built.
        let around = usize::from(p.show_headers) + usize::from(total != 0);
        let width = rf.cols.saturating_add(vals.cols);
        let fits = |n: usize| check_output_size(n.saturating_add(around), width);
        lines = with_subtotals(lines, depth, rf.cols, total < 0, &fits, &mut rows_of, &mut agg_row)?;
    }
    let mut out = Vec::new();
    if p.show_headers {
        let mut h = p.field_headers.clone();
        h.extend(p.value_headers.iter().cloned());
        out.push(h);
    }
    // With subtotals Excel calls the last line "Grand Total".
    let mut total_line = vec![Value::from(if depth > 1 { "Grand Total" } else { "Total" })];
    total_line.resize(rf.cols, Value::Empty);
    total_line.extend(agg_row(&p.rows));
    if total < 0 {
        out.push(total_line);
        out.extend(lines);
    } else {
        out.extend(lines);
        if total > 0 {
            out.push(total_line);
        }
    }
    from_rows(out, &Value::Empty)
}

/// GROUPBY's subtotals: `lines` (`key_cols` keys, then values) with the lines of each group of a
/// key level kept together in the order the groups first appear, and for every key level above
/// `depth` a line after the group (before it when `before`): the group's keys, blanks for the deeper
/// keys, and the values aggregated over the group's source rows (`rows_of` maps a line's keys to them).
///
/// No recursion, so a deep total depth can't exhaust the stack: each line gets the id of its key
/// prefix at every level (ids count up as prefixes first appear), a stable sort by those ids brings
/// each group together, and one walk adds a subtotal where a group starts or ends. `fits` gets the
/// line count as the groups are found, so an oversized result is refused before it is built.
fn with_subtotals(
    mut lines: Vec<Vec<Value>>,
    depth: usize,
    key_cols: usize,
    before: bool,
    fits: &dyn Fn(usize) -> R<()>,
    rows_of: &mut dyn FnMut(&[Value]) -> Vec<usize>,
    agg: &mut dyn FnMut(&[usize]) -> Vec<Value>,
) -> R<Vec<Vec<Value>>> {
    let levels = depth.saturating_sub(1);
    if levels == 0 {
        return Ok(lines);
    }
    let mut ids: HashMap<(usize, String), usize> = HashMap::new();
    let mut paths: Vec<Vec<usize>> = Vec::with_capacity(lines.len());
    for line in &lines {
        let mut parent = usize::MAX;
        let mut path = Vec::with_capacity(levels);
        for level in 0..levels {
            let next = ids.len();
            parent = *ids.entry((parent, row_key(line.get(level..=level).unwrap_or_default()))).or_insert(next);
            path.push(parent);
        }
        fits(lines.len().saturating_add(ids.len()))?;
        paths.push(path);
    }
    let mut group_rows: Vec<Vec<usize>> = vec![Vec::new(); ids.len()];
    for (line, path) in lines.iter().zip(&paths) {
        let rows = rows_of(line.get(..key_cols).unwrap_or_default());
        for &id in path {
            if let Some(g) = group_rows.get_mut(id) {
                g.extend_from_slice(&rows);
            }
        }
    }
    let mut subtotal = |keys: &[Value], level: usize, id: usize| -> Vec<Value> {
        let mut line: Vec<Value> = keys.iter().take(level + 1).cloned().collect();
        line.resize(key_cols, Value::Empty);
        line.extend(agg(group_rows.get(id).map(Vec::as_slice).unwrap_or_default()));
        line
    };
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| paths.get(a).cmp(&paths.get(b)));
    let mut out = Vec::with_capacity(lines.len().saturating_add(ids.len()));
    let mut prev: Option<(Vec<Value>, &[usize])> = None;
    for &i in &order {
        let (Some(line), Some(path)) = (lines.get_mut(i).map(std::mem::take), paths.get(i)) else { continue };
        // The first level at which this line leaves the previous line's groups.
        let split = prev.as_ref().map_or(0, |(_, p)| p.iter().zip(path).take_while(|(a, b)| a == b).count());
        if before {
            for (level, &id) in path.iter().enumerate().skip(split) {
                out.push(subtotal(&line, level, id));
            }
        } else if let Some((keys, p)) = &prev {
            for (level, &id) in p.iter().enumerate().skip(split).rev() {
                out.push(subtotal(keys, level, id));
            }
        }
        prev = Some((line.get(..key_cols).unwrap_or_default().to_vec(), path));
        out.push(line);
    }
    if !before && let Some((keys, p)) = &prev {
        for (level, &id) in p.iter().enumerate().rev() {
            out.push(subtotal(keys, level, id));
        }
    }
    Ok(out)
}

/// Height and width of a PIVOTBY grid: `header_rows` (one per column field, plus the value-name row) and
/// `body_rows` (row keys plus the total row), each `lead` row-field cells wide plus `value_cols` cells per column key.
fn pivot_dims(header_rows: usize, body_rows: usize, col_keys: usize, value_cols: usize, lead: usize) -> (usize, usize) {
    (header_rows.saturating_add(body_rows), col_keys.saturating_mul(value_cols).saturating_add(lead))
}

fn pivotby(args: &[Arg], ctx: &mut dyn Ctx) -> R<Value> {
    let rf = arr_arg(args, 0)?;
    let cf = arr_arg(args, 1)?;
    let vals = arr_arg(args, 2)?;
    let f = agg_arg(args, 3, ctx)?;
    let fh = opt_count(args, 4)?;
    let row_total = opt_count(args, 5)?.unwrap_or(1);
    let row_sort = opt_count(args, 6)?.unwrap_or(1);
    let col_total = opt_count(args, 7)?.unwrap_or(1);
    let col_sort = opt_count(args, 8)?.unwrap_or(1);
    let filter = opt_arr(args, 9)?;
    let p = prepare(&[&rf, &cf], &vals, fh, filter.as_ref())?;
    let rkeys: Vec<Vec<Value>> = p.rows.iter().map(|&r| key_row(&rf, r)).collect();
    let ckeys: Vec<Vec<Value>> = p.rows.iter().map(|&r| key_row(&cf, r)).collect();
    let rk = distinct_keys(&rkeys, if row_sort.unsigned_abs() as usize <= rf.cols { row_sort } else { 0 });
    let ck = distinct_keys(&ckeys, if col_sort.unsigned_abs() as usize <= cf.cols { col_sort } else { 0 });
    // Source rows of every row key, column key and pair, built in one pass, so a total reads
    // its rows instead of rescanning the whole source once per total.
    let mut cell: HashMap<(String, String), Vec<usize>> = HashMap::new();
    let mut by_row: HashMap<String, Vec<usize>> = HashMap::new();
    let mut by_col: HashMap<String, Vec<usize>> = HashMap::new();
    for ((r, c), &i) in rkeys.iter().zip(ckeys.iter()).zip(p.rows.iter()) {
        let (r, c) = (row_key(r), row_key(c));
        by_row.entry(r.clone()).or_default().push(i);
        by_col.entry(c.clone()).or_default().push(i);
        cell.entry((r, c)).or_default().push(i);
    }
    let mut agg = |rows: &[usize], c: usize| -> Value {
        if rows.is_empty() {
            return Value::Empty;
        }
        let vs: Vec<&Value> = rows.iter().filter_map(|&r| vals.get(r, c)).collect();
        aggregate_with(f, &vs, ctx)
    };
    let rows_for = |r: Option<&Vec<Value>>, c: Option<&Vec<Value>>| -> Vec<usize> {
        match (r, c) {
            (Some(r), Some(c)) => cell.get(&(row_key(r), row_key(c))).cloned().unwrap_or_default(),
            (Some(r), None) => by_row.get(&row_key(r)).cloned().unwrap_or_default(),
            (None, Some(c)) => by_col.get(&row_key(c)).cloned().unwrap_or_default(),
            (None, None) => p.rows.clone(),
        }
    };
    let mut cols: Vec<Option<&Vec<Value>>> = ck.iter().map(Some).collect();
    if col_total > 0 {
        cols.push(None);
    } else if col_total < 0 {
        cols.insert(0, None);
    }
    let lead = rf.cols;
    let value_row = vals.cols > 1 || p.show_headers;
    let header_rows = cf.cols.saturating_add(usize::from(value_row));
    let body_rows = rk.len().saturating_add(usize::from(row_total != 0));
    // Reject before building any header or body row: every column-field header row is as wide as the whole grid.
    let (height, width) = pivot_dims(header_rows, body_rows, cols.len(), vals.cols, lead);
    if height.saturating_mul(width) > MAX_CELLS {
        return Err(CellError::Num);
    }
    let mut out: Vec<Vec<Value>> = Vec::new();
    // Column header rows: one per column field, plus value names when several value columns.
    for level in 0..cf.cols {
        let mut line = vec![Value::Empty; lead];
        for c in &cols {
            for _ in 0..vals.cols {
                line.push(match c {
                    Some(k) => k.get(level).cloned().unwrap_or(Value::Empty),
                    None if level == 0 => Value::from("Total"),
                    None => Value::Empty,
                });
            }
        }
        out.push(line);
    }
    if value_row {
        let mut line: Vec<Value> = if p.show_headers { p.field_headers.iter().take(lead).cloned().collect() } else { vec![Value::Empty; lead] };
        line.resize(lead, Value::Empty);
        for _ in &cols {
            line.extend(p.value_headers.iter().cloned());
        }
        out.push(line);
    }
    let mut body_line = |r: Option<&Vec<Value>>| -> Vec<Value> {
        let mut line: Vec<Value> = match r {
            Some(k) => k.clone(),
            None => {
                let mut l = vec![Value::from("Total")];
                l.resize(lead, Value::Empty);
                l
            }
        };
        for c in &cols {
            let rows = rows_for(r, *c);
            for vc in 0..vals.cols {
                line.push(agg(&rows, vc));
            }
        }
        line
    };
    if row_total < 0 {
        out.push(body_line(None));
    }
    for k in &rk {
        out.push(body_line(Some(k)));
    }
    if row_total > 0 {
        out.push(body_line(None));
    }
    from_rows(out, &Value::Empty)
}

pub(crate) fn specs() -> Vec<FnSpec> {
    vec![
        f!(
            "FILTER",
            2,
            3,
            Lookup,
            A,
            "FILTER(array, include, [if_empty])",
            "Keeps the rows or columns of an array whose include flag is TRUE.",
            filter
        ),
        f!(
            "SORT",
            1,
            4,
            Lookup,
            A,
            "SORT(array, [sort_index], [sort_order], [by_col])",
            "Sorts the rows (or columns) of an array by one or more of its columns.",
            sort
        ),
        f!(
            "SORTBY",
            2,
            VAR,
            Lookup,
            A,
            "SORTBY(array, by_array1, [sort_order1], ...)",
            "Sorts an array by the values of other arrays of matching size.",
            sortby
        ),
        f!("UNIQUE", 1, 3, Lookup, A, "UNIQUE(array, [by_col], [exactly_once])", "Returns the distinct rows or columns of an array.", unique),
        f!("SEQUENCE", 1, 4, MathTrig, A, "SEQUENCE(rows, [columns], [start], [step])", "Builds an array of evenly stepped numbers.", sequence),
        f!(volatile;
            "RANDARRAY",
            0,
            5,
            MathTrig,
            A,
            "RANDARRAY([rows], [columns], [min], [max], [whole_number])",
            "Builds an array of random numbers in a range.",
            randarray
        ),
        f!("VSTACK", 1, VAR, Lookup, A, "VSTACK(array1, [array2], ...)", "Stacks arrays on top of each other.", vstack),
        f!("HSTACK", 1, VAR, Lookup, A, "HSTACK(array1, [array2], ...)", "Places arrays side by side.", hstack),
        f!(
            "TOROW",
            1,
            3,
            Lookup,
            A,
            "TOROW(array, [ignore], [scan_by_column])",
            "Flattens an array into a single row, optionally skipping blanks or errors.",
            torow
        ),
        f!(
            "TOCOL",
            1,
            3,
            Lookup,
            A,
            "TOCOL(array, [ignore], [scan_by_column])",
            "Flattens an array into a single column, optionally skipping blanks or errors.",
            tocol
        ),
        f!(
            "WRAPROWS",
            2,
            3,
            Lookup,
            A,
            "WRAPROWS(vector, wrap_count, [pad_with])",
            "Breaks a row or column into rows of the given length.",
            wraprows
        ),
        f!(
            "WRAPCOLS",
            2,
            3,
            Lookup,
            A,
            "WRAPCOLS(vector, wrap_count, [pad_with])",
            "Breaks a row or column into columns of the given length.",
            wrapcols
        ),
        f!("TAKE", 2, 3, Lookup, A, "TAKE(array, rows, [columns])", "Keeps rows or columns from the start (or end) of an array.", take_fn),
        f!("DROP", 2, 3, Lookup, A, "DROP(array, rows, [columns])", "Removes rows or columns from the start (or end) of an array.", drop_fn),
        f!(
            "CHOOSEROWS",
            2,
            VAR,
            Lookup,
            A,
            "CHOOSEROWS(array, row_num1, [row_num2], ...)",
            "Picks rows of an array by position (negative counts from the end).",
            chooserows
        ),
        f!(
            "CHOOSECOLS",
            2,
            VAR,
            Lookup,
            A,
            "CHOOSECOLS(array, col_num1, [col_num2], ...)",
            "Picks columns of an array by position (negative counts from the end).",
            choosecols
        ),
        f!(
            "EXPAND",
            2,
            4,
            Lookup,
            A,
            "EXPAND(array, rows, [columns], [pad_with])",
            "Grows an array to the given size, filling new cells with a pad value.",
            expand
        ),
        f!(
            "GROUPBY",
            3,
            8,
            Lookup,
            A,
            "GROUPBY(row_fields, values, function, [field_headers], [total_depth], [sort_order], [filter_array], [field_relationship])",
            "Groups rows by key fields and aggregates the values of each group.",
            groupby
        ),
        f!(
            "PIVOTBY",
            4,
            11,
            Lookup,
            A,
            "PIVOTBY(row_fields, col_fields, values, function, [field_headers], [row_total_depth], [row_sort_order], [col_total_depth], [col_sort_order], [filter_array], [relative_to])",
            "Builds a summary table grouping values by row and column fields.",
            pivotby
        ),
    ]
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellError, Value};

    use crate::util::testutil::*;

    fn grid() -> Value {
        arr(vec![vec![tv("b"), nv(2.0), nv(20.0)], vec![tv("a"), nv(3.0), nv(10.0)], vec![tv("c"), nv(1.0), nv(30.0)]])
    }

    #[test]
    fn filter() {
        let inc = arr(vec![vec![Value::Bool(true)], vec![Value::Bool(false)], vec![Value::Bool(true)]]);
        let r = ev("FILTER", vec![rf(grid()), av(inc)]);
        assert_eq!(rows_of(&r), vec![vec![tv("b"), nv(2.0), nv(20.0)], vec![tv("c"), nv(1.0), nv(30.0)]]);
        let r = ev("FILTER", vec![rf(grid()), av(col(&[0.0, 1.0, 0.0]))]);
        assert_eq!(rows_of(&r), vec![vec![tv("a"), nv(3.0), nv(10.0)]]);
        let r = ev("FILTER", vec![rf(grid()), av(row(&[1.0, 0.0, 1.0]))]);
        assert_eq!(rows_of(&r).first().unwrap(), &vec![tv("b"), nv(20.0)]);
        is_err(ev("FILTER", vec![rf(grid()), av(col(&[0.0, 0.0, 0.0]))]), CellError::Calc);
        is_text(ev("FILTER", vec![rf(grid()), av(col(&[0.0, 0.0, 0.0])), t("none")]), "none");
        is_err(ev("FILTER", vec![rf(grid()), av(col(&[1.0, 0.0]))]), CellError::Value);
        let errinc = arr(vec![vec![nv(1.0)], vec![Value::Error(CellError::Div0)], vec![nv(1.0)]]);
        is_err(ev("FILTER", vec![rf(grid()), av(errinc)]), CellError::Div0);
    }

    #[test]
    fn sort() {
        let r = ev("SORT", vec![rf(grid())]);
        assert_eq!(rows_of(&r).iter().map(|l| l[0].clone()).collect::<Vec<_>>(), vec![tv("a"), tv("b"), tv("c")]);
        let r = ev("SORT", vec![rf(grid()), n(2.0), n(-1.0)]);
        assert_eq!(rows_of(&r).iter().map(|l| l[1].clone()).collect::<Vec<_>>(), vec![nv(3.0), nv(2.0), nv(1.0)]);
        let r = ev("SORT", vec![av(row(&[3.0, 1.0, 2.0])), n(1.0), n(1.0), b(true)]);
        assert_eq!(rows_of(&r), vec![vec![nv(1.0), nv(2.0), nv(3.0)]]);
        is_err(ev("SORT", vec![rf(grid()), n(4.0)]), CellError::Value);
        is_err(ev("SORT", vec![rf(grid()), n(1.0), n(2.0)]), CellError::Value);
        let mixed = arr(vec![vec![tv("x")], vec![nv(5.0)], vec![Value::Bool(true)], vec![nv(-1.0)]]);
        assert_eq!(rows_of(&ev("SORT", vec![av(mixed)])), vec![vec![nv(-1.0)], vec![nv(5.0)], vec![tv("x")], vec![Value::Bool(true)]]);
        // Multiple keys.
        let m = arr(vec![vec![nv(1.0), nv(2.0)], vec![nv(1.0), nv(1.0)], vec![nv(0.0), nv(5.0)]]);
        let r = ev("SORT", vec![av(m), av(row(&[1.0, 2.0])), av(row(&[1.0, -1.0]))]);
        assert_eq!(rows_of(&r), vec![vec![nv(0.0), nv(5.0)], vec![nv(1.0), nv(2.0)], vec![nv(1.0), nv(1.0)]]);
    }

    /// Security: a column repeated in sort_index keys the sort once. The keys copied per row stay
    /// within the row, however long sort_index is, and a repeat (even with the other order) can't
    /// change the result, since its first use already ordered the rows.
    #[test]
    fn sort_drops_repeated_sort_indices() {
        let m = || av(arr(vec![vec![nv(1.0), nv(2.0)], vec![nv(1.0), nv(1.0)], vec![nv(0.0), nv(5.0)]]));
        let want = vec![vec![nv(0.0), nv(5.0)], vec![nv(1.0), nv(2.0)], vec![nv(1.0), nv(1.0)]];
        let r = ev("SORT", vec![m(), av(row(&[1.0, 1.0, 2.0, 1.0])), av(row(&[1.0, -1.0, -1.0, 1.0]))]);
        assert_eq!(rows_of(&r), want);
        let idx = vec![1.0; 1_000_000];
        let tall = av(gridcraft_core::Array::filled(2_000, 1, nv(1.0)).into());
        let r = ev("SORT", vec![tall, av(row(&idx))]);
        assert_eq!(rows_of(&r).len(), 2_000);
    }

    #[test]
    fn sortby() {
        let r = ev("SORTBY", vec![av(col(&[1.0, 2.0, 3.0])), av(col(&[30.0, 10.0, 20.0]))]);
        assert_eq!(rows_of(&r), vec![vec![nv(2.0)], vec![nv(3.0)], vec![nv(1.0)]]);
        let r = ev("SORTBY", vec![av(col(&[1.0, 2.0, 3.0])), av(col(&[30.0, 10.0, 20.0])), n(-1.0)]);
        assert_eq!(rows_of(&r), vec![vec![nv(1.0)], vec![nv(3.0)], vec![nv(2.0)]]);
        let r = ev("SORTBY", vec![av(row(&[1.0, 2.0])), av(row(&[2.0, 1.0]))]);
        assert_eq!(rows_of(&r), vec![vec![nv(2.0), nv(1.0)]]);
        is_err(ev("SORTBY", vec![av(col(&[1.0, 2.0, 3.0])), av(col(&[1.0, 2.0]))]), CellError::Value);
        is_err(ev("SORTBY", vec![av(col(&[1.0, 2.0])), av(col(&[1.0, 2.0])), n(0.0)]), CellError::Value);
    }

    #[test]
    fn unique_keeps_rows_containing_separator_apart() {
        let m = arr(vec![vec![tv("a"), tv("b\u{1}tc")], vec![tv("a\u{1}tb"), tv("c")]]);
        assert_eq!(rows_of(&ev("UNIQUE", vec![av(m)])).len(), 2);
    }

    #[test]
    fn unique() {
        let v = arr(vec![vec![tv("a")], vec![tv("B")], vec![tv("A")], vec![tv("c")], vec![tv("b")]]);
        assert_eq!(rows_of(&ev("UNIQUE", vec![av(v.clone())])), vec![vec![tv("a")], vec![tv("B")], vec![tv("c")]]);
        assert_eq!(rows_of(&ev("UNIQUE", vec![av(v.clone()), b(false), b(true)])), vec![vec![tv("c")]]);
        assert_eq!(rows_of(&ev("UNIQUE", vec![av(row(&[1.0, 2.0, 1.0])), b(true)])), vec![vec![nv(1.0), nv(2.0)]]);
        is_err(ev("UNIQUE", vec![av(col(&[1.0, 1.0])), b(false), b(true)]), CellError::Calc);
        let m = arr(vec![vec![nv(1.0), nv(2.0)], vec![nv(1.0), nv(2.0)], vec![nv(1.0), nv(3.0)]]);
        assert_eq!(rows_of(&ev("UNIQUE", vec![av(m)])).len(), 2);
    }

    #[test]
    fn sequence_and_rand() {
        assert_eq!(rows_of(&ev("SEQUENCE", vec![n(3.0)])), vec![vec![nv(1.0)], vec![nv(2.0)], vec![nv(3.0)]]);
        assert_eq!(rows_of(&ev("SEQUENCE", vec![n(2.0), n(2.0), n(0.0), n(5.0)])), vec![vec![nv(0.0), nv(5.0)], vec![nv(10.0), nv(15.0)]]);
        assert_eq!(rows_of(&ev("SEQUENCE", vec![empty(), n(3.0)])), vec![vec![nv(1.0), nv(2.0), nv(3.0)]]);
        is_err(ev("SEQUENCE", vec![n(0.0)]), CellError::Calc);
        is_err(ev("SEQUENCE", vec![n(-1.0)]), CellError::Value);
        is_err(ev("SEQUENCE", vec![n(1e6), n(1e6)]), CellError::Num);
        is_err(ev("SEQUENCE", vec![t("x")]), CellError::Value);
        let r = ev("RANDARRAY", vec![n(3.0), n(4.0), n(1.0), n(6.0), b(true)]);
        let rows = rows_of(&r);
        assert_eq!((rows.len(), rows[0].len()), (3, 4));
        for v in rows.iter().flatten() {
            let x = v.as_f64().unwrap();
            assert!((1.0..=6.0).contains(&x) && x.fract() == 0.0);
        }
        let x = ev("RANDARRAY", vec![]);
        assert!(x.as_f64().is_some_and(|x| (0.0..1.0).contains(&x)) || matches!(x, Value::Array(_)));
        is_err(ev("RANDARRAY", vec![n(1.0), n(1.0), n(5.0), n(1.0)]), CellError::Value);
        assert!(crate::lookup("RANDARRAY").unwrap().volatile);
    }

    #[test]
    fn stacking() {
        let r = ev("VSTACK", vec![av(row(&[1.0, 2.0])), av(row(&[3.0]))]);
        assert_eq!(rows_of(&r), vec![vec![nv(1.0), nv(2.0)], vec![nv(3.0), Value::Error(CellError::NA)]]);
        let r = ev("HSTACK", vec![av(col(&[1.0, 2.0])), n(3.0)]);
        assert_eq!(rows_of(&r), vec![vec![nv(1.0), nv(3.0)], vec![nv(2.0), Value::Error(CellError::NA)]]);
        let m = arr(vec![vec![nv(1.0), Value::Empty], vec![Value::Error(CellError::Div0), nv(4.0)]]);
        assert_eq!(rows_of(&ev("TOROW", vec![av(m.clone())])).first().unwrap().len(), 4);
        assert_eq!(rows_of(&ev("TOROW", vec![av(m.clone()), n(1.0)])), vec![vec![nv(1.0), Value::Error(CellError::Div0), nv(4.0)]]);
        assert_eq!(rows_of(&ev("TOCOL", vec![av(m.clone()), n(3.0)])), vec![vec![nv(1.0)], vec![nv(4.0)]]);
        assert_eq!(rows_of(&ev("TOCOL", vec![av(m.clone()), n(2.0), b(true)])), vec![vec![nv(1.0)], vec![Value::Empty], vec![nv(4.0)]]);
        is_err(ev("TOCOL", vec![av(m), n(4.0)]), CellError::Value);
        is_err(ev("TOROW", vec![av(arr(vec![vec![Value::Empty]])), n(1.0)]), CellError::Calc);
    }

    #[test]
    fn output_size_checked_before_padding() {
        assert_eq!(super::check_output_size(1_000_001, 10), Err(CellError::Num));
        assert_eq!(super::check_output_size(usize::MAX, 2), Err(CellError::Num));
        assert_eq!(super::check_output_size(1_000_000, 10), Ok(()));
        // A 1M-row column beside a 9-wide row: ~1M input cells, but the padded result would be 10,000,010.
        let tall = Value::from(gridcraft_core::Array::column(vec![nv(1.0); 1_000_001]));
        let wide = row(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        is_err(ev("HSTACK", vec![av(tall), av(wide)]), CellError::Num);
    }

    #[test]
    fn wrapping() {
        let r = ev("WRAPROWS", vec![av(row(&[1.0, 2.0, 3.0, 4.0, 5.0])), n(2.0)]);
        assert_eq!(rows_of(&r), vec![vec![nv(1.0), nv(2.0)], vec![nv(3.0), nv(4.0)], vec![nv(5.0), Value::Error(CellError::NA)]]);
        let r = ev("WRAPCOLS", vec![av(row(&[1.0, 2.0, 3.0])), n(2.0), n(0.0)]);
        assert_eq!(rows_of(&r), vec![vec![nv(1.0), nv(3.0)], vec![nv(2.0), nv(0.0)]]);
        is_err(ev("WRAPROWS", vec![rf(grid()), n(2.0)]), CellError::Value);
        is_err(ev("WRAPROWS", vec![av(row(&[1.0])), n(0.0)]), CellError::Num);
    }

    #[test]
    fn take_drop_choose_expand() {
        let r = ev("TAKE", vec![rf(grid()), n(2.0)]);
        assert_eq!(rows_of(&r).len(), 2);
        let r = ev("TAKE", vec![rf(grid()), n(-1.0), n(1.0)]);
        assert_eq!(rows_of(&r), vec![vec![tv("c")]]);
        let r = ev("TAKE", vec![rf(grid()), empty(), n(-1.0)]);
        assert_eq!(rows_of(&r), vec![vec![nv(20.0)], vec![nv(10.0)], vec![nv(30.0)]]);
        assert_eq!(rows_of(&ev("TAKE", vec![rf(grid()), n(10.0)])).len(), 3);
        is_err(ev("TAKE", vec![rf(grid()), n(0.0)]), CellError::Calc);
        let r = ev("DROP", vec![rf(grid()), n(1.0), n(2.0)]);
        assert_eq!(rows_of(&r), vec![vec![nv(10.0)], vec![nv(30.0)]]);
        let r = ev("DROP", vec![rf(grid()), n(-2.0)]);
        assert_eq!(rows_of(&r), vec![vec![tv("b"), nv(2.0), nv(20.0)]]);
        is_err(ev("DROP", vec![rf(grid()), n(3.0)]), CellError::Calc);
        let r = ev("CHOOSEROWS", vec![rf(grid()), n(3.0), n(-3.0)]);
        assert_eq!(rows_of(&r), vec![vec![tv("c"), nv(1.0), nv(30.0)], vec![tv("b"), nv(2.0), nv(20.0)]]);
        let r = ev("CHOOSECOLS", vec![rf(grid()), av(row(&[1.0, 3.0]))]);
        assert_eq!(rows_of(&r).first().unwrap(), &vec![tv("b"), nv(20.0)]);
        is_err(ev("CHOOSEROWS", vec![rf(grid()), n(4.0)]), CellError::Value);
        is_err(ev("CHOOSECOLS", vec![rf(grid()), n(0.0)]), CellError::Value);
        let r = ev("EXPAND", vec![av(row(&[1.0, 2.0])), n(2.0), empty(), n(0.0)]);
        assert_eq!(rows_of(&r), vec![vec![nv(1.0), nv(2.0)], vec![nv(0.0), nv(0.0)]]);
        let r = ev("EXPAND", vec![av(row(&[1.0])), n(1.0), n(2.0)]);
        assert_eq!(rows_of(&r), vec![vec![nv(1.0), Value::Error(CellError::NA)]]);
        is_err(ev("EXPAND", vec![rf(grid()), n(2.0)]), CellError::Value);
        is_err(ev("EXPAND", vec![av(row(&[1.0])), n(1e7), n(1e4)]), CellError::Num);
    }

    fn sales() -> (Value, Value, Value) {
        let region = arr(vec![vec![tv("Region")], vec![tv("East")], vec![tv("West")], vec![tv("East")], vec![tv("West")], vec![tv("North")]]);
        let item = arr(vec![vec![tv("Item")], vec![tv("A")], vec![tv("A")], vec![tv("B")], vec![tv("B")], vec![tv("A")]]);
        let amt = arr(vec![vec![tv("Amount")], vec![nv(10.0)], vec![nv(20.0)], vec![nv(5.0)], vec![nv(7.0)], vec![nv(1.0)]]);
        (region, item, amt)
    }

    #[test]
    fn groupby() {
        let (region, _, amt) = sales();
        let r = ev("GROUPBY", vec![rf(region.clone()), rf(amt.clone()), t("SUM")]);
        assert_eq!(
            rows_of(&r),
            vec![
                vec![tv("Region"), tv("Amount")],
                vec![tv("East"), nv(15.0)],
                vec![tv("North"), nv(1.0)],
                vec![tv("West"), nv(27.0)],
                vec![tv("Total"), nv(43.0)],
            ]
        );
        let r = ev("GROUPBY", vec![rf(region.clone()), rf(amt.clone()), n(2.0), n(1.0), n(0.0), n(-1.0)]);
        assert_eq!(rows_of(&r), vec![vec![tv("West"), nv(2.0)], vec![tv("North"), nv(1.0)], vec![tv("East"), nv(2.0)]]);
        let r = ev("GROUPBY", vec![rf(region.clone()), rf(amt.clone()), t("max"), n(1.0), n(0.0), n(-2.0)]);
        assert_eq!(rows_of(&r).first().unwrap(), &vec![tv("West"), nv(20.0)]);
        let filt = arr(vec![
            vec![Value::Empty],
            vec![Value::Bool(true)],
            vec![Value::Bool(false)],
            vec![Value::Bool(true)],
            vec![Value::Bool(true)],
            vec![Value::Bool(true)],
        ]);
        let r = ev("GROUPBY", vec![rf(region.clone()), rf(amt.clone()), t("AVERAGE"), n(1.0), n(0.0), n(1.0), av(filt)]);
        assert_eq!(rows_of(&r), vec![vec![tv("East"), nv(7.5)], vec![tv("North"), nv(1.0)], vec![tv("West"), nv(7.0)]]);
        is_err(ev("GROUPBY", vec![rf(region.clone()), rf(amt.clone()), t("BOGUS")]), CellError::Value);
        is_err(ev("GROUPBY", vec![rf(region), av(col(&[1.0])), t("SUM")]), CellError::Value);
    }

    #[test]
    fn groupby_subtotals_past_the_size_limit_are_refused_before_they_are_built() {
        // One row with 4000 key columns at depth 4000 asks for 3999 subtotal lines, each 4001 cells
        // wide: 16 million cells, over the limit. It is #NUM! before any subtotal line exists (it
        // used to build them all, nesting 4000 levels deep, and refuse them afterwards).
        let keys = av(arr(vec![(0..4000).map(|i| nv(f64::from(i))).collect()]));
        is_err(ev("GROUPBY", vec![keys, av(col(&[1.0])), t("SUM"), empty(), n(4000.0)]), CellError::Num);
    }

    #[test]
    fn groupby_total_depth_adds_subtotals() {
        // #1456 item 1, values as Excel 365 shows them.
        let keys2 = || av(arr(vec![vec![tv("a"), tv("x")], vec![tv("a"), tv("y")], vec![tv("b"), tv("x")]]));
        let vals3 = || av(col(&[1.0, 2.0, 3.0]));
        let e = Value::Empty;
        let r = ev("GROUPBY", vec![keys2(), vals3(), t("SUM"), empty(), n(2.0)]);
        assert_eq!(
            rows_of(&r),
            vec![
                vec![tv("a"), tv("x"), nv(1.0)],
                vec![tv("a"), tv("y"), nv(2.0)],
                vec![tv("a"), e.clone(), nv(3.0)],
                vec![tv("b"), tv("x"), nv(3.0)],
                vec![tv("b"), e.clone(), nv(3.0)],
                vec![tv("Grand Total"), e.clone(), nv(6.0)],
            ]
        );
        let r = ev("GROUPBY", vec![keys2(), vals3(), t("SUM"), empty(), n(-2.0)]);
        assert_eq!(
            rows_of(&r),
            vec![
                vec![tv("Grand Total"), e.clone(), nv(6.0)],
                vec![tv("a"), e.clone(), nv(3.0)],
                vec![tv("a"), tv("x"), nv(1.0)],
                vec![tv("a"), tv("y"), nv(2.0)],
                vec![tv("b"), e.clone(), nv(3.0)],
                vec![tv("b"), tv("x"), nv(3.0)],
            ]
        );
        let keys3 = || {
            av(arr(vec![
                vec![tv("a"), tv("x"), tv("p")],
                vec![tv("a"), tv("x"), tv("q")],
                vec![tv("a"), tv("y"), tv("p")],
                vec![tv("b"), tv("x"), tv("p")],
            ]))
        };
        let vals4 = || av(col(&[1.0, 2.0, 3.0, 4.0]));
        let r = ev("GROUPBY", vec![keys3(), vals4(), t("SUM"), empty(), n(3.0)]);
        let line = |k: [&str; 3], v: f64| k.iter().map(|s| if s.is_empty() { Value::Empty } else { tv(s) }).chain([nv(v)]).collect::<Vec<_>>();
        assert_eq!(
            rows_of(&r),
            vec![
                line(["a", "x", "p"], 1.0),
                line(["a", "x", "q"], 2.0),
                line(["a", "x", ""], 3.0),
                line(["a", "y", "p"], 3.0),
                line(["a", "y", ""], 3.0),
                line(["a", "", ""], 6.0),
                line(["b", "x", "p"], 4.0),
                line(["b", "x", ""], 4.0),
                line(["b", "", ""], 4.0),
                line(["Grand Total", "", ""], 10.0),
            ]
        );
        // Depth 1 keeps "Total"; a depth past the key columns, or subtotals of a flat table, are errors.
        let r = ev("GROUPBY", vec![keys2(), vals3(), t("SUM"), empty(), n(1.0)]);
        assert_eq!(rows_of(&r).last().unwrap(), &vec![tv("Total"), e, nv(6.0)]);
        is_err(ev("GROUPBY", vec![av(arr(vec![vec![tv("a")], vec![tv("b")]])), av(col(&[1.0, 2.0])), t("SUM"), empty(), n(2.0)]), CellError::Value);
        is_err(ev("GROUPBY", vec![keys2(), vals3(), t("SUM"), empty(), n(2.0), empty(), empty(), n(1.0)]), CellError::Value);
    }

    #[test]
    fn pivotby() {
        let (region, item, amt) = sales();
        let r = ev("PIVOTBY", vec![rf(region), rf(item), rf(amt), t("SUM"), n(1.0)]);
        assert_eq!(
            rows_of(&r),
            vec![
                vec![Value::Empty, tv("A"), tv("B"), tv("Total")],
                vec![tv("East"), nv(10.0), nv(5.0), nv(15.0)],
                vec![tv("North"), nv(1.0), Value::Empty, nv(1.0)],
                vec![tv("West"), nv(20.0), nv(7.0), nv(27.0)],
                vec![tv("Total"), nv(31.0), nv(12.0), nv(43.0)],
            ]
        );
    }

    /// Security: PIVOTBY totals read their source rows from indexes built in the grouping pass,
    /// rather than rescanning the source once per total, so many distinct keys stay linear.
    #[test]
    fn pivotby_totals_with_many_groups() {
        let keys: Vec<f64> = (0..20_000).map(f64::from).collect();
        let r = rows_of(&ev("PIVOTBY", vec![av(col(&keys)), av(col(&vec![1.0; 20_000])), av(col(&vec![2.0; 20_000])), t("SUM")]));
        assert_eq!(r.get(1), Some(&vec![nv(0.0), nv(2.0), nv(2.0)]));
        assert_eq!(r.last(), Some(&vec![tv("Total"), nv(40_000.0), nv(40_000.0)]));
    }

    #[test]
    fn pivotby_size_check_counts_column_header_rows() {
        // One row, n column fields and n value columns: n header rows + value-name row + body row + total row,
        // each 1 + 2n wide. The size checked up front matches the grid PIVOTBY builds.
        let dims = |k: u32, totals: bool| {
            let fields: Vec<f64> = (0..k).map(f64::from).collect();
            let mut args = vec![av(row(&[1.0])), av(row(&fields)), av(row(&fields)), t("SUM")];
            if !totals {
                args.extend([empty(), n(0.0), empty(), n(0.0)]);
            }
            let r = rows_of(&ev("PIVOTBY", args));
            (r.len(), r.first().map_or(0, Vec::len))
        };
        assert_eq!(dims(2, true), super::pivot_dims(3, 2, 2, 2, 1));
        assert_eq!(dims(10, true), (13, 21));
        assert_eq!(dims(10, true), super::pivot_dims(11, 2, 2, 10, 1));
        assert_eq!(dims(10, false), (12, 11));
        assert_eq!(dims(10, false), super::pivot_dims(11, 1, 1, 10, 1));
        // n = 2300: the 2300 header rows alone are 10.58M cells, so the check must count them before any is built.
        let (h, w) = super::pivot_dims(2301, 2, 2, 2300, 1);
        assert!(2300 * w > crate::util::MAX_CELLS && h * w > crate::util::MAX_CELLS);
        let wide: Vec<f64> = (0..2300).map(f64::from).collect();
        is_err(ev("PIVOTBY", vec![av(row(&[1.0])), av(row(&wide)), av(row(&wide)), t("SUM")]), CellError::Num);
    }
}
