use super::lp::Outcome;
use super::model::{Constraint, Goal, Method, Model, Rel, Rhs, parse_model, saved_model};
use super::problem::{Evaluator, Problem, as_num, build_problem};
use super::sensitivity::{sensitivity_refusal, sensitivity_report};
use super::{
    Cell, CellRef, Ctx, DocState, EngineError, Json, PRECISION, REPORT_CMD, Result, Session, Sheet, Trial, Value, bad, bool_param, edit, json,
    optimise, str_param,
};

// ---------------------------------------------------------------- reports

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ReportKind {
    Answer,
    Sensitivity,
    Limits,
}

impl ReportKind {
    fn title(self) -> &'static str {
        match self {
            ReportKind::Answer => "Answer",
            ReportKind::Sensitivity => "Sensitivity",
            ReportKind::Limits => "Limits",
        }
    }
}

/// The `reports` list (`None` when absent).
pub(super) fn report_kinds(cmd: &'static str, p: &Json) -> Result<Option<Vec<ReportKind>>> {
    let Some(list) = p.get("reports") else { return Ok(None) };
    let list = list.as_array().ok_or_else(|| bad(cmd, "`reports` must be an array of answer, sensitivity and limits"))?;
    let mut kinds = Vec::new();
    for k in list {
        let kind = match k.as_str().map(|t| t.trim().to_ascii_lowercase()).as_deref() {
            Some("answer") => ReportKind::Answer,
            Some("sensitivity") => ReportKind::Sensitivity,
            Some("limits") => ReportKind::Limits,
            _ => return Err(bad(cmd, format!("unknown report {k} (answer, sensitivity or limits)"))),
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    Ok(Some(kinds))
}

/// A report's cells: (row, column, value, bold).
pub(super) type Lines = Vec<(u32, u32, Value, bool)>;

pub(super) fn abs_ref(c: CellRef) -> String {
    format!("${}${}", gridcraft_core::col_to_letters(c.col), u64::from(c.row) + 1)
}

/// How many cells to the left and above a cell its name is searched for.
const NAME_REACH: u32 = 1_000;

/// A cell's name in the reports: the nearest text to its left and the nearest text above it.
pub(super) fn cell_name(sh: &Sheet, c: CellRef) -> String {
    let text = |c: CellRef| match sh.value(c) {
        Value::Text(t) if !t.trim().is_empty() => Some(t.to_string()),
        _ => None,
    };
    // Labels are next to their cells: looking further than this on a sparse sheet only costs time
    // (a thousand cells near the last row would each probe a million empty coordinates).
    let left = (c.col.saturating_sub(NAME_REACH)..c.col).rev().find_map(|col| text(CellRef::new(c.row, col)));
    let top = (c.row.saturating_sub(NAME_REACH)..c.row).rev().find_map(|row| text(CellRef::new(row, c.col)));
    [left, top].into_iter().flatten().collect::<Vec<_>>().join(" ")
}

fn method_name(m: Method) -> &'static str {
    match m {
        Method::Simplex => "Simplex LP",
        Method::Grg => "GRG Nonlinear",
        Method::Evolutionary => "Evolutionary",
    }
}

/// Header labels in bold from column 1, then the rows below them; returns the next free row.
pub(super) fn table(out: &mut Lines, row: u32, headers: &[&str], rows: Vec<Vec<Value>>) -> u32 {
    for (k, h) in headers.iter().enumerate() {
        out.push((row, 1 + k as u32, Value::text(*h), true));
    }
    let mut r = row;
    for cells in rows {
        r = r.saturating_add(1);
        for (k, v) in cells.into_iter().enumerate() {
            out.push((r, 1 + k as u32, v, false));
        }
    }
    r.saturating_add(2)
}

/// Everything a report describes: the model, its starting and final values, and the result.
pub(super) struct ReportInput<'a> {
    pub(super) model: &'a Model,
    pub(super) prob: &'a Problem,
    pub(super) original: Vec<f64>,
    pub(super) solution: Vec<f64>,
    pub(super) message: String,
    pub(super) budget: u64,
}

pub(super) fn title(out: &mut Lines, kind: ReportKind, sheet: &str) -> u32 {
    out.push((0, 0, Value::text(format!("GridCraft {} Report", kind.title())), true));
    out.push((1, 0, Value::text(format!("Worksheet: {sheet}")), false));
    3
}

