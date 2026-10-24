//! Typographic Export workflow using `pdf-typographic-converter`.
//!
//! Exports an active PDF document to high-fidelity Word (.docx), Excel (.xlsx),
//! or PowerPoint (.pptx) documents using PDFium character extraction and typographic clustering.

use pdf_typographic_converter::{emitters, extract, layout};
use std::path::Path;

/// Supported typographic export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Microsoft Word (.docx) document
    Docx,
    /// Microsoft Excel (.xlsx) spreadsheet
    Xlsx,
    /// Microsoft PowerPoint (.pptx) presentation
    Pptx,
}

impl ExportFormat {
    /// File extension without leading period.
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Docx => "docx",
            ExportFormat::Xlsx => "xlsx",
            ExportFormat::Pptx => "pptx",
        }
    }
}

/// Convert a PDF file at `pdf_path` into the requested format and write to `output_path`.
pub fn export_pdf<P: AsRef<Path>, Q: AsRef<Path>>(
    pdf_path: P,
    output_path: Q,
    format: ExportFormat,
) -> Result<(), String> {
    let pdf_path_ref = pdf_path.as_ref();
    let output_path_ref = output_path.as_ref();

    let page_data = extract::extract_pages_full(pdf_path_ref)
        .map_err(|e| format!("Failed to extract pages from PDF at {}: {e:#}", pdf_path_ref.display()))?;

    let document_layout = layout::build_layout(page_data);

    match format {
        ExportFormat::Docx => emitters::export_docx(&document_layout, output_path_ref)
            .map_err(|e| format!("Failed to export DOCX to {}: {e:#}", output_path_ref.display())),
        ExportFormat::Xlsx => emitters::export_xlsx(&document_layout, output_path_ref)
            .map_err(|e| format!("Failed to export XLSX to {}: {e:#}", output_path_ref.display())),
        ExportFormat::Pptx => emitters::export_pptx(&document_layout, output_path_ref)
            .map_err(|e| format!("Failed to export PPTX to {}: {e:#}", output_path_ref.display())),
    }
}

/// Convert PDF bytes in memory into the requested format and write to `output_path`.
pub fn export_pdf_bytes<Q: AsRef<Path>>(
    pdf_bytes: &[u8],
    output_path: Q,
    format: ExportFormat,
) -> Result<(), String> {
    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join(format!(
        "pdfcraft_export_{}_{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    std::fs::write(&temp_file_path, pdf_bytes)
        .map_err(|e| format!("Failed to write temporary PDF to {}: {e}", temp_file_path.display()))?;

    let result = export_pdf(&temp_file_path, output_path, format);
    let _ = std::fs::remove_file(&temp_file_path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_formats_extension() {
        assert_eq!(ExportFormat::Docx.extension(), "docx");
        assert_eq!(ExportFormat::Xlsx.extension(), "xlsx");
        assert_eq!(ExportFormat::Pptx.extension(), "pptx");
    }

    #[test]
    fn test_export_pdf_missing_file_returns_error() {
        let non_existent = Path::new("non_existent_doc_12345.pdf");
        let out = Path::new("out.docx");
        let res = export_pdf(non_existent, out, ExportFormat::Docx);
        assert!(res.is_err(), "exporting non-existent PDF should return Err");
    }
}

