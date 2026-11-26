//! Lookup and reference functions over evaluated arrays: VLOOKUP, HLOOKUP, LOOKUP, MATCH,
//! XLOOKUP, XMATCH, INDEX, ROWS, COLUMNS, TRANSPOSE, ADDRESS.
//!
//! Reference-only behaviour (INDEX returning a reference, OFFSET, ROW…) belongs to the host.

use std::cmp::Ordering;

use gridcraft_core::{Array, CellError, Value, col_to_letters, compare};

use crate::criteria::{has_wildcards, lookup_equal};
use crate::util::{R, arg, array_val, as_array, has, int, num, opt_bool, opt_int, opt_num, text, text_val};
use crate::{Arg, Ctx, FnSpec};

// ---------------------------------------------------------------------------------------------
// Search helpers

/// Lookup needle: the scalar to look for. Blank looks for 0; errors propagate.
fn needle(args: &[Arg], i: usize) -> R<Value> {
    match arg(args, i)?.value.scalar() {
        Value::Error(e) => Err(e),
        Value::Empty => Ok(Value::Number(0.0)),
        v => Ok(v),
    }
}

/// Order of two values for the approximate and binary searches: [`compare`], except that numbers
/// compare exactly. Excel's `=` treats numbers equal to 15 significant digits as equal, but its
/// lookups don't. Observed in Excel 365 with A1 holding
/// `=0.1+0.2` (0.30000000000000004): `A1=0.3` is TRUE, yet `MATCH(0.3,A1,1)` is #N/A because
/// A1 is larger, and in {0.1;0.2;0.3;A1;0.4} `MATCH(0.3,…,1)` is 3 and `MATCH(A1,…,1)` is 4.
/// The same holds for `=100+1E-14` against 100, and in every XLOOKUP / XMATCH mode.
fn lookup_order(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.partial_cmp(y).unwrap_or_else(|| compare(a, b)),
        _ => compare(a, b),
    }
}

fn same_type(a: &Value, b: &Value) -> bool {
    matches!((a, b), (Value::Number(_), Value::Number(_)) | (Value::Text(_), Value::Text(_)) | (Value::Bool(_), Value::Bool(_)))
}

/// Exact search (first match, or last when `reverse`). Text needles match wildcards when `wild`.
fn find_exact(x: &Value, hay: &[Value], wild: bool, reverse: bool) -> Option<usize> {
    let wild = wild && matches!(x, Value::Text(t) if has_wildcards(t));
    if reverse { hay.iter().rposition(|v| lookup_equal(x, v, wild)) } else { hay.iter().position(|v| lookup_equal(x, v, wild)) }
}

/// Approximate match of VLOOKUP, HLOOKUP, LOOKUP and MATCH: the position of the largest entry
/// `<= x` in data sorted ascending (`desc = false`), or of the smallest entry `>= x` in data
/// sorted descending. Like Excel this is a bisection over the whole vector, so on unsorted data
/// it answers what Excel answers rather than a linear "best" entry:
/// - the probe is the middle entry of the remaining window; a probe whose type differs from the
///   needle (text vs number vs logical, blanks, errors) is replaced by the nearest entry of the
///   needle's type below it in the window; when there is none the window moves up past it, so
///   entries of other types are skipped (`LOOKUP(9.9E+307, range)` finds the last number);
/// - a probe equal to the needle ends the search at once (the run of equal entries that follows
///   it is taken, so sorted duplicates give the last one).
///
/// Observed in Excel: `VLOOKUP(534, {"INTEGRAL";0;1;2;534;9999999999;-9999999999}, …)` finds 534,
/// `HLOOKUP("STRING", {"INTEGRAL","DOUBLE","BLANK","STRING","REF","AREA (rows)",<blank>}, …)`
/// finds "STRING" and `LOOKUP("DOUBLE", {"DOUBLE","BLANK","STRING","REF","AREA (rows)"}, …)`
/// finds "DOUBLE". Work is linear in the vector length at worst.
fn bsearch_last(x: &Value, hay: &[Value], desc: bool) -> Option<usize> {
    let mut hi = hay.len().checked_sub(1)?;
    let mut lo = 0usize;
    let mut best = None;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let Some(j) = (lo..=mid).rev().find(|&j| hay.get(j).is_some_and(|v| same_type(x, v))) else {
            lo = mid + 1;
            continue;
        };
        let v = hay.get(j)?;
        let o = lookup_order(v, x);
        if o == Ordering::Equal {
            let mut k = j;
            while hay.get(k + 1).is_some_and(|w| same_type(x, w) && lookup_order(w, x) == Ordering::Equal) {
                k += 1;
            }
            return Some(k);
        }
        let before = if desc { o == Ordering::Greater } else { o == Ordering::Less };
        if before {
            best = Some(j);
            lo = mid + 1;
        } else if j == 0 {
            break;
        } else {
            hi = j - 1;
        }
    }
    best
}

/// Order of an entry relative to the needle for XLOOKUP / XMATCH binary search (search mode ±2):
/// Excel's sort order over all types (numbers < text < logicals < errors), reversed for
/// descending data, with blanks always last like a sort leaves them. Unlike the legacy
/// approximate match no entry is skipped (observed: `XLOOKUP(46523, {9700;"xyz";"xyz";160726;
/// 204100;510300}, …, 0, 1, 2)` returns the row of the first "xyz").
fn binary_order(v: &Value, x: &Value, desc: bool) -> Ordering {
    match v {
        Value::Empty => Ordering::Greater,
        _ if desc => lookup_order(v, x).reverse(),
        _ => lookup_order(v, x),
    }
}

/// Binary search for the first entry not ordered before the needle (`>= x` ascending, `<= x`
/// descending).
fn bsearch_first(x: &Value, hay: &[Value], desc: bool) -> Option<usize> {
    let p = hay.partition_point(|v| binary_order(v, x, desc) == Ordering::Less);
    (p < hay.len()).then_some(p)
}

/// Binary search for the last entry not ordered after the needle (`<= x` ascending, `>= x`
/// descending).
fn bsearch_upto(x: &Value, hay: &[Value], desc: bool) -> Option<usize> {
    hay.partition_point(|v| binary_order(v, x, desc) != Ordering::Greater).checked_sub(1)
}