fn format_constraint_row(sh: &Sheet, c: &Constraint, cell: CellRef, v: f64, rhs: &[f64], k: usize) -> Vec<Value> {
    let (formula, status, slack) = match (c.rel, &c.rhs) {
        (Rel::Int, _) => (format!("{}=Integer", abs_ref(cell)), "Binding", 0.0),
        (Rel::Bin, _) => (format!("{}=Binary", abs_ref(cell)), "Binding", 0.0),
        (rel, rhs_cells) => {
            let b = rhs.get(k).or_else(|| rhs.first()).copied().unwrap_or(f64::NAN);
            let rhs_text = match rhs_cells {
                Rhs::Cells(cells) => cells.get(k).or_else(|| cells.first()).map(|c| abs_ref(*c)).unwrap_or_default(),
                _ => gridcraft_core::number_to_text(b),
            };
            let slack = (v - b).abs();
            let binding = rel == Rel::Eq || slack <= PRECISION * (1.0 + b.abs());
            let slack = if binding { 0.0 } else { slack };
            (format!("{}{}{rhs_text}", abs_ref(cell), rel.text()), if binding { "Binding" } else { "Not Binding" }, slack)
        }
    };
    let num = |v: f64| if v.is_finite() { Value::number(v) } else { Value::Error(gridcraft_core::CellError::Value) };
    vec![Value::text(abs_ref(cell)), Value::text(cell_name(sh, cell)), num(v), Value::text(formula), Value::text(status), num(slack)]
}

fn answer_report(d: &DocState, sh: &Sheet, inp: &ReportInput) -> Lines {
    let m = inp.model;
    let mut trial = Trial::new(d);
    let read = |trial: &mut Trial, x: &[f64], cells: &[CellRef]| -> Vec<f64> {
        let sets: Vec<(usize, CellRef, Value)> = m.vars.iter().zip(x).map(|(c, v)| (m.sheet, *c, Value::number(*v))).collect();
        let reads: Vec<(usize, CellRef)> = cells.iter().map(|c| (m.sheet, *c)).collect();
        trial.run(&sets, &reads).iter().map(as_num).collect()
    };
    let f0 = read(&mut trial, &inp.original, &[m.objective]).first().copied().unwrap_or(f64::NAN);
    // Every cell the constraints read, at the solution.
    let mut cells = vec![m.objective];
    for c in &m.cons {
        cells.extend(&c.lhs);
        if let Rhs::Cells(r) = &c.rhs {
            cells.extend(r);
        }
    }
    let vals = read(&mut trial, &inp.solution, &cells);
    let mut at = vals.iter().copied();
    let f1 = at.next().unwrap_or(f64::NAN);
    let num = |v: f64| if v.is_finite() { Value::number(v) } else { Value::Error(gridcraft_core::CellError::Value) };
    let mut out = Lines::new();
    let mut r = title(&mut out, ReportKind::Answer, &sh.name);
    out.push((r, 0, Value::text(format!("Result: {}", inp.message)), false));
    out.push((r + 1, 0, Value::text(format!("Engine: {}", method_name(m.method))), false));
    r += 3;
    let goal = match m.goal {
        Goal::Max => "Max".to_string(),
        Goal::Min => "Min".to_string(),
        Goal::Value(v) => format!("Value Of {v}"),
    };
    out.push((r, 0, Value::text(format!("Objective Cell ({goal})")), true));
    let obj = vec![vec![Value::text(abs_ref(m.objective)), Value::text(cell_name(sh, m.objective)), num(f0), num(f1)]];
    r = table(&mut out, r + 1, &["Cell", "Name", "Original Value", "Final Value"], obj);
    out.push((r, 0, Value::text("Variable Cells"), true));
    let kind = |j: usize| {
        let bin = m.cons.iter().any(|c| c.rel == Rel::Bin && m.vars.get(j).is_some_and(|v| c.lhs.contains(v)));
        match (bin, inp.prob.integer.get(j).copied().unwrap_or(false)) {
            (true, _) => "Binary",
            (false, true) => "Integer",
            _ => "Contin",
        }
    };
    let vars = m
        .vars
        .iter()
        .enumerate()
        .map(|(j, c)| {
            let (a, b) = (inp.original.get(j).copied().unwrap_or(f64::NAN), inp.solution.get(j).copied().unwrap_or(f64::NAN));
            vec![Value::text(abs_ref(*c)), Value::text(cell_name(sh, *c)), num(a), num(b), Value::text(kind(j))]
        })
        .collect();
    r = table(&mut out, r + 1, &["Cell", "Name", "Original Value", "Final Value", "Integer"], vars);
    out.push((r, 0, Value::text("Constraints"), true));
    let mut rows = Vec::new();
    for c in &m.cons {
        let lhs: Vec<f64> = c.lhs.iter().map(|_| at.next().unwrap_or(f64::NAN)).collect();
        let rhs: Vec<f64> = match &c.rhs {
            Rhs::Cells(cells) => cells.iter().map(|_| at.next().unwrap_or(f64::NAN)).collect(),
            Rhs::Const(v) => vec![*v],
            Rhs::None => vec![],
        };
        for (k, (cell, v)) in c.lhs.iter().zip(&lhs).enumerate() {
            rows.push(format_constraint_row(sh, c, *cell, *v, &rhs, k));
        }
    }
    table(&mut out, r + 1, &["Cell", "Name", "Cell Value", "Formula", "Status", "Slack"], rows);
    out
}

