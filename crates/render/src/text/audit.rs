//! Content a human does not see but a text extractor does: the raw material of prompt-injection
//! audits. A page device that records every glyph a text extractor would read, with what makes
//! it invisible (or nearly) to a reader: render modes 3/7 (`GlyphDrawMode::Invisible`), glyphs
//! wholly off the displayed page, and the solid fill colour a near-invisible paint would use.
//! ISO 32000-2 §9.3.3 defines the render modes; what counts as "invisible" is our
//! classification, not the format's.
//!
//! Unlike text extraction ([`super::device`]) nothing is dropped: invisible and off-page glyphs
//! are the point. Memory is the caller's: glyphs stream through a sink one at a time, so a
//! hostile page costs the caller nothing it did not ask for. `/ActualText` replacements
//! (ISO 32000-2 §14.9.4) are reported as extraction sees them — one glyph standing for the
//! span, flagged [`GlyphAudit::actual_text`] so a caller can review what the reader saw against
//! what the extractor reads.

use super::{
    Affine, BezPath, BlendMode, ClipPath, Context, Device, Glyph, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode,
    Pdf, Rect, Shape, SoftMask, TransformExt, interpret_page,
};

/// One glyph a text extractor reads, with what makes it invisible (or nearly) to a reader.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphAudit {
    /// The glyph's Unicode text (one character, or several for a ligature), or the
    /// `/ActualText` string the extractor reads in place of the span.
    pub text: String,
    /// Em-box bounding rect in page view space (points, origin top-left, y down): [x0, y0, x1, y1].
    pub rect: [f32; 4],
    /// Painted by no render mode: `Tr 3`/`Tr 7` (invisible), or a clip-shaping glyph.
    pub unpainted: bool,
    /// Wholly outside the displayed page (the crop box after rotation).
    pub off_page: bool,
    /// Solid fill colour, RGBA 0–255; `None` for patterns and strokes.
    pub fill: Option<[u8; 4]>,
    /// The glyph stands for a `/ActualText` replacement: `text` is what an extractor reads,
    /// while a reader sees whatever the source span paints. The span's own visibility flags
    /// (painted below) are the source glyphs', not the replacement string's.
    pub actual_text: bool,
}

/// The audit of one page: every glyph an extractor reads, in content order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageAudit {
    pub glyphs: Vec<GlyphAudit>,
}

/// What an open `/ActualText` replacement has drawn so far, aggregated glyph by glyph: the
/// reader sees this, the extractor reads the replacement string. Nothing per-glyph is kept,
/// so a page-long valid span costs the same as a one-glyph one.
struct ActualDrawn {
    /// Union rectangle of the drawn glyphs; `None` while nothing has painted.
    rect: Option<[f32; 4]>,
    /// Painted by no glyph in the span: `all` over the drawn glyphs, so the empty span's
    /// identity is `true`.
    unpainted: bool,
    off_page: bool,
    fill: Option<[u8; 4]>,
}

impl Default for ActualDrawn {
    fn default() -> Self {
        Self { rect: None, unpainted: true, off_page: true, fill: None }
    }
}

impl ActualDrawn {
    fn draw(&mut self, glyph: &GlyphAudit) {
        self.rect = Some(match self.rect {
            Some(r) => [r[0].min(glyph.rect[0]), r[1].min(glyph.rect[1]), r[2].max(glyph.rect[2]), r[3].max(glyph.rect[3])],
            None => glyph.rect,
        });
        self.unpainted &= glyph.unpainted;
        self.off_page &= glyph.off_page;
        if self.fill.is_none() {
            self.fill = glyph.fill;
        }
    }
}

struct AuditDevice<'s> {
    page: Rect,
    sink: &'s mut dyn FnMut(GlyphAudit),
    /// The outermost open `/ActualText` replacement: its depth, text and what the span has
    /// drawn, mirroring `TextDevice` (an inner replacement is covered by the outer).
    actual: Option<(usize, String, ActualDrawn)>,
    depth: usize,
}

