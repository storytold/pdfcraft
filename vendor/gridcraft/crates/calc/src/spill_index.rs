//! A spatial index over spill areas: which areas cover a cell, and which meet a range, without
//! looking at every area. Areas are registered in 16 x 16 cell blocks, so a lookup touches one
//! block (a cell) or the blocks a range covers (a range) rather than every spill on the sheet.
//! An area that would take more than `BIG_AREA_BLOCKS` blocks (a full-height column spill takes
//! 65,536) is not registered in blocks but kept in a short list that every lookup tests directly,
//! so registering costs what the area's size warrants only for small areas.

use crate::recalc::{FxHasher, Key};
use gridcraft_core::{CellRef, RangeRef};
use std::hash::BuildHasherDefault;

type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;

/// Block edge in cells (rows and columns).
const BLOCK: u32 = 16;

/// Areas spanning more blocks than this are tested directly instead of being registered.
const BIG_AREA_BLOCKS: u64 = 1024;

/// One indexed area: its rectangle, and whether it covers no cells (an array with no rows or no
/// columns still meets the ranges its anchor is in, as a 1 x 1 area does for taint tracking).
#[derive(Clone, Copy)]
struct Entry {
    rect: RangeRef,
    empty: bool,
}

#[derive(Default)]
struct SheetAreas {
    entries: HashMap<CellRef, Entry>,
    blocks: HashMap<(u32, u32), Vec<CellRef>>,
    /// Anchors of the areas too big to register in blocks.
    big: Vec<CellRef>,
}

/// What covers a cell.
pub(crate) enum Cover {
    None,
    One(CellRef),
    /// Several areas overlap here: the caller decides which one counts.
    Many,
}

#[derive(Default)]
pub(crate) struct SpillIndex {
    sheets: HashMap<usize, SheetAreas>,
}

fn block_span(rect: &RangeRef) -> (u32, u32, u32, u32) {
    (rect.start.row / BLOCK, rect.end.row / BLOCK, rect.start.col / BLOCK, rect.end.col / BLOCK)
}

fn block_count(rect: &RangeRef) -> u64 {
    let (r0, r1, c0, c1) = block_span(rect);
    u64::from(r1 - r0 + 1) * u64::from(c1 - c0 + 1)
}

/// Whether registering `areas` in blocks costs less than checking each of them against each of
/// `lookups` cells would: a few block entries cost about one containment check each. A tall or
/// wide area is not registered at all (see `BIG_AREA_BLOCKS`) and so costs nothing here.
pub(crate) fn indexing_pays<'a>(areas: impl ExactSizeIterator<Item = &'a RangeRef>, lookups: usize) -> bool {
    const BLOCK_COST: u64 = 16;
    let budget = (lookups as u64).saturating_mul(areas.len() as u64) / BLOCK_COST;
    let mut blocks = 0u64;
    for r in areas {
        let n = block_count(r);
        if n > BIG_AREA_BLOCKS {
            continue;
        }
        blocks = blocks.saturating_add(n);
        if blocks > budget {
            return false;
        }
    }
    true
}

impl SpillIndex {
    /// Whether any area is indexed on `sheet`.
    #[cfg(test)]
    pub(crate) fn has_sheet(&self, sheet: usize) -> bool {
        self.sheets.get(&sheet).is_some_and(|s| !s.entries.is_empty())
    }

    /// Indexes the area an array of `rows` x `cols` anchored at `anchor` covers (replacing the
    /// one already indexed for that anchor).
    pub(crate) fn insert(&mut self, (sheet, anchor): Key, rows: usize, cols: usize) {
        let end =
            CellRef::new(anchor.row.saturating_add((rows as u32).saturating_sub(1)), anchor.col.saturating_add((cols as u32).saturating_sub(1)));
        self.insert_rect((sheet, anchor), RangeRef::new(anchor, end), rows == 0 || cols == 0);
    }

    /// Indexes an area given as a range.
    pub(crate) fn insert_rect(&mut self, (sheet, anchor): Key, rect: RangeRef, empty: bool) {
        self.remove((sheet, anchor));
        let areas = self.sheets.entry(sheet).or_default();
        if block_count(&rect) > BIG_AREA_BLOCKS {
            areas.big.push(anchor);
        } else {
            let (r0, r1, c0, c1) = block_span(&rect);
            for br in r0..=r1 {
                for bc in c0..=c1 {
                    areas.blocks.entry((br, bc)).or_default().push(anchor);
                }
            }
        }
        areas.entries.insert(anchor, Entry { rect, empty });
    }

