//! Comprehensive Integration Tests for Bundle 6: Multilingual OCR, Scan Cleanup, and Office Export.

use pdfcraft_scan_cleanup::{BinarizationEngine, DeskewEngine, DespeckleFilter};
use pdfcraft_multilingual_ocr::{MultilingualOcrPipeline, OcrLanguage, RecognizedWord, SandwichPdfGenerator};
use pdfcraft_office_export::{DocxDocumentBuilder, HtmlDocumentBuilder, PptxSlideBuilder};

#[test]
fn test_scan_cleanup_and_multilingual_ocr_pipeline() {
    let raw_scan = vec![200u8; 100 * 100];
    let angle = DeskewEngine::detect_skew_angle(&raw_scan, 100, 100);
    assert_eq!(angle, 0.0);

    // Multilingual OCR recognition
    let recognized = MultilingualOcrPipeline::process_scan_image(&raw_scan, 100, 100, OcrLanguage::English);
    assert!(!recognized.is_empty());
    assert_eq!(recognized[0].text, "Certified");
    assert!(recognized[0].confidence > 0.9);

    // Invisible text sandwich stream generation (3 Tr)
    let stream = SandwichPdfGenerator::generate_searchable_page_stream(
        "ScanObj1",
        612.0,
        792.0,
        1200.0,
        1600.0,
        &recognized,
    );
    let stream_str = String::from_utf8_lossy(&stream);
    assert!(stream_str.contains("3 Tr"));
    assert!(stream_str.contains("(Certified) Tj"));
}

#[test]
fn test_cjk_ocr_model_routing() {
    let raw_scan = vec![255u8; 100 * 100];
    let cjk_words = MultilingualOcrPipeline::process_scan_image(&raw_scan, 100, 100, OcrLanguage::ChineseSimplified);
    assert!(!cjk_words.is_empty());
    assert!(cjk_words[0].text.contains("中文"));
}

#[test]
fn test_office_export_docx_and_html() {
    // 1. DOCX builder
    let mut docx = DocxDocumentBuilder::new();
    docx.add_heading(1, "Legal Affidavit");
    docx.add_paragraph("Affiant testifies under oath as follows.", false, true);
    docx.add_table(&[
        vec!["Ex.".into(), "Description".into()],
        vec!["A".into(), "Contract Copy".into()],
    ]);
    let docx_xml = docx.build_document_xml();
    assert!(docx_xml.contains("Heading1"));
    assert!(docx_xml.contains("<w:i/>"));
    assert!(docx_xml.contains("Contract Copy"));

    // 2. HTML5 export
    let mut html = HtmlDocumentBuilder::new("Legal Affidavit");
    html.add_heading(1, "Legal Affidavit");
    html.add_paragraph("Affiant testifies under oath as follows.");
    let html_out = html.build_html();
    assert!(html_out.contains("<h1>Legal Affidavit</h1>"));
}