fn limits_report(d: &DocState, sh: &Sheet, inp: &ReportInput) -> Result<Lines> {
    let m = inp.model;
    if inp.prob.integer.iter().any(|b| *b) {
        return Err(EngineError::Other("The Limits report doesn't apply to models with integer or binary constraints.".into()));
    }
    if m.method == Method::Evolutionary {
        return Err(EngineError::Other("The Limits report is available for Simplex LP and GRG Nonlinear.".into()));
    }
    let n = m.vars.len();
    let x = &inp.solution;
    // Each variable is minimised and maximised with the others held at the solution.
    let per = (inp.budget / (2 * n as u64).max(1)).max(n as u64 + 3);
    let mut ev = Evaluator {
        trial: Trial::new(d),
        sheet: m.sheet,
        vars: m.vars.clone(),
        objective: m.objective,
        rows: inp.prob.rows.clone(),
        evals: 0,
        budget: 0,
        target: None,
    };
    let objective_at = |ev: &mut Evaluator, x: &[f64]| {
        ev.target = None;
        ev.budget = ev.evals.saturating_add(1);
        ev.eval(x).map(|p| p.f).unwrap_or(f64::NAN)
    };
    let num = |v: f64| if v.is_finite() { Value::number(v) } else { Value::Error(gridcraft_core::CellError::Value) };
    let f = objective_at(&mut ev, x);
    let mut rows = Vec::new();
    for (j, c) in m.vars.iter().enumerate() {
        let (mut lo, mut hi) = (x.clone(), x.clone());
        if let (Some(l), Some(h), Some(pl), Some(ph)) = (lo.get_mut(j), hi.get_mut(j), inp.prob.lo.get(j), inp.prob.hi.get(j)) {
            *l = *pl;
            *h = *ph;
        }
        let mut ends = Vec::new();
        for goal in [Goal::Min, Goal::Max] {
            ev.target = Some(j);
            ev.evals = 0;
            ev.budget = per;
            let solve = optimise(&mut ev, m, goal, x, super::Bounds { lo: &lo, hi: &hi, integer: &inp.prob.integer }, 0.0);
            ends.push(match solve.outcome {
                Outcome::Solved(p, _, _) => {
                    let v = p.get(j).copied().unwrap_or(f64::NAN);
                    (num(v), num(objective_at(&mut ev, &p)))
                }
                Outcome::Unbounded => (Value::text("Unbounded"), Value::Empty),
                _ => (Value::Error(gridcraft_core::CellError::NA), Value::Empty),
            });
        }
        let mut row = vec![Value::text(abs_ref(*c)), Value::text(cell_name(sh, *c)), num(x.get(j).copied().unwrap_or(f64::NAN))];
        for (v, o) in ends {
            row.push(v);
            row.push(o);
        }
        rows.push(row);
    }
    let mut out = Lines::new();
    let r = title(&mut out, ReportKind::Limits, &sh.name);
    let obj = vec![vec![Value::text(abs_ref(m.objective)), Value::text(cell_name(sh, m.objective)), num(f)]];
    let r = table(&mut out, r, &["Objective Cell", "Name", "Value"], obj);
    table(&mut out, r, &["Variable Cell", "Name", "Value", "Lower Limit", "Objective Result", "Upper Limit", "Objective Result"], rows);
    Ok(out)
}