/// A 1-D view of an array (row or column), `None` when it is 2-D.
fn vector(a: &Array) -> Option<Vec<Value>> {
    if a.rows == 1 || a.cols == 1 { Some(a.data.clone()) } else { None }
}

fn column(a: &Array, c: usize) -> Vec<Value> {
    (0..a.rows).map(|r| a.get(r, c).cloned().unwrap_or(Value::Empty)).collect()
}

fn row(a: &Array, r: usize) -> Vec<Value> {
    (0..a.cols).map(|c| a.get(r, c).cloned().unwrap_or(Value::Empty)).collect()
}

// ---------------------------------------------------------------------------------------------
// VLOOKUP / HLOOKUP / LOOKUP / MATCH

fn vh_lookup(args: &[Arg], vertical: bool) -> R<Value> {
    let x = needle(args, 0)?;
    let table = as_array(&arg(args, 1)?.value);
    if let Value::Error(e) = &arg(args, 1)?.value {
        return Err(*e);
    }
    let idx = num(args, 2)?;
    // NaN too: it would pass `idx < 1.0` and become index 0 (#2163).
    if idx.is_nan() || idx < 1.0 {
        return Err(CellError::Value);
    }
    let idx = (idx.trunc() as usize).saturating_sub(1);
    let approx = opt_bool(args, 3, true)?;
    let (keys, len) = if vertical { (column(&table, 0), table.cols) } else { (row(&table, 0), table.rows) };
    if idx >= len {
        return Err(CellError::Ref);
    }
    let pos = if approx { bsearch_last(&x, &keys, false) } else { find_exact(&x, &keys, true, false) };
    let pos = pos.ok_or(CellError::NA)?;
    let v = if vertical { table.get(pos, idx) } else { table.get(idx, pos) };
    // An empty target stays blank, as in Excel: ISBLANK sees it as blank and `&` /
    // CONCATENATE treat it as ""; only a formula's final result displays 0 (#374).
    Ok(v.cloned().unwrap_or(Value::Empty))
}

fn vlookup(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    vh_lookup(a, true)
}

fn hlookup(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    vh_lookup(a, false)
}

fn lookup(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = needle(args, 0)?;
    let lv = &arg(args, 1)?.value;
    if let Value::Error(e) = lv {
        return Err(*e);
    }
    let la = as_array(lv);
    if args.len() >= 3 {
        let keys = if la.rows == 1 || la.cols == 1 { la.data.clone() } else { column(&la, 0) };
        let pos = bsearch_last(&x, &keys, false).ok_or(CellError::NA)?;
        let rv = &arg(args, 2)?.value;
        if let Value::Error(e) = rv {
            return Err(*e);
        }
        let ra = as_array(rv);
        let v = if ra.rows == 1 || ra.cols == 1 {
            ra.data.get(pos).cloned()
        } else if ra.rows >= keys.len() {
            ra.get(pos, 0).cloned()
        } else {
            None
        };
        return v.ok_or(CellError::NA);
    }
    // Array form: search the first row of a wide array, else the first column; return the
    // matching entry from the last row / column.
    if la.cols > la.rows {
        let keys = row(&la, 0);
        let pos = bsearch_last(&x, &keys, false).ok_or(CellError::NA)?;
        Ok(la.get(la.rows - 1, pos).cloned().unwrap_or(Value::Empty))
    } else {
        let keys = column(&la, 0);
        let pos = bsearch_last(&x, &keys, false).ok_or(CellError::NA)?;
        Ok(la.get(pos, la.cols - 1).cloned().unwrap_or(Value::Empty))
    }
}

fn match_fn(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = needle(args, 0)?;
    let av = &arg(args, 1)?.value;
    if let Value::Error(e) = av {
        return Err(*e);
    }
    let hay = vector(&as_array(av)).ok_or(CellError::NA)?;
    let mt = opt_num(args, 2, 1.0)?;
    let pos = if mt == 0.0 {
        find_exact(&x, &hay, true, false)
    } else if mt > 0.0 {
        bsearch_last(&x, &hay, false)
    } else {
        bsearch_last(&x, &hay, true)
    };
    pos.map(|p| Value::Number((p + 1) as f64)).ok_or(CellError::NA)
}

// ---------------------------------------------------------------------------------------------
// XLOOKUP / XMATCH

/// Core XMATCH search: 0-based position in `hay`.
fn xsearch(x: &Value, hay: &[Value], match_mode: i64, search_mode: i64) -> R<Option<usize>> {
    if !matches!(match_mode, -1..=2) || !matches!(search_mode, -2 | -1 | 1 | 2) {
        return Err(CellError::Value);
    }
    if search_mode.abs() == 2 {
        let desc = search_mode == -2;
        if match_mode == 2 {
            // Wildcards are not supported with binary search: Excel falls back to exact.
            return Ok(bsearch_exact(x, hay, desc));
        }
        return Ok(match match_mode {
            0 => bsearch_exact(x, hay, desc),
            // Next smaller: last <= x (ascending) / first <= x (descending).
            -1 => {
                if desc {
                    bsearch_first(x, hay, true)
                } else {
                    bsearch_upto(x, hay, false)
                }
            }
            // Next larger: first >= x (ascending) / last >= x (descending).
            _ => {
                if desc {
                    bsearch_upto(x, hay, true)
                } else {
                    bsearch_first(x, hay, false)
                }
            }
        });
    }
    let reverse = search_mode == -1;
    if let Some(p) = find_exact(x, hay, match_mode == 2, reverse) {
        return Ok(Some(p));
    }
    if match_mode == 0 || match_mode == 2 {
        return Ok(None);
    }
    // Best candidate: the largest value below (mode -1) or smallest above (mode 1).
    let order: Box<dyn Iterator<Item = usize>> = if reverse { Box::new((0..hay.len()).rev()) } else { Box::new(0..hay.len()) };
    let mut best: Option<usize> = None;
    for i in order {
        let Some(v) = hay.get(i) else { continue };
        if !same_type(x, v) {
            continue;
        }
        let o = lookup_order(v, x);
        let candidate = if match_mode == -1 { o == Ordering::Less } else { o == Ordering::Greater };
        if !candidate {
            continue;
        }
        let better = match best.and_then(|b| hay.get(b)) {
            None => true,
            Some(bv) => {
                let c = lookup_order(v, bv);
                if match_mode == -1 { c == Ordering::Greater } else { c == Ordering::Less }
            }
        };
        if better {
            best = Some(i);
        }
    }
    Ok(best)
}

