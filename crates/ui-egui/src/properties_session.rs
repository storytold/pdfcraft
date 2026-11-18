//! Document Properties keeps its widget buffers local and commits one revision-bound draft.

use edit_session::EditSession;
use pdfcraft_engine::{DocId, Document, Edit, InitialView};

use crate::{CustomDraft, PdfCraftApp};

pub(crate) const INFO_KEYS: [&str; 4] = ["Title", "Author", "Subject", "Keywords"];

/// The small, accepted values shared by the Description, Initial View and Custom tabs.
#[derive(Clone, Debug)]
pub(crate) struct Values {
    description: [String; 4],
    view: InitialView,
    custom: Vec<(String, String)>,
}

impl PartialEq for Values {
    /// Compare accepted values using the dialog's whitespace and unordered Custom-row rules.
    fn eq(&self, other: &Self) -> bool {
        // Preserve the dialog's existing whitespace/no-op rules, without stripping spaces
        // from the text widgets while the user types. Custom row order is not a PDF edit.
        self.description.iter().zip(&other.description).all(|(a, b)| a.trim() == b.trim())
            && self.view == other.view
            && self.custom.len() == other.custom.len()
            && self.custom.iter().all(|entry| other.custom.contains(entry))
    }
}

impl Values {
    /// Snapshot the editable values; PDF bytes and undo history remain owned by the engine.
    fn read(doc: &Document) -> Self {
        Self { description: INFO_KEYS.map(|key| doc.info_value(key).unwrap_or_default()), view: doc.initial_view(), custom: doc.custom_info() }
    }

    /// Translate an accepted draft into existing engine edits, preserving unrelated PDF metadata.
    fn edits_to(&self, after: &Self) -> Vec<Edit> {
        let mut edits: Vec<Edit> = INFO_KEYS
            .iter()
            .zip(self.description.iter().zip(&after.description))
            .filter(|(_, (before, after))| before.trim() != after.trim())
            .map(|(key, (_, value))| Edit::SetInfo { key: (*key).into(), value: value.clone() })
            .collect();
        if self.view != after.view {
            edits.push(Edit::SetInitialView(Box::new(after.view.clone())));
        }
        for (key, _) in self.custom.iter().filter(|(key, _)| !after.custom.iter().any(|(name, _)| name == key)) {
            edits.push(Edit::SetInfo { key: key.clone(), value: String::new() });
        }
        for (key, value) in after.custom.iter().filter(|entry| !self.custom.contains(entry)) {
            edits.push(Edit::SetInfo { key: key.clone(), value: value.clone() });
        }
        edits
    }
}

/// Changes to permissions, Undo/Redo and in-place reloads invalidate the original draft too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    document: DocId,
    generation: u64,
    editable: bool,
}

impl Context {
    /// Capture the identity, monotonic generation and permission that bind the draft.
    fn read(doc: &Document) -> Self {
        Self { document: doc.id, generation: doc.edit_generation(), editable: doc.allows_modification() }
    }
}

pub(crate) type Draft = EditSession<Values, Context>;

impl PdfCraftApp {
    /// Discard local form buffers and their session, without issuing an engine command.
    pub(crate) fn discard_properties(&mut self) {
        self.props_session = None;
        self.props_draft = None;
        self.view_draft = None;
        self.custom_draft = None;
    }

    /// Bind edits to one document revision. Clean inspection tabs may follow same-document
    /// edits, preserving the existing live Fonts/Space Audit behavior without rebasing input.
    pub(crate) fn prepare_properties(&mut self) -> bool {
        self.stage_properties();
        let live = self.active_ids().and_then(|(_, id)| self.session.get(id)).map(|doc| (Values::read(doc), Context::read(doc)));
        let mut stale = match (&self.props_session, &live) {
            (Some(draft), Some((value, context))) => draft.check_live(value, context).is_err(),
            (Some(_), None) => true,
            (None, _) => false,
        };
        let clean_inspector = matches!(
            self.dialog,
            Some(crate::Dialog::Properties(
                crate::PropsTab::Fonts | crate::PropsTab::Advanced | crate::PropsTab::SpaceAudit | crate::PropsTab::Security
            ))
        ) && self.props_session.as_ref().is_some_and(|draft| {
            draft.is_valid()
                && draft.original() == draft.preview()
                && live.as_ref().is_some_and(|(_, context)| context.document == draft.context().document)
        }) && self.custom_draft.as_ref().is_some_and(|(_, custom)| custom.name.is_empty() && custom.value.is_empty());
        if stale && clean_inspector {
            self.discard_properties();
            stale = false;
        }
        if stale || live.is_none() {
            self.discard_properties();
            self.dialog = None;
            if stale {
                self.notify(tl!("The document changed. Reopen Document Properties to edit the current version.").to_string());
            }
            return false;
        }
        if self.props_session.is_none()
            && let Some((values, context)) = live
        {
            self.props_draft = Some((context.document, values.description.clone()));
            self.view_draft = Some((context.document, values.view.clone()));
            self.custom_draft = Some((context.document, CustomDraft { props: values.custom.clone(), ..Default::default() }));
            self.props_session = Some(EditSession::new(values, context));
        }
        true
    }

