//! pdfcraft-ps-export — PDF → PostScript/EPS export (Layer A).
//!
//! The crate implements hayro-interpret's `Device` trait as a PostScript emitter and drives it
//! through the same page-drive entry the raster renderer uses — and with the same font-resolver
//! wiring (`pdfcraft_render::page_font_resolver`, the craft-fonts Japanese fallback over the
//! standard-14 substitutes) — so export semantics match `page_render` by construction. [`export`]
//! validates the page selection, drives every requested page (content, annotations and form
//! XObjects through `interpret_page`) into its own operator buffer, then assembles the
//! DSC-structured program: header, font prolog, one `%%Page` block per page in ascending order,
//! trailer — with every degradation recorded both in [`ExportedPs::warnings`] and as a
//! `%%PdfCraftNote:` comment.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod cid;
mod color;
mod device;
mod dsc;
mod flatten;
mod fonts;
mod images;
mod pattern;
mod psnum;
mod sfnt;
mod shading;
mod type1_subset;

use std::sync::Arc;

use hayro::hayro_interpret::hayro_syntax::{LoadPdfError, Pdf};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, FillRule, InterpreterCache, InterpreterSettings, RectExt, TransformExt, interpret_page,
};
use kurbo::{Affine, Point, Rect, Shape};

use crate::device::PsDevice;
use crate::fonts::FontTable;

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
/// `pages` are 0-based page indices; they are validated against the page count (out of
/// range → [`PsExportError::InvalidPages`]), emitted in ascending order, and duplicates
/// collapse. EPS output needs exactly one page
/// ([`PsExportError::EpsNeedsExactlyOnePage`] otherwise). The drive mirrors
/// `hayro::render_into`: the same `interpret_page` entry (annotations and form XObjects
/// included), a crop-box clip and page transparency group around the content, and the
/// interpreter's nesting-depth and content budgets bounding hostile documents. Every
/// degradation lands in [`ExportedPs::warnings`] *and* as a `%%PdfCraftNote:` comment in
/// [`ExportedPs::bytes`].
///
/// Equivalent to [`export_with_config`] over a default [`pdfcraft_render::RenderConfig`]:
/// no password, the document's own layer state and comments visible.
pub fn export(bytes: Arc<Vec<u8>>, pages: &[usize], format: PsFormat) -> Result<ExportedPs, PsExportError> {
    export_with_config(bytes, &pdfcraft_render::RenderConfig::default(), pages, format)
}

