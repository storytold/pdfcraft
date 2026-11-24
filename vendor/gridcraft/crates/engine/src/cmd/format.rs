//! Formatting: Home › Font, Alignment, Number, Styles, Cells › Format.

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{BorderLine, BorderStyle, Borders, Color, Fill, HAlign, NumFmt, Style, StyleId, Theme, Underline, VAlign, VertAlign};
use serde_json::{Value as Json, json};

use super::{
    CommandSpec, EngineError, Result, Session, bad, bool_param, cmd, edit, f64_param, has_doc, ok, parse_lines, split_sheet, str_param,
    target_ranges, target_sheet,
};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // Font
        cmd!("home.bold", "Bold", ["Home", "Font"], Some("Cmd+B"), "{range?, on?: bool}", has_doc, |s, p| toggle(
            s,
            p,
            |st| st.font.bold,
            |st, v| st.font.bold = v
        )),
        cmd!("home.italic", "Italic", ["Home", "Font"], Some("Cmd+I"), "{range?, on?}", has_doc, |s, p| toggle(
            s,
            p,
            |st| st.font.italic,
            |st, v| st.font.italic = v
        )),
        cmd!(
            "home.underline",
            "Underline",
            ["Home", "Font"],
            Some("Cmd+U"),
            "{range?, on?, style?: single|double|singleAccounting|doubleAccounting}",
            has_doc,
            underline
        ),
        cmd!("home.doubleUnderline", "Double Underline", ["Home", "Font"], None, "{range?}", has_doc, |s, p| underline(
            s,
            &with(p, "style", json!("double"))
        )),
        cmd!("home.strikethrough", "Strikethrough", ["Home", "Font"], Some("Cmd+Shift+X"), "{range?, on?}", has_doc, |s, p| toggle(
            s,
            p,
            |st| st.font.strike,
            |st, v| st.font.strike = v
        )),
        cmd!("home.superscript", "Superscript", ["Home", "Font"], None, "{range?, on?}", has_doc, |s, p| toggle(
            s,
            p,
            |st| st.font.vert == VertAlign::Superscript,
            |st, v| st.font.vert = if v { VertAlign::Superscript } else { VertAlign::Baseline }
        )),
        cmd!("home.subscript", "Subscript", ["Home", "Font"], None, "{range?, on?}", has_doc, |s, p| toggle(
            s,
            p,
            |st| st.font.vert == VertAlign::Subscript,
            |st, v| st.font.vert = if v { VertAlign::Subscript } else { VertAlign::Baseline }
        )),
        cmd!("home.fontName", "Font", ["Home", "Font"], None, "{range?, name: \"Calibri\"}", has_doc, font_name),
        cmd!("home.fontSize", "Font Size", ["Home", "Font"], None, "{range?, size: 11}", has_doc, font_size),
        cmd!("home.increaseFontSize", "Increase Font Size", ["Home", "Font"], Some("Cmd+Shift+>"), "{range?}", has_doc, |s, p| step_font(s, p, true)),
        cmd!("home.decreaseFontSize", "Decrease Font Size", ["Home", "Font"], Some("Cmd+Shift+<"), "{range?}", has_doc, |s, p| step_font(
            s, p, false
        )),
        cmd!(
            "home.fontColor",
            "Font Color",
            ["Home", "Font"],
            None,
            "{range?, color: \"#C00000\" | \"auto\" | {theme: 4, tint: -0.25}}",
            has_doc,
            font_color
        ),
        cmd!("home.fillColor", "Fill Color", ["Home", "Font"], None, "{range?, color: \"#FFFF00\" | \"none\" | {theme, tint}}", has_doc, fill_color),
        cmd!(
            "home.borders",
            "Borders",
            ["Home", "Font"],
            None,
            "{range?, preset: bottom|top|left|right|none|all|outside|thickOutside|thickBottom|doubleBottom|topBottom|topThickBottom|topDoubleBottom|insideHorizontal|insideVertical|inside|diagonalDown|diagonalUp, style?: thin|medium|thick|dashed|dotted|double|hair…, color?}",
            has_doc,
            borders
        ),
        cmd!(
            "home.formatCells",
            "Format Cells…",
            ["Home", "Cells", "Format"],
            Some("Cmd+1"),
            "{range?, style: {font?, fill?, borders?, align?: {reading_order?: Context|LeftToRight|RightToLeft|null, ...}, numFmt?, protection?}} (partial Style JSON merged in; null clears reading_order)",
            has_doc,
            format_cells
        ),
        // Alignment
        CommandSpec {
            shortcut_mac: Some("Cmd+L"),
            ..cmd!("home.alignLeft", "Align Left", ["Home", "Alignment"], Some("Cmd+L"), "{range?}", has_doc, |s, p| halign(s, p, HAlign::Left))
        },
        CommandSpec {
            shortcut_mac: Some("Cmd+E"),
            ..cmd!("home.alignCenter", "Center", ["Home", "Alignment"], Some("Cmd+E"), "{range?}", has_doc, |s, p| halign(s, p, HAlign::Center))
        },
        cmd!("home.alignRight", "Align Right", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| halign(s, p, HAlign::Right)),
        cmd!("home.justify", "Justify", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| halign(s, p, HAlign::Justify)),
        cmd!("home.alignTop", "Top Align", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| valign(s, p, VAlign::Top)),
        cmd!("home.alignMiddle", "Middle Align", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| valign(s, p, VAlign::Center)),
        cmd!("home.alignBottom", "Bottom Align", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| valign(s, p, VAlign::Bottom)),
        cmd!("home.wrapText", "Wrap Text", ["Home", "Alignment"], None, "{range?, on?}", has_doc, |s, p| toggle(
            s,
            p,
            |st| st.align.wrap,
            |st, v| st.align.wrap = v
        )),
        cmd!("home.shrinkToFit", "Shrink to Fit", [], None, "{range?, on?}", has_doc, |s, p| toggle(
            s,
            p,
            |st| st.align.shrink,
            |st, v| st.align.shrink = v
        )),
        cmd!("home.increaseIndent", "Increase Indent", ["Home", "Alignment"], Some("Ctrl+Alt+Tab"), "{range?}", has_doc, |s, p| indent(s, p, 1)),
        cmd!("home.decreaseIndent", "Decrease Indent", ["Home", "Alignment"], Some("Ctrl+Alt+Shift+Tab"), "{range?}", has_doc, |s, p| indent(
            s, p, -1
        )),
        cmd!(
            "home.orientation",
            "Orientation",
            ["Home", "Alignment"],
            None,
            "{range?, angle: -90..90 | \"vertical\" | \"counterclockwise\" | \"clockwise\" | \"up\" | \"down\" | 0}",
            has_doc,
            orientation
        ),
        cmd!("home.mergeCenter", "Merge & Center", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| merge(s, p, "center")),
        cmd!("home.mergeAcross", "Merge Across", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| merge(s, p, "across")),
        cmd!("home.mergeCells", "Merge Cells", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| merge(s, p, "cells")),
        cmd!("home.unmergeCells", "Unmerge Cells", ["Home", "Alignment"], None, "{range?}", has_doc, |s, p| merge(s, p, "unmerge")),
        // Number
        cmd!(
            "home.numberFormat",
            "Number Format",
            ["Home", "Number"],
            None,
            "{range?, format: \"General\"|\"Number\"|\"Currency\"|\"Accounting\"|\"Short Date\"|\"Long Date\"|\"Time\"|\"Percentage\"|\"Fraction\"|\"Scientific\"|\"Text\" | code: \"0.00\"}",
            has_doc,
            number_format
        ),
        cmd!("home.accounting", "Accounting Number Format", ["Home", "Number"], None, "{range?, symbol?: \"$\"}", has_doc, accounting),
        cmd!("home.percent", "Percent Style", ["Home", "Number"], Some("Ctrl+Shift+%"), "{range?}", has_doc, |s, p| set_fmt(s, p, "0%")),
        cmd!("home.comma", "Comma Style", ["Home", "Number"], None, "{range?}", has_doc, |s, p| set_fmt(s, p, COMMA)),
        cmd!("home.increaseDecimal", "Increase Decimal", ["Home", "Number"], None, "{range?}", has_doc, |s, p| decimals(s, p, 1)),
        cmd!("home.decreaseDecimal", "Decrease Decimal", ["Home", "Number"], None, "{range?}", has_doc, |s, p| decimals(s, p, -1)),
        // Styles
        cmd!(
            "home.cellStyle",
            "Cell Styles",
            ["Home", "Styles"],
            None,
            "{range?, name: \"Good\"|\"Bad\"|\"Neutral\"|\"Heading 1\"|\"Title\"|\"Total\"|\"Accent1\"…|\"Normal\"}",
            has_doc,
            cell_style
        ),
        // Cells › Format
        cmd!(
            "home.rowHeight",
            "Row Height…",
            ["Home", "Cells", "Format"],
            None,
            "{sheet?, rows?: \"2:5\" | \"2:2,4:5\" | \"Sheet2!2:5\", height: 20 (pixels) | points: 15}",
            has_doc,
            row_height
        ),
        cmd!(
            "home.columnWidth",
            "Column Width…",
            ["Home", "Cells", "Format"],
            None,
            "{sheet?, cols?: \"B:D\" | \"B:B,D:F\" | \"Sheet2!B:D\", width: 64 (pixels) | chars: 8.43}",
            has_doc,
            col_width
        ),
        cmd!("home.autofitRowHeight", "AutoFit Row Height", ["Home", "Cells", "Format"], None, "{sheet?, rows?}", has_doc, autofit_rows),
        cmd!("home.autofitColumnWidth", "AutoFit Column Width", ["Home", "Cells", "Format"], None, "{sheet?, cols?}", has_doc, autofit_cols),
        cmd!("home.defaultWidth", "Default Width…", ["Home", "Cells", "Format"], None, "{sheet?, width: 64 (pixels)}", has_doc, default_width),
        cmd!("home.hideRows", "Hide Rows", ["Home", "Cells", "Format", "Hide & Unhide"], Some("Cmd+9"), "{sheet?, rows?}", has_doc, |s, p| hide(
            s, p, true, true
        )),
        cmd!("home.hideColumns", "Hide Columns", ["Home", "Cells", "Format", "Hide & Unhide"], Some("Cmd+0"), "{sheet?, cols?}", has_doc, |s, p| {
            hide(s, p, false, true)
        }),
        cmd!(
            "home.unhideRows",
            "Unhide Rows",
            ["Home", "Cells", "Format", "Hide & Unhide"],
            Some("Cmd+Shift+9"),
            "{sheet?, rows?}",
            has_doc,
            |s, p| hide(s, p, true, false)
        ),
        cmd!(
            "home.unhideColumns",
            "Unhide Columns",
            ["Home", "Cells", "Format", "Hide & Unhide"],
            Some("Cmd+Shift+0"),
            "{sheet?, cols?}",
            has_doc,
            |s, p| hide(s, p, false, false)
        ),
        cmd!("home.lockCell", "Lock Cell", ["Home", "Cells", "Format"], None, "{range?, on?}", has_doc, |s, p| toggle(
            s,
            p,
            |st| st.protection.locked,
            |st, v| st.protection.locked = v
        )),
    ]
}