    /// Stage widget values after their existing per-control validation. The Custom tab's
    /// Name/Value input boxes are not accepted properties until Add or Update is pressed.
    pub(crate) fn stage_properties(&mut self) {
        let expected = self.props_session.as_ref().map(|draft| draft.context().document);
        let proposal = match (&self.props_draft, &self.view_draft, &self.custom_draft) {
            (Some((id, description)), Some((vid, view)), Some((cid, custom))) if Some(*id) == expected && id == vid && id == cid => {
                Ok(Values { description: description.clone(), view: view.clone(), custom: custom.props.clone() })
            }
            _ => Err(()),
        };
        if let Some(draft) = &mut self.props_session {
            let _ = draft.propose(proposal);
        }
    }

    /// Whether a valid proposal needs OK/Cancel instead of the unchanged dialog's Close button.
    pub(crate) fn properties_changed(&self) -> bool {
        self.props_session.as_ref().is_some_and(|draft| draft.is_valid() && draft.original() != draft.preview())
    }

    /// Check and consume once on the serialized UI command path. An engine failure retains
    /// the widget values and restores the uncommitted session, so the user can correct them.
    pub(crate) fn finish_properties(&mut self) -> bool {
        if !self.prepare_properties() {
            return true;
        }
        self.stage_properties();
        let Some((live, context)) = self.active_ids().and_then(|(_, id)| self.session.get(id)).map(|doc| (Values::read(doc), Context::read(doc)))
        else {
            return false;
        };
        if self.props_session.as_ref().is_none_or(|draft| draft.check(&live, &context).is_err()) {
            return false;
        }
        let Some(draft) = self.props_session.take() else { return false };
        match draft.finish(&live, &context) {
            Ok(Some(change)) => {
                let edits = change.before.edits_to(&change.after);
                if self.apply_edit(Edit::Batch { label: "Change document properties".into(), edits }) {
                    true
                } else {
                    // Session::apply is atomic on error; do not rebase to any newer document.
                    let mut retry = EditSession::new(change.before, context);
                    let _ = retry.propose::<()>(Ok(change.after));
                    self.props_session = Some(retry);
                    false
                }
            }
            Ok(None) => true,
            Err(_) => {
                self.discard_properties();
                self.notify(tl!("The document changed. Reopen Document Properties to edit the current version.").to_string());
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a one-page document with a pending title edit for direct commit-path regressions.
    fn app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes("properties.pdf", None, bytes.as_ref().clone()).unwrap();
        app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Description));
        assert!(app.prepare_properties());
        app.props_draft.as_mut().unwrap().1[0] = "Draft title".into();
        app
    }

    #[test]
    /// A concurrent edit must be rejected at commit even before another UI frame checks freshness.
    fn commit_rechecks_revision_without_an_intervening_frame() {
        let mut app = app();
        let (_, id) = app.active_ids().unwrap();
        app.session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Newer title".into() }).unwrap();
        let generation = app.session.get(id).unwrap().edit_generation();
        assert!(app.finish_properties());
        let doc = app.session.get(id).unwrap();
        assert_eq!(doc.info_value("Title").as_deref(), Some("Newer title"));
        assert_eq!(doc.edit_generation(), generation);
        assert!(app.props_session.is_none());
        assert!(app.props_draft.is_none());
    }

    #[test]
    /// An invalid view must roll back the title too, retain input, and permit one corrected undo step.
    fn engine_refusal_retains_draft_and_retry_is_one_atomic_edit() {
        let mut app = app();
        let (_, id) = app.active_ids().unwrap();
        let original_bytes = app.session.get(id).unwrap().bytes.clone();
        let generation = app.session.get(id).unwrap().edit_generation();
        // The public control buffers can receive an invalid page even though the widget
        // bounds its input. The engine rejects the entire title + view batch atomically.
        app.view_draft.as_mut().unwrap().1.page = usize::MAX;
        assert!(!app.finish_properties());
        let doc = app.session.get(id).unwrap();
        assert_eq!(doc.bytes, original_bytes);
        assert_eq!(doc.info_value("Title"), None);
        assert_eq!(doc.edit_generation(), generation);
        assert!(!doc.dirty && doc.can_undo().is_none());
        assert_eq!(app.props_draft.as_ref().unwrap().1[0], "Draft title");
        assert!(app.props_session.is_some());

        app.view_draft.as_mut().unwrap().1.page = 0;
        assert!(app.finish_properties());
        assert_eq!(app.session.get(id).unwrap().info_value("Title").as_deref(), Some("Draft title"));
        assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Change document properties"));
        app.session.undo(id).unwrap();
        assert_eq!(app.session.get(id).unwrap().info_value("Title"), None);
        assert!(app.session.get(id).unwrap().can_undo().is_none());
    }

    #[test]
    /// Matching form-buffer identities must also belong to the session's original document.
    fn buffers_from_another_document_cannot_commit() {
        let mut app = app();
        let (_, id) = app.active_ids().unwrap();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        let other = app.session.open("other.pdf", None, bytes, None).unwrap();
        app.props_draft.as_mut().unwrap().0 = other;
        app.view_draft.as_mut().unwrap().0 = other;
        app.custom_draft.as_mut().unwrap().0 = other;
        assert!(!app.finish_properties());
        for doc_id in [id, other] {
            let doc = app.session.get(doc_id).unwrap();
            assert_eq!(doc.info_value("Title"), None);
            assert!(!doc.dirty && doc.can_undo().is_none());
        }
    }
}
