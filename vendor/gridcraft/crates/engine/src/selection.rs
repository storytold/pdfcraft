//! Cell selection: one or more ranges with an active cell.

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use gridcraft_model::Sheet;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// The active (white) cell.
    pub active: CellRef,
    /// Where a Shift-extension is anchored.
    pub anchor: CellRef,
    /// Selected areas; the last one contains the active cell. Never empty.
    pub ranges: Vec<RangeRef>,
}

impl Default for Selection {
    fn default() -> Self {
        Selection::at(CellRef::default())
    }
}

impl Selection {
    pub fn at(c: CellRef) -> Selection {
        Selection { active: c, anchor: c, ranges: vec![RangeRef::cell(c)] }
    }
    pub fn range(r: RangeRef) -> Selection {
        Selection { active: r.start, anchor: r.start, ranges: vec![r] }
    }
    /// The current (last) area.
    pub fn current(&self) -> RangeRef {
        self.ranges.last().copied().unwrap_or(RangeRef::cell(self.active))
    }
    /// Bounding range of all areas.
    pub fn bounds(&self) -> RangeRef {
        let mut it = self.ranges.iter();
        let first = it.next().copied().unwrap_or(RangeRef::cell(self.active));
        it.fold(first, |a, b| a.union(b))
    }
    pub fn contains(&self, c: CellRef) -> bool {
        self.ranges.iter().any(|r| r.contains(c))
    }
    pub fn is_single_cell(&self) -> bool {
        self.ranges.len() == 1 && self.current().is_single()
    }
    /// Whether the selection is exactly one merged cell, which Enter and Tab treat as one cell.
    pub fn is_one_merged_cell(&self, sheet: &Sheet) -> bool {
        self.ranges.len() == 1 && sheet.merge_at(self.active).is_some_and(|m| m == self.current())
    }
    /// The corner of the current area opposite the anchor: the end that Shift+arrows move.
    pub fn moving_end(&self) -> CellRef {
        let r = self.current();
        let row = if self.anchor.row == r.start.row { r.end.row } else { r.start.row };
        let col = if self.anchor.col == r.start.col { r.end.col } else { r.start.col };
        CellRef::new(row, col)
    }
    /// Extends the current area from the anchor to `to` (Shift+click / Shift+arrows).
    pub fn extend_to(&mut self, to: CellRef) {
        let r = RangeRef::new(self.anchor, to);
        if let Some(last) = self.ranges.last_mut() {
            *last = r;
        } else {
            self.ranges.push(r);
        }
    }
    /// The A1 text of all areas (`A1:B2,D4`).
    pub fn a1(&self) -> String {
        self.ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(",")
    }
    /// Expands areas so merged cells are included whole. Whole columns and whole rows stay as
    /// they are: clicking column B's header selects just B even when B1:C1 is merged, as in Excel.
    pub fn expand_merges(&mut self, sheet: &Sheet) {
        for r in self.ranges.iter_mut() {
            *r = with_merges(sheet, *r);
        }
    }
    /// Shift+arrows: moves the edge of the current area that the anchor doesn't hold by `dr`
    /// rows and `dc` columns, keeping merged cells whole, as Excel does. The anchor is the whole
    /// merged cell it is in, so from a selected merged cell Shift+Left and Shift+Up widen the
    /// selection past its left and top edges. A step that would shrink the area onto the inside
    /// of a merged cell (where keeping it whole would undo the step) goes on to the next line
    /// that does shrink it, and when the area can't shrink any more it grows in that direction.
    pub fn extend_by(&mut self, sheet: &Sheet, dr: i64, dc: i64) {
        let anchor = sheet.merge_at(self.anchor).unwrap_or(RangeRef::cell(self.anchor));
        let mut r = self.current();
        if dc != 0 {
            r = step_edge(sheet, r, anchor, dc, true);
        }
        if dr != 0 {
            r = step_edge(sheet, r, anchor, dr, false);
        }
        if let Some(last) = self.ranges.last_mut() {
            *last = r;
        } else {
            self.ranges.push(r);
        }
    }
    /// Moves the active cell within the selection (Enter/Tab inside a multi-cell selection).
    pub fn cycle(&mut self, forward: bool, by_row: bool) {
        let r = self.current();
        if r.is_single() && self.ranges.len() == 1 {
            return;
        }
        let a = self.active;
        let (mut row, mut col) = (a.row as i64, a.col as i64);
        let step: i64 = if forward { 1 } else { -1 };
        if by_row {
            col += step;
            if col > r.end.col as i64 {
                col = r.start.col as i64;
                row += 1;
            } else if col < r.start.col as i64 {
                col = r.end.col as i64;
                row -= 1;
            }
            if row > r.end.row as i64 {
                row = r.start.row as i64;
            } else if row < r.start.row as i64 {
                row = r.end.row as i64;
            }
        } else {
            row += step;
            if row > r.end.row as i64 {
                row = r.start.row as i64;
                col += 1;
            } else if row < r.start.row as i64 {
                row = r.end.row as i64;
                col -= 1;
            }
            if col > r.end.col as i64 {
                col = r.start.col as i64;
            } else if col < r.start.col as i64 {
                col = r.end.col as i64;
            }
        }
        self.active = CellRef::new(row.clamp(0, MAX_ROWS as i64 - 1) as u32, col.clamp(0, MAX_COLS as i64 - 1) as u32);
    }
}

