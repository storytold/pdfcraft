//! SyncTeX search: map LaTeX source positions to PDF positions and back. Standalone, layer L0.
//!
//! Written from the public `synctex(5)` description of the file format. `SyncTex::parse` reads
//! a `.synctex` or `.synctex.gz` file (gzip is detected by its magic bytes) into sheets of boxes
//! and point records, in points from each page's top-left corner.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::PathBuf;

mod lookup;
mod parse;

pub use lookup::NEAR;
pub use parse::{MAX_DEPTH, MAX_INFLATED, MAX_RECORDS, SyncTex};

/// A rectangle `[x0, y0, x1, y1]` in points from the page's top-left corner.
pub type Rect = [f64; 4];

/// A position in a LaTeX source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePos {
    pub file: PathBuf,
    pub line: u32,
    pub column: u32,
}

/// A position in the PDF: a 1-based sheet (page) and the rectangles that mark it.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfPos {
    pub sheet: u32,
    pub rects: Vec<Rect>,
}

/// The source file is not among the inputs of the SyncTeX file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownSource;

/// Why a SyncTeX file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a SyncTeX file: it does not start with \"SyncTeX Version:\"")]
    NotSyncTex,
    #[error("the SyncTeX file is larger than the {} byte limit once unzipped", MAX_INFLATED)]
    TooLarge,
    #[error("the SyncTeX file has more than the {} records that are read", MAX_RECORDS)]
    TooManyRecords,
    #[error("the SyncTeX file could not be unzipped: {0}")]
    Inflate(String),
}