fn bsearch_exact(x: &Value, hay: &[Value], desc: bool) -> Option<usize> {
    let p = bsearch_first(x, hay, desc)?;
    let v = hay.get(p)?;
    if binary_order(v, x, desc) == Ordering::Equal { Some(p) } else { None }
}

fn xlookup(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = needle(args, 0)?;
    let lv = &arg(args, 1)?.value;
    if let Value::Error(e) = lv {
        return Err(*e);
    }
    let rv = &arg(args, 2)?.value;
    if let Value::Error(e) = rv {
        return Err(*e);
    }
    let la = as_array(lv);
    let ra = as_array(rv);
    let hay = vector(&la).ok_or(CellError::Value)?;
    // Orientation: a column lookup returns rows of the return array; a row lookup returns columns.
    let vertical = if la.rows == 1 && la.cols == 1 { ra.rows == 1 || ra.cols != 1 } else { la.cols == 1 };
    if vertical && ra.rows != la.rows || !vertical && ra.cols != la.cols {
        return Err(CellError::Value);
    }
    let mm = if has(args, 4) { opt_int(args, 4, 0)? } else { 0 };
    let sm = if has(args, 5) { opt_int(args, 5, 1)? } else { 1 };
    match xsearch(&x, &hay, mm, sm)? {
        Some(p) => {
            if vertical {
                let r = row(&ra, p);
                if r.len() == 1 {
                    return Ok(r.into_iter().next().unwrap_or(Value::Empty));
                }
                array_val(1, r.len(), r)
            } else {
                let c = column(&ra, p);
                if c.len() == 1 {
                    return Ok(c.into_iter().next().unwrap_or(Value::Empty));
                }
                array_val(c.len(), 1, c)
            }
        }
        None => {
            if has(args, 3) {
                Ok(arg(args, 3)?.value.clone())
            } else {
                Err(CellError::NA)
            }
        }
    }
}

fn xmatch(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let x = needle(args, 0)?;
    let lv = &arg(args, 1)?.value;
    if let Value::Error(e) = lv {
        return Err(*e);
    }
    let hay = vector(&as_array(lv)).ok_or(CellError::Value)?;
    let mm = if has(args, 2) { int(args, 2)? } else { 0 };
    let sm = if has(args, 3) { int(args, 3)? } else { 1 };
    xsearch(&x, &hay, mm, sm)?.map(|p| Value::Number((p + 1) as f64)).ok_or(CellError::NA)
}

// ---------------------------------------------------------------------------------------------
// INDEX / ROWS / COLUMNS / TRANSPOSE / ADDRESS

fn index(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let av = &arg(args, 0)?.value;
    if let Value::Error(e) = av {
        return Err(*e);
    }
    let a = as_array(av);
    let r = if has(args, 1) { num(args, 1)? } else { 0.0 };
    let c = if has(args, 2) { num(args, 2)? } else { 0.0 };
    if has(args, 3) && num(args, 3)?.trunc() != 1.0 {
        return Err(CellError::Ref);
    }
    if r < 0.0 || c < 0.0 {
        return Err(CellError::Value);
    }
    let (mut r, mut c) = (r.trunc() as usize, c.trunc() as usize);
    // A single row with only one index: the index selects the column.
    if a.rows == 1 && args.len() == 2 && a.cols > 1 {
        c = r;
        r = 1;
    }
    if r > a.rows || c > a.cols {
        return Err(CellError::Ref);
    }
    match (r, c) {
        (0, 0) => Ok(Value::Array(std::sync::Arc::new(a.into_owned()))),
        (0, c) => {
            let v = column(&a, c - 1);
            if v.len() == 1 {
                return Ok(v.into_iter().next().unwrap_or(Value::Empty));
            }
            array_val(v.len(), 1, v)
        }
        (r, 0) => {
            let v = row(&a, r - 1);
            if v.len() == 1 {
                return Ok(v.into_iter().next().unwrap_or(Value::Empty));
            }
            array_val(1, v.len(), v)
        }
        (r, c) => a.get(r - 1, c - 1).cloned().ok_or(CellError::Ref),
    }
}

fn rows_fn(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    match &arg(args, 0)?.value {
        Value::Array(a) => Ok(Value::Number(a.rows as f64)),
        Value::Error(e) => Err(*e),
        _ => Ok(Value::Number(1.0)),
    }
}

fn columns_fn(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    match &arg(args, 0)?.value {
        Value::Array(a) => Ok(Value::Number(a.cols as f64)),
        Value::Error(e) => Err(*e),
        _ => Ok(Value::Number(1.0)),
    }
}

fn transpose(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    match &arg(args, 0)?.value {
        Value::Array(a) => Ok(Value::from(a.transpose())),
        v => Ok(v.clone()),
    }
}

/// Sheet text for ADDRESS, quoted when Excel quotes it. A `[Book]Sheet` prefix is judged by its
/// parts (Microsoft's ADDRESS examples: `"[Book1]Sheet1"` stays bare, `"EXCEL SHEET"` becomes
/// `'EXCEL SHEET'`). Unlike in a formula, `TRUE` and `FALSE` stay bare.
fn quote_sheet(s: &str) -> String {
    let bare = |p: &str| p.eq_ignore_ascii_case("TRUE") || p.eq_ignore_ascii_case("FALSE") || !gridcraft_core::sheet_needs_quotes(p);
    let word = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    let plain = match s.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((book, sheet)) => word(book) && bare(sheet),
        None => bare(s),
    };
    if plain { s.to_string() } else { format!("'{}'", s.replace('\'', "''")) }
}

