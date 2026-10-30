//! Object selections restored by undoing the exact deletion that removed them.

use pdfcraft_engine::{Document, Edit, HistoryStepId, ImageEdit, MAX_MOVE_OBJECTS, ObjectKind, ObjectTarget};

use crate::{DocView, QuickTool};

#[derive(Default)]
pub(crate) struct DeletionHistory {
    records: Vec<(HistoryStepId, Selection)>,
}

#[derive(Clone)]
pub(super) struct Selection {
    page: usize,
    targets: Vec<ObjectTarget>,
    added: bool,
}

impl DeletionHistory {
    pub(super) fn prune(&mut self, doc: &Document) {
        // The engine bounds its history. Membership also drops abandoned redo branches and resets.
        self.records.retain(|(id, _)| doc.contains_history_step(id));
    }

    /// Associate a captured deletion selection with the new undo entry, if the edit created one.
    /// Prune stale entries, and use `previous` to exclude successful no-ops.
    pub(super) fn applied(&mut self, doc: &Document, previous: Option<&HistoryStepId>, selection: Option<Selection>) {
        self.prune(doc);
        if let (Some(id), Some(selection)) = (doc.next_undo_id(), selection)
            && Some(&id) != previous
        {
            self.records.push((id, selection));
        }
    }

    /// Find the captured selection for the exact undo or redo entry about to move.
    /// History identity distinguishes reused object indices and entries on abandoned branches.
    pub(super) fn before_step(&self, doc: &Document, undo: bool) -> Option<Selection> {
        let id = if undo { doc.next_undo_id() } else { doc.next_redo_id() }?;
        self.records.iter().find(|(step, _)| *step == id).map(|(_, selection)| selection.clone())
    }
}

/// Describe a supported object deletion using the target index recorded by that edit.
/// Other edits are outside this object-deletion subset.
fn deleted_target(edit: &Edit) -> Option<(usize, ObjectTarget)> {
    match edit {
        Edit::EditTextBlock { page, block, text, .. } if text.is_empty() => Some((*page, ObjectTarget { kind: ObjectKind::Text, index: *block })),
        Edit::EditPageImage { page, index, change: ImageEdit::Delete } => Some((*page, ObjectTarget { kind: ObjectKind::Image, index: *index })),
        Edit::DeleteContent { page, index } => Some((*page, ObjectTarget { kind: ObjectKind::Added, index: *index })),
        _ => None,
    }
}

impl Selection {
    pub(super) fn on_current_page(&self, view: &DocView) -> bool {
        view.history_selection_on_page(self.page)
    }

    /// Capture deleted targets only when the edit refers to the original page inventory.
    /// Batches must be bounded and on one page, with unique descending indices per kind and source targets before added content.
    /// Unsupported or ambiguous edits return `None` to avoid selecting another original object.
    pub(super) fn capture(edit: &Edit, view: &DocView, doc: &Document) -> Option<Self> {
        let deleted = match edit {
            Edit::Batch { edits, .. } if !edits.is_empty() && edits.len() <= MAX_MOVE_OBJECTS => {
                edits.iter().map(deleted_target).collect::<Option<Vec<_>>>()?
            }
            _ => vec![deleted_target(edit)?],
        };
        let page = deleted.first()?.0;
        let (mut text, mut image, mut added) = (None, None, None);
        let mut saw_added = false;
        for (p, target) in &deleted {
            if *p != page {
                return None;
            }
            // Added text participates in grouping before source blocks are filtered. Removing
            // it can renumber source indices, so source deletions must precede added items.
            if target.kind == ObjectKind::Added {
                saw_added = true;
            } else if saw_added {
                return None;
            }
            // Only descending, unique indices still refer to the original inventory after
            // preceding deletions. Never sort arbitrary sequential edits into a false capture.
            let previous = match target.kind {
                ObjectKind::Text => &mut text,
                ObjectKind::Image => &mut image,
                ObjectKind::Added => &mut added,
            };
            if previous.is_some_and(|index| target.index >= index) {
                return None;
            }
            *previous = Some(target.index);
        }
        let inventory = doc.editable_objects(page).ok()?;
        if deleted.iter().any(|(_, target)| !inventory.iter().any(|object| object.target == *target)) {
            return None;
        }
        let added = deleted.len() == 1 && deleted.first()?.1.kind == ObjectKind::Added && view.objects.count() == 0;
        Some(Self { page, targets: deleted.into_iter().map(|(_, target)| target).collect(), added })
    }