/// Builds every report before the solution or saved model can change.
/// What happens to a Sensitivity report the model allows but the solution doesn't: `Refuse`
/// fails the call before anything changes; `Skip` leaves the report out, with its reason, and
/// writes the others. `optimum` is false when the solve stopped before an optimum, so the
/// report isn't attempted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum NotOptimal {
    Refuse,
    Skip { optimum: bool },
}

/// The reports left out under [`NotOptimal::Skip`], as `{report, reason}`.
pub(super) type Skipped = Vec<Json>;

pub(super) fn prepare_reports(
    d: &DocState,
    kinds: &[ReportKind],
    inp: &ReportInput,
    not_optimal: NotOptimal,
) -> Result<(Vec<(String, Lines)>, Skipped)> {
    if kinds.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    // A request that must write a sheet is refused before any report is built (a Limits report
    // runs two optimisations per variable); only a Sensitivity report that may be skipped waits
    // for the check after the loop.
    let must_write = kinds.iter().any(|k| *k != ReportKind::Sensitivity || not_optimal == NotOptimal::Refuse);
    if must_write && d.wb.protected_structure {
        return Err(protected());
    }
    // A model that has no Sensitivity report (integer, Evolutionary, Value Of) is refused in
    // both modes before any report is built: a Limits report listed first would otherwise run
    // its two optimisations per variable for an error. Only the solution's own refusal (not
    // optimal) waits for the loop, where `Skip` may leave the report out.
    if kinds.contains(&ReportKind::Sensitivity)
        && let Some(why) = sensitivity_refusal(inp.model, inp.prob)
    {
        return Err(EngineError::Other(why.into()));
    }
    let sh = d.wb.sheet(inp.model.sheet).ok_or(EngineError::NoDocument)?;
    let mut built = Vec::new();
    let mut skipped = Vec::new();
    for k in kinds {
        let lines = match k {
            ReportKind::Answer => answer_report(d, sh, inp),
            ReportKind::Sensitivity => match not_optimal {
                NotOptimal::Skip { optimum } => {
                    let got = if optimum {
                        sensitivity_report(d, sh, inp).map_err(|e| e.to_string())
                    } else {
                        Err("Solver did not reach an optimal solution, so the Sensitivity report was not written.".to_string())
                    };
                    match got {
                        Ok(lines) => lines,
                        Err(reason) => {
                            skipped.push(json!({"report": "sensitivity", "reason": reason}));
                            continue;
                        }
                    }
                }
                NotOptimal::Refuse => sensitivity_report(d, sh, inp)?,
            },
            ReportKind::Limits => limits_report(d, sh, inp)?,
        };
        let name = (1..=10_000u32)
            .map(|i| format!("{} Report {i}", k.title()))
            .find(|n| d.wb.sheet_index(n).is_none() && !built.iter().any(|(b, _): &(String, Lines)| b == n))
            .ok_or_else(|| EngineError::Other("Too many report sheets.".into()))?;
        built.push((name, lines));
    }
    // Only a report sheet changes the workbook's structure, so a request whose reports were all
    // skipped still succeeds in a protected workbook.
    if !built.is_empty() && d.wb.protected_structure {
        return Err(protected());
    }
    Ok((built, skipped))
}

fn protected() -> EngineError {
    EngineError::Other("The workbook is protected and cannot be changed.".into())
}

/// Inserts prepared reports before the model's sheet, which stays active.
pub(super) fn write_reports(cx: &mut Ctx, at: usize, built: Vec<(String, Lines)>) -> Vec<String> {
    let names: Vec<String> = built.iter().map(|(n, _)| n.clone()).collect();
    let count = built.len();
    if count == 0 {
        return names;
    }
    let bold = cx.wb.styles.derive(gridcraft_model::StyleId::DEFAULT, |st| st.font.bold = true);
    for (i, (name, lines)) in built.into_iter().enumerate() {
        let mut sheet = Sheet::new(name);
        for (r, c, v, b) in lines {
            let style = if b { bold } else { gridcraft_model::StyleId::DEFAULT };
            sheet.set_cell(CellRef::new(r, c), Cell { value: v, formula: None, style, py: None });
        }
        for (c, w) in [(0u32, 14.0f32), (1, 110.0), (2, 140.0), (3, 120.0), (4, 120.0), (5, 155.0), (6, 140.0), (7, 140.0)] {
            sheet.cols.insert(c, gridcraft_model::LineInfo { size: Some(w), ..Default::default() });
        }
        cx.wb.sheets.insert(at + i, std::sync::Arc::new(sheet));
    }
    for nm in cx.wb.names.iter_mut() {
        if let Some(sc) = nm.scope.as_mut()
            && *sc >= at
        {
            *sc += count;
        }
    }
    if cx.wb.active_sheet() >= at {
        cx.wb.set_active_sheet(cx.wb.active_sheet().saturating_add(count));
    }
    cx.structural = true;
    names
}

