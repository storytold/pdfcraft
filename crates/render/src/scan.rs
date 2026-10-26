//! Which pages are scans, for drawing them through a dark reading setting's scan filter
//! ([`crate::ScanFilter`]): a picture covers at least half of the page and the page has no visible
//! text of its own. An invisible text layer (OCR) is not visible text, and neither are comments:
//! a scan with notes on it is still a scan. The same rule as Edit's recognising text on scans.

use std::collections::HashMap;
use std::sync::Mutex;

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    interpret_page,
};
use kurbo::{Affine, BezPath, Rect};

/// Whether each page judged so far is a scan. One per render pool, so per document bytes: an
/// edit makes a new pool and the pages are judged again.
#[derive(Debug, Default)]
pub(crate) struct ScanPages(Mutex<HashMap<usize, bool>>);

impl ScanPages {
    /// Whether `page` is a scan, judged on first use. The caller catches panics from the
    /// interpreter; a page that cannot be read is no scan.
    pub(crate) fn is_scan(&self, pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> bool {
        if let Some(known) = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&page) {
            return *known;
        }
        // Judged outside the lock: another worker may judge the same page meanwhile, to the
        // same answer.
        let scan = is_scan(pdf, page, settings);
        // A cancelled render stopped the reading part-way: its answer is not kept (and the render
        // it was for is dropped anyway).
        if !settings.cancelled.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Acquire)) {
            self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(page, scan);
        }
        scan
    }
}

/// Interprets `page` without drawing (pictures are not decoded) and applies the rule.
fn is_scan(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> bool {
    let pages = pdf.pages();
    let Some(p) = pages.get(page) else { return false };
    let (w, h) = p.render_dimensions();
    let area = f64::from(w) * f64::from(h);
    if !(area.is_finite() && area > 0.0) {
        return false;
    }
    let cache = InterpreterCache::new();
    let initial = p.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, f64::from(w), f64::from(h)), &cache, p.xref(), settings.clone());
    let mut dev = ScanDevice::default();
    interpret_page(p, &mut ctx, &mut dev);
    !dev.text && dev.largest_picture >= 0.5 * area
}

/// Records the largest picture's area and whether visible page text was drawn.
#[derive(Default)]
struct ScanDevice {
    /// The largest area a picture covers, in square points of the page as drawn.
    largest_picture: f64,
    text: bool,
    in_annotations: bool,
}

