//! Colour glyphs (OpenType `COLR` v0 and v1, colours from `CPAL`), as colour emoji fonts draw
//! them: layers of glyph outlines filled with solid colours or gradients.

use deckcraft_color::Rgba;
use deckcraft_fonts::FontFace;
use kurbo::{Affine, BezPath, Point, Rect};
use skrifa::color::{Brush, ColorPainter, ColorStop, CompositeMode, Extend, Transform};
use skrifa::raw::types::BoundingBox;
use skrifa::{GlyphId, MetadataProvider};
use vello_cpu::{RenderContext, peniko};

/// Nesting depth of clips and layers past which a glyph opens no more (hostile fonts).
const MAX_DEPTH: usize = 64;
/// Fills and gradient stops one colour glyph may paint; the rest of a hostile glyph is skipped.
const MAX_WORK: usize = 100_000;

/// Paint `gid` of `face` as a colour glyph, its origin mapped by `at` (y down, points) at `scale`
/// points per font unit. `fg` is the text colour (palette index 0xFFFF) and `alpha` fades it all.
/// False when the glyph has no colour version, so the caller draws the plain outline.
pub fn paint(ctx: &mut RenderContext, face: &FontFace, gid: u32, at: Affine, scale: f64, fg: Rgba, alpha: f64) -> bool {
    let Some(font) = face.skrifa() else { return false };
    let glyphs = font.color_glyphs();
    let Some(glyph) = glyphs.get(GlyphId::new(gid)) else { return false };
    let palette: Vec<Rgba> = font
        .color_palettes()
        .get(0)
        .map(|p| p.colors().iter().map(|c| Rgba { r: c.red, g: c.green, b: c.blue, a: c.alpha }).collect())
        .unwrap_or_default();
    // Font units are y up; the slide is y down.
    let base = at * Affine::scale(scale) * Affine::scale_non_uniform(1.0, -1.0);
    let mut p = Painter { ctx, face, base, stack: vec![Affine::IDENTITY], palette, fg, opened: Vec::new(), work: 0 };
    p.ctx.push_layer(None, None, Some(alpha.clamp(0.0, 1.0) as f32), None, None);
    let ok = glyph.paint(face.location(), &mut p).is_ok();
    // A malformed glyph may leave layers open: close them all.
    while let Some(open) = p.opened.pop() {
        if open {
            p.ctx.pop_layer();
        }
    }
    p.ctx.pop_layer();
    ok
}

struct Painter<'a> {
    ctx: &'a mut RenderContext,
    face: &'a FontFace,
    base: Affine,
    /// Transforms pushed by the glyph (font units), innermost last.
    stack: Vec<Affine>,
    palette: Vec<Rgba>,
    fg: Rgba,
    /// Clips and composite groups pushed by the glyph, innermost last: true when a layer was
    /// opened on the context for it (false past `MAX_DEPTH`, so its pop is skipped too).
    opened: Vec<bool>,
    /// Fills plus gradient stops painted so far, against [`MAX_WORK`].
    work: usize,
}

fn affine(t: &Transform) -> Affine {
    Affine::new([t.xx as f64, t.yx as f64, t.xy as f64, t.yy as f64, t.dx as f64, t.dy as f64])
}

fn pt(p: skrifa::raw::types::Point<f32>) -> Point {
    Point::new(p.x as f64, p.y as f64)
}

fn extend(e: Extend) -> peniko::Extend {
    match e {
        Extend::Repeat => peniko::Extend::Repeat,
        Extend::Reflect => peniko::Extend::Reflect,
        _ => peniko::Extend::Pad,
    }
}

fn blend(m: CompositeMode) -> peniko::BlendMode {
    use peniko::{Compose, Mix};
    let (mix, compose) = match m {
        CompositeMode::Clear => (Mix::Normal, Compose::Clear),
        CompositeMode::Src => (Mix::Normal, Compose::Copy),
        CompositeMode::Dest => (Mix::Normal, Compose::Dest),
        CompositeMode::DestOver => (Mix::Normal, Compose::DestOver),
        CompositeMode::SrcIn => (Mix::Normal, Compose::SrcIn),
        CompositeMode::DestIn => (Mix::Normal, Compose::DestIn),
        CompositeMode::SrcOut => (Mix::Normal, Compose::SrcOut),
        CompositeMode::DestOut => (Mix::Normal, Compose::DestOut),
        CompositeMode::SrcAtop => (Mix::Normal, Compose::SrcAtop),
        CompositeMode::DestAtop => (Mix::Normal, Compose::DestAtop),
        CompositeMode::Xor => (Mix::Normal, Compose::Xor),
        CompositeMode::Plus => (Mix::Normal, Compose::Plus),
        CompositeMode::Screen => (Mix::Screen, Compose::SrcOver),
        CompositeMode::Overlay => (Mix::Overlay, Compose::SrcOver),
        CompositeMode::Darken => (Mix::Darken, Compose::SrcOver),
        CompositeMode::Lighten => (Mix::Lighten, Compose::SrcOver),
        CompositeMode::ColorDodge => (Mix::ColorDodge, Compose::SrcOver),
        CompositeMode::ColorBurn => (Mix::ColorBurn, Compose::SrcOver),
        CompositeMode::HardLight => (Mix::HardLight, Compose::SrcOver),
        CompositeMode::SoftLight => (Mix::SoftLight, Compose::SrcOver),
        CompositeMode::Difference => (Mix::Difference, Compose::SrcOver),
        CompositeMode::Exclusion => (Mix::Exclusion, Compose::SrcOver),
        CompositeMode::Multiply => (Mix::Multiply, Compose::SrcOver),
        CompositeMode::HslHue => (Mix::Hue, Compose::SrcOver),
        CompositeMode::HslSaturation => (Mix::Saturation, Compose::SrcOver),
        CompositeMode::HslColor => (Mix::Color, Compose::SrcOver),
        CompositeMode::HslLuminosity => (Mix::Luminosity, Compose::SrcOver),
        _ => (Mix::Normal, Compose::SrcOver),
    };
    peniko::BlendMode::new(mix, compose)
}