/// `r` grown until every merged cell it touches is inside it. Whole columns and whole rows stay
/// as they are.
fn with_merges(sheet: &Sheet, r: RangeRef) -> RangeRef {
    if r.is_full_cols() || r.is_full_rows() {
        return r;
    }
    let mut cur = r;
    loop {
        let mut grown = cur;
        for m in &sheet.merges {
            if m.intersects(&grown) {
                grown = grown.union(m);
            }
        }
        if grown == cur {
            return cur;
        }
        cur = grown;
    }
}

/// One Shift+arrow step of [`Selection::extend_by`] along one axis (`cols`: columns, else rows).
fn step_edge(sheet: &Sheet, r: RangeRef, anchor: RangeRef, d: i64, cols: bool) -> RangeRef {
    let max = i64::from(if cols { MAX_COLS } else { MAX_ROWS }) - 1;
    let hidden = |i: i64| if cols { sheet.is_col_hidden(i as u32) } else { sheet.is_row_hidden(i as u32) };
    // `from` moved by `by`, then on past hidden lines in the same direction (not off the sheet).
    let land = |from: i64, by: i64| {
        let mut t = from.saturating_add(by).clamp(0, max);
        // Bounded like the arrow keys' walk over hidden lines.
        for _ in 0..100_000 {
            if !hidden(t) {
                break;
            }
            let n = (t + by.signum()).clamp(0, max);
            if n == t {
                break;
            }
            t = n;
        }
        t
    };
    let span = |x: &RangeRef| {
        if cols { (i64::from(x.start.col), i64::from(x.end.col)) } else { (i64::from(x.start.row), i64::from(x.end.row)) }
    };
    let with = |lo: i64, hi: i64| {
        let (lo, hi) = (lo.clamp(0, max) as u32, hi.clamp(0, max) as u32);
        let (s, e) = if cols {
            (CellRef::new(r.start.row, lo), CellRef::new(r.end.row, hi))
        } else {
            (CellRef::new(lo, r.start.col), CellRef::new(hi, r.end.col))
        };
        with_merges(sheet, RangeRef::new(s, e))
    };
    let (lo, hi) = span(&r);
    let (alo, ahi) = span(&anchor);
    // The anchor holds the low edge unless the area reaches past it on that side.
    let moving_hi = lo >= alo;
    let shrinking = if moving_hi { d < 0 && hi > ahi } else { d > 0 && lo < alo };
    if shrinking {
        let mut t = if moving_hi { hi } else { lo };
        let mut by = d;
        loop {
            t = if moving_hi { land(t, by).max(ahi) } else { land(t, by).min(alo) };
            let cand = if moving_hi { with(lo, t) } else { with(t, hi) };
            if cand != r {
                return cand;
            }
            if t == if moving_hi { ahi } else { alo } {
                break;
            }
            // The step was undone because a merged cell crosses the new edge, and so is every
            // edge up to where that merged cell starts (ends, for the low edge): go on from there
            // so a tall or wide merged cell costs one step, not one per line it covers.
            let across = sheet.merges.iter().filter(|m| m.intersects(&r)).filter_map(|m| {
                let (ms, me) = span(m);
                if moving_hi { (ms <= t && me > t).then_some(ms) } else { (ms < t && me >= t).then_some(me) }
            });
            t = if moving_hi { across.min().map_or(t, |ms| ms.min(t)) } else { across.max().map_or(t, |me| me.max(t)) };
            by = d.signum();
        }
    }
    // Grow the area on the side the arrow points to.
    if d > 0 { with(lo, land(hi, d)) } else { with(land(lo, d), hi) }
}

