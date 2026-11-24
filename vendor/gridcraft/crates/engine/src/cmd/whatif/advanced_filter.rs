use super::{Cell, CellRef, EngineError, HashSet, Json, RangeRef, Result, Session, Value, bad, bool_param, edit, json, parse_range, str_param};

// ---------------------------------------------------------------- advanced filter

#[derive(Clone, Debug)]
enum Cond {
    /// Column offset in the list, comparison operator and operand text.
    Compare(u32, Value),
    /// A computed criterion: formula relative to the first data row, evaluated at `at`.
    Computed(gridcraft_formula::Expr, CellRef),
}

/// Splits `">=10"` into (`">="`, `"10"`); text without an operator means "begins with".
fn split_op(t: &str) -> (&str, &str) {
    for op in ["<>", ">=", "<=", "=", ">", "<"] {
        if let Some(rest) = t.strip_prefix(op) {
            return (op, rest);
        }
    }
    ("begins", t)
}

/// Does a list value match one criteria cell's value (Excel's advanced-filter semantics)?
pub(crate) fn criterion_matches(v: &Value, text: &str, crit: &Value) -> bool {
    let crit_text = match crit {
        Value::Empty => return true,
        Value::Number(n) => return v.as_f64().is_some_and(|x| (x - n).abs() <= 1e-12 * n.abs().max(1.0)),
        Value::Bool(b) => return matches!(v, Value::Bool(x) if x == b),
        Value::Error(e) => return v.as_error() == Some(*e),
        Value::Text(t) => t.to_string(),
        Value::Array(_) => return false,
    };
    let (op, operand) = split_op(&crit_text);
    if operand.is_empty() {
        return match op {
            "=" => v.is_empty() || text.is_empty(),
            "<>" => !(v.is_empty() || text.is_empty()),
            _ => true,
        };
    }
    let num = gridcraft_core::parse::parse_number_text(operand);
    let lower_text = text.to_lowercase();
    let lower_op = operand.to_lowercase();
    use std::cmp::Ordering;
    let cmp: Option<Ordering> = match (v, num) {
        (Value::Number(a), Some(b)) => a.partial_cmp(&b),
        (Value::Number(_), None) => None,
        (Value::Empty, _) => None,
        (_, Some(_)) if op != "begins" => None,
        _ => Some(gridcraft_core::compare_text(text, operand)),
    };
    let wild = |pat: &str| crate::cmd::edit::wildcard(&lower_text, pat);
    match op {
        "=" => {
            if num.is_none() && operand.contains(['*', '?']) {
                wild(&lower_op)
            } else if num.is_none() {
                lower_text == lower_op
            } else {
                cmp == Some(Ordering::Equal)
            }
        }
        "<>" => {
            if num.is_none() && operand.contains(['*', '?']) {
                !wild(&lower_op)
            } else if num.is_none() {
                lower_text != lower_op
            } else {
                cmp != Some(Ordering::Equal)
            }
        }
        ">" => cmp == Some(Ordering::Greater),
        "<" => cmp == Some(Ordering::Less),
        ">=" => matches!(cmp, Some(Ordering::Greater | Ordering::Equal)),
        "<=" => matches!(cmp, Some(Ordering::Less | Ordering::Equal)),
        _ => {
            // Plain text criterion: begins with (wildcards allowed); a plain number equals.
            match (v, num) {
                (Value::Number(a), Some(b)) => (a - b).abs() <= 1e-12 * b.abs().max(1.0),
                _ => !v.is_empty() && wild(&format!("{lower_op}*")),
            }
        }
    }
}

/// Most (list row, criteria row) pairs one Advanced Filter tests.
const MAX_FILTER_TESTS: u64 = 50_000_000;

