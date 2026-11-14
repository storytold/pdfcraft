// PdfCraft patch: convert one rectangle of a page (`convert_region`), for copying an area as
// vectors. The whole-page `convert` is unchanged.

use crate::SvgRenderer;
use hayro_interpret::font::Glyph;
use hayro_interpret::{GlyphDrawMode, Image, PathDrawMode, StrokeProps};
use kurbo::{Affine, BezPath, Rect, Shape};

/// The id of the root clip that hides what lies outside the region.
const REGION_CLIP_ID: &str = "pdfcraft-region";

/// Whether `bbox` (device space) misses `region` entirely. A box with a NaN side misses: what
/// it bounds draws nothing.
pub(crate) fn misses(region: Rect, bbox: Rect) -> bool {
    !(bbox.x1 >= region.x0 && bbox.x0 <= region.x1 && bbox.y1 >= region.y0 && bbox.y0 <= region.y1)
}

/// How far a stroke may reach past its path: half the line width, times the miter limit for
/// sharp joins (at least √2 for square caps), so the box only ever over-covers.
fn stroke_reach(s: &StrokeProps) -> f64 {
    let w = f64::from(s.line_width.abs());
    let miter = f64::from(s.miter_limit).max(std::f64::consts::SQRT_2);
    w * 0.5 * miter
}

/// The device-space box of `path` drawn with `transform`, a stroke widened by its reach.
pub(crate) fn path_box(path: &BezPath, transform: Affine, stroke: Option<&StrokeProps>) -> Rect {
    let mut b = path.bounding_box();
    if let Some(s) = stroke {
        let r = stroke_reach(s);
        b = b.inflate(r, r);
    }
    transform.transform_rect_bbox(b)
}

impl SvgRenderer<'_> {
    /// Whether a top-level draw with device-space box `bbox` lies wholly outside the region and
    /// is left out. Draws inside patterns, masks and Type 3 glyph definitions are never culled:
    /// their coordinates are not the page's.
    pub(crate) fn culled(&self, bbox: impl FnOnce() -> Rect) -> bool {
        self.cull.is_some_and(|region| misses(region, bbox()))
    }

    pub(crate) fn path_culled(
        &self,
        path: &BezPath,
        transform: Affine,
        mode: &PathDrawMode,
    ) -> bool {
        let stroke = match mode {
            PathDrawMode::Stroke(s) => Some(s),
            PathDrawMode::Fill(_) => None,
        };
        self.culled(|| path_box(path, transform, stroke))
    }

    /// Outline glyphs are culled by their outline's box; a Type 3 glyph has no cheap box, so it
    /// is kept (and hidden by the root clip).
    pub(crate) fn glyph_culled(
        &self,
        glyph: &Glyph<'_>,
        transform: Affine,
        glyph_transform: Affine,
        mode: &GlyphDrawMode,
    ) -> bool {
        let Glyph::Outline(o) = glyph else {
            return false;
        };
        let stroke = match mode {
            GlyphDrawMode::Stroke(s) => Some(s),
            _ => None,
        };
        self.culled(|| path_box(&o.outline(), transform * glyph_transform, stroke))
    }

    pub(crate) fn image_culled(&self, image: &Image<'_, '_>, transform: Affine) -> bool {
        let unit = Rect::new(
            0.0,
            0.0,
            f64::from(image.width()),
            f64::from(image.height()),
        );
        self.culled(|| transform.transform_rect_bbox(unit))
    }

    /// The root `<svg>` for `region`: its view box, a physical size in points, the clip every
    /// draw sits under, and an opaque background as a `<rect>` (which every editor shows, where
    /// a CSS background is not).
    pub(crate) fn write_region_header(&mut self, region: Rect) {
        let (x, y, w, h) = (
            region.x0 as f32,
            region.y0 as f32,
            region.width() as f32,
            region.height() as f32,
        );
        self.xml.start_element("svg");
        self.xml
            .write_attribute_fmt("viewBox", format_args!("{x} {y} {w} {h}"));
        self.xml.write_attribute_fmt("width", format_args!("{w}pt"));
        self.xml
            .write_attribute_fmt("height", format_args!("{h}pt"));
        self.xml
            .write_attribute("xmlns", "http://www.w3.org/2000/svg");
        self.xml
            .write_attribute("xmlns:xlink", "http://www.w3.org/1999/xlink");
        self.xml.start_element("defs");
        self.xml.start_element("clipPath");
        self.xml.write_attribute("id", REGION_CLIP_ID);
        self.write_region_rect(x, y, w, h);
        self.xml.end_element();
        self.xml.end_element();
        self.xml.end_element();
        self.xml.start_element("g");
        self.xml
            .write_attribute_fmt("clip-path", format_args!("url(#{REGION_CLIP_ID})"));
        let [r, g, b, a] = self.render_settings.bg_color;
        if a > 0 {
            self.write_region_rect(x, y, w, h);
            self.xml
                .write_attribute_fmt("fill", format_args!("rgb({r},{g},{b})"));
            if a < 255 {
                self.xml
                    .write_attribute_fmt("fill-opacity", format_args!("{}", f32::from(a) / 255.0));
            }
            self.xml.end_element();
        }
        self.region_group = true;
        self.cull = Some(region);
    }

    /// Starts a `<rect>` element (left open for its attributes).
    fn write_region_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.xml.start_element("rect");
        self.xml.write_attribute_fmt("x", format_args!("{x}"));
        self.xml.write_attribute_fmt("y", format_args!("{y}"));
        self.xml.write_attribute_fmt("width", format_args!("{w}"));
        self.xml.write_attribute_fmt("height", format_args!("{h}"));
    }
}
