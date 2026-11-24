//! View tab (sheet view state that's saved with the file) and Page Layout settings.

use gridcraft_core::RangeRef;
use gridcraft_model::Orientation;
use serde_json::{Value as Json, json};

use super::{
    CommandSpec, EngineError, Result, Session, always, bad, bool_param, cell_param, cell_param_on, cmd, edit, f64_param, has_doc, str_param,
    target_range, target_sheet, u32_param,
};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "view.freezePanes",
            "Freeze Panes",
            ["View", "Window", "Freeze Panes"],
            None,
            "{sheet?, cell?: \"B2\" (rows above and columns left of it freeze); without `cell`, unfreezes a frozen sheet}",
            has_doc,
            freeze_panes
        ),
        cmd!("view.freezeTopRow", "Freeze Top Row", ["View", "Window", "Freeze Panes"], None, "{sheet?}", has_doc, |s, p| set_freeze(
            s,
            target_sheet(s, p)?,
            Some((1, 0))
        )),
        cmd!("view.freezeFirstColumn", "Freeze First Column", ["View", "Window", "Freeze Panes"], None, "{sheet?}", has_doc, |s, p| set_freeze(
            s,
            target_sheet(s, p)?,
            Some((0, 1))
        )),
        cmd!("view.unfreezePanes", "Unfreeze Panes", ["View", "Window", "Freeze Panes"], None, "{sheet?}", has_doc, |s, p| set_freeze(
            s,
            target_sheet(s, p)?,
            None
        )),
        cmd!("view.gridlines", "Gridlines", ["View", "Show"], None, "{on?}", has_doc, |s, p| sheet_flag(s, p, "gridlines")),
        cmd!("view.headings", "Headings", ["View", "Show"], None, "{on?}", has_doc, |s, p| sheet_flag(s, p, "headings")),
        cmd!("view.showZeros", "Show Zeros", [], None, "{on?}", has_doc, |s, p| sheet_flag(s, p, "zeros")),
        cmd!("view.showOutlineSymbols", "Show Outline Symbols", [], Some("Ctrl+8"), "{on?}", has_doc, |s, p| sheet_flag(s, p, "outlineSymbols")),
        cmd!("view.showObjects", "Show Objects", [], Some("Ctrl+6"), "{on?}", has_doc, show_objects),
        cmd!(
            "view.showSheetTabs",
            "Show Sheet Tabs",
            [],
            None,
            "{on?} (File › Options › Advanced, this workbook; toggles without `on`)",
            has_doc,
            |s, p| { workbook_view_flag(s, p, "sheetTabs") }
        ),
        cmd!(
            "view.showHorizontalScrollBar",
            "Show Horizontal Scroll Bar",
            [],
            None,
            "{on?} (this workbook; toggles without `on`)",
            has_doc,
            |s, p| { workbook_view_flag(s, p, "horizontal") }
        ),
        cmd!("view.showVerticalScrollBar", "Show Vertical Scroll Bar", [], None, "{on?} (this workbook; toggles without `on`)", has_doc, |s, p| {
            workbook_view_flag(s, p, "vertical")
        }),
        cmd!("view.rightToLeft", "Sheet Right-to-Left", ["Page Layout", "Sheet Options"], None, "{on?}", has_doc, |s, p| sheet_flag(s, p, "rtl")),
        cmd!("view.zoom", "Zoom", ["View", "Zoom"], None, "{percent: 10..400}", has_doc, zoom),
        cmd!("view.zoom100", "100%", ["View", "Zoom"], None, "{}", has_doc, |s, _| zoom(s, &json!({"percent": 100}))),
        cmd!(query "view.formulaBar", "Formula Bar", ["View", "Show"], None, "{on?: bool}", always, |s, p| {
            s.prefs.formula_bar = bool_param(p, "on").unwrap_or(!s.prefs.formula_bar);
            Ok(json!({"on": s.prefs.formula_bar}))
        }),
        cmd!(
            query "view.statusBar",
            "Customize Status Bar",
            [],
            None,
            "{average?, count?, numericalCount?, minimum?, maximum?, sum?: bool (the statistics shown for a selection; returns all six)}",
            always,
            status_bar
        ),
        cmd!("view.zoomToSelection", "Zoom to Selection", ["View", "Zoom"], None, "{viewWidth?, viewHeight?}", has_doc, zoom_selection),
        cmd!("pageLayout.orientation", "Orientation", ["Page Layout", "Page Setup"], None, "{orientation: portrait|landscape}", has_doc, orientation),
        cmd!(
            "pageLayout.size",
            "Size",
            ["Page Layout", "Page Setup"],
            None,
            "{paper: Letter|Legal|A4|A3|A5|A6|Tabloid|Executive|Ledger|B4|B5|B6|B4 (ISO)|B5 (ISO)|B6 (ISO)|Japanese Postcard|Japanese Double Postcard|Japanese Envelope #3|Japanese Envelope #4|Japanese Envelope Kaku #2|Japanese Envelope Kaku #3 (B4/B5/B6 are the JIS sizes)}",
            has_doc,
            paper
        ),
        cmd!(
            "pageLayout.margins",
            "Margins",
            ["Page Layout", "Page Setup"],
            None,
            "{preset?: normal|wide|narrow, left?, right?, top?, bottom?, header?, footer? (inches)}",
            has_doc,
            margins
        ),
        cmd!(
            "pageLayout.printArea",
            "Set Print Area",
            ["Page Layout", "Page Setup", "Print Area"],
            None,
            "{range?, clear?: bool}",
            has_doc,
            print_area
        ),
        cmd!(
            "pageLayout.printTitles",
            "Print Titles",
            ["Page Layout", "Page Setup"],
            None,
            "{rows?: \"1:1\", cols?: \"A:A\"}",
            has_doc,
            print_titles
        ),
        cmd!(
            "pageLayout.scaleToFit",
            "Scale to Fit",
            ["Page Layout", "Scale to Fit"],
            None,
            "{scale?: 100, width?: pages, height?: pages}",
            has_doc,
            scale_fit
        ),
        cmd!("pageLayout.printGridlines", "Print Gridlines", ["Page Layout", "Sheet Options"], None, "{on?}", has_doc, |s, p| sheet_flag(
            s,
            p,
            "printGridlines"
        )),
        cmd!("pageLayout.printHeadings", "Print Headings", ["Page Layout", "Sheet Options"], None, "{on?}", has_doc, |s, p| sheet_flag(
            s,
            p,
            "printHeadings"
        )),
        cmd!(
            "pageLayout.breaks",
            "Breaks",
            ["Page Layout", "Page Setup"],
            None,
            "{insert?: bool, remove?: bool, reset?: bool, cell?}",
            has_doc,
            breaks
        ),
        cmd!(
            "pageLayout.headerFooter",
            "Header & Footer",
            ["Insert", "Text"],
            None,
            "{sheet?, header?: \"&C&P\", footer?: \"&CPage &P of &N\", differentFirst?: bool, differentOddEven?: bool, scaleWithDoc?: bool, alignWithMargins?: bool, firstHeader?, firstFooter?, evenHeader?, evenFooter?} or one section: {sheet?, page?: odd|even|first, part: header|footer, section: left|center|right, text: \"Page &[Page] of &[Pages]\"} (field names or codes) → {header, footer, differentFirst, differentOddEven, scaleWithDoc, alignWithMargins, firstHeader, firstFooter, evenHeader, evenFooter}",
            has_doc,
            header_footer
        ),
        cmd!(
            "pageLayout.theme",
            "Themes",
            ["Page Layout", "Themes"],
            None,
            "{name: Craft|Slate|Meadow|Ember|Ocean|Orchid|Graphite, colors?: [12 hex], majorFont?, minorFont?}",
            has_doc,
            theme
        ),
    ]
}

