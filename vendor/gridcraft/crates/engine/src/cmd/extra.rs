//! More ribbon commands: Chart Design and Format, object arrangement, workbook views, table
//! tools, Get Data, scripts (Automate), cell checkboxes, theme colours and fonts.

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{
    Anchor, Chart, Color, DefinedName, HAlign, LegendPos, NumFmt, Shape, ShapeKind, Sheet, SheetViewMode, TableColumn, TotalsFn, Trendline,
    TrendlineKind, Workbook,
};
use serde_json::{Value as Json, json};

use super::{
    CommandSpec, EngineError, Result, Session, always, anchor_mode_name, bad, bool_param, cell_param, cell_param_on, cmd, edit, f64_param, has_doc,
    ok, split_sheet, str_param, target_range, target_ranges, target_sheet, u32_param,
};

mod cells;
mod chart;
mod data;
mod objects;
mod scripts;
mod window;

pub(crate) use cells::is_checkbox_cell;
use cells::{background, checkbox, draw_border, new_cell_style, recently_used, theme_colors, theme_fonts, toggle_checkbox, use_in_formula};
use chart::{add_element, change_colors, format_series, move_chart, quick_layout, select_data, set_values_range};
use data::{from_csv, from_table_range, queries, table_resize};
use objects::{align, arrange, rotate, selection_pane, shape_fill, shape_outline};
pub(crate) use scripts::has_scripts;
use scripts::{all_scripts, delete_script, new_script, record, run_script, stop_action, trust_scripts};
use window::{arrange_all, custom_views, hide_window, new_window, ruler, set_view, split, switch_windows, unhide_window, watch_window};
pub(crate) use window::{keep_view_modes, keep_watch_list, window_param};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // Chart Design / Format
        cmd!(
            "chart.addElement",
            "Add Chart Element",
            ["Chart Design", "Chart Layouts"],
            None,
            "{chart?, element: title|legend|dataLabels|dataTable|gridlines|axisTitles|smooth|trendline, on?: bool, position?: bottom|top|left|right; dataTable: dataTableKeys?: bool, dataTableHorizontalBorder?: bool, dataTableVerticalBorder?: bool, dataTableOutline?: bool; trendline: series?: index (default all), type?: linear|exponential|linearForecast|movingAverage|logarithmic|power|polynomial, order?: 2..6, period?: 2..255, forward?, backward?, displayEquation?, displayRSquared?}",
            has_doc,
            add_element
        ),
        cmd!("chart.quickLayout", "Quick Layout", ["Chart Design", "Chart Layouts"], None, "{chart?, layout: 1..6}", has_doc, quick_layout),
        cmd!(
            "chart.changeColors",
            "Change Colors",
            ["Chart Design", "Chart Styles"],
            None,
            "{chart?, palette: colorful|monochrome|accent1..accent6 | colors: [hex…]}",
            has_doc,
            change_colors
        ),
        cmd!(
            "chart.selectData",
            "Select Data…",
            ["Chart Design", "Data"],
            None,
            "{chart?, range?: \"Sheet1!A1:D7\", byRows?: bool, categories?: \"Sheet1!A2:A7\" (Horizontal (Category) Axis Labels for every series; \"\" clears), removeSeries?: [index…]}",
            has_doc,
            select_data
        ),
        // Dragging a corner of a selected chart's values outline on the sheet (#4389).
        cmd!(
            "chart.setValuesRange",
            "Resize Chart Data",
            [],
            None,
            "{chart?, range: \"B2:C9\" (the block every series' values come from, one column or row per series, on the data's sheet; names and category labels that line up follow it, series keep their formatting)}",
            has_doc,
            set_values_range
        ),
        cmd!("chart.changeType", "Change Chart Type…", ["Chart Design", "Type"], None, "{chart?, type, subtype?}", has_doc, |s, p| s
            .execute("chart.set", p.clone())),
        cmd!(
            "chart.move",
            "Move Chart…",
            ["Chart Design", "Location"],
            None,
            "{chart?, sheet?: name (moves to that sheet), at?: \"B2\"}",
            has_doc,
            move_chart
        ),
        cmd!(
            "chart.formatSelection",
            "Format Selection",
            ["Format", "Current Selection"],
            None,
            "{chart?, series?: index, color?: hex}",
            has_doc,
            format_series
        ),
        cmd!(
            "chart.shapeFill",
            "Shape Fill",
            ["Format", "Shape Styles"],
            None,
            "{chart? | kind: shape, id?, series?, color: hex}",
            has_doc,
            shape_fill
        ),
        cmd!("chart.shapeOutline", "Shape Outline", ["Format", "Shape Styles"], None, "{kind: shape, id?, color: hex}", has_doc, shape_outline),
        // Arrange
        cmd!(
            "arrange.bringForward",
            "Bring Forward",
            ["Page Layout", "Arrange"],
            None,
            "{kind: chart|image|shape|slicer|equation|control, id, toFront?: bool}",
            has_doc,
            |s, p| arrange(s, p, true)
        ),
        cmd!(
            "arrange.sendBackward",
            "Send Backward",
            ["Page Layout", "Arrange"],
            None,
            "{kind: chart|image|shape|slicer|equation|control, id, toBack?: bool}",
            has_doc,
            |s, p| arrange(s, p, false)
        ),
        cmd!(query "arrange.selectionPane", "Selection Pane", ["Page Layout", "Arrange"], None, "{} → objects on the sheet in stacking order", has_doc, selection_pane),
        cmd!(
            "arrange.align",
            "Align",
            ["Page Layout", "Arrange"],
            None,
            "{kind: chart|image|shape|slicer|equation|control, ids: [..], align: left|center|right|top|middle|bottom}",
            has_doc,
            align
        ),
        cmd!(
            "arrange.group",
            "Group",
            ["Page Layout", "Arrange"],
            None,
            "{kind, ids} (moves objects together: aligns their anchors to the first)",
            has_doc,
            |s, p| align(s, &super::format::merge_json(p.clone(), &json!({"align": "left"})))
        ),
        cmd!(
            "arrange.rotate",
            "Rotate",
            ["Page Layout", "Arrange"],
            None,
            "{kind: shape, id} (swaps width/height for a 90° turn; `flip` is rejected: shapes have no flip state)",
            has_doc,
            rotate
        ),
        // Views
        cmd!(noundo "view.normal", "Normal", ["View", "Workbook Views"], None, "{sheet?} → {view} (per sheet, saved with the file)", has_doc, |s, p| set_view(
            s,
            p,
            SheetViewMode::Normal
        )),
        cmd!(
            noundo "view.pageLayout",
            "Page Layout",
            ["View", "Workbook Views"],
            None,
            "{sheet?} → {view}: pages on paper with rulers; headers and footers are edited in place (per sheet, saved with the file)",
            has_doc,
            |s, p| set_view(s, p, SheetViewMode::PageLayout)
        ),
        cmd!(
            noundo "view.pageBreakPreview",
            "Page Break Preview",
            ["View", "Workbook Views"],
            None,
            "{sheet?} → {view} (per sheet, saved with the file)",
            has_doc,
            |s, p| set_view(s, p, SheetViewMode::PageBreakPreview)
        ),
        cmd!(noundo "view.customViews", "Custom Views…", ["View", "Workbook Views"], None, "{save?: name, show?: name, delete?: name} → list", has_doc, custom_views),
        cmd!(noundo "view.split", "Split", ["View", "Window"], None, "{} (toggles a split at the active cell; shown as frozen panes in this version)", has_doc, split),
        cmd!(noundo "view.newWindow", "New Window", ["View", "Window"], None, "{} → {window, doc, title}: opens another window on the active workbook (the same workbook: saving from either window saves it)", has_doc, new_window),
        cmd!(
            noundo "view.arrangeAll",
            "Arrange All",
            ["View", "Window"],
            None,
            "{arrange?: tiled|horizontal|vertical|cascade, activeWorkbook?: bool} → {arrange, windows}: lays out the visible windows (tiled by default; `activeWorkbook` only the active workbook's), the active window first",
            has_doc,
            arrange_all
        ),
        cmd!(noundo "view.hideWindow", "Hide", ["View", "Window"], None, "{window?, doc?} (the active window by default)", has_doc, hide_window),
        cmd!(noundo "view.unhideWindow", "Unhide…", ["View", "Window"], None, "{window?, doc?, index?} (the most recently hidden window by default)", has_doc, unhide_window),
        cmd!(
            noundo "view.switchWindows",
            "Switch Windows",
            ["View", "Window"],
            None,
            "{window?, doc?} → open windows [{window, doc, index, title, dirty, active, hidden}]; with `window` (an id from the list), or `doc` (a workbook id: its most recent window), shows and activates that window first",
            always,
            switch_windows
        ),
        cmd!(noundo "view.ruler", "Ruler", ["View", "Show"], None, "{on?} → {on}: rulers in Page Layout view (an application option)", has_doc, ruler),
        cmd!(query "view.macros", "Macros", ["View", "Macros"], None, "{} → scripts", has_doc, all_scripts),
        // Table tools
        cmd!("table.resize", "Resize Table", ["Table Design", "Properties"], None, "{table?, range: \"A1:F20\"}", has_doc, table_resize),
        cmd!("table.removeDuplicates", "Remove Duplicates", ["Table Design", "Tools"], None, "{table?, columns?}", has_doc, |s, p| s
            .execute("data.removeDuplicates", p.clone())),
        // Get & Transform
        cmd!(
            "data.fromTextCsv",
            "From Text/CSV",
            ["Data", "Get & Transform Data"],
            None,
            "{path | text, delimiter?: \",\", sheet?: new sheet name} → imports into a new sheet as a table",
            has_doc,
            from_csv
        ),
        cmd!(
            "data.getData",
            "Get Data",
            ["Data", "Get & Transform Data"],
            None,
            "{path | text} (CSV/TSV/JSON array of objects) → new sheet table",
            has_doc,
            from_csv
        ),
        cmd!(
            "data.fromTableRange",
            "From Table/Range",
            ["Data", "Get & Transform Data"],
            None,
            "{range?} → copies the table/range to a new sheet as a table",
            has_doc,
            from_table_range
        ),
        cmd!(query "data.queriesConnections", "Queries & Connections", ["Data", "Queries & Connections"], None, "{} → imported tables", has_doc, queries),
        // Automate
        cmd!(noundo "automate.recordActions", "Record Actions", ["Automate", "Scripting Tools"], None, "{stop?: bool, name?: script name when stopping}", has_doc, record),
        cmd!(
            "automate.newScript",
            "New Script",
            ["Automate", "Scripting Tools"],
            None,
            "{name, commands: [{command, params}] }",
            has_doc,
            new_script
        ),
        cmd!(query "automate.allScripts", "All Scripts", ["Automate", "Scripting Tools"], None, "{} → saved scripts", has_doc, all_scripts),
        cmd!(
            "automate.runScript",
            "Run Script",
            [],
            None,
            "{name, trusted?: bool (runs a script from an untrusted workbook this once)} — refuses scripts in a workbook opened from a file until it is trusted, and steps that reach outside the workbook (file.*, link.*, any `path` param)",
            has_doc,
            run_script
        ),
        cmd!(
            noundo "automate.trustScripts",
            "Trust Scripts",
            ["Automate", "Scripting Tools"],
            None,
            "{trusted?: bool (default true)} → {trusted}: lets this workbook's scripts run for the rest of the session",
            has_doc,
            trust_scripts
        ),
        cmd!("automate.deleteScript", "Delete Script", [], None, "{name}", has_doc, delete_script),
        cmd!(
            noundo "automate.stopAction",
            "Stop Action",
            [],
            None,
            "{} → {stopped: bool}: stops an action left running on the active workbook (an open undo group, such as a debugger's) and puts back the workbook and its undo history as they were when it began",
            has_doc,
            stop_action
        ),
        // Cell controls, themes, borders, styles
        cmd!(
            "insert.checkbox",
            "Checkbox",
            ["Insert", "Cell Controls"],
            None,
            "{range?} (TRUE/FALSE and empty cells draw as checkboxes, empty ones become FALSE, other values show as they are; click or Space toggles; Delete unchecks, then removes; Clear Formats removes and keeps the values)",
            has_doc,
            checkbox
        ),
        cmd!("cell.toggleCheckbox", "Toggle Checkbox", [], Some("Space"), "{cell?}", has_doc, toggle_checkbox),
        cmd!("pageLayout.themeColors", "Colors", ["Page Layout", "Themes"], None, "{name: theme name | colors: [12 hex]}", has_doc, theme_colors),
        cmd!("pageLayout.themeFonts", "Fonts", ["Page Layout", "Themes"], None, "{major, minor}", has_doc, theme_fonts),
        cmd!(
            "pageLayout.themeEffects",
            "Effects",
            ["Page Layout", "Themes"],
            None,
            "{name} (chart effects preset; stored with the theme name)",
            has_doc,
            |_, _| ok()
        ),
        cmd!(
            "pageLayout.background",
            "Background",
            ["Page Layout", "Page Setup"],
            None,
            "{path? | clear?: bool} (sheet background picture)",
            has_doc,
            background
        ),
        cmd!(
            "home.drawBorder",
            "Draw Border",
            ["Home", "Font"],
            None,
            "{range, edge: top|bottom|left|right|grid, style?, color?}",
            has_doc,
            draw_border
        ),
        cmd!(
            "home.newCellStyle",
            "New Cell Style…",
            ["Home", "Styles"],
            None,
            "{name, fromCell?: \"A1\" (default: active cell)}",
            has_doc,
            new_cell_style
        ),
        cmd!(query "home.addIns", "Add-ins", ["Home", "Add-ins"], None, "{} → GridCraft extends through MCP and scripts instead of add-ins", always, |_, _| Ok(json!({"addIns": [], "hint": "Use gridcraft-cli mcp or Automate › scripts"}))),
        cmd!(query "formulas.recentlyUsed", "Recently Used", ["Formulas", "Function Library"], None, "{} → the functions most recently entered in formulas, newest first", has_doc, recently_used),
        cmd!(query "formulas.financial", "Financial", ["Formulas", "Function Library"], None, "{}", always, |s, _| s.execute("formulas.functions", json!({"category": "Financial"}))),
        cmd!(query "formulas.logical", "Logical", ["Formulas", "Function Library"], None, "{}", always, |s, _| s.execute("formulas.functions", json!({"category": "Logical"}))),
        cmd!(query "formulas.text", "Text", ["Formulas", "Function Library"], None, "{}", always, |s, _| s.execute("formulas.functions", json!({"category": "Text"}))),
        cmd!(query "formulas.dateTime", "Date & Time", ["Formulas", "Function Library"], None, "{}", always, |s, _| s.execute("formulas.functions", json!({"category": "DateTime"}))),
        cmd!(query "formulas.lookupReference", "Lookup & Reference", ["Formulas", "Function Library"], None, "{}", always, |s, _| s.execute("formulas.functions", json!({"category": "Lookup"}))),
        cmd!(query "formulas.mathTrig", "Math & Trig", ["Formulas", "Function Library"], None, "{}", always, |s, _| s.execute("formulas.functions", json!({"category": "MathTrig"}))),
        cmd!(query "formulas.moreFunctions", "More Functions", ["Formulas", "Function Library"], None, "{category?: Statistical|Engineering|Information|Database|Compatibility|Web|Cube}", always, |s, p| s.execute("formulas.functions", json!({"category": str_param(p, "category").unwrap_or("Statistical")}))),
        cmd!(
            "formulas.useInFormula",
            "Use in Formula",
            ["Formulas", "Defined Names"],
            None,
            "{name} (inserts the name into the cell being edited)",
            has_doc,
            use_in_formula
        ),
        cmd!(query "formulas.watchWindow", "Watch Window", ["Formulas", "Formula Auditing"], None, "{add?: \"Sheet1!B5\", remove?: \"…\"} → watched cells with current values", has_doc, watch_window),
    ]
}

