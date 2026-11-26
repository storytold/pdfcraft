//! The pasteboard: what the editor draws around the slide.
//!
//! PowerPoint's Normal view draws shapes, pictures and text that sit partly or wholly off the slide
//! on the area around it, at full strength and with their effects (master and layout graphics
//! too), so they can be seen and grabbed. Slide shows, exports and thumbnails clip to the slide.
//!
//! The editor renders only what it needs: the slide, grown to take in the drawing that spills off
//! it and is in view ([`pasteboard_region`]). A slide with nothing off it costs what it always did.

use deckcraft_geom::Xfrm;
use deckcraft_model::resolve::{self, Ctx};
use deckcraft_model::style::{Effects, Line};
use deckcraft_model::{Presentation, Shape, ShapeKind};
use kurbo::Rect;

use crate::MAX_SIDE;

/// Largest distance (points) a shape's outline or effects are taken to reach beyond its box:
/// hostile files can't blow the pasteboard up.
const MAX_SPILL: f64 = 2000.0;

/// The visible area is rounded out to tiles of this many pixels, so scrolling over a pasteboard
/// that is wider than the window re-renders only when a new tile comes into view.
pub const TILE: i64 = 256;

/// How far (points) a shape's outline and effects reach beyond its box: half the outline and its
/// arrowheads, the outer shadow (offset, blur and scale), the glow and the reflection.
pub(crate) fn spill(line: Option<&Line>, effects: Option<&Effects>, w: f64, h: f64) -> f64 {
    let fin = |v: f64| if v.is_finite() { v.abs() } else { 0.0 };
    let mut s: f64 = 0.0;
    if let Some(l) = line
        && l.fill.as_ref().is_some_and(|f| !f.is_none())
    {
        // Arrowheads reach up to a few line widths past the end point.
        s = s.max(fin(l.width.unwrap_or(0.75)) * 3.0 + 1.0);
    }
    if let Some(e) = effects {
        let big = fin(w).max(fin(h));
        if let Some(sh) = &e.outer_shadow {
            let grow = (fin(sh.sx).max(fin(sh.sy)) - 1.0).max(0.0) * big;
            s = s.max(fin(sh.dist) + fin(sh.blur) + grow);
        }
        if let Some(g) = &e.glow {
            s = s.max(fin(g.radius));
        }
        if let Some(r) = &e.reflection {
            s = s.max(fin(h) + fin(r.dist));
        }
    }
    s.min(MAX_SPILL)
}

/// Slide-space box a top-level shape draws into, its spill included.
fn shape_box(rctx: &Ctx, s: &Shape) -> Option<Rect> {
    if s.hidden {
        return None;
    }
    let x: Xfrm = resolve::xfrm(rctx, s);
    if !x.is_finite() {
        return None;
    }
    let b = match &s.kind {
        ShapeKind::Ink { strokes } => {
            let mut pts = strokes.iter().flat_map(|k| k.points.iter()).filter(|(px, py, _)| px.is_finite() && py.is_finite());
            let (px, py, _) = pts.next()?;
            let r = pts.fold(Rect::new(*px, *py, *px, *py), |r, (px, py, _)| r.union_pt(kurbo::Point::new(*px, *py)));
            let w = strokes.iter().map(|k| k.width).filter(|w| w.is_finite()).fold(0.0_f64, f64::max).min(MAX_SPILL);
            return Some(r.inflate(w, w));
        }
        _ => x.bounds(),
    };
    let (line, _) = resolve::line(rctx, s);
    let (fx, _) = resolve::effects(rctx, s);
    let d = spill(Some(&line), fx.as_ref(), x.w, x.h);
    Some(b.inflate(d, d))
}

fn union_boxes<'a>(rctx: &Ctx, shapes: impl IntoIterator<Item = &'a Shape>, into: Rect) -> Rect {
    shapes.into_iter().filter_map(|s| shape_box(rctx, s)).filter(|r| r.is_finite()).fold(into, |a, r| a.union(r))
}

