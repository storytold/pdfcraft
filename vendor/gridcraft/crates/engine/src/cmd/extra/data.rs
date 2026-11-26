use super::{CellRef, EngineError, Json, RangeRef, Result, Session, TableColumn, TotalsFn, Value, bad, edit, json, ok, str_param, target_range};

pub(super) fn table_resize(s: &mut Session, p: &Json) -> Result<Json> {
    let r = str_param(p, "range").and_then(RangeRef::parse).ok_or_else(|| bad("table.resize", "missing `range`"))?;
    let d = s.doc()?;
    let si = d.wb.active_sheet();
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let ti = match str_param(p, "table") {
        Some(n) => sh.tables.iter().position(|t| t.name.eq_ignore_ascii_case(n)),
        None => sh.tables.iter().position(|t| t.range.contains(d.selection.active)),
    }
    .ok_or_else(|| bad("table.resize", "select a cell in a table"))?;
    edit(s, |cx| {
        let sheet = cx.sheet_mut(si)?;
        let headers: Vec<String> = (r.start.col..=r.end.col).map(|c| sheet.value(CellRef::new(r.start.row, c)).display()).collect();
        if sheet.tables.iter().enumerate().any(|(i, o)| i != ti && o.range.intersects(&r)) {
            return Err(EngineError::Other("A table can't overlap another table.".into()));
        }
        let t = sheet.tables.get_mut(ti).ok_or_else(|| bad("table", "gone"))?;
        if r.start.row != t.range.start.row || !r.intersects(&t.range) {
            return Err(EngineError::Other(
                "The headers must remain in the same row, and the resulting table range must overlap the original table range.".into(),
            ));
        }
        let mut cols = Vec::new();
        for (i, h) in headers.iter().enumerate() {
            let name = if h.is_empty() { format!("Column{}", i + 1) } else { h.clone() };
            let existing = t.columns.iter().find(|c| c.name.eq_ignore_ascii_case(&name)).cloned();
            cols.push(existing.unwrap_or(TableColumn {
                name,
                totals: TotalsFn::None,
                totals_label: None,
                formula: None,
                unique_name: None,
                xml: None,
            }));
        }
        t.columns = cols;
        t.range = r;
        cx.structural = true;
        Ok(Json::Null)
    })
}

/// Most cells (rows × columns, blanks included) Get Data imports. The rows are built densely
/// before they are written, so a few sparse records or one far-away CSV cell must not expand
/// into a rectangle of billions of slots.
const MAX_IMPORT_CELLS: u64 = 10_000_000;

fn too_many_cells() -> EngineError {
    EngineError::Other(format!("The data is too large to import ({MAX_IMPORT_CELLS} cells at most)."))
}

