//! Dependency graph and recalculation.
//!
//! The graph maps every formula cell to its static precedents (resolved areas). A reverse index
//! answers "which formulas read this cell?": single-cell precedents in a hash map, ranges as row
//! intervals per column (wide ranges per sheet), each an interval index so a lookup costs
//! O(log n + matches) however many ranges share the column. After an edit, the dirty set is the
//! transitive closure of dependents; it is evaluated in topological order, with on-demand
//! evaluation as a safety net for dynamic references. Cycles produce `#CIRC!` unless iterative
//! calculation is on: then a formula read while it is being evaluated gives its previous value,
//! and the formulas that read one are evaluated again until the loop settles
//! (`PassHost::iterate_loops`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use gridcraft_core::{Array, CellError, CellRef, RangeRef, Value};
use gridcraft_formula::Expr;
use gridcraft_model::{CalcMode, Workbook};

use crate::eval::{Evaluator, Host, precedents, stack_position};
use crate::spill_index::{SpillIndex, indexing_pays};

/// Changed cells (in one `cells_changed`) from which a sheet's spill areas may be indexed.
const INDEX_SPILLS_FROM: usize = 16;

/// Whether a sheet's spill areas are indexed for a batch of `batch` changed cells of which
/// `on_sheet` are on that sheet: the batch must be a paste, and registering the areas must cost
/// less than walking them once for each of the sheet's own changed cells.
fn index_spills<'a>(batch: usize, on_sheet: usize, areas: impl ExactSizeIterator<Item = &'a RangeRef>) -> bool {
    batch > INDEX_SPILLS_FROM && areas.len() > 0 && indexing_pays(areas, on_sheet)
}

mod graph;
mod pass_host;

#[cfg(test)]
use graph::Intervals;
pub use graph::{FxHasher, Graph, Key};
use graph::{HashMap, HashSet, Node, Taken};
use pass_host::{PassHost, spills_as_before};

/// Calculation state kept beside a workbook.
#[derive(Clone, Debug)]
pub struct Calc {
    pub graph: Graph,
    rng: u64,
    /// Fixed "now" for deterministic tests; `None` = system clock.
    pub fixed_now: Option<f64>,
    /// Cells whose formulas have errors from cycles.
    pub circular: Vec<Key>,
    pub last_recalc_cells: usize,
    /// Which anchors held a spill when the current calculation started (see [`Standing`]).
    standing: Standing,
    /// The cells whose value is `#SPILL!`, kept so that finding the formulas an emptied cell or a
    /// freed spill area may unblock doesn't walk every formula or window of cells. Filled in
    /// `rebuild`, kept up to date by `cells_changed` and by the write-back of a run; an entry is
    /// always checked against the workbook before it is used.
    spill_errors: SpillErrors,
}

/// Who gets the cells when two arrays want the same ones. Excel (measured by a reviewer on
/// PR #2483) lets the spill that already exists keep its cells, whichever anchor comes first;
/// the other formula shows `#SPILL!`, and clearing the winner lets it spill. The rule here, for
/// each cell both want:
///
/// 1. the anchor whose spill held that cell before this calculation keeps it, so an array that
///    grows into another's area is `#SPILL!` even when it held a spill of its own;
/// 2. otherwise an anchor that held a spill before this calculation outranks one that did not;
/// 3. otherwise the anchor first in reading order (row, then column) wins.
///
/// Rule 3 alone decides when nothing is remembered, so the result never depends on hash order
/// or evaluation order. A full recalculation remembers the layout the sheet held before it
/// started, so it agrees with the incremental one; a workbook that is calculated for the first
/// time has no such layout (rule 3 only). The xlsx reader restores the area each dynamic array
/// was saved spilling over, so a workbook opened from a file keeps the layout it was saved with.
#[derive(Clone, Debug, Default)]
struct Standing {
    /// `true`: `keys` are the anchors that held a spill (full recalculation, which lays every
    /// spill out afresh). `false`: `keys` are the formulas that held none (incremental).
    held: bool,
    keys: HashSet<Key>,
    /// The area each anchor that held a spill filled before this calculation, for the anchors the
    /// calculation lays out again (full: all of them; incremental: the formulas it recalculates).
    /// Any other anchor still fills the area the sheet shows.
    areas: HashMap<Key, RangeRef>,
}

impl Standing {
    fn had_spill(&self, k: Key) -> bool {
        self.keys.contains(&k) == self.held
    }

    /// Whether `a` takes cells from `b` when both want them and neither held them (rules 2, 3).
    fn outranks(&self, a: Key, b: Key) -> bool {
        match (self.had_spill(a), self.had_spill(b)) {
            (true, false) => true,
            (false, true) => false,
            _ => (a.1.row, a.1.col) < (b.1.row, b.1.col),
        }
    }

    /// Whether `k`'s spill held `cell` before this calculation; `now` is the area the sheet
    /// shows for `k`.
    fn held_cell(&self, k: Key, now: Option<RangeRef>, cell: CellRef) -> bool {
        self.had_spill(k) && self.areas.get(&k).copied().or(now).is_some_and(|r| r.contains(cell))
    }

    /// Whether `a` takes `cell` from `b` when both want it (rule 1, then [`Self::outranks`]). Each
    /// anchor comes with the area the sheet shows for it.
    fn outranks_at(&self, (a, a_now): (Key, Option<RangeRef>), (b, b_now): (Key, Option<RangeRef>), cell: CellRef) -> bool {
        match (self.held_cell(a, a_now, cell), self.held_cell(b, b_now, cell)) {
            (true, false) => true,
            (false, true) => false,
            _ => self.outranks(a, b),
        }
    }
}

/// A set of cells by row, each row's columns in their own set: a window of rows and columns is
/// found by one column range per row that has an entry, so the cost follows the cells in the
/// window, not those to the side of it in the same rows.
#[derive(Clone, Debug, Default)]
struct SpillErrors {
    rows: BTreeMap<(usize, u32), BTreeSet<u32>>,
}

impl SpillErrors {
    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn clear(&mut self) {
        self.rows.clear();
    }

    fn insert(&mut self, (sheet, c): Key) {
        self.rows.entry((sheet, c.row)).or_default().insert(c.col);
    }

    fn remove(&mut self, (sheet, c): &Key) {
        if let Some(cols) = self.rows.get_mut(&(*sheet, c.row)) {
            cols.remove(&c.col);
            if cols.is_empty() {
                self.rows.remove(&(*sheet, c.row));
            }
        }
    }

    /// The cells of `sheet` in `rows` x `cols` (both inclusive), in row-major order.
    fn range(&self, sheet: usize, rows: (u32, u32), cols: (u32, u32)) -> impl Iterator<Item = CellRef> + '_ {
        self.rows
            .range((sheet, rows.0)..=(sheet, rows.1))
            .flat_map(move |(&(_, row), set)| set.range(cols.0..=cols.1).map(move |&col| CellRef::new(row, col)))
    }
}

impl Default for Calc {
    fn default() -> Self {
        Calc {
            graph: Graph::default(),
            rng: 0x9E37_79B9_7F4A_7C15,
            fixed_now: None,
            circular: vec![],
            last_recalc_cells: 0,
            standing: Standing::default(),
            spill_errors: SpillErrors::default(),
        }
    }
}

/// Pops one finished component off Tarjan's stack, labelling its nodes with `comp_index`. The
/// component is everything up to and including `root`.
fn pop_component(stack: &mut Vec<usize>, on_stack: &mut [bool], comp: &mut [usize], comp_index: usize, root: usize) {
    while let Some(w) = stack.pop() {
        on_stack[w] = false;
        comp[w] = comp_index;
        if w == root {
            break;
        }
    }
}

