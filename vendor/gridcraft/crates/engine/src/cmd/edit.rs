//! Cell entry, selection, undo/redo, clipboard, clear, fill, find & replace.

use gridcraft_core::{CellRef, MAX_ROWS, RangeRef, Value};
use gridcraft_model::{Cell, Formula, StyleId};
use serde_json::{Value as Json, json};

use super::{
    CommandSpec, EngineError, Result, bad, bool_param, can_redo, can_undo, cell_param, cell_param_on, cmd, edit, f64_param, has_clipboard, has_doc,
    locked_cell_error, ok, split_sheet, str_param, target_range, target_ranges, target_sheet, u32_param,
};
use crate::selection::{current_region, jump};
use crate::{Clipboard, Session};

mod cell;
mod clipboard;
mod fill;
mod find;
mod go_to;
mod paste_special;
mod selection;

use cell::{cell_set, range_fill, range_set_values};
pub(crate) use cell::{check_arg_counts, input_to_cell, is_formula_input, json_to_input, parse_typed_formula};
#[cfg(test)]
pub(crate) use clipboard::parse_tsv;
use clipboard::{clear_clipboard, copy, cut, paste, range_move};
use fill::{auto_fill, fill_dir, fill_series, flash_fill};
#[cfg(test)]
use find::replace_ci;
pub(crate) use find::wildcard;
use find::{find, replace};
pub(super) use go_to::go_to;
pub use go_to::go_to_names;
use go_to::go_to_special;
#[cfg(test)]
use go_to::{GO_TO_MAX_AREAS, GO_TO_RECENT, reference_parts};
use paste_special::paste_special;
pub(crate) use paste_special::{place_moved_rules, rewrite_all_formulas_many, rewrite_python_refs};
use selection::{select_all, select_col, select_region, select_row, selection_move, selection_next, selection_next_corner, selection_set};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("cell.set", "Enter Cell", [], None, "{cell?: \"B2\", input: \"text, number or =formula\", sheet?, array?: bool}", has_doc, cell_set),
        cmd!(
            "range.setValues",
            "Set Values",
            [],
            None,
            "{range?: \"A1\", values: [[...]] (rows of numbers/strings/bools/null; strings starting with = are formulas), parse?: bool (default true: a string goes through typed input, so \"05.04.2026\" becomes a date and \"2.500\" a number, by en-US rules; false stores every other string as the text it is, so a dd.mm.yyyy date or a thousands separator survives, a leading ' is kept and an empty string clears the cell)} → {cells, convertedCount, converted: [{cell, input, stored}] (up to 20 of the strings typed input read as a number, date, time, boolean or error, so a script can notice)}",
            has_doc,
            range_set_values
        ),
        cmd!(
            "range.fill",
            "Fill Range With Input",
            [],
            Some("Ctrl+Enter"),
            "{range?, input}: enters the same input in every selected cell (relative formulas adjust)",
            has_doc,
            range_fill
        ),
        cmd!(noundo "selection.set", "Select", [], None, "{range: \"A1:B2,D4\" | cell, active?: \"A1\", sheet?}", has_doc, selection_set),
        cmd!(noundo "selection.move", "Move Selection", [], None, "{dr, dc, extend?: bool, jump?: bool (Ctrl+arrow), page?: bool}", has_doc, selection_move),
        cmd!(noundo "selection.next", "Next Cell in Selection", [], None, "{forward?: true, byRow?: false} (Enter/Tab inside a selection)", has_doc, selection_next),
        cmd!(
            noundo "selection.nextCorner",
            "Next Corner of Selection",
            [],
            Some("Ctrl+."),
            "{} (the active cell moves clockwise to the next corner of the selected area, which stays selected)",
            has_doc,
            selection_next_corner
        ),
        cmd!(noundo "edit.selectAll", "Select All", ["Edit"], Some("Cmd+A"), "{} (current region first, then the whole sheet)", has_doc, select_all),
        cmd!(noundo "selection.currentRegion", "Select Current Region", [], Some("Ctrl+Shift+8"), "{}", has_doc, select_region),
        cmd!(noundo "selection.row", "Select Entire Row", [], Some("Shift+Space"), "{}", has_doc, select_row),
        cmd!(noundo "selection.column", "Select Entire Column", [], Some("Ctrl+Space"), "{}", has_doc, select_col),
        cmd!(noundo "edit.goTo", "Go To…", ["Home", "Editing", "Find & Select"], Some("Ctrl+G"), "{reference: \"B5\" | \"Sheet2!A1:C3\" | name | \"Sheet2!Name\" | \"B14:C22,F19:G30\" | \"Price, Type\", record?: bool} (record: keep a reference without names among the recent ones) or {list: true} → {names, recent}", has_doc, go_to),
        cmd!(noundo "edit.goToSpecial", "Go To Special…", ["Home", "Editing", "Find & Select"], None, "{kind: blanks|constants|formulas|comments|lastCell|currentRegion|visible|conditionalFormats|dataValidation|errors|numbers|text}", has_doc, go_to_special),
        cmd!(noundo "edit.undo", "Undo", ["Edit"], Some("Cmd+Z"), "{steps?: 1}", can_undo, undo),
        cmd!(noundo "edit.redo", "Redo", ["Edit"], Some("Cmd+Y"), "{steps?: 1}", can_redo, redo),
        cmd!(noundo "edit.copy", "Copy", ["Home", "Clipboard"], Some("Cmd+C"), "{range?, html?: bool} → {range, text, html?} (html: a styled HTML table, only when requested and small enough)", has_doc, copy),
        cmd!(noundo "edit.cut", "Cut", ["Home", "Clipboard"], Some("Cmd+X"), "{range?, html?: bool} → {range, text, html?} (html: a styled HTML table, only when requested and small enough)", has_doc, cut),
        cmd!(
            "edit.paste",
            "Paste",
            ["Home", "Clipboard"],
            Some("Cmd+V"),
            "{at?: \"C3\", text?: \"tab-separated text from the system clipboard\", html?: \"the clipboard's HTML flavour; cells the source recorded as numbers (x:num, sdval) stay numbers\"}",
            has_doc,
            paste
        ),
        cmd!(
            "range.move",
            "Drag and Drop",
            [],
            None,
            "{range?, to: \"C3\", overwrite?: bool} (drag the selection's border: a cut and paste that asks before replacing data unless overwrite)",
            has_doc,
            range_move
        ),
        CommandSpec {
            shortcut_mac: Some("Ctrl+Cmd+V"),
            ..cmd!(
                "edit.pasteSpecial",
                "Paste Special…",
                ["Home", "Clipboard"],
                Some("Ctrl+Alt+V"),
                "{what: all|formulas|values|formats|comments|validation|allExceptBorders|columnWidths|formulasAndNumberFormats|valuesAndNumberFormats, operation?: none|add|subtract|multiply|divide, skipBlanks?, transpose?, link?}",
                has_clipboard,
                paste_special
            )
        },
        cmd!(noundo "edit.clearClipboard", "Cancel Copy", [], Some("Escape"), "{}", has_doc, clear_clipboard),
        cmd!("edit.clearAll", "Clear All", ["Home", "Editing", "Clear"], None, "{range?}", has_doc, |s, p| clear(s, p, "all")),
        cmd!("edit.clearFormats", "Clear Formats", ["Home", "Editing", "Clear"], None, "{range?}", has_doc, |s, p| clear(s, p, "formats")),
        cmd!("edit.clearContents", "Clear Contents", ["Home", "Editing", "Clear"], Some("Delete"), "{range?}", has_doc, |s, p| clear(
            s, p, "contents"
        )),
        cmd!("edit.clearComments", "Clear Comments and Notes", ["Home", "Editing", "Clear"], None, "{range?}", has_doc, |s, p| clear(
            s, p, "comments"
        )),
        cmd!("edit.clearHyperlinks", "Clear Hyperlinks", ["Home", "Editing", "Clear"], None, "{range?}", has_doc, |s, p| clear(s, p, "hyperlinks")),
        cmd!("edit.fillDown", "Fill Down", ["Home", "Editing", "Fill"], Some("Cmd+D"), "{range?}", has_doc, |s, p| fill_dir(s, p, 1, 0)),
        cmd!("edit.fillRight", "Fill Right", ["Home", "Editing", "Fill"], Some("Cmd+R"), "{range?}", has_doc, |s, p| fill_dir(s, p, 0, 1)),
        cmd!("edit.fillUp", "Fill Up", ["Home", "Editing", "Fill"], None, "{range?}", has_doc, |s, p| fill_dir(s, p, -1, 0)),
        cmd!("edit.fillLeft", "Fill Left", ["Home", "Editing", "Fill"], None, "{range?}", has_doc, |s, p| fill_dir(s, p, 0, -1)),
        cmd!(
            "edit.autoFill",
            "AutoFill",
            [],
            None,
            "{source: \"A1:A2\", target: \"A1:A10\", mode?: series|alternate|copy|formats|values|extend|days|weekdays|months|years}: the fill-handle drag (alternate = Ctrl+drag: a lone number counts up, any other series copies; values = Fill Without Formatting: the series, targets keep their formats; extend = Fill Series: the series, and a lone number counts up; days, weekdays, months, years = Fill Days, Fill Weekdays, Fill Months, Fill Years: dates step by one unit, or by the difference between the first two dates in that unit, and weekdays skip Saturday and Sunday)",
            has_doc,
            auto_fill
        ),
        cmd!(
            "edit.fillSeries",
            "Series…",
            ["Home", "Editing", "Fill"],
            None,
            "{range?, direction?: columns|rows, type?: linear|growth|date|autofill, step?: 1, stop?, dateUnit?: day|weekday|month|year}",
            has_doc,
            fill_series
        ),
        cmd!("edit.flashFill", "Flash Fill", ["Home", "Editing", "Fill"], Some("Ctrl+E"), "{range?}", has_doc, flash_fill),
        cmd!(noundo "edit.find", "Find…", ["Home", "Editing", "Find & Select"], Some("Cmd+F"), "{what, matchCase?, wholeCell?, lookIn?: formulas|values, byColumns?, all?: bool, within?: sheet|workbook|selection} (selection: only the selected cells; Find Next keeps them selected and moves the active cell)", has_doc, find),
        cmd!(
            "edit.replace",
            "Replace…",
            ["Home", "Editing", "Find & Select"],
            Some("Ctrl+H"),
            "{what, with, matchCase?, wholeCell?, all?: true, within?: sheet|workbook|selection} → {replaced, skipped?} (in a Python cell the code is edited; skipped counts Python cells whose code would no longer fit)",
            has_doc,
            replace
        ),
        cmd!(
            "edit.formatPainter",
            "Format Painter",
            ["Home", "Clipboard"],
            None,
            "{sticky?: bool} — first call picks up the selection's formats; with {apply: \"C3:D4\"} pastes them",
            has_doc,
            format_painter
        ),
        cmd!(noundo "edit.beginEdit", "Edit Cell", [], Some("F2"), "{}", has_doc, begin_edit),
    ]
}

