use std::collections::HashMap;

/// A paragraph of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub text: String,
    /// [x0, y0, x1, y1] on the displayed page in points (y up).
    pub rect: [f64; 4],
    /// Font size in points.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// Text fill colour as sRGB components in 0..=1 (black when unknown).
    pub color: [f64; 3],
    /// Counterclockwise baseline angle in user space (degrees).
    pub rotation: f64,
    /// Whether the source had a word space just before (`[0]`) and just after (`[1]`) `text`,
    /// which `text` leaves out. A block that continues this one on its line is a run of the same
    /// paragraph, and these say whether a space separates them (#4158).
    pub space_around: [bool; 2],
}

/// An image of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// "png" or "jpg".
    pub ext: &'static str,
    /// Shared: a picture drawn in many places is held once.
    pub bytes: std::sync::Arc<[u8]>,
    pub rect: [f64; 4],
    /// Width and height before rotation, in points.
    pub size: [f64; 2],
    /// Counterclockwise rotation in user space (degrees).
    pub rotation: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub blocks: Vec<Block>,
    pub images: Vec<Image>,
    /// The colour the whole page is painted before its content (a page background), as sRGB
    /// components in 0..=1; `None` for a page with no background of its own.
    pub background: Option<[f64; 3]>,
}

/// One cell of a detected table. `span` is how many grid columns the cell covers (a header
/// row that stretches over sub-columns).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// Text fill colour as sRGB components in 0..=1.
    pub color: [f64; 3],
    pub span: usize,
}

/// A table detected from the page's text layout: rows of cells, and the left edge of each
/// grid column (user space, y up) so the writers can size the columns.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rect: [f64; 4],
    pub cols: Vec<f64>,
    pub rows: Vec<Vec<Cell>>,
}