/// Strongly connected components of a graph given as adjacency lists (Tarjan, iterative): each
/// node's component index, and the number of nodes in each component.
fn components(edges: &[Vec<usize>]) -> (Vec<usize>, Vec<usize>) {
    let n = edges.len();
    let (mut low, mut num) = (vec![0usize; n], vec![usize::MAX; n]);
    let (mut on_stack, mut stack, mut comp) = (vec![false; n], Vec::new(), vec![usize::MAX; n]);
    let (mut next, mut comps) = (0usize, 0usize);
    for root in 0..n {
        if num[root] != usize::MAX {
            continue;
        }
        let mut call: Vec<(usize, usize)> = vec![(root, 0)];
        num[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(top) = call.last_mut() {
            let v = top.0;
            if let Some(&w) = edges.get(v).and_then(|e| e.get(top.1)) {
                top.1 += 1;
                if num[w] == usize::MAX {
                    num[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(num[w]);
                }
                continue;
            }
            call.pop();
            if let Some(&(u, _)) = call.last() {
                low[u] = low[u].min(low[v]);
            }
            if low[v] == num[v] {
                pop_component(&mut stack, &mut on_stack, &mut comp, comps, v);
                comps += 1;
            }
        }
    }
    let mut size = vec![0usize; comps];
    for &c in &comp {
        if let Some(s) = size.get_mut(c) {
            *s += 1;
        }
    }
    (comp, size)
}

impl Calc {
    pub fn new() -> Self {
        Self::default()
    }

    fn now(&self) -> f64 {
        if let Some(n) = self.fixed_now {
            return n;
        }
        now_serial()
    }

    /// Rebuilds the graph from scratch.
    pub fn rebuild(&mut self, wb: &Workbook) {
        self.graph = Graph::default();
        let mut circ = Vec::new();
        self.spill_errors.clear();
        for (si, sheet) in wb.sheets.iter().enumerate() {
            for (c, cell) in sheet.cells.iter() {
                if cell.value == Value::Error(CellError::Spill) {
                    self.spill_errors.insert((si, c));
                }
                if let Some(f) = &cell.formula
                    && let Some((areas, dynamic)) = f.with_expr(|e| precedents(wb, si, e))
                {
                    self.graph.insert((si, c), Node { areas, dynamic });
                    if cell.value == Value::Error(CellError::Circ) {
                        circ.push((si, c));
                    }
                } else if cell.py.is_some() {
                    // A Python cell lays out its kept result, so it is calculated like a formula
                    // that reads nothing: its spill is blocked and unblocked like a formula's.
                    self.graph.insert((si, c), Node { areas: Vec::new(), dynamic: false });
                }
            }
        }
        self.graph.settle();
        self.circular = if wb.calc.iterative { Vec::new() } else { self.loops_among(&circ) };
    }

    /// The circular references of a rebuilt workbook (after undo, redo or a rollback, which put
    /// back cells without calculating them), from the formulas whose cached value is `#CIRC!`:
    /// the listed cells still on a loop, and one cell of each loop none of them is on.
    fn loops_among(&self, circ: &[Key]) -> Vec<Key> {
        if circ.is_empty() {
            return Vec::new();
        }
        let n = circ.len();
        let index: HashMap<Key, usize> = circ.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        // Edges from a cell to the `#CIRC!` formulas that read it; loops are the same either way.
        let mut deps = Vec::new();
        let edges: Vec<Vec<usize>> = circ
            .iter()
            .map(|k| {
                deps.clear();
                self.graph.dependents(k.0, k.1, &mut deps);
                let mut e: Vec<usize> = deps.iter().filter_map(|d| index.get(d).copied()).collect();
                e.sort_unstable();
                e.dedup();
                e
            })
            .collect();
        // Loop members: nodes in a component of more than one, or with an edge to themselves.
        let (comp, size) = components(&edges);
        let on_loop = |i: usize| comp.get(i).and_then(|c| size.get(*c)).is_some_and(|s| *s > 1) || edges.get(i).is_some_and(|e| e.contains(&i));
        // A formula with dynamic references (INDIRECT, OFFSET) may close a loop the graph can't see.
        let mut out: Vec<Key> =
            self.circular.iter().copied().filter(|k| index.get(k).is_some_and(|&i| on_loop(i) || self.graph.dynamic.contains(k))).collect();
        let mut covered: HashSet<usize> = out.iter().filter_map(|k| index.get(k)).filter_map(|&i| comp.get(i).copied()).collect();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| circ.get(i).map(|(s, c)| (*s, c.row, c.col)));
        for i in order {
            if let (Some(&c), Some(&k)) = (comp.get(i), circ.get(i))
                && on_loop(i)
                && covered.insert(c)
            {
                out.push(k);
            }
        }
        out
    }

    /// The cells of `sheet` in `rows` x `cols` (inclusive, `cols` a lower and an upper column) that
    /// show `#SPILL!`, in row-major order.
    fn spill_errors_in(&self, wb: &Workbook, sheet: usize, rows: (u32, u32), cols: (u32, u32)) -> Vec<CellRef> {
        let Some(sh) = wb.sheet(sheet) else { return Vec::new() };
        self.spill_errors.range(sheet, rows, cols).filter(|c| sh.cells.get(*c).is_some_and(|x| x.value == Value::Error(CellError::Spill))).collect()
    }

    /// The formulas showing `#SPILL!` on `sheet` at or above and to the left of `end`: what a
    /// cell emptied or a spill area freed there may unblock.
    fn unblock_candidates(&self, wb: &Workbook, sheet: usize, end: CellRef) -> Vec<Key> {
        let cells = self.spill_errors_in(wb, sheet, (0, end.row), (0, end.col));
        cells.into_iter().filter(|c| self.graph.nodes.contains_key(&(sheet, *c))).map(|c| (sheet, c)).collect()
    }

    /// Recalculates every formula (F9 / Ctrl+Alt+F9, and after loading).
    pub fn recalc_all(&mut self, wb: &mut Workbook) {
        self.rebuild(wb);
        // The layout the sheets hold now decides who keeps cells two arrays both want.
        // (A Python cell that holds a spill counts too, as it does in the incremental pass.)
        let mut areas: HashMap<Key, RangeRef> = HashMap::default();
        for i in 0..wb.sheets.len() {
            if let Some(sh) = wb.sheet(i) {
                areas.extend(
                    sh.spill_ranges
                        .iter()
                        .filter(|(a, _)| sh.cell(**a).is_some_and(|c| c.formula.is_some() || c.py.is_some()))
                        .map(|(a, r)| ((i, *a), *r)),
                );
            }
        }
        let held = areas.keys().copied().collect();
        // Spills are laid out again from the formulas where they are now (a structural edit moves
        // or deletes the formulas, not the values they spilled).
        for i in 0..wb.sheets.len() {
            if wb.sheet(i).is_some_and(|s| !s.spill_ranges.is_empty() || !s.spill.is_empty())
                && let Some(sh) = wb.sheet_mut(i)
            {
                sh.spill.clear();
                sh.spill_ranges.clear();
            }
        }
        let all: Vec<Key> = self.graph.nodes.keys().copied().collect();
        self.standing = Standing { held: true, keys: held, areas };
        self.run(wb, all, true);
        self.standing = Standing::default();
    }

    /// Updates the graph for cells whose content changed and recalculates what depends on them.
    pub fn cells_changed(&mut self, wb: &mut Workbook, changed: &[Key]) {
        let mut cleared: Vec<(usize, RangeRef)> = Vec::new();
        for &k in changed {
            if wb.sheet(k.0).and_then(|s| s.cell(k.1)).is_some_and(|c| c.value == Value::Error(CellError::Spill)) {
                self.spill_errors.insert(k);
            } else {
                self.spill_errors.remove(&k);
            }
            let precedent =
                wb.sheet(k.0).and_then(|s| s.cell(k.1)).and_then(|c| c.formula.as_ref()).and_then(|f| f.with_expr(|e| precedents(wb, k.0, e)));
            let py = wb.sheet(k.0).and_then(|s| s.cell(k.1)).and_then(|c| c.py.as_ref());
            let python = py.is_some();
            // A Python cell whose result (if any) is not an array spills nothing.
            let spills = py.and_then(|p| p.result_value()).is_some_and(|v| matches!(v, Value::Array(_)));
            match precedent {
                Some((areas, dynamic)) => {
                    self.graph.insert(k, Node { areas, dynamic });
                }
                // See `rebuild`: its spill is laid out again from its result when it is calculated.
                None if python => {
                    self.graph.insert(k, Node { areas: Vec::new(), dynamic: false });
                    // New code has no result yet: the spill the cell had (its old result's, or a
                    // formula's) goes, and what read it is recalculated, as for a formula
                    // replaced by a constant.
                    if !spills
                        && let Some(sh) = wb.sheet_mut(k.0)
                        && let Some(r) = sh.spill_ranges.remove(&k.1)
                    {
                        sh.clear_spill(r);
                        cleared.push((k.0, r));
                    }
                }
                None => {
                    self.graph.remove(k);
                    // A formula replaced by a constant (or cleared) takes its spilled values with it.
                    if let Some(sh) = wb.sheet_mut(k.0)
                        && let Some(r) = sh.spill_ranges.remove(&k.1)
                    {
                        for c in r.iter() {
                            sh.spill.remove(&c);
                        }
                        cleared.push((k.0, r));
                    }
                    // A formula that doesn't parse evaluates to #NAME?.
                    if let Some(cell) = wb.sheet_mut(k.0).and_then(|s| s.cells.get_mut(k.1))
                        && cell.formula.is_some()
                    {
                        cell.value = Value::Error(CellError::Name);
                        self.spill_errors.remove(&k);
                    }
                }
            }
        }
        self.graph.settle();
        if wb.calc.mode == CalcMode::Manual {
            // Only the edited formulas themselves are evaluated.
            let own: Vec<Key> = changed.iter().copied().filter(|k| self.graph.nodes.contains_key(k)).collect();
            self.run(wb, own, false);
            return;
        }
        let mut seeds: Vec<Key> = changed.to_vec();
        let mut emptied: Vec<Key> = Vec::new();
        // Typing into (or clearing) a cell a dynamic array spills over (or would spill over)
        // re-evaluates the anchor, which may now be blocked or unblocked.
        // With many changed cells (a paste) a sheet's spill areas are indexed once instead of being
        // walked for each one, when registering them costs less than walking them would.
        let mut areas = SpillIndex::default();
        let mut strategy: HashMap<usize, bool> = HashMap::default();
        let mut per_sheet: HashMap<usize, usize> = HashMap::default();
        for k in changed {
            *per_sheet.entry(k.0).or_insert(0) += 1;
        }
        let mut at = Vec::new();
        for &k in changed {
            if let Some(sh) = wb.sheet(k.0) {
                let indexed = *strategy.entry(k.0).or_insert_with(|| {
                    let indexed = index_spills(changed.len(), per_sheet[&k.0], sh.spill_ranges.values());
                    if indexed {
                        for (anchor, r) in &sh.spill_ranges {
                            areas.insert_rect((k.0, *anchor), *r, false);
                        }
                    }
                    indexed
                });
                if indexed {
                    // Anchor order, as walking `spill_ranges` gives.
                    areas.meeting(k.0, RangeRef::new(k.1, k.1), &mut at);
                    seeds.extend(at.iter().filter(|a| **a != k.1).map(|a| (k.0, *a)));
                } else {
                    for (anchor, r) in &sh.spill_ranges {
                        if *anchor != k.1 && r.contains(k.1) {
                            seeds.push((k.0, *anchor));
                        }
                    }
                }
                if !self.spill_errors.is_empty() {
                    let (r0, c0) = (k.1.row.saturating_sub(64), k.1.col.saturating_sub(64));
                    seeds.extend(self.spill_errors_in(wb, k.0, (r0, k.1.row), (c0, k.1.col)).into_iter().filter(|c| *c != k.1).map(|c| (k.0, c)));
                }
                if !sh.cells.has(k.1) {
                    match emptied.iter_mut().find(|x| x.0 == k.0) {
                        Some(x) => x.1 = CellRef::new(x.1.row.max(k.1.row), x.1.col.max(k.1.col)),
                        None => emptied.push(k),
                    }
                }
            }
        }
        // So can the area a removed formula spilled over.
        for (si, r) in &cleared {
            match emptied.iter_mut().find(|x| x.0 == *si) {
                Some(x) => x.1 = CellRef::new(x.1.row.max(r.end.row), x.1.col.max(r.end.col)),
                None => emptied.push((*si, r.end)),
            }
        }
        // An emptied cell can unblock an anchor any distance above or to the left of it.
        for (si, end) in emptied {
            seeds.extend(self.unblock_candidates(wb, si, end));
        }
        // Spill areas of changed anchors also change. They stay rectangles: one seed per spilled
        // cell listed the formulas reading the area once per cell.
        let mut areas: Vec<(usize, RangeRef)> = cleared;
        for &k in changed {
            if let Some(r) = wb.sheet(k.0).and_then(|s| s.spill_ranges.get(&k.1)) {
                areas.push((k.0, *r));
            }
        }
        let t0 = prof_now();
        let dirty = self.dirty_closure(wb, &seeds, &areas);
        let t1 = prof_now();
        let n = dirty.len();
        self.run(wb, dirty, false);
        if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
            eprintln!("recalc: {} seeds, closure {} cells {:.1} ms, run {:.1} ms", seeds.len(), n, t1 - t0, prof_now() - t1);
        }
    }

    /// The formulas to recalculate after `seeds` and every cell of `areas` changed: the seeds
    /// that are formulas, and everything that reads them, directly or through other formulas.
    fn dirty_closure(&self, wb: &Workbook, seeds: &[Key], areas: &[(usize, RangeRef)]) -> Vec<Key> {
        let mut seen: HashSet<Key> = HashSet::default();
        let mut queue: VecDeque<Key> = VecDeque::new();
        // Each range-index entry is listed once in the whole walk: many changed cells under one
        // broad reference (or many formulas inside one) would otherwise list every formula reading
        // it once per cell, which squared the work.
        let mut taken = Taken::default();
        let mut deps = Vec::new();
        for k in seeds {
            if self.graph.nodes.contains_key(k) && seen.insert(*k) {
                queue.push_back(*k);
            }
            deps.clear();
            self.graph.dependents_untaken(k.0, k.1, &mut taken, &mut deps);
            for d in deps.drain(..) {
                if seen.insert(d) {
                    queue.push_back(d);
                }
            }
        }
        for &(si, r) in areas {
            if let Some(sh) = wb.sheet(si) {
                for (c, _) in sh.cells.iter_range(r) {
                    if self.graph.nodes.contains_key(&(si, c)) && seen.insert((si, c)) {
                        queue.push_back((si, c));
                    }
                }
            }
            deps.clear();
            self.graph.dependents_of_range(si, r, &mut deps);
            for d in deps.drain(..) {
                if seen.insert(d) {
                    queue.push_back(d);
                }
            }
        }
        for k in &self.graph.dynamic {
            if seen.insert(*k) {
                queue.push_back(*k);
            }
        }
        while let Some(k) = queue.pop_front() {
            deps.clear();
            self.graph.dependents_untaken(k.0, k.1, &mut taken, &mut deps);
            for d in deps.drain(..) {
                if seen.insert(d) {
                    queue.push_back(d);
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Evaluates the dirty set and writes results (and spills) back.
    fn run(&mut self, wb: &mut Workbook, mut dirty: Vec<Key>, full: bool) {
        let mut dynamic_again = !self.graph.dynamic.is_empty();
        self.last_recalc_cells = 0;
        let mut seen: HashSet<Key> = HashSet::default();
        if !full {
            self.standing = Standing::default();
        }
        for _round in 0..8 {
            if dirty.is_empty() {
                break;
            }
            if !full {
                // What each formula held before this calculation touched it.
                for &k in &dirty {
                    if seen.insert(k) {
                        match wb.sheet(k.0).and_then(|s| s.spill_ranges.get(&k.1)) {
                            Some(r) => {
                                self.standing.areas.insert(k, *r);
                            }
                            None => {
                                self.standing.keys.insert(k);
                            }
                        }
                    }
                }
            }
            // Row-major order is a good topological guess; on-demand evaluation fixes the rest.
            dirty.sort_by_key(|(s, c)| (*s, c.row, c.col));
            let mut exprs: HashMap<Key, std::sync::Arc<Expr>> = HashMap::with_capacity_and_hasher(dirty.len(), Default::default());
            for k in &dirty {
                if let Some(e) = wb.sheet(k.0).and_then(|s| s.cell(k.1)).and_then(|c| c.formula.as_ref()).and_then(|f| f.expr_arc()) {
                    exprs.insert(*k, e);
                }
            }
            let python: HashMap<Key, Value> = dirty
                .iter()
                .filter_map(|k| wb.sheet(k.0).and_then(|s| s.cell(k.1)).and_then(|c| c.py.as_ref()).and_then(|p| p.result_value()).map(|v| (*k, v)))
                .collect();
            let now = self.now();
            let mut rng = self.rng;
            let tp = prof_now();
            let (results, spills, cycles) = {
                let mut host = PassHost::new(wb, &exprs, &mut rng, now).with_python(python);
                host.standing = Some(&self.standing);
                host.iterate = wb.calc.iterative;
                for k in &dirty {
                    host.calculate(*k);
                }
                if host.iterate && !host.members.is_empty() {
                    let passes = host.iterate_loops(&dirty, wb.calc.max_iterations, wb.calc.max_change);
                    if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
                        eprintln!("  iterative calculation: {} passes over {} formulas", passes, host.tainted.len());
                    }
                }
                (host.results, host.spills, host.cycles)
            };
            self.rng = rng;
            if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
                eprintln!("  eval {} formulas {:.1} ms", results.len(), prof_now() - tp);
            }
            let tw = prof_now();
            self.last_recalc_cells += results.len();
            // The loops this round finds replace the ones listed for the formulas it evaluated.
            if !self.circular.is_empty() {
                let evaluated: HashSet<Key> = dirty.iter().copied().collect();
                self.circular.retain(|k| !evaluated.contains(k));
            }
            self.circular.extend(cycles);
            let mut spill_changes = self.write_back(wb, &dirty, results, &spills);
            // Formulas with references only known while evaluating (INDIRECT, OFFSET) may have read
            // a spill area before it was laid out: they are evaluated once more.
            if dynamic_again && !spills.is_empty() {
                dynamic_again = false;
                spill_changes.extend(self.graph.dynamic.iter().copied());
            }
            if full {
                // Everything was already evaluated once; spill dependents need another pass.
            }
            if std::env::var_os("GRIDCRAFT_PROFILE").is_some() {
                eprintln!("  write-back {:.1} ms", prof_now() - tw);
            }
            spill_changes.sort_by_key(|(s, c)| (*s, c.row, c.col));
            spill_changes.dedup();
            if spill_changes.is_empty() {
                break;
            }
            dirty = self.dirty_closure(wb, &spill_changes, &[]);
        }
        // Loops among formulas the run did not evaluate stay listed while they are still formulas.
        // Iterative calculation reports none.
        if wb.calc.iterative {
            self.circular.clear();
        } else {
            self.circular.retain(|k| self.graph.nodes.contains_key(k));
        }
    }

    fn write_back(
        &mut self,
        wb: &mut Workbook,
        dirty: &[Key],
        results: HashMap<Key, Value>,
        spills: &HashMap<Key, std::sync::Arc<Array>>,
    ) -> Vec<Key> {
        let mut spill_changes = Vec::new();
        let mut freed = Vec::new();
        for (key, value) in results {
            let Some(sheet) = wb.sheet_mut(key.0) else { continue };
            if let Some(cell) = sheet.cells.get_mut(key.1) {
                if value == Value::Error(CellError::Spill) {
                    self.spill_errors.insert(key);
                } else {
                    self.spill_errors.remove(&key);
                }
                if cell.value != value {
                    cell.value = value;
                }
            }
        }
        for key in dirty {
            let new = spills.get(key);
            let Some(sheet) = wb.sheet_mut(key.0) else { continue };
            let old = sheet.spill_ranges.get(&key.1).copied();
            if old.is_none() && new.is_none() {
                continue;
            }
            // An unchanged spill does not alter any values its dependents read.
            if let (Some(range), Some(array)) = (old, new)
                && spills_as_before(sheet, key.1, range, array)
            {
                continue;
            }
            if let Some(range) = old {
                for cell in range.iter() {
                    sheet.spill.remove(&cell);
                }
                sheet.spill_ranges.remove(&key.1);
            }
            let mut changed_range = old;
            if let Some(array) = new {
                let end = CellRef::new(key.1.row.saturating_add(array.rows as u32 - 1), key.1.col.saturating_add(array.cols as u32 - 1));
                let range = RangeRef::new(key.1, end);
                // Two arrays that want the same cells (see [`Standing`]): a spill already laid out
                // that this anchor outranks on every cell they share is taken over, and recalculated.
                let standing = &self.standing;
                // A spill can only be in the way where it has cells besides its anchor, and an
                // anchor itself blocks the array. When no spilled cell lies in the range, the owners
                // are not scanned: a full recalculation of many disjoint spills stays linear.
                let outranked: Vec<(CellRef, RangeRef)> = if range.iter().any(|cell| cell != key.1 && sheet.spill.contains_key(&cell)) {
                    #[cfg(test)]
                    OWNER_SCANS.with(|n| n.set(n.get() + 1));
                    sheet
                        .spill_ranges
                        .iter()
                        .filter(|(o, r)| {
                            **o != key.1
                                && r.intersection(&range).is_some_and(|shared| {
                                    shared.iter().all(|cell| standing.outranks_at((*key, old), ((key.0, **o), Some(**r)), cell))
                                })
                        })
                        .map(|(o, r)| (*o, *r))
                        .collect()
                } else {
                    Vec::new()
                };
                let blocked = spill_is_blocked(sheet, (*key, old), range, end, standing);
                if !blocked {
                    for (o, r) in outranked {
                        for c in r.iter() {
                            sheet.spill.remove(&c);
                        }
                        sheet.spill_ranges.remove(&o);
                        if let Some(cell) = sheet.cells.get_mut(o) {
                            cell.value = Value::Error(CellError::Spill);
                            self.spill_errors.insert((key.0, o));
                        }
                        spill_changes.push((key.0, o));
                        self.graph.dependents_of_range(key.0, r, &mut spill_changes);
                        freed.push((key.0, r));
                    }
                }
                if blocked {
                    if let Some(cell) = sheet.cells.get_mut(key.1) {
                        cell.value = Value::Error(CellError::Spill);
                        self.spill_errors.insert(*key);
                    }
                    let mut blocked_dependents = Vec::new();
                    self.graph.dependents_of_range(key.0, range, &mut blocked_dependents);
                    spill_changes.extend(blocked_dependents.into_iter().filter(|dependent| dependent != key));
                } else {
                    fill_spill(sheet, key.1, range, array);
                    changed_range = Some(changed_range.map_or(range, |previous| previous.union(&range)));
                }
            }
            if let Some(range) = changed_range {
                let mut dependents = Vec::new();
                self.graph.dependents_of_range(key.0, range, &mut dependents);
                let freed_old_range = match (old, sheet.spill_ranges.get(&key.1)) {
                    (Some(previous), Some(current)) => !(current.contains(previous.start) && current.contains(previous.end)),
                    (_, None) => true,
                    (None, Some(_)) => false,
                };
                spill_changes.extend(dependents.into_iter().filter(|dependent| dependent != key || freed_old_range));
            }
            if let Some(previous) = old
                && !sheet.spill_ranges.get(&key.1).is_some_and(|current| current.contains(previous.start) && current.contains(previous.end))
            {
                freed.push((key.0, previous));
            }
        }
        for (sheet_index, range) in freed {
            spill_changes.extend(self.unblock_candidates(wb, sheet_index, range.end));
        }
        spill_changes
    }
}

// Owner scans done by `write_back`, to check that disjoint spills never walk every owner.
#[cfg(test)]
thread_local! {
    pub(crate) static OWNER_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether the array at `anchor` (with the area the sheet showed for it) can't be laid out over
/// `range`. A spill already laid out that `anchor` outranks does not block it: it is taken over
/// when written back.
fn spill_is_blocked(
    sheet: &gridcraft_model::Sheet,
    (anchor, now): (Key, Option<RangeRef>),
    range: RangeRef,
    end: CellRef,
    standing: &Standing,
) -> bool {
    let outranked = |cell: CellRef| {
        sheet
            .spill_ranges
            .iter()
            .any(|(o, r)| *o != anchor.1 && r.contains(cell) && standing.outranks_at((anchor, now), ((anchor.0, *o), Some(*r)), cell))
    };
    end.row >= gridcraft_core::MAX_ROWS
        || end.col >= gridcraft_core::MAX_COLS
        || range.iter().any(|cell| cell != anchor.1 && (sheet.cells.has(cell) || (sheet.spill.contains_key(&cell) && !outranked(cell))))
        || sheet.merges.iter().any(|merge| merge.intersects(&range))
        || sheet.tables.iter().any(|table| table.range.contains(anchor.1))
}

fn fill_spill(sheet: &mut gridcraft_model::Sheet, anchor: CellRef, range: RangeRef, array: &Array) {
    for (index, cell) in range.iter().enumerate() {
        if cell != anchor {
            sheet.spill.insert(cell, array.data.get(index).cloned().unwrap_or_default());
        }
    }
    sheet.spill_ranges.insert(anchor, range);
}

/// Current date-time as a serial (1900 system), from the system clock (browser clock on wasm).
/// UTC; the local offset isn't available without platform calls. Falls back to a fixed date only
/// when no clock is available.
pub fn now_serial() -> f64 {
    gridcraft_core::date::unix_now_ms().map_or(46302.5, gridcraft_core::date::serial_from_unix_ms)
}

/// Evaluates a formula in the context of a cell without storing it (conditional formats, data
/// validation, Evaluate Formula, Name Manager previews, the CLI `eval`).
pub fn evaluate(wb: &Workbook, sheet: usize, at: CellRef, formula: &str) -> Value {
    let body = formula.strip_prefix('=').unwrap_or(formula);
    let Ok(expr) = gridcraft_formula::parse(body) else { return Value::Error(CellError::Name) };
    evaluate_expr(wb, sheet, at, &expr)
}

pub fn evaluate_expr(wb: &Workbook, sheet: usize, at: CellRef, expr: &Expr) -> Value {
    let exprs = HashMap::default();
    let mut rng = 0x1234_5678_9ABC_DEF0u64;
    let mut host = PassHost::new(wb, &exprs, &mut rng, now_serial());
    let mut ev = Evaluator::new(&mut host, sheet, at);
    ev.value(expr)
}

/// What [`evaluate_bounded`] found: the formula's reference (not read), its value, or that it ran
/// out of work.
#[derive(Clone, Debug)]
pub enum Bounded {
    /// The formula is a reference (`$B$1:$B$9`, a name for one, `OFFSET(…)`): its areas, unread.
    Areas(Vec<crate::eval::Area>),
    Value(Value),
    /// The formula needed more than the work it was given.
    Exhausted,
}

/// Evaluates a formula in the context of a cell with at most `work` units of work (one per
/// expression evaluated, per cell read, …), keeping a reference result as its areas so the caller
/// can read as much of them as it needs (a validation list's search source).
pub fn evaluate_bounded(wb: &Workbook, sheet: usize, at: CellRef, formula: &str, work: u64) -> Bounded {
    let body = formula.strip_prefix('=').unwrap_or(formula);
    let Ok(expr) = gridcraft_formula::parse(body) else { return Bounded::Value(Value::Error(CellError::Name)) };
    let exprs = HashMap::default();
    let mut rng = 0x1234_5678_9ABC_DEF0u64;
    let mut host = PassHost::new(wb, &exprs, &mut rng, now_serial());
    let mut ev = Evaluator::new(&mut host, sheet, at).with_work(work);
    let r = ev.eval(&expr);
    if ev.is_exhausted() {
        return Bounded::Exhausted;
    }
    match r {
        crate::eval::Ev::R(areas) => Bounded::Areas(areas),
        crate::eval::Ev::V(v) => Bounded::Value(v),
        crate::eval::Ev::L(_) => Bounded::Value(Value::Error(CellError::Calc)),
    }
}

fn prof_now() -> f64 {
    gridcraft_core::date::unix_now_ms().unwrap_or(0.0)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    use gridcraft_core::{CellRef, RangeRef, Value};
    use gridcraft_model::{Cell, Formula, Workbook};

    use super::{Calc, HashMap, Host, Intervals, Key, PassHost, index_spills};

    /// xorshift64 for repeatable pseudo-random cases.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn found(iv: &Intervals<u32>, a: u32, b: u32) -> Vec<u32> {
        let mut v = Vec::new();
        iv.query(a, b, &mut |t| v.push(*t));
        v.sort_unstable();
        v
    }

    /// One changed cell on a sheet with many small spills does not index them just because the
    /// batch (mostly on another sheet) is large; the sheet's own share of the batch decides.
    #[test]
    fn the_spill_index_is_chosen_from_the_sheets_own_changed_cells() {
        let small: Vec<RangeRef> = (0..1_000).map(|i| RangeRef::new(CellRef::new(i * 3, 0), CellRef::new(i * 3 + 2, 0))).collect();
        // Same areas, same batch of 5000: indexed when 5000 of them are on the sheet, walked when 1 is.
        assert!(index_spills(5_000, 5_000, small.iter()));
        assert!(!index_spills(5_000, 1, small.iter()));
        // Not a paste, or nothing to index: never.
        assert!(!index_spills(16, 16, small.iter()));
        assert!(!index_spills(5_000, 5_000, [].iter()));
    }

    /// A 1,000 x 1,000 Python result laid out from A1 in a pass, with the same values already
    /// spilled on the sheet from the last calculation, as a full recalculation finds them.
    fn big_python_spill() -> (Workbook, HashMap<Key, Value>) {
        let n = 1_000usize;
        let data: Vec<Value> = (0..n * n).map(|i| Value::Number(i as f64)).collect();
        let array = gridcraft_core::Array::new(n, n, data.clone()).unwrap();
        let mut wb = Workbook::new();
        let sh = wb.sheet_mut(0).unwrap();
        let anchor = CellRef::new(0, 0);
        let py = gridcraft_model::PyCode::new("big");
        sh.set_cell(anchor, Cell { value: Value::Number(0.0), py: Some(std::sync::Arc::new(py)), ..Cell::default() });
        let area = RangeRef::new(anchor, CellRef::new(n as u32 - 1, n as u32 - 1));
        sh.spill_ranges.insert(anchor, area);
        for (i, c) in area.iter().enumerate().skip(1) {
            sh.spill.insert(c, data[i].clone());
        }
        let mut python = HashMap::default();
        python.insert((0, anchor), Value::Array(std::sync::Arc::new(array)));
        (wb, python)
    }

    #[test]
    fn a_read_beside_a_large_spill_walks_none_of_it() {
        let (wb, python) = big_python_spill();
        let exprs = HashMap::default();
        let mut rng = 1u64;
        let mut host = PassHost::new(&wb, &exprs, &mut rng, 0.0).with_python(python);
        assert_eq!(host.cell_value(0, CellRef::new(0, 0)), Value::Number(0.0));
        // Beside the spill (column ALM, the 1,001st), over the rows it covers.
        let beside = RangeRef::parse("ALM1:ALM1000").unwrap();
        host.spill_visits = 0;
        assert!(host.range_values(0, beside).iter().all(Value::is_empty));
        assert!(host.visit_range(0, beside, &mut |_| true), "a disjoint read takes the fast path");
        assert_eq!(host.spill_visits, 0, "nothing of the spill is walked");
        // A read inside it still sees the spilled values, walking only the cells it reads.
        let inside = RangeRef::parse("B2:C3").unwrap();
        host.spill_visits = 0;
        assert_eq!(host.range_values(0, inside), [1_001.0, 1_002.0, 2_001.0, 2_002.0].map(Value::Number).to_vec());
        assert!(host.spill_visits <= 8, "{}", host.spill_visits);
        assert!(!host.visit_range(0, inside, &mut |_| true), "an overlapping read leaves the block to range_values");
    }

    /// A Python array laid out in this pass is seen by the indexed readers, whether the sheet
    /// has no spill from before or an older, smaller one.
    #[test]
    fn a_python_spill_found_in_the_pass_is_read_through_the_index() {
        for stale in [false, true] {
            let mut wb = Workbook::new();
            let sh = wb.sheet_mut(0).unwrap();
            let anchor = CellRef::new(0, 0);
            let py = gridcraft_model::PyCode::new("grid");
            sh.set_cell(anchor, Cell { value: Value::Number(0.0), py: Some(std::sync::Arc::new(py)), ..Cell::default() });
            if stale {
                // The last calculation spilled a 2 x 2 block of 100s.
                sh.spill_ranges.insert(anchor, RangeRef::new(anchor, CellRef::new(1, 1)));
                for c in RangeRef::new(anchor, CellRef::new(1, 1)).iter().skip(1) {
                    sh.spill.insert(c, Value::Number(100.0));
                }
            }
            let data: Vec<Value> = (1..=9).map(|i| Value::Number(f64::from(i))).collect();
            let array = gridcraft_core::Array::new(3, 3, data).unwrap();
            let mut python = HashMap::default();
            python.insert((0, anchor), Value::Array(std::sync::Arc::new(array)));
            let exprs = HashMap::default();
            let mut rng = 1u64;
            let mut host = PassHost::new(&wb, &exprs, &mut rng, 0.0).with_python(python);
            assert_eq!(host.cell_value(0, anchor), Value::Number(1.0));
            let all = RangeRef::parse("A1:C3").unwrap();
            let sum: f64 = host.range_values(0, all).iter().map(|v| if let Value::Number(n) = v { *n } else { 0.0 }).sum();
            assert_eq!(sum, 45.0, "stale={stale}: a SUM over the new 3 x 3 spill reads all of it");
            assert_eq!(host.cell_value(0, CellRef::new(2, 2)), Value::Number(9.0), "stale={stale}");
            assert!(!host.visit_range(0, all, &mut |_| true), "stale={stale}: the new spill leaves the block to range_values");
        }
    }

    #[test]
    fn intervals_match_a_linear_scan() {
        let mut iv: Intervals<u32> = Intervals::default();
        let mut model: Vec<(u32, u32, u32)> = Vec::new();
        let mut s = 0x9E37_79B9_7F4A_7C15;
        for step in 0..20_000u32 {
            let roll = rng(&mut s) % 10;
            if roll < 6 || model.is_empty() {
                let r0 = (rng(&mut s) % 500) as u32;
                let r1 = r0 + (rng(&mut s) % 40) as u32;
                if !model.contains(&(r0, r1, step)) {
                    iv.insert(r0, r1, step);
                    model.push((r0, r1, step));
                }
            } else {
                let i = (rng(&mut s) % model.len() as u64) as usize;
                let (r0, r1, t) = model.swap_remove(i);
                iv.remove(r0, r1, t);
                if roll == 9 {
                    // Removed and put back before the next merge.
                    iv.insert(r0, r1, t);
                    model.push((r0, r1, t));
                }
            }
            if rng(&mut s).is_multiple_of(50) {
                iv.settle();
            }
            if step.is_multiple_of(97) {
                let a = (rng(&mut s) % 560) as u32;
                let b = a + (rng(&mut s) % 30) as u32;
                let mut want: Vec<u32> = model.iter().filter(|(r0, r1, _)| *r0 <= b && *r1 >= a).map(|(_, _, t)| *t).collect();
                want.sort_unstable();
                assert_eq!(found(&iv, a, b), want, "rows {a}..={b} at step {step}");
            }
        }
        iv.settle();
        for row in 0..560 {
            let mut want: Vec<u32> = model.iter().filter(|(r0, r1, _)| *r0 <= row && row <= *r1).map(|(_, _, t)| *t).collect();
            want.sort_unstable();
            assert_eq!(found(&iv, row, row), want, "row {row}");
        }
        for (r0, r1, t) in std::mem::take(&mut model) {
            iv.remove(r0, r1, t);
        }
        iv.settle();
        assert!(iv.is_empty());
    }

    #[test]
    fn interval_queries_cost_log_n_plus_matches() {
        // 40k one-row ranges (like =SUM(E1:J1) down a column) and 1k ranges over everything:
        // a point query visits O(log n) tree nodes per match, not every range in the column.
        let n = 40_000u32;
        let mut iv: Intervals<u32> = Intervals::default();
        for r in 0..n {
            iv.insert(r, r, r);
        }
        for t in 0..1_000 {
            iv.insert(0, n, n + t);
        }
        iv.settle();
        let mut worst = 0;
        for row in (0..n).step_by(7) {
            let mut k = 0;
            let visited = iv.query(row, row, &mut |_| k += 1);
            assert_eq!(k, 1_001);
            worst = worst.max(visited);
        }
        // 1,001 matches in a tree of depth 16: well under 4 nodes per match plus the path.
        assert!(worst < 4 * 1_001 + 64, "visited {worst} nodes");
        let mut k = 0;
        let visited = iv.query(n + 1, n + 5, &mut |_| k += 1);
        assert_eq!(k, 0);
        assert!(visited < 64, "visited {visited} nodes for no match");
    }

    #[test]
    fn untaken_queries_list_each_interval_once_in_all() {
        // 1,000 ranges over every row of 5,000: point queries at every row used to list all 1,000
        // each time (5 million matches); sharing `taken`, each range is listed once in all.
        let n = 5_000u32;
        let mut iv: Intervals<u32> = Intervals::default();
        for t in 0..1_000 {
            iv.insert(0, n, t);
        }
        for r in 0..n {
            iv.insert(r, r, 10_000 + r);
        }
        iv.settle();
        let mut taken = Vec::new();
        let mut listed = Vec::new();
        for row in 0..=n + 3 {
            iv.query_untaken(row, row, &mut taken, &mut |t| {
                listed.push(*t);
                true
            });
        }
        listed.sort_unstable();
        let want: Vec<u32> = (0..1_000).chain(10_000..10_000 + n).collect();
        assert_eq!(listed, want);
        // An entry the caller declines is offered again.
        let mut taken = Vec::new();
        let mut offered = 0;
        for _ in 0..3 {
            iv.query_untaken(7, 7, &mut taken, &mut |t| {
                if *t == 10_007 {
                    offered += 1;
                }
                *t != 10_007
            });
        }
        assert_eq!(offered, 3);
    }

    #[test]
    fn dirty_closure_matches_a_walk_of_every_cell() {
        // Random formulas over single cells, narrow ranges and wide (over 64 columns) ranges, some
        // removed or left unsettled: the closure equals the one found by listing every dependent
        // of every changed cell, area cells included, as before `taken` and areas existed.
        use super::{HashSet, Node};
        use crate::eval::Area;
        use std::collections::VecDeque;
        let mut s = 0x2545_F491_4F6C_DD1D;
        for case in 0..40 {
            let mut wb = Workbook::new();
            let mut calc = Calc::new();
            let pick = |s: &mut u64, n: u64| (rng(s) % n) as u32;
            let mut keys: Vec<Key> = Vec::new();
            for _ in 0..300 {
                let k = (0, CellRef::new(pick(&mut s, 120), pick(&mut s, 150)));
                let mut areas = Vec::new();
                for _ in 0..1 + pick(&mut s, 3) {
                    let start = CellRef::new(pick(&mut s, 120), pick(&mut s, 150));
                    let end = match pick(&mut s, 3) {
                        0 => start,
                        1 => CellRef::new(start.row + pick(&mut s, 40), start.col + pick(&mut s, 5)),
                        _ => CellRef::new(start.row + pick(&mut s, 20), start.col + 65 + pick(&mut s, 30)),
                    };
                    areas.push(Area { sheet: 0, range: RangeRef::new(start, end) });
                }
                if let Some(sh) = wb.sheet_mut(0) {
                    sh.cells.set(k.1, Cell::formula(Formula::new("=1")));
                }
                calc.graph.insert(k, Node { areas, dynamic: false });
                keys.push(k);
                if rng(&mut s).is_multiple_of(7) {
                    calc.graph.settle();
                }
            }
            for _ in 0..30 {
                let k = keys[(rng(&mut s) % keys.len() as u64) as usize];
                calc.graph.remove(k);
                if let Some(sh) = wb.sheet_mut(0) {
                    sh.cells.remove(k.1);
                }
            }
            if case % 2 == 0 {
                calc.graph.settle();
            }
            let seeds: Vec<Key> = (0..1 + pick(&mut s, 25)).map(|_| (0, CellRef::new(pick(&mut s, 140), pick(&mut s, 220)))).collect();
            let areas: Vec<(usize, RangeRef)> = (0..pick(&mut s, 3))
                .map(|_| {
                    let start = CellRef::new(pick(&mut s, 120), pick(&mut s, 150));
                    (0, RangeRef::new(start, CellRef::new(start.row + pick(&mut s, 30), start.col + pick(&mut s, 80))))
                })
                .collect();
            let got: HashSet<Key> = calc.dirty_closure(&wb, &seeds, &areas).into_iter().collect();
            // The walk before: every area cell is a seed, and every cell lists all its dependents.
            let mut all = seeds.clone();
            for (si, r) in &areas {
                all.extend(r.iter().map(|c| (*si, c)));
            }
            let mut want: HashSet<Key> = HashSet::default();
            let mut queue: VecDeque<Key> = VecDeque::new();
            let mut deps = Vec::new();
            for k in &all {
                if calc.graph.nodes.contains_key(k) && want.insert(*k) {
                    queue.push_back(*k);
                }
                deps.clear();
                calc.graph.dependents(k.0, k.1, &mut deps);
                queue.extend(deps.iter().copied().filter(|d| want.insert(*d)));
            }
            while let Some(k) = queue.pop_front() {
                deps.clear();
                calc.graph.dependents(k.0, k.1, &mut deps);
                queue.extend(deps.iter().copied().filter(|d| want.insert(*d)));
            }
            assert_eq!(got, want, "case {case}");
        }
    }

    #[test]
    fn clearing_a_spill_recalculates_formulas_reading_it_once_each() {
        // 300 formulas read the area an anchor spills into; clearing the anchor changes all of it
        // and every one of them is recalculated (they were listed once per spilled cell).
        let mut wb = Workbook::new();
        let mut calc = Calc::new();
        let set = |wb: &mut Workbook, calc: &mut Calc, at: CellRef, input: &str| {
            if let Some(sh) = wb.sheet_mut(0) {
                if input.is_empty() {
                    sh.cells.remove(at);
                } else {
                    sh.cells.set(at, Cell::formula(Formula::new(input)));
                }
            }
            calc.cells_changed(wb, &[(0, at)]);
        };
        set(&mut wb, &mut calc, CellRef::new(0, 0), "=SEQUENCE(2000)");
        for r in 0..300 {
            set(&mut wb, &mut calc, CellRef::new(r, 2), "=COUNT(A1:A2000)");
        }
        assert_eq!(wb.sheet(0).map(|s| s.value(CellRef::new(299, 2))), Some(Value::Number(2000.0)));
        set(&mut wb, &mut calc, CellRef::new(0, 0), "");
        for r in 0..300 {
            assert_eq!(wb.sheet(0).map(|s| s.value(CellRef::new(r, 2))), Some(Value::Number(0.0)), "row {r}");
        }
    }

    /// `rows` rows of six numbers (E:J) and `=SUM(E{r}:J{r})` in K.
    fn sum_rows(rows: u32) -> Workbook {
        let mut wb = Workbook::new();
        if let Some(sh) = wb.sheet_mut(0) {
            for r in 0..rows {
                for c in 4..10 {
                    sh.cells.set(CellRef::new(r, c), Cell::value(Value::Number(f64::from(r % 97 + c))));
                }
                sh.cells.set(CellRef::new(r, 10), Cell::formula(Formula::new(&format!("=SUM(E{0}:J{0})", r + 1))));
            }
        }
        wb
    }

    /// Opening (full recalc), changing every value of a column, and re-entering every formula.
    fn open_and_bulk_edit(rows: u32) -> Duration {
        let mut wb = sum_rows(rows);
        let mut calc = Calc::new();
        let started = gridcraft_core::date::unix_now_ms().unwrap_or(0.0);
        calc.recalc_all(&mut wb);
        let values: Vec<Key> = (0..rows).map(|r| (0, CellRef::new(r, 4))).collect();
        if let Some(sh) = wb.sheet_mut(0) {
            for (_, c) in &values {
                sh.cells.set(*c, Cell::value(Value::Number(1.0)));
            }
        }
        calc.cells_changed(&mut wb, &values);
        let formulas: Vec<Key> = (0..rows).map(|r| (0, CellRef::new(r, 10))).collect();
        calc.cells_changed(&mut wb, &formulas);
        let ended = gridcraft_core::date::unix_now_ms().unwrap_or(started);
        let elapsed = Duration::from_secs_f64((ended - started).max(0.0) / 1000.0);
        let last = wb.sheet(0).map(|s| s.value(CellRef::new(rows - 1, 10)));
        let r = rows - 1;
        assert_eq!(last, Some(Value::Number(f64::from(1 + (5..10).map(|c| r % 97 + c).sum::<u32>()))));
        elapsed
    }

    #[test]
    fn range_formulas_recalc_and_bulk_edit_correctly() {
        // The values behind the timing test below, checked on every run.
        open_and_bulk_edit(500);
    }

    #[test]
    #[ignore = "timing-sensitive; run with --ignored in release"]
    fn recalc_with_range_formulas_scales_linearly() {
        // Each row's SUM reads a range in the same columns. Recalculating on open used to walk
        // every row per range read (the used range), and finding a cell's dependents scanned
        // every range in its column: 4x the rows took 16x the time. Linear is 4x; allow 8x.
        let best = |rows| (0..3).map(|_| open_and_bulk_edit(rows)).min().unwrap_or_default();
        let small = best(5_000);
        let large = best(20_000);
        assert!(large < small * 8 + Duration::from_millis(100), "5k rows {small:?}, 20k rows {large:?}");
    }

    #[test]
    fn range_dependents_after_edits() {
        // Formulas that read ranges, wide ranges and whole columns, found again after edits.
        let mut wb = Workbook::new();
        let mut calc = Calc::new();
        let set = |wb: &mut Workbook, calc: &mut Calc, at: &str, input: &str| {
            let c = CellRef::parse(at).unwrap();
            let cell = if input.starts_with('=') { Cell::formula(Formula::new(input)) } else { Cell::value(Value::Number(input.parse().unwrap())) };
            wb.sheet_mut(0).unwrap().cells.set(c, cell);
            calc.cells_changed(wb, &[(0, c)]);
        };
        let num = |wb: &Workbook, at: &str| {
            let v = wb.sheet(0).unwrap().value(CellRef::parse(at).unwrap());
            v.as_f64().unwrap_or_else(|| panic!("{at} = {v:?}"))
        };
        set(&mut wb, &mut calc, "A1", "1");
        set(&mut wb, &mut calc, "B2", "2");
        set(&mut wb, &mut calc, "CZ3", "3");
        set(&mut wb, &mut calc, "DA5", "=SUM(A1:B2)");
        set(&mut wb, &mut calc, "DA6", "=SUM(A1:CZ3)");
        set(&mut wb, &mut calc, "DA7", "=SUM(B:B)");
        set(&mut wb, &mut calc, "DA8", "=SUM(2:2)");
        assert_eq!((num(&wb, "DA5"), num(&wb, "DA6"), num(&wb, "DA7"), num(&wb, "DA8")), (3.0, 6.0, 2.0, 2.0));
        set(&mut wb, &mut calc, "B2", "20");
        assert_eq!((num(&wb, "DA5"), num(&wb, "DA6"), num(&wb, "DA7"), num(&wb, "DA8")), (21.0, 24.0, 20.0, 20.0));
        // Re-entering a formula with another range drops the old one.
        set(&mut wb, &mut calc, "DA5", "=SUM(A1:A2)");
        set(&mut wb, &mut calc, "B2", "5");
        assert_eq!(num(&wb, "DA5"), 1.0);
        let mut deps = Vec::new();
        calc.graph.dependents(0, CellRef::parse("B2").unwrap(), &mut deps);
        deps.sort_unstable();
        let at = |s: &str| (0, CellRef::parse(s).unwrap());
        assert_eq!(deps, vec![at("DA6"), at("DA7"), at("DA8")]);
        deps.clear();
        calc.graph.dependents_of_range(0, RangeRef::new(CellRef::parse("A1").unwrap(), CellRef::parse("A9").unwrap()), &mut deps);
        deps.sort_unstable();
        deps.dedup();
        assert_eq!(deps, vec![at("DA5"), at("DA6"), at("DA8")]);
        set(&mut wb, &mut calc, "CZ3", "30");
        assert_eq!(num(&wb, "DA6"), 36.0);
        // Ranges on another sheet.
        wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Data")));
        set(&mut wb, &mut calc, "DA9", "=SUM(Data!A1:A3)+SUM(Data!A:CZ)");
        assert_eq!(num(&wb, "DA9"), 0.0);
        wb.sheet_mut(1).unwrap().cells.set(CellRef::parse("A2").unwrap(), Cell::value(Value::Number(4.0)));
        calc.cells_changed(&mut wb, &[(1, CellRef::parse("A2").unwrap())]);
        assert_eq!(num(&wb, "DA9"), 8.0);
        deps.clear();
        calc.graph.dependents(1, CellRef::parse("A2").unwrap(), &mut deps);
        assert_eq!(deps, vec![at("DA9"), at("DA9")]);
    }

    /// A formula at the nesting limit reading thousands of formulas not calculated yet suspends the
    /// evaluations in progress once, not once per formula read.
    #[test]
    fn wide_read_at_the_nesting_limit_suspends_once() {
        const WIDTH: u32 = 3000;
        let mut wb = Workbook::new();
        let mut exprs = HashMap::default();
        let mut put = |wb: &mut Workbook, at: String, text: String| {
            let at = CellRef::parse(&at).unwrap();
            wb.sheet_mut(0).unwrap().cells.set(at, Cell::formula(Formula::new(&text)));
            exprs.insert((0, at), std::sync::Arc::new(gridcraft_formula::parse(&text[1..]).unwrap()));
        };
        // A1 reads A2, ..., A64 (the 64th formula deep) reads a wide range of formulas.
        for r in 1..64 {
            put(&mut wb, format!("A{r}"), format!("=A{}+1", r + 1));
        }
        put(&mut wb, "A64".to_string(), format!("=SUM(B1:B{WIDTH})"));
        for r in 1..=WIDTH {
            put(&mut wb, format!("B{r}"), format!("={r}*2"));
        }
        let mut rng = 1u64;
        let mut host = PassHost::new(&wb, &exprs, &mut rng, 0.0);
        host.calculate((0, CellRef::parse("A1").unwrap()));
        let total = (WIDTH * (WIDTH + 1)) as f64;
        assert_eq!(host.results.get(&(0, CellRef::parse("A1").unwrap())), Some(&Value::Number(total + 63.0)));
        assert!(host.cycles.is_empty());
        assert!(host.suspensions <= 3, "{} suspensions", host.suspensions);
    }

    /// For tests of `PassHost::visit_range`, which can't be reached from outside this module: the
    /// values it hands over for `range` while the formulas in `pending` (cell, formula) have not been
    /// calculated yet, or `None` when it declines the block.
    pub(crate) fn probe_visit_range(wb: &Workbook, pending: &[(&str, &str)], range: RangeRef) -> Option<Vec<Value>> {
        let mut exprs = HashMap::default();
        for (at, formula) in pending {
            let expr = gridcraft_formula::parse(formula.strip_prefix('=').unwrap_or(formula)).ok()?;
            exprs.insert((0, CellRef::parse(at)?), std::sync::Arc::new(expr));
        }

        // A cell in `pending` is calculated the first time something reads it, which is the state
        // of a real pass partway through; `visit_range` has to cope with the formulas it reads
        // being calculated under it.
        let mut rng = 1u64;
        let mut host = PassHost::new(wb, &exprs, &mut rng, 0.0);

        let mut seen = Vec::new();
        host.visit_range(0, range, &mut |v| {
            seen.push(v.clone());
            true
        })
        .then_some(seen)
    }
}
