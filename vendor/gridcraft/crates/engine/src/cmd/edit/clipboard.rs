use super::cell::input_to_cell;
use super::paste_special::paste_special;
use super::{
    Cell, CellRef, Clipboard, EngineError, Json, RangeRef, Result, Session, Value, bad, bool_param, cell_param_on, edit, json, ok, str_param,
    target_ranges, target_sheet,
};

// ---------------------------------------------------------------- clipboard

fn copy_impl(s: &mut Session, p: &Json, cut: bool) -> Result<Json> {
    let multiple = || EngineError::Other("This action won't work on multiple selections.".into());
    let sheet = target_sheet(s, p)?;
    let ranges = target_ranges(s, p)?;
    if ranges.len() > 1 && cut {
        return Err(multiple());
    }
    // The selection as made (untrimmed), for Insert Copied Cells.
    let mut selected = ranges.first().copied().unwrap_or_default();
    let d = s.doc()?;
    let Some(sh) = d.wb.sheet(sheet) else { return Err(EngineError::NoDocument) };
    let trim = |range: RangeRef| {
        if range.is_full_cols() || range.is_full_rows() {
            let used = sh.used_range().unwrap_or(RangeRef::cell(range.start));
            range.intersection(&used.union(&RangeRef::cell(range.start))).unwrap_or(range)
        } else {
            range
        }
    };
    let (wb, range, areas) = if ranges.len() > 1 {
        let areas: Vec<RangeRef> = ranges.iter().map(|r| trim(*r)).collect();
        let (wb, block) = slide_together(&d.wb, sheet, &areas).ok_or_else(multiple)?;
        (std::sync::Arc::new(wb), block, areas)
    } else {
        let range = trim(ranges.first().copied().unwrap_or_default());
        // A copied range with rows a filter hides takes only the rows showing: they are copied
        // like a multi-area selection stacked top to bottom (formulas as values), as in Excel.
        // A cut still takes every row.
        let hidden = if cut { Default::default() } else { super::filtered_rows(sh, range) };
        let visible = if hidden.is_empty() { Vec::new() } else { visible_row_runs(range, &hidden) };
        let any_visible = !visible.is_empty();
        let packed = match visible.as_slice() {
            [] => (d.wb.clone(), range, Vec::new()),
            [only] => (d.wb.clone(), *only, Vec::new()),
            _ => {
                let (wb, block) = slide_together(&d.wb, sheet, &visible).ok_or_else(multiple)?;
                (std::sync::Arc::new(wb), block, visible)
            }
        };
        // Whole rows copied with some hidden by a filter come as the block of rows showing, so
        // Insert Copied Cells makes room for those rows, not for the hidden ones too.
        if !hidden.is_empty() && any_visible && selected.is_full_rows() {
            selected = RangeRef::rows(packed.1.start.row, packed.1.end.row);
        }
        packed
    };
    let text = crate::display::range_text(&wb, sheet, range);
    // The HTML rendering is for the host clipboard (UI); it can reach 4 MB, so agents and
    // scripts get it only when they ask for it.
    let html = if p.get("html").and_then(Json::as_bool) == Some(true) { crate::io::range_html(&wb, sheet, range, &text) } else { None };
    let clip = Clipboard { wb, sheet, range, cut, doc_uid: d.uid, text: text.clone(), areas, selected, trust: d.trust };
    s.clipboard = Some(clip);
    let mut result = json!({"range": range.a1(), "text": text});
    if let Some(html) = html {
        result["html"] = Json::String(html);
    }
    Ok(result)
}

