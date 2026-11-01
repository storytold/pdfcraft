//! Edit a PDF ▸ Add content: text and images added as page content (execution plan M7.4).
//!
//! Each added item is its own content stream on the page, tagged `/PCMark /Added`, with its
//! parameters (text, font, size, colour, alignment, box; or image and box) kept in the stream
//! dictionary under `/PCAdded`. That keeps the item editable later (move, resize, retype,
//! reformat, delete) without content-stream surgery, while every viewer sees ordinary page
//! content. Boxes are in display space (points, origin at the bottom-left of the page as shown,
//! after `/UserUnit` and `/Rotate`), so items stay upright on rotated pages; an item records the
//! `/UserUnit` it was written for.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::{
    GlyphError, GlyphOutline, ShapedCluster, ShapedScript, Std14, literal, shape_arabic, shape_hebrew, shape_thai, shaped_glyph, win_ansi,
};
use unicode_bidi::{Level, ParagraphBidiInfo};

use crate::{EditError, MarkKind, check, contents, matrix_number, n, page_list, place_tagged};

mod draw;
mod layout;
mod store;
mod types;

#[cfg(test)]
use draw::{GLYPHS_PER_FONT, LaidLine, draw_laid_shaped};
#[cfg(test)]
use layout::Piece;
#[cfg(test)]
pub(crate) use layout::WRAP_CALLS;
pub use layout::{PASTE_STEP, first_undrawable, lines, paste_place, paste_rect, text_rect, text_width};
pub use store::{add_content, delete_content, isolate_content, list_added, scan_markers, update_content};
pub use types::{Added, AddedImage, AddedText, Align, CHAR_SPACING_LIMIT, Family, MarkerScan};

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(AddedText),
    Image(AddedImage),
}

impl Content {
    pub fn rect(&self) -> [f64; 4] {
        match self {
            Content::Text(t) => t.rect,
            Content::Image(i) => i.rect,
        }
    }

    /// The same item with its box and text size scaled by `factor`.
    fn scaled(self, factor: f64) -> Content {
        match self {
            _ if factor == 1.0 => self,
            Content::Text(t) => {
                Content::Text(AddedText { rect: t.rect.map(|v| v * factor), size: t.size * factor, char_spacing: t.char_spacing * factor, ..t })
            }
            Content::Image(i) => Content::Image(AddedImage { rect: i.rect.map(|v| v * factor), ..i }),
        }
    }

    /// The same item moved/resized to `rect` (text keeps its computed height).
    pub fn with_rect(&self, rect: [f64; 4]) -> Content {
        match self {
            Content::Text(t) => Content::Text(AddedText { rect, ..t.clone() }),
            Content::Image(i) => Content::Image(AddedImage { rect, ..i.clone() }),
        }
    }
}

fn align_code(a: Align) -> i64 {
    match a {
        Align::Left => 0,
        Align::Center => 1,
        Align::Right => 2,
        Align::Justify => 3,
    }
}

fn align_from(code: Option<i64>) -> Align {
    match code {
        Some(1) => Align::Center,
        Some(2) => Align::Right,
        Some(3) => Align::Justify,
        _ => Align::Left,
    }
}

#[cfg(test)]
mod arabic_tests;
