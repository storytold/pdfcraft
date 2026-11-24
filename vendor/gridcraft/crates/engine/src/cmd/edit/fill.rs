use super::{CellRef, EngineError, Json, RangeRef, Result, Session, bad, edit, f64_param, json, str_param, target_range, target_sheet};

// ---------------------------------------------------------------- fill

pub(super) fn fill_dir(s: &mut Session, p: &Json, dr: i64, dc: i64) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let mut r = target_range(s, p)?;
    // Whole rows or columns fill only where the sheet has content (nothing on an empty sheet).
    if r.is_full_cols() || r.is_full_rows() {
        match s.doc()?.wb.sheet(sheet).and_then(|sh| sh.used_range()).and_then(|u| r.intersection(&u)) {
            Some(u) => r = u,
            None => return Ok(Json::Null),
        }
    }
    if r.count() > crate::fill::MAX_FILL_CELLS {
        return Err(bad("edit.fill", "range too large"));
    }
    // With one row selected, Fill Down copies from the row above (Excel behaviour).
    if dr == 1 && r.height() == 1 {
        r.start.row = r.start.row.saturating_sub(1);
    }
    if dc == 1 && r.width() == 1 {
        r.start.col = r.start.col.saturating_sub(1);
    }
    let src = if dr == 1 {
        RangeRef::new(r.start, CellRef::new(r.start.row, r.end.col))
    } else if dr == -1 {
        RangeRef::new(CellRef::new(r.end.row, r.start.col), r.end)
    } else if dc == 1 {
        RangeRef::new(r.start, CellRef::new(r.end.row, r.start.col))
    } else {
        RangeRef::new(CellRef::new(r.start.row, r.end.col), r.end)
    };
    let skip = s.doc()?.wb.sheet(sheet).map(|sh| super::filtered_rows(sh, r)).unwrap_or_default();
    edit(s, |cx| {
        crate::fill::fill_except(cx, sheet, src, r, crate::fill::FillMode::Copy, &skip)?;
        Ok(Json::Null)
    })
}

pub(super) fn auto_fill(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let src = str_param(p, "source").and_then(RangeRef::parse).map_or_else(|| target_range(s, p), Ok)?;
    let Some(target) = str_param(p, "target").and_then(RangeRef::parse) else { return Err(bad("edit.autoFill", "missing `target`")) };
    let mode = match str_param(p, "mode").unwrap_or("series") {
        "copy" => crate::fill::FillMode::Copy,
        "alternate" => crate::fill::FillMode::Alternate,
        "formats" => crate::fill::FillMode::Formats,
        "values" => crate::fill::FillMode::ValuesOnly,
        "extend" => crate::fill::FillMode::Extend,
        "days" => crate::fill::FillMode::Dates(crate::fill::DateUnit::Day),
        "weekdays" => crate::fill::FillMode::Dates(crate::fill::DateUnit::Weekday),
        "months" => crate::fill::FillMode::Dates(crate::fill::DateUnit::Month),
        "years" => crate::fill::FillMode::Dates(crate::fill::DateUnit::Year),
        _ => crate::fill::FillMode::Series,
    };
    if target.count() > crate::fill::MAX_FILL_CELLS || src.count() > crate::fill::MAX_FILL_CELLS {
        return Err(bad("edit.autoFill", "range too large"));
    }
    // Excel refuses a target that doesn't hold the source in a corner. Such targets used to fill
    // every source lane across the whole target extent: billions of cells for whole rows (`2:3`)
    // into whole columns (`A:A`).
    if !crate::fill::holds_source_in_corner(src, target) {
        return Err(bad("edit.autoFill", "the target must contain the source in one of its corners"));
    }
    edit(s, |cx| {
        crate::fill::fill(cx, sheet, src, target, mode)?;
        *cx.sel = crate::Selection { active: src.start, anchor: src.start, ranges: vec![target.union(&src)] };
        Ok(Json::Null)
    })
}

pub(super) fn fill_series(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let r = target_range(s, p)?;
    let rows = str_param(p, "direction").map(|d| d == "rows").unwrap_or(r.width() > r.height());
    let kind = str_param(p, "type").unwrap_or("linear").to_string();
    let step = f64_param(p, "step").unwrap_or(1.0);
    let stop = f64_param(p, "stop");
    let unit = str_param(p, "dateUnit").unwrap_or("day").to_string();
    edit(s, |cx| {
        crate::fill::series(cx, sheet, crate::fill::SeriesOptions { range: r, rows, kind: &kind, step, stop, unit: &unit })?;
        Ok(Json::Null)
    })
}

pub(super) fn flash_fill(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let r = target_range(s, p)?;
    let n = edit(s, |cx| crate::fill::flash_fill(cx, sheet, r.start))?;
    if n == 0 {
        return Err(EngineError::Other(
            "We looked at all the data next to your selection and didn't see a pattern for filling in values for you.".into(),
        ));
    }
    Ok(json!({"filled": n}))
}
