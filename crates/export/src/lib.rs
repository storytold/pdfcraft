//! pdfcraft-export — Export a PDF ▸ Word, HTML, RTF (L4).
//!
//! The engine reduces each page to [`Page`]: paragraphs (text, box, size, bold/italic) and
//! images (encoded bytes and box), in reading order. The writers turn that into a flowing
//! document: explicit H1–H6/P structure levels take precedence over font-size inference. Word keeps pages with
//! rotated text, or text overlapped by artwork or watermark-sized text, as positioned, editable
//! shapes to avoid reflowing overlays; other pages flow with images inline.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod pptx;
mod zip;

pub use pptx::pptx;
pub use zip::Zip;

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
    /// The PDF `/BaseFont` of the text (empty when unknown). Word output names the matching family
    /// so the text is set in a face of the source's widths, not Word's default one.
    pub font: String,
    /// Text fill colour as sRGB components in 0..=1 (black when unknown).
    pub color: [f64; 3],
    /// Counterclockwise baseline angle in user space (degrees).
    pub rotation: f64,
    /// Explicit structure level: Some(0) is body, Some(1..=6) is a heading; None infers from size.
    pub heading_level: Option<u8>,
    /// How the source lays the paragraph out (see [`Layout`]).
    pub layout: Layout,
}

/// How the lines of a paragraph line up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// The source's line structure for a paragraph. Formats that place text at fixed positions use it
/// to keep the source's line breaks, alignment and line pitch; flowing formats ignore it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    /// The text of each source row (empty when unknown, or for a single row).
    pub lines: Vec<String>,
    pub align: Align,
    /// Distance between successive baselines in points (0 when unknown).
    pub leading: f64,
    /// Where each source row sits, `[x0, y0, x1, y1]` on the page, y up, like [`Block::rect`]
    /// (empty when unknown). When there is one per row, fixed-position formats place each row on
    /// its own, so no row depends on how the reader wraps or spaces lines.
    pub rects: Vec<[f64; 4]>,
}

/// An image of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// "png" or "jpg".
    pub ext: &'static str,
    pub bytes: Vec<u8>,
    pub rect: [f64; 4],
    /// Width and height before rotation, in points.
    pub size: [f64; 2],
    /// Counterclockwise rotation in user space (degrees).
    pub rotation: f64,
}

/// A filled rectangle of a page (a table's cell background, a shaded band).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    /// [x0, y0, x1, y1] on the displayed page in points (y up).
    pub rect: [f64; 4],
    /// sRGB components in 0..=1.
    pub color: [f64; 3],
}

/// A horizontal or vertical line of a page (a table border, a rule): `rect` has no extent across
/// it, `width` is its thickness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    /// [x0, y0, x1, y1] on the displayed page in points (y up).
    pub rect: [f64; 4],
    pub width: f64,
    /// sRGB components in 0..=1.
    pub color: [f64; 3],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub blocks: Vec<Block>,
    pub images: Vec<Image>,
    /// Filled rectangles, in drawing order (later ones are painted over earlier ones).
    pub fills: Vec<Fill>,
    /// Ruling lines, in drawing order.
    pub rules: Vec<Rule>,
    /// Dots, wedges and other outlines that are not rectangles or straight lines, in drawing order.
    pub shapes: Vec<Shape>,
}

/// One segment of a [`Shape`]; points are on the displayed page in points (y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move([f64; 2]),
    Line([f64; 2]),
    /// A cubic Bezier: two control points, then the end point.
    Curve([f64; 2], [f64; 2], [f64; 2]),
    Close,
}

/// A filled and/or stroked outline of a page: a bullet dot, a rating light, a pie wedge.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// [x0, y0, x1, y1] box of every point of `path` on the displayed page in points (y up).
    pub rect: [f64; 4],
    pub path: Vec<Seg>,
    /// sRGB components in 0..=1, when filled.
    pub fill: Option<[f64; 3]>,
    /// Line colour (sRGB, 0..=1) and thickness in points, when the outline is drawn.
    pub stroke: Option<([f64; 3], f64)>,
}

/// One side of a cell the source draws a line along.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    /// Thickness in points.
    pub width: f64,
    pub color: [f64; 3],
}

/// One cell of a detected table. `span` is how many grid columns the cell covers (a header
/// row that stretches over sub-columns).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The PDF `/BaseFont` of the cell's text (empty when unknown).
    pub font: String,
    /// Text fill colour as sRGB components in 0..=1.
    pub color: [f64; 3],
    pub span: usize,
    /// The background the source paints under the cell, when it paints one.
    pub fill: Option<[f64; 3]>,
    /// Lines the source draws along the cell's top, left, bottom and right edges.
    pub borders: [Option<Border>; 4],
}

/// A table detected from the page's text layout: rows of cells, and the left edge of each
/// grid column (user space, y up) so the writers can size the columns.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rect: [f64; 4],
    pub cols: Vec<f64>,
    pub rows: Vec<Vec<Cell>>,
    /// Per row, the lowest and highest edge of its text (y up).
    pub spans: Vec<[f64; 2]>,
}

