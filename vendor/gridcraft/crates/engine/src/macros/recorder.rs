//! Turning a recording into a macro.
//!
//! The recording buffer (`crate::record`) holds the top-level commands run while recording, each
//! with the sheet and selection it ran in. When recording stops:
//!
//! - commands a macro may not run (saving, opening files…) and other macros' commands are left
//!   out; a macro run while recording is replaced by its steps when it has no inputs, no output
//!   and only plain commands (otherwise it is left out), and an Automate script run while
//!   recording by the steps it ran, leaving out the steps a macro may not run; both are the steps
//!   as they ran, kept with the recording ([`crate::record::Recorder::ran`]), and a run without
//!   them is left out;
//! - a `sheet.activate` or `selection.set` step is added where the sheet or selection changed
//!   between commands without one, and runs of `selection.set` collapse to the last;
//! - with relative references (the default), the selection when recording started is the example
//!   input: references on its sheet in, next to or near it become tokens (`Input.col(2)`,
//!   `Input.below`, `Input.cell(1,4)`), also inside formulas (`{{Input.col(2)}}`), and the input's
//!   shape, header and column types are inferred from it as it was when recording started;
//! - recorded text that playback would read as a token stays text: a reference parameter that
//!   reads as one (a defined name called `Output`) becomes `{"$text": …}`, and `{{` in a string
//!   becomes `{{{{`.

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{ColType, InputKind, MAX_MACRO_STEPS, Macro, NamedInput, Step, StepKind, Workbook};
use serde_json::{Map, Value as Json, json};
use std::collections::BTreeMap;

use super::policy::{Policy, macro_policy};
use crate::Selection;
use crate::record::RecordedStep;

/// How far outside the example input a reference may be and still be made relative to it.
const NEAR_ROWS: u32 = 50;
const NEAR_COLS: u32 = 26;

/// Where recording started, kept beside the buffer, with the name, shortcut and description the
/// macro is to be saved with (from the Record Macro dialog), applied when recording stops.
#[derive(Clone, Debug)]
pub struct RecordingStart {
    pub doc: u64,
    pub sheet: usize,
    pub sheet_name: String,
    pub selection: Selection,
    pub relative: bool,
    /// With relative references, the example input (the selection) as it was when recording
    /// started: the recorded steps may change its values (text made into numbers), and the
    /// macro is for data like the data it was recorded on.
    pub input: Option<NamedInput>,
    /// The name, shortcut and description given when recording started (Record Macro).
    pub name: Option<String>,
    pub shortcut: Option<String>,
    pub description: Option<String>,
}

impl RecordingStart {
    /// Recording starting now in `d`.
    pub fn new(d: &crate::DocState, relative: bool) -> RecordingStart {
        let sheet = d.wb.active_sheet();
        RecordingStart {
            doc: d.uid,
            sheet,
            sheet_name: d.wb.active().map(|sh| sh.name.clone()).unwrap_or_default(),
            selection: d.selection.clone(),
            relative,
            input: relative.then(|| infer_input(&d.wb, sheet, d.selection.current())),
            name: None,
            shortcut: None,
            description: None,
        }
    }
}

/// The steps a recording keeps for a run of `m`: its enabled command steps as written, when they
/// stand on their own (no inputs, no output and only plain commands; a Repeat, If or Stop means
/// nothing outside its macro). `None` when they don't, and the recording leaves the run out.
pub fn inline_steps(m: &Macro) -> Option<Vec<(String, Json)>> {
    if !m.inputs.is_empty() || m.output.anchor.is_some() || !m.steps.iter().all(|x| matches!(x.kind, StepKind::Command { .. })) {
        return None;
    }
    Some(
        m.steps
            .iter()
            .filter(|x| x.enabled)
            .filter_map(|x| match &x.kind {
                StepKind::Command { id, params } => Some((id.clone(), params.clone())),
                _ => None,
            })
            .collect(),
    )
}

