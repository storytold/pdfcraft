use super::model::Goal;
use super::problem::Evaluator;
use super::{CellRef, EPS, LP_WORK};

// ---------------------------------------------------------------- evaluation

type BoundRow = (Vec<(usize, f64)>, Sense, f64);

/// A row of an LP: coefficients, sense and right side.
pub(super) type LpRow = (Vec<f64>, Sense, f64);

/// Objective (gradient, constant) and per-row (gradient, constant).
pub(super) type Linearized = ((Vec<f64>, f64), Vec<(Vec<f64>, f64)>);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Sense {
    Le,
    Ge,
    Eq,
}

/// A scalar constraint `lhs (sense) rhs`.
#[derive(Clone, Debug)]
pub(super) struct Row {
    pub(super) lhs: CellRef,
    pub(super) sense: Sense,
    pub(super) rhs: Option<CellRef>,
    pub(super) rhs_const: f64,
}

/// What a relaxation (or the whole solve) ended with.
#[derive(Clone, Debug)]
pub(super) enum Outcome {
    /// x, objective to minimise, and whether the solver converged (vs. ran out of budget).
    Solved(Vec<f64>, f64, bool),
    Infeasible,
    Unbounded,
    Limit,
}

/// A column of the simplex tableau standing for a variable: `x_var = offset + Σ sign·y`.
pub(super) struct Map {
    pub(super) var: usize,
    pub(super) sign: f64,
}

struct VariableMap {
    columns: Vec<Map>,
    offset: Vec<f64>,
    bound_rows: Vec<BoundRow>,
}

/// The final tableau of an optimal LP, for the Sensitivity report.
pub(super) struct FinalTableau {
    /// Rows `B⁻¹A | B⁻¹b`.
    pub(super) t: Vec<Vec<f64>>,
    pub(super) basis: Vec<usize>,
    /// The phase-2 reduced costs.
    pub(super) obj: Vec<f64>,
    pub(super) width: usize,
    /// The first artificial column.
    pub(super) first_art: usize,
    pub(super) cols: Vec<Map>,
    /// Per input row: the column that started as its unit column, and whether the row was
    /// negated to make its right side non-negative.
    pub(super) ident: Vec<usize>,
    pub(super) flipped: Vec<bool>,
}

struct Tableau {
    rows: Vec<Vec<f64>>,
    basis: Vec<usize>,
    width: usize,
    decision_cols: usize,
    slack_cols: usize,
    artificial_cols: usize,
}

fn map_variables(n: usize, lo: &[f64], hi: &[f64]) -> VariableMap {
    let mut columns = Vec::new();
    let mut offset = vec![0.0; n];
    let mut bound_rows = Vec::new();
    for j in 0..n {
        let lower = lo.get(j).copied().unwrap_or(f64::NEG_INFINITY);
        let upper = hi.get(j).copied().unwrap_or(f64::INFINITY);
        if lower.is_finite() {
            if let Some(value) = offset.get_mut(j) {
                *value = lower;
            }
            columns.push(Map { var: j, sign: 1.0 });
            if upper.is_finite() {
                bound_rows.push((vec![(columns.len() - 1, 1.0)], Sense::Le, upper - lower));
            }
        } else if upper.is_finite() {
            if let Some(value) = offset.get_mut(j) {
                *value = upper;
            }
            columns.push(Map { var: j, sign: -1.0 });
        } else {
            columns.push(Map { var: j, sign: 1.0 });
            columns.push(Map { var: j, sign: -1.0 });
        }
    }
    VariableMap { columns, offset, bound_rows }
}

/// The rows over the tableau's columns, each with a non-negative right side, and per row
/// whether it was negated to get there.
fn normalize_rows(rows: &[(Vec<f64>, Sense, f64)], bound_rows: Vec<BoundRow>, columns: &[Map], offset: &[f64]) -> (Vec<LpRow>, Vec<bool>) {
    let width = columns.len();
    let mut transformed = Vec::with_capacity(rows.len() + bound_rows.len());
    for (coefficients, sense, rhs) in rows {
        let mut row = vec![0.0; width];
        for (index, mapping) in columns.iter().enumerate() {
            if let Some(value) = row.get_mut(index) {
                *value = coefficients.get(mapping.var).copied().unwrap_or(0.0) * mapping.sign;
            }
        }
        let shift: f64 = coefficients.iter().zip(offset).map(|(coefficient, base)| coefficient * base).sum();
        transformed.push((row, *sense, rhs - shift));
    }
    for (entries, sense, rhs) in bound_rows {
        let mut row = vec![0.0; width];
        for (index, value) in entries {
            if let Some(slot) = row.get_mut(index) {
                *slot = value;
            }
        }
        transformed.push((row, sense, rhs));
    }
    // Rows with a negative right side are negated so every right side is >= 0.
    let flipped = transformed.iter().map(|row| row.2 < 0.0).collect();
    let normalized = transformed
        .into_iter()
        .map(|(row, sense, rhs)| {
            if rhs < 0.0 {
                let sense = match sense {
                    Sense::Le => Sense::Ge,
                    Sense::Ge => Sense::Le,
                    Sense::Eq => Sense::Eq,
                };
                (row.iter().map(|v| -v).collect(), sense, -rhs)
            } else {
                (row, sense, rhs)
            }
        })
        .collect();
    (normalized, flipped)
}

