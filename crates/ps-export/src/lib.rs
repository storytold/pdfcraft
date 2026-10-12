//! pdfcraft-ps-export — PDF → PostScript/EPS export (Layer A).
//!
//! The crate implements hayro-interpret's `Device` trait as a PostScript emitter and drives it
//! through the same page-drive entry the raster renderer uses, so export semantics match
//! `page_render` by construction. This skeleton holds the public interface plus the
//! reachability canaries for the vendored hayro stack and the emitter's device core
//! (geometry, colour, clip); text, images, shadings and the `export()` assembly arrive with
//! the later Layer A tasks.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod device;
mod dsc;
mod fonts;
mod images;
mod psnum;
mod shading;

use std::sync::Arc;

/// The output flavour: a whole-document PostScript program or a single-page EPS file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PsFormat {
    /// Multi-page PostScript (`%!PS-Adobe-3.0`, one `%%Page` block per exported page).
    Ps,
    /// Encapsulated PostScript for a single page (`%!PS-Adobe-3.0 EPSF-3.0`, guarded `showpage`).
    Eps,
}

/// A recorded degradation: something in the source had no faithful PostScript equivalent.
#[derive(Clone, Debug, PartialEq)]
pub struct FidelityNote {
    /// The 1-based page the note refers to, or `None` for document-level notes.
    pub page: Option<usize>,
    /// Human-readable description of what was degraded (also emitted as a `%%PdfCraftNote:`).
    pub text: String,
}

/// A successful export: the PostScript program plus every fidelity note.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportedPs {
    /// The complete, DSC-structured PostScript document.
    pub bytes: Vec<u8>,
    /// Every degradation, once each (also embedded as `%%PdfCraftNote:` comments in `bytes`).
    pub warnings: Vec<FidelityNote>,
}

/// Why an export could not be produced. Each variant names what was wrong and what is allowed.
#[derive(Clone, Debug, PartialEq)]
pub enum PsExportError {
    /// The bytes could not be opened as a PDF; the string says what failed.
    UnreadableDocument(String),
    /// Page indices out of range: the requested 0-based indices and the actual page count.
    InvalidPages { requested: Vec<usize>, page_count: usize },
    /// EPS output needs exactly one page; `requested` is how many were asked for.
    EpsNeedsExactlyOnePage { requested: usize },
}

/// Export the given PDF `bytes` to PostScript (`PsFormat::Ps`) or EPS (`PsFormat::Eps`).
///
/// `pages` are 0-based page indices; they are validated against the page count, emitted in
/// ascending order, and duplicates collapse. Not yet implemented — the emitter arrives with the
/// later Layer A tasks — so for now every call fails with [`PsExportError::UnreadableDocument`].
pub fn export(_bytes: Arc<Vec<u8>>, _pages: &[usize], _format: PsFormat) -> Result<ExportedPs, PsExportError> {
    Err(PsExportError::UnreadableDocument("not yet implemented".into()))
}

#[cfg(test)]
mod tests {
    use hayro::hayro_interpret::font::Glyph;
    use hayro::hayro_interpret::hayro_syntax::{Filter, Pdf};
    use hayro::hayro_interpret::{
        BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask,
        interpret_page,
    };
    use kurbo::{Affine, BezPath, Rect, Shape};

