//! Failed applies preserve history, while clear-only refusals and no-ops also preserve notices.

use super::*;
use crate::history::{Keys, MAX_UNDO, Scope};

#[derive(PartialEq)]
struct HistoryEntry {
    label: String,
    bytes: Vec<u8>,
    keys: Keys,
    scope: Scope,
}

impl std::fmt::Debug for HistoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryEntry")
            .field("label", &self.label)
            .field("byte_length", &self.bytes.len())
            .field("render_key_present", &self.keys.render.is_some())
            .field("reopen_key_present", &self.keys.reopen.is_some())
            .field("scope", &self.scope)
            .finish()
    }
}

fn history(entries: &[crate::history::Snapshot]) -> Vec<HistoryEntry> {
    entries
        .iter()
        .map(|(label, cos, keys, scope)| HistoryEntry {
            label: label.to_string(),
            bytes: pdfcraft_cos::write_incremental(cos, &Default::default()).unwrap(),
            keys: keys.clone(),
            scope: *scope,
        })
        .collect()
}

#[derive(PartialEq)]
struct State {
    bytes: Arc<Vec<u8>>,
    working: Vec<u8>,
    dirty: bool,
    generation: u64,
    notices: Vec<TextNotice>,
    keys: Keys,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let undo_labels: Vec<_> = self.undo.iter().map(|entry| entry.label.as_str()).collect();
        let redo_labels: Vec<_> = self.redo.iter().map(|entry| entry.label.as_str()).collect();
        f.debug_struct("State")
            .field("byte_length", &self.bytes.len())
            .field("working_byte_length", &self.working.len())
            .field("dirty", &self.dirty)
            .field("generation", &self.generation)
            .field("notice_count", &self.notices.len())
            .field("render_key_present", &self.keys.render.is_some())
            .field("reopen_key_present", &self.keys.reopen.is_some())
            .field("undo_labels", &undo_labels)
            .field("redo_labels", &redo_labels)
            .finish()
    }
}

fn state(session: &Session, id: DocId) -> State {
    let doc = session.get(id).unwrap();
    let editor = doc.editor.as_ref().unwrap();
    State {
        bytes: doc.bytes.clone(),
        working: pdfcraft_cos::write_incremental(&editor.cos, &Default::default()).unwrap(),
        dirty: doc.dirty,
        generation: doc.generation,
        notices: doc.edit_notices.clone(),
        keys: editor.keys.clone(),
        undo: history(&editor.undo),
        redo: history(&editor.redo),
    }
}

fn notice(session: &mut Session, id: DocId) {
    session.doc_mut(id).unwrap().edit_notices.push(TextNotice { field: None, page: 0, ch: '☺', redrawn_elsewhere: false });
}

#[test]
fn apply_refresh_failure_preserves_populated_redo_and_clears_legacy_notices() {
    let (mut session, id) = session_with(1);
    session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    session.undo(id).unwrap();
    notice(&mut session, id);
    let mut before = state(&session, id);
    assert_eq!(before.redo.len(), 1);
    before.notices.clear();
    panic_on_refresh(1);
    assert_eq!(
        session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Rejected title".into() }),
        Err(EditError::Reopen("injected refresh panic".into()))
    );
    assert_eq!(state(&session, id), before);
    assert!(session.get(id).unwrap().info.title.is_none());
    session.redo(id).unwrap();
    assert_eq!(session.get(id).unwrap().info.pages[0].rotation, 90, "the preserved redo is usable");
}

