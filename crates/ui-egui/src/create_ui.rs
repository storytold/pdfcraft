//! Create a PDF (blank, from images, from text) and Reduce File Size (execution plan M10.2,
//! M11.1). Opening an image or a text file converts it to a new, unsaved PDF, as Acrobat does.

use std::sync::Arc;

use crate::PdfCraftApp;

/// File types Open accepts besides PDF (converted on open).
pub const CONVERTIBLE: [&str; 12] = ["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx", "txt", "text"];

fn is_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8])
        || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"II*\0")
        || bytes.starts_with(b"MM\0*")
        || bytes.starts_with(b"GIF8")
        // JPEG 2000: a JP2 file or a raw codestream.
        || bytes.starts_with(&[0, 0, 0, 0x0C, b'j', b'P', b' ', b' '])
        || bytes.starts_with(&[0xFF, 0x4F, 0xFF, 0x51])
        // BMP: "BM" and a known header size (so text starting with "BM" stays text).
        || (bytes.starts_with(b"BM") && bytes.get(14..18).is_some_and(|h| matches!(u32::from_le_bytes([h[0], h[1], h[2], h[3]]), 12 | 40 | 52 | 56 | 108 | 124)))
}

/// What Create ▸ Clipboard found on the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub enum Clip {
    /// RGBA pixels, row by row.
    Image {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    Text(String),
}

/// An image on the system clipboard, or else its text.
#[cfg(not(target_arch = "wasm32"))]
fn read_clipboard() -> Option<Clip> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if let Ok(img) = cb.get_image() {
        return Some(Clip::Image { width: img.width, height: img.height, rgba: img.bytes.into_owned() });
    }
    cb.get_text().ok().filter(|t| !t.trim().is_empty()).map(Clip::Text)
}

