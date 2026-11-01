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

/// How a page's place in the document changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    Inserted,
    Deleted,
    Moved,
}

impl PageKind {
    pub fn label(self) -> &'static str {
        match self {
            PageKind::Inserted => "Inserted page",
            PageKind::Deleted => "Deleted page",
            PageKind::Moved => "Moved page",
        }
    }
}

/// A page that was inserted, deleted, or moved between the two documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageChange {
    pub kind: PageKind,
    /// 0-based page in `old` (for `Deleted` and `Moved`; for `Inserted`, the neighbouring old page).
    pub old_page: usize,
    /// 0-based page in `new` (for `Inserted` and `Moved`; for `Deleted`, the neighbouring new page).
    pub new_page: usize,
}

/// What the comparison found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comparison {
    pub changes: Vec<Change>,
    /// Matched `(old_page, new_page)` pairs in `new` page order.
    pub page_pairs: Vec<(usize, usize)>,
    /// Pages that were inserted, deleted, or moved.
    pub page_changes: Vec<PageChange>,
    /// Words in each document.
    pub old_words: usize,
    pub new_words: usize,
}

impl Comparison {
    pub fn count(&self, kind: Kind) -> usize {
        self.changes.iter().filter(|c| c.kind == kind).count()
    }

    pub fn page_count(&self, kind: PageKind) -> usize {
        self.page_changes.iter().filter(|c| c.kind == kind).count()
    }

    /// Whether the documents' text and page order are the same.
    pub fn identical(&self) -> bool {
        self.changes.is_empty() && self.page_changes.is_empty()
    }

    /// The counterpart in `old` of `new_page`, if `new_page` was not inserted.
    pub fn old_page(&self, new_page: usize) -> Option<usize> {
        self.page_pairs.iter().find(|&&(_, np)| np == new_page).map(|&(op, _)| op)
    }

    /// The counterpart in `new` of `old_page`, if `old_page` was not deleted.
    pub fn new_page(&self, old_page: usize) -> Option<usize> {
        self.page_pairs.iter().find(|&&(op, _)| op == old_page).map(|&(_, np)| np)
    }

    /// The `old` page to align with `new_page` in side-by-side view (its counterpart, or the
    /// neighbouring old page when `new_page` was inserted).
    pub fn sync_old_page(&self, new_page: usize) -> usize {
        self.old_page(new_page)
            .or_else(|| self.page_changes.iter().find(|c| c.kind == PageKind::Inserted && c.new_page == new_page).map(|c| c.old_page))
            .or_else(|| self.page_pairs.iter().rev().find(|&&(_, np)| np < new_page).map(|&(op, _)| op))
            .unwrap_or(new_page)
    }

