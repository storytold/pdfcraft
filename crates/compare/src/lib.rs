//! pdfcraft-compare — Compare files: text differences (L4).
//!
//! Both documents' words (in reading order, with page and box) are diffed with Myers'
//! O((N+M)·D) algorithm after the common prefix and suffix are set aside; the edit script is
//! grouped into [`Change`]s — inserted, deleted or replaced runs of words — each with the
//! rectangles to highlight on the old and new pages.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// A word of one document.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// 0-based page.
    pub page: usize,
    /// [x0, y0, x1, y1] in the page's user space.
    pub rect: [f64; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Inserted,
    Deleted,
    Replaced,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Inserted => "Inserted",
            Kind::Deleted => "Deleted",
            Kind::Replaced => "Replaced",
        }
    }
}

/// One side of a change: its words' text, page and rectangles (merged per line).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Side {
    pub text: String,
    /// The page the change is on (for a pure insertion or deletion, where the other side's
    /// words would have been).
    pub page: usize,
    pub rects: Vec<[f64; 4]>,
    /// For an empty side (the other side's words were inserted or deleted): the box of the
    /// word next to where they would be.
    pub near: Option<[f64; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub kind: Kind,
    pub old: Side,
    pub new: Side,
}

/// What the comparison found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comparison {
    pub changes: Vec<Change>,
    /// Words in each document.
    pub old_words: usize,
    pub new_words: usize,
}

impl Comparison {
    pub fn count(&self, kind: Kind) -> usize {
        self.changes.iter().filter(|c| c.kind == kind).count()
    }

