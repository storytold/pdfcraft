//! PdfCraft ISO Standards: ISO 19005 (PDF/A) and ISO 15930 (PDF/X) verification & fixups.
//!
//! Subsystem covering Milestone M11:
//! - [P1] Verify PDF/A compliance (1-4)
//! - [P1] Save as PDF/A (1b, 2b, 3b, 4)
//! - [P1] Verify PDF/X compliance and save as PDF/X (1a, 3, 4)

pub mod pdfa;
pub mod pdfx;

pub use pdfa::{PdfaEncryptionRule, PdfaLevel, PdfaOutputIntentRule, PdfaXmpMetadataRule};
pub use pdfx::PdfxBoxesRule;
