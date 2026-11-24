use super::{
    Anchor, AnchorMode, Arc, Cell, CellRef, Comment, EngineError, Formula, Hyperlink, Json, LineInfo, Ordering, RangeRef, Result, Session, Sheet,
    Value, bad, bool_param, col_param, data_range, edit, guess_header, json, ok, sort_compare, split_sheet, str_param, target_sheet,
};

#[derive(Clone)]
struct SortKey {
    col: u32,
    desc: bool,
    by: String,
    list: Option<Vec<String>>,
}

/// The sheet and range a sort works on: the sheet `sheet` or a `Sheet!` prefix on `range` names,
/// else the active one. The selection belongs to the active sheet, so another sheet needs `range`.
fn sort_target(s: &Session, p: &Json) -> Result<(usize, RangeRef)> {
    let sheet = target_sheet(s, p)?;
    let d = s.doc()?;
    if let Some(t) = str_param(p, "range")
        && let (Some(name), _) = split_sheet(t)
        && d.wb.sheet_index(&name) != Some(sheet)
    {
        return Err(bad("data.sort", format!("`{t}` names a sheet other than `sheet`")));
    }
    if sheet != d.wb.active_sheet() && p.get("range").is_none() {
        return Err(bad("data.sort", "missing `range`: the selection is on the active sheet"));
    }
    Ok((sheet, data_range(s, p)?))
}