    /// Whether the documents' text is the same.
    pub fn identical(&self) -> bool {
        self.changes.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Segments longer than this (old + new words) are first split at anchors, so Myers' saved
/// frontiers (O(D²) memory) stay small.
const DIRECT: usize = 3000;

/// The edit script for `a` → `b`.
fn diff(a: &[u64], b: &[u64], out: &mut Vec<Op>, depth: usize) {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    out.extend(std::iter::repeat_n(Op::Equal, prefix));
    let (ma, mb) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let replace = |out: &mut Vec<Op>| out.extend(std::iter::repeat_n(Op::Delete, ma.len()).chain(std::iter::repeat_n(Op::Insert, mb.len())));
    if ma.len() + mb.len() <= DIRECT {
        out.extend(myers(ma, mb));
    } else {
        let anchors = if depth > 8 { Vec::new() } else { anchors(ma, mb) };
        if anchors.is_empty() {
            // A long stretch with nothing in common: one replacement.
            replace(out);
        } else {
            let (mut i, mut j) = (0, 0);
            for (x, y) in anchors {
                diff(&ma[i..x], &mb[j..y], out, depth + 1);
                out.push(Op::Equal);
                i = x + 1;
                j = y + 1;
            }
            diff(&ma[i..], &mb[j..], out, depth + 1);
        }
    }
    out.extend(std::iter::repeat_n(Op::Equal, suffix));
}

/// Patience anchors: words that occur once in each side, in an order both sides agree on
/// (the longest increasing subsequence of their positions).
fn anchors(a: &[u64], b: &[u64]) -> Vec<(usize, usize)> {
    use std::collections::HashMap;
    let mut count: HashMap<u64, (usize, usize, usize, usize)> = HashMap::new();
    for (i, h) in a.iter().enumerate() {
        let e = count.entry(*h).or_insert((0, 0, 0, 0));
        e.0 += 1;
        e.2 = i;
    }
    for (j, h) in b.iter().enumerate() {
        if let Some(e) = count.get_mut(h) {
            e.1 += 1;
            e.3 = j;
        }
    }
    let mut pairs: Vec<(usize, usize)> = count.values().filter(|e| e.0 == 1 && e.1 == 1).map(|e| (e.2, e.3)).collect();
    pairs.sort();
    // LIS on the b positions (patience sorting with back links).
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; pairs.len()];
    for (k, &(_, y)) in pairs.iter().enumerate() {
        let pos = tails.partition_point(|&t| pairs[t].1 < y);
        if pos > 0 {
            prev[k] = tails[pos - 1];
        }
        if pos == tails.len() {
            tails.push(k);
        } else {
            tails[pos] = k;
        }
    }
    let mut out = Vec::new();
    let mut k = tails.last().copied().unwrap_or(usize::MAX);
    while k != usize::MAX {
        out.push(pairs[k]);
        k = prev[k];
    }
    out.reverse();
    out
}

/// Myers' diff of `a` and `b`: the edit script.
fn myers(a: &[u64], b: &[u64]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return std::iter::repeat_n(Op::Delete, n).chain(std::iter::repeat_n(Op::Insert, m)).collect();
    }
    let max = n + m;
    let off = max as isize + 1;
    let mut v = vec![0usize; 2 * max + 3];
    // Frontier d only spans diagonals -d..=d: save just that slice.
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let mut found = false;
    'outer: for d in 0..=max as isize {
        trace.push(v[(off - d - 1).max(0) as usize..=(off + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let i = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) { v[i + 1] } else { v[i - 1] + 1 };
            let mut y = (x as isize - k) as usize;
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                found = true;
                break 'outer;
            }
            k += 2;
        }
    }
    if !found {
        // Too different: replace everything.
        return std::iter::repeat_n(Op::Delete, n).chain(std::iter::repeat_n(Op::Insert, m)).collect();
    }
    // Walk back through the saved frontiers.
    let mut ops = Vec::new();
    let (mut x, mut y) = (n as isize, m as isize);
    for d in (0..trace.len() as isize).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let base = (off - d - 1).max(0);
        let i = |k: isize| (k + off - base) as usize;
        let prev_k = if k == -d || (k != d && v[i(k - 1)] < v[i(k + 1)]) { k + 1 } else { k - 1 };
        let prev_x = v[i(prev_k)] as isize;
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            ops.push(if x == prev_x { Op::Insert } else { Op::Delete });
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    ops
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Merge word boxes on the same page and line into one rectangle per line.
fn rects(words: &[&Word]) -> Vec<[f64; 4]> {
    let mut out: Vec<[f64; 4]> = Vec::new();
    for w in words {
        let r = w.rect;
        match out.last_mut() {
            Some(l) if (l[1] - r[1]).abs() < (r[3] - r[1]).max(1.0) * 0.5 && r[0] - l[2] < (r[3] - r[1]).max(4.0) * 2.0 && r[0] >= l[0] - 1.0 => {
                l[0] = l[0].min(r[0]);
                l[1] = l[1].min(r[1]);
                l[2] = l[2].max(r[2]);
                l[3] = l[3].max(r[3]);
            }
            _ => out.push(r),
        }
    }
    out
}

/// Split `start..end` in `words` into contiguous index ranges that stay on the same page.
fn page_runs(words: &[Word], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut s = start;
    while s < end {
        let Some(first) = words.get(s) else { break };
        let page = first.page;
        let mut e = s + 1;
        while e < end && words.get(e).is_some_and(|w| w.page == page) {
            e += 1;
        }
        runs.push((s, e));
        s = e;
    }
    runs
}

/// Pick the neighbouring word in `words` next to `at` for a one-sided change spanning
/// `start..end` in `changed_doc`, preferring `before` when the change sits at the end of
/// its page in `changed_doc`.
fn neighbour<'a>(words: &'a [Word], at: usize, changed_doc: &[Word], start: usize, end: usize) -> Option<&'a Word> {
    let after = words.get(at);
    let before = at.checked_sub(1).and_then(|p| words.get(p));
    let first_page = changed_doc.get(start).map(|w| w.page);
    let last_page = end.checked_sub(1).and_then(|p| changed_doc.get(p)).map(|w| w.page);
    let before_same_page = start.checked_sub(1).and_then(|p| changed_doc.get(p)).map(|w| w.page) == first_page;
    let after_same_page = changed_doc.get(end).map(|w| w.page) == last_page;
    if !after_same_page && before_same_page { before.or(after) } else { after.or(before) }
}

