use super::{
    CellRef, EngineError, Json, LineInfo, RangeRef, Result, Session, bad, bool_param, col_param, data_target, edit, json, outline, parse_lines,
    str_param, target_sheet,
};

// ---------------------------------------------------------------- outline

fn outline_lines(s: &Session, p: &Json) -> Result<(bool, u32, u32)> {
    if let Some(r) = str_param(p, "rows").and_then(parse_lines) {
        return Ok((true, r.start.row, r.end.row));
    }
    if let Some(r) = str_param(p, "cols").and_then(parse_lines) {
        return Ok((false, r.start.col, r.end.col));
    }
    let r = s.doc()?.selection.current();
    match str_param(p, "by") {
        Some("rows") => Ok((true, r.start.row, r.end.row)),
        Some("cols" | "columns") => Ok((false, r.start.col, r.end.col)),
        _ if r.is_full_cols() && !r.is_full_rows() => Ok((false, r.start.col, r.end.col)),
        _ => Ok((true, r.start.row, r.end.row)),
    }
}

/// Outline commands change line visibility and grouping: a protected sheet must allow formatting that axis.
fn refuse_unless_outline_allowed(s: &Session, sheet: usize, rows: bool) -> Result<()> {
    if rows {
        crate::cmd::refuse_unless_allowed(s, sheet, "Formatting rows", |p| p.format_rows)
    } else {
        crate::cmd::refuse_unless_allowed(s, sheet, "Formatting columns", |p| p.format_columns)
    }
}

pub(super) fn group(s: &mut Session, p: &Json, on: bool) -> Result<Json> {
    let si = target_sheet(s, p)?;
    // The selection belongs to the active sheet, so another sheet needs its lines.
    if si != s.doc()?.wb.active_sheet() && p.get("rows").is_none() && p.get("cols").is_none() {
        return Err(bad(if on { "data.group" } else { "data.ungroup" }, "missing `rows` or `cols`: the selection is on the active sheet"));
    }
    let (rows, a, b) = outline_lines(s, p)?;
    refuse_unless_outline_allowed(s, si, rows)?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(si)?;
        let before = outline::summary_before(sh, rows);
        let map = if rows { &mut sh.rows } else { &mut sh.cols };
        let b = b.min(a.saturating_add(1_048_575));
        for i in a..=b {
            let e = map.entry(i).or_default();
            e.outline = if on { (e.outline + 1).min(outline::MAX_LEVEL) } else { e.outline.saturating_sub(1) };
            if !on && e.outline == 0 {
                e.hidden = false;
            }
        }
        // A summary line whose group is gone has nothing left to collapse.
        let summary = if before { a.checked_sub(1) } else { b.checked_add(1) };
        if !on
            && !outline::groups_with(map, before).iter().any(|g| if before { g.start == a } else { g.end == b })
            && let Some(e) = summary.and_then(|i| map.get_mut(&i))
        {
            e.collapsed = false;
        }
        cx.structural = true; // ungrouping reveals hidden rows: SUBTOTAL(10x) depends on it
        Ok(Json::Null)
    })
}

/// Hide Detail / Show Detail: the innermost group (or the one at `level`) that holds the lines, or
/// failing that the one they are the summary of. Hiding marks the summary line collapsed (the
/// + button, saved to XLSX); showing keeps inner groups that were collapsed on their own collapsed.
pub(super) fn detail(s: &mut Session, p: &Json, hide: bool) -> Result<Json> {
    let (rows, a, b) = outline_lines(s, p)?;
    refuse_unless_outline_allowed(s, s.doc()?.wb.active_sheet(), rows)?;
    let want = p.get("level").and_then(Json::as_u64).map(|l| l.min(outline::MAX_LEVEL as u64) as u8);
    edit(s, |cx| {
        let si = cx.sheet_index();
        let sh = cx.sheet_mut(si)?;
        let before = outline::summary_before(sh, rows);
        let map = if rows { &mut sh.rows } else { &mut sh.cols };
        let all = outline::groups_with(map, before);
        let level_ok = |g: &outline::OutlineGroup| want.is_none_or(|l| g.level == l);
        let pick = |hit: &dyn Fn(&outline::OutlineGroup) -> bool| -> Vec<outline::OutlineGroup> {
            let hits: Vec<_> = all.iter().filter(|g| level_ok(g) && hit(g)).copied().collect();
            let deepest = hits.iter().map(|g| g.level).max().unwrap_or(0);
            hits.into_iter().filter(|g| g.level == deepest).collect()
        };
        let mut chosen = pick(&|g| g.start <= b && g.end >= a);
        if chosen.is_empty() {
            chosen = pick(&|g| g.has_summary() && g.summary() >= a && g.summary() <= b);
        }
        for g in chosen {
            // Inner groups collapsed on their own (their summary marked), before this one changes.
            let inner: Vec<outline::OutlineGroup> = all
                .iter()
                .filter(|o| o.level > g.level && o.start >= g.start && o.end <= g.end && o.has_summary() && o.summary() != g.summary())
                .filter(|o| map.get(&o.summary()).is_some_and(|e| e.collapsed))
                .copied()
                .collect();
            for i in g.start..=g.end {
                if let Some(e) = map.get_mut(&i) {
                    set_hidden(e, hide || inner.iter().any(|o| o.contains(i)));
                }
            }
            if !g.has_summary() {
                continue;
            }
            if hide {
                map.entry(g.summary()).or_default().collapsed = true;
            } else if let Some(e) = map.get_mut(&g.summary()) {
                e.collapsed = false;
            }
        }
        cx.structural = true;
        Ok(Json::Null)
    })
}

