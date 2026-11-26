use pdf_typographic_converter::extract::{extract_glyphs_with_pdfium, init_pdfium, init_pdfium_with_path, RawGlyph};
use pdfium_render::prelude::{PdfPageObjectsCommon, PdfPagePaperSize, PdfPoints, Pdfium};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let seq = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pdfcraft-typographic-{prefix}-{}-{seq}", std::process::id()));
        fs::create_dir_all(&path).expect("Failed to create unique temp directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn create_sample_pdf(pdfium: &Pdfium, file_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut document = pdfium.create_new_pdf()?;

    // Page 1: Standard text objects
    {
        let mut page = document.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;

        let helvetica = document.fonts_mut().helvetica();
        page.objects_mut().create_text_object(PdfPoints::new(50.0), PdfPoints::new(750.0), "Hello", helvetica, PdfPoints::new(14.0))?;

        let helvetica_bold = document.fonts_mut().helvetica_bold();
        page.objects_mut().create_text_object(PdfPoints::new(50.0), PdfPoints::new(700.0), "BoldText", helvetica_bold, PdfPoints::new(18.0))?;

        let times_italic = document.fonts_mut().times_italic();
        page.objects_mut().create_text_object(PdfPoints::new(50.0), PdfPoints::new(650.0), "ItalicText", times_italic, PdfPoints::new(12.0))?;
    }

    // Page 2: Completely empty page (to test graceful handling of empty pages)
    {
        let _empty_page = document.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;
    }

    // Page 3: Additional text with punctuation and numbers
    {
        let mut page = document.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;

        let courier = document.fonts_mut().courier();
        page.objects_mut().create_text_object(PdfPoints::new(72.0), PdfPoints::new(500.0), "123!?", courier, PdfPoints::new(10.0))?;
    }

    document.save_to_file(file_path)?;
    Ok(())
}

#[test]
fn test_raw_glyph_json_roundtrip() {
    let glyphs = vec![RawGlyph {
        char_code: 'A',
        font_name: "Helvetica".to_string(),
        font_size: 12.0,
        is_bold: true,
        is_italic: false,
        bounds: (50.0, 762.0, 58.0, 750.0),
        origin_x: 50.0,
        origin_y: 750.0,
    }];
    let json = serde_json::to_string_pretty(&glyphs).expect("Failed to serialize to JSON");
    assert!(json.starts_with("[\n"), "JSON should be an array");
    let deserialized: Vec<RawGlyph> = serde_json::from_str(&json).expect("Failed to deserialize JSON");
    assert_eq!(glyphs, deserialized);
}

#[test]
fn test_init_pdfium_with_missing_explicit_path_fails() {
    let dir = TempDir::new("missing-pdfium");
    let missing_path = dir.path().join("nonexistent-pdfium.dll");
    let err = init_pdfium_with_path(Some(&missing_path)).expect_err("Missing explicit PDFium path must fail");
    assert!(err.to_string().contains("--pdfium-path"), "Unexpected error: {err}");
}