fn build_tableau(normalized: &[(Vec<f64>, Sense, f64)], decision_cols: usize, slack_cols: usize, artificial_cols: usize) -> Tableau {
    let width = decision_cols + slack_cols + artificial_cols;
    let mut rows = Vec::with_capacity(normalized.len());
    let mut basis = vec![0usize; normalized.len()];
    let (mut slack, mut artificial) = (decision_cols, decision_cols + slack_cols);
    for (i, (coefficients, sense, rhs)) in normalized.iter().enumerate() {
        let mut row = vec![0.0; width + 1];
        for (value, coefficient) in row.iter_mut().zip(coefficients) {
            *value = *coefficient;
        }
        let basic = match sense {
            Sense::Le => {
                if let Some(value) = row.get_mut(slack) {
                    *value = 1.0;
                }
                let basic = slack;
                slack += 1;
                basic
            }
            Sense::Ge => {
                if let Some(value) = row.get_mut(slack) {
                    *value = -1.0;
                }
                slack += 1;
                if let Some(value) = row.get_mut(artificial) {
                    *value = 1.0;
                }
                let basic = artificial;
                artificial += 1;
                basic
            }
            Sense::Eq => {
                if let Some(value) = row.get_mut(artificial) {
                    *value = 1.0;
                }
                let basic = artificial;
                artificial += 1;
                basic
            }
        };
        if let Some(value) = row.get_mut(width) {
            *value = *rhs;
        }
        if let Some(slot) = basis.get_mut(i) {
            *slot = basic;
        }
        rows.push(row);
    }
    Tableau { rows, basis, width, decision_cols, slack_cols, artificial_cols }
}

fn phase_one(tableau: &mut Tableau, normalized: &[(Vec<f64>, Sense, f64)], budget: &mut u64) -> Result<(), Outcome> {
    if tableau.artificial_cols == 0 {
        return Ok(());
    }
    let mut objective = vec![0.0; tableau.width + 1];
    for column in tableau.decision_cols + tableau.slack_cols..tableau.width {
        if let Some(value) = objective.get_mut(column) {
            *value = 1.0;
        }
    }
    for (i, row) in tableau.rows.iter().enumerate() {
        if tableau.basis.get(i).is_some_and(|basic| *basic >= tableau.decision_cols + tableau.slack_cols) {
            for (value, coefficient) in objective.iter_mut().zip(row) {
                *value -= coefficient;
            }
        }
    }
    let all = vec![true; tableau.width];
    if matches!(simplex(&mut tableau.rows, &mut tableau.basis, &mut objective, &all, budget), Step::Limit) {
        return Err(Outcome::Limit);
    }
    let scale = normalized.iter().map(|row| row.2.abs()).fold(1.0f64, f64::max);
    if -objective.get(tableau.width).copied().unwrap_or(0.0) > 1e-7 * scale {
        return Err(Outcome::Infeasible);
    }
    for i in 0..tableau.rows.len() {
        if tableau.basis.get(i).is_none_or(|basic| *basic < tableau.decision_cols + tableau.slack_cols) {
            continue;
        }
        let pivot_col = tableau
            .rows
            .get(i)
            .and_then(|row| (0..tableau.decision_cols + tableau.slack_cols).find(|&column| row.get(column).is_some_and(|value| value.abs() > EPS)));
        if let Some(column) = pivot_col {
            pivot(&mut tableau.rows, &mut objective, i, column);
            if let Some(basic) = tableau.basis.get_mut(i) {
                *basic = column;
            }
        }
    }
    Ok(())
}