impl Painter<'_> {
    fn current(&self) -> Affine {
        self.base * self.stack.last().copied().unwrap_or(Affine::IDENTITY)
    }

    fn color(&self, index: u16, alpha: f32) -> peniko::Color {
        let c = if index == 0xFFFF { self.fg } else { self.palette.get(index as usize).copied().unwrap_or(self.fg) };
        let a = (c.a as f64 * (alpha as f64).clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u8;
        peniko::Color::from_rgba8(c.r, c.g, c.b, a)
    }

    fn stops(&self, stops: &[ColorStop]) -> Vec<peniko::ColorStop> {
        stops.iter().map(|s| peniko::ColorStop { offset: s.offset.clamp(0.0, 1.0), color: self.color(s.palette_index, s.alpha).into() }).collect()
    }

    /// The glyph outline in font units, y up.
    fn outline(&self, gid: GlyphId) -> BezPath {
        let p = deckcraft_fonts::FontDb::global().outline(self.face, gid.to_u32());
        Affine::scale_non_uniform(1.0, -1.0) * (*p).clone()
    }

    fn push_clip(&mut self, path: &BezPath) {
        let open = self.opened.len() < MAX_DEPTH;
        if open {
            self.ctx.set_transform(self.current());
            self.ctx.push_clip_layer(path);
        }
        self.opened.push(open);
    }

    fn pop(&mut self) {
        if self.opened.pop() == Some(true) {
            self.ctx.pop_layer();
        }
    }

    /// Fill the open clip area (a large box in the current transform) with `paint`.
    fn cover(&mut self, paint: impl Into<vello_cpu::PaintType>) {
        self.ctx.set_transform(self.current());
        self.ctx.set_paint(paint);
        self.ctx.fill_rect(&Rect::new(-1.0e5, -1.0e5, 1.0e5, 1.0e5));
    }
}

impl ColorPainter for Painter<'_> {
    fn push_transform(&mut self, t: Transform) {
        let next = self.stack.last().copied().unwrap_or(Affine::IDENTITY) * affine(&t);
        self.stack.push(next);
    }

    fn pop_transform(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    fn push_clip_glyph(&mut self, gid: GlyphId) {
        let path = self.outline(gid);
        self.push_clip(&path);
    }

    fn push_clip_box(&mut self, b: BoundingBox<f32>) {
        let r = Rect::new(b.x_min as f64, b.y_min as f64, b.x_max as f64, b.y_max as f64);
        self.push_clip(&kurbo::Shape::to_path(&r, 0.1));
    }

    fn pop_clip(&mut self) {
        self.pop();
    }

    fn fill(&mut self, brush: Brush<'_>) {
        let stops = match &brush {
            Brush::Solid { .. } => 0,
            Brush::LinearGradient { color_stops, .. } | Brush::RadialGradient { color_stops, .. } | Brush::SweepGradient { color_stops, .. } => {
                color_stops.len()
            }
        };
        self.work = self.work.saturating_add(1 + stops);
        if self.work > MAX_WORK {
            return;
        }
        match brush {
            Brush::Solid { palette_index, alpha } => {
                let c = self.color(palette_index, alpha);
                self.cover(c);
            }
            Brush::LinearGradient { p0, p1, color_stops, extend: e } => {
                let g = peniko::Gradient::new_linear(pt(p0), pt(p1)).with_stops(self.stops(color_stops).as_slice()).with_extend(extend(e));
                self.cover(g);
            }
            Brush::RadialGradient { c0, r0, c1, r1, color_stops, extend: e } => {
                let g = peniko::Gradient::new_two_point_radial(pt(c0), r0.max(0.0), pt(c1), r1.max(0.0))
                    .with_stops(self.stops(color_stops).as_slice())
                    .with_extend(extend(e));
                self.cover(g);
            }
            Brush::SweepGradient { c0, start_angle, end_angle, color_stops, extend: e } => {
                let g = peniko::Gradient::new_sweep(pt(c0), start_angle.to_radians(), end_angle.to_radians())
                    .with_stops(self.stops(color_stops).as_slice())
                    .with_extend(extend(e));
                self.cover(g);
            }
        }
    }

    fn push_layer(&mut self, mode: CompositeMode) {
        let open = self.opened.len() < MAX_DEPTH;
        if open {
            self.ctx.push_blend_layer(blend(mode));
        }
        self.opened.push(open);
    }

    fn pop_layer(&mut self) {
        self.pop();
    }
}
