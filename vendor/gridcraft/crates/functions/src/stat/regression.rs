use super::descriptive::{arg_pairs, intercept, moments, slope};
use super::{Arg, C, CellError, R, Value, arg, array_val, as_array, boolean, has, num, num_val};

pub(super) fn forecast(a: &[Arg], _: C) -> R<Value> {
    let x = num(a, 0)?;
    let (ys, xs) = arg_pairs(a, 1, 2)?;
    if ys.is_empty() {
        return Err(CellError::Div0);
    }
    num_val(intercept(&ys, &xs)? + slope(&ys, &xs)? * x)
}

pub(super) fn steyx(a: &[Arg], _: C) -> R<Value> {
    let (ys, xs) = arg_pairs(a, 0, 1)?;
    let n = ys.len() as f64;
    if ys.len() < 3 {
        return Err(CellError::Div0);
    }
    let (sxy, sxx, syy, _, _) = moments(&xs, &ys)?;
    if sxx == 0.0 {
        return Err(CellError::Div0);
    }
    num_val(((syy - sxy * sxy / sxx) / (n - 2.0)).max(0.0).sqrt())
}

pub(super) fn covariance(a: &[Arg], sample: bool) -> R<Value> {
    let (xs, ys) = arg_pairs(a, 0, 1)?;
    let n = xs.len();
    if n == 0 || (sample && n < 2) {
        return Err(CellError::Div0);
    }
    let (sxy, ..) = moments(&xs, &ys)?;
    num_val(sxy / if sample { (n - 1) as f64 } else { n as f64 })
}

// ---------------------------------------------------------------------------------------------
// Regression (LINEST, LOGEST, TREND, GROWTH)
// ---------------------------------------------------------------------------------------------

struct Xy {
    y: Vec<f64>,
    /// n rows of k predictor values.
    x: Vec<Vec<f64>>,
    k: usize,
    y_is_row: bool,
    y_rows: usize,
    y_cols: usize,
}

fn strict_num(v: &Value) -> R<f64> {
    match v {
        Value::Number(n) => Ok(*n),
        Value::Error(e) => Err(*e),
        _ => Err(CellError::Value),
    }
}

/// Most predictor columns LINEST, LOGEST, TREND and GROWTH accept. The fit works on a k×k
/// triangle in O(k³), so an unbounded k (one `SEQUENCE(1,16384)` argument) allocated
/// gigabytes and could abort the process.
pub(super) const MAX_REGRESSORS: usize = 255;

/// Most multiply-adds (rows × k²) a regression may spend on its factorisation.
const MAX_REGRESSION_WORK: usize = 1_000_000_000;

/// Rejects regressions too large to fit: more than [`MAX_REGRESSORS`] predictors is `#VALUE!`,
/// more than [`MAX_REGRESSION_WORK`] multiply-adds is `#NUM!`.
pub(super) fn check_regression_size(n: usize, k: usize) -> R<()> {
    if k > MAX_REGRESSORS {
        return Err(CellError::Value);
    }
    match n.checked_mul(k).and_then(|nk| nk.checked_mul(k)) {
        Some(work) if work <= MAX_REGRESSION_WORK => Ok(()),
        _ => Err(CellError::Num),
    }
}

fn parse_xy(y: &Value, x: Option<&Value>) -> R<Xy> {
    let ya = as_array(y);
    let n = ya.data.len();
    let y_is_row = ya.rows == 1 && ya.cols > 1;
    let yv: Vec<f64> = ya.data.iter().map(strict_num).collect::<R<_>>()?;
    let (xs, k) = match x {
        None => ((1..=n).map(|i| vec![i as f64]).collect(), 1),
        Some(xv) => {
            let xa = as_array(xv);
            if xa.rows == ya.rows && xa.cols == ya.cols {
                (xa.data.iter().map(|v| strict_num(v).map(|f| vec![f])).collect::<R<Vec<_>>>()?, 1)
            } else if !y_is_row && xa.rows == n {
                check_regression_size(n, xa.cols)?;
                let mut rows = Vec::with_capacity(n);
                for r in 0..n {
                    rows.push((0..xa.cols).map(|c| strict_num(xa.get(r, c).unwrap_or(&Value::Empty))).collect::<R<Vec<_>>>()?);
                }
                (rows, xa.cols)
            } else if y_is_row && xa.cols == n {
                check_regression_size(n, xa.rows)?;
                let mut rows = Vec::with_capacity(n);
                for c in 0..n {
                    rows.push((0..xa.rows).map(|r| strict_num(xa.get(r, c).unwrap_or(&Value::Empty))).collect::<R<Vec<_>>>()?);
                }
                (rows, xa.rows)
            } else {
                return Err(CellError::Ref);
            }
        }
    };
    Ok(Xy { y: yv, x: xs, k, y_is_row, y_rows: ya.rows, y_cols: ya.cols })
}

