use super::RangeRef;
use crate::eval::Area;
use gridcraft_core::CellRef;
use std::collections::BTreeSet;
use std::hash::{BuildHasherDefault, Hasher};

/// A fast, non-cryptographic hasher for cell keys (FxHash-style multiply-rotate).
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

impl Hasher for FxHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(*b as u64);
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
}

pub(super) type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;

pub(super) type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

pub type Key = (usize, CellRef);

#[derive(Clone, Debug, Default)]
pub(super) struct Node {
    pub(super) areas: Vec<Area>,
    pub(super) dynamic: bool,
}

/// Ranges wider than this many columns go to the per-sheet wide index instead of per-column ones.
const WIDE: u32 = 64;

/// Insertions an [`Intervals`] keeps aside (scanned by every query) before merging them in.
const PENDING: usize = 32;

/// Row intervals `[r0, r1]`, each with a payload, answering "which intervals overlap rows
/// `a..=b`?" in O(log n + k).
///
/// The intervals are sorted by start with an implicit segment tree of their largest end, so a query
/// only descends into subtrees that hold a match. Removals mark entries dead (re-inserting one
/// revives it); insertions wait in a small sorted set. [`Intervals::settle`] merges them and drops
/// dead entries once either grows, so mutations cost O(log n) amortised.
#[derive(Clone, Debug)]
pub(super) struct Intervals<T> {
    /// Sorted, without duplicates.
    items: Vec<(u32, u32, T)>,
    dead: Vec<bool>,
    dead_count: usize,
    /// Segment tree over `items`: node `i` has children `2i` and `2i + 1`, leaves start at
    /// `max_end.len() / 2`. Each node holds the largest end below it (dead entries included).
    max_end: Vec<u32>,
    /// Inserted since the last merge.
    pending: BTreeSet<(u32, u32, T)>,
}

impl<T> Default for Intervals<T> {
    fn default() -> Self {
        Intervals { items: Vec::new(), dead: Vec::new(), dead_count: 0, max_end: Vec::new(), pending: BTreeSet::new() }
    }
}

impl<T: Copy + Ord> Intervals<T> {
    pub(super) fn is_empty(&self) -> bool {
        self.items.len() == self.dead_count && self.pending.is_empty()
    }

    pub(super) fn insert(&mut self, r0: u32, r1: u32, t: T) {
        let it = (r0, r1, t);
        if let Ok(i) = self.items.binary_search(&it) {
            if let Some(d) = self.dead.get_mut(i)
                && *d
            {
                *d = false;
                self.dead_count -= 1;
            }
            return;
        }
        self.pending.insert(it);
    }

    pub(super) fn remove(&mut self, r0: u32, r1: u32, t: T) {
        let it = (r0, r1, t);
        if let Ok(i) = self.items.binary_search(&it) {
            if let Some(d) = self.dead.get_mut(i)
                && !*d
            {
                *d = true;
                self.dead_count += 1;
            }
            return;
        }
        self.pending.remove(&it);
    }

    /// Merges pending insertions and drops dead entries when there are enough of them.
    pub(super) fn settle(&mut self) {
        if self.pending.len() <= PENDING && self.dead_count <= (self.items.len() / 2).max(PENDING) {
            return;
        }
        let mut all: Vec<(u32, u32, T)> = Vec::with_capacity(self.items.len() - self.dead_count + self.pending.len());
        all.extend(self.items.iter().zip(&self.dead).filter(|(_, d)| !**d).map(|(it, _)| *it));
        all.extend(std::mem::take(&mut self.pending));
        // Two sorted runs: the stable sort merges them in linear time.
        all.sort();
        all.dedup();
        let size = all.len().next_power_of_two();
        let mut max_end = vec![0; 2 * size];
        if let Some(leaves) = max_end.get_mut(size..) {
            for (slot, it) in leaves.iter_mut().zip(&all) {
                *slot = it.1;
            }
        }
        for i in (1..size).rev() {
            let m = max_end.get(2 * i).copied().unwrap_or(0).max(max_end.get(2 * i + 1).copied().unwrap_or(0));
            if let Some(slot) = max_end.get_mut(i) {
                *slot = m;
            }
        }
        self.dead = vec![false; all.len()];
        self.dead_count = 0;
        self.items = all;
        self.max_end = max_end;
    }