/// Where a gathered command came from, which says how its parameters are written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// Run while recording: plain parameters, which may become relative.
    Recorded,
    /// A step of a recorded script run: plain parameters, kept as they are.
    Script,
    /// A step of a recorded macro run: macro parameters (tokens and all), kept as written.
    Macro,
}

/// A recording turned into a macro, with what was left out.
pub struct Converted {
    pub mac: Macro,
    /// Commands left out (ids), in order.
    pub skipped: Vec<String>,
    /// Steps not kept: past the macro's step limit (a recorded run of a long macro adds all of
    /// its steps) or recorded in another workbook.
    pub cut: usize,
}

/// Builds a macro called `name` from the recorded steps.
/// `ran` is what a macro or script run ran, by step index ([`crate::record::Recorder::ran`]); a
/// run without an entry is left out and listed in `skipped`.
pub fn convert(name: &str, start: &RecordingStart, steps: &[RecordedStep], ran: &BTreeMap<usize, Vec<(String, Json)>>) -> Converted {
    let mut skipped = Vec::new();
    // Commands as they'll be played back: (id, params, sheet before, selection before, where
    // they came from). At most the macro's step limit are gathered: a recording of a thousand
    // runs of a long macro would otherwise expand to millions of steps before the limit cut them
    // back.
    let mut cmds: Vec<(String, Json, usize, String, Selection, Origin)> = Vec::new();
    let mut cut = steps.iter().filter(|st| st.doc != start.doc).count();
    let refused = |id: &str, params: &Json| matches!(macro_policy(id, params, crate::Trust::Local), Policy::Deny(_) | Policy::NeedsPermission(_));
    let mut push = |cmds: &mut Vec<(String, Json, usize, String, Selection, Origin)>, cmd: (String, Json, usize, String, Selection, Origin)| {
        if cmds.len() < MAX_MACRO_STEPS {
            cmds.push(cmd);
        } else {
            cut = cut.saturating_add(1);
        }
    };
    for (index, st) in steps.iter().enumerate().filter(|(_, st)| st.doc == start.doc) {
        let id = st.id.as_str();
        // Another macro's steps (when they stand on their own, [`inline_steps`]) or an Automate
        // script's, as they ran: what the run kept with the recording. Without them the run is
        // left out; the macro or script as it stands now may not be what ran.
        let origin = match id {
            "macro.run" => Some(Origin::Macro),
            "automate.runScript" => Some(Origin::Script),
            _ => None,
        };
        if let Some(origin) = origin {
            match ran.get(&index) {
                Some(steps) => {
                    for (cid, params) in steps {
                        if refused(cid, params) {
                            skipped.push(cid.clone());
                            continue;
                        }
                        push(&mut cmds, (cid.clone(), params.clone(), st.sheet, st.sheet_name.clone(), st.selection.clone(), origin));
                    }
                }
                None => skipped.push(id.to_string()),
            }
            continue;
        }
        if refused(id, &st.params) {
            skipped.push(id.to_string());
            continue;
        }
        push(&mut cmds, (id.to_string(), st.params.clone(), st.sheet, st.sheet_name.clone(), st.selection.clone(), Origin::Recorded));
    }

    // Sheet and selection changes made without a command: where a command ran on another sheet,
    // or with another selection, than the one before it, say so (a command that moved the
    // selection itself does it again when played back, so the extra step is harmless).
    // Each step keeps the name of the sheet it runs on.
    let mut out: Vec<(String, Json, String, Origin)> = Vec::new();
    let (mut sheet, mut sel) = (start.sheet, start.selection.clone());
    for (id, params, at_sheet, sheet_name, at_sel, origin) in cmds {
        if at_sheet != sheet {
            out.push(("sheet.activate".into(), json!({"sheet": sheet_name}), sheet_name.clone(), Origin::Recorded));
        }
        // A recorded selection.set just before already put the selection there.
        let selected = matches!(out.last(), Some((last, ..)) if last == "selection.set");
        if (at_sel != sel || at_sheet != sheet) && !selected {
            out.push(("selection.set".into(), selection_params(&at_sel), sheet_name.clone(), Origin::Recorded));
        }
        (sheet, sel) = (at_sheet, at_sel);
        out.push((id, params, sheet_name, origin));
    }
    // Runs of selection.set (or sheet.activate): the last one wins.
    let mut collapsed: Vec<(String, Json, String, Origin)> = Vec::new();
    for (id, params, on, origin) in out {
        if matches!(id.as_str(), "selection.set" | "sheet.activate")
            && let Some((last, ..)) = collapsed.last()
            && *last == id
        {
            collapsed.pop();
        }
        collapsed.push((id, params, on, origin));
    }

    // Relative references. Only references on the input's sheet become tokens: on another sheet
    // `C4` is that sheet's C4.
    let example = start.selection.current();
    let mut used_input = false;
    let steps: Vec<Step> = collapsed
        .into_iter()
        .map(|(id, params, on, origin)| {
            // A step that names another sheet with its `sheet` parameter runs there, whatever
            // sheet was active: its unqualified references are that sheet's cells.
            let names_other_sheet = params.get("sheet").is_some_and(|v| match v {
                Json::String(n) => !n.eq_ignore_ascii_case(&start.sheet_name),
                Json::Number(i) => i.as_u64() != Some(start.sheet as u64),
                _ => false,
            });
            let here = on.eq_ignore_ascii_case(&start.sheet_name) && !names_other_sheet;
            // Plain text first, so the tokens added next are the only ones.
            let params = if origin == Origin::Macro { params } else { literal(&params, None) };
            let params = if start.relative && origin == Origin::Recorded {
                relative(&params, example, &start.sheet_name, here, &mut used_input, None)
            } else {
                params
            };
            Step::command(id, params)
        })
        .collect();
    let mut mac = Macro { name: name.to_string(), steps, ..Macro::default() };
    if start.relative && used_input {
        mac.inputs = vec![start.input.clone().unwrap_or_default()];
    }
    Converted { mac, skipped, cut }
}