pub(super) fn advanced_filter(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "data.advancedFilter";
    let d = s.doc()?;
    let (si, list) = match str_param(p, "range") {
        Some(t) => parse_range(&d.wb, t, CMD)?,
        None => {
            let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
            (d.wb.active_sheet(), crate::selection::current_region(sh, d.selection.active))
        }
    };
    let criteria = str_param(p, "criteria").filter(|t| !t.trim().is_empty()).map(|t| parse_range(&d.wb, t, CMD)).transpose()?;
    if list.height() < 1 || list.count() > 50_000_000 {
        return Err(bad(CMD, "the list range is too large"));
    }
    if let Some((_, crit)) = criteria
        && (crit.width() > 1024 || crit.height() > 10_000)
    {
        return Err(bad(CMD, "the criteria range is too large"));
    }
    let wb = &d.wb;
    let sh = wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let headers: Vec<String> =
        (list.start.col..=list.end.col).map(|c| crate::display::cell_text(wb, sh, CellRef::new(list.start.row, c)).trim().to_lowercase()).collect();
    let first_data = list.start.row + 1;
    // Criteria rows → conditions.
    let mut rows: Vec<Vec<Cond>> = Vec::new();
    if let Some((csi, crit)) = criteria {
        let csh = wb.sheet(csi).ok_or(EngineError::NoDocument)?;
        for cr in crit.start.row + 1..=crit.end.row {
            let mut conds = Vec::new();
            for cc in crit.start.col..=crit.end.col {
                let at = CellRef::new(cr, cc);
                let cell = csh.cell(at);
                let v = csh.value(at);
                let formula = cell.and_then(|c| c.formula.as_ref());
                if v.is_empty() && formula.is_none() {
                    continue;
                }
                let head = crate::display::cell_text(wb, csh, CellRef::new(crit.start.row, cc)).trim().to_lowercase();
                match headers.iter().position(|h| !head.is_empty() && *h == head) {
                    Some(off) => conds.push(Cond::Compare(off as u32, v)),
                    None => match formula.and_then(|f| f.expr()) {
                        Some(e) => conds.push(Cond::Computed(e, at)),
                        None => return Err(EngineError::Other(format!("The criteria label `{head}` doesn't match a column of the list."))),
                    },
                }
            }
            rows.push(conds);
        }
    }
    // Each list row is tried against every criteria row, so bound their product too.
    if u64::from(list.height()) * rows.len() as u64 > MAX_FILTER_TESTS {
        return Err(bad(CMD, "the list and criteria ranges together are too large"));
    }
    let matches_row = |row: u32| -> bool {
        if rows.is_empty() {
            return true;
        }
        rows.iter().any(|conds| {
            conds.iter().all(|c| match c {
                Cond::Compare(off, crit) => {
                    let at = CellRef::new(row, list.start.col + off);
                    let v = sh.value(at);
                    let text = crate::display::cell_text(wb, sh, at);
                    criterion_matches(&v, &text, crit)
                }
                Cond::Computed(e, at) => {
                    let shifted = gridcraft_formula::adjust::shift_relative(e.clone(), row as i64 - first_data as i64, 0);
                    let v = gridcraft_calc::recalc::evaluate_expr(wb, criteria.map(|(si, _)| si).unwrap_or(si), *at, &shifted);
                    matches!(v.scalar().to_bool(), Ok(true))
                }
            })
        })
    };
    let unique = bool_param(p, "unique").unwrap_or(false);
    let mut seen: HashSet<Vec<String>> = HashSet::new();
    let mut keep: Vec<(u32, bool)> = Vec::new();
    for row in first_data..=list.end.row {
        let mut ok = matches_row(row);
        if ok && unique {
            let key: Vec<String> =
                (list.start.col..=list.end.col).map(|c| crate::display::cell_text(wb, sh, CellRef::new(row, c)).to_lowercase()).collect();
            ok = seen.insert(key);
        }
        keep.push((row, ok));
    }
    let matched = keep.iter().filter(|(_, k)| *k).count();
    if let Some(t) = str_param(p, "copyTo") {
        let (dsi, dest) = parse_range(wb, t, CMD)?;
        let dest = dest.start;
        let mut rows_out: Vec<u32> = vec![list.start.row];
        rows_out.extend(keep.iter().filter(|(_, k)| *k).map(|(r, _)| *r));
        let end =
            dest.offset(rows_out.len() as i64 - 1, list.width() as i64 - 1).ok_or_else(|| bad(CMD, "the extract range exceeds the worksheet"))?;
        let out_range = RangeRef::new(dest, end);
        if dsi == si && out_range.intersects(&list) {
            return Err(EngineError::Other("The extract range can't overlap the list range.".into()));
        }
        let cells: Vec<(CellRef, Cell)> = rows_out
            .iter()
            .enumerate()
            .flat_map(|(i, row)| {
                (list.start.col..=list.end.col).map(move |c| {
                    let src = CellRef::new(*row, c);
                    // The full output footprint was checked above.
                    let to = CellRef::new(dest.row + i as u32, dest.col + c - list.start.col);
                    let style = sh.style_id(src);
                    (to, Cell { value: sh.value(src), formula: None, style, py: None })
                })
            })
            .collect();
        edit(s, |cx| {
            let shm = cx.sheet_mut(dsi)?;
            let old: Vec<CellRef> = shm.cells.iter_range(out_range).map(|(c, _)| c).collect();
            for c in &old {
                shm.remove_cell(*c);
            }
            for (c, cell) in cells {
                shm.set_cell(c, cell);
            }
            for c in old {
                cx.touch(dsi, c);
            }
            for c in out_range.iter() {
                cx.touch(dsi, c);
            }
            Ok(())
        })?;
        return Ok(json!({"matched": matched, "range": out_range.a1()}));
    }
    let hidden = keep.iter().filter(|(_, k)| !*k).count();
    edit(s, |cx| {
        let shm = cx.sheet_mut(si)?;
        for (row, k) in &keep {
            if *k {
                if let Some(e) = shm.rows.get_mut(row) {
                    e.hidden = false;
                    if *e == gridcraft_model::LineInfo::default() {
                        shm.rows.remove(row);
                    }
                }
            } else {
                shm.rows.entry(*row).or_default().hidden = true;
            }
        }
        cx.structural = true;
        Ok(())
    })?;
    Ok(json!({"matched": matched, "hiddenRows": hidden}))
}