    /// Calls `f` for every interval overlapping rows `a..=b` (`r0 <= b && r1 >= a`). Returns the
    /// number of tree nodes visited.
    pub(super) fn query(&self, a: u32, b: u32, f: &mut dyn FnMut(&T)) -> usize {
        for it in &self.pending {
            if it.0 <= b && it.1 >= a {
                f(&it.2);
            }
        }
        // Entries before `p` start at or before row `b`.
        let p = self.items.partition_point(|it| it.0 <= b);
        if p == 0 {
            return 0;
        }
        self.walk(1, 0, self.max_end.len() / 2, p, a, f)
    }

    /// [`Intervals::query`] that skips entries an earlier call with the same `taken` already
    /// took (and whole subtrees of them), so that a series of queries lists each interval once in
    /// all. `f` answers whether it took the entry; one it declines is offered again later.
    /// `taken` belongs to this index as it is now; start it empty. Pending insertions (a few at
    /// most after [`Intervals::settle`]) are offered every time.
    pub(super) fn query_untaken(&self, a: u32, b: u32, taken: &mut Vec<bool>, f: &mut dyn FnMut(&T) -> bool) {
        for it in &self.pending {
            if it.0 <= b && it.1 >= a {
                f(&it.2);
            }
        }
        let p = self.items.partition_point(|it| it.0 <= b);
        if p == 0 {
            return;
        }
        if taken.len() != self.max_end.len() {
            *taken = vec![false; self.max_end.len()];
        }
        self.walk_untaken(1, 0, self.max_end.len() / 2, p, a, taken, f);
    }

    /// Returns whether everything under `node` has now been reported (or is dead).
    #[allow(clippy::too_many_arguments)]
    fn walk_untaken(&self, node: usize, lo: usize, hi: usize, p: usize, a: u32, taken: &mut [bool], f: &mut dyn FnMut(&T) -> bool) -> bool {
        if taken.get(node).copied().unwrap_or(true) {
            return true;
        }
        if lo >= p || self.max_end.get(node).is_none_or(|m| *m < a) {
            return false;
        }
        let done = if hi - lo <= 1 {
            match (self.items.get(lo), self.dead.get(lo)) {
                (Some(it), Some(false)) if it.1 >= a => f(&it.2),
                (Some(_), Some(true)) => true,
                _ => false,
            }
        } else {
            let mid = lo + (hi - lo) / 2;
            let left = self.walk_untaken(2 * node, lo, mid, p, a, taken, f);
            let right = self.walk_untaken(2 * node + 1, mid, hi, p, a, taken, f);
            left && right
        };
        if done && let Some(slot) = taken.get_mut(node) {
            *slot = true;
        }
        done
    }

    fn walk(&self, node: usize, lo: usize, hi: usize, p: usize, a: u32, f: &mut dyn FnMut(&T)) -> usize {
        if lo >= p || self.max_end.get(node).is_none_or(|m| *m < a) {
            return 1;
        }
        if hi - lo <= 1 {
            if let (Some(it), Some(false)) = (self.items.get(lo), self.dead.get(lo))
                && it.1 >= a
            {
                f(&it.2);
            }
            return 1;
        }
        let mid = lo + (hi - lo) / 2;
        1 + self.walk(2 * node, lo, mid, p, a, f) + self.walk(2 * node + 1, mid, hi, p, a, f)
    }
}