/// The runs of consecutive rows of `range` not in `hidden`, as ranges of `range`'s columns.
fn visible_row_runs(range: RangeRef, hidden: &std::collections::BTreeSet<u32>) -> Vec<RangeRef> {
    let mut runs = Vec::new();
    let mut start = None;
    for row in range.start.row..=range.end.row {
        match (hidden.contains(&row), start) {
            (false, None) => start = Some(row),
            (true, Some(first)) => {
                runs.push(RangeRef::new(CellRef::new(first, range.start.col), CellRef::new(row - 1, range.end.col)));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(first) = start {
        runs.push(RangeRef::new(CellRef::new(first, range.start.col), CellRef::new(range.end.row, range.end.col)));
    }
    runs
}

/// The validation rules of `src` cut into the areas of a packed block: one rule per area a rule
/// touches, over that area's part moved by the area's shift. `sorted` areas run in order.
fn pack_validations(src: &gridcraft_model::Sheet, sorted: &[RangeRef], shifts: &[(i64, i64)], across: bool) -> Vec<gridcraft_model::Validation> {
    let key = |c: CellRef| if across { c.col } else { c.row };
    let mut packed_rules = Vec::new();
    for dv in &src.validations {
        let Some(from) = dv.ranges.first().map(|r| r.start) else { continue };
        // The areas each of the rule's ranges touches lie side by side in the sorted list.
        let end_key = |a: &RangeRef| if across { a.end.col } else { a.end.row };
        let mut by_area: std::collections::BTreeMap<usize, Vec<RangeRef>> = Default::default();
        for r in &dv.ranges {
            let first = sorted.partition_point(|a| end_key(a) < key(r.start));
            for (i, a) in sorted.iter().enumerate().skip(first).take_while(|(_, a)| key(a.start) <= end_key(r)) {
                let (dr, dc) = shifts[i];
                let moved = r.intersection(a).and_then(|x| Some(RangeRef::new(x.start.offset(dr, dc)?, x.end.offset(dr, dc)?)));
                by_area.entry(i).or_default().extend(moved);
            }
        }
        for ranges in by_area.into_values() {
            let Some(to) = ranges.first().map(|r| r.start) else { continue };
            let mut rule = dv.clone();
            rule.f1 = super::super::data::move_rule_formula(&dv.f1, from, to);
            rule.f2 = dv.f2.as_ref().map(|f| super::super::data::move_rule_formula(f, from, to));
            rule.ranges = ranges;
            packed_rules.push(rule);
        }
    }
    packed_rules
}

/// Excel copies a multi-area selection only when the areas line up: all cover the same rows (they
/// slide together side by side, left to right) or all cover the same columns (stacked, top to
/// bottom), without overlapping. Returns a copy of the workbook whose sheet holds the areas packed
/// into one block at the first area's place, and that block. Formulas travel as their values, as
/// in Excel. `None` when the areas don't line up.
fn slide_together(wb: &gridcraft_model::Workbook, sheet: usize, areas: &[RangeRef]) -> Option<(gridcraft_model::Workbook, RangeRef)> {
    let first = *areas.first()?;
    let across = areas.iter().all(|a| a.start.row == first.start.row && a.end.row == first.end.row);
    let down = areas.iter().all(|a| a.start.col == first.start.col && a.end.col == first.end.col);
    let mut sorted = areas.to_vec();
    if across {
        sorted.sort_by_key(|a| a.start.col);
    } else if down {
        sorted.sort_by_key(|a| a.start.row);
    } else {
        return None;
    }
    if sorted.windows(2).any(|w| matches!(w, [a, b] if a.intersects(b))) {
        return None;
    }
    let anchor = sorted.first()?.start;
    let (h, w) = if across {
        (first.height(), sorted.iter().map(RangeRef::width).fold(0u32, u32::saturating_add))
    } else {
        (sorted.iter().map(RangeRef::height).fold(0u32, u32::saturating_add), first.width())
    };
    let block = RangeRef::new(anchor, anchor.offset(i64::from(h) - 1, i64::from(w) - 1)?);
    let src = wb.sheet(sheet)?;
    let mut out = wb.clone();
    let dst = out.sheet_mut(sheet)?;
    dst.take_cells(block);
    dst.spill.retain(|c, _| !block.contains(*c));
    dst.spill_ranges.retain(|c, _| !block.contains(*c));
    dst.comments.retain(|c, _| !block.contains(*c));
    dst.hyperlinks.retain(|c, _| !block.contains(*c));
    super::super::data::remove_validation_cells(&mut dst.validations, &[block]);
    dst.merges.retain(|m| !m.intersects(&block));
    if across {
        dst.cols.retain(|c, _| !(block.start.col..=block.end.col).contains(c));
    } else {
        dst.rows.retain(|r, _| !(block.start.row..=block.end.row).contains(r));
    }
    // Where each area lands: the shift of its cells, and its first row or column in the block.
    let key = |c: CellRef| if across { c.col } else { c.row };
    let mut next = if across { anchor.col } else { anchor.row };
    let mut shifts: Vec<(i64, i64)> = Vec::with_capacity(sorted.len());
    for a in &sorted {
        let to = i64::from(next);
        shifts.push(if across { (0, to - i64::from(a.start.col)) } else { (to - i64::from(a.start.row), 0) });
        next = next.saturating_add(if across { a.width() } else { a.height() });
    }
    // The areas never overlap and run in order, so the area holding a cell is found by search:
    // each map below is read once, whatever the number of areas.
    let area_of = |c: CellRef| -> Option<usize> {
        let i = sorted.partition_point(|a| key(a.start) <= key(c)).checked_sub(1)?;
        sorted.get(i)?.contains(c).then_some(i)
    };
    let moved = |c: CellRef| -> Option<CellRef> {
        let (dr, dc) = shifts[area_of(c)?];
        c.offset(dr, dc)
    };
    for (a, &(dr, dc)) in sorted.iter().zip(&shifts) {
        for (c, _) in src.cells.iter_range(*a) {
            let Some(to) = c.offset(dr, dc) else { continue };
            let style = src.cell(c).map(|x| x.style).unwrap_or_default();
            dst.set_cell(to, Cell { value: src.value(c), formula: None, style, py: None });
            if let Some(picture) = src.cell_pictures.get(&c) {
                dst.cell_pictures.insert(to, picture.clone());
            }
        }
    }
    for c in src.spill.keys().copied() {
        let Some(to) = moved(c) else { continue };
        let style = src.cell(c).map(|x| x.style).unwrap_or_default();
        dst.set_cell(to, Cell { value: src.value(c), formula: None, style, py: None });
        if let Some(picture) = src.cell_pictures.get(&c) {
            dst.cell_pictures.insert(to, picture.clone());
        }
    }
    // Links and validation rules follow their cells too, so a row the filter hides leaves its own
    // behind.
    for (c, link) in &src.hyperlinks {
        if let Some(to) = moved(*c) {
            dst.hyperlinks.insert(to, link.clone());
        }
    }
    let packed_rules = pack_validations(src, &sorted, &shifts, across);
    for (c, comment) in &src.comments {
        if let Some(to) = moved(*c) {
            dst.comments.insert(to, comment.clone());
        }
    }
    for m in &src.merges {
        let Some(i) = area_of(m.start).filter(|&i| sorted[i].contains_range(m)) else { continue };
        let (dr, dc) = shifts[i];
        if let (Some(s), Some(e)) = (m.start.offset(dr, dc), m.end.offset(dr, dc)) {
            dst.merges.push(RangeRef::new(s, e));
        }
    }
    // Column widths (side by side) or row heights and hidden rows (stacked) follow their cells.
    let mut next = if across { anchor.col } else { anchor.row };
    for a in &sorted {
        if across {
            for (c, info) in src.cols.range(a.start.col..=a.end.col) {
                dst.cols.insert(next.saturating_add(c.saturating_sub(a.start.col)), *info);
            }
        } else {
            for (r, info) in src.rows.range(a.start.row..=a.end.row) {
                dst.rows.insert(next.saturating_add(r.saturating_sub(a.start.row)), *info);
            }
        }
        next = next.saturating_add(if across { a.width() } else { a.height() });
    }
    dst.validations.extend(packed_rules);
    Some((out, block))
}

pub(super) fn copy(s: &mut Session, p: &Json) -> Result<Json> {
    copy_impl(s, p, false)
}

pub(super) fn cut(s: &mut Session, p: &Json) -> Result<Json> {
    copy_impl(s, p, true)
}

pub(super) fn clear_clipboard(s: &mut Session, _: &Json) -> Result<Json> {
    s.clipboard = None;
    s.format_painter = None;
    ok()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum What {
    All,
    Formulas,
    Values,
    Formats,
    Comments,
    Validation,
    AllExceptBorders,
    ColumnWidths,
    FormulasAndNumberFormats,
    ValuesAndNumberFormats,
}

pub(super) fn paste(s: &mut Session, p: &Json) -> Result<Json> {
    // External text (from the system clipboard) when it differs from ours.
    if let Some(text) = str_param(p, "text")
        && s.clipboard.as_ref().is_none_or(|c| c.text.trim_end_matches(['\r', '\n']) != text.trim_end_matches(['\r', '\n']))
    {
        return paste_text(s, p, text);
    }
    if s.clipboard.is_none() {
        return Err(EngineError::Other("Nothing to paste.".into()));
    }
    paste_special(s, &json!({"what": "all", "at": p.get("at").cloned().unwrap_or(Json::Null)}))
}

/// Drag and drop (dragging the selection's border): moves `range` so its top-left cell lands on
/// `to`, as Cut and Paste would. Like Excel, it asks before replacing cells that hold data or show a
/// spilled value (other than the moved cells themselves); run it again with `overwrite: true` to replace them. A
/// refused or failed move leaves the clipboard as it was.
pub(super) fn range_move(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let to = cell_param_on(s, p, "to", sheet)?.ok_or_else(|| bad("range.move", "missing `to`"))?;
    let kept = s.clipboard.take();
    let moved = cut(s, p).and_then(|_| {
        let src = s.clipboard.as_ref().map(|c| c.range).unwrap_or_default();
        let dest = to.offset(i64::from(src.height()) - 1, i64::from(src.width()) - 1).map(|end| RangeRef::new(to, end));
        if bool_param(p, "overwrite") != Some(true)
            && let Some(dest) = dest
            && let Some(sh) = s.doc()?.wb.sheet(sheet)
            && (sh.cells.iter_range(dest).any(|(c, cell)| !src.contains(c) && cell.has_content())
                || sh.spill.range(dest.start..=dest.end).any(|(c, v)| dest.contains(*c) && !src.contains(*c) && *v != Value::Empty))
        {
            return Err(EngineError::Confirm("There's already data here. Do you want to replace it?".into()));
        }
        // On the range's sheet, which `paste` would leave to the active one. The selection
        // belongs to the active sheet: it follows the moved cells only when they are on it.
        let selection = s.doc()?.selection.clone();
        let pasted = paste_special(s, &json!({"what": "all", "at": to.a1(), "sheet": sheet}))?;
        if sheet != s.doc()?.wb.active_sheet() {
            s.doc_mut()?.selection = selection;
        }
        Ok(pasted)
    });
    if moved.is_err() {
        s.clipboard = kept;
    }
    moved
}

fn paste_text(s: &mut Session, p: &Json, text: &str) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let at = cell_param_on(s, p, "at", sheet)?.unwrap_or(s.doc()?.selection.current().start);
    let rows: Vec<Vec<String>> = parse_tsv(text);
    let h = rows.len();
    let w = rows.iter().map(Vec::len).max().unwrap_or(0);
    // The HTML flavour, when the host passed it: the number behind each cell's shown text. Read
    // only as far as the text's own shape, so spans in the markup can't expand without bound.
    let html = str_param(p, "html").map(|html| crate::clip_html::table_cells(html, h, w)).unwrap_or_default();
    edit(s, |cx| {
        for (ri, row) in rows.iter().enumerate() {
            for (ci, v) in row.iter().enumerate() {
                let Some(c) = at.offset(ri as i64, ci as i64) else { continue };
                let mut cell = input_to_cell(v, sheet, c, &mut cx.wb).unwrap_or_else(|_| Some(Cell::value(Value::text(v.as_str()))));
                if let Some(cl) = cell.as_mut() {
                    numeric_from_html(cl, v, html.get(ri).and_then(|r| r.get(ci)), &mut cx.wb);
                }
                let sh = cx.sheet_mut(sheet)?;
                match cell {
                    Some(cell) => sh.set_cell(c, cell),
                    None => {
                        sh.remove_cell(c);
                    }
                }
                cx.touch(sheet, c);
            }
        }
        if h > 0 && w > 0 {
            let end = at.offset_clamped(h as i64 - 1, w as i64 - 1);
            *cx.sel = crate::Selection { active: at, anchor: at, ranges: vec![RangeRef::new(at, end)] };
        }
        Ok(())
    })?;
    Ok(json!({"rows": h, "cols": w}))
}

/// A pasted cell that came out as text ("Rp10,000.00" isn't en-US input) becomes the number the
/// source recorded for it in its HTML, with a format that shows the same text when one can be
/// inferred and the cell has none. The HTML cell must show the same text as the tab-separated
/// field, so HTML that belongs to another copy (or a table laid out differently) changes nothing.
fn numeric_from_html(cell: &mut Cell, field: &str, html: Option<&crate::clip_html::HtmlCell>, wb: &mut gridcraft_model::Workbook) {
    let Some(hc) = html else { return };
    let Some(n) = hc.num else { return };
    if cell.formula.is_some() || !matches!(cell.value, Value::Text(_)) || wb.styles.get(cell.style).num_fmt.as_str() == "@" {
        return;
    }
    if crate::clip_html::normalized(&hc.text) != crate::clip_html::normalized(field) {
        return;
    }
    cell.value = Value::number(n);
    if wb.styles.get(cell.style).num_fmt.as_str() == "General"
        && let Some(code) = crate::clip_html::format_for(field, n)
    {
        cell.style = wb.styles.derive(cell.style, |s| s.num_fmt = gridcraft_model::NumFmt::new(&code));
    }
}

/// Tab-separated text with Excel-style quoting.
pub(crate) fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    // Text read again as unquoted, ahead of the rest of the input: a field that opened with a
    // quote but turned out not to be quoted.
    let mut replay: std::collections::VecDeque<char> = std::collections::VecDeque::new();
    // Text is replayed at most as much as the input is long; past that, quotes are text, so
    // hostile input full of fields that only look quoted reads in linear time.
    let mut replay_budget = text.len();
    let mut quoted = false;
    let mut at_start = true;
    // A quoted field's text as it stands in the input, for a field that turns out not to be one.
    let mut raw = String::new();
    loop {
        let c = match replay.pop_front().or_else(|| chars.next()) {
            Some(c) => c,
            // A quote that never closes: the field is its text as written.
            None if quoted => {
                quoted = false;
                field.clear();
                field.push('"');
                replay_budget = replay_budget.saturating_sub(raw.len());
                replay.extend(raw.chars().skip(1));
                raw.clear();
                continue;
            }
            None => break,
        };
        if quoted {
            raw.push(c);
            if c == '"' {
                let next = replay.front().copied().or_else(|| chars.peek().copied());
                if next == Some('"') {
                    field.push('"');
                    raw.push('"');
                    if replay.pop_front().is_none() {
                        chars.next();
                    }
                } else {
                    quoted = false;
                    // A field is quoted only when its closing quote ends it, as Excel reads it:
                    // `"hi" there` is that text, quotes included, and a tab or line break inside
                    // it still ends the cell, as if the leading quote weren't there.
                    if !matches!(next, None | Some('\t' | '\n' | '\r')) {
                        field.clear();
                        field.push('"');
                        replay_budget = replay_budget.saturating_sub(raw.len());
                        let rest: Vec<char> = std::mem::take(&mut raw).chars().skip(1).collect();
                        for ch in rest.into_iter().rev() {
                            replay.push_front(ch);
                        }
                    }
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if at_start && replay_budget > 0 => {
                quoted = true;
                at_start = false;
                raw = String::from('"');
            }
            '\t' => {
                row.push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_start = true;
            }
            _ => {
                field.push(c);
                at_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}