/// What goes into the output, in order.
enum Item<'a> {
    Para(&'a Block, u8),
    /// Owned: tables are detected per page inside [`items`], so they cannot borrow.
    Table(Table),
    Img(&'a Image),
    PageBreak,
}

/// The body text size: the size most of the document's characters are set in.
fn body_size(pages: &[Page]) -> f64 {
    let mut by: Vec<(i64, usize)> = Vec::new();
    for b in pages.iter().flat_map(|p| &p.blocks) {
        let k = (b.size * 2.0).round() as i64;
        match by.iter_mut().find(|e| e.0 == k) {
            Some(e) => e.1 += b.text.len(),
            None => by.push((k, b.text.len())),
        }
    }
    by.into_iter().max_by_key(|e| e.1).map_or(11.0, |e| e.0 as f64 / 2.0)
}

/// Explicit structure level, or size-based inference (1/2/0) for untagged content.
fn level(b: &Block, body: f64) -> u8 {
    if let Some(level) = b.heading_level.filter(|level| *level <= 6) {
        return level;
    }
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
const MAX_COLS: usize = 63;
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
                .filter_map(|(_, ai)| blocks.get(*ai).filter(|_| *ai != bi))
                .any(|a| a.rect[3].min(b.rect[3]) - a.rect[1].max(b.rect[1]) > 0.5 * (a.rect[3] - a.rect[1]).min(b.rect[3] - b.rect[1]))
        })
        .collect()
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
        edges.sort_by(|a, b| a.total_cmp(b));
        edges.dedup_by(|a, b| (*a - *b).abs() <= COL_TOL);
        let ncols = edges.len();
        let ok = (ncols >= 3 || (ncols == 2 && run.len() >= 3)) && ncols <= MAX_COLS;
        if !ok {
            run.clear();
            return;
        }
        let right = used.iter().map(|&i| blocks[i].rect[2]).fold(f64::MIN, f64::max) + COL_TOL;
        let mut rows_out = Vec::new();
        let mut slots: Vec<Option<Cell>> = vec![None; ncols];
        let mut spans = Vec::new();
        for row in run.drain(..) {
            for slot in slots.iter_mut() {
                *slot = None;
            }
            spans.push([
                row.iter().map(|(_, bi)| blocks[*bi].rect[1]).fold(f64::MAX, f64::min),
                row.iter().map(|(_, bi)| blocks[*bi].rect[3]).fold(f64::MIN, f64::max),
            ]);
            for (_ci, bi) in row {
                let b = &blocks[bi];
                let Some(slot) = edges.iter().position(|e| (*e - b.rect[0]).abs() <= COL_TOL) else { continue };
                let span = edges[slot + 1..].iter().filter(|e| **e > b.rect[0] + COL_TOL && **e < b.rect[2] - COL_TOL).count() + 1;
                let cell = Cell {
                    text: b.text.trim().to_string(),
                    size: b.size,
                    bold: b.bold,
                    italic: b.italic,
                    font: b.font.clone(),
                    color: b.color,
                    span,
                    fill: None,
                    borders: [None; 4],
                };
                match &mut slots[slot] {
                    Some(c) => {
                        c.text.push(' ');
                        c.text.push_str(&cell.text);
                    }
                    None => slots[slot] = Some(cell),
                }
                consumed[bi] = true;
            }
            let mut materialized = Vec::with_capacity(ncols);
            let mut column = 0;
            while column < ncols {
                match slots[column].take() {
                    Some(mut cell) => {
                        cell.span = cell.span.min(ncols - column).max(1);
                        column += cell.span;
                        materialized.push(cell);
                    }
                    None => {
                        column += 1;
                        materialized.push(Cell {
                            text: String::new(),
                            size: 11.0,
                            bold: false,
                            italic: false,
                            font: String::new(),
                            color: [0.0; 3],
                            span: 1,
                            fill: None,
                            borders: [None; 4],
                        });
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
        out.push(Table { rect, cols: edges, rows: rows_out, spans });
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
        es.sort_by(|a, b| a.total_cmp(b));
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

fn is_white(c: [f64; 3]) -> bool {
    c.iter().all(|v| v.is_nan() || *v >= 0.98)
}

/// The most cell and rule comparisons [`style_table`] makes for one table, so a page with
/// thousands of lines over a huge grid stays quick.
const STYLE_BUDGET: usize = 4_000_000;

/// The index of the interval in `spans` that holds `v`, the one `v` is closest to the middle of.
fn nearest_span(spans: &[[f64; 2]], v: f64) -> Option<usize> {
    spans
        .iter()
        .enumerate()
        .filter(|(_, s)| s[0] <= v && v <= s[1])
        .min_by(|a, b| (v - (a.1[0] + a.1[1]) / 2.0).abs().total_cmp(&(v - (b.1[0] + b.1[1]) / 2.0).abs()))
        .map(|(i, _)| i)
}

/// Give a detected table the borders and backgrounds its page draws. The grid is only inferred
/// from the text, so a line is attached to a cell edge only when it runs along it (between the
/// text of two rows, beside a column's left edge, or just outside the table's first and last
/// row and column) and covers the cell; a cell is shaded only when a fill covers where its text
/// starts. Lines that merely align with the text are not turned into borders.
pub(crate) fn style_table(t: &mut Table, fills: &[Fill], rules: &[Rule]) {
    let (n, nrows) = (t.cols.len(), t.rows.len());
    if n == 0 || nrows == 0 || t.spans.len() != nrows || (fills.is_empty() && rules.is_empty()) {
        return;
    }
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let size = t.rows.iter().flatten().map(|c| c.size).filter(|s| s.is_finite()).fold(0.0, f64::max);
    let margin = (size * 0.8).clamp(4.0, 14.0);
    // Where a horizontal line between rows k-1 and k (or above the first, below the last) may be.
    let horizontal: Vec<[f64; 2]> = (0..=nrows)
        .map(|k| match (k.checked_sub(1).and_then(|p| t.spans.get(p)), t.spans.get(k)) {
            (Some(above), Some(below)) => {
                let (a, b) = (below[1], above[0]);
                [a.min(b) - 1.0, a.max(b) + 1.0]
            }
            (None, Some(first)) => [first[1] - 1.0, first[1] + margin],
            (Some(last), None) => [last[0] - margin, last[0] + 1.0],
            (None, None) => [f64::NAN; 2],
        })
        .collect();
    // Where a vertical line on the left of column j (right of the last, for j = n) may be.
    let vertical: Vec<[f64; 2]> = (0..=n)
        .map(|j| {
            let at = edges.get(j).copied().unwrap_or(f64::NAN);
            if j == 0 {
                [at - 12.0, at + 1.0]
            } else if j == n {
                [at - COL_TOL - 1.0, at + 12.0]
            } else {
                let gap = at - edges.get(j - 1).copied().unwrap_or(f64::NAN);
                [at - (gap / 2.0).clamp(1.0, 12.0), at + 1.0]
            }
        })
        .collect();
    // The grid columns each cell covers, and the middle of each row.
    let columns: Vec<Vec<[usize; 2]>> = t
        .rows
        .iter()
        .map(|row| {
            let mut c = 0usize;
            row.iter()
                .map(|cell| {
                    let at = [c.min(n), c.saturating_add(cell.span.max(1)).min(n)];
                    c = c.saturating_add(cell.span.max(1));
                    at
                })
                .collect()
        })
        .collect();
    let middle: Vec<f64> = t.spans.iter().map(|s| (s[0] + s[1]) / 2.0).collect();
    let mut budget = STYLE_BUDGET;
    for rule in rules {
        let border = Some(Border { width: rule.width, color: rule.color });
        let (dx, dy) = (rule.rect[2] - rule.rect[0], rule.rect[3] - rule.rect[1]);
        if dy <= dx {
            let Some(k) = nearest_span(&horizontal, (rule.rect[1] + rule.rect[3]) / 2.0) else { continue };
            for (row, side) in [(k.checked_sub(1), 2usize), (Some(k), 0)] {
                let Some(row) = row.filter(|r| *r < nrows) else { continue };
                let (Some(cells), Some(cols)) = (t.rows.get_mut(row), columns.get(row)) else { continue };
                budget = budget.saturating_sub(cells.len().max(1));
                for (cell, [a, b]) in cells.iter_mut().zip(cols) {
                    let centre = (edges.get(*a).copied().unwrap_or(f64::NAN) + edges.get(*b).copied().unwrap_or(f64::NAN)) / 2.0;
                    if rule.rect[0] <= centre + 1.0 && rule.rect[2] >= centre - 1.0 {
                        cell.borders[side] = border;
                    }
                }
            }
        } else {
            let Some(j) = nearest_span(&vertical, (rule.rect[0] + rule.rect[2]) / 2.0) else { continue };
            for (row, cells) in t.rows.iter_mut().enumerate() {
                if budget == 0 {
                    break;
                }
                budget = budget.saturating_sub(1);
                let y = middle.get(row).copied().unwrap_or(f64::NAN);
                if !(rule.rect[1] - 1.0 <= y && y <= rule.rect[3] + 1.0) {
                    continue;
                }
                for (cell, [a, b]) in cells.iter_mut().zip(columns.get(row).map(Vec::as_slice).unwrap_or_default()) {
                    if *b == j {
                        cell.borders[3] = border;
                    }
                    if *a == j {
                        cell.borders[1] = border;
                    }
                }
            }
        }
        if budget == 0 {
            break;
        }
    }
    for (row, cells) in t.rows.iter_mut().enumerate() {
        let y = middle.get(row).copied().unwrap_or(f64::NAN);
        for (cell, [a, _]) in cells.iter_mut().zip(columns.get(row).map(Vec::as_slice).unwrap_or_default()) {
            if budget == 0 {
                return;
            }
            budget = budget.saturating_sub(fills.len().max(1));
            let x = edges.get(*a).copied().unwrap_or(f64::NAN) + 1.0;
            cell.fill = fills
                .iter()
                .rev()
                .find(|f| f.rect[0] - 0.5 <= x && x <= f.rect[2] && f.rect[1] - 0.5 <= y && y <= f.rect[3] + 0.5)
                .map(|f| f.color)
                .filter(|c| !is_white(*c));
        }
    }
}

/// The fill painted over the middle of `rect`, if one is and it is not white (the page).
fn background(fills: &[Fill], rect: [f64; 4]) -> Option<[f64; 3]> {
    let (x, y) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    fills.iter().rev().find(|f| f.rect[0] <= x && x <= f.rect[2] && f.rect[1] <= y && y <= f.rect[3]).map(|f| f.color).filter(|c| !is_white(*c))
}

fn items(pages: &[Page]) -> Vec<Item<'_>> {
    let body = body_size(pages);
    let mut out = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        if i > 0 {
            out.push(Item::PageBreak);
        }
        let (tables, consumed) = tables(&p.blocks);
        // Blocks and images by their top edge, top to bottom.
        let mut parts: Vec<(f64, f64, Item)> = p
            .blocks
            .iter()
            .enumerate()
            .filter(|(bi, _)| !consumed.get(*bi).copied().unwrap_or(false))
            .map(|(_, b)| (b.rect[3], b.rect[0], Item::Para(b, level(b, body))))
            .collect();
        parts.extend(tables.into_iter().map(|mut t| {
            style_table(&mut t, &p.fills, &p.rules);
            (t.rect[3], t.rect[0], Item::Table(t))
        }));
        parts.extend(p.images.iter().map(|im| (im.rect[3], im.rect[0], Item::Img(im))));
        parts.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
        out.extend(parts.into_iter().map(|x| x.2));
    }
    out
}

/// Whether a character may appear in the output. XML 1.0 forbids the C0 controls other than tab,
/// line feed and carriage return, and U+FFFE/U+FFFF. Text extracted from PDFs often holds them
/// (fonts without a usable `/ToUnicode`), and Word refuses a whole document over one (#72).
fn allowed(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || !(c.is_ascii_control() || c == '\u{FFFE}' || c == '\u{FFFF}')
}

pub(crate) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}

/// One HTML file: headings and paragraphs, tables as real `<table>` elements, images inline, a
/// rule between pages.
pub fn html(pages: &[Page], title: &str) -> String {
    let mut s = format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>body{{max-width:46em;margin:2em auto;font-family:sans-serif;line-height:1.45}}\
img{{max-width:100%}}hr{{border:0;border-top:1px solid #ccc;margin:2em 0}}\
table{{border-collapse:collapse;margin:1em 0}}td,th{{border:1px solid #999;padding:2px 6px;vertical-align:top}}</style>\n</head>\n<body>\n",
        esc(title)
    );
    for it in items(pages) {
        match it {
            Item::Para(b, lvl) => {
                let mut t = esc(&b.text);
                if b.italic {
                    t = format!("<em>{t}</em>");
                }
                if b.bold && lvl == 0 {
                    t = format!("<strong>{t}</strong>");
                }
                let style = hex(b.color).map_or(String::new(), |h| format!(" style=\"color:#{h}\""));
                match lvl {
                    0 => s.push_str(&format!("<p{style}>{t}</p>\n")),
                    l => s.push_str(&format!("<h{l}{style}>{t}</h{l}>\n")),
                }
            }
            Item::Table(t) => {
                s.push_str("<table>\n");
                for row in &t.rows {
                    s.push_str("<tr>");
                    for c in row {
                        let mut body = esc(&c.text);
                        if c.italic {
                            body = format!("<em>{body}</em>");
                        }
                        if c.bold {
                            body = format!("<strong>{body}</strong>");
                        }
                        let span = if c.span > 1 { format!(" colspan=\"{}\"", c.span) } else { String::new() };
                        let style = match (hex(c.color), c.fill) {
                            (colour, Some(f)) => {
                                let colour = colour.map_or(String::new(), |h| format!("color:#{h};"));
                                format!(" style=\"{colour}background:#{}\"", hex6(f))
                            }
                            (Some(h), None) => format!(" style=\"color:#{h}\""),
                            (None, None) => String::new(),
                        };
                        s.push_str(&format!("<td{span}{style}>{body}</td>"));
                    }
                    s.push_str("</tr>\n");
                }
                s.push_str("</table>\n");
            }
            Item::Img(im) => {
                let mime = if im.ext == "jpg" { "image/jpeg" } else { "image/png" };
                s.push_str(&format!("<p><img alt=\"\" src=\"data:{mime};base64,{}\"></p>\n", base64(&im.bytes)));
            }
            Item::PageBreak => s.push_str("<hr>\n"),
        }
    }
    s.push_str("</body>\n</html>\n");
    s
}

/// A colour's 8-bit sRGB components (NaN and out-of-range values clamp; NaN reads as 0).
pub(crate) fn rgb8(c: [f64; 3]) -> [u8; 3] {
    // The value is clamped to 0..=255 before the cast, so it cannot truncate.
    c.map(|v| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 })
}

/// The colour as `RRGGBB`, or `None` for black (the default every format already uses).
pub(crate) fn hex(c: [f64; 3]) -> Option<String> {
    let [r, g, b] = rgb8(c);
    (r | g | b != 0).then(|| format!("{r:02X}{g:02X}{b:02X}"))
}

/// The real character for a code of the Symbol font (Adobe's Symbol encoding), for the codes
/// documents use in practice: Greek letters, arrows, operators and the bullet.
fn symbol_char(code: u8) -> Option<char> {
    const UPPER: [char; 26] =
        ['Α', 'Β', 'Χ', 'Δ', 'Ε', 'Φ', 'Γ', 'Η', 'Ι', 'ϑ', 'Κ', 'Λ', 'Μ', 'Ν', 'Ο', 'Π', 'Θ', 'Ρ', 'Σ', 'Τ', 'Υ', 'ς', 'Ω', 'Ξ', 'Ψ', 'Ζ'];
    const LOWER: [char; 26] =
        ['α', 'β', 'χ', 'δ', 'ε', 'φ', 'γ', 'η', 'ι', 'ϕ', 'κ', 'λ', 'μ', 'ν', 'ο', 'π', 'θ', 'ρ', 'σ', 'τ', 'υ', 'ϖ', 'ω', 'ξ', 'ψ', 'ζ'];
    Some(match code {
        0x41..=0x5A => UPPER[usize::from(code - 0x41)],
        0x61..=0x7A => LOWER[usize::from(code - 0x61)],
        0x22 => '∀',
        0x24 => '∃',
        0x2D => '−',
        0x40 => '≅',
        0xA2 => '′',
        0xA3 => '≤',
        0xA5 => '∞',
        0xAB => '↔',
        0xAC => '←',
        0xAD => '↑',
        0xAE => '→',
        0xAF => '↓',
        0xB0 => '°',
        0xB1 => '±',
        0xB2 => '″',
        0xB3 => '≥',
        0xB4 => '×',
        0xB5 => '∝',
        0xB6 => '∂',
        0xB7 => '•',
        0xB8 => '÷',
        0xB9 => '≠',
        0xBA => '≡',
        0xBB => '≈',
        0xBC => '…',
        0xD6 => '√',
        0xDE => '⇒',
        0xDB => '⇔',
        0xE5 => '∑',
        0xF2 => '∫',
        _ => return None,
    })
}

/// The real character for a code of Wingdings (the bullet and tick marks Word draws).
fn wingdings_char(code: u8) -> Option<char> {
    Some(match code {
        0x6C => '●',
        0x6E => '■',
        0x6F => '□',
        0x71 => '❑',
        0x75 => '◆',
        0x76 => '❖',
        0xA7 => '▪',
        0xA8 => '◻',
        0xD8 => '➢',
        0xFB => '✗',
        0xFC => '✓',
        0xFE => '☑',
        _ => return None,
    })
}

/// Symbol and Wingdings characters (Word bullets, tick marks, Greek letters) reach a PDF's text
/// as private-use code points (U+F020..=U+F0FF, the font's code plus 0xF000). In any other font
/// those draw as unrelated glyphs or boxes, so each is replaced by the real Unicode character
/// when it is one of the well-known ones; the font name picks the table (Symbol, or Wingdings
/// and ZapfDingbats bullets), and an unnamed font gets only the unambiguous bullets. Other
/// characters are left alone.
pub fn symbols_to_unicode(font: &str, text: &str) -> String {
    if !text.chars().any(|c| ('\u{F020}'..='\u{F0FF}').contains(&c)) {
        return text.to_string();
    }
    let name = font.to_ascii_lowercase();
    text.chars()
        .map(|c| {
            let Some(code) = u32::from(c).checked_sub(0xF000).and_then(|v| u8::try_from(v).ok()).filter(|v| *v >= 0x20) else { return c };
            let found = if name.contains("wingdings") || name.contains("dingbats") {
                wingdings_char(code)
            } else if name.contains("symbol") {
                symbol_char(code)
            } else {
                // Without a font name only the bullets nobody uses for anything else.
                match code {
                    0xB7 => symbol_char(code),
                    0xA7 | 0xD8 | 0xFC => wingdings_char(code),
                    _ => None,
                }
            };
            found.unwrap_or(c)
        })
        .collect()
}

/// The Word font family for a PDF `/BaseFont`, or `None` to leave the choice to Word (no font
/// named, or a symbol font whose glyphs no other family has). Subset tags (`ABCDEF+`), style
/// suffixes (`-Bold`, `,Italic`) and the `MT`/`PSMT` foundry suffixes are dropped, the PDF's
/// standard-14 names become their Word equivalents, and run-together names are spaced
/// (`TimesNewRoman` is `Times New Roman`).
pub(crate) fn font_family(base_font: &str) -> Option<String> {
    let name = base_font.trim();
    let name = match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    };
    let stem = name.split(['-', ',']).next().unwrap_or_default().trim();
    let stem = ["PSMT", "MT", "PS"]
        .iter()
        .find_map(|suffix| stem.strip_suffix(suffix))
        .filter(|rest| ["Arial", "TimesNewRoman", "CourierNew"].iter().any(|known| rest.starts_with(known)))
        .unwrap_or(stem);
    let family = match stem.to_ascii_lowercase().as_str() {
        "" | "symbol" | "zapfdingbats" => return None,
        "helvetica" | "arial" => "Arial".to_string(),
        "times" | "timesroman" | "timesnewroman" => "Times New Roman".to_string(),
        "courier" | "couriernew" => "Courier New".to_string(),
        _ => {
            let mut spaced = String::with_capacity(stem.len() + 4);
            let mut prev = ' ';
            for c in stem.chars().filter(|c| !c.is_control()).take(64) {
                if prev.is_ascii_lowercase() && c.is_ascii_uppercase() {
                    spaced.push(' ');
                }
                spaced.push(c);
                prev = c;
            }
            spaced
        }
    };
    (!family.trim().is_empty()).then_some(family)
}

/// A formatted text run (shared by paragraphs and table cells).
fn run_xml(text: &str, size: f64, bold: bool, italic: bool, font: &str, color: [f64; 3]) -> String {
    let mut rpr = String::new();
    if let Some(family) = font_family(font) {
        let family = esc(&family);
        rpr.push_str(&format!("<w:rFonts w:ascii=\"{family}\" w:hAnsi=\"{family}\" w:eastAsia=\"{family}\" w:cs=\"{family}\"/>"));
    }
    if bold {
        rpr.push_str("<w:b/>");
    }
    if italic {
        rpr.push_str("<w:i/>");
    }
    if let Some(h) = hex(color) {
        rpr.push_str(&format!("<w:color w:val=\"{h}\"/>"));
    }
    let half_points = if size.is_finite() { (size * 2.0).round().clamp(2.0, 3276.0) as i64 } else { 24 };
    rpr.push_str(&format!("<w:sz w:val=\"{half_points}\"/>"));
    format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(text))
}

/// The colour as `RRGGBB` (black included, for the places a missing colour would not mean black).
pub(crate) fn hex6(c: [f64; 3]) -> String {
    let [r, g, b] = rgb8(c);
    format!("{r:02X}{g:02X}{b:02X}")
}

/// A paragraph's or cell's background as the Word `w:shd` element.
fn shd_xml(c: [f64; 3]) -> String {
    format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>", hex6(c))
}

/// The Word `w:tcBorders` of a cell (empty when the source draws no line along it).
fn tc_borders(borders: &[Option<Border>; 4]) -> String {
    if borders.iter().all(Option::is_none) {
        return String::new();
    }
    let mut s = String::from("<w:tcBorders>");
    for (side, border) in ["top", "left", "bottom", "right"].iter().zip(borders) {
        if let Some(b) = border {
            // Eighths of a point: a hairline is 2, Word's thickest line is 96.
            let sz = if b.width.is_finite() { (b.width * 8.0).round().clamp(2.0, 96.0) as i64 } else { 4 };
            s.push_str(&format!("<w:{side} w:val=\"single\" w:sz=\"{sz}\" w:space=\"0\" w:color=\"{}\"/>", hex6(b.color)));
        }
    }
    s.push_str("</w:tcBorders>");
    s
}

/// One Word table: explicit absent borders (alignment alone cannot prove PDF rules) except along
/// the lines the page draws, a grid sized from the detected column edges, `gridSpan` for spanning
/// cells, the page's background colours, empty cells for holes.
fn docx_table(t: &Table) -> String {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let widths: Vec<i64> = edges.windows(2).map(|w| ((w[1] - w[0]).max(20.0) * 20.0).round().clamp(60.0, 31680.0) as i64).collect();
    let mut s = String::from("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>");
    for edge in ["top", "left", "bottom", "right", "insideH", "insideV"] {
        s.push_str(&format!("<w:{edge} w:val=\"nil\"/>"));
    }
    s.push_str("</w:tblBorders></w:tblPr><w:tblGrid>");
    for w in &widths {
        s.push_str(&format!("<w:gridCol w:w=\"{w}\"/>"));
    }
    s.push_str("</w:tblGrid>");
    for row in &t.rows {
        s.push_str("<w:tr>");
        for c in row {
            let span = if c.span > 1 { format!("<w:gridSpan w:val=\"{}\"/>", c.span) } else { String::new() };
            let w: i64 = widths.iter().take(c.span.min(widths.len())).sum();
            let borders = tc_borders(&c.borders);
            let shading = c.fill.map_or(String::new(), shd_xml);
            s.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{span}{borders}{shading}</w:tcPr><w:p>{}</w:p></w:tc>",
                run_xml(&c.text, c.size, c.bold, c.italic, &c.font, c.color)
            ));
        }
        // Pad the grid so every row covers all columns (Word rejects short rows).
        let used: usize = row.iter().map(|c| c.span).sum();
        for _ in used..t.cols.len() {
            s.push_str("<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p/></w:tc>");
        }
        s.push_str("</w:tr>");
    }
    s.push_str("</w:tbl>");
    s
}

