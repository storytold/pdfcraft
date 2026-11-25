//! Prepare and insert pages from clipboard content (copied pages, an image, or plain text).

use std::sync::Arc;

use crate::{DocId, Edit, EditError, Session, guard};

/// Largest side (pixels) accepted for a clipboard image.
pub const MAX_CLIPBOARD_IMAGE_SIDE: usize = 16_384;

/// Largest byte length accepted for a clipboard image (raw RGBA pixels or encoded image file).
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// Largest byte length accepted for clipboard plain text.
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Notice returned when the clipboard holds nothing that can become pages.
pub const EMPTY_CLIPBOARD_NOTICE: &str = "The clipboard has no pages, image or text to insert";

/// Content turned into PDF pages by `page.insert_clipboard` / `page_insert_clipboard`.
#[derive(Clone, Debug)]
pub enum ClipboardPages {
    /// Pages copied from a PDF (the same document or another one).
    Pages { bytes: Arc<Vec<u8>>, pages: Vec<usize>, field_conflicts: pdfcraft_organize::FieldConflicts },
    /// Raw RGBA8 pixels read from the system clipboard, row by row.
    RgbaImage { width: usize, height: usize, rgba: Vec<u8> },
    /// Encoded image bytes (PNG, JPEG, TIFF, GIF, BMP or JPEG 2000).
    Image { name: String, bytes: Vec<u8> },
    /// Plain UTF-8 text.
    Text(String),
}

/// Encode raw RGBA8 clipboard pixels as a PNG within [`MAX_CLIPBOARD_IMAGE_SIDE`] and
/// [`MAX_CLIPBOARD_IMAGE_BYTES`].
pub fn png_from_rgba(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, EditError> {
    if width > MAX_CLIPBOARD_IMAGE_SIDE || height > MAX_CLIPBOARD_IMAGE_SIDE || rgba.len() > MAX_CLIPBOARD_IMAGE_BYTES {
        return Err(EditError::Invalid("the clipboard image exceeds the size limit".into()));
    }
    let expected = width.checked_mul(height).and_then(|p| p.checked_mul(4));
    let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(EditError::Invalid("the clipboard image is empty or malformed".into()));
    };
    if w == 0 || h == 0 || expected != Some(rgba.len()) {
        return Err(EditError::Invalid("the clipboard image is empty or malformed".into()));
    }
    guard(|| {
        let img = craft_image_codecs::Image::from_u8(w, h, craft_image_codecs::ChannelLayout::Rgba, rgba.to_vec())
            .map_err(|e| EditError::Invalid(e.to_string()))?;
        craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default())
            .map_err(|e| EditError::Invalid(e.to_string()))
    })
    .map_err(|_| EditError::Source("the clipboard image could not be encoded".into()))?
}

impl Session {
    /// Convert `source` into one [`Edit::InsertPagesFrom`] labelled `"Insert pages from clipboard"`,
    /// returning the edit and the number of pages it will insert.
    pub fn prepare_clipboard_insert(&self, source: ClipboardPages, at: usize) -> Result<(Edit, usize), EditError> {
        match source {
            ClipboardPages::Pages { bytes, pages, field_conflicts } => {
                if pages.is_empty() {
                    return Err(EditError::Invalid(EMPTY_CLIPBOARD_NOTICE.into()));
                }
                let count = pages.len();
                Ok((Edit::InsertPagesFrom { name: "clipboard".into(), bytes, pages: Some(pages), at, field_conflicts }, count))
            }
            ClipboardPages::RgbaImage { width, height, rgba } => {
                let png = png_from_rgba(width, height, &rgba)?;
                let bytes = guard(|| self.create_from_images(&[("Clipboard.png".into(), png)]))
                    .map_err(|_| EditError::Source("the clipboard image could not be read".into()))??;
                let count = self.page_count_of("Clipboard.pdf", &bytes)?;
                Ok((
                    Edit::InsertPagesFrom {
                        name: "clipboard".into(),
                        bytes,
                        pages: None,
                        at,
                        field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
                    },
                    count,
                ))
            }
            ClipboardPages::Image { name, bytes } => {
                if bytes.is_empty() {
                    return Err(EditError::Invalid(EMPTY_CLIPBOARD_NOTICE.into()));
                }
                if bytes.len() > MAX_CLIPBOARD_IMAGE_BYTES {
                    return Err(EditError::Invalid("the clipboard image exceeds the size limit".into()));
                }
                let bytes = guard(|| self.create_from_images(&[(name, bytes)]))
                    .map_err(|_| EditError::Source("the clipboard image could not be read".into()))??;
                let count = self.page_count_of("Clipboard.pdf", &bytes)?;
                Ok((
                    Edit::InsertPagesFrom {
                        name: "clipboard".into(),
                        bytes,
                        pages: None,
                        at,
                        field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
                    },
                    count,
                ))
            }
            ClipboardPages::Text(text) => {
                if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
                    return Err(EditError::Invalid("the clipboard text exceeds the size limit".into()));
                }
                if text.trim().is_empty() {
                    return Err(EditError::Invalid(EMPTY_CLIPBOARD_NOTICE.into()));
                }
                let bytes = self.create_from_text("Clipboard", &text)?;
                let count = self.page_count_of("Clipboard.pdf", &bytes)?;
                Ok((
                    Edit::InsertPagesFrom {
                        name: "clipboard".into(),
                        bytes,
                        pages: None,
                        at,
                        field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
                    },
                    count,
                ))
            }
        }
    }

    /// Insert pages from `source` into document `id` at 0-based page index `at`, returning how
    /// many pages were inserted. Respects document assembly permissions and records one undoable
    /// `"Insert pages from clipboard"` edit.
    pub fn insert_clipboard_pages(&mut self, id: DocId, source: ClipboardPages, at: usize) -> Result<usize, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if !doc.allows_assembly() {
            return Err(EditError::NotPermitted("page assembly"));
        }
        let (edit, count) = self.prepare_clipboard_insert(source, at)?;
        self.apply(id, edit)?;
        Ok(count)
    }
}
