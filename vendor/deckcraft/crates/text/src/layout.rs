use super::breaking::{BreakParams, RawLine, Tab, break_lines, line_metrics, measure_str, tab_advance, tab_segment, tabs_of};
use super::run::{Deco, DecoLine, GlyphRun};
use super::shape::{SUB_SUPER, Upright, shape_para};
use super::style::{Cell, Opts, ParaShaped, Style, spacing_pts};
use super::{
    Align, Anchor, Arc, AutoFit, BodyProps, Bullet, Caps, Ctx, FontFace, LineInfo, Rect, Shape, Spacing, Strike, TextBody, TextDir, TextLayout, bidi,
    joins_following, joins_preceding, resolve,
};

#[cfg(test)]
thread_local! {
    /// How many times `layout_scaled` ran on this thread (tests count autofit's passes).
    pub(crate) static LAYOUT_PASSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Lay out a shape's text body.
pub fn layout(ctx: &Ctx, shape: &Shape, body: &TextBody, opts: &Opts) -> TextLayout {
    let bp = resolve::body(ctx, shape);
    let autofit = bp.autofit.unwrap_or(AutoFit::None);
    // Shrink on overflow starts from the scale the file was saved with (`fontScale`,
    // `lnSpcReduction`), which is what PowerPoint draws; editing the text resets it.
    let (scale0, reduce0, no_saved_scale) = match (autofit, opts.no_shrink) {
        (AutoFit::Shrink { font_scale, line_reduction }, false) => (
            font_scale.filter(|s| s.is_finite()).map(|s| s.clamp(0.1, 1.0)).unwrap_or(1.0),
            if line_reduction.is_finite() { line_reduction.clamp(0.0, 0.2) } else { 0.0 },
            font_scale.is_none(),
        ),
        _ => (1.0, 0.0, false),
    };
    let mut l = layout_scaled(ctx, shape, body, &bp, opts, scale0, reduce0);
    // A scale PowerPoint saved is drawn as it is: PowerPoint fits the text again only when it
    // changes (and editing resets the saved scale), not when a file opens, even where our
    // metrics would call it overflowing (trailing empty paragraphs, substituted fonts).
    // A body whose file has no `fontScale` at all (`<a:normAutofit/>`) is the schema default
    // 100 % and is drawn as it is too, not refitted on open (#724).
    let saved = no_saved_scale || scale0 < 1.0 || reduce0 > 0.0;
    if let (AutoFit::Shrink { .. }, false) = (autofit, opts.no_shrink)
        && l.overflow
        && !saved
    {
        // Text that overflows even at the smallest scale gets that scale: every larger one
        // overflows too, so searching would only lay it out a dozen more times (seconds for a
        // body of tens of thousands of paragraphs, on every redraw of the slide).
        let smallest = layout_scaled(ctx, shape, body, &bp, opts, 0.1, 0.2);
        if smallest.overflow {
            return smallest;
        }
        // Binary search the largest scale that fits (steps like 90%, 80%… with line reduction).
        let (mut lo, mut hi) = (0.1_f64, scale0);
        let mut best = Some(smallest);
        for _ in 0..12 {
            let mid = (lo + hi) / 2.0;
            let red: f64 = ((1.0 - mid) * 0.8_f64).min(0.2).max(reduce0);
            let t = layout_scaled(ctx, shape, body, &bp, opts, mid, red);
            if t.overflow {
                hi = mid;
            } else {
                lo = mid;
                best = Some(t);
            }
        }
        if let Some(b) = best {
            l = b;
        }
    }
    l
}

/// Geometry of the text box shared by every line: rotation, inner rectangle, wrapping and columns.
struct BoxGeom {
    vert: TextDir,
    rotation: f64,
    inner: Rect,
    wrap: bool,
    ncols: usize,
    col_gap: f64,
    col_w: f64,
    rtl_cols: bool,
}

impl BoxGeom {
    fn new(shape: &Shape, bp: &BodyProps, text_rect: Rect) -> Self {
        let vert = bp.vert.unwrap_or(TextDir::Horizontal);
        let rotation = match vert {
            TextDir::Vertical | TextDir::EaVertical | TextDir::Stacked | TextDir::StackedRtl | TextDir::MongolianVertical => 90.0,
            TextDir::Vertical270 => 270.0,
            TextDir::Horizontal => 0.0,
        } + bp.rot.unwrap_or(0.0);
        // For vertical text, lay out in a box with swapped dimensions centred on the text rect.
        let r = if (rotation - 90.0).abs() < 1e-6 || (rotation - 270.0).abs() < 1e-6 {
            let c = text_rect.center();
            Rect::new(c.x - text_rect.height() / 2.0, c.y - text_rect.width() / 2.0, c.x + text_rect.height() / 2.0, c.y + text_rect.width() / 2.0)
        } else {
            text_rect
        };
        // Insets wider than the shape scale down proportionally to fit inside it (#1632).
        let (mut ins_l, mut ins_r) = (bp.inset_l.unwrap_or(7.2), bp.inset_r.unwrap_or(7.2));
        let (mut ins_t, mut ins_b) = (bp.inset_t.unwrap_or(3.6), bp.inset_b.unwrap_or(3.6));
        let w = r.width().max(0.0);
        let total_x = ins_l + ins_r;
        if ins_l >= 0.0 && ins_r >= 0.0 && total_x > w && total_x > 0.0 {
            let s = w / total_x;
            ins_l *= s;
            ins_r *= s;
        }
        let h = r.height().max(0.0);
        let total_y = ins_t + ins_b;
        if ins_t >= 0.0 && ins_b >= 0.0 && total_y > h && total_y > 0.0 {
            let s = h / total_y;
            ins_t *= s;
            ins_b *= s;
        }
        let inner = Rect::new(r.x0 + ins_l, r.y0 + ins_t, (r.x1 - ins_r).max(r.x0 + ins_l), (r.y1 - ins_b).max(r.y0 + ins_t));
        // Text that follows a WordArt arch or circle is set on one line per paragraph.
        let follow = bp.warp.as_deref().is_some_and(deckcraft_geom::warp::is_follow);
        let wrap = bp.wrap.unwrap_or(true) && !follow && !matches!(bp.autofit, Some(AutoFit::Shape) if shape.text_box && !bp.wrap.unwrap_or(true));
        let ncols = bp.columns.unwrap_or(1).clamp(1, 16) as usize;
        let col_gap = bp.col_spacing.unwrap_or(0.0).max(0.0);
        let col_w = ((inner.width() - col_gap * (ncols as f64 - 1.0)) / ncols as f64).max(1.0);
        let rtl_cols = bp.rtl_col.unwrap_or(false) && ncols > 1;
        BoxGeom { vert, rotation, inner, wrap, ncols, col_gap, col_w, rtl_cols }
    }

    /// East Asian vertical text: CJK stands upright in text turned a quarter turn; stacked text
    /// stands every character upright, one under the other.
    fn upright(&self) -> Upright {
        match self.vert {
            TextDir::EaVertical => Upright::Cjk,
            TextDir::Stacked | TextDir::StackedRtl => Upright::All,
            _ => Upright::None,
        }
    }

    /// Mongolian vertical and stacked text: turned like vert, their lines run from left to right
    /// (as PowerPoint draws both).
    fn lines_ltr(&self) -> bool {
        matches!(self.vert, TextDir::MongolianVertical | TextDir::Stacked)
    }

    /// Where column `c` starts: from the left, or from the right with `rtlCol` (PowerPoint puts the
    /// first column on the right).
    fn col_x(&self, c: usize) -> f64 {
        (if self.rtl_cols { self.ncols.saturating_sub(1 + c) } else { c }) as f64 * (self.col_w + self.col_gap)
    }
}

/// Number shown for a paragraph at list level `lvl`, advancing the per-level counters. `start_at`
/// is the paragraph's numbering start (`None`: not numbered); `run_start` holds the start each
/// level's current run of numbers began with, and a paragraph that changes it starts a new run
/// (PowerPoint's "Start at").
fn next_number(numbering: &mut [u32], run_start: &mut [u32], lvl: usize, start_at: Option<u32>, empty: bool) -> Option<u32> {
    match start_at {
        Some(start) if !empty => {
            if let (Some(rs), Some(n)) = (run_start.get_mut(lvl), numbering.get_mut(lvl))
                && *rs != start
            {
                *rs = start;
                *n = 0;
            }
            if let Some(n) = numbering.get_mut(lvl) {
                *n += 1;
            }
            for deeper in numbering.iter_mut().skip(lvl + 1) {
                *deeper = 0;
            }
            numbering.get(lvl).copied()
        }
        // Preview the next number without consuming it: empty paragraphs do not advance
        // numbering in the saved document or affect later populated paragraphs. A paragraph
        // with its own start would begin a new run, so it previews that run's first number.
        Some(start) => {
            if run_start.get(lvl).is_some_and(|rs| *rs != start) {
                Some(1)
            } else {
                numbering.get(lvl).map(|n| n.saturating_add(1))
            }
        }
        None => {
            if !empty {
                for n in numbering.iter_mut().skip(lvl) {
                    *n = 0;
                }
            }
            None
        }
    }
}

fn shape_paragraphs(ctx: &Ctx, shape: &Shape, body: &TextBody, opts: &Opts, scale: f64, upright: Upright) -> Vec<ParaShaped> {
    let mut numbering: Vec<u32> = vec![0; 9];
    let mut run_start: Vec<u32> = vec![0; 9];
    let mut shaped_paras: Vec<ParaShaped> = Vec::with_capacity(body.paragraphs.len());
    for (pi, para) in body.paragraphs.iter().enumerate() {
        let lvl = para.level.min(8) as usize;
        let pp = resolve::para(ctx, shape, para);
        let start_at = match &pp.bullet {
            Some(Bullet::AutoNum { start_at, .. }) => Some(*start_at),
            _ => None,
        };
        let number = next_number(&mut numbering, &mut run_start, lvl, start_at, para.is_empty());
        shaped_paras.push(shape_para(ctx, shape, body, pi, scale, opts, number, upright));
    }
    shaped_paras
}

/// A laid-out line with its placement data and, on a paragraph's first line, its bullet.
struct PlacedLine {
    info: LineInfo,
    place: LinePlace,
    bullet: Option<(String, f64, f64)>,
    /// Width of the line's text plus its start offset and right margin.
    content_w: f64,
}

/// Vertical flow state while paragraphs are stacked.
struct Flow {
    y: f64,
    prev_after: f64,
    /// How far the last line's full pitch reaches past the text (exact spacing).
    tail: f64,
    lines: Vec<PlacedLine>,
}

/// Measurements shared by the lines of one paragraph.
struct ParaEnv<'a> {
    p: &'a ParaShaped,
    g: &'a BoxGeom,
    reduce: f64,
    margin: f64,
    indent: f64,
    mr: f64,
    bullet_w: f64,
    first_x: f64,
    rest_x: f64,
    tabs: &'a [Tab],
}