fn selection_params(sel: &Selection) -> Json {
    let ranges: Vec<String> = sel.ranges.iter().map(RangeRef::a1).collect();
    json!({"range": ranges.join(","), "active": sel.active.a1()})
}

/// Plain recorded `params` written so playback reads them as they are: a string in a reference
/// parameter that playback would read as a token, or that holds `{{`, becomes `{"$text": …}`,
/// and `{{` in any other string becomes `{{{{`.
fn literal(v: &Json, key: Option<&str>) -> Json {
    match v {
        Json::Object(o) => Json::Object(o.iter().map(|(k, x)| (k.clone(), literal(x, Some(k)))).collect()),
        Json::Array(a) => Json::Array(a.iter().map(|x| literal(x, key)).collect()),
        Json::String(t) if key.is_some_and(super::resolve::is_ref_key) && (t.contains("{{") || super::resolve::reads_as_token(t, &["Input"])) => {
            json!({"$text": t})
        }
        Json::String(t) if t.contains("{{") => json!(t.replace("{{", "{{{{")),
        other => other.clone(),
    }
}

/// `params` with references near `example` (on `sheet`) made into tokens; `here` says whether the
/// step runs on that sheet, so its unqualified references are on it.
fn relative(v: &Json, example: RangeRef, sheet: &str, here: bool, used: &mut bool, key: Option<&str>) -> Json {
    match v {
        // Text kept as text ([`literal`]).
        Json::Object(o) if o.len() == 1 && o.contains_key("$text") => v.clone(),
        Json::Object(o) => {
            // A selection of several areas stays as it is, its active cell with it: a token for
            // the active cell alone would put it outside the areas on playback.
            let several_areas = o.get("range").and_then(Json::as_str).is_some_and(|r| r.contains(','));
            let mut out = Map::new();
            for (k, x) in o {
                let x = if several_areas && k == "active" { x.clone() } else { relative(x, example, sheet, here, used, Some(k)) };
                out.insert(k.clone(), x);
            }
            Json::Object(out)
        }
        Json::Array(a) => Json::Array(a.iter().map(|x| relative(x, example, sheet, here, used, key)).collect()),
        Json::String(t) if key.is_some_and(super::resolve::is_ref_key) => {
            // One area becomes a token; a list of areas ("A1:B2,D4") stays as it is.
            if t.contains(',') {
                return v.clone();
            }
            match ref_token(t, example, sheet, here) {
                Some(token) => {
                    *used = true;
                    json!(token)
                }
                None => v.clone(),
            }
        }
        Json::String(t) if here && t.starts_with('=') && example.count() > 1 => {
            let (f, changed) = formula_tokens(t, example);
            *used |= changed;
            json!(f)
        }
        other => other.clone(),
    }
}