/// What goes into the output, in order.
pub(crate) enum Item<'a> {
    /// A paragraph's runs (at least one) and its heading level.
    Para(Vec<Run<'a>>, u8),
    /// Owned: tables are detected per page inside [`items`], so they cannot borrow.
    Table(Table),
    Img(&'a Image),
    PageBreak,
}

#[cfg(test)]
thread_local! {
    /// Entries read to find a font size's or a colour's slot, for the tests that keep it constant.
    pub(crate) static LOOKUP_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts `n` entries read by a lookup (tests only).
#[inline]
pub(crate) fn count_lookup_steps(_n: usize) {
    #[cfg(test)]
    LOOKUP_STEPS.with(|c| c.set(c.get().saturating_add(_n)));
}

/// The body text size: the size most of the document's characters are set in.
pub(crate) fn body_size(pages: &[Page]) -> f64 {
    // Sizes in first-seen order (which breaks ties), and where each one is in it.
    let mut by: Vec<(i64, usize)> = Vec::new();
    let mut slot: HashMap<i64, usize> = HashMap::new();
    for b in pages.iter().flat_map(|p| &p.blocks) {
        let k = (b.size * 2.0).round() as i64;
        count_lookup_steps(1);
        let i = *slot.entry(k).or_insert_with(|| {
            by.push((k, 0));
            by.len() - 1
        });
        if let Some(e) = by.get_mut(i) {
            e.1 = e.1.saturating_add(b.text.len());
        }
    }
    by.into_iter().max_by_key(|e| e.1).map_or(11.0, |e| e.0 as f64 / 2.0)
}

/// Heading level for a block: 1 for ≥ 1.6× the body size, 2 for ≥ 1.25×, 0 for body text.
fn level(b: &Block, body: f64) -> u8 {
    let short = b.text.len() < 200;
    if short && b.size >= body * 1.6 {
        1
    } else if short && (b.size >= body * 1.25 || (b.bold && b.size >= body && b.text.len() < 80 && !b.text.ends_with('.'))) {
        2
    } else {
        0
    }
}

/// How far two cell left edges may drift (points) and still be the same grid column.
const COL_TOL: f64 = 4.0;

/// More grid columns than this isn't a table. It is Word's limit: a .docx whose table has 64 or
/// more columns doesn't open. It also bounds a table's size: every row is materialized to all
/// its columns, so a hostile page laid out as a staircase of blocks would otherwise make
/// (blocks / 2)² cells.
pub(crate) const MAX_COLS: usize = 63;

/// Two blocks in a row this close (as a fraction of their font size) are one line of running
/// text that changed font or colour, such as a link inside a sentence, not two table cells.
const INLINE_GAP: f64 = 0.5;

/// How many blocks ending near a block's start are looked at to find the one it continues; a
/// page laid out so that many end at the same place stays quick.
const CONTINUATION_PROBE: usize = 64;

/// Per block, whether it continues another on the same line: it starts where that one ends (up to
/// [`INLINE_GAP`] later, or a point earlier for rounding), so it is the same line of text running
/// on (a link inside a sentence, a superscript), not a cell of its own.
fn continuations(blocks: &[Block]) -> Vec<bool> {
    continued_from(blocks).into_iter().map(|from| from.is_some()).collect()
}

/// Per block, the block it continues on the same line (see [`continuations`]): of those it could
/// continue, the one that ends nearest to where it starts.
fn continued_from(blocks: &[Block]) -> Vec<Option<usize>> {
    let mut by_end: Vec<(f64, usize)> = blocks.iter().enumerate().map(|(i, b)| (b.rect[2], i)).collect();
    by_end.sort_by(|a, b| a.0.total_cmp(&b.0));
    blocks
        .iter()
        .enumerate()
        .map(|(bi, b)| {
            let k = by_end.partition_point(|(end, _)| *end < b.rect[0] - INLINE_GAP * b.size);
            by_end
                .get(k..)
                .unwrap_or_default()
                .iter()
                .take_while(|(end, _)| *end <= b.rect[0] + 1.0)
                .take(CONTINUATION_PROBE)
                .filter_map(|(_, ai)| blocks.get(*ai).filter(|_| *ai != bi).map(|a| (*ai, a)))
                .filter(|(_, a)| a.rect[3].min(b.rect[3]) - a.rect[1].max(b.rect[1]) > 0.5 * (a.rect[3] - a.rect[1]).min(b.rect[3] - b.rect[1]))
                .min_by(|(_, a), (_, c)| (a.rect[2] - b.rect[0]).abs().total_cmp(&(c.rect[2] - b.rect[0]).abs()))
                .map(|(ai, _)| ai)
        })
        .collect()
}

/// A gap between two runs wider than this (as a fraction of the font size) is a word space the
/// text left out. It is the gap the reading-block grouping uses for the same purpose.
const WORD_GAP: f64 = 0.12;

/// One run of a paragraph: a block, and the text it contributes (its own, plus the word space
/// before the next run when the source had one there or the page shows a gap, but the text has
/// none).
pub(crate) struct Run<'a> {
    pub(crate) block: &'a Block,
    pub(crate) text: String,
}

/// A page's paragraphs, each a list of runs from left to right along its line. A block that
/// continues another on the same line ([`continued_from`]: an italic case name, a link, a
/// superscript) is a run in its own style inside that block's paragraph, not a paragraph of its
/// own (#4158). Blocks marked in `skip` (table cells, a slide title) are left out and join
/// nothing. Paragraphs come in the order of their first runs in `blocks`.
pub(crate) fn paragraphs<'a>(blocks: &'a [Block], skip: &[bool]) -> Vec<Vec<Run<'a>>> {
    let skipped = |i: usize| skip.get(i).copied().unwrap_or(false);
    let from = continued_from(blocks);
    // Left to right. A block joins only one that starts strictly further left (in `total_cmp`'s
    // order, NaN included), so that one is visited first and has its paragraph already, and the
    // links cannot form a cycle.
    let mut order: Vec<(f64, usize)> = blocks.iter().enumerate().filter(|(i, _)| !skipped(*i)).map(|(i, b)| (b.rect[0], i)).collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    // Each block's paragraph, named by its first block, and each paragraph's blocks in order.
    let mut head: Vec<usize> = (0..blocks.len()).collect();
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for &(x0, bi) in &order {
        let Some(b) = blocks.get(bi) else { continue };
        let joined = from
            .get(bi)
            .copied()
            .flatten()
            .filter(|ai| !skipped(*ai))
            .filter(|ai| blocks.get(*ai).is_some_and(|a| a.rect[0].total_cmp(&x0).is_lt() && (a.rotation - b.rotation).abs() <= 0.1))
            .and_then(|ai| head.get(ai).copied());
        let h = joined.unwrap_or(bi);
        if let Some(slot) = head.get_mut(bi) {
            *slot = h;
        }
        if let Some(m) = members.get_mut(h) {
            m.push(bi);
        }
    }
    members
        .into_iter()
        .filter(|m| !m.is_empty())
        .map(|m| {
            let mut runs: Vec<Run<'a>> = Vec::with_capacity(m.len());
            for b in m.into_iter().filter_map(|bi| blocks.get(bi)) {
                if let Some(prev) = runs.last_mut()
                    && !prev.text.ends_with(char::is_whitespace)
                    && !b.text.starts_with(char::is_whitespace)
                    && (prev.block.space_around[1] || b.space_around[0] || b.rect[0] - prev.block.rect[2] > b.size * WORD_GAP)
                {
                    prev.text.push(' ');
                }
                runs.push(Run { block: b, text: b.text.clone() });
            }
            runs
        })
        .collect()
}