/// The worksheet (index), range and header guess the Custom Sort dialog works on: the sheet named by
/// `sheet` or a `Sheet!` prefix on `range`, else the active one.
pub fn sort_context(s: &Session, p: &Json) -> Result<(usize, RangeRef, bool)> {
    let (sheet, explicit) = sort_target(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let sel = d.selection.current();
    // An explicit block wins over the table range, so a selected totals row can be sorted.
    let r = match sh.used_range() {
        Some(used) if p.get("range").is_none() && !sel.is_single() => sel.intersection(&used).unwrap_or(sel),
        _ => explicit,
    };
    Ok((sheet, r, bool_param(p, "header").unwrap_or_else(|| guess_header(&d.wb, sh, r))))
}

pub(super) fn quick_sort(s: &mut Session, p: &Json, asc: bool) -> Result<Json> {
    let (sheet, r) = sort_target(s, p)?;
    let col = col_param(p.get("column"), r.start.col).unwrap_or(s.doc()?.selection.active.col).clamp(r.start.col, r.end.col);
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let header = guess_header(&d.wb, sh, r);
    do_sort(s, sheet, r, header, vec![SortKey { col, desc: !asc, by: "values".into(), list: None }], false, false)
}

pub(super) fn sort(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, r) = sort_target(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let header = bool_param(p, "header").unwrap_or_else(|| guess_header(&d.wb, sh, r));
    let by_cols = str_param(p, "orientation") == Some("columns");
    let case = bool_param(p, "matchCase").unwrap_or(false);
    let mut keys: Vec<SortKey> = Vec::new();
    for k in p.get("keys").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default() {
        let col = if by_cols {
            let row = match k.get("row").and_then(Json::as_u64) {
                // Worksheet rows are 1-based; 0 (or a value past u32) has no coordinate.
                Some(n) => {
                    let row = u32::try_from(n).ok().and_then(|n| n.checked_sub(1));
                    Some(row.ok_or_else(|| bad("data.sort", "sort key row must be 1 or greater"))?)
                }
                None => None,
            };
            row.or_else(|| col_param(k.get("column"), r.start.row))
        } else {
            col_param(k.get("column"), r.start.col)
        };
        let Some(col) = col else { continue };
        keys.push(SortKey {
            col,
            desc: k.get("order").and_then(Json::as_str) == Some("desc"),
            by: k.get("by").and_then(Json::as_str).unwrap_or("values").to_string(),
            list: k.get("customList").and_then(Json::as_array).map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
        });
    }
    if keys.is_empty() {
        let (sheet, r, header) = sort_context(s, p)?;
        let name = s.doc()?.wb.sheet(sheet).map(|sh| sh.name.clone()).ok_or(EngineError::NoDocument)?;
        s.ui_requests.push(crate::UiRequest::Dialog("sort".into(), json!({"range": r.a1(), "header": header, "sheet": name})));
        return ok();
    }
    if keys.len() > 64 {
        return Err(bad("data.sort", "too many sort levels (64 max)"));
    }
    do_sort(s, sheet, r, header, keys, by_cols, case)
}

fn do_sort(s: &mut Session, sheet: usize, r: RangeRef, header: bool, keys: Vec<SortKey>, by_cols: bool, case: bool) -> Result<Json> {
    // Protect Sheet ▸ Sort. Both Sort… and the A-to-Z buttons come through here.
    crate::cmd::refuse_unless_allowed(s, sheet, "Sorting", |p| p.sort)?;
    let body = if by_cols {
        if header && r.width() > 1 { RangeRef::new(CellRef::new(r.start.row, r.start.col + 1), r.end) } else { r }
    } else if header && r.height() > 1 {
        RangeRef::new(CellRef::new(r.start.row + 1, r.start.col), r.end)
    } else {
        r
    };
    if body.count() > 20_000_000 {
        return Err(bad("data.sort", "range too large"));
    }
    edit(s, |cx| {
        let Some(sh) = cx.wb.sheet(sheet) else { return Ok(Json::Null) };
        if sh.merges.iter().any(|m| m.intersects(&body) && !m.is_single()) {
            return Err(EngineError::Other("To do this, all the merged cells need to be the same size.".into()));
        }
        let n = if by_cols { body.width() } else { body.height() };
        let line = |i: u32, k: u32| if by_cols { CellRef::new(k, body.start.col + i) } else { CellRef::new(body.start.row + i, k) };
        let theme = cx.wb.theme.clone();
        let styles = &cx.wb.styles;
        let key_val = |i: u32, k: &SortKey| -> (Value, u32) {
            let c = line(i, k.col);
            let v = sh.value(c);
            let color = match k.by.as_str() {
                "cellColor" => {
                    styles.get(sh.style_id(c)).fill.fg.resolve(&theme).map(|[r, g, b]| u32::from_be_bytes([0, r, g, b])).unwrap_or(u32::MAX)
                }
                "fontColor" => {
                    styles.get(sh.style_id(c)).font.color.resolve(&theme).map(|[r, g, b]| u32::from_be_bytes([0, r, g, b])).unwrap_or(u32::MAX)
                }
                _ => 0,
            };
            (v, color)
        };
        let mut order: Vec<u32> = (0..n).collect();
        let keyed: Vec<Vec<(Value, u32)>> = (0..n).map(|i| keys.iter().map(|k| key_val(i, k)).collect()).collect();
        order.sort_by(|&a, &b| {
            for (ki, k) in keys.iter().enumerate() {
                let (va, ca) = &keyed[a as usize][ki];
                let (vb, cb) = &keyed[b as usize][ki];
                let plain = || {
                    if case && va.is_text() && vb.is_text() {
                        sort_compare(va, vb).then_with(|| case_order(va.as_text().unwrap_or(""), vb.as_text().unwrap_or("")))
                    } else {
                        sort_compare(va, vb)
                    }
                };
                let o = if k.by != "values" {
                    ca.cmp(cb)
                } else if let Some(list) = &k.list {
                    let pos = |v: &Value| v.as_text().and_then(|t| list.iter().position(|x| x.eq_ignore_ascii_case(t)));
                    match (pos(va), pos(vb)) {
                        (Some(a), Some(b)) => a.cmp(&b),
                        (Some(_), None) => Ordering::Less,
                        (None, Some(_)) => Ordering::Greater,
                        // Values not in the list follow the listed ones in the usual order, as in Excel.
                        // A descending key is left as it was: they tie, so the next key decides.
                        // A blank has no place in the list either, but still goes last.
                        (None, None) if k.desc => va.is_empty().cmp(&vb.is_empty()),
                        (None, None) => plain(),
                    }
                } else {
                    plain()
                };
                // Blanks always sort last, whatever the order.
                let o = if k.desc && !(va.is_empty() || vb.is_empty()) { o.reverse() } else { o };
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        });
        if order.iter().enumerate().all(|(i, &o)| i as u32 == o) {
            return Ok(Json::Null);
        }
        // Move whole lines (cells with formulas adjusted for relative references).
        let lanes: Vec<u32> = if by_cols { (body.start.row..=body.end.row).collect() } else { (body.start.col..=body.end.col).collect() };
        // Notes and hyperlinks hang off a cell, not a row, so they travel with the row they
        // describe — the way cell pictures and row heights do. Collect them before inserting,
        // so a swap can't resurrect one from a destination that is also a source.
        let side: Vec<(CellRef, Comment)> = order
            .iter()
            .enumerate()
            .flat_map(|(dst_i, &src_i)| lanes.iter().map(move |&k| (src_i, dst_i as u32, k)))
            .filter_map(|(src_i, dst_i, k)| sh.comments.get(&line(src_i, k)).map(|cm| (line(dst_i, k), cm.clone())))
            .collect();
        let links: Vec<(CellRef, Hyperlink)> = order
            .iter()
            .enumerate()
            .flat_map(|(dst_i, &src_i)| lanes.iter().map(move |&k| (src_i, dst_i as u32, k)))
            .filter_map(|(src_i, dst_i, k)| sh.hyperlinks.get(&line(src_i, k)).map(|h| (line(dst_i, k), h.clone())))
            .collect();
        let mut moved: Vec<(CellRef, Option<Cell>)> = Vec::with_capacity(body.count() as usize);
        let mut pictures = Vec::new();
        for (dst_i, &src_i) in order.iter().enumerate() {
            for &k in &lanes {
                let src = line(src_i, k);
                let dst = line(dst_i as u32, k);
                let mut cell = sh.cell(src).cloned();
                if let Some(c) = cell.as_mut()
                    && let Some(f) = &c.formula
                    && let Some(e) = f.expr()
                {
                    let shifted = gridcraft_formula::adjust::shift_relative(e, dst.row as i64 - src.row as i64, dst.col as i64 - src.col as i64);
                    c.formula = Some(Arc::new(Formula::from_expr(shifted)));
                }
                if let Some(picture) = sh.cell_pictures.get(&src) {
                    pictures.push((dst, picture.clone()));
                }
                moved.push((dst, cell));
            }
        }
        // Row heights travel with rows.
        let heights: Vec<Option<LineInfo>> =
            if by_cols { vec![] } else { order.iter().map(|&i| sh.rows.get(&(body.start.row + i)).copied()).collect() };
        // Floating objects anchored inside the sorted body travel with their lines, unless they
        // are pinned absolutely ("Don't move or size with cells", `editAs="absolute"`). Reordering
        // is the inverse of `order`: `dst_of[src] = dst`.
        let dst_of: Vec<u32> = {
            let mut v = vec![0u32; order.len()];
            for (dst_i, &src_i) in order.iter().enumerate() {
                if let Some(slot) = v.get_mut(src_i as usize) {
                    *slot = dst_i as u32;
                }
            }
            v
        };
        // The object's own geometry (its top-left anchor cell, offset and size) is read from the
        // pre-sort `sh`; a two-cell object's bottom-right cell is derived from its size here so it
        // can be re-fitted to the lines it spans after the sort.
        let objs: Vec<Anchor> = sh.object_anchors().copied().collect();
        let descs: Vec<(u32, u32, f32, f32)> = objs
            .iter()
            .map(|a| {
                let x2 = sh.col_left(a.cell.col) + a.dx as f64 + a.width.max(0.0) as f64;
                let y2 = sh.row_top(a.cell.row) + a.dy as f64 + a.height.max(0.0) as f64;
                let tc = sh.col_at(x2);
                let tr = sh.row_at(y2);
                (tc, tr, (x2 - sh.col_left(tc)) as f32, (y2 - sh.row_top(tr)) as f32)
            })
            .collect();
        let shm = cx.sheet_mut(sheet)?;
        for (c, cell) in &moved {
            match cell {
                Some(x) => shm.set_cell(*c, x.clone()),
                None => {
                    shm.remove_cell(*c);
                }
            }
        }
        // Every cell of the body is the destination of exactly one source cell, so clearing the
        // body and inserting the permuted entries is the same permutation the cells went through.
        for c in body.iter() {
            shm.comments.remove(&c);
            shm.hyperlinks.remove(&c);
        }
        for (at, cm) in side {
            shm.comments.insert(at, cm);
        }
        for (at, h) in links {
            shm.hyperlinks.insert(at, h);
        }
        shm.cell_pictures.extend(pictures);
        for (i, h) in heights.into_iter().enumerate() {
            let row = body.start.row + i as u32;
            match h {
                Some(info) => {
                    shm.rows.insert(row, LineInfo { hidden: shm.rows.get(&row).is_some_and(|x| x.hidden), ..info });
                }
                None => {
                    if let Some(e) = shm.rows.get_mut(&row) {
                        e.size = None;
                    }
                }
            }
        }
        // Re-anchor floating objects now that the row heights are final. `objs` was collected in
        // `object_anchors` order, which `object_anchors_mut` repeats.
        // Only objects anchored inside the sorted block move: within its lines on the sort axis
        // and within its span on the other axis (a picture beside the block stays put).
        let (b0, b1) = if by_cols { (body.start.col, body.end.col) } else { (body.start.row, body.end.row) };
        let (o0, o1) = if by_cols { (body.start.row, body.end.row) } else { (body.start.col, body.end.col) };
        let new_anchors: Vec<Anchor> = objs
            .into_iter()
            .zip(descs)
            .map(|(a, desc)| {
                let other = if by_cols { a.cell.row } else { a.cell.col };
                if other < o0 || other > o1 { a } else { sorted_anchor(shm, a, desc, by_cols, b0, b1, &dst_of) }
            })
            .collect();
        for (a, n) in shm.object_anchors_mut().zip(new_anchors) {
            *a = n;
        }
        for (c, _) in moved {
            cx.changed.push((sheet, c));
        }
        cx.structural = true;
        Ok(Json::Null)
    })?;
    Ok(json!({"sorted": body.a1()}))
}

/// Re-anchor one floating object for a sort. `bottom` is the object's bottom-right line and the
/// offset of that corner inside it, measured before the sort. `map` (via `dst_of`) translates a
/// line index on the moving axis and is the identity outside `b0..=b1`. `sh` is the post-sort
/// grid (row heights already reordered), so re-fitting uses the heights the object lands on.
/// Objects pinned absolutely ("Don't move or size with cells") never move; "move but don't size"
/// keeps its size; "move and size" is re-fitted when its whole extent stays inside the body, and
/// keeps its size when the sort turns its lines around (its bottom line now sorts before its top
/// line), since the lines then enclose nothing to stretch it between.
fn sorted_anchor(sh: &Sheet, a: Anchor, bottom: (u32, u32, f32, f32), by_cols: bool, b0: u32, b1: u32, dst_of: &[u32]) -> Anchor {
    if a.mode == AnchorMode::Absolute {
        return a;
    }
    let (tc, tr, tdx, tdy) = bottom;
    let map = |v: u32| -> u32 { if v >= b0 && v <= b1 { dst_of.get((v - b0) as usize).map_or(v, |&d| b0 + d) } else { v } };
    let (axis_start, axis_end) = if by_cols { (a.cell.col, tc) } else { (a.cell.row, tr) };
    if axis_start < b0 || axis_start > b1 {
        return a;
    }
    let (nr, nc) = if by_cols { (a.cell.row, map(a.cell.col)) } else { (map(a.cell.row), a.cell.col) };
    if a.mode == AnchorMode::MoveAndSize && axis_end >= b0 && axis_end <= b1 {
        let (nbr, nbc) = if by_cols { (tr, map(tc)) } else { (map(tr), tc) };
        let x1 = sh.col_left(nc) + a.dx as f64;
        let y1 = sh.row_top(nr) + a.dy as f64;
        let x2 = sh.col_left(nbc) + tdx as f64;
        let y2 = sh.row_top(nbr) + tdy as f64;
        // An extent that is not positive means the sort put the bottom-right line before the
        // top-left one on that axis: the object keeps its old size there.
        let fit = |extent: f64, old: f32| if extent > 0.0 { extent as f32 } else { old.max(0.0) };
        Anchor { cell: CellRef::new(nr, nc), dx: a.dx, dy: a.dy, width: fit(x2 - x1, a.width), height: fit(y2 - y1, a.height), mode: a.mode }
    } else {
        Anchor { cell: CellRef::new(nr, nc), ..a }
    }
}

/// Case-sensitive tie-break for text that is equal ignoring case: as in Excel, lowercase sorts
/// before uppercase at the first letter that differs in case.
fn case_order(a: &str, b: &str) -> Ordering {
    a.chars().zip(b.chars()).find(|(x, y)| x != y).map_or(Ordering::Equal, |(x, y)| y.is_lowercase().cmp(&x.is_lowercase()))
}

pub(super) fn sort_key(v: &Value) -> (u8, i64, String) {
    match v {
        Value::Number(n) => (0, (n * 1e6) as i64, String::new()),
        Value::Text(t) => (1, 0, t.to_lowercase()),
        Value::Bool(b) => (2, *b as i64, String::new()),
        Value::Empty => (9, 0, String::new()),
        _ => (3, 0, v.display()),
    }
}
