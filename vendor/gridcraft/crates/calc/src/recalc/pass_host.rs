use super::graph::{HashMap, HashSet, Key};
use super::{Evaluator, Host, Workbook, stack_position};
use crate::spill_index::{Cover, SpillIndex};
use gridcraft_core::{Array, CellError, CellRef, RangeRef, Value};
use gridcraft_formula::Expr;

/// Formulas a formula may calculate on demand one inside the other (each needing one another's
/// value before it is done). Each level takes stack, so a chain deeper than this is suspended and
/// calculated from the top instead (see [`PassHost::calculate`]): long dependency chains calculate
/// whatever order they come in, with bounded stack.
const MAX_NESTED: usize = 64;

/// Formula evaluations iterative calculation may spend on one recalculation (passes × formulas
/// evaluated again), however high Maximum Iterations is.
const MAX_ITERATION_WORK: usize = 10_000_000;

/// A block's results by last row: its sum and count, or `None` when it can't be summed from constants.
type BlockSums = std::collections::BTreeMap<u32, Option<(f64, u64)>>;

pub(super) struct PassHost<'a> {
    wb: &'a Workbook,
    exprs: &'a HashMap<Key, std::sync::Arc<Expr>>,
    pub(super) results: HashMap<Key, Value>,
    pending: HashSet<Key>,
    in_progress: HashSet<Key>,
    /// New spills found during the pass: anchor → array.
    pub(super) spills: HashMap<Key, std::sync::Arc<Array>>,
    /// Where the areas in `spills` are, so a read finds the ones it meets without scanning them all.
    spill_index: SpillIndex,
    /// Used range per sheet: the workbook doesn't change during a pass, and working it out walks
    /// every row, so once per sheet rather than once per range read.
    used: HashMap<usize, Option<RangeRef>>,
    rng: &'a mut u64,
    /// Who held a spill before the calculation; unset: every formula did (reading order decides).
    pub(super) standing: Option<&'a super::Standing>,
    now: f64,
    cycle: bool,
    pub(super) cycles: Vec<Key>,
    depth: usize,
    /// A formula needed at [`MAX_NESTED`] levels deep: the evaluations in progress are abandoned
    /// and it is calculated first.
    suspend: Option<Key>,
    /// Formulas whose evaluation was abandoned for another one, to be calculated again after it.
    /// They all need it, so reading one of them while it is calculated is a circular reference,
    /// as reading a formula in progress is.
    waiting: HashSet<Key>,
    /// The formulas abandoned since the last suspension.
    abandoned: Vec<Key>,
    /// Other formulas read at [`MAX_NESTED`] levels deep by the evaluations being abandoned. They
    /// are calculated along with the suspended one, rather than one per restart (a wide range
    /// read at the limit would restart the abandoned evaluations once per cell in it).
    also: Vec<Key>,
    /// Suspensions so far (tests check that a wide read at the limit needs one, not one per cell).
    #[cfg(test)]
    pub(super) suspensions: usize,
    /// Spilled values looked at by range reads (tests check that a read outside a large spill
    /// looks at none of it).
    #[cfg(test)]
    pub(super) spill_visits: usize,
    /// The formulas in `cycles`.
    cycle_seen: HashSet<Key>,
    /// Iterative calculation: a circular reference reads the cell's previous value instead of
    /// giving `#CIRC!`.
    pub(super) iterate: bool,
    /// Values from the previous iteration; cells not in it read their stored value.
    prev: HashMap<Key, Value>,
    /// Formulas being evaluated, innermost last (kept only when iterating).
    stack: Vec<Key>,
    /// Formulas found on a circular reference (when iterating).
    pub(super) members: HashSet<Key>,
    /// Results that read a previous value, directly or through other formulas.
    pub(super) tainted: HashSet<Key>,
    /// Whether the formula being evaluated has read a previous value.
    taint: bool,
    /// "Set precision as displayed" is on: results are rounded to their cells' formats before
    /// they are stored or read by other formulas.
    precision: Option<crate::DisplayedPrecision>,
    /// `stack_position()` where the outermost formula being calculated started.
    stack_top: usize,
    /// Whether an evaluation ran past the stack budget since the innermost formula started.
    out_of_stack: bool,
    /// Sum and count of blocks that hold only constants, for `range_sum`. Keyed by sheet, first
    /// cell and last column; each value maps a last row to the block's result (`None`: the block
    /// has a formula, a Python cell, an error value or a spilled value, so every longer one has too).
    sums: HashMap<(usize, CellRef, u32), BlockSums>,
    /// The kept results of the Python cells in the pass: a Python cell isn't evaluated, its
    /// result is laid out (an array spills from the cell, as a formula's does). Each one is in
    /// `pending` until it is laid out, so a formula that reads it first gets its new value.
    python: HashMap<Key, Value>,
}