const ACCOUNTING: &str = "_(\"$\"* #,##0.00_);_(\"$\"* \\(#,##0.00\\);_(\"$\"* \"-\"??_);_(@_)";
const COMMA: &str = "_(* #,##0.00_);_(* \\(#,##0.00\\);_(* \"-\"??_);_(@_)";

/// Accounting Number Format with the currency symbol to use (Excel's own button takes the system
/// currency; `symbol` is how the control channel picks another one, e.g. `"L"` for lempiras).
fn accounting(s: &mut Session, p: &Json) -> Result<Json> {
    let symbol = str_param(p, "symbol").unwrap_or("$");
    // The symbol goes into a quoted literal: a quote or a backslash would end it, and a control
    // character would reach the format's text as-is.
    if symbol.contains(['"', '\\']) || symbol.chars().any(char::is_control) {
        return Err(bad("home.accounting", "the symbol can't contain a quote, a backslash or a control character"));
    }
    set_fmt(s, p, &accounting_format(symbol))
}

fn accounting_format(symbol: &str) -> String {
    let symbol = format!("\"{symbol}\"");
    format!("_({symbol}* #,##0.00_);_({symbol}* \\(#,##0.00\\);_({symbol}* \"-\"??_);_(@_)")
}

fn with(p: &Json, key: &str, v: Json) -> Json {
    let mut m = p.as_object().cloned().unwrap_or_default();
    m.insert(key.into(), v);
    Json::Object(m)
}

