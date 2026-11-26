use super::layout::{DENSE_BLOCK, SheetLayout};
use super::objects::draw_cell_picture;
use super::text::{TextPlacement, draw_text};
use super::{
    BorderLine, BorderStyle, Borders, CellRef, Color, Font, Page, PatternType, Point, RangeRef, Rgb, Sheet, Style, StyleId, Underline, Value,
    Workbook, split_sheet,
};

// ---------------------------------------------------------------- drawing

pub(super) fn rgb_of(c: Color, wb: &Workbook) -> Option<Rgb> {
    c.resolve(&wb.theme)
}

fn blend(fg: [u8; 4]) -> Rgb {
    let a = fg[3] as u16;
    let mix = |c: u8| ((c as u16 * a + 255 * (255 - a)) / 255) as u8;
    [mix(fg[0]), mix(fg[1]), mix(fg[2])]
}

/// The effective look of a cell: own style, table style, conditional formats.
pub(super) struct Look {
    pub(super) style: Style,
    fill: Option<Rgb>,
    pub(super) cf: Option<crate::cf::CfLook>,
}

fn cell_look(wb: &Workbook, si: usize, sh: &Sheet, c: CellRef, cf: &mut crate::cf::CfCache) -> Look {
    let mut style = match sh.table_at(c) {
        Some(t) => crate::tables::table_cell_shown_style(wb, si, t, cf.table_edges(sh, t), c),
        None => wb.styles.get(sh.style_id(c)).clone(),
    };
    let cfl = cf.look(wb, si, c);
    if let Some(st) = cfl.as_ref().and_then(|l| l.style.as_ref()) {
        if st.align.reading_order.is_some() {
            style.align.reading_order = st.align.reading_order;
        }
        if st.fill.pattern != PatternType::None {
            style.fill = st.fill;
        }
        if st.font.color != Color::Auto {
            style.font.color = st.font.color;
        }
        style.font.bold |= st.font.bold;
        style.font.italic |= st.font.italic;
        style.font.strike |= st.font.strike;
        if st.font.underline != Underline::None {
            style.font.underline = st.font.underline;
        }
        // In a differential style General means "not set" (#1163).
        if st.num_fmt.as_str() != "General" {
            style.num_fmt = st.num_fmt.clone();
        }
        for (dst, src) in [
            (&mut style.borders.left, st.borders.left),
            (&mut style.borders.right, st.borders.right),
            (&mut style.borders.top, st.borders.top),
            (&mut style.borders.bottom, st.borders.bottom),
        ] {
            if !src.is_none() {
                *dst = src;
            }
        }
    }
    let mut fill = match style.fill.pattern {
        PatternType::None => None,
        PatternType::Solid => rgb_of(style.fill.fg, wb),
        _ => {
            let fg = rgb_of(style.fill.fg, wb).unwrap_or([0, 0, 0]);
            let bg = rgb_of(style.fill.bg, wb).unwrap_or([255, 255, 255]);
            Some([0, 1, 2].map(|i| ((fg[i] as u16 + bg[i] as u16 * 3) / 4) as u8))
        }
    };
    if let Some(rgb) = cfl.as_ref().and_then(|l| l.scale_fill) {
        fill = Some(rgb);
    }
    Look { style, fill, cf: cfl }
}

/// PDF measurement for chart layout.
pub(super) struct PdfMeasure;

impl gridcraft_chart::Measure for PdfMeasure {
    fn text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        gridcraft_pdf::text_width(if bold { Font::HelveticaBold } else { Font::Helvetica }, size, text)
    }
}

/// Maps chart-space coordinates to the page.
#[derive(Clone, Copy)]
pub(super) struct Xf {
    pub(super) ox: f32,
    pub(super) oy: f32,
    pub(super) k: f32,
}

impl Xf {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.ox + x * self.k, self.oy + y * self.k)
    }
    fn pts(&self, pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
        pts.iter().map(|q| [self.ox + q[0] * self.k, self.oy + q[1] * self.k]).collect()
    }
}