fn freeze_panes(s: &mut Session, p: &Json) -> Result<Json> {
    let si = target_sheet(s, p)?;
    let d = s.doc()?;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    if sh.freeze.is_some() && p.get("cell").is_none() {
        return set_freeze(s, si, None);
    }
    let at = match cell_param_on(s, p, "cell", si)? {
        Some(c) => c,
        None if si == d.wb.active_sheet() => d.selection.active,
        // The selection belongs to the active sheet.
        None => return Err(bad("view.freezePanes", "missing `cell`: the selection is on the active sheet")),
    };
    let top = sh.view_top_left;
    let rows = at.row.saturating_sub(top.row);
    let cols = at.col.saturating_sub(top.col);
    let _ = (rows, cols);
    let (rows, cols) = (at.row, at.col);
    set_freeze(s, si, if rows == 0 && cols == 0 { None } else { Some((rows, cols)) })
}

fn set_freeze(s: &mut Session, si: usize, f: Option<(u32, u32)>) -> Result<Json> {
    edit(s, |cx| {
        cx.sheet_mut(si)?.freeze = f;
        Ok(json!({"freeze": f}))
    })
}

fn sheet_flag(s: &mut Session, p: &Json, which: &str) -> Result<Json> {
    edit(s, |cx| {
        let si = cx.sheet_index();
        let sh = cx.sheet_mut(si)?;
        let slot = match which {
            "gridlines" => &mut sh.show_gridlines,
            "headings" => &mut sh.show_headings,
            "zeros" => &mut sh.show_zeros,
            "outlineSymbols" => &mut sh.show_outline_symbols,
            "rtl" => &mut sh.right_to_left,
            "printGridlines" => &mut sh.print.gridlines,
            _ => &mut sh.print.headings,
        };
        *slot = bool_param(p, "on").unwrap_or(!*slot);
        Ok(json!({"on": *slot}))
    })
}

