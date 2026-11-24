use super::opts::{RenderOpts, SlideFields};
use super::raster::{Image, MAX_SIDE};
use super::renderer::{Frame, RENDERER, Renderer};
use super::{Affine, Ctx, PathMorph, Presentation, Rect, RenderContext, Shape, ShapeId, Slide, TextMorph, resolve};

static PANICS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn note_panic() {
    PANICS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Render panics caught (and drawn as blank) since start; soak tests require it to stay 0.
pub fn panics_caught() -> u64 {
    PANICS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Does anything on this slide (or its layout/master graphics) use blur-based effects?
pub fn needs_filters(pres: &Presentation, slide: &Slide) -> bool {
    let Some(rctx) = Ctx::for_slide(pres, slide) else { return false };
    let mut any = false;
    let mut check = |shapes: &[Shape]| {
        deckcraft_model::walk(shapes, &mut |s, _| {
            if any {
                return;
            }
            let (e, _) = resolve::effects(&rctx, s);
            if e.is_some_and(|e| !e.is_empty()) || text_has_effects(&rctx, s) {
                any = true;
            }
        });
    };
    check(&slide.shapes);
    if let Some(l) = rctx.layout {
        check(&l.shapes);
    }
    check(&rctx.master.shapes);
    any
}

/// Does the shape's text have a shadow or glow (on a run, in its paragraph's `defRPr`, in its list
/// style, or in the master's or presentation's text styles it inherits)? Such text is blurred,
/// which needs filters.
fn text_has_effects(rctx: &Ctx, s: &Shape) -> bool {
    use deckcraft_model::text::{ListStyle, RunProps};
    let fx = |r: &RunProps| r.shadow.as_ref().is_some_and(|s| !s.inner) || r.glow.as_ref().is_some_and(|g| g.radius > 0.0);
    let styled = |l: &ListStyle| l.levels.iter().flatten().any(|lv| fx(&lv.run));
    let Some(t) = &s.text else { return false };
    t.paragraphs.iter().any(|p| p.runs.iter().any(|r| fx(&r.props)) || fx(&p.end_props) || fx(&p.def_props))
        || styled(&t.list_style)
        || styled(&rctx.master.title_style)
        || styled(&rctx.master.body_style)
        || styled(&rctx.master.other_style)
        || styled(&rctx.pres.default_text_style)
}

/// Render slide `index` at `opts.scale` pixels per point. A panic inside the renderer becomes a
/// blank image (logged), never a crash.
pub fn render_slide(pres: &Presentation, index: usize, opts: &RenderOpts) -> Image {
    render_slide_drawn(pres, index, opts).0
}

/// [`render_slide`], also returning how far the drawing reached, in slide space (at least the
/// slide; off it, what the shapes, their effects and their text drew, the parts outside the output
/// included). The editor grows its pasteboard to it ([`pasteboard_region`]).
pub fn render_slide_drawn(pres: &Presentation, index: usize, opts: &RenderOpts) -> (Image, Rect) {
    let page = Rect::new(0.0, 0.0, pres.slide_size.width, pres.slide_size.height);
    let Some(slide) = pres.slides.get(index) else { return (Image::default(), page) };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RENDERER.with(|r| {
            let mut r = r.borrow_mut();
            r.drawn = None;
            let img = r.slide(pres, slide, index, opts);
            (img, r.drawn.take())
        })
    })) {
        Ok((img, drawn)) => {
            let k = opts.scale;
            let to_slide =
                |d: Rect| Rect::new((d.x0 - opts.offset.0) / k, (d.y0 - opts.offset.1) / k, (d.x1 - opts.offset.0) / k, (d.y1 - opts.offset.1) / k);
            let extent = match drawn {
                Some(d) if k.is_finite() && k > 0.0 && to_slide(d).is_finite() => page.union(to_slide(d)),
                _ => page,
            };
            (img, extent)
        }
        Err(_) => {
            log::error!("rendering slide {index} panicked; showing a blank slide");
            note_panic();
            RENDERER.with(|r| *r.borrow_mut() = Renderer::default());
            let (w, h) = output_size(pres, opts);
            (Image { width: w as u32, height: h as u32, pixels: vec![255; w as usize * h as usize * 4] }, page)
        }
    }
}

/// Which part of a slide [`render_slide_part`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlidePart {
    /// The background fill with the master and layout graphics.
    Backdrop,
    /// The slide's own shapes over transparency.
    Content,
}

/// Render one part of slide `index` (transitions that move the contents over the backgrounds).
/// A panic inside the renderer becomes a transparent image (logged), never a crash.
pub fn render_slide_part(pres: &Presentation, index: usize, part: SlidePart, opts: &RenderOpts) -> Image {
    let Some(slide) = pres.slides.get(index) else { return Image::default() };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| RENDERER.with(|r| r.borrow_mut().slide_part(pres, slide, index, part, opts)))) {
        Ok(img) => img,
        Err(_) => {
            log::error!("rendering part of slide {index} panicked; showing nothing");
            note_panic();
            RENDERER.with(|r| *r.borrow_mut() = Renderer::default());
            let (w, h) = output_size(pres, opts);
            Image { width: w as u32, height: h as u32, pixels: vec![0; w as usize * h as usize * 4] }
        }
    }
}