    /// The `new` page to align with `old_page` in side-by-side view (its counterpart, or the
    /// neighbouring new page when `old_page` was deleted).
    pub fn sync_new_page(&self, old_page: usize) -> usize {
        self.new_page(old_page)
            .or_else(|| self.page_changes.iter().find(|c| c.kind == PageKind::Deleted && c.old_page == old_page).map(|c| c.new_page))
            .or_else(|| self.page_pairs.iter().rev().find(|&&(op, _)| op < old_page).map(|&(_, np)| np))
            .unwrap_or(old_page)
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

fn side_on_page(words: &[&Word], neighbour: Option<&Word>, fallback_page: usize) -> Side {
    Side {
        text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
        page: words.first().or(neighbour.as_ref()).map_or(fallback_page, |w| w.page),
        rects: rects(words),
        near: if words.is_empty() { neighbour.map(|w| w.rect) } else { None },
    }
}

struct PageProfile {
    hashes: Vec<u64>,
    unigrams: std::collections::HashMap<u64, usize>,
    bigrams: std::collections::HashMap<(u64, u64), usize>,
}

impl PageProfile {
    fn from_words(words: &[&Word]) -> Self {
        use std::collections::HashMap;
        let hashes: Vec<u64> = words.iter().map(|w| hash(&w.text)).collect();
        let mut unigrams = HashMap::with_capacity(hashes.len());
        for &h in &hashes {
            *unigrams.entry(h).or_insert(0) += 1;
        }
        let mut bigrams = HashMap::with_capacity(hashes.len().saturating_sub(1));
        for pair in hashes.windows(2) {
            *bigrams.entry((pair[0], pair[1])).or_insert(0) += 1;
        }
        Self { hashes, unigrams, bigrams }
    }
}

/// Content similarity in `0.0..=1.0` between two pages' words (unigram + phrase Dice overlap).
fn page_similarity(a: &PageProfile, b: &PageProfile) -> f64 {
    if a.hashes.is_empty() || b.hashes.is_empty() {
        return 0.0;
    }
    if a.hashes == b.hashes {
        return 1.0;
    }
    let common_1: usize = a.unigrams.iter().map(|(h, &c)| c.min(b.unigrams.get(h).copied().unwrap_or(0))).sum();
    let dice_1 = (2 * common_1) as f64 / (a.hashes.len() + b.hashes.len()) as f64;
    if a.hashes.len() >= 2 && b.hashes.len() >= 2 {
        let common_2: usize = a.bigrams.iter().map(|(bg, &c)| c.min(b.bigrams.get(bg).copied().unwrap_or(0))).sum();
        let dice_2 = (2 * common_2) as f64 / (a.hashes.len() - 1 + b.hashes.len() - 1) as f64;
        0.7 * dice_1 + 0.3 * dice_2
    } else {
        dice_1
    }
}

/// Longest increasing subsequence of matched `(old_page, new_page)` pairs (sorted by `old_page`),
/// returning which `old_page` indices remain in monotonic order.
fn in_order_mask(n: usize, old_to_new: &[Option<usize>]) -> Vec<bool> {
    let pairs: Vec<(usize, usize)> = (0..n).filter_map(|p| old_to_new.get(p).copied().flatten().map(|q| (p, q))).collect();
    let mut in_order = vec![false; n];
    if pairs.is_empty() {
        return in_order;
    }
    // Maximise LIS length, breaking ties in favour of smaller page displacement.
    let mut dp: Vec<(usize, usize)> = vec![(1, 0); pairs.len()];
    let mut prev: Vec<Option<usize>> = vec![None; pairs.len()];
    for i in 0..pairs.len() {
        let disp_i = pairs[i].0.abs_diff(pairs[i].1);
        dp[i] = (1, disp_i);
        for j in 0..i {
            if pairs[j].1 < pairs[i].1 {
                let cand = (dp[j].0 + 1, dp[j].1.saturating_add(disp_i));
                if cand.0 > dp[i].0 || (cand.0 == dp[i].0 && cand.1 < dp[i].1) {
                    dp[i] = cand;
                    prev[i] = Some(j);
                }
            }
        }
    }
    let mut best = 0;
    for i in 1..pairs.len() {
        if dp[i].0 > dp[best].0 || (dp[i].0 == dp[best].0 && dp[i].1 < dp[best].1) {
            best = i;
        }
    }
    let mut cur = Some(best);
    while let Some(k) = cur {
        if let Some(slot) = in_order.get_mut(pairs[k].0) {
            *slot = true;
        }
        cur = prev[k];
    }
    in_order
}

/// Compare two documents' words.
pub fn compare(old: &[Word], new: &[Word]) -> Comparison {
    compare_with_pages(old, 0, new, 0)
}

/// Compare two documents' words with explicit page counts (so blank or image-only pages are
/// aligned as well).
pub fn compare_with_pages(old: &[Word], old_pages: usize, new: &[Word], new_pages: usize) -> Comparison {
    let n = old_pages.max(old.iter().map(|w| w.page + 1).max().unwrap_or(0));
    let m = new_pages.max(new.iter().map(|w| w.page + 1).max().unwrap_or(0));
    if n == 0 && m == 0 {
        return Comparison::default();
    }
    let mut old_by_page: Vec<Vec<&Word>> = vec![Vec::new(); n];
    for w in old {
        if let Some(p) = old_by_page.get_mut(w.page) {
            p.push(w);
        }
    }
    let mut new_by_page: Vec<Vec<&Word>> = vec![Vec::new(); m];
    for w in new {
        if let Some(q) = new_by_page.get_mut(w.page) {
            q.push(w);
        }
    }

    const MIN_PAGE_SIM: f64 = 0.25;
    const MAX_POSTINGS: usize = 32;
    const TOP_CANDIDATES: usize = 4;
    let old_profiles: Vec<PageProfile> = old_by_page.iter().map(|w| PageProfile::from_words(w)).collect();
    let new_profiles: Vec<PageProfile> = new_by_page.iter().map(|w| PageProfile::from_words(w)).collect();

    let mut token_to_new: std::collections::HashMap<u64, Vec<(usize, usize)>> = std::collections::HashMap::new();
    for (q, pb) in new_profiles.iter().enumerate() {
        for (&h, &c) in &pb.unigrams {
            token_to_new.entry(h).or_default().push((q, c));
        }
    }

    let mut candidates: Vec<(usize, usize, f64)> = Vec::new();
    let mut votes = vec![0usize; m];
    let mut touched = Vec::new();
    for (p, pa) in old_profiles.iter().enumerate() {
        if pa.hashes.is_empty() {
            continue;
        }
        for (&h, &c) in &pa.unigrams {
            if let Some(postings) = token_to_new.get(&h)
                && postings.len() <= MAX_POSTINGS
            {
                for &(q, qc) in postings {
                    if votes[q] == 0 {
                        touched.push(q);
                    }
                    votes[q] += c.min(qc);
                }
            }
        }
        if touched.is_empty() {
            for (&h, &c) in &pa.unigrams {
                if let Some(postings) = token_to_new.get(&h) {
                    for &(q, qc) in postings {
                        if votes[q] == 0 {
                            touched.push(q);
                        }
                        votes[q] += c.min(qc);
                    }
                }
            }
        }
        if p < m && votes[p] == 0 && !new_profiles[p].hashes.is_empty() {
            touched.push(p);
        }
        touched.sort_unstable_by(|&qa, &qb| votes[qb].cmp(&votes[qa]).then_with(|| p.abs_diff(qa).cmp(&p.abs_diff(qb))));
        for &q in touched.iter().take(TOP_CANDIDATES) {
            let pb = &new_profiles[q];
            let sim = page_similarity(pa, pb);
            if sim >= MIN_PAGE_SIM {
                let rel_dist = (p as f64 / n.max(1) as f64 - q as f64 / m.max(1) as f64).abs();
                let bonus = 1e-4 * (1.0 - rel_dist) + if p == q { 1e-5 } else { 0.0 };
                candidates.push((p, q, sim + bonus));
            }
        }
        for &q in &touched {
            votes[q] = 0;
        }
        touched.clear();
    }
    candidates.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.abs_diff(a.1).cmp(&b.0.abs_diff(b.1))).then_with(|| (a.0, a.1).cmp(&(b.0, b.1))));

