//! The reading report: the machine-readable record of one document's reading — what was read,
//! how long it took, what the reading is unsure of, what it produced, and why anything was
//! refused.
//!
//! The report is the contract between the reading pipeline and everything that judges its
//! output: a person reviewing a scan, a test asserting a threshold, or the next stage deciding
//! whether it may export. It therefore has a fixed shape ([`ReadingReport::to_json`]) and a
//! closed refusal vocabulary ([`Refusal`]): a stage that wants to say why it stopped picks a
//! word from [`Refusal`] rather than writing prose.
//!
//! `low_confidence_characters.percent` is the one derived field: it is computed from its count
//! and total when the report is written, so the two can never disagree.

use serde_json::{Value, json};

/// Where a document's reading came from: the file, its digest and page count, and the raster
/// policy the reading used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub path: String,
    /// The source file's SHA-256, lowercase hex.
    pub sha256: String,
    pub pages: usize,
    /// The raster policy the reading used (`"upscale_lanczos_x3"`, `"native"`, …).
    pub dpi_policy: String,
}

/// How many characters a reading left below the confidence bar, of all the characters it
/// produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowConfidenceCharacters {
    pub count: usize,
    pub total: usize,
}

impl LowConfidenceCharacters {
    /// The share of characters left below the bar, as a percentage: `100 × count / total`, and
    /// 0.0 for a reading that produced no characters (there is no percentage of nothing).
    pub fn percent(self) -> f64 {
        if self.total == 0 { 0.0 } else { 100.0 * self.count as f64 / self.total as f64 }
    }
}

/// One stretch of text a reading is unsure of: where it is, what it read, how sure it was and
/// which engine read it.
#[derive(Clone, Debug, PartialEq)]
pub struct UncertainSpan {
    /// The 1-based page the span is on.
    pub page: usize,
    /// The span's box in page-image pixels, `[left, top, width, height]`.
    pub rect: [f32; 4],
    pub text: String,
    /// The winning reading's confidence, 0–100.
    pub confidence: f32,
    /// The id of the engine that read it (`"ocrs"`, `"tesseract"`, …).
    pub source: String,
}

/// A file the reading produced, and what it is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub path: String,
    /// What the file is (`"editable_copy"`, …).
    pub kind: String,
}

/// Why a stage refused a document or an output. The vocabulary is closed, so a caller can act
/// on a report instead of reading prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// The source could not be read at all.
    UnreadableSource,
    /// The reading produced no text, so there is nothing to export.
    EmptyReading,
    /// More characters were left below the confidence bar than the caller accepts.
    LowConfidenceAboveThreshold,
    /// The reading does not fit the document (its pages or regions disagree).
    StructuralMismatch,
    /// Writing an output failed.
    ExportFailure,
    /// A model the reading needs is not installed.
    MissingModel,
}

impl Refusal {
    /// The report's word for this refusal.
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::UnreadableSource => "unreadable_source",
            Refusal::EmptyReading => "empty_reading",
            Refusal::LowConfidenceAboveThreshold => "low_confidence_above_threshold",
            Refusal::StructuralMismatch => "structural_mismatch",
            Refusal::ExportFailure => "export_failure",
            Refusal::MissingModel => "missing_model",
        }
    }
}

/// One document's reading, as reported.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadingReport {
    /// The file the reading came from.
    pub source: Source,
    /// The PDF text layer policy the reading used (`"auto"`, `"never"`, …).
    pub pdf_text_layer_policy: String,
    /// How many pages the reading covered.
    pub pages: usize,
    pub recognition_time_seconds: f64,
    /// How many errors the reading recorded.
    pub errors: usize,
    /// How many warnings it recorded.
    pub warnings: usize,
    pub low_confidence_characters: LowConfidenceCharacters,
    pub uncertain_spans: Vec<UncertainSpan>,
    pub outputs: Vec<Output>,
    pub refusals: Vec<Refusal>,
}

impl UncertainSpan {
    fn to_json(&self) -> Value {
        json!({
            "page": self.page,
            "rect": self.rect,
            "text": self.text,
            "confidence": self.confidence,
            "source": self.source,
        })
    }
}

impl Output {
    fn to_json(&self) -> Value {
        json!({ "path": self.path, "kind": self.kind })
    }
}