/// Applies `f` to the style of every targeted cell (whole rows/columns set the line format too).
pub(crate) fn apply_style(s: &mut Session, p: &Json, f: impl Fn(&mut Style)) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let ranges = target_ranges(s, p)?;
    let guarded = s.doc()?.wb.sheet(sheet).is_some_and(|sh| sh.protection.is_some());
    if s.doc()?.wb.sheet(sheet).is_some_and(|sh| sh.protection.as_ref().is_some_and(|pr| !pr.format_cells)) {
        return Err(EngineError::Other("The cell or chart you're trying to change is on a protected sheet.".into()));
    }
    edit(s, |cx| {
        // Formatting on a protected sheet keeps the cell's permission flags: a format call
        // that flips `locked` from the format permission would authorize content edits that
        // the password should still gate, so those flags may not change (Excel keeps that
        // checkbox disabled while the sheet is protected).
        const FLAG_GUARD: &str = "You can't change cell protection on a protected sheet. Unprotect it first.";
        macro_rules! guard_flags {
            ($old:expr, $new:expr) => {
                if guarded && cx.wb.styles.get($old).protection != cx.wb.styles.get($new).protection {
                    return Err(EngineError::Other(FLAG_GUARD.into()));
                }
            };
        }
        let mut cache: std::collections::HashMap<StyleId, StyleId> = std::collections::HashMap::new();
        for r in &ranges {
            let full_rows = r.is_full_rows();
            let full_cols = r.is_full_cols();
            if full_rows || full_cols {
                // Line formats.
                let lines: Vec<u32> = if full_rows && !full_cols { (r.start.row..=r.end.row).collect() } else { (r.start.col..=r.end.col).collect() };
                for line in lines {
                    let cur = {
                        let sh = cx.wb.sheet(sheet);
                        let info = if full_rows && !full_cols { sh.and_then(|s| s.rows.get(&line)) } else { sh.and_then(|s| s.cols.get(&line)) };
                        info.and_then(|i| i.style).unwrap_or_default()
                    };
                    let new = *cache.entry(cur).or_insert_with(|| cx.wb.styles.derive(cur, &f));
                    guard_flags!(cur, new);
                    let sh = cx.sheet_mut(sheet)?;
                    let map = if full_rows && !full_cols { &mut sh.rows } else { &mut sh.cols };
                    map.entry(line).or_default().style = Some(new);
                }
                // Existing cells in those lines.
                let cells: Vec<(CellRef, StyleId)> =
                    cx.wb.sheet(sheet).map(|sh| sh.cells.iter_range(*r).map(|(c, x)| (c, x.style)).collect()).unwrap_or_default();
                for (c, st) in &cells {
                    let new = *cache.entry(*st).or_insert_with(|| cx.wb.styles.derive(*st, &f));
                    guard_flags!(*st, new);
                    cx.sheet_mut(sheet)?.set_style(*c, new);
                }
                // Formatting touched these cells: re-fit their rows (Excel auto-fits).
                cx.fit_rows.extend(cells.iter().map(|(c, _)| (sheet, c.row)));
                continue;
            }
            if r.count() > 4_000_000 {
                return Err(bad("format", "selection too large to format cell by cell"));
            }
            if r.height() <= 100_000 {
                cx.fit_rows.extend((r.start.row..=r.end.row).map(|row| (sheet, row)));
            }
            for c in r.iter() {
                let st = cx.wb.sheet(sheet).map(|sh| sh.style_id(c)).unwrap_or_default();
                let new = *cache.entry(st).or_insert_with(|| cx.wb.styles.derive(st, &f));
                if new != st {
                    guard_flags!(st, new);
                    cx.sheet_mut(sheet)?.set_style(c, new);
                }
            }
        }
        Ok(Json::Null)
    })
}