    let mut old_to_new: Vec<Option<usize>> = vec![None; n];
    let mut new_to_old: Vec<Option<usize>> = vec![None; m];
    for (p, q, _) in candidates {
        if old_to_new[p].is_none() && new_to_old[q].is_none() {
            old_to_new[p] = Some(q);
            new_to_old[q] = Some(p);
        }
    }

    // If both documents have the same page count and no pages matched by text similarity
    // (e.g. a 1-page document whose words were all replaced), keep 1:1 positional pairing.
    if n == m && old_to_new.iter().all(Option::is_none) {
        for p in 0..n {
            old_to_new[p] = Some(p);
            new_to_old[p] = Some(p);
        }
    }

    // Pair blank/image-only pages that sit in the same monotonic gap between matched text pages.
    let initial_in_order = in_order_mask(n, &old_to_new);
    let mut anchors: Vec<(usize, usize)> = (0..n).filter(|&p| initial_in_order[p]).filter_map(|p| old_to_new[p].map(|q| (p, q))).collect();
    let mut bounds = Vec::with_capacity(anchors.len() + 2);
    bounds.push((0usize, 0usize));
    for &(p, q) in &anchors {
        bounds.push((p, q));
    }
    bounds.push((n, m));
    for w in bounds.windows(2) {
        let ((p0, q0), (p1, q1)) = (w[0], w[1]);
        let empty_old: Vec<usize> = (p0..p1).filter(|&p| old_to_new[p].is_none() && old_by_page[p].is_empty()).collect();
        let empty_new: Vec<usize> = (q0..q1).filter(|&q| new_to_old[q].is_none() && new_by_page[q].is_empty()).collect();
        for (&p, &q) in empty_old.iter().zip(empty_new.iter()) {
            old_to_new[p] = Some(q);
            new_to_old[q] = Some(p);
            anchors.push((p, q));
        }
        // Same number of leftover text pages on both sides of the gap: treat them as edited pages.
        let rest_old: Vec<usize> = (p0..p1).filter(|&p| old_to_new[p].is_none()).collect();
        let rest_new: Vec<usize> = (q0..q1).filter(|&q| new_to_old[q].is_none()).collect();
        if !rest_old.is_empty() && rest_old.len() == rest_new.len() {
            for (&p, &q) in rest_old.iter().zip(rest_new.iter()) {
                old_to_new[p] = Some(q);
                new_to_old[q] = Some(p);
            }
        }
    }