    /// The still-encoded payload of the fixture's DCTDecode image; nothing in a reachability
    /// test decodes it, so the bytes are arbitrary but recognisable.
    const IMAGE_PAYLOAD: [u8; 6] = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc];

    /// A `<< … >> stream … endstream` object body, in the hand-written style of
    /// `crates/engine/src/tests.rs` (length computed, not counted by hand).
    fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
        let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        obj.extend_from_slice(body);
        obj.extend_from_slice(b"\nendstream");
        obj
    }

    /// A minimal one-page PDF, hand-written like `crates/engine/src/tests.rs::fixture`. The page
    /// content draws a form XObject (`/Fm1`), a DCTDecode image XObject (`/Im1`), a filled
    /// rectangle and one line of non-embedded Type 1 text (`/F1`); a square annotation's
    /// appearance stream draws one more rectangle.
    fn fixture() -> Vec<u8> {
        let objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [8 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            stream_obj(
                "/Type /XObject /Subtype /Form /BBox [0 0 100 100]",
                b"0 1 0 rg 10 10 80 80 re f",
            ),
            stream_obj(
                "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
                &IMAGE_PAYLOAD,
            ),
            b"<< /Type /Annot /Subtype /Square /Rect [100 150 190 260] /F 4 /AP << /N 7 0 R >> >>".to_vec(),
            stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 90 110]", b"0 0 1 rg 5 5 80 100 re f"),
            b"<< /Type /Page /Parent 2 0 R /Contents 9 0 R /Annots [6 0 R] /Resources << /Font << /F1 3 0 R >> /XObject << /Fm1 4 0 R /Im1 5 0 R >> >> >>".to_vec(),
            stream_obj("", b"q /Fm1 Do Q\nq 200 0 0 200 0 0 cm /Im1 Do Q\n1 0 0 rg\n20 40 160 60 re f\nBT /F1 24 Tf 20 150 Td (P) Tj ET"),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// A `DummyDevice` clone that records which device methods ran and what the glyph and image
    /// arguments exposed through public API (the DummyDevice itself keeps nothing).
    #[derive(Default)]
    struct RecordingDevice {
        paths: usize,
        glyphs: usize,
        images: usize,
        clip_pushes: usize,
        clip_pops: usize,
        group_pushes: usize,
        group_pops: usize,
        image_filters: Vec<Filter>,
        image_bytes: Vec<u8>,
        outline_elements: usize,
        outline_size: (f64, f64),
        glyph_font_keys: Vec<u128>,
        glyph_ids: Vec<u32>,
        glyph_font_bytes: Vec<Option<usize>>,
        glyph_unicode: Vec<bool>,
    }

    impl Device<'_> for RecordingDevice {
        fn set_soft_mask(&mut self, _mask: Option<SoftMask<'_>>) {}
        fn set_blend_mode(&mut self, _blend_mode: BlendMode) {}
        fn draw_path(&mut self, _path: &BezPath, _transform: Affine, _paint: &Paint<'_>, _mode: &PathDrawMode) {
            self.paths += 1;
        }
        fn push_clip_path(&mut self, _clip_path: &ClipPath) {
            self.clip_pushes += 1;
        }
        fn push_transparency_group(&mut self, _opacity: f32, _mask: Option<SoftMask<'_>>, _blend_mode: BlendMode) {
            self.group_pushes += 1;
        }
        fn draw_glyph(&mut self, glyph: &Glyph<'_>, _transform: Affine, _glyph_transform: Affine, _paint: &Paint<'_>, _mode: &GlyphDrawMode) {
            self.glyphs += 1;
            if let Glyph::Outline(g) = glyph {
                let outline = g.outline();
                self.outline_elements = outline.elements().len();
                let bbox = outline.bounding_box();
                self.outline_size = (bbox.width(), bbox.height());
                self.glyph_font_keys.push(g.font_cache_key());
                self.glyph_ids.push(g.glyph_id().to_u32());
                self.glyph_font_bytes.push(g.font_data().map(|d| d.data.as_ref().as_ref().len()));
                self.glyph_unicode.push(g.as_unicode().is_some());
            }
        }
        fn draw_image(&mut self, image: Image<'_, '_>, _transform: Affine) {
            self.images += 1;
            if let Image::Raster(raster) = &image {
                self.image_filters = raster.stream().filters().to_vec();
                self.image_bytes = raster.stream().raw_data().into_owned();
            }
        }
        fn pop_clip_path(&mut self) {
            self.clip_pops += 1;
        }
        fn pop_transparency_group(&mut self) {
            self.group_pops += 1;
        }
    }

    /// Parse the fixture and drive one custom `Device` over its page through
    /// `hayro_interpret::interpret_page` — the same entry `hayro::render_into` uses for its
    /// own renderer (findings (c)).
    fn driven() -> RecordingDevice {
        let pdf = Pdf::new(fixture()).expect("fixture parses");
        let page = pdf.pages().first().expect("fixture has a page");
        let cache = InterpreterCache::new();
        let mut state = Context::new(Affine::IDENTITY, Rect::new(0.0, 0.0, 200.0, 300.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = RecordingDevice::default();
        interpret_page(page, &mut state, &mut device);
        device
    }

    /// Spike (a): a DCTDecode image XObject reaches the device with its still-encoded bytes and
    /// its filter name readable through public API: `Image::Raster` → `RasterImage::stream()` →
    /// `Stream::filters()` / `Stream::raw_data()`.
    #[test]
    fn image_exposes_encoded_bytes() {
        let device = driven();
        assert_eq!(device.images, 1, "the fixture's image must reach draw_image exactly once");
        assert_eq!(device.image_filters.as_slice(), &[Filter::DctDecode], "filter name via RasterImage::stream().filters()");
        assert_eq!(device.image_bytes, IMAGE_PAYLOAD, "encoded bytes via RasterImage::stream().raw_data()");
    }

    /// Spike (b): an outline glyph yields its outline (`OutlineGlyph::outline() -> BezPath`) and
    /// its font metadata through public API; no `pub fn` needs adding to `vendor/hayro` (its
    /// `outline_cache` is only the rasterizer's own memoization).
    #[test]
    fn glyph_outline_reachable() {
        let device = driven();
        assert_eq!(device.glyphs, 1, "the fixture's text must reach draw_glyph exactly once");
        assert!(device.outline_elements > 0, "OutlineGlyph::outline() returned an empty BezPath");
        let (w, h) = device.outline_size;
        assert!(w > 0.0 && h > 0.0, "the 'P' outline has no extent: {w}x{h}");
        assert_eq!(device.glyph_font_keys.len(), 1, "font_cache_key reachable");
        assert_eq!(device.glyph_unicode, vec![true], "'P' maps to Unicode via as_unicode()");
        // Non-embedded Type 1 (standard-font substitute): `font_data()` is `None` — Type 1
        // FontFile bytes must come from the font dict instead (findings (b), Task 5).
        assert_eq!(device.glyph_font_bytes, vec![None], "font_data() is None for Type 1");
        assert!(device.glyph_ids.len() == 1, "glyph_id reachable");
    }

    /// Spike (c): a custom `Device` impl is driven over a one-page fixture through
    /// `hayro_interpret::interpret_page`, which also interprets annotations (`render_annotations`
    /// defaults on) and, through the content stream's `Do`, form XObjects.
    #[test]
    fn drive_entry_exists() {
        let device = driven();
        // Page rectangle + form XObject rectangle + annotation appearance rectangle: exactly
        // three fills, which is only possible if forms and annotations are interpreted through
        // the same entry as the page content.
        assert_eq!(device.paths, 3, "page/form/annotation rectangles must all reach draw_path");
        assert!(device.clip_pushes >= 1, "the form XObject pushes its /BBox clip");
        assert_eq!(device.clip_pushes, device.clip_pops, "clip pushes and pops balance");
        assert!(device.group_pushes >= 1, "the image draw wraps in a transparency group");
        assert_eq!(device.group_pushes, device.group_pops, "group pushes and pops balance");
        assert_eq!(device.images, 1);
        assert_eq!(device.glyphs, 1);
    }
}