/// Pages with rotated text, or text that actually overlaps an image or an oversized
/// (watermark-like) text block, cannot be put in paragraph flow without changing their layout.
/// Keep those pages as editable, absolutely positioned text boxes. Every other page, including
/// one with a logo or a big title that sits clear of the text, flows as paragraphs and tables.
fn positioned(page: &Page) -> bool {
    if page.blocks.iter().any(|b| b.rotation.abs() > 0.1) {
        return true;
    }
    let body = body_size(std::slice::from_ref(page)).max(1.0);
    let overlays = page
        .images
        .iter()
        .map(|im| (im.rect, None))
        .chain(page.blocks.iter().enumerate().filter(|(_, b)| b.size >= 2.0 * body).map(|(i, b)| (b.rect, Some(i))));
    // Bound the pairwise test on hostile pages (many overlays times many blocks): past the
    // budget, keep the default flowing layout.
    let mut budget: usize = 1_000_000;
    for (r, own) in overlays {
        for (i, b) in page.blocks.iter().enumerate() {
            if budget == 0 {
                return false;
            }
            budget -= 1;
            if Some(i) != own && overlaps(r, b.rect) {
                return true;
            }
        }
    }
    false
}

/// Whether two [x0, y0, x1, y1] boxes share more than a hairline (0.5 pt each way); boxes that
/// only touch, or hostile non-finite coordinates, do not count.
fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    let w = a[2].min(b[2]) - a[0].max(b[0]);
    let h = a[3].min(b[3]) - a[1].max(b[1]);
    w.is_finite() && h.is_finite() && w > 0.5 && h > 0.5
}

/// Bound every value written into a shape's style, including hostile PDF coordinates.
fn layout_number(n: f64) -> f64 {
    if n.is_finite() { n.clamp(-1584.0, 1584.0) } else { 0.0 }
}

/// Most background and line shapes one positioned Word page draws.
const MAX_SHAPES: usize = 8192;

/// Most images a Word export embeds, and most image bytes (the zip writer has 32-bit sizes and
/// a 16-bit entry count; a hostile PDF must not be able to overflow them).
const MAX_MEDIA: usize = 1000;
const MAX_MEDIA_BYTES: usize = 512 << 20;

/// Whether `im` can still be embedded: it has data, and the package stays within its limits.
fn room_for(media: &[(String, &Image)], im: &Image) -> bool {
    !im.bytes.is_empty()
        && media.len() < MAX_MEDIA
        && media.iter().map(|(_, m)| m.bytes.len()).fold(im.bytes.len(), usize::saturating_add) <= MAX_MEDIA_BYTES
}

/// Points as EMU (12700 per point); non-finite values are 0.
fn emu(pt: f64) -> i64 {
    if pt.is_finite() { (pt * 12700.0).round() as i64 } else { 0 }
}

/// The `a:graphic` of one picture (DrawingML), rotated by `rotation` degrees when given.
fn picture_xml(n: usize, name: &str, (cx, cy): (i64, i64), rotation: Option<f64>) -> String {
    let rot = rotation.map_or(String::new(), |r| format!(" rot=\"{}\"", (r * 60000.0).round() as i64));
    format!(
        "<wp:docPr id=\"{n}\" name=\"Picture {n}\"/><wp:cNvGraphicFramePr/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"rIdImg{n}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm{rot}><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
</a:graphicData></a:graphic>"
    )
}

/// Most path segments one shape writes; a longer outline is cut off.
const MAX_SHAPE_SEGS: usize = 4096;

/// A freeform VML shape (`v:shape` with a path) for `sh`, placed on the page. The path is in
/// hundredths of a point, from the shape's top-left corner.
fn shape_xml(page_height: f64, id: usize, sh: &Shape) -> String {
    const K: f64 = 100.0;
    let (w, h) = (layout_number(sh.rect[2] - sh.rect[0]).max(0.25), layout_number(sh.rect[3] - sh.rect[1]).max(0.25));
    let (x, y) = (layout_number(sh.rect[0]), layout_number(page_height - sh.rect[3]));
    let top = layout_number(sh.rect[3]);
    let pt = |p: &[f64; 2]| format!("{},{}", ((layout_number(p[0]) - x) * K).round() as i64, ((top - layout_number(p[1])) * K).round() as i64);
    let mut path = String::new();
    for seg in sh.path.iter().take(MAX_SHAPE_SEGS) {
        match seg {
            Seg::Move(p) => path.push_str(&format!("m {} ", pt(p))),
            Seg::Line(p) => path.push_str(&format!("l {} ", pt(p))),
            Seg::Curve(a, b, c) => path.push_str(&format!("c {},{},{} ", pt(a), pt(b), pt(c))),
            Seg::Close => path.push_str("x "),
        }
    }
    path.push('e');
    let fill = sh.fill.map_or(String::from(" filled=\"f\""), |c| format!(" fillcolor=\"#{}\"", hex6(c)));
    let line = sh.stroke.map_or(String::from(" stroked=\"f\""), |(c, wd)| {
        format!(" stroked=\"t\" strokecolor=\"#{}\" strokeweight=\"{:.2}pt\"", hex6(c), layout_number(wd).clamp(0.1, 72.0))
    });
    format!(
        "<w:r><w:pict><v:shape id=\"Shape{id}\" style=\"position:absolute;margin-left:{x:.3}pt;margin-top:{y:.3}pt;width:{w:.3}pt;height:{h:.3}pt;z-index:-3;mso-position-horizontal-relative:page;mso-position-vertical-relative:page\" coordsize=\"{},{}\" path=\"{path}\"{fill}{line}><w10:wrap type=\"none\"/></v:shape></w:pict></w:r>",
        (w * K).round() as i64,
        (h * K).round() as i64,
    )
}