/// Export with the viewer configuration the document is being shown under: the password
/// that opens it, layer (OCG) visibility overrides and View ▸ Hide all comments — the
/// same [`pdfcraft_render::RenderConfig`] the raster renderer's `page_render` honours, so
/// an export matches what the user sees (an encrypted document exports with its password;
/// a hidden layer or hidden comment stays out of the program). See [`export`] for the
/// page selection and drive semantics.
pub fn export_with_config(
    bytes: Arc<Vec<u8>>,
    config: &pdfcraft_render::RenderConfig,
    pages: &[usize],
    format: PsFormat,
) -> Result<ExportedPs, PsExportError> {
    // AGENTS §4's last-resort guard, the same one the raster renderer keeps around its
    // own parse and page drives: a panic inside hayro on a hostile document becomes an
    // error (parse) or a fidelity note (page drive), never an unwind into the caller —
    // the UI and wasm paths call this on their thread.
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Pdf::new_with_password(bytes, config.password.as_deref().unwrap_or(""))))
        .map_err(|_| PsExportError::UnreadableDocument("the PDF parser crashed on this document".into()))?;
    let pdf = parsed.map_err(|err| {
        let reason = match err {
            LoadPdfError::Decryption(_) => "the document is encrypted and no working password was given for it",
            LoadPdfError::Invalid => "the bytes are not a readable PDF",
        };
        PsExportError::UnreadableDocument(reason.to_string())
    })?;
    let page_count = pdf.pages().len();
    if pages.iter().any(|&index| index >= page_count) {
        return Err(PsExportError::InvalidPages { requested: pages.to_vec(), page_count });
    }
    if matches!(format, PsFormat::Eps) && pages.len() != 1 {
        return Err(PsExportError::EpsNeedsExactlyOnePage { requested: pages.len() });
    }
    let mut order = pages.to_vec();
    order.sort_unstable();
    order.dedup();

    // Drive every requested page into its own buffer. One `FontTable` and one
    // `Colorspaces`/`Patterns` resource table run across the whole document
    // (font decisions, slice names, separation and pattern resource names
    // carry between pages), one device per page (notes tag with the page's
    // 1-based output position).
    let cache = InterpreterCache::new();
    let mut fonts = FontTable::default();
    let mut colorspaces = crate::color::Colorspaces::default();
    let mut patterns = crate::pattern::Patterns::default();
    let mut driven: Vec<(Vec<u8>, Vec<FidelityNote>, [f64; 4])> = Vec::new();
    for &index in &order {
        let Some(page) = pdf.pages().get(index) else {
            continue;
        };
        let no = driven.len() + 1;
        // The same initial transform the raster renderer composes for its own device
        // (`render_into`), minus the y-flip: PostScript keeps PDF's y-up user space.
        // /Rotate, a crop-box offset and /UserUnit land here, so every draw that
        // follows bakes a correct total CTM.
        let initial = page.initial_transform(false).to_kurbo();
        // The page's clip and DSC bounding box: the (crop ∩ media) box, mapped through
        // that transform — axis-aligned, PDF rotations being multiples of 90°.
        let crop = page.intersected_crop_box().to_kurbo();
        let bbox = transformed_bbox(crop, initial);
        // The page's device-space box: the shadow scene's page (its regions clip to
        // it) and the 300-dpi page matrix both come from it.
        let page_rect = Rect::new(bbox[0], bbox[1], bbox[2], bbox[3]);
        let mut state = Context::new(
            initial,
            page_rect,
            &cache,
            page.xref(),
            // The same font-resolver wiring the raster renderer's `RenderConfig`
            // configures its page drives with (`page_font_resolver`: the craft-fonts
            // Japanese fallback over the standard-14 substitutes), so non-embedded
            // Adobe-Japan1 text meets the same face in the exported program and in
            // `page_render`'s raster. The layer overrides and comment visibility the
            // caller's config carries apply on top, as they do on screen; the
            // nesting-depth/page-content budgets the safety rules pin live in the
            // interpreter's `Context` and reset with this fresh per-page context.
            interpreter_settings(config),
        );
        let mut device = PsDevice::from_table(Some(no), page.resources(), std::mem::take(&mut fonts), page_rect);
        // Seed the page's emitter with the document's colorspace and pattern
        // tables (the same carry the fonts take): resource names stay stable
        // and repeats stay single across pages.
        device.emit.colorspaces = std::mem::take(&mut colorspaces);
        device.emit.patterns = std::mem::take(&mut patterns);
        // Mirror `render_into`: clip to the page box, wrap the page in a transparency
        // group (full opacity, no mask — not a degradation), interpret, unwrap, unclip.
        let mut clip_path = crop.to_path(0.1);
        clip_path.apply_affine(initial);
        // The clip floor the page's own clip opens from: a panic mid-interpretation
        // can leave content clips open (each an unmatched `gsave`), and the cleanup
        // below pops exactly one clip — without unwinding to this floor, that pop
        // would close the innermost stray and the page clip's `gsave` would ride the
        // page's wrapper pair onto the program's gstate stack, page after crashed
        // page.
        let clip_floor = device.clip_depth();
        device.push_clip_path(&ClipPath { path: clip_path, fill: FillRule::NonZero });
        device.push_transparency_group(1.0, None, BlendMode::Normal);
        // The raster renderer's last-resort guard again: a panic while interpreting a
        // hostile page is caught and disclosed as a note instead of unwinding; the
        // page's partial buffer still lands, wrapped in its own gsave/grestore.
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| interpret_page(page, &mut state, &mut device))).is_err();
        if crashed {
            device.emit.note("page interpretation crashed; output truncated");
        }
        device.pop_transparency_group();
        device.pop_clip_path();
        device.finish();
        if crashed {
            // After `finish` no group still withholds a buffer, so every clip's
            // `gsave` is in the final program: close the clips the panic left open
            // (the page clip's own included) down to the floor, each `grestore`
            // landing after the `gsave` it balances.
            device.unwind_clips_to(clip_floor);
        }
        let body = std::mem::take(&mut device.emit.out);
        let notes = std::mem::take(&mut device.emit.notes);
        fonts = std::mem::take(&mut device.fonts);
        colorspaces = std::mem::take(&mut device.emit.colorspaces);
        patterns = std::mem::take(&mut device.emit.patterns);
        driven.push((body, notes, bbox));
    }

    // Assemble: header → (EPS bbox note) → prolog → pages → trailer.
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    out.extend_from_slice(&dsc::document_header(format, order.len(), union_bboxes(&driven)));
    if matches!(format, PsFormat::Eps) {
        // An EPS consumer places the artwork through `%%BoundingBox`, and the exact
        // extent of what a page paints is only knowable by painting it: the CropBox is
        // the documented approximation, and it is disclosed as such.
        let note = FidelityNote { page: None, text: "BoundingBox approximated from CropBox".into() };
        out.extend_from_slice(&dsc::note(&note.text));
        warnings.push(note);
    }
    let preamble = fonts.preamble();
    // The separation and pattern resources ride in the same prolog block, after
    // the fonts.
    let colorspaces = colorspaces.prolog();
    let patterns = patterns.prolog();
    if !preamble.is_empty() || !colorspaces.is_empty() || !patterns.is_empty() {
        out.extend_from_slice(b"%%BeginProlog\n");
        out.extend_from_slice(&preamble);
        out.extend_from_slice(&colorspaces);
        out.extend_from_slice(&patterns);
        out.extend_from_slice(b"%%EndProlog\n");
    }
    for (no, (body, notes, bbox)) in driven.iter().enumerate() {
        out.extend_from_slice(&dsc::page_header(no + 1, *bbox));
        // The balancing gsave: a stray `grestore` (an unbalanced `pop_clip_path` from
        // hostile content) can then only eat this one instead of stack-underflowing the
        // program or unwinding a later page's setup.
        out.extend_from_slice(b"gsave\n");
        out.extend_from_slice(body);
        out.extend_from_slice(b"grestore\n");
        if matches!(format, PsFormat::Ps) {
            // EPS never executes `showpage`: the importing application owns the page
            // ejection, and an importer that ran it would lose its own page.
            out.extend_from_slice(b"showpage\n");
        }
        for note in notes {
            out.extend_from_slice(&dsc::note(&note.text));
            warnings.push(note.clone());
        }
    }
    out.extend_from_slice(&dsc::trailer(order.len()));
    Ok(ExportedPs { bytes: out, warnings })
}

