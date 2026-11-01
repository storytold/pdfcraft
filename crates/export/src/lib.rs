//! pdfcraft-export — Export a PDF ▸ Word, HTML, RTF, Markdown (L4).
//!
//! The engine reduces each page to [`Page`]: paragraphs (text, box, size, bold/italic) and
//! images (encoded bytes and box), in reading order. The writers turn that into a flowing
//! document: headings are the paragraphs set larger than the body text, and a block that
//! continues another on its line (an italic word, a link) is a run of that block's paragraph.
//! Word keeps pages with
//! rotated text, text overlapped by artwork or watermark-sized text, form fields or text framed by
//! rules (and every page of a document most of whose pages are such) as positioned, editable
//! shapes: the page's vector paths behind the text, its text line by line, its fields as Word
//! form fields. Other pages flow with images inline.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod pptx;
mod zip;

pub use pptx::pptx;
pub use zip::Zip;

mod as_docx;
mod as_html;
mod as_markdown;
mod as_rtf;
mod layout;

pub(crate) use as_docx::hex;
#[cfg(test)]
use as_docx::{FRAMING_SHAPES, MAX_MEDIA, docx_table, word_font};
pub use as_docx::{MAX_DOCX_PAGE_POINTS, MAX_DOCX_PAGE_SHAPES, docx, docx_checked};
pub(crate) use as_html::esc;
pub use as_html::html;
#[cfg(test)]
use as_html::{allowed, base64};
pub use as_markdown::markdown;
#[cfg(test)]
use as_markdown::{emphasis, paragraph};
pub use as_rtf::rtf;
pub use layout::{Block, Cell, Field, FieldKind, Image, Page, Shape, Stroke, Table};
#[cfg(test)]
use layout::{LOOKUP_STEPS, MAX_COLS, body_size};
pub(crate) use layout::{Run, paragraphs, tables};

#[cfg(test)]
mod tests;