impl ReadingReport {
    /// A report for `source` read under `pdf_text_layer_policy`, with every count at zero and
    /// no spans, outputs or refusals yet.
    pub fn new(source: Source, pdf_text_layer_policy: impl Into<String>) -> ReadingReport {
        ReadingReport {
            pages: source.pages,
            source,
            pdf_text_layer_policy: pdf_text_layer_policy.into(),
            recognition_time_seconds: 0.0,
            errors: 0,
            warnings: 0,
            low_confidence_characters: LowConfidenceCharacters::default(),
            uncertain_spans: Vec::new(),
            outputs: Vec::new(),
            refusals: Vec::new(),
        }
    }

    /// The report as JSON, with exactly the documented fields.
    pub fn to_json(&self) -> Value {
        json!({
            "source": {
                "path": self.source.path,
                "sha256": self.source.sha256,
                "pages": self.source.pages,
                "dpi_policy": self.source.dpi_policy,
            },
            "pdf_text_layer_policy": self.pdf_text_layer_policy,
            "pages": self.pages,
            "recognition_time_seconds": self.recognition_time_seconds,
            "errors": self.errors,
            "warnings": self.warnings,
            "low_confidence_characters": {
                "count": self.low_confidence_characters.count,
                "total": self.low_confidence_characters.total,
                "percent": self.low_confidence_characters.percent(),
            },
            "uncertain_spans": self.uncertain_spans.iter().map(UncertainSpan::to_json).collect::<Vec<Value>>(),
            "outputs": self.outputs.iter().map(Output::to_json).collect::<Vec<Value>>(),
            "refusals": self.refusals.iter().map(|refusal| refusal.as_str()).collect::<Vec<&str>>(),
        })
    }

    /// The report as a JSON string.
    pub fn to_json_string(&self) -> String {
        self.to_json().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source { path: "scan.pdf".into(), sha256: "0".repeat(64), pages: 9, dpi_policy: "upscale_lanczos_x3".into() }
    }

    #[test]
    fn the_report_carries_counts_confidence_and_uncertain_spans() {
        let mut report = ReadingReport::new(source(), "auto");
        report.errors = 1;
        report.warnings = 2;
        report.recognition_time_seconds = 15.2;
        report.low_confidence_characters = LowConfidenceCharacters { count: 245, total: 18_723 };
        report.uncertain_spans.push(UncertainSpan {
            page: 1,
            rect: [10.0, 20.0, 30.0, 12.0],
            text: "plaintif".into(),
            confidence: 42.0,
            source: "ocrs".into(),
        });
        let json = report.to_json();
        assert_eq!(json["pages"].as_u64(), Some(9), "{json}");
        assert_eq!(json["source"]["pages"].as_u64(), Some(9), "{json}");
        assert_eq!(json["source"]["dpi_policy"].as_str(), Some("upscale_lanczos_x3"), "{json}");
        assert_eq!(json["errors"].as_u64(), Some(1), "{json}");
        assert_eq!(json["warnings"].as_u64(), Some(2), "{json}");
        let percent = json["low_confidence_characters"]["percent"].as_f64().expect("a percentage");
        assert!((percent - 100.0 * 245.0 / 18_723.0).abs() < 1e-9, "{json}");
        let span = &json["uncertain_spans"][0];
        assert_eq!(span["page"].as_u64(), Some(1), "{json}");
        assert_eq!(span["text"].as_str(), Some("plaintif"), "{json}");
        assert_eq!(span["rect"].as_array().map(Vec::len), Some(4), "{json}");
        assert_eq!(span["confidence"].as_f64(), Some(42.0), "{json}");
        assert_eq!(span["source"].as_str(), Some("ocrs"), "{json}");
    }

    /// An empty reading has no percentage of nothing, and says why it is empty.
    #[test]
    fn an_empty_reading_reports_no_percentage_and_its_refusal() {
        let mut report = ReadingReport::new(source(), "auto");
        report.refusals.push(Refusal::EmptyReading);
        report.refusals.push(Refusal::MissingModel);
        let json = report.to_json();
        assert_eq!(json["low_confidence_characters"]["percent"].as_f64(), Some(0.0), "{json}");
        assert_eq!(json["refusals"], json!(["empty_reading", "missing_model"]), "{json}");
        assert_eq!(json["outputs"], json!([]), "{json}");
    }

    /// The report is a contract: its top-level fields are exactly these.
    #[test]
    fn the_report_has_exactly_the_documented_fields() {
        let report = ReadingReport::new(source(), "auto");
        let json = report.to_json();
        let mut keys: Vec<&str> = json.as_object().map(|object| object.keys().map(String::as_str).collect()).unwrap_or_default();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "errors",
                "low_confidence_characters",
                "outputs",
                "pages",
                "pdf_text_layer_policy",
                "recognition_time_seconds",
                "refusals",
                "source",
                "uncertain_spans",
                "warnings",
            ]
        );
    }
}