/// The interpreter settings every exported page is driven with: the caller's
/// [`pdfcraft_render::RenderConfig`] mapped onto the interpreter exactly as the raster
/// renderer's own `RenderConfig::settings` maps it (layer overrides, comment visibility),
/// plus the same font-resolver wiring ([`pdfcraft_render::page_font_resolver`]), so glyph
/// substitution — including the craft-fonts Japanese fallback for non-embedded
/// Adobe-Japan1 text — is identical on both sides of the oracle. Everything else stays
/// at the defaults.
fn interpreter_settings(config: &pdfcraft_render::RenderConfig) -> InterpreterSettings {
    InterpreterSettings {
        ocg_overrides: config.layers.clone(),
        hide_comments: config.hide_comments,
        font_resolver: pdfcraft_render::page_font_resolver(),
        ..InterpreterSettings::default()
    }
}

/// The axis-aligned box `rect` occupies after `transform`, as `[llx lly urx ury]`
/// (the four corners mapped and folded; PDF page rotations are multiples of 90°).
fn transformed_bbox(rect: Rect, transform: Affine) -> [f64; 4] {
    let corners = [Point::new(rect.x0, rect.y0), Point::new(rect.x1, rect.y0), Point::new(rect.x0, rect.y1), Point::new(rect.x1, rect.y1)]
        .map(|p| transform * p);
    let xs = [corners[0].x, corners[1].x, corners[2].x, corners[3].x];
    let ys = [corners[0].y, corners[1].y, corners[2].y, corners[3].y];
    [
        xs[0].min(xs[1]).min(xs[2]).min(xs[3]),
        ys[0].min(ys[1]).min(ys[2]).min(ys[3]),
        xs[0].max(xs[1]).max(xs[2]).max(xs[3]),
        ys[0].max(ys[1]).max(ys[2]).max(ys[3]),
    ]
}