impl<'a> PassHost<'a> {
    pub(super) fn new(wb: &'a Workbook, exprs: &'a HashMap<Key, std::sync::Arc<Expr>>, rng: &'a mut u64, now: f64) -> Self {
        PassHost {
            wb,
            exprs,
            results: HashMap::with_capacity_and_hasher(exprs.len(), Default::default()),
            pending: exprs.keys().copied().collect(),
            in_progress: HashSet::default(),
            spills: HashMap::default(),
            spill_index: SpillIndex::default(),
            used: HashMap::default(),
            rng,
            standing: None,
            now,
            cycle: false,
            cycles: vec![],
            depth: 0,
            suspend: None,
            waiting: HashSet::default(),
            abandoned: Vec::new(),
            also: Vec::new(),
            #[cfg(test)]
            suspensions: 0,
            #[cfg(test)]
            spill_visits: 0,
            cycle_seen: HashSet::default(),
            iterate: false,
            prev: HashMap::default(),
            stack: Vec::new(),
            members: HashSet::default(),
            tainted: HashSet::default(),
            taint: false,
            precision: wb.calc.precision_as_displayed.then(crate::DisplayedPrecision::new),
            stack_top: 0,
            out_of_stack: false,
            sums: HashMap::default(),
            python: HashMap::default(),
        }
    }

    /// Adds the dirty Python cells of the pass and their kept results: like the formulas, they
    /// are pending until laid out, so every read path calculates them before reading them.
    pub(super) fn with_python(mut self, python: HashMap<Key, Value>) -> Self {
        self.pending.extend(python.keys().copied());
        self.python = python;
        self
    }
}

/// Most spill areas a range read meets before they are checked for overlapping one another by
/// walking all of the pass's spills instead of comparing pairs.
const MAX_UNORDERED_HITS: usize = 16;

/// Whether an array at `anchor` covers `c`.
fn spill_covers(anchor: CellRef, arr: &Array, c: CellRef) -> bool {
    c.row >= anchor.row && c.col >= anchor.col && ((c.row - anchor.row) as usize) < arr.rows && ((c.col - anchor.col) as usize) < arr.cols
}

/// Whether two of the spill areas anchored at `hits` share a cell inside `range`.
fn spill_hits_overlap(index: &SpillIndex, sheet: usize, hits: &[CellRef], range: RangeRef) -> bool {
    let rects: Vec<RangeRef> = hits.iter().filter_map(|a| index.area(sheet, *a)).filter(|(_, empty)| !empty).map(|(r, _)| r).collect();
    rects.iter().enumerate().any(|(i, a)| {
        rects.iter().skip(i + 1).any(|b| {
            a.intersects(b)
                && RangeRef::new(
                    CellRef::new(a.start.row.max(b.start.row), a.start.col.max(b.start.col)),
                    CellRef::new(a.end.row.min(b.end.row), a.end.col.min(b.end.col)),
                )
                .intersects(&range)
        })
    })
}

