//! Undo/redo: the editing state of a document, its snapshots and their labels.

use std::sync::Arc;

use crate::{AppearanceCache, Edit, EditError, Shape};

/// Undo/redo depth. Snapshots share unchanged data, so this is memory-cheap.
pub(crate) const MAX_UNDO: usize = 100;

/// The passwords that go with one state of a document (protection can change them).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Keys {
    /// What the renderer and inspector open the working file with (the user password).
    pub(crate) render: Option<String>,
    /// What the editor re-opens a saved file with: the strongest password known (owner if any).
    pub(crate) reopen: Option<String>,
}

/// What an edit can change, and so what the view data must be rebuilt from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Anything: re-inspect the whole document.
    Full,
    /// Document-information entries; page content and appearances are unchanged.
    Metadata,
    /// Only comments: re-read the comment list from the object graph.
    Comments,
    /// Only form field values (and their widget appearances).
    Form,
    /// Link annotations; the full inspection is refreshed, but page-content indexes stay valid.
    Links,
    /// Only page content streams, resources and added content/marks: the page tree,
    /// annotations, form fields, links, outline and fonts are unchanged, so the full
    /// re-inspection is skipped (seconds on very large files).
    Content,
}

/// Edits during which field scripts run.
pub(crate) fn uses_scripts(edit: &Edit) -> bool {
    match edit {
        Edit::SetFieldValue { .. } | Edit::ApplyScriptChanges { .. } | Edit::SetFieldScript { .. } | Edit::SetCalculationOrder { .. } => true,
        Edit::Batch { edits, .. } => edits.iter().any(uses_scripts),
        _ => false,
    }
}

pub(crate) fn scope_of(edit: &Edit) -> Scope {
    match edit {
        Edit::SetInfo { .. } => Scope::Metadata,
        // The Attachments list is read from the whole document.
        Edit::AddAttachment { .. } | Edit::DeleteAttachment { .. } => Scope::Full,
        // A file attachment also changes the Attachments list.
        Edit::AddAnnotation(a) if matches!(a.shape, Shape::Attachment { .. }) => Scope::Full,
        Edit::AddMeasurement(_)
        | Edit::AddAnnotation(_)
        | Edit::AddCustomStamp { .. }
        | Edit::DeleteAnnotation { .. }
        | Edit::SetAnnotationContents { .. }
        | Edit::ReplyToAnnotation { .. }
        | Edit::SetAnnotationStatus { .. }
        | Edit::MarkAnnotation { .. }
        | Edit::LockAnnotation { .. }
        | Edit::ReplaceText { .. }
        | Edit::EraseInk { .. }
        | Edit::MoveAnnotation { .. }
        | Edit::ReshapeAnnotation { .. }
        | Edit::ResizeAnnotation { .. }
        | Edit::StyleAnnotation { .. }
        | Edit::FillAnnotation { .. }
        | Edit::FontSizeAnnotation { .. }
        | Edit::SetAnnotationInfo { .. } => Scope::Comments,
        Edit::AddLink { .. } | Edit::SetLink { .. } | Edit::DeleteLink { .. } | Edit::RemoveLinks { .. } | Edit::AddLinks { .. } => Scope::Links,
        Edit::SetFieldValue { .. } | Edit::ResetForm { .. } | Edit::SetFieldImage { .. } | Edit::ApplyScriptChanges { .. } => Scope::Form,
        // Page content, resources and added content/marks only: the page tree, annotations,
        // fields, links and document metadata are untouched (see `refresh_content`).
        Edit::MoveObjects { .. }
        | Edit::EditTextLine { .. }
        | Edit::EditTextBlock { .. }
        | Edit::EditPageImage { .. }
        | Edit::AddText { .. }
        | Edit::AddImage { .. }
        | Edit::UpdateContent { .. }
        | Edit::DeleteContent { .. }
        | Edit::ReplaceImage { .. }
        | Edit::PasteContent { .. }
        | Edit::AddHeaderFooter { .. }
        | Edit::AddWatermark { .. }
        | Edit::AddBackground { .. }
        | Edit::RemoveMarks { .. } => Scope::Content,
        Edit::Batch { edits, .. } => {
            let mut scopes = edits.iter().map(scope_of);
            let first = scopes.next().unwrap_or(Scope::Full);
            if scopes.all(|s| s == first) { first } else { Scope::Full }
        }
        _ => Scope::Full,
    }
}

/// One undo/redo step: its label, the document state, its passwords, and what it changed.
pub(crate) type Snapshot = (Label, pdfcraft_cos::Document, Keys, Scope);

