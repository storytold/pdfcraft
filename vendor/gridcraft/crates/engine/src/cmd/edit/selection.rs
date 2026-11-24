use super::{
    CellRef, EngineError, Json, RangeRef, Result, Session, bool_param, cell_param, current_region, json, jump, split_sheet, str_param, target_ranges,
    target_sheet,
};

// ---------------------------------------------------------------- selection

/// Whether the cell may be selected: a protected sheet can forbid selecting locked and/or unlocked cells.
fn cell_selectable(wb: &gridcraft_model::Workbook, sheet: &gridcraft_model::Sheet, c: CellRef) -> bool {
    let Some(p) = sheet.protection.as_ref() else { return true };
    if wb.styles.get(sheet.style_id(c)).protection.locked { p.select_locked } else { p.select_unlocked }
}

pub(super) fn selection_set(s: &mut Session, p: &Json) -> Result<Json> {
    if p.get("sheet").is_some() {
        let i = target_sheet(s, p)?;
        s.doc_mut()?.wb_switch_sheet(i);
    } else if let Some(r) = str_param(p, "range").or_else(|| str_param(p, "cell"))
        && let (Some(_), _) = split_sheet(r)
    {
        let i = target_sheet(s, p)?;
        s.doc_mut()?.wb_switch_sheet(i);
    }
    let ranges = target_ranges(s, p)?;
    let active = cell_param(s, p, "active")?;
    let d = s.doc_mut()?;
    let active = active.or_else(|| ranges.last().map(|r| r.start)).unwrap_or_default();
    let mut sel = crate::Selection { active, anchor: ranges.last().map(|r| r.start).unwrap_or(active), ranges };
    if let Some(sh) = d.wb.active() {
        sel.expand_merges(sh);
        if !cell_selectable(&d.wb, sh, sel.active) {
            return Err(EngineError::Other("The cell you are trying to change is on a protected sheet that does not allow selecting it.".into()));
        }
    }
    d.selection = sel;
    Ok(json!({"selection": d.selection.a1(), "active": d.selection.active.a1()}))
}

impl crate::DocState {
    /// Switches the active sheet, remembering each sheet's selection.
    pub fn wb_switch_sheet(&mut self, i: usize) {
        if i >= self.wb.sheets.len() || i == self.wb.active_sheet() {
            return;
        }
        let cur = self.wb.active_sheet();
        if self.sheet_selections.len() < self.wb.sheets.len() {
            self.sheet_selections.resize(self.wb.sheets.len(), crate::Selection::default());
        }
        if let Some(slot) = self.sheet_selections.get_mut(cur) {
            *slot = self.selection.clone();
        }
        // View state: set in place, so the workbook is neither copied nor made dirty.
        self.wb.set_active_sheet(i);
        self.selection = self.sheet_selections.get(i).cloned().unwrap_or_default();
    }
}

