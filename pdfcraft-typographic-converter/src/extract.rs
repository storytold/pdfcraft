use crate::layout::{ExtractedPageData, PageImage};
use anyhow::{Context, Result};
use pdfium_render::prelude::{
    PdfDocument, PdfFontWeight, PdfPageObject, PdfPageObjectCommon, PdfPageObjectsCommon, PdfPageText, PdfPageTextChar, PdfPageTextObject, Pdfium,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Tuple of `(page_index, width, height, glyphs)` returned by page extraction helpers.
pub type PageGlyphs = (usize, f32, f32, Vec<RawGlyph>);

/// Extracted typographic information for an individual character/glyph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawGlyph {
    pub char_code: char,
    pub font_name: String,
    pub font_size: f32,
    pub is_bold: bool,
    pub is_italic: bool,
    pub bounds: (f32, f32, f32, f32), // (left, top, right, bottom)
    pub origin_x: f32,
    pub origin_y: f32,
}

fn bind_explicit_pdfium_path(path: &Path, source: &str) -> Result<Pdfium> {
    if !path.is_file() {
        anyhow::bail!("PDFium dynamic library from {source} does not exist or is not a file: {}", path.display());
    }
    let bindings = Pdfium::bind_to_library(path)
        .map_err(|err| anyhow::anyhow!("Failed to bind PDFium dynamic library from {source} ({}): {err}", path.display()))?;
    Ok(Pdfium::new(bindings))
}

/// Initialize PDFium using an explicit path, environment override, or system library.
///
/// Resolution order (never probes the current working directory):
/// 1. Explicit `custom_path` passed by the caller (`--pdfium-path`)
/// 2. `PDFIUM_DYNAMIC_LIB_PATH` environment variable (falling back to `PDFIUM_PATH`)
/// 3. Standard system library loading (`Pdfium::bind_to_system_library`)
pub fn init_pdfium_with_path(custom_path: Option<&Path>) -> Result<Pdfium> {
    // 1. Explicit path passed by caller
    if let Some(path) = custom_path {
        return bind_explicit_pdfium_path(path, "--pdfium-path");
    }

    // 2. Explicit environment variable override
    for var_name in ["PDFIUM_DYNAMIC_LIB_PATH", "PDFIUM_PATH"] {
        if let Ok(env_path) = std::env::var(var_name) {
            let trimmed = env_path.trim();
            if !trimmed.is_empty() {
                return bind_explicit_pdfium_path(Path::new(trimmed), var_name);
            }
        }
    }

    // 3. Standard system library loading
    if let Ok(bindings) = Pdfium::bind_to_system_library() {
        return Ok(Pdfium::new(bindings));
    }

    let lib_name = Pdfium::pdfium_platform_library_name();
    anyhow::bail!(
        "Failed to initialize PDFium: could not bind system PDFium ({:?}). \
        Set PDFIUM_DYNAMIC_LIB_PATH or pass --pdfium-path.",
        lib_name
    )
}

/// Convenience function to initialize PDFium using default discovery paths.
pub fn init_pdfium() -> Result<Pdfium> {
    init_pdfium_with_path(None)
}

/// Extracts glyphs from a PDF file located at `pdf_path`.
pub fn extract_glyphs<P: AsRef<Path>>(pdf_path: P) -> Result<Vec<RawGlyph>> {
    let pdfium = init_pdfium()?;
    extract_glyphs_with_pdfium(&pdfium, pdf_path)
}

/// Extracts full page data (glyphs + embedded images like barcodes/QRs) grouped by page.
pub fn extract_pages_full<P: AsRef<Path>>(pdf_path: P) -> Result<Vec<ExtractedPageData>> {
    let pdfium = init_pdfium()?;
    extract_pages_full_with_pdfium(&pdfium, pdf_path)
}

/// Extracts glyphs grouped by page from a PDF file located at `pdf_path`.
/// Returns `Vec<(page_index, width, height, Vec<RawGlyph>)>`.
pub fn extract_pages<P: AsRef<Path>>(pdf_path: P) -> Result<Vec<PageGlyphs>> {
    let pdfium = init_pdfium()?;
    extract_pages_with_pdfium(&pdfium, pdf_path)
}

