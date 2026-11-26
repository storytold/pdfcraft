use super::{Arg, CellError, Ctx, MAX_CELLS, R, Value, array_val, as_array, check_size, num, num_val, to_sig_digits};

// ---------------------------------------------------------------------------------------------
// Matrices

/// A numeric matrix (every element must be a number, else `#VALUE!`).
fn matrix(v: &Value) -> R<(usize, usize, Vec<f64>)> {
    let arr = as_array(v);
    let mut out = Vec::with_capacity(arr.data.len());
    for x in arr.iter() {
        match x {
            Value::Number(n) => out.push(*n),
            Value::Error(e) => return Err(*e),
            _ => return Err(CellError::Value),
        }
    }
    Ok((arr.rows, arr.cols, out))
}

fn to_value_array(rows: usize, cols: usize, data: Vec<f64>) -> R<Value> {
    let vals = data.into_iter().map(|x| if x.is_finite() { Value::Number(x) } else { Value::Error(CellError::Num) }).collect();
    array_val(rows, cols, vals)
}

pub(super) fn mmult(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (ar, ac, ad) = matrix(&crate::util::arg(a, 0)?.value)?;
    let (br, bc, bd) = matrix(&crate::util::arg(a, 1)?.value)?;
    if ac != br {
        return Err(CellError::Value);
    }
    if ar.saturating_mul(bc) > MAX_CELLS || ar.saturating_mul(bc).saturating_mul(ac) > 2_000_000_000 {
        return Err(CellError::Num);
    }
    let mut out = vec![0.0; ar * bc];
    for i in 0..ar {
        for k in 0..ac {
            let x = ad.get(i * ac + k).copied().unwrap_or(0.0);
            if x == 0.0 {
                continue;
            }
            for j in 0..bc {
                if let (Some(o), Some(y)) = (out.get_mut(i * bc + j), bd.get(k * bc + j)) {
                    *o += x * y;
                }
            }
        }
    }
    to_value_array(ar, bc, out)
}

/// LU decomposition with partial pivoting; returns (lu, perm, sign) or None when singular.
fn square(a: &[Arg]) -> R<(usize, Vec<f64>)> {
    let (r, c, d) = matrix(&crate::util::arg(a, 0)?.value)?;
    if r != c {
        return Err(CellError::Value);
    }
    if r > 2000 {
        return Err(CellError::Num);
    }
    Ok((r, d))
}

fn determinant(n: usize, mut m: Vec<f64>) -> f64 {
    let mut det = 1.0;
    for col in 0..n {
        let mut piv = col;
        let mut best = 0.0;
        for row in col..n {
            let v = m.get(row * n + col).copied().unwrap_or(0.0).abs();
            if v > best {
                best = v;
                piv = row;
            }
        }
        if best == 0.0 {
            return 0.0;
        }
        if piv != col {
            for k in 0..n {
                m.swap(piv * n + k, col * n + k);
            }
            det = -det;
        }
        let p = m.get(col * n + col).copied().unwrap_or(0.0);
        det *= p;
        for row in col + 1..n {
            let f = m.get(row * n + col).copied().unwrap_or(0.0) / p;
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                let v = m.get(col * n + k).copied().unwrap_or(0.0);
                if let Some(x) = m.get_mut(row * n + k) {
                    *x -= f * v;
                }
            }
        }
    }
    det
}

pub(super) fn mdeterm(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (n, m) = square(a)?;
    let d = determinant(n, m);
    // Tidy results that should be integers (e.g. 0.9999999999999998 → 1).
    num_val(if d.abs() > 1e-300 { to_sig_digits(d, 15) } else { d })
}

pub(super) fn minverse(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let (n, mut m) = square(a)?;
    let mut inv = vec![0.0; n * n];
    for i in 0..n {
        if let Some(x) = inv.get_mut(i * n + i) {
            *x = 1.0;
        }
    }
    let scale = m.iter().fold(0.0f64, |acc, x| acc.max(x.abs())).max(1e-300);
    for col in 0..n {
        let mut piv = col;
        let mut best = 0.0;
        for row in col..n {
            let v = m.get(row * n + col).copied().unwrap_or(0.0).abs();
            if v > best {
                best = v;
                piv = row;
            }
        }
        if best <= scale * 1e-15 {
            return Err(CellError::Num);
        }
        if piv != col {
            for k in 0..n {
                m.swap(piv * n + k, col * n + k);
                inv.swap(piv * n + k, col * n + k);
            }
        }
        let p = m.get(col * n + col).copied().unwrap_or(1.0);
        for k in 0..n {
            if let Some(x) = m.get_mut(col * n + k) {
                *x /= p;
            }
            if let Some(x) = inv.get_mut(col * n + k) {
                *x /= p;
            }
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let f = m.get(row * n + col).copied().unwrap_or(0.0);
            if f == 0.0 {
                continue;
            }
            for k in 0..n {
                let mv = m.get(col * n + k).copied().unwrap_or(0.0);
                let iv = inv.get(col * n + k).copied().unwrap_or(0.0);
                if let Some(x) = m.get_mut(row * n + k) {
                    *x -= f * mv;
                }
                if let Some(x) = inv.get_mut(row * n + k) {
                    *x -= f * iv;
                }
            }
        }
    }
    // Rounding noise in the inverse scales with 1/scale of the input matrix.
    let noise = 1e-15 / scale;
    let inv = inv.into_iter().map(|x| if x.abs() < noise { 0.0 } else { x }).collect();
    to_value_array(n, n, inv)
}

pub(super) fn munit(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let n = num(a, 0)?.trunc();
    let (r, _) = check_size(n, n)?;
    let mut data = vec![Value::Number(0.0); r * r];
    for i in 0..r {
        if let Some(x) = data.get_mut(i * r + i) {
            *x = Value::Number(1.0);
        }
    }
    array_val(r, r, data)
}
