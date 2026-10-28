//! PdfCraft Arlington: Structural PDF 1.0 - 2.0 dictionary validation based on the Arlington PDF Model.
//!
//! Subsystem covering Milestone M2:
//! - [P1] Structural validation against the Arlington PDF model
//! - [P1] PDF 2.0 UTF-8 text strings and Associated Files (/AF)

pub mod model;
pub mod validator;

pub use model::{ArlingtonDictionarySpec, ArlingtonKeySpec, ArlingtonType, PdfVersion};
pub use validator::{ArlingtonValidator, ValidationError};