impl ParaEnv<'_> {
    fn start_x(&self, li: usize) -> f64 {
        if li == 0 { self.first_x } else { self.rest_x }
    }
}

pub(crate) fn layout_scaled(ctx: &Ctx, shape: &Shape, body: &TextBody, bp: &BodyProps, opts: &Opts, scale: f64, reduce: f64) -> TextLayout {
    #[cfg(test)]
    LAYOUT_PASSES.with(|n| n.set(n.get() + 1));
    let g = BoxGeom::new(shape, bp, opts.rect);
    let shaped_paras = shape_paragraphs(ctx, shape, body, opts, scale, g.upright());
    let mut out = TextLayout { inner: g.inner, font_scale: scale, line_reduction: reduce, rotation: g.rotation, ..Default::default() };
    let first_last = bp.space_first_last.unwrap_or(false);
    let mut flow = Flow { y: 0.0, prev_after: 0.0, tail: 0.0, lines: vec![] };
    for (pi, p) in shaped_paras.iter().enumerate() {
        place_paragraph(p, pi, &g, (reduce, first_last), &mut flow, &mut out);
    }
    // The text ends at the last line's descent under exact spacing; the last paragraph's space
    // after counts only when the body says so.
    let total = (flow.y - flow.tail).max(0.0) + if first_last { flow.prev_after } else { 0.0 };
    out.content_height = total;
    let mut lines = flow.lines;
    assign_columns(&mut lines, &g);
    let used_h = if g.ncols > 1 { lines.iter().map(|l| l.info.bottom).fold(0.0, f64::max) } else { total };
    out.overflow = used_h > g.inner.height() + 0.5 || (g.ncols > 1 && lines.last().is_some_and(|l| l.info.bottom > g.inner.height() + 0.5));
    let (ox, oy) = text_origin(&lines, &g, bp, used_h);
    // Emit runs, in visual order.
    for mut pl in lines {
        position_line(&mut pl.info, ox, oy, &g);
        if let Some(p) = shaped_paras.get(pl.info.para) {
            let to_empty = body.paragraphs.get(pl.info.para).is_some_and(|p| p.is_empty()) && opts.prompt_color.is_none();
            emit_bullet(&mut out, p, &pl, ox + g.col_x(pl.info.column), to_empty);
            emit_line_runs(&mut out, p, &pl.info, &pl.place);
        }
        out.lines.push(pl.info);
    }
    out.boundaries = shaped_paras
        .iter()
        .map(|p| {
            let mut b: Vec<bool> = p.cells.iter().map(|c| c.boundary).collect();
            b.push(true);
            if let Some(f) = b.first_mut() {
                *f = true;
            }
            b
        })
        .collect();
    // Merge adjacent decorations of the same colour on the same line.
    out.decos = merge_decos(std::mem::take(&mut out.decos));
    out
}

