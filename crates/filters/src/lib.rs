//! PDF stream filters (`craft-pdf::filters`).
//!
//! JPEG and JPEG 2000 images also decode to their samples on their own here ([`decode_dct`],
//! [`decode_jpx`]), for code that changes pixels and stores them again; the caller pays for that
//! first (`pdfcraft_cos::ImageBudget::pay_jpeg`).
// shortcut: `dct` and `jpx` live in this app beside the craft-pdf re-export; move them to
// craft-libs' stream codecs (AGENTS.md rule 9) and re-export them here instead.
pub use craft_pdf::filters::*;

mod dct;
mod jpx;

pub use dct::decode_dct;
pub use jpx::{JpxSamples, decode_jpx};
