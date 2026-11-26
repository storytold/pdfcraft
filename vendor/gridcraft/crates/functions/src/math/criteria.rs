use super::{Arg, Array, CellError, Criterion, Ctx, R, Value, as_array, has, num_val};

// ---------------------------------------------------------------------------------------------
// Criteria aggregates

/// Values at the matched positions of `target` for every (range, criteria) pair.
fn criteria_mask(a: &[Arg], first_pair: usize, rows: usize, cols: usize) -> R<Vec<bool>> {
    let rest = a.get(first_pair..).unwrap_or(&[]);
    if rest.is_empty() || rest.len() % 2 != 0 {
        return Err(CellError::Value);
    }
    // A text criterion over 255 characters is `#VALUE!` only when there is a single criteria
    // pair. With two or more pairs Excel matches its first 255 characters instead.
    let single_pair = rest.len() == 2;
    let mut mask = vec![true; rows.saturating_mul(cols)];
    for pair in rest.chunks(2) {
        let (Some(range), Some(crit)) = (pair.first(), pair.get(1)) else { return Err(CellError::Value) };
        let r = as_array(&range.value);
        if r.rows != rows || r.cols != cols {
            return Err(CellError::Value);
        }
        let scalar = crit.value.scalar();
        let c = if single_pair { Criterion::parse_arg(&scalar)? } else { Criterion::parse_arg_truncated(&scalar) };
        for (m, v) in mask.iter_mut().zip(r.iter()) {
            if *m && !c.matches(v) {
                *m = false;
            }
        }
    }
    Ok(mask)
}

/// Numbers in `target` where `mask` is set (errors at matched positions propagate).
fn masked_numbers(target: &Array, mask: &[bool]) -> R<Vec<f64>> {
    let mut out = Vec::new();
    for (v, m) in target.iter().zip(mask.iter()) {
        if !*m {
            continue;
        }
        match v {
            Value::Number(n) => out.push(*n),
            Value::Error(e) => return Err(*e),
            _ => {}
        }
    }
    Ok(out)
}

pub(super) fn countifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let first = as_array(&crate::util::arg(a, 0)?.value);
    let mask = criteria_mask(a, 0, first.rows, first.cols)?;
    num_val(mask.iter().filter(|m| **m).count() as f64)
}

/// The values of the `*IFS` target range with the mask from its pairs.
fn ifs_numbers(a: &[Arg]) -> R<Vec<f64>> {
    let target = as_array(&crate::util::arg(a, 0)?.value);
    let mask = criteria_mask(a, 1, target.rows, target.cols)?;
    masked_numbers(&target, &mask)
}

pub(super) fn sumifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(ifs_numbers(a)?.iter().sum())
}

pub(super) fn averageifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = ifs_numbers(a)?;
    if v.is_empty() {
        return Err(CellError::Div0);
    }
    num_val(v.iter().sum::<f64>() / v.len() as f64)
}

pub(super) fn maxifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = ifs_numbers(a)?;
    num_val(v.iter().copied().reduce(f64::max).unwrap_or(0.0))
}

pub(super) fn minifs(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = ifs_numbers(a)?;
    num_val(v.iter().copied().reduce(f64::min).unwrap_or(0.0))
}

pub(super) fn countif(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let range = as_array(&crate::util::arg(a, 0)?.value);
    let c = Criterion::parse_arg(&crate::util::arg(a, 1)?.value.scalar())?;
    num_val(range.iter().filter(|v| c.matches(v)).count() as f64)
}

/// SUMIF / AVERAGEIF: matched numbers from the sum range, read at the criteria range's positions.
/// Excel sizes a sum range reference like the criteria range, anchored at its own top-left cell;
/// the evaluator does that resizing (it has the references), so cells line up one to one here.
fn if_numbers(a: &[Arg]) -> R<Vec<f64>> {
    let range = as_array(&crate::util::arg(a, 0)?.value);
    let c = Criterion::parse_arg(&crate::util::arg(a, 1)?.value.scalar())?;
    let sum_range = if has(a, 2) { Some(as_array(&crate::util::arg(a, 2)?.value)) } else { None };
    let mut out = Vec::new();
    for r in 0..range.rows {
        for col in 0..range.cols {
            let Some(v) = range.get(r, col) else { continue };
            if !c.matches(v) {
                continue;
            }
            let target = match &sum_range {
                Some(s) => s.get(r, col),
                None => Some(v),
            };
            match target {
                Some(Value::Number(n)) => out.push(*n),
                Some(Value::Error(e)) => return Err(*e),
                _ => {}
            }
        }
    }
    Ok(out)
}

pub(super) fn sumif(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    num_val(if_numbers(a)?.iter().sum())
}

pub(super) fn averageif(a: &[Arg], _c: &mut dyn Ctx) -> R<Value> {
    let v = if_numbers(a)?;
    if v.is_empty() {
        return Err(CellError::Div0);
    }
    num_val(v.iter().sum::<f64>() / v.len() as f64)
}
