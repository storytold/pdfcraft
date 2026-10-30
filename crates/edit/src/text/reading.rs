//! Read-only structure replacements for the page-based Office reading model.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Document, ObjRef};

use super::blocks::ReadingBlocks;
use super::interp::{MAX_PAGE_TEXT, Shown};
use super::structure::{ContentKey, StructureTextIndex};
use super::{EditError, regions};

const MAX_STAGED_RUNS: usize = 100_000;

/// A bounded structure index shared by the pages of one read-only export.
/// Editable text and glyph-audit APIs keep their own operator-based interpretation.
pub struct ReadingContext<'a> {
    doc: &'a Document,
    pages: Vec<pdfcraft_model::Page>,
    structure: StructureTextIndex,
    atom_limit: usize,
}

impl<'a> ReadingContext<'a> {
    pub fn new(doc: &'a Document) -> Self {
        let pages = pdfcraft_model::pages(doc);
        let references: Vec<_> = pages.iter().map(|page| page.obj).collect();
        let structure = StructureTextIndex::new(doc, &references);
        Self { doc, pages, structure, atom_limit: MAX_STAGED_RUNS }
    }

    #[cfg(test)]
    pub(crate) fn with_atom_limit(doc: &'a Document, atom_limit: usize) -> Self {
        let mut context = Self::new(doc);
        context.atom_limit = atom_limit.min(MAX_STAGED_RUNS);
        context
    }

    /// A page's reading paragraphs, with an incompleteness flag for bounded form reading or
    /// structure replacements that could not safely be placed in this page-based model.
    pub fn blocks(&self, page: usize) -> Result<ReadingBlocks, EditError> {
        let page = self.pages.get(page).ok_or_else(|| EditError::Invalid(format!("page {} does not exist", page.saturating_add(1))))?;
        let (lines, truncated) = super::lines::reading_lines(self.doc, page, &self.structure, self.atom_limit)?;
        Ok(ReadingBlocks { blocks: regions::group_reading_blocks(&lines), truncated })
    }
}

#[derive(Clone, Copy)]
pub(super) enum ContentContext {
    Page,
    Form(ObjRef),
    // A directly embedded form has no reference an MCR can identify. It can still be
    // included by an enclosing page-owned sequence, but must not borrow the page's MCIDs.
    Unaddressable,
}

#[derive(Default)]
pub(super) struct Coverage {
    members: HashMap<ContentKey, MemberCoverage>,
    ambiguous: bool,
}

#[derive(Default)]
struct MemberCoverage {
    begins: usize,
    ends: usize,
}

impl Coverage {
    fn single_closed(&self, key: &ContentKey) -> bool {
        self.members.get(key).is_some_and(|coverage| coverage.begins == 1 && coverage.ends == 1)
    }
}

pub(super) struct Observation {
    pub(super) owner: Option<usize>,
    pub(super) key: ContentKey,
    pub(super) first: bool,
}

pub(super) struct StructureScope<'a> {
    pub(super) index: &'a StructureTextIndex,
    pub(super) page: ObjRef,
    pub(super) context: ContentContext,
    pub(super) inherited_owner: Option<usize>,
    pub(super) coverage: &'a mut Coverage,
}

impl StructureScope<'_> {
    pub(super) fn observe(&mut self, mcid: i64) -> Option<Observation> {
        let stream = match self.context {
            ContentContext::Page => None,
            ContentContext::Form(reference) => Some(reference),
            ContentContext::Unaddressable => return None,
        };
        let key = ContentKey { page: self.page, stream, mcid };
        if !self.index.declared(&key) {
            return None;
        }
        let owner = self.index.owner(&key);
        // Only bounded structure declarations are retained, including keys whose tag has no
        // replacement. A nested sibling declaration must not inherit another tag's text.
        let count = self.coverage.members.entry(key).or_default();
        count.begins = count.begins.saturating_add(1);
        Some(Observation { owner, key, first: count.begins == 1 })
    }

    pub(super) fn close(&mut self, key: &ContentKey) {
        if let Some(count) = self.coverage.members.get_mut(key) {
            count.ends = count.ends.saturating_add(1);
        }
    }

    pub(super) fn mark_ambiguous(&mut self) {
        self.coverage.ambiguous = true;
    }
}

pub(super) struct ReadingAtom {
    pub(super) shown: Shown,
    pub(super) stream: usize,
    pub(super) barrier: usize,
}

/// Validate every member before changing any run. Structure membership is not a contiguous
/// operator interval: an unrelated paragraph may occur between two members of one owner.
pub(super) fn apply_structure(
    atoms: Vec<ReadingAtom>,
    index: &StructureTextIndex,
    page: ObjRef,
    coverage: &Coverage,
    forms_truncated: bool,
) -> (Vec<ReadingAtom>, bool) {
    let mut incomplete = index.incomplete(page);
    if index.owners_on_page(page).next().is_none() {
        return (atoms, incomplete);
    }
    if forms_truncated || coverage.ambiguous || atoms.iter().any(|atom| atom.shown.structure_ambiguous) {
        return (atoms, true);
    }

    struct Group {
        first: usize,
        bytes: usize,
        coherent: bool,
    }
    let mut groups: HashMap<usize, Group> = HashMap::new();
    let mut total_bytes = 0usize;
    for (at, atom) in atoms.iter().enumerate() {
        total_bytes = total_bytes.saturating_add(atom.shown.text.len());
        if let Some(owner) = atom.shown.structure_owner {
            let group = groups.entry(owner).or_insert(Group { first: at, bytes: 0, coherent: true });
            group.bytes = group.bytes.saturating_add(atom.shown.text.len());
            group.coherent &= atoms.get(group.first).is_some_and(|first| first.shown.coherent_reading_anchor(&atom.shown));
        }
    }
    // Do not add structure text to a page that has already consumed its reading allowance.
    if total_bytes > MAX_PAGE_TEXT {
        return (atoms, true);
    }

    let mut eligible = HashSet::new();
    for owner in index.owners_on_page(page) {
        let Some(group) = groups.get(&owner) else {
            incomplete = true; // A graphic-only or missing member has no safe text anchor.
            continue;
        };
        let members = index.members(owner);
        let Some(text) = index.text(owner) else {
            incomplete = true;
            continue;
        };
        if !group.coherent || members.is_empty() || !members.iter().all(|key| coverage.single_closed(key)) {
            incomplete = true;
            continue;
        }
        let Some(bytes) =
            total_bytes.checked_sub(group.bytes).and_then(|bytes| bytes.checked_add(text.len())).filter(|bytes| *bytes <= MAX_PAGE_TEXT)
        else {
            incomplete = true;
            continue;
        };
        total_bytes = bytes;
        eligible.insert(owner);
    }

    let mut out: Vec<ReadingAtom> = Vec::with_capacity(atoms.len());
    let mut destinations: HashMap<usize, usize> = HashMap::new();
    for atom in atoms {
        if let Some(owner) = atom.shown.structure_owner.filter(|owner| eligible.contains(owner)) {
            if let Some(destination) = destinations.get(&owner).and_then(|at| out.get_mut(*at)) {
                destination.shown.merge_reading(atom.shown);
                continue;
            }
            destinations.insert(owner, out.len());
        }
        out.push(atom);
    }
    for (owner, at) in destinations {
        if let (Some(atom), Some(text)) = (out.get_mut(at), index.text(owner)) {
            atom.shown.replace_reading_text(text.to_owned());
        }
    }
    // Empty replacements retain a zero-text pen anchor. Dropping it would turn the removed
    // glyph's advance into an inferred word space between its unowned neighbors.
    (out, incomplete)
}
