//! Tables: cell fills from the table style (or overrides), borders, and cell text.

use deckcraft_color::{ColorTransform, Rgba, SchemeSlot};
use deckcraft_model::resolve::Ctx;
use deckcraft_model::style::{ColorRef, Fill};
use deckcraft_model::table::{Cell, Table, TableBorder, TablePart, TablePartStyle, TableStyleDef};
use deckcraft_model::text::{Anchor, TextBody, TextDir};
use deckcraft_model::{Presentation, Shape};
use deckcraft_text::Fields;
use kurbo::{Affine, BezPath, Rect, Shape as _};
use vello_cpu::{RenderContext, peniko};

use crate::color;

/// Style colours for a cell: (fill, text colour, bold).
pub struct CellLook {
    pub fill: Option<Rgba>,
    pub text: Option<Rgba>,
    pub bold: bool,
    pub border: Option<(Rgba, f64)>,
    /// Left, right, top and bottom borders from a file-defined style (a fully transparent colour
    /// means no border); `None` falls back to `border`.
    pub sides: [Option<(Rgba, f64)>; 4],
    /// A file-defined style's fill as a paint (gradients, see-through colours), with its
    /// placeholder colour; drawn instead of `fill`.
    pub paint: Option<(Fill, Option<Rgba>)>,
}

fn accent(ctx: &Ctx, style: &str) -> Rgba {
    let slot = match style.rsplit('-').next().unwrap_or("accent1") {
        "accent2" => SchemeSlot::Accent2,
        "accent3" => SchemeSlot::Accent3,
        "accent4" => SchemeSlot::Accent4,
        "accent5" => SchemeSlot::Accent5,
        "accent6" => SchemeSlot::Accent6,
        "tx1" => SchemeSlot::Tx1,
        _ => SchemeSlot::Accent1,
    };
    ctx.color(&ColorRef::scheme(slot), None)
}

/// The look of cell (`r`, `c`) from a table style the file defines: each part that applies to
/// the cell (whole table, bands, first and last rows and columns, corners, in that order) sets the
/// fill, text and the borders on its region's edges (its inside lines within it).
fn look_def(ctx: &Ctx, def: &TableStyleDef, t: &Table, r: usize, c: usize) -> CellLook {
    let (rows, cols) = (t.rows.len(), t.cols.len());
    let (r1, c1) = t.cell(r, c).map(|cell| (r + cell.row_span.max(1) as usize - 1, c + cell.grid_span.max(1) as usize - 1)).unwrap_or((r, c));
    let (r1, c1) = (r1.min(rows.saturating_sub(1)), c1.min(cols.saturating_sub(1)));
    let last_r = rows.saturating_sub(1);
    let last_c = cols.saturating_sub(1);
    let header = t.first_row && r == 0;
    let total = t.last_row && r1 == last_r && rows > 1;
    let first_col = t.first_col && c == 0;
    let last_col = t.last_col && c1 == last_c && cols > 1;
    let body_r = r.saturating_sub(usize::from(t.first_row));
    let body_c = c.saturating_sub(usize::from(t.first_col));
    let mut look = CellLook { fill: None, text: None, bold: false, border: None, sides: [None; 4], paint: None };
    for part in TablePart::ALL {
        // (applies, region rows, region cols)
        let (on, region) = match part {
            TablePart::WholeTbl => (true, (0, last_r, 0, last_c)),
            TablePart::Band1H => (t.band_row && !header && !total && body_r % 2 == 0, (r, r1, 0, last_c)),
            TablePart::Band2H => (t.band_row && !header && !total && body_r % 2 == 1, (r, r1, 0, last_c)),
            TablePart::Band1V => (t.band_col && !first_col && !last_col && body_c % 2 == 0, (0, last_r, c, c1)),
            TablePart::Band2V => (t.band_col && !first_col && !last_col && body_c % 2 == 1, (0, last_r, c, c1)),
            TablePart::LastCol => (last_col, (0, last_r, c, last_c)),
            TablePart::FirstCol => (first_col, (0, last_r, 0, c1)),
            TablePart::LastRow => (total, (r, last_r, 0, last_c)),
            TablePart::FirstRow => (header, (0, r1, 0, last_c)),
            TablePart::SeCell => (total && last_col, (r, r1, c, c1)),
            TablePart::SwCell => (total && first_col, (r, r1, c, c1)),
            TablePart::NeCell => (header && last_col, (r, r1, c, c1)),
            TablePart::NwCell => (header && first_col, (r, r1, c, c1)),
        };
        if !on {
            continue;
        }
        let Some(ps) = def.part(part) else { continue };
        if let Some(p) = style_fill(ctx, ps) {
            look.fill = fill_color(ctx, &p.0, p.1);
            look.paint = Some(p);
        }
        if let Some(tc) = &ps.text_color {
            look.text = Some(ctx.color(tc, None));
        }
        if let Some(b) = ps.bold {
            look.bold = b;
        }
        let (r0, rr1, c0, cc1) = region;
        // Left, right, top, bottom of the cell: the part's edge or its inside line.
        let pick = [if c == c0 { 0 } else { 5 }, if c1 == cc1 { 1 } else { 5 }, if r == r0 { 2 } else { 4 }, if r1 == rr1 { 3 } else { 4 }];
        for (side, i) in pick.iter().enumerate() {
            if let (Some(Some(b)), Some(slot)) = (ps.borders.get(*i), look.sides.get_mut(side)) {
                *slot = Some(border_look(ctx, b));
            }
        }
    }
    look
}