/// The token for an A1 reference on the example's sheet near the example input. `here` says whether
/// an unqualified reference is on that sheet.
fn ref_token(text: &str, example: RangeRef, sheet: &str, here: bool) -> Option<String> {
    let (on, body) = crate::cmd::split_sheet(text.trim());
    match on {
        Some(on) if !on.eq_ignore_ascii_case(sheet) => return None,
        None if !here => return None,
        _ => {}
    }
    let t = RangeRef::parse(body)?;
    if t.is_full_cols() || t.is_full_rows() {
        return None;
    }
    token(t, example)
}

/// `t` as a token relative to `r`.
fn token(t: RangeRef, r: RangeRef) -> Option<String> {
    if t == r {
        return Some("Input".into());
    }
    let rel_row = |row: u32| i64::from(row) - i64::from(r.start.row) + 1;
    let rel_col = |col: u32| i64::from(col) - i64::from(r.start.col) + 1;
    let cols = |a: u32, b: u32| if a == b { format!("col({})", rel_col(a)) } else { format!("cols({}..{})", rel_col(a), rel_col(b)) };
    if r.contains_range(&t) {
        let full_rows = t.start.row == r.start.row && t.end.row == r.end.row;
        let full_cols = t.start.col == r.start.col && t.end.col == r.end.col;
        return Some(if full_rows {
            format!("Input.{}", cols(t.start.col, t.end.col))
        } else if full_cols && t.start.row == t.end.row {
            format!("Input.row({})", rel_row(t.start.row))
        } else if full_cols && t.end.row == r.end.row {
            format!("Input.rows({}..)", rel_row(t.start.row))
        } else if full_cols {
            format!("Input.rows({}..{})", rel_row(t.start.row), rel_row(t.end.row))
        } else if t.is_single() {
            format!("Input.cell({},{})", rel_row(t.start.row), rel_col(t.start.col))
        } else {
            format!("Input.cell({},{}).resize({},{})", rel_row(t.start.row), rel_col(t.start.col), t.height(), t.width())
        });
    }
    let near = t.start.row.saturating_add(NEAR_ROWS) >= r.start.row
        && t.end.row <= r.end.row.saturating_add(NEAR_ROWS)
        && t.start.col.saturating_add(NEAR_COLS) >= r.start.col
        && t.end.col <= r.end.col.saturating_add(NEAR_COLS);
    if !near {
        return None;
    }
    // Just below the input, in its columns: follows the input's length.
    if t.start.row == r.end.row + 1 && t.start.col >= r.start.col && t.end.col <= r.end.col {
        let base = if t.start.col == r.start.col && t.end.col == r.end.col {
            "Input".to_string()
        } else {
            format!("Input.{}", cols(t.start.col, t.end.col))
        };
        return Some(if t.height() == 1 { format!("{base}.below") } else { format!("{base}.below({})", t.height()) });
    }
    // Just right of the input, in its rows: follows its width.
    if t.start.col == r.end.col + 1 && t.start.row >= r.start.row && t.end.row <= r.end.row {
        let base = if t.start.row == r.start.row && t.end.row == r.end.row {
            "Input".to_string()
        } else if t.start.row == t.end.row {
            format!("Input.row({})", rel_row(t.start.row))
        } else {
            format!("Input.rows({}..{})", rel_row(t.start.row), rel_row(t.end.row))
        };
        return Some(if t.width() == 1 { format!("{base}.right") } else { format!("{base}.right({})", t.width()) });
    }
    let cell = format!("Input.cell({},{})", rel_row(t.start.row), rel_col(t.start.col));
    Some(if t.is_single() { cell } else { format!("{cell}.resize({},{})", t.height(), t.width()) })
}