/// Merges cells into vertical runs per column (a compact multi-area selection).
fn coalesce(cells: &[CellRef]) -> Vec<RangeRef> {
    let mut sorted = cells.to_vec();
    sorted.sort_by_key(|c| (c.col, c.row));
    let mut out: Vec<RangeRef> = Vec::new();
    for c in sorted {
        if let Some(last) = out.last_mut()
            && last.start.col == c.col
            && last.end.col == c.col
            && last.end.row + 1 == c.row
        {
            last.end.row = c.row;
            continue;
        }
        out.push(RangeRef::cell(c));
    }
    if out.len() > 2000 {
        out.truncate(2000);
    }
    out
}

// ---------------------------------------------------------------- history

fn undo(s: &mut Session, p: &Json) -> Result<Json> {
    let steps = u32_param(p, "steps").unwrap_or(1).max(1);
    let d = s.doc_mut()?;
    let mut label = String::new();
    let now = d.wb.clone();
    for _ in 0..steps {
        let Some(e) = d.undo.pop() else { break };
        let cur_wb = d.wb.clone();
        // Redo goes back to the undone edit's cells, not wherever the user clicked before undoing (#1855).
        let cur_selection = e.selection.clone();
        label = e.label.clone();
        d.wb = e.wb;
        d.selection = e.selection;
        // Step by step: a rename and a new sheet undone together match up only one step apart.
        super::extra::keep_view_modes(d, &cur_wb);
        crate::push_history(&mut d.redo, label.clone(), cur_wb, &d.wb, cur_selection);
    }
    super::extra::keep_watch_list(&now, &mut d.wb);
    d.calc.rebuild(&d.wb);
    d.revision += 1;
    Ok(json!({"undone": label}))
}