fn positioned_page<'a>(page: &'a Page, media: &mut Vec<(String, &'a Image)>, shape_id: &mut usize) -> String {
    // One 1 pt paragraph anchors the whole page. Shapes never reserve body space, so even a
    // page-sized watermark cannot push the invoice footer onto another page.
    let mut s = String::from("<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/></w:pPr>");
    let body = body_size(std::slice::from_ref(page)).max(1.0);
    // A paragraph with a rectangle for each of its rows is set row by row: every row sits where
    // the source had it, so the rows cannot drift into each other whatever line height, spacing or
    // wrapping Word applies, and a row's own alignment is its position.
    let mut shapes: Vec<std::borrow::Cow<'_, Block>> = Vec::new();
    for b in &page.blocks {
        let rows = &b.layout.lines;
        if b.rotation.abs() <= 0.1 && rows.len() > 1 && b.layout.rects.len() == rows.len() {
            for (row, rect) in rows.iter().zip(&b.layout.rects) {
                if !row.trim().is_empty() {
                    shapes.push(std::borrow::Cow::Owned(Block {
                        text: row.clone(),
                        rect: *rect,
                        layout: Layout { align: b.layout.align, ..Layout::default() },
                        ..b.clone()
                    }));
                }
            }
        } else {
            shapes.push(std::borrow::Cow::Borrowed(b));
        }
    }
    // The source's backgrounds and lines, as shapes behind the text (negative z-order, so below
    // text at 1), in the order the page paints them.
    let boxes = page
        .fills
        .iter()
        .filter(|f| !is_white(f.color))
        .map(|f| (f.rect, f.color, -5))
        .chain(page.rules.iter().map(|r| {
            let half = layout_number(r.width).clamp(0.25, 72.0) / 2.0;
            let rect = if r.rect[3] - r.rect[1] <= r.rect[2] - r.rect[0] {
                [r.rect[0], r.rect[1] - half, r.rect[2], r.rect[3] + half]
            } else {
                [r.rect[0] - half, r.rect[1], r.rect[2] + half, r.rect[3]]
            };
            (rect, r.color, -4)
        }))
        .take(MAX_SHAPES);
    for (rect, color, z) in boxes {
        *shape_id = shape_id.saturating_add(1);
        let (w, h) = (layout_number(rect[2] - rect[0]).max(0.25), layout_number(rect[3] - rect[1]).max(0.25));
        let (x, y) = (layout_number(rect[0]), layout_number(page.height - rect[3]));
        s.push_str(&format!(
            "<w:r><w:pict><v:rect id=\"Box{}\" style=\"position:absolute;margin-left:{x:.3}pt;margin-top:{y:.3}pt;width:{w:.3}pt;height:{h:.3}pt;z-index:{z};mso-position-horizontal-relative:page;mso-position-vertical-relative:page\" fillcolor=\"#{}\" stroked=\"f\"><w10:wrap type=\"none\"/></v:rect></w:pict></w:r>",
            shape_id,
            hex6(color)
        ));
    }
    // Dots and wedges over the backgrounds, still behind the text.
    for sh in page.shapes.iter().take(MAX_SHAPES) {
        *shape_id = shape_id.saturating_add(1);
        s.push_str(&shape_xml(page.height, *shape_id, sh));
    }
    for b in shapes.iter().map(|b| b.as_ref()) {
        *shape_id = shape_id.saturating_add(1);
        let rotation = layout_number(-b.rotation).rem_euclid(360.0);
        let (sin, cos) = rotation.to_radians().sin_cos();
        let (bw, bh) = (layout_number(b.rect[2] - b.rect[0]).max(1.0), layout_number(b.rect[3] - b.rect[1]).max(1.0));
        let h = if b.rotation.abs() > 0.1 { layout_number(b.size).max(1.0) } else { bh };
        // Recover the unrotated width from the axis-aligned box. Use the larger axis so
        // quarter turns and 45-degree watermarks do not divide by (nearly) zero.
        let w = if b.rotation.abs() <= 0.1 {
            bw
        } else if cos.abs() >= sin.abs() {
            (bw - h * sin.abs()) / cos.abs()
        } else {
            (bh - h * cos.abs()) / sin.abs()
        };
        let w = layout_number(w).max(1.0);
        let (x, y) = (layout_number(b.rect[0] + (bw - w) / 2.0), layout_number(page.height - b.rect[3] + (bh - h) / 2.0));
        let z = if b.size >= 2.0 * body { -2 } else { 1 };
        // Word sets text slightly wider than the PDF's own font metrics (hinting, kerning,
        // rounding, or a substitute face), and the box is exactly as wide as the source text, so the
        // last character or word would wrap onto a new line. Give upright text slack on the side(s)
        // the text can grow into: to the right of left-aligned text, both sides of centred text,
        // to the left of right-aligned text. The source's rows stay as they were (a paragraph's
        // own line breaks, not Word's re-wrapping, whose different line count would run into the
        // text below), at the source's line pitch.
        let upright = b.rotation.abs() <= 0.1;
        let rows = &b.layout.lines;
        let multi = upright && rows.len() > 1;
        let align = if upright { b.layout.align } else { Align::Left };
        let (x, w) = if upright {
            let pad = if align == Align::Left { (b.size.max(1.0) * 0.5 + w * 0.04).min(72.0) } else { (b.size.max(1.0) + w * 0.15).min(200.0) };
            match align {
                Align::Left => (x, w + pad),
                Align::Center => (x - pad, w + 2.0 * pad),
                Align::Right => (x - pad, w + pad),
            }
        } else {
            (x, w)
        };
        let (x, w) = (layout_number(x), layout_number(w).max(1.0));
        // Word sets a line as tall as its own font's metrics make it, and puts the baseline in it
        // accordingly, so text lands a few points above or below where the PDF drew it: over the
        // rule of the box it sits in. Pin the line height instead (the source's own, for a single
        // line; its pitch for a paragraph) and move the shape so the first baseline is where the
        // PDF had it. Word puts about four fifths of an exact-height line above the baseline.
        let natural = b.size.max(1.0);
        let (line_height, y) = if !upright {
            (None, y)
        } else if multi {
            let lead = b.layout.leading;
            if lead.is_finite() && lead >= natural * 0.8 {
                let lead = lead.min(natural * 6.0);
                // The first row's own box: the block less the pitch of the rows below it.
                let first = bh - (rows.len().saturating_sub(1)) as f64 * lead;
                let first = if first.is_finite() && first >= natural * 0.5 { first } else { lead };
                (Some(lead), layout_number(y + ((first - lead) * 0.8).clamp(-natural, natural)))
            } else {
                (None, y)
            }
        } else {
            let line = bh.clamp(natural, natural * 2.0);
            (Some(line), layout_number(y + ((bh - line) * 0.8).clamp(-natural, natural)))
        };
        // A line of left-aligned text never wraps. Centred and right-aligned text keeps its box width,
        // which it needs to stay aligned.
        let no_wrap = upright && align == Align::Left && (multi || bh <= b.size.max(1.0) * 1.9);
        let wrap = if no_wrap { ";mso-wrap-style:none" } else { "" };
        // Text turned a quarter (the label down the side of a table) is a box the size of the text
        // as it sits on the page, with the text flowing vertically in it: the shape itself is not
        // rotated, so no viewer can wrap it into a column of single characters.
        let quarter = if (rotation - 270.0).abs() < 0.5 {
            Some(true)
        } else if (rotation - 90.0).abs() < 0.5 {
            Some(false)
        } else {
            None
        };
        let (style, flow) = if let Some(bottom_to_top) = quarter {
            let size = b.size.max(1.0);
            let pad = (size * 0.5 + bh * 0.04).min(72.0);
            let top = layout_number(page.height - b.rect[3]);
            let (across, length) = (bw.max(size * 1.2), bh + pad);
            // The text starts at the bottom of a bottom-to-top box and at the top of the other,
            // and has room to grow past its far end.
            let y = if bottom_to_top { top - pad } else { top };
            let (x, y) = (layout_number(b.rect[0]), layout_number(y));
            let direction = if bottom_to_top { "layout-flow:vertical;mso-layout-flow-alt:bottom-to-top;" } else { "layout-flow:vertical;" };
            (
                format!(
                    "position:absolute;margin-left:{x:.3}pt;margin-top:{y:.3}pt;width:{across:.3}pt;height:{length:.3}pt;z-index:{z};mso-wrap-style:none;mso-position-horizontal-relative:page;mso-position-vertical-relative:page"
                ),
                format!("{direction}mso-wrap-style:none"),
            )
        } else {
            (
                format!(
                    "position:absolute;margin-left:{x:.3}pt;margin-top:{y:.3}pt;width:{w:.3}pt;height:{h:.3}pt;rotation:{rotation:.3};z-index:{z}{wrap};mso-position-horizontal-relative:page;mso-position-vertical-relative:page"
                ),
                format!("mso-fit-shape-to-text:t{wrap}"),
            )
        };
        let heading =
            b.heading_level.filter(|level| (1..=6).contains(level)).map_or(String::new(), |level| format!("<w:pStyle w:val=\"Heading{level}\"/>"));
        let pitch =
            line_height.map_or(String::new(), |h| format!(" w:line=\"{}\" w:lineRule=\"exact\"", (h * 20.0).round().clamp(20.0, 31680.0) as i64));
        let jc = match align {
            Align::Left => "",
            Align::Center => "<w:jc w:val=\"center\"/>",
            Align::Right => "<w:jc w:val=\"right\"/>",
        };
        let text = if multi {
            rows.iter().map(|row| run_xml(row, b.size, b.bold, b.italic, &b.font, b.color)).collect::<Vec<_>>().join("<w:r><w:br/></w:r>")
        } else {
            run_xml(&b.text, b.size, b.bold, b.italic, &b.font, b.color)
        };
        s.push_str(&format!(
            "<w:r><w:pict><v:rect id=\"Text{}\" style=\"{style}\" filled=\"f\" stroked=\"f\"><v:textbox inset=\"0,0,0,0\" style=\"{flow}\"><w:txbxContent><w:p><w:pPr>{heading}<w:spacing w:before=\"0\" w:after=\"0\"{pitch}/>{jc}</w:pPr>{text}</w:p></w:txbxContent></v:textbox><w10:wrap type=\"none\"/></v:rect></w:pict></w:r>",
            shape_id
        ));
    }
    for im in &page.images {
        if !room_for(media, im) {
            continue;
        }
        *shape_id = shape_id.saturating_add(1);
        let n = media.len().saturating_add(1);
        let (w, h) = (layout_number(im.size[0]).max(1.0), layout_number(im.size[1]).max(1.0));
        let (x, y) = (layout_number((im.rect[0] + im.rect[2] - w) / 2.0), layout_number(page.height - (im.rect[1] + im.rect[3] + h) / 2.0));
        let rotation = layout_number(-im.rotation).rem_euclid(360.0);
        let name = format!("image{n}.{}", im.ext);
        let pic = picture_xml(n, &name, (emu(w), emu(h)), Some(rotation));
        // A DrawingML anchor, the way Word itself stores a picture placed behind text: a VML
        // rectangle with an image does not show the picture.
        s.push_str(&format!(
            "<w:r><w:drawing><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"{}\" behindDoc=\"1\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\">\
<wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionV>\
<wp:extent cx=\"{}\" cy=\"{}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:wrapNone/>",
            n.saturating_add(1),
            emu(x),
            emu(y),
            emu(w),
            emu(h),
        ));
        s.push_str(&pic);
        s.push_str("</wp:anchor></w:drawing></w:r>");
        media.push((name, im));
    }
    s.push_str("</w:p>");
    s
}

fn word_page_size(page: &Page) -> (i64, i64) {
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(144.0, 31680.0) as i64 } else { 12240 };
    (twips(page.width), twips(page.height))
}

fn section(page: &Page) -> String {
    let (pw, ph) = word_page_size(page);
    let (mx, my) = ((pw / 8).min(1440), (ph / 8).min(1440));
    format!(
        "<w:sectPr><w:type w:val=\"nextPage\"/><w:pgSz w:w=\"{pw}\" w:h=\"{ph}\"/><w:pgMar w:top=\"{my}\" w:right=\"{mx}\" w:bottom=\"{my}\" w:left=\"{mx}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/></w:sectPr>",
        my / 2,
        my / 2
    )
}

