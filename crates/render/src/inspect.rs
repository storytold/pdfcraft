//! Document inspection: everything panels need that is not pixels.
//!
//! Page geometry comes from hayro (which resolves inheritance and rotation); the rest comes from
//! the lazy COS reader through a read-only lopdf object adapter. The original lopdf loader
//! remains the compatibility fallback. When neither can inspect a renderable file, panels
//! are empty and `warnings` says why.

use std::cell::OnceCell;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro::hayro_syntax::DecryptionError;
use hayro::hayro_syntax::LoadPdfError;
use hayro::hayro_syntax::Pdf;
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};
use pdfcraft_cos::page_labels::{
    MAX_LABEL_BYTES, MAX_LABEL_TOTAL_BYTES, MAX_LABEL_TREE_DEPTH, MAX_LABEL_TREE_WORK, MAX_PREFIX_BYTES, MAX_PREFIX_TOTAL_BYTES, alpha, roman,
};

use crate::OpenError;
use crate::structure::{LazyStructure, Structure};

mod annotations;
mod core;
mod model;
mod navigation;
mod resources;

#[cfg(test)]
use annotations::{MAX_ANNOTATION_BYTES, MAX_LINK_ATTACHMENT_TEXT_BYTES, MAX_REMOTE_FILE_SPEC_BYTES, retained};
use core::Inspector;
pub use core::{check_pages, inspect, pretty_date};
#[cfg(test)]
use core::{inspect_structure, spend};
pub use model::{
    Annotation, Attachment, AttachmentSource, DestView, Field, FieldKind, FontInfo, Layer, LayerOp, Link, LinkTarget, OutlineItem, PageInfo,
    PageTransition, RemoteDestination, TransitionStyle, Xfa, link_replies, thread_tops,
};
#[cfg(test)]
use navigation::{OUTLINE_DEPTH, OUTLINE_WINDOW};
pub use resources::attachment_data;
#[cfg(test)]
use resources::{xfa_config_dynamic_render, xfa_packet_text};

#[derive(Clone, Debug, Default)]
pub struct DocInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    /// Custom document properties: `/Info` entries with a key the spec does not name and a text
    /// value, as (name, text), in the dictionary's order (at most 256).
    pub custom: Vec<(String, String)>,
    pub pdf_version: String,
    pub file_size: usize,
    pub encrypted: bool,
    pub tagged: bool,
    pub has_javascript: bool,
    /// The form is (also) an XFA form, which isn't read yet (`form.xfa-*` in the parity list).
    pub xfa: Option<Xfa>,
    pub pages: Vec<PageInfo>,
    pub outline: Vec<OutlineItem>,
    /// The outline has bookmarks that `outline` leaves out (see `OUTLINE_WINDOW`, `OUTLINE_DEPTH`).
    pub outline_more: bool,
    pub annotations: Vec<Annotation>,
    pub fields: Vec<Field>,
    pub links: Vec<Link>,
    pub layers: Vec<Layer>,
    /// Radio-button layer groups (`/OCProperties /D /RBGroups`): turning one layer of a group on
    /// turns the others off.
    pub layer_groups: Vec<Vec<(u32, u16)>>,
    pub fonts: Vec<FontInfo>,
    pub attachments: Vec<Attachment>,
    /// Pages with a presentation transition (`/Trans`) or display time (`/Dur`), in page order.
    pub transitions: Vec<PageTransition>,
    /// Where the document opens (`/OpenAction`): the (0-based page, view) of its destination,
    /// named destinations resolved. `None` without one, for an action that goes nowhere in this
    /// document (a script, say), or for one that doesn't resolve to a page of it.
    pub open_dest: Option<(usize, DestView)>,
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests;