/// File › Options › Advanced › Display options for this workbook: the sheet tabs and the two
/// scroll bars, saved with the workbook (`<workbookView showSheetTabs … />`).
fn workbook_view_flag(s: &mut Session, p: &Json, which: &str) -> Result<Json> {
    edit(s, |cx| {
        let view = &mut cx.wb.view;
        let slot = match which {
            "sheetTabs" => &mut view.show_sheet_tabs,
            "horizontal" => &mut view.show_horizontal_scroll,
            _ => &mut view.show_vertical_scroll,
        };
        *slot = bool_param(p, "on").unwrap_or(!*slot);
        Ok(json!({"on": *slot}))
    })
}

/// Ctrl+6: hides every object, or shows them again. Placeholders (from a file) count as shown.
fn show_objects(s: &mut Session, p: &Json) -> Result<Json> {
    use gridcraft_model::ShowObjects;
    edit(s, |cx| {
        let on = bool_param(p, "on").unwrap_or(cx.wb.show_objects == ShowObjects::None);
        cx.wb.show_objects = if on { ShowObjects::All } else { ShowObjects::None };
        Ok(json!({"on": on}))
    })
}

fn status_bar(s: &mut Session, p: &Json) -> Result<Json> {
    let st = &mut s.prefs.status_bar;
    for (key, flag) in [
        ("average", &mut st.average),
        ("count", &mut st.count),
        ("numericalCount", &mut st.numerical_count),
        ("minimum", &mut st.minimum),
        ("maximum", &mut st.maximum),
        ("sum", &mut st.sum),
    ] {
        if let Some(on) = bool_param(p, key) {
            *flag = on;
        }
    }
    Ok(json!({
        "average": st.average,
        "count": st.count,
        "numericalCount": st.numerical_count,
        "minimum": st.minimum,
        "maximum": st.maximum,
        "sum": st.sum,
    }))
}