/// The style of the active cell (what toggles read).
pub(crate) fn active_style(s: &Session) -> Result<Style> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    Ok(d.wb.styles.get(sh.style_id(d.selection.active)).clone())
}

fn toggle(s: &mut Session, p: &Json, get: fn(&Style) -> bool, set: fn(&mut Style, bool)) -> Result<Json> {
    let on = match bool_param(p, "on") {
        Some(v) => v,
        None => !get(&active_style(s)?),
    };
    apply_style(s, p, |st| set(st, on))?;
    Ok(json!({"on": on}))
}

fn underline(s: &mut Session, p: &Json) -> Result<Json> {
    let style = match str_param(p, "style").unwrap_or("single") {
        "double" => Underline::Double,
        "singleAccounting" => Underline::SingleAccounting,
        "doubleAccounting" => Underline::DoubleAccounting,
        "none" => Underline::None,
        _ => Underline::Single,
    };
    let cur = active_style(s)?.font.underline;
    let on = bool_param(p, "on").unwrap_or(cur != style);
    let u = if on { style } else { Underline::None };
    apply_style(s, p, |st| st.font.underline = u)?;
    Ok(json!({"on": on}))
}

fn font_name(s: &mut Session, p: &Json) -> Result<Json> {
    let Some(name) = str_param(p, "name").map(str::to_string) else { return Err(bad("home.fontName", "missing `name`")) };
    apply_style(s, p, |st| {
        st.font.name = name.clone();
        st.font.charset = None;
        st.font.family = None;
    })
}

fn font_size(s: &mut Session, p: &Json) -> Result<Json> {
    let size = f64_param(p, "size").ok_or_else(|| bad("home.fontSize", "missing `size`"))?;
    if !(1.0..=409.0).contains(&size) {
        return Err(EngineError::Other("The number must be between 1 and 409.".into()));
    }
    let size = ((size * 2.0).round() / 2.0) as f32;
    apply_style(s, p, |st| st.font.size = size)
}

const SIZES: [f32; 16] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0, 48.0, 72.0, 96.0, 409.0];

fn step_font(s: &mut Session, p: &Json, up: bool) -> Result<Json> {
    let cur = active_style(s)?.font.size;
    let next =
        if up { SIZES.iter().copied().find(|x| *x > cur).unwrap_or(409.0) } else { SIZES.iter().rev().copied().find(|x| *x < cur).unwrap_or(1.0) };
    apply_style(s, p, |st| st.font.size = next)?;
    Ok(json!({"size": next}))
}

pub(crate) fn color_param(v: Option<&Json>) -> Option<Color> {
    match v? {
        Json::String(s) if s.eq_ignore_ascii_case("auto") || s.eq_ignore_ascii_case("automatic") || s.eq_ignore_ascii_case("none") => {
            Some(Color::Auto)
        }
        Json::String(s) => Color::from_hex(s),
        Json::Object(o) => {
            let theme = o.get("theme")?.as_u64()? as u8;
            let tint = o.get("tint").and_then(Json::as_f64).unwrap_or(0.0);
            Some(Color::Theme(theme.min(11), (tint.clamp(-1.0, 1.0) * 1000.0) as i16))
        }
        _ => None,
    }
}

fn font_color(s: &mut Session, p: &Json) -> Result<Json> {
    let c = color_param(p.get("color")).ok_or_else(|| bad("home.fontColor", "missing or invalid `color`"))?;
    apply_style(s, p, |st| st.font.color = c)
}

fn fill_color(s: &mut Session, p: &Json) -> Result<Json> {
    let c = color_param(p.get("color")).ok_or_else(|| bad("home.fillColor", "missing or invalid `color`"))?;
    apply_style(s, p, |st| st.fill = if c == Color::Auto { Fill::default() } else { Fill::solid(c) })
}

fn border_style(name: &str) -> BorderStyle {
    match name {
        "medium" => BorderStyle::Medium,
        "thick" => BorderStyle::Thick,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        "double" => BorderStyle::Double,
        "hair" => BorderStyle::Hair,
        "mediumDashed" => BorderStyle::MediumDashed,
        "dashDot" => BorderStyle::DashDot,
        "mediumDashDot" => BorderStyle::MediumDashDot,
        "dashDotDot" => BorderStyle::DashDotDot,
        "slantDashDot" => BorderStyle::SlantDashDot,
        "none" => BorderStyle::None,
        _ => BorderStyle::Thin,
    }
}

