use super::{
    Cell, CellRef, EngineError, Formula, Json, MAX_ROWS, RangeRef, Result, Session, StyleId, Value, bad, bool_param, cell_param_on, edit, json,
    locked_cell_error, str_param, target_range, target_ranges, target_sheet,
};

// ---------------------------------------------------------------- entry

/// Typing `=PY(` doesn't make a Python cell; this says what does.
pub(crate) const PY_FORMULA: &str = "Python cells are made with the python.set command (the code as `code`), not by typing =PY(…).";

/// Whether typing `input` into a cell enters a formula: `=…`, or `+…`/`-…` followed by a name or
/// a parenthesis (not a number), as Excel takes `+SUM(1)`.
pub(crate) fn is_formula_input(input: &str) -> bool {
    input.starts_with('=')
        || (input.len() > 1
            && (input.starts_with('+') || input.starts_with('-'))
            && gridcraft_core::parse::parse_number_text(input).is_none()
            && input.chars().nth(1).is_some_and(|c| c.is_ascii_alphabetic() || c == '('))
}

/// What typing `input` into a cell produces (constant or formula, plus an automatic number
/// format). Inherits the destination's cell, row or column style, including its number
/// format, before parsing. Errors when the formula can't be parsed.
pub(crate) fn input_to_cell(input: &str, sheet: usize, at: CellRef, wb: &mut gridcraft_model::Workbook) -> Result<Option<Cell>> {
    let style = wb.sheet(sheet).map(|sh| sh.style_id(at)).unwrap_or_default();
    if input.is_empty() {
        let c = Cell { value: Value::Empty, formula: None, style, py: None };
        return Ok(if c.is_blank() { None } else { Some(c) });
    }
    let fmt_is_text = wb.styles.get(style).num_fmt.as_str() == "@";
    if fmt_is_text {
        return Ok(Some(Cell { value: Value::text(input), formula: None, style, py: None }));
    }
    let is_formula = is_formula_input(input);
    if is_formula && gridcraft_model::python::is_py_formula(input) {
        return Err(EngineError::Other(PY_FORMULA.into()));
    }
    if is_formula {
        let body = input.strip_prefix('=').unwrap_or(input);
        let expr = wb.name_call_case(parse_typed_formula(body)?);
        check_arg_counts(&expr)?;
        let mut cell = Cell::formula(Formula::from_expr(expr));
        cell.style = style;
        // Formulas whose result is a date/time get a format from functions like TODAY().
        if wb.styles.get(style).num_fmt.as_str() == "General" {
            let f = cell.formula.as_ref().map(|f| f.text.to_ascii_uppercase()).unwrap_or_default();
            let auto = if f.starts_with("TODAY(")
                || f.starts_with("DATE(")
                || f.starts_with("EDATE(")
                || f.starts_with("EOMONTH(")
                || f.starts_with("WORKDAY(")
            {
                Some("m/d/yyyy")
            } else if f.starts_with("NOW(") {
                Some("m/d/yyyy h:mm")
            } else if f.starts_with("TIME(") {
                Some("h:mm AM/PM")
            } else {
                None
            };
            if let Some(code) = auto {
                cell.style = wb.styles.derive(style, |s| s.num_fmt = gridcraft_model::NumFmt::new(code));
            }
        }
        return Ok(Some(cell));
    }
    let mut parsed = gridcraft_core::parse::parse_input(input, wb.date_system);
    // Like Excel's automatic percent entry: a bare number of 1 or more typed into a cell already
    // formatted as a percentage is a percent (3 -> 3%); smaller numbers are taken as fractions.
    if parsed.format.is_none()
        && !input.contains('%')
        && wb.styles.get(style).num_fmt.as_str().ends_with('%')
        && let Value::Number(n) = parsed.value
        && n.abs() >= 1.0
    {
        parsed.value = Value::Number(n / 100.0);
    }
    let mut style = style;
    if let Some(code) = parsed.format {
        let cur = wb.styles.get(style).num_fmt.as_str().to_string();
        if cur == "General" {
            style = wb.styles.derive(style, |s| s.num_fmt = gridcraft_model::NumFmt::new(code));
        }
    }
    // Wrap text automatically when the input has line breaks (Alt+Enter).
    if input.contains('\n') && !wb.styles.get(style).align.wrap {
        style = wb.styles.derive(style, |s| s.align.wrap = true);
    }
    Ok(Some(Cell { value: parsed.value, formula: None, style, py: None }))
}

