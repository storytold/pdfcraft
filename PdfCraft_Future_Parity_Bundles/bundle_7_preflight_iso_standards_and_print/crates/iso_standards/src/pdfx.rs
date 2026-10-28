//! ISO 15930 (PDF/X) Pre-Press Exchange Conformance Engine.

use pdfcraft_preflight_engine::{PreflightRule, PreflightSeverity, PreflightViolation};

pub struct PdfxBoxesRule;

impl PreflightRule for PdfxBoxesRule {
    fn id(&self) -> &'static str { "PDFX_TRIM_OR_BLEED_BOX" }
    fn description(&self) -> &'static str { "PDF/X requires every page to specify either a TrimBox or a BleedBox" }
    fn severity(&self) -> PreflightSeverity { PreflightSeverity::Error }

    fn check(&self, doc_text: &str) -> Vec<PreflightViolation> {
        if doc_text.contains("/Type /Page") && !doc_text.contains("/TrimBox") && !doc_text.contains("/BleedBox") {
            vec![PreflightViolation {
                rule_id: self.id().into(),
                severity: self.severity(),
                message: "Page lacks explicit /TrimBox or /BleedBox required for commercial pre-press".into(),
                object_id: None,
                page_number: Some(1),
                fixable: true,
            }]
        } else {
            Vec::new()
        }
    }

    fn fixup(&self, doc_text: &mut String) -> Result<bool, &'static str> {
        if let Some(pos) = doc_text.find("/MediaBox [0 0 612 792]") {
            doc_text.insert_str(pos + 23, " /TrimBox [18 18 594 774]");
            return Ok(true);
        }
        Ok(false)
    }
}