struct Fit {
    coef: Vec<f64>,
    b: f64,
    se: Vec<f64>,
    se_b: Option<f64>,
    r2: R<f64>,
    sey: R<f64>,
    f: R<f64>,
    df: f64,
    ssreg: f64,
    ssresid: f64,
}

/// Euclidean norm without intermediate overflow or underflow.
fn norm2(v: &[f64]) -> f64 {
    let m = v.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    if m == 0.0 || !m.is_finite() {
        return m;
    }
    m * v.iter().map(|x| (x / m) * (x / m)).sum::<f64>().sqrt()
}

/// Result of [`qr_solve`]: the least-squares solution on the kept columns, in the caller's
/// column scale, and what the standard errors need.
struct QrFit {
    coef: Vec<f64>,
    dropped: Vec<bool>,
    /// Residual sum of squares of the fit.
    ssresid: f64,
    /// Diagonal of `(X'X)^-1` over the kept columns (0 for dropped ones).
    diag: Vec<f64>,
    /// `xm' (X'X)^-1 xm` over the kept columns.
    quad: f64,
}

/// Least squares by Householder QR on unit-norm columns, instead of the normal equations, whose
/// condition number is the square of the data's. Columns are taken left to right; one whose part
/// outside the span of the kept columns is under 1e-6 of its own length (the 1e-12 relative
/// pivot of the earlier normal-equation solver, on squares) is collinear: it is dropped, with
/// coefficient and standard error 0, as Excel does.
///
/// `constant[j]` says column `j` was constant before centring. Centring it leaves rounding
/// residue, not 0 (`[0.1; 3]` centres to about -1.4e-17): it is collinear with the intercept, so
/// it is dropped instead of being scaled up to unit length. Whether a column is constant is
/// decided on its raw values, not by comparing the centred length with the offset: a predictor
/// like `[1e13, 1e13 + 1, 1e13 + 2]` is far smaller than its offset and still varies.
fn qr_solve(mut cols: Vec<Vec<f64>>, mut y: Vec<f64>, xm: &[f64], constant: &[bool]) -> QrFit {
    let n = y.len();
    let k = cols.len();
    let mut scale = vec![1.0; k];
    let mut dropped = vec![false; k];
    // Householder vectors (unit norm, acting on rows `at..n`) and the columns of R.
    let mut refl: Vec<(usize, Vec<f64>)> = Vec::new();
    let mut r_cols: Vec<Vec<f64>> = Vec::new();
    let mut kept: Vec<usize> = Vec::new();
    for (j, col) in cols.iter_mut().enumerate() {
        let len = norm2(col);
        if constant.get(j).copied().unwrap_or(false) || !(len > 1e-300 && len.is_finite()) {
            dropped[j] = true;
            continue;
        }
        col.iter_mut().for_each(|v| *v /= len);
        scale[j] = len;
        for (at, v) in &refl {
            apply_reflector(*at, v, col);
        }
        let at = refl.len();
        let tail = col.get(at..).unwrap_or(&[]);
        let alpha = norm2(tail);
        if alpha * alpha <= 1e-12 {
            dropped[j] = true;
            continue;
        }
        let sign = if tail.first().is_some_and(|t| *t < 0.0) { -1.0 } else { 1.0 };
        let mut v: Vec<f64> = tail.to_vec();
        if let Some(h) = v.first_mut() {
            *h += sign * alpha;
        }
        let vn = norm2(&v);
        v.iter_mut().for_each(|x| *x /= vn);
        let mut rc: Vec<f64> = col.get(..at).unwrap_or(&[]).to_vec();
        rc.push(-sign * alpha);
        r_cols.push(rc);
        refl.push((at, v));
        kept.push(j);
    }
    for (at, v) in &refl {
        apply_reflector(*at, v, &mut y);
    }
    let r = kept.len();
    // Back substitution R c = (Q'y)[..r], then undo the column scaling.
    let mut c = vec![0.0; r];
    for i in (0..r).rev() {
        let mut t = y.get(i).copied().unwrap_or(0.0);
        for jj in i + 1..r {
            t -= r_cols.get(jj).and_then(|rc| rc.get(i)).copied().unwrap_or(0.0) * c.get(jj).copied().unwrap_or(0.0);
        }
        if let Some(ci) = c.get_mut(i) {
            *ci = t / r_cols.get(i).and_then(|rc| rc.get(i)).copied().unwrap_or(f64::NAN);
        }
    }
    let ssresid = y.get(r..n).unwrap_or(&[]).iter().map(|v| v * v).sum::<f64>();
    // R^-1 (upper triangular), row i of the inverse at rinv[i].
    let rij = |i: usize, j: usize| r_cols.get(j).and_then(|rc| rc.get(i)).copied().unwrap_or(0.0);
    let mut rinv = vec![vec![0.0; r]; r];
    for t in 0..r {
        for i in (0..=t).rev() {
            let mut v = if i == t { 1.0 } else { 0.0 };
            for jj in i + 1..=t {
                v -= rij(i, jj) * rinv.get(jj).and_then(|row| row.get(t)).copied().unwrap_or(0.0);
            }
            if let Some(cell) = rinv.get_mut(i).and_then(|row| row.get_mut(t)) {
                *cell = v / rij(i, i);
            }
        }
    }
    let mut coef = vec![0.0; k];
    let mut diag = vec![0.0; k];
    for (i, &j) in kept.iter().enumerate() {
        let s = scale.get(j).copied().unwrap_or(1.0);
        if let Some(v) = coef.get_mut(j) {
            *v = c.get(i).copied().unwrap_or(0.0) / s;
        }
        let row = rinv.get(i);
        if let Some(v) = diag.get_mut(j) {
            *v = row.map_or(0.0, |rw| rw.iter().map(|x| x * x).sum::<f64>()) / (s * s);
        }
    }
    // xm' (S R'R S)^-1 xm = |R^-T S^-1 xm|².
    let w: Vec<f64> = kept.iter().map(|&j| xm.get(j).copied().unwrap_or(0.0) / scale.get(j).copied().unwrap_or(1.0)).collect();
    let quad = (0..r)
        .map(|t| {
            (0..=t).map(|i| rinv.get(i).and_then(|row| row.get(t)).copied().unwrap_or(0.0) * w.get(i).copied().unwrap_or(0.0)).sum::<f64>().powi(2)
        })
        .sum::<f64>();
    QrFit { coef, dropped, ssresid, diag, quad }
}