fn redo(s: &mut Session, p: &Json) -> Result<Json> {
    let steps = u32_param(p, "steps").unwrap_or(1).max(1);
    let d = s.doc_mut()?;
    let mut label = String::new();
    let now = d.wb.clone();
    for _ in 0..steps {
        let Some(e) = d.redo.pop() else { break };
        let cur_wb = d.wb.clone();
        let cur_selection = d.selection.clone();
        label = e.label.clone();
        d.wb = e.wb;
        d.selection = e.selection;
        super::extra::keep_view_modes(d, &cur_wb);
        crate::push_history(&mut d.undo, label.clone(), cur_wb, &d.wb, cur_selection);
    }
    super::extra::keep_watch_list(&now, &mut d.wb);
    d.calc.rebuild(&d.wb);
    d.revision += 1;
    Ok(json!({"redone": label}))
}

// ---------------------------------------------------------------- clear

/// The rows of `range` an applied filter hides (rows hidden by hand are not among them).
fn filtered_rows(sh: &gridcraft_model::Sheet, range: RangeRef) -> std::collections::BTreeSet<u32> {
    let spans = sh.filter_row_spans();
    if spans.is_empty() {
        return Default::default();
    }
    let filtered = |row: u32| spans.iter().any(|&(first, last)| row >= first && row <= last);
    sh.rows.range(range.start.row..=range.end.row).filter(|(r, info)| info.hidden && filtered(**r)).map(|(r, _)| *r).collect()
}

