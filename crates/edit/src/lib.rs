//! pdfcraft-edit — page content editing (L4). Today: page marks, Acrobat's Header & footer,
//! Watermark, Background and Bates numbering (execution plan M7.6).
//!
//! **How marks are stored.** Each mark is one content stream appended to (or, for content behind
//! the page, inserted after the backgrounds at the start of) the page's `/Contents`, drawn in
//! display space (after `/Rotate`) and wrapped in a marked-content artifact (§14.8.2.2) so
//! readers and screen readers skip it:
//! `/Artifact <</Type /Pagination /Subtype /Header /PCMark /HeaderFooter>> BDC … EMC`. The
//! `/PCMark` entry (also in the stream dictionary) names the kind of mark, which is how Update and
//! Remove find them again without decompressing page content. When
//! anything is drawn on top, the page's original content is wrapped in `q … Q` (two tiny streams)
//! so its graphics state cannot reach the mark. The closing stream also ends known unfinished
//! marked-content sequences before the new mark.
//!
//! Text uses standard Helvetica (WinAnsi) added to the page resources as `/PCHelv`; headers and
//! footers may use standard Times (`/PCTimes`) or Courier (`/PCCour`) instead (#3754).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use pdfcraft_fonts::{helvetica_width, literal, win_ansi};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EditError {
    /// Holds the 0-based index; the message names the 1-based page a user counts (#1550).
    #[error("page {} does not exist", .0.saturating_add(1))]
    NoSuchPage(usize),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
}

pub type Rgb = [f64; 3];

pub mod backdrop;
pub use backdrop::{Backdrop, backdrops_left_off, page_backdrop, page_backdrops};
mod background;
pub use background::page_background;
pub mod objects;
pub use objects::{EditableObject, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget, editable_objects, move_objects};

mod flatten;
pub use flatten::{flatten, flatten_fill_sign};
pub mod images;
pub use images::{ImageChange, PageImage, change_image, page_images, reading_images, rect_to_rect, turn_about_centre, turn_by_degrees};
pub mod bidi;
pub mod indic;
mod linebreak;
pub mod paragraph_align;
pub mod text;
pub use text::{
    BlockKey, BlockStyle, LineEdit, MAX_FORM_DEPTH, MAX_FORM_VISITS, RangeStyle, ReadingBlocks, TextBlock, TextLine, TextSession, block_overlapping,
    clear_block, glyph_text_lines, reading_blocks, reading_blocks_checked, replace_block, replace_line, rewrite_block, text_blocks, text_lines,
};
pub mod added;
pub use added::{
    Added, AddedImage, AddedText, Align, Content, Family, MarkerScan, add_content, delete_content, first_undrawable, isolate_content, list_added,
    scan_markers, update_content,
};

mod apply;
mod marks;
mod place;

pub use apply::{add_background, add_header_footer, add_watermark, marks_present, remove_marks};
#[cfg(test)]
use marks::bates_start;
pub use marks::{Background, Context, HeaderFooter, MarkKind, MarkSource, Watermark, expand};
use marks::{matrix_number, n};
pub(crate) use place::undrawable;
#[cfg(test)]
use place::{ISOLATION_MAX_DEPTH, OLD_WRAP_CLOSE, WRAP_CLOSE, WRAP_OPEN, is_wrap_close, page_fonts, stream_bytes, stream_tag, tagged, wrapper_role};
use place::{check, check_on_top, contents, page_list, place_tagged};
pub use place::{stamp, undrawable_message};

#[cfg(test)]
mod marked_isolation_tests;
#[cfg(test)]
mod tests;