fn slide_rect(pres: &Presentation) -> Rect {
    Rect::new(0.0, 0.0, pres.slide_size.width, pres.slide_size.height)
}

/// What slide `index` draws, in slide space: at least the slide, grown by every shape (and master
/// and layout graphic) that reaches off it. Estimated from the shapes' boxes, outlines and effects;
/// text spilling out of its box is found while rendering ([`crate::render_slide_drawn`]).
pub fn slide_extent(pres: &Presentation, index: usize) -> Rect {
    let page = slide_rect(pres);
    let Some(slide) = pres.slides.get(index) else { return page };
    let Some(rctx) = Ctx::for_slide(pres, slide) else { return page };
    let mut r = union_boxes(&rctx, &slide.shapes, page);
    let (show_layout, show_master) = resolve::show_master_shapes(slide, rctx.layout);
    if show_master {
        let mctx = Ctx::for_master(pres, rctx.master);
        r = union_boxes(&mctx, rctx.master.shapes.iter().filter(|s| s.ph.is_none()), r);
    }
    if show_layout && let Some(l) = rctx.layout {
        let lctx = Ctx::for_layout(pres, rctx.master, l);
        r = union_boxes(&lctx, l.shapes.iter().filter(|s| s.ph.is_none()), r);
    }
    r
}

/// What a layout (or the master, `layout: None`) draws in Slide Master view, in slide space.
pub fn layout_extent(pres: &Presentation, master: usize, layout: Option<usize>) -> Rect {
    let page = slide_rect(pres);
    let Some(m) = pres.masters.get(master) else { return page };
    match layout.and_then(|i| m.layouts.get(i)) {
        Some(l) => {
            let mut r = union_boxes(&Ctx::for_layout(pres, m, l), &l.shapes, page);
            if l.show_master_shapes {
                r = union_boxes(&Ctx::for_master(pres, m), m.shapes.iter().filter(|s| s.ph.is_none()), r);
            }
            r
        }
        None => union_boxes(&Ctx::for_master(pres, m), &m.shapes, page),
    }
}

/// A rectangle of pixels on the editor canvas, relative to the slide's top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Region {
    /// The slide itself, `w` × `h` pixels.
    pub fn slide(w: u32, h: u32) -> Self {
        Region { x: 0, y: 0, w: w.max(1), h: h.max(1) }
    }

    fn edges(&self) -> (i64, i64, i64, i64) {
        let (x, y) = (self.x as i64, self.y as i64);
        (x, y, x + self.w as i64, y + self.h as i64)
    }

    fn from_edges(x0: i64, y0: i64, x1: i64, y1: i64) -> Self {
        let clamp = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let side = |a: i64, b: i64| (b - a).clamp(1, MAX_SIDE as i64) as u32;
        Region { x: clamp(x0), y: clamp(y0), w: side(x0, x1), h: side(y0, y1) }
    }

    /// Does this region cover all of `other`?
    pub fn contains(&self, other: &Region) -> bool {
        let (a, b) = (self.edges(), other.edges());
        a.0 <= b.0 && a.1 <= b.1 && a.2 >= b.2 && a.3 >= b.3
    }

    /// Is the region more than the slide (some of the pasteboard is drawn)?
    pub fn beyond(&self, slide: (u32, u32)) -> bool {
        *self != Region::slide(slide.0, slide.1)
    }

    /// Render options' `offset` that puts the slide's top-left corner where it falls in the region.
    pub fn offset(&self) -> (f64, f64) {
        (-(self.x as f64), -(self.y as f64))
    }

    /// The region's size, for render options' `size`.
    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
}

/// `r` (slide space) in pixels at `scale` pixels per point, rounded outwards; `None` when not finite.
fn to_px(r: Rect, scale: f64) -> Option<(i64, i64, i64, i64)> {
    let lim = i32::MAX as f64 / 2.0;
    let px = |v: f64| if v.is_finite() { Some(v.clamp(-lim, lim)) } else { None };
    Some((
        px((r.x0 * scale).floor())? as i64,
        px((r.y0 * scale).floor())? as i64,
        px((r.x1 * scale).ceil())? as i64,
        px((r.y1 * scale).ceil())? as i64,
    ))
}

