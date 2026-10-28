//! PdfCraft Appearances: Complete appearance stream (/AP) synthesizer for all 18 annotation types and XHTML rich text (/RC).
//!
//! Subsystem covering Milestone M5:
//! - [P0] Generate correct appearances for every markup type
//! - [P0] Text box comment with rich text (/RC)
//! - [P1] Insert-text proposal (caret)
//! - [P1] All ten line-ending styles and dashed/cloudy borders

pub mod markup_types;
pub mod rich_text;
pub mod synthesizer;

pub use markup_types::{AnnotationMarkup, LineEndingStyle};
pub use rich_text::{RichTextParser, RichTextSpan};
pub use synthesizer::{AppearanceStream, AppearanceSynthesizer};