/// Depth-first branch and bound over a relaxation solver; returns the best integer point.
pub(super) fn branch_and_bound(
    lo: &[f64],
    hi: &[f64],
    integer: &[bool],
    max_nodes: usize,
    gap: f64,
    relax: &mut dyn FnMut(&[f64], &[f64]) -> Outcome,
) -> (Outcome, usize) {
    let mut stack: Vec<(Vec<f64>, Vec<f64>)> = vec![(lo.to_vec(), hi.to_vec())];
    let mut best: Option<(Vec<f64>, f64)> = None;
    let mut nodes = 0usize;
    let mut limited = false;
    let mut root = true;
    while let Some((l, h)) = stack.pop() {
        if nodes >= max_nodes {
            limited = true;
            break;
        }
        nodes += 1;
        let out = relax(&l, &h);
        let was_root = std::mem::replace(&mut root, false);
        let (x, obj, converged) = match out {
            Outcome::Solved(x, obj, c) => (x, obj, c),
            Outcome::Unbounded if was_root => return (Outcome::Unbounded, nodes),
            Outcome::Limit => {
                limited = true;
                break;
            }
            _ => continue,
        };
        if !converged {
            limited = true;
        }
        if let Some((_, b)) = &best
            && obj >= b - gap * b.abs().max(1e-12) - 1e-9 * (1.0 + b.abs())
        {
            continue;
        }
        // The most fractional integer variable.
        let frac = x
            .iter()
            .zip(integer)
            .enumerate()
            .filter(|(_, (_, int))| **int)
            .map(|(j, (v, _))| (j, (v - v.round()).abs()))
            .filter(|(_, f)| *f > 1e-6)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match frac {
            None => {
                let snapped: Vec<f64> = x.iter().zip(integer).map(|(v, int)| if *int { v.round() } else { *v }).collect();
                best = Some((snapped, obj));
            }
            Some((j, _)) => {
                let v = x.get(j).copied().unwrap_or(0.0);
                let (mut down_h, mut up_l) = (h.clone(), l.clone());
                if let Some(e) = down_h.get_mut(j) {
                    *e = e.min(v.floor());
                }
                if let Some(e) = up_l.get_mut(j) {
                    *e = e.max(v.ceil());
                }
                // Explore the nearer side first (pushed last).
                if v - v.floor() < 0.5 {
                    stack.push((up_l, h));
                    stack.push((l, down_h));
                } else {
                    stack.push((l, down_h));
                    stack.push((up_l, h));
                }
            }
        }
    }
    match best {
        Some((x, obj)) => (Outcome::Solved(x, obj, !limited), nodes),
        None if limited => (Outcome::Limit, nodes),
        None => (Outcome::Infeasible, nodes),
    }
}

// ---------------------------------------------------------------- simplex

/// Pivots the tableau on (r, j), updating the objective row.
fn pivot(t: &mut [Vec<f64>], obj: &mut [f64], r: usize, j: usize) {
    let Some(prow) = t.get_mut(r) else { return };
    let pv = prow.get(j).copied().unwrap_or(0.0);
    if pv == 0.0 {
        return;
    }
    for v in prow.iter_mut() {
        *v /= pv;
    }
    let prow = prow.clone();
    for (i, row) in t.iter_mut().enumerate() {
        if i == r {
            continue;
        }
        let f = row.get(j).copied().unwrap_or(0.0);
        if f != 0.0 {
            for (v, p) in row.iter_mut().zip(&prow) {
                *v -= f * p;
            }
        }
    }
    let f = obj.get(j).copied().unwrap_or(0.0);
    if f != 0.0 {
        for (v, p) in obj.iter_mut().zip(&prow) {
            *v -= f * p;
        }
    }
}

enum Step {
    Optimal,
    Unbounded,
    Limit,
}

/// Minimises with the reduced costs in `obj` (last entry: −objective). Columns not `allowed`
/// never enter.
fn simplex(t: &mut [Vec<f64>], basis: &mut [usize], obj: &mut [f64], allowed: &[bool], budget: &mut u64) -> Step {
    let n = obj.len().saturating_sub(1);
    let cost = (t.len() as u64 + 1).saturating_mul(n as u64 + 1).max(1);
    let mut stalled = 0u32;
    loop {
        if *budget < cost {
            return Step::Limit;
        }
        *budget -= cost;
        let bland = stalled > 50;
        let mut enter = None;
        let mut best = -EPS;
        for (j, d) in obj.iter().take(n).enumerate() {
            if !allowed.get(j).copied().unwrap_or(false) || *d >= -EPS {
                continue;
            }
            if bland {
                enter = Some(j);
                break;
            }
            if *d < best {
                best = *d;
                enter = Some(j);
            }
        }
        let Some(j) = enter else { return Step::Optimal };
        let mut leave: Option<(usize, f64)> = None;
        for (i, row) in t.iter().enumerate() {
            let a = row.get(j).copied().unwrap_or(0.0);
            if a <= EPS {
                continue;
            }
            let ratio = row.get(n).copied().unwrap_or(0.0) / a;
            let better = match leave {
                None => true,
                Some((li, lr)) => ratio < lr - 1e-12 || (ratio <= lr + 1e-12 && basis.get(i) < basis.get(li)),
            };
            if better {
                leave = Some((i, ratio));
            }
        }
        let Some((r, ratio)) = leave else { return Step::Unbounded };
        stalled = if ratio <= EPS { stalled + 1 } else { 0 };
        pivot(t, obj, r, j);
        if let Some(b) = basis.get_mut(r) {
            *b = j;
        }
    }
}