/// The pixels the editor renders: the slide (`slide_px`, its size in pixels at `scale` pixels per
/// point) plus the part of `extent` (slide space, from [`slide_extent`] or what a render drew) that
/// falls within `visible`, the canvas's area relative to the slide's top-left corner. The visible
/// area is rounded out to whole [`TILE`]s.
pub fn pasteboard_region(slide_px: (u32, u32), scale: f64, extent: Rect, visible: Region) -> Region {
    let page = Region::slide(slide_px.0, slide_px.1);
    if !(scale.is_finite() && scale > 0.0) {
        return page;
    }
    let Some((ex0, ey0, ex1, ey1)) = to_px(extent, scale) else { return page };
    let (vx0, vy0, vx1, vy1) = visible.edges();
    let (vx0, vy0) = (vx0.div_euclid(TILE) * TILE, vy0.div_euclid(TILE) * TILE);
    let (vx1, vy1) = ((vx1 + TILE - 1).div_euclid(TILE) * TILE, (vy1 + TILE - 1).div_euclid(TILE) * TILE);
    let (cx0, cy0, cx1, cy1) = (ex0.max(vx0), ey0.max(vy0), ex1.min(vx1), ey1.min(vy1));
    if cx0 >= cx1 || cy0 >= cy1 {
        return page;
    }
    let (px0, py0, px1, py1) = page.edges();
    Region::from_edges(cx0.min(px0), cy0.min(py0), cx1.max(px1), cy1.max(py1))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use deckcraft_model::Shape;

    fn black() -> deckcraft_model::ColorRef {
        deckcraft_model::ColorRef::rgb(deckcraft_color::Rgba::BLACK)
    }

    fn deck_with(x: f64, y: f64, w: f64, h: f64) -> Presentation {
        let mut p = Presentation::default();
        let sl = std::sync::Arc::make_mut(&mut p.slides[0]);
        sl.shapes.push(Shape { id: deckcraft_model::ShapeId(900), xfrm: Some(Xfrm::new(x, y, w, h)), ..Default::default() });
        p
    }

    #[test]
    fn a_slide_with_nothing_off_it_extends_no_further_than_the_slide() {
        let p = deck_with(100.0, 100.0, 50.0, 50.0);
        assert_eq!(slide_extent(&p, 0), Rect::new(0.0, 0.0, p.slide_size.width, p.slide_size.height));
    }

    #[test]
    fn shapes_off_the_slide_grow_the_extent() {
        let p = deck_with(-200.0, 50.0, 100.0, 80.0);
        let e = slide_extent(&p, 0);
        assert!(e.x0 <= -200.0 && e.x0 > -220.0, "{e:?}");
        assert_eq!(e.y0, 0.0);
        assert_eq!(e.x1, p.slide_size.width);
        // Missing slides are just the page.
        assert_eq!(slide_extent(&p, 7), Rect::new(0.0, 0.0, p.slide_size.width, p.slide_size.height));
    }

    #[test]
    fn hidden_shapes_and_hostile_boxes_do_not_grow_it() {
        let mut p = deck_with(-5000.0, 0.0, 10.0, 10.0);
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes.last_mut().unwrap().hidden = true;
        let page = Rect::new(0.0, 0.0, p.slide_size.width, p.slide_size.height);
        assert_eq!(slide_extent(&p, 0), page);
        let p = deck_with(f64::NAN, 0.0, 10.0, 10.0);
        assert_eq!(slide_extent(&p, 0), page);
        let p = deck_with(f64::INFINITY, 0.0, 10.0, 10.0);
        assert_eq!(slide_extent(&p, 0), page);
    }

    #[test]
    fn effects_count_towards_the_extent() {
        let mut p = deck_with(10.0, -100.0, 100.0, 50.0);
        let sh = std::sync::Arc::make_mut(&mut p.slides[0]).shapes.last_mut().unwrap();
        sh.effects = Some(Effects {
            outer_shadow: Some(deckcraft_model::style::Shadow {
                color: black(),
                blur: 10.0,
                dist: 20.0,
                dir: 270.0,
                inner: false,
                sx: 1.0,
                sy: 1.0,
                kx: 0.0,
                ky: 0.0,
                align: String::new(),
                rotate_with_shape: false,
            }),
            ..Default::default()
        });
        assert!(slide_extent(&p, 0).y0 <= -130.0);
        assert_eq!(
            spill(
                None,
                Some(&Effects { glow: Some(deckcraft_model::style::Glow { color: black(), radius: f64::MAX }), ..Default::default() }),
                1.0,
                1.0
            ),
            MAX_SPILL
        );
    }

    #[test]
    fn master_graphics_off_the_slide_count_but_not_master_placeholders() {
        let mut p = Presentation::default();
        let m = std::sync::Arc::make_mut(&mut p.masters[0]);
        m.shapes.push(Shape { id: deckcraft_model::ShapeId(901), xfrm: Some(Xfrm::new(-150.0, 10.0, 100.0, 50.0)), ..Default::default() });
        assert!(slide_extent(&p, 0).x0 <= -150.0);
        assert!(layout_extent(&p, 0, None).x0 <= -150.0);
        assert!(layout_extent(&p, 0, Some(0)).x0 <= -150.0 || !p.masters[0].layouts[0].show_master_shapes);
    }

    #[test]
    fn the_region_is_the_slide_unless_something_off_it_is_in_view() {
        let slide = (960, 540);
        let page = Region::slide(960, 540);
        let view = Region { x: -100, y: -50, w: 1160, h: 640 };
        let whole = Rect::new(0.0, 0.0, 960.0, 540.0);
        assert_eq!(pasteboard_region(slide, 1.0, whole, view), page);
        assert!(!pasteboard_region(slide, 1.0, whole, view).beyond(slide));
        // Off to the left, in view: the region reaches it.
        let r = pasteboard_region(slide, 1.0, Rect::new(-80.0, 0.0, 960.0, 540.0), view);
        assert_eq!((r.x, r.y, r.w, r.h), (-80, 0, 1040, 540));
        assert!(r.beyond(slide) && r.contains(&page));
        assert_eq!(r.offset(), (80.0, 0.0));
        // Far off and out of view (the view rounds out to tiles): only what's visible.
        let r = pasteboard_region(slide, 1.0, Rect::new(-5000.0, 0.0, 960.0, 540.0), view);
        assert_eq!(r.x, -256);
        // Wholly out of view: the slide.
        assert_eq!(pasteboard_region(slide, 1.0, Rect::new(-5000.0, 0.0, -4000.0, 540.0).union(whole), Region { x: 0, y: 0, w: 960, h: 540 }), page);
        // Scale applies; nonsense falls back to the slide.
        let r = pasteboard_region((480, 270), 0.5, Rect::new(-80.0, 0.0, 960.0, 540.0), view);
        assert_eq!(r.x, -40);
        assert_eq!(pasteboard_region(slide, f64::NAN, whole, view), page);
        assert_eq!(pasteboard_region(slide, 1.0, Rect::new(f64::NAN, 0.0, 1.0, 1.0), view), page);
        // Huge extents are capped.
        let r =
            pasteboard_region(slide, 1.0, Rect::new(-1e12, -1e12, 1e12, 1e12), Region { x: -1_000_000, y: -1_000_000, w: 2_000_000, h: 2_000_000 });
        assert!(r.w <= MAX_SIDE && r.h <= MAX_SIDE);
    }

    #[test]
    fn scrolling_within_a_tile_keeps_the_region() {
        let slide = (2000, 1125);
        let e = Rect::new(-500.0, 0.0, 960.0, 540.0);
        let a = pasteboard_region(slide, 2.0, e, Region { x: -300, y: 0, w: 1000, h: 700 });
        let b = pasteboard_region(slide, 2.0, e, Region { x: -290, y: 10, w: 1000, h: 700 });
        assert_eq!(a, b);
    }
}