/// Where the first line's text starts: past the bullet when there is one.
fn first_line_start(has_bullet: bool, margin: f64, indent: f64, bullet_w: f64) -> f64 {
    if !has_bullet {
        return (margin + indent).max(0.0);
    }
    let bx = (margin + indent).max(0.0);
    // A hanging bullet's text starts at the margin. When the hang reaches left of the box
    // (marL + indent < 0), PowerPoint pins the bullet to the box's edge and keeps the
    // hang: the first line's text starts at -indent (later lines stay at the margin).
    let stop = margin.max(-indent);
    if indent < 0.0 && bx + bullet_w <= stop { stop } else { bx + bullet_w + if indent >= 0.0 { 0.0 } else { 3.0 } }
}

/// Breaks one paragraph into lines and stacks them onto `flow`. `spacing` is
/// (line spacing reduction, whether the first paragraph's space before counts).
fn place_paragraph(p: &ParaShaped, pi: usize, g: &BoxGeom, spacing: (f64, bool), flow: &mut Flow, out: &mut TextLayout) {
    let (reduce, first_last) = spacing;
    let props = &p.props;
    let margin = props.margin_left.unwrap_or(0.0).max(0.0);
    let indent = props.indent.unwrap_or(0.0);
    let mr = props.margin_right.unwrap_or(0.0).max(0.0);
    let bullet_w = p.bullet.as_ref().map(|(s, st)| measure_str(&st.face, st.size, s).0).unwrap_or(0.0);
    let first_x = first_line_start(p.bullet.is_some(), margin, indent, bullet_w);
    let rest_x = margin;
    let tabs = tabs_of(props);
    let first_w = (g.col_w - first_x - mr).max(1.0);
    let rest_w = (g.col_w - rest_x - mr).max(1.0);
    let raw = break_lines(
        p,
        &BreakParams {
            first_w,
            rest_w,
            wrap: g.wrap,
            tab_stops: &tabs,
            default_tab: props.default_tab.unwrap_or(72.0),
            x0_first: first_x,
            x0_rest: rest_x,
        },
    );
    // The first paragraph's space before counts only when the body says so.
    let before = if pi == 0 && !first_last { 0.0 } else { spacing_pts(props.space_before, p.lead_size) };
    flow.y += before.max(0.0) + flow.prev_after;
    let first_line_idx = flow.lines.len();
    let env = ParaEnv { p, g, reduce, margin, indent, mr, bullet_w, first_x, rest_x, tabs: &tabs };
    for (li, rl) in raw.iter().enumerate() {
        let is_last = li + 1 == raw.len();
        let placed = place_line(&env, pi, rl, li, is_last, flow);
        out.content_width = out.content_width.max(placed.content_w);
        flow.lines.push(placed);
    }
    out.para_lines.push((first_line_idx, raw.len()));
    flow.prev_after = spacing_pts(props.space_after, p.lead_size).max(0.0);
}

