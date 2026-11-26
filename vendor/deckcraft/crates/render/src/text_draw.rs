use super::geometry::{blur, color, pre_translate, scale_of};
use super::opts::ParaState;
use super::{
    Affine, Arc, BezPath, Ctx, FontDb, HashMap, Opts, PhType, Rect, RenderContext, Rgba, Shape, TextBody, Vec2, colr, paint, peniko, resolve,
};

/// The prompt shown in an empty placeholder, formatted like its content would be.
pub(crate) fn prompt_body(rctx: &Ctx, s: &Shape, kind: PhType) -> TextBody {
    // Layout/master prompt text when the layout supplies a custom one.
    let (lp, _) = resolve::parents(rctx, s);
    let custom = lp.and_then(|l| l.ph.as_ref().filter(|p| p.has_custom_prompt).and(l.text.as_ref())).filter(|t| !t.is_empty());
    if let Some(t) = custom {
        return t.clone();
    }
    if matches!(rctx.owner, resolve::Owner::Layout | resolve::Owner::Master) {
        return TextBody::default();
    }
    let txt = kind.prompt();
    if txt.is_empty() || kind.is_footer_kind() {
        return TextBody::default();
    }
    let mut t = TextBody::from_text(txt);
    if matches!(kind, PhType::Picture | PhType::Chart | PhType::Table | PhType::Media | PhType::Diagram | PhType::ClipArt) {
        t.body.anchor = Some(deckcraft_model::text::Anchor::Bottom);
        if let Some(p) = t.paragraphs.first_mut() {
            p.props.align = Some(deckcraft_model::text::Align::Center);
            p.props.bullet = Some(deckcraft_model::text::Bullet::None);
            p.props.margin_left = Some(0.0);
            p.props.indent = Some(0.0);
            for r in &mut p.runs {
                r.props.size = Some(14.0);
            }
        }
    }
    t
}

/// Most path elements this thread's glyph cache retains. Its entries are shared with the font
/// cache, but they stay alive here after that cache evicts them, so they are bounded by elements.
const GLYPH_CACHE_MAX_ELEMENTS: usize = 1 << 22;

thread_local! {
    /// The cached outlines, and the path elements they hold.
    static GLYPH_CACHE: std::cell::RefCell<(HashMap<(u32, u32), Arc<BezPath>>, usize)> = std::cell::RefCell::new((HashMap::new(), 0));
}

pub(crate) fn glyph(face: &deckcraft_fonts::FontFace, gid: u32) -> Arc<BezPath> {
    let key = (face.id(), gid);
    if let Some(p) = GLYPH_CACHE.with(|c| c.borrow().0.get(&key).cloned()) {
        return p;
    }
    let p = FontDb::global().outline(face, gid);
    GLYPH_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let n = p.elements().len();
        if c.0.len() > 200_000 || c.1.saturating_add(n) > GLYPH_CACHE_MAX_ELEMENTS {
            c.0.clear();
            c.1 = 0;
        }
        c.1 += n;
        c.0.insert(key, p.clone());
    });
    p
}

/// Draw a text body into shape space `m` within the text rect `opts.rect`.
pub fn draw_text(ctx: &mut RenderContext, rctx: &Ctx, s: &Shape, body: &TextBody, m: Affine, opts: &Opts) {
    let l = deckcraft_text::layout(rctx, s, body, opts);
    draw_layout(ctx, &l, opts.rect, m);
}

/// Add transient list markers to an editor layout, using the document's own bullet glyphs.
pub(crate) fn preview_empty_bullets(l: &mut deckcraft_text::TextLayout) {
    for mut run in l.empty_bullets.drain(..) {
        run.color = Rgba::rgb(0xB3, 0xB3, 0xB3);
        run.alpha = 1.0;
        l.runs.push(run);
    }
}