/// The Clear commands (Delete is Clear Contents). As in Excel, the rows an applied filter hides
/// are left as they are; rows hidden by hand are cleared.
fn clear(s: &mut Session, p: &Json, what: &str) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let ranges = target_ranges(s, p)?;
    // Delete on checkboxes works in two passes (as in Excel): checked boxes are unchecked first,
    // and only when every box in the selection is unchecked are the boxes removed.
    let uncheck_only = what == "contents" && {
        let d = s.doc()?;
        d.wb.sheet(sheet).is_some_and(|sh| {
            ranges.iter().any(|r| {
                r.intersection(&sh.used_range().unwrap_or(RangeRef::cell(r.start))).is_some_and(|r| {
                    let skip = filtered_rows(sh, r);
                    sh.cells
                        .iter_range(r)
                        .any(|(c, x)| !skip.contains(&c.row) && x.value == Value::Bool(true) && super::extra::is_checkbox_cell(&d.wb, sh, c))
                })
            })
        })
    };
    // Notes and links aren't cell contents, so the cell comparison that guards a protected sheet
    // doesn't see them go: a locked cell's are refused here, stored cell or not.
    if matches!(what, "all" | "comments" | "hyperlinks") {
        let d = s.doc()?;
        if let Some(sh) = d.wb.sheet(sheet).filter(|sh| sh.is_protected()) {
            for &whole in &ranges {
                let skip = filtered_rows(sh, whole);
                let notes = matches!(what, "all" | "comments").then(|| sh.comments.keys()).into_iter().flatten();
                let links = matches!(what, "all" | "hyperlinks").then(|| sh.hyperlinks.keys()).into_iter().flatten();
                for &c in notes.chain(links).filter(|c| whole.contains(**c) && !skip.contains(&c.row)) {
                    if d.wb.styles.get(sh.style_id(c)).protection.locked
                        && let Some(m) = locked_cell_error(sh, c, &d.unlocked_ranges)
                    {
                        return Err(EngineError::Other(m));
                    }
                }
            }
        }
    }
    edit(s, |cx| {
        for &whole in &ranges {
            // Cells are only stored inside the used range. Notes and links are kept apart from
            // them, so they are cleared over the whole range: one from a file can sit on a cell
            // with nothing stored in it.
            let used = cx.wb.sheet(sheet).and_then(|sh| sh.used_range()).and_then(|u| whole.intersection(&u));
            let skip = cx.wb.sheet(sheet).map(|sh| filtered_rows(sh, whole)).unwrap_or_default();
            let cells: Vec<(CellRef, Cell)> = match (cx.wb.sheet(sheet), used) {
                (Some(sh), Some(r)) => sh.cells.iter_range(r).filter(|(c, _)| !skip.contains(&c.row)).map(|(c, x)| (c, x.clone())).collect(),
                _ => Vec::new(),
            };
            // Report every stored cell we touch, even beyond a million coordinates in a sparse
            // selection. Removed formula anchors must also reach calc so their spills disappear.
            cx.changed.extend(cells.iter().map(|(c, _)| (sheet, *c)));
            let mut cells = cells;
            if what == "contents" {
                for (_, cell) in cells.iter_mut() {
                    let checkbox = cx.wb.styles.get(cell.style).num_fmt.as_str() == "checkbox";
                    cell.formula = None;
                    cell.py = None;
                    if checkbox && uncheck_only {
                        cell.value = Value::Bool(false);
                    } else {
                        cell.value = Value::Empty;
                        if checkbox {
                            cell.style = cx.wb.styles.derive(cell.style, |st| st.num_fmt = gridcraft_model::NumFmt::new("General"));
                        }
                    }
                }
            }
            let cleared = |c: CellRef| whole.contains(c) && !skip.contains(&c.row);
            // A removed note or link changes its cell even with nothing stored there.
            let metadata: Vec<CellRef> = cx
                .wb
                .sheet(sheet)
                .map(|sh| {
                    let notes = matches!(what, "all" | "comments").then(|| sh.comments.keys()).into_iter().flatten();
                    let links = matches!(what, "all" | "hyperlinks").then(|| sh.hyperlinks.keys()).into_iter().flatten();
                    notes.chain(links).copied().filter(|c| cleared(*c)).collect()
                })
                .unwrap_or_default();
            cx.changed.extend(metadata.into_iter().map(|c| (sheet, c)));
            let sh = cx.sheet_mut(sheet)?;
            for (c, mut cell) in cells {
                match what {
                    "all" => {
                        sh.remove_cell(c);
                    }
                    "contents" => {
                        sh.set_cell(c, cell);
                    }
                    "formats" => {
                        cell.style = StyleId::DEFAULT;
                        sh.cells.set(c, cell);
                    }
                    _ => {}
                }
            }
            if matches!(what, "all" | "comments") {
                sh.comments.retain(|c, _| !cleared(*c));
            }
            if matches!(what, "all" | "hyperlinks") {
                sh.hyperlinks.retain(|c, _| !cleared(*c));
            }
            if let Some(r) = used
                && matches!(what, "all" | "formats")
            {
                let mut unmerged = false;
                sh.merges.retain(|m| {
                    let keep = !r.contains_range(m) || skip.range(m.start.row..=m.end.row).next().is_some();
                    unmerged |= !keep;
                    keep
                });
                if unmerged {
                    // An empty merge can block a spill without storing any cells in this range.
                    // As in Unmerge, retry blocked formula anchors that could reach the freed area.
                    let reach = RangeRef::new(CellRef::new(0, 0), r.end);
                    let blocked: Vec<_> = sh
                        .cells
                        .iter_range(reach)
                        .filter(|(_, cell)| cell.formula.is_some() && cell.value == Value::Error(gridcraft_core::CellError::Spill))
                        .map(|(c, _)| (sheet, c))
                        .collect();
                    cx.changed.extend(blocked);
                }
            }
        }
        Ok(Json::Null)
    })
}