/// Parses a formula as it is typed (without the `=`). Excel closes missing parentheses for you,
/// and refuses a formula that still doesn't parse.
pub(crate) fn parse_typed_formula(body: &str) -> Result<gridcraft_formula::Expr> {
    // Excel closes missing parentheses for you.
    let parsed = gridcraft_formula::parse_closing_parens(body, gridcraft_formula::parse);
    parsed.map_err(|e| EngineError::Other(format!("There's a problem with this formula: {e}")))
}

/// Like Excel, a formula that calls a built-in function with too few or too many arguments is
/// refused when it's entered, rather than stored to show `#VALUE!`. Functions the registry doesn't
/// hold (special forms like LET, LAMBDA, OFFSET, INDIRECT, SUBTOTAL), defined LAMBDA names and
/// unknown functions are skipped.
pub(crate) fn check_arg_counts(expr: &gridcraft_formula::Expr) -> Result<()> {
    let mut bad = None;
    expr.walk(&mut |e| {
        if bad.is_none()
            && let gridcraft_formula::Expr::Call(name, args) = e
            && let Some(spec) = gridcraft_functions::lookup(name)
            && !spec.accepts(args.len())
        {
            let few = args.len() < spec.min_args;
            bad = Some(format!("{} has too {} arguments. Use {}", spec.name, if few { "few" } else { "many" }, spec.signature));
        }
    });
    match bad {
        Some(msg) => Err(EngineError::Other(msg)),
        None => Ok(()),
    }
}

pub(super) fn cell_set(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let at = match cell_param_on(s, p, "cell", sheet)? {
        Some(c) => c,
        None => s.doc()?.selection.active,
    };
    // Both spellings take a string, number or boolean, as typed (#697).
    let input = p.get("input").or_else(|| p.get("value")).map(json_to_input).ok_or_else(|| bad("cell.set", "missing `input` (or `value`)"))?;
    let array = bool_param(p, "array").unwrap_or(false);
    let d = s.doc()?;
    if let Some(sh) = d.wb.sheet(sheet)
        && sh.is_protected()
        && d.wb.styles.get(sh.style_id(at)).protection.locked
        && let Some(m) = locked_cell_error(sh, at, &d.unlocked_ranges)
    {
        return Err(EngineError::Other(m));
    }
    let active_sheet = s.doc()?.wb.active_sheet();
    edit(s, |cx| {
        if array {
            // Legacy Ctrl+Shift+Enter array formula over the selection, when the selection holds
            // the cell; a cell elsewhere (or on another sheet) gets a one-cell array.
            let selected = cx.sel.current();
            let range = if sheet == active_sheet && selected.contains(at) { selected } else { RangeRef::cell(at) };
            // A table header can't hold an array's value, and a table resizes under it.
            if !range.is_single() && cx.wb.sheet(sheet).is_some_and(|sh| sh.tables.iter().any(|t| t.range.intersects(&range))) {
                return Err(EngineError::Other("Multi-cell array formulas aren't allowed in tables.".into()));
            }
            let mut cell = input_to_cell(&input, sheet, range.start, &mut cx.wb)?.unwrap_or_default();
            if let Some(f) = cell.formula.as_mut() {
                std::sync::Arc::make_mut(f).array = Some(range);
            }
            let sh = cx.sheet_mut(sheet)?;
            // The array takes the whole range: other cells in it lose their contents.
            for c in range.iter().skip(1).take(1_000_000) {
                if let Some(old) = sh.cells.get(c).filter(|x| x.has_content()) {
                    let style = old.style;
                    sh.set_cell(c, Cell { style, ..Cell::default() });
                }
            }
            sh.set_cell(range.start, cell);
            cx.touch(sheet, range.start);
            return Ok(Json::Null);
        }
        let cell = input_to_cell(&input, sheet, at, &mut cx.wb)?;
        let sh = cx.sheet_mut(sheet)?;
        match cell {
            Some(c) => sh.set_cell(at, c),
            None => {
                sh.remove_cell(at);
            }
        }
        crate::cmd::data::grow_autofilter(sh, at);
        cx.touch(sheet, at);
        Ok(Json::Null)
    })?;
    let v = s.doc()?.wb.sheet(sheet).map(|sh| sh.value(at)).unwrap_or_default();
    Ok(json!({"cell": at.a1(), "value": v.display()}))
}

