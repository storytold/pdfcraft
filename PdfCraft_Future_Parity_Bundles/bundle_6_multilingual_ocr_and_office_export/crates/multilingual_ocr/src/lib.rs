//! PdfCraft Multilingual OCR: Language selection, cleanup, and invisible searchable text layer generator.
//!
//! Subsystem covering Milestone M10:
//! - [P0] OCR document language selection (Latin, CJK, Cyrillic, Arabic)
//! - [P0] Searchable image output (invisible text layer 3 Tr)
//! - [P1] Pluggable OCR engine pipeline

pub mod language;
pub mod sandwich;
pub mod pipeline;

pub use language::OcrLanguage;
pub use sandwich::{RecognizedWord, SandwichPdfGenerator};
pub use pipeline::MultilingualOcrPipeline;