/// Draws chart primitives onto a page.
pub(super) fn draw_prims(pg: &mut Page, prims: &[gridcraft_chart::Prim], xf: Xf) {
    use gridcraft_chart::{HAlign as H, Prim, VAlign as V};
    let k = xf.k;
    for p in prims {
        match p {
            Prim::Rect { x, y, w, h, fill, stroke, .. } => {
                let (px, py) = xf.p(*x, *y);
                if let Some(f) = fill {
                    pg.fill_rect(px, py, w * k, h * k, blend(*f));
                }
                if let Some((c, lw)) = stroke {
                    pg.rect_stroke(px, py, w * k, h * k, (lw * k).max(0.1), blend(*c));
                }
            }
            Prim::Line { pts, color, width, dash } => {
                let d = if *dash { vec![3.0 * k, 2.0 * k] } else { vec![] };
                pg.polyline(&xf.pts(pts), (width * k).max(0.1), blend(*color), &d);
            }
            Prim::Polygon { pts, fill, stroke } => {
                pg.polygon(&xf.pts(pts), Some(blend(*fill)), stroke.map(|(c, w)| (blend(c), (w * k).max(0.1))));
            }
            Prim::Wedge { cx, cy, r_outer, r_inner, a0, a1, fill, stroke } => {
                let span = (a1 - a0).abs().min(std::f32::consts::TAU);
                let n = ((span / (std::f32::consts::PI / 36.0)).ceil() as usize).clamp(2, 400);
                let at = |a: f32, r: f32| [cx + r * a.sin(), cy - r * a.cos()];
                let mut pts: Vec<[f32; 2]> = (0..=n).map(|i| at(a0 + (a1 - a0) * i as f32 / n as f32, *r_outer)).collect();
                if *r_inner > 0.0 {
                    pts.extend((0..=n).rev().map(|i| at(a0 + (a1 - a0) * i as f32 / n as f32, *r_inner)));
                } else {
                    pts.push([*cx, *cy]);
                }
                pg.polygon(&xf.pts(&pts), Some(blend(*fill)), stroke.map(|(c, w)| (blend(c), (w * k).max(0.1))));
            }
            Prim::Circle { cx, cy, r, fill, stroke } => {
                let (px, py) = xf.p(*cx, *cy);
                pg.circle(px, py, r * k, fill.map(blend), stroke.map(|(c, w)| (blend(c), (w * k).max(0.1))));
            }
            Prim::Text { x, y, text, size, color, bold, align, valign, rotation } => {
                let font = if *bold { Font::HelveticaBold } else { Font::Helvetica };
                let size = size * k;
                let w = gridcraft_pdf::text_width(font, size, text);
                let dx = match align {
                    H::Left => 0.0,
                    H::Center => -w / 2.0,
                    H::Right => -w,
                };
                let dy = match valign {
                    V::Top => size * 0.78,
                    V::Middle => size * 0.35,
                    V::Bottom => -size * 0.21,
                };
                let (sn, cs) = if rotation.is_finite() { rotation.sin_cos() } else { (0.0, 1.0) };
                let (ax, ay) = xf.p(*x, *y);
                pg.text_rotated(Point::new(ax + dx * cs - dy * sn, ay + dx * sn + dy * cs), size, font, blend(*color), text, *rotation);
            }
        }
    }
}

/// Everything needed to draw one page block (a run of rows × a run of columns).
pub(super) struct Block<'a> {
    pub(super) wb: &'a Workbook,
    si: usize,
    pub(super) sh: &'a Sheet,
    rows: (u32, u32),
    cols: (u32, u32),
    /// Page x of each column's left edge, plus the right edge of the last.
    pub(super) xs: Vec<f32>,
    pub(super) ys: Vec<f32>,
    pub(super) k: f32,
    pub(super) scale: f32,
    pub(super) mirror_axis: f32,
}