/// Max ascent / descent / size over the line's styles (or the lead style if empty).
fn line_extent(p: &ParaShaped, rl: &RawLine) -> (f64, f64, f64) {
    let (mut asc, mut desc, mut size) = (0.0f64, 0.0f64, 0.0f64);
    for c in p.cells.get(rl.start..rl.end.max(rl.start)).unwrap_or(&[]) {
        if let (Some(st), Some(face)) = (p.styles.get(c.style), p.faces.get(c.face)) {
            let sz = st.size;
            // The run's own fonts set the line; a fallback face (emoji, a symbol or a
            // script the run's fonts lack) does not move the baseline in PowerPoint.
            let own = Arc::ptr_eq(face, &st.face)
                || [st.ea.as_deref(), st.cs.as_deref(), st.sym.as_deref()].into_iter().flatten().any(|f| face.family.eq_ignore_ascii_case(f));
            let (a, d) = line_metrics(if own { face } else { &st.face }, sz);
            asc = asc.max(a);
            desc = desc.max(d);
            size = size.max(sz);
        }
    }
    if size == 0.0 {
        // PowerPoint sizes an empty line by the mark that ends it: the `a:rPr` of the
        // `a:br` that closes it, or `a:endParaRPr` for the paragraph's last line -- not
        // by the paragraph's first run (#780). An empty *paragraph* already lands on
        // `end_props` through `lead`, and keeps the same height here.
        let brk = p.cells.get(rl.end).filter(|c| c.ch == '\u{b}').and_then(|c| p.styles.get(c.style));
        let (face, sz) = match brk {
            Some(st) => (&st.face, st.size),
            None if rl.end >= p.cells.len() => (&p.end_face, p.end_size),
            // Anything else empty (a line of tabs alone) keeps the old measure.
            None => (&p.lead_face, p.lead_size),
        };
        let (a, d) = line_metrics(face, sz);
        return (a, d, sz);
    }
    (asc, desc, size)
}

/// Logical advance of each cell on the line (tabs measure from the start edge) and their sum.
fn line_advances(env: &ParaEnv, rl: &RawLine, is_last: bool, x0: f64) -> (Vec<f64>, f64) {
    let p = env.p;
    let line_cells = p.cells.get(rl.start..rl.end).unwrap_or(&[]);
    let n = rl.end.saturating_sub(rl.start);
    let mut adv = Vec::with_capacity(n);
    let mut x = 0.0;
    // A line that breaks at a soft hyphen ends with a visible hyphen.
    let shy_end = !is_last && line_cells.last().is_some_and(|c| c.ch == '\u{AD}');
    for (k, c) in line_cells.iter().enumerate() {
        let a = if shy_end && k + 1 == n {
            p.faces.get(c.face).zip(p.styles.get(c.style)).map(|(f, st)| measure_str(f, st.size, "-").0).unwrap_or(0.0)
        } else if c.ch == '\t' {
            tab_advance(x0 + x, env.tabs, p.props.default_tab.unwrap_or(72.0), tab_segment(&p.cells, rl.start + k))
        } else {
            c.adv
        };
        adv.push(a);
        x += a;
    }
    (adv, x)
}

/// Justification: kashida (justLow, Arabic), spaces, or every character. Widens `adv` and returns
/// the kashida widths and whether the line was filled to the box edges.
fn justify_line(p: &ParaShaped, rl: &RawLine, adv: &mut [f64], align: Align, slack: f64, stretch: bool) -> (Vec<f64>, bool) {
    let n = adv.len();
    let line_cells = p.cells.get(rl.start..rl.end).unwrap_or(&[]);
    let mut kashida = vec![0.0; n];
    let mut filled = false;
    if stretch && align == Align::JustLow {
        let ops = kashida_opportunities(p, rl.start, rl.end);
        if !ops.is_empty() {
            let each = slack / ops.len() as f64;
            for k in ops {
                if let (Some(e), Some(a)) = (kashida.get_mut(k), adv.get_mut(k)) {
                    *e = each;
                    *a += each;
                }
            }
            filled = true;
        }
    }
    if stretch && !filled && matches!(align, Align::Justify | Align::JustLow | Align::ThaiDistributed) {
        let spaces = line_cells.iter().filter(|c| c.ch == ' ').count();
        if spaces > 0 {
            for (a, c) in adv.iter_mut().zip(line_cells) {
                if c.ch == ' ' {
                    *a += slack / spaces as f64;
                }
            }
            filled = true;
        }
    }
    if align == Align::Distributed && n > 1 {
        for a in adv.iter_mut().take(n - 1) {
            *a += slack / (n - 1) as f64;
        }
        filled = true;
    }
    (kashida, filled)
}

