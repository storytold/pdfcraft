use gridcraft_core::split_areas;

use super::{
    CMD, CellRef, DEFAULT_EVALS, DefinedName, EngineError, Json, MAX_CONSTRAINTS, MAX_EVALS, MAX_POPULATION, MAX_VARS, MIN_POPULATION, RangeRef,
    Result, Session, Workbook, bad, bool_param, f64_param, json, split_sheet, str_param,
};

// ---------------------------------------------------------------- model

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Rel {
    Le,
    Eq,
    Ge,
    Int,
    Bin,
}

impl Rel {
    fn parse(s: &str) -> Option<Rel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "<=" | "≤" | "=<" | "le" => Some(Rel::Le),
            ">=" | "≥" | "=>" | "ge" => Some(Rel::Ge),
            "=" | "==" | "eq" => Some(Rel::Eq),
            "int" | "integer" => Some(Rel::Int),
            "bin" | "binary" => Some(Rel::Bin),
            _ => None,
        }
    }
    pub(super) fn text(self) -> &'static str {
        match self {
            Rel::Le => "<=",
            Rel::Eq => "=",
            Rel::Ge => ">=",
            Rel::Int => "int",
            Rel::Bin => "bin",
        }
    }
    /// The relation code of the saved model (`solver_relN`), as in Solver's VBA `SolverAdd`.
    fn code(self) -> u8 {
        match self {
            Rel::Le => 1,
            Rel::Eq => 2,
            Rel::Ge => 3,
            Rel::Int => 4,
            Rel::Bin => 5,
        }
    }
    fn from_code(c: &str) -> Option<Rel> {
        match c.trim() {
            "1" => Some(Rel::Le),
            "2" => Some(Rel::Eq),
            "3" => Some(Rel::Ge),
            "4" => Some(Rel::Int),
            "5" => Some(Rel::Bin),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum Rhs {
    None,
    Const(f64),
    Cells(Vec<CellRef>),
}

#[derive(Clone, Debug)]
pub(super) struct Constraint {
    pub(super) lhs: Vec<CellRef>,
    lhs_text: String,
    pub(super) rel: Rel,
    pub(super) rhs: Rhs,
    rhs_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Goal {
    Max,
    Min,
    Value(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Method {
    Simplex,
    Grg,
    Evolutionary,
}

/// The Evolutionary method's options (Solver Options › Evolutionary). The search keeps no
/// clock, so the time without improvement is counted in generations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EvoOptions {
    pub(super) population: usize,
    pub(super) mutation: f64,
    pub(super) seed: u64,
    pub(super) convergence: f64,
    pub(super) stall: u64,
    pub(super) require_bounds: bool,
}

impl Default for EvoOptions {
    fn default() -> EvoOptions {
        EvoOptions { population: 100, mutation: 0.075, seed: 0, convergence: 1e-4, stall: 50, require_bounds: true }
    }
}

fn evo_options(p: &Json) -> EvoOptions {
    let d = EvoOptions::default();
    let num = |k: &str| p.get(k).and_then(Json::as_f64).or_else(|| str_param(p, k).and_then(num_text)).filter(|v| v.is_finite());
    EvoOptions {
        population: num("populationSize").map_or(d.population, |v| v.clamp(MIN_POPULATION as f64, MAX_POPULATION as f64) as usize),
        mutation: num("mutationRate").map_or(d.mutation, |v| v.clamp(0.0, 1.0)),
        seed: num("randomSeed").map_or(d.seed, |v| v.clamp(0.0, f64::from(u32::MAX)) as u64),
        convergence: num("convergence").map_or(d.convergence, |v| v.clamp(0.0, 1.0)),
        stall: num("generationsWithoutImprovement").map_or(d.stall, |v| v.clamp(1.0, 100_000.0) as u64),
        require_bounds: bool_param(p, "requireBounds").unwrap_or(d.require_bounds),
    }
}

#[derive(Clone, Debug)]
pub(super) struct Model {
    pub(super) sheet: usize,
    pub(super) objective: CellRef,
    pub(super) goal: Goal,
    pub(super) vars: Vec<CellRef>,
    vars_text: String,
    pub(super) cons: Vec<Constraint>,
    pub(super) non_negative: bool,
    pub(super) method: Method,
    pub(super) evo: EvoOptions,
    /// The evaluation limit (in place of Excel's iterations and time limits).
    pub(super) max_evals: u64,
    /// Integer Optimality, as a fraction.
    pub(super) gap: f64,
}

/// Cells of `"B2:B4,D1"` on one sheet (row-major per area, in order), at most `cap`.
fn cells_of(wb: &Workbook, t: &str, default_sheet: usize, cap: usize) -> Result<(usize, Vec<CellRef>)> {
    let mut sheet = None;
    let mut out = Vec::new();
    // A comma inside a quoted sheet name (`'Sales, West'!$A$1`) does not end the area.
    for part in split_areas(t).into_iter().map(str::trim).filter(|x| !x.is_empty()) {
        let (sh, body) = split_sheet(part);
        let si = match sh {
            Some(n) => wb.sheet_index(&n).ok_or_else(|| bad(CMD, format!("no sheet named `{n}`")))?,
            None => default_sheet,
        };
        if *sheet.get_or_insert(si) != si {
            return Err(bad(CMD, "all cells of a reference must be on one sheet"));
        }
        let r = RangeRef::parse(body.trim()).ok_or_else(|| bad(CMD, format!("not a reference: `{part}`")))?;
        if r.count() > cap as u64 || out.len() + r.count() as usize > cap {
            return Err(bad(CMD, format!("too many cells (at most {cap})")));
        }
        out.extend(r.iter());
    }
    if out.is_empty() {
        return Err(bad(CMD, format!("not a reference: `{t}`")));
    }
    Ok((sheet.unwrap_or(default_sheet), out))
}

fn num_text(t: &str) -> Option<f64> {
    gridcraft_core::parse::parse_number_text(t.trim()).filter(|v| v.is_finite())
}

/// `t` split at the first `pat` outside quoted sheet names: `'A<=B'!$C$1 <= 5` splits at the
/// second `<=`. A doubled quote inside a name (`'O''Brien<=1'!A1`) toggles the quoted state
/// twice, as in [`split_areas`], so it doesn't end the name.
fn split_unquoted<'a>(t: &'a str, pat: &str) -> Option<(&'a str, &'a str)> {
    let mut quoted = false;
    for (i, c) in t.char_indices() {
        if c == '\'' {
            quoted = !quoted;
        } else if !quoted && t.get(i..).is_some_and(|rest| rest.starts_with(pat)) {
            return Some((t.get(..i)?, t.get(i.saturating_add(pat.len())..)?));
        }
    }
    None
}

/// Splits `"B5 <= 10"` / `"B2 int"` into (lhs, relation, rhs). A relation's characters inside
/// a quoted sheet name (`'A<=B'!$C$1 <= 5`) are part of the name.
fn split_constraint(t: &str) -> Option<(String, Rel, String)> {
    for op in ["<=", ">=", "=<", "=>", "≤", "≥", "=="] {
        if let Some((a, b)) = split_unquoted(t, op) {
            return Some((a.trim().into(), Rel::parse(op)?, b.trim().into()));
        }
    }
    if let Some((a, b)) = split_unquoted(t, "=") {
        return Some((a.trim().into(), Rel::Eq, b.trim().into()));
    }
    let lower = t.trim().to_ascii_lowercase();
    for (suffix, rel) in [("integer", Rel::Int), ("binary", Rel::Bin), ("int", Rel::Int), ("bin", Rel::Bin)] {
        if let Some(a) = lower.strip_suffix(suffix) {
            let a = a.trim();
            return Some((t.trim().get(..a.len())?.trim().to_string(), rel, String::new()));
        }
    }
    None
}

fn parse_constraint(wb: &Workbook, sheet: usize, c: &Json) -> Result<Constraint> {
    let (lhs_text, rel, rhs_json) = match c {
        Json::String(t) => {
            let (l, r, rhs) = split_constraint(t).ok_or_else(|| bad(CMD, format!("not a constraint: `{t}`")))?;
            (l, r, Json::String(rhs))
        }
        Json::Object(_) => {
            let lhs = str_param(c, "lhs").or_else(|| str_param(c, "cell")).or_else(|| str_param(c, "range")).unwrap_or("").to_string();
            let op = str_param(c, "op").or_else(|| str_param(c, "relation")).unwrap_or("");
            let rel = Rel::parse(op).ok_or_else(|| bad(CMD, format!("unknown constraint relation `{op}`")))?;
            (lhs, rel, c.get("rhs").or_else(|| c.get("value")).cloned().unwrap_or(Json::Null))
        }
        _ => return Err(bad(CMD, "a constraint is a string like \"B5 <= 10\" or {lhs, op, rhs}")),
    };
    let (lsi, lhs) = cells_of(wb, &lhs_text, sheet, MAX_CONSTRAINTS)?;
    if lsi != sheet {
        return Err(EngineError::Other("Constraint cells must be on the objective cell's sheet.".into()));
    }
    let (rhs, rhs_text) = match rel {
        Rel::Int | Rel::Bin => (Rhs::None, String::new()),
        _ => match &rhs_json {
            Json::Number(n) => {
                let v = n.as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad(CMD, "the constraint's right side must be a number"))?;
                (Rhs::Const(v), v.to_string())
            }
            Json::String(t) => {
                let t = t.trim().trim_start_matches('=').trim();
                if let Some(v) = num_text(t) {
                    (Rhs::Const(v), t.to_string())
                } else {
                    let (rsi, cells) = cells_of(wb, t, sheet, MAX_CONSTRAINTS)?;
                    if rsi != sheet {
                        return Err(EngineError::Other("Constraint cells must be on the objective cell's sheet.".into()));
                    }
                    if cells.len() != 1 && cells.len() != lhs.len() {
                        return Err(EngineError::Other("The constraint's two sides must be the same size (or the right side one cell).".into()));
                    }
                    (Rhs::Cells(cells), t.to_string())
                }
            }
            _ => return Err(bad(CMD, "missing the constraint's right side")),
        },
    };
    Ok(Constraint { lhs, lhs_text, rel, rhs, rhs_text })
}

pub(super) fn parse_model(s: &Session, p: &Json) -> Result<Model> {
    let d = s.doc()?;
    let wb = &d.wb;
    let obj_text = str_param(p, "objective").or_else(|| str_param(p, "setObjective")).or_else(|| str_param(p, "set")).unwrap_or("");
    let (sheet, obj) = cells_of(wb, obj_text, wb.active_sheet(), 1).map_err(|_| EngineError::Other("The objective must be a single cell.".into()))?;
    let objective = *obj.first().ok_or_else(|| EngineError::Other("The objective must be a single cell.".into()))?;
    let sh = wb.sheet(sheet).ok_or(EngineError::NoDocument)?;
    if sh.cell(objective).is_none_or(|c| c.formula.is_none()) {
        return Err(EngineError::Other("The objective cell must contain a formula.".into()));
    }
    let goal = match str_param(p, "goal").or_else(|| str_param(p, "to")).unwrap_or("max").to_ascii_lowercase().as_str() {
        "max" | "maximize" | "maximum" => Goal::Max,
        "min" | "minimize" | "minimum" => Goal::Min,
        "value" | "valueof" | "value of" => {
            let v = f64_param(p, "value")
                .or_else(|| str_param(p, "value").and_then(num_text))
                .filter(|v| v.is_finite())
                .ok_or_else(|| bad(CMD, "goal `value` needs a number in `value`"))?;
            Goal::Value(v)
        }
        g => return Err(bad(CMD, format!("unknown goal `{g}` (max, min or value)"))),
    };
    let vars_text = str_param(p, "variables").or_else(|| str_param(p, "changing")).or_else(|| str_param(p, "byChanging")).unwrap_or("").to_string();
    let (vsi, mut vars) = cells_of(wb, &vars_text, sheet, MAX_VARS).map_err(|e| match e {
        EngineError::BadParams { msg, .. } if msg.starts_with("too many") => {
            EngineError::Other(format!("Too many variable cells (at most {MAX_VARS})."))
        }
        _ => EngineError::Other("The variable cells must be a reference like B2:B4.".into()),
    })?;
    if vsi != sheet {
        return Err(EngineError::Other("The variable cells must be on the objective cell's sheet.".into()));
    }
    let mut seen = std::collections::HashSet::new();
    vars.retain(|c| seen.insert(*c));
    // Python cells count as formulas: a solution written over one would lose its code.
    if vars.iter().any(|c| sh.cell(*c).is_some_and(|c| c.formula.is_some() || c.py.is_some())) {
        return Err(EngineError::Other("Variable cells must hold constants, not formulas.".into()));
    }
    let mut cons = Vec::new();
    let mut scalar = 0usize;
    if let Some(list) = p.get("constraints") {
        let list = list.as_array().ok_or_else(|| bad(CMD, "`constraints` must be an array"))?;
        if list.len() > MAX_CONSTRAINTS {
            return Err(EngineError::Other(format!("Too many constraints (at most {MAX_CONSTRAINTS}).")));
        }
        for c in list {
            let c = parse_constraint(wb, sheet, c)?;
            scalar = scalar.saturating_add(c.lhs.len());
            if scalar > MAX_CONSTRAINTS {
                return Err(EngineError::Other(format!("Too many constraints (at most {MAX_CONSTRAINTS} cells).")));
            }
            if matches!(c.rel, Rel::Int | Rel::Bin) && c.lhs.iter().any(|x| !vars.contains(x)) {
                return Err(EngineError::Other("Integer and binary constraints must be on variable cells.".into()));
            }
            cons.push(c);
        }
    }
    let non_negative = bool_param(p, "nonNegative").unwrap_or(true);
    let method = match str_param(p, "method").unwrap_or("grg").to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
        "grg" | "grgnonlinear" | "nonlinear" => Method::Grg,
        "simplex" | "simplexlp" | "lp" | "linear" => Method::Simplex,
        "evolutionary" | "evo" | "genetic" => Method::Evolutionary,
        m => return Err(bad(CMD, format!("unknown method `{m}` (grg, simplex or evolutionary)"))),
    };
    let num = |k: &str| p.get(k).and_then(Json::as_f64).or_else(|| str_param(p, k).and_then(num_text)).filter(|v| v.is_finite());
    let max_evals = num("maxEvaluations").filter(|v| *v >= 1.0).map_or(DEFAULT_EVALS, |v| (v as u64).min(MAX_EVALS));
    // Integer Optimality is in percent: "5" and "5%" both mean 5 % (number parsing would read
    // "5%" as 0.05).
    let percent = p
        .get("integerOptimality")
        .and_then(Json::as_f64)
        .or_else(|| str_param(p, "integerOptimality").and_then(|t| num_text(t.trim().trim_end_matches('%'))))
        .filter(|v| v.is_finite());
    let gap = percent.map_or(0.0, |v| v.clamp(0.0, 100.0) / 100.0);
    Ok(Model { sheet, objective, goal, vars, vars_text, cons, non_negative, method, evo: evo_options(p), max_evals, gap })
}

/// Whether a `data.solver` result status is a solution the Sensitivity report can describe: an
/// optimum (Simplex LP) or a converged local optimum (GRG Nonlinear). A run that stopped at its
/// evaluation limit, or an Evolutionary run, has none.
pub fn sensitivity_status(status: &str) -> bool {
    matches!(status, "optimal" | "converged")
}

/// Whether `data.solver` params (such as the `model` of its result) describe a model that can
/// have a Sensitivity report: Simplex LP or GRG Nonlinear, a Max or Min objective and no
/// integer or binary constraints.
pub fn sensitivity_applies(p: &Json) -> bool {
    let method = str_param(p, "method").unwrap_or("grg").to_ascii_lowercase().replace([' ', '_', '-'], "");
    let goal = str_param(p, "goal").or_else(|| str_param(p, "to")).unwrap_or("max").to_ascii_lowercase();
    let integer = p.get("constraints").and_then(Json::as_array).is_some_and(|list| {
        list.iter().any(|c| {
            let rel = match c {
                Json::String(t) => split_constraint(t).map(|(_, rel, _)| rel),
                Json::Object(_) => str_param(c, "op").or_else(|| str_param(c, "relation")).and_then(Rel::parse),
                _ => None,
            };
            matches!(rel, Some(Rel::Int | Rel::Bin))
        })
    });
    !integer && !matches!(method.as_str(), "evolutionary" | "evo" | "genetic") && !matches!(goal.as_str(), "value" | "valueof" | "value of")
}

// ---------------------------------------------------------------- saved model

/// `"B2:B4,D1"` as absolute references, each qualified by `sheet_name` when given.
fn abs_text(sheet_name: Option<&str>, cells: &str) -> String {
    let q = sheet_name.map(|n| format!("{}!", crate::pivot::quote_sheet(n))).unwrap_or_default();
    split_areas(cells)
        .into_iter()
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .filter_map(|part| {
            let r = RangeRef::parse(split_sheet(part).1.trim())?;
            let abs = |c: CellRef| format!("${}${}", gridcraft_core::col_to_letters(c.col), c.row as u64 + 1);
            Some(if r.is_single() { format!("{q}{}", abs(r.start)) } else { format!("{q}{}:{}", abs(r.start), abs(r.end)) })
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn ref_text(sheet_name: &str, cells: &str) -> String {
    abs_text(Some(sheet_name), cells)
}

/// The evaluation limit and the generations without improvement have no Excel equivalent:
/// Excel's `solver_itr` counts iterations (100 by default) and `solver_mni` seconds, so a
/// workbook from Excel would cut GridCraft's search short. They are saved under GridCraft's own
/// names, which Excel ignores.
const EVALS_NAME: &str = "gridcraft_solver_evals";
const STALL_NAME: &str = "gridcraft_solver_stall";

/// The names of a saved model after `solver_`, as Excel's Solver writes them (GridCraft writes
/// some of them). Saving a model and Reset All remove these and no other `solver_` names.
const SOLVER_KEYS: [&str; 28] = [
    "adj", "cvg", "drv", "eng", "est", "itr", "lin", "mip", "mni", "mrt", "msl", "neg", "nod", "num", "nwt", "opt", "pre", "rbv", "rlx", "rsd",
    "scl", "sho", "ssz", "tim", "tol", "typ", "val", "ver",
];

/// Whether `name` is one of the names of a saved model: Excel's Solver names (`solver_opt`,
/// `solver_lhs1` and so on) and GridCraft's own. A user's name such as `solver_budget` is not.
pub(super) fn is_model_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name == EVALS_NAME || name == STALL_NAME {
        return true;
    }
    let Some(key) = name.strip_prefix("solver_") else { return false };
    if SOLVER_KEYS.contains(&key) {
        return true;
    }
    // One name per constraint: solver_lhs1, solver_rel1, solver_rhs1, …
    ["lhs", "rel", "rhs"].iter().filter_map(|p| key.strip_prefix(p)).any(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The model as hidden names scoped to its sheet (replacing any saved before).
pub(super) fn save_model(wb: &mut Workbook, m: &Model) {
    let Some(sheet_name) = wb.sheet(m.sheet).map(|s| s.name.clone()) else { return };
    wb.names.retain(|n| !(n.scope == Some(m.sheet) && is_model_name(&n.name)));
    let mut put = |name: String, formula: String| {
        wb.names.push(DefinedName { name, scope: Some(m.sheet), formula, comment: String::new(), hidden: true });
    };
    put("solver_opt".into(), ref_text(&sheet_name, &m.objective.a1()));
    let (typ, val) = match m.goal {
        Goal::Max => (1, 0.0),
        Goal::Min => (2, 0.0),
        Goal::Value(v) => (3, v),
    };
    put("solver_typ".into(), typ.to_string());
    put("solver_val".into(), val.to_string());
    put("solver_adj".into(), ref_text(&sheet_name, &m.vars_text));
    for (i, c) in m.cons.iter().enumerate() {
        let k = i + 1;
        put(format!("solver_lhs{k}"), ref_text(&sheet_name, &c.lhs_text));
        put(format!("solver_rel{k}"), c.rel.code().to_string());
        let rhs = match &c.rhs {
            Rhs::None => {
                if c.rel == Rel::Bin {
                    "\"binary\"".to_string()
                } else {
                    "\"integer\"".to_string()
                }
            }
            Rhs::Const(v) => v.to_string(),
            Rhs::Cells(_) => ref_text(&sheet_name, &c.rhs_text),
        };
        put(format!("solver_rhs{k}"), rhs);
    }
    put("solver_num".into(), m.cons.len().to_string());
    put("solver_neg".into(), if m.non_negative { "1" } else { "2" }.into());
    let eng = match m.method {
        Method::Grg => "1",
        Method::Simplex => "2",
        Method::Evolutionary => "3",
    };
    put("solver_eng".into(), eng.into());
    put(EVALS_NAME.into(), m.max_evals.to_string());
    put("solver_tol".into(), m.gap.to_string());
    // The Evolutionary options are kept whatever the method, as Solver Options sets them, so
    // switching methods doesn't lose them.
    put("solver_ssz".into(), m.evo.population.to_string());
    put("solver_mrt".into(), m.evo.mutation.to_string());
    put("solver_rsd".into(), m.evo.seed.to_string());
    put("solver_rbv".into(), if m.evo.require_bounds { "1" } else { "2" }.into());
    put("solver_cvg".into(), m.evo.convergence.to_string());
    put(STALL_NAME.into(), m.evo.stall.to_string());
    put("solver_ver".into(), "3".into());
}

/// The saved model of a sheet as `data.solver` params, if any.
///
/// A sheet's own (sheet-level) model wins over one saved as workbook-level names, and its names
/// are read alone, so the two never mix. A sheet-level `solver_opt` of `""` (written by Reset
/// All over a workbook-level model) means the sheet has no model.
pub fn saved_model(wb: &Workbook, sheet: usize) -> Option<Json> {
    let named = |n: &str, scope: Option<usize>| wb.names.iter().find(|d| d.name.eq_ignore_ascii_case(n) && d.scope == scope);
    let scope = if named("solver_opt", Some(sheet)).is_some() { Some(sheet) } else { None };
    let get = |n: &str| named(n, scope).map(|d| d.formula.trim().trim_start_matches('=').to_string());
    let local = |t: String| local_refs(&t);
    let objective = local(get("solver_opt")?);
    if objective.trim_matches('"').trim().is_empty() {
        return None;
    }
    let goal = match get("solver_typ").as_deref().map(str::trim) {
        Some("2") => "min",
        Some("3") => "value",
        _ => "max",
    };
    let value = get("solver_val").and_then(|v| num_text(&v)).unwrap_or(0.0);
    let count = get("solver_num").and_then(|v| num_text(&v)).unwrap_or(0.0).clamp(0.0, MAX_CONSTRAINTS as f64) as usize;
    let mut constraints = Vec::new();
    for k in 1..=count {
        let (Some(lhs), Some(rel)) = (get(&format!("solver_lhs{k}")), get(&format!("solver_rel{k}")).and_then(|r| Rel::from_code(&r))) else {
            continue;
        };
        let lhs = local(lhs);
        constraints.push(match rel {
            Rel::Int | Rel::Bin => json!(format!("{lhs} {}", rel.text())),
            _ => {
                let rhs = get(&format!("solver_rhs{k}")).unwrap_or_default();
                let rhs = if num_text(&rhs).is_some() { rhs } else { local(rhs) };
                json!(format!("{lhs} {} {rhs}", rel.text()))
            }
        });
    }
    let method = match get("solver_eng").as_deref().map(str::trim) {
        Some("2") => "simplex",
        Some("3") => "evolutionary",
        _ => "grg",
    };
    let mut model = json!({
        "objective": objective,
        "goal": goal,
        "value": value,
        "variables": get("solver_adj").map(local).unwrap_or_default(),
        "constraints": constraints,
        "nonNegative": get("solver_neg").as_deref().map(str::trim) != Some("2"),
        "method": method,
    });
    for (name, key) in [
        ("solver_ssz", "populationSize"),
        ("solver_mrt", "mutationRate"),
        ("solver_rsd", "randomSeed"),
        ("solver_cvg", "convergence"),
        (STALL_NAME, "generationsWithoutImprovement"),
        (EVALS_NAME, "maxEvaluations"),
    ] {
        if let Some(v) = get(name).and_then(|v| num_text(&v)) {
            model[key] = number(v);
        }
    }
    if let Some(v) = get("solver_tol").and_then(|v| num_text(&v)) {
        model["integerOptimality"] = number(percent(v));
    }
    if let Some(v) = get("solver_rbv") {
        model["requireBounds"] = json!(v.trim() != "2");
    }
    Some(model)
}

/// The model as `data.solver` params, in the form [`saved_model`] returns (the objective
/// qualified by its sheet).
pub(super) fn model_params(wb: &Workbook, m: &Model) -> Json {
    let sheet = wb.sheet(m.sheet).map(|s| crate::pivot::quote_sheet(&s.name)).unwrap_or_default();
    let (goal, value) = match m.goal {
        Goal::Max => ("max", 0.0),
        Goal::Min => ("min", 0.0),
        Goal::Value(v) => ("value", v),
    };
    let constraints: Vec<Json> = m
        .cons
        .iter()
        .map(|c| match c.rel {
            Rel::Int | Rel::Bin => json!(format!("{} {}", c.lhs_text, c.rel.text())),
            _ => json!(format!("{} {} {}", c.lhs_text, c.rel.text(), c.rhs_text)),
        })
        .collect();
    let method = match m.method {
        Method::Simplex => "simplex",
        Method::Grg => "grg",
        Method::Evolutionary => "evolutionary",
    };
    let mut out = json!({
        "objective": format!("{sheet}!{}", m.objective.a1()),
        "goal": goal,
        "value": value,
        "variables": m.vars_text,
        "constraints": constraints,
        "nonNegative": m.non_negative,
        "method": method,
        "maxEvaluations": m.max_evals,
        "integerOptimality": number(percent(m.gap)),
    });
    let e = &m.evo;
    out["populationSize"] = json!(e.population);
    out["mutationRate"] = json!(e.mutation);
    out["randomSeed"] = json!(e.seed);
    out["convergence"] = json!(e.convergence);
    out["generationsWithoutImprovement"] = json!(e.stall);
    out["requireBounds"] = json!(e.require_bounds);
    out
}

// ---------------------------------------------------------------- model on cells

/// The cells Save Model writes, top to bottom, as formula text.
pub(super) fn model_formulas(m: &Model) -> Result<Vec<String>> {
    // A comma inside a quoted sheet name (`'Sales, West'!$A$1`) doesn't start another area.
    let one_area = |t: &str| split_areas(t).into_iter().filter(|a| !a.trim().is_empty()).count() <= 1;
    let mut rows = Vec::with_capacity(m.cons.len() + 3);
    let obj = abs_text(None, &m.objective.a1());
    rows.push(match m.goal {
        Goal::Max => format!("=MAX({obj})"),
        Goal::Min => format!("=MIN({obj})"),
        Goal::Value(v) => format!("={obj}={}", gridcraft_core::number_to_text(v)),
    });
    rows.push(format!("=COUNT({})", abs_text(None, &m.vars_text)));
    for c in &m.cons {
        // A number on the right side (`1,000`) is no range, whatever its separators.
        if !one_area(&c.lhs_text) || (matches!(c.rhs, Rhs::Cells(_)) && !one_area(&c.rhs_text)) {
            return Err(EngineError::Other("Save Model needs each side of a constraint to be one range.".into()));
        }
        let lhs = abs_text(None, &c.lhs_text);
        rows.push(match (c.rel, &c.rhs) {
            (Rel::Int, _) => format!("=AND({lhs}=INT({lhs}))"),
            (Rel::Bin, _) => format!("=AND(({lhs}=0)+({lhs}=1))"),
            (rel, Rhs::Cells(_)) => format!("=AND({lhs}{}{})", rel.text(), abs_text(None, &c.rhs_text)),
            (rel, Rhs::Const(v)) => format!("=AND({lhs}{}{})", rel.text(), gridcraft_core::number_to_text(*v)),
            (rel, Rhs::None) => format!("=AND({lhs}{}0)", rel.text()),
        });
    }
    let e = &m.evo;
    let n = gridcraft_core::number_to_text;
    let eng = match m.method {
        Method::Grg => 1,
        Method::Simplex => 2,
        Method::Evolutionary => 3,
    };
    let yes = |b: bool| if b { 1 } else { 2 };
    rows.push(format!(
        "={{{eng},{},{},{},{},{},{},{},{},{}}}",
        yes(m.non_negative),
        e.population,
        n(e.mutation),
        e.seed,
        yes(e.require_bounds),
        n(e.convergence),
        e.stall,
        m.max_evals,
        n(percent(m.gap))
    ));
    Ok(rows)
}

/// `NAME(inner)` → inner, ignoring case.
fn call_args(t: &str, name: &str) -> Option<String> {
    let head = t.get(..name.len() + 1)?;
    (head.eq_ignore_ascii_case(&format!("{name}(")) && t.ends_with(')')).then(|| t.get(name.len() + 1..t.len() - 1).map(str::to_string))?
}

/// References without sheet names or `$`.
fn local_refs(t: &str) -> String {
    split_areas(t).into_iter().map(|p| split_sheet(p.trim()).1.replace('$', "")).collect::<Vec<_>>().join(",")
}

fn not_a_model() -> EngineError {
    EngineError::Other("The range doesn't hold a Solver model saved with Save Model.".into())
}

/// How many values the options row of Save Model holds at least: method, non-negative,
/// population size, mutation rate, random seed, require bounds, convergence and generations
/// without improvement.
const OPTIONS_FIELDS: usize = 8;

/// The options row of Save Model, `{method, non-negative, …}`, into `model`. Refused when it
/// holds fewer than [`OPTIONS_FIELDS`] values or a value that isn't a number: a row cut short
/// or edited by hand would otherwise load with default options in place of the saved ones.
fn read_options_row(model: &mut Json, list: &str) -> Result<()> {
    let incomplete = || {
        EngineError::Other(format!(
            "The model's options row doesn't hold the {OPTIONS_FIELDS} or more numbers Save Model writes, so the range isn't a complete model saved with Save Model."
        ))
    };
    // Every value is checked; only the first ones are kept (the row is one formula, so its
    // length is bounded by the formula's).
    let mut v: Vec<f64> = Vec::with_capacity(16);
    let mut count = 0usize;
    for t in list.split([',', ';']) {
        let x = num_text(t).ok_or_else(incomplete)?;
        count = count.saturating_add(1);
        if v.len() < 16 {
            v.push(x);
        }
    }
    if count < OPTIONS_FIELDS {
        return Err(incomplete());
    }
    let get = |k: usize| v.get(k).copied();
    model["method"] = json!(match get(0).map(|x| x as i64) {
        Some(2) => "simplex",
        Some(3) => "evolutionary",
        _ => "grg",
    });
    model["nonNegative"] = json!(get(1) != Some(2.0));
    for (k, key) in [
        (2, "populationSize"),
        (3, "mutationRate"),
        (4, "randomSeed"),
        (6, "convergence"),
        (7, "generationsWithoutImprovement"),
        (8, "maxEvaluations"),
        (9, "integerOptimality"),
    ] {
        if let Some(x) = get(k) {
            model[key] = json!(x);
        }
    }
    model["requireBounds"] = json!(get(5) != Some(2.0));
    Ok(())
}

/// One constraint row of Save Model (`AND(relation)`) as a `data.solver` constraint.
fn read_constraint_row(t: String) -> Result<String> {
    let t = call_args(&t, "AND").unwrap_or(t);
    if let Some((x, y)) = split_unquoted(&t, "=INT(").or_else(|| split_unquoted(&t, "=int(")) {
        if y.strip_suffix(')') != Some(x) {
            return Err(not_a_model());
        }
        return Ok(format!("{} int", local_refs(x)));
    }
    if let Some(x) = t.strip_prefix('(').and_then(|x| x.strip_suffix("=1)")).and_then(|x| split_unquoted(x, "=0)+(")).filter(|(a, b)| a == b) {
        return Ok(format!("{} bin", local_refs(x.0)));
    }
    let (l, rel, r) = split_constraint(&t).ok_or_else(not_a_model)?;
    let r = r.trim_matches('"');
    Ok(match r.to_ascii_lowercase().as_str() {
        "integer" => format!("{} int", local_refs(&l)),
        "binary" => format!("{} bin", local_refs(&l)),
        _ if num_text(r).is_some() => format!("{} {} {r}", local_refs(&l), rel.text()),
        _ => format!("{} {} {}", local_refs(&l), rel.text(), local_refs(r)),
    })
}

/// Reads formula text written by Save Model back into `data.solver` params.
pub(super) fn parse_model_formulas(rows: &[String]) -> Result<Json> {
    let body = |t: &str| t.trim().trim_start_matches('=').replace(' ', "");
    let first = body(rows.first().ok_or_else(not_a_model)?);
    let (goal, objective, value) = if let Some(o) = call_args(&first, "MAX") {
        ("max", o, 0.0)
    } else if let Some(o) = call_args(&first, "MIN") {
        ("min", o, 0.0)
    } else {
        let (o, v) = split_unquoted(&first, "=").ok_or_else(not_a_model)?;
        ("value", o.to_string(), num_text(v).ok_or_else(not_a_model)?)
    };
    let variables = call_args(&body(rows.get(1).ok_or_else(not_a_model)?), "COUNT").ok_or_else(not_a_model)?;
    let mut model = json!({"objective": local_refs(&objective), "goal": goal, "value": value, "variables": local_refs(&variables)});
    let mut constraints = Vec::new();
    let rest = rows.get(2..).unwrap_or(&[]);
    for (i, row) in rest.iter().enumerate() {
        let t = body(row);
        if let Some(list) = t.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
            // The options: the last row.
            if i + 1 != rest.len() {
                return Err(not_a_model());
            }
            read_options_row(&mut model, list)?;
            break;
        }
        constraints.push(json!(read_constraint_row(t)?));
    }
    model["constraints"] = json!(constraints);
    Ok(model)
}

/// Integer Optimality as a percent from its stored fraction. The product is rounded to 1E-10 %,
/// so a fraction such as 0.07 reads back as 7 and not 7.000000000000001.
fn percent(fraction: f64) -> f64 {
    let p = fraction * 100.0;
    let rounded = (p * 1e10).round() / 1e10;
    if rounded.is_finite() { rounded } else { p }
}

/// A number read from a hidden name as JSON: a whole number as an integer, so the Solver dialog
/// shows 20000 and not 20000.0.
fn number(v: f64) -> Json {
    // Whole numbers below 2^53 are exact as integers.
    if v.fract() == 0.0 && v.abs() < 9_007_199_254_740_992.0 { json!(v as i64) } else { json!(v) }
}
