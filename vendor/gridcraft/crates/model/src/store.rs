//! Copy-on-write sparse cell storage.
//!
//! Cells live in bands of 64 rows, each band behind an `Arc`. Cloning a store (an undo snapshot)
//! copies only the band pointers; editing a cell copies just its band.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::ops::RangeInclusive;
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef};
use serde::{Deserialize, Serialize};

use crate::cell::Cell;

const BAND: u32 = 64;

fn has_value(cell: &Cell) -> bool {
    cell.has_content()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Band {
    /// row → (col → cell)
    rows: BTreeMap<u32, BTreeMap<u32, Cell>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CellStore {
    bands: BTreeMap<u32, Arc<Band>>,
    len: usize,
}

impl CellStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn get(&self, c: CellRef) -> Option<&Cell> {
        self.bands.get(&(c.row / BAND))?.rows.get(&c.row)?.get(&c.col)
    }
    /// Mutable access (copies the band if shared).
    pub fn get_mut(&mut self, c: CellRef) -> Option<&mut Cell> {
        let band = self.bands.get_mut(&(c.row / BAND))?;
        if !band.rows.get(&c.row).is_some_and(|r| r.contains_key(&c.col)) {
            return None;
        }
        Arc::make_mut(band).rows.get_mut(&c.row)?.get_mut(&c.col)
    }
    /// Stores a cell; a default (empty, unstyled) cell is removed instead.
    pub fn set(&mut self, c: CellRef, cell: Cell) {
        if cell.is_blank() {
            self.remove(c);
            return;
        }
        let band = Arc::make_mut(self.bands.entry(c.row / BAND).or_default());
        if band.rows.entry(c.row).or_default().insert(c.col, cell).is_none() {
            self.len += 1;
        }
    }
    /// Puts every cell in the band its row says and recounts the length — serialized band keys
    /// and the cached count are derived state, and the ones a file carries are not trusted. Cells
    /// outside the sheet grid are dropped; returns how many (#1008).
    pub fn rebuild(&mut self) -> usize {
        let mut out = CellStore::new();
        let mut dropped = 0;
        for (_, band) in std::mem::take(&mut self.bands) {
            for (row, cols) in band.rows.iter() {
                for (col, cell) in cols.iter() {
                    let at = CellRef::new(*row, *col);
                    if at.is_valid() {
                        out.set(at, cell.clone());
                    } else {
                        dropped += 1;
                    }
                }
            }
        }
        *self = out;
        dropped
    }

    pub fn remove(&mut self, c: CellRef) -> Option<Cell> {
        let key = c.row / BAND;
        let band = self.bands.get_mut(&key)?;
        if !band.rows.get(&c.row).is_some_and(|r| r.contains_key(&c.col)) {
            return None;
        }
        let b = Arc::make_mut(band);
        let row = b.rows.get_mut(&c.row)?;
        let old = row.remove(&c.col);
        if row.is_empty() {
            b.rows.remove(&c.row);
        }
        if b.rows.is_empty() {
            self.bands.remove(&key);
        }
        if old.is_some() {
            self.len -= 1;
        }
        old
    }
    /// Cells in `r`, row-major. Efficient for sparse data: skips empty bands and rows.
    pub fn iter_range(&self, r: RangeRef) -> impl Iterator<Item = (CellRef, &Cell)> + '_ {
        let (b0, b1) = (r.start.row / BAND, r.end.row / BAND);
        self.bands.range(b0..=b1).flat_map(move |(_, band)| {
            band.rows
                .range(r.start.row..=r.end.row)
                .flat_map(move |(row, cols)| cols.range(r.start.col..=r.end.col).map(move |(col, cell)| (CellRef::new(*row, *col), cell)))
        })
    }
    pub fn iter(&self) -> impl Iterator<Item = (CellRef, &Cell)> + '_ {
        self.bands
            .values()
            .flat_map(|band| band.rows.iter().flat_map(|(row, cols)| cols.iter().map(move |(col, cell)| (CellRef::new(*row, *col), cell))))
    }
    /// Cells of one row in `c0..=c1`.
    pub fn row(&self, row: u32, c0: u32, c1: u32) -> impl Iterator<Item = (u32, &Cell)> + '_ {
        self.bands.get(&(row / BAND)).and_then(|b| b.rows.get(&row)).into_iter().flat_map(move |cols| cols.range(c0..=c1).map(|(c, cell)| (*c, cell)))
    }
    /// Smallest range containing every stored cell, `None` when empty.
    pub fn used_range(&self) -> Option<RangeRef> {
        let first_band = self.bands.values().next()?;
        let last_band = self.bands.values().next_back()?;
        let r0 = *first_band.rows.keys().next()?;
        let r1 = *last_band.rows.keys().next_back()?;
        let mut c0 = u32::MAX;
        let mut c1 = 0;
        for band in self.bands.values() {
            for cols in band.rows.values() {
                if let (Some(a), Some(b)) = (cols.keys().next(), cols.keys().next_back()) {
                    c0 = c0.min(*a);
                    c1 = c1.max(*b);
                }
            }
        }
        if c0 == u32::MAX {
            return None;
        }
        Some(RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1)))
    }
    /// Last stored row in column `col` at or above `below_or_at` (for Ctrl+arrow navigation).
    pub fn has(&self, c: CellRef) -> bool {
        self.get(c).is_some_and(has_value)
    }
    /// Last row (inclusive, capped at `cap`) of the maximal run of contiguous rows at or after
    /// `from.row` that store a cell in `from.col` with a non-empty *value* in `from.col`. `None`
    /// when `from` itself doesn't qualify.
    ///
    /// Deliberately narrower than `has`/`get` (which also count a formula with no cached value
    /// yet, for Ctrl+arrow navigation): this matches `Sheet::value_ref`, whose callers test the
    /// `Value` it returns with `!v.is_empty()` regardless of whether a formula produced it — a
    /// formula cell with an empty cached value (`=""`, or not yet recalculated) is "empty" to
    /// them. A stored cell with no value and no formula (for example a formatted spill child) does
    /// fall through to `spill` in `value_ref`, so `last_contiguous_nonempty` asks `value_ref`
    /// about the row where this run stops.
    ///
    /// Walks each band's row map with `BTreeMap::range` (one in-order pass per band) instead of
    /// one `get` per row, so a long, densely-populated run costs roughly one tree-traversal step
    /// per row rather than one binary search per row.
    pub fn contiguous_value_run(&self, from: CellRef, cap: u32) -> Option<u32> {
        let mut last = None;
        let mut expect = from.row;
        while expect <= cap {
            let band_key = expect / BAND;
            let Some(band) = self.bands.get(&band_key) else { break };
            let band_hi = ((band_key + 1) * BAND - 1).min(cap);
            let mut advanced = false;
            for (r, cols) in band.rows.range(expect..=band_hi) {
                if *r != expect || cols.get(&from.col).is_none_or(|c| c.value.is_empty()) {
                    return last;
                }
                last = Some(expect);
                expect += 1;
                advanced = true;
            }
            if !advanced {
                break;
            }
        }
        last
    }
    /// Positions whose cells differ between `self` and `other` (stored on one side only, or not
    /// equal). Bands both stores share are skipped, so comparing an edited copy with the store
    /// it was cloned from only visits the bands the edit touched.
    pub fn diff(&self, other: &CellStore) -> Vec<CellRef> {
        let empty = Band::default();
        let mut keys: Vec<u32> = self.bands.keys().chain(other.bands.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        let mut out = Vec::new();
        for k in keys {
            let (a, b) = (self.bands.get(&k), other.bands.get(&k));
            if let (Some(a), Some(b)) = (a, b)
                && Arc::ptr_eq(a, b)
            {
                continue;
            }
            let (a, b) = (a.map_or(&empty, |x| x), b.map_or(&empty, |x| x));
            for (row, cols) in &a.rows {
                for (col, cell) in cols {
                    if b.rows.get(row).and_then(|r| r.get(col)) != Some(cell) {
                        out.push(CellRef::new(*row, *col));
                    }
                }
            }
            for (row, cols) in &b.rows {
                for col in cols.keys() {
                    if !a.rows.get(row).is_some_and(|r| r.contains_key(col)) {
                        out.push(CellRef::new(*row, *col));
                    }
                }
            }
        }
        out
    }
    /// Approximate heap storage kept alive by this snapshot's bands that `newer` no longer
    /// shares. This is for bounding Undo history, not for reporting process memory: BTreeMap
    /// nodes and allocator overhead vary by platform. Shared formulas and values are counted
    /// only when the newer band no longer holds the same Arc.
    pub fn retained_bytes_against(&self, newer: &CellStore) -> usize {
        use std::mem::size_of;

        let mut bytes = 0usize;
        for (key, band) in &self.bands {
            let next = newer.bands.get(key);
            if next.is_some_and(|b| Arc::ptr_eq(b, band)) {
                continue;
            }
            bytes = bytes.saturating_add(size_of::<Band>());
            for (row, cols) in &band.rows {
                bytes = bytes.saturating_add(size_of::<u32>() + 3 * size_of::<usize>());
                let next_row = next.and_then(|b| b.rows.get(row));
                for (col, cell) in cols {
                    // Cells and BTreeMap nodes are duplicated with a cloned band even when
                    // their formula/value Arcs are still shared with the next snapshot.
                    bytes = bytes.saturating_add(size_of::<u32>() + size_of::<Cell>() + 3 * size_of::<usize>());
                    let next_cell = next_row.and_then(|r| r.get(col));
                    if let Some(formula) = &cell.formula
                        && !next_cell.and_then(|c| c.formula.as_ref()).is_some_and(|f| Arc::ptr_eq(f, formula))
                    {
                        bytes = bytes.saturating_add(size_of::<crate::cell::Formula>() + formula.text.len());
                        if let Some(expr) = &formula.expr {
                            expr.walk(&mut |_| bytes = bytes.saturating_add(size_of::<gridcraft_formula::Expr>()));
                        }
                    }
                    match &cell.value {
                        gridcraft_core::Value::Text(text)
                            if !next_cell.and_then(|c| c.value.as_text()).is_some_and(|t| std::ptr::eq(t, text.as_ref())) =>
                        {
                            bytes = bytes.saturating_add(text.len());
                        }
                        gridcraft_core::Value::Array(array)
                            if !next_cell.is_some_and(|c| matches!(&c.value, gridcraft_core::Value::Array(a) if Arc::ptr_eq(a, array))) =>
                        {
                            bytes = bytes.saturating_add(
                                size_of::<gridcraft_core::Array>() + array.data.len().saturating_mul(size_of::<gridcraft_core::Value>()),
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        bytes
    }
    /// Removes every cell in `r` and returns them, row-major.
    pub fn take_range(&mut self, r: RangeRef) -> Vec<(CellRef, Cell)> {
        if r.end.col < gridcraft_core::MAX_COLS - 1 {
            // A block with columns to its right: cutting it out of each row map measured slower
            // than removing its cells one by one (#3575).
            let keys: Vec<CellRef> = self.iter_range(r).map(|(c, _)| c).collect();
            return keys.into_iter().filter_map(|c| self.remove(c).map(|cell| (c, cell))).collect();
        }
        let rows = self.take_rows(r.start.row..=r.end.row, r.start.col..=r.end.col);
        rows.into_iter().flat_map(|(row, cols)| cols.into_iter().map(move |(col, cell)| (CellRef::new(row, col), cell))).collect()
    }
    /// Rows strictly greater than or equal to `from` moved by `delta` (positive = down). Rows that
    /// would land before `from`... callers delete the vacated block first.
    pub fn shift_rows(&mut self, from: u32, delta: i64) {
        let moved = self.take_rows(from..=u32::MAX, 0..=u32::MAX);
        self.put_rows(moved.into_iter().filter_map(|(row, cols)| Some((shifted(row, delta, gridcraft_core::MAX_ROWS)?, cols))));
    }
    pub fn shift_cols(&mut self, from: u32, delta: i64) {
        let moved = self.take_rows(0..=u32::MAX, from..=u32::MAX);
        self.put_rows(moved.into_iter().map(|(row, cols)| (row, shift_keys(cols, delta))));
    }
    /// Deletes the rows in `spans` (`(first, count)`, ascending and disjoint) and closes the gaps
    /// in one pass: each later cell moves up once by the number of deleted rows above it.
    pub fn delete_rows(&mut self, spans: &[(u32, u32)]) {
        self.delete_lines(spans, true);
    }
    /// Column form of [`CellStore::delete_rows`].
    pub fn delete_cols(&mut self, spans: &[(u32, u32)]) {
        self.delete_lines(spans, false);
    }
    fn delete_lines(&mut self, spans: &[(u32, u32)], rows: bool) {
        let Some(&(first, _)) = spans.first() else { return };
        // Lines deleted before the end of each span, for mapping a kept line to its new place.
        let mut before = Vec::with_capacity(spans.len());
        let mut total = 0u32;
        for &(_, count) in spans {
            total = total.saturating_add(count);
            before.push(total);
        }
        let map = |v: u32| -> Option<u32> {
            let i = spans.partition_point(|&(at, _)| at <= v);
            let Some(i) = i.checked_sub(1) else { return Some(v) };
            let (at, count) = spans.get(i).copied().unwrap_or_default();
            if v - at < count { None } else { Some(v - before.get(i).copied().unwrap_or_default()) }
        };
        let keys: Vec<CellRef> = if rows {
            let r = RangeRef::new(CellRef::new(first, 0), CellRef::new(gridcraft_core::MAX_ROWS - 1, gridcraft_core::MAX_COLS - 1));
            self.iter_range(r).map(|(c, _)| c).collect()
        } else {
            self.iter().filter(|(c, _)| c.col >= first).map(|(c, _)| c).collect()
        };
        let moved: Vec<(CellRef, Cell)> = keys.into_iter().filter_map(|c| self.remove(c).map(|cell| (c, cell))).collect();
        for (c, cell) in moved {
            let to = if rows { map(c.row).map(|r| CellRef::new(r, c.col)) } else { map(c.col).map(|x| CellRef::new(c.row, x)) };
            if let Some(to) = to {
                self.set(to, cell);
            }
        }
    }
    /// Shifts cells within rows `r0..=r1` and columns `>= from` horizontally (Insert/Delete Cells
    /// with shift right/left).
    pub fn shift_cols_in_rows(&mut self, r0: u32, r1: u32, from: u32, delta: i64) {
        let r = RangeRef::new(CellRef::new(r0, from), CellRef::new(r1, gridcraft_core::MAX_COLS - 1));
        let moved = self.take_rows(r.start.row..=r.end.row, r.start.col..=r.end.col);
        self.put_rows(moved.into_iter().map(|(row, cols)| (row, shift_keys(cols, delta))));
    }
    /// Shifts cells within columns `c0..=c1` and rows `>= from` vertically.
    pub fn shift_rows_in_cols(&mut self, c0: u32, c1: u32, from: u32, delta: i64) {
        // Cell by cell: the moved cells sit between columns that stay, so moving row maps
        // measured no faster here (#3575).
        let range = RangeRef::new(CellRef::new(from, c0), CellRef::new(gridcraft_core::MAX_ROWS - 1, c1));
        for (c, cell) in self.take_range(range) {
            if let Some(nr) = shifted(c.row, delta, gridcraft_core::MAX_ROWS) {
                self.set(CellRef::new(nr, c.col), cell);
            }
        }
    }
    /// Removes the cells in `rows` x `cols` and returns them grouped by row, in row order. Moves
    /// whole column maps (or the split-off part of one) instead of removing cell by cell, and only
    /// copies bands that hold a cell in the block, so snapshots keep sharing the others.
    fn take_rows(&mut self, rows: RangeInclusive<u32>, cols: RangeInclusive<u32>) -> Vec<(u32, BTreeMap<u32, Cell>)> {
        let (r0, r1, c0, c1) = (*rows.start(), *rows.end(), *cols.start(), *cols.end());
        let mut out = Vec::new();
        if r0 > r1 || c0 > c1 {
            return out;
        }
        let whole_rows = c0 == 0 && c1 == u32::MAX;
        let keys: Vec<u32> = self
            .bands
            .range(r0 / BAND..=r1 / BAND)
            .filter(|(_, band)| band.rows.range(r0..=r1).any(|(_, row)| row.range(c0..=c1).next().is_some()))
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            let Some(band) = self.bands.get_mut(&key) else { continue };
            let inside = |row: Option<&u32>| row.is_some_and(|r| (r0..=r1).contains(r));
            if whole_rows && inside(band.rows.keys().next()) && inside(band.rows.keys().next_back()) {
                // The whole band moves: no splitting.
                let Some(band) = self.bands.remove(&key) else { continue };
                for (row, taken) in Arc::unwrap_or_clone(band).rows {
                    self.len -= taken.len();
                    out.push((row, taken));
                }
                continue;
            }
            let band = Arc::make_mut(band);
            let mut block = band.rows.split_off(&r0);
            let mut tail = match r1.checked_add(1) {
                Some(next) => block.split_off(&next),
                None => BTreeMap::new(),
            };
            for (row, mut kept) in block {
                let taken = if whole_rows {
                    std::mem::take(&mut kept)
                } else if c1.checked_add(1).is_none_or(|next| kept.range(next..).next().is_none()) {
                    // Nothing right of the block: one split.
                    kept.split_off(&c0)
                } else {
                    let mut taken = BTreeMap::new();
                    for (col, cell) in kept.extract_if(c0..=c1, |_, _| true) {
                        taken.insert(col, cell);
                    }
                    taken
                };
                if !kept.is_empty() {
                    band.rows.insert(row, kept);
                }
                if !taken.is_empty() {
                    self.len -= taken.len();
                    out.push((row, taken));
                }
            }
            band.rows.append(&mut tail);
            if band.rows.is_empty() {
                self.bands.remove(&key);
            }
        }
        out
    }
    /// Stores rows of cells (each row at most once) the way `set` stores each cell: a cell
    /// replaces what is there, and a blank cell clears its destination instead of being stored.
    /// A row with nothing stored yet takes the whole map. Rows in order share one band lookup.
    fn put_rows(&mut self, rows: impl Iterator<Item = (u32, BTreeMap<u32, Cell>)>) {
        let mut rows = rows.peekable();
        let mut group = Vec::new();
        while let Some((first, cols)) = rows.next() {
            let key = first / BAND;
            group.clear();
            group.push((first, cols));
            while let Some(next) = rows.next_if(|(row, _)| row / BAND == key) {
                group.push(next);
            }
            // Leave the band shared when only blank cells land on empty positions.
            let existing = self.bands.get(&key);
            let writes = group.iter().any(|(row, cols)| {
                let dest = existing.and_then(|band| band.rows.get(row));
                cols.iter().any(|(col, cell)| !cell.is_blank() || dest.is_some_and(|d| d.contains_key(col)))
            });
            if !writes {
                continue;
            }
            let band = Arc::make_mut(self.bands.entry(key).or_default());
            for (row, mut cols) in group.drain(..) {
                match band.rows.entry(row) {
                    Entry::Vacant(slot) => {
                        if cols.values().any(Cell::is_blank) {
                            cols.retain(|_, cell| !cell.is_blank());
                        }
                        if !cols.is_empty() {
                            self.len += cols.len();
                            slot.insert(cols);
                        }
                    }
                    Entry::Occupied(mut slot) => {
                        let dest = slot.get_mut();
                        for (col, cell) in cols {
                            if cell.is_blank() {
                                if dest.remove(&col).is_some() {
                                    self.len -= 1;
                                }
                            } else if dest.insert(col, cell).is_none() {
                                self.len += 1;
                            }
                        }
                        if dest.is_empty() {
                            slot.remove();
                        }
                    }
                }
            }
            if band.rows.is_empty() {
                self.bands.remove(&key);
            }
        }
    }
}

/// `at + delta` when it lies in `0..limit`.
fn shifted(at: u32, delta: i64, limit: u32) -> Option<u32> {
    let to = (at as i64).checked_add(delta)?;
    if (0..limit as i64).contains(&to) { Some(to as u32) } else { None }
}

/// The cells of one row with their columns moved by `delta`; cells that would leave the sheet are
/// dropped.
fn shift_keys(cols: BTreeMap<u32, Cell>, delta: i64) -> BTreeMap<u32, Cell> {
    let mut out = BTreeMap::new();
    for (col, cell) in cols {
        if let Some(c) = shifted(col, delta, gridcraft_core::MAX_COLS) {
            out.insert(c, cell);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_core::{MAX_COLS, MAX_ROWS, Value};

    fn c(r: u32, col: u32) -> CellRef {
        CellRef::new(r, col)
    }

    #[test]
    fn set_get_remove() -> Result<(), Box<dyn std::error::Error>> {
        let mut s = CellStore::new();
        s.set(c(0, 0), Cell::value(1.0.into()));
        s.set(c(100, 5), Cell::value("x".into()));
        assert_eq!(s.len(), 2);
        assert_eq!(s.get(c(100, 5)).map(|x| x.value.clone()), Some(Value::from("x")));
        assert_eq!(s.used_range().ok_or("none")?.a1(), "A1:F101");
        s.set(c(0, 0), Cell::default());
        assert_eq!(s.len(), 1);
        assert!(s.remove(c(100, 5)).is_some());
        assert!(s.is_empty());
        assert!(s.used_range().is_none());
        Ok(())
    }

    #[test]
    fn cow_snapshots() -> Result<(), Box<dyn std::error::Error>> {
        let mut s = CellStore::new();
        for r in 0..1000 {
            s.set(c(r, 0), Cell::value((r as f64).into()));
        }
        let snap = s.clone();
        s.set(c(5, 0), Cell::value(99.0.into()));
        assert_eq!(snap.get(c(5, 0)).ok_or("none")?.value, Value::Number(5.0));
        assert_eq!(s.get(c(5, 0)).ok_or("none")?.value, Value::Number(99.0));
        Ok(())
    }

    #[test]
    fn diff_visits_changed_bands() {
        let mut s = CellStore::new();
        for r in 0..1000 {
            s.set(c(r, 0), Cell::value((r as f64).into()));
        }
        let snap = s.clone();
        assert!(s.diff(&snap).is_empty());
        s.set(c(5, 0), Cell::value(99.0.into()));
        s.set(c(700, 3), Cell::value("new".into()));
        s.remove(c(900, 0));
        assert_eq!(snap.diff(&s), [c(5, 0), c(700, 3), c(900, 0)]);
    }

    #[test]
    fn range_iteration_order() -> Result<(), Box<dyn std::error::Error>> {
        let mut s = CellStore::new();
        s.set(c(70, 2), Cell::value(3.0.into()));
        s.set(c(1, 1), Cell::value(1.0.into()));
        s.set(c(1, 3), Cell::value(2.0.into()));
        s.set(c(500, 0), Cell::value(4.0.into()));
        let v: Vec<f64> =
            s.iter_range(RangeRef::parse("A1:D100").ok_or("none")?).map(|(_, x)| x.value.as_f64()).collect::<Option<_>>().ok_or("none")?;
        assert_eq!(v, [1.0, 2.0, 3.0]);
        Ok(())
    }

    #[test]
    fn shifting() {
        let mut s = CellStore::new();
        s.set(c(2, 0), Cell::value(1.0.into()));
        s.set(c(5, 0), Cell::value(2.0.into()));
        s.shift_rows(3, 2);
        assert!(s.get(c(7, 0)).is_some());
        assert!(s.get(c(2, 0)).is_some());
        s.shift_cols(0, 1);
        assert!(s.get(c(7, 1)).is_some());
    }

    /// The per-cell algorithms the shifts used before they moved whole rows (#3575), kept as the
    /// reference the structural versions must match cell for cell.
    mod per_cell {
        use super::*;

        fn reset(s: &mut CellStore, moved: Vec<(CellRef, Cell)>, to: impl Fn(CellRef) -> Option<CellRef>) {
            for (at, cell) in moved {
                if let Some(at) = to(at) {
                    s.set(at, cell);
                }
            }
        }
        fn row_by(delta: i64) -> impl Fn(CellRef) -> Option<CellRef> {
            move |at| {
                let r = at.row as i64 + delta;
                (0..MAX_ROWS as i64).contains(&r).then(|| CellRef::new(r as u32, at.col))
            }
        }
        fn col_by(delta: i64) -> impl Fn(CellRef) -> Option<CellRef> {
            move |at| {
                let c = at.col as i64 + delta;
                (0..MAX_COLS as i64).contains(&c).then(|| CellRef::new(at.row, c as u32))
            }
        }
        pub fn take_range(s: &mut CellStore, r: RangeRef) -> Vec<(CellRef, Cell)> {
            let keys: Vec<CellRef> = s.iter_range(r).map(|(c, _)| c).collect();
            keys.into_iter().filter_map(|c| s.remove(c).map(|cell| (c, cell))).collect()
        }
        pub fn shift_rows(s: &mut CellStore, from: u32, delta: i64) {
            let keys: Vec<CellRef> = s.iter().filter(|(c, _)| c.row >= from).map(|(c, _)| c).collect();
            let moved = keys.into_iter().filter_map(|c| s.remove(c).map(|cell| (c, cell))).collect();
            reset(s, moved, row_by(delta));
        }
        pub fn shift_cols(s: &mut CellStore, from: u32, delta: i64) {
            let keys: Vec<CellRef> = s.iter().filter(|(c, _)| c.col >= from).map(|(c, _)| c).collect();
            let moved = keys.into_iter().filter_map(|c| s.remove(c).map(|cell| (c, cell))).collect();
            reset(s, moved, col_by(delta));
        }
        pub fn shift_cols_in_rows(s: &mut CellStore, r0: u32, r1: u32, from: u32, delta: i64) {
            let moved = take_range(s, RangeRef::new(CellRef::new(r0, from), CellRef::new(r1, MAX_COLS - 1)));
            reset(s, moved, col_by(delta));
        }
        pub fn shift_rows_in_cols(s: &mut CellStore, c0: u32, c1: u32, from: u32, delta: i64) {
            let moved = take_range(s, RangeRef::new(CellRef::new(from, c0), CellRef::new(MAX_ROWS - 1, c1)));
            reset(s, moved, row_by(delta));
        }
    }

    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % n.max(1)
        }
        fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
            xs[self.below(xs.len() as u64) as usize]
        }
    }

    /// A position near a band edge, the sheet edge, or anywhere below `max`.
    fn coord(rng: &mut Lcg, max: u32, edges: &[u32]) -> u32 {
        match rng.below(3) {
            0 => rng.pick(edges).min(max - 1),
            1 => max - 1 - rng.below(70.min(max as u64)) as u32,
            _ => rng.below(max as u64) as u32,
        }
    }

    /// Random store with values, styled-only cells, a few cells off the grid, and blank cells left
    /// behind by `get_mut` (which `set` never stores, but the old shifts dropped and let clear
    /// their destination).
    fn random_store(rng: &mut Lcg, max_r: u32, max_c: u32) -> CellStore {
        let row_edges = [0, 1, 62, 63, 64, 65, 127, 128, 129, 191, 192, MAX_ROWS - 64, MAX_ROWS - 1];
        let col_edges = [0, 1, 2, 63, 64, 65, MAX_COLS - 2, MAX_COLS - 1];
        let mut s = CellStore::new();
        for _ in 0..rng.below(300) {
            let mut at = CellRef::new(coord(rng, max_r, &row_edges), coord(rng, max_c, &col_edges));
            // Now and then a cell outside the grid (`set` does not check): the shifts must treat
            // it as the old code did.
            match rng.below(60) {
                0 => at.row = MAX_ROWS + rng.below(3) as u32,
                1 => at.col = MAX_COLS + rng.below(3) as u32,
                _ => {}
            }
            let cell = match rng.below(5) {
                0 => Cell { style: crate::style::StyleId(1 + rng.below(3) as u32), ..Cell::default() },
                1 => Cell::value(Value::from("t")),
                _ => Cell::value(Value::Number(rng.below(1000) as f64)),
            };
            s.set(at, cell);
            if rng.below(8) == 0
                && let Some(cell) = s.get_mut(at)
            {
                *cell = Cell::default();
            }
        }
        s
    }

    fn assert_same(new: &CellStore, old: &CellStore, base: &CellStore, base_copy: &CellStore, what: &str) {
        assert_eq!(new, old, "{what}");
        assert_eq!(new.len(), new.iter().count(), "{what}: len");
        assert!(new.iter().map(|(c, x)| (c, x.clone())).eq(old.iter().map(|(c, x)| (c, x.clone()))), "{what}: iteration");
        assert!(new.bands.values().all(|b| !b.rows.is_empty() && b.rows.values().all(|r| !r.is_empty())), "{what}: empty band or row");
        // Copy-on-write: every band the old code left shared with the snapshot stays shared.
        for (k, b) in &base.bands {
            if old.bands.get(k).is_some_and(|o| Arc::ptr_eq(o, b)) {
                assert!(new.bands.get(k).is_some_and(|n| Arc::ptr_eq(n, b)), "{what}: band {k} copied");
            }
        }
        assert_eq!(base, base_copy, "{what}: snapshot changed");
    }

    #[test]
    fn structural_shifts_match_per_cell_shifts() {
        let mut rng = Lcg(3575);
        let deltas = [0i64, 1, -1, 2, -2, 63, -63, 64, -64, 65, -65, 128, -128, 1000, -1000, MAX_ROWS as i64, -(MAX_ROWS as i64)];
        let row_edges = [0, 1, 63, 64, 65, 127, 128, MAX_ROWS - 1, MAX_ROWS];
        let col_edges = [0, 1, 63, 64, 65, MAX_COLS - 1, MAX_COLS];
        for trial in 0..3000 {
            let max_r = [200, 5000, MAX_ROWS][trial % 3];
            let max_c = [30, MAX_COLS][trial / 3 % 2];
            let mut base = random_store(&mut rng, max_r, max_c);
            for step in 0..4 {
                let snapshot = base.clone();
                let (mut new, mut old) = (base.clone(), base.clone());
                let delta = if rng.below(2) == 0 { rng.pick(&deltas) } else { rng.below(301) as i64 - 150 };
                let row = if rng.below(2) == 0 { rng.pick(&row_edges) } else { rng.below(max_r as u64 + 1) as u32 };
                let col = if rng.below(2) == 0 { rng.pick(&col_edges) } else { rng.below(max_c as u64 + 1) as u32 };
                let (row2, col2) = (rng.below(max_r as u64 + 1) as u32, rng.below(max_c as u64 + 1) as u32);
                let op = rng.below(5);
                match op {
                    0 => {
                        new.shift_rows(row, delta);
                        per_cell::shift_rows(&mut old, row, delta);
                    }
                    1 => {
                        new.shift_cols(col, delta);
                        per_cell::shift_cols(&mut old, col, delta);
                    }
                    2 => {
                        new.shift_cols_in_rows(row, row2, col, delta);
                        per_cell::shift_cols_in_rows(&mut old, row, row2, col, delta);
                    }
                    3 => {
                        new.shift_rows_in_cols(col, col2, row, delta);
                        per_cell::shift_rows_in_cols(&mut old, col, col2, row, delta);
                    }
                    _ => {
                        let r = RangeRef::new(CellRef::new(row, col), CellRef::new(row2, col2));
                        assert_eq!(new.take_range(r), per_cell::take_range(&mut old, r), "trial {trial} step {step}: taken cells");
                    }
                }
                let what = format!("trial {trial} step {step} op {op} row {row}..{row2} col {col}..{col2} delta {delta}");
                assert_same(&new, &old, &base, &snapshot, &what);
                base = new;
            }
        }
    }

    #[test]
    fn off_grid_cells_right_of_a_block_stay_put() {
        let mut s = CellStore::new();
        for col in [0, 5, MAX_COLS - 1, MAX_COLS + 1] {
            s.set(c(3, col), Cell::value((col as f64).into()));
        }
        let mut old = s.clone();
        s.shift_cols_in_rows(3, 3, 0, 1);
        per_cell::shift_cols_in_rows(&mut old, 3, 3, 0, 1);
        assert_eq!(s, old);
        assert_eq!(s.get(c(3, MAX_COLS + 1)).map(|x| x.value.clone()), Some(Value::Number((MAX_COLS + 1) as f64)));
        let r = RangeRef::rows(3, 3);
        assert_eq!(s.take_range(r), per_cell::take_range(&mut old, r));
        assert_eq!(s, old);
    }

    #[test]
    fn moved_blank_cell_clears_its_destination() {
        let mut s = CellStore::new();
        s.set(c(3, 0), Cell::value(1.0.into()));
        s.set(c(5, 0), Cell::value(2.0.into()));
        s.set(c(5, 1), Cell::value(3.0.into()));
        if let Some(cell) = s.get_mut(c(5, 0)) {
            *cell = Cell::default();
        }
        s.shift_rows(5, -2);
        assert!(s.get(c(3, 0)).is_none());
        assert_eq!(s.get(c(3, 1)).map(|x| x.value.clone()), Some(Value::Number(3.0)));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn shifting_keeps_untouched_bands_shared() {
        let mut s = CellStore::new();
        for r in 0..256 {
            s.set(c(r, 0), Cell::value((r as f64).into()));
        }
        s.set(c(200, 5), Cell::value("x".into()));
        let snap = s.clone();
        s.shift_rows(130, 1);
        assert!(Arc::ptr_eq(&s.bands[&0], &snap.bands[&0]));
        assert!(Arc::ptr_eq(&s.bands[&1], &snap.bands[&1]));
        s.shift_cols(5, 1);
        assert!(Arc::ptr_eq(&s.bands[&0], &snap.bands[&0]));
        assert_eq!(s.get(c(201, 6)).map(|x| x.value.clone()), Some(Value::from("x")));
    }
}