/// Left edge of the line's text inside its box `(box_l, box_r)`.
fn line_left(align: Align, filled: bool, rtl: bool, bx: (f64, f64), line_w: f64, slack: f64, n: usize) -> f64 {
    let (box_l, box_r) = bx;
    if filled {
        return box_l;
    }
    match align {
        Align::Left => box_l,
        Align::Right => box_r - line_w,
        Align::Center => box_l + (box_r - box_l - line_w) / 2.0,
        Align::Distributed if n <= 1 => box_l + slack / 2.0,
        // Justified lines that aren't stretched (the last one) sit at the start edge.
        Align::Justify | Align::JustLow | Align::Distributed | Align::ThaiDistributed if rtl => box_r - line_w,
        Align::Justify | Align::JustLow | Align::Distributed | Align::ThaiDistributed => box_l,
    }
}

/// Visual boxes of each cell and the caret x for every boundary, from the visual order and widths.
fn edges_and_carets(order: &[usize], levels: &[u8], adv: &[f64], left: f64, rtl: bool, line_w: f64) -> (Vec<(f64, f64)>, Vec<f64>) {
    let n = adv.len();
    let mut edges = vec![(left, left); n];
    let mut vx = left;
    for &k in order {
        let a = adv.get(k).copied().unwrap_or(0.0);
        if let Some(e) = edges.get_mut(k) {
            *e = (vx, vx + a);
        }
        vx += a;
    }
    let odd = |k: usize| levels.get(k).is_some_and(|l| l % 2 == 1);
    let mut caret_x: Vec<f64> = (0..n).map(|k| edges.get(k).map(|e| if odd(k) { e.1 } else { e.0 }).unwrap_or(left)).collect();
    caret_x.push(match n.checked_sub(1) {
        Some(k) => edges.get(k).map(|e| if odd(k) { e.0 } else { e.1 }).unwrap_or(left),
        None if rtl => left + line_w,
        None => left,
    });
    (edges, caret_x)
}

/// Bullet text, x and baseline for a paragraph's first line.
fn line_bullet(env: &ParaEnv, align: Align, content_shift: f64, baseline: f64) -> Option<(String, f64, f64)> {
    let p = env.p;
    p.bullet.as_ref().map(|(s, _)| {
        // Centred and end-aligned text carries its bullet along.
        let end_aligned = if p.rtl { align == Align::Left } else { align == Align::Right };
        let shifted = align == Align::Center || end_aligned;
        let b = if shifted && p.cells.is_empty() {
            content_shift + env.first_x - env.bullet_w
        } else {
            (env.margin + env.indent).max(0.0) + if shifted { content_shift } else { 0.0 }
        };
        // x_bullet = S - (marL + indent) for RTL: the bullet's start edge is its right.
        let bx = if p.rtl { env.g.col_w - b - env.bullet_w } else { b };
        (s.clone(), bx, baseline)
    })
}

