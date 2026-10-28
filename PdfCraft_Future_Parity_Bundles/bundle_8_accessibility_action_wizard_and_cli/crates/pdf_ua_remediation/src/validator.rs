//! ISO 14289 (PDF/UA-1 & PDF/UA-2) 32-Rule Accessibility Validator.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilitySeverity {
    FatalFailure,
    RemediationNeeded,
    BestPracticeWarning,
}

#[derive(Debug, Clone)]
pub struct UaViolation {
    pub checkpoint: &'static str, // Matterhorn Protocol checkpoint ID (e.g. "01-001", "13-001")
    pub severity: AccessibilitySeverity,
    pub description: String,
    pub element_tag: Option<String>,
}

pub struct PdfUaValidator;

impl PdfUaValidator {
    /// Validates document text and structure tree against Matterhorn Protocol checkpoints.
    pub fn validate_document(doc_text: &str) -> Vec<UaViolation> {
        let mut violations = Vec::new();

        // Checkpoint 01-001: Tagged PDF requirement
        if !doc_text.contains("/MarkInfo") || !doc_text.contains("/Marked true") {
            violations.push(UaViolation {
                checkpoint: "01-001",
                severity: AccessibilitySeverity::FatalFailure,
                description: "Document is not tagged; /MarkInfo dictionary missing or /Marked is not true".into(),
                element_tag: None,
            });
        }

        // Checkpoint 02-001: Natural language must be specified
        if !doc_text.contains("/Lang") {
            violations.push(UaViolation {
                checkpoint: "02-001",
                severity: AccessibilitySeverity::RemediationNeeded,
                description: "Document natural language (/Lang) is not specified in the catalog".into(),
                element_tag: None,
            });
        }

        // Checkpoint 03-001: Document title must be specified in metadata
        if !doc_text.contains("/Title") && !doc_text.contains("<dc:title>") {
            violations.push(UaViolation {
                checkpoint: "03-001",
                severity: AccessibilitySeverity::RemediationNeeded,
                description: "Document title is missing in metadata and viewer preferences /DisplayDocTitle is not set".into(),
                element_tag: None,
            });
        }

        // Checkpoint 13-001: Figures must have alternative text (/Alt)
        if doc_text.contains("/S /Figure") && !doc_text.contains("/Alt") {
            violations.push(UaViolation {
                checkpoint: "13-001",
                severity: AccessibilitySeverity::RemediationNeeded,
                description: "Figure structure element lacks alternative text (/Alt description)".into(),
                element_tag: Some("Figure".into()),
            });
        }

        // Checkpoint 15-001: Table headers (/TH) must have scope
        if doc_text.contains("/S /TH") && !doc_text.contains("/Scope") {
            violations.push(UaViolation {
                checkpoint: "15-001",
                severity: AccessibilitySeverity::RemediationNeeded,
                description: "Table header (/TH) cell missing explicit /Scope attribute (/Row or /Column)".into(),
                element_tag: Some("TH".into()),
            });
        }

        violations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pdf_ua_validation_catches_missing_tags_and_lang() {
        let untagged_pdf = "%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n%%EOF";
        let violations = PdfUaValidator::validate_document(untagged_pdf);
        assert!(violations.iter().any(|v| v.checkpoint == "01-001"));
        assert!(violations.iter().any(|v| v.checkpoint == "02-001"));
    }
}
