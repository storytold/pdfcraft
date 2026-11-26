use super::geometry::{
    Caster, blur, border_outside, camera_turn, color, drawable, fill_opacity, outside_line, scale_of, shadow_local, shape_geometry,
};
use super::opts::{RenderOpts, ShapeState, SlideFields};
use super::raster::Image;
use super::slide::{SlidePart, needs_filters, output_size};
use super::stroke::{arrowheads, head_trims, stroke, trim_ends};
use super::text_draw::{draw_layout_with, draw_text, line_bounds, preview_empty_bullets, prompt_body, text_transform, text_warp};
use super::*;

pub struct Renderer {
    resources: Resources,
    /// Outlines morphing in the frame being drawn by `blend`.
    paths: Vec<PathMorph>,
    /// The background of the slide (layout, master) being drawn, its placeholder colour and the
    /// slide's view transform: shapes filled with the background (`useBgFill`) show it through.
    bg: Option<(Fill, Option<Rgba>, Affine)>,
    /// What the frame being drawn has painted so far, in output pixels: the editor learns from it
    /// how far a slide's drawing (text spilling out of its box too) reaches off the slide.
    pub(crate) drawn: Option<Rect>,
    /// The transform a top-level shape is drawn under (slide to pixels); a group's children draw
    /// under it combined with the group's child-coordinate mapping, which must not scale lines.
    view: Affine,
    /// The fill of the group being drawn, with the transform and box it is laid out in: children
    /// filled with the group's fill (`a:grpFill`) show it, as one fill across the whole group.
    group_fill: Option<(Fill, Affine, Rect)>,
    /// Their aligned outlines, kept from frame to frame.
    morphs: morph_path::Cache,
    /// Whether we are currently drawing an outer-shadow silhouette pass for an ancestor group.
    in_shadow_pass: bool,
    /// Cumulative shape traversal counter to prevent nested group amplification DoS.
    render_work: usize,
    /// Visits spent on outer-shadow silhouette passes, kept apart so they cannot use up the budget
    /// of the normal drawing that follows.
    shadow_work: usize,
    /// `RenderOpts::skip` as a set, with the slice (address, length) it was built from: the
    /// slice can be as long as the slide, and is tested for every shape drawn. Forgotten when a
    /// frame starts, so another frame's slice at the same address is never mistaken for it.
    skip: Option<(usize, usize, std::collections::HashSet<ShapeId>)>,
}

impl Default for Renderer {
    fn default() -> Self {
        Renderer {
            resources: Resources::new(),
            paths: Vec::new(),
            bg: None,
            drawn: None,
            view: Affine::IDENTITY,
            group_fill: None,
            morphs: morph_path::Cache::default(),
            in_shadow_pass: false,
            render_work: 0,
            shadow_work: 0,
            skip: None,
        }
    }
}

thread_local! {
    pub(crate) static RENDERER: std::cell::RefCell<Renderer> = std::cell::RefCell::new(Renderer::default());
}

pub(crate) struct Frame<'a> {
    pub(crate) pres: &'a Presentation,
    pub(crate) opts: &'a RenderOpts<'a>,
    pub(crate) fields: SlideFields,
}

/// A shape being drawn: its resolved geometry, fill, line and effects, in shape space `m` at size
/// `w` x `h`, shared by the stages of `Renderer::geometry_shape`.
struct ShapeDraw<'a, 'ctx> {
    f: &'a Frame<'a>,
    rctx: &'a Ctx<'ctx>,
    s: &'a Shape,
    st: &'a ShapeState,
    m: Affine,
    w: f64,
    h: f64,
    depth: usize,
    geo: Geometry,
    fades: Vec<deckcraft_geom::morph::Fade>,
    fill: Option<Fill>,
    fill_ph: Option<Rgba>,
    line: Line,
    line_ph: Option<Rgba>,
    effects: Option<Effects>,
    fx_ph: Option<Rgba>,
    outline: BezPath,
    is_pic: bool,
    empty_ph: bool,
}

impl ShapeDraw<'_, '_> {
    /// How much of the fill and stroke of path `i` shows (morphing outlines fade).
    fn fade(&self, i: usize) -> deckcraft_geom::morph::Fade {
        self.fades.get(i).copied().unwrap_or(deckcraft_geom::morph::Fade { fill: 1.0, stroke: 1.0 })
    }
}

/// What casts a shape's shadow, glow and reflection: its geometry, fill and outline, in shape
/// space `m` at size `w` x `h`.
struct FxSource<'a> {
    geo: &'a Geometry,
    fill: &'a Option<Fill>,
    fill_alpha: Option<f64>,
    outline: Option<Caster<'a>>,
    content: Option<&'a deckcraft_model::style::PictureFill>,
    m: Affine,
    w: f64,
    h: f64,
}

impl Renderer {
    fn canvas(pres: &Presentation, opts: &RenderOpts, filters: bool) -> (RenderContext, u16, u16, Affine) {
        let (w, h) = output_size(pres, opts);
        let mut ctx = RenderContext::new_with(
            w,
            h,
            vello_cpu::RenderSettings { num_threads: if cfg!(target_arch = "wasm32") || filters { 0 } else { opts.threads }, ..Default::default() },
        );
        if let Some(c) = opts.clear {
            ctx.set_paint(color(c, 1.0));
            ctx.fill_rect(&Rect::new(0.0, 0.0, w as f64, h as f64));
        }
        let view = Affine::translate((opts.offset.0, opts.offset.1)) * Affine::scale(opts.scale);
        (ctx, w, h, view)
    }

    /// Note that `r` (local space, placed by `m`) was drawn.
    fn track(&mut self, m: Affine, r: Rect) {
        let b = m.transform_rect_bbox(r);
        if b.is_finite() {
            self.drawn = Some(self.drawn.map_or(b, |d| d.union(b)));
        }
    }

    pub(crate) fn fields(pres: &Presentation, index: usize, opts: &RenderOpts) -> SlideFields {
        SlideFields::new(pres.slide_number(index), pres.header_footer.footer_text.clone(), opts)
    }

    /// Background plus the master and layout graphics the slide shows.
    fn backdrop(&mut self, ctx: &mut RenderContext, f: &Frame, rctx: &Ctx, slide: &Slide, view: Affine) {
        // Backdrop graphics never take shape states: their ids may clash with the slide's shapes.
        let bopts = RenderOpts { state: None, date_text: f.opts.date_text.clone(), ..*f.opts };
        let bf = Frame { pres: f.pres, opts: &bopts, fields: f.fields.clone() };
        let f = &bf;
        self.background(ctx, f, rctx, Some(slide), view);
        let (show_layout, show_master) = resolve::show_master_shapes(slide, rctx.layout);
        if show_master {
            let mctx = Ctx::for_master(f.pres, rctx.master);
            for s in &rctx.master.shapes {
                if s.ph.is_none() {
                    self.shape(ctx, f, &mctx, s, view, 0);
                }
            }
        }
        if show_layout && let Some(l) = rctx.layout {
            let lctx = Ctx::for_layout(f.pres, rctx.master, l);
            for s in &l.shapes {
                if s.ph.is_none() {
                    self.shape(ctx, f, &lctx, s, view, 0);
                }
            }
        }
    }

    pub(crate) fn slide(&mut self, pres: &Presentation, slide: &Slide, index: usize, opts: &RenderOpts) -> Image {
        self.render_work = 0;
        self.shadow_work = 0;
        self.skip = None;
        self.in_shadow_pass = false;
        let (mut ctx, w, h, view) = Self::canvas(pres, opts, needs_filters(pres, slide));
        let f = Frame { pres, opts, fields: Self::fields(pres, index, opts) };
        if let Some(rctx) = Ctx::for_slide(pres, slide) {
            self.backdrop(&mut ctx, &f, &rctx, slide, view);
            for s in &slide.shapes {
                self.shape(&mut ctx, &f, &rctx, s, view, 0);
            }
        }
        self.finish(ctx, w, h)
    }

