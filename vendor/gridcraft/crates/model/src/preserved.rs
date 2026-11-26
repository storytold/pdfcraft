//! File content GridCraft does not model, carried through unchanged so that saving a workbook
//! never silently drops it (XLSX pass-through: custom XML, external links, connections, query
//! tables, form controls, OLE objects…).
//!
//! The data is opaque to everything except the file-format crate: the model only holds it, so a
//! sheet's kept content disappears with the sheet. It is not part of the JSON format.

use std::sync::Arc;

use crate::sheet::Visibility;

/// A relationship from an owning part (the package root, the workbook or a sheet, or a kept part)
/// to a kept part or to an external target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedRel {
    /// The id in the source file (`rId7`). Ids of an owner GridCraft writes are reassigned on save.
    pub id: String,
    /// The full relationship type URI.
    pub rel_type: String,
    /// The original part name of the target (no leading `/`), or the raw target of an external
    /// relationship. External targets are never opened.
    pub target: String,
    pub external: bool,
}

/// A kept package part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedPart {
    /// Part name in the source file (no leading `/`).
    pub name: String,
    pub content_type: String,
    pub data: Arc<[u8]>,
    /// The part's own relationships, ids unchanged (the part's bytes refer to them).
    pub rels: Vec<PreservedRel>,
}

/// A raw XML element of an owning part that refers to kept parts, re-emitted on save with its
/// relationship ids rewritten, or that Excel needs back unchanged (the workbook's `fileVersion`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedXml {
    /// Which slot of the owner it belongs in (`externalReferences`, `fileVersion`, `ext`,
    /// `oleObjects`, `controls`).
    pub slot: String,
    /// The element, namespace declarations it needs included.
    pub xml: String,
}

/// A chart sheet (a sheet that holds one chart and no cells) kept as it was in the source file. Its
/// part, drawing, chart, style and colour parts are in [`Preserved::parts`]; the workbook
/// relationship to it is in [`Preserved::rels`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreservedSheet {
    pub name: String,
    pub visibility: Visibility,
    /// The id of the workbook relationship to the chart sheet part, in the source file.
    pub rel_id: String,
    /// How many worksheets come before it: its position among the model's sheets.
    pub before: usize,
    /// Its `sheetId` in the source file.
    pub sheet_id: u32,
}

/// Unmodelled content kept for one owner (the workbook, which also holds package-root content,
/// or one sheet).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preserved {
    /// Relationships of the package root (`_rels/.rels`) to kept parts (workbook owner only).
    pub package_rels: Vec<PreservedRel>,
    /// Relationships of the owning workbook or sheet part to kept parts.
    pub rels: Vec<PreservedRel>,
    /// Every kept part reachable from `package_rels` and `rels`.
    pub parts: Vec<PreservedPart>,
    /// Owner XML elements that refer to kept parts.
    pub elements: Vec<PreservedXml>,
    /// The sheet's legacy VML drawing when kept elements need its shapes (form controls, OLE
    /// objects). Its own relationships point into `parts`.
    pub vml: Option<PreservedPart>,
    /// Chart sheets, in file order (workbook owner only).
    pub sheets: Vec<PreservedSheet>,
}

impl Preserved {
    pub fn is_empty(&self) -> bool {
        self.package_rels.is_empty()
            && self.rels.is_empty()
            && self.parts.is_empty()
            && self.elements.is_empty()
            && self.vml.is_none()
            && self.sheets.is_empty()
    }
    /// Total bytes held.
    pub fn byte_len(&self) -> usize {
        self.parts.iter().chain(self.vml.iter()).fold(0usize, |n, p| n.saturating_add(p.data.len()))
    }
}