/// A paragraph's heading level: the lowest of its runs', so body text that runs on after a bold
/// lead-in on the same line keeps the paragraph body text. A marker set smaller than the first run
/// (a footnote number, "st" in "1st") does not count, so a heading keeps its footnote marker.
fn para_level(runs: &[Run], body: f64) -> u8 {
    let Some(first) = runs.first() else { return 0 };
    let marker = |r: &Run| r.block.size < first.block.size * 0.9 && r.block.text.trim().chars().count() <= 3;
    runs.iter().filter(|r| !marker(r)).map(|r| level(r.block, body)).min().unwrap_or(0)
}

/// A cell is short: taller blocks are body text (or a multi-column layout), not table cells.
fn is_cell_like(b: &Block) -> bool {
    (b.rect[3] - b.rect[1]) <= b.size * 5.0 && !b.text.trim().is_empty()
}

/// Detect tables in a page's blocks from their text layout: columns are left edges shared by
/// several short blocks, rows are blocks whose vertical spans overlap. Returns the tables and,
/// per block, whether a table consumed it (so `items` can drop it from the paragraph flow).
pub(crate) fn tables(blocks: &[Block]) -> (Vec<Table>, Vec<bool>) {
    let mut consumed = vec![false; blocks.len()];
    // Candidate columns: cluster the left edges of cell-like blocks.
    let mut xs: Vec<(f64, usize)> = blocks.iter().enumerate().filter(|(_, b)| is_cell_like(b)).map(|(i, b)| (b.rect[0], i)).collect();
    xs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cols: Vec<(f64, Vec<usize>)> = Vec::new(); // (left edge, member block indices)
    for (x, i) in xs {
        match cols.last_mut() {
            Some((e, m)) if (x - *e).abs() <= COL_TOL => {
                *e = (*e * m.len() as f64 + x) / (m.len() + 1) as f64;
                m.push(i);
            }
            _ => cols.push((x, vec![i])),
        }
    }
    let cols: Vec<(f64, Vec<usize>)> = cols.into_iter().filter(|(_, m)| m.len() >= 2).collect();
    if cols.len() < 2 {
        return (Vec::new(), consumed);
    }
    // Each block's column (an index into `cols`), so lookups stay linear on pages with many blocks.
    let mut col_of: Vec<Option<usize>> = vec![None; blocks.len()];
    for (ci, (_, members)) in cols.iter().enumerate() {
        for &i in members {
            if let Some(c) = col_of.get_mut(i) {
                *c = Some(ci);
            }
        }
    }
    // The column whose edge is within tolerance of `x` (edges are in ascending order).
    let column_at = |x: f64| -> Option<usize> {
        let k = cols.partition_point(|(e, _)| *e < x - COL_TOL);
        cols.get(k).filter(|(e, _)| (*e - x).abs() <= COL_TOL).map(|_| k)
    };
    // Assign every cell-like block to its nearest column (within tolerance).
    let mut rows: Vec<Vec<(usize, usize)>> = Vec::new(); // (column index, block index), top to bottom
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by(|a, b| blocks[*b].rect[3].total_cmp(&blocks[*a].rect[3]).then(blocks[*a].rect[0].total_cmp(&blocks[*b].rect[0])));
    for &bi in &order {
        let b = &blocks[bi];
        if !is_cell_like(b) {
            continue;
        }
        let Some(ci) = column_at(b.rect[0]) else { continue };
        let joins = rows
            .last()
            .and_then(|row| row.first())
            .map(|(_, fi)| {
                let rb = &blocks[*fi];
                let overlap = rb.rect[3].min(b.rect[3]) - rb.rect[1].max(b.rect[1]);
                overlap > 0.5 * (rb.rect[3] - rb.rect[1]).min(b.rect[3] - b.rect[1])
            })
            .unwrap_or(false);
        if joins {
            if let Some(row) = rows.last_mut() {
                row.push((ci, bi));
            }
        } else {
            rows.push(vec![(ci, bi)]);
        }
    }
    // Runs of consecutive rows that have two or more distinct columns are tables.
    let mut out = Vec::new();
    let flush = |run: &mut Vec<Vec<(usize, usize)>>, out: &mut Vec<Table>, consumed: &mut Vec<bool>| {
        if run.len() < 2 {
            run.clear();
            return;
        }
        let mut used: Vec<usize> = run.iter().flatten().map(|(_, i)| *i).collect();
        used.sort_unstable();
        used.dedup();
        let mut edges: Vec<f64> = used.iter().filter_map(|&i| col_of.get(i).copied().flatten()).filter_map(|ci| cols.get(ci)).map(|c| c.0).collect();
        edges.sort_by(f64::total_cmp);
        edges.dedup_by(|a, b| (*a - *b).abs() <= COL_TOL);
        let ncols = edges.len();
        let ok = (ncols >= 3 || (ncols == 2 && run.len() >= 3)) && ncols <= MAX_COLS;
        if !ok {
            run.clear();
            return;
        }
        let right = used.iter().map(|&i| blocks[i].rect[2]).fold(f64::MIN, f64::max) + COL_TOL;
        let mut rows_out = Vec::new();
        let mut slots: Vec<Option<(Cell, Vec<usize>)>> = vec![None; ncols];
        for row in run.drain(..) {
            for slot in slots.iter_mut() {
                *slot = None;
            }
            for (_ci, bi) in row {
                let b = &blocks[bi];
                let Some(slot) = edges.iter().position(|e| (*e - b.rect[0]).abs() <= COL_TOL) else { continue };
                let span = edges[slot + 1..].iter().filter(|e| **e > b.rect[0] + COL_TOL && **e < b.rect[2] - COL_TOL).count() + 1;
                let cell = Cell { text: b.text.trim().to_string(), size: b.size, bold: b.bold, italic: b.italic, color: b.color, span };
                match slots.get_mut(slot) {
                    Some(Some((c, sources))) => {
                        c.text.push(' ');
                        c.text.push_str(&cell.text);
                        sources.push(bi);
                    }
                    Some(empty @ None) => *empty = Some((cell, vec![bi])),
                    None => continue,
                }
            }
            let mut materialized = Vec::with_capacity(ncols);
            let mut column = 0;
            while column < ncols {
                match slots.get_mut(column).and_then(Option::take) {
                    Some((mut cell, sources)) => {
                        // A spanning cell can skip later slots. Only consume source blocks
                        // when their text is emitted, leaving covered text in paragraph flow.
                        for bi in sources {
                            if let Some(taken) = consumed.get_mut(bi) {
                                *taken = true;
                            }
                        }
                        cell.span = cell.span.min(ncols - column).max(1);
                        column += cell.span;
                        materialized.push(cell);
                    }
                    None => {
                        column += 1;
                        materialized.push(Cell { text: String::new(), size: 11.0, bold: false, italic: false, color: [0.0; 3], span: 1 });
                    }
                }
            }
            while materialized.last().is_some_and(|cell| cell.text.is_empty()) && materialized.iter().any(|cell| cell.span > 1) {
                materialized.pop();
            }
            rows_out.push(materialized);
        }
        let rect = [
            edges[0],
            used.iter().map(|&i| blocks[i].rect[1]).fold(f64::MAX, f64::min),
            right,
            used.iter().map(|&i| blocks[i].rect[3]).fold(f64::MIN, f64::max),
        ];
        out.push(Table { rect, cols: edges, rows: rows_out });
    };
    // Runs of consecutive multi-column rows are tables. Rows on either side of a run that
    // hold a single cell wide enough to span two columns (a header band, a totals row) join
    // the table; other single-column rows (captions, following text) stay paragraphs.
    let spans_run = |edges: &[f64], bi: usize| -> bool {
        let b = &blocks[bi];
        edges.iter().any(|e| (*e - b.rect[0]).abs() <= COL_TOL) && edges.iter().any(|e| *e > b.rect[0] + COL_TOL && *e < b.rect[2] - COL_TOL)
    };
    let run_edges = |run: &[Vec<(usize, usize)>]| -> Vec<f64> {
        let mut es: Vec<f64> =
            run.iter().flatten().filter_map(|(_, i)| col_of.get(*i).copied().flatten()).filter_map(|ci| cols.get(ci)).map(|c| c.0).collect();
        es.sort_by(f64::total_cmp);
        es.dedup_by(|a, b| (*a - *b).abs() <= COL_TOL);
        es
    };
    let cont = continuations(blocks);
    let qual: Vec<bool> = rows
        .iter()
        .map(|row| {
            // A block that continues another adds no column of its own, and the cells of a table row
            // sit side by side: blocks that overlap are a paragraph's lines, not cells (#2806).
            let cells: Vec<(usize, usize)> = row.iter().copied().filter(|(_, bi)| !cont.get(*bi).copied().unwrap_or(false)).collect();
            let mut spans: Vec<[f64; 2]> = cells.iter().filter_map(|(_, bi)| blocks.get(*bi)).map(|b| [b.rect[0], b.rect[2]]).collect();
            spans.sort_by(|a, b| a[0].total_cmp(&b[0]));
            let apart = spans.windows(2).all(|w| w[1][0] >= w[0][1] - COL_TOL);
            let mut cs: Vec<usize> = cells.iter().map(|(c, _)| *c).collect();
            cs.sort_unstable();
            cs.dedup();
            apart && cs.len() >= 2
        })
        .collect();
    let mut i = 0;
    while i < rows.len() {
        if !qual[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < rows.len() && qual[i] {
            i += 1;
        }
        let end = i; // run = rows[start..end]
        let mut pieces: Vec<Vec<(usize, usize)>> = Vec::new();
        if start > 0 && rows[start - 1].len() == 1 && spans_run(&run_edges(&rows[start..end]), rows[start - 1][0].1) {
            pieces.push(rows[start - 1].clone());
        }
        pieces.extend(rows[start..end].iter().cloned());
        if end < rows.len() && rows[end].len() == 1 && spans_run(&run_edges(&rows[start..end]), rows[end][0].1) {
            pieces.push(rows[end].clone());
        }
        flush(&mut pieces, &mut out, &mut consumed);
    }
    (out, consumed)
}

pub(crate) fn items(pages: &[Page]) -> Vec<Item<'_>> {
    let body = body_size(pages);
    let mut out = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        if i > 0 {
            out.push(Item::PageBreak);
        }
        let (tables, consumed) = tables(&p.blocks);
        // Blocks and images by their top edge, top to bottom.
        // A paragraph sits where its first run does: its other runs follow along the same line.
        let mut parts: Vec<(f64, f64, Item)> = paragraphs(&p.blocks, &consumed)
            .into_iter()
            .filter_map(|runs| {
                let first = runs.first()?.block;
                let lvl = para_level(&runs, body);
                Some((first.rect[3], first.rect[0], Item::Para(runs, lvl)))
            })
            .collect();
        parts.extend(tables.into_iter().map(|t| (t.rect[3], t.rect[0], Item::Table(t))));
        parts.extend(p.images.iter().map(|im| (im.rect[3], im.rect[0], Item::Img(im))));
        parts.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
        out.extend(parts.into_iter().map(|x| x.2));
    }
    out
}
