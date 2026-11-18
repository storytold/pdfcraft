//! Several edits as one undo step, or none at all: a document's editing state set aside at one
//! moment ([`EditMark`]), so that what follows can be folded into a single step of its history
//! ([`Session::fold_edits`]) or put back as it was ([`Session::restore_mark`]). A plug-in run
//! (RFC-0046) uses it: the run is one step named after the plug-in, and a run that fails leaves
//! the document, its passwords and its undo and redo history as they were.
//!
//! The mark holds the whole editing state (the object graph and the undo and redo snapshots),
//! which share unchanged data, so it is cheap and does not depend on how many steps the run
//! makes: the history's cap ([`MAX_UNDO`]) can drop old steps meanwhile without losing the way
//! back.

use super::{DocId, EditError, Label, MAX_UNDO, Scope, Session, guard};
use crate::history::{Keys, Snapshot};

/// The wording of a plug-in run's undo step ("Undo Plug-in: Page tools"), with the plug-in's name
/// in its hole. Listed in [`EDIT_LABELS`](crate::EDIT_LABELS), so an interface translates it.
pub const PLUGIN_RUN_LABEL: &str = "Plug-in: {name}";

/// A document's editing state at one moment (see the module docs).
pub struct EditMark {
    id: DocId,
    cos: pdfcraft_cos::Document,
    keys: Keys,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    generation: u64,
}

impl EditMark {
    /// The document it belongs to.
    pub fn doc(&self) -> DocId {
        self.id
    }
}

impl Session {
    /// Set document `id`'s editing state aside, to fold or put back what follows. An error when
    /// the document isn't open or can't be edited (an encrypted file without its password).
    pub fn edit_mark(&self, id: DocId) -> Result<EditMark, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let editor =
            doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_else(|| "it can't be edited".into())))?;
        Ok(EditMark {
            id,
            cos: editor.cos.clone(),
            keys: editor.keys.clone(),
            undo: editor.undo.clone(),
            redo: editor.redo.clone(),
            generation: doc.generation,
        })
    }

    /// Whether document `mark.doc()` changed since `mark` (an edit, an undo or a redo). A
    /// document that was closed meanwhile counts as changed.
    pub fn changed_since(&self, mark: &EditMark) -> bool {
        self.get(mark.id).is_none_or(|d| d.generation != mark.generation)
    }

    /// Make every edit since `mark` one undo step named `label`: Undo then takes the document
    /// back to where it was at the mark, in one step, and Redo brings all of it back. The redo
    /// history is gone, as after any edit. Returns whether there was anything to fold (nothing
    /// changes when the document is as it was).
    pub fn fold_edits(&mut self, mark: EditMark, label: Label) -> Result<bool, EditError> {
        if !self.changed_since(&mark) {
            return Ok(false);
        }
        let doc = self.doc_mut(mark.id)?;
        let editor = doc.editor.as_mut().ok_or(EditError::NothingToUndo)?;
        let mut undo = mark.undo;
        undo.push((label, mark.cos, mark.keys, Scope::Full));
        if undo.len() > MAX_UNDO {
            let extra = undo.len() - MAX_UNDO;
            undo.drain(..extra);
        }
        editor.undo = undo;
        editor.redo.clear();
        Self::sync_dirty(doc);
        Ok(true)
    }

    /// Put document `mark.doc()` back as it was at `mark`: its object graph, passwords and undo
    /// and redo history (the mark stays usable). Does nothing when it hasn't changed since.
    pub fn restore_mark(&mut self, mark: &EditMark) -> Result<(), EditError> {
        if !self.changed_since(mark) {
            return Ok(());
        }
        let doc = self.doc_mut(mark.id)?;
        let editor = doc.editor.as_mut().ok_or(EditError::NothingToUndo)?;
        editor.cos = mark.cos.clone();
        editor.keys = mark.keys.clone();
        editor.undo = mark.undo.clone();
        editor.redo = mark.redo.clone();
        Self::adopt_keys(doc);
        let refreshed = guard(|| Self::refresh(doc)).unwrap_or_else(|m| Err(EditError::Reopen(m)));
        if refreshed.is_err() {
            Self::show_released(doc);
        }
        doc.edit_notices.clear();
        doc.xfa_exited.clear();
        Self::sync_dirty(doc);
        doc.generation += 1;
        refreshed
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::tests::fixture;
    use crate::{DocId, Edit, Label, PLUGIN_RUN_LABEL, Session};

    fn session(n: usize) -> (Session, DocId) {
        let mut s = Session::new().with_clock(|| 1_700_000_000);
        let id = s.open("fixture.pdf", None, Arc::new(fixture(n)), None).unwrap();
        (s, id)
    }

    fn pages(s: &Session, id: DocId) -> usize {
        s.get(id).unwrap().info.pages.len()
    }

    #[test]
    fn edits_after_a_mark_fold_into_one_undo_step() {
        let (mut s, id) = session(3);
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let before = s.get(id).unwrap().undo_depth();
        let mark = s.edit_mark(id).unwrap();
        s.apply(id, Edit::DeletePages { pages: vec![2] }).unwrap();
        s.apply(id, Edit::InsertBlankPage { at: 0, width: 200.0, height: 300.0 }).unwrap();
        let edited = pages(&s, id);
        assert!(s.fold_edits(mark, Label::with(PLUGIN_RUN_LABEL, "Tidy")).unwrap());
        let doc = s.get(id).unwrap();
        assert_eq!(doc.undo_depth(), before + 1, "the run is one step");
        assert_eq!(doc.can_undo(), Some("Plug-in: Tidy"));
        assert_eq!(pages(&s, id), edited);
        s.undo(id).unwrap();
        assert_eq!(pages(&s, id), 3, "one Undo takes the whole run back");
        assert_eq!(s.get(id).unwrap().can_undo(), Some("Rotate page"), "the step before the run is still there");
        s.redo(id).unwrap();
        assert_eq!(pages(&s, id), edited, "Redo brings all of it back");
    }

    #[test]
    fn a_mark_with_nothing_after_it_folds_nothing() {
        let (mut s, id) = session(2);
        let mark = s.edit_mark(id).unwrap();
        assert!(!s.changed_since(&mark));
        assert!(!s.fold_edits(mark, Label::with(PLUGIN_RUN_LABEL, "Nothing")).unwrap());
        assert_eq!(s.get(id).unwrap().undo_depth(), 0);
        assert!(!s.get(id).unwrap().dirty);
    }

    #[test]
    fn restoring_a_mark_puts_the_document_and_its_history_back() {
        let (mut s, id) = session(3);
        s.apply(id, Edit::RotatePages { pages: vec![1], degrees: 90 }).unwrap();
        s.undo(id).unwrap();
        let mark = s.edit_mark(id).unwrap();
        // More edits than the history keeps: the mark still knows the way back.
        for _ in 0..(crate::history::MAX_UNDO + 5) {
            s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        }
        s.apply(id, Edit::DeletePages { pages: vec![2] }).unwrap();
        assert!(s.changed_since(&mark));
        s.restore_mark(&mark).unwrap();
        let doc = s.get(id).unwrap();
        assert_eq!(pages(&s, id), 3);
        assert_eq!(doc.info.pages[0].rotation, 0, "every edit after the mark is gone");
        assert_eq!(doc.undo_depth(), 0);
        assert_eq!(doc.can_redo(), Some("Rotate page"), "the redo history is as it was");
        assert!(!doc.dirty);
    }
}