pub(crate) fn json_to_input(v: &Json) -> String {
    match v {
        Json::Null => String::new(),
        Json::Bool(b) => {
            if *b {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        Json::Number(n) => n.to_string(),
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// How many converted strings `range.setValues` reports back, and how much of each input and
/// stored text: the report is a diagnostic, so a big write must not put a second copy of its data
/// in the result.
const MAX_REPORTED_CONVERSIONS: usize = 20;
const MAX_REPORTED_CHARS: usize = 32;

/// `text` cut to [`MAX_REPORTED_CHARS`] characters for the conversion report, with `…` when it was
/// cut. Counting characters keeps a multi-byte input whole.
fn clipped(text: &str) -> String {
    let mut out: String = text.chars().take(MAX_REPORTED_CHARS).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

pub(super) fn range_set_values(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let start = target_range(s, p)?.start;
    let Some(rows) = p.get("values").and_then(Json::as_array) else { return Err(bad("range.setValues", "missing `values` (array of rows)")) };
    // A string is run through typed input by default, which reads it with en-US rules: a caller
    // sending "05.04.2026" from a dd.mm.yyyy export got 4 May, and "2.500" became 2.5, in silence
    // (#2255). `parse: false` stores a string as the text it is; numbers, booleans and formulas
    // are unaffected either way.
    let parse = match p.get("parse") {
        None | Some(Json::Null) => true,
        Some(Json::Bool(b)) => *b,
        Some(other) => return Err(bad("range.setValues", format!("parse {other}: true or false"))),
    };
    if rows.len() > MAX_ROWS as usize {
        return Err(bad("range.setValues", "too many rows"));
    }
    let mut count = 0;
    // The strings typed input read as something other than the text given, for the result: a
    // caller can notice a "05.04.2026" that became a date without reading the file back (#2255).
    let mut converted: Vec<(CellRef, String)> = Vec::new();
    let mut converted_count = 0;
    edit(s, |cx| {
        for (ri, row) in rows.iter().enumerate() {
            let cols: Vec<&Json> = match row.as_array() {
                Some(a) => a.iter().collect(),
                None => vec![row],
            };
            for (ci, v) in cols.iter().enumerate() {
                let Some(at) = start.offset(ri as i64, ci as i64) else { continue };
                let input = json_to_input(v);
                let cell = match v {
                    Json::Number(n) => Some(Cell {
                        value: Value::number(n.as_f64().unwrap_or(0.0)),
                        formula: None,
                        style: cx.wb.sheet(sheet).map(|sh| sh.style_id(at)).unwrap_or_default(),
                        py: None,
                    }),
                    Json::Bool(b) => Some(Cell {
                        value: Value::Bool(*b),
                        formula: None,
                        style: cx.wb.sheet(sheet).map(|sh| sh.style_id(at)).unwrap_or_default(),
                        py: None,
                    }),
                    // Text as given: no date or number reading, and a leading `'` is part of the
                    // text rather than the marker that makes it text. A formula is still a
                    // formula, which is what the ticket asks for and what `=` means in a cell.
                    Json::String(text) if !parse && !text.starts_with('=') => {
                        if text.is_empty() {
                            // An empty string clears the cell, as `null` and the default path do:
                            // an empty text cell is not blank, and a script clearing cells with
                            // `""` would leave cells `COUNTA` counts. The cell keeps its formatting.
                            input_to_cell("", sheet, at, &mut cx.wb)?
                        } else {
                            let mut style = cx.wb.sheet(sheet).map(|sh| sh.style_id(at)).unwrap_or_default();
                            // Line breaks still turn on Wrap Text, as typed input does.
                            if text.contains('\n') && !cx.wb.styles.get(style).align.wrap {
                                style = cx.wb.styles.derive(style, |s| s.align.wrap = true);
                            }
                            Some(Cell { value: Value::Text(text.as_str().into()), formula: None, style, py: None })
                        }
                    }
                    _ => input_to_cell(&input, sheet, at, &mut cx.wb)?,
                };
                // A string that became a formula was written as one (`=`), which is deliberate;
                // only a number (dates and times included), boolean or error is a reading of the
                // string and worth reporting.
                if let Some(c) = &cell
                    && matches!(v, Json::String(_))
                    && c.formula.is_none()
                    && matches!(c.value, Value::Number(_) | Value::Bool(_) | Value::Error(_))
                {
                    converted_count += 1;
                    if converted.len() < MAX_REPORTED_CONVERSIONS {
                        converted.push((at, input.clone()));
                    }
                }
                let sh = cx.sheet_mut(sheet)?;
                match cell {
                    Some(c) => sh.set_cell(at, c),
                    None => {
                        sh.remove_cell(at);
                    }
                }
                crate::cmd::data::grow_autofilter(sh, at);
                cx.touch(sheet, at);
                count += 1;
            }
        }
        Ok(())
    })?;
    let d = s.doc()?;
    let reported: Vec<Json> =
        d.wb.sheet(sheet)
            .map(|sh| {
                converted
                .iter()
                .map(|(at, input)| json!({"cell": at.a1(), "input": clipped(input), "stored": clipped(&crate::display::cell_text(&d.wb, sh, *at))}))
                .collect()
            })
            .unwrap_or_default();
    Ok(json!({"cells": count, "convertedCount": converted_count, "converted": reported}))
}

pub(super) fn range_fill(s: &mut Session, p: &Json) -> Result<Json> {
    let sheet = target_sheet(s, p)?;
    let ranges = target_ranges(s, p)?;
    let input = str_param(p, "input").unwrap_or("").to_string();
    let origin = s.doc()?.selection.active;
    edit(s, |cx| {
        // Each cell gets what typing the input there gives: the automatic number format where
        // it's General, its own format otherwise, text where it's Text. That depends only on
        // the cell's style, so the input is parsed once per style.
        let mut by_style: std::collections::HashMap<StyleId, Option<Cell>> = std::collections::HashMap::new();
        for r in &ranges {
            if r.count() > 2_000_000 {
                return Err(bad("range.fill", "range too large"));
            }
            let skip = cx.wb.sheet(sheet).map(|sh| super::filtered_rows(sh, *r)).unwrap_or_default();
            for at in r.iter() {
                if skip.contains(&at.row) {
                    continue;
                }
                let style = cx.wb.sheet(sheet).map(|sh| sh.style_id(at)).unwrap_or_default();
                let base = match by_style.get(&style) {
                    Some(c) => c.clone(),
                    None => {
                        let c = input_to_cell(&input, sheet, at, &mut cx.wb)?;
                        by_style.insert(style, c.clone());
                        c
                    }
                };
                let mut cell = base.unwrap_or_default();
                if let Some(f) = &cell.formula
                    && let Some(e) = f.expr()
                {
                    let shifted = gridcraft_formula::adjust::shift_relative(e, at.row as i64 - origin.row as i64, at.col as i64 - origin.col as i64);
                    cell.formula = Some(std::sync::Arc::new(Formula::from_expr(shifted)));
                }
                let sh = cx.sheet_mut(sheet)?;
                sh.set_cell(at, cell);
                crate::cmd::data::grow_autofilter(sh, at);
                cx.touch(sheet, at);
            }
        }
        Ok(Json::Null)
    })
}