/// Lays out line `li` (`rl`) of a paragraph at the flow's current height and advances the flow.
fn place_line(env: &ParaEnv, pi: usize, rl: &RawLine, li: usize, is_last: bool, flow: &mut Flow) -> PlacedLine {
    let (p, g) = (env.p, env.g);
    let props = &p.props;
    let (asc, desc, size) = line_extent(p, rl);
    let natural = (asc + desc).max(size * 1.0);
    let lh = match props.line_spacing {
        // The reduction comes off the percentage (90 % less 20 % is 70 %), as PowerPoint
        // applies `lnSpcReduction`.
        Some(Spacing::Pct(f)) => natural * (f - env.reduce).max(f * 0.1).max(0.0),
        Some(Spacing::Pts(v)) => v * (1.0 - env.reduce),
        None => natural * (1.0 - env.reduce),
    };
    let ratio = if natural > 0.0 { lh / natural } else { 1.0 };
    let top = flow.y;
    // Exact spacing (`spcPts`), as PowerPoint sets it: the baseline sits at 80 % of the
    // line pitch, and the text ends at the last line's descent rather than a full pitch.
    let exact = matches!(props.line_spacing, Some(Spacing::Pts(_)));
    let baseline = if exact { top + lh * 0.8 } else { top + asc * ratio };
    flow.tail = if exact { (lh * 0.2 - desc).max(-lh) } else { 0.0 };
    flow.y += lh;
    let x0 = env.start_x(li);
    let avail = (g.col_w - x0 - env.mr).max(0.0);
    let n = rl.end.saturating_sub(rl.start);
    let line_cells = p.cells.get(rl.start..rl.end).unwrap_or(&[]);
    let (mut adv, width) = line_advances(env, rl, is_last, x0);
    let align = props.align.unwrap_or(Align::Left);
    let slack = (avail - width).max(0.0);
    let stretch = !is_last && g.wrap && slack > 0.0;
    let (kashida, filled) = justify_line(p, rl, &mut adv, align, slack, stretch);
    let line_w: f64 = adv.iter().sum();
    // The line box, in column coordinates: the start edge is the left one for LTR and the
    // right one (S = W - rIns) for RTL paragraphs.
    let (box_l, box_r) = if p.rtl { (env.mr, g.col_w - x0) } else { (x0, g.col_w - env.mr) };
    let left = line_left(align, filled, p.rtl, (box_l, box_r), line_w, slack, n);
    // Distance from the box's start edge to the text (bullets move with centred or
    // end-aligned text).
    let content_shift = if p.rtl { box_r - (left + line_w) } else { left - box_l };
    // Rules L1 + L2: this line's visual order.
    let chars: Vec<char> = line_cells.iter().map(|c| c.ch).collect();
    let raw_levels: Vec<u8> = line_cells.iter().map(|c| c.level).collect();
    let levels = bidi::line_levels(&chars, &raw_levels, u8::from(p.rtl));
    let order = bidi::visual_order(&levels);
    let (edges, caret_x) = edges_and_carets(&order, &levels, &adv, left, p.rtl, line_w);
    let bullet = if li == 0 { line_bullet(env, align, content_shift, baseline) } else { None };
    let boundary: Vec<bool> = line_cells.iter().map(|c| c.boundary).collect();
    PlacedLine {
        info: LineInfo {
            para: pi,
            start: rl.start,
            end: rl.end,
            top,
            baseline,
            bottom: flow.y,
            caret_x,
            column: 0,
            rtl: p.rtl,
            levels,
            edges,
            boundary,
            extent: (left, left + line_w),
            start_offset: x0,
        },
        place: LinePlace { order, kashida },
        bullet,
        content_w: width + x0 + env.mr,
    }
}

/// Columns: distribute lines by height.
fn assign_columns(lines: &mut [PlacedLine], g: &BoxGeom) {
    let col_h = if g.ncols > 1 { g.inner.height() } else { f64::INFINITY };
    let mut col = 0usize;
    let mut col_y0 = 0.0;
    for pl in lines.iter_mut() {
        let li = &mut pl.info;
        if g.ncols > 1 && li.bottom - col_y0 > col_h + 0.01 && col + 1 < g.ncols {
            col += 1;
            col_y0 = li.top;
        }
        li.column = col;
        if col > 0 {
            let dy = col_y0;
            li.top -= dy;
            li.baseline -= dy;
            li.bottom -= dy;
        }
        let cx = g.col_x(col);
        if cx != 0.0 {
            li.shift_x(cx);
        }
    }
}

/// Origin (x, y) of the text block inside the box, from the anchor and `anchorCtr`.
fn text_origin(lines: &[PlacedLine], g: &BoxGeom, bp: &BodyProps, used_h: f64) -> (f64, f64) {
    let inner = g.inner;
    let dy = match bp.anchor.unwrap_or(Anchor::Top) {
        Anchor::Top => 0.0,
        Anchor::Middle => (inner.height() - used_h) / 2.0,
        // PowerPoint draws justified and distributed anchoring at the bottom.
        Anchor::Bottom | Anchor::Justified | Anchor::Distributed => inner.height() - used_h,
    };
    // anchorCtr: centre the drawn text block on the box. Lines sit where their alignment put
    // them, so the origin is set so the block's centre lands on the box's centre — a left-edge
    // offset alone pushed right-aligned lines out of the box (#736). Empty lines carry no ink
    // and stay out of the block. Overflowing content centres symmetrically past both edges.
    // Not applied to vertical text (there the offset would run along the wrong axis) or
    // multi-column bodies (centring unimplemented for them).
    let centre_block = bp.anchor_ctr.unwrap_or(false) && g.ncols == 1 && matches!(g.vert, TextDir::Horizontal);
    let ox = if centre_block && !lines.is_empty() {
        let ink = lines.iter().map(|l| &l.info).filter(|li| li.end > li.start);
        let (bmin, bmax) = ink.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), li| (a.min(li.extent.0), b.max(li.extent.1)));
        if bmax >= bmin { inner.x0 + inner.width() / 2.0 - (bmin + bmax) / 2.0 } else { inner.x0 }
    } else {
        inner.x0
    };
    (ox, inner.y0 + dy)
}

/// Moves a line from column coordinates into shape-local ones.
fn position_line(line: &mut LineInfo, ox: f64, oy: f64, g: &BoxGeom) {
    line.top += oy;
    line.baseline += oy;
    line.bottom += oy;
    line.shift_x(ox);
    if g.lines_ltr() {
        // The line's place mirrored across the box: the first line ends up on the left.
        let d = g.inner.y0 + g.inner.y1 - line.top - line.bottom;
        line.top += d;
        line.baseline += d;
        line.bottom += d;
    }
}