impl<'a> Device<'a> for AuditDevice<'_> {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}

    fn begin_marked_content(&mut self, _: &[u8], _: Option<i32>) {
        self.depth = self.depth.saturating_add(1);
    }

    fn actual_text(&mut self, text: &[u8]) {
        // A replacement nested in another is already covered by the outer one (as TextDevice).
        if self.actual.is_none() {
            let text = pdfcraft_cos::PdfString::literal(text).to_text();
            self.actual = Some((self.depth, text, ActualDrawn::default()));
        }
    }

    fn end_marked_content(&mut self) {
        if let Some((_, text, drawn)) = self.actual.take_if(|(depth, _, _)| *depth == self.depth) {
            self.emit_actual(text, drawn);
        }
        self.depth = self.depth.saturating_sub(1);
    }

    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, paint: &Paint<'a>, mode: &GlyphDrawMode) {
        let g = self.audited_glyph(glyph, transform, glyph_transform, paint, mode);
        match &mut self.actual {
            // Inside an `/ActualText` span the extractor will not read this glyph's own text;
            // the span's merged replacement is reported on `end_marked_content`. A glyph with
            // no readable text of its own (no Unicode mapping, control-only) still paints, so
            // the span's geometry and visibility record it: an extractor reads the span's
            // replacement over such glyphs too, and dropping them would hide the span.
            Some((_, _, drawn)) => drawn.draw(&g),
            // Outside a span, a glyph without readable text is nothing an extractor reads.
            None if !g.text.is_empty() => (self.sink)(g),
            None => {}
        }
    }
}

impl AuditDevice<'_> {
    /// Emit the replacement an extractor reads in the span's place, as one merged glyph (as
    /// TextDevice's `merge`): the reader sees whatever the source glyphs painted — the merged
    /// glyph's paint fields carry that. A control-character replacement reads as nothing.
    fn emit_actual(&mut self, text: String, drawn: ActualDrawn) {
        if text.chars().all(char::is_control) {
            return;
        }
        let Some(rect) = drawn.rect else { return };
        (self.sink)(GlyphAudit { text, rect, unpainted: drawn.unpainted, off_page: drawn.off_page, fill: drawn.fill, actual_text: true });
    }

    /// Flush a `/ActualText` span still open at page end (a missing `EMC`), as
    /// `TextDevice::finish` does for extraction.
    fn finish(&mut self) {
        if let Some((_, text, drawn)) = self.actual.take() {
            self.emit_actual(text, drawn);
        }
    }

    fn audited_glyph(
        &mut self,
        glyph: &Glyph<'_>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'_>,
        mode: &GlyphDrawMode,
    ) -> GlyphAudit {
        // A glyph with no Unicode mapping (or only control characters) has no readable text
        // of its own — the empty record makes `draw_glyph` drop it outside a span and record
        // only its geometry inside one. A /ActualText replacement over such a glyph is still
        // what an extractor reads, so the span must not lose the glyph's paint.
        let text = match glyph.as_unicode() {
            Some(craft_pdf::interpret::hayro_cmap::BfString::Char(c)) => c.to_string(),
            Some(craft_pdf::interpret::hayro_cmap::BfString::String(s)) => s,
            None => String::new(),
        };
        let text = if text.chars().all(char::is_control) { String::new() } else { text };
        let fill = match paint {
            Paint::Color(c) => Some(c.to_rgba().to_rgba8()),
            Paint::Pattern(_) => None,
        };
        // Clip-shaping glyphs paint no text of their own (pdfcraft patch note on
        // `GlyphDrawMode::Clip`): an extractor reads them, a reader sees at most their clip.
        let unpainted = !matches!(mode, GlyphDrawMode::Fill | GlyphDrawMode::Stroke(_));
        // Glyph space → view space, as text extraction reads it: 1000 units per em, the em box
        // spanning descender (−200) to ascender (800). Vertical fonts advance along −y and keep
        // the outline upright; the em-box bounding rect is the same either way.
        let t = transform * glyph_transform;
        let advance = match glyph {
            Glyph::Outline(g) => {
                g.advance_width().filter(|a| *a > 0.0).map(f64::from).unwrap_or_else(|| g.outline().bounding_box().width().max(500.0))
            }
            Glyph::Type3(g) => g.advance_width().filter(|a| a.is_finite() && *a > 0.0).map(f64::from).unwrap_or(600.0),
        };
        let em = Rect::new(0.0, -200.0, advance, 800.0);
        let corners = [(em.x0, em.y0), (em.x1, em.y0), (em.x1, em.y1), (em.x0, em.y1)].map(|(x, y)| t * kurbo::Point::new(x, y));
        let b = corners.iter().fold(Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |b, p| {
            Rect::new(b.x0.min(p.x), b.y0.min(p.y), b.x1.max(p.x), b.y1.max(p.y))
        });
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) || b.width() > 10_000.0 || b.height() > 10_000.0 {
            // A hostile or broken transform: the text still goes to extractors, so record it
            // without a box, flagged off-page.
            return GlyphAudit { text, rect: [0.0; 4], unpainted, off_page: true, fill, actual_text: false };
        }
        let off_page = b.x0.max(self.page.x0) >= b.x1.min(self.page.x1) || b.y0.max(self.page.y0) >= b.y1.min(self.page.y1);
        GlyphAudit { text, rect: [b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32], unpainted, off_page, fill, actual_text: false }
    }
}