/// Where text laid out in text rect `tr` is drawn, for a shape drawn with `m` (shape-local →
/// output): `m`, without mirroring, then the layout's `rotation` about the rect's centre.
///
/// Flips mirror a shape's geometry, never its text. When `m` reflects (flipH or flipV, the shape's
/// own or a group's) the text rect keeps its mirrored place and the text is mirrored back about
/// the rect's centre: a flipH shape's text stays readable and a flipV shape's text is turned 180°
/// (both flips: 180°), as in PowerPoint. Rendering, hit testing and the caret all use this.
pub fn text_transform(m: Affine, tr: Rect, rotation: f64) -> Affine {
    let c = tr.center().to_vec2();
    let mut t = m;
    if m.determinant() < 0.0 && c.is_finite() {
        t = t * Affine::translate(c) * Affine::scale_non_uniform(-1.0, 1.0) * Affine::translate(-c);
    }
    if rotation != 0.0 && rotation.is_finite() && c.is_finite() {
        t = t * Affine::translate(c) * Affine::rotate(rotation.to_radians()) * Affine::translate(-c);
    }
    t
}

/// The WordArt warp of `s`'s text over its text rectangle `tr`, if it has one DeckCraft draws.
pub fn text_warp(rctx: &Ctx, s: &Shape, tr: Rect) -> Option<deckcraft_geom::warp::Warp> {
    let bp = resolve::body(rctx, s);
    let name = bp.warp.as_deref()?;
    deckcraft_geom::warp::warp(name, &bp.warp_adj, tr)
}

/// Draw an already laid-out text block.
pub fn draw_layout(ctx: &mut RenderContext, l: &deckcraft_text::TextLayout, tr: Rect, m: Affine) {
    draw_layout_with(ctx, l, tr, m, &[]);
}

/// Draw a laid-out text block with per-paragraph animation states.
pub fn draw_layout_with(ctx: &mut RenderContext, l: &deckcraft_text::TextLayout, tr: Rect, m: Affine, paras: &[(usize, ParaState)]) {
    let para = |i: usize| paras.iter().find(|(p, _)| *p == i).map(|(_, s)| *s).unwrap_or_default();
    let m = text_transform(m, tr, l.rotation);
    let lines_of = LineIndex::new(l);
    let deco = |ctx: &mut RenderContext, d: &deckcraft_text::Deco| {
        let ps = para(d.para);
        if !ps.visible || ps.opacity <= 0.001 {
            return;
        }
        ctx.set_transform(m * Affine::translate(ps.offset));
        ctx.set_paint(color(d.color, ps.opacity));
        fill_deco(ctx, d);
    };
    for d in l.decos.iter().filter(|d| d.behind) {
        deco(ctx, d);
    }
    // Text shadows, then glows, under all the text.
    for pass in [TextEffect::Shadow, TextEffect::Glow] {
        for run in &l.runs {
            let ps = para(run.para);
            if ps.visible && ps.opacity > 0.001 {
                text_effect(ctx, run, pass, m * Affine::translate(ps.offset), ps.opacity);
            }
        }
    }
    for run in &l.runs {
        if (run.alpha <= 0.0 || run.color.a == 0) && run.outline.is_none() {
            continue;
        }
        let ps = para(run.para);
        if !ps.visible || ps.opacity <= 0.001 {
            continue;
        }
        let m = m * Affine::translate(ps.offset);
        // A gradient fill starts over on every line and spans that line's text (from its top to
        // its bottom, from its first to its last character), as in PowerPoint.
        let mut grads: Vec<(usize, peniko::Gradient)> = Vec::new();
        for (gid, x, y) in &run.glyphs {
            let at = m * glyph_place(run, *gid, *x, *y);
            let Some(g) = run.gradient.as_ref().filter(|g| !g.stops.is_empty()) else {
                draw_glyph(ctx, run, *gid, at, ps.opacity);
                continue;
            };
            let li = lines_of.line_at(run.para, *y);
            if !grads.iter().any(|(i, _)| *i == li) {
                let scheme = deckcraft_color::ColorScheme::new(String::new(), [Rgba::BLACK; 12]);
                let stops = g.stops.iter().map(|s| (s.pos, s.color.resolve(&scheme, None))).collect();
                let bounds = l.lines.get(li).and_then(line_bounds).unwrap_or(tr);
                grads.push((li, paint::gradient(&g.shape, stops, bounds, run.alpha * ps.opacity)));
            }
            let fill = grads.iter().find(|(i, _)| *i == li).map(|(_, g)| (g, Affine::translate((-*x, -*y))));
            draw_glyph_with(ctx, run, *gid, at, ps.opacity, fill);
        }
    }
    for d in l.decos.iter().filter(|d| !d.behind) {
        deco(ctx, d);
    }
}