fn borders(s: &mut Session, p: &Json) -> Result<Json> {
    let preset = str_param(p, "preset").unwrap_or("bottom").to_string();
    let color = color_param(p.get("color")).unwrap_or(Color::Auto);
    let base = border_style(str_param(p, "style").unwrap_or("thin"));
    let sheet = target_sheet(s, p)?;
    let ranges = target_ranges(s, p)?;
    if s.doc()?.wb.sheet(sheet).is_some_and(|sh| sh.protection.as_ref().is_some_and(|pr| !pr.format_cells)) {
        return Err(EngineError::Other("The cell or chart you're trying to change is on a protected sheet.".into()));
    }
    edit(s, |cx| {
        for r in &ranges {
            if r.count() > 2_000_000 {
                return Err(bad("home.borders", "selection too large"));
            }
            for c in r.iter() {
                let (top, bottom, left, right) = (c.row == r.start.row, c.row == r.end.row, c.col == r.start.col, c.col == r.end.col);
                let st = cx.wb.sheet(sheet).map(|sh| sh.style_id(c)).unwrap_or_default();
                let line = |style: BorderStyle| BorderLine { style, color };
                let new = cx.wb.styles.derive(st, |s| {
                    let b = &mut s.borders;
                    match preset.as_str() {
                        "none" => *b = Borders::default(),
                        "all" => {
                            b.top = line(base);
                            b.bottom = line(base);
                            b.left = line(base);
                            b.right = line(base);
                        }
                        "outside" | "thickOutside" => {
                            let st = if preset == "thickOutside" { BorderStyle::Thick } else { base };
                            if top {
                                b.top = line(st);
                            }
                            if bottom {
                                b.bottom = line(st);
                            }
                            if left {
                                b.left = line(st);
                            }
                            if right {
                                b.right = line(st);
                            }
                        }
                        "inside" | "insideHorizontal" | "insideVertical" => {
                            if preset != "insideVertical" {
                                if !top {
                                    b.top = line(base);
                                }
                                if !bottom {
                                    b.bottom = line(base);
                                }
                            }
                            if preset != "insideHorizontal" {
                                if !left {
                                    b.left = line(base);
                                }
                                if !right {
                                    b.right = line(base);
                                }
                            }
                        }
                        "top" if top => b.top = line(base),
                        "bottom" if bottom => b.bottom = line(base),
                        "left" if left => b.left = line(base),
                        "right" if right => b.right = line(base),
                        "thickBottom" if bottom => b.bottom = line(BorderStyle::Thick),
                        "doubleBottom" if bottom => b.bottom = line(BorderStyle::Double),
                        "topBottom" => {
                            if top {
                                b.top = line(base);
                            }
                            if bottom {
                                b.bottom = line(base);
                            }
                        }
                        "topThickBottom" => {
                            if top {
                                b.top = line(base);
                            }
                            if bottom {
                                b.bottom = line(BorderStyle::Thick);
                            }
                        }
                        "topDoubleBottom" => {
                            if top {
                                b.top = line(base);
                            }
                            if bottom {
                                b.bottom = line(BorderStyle::Double);
                            }
                        }
                        "diagonalDown" => b.diag_down = line(base),
                        "diagonalUp" => b.diag_up = line(base),
                        _ => {}
                    }
                });
                cx.sheet_mut(sheet)?.set_style(c, new);
                if preset == "none" {
                    // A shared line can be stored on either cell. Selected cells are handled
                    // by this loop; remove only facing edges outside this range.
                    for (boundary, dr, dc) in [(top, -1, 0), (bottom, 1, 0), (left, 0, -1), (right, 0, 1)] {
                        if !boundary {
                            continue;
                        }
                        let Some(neighbor) = c.offset(dr, dc) else { continue };
                        let st = cx.wb.sheet(sheet).map(|sh| sh.style_id(neighbor)).unwrap_or_default();
                        let new = cx.wb.styles.derive(st, |s| {
                            let edge = match (dr, dc) {
                                (-1, _) => &mut s.borders.bottom,
                                (1, _) => &mut s.borders.top,
                                (_, -1) => &mut s.borders.right,
                                _ => &mut s.borders.left,
                            };
                            *edge = BorderLine::default();
                        });
                        if new != st {
                            cx.sheet_mut(sheet)?.set_style(neighbor, new);
                        }
                    }
                }
            }
        }
        Ok(Json::Null)
    })
}

fn format_cells(s: &mut Session, p: &Json) -> Result<Json> {
    let Some(patch) = p.get("style").map(snake_case_keys) else {
        // Menu invocation without params: the UI opens the dialog.
        s.ui_requests.push(crate::UiRequest::Dialog("formatCells".into(), json!({})));
        return ok();
    };
    // Validate by merging into the active style once.
    let base = serde_json::to_value(active_style(s)?).map_err(|e| EngineError::Other(e.to_string()))?;
    let merged = merge_json(base, &patch);
    let _: Style = serde_json::from_value(merged).map_err(|e| bad("home.formatCells", e.to_string()))?;
    apply_style(s, p, |st| {
        if let Ok(cur) = serde_json::to_value(&*st)
            && let Ok(new) = serde_json::from_value::<Style>(merge_json(cur, &patch))
        {
            *st = new;
        }
    })
}

