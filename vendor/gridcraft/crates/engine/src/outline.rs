//! Outline groups (Data ▸ Group) of a sheet's rows or columns: the runs of lines at or above
//! each outline level, as the grid draws them (brackets, − / + buttons, level buttons) and the
//! Show / Hide Detail commands act on them. Summary rows sit below their detail and summary
//! columns right of it, or above / left of it when the sheet's outline settings say so
//! ([`Sheet::summary_below`], [`Sheet::summary_right`]).

use std::collections::BTreeMap;

use gridcraft_model::{LineInfo, Sheet};

/// The deepest outline level XLSX allows.
pub const MAX_LEVEL: u8 = 7;

/// One group: lines `start..=end` at outline level `level` or deeper, with its summary line
/// just after `end`, or just before `start` when `summary_before` is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutlineGroup {
    pub level: u8,
    pub start: u32,
    pub end: u32,
    /// Every detail line is hidden: the button shows +.
    pub collapsed: bool,
    /// The summary line is above (rows) or left of (columns) the detail.
    pub summary_before: bool,
}

impl OutlineGroup {
    /// The summary line, where the − / + button sits.
    pub fn summary(&self) -> u32 {
        if self.summary_before { self.start.saturating_sub(1) } else { self.end.saturating_add(1) }
    }
    /// Whether the summary line exists: a group that starts at the first line (summary before)
    /// or ends at the last one (summary after) has none.
    pub fn has_summary(&self) -> bool {
        !self.contains(self.summary())
    }
    pub fn contains(&self, line: u32) -> bool {
        line >= self.start && line <= self.end
    }
}

/// Whether the sheet's summary rows (`rows`) or summary columns sit before their detail.
pub fn summary_before(sh: &Sheet, rows: bool) -> bool {
    if rows { !sh.summary_below } else { !sh.summary_right }
}

/// The deepest outline level used by these lines (0 = no outline).
pub fn max_level(lines: &BTreeMap<u32, LineInfo>) -> u8 {
    lines.values().map(|l| l.outline.min(MAX_LEVEL)).max().unwrap_or(0)
}

/// Every group, with its summary line after the detail; see [`groups_with`].
pub fn groups(lines: &BTreeMap<u32, LineInfo>) -> Vec<OutlineGroup> {
    groups_with(lines, false)
}

/// Every group, ordered by where it ends (inner groups before the group around them), with its
/// summary line before the detail when `summary_before` is set.
pub fn groups_with(lines: &BTreeMap<u32, LineInfo>, summary_before: bool) -> Vec<OutlineGroup> {
    let mut out = Vec::new();
    // Open group per level (index level - 1): start line and whether all its lines are hidden.
    let mut open: [Option<(u32, bool)>; MAX_LEVEL as usize] = [None; MAX_LEVEL as usize];
    let mut prev: Option<u32> = None;
    let close_from = |open: &mut [Option<(u32, bool)>; MAX_LEVEL as usize], from: u8, end: u32, out: &mut Vec<OutlineGroup>| {
        for level in (from..=MAX_LEVEL).rev() {
            if let Some(slot) = open.get_mut(level as usize - 1)
                && let Some((start, hidden)) = slot.take()
            {
                out.push(OutlineGroup { level, start, end, collapsed: hidden, summary_before });
            }
        }
    };
    for (&i, info) in lines.iter().filter(|(_, l)| l.outline > 0) {
        let level = info.outline.min(MAX_LEVEL);
        match prev {
            // A gap ends every group.
            Some(p) if p.checked_add(1) != Some(i) => close_from(&mut open, 1, p, &mut out),
            Some(p) if level < MAX_LEVEL => close_from(&mut open, level + 1, p, &mut out),
            _ => {}
        }
        for slot in open.iter_mut().take(level as usize) {
            match slot {
                Some((_, hidden)) => *hidden &= info.hidden,
                None => *slot = Some((i, info.hidden)),
            }
        }
        prev = Some(i);
    }
    if let Some(p) = prev {
        close_from(&mut open, 1, p, &mut out);
    }
    out
}