impl Block<'_> {
    fn point_x(&self, x: f32) -> f32 {
        if self.sh.right_to_left { self.mirror_axis - x } else { x }
    }
    pub(super) fn text_x(&self, x: f32, width: f32) -> f32 {
        if self.sh.right_to_left { self.mirror_axis - x - width } else { x }
    }
    pub(super) fn drawn_rect(&self, r: RangeRef) -> (f32, f32, f32, f32) {
        let (x, y, w, h) = self.rect(r);
        (self.text_x(x, w), y, w, h)
    }
    fn x(&self, col: u32) -> f32 {
        if col >= self.cols.0 && col <= self.cols.1 + 1 {
            return self.xs.get((col - self.cols.0) as usize).copied().unwrap_or(0.0);
        }
        let x0 = self.xs.first().copied().unwrap_or(0.0);
        x0 + ((self.sh.col_left(col) - self.sh.col_left(self.cols.0)) as f32) * self.k
    }
    fn y(&self, row: u32) -> f32 {
        if row >= self.rows.0 && row <= self.rows.1 + 1 {
            return self.ys.get((row - self.rows.0) as usize).copied().unwrap_or(0.0);
        }
        let y0 = self.ys.first().copied().unwrap_or(0.0);
        y0 + ((self.sh.row_top(row) - self.sh.row_top(self.rows.0)) as f32) * self.k
    }
    pub(super) fn rect(&self, r: RangeRef) -> (f32, f32, f32, f32) {
        let (x0, y0) = (self.x(r.start.col), self.y(r.start.row));
        (x0, y0, self.x(r.end.col.saturating_add(1)) - x0, self.y(r.end.row.saturating_add(1)) - y0)
    }
    pub(super) fn range(&self) -> RangeRef {
        RangeRef::new(CellRef::new(self.rows.0, self.cols.0), CellRef::new(self.rows.1, self.cols.1))
    }
}

fn border_dash(style: BorderStyle, k: f32) -> Vec<f32> {
    let u = k.max(0.2);
    match style {
        BorderStyle::Dashed | BorderStyle::MediumDashed => vec![4.0 * u, 2.0 * u],
        BorderStyle::Dotted => vec![1.0 * u, 1.0 * u],
        BorderStyle::Hair => vec![0.75 * u, 0.75 * u],
        BorderStyle::DashDot | BorderStyle::MediumDashDot | BorderStyle::SlantDashDot => vec![4.0 * u, 1.5 * u, 1.0 * u, 1.5 * u],
        BorderStyle::DashDotDot | BorderStyle::MediumDashDotDot => vec![4.0 * u, 1.5 * u, 1.0 * u, 1.5 * u, 1.0 * u, 1.5 * u],
        _ => vec![],
    }
}

fn draw_border(pg: &mut Page, b: &BorderLine, wb: &Workbook, start: Point, end: Point, k: f32) {
    if b.is_none() {
        return;
    }
    let (x0, y0, x1, y1) = (start.x, start.y, end.x, end.y);
    let color = rgb_of(b.color, wb).unwrap_or([0, 0, 0]);
    if b.style == BorderStyle::Double {
        let off = 1.0 * k;
        let lw = (0.75 * k).max(0.2);
        let (dx, dy) = if (y0 - y1).abs() < 0.001 { (0.0, off) } else { (off, 0.0) };
        pg.stroke_line(x0 - dx, y0 - dy, x1 - dx, y1 - dy, lw, color);
        pg.stroke_line(x0 + dx, y0 + dy, x1 + dx, y1 + dy, lw, color);
        return;
    }
    let lw = (b.style.width() * k).clamp(0.25, 6.0);
    pg.stroke_line_dashed(Point::new(x0, y0), Point::new(x1, y1), lw, color, &border_dash(b.style, k));
}