const VIEWS_NAME: &str = "_sc_custom_views";
const SCRIPTS_NAME: &str = "_sc_scripts";
const WATCH_NAME: &str = "_sc_watch";

/// Workbook-level JSON stored in a hidden defined name (round-trips through XLSX).
fn stash_get(wb: &Workbook, key: &str) -> Json {
    wb.names
        .iter()
        .find(|n| n.name == key)
        .and_then(|n| n.formula.strip_prefix('"').and_then(|t| t.strip_suffix('"')).map(|t| t.replace("\"\"", "\"")))
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(json!({}))
}

/// Like `stash_get`, for the name-keyed lists (views, scripts): a stored value that is not a JSON object is treated as empty.
fn stash_map(wb: &Workbook, key: &str) -> Json {
    let v = stash_get(wb, key);
    if v.is_object() { v } else { json!({}) }
}

fn stash_set(wb: &mut Workbook, key: &str, v: &Json) {
    let text = v.to_string();
    let formula = format!("\"{}\"", text.replace('"', "\"\""));
    wb.names.retain(|n| n.name != key);
    wb.names.push(DefinedName { name: key.into(), scope: None, formula, comment: String::new(), hidden: true });
}

/// Number format that marks checkbox cells (TRUE/FALSE values drawn as boxes).
pub const CHECKBOX_FORMAT: &str = "\"☐\";\"☐\";\"☐\";[=1]\"☑\"";

/// Copy of the params that is always a JSON object, so keys can be assigned; any other JSON value counts as no params (like null).
fn object_params(p: &Json) -> Json {
    Json::Object(p.as_object().cloned().unwrap_or_default())
}
