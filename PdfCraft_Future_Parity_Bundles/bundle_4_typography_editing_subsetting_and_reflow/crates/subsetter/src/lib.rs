//! PdfCraft Subsetter: Font subset expansion, glyph injection, and ToUnicode synchronization.
//!
//! Subsystem covering Milestone M7:
//! - [P0] Edit existing text in place: font subsetting and glyph expansion

pub mod tables;
pub mod injector;

pub use tables::{FontTableDirectory, TableRecord};
pub use injector::FontSubsetExpander;