/// Merged areas active at one row of a downward sweep, kept disjoint: by first column, their last
/// column and last row. Adding, dropping and looking up a merge costs log(active merges).
#[derive(Default)]
struct MergeSweep {
    cols: std::collections::BTreeMap<u32, (u32, u32)>,
    /// (last row, first column) of the active merges, soonest ending first.
    ends: std::collections::BinaryHeap<std::cmp::Reverse<(u32, u32)>>,
}

impl MergeSweep {
    /// Moves down to `row` (rows never decrease), dropping the merges that end above it.
    fn advance(&mut self, row: u32) {
        while let Some(std::cmp::Reverse((end, col))) = self.ends.peek().copied() {
            if end >= row {
                break;
            }
            self.ends.pop();
            self.cols.remove(&col);
        }
    }

    /// Does an active merge meet columns `c0..=c1`? The intervals are disjoint, so only the last
    /// one starting at or before `c1` can.
    fn meets(&self, c0: u32, c1: u32) -> bool {
        self.cols.range(..=c1).next_back().is_some_and(|(_, (end, _))| *end >= c0)
    }

    /// Adds merge `m`, at row `row`, unless it overlaps an active one.
    fn add(&mut self, m: RangeRef, row: u32) -> bool {
        self.advance(row);
        if self.meets(m.start.col, m.end.col) {
            return false;
        }
        self.cols.insert(m.start.col, (m.end.col, m.end.row));
        self.ends.push(std::cmp::Reverse((m.end.row, m.start.col)));
        true
    }
}

/// The merged areas that meet `block`, ordered by the row where they enter it, then by column,
/// leaving out any that overlaps one kept before it (only a damaged file has overlapping merges;
/// Excel drops them when it repairs one). One sweep down the rows instead of a scan of every
/// merge per cell.
fn block_merges(sh: &Sheet, block: RangeRef) -> Vec<RangeRef> {
    let top = |m: &RangeRef| m.start.row.max(block.start.row);
    let mut merges: Vec<RangeRef> = sh.merges.iter().filter(|m| m.intersects(&block)).copied().collect();
    merges.sort_by_key(|m| (top(m), m.start.col));
    let mut sweep = MergeSweep::default();
    merges.retain(|m| sweep.add(*m, top(m)));
    merges
}

/// Answers "is this cell inside a merge?" for cells asked in row-major order, from the merges of
/// [`block_merges`].
struct Covered<'a> {
    merges: &'a [RangeRef],
    top: u32,
    next: usize,
    sweep: MergeSweep,
}

impl<'a> Covered<'a> {
    fn new(merges: &'a [RangeRef], block: RangeRef) -> Self {
        Self { merges, top: block.start.row, next: 0, sweep: MergeSweep::default() }
    }

    fn covers(&mut self, c: CellRef) -> bool {
        while let Some(m) = self.merges.get(self.next).filter(|m| m.start.row.max(self.top) <= c.row) {
            self.sweep.add(*m, m.start.row.max(self.top));
            self.next += 1;
        }
        self.sweep.advance(c.row);
        self.sweep.meets(c.col, c.col)
    }
}

