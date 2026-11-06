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

/// The names of the Print dialog's choices, shared by the `doc_print` tool and the settings
/// file, so a saved preference reads like a tool argument and there is one vocabulary to keep.
/// Each `parse_*` returns `None` for an unknown name (settings and tool arguments are
/// untrusted input; the caller keeps its default).
pub mod names {
    use super::{Binding, Content, SizeMode, spool::Duplex};

    /// Two-sided printing as `doc_print` names it: `None` leaves it to the printer (`printer`).
    pub fn duplex(duplex: Option<Duplex>) -> &'static str {
        match duplex {
            None => "printer",
            Some(Duplex::Off) => "off",
            Some(Duplex::LongEdge) => "long-edge",
            Some(Duplex::ShortEdge) => "short-edge",
        }
    }

    /// [`duplex`] read back; `None` for a name it doesn't give.
    pub fn parse_duplex(name: &str) -> Option<Option<Duplex>> {
        match name {
            "printer" => Some(None),
            "off" => Some(Some(Duplex::Off)),
            "long-edge" => Some(Some(Duplex::LongEdge)),
            "short-edge" => Some(Some(Duplex::ShortEdge)),
            _ => None,
        }
    }

    /// Acrobat's Comments & Forms choices.
    pub fn content(content: Content) -> &'static str {
        match content {
            Content::Document => "document",
            Content::DocumentAndMarkups => "document-and-markups",
            Content::DocumentAndStamps => "document-and-stamps",
            Content::FormFieldsOnly => "form-fields-only",
        }
    }

    pub fn parse_content(name: &str) -> Option<Content> {
        match name {
            "document" => Some(Content::Document),
            "document-and-markups" => Some(Content::DocumentAndMarkups),
            "document-and-stamps" => Some(Content::DocumentAndStamps),
            "form-fields-only" => Some(Content::FormFieldsOnly),
            _ => None,
        }
    }

    /// The page sizing choices; a custom scale is named `custom` and carries its percent
    /// separately (see [`parse_size_mode`]).
    pub fn size_mode(mode: SizeMode) -> &'static str {
        match mode {
            SizeMode::Fit => "fit",
            SizeMode::Actual => "actual",
            SizeMode::Shrink => "shrink",
            SizeMode::Custom(_) => "custom",
        }
    }

    /// `custom` takes `scale` as its percent; the caller has validated it.
    pub fn parse_size_mode(name: &str, scale: f64) -> Option<SizeMode> {
        match name {
            "fit" => Some(SizeMode::Fit),
            "actual" => Some(SizeMode::Actual),
            "shrink" => Some(SizeMode::Shrink),
            "custom" => Some(SizeMode::Custom(scale)),
            _ => None,
        }
    }

    /// A booklet's binding side.
    pub fn binding(binding: Binding) -> &'static str {
        match binding {
            Binding::Left => "left",
            Binding::Right => "right",
        }
    }

    pub fn parse_binding(name: &str) -> Option<Binding> {
        match name {
            "left" => Some(Binding::Left),
            "right" => Some(Binding::Right),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_choice_round_trips_through_its_name_and_unknown_names_are_none() {
            for d in [None, Some(Duplex::Off), Some(Duplex::LongEdge), Some(Duplex::ShortEdge)] {
                assert_eq!(parse_duplex(duplex(d)), Some(d));
            }
            for c in [Content::Document, Content::DocumentAndMarkups, Content::DocumentAndStamps, Content::FormFieldsOnly] {
                assert_eq!(parse_content(content(c)), Some(c));
            }
            for m in [SizeMode::Fit, SizeMode::Actual, SizeMode::Shrink, SizeMode::Custom(62.5)] {
                assert_eq!(parse_size_mode(size_mode(m), 62.5), Some(m));
            }
            for b in [Binding::Left, Binding::Right] {
                assert_eq!(parse_binding(binding(b)), Some(b));
            }
            assert_eq!(parse_binding("Right"), None);
            assert_eq!(parse_duplex("Long-Edge"), None);
            assert_eq!(parse_content(""), None);
            assert_eq!(parse_size_mode("fit-to-page", 100.0), None);
        }
    }
}