/// Whether an array result at `k` can't spill: it is in a table, or its area runs off the sheet,
/// overlaps merged cells, holds other cells, or the spill of another formula that outranks it on
/// a cell they share (see `Standing`; unless that formula is recalculated in this pass, and so
/// may spill elsewhere now). A spill it outranks gives way to it when written back.
fn spill_blocked(host: &PassHost<'_>, (sheet, anchor): Key, a: &Array) -> bool {
    let Some(sh) = host.wb.sheet(sheet) else { return false };
    let end =
        CellRef::new(anchor.row.saturating_add((a.rows as u32).saturating_sub(1)), anchor.col.saturating_add((a.cols as u32).saturating_sub(1)));
    let range = RangeRef::new(anchor, end);
    let own = sh.spill_ranges.get(&anchor);
    let default = super::Standing::default();
    let standing = host.standing.unwrap_or(&default);
    let spilled_over = |c: CellRef| {
        !own.is_some_and(|o| o.contains(c))
            && sh.spill_ranges.iter().any(|(o, r)| {
                // A formula or Python cell recalculated in this pass lays its array out again.
                *o != anchor
                    && r.contains(c)
                    && !host.exprs.contains_key(&(sheet, *o))
                    && !host.python.contains_key(&(sheet, *o))
                    && standing.outranks_at(((sheet, *o), Some(*r)), ((sheet, anchor), own.copied()), c)
            })
    };
    end.row >= gridcraft_core::MAX_ROWS
        || end.col >= gridcraft_core::MAX_COLS
        || sh.merges.iter().any(|m| m.intersects(&range))
        || sh.tables.iter().any(|t| t.range.contains(anchor))
        || sh.cells.iter_range(range).any(|(c, cell)| c != anchor && (!cell.value.is_empty() || cell.formula.is_some()))
        || sh.spill.range(range.start..=range.end).any(|(c, _)| range.contains(*c) && spilled_over(*c))
}

/// Whether `arr`, anchored at `anchor`, would spill exactly what the sheet already holds over
/// `range`: the same area, nothing new in the way, and the same values.
pub(super) fn spills_as_before(sheet: &gridcraft_model::Sheet, anchor: CellRef, range: RangeRef, arr: &Array) -> bool {
    let end =
        CellRef::new(anchor.row.saturating_add((arr.rows as u32).saturating_sub(1)), anchor.col.saturating_add((arr.cols as u32).saturating_sub(1)));
    RangeRef::new(anchor, end) == range
        && !sheet.merges.iter().any(|m| m.intersects(&range))
        && !sheet.tables.iter().any(|t| t.range.contains(anchor))
        && range
            .iter()
            .enumerate()
            .all(|(i, c)| c == anchor || (!sheet.cells.has(c) && sheet.spill.get(&c) == Some(arr.data.get(i).unwrap_or(&Value::Empty))))
}

/// The spilled values `sheet` stores inside `range`, passed to `f` until it returns false. Values
/// outside the range are not walked: with fewer rows in the range than spilled values, each row
/// is looked up on its own; otherwise one ordered scan visits each spilled value at most once.
/// Returns how many values were looked at.
fn stored_spill_in<'s>(sheet: &'s gridcraft_model::Sheet, range: RangeRef, f: &mut dyn FnMut(CellRef, &'s Value) -> bool) -> usize {
    let mut seen = 0usize;
    if sheet.spill.is_empty() {
        return 0;
    }
    if (range.height() as usize) < sheet.spill.len() {
        for row in range.start.row..=range.end.row {
            for (c, v) in sheet.spill.range(CellRef::new(row, range.start.col)..=CellRef::new(row, range.end.col)) {
                seen += 1;
                if !f(*c, v) {
                    return seen;
                }
            }
        }
    } else {
        for (c, v) in sheet.spill.range(range.start..=range.end) {
            seen += 1;
            if range.contains(*c) && !f(*c, v) {
                return seen;
            }
        }
    }
    seen
}