/// Extracts glyphs from a PDF file using an existing `Pdfium` instance.
pub fn extract_glyphs_with_pdfium<P: AsRef<Path>>(pdfium: &Pdfium, pdf_path: P) -> Result<Vec<RawGlyph>> {
    let pages = extract_pages_with_pdfium(pdfium, pdf_path)?;
    Ok(pages.into_iter().flat_map(|(_, _, _, glyphs)| glyphs).collect())
}

/// Extracts glyphs grouped by page using an existing `Pdfium` instance.
/// Returns `Vec<(page_index, width, height, Vec<RawGlyph>)>`.
pub fn extract_pages_with_pdfium<P: AsRef<Path>>(pdfium: &Pdfium, pdf_path: P) -> Result<Vec<PageGlyphs>> {
    let pages = extract_pages_full_with_pdfium(pdfium, pdf_path)?;
    Ok(pages.into_iter().map(|p| (p.page_index, p.width, p.height, p.glyphs)).collect())
}

fn is_xml_valid_char(c: char) -> bool {
    matches!(
        c,
        '\u{0009}' | '\u{000A}' | '\u{000D}' | '\u{0020}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}

const MAX_FORM_VISITS: usize = 1_000;
const MAX_PAGE_IMAGE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Default)]
struct PageExtractAcc {
    glyphs: Vec<RawGlyph>,
    images: Vec<PageImage>,
    found_text_object: bool,
    form_visits: usize,
    image_bytes: usize,
    skipped_images: usize,
}

fn extract_text_object(text_object: &PdfPageTextObject<'_>, text_page_opt: Option<&PdfPageText<'_>>, acc: &mut PageExtractAcc) {
    acc.found_text_object = true;
    let Some(text_page) = text_page_opt else {
        return;
    };
    let Ok(chars) = text_page.chars_for_object(text_object) else {
        return;
    };
    for ch in chars.iter() {
        let glyph = extract_single_glyph(&ch, Some(text_object));
        if is_xml_valid_char(glyph.char_code) {
            acc.glyphs.push(glyph);
        }
    }
}

fn extract_image_object(object: &PdfPageObject<'_>, document: &PdfDocument<'_>, acc: &mut PageExtractAcc) {
    let Some(image_object) = object.as_image_object() else {
        return;
    };
    let (left, top, right, bottom) = match object.bounds() {
        Ok(quad) => {
            let r = quad.to_rect();
            (r.left().value, r.top().value, r.right().value, r.bottom().value)
        }
        Err(_) => (0.0, 0.0, 0.0, 0.0),
    };
    let width_pt = (right - left).abs();
    let height_pt = (top - bottom).abs();

    let mut bytes = image_object.get_raw_image_data().unwrap_or_default();
    let is_png = bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    let is_jpeg = bytes.starts_with(&[0xFF, 0xD8, 0xFF]);

    if !is_png && !is_jpeg {
        bytes = image_object
            .get_processed_bitmap(document)
            .ok()
            .and_then(|bitmap| {
                let w = bitmap.width() as u32;
                let h = bitmap.height() as u32;
                if w > 0 && h > 0 {
                    encode_png_exact(w, h, &bitmap.as_rgba_bytes()).ok()
                } else {
                    None
                }
            })
            .unwrap_or_default();
    }

    if bytes.is_empty() {
        return;
    }

    let next_total = acc.image_bytes.saturating_add(bytes.len());
    if next_total <= MAX_PAGE_IMAGE_BYTES {
        acc.image_bytes = next_total;
        acc.images.push(PageImage { bytes, bounds: (left, top, right, bottom), width_pt, height_pt });
    } else {
        acc.skipped_images = acc.skipped_images.saturating_add(1);
    }
}