fn address(args: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let r = num(args, 0)?.trunc();
    let c = num(args, 1)?.trunc();
    let abs = if has(args, 2) { int(args, 2)? } else { 1 };
    let a1 = if has(args, 3) { opt_bool(args, 3, true)? } else { true };
    if !(1.0..=1_048_576.0).contains(&r) || !(1.0..=16_384.0).contains(&c) || !(1..=4).contains(&abs) {
        return Err(CellError::Value);
    }
    let (ri, ci) = (r as u32, c as u32);
    let (row_abs, col_abs) = match abs {
        1 => (true, true),
        2 => (true, false),
        3 => (false, true),
        _ => (false, false),
    };
    let cell = if a1 {
        format!("{}{}{}{}", if col_abs { "$" } else { "" }, col_to_letters(ci - 1), if row_abs { "$" } else { "" }, ri)
    } else {
        let rp = if row_abs { format!("R{ri}") } else { format!("R[{ri}]") };
        let cp = if col_abs { format!("C{ci}") } else { format!("C[{ci}]") };
        format!("{rp}{cp}")
    };
    if has(args, 4) {
        let sheet = text(args, 4)?;
        if sheet.is_empty() {
            return text_val(format!("!{cell}"));
        }
        return text_val(format!("{}!{cell}", quote_sheet(&sheet)));
    }
    text_val(cell)
}

const XL: &[bool] = &[true, false, false, false, false, false];
const VL: &[bool] = &[true, false, true, true];
const ML: &[bool] = &[true, false, true, true];
const IDX: &[bool] = &[false, true, true, true];