/// Replaces references in a formula that are exactly the example input, one of its columns or
/// rows, or a column without its first row, with `{{token}}`.
fn formula_tokens(f: &str, r: RangeRef) -> (String, bool) {
    // Each reference in the formula text is read and matched on its own, so the work is the
    // formula's length whatever the example's size (an example of a whole sheet has a million
    // rows, which no list of candidates may grow with).
    let mut out = String::with_capacity(f.len());
    let mut changed = false;
    let (mut i, mut in_text) = (0, false);
    let bytes = f.as_bytes();
    while i < f.len() {
        let Some(rest) = f.get(i..) else { break };
        let ch = rest.chars().next().unwrap_or(' ');
        if ch == '"' {
            in_text = !in_text;
        }
        // A reference right after `{` stays as it is: `{{` before the token would read as the
        // escape of a literal `{{`.
        let prev_ok = i == 0 || !bytes.get(i - 1).is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'$' | b'!' | b'_' | b'.' | b':' | b'{'));
        if !in_text && prev_ok && ch.is_ascii_alphabetic() {
            // The longest run of reference characters here: letters, digits and one `:`.
            let n = rest.bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b':').count();
            let text = rest.get(..n).unwrap_or("");
            let followed = rest.get(n..).and_then(|x| x.chars().next()).is_some_and(|c| matches!(c, '!' | '(' | '_' | '$'));
            if !followed
                && !text.contains('$')
                && let Some(t) = RangeRef::parse(text)
                && let Some(tok) = formula_token(t, r)
            {
                out.push_str("{{");
                out.push_str(&tok);
                out.push_str("}}");
                i += n;
                changed = true;
                continue;
            }
        }
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, changed)
}

/// The token for a reference inside a formula: exactly the example input, one of its columns or
/// rows, or a column without its first row. Other references stay as they are.
fn formula_token(t: RangeRef, r: RangeRef) -> Option<String> {
    if t == r {
        return Some("Input".into());
    }
    if !r.contains_range(&t) {
        return None;
    }
    let col = usize::try_from(t.start.col.checked_sub(r.start.col)?).ok()?.checked_add(1)?;
    let row = usize::try_from(t.start.row.checked_sub(r.start.row)?).ok()?.checked_add(1)?;
    if t.start.col == t.end.col && t.start.row == r.start.row && t.end.row == r.end.row {
        return Some(format!("Input.col({col})"));
    }
    if t.start.col == t.end.col && r.height() > 2 && t.start.row == r.start.row.checked_add(1)? && t.end.row == r.end.row {
        return Some(format!("Input.rows(2..).col({col})"));
    }
    if r.width() > 1 && t.start.row == t.end.row && t.start.col == r.start.col && t.end.col == r.end.col {
        return Some(format!("Input.row({row})"));
    }
    None
}