/// Draws the cells of a block: fills, gridlines, data bars, borders and text.
pub(super) fn draw_block(pg: &mut Page, b: &Block, gridlines: bool, cf: &mut crate::cf::CfCache) {
    let (bx, by) = (b.xs.first().copied().unwrap_or(0.0), b.ys.first().copied().unwrap_or(0.0));
    let (bw, bh) = (b.xs.last().copied().unwrap_or(bx) - bx, b.ys.last().copied().unwrap_or(by) - by);
    if bw <= 0.0 || bh <= 0.0 {
        return;
    }
    let block = b.range();
    let sh = b.sh;
    let merges = block_merges(sh, block);
    let by_start: std::collections::HashMap<CellRef, RangeRef> = merges.iter().map(|m| (m.start, *m)).collect();
    // Cells worth visiting, in row-major order (which `Covered` needs).
    let mut covered = Covered::new(&merges, block);
    let cells: Vec<CellRef> = if block.count() <= DENSE_BLOCK {
        block.iter().filter(|c| !covered.covers(*c)).collect()
    } else {
        let mut set: Vec<CellRef> = sh.cells.iter_range(block).map(|(c, _)| c).collect();
        set.extend(sh.spill.range(block.start..=block.end).map(|(c, _)| *c).filter(|c| block.contains(*c)));
        set.sort_unstable();
        set.dedup();
        set.retain(|c| !covered.covers(*c));
        set
    };
    pg.save();
    pg.clip_rect(b.text_x(bx, bw), by, bw, bh);
    if gridlines {
        let grey = [0xB0, 0xB0, 0xB0];
        let lw = (0.35 * b.scale).clamp(0.1, 0.5);
        for x in &b.xs {
            pg.stroke_line(b.point_x(*x), by, b.point_x(*x), by + bh, lw, grey);
        }
        for y in &b.ys {
            pg.stroke_line(b.point_x(bx), *y, b.point_x(bx + bw), *y, lw, grey);
        }
    }
    // Cheap pre-filter: blank, unstyled cells outside tables and conditional formats draw nothing.
    let skip = |c: &CellRef| {
        sh.cond_formats.is_empty()
            && sh.value_ref(*c).is_none_or(|v| v.is_empty())
            && sh.style_id(*c) == StyleId::DEFAULT
            && (sh.tables.is_empty() || sh.table_at(*c).is_none())
    };
    let looks: Vec<(CellRef, Look)> = cells
        .iter()
        .filter(|c| !skip(c))
        .chain(merges.iter().map(|m| &m.start))
        .map(|c| (*c, cell_look(b.wb, b.si, sh, *c, cf)))
        .filter(|(c, l)| {
            l.fill.is_some()
                || l.cf.is_some()
                || by_start.contains_key(c)
                || !sh.value(*c).is_empty()
                || l.style.borders != Borders::default()
                || l.style.num_fmt.as_str() == "checkbox"
        })
        .collect();
    let rect_of = |c: CellRef| -> (f32, f32, f32, f32, bool) {
        match by_start.get(&c) {
            Some(m) => {
                let (x, y, w, h) = b.rect(*m);
                (x, y, w, h, true)
            }
            None => {
                let (x, y, w, h) = b.rect(RangeRef::cell(c));
                (x, y, w, h, false)
            }
        }
    };
    // Fills (merged blocks hide the gridlines inside them).
    for (c, l) in &looks {
        let (x, y, w, h, merged) = rect_of(*c);
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        if let Some(f) = l.fill {
            pg.fill_rect(b.text_x(x, w), y, w, h, f);
        } else if merged && gridlines {
            pg.fill_rect(b.text_x(x, w), y, w, h, [255, 255, 255]);
        }
        if merged && gridlines {
            pg.rect_stroke(b.text_x(x, w), y, w, h, (0.35 * b.scale).clamp(0.1, 0.5), [0xB0, 0xB0, 0xB0]);
        }
        if let Some((frac, rgb, _)) = l.cf.as_ref().and_then(|cf| cf.bar) {
            let pad = 1.5 * b.k;
            let bwid = ((w - 2.0 * pad) * frac.clamp(0.0, 1.0)).max(0.0);
            pg.fill_rect(b.text_x(x + pad, bwid), y + pad, bwid, (h - 2.0 * pad).max(0.0), rgb);
        }
    }
    // Borders.
    for (c, l) in &looks {
        let (x, y, w, h, merged) = rect_of(*c);
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let bd = &l.style.borders;
        let (right, bottom) = match by_start.get(c).filter(|_| merged) {
            Some(m) => (
                b.wb.styles.get(sh.style_id(CellRef::new(m.start.row, m.end.col))).borders.right,
                b.wb.styles.get(sh.style_id(CellRef::new(m.end.row, m.start.col))).borders.bottom,
            ),
            None => (bd.right, bd.bottom),
        };
        draw_border(pg, &bd.top, b.wb, Point::new(b.point_x(x), y), Point::new(b.point_x(x + w), y), b.k);
        draw_border(pg, &bottom, b.wb, Point::new(b.point_x(x), y + h), Point::new(b.point_x(x + w), y + h), b.k);
        draw_border(pg, &bd.left, b.wb, Point::new(b.point_x(x), y), Point::new(b.point_x(x), y + h), b.k);
        draw_border(pg, &right, b.wb, Point::new(b.point_x(x + w), y), Point::new(b.point_x(x + w), y + h), b.k);
        draw_border(pg, &bd.diag_down, b.wb, Point::new(b.point_x(x), y), Point::new(b.point_x(x + w), y + h), b.k);
        draw_border(pg, &bd.diag_up, b.wb, Point::new(b.point_x(x), y + h), Point::new(b.point_x(x + w), y), b.k);
    }
    // Text.
    for (c, l) in &looks {
        let (x, y, w, h, merged) = rect_of(*c);
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        if let Some(picture) = sh.cell_pictures.get(c) {
            draw_cell_picture(pg, &picture.data, (b.text_x(x, w), y, w, h), b.k);
        } else if let Some(on) = checkbox_state(l, &sh.value(*c)) {
            draw_checkbox(pg, (b.text_x(x, w), y, w, h), b.k, on);
        } else {
            draw_text(pg, b, TextPlacement { cell: *c, look: l, rect: (x, y, w, h), merged, merges: &merges, limits: (bx, bx + bw) });
        }
    }
    pg.restore();
}