    let in_order = in_order_mask(n, &old_to_new);
    let page_pairs: Vec<(usize, usize)> = (0..m).filter_map(|q| new_to_old[q].map(|p| (p, q))).collect();

    let anchor_new_for_old = |p: usize| -> (usize, Option<&Word>) {
        if let Some((prev_p, prev_q)) = (0..p).rev().find_map(|k| old_to_new[k].map(|q| (k, q))) {
            let _ = prev_p;
            (prev_q, new_by_page.get(prev_q).and_then(|w| w.last().copied()))
        } else {
            (0, new_by_page.first().and_then(|w| w.first().copied()))
        }
    };
    let anchor_old_for_new = |q: usize| -> (usize, Option<&Word>) {
        if let Some((prev_q, prev_p)) = (0..q).rev().find_map(|k| new_to_old[k].map(|p| (k, p))) {
            let _ = prev_q;
            (prev_p, old_by_page.get(prev_p).and_then(|w| w.last().copied()))
        } else {
            (0, old_by_page.first().and_then(|w| w.first().copied()))
        }
    };

    // Group deleted old pages by the new page they precede (or `m` when they sit after all matched pages).
    let mut deleted_before_new: Vec<Vec<usize>> = vec![Vec::new(); m + 1];
    let mut page_changes = Vec::new();
    for p in 0..n {
        match old_to_new[p] {
            None => {
                let (anchor_new, _) = anchor_new_for_old(p);
                let slot = (p + 1..n).find_map(|k| old_to_new[k]).unwrap_or(m);
                deleted_before_new[slot].push(p);
                page_changes.push(PageChange { kind: PageKind::Deleted, old_page: p, new_page: anchor_new });
            }
            Some(q) if !in_order[p] => {
                page_changes.push(PageChange { kind: PageKind::Moved, old_page: p, new_page: q });
            }
            Some(_) => {}
        }
    }
    for (q, matched_old) in new_to_old.iter().enumerate() {
        if matched_old.is_none() {
            let (anchor_old, _) = anchor_old_for_new(q);
            page_changes.push(PageChange { kind: PageKind::Inserted, old_page: anchor_old, new_page: q });
        }
    }
    page_changes.sort_by_key(|c| (c.new_page, c.old_page));

