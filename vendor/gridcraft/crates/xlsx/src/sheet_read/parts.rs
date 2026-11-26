use super::{
    AutoFilter, CellRef, Comment, Ctx, El, FilterCriterion, HashMap, MAX_COLS, MAX_ROWS, NoteBox, NoteCorner, RangeRef, Sheet, Table, TableColumn,
    TotalsFn, XmlColumnPr, from_file, xml,
};

/// `rels` maps relationship ids to relationships (the first of duplicate ids). `budget` counts the
/// cells and bytes this sheet's hyperlinks have copied so far, across `<hyperlinks>` blocks.
pub(super) fn from_craft_table(cx: &mut Ctx<'_>, t: &craft_xlsx::Table) -> Table {
    let columns = t
        .columns
        .iter()
        .map(|c| {
            let totals = match c.totals_row_function.as_deref().unwrap_or("none") {
                "average" => TotalsFn::Average,
                "count" => TotalsFn::Count,
                "countNums" => TotalsFn::CountNums,
                "max" => TotalsFn::Max,
                "min" => TotalsFn::Min,
                "sum" => TotalsFn::Sum,
                "stdDev" => TotalsFn::StdDev,
                "var" => TotalsFn::Var,
                "custom" => TotalsFn::Custom,
                _ => TotalsFn::None,
            };
            let totals_label =
                if totals == TotalsFn::Custom { c.totals_row_formula.as_ref().map(|(f, _)| from_file(f.trim())) } else { c.totals_row_label.clone() };
            let xml = c.other.iter().find(|e| e.name == "xmlColumnPr").and_then(|x| {
                Some(XmlColumnPr {
                    map_id: x.attr_u32("mapId")?,
                    xpath: x.attr("xpath")?.to_string(),
                    data_type: x.attr("xmlDataType").unwrap_or("string").to_string(),
                    denormalized: x.flag("denormalized", false),
                })
            });
            TableColumn {
                name: xml::decode_xstring(c.name.as_deref().unwrap_or("")),
                totals,
                totals_label,
                formula: c.calculated_formula.as_ref().map(|(f, _)| from_file(f.trim())),
                unique_name: c.unique_name.clone(),
                xml,
            }
        })
        .collect();
    let xml_table = t.table_type.as_deref() == Some("xml");
    let si = t.style.as_ref();
    let filter = t.auto_filter.as_ref().and_then(|af| read_autofilter(cx, &super::from_craft_el(af))).map(|af| af.criteria).unwrap_or_default();
    Table {
        id: t.id.unwrap_or(0),
        name: t.display_name.as_deref().or(t.name.as_deref()).unwrap_or("Table").to_string(),
        range: super::to_range_ref(t.range),
        header_row: t.header_row_count.unwrap_or(1) > 0,
        totals_row: t.totals_row_count.unwrap_or(0) > 0,
        columns,
        style: si.and_then(|s| s.name.as_deref()).unwrap_or("").to_string(),
        banded_rows: si.is_some_and(|s| s.show_row_stripes.unwrap_or(false)),
        banded_cols: si.is_some_and(|s| s.show_column_stripes.unwrap_or(false)),
        first_col: si.is_some_and(|s| s.show_first_column.unwrap_or(false)),
        last_col: si.is_some_and(|s| s.show_last_column.unwrap_or(false)),
        filter_button: t.auto_filter.is_some(),
        filter,
        table_type: xml_table.then(|| "xml".to_string()),
        connection_id: t.connection_id.filter(|_| xml_table),
    }
}

pub(super) fn apply_craft_comments(comments: &craft_xlsx::Comments, sheet: &mut Sheet) {
    for c in &comments.comments {
        let cell = CellRef::new(c.at.row, c.at.col);
        let author = comments.author(c).unwrap_or("").to_string();
        let mut text = c.text.text.to_string();
        if !author.is_empty()
            && let Some(rest) = text.strip_prefix(&format!("{author}:"))
        {
            text = rest.strip_prefix('\n').unwrap_or(rest).to_string();
        }
        sheet
            .comments
            .insert(cell, Comment { author, text, replies: vec![], threaded: false, resolved: false, visible: false, dates: vec![], note_box: None });
    }
}

pub(super) fn apply_craft_note_shapes(shapes: &[craft_xlsx::NoteShape], sheet: &mut Sheet) {
    for s in shapes {
        let cell = CellRef::new(s.at.row, s.at.col);
        let Some(c) = sheet.comments.get_mut(&cell) else { continue };
        if s.visible && !c.threaded {
            c.visible = true;
        }
        if let Some([c1, dx1, r1, dy1, c2, dx2, r2, dy2]) = s.anchor
            && c1.max(c2) < MAX_COLS
            && r1.max(r2) < MAX_ROWS
        {
            let corner = |col: u32, col_px: u32, row: u32, row_px: u32| NoteCorner {
                col: i64::from(col) - i64::from(cell.col),
                col_px,
                row: i64::from(row) - i64::from(cell.row),
                row_px,
            };
            let nb = NoteBox { from: corner(c1, dx1, r1, dy1), to: corner(c2, dx2, r2, dy2), size_pt: s.size_pt };
            c.note_box = (nb != crate::sheet_write::DEFAULT_NOTE_BOX).then_some(nb);
        }
    }
}

