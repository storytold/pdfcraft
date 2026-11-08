use pdf_typographic_converter::extract::{extract_glyphs_with_pdfium, init_pdfium};
use pdfium_render::prelude::*;
use std::fs;
use std::path::PathBuf;

fn create_sample_pdf(file_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let pdfium = init_pdfium()?;
    let mut document = pdfium.create_new_pdf()?;

    // Page 1: Standard text objects
    {
        let mut page = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())?;

        let helvetica = document.fonts_mut().helvetica();
        page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(750.0),
            "Hello",
            helvetica,
            PdfPoints::new(14.0),
        )?;

        let helvetica_bold = document.fonts_mut().helvetica_bold();
        page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(700.0),
            "BoldText",
            helvetica_bold,
            PdfPoints::new(18.0),
        )?;

        let times_italic = document.fonts_mut().times_italic();
        page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(650.0),
            "ItalicText",
            times_italic,
            PdfPoints::new(12.0),
        )?;
    }

    // Page 2: Completely empty page (to test graceful handling of empty pages)
    {
        let _empty_page = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())?;
    }

    // Page 3: Additional text with punctuation and numbers
    {
        let mut page = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())?;

        let courier = document.fonts_mut().courier();
        page.objects_mut().create_text_object(
            PdfPoints::new(72.0),
            PdfPoints::new(500.0),
            "123!?",
            courier,
            PdfPoints::new(10.0),
        )?;
    }

    document.save_to_file(file_path)?;
    Ok(())
}

#[test]
fn test_glyph_extraction_from_pdf() {
    let temp_dir = std::env::temp_dir();
    let test_pdf_path = temp_dir.join("test_sample_document.pdf");

    // Create the test PDF
    create_sample_pdf(&test_pdf_path).expect("Failed to create sample PDF");

    let pdfium = init_pdfium().expect("Failed to initialize PDFium");
    let glyphs = extract_glyphs_with_pdfium(&pdfium, &test_pdf_path)
        .expect("Failed to extract glyphs");

    assert!(!glyphs.is_empty(), "Glyphs list should not be empty");

    // Verify presence of characters from page 1 and page 3
    let extracted_chars: Vec<char> = glyphs.iter().map(|g| g.char_code).collect();
    let text_string: String = extracted_chars.iter().collect();

    assert!(
        text_string.contains("Hello"),
        "Extracted text should contain 'Hello', got: {}",
        text_string
    );
    assert!(
        text_string.contains("BoldText"),
        "Extracted text should contain 'BoldText', got: {}",
        text_string
    );
    assert!(
        text_string.contains("ItalicText"),
        "Extracted text should contain 'ItalicText', got: {}",
        text_string
    );
    assert!(
        text_string.contains("123!?"),
        "Extracted text should contain '123!?', got: {}",
        text_string
    );

    // Verify fields on each glyph
    for glyph in &glyphs {
        assert!(glyph.font_size > 0.0, "Font size must be greater than 0");
        assert!(!glyph.font_name.is_empty(), "Font name must not be empty");

        // Verify bounds: (left, top, right, bottom)
        let (left, top, right, bottom) = glyph.bounds;
        // In PDF coordinates, left <= right
        assert!(
            left <= right,
            "Left ({}) should be <= Right ({}) for char '{}'",
            left,
            right,
            glyph.char_code
        );
        // And top is typically >= bottom in standard PDF coordinates
        assert!(
            top >= bottom,
            "Top ({}) should be >= Bottom ({}) for char '{}'",
            top,
            bottom,
            glyph.char_code
        );

        // Origin should be valid coordinates
        assert!(!glyph.origin_x.is_nan());
        assert!(!glyph.origin_y.is_nan());
    }

    // Verify bold detection on "BoldText" glyphs
    let bold_glyphs: Vec<_> = glyphs
        .iter()
        .filter(|g| "BoldText".contains(g.char_code))
        .collect();
    assert!(!bold_glyphs.is_empty());
    assert!(
        bold_glyphs.iter().any(|g| g.is_bold),
        "At least some BoldText glyphs should have is_bold == true"
    );

    // Verify italic detection on "ItalicText" glyphs
    let italic_glyphs: Vec<_> = glyphs
        .iter()
        .filter(|g| "ItalicText".contains(g.char_code))
        .collect();
    assert!(!italic_glyphs.is_empty());
    assert!(
        italic_glyphs.iter().any(|g| g.is_italic),
        "At least some ItalicText glyphs should have is_italic == true"
    );

    // Verify JSON serialization
    let json = serde_json::to_string_pretty(&glyphs).expect("Failed to serialize to JSON");
    assert!(json.starts_with("[\n"), "JSON should be an array");

    // Deserialize back and compare
    let deserialized: Vec<pdf_typographic_converter::RawGlyph> =
        serde_json::from_str(&json).expect("Failed to deserialize JSON");
    assert_eq!(glyphs.len(), deserialized.len());

    // Clean up
    let _ = fs::remove_file(&test_pdf_path);
}

#[test]
fn test_empty_page_handling() {
    let temp_dir = std::env::temp_dir();
    let test_pdf_path = temp_dir.join("test_empty_document.pdf");

    let pdfium = init_pdfium().expect("Failed to initialize PDFium");
    let mut document = pdfium.create_new_pdf().expect("Failed to create PDF");
    // Add 2 empty pages
    document
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .expect("Failed to create page");
    document
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .expect("Failed to create page");
    document
        .save_to_file(&test_pdf_path)
        .expect("Failed to save PDF");

    let glyphs = extract_glyphs_with_pdfium(&pdfium, &test_pdf_path)
        .expect("Should handle empty pages without error");

    assert!(glyphs.is_empty(), "Glyphs on empty PDF should be empty");

    let _ = fs::remove_file(&test_pdf_path);
}

#[test]
fn test_cli_binary_execution() {
    let temp_dir = std::env::temp_dir();
    let test_pdf_path = temp_dir.join("test_cli_document.pdf");
    let output_dir = temp_dir.join("cli_output_test");

    create_sample_pdf(&test_pdf_path).expect("Failed to create sample PDF");

    let binary_path = env!("CARGO_BIN_EXE_pdf-typographic-converter");
    let output = std::process::Command::new(binary_path)
        .arg(&test_pdf_path)
        .arg("--out")
        .arg(&output_dir)
        .output()
        .expect("Failed to execute binary");

    assert!(
        output.status.success(),
        "CLI execution should succeed. Stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout_str = String::from_utf8(output.stdout).expect("Stdout should be UTF-8");
    assert!(
        stdout_str.contains("=== Conversion Summary ==="),
        "Stdout should contain summary header, got: {}",
        stdout_str
    );
    assert!(
        stdout_str.contains("Total Lines:"),
        "Stdout should contain 'Total Lines:'"
    );
    assert!(
        stdout_str.contains("Total Text Runs:"),
        "Stdout should contain 'Total Text Runs:'"
    );
    assert!(
        stdout_str.contains("Detected Superscripts:"),
        "Stdout should contain 'Detected Superscripts:'"
    );

    // Verify generated files exist in output directory
    assert!(output_dir.join("test_cli_document.docx").exists());
    assert!(output_dir.join("test_cli_document.xlsx").exists());
    assert!(output_dir.join("test_cli_document.pptx").exists());

    let _ = fs::remove_file(&test_pdf_path);
    let _ = fs::remove_dir_all(&output_dir);
}
