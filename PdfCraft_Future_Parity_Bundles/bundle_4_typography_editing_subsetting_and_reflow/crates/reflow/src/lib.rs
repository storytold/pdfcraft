//! PdfCraft Reflow: In-place paragraph reflow, line spacing, bi-directional text, and bullet lists.
//!
//! Subsystem covering Milestone M7:
//! - [P1] Line and paragraph spacing & boundary reflow
//! - [P1] Bulleted and numbered lists
//! - [P1] Edit right-to-left and CJK text

pub mod bidi;
pub mod paragraph;

pub use bidi::{BidiClassifier, TextDirection, TextRun};
pub use paragraph::{ParagraphReflowEngine, ReflowedLine};