impl Selection {
    /// Ctrl+. (Next Corner of Selection): the active cell moves clockwise to the next corner of
    /// the current area (top-left, top-right, bottom-right, bottom-left) and becomes the anchor,
    /// so Shift+arrows then move the opposite corner. The area stays selected. From a cell that
    /// isn't a corner the next corner is the top-left one; a single cell stays where it is.
    pub fn next_corner(&mut self) {
        self.next_corner_by(Some);
    }

    /// [`Selection::next_corner`] where `stop` says where each corner takes the active cell: the
    /// top-left cell of a merged cell the corner is in, or `None` for a corner that can't be
    /// selected (it is skipped). Corners that end up on the active cell are one stop with it.
    pub fn next_corner_by(&mut self, stop: impl Fn(CellRef) -> Option<CellRef>) {
        let r = self.current();
        let corners = [r.start, CellRef::new(r.start.row, r.end.col), r.end, CellRef::new(r.end.row, r.start.col)];
        let next = match corners.iter().rposition(|c| stop(*c) == Some(self.active)) {
            Some(i) => (1..=4).filter_map(|k| corners.get((i + k) % 4)).filter_map(|c| stop(*c)).find(|c| *c != self.active),
            None => corners.iter().find_map(|c| stop(*c)),
        };
        if let Some(next) = next {
            self.active = next;
            self.anchor = next;
        }
    }
}

/// Ctrl+arrow: jump to the edge of the current data region (Excel's End mode).
pub fn jump(sheet: &Sheet, from: CellRef, dr: i64, dc: i64) -> CellRef {
    if dr == 0 && dc == 0 {
        // No direction: nowhere to go (the search below would never advance).
        return from;
    }
    let filled = |c: CellRef| sheet.value_ref(c).is_some_and(|v| !v.is_empty());
    let Some(mut cur) = from.offset(dr, dc) else { return from };
    let here = filled(from);
    let next = filled(cur);
    if here && next {
        // Move to the last filled cell of the run.
        while let Some(n) = cur.offset(dr, dc) {
            if !filled(n) {
                break;
            }
            cur = n;
        }
        return cur;
    }
    // Move to the next filled cell, or the sheet edge. Use the used range to avoid walking a
    // million empty cells.
    let used = sheet.used_range();
    loop {
        if filled(cur) {
            return cur;
        }
        let beyond = match used {
            Some(u) => {
                (dr > 0 && cur.row > u.end.row)
                    || (dc > 0 && cur.col > u.end.col)
                    || (dr < 0 && cur.row < u.start.row)
                    || (dc < 0 && cur.col < u.start.col)
            }
            None => true,
        };
        if beyond {
            let r = if dr > 0 {
                MAX_ROWS - 1
            } else if dr < 0 {
                0
            } else {
                cur.row
            };
            let c = if dc > 0 {
                MAX_COLS - 1
            } else if dc < 0 {
                0
            } else {
                cur.col
            };
            return CellRef::new(r, c);
        }
        match cur.offset(dr, dc) {
            Some(n) => cur = n,
            None => return cur,
        }
    }
}