fn filter_op(s: Option<&str>) -> &'static str {
    match s.unwrap_or("equal") {
        "lessThan" => "<",
        "lessThanOrEqual" => "<=",
        "notEqual" => "<>",
        "greaterThanOrEqual" => ">=",
        "greaterThan" => ">",
        _ => "=",
    }
}

pub fn read_autofilter(cx: &mut Ctx<'_>, e: &El) -> Option<AutoFilter> {
    let range = e.attr("ref").and_then(RangeRef::parse)?;
    let mut criteria = vec![];
    for fc in e.kids("filterColumn") {
        let Some(col) = fc.attr_u32("colId") else { continue };
        let crit = if let Some(f) = fc.child("filters") {
            FilterCriterion::Values {
                values: f.kids("filter").filter_map(|x| x.attr("val")).map(str::to_string).collect(),
                blanks: f.flag("blank", false),
            }
        } else if let Some(cf) = fc.child("customFilters") {
            let mut it = cf.kids("customFilter").map(|c| (filter_op(c.attr("operator")).to_string(), c.attr("val").unwrap_or("").to_string()));
            let Some(a) = it.next() else { continue };
            FilterCriterion::Custom { a, b: it.next(), and: cf.flag("and", false) }
        } else if let Some(t) = fc.child("top10") {
            FilterCriterion::Top10 {
                bottom: !t.flag("top", true),
                percent: t.flag("percent", false),
                count: t.attr_f64("val").unwrap_or(10.0).clamp(0.0, 1e9) as u32,
            }
        } else if let Some(d) = fc.child("dynamicFilter") {
            match d.attr("type") {
                Some("aboveAverage") => FilterCriterion::AboveAverage(true),
                Some("belowAverage") => FilterCriterion::AboveAverage(false),
                _ => {
                    cx.warn("date filters in AutoFilters are not supported");
                    continue;
                }
            }
        } else if let Some(c) = fc.child("colorFilter") {
            let Some(dxf) = c.attr_u32("dxfId").and_then(|i| cx.dxfs.get(i as usize)) else { continue };
            if c.flag("cellColor", true) { FilterCriterion::FillColor(dxf.fill.fg) } else { FilterCriterion::FontColor(dxf.font.color) }
        } else if let Some(i) = fc.child("iconFilter") {
            let Some(set) = i.attr("iconSet") else { continue };
            FilterCriterion::Icon { set: set.to_string(), id: i.attr_u32("iconId") }
        } else {
            continue;
        };
        criteria.push((col, crit));
    }
    Some(AutoFilter { range, criteria })
}

pub(super) fn read_threaded(cx: &Ctx<'_>, x: &El, sheet: &mut Sheet) {
    let mut by_id: HashMap<String, CellRef> = HashMap::new();
    let mut replies: Vec<(String, String, String, String)> = vec![];
    for t in x.kids("threadedComment") {
        let Some(cell) = t.attr("ref").and_then(CellRef::parse) else { continue };
        let author = t.attr("personId").and_then(|p| cx.persons.get(p)).cloned().unwrap_or_default();
        let text = t.child("text").map(|e| e.text.clone()).unwrap_or_default();
        let date = t.attr("dT").unwrap_or("").to_string();
        match t.attr("parentId") {
            Some(p) => replies.push((p.to_string(), author, text, date)),
            None => {
                if let Some(id) = t.attr("id") {
                    by_id.insert(id.to_string(), cell);
                }
                let dates = if date.is_empty() { vec![] } else { vec![date] };
                sheet.comments.insert(
                    cell,
                    Comment { author, text, replies: vec![], threaded: true, resolved: t.flag("done", false), visible: false, dates, note_box: None },
                );
            }
        }
    }
    // Replies in file order; ones whose thread is missing are dropped.
    for (parent, author, text, date) in replies {
        if let Some(c) = by_id.get(&parent)
            && let Some(cm) = sheet.comments.get_mut(c)
        {
            if !date.is_empty() {
                // `dates[0]` is the thread, `dates[i + 1]` reply i; unknown dates stay empty.
                let at = cm.replies.len() + 1;
                if cm.dates.len() < at {
                    cm.dates.resize(at, String::new());
                }
                cm.dates.push(date);
            }
            cm.replies.push((author, text));
        }
    }
}