/// A checkbox cell prints TRUE/FALSE (and empty) as a box, as on screen (#2972); other values
/// print as text.
fn checkbox_state(l: &Look, v: &Value) -> Option<bool> {
    if l.style.num_fmt.as_str() != "checkbox" || l.cf.as_ref().is_some_and(|cf| cf.hide_value) {
        return None;
    }
    match v {
        Value::Bool(on) => Some(*on),
        Value::Empty => Some(false),
        _ => None,
    }
}

/// A checkbox centred in the cell: white with a grey outline, or green with a white tick.
fn draw_checkbox(pg: &mut Page, (x, y, w, h): (f32, f32, f32, f32), k: f32, on: bool) {
    let s = (13.2 * k).min(w - 2.0 * k).min(h - 2.0 * k);
    if s <= 0.0 {
        return;
    }
    let (bx, by) = (x + (w - s) / 2.0, y + (h - s) / 2.0);
    let corners = [[bx, by], [bx + s, by], [bx + s, by + s], [bx, by + s]];
    if on {
        let green = [0x10, 0x7C, 0x41];
        pg.polygon(&corners, Some(green), Some((green, 0.75 * k)));
        let tick = [[bx + 0.22 * s, by + 0.52 * s], [bx + 0.42 * s, by + 0.72 * s], [bx + 0.78 * s, by + 0.30 * s]];
        pg.polyline(&tick, 0.14 * s, [0xFF, 0xFF, 0xFF], &[]);
    } else {
        pg.polygon(&corners, Some([0xFF, 0xFF, 0xFF]), Some(([110, 110, 110], 0.75 * k)));
    }
}

pub(super) struct PendingPdfLink {
    pub(super) page: usize,
    pub(super) sheet: usize,
    pub(super) rect: (f32, f32, f32, f32),
    pub(super) target: String,
}

pub(super) struct PrintedBlock {
    pub(super) page: usize,
    pub(super) sheet: usize,
    pub(super) rows: (u32, u32),
    pub(super) cols: (u32, u32),
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) k: f32,
    /// The page x that right-to-left sheets are mirrored about; `None` for left-to-right sheets.
    pub(super) mirror_axis: Option<f32>,
}