/// Build one side of a [`Change`] from its changed words or its anchor word in that document.
fn side(words: &[&Word], neighbour: Option<&Word>) -> Side {
    Side {
        text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
        page: words.first().or(neighbour.as_ref()).map_or(0, |w| w.page),
        rects: rects(words),
        near: if words.is_empty() { neighbour.map(|w| w.rect) } else { None },
    }
}

/// Compare two documents' words.
pub fn compare(old: &[Word], new: &[Word]) -> Comparison {
    let a: Vec<u64> = old.iter().map(|w| hash(&w.text)).collect();
    let b: Vec<u64> = new.iter().map(|w| hash(&w.text)).collect();
    let mut ops = Vec::with_capacity(a.len().max(b.len()));
    diff(&a, &b, &mut ops, 0);

    let mut changes = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut k = 0;
    while k < ops.len() {
        if ops[k] == Op::Equal {
            i += 1;
            j += 1;
            k += 1;
            continue;
        }
        let (i0, j0) = (i, j);
        while k < ops.len() && ops[k] != Op::Equal {
            match ops[k] {
                Op::Delete => i += 1,
                _ => j += 1,
            }
            k += 1;
        }
        // Page runs from each side are paired positionally (`del_runs[r]` with `ins_runs[r]`);
        // any trailing runs on the side that spans more pages become one-sided changes.
        let del_runs = page_runs(old, i0, i);
        let ins_runs = page_runs(new, j0, j);
        for r in 0..del_runs.len().max(ins_runs.len()) {
            let (ds, de) = del_runs.get(r).copied().unwrap_or((i, i));
            let (is, ie) = ins_runs.get(r).copied().unwrap_or((j, j));
            let del: Vec<&Word> = old.get(ds..de).into_iter().flatten().collect();
            let ins: Vec<&Word> = new.get(is..ie).into_iter().flatten().collect();
            let kind = match (del.is_empty(), ins.is_empty()) {
                (false, false) => Kind::Replaced,
                (false, true) => Kind::Deleted,
                _ => Kind::Inserted,
            };
            // Where a one-sided change sits in the other document: next to the neighbouring word.
            let old_near = neighbour(old, ds, new, is, ie);
            let new_near = neighbour(new, is, old, ds, de);
            changes.push(Change { kind, old: side(&del, old_near), new: side(&ins, new_near) });
        }
    }
    Comparison { changes, old_words: old.len(), new_words: new.len() }
}