/// Hides or shows an outline line. A line shown with a zero size (files that hide columns as
/// `width="0" hidden="1"`) gets the default size back, as Unhide does; otherwise + would mark the
/// group expanded and leave it invisible (#1714).
fn set_hidden(e: &mut LineInfo, hidden: bool) {
    e.hidden = hidden;
    if !hidden && e.size == Some(0.0) {
        e.size = None;
    }
}

/// The outline's level buttons: show rows / columns down to outline level `n` (1 = only the
/// top-level summaries); deeper detail is hidden and its summary lines marked collapsed.
pub(super) fn show_levels(s: &mut Session, p: &Json) -> Result<Json> {
    let level = |k: &str| p.get(k).and_then(Json::as_u64).map(|n| n.clamp(1, outline::MAX_LEVEL as u64 + 1) as u8);
    let (rows, cols) = (level("rows"), level("cols"));
    if rows.is_none() && cols.is_none() {
        return Err(EngineError::BadParams {
            cmd: "data.showLevels".into(),
            msg: "give `rows` or `cols`: the outline level to show (1 = top-level summaries only)".into(),
        });
    }
    let active = s.doc()?.wb.active_sheet();
    for (n, is_rows) in [(rows, true), (cols, false)] {
        if n.is_some() {
            refuse_unless_outline_allowed(s, active, is_rows)?;
        }
    }
    edit(s, |cx| {
        let si = cx.sheet_index();
        let sh = cx.sheet_mut(si)?;
        let before = (outline::summary_before(sh, true), outline::summary_before(sh, false));
        for (map, n, before) in [(&mut sh.rows, rows, before.0), (&mut sh.cols, cols, before.1)] {
            let Some(n) = n else { continue };
            let groups = outline::groups_with(map, before);
            for e in map.values_mut().filter(|e| e.outline > 0) {
                set_hidden(e, e.outline >= n);
            }
            // Inner groups come first, so a summary line shared with the group around it ends up
            // as that outer group says.
            for g in groups.into_iter().filter(|g| g.has_summary()) {
                map.entry(g.summary()).or_default().collapsed = g.level >= n;
            }
        }
        cx.structural = true;
        Ok(Json::Null)
    })
}

/// Data ▸ Outline ▸ Settings: whether summary rows sit below their detail and summary columns
/// right of it. A changed direction moves each collapsed group's mark (+) to its new summary line.
/// Moves the collapsed marks of a line axis to the summary line on its new side (`on` is the new
/// setting, `was_before` the old one). A line that nested groups share keeps its mark on the outermost
/// group whose detail is all hidden, or the outermost when none is.
fn move_marks(map: &mut std::collections::BTreeMap<u32, LineInfo>, was_before: bool, on: bool) {
    let mut marked: Vec<outline::OutlineGroup> =
        outline::groups_with(map, was_before).into_iter().filter(|g| g.has_summary() && map.get(&g.summary()).is_some_and(|e| e.collapsed)).collect();
    // A mark on a line that nested groups share moves with the outermost of them whose
    // detail is all hidden, or with the outermost when none is.
    let mut owner: std::collections::BTreeMap<u32, (bool, u8)> = std::collections::BTreeMap::new();
    for g in &marked {
        let key = (!g.collapsed, g.level);
        let best = owner.entry(g.summary()).or_insert(key);
        *best = (*best).min(key);
    }
    marked.retain(|g| owner.get(&g.summary()) == Some(&(!g.collapsed, g.level)));
    for g in &marked {
        if let Some(e) = map.get_mut(&g.summary()) {
            e.collapsed = false;
        }
    }
    for g in marked {
        let g = outline::OutlineGroup { summary_before: !on, ..g };
        if g.has_summary() {
            map.entry(g.summary()).or_default().collapsed = true;
        }
    }
}