/// Where glyph `gid` of `run` with its pen at (`x`, `y`) is drawn, in text space: there, turned a
/// quarter turn back about its em box's centre when it stands upright in vertical text.
pub(crate) fn glyph_place(run: &deckcraft_text::GlyphRun, gid: u32, x: f64, y: f64) -> Affine {
    let at = Affine::translate((x, y));
    match run.upright_centre(gid, x, y) {
        Some((cx, cy)) => Affine::translate((cx, cy)) * Affine::rotate(-std::f64::consts::FRAC_PI_2) * Affine::translate((-cx, -cy)) * at,
        None => at,
    }
}

/// Fill an underline, strikethrough or highlight with the current paint and transform: its box,
/// its dashes, or its zigzag stroked.
pub(crate) fn fill_deco(ctx: &mut RenderContext, d: &deckcraft_text::Deco) {
    if let Some((pts, w)) = d.wave() {
        let mut path = BezPath::new();
        for (i, p) in pts.iter().enumerate() {
            if i == 0 {
                path.move_to(*p);
            } else {
                path.line_to(*p);
            }
        }
        ctx.set_stroke(kurbo::Stroke::new(w.max(0.01)));
        ctx.stroke_path(&path);
        return;
    }
    for r in d.pieces() {
        ctx.fill_rect(&r);
    }
}

/// Finds the line of a paragraph holding a glyph on baseline `y` without scanning the paragraph's
/// lines for every glyph. A paragraph whose lines are ordered top to bottom (every layout the text
/// crate produces) is searched by bisection; any other falls back to a scan with the same answer.
pub(crate) struct LineIndex<'a> {
    l: &'a deckcraft_text::TextLayout,
    /// Per paragraph: its lines' tops, bottoms and baselines all rise with the line index.
    ordered: Vec<bool>,
}

impl<'a> LineIndex<'a> {
    pub(crate) fn new(l: &'a deckcraft_text::TextLayout) -> Self {
        let ordered = l
            .para_lines
            .iter()
            .map(|&(first, count)| {
                let end = first.saturating_add(count).min(l.lines.len());
                l.lines.get(first..end).is_some_and(|ls| {
                    ls.iter().all(|a| a.top <= a.bottom)
                        && ls.windows(2).all(|w| w[0].top <= w[1].top && w[0].bottom <= w[1].bottom && w[0].baseline <= w[1].baseline)
                })
            })
            .collect();
        LineIndex { l, ordered }
    }

    /// The nearest line baseline to `y` among `lines` (the first of equals), as an index into `lines`.
    fn nearest(lines: &[deckcraft_text::LineInfo], y: f64) -> Option<usize> {
        let i = lines.partition_point(|li| li.baseline < y);
        let best = match (i.checked_sub(1), lines.get(i)) {
            (Some(p), Some(n)) => {
                if (lines[p].baseline - y).abs() <= (n.baseline - y).abs() {
                    p
                } else {
                    i
                }
            }
            (Some(p), None) => p,
            (None, Some(_)) => i,
            (None, None) => return None,
        };
        let b = lines[best].baseline;
        Some(lines.partition_point(|li| li.baseline < b))
    }

    /// The paragraph's lines when they are ordered, with the index of the first.
    fn ordered_lines(&self, para: usize) -> Option<(usize, &[deckcraft_text::LineInfo])> {
        let &(first, count) = self.l.para_lines.get(para)?;
        let end = first.saturating_add(count).min(self.l.lines.len());
        (self.ordered[para] && first < end).then(|| (first, &self.l.lines[first..end]))
    }

