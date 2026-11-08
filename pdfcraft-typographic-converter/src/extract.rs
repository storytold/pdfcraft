use crate::layout::{ExtractedPageData, PageImage};
use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// Initialize PDFium, supporting binding via a local dynamic library or system PDFium.
///
/// If a `custom_path` or the `PDFIUM_PATH` environment variable is provided,
/// it will attempt to load the library from that path first.
/// Otherwise, it looks for the platform library in common local locations,
/// and falls back to binding to the system library.
pub fn init_pdfium_with_path(custom_path: Option<&Path>) -> Result<Pdfium> {
    // 1. Explicit path passed by caller
    if let Some(path) = custom_path {
        if path.exists() {
            if let Ok(bindings) = Pdfium::bind_to_library(path) {
                return Ok(Pdfium::new(bindings));
            }
        }
    }

    // 2. Check PDFIUM_PATH environment variable
    if let Ok(env_path) = std::env::var("PDFIUM_PATH") {
        let p = Path::new(&env_path);
        if p.exists() {
            if let Ok(bindings) = Pdfium::bind_to_library(p) {
                return Ok(Pdfium::new(bindings));
            }
        }
    }

    // 3. Check local candidate paths (current working directory, ./lib, next to exe)
    let lib_name = Pdfium::pdfium_platform_library_name();
    let mut candidate_paths: Vec<PathBuf> = vec![
        PathBuf::from(&lib_name),
        PathBuf::from(".").join(&lib_name),
        PathBuf::from("lib").join(&lib_name),
        PathBuf::from("..").join("lib").join(&lib_name),
    ];

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidate_paths.push(exe_dir.join(&lib_name));
            candidate_paths.push(exe_dir.join("lib").join(&lib_name));
        }
    }

    for path in candidate_paths {
        if path.exists() {
            if let Ok(bindings) = Pdfium::bind_to_library(&path) {
                return Ok(Pdfium::new(bindings));
            }
        }
    }

    // 4. Try binding to the system-installed library
    if let Ok(bindings) = Pdfium::bind_to_system_library() {
        return Ok(Pdfium::new(bindings));
    }

    anyhow::bail!(
        "Failed to initialize PDFium: could not bind via local dynamic library ({:?}) \
        or system PDFium. Please ensure the dynamic library is in the current directory, \
        in PATH, or set the PDFIUM_PATH environment variable.",
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

/// Extracts glyphs grouped by page from a PDF file located at `pdf_path`.
/// Extracts full page data (glyphs + embedded images like barcodes/QRs) grouped by page.
pub fn extract_pages_full<P: AsRef<Path>>(
    pdf_path: P,
) -> Result<Vec<ExtractedPageData>> {
    let pdfium = init_pdfium()?;
    extract_pages_full_with_pdfium(&pdfium, pdf_path)
}

/// Extracts glyphs grouped by page from a PDF file located at `pdf_path`.
/// Returns `Vec<(page_index, width, height, Vec<RawGlyph>)>`.
#[allow(clippy::type_complexity)]
pub fn extract_pages<P: AsRef<Path>>(
    pdf_path: P,
) -> Result<Vec<(usize, f32, f32, Vec<RawGlyph>)>> {
    let pdfium = init_pdfium()?;
    extract_pages_with_pdfium(&pdfium, pdf_path)
}

/// Extracts glyphs from a PDF file using an existing `Pdfium` instance.
pub fn extract_glyphs_with_pdfium<P: AsRef<Path>>(
    pdfium: &Pdfium,
    pdf_path: P,
) -> Result<Vec<RawGlyph>> {
    let pages = extract_pages_with_pdfium(pdfium, pdf_path)?;
    Ok(pages.into_iter().flat_map(|(_, _, _, glyphs)| glyphs).collect())
}

/// Extracts glyphs grouped by page using an existing `Pdfium` instance.
/// Returns `Vec<(page_index, width, height, Vec<RawGlyph>)>`.
#[allow(clippy::type_complexity)]
pub fn extract_pages_with_pdfium<P: AsRef<Path>>(
    pdfium: &Pdfium,
    pdf_path: P,
) -> Result<Vec<(usize, f32, f32, Vec<RawGlyph>)>> {
    let pages = extract_pages_full_with_pdfium(pdfium, pdf_path)?;
    Ok(pages
        .into_iter()
        .map(|p| (p.page_index, p.width, p.height, p.glyphs))
        .collect())
}

/// Extracts full page data (glyphs + images) grouped by page using an existing `Pdfium` instance.
pub fn extract_pages_full_with_pdfium<P: AsRef<Path>>(
    pdfium: &Pdfium,
    pdf_path: P,
) -> Result<Vec<ExtractedPageData>> {
    let path_ref = pdf_path.as_ref();
    if !path_ref.exists() {
        anyhow::bail!("Input PDF file not found: {}", path_ref.display());
    }

    let document = pdfium
        .load_pdf_from_file(path_ref, None)
        .with_context(|| format!("Failed to load PDF document from {}", path_ref.display()))?;

    let mut pages_result = Vec::new();

    // Iterate across all pages
    for (page_index, page) in document.pages().iter().enumerate() {
        let width = page.width().value;
        let height = page.height().value;

        let page_objects = page.objects();
        let mut page_glyphs = Vec::new();
        let mut page_images = Vec::new();
        let mut found_text_object = false;

        // Extract text and image objects on this page
        let text_page_opt = page.text().ok();

        for object in page_objects.iter() {
            if let Some(text_object) = object.as_text_object() {
                found_text_object = true;

                if let Some(ref text_page) = text_page_opt {
                    if let Ok(chars) = text_page.chars_for_object(text_object) {
                        for ch in chars.iter() {
                            let glyph = extract_single_glyph(&ch, Some(text_object));
                            page_glyphs.push(glyph);
                        }
                    }
                }
            } else if let Some(image_object) = object.as_image_object() {
                // Extract image object (barcodes, QR codes, illustrations, etc.)
                let raw_data = image_object.get_raw_image_data().unwrap_or_default();
                let (left, top, right, bottom) = match object.bounds() {
                    Ok(quad) => {
                        let r = quad.to_rect();
                        (r.left().value, r.top().value, r.right().value, r.bottom().value)
                    }
                    Err(_) => (0.0, 0.0, 0.0, 0.0),
                };
                let width_pt = (right - left).abs();
                let height_pt = (top - bottom).abs();

                let mut bytes = raw_data;
                let is_png = bytes.starts_with(&[0x89, b'P', b'N', b'G']);
                let is_jpeg = bytes.starts_with(&[0xFF, 0xD8, 0xFF]);

                if !is_png && !is_jpeg {
                    if let Ok(bitmap) = image_object.get_processed_bitmap(&document) {
                        let w = bitmap.width() as u32;
                        let h = bitmap.height() as u32;
                        if w > 0 && h > 0 {
                            let rgba = bitmap.as_rgba_bytes();
                            if let Ok(png_data) = encode_png_exact(w, h, &rgba) {
                                bytes = png_data;
                            }
                        }
                    }
                }

                if !bytes.is_empty() {
                    page_images.push(PageImage {
                        bytes,
                        bounds: (left, top, right, bottom),
                        width_pt,
                        height_pt,
                    });
                }
            }
        }

        // Fallback: If no text objects were found on the page, or if text objects
        // yielded no glyphs while text_page has characters, extract them directly from text_page.
        if !found_text_object || page_glyphs.is_empty() {
            if let Some(ref text_page) = text_page_opt {
                if !text_page.is_empty() {
                    for ch in text_page.chars().iter() {
                        let glyph = extract_single_glyph(&ch, None);
                        page_glyphs.push(glyph);
                    }
                }
            }
        }

        pages_result.push(ExtractedPageData {
            page_index,
            width,
            height,
            glyphs: page_glyphs,
            images: page_images,
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
fn extract_single_glyph(
    ch: &PdfPageTextChar,
    text_object: Option<&PdfPageTextObject>,
) -> RawGlyph {
    // 1. Character code
    let char_code = ch.unicode_char().unwrap_or_else(|| {
        char::from_u32(ch.unicode_value()).unwrap_or('\0')
    });

    // 2. Font name (with fallback to parent text object and default)
    let mut font_name = ch.font_name();
    if font_name.is_empty() {
        if let Some(obj) = text_object {
            let name = obj.font().name();
            if !name.is_empty() {
                font_name = name;
            }
        }
    }
    if font_name.is_empty() {
        font_name = "Unknown".to_string();
    }

    // 3. Font size
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
    if font_size < 0.0 {
        font_size = 0.0;
    }

    // 4. Bold determination
    let is_bold = determine_is_bold(ch, text_object, &font_name);

    // 5. Italic determination
    let is_italic = determine_is_italic(ch, text_object, &font_name);

    // 6. Bounds: (left, top, right, bottom)
    let bounds = ch
        .tight_bounds()
        .or_else(|_| ch.loose_bounds())
        .map(|rect| {
            (
                rect.left().value,
                rect.top().value,
                rect.right().value,
                rect.bottom().value,
            )
        })
        .unwrap_or_else(|_| {
            if let Some(obj) = text_object {
                obj.bounds()
                    .map(|r| (r.left().value, r.top().value, r.right().value, r.bottom().value))
                    .unwrap_or((0.0, 0.0, 0.0, 0.0))
            } else {
                (0.0, 0.0, 0.0, 0.0)
            }
        });

    // 7. Origin X and Origin Y
    let (origin_x, origin_y) = match ch.origin() {
        Ok((ox, oy)) => (ox.value, oy.value),
        Err(_) => {
            let ox = ch.origin_x().map(|p| p.value).unwrap_or(bounds.0);
            let oy = ch.origin_y().map(|p| p.value).unwrap_or(bounds.3);
            (ox, oy)
        }
    };

    RawGlyph {
        char_code,
        font_name,
        font_size,
        is_bold,
        is_italic,
        bounds,
        origin_x,
        origin_y,
    }
}

/// Determines whether a glyph has a bold typeface using character properties,
/// font descriptor flags, font weight, parent text object, and font name heuristics.
fn determine_is_bold(
    ch: &PdfPageTextChar,
    text_object: Option<&PdfPageTextObject>,
    font_name: &str,
) -> bool {
    // Check character font weight
    if let Some(weight) = ch.font_weight() {
        match weight {
            PdfFontWeight::Weight600
            | PdfFontWeight::Weight700Bold
            | PdfFontWeight::Weight800
            | PdfFontWeight::Weight900 => return true,
            PdfFontWeight::Custom(w) if w >= 600 => return true,
            _ => {}
        }
    }

    // Check bold reenforced flag on character
    if ch.font_is_bold_reenforced() {
        return true;
    }

    // Check parent text object font descriptor if available
    if let Some(obj) = text_object {
        let font = obj.font();
        if let Ok(weight) = font.weight() {
            match weight {
                PdfFontWeight::Weight600
                | PdfFontWeight::Weight700Bold
                | PdfFontWeight::Weight800
                | PdfFontWeight::Weight900 => return true,
                PdfFontWeight::Custom(w) if w >= 600 => return true,
                _ => {}
            }
        }
        if font.is_bold_reenforced() {
            return true;
        }
    }

    // Fallback: check font family / name for bold tokens
    let lower_name = font_name.to_lowercase();
    lower_name.contains("bold")
        || lower_name.contains("black")
        || lower_name.contains("heavy")
        || lower_name.contains("semibold")
        || lower_name.contains("demibold")
}

/// Determines whether a glyph has an italic/slanted typeface using character properties,
/// font flags, parent text object font angles, and font name heuristics.
fn determine_is_italic(
    ch: &PdfPageTextChar,
    text_object: Option<&PdfPageTextObject>,
    font_name: &str,
) -> bool {
    // Check character italic flag
    if ch.font_is_italic() {
        return true;
    }

    // Check parent text object font descriptor if available
    if let Some(obj) = text_object {
        let font = obj.font();
        if font.is_italic() {
            return true;
        }
        if let Ok(angle) = font.italic_angle() {
            if angle != 0 {
                return true;
            }
        }
    }

    // Fallback: check font name for italic/oblique tokens
    let lower_name = font_name.to_lowercase();
    lower_name.contains("italic") || lower_name.contains("oblique")
}