fn png_from_rgba(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h) = (u32::try_from(width).map_err(|e| e.to_string())?, u32::try_from(height).map_err(|e| e.to_string())?);
    if w == 0 || h == 0 || rgba.len() != width * height * 4 {
        return Err("the clipboard image is empty or malformed".into());
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

impl PdfCraftApp {
    /// Convert a non-PDF file (image, text) into a new tab. Returns `None` when `bytes` is not
    /// something Create understands (the caller then tries to open it as a PDF).
    pub(crate) fn open_converted(&mut self, name: &str, bytes: &[u8]) -> Option<Result<(), String>> {
        let head = &bytes[..bytes.len().min(1024)];
        if head.windows(5).any(|w| w == b"%PDF-") {
            return None;
        }
        let created = if is_image(bytes) {
            self.session.create_from_images(&[(name.to_string(), bytes.to_vec())])
        } else if name.to_ascii_lowercase().ends_with(".txt") {
            let text = String::from_utf8_lossy(bytes);
            self.session.create_from_text(stem(name), &text)
        } else {
            return None;
        };
        Some(self.open_created_bytes(&format!("{}.pdf", stem(name)), created.map_err(|e| e.to_string())))
    }

    fn open_created_bytes(&mut self, name: &str, created: Result<Arc<Vec<u8>>, String>) -> Result<(), String> {
        let bytes = created?;
        let id = self.session.open_new(name, bytes).map_err(|e| e.to_string())?;
        let info = &self.session.get(id).ok_or("the new document could not be opened")?.info;
        self.views.push(crate::DocView::new(id, info));
        self.active = Some(self.views.len() - 1);
        Ok(())
    }

    /// Create ▸ Clipboard: a new document from the image (one page, its size) or the text on
    /// the clipboard, as Acrobat does.
    pub(crate) fn create_from_clipboard(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        let clip = read_clipboard();
        #[cfg(target_arch = "wasm32")]
        let clip: Option<Clip> = None;
        match clip {
            Some(c) => {
                if let Err(e) = self.create_from_clip(c) {
                    self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
                }
            }
            None => self.notify_tr("The clipboard has no image or text"),
        }
    }

    /// Create a new document from clipboard contents.
    pub fn create_from_clip(&mut self, clip: Clip) -> Result<(), String> {
        let created = match clip {
            Clip::Image { width, height, rgba } => {
                let png = png_from_rgba(width, height, &rgba)?;
                self.session.create_from_images(&[("Clipboard.png".into(), png)])
            }
            Clip::Text(t) => self.session.create_from_text("Clipboard", &t),
        };
        self.open_created_bytes("Clipboard.pdf", created.map_err(|e| e.to_string()))
    }

    /// Create ▸ Blank page: a new untitled US Letter document.
    pub(crate) fn create_blank(&mut self) {
        let created = self.session.create_blank(612.0, 792.0, 1).map_err(|e| e.to_string());
        if let Err(e) = self.open_created_bytes("Untitled.pdf", created) {
            self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
        }
    }

    /// Create ▸ Images: several images, one page each, in one new document.
    pub(crate) fn create_from_images_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(files) = rfd::FileDialog::new()
                .add_filter(tl!("Images"), &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"])
                .set_title(tl!("Choose images"))
                .pick_files()
            else {
                return;
            };
            let mut images = Vec::new();
            for f in files {
                match std::fs::read(&f) {
                    Ok(b) => images.push((f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), b)),
                    Err(e) => {
                        self.notify_fmt("Couldn't read {name}: {e}", &[("name", &f.display().to_string()), ("e", &e.to_string())]);
                        return;
                    }
                }
            }
            self.create_from_images(images);
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("On the web, open or drop an image to convert it");
    }

    /// One new document from images (tests and automation call this directly).
    pub fn create_from_images(&mut self, images: Vec<(String, Vec<u8>)>) {
        if images.is_empty() {
            return;
        }
        let name = if images.len() == 1 { format!("{}.pdf", stem(&images[0].0)) } else { "Images.pdf".to_string() };
        let created = self.session.create_from_images(&images).map_err(|e| e.to_string());
        if let Err(e) = self.open_created_bytes(&name, created) {
            self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
        }
    }

    /// Reduce File Size: write a compacted copy with images downsampled (the open document is
    /// unchanged).
    pub(crate) fn reduce_file_size(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let result = self.session.reduced_bytes(id).map(|(b, _)| (b, String::new()));
        self.save_optimized(id, "reduced", result);
    }

    /// Save an optimized copy (Reduce File Size, Optimize PDF) next to the original, reporting
    /// the saving.
    pub(crate) fn save_optimized(
        &mut self,
        id: pdfcraft_engine::DocId,
        suffix: &str,
        result: Result<(Arc<Vec<u8>>, String), pdfcraft_engine::EditError>,
    ) {
        let Some(doc) = self.session.get(id) else { return };
        let (before, name) = (doc.bytes.len(), format!("{} ({suffix}).pdf", stem(&doc.name)));
        let (bytes, detail) = match result {
            Ok(r) => r,
            Err(e) => {
                self.notify_fmt("Couldn't optimize the file: {e}", &[("e", &e.to_string())]);
                return;
            }
        };
        let saved = |app: &mut PdfCraftApp, place: String| {
            let pct = 100.0 * (1.0 - bytes.len() as f64 / before.max(1) as f64);
            app.notify_fmt(
                "Saved {place}: {before} → {after} ({pct}% smaller){detail}",
                &[
                    ("place", &place),
                    ("before", &crate::panels::human_size(before)),
                    ("after", &crate::panels::human_size(bytes.len())),
                    ("pct", &format!("{pct:.0}")),
                    ("detail", &detail),
                ],
            );
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = match &self.save_override {
                Some(p) => Some(p.clone()),
                None => rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(&name).save_file().map(|p| p.to_string_lossy().into_owned()),
            };
            let Some(path) = path else { return };
            match crate::editing::write_atomically(&path, &bytes) {
                Ok(()) => saved(self, path),
                Err(e) => self.notify_fmt("Couldn't write {path}: {e}", &[("path", &path), ("e", &e.to_string())]),
            }
        }
        #[cfg(target_arch = "wasm32")]
        match crate::editing::download(&name, &bytes) {
            Ok(()) => saved(self, name),
            Err(e) => self.notify_fmt("Couldn't download {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
        }
    }
}