fn zoom(s: &mut Session, p: &Json) -> Result<Json> {
    let z = f64_param(p, "percent").ok_or_else(|| bad("view.zoom", "missing `percent`"))?.clamp(10.0, 400.0) as u16;
    let d = s.doc_mut()?;
    let dirty = d.is_dirty();
    let wb = std::sync::Arc::make_mut(&mut d.wb);
    let si = wb.active_sheet();
    if let Some(sh) = wb.sheet_mut(si) {
        sh.zoom = z;
    }
    if !dirty {
        d.saved = d.wb.clone();
    }
    Ok(json!({"zoom": z}))
}

fn zoom_selection(s: &mut Session, p: &Json) -> Result<Json> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let r = d.selection.current();
    let w = (sh.col_left(r.end.col + 1) - sh.col_left(r.start.col)).max(1.0);
    let h = (sh.row_top(r.end.row + 1) - sh.row_top(r.start.row)).max(1.0);
    let vw = f64_param(p, "viewWidth").unwrap_or(1200.0);
    let vh = f64_param(p, "viewHeight").unwrap_or(700.0);
    let z = ((vw / w).min(vh / h) * 100.0).clamp(10.0, 400.0).floor();
    zoom(s, &json!({"percent": z}))
}

fn orientation(s: &mut Session, p: &Json) -> Result<Json> {
    let o = if str_param(p, "orientation") == Some("landscape") { Orientation::Landscape } else { Orientation::Portrait };
    edit(s, |cx| {
        let si = cx.sheet_index();
        cx.sheet_mut(si)?.print.orientation = o;
        Ok(Json::Null)
    })
}

fn paper(s: &mut Session, p: &Json) -> Result<Json> {
    let paper = str_param(p, "paper").unwrap_or("Letter").to_string();
    edit(s, |cx| {
        let si = cx.sheet_index();
        cx.sheet_mut(si)?.print.paper = paper.clone();
        Ok(Json::Null)
    })
}

fn margins(s: &mut Session, p: &Json) -> Result<Json> {
    let preset = match str_param(p, "preset") {
        Some("wide") => Some([1.0, 1.0, 1.0, 1.0, 0.5, 0.5]),
        Some("narrow") => Some([0.25, 0.25, 0.75, 0.75, 0.3, 0.3]),
        Some("normal") => Some([0.7, 0.7, 0.75, 0.75, 0.3, 0.3]),
        _ => None,
    };
    edit(s, |cx| {
        let si = cx.sheet_index();
        let m = &mut cx.sheet_mut(si)?.print.margins;
        if let Some(pr) = preset {
            *m = pr;
        }
        for (i, k) in ["left", "right", "top", "bottom", "header", "footer"].iter().enumerate() {
            if let Some(v) = f64_param(p, k)
                && let Some(slot) = m.get_mut(i)
            {
                *slot = v.clamp(0.0, 20.0) as f32;
            }
        }
        Ok(Json::Null)
    })
}

fn print_area(s: &mut Session, p: &Json) -> Result<Json> {
    let clear = bool_param(p, "clear").unwrap_or(false);
    let r = target_range(s, p)?;
    edit(s, |cx| {
        let si = cx.sheet_index();
        cx.sheet_mut(si)?.print.print_area = if clear { None } else { Some(r) };
        Ok(Json::Null)
    })
}

