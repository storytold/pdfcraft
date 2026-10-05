//! Typed signatures and bounded glyph outlines from approved bundled fonts.

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};

static FONT: &[u8] = include_bytes!("../../../assets/fonts/DancingScript.ttf");

/// The approved Japanese fallback face used by the editor and generated Type 3 fonts.
pub static SHIPPORI_MINCHO: &[u8] = include_bytes!("../../../assets/fonts/ShipporiMincho-Regular.ttf");

/// Outlines of a line of existing text at a font size of 1 (em units).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptOutline {
    pub contours: Vec<Vec<[f64; 2]>>,
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
}

/// One glyph outline in em units, with the baseline at y = 0.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphOutline {
    pub contours: Vec<Vec<[f64; 2]>>,
    pub width: f64,
    pub bbox: [f64; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphError {
    Missing,
    TooComplex,
}

struct Flatten {
    contours: Vec<Vec<[f64; 2]>>,
    cur: Vec<[f64; 2]>,
    scale: f64,
    dx: f64,
    points: usize,
    too_complex: bool,
}

impl Flatten {
    fn new(scale: f64) -> Self {
        Self { contours: Vec::new(), cur: Vec::new(), scale, dx: 0.0, points: 0, too_complex: false }
    }

    fn pt(&self, x: f32, y: f32) -> [f64; 2] {
        [self.dx + x as f64 * self.scale, y as f64 * self.scale]
    }

    fn last(&self) -> [f64; 2] {
        self.cur.last().copied().unwrap_or([self.dx, 0.0])
    }

    fn push(&mut self, p: [f64; 2]) {
        const MAX_POINTS: usize = 4096;
        if self.points >= MAX_POINTS {
            self.too_complex = true;
            return;
        }
        self.points += 1;
        self.cur.push(p);
    }

    fn close(&mut self) {
        if self.cur.len() > 2 {
            self.contours.push(std::mem::take(&mut self.cur));
        } else {
            self.cur.clear();
        }
    }
}

const STEPS: usize = 8;

impl OutlinePen for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.push(self.pt(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push(self.pt(x, y));
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (p0, c, p) = (self.last(), self.pt(cx, cy), self.pt(x, y));
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let u = 1.0 - t;
            self.push([u * u * p0[0] + 2.0 * u * t * c[0] + t * t * p[0], u * u * p0[1] + 2.0 * u * t * c[1] + t * t * p[1]]);
        }
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let (p0, c0, c1, p) = (self.last(), self.pt(c0x, c0y), self.pt(c1x, c1y), self.pt(x, y));
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.push([a * p0[0] + b * c0[0] + c * c1[0] + d * p[0], a * p0[1] + b * c0[1] + c * c1[1] + d * p[1]]);
        }
    }

    fn close(&mut self) {
        Flatten::close(self);
    }
}

/// Return one Shippori Mincho glyph, bounded so hostile replacement text cannot allocate
/// unbounded outline data.
pub fn shippori_glyph(ch: char) -> Result<GlyphOutline, GlyphError> {
    let Ok(font) = FontRef::new(SHIPPORI_MINCHO) else { return Err(GlyphError::Missing) };
    let loc = LocationRef::default();
    let metrics = font.metrics(Size::unscaled(), loc);
    let scale = 1.0 / metrics.units_per_em.max(1) as f64;
    let Some(gid) = font.charmap().map(ch) else { return Err(GlyphError::Missing) };
    let advances = font.glyph_metrics(Size::unscaled(), loc);
    let width = advances.advance_width(gid).unwrap_or(0.0) as f64 * scale;
    if !width.is_finite() || width <= 0.0 || width > 2.0 {
        return Err(GlyphError::Missing);
    }
    let Some(glyph) = font.outline_glyphs().get(gid) else { return Err(GlyphError::Missing) };
    let mut pen = Flatten::new(scale);
    let _ = glyph.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen);
    pen.close();
    if pen.too_complex || pen.contours.len() > 256 {
        return Err(GlyphError::TooComplex);
    }
    let mut bbox = [0.0, 0.0, width, 0.0];
    let mut any = false;
    for p in pen.contours.iter().flatten() {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err(GlyphError::Missing);
        }
        if !any {
            bbox = [p[0], p[1], p[0], p[1]];
            any = true;
        } else {
            bbox[0] = bbox[0].min(p[0]);
            bbox[1] = bbox[1].min(p[1]);
            bbox[2] = bbox[2].max(p[0]);
            bbox[3] = bbox[3].max(p[1]);
        }
    }
    Ok(GlyphOutline { contours: pen.contours, width, bbox })
}

/// The outlines of `text` in the script font (characters it lacks are skipped).
pub fn script_outline(text: &str) -> ScriptOutline {
    let Ok(font) = FontRef::new(FONT) else { return ScriptOutline::default() };
    let loc = LocationRef::default();
    let metrics = font.metrics(Size::unscaled(), loc);
    let scale = 1.0 / metrics.units_per_em.max(1) as f64;
    let charmap = font.charmap();
    let glyphs = font.outline_glyphs();
    let advances = font.glyph_metrics(Size::unscaled(), loc);
    let mut pen = Flatten::new(scale);
    for ch in text.chars() {
        let Some(gid) = charmap.map(ch) else { continue };
        if let Some(g) = glyphs.get(gid) {
            let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen);
            pen.close();
        }
        pen.dx += advances.advance_width(gid).unwrap_or(0.0) as f64 * scale;
    }
    pen.close();
    ScriptOutline { contours: pen.contours, width: pen.dx, ascent: metrics.ascent as f64 * scale, descent: metrics.descent as f64 * scale }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_name_becomes_outlines() {
        let o = super::script_outline("Ada L.");
        assert!(o.contours.len() >= 5, "{}", o.contours.len());
        assert!(o.width > 1.0 && o.width < 6.0, "{}", o.width);
        assert!(o.ascent > 0.5 && o.descent < 0.0);
        let max_x = o.contours.iter().flatten().map(|p| p[0]).fold(0.0, f64::max);
        assert!(max_x <= o.width + 0.2);
        assert!(super::script_outline("").contours.is_empty());
    }

    #[test]
    fn shippori_resolves_japanese_glyphs() {
        let g = super::shippori_glyph('こ').expect("bundled font has Japanese glyph");
        assert!(g.width > 0.2 && g.width < 2.0);
        assert!(!g.contours.is_empty());
        assert_eq!(super::shippori_glyph('\u{1f4a9}'), Err(super::GlyphError::Missing));
    }
}