#[test]
#[ignore = "requires external PDFium dynamic library (set PDFIUM_DYNAMIC_LIB_PATH and run with -- --ignored)"]
fn test_glyph_extraction_from_pdf() {
    let pdfium = init_pdfium().expect("PDFium library must be available when running ignored PDFium tests");

    let temp_dir = TempDir::new("extract-glyphs");
    let test_pdf_path = temp_dir.path().join("test_sample_document.pdf");

    create_sample_pdf(&pdfium, &test_pdf_path).expect("Failed to create sample PDF");

    let glyphs = extract_glyphs_with_pdfium(&pdfium, &test_pdf_path).expect("Failed to extract glyphs");
    assert!(!glyphs.is_empty(), "Glyphs list should not be empty");

    let text_string: String = glyphs.iter().map(|g| g.char_code).collect();
    assert!(text_string.contains("Hello"), "Extracted text should contain 'Hello', got: {text_string}");
    assert!(text_string.contains("BoldText"), "Extracted text should contain 'BoldText', got: {text_string}");
    assert!(text_string.contains("ItalicText"), "Extracted text should contain 'ItalicText', got: {text_string}");
    assert!(text_string.contains("123!?"), "Extracted text should contain '123!?', got: {text_string}");

    for glyph in &glyphs {
        assert!(glyph.font_size > 0.0, "Font size must be greater than 0");
        assert!(!glyph.font_name.is_empty(), "Font name must not be empty");

        let (left, top, right, bottom) = glyph.bounds;
        assert!(left <= right, "Left ({left}) should be <= Right ({right}) for char '{}'", glyph.char_code);
        assert!(top >= bottom, "Top ({top}) should be >= Bottom ({bottom}) for char '{}'", glyph.char_code);
        assert!(!glyph.origin_x.is_nan());
        assert!(!glyph.origin_y.is_nan());
    }

    let bold_glyphs: Vec<_> = glyphs.iter().filter(|g| (g.origin_y - 700.0).abs() < 5.0).collect();
    assert_eq!(bold_glyphs.len(), "BoldText".len());
    for g in &bold_glyphs {
        assert!(g.is_bold, "Glyph '{}' in BoldText span must have is_bold == true", g.char_code);
        assert!(!g.is_italic, "Glyph '{}' in BoldText span must have is_italic == false", g.char_code);
    }

    let italic_glyphs: Vec<_> = glyphs.iter().filter(|g| (g.origin_y - 650.0).abs() < 5.0).collect();
    assert_eq!(italic_glyphs.len(), "ItalicText".len());
    for g in &italic_glyphs {
        assert!(g.is_italic, "Glyph '{}' in ItalicText span must have is_italic == true", g.char_code);
        assert!(!g.is_bold, "Glyph '{}' in ItalicText span must have is_bold == false", g.char_code);
    }
}

#[test]
#[ignore = "requires external PDFium dynamic library (set PDFIUM_DYNAMIC_LIB_PATH and run with -- --ignored)"]
fn test_empty_page_handling() {
    let pdfium = init_pdfium().expect("PDFium library must be available when running ignored PDFium tests");

    let temp_dir = TempDir::new("empty-pages");
    let test_pdf_path = temp_dir.path().join("test_empty_document.pdf");

    let mut document = pdfium.create_new_pdf().expect("Failed to create PDF");
    document.pages_mut().create_page_at_end(PdfPagePaperSize::a4()).expect("Failed to create page");
    document.pages_mut().create_page_at_end(PdfPagePaperSize::a4()).expect("Failed to create page");
    document.save_to_file(&test_pdf_path).expect("Failed to save PDF");

    let glyphs = extract_glyphs_with_pdfium(&pdfium, &test_pdf_path).expect("Should handle empty pages without error");
    assert!(glyphs.is_empty(), "Glyphs on empty PDF should be empty");
}

#[test]
#[ignore = "requires external PDFium dynamic library (set PDFIUM_DYNAMIC_LIB_PATH and run with -- --ignored)"]
fn test_cli_binary_execution() {
    let pdfium = init_pdfium().expect("PDFium library must be available when running ignored PDFium tests");

    let temp_dir = TempDir::new("cli-exec");
    let test_pdf_path = temp_dir.path().join("test_cli_document.pdf");
    let output_dir = temp_dir.path().join("cli_output");

    create_sample_pdf(&pdfium, &test_pdf_path).expect("Failed to create sample PDF");

    let binary_path = env!("CARGO_BIN_EXE_pdf-typographic-converter");
    let output =
        std::process::Command::new(binary_path).arg(&test_pdf_path).arg("--out").arg(&output_dir).output().expect("Failed to execute binary");

    assert!(output.status.success(), "CLI execution should succeed. Stderr: {}", String::from_utf8_lossy(&output.stderr));

    let stdout_str = String::from_utf8(output.stdout).expect("Stdout should be UTF-8");
    assert!(stdout_str.contains("=== Conversion Summary ==="), "Stdout should contain summary header, got: {stdout_str}");
    assert!(stdout_str.contains("Total Lines:"), "Stdout should contain 'Total Lines:'");
    assert!(stdout_str.contains("Total Text Runs:"), "Stdout should contain 'Total Text Runs:'");
    assert!(stdout_str.contains("Detected Superscripts:"), "Stdout should contain 'Detected Superscripts:'");

    assert!(output_dir.join("test_cli_document.docx").exists());
    assert!(output_dir.join("test_cli_document.xlsx").exists());
    assert!(output_dir.join("test_cli_document.pptx").exists());
}