/// A style part's fill, a theme fill reference resolved to the theme's fill with its colour as
/// the placeholder colour.
fn style_fill(ctx: &Ctx, ps: &TablePartStyle) -> Option<(Fill, Option<Rgba>)> {
    if let Some((idx, col)) = &ps.fill_ref {
        let ph = ctx.color(col, None);
        return Some(match ctx.master.theme.format.fills.get((*idx as usize).saturating_sub(1)) {
            Some(f) if *idx > 0 => (f.clone(), Some(ph)),
            _ if *idx == 0 => (Fill::None, None),
            _ => (Fill::solid(ColorRef::rgb(ph)), None),
        });
    }
    ps.fill.clone().map(|f| (f, None))
}

/// A fill's colour for a table cell (gradients and pictures by their first colour).
fn fill_color(ctx: &Ctx, f: &Fill, ph: Option<Rgba>) -> Option<Rgba> {
    match f {
        Fill::None => Some(Rgba::TRANSPARENT),
        Fill::Solid { color: c } => Some(ctx.color(c, ph)),
        Fill::Gradient(g) => g.stops.first().map(|s| ctx.color(&s.color, ph)),
        Fill::Pattern(p) => Some(ctx.color(&p.bg, ph)),
        _ => None,
    }
}

/// A style border's colour and width (a transparent colour for no line).
fn border_look(ctx: &Ctx, b: &TableBorder) -> (Rgba, f64) {
    match b {
        TableBorder::Line { line } => match &line.fill {
            Some(Fill::Solid { color: c }) => (ctx.color(c, None), line.width.unwrap_or(0.75)),
            Some(Fill::Gradient(g)) => (g.stops.first().map(|s| ctx.color(&s.color, None)).unwrap_or(Rgba::BLACK), line.width.unwrap_or(0.75)),
            _ => (Rgba::TRANSPARENT, 0.0),
        },
        TableBorder::Ref { idx, color } => {
            let w = ctx.master.theme.format.lines.get((*idx as usize).saturating_sub(1)).and_then(|l| l.width).unwrap_or(0.75);
            if *idx == 0 { (Rgba::TRANSPARENT, 0.0) } else { (ctx.color(color, None), w) }
        }
    }
}

pub fn resolve_style<'a>(ctx: &'a Ctx, t: &Table) -> Option<&'a TableStyleDef> {
    ctx.pres.table_styles.iter().find(|d| d.id.eq_ignore_ascii_case(&t.style))
}

#[allow(dead_code)]
pub fn look(ctx: &Ctx, t: &Table, r: usize, c: usize) -> CellLook {
    look_with_def(ctx, resolve_style(ctx, t), t, r, c)
}

