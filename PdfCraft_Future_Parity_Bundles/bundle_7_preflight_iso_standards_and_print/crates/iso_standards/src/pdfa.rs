//! ISO 19005 (PDF/A) Archival Conformance Engine.

use pdfcraft_preflight_engine::{PreflightRule, PreflightSeverity, PreflightViolation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfaLevel {
    A1b,
    A2b,
    A3b,
    A4,
}

pub struct PdfaEncryptionRule;

impl PreflightRule for PdfaEncryptionRule {
    fn id(&self) -> &'static str { "PDFA_NO_ENCRYPTION" }
    fn description(&self) -> &'static str { "PDF/A prohibits document encryption (/Encrypt dictionary)" }
    fn severity(&self) -> PreflightSeverity { PreflightSeverity::Error }

    fn check(&self, doc_text: &str) -> Vec<PreflightViolation> {
        if doc_text.contains("/Encrypt") {
            vec![PreflightViolation {
                rule_id: self.id().into(),
                severity: self.severity(),
                message: "Document is encrypted; ISO 19005 requires all PDF/A files to be unencrypted".into(),
                object_id: None,
                page_number: None,
                fixable: false,
            }]
        } else {
            Vec::new()
        }
    }

    fn fixup(&self, _doc_text: &mut String) -> Result<bool, &'static str> {
        // Encryption removal requires master password
        Ok(false)
    }
}

pub struct PdfaOutputIntentRule {
    pub level: PdfaLevel,
}

impl PreflightRule for PdfaOutputIntentRule {
    fn id(&self) -> &'static str { "PDFA_OUTPUT_INTENT" }
    fn description(&self) -> &'static str { "PDF/A requires at least one OutputIntent dictionary with standard subtype" }
    fn severity(&self) -> PreflightSeverity { PreflightSeverity::Error }

    fn check(&self, doc_text: &str) -> Vec<PreflightViolation> {
        let has_intent = doc_text.contains("/OutputIntents") || doc_text.contains("/GTS_PDFA1") || doc_text.contains("/GTS_PDFA2");
        if !has_intent {
            vec![PreflightViolation {
                rule_id: self.id().into(),
                severity: self.severity(),
                message: "Missing standard PDF/A OutputIntent dictionary defining device color profile".into(),
                object_id: None,
                page_number: None,
                fixable: true,
            }]
        } else {
            Vec::new()
        }
    }

    fn fixup(&self, doc_text: &mut String) -> Result<bool, &'static str> {
        if !doc_text.contains("/OutputIntents") {
            let intent_dict = "<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB IEC61966-2.1) /Info (sRGB) >>";
            if let Some(pos) = doc_text.find("/Type /Catalog") {
                doc_text.insert_str(pos + 14, &format!(" /OutputIntents [ {} ]", intent_dict));
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub struct PdfaXmpMetadataRule;

impl PreflightRule for PdfaXmpMetadataRule {
    fn id(&self) -> &'static str { "PDFA_XMP_METADATA" }
    fn description(&self) -> &'static str { "PDF/A requires XMP metadata stream containing pdfaid schema" }
    fn severity(&self) -> PreflightSeverity { PreflightSeverity::Error }

    fn check(&self, doc_text: &str) -> Vec<PreflightViolation> {
        if !doc_text.contains("pdfaid:part") {
            vec![PreflightViolation {
                rule_id: self.id().into(),
                severity: self.severity(),
                message: "Missing XMP metadata stream with declared pdfaid:part conformance tag".into(),
                object_id: None,
                page_number: None,
                fixable: true,
            }]
        } else {
            Vec::new()
        }
    }

    fn fixup(&self, _doc_text: &mut String) -> Result<bool, &'static str> {
        Ok(false)
    }
}