/// A Word document (.docx, Office Open XML). Ordinary pages flow as paragraphs and tables;
/// pages with rotated text, or text that overlaps artwork, use floating shapes to retain their layout.
pub fn docx(pages: &[Page], title: &str) -> Vec<u8> {
    let mut body = String::new();
    let mut media: Vec<(String, &Image)> = Vec::new();
    // Word needs a paragraph between adjacent tables and after the last one in the body.
    let mut after_table = false;
    let run = |b: &Block| run_xml(&b.text, b.size, b.bold, b.italic, &b.font, b.color);
    let mut shape_id = 0usize;
    for (page_index, page) in pages.iter().enumerate() {
        let (pw, _) = word_page_size(page);
        let text_w = (pw - 2 * (pw / 8).min(1440)) as f64 / 20.0;
        if positioned(page) {
            body.push_str(&positioned_page(page, &mut media, &mut shape_id));
            after_table = false;
        } else {
            // Dots and wedges are page-anchored, so a 1 pt paragraph carries them without moving
            // the flowing text.
            if !page.shapes.is_empty() {
                body.push_str("<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/></w:pPr>");
                for sh in page.shapes.iter().take(MAX_SHAPES) {
                    shape_id = shape_id.saturating_add(1);
                    body.push_str(&shape_xml(page.height, shape_id, sh));
                }
                body.push_str("</w:p>");
            }
            for it in items(std::slice::from_ref(page)) {
                match &it {
                    Item::Para(b, lvl) => {
                        let heading = if (1..=6).contains(lvl) { format!("<w:pStyle w:val=\"Heading{lvl}\"/>") } else { String::new() };
                        // Text over a coloured band keeps the band, or light text would vanish.
                        let shading = background(&page.fills, b.rect).map_or(String::new(), shd_xml);
                        let style =
                            if heading.is_empty() && shading.is_empty() { String::new() } else { format!("<w:pPr>{heading}{shading}</w:pPr>") };
                        body.push_str(&format!("<w:p>{style}{}</w:p>", run(b)));
                    }
                    Item::Table(t) => {
                        if after_table {
                            body.push_str("<w:p/>");
                        }
                        body.push_str(&docx_table(t));
                    }
                    Item::Img(im) => {
                        if room_for(&media, im) {
                            let n = media.len() + 1;
                            let name = format!("image{n}.{}", im.ext);
                            // Size on the page, at most the text width.
                            let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
                            let k = (text_w / w).min(1.0);
                            let (cx, cy) = (emu(w * k), emu(h * k));
                            body.push_str(&format!("<w:p><w:r><w:drawing><wp:inline><wp:extent cx=\"{cx}\" cy=\"{cy}\"/>"));
                            body.push_str(&picture_xml(n, &name, (cx, cy), None));
                            body.push_str("</wp:inline></w:drawing></w:r></w:p>");
                            media.push((name, im));
                        }
                    }
                    Item::PageBreak => body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
                }
                after_table = matches!(it, Item::Table(_));
            }
        }
        // Section breaks carry this source page's size and create exactly one next page.
        // The last section must be a direct child of the body (OOXML 17.6.17).
        if page_index + 1 < pages.len() {
            body.push_str(&format!(
                "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/>{}</w:pPr></w:p>",
                section(page)
            ));
        } else {
            if after_table || page.blocks.is_empty() && page.images.is_empty() {
                body.push_str("<w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/></w:pPr></w:p>");
            }
            body.push_str(&section(page));
        }
        after_table = false;
    }
    let doc = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:w10=\"urn:schemas-microsoft-com:office:word\"><w:body>{body}\
</w:body></w:document>"
    );
    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    );
    types.push_str("</Types>");
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>";
    let mut doc_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    );
    for (i, (name, _)) in media.iter().enumerate() {
        doc_rels.push_str(&format!(
            "<Relationship Id=\"rIdImg{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{name}\"/>",
            i + 1
        ));
    }
    doc_rels.push_str("</Relationships>");
    let styles = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"120\"/></w:pPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"240\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"200\"/><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>\
";
    let mut styles = styles.to_owned();
    for level in 3..=6 {
        styles.push_str(&format!("<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\"><w:name w:val=\"heading {level}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:pPr><w:keepNext/><w:spacing w:before=\"200\"/><w:outlineLvl w:val=\"{}\"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>", level - 1));
    }
    styles.push_str("</w:styles>");
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title></cp:coreProperties>",
        esc(title)
    );
    let mut z = Zip::default();
    z.add("[Content_Types].xml", types.as_bytes(), true);
    z.add("_rels/.rels", rels.as_bytes(), true);
    z.add("docProps/core.xml", core.as_bytes(), true);
    z.add("word/document.xml", doc.as_bytes(), true);
    z.add("word/styles.xml", styles.as_bytes(), true);
    z.add("word/_rels/document.xml.rels", doc_rels.as_bytes(), true);
    for (name, im) in &media {
        z.add(&format!("word/media/{name}"), &im.bytes, false);
    }
    z.finish()
}