/// Which parts of the table a cell sits in, for the preset styles.
#[derive(Clone, Copy)]
struct PresetPart {
    header: bool,
    total: bool,
    first_col: bool,
    last_col: bool,
    banded: bool,
}

impl PresetPart {
    /// In a header, total, first or last column: the parts a preset style makes strong.
    fn strong(self) -> bool {
        self.header || self.total || self.first_col || self.last_col
    }
}

/// Tint `k` of accent `a`.
fn tint(a: Rgba, k: i32) -> Rgba {
    deckcraft_color::apply(a, &[ColorTransform::Tint(k)])
}

fn light1_look(a: Rgba, p: PresetPart, on: impl Fn(Rgba) -> Rgba) -> CellLook {
    let fill = p.banded.then(|| tint(a, 20000));
    CellLook { fill, text: fill.map(on), bold: p.strong(), border: if p.header { Some((a, 1.0)) } else { None }, sides: [None; 4], paint: None }
}

fn light2_look(a: Rgba, p: PresetPart) -> CellLook {
    CellLook {
        fill: if p.header { Some(a) } else { None },
        text: if p.header { Some(Rgba::WHITE) } else { None },
        bold: p.header || p.total || p.first_col,
        border: Some((a, 1.0)),
        sides: [None; 4],
        paint: None,
    }
}

fn dark1_look(a: Rgba, p: PresetPart) -> CellLook {
    CellLook {
        fill: Some(if p.header {
            Rgba::BLACK
        } else if p.banded {
            deckcraft_color::apply(a, &[ColorTransform::Shade(60000)])
        } else {
            deckcraft_color::apply(a, &[ColorTransform::Shade(75000)])
        }),
        text: Some(Rgba::WHITE),
        bold: p.header || p.total || p.first_col,
        border: Some((Rgba::WHITE, 1.0)),
        sides: [None; 4],
        paint: None,
    }
}

fn medium4_look(a: Rgba, p: PresetPart, on: impl Fn(Rgba) -> Rgba) -> CellLook {
    let fill = if p.header || p.banded { tint(a, 40000) } else { tint(a, 20000) };
    CellLook { fill: Some(fill), text: Some(on(fill)), bold: p.strong(), border: Some((a, 0.75)), sides: [None; 4], paint: None }
}

/// medium2: strong header, banded light tints, white grid.
fn medium2_look(a: Rgba, p: PresetPart, on: impl Fn(Rgba) -> Rgba) -> CellLook {
    let strong = p.strong();
    let fill = if strong {
        a
    } else if p.banded {
        tint(a, 40000)
    } else {
        tint(a, 20000)
    };
    CellLook {
        fill: Some(fill),
        text: Some(if strong { Rgba::WHITE } else { on(fill) }),
        bold: strong,
        border: Some((Rgba::WHITE, 1.0)),
        sides: [None; 4],
        paint: None,
    }
}

pub fn look_with_def(ctx: &Ctx, def: Option<&TableStyleDef>, t: &Table, r: usize, c: usize) -> CellLook {
    if let Some(def) = def {
        return look_def(ctx, def, t, r, c);
    }
    let a = accent(ctx, &t.style);
    let kind = t.style.split('-').next().unwrap_or("medium2");
    let header = t.first_row && r == 0;
    let total = t.last_row && r + 1 == t.rows.len();
    let first_col = t.first_col && c == 0;
    let last_col = t.last_col && c + 1 == t.cols.len();
    let body_r = if t.first_row { r.wrapping_sub(1) } else { r };
    let band = t.band_row && !header && !total && body_r % 2 == 0;
    let band_c = t.band_col && c.is_multiple_of(2);
    let p = PresetPart { header, total, first_col, last_col, banded: band || band_c };
    let text_color = ctx.color(&ColorRef::scheme(SchemeSlot::Tx1), None);
    // Text on a tinted cell never inherits the text colour, which a dark theme makes light and
    // unreadable on the light tint: PowerPoint's medium styles set it to the theme's dark colour
    // (dk1). A dark theme may keep that in dk1 and map tx1 away from it, or (Nocturne) put it in
    // lt1, so of the two the one contrasting more with the cell's fill is used.
    let dk1 = ctx.color(&ColorRef::scheme(SchemeSlot::Dk1), None);
    let lt1 = ctx.color(&ColorRef::scheme(SchemeSlot::Lt1), None);
    let on = |fill: Rgba| if dk1.contrast(fill) >= lt1.contrast(fill) { dk1 } else { lt1 };
    match kind {
        "none" => CellLook { fill: None, text: None, bold: false, border: None, sides: [None; 4], paint: None },
        "grid" => CellLook { fill: None, text: None, bold: false, border: Some((text_color, 0.75)), sides: [None; 4], paint: None },
        "light1" => light1_look(a, p, on),
        "light2" => light2_look(a, p),
        "dark1" => dark1_look(a, p),
        "medium4" => medium4_look(a, p, on),
        _ => medium2_look(a, p, on),
    }
}