/// Audit one page, streaming every glyph an extractor reads to `sink` in content order.
/// Memory stays with the caller. Panics inside the interpreter are the caller's to catch
/// (`crate::guard`).
pub fn audit_page_with(pdf: &Pdf, page: usize, settings: &InterpreterSettings, sink: &mut dyn FnMut(GlyphAudit)) -> Result<(), String> {
    let pages = pdf.pages();
    let p = pages.get(page).ok_or("the page could not be read")?;
    let (w, h) = p.render_dimensions();
    let cache = InterpreterCache::new();
    let settings = settings.clone();
    let initial = p.initial_transform(true).to_kurbo();
    let c = p.intersected_crop_box();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, f64::from(w), f64::from(h)), &cache, p.xref(), settings);
    let mut dev = AuditDevice { page: initial.transform_rect_bbox(Rect::new(c.x0, c.y0, c.x1, c.y1)), sink, actual: None, depth: 0 };
    interpret_page(p, &mut ctx, &mut dev);
    // A span left open (a missing EMC) still replaces what extraction reads: flush it, as
    // TextDevice::finish does.
    dev.finish();
    Ok(())
}

/// Audit one page, collecting every glyph an extractor reads. Prefer [`audit_page_with`]: a
/// hostile page's glyph count is the caller's memory here. Panics inside the interpreter are
/// the caller's to catch (`crate::guard`).
pub fn audit_page(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> Result<PageAudit, String> {
    let mut glyphs = Vec::new();
    audit_page_with(pdf, page, settings, &mut |g| glyphs.push(g))?;
    Ok(PageAudit { glyphs })
}

#[cfg(test)]
mod tests {
    use super::super::tests::marked_pages;

    use super::*;

    fn audit(content: &str) -> PageAudit {
        audit_page(&marked_pages(&[content]), 0, &InterpreterSettings::default()).unwrap()
    }

    #[test]
    fn visible_text_is_not_flagged() {
        let audit = audit("BT /F1 20 Tf 1 0 0 1 100 150 Tm (Hello) Tj ET");
        assert_eq!(audit.glyphs.len(), 5);
        assert!(audit.glyphs.iter().all(|g| !g.unpainted && !g.off_page && !g.actual_text), "{:?}", audit.glyphs);
        assert!(audit.glyphs.iter().all(|g| g.fill.is_some_and(|[r, g, b, _]| r == 0 && g == 0 && b == 0)), "{:?}", audit.glyphs);
    }

    #[test]
    fn render_mode_3_text_is_unpainted() {
        let audit = audit("BT /F1 20 Tf 1 0 0 1 100 150 Tm (See) Tj 3 Tr 1 0 0 1 100 100 Tm (Hide) Tj ET");
        assert_eq!(audit.glyphs.len(), 7);
        assert_eq!(audit.glyphs.iter().filter(|g| g.unpainted).count(), 4, "{:?}", audit.glyphs);
    }

    #[test]
    fn off_page_text_is_flagged_but_recorded() {
        let audit = audit("BT /F1 20 Tf 1 0 0 1 100 150 Tm (On) Tj 1 0 0 1 500 150 Tm (Off) Tj ET");
        assert_eq!(audit.glyphs.len(), 5);
        assert_eq!(audit.glyphs.iter().filter(|g| g.off_page).count(), 3, "{:?}", audit.glyphs);
    }

    #[test]
    fn white_fill_is_recorded_as_white() {
        let audit = audit("BT /F1 20 Tf 1 1 1 rg 1 0 0 1 100 150 Tm (Ghost) Tj ET");
        assert_eq!(audit.glyphs.len(), 5);
        assert!(audit.glyphs.iter().all(|g| g.fill.is_some_and(|[r, g, b, _]| r == 255 && g == 255 && b == 255)), "{:?}", audit.glyphs);
    }

    /// The extractor reads a `/Span <</ActualText …>>` replacement in the span's place, and
    /// the audit must say it is reading one.
    #[test]
    fn actual_text_is_reported_as_a_replacement() {
        let audit = audit("BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (fi)>> BDC (find) Tj EMC ET");
        let replaced: Vec<&GlyphAudit> = audit.glyphs.iter().filter(|g| g.actual_text).collect();
        assert_eq!(replaced.len(), 1, "{:?}", audit.glyphs);
        assert_eq!(replaced[0].text, "fi");
        // The reader sees what the source glyphs painted: their own (painted) flags stand.
        assert!(!replaced[0].unpainted);
        // Nothing else is recorded: the extractor reads the replacement in the span's place.
        assert_eq!(audit.glyphs.len(), 1, "{:?}", audit.glyphs);
    }

    /// `/ActualText` over glyphs with no Unicode mapping (control codes in a standard font
    /// stand in for a custom-encoded font): the extractor still reads the replacement, so
    /// the span must be recorded — with the geometry its source glyphs painted — rather
    /// than dropped, or hidden injection text escapes the audit.
    #[test]
    fn actual_text_over_glyphs_without_unicode_is_reported() {
        let audit = audit("BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (INJECTED)>> BDC (\\001\\002) Tj EMC ET");
        let replaced: Vec<&GlyphAudit> = audit.glyphs.iter().filter(|g| g.actual_text).collect();
        assert_eq!(replaced.len(), 1, "{:?}", audit.glyphs);
        assert_eq!(replaced[0].text, "INJECTED");
        assert!(!replaced[0].unpainted, "the source glyphs paint: {:?}", audit.glyphs);
        let r = replaced[0].rect;
        assert!(r[2] > r[0] && r[3] > r[1], "a non-degenerate box where the glyphs painted: {r:?}");
        assert_eq!(audit.glyphs.len(), 1, "only the replacement is recorded: {:?}", audit.glyphs);
    }

    /// A visible glyph standing for an invisible replacement must not inherit visible flags:
    /// the reader sees the visible source, the extractor reads text the reader never sees.
    #[test]
    fn actual_text_over_invisible_span_stays_invisible() {
        let audit = audit("BT /F1 20 Tf 3 Tr 1 0 0 1 100 150 Tm /Span <</ActualText (fi)>> BDC (find) Tj EMC ET");
        let replaced: Vec<&GlyphAudit> = audit.glyphs.iter().filter(|g| g.actual_text).collect();
        assert_eq!(replaced.len(), 1, "{:?}", audit.glyphs);
        assert!(replaced[0].unpainted, "{:?}", replaced[0]);
    }

    /// An empty replacement reads as nothing, and the audit records nothing for the span.
    #[test]
    fn empty_actual_text_records_nothing() {
        let audit = audit("BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText ()>> BDC (find) Tj EMC ET");
        assert!(audit.glyphs.is_empty(), "{:?}", audit.glyphs);
    }

    /// A span left open (a missing `EMC`) still replaces what extraction reads, so the audit
    /// must flush it at page end exactly as `TextDevice::finish` does.
    #[test]
    fn unclosed_actual_text_span_is_flushed() {
        let audit = audit("BT /F1 20 Tf 1 0 0 1 100 150 Tm /Span <</ActualText (fi)>> BDC (find) Tj ET");
        let replaced: Vec<&GlyphAudit> = audit.glyphs.iter().filter(|g| g.actual_text).collect();
        assert_eq!(replaced.len(), 1, "{:?}", audit.glyphs);
        assert_eq!(replaced[0].text, "fi");
        assert_eq!(audit.glyphs.len(), 1, "{:?}", audit.glyphs);
    }
}