impl PassHost<'_> {
    /// Calculates `k` (and the formulas it reads) with bounded stack: when [`Self::compute`] would
    /// go deeper than [`MAX_NESTED`], it suspends what is in progress, the formula it needed is
    /// calculated from here, then the suspended ones again (reading the results kept so far).
    pub(super) fn calculate(&mut self, k: Key) {
        // Formulas to calculate, innermost last, each with the ones abandoned to calculate the
        // next.
        let mut work: Vec<(Key, Vec<Key>)> = vec![(k, Vec::new())];
        while let Some(&(top, _)) = work.last() {
            self.compute(top);
            match self.suspend.take() {
                Some(next) => {
                    #[cfg(test)]
                    {
                        self.suspensions += 1;
                    }
                    let held = std::mem::take(&mut self.abandoned);
                    self.waiting.extend(held.iter().copied());
                    if let Some(entry) = work.last_mut() {
                        entry.1 = held;
                    }
                    // The other formulas read at the limit are calculated after `next`, before the
                    // held evaluations restart. Each has nothing held of its own, so the held ones
                    // are released only when the last of them is done.
                    let mut seen: HashSet<Key> = HashSet::default();
                    seen.insert(next);
                    for s in std::mem::take(&mut self.also) {
                        if seen.insert(s) {
                            work.push((s, Vec::new()));
                        }
                    }
                    work.push((next, Vec::new()));
                }
                None => {
                    work.pop();
                    if let Some(entry) = work.last_mut() {
                        for a in entry.1.drain(..) {
                            self.waiting.remove(&a);
                        }
                    }
                }
            }
        }
    }

    fn compute(&mut self, k: Key) -> Value {
        if let Some(v) = self.results.get(&k) {
            let v = v.clone();
            if self.iterate && self.tainted.contains(&k) {
                self.taint = true;
            }
            return v;
        }
        if let Some(result) = self.python.get(&k).cloned() {
            return self.python_result(k, result);
        }
        let Some(expr) = self.exprs.get(&k) else {
            return self.wb.sheet(k.0).map(|s| s.value(k.1)).unwrap_or_default();
        };
        if self.suspend.is_some() {
            // Unwinding to `calculate`: the value is thrown away. A formula read at the limit is
            // calculated with the suspended one, so the evaluations are not restarted for each.
            if self.depth >= MAX_NESTED && !self.in_progress.contains(&k) && !self.waiting.contains(&k) {
                self.also.push(k);
            }
            return Value::Error(CellError::Calc);
        }
        if self.in_progress.contains(&k) || self.waiting.contains(&k) {
            self.cycle = true;
            if self.iterate {
                return self.circular_read(k);
            }
            // Once each, though a suspended evaluation meets it again.
            if self.cycle_seen.insert(k) {
                self.cycles.push(k);
            }
            return Value::Error(CellError::Circ);
        }
        if self.depth >= MAX_NESTED {
            self.suspend = Some(k);
            return Value::Error(CellError::Calc);
        }
        if self.depth == 0 {
            self.stack_top = stack_position();
        }
        self.in_progress.insert(k);
        self.depth += 1;
        let outer_taint = std::mem::replace(&mut self.taint, false);
        if self.iterate {
            self.stack.push(k);
        }
        let expr = std::sync::Arc::clone(expr);
        let outer_out_of_stack = std::mem::replace(&mut self.out_of_stack, false);
        let v = {
            let mut ev = Evaluator::new(self, k.0, k.1);
            ev.value(&expr)
        };
        let out_of_stack = std::mem::replace(&mut self.out_of_stack, outer_out_of_stack);
        if out_of_stack && self.depth > 1 && self.suspend.is_none() {
            // It ran out of the stack left below the formulas that read it: it is calculated
            // from the top instead, as a formula needed at `MAX_NESTED` levels deep is.
            self.suspend = Some(k);
            self.depth -= 1;
            self.in_progress.remove(&k);
            if self.iterate {
                self.stack.pop();
            }
            self.taint = outer_taint;
            return Value::Error(CellError::Calc);
        }
        if self.suspend.is_some() {
            // Abandoned: `calculate` evaluates this formula again once the one it needs is done.
            self.depth -= 1;
            self.in_progress.remove(&k);
            self.abandoned.push(k);
            if self.iterate {
                self.stack.pop();
            }
            self.taint = outer_taint;
            return Value::Error(CellError::Calc);
        }
        // A legacy array formula fills the range it was entered in.
        let legacy = self.wb.sheet(k.0).and_then(|s| s.cell(k.1)).and_then(|c| c.formula.as_ref()).and_then(|f| f.array);
        let v = match legacy {
            Some(range) => fit_to_range(v, range),
            None => v,
        };
        self.depth -= 1;
        self.in_progress.remove(&k);
        self.pending.remove(&k);
        if self.iterate {
            self.stack.pop();
            if self.taint {
                self.tainted.insert(k);
            } else {
                self.tainted.remove(&k);
            }
        }
        self.taint |= outer_taint;
        // A blank cell shows as 0 in a formula's result, inside an array as well (`=A1:A3`,
        // FILTER or SORT of a range with blanks). GROUPBY and PIVOTBY lay out blank cells
        // of their own, which stay blank.
        let keep_blanks = matches!(&*expr, Expr::Call(n, _) if matches!(n.as_str(), "GROUPBY" | "PIVOTBY"));
        let v = match v {
            Value::Array(a) => {
                let a = if keep_blanks || !a.data.iter().any(|x| matches!(x, Value::Empty)) {
                    a
                } else {
                    let data = a.data.iter().map(|x| if matches!(x, Value::Empty) { Value::Number(0.0) } else { x.clone() }).collect();
                    Array::new(a.rows, a.cols, data).map(std::sync::Arc::new).unwrap_or(a)
                };
                // Precision as displayed: each element takes the format of the cell it lands in.
                let a = match self.precision.as_mut() {
                    Some(p) if a.data.iter().any(|x| matches!(x, Value::Number(_))) => {
                        let cols = a.cols.max(1);
                        let data = a
                            .data
                            .iter()
                            .enumerate()
                            .map(|(i, x)| {
                                let at = CellRef::new(k.1.row.saturating_add((i / cols) as u32), k.1.col.saturating_add((i % cols) as u32));
                                p.round_value(self.wb, k.0, at, x.clone())
                            })
                            .collect();
                        Array::new(a.rows, a.cols, data).map(std::sync::Arc::new).unwrap_or(a)
                    }
                    _ => a,
                };
                // A dynamic-array formula anchors a spill range even while its result is one
                // value -- a FILTER matching one row, a SORT or UNIQUE over one item -- so `C1#`
                // is the 1 x 1 range and `ROWS(C1#)` is 1, as in Excel (#1491). A formula that
                // returns a scalar never reaches this arm, so it does not become an anchor.
                self.spill_index.insert(k, a.rows, a.cols);
                self.spills.insert(k, a.clone());
                a.data.first().cloned().unwrap_or_default()
            }
            Value::Empty => Value::Number(0.0),
            v @ Value::Number(_) => match self.precision.as_mut() {
                Some(p) => p.round_value(self.wb, k.0, k.1, v),
                None => v,
            },
            v => v,
        };
        // An array that can't spill is #SPILL! for the formulas evaluated after it in this pass too.
        let v = match self.spills.get(&k) {
            Some(a) if spill_blocked(self, k, a) => {
                self.spills.remove(&k);
                self.spill_index.remove(k);
                Value::Error(CellError::Spill)
            }
            _ => v,
        };
        self.results.insert(k, v.clone());
        v
    }

    /// A Python cell's kept result as its value: an array spills from the cell unless it is
    /// blocked, malformed or larger than [`gridcraft_model::python::MAX_RESULT_CELLS`] (only a
    /// crafted file holds one), which shows `#SPILL!`.
    fn python_result(&mut self, k: Key, result: Value) -> Value {
        self.pending.remove(&k);
        let v = match result {
            Value::Array(a) => {
                let sane = a.rows >= 1
                    && a.cols >= 1
                    && a.rows.checked_mul(a.cols).is_some_and(|n| n == a.data.len() && n <= gridcraft_model::python::MAX_RESULT_CELLS);
                if sane {
                    self.spill_index.insert(k, a.rows, a.cols);
                    self.spills.insert(k, a.clone());
                    a.data.first().cloned().unwrap_or_default()
                } else {
                    Value::Error(CellError::Spill)
                }
            }
            v => v,
        };
        let v = match self.spills.get(&k) {
            Some(a) if spill_blocked(self, k, a) => {
                self.spills.remove(&k);
                self.spill_index.remove(k);
                Value::Error(CellError::Spill)
            }
            _ => v,
        };
        self.results.insert(k, v.clone());
        v
    }

    /// A formula's previous value for iterative calculation: from the last iteration, else the
    /// value it shows. Blank, and the `#CIRC!` of a loop calculated with iteration off, are 0.
    fn previous(&self, k: Key) -> Value {
        let v = match self.prev.get(&k) {
            Some(v) => v.clone(),
            None => self.wb.sheet(k.0).map(|s| s.value(k.1)).unwrap_or_default(),
        };
        if matches!(v, Value::Empty | Value::Error(CellError::Circ)) { Value::Number(0.0) } else { v }
    }

    /// Reading `k` while it is being evaluated, with iterative calculation on: its previous value.
    /// The formulas around the loop are the ones whose changes decide when iteration stops.
    fn circular_read(&mut self, k: Key) -> Value {
        self.taint = true;
        match self.stack.iter().rposition(|s| *s == k) {
            Some(i) => self.members.extend(self.stack.get(i..).unwrap_or_default().iter().copied()),
            // Too deep to follow: the formula itself is watched.
            None => {
                self.members.insert(k);
            }
        }
        self.previous(k)
    }

    /// Iterative calculation, after a first pass over `order` that met circular references. The
    /// formulas that read a previous value (the loops and what depends on them) are evaluated
    /// again in the same order, each pass starting from the last one's values and reading what it
    /// has already produced (Gauss-Seidel), until no formula on a loop changes by `max_change` or
    /// more, or after `max_iterations` passes in all. Returns the number of passes.
    pub(super) fn iterate_loops(&mut self, order: &[Key], max_iterations: u32, max_change: f64) -> u32 {
        let again: Vec<Key> = order.iter().copied().filter(|k| self.tainted.contains(k)).collect();
        let set: HashSet<Key> = again.iter().copied().collect();
        // Bounds the work whatever Maximum Iterations says.
        let max_passes = u32::try_from(MAX_ITERATION_WORK / again.len().max(1)).unwrap_or(u32::MAX).max(1);
        let mut passes = 1u32;
        while passes < max_iterations.min(max_passes) && !self.converged(max_change) {
            self.prev = again.iter().filter_map(|k| self.results.remove(k).map(|v| (*k, v))).collect();
            self.pending = set.clone();
            let gone: Vec<Key> = self.spills.keys().filter(|a| set.contains(a)).copied().collect();
            for a in gone {
                self.spills.remove(&a);
                self.spill_index.remove(a);
            }
            for k in &again {
                self.calculate(*k);
            }
            passes += 1;
        }
        passes
    }

    /// Whether every formula on a loop changed by less than `max_change` in the last pass (text,
    /// logical and error values: not at all).
    fn converged(&self, max_change: f64) -> bool {
        self.members.iter().all(|k| {
            let Some(new) = self.results.get(k) else { return true };
            match (new, &self.previous(*k)) {
                (Value::Number(a), Value::Number(b)) => {
                    let d = (a - b).abs();
                    d < max_change || d == 0.0
                }
                (new, old) => new == old,
            }
        })
    }
}