/// Minimises `c·x` subject to `rows` (coefficients, sense, rhs) and `lo ≤ x ≤ hi`.
pub(super) fn solve_lp(c: &[f64], rows: &[(Vec<f64>, Sense, f64)], lo: &[f64], hi: &[f64], budget: &mut u64) -> Outcome {
    solve_lp_tableau(c, rows, lo, hi, budget).0
}

/// [`solve_lp`], with the final tableau when it is optimal.
pub(super) fn solve_lp_tableau(
    c: &[f64],
    rows: &[(Vec<f64>, Sense, f64)],
    lo: &[f64],
    hi: &[f64],
    budget: &mut u64,
) -> (Outcome, Option<FinalTableau>) {
    if lo.iter().zip(hi).any(|(l, h)| l > h) {
        return (Outcome::Infeasible, None);
    }
    // x_j = offset + Σ sign·y_k with y ≥ 0.
    let mapping = map_variables(c.len(), lo, hi);
    let VariableMap { columns: cols, offset, bound_rows: extra } = mapping;
    let ny = cols.len();
    // Rows over y: (coefficients, sense, rhs).
    let (normalized, flipped) = normalize_rows(rows, extra, &cols, &offset);
    let n_slack = normalized.iter().filter(|row| row.1 != Sense::Eq).count();
    let n_art = normalized.iter().filter(|row| row.1 != Sense::Le).count();
    let mut tableau = build_tableau(&normalized, ny, n_slack, n_art);
    // Each row's starting basic column is its unit column.
    let ident = tableau.basis.clone();
    if let Err(outcome) = phase_one(&mut tableau, &normalized, budget) {
        return (outcome, None);
    }
    let Tableau { rows: mut t, mut basis, width, decision_cols: ny, slack_cols: n_slack, .. } = tableau;
    let is_art = |column: usize| column >= ny + n_slack && column < width;
    // Phase 2: the real costs; artificials may not re-enter.
    let mut cost = vec![0.0; width + 1];
    for (k, mp) in cols.iter().enumerate() {
        if let Some(v) = cost.get_mut(k) {
            *v = c.get(mp.var).copied().unwrap_or(0.0) * mp.sign;
        }
    }
    let mut obj = cost.clone();
    for (i, row) in t.iter().enumerate() {
        let cb = basis.get(i).and_then(|b| cost.get(*b)).copied().unwrap_or(0.0);
        if cb != 0.0 {
            for (v, x) in obj.iter_mut().zip(row) {
                *v -= cb * x;
            }
        }
    }
    let allowed: Vec<bool> = (0..width).map(|j| !is_art(j)).collect();
    match simplex(&mut t, &mut basis, &mut obj, &allowed, budget) {
        Step::Limit => return (Outcome::Limit, None),
        Step::Unbounded => return (Outcome::Unbounded, None),
        Step::Optimal => {}
    }
    let mut y = vec![0.0; width];
    for (i, row) in t.iter().enumerate() {
        if let (Some(b), Some(v)) = (basis.get(i), row.get(width))
            && let Some(e) = y.get_mut(*b)
        {
            *e = *v;
        }
    }
    let mut x = offset;
    for (k, mp) in cols.iter().enumerate() {
        if let Some(e) = x.get_mut(mp.var) {
            *e += mp.sign * y.get(k).copied().unwrap_or(0.0);
        }
    }
    let val: f64 = c.iter().zip(&x).map(|(a, b)| a * b).sum();
    let tableau = FinalTableau { t, basis, obj, width, first_art: ny + n_slack, cols, ident, flipped };
    (Outcome::Solved(x, val, true), Some(tableau))
}