/// PDF annotations are not affected by graphics clipping, so intersect each cell with its page
/// block before adding a clickable rectangle. A repeated print-title cell may appear on several
/// pages and receives one annotation on each of them.
pub(super) fn draw_block_links(pg: &mut Page, b: &Block, page: usize, pending: &mut Vec<PendingPdfLink>) {
    let range = b.range();
    let (Some(&bx), Some(&by), Some(&right), Some(&bottom)) = (b.xs.first(), b.ys.first(), b.xs.last(), b.ys.last()) else { return };
    let by_start: std::collections::HashMap<CellRef, RangeRef> = block_merges(b.sh, range).into_iter().map(|m| (m.start, m)).collect();
    for (cell, link) in b.sh.hyperlinks.range(range.start..=range.end) {
        if !range.contains(*cell) {
            continue;
        }
        let cell_range = by_start.get(cell).copied().unwrap_or(RangeRef::cell(*cell));
        let (x, y, w, h) = b.rect(cell_range);
        let (x0, y0, x1, y1) = (x.max(bx), y.max(by), (x + w).min(right), (y + h).min(bottom));
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let rect = (b.text_x(x0, x1 - x0), y0, x1 - x0, y1 - y0);
        match crate::links::classify(&link.target) {
            crate::links::LinkKind::External(uri) => pg.link_uri(rect.0, rect.1, rect.2, rect.3, &uri),
            crate::links::LinkKind::Internal(target) if !link.external => {
                pending.push(PendingPdfLink { page, sheet: b.si, rect, target });
            }
            _ => {}
        }
    }
}

pub(super) fn printed_target(wb: &Workbook, blocks: &[PrintedBlock], source_sheet: usize, target: &str) -> Option<(usize, f32, f32)> {
    let target = target.trim().trim_start_matches('#').trim_start_matches('=');
    let (sheet_name, range) = split_sheet(target);
    let sheet = sheet_name.map_or(Some(source_sheet), |name| wb.sheet_index(&name))?;
    let cell = RangeRef::parse(&range.replace('$', ""))?.start;
    let sh = wb.sheet(sheet)?;
    let block = blocks.iter().find(|b| b.sheet == sheet && (b.rows.0..=b.rows.1).contains(&cell.row) && (b.cols.0..=b.cols.1).contains(&cell.col))?;
    let x = block.x + (sh.col_left(cell.col) - sh.col_left(block.cols.0)) as f32 * block.k;
    let x = match block.mirror_axis {
        Some(axis) => axis - x - (sh.col_left(cell.col.saturating_add(1)) - sh.col_left(cell.col)) as f32 * block.k,
        None => x,
    };
    let y = block.y + (sh.row_top(cell.row) - sh.row_top(block.rows.0)) as f32 * block.k;
    Some((block.page, x, y))
}

pub(super) fn draw_headings(pg: &mut Page, b: &Block, col_heads: bool, row_heads: bool, at: (f32, f32), size: (f32, f32)) {
    let fill = [0xF2, 0xF2, 0xF2];
    let line = [0xA0, 0xA0, 0xA0];
    let fsize = 9.0 * b.scale.max(0.1);
    let lw = (0.35 * b.scale).clamp(0.1, 0.5);
    if col_heads {
        let y = at.1;
        for col in b.cols.0..=b.cols.1 {
            let (x0, x1) = (b.x(col), b.x(col + 1));
            if x1 - x0 <= 0.0 {
                continue;
            }
            pg.fill_rect(b.text_x(x0, x1 - x0), y, x1 - x0, size.1, fill);
            pg.rect_stroke(b.text_x(x0, x1 - x0), y, x1 - x0, size.1, lw, line);
            let t = gridcraft_core::col_to_letters(col);
            let tw = gridcraft_pdf::text_width(Font::Helvetica, fsize, &t);
            pg.text(b.text_x(x0 + (x1 - x0 - tw) / 2.0, tw), y + size.1 / 2.0 + fsize * 0.35, fsize, Font::Helvetica, [0x40, 0x40, 0x40], &t);
        }
    }
    if row_heads {
        let x = at.0;
        for row in b.rows.0..=b.rows.1 {
            let (y0, y1) = (b.y(row), b.y(row + 1));
            if y1 - y0 <= 0.0 {
                continue;
            }
            pg.fill_rect(b.text_x(x, size.0), y0, size.0, y1 - y0, fill);
            pg.rect_stroke(b.text_x(x, size.0), y0, size.0, y1 - y0, lw, line);
            let t = (row + 1).to_string();
            let tw = gridcraft_pdf::text_width(Font::Helvetica, fsize, &t);
            pg.text(b.text_x(x + (size.0 - tw) / 2.0, tw), y0 + (y1 - y0) / 2.0 + fsize * 0.35, fsize, Font::Helvetica, [0x40, 0x40, 0x40], &t);
        }
    }
}

