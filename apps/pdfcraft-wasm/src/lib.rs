//! The PdfCraft engine as a headless module: a web page opens a PDF from bytes, draws pages to
//! RGBA, lists and rewrites the text paragraphs, changes pages and saves the result. The page
//! brings its own interface; this crate has none (the full application is `pdfcraft-web`).
//!
//! [`core`] holds all the logic with plain Rust types, so it is tested natively. The wasm-bindgen
//! layer on top of it (`bindings`, wasm32 only) only converts types.

pub mod core;
pub mod editing;
pub mod review;
pub mod security;

#[cfg(target_arch = "wasm32")]
mod bindings;