    pub(crate) fn slide_part(&mut self, pres: &Presentation, slide: &Slide, index: usize, part: SlidePart, opts: &RenderOpts) -> Image {
        self.render_work = 0;
        self.shadow_work = 0;
        self.skip = None;
        self.in_shadow_pass = false;
        let (mut ctx, w, h, view) = Self::canvas(pres, opts, needs_filters(pres, slide));
        let f = Frame { pres, opts, fields: Self::fields(pres, index, opts) };
        if let Some(rctx) = Ctx::for_slide(pres, slide) {
            match part {
                SlidePart::Backdrop => self.backdrop(&mut ctx, &f, &rctx, slide, view),
                SlidePart::Content => {
                    // Shapes filled with the background (`useBgFill`) still show it.
                    let (fill, ph) = resolve::background(&rctx, Some(slide));
                    self.bg = Some((fill, ph, view));
                    for s in &slide.shapes {
                        self.shape(&mut ctx, &f, &rctx, s, view, 0);
                    }
                }
            }
        }
        self.finish(ctx, w, h)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn blend(
        &mut self,
        pres: &Presentation,
        old: (&Slide, usize),
        new: (&Slide, usize),
        mix: f64,
        shapes: &[(Shape, bool)],
        text: &[TextMorph],
        paths: &[PathMorph],
        opts: &RenderOpts,
    ) -> Image {
        let mix = if mix.is_finite() { mix.clamp(0.0, 1.0) } else { 1.0 };
        self.render_work = 0;
        self.shadow_work = 0;
        self.skip = None;
        self.in_shadow_pass = false;
        let (mut ctx, w, h, view) = Self::canvas(pres, opts, needs_filters(pres, old.0) || needs_filters(pres, new.0));
        let (Some(co), Some(cn)) = (Ctx::for_slide(pres, old.0), Ctx::for_slide(pres, new.0)) else { return self.finish(ctx, w, h) };
        if mix < 1.0 {
            let f = Frame { pres, opts, fields: Self::fields(pres, old.1, opts) };
            self.backdrop(&mut ctx, &f, &co, old.0, view);
        }
        if mix > 0.0 {
            let f = Frame { pres, opts, fields: Self::fields(pres, new.1, opts) };
            let layered = mix < 1.0;
            if layered {
                ctx.push_layer(None, None, Some(mix as f32), None, None);
            }
            self.backdrop(&mut ctx, &f, &cn, new.0, view);
            if layered {
                ctx.pop_layer();
            }
        }
        let fo = Frame { pres, opts, fields: Self::fields(pres, old.1, opts) };
        let fnew = Frame { pres, opts, fields: Self::fields(pres, new.1, opts) };
        let mut drawn: Vec<ShapeId> = Vec::new();
        // Set only now: the backdrop's ids may clash with the frame's.
        self.paths = paths.to_vec();
        for (s, from_old) in shapes {
            let (f, c) = if *from_old { (&fo, &co) } else { (&fnew, &cn) };
            let Some(tm) = text.iter().find(|m| m.old == s.id || m.new == s.id) else {
                self.shape(&mut ctx, f, c, s, view, 0);
                continue;
            };
            self.shape(&mut ctx, f, c, &Shape { text: None, ..s.clone() }, view, 0);
            drawn.push(s.id);
            if !(drawn.contains(&tm.old) && drawn.contains(&tm.new)) {
                continue;
            }
            let find = |id: ShapeId| shapes.iter().map(|(s, _)| s).find(|s| s.id == id);
            if let (Some(a), Some(b)) = (find(tm.old), find(tm.new)) {
                let now = b.xfrm.unwrap_or_else(|| resolve::xfrm(&cn, b));
                morph_text::draw(&mut ctx, (&co, a, &fo.fields), (&cn, b, &fnew.fields), tm, now, view);
            }
        }
        self.paths.clear();
        self.morphs.sweep();
        self.finish(ctx, w, h)
    }

    pub(crate) fn layout(&mut self, pres: &Presentation, mi: usize, li: Option<usize>, opts: &RenderOpts) -> Image {
        self.render_work = 0;
        self.shadow_work = 0;
        self.skip = None;
        self.in_shadow_pass = false;
        let (w, h) = output_size(pres, opts);
        let mut ctx = RenderContext::new_with(w, h, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
        let view = Affine::translate((opts.offset.0, opts.offset.1)) * Affine::scale(opts.scale);
        let f = Frame { pres, opts, fields: SlideFields::new(1, String::new(), opts) };
        if let Some(m) = pres.masters.get(mi) {
            match li.and_then(|i| m.layouts.get(i)) {
                Some(l) => {
                    let c = Ctx::for_layout(pres, m, l);
                    self.background(&mut ctx, &f, &c, None, view);
                    if l.show_master_shapes {
                        let mc = Ctx::for_master(pres, m);
                        for s in m.shapes.iter().filter(|s| s.ph.is_none()) {
                            self.shape(&mut ctx, &f, &mc, s, view, 0);
                        }
                    }
                    for s in &l.shapes {
                        self.shape(&mut ctx, &f, &c, s, view, 0);
                    }
                }
                None => {
                    let c = Ctx::for_master(pres, m);
                    self.background(&mut ctx, &f, &c, None, view);
                    for s in &m.shapes {
                        self.shape(&mut ctx, &f, &c, s, view, 0);
                    }
                }
            }
        }
        self.finish(ctx, w, h)
    }

    pub(crate) fn finish(&mut self, mut ctx: RenderContext, w: u16, h: u16) -> Image {
        ctx.flush();
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        if let Some(pm) = vello_cpu::PixmapMut::new(w, h, &mut pixels) {
            ctx.render(pm, &mut self.resources);
        }
        Image { width: w as u32, height: h as u32, pixels }
    }

    fn background(&mut self, ctx: &mut RenderContext, f: &Frame, rctx: &Ctx, slide: Option<&Slide>, view: Affine) {
        let (fill, ph) = resolve::background(rctx, slide);
        if f.opts.transparent_background {
            self.bg = Some((fill, ph, view));
            return;
        }
        let r = Rect::new(0.0, 0.0, f.pres.slide_size.width, f.pres.slide_size.height);
        ctx.set_transform(view);
        let path = r.to_path(0.1);
        // PowerPoint lays every background over white: a picture stretched into part of the
        // slide, or a see-through colour, leaves white around and under it.
        ctx.set_paint(color(Rgba::WHITE, 1.0));
        ctx.fill_path(&path);
        paint::fill_path(ctx, rctx, &fill, ph, &path, r, view, FillMode::Norm, 1.0, f.pres);
        self.bg = Some((fill, ph, view));
    }

    pub(crate) fn shape(&mut self, ctx: &mut RenderContext, f: &Frame, rctx: &Ctx, s: &Shape, parent: Affine, depth: usize) {
        if depth == 0 {
            let outer = std::mem::replace(&mut self.view, parent);
            self.shape_at(ctx, f, rctx, s, parent, depth);
            self.view = outer;
        } else {
            self.shape_at(ctx, f, rctx, s, parent, depth);
        }
    }

    /// How lines are drawn for a shape under `m` at `depth`: the transform to stroke under and the
    /// one that takes the shape's outline into it. Inside a group they are the slide-level view and
    /// the rest of the chain, so the group's child-coordinate scale moves and sizes the outline but
    /// never the line width; outside a group, `m` itself and no change to the outline.
    fn line_space(&self, m: Affine, depth: usize) -> (Affine, Affine) {
        let view = self.view;
        if depth > 0 && view.determinant().abs() > 1e-12 && view.is_finite() { (view, view.inverse() * m) } else { (m, Affine::IDENTITY) }
    }

    /// Shape-space units per point: a group's child coordinates can scale a shape's box (a legacy
    /// 1360-unit child space makes one unit hundreds of EMUs), but 3-D sizes kept in points
    /// (contour width, extrusion depth, bevel) stay points, as PowerPoint draws them. 1 outside a
    /// group.
    fn per_point(&self, m: Affine, depth: usize) -> f64 {
        let k = scale_of(self.line_space(m, depth).1);
        if k.is_finite() && k > 1e-12 { 1.0 / k } else { 1.0 }
    }

    /// [`Renderer::per_point`] along the shape's own x and y axes: a group can stretch its child
    /// space unevenly, and a tiled fill keeps its size in points on the slide on both axes.
    fn per_point_xy(&self, m: Affine, depth: usize) -> (f64, f64) {
        let c = self.line_space(m, depth).1.as_coeffs();
        let inv = |s: f64| if s.is_finite() && s > 1e-12 { 1.0 / s } else { 1.0 };
        (inv(c[0].hypot(c[1])), inv(c[2].hypot(c[3])))
    }

    /// The size and transform a shape's text is set in: inside a group whose child coordinates
    /// scale the shape, the shape's size in slide points and `m` with that scale taken out, so
    /// insets and font sizes stay in points; otherwise `w`, `h` and `m` themselves.
    fn text_space(&self, m: Affine, depth: usize, w: f64, h: f64) -> (f64, f64, Affine) {
        let (_, rel) = self.line_space(m, depth);
        crate::placed::text_space(rel, m, depth, w, h)
    }

    /// A shape's geometry in its own `w` × `h` space, built at its size on the slide: inside a
    /// group whose child coordinates scale the shape unevenly, the preset's rounded corners and
    /// other size-derived features stay as they would be on a plain shape of that size (#1083).
    fn scaled_geometry(&self, s: &Shape, m: Affine, depth: usize, w: f64, h: f64) -> Geometry {
        let (tw, th, _) = self.text_space(m, depth, w, h);
        if (tw, th) == (w, h) {
            return shape_geometry(s, w, h);
        }
        let mut geo = shape_geometry(s, tw, th);
        let k = |a: f64, b: f64| if b.abs() > 1e-12 { a / b } else { 1.0 };
        let back = Affine::scale_non_uniform(k(w, tw), k(h, th));
        for p in &mut geo.paths {
            p.path.apply_affine(back);
        }
        geo.text_rect = back.transform_rect_bbox(geo.text_rect);
        for p in &mut geo.sites {
            *p = back * *p;
        }
        geo
    }

    /// Whether `id` is in `skip`, looked up in a set built once per frame and slice.
    fn skipped(&mut self, skip: &[ShapeId], id: ShapeId) -> bool {
        if skip.is_empty() {
            return false;
        }
        let key = (skip.as_ptr() as usize, skip.len());
        match &self.skip {
            Some((p, n, set)) if (*p, *n) == key => set.contains(&id),
            _ => {
                let set: std::collections::HashSet<ShapeId> = skip.iter().copied().collect();
                let hit = set.contains(&id);
                self.skip = Some((key.0, key.1, set));
                hit
            }
        }
    }

    fn shape_at(&mut self, ctx: &mut RenderContext, f: &Frame, rctx: &Ctx, s: &Shape, parent: Affine, depth: usize) {
        if depth > 32 || (s.hidden && !f.opts.show_hidden) || self.skipped(f.opts.skip, s.id) {
            return;
        }
        let work = if self.in_shadow_pass { &mut self.shadow_work } else { &mut self.render_work };
        *work = work.saturating_add(1);
        if *work > 100_000 {
            return;
        }
        // Footer placeholders on layouts/masters show only on slides that include them.
        let st = f.opts.state.map(|g| g(s.id)).unwrap_or_default();
        if !st.visible || st.opacity <= 0.001 {
            return;
        }
        let x = resolve::xfrm(rctx, s);
        if !x.is_finite() {
            return;
        }
        let mut m = parent * x.affine();
        if st.offset != Vec2::ZERO || st.scale != (1.0, 1.0) || st.rotate != 0.0 {
            let c = Point::new(x.w / 2.0, x.h / 2.0);
            let around = Affine::translate(c.to_vec2())
                * Affine::rotate(st.rotate.to_radians())
                * Affine::scale_non_uniform(st.scale.0, st.scale.1)
                * Affine::translate(-c.to_vec2());
            m = parent * Affine::translate(st.offset) * x.affine() * around;
        }
        if let Some(a) = camera_turn(rctx, s, x.w, x.h) {
            m *= a;
        }
        let layered = st.opacity < 0.999 || st.clip.is_some();
        if layered {
            ctx.set_transform(m);
            let clip = st.clip.map(|[l, t, r, b]| Rect::new(l * x.w, t * x.h, r * x.w, b * x.h).to_path(0.1));
            ctx.push_layer(clip.as_ref(), None, Some(st.opacity.clamp(0.0, 1.0) as f32), None, None);
        }
        match &s.kind {
            ShapeKind::Group { children, child } => {
                let ch = Rect::new(child.x, child.y, child.x + child.w, child.y + child.h);
                let gm = parent * deckcraft_geom::group_child_affine(&x, ch);
                // A group's own fill (a group filled with its parent's passes that on).
                let own = match &s.fill {
                    Some(Fill::Group) | None => self.group_fill.clone(),
                    // A tile is given in points; this group's own space may be scaled by its parents.
                    Some(fl) => Some((paint::tiled_per_point(fl, self.per_point_xy(m, depth)).into_owned(), m, Rect::new(0.0, 0.0, x.w, x.h))),
                };
                let outer = std::mem::replace(&mut self.group_fill, own);
                // A group's outer shadow is cast by everything in it, as PowerPoint draws it: the
                // children drawn once more under the shadow's offset, scale and skew, recoloured to
                // the shadow colour and blurred, below the group.
                if !self.in_shadow_pass
                    && let Some(sh) = s.effects.as_ref().and_then(|e| e.outer_shadow.as_ref())
                {
                    let c = rctx.color(&sh.color, None);
                    let k = scale_of(m);
                    let d = Vec2::new(sh.dir.to_radians().cos(), sh.dir.to_radians().sin()) * sh.dist;
                    let local = shadow_local(sh, x.w, x.h);
                    let (own, pre) =
                        if sh.rotate_with_shape { (Affine::translate(d) * local, Affine::IDENTITY) } else { (local, Affine::translate(d * k)) };
                    let det = m.determinant();
                    if det.is_finite() && det.abs() > 1e-12 {
                        let shift = pre * m * own * m.inverse();
                        ctx.set_transform(Affine::IDENTITY);
                        ctx.push_layer(None, None, Some((c.a as f32) / 255.0), None, Some(blur(sh.blur / 3.0 * k)));
                        self.in_shadow_pass = true;
                        for ch in children {
                            self.shape(ctx, f, rctx, ch, shift * gm, depth + 1);
                        }
                        self.in_shadow_pass = false;
                        ctx.set_transform(Affine::IDENTITY);
                        ctx.push_layer(None, Some(peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcIn)), None, None, None);
                        ctx.set_paint(color(c.with_alpha(255), 1.0));
                        let (w, h) = (f.opts.scale * f.pres.slide_size.width, f.opts.scale * f.pres.slide_size.height);
                        ctx.fill_rect(&Rect::new(-w, -h, 3.0 * w, 3.0 * h).inflate(1e4, 1e4));
                        ctx.pop_layer();
                        ctx.pop_layer();
                    }
                }
                for c in children {
                    self.shape(ctx, f, rctx, c, gm, depth + 1);
                }
                self.group_fill = outer;
            }
            ShapeKind::Table(t) => {
                // Rows grow with their text: what the cells cover, borders included.
                let cells = table::draw(ctx, f.pres, rctx, t, m, x.w, x.h, &f.fields, f.opts.edit && f.opts.editing_text == Some(s.id));
                self.track(m, cells.unwrap_or(Rect::new(0.0, 0.0, x.w, x.h)).inflate(2.0, 2.0));
            }
            ShapeKind::Chart(c) => {
                self.track(m, Rect::new(0.0, 0.0, x.w, x.h));
                chart::draw(ctx, rctx, c, m, x.w, x.h)
            }
            ShapeKind::Ink { strokes } => {
                for sk in strokes {
                    let legacy = deckcraft_model::ink_pressure_is_legacy(&sk.points);
                    let pen = if sk.width.is_finite() { sk.width.abs() } else { 0.0 };
                    for (px, py, pr) in &sk.points {
                        let w = deckcraft_model::ink_sample_width(pen, *pr, legacy);
                        self.track(parent, Rect::new(*px, *py, *px, *py).inflate(w, w));
                    }
                    let pts: Vec<(f64, f64, f32)> = sk.points.iter().copied().filter(|p| p.0.is_finite() && p.1.is_finite()).collect();
                    if pts.len() < 2 {
                        continue;
                    }
                    let (base, rel) = self.line_space(parent, depth);
                    ctx.set_transform(base);
                    ctx.set_paint(color(sk.color, if sk.highlighter { 0.4 } else { 1.0 }));
                    // Uniform-width and highlighter strokes are one path: segments drawn one by one
                    // overlap at the joins, which darkens see-through highlighter ink in beads.
                    if legacy || sk.highlighter {
                        let mut bp = BezPath::new();
                        for (i, (px, py, _)) in pts.iter().enumerate() {
                            if i == 0 {
                                bp.move_to((*px, *py));
                            } else {
                                bp.line_to((*px, *py));
                            }
                        }
                        bp.apply_affine(rel);
                        ctx.set_stroke(kurbo::Stroke::new(pen.max(0.1)).with_caps(kurbo::Cap::Round).with_join(kurbo::Join::Round));
                        ctx.stroke_path(&bp);
                        continue;
                    }
                    for pair in pts.windows(2) {
                        let [p0, p1] = pair else { continue };
                        let mut bp = BezPath::new();
                        bp.move_to((p0.0, p0.1));
                        bp.line_to((p1.0, p1.1));
                        bp.apply_affine(rel);
                        let a = deckcraft_model::ink_sample_width(pen, p0.2, legacy);
                        let b = deckcraft_model::ink_sample_width(pen, p1.2, legacy);
                        ctx.set_stroke(kurbo::Stroke::new(((a + b) * 0.5).max(0.1)).with_caps(kurbo::Cap::Round).with_join(kurbo::Join::Round));
                        ctx.stroke_path(&bp);
                    }
                }
            }
            _ => self.geometry_shape(ctx, f, rctx, s, m, x.w, x.h, &st, depth),
        }
        if layered {
            ctx.pop_layer();
        }
    }

    /// Fill the filled sub-paths of `geo` (a `w` × `h` shape placed by `m`) with `fl`, faded per
    /// sub-path by `fade`: `useBgFill` shows the slide background through the shape and `grpFill`
    /// the group's fill.
    #[allow(clippy::too_many_arguments)]
    fn fill_geometry(
        &self,
        ctx: &mut RenderContext,
        f: &Frame,
        rctx: &Ctx,
        fl: &Fill,
        fill_ph: Option<Rgba>,
        geo: &Geometry,
        (w, h): (f64, f64),
        m: Affine,
        per_point: (f64, f64),
        fade: &dyn Fn(usize) -> deckcraft_geom::morph::Fade,
    ) {
        for (i, sp) in geo.paths.iter().enumerate() {
            if sp.fill == FillMode::None || fade(i).fill <= 0.0 {
                continue;
            }
            ctx.set_transform(m);
            ctx.set_fill_rule(if sp.even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
            match (fl, &self.bg) {
                // `useBgFill`: the slide's background, laid out on the slide (not the
                // shape), shows through the shape.
                (Fill::Background, Some((bg, ph, view))) => {
                    let slide = Rect::new(0.0, 0.0, f.pres.slide_size.width, f.pres.slide_size.height);
                    let path = (view.inverse() * m) * sp.path.clone();
                    paint::fill_path(ctx, rctx, bg, *ph, &path, slide, *view, sp.fill, fade(i).fill, f.pres);
                }
                // `grpFill`: the group's fill, laid out over the whole group.
                (Fill::Group, _) => {
                    if let Some((gf, gm, gbox)) = &self.group_fill
                        && gm.determinant().abs() > 1e-12
                    {
                        let path = (gm.inverse() * m) * sp.path.clone();
                        paint::fill_path(ctx, rctx, gf, None, &path, *gbox, *gm, sp.fill, fade(i).fill, f.pres);
                    }
                }
                _ => {
                    let fl = paint::tiled_per_point(fl, per_point);
                    paint::fill_path(ctx, rctx, &fl, fill_ph, &sp.path, Rect::new(0.0, 0.0, w, h), m, sp.fill, fade(i).fill, f.pres)
                }
            }
            ctx.set_fill_rule(peniko::Fill::NonZero);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn geometry_shape(
        &mut self,
        ctx: &mut RenderContext,
        f: &Frame,
        rctx: &Ctx,
        s: &Shape,
        m: Affine,
        w: f64,
        h: f64,
        st: &ShapeState,
        depth: usize,
    ) {
        let (geo, fades) = match self.paths.iter().find(|p| p.id == s.id) {
            Some(pm) => self.morphs.geometry(pm, s, w, h),
            None => (self.scaled_geometry(s, m, depth, w, h), Vec::new()),
        };
        if !drawable(&geo, m, w, h) {
            log::warn!("render: shape {} skipped: its outline is not finite or far larger than any screen", s.id.0);
            return;
        }
        let (fill, fill_ph) = resolve::fill(rctx, s);
        let (line, line_ph) = resolve::line(rctx, s);
        let (effects, fx_ph) = resolve::effects(rctx, s);
        let reach = extent::spill(Some(&line), effects.as_ref(), w, h);
        self.track(m, Rect::new(0.0, 0.0, w, h).inflate(reach, reach));
        let is_pic = matches!(s.kind, ShapeKind::Picture { .. });
        let outline = geo.outline();
        // Placeholders in the editor: dashed outline and prompt, nothing else when empty.
        let empty_ph = s.ph.is_some() && s.text.as_ref().is_none_or(TextBody::is_empty) && !is_pic && !matches!(s.kind, ShapeKind::Media(_));
        let fill = if let Some(t) = st.tint { Some(Fill::solid(deckcraft_model::ColorRef::rgb(t))) } else { fill };
        let d = ShapeDraw { f, rctx, s, st, m, w, h, depth, geo, fades, fill, fill_ph, line, line_ph, effects, fx_ph, outline, is_pic, empty_ph };
        self.cast_effects(ctx, &d);
        let soft = d.effects.as_ref().and_then(|e| e.soft_edge).filter(|r| *r > 0.0);
        if let Some(r) = soft {
            ctx.set_transform(Affine::IDENTITY);
            ctx.push_layer(None, None, None, None, None);
            let _ = r;
        }
        let pt = self.point_scale(&d);
        self.draw_extrusion(ctx, &d, pt);
        self.draw_contour(ctx, &d, pt);
        self.draw_fill(ctx, &d);
        self.draw_fill_overlay(ctx, &d);
        self.draw_bevel(ctx, &d, pt);
        self.draw_outline(ctx, &d);
        if let Some(r) = soft {
            self.draw_feather(ctx, &d, r);
        }
        self.draw_inner_shadow(ctx, &d);
        self.draw_text_body(ctx, &d);
    }

    /// Shape-space units per point for the 3-D sizes, measured without the camera's turn (which
    /// `shape_at` puts in `m`): its foreshortening is not a child-coordinate scale.
    fn point_scale(&self, d: &ShapeDraw) -> f64 {
        let turn = d
            .effects
            .as_ref()
            .and_then(|e| e.raw3d.as_deref())
            .and_then(camera::from_raw)
            .and_then(|c| camera::affine(&c, d.w, d.h))
            .filter(|t| t.determinant().abs() > 1e-9);
        self.per_point(turn.map_or(d.m, |t| d.m * t.inverse()), d.depth)
    }

    fn cast_effects(&mut self, ctx: &mut RenderContext, d: &ShapeDraw) {
        let m = d.m;
        let w = d.w;
        let h = d.h;
        let depth = d.depth;
        let fill_ph = d.fill_ph;
        let line_ph = d.line_ph;
        let fx_ph = d.fx_ph;
        let is_pic = d.is_pic;
        let s = d.s;
        let rctx = d.rctx;
        let geo = &d.geo;
        let fill = &d.fill;
        let effects = &d.effects;
        let line = &d.line;
        // Effects below the shape.
        if let Some(e) = &effects {
            // A picture (or picture fill) casts its shadow from its pixels, not its frame. A tile
            // is sized in points on the slide, as the face's is, and so is its shadow and reflection.
            let content = match (&s.kind, &fill) {
                (ShapeKind::Picture { fill: pf }, _) | (_, Some(Fill::Picture(pf))) => {
                    let pf = paint::tiled_picture_per_point(pf, self.per_point_xy(m, depth));
                    Some(paint::resolved(rctx, &pf, None).into_owned())
                }
                _ => None,
            };
            // What casts the shadow: the fill as see-through as it is, and the outline.
            let fill_alpha = fill_opacity(rctx, fill, fill_ph);
            let outline = line.fill.as_ref().filter(|f| !f.is_none()).map(|lf| {
                let a = match lf {
                    Fill::Solid { color: c } => rctx.color(c, line_ph).a as f64 / 255.0,
                    _ => 1.0,
                };
                let (base, rel) = self.line_space(m, depth);
                let lw = line.width.unwrap_or(0.75).max(0.0);
                let outside = if border_outside(is_pic, effects.as_ref()) { outside_line(line, lw) } else { None };
                Caster { line: outside.map_or(std::borrow::Cow::Borrowed(line), std::borrow::Cow::Owned), ph: line_ph, alpha: a, base, rel }
            });
            self.effects_below(ctx, rctx, e, fx_ph, FxSource { geo, fill, fill_alpha, outline, content: content.as_ref(), m, w, h });
        }
    }

    fn draw_extrusion(&mut self, ctx: &mut RenderContext, d: &ShapeDraw, pt: f64) {
        let m = d.m;
        let w = d.w;
        let h = d.h;
        let fill_ph = d.fill_ph;
        let rctx = d.rctx;
        let fill = &d.fill;
        let effects = &d.effects;
        let outline = &d.outline;
        // A 3-D extrusion under a turned camera: the sides, swept behind the face.
        if let Some(raw) = effects.as_ref().and_then(|e| e.raw3d.as_deref())
            && let Some(ex) = extrude::from_raw(raw)
            && let Some(cam) = camera::from_raw(raw)
            && let Some(turn) = camera::affine(&cam, w, h)
            && turn.determinant().abs() > 1e-9
        {
            let face = match &fill {
                Some(Fill::Solid { color: c }) => Some(rctx.color(c, fill_ph)),
                _ => None,
            };
            let pre = m * turn.inverse();
            let lit = light::Lighting::from_raw(raw);
            let d = ex.depth * pt;
            extrude::draw(ctx, outline, w, h, &cam, d, pre, turn, extrude::side_color(rctx, &ex, face), &lit);
        }
    }

    fn draw_contour(&mut self, ctx: &mut RenderContext, d: &ShapeDraw, pt: f64) {
        let m = d.m;
        let fill_ph = d.fill_ph;
        let rctx = d.rctx;
        let fill = &d.fill;
        let effects = &d.effects;
        let outline = &d.outline;
        // A 3-D contour: a band around the outline, behind the face (and in front of any sides).
        if let Some(raw) = effects.as_ref().and_then(|e| e.raw3d.as_deref())
            && let Some(c) = extrude::contour_from_raw(raw)
        {
            let face = match &fill {
                Some(Fill::Solid { color: col }) => Some(rctx.color(col, fill_ph)),
                _ => None,
            };
            let col = c.color.as_ref().map(|k| rctx.color(k, face)).or(face).unwrap_or(Rgba::rgb(0xff, 0xff, 0xff));
            extrude::draw_contour(ctx, outline, m, c.width * pt, col, &light::Lighting::from_raw(raw));
        }
    }

    fn draw_fill(&mut self, ctx: &mut RenderContext, d: &ShapeDraw) {
        let m = d.m;
        let w = d.w;
        let h = d.h;
        let fill_ph = d.fill_ph;
        let empty_ph = d.empty_ph;
        let s = d.s;
        let f = d.f;
        let rctx = d.rctx;
        let geo = &d.geo;
        let fill = &d.fill;
        let outline = &d.outline;
        let fade = |i: usize| d.fade(i);
        let pt_xy = self.per_point_xy(m, d.depth);
        // Fill.
        match &s.kind {
            ShapeKind::Picture { fill: pf } => {
                // The picture's own fill shows behind its transparent pixels, as in PowerPoint (an
                // emphasis tint recolours it, as it does any shape's fill). A picture that sets
                // neither a fill nor a style shows the fill its layout or master placeholder sets
                // (#2068). A style fill alone, its own or inherited, stays undrawn.
                let explicit = |sh: &deckcraft_model::Shape| sh.fill.as_ref().is_some_and(|f| !f.is_none());
                let inherited = || {
                    let (lp, mp) = resolve::parents(rctx, s);
                    [lp, mp].into_iter().flatten().find(|p| p.fill.is_some() || p.style.is_some()).is_some_and(explicit)
                };
                if (explicit(s) || (s.fill.is_none() && s.style.is_none() && inherited()))
                    && let Some(fl) = &fill
                {
                    self.fill_geometry(ctx, f, rctx, fl, fill_ph, geo, (w, h), m, pt_xy, &fade);
                }
                ctx.set_transform(m);
                let bounds = Rect::new(0.0, 0.0, w, h);
                ctx.push_clip_layer(outline);
                let t = f.opts.gif_times.iter().find(|g| g.0 == s.id).map(|g| g.1);
                let pf = paint::tiled_picture_per_point(pf, pt_xy);
                paint::picture(ctx, f.pres, &paint::resolved(rctx, &pf, None), bounds, m, t);
                ctx.pop_layer();
            }
            // An object DeckCraft doesn't model (an embedded OLE object, an unknown graphic): its
            // preview picture, stretched over the frame, as PowerPoint draws it.
            ShapeKind::Opaque { preview: Some(media), .. } => {
                // An embedded object's fallback picture can carry a fill (old equation objects
                // drawn on coloured boxes): behind the preview, as for a picture.
                if s.fill.as_ref().is_some_and(|f| !f.is_none())
                    && let Some(fl) = &fill
                {
                    self.fill_geometry(ctx, f, rctx, fl, fill_ph, geo, (w, h), m, pt_xy, &fade);
                }
                ctx.set_transform(m);
                let pf = deckcraft_model::style::PictureFill { media: *media, ..Default::default() };
                paint::picture(ctx, f.pres, &pf, Rect::new(0.0, 0.0, w, h), m, None);
            }
            ShapeKind::Media(mc) => {
                ctx.set_transform(m);
                let bounds = Rect::new(0.0, 0.0, w, h);
                if let Some(p) = mc.poster {
                    let pf = deckcraft_model::style::PictureFill { media: p, crop: mc.crop, ..Default::default() };
                    ctx.push_clip_layer(outline);
                    paint::picture(ctx, f.pres, &pf, bounds, m, None);
                    ctx.pop_layer();
                } else if mc.video {
                    ctx.set_paint(peniko::Color::from_rgba8(20, 20, 24, 255));
                    ctx.fill_rect(&bounds);
                }
                if !mc.video || mc.poster.is_none() {
                    paint::media_glyph(ctx, mc.video, bounds);
                }
            }
            _ => {
                if let Some(fl) = &fill
                    && !(empty_ph && f.opts.edit && s.fill.is_none() && s.line.is_none() && s.style.is_none())
                {
                    self.fill_geometry(ctx, f, rctx, fl, fill_ph, geo, (w, h), m, pt_xy, &fade);
                }
            }
        }
    }

    fn draw_fill_overlay(&mut self, ctx: &mut RenderContext, d: &ShapeDraw) {
        let m = d.m;
        let w = d.w;
        let h = d.h;
        let fx_ph = d.fx_ph;
        let f = d.f;
        let rctx = d.rctx;
        let geo = &d.geo;
        let fill = &d.fill;
        let effects = &d.effects;
        let fade = |i: usize| d.fade(i);
        // `a:fillOverlay`: a fill blended over the shape's fill (PowerPoint's "Flow" themes multiply
        // a gradient over a tiled picture), inside the filled outline, under the line.
        if let Some(ov) = effects.as_ref().and_then(|e| e.fill_overlay.as_ref())
            && fill.as_ref().is_some_and(|f| !f.is_none())
        {
            let mix = match ov.blend.as_str() {
                "mult" => peniko::Mix::Multiply,
                "screen" => peniko::Mix::Screen,
                "darken" => peniko::Mix::Darken,
                "lighten" => peniko::Mix::Lighten,
                _ => peniko::Mix::Normal,
            };
            for (i, sp) in geo.paths.iter().enumerate().filter(|(i, sp)| sp.fill != FillMode::None && fade(*i).fill > 0.0) {
                ctx.set_transform(m);
                ctx.push_layer(Some(&sp.path), Some(peniko::BlendMode::new(mix, peniko::Compose::SrcOver)), None, None, None);
                paint::fill_path(ctx, rctx, &ov.fill, fx_ph, &sp.path, Rect::new(0.0, 0.0, w, h), m, FillMode::Norm, fade(i).fill, f.pres);
                ctx.pop_layer();
            }
        }
    }

    fn draw_bevel(&mut self, ctx: &mut RenderContext, d: &ShapeDraw, pt: f64) {
        let m = d.m;
        let w = d.w;
        let h = d.h;
        let fill_ph = d.fill_ph;
        let is_pic = d.is_pic;
        let rctx = d.rctx;
        let geo = &d.geo;
        let fill = &d.fill;
        let effects = &d.effects;
        let outline = &d.outline;
        // A 3-D bevel: the face and a band inside the outline lit as surfaces.
        if let Some(bv) = effects.as_ref().and_then(|e| e.raw3d.as_deref()).and_then(bevel::from_raw) {
            let bv = bevel::Bevel { w: bv.w * pt, h: bv.h * pt, ..bv };
            let solid = match &fill {
                Some(Fill::Solid { color: c }) => Some(rctx.color(c, fill_ph)),
                _ => None,
            };
            let filled = is_pic || fill.as_ref().is_some_and(|f| !f.is_none());
            if filled {
                let mut area = BezPath::new();
                for sp in geo.paths.iter().filter(|sp| sp.fill != FillMode::None) {
                    area.extend(sp.path.iter());
                }
                if is_pic || area.elements().is_empty() {
                    area = outline.clone();
                }
                bevel::draw(ctx, &bv, if is_pic { None } else { solid }, &area, Rect::new(0.0, 0.0, w, h), m, 1.0);
            }
        }
    }

    fn draw_outline(&mut self, ctx: &mut RenderContext, d: &ShapeDraw) {
        let m = d.m;
        let w = d.w;
        let h = d.h;
        let depth = d.depth;
        let line_ph = d.line_ph;
        let is_pic = d.is_pic;
        let s = d.s;
        let rctx = d.rctx;
        let geo = &d.geo;
        let effects = &d.effects;
        let outline = &d.outline;
        let line = d.line.clone();
        let fade = |i: usize| d.fade(i);
        // Some presets' outlines default to sharp corners (PowerPoint's own per-shape default).
        let line = match (&s.geom, line.join) {
            (Geom::Preset { name, .. }, None) if preset::mitred_by_default(name) => {
                Line { join: Some(deckcraft_model::style::LineJoin::Miter), ..line }
            }
            _ => line,
        };
        // Outline.
        if line.fill.as_ref().is_some_and(|f| !f.is_none()) {
            let lw = line.width.unwrap_or(0.75).max(0.0);
            let (base, rel) = self.line_space(m, depth);
            // Arrowheads go on lines, arcs and open freeforms (filled or not), never on closed
            // outlines or on presets like brackets.
            let ends = s.is_line()
                || geo.is_open()
                || (matches!(s.geom, Geom::Custom { .. })
                    && geo.paths.iter().find(|p| p.stroke).is_some_and(|p| !p.path.elements().iter().any(|e| matches!(e, PathEl::ClosePath))));
            let trims = if ends { head_trims(&line, lw) } else { (0.0, 0.0) };
            let has_heads = ends && [&line.head, &line.tail].iter().any(|e| e.as_ref().is_some_and(|e| e.kind != "none"));
            // A see-through line and its arrowheads are one shape in PowerPoint: drawn opaque into
            // a layer that is then faded, so they don't darken where they meet.
            let see_through = match &line.fill {
                Some(Fill::Solid { color: c }) if has_heads => Some(rctx.color(c, line_ph)).filter(|c| c.a < 255),
                _ => None,
            };
            let line = match see_through {
                Some(c) => {
                    ctx.set_transform(Affine::IDENTITY);
                    ctx.push_layer(None, None, Some(c.a as f32 / 255.0), None, None);
                    std::borrow::Cow::Owned(Line {
                        fill: Some(Fill::solid(deckcraft_model::style::ColorRef::rgb(c.with_alpha(255)))),
                        ..line.clone()
                    })
                }
                None => std::borrow::Cow::Borrowed(&line),
            };
            // A picture's border lies outside the picture, as PowerPoint draws it: stroked twice
            // as wide, outside the outline only (see `border_outside`).
            let outside = border_outside(is_pic, effects.as_ref()).then(|| outside_line(&line, lw)).flatten();
            let clipped = outside.is_some();
            let (line, lw) = match outside {
                Some(o) => {
                    let mut clip = Rect::new(0.0, 0.0, w, h).inflate(w + h + lw * 8.0, w + h + lw * 8.0).to_path(0.1);
                    clip.extend(outline.iter());
                    ctx.set_transform(m);
                    ctx.set_fill_rule(peniko::Fill::EvenOdd);
                    ctx.push_clip_layer(&clip);
                    ctx.set_fill_rule(peniko::Fill::NonZero);
                    (std::borrow::Cow::Owned(o), lw * 2.0)
                }
                None => (line, lw),
            };
            let first_stroked = geo.paths.iter().position(|p| p.stroke);
            for (i, sp) in geo.paths.iter().enumerate().filter(|(i, p)| p.stroke && fade(*i).stroke > 0.0) {
                let mut path = if rel == Affine::IDENTITY {
                    std::borrow::Cow::Borrowed(&sp.path)
                } else {
                    let mut moved = sp.path.clone();
                    moved.apply_affine(rel);
                    std::borrow::Cow::Owned(moved)
                };
                // The line stops inside its arrowheads, so a wide line doesn't poke out of them.
                let whole = path.clone();
                if Some(i) == first_stroked && trims != (0.0, 0.0) {
                    path = std::borrow::Cow::Owned(trim_ends(&path, trims.0, trims.1));
                }
                stroke(ctx, rctx, &line, line_ph, &path, Some(&whole), base, lw, fade(i).stroke);
            }
            if ends {
                arrowheads(ctx, rctx, &line, line_ph, geo, rel, base, lw);
            }
            if clipped {
                ctx.pop_layer();
            }
            if see_through.is_some() {
                ctx.pop_layer();
            }
        }
    }

    fn draw_feather(&mut self, ctx: &mut RenderContext, d: &ShapeDraw, r: f64) {
        let m = d.m;
        let outline = &d.outline;
        // Feather: the shape (fill and line) fades to nothing at its outline. Measured on
        // PowerPoint's export: the mask is the outline shrunk by 0.85 r, blurred by a third of
        // r, so it's half opaque 0.85 r inside the edge and transparent at the edge itself.
        ctx.set_transform(m);
        ctx.push_layer(
            None,
            Some(peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::DestIn)),
            None,
            None,
            Some(blur(r / 3.0 * scale_of(m))),
        );
        ctx.set_paint(peniko::Color::from_rgba8(0, 0, 0, 255));
        ctx.fill_path(outline);
        ctx.push_blend_layer(peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::DestOut));
        ctx.set_stroke(kurbo::Stroke::new(r * 1.7).with_join(kurbo::Join::Round));
        ctx.stroke_path(outline);
        ctx.pop_layer();
        ctx.pop_layer();
        ctx.pop_layer();
    }

    fn draw_inner_shadow(&mut self, ctx: &mut RenderContext, d: &ShapeDraw) {
        let m = d.m;
        let w = d.w;
        let h = d.h;
        let fx_ph = d.fx_ph;
        let rctx = d.rctx;
        let effects = &d.effects;
        let outline = &d.outline;
        // Inner shadow.
        if let Some(e) = &effects
            && let Some(sh) = &e.inner_shadow
        {
            ctx.set_transform(m);
            ctx.push_clip_layer(outline);
            let c = rctx.color(&sh.color, fx_ph);
            let d = Vec2::new(sh.dir.to_radians().cos(), sh.dir.to_radians().sin()) * sh.dist;
            // As PowerPoint draws it (measured on its export): the shadow falls inside along the
            // edges it points to (45 degrees: the bottom and right), blurred by a third of the
            // blur radius.
            ctx.push_layer(None, None, None, None, Some(blur(sh.blur / 3.0 * scale_of(m))));
            ctx.set_paint(color(c, 1.0));
            let mut ring = Rect::new(0.0, 0.0, w, h).inflate(w.max(h), w.max(h)).to_path(0.1);
            let mut hole = outline.clone();
            hole.apply_affine(Affine::translate(-d));
            ring.extend(hole.iter());
            ctx.set_fill_rule(peniko::Fill::EvenOdd);
            ctx.fill_path(&ring);
            ctx.set_fill_rule(peniko::Fill::NonZero);
            ctx.pop_layer();
            ctx.pop_layer();
        }
    }

    fn draw_text_body(&mut self, ctx: &mut RenderContext, d: &ShapeDraw) {
        let m = d.m;
        let w = d.w;
        let h = d.h;
        let depth = d.depth;
        let empty_ph = d.empty_ph;
        let s = d.s;
        let f = d.f;
        let rctx = d.rctx;
        let st = d.st;
        let geo = &d.geo;
        // Text (or the placeholder prompt in the editor).
        let skip_text = f.opts.skip_text == Some(s.id);
        if let Some(body) = &s.text
            && !skip_text
        {
            // Text in a group is set at the shape's size on the slide, in points: a group's
            // child-coordinate scale sizes and places the box, never the type (#233's twin).
            let (tw, th, m) = self.text_space(m, depth, w, h);
            let text_geo;
            let geo = if (tw, th) == (w, h) {
                geo
            } else {
                text_geo = shape_geometry(s, tw, th);
                &text_geo
            };
            let tr = geo.text_rect;
            let editing = f.opts.edit && f.opts.editing_text == Some(s.id);
            if !body.is_empty() || editing {
                // Imported placeholders can have no paragraph until the first keystroke.
                let mut empty;
                let body = if editing && body.paragraphs.is_empty() {
                    empty = body.clone();
                    empty.paragraphs.push(Default::default());
                    &empty
                } else {
                    body
                };
                let mut l = deckcraft_text::layout(rctx, s, body, &Opts { rect: tr, fields: &f.fields, prompt_color: None, no_shrink: false });
                if editing {
                    preview_empty_bullets(&mut l);
                }
                let tm = text_transform(m, tr, l.rotation);
                for b in l.lines.iter().filter_map(line_bounds) {
                    // Glyphs reach a little past their carets and line box (italics, swashes).
                    let pad = b.height() * 0.3;
                    self.track(tm, b.inflate(pad, pad));
                }
                match text_warp(rctx, s, tr).filter(|_| l.rotation == 0.0) {
                    Some(w) => warp::draw(ctx, &l, tr, m, &st.paras, &w),
                    None => draw_layout_with(ctx, &l, tr, m, &st.paras),
                }
            } else if f.opts.edit && s.ph.is_some() {
                let kind = s.ph_type().unwrap_or(PhType::Body);
                let prompt_body = prompt_body(rctx, s, kind);
                if !prompt_body.is_empty() {
                    let grey = Rgba::rgb(0x59, 0x59, 0x59);
                    draw_text(ctx, rctx, s, &prompt_body, m, &Opts { rect: tr, fields: &f.fields, prompt_color: Some(grey), no_shrink: false });
                }
            }
        }
        if empty_ph && f.opts.edit {
            // Dashed placeholder outline.
            ctx.set_transform(m);
            let px = 1.0 / scale_of(m).max(1e-6);
            ctx.set_paint(peniko::Color::from_rgba8(0xA6, 0xA6, 0xA6, 255));
            ctx.set_stroke(kurbo::Stroke::new(px).with_dashes(0.0, [3.0 * px, 2.0 * px]));
            ctx.stroke_path(&Rect::new(0.0, 0.0, w, h).to_path(0.1));
            if matches!(s.ph_type(), Some(PhType::Picture | PhType::Chart | PhType::Table | PhType::Media | PhType::Diagram)) {
                paint::placeholder_icon(ctx, s.ph_type().unwrap_or(PhType::Picture), Rect::new(0.0, 0.0, w, h));
            }
        }
    }

    fn effects_below(&mut self, ctx: &mut RenderContext, rctx: &Ctx, e: &Effects, ph: Option<Rgba>, src: FxSource) {
        let FxSource { geo, fill, fill_alpha, outline, content, m, w, h } = src;
        let silhouette = {
            let mut p = BezPath::new();
            for sp in &geo.paths {
                if sp.fill != FillMode::None || fill.is_none() {
                    p.extend(sp.path.iter());
                }
            }
            if p.elements().is_empty() { geo.outline() } else { p }
        };
        let k = scale_of(m);
        if let Some(sh) = &e.outer_shadow {
            let c = rctx.color(&sh.color, ph);
            let d = Vec2::new(sh.dir.to_radians().cos(), sh.dir.to_radians().sin()) * sh.dist;
            let local = shadow_local(sh, w, h);
            // Shadow offset is in slide space (doesn't rotate with the shape unless asked).
            let (own, pre) = if sh.rotate_with_shape { (Affine::translate(d) * local, Affine::IDENTITY) } else { (local, Affine::translate(d * k)) };
            let sm = pre * m * own;
            ctx.set_transform(Affine::IDENTITY);
            // Blurred by a third of the blur radius, as PowerPoint's export shows.
            ctx.push_layer(None, None, Some((c.a as f32) / 255.0), None, Some(blur(sh.blur / 3.0 * k)));
            ctx.set_transform(sm);
            match content {
                // As PowerPoint draws it: the shadow is the picture's own coverage (a logo on a
                // transparent background casts the logo's shadow): the picture, clipped to the
                // shape, recoloured to the shadow colour.
                Some(pf) => {
                    ctx.push_clip_layer(&silhouette);
                    paint::picture(ctx, rctx.pres, pf, Rect::new(0.0, 0.0, w, h), sm, None);
                    // Drawing the picture leaves its own pixel transform set: back to the shape's,
                    // or the recolouring below covers only part of a large picture's shadow.
                    ctx.set_transform(sm);
                    ctx.push_layer(None, Some(peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcIn)), None, None, None);
                    ctx.set_paint(color(c.with_alpha(255), 1.0));
                    ctx.fill_rect(&Rect::new(0.0, 0.0, w, h).inflate(1.0, 1.0));
                    ctx.pop_layer();
                    ctx.pop_layer();
                }
                // The shadow is cast by what is drawn, as PowerPoint casts it: the filled part as
                // see-through as its fill (none for an outline-only shape) …
                None => {
                    if let Some(a) = fill_alpha {
                        ctx.set_paint(color(c.with_alpha(255), a));
                        // Each subpath under its own fill rule, as the shape itself is
                        // drawn: one merged path filled NonZero fills the hole of an
                        // even-odd shape (a donut, a frame) (#1596).
                        for sp in geo.paths.iter().filter(|sp| sp.fill != FillMode::None) {
                            ctx.set_fill_rule(if sp.even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
                            ctx.fill_path(&sp.path);
                        }
                        ctx.set_fill_rule(peniko::Fill::NonZero);
                    }
                }
            }
            // … and the outline (a picture's border too).
            if let Some(o) = &outline {
                let cast = Line { fill: Some(Fill::solid(deckcraft_model::style::ColorRef::rgb(c.with_alpha(255)))), ..o.line.as_ref().clone() };
                let lw = o.line.width.unwrap_or(0.75).max(0.0);
                for sp in geo.paths.iter().filter(|sp| sp.stroke) {
                    let mut path = sp.path.clone();
                    path.apply_affine(o.rel * own);
                    stroke(ctx, rctx, &cast, o.ph, &path, None, pre * o.base, lw, o.alpha);
                }
            }
            ctx.pop_layer();
        }
        // A shape with no fill and no line (a text box) draws nothing to glow around: its text's
        // own glow is all PowerPoint shows, not a halo round the box.
        let glows = fill_alpha.is_some() || content.is_some() || outline.is_some();
        if let Some(g) = e.glow.as_ref().filter(|_| glows) {
            let c = rctx.color(&g.color, ph);
            ctx.set_transform(Affine::IDENTITY);
            // Measured on PowerPoint's export: a glow of radius r is half strength about half of r
            // out from the outline and gone by r.
            ctx.push_layer(None, None, Some((c.a as f32) / 255.0), None, Some(blur(g.radius * 0.17 * k)));
            ctx.set_transform(m);
            ctx.set_paint(color(c.with_alpha(255), 1.0));
            ctx.set_stroke(kurbo::Stroke::new(g.radius.max(0.0) * 0.95).with_join(kurbo::Join::Round));
            ctx.stroke_path(&silhouette);
            // A shape without a fill (an outline, a frame) glows along its line only (#225).
            if fill_alpha.is_some() || content.is_some() {
                // Per subpath under its own fill rule, as in the shadow above: the
                // merged silhouette filled NonZero fills an even-odd hole (#1596).
                // When no subpath takes part the silhouette is the outline fallback.
                let mut filled_any = false;
                for sp in geo.paths.iter().filter(|sp| sp.fill != FillMode::None || fill.is_none()) {
                    ctx.set_fill_rule(if sp.even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
                    ctx.fill_path(&sp.path);
                    filled_any = true;
                }
                if !filled_any {
                    ctx.fill_path(&silhouette);
                }
                ctx.set_fill_rule(peniko::Fill::NonZero);
            }
            ctx.pop_layer();
        }
        if let Some(rf) = &e.reflection {
            // Mirror below the shape, fading out.
            let mm = m * Affine::translate((0.0, 2.0 * h + rf.dist)) * Affine::scale_non_uniform(1.0, -1.0);
            ctx.set_transform(Affine::IDENTITY);
            ctx.push_layer(None, None, Some(rf.start_alpha.clamp(0.0, 1.0) as f32 * 0.6), None, Some(blur(rf.blur * 0.5 * k)));
            ctx.set_transform(mm);
            match content {
                // A picture reflects its pixels (it has no fill of its own).
                Some(pf) => {
                    ctx.push_clip_layer(&silhouette);
                    paint::picture(ctx, rctx.pres, pf, Rect::new(0.0, 0.0, w, h), mm, None);
                    ctx.pop_layer();
                }
                None => {
                    if let Some(fl) = fill {
                        // Per subpath under its own fill rule, as in the shadow above:
                        // the merged silhouette filled NonZero fills an even-odd hole
                        // (#1596). The outline fallback covers shapes with no
                        // participating subpath.
                        let mut filled_any = false;
                        for sp in geo.paths.iter().filter(|sp| sp.fill != FillMode::None || fill.is_none()) {
                            ctx.set_fill_rule(if sp.even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
                            paint::fill_path(ctx, rctx, fl, ph, &sp.path, Rect::new(0.0, 0.0, w, h), mm, FillMode::Norm, 1.0, rctx.pres);
                            filled_any = true;
                        }
                        if !filled_any {
                            paint::fill_path(ctx, rctx, fl, ph, &silhouette, Rect::new(0.0, 0.0, w, h), mm, FillMode::Norm, 1.0, rctx.pres);
                        }
                        ctx.set_fill_rule(peniko::Fill::NonZero);
                    }
                }
            }
            ctx.set_transform(mm);
            // Fade mask.
            ctx.push_layer(None, Some(peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::DestIn)), None, None, None);
            let grad = peniko::Gradient::new_linear(Point::new(0.0, h), Point::new(0.0, h * (1.0 - rf.end_pos.clamp(0.01, 1.0))))
                .with_stops([(0.0, peniko::Color::from_rgba8(0, 0, 0, 255)), (1.0, peniko::Color::from_rgba8(0, 0, 0, 0))].as_slice());
            ctx.set_paint(grad);
            ctx.fill_rect(&Rect::new(0.0, 0.0, w, h).inflate(4.0, 4.0));
            ctx.pop_layer();
            ctx.pop_layer();
        }
    }
}