/// Render a frame between slides `old` and `new` (indices into `pres.slides`), e.g. of Morph: the
/// backdrop (background, master and layout graphics) of `old` cross-fading into that of `new` by
/// `mix` (0..1), then `shapes` in order, each resolved against the slide it comes from (`true`:
/// `old`). Pairs of shapes in `text` draw their boxes as usual but their texts morph by words or
/// characters ([`TextMorph`]), on top of the later shape of the pair. Shapes listed in `paths` draw
/// an outline morphing from another geometry into their own ([`PathMorph`]). `opts.state` applies
/// to `shapes` only.
pub fn render_blend(
    pres: &Presentation,
    (old, new): (usize, usize),
    mix: f64,
    shapes: &[(Shape, bool)],
    text: &[TextMorph],
    paths: &[PathMorph],
    opts: &RenderOpts,
) -> Image {
    let (Some(a), Some(b)) = (pres.slides.get(old), pres.slides.get(new)) else { return Image::default() };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RENDERER.with(|r| r.borrow_mut().blend(pres, (a, old), (b, new), mix, shapes, text, paths, opts))
    })) {
        Ok(img) => img,
        Err(_) => {
            log::error!("rendering a frame between slides {old} and {new} panicked; showing the new slide");
            note_panic();
            RENDERER.with(|r| *r.borrow_mut() = Renderer::default());
            render_slide(pres, new, opts)
        }
    }
}

/// Render a layout (Slide Master view thumbnail and canvas).
pub fn render_layout(pres: &Presentation, master: usize, layout: Option<usize>, opts: &RenderOpts) -> Image {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| RENDERER.with(|r| r.borrow_mut().layout(pres, master, layout, opts)))) {
        Ok(img) => img,
        Err(_) => {
            log::error!("rendering a layout panicked; showing a blank image");
            note_panic();
            RENDERER.with(|r| *r.borrow_mut() = Renderer::default());
            Image::default()
        }
    }
}

pub(crate) fn output_size(pres: &Presentation, opts: &RenderOpts) -> (u16, u16) {
    let (w, h) = opts
        .size
        .unwrap_or(((pres.slide_size.width * opts.scale).round().max(1.0) as u32, (pres.slide_size.height * opts.scale).round().max(1.0) as u32));
    (w.clamp(1, MAX_SIDE) as u16, h.clamp(1, MAX_SIDE) as u16)
}

/// Render `ids` together onto a transparent canvas, in their slide positions (copy as a picture).
pub fn render_shapes(pres: &Presentation, slide: &Slide, ids: &[ShapeId], scale: f64) -> Option<Image> {
    if ids.is_empty() || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let rctx = Ctx::for_slide(pres, slide)?;
    let mut items = Vec::new();
    let mut bounds: Option<Rect> = None;
    for id in ids.iter().take(256) {
        let Some(s) = slide.shape(*id) else { continue };
        let b = resolve::xfrm(&rctx, s).bounds();
        if !b.width().is_finite() || !b.height().is_finite() || b.width() < 0.0 || b.height() < 0.0 {
            continue;
        }
        bounds = Some(match bounds {
            Some(u) => u.union(b),
            None => b,
        });
        items.push(s);
    }
    let b = bounds?.inflate(8.0, 8.0);
    if !(b.width() > 0.0 && b.height() > 0.0 && b.width().is_finite() && b.height().is_finite()) {
        return None;
    }
    let fit = (2048.0 / b.width().max(b.height())).min(scale);
    if !(fit.is_finite() && fit > 0.0) {
        return None;
    }
    let w = (b.width() * fit).ceil().clamp(1.0, 2048.0) as u16;
    let h = (b.height() * fit).ceil().clamp(1.0, 2048.0) as u16;
    let mut ctx = RenderContext::new_with(w, h, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
    let opts = RenderOpts { scale: fit, ..Default::default() };
    let f = Frame { pres, opts: &opts, fields: SlideFields::new(1, String::new(), &opts) };
    let view = Affine::scale(fit) * Affine::translate(-b.origin().to_vec2());
    RENDERER.with(|r| {
        let mut r = r.borrow_mut();
        for s in items {
            r.shape(&mut ctx, &f, &rctx, s, view, 0);
        }
        Some(r.finish(ctx, w, h))
    })
}

/// Render one shape alone onto a transparent canvas (drag previews, copy as picture).
pub fn render_shape(pres: &Presentation, slide: &Slide, id: ShapeId, scale: f64) -> Option<Image> {
    let rctx = Ctx::for_slide(pres, slide)?;
    let s = slide.shape(id)?;
    let x = resolve::xfrm(&rctx, s);
    let b = x.bounds().inflate(8.0, 8.0);
    let w = (b.width() * scale).ceil().clamp(1.0, MAX_SIDE as f64) as u16;
    let h = (b.height() * scale).ceil().clamp(1.0, MAX_SIDE as f64) as u16;
    let mut ctx = RenderContext::new_with(w, h, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
    let opts = RenderOpts { scale, ..Default::default() };
    let f = Frame { pres, opts: &opts, fields: SlideFields::new(1, String::new(), &opts) };
    let view = Affine::scale(scale) * Affine::translate(-b.origin().to_vec2());
    RENDERER.with(|r| {
        let mut r = r.borrow_mut();
        r.shape(&mut ctx, &f, &rctx, s, view, 0);
        Some(r.finish(ctx, w, h))
    })
}