    // Diff the full sequence of words after reordering moved `old` pages into `new` page order,
    // so text that reflows across a page break matches across the boundary.
    let mut reordered_old: Vec<&Word> = Vec::with_capacity(old.len());
    let mut ordered_new: Vec<&Word> = Vec::with_capacity(new.len());
    for q in 0..m {
        for &del_p in &deleted_before_new[q] {
            reordered_old.extend_from_slice(&old_by_page[del_p]);
        }
        if let Some(p) = new_to_old[q] {
            reordered_old.extend_from_slice(&old_by_page[p]);
        }
        ordered_new.extend_from_slice(&new_by_page[q]);
    }
    for &del_p in &deleted_before_new[m] {
        reordered_old.extend_from_slice(&old_by_page[del_p]);
    }

    let a: Vec<u64> = reordered_old.iter().map(|w| hash(&w.text)).collect();
    let b: Vec<u64> = ordered_new.iter().map(|w| hash(&w.text)).collect();
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
        let (mut del, mut ins): (Vec<&Word>, Vec<&Word>) = (Vec::new(), Vec::new());
        while k < ops.len() && ops[k] != Op::Equal {
            match ops[k] {
                Op::Delete => {
                    if let Some(&w) = reordered_old.get(i) {
                        del.push(w);
                    }
                    i += 1;
                }
                _ => {
                    if let Some(&w) = ordered_new.get(j) {
                        ins.push(w);
                    }
                    j += 1;
                }
            }
            k += 1;
        }
        let kind = match (del.is_empty(), ins.is_empty()) {
            (false, false) => Kind::Replaced,
            (false, true) => Kind::Deleted,
            _ => Kind::Inserted,
        };
        let (old_page, old_near) = if let Some(&w) = del.first() {
            (w.page, None)
        } else {
            let ins_page = ins.first().map_or(0, |w| w.page);
            let target_old = new_to_old.get(ins_page).copied().flatten().unwrap_or_else(|| anchor_old_for_new(ins_page).0);
            let near = reordered_old
                .get(i)
                .copied()
                .filter(|w| w.page == target_old)
                .or_else(|| i.checked_sub(1).and_then(|idx| reordered_old.get(idx).copied()).filter(|w| w.page == target_old))
                .or_else(|| anchor_old_for_new(ins_page).1)
                .or_else(|| reordered_old.get(i).copied())
                .or_else(|| reordered_old.last().copied());
            (target_old, near)
        };
        let (new_page, new_near) = if let Some(&w) = ins.first() {
            (w.page, None)
        } else {
            let del_page = del.first().map_or(0, |w| w.page);
            let target_new = old_to_new.get(del_page).copied().flatten().unwrap_or_else(|| anchor_new_for_old(del_page).0);
            let near = ordered_new
                .get(j)
                .copied()
                .filter(|w| w.page == target_new)
                .or_else(|| j.checked_sub(1).and_then(|idx| ordered_new.get(idx).copied()).filter(|w| w.page == target_new))
                .or_else(|| anchor_new_for_old(del_page).1)
                .or_else(|| ordered_new.get(j).copied())
                .or_else(|| ordered_new.last().copied());
            (target_new, near)
        };
        changes.push(Change { kind, old: side_on_page(&del, old_near, old_page), new: side_on_page(&ins, new_near, new_page) });
    }

    // Pure pagination reflow of identical text produces no word changes; do not report reflowed
    // page breaks as inserted or deleted pages, while keeping blank or image-only page changes.
    if changes.is_empty() && (!old.is_empty() || !new.is_empty()) {
        page_changes.retain(|c| match c.kind {
            PageKind::Moved => true,
            PageKind::Inserted => new_by_page.get(c.new_page).is_some_and(Vec::is_empty),
            PageKind::Deleted => old_by_page.get(c.old_page).is_some_and(Vec::is_empty),
        });
    }

    Comparison { changes, page_pairs, page_changes, old_words: old.len(), new_words: new.len() }
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
    if !c.page_changes.is_empty() {
        s.push_str(&format!(
            "Page changes: {} inserted, {} deleted, {} moved.\n",
            c.page_count(PageKind::Inserted),
            c.page_count(PageKind::Deleted),
            c.page_count(PageKind::Moved),
        ));
        for pc in &c.page_changes {
            match pc.kind {
                PageKind::Inserted => s.push_str(&format!("   - Inserted page {}\n", pc.new_page + 1)),
                PageKind::Deleted => s.push_str(&format!("   - Deleted old page {}\n", pc.old_page + 1)),
                PageKind::Moved => s.push_str(&format!("   - Moved old page {} to page {}\n", pc.old_page + 1, pc.new_page + 1)),
            }
        }
    }
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
    fn rewritten_page_between_matching_pages_keeps_a_visual_pair() {
        let mut old = words("unchanged introduction", 0);
        old.extend(words("old payment conditions", 1));
        old.extend(words("unchanged conclusion", 2));
        let mut new = words("unchanged introduction", 0);
        new.extend(words("entirely rewritten terms", 1));
        new.extend(words("unchanged conclusion", 2));
        let c = compare_with_pages(&old, 3, &new, 3);
        assert_eq!(c.page_pairs, [(0, 0), (1, 1), (2, 2)]);
        assert!(c.page_changes.is_empty());
        assert!(c.changes.iter().any(|c| c.old.page == 1 && c.new.page == 1));
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

    #[test]
    fn realigns_pages_after_insertions_deletions_and_moves() {
        let page_a = "contract overview section one terms and conditions apply";
        let page_b_old = "payment schedule thirty days net after invoice receipt";
        let page_b_new = "payment schedule fortyfive days net after invoice receipt";
        let page_c = "signatures and witness attestation final agreement clause";
        let page_x = "inserted appendix regulatory disclosure statement";

        // 1. Moved pages (A, B_old, C -> A, C, B_new): C is unchanged on its new page,
        // and B only reports "thirty" -> "fortyfive" between old page 1 and new page 2.
        let mut old_moved = words(page_a, 0);
        old_moved.extend(words(page_b_old, 1));
        old_moved.extend(words(page_c, 2));

        let mut new_moved = words(page_a, 0);
        new_moved.extend(words(page_c, 1));
        new_moved.extend(words(page_b_new, 2));

        let cm = compare(&old_moved, &new_moved);
        let got_moved: Vec<(Kind, usize, &str, usize, &str)> =
            cm.changes.iter().map(|ch| (ch.kind, ch.old.page, ch.old.text.as_str(), ch.new.page, ch.new.text.as_str())).collect();
        assert_eq!(got_moved, [(Kind::Replaced, 1, "thirty", 2, "fortyfive")]);
        assert_eq!(cm.page_pairs, [(0, 0), (2, 1), (1, 2)]);
        assert_eq!(cm.page_count(PageKind::Moved), 1);
        assert_eq!(cm.sync_old_page(1), 2);
        assert_eq!(cm.sync_old_page(2), 1);
        assert_eq!(cm.sync_new_page(1), 2);
        assert_eq!(cm.sync_new_page(2), 1);

        // 2. Inserted page (A, B_old, C -> A, X, B_new, C): X is inserted on new page 1,
        // and B_old (old page 1) realigns with B_new (new page 2).
        let mut new_ins = words(page_a, 0);
        new_ins.extend(words(page_x, 1));
        new_ins.extend(words(page_b_new, 2));
        new_ins.extend(words(page_c, 3));

        let ci = compare(&old_moved, &new_ins);
        let got_ins: Vec<(Kind, usize, &str, usize, &str)> =
            ci.changes.iter().map(|ch| (ch.kind, ch.old.page, ch.old.text.as_str(), ch.new.page, ch.new.text.as_str())).collect();
        assert_eq!(got_ins, [(Kind::Inserted, 0, "", 1, page_x), (Kind::Replaced, 1, "thirty", 2, "fortyfive"),]);
        assert_eq!(ci.page_pairs, [(0, 0), (1, 2), (2, 3)]);
        assert_eq!(ci.page_changes, [PageChange { kind: PageKind::Inserted, old_page: 0, new_page: 1 }]);
        assert_eq!(ci.sync_old_page(1), 0);
        assert_eq!(ci.sync_old_page(2), 1);
        assert_eq!(ci.sync_old_page(3), 2);

        // 3. Deleted page (A, X, B_old, C -> A, B_new, C): X is deleted from old page 1,
        // and B_old (old page 2) realigns with B_new (new page 1).
        let mut old_del = words(page_a, 0);
        old_del.extend(words(page_x, 1));
        old_del.extend(words(page_b_old, 2));
        old_del.extend(words(page_c, 3));

        let mut new_del = words(page_a, 0);
        new_del.extend(words(page_b_new, 1));
        new_del.extend(words(page_c, 2));

        let cd = compare(&old_del, &new_del);
        let got_del: Vec<(Kind, usize, &str, usize, &str)> =
            cd.changes.iter().map(|ch| (ch.kind, ch.old.page, ch.old.text.as_str(), ch.new.page, ch.new.text.as_str())).collect();
        assert_eq!(got_del, [(Kind::Deleted, 1, page_x, 0, ""), (Kind::Replaced, 2, "thirty", 1, "fortyfive"),]);
        assert_eq!(cd.page_pairs, [(0, 0), (2, 1), (3, 2)]);
        assert_eq!(cd.page_changes, [PageChange { kind: PageKind::Deleted, old_page: 1, new_page: 0 }]);
        assert_eq!(cd.sync_new_page(1), 0);
        assert_eq!(cd.sync_new_page(2), 1);
        assert_eq!(cd.sync_new_page(3), 2);
    }

    #[test]
    fn blank_page_changes_survive_identical_text() {
        let mut old = words("first unchanged page", 0);
        old.extend(words("last unchanged page", 1));
        let mut with_blank = words("first unchanged page", 0);
        with_blank.extend(words("last unchanged page", 2));
        for (a, a_pages, b, b_pages, kind) in [(&old, 2, &with_blank, 3, PageKind::Inserted), (&with_blank, 3, &old, 2, PageKind::Deleted)] {
            let c = compare_with_pages(a, a_pages, b, b_pages);
            assert!(c.changes.is_empty());
            assert_eq!(c.page_count(kind), 1, "{c:?}");
            assert!(!c.identical());
            let change = &c.page_changes[0];
            assert_eq!(if kind == PageKind::Inserted { change.new_page } else { change.old_page }, 1);
        }
    }

    #[test]
    fn word_pushed_across_page_break_is_not_reported_changed() {
        // Inserting "very" near the start of page 0 pushes "zeta" from the end of page 0
        // onto the start of page 1; only the single inserted word is reported.
        let mut old = words("alpha beta gamma delta epsilon zeta", 0);
        old.extend(words("eta theta iota kappa lambda mu", 1));
        let mut new = words("alpha very beta gamma delta epsilon", 0);
        new.extend(words("zeta eta theta iota kappa lambda mu", 1));

        let c = compare(&old, &new);
        let got: Vec<(Kind, usize, &str, usize, &str)> =
            c.changes.iter().map(|ch| (ch.kind, ch.old.page, ch.old.text.as_str(), ch.new.page, ch.new.text.as_str())).collect();
        assert_eq!(got, [(Kind::Inserted, 0, "", 0, "very")]);

        // Pure pagination reflow of identical words remains identical.
        let mut reflowed = words("alpha beta gamma delta epsilon", 0);
        reflowed.extend(words("zeta eta theta iota kappa lambda mu", 1));
        assert!(compare(&old, &reflowed).identical());

        // Inserting a blank or image-only page preserves the page insertion even with identical text.
        let mut old_two = words("alpha beta gamma delta epsilon", 0);
        old_two.extend(words("zeta eta theta iota kappa lambda", 1));
        let mut new_three = words("alpha beta gamma delta epsilon", 0);
        new_three.extend(words("zeta eta theta iota kappa lambda", 2));
        let cb = compare_with_pages(&old_two, 2, &new_three, 3);
        assert!(!cb.identical());
        assert_eq!(cb.page_changes, [PageChange { kind: PageKind::Inserted, old_page: 0, new_page: 1 }]);
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
