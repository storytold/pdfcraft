//! pdfcraft-export — Export a PDF ▸ Word, HTML, RTF (L4).
//!
//! The engine reduces each page to [`Page`]: paragraphs (text, box, size, bold/italic) and
//! images (encoded bytes and box), in reading order. The writers turn that into a flowing
//! document: headings are the paragraphs set larger than the body text, and a block that
//! continues another on its line (an italic word, a link) is a run of that block's paragraph.
//! Word keeps pages with
//! rotated text, or text overlapped by artwork or watermark-sized text, as positioned, editable
//! shapes to avoid reflowing overlays; other pages flow with images inline.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::HashMap;

mod pptx;
mod zip;

pub use pptx::pptx;
pub use zip::Zip;

mod as_docx;
mod as_html;
mod as_rtf;
mod layout;

pub use as_docx::docx;
pub(crate) use as_docx::hex;
#[cfg(test)]
use as_docx::{MAX_MEDIA, docx_table};
pub(crate) use as_html::esc;
pub use as_html::html;
#[cfg(test)]
use as_html::{allowed, base64};
pub use as_rtf::rtf;
pub use layout::{Block, Cell, Image, Page, Table};
#[cfg(test)]
use layout::{LOOKUP_STEPS, MAX_COLS, body_size};
pub(crate) use layout::{Run, paragraphs, tables};

#[cfg(test)]
mod tests;