pub(super) fn from_csv(s: &mut Session, p: &Json) -> Result<Json> {
    let bytes = if let Some(t) = str_param(p, "text") {
        t.as_bytes().to_vec()
    } else if let Some(path) = str_param(p, "path") {
        crate::io::read_file(path)?
    } else {
        s.ui_requests.push(crate::UiRequest::Dialog("open".into(), json!({})));
        return ok();
    };
    let text = String::from_utf8_lossy(&bytes);
    // JSON array of objects → rows.
    let rows: Vec<Vec<Json>> =
        if text.trim_start().starts_with('[') {
            let arr: Vec<Json> = serde_json::from_str(&text).map_err(|e| bad("data.getData", e.to_string()))?;
            let mut keys: Vec<String> = Vec::new();
            let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for o in &arr {
                if let Some(m) = o.as_object() {
                    for k in m.keys() {
                        if !seen.contains(k.as_str()) {
                            // Stop collecting at the column limit.
                            if keys.len() >= gridcraft_core::MAX_COLS as usize {
                                return Err(EngineError::Other("The data has more fields than a sheet has columns.".into()));
                            }
                            seen.insert(k.as_str());
                            keys.push(k.clone());
                        }
                    }
                }
            }
            if arr.len() as u64 + 1 > u64::from(gridcraft_core::MAX_ROWS) {
                return Err(EngineError::Other("The data has more rows than a sheet has.".into()));
            }
            if (arr.len() as u64 + 1).saturating_mul(keys.len().max(1) as u64) > MAX_IMPORT_CELLS {
                return Err(too_many_cells());
            }
            let mut out = vec![keys.iter().map(|k| json!(k)).collect::<Vec<_>>()];
            for o in &arr {
                out.push(keys.iter().map(|k| o.get(k).cloned().unwrap_or(Json::Null)).collect());
            }
            out
        } else {
            let delim = str_param(p, "delimiter")
                .and_then(|d| d.chars().next())
                .unwrap_or(if text.lines().next().is_some_and(|l| l.contains('\t')) { '\t' } else { ',' });
            let opts = gridcraft_xlsx::CsvOptions { delimiter: delim as u8, max_used_cells: Some(MAX_IMPORT_CELLS), ..Default::default() };
            // The cell limit is enforced while parsing, before an oversized file is stored.
            let wb = gridcraft_xlsx::read_csv(&bytes, &opts).map_err(|e| match e {
                gridcraft_xlsx::IoError::TooLarge(_) => too_many_cells(),
                e => EngineError::Other(e.to_string()),
            })?;
            let sh = wb.sheet(0).ok_or(EngineError::NoDocument)?;
            let Some(u) = sh.used_range() else { return Err(EngineError::Other("The file is empty.".into())) };
            if u.count() > MAX_IMPORT_CELLS {
                return Err(too_many_cells());
            }
            (u.start.row..=u.end.row)
                .map(|r| {
                    (u.start.col..=u.end.col)
                        .map(|c| match sh.value(CellRef::new(r, c)) {
                            Value::Number(n) => json!(n),
                            Value::Bool(b) => json!(b),
                            Value::Empty => Json::Null,
                            v => json!(v.display()),
                        })
                        .collect()
                })
                .collect()
        };
    if rows.is_empty() || rows.len() > 1_048_576 {
        return Err(EngineError::Other("Nothing to import.".into()));
    }
    let name = str_param(p, "sheet").map(str::to_string).or_else(|| {
        str_param(p, "path").and_then(|x| std::path::Path::new(x).file_stem().and_then(std::ffi::OsStr::to_str).map(|f| f.chars().take(31).collect()))
    });
    let r = s.execute("home.insertSheet", json!({"name": name})).or_else(|_| s.execute("home.insertSheet", json!({})))?;
    let w = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    s.execute("range.setValues", json!({"range": "A1", "values": rows}))?;
    let range = RangeRef::new(CellRef::new(0, 0), CellRef::new(rows.len() as u32 - 1, w as u32 - 1));
    let _ = s.execute("insert.table", json!({"range": range.a1(), "header": true}));
    let _ = s.execute("home.autofitColumnWidth", json!({"cols": RangeRef::cols(0, w as u32 - 1).a1()}));
    Ok(json!({"sheet": r["name"], "rows": rows.len(), "cols": w}))
}

pub(super) fn from_table_range(s: &mut Session, p: &Json) -> Result<Json> {
    let r = target_range(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let r = if r.is_single() { crate::selection::current_region(sh, r.start) } else { r };
    let text = crate::display::range_text(&d.wb, d.wb.active_sheet(), r);
    from_csv(s, &json!({"text": text, "delimiter": "\t", "sheet": format!("{} (2)", sh.name).chars().take(31).collect::<String>()}))
}

pub(super) fn queries(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc()?;
    Ok(Json::Array(
        d.wb.sheets
            .iter()
            .flat_map(|sh| {
                sh.tables
                    .iter()
                    .map(move |t| json!({"name": t.name, "sheet": sh.name, "range": t.range.a1(), "rows": t.range.height().saturating_sub(1)}))
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn s() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        s
    }

    #[test]
    fn sparse_imports_are_refused_before_their_rows_are_built() {
        let mut s = s();
        let sheets = s.doc().unwrap().wb.sheets.len();
        // 5,000 records with one field each: 5,001 × 5,000 slots once every record gets every field.
        let records: Vec<_> = (0..5_000).map(|i| json!({ format!("k{i}"): i })).collect();
        let text = serde_json::to_string(&records).unwrap();
        let err = s.execute("data.getData", json!({"text": text})).expect_err("too many cells");
        assert!(err.to_string().contains("too large"), "{err}");
        // A CSV whose last line reaches 16,001 columns, 1,000 lines down.
        let text = format!("1{}{}x", "\n".repeat(999), ",".repeat(16_000));
        let err = s.execute("data.fromTextCsv", json!({"text": text})).expect_err("too many cells");
        assert!(err.to_string().contains("too large"), "{err}");
        assert_eq!(s.doc().unwrap().wb.sheets.len(), sheets);
        // Small imports still work.
        let out = s.execute("data.getData", json!({"text": r#"[{"a": 1}, {"b": 2}]"#})).unwrap();
        assert_eq!((out["rows"].as_u64(), out["cols"].as_u64()), (Some(3), Some(2)), "{out}");
    }
}
