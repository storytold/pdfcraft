//! pdfcraft-annot — comments (annotations), ISO 32000-2 §12.5, execution plan M5.1–M5.3.
//!
//! Builders for the comment types Acrobat's commenting tools create (sticky note, highlight,
//! underline, strikethrough, squiggly, rectangle, oval, line/arrow, freehand ink, text box),
//! appearance streams for them ([`appearance`]), and the edits a comment goes through: reply,
//! change its text, recolour, move, resize and delete.
//!
//! Addressing: a comment is `(page, index)`, its position in the page's `/Annots` array, which
//! is what `pdfcraft_render::Annotation::index` reports. Inline annotation dictionaries are
//! promoted to indirect objects when they are edited (replies need a reference to point at).
//!
//! Every edit mutates a `pdfcraft_cos::Document` (copy-on-write); callers snapshot it first
//! for undo. Keys we do not understand are left alone.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

mod create;
mod draw;
mod info;
mod lookup;
mod model3d;
mod modify;
mod threed;
mod types;

#[cfg(test)]
use create::num_array;
pub use create::{add_annotation, callout_attach, orient_image_stamp};
pub use draw::{AppearanceCache, set_appearance, with_missing_appearances, with_missing_appearances_cached};
pub use info::{Props, Summary, is_comment_subtype, props, set_info, summaries};
#[cfg(test)]
use lookup::{ANNOTS_ENTRIES_COPIED, PageWalk, annot_dict, annot_ref};
use lookup::{annots, page_ref};
pub use lookup::{
    has_visible_fill_sign, is_fill_sign, isolate_annotation, isolate_annotations, page_refs, picture_rotation, set_annots, signature_image,
};
pub use model3d::{Model3d, Model3dKind, model_3d, model_3d_annotations};
pub(crate) use modify::n;
pub use modify::{
    add_reply, add_text_replacement, deletable, delete_all_annotations, delete_annotation, erase_ink, fillable, goes_with, lift_annotation,
    move_annotation, set_contents, set_fill, set_font_size, set_locked, set_marked, set_part_opacity, set_points, set_rect, set_review_state,
    set_style, with_replies_and_popups,
};
pub use threed::{Activation as Activation3d, MAX_MODEL_BYTES, ModelFormat, NewModel3d, PosterPicture, add_3d_model, check_model};
pub use types::{
    AttachIcon, FillMark, LineEnding, Markup, Meta, NOTE_SIZE, NewAnnotation, NoteIcon, OverlayFont, OverlayLook, ReviewState, Rgb, Shape,
    StampGroup, StampKind, Style, StyleChange, TextReplacement, rect_quad,
};

pub mod appearance;
pub mod links;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AnnotError {
    #[error("the document has no page tree")]
    NoPageTree,
    /// Holds the 0-based index; the message names the 1-based page a user counts (#1550).
    #[error("page {} does not exist", .0.saturating_add(1))]
    NoSuchPage(usize),
    #[error("there is no comment {index} on page {page}")]
    NoSuchAnnotation { page: usize, index: usize },
    #[error("{0}")]
    Invalid(String),
    #[error("{0} comments can't be restyled yet (their appearance can't be regenerated)")]
    Unsupported(String),
    /// Text the standard fonts can't draw where `?` would be permanent or replace a good drawing:
    /// a redaction's overlay text, or an edit that would redraw a comment another program drew
    /// with a font that has those characters. The message names the character.
    #[error("{0}")]
    Undrawable(String),
    /// A 3D model file that is neither U3D (starts with `U3D` and a zero byte) nor PRC (starts `PRC`).
    #[error("not a 3D model: the file must be U3D (ECMA-363) or PRC (ISO 14739-1)")]
    NotAModel,
    /// A 3D model larger than `MAX_MODEL_BYTES`.
    #[error("the 3D model is {size} bytes; the largest accepted is {max}")]
    ModelTooLarge { size: usize, max: usize },
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
}