impl<'a> Device<'a> for ScanDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, _: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, mode: &GlyphDrawMode) {
        if !self.in_annotations && matches!(mode, GlyphDrawMode::Fill | GlyphDrawMode::Stroke(_)) {
            self.text = true;
        }
    }
    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        if self.in_annotations {
            return;
        }
        // The transform places the picture's pixels (one unit each): its area as drawn is the
        // transform's determinant times the pixel count. Read from the image's dictionary, not
        // decoded.
        let pixels = f64::from(image.width()) * f64::from(image.height());
        let area = transform.determinant().abs() * pixels;
        if area.is_finite() && area > self.largest_picture {
            self.largest_picture = area;
        }
    }
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn begin_annotations(&mut self) {
        self.in_annotations = true;
    }
    fn end_annotations(&mut self) {
        self.in_annotations = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A 200 × 100 pt page drawing `body`, with a 2 × 2 picture `/Im0` of scanner-grey paper
    /// (240) and Helvetica `/F1`.
    fn page_pdf(body: &str) -> Vec<u8> {
        let image = "f0f0f0f0>";
        format!(
            "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /XObject << /Im0 5 0 R >> /Font << /F1 6 0 R >> >> /Contents 4 0 R >> endobj\n\
             4 0 obj << /Length {} >> stream\n{body}endstream endobj\n\
             5 0 obj << /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length {} >> stream\n{image}\nendstream endobj\n\
             6 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
            body.len(),
            image.len(),
        )
        .into_bytes()
    }

    fn judged(body: &str) -> bool {
        let pdf = Pdf::new(Arc::new(page_pdf(body))).unwrap();
        ScanPages::default().is_scan(&pdf, 0, &InterpreterSettings::default())
    }

    #[test]
    fn a_page_sized_picture_without_text_is_a_scan() {
        assert!(judged("q 200 0 0 100 0 0 cm /Im0 Do Q\n"), "the whole page");
        assert!(judged("q 100 0 0 100 0 0 cm /Im0 Do Q\n"), "half of it");
        // An OCR layer is invisible text (3 Tr): still a scan.
        assert!(judged("q 200 0 0 100 0 0 cm /Im0 Do Q\nBT 3 Tr /F1 12 Tf 10 10 Td (recognised) Tj ET\n"), "with an OCR layer");
    }

    #[test]
    fn small_pictures_and_visible_text_are_no_scan() {
        assert!(!judged("q 99 0 0 100 0 0 cm /Im0 Do Q\n"), "under half of the page");
        assert!(!judged("q 200 0 0 100 0 0 cm /Im0 Do Q\nBT /F1 12 Tf 10 10 Td (caption) Tj ET\n"), "a picture behind real text");
        assert!(!judged("BT /F1 12 Tf 10 10 Td (text only) Tj ET\n"), "a text page");
        assert!(!judged(""), "an empty page");
        let pdf = Pdf::new(Arc::new(page_pdf(""))).unwrap();
        assert!(!ScanPages::default().is_scan(&pdf, 7, &InterpreterSettings::default()), "a page that does not exist");
    }

    #[test]
    fn a_page_is_judged_once() {
        let pdf = Pdf::new(Arc::new(page_pdf("q 200 0 0 100 0 0 cm /Im0 Do Q\n"))).unwrap();
        let pages = ScanPages::default();
        assert!(pages.is_scan(&pdf, 0, &InterpreterSettings::default()));
        // The answer is kept: asked again, even about another document's page, it is not read again.
        let other = Pdf::new(Arc::new(page_pdf(""))).unwrap();
        assert!(pages.is_scan(&other, 0, &InterpreterSettings::default()));
    }

    #[test]
    fn a_cancelled_reading_is_not_kept() {
        let pdf = Pdf::new(Arc::new(page_pdf("q 200 0 0 100 0 0 cm /Im0 Do Q\n"))).unwrap();
        let pages = ScanPages::default();
        let cancelled = InterpreterSettings { cancelled: Some(Arc::new(std::sync::atomic::AtomicBool::new(true))), ..Default::default() };
        assert!(!pages.is_scan(&pdf, 0, &cancelled), "stopped before the picture");
        assert!(pages.is_scan(&pdf, 0, &InterpreterSettings::default()), "read again in full");
    }

    /// The middle pixel of page 0 rendered with `colors`, and with `tag`.
    fn middle(body: &str, colors: crate::ReadingColors, tag: u64) -> [u8; 4] {
        let config = crate::RenderConfig { reading_colors: colors, ..Default::default() };
        let mut renderer = crate::PageRenderer::new(Arc::new(page_pdf(body)), config);
        let page = renderer.render(crate::RenderRequest { page: 0, scale: 1.0, tag, ..Default::default() });
        assert!(page.error.is_none(), "{:?}", page.error);
        let at = ((page.height / 2 * page.width + page.width / 2) * 4) as usize;
        page.rgba[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn white_on_black_draws_a_scan_dark() {
        let scan = "q 200 0 0 100 0 0 cm /Im0 Do Q\n";
        let dark = crate::ReadingColors::WHITE_ON_BLACK;
        assert_eq!(middle(scan, crate::ReadingColors::default(), 0), [240, 240, 240, 255], "off: the scan's own paper");
        assert_eq!(middle(scan, dark, 0), [0, 0, 0, 255], "the paper takes the page background");
        let invert = crate::ReadingColors { scans: crate::ScanColors { mode: crate::ScanMode::Invert, ..crate::ScanColors::DEFAULT }, ..dark };
        assert_eq!(middle(scan, invert, 0), [15, 15, 15, 255], "a plain negative");
        let off = crate::ReadingColors { scans: crate::ScanColors { mode: crate::ScanMode::Off, ..crate::ScanColors::DEFAULT }, ..dark };
        assert_eq!(middle(scan, off, 0), [240, 240, 240, 255], "scans set to keep their colours");
        // A print preview shows the printout's colours; a light setting leaves scans alone.
        assert_eq!(middle(scan, dark, crate::PRINT_TAG), [240, 240, 240, 255], "print preview");
        assert_eq!(middle(scan, crate::ReadingColors::BLACK_ON_WHITE, 0), [240, 240, 240, 255], "black on white");
        // A picture that is no scan keeps its colours, as before.
        assert_eq!(middle("q 200 0 0 100 0 0 cm /Im0 Do Q\nBT /F1 12 Tf 10 10 Td (caption) Tj ET\n", dark, 0), [240, 240, 240, 255], "behind text");
    }
}