pub(super) fn selection_move(s: &mut Session, p: &Json) -> Result<Json> {
    let dr = p.get("dr").and_then(Json::as_i64).unwrap_or(0);
    let dc = p.get("dc").and_then(Json::as_i64).unwrap_or(0);
    let extend = bool_param(p, "extend").unwrap_or(false);
    let jmp = bool_param(p, "jump").unwrap_or(false);
    let d = s.doc_mut()?;
    let Some(sheet) = d.wb.active() else { return Err(EngineError::NoDocument) };
    let sel = &mut d.selection;
    if extend && !jmp {
        // Shift+arrows move the edge the anchor doesn't hold, keeping merged cells whole.
        sel.extend_by(sheet, dr, dc);
        return Ok(json!({"selection": sel.a1(), "active": sel.active.a1()}));
    }
    // Extension moves the far corner, not the active cell.
    let from = if extend { sel.moving_end() } else { sel.active };
    // Skip hidden rows/columns and step over merged areas.
    let mut to = if jmp && extend && (dr == 0) != (dc == 0) {
        // Excel: Ctrl+Shift+arrow searches the active cell's column (or row) from the moving
        // edge, so a header row selected with Ctrl+Shift+Right extends down to the end of the
        // active column's data.
        let probe = if dr != 0 { CellRef::new(from.row, sel.active.col) } else { CellRef::new(sel.active.row, from.col) };
        let edge = jump(sheet, probe, dr.signum(), dc.signum());
        if dr != 0 { CellRef::new(edge.row, from.col) } else { CellRef::new(from.row, edge.col) }
    } else if jmp {
        jump(sheet, from, dr.signum(), dc.signum())
    } else {
        let mut t = from.offset_clamped(dr, dc);
        if !extend && let Some(m) = sheet.merge_at(from) {
            // Leaving a merged cell moves from its edge.
            if dr > 0 {
                t = CellRef::new(m.end.offset_clamped(dr, 0).row, t.col);
            } else if dc > 0 {
                t = CellRef::new(t.row, m.end.offset_clamped(0, dc).col);
            }
        }
        let mut guard = 0;
        while (sheet.is_row_hidden(t.row) || sheet.is_col_hidden(t.col)) && guard < 100_000 {
            // Step each hidden axis along its own direction, so a diagonal exit from a merge
            // (Tab-run return column) keeps its column while skipping hidden rows.
            let mut n = CellRef::new(
                if sheet.is_row_hidden(t.row) { t.offset_clamped(dr.signum(), 0).row } else { t.row },
                if sheet.is_col_hidden(t.col) { t.offset_clamped(0, dc.signum()).col } else { t.col },
            );
            if n == t {
                // The hidden axis has no direction (a move along the other axis): walk as before.
                n = t.offset_clamped(dr.signum(), dc.signum());
            }
            if n == t {
                break;
            }
            t = n;
            guard += 1;
        }
        t
    };
    if let Some(m) = sheet.merge_at(to)
        && !extend
    {
        to = m.start;
    }
    if !extend && !cell_selectable(&d.wb, sheet, to) {
        // Step on to the next selectable cell in the same direction; stay put if there is none.
        let (sr, sc) = (dr.signum(), dc.signum());
        let mut t = to;
        let mut found = false;
        if !jmp && (sr, sc) != (0, 0) {
            for _ in 0..10_000 {
                let n = t.offset_clamped(sr, sc);
                if n == t {
                    break;
                }
                t = n;
                if !sheet.is_row_hidden(t.row) && !sheet.is_col_hidden(t.col) && cell_selectable(&d.wb, sheet, t) {
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Ok(json!({"selection": sel.a1(), "active": sel.active.a1()}));
        }
        to = t;
    }
    if extend {
        sel.extend_to(to);
        sel.expand_merges(sheet);
    } else {
        *sel = crate::Selection::at(to);
        if let Some(m) = sheet.merge_at(to) {
            sel.ranges = vec![m];
        }
    }
    Ok(json!({"selection": sel.a1(), "active": sel.active.a1()}))
}

pub(super) fn selection_next(s: &mut Session, p: &Json) -> Result<Json> {
    let fwd = bool_param(p, "forward").unwrap_or(true);
    let by_row = bool_param(p, "byRow").unwrap_or(false);
    let d = s.doc_mut()?;
    let Some(sheet) = d.wb.active() else { return Err(EngineError::NoDocument) };
    let sel = &d.selection;
    if sel.is_one_merged_cell(sheet) {
        // A selection that is just one merged cell is one cell, as in Excel: Enter and Tab leave
        // it for the next cell instead of walking the cells it covers.
        let step = if fwd { 1 } else { -1 };
        let (dr, dc) = if by_row { (0, step) } else { (step, 0) };
        selection_move(s, &json!({"dr": dr, "dc": dc}))?;
        return Ok(json!({"active": s.doc()?.selection.active.a1()}));
    }
    // Inside a larger selection the cells a merged cell covers are skipped: it is visited once,
    // at its top-left cell.
    let area = sel.current();
    let cells =
        (u64::from(area.end.row.saturating_sub(area.start.row)) + 1).saturating_mul(u64::from(area.end.col.saturating_sub(area.start.col)) + 1);
    let mut next = sel.clone();
    for _ in 0..cells.min(1 << 20) {
        next.cycle(fwd, by_row);
        if sheet.merge_at(next.active).is_none_or(|m| m.start == next.active) {
            break;
        }
    }
    d.selection = next;
    Ok(json!({"active": d.selection.active.a1()}))
}

pub(super) fn selection_next_corner(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc_mut()?;
    let Some(sheet) = d.wb.active() else { return Err(EngineError::NoDocument) };
    // A corner in a merged cell lands on its top-left cell. One in a hidden row or column, one
    // the sheet's protection doesn't let you select, and a merged cell that starts outside the
    // selected area are skipped.
    let area = d.selection.current();
    let stop = |c: CellRef| {
        Some(sheet.merge_at(c).map_or(c, |m| m.start))
            .filter(|c| area.contains(*c) && !sheet.is_row_hidden(c.row) && !sheet.is_col_hidden(c.col) && cell_selectable(&d.wb, sheet, *c))
    };
    let mut next = d.selection.clone();
    next.next_corner_by(stop);
    d.selection = next;
    Ok(json!({"active": d.selection.active.a1(), "selection": d.selection.a1()}))
}

pub(super) fn select_all(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc_mut()?;
    let Some(sheet) = d.wb.active() else { return Err(EngineError::NoDocument) };
    let region = current_region(sheet, d.selection.active);
    let all = RangeRef::all();
    let r = if region.is_single() || d.selection.current() == region { all } else { region };
    let active = d.selection.active;
    d.selection = crate::Selection { active, anchor: r.start, ranges: vec![r] };
    Ok(json!({"selection": d.selection.a1()}))
}

pub(super) fn select_region(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc_mut()?;
    let Some(sheet) = d.wb.active() else { return Err(EngineError::NoDocument) };
    let r = current_region(sheet, d.selection.active);
    let active = d.selection.active;
    d.selection = crate::Selection { active, anchor: r.start, ranges: vec![r] };
    Ok(json!({"selection": d.selection.a1()}))
}

pub(super) fn select_row(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc_mut()?;
    let r = d.selection.current();
    let active = d.selection.active;
    d.selection = crate::Selection { active, anchor: CellRef::new(r.start.row, 0), ranges: vec![RangeRef::rows(r.start.row, r.end.row)] };
    Ok(json!({"selection": d.selection.a1()}))
}

pub(super) fn select_col(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc_mut()?;
    let r = d.selection.current();
    let active = d.selection.active;
    d.selection = crate::Selection { active, anchor: CellRef::new(0, r.start.col), ranges: vec![RangeRef::cols(r.start.col, r.end.col)] };
    Ok(json!({"selection": d.selection.a1()}))
}
