//! Export a PDF ▸ Image and Text (execution plan M10.3, the first formats).
//!
//! [`Exporter`] renders pages to PNG, JPEG or TIFF at a resolution and extracts reading-order
//! text, from a
//! document's working file (so unsaved edits and hidden layers are respected, as on screen).

use printcraft_render::{PageRenderer, RenderRequest, RequestKind};

use crate::Document;

/// Renders and extracts pages of one document state.
pub struct Exporter {
    renderer: PageRenderer,
    pages: usize,
}

/// What an export needs from a document, as plain values that can move to a worker thread.
#[derive(Clone)]
pub struct ExportSource {
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub config: printcraft_render::RenderConfig,
    pub pages: usize,
}

impl Document {
    /// The current state, for exporting on another thread.
    pub fn export_source(&self) -> ExportSource {
        ExportSource { bytes: self.bytes.clone(), config: self.config.clone(), pages: self.info.pages.len() }
    }
}

/// Export all images: the images `pages` (0-based) use, each once, skipping those under
/// `min_side` pixels on their shorter side. JPEGs come out unchanged, other images as PNG.
///
/// # Errors
///
/// When `src`'s bytes cannot be opened as a PDF (a `CosError` message): they are not a PDF, they
/// need a password `src` does not carry, or the structure cannot be read.
pub fn extract_images(src: &ExportSource, pages: &[usize], min_side: u32) -> Result<printcraft_create::ImageExport, String> {
    let doc = printcraft_cos::Document::open_with_password(src.bytes.clone(), src.config.password.as_deref()).map_err(|e| e.to_string())?;
    Ok(printcraft_create::extract_images(&doc, pages, min_side))
}

/// The file name for the `index`-th (1-based) exported image: `<stem>_Page_<n>_Image_<index>.<ext>`.
#[must_use]
pub fn image_file_name(stem: &str, image: &printcraft_create::ExtractedImage, index: usize) -> String {
    format!("{stem}_Page_{}_Image_{index:04}.{}", image.page + 1, image.extension)
}

impl Exporter {
    pub fn new(doc: &Document) -> Self {
        Self::from_source(doc.export_source())
    }

    #[must_use]
    pub fn from_source(src: ExportSource) -> Self {
        Self { renderer: PageRenderer::new(src.bytes, src.config), pages: src.pages }
    }

    fn check(&self, page: usize) -> Result<(), String> {
        if page < self.pages { Ok(()) } else { Err(format!("page {} does not exist", page + 1)) }
    }

    /// Page `page` (0-based) as a PNG at `dpi` (capped by the renderer's size limits).
    ///
    /// # Errors
    ///
    /// When `page` is out of range, the renderer reports an error for the page, or `encode_png`
    /// fails (zero dimensions, or `rgba` not `width * height * 4` bytes long).
    pub fn png(&mut self, page: usize, dpi: f64) -> Result<Vec<u8>, String> {
        self.check(page)?;
        let r = self.renderer.render(RenderRequest { page, scale: (dpi.clamp(18.0, 1200.0) / 72.0) as f32, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("page {}: {e}", page + 1));
        }
        encode_png(r.width, r.height, &r.rgba)
    }

    /// Page `page` as an image file of `format`.
    ///
    /// # Errors
    ///
    /// When `page` is out of range, the renderer reports an error for the page, or the chosen
    /// encoder rejects the image: zero dimensions (PNG, JPEG and TIFF), a `premultiplied` buffer
    /// that is not `width * height * 4` bytes long (PNG), or a `width` or `height` above 65535
    /// (JPEG).
    pub fn image(&mut self, page: usize, dpi: f64, format: ImageFormat) -> Result<Vec<u8>, String> {
        self.check(page)?;
        let r = self.renderer.render(RenderRequest { page, scale: (dpi.clamp(18.0, 1200.0) / 72.0) as f32, ..Default::default() });
        if let Some(e) = r.error {
            return Err(format!("page {}: {e}", page + 1));
        }
        match format {
            ImageFormat::Png => encode_png(r.width, r.height, &r.rgba),
            ImageFormat::Jpeg { quality } => encode_jpeg(r.width, r.height, &r.rgba, quality),
            ImageFormat::Tiff => encode_tiff(r.width, r.height, &r.rgba),
        }
    }

    /// The reading-order text of a page.
    ///
    /// # Errors
    ///
    /// When `page` is out of range, the renderer reports an error for the page, or the page has no
    /// text layer.
    pub fn text(&mut self, page: usize) -> Result<String, String> {
        self.check(page)?;
        let r = self.renderer.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
        match r.text {
            Some(t) => Ok(t.plain_text()),
            None => Err(format!("page {}: {}", page + 1, r.error.unwrap_or_else(|| "no text layer".into()))),
        }
    }

    /// The text of several pages, separated by form feeds (as `pdftotext` does).
    ///
    /// # Errors
    ///
    /// When any page in `pages` fails as [`Exporter::text`] would: it is out of range, fails to
    /// render, or has no text layer. The first failing page stops the extraction.
    pub fn text_of(&mut self, pages: &[usize]) -> Result<String, String> {
        let mut out = String::new();
        for (k, p) in pages.iter().enumerate() {
            if k > 0 {
                out.push('\u{c}');
            }
            out.push_str(self.text(*p)?.trim_end());
            out.push('\n');
        }
        Ok(out)
    }
}

/// Premultiplied RGBA → PNG (straight alpha).
///
/// # Errors
///
/// When `width` or `height` is zero, `premultiplied` is not `width * height * 4` bytes long, or
/// writing the header or pixel data fails.
pub fn encode_png(width: u32, height: u32, premultiplied: &[u8]) -> Result<Vec<u8>, String> {
    let mut rgba = premultiplied.to_vec();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Image file formats for Export ▸ Image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    /// Quality 1–100.
    Jpeg {
        quality: u8,
    },
    Tiff,
}

impl ImageFormat {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg { .. } => "jpg",
            ImageFormat::Tiff => "tif",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg { .. } => "JPEG",
            ImageFormat::Tiff => "TIFF",
        }
    }
}

/// Premultiplied RGBA composited over white → RGB.
fn over_white(premultiplied: &[u8]) -> Vec<u8> {
    premultiplied
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let k = 255 - p[3];
            [p[0].saturating_add(k), p[1].saturating_add(k), p[2].saturating_add(k)]
        })
        .collect()
}

/// Premultiplied RGBA → baseline JPEG (pages are opaque: transparency becomes white paper).
///
/// # Errors
///
/// When `width` or `height` is zero or above 65535 (the JPEG limit), or writing the compressed
/// data fails.
pub fn encode_jpeg(width: u32, height: u32, premultiplied: &[u8], quality: u8) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let rgb = over_white(premultiplied);
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
        .write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Premultiplied RGBA → TIFF (RGB, LZW-compressed by the encoder's default).
///
/// # Errors
///
/// When `width` or `height` is zero, or the TIFF encoder cannot write the image data.
pub fn encode_tiff(width: u32, height: u32, premultiplied: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let rgb = over_white(premultiplied);
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::tiff::TiffEncoder::new(&mut out).write_image(&rgb, width, height, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}