/// The example input's shape: a header when the first row is all text over other kinds of
/// values, and each column's type when all its values agree.
pub fn infer_input(wb: &Workbook, sheet: usize, r: RangeRef) -> NamedInput {
    let mut input = NamedInput::default();
    if r.is_single() {
        input.kind = InputKind::Cell;
        input.rows = [1, 1];
        input.cols = [1, 1];
        return input;
    }
    input.cols = [r.width(), r.width()];
    let Some(sh) = wb.sheet(sheet) else { return input };
    if r.count() > 100_000 {
        return input;
    }
    let kind = |c: CellRef| -> Option<ColType> {
        let v = sh.value(c);
        Some(match v {
            Value::Empty => return None,
            Value::Text(t) if t.is_empty() => return None,
            Value::Number(_) if crate::display::parsed(wb.styles.get(sh.style_id(c)).num_fmt.as_str()).is_date() => ColType::Date,
            Value::Number(_) => ColType::Number,
            Value::Text(_) => ColType::Text,
            Value::Bool(_) => ColType::Bool,
            _ => ColType::Any,
        })
    };
    let column = |col: u32, from: u32| -> ColType {
        let mut seen: Option<ColType> = None;
        for row in from..=r.end.row {
            match (seen, kind(CellRef::new(row, col))) {
                (_, None) => {}
                (None, k) => seen = k,
                (Some(a), Some(b)) if a != b => return ColType::Any,
                _ => {}
            }
        }
        seen.unwrap_or(ColType::Any)
    };
    let first_text = (r.start.col..=r.end.col).all(|c| kind(CellRef::new(r.start.row, c)) == Some(ColType::Text));
    let below = (r.start.col..=r.end.col).map(|c| column(c, r.start.row + 1)).collect::<Vec<_>>();
    input.header = r.height() > 1 && first_text && below.iter().any(|k| !matches!(k, ColType::Text | ColType::Any));
    input.col_types = if input.header { below } else { (r.start.col..=r.end.col).map(|c| column(c, r.start.row)).collect() };
    if input.col_types.iter().all(|k| *k == ColType::Any) {
        input.col_types.clear();
    }
    input.rows = [1, 0];
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a1: &str) -> RangeRef {
        RangeRef::parse(a1).unwrap()
    }

    #[test]
    fn references_near_the_example_become_tokens() {
        let ex = r("B2:D11");
        for (t, want) in [
            ("B2:D11", Some("Input")),
            ("C2:C11", Some("Input.col(2)")),
            ("C2:D11", Some("Input.cols(2..3)")),
            ("B2:D2", Some("Input.row(1)")),
            ("B3:D11", Some("Input.rows(2..)")),
            ("B3:D5", Some("Input.rows(2..4)")),
            ("C4", Some("Input.cell(3,2)")),
            ("C4:D5", Some("Input.cell(3,2).resize(2,2)")),
            ("B12:D12", Some("Input.below")),
            ("C12", Some("Input.col(2).below")),
            ("C12:C13", Some("Input.col(2).below(2)")),
            ("E2:E11", Some("Input.right")),
            ("E5", Some("Input.row(4).right")),
            ("E3:F4", Some("Input.rows(2..3).right(2)")),
            ("G2", Some("Input.cell(1,6)")),
            ("A1", Some("Input.cell(0,0)")),
            ("Z500", None),
            ("A:A", None),
        ] {
            assert_eq!(ref_token(t, ex, "Sheet1", true).as_deref(), want, "{t}");
        }
        assert_eq!(ref_token("Sheet1!C4", ex, "Sheet1", true).as_deref(), Some("Input.cell(3,2)"));
        assert_eq!(ref_token("Other!C4", ex, "Sheet1", true), None);
        // On another sheet only a reference qualified with the input's sheet is on it.
        assert_eq!(ref_token("C4", ex, "Sheet1", false), None);
        assert_eq!(ref_token("Sheet1!C4", ex, "Sheet1", false).as_deref(), Some("Input.cell(3,2)"));
    }

    #[test]
    fn formulas_get_tokens_for_exact_references() {
        let ex = r("B2:C5");
        for (f, want) in [
            ("=SUM(C3:C5)", "=SUM({{Input.rows(2..).col(2)}})"),
            ("=SUM(C2:C5)+COUNT(B2:C5)", "=SUM({{Input.col(2)}})+COUNT({{Input}})"),
            ("=SUM($C$3:$C$5)", "=SUM($C$3:$C$5)"),
            ("=SUM(Other!C3:C5)", "=SUM(Other!C3:C5)"),
            ("=\"C3:C5\"&C3:C5", "=\"C3:C5\"&{{Input.rows(2..).col(2)}}"),
            ("=SUM(AC3:C5)", "=SUM(AC3:C5)"),
            ("=B2", "=B2"),
        ] {
            assert_eq!(formula_tokens(f, ex).0, want, "{f}");
        }
    }
}