pub(super) fn outline_settings(s: &mut Session, p: &Json) -> Result<Json> {
    let si = target_sheet(s, p)?;
    edit(s, |cx| {
        let sh = cx.sheet_mut(si)?;
        for (rows, key) in [(true, "summaryBelow"), (false, "summaryRight")] {
            let Some(on) = bool_param(p, key) else { continue };
            let was_before = outline::summary_before(sh, rows);
            if rows {
                sh.summary_below = on;
            } else {
                sh.summary_right = on;
            }
            if was_before != on {
                continue;
            }
            move_marks(if rows { &mut sh.rows } else { &mut sh.cols }, was_before, on);
        }
        Ok(json!({"summaryBelow": sh.summary_below, "summaryRight": sh.summary_right}))
    })
}

pub(super) fn subtotal(s: &mut Session, p: &Json) -> Result<Json> {
    let (sheet, r) = data_target(s, p, "data.subtotal")?;
    let header = bool_param(p, "header").unwrap_or(true);
    let group_col = col_param(p.get("groupBy"), r.start.col).unwrap_or(r.start.col);
    let func = str_param(p, "function").unwrap_or("sum");
    let code = match func {
        "count" => 3,
        "average" => 1,
        "max" => 4,
        "min" => 5,
        "product" => 6,
        _ => 9,
    };
    let cols: Vec<u32> = p
        .get("columns")
        .and_then(Json::as_array)
        .map(|a| a.iter().filter_map(|c| col_param(Some(c), r.start.col)).collect())
        .unwrap_or_else(|| vec![r.end.col]);
    let label = match func {
        "count" => "Count",
        "average" => "Average",
        "max" => "Max",
        "min" => "Min",
        "product" => "Product",
        _ => "Total",
    };
    // Group boundaries (header in the first row).
    let d = s.doc()?;
    let sh = d.wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    let mut groups: Vec<(u32, u32, String)> = Vec::new();
    let first = r.start.row + u32::from(header);
    let mut start = first;
    for row in first..=r.end.row + 1 {
        let cur = sh.value(CellRef::new(row, group_col)).display();
        let prev = sh.value(CellRef::new(start, group_col)).display();
        if row > r.end.row || cur != prev {
            groups.push((start, row - 1, prev));
            start = row;
        }
    }
    // The totals go below each group, so the outline puts summary rows below their detail (the
    // Subtotal dialog's "Summary below data"), in the same undo step.
    if !sh.summary_below {
        s.execute("data.outlineSettings", json!({"sheet": sheet, "summaryBelow": true}))?;
    }
    // Insert a total row after each group, bottom-up so indices stay valid.
    let mut inserted = 0;
    for (g0, g1, name) in groups.iter().rev() {
        s.execute("home.insertRows", json!({"sheet": sheet, "rows": format!("{}:{}", g1 + 2, g1 + 2)}))?;
        let row = g1 + 1;
        let lbl = CellRef::new(row, group_col).a1();
        s.execute("cell.set", json!({"sheet": sheet, "cell": lbl, "input": format!("{name} {label}")}))?;
        for c in &cols {
            let range = RangeRef::new(CellRef::new(*g0, *c), CellRef::new(*g1, *c)).a1();
            s.execute("cell.set", json!({"sheet": sheet, "cell": CellRef::new(row, *c).a1(), "input": format!("=SUBTOTAL({code},{range})")}))?;
        }
        s.execute(
            "home.bold",
            json!({"sheet": sheet, "range": RangeRef::new(CellRef::new(row, r.start.col), CellRef::new(row, r.end.col)).a1(), "on": true}),
        )?;
        s.execute("data.group", json!({"sheet": sheet, "rows": format!("{}:{}", g0 + 1, g1 + 1)}))?;
        inserted += 1;
    }
    let last = r.end.row + inserted + 1;
    let lbl = CellRef::new(last, group_col).a1();
    s.execute("home.insertRows", json!({"sheet": sheet, "rows": format!("{}:{}", last + 1, last + 1)}))?;
    s.execute("cell.set", json!({"sheet": sheet, "cell": lbl, "input": format!("Grand {label}")}))?;
    for c in &cols {
        let range = RangeRef::new(CellRef::new(first, *c), CellRef::new(last - 1, *c)).a1();
        s.execute("cell.set", json!({"sheet": sheet, "cell": CellRef::new(last, *c).a1(), "input": format!("=SUBTOTAL({code},{range})")}))?;
    }
    // One outer group over every data and subtotal row, summarised by the grand total, as in
    // Excel: outline level 1 then shows only the grand total, and level 2 the subtotals.
    s.execute("data.group", json!({"sheet": sheet, "rows": format!("{}:{}", first + 1, last)}))?;
    Ok(json!({"groups": inserted}))
}