/// Emits the bullet glyph run of a line's paragraph, if the line carries one. `shift_x` is the
/// text block's x origin plus the line's column offset.
fn emit_bullet(out: &mut TextLayout, p: &ParaShaped, pl: &PlacedLine, shift_x: f64, to_empty: bool) {
    let line = &pl.info;
    let Some((s, bx, _)) = pl.bullet.as_ref() else { return };
    let Some((_, st)) = p.bullet.as_ref() else { return };
    let (_, g) = measure_str(&st.face, st.size, s);
    let run = GlyphRun {
        face: st.face.clone(),
        size: st.size,
        color: st.color,
        glyphs: g.into_iter().map(|(gid, x)| (gid, shift_x + bx + x, line.baseline)).collect(),
        cells: vec![],
        fake_bold: false,
        fake_italic: false,
        outline: None,
        para: line.para,
        chars: (0, 0),
        link: false,
        alpha: st.alpha,
        gradient: None,
        effects: None,
        upright: false,
    };
    if to_empty {
        out.empty_bullets.push(run);
    } else {
        out.runs.push(run);
    }
}

/// Position of one cell on its line: index in the line, in the paragraph, its box, and the glyph
/// baseline and size.
struct CellPos {
    k: usize,
    ci: usize,
    cx: f64,
    cx1: f64,
    base_y: f64,
    size: f64,
}

/// Starts a glyph run for `cell` drawn in style `st`.
fn new_run(cell: &Cell, st: &Style, face: &Arc<FontFace>, para: usize, pos: &CellPos) -> GlyphRun {
    GlyphRun {
        face: face.clone(),
        size: pos.size,
        color: st.color,
        glyphs: vec![],
        cells: vec![],
        fake_bold: st.fake_bold,
        fake_italic: st.fake_italic,
        outline: st.outline,
        para,
        chars: (pos.ci, pos.ci + 1),
        link: st.link,
        alpha: st.alpha,
        gradient: st.gradient.clone(),
        effects: st.effects,
        upright: cell.upright,
    }
}

/// Emits one line's glyph runs and decorations, in visual order.
fn emit_line_runs(out: &mut TextLayout, p: &ParaShaped, line: &LineInfo, place: &LinePlace) {
    // Group visually consecutive cells by (style, face).
    let mut cur: Option<GlyphRun> = None;
    let mut cur_key = (usize::MAX, usize::MAX, false, false);
    for &k in &place.order {
        let ci = line.start + k;
        let Some(cell) = p.cells.get(ci) else { continue };
        let Some(st) = p.styles.get(cell.style) else { continue };
        let Some(&(cx, cx1)) = line.edges.get(k) else { continue };
        let base_y = line.baseline - st.baseline * st.size;
        let size = if st.baseline != 0.0 { st.size * SUB_SUPER } else { st.size };
        let small = st.caps == Caps::Small && cell.ch.is_lowercase();
        let size = if small { size * 0.8 } else { size };
        let pos = CellPos { k, ci, cx, cx1, base_y, size };
        // Small capitals are drawn smaller than the capitals beside them: a run of their own.
        let key = (cell.style, cell.face, small, cell.upright);
        if key != cur_key {
            if let Some(r) = cur.take() {
                out.runs.push(r);
            }
            let Some(face) = p.faces.get(cell.face) else { continue };
            cur = Some(new_run(cell, st, face, line.para, &pos));
            cur_key = key;
        }
        if let Some(r) = cur.as_mut() {
            push_cell_glyphs(r, p, line, place, cell, &pos);
        }
        if cx1 - cx > 0.0 {
            push_cell_decos(out, cell, st, line, &pos);
        }
    }
    if let Some(r) = cur.take() {
        out.runs.push(r);
    }
}

/// Appends the glyphs a cell draws to the run `r`.
fn push_cell_glyphs(r: &mut GlyphRun, p: &ParaShaped, line: &LineInfo, place: &LinePlace, cell: &Cell, pos: &CellPos) {
    let CellPos { k, ci, cx, cx1, base_y, .. } = *pos;
    r.chars = (r.chars.0.min(ci), r.chars.1.max(ci + 1));
    if cell.ch == '\u{AD}' && cx1 - cx > 0.01 {
        // The soft hyphen a line breaks at is drawn as a hyphen.
        if let Some(f) = p.faces.get(cell.face) {
            r.glyphs.push((f.glyph_for('-'), cx, base_y));
            r.cells.push((ci, cell.ch));
        }
    } else if cell.head == ci && !cell.glyphs.is_empty() {
        // The cluster's glyphs start at its leftmost cell; kashida (right-to-left
        // only) fills the gap on the cluster's left with tatweel glyphs.
        let cells_of = (k..line.end.saturating_sub(line.start)).take_while(|&j| p.cells.get(line.start + j).is_some_and(|c| c.head == ci));
        let (mut cl, mut kash) = (cx, 0.0);
        for j in cells_of {
            cl = cl.min(line.edges.get(j).map(|e| e.0).unwrap_or(cx));
            kash += place.kashida.get(j).copied().unwrap_or(0.0);
        }
        if kash > 0.0 {
            push_tatweel(r, p.faces.get(cell.face), cell, (cl, kash), pos);
        }
        for (gid, gx, gy) in &cell.glyphs {
            r.glyphs.push((*gid, cl + kash + gx, base_y + gy));
            r.cells.push((ci, cell.ch));
        }
    }
}