#[test]
fn apply_refresh_failure_does_not_evict_the_oldest_capped_undo() {
    let (mut session, id) = session_with(1);
    for index in 0..MAX_UNDO {
        session.apply(id, Edit::SetInfo { key: "Title".into(), value: format!("Title {index}") }).unwrap();
    }
    notice(&mut session, id);
    let mut before = state(&session, id);
    assert_eq!(before.undo.len(), MAX_UNDO);
    before.notices.clear();
    panic_on_refresh(1);
    assert!(matches!(session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Not committed".into() }), Err(EditError::Reopen(_))));
    assert_eq!(state(&session, id), before, "failed apply neither publishes nor evicts a history entry");
    for _ in 0..MAX_UNDO {
        session.undo(id).unwrap();
    }
    assert!(session.get(id).unwrap().info.title.is_none(), "even the oldest original state remains reachable");
}

#[test]
fn clear_comments_noop_preserves_clean_and_dirty_state_including_redo_and_notices() {
    for dirty in [false, true] {
        let (mut session, id) = session_with(1);
        if dirty {
            session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Keep".into() }).unwrap();
        }
        session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        session.undo(id).unwrap();
        notice(&mut session, id);
        for edit in [
            Edit::ClearComments { page: None },
            Edit::ClearComments { page: Some(0) },
            Edit::Batch {
                label: "Clear empty comments".into(),
                edits: vec![
                    Edit::ClearComments { page: None },
                    Edit::Batch { label: "Empty nested clear".into(), edits: vec![Edit::ClearComments { page: Some(0) }] },
                    Edit::Batch { label: "Empty child".into(), edits: vec![] },
                ],
            },
        ] {
            let before = state(&session, id);
            assert_eq!(before.dirty, dirty);
            session.apply(id, edit).unwrap();
            assert_eq!(state(&session, id), before);
            assert!(Arc::ptr_eq(&session.get(id).unwrap().bytes, &before.bytes), "no-op keeps the existing working bytes");
        }
        session.redo(id).unwrap();
        assert_eq!(session.get(id).unwrap().info.pages[0].rotation, 90);
    }
}

#[test]
fn flatten_fill_sign_noop_preserves_history_and_clears_legacy_notices() {
    let (mut session, id) = session_with(1);
    session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    session.undo(id).unwrap();
    notice(&mut session, id);
    let mut expected = state(&session, id);
    expected.notices.clear();
    session.apply(id, Edit::FlattenFillSign).unwrap();
    assert_eq!(state(&session, id), expected);
    assert!(Arc::ptr_eq(&session.get(id).unwrap().bytes, &expected.bytes), "the legacy no-op keeps its working bytes");
    session.redo(id).unwrap();
    assert_eq!(session.get(id).unwrap().info.pages[0].rotation, 90);
}

#[test]
fn empty_batches_keep_legacy_history_and_notice_behavior() {
    for nested in [false, true] {
        let (mut session, id) = session_with(1);
        session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        session.undo(id).unwrap();
        notice(&mut session, id);
        let before = state(&session, id);
        assert_eq!(before.redo.len(), 1);
        let edits = if nested { vec![Edit::Batch { label: "Empty child".into(), edits: vec![] }] } else { vec![] };
        session.apply(id, Edit::Batch { label: "Empty legacy batch".into(), edits }).unwrap();
        let after = state(&session, id);
        assert_eq!(after.working, before.working);
        assert_eq!(after.keys, before.keys);
        assert_eq!(after.generation, before.generation + 1);
        assert!(after.dirty && after.notices.is_empty() && after.redo.is_empty());
        assert_eq!(after.undo.len(), before.undo.len() + 1);
        assert_eq!(after.undo[..before.undo.len()], before.undo);
        assert_eq!(session.get(id).unwrap().can_undo(), Some("Empty legacy batch"));
    }
}

#[test]
fn clear_comments_noop_does_not_mask_invalid_page_read_only_or_permission_refusal() {
    let (mut session, id) = session_with(1);
    notice(&mut session, id);
    let before = state(&session, id);
    assert!(session.apply(id, Edit::ClearComments { page: Some(1) }).is_err());
    assert_eq!(state(&session, id), before);

    let denied = session.open("denied.pdf", None, protected("", "owner", !(1 << 5)), None).unwrap();
    notice(&mut session, denied);
    let before = state(&session, denied);
    assert_eq!(session.apply(denied, Edit::ClearComments { page: None }), Err(EditError::NotPermitted("comments")));
    assert_eq!(state(&session, denied), before);

    let before = state(&session, id);
    let editor = session.doc_mut(id).unwrap().editor.take();
    session.doc_mut(id).unwrap().read_only_reason = Some("test read-only document".into());
    assert!(matches!(session.apply(id, Edit::ClearComments { page: None }), Err(EditError::ReadOnly(_))));
    session.doc_mut(id).unwrap().editor = editor;
    assert_eq!(state(&session, id), before);
}

#[test]
fn clear_comments_batch_with_a_real_change_commits_exactly_once() {
    let (mut session, id) = session_with(1);
    let before = state(&session, id);
    session
        .apply(
            id,
            Edit::Batch {
                label: "Clear and rotate".into(),
                edits: vec![
                    Edit::ClearComments { page: None },
                    Edit::RotatePages { pages: vec![0], degrees: 90 },
                    Edit::ClearComments { page: Some(0) },
                ],
            },
        )
        .unwrap();
    let after = state(&session, id);
    assert_eq!(after.undo.len(), before.undo.len() + 1);
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(session.get(id).unwrap().info.pages[0].rotation, 90);
    session.undo(id).unwrap();
    assert_eq!(session.get(id).unwrap().bytes, before.bytes);
    assert_eq!(session.get(id).unwrap().info.pages[0].rotation, 0);
}

#[test]
fn clear_comments_full_refresh_failure_preserves_comments_and_redo() {
    let (mut session, id) = session_with(1);
    session.apply(id, rect_comment(0, [10.0, 10.0, 50.0, 50.0])).unwrap();
    session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    session.undo(id).unwrap();
    notice(&mut session, id);
    let before = state(&session, id);
    let listed = |doc: &Document| {
        doc.info
            .annotations
            .iter()
            .map(|annotation| (annotation.page, annotation.index, annotation.subtype.clone(), annotation.contents.clone(), annotation.rect))
            .collect::<Vec<_>>()
    };
    let comments = listed(session.get(id).unwrap());
    panic_on_refresh(1);
    assert!(matches!(session.apply(id, Edit::ClearComments { page: None }), Err(EditError::Reopen(_))));
    assert_eq!(state(&session, id), before);
    assert_eq!(listed(session.get(id).unwrap()), comments);
    session.redo(id).unwrap();
    assert_eq!(session.get(id).unwrap().info.pages[0].rotation, 90);
}

#[test]
fn clear_comments_does_not_rewrite_xfa_datasets_or_other_source_objects() {
    for batch in [false, true] {
        let template = pdfcraft_xfa::fixtures::fields_template("<form><page1><country>JP</country><langs>English</langs></page1></form>");
        let mut session = Session::new().with_clock(|| 1_700_000_000);
        let id = session.open("xfa.pdf", None, Arc::new(pdfcraft_xfa::fixtures::shell(&template)), None).unwrap();
        assert!(session.get(id).unwrap().info.xfa.is_some());
        session.apply(id, rect_comment(0, [10.0, 10.0, 50.0, 50.0])).unwrap();
        let cos = &session.get(id).unwrap().editor.as_ref().unwrap().cos;
        let pages = pdfcraft_annot::page_refs(cos).unwrap();
        let original: Vec<_> = cos.scan_objects().collect();
        let packets = pdfcraft_xfa::read_packets(cos).unwrap().unwrap().xdp;
        let mut page_before = cos.get(pages[0]).as_dict().unwrap().clone();
        page_before.remove(b"Annots");
        let clear = Edit::ClearComments { page: Some(0) };
        let edit = if batch { Edit::Batch { label: "Clear XFA comments".into(), edits: vec![clear] } } else { clear };
        session.apply(id, edit).unwrap();
        let doc = session.get(id).unwrap();
        let cos = &doc.editor.as_ref().unwrap().cos;
        assert_eq!(pdfcraft_xfa::read_packets(cos).unwrap().unwrap().xdp, packets);
        assert_eq!(cos.object_numbers().len(), original.len(), "an annotation-only edit adds no XFA datasets object");
        for (reference, object) in original {
            if reference != pages[0] {
                assert_eq!(cos.get(reference), object, "source object {reference:?} is outside the cleared page membership");
            }
        }
        let mut page_after = cos.get(pages[0]).as_dict().unwrap().clone();
        page_after.remove(b"Annots");
        assert_eq!(page_after, page_before);
        assert!(doc.info.xfa.is_some());
        assert!(doc.info.annotations.is_empty());
        session.undo(id).unwrap();
        assert_eq!(session.get(id).unwrap().info.annotations.len(), 1);
    }
}