    pub(crate) fn remove(&mut self, (sheet, anchor): Key) {
        let Some(areas) = self.sheets.get_mut(&sheet) else { return };
        let Some(old) = areas.entries.remove(&anchor) else { return };
        if block_count(&old.rect) > BIG_AREA_BLOCKS {
            areas.big.retain(|a| *a != anchor);
            return;
        }
        let (r0, r1, c0, c1) = block_span(&old.rect);
        for br in r0..=r1 {
            for bc in c0..=c1 {
                if let Some(list) = areas.blocks.get_mut(&(br, bc)) {
                    list.retain(|a| *a != anchor);
                    if list.is_empty() {
                        areas.blocks.remove(&(br, bc));
                    }
                }
            }
        }
    }

    /// The areas that cover `c`.
    pub(crate) fn covering(&self, sheet: usize, c: CellRef) -> Cover {
        let Some(areas) = self.sheets.get(&sheet) else { return Cover::None };
        let in_block = areas.blocks.get(&(c.row / BLOCK, c.col / BLOCK)).map_or(&[][..], |l| l.as_slice());
        let mut found = Cover::None;
        for a in in_block.iter().chain(&areas.big) {
            if let Some(e) = areas.entries.get(a)
                && !e.empty
                && e.rect.contains(c)
            {
                found = match found {
                    Cover::None => Cover::One(*a),
                    _ => return Cover::Many,
                };
            }
        }
        found
    }

    /// The anchors of the areas whose rectangle meets `range` (an empty area counts through its
    /// anchor cell), sorted by anchor.
    pub(crate) fn meeting(&self, sheet: usize, range: RangeRef, out: &mut Vec<CellRef>) {
        out.clear();
        let Some(areas) = self.sheets.get(&sheet) else { return };
        let (r0, r1, c0, c1) = block_span(&range);
        let touched = u64::from(r1 - r0 + 1) * u64::from(c1 - c0 + 1);
        if touched > 4 * areas.entries.len() as u64 + 16 {
            // A range wider than the areas are many: look at each area once.
            out.extend(areas.entries.iter().filter(|(_, e)| e.rect.intersects(&range)).map(|(a, _)| *a));
        } else {
            for br in r0..=r1 {
                for bc in c0..=c1 {
                    if let Some(list) = areas.blocks.get(&(br, bc)) {
                        out.extend(list.iter().copied().filter(|a| areas.entries.get(a).is_some_and(|e| e.rect.intersects(&range))));
                    }
                }
            }
            out.extend(areas.big.iter().copied().filter(|a| areas.entries.get(a).is_some_and(|e| e.rect.intersects(&range))));
        }
        out.sort_unstable();
        out.dedup();
    }