    /// Restore only while the editing surface is active and no uncommitted editor work would be displaced.
    /// The caller has checked that this page is current and navigation is settled.
    pub(super) fn restore(&self, view: &mut DocView, doc: &Document, tool: QuickTool, content_active: bool) {
        // Restoring a selection must never discard typing in another, uncommitted editor.
        if view.content.draft.is_some()
            || view.line_editor.as_ref().is_some_and(|editor| editor.has_changes())
            || view.pending_text_open.is_some()
            || view.pending_edit.is_some()
        {
            return;
        }
        if (self.added && content_active) || (!self.added && tool == QuickTool::EditText) {
            crate::object_ui::restore_deleted(view, doc, self.page, &self.targets, self.added);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PdfCraftApp;
    use pdfcraft_engine::{AddedText, BlockStyle};

    /// Build an original page with two separated source paragraphs and one added text item.
    fn app() -> PdfCraftApp {
        // Contributor-original text-only fixture, generated with a complete classic xref.
        let body = "BT /F1 18 Tf 40 430 Td (First) Tj ET BT /F1 18 Tf 40 90 Td (Last) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        let mut app = PdfCraftApp::new();
        app.open_bytes("history.pdf", None, bytes).unwrap();
        assert!(app.apply_edit(Edit::AddText {
            page: 0,
            text: AddedText { text: "Added".into(), rect: [100.0, 200.0, 180.0, 230.0], ..Default::default() },
        }));
        app
    }

    fn capture(app: &PdfCraftApp, edit: &Edit) -> Option<Selection> {
        let view = &app.views[0];
        Selection::capture(edit, view, app.session.get(view.id).unwrap())
    }

    fn delete(target: ObjectTarget) -> Edit {
        match target.kind {
            ObjectKind::Text => Edit::EditTextBlock { page: 0, block: target.index, text: String::new(), style: BlockStyle::default() },
            ObjectKind::Added => Edit::DeleteContent { page: 0, index: target.index },
            ObjectKind::Image => Edit::EditPageImage { page: 0, index: target.index, change: ImageEdit::Delete },
        }
    }

    /// Only original-state references may be captured, even when a batch's sequential edits would be valid.
    #[test]
    fn capture_rejects_sequential_batches_that_do_not_name_original_targets() {
        let app = app();
        let doc = app.session.get(app.views[0].id).unwrap();
        let inventory = doc.editable_objects(0).unwrap();
        let mut text = inventory.iter().filter(|object| object.target.kind == ObjectKind::Text).map(|object| object.target).collect::<Vec<_>>();
        text.sort_by_key(|target| target.index);
        assert_eq!(text.len(), 2);
        let (first, last) = (*text.first().unwrap(), *text.last().unwrap());
        let added = inventory.iter().find(|object| object.target.kind == ObjectKind::Added).unwrap().target;
        let batch = |edits| Edit::Batch { label: "Delete content".into(), edits };
        let canonical = batch(vec![delete(last), delete(first), delete(added)]);
        assert_eq!(capture(&app, &canonical).unwrap().targets, [last, first, added]);
        for (name, edit) in [
            ("ascending source indices", batch(vec![delete(first), delete(last)])),
            ("added before source", batch(vec![delete(added), delete(last)])),
            ("duplicate reference", batch(vec![delete(last), delete(last)])),
            ("nested batch", batch(vec![batch(vec![delete(last)])])),
            ("mixed pages", batch(vec![delete(first), Edit::DeleteContent { page: 1, index: 0 }])),
            ("oversized batch", batch(vec![delete(first); MAX_MOVE_OBJECTS + 1])),
            ("missing object", delete(ObjectTarget { kind: ObjectKind::Text, index: usize::MAX })),
            ("ordinary edit", Edit::EditTextBlock { page: 0, block: first.index, text: "Changed".into(), style: BlockStyle::default() }),
        ] {
            assert!(capture(&app, &edit).is_none(), "{name} must not be interpreted as original-state targets");
        }
    }

    #[test]
    fn abandoned_redo_records_are_pruned_by_live_history_identity() {
        let mut app = app();
        let id = app.views[0].id;
        let target = app
            .session
            .get(id)
            .unwrap()
            .editable_objects(0)
            .unwrap()
            .into_iter()
            .find(|object| object.target.kind == ObjectKind::Text)
            .unwrap()
            .target;
        assert!(app.apply_edit(delete(target)));
        let step = app.session.get(id).unwrap().next_undo_id().unwrap();
        assert_eq!(app.views[0].deletion_history.records.len(), 1);
        app.undo();
        assert_eq!(app.views[0].deletion_history.records.len(), 1, "a redoable deletion stays available");
        assert!(app.apply_edit(Edit::SetInfo { key: "Title".into(), value: "New branch".into() }));
        assert!(!app.session.get(id).unwrap().contains_history_step(&step));
        assert!(app.views[0].deletion_history.records.is_empty());
    }

    #[test]
    fn restoration_leaves_an_uncommitted_added_text_draft_active() {
        let mut app = app();
        let id = app.views[0].id;
        let doc = app.session.get(id).unwrap();
        let target = doc.editable_objects(0).unwrap().into_iter().find(|object| object.target.kind == ObjectKind::Text).unwrap().target;
        let selection = capture(&app, &delete(target)).unwrap();
        let view = &mut app.views[0];
        let draft = crate::content_ui::TextDraft {
            page: 0,
            index: None,
            rect: [20.0, 20.0, 120.0, 40.0],
            text: "Uncommitted work".into(),
            style: AddedText::default(),
            focus: true,
        };
        view.content.draft = Some(draft.clone());
        selection.restore(view, doc, QuickTool::EditText, true);
        assert_eq!(view.content.draft, Some(draft));
        assert_eq!(view.objects.count(), 0, "Undo must not switch the active typing context into a restored object selection");
    }
}