    /// The line of paragraph `para` holding a glyph on baseline `y` (the nearest baseline).
    pub(crate) fn line_at(&self, para: usize, y: f64) -> usize {
        if let Some((first, ls)) = self.ordered_lines(para) {
            return Self::nearest(ls, y).map_or(0, |i| first + i);
        }
        line_at_scan(self.l, para, y)
    }

    /// The first line of paragraph `para` whose box holds `y`, else the nearest baseline's line.
    pub(crate) fn line_of(&self, para: usize, y: f64) -> Option<usize> {
        let Some((first, ls)) = self.ordered_lines(para) else { return crate::warp::line_of_scan(self.l, para, y) };
        let i = ls.partition_point(|li| li.bottom + 0.01 < y);
        if ls.get(i).is_some_and(|li| y >= li.top - 0.01) {
            return Some(first + i);
        }
        Self::nearest(ls, y).map(|i| first + i)
    }
}

/// The line of paragraph `para` holding a glyph on baseline `y` (the nearest baseline), by scanning.
fn line_at_scan(l: &deckcraft_text::TextLayout, para: usize, y: f64) -> usize {
    if let Some(&(first, count)) = l.para_lines.get(para) {
        if count == 0 {
            return 0;
        }
        let end = (first + count).min(l.lines.len());
        if first < end {
            return l.lines[first..end]
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1.baseline - y).abs().total_cmp(&(b.1.baseline - y).abs()))
                .map(|(i, _)| first + i)
                .unwrap_or(0);
        }
    }
    l.lines
        .iter()
        .enumerate()
        .filter(|(_, li)| li.para == para)
        .min_by(|a, b| (a.1.baseline - y).abs().total_cmp(&(b.1.baseline - y).abs()))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// The box a line's text covers: its top and bottom, its first and last caret.
pub(crate) fn line_bounds(li: &deckcraft_text::LineInfo) -> Option<Rect> {
    let (x0, x1) = li.caret_x.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(*x), b.max(*x)));
    let r = Rect::new(x0, li.top, x1, li.bottom);
    (r.is_finite() && r.width() > 0.0 && r.height() > 0.0).then_some(r)
}

#[derive(Clone, Copy, PartialEq)]
enum TextEffect {
    Shadow,
    Glow,
}

/// Draw `run`'s shadow or glow (if it has one) under its glyphs, placed by `m`.
fn text_effect(ctx: &mut RenderContext, run: &deckcraft_text::GlyphRun, which: TextEffect, m: Affine, opacity: f64) {
    let Some(fx) = run.effects else { return };
    let k = run.size / run.face.upem.max(1.0);
    let skew = if run.fake_italic { Affine::new([1.0, 0.0, -0.2, 1.0, 0.0, 0.0]) } else { Affine::IDENTITY };
    let px = scale_of(m);
    let (c, sigma, offset, grow) = match (which, fx.shadow, fx.glow) {
        (TextEffect::Shadow, Some(s), _) => (s.color, s.blur * 0.5 * px, Vec2::new(s.dx, s.dy), 0.0),
        // Measured on PowerPoint's export: a text glow of radius r is solid to about a third of r
        // past the glyph's edge and fades out by about two thirds of r.
        (TextEffect::Glow, _, Some((gc, r))) => (gc, r * 0.15 * px, Vec2::ZERO, r * 0.68),
        _ => return,
    };
    let alpha = (c.a as f64 / 255.0) * opacity;
    if alpha <= 0.0 {
        return;
    }
    ctx.set_transform(Affine::IDENTITY);
    ctx.push_layer(None, None, Some(alpha as f32), None, Some(blur(sigma)));
    ctx.set_paint(color(c.with_alpha(255), 1.0));
    for (gid, x, y) in &run.glyphs {
        let path = glyph(&run.face, *gid);
        if path.elements().is_empty() {
            continue;
        }
        // The offset is in slide space: a shadow falls the same way whatever the text's rotation.
        let at = pre_translate(m * glyph_place(run, *gid, *x, *y), offset * px) * skew * Affine::scale(k);
        ctx.set_transform(at);
        ctx.fill_path(&path);
        if grow > 0.0 {
            ctx.set_stroke(kurbo::Stroke::new(grow / k.max(1e-9)).with_join(kurbo::Join::Round));
            ctx.stroke_path(&path);
        }
    }
    ctx.pop_layer();
}