/// Range-index entries one dependency walk has already listed (see [`Graph::dependents_untaken`]).
#[derive(Default)]
pub(super) struct Taken {
    cols: HashMap<(usize, u32), Vec<bool>>,
    wide: HashMap<usize, Vec<bool>>,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub(super) nodes: HashMap<Key, Node>,
    /// single cell → dependents
    cell_deps: HashMap<Key, HashSet<Key>>,
    /// (sheet, col) → row intervals of the ranges (up to `WIDE` columns) that cover the column
    col_deps: HashMap<(usize, u32), Intervals<Key>>,
    /// sheet → row intervals of wider ranges, with their columns: (first, last, dependent)
    wide_deps: HashMap<usize, Intervals<(u32, u32, Key)>>,
    pub(super) dynamic: HashSet<Key>,
    /// Range indexes changed since the last [`Graph::settle`].
    unsettled_cols: HashSet<(usize, u32)>,
    unsettled_wide: HashSet<usize>,
}

/// A formula's precedents split by shape: single cells, column intervals and wide ranges.
type Entries = (Vec<Key>, Vec<(usize, u32, u32, u32)>, Vec<(usize, RangeRef)>);

impl Graph {
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Index entries of a formula's precedents, without duplicates: single cells, column
    /// intervals and wide ranges.
    fn entries(areas: &[Area]) -> Entries {
        let (mut cells, mut cols, mut wide) = (Vec::new(), Vec::new(), Vec::new());
        for a in areas {
            if a.range.is_single() {
                cells.push((a.sheet, a.range.start));
            } else if a.range.width() > WIDE {
                wide.push((a.sheet, a.range));
            } else {
                for c in a.range.start.col..=a.range.end.col {
                    cols.push((a.sheet, c, a.range.start.row, a.range.end.row));
                }
            }
        }
        cells.sort_unstable();
        cells.dedup();
        cols.sort_unstable();
        cols.dedup();
        wide.sort_unstable_by_key(|(s, r)| (*s, r.start, r.end));
        wide.dedup();
        (cells, cols, wide)
    }

    pub(super) fn insert(&mut self, k: Key, node: Node) {
        self.remove(k);
        let (cells, cols, wide) = Self::entries(&node.areas);
        for c in cells {
            self.cell_deps.entry(c).or_default().insert(k);
        }
        for (s, c, r0, r1) in cols {
            self.col_deps.entry((s, c)).or_default().insert(r0, r1, k);
            self.unsettled_cols.insert((s, c));
        }
        for (s, r) in wide {
            self.wide_deps.entry(s).or_default().insert(r.start.row, r.end.row, (r.start.col, r.end.col, k));
            self.unsettled_wide.insert(s);
        }
        if node.dynamic {
            self.dynamic.insert(k);
        }
        self.nodes.insert(k, node);
    }

    pub(super) fn remove(&mut self, k: Key) {
        let Some(old) = self.nodes.remove(&k) else { return };
        let (cells, cols, wide) = Self::entries(&old.areas);
        for c in cells {
            if let Some(v) = self.cell_deps.get_mut(&c) {
                v.remove(&k);
                if v.is_empty() {
                    self.cell_deps.remove(&c);
                }
            }
        }
        for (s, c, r0, r1) in cols {
            if let Some(v) = self.col_deps.get_mut(&(s, c)) {
                v.remove(r0, r1, k);
                self.unsettled_cols.insert((s, c));
            }
        }
        for (s, r) in wide {
            if let Some(v) = self.wide_deps.get_mut(&s) {
                v.remove(r.start.row, r.end.row, (r.start.col, r.end.col, k));
                self.unsettled_wide.insert(s);
            }
        }
        self.dynamic.remove(&k);
    }