/// Editing state of a document (absent when the document cannot be edited yet, e.g. encrypted).
#[derive(Clone)]
pub(crate) struct Editor {
    pub(crate) cos: pdfcraft_cos::Document,
    /// The file as last opened or saved. Incremental saves append to it, and Revert opens it.
    /// Undo may restore a snapshot whose own base is an earlier revision; this does not follow it.
    pub(crate) saved: Arc<Vec<u8>>,
    /// Passwords that open [`Self::saved`].
    pub(crate) saved_keys: Keys,
    /// [`Self::saved`] was never written where the document belongs: a new document (combine,
    /// extract, create) or one restored from recovery. It stays unsaved until a save, whatever
    /// undo brings back, or closing it would lose the whole document without asking.
    pub(crate) unsaved_file: bool,
    pub(crate) undo: Vec<Snapshot>,
    pub(crate) redo: Vec<Snapshot>,
    pub(crate) keys: Keys,
    /// Display appearances built for annotations without any, reused while no annotation edit
    /// runs (content edits leave every annotation dictionary alone). Cleared on any refresh
    /// that may change annotations; see [`AppearanceCache`].
    pub(crate) appearances: AppearanceCache,
    /// Display appearances drawn for form fields the file leaves to the viewer (#3470), reused
    /// for each widget while the objects it was drawn from are unchanged; see
    /// [`pdfcraft_forms::FieldAppearanceCache`].
    pub(crate) field_appearances: pdfcraft_forms::FieldAppearanceCache,
    /// The text blocks last computed for one page, reused while the document is unchanged.
    pub(crate) text_blocks: TextBlocksCache,
}

/// One page's text blocks ([`pdfcraft_edit::text_blocks`]) and the document state they were read
/// from. Opening a paragraph and applying its edit both need them, and grouping a long page is not
/// cheap. The state ([`pdfcraft_cos::ReadState`]) changes with every edit and save and returns
/// with undo, so a stale entry never matches, also after the document has been edited.
#[derive(Default)]
pub(crate) struct TextBlocksCache(std::sync::Mutex<Option<CachedTextBlocks>>);

/// The document state, the page (0-based) and its text blocks.
type CachedTextBlocks = (pdfcraft_cos::ReadState, usize, Arc<Vec<pdfcraft_edit::TextBlock>>);

impl Clone for TextBlocksCache {
    fn clone(&self) -> Self {
        // A poisoned cache only loses the cached blocks.
        Self(std::sync::Mutex::new(self.0.lock().ok().and_then(|c| c.clone())))
    }
}

impl Editor {
    /// The text blocks on `page` (0-based), from the cache when the document hasn't changed.
    pub(crate) fn text_blocks(&self, page: usize) -> Result<Arc<Vec<pdfcraft_edit::TextBlock>>, EditError> {
        if let Ok(cache) = self.text_blocks.0.lock()
            && let Some((state, p, blocks)) = cache.as_ref()
            && *p == page
            && state.matches(&self.cos)
        {
            return Ok(blocks.clone());
        }
        let blocks = Arc::new(pdfcraft_edit::text_blocks(&self.cos, page)?);
        if let Ok(mut cache) = self.text_blocks.0.lock() {
            *cache = Some((self.cos.read_state(), page, blocks.clone()));
        }
        Ok(blocks)
    }
}

/// What an edit is called in the Edit menu and in the history ("Undo Rotate pages"), in two parts:
/// a whole English wording, and the one value the document itself supplies for the wording's hole
/// (a file name, a field name). An interface in another language looks the wording up and says the
/// label with [`said`](Self::said); [`as_str`](Self::as_str) is the English, said the same way, so
/// the two cannot drift apart.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// The wording, with one named hole (`{name}`) where `value` goes. Listed in [`EDIT_LABELS`](crate::EDIT_LABELS).
    pub wording: &'static str,
    /// What the document supplies, when the wording has a hole for it.
    pub value: Option<String>,
    text: String,
}

impl Label {
    /// A wording with nothing to fill in.
    pub fn of(wording: &'static str) -> Self {
        Self { wording, value: None, text: wording.to_string() }
    }

    /// A wording whose hole the document fills in.
    pub fn with(wording: &'static str, value: impl Into<String>) -> Self {
        let mut label = Self { wording, value: Some(value.into()), text: String::new() };
        label.text = label.said(wording);
        label
    }

    /// The label read with this wording: the interface's translation of [`wording`](Self::wording),
    /// or the English wording itself. What the document supplies goes in as it stands and is not
    /// read again, so a name with braces in it stays whole.
    pub fn said(&self, wording: &str) -> String {
        let Some(value) = &self.value else { return wording.to_owned() };
        // `{` and `}` are ASCII, so these are char boundaries.
        match (wording.find('{'), wording.find('}')) {
            (Some(open), Some(close)) if open < close => {
                let mut said = String::with_capacity(wording.len() + value.len());
                said.push_str(&wording[..open]);
                said.push_str(value);
                said.push_str(&wording[close + 1..]);
                said
            }
            // A translation that lost its hole is said as it stands rather than losing the value.
            _ => wording.to_owned(),
        }
    }

    /// The label as English reads it.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// The passwords after `edit`, if it changes them.
pub(crate) fn keys_after(edit: &Edit) -> Option<Keys> {
    match edit {
        Edit::Protect(p) => {
            Some(Keys { render: p.open_password.clone(), reopen: p.permissions_password.clone().or_else(|| p.open_password.clone()) })
        }
        Edit::RemoveProtection => Some(Keys::default()),
        Edit::Batch { edits, .. } => edits.iter().rev().find_map(keys_after),
        _ => None,
    }
}