/// Running edges 0, s0, s0+s1, … of a list of sizes (negative sizes count as 0).
fn edges(sizes: &[f64]) -> Vec<f64> {
    std::iter::once(0.0)
        .chain(sizes.iter().scan(0.0, |a, s| {
            *a += s.max(0.0);
            Some(*a)
        }))
        .collect()
}

/// A cell's text as drawn: its margins, anchor and direction, plus the table style's text colour and bold.
pub fn cell_body(rctx: &Ctx, t: &Table, r: usize, c: usize, cell: &Cell) -> TextBody {
    cell_body_with_def(rctx, resolve_style(rctx, t), t, r, c, cell)
}

pub fn cell_body_with_def(rctx: &Ctx, def: Option<&TableStyleDef>, t: &Table, r: usize, c: usize, cell: &Cell) -> TextBody {
    let lk = look_with_def(rctx, def, t, r, c);
    let mut body = cell.text.clone();
    let [ml, mr, mt, mb] = cell.margins.unwrap_or([7.2, 7.2, 3.6, 3.6]);
    body.body.inset_l = Some(ml);
    body.body.inset_r = Some(mr);
    body.body.inset_t = Some(mt);
    body.body.inset_b = Some(mb);
    body.body.anchor = Some(cell.anchor.unwrap_or(Anchor::Top));
    if cell.vertical.is_some() {
        body.body.vert = cell.vertical;
    }
    for p in &mut body.paragraphs {
        for run in &mut p.runs {
            if run.props.fill.is_none()
                && let Some(tc) = lk.text
            {
                run.props.fill = Some(Fill::solid(ColorRef::rgb(tc)));
            }
            if lk.bold && run.props.bold.is_none() {
                run.props.bold = Some(true);
            }
        }
    }
    body
}

/// Heights the rows are drawn at. A stored row height (`a:tr/@h`) is a minimum: the row grows so the
/// text of each of its cells fits, as in PowerPoint. Files from pandoc or python-pptx store 0.
/// Cells spanning several rows and vertical text keep the stored height.
pub fn row_heights(rctx: &Ctx, t: &Table, fields: &dyn Fields) -> Vec<f64> {
    row_heights_with_def(rctx, resolve_style(rctx, t), t, fields)
}

/// [`row_heights`] with the table's style already resolved, for a caller that needs it too.
fn row_heights_with_def(rctx: &Ctx, style_def: Option<&TableStyleDef>, t: &Table, fields: &dyn Fields) -> Vec<f64> {
    let xs = edges(&t.cols);
    let mut out = Vec::with_capacity(t.rows.len());
    for (ri, row) in t.rows.iter().enumerate() {
        let mut h = row.height.max(0.0);
        for (ci, cell) in row.cells.iter().enumerate().take(t.cols.len()) {
            if cell.is_covered() || cell.row_span > 1 || !matches!(cell.vertical, None | Some(TextDir::Horizontal)) {
                continue;
            }
            let c1 = ci.saturating_add(cell.grid_span.max(1) as usize).min(t.cols.len());
            let (Some(x0), Some(x1)) = (xs.get(ci), xs.get(c1)) else { continue };
            let body = cell_body_with_def(rctx, style_def, t, ri, ci, cell);
            let tmp = Shape::table_cell(body.clone());
            let need = deckcraft_text::fit_height(rctx, &tmp, &body, Rect::new(*x0, 0.0, *x1, h.max(1.0)), fields);
            if need.is_finite() {
                h = h.max(need.min(20_000.0));
            }
        }
        out.push(h);
    }
    out
}

