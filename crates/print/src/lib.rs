//! Printing (execution plan M10.5). Layer L4.
//!
//! [`impose`] turns the pages to print into a print-ready PDF of *sheets*, laid out the way
//! Acrobat's Print dialog describes them:
//! - **Size**: one page per sheet, Fit, Actual size, Shrink oversized pages or a custom scale,
//!   centred;
//! - **Multiple**: pages per sheet in a grid (2, 4, 6, 9, 16 or custom), four page orders,
//!   optional page borders and auto-rotation;
//! - **Booklet**: saddle-stitched spreads (both sides, front or back only; left or right
//!   binding);
//! - **Poster**: each page enlarged across several sheets with overlap and cut marks;
//! - orientation (auto, portrait, landscape) and what to print (the document, with markups,
//!   with stamps, or form fields only; annotations print only with their Print flag).
//!
//! The sheets are written as a new, unencrypted, garbage-collected PDF; [`spool`] hands it to
//! the system's print spooler (CUPS on macOS and Linux).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_content::Matrix;
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};

pub mod range;
pub mod spool;
#[cfg(test)]
mod tests;

pub use range::{Subset, select_listed, select_pages};

mod error;
mod imposition;
mod settings;
mod sheets;

pub use error::{PrintError, WORDINGS, Words, in_english};
#[cfg(test)]
use imposition::{PAGES_LISTED, appearance, appearance_matrix, decoded_contents, page_form, prints};
pub use imposition::{impose, keep_printed, sheet_pages};
pub use settings::{
    Binding, BookletSubset, Content, Crop, LABEL_PAPER, Layout, Orientation, PAPERS, PageOrder, Raster, Settings, SizeMode, booklet_duplex,
    cups_media, media_option,
};
#[cfg(test)]
use sheets::text_direction;
pub use sheets::{LABEL_MARGIN, MARGIN, Placement, Sheet, layout, matching_paper, text_reads_up};
