//! Rich Text Format through craft-rtf's full writer (storytold/craft-libs#793): the pages are
//! mapped onto a `craft_wordmodel::Document` ([`super::as_word`]), every page flowing.

use super::as_word::{Meta, flow_doc};
use super::layout::Page;

/// The body text's font, as PdfCraft's RTF has always named it (a reader without Helvetica draws
/// it in its sans-serif, which the font table's `\fswiss` asks for).
const FONT: &str = "Helvetica";

/// Rich Text Format: paragraphs with their sizes, bold, italic and colours, headings, tables,
/// pictures, and each page in a section of its own size.
pub fn rtf(pages: &[Page]) -> String {
    rtf_with_title(pages, "")
}

/// [`rtf`] with the document's title (`\info\title`).
pub fn rtf_with_title(pages: &[Page], title: &str) -> String {
    craft_rtf::doc::export(&flow_doc(pages, &Meta { title, language: None, font: FONT }))
}
