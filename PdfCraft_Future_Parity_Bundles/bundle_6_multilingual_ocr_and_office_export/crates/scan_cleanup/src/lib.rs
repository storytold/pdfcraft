//! PdfCraft Scan Cleanup: Hough deskewing, Otsu binarization, and despeckle filtering.
//!
//! Subsystem covering Milestone M10:
//! - [P1] Detect and fix page rotation & deskew
//! - [P1] Despeckle and background removal

pub mod deskew;
pub mod binarize;
pub mod despeckle;

pub use deskew::DeskewEngine;
pub use binarize::BinarizationEngine;
pub use despeckle::DespeckleFilter;
