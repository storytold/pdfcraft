//! L7 UniFFI bridge for PdfCraft mobile (iOS and Android).
//!
//! Provides a safe, panic-free foreign-function interface to the PdfCraft Rust engine.

pub mod advanced;
pub mod cache;
pub mod document;
pub mod edit;
pub mod engine;
pub mod error;
pub mod forms;
pub mod ocr;
pub mod prefetch;
pub mod profile;
pub mod render;
pub mod signing;
pub mod threading;
pub mod tools;

pub use advanced::*;

pub use cache::*;
pub use document::*;
pub use edit::*;
pub use engine::*;
pub use error::*;
pub use forms::*;
pub use ocr::*;
pub use prefetch::*;
pub use profile::*;
pub use render::*;
pub use signing::*;
pub use threading::*;
pub use tools::*;

uniffi::setup_scaffolding!();