fn print_titles(s: &mut Session, p: &Json) -> Result<Json> {
    // An absent key leaves that axis alone, and an empty or null value clears it — the same
    // reading `pageLayout.pageSetup` gives `titleRows`/`titleCols`. Assigning `None` for a
    // key that was never sent used to wipe the other axis.
    let axis = |key: &str, is_cols: bool| -> Result<Option<Option<(u32, u32)>>> {
        match p.get(key) {
            None => Ok(None),
            Some(Json::Null) => Ok(Some(None)),
            Some(Json::String(t)) if t.trim().is_empty() => Ok(Some(None)),
            Some(Json::String(t)) => match RangeRef::parse(t) {
                Some(r) => Ok(Some(Some(if is_cols { (r.start.col, r.end.col) } else { (r.start.row, r.end.row) }))),
                None => Err(bad("pageLayout.printTitles", format!("`{key}` is not a range: `{t}`"))),
            },
            Some(_) => Err(bad("pageLayout.printTitles", format!("`{key}` must be text like \"1:2\" or \"A:A\""))),
        }
    };
    let (rows, cols) = (axis("rows", false)?, axis("cols", true)?);
    edit(s, |cx| {
        let si = cx.sheet_index();
        let pr = &mut cx.sheet_mut(si)?.print;
        if let Some(t) = rows {
            pr.title_rows = t;
        }
        if let Some(t) = cols {
            pr.title_cols = t;
        }
        Ok(Json::Null)
    })
}

fn scale_fit(s: &mut Session, p: &Json) -> Result<Json> {
    edit(s, |cx| {
        let si = cx.sheet_index();
        let pr = &mut cx.sheet_mut(si)?.print;
        if let Some(sc) = f64_param(p, "scale") {
            pr.scale = sc.clamp(10.0, 400.0) as u16;
            pr.fit_to = None;
        }
        if p.get("width").is_some() || p.get("height").is_some() {
            pr.fit_to = Some((u32_param(p, "width").unwrap_or(0).min(32767) as u16, u32_param(p, "height").unwrap_or(0).min(32767) as u16));
        }
        Ok(Json::Null)
    })
}

fn breaks(s: &mut Session, p: &Json) -> Result<Json> {
    let at = cell_param(s, p, "cell")?.unwrap_or(s.doc()?.selection.active);
    let insert = bool_param(p, "insert").unwrap_or(false);
    let remove = bool_param(p, "remove").unwrap_or(false);
    let reset = bool_param(p, "reset").unwrap_or(false);
    edit(s, |cx| {
        let si = cx.sheet_index();
        let pr = &mut cx.sheet_mut(si)?.print;
        if reset {
            pr.row_breaks.clear();
            pr.col_breaks.clear();
        } else if insert || !remove {
            if at.row > 0 && !pr.row_breaks.contains(&at.row) {
                pr.row_breaks.push(at.row);
                pr.row_breaks.sort_unstable();
            }
            if at.col > 0 && !pr.col_breaks.contains(&at.col) {
                pr.col_breaks.push(at.col);
                pr.col_breaks.sort_unstable();
            }
        } else {
            pr.row_breaks.retain(|r| *r != at.row);
            pr.col_breaks.retain(|c| *c != at.col);
        }
        Ok(Json::Null)
    })
}

/// Excel keeps at most 255 characters per header or footer.
const MAX_HEADER_FOOTER: usize = 255;