/// The group at `level` that holds `line`.
pub fn group_at(lines: &BTreeMap<u32, LineInfo>, line: u32, level: u8) -> Option<OutlineGroup> {
    groups(lines).into_iter().find(|g| g.level == level && g.contains(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(spec: &[(u32, u8, bool)]) -> BTreeMap<u32, LineInfo> {
        spec.iter().map(|&(i, outline, hidden)| (i, LineInfo { outline, hidden, ..Default::default() })).collect()
    }

    #[test]
    fn no_outline_has_no_groups() {
        let l = lines(&[(3, 0, true)]);
        assert_eq!(max_level(&l), 0);
        assert!(groups(&l).is_empty());
    }

    #[test]
    fn nested_and_separate_groups() {
        // Rows 5..8 at level 1 with 6..7 at level 2; rows 12..13 at level 1 after a gap.
        let l = lines(&[(4, 1, false), (5, 2, false), (6, 2, false), (7, 1, false), (11, 1, true), (12, 1, true)]);
        assert_eq!(max_level(&l), 2);
        let g = groups(&l);
        assert_eq!(
            g,
            vec![
                OutlineGroup { level: 2, start: 5, end: 6, collapsed: false, summary_before: false },
                OutlineGroup { level: 1, start: 4, end: 7, collapsed: false, summary_before: false },
                OutlineGroup { level: 1, start: 11, end: 12, collapsed: true, summary_before: false },
            ]
        );
        assert_eq!(g[1].summary(), 8);
        assert_eq!(group_at(&l, 6, 1).map(|g| g.start), Some(4));
        assert_eq!(group_at(&l, 4, 2), None);
    }

    #[test]
    fn adjacent_groups_at_one_level_split_by_a_shallower_line() {
        // Two level-2 groups inside one level-1 group, separated by a level-1 line.
        let l = lines(&[(0, 2, true), (1, 2, true), (2, 1, false), (3, 2, false)]);
        let g = groups(&l);
        assert_eq!(g.iter().filter(|g| g.level == 2).count(), 2);
        assert_eq!(g.iter().find(|g| g.level == 2 && g.start == 0).map(|g| g.collapsed), Some(true));
        assert_eq!(g.iter().find(|g| g.level == 1).map(|g| (g.start, g.end, g.collapsed)), Some((0, 3, false)));
    }

    #[test]
    fn deepest_level_and_last_line_do_not_overflow() {
        let l = lines(&[(u32::MAX - 1, 9, false), (u32::MAX, 7, false)]);
        let g = groups(&l);
        assert_eq!(g.len(), 7);
        assert!(g.iter().all(|g| g.start == u32::MAX - 1 && g.end == u32::MAX));
        assert_eq!(g[0].summary(), u32::MAX);
        assert!(!g[0].has_summary());
    }

    #[test]
    fn summary_before_the_detail() {
        // Rows 5..8 at level 1 with 6..7 at level 2: the summaries are rows 4 and 5.
        let l = lines(&[(4, 1, false), (5, 2, false), (6, 2, false), (7, 1, false), (0, 1, false)]);
        let g = groups_with(&l, true);
        let at = |level: u8, start: u32| g.iter().find(|g| g.level == level && g.start == start).copied();
        assert_eq!(at(1, 4).map(|g| (g.summary(), g.has_summary())), Some((3, true)));
        assert_eq!(at(2, 5).map(|g| (g.summary(), g.has_summary())), Some((4, true)));
        // A group on the first line has no line before it.
        assert_eq!(at(1, 0).map(|g| (g.summary(), g.has_summary())), Some((0, false)));
        let mut sh = Sheet::new("S");
        assert!(!summary_before(&sh, true) && !summary_before(&sh, false));
        sh.summary_below = false;
        assert!(summary_before(&sh, true) && !summary_before(&sh, false));
    }
}
