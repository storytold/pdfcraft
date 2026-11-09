//! pdfcraft-export — Export a PDF ▸ Word, HTML, RTF, Markdown (L4).
//!
//! The engine reduces each page to [`Page`]: paragraphs (text, box, size, bold/italic) and
//! images (encoded bytes and box), in reading order. The writers turn that into a flowing
//! document: headings are the paragraphs set larger than the body text; a block that
//! continues another on its line (an italic word, a link) is a run of that block's paragraph, and
//! a paragraph that wraps onto the next line runs on there. Word keeps pages with
//! rotated text, or text overlapped by artwork or watermark-sized text, as positioned, editable
//! shapes to avoid reflowing overlays; other pages flow with images inline.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod pptx;
mod zip;

pub use pptx::pptx;
pub use zip::Zip;

mod as_docx;
mod as_html;
mod as_markdown;
mod as_rtf;
mod clip;
mod flow;
mod layout;

pub(crate) use as_docx::hex;
#[cfg(test)]
use as_docx::{MAX_MEDIA, docx_table};
pub use as_docx::{docx, docx_with_language};
pub(crate) use as_html::esc;
#[cfg(test)]
use as_html::{allowed, base64};
pub use as_html::{html, html_simple, html_simple_with_language, html_with_language};
pub use as_markdown::markdown;
#[cfg(test)]
use as_markdown::{emphasis, paragraph};
pub use as_rtf::rtf;
pub use clip::{ClipBuilder, ClipSpan, ClipStyle, ClipText, MAX_CLIP_HTML_BYTES, MAX_CLIP_SPANS, MAX_CLIP_TEXT_BYTES, clip_html, font_family};
#[cfg(test)]
use flow::list_item;
pub use layout::{Block, Cell, Image, Page, Table};
#[cfg(test)]
use layout::{LOOKUP_STEPS, MAX_COLS, body_size};
pub(crate) use layout::{Run, flowing_paragraphs, tables};

#[cfg(test)]
mod tests;