/// Draw glyph `gid` of `run` with its origin (on the baseline) mapped by `at`, faded by `opacity`.
pub(crate) fn draw_glyph(ctx: &mut RenderContext, run: &deckcraft_text::GlyphRun, gid: u32, at: Affine, opacity: f64) {
    draw_glyph_with(ctx, run, gid, at, opacity, None);
}

/// [`draw_glyph`], filled with `fill` (a gradient laid out in the text's space, and the map from
/// the glyph's origin to that space) instead of the run's colour.
fn draw_glyph_with(
    ctx: &mut RenderContext,
    run: &deckcraft_text::GlyphRun,
    gid: u32,
    at: Affine,
    opacity: f64,
    fill: Option<(&peniko::Gradient, Affine)>,
) {
    let alpha = run.alpha * opacity;
    if (alpha <= 0.0 || run.color.a == 0) && run.outline.is_none() {
        return;
    }
    let k = run.size / run.face.upem.max(1.0);
    // Colour glyphs (emoji) paint their own colours; an outline (text outline effect) still
    // strokes the plain glyph below.
    if alpha > 0.0 && run.outline.is_none() && run.face.is_color() && colr::paint(ctx, &run.face, gid, at, k, run.color, alpha) {
        return;
    }
    let path = glyph(&run.face, gid);
    if path.elements().is_empty() {
        return;
    }
    let skew = if run.fake_italic { Affine::new([1.0, 0.0, -0.2, 1.0, 0.0, 0.0]) } else { Affine::IDENTITY };
    ctx.set_transform(at * skew * Affine::scale(k));
    if let Some((g, to_text)) = fill
        && alpha > 0.0
    {
        ctx.set_paint(g.clone());
        ctx.set_paint_transform((skew * Affine::scale(k)).inverse() * to_text);
        ctx.fill_path(&path);
        ctx.reset_paint_transform();
    } else if alpha > 0.0 && run.color.a > 0 {
        ctx.set_paint(color(run.color, alpha));
        ctx.fill_path(&path);
        if run.fake_bold {
            ctx.set_stroke(kurbo::Stroke::new(run.face.upem * 0.025).with_join(kurbo::Join::Round));
            ctx.stroke_path(&path);
        }
    }
    if let Some((oc, ow)) = run.outline {
        ctx.set_paint(color(oc, opacity));
        ctx.set_stroke(kurbo::Stroke::new(ow / k.max(1e-9)).with_join(kurbo::Join::Round));
        ctx.stroke_path(&path);
    }
}

#[cfg(test)]
mod line_index_tests {
    use super::*;
    use deckcraft_text::{LineInfo, TextLayout};

    fn layout(ordered: bool) -> TextLayout {
        let line = |para, top: f64| LineInfo { para, top, baseline: top + 8.0, bottom: top + 10.0, ..Default::default() };
        let mut lines: Vec<LineInfo> = (0..40).map(|i| line(usize::from(i >= 25), f64::from(i) * 10.0)).collect();
        if !ordered {
            lines.swap(3, 7);
        }
        TextLayout { lines, para_lines: vec![(0, 25), (25, 15)], ..Default::default() }
    }

    #[test]
    fn bisection_matches_the_scan() {
        for ordered in [true, false] {
            let l = layout(ordered);
            let idx = LineIndex::new(l_ref(&l));
            assert_eq!(idx.ordered, vec![ordered, true]);
            for para in 0..2 {
                for k in -20..450 {
                    let y = f64::from(k) * 0.9;
                    assert_eq!(idx.line_at(para, y), line_at_scan(&l, para, y), "line_at {para} {y}");
                    assert_eq!(idx.line_of(para, y), crate::warp::line_of_scan(&l, para, y), "line_of {para} {y}");
                }
            }
        }
    }

    fn l_ref(l: &TextLayout) -> &TextLayout {
        l
    }
}