// ---------------------------------------------------------------- format painter

fn format_painter(s: &mut Session, p: &Json) -> Result<Json> {
    if let Some(dest) = str_param(p, "apply").and_then(RangeRef::parse) {
        let Some((wb, sheet, src, sticky)) = s.format_painter.clone() else { return Err(EngineError::Other("Pick up a format first.".into())) };
        let dest_sheet = s.doc()?.wb.active_sheet();
        let same = std::sync::Arc::ptr_eq(&wb, &s.doc()?.wb) || s.doc()?.wb.styles == wb.styles;
        edit(s, |cx| {
            let Some(src_sheet) = wb.sheet(sheet) else { return Ok(()) };
            let (h, w) = (src.height(), src.width());
            let dest = if dest.is_single() { RangeRef::new(dest.start, dest.start.offset_clamped(h as i64 - 1, w as i64 - 1)) } else { dest };
            // Whole rows or columns take a line format, as the ribbon's formatting commands give
            // them, instead of a style on each of their cells (a column alone has a million). The
            // line format comes from the first cell of the source pattern across the line; cells
            // already stored follow the pattern. Row heights and column widths stay as they are.
            let rows = dest.is_full_rows() && !dest.is_full_cols();
            let cols = dest.is_full_cols() && !dest.is_full_rows();
            let src_full = (rows && src.is_full_rows()) || (cols && src.is_full_cols());
            if rows || cols {
                let (first, src_first, n) = if rows { (dest.start.row, src.start.row, h) } else { (dest.start.col, src.start.col, w) };
                let lines = if rows { dest.start.row..=dest.end.row } else { dest.start.col..=dest.end.col };
                for line in lines {
                    let sl = src_first + (line - first) % n;
                    // The source cell for position `k` along the line.
                    let at = |k: u32| if rows { CellRef::new(sl, src.start.col + k % w) } else { CellRef::new(src.start.row + k % h, sl) };
                    let line_style = if src_full {
                        if rows { src_sheet.rows.get(&sl) } else { src_sheet.cols.get(&sl) }.and_then(|i| i.style)
                    } else {
                        Some(src_sheet.style_id(at(0)))
                    };
                    let line_style = line_style.map(|st| if same { st } else { cx.wb.styles.intern(wb.styles.get(st).clone()) });
                    let (dest_line, src_line) = if rows {
                        (RangeRef::rows(line, line), RangeRef::rows(sl, sl))
                    } else {
                        (RangeRef::cols(line, line), RangeRef::cols(sl, sl))
                    };
                    let mut cells: Vec<CellRef> =
                        cx.wb.sheet(dest_sheet).map(|sh| sh.cells.iter_range(dest_line).map(|(c, _)| c).collect()).unwrap_or_default();
                    if src_full {
                        cells.extend(
                            src_sheet
                                .cells
                                .iter_range(src_line)
                                .map(|(c, _)| if rows { CellRef::new(line, c.col) } else { CellRef::new(c.row, line) }),
                        );
                    }
                    if cols {
                        // A row format outranks a column format: the column's cells in formatted
                        // rows need a style of their own.
                        cells.extend(
                            cx.wb
                                .sheet(dest_sheet)
                                .into_iter()
                                .flat_map(|sh| sh.rows.iter().filter(|(_, i)| i.style.is_some()).map(|(r, _)| CellRef::new(*r, line))),
                        );
                    }
                    let sh = cx.sheet_mut(dest_sheet)?;
                    (if rows { &mut sh.rows } else { &mut sh.cols }).entry(line).or_default().style = line_style;
                    for c in cells {
                        let st = src_sheet.style_id(at(if rows { c.col } else { c.row }));
                        let st = if same { st } else { cx.wb.styles.intern(wb.styles.get(st).clone()) };
                        cx.sheet_mut(dest_sheet)?.set_style(c, st);
                        cx.touch(dest_sheet, c);
                    }
                }
                *cx.sel = crate::Selection::range(dest);
                return Ok(());
            }
            for c in dest.iter().take(2_000_000) {
                let sc = CellRef::new(src.start.row + (c.row - dest.start.row) % h, src.start.col + (c.col - dest.start.col) % w);
                let st = src_sheet.style_id(sc);
                let st = if same { st } else { cx.wb.styles.intern(wb.styles.get(st).clone()) };
                cx.sheet_mut(dest_sheet)?.set_style(c, st);
                cx.touch(dest_sheet, c);
            }
            *cx.sel = crate::Selection::range(dest);
            Ok(())
        })?;
        if !sticky {
            s.format_painter = None;
        }
        return ok();
    }
    let sticky = bool_param(p, "sticky").unwrap_or(false);
    let d = s.doc()?;
    s.format_painter = Some((d.wb.clone(), d.wb.active_sheet(), d.selection.current(), sticky));
    ok()
}

fn begin_edit(s: &mut Session, _: &Json) -> Result<Json> {
    s.ui_requests.push(crate::UiRequest::EditCell(None));
    ok()
}

#[cfg(test)]
mod tests;