fn rtf_text(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '\\' | '{' | '}' => {
                o.push('\\');
                o.push(c);
            }
            c if (c as u32) < 128 => o.push(c),
            c => {
                // \uN with a ? fallback; UTF-16 units as signed 16-bit numbers.
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    o.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    o
}

/// Rich Text Format: paragraphs with their sizes and bold/italic, real table rows, page breaks
/// between pages (images are left out).
pub fn rtf(pages: &[Page]) -> String {
    let items = items(pages);
    // The colour table: entry 0 is the default (auto) colour, then each non-black colour once.
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut note = |c: [f64; 3]| {
        let c = rgb8(c);
        if c != [0, 0, 0] && !colors.contains(&c) {
            colors.push(c);
        }
    };
    for it in &items {
        match it {
            Item::Para(b, _) => note(b.color),
            Item::Table(t) => t.rows.iter().flatten().for_each(|c| note(c.color)),
            Item::Img(_) | Item::PageBreak => {}
        }
    }
    // `\cfN` for a colour (nothing for black, which keeps the default).
    let cf = |c: [f64; 3]| {
        let c = rgb8(c);
        colors.iter().position(|e| *e == c).map_or(String::new(), |i| format!("\\cf{}", i + 1))
    };
    let mut s = String::from("{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Helvetica;}}\n");
    if !colors.is_empty() {
        s.push_str("{\\colortbl;");
        for [r, g, b] in &colors {
            s.push_str(&format!("\\red{r}\\green{g}\\blue{b};"));
        }
        s.push_str("}\n");
    }
    for it in items {
        match it {
            Item::Para(b, _) => {
                let mut fmt = format!("\\fs{}", (b.size * 2.0).round() as i64);
                if b.bold {
                    fmt.push_str("\\b");
                }
                if b.italic {
                    fmt.push_str("\\i");
                }
                fmt.push_str(&cf(b.color));
                s.push_str(&format!("{{\\pard{fmt} {}\\par}}\n", rtf_text(&b.text)));
            }
            Item::Table(t) => {
                let mut edges = t.cols.clone();
                edges.push(t.rect[2]);
                let cellx: Vec<i64> = edges.iter().skip(1).map(|e| (e * 20.0).round() as i64).collect();
                for row in &t.rows {
                    s.push_str("\\trowd\\trgaph108");
                    // The row's cell boundaries come first: each cell ends at the right edge of
                    // the last grid column it spans.
                    let mut column = 0;
                    for c in row {
                        column += c.span.max(1);
                        if let Some(x) = cellx.get(column.min(cellx.len()).saturating_sub(1)) {
                            s.push_str(&format!("\\cellx{x}"));
                        }
                    }
                    for c in row {
                        let mut fmt = format!("\\intbl\\fs{}", (c.size * 2.0).round() as i64);
                        if c.bold {
                            fmt.push_str("\\b");
                        }
                        if c.italic {
                            fmt.push_str("\\i");
                        }
                        fmt.push_str(&cf(c.color));
                        s.push_str(&format!("{{{fmt} {}}}\\cell", rtf_text(&c.text)));
                    }
                    s.push_str("\\row\n");
                }
            }
            Item::Img(_) => {}
            Item::PageBreak => s.push_str("\\page\n"),
        }
    }
    s.push('}');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Page {
        let b = |t: &str, y: f64, size: f64, bold: bool| Block {
            text: t.into(),
            rect: [72.0, y, 500.0, y + size],
            size,
            bold,
            italic: false,
            font: String::new(),
            color: [0.0; 3],
            rotation: 0.0,
            heading_level: None,
            layout: Layout::default(),
        };
        Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                b("Body text after the picture, which is long enough to be the main text size of the page.", 400.0, 11.0, false),
                b("Annual Report", 700.0, 24.0, true),
                b("Overview", 650.0, 14.0, true),
                b("Some <body> text & more of it so that eleven points is the body size here.", 620.0, 11.0, false),
            ],
            images: vec![Image {
                ext: "png",
                bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03".to_vec(),
                rect: [72.0, 450.0, 272.0, 600.0],
                size: [200.0, 150.0],
                rotation: 0.0,
            }],
            ..Page::default()
        }
    }

    #[test]
    fn html_has_headings_paragraphs_and_images_in_order() {
        let h = html(&[page(), page()], "Report");
        let order: Vec<usize> = ["<h1>Annual Report</h1>", "<h2>Overview</h2>", "<p>Some &lt;body&gt; text &amp;", "<img", "<p>Body text after"]
            .iter()
            .map(|x| h.find(x).unwrap_or_else(|| panic!("{x} missing in {h}")))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        assert_eq!(h.matches("<hr>").count(), 1);
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }

    #[test]
    fn docx_is_a_word_package() {
        let d = docx(&[page()], "Report");
        assert!(d.starts_with(b"PK"));
        let names: Vec<&str> =
            ["[Content_Types].xml", "word/document.xml", "word/styles.xml", "word/_rels/document.xml.rels", "word/media/image1.png", "_rels/.rels"]
                .to_vec();
        for n in names {
            assert!(d.windows(n.len()).any(|w| w == n.as_bytes()), "{n}");
        }
    }

    #[test]
    fn word_does_not_invent_borders_for_aligned_invoice_fields() {
        let xml = part(&docx(&[table_page()], "Invoice"), "word/document.xml");
        assert!(xml.contains("<w:tbl>"));
        assert!(!xml.contains("w:val=\"single\""), "text alignment is not evidence of a border: {xml}");
        assert!(xml.contains("<w:insideH w:val=\"nil\"/>"));
    }

    #[test]
    fn word_keeps_a_page_with_a_clear_image_and_big_title_in_flow() {
        // page() has a picture between paragraphs and a 24 pt title over 11 pt body text, but
        // nothing overlaps: it must stay flowing paragraphs with the image inline.
        let xml = part(&docx(&[page()], "Report"), "word/document.xml");
        assert!(!xml.contains("position:absolute"), "nothing overlaps, so nothing floats: {xml}");
        assert!(xml.contains("<wp:inline>"), "the picture stays inline: {xml}");
        assert!(xml.contains("<w:pStyle w:val=\"Heading1\"/>"), "the title stays a heading: {xml}");
        let (title, body) = (xml.find(">Annual Report</w:t>"), xml.find(">Body text after the picture"));
        assert!(title.is_some() && body.is_some() && title < body, "reading order kept: {xml}");
        // The same page with the picture moved over the body text floats.
        let mut over = page();
        over.images[0].rect = [72.0, 380.0, 272.0, 530.0];
        let xml = part(&docx(&[over], "Report"), "word/document.xml");
        assert!(xml.contains("position:absolute"), "an image over text floats: {xml}");
    }

    #[test]
    fn word_overlap_does_not_consume_invoice_body_space() {
        let mut p = table_page();
        p.blocks.push(Block {
            text: "DRAFT".into(),
            rect: [100.0, 200.0, 500.0, 700.0],
            size: 100.0,
            bold: false,
            italic: false,
            font: String::new(),
            color: [0.8; 3],
            rotation: 0.0,
            heading_level: None,
            layout: Layout::default(),
        });
        p.images = page().images;
        let xml = part(&docx(&[p], "Invoice"), "word/document.xml");
        assert!(xml.contains("position:absolute"), "overlapping text must float: {xml}");
        assert!(!xml.contains("<wp:inline>"), "images must not displace invoice text");
        assert!(!xml.contains("<w:tbl>"), "the source text boxes must keep their positions");
        assert_eq!(xml.matches(">DRAFT</w:t>").count(), 1);
        assert_eq!(xml.matches("w:type=\"page\"").count(), 0);
        assert!(xml.contains("margin-left:72.000pt;margin-top:160.000pt"), "Meter field keeps its source position: {xml}");
        let watermark = xml.split(">DRAFT</w:t>").next().unwrap().rsplit("<v:rect ").next().unwrap();
        assert!(watermark.contains("z-index:-2"), "watermark is behind foreground graphics");
        // The picture is a DrawingML anchor behind the text (a VML rectangle shows no picture in Word).
        assert!(!xml.contains("<v:imagedata"), "{xml}");
        assert!(xml.contains("<wp:anchor ") && xml.contains("behindDoc=\"1\"") && xml.contains("<a:blip r:embed=\"rIdImg1\"/>"), "{xml}");
    }

    #[test]
    fn word_rotated_pages_keep_separate_page_sizes_and_unique_shapes() {
        let mut p = table_page();
        p.blocks[0].rotation = 45.0;
        let mut second = p.clone();
        second.width = 400.0;
        second.height = 500.0;
        let xml = part(&docx(&[p, second], "Two invoices"), "word/document.xml");
        assert!(xml.contains("rotation:315.000"), "PDF y-up becomes Word y-down");
        assert_eq!(xml.matches("<w:sectPr>").count(), 2);
        assert!(xml.contains("<w:pgSz w:w=\"8000\" w:h=\"10000\"/>"));
        assert_eq!(xml.matches("id=\"Text1\"").count(), 1);
        assert_eq!(xml.matches(">Meter</w:t>").count(), 2);
    }

    #[test]
    fn word_quarter_turned_table_text_flows_vertically_in_an_unrotated_box() {
        // A label down the side of a table, 11 pt text running bottom to top over 48 pt, is a
        // 12.65 x 48 pt box. Rotating a box that size and wrapping it is what made each character
        // land on its own line.
        let mut up = block("Side label", 70.0, 610.0, 82.65, 658.3);
        up.rotation = 90.0;
        let mut down = block("Other label", 40.0, 610.0, 52.65, 660.0);
        down.rotation = -90.0;
        let page = Page { width: 612.0, height: 792.0, blocks: vec![up, down], images: Vec::new(), ..Page::default() };
        let xml = part(&docx(&[page], "Table"), "word/document.xml");
        assert_eq!(xml.matches("layout-flow:vertical").count(), 2, "{xml}");
        assert_eq!(xml.matches("mso-layout-flow-alt:bottom-to-top").count(), 1, "{xml}");
        assert!(!xml.contains("rotation:"), "quarter turns are not rotated shapes: {xml}");
        assert!(xml.contains(">Side label</w:t>"), "{xml}");
        let wide = xml.find("height:").and_then(|i| xml[i + 7..].split("pt").next()?.parse::<f64>().ok()).unwrap_or(0.0);
        assert!(wide > 48.3, "room for the text to grow along its line: {wide}");
    }

    #[test]
    fn positioned_word_text_keeps_explicit_heading_levels() {
        let mut heading = block("Small heading", 72.0, 650.0, 180.0, 662.0);
        heading.rotation = 45.0;
        heading.heading_level = Some(5);
        let page = Page { width: 612.0, height: 792.0, blocks: vec![heading], images: Vec::new(), ..Page::default() };
        let xml = part(&docx(&[page], "Tagged"), "word/document.xml");
        assert!(xml.contains("<w:txbxContent>"), "{xml}");
        assert!(xml.contains("<w:pStyle w:val=\"Heading5\"/>"), "{xml}");
    }

    /// One part of a package written by [`Zip`], inflated.
    pub(crate) fn part(zip: &[u8], name: &str) -> String {
        let at = |i: usize, n: usize| zip[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | *b as usize);
        let mut i = 0;
        while zip[i..].starts_with(b"PK\x03\x04") {
            let (size, name_len) = (at(i + 18, 4), at(i + 26, 2));
            let data = i + 30 + name_len;
            if &zip[i + 30..data] == name.as_bytes() {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut flate2::read::DeflateDecoder::new(&zip[data..data + size]), &mut s).unwrap();
                return s;
            }
            i = data + size;
        }
        panic!("{name} missing")
    }

    #[test]
    fn word_opens_text_with_control_characters_and_long_pages() {
        // #72: text extracted without a usable /ToUnicode holds C0 controls, which XML forbids;
        // Word then refused the whole file. A till receipt is also taller than Word's 22 in.
        let mut p = page();
        p.height = 2400.0;
        p.blocks.push(Block {
            text: "Total\u{0}\u{3}\u{c} 4.50\u{FFFF}\tGBP".into(),
            rect: [72.0, 100.0, 300.0, 110.0],
            size: f64::NAN,
            bold: false,
            italic: false,
            font: String::new(),
            color: [0.0; 3],
            rotation: 0.0,
            heading_level: None,
            layout: Layout::default(),
        });
        let xml = part(&docx(&[p.clone()], "Receipt\u{1}"), "word/document.xml");
        assert!(xml.contains(">Total 4.50\tGBP</w:t>"), "{xml}");
        assert!(!xml.chars().any(|c| !allowed(c)));
        assert!(xml.contains("<w:pgSz w:w=\"12240\" w:h=\"31680\"/>"), "clamped to 22 in: {xml}");
        assert!(xml.contains("<w:sz w:val=\"24\"/>"), "a non-finite size falls back to 12 pt");
        assert!(rtf(&[p]).contains("Total 4.50\tGBP"));
        // A narrow page keeps room for text: margins shrink.
        let narrow = Page { width: 100.0, height: 200.0, ..page() };
        let xml = part(&docx(&[narrow], "Narrow"), "word/document.xml");
        assert!(xml.contains("<w:pgMar w:top=\"500\" w:right=\"250\" w:bottom=\"500\" w:left=\"250\""), "{xml}");
    }

    #[test]
    fn text_colour_survives_every_format() {
        // #526: a dark green heading (0.05 0.23 0.18 rg) came out black in Word.
        let mut p = page();
        p.blocks[1].color = [0.05, 0.23, 0.18];
        p.blocks[3].color = [f64::NAN, 2.0, -1.0];
        let xml = part(&docx(&[p.clone()], "Colour"), "word/document.xml");
        assert!(xml.contains("<w:color w:val=\"0D3B2E\"/>"), "{xml}");
        assert!(xml.contains("<w:color w:val=\"00FF00\"/>"), "out-of-range values clamp: {xml}");
        // Black text keeps Word's default colour.
        assert_eq!(xml.matches("<w:color ").count(), 2, "{xml}");
        let h = html(&[p.clone()], "Colour");
        assert!(h.contains("<h1 style=\"color:#0D3B2E\">Annual Report</h1>"), "{h}");
        assert!(h.contains("<p style=\"color:#00FF00\">"), "{h}");
        let r = rtf(&[p.clone()]);
        assert!(r.contains("{\\colortbl;\\red13\\green59\\blue46;\\red0\\green255\\blue0;}"), "{r}");
        assert!(r.contains("\\cf1 Annual Report"), "{r}");
        let ppt = pptx(&[p], "Colour");
        let pxml = part(&ppt, "ppt/slides/slide1.xml");
        assert!(pxml.contains("<a:srgbClr val=\"0D3B2E\"/>"), "{pxml}");
        assert!(pxml.contains("<a:srgbClr val=\"00FF00\"/>"), "out-of-range values clamp: {pxml}");
        // A table cell keeps its colour too.
        let mut t = table_page();
        t.blocks[0].color = [1.0, 0.0, 0.0];
        let xml = part(&docx(&[t.clone()], "Grid"), "word/document.xml");
        assert!(xml.contains("<w:color w:val=\"FF0000\"/>"), "{xml}");
        assert!(html(&[t], "Grid").contains("style=\"color:#FF0000\""));
    }

    #[test]
    fn word_text_keeps_the_source_font_and_single_lines_do_not_wrap() {
        for (pdf, word) in [
            ("ABCDEF+TimesNewRomanPS-BoldMT", Some("Times New Roman")),
            ("Helvetica-Oblique", Some("Arial")),
            ("ArialMT", Some("Arial")),
            ("Arial,Bold", Some("Arial")),
            ("Courier", Some("Courier New")),
            ("SegoeUI-Semibold", Some("Segoe UI")),
            ("OpenSans-Regular", Some("Open Sans")),
            ("Symbol", None),
            ("", None),
        ] {
            assert_eq!(font_family(pdf).as_deref(), word, "{pdf}");
        }
        let mut p = table_page();
        p.blocks[0].font = "ABCDEF+Calibri-Bold".into();
        p.blocks[0].rect = [72.0, 620.0, 192.0, 632.0];
        let xml = part(&docx(std::slice::from_ref(&p), "Fonts"), "word/document.xml");
        assert!(xml.contains("<w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:eastAsia=\"Calibri\" w:cs=\"Calibri\"/>"), "{xml}");
        assert_eq!(xml.matches("<w:rFonts ").count(), 1, "unnamed fonts are left to Word: {xml}");
        // Positioned (overlap) pages: a one-line block never wraps, a tall one still does.
        let mut shape = page();
        shape.blocks[2].rotation = 30.0;
        shape.blocks[0].font = "Verdana".into();
        shape.blocks.push(Block { rect: [72.0, 100.0, 300.0, 160.0], ..block("Several rows of text in a paragraph", 0.0, 0.0, 1.0, 1.0) });
        let xml = part(&docx(&[shape], "Shapes"), "word/document.xml");
        assert!(xml.contains("w:ascii=\"Verdana\""), "{xml}");
        // The box is wider than the source text (428 pt), so Word's slightly wider text still fits.
        assert!(xml.contains("width:450.620pt"), "text boxes need slack to the right: {xml}");
        assert!(xml.contains("mso-fit-shape-to-text:t;mso-wrap-style:none"), "single lines must not wrap: {xml}");
        assert!(xml.contains("mso-fit-shape-to-text:t\""), "multi-row blocks keep wrapping: {xml}");
    }

    #[test]
    fn word_positioned_text_keeps_source_rows_alignment_and_line_pitch() {
        let mut shape = page();
        shape.blocks[2].rotation = 30.0; // forces the positioned layout
        let rows = Layout { lines: vec!["Line one".into(), "Line two".into()], align: Align::Center, leading: 14.0, rects: Vec::new() };
        shape.blocks.push(Block { rect: [100.0, 300.0, 300.0, 326.0], layout: rows, ..block("Line one Line two", 0.0, 0.0, 1.0, 1.0) });
        let xml = part(&docx(&[shape], "Rows"), "word/document.xml");
        // Source rows stay rows, at the source's pitch, centred, in a box with slack on both sides.
        assert!(xml.contains("Line one</w:t></w:r><w:r><w:br/></w:r><w:r><w:rPr>"), "{xml}");
        assert!(xml.contains("w:line=\"280\" w:lineRule=\"exact\"/><w:jc w:val=\"center\"/>"), "{xml}");
        assert!(xml.contains("margin-left:58.000pt") && xml.contains("width:284.000pt"), "{xml}");
    }

    #[test]
    fn word_positioned_rows_with_rectangles_become_one_shape_each_at_their_own_place() {
        let mut shape = page();
        shape.blocks[2].rotation = 30.0; // forces the positioned layout
        let rows = Layout {
            lines: vec!["First row".into(), "Second".into()],
            align: Align::Center,
            leading: 14.0,
            rects: vec![[100.0, 330.0, 200.0, 342.0], [120.0, 316.0, 180.0, 328.0]],
        };
        shape.blocks.push(Block { rect: [100.0, 316.0, 200.0, 342.0], layout: rows, ..block("First row Second", 0.0, 0.0, 1.0, 1.0) });
        let xml = part(&docx(&[shape], "Rows"), "word/document.xml");
        assert!(!xml.contains("First row Second") && !xml.contains("<w:br/>"), "rows are not one paragraph: {xml}");
        let first = xml.split("<v:rect").find(|s| s.contains("First row")).unwrap_or_default();
        let second = xml.split("<v:rect").find(|s| s.contains("Second")).unwrap_or_default();
        // 792 - 342 = 450 and 792 - 328 = 464 from the top: each row keeps its own height.
        assert!(first.contains("margin-top:450.000pt"), "{first}");
        assert!(second.contains("margin-top:464.000pt"), "{second}");
        assert!(first.contains("<w:jc w:val=\"center\"/>") && second.contains("<w:jc w:val=\"center\"/>"));
    }

    #[test]
    fn word_positioned_single_lines_pin_their_height_and_first_baseline() {
        let mut shape = page();
        shape.blocks[2].rotation = 30.0; // forces the positioned layout
        // A 30 pt box around 12 pt text: Word's own line would sit higher or lower in it than the
        // PDF drew it, so the line is fixed at 24 pt and the shape moved down by 80% of the 6 pt left over.
        shape.blocks.push(Block { rect: [100.0, 300.0, 200.0, 330.0], ..block("Boxed", 0.0, 0.0, 1.0, 1.0) });
        let xml = part(&docx(&[shape], "Lines"), "word/document.xml");
        let shape_xml = xml.split("<v:rect").find(|s| s.contains("Boxed")).unwrap_or_default();
        assert!(shape_xml.contains("margin-top:466.800pt"), "{shape_xml}");
        assert!(shape_xml.contains("w:line=\"480\" w:lineRule=\"exact\""), "{shape_xml}");
    }

    #[test]
    fn word_tables_keep_the_borders_and_backgrounds_the_page_draws() {
        let grey = [0.8, 0.8, 0.8];
        let line = |rect: [f64; 4]| Rule { rect, width: 1.0, color: [0.0; 3] };
        let mut p = table_page();
        // No lines or fills: no borders or shading are invented.
        let plain = part(&docx(std::slice::from_ref(&p), "Bill"), "word/document.xml");
        assert!(!plain.contains("<w:tcBorders>") && !plain.contains("<w:shd "), "{plain}");

        p.fills = vec![Fill { rect: [0.0, 0.0, 612.0, 792.0], color: [1.0; 3] }, Fill { rect: [60.0, 614.0, 500.0, 640.0], color: grey }];
        p.rules = vec![
            line([68.0, 636.0, 500.0, 636.0]),  // above the header row
            line([68.0, 610.0, 500.0, 610.0]),  // under the header row
            line([228.0, 526.0, 228.0, 636.0]), // between the first two columns
            line([300.0, 200.0, 400.0, 200.0]), // far from the table: not a border
        ];
        let xml = part(&docx(std::slice::from_ref(&p), "Bill"), "word/document.xml");
        // The three header cells are shaded; the white page fill is not drawn.
        assert_eq!(xml.matches("w:fill=\"CCCCCC\"").count(), 3, "{xml}");
        assert!(!xml.contains("w:fill=\"FFFFFF\""), "{xml}");
        assert!(xml.contains("<w:tcBorders><w:top w:val=\"single\""), "{xml}");
        assert!(xml.contains("<w:left w:val=\"single\"") && xml.contains("<w:bottom w:val=\"single\""), "{xml}");
        // Header row (3 cells) and the first-column cells of the rows below it carry borders;
        // the rule far below is not one of them.
        let bordered = xml.matches("<w:tcBorders>").count();
        assert!((3..=9).contains(&bordered), "{bordered} bordered cells: {xml}");
        let h = html(std::slice::from_ref(&p), "Bill");
        assert!(h.contains("background:#CCCCCC"), "{h}");
    }

    #[test]
    fn word_paragraphs_over_a_band_keep_its_colour() {
        let mut p = page();
        p.fills = vec![Fill { rect: [0.0, 0.0, 612.0, 792.0], color: [0.1, 0.2, 0.5] }];
        let xml = part(&docx(&[p], "Band"), "word/document.xml");
        assert!(xml.contains("<w:pPr><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"1A3380\"/></w:pPr>"), "{xml}");
    }

    #[test]
    fn word_positioned_pages_draw_backgrounds_and_rules_as_shapes_behind_the_text() {
        let mut shape = page();
        shape.blocks[2].rotation = 30.0; // forces the positioned layout
        shape.fills =
            vec![Fill { rect: [50.0, 700.0, 250.0, 740.0], color: [0.8, 0.8, 0.8] }, Fill { rect: [0.0, 0.0, 10.0, 10.0], color: [1.0; 3] }];
        shape.rules = vec![Rule { rect: [50.0, 695.0, 250.0, 695.0], width: 2.0, color: [1.0, 0.0, 0.0] }];
        let xml = part(&docx(&[shape], "Shapes"), "word/document.xml");
        // 792 - 740 = 52 from the top; the rule is centred on its line (792 - 696 = 96).
        let band = xml.split("<v:rect").find(|s| s.contains("fillcolor=\"#CCCCCC\"")).unwrap_or_default();
        assert!(band.contains("margin-top:52.000pt") && band.contains("width:200.000pt") && band.contains("height:40.000pt"), "{band}");
        assert!(band.contains("z-index:-5") && band.contains("stroked=\"f\""), "{band}");
        let rule = xml.split("<v:rect").find(|s| s.contains("fillcolor=\"#FF0000\"")).unwrap_or_default();
        assert!(rule.contains("margin-top:96.000pt") && rule.contains("height:2.000pt"), "{rule}");
        assert!(!xml.contains("fillcolor=\"#FFFFFF\""), "the white page fill is not drawn: {xml}");
    }

    #[test]
    fn a_page_with_a_hostile_number_of_lines_and_fills_stays_bounded() {
        let mut p = table_page();
        p.fills = (0..50_000).map(|i| Fill { rect: [0.0, i as f64, 612.0, i as f64 + 20.0], color: [0.5; 3] }).collect();
        p.rules = (0..50_000).map(|i| Rule { rect: [60.0, i as f64, 500.0, i as f64], width: 1.0, color: [0.0; 3] }).collect();
        let started = std::time::Instant::now();
        let d = docx(std::slice::from_ref(&p), "Hostile");
        assert!(d.starts_with(b"PK"));
        let mut shape = p.clone();
        shape.blocks[0].rotation = 30.0;
        let xml = part(&docx(&[shape], "Hostile"), "word/document.xml");
        assert!(xml.matches("<v:rect").count() < 9000, "{} shapes", xml.matches("<v:rect").count());
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
    }

    #[test]
    fn word_draws_dots_and_wedges_as_freeform_shapes_behind_the_text() {
        // A page that would flow, with a green dot and a stroked red wedge on it.
        let mut p = page();
        p.shapes = vec![
            Shape {
                rect: [100.0, 700.0, 110.0, 710.0],
                path: vec![
                    Seg::Move([110.0, 705.0]),
                    Seg::Curve([110.0, 707.76], [107.76, 710.0], [105.0, 710.0]),
                    Seg::Line([100.0, 700.0]),
                    Seg::Close,
                ],
                fill: Some([0.0, 1.0, 0.0]),
                stroke: None,
            },
            Shape {
                rect: [200.0, 600.0, 250.0, 650.0],
                path: vec![Seg::Move([200.0, 600.0]), Seg::Line([250.0, 650.0])],
                fill: None,
                stroke: Some(([1.0, 0.0, 0.0], 3.0)),
            },
        ];
        let xml = part(&docx(&[p], "Dots"), "word/document.xml");
        // 792 - 710 = 82 from the top. The path is in hundredths of a point from the top-left
        // corner: (110, 705) is (1000, 500); the curve ends at (105, 710) = (500, 0).
        let dot = xml.split("<v:shape").find(|s| s.contains("fillcolor=\"#00FF00\"")).unwrap_or_default();
        assert!(dot.contains("margin-left:100.000pt") && dot.contains("margin-top:82.000pt"), "{dot}");
        assert!(dot.contains("width:10.000pt") && dot.contains("height:10.000pt") && dot.contains("coordsize=\"1000,1000\""), "{dot}");
        assert!(dot.contains("path=\"m 1000,500 c 1000,224,776,0,500,0 l 0,1000 x e\"") && dot.contains("stroked=\"f\""), "{dot}");
        assert!(dot.contains("z-index:-3"), "{dot}");
        let wedge = xml.split("<v:shape").find(|s| s.contains("strokecolor=\"#FF0000\"")).unwrap_or_default();
        assert!(wedge.contains("filled=\"f\"") && wedge.contains("strokeweight=\"3.00pt\"") && wedge.contains("stroked=\"t\""), "{wedge}");
        assert!(xml.contains("Body text after the picture"), "the page's text is still there: {xml}");
    }

    #[test]
    fn a_flowing_table_page_keeps_flowing_and_still_draws_its_dots() {
        let mut p = table_page();
        p.shapes = vec![Shape {
            rect: [100.0, 700.0, 110.0, 710.0],
            path: vec![Seg::Move([110.0, 705.0]), Seg::Line([100.0, 700.0]), Seg::Close],
            fill: Some([1.0, 0.0, 0.0]),
            stroke: None,
        }];
        assert!(!positioned(&p));
        let xml = part(&docx(&[p], "Flow"), "word/document.xml");
        assert!(xml.contains("<v:shape") && xml.contains("fillcolor=\"#FF0000\""), "{xml}");
        assert!(xml.contains("<w:tbl>") && !xml.contains("v:textbox"), "the table still flows as a table");
    }

    #[test]
    fn a_page_with_hostile_shapes_stays_bounded_and_finite() {
        let mut p = page();
        p.shapes = (0..50_000)
            .map(|i| Shape {
                rect: [f64::NAN, f64::INFINITY, 1e300, -1e300],
                path: vec![Seg::Move([f64::NAN, 1e300]), Seg::Curve([f64::INFINITY; 2], [1e300; 2], [i as f64; 2]), Seg::Close],
                fill: Some([2.0, -1.0, f64::NAN]),
                stroke: Some(([0.0; 3], f64::NAN)),
            })
            .collect();
        let xml = part(&docx(&[p], "Hostile"), "word/document.xml");
        assert!(xml.matches("<v:shape").count() <= MAX_SHAPES);
        assert!(!xml.contains("NaN") && !xml.contains("inf"), "numbers stay finite");
    }

    #[test]
    fn symbol_and_wingdings_private_use_characters_become_real_ones() {
        // Word's bullets: Symbol U+F0B7, Wingdings U+F0A7 / U+F0D8 / U+F0FC.
        assert_eq!(symbols_to_unicode("ABCDEF+Symbol", "\u{F0B7} item \u{F061}\u{F062}"), "• item αβ");
        assert_eq!(symbols_to_unicode("Wingdings", "\u{F0A7}\u{F0FC}\u{F0D8}"), "▪✓➢");
        // No font name: only the unambiguous bullets; letters stay as they are.
        assert_eq!(symbols_to_unicode("", "\u{F0B7}\u{F061}"), "•\u{F061}");
        // Ordinary text, other private-use characters and unknown codes are untouched.
        assert_eq!(symbols_to_unicode("Arial", "plain \u{E000} text"), "plain \u{E000} text");
        assert_eq!(symbols_to_unicode("Symbol", "\u{F0FF}\u{F100}"), "\u{F0FF}\u{F100}");
    }

    #[test]
    fn rtf_escapes_and_sizes() {
        let mut p = page();
        p.blocks.push(Block {
            text: "Café {x}".into(),
            rect: [72.0, 100.0, 200.0, 110.0],
            size: 10.0,
            bold: false,
            italic: true,
            font: String::new(),
            color: [0.0; 3],
            rotation: 0.0,
            heading_level: None,
            layout: Layout::default(),
        });
        let r = rtf(&[p]);
        assert!(r.starts_with("{\\rtf1") && r.ends_with('}'));
        assert!(r.contains("\\fs48\\b Annual Report"));
        assert!(r.contains("\\fs20\\i Caf\\u233? \\{x\\}"), "{r}");
    }

    /// A 3-column table with a spanning header row and a hole in the last row.
    fn table_page() -> Page {
        let cell = |t: &str, x: f64, y: f64, w: f64| Block {
            text: t.into(),
            rect: [x, y, x + w, y + 12.0],
            size: 11.0,
            bold: false,
            italic: false,
            font: String::new(),
            color: [0.0; 3],
            rotation: 0.0,
            heading_level: None,
            layout: Layout::default(),
        };
        Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                cell("Meter", 72.0, 620.0, 120.0),
                cell("Unit", 232.0, 620.0, 80.0),
                cell("Reading", 392.0, 620.0, 100.0),
                cell("A1", 72.0, 590.0, 120.0),
                cell("kWh", 232.0, 590.0, 80.0),
                cell("37414.00", 392.0, 590.0, 100.0),
                cell("A2", 72.0, 560.0, 120.0),
                cell("kW", 232.0, 560.0, 80.0),
                // A2's row is missing its third cell (an empty cell in the grid).
                cell("Totals", 72.0, 530.0, 240.0),
                Block {
                    text: "Notes follow the table and are long enough to be the body text size of the page here.".into(),
                    rect: [72.0, 460.0, 500.0, 471.0],
                    size: 11.0,
                    bold: false,
                    italic: false,
                    font: String::new(),
                    color: [0.0; 3],
                    rotation: 0.0,
                    heading_level: None,
                    layout: Layout::default(),
                },
            ],
            images: Vec::new(),
            ..Page::default()
        }
    }

    #[test]
    fn grid_text_becomes_a_real_table_in_every_format() {
        let p = table_page();
        let h = html(std::slice::from_ref(&p), "Bill");
        assert!(h.contains("<table>"), "{h}");
        assert!(h.contains("<td colspan=\"2\">Totals</td>"), "spanning cell: {h}");
        assert_eq!(h.matches("<tr>").count(), 4, "{h}");
        assert!(h.contains("<td></td>"), "the hole is an empty cell: {h}");
        assert!(h.contains("<p>Notes follow"), "the paragraph after the table stays: {h}");

        let d = docx(std::slice::from_ref(&p), "Bill");
        assert!(d.starts_with(b"PK"));
        assert!(d.windows(b"word/document.xml".len()).any(|w| w == b"word/document.xml"));
        let (tables, _) = tables(&p.blocks);
        assert_eq!(tables.len(), 1);
        let table_xml = docx_table(&tables[0]);
        assert!(table_xml.contains("<w:tblGrid>"));
        assert!(table_xml.contains("<w:gridSpan w:val=\"2\"/>"), "{table_xml}");
        assert_eq!(table_xml.matches("<w:tr>").count(), 4);

        let r = rtf(std::slice::from_ref(&p));
        assert!(r.contains("\\trowd"), "{r}");
        assert_eq!(r.matches("\\row").count(), 4);
        // Boundaries precede the cells; "Totals" spans two columns, so it ends at the third edge.
        assert!(r.contains("\\trowd\\trgaph108\\cellx4640\\cellx7840\\cellx9920{"), "{r}");
        assert!(r.contains("\\trowd\\trgaph108\\cellx7840{\\intbl\\fs22 Totals}\\cell\\row"), "{r}");

        let pt = pptx(std::slice::from_ref(&p), "Bill");
        assert!(pt.starts_with(b"PK"));
        assert!(pt.windows(b"ppt/slides/slide1.xml".len()).any(|w| w == b"ppt/slides/slide1.xml"));
        let slide_xml = part(&pt, "ppt/slides/slide1.xml");
        assert!(slide_xml.contains("<a:tbl>"));
        assert!(slide_xml.contains("gridSpan=\"2\""));
        assert_eq!(slide_xml.matches("<a:tr ").count(), 4);
    }

    #[test]
    fn a_staircase_of_blocks_is_not_a_huge_table() {
        // Row k holds blocks in columns k and k + 1: every column has two members and every row
        // two columns, so without a cap this would be one table of (n / 2)² cells.
        let n = 4000;
        let blocks: Vec<Block> = (0..n)
            .map(|i| {
                let (row, col) = (i / 2, i / 2 + i % 2);
                let (x, y) = (10.0 + col as f64 * 20.0, 10_000.0 - row as f64 * 14.0);
                Block {
                    text: "x".into(),
                    rect: [x, y, x + 8.0, y + 12.0],
                    size: 11.0,
                    bold: false,
                    italic: false,
                    font: String::new(),
                    color: [0.0; 3],
                    rotation: 0.0,
                    heading_level: None,
                    layout: Layout::default(),
                }
            })
            .collect();
        let started = std::time::Instant::now();
        let (tables, _) = tables(&blocks);
        assert!(tables.iter().all(|t| t.cols.len() <= MAX_COLS), "{} columns", tables.iter().map(|t| t.cols.len()).max().unwrap_or(0));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
    }

    #[test]
    fn word_opens_tables_up_to_its_column_limit() {
        // Rows of short numbers on a regular grid, n columns wide.
        let grid = |n: usize| Page {
            width: 1500.0,
            height: 792.0,
            blocks: (0..3)
                .flat_map(|row| {
                    (0..n).map(move |col| {
                        let (x, y) = (20.0 + col as f64 * 22.0, 700.0 - row as f64 * 14.0);
                        Block {
                            text: (col + 1).to_string(),
                            rect: [x, y, x + 8.0, y + 8.0],
                            size: 8.0,
                            bold: false,
                            italic: false,
                            font: String::new(),
                            color: [0.0; 3],
                            rotation: 0.0,
                            heading_level: None,
                            layout: Layout::default(),
                        }
                    })
                })
                .collect(),
            images: Vec::new(),
            ..Page::default()
        };
        let grid_cols = |p: &Page| part(&docx(std::slice::from_ref(p), "Grid"), "word/document.xml").matches("<w:gridCol ").count();
        assert_eq!(grid_cols(&grid(63)), 63, "63 columns is still a table");
        // Word refuses to open a document with a 64-column table, so that grid stays text.
        assert_eq!(grid_cols(&grid(64)), 0);
    }

    fn block(t: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Block {
        Block {
            text: t.into(),
            rect: [x0, y0, x1, y1],
            size: 12.0,
            bold: false,
            italic: false,
            font: String::new(),
            color: [0.0; 3],
            rotation: 0.0,
            heading_level: None,
            layout: Layout::default(),
        }
    }

    /// #2806: a sentence whose link is set in another colour or font runs on as separate blocks
    /// on one line. They are not table columns, even when several such lines share left edges.
    #[test]
    fn a_link_inside_a_sentence_is_not_a_table_column() {
        let blue = |t: &str, x0: f64, y0: f64, x1: f64, y1: f64| Block { color: [0.02, 0.388, 0.757], ..block(t, x0, y0, x1, y1) };
        let p = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                block("6.3. 2. All content shown at", 121.7, 710.3, 272.0, 724.9),
                blue("http", 273.5, 710.3, 294.1, 724.9),
                blue("s", 294.1, 709.8, 298.8, 725.8),
                blue("://women", 298.8, 710.3, 348.0, 724.9),
                blue("g", 348.1, 709.8, 353.7, 725.8),
                blue("randprix.fide.com", 353.7, 710.3, 443.2, 724.9),
                block("shall be reviewed and approved by the Press Officer.", 164.3, 622.5, 533.1, 724.9),
                block("6.3. 3. The Organiser shall warrant that", 121.7, 601.8, 345.1, 616.5),
                blue("https://womengrandprix.fide.com", 350.2, 601.3, 519.9, 617.4),
                block("is managed in a professional way and furthermore that it is capable of", 164.3, 557.9, 533.0, 616.5),
                block("6. 3. 4. The Press Officer shall report to the FIDE Head of PR.", 121.7, 516.6, 530.3, 551.9),
                block("conducted during a WGP tournament.", 164.3, 502.0, 352.5, 516.6),
            ],
            images: Vec::new(),
            ..Page::default()
        };
        let h = html(std::slice::from_ref(&p), "Regulations");
        assert!(!h.contains("<table>"), "running text became a table: {h}");
        for b in &p.blocks {
            assert!(h.contains(&b.text), "{} was lost: {h}", b.text);
        }
    }

    /// #2806: lines that each run on into a link starting at the same place are sentences, not a
    /// two-column table, even three in a row.
    #[test]
    fn sentences_that_run_on_into_links_are_not_a_table() {
        let blue = |t: &str, x0: f64, y0: f64, x1: f64, y1: f64| Block { color: [0.02, 0.388, 0.757], ..block(t, x0, y0, x1, y1) };
        let mut blocks = Vec::new();
        for (n, lead) in ["Rules are published at", "Pairings are posted at", "Results are listed at"].iter().enumerate() {
            let y = 700.0 - 16.0 * n as f64;
            blocks.push(block(lead, 72.0, y, 211.0, y + 12.0));
            blocks.push(blue("https://example.org/event", 212.5, y, 360.0, y + 12.0));
        }
        let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
        let h = html(std::slice::from_ref(&p), "Event");
        assert!(!h.contains("<table>"), "sentences became a table: {h}");
        assert_eq!(h.matches("https://example.org/event").count(), 3, "{h}");
    }

    /// #2806: a superscript right after a number ("1" "st") continues its cell; the prize table
    /// around it stays a table.
    #[test]
    fn a_superscript_beside_a_number_keeps_its_table() {
        let mut blocks = vec![block("PLACE", 100.0, 700.0, 140.0, 712.0), block("PRIZE (euros)", 300.0, 700.0, 380.0, 712.0)];
        for (n, (row, sup)) in [("1", "st"), ("2", "nd"), ("3", "rd"), ("4", "th")].iter().enumerate() {
            let y = 680.0 - 20.0 * n as f64;
            blocks.push(block(row, 100.0, y, 106.0, y + 12.0));
            blocks.push(Block { size: 8.0, ..block(sup, 106.0, y + 5.0, 113.0, y + 13.0) });
            blocks.push(block(&format!("{},000", 30 - 5 * n), 300.0, y, 340.0, y + 12.0));
        }
        let p = Page { width: 612.0, height: 792.0, blocks, images: Vec::new(), ..Page::default() };
        let h = html(std::slice::from_ref(&p), "Prizes");
        assert!(h.contains("<table>"), "the prize table was lost: {h}");
        assert!(h.contains("30,000") && h.contains("15,000"), "{h}");
    }

    #[test]
    fn two_column_body_text_is_not_mistaken_for_a_table() {
        let para = |t: &str, x: f64, y: f64| Block {
            text: t.into(),
            rect: [x, y, x + 200.0, y + 90.0], // nine lines tall: body text, not a cell
            size: 11.0,
            bold: false,
            italic: false,
            font: String::new(),
            color: [0.0; 3],
            rotation: 0.0,
            heading_level: None,
            layout: Layout::default(),
        };
        let p = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                para("Left column text of the page, long enough to be a flowing paragraph.", 72.0, 500.0),
                para("Right column text of the page, long enough to be a flowing paragraph.", 320.0, 500.0),
                para("Second left block, still a tall flowing paragraph rather than a cell.", 72.0, 400.0),
                para("Second right block, still a tall flowing paragraph rather than a cell.", 320.0, 400.0),
            ],
            images: Vec::new(),
            ..Page::default()
        };
        let h = html(&[p], "Paper");
        assert!(!h.contains("<table>"), "tall two-column text must stay paragraphs: {h}");
    }

    /// #1936: every picture Word is told to draw (`r:embed`) names a relationship whose target is a
    /// file in the package, with a content type; JPEG pictures too, inline or placed.
    #[test]
    fn word_pictures_resolve_to_media_in_the_package() {
        let jpeg = Image { ext: "jpg", bytes: vec![0xFF, 0xD8, 0xFF, 0xD9], ..page().images[0].clone() };
        for placed in [false, true] {
            let mut p = page();
            p.images = vec![jpeg.clone()];
            if placed {
                p.images[0].rect = [72.0, 640.0, 272.0, 790.0];
                p.blocks[0].rect = [72.0, 700.0, 300.0, 760.0];
            }
            let zip = docx(&[p], "T");
            let xml = part(&zip, "word/document.xml");
            assert_eq!(xml.contains("<wp:anchor "), placed, "{xml}");
            let rels = part(&zip, "word/_rels/document.xml.rels");
            let embed = xml.split("r:embed=\"").nth(1).and_then(|r| r.split('"').next()).unwrap();
            let rel = rels.split(&format!("Id=\"{embed}\"")).nth(1).unwrap();
            assert!(rel.contains("relationships/image\""), "{rels}");
            let target = rel.split("Target=\"").nth(1).and_then(|t| t.split('"').next()).unwrap();
            assert_eq!(target, "media/image1.jpg");
            assert!(zip.windows(21).any(|w| w == b"word/media/image1.jpg"));
            assert!(part(&zip, "[Content_Types].xml").contains("Extension=\"jpg\" ContentType=\"image/jpeg\""));
            assert!(xml.matches("<wp:docPr ").count() == 1 && !xml.contains("<v:imagedata"));
        }
    }

    #[test]
    fn word_export_bounds_the_number_of_pictures_and_skips_empty_ones() {
        let mut p = page();
        let one = p.images[0].clone();
        p.images = vec![Image { bytes: Vec::new(), ..one.clone() }];
        p.images.extend(std::iter::repeat_n(one, MAX_MEDIA + 10));
        let zip = docx(&[p], "T");
        let rels = part(&zip, "word/_rels/document.xml.rels");
        assert_eq!(rels.matches("relationships/image\"").count(), MAX_MEDIA);
    }
}