fn header_footer(s: &mut Session, p: &Json) -> Result<Json> {
    const CMD: &str = "pageLayout.headerFooter";
    let si = target_sheet(s, p)?;
    let cap = |t: &str| t.chars().take(MAX_HEADER_FOOTER).collect::<String>();
    // One section edited in place (Page Layout view).
    let section = match (str_param(p, "part"), str_param(p, "section")) {
        (None, None) => None,
        (part, sec) => {
            let footer = match part.unwrap_or("header") {
                "header" => false,
                "footer" => true,
                other => return Err(bad(CMD, format!("`part` must be header or footer, not `{other}`"))),
            };
            let idx = match sec.unwrap_or("center") {
                "left" => 0usize,
                "center" | "centre" => 1,
                "right" => 2,
                other => return Err(bad(CMD, format!("`section` must be left, center or right, not `{other}`"))),
            };
            let page = match str_param(p, "page") {
                None => gridcraft_model::HfPage::Odd,
                Some(id) => gridcraft_model::HfPage::parse(id).ok_or_else(|| bad(CMD, format!("`page` must be odd, even or first, not `{id}`")))?,
            };
            Some((page, footer, idx, super::print::fields_to_codes(str_param(p, "text").unwrap_or(""))))
        }
    };
    edit(s, |cx| {
        let pr = &mut cx.sheet_mut(si)?.print;
        if let Some(h) = str_param(p, "header") {
            pr.header = cap(h);
        }
        if let Some(f) = str_param(p, "footer") {
            pr.footer = cap(f);
        }
        for (key, slot) in [
            ("firstHeader", &mut pr.first_header),
            ("firstFooter", &mut pr.first_footer),
            ("evenHeader", &mut pr.even_header),
            ("evenFooter", &mut pr.even_footer),
        ] {
            if let Some(t) = str_param(p, key) {
                *slot = cap(t);
            }
        }
        if let Some(on) = bool_param(p, "differentFirst") {
            pr.different_first = on;
        }
        if let Some(on) = bool_param(p, "differentOddEven") {
            pr.different_odd_even = on;
        }
        if let Some(on) = bool_param(p, "scaleWithDoc") {
            pr.hf_scale_with_doc = on;
        }
        if let Some(on) = bool_param(p, "alignWithMargins") {
            pr.hf_align_with_margins = on;
        }
        if let Some((page, footer, idx, text)) = &section {
            let spec = pr.hf_mut(*page, *footer);
            let mut parts = super::print::split_header_footer(spec);
            if let Some(slot) = parts.get_mut(*idx) {
                *slot = text.clone();
            }
            *spec = cap(&super::print::join_header_footer(&parts));
        }
        Ok(json!({
            "header": pr.header,
            "footer": pr.footer,
            "differentFirst": pr.different_first,
            "differentOddEven": pr.different_odd_even,
            "scaleWithDoc": pr.hf_scale_with_doc,
            "alignWithMargins": pr.hf_align_with_margins,
            "firstHeader": pr.first_header,
            "firstFooter": pr.first_footer,
            "evenHeader": pr.even_header,
            "evenFooter": pr.even_footer,
        }))
    })
}

/// GridCraft's own theme set (original palettes).
pub fn themes() -> Vec<(&'static str, [u32; 12])> {
    vec![
        ("Craft", [0xFFFFFF, 0x000000, 0xE8E8E8, 0x0E2841, 0x156082, 0xE97132, 0x196B24, 0x0F9ED5, 0xA02B93, 0x4EA72E, 0x467886, 0x96607D]),
        ("Slate", [0xFFFFFF, 0x1F2328, 0xEEF1F4, 0x3B4652, 0x4F6D8A, 0x8AA1B8, 0xC27C4C, 0x6F9A72, 0x9D6C9B, 0xD3B25A, 0x2F6FB0, 0x7A5C8F]),
        ("Meadow", [0xFFFFFF, 0x16241A, 0xEEF4EA, 0x2E4A33, 0x4D8B3F, 0x9BC53D, 0xE4B33A, 0x2E8C83, 0xC4643C, 0x7B6A9E, 0x2B7A57, 0x6E7F3A]),
        ("Ember", [0xFFFFFF, 0x2A1A14, 0xF6EEE8, 0x5A2E1E, 0xD9532B, 0xF2994A, 0xF2C94C, 0x8C3B2E, 0x4A6FA5, 0x6F8F4E, 0xB5472A, 0x8C6A5A]),
        ("Ocean", [0xFFFFFF, 0x0B1E2D, 0xE8F1F7, 0x123A5A, 0x1F78B4, 0x33A6CC, 0x52C7B8, 0x3B5BA5, 0xF2A65A, 0x8E6CB8, 0x1A6FA8, 0x6A7FB0]),
        ("Orchid", [0xFFFFFF, 0x24152B, 0xF3EDF6, 0x4B2C5E, 0x8E44AD, 0xC56CF0, 0xF78FB3, 0x3DC1D3, 0xF5CD79, 0x63CDDA, 0x7E3FA0, 0xB066B0]),
        ("Graphite", [0xFFFFFF, 0x111111, 0xEDEDED, 0x333333, 0x595959, 0x7F7F7F, 0xA5A5A5, 0x3C78D8, 0xCC4125, 0x6AA84F, 0x3C78D8, 0x8E7CC3]),
    ]
}