/// The compare report's text: a summary, then each change with its pages.
pub fn report(c: &Comparison, old_name: &str, new_name: &str) -> String {
    let mut s = format!(
        "Compare Report\n\nOld file: {old_name}\nNew file: {new_name}\n\n{} changes: {} replaced, {} inserted, {} deleted.\n",
        c.changes.len(),
        c.count(Kind::Replaced),
        c.count(Kind::Inserted),
        c.count(Kind::Deleted)
    );
    if c.identical() {
        s.push_str("\nThe documents' text is identical.\n");
    }
    for (n, ch) in c.changes.iter().enumerate() {
        s.push_str(&format!("\n{}. {} (old page {}, new page {})\n", n + 1, ch.kind.label(), ch.old.page + 1, ch.new.page + 1));
        if !ch.old.text.is_empty() {
            s.push_str(&format!("   Old: {}\n", ch.old.text));
        }
        if !ch.new.text.is_empty() {
            s.push_str(&format!("   New: {}\n", ch.new.text));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str, page: usize) -> Vec<Word> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, t)| Word { text: t.into(), page, rect: [10.0 + 40.0 * i as f64, 700.0, 45.0 + 40.0 * i as f64, 710.0] })
            .collect()
    }

    #[test]
    fn identical_text_has_no_changes() {
        let c = compare(&words("the same words here", 0), &words("the same words here", 0));
        assert!(c.identical());
    }

    #[test]
    fn insertions_deletions_and_replacements() {
        let old = words("The quick brown fox jumps over the lazy dog", 0);
        let new = words("The quick red fox leaps over the very lazy dog today", 0);
        let c = compare(&old, &new);
        let got: Vec<(Kind, &str, &str)> = c.changes.iter().map(|x| (x.kind, x.old.text.as_str(), x.new.text.as_str())).collect();
        assert_eq!(
            got,
            [(Kind::Replaced, "brown", "red"), (Kind::Replaced, "jumps", "leaps"), (Kind::Inserted, "", "very"), (Kind::Inserted, "", "today"),]
        );
        assert_eq!(c.changes[0].new.rects, [[90.0, 700.0, 125.0, 710.0]]);
        assert_eq!(c.count(Kind::Inserted), 2);
    }

    #[test]
    fn deletions_across_pages_keep_their_pages() {
        let mut old = words("alpha beta gamma", 0);
        old.extend(words("delta epsilon", 1));
        let mut new = words("alpha gamma", 0);
        new.extend(words("delta", 1));
        let c = compare(&old, &new);
        let got: Vec<(Kind, usize, &str)> = c.changes.iter().map(|x| (x.kind, x.old.page, x.old.text.as_str())).collect();
        assert_eq!(got, [(Kind::Deleted, 0, "beta"), (Kind::Deleted, 1, "epsilon")]);
        // The deletion's place in the new document: the page of the words around it.
        assert_eq!(c.changes[1].new.page, 1);
        let r = report(&c, "v1.pdf", "v2.pdf");
        assert!(r.contains("2 changes: 0 replaced, 0 inserted, 2 deleted.") && r.contains("Old: epsilon"), "{r}");
    }

    #[test]
    fn changes_at_page_boundaries_stay_on_their_own_page() {
        // Deleting the last word of page 0 ("gamma") must anchor its new-document page and
        // `near` box to "beta" on page 0, not jump to "delta" at the start of page 1.
        let mut old = words("alpha beta gamma", 0);
        old.extend(words("delta epsilon", 1));
        let mut new = words("alpha beta", 0);
        new.extend(words("delta epsilon", 1));
        let c = compare(&old, &new);
        assert_eq!(c.changes.len(), 1);
        assert_eq!((c.changes[0].kind, c.changes[0].old.page, c.changes[0].new.page), (Kind::Deleted, 0, 0));
        assert_eq!(c.changes[0].new.near, Some(new[1].rect), "deletion at end of page 0 anchors to preceding word on page 0");

        // Symmetrically, inserting "gamma" at the end of page 0 must anchor its old-document
        // page and `near` box to "beta" on page 0, not "delta" on page 1.
        let c_ins = compare(&new, &old);
        assert_eq!(c_ins.changes.len(), 1);
        assert_eq!((c_ins.changes[0].kind, c_ins.changes[0].old.page, c_ins.changes[0].new.page), (Kind::Inserted, 0, 0));
        assert_eq!(c_ins.changes[0].old.near, Some(new[1].rect), "insertion at end of page 0 anchors to preceding word on page 0");

        // A multi-word change that crosses a page break splits into one Change per page so
        // every page keeps its own highlight rectangles and anchor page.
        let cross_old = vec![
            Word { text: "keep".into(), page: 0, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "end0".into(), page: 0, rect: [50.0, 700.0, 85.0, 710.0] },
            Word { text: "start1".into(), page: 1, rect: [90.0, 700.0, 125.0, 710.0] },
            Word { text: "tail".into(), page: 1, rect: [10.0, 680.0, 45.0, 690.0] },
        ];
        let cross_new = vec![
            Word { text: "keep".into(), page: 0, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "tail".into(), page: 1, rect: [10.0, 680.0, 45.0, 690.0] },
        ];
        let c_cross = compare(&cross_old, &cross_new);
        assert_eq!(c_cross.changes.len(), 2);
        assert_eq!((c_cross.changes[0].old.page, c_cross.changes[0].new.page), (0, 0));
        assert_eq!(c_cross.changes[0].old.rects, [[50.0, 700.0, 85.0, 710.0]]);
        assert_eq!((c_cross.changes[1].old.page, c_cross.changes[1].new.page), (1, 1));
        assert_eq!(c_cross.changes[1].old.rects, [[90.0, 700.0, 125.0, 710.0]]);

        // Even when page numbers are shifted between old and new (e.g. a leading page was removed),
        // deleting the last word of old page 1 ("X") still anchors to "A" on new page 0 rather than
        // jumping to "B" on new page 1 just because new page 1 == old page 1.
        let shifted_old = vec![
            Word { text: "A".into(), page: 1, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "X".into(), page: 1, rect: [50.0, 700.0, 85.0, 710.0] },
            Word { text: "B".into(), page: 2, rect: [10.0, 700.0, 45.0, 710.0] },
        ];
        let shifted_new = vec![
            Word { text: "A".into(), page: 0, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "B".into(), page: 1, rect: [10.0, 700.0, 45.0, 710.0] },
        ];
        let c_shifted = compare(&shifted_old, &shifted_new);
        assert_eq!(c_shifted.changes.len(), 1);
        assert_eq!((c_shifted.changes[0].old.page, c_shifted.changes[0].new.page), (1, 0));
        assert_eq!(c_shifted.changes[0].new.near, Some(shifted_new[0].rect));

        // A replacement crossing a page break on both sides pairs the page runs positionally,
        // and any extra run on the longer side becomes a trailing one-sided change.
        let rep_old = vec![
            Word { text: "keep".into(), page: 0, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "old0".into(), page: 0, rect: [50.0, 700.0, 85.0, 710.0] },
            Word { text: "old1".into(), page: 1, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "old2".into(), page: 2, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "tail".into(), page: 2, rect: [50.0, 700.0, 85.0, 710.0] },
        ];
        let rep_new = vec![
            Word { text: "keep".into(), page: 0, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "new0".into(), page: 0, rect: [50.0, 700.0, 85.0, 710.0] },
            Word { text: "new1".into(), page: 1, rect: [10.0, 700.0, 45.0, 710.0] },
            Word { text: "tail".into(), page: 1, rect: [50.0, 700.0, 85.0, 710.0] },
        ];
        let c_rep = compare(&rep_old, &rep_new);
        let got_rep: Vec<(Kind, usize, usize, &str, &str)> =
            c_rep.changes.iter().map(|x| (x.kind, x.old.page, x.new.page, x.old.text.as_str(), x.new.text.as_str())).collect();
        assert_eq!(got_rep, [(Kind::Replaced, 0, 0, "old0", "new0"), (Kind::Replaced, 1, 1, "old1", "new1"), (Kind::Deleted, 2, 1, "old2", "")]);
    }

    #[test]
    fn adjacent_words_merge_into_one_rectangle_per_line() {
        let old = words("a b c", 0);
        let new = words("x y z", 0);
        let c = compare(&old, &new);
        assert_eq!(c.changes.len(), 1);
        assert_eq!(c.changes[0].new.rects.len(), 1);
    }

    #[test]
    fn long_documents_with_scattered_edits() {
        let text: Vec<String> = (0..20_000).map(|i| format!("w{i}")).collect();
        let old: Vec<Word> = text.iter().map(|t| Word { text: t.clone(), page: 0, rect: [0.0; 4] }).collect();
        let mut new = old.clone();
        for k in [100usize, 5000, 12_000, 19_000] {
            new[k].text = format!("changed{k}");
        }
        new.remove(8000);
        let c = compare(&old, &new);
        assert_eq!(c.changes.len(), 5, "{:?}", c.changes.iter().map(|x| (x.kind, &x.old.text, &x.new.text)).collect::<Vec<_>>());
        assert_eq!(c.count(Kind::Replaced), 4);
    }

    #[test]
    fn very_different_documents_finish() {
        let old: Vec<Word> = (0..30_000).map(|i| Word { text: format!("a{i}"), page: 0, rect: [0.0; 4] }).collect();
        let new: Vec<Word> = (0..30_000).map(|i| Word { text: format!("b{i}"), page: 0, rect: [0.0; 4] }).collect();
        let c = compare(&old, &new);
        assert_eq!(c.changes.len(), 1);
        assert_eq!(c.changes[0].kind, Kind::Replaced);
    }
}

