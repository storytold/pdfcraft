//! Rich Text Format through craft-rtf's full writer (storytold/craft-libs#793): the pages are
//! mapped onto a `craft_wordmodel::Document` ([`super::as_word`]), every page flowing.

use super::as_word::{Meta, flow_doc};
use super::layout::Page;

/// The body text's font, as PdfCraft's RTF has always named it (a reader without Helvetica draws
/// it in its sans-serif, which the font table's `\fswiss` asks for).
const FONT: &str = "Helvetica";

/// The application the RTF names as its writer (`{\*\generator}`).
pub(crate) const GENERATOR: &str = "PdfCraft";

/// Rich Text Format: paragraphs with their sizes, bold, italic and colours, headings, tables,
/// pictures, and each page in a section of its own size.
pub fn rtf(pages: &[Page]) -> String {
    rtf_with_title(pages, "")
}

/// [`rtf`] with the document's title (`\info\title`).
pub fn rtf_with_title(pages: &[Page], title: &str) -> String {
    craft_rtf::doc::export_with_generator(&flow_doc(pages, &Meta { title, language: None, font: FONT }), GENERATOR)
}

/// RTF plus zero-based pages where nonempty pictures could not be embedded. Page text and
/// other surviving content are retained; this is partial-output reporting, not a page drop.
pub fn rtf_with_title_checked(pages: &[Page], title: &str) -> (String, Vec<usize>) {
    let mut omitted = Vec::new();
    let doc = super::as_word::flow_doc_with_omissions(pages, &Meta { title, language: None, font: FONT }, &mut omitted);
    (craft_rtf::doc::export_with_generator(&doc, GENERATOR), omitted)
}