/// Builds a block for rows × cols positioned at (x, y) on the page.
pub(super) fn make_block<'a>(
    wb: &'a Workbook,
    si: usize,
    sh: &'a Sheet,
    rows: (u32, u32),
    cols: (u32, u32),
    origin: (f32, f32),
    lay: &SheetLayout,
) -> Block<'a> {
    let (x, y) = origin;
    let k = lay.k();
    let mut xs = Vec::with_capacity((cols.1 - cols.0 + 2) as usize);
    let mut acc = x;
    xs.push(acc);
    for c in cols.0..=cols.1 {
        acc += sh.col_width(c) * k;
        xs.push(acc);
    }
    let mut ys = Vec::with_capacity((rows.1 - rows.0 + 2) as usize);
    let mut acc = y;
    ys.push(acc);
    for r in rows.0..=rows.1 {
        acc += sh.row_height(r) * k;
        ys.push(acc);
    }
    Block { wb, si, sh, rows, cols, xs, ys, k, scale: lay.scale, mirror_axis: lay.page_w }
}

#[cfg(test)]
mod merge_tests {
    use super::{CellRef, Covered, RangeRef, Sheet, block_merges};

    fn r(a: &str) -> RangeRef {
        RangeRef::parse(a).unwrap()
    }

    #[test]
    fn block_merges_drop_overlaps_and_cover_like_a_scan() {
        let mut sh = Sheet::new("S");
        sh.merges = vec![r("B2:C3"), r("E1:F8"), r("B2:B5"), r("A10:D10"), r("H3:H4"), r("Z1:Z2"), r("C3:D4"), r("A1:A1048576")];
        let block = r("B1:H12");
        let merges = block_merges(&sh, block);
        // B2:B5 and C3:D4 overlap B2:C3, which comes first; Z1:Z2 is outside the block.
        assert_eq!(merges, vec![r("E1:F8"), r("B2:C3"), r("H3:H4"), r("A10:D10")]);
        let mut covered = Covered::new(&merges, block);
        for c in block.iter() {
            assert_eq!(covered.covers(c), merges.iter().any(|m| m.contains(c)), "{}", c.a1());
        }
        // Cells asked sparsely, still in order.
        let mut covered = Covered::new(&merges, block);
        for (a, want) in [("C1", false), ("F5", true), ("G5", false), ("D10", true), ("H12", false)] {
            assert_eq!(covered.covers(CellRef::parse(a).unwrap()), want, "{a}");
        }
    }

    #[test]
    fn many_stacked_merges_are_swept_once() {
        // 50,000 merges over the same cells: one is kept, and the block is checked against it
        // without comparing every cell with every merge.
        let mut sh = Sheet::new("S");
        sh.merges = (0..50_000u32).map(|i| RangeRef::new(CellRef::new(0, 0), CellRef::new(199, 1 + i % 50))).collect();
        let block = r("A1:AZ200");
        let merges = block_merges(&sh, block);
        assert_eq!(merges, vec![r("A1:B200")]);
        let mut covered = Covered::new(&merges, block);
        assert_eq!(block.iter().filter(|c| covered.covers(*c)).count(), 400);
    }
}