    /// The rectangle indexed for `anchor` and whether it covers no cells.
    pub(crate) fn area(&self, sheet: usize, anchor: CellRef) -> Option<(RangeRef, bool)> {
        self.sheets.get(&sheet)?.entries.get(&anchor).map(|e| (e.rect, e.empty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(r: u32, c: u32) -> CellRef {
        CellRef::new(r, c)
    }

    #[test]
    fn finds_the_areas_covering_a_cell_and_meeting_a_range() {
        let mut ix = SpillIndex::default();
        ix.insert((0, at(0, 0)), 40, 3);
        ix.insert((0, at(30, 1)), 1, 40);
        ix.insert((1, at(0, 0)), 5, 5);
        assert!(matches!(ix.covering(0, at(39, 2)), Cover::One(a) if a == at(0, 0)));
        assert!(matches!(ix.covering(0, at(30, 2)), Cover::Many));
        assert!(matches!(ix.covering(0, at(40, 0)), Cover::None));
        assert!(matches!(ix.covering(2, at(0, 0)), Cover::None));
        let mut hits = Vec::new();
        ix.meeting(0, RangeRef::new(at(30, 30), at(50, 60)), &mut hits);
        assert_eq!(hits, vec![at(30, 1)]);
        ix.meeting(0, RangeRef::new(at(0, 0), at(1_000_000, 16_000)), &mut hits);
        assert_eq!(hits, vec![at(0, 0), at(30, 1)]);
        ix.remove((0, at(30, 1)));
        assert!(matches!(ix.covering(0, at(30, 2)), Cover::One(_)));
        assert!(ix.has_sheet(1) && !ix.has_sheet(2));
        ix.insert((0, at(0, 0)), 1, 1);
        assert!(matches!(ix.covering(0, at(39, 2)), Cover::None));
    }

    #[test]
    fn tall_or_wide_areas_are_not_registered_in_blocks() {
        let column = RangeRef::new(at(0, 0), at(1_048_575, 0));
        assert!(block_count(&column) > BIG_AREA_BLOCKS);
        let mut ix = SpillIndex::default();
        ix.insert_rect((0, at(0, 0)), column, false);
        assert!(ix.sheets[&0].blocks.is_empty());
        assert!(matches!(ix.covering(0, at(900_000, 0)), Cover::One(a) if a == at(0, 0)));
        assert!(matches!(ix.covering(0, at(900_000, 1)), Cover::None));
        let mut hits = Vec::new();
        ix.meeting(0, RangeRef::new(at(5, 0), at(6, 3)), &mut hits);
        assert_eq!(hits, vec![at(0, 0)]);
        ix.remove((0, at(0, 0)));
        assert!(matches!(ix.covering(0, at(900_000, 0)), Cover::None) && ix.sheets[&0].big.is_empty());
        // Registering a big area costs nothing (lookups test it directly), so a small paste still indexes next to many small areas.
        assert!(indexing_pays([column].iter(), 17));
        // Many small areas pay back for a paste of a hundred cells, but not for a handful.
        let small: Vec<RangeRef> = (0..1_000).map(|i| RangeRef::new(at(i * 3, 0), at(i * 3 + 2, 0))).collect();
        assert!(indexing_pays(small.iter(), 100));
        assert!(!indexing_pays(small.iter(), 17));
    }

    /// The index against the old way: test every area for every lookup. Mixed shapes: small,
    /// thin and tall, wide, big (kept out of the blocks), empty, on two sheets, replaced and removed.
    #[test]
    fn matches_a_scan_of_every_area_on_mixed_shapes() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as u32 % n
        };
        let mut ix = SpillIndex::default();
        let mut model: std::collections::BTreeMap<(usize, CellRef), (RangeRef, bool)> = Default::default();
        for step in 0..3_000 {
            let sheet = next(2) as usize;
            let anchor = at(next(400) * 5, next(40) * 3);
            if next(4) == 0 {
                ix.remove((sheet, anchor));
                model.remove(&(sheet, anchor));
            } else {
                let (rows, cols) = match next(8) {
                    0 => (0, 1 + next(5) as usize),
                    1 => (1 + next(5) as usize, 0),
                    2 => (5_000 + next(100_000) as usize, 1),
                    3 => (1, 1 + next(40_000) as usize),
                    4 => (200 + next(300) as usize, 40 + next(40) as usize),
                    _ => (1 + next(60) as usize, 1 + next(40) as usize),
                };
                ix.insert((sheet, anchor), rows, cols);
                let end = at(anchor.row + (rows as u32).saturating_sub(1), anchor.col + (cols as u32).saturating_sub(1));
                model.insert((sheet, anchor), (RangeRef::new(anchor, end), rows == 0 || cols == 0));
            }
            if step % 7 != 0 {
                continue;
            }
            let sheet = next(3) as usize;
            for _ in 0..20 {
                let cell = at(next(2_100), next(130));
                let want: Vec<CellRef> =
                    model.iter().filter(|((s, _), (r, e))| *s == sheet && !e && r.contains(cell)).map(|((_, a), _)| *a).collect();
                match ix.covering(sheet, cell) {
                    Cover::None => assert!(want.is_empty(), "{cell:?}"),
                    Cover::One(a) => assert_eq!(want, vec![a], "{cell:?}"),
                    Cover::Many => assert!(want.len() > 1, "{cell:?}"),
                }
                let (r0, c0) = (next(2_100), next(130));
                let (tall, wide) = (if next(5) == 0 { 3_000 } else { 40 }, if next(5) == 0 { 130 } else { 20 });
                let range = RangeRef::new(at(r0, c0), at(r0 + next(tall), c0 + next(wide)));
                let want: Vec<CellRef> = model.iter().filter(|((s, _), (r, _))| *s == sheet && r.intersects(&range)).map(|((_, a), _)| *a).collect();
                let mut got = Vec::new();
                ix.meeting(sheet, range, &mut got);
                assert_eq!(got, want, "{range:?}");
            }
        }
    }

    #[test]
    fn an_area_without_cells_covers_none_but_is_met_at_its_anchor() {
        let mut ix = SpillIndex::default();
        ix.insert((0, at(3, 3)), 0, 4);
        assert!(matches!(ix.covering(0, at(3, 3)), Cover::None));
        let mut hits = Vec::new();
        ix.meeting(0, RangeRef::new(at(3, 3), at(3, 3)), &mut hits);
        assert_eq!(hits, vec![at(3, 3)]);
    }
}
