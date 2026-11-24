//! GETPIVOTDATA: a value from a PivotTable report, found by its value field and the row and
//! column items that locate it (Excel's documented semantics). Each PivotTable's value cells are
//! indexed in `PivotTable::data_index` (set on refresh and saved with the report; built on first
//! use only for a pivot loaded without one, and only when it matches the stored report); the
//! result is read from the report cell the sheet shows.

use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_formula::Expr;
use gridcraft_model::{PIVOT_INDEXER, PivotDataCell, PivotItemRef, PivotLookup, PivotTable};

use crate::eval::{Ev, Evaluator};

/// Excel allows 126 field/item pairs.
const MAX_PAIRS: usize = 126;

fn same_text(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// A number matches an item with that value (dates as serials) or with that label; anything
/// else matches the label as text. With `exact`, a number matches only an item of that value.
fn item_matches(item: &PivotItemRef, v: &Value, exact: bool) -> bool {
    if let Value::Number(n) = v {
        if let Value::Number(x) = &item.value
            && n == x
        {
            return true;
        }
        if exact {
            return false;
        }
    }
    if let Value::Bool(b) = v {
        return item.value == Value::Bool(*b);
    }
    v.to_text().is_ok_and(|t| same_text(&t, &item.label))
}

/// Whether `cell` is the value field asked for with exactly these items.
fn cell_matches(cell: &PivotDataCell, data_field: &str, pairs: &[(String, Value)], exact: bool) -> bool {
    (same_text(&cell.data_field, data_field) || same_text(&cell.source_col, data_field))
        && cell.items.len() == pairs.len()
        && pairs.iter().all(|(f, v)| cell.items.iter().any(|it| same_text(&it.field, f) && item_matches(it, v, exact)))
}

fn is_axis_field(pt: &PivotTable, field: &str) -> bool {
    pt.rows.iter().chain(pt.columns.iter()).any(|f| same_text(&f.source_col, field))
}

/// The value cell whose value field and items are exactly those asked for. Text items hit the
/// hash index; numbers and booleans also match by value, which a miss falls back to scanning for.
/// A number prefers the item of that value: two numeric items can show the same label (0.6 and 1
/// formatted as `0`), so a label hit counts only when no item has the exact value.
fn find<'a>(cells: &'a PivotLookup, data_field: &str, pairs: &[(String, Value)]) -> Option<&'a PivotDataCell> {
    let labels: Option<Vec<(&str, String, bool)>> =
        pairs.iter().map(|(f, v)| v.to_text().ok().map(|t| (f.as_str(), t, matches!(v, Value::Bool(_))))).collect();
    let hit = labels.as_ref().and_then(|labels| {
        let refs: Vec<(&str, &str, bool)> = labels.iter().map(|(f, t, b)| (*f, t.as_str(), *b)).collect();
        cells.find(data_field, &refs)
    });
    if pairs.iter().all(|(_, v)| matches!(v, Value::Text(_))) {
        return hit;
    }
    if let Some(c) = hit.filter(|c| cell_matches(c, data_field, pairs, true)) {
        return Some(c);
    }
    let all = cells.cells()?;
    all.iter()
        .find(|cell| cell_matches(cell, data_field, pairs, true))
        .or(hit)
        .or_else(|| all.iter().find(|cell| cell_matches(cell, data_field, pairs, false)))
}

impl Evaluator<'_> {
    pub(crate) fn getpivotdata(&mut self, args: &[Expr]) -> Ev {
        match self.pivot_value(args) {
            Ok(v) => Ev::V(v),
            Err(e) => Ev::V(Value::Error(e)),
        }
    }

    fn scalar_arg(&mut self, e: &Expr) -> Result<Value, CellError> {
        match self.value(e).scalar() {
            Value::Error(e) => Err(e),
            v => Ok(v),
        }
    }

    fn pivot_value(&mut self, args: &[Expr]) -> Result<Value, CellError> {
        let (Some(field_expr), Some(table_expr)) = (args.first(), args.get(1)) else { return Err(CellError::Value) };
        let rest = args.get(2..).unwrap_or_default();
        if rest.len() % 2 != 0 || rest.len() / 2 > MAX_PAIRS {
            return Err(CellError::Ref);
        }
        let data_field = self.scalar_arg(field_expr)?.to_text()?;
        let area = match self.eval(table_expr) {
            Ev::R(areas) => areas.first().copied().ok_or(CellError::Ref)?,
            Ev::V(Value::Error(e)) => return Err(e),
            _ => return Err(CellError::Ref),
        };
        let mut pairs: Vec<(String, Value)> = Vec::with_capacity(rest.len() / 2);
        for pair in rest.chunks(2) {
            let (Some(f), Some(i)) = (pair.first(), pair.get(1)) else { return Err(CellError::Ref) };
            let field = self.scalar_arg(f)?.to_text()?;
            let item = self.scalar_arg(i)?;
            if pairs.iter().any(|(g, _)| same_text(g, &field)) {
                return Err(CellError::Ref);
            }
            pairs.push((field, item));
        }
        let wb = self.host.workbook();
        let sheet = wb.sheet(area.sheet).ok_or(CellError::Ref)?;
        let pt = sheet.pivots.iter().find(|p| p.last_range.is_some_and(|r| r.intersects(&area.range))).ok_or(CellError::Ref)?;
        // A report filter field may be named when the report shows exactly that one item.
        let mut axis_pairs = Vec::with_capacity(pairs.len());
        for (field, item) in pairs {
            if is_axis_field(pt, &field) {
                axis_pairs.push((field, item));
                continue;
            }
            let filter = pt.filters.iter().find(|f| same_text(&f.source_col, &field)).ok_or(CellError::Ref)?;
            let shown = match filter.selected.as_deref() {
                Some([one]) => item.to_text().is_ok_and(|t| same_text(&t, one)),
                _ => false,
            };
            if !shown {
                return Err(CellError::Ref);
            }
        }
        // The report's value cells. A pivot loaded without them builds them once; a stored report
        // that no longer matches its source gets no index at all, so the lookup is #REF! until a
        // refresh — never an index cached from a failed build, because the shared lookup keeps
        // the first one set and undo or reopen could then never rebuild it.
        if pt.data_index.cells().is_none() {
            let build = PIVOT_INDEXER.get().ok_or(CellError::Ref)?;
            if let Ok(cells) = build(wb, area.sheet, pt) {
                pt.data_index.set_cells(cells);
            }
        }
        let cell = find(&pt.data_index, &data_field, &axis_pairs).ok_or(CellError::Ref)?;
        // The value is the one the sheet shows, so the formula and the report cell always agree.
        let report = pt.last_range.ok_or(CellError::Ref)?;
        let at = CellRef::new(report.start.row.saturating_add(cell.row), report.start.col.saturating_add(cell.col));
        if !report.contains(at) {
            return Err(CellError::Ref);
        }
        Ok(sheet.value(at))
    }
}