/// Cell rectangles in table-local space, honouring merges: (row, col, rect) for visible cells.
/// `heights` are the drawn row heights ([`row_heights`]).
pub fn cell_rects(t: &Table, heights: &[f64]) -> Vec<(usize, usize, Rect)> {
    let mut out = vec![];
    let xs = edges(&t.cols);
    let ys = edges(heights);
    for (ri, row) in t.rows.iter().enumerate() {
        for (ci, cell) in row.cells.iter().enumerate().take(t.cols.len()) {
            if cell.is_covered() {
                continue;
            }
            let c1 = (ci + cell.grid_span.max(1) as usize).min(t.cols.len());
            let r1 = (ri + cell.row_span.max(1) as usize).min(t.rows.len());
            let (Some(x0), Some(x1), Some(y0), Some(y1)) = (xs.get(ci), xs.get(c1), ys.get(ri), ys.get(r1)) else { continue };
            out.push((ri, ci, Rect::new(*x0, *y0, *x1, *y1)));
        }
    }
    out
}

/// Can a cell's text be left undrawn because it lands wholly outside the `target` (pixels)? Only
/// for cells whose text stays inside the cell: its row grew to fit it (one row, horizontal text,
/// not at the height cap), so nothing overflows. A margin covers italic overhang and the like.
/// A table of thousands of rows otherwise lays out every cell's text off the slide's bottom.
fn text_off_target(cell: &Cell, rect: Rect, m: Affine, target: (u16, u16)) -> bool {
    if cell.row_span > 1 || !matches!(cell.vertical, None | Some(TextDir::Horizontal)) || rect.height() >= 20_000.0 {
        return false;
    }
    let b = m.transform_rect_bbox(rect).inflate(64.0, 64.0);
    let view = Rect::new(0.0, 0.0, f64::from(target.0), f64::from(target.1));
    b.is_finite() && (b.x1 < view.x0 || b.y1 < view.y0 || b.x0 > view.x1 || b.y0 > view.y1)
}