pub(super) fn insert_report_selections(s: &mut Session, at: usize, count: usize) {
    if let Ok(d) = s.doc_mut() {
        for i in 0..count {
            d.sheet_selections.insert((at + i).min(d.sheet_selections.len()), crate::Selection::default());
        }
    }
}

/// `data.solverReport`: reports for a solution of the sheet's saved model.
pub(super) fn report(s: &mut Session, p: &Json) -> Result<Json> {
    let kinds = report_kinds(REPORT_CMD, p)?.ok_or_else(|| bad(REPORT_CMD, "missing `reports` (answer, sensitivity, limits)"))?;
    if kinds.is_empty() {
        return Err(bad(REPORT_CMD, "`reports` is empty (answer, sensitivity, limits)"));
    }
    let d = s.doc()?;
    // The given model as is, else the saved model with any params given here over it.
    let params = match p.get("model") {
        Some(m @ Json::Object(_)) => m.clone(),
        Some(Json::Null) | None => {
            let mut params = saved_model(&d.wb, d.wb.active_sheet()).unwrap_or_else(|| json!({}));
            if let (Some(base), Some(over)) = (params.as_object_mut(), p.as_object()) {
                for (k, v) in over {
                    base.insert(k.clone(), v.clone());
                }
            }
            params
        }
        Some(_) => return Err(bad(REPORT_CMD, "`model` must be data.solver params")),
    };
    let model = parse_model(s, &params)?;
    let prob = build_problem(&model);
    let d = s.doc()?;
    let sh = d.wb.sheet(model.sheet).ok_or(EngineError::NoDocument)?;
    let current: Vec<f64> = model.vars.iter().map(|c| as_num(&sh.value(*c))).collect();
    // `{cell: number}` maps; cells they leave out keep their current values.
    let point = |key: &str| -> Result<Vec<f64>> {
        let mut x = current.clone();
        let Some(map) = p.get(key) else { return Ok(x) };
        let map = map.as_object().ok_or_else(|| bad(REPORT_CMD, format!("`{key}` must map cells to numbers")))?;
        for (cell, v) in map {
            let c = CellRef::parse(&cell.replace('$', "")).ok_or_else(|| bad(REPORT_CMD, format!("not a cell: `{cell}`")))?;
            let j = model.vars.iter().position(|v| *v == c).ok_or_else(|| bad(REPORT_CMD, format!("`{cell}` is not a variable cell")))?;
            let v = v.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad(REPORT_CMD, format!("`{key}.{cell}` must be a number")))?;
            if let Some(e) = x.get_mut(j) {
                *e = v;
            }
        }
        Ok(x)
    };
    let solution = point("values")?;
    let original = if p.get("original").is_some() { point("original")? } else { solution.clone() };
    let budget = model.max_evals;
    let message = str_param(p, "message").unwrap_or("").chars().take(500).collect();
    let inp = ReportInput { model: &model, prob: &prob, original, solution, message, budget };
    let not_optimal = if bool_param(p, "skipNotOptimal").unwrap_or(false) { NotOptimal::Skip { optimum: true } } else { NotOptimal::Refuse };
    let (built, skipped) = prepare_reports(s.doc()?, &kinds, &inp, not_optimal)?;
    // Nothing left to write (every report skipped): no edit, so no Undo step.
    let sheets = if built.is_empty() { Vec::new() } else { edit(s, |cx| Ok(write_reports(cx, model.sheet, built)))? };
    insert_report_selections(s, model.sheet, sheets.len());
    let mut out = json!({"sheets": sheets});
    if !skipped.is_empty() {
        out["skippedReports"] = json!(skipped);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_searched_only_a_bounded_distance() {
        let mut sh = Sheet::new("S".to_string());
        sh.set_cell(CellRef::new(0, 0), gridcraft_model::Cell::value(Value::text("Far")));
        sh.set_cell(CellRef::new(5, 4), gridcraft_model::Cell::value(Value::text("Near")));
        assert_eq!(cell_name(&sh, CellRef::new(5, 5)), "Near");
        assert_eq!(cell_name(&sh, CellRef::new(0, NAME_REACH + 1)), "");
    }
}