pub(crate) fn specs() -> Vec<FnSpec> {
    use crate::util::{A, S};
    vec![
        f!(
            "VLOOKUP",
            3,
            4,
            Lookup,
            VL,
            "VLOOKUP(lookup_value, table_array, col_index_num, [range_lookup])",
            "Finds a value in the first column of a table and returns the value in the same row of another column.",
            vlookup
        ),
        f!(
            "HLOOKUP",
            3,
            4,
            Lookup,
            VL,
            "HLOOKUP(lookup_value, table_array, row_index_num, [range_lookup])",
            "Finds a value in the first row of a table and returns the value in the same column of another row.",
            hlookup
        ),
        f!(
            "LOOKUP",
            2,
            3,
            Lookup,
            &[true, false, false],
            "LOOKUP(lookup_value, lookup_vector, [result_vector])",
            "Approximate lookup in a sorted vector or the first row/column of an array.",
            lookup
        ),
        f!(
            "MATCH",
            2,
            3,
            Lookup,
            &[true, false, true],
            "MATCH(lookup_value, lookup_array, [match_type])",
            "Returns the 1-based position of a value in a row or column.",
            match_fn
        ),
        f!(
            "XLOOKUP",
            3,
            6,
            Lookup,
            XL,
            "XLOOKUP(lookup_value, lookup_array, return_array, [if_not_found], [match_mode], [search_mode])",
            "Searches a row or column and returns the matching item, row or column from another array.",
            xlookup
        ),
        f!(
            "XMATCH",
            2,
            4,
            Lookup,
            ML,
            "XMATCH(lookup_value, lookup_array, [match_mode], [search_mode])",
            "Returns the position of a value in a row or column with flexible match and search modes.",
            xmatch
        ),
        f!(
            "INDEX",
            2,
            4,
            Lookup,
            IDX,
            "INDEX(array, row_num, [column_num], [area_num])",
            "Returns the element, row or column of an array at the given position.",
            index
        ),
        f!("ROWS", 1, 1, Lookup, A, "ROWS(array)", "Counts the rows in an array or range.", rows_fn),
        f!("COLUMNS", 1, 1, Lookup, A, "COLUMNS(array)", "Counts the columns in an array or range.", columns_fn),
        f!("TRANSPOSE", 1, 1, Lookup, A, "TRANSPOSE(array)", "Swaps the rows and columns of an array.", transpose),
        f!(
            "ADDRESS",
            2,
            5,
            Lookup,
            S,
            "ADDRESS(row_num, column_num, [abs_num], [a1], [sheet_text])",
            "Builds a cell address as text from row and column numbers.",
            address
        ),
    ]
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellError, Value};

    use crate::util::testutil::*;

    fn table() -> Value {
        arr(vec![
            vec![nv(1.0), tv("one"), nv(10.0)],
            vec![nv(2.0), tv("two"), nv(20.0)],
            vec![nv(3.0), tv("three"), nv(30.0)],
            vec![nv(5.0), tv("five"), Value::Empty],
        ])
    }

    fn names() -> Value {
        arr(vec![vec![tv("Apple"), nv(1.0)], vec![tv("Banana"), nv(2.0)], vec![tv("Cherry"), nv(3.0)]])
    }

    #[test]
    fn vlookup() {
        is_text(ev("VLOOKUP", vec![n(2.0), rf(table()), n(2.0)]), "two");
        is_text(ev("VLOOKUP", vec![n(4.0), rf(table()), n(2.0)]), "three");
        is_text(ev("VLOOKUP", vec![n(4.0), rf(table()), n(2.0), b(true)]), "three");
        is_text(ev("VLOOKUP", vec![n(99.0), rf(table()), n(2.0)]), "five");
        is_err(ev("VLOOKUP", vec![n(0.5), rf(table()), n(2.0)]), CellError::NA);
        is_err(ev("VLOOKUP", vec![n(4.0), rf(table()), n(2.0), b(false)]), CellError::NA);
        is_err(ev("VLOOKUP", vec![n(4.0), rf(table()), n(2.0), empty()]), CellError::NA);
        close(ev("VLOOKUP", vec![n(3.0), rf(table()), n(3.0), b(false)]), 30.0);
        // An empty target stays blank (the cell displays 0 only as a final result) (#374).
        assert_eq!(ev("VLOOKUP", vec![n(5.0), rf(table()), n(3.0), b(false)]), Value::Empty);
        is_err(ev("VLOOKUP", vec![n(3.0), rf(table()), n(4.0)]), CellError::Ref);
        is_err(ev("VLOOKUP", vec![n(3.0), rf(table()), n(0.0)]), CellError::Value);
        close(ev("VLOOKUP", vec![t("b*"), rf(names()), n(2.0), b(false)]), 2.0);
        close(ev("VLOOKUP", vec![t("cherry"), rf(names()), n(2.0), b(false)]), 3.0);
        close(ev("VLOOKUP", vec![t("?pple"), rf(names()), n(2.0), b(false)]), 1.0);
        close(ev("VLOOKUP", vec![t("Bz"), rf(names()), n(2.0)]), 2.0);
        is_err(ev("VLOOKUP", vec![e(CellError::Div0), rf(names()), n(2.0)]), CellError::Div0);
        // Lifting over the lookup value.
        let r = ev("VLOOKUP", vec![av(col(&[1.0, 3.0])), rf(table()), n(3.0), b(false)]);
        assert_eq!(rows_of(&r), vec![vec![nv(10.0)], vec![nv(30.0)]]);
    }

    #[test]
    fn hlookup() {
        let t2 = arr(vec![vec![nv(1.0), nv(2.0), nv(3.0)], vec![tv("a"), tv("b"), tv("c")]]);
        is_text(ev("HLOOKUP", vec![n(2.0), rf(t2.clone()), n(2.0), b(false)]), "b");
        is_text(ev("HLOOKUP", vec![n(2.5), rf(t2.clone()), n(2.0)]), "b");
        is_err(ev("HLOOKUP", vec![n(2.0), rf(t2), n(3.0)]), CellError::Ref);
    }

    #[test]
    fn lookup_fn() {
        close(ev("LOOKUP", vec![n(4.0), av(row(&[1.0, 3.0, 5.0])), av(row(&[10.0, 30.0, 50.0]))]), 30.0);
        close(ev("LOOKUP", vec![n(5.0), av(row(&[1.0, 3.0, 5.0])), av(col(&[10.0, 30.0, 50.0]))]), 50.0);
        is_err(ev("LOOKUP", vec![n(0.0), av(row(&[1.0, 3.0, 5.0]))]), CellError::NA);
        close(ev("LOOKUP", vec![n(2.5), rf(table())]), 20.0);
        let wide = arr(vec![vec![nv(1.0), nv(2.0), nv(3.0)], vec![tv("a"), tv("b"), tv("c")]]);
        is_text(ev("LOOKUP", vec![n(2.0), av(wide)]), "b");
        // Text mixed in is skipped.
        let mixed = arr(vec![vec![nv(1.0), tv("x"), nv(3.0), tv("y")]]);
        close(ev("LOOKUP", vec![n(9.0), av(mixed.clone()), av(row(&[1.0, 2.0, 3.0, 4.0]))]), 3.0);
        close(ev("LOOKUP", vec![t("zz"), av(mixed), av(row(&[1.0, 2.0, 3.0, 4.0]))]), 4.0);
    }

    /// Approximate matches over unsorted, mixed-type data as observed in Excel
    /// (FormulaEvalTestData rows "Vlookup", "Hlookup", "Lookup"): Excel bisects the whole
    /// vector and stops at an equal entry, so answers differ from a "best entry" scan.
    #[test]
    fn approximate_match_follows_bisection() {
        // B6:H12 of the test data: header row, then one row per data kind.
        let data = arr(vec![
            vec![tv("INTEGRAL"), tv("DOUBLE"), tv("BLANK"), tv("STRING"), tv("REF"), tv("AREA (rows)"), Value::Empty],
            vec![nv(0.0), nv(0.0), Value::Empty, tv(""), nv(0.0), nv(1.0), nv(2.0)],
            vec![nv(1.0), nv(0.00001), Value::Empty, tv("nospaces"), nv(1.0), nv(1.001), nv(2.999)],
            vec![nv(2.0), nv(1.1), Value::Empty, tv("sp ace s"), nv(9999999999.0), nv(2.3456), nv(3.4567)],
            vec![nv(534.0), nv(1.00001), Value::Empty, tv("endsWithSpace "), nv(0.00001), tv("alphanum"), nv(3.0)],
            vec![nv(9999999999.0), nv(2.99999), Value::Empty, tv(" beginsWithSpace"), nv(0.0), tv("blanksIncl"), Value::Empty],
            vec![nv(-9999999999.0), nv(999999.99999), Value::Empty, tv("non-numeric"), tv("sp ace s"), tv("E8"), nv(545.0)],
        ]);
        close(ev("VLOOKUP", vec![n(534.0), rf(data.clone()), n(2.0)]), 1.00001);
        is_text(ev("VLOOKUP", vec![n(534.0), rf(data.clone()), n(4.0)]), "endsWithSpace ");
        is_text(ev("VLOOKUP", vec![n(534.0), rf(data.clone()), n(4.0), b(true)]), "endsWithSpace ");
        is_text(ev("HLOOKUP", vec![t("STRING"), rf(data.clone()), n(6.0), b(true)]), " beginsWithSpace");
        let keys = arr(vec![vec![tv("DOUBLE"), tv("BLANK"), tv("STRING"), tv("REF"), tv("AREA (rows)")]]);
        let results = arr(vec![vec![nv(1.1), Value::Empty, tv("sp ace s"), nv(9999999999.0), nv(2.3456)]]);
        close(ev("LOOKUP", vec![t("DOUBLE"), rf(keys), rf(results)]), 1.1);
        let b6_b15 = col(&[0.0, 1.0, 2.0, 534.0, 9999999999.0, -9999999999.0, -534.0, -2.0, -1.0]);
        let mut keys = vec![tv("INTEGRAL")];
        keys.extend(rows_of(&b6_b15).into_iter().flatten());
        let words = ["STRING", "", "nospaces", "sp ace s", "endsWithSpace ", " beginsWithSpace", "non-numeric", "0", "0.000001", "1"];
        let e_col = arr(words.iter().map(|w| vec![tv(w)]).collect());
        is_text(ev("LOOKUP", vec![n(534.0), rf(arr(keys.into_iter().map(|k| vec![k]).collect())), rf(e_col)]), "endsWithSpace ");
        // Sorted duplicates give the last one; descending data with MATCH -1 too.
        close(ev("MATCH", vec![n(2.0), av(row(&[1.0, 2.0, 2.0, 2.0, 3.0])), n(1.0)]), 4.0);
        close(ev("MATCH", vec![n(20.0), av(row(&[40.0, 20.0, 20.0, 10.0])), n(-1.0)]), 3.0);
        // Only text and blanks: no number to find.
        is_err(ev("MATCH", vec![n(5.0), av(arr(vec![vec![tv("a"), Value::Empty, tv("b")]]))]), CellError::NA);
        // A long run of trailing blanks (whole-column lookups) still finds the last number.
        let mut tall = vec![vec![nv(1.0)], vec![nv(2.0)], vec![nv(3.0)]];
        tall.extend((0..10_000).map(|_| vec![Value::Empty]));
        close(ev("MATCH", vec![n(9.0), rf(arr(tall))]), 3.0);
    }

    #[test]
    fn match_fn() {
        let v = col(&[10.0, 20.0, 30.0, 40.0]);
        close(ev("MATCH", vec![n(30.0), rf(v.clone()), n(0.0)]), 3.0);
        close(ev("MATCH", vec![n(35.0), rf(v.clone())]), 3.0);
        close(ev("MATCH", vec![n(35.0), rf(v.clone()), n(1.0)]), 3.0);
        close(ev("MATCH", vec![n(100.0), rf(v.clone())]), 4.0);
        is_err(ev("MATCH", vec![n(5.0), rf(v.clone())]), CellError::NA);
        is_err(ev("MATCH", vec![n(35.0), rf(v), n(0.0)]), CellError::NA);
        let d = col(&[40.0, 30.0, 20.0, 10.0]);
        close(ev("MATCH", vec![n(25.0), rf(d.clone()), n(-1.0)]), 2.0);
        close(ev("MATCH", vec![n(40.0), rf(d.clone()), n(-1.0)]), 1.0);
        is_err(ev("MATCH", vec![n(50.0), rf(d), n(-1.0)]), CellError::NA);
        let s = arr(vec![vec![tv("apple"), tv("banana"), tv("cherry")]]);
        close(ev("MATCH", vec![t("B*"), av(s.clone()), n(0.0)]), 2.0);
        close(ev("MATCH", vec![t("CHERRY"), av(s.clone()), n(0.0)]), 3.0);
        close(ev("MATCH", vec![t("c"), av(s.clone())]), 2.0);
        is_err(ev("MATCH", vec![n(1.0), rf(table())]), CellError::NA);
        close(ev("MATCH", vec![b(true), av(arr(vec![vec![nv(1.0), Value::Bool(true)]])), n(0.0)]), 2.0);
    }

    #[test]
    fn xlookup() {
        let keys = col(&[1.0, 2.0, 3.0, 5.0]);
        let vals = col(&[10.0, 20.0, 30.0, 50.0]);
        close(ev("XLOOKUP", vec![n(3.0), rf(keys.clone()), rf(vals.clone())]), 30.0);
        is_err(ev("XLOOKUP", vec![n(4.0), rf(keys.clone()), rf(vals.clone())]), CellError::NA);
        is_text(ev("XLOOKUP", vec![n(4.0), rf(keys.clone()), rf(vals.clone()), t("none")]), "none");
        close(ev("XLOOKUP", vec![n(4.0), rf(keys.clone()), rf(vals.clone()), empty(), n(-1.0)]), 30.0);
        close(ev("XLOOKUP", vec![n(4.0), rf(keys.clone()), rf(vals.clone()), empty(), n(1.0)]), 50.0);
        is_err(ev("XLOOKUP", vec![n(6.0), rf(keys.clone()), rf(vals.clone()), empty(), n(1.0)]), CellError::NA);
        close(ev("XLOOKUP", vec![n(4.0), rf(keys.clone()), rf(vals.clone()), empty(), n(-1.0), n(2.0)]), 30.0);
        close(ev("XLOOKUP", vec![n(4.0), rf(keys.clone()), rf(vals.clone()), empty(), n(1.0), n(2.0)]), 50.0);
        close(ev("XLOOKUP", vec![n(5.0), rf(keys.clone()), rf(vals.clone()), empty(), n(0.0), n(2.0)]), 50.0);
        let dup = col(&[1.0, 2.0, 1.0]);
        let dv = col(&[10.0, 20.0, 30.0]);
        close(ev("XLOOKUP", vec![n(1.0), rf(dup.clone()), rf(dv.clone())]), 10.0);
        close(ev("XLOOKUP", vec![n(1.0), rf(dup), rf(dv), empty(), n(0.0), n(-1.0)]), 30.0);
        // Descending binary search.
        let desc = col(&[50.0, 30.0, 20.0, 10.0]);
        close(ev("XLOOKUP", vec![n(25.0), rf(desc.clone()), rf(col(&[1.0, 2.0, 3.0, 4.0])), empty(), n(-1.0), n(-2.0)]), 3.0);
        close(ev("XLOOKUP", vec![n(25.0), rf(desc), rf(col(&[1.0, 2.0, 3.0, 4.0])), empty(), n(1.0), n(-2.0)]), 2.0);
        // Wildcards only in mode 2.
        let names_col = arr(vec![vec![tv("Apple")], vec![tv("Banana")]]);
        is_err(ev("XLOOKUP", vec![t("b*"), rf(names_col.clone()), rf(col(&[1.0, 2.0]))]), CellError::NA);
        close(ev("XLOOKUP", vec![t("b*"), rf(names_col), rf(col(&[1.0, 2.0])), empty(), n(2.0)]), 2.0);
        // Returning a whole row.
        let r = ev("XLOOKUP", vec![n(2.0), rf(col(&[1.0, 2.0, 3.0])), rf(table_rows3())]);
        assert_eq!(rows_of(&r), vec![vec![nv(2.0), tv("two"), nv(20.0)]]);
        // Horizontal lookup returning a column.
        let hk = row(&[1.0, 2.0]);
        let hv = arr(vec![vec![tv("a"), tv("b")], vec![tv("c"), tv("d")]]);
        assert_eq!(rows_of(&ev("XLOOKUP", vec![n(2.0), av(hk.clone()), av(hv)])), vec![vec![tv("b")], vec![tv("d")]]);
        is_err(ev("XLOOKUP", vec![n(2.0), av(hk), av(col(&[1.0, 2.0, 3.0]))]), CellError::Value);
        is_err(ev("XLOOKUP", vec![n(2.0), rf(keys.clone()), rf(vals.clone()), empty(), n(3.0)]), CellError::Value);
        is_err(ev("XLOOKUP", vec![n(2.0), rf(table()), rf(vals)]), CellError::Value);
    }

    /// XLOOKUP binary search (search mode ±2). Observed in Excel (xlookup.xlsx, Sheet2): next
    /// larger over ascending data with text mixed in (0.22) and over descending data (0.24); the
    /// bisection compares across types in sort order (numbers < text) instead of skipping text.
    /// The other modes follow from the same order.
    #[test]
    fn xlookup_binary_search_observed() {
        let rates = col(&[0.1, 0.22, 0.24, 0.32, 0.35, 0.37]);
        let incomes = arr(vec![vec![nv(9700.0)], vec![tv("xyz")], vec![tv("xyz")], vec![nv(160726.0)], vec![nv(204100.0)], vec![nv(510300.0)]]);
        close(ev("XLOOKUP", vec![n(46523.0), rf(incomes.clone()), rf(rates.clone()), n(0.0), n(1.0), n(2.0)]), 0.22);
        close(ev("XLOOKUP", vec![n(46523.0), rf(incomes), rf(rates), n(0.0), n(-1.0), n(2.0)]), 0.1);
        let desc_incomes = col(&[510300.0, 204100.0, 160726.0, 84200.0, 39475.0, 9700.0]);
        let desc_rates = col(&[0.37, 0.35, 0.32, 0.24, 0.22, 0.1]);
        close(ev("XLOOKUP", vec![n(46523.0), rf(desc_incomes.clone()), rf(desc_rates.clone()), n(0.0), n(1.0), n(-2.0)]), 0.24);
        close(ev("XLOOKUP", vec![n(46523.0), rf(desc_incomes.clone()), rf(desc_rates.clone()), n(0.0), n(-1.0), n(-2.0)]), 0.22);
        close(ev("XLOOKUP", vec![n(84200.0), rf(desc_incomes), rf(desc_rates), n(0.0), n(0.0), n(-2.0)]), 0.24);
    }

    fn table_rows3() -> Value {
        arr(vec![vec![nv(1.0), tv("one"), nv(10.0)], vec![nv(2.0), tv("two"), nv(20.0)], vec![nv(3.0), tv("three"), nv(30.0)]])
    }

    #[test]
    fn xmatch() {
        let v = col(&[10.0, 20.0, 30.0]);
        close(ev("XMATCH", vec![n(20.0), rf(v.clone())]), 2.0);
        is_err(ev("XMATCH", vec![n(25.0), rf(v.clone())]), CellError::NA);
        close(ev("XMATCH", vec![n(25.0), rf(v.clone()), n(1.0)]), 3.0);
        close(ev("XMATCH", vec![n(25.0), rf(v.clone()), n(-1.0)]), 2.0);
        close(ev("XMATCH", vec![n(30.0), rf(v.clone()), n(0.0), n(2.0)]), 3.0);
        close(ev("XMATCH", vec![t("a?c"), av(arr(vec![vec![tv("xyz"), tv("abc")]])), n(2.0)]), 2.0);
        let r = ev("XMATCH", vec![av(row(&[30.0, 10.0])), rf(v)]);
        assert_eq!(rows_of(&r), vec![vec![nv(3.0), nv(1.0)]]);
    }

    /// Lookups compare numbers exactly, unlike `=` (15 significant digits). Observed in Excel 365.
    #[test]
    fn lookups_compare_numbers_exactly() {
        let e = 0.1 + 0.2;
        let one = || rf(col(&[e]));
        // Exact modes miss a key one bit away.
        is_err(ev("MATCH", vec![n(0.3), one(), n(0.0)]), CellError::NA);
        is_err(ev("VLOOKUP", vec![n(0.3), one(), n(1.0), b(false)]), CellError::NA);
        is_err(ev("XMATCH", vec![n(0.3), one()]), CellError::NA);
        close(ev("MATCH", vec![n(0.3), rf(col(&[e, 0.3])), n(0.0)]), 2.0);
        close(ev("MATCH", vec![n(e), rf(col(&[e, 0.3])), n(0.0)]), 1.0);
        is_err(ev("MATCH", vec![n(100.0), rf(col(&[100.0 + 1e-14])), n(0.0)]), CellError::NA);
        // 0 and -0 are still the same key.
        close(ev("MATCH", vec![n(-0.0), rf(col(&[0.0])), n(0.0)]), 1.0);
        // Approximate modes: 0.3 sorts below 0.30000000000000004.
        is_err(ev("MATCH", vec![n(0.3), one(), n(1.0)]), CellError::NA);
        close(ev("MATCH", vec![n(0.3), one(), n(-1.0)]), 1.0);
        is_err(ev("LOOKUP", vec![n(0.3), one()]), CellError::NA);
        is_err(ev("XMATCH", vec![n(0.3), one(), n(-1.0)]), CellError::NA);
        close(ev("XMATCH", vec![n(0.3), one(), n(1.0)]), 1.0);
        // The bisection stops on 0.3 and doesn't run on into the next entry.
        let list = || rf(col(&[0.1, 0.2, 0.3, e, 0.4]));
        close(ev("MATCH", vec![n(0.3), list(), n(1.0)]), 3.0);
        close(ev("MATCH", vec![n(e), list(), n(1.0)]), 4.0);
        close(ev("XMATCH", vec![n(0.3), list(), n(0.0), n(2.0)]), 3.0);
        close(ev("XMATCH", vec![n(e), list(), n(0.0), n(2.0)]), 4.0);
        let asc = || rf(col(&[0.1, e, 0.5]));
        let desc = || rf(col(&[0.5, e, 0.1]));
        close(ev("MATCH", vec![n(0.3), asc(), n(1.0)]), 1.0);
        close(ev("MATCH", vec![n(0.3), desc(), n(-1.0)]), 2.0);
        is_err(ev("XMATCH", vec![n(0.3), asc(), n(0.0), n(2.0)]), CellError::NA);
        close(ev("XMATCH", vec![n(0.3), asc(), n(-1.0), n(2.0)]), 1.0);
        close(ev("XMATCH", vec![n(0.3), asc(), n(1.0), n(2.0)]), 2.0);
        close(ev("XMATCH", vec![n(0.3), desc(), n(-1.0), n(-2.0)]), 3.0);
        close(ev("XMATCH", vec![n(0.3), asc(), n(-1.0)]), 1.0);
        close(ev("XMATCH", vec![n(0.3), asc(), n(1.0)]), 2.0);
    }

    #[test]
    fn index() {
        let t3 = table_rows3();
        is_text(ev("INDEX", vec![rf(t3.clone()), n(2.0), n(2.0)]), "two");
        close(ev("INDEX", vec![rf(t3.clone()), n(3.0), n(3.0)]), 30.0);
        assert_eq!(rows_of(&ev("INDEX", vec![rf(t3.clone()), n(0.0), n(3.0)])), vec![vec![nv(10.0)], vec![nv(20.0)], vec![nv(30.0)]]);
        assert_eq!(rows_of(&ev("INDEX", vec![rf(t3.clone()), n(1.0), n(0.0)])), vec![vec![nv(1.0), tv("one"), nv(10.0)]]);
        assert_eq!(rows_of(&ev("INDEX", vec![rf(t3.clone()), n(1.0)])), vec![vec![nv(1.0), tv("one"), nv(10.0)]]);
        is_err(ev("INDEX", vec![rf(t3.clone()), n(4.0), n(1.0)]), CellError::Ref);
        is_err(ev("INDEX", vec![rf(t3.clone()), n(1.0), n(4.0)]), CellError::Ref);
        is_err(ev("INDEX", vec![rf(t3), n(-1.0), n(1.0)]), CellError::Value);
        close(ev("INDEX", vec![av(row(&[5.0, 6.0, 7.0])), n(2.0)]), 6.0);
        close(ev("INDEX", vec![av(col(&[5.0, 6.0, 7.0])), n(3.0)]), 7.0);
        close(ev("INDEX", vec![n(9.0), n(1.0), n(1.0)]), 9.0);
        let r = ev("INDEX", vec![av(col(&[5.0, 6.0, 7.0])), av(row(&[3.0, 1.0]))]);
        assert_eq!(rows_of(&r), vec![vec![nv(7.0), nv(5.0)]]);
    }

    #[test]
    fn shape_and_address() {
        close(ev("ROWS", vec![rf(table())]), 4.0);
        close(ev("COLUMNS", vec![rf(table())]), 3.0);
        close(ev("ROWS", vec![n(1.0)]), 1.0);
        let tr = ev("TRANSPOSE", vec![av(row(&[1.0, 2.0, 3.0]))]);
        assert_eq!(rows_of(&tr), vec![vec![nv(1.0)], vec![nv(2.0)], vec![nv(3.0)]]);
        is_text(ev("ADDRESS", vec![n(1.0), n(1.0)]), "$A$1");
        is_text(ev("ADDRESS", vec![n(2.0), n(3.0), n(2.0)]), "C$2");
        is_text(ev("ADDRESS", vec![n(2.0), n(3.0), n(3.0)]), "$C2");
        is_text(ev("ADDRESS", vec![n(2.0), n(28.0), n(4.0)]), "AB2");
        is_text(ev("ADDRESS", vec![n(2.0), n(3.0), n(1.0), b(false)]), "R2C3");
        is_text(ev("ADDRESS", vec![n(2.0), n(3.0), n(4.0), b(false)]), "R[2]C[3]");
        is_text(ev("ADDRESS", vec![n(1.0), n(1.0), n(1.0), b(true), t("Sheet1")]), "Sheet1!$A$1");
        is_text(ev("ADDRESS", vec![n(1.0), n(1.0), n(1.0), b(true), t("My Sheet")]), "'My Sheet'!$A$1");
        // Observed in Excel (FormulaEvalTestData) and Microsoft's ADDRESS examples; the last
        // case follows from the quoting rule for sheet names.
        let cases: [(f64, f64, f64, bool, &str, &str); 4] = [
            (2.0, 3.0, 3.0, true, "[Book1]Sheet1", "[Book1]Sheet1!$C2"),
            (2.0, 3.0, 1.0, false, "[Book1]Sheet1", "[Book1]Sheet1!R2C3"),
            (2.0, 3.0, 1.0, false, "EXCEL SHEET", "'EXCEL SHEET'!R2C3"),
            (1.0, 1.0, 1.0, true, "[My Book]Sheet1", "'[My Book]Sheet1'!$A$1"),
        ];
        for (r, c, abs, a1, sheet, want) in cases {
            is_text(ev("ADDRESS", vec![n(r), n(c), n(abs), b(a1), t(sheet)]), want);
        }
        // Sheet names that read as a cell or an R1C1 reference, or start with a dot, are quoted;
        // off-sheet cells and booleans are not (#1458).
        for (sheet, want) in
            [("A1", "'A1'!$A$1"), ("R1C1", "'R1C1'!$A$1"), ("RC", "'RC'!$A$1"), (".x", "'.x'!$A$1"), ("XFE1", "XFE1!$A$1"), ("TRUE", "TRUE!$A$1")]
        {
            is_text(ev("ADDRESS", vec![n(1.0), n(1.0), n(1.0), b(true), t(sheet)]), want);
        }
        is_text(ev("ADDRESS", vec![n(1048576.0), n(16384.0)]), "$XFD$1048576");
        is_err(ev("ADDRESS", vec![n(0.0), n(1.0)]), CellError::Value);
        is_err(ev("ADDRESS", vec![n(1.0), n(16385.0)]), CellError::Value);
        is_err(ev("ADDRESS", vec![n(1.0), n(1.0), n(5.0)]), CellError::Value);
    }
}