/// Fills `kash` points left of a cluster at `cl` with tatweel glyphs.
fn push_tatweel(r: &mut GlyphRun, face: Option<&Arc<FontFace>>, cell: &Cell, fill: (f64, f64), pos: &CellPos) {
    let (cl, kash) = fill;
    let tg = face.map(|f| f.glyph_for('\u{0640}')).unwrap_or(0);
    let tw = face.map(|f| f.advance(tg) * pos.size / f.upem.max(1.0)).unwrap_or(0.0);
    if tg != 0 && tw > 0.0 {
        let count = ((kash / tw).ceil() as usize).clamp(1, 256);
        for i in 0..count {
            r.glyphs.push((tg, cl + (i as f64 * tw).min((kash - tw).max(0.0)), pos.base_y));
            r.cells.push((pos.ci, cell.ch));
        }
    }
}

/// Highlight, underline and strikethrough boxes of a cell.
fn push_cell_decos(out: &mut TextLayout, cell: &Cell, st: &Style, line: &LineInfo, pos: &CellPos) {
    let (cx, cx1) = (pos.cx, pos.cx1);
    if let Some(hl) = st.highlight {
        out.decos.push(Deco { rect: Rect::new(cx, line.top, cx1, line.bottom), color: hl, behind: true, para: line.para, line: DecoLine::Solid });
    }
    if cell.ch.is_whitespace() && st.underline.is_none() {
        return;
    }
    if let Some((uc, u)) = st.underline {
        // At the underline position and thickness of the run's own font.
        let k = st.size / st.face.upem.max(1.0);
        let (p, t) = (st.face.underline.0 * k, (st.face.underline.1 * k).max(0.5));
        for (y0, y1, kind) in u.boxes(p, t) {
            out.decos.push(Deco {
                rect: Rect::new(cx, line.baseline + y0, cx1, line.baseline + y1),
                color: uc,
                behind: false,
                para: line.para,
                line: kind,
            });
        }
    }
    if let Some(sk) = st.strike {
        // At the strikeout position and thickness of the run's own font.
        let k = st.size / st.face.upem.max(1.0);
        let t = (st.face.strikeout.1 * k).max(0.5);
        let sy = line.baseline - st.face.strikeout.0 * k;
        out.decos.push(Deco { rect: Rect::new(cx, sy, cx1, sy + t), color: st.color, behind: false, para: line.para, line: DecoLine::Solid });
        if sk == Strike::Double {
            out.decos.push(Deco {
                rect: Rect::new(cx, sy - t * 2.0, cx1, sy - t),
                color: st.color,
                behind: false,
                para: line.para,
                line: DecoLine::Solid,
            });
        }
    }
}

fn merge_decos(v: Vec<Deco>) -> Vec<Deco> {
    let mut out: Vec<Deco> = Vec::with_capacity(v.len());
    for d in v {
        // Lines drawn together (a double underline, a strikethrough) interleave: look a few back.
        if let Some(last) = out.iter_mut().rev().take(4).find(|last| {
            last.color == d.color
                && last.behind == d.behind
                && last.line == d.line
                && (last.rect.y0 - d.rect.y0).abs() < 0.01
                && (last.rect.y1 - d.rect.y1).abs() < 0.01
                && (last.rect.x1 - d.rect.x0).abs() < 0.5
        }) {
            last.rect.x1 = d.rect.x1;
            continue;
        }
        out.push(d);
    }
    out
}

/// How a line's cells are placed: visual order (indices from the line start) and kashida widths
/// (the boxes, with justification, are in [`LineInfo::edges`]).
struct LinePlace {
    order: Vec<usize>,
    kashida: Vec<f64>,
}

/// Kashida opportunities on a line (indices from the line start), at most one per word (its last
/// join, where elongation reads most naturally): the last cell of a right-to-left Arabic grapheme
/// that joins the next letter, in a face with a tatweel glyph, when the two aren't one ligature.
fn kashida_opportunities(p: &ParaShaped, start: usize, end: usize) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut word_has = false;
    for i in start..end {
        let Some(c) = p.cells.get(i) else { continue };
        if c.ch.is_whitespace() {
            word_has = false;
            continue;
        }
        if !c.boundary || c.level % 2 == 0 || !joins_following(c.ch) {
            continue;
        }
        // The next grapheme on the line.
        let Some(nx) = (i + 1..end).find(|&j| p.cells.get(j).is_some_and(|d| d.boundary)) else { continue };
        let Some(d) = p.cells.get(nx) else { continue };
        let same_cluster = d.head == c.head || d.head != nx;
        let tatweel = p.faces.get(c.face).is_some_and(|f| f.glyph_for('\u{0640}') != 0);
        if joins_preceding(d.ch) && d.face == c.face && !same_cluster && tatweel {
            // A later join in the same word replaces the earlier one.
            if word_has {
                out.pop();
            }
            out.push(nx - 1 - start);
            word_has = true;
        }
    }
    out
}
