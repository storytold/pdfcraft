//! Comprehensive Integration Tests for Bundle 7: Preflight Rule Engine, ISO Standards, and Print.

use pdfcraft_preflight_engine::{PreflightRunner, PreflightSeverity};
use pdfcraft_iso_standards::{PdfaEncryptionRule, PdfaLevel, PdfaOutputIntentRule, PdfxBoxesRule};
use pdfcraft_optimizer::{ImageDownsampler, OptimizationSettings};
use pdfcraft_linearizer::{FastWebViewLinearizer, LinearizationParams};
use pdfcraft_print_production::PrintProductionEngine;

#[test]
fn test_preflight_analysis_and_automated_fixups() {
    let mut runner = PreflightRunner::new();
    runner.add_rule(Box::new(PdfaEncryptionRule));
    runner.add_rule(Box::new(PdfaOutputIntentRule { level: PdfaLevel::A1b }));

    // Document missing OutputIntent
    let initial_pdf = "%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n%%EOF".to_string();
    let report = runner.run_analysis("PDF/A-1b Profile", &initial_pdf);
    assert!(!report.passed);
    assert_eq!(report.violations.len(), 1);
    assert!(report.violations[0].fixable);

    // Apply automated fixup
    let mut fixup_pdf = initial_pdf.clone();
    let fixups_count = runner.run_fixups(&mut fixup_pdf);
    assert_eq!(fixups_count, 1);
    assert!(fixup_pdf.contains("/OutputIntents"));

    // Re-verify after fixup
    let report_after = runner.run_analysis("PDF/A-1b Profile", &fixup_pdf);
    assert!(report_after.passed);
}

#[test]
fn test_optimizer_and_linearization() {
    // 1. Optimizer downsampling
    let samples = vec![200u8; 8 * 8 * 3]; // 8x8 RGB
    let (downsampled, dw, dh) = ImageDownsampler::downsample_pixels(&samples, 8, 8, 3, 2);
    assert_eq!((dw, dh), (4, 4));
    assert_eq!(downsampled.len(), 4 * 4 * 3);

    // 2. Fast Web View linearization
    let dummy_pdf = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF";
    let linearized = FastWebViewLinearizer::linearize(dummy_pdf, 5);
    let lin_str = String::from_utf8_lossy(&linearized);
    assert!(lin_str.contains("/Linearized 1.0"));
    assert!(lin_str.contains("/S /PageOffset"));
}

#[test]
fn test_cmyk_separations_and_tac_limits() {
    let cmyk_data = vec![
        255, 0, 0, 0,   // Pure Cyan
        0, 255, 0, 0,   // Pure Magenta
        0, 0, 255, 0,   // Pure Yellow
        0, 0, 0, 255,   // Pure Black
        255, 255, 255, 255, // Rich 400% Black
    ];

    let plates = PrintProductionEngine::extract_plates(&cmyk_data, 5, 1);
    assert_eq!(plates.cyan_plate[0], 255);
    assert_eq!(plates.magenta_plate[1], 255);
    assert_eq!(plates.yellow_plate[2], 255);
    assert_eq!(plates.black_plate[3], 255);

    let tac_violations = PrintProductionEngine::calculate_tac_violations(&cmyk_data, 300.0);
    assert_eq!(tac_violations, vec![false, false, false, false, true]);
}