/// The contiguous data region around `c` (Ctrl+A / Ctrl+Shift+8, AutoSum, sort, tables).
pub fn current_region(sheet: &Sheet, c: CellRef) -> RangeRef {
    let filled = |c: CellRef| sheet.cell(c).is_some_and(gridcraft_model::Cell::has_content) || sheet.spill.contains_key(&c);
    // Whether a row (or column) has a filled cell between two columns (rows), inclusive.
    let row_has = |row: u32, a: u32, b: u32| (a..=b).any(|col| filled(CellRef::new(row, col)));
    let col_has = |col: u32, a: u32, b: u32| (a..=b).any(|row| filled(CellRef::new(row, col)));
    let (mut r0, mut r1, mut c0, mut c1) = (c.row, c.row, c.col, c.col);
    // Grow one side at a time while the line next to it (diagonals included) has data. Each
    // pass scans only the lines next to the region, so tall regions take linear time.
    loop {
        let before = (r0, r1, c0, c1);
        let (lo_c, hi_c) = (c0.saturating_sub(1), (c1 + 1).min(MAX_COLS - 1));
        while r0 > 0 && row_has(r0 - 1, lo_c, hi_c) {
            r0 -= 1;
        }
        while r1 + 1 < MAX_ROWS && row_has(r1 + 1, lo_c, hi_c) {
            r1 += 1;
        }
        let (lo_r, hi_r) = (r0.saturating_sub(1), (r1 + 1).min(MAX_ROWS - 1));
        while c0 > 0 && col_has(c0 - 1, lo_r, hi_r) {
            c0 -= 1;
        }
        while c1 + 1 < MAX_COLS && col_has(c1 + 1, lo_r, hi_r) {
            c1 += 1;
        }
        if (r0, r1, c0, c1) == before {
            break;
        }
    }
    RangeRef::new(CellRef::new(r0, c0), CellRef::new(r1, c1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_core::Value;

    #[test]
    fn jumps() {
        let mut s = Sheet::new("S");
        for r in 0..5 {
            s.set_value(CellRef::new(r, 0), Value::Number(1.0));
        }
        s.set_value(CellRef::new(10, 0), Value::Number(1.0));
        assert_eq!(jump(&s, CellRef::new(0, 0), 1, 0), CellRef::new(4, 0));
        // Without a direction it stays put (it used to loop forever from a blank cell).
        assert_eq!(jump(&s, CellRef::new(7, 0), 0, 0), CellRef::new(7, 0));
        assert_eq!(jump(&s, CellRef::new(4, 0), 1, 0), CellRef::new(10, 0));
        assert_eq!(jump(&s, CellRef::new(10, 0), 1, 0), CellRef::new(MAX_ROWS - 1, 0));
        assert_eq!(jump(&s, CellRef::new(0, 0), 0, 1), CellRef::new(0, MAX_COLS - 1));
        assert_eq!(jump(&s, CellRef::new(0, 0), -1, 0), CellRef::new(0, 0));
    }

    #[test]
    fn regions() {
        let mut s = Sheet::new("S");
        for r in 2..5 {
            for c in 1..4 {
                s.set_value(CellRef::new(r, c), Value::Number(1.0));
            }
        }
        assert_eq!(current_region(&s, CellRef::new(3, 2)).a1(), "B3:D5");
        assert_eq!(current_region(&s, CellRef::new(5, 4)).a1(), "B3:E6");
        assert_eq!(current_region(&s, CellRef::new(20, 20)).a1(), "U21");
        // Taller than 10,000 rows: it used to stop growing after 10,000 steps.
        let mut s = Sheet::new("S");
        for r in 0..20_000 {
            s.set_value(CellRef::new(r, 0), Value::Number(r as f64));
            s.set_value(CellRef::new(r, 1), Value::Number(1.0));
        }
        assert_eq!(current_region(&s, CellRef::new(5, 0)).a1(), "A1:B20000");
    }

    #[test]
    fn shift_arrows_from_a_merged_cell() {
        let mut s = Sheet::new("S");
        s.merges.push(RangeRef::parse("B2:C3").unwrap());
        let at = |s: &Sheet, a1: &str| {
            let mut sel = Selection::at(CellRef::parse(a1).unwrap());
            sel.expand_merges(s);
            sel
        };
        for ((dr, dc), want) in [((0, -1), "A2:C3"), ((-1, 0), "B1:C3"), ((0, 1), "B2:D3"), ((1, 0), "B2:C4")] {
            let mut sel = at(&s, "B2");
            sel.extend_by(&s, dr, dc);
            assert_eq!(sel.a1(), want);
        }
        // Shrinking onto the inside of the merged cell goes on past it.
        let mut sel = at(&s, "A1");
        sel.extend_by(&s, 1, 1);
        assert_eq!(sel.a1(), "A1:C3");
        sel.extend_by(&s, 0, -1);
        assert_eq!(sel.a1(), "A1:A3");
        // A hidden column is stepped over, and the sheet edge stops the area.
        s.cols.entry(3).or_default().hidden = true;
        let mut sel = at(&s, "B2");
        sel.extend_by(&s, 0, 1);
        assert_eq!(sel.a1(), "B2:E3");
        let mut sel = at(&s, "A1");
        sel.extend_by(&s, -1, -1);
        assert_eq!(sel.a1(), "A1");
    }

    #[test]
    fn cycling() {
        let mut sel = Selection::range(RangeRef::parse("A1:B2").unwrap());
        sel.cycle(true, false);
        assert_eq!(sel.active.a1(), "A2");
        sel.cycle(true, false);
        assert_eq!(sel.active.a1(), "B1");
        sel.cycle(true, true);
        assert_eq!(sel.active.a1(), "A2");
    }
}