/// Applies the Householder reflection `I - 2vv'` on rows `at..` of `x`.
fn apply_reflector(at: usize, v: &[f64], x: &mut [f64]) {
    let Some(tail) = x.get_mut(at..) else { return };
    let dot: f64 = v.iter().zip(tail.iter()).map(|(a, b)| a * b).sum();
    tail.iter_mut().zip(v).for_each(|(t, a)| *t -= 2.0 * dot * a);
}

fn regress(y: &[f64], x: &[Vec<f64>], k: usize, constant: bool) -> R<Fit> {
    let n = y.len();
    if n == 0 || x.len() != n {
        return Err(CellError::Value);
    }
    check_regression_size(n, k)?;
    let nf = n as f64;
    let (xm, ym) = if constant {
        let xm: Vec<f64> = (0..k).map(|j| x.iter().map(|r| r.get(j).copied().unwrap_or(0.0)).sum::<f64>() / nf).collect();
        (xm, y.iter().sum::<f64>() / nf)
    } else {
        (vec![0.0; k], 0.0)
    };
    // (Centred) predictor columns and response.
    let cols: Vec<Vec<f64>> =
        (0..k).map(|j| x.iter().map(|r| r.get(j).copied().unwrap_or(0.0) - xm.get(j).copied().unwrap_or(0.0)).collect()).collect();
    let yc: Vec<f64> = y.iter().map(|v| v - ym).collect();
    let sstot: f64 = if constant { yc.iter().map(|v| v * v).sum() } else { y.iter().map(|v| v * v).sum() };
    // Only centring leaves rounding residue behind; without an intercept a constant column is data.
    // Constant means every raw value is the same double: distinct values, however close, vary.
    let constant_cols: Vec<bool> = (0..k)
        .map(|j| {
            let mut col = x.iter().map(|r| r.get(j).copied().unwrap_or(0.0));
            let first = col.next();
            constant && first.is_some_and(|f| col.all(|v| v == f))
        })
        .collect();
    let QrFit { coef, dropped, ssresid, diag, quad } = qr_solve(cols, yc, &xm, &constant_cols);
    let b = if constant { ym - coef.iter().zip(&xm).map(|(c, m)| c * m).sum::<f64>() } else { 0.0 };
    let used = dropped.iter().filter(|d| !**d).count() + usize::from(constant);
    let df = nf - used as f64;
    let ssreg = sstot - ssresid;
    let r2 = if sstot == 0.0 { Err(CellError::Num) } else { Ok(ssreg / sstot) };
    let sey2 = if df > 0.0 { Ok(ssresid / df) } else { Err(CellError::Num) };
    let sey = sey2.map(f64::sqrt);
    let v1 = if constant { nf - df - 1.0 } else { nf - df };
    let f = match sey2 {
        Ok(s2) if s2 > 0.0 && v1 > 0.0 => Ok((ssreg / v1) / s2),
        _ => Err(CellError::Num),
    };
    let s2 = sey2.unwrap_or(f64::NAN);
    let se: Vec<f64> =
        (0..k).map(|i| if dropped.get(i).copied().unwrap_or(true) { 0.0 } else { (diag.get(i).copied().unwrap_or(0.0) * s2).sqrt() }).collect();
    let se_b = if constant { Some((s2 * (1.0 / nf + quad)).sqrt()) } else { None };
    Ok(Fit { coef, b, se, se_b, r2, sey, f, df, ssreg, ssresid })
}