fn extract_object_content(
    object: &PdfPageObject<'_>,
    text_page_opt: Option<&PdfPageText<'_>>,
    document: &PdfDocument<'_>,
    acc: &mut PageExtractAcc,
) -> Result<()> {
    if let Some(text_object) = object.as_text_object() {
        extract_text_object(text_object, text_page_opt, acc);
    } else if object.as_image_object().is_some() {
        extract_image_object(object, document, acc);
    } else if let Some(form_object) = object.as_x_object_form_object() {
        acc.form_visits = acc.form_visits.saturating_add(1);
        if acc.form_visits > MAX_FORM_VISITS {
            anyhow::bail!("Exceeded maximum nested form XObject visit limit ({MAX_FORM_VISITS}) while extracting page");
        }
        for child in form_object.iter() {
            extract_object_content(&child, text_page_opt, document, acc)?;
        }
    }
    Ok(())
}

fn extract_fallback_text_page(text_page_opt: Option<&PdfPageText<'_>>, acc: &mut PageExtractAcc) {
    if acc.found_text_object && !acc.glyphs.is_empty() {
        return;
    }
    let Some(text_page) = text_page_opt else {
        return;
    };
    if text_page.is_empty() {
        return;
    }
    for ch in text_page.chars().iter() {
        let glyph = extract_single_glyph(&ch, None);
        if is_xml_valid_char(glyph.char_code) {
            acc.glyphs.push(glyph);
        }
    }
}

/// Extracts full page data (glyphs + images) grouped by page using an existing `Pdfium` instance.
pub fn extract_pages_full_with_pdfium<P: AsRef<Path>>(pdfium: &Pdfium, pdf_path: P) -> Result<Vec<ExtractedPageData>> {
    let path_ref = pdf_path.as_ref();
    if !path_ref.exists() {
        anyhow::bail!("Input PDF file not found: {}", path_ref.display());
    }

    let document = pdfium.load_pdf_from_file(path_ref, None).with_context(|| format!("Failed to load PDF document from {}", path_ref.display()))?;

    let mut pages_result = Vec::new();

    for (page_index, page) in document.pages().iter().enumerate() {
        let width = page.width().value;
        let height = page.height().value;
        let page_objects = page.objects();
        let text_page_opt = page.text().ok();
        let mut acc = PageExtractAcc::default();

        for object in page_objects.iter() {
            extract_object_content(&object, text_page_opt.as_ref(), &document, &mut acc)?;
        }

        // Fallback when page has characters in text_page without accessible text objects.
        extract_fallback_text_page(text_page_opt.as_ref(), &mut acc);

        pages_result.push(ExtractedPageData {
            page_index,
            width,
            height,
            glyphs: acc.glyphs,
            images: acc.images,
            skipped_images: acc.skipped_images,
        });
    }

    Ok(pages_result)
}