    /// Tidies the range indexes changed by `insert`/`remove` (queries stay correct without it,
    /// just slower).
    pub(super) fn settle(&mut self) {
        for key in std::mem::take(&mut self.unsettled_cols) {
            if let Some(v) = self.col_deps.get_mut(&key) {
                v.settle();
                if v.is_empty() {
                    self.col_deps.remove(&key);
                }
            }
        }
        for key in std::mem::take(&mut self.unsettled_wide) {
            if let Some(v) = self.wide_deps.get_mut(&key) {
                v.settle();
                if v.is_empty() {
                    self.wide_deps.remove(&key);
                }
            }
        }
    }

    /// Formulas that read cell `c` of `sheet`.
    pub fn dependents(&self, sheet: usize, c: CellRef, out: &mut Vec<Key>) {
        if let Some(v) = self.cell_deps.get(&(sheet, c)) {
            out.extend(v.iter().copied());
        }
        if let Some(v) = self.col_deps.get(&(sheet, c.col)) {
            v.query(c.row, c.row, &mut |k| out.push(*k));
        }
        if let Some(v) = self.wide_deps.get(&sheet) {
            v.query(c.row, c.row, &mut |(c0, c1, k)| {
                if *c0 <= c.col && c.col <= *c1 {
                    out.push(*k);
                }
            });
        }
    }

    /// [`Graph::dependents`] leaving out the formulas whose range reads an earlier call with the
    /// same `taken` already listed. A walk over many changed cells that a few broad ranges cover
    /// then lists those formulas once, instead of once per cell (which squared the work). The
    /// graph must not change while `taken` is in use.
    pub(super) fn dependents_untaken(&self, sheet: usize, c: CellRef, taken: &mut Taken, out: &mut Vec<Key>) {
        if let Some(v) = self.cell_deps.get(&(sheet, c)) {
            out.extend(v.iter().copied());
        }
        if let Some(v) = self.col_deps.get(&(sheet, c.col)) {
            v.query_untaken(c.row, c.row, taken.cols.entry((sheet, c.col)).or_default(), &mut |k| {
                out.push(*k);
                true
            });
        }
        if let Some(v) = self.wide_deps.get(&sheet) {
            // A wide entry is taken once it covers the cell's column too.
            v.query_untaken(c.row, c.row, taken.wide.entry(sheet).or_default(), &mut |(c0, c1, k)| {
                let hit = *c0 <= c.col && c.col <= *c1;
                if hit {
                    out.push(*k);
                }
                hit
            });
        }
    }

    /// Formulas that read any cell of `range` (a formula can be listed more than once).
    pub fn dependents_of_range(&self, sheet: usize, range: RangeRef, out: &mut Vec<Key>) {
        if range.count() <= 4096 {
            for c in range.iter() {
                if let Some(v) = self.cell_deps.get(&(sheet, c)) {
                    out.extend(v.iter().copied());
                }
            }
        } else {
            for ((s, c), v) in &self.cell_deps {
                if *s == sheet && range.contains(*c) {
                    out.extend(v.iter().copied());
                }
            }
        }
        let (r0, r1) = (range.start.row, range.end.row);
        if (range.width() as usize) <= self.col_deps.len() {
            for col in range.start.col..=range.end.col {
                if let Some(v) = self.col_deps.get(&(sheet, col)) {
                    v.query(r0, r1, &mut |k| out.push(*k));
                }
            }
        } else {
            for ((s, col), v) in &self.col_deps {
                if *s == sheet && range.start.col <= *col && *col <= range.end.col {
                    v.query(r0, r1, &mut |k| out.push(*k));
                }
            }
        }
        if let Some(v) = self.wide_deps.get(&sheet) {
            v.query(r0, r1, &mut |(c0, c1, k)| {
                if *c0 <= range.end.col && range.start.col <= *c1 {
                    out.push(*k);
                }
            });
        }
    }

    /// Precedent areas of a formula cell (for Trace Precedents).
    pub fn precedents_of(&self, k: Key) -> Vec<Area> {
        self.nodes.get(&k).map(|n| n.areas.clone()).unwrap_or_default()
    }
}