#[cfg(test)]
thread_local! {
    /// Cells whose text `draw` laid out on this thread (tests check off-slide cells are skipped).
    pub(crate) static TEXT_CELLS_DRAWN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Draws the table; returns the box its cells cover (table space), which grows with their text.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ctx: &mut RenderContext,
    pres: &Presentation,
    rctx: &Ctx,
    t: &Table,
    m: Affine,
    _w: f64,
    _h: f64,
    fields: &dyn deckcraft_text::Fields,
    editing: bool,
) -> Option<kurbo::Rect> {
    let _ = pres;
    let style_def = resolve_style(rctx, t);
    let rects = cell_rects(t, &row_heights_with_def(rctx, style_def, t, fields));
    let covered = rects.iter().map(|(_, _, r)| *r).reduce(|a, b| a.union(b));
    let style_bg = style_def.and_then(|d| d.background.as_ref()).and_then(|b| style_fill(rctx, b));
    let own_bg = t.background.clone().map(|f| (f, None));
    if let Some(all) = rects.iter().map(|(_, _, r)| *r).reduce(|a, b| a.union(b)) {
        for (bg, ph) in style_bg.into_iter().chain(own_bg) {
            ctx.set_transform(m);
            crate::paint::fill_path(ctx, rctx, &bg, ph, &all.to_path(0.1), all, m, deckcraft_geom::preset::FillMode::Norm, 1.0, rctx.pres);
        }
    }
    for (r, c, rect) in &rects {
        let Some(cell) = t.cell(*r, *c) else { continue };
        let lk = look_with_def(rctx, style_def, t, *r, *c);
        ctx.set_transform(m);
        match &cell.fill {
            Some(f) => {
                crate::paint::fill_path(ctx, rctx, f, None, &rect.to_path(0.1), *rect, m, deckcraft_geom::preset::FillMode::Norm, 1.0, rctx.pres)
            }
            None => match &lk.paint {
                Some((f, ph)) => {
                    crate::paint::fill_path(ctx, rctx, f, *ph, &rect.to_path(0.1), *rect, m, deckcraft_geom::preset::FillMode::Norm, 1.0, rctx.pres)
                }
                None => {
                    if let Some(fc) = lk.fill.filter(|f| f.a > 0) {
                        ctx.set_paint(color(fc, 1.0));
                        ctx.fill_rect(rect);
                    }
                }
            },
        }
    }
    // Diagonal borders, under the edges: drawn like any line (dashes, compound, caps).
    for (r, c, rect) in &rects {
        let Some(cell) = t.cell(*r, *c) else { continue };
        for (line, a, b) in [(&cell.diag_down, (rect.x0, rect.y0), (rect.x1, rect.y1)), (&cell.diag_up, (rect.x0, rect.y1), (rect.x1, rect.y0))] {
            let Some(l) = line.as_ref().filter(|l| l.fill.as_ref().is_some_and(|f| !f.is_none())) else { continue };
            let mut p = BezPath::new();
            p.move_to(a);
            p.line_to(b);
            crate::stroke(ctx, rctx, l, None, &p, None, m, l.width.unwrap_or(1.0).max(0.0), 1.0);
        }
    }
    // Borders: explicit cell borders, else the style's grid.
    for (r, c, rect) in &rects {
        let Some(cell) = t.cell(*r, *c) else { continue };
        let lk = look_with_def(rctx, style_def, t, *r, *c);
        let edges = [
            (rect.x0, rect.y0, rect.x0, rect.y1),
            (rect.x1, rect.y0, rect.x1, rect.y1),
            (rect.x0, rect.y0, rect.x1, rect.y0),
            (rect.x0, rect.y1, rect.x1, rect.y1),
        ];
        for (i, e) in edges.iter().enumerate() {
            let line = cell.borders.get(i).and_then(|b| b.as_ref());
            let (col, wd) = match line {
                // An explicit border is drawn like any line: dashed, compound, gradient.
                Some(l) => {
                    if l.fill.as_ref().is_some_and(|f| !f.is_none()) {
                        let mut p = BezPath::new();
                        p.move_to((e.0, e.1));
                        p.line_to((e.2, e.3));
                        crate::stroke(ctx, rctx, l, None, &p, None, m, l.width.unwrap_or(1.0).max(0.25), 1.0);
                    }
                    continue;
                }
                None => match lk.sides.get(i).copied().flatten().or(lk.border) {
                    Some(b) if b.0.a > 0 && b.1 > 0.0 => b,
                    _ => continue,
                },
            };
            let mut p = BezPath::new();
            p.move_to((e.0, e.1));
            p.line_to((e.2, e.3));
            ctx.set_transform(m);
            ctx.set_paint(color(col, 1.0));
            ctx.set_stroke(kurbo::Stroke::new(wd.max(0.25)));
            ctx.stroke_path(&p);
        }
    }
    // Text.
    for (r, c, rect) in &rects {
        let Some(cell) = t.cell(*r, *c) else { continue };
        if (cell.text.is_empty() && !editing) || text_off_target(cell, *rect, m, (ctx.width(), ctx.height())) {
            continue;
        }
        let body = cell_body_with_def(rctx, style_def, t, *r, *c, cell);
        let tmp = Shape::table_cell(body.clone());
        #[cfg(test)]
        TEXT_CELLS_DRAWN.with(|n| n.set(n.get() + 1));
        let mut l = deckcraft_text::layout(rctx, &tmp, &body, &deckcraft_text::Opts { rect: *rect, fields, prompt_color: None, no_shrink: false });
        if editing {
            crate::preview_empty_bullets(&mut l);
        }
        crate::draw_layout(ctx, &l, *rect, m);
        let _ = peniko::Fill::NonZero;
    }
    covered
}