/// The style patch with camelCase keys (`numFmt`, `diagDown`, as documented) renamed to the
/// style's own snake_case (`num_fmt`, `diag_down`). Keys starting with a capital (`Rgb`) stay.
fn snake_case_keys(v: &Json) -> Json {
    match v {
        Json::Object(m) => Json::Object(
            m.iter()
                .map(|(k, v)| {
                    let key = if k.starts_with(|c: char| c.is_ascii_lowercase()) {
                        k.chars().fold(String::new(), |mut out, c| {
                            if c.is_ascii_uppercase() {
                                out.push('_');
                            }
                            out.push(c.to_ascii_lowercase());
                            out
                        })
                    } else {
                        k.clone()
                    };
                    (key, snake_case_keys(v))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Deep-merges `patch` into `base` (objects merge, everything else replaces).
pub(crate) fn merge_json(mut base: Json, patch: &Json) -> Json {
    match (&mut base, patch) {
        (Json::Object(b), Json::Object(p)) => {
            for (k, v) in p {
                let cur = b.remove(k).unwrap_or(Json::Null);
                b.insert(k.clone(), merge_json(cur, v));
            }
            base
        }
        (_, p) => p.clone(),
    }
}

fn halign(s: &mut Session, p: &Json, a: HAlign) -> Result<Json> {
    // Clicking the active alignment again returns to General.
    let cur = active_style(s)?.align.h;
    let a = if cur == a && p.get("on").is_none() { HAlign::General } else { a };
    apply_style(s, p, |st| st.align.h = a)
}

fn valign(s: &mut Session, p: &Json, a: VAlign) -> Result<Json> {
    apply_style(s, p, |st| st.align.v = a)
}

fn indent(s: &mut Session, p: &Json, d: i32) -> Result<Json> {
    apply_style(s, p, |st| {
        st.align.indent = (st.align.indent as i32 + d).clamp(0, 250) as u8;
        if st.align.indent > 0 && matches!(st.align.h, HAlign::General | HAlign::Center | HAlign::Fill | HAlign::Justify | HAlign::CenterAcross) {
            st.align.h = HAlign::Left;
        }
    })
}

fn orientation(s: &mut Session, p: &Json) -> Result<Json> {
    let angle: i16 = match p.get("angle") {
        Some(Json::Number(n)) => n.as_f64().unwrap_or(0.0).clamp(-90.0, 90.0) as i16,
        Some(Json::String(t)) => match t.as_str() {
            "vertical" => 255,
            "counterclockwise" => 45,
            "clockwise" => -45,
            "up" => 90,
            "down" => -90,
            _ => 0,
        },
        _ => 0,
    };
    apply_style(s, p, |st| st.align.rotation = angle)
}

fn merge(s: &mut Session, p: &Json, how: &str) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let ranges = target_ranges(s, p)?;
    let how =
        if how == "center" && s.doc()?.wb.sheet(sheet).is_some_and(|sh| ranges.iter().all(|r| sh.merges.contains(r))) { "unmerge" } else { how };
    edit(s, |cx| {
        let mut touched = Vec::new();
        // Areas whose merging changes: a merged cell blocks a dynamic array's spill.
        let mut areas: Vec<RangeRef> = ranges.clone();
        for r in &ranges {
            let sh = cx.sheet_mut(sheet)?;
            areas.extend(sh.merges.iter().filter(|m| m.intersects(r)));
            sh.merges.retain(|m| !m.intersects(r));
            match how {
                "unmerge" => {}
                "across" => {
                    for row in r.start.row..=r.end.row {
                        let m = RangeRef::new(CellRef::new(row, r.start.col), CellRef::new(row, r.end.col));
                        if !m.is_single() {
                            sh.merges.push(m);
                            // As below: only the first value of each row survives.
                            let others: Vec<CellRef> = sh.cells.iter_range(m).map(|(c, _)| c).filter(|c| *c != m.start).collect();
                            for c in others {
                                if let Some(mut cell) = sh.cells.get(c).cloned() {
                                    cell.clear_content();
                                    sh.set_cell(c, cell);
                                }
                                touched.push((sheet, c));
                            }
                        }
                    }
                }
                _ => {
                    if !r.is_single() {
                        sh.merges.push(*r);
                    }
                    // Only the upper-left value survives (Excel warns about this).
                    if r.count() <= 1_000_000 {
                        let others: Vec<CellRef> = sh.cells.iter_range(*r).map(|(c, _)| c).filter(|c| *c != r.start).collect();
                        for c in others {
                            if let Some(mut cell) = sh.cells.get(c).cloned() {
                                cell.clear_content();
                                sh.set_cell(c, cell);
                            }
                            touched.push((sheet, c));
                        }
                    }
                }
            }
        }
        // Spills over the areas, and blocked ones that could reach them, are evaluated again.
        let sh = cx.sheet_mut(sheet)?;
        for a in &areas {
            touched.extend(sh.spill_ranges.iter().filter(|(_, r)| r.intersects(a)).map(|(anchor, _)| (sheet, *anchor)));
            let reach = RangeRef::new(CellRef::new(0, 0), a.end);
            let blocked = sh
                .cells
                .iter_range(reach)
                // A Python cell's result is blocked and unblocked like a formula's.
                .filter(|(_, x)| {
                    (x.formula.is_some() || x.py.is_some()) && x.value == gridcraft_core::Value::Error(gridcraft_core::CellError::Spill)
                });
            touched.extend(blocked.map(|(c, _)| (sheet, c)));
        }
        cx.changed.extend(touched);
        Ok(())
    })?;
    if how == "center" {
        apply_style(s, p, |st| st.align.h = HAlign::Center)?;
    }
    ok()
}

/// Ribbon format names → codes.
pub fn format_code_for(name: &str) -> &str {
    match name {
        "General" => "General",
        "Number" => "0.00",
        "Currency" => "\"$\"#,##0.00",
        "Accounting" => ACCOUNTING,
        "Short Date" => "m/d/yyyy",
        "Long Date" => "[$-F800]dddd, mmmm dd, yyyy",
        "Time" => "[$-F400]h:mm:ss AM/PM",
        "Percentage" => "0.00%",
        "Fraction" => "# ?/?",
        "Scientific" => "0.00E+00",
        "Text" => "@",
        "Comma" => COMMA,
        other => other,
    }
}

fn number_format(s: &mut Session, p: &Json) -> Result<Json> {
    let code = str_param(p, "code")
        .map(str::to_string)
        .or_else(|| str_param(p, "format").map(|f| format_code_for(f).to_string()))
        .ok_or_else(|| bad("home.numberFormat", "missing `format` or `code`"))?;
    set_fmt(s, p, &code)
}

fn set_fmt(s: &mut Session, p: &Json, code: &str) -> Result<Json> {
    let nf = NumFmt::new(code);
    apply_style(s, p, |st| st.num_fmt = nf.clone())
}

/// Adds or removes one decimal place in the number format (Increase/Decrease Decimal).
pub fn adjust_decimals(code: &str, value: &gridcraft_core::Value, delta: i32) -> String {
    if code == "General" {
        // Start from the displayed number of decimals.
        let text = value.as_f64().map(gridcraft_core::number_to_text).unwrap_or_default();
        let dec = text.split_once('.').map_or(0, |(_, d)| d.len()) as i32;
        let n = (dec + delta).max(0) as usize;
        return if n == 0 { "0".into() } else { format!("0.{}", "0".repeat(n)) };
    }
    let sections: Vec<String> = code
        .split(';')
        .map(|sec| {
            // Find the last digit placeholder run before any exponent / percent etc.
            let chars: Vec<char> = sec.chars().collect();
            let mut in_quote = false;
            let mut in_bracket = false;
            let mut last_digit = None;
            let mut point = None;
            for (i, c) in chars.iter().enumerate() {
                if *c == '"' {
                    in_quote = !in_quote;
                }
                if *c == '[' && !in_quote {
                    in_bracket = true;
                }
                if *c == ']' && !in_quote {
                    in_bracket = false;
                    continue;
                }
                if in_quote || in_bracket {
                    continue;
                }
                if matches!(c, '0' | '#' | '?') {
                    last_digit = Some(i);
                }
                if *c == '.' && point.is_none() {
                    point = Some(i);
                }
                if matches!(c, 'E' | 'e') {
                    break;
                }
            }
            let Some(ld) = last_digit else { return sec.to_string() };
            let mut out: Vec<char> = chars.clone();
            if delta > 0 {
                match point {
                    Some(_) => out.insert(ld + 1, '0'),
                    None => {
                        out.insert(ld + 1, '0');
                        out.insert(ld + 1, '.');
                    }
                }
            } else if let Some(pt) = point
                && ld > pt
            {
                out.remove(ld);
                if ld == pt + 1 {
                    out.remove(pt);
                }
            }
            out.into_iter().collect()
        })
        .collect();
    sections.join(";")
}

fn decimals(s: &mut Session, p: &Json, delta: i32) -> Result<Json> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let a = d.selection.active;
    let code = d.wb.styles.get(sh.style_id(a)).num_fmt.as_str().to_string();
    let new = adjust_decimals(&code, &sh.value(a), delta);
    set_fmt(s, p, &new)
}

pub use gridcraft_model::points_to_chars;

mod cell_styles;
mod sizes;

use cell_styles::cell_style;
pub use cell_styles::{CELL_STYLE_NAMES, builtin_cell_style};
pub use sizes::{approx_text_width, chars_to_points};
use sizes::{autofit_cols, autofit_rows, col_width, default_width, hide, row_height};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_across_keeps_the_first_value_of_each_row() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2], [3, 4]]})).unwrap();
        s.execute("cell.set", json!({"cell": "D1", "input": "=SUM(A1:B2)"})).unwrap();
        s.execute("home.mergeAcross", json!({"range": "A1:B2"})).unwrap();
        let v = |s: &mut crate::Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap()["value"].clone();
        assert_eq!(v(&mut s, "B1"), Json::Null);
        assert_eq!(v(&mut s, "B2"), Json::Null);
        assert_eq!(v(&mut s, "D1"), json!(4.0));
    }

    #[test]
    fn the_default_symbol_reproduces_the_accounting_format() {
        assert_eq!(accounting_format("$"), ACCOUNTING);
    }

    #[test]
    fn decimal_steps() {
        let v = gridcraft_core::Value::Number(1.5);
        assert_eq!(adjust_decimals("General", &v, 1), "0.00");
        assert_eq!(adjust_decimals("General", &v, -1), "0");
        assert_eq!(adjust_decimals("0", &v, 1), "0.0");
        assert_eq!(adjust_decimals("0.00", &v, -1), "0.0");
        assert_eq!(adjust_decimals("0.0", &v, -1), "0");
        assert_eq!(adjust_decimals("#,##0.00;[Red]-#,##0.00", &v, 1), "#,##0.000;[Red]-#,##0.000");
        assert_eq!(adjust_decimals("0%", &v, 1), "0.0%");
    }

    #[test]
    fn format_cells_takes_the_documented_keys() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("cell.set", json!({"cell": "A1", "input": "5"})).unwrap();
        s.execute("home.formatCells", json!({"range": "A1", "style": {"numFmt": "0.0", "borders": {"diagUp": {"style": "Thin"}}}})).unwrap();
        let st = &s.execute("cell.get", json!({"cell": "A1"})).unwrap()["style"];
        assert_eq!(st["num_fmt"], "0.0");
        assert_eq!(st["borders"]["diag_up"]["style"], "Thin");
        // The style's own snake_case keys keep working.
        s.execute("home.formatCells", json!({"range": "A1", "style": {"num_fmt": "0.00"}})).unwrap();
        assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["style"]["num_fmt"], "0.00");
    }

    #[test]
    fn arabic_vowel_and_direction_marks_add_no_width() {
        let plain = approx_text_width("محمد يدرس في المدرسة", 11.0, false);
        assert_eq!(approx_text_width("مُحَمَّد يَدْرُسُ فِي الْمَدْرَسَةِ", 11.0, false), plain);
        assert_eq!(approx_text_width("\u{200F}محمد يدرس في المدرسة\u{200C}", 11.0, false), plain);
        assert_eq!(approx_text_width("ب\u{0897}\u{0898}\u{089F}", 11.0, false), approx_text_width("ب", 11.0, false));
        assert!(approx_text_width("\u{08E2}", 11.0, false) > 0.0, "the disputed end of ayah is a visible sign");
    }

    #[test]
    fn arabic_text_fits_a_column_about_as_wide_as_latin_text_of_the_same_length() {
        let mut s = crate::Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [["مُحَمَّد يَدْرُسُ فِي الْمَدْرَسَةِ", "Hello there in school"]]})).unwrap();
        s.execute("home.autofitColumnWidth", json!({"cols": "A:B"})).unwrap();
        let doc = s.doc().unwrap();
        let width = |c: u32| doc.wb.active().and_then(|sh| sh.cols.get(&c)).and_then(|e| e.size).unwrap();
        let ratio = width(0) / width(1);
        assert!((0.75..=1.15).contains(&ratio), "Arabic {} vs Latin {}", width(0), width(1));
    }

    #[test]
    fn autofit_of_many_separate_columns_fits_only_the_selected_ones() {
        let mut s = crate::Session::new();
        s.new_workbook();
        let long = "a much longer text than the others";
        // Columns A, C, E, G, I, K are selected (six runs), the ones between hold long text too.
        let row: Vec<_> = (0..11).map(|c| if c % 2 == 0 { json!("x") } else { json!(long) }).collect();
        s.execute("range.setValues", json!({"range": "A1", "values": [row]})).unwrap();
        s.execute("home.autofitColumnWidth", json!({"cols": "A:A,C:C,E:E,G:G,I:I,K:K"})).unwrap();
        let doc = s.doc().unwrap();
        let size = |c: u32| doc.wb.active().and_then(|sh| sh.cols.get(&c)).and_then(|e| e.size);
        assert!([0, 2, 4, 6, 8, 10].iter().all(|&c| size(c).is_some()));
        assert!([1, 3, 5, 7, 9].iter().all(|&c| size(c).is_none()));
    }

    #[test]
    fn arabic_word_ligatures_are_wider_than_letters() {
        let em = |t: &str| approx_text_width(t, 72.0, false) / 96.0;
        assert!(em("\u{FDFD}") > 8.0);
        assert!(em("\u{FDFA}") > 1.0);
        let honorifics = ['\u{FD40}', '\u{FD4B}', '\u{FD4C}', '\u{FD4F}', '\u{FDC8}', '\u{FDCF}', '\u{FDFE}', '\u{FDFF}'];
        for honorific in honorifics.into_iter().chain(['\u{FBC3}', '\u{FBD2}', '\u{FD90}', '\u{FD91}']) {
            assert!(em(&honorific.to_string()) > 1.0, "U+{:04X}", u32::from(honorific));
        }
        assert!(em("ب") < 0.6);
    }

    #[test]
    fn arabic_digits_are_as_wide_as_ascii_digits_and_punctuation_as_a_letter() {
        let width = |t: &str| approx_text_width(t, 11.0, false);
        assert_eq!(width("١٢٣"), width("123"));
        assert_eq!(width("۴۵۶"), width("456"));
        assert_eq!(width("،؟"), width("بب"));
    }

    #[test]
    fn cjk_text_keeps_a_full_em_per_character() {
        assert_eq!(approx_text_width("中文", 12.0, false), 2.0 * 12.0 * 96.0 / 72.0);
    }

    #[test]
    fn width_units() {
        assert_eq!(chars_to_points(8.43), 64.0);
        assert!((points_to_chars(64.0) - 8.43).abs() < 0.01);
    }
}
