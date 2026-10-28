//! PdfCraft Office Export: High-fidelity DOCX, PPTX, and HTML5 structural exporters.
//!
//! Subsystem covering Milestone M10:
//! - [P1] Export to Word (.docx)
//! - [P1] Export to PowerPoint (.pptx)
//! - [P1] Export to HTML

pub mod docx;
pub mod pptx;
pub mod html;

pub use docx::DocxDocumentBuilder;
pub use pptx::PptxSlideBuilder;
pub use html::HtmlDocumentBuilder;