fn const_arg(a: &[Arg], i: usize) -> R<bool> {
    if has(a, i) { boolean(a, i) } else { Ok(true) }
}

fn opt_value(a: &[Arg], i: usize) -> Option<&Value> {
    if has(a, i) { a.get(i).map(|x| &x.value) } else { None }
}

pub(super) fn linest_impl(a: &[Arg], log: bool) -> R<Value> {
    let mut xy = parse_xy(&arg(a, 0)?.value, opt_value(a, 1))?;
    if log {
        if xy.y.iter().any(|&v| v <= 0.0) {
            return Err(CellError::Num);
        }
        xy.y.iter_mut().for_each(|v| *v = v.ln());
    }
    let constant = const_arg(a, 2)?;
    let stats = if has(a, 3) { boolean(a, 3)? } else { false };
    let fit = regress(&xy.y, &xy.x, xy.k, constant)?;
    let tr = |v: f64| if log { v.exp() } else { v };
    let cols = xy.k + 1;
    let mut data: Vec<Value> = Vec::new();
    for c in fit.coef.iter().rev() {
        data.push(Value::number(tr(*c)));
    }
    data.push(Value::number(tr(fit.b)));
    if stats {
        let rv = |r: R<f64>| r.map(Value::number).unwrap_or_else(Value::Error);
        let na = Value::Error(CellError::NA);
        for s in fit.se.iter().rev() {
            data.push(Value::number(*s));
        }
        data.push(fit.se_b.map(Value::number).unwrap_or(na.clone()));
        let rows: [[Value; 2]; 3] =
            [[rv(fit.r2), rv(fit.sey)], [rv(fit.f), Value::number(fit.df)], [Value::number(fit.ssreg), Value::number(fit.ssresid)]];
        for [p, q] in rows {
            data.push(p);
            if cols > 1 {
                data.push(q);
            }
            for _ in 2..cols {
                data.push(na.clone());
            }
        }
        return array_val(5, cols, data);
    }
    array_val(1, cols, data)
}

pub(super) fn trend_impl(a: &[Arg], log: bool) -> R<Value> {
    let mut xy = parse_xy(&arg(a, 0)?.value, opt_value(a, 1))?;
    if log {
        if xy.y.iter().any(|&v| v <= 0.0) {
            return Err(CellError::Num);
        }
        xy.y.iter_mut().for_each(|v| *v = v.ln());
    }
    let constant = const_arg(a, 3)?;
    let fit = regress(&xy.y, &xy.x, xy.k, constant)?;
    let predict = |row: &[f64]| -> Value {
        let v = fit.b + fit.coef.iter().zip(row).map(|(c, x)| c * x).sum::<f64>();
        Value::number(if log { v.exp() } else { v })
    };
    match opt_value(a, 2) {
        None => {
            let data: Vec<Value> = xy.x.iter().map(|r| predict(r)).collect();
            array_val(xy.y_rows, xy.y_cols, data)
        }
        Some(nv) => {
            let na = as_array(nv);
            if xy.k == 1 {
                let data = na.data.iter().map(|v| strict_num(v).map(|x| predict(&[x]))).collect::<R<Vec<_>>>()?;
                array_val(na.rows, na.cols, data)
            } else if !xy.y_is_row && na.cols == xy.k {
                let mut data = Vec::with_capacity(na.rows);
                for r in 0..na.rows {
                    let row = (0..na.cols).map(|c| strict_num(na.get(r, c).unwrap_or(&Value::Empty))).collect::<R<Vec<_>>>()?;
                    data.push(predict(&row));
                }
                array_val(na.rows, 1, data)
            } else if xy.y_is_row && na.rows == xy.k {
                let mut data = Vec::with_capacity(na.cols);
                for c in 0..na.cols {
                    let row = (0..na.rows).map(|r| strict_num(na.get(r, c).unwrap_or(&Value::Empty))).collect::<R<Vec<_>>>()?;
                    data.push(predict(&row));
                }
                array_val(1, na.cols, data)
            } else {
                Err(CellError::Ref)
            }
        }
    }
}