/// The bounding box covering every driven page (an empty selection is an empty box).
fn union_bboxes(driven: &[(Vec<u8>, Vec<FidelityNote>, [f64; 4])]) -> [f64; 4] {
    let mut union = [0.0; 4];
    let mut first = true;
    for (_, _, bbox) in driven {
        if first {
            union = *bbox;
            first = false;
            continue;
        }
        union[0] = union[0].min(bbox[0]);
        union[1] = union[1].min(bbox[1]);
        union[2] = union[2].max(bbox[2]);
        union[3] = union[3].max(bbox[3]);
    }
    union
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

    /// Assemble a PDF from object bodies (numbered from 1), computing the xref like
    /// `crates/engine/src/tests.rs::fixture` (lengths and offsets, never counted by hand).
    fn assemble(objs: Vec<Vec<u8>>) -> Vec<u8> {
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

    /// A minimal one-page PDF, hand-written like `crates/engine/src/tests.rs::fixture`. The page
    /// content draws a form XObject (`/Fm1`), a DCTDecode image XObject (`/Im1`), a filled
    /// rectangle and one line of non-embedded Type 1 text (`/F1`); a square annotation's
    /// appearance stream draws one more rectangle.
    fn fixture() -> Vec<u8> {
        assemble(vec![
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
        ])
    }

    /// A two-page PDF: page 1 fills red near the bottom, page 2 fills blue near the top
    /// (distinct ink per page, so ordering assertions can tell them apart).
    fn two_page_pdf() -> Vec<u8> {
        assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << >> >>".to_vec(),
            stream_obj("", b"1 0 0 rg 20 40 160 60 re f"),
            stream_obj("", b"0 0 1 rg 20 120 160 60 re f"),
        ])
    }

    /// A one-page PDF that sets an `/BM /Multiply` ExtGState before each of two draws:
    /// the emitter sees the blend mode twice but must record the degradation once.
    fn blend_pdf() -> Vec<u8> {
        assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> >> >>".to_vec(),
            stream_obj("", b"q /GS1 gs 1 0 0 rg 20 40 160 60 re f Q\nq /GS1 gs 0 1 0 rg 20 120 160 60 re f Q"),
            b"<< /Type /ExtGState /BM /Multiply >>".to_vec(),
        ])
    }

    /// A one-page PDF that sets an `/ca 0.5` ExtGState before each of two draws: the
    /// emitter sees partial paint alpha twice but must record the degradation once.
    fn alpha_pdf() -> Vec<u8> {
        assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> >> >>".to_vec(),
            stream_obj("", b"q /GS1 gs 1 0 0 rg 20 40 160 60 re f Q\nq /GS1 gs 0 1 0 rg 20 120 160 60 re f Q"),
            b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
        ])
    }

    /// A one-page PDF drawing a form XObject whose resources include itself: left
    /// unbounded, `/Fm0 Do` recurses forever. The interpreter's nesting depth must
    /// bound it, so `export()` returns.
    fn deep_form_pdf() -> Vec<u8> {
        assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /XObject << /Fm0 4 0 R >> >> >>".to_vec(),
            stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /XObject << /Fm0 4 0 R >> >>", b"q /Fm0 Do Q"),
            stream_obj("", b"q /Fm0 Do Q 1 0 0 rg 10 10 50 50 re f"),
        ])
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

    // ------------------------------------------------------- `export()` assembly

    use super::{FidelityNote, PsExportError, PsFormat, export};

    /// An export's bytes as a string (the emitter's output is pure ASCII).
    fn text(out: &super::ExportedPs) -> String {
        String::from_utf8(out.bytes.clone()).expect("ASCII output")
    }

    /// A whole-document export carries the DSC structure end to end: header with the page
    /// count, one `%%Page` block per exported page in ascending order with duplicates
    /// collapsed, `showpage` per page, and the trailer after the last page.
    #[test]
    fn export_two_pages_has_pages_trailer_and_page_comments() {
        let bytes = std::sync::Arc::new(two_page_pdf());
        // Out-of-order request with a duplicate: output ascending, each page once.
        let out = export(std::sync::Arc::clone(&bytes), &[1, 0, 1], PsFormat::Ps).expect("valid request");
        let s = text(&out);
        assert!(s.starts_with("%!PS-Adobe-3.0\n"), "PS header: {s}");
        assert!(s.contains("%%Pages: 2\n"), "exported page count: {s}");
        assert!(s.contains("%%Page: 1 1\n%%PageBoundingBox: 0 0 200 300\n"), "page 1 block: {s}");
        assert!(s.contains("%%Page: 2 2\n%%PageBoundingBox: 0 0 200 300\n"), "page 2 block: {s}");
        assert_eq!(s.matches("showpage\n").count(), 2, "one showpage per page: {s}");
        let p1 = s.find("%%Page: 1 1").expect("page 1 present");
        let red = s.find("1 0 0 setrgbcolor").expect("page 1 ink");
        let p2 = s.find("%%Page: 2 2").expect("page 2 present");
        let blue = s.find("0 0 1 setrgbcolor").expect("page 2 ink");
        assert!(p1 < red && red < p2 && p2 < blue, "ascending pages, each with its own ink: {s}");
        assert!(!s.contains("%%BeginProlog"), "pages with no fonts and no separations define no prolog resources: {s}");
        assert!(s.ends_with("%%Trailer\n%%Pages: 2\n%%EOF\n"), "trailer closes the document: {s}");
        assert_eq!(out.warnings, Vec::<FidelityNote>::new(), "plain pages degrade nothing");
    }

    /// EPS output needs exactly one page: zero or two requested pages are actionable
    /// errors, and one page exports with the EPSF header, no `showpage` and the
    /// bounding-box approximation note.
    #[test]
    fn eps_requires_exactly_one_page() {
        let bytes = std::sync::Arc::new(two_page_pdf());
        let err = export(std::sync::Arc::clone(&bytes), &[0, 1], PsFormat::Eps).expect_err("two pages cannot be one EPS");
        assert_eq!(err, PsExportError::EpsNeedsExactlyOnePage { requested: 2 });
        let err = export(std::sync::Arc::clone(&bytes), &[], PsFormat::Eps).expect_err("EPS needs a page");
        assert_eq!(err, PsExportError::EpsNeedsExactlyOnePage { requested: 0 });

        let out = export(bytes, &[1], PsFormat::Eps).expect("one page exports as EPS");
        let s = text(&out);
        assert!(s.starts_with("%!PS-Adobe-3.0 EPSF-3.0\n"), "EPSF header: {s}");
        assert!(!s.contains("showpage"), "EPS never ejects the importing application's page: {s}");
        assert!(s.contains("%%Page: 1 1\n"), "the single page block: {s}");
        assert_eq!(
            out.warnings,
            vec![FidelityNote { page: None, text: "BoundingBox approximated from CropBox".into() }],
            "the CropBox approximation is disclosed"
        );
        assert!(s.contains("%%PdfCraftNote: BoundingBox approximated from CropBox\n"), "note embedded: {s}");
    }

    /// An out-of-range page index names the requested indices and the real page count,
    /// so a caller can act on exactly what was wrong.
    #[test]
    fn invalid_page_index_is_actionable() {
        let err = export(std::sync::Arc::new(fixture()), &[0, 3], PsFormat::Ps).expect_err("index 3 is out of range");
        assert_eq!(err, PsExportError::InvalidPages { requested: vec![0, 3], page_count: 1 });
    }

    /// The viewer configuration reaches the drive (finding: `postscript` used to pass
    /// only the bytes). View ▸ Hide all comments keeps the fixture's square annotation
    /// — a markup annotation whose appearance paints blue — out of the program, while
    /// the default config draws it; and `export` is exactly `export_with_config` over a
    /// default config, so the old API's behaviour is unchanged.
    #[test]
    fn config_reaches_the_drive_and_export_is_the_default_config() {
        // The fixture's passthrough DCT payload is binary, so read the program lossily.
        let lossy = |out: &super::ExportedPs| String::from_utf8_lossy(&out.bytes).into_owned();
        let bytes = std::sync::Arc::new(fixture());
        let plain = export(std::sync::Arc::clone(&bytes), &[0], PsFormat::Ps).expect("the default config exports");
        let default = super::export_with_config(std::sync::Arc::clone(&bytes), &pdfcraft_render::RenderConfig::default(), &[0], PsFormat::Ps)
            .expect("the default config exports");
        assert_eq!(plain, default, "export() is export_with_config over a default config");
        assert!(lossy(&plain).contains("0 0 1 setrgbcolor"), "the annotation's blue ink is drawn by default");

        let hidden = pdfcraft_render::RenderConfig { hide_comments: true, ..pdfcraft_render::RenderConfig::default() };
        let out = super::export_with_config(bytes, &hidden, &[0], PsFormat::Ps).expect("hidden comments export");
        let s = lossy(&out);
        assert!(!s.contains("0 0 1 setrgbcolor"), "the markup annotation is not drawn: {s}");
    }

    /// A `/BM /Multiply` ExtGState degrades to a fidelity note, not to pseudo-ops: the
    /// body still draws, the note is recorded once for the page however often the mode
    /// is set, and no transparency pseudo-operator appears.
    #[test]
    fn blend_mode_produces_note_not_ops() {
        let out = export(std::sync::Arc::new(blend_pdf()), &[0], PsFormat::Ps).expect("transparent content exports");
        let s = text(&out);
        assert_eq!(s.matches("fill\n").count(), 2, "both draws still emit: {s}");
        assert!(!s.contains("/Blend"), "no transparency pseudo-ops: {s}");
        assert_eq!(s.matches("Multiply").count(), 1, "the mode name appears once, in the note: {s}");
        assert_eq!(
            out.warnings,
            vec![FidelityNote { page: Some(1), text: "transparency ignored (blend Multiply)".into() }],
            "one note per page, first occurrence only"
        );
        assert!(s.contains("%%PdfCraftNote: transparency ignored (blend Multiply)\n"), "note embedded: {s}");
    }

    /// A `/ca 0.5` ExtGState degrades to one paint-alpha note per page however many
    /// draws carry it (the module's transparency policy: first occurrence only).
    #[test]
    fn paint_alpha_produces_one_note() {
        let out = export(std::sync::Arc::new(alpha_pdf()), &[0], PsFormat::Ps).expect("partially transparent content exports");
        let s = text(&out);
        assert_eq!(s.matches("fill\n").count(), 2, "both draws still emit: {s}");
        assert_eq!(
            out.warnings,
            vec![FidelityNote { page: Some(1), text: "transparency ignored (paint alpha)".into() }],
            "one note per page, first occurrence only"
        );
        assert_eq!(s.matches("%%PdfCraftNote: transparency ignored (paint alpha)\n").count(), 1, "note embedded once: {s}");
    }

    /// A self-referencing form XObject exports to completion (the interpreter's nesting
    /// depth bound holds) with a balanced graphics-state stack.
    #[test]
    fn deep_form_xobjects_complete() {
        let out = export(std::sync::Arc::new(deep_form_pdf()), &[0], PsFormat::Ps).expect("deep recursion is bounded");
        let s = text(&out);
        assert!(s.contains("fill\n"), "content after the form drew: {s}");
        assert!(s.ends_with("%%EOF\n"), "document closed: {s}");
        assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balance: {s}");
    }

    /// A one-page PDF drawing a backdrop, then a form XObject with a
    /// `/Group << /S /Transparency >>` under an ExtGState with `/ca 0.5`: the
    /// interpreter pushes a real-transparency group around the form's content, so
    /// the flattening runs on the real drive path.
    fn transparency_group_pdf() -> Vec<u8> {
        assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << /GS1 5 0 R >> /XObject << /Fm1 6 0 R >> >> >>".to_vec(),
            stream_obj("", b"1 0 0 rg 10 250 180 40 re f\nq /GS1 gs /Fm1 Do Q"),
            b"<< /Type /ExtGState /ca 0.5 >>".to_vec(),
            stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 200 300] /Group << /S /Transparency >>", b"0 0 1 rg 20 20 100 50 re f"),
        ])
    }

    /// The transparency flattening is format-independent: the opacity fixture exports
    /// as PS and as EPS with the same single raster image (the group's vector ops
    /// withheld and replaced) and the same disclosure note.
    #[test]
    fn eps_and_ps_share_flattening() {
        let bytes = std::sync::Arc::new(transparency_group_pdf());
        for format in [PsFormat::Ps, PsFormat::Eps] {
            let out = export(std::sync::Arc::clone(&bytes), &[0], format).expect("the fixture exports");
            let s = text(&out);
            assert_eq!(s.matches(">> image").count(), 1, "exactly one raster image, in both flavours ({format:?}): {s}");
            assert!(s.contains("/DeviceRGB setcolorspace\n"), "the opaque-raster pipeline ({format:?}): {s}");
            assert!(out.warnings.iter().any(|n| n.text == "transparency flattened to raster"), "disclosed ({format:?}): {:?}", out.warnings);
            assert!(s.contains("%%PdfCraftNote: transparency flattened to raster\n"), "the note travels in the file ({format:?}): {s}");
            assert_eq!(s.matches("gsave").count(), s.matches("grestore").count(), "gsave/grestore balance ({format:?}): {s}");
        }
    }

    /// An empty page selection yields the header and trailer only — still complete,
    /// valid DSC PostScript with a zero page count.
    #[test]
    fn empty_pages_slice_yields_header_only_valid_ps() {
        let out = export(std::sync::Arc::new(fixture()), &[], PsFormat::Ps).expect("empty selection exports");
        assert_eq!(
            out.bytes,
            b"%!PS-Adobe-3.0\n\
              %%BoundingBox: 0 0 0 0\n\
              %%Pages: 0\n\
              %%Creator: PdfCraft\n\
              %%EndComments\n\
              %%Trailer\n\
              %%Pages: 0\n\
              %%EOF\n"
                .to_vec(),
            "header + trailer, nothing between"
        );
        assert!(out.warnings.is_empty(), "nothing was driven, nothing degraded");
    }

    /// Export drives its pages with the raster renderer's font resolver (spec D3's "match
    /// `page_render` by construction"): [`super::interpreter_settings`] installs
    /// `pdfcraft_render::page_font_resolver`, whose craft-fonts Japanese fallback answers a
    /// non-embedded Adobe-Japan1 query with a real Japanese face where the bare default
    /// resolver answers with the standard-14 Latin substitute — the divergence that once
    /// made the oracle-side raster draw Japanese while the export vectorized Latin glyphs.
    /// Built without craft-fonts there is no Japanese face, both resolvers answer alike,
    /// and the parity holds that way too.
    #[test]
    fn export_uses_the_raster_renderers_font_resolver() {
        use hayro::hayro_interpret::font::{FallbackFontQuery, FontQuery};
        use hayro::hayro_interpret::hayro_cmap::{CharacterCollection, CidFamily};

        let query = FontQuery::Fallback(FallbackFontQuery {
            post_script_name: Some("HeiseiMin-W3".into()),
            character_collection: Some(CharacterCollection { family: CidFamily::AdobeJapan1, supplement: 2 }),
            ..FallbackFontQuery::default()
        });
        let as_bytes =
            |resolved: Option<(hayro::hayro_interpret::font::FontData, u32)>| resolved.map(|(data, index)| (data.as_ref().as_ref().to_vec(), index));
        let wired = as_bytes((super::interpreter_settings(&pdfcraft_render::RenderConfig::default()).font_resolver)(&query));
        let bare = as_bytes((InterpreterSettings::default().font_resolver)(&query));
        if pdfcraft_fonts::document_japanese_font().is_none() {
            eprintln!("built without craft-fonts (CRAFT_FONTS_DIR unset): both resolvers fall back alike");
            assert_eq!(wired, bare, "without craft-fonts the resolver must not change substitution");
            return;
        }
        let Some((face, _)) = wired else {
            panic!("the Adobe-Japan1 fallback query resolved to no face");
        };
        assert_ne!(Some((face.clone(), 0)), bare, "the wired resolver answers with the craft-fonts Japanese face, not the standard-14 substitute");
    }
}