/// The LP to minimise for `goal`, from the linearised model: the costs, and the rows
/// (coefficients, sense, right side) in the order of `model_rows`.
pub(super) fn linear_program(goal: Goal, cf: &[f64], f_const: f64, lin: &[(Vec<f64>, f64)], model_rows: &[Row]) -> (Vec<f64>, Vec<LpRow>) {
    let mut rows: Vec<LpRow> = lin.iter().zip(model_rows).map(|((a, k), r)| (a.clone(), r.sense, -k)).collect();
    let c = match goal {
        Goal::Max => cf.iter().map(|v| -v).collect(),
        Goal::Min => cf.to_vec(),
        Goal::Value(v) => {
            rows.push((cf.to_vec(), Sense::Eq, v - f_const));
            vec![0.0; cf.len()]
        }
    };
    (c, rows)
}

/// The linear program Simplex LP solves: the model linearised at `x0`, its objective
/// coefficients, the costs and rows for the goal, and the work limit for its size.
pub(super) struct SimplexProgram {
    pub(super) cf: Vec<f64>,
    pub(super) c: Vec<f64>,
    pub(super) rows: Vec<LpRow>,
    pub(super) work: u64,
}

/// Builds the [`SimplexProgram`] at `x0`. The solve and the Sensitivity report both build it
/// here from the start values, so the report's solve repeats the solve's pivots and returns the
/// same vertex: linearising at another point changes the coefficients in their last bits, and
/// that can break a tie between pivots the other way.
pub(super) fn simplex_program(ev: &mut Evaluator, goal: Goal, x0: &[f64]) -> std::result::Result<SimplexProgram, &'static str> {
    let ((cf, f_const), lin) = linearize(ev, x0)?;
    let (c, rows) = linear_program(goal, &cf, f_const, &lin, &ev.rows);
    let size = (rows.len() as u64 + 2 * x0.len() as u64 + 1) * (4 * x0.len() as u64 + 2 * rows.len() as u64 + 1);
    Ok(SimplexProgram { cf, c, rows, work: LP_WORK.max(size) })
}

/// Linearises the model at `x0` (n + 1 evaluations) and checks it at one more point. Returns the
/// objective gradient and constant, and per row the gradient and constant of `lhs − rhs`.
pub(super) fn linearize(ev: &mut Evaluator, x0: &[f64]) -> std::result::Result<Linearized, &'static str> {
    let n = x0.len();
    let p0 = ev.eval(x0).ok_or("budget")?;
    if !p0.f.is_finite() || p0.g.iter().any(|v| !v.is_finite()) {
        return Err("error");
    }
    let mut cf = vec![0.0; n];
    let mut ga = vec![vec![0.0; n]; p0.g.len()];
    for j in 0..n {
        let mut x = x0.to_vec();
        if let Some(v) = x.get_mut(j) {
            *v += 1.0;
        }
        let p = ev.eval(&x).ok_or("budget")?;
        if let Some(v) = cf.get_mut(j) {
            *v = p.f - p0.f;
        }
        for (row, (gi, g0)) in ga.iter_mut().zip(p.g.iter().zip(&p0.g)) {
            if let Some(v) = row.get_mut(j) {
                *v = gi - g0;
            }
        }
    }
    let dot = |a: &[f64], x: &[f64]| a.iter().zip(x).map(|(p, q)| p * q).sum::<f64>();
    let f_const = p0.f - dot(&cf, x0);
    let rows: Vec<(Vec<f64>, f64)> = ga.into_iter().zip(&p0.g).map(|(a, g0)| (a.clone(), g0 - dot(&a, x0))).collect();
    // Linearity check at a spread-out point.
    let xt: Vec<f64> =
        x0.iter().enumerate().map(|(j, v)| v + if j % 2 == 0 { 1.7 + 0.3 * (j % 5) as f64 } else { -0.6 - 0.2 * (j % 3) as f64 }).collect();
    let pt = ev.eval(&xt).ok_or("budget")?;
    let close = |actual: f64, pred: f64, terms: f64| actual.is_finite() && (actual - pred).abs() <= 1e-6 * (1.0 + actual.abs() + terms);
    let terms = |a: &[f64]| a.iter().zip(&xt).map(|(p, q)| (p * q).abs()).sum::<f64>();
    if !close(pt.f, f_const + dot(&cf, &xt), terms(&cf)) {
        return Err("nonlinear");
    }
    for ((a, k), g) in rows.iter().zip(&pt.g) {
        if !close(*g, k + dot(a, &xt), terms(a)) {
            return Err("nonlinear");
        }
    }
    Ok(((cf, f_const), rows))
}