/// Encode RGBA buffer into a PNG byte stream using the `png` crate.
fn encode_png_exact(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Extracts all required typographic attributes for a single character glyph.
fn extract_single_glyph(ch: &PdfPageTextChar, text_object: Option<&PdfPageTextObject>) -> RawGlyph {
    let char_code = ch.unicode_char().unwrap_or_else(|| char::from_u32(ch.unicode_value()).unwrap_or('\0'));
    let font_name = resolve_font_name(ch, text_object);
    let font_size = resolve_font_size(ch, text_object);
    let is_bold = determine_is_bold(ch, text_object, &font_name);
    let is_italic = determine_is_italic(ch, text_object, &font_name);

    let bounds = ch
        .tight_bounds()
        .or_else(|_| ch.loose_bounds())
        .map(|rect| (rect.left().value, rect.top().value, rect.right().value, rect.bottom().value))
        .unwrap_or_else(|_| {
            text_object
                .and_then(|obj| obj.bounds().ok())
                .map(|r| (r.left().value, r.top().value, r.right().value, r.bottom().value))
                .unwrap_or((0.0, 0.0, 0.0, 0.0))
        });

    let (origin_x, origin_y) = match ch.origin() {
        Ok((ox, oy)) => (ox.value, oy.value),
        Err(_) => {
            let ox = ch.origin_x().map(|p| p.value).unwrap_or(bounds.0);
            let oy = ch.origin_y().map(|p| p.value).unwrap_or(bounds.3);
            (ox, oy)
        }
    };

    RawGlyph { char_code, font_name, font_size, is_bold, is_italic, bounds, origin_x, origin_y }
}

fn resolve_font_name(ch: &PdfPageTextChar, text_object: Option<&PdfPageTextObject>) -> String {
    let name = ch.font_name();
    if !name.is_empty() {
        return name;
    }
    if let Some(obj) = text_object {
        let obj_name = obj.font().name();
        if !obj_name.is_empty() {
            return obj_name;
        }
    }
    "Unknown".to_string()
}

fn resolve_font_size(ch: &PdfPageTextChar, text_object: Option<&PdfPageTextObject>) -> f32 {
    let mut font_size = ch.scaled_font_size().value;
    if font_size <= 0.0 {
        font_size = ch.unscaled_font_size().value;
    }
    if font_size <= 0.0 {
        if let Some(obj) = text_object {
            font_size = obj.scaled_font_size().value;
            if font_size <= 0.0 {
                font_size = obj.unscaled_font_size().value;
            }
        }
    }
    font_size.max(0.0)
}

fn weight_is_bold(weight: PdfFontWeight) -> bool {
    match weight {
        PdfFontWeight::Weight600 | PdfFontWeight::Weight700Bold | PdfFontWeight::Weight800 | PdfFontWeight::Weight900 => true,
        PdfFontWeight::Custom(w) => w >= 600,
        _ => false,
    }
}

/// Determines whether a glyph has a bold typeface using character properties,
/// font descriptor flags, font weight, parent text object, and font name heuristics.
fn determine_is_bold(ch: &PdfPageTextChar, text_object: Option<&PdfPageTextObject>, font_name: &str) -> bool {
    if ch.font_weight().is_some_and(weight_is_bold) || ch.font_is_bold_reenforced() {
        return true;
    }

    if let Some(obj) = text_object {
        let font = obj.font();
        if font.weight().is_ok_and(weight_is_bold) || font.is_bold_reenforced() {
            return true;
        }
    }

    let lower_name = font_name.to_lowercase();
    lower_name.contains("bold")
        || lower_name.contains("black")
        || lower_name.contains("heavy")
        || lower_name.contains("semibold")
        || lower_name.contains("demibold")
}

/// Determines whether a glyph has an italic/slanted typeface using character properties,
/// font flags, parent text object font angles, and font name heuristics.
fn determine_is_italic(ch: &PdfPageTextChar, text_object: Option<&PdfPageTextObject>, font_name: &str) -> bool {
    if ch.font_is_italic() {
        return true;
    }

    if let Some(obj) = text_object {
        let font = obj.font();
        if font.is_italic() || font.italic_angle().is_ok_and(|angle| angle != 0) {
            return true;
        }
    }

    let lower_name = font_name.to_lowercase();
    lower_name.contains("italic") || lower_name.contains("oblique")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_missing_pdfium_path_fails_without_fallback() {
        let missing = Path::new("definitely_missing_pdfium_library_for_test.dll");
        let err = init_pdfium_with_path(Some(missing)).unwrap_err().to_string();
        assert!(err.contains("--pdfium-path"), "unexpected error: {err}");
    }

    #[test]
    fn xml_valid_char_filters_forbidden_controls() {
        assert!(is_xml_valid_char('A'));
        assert!(is_xml_valid_char('\t'));
        assert!(is_xml_valid_char('\n'));
        assert!(!is_xml_valid_char('\0'));
        assert!(!is_xml_valid_char('\u{0008}'));
    }

    #[test]
    fn encode_png_exact_roundtrips_rgba_dimensions() {
        let rgba = [255, 0, 0, 255, 0, 255, 0, 255];
        let encoded = encode_png_exact(2, 1, &rgba).expect("PNG encoding should succeed");
        assert!(encoded.starts_with(&[0x89, b'P', b'N', b'G']));
        let decoder = png::Decoder::new(encoded.as_slice());
        let reader = decoder.read_info().expect("encoded PNG should decode");
        assert_eq!(reader.info().width, 2);
        assert_eq!(reader.info().height, 1);
    }
}