/// Visual compare: the regions where two renderings of a page differ, as pixel boxes
/// `[x0, y0, x1, y1]` (y down) in `a`'s image. Both images are RGBA; `b` is sampled at the
/// same relative position when sizes differ. Pixels count as different when a channel differs
/// by more than `tolerance`; differing cells of a coarse grid are joined into regions.
pub fn visual_regions(a: (&[u8], u32, u32), b: (&[u8], u32, u32), tolerance: u8) -> Vec<[u32; 4]> {
    let (pa, wa, ha) = a;
    let (pb, wb, hb) = b;
    if wa == 0 || ha == 0 || wb == 0 || hb == 0 {
        return Vec::new();
    }
    const CELL: u32 = 8;
    let (gw, gh) = (wa.div_ceil(CELL), ha.div_ceil(CELL));
    let mut grid = vec![false; (gw * gh) as usize];
    for y in 0..ha {
        let yb = (y as u64 * hb as u64 / ha as u64) as u32;
        for x in 0..wa {
            let xb = (x as u64 * wb as u64 / wa as u64) as u32;
            let ia = ((y * wa + x) * 4) as usize;
            let ib = ((yb * wb + xb) * 4) as usize;
            let (Some(ca), Some(cb)) = (pa.get(ia..ia + 3), pb.get(ib..ib + 3)) else { continue };
            if ca.iter().zip(cb).any(|(p, q)| p.abs_diff(*q) > tolerance) {
                grid[((y / CELL) * gw + x / CELL) as usize] = true;
            }
        }
    }
    // Connected cells (8-neighbourhood, with a one-cell gap bridged) become one region.
    let mut seen = vec![false; grid.len()];
    let mut out = Vec::new();
    for start in 0..grid.len() {
        if !grid[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(c) = stack.pop() {
            let (cx, cy) = (c as u32 % gw, c as u32 / gw);
            x0 = x0.min(cx);
            y0 = y0.min(cy);
            x1 = x1.max(cx);
            y1 = y1.max(cy);
            for dy in -2i64..=2 {
                for dx in -2i64..=2 {
                    let (nx, ny) = (cx as i64 + dx, cy as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= gw as i64 || ny >= gh as i64 {
                        continue;
                    }
                    let n = (ny as u32 * gw + nx as u32) as usize;
                    if grid[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        out.push([x0 * CELL, y0 * CELL, ((x1 + 1) * CELL).min(wa), ((y1 + 1) * CELL).min(ha)]);
    }
    out
}

#[cfg(test)]
mod visual_tests {
    use super::*;

    fn image(w: u32, h: u32, boxes: &[[u32; 4]]) -> Vec<u8> {
        let mut px = vec![255u8; (w * h * 4) as usize];
        for b in boxes {
            for y in b[1]..b[3] {
                for x in b[0]..b[2] {
                    let i = ((y * w + x) * 4) as usize;
                    px[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        px
    }

    #[test]
    fn finds_changed_regions() {
        let a = image(200, 200, &[[10, 10, 50, 20]]);
        let b = image(200, 200, &[[10, 10, 50, 20], [120, 150, 160, 170]]);
        assert!(visual_regions((&a, 200, 200), (&a, 200, 200), 16).is_empty());
        let r = visual_regions((&a, 200, 200), (&b, 200, 200), 16);
        assert_eq!(r, [[120, 144, 160, 176]]);
        // Two nearby marks join; distant ones don't.
        let c = image(200, 200, &[[10, 10, 50, 20], [100, 100, 104, 104], [110, 100, 114, 104], [10, 180, 14, 184]]);
        assert_eq!(visual_regions((&a, 200, 200), (&c, 200, 200), 16).len(), 2);
    }
}
