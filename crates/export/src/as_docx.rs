//! Word (.docx) through craft-docx (storytold/craft-libs#793): the pages are mapped onto a
//! `craft_wordmodel::Document` ([`super::as_word`]); pages that can't flow keep their layout as
//! positioned text boxes and pictures.

use super::as_word::{Meta, word_doc};
use super::layout::{Image, Page};
pub(crate) fn image_mime(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// A colour's 8-bit sRGB components (NaN and out-of-range values clamp; NaN reads as 0).
pub(crate) fn rgb8(c: [f64; 3]) -> [u8; 3] {
    // The value is clamped to 0..=255 before the cast, so it cannot truncate.
    c.map(|v| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 })
}

/// The colour as `RRGGBB`, or `None` for black (the default every format already uses).
pub(crate) fn hex(c: [f64; 3]) -> Option<String> {
    let [r, g, b] = rgb8(c);
    (r | g | b != 0).then(|| format!("{r:02X}{g:02X}{b:02X}"))
}

/// Most images a Word export embeds, and most image bytes (the zip writer has 32-bit sizes and
/// a 16-bit entry count; a hostile PDF must not be able to overflow them).
pub(crate) const MAX_MEDIA: usize = 1000;

const MAX_MEDIA_BYTES: usize = 512 << 20;

/// Whether `im` can still be embedded: it has data, and the package stays within its limits.
pub(crate) fn room_for(media: &[(String, &Image)], im: &Image) -> bool {
    !im.bytes.is_empty()
        && media.len() < MAX_MEDIA
        && media.iter().map(|(_, m)| m.bytes.len()).fold(im.bytes.len(), usize::saturating_add) <= MAX_MEDIA_BYTES
}

/// The body text's font. A reader without Arial draws it in a sans-serif: the font table says
/// it is one (Word and LibreOffice substitute their own sans, LibreOffice Liberation Sans).
const FONT: &str = "Arial";

/// The application `docProps/app.xml` names.
const APPLICATION: &str = "PdfCraft";

/// A Word document (.docx, Office Open XML). Ordinary pages flow as paragraphs and tables;
/// pages with rotated text, or text that overlaps artwork, use floating shapes to retain their layout.
pub fn docx(pages: &[Page], title: &str) -> Vec<u8> {
    docx_with_language(pages, title, None)
}

/// [`docx`] for a document in `language` (a BCP 47 tag such as `bg-BG`, as a PDF's `/Lang` gives
/// it): the default language of its text, so Word checks the spelling in that language rather
/// than in its own. A language that is not a well formed tag is left out.
pub fn docx_with_language(pages: &[Page], title: &str, language: Option<&str>) -> Vec<u8> {
    craft_docx::set_application_name(APPLICATION);
    // Writing a document built here fails only on a hostile size the mapping already bounds; an
    // empty result is what the export returned for a package it couldn't finish.
    craft_docx::write(&word_doc(pages, &Meta { title, language, font: FONT })).unwrap_or_default()
}