/// A legacy (Ctrl+Shift+Enter) array formula's result fitted to the range it was entered in, as
/// Excel does: a single row or column repeats across the range, a single value fills it, cells
/// past the result show `#N/A`, and a result larger than the range is cut off.
fn fit_to_range(v: Value, range: RangeRef) -> Value {
    if range.count() > crate::eval::MAX_CELLS {
        return Value::Error(CellError::Num);
    }
    let (h, w) = (range.height() as usize, range.width() as usize);
    let Value::Array(a) = v else {
        return if h * w == 1 { v } else { Array::new(h, w, vec![v; h * w]).map(Value::from).unwrap_or(Value::Error(CellError::Value)) };
    };
    if h * w == 1 {
        return a.data.first().cloned().unwrap_or_default();
    }
    let mut data = Vec::with_capacity(h * w);
    for r in 0..h {
        for c in 0..w {
            let (r, c) = (if a.rows == 1 { 0 } else { r }, if a.cols == 1 { 0 } else { c });
            data.push(a.get(r, c).cloned().unwrap_or(Value::Error(CellError::NA)));
        }
    }
    Array::new(h, w, data).map(Value::from).unwrap_or(Value::Error(CellError::Value))
}

impl Host for PassHost<'_> {
    fn workbook(&self) -> &Workbook {
        self.wb
    }
    fn stack_top(&self) -> Option<usize> {
        (self.depth > 1).then_some(self.stack_top)
    }
    fn stack_exhausted(&mut self) {
        self.out_of_stack = true;
    }
    fn spill_range(&mut self, sheet: usize, anchor: CellRef) -> Option<RangeRef> {
        let k = (sheet, anchor);
        if !(self.pending.contains(&k) || self.results.contains_key(&k)) {
            return self.wb.sheet(sheet)?.spill_ranges.get(&anchor).copied();
        }
        // Recalculated in this pass: its new array, unless something blocks it (#SPILL!).
        self.compute(k);
        let arr = self.spills.get(&k)?;
        let end = CellRef::new(
            anchor.row.saturating_add((arr.rows as u32).saturating_sub(1)),
            anchor.col.saturating_add((arr.cols as u32).saturating_sub(1)),
        );
        let range = RangeRef::new(anchor, end);
        let sh = self.wb.sheet(sheet)?;
        let blocked = end.row >= gridcraft_core::MAX_ROWS
            || end.col >= gridcraft_core::MAX_COLS
            || sh.cells.iter_range(range).any(|(c, cell)| c != anchor && (cell.formula.is_some() || !cell.value.is_empty()))
            || sh.merges.iter().any(|m| m.intersects(&range));
        (!blocked).then_some(range)
    }
    fn cell_value(&mut self, sheet: usize, c: CellRef) -> Value {
        let k = (sheet, c);
        if self.pending.contains(&k) || self.results.contains_key(&k) {
            return self.compute(k);
        }
        // A cell covered by a spill computed in this pass. Where several overlap, the first in
        // `spills`' iteration order counts, as it always has.
        let covering = match self.spill_index.covering(sheet, c) {
            Cover::None => None,
            Cover::One(a) => self.spills.get_key_value(&(sheet, a)),
            Cover::Many => self.spills.iter().find(|(a, arr)| a.0 == sheet && spill_covers(a.1, arr, c)),
        };
        if let Some((anchor, arr)) = covering {
            let (dr, dc) = ((c.row - anchor.1.row) as usize, (c.col - anchor.1.col) as usize);
            if self.iterate && self.tainted.contains(anchor) {
                self.taint = true;
            }
            return arr.get(dr, dc).cloned().unwrap_or_default();
        }
        self.wb.sheet(sheet).map(|s| s.value(c)).unwrap_or_default()
    }
    fn range_values(&mut self, sheet: usize, range: RangeRef) -> Vec<Value> {
        let (h, w) = (range.height() as usize, range.width() as usize);
        let mut out = vec![Value::Empty; h * w];
        let Some(sh) = self.wb.sheet(sheet) else { return out };
        // Sparse walk: only stored cells; formula and Python cells that are (or were) dirty go
        // through `compute`, constants are copied straight from the store.
        let mut formulas: Vec<(usize, CellRef)> = Vec::new();
        for (c, cell) in sh.cells.iter_range(range) {
            let i = (c.row - range.start.row) as usize * w + (c.col - range.start.col) as usize;
            if cell.formula.is_some() || cell.py.is_some() {
                formulas.push((i, c));
            } else if let Some(slot) = out.get_mut(i) {
                *slot = cell.value.clone();
            }
        }
        let _seen = stored_spill_in(sh, range, &mut |c, v| {
            let i = (c.row - range.start.row) as usize * w + (c.col - range.start.col) as usize;
            if let Some(slot) = out.get_mut(i)
                && slot.is_empty()
            {
                *slot = v.clone();
            }
            true
        });
        #[cfg(test)]
        {
            self.spill_visits += _seen;
        }
        for (i, c) in formulas {
            let v = self.cell_value(sheet, c);
            if let Some(slot) = out.get_mut(i) {
                *slot = v;
            }
        }
        if !self.spills.is_empty() {
            let mut hits = Vec::new();
            self.spill_index.meeting(sheet, range, &mut hits);
            // Iterating: an array spilled by a formula that read a previous value.
            if self.iterate && hits.iter().any(|a| self.tainted.contains(&(sheet, *a))) {
                self.taint = true;
            }
            // Where areas overlap inside the range, the last in `spills`' iteration order wins,
            // as it always has; otherwise the order doesn't matter.
            let ordered = hits.len() > MAX_UNORDERED_HITS || spill_hits_overlap(&self.spill_index, sheet, &hits, range);
            let mut put = |anchor: CellRef, arr: &Array| {
                if arr.rows == 0 || arr.cols == 0 {
                    return;
                }
                let (r0, c0) = (range.start.row.max(anchor.row), range.start.col.max(anchor.col));
                let r1 = range.end.row.min(anchor.row.saturating_add((arr.rows as u32).saturating_sub(1)));
                let c1 = range.end.col.min(anchor.col.saturating_add((arr.cols as u32).saturating_sub(1)));
                for r in r0..=r1 {
                    for cc in c0..=c1 {
                        if (r, cc) != (anchor.row, anchor.col)
                            && let Some(slot) = out.get_mut((r - range.start.row) as usize * w + (cc - range.start.col) as usize)
                        {
                            *slot = arr.get((r - anchor.row) as usize, (cc - anchor.col) as usize).cloned().unwrap_or_default();
                        }
                    }
                }
            };
            if ordered {
                let wanted: HashSet<Key> = hits.iter().map(|a| (sheet, *a)).collect();
                for (a, arr) in &self.spills {
                    if wanted.contains(a) {
                        put(a.1, arr);
                    }
                }
            } else {
                for a in &hits {
                    if let Some(arr) = self.spills.get(&(sheet, *a)) {
                        put(*a, arr);
                    }
                }
            }
        }
        out
    }
    fn visit_range(&mut self, sheet: usize, range: RangeRef, f: &mut dyn FnMut(&Value) -> bool) -> bool {
        let wb = self.wb;
        let Some(sh) = wb.sheet(sheet) else { return false };
        // Formula and Python cells first, in row-major order like `range_values`: reading one
        // calculates it (a Python cell lays out its result) and the pass remembers the value, so
        // the walk below gets the same value again without this method holding any.
        for (c, cell) in sh.cells.iter_range(range) {
            if cell.formula.is_some() || cell.py.is_some() {
                let _ = self.cell_value(sheet, c);
            }
        }
        // Spilled values sit between the stored cells (and the formulas just calculated may have
        // produced some): leave such blocks to `range_values`.
        let mut arrays = Vec::new();
        self.spill_index.meeting(sheet, range, &mut arrays);
        let mut stored = false;
        let _seen = stored_spill_in(sh, range, &mut |_, _| {
            stored = true;
            false
        });
        #[cfg(test)]
        {
            self.spill_visits += _seen;
        }
        if !arrays.is_empty() || stored {
            return false;
        }
        for (c, cell) in sh.cells.iter_range(range) {
            let go = if cell.formula.is_some() || cell.py.is_some() { f(&self.cell_value(sheet, c)) } else { f(&cell.value) };
            if !go {
                break;
            }
        }
        true
    }
    fn range_sum(&mut self, sheet: usize, range: RangeRef) -> Option<(f64, u64)> {
        // Constants can't change during a pass, so a block's result stays valid for it. Spills
        // found in this pass (and iteration, which tracks what a read depends on) go the long way.
        if self.iterate || self.spills.keys().any(|a| a.0 == sheet) {
            return None;
        }
        let sh = self.wb.sheet(sheet)?;
        let (key, last) = ((sheet, range.start, range.end.col), range.end.row);
        let known = self.sums.entry(key).or_default();
        if let Some(r) = known.get(&last) {
            return *r;
        }
        // Start from the longest shorter block of the same rows from the top: summing row-major
        // from there on is the same additions in the same order as one walk from the start.
        let (from, mut sum, mut count) = match known.range(..last).next_back() {
            Some((_, None)) => {
                known.insert(last, None);
                return None;
            }
            Some((&row, &Some((sum, count)))) => (row + 1, sum, count),
            None => (range.start.row, 0.0, 0),
        };
        let rest = RangeRef::new(CellRef::new(from, range.start.col), range.end);
        let mut plain = sh.spill.range(rest.start..=rest.end).all(|(c, _)| !rest.contains(*c));
        if plain {
            for (_, cell) in sh.cells.iter_range(rest) {
                match &cell.value {
                    _ if cell.formula.is_some() || cell.py.is_some() => plain = false,
                    Value::Error(_) => plain = false,
                    Value::Number(n) => {
                        sum += n;
                        count = u64::saturating_add(count, 1);
                    }
                    _ => {}
                }
                if !plain {
                    break;
                }
            }
        }
        let result = plain.then_some((sum, count));
        known.insert(last, result);
        result
    }
    fn used_range(&mut self, sheet: usize) -> Option<RangeRef> {
        let wb = self.wb;
        *self.used.entry(sheet).or_insert_with(|| wb.sheet(sheet).and_then(|s| s.used_range()))
    }
    fn now_serial(&self) -> f64 {
        self.now
    }
    fn random(&mut self) -> f64 {
        // xorshift64*
        let mut x = *self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}