fn theme(s: &mut Session, p: &Json) -> Result<Json> {
    let name = str_param(p, "name").unwrap_or("Craft").to_string();
    let colors = p
        .get("colors")
        .and_then(Json::as_array)
        .and_then(|a| {
            let v: Vec<u32> = a.iter().filter_map(|c| c.as_str().and_then(|h| u32::from_str_radix(h.trim_start_matches('#'), 16).ok())).collect();
            <[u32; 12]>::try_from(v).ok()
        })
        .or_else(|| themes().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)).map(|(_, c)| c))
        .ok_or_else(|| bad("pageLayout.theme", format!("unknown theme `{name}`")))?;
    let major = str_param(p, "majorFont").map(str::to_string);
    let minor = str_param(p, "minorFont").map(str::to_string);
    edit(s, |cx| {
        cx.wb.theme.name = name.clone();
        cx.wb.theme.colors = colors;
        if let Some(m) = &major {
            cx.wb.theme.major_font = m.clone();
        }
        if let Some(m) = &minor {
            cx.wb.theme.minor_font = m.clone();
        }
        Ok(Json::Null)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_100_resets_the_active_sheet_zoom() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("view.zoom", json!({"percent": 150})).unwrap();
        assert_eq!(s.execute("view.zoom100", json!({})).unwrap()["zoom"], 100);
        assert_eq!(s.doc().unwrap().wb.active().unwrap().zoom, 100);
    }

    #[test]
    fn formula_bar_toggles_without_a_workbook_and_defaults_to_shown() {
        let mut s = Session::new();
        assert!(s.prefs.formula_bar);
        assert_eq!(s.execute("view.formulaBar", json!({})).unwrap()["on"], false);
        assert_eq!(s.execute("view.formulaBar", json!({"on": true})).unwrap()["on"], true);
        // A prefs.json saved before the option existed keeps the formula bar shown.
        let old: crate::Prefs = serde_json::from_str(r#"{"userName": "Ann"}"#).unwrap();
        assert!(old.formula_bar);
    }

    #[test]
    fn status_bar_statistics_default_to_average_count_and_sum_and_turn_on_one_at_a_time() {
        let mut s = Session::new();
        let all = json!({"average": true, "count": true, "numericalCount": false, "minimum": false, "maximum": false, "sum": true});
        assert_eq!(s.execute("view.statusBar", json!({})).unwrap(), all);
        let r = s.execute("view.statusBar", json!({"minimum": true, "sum": false, "count": "yes"})).unwrap();
        assert_eq!(r, json!({"average": true, "count": true, "numericalCount": false, "minimum": true, "maximum": false, "sum": false}));
        assert!(s.prefs.status_bar.minimum && !s.prefs.status_bar.sum);
        // A prefs.json saved before the option existed, or with only some of the six, keeps the defaults for the rest.
        let old: crate::Prefs = serde_json::from_str(r#"{"userName": "Ann"}"#).unwrap();
        assert_eq!(old.status_bar, crate::StatusBarStats::default());
        let part: crate::Prefs = serde_json::from_str(r#"{"statusBar": {"maximum": true}}"#).unwrap();
        assert!(part.status_bar.maximum && part.status_bar.average && part.status_bar.sum && !part.status_bar.minimum);
        let saved = serde_json::to_value(&s.prefs).unwrap();
        assert_eq!(saved["statusBar"]["minimum"], true);
        assert_eq!(saved["statusBar"]["numericalCount"], false);
    }
}
