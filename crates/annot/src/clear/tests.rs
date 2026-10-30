use super::{ClearCommentsReport, Limits, clear_comments, clear_with_limits};
use crate::{AnnotError, Dict, Document, ObjRef, Object};

fn annotation(subtype: &str) -> Dict {
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("Annot"));
    dict.set(b"Subtype".to_vec(), Object::name(subtype));
    dict.set(b"Unknown".to_vec(), Object::Int(91));
    dict
}

fn fixture(count: usize) -> (Document, Vec<ObjRef>) {
    let mut doc = Document::new_empty();
    let root = doc.root().unwrap();
    let tree = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
    let mut pages = Vec::new();
    for _ in 0..count {
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(tree));
        page.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 300.into(), 200.into()]));
        page.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
        page.set(b"Unknown".to_vec(), Object::Int(72));
        pages.push(doc.add(page));
    }
    doc.update_dict(tree, |d| {
        d.set(b"Kids".to_vec(), Object::Array(pages.iter().copied().map(Object::Ref).collect()));
        d.set(b"Count".to_vec(), Object::Int(count as i64));
    })
    .unwrap();
    (doc, pages)
}

fn set_list(doc: &mut Document, page: ObjRef, list: Vec<Object>) {
    doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(list))).unwrap();
}

fn list(doc: &Document, page: ObjRef) -> Vec<Object> {
    let page = doc.get(page);
    page.as_dict().unwrap().get(b"Annots").map(|a| doc.resolve(a).as_array().unwrap().clone()).unwrap_or_default()
}

fn refuses_without_writes(doc: &mut Document, page: Option<usize>) {
    let before = doc.read_state();
    assert!(clear_comments(doc, page).is_err());
    assert!(before.matches(doc));
}

#[test]
fn clear_uses_original_memberships_including_hidden_replies_states_and_groups() {
    let (mut doc, pages) = fixture(2);
    let first = doc.add(annotation("Text"));
    let mut reply = annotation("Text");
    reply.set(b"IRT".to_vec(), Object::Ref(first));
    let reply = doc.add(reply);
    let second = doc.add(annotation("Square"));
    let mut popup = annotation("Popup");
    popup.set(b"Parent".to_vec(), Object::Ref(first));
    let popup = doc.add(popup);
    let mut state = annotation("Text");
    state.set(b"State".to_vec(), Object::name("Accepted"));
    state.set(b"F".to_vec(), Object::Int(2));
    let mut group = annotation("Caret");
    group.set(b"IRT".to_vec(), Object::Ref(second));
    group.set(b"RT".to_vec(), Object::name("Group"));
    let mut widget = annotation("Widget");
    widget.set(b"Parent".to_vec(), Object::Ref(first));
    let widget = Object::Dict(widget);
    let link = Object::Dict(annotation("Link"));
    let page_two = Object::Dict(annotation("Stamp"));
    // A root's dependency precedes another root: sequential descending deletion can shift it.
    set_list(
        &mut doc,
        pages[0],
        vec![reply.into(), popup.into(), widget.clone(), first.into(), state.into(), second.into(), group.into(), link.clone()],
    );
    set_list(&mut doc, pages[1], vec![page_two.clone()]);
    let page_before = doc.get(pages[0]).as_dict().unwrap().clone();
    let payload = doc.get(first);
    let report = clear_comments(&mut doc, Some(0)).unwrap();
    assert_eq!(report, ClearCommentsReport { comments_removed: 5, popups_removed: 1, pages_changed: 1 });
    assert!(report.changed());
    assert_eq!(list(&doc, pages[0]), [widget, link]);
    assert_eq!(list(&doc, pages[1]), [page_two]);
    assert_eq!(doc.get(first), payload);
    let mut expected = page_before;
    expected.set(b"Annots".to_vec(), Object::Array(list(&doc, pages[0])));
    assert_eq!(doc.get(pages[0]).as_dict(), Some(&expected));
}

fn shared_array(scope: Option<usize>) {
    let (mut doc, pages) = fixture(2);
    let comment = doc.add(annotation("Text"));
    let original = vec![Object::Ref(comment), Object::Dict(annotation("Link"))];
    let array = doc.add(Object::Array(original.clone()));
    for page in &pages {
        doc.update_dict(*page, |d| d.set(b"Annots".to_vec(), Object::Ref(array))).unwrap();
    }
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"VendorList".to_vec(), Object::Ref(array))).unwrap();
    let array_before = doc.get(array);
    let payload_before = doc.get(comment);
    let second_before = doc.get(pages[1]);
    let report = clear_comments(&mut doc, scope).unwrap();
    assert_eq!(report.comments_removed, if scope.is_none() { 2 } else { 1 });
    assert_eq!(doc.get(array), array_before, "old array may still be referenced outside pages");
    assert_eq!(doc.get(comment), payload_before);
    assert_eq!(list(&doc, pages[0]), [Object::Dict(annotation("Link"))]);
    assert!(matches!(doc.get(pages[0]).as_dict().unwrap().get(b"Annots"), Some(Object::Array(_))));
    if scope.is_some() {
        assert_eq!(doc.get(pages[1]), second_before);
    } else {
        assert_eq!(list(&doc, pages[1]), [Object::Dict(annotation("Link"))]);
        assert!(matches!(doc.get(pages[1]).as_dict().unwrap().get(b"Annots"), Some(Object::Array(_))));
    }
    assert_eq!(doc.get(root).as_dict().unwrap().reference(b"VendorList"), Some(array));
}

#[test]
fn page_scope_detaches_shared_arrays_and_preserves_shared_annotation_payloads() {
    shared_array(Some(0));
}

#[test]
fn all_scope_also_preserves_arrays_referenced_outside_pages() {
    shared_array(None);
}

#[test]
fn aliases_and_inline_popups_use_canonical_comment_identity_and_count_memberships() {
    let (mut doc, pages) = fixture(1);
    let comment = doc.add(annotation("Text"));
    let middle = doc.add(Object::Ref(comment));
    let alias = doc.add(Object::Ref(middle));
    let mut popup = annotation("Popup");
    popup.set(b"Parent".to_vec(), Object::Ref(middle));
    let popup_ref = doc.add(popup.clone());
    let popup_alias = doc.add(Object::Ref(popup_ref));
    doc.update_dict(comment, |d| d.set(b"Popup".to_vec(), Object::Ref(popup_alias))).unwrap();
    set_list(&mut doc, pages[0], vec![alias.into(), comment.into(), popup_alias.into(), popup.into()]);
    assert_eq!(clear_comments(&mut doc, None).unwrap(), ClearCommentsReport { comments_removed: 2, popups_removed: 2, pages_changed: 1 });
    assert!(!doc.get(pages[0]).as_dict().unwrap().contains(b"Annots"));
}

#[test]
fn inline_comment_forward_popup_and_orphan_popup_are_distinguished() {
    let (mut doc, pages) = fixture(1);
    let popup = doc.add(annotation("Popup"));
    let mut comment = annotation("Text");
    comment.set(b"Popup".to_vec(), Object::Ref(popup));
    let orphan = Object::Dict(annotation("Popup"));
    set_list(&mut doc, pages[0], vec![orphan.clone(), popup.into(), comment.into()]);
    assert_eq!(clear_comments(&mut doc, None).unwrap(), ClearCommentsReport { comments_removed: 1, popups_removed: 1, pages_changed: 1 });
    assert_eq!(list(&doc, pages[0]), [orphan]);
}

#[test]
fn taxonomy_preserves_unclassified_and_excluded_entries_without_relationship_bridges() {
    let (mut doc, pages) = fixture(1);
    let comment = doc.add(annotation("Text"));
    let widget = doc.add(annotation("Widget"));
    doc.update_dict(comment, |d| d.set(b"Popup".to_vec(), Object::Ref(widget))).unwrap();
    let retained_types = ["Link", "Widget", "Screen", "Movie", "RichMedia", "3D", "PrinterMark", "TrapNet", "Watermark", "Popup", ""];
    let mut retained = Vec::new();
    for subtype in retained_types {
        let mut dict = annotation(subtype);
        if subtype != "Popup" {
            dict.set(b"Parent".to_vec(), Object::Ref(comment));
            dict.set(b"IRT".to_vec(), Object::Ref(comment));
            dict.set(b"Popup".to_vec(), Object::Ref(widget));
        }
        retained.push(Object::Dict(dict));
    }
    let mut no_subtype = Dict::new();
    no_subtype.set(b"Contents".to_vec(), Object::Int(17));
    retained.extend([Object::Dict(no_subtype), Object::Int(9), Object::Null]);
    let mut wrong_subtype = annotation("Text");
    wrong_subtype.set(b"Subtype".to_vec(), Object::Int(5));
    retained.push(Object::Dict(wrong_subtype));
    let mut original = retained.clone();
    original.extend([Object::Ref(comment), Object::Dict(annotation("VendorComment")), Object::Dict(annotation("Sound"))]);
    set_list(&mut doc, pages[0], original);
    assert_eq!(clear_comments(&mut doc, None).unwrap().comments_removed, 3);
    assert_eq!(list(&doc, pages[0]), retained);
}

#[test]
fn late_locked_comments_and_popups_refuse_all_pages_before_any_write() {
    for kind in ["Text", "Popup"] {
        for indirect in [false, true] {
            let (mut doc, pages) = fixture(2);
            let first = doc.add(annotation("Square"));
            set_list(&mut doc, pages[0], vec![first.into()]);
            let flags = doc.add(Object::Int(128));
            let flags_alias = doc.add(Object::Ref(flags));
            let mut locked = annotation(kind);
            locked.set(b"F".to_vec(), Object::Ref(flags_alias));
            locked.set(b"IRT".to_vec(), Object::Ref(first));
            if kind == "Popup" {
                locked.set(b"Parent".to_vec(), Object::Ref(first));
            }
            let locked = if indirect { Object::Ref(doc.add(locked)) } else { Object::Dict(locked) };
            set_list(&mut doc, pages[1], vec![locked]);
            refuses_without_writes(&mut doc, None);
        }
    }
}

#[test]
fn locks_outside_scope_or_on_retained_noncomments_do_not_block() {
    let (mut doc, pages) = fixture(2);
    let mut locked_comment = annotation("Text");
    locked_comment.set(b"F".to_vec(), Object::Int(128));
    set_list(&mut doc, pages[1], vec![locked_comment.into()]);
    let mut widget = annotation("Widget");
    widget.set(b"F".to_vec(), Object::Int(128));
    set_list(&mut doc, pages[0], vec![annotation("Text").into(), widget.clone().into()]);
    assert_eq!(clear_comments(&mut doc, Some(0)).unwrap().comments_removed, 1);
    assert_eq!(list(&doc, pages[0]), [Object::Dict(widget)]);
    assert_eq!(list(&doc, pages[1]).len(), 1);
}

#[test]
fn contradictory_popup_ownership_is_refused_and_cycles_between_comments_need_no_walk() {
    let (mut doc, pages) = fixture(1);
    let first = doc.add(annotation("Text"));
    let second = doc.add(annotation("Text"));
    let mut popup = annotation("Popup");
    popup.set(b"Parent".to_vec(), Object::Ref(second));
    let popup = doc.add(popup);
    doc.update_dict(first, |d| {
        d.set(b"Popup".to_vec(), Object::Ref(popup));
        d.set(b"IRT".to_vec(), Object::Ref(second));
    })
    .unwrap();
    doc.update_dict(second, |d| d.set(b"IRT".to_vec(), Object::Ref(first))).unwrap();
    set_list(&mut doc, pages[0], vec![first.into(), second.into(), popup.into()]);
    refuses_without_writes(&mut doc, None);
    doc.update_dict(popup, |d| d.set(b"Parent".to_vec(), Object::Ref(first))).unwrap();
    assert_eq!(clear_comments(&mut doc, None).unwrap().comments_removed, 2);
}

#[test]
fn malformed_membership_and_alias_cycles_never_clear_a_prefix() {
    for malformed in [Object::Int(3), Object::Dict(Dict::new()), Object::Null] {
        let (mut doc, pages) = fixture(2);
        set_list(&mut doc, pages[0], vec![annotation("Text").into()]);
        doc.update_dict(pages[1], |d| d.set(b"Annots".to_vec(), malformed)).unwrap();
        refuses_without_writes(&mut doc, None);
    }
    for in_membership in [false, true] {
        let (mut doc, pages) = fixture(2);
        set_list(&mut doc, pages[0], vec![annotation("Text").into()]);
        let first = doc.add(Object::Null);
        let second = doc.add(Object::Ref(first));
        doc.set(first, Object::Ref(second));
        let membership = if in_membership { Object::Array(vec![Object::Ref(first)]) } else { Object::Ref(first) };
        doc.update_dict(pages[1], |d| d.set(b"Annots".to_vec(), membership)).unwrap();
        refuses_without_writes(&mut doc, None);
    }
}

#[test]
fn checked_page_walk_refuses_cycles_invalid_children_and_incomplete_branches() {
    for broken in [Object::Array(vec![Object::Int(5)]), Object::Int(7), Object::Null] {
        let (mut doc, _) = fixture(1);
        let root = doc.root().unwrap();
        let tree = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
        doc.update_dict(tree, |d| d.set(b"Kids".to_vec(), broken)).unwrap();
        refuses_without_writes(&mut doc, None);
    }
    let (mut doc, pages) = fixture(1);
    set_list(&mut doc, pages[0], vec![annotation("Text").into()]);
    let root = doc.root().unwrap();
    let tree = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
    doc.update_dict(tree, |d| d.set(b"Kids".to_vec(), Object::Array(vec![pages[0].into(), tree.into()]))).unwrap();
    refuses_without_writes(&mut doc, None);
}

fn index_oracle_fixture() -> (Document, Vec<ObjRef>, ObjRef) {
    let (mut doc, pages) = fixture(2);
    for page in &pages {
        set_list(&mut doc, *page, vec![annotation("Text").into()]);
    }
    let root = doc.root().unwrap();
    let tree = doc.get(root).as_dict().unwrap().reference(b"Pages").unwrap();
    (doc, pages, tree)
}

fn assert_model_scope_refused(original: &Document, expected_pages: &[ObjRef]) {
    let model_pages: Vec<_> = pdfcraft_model::pages(original).into_iter().map(|page| page.obj).collect();
    assert_eq!(model_pages, expected_pages, "pin the model's actual page-index oracle before attempting deletion");
    for scope in [Some(0), None] {
        let mut doc = original.clone();
        let before = doc.read_state();
        let result = clear_comments(&mut doc, scope);
        assert!(result.is_err() && before.matches(&doc), "scope {scope:?} must refuse the ambiguous page mapping without writes: {result:?}");
        assert!(result.unwrap_err().to_string().contains("alias"), "the refusal explains the ambiguous structural alias");
    }
}

#[test]
fn page_index_oracle_refuses_aliased_page_nodes_without_writes() {
    let (mut doc, pages, tree) = index_oracle_fixture();
    let alias = doc.add(Object::Ref(pages[0]));
    doc.update_dict(tree, |d| d.set(b"Kids".to_vec(), Object::Array(vec![alias.into(), pages[1].into()]))).unwrap();
    // The model skips Ref -> Ref, so the visible first page is B, not hidden page A.
    assert_model_scope_refused(&doc, &[pages[1]]);
}

#[test]
fn page_index_oracle_refuses_aliased_kids_arrays_without_writes() {
    let (mut doc, pages, tree) = index_oracle_fixture();
    let real_kids = doc.add(Object::Array(vec![Object::Ref(pages[0])]));
    let alias_kids = doc.add(Object::Ref(real_kids));
    let mut branch = Dict::new();
    branch.set(b"Type".to_vec(), Object::name("Pages"));
    branch.set(b"Parent".to_vec(), Object::Ref(tree));
    branch.set(b"Kids".to_vec(), Object::Ref(alias_kids));
    branch.set(b"Count".to_vec(), Object::Int(1));
    let branch = doc.add(branch);
    doc.update_dict(pages[0], |d| d.set(b"Parent".to_vec(), Object::Ref(branch))).unwrap();
    doc.update_dict(tree, |d| d.set(b"Kids".to_vec(), Object::Array(vec![branch.into(), pages[1].into()]))).unwrap();
    // A valid sibling remains after the aliased branch the model cannot enumerate.
    assert_model_scope_refused(&doc, &[pages[1]]);
}

#[test]
fn page_index_oracle_refuses_aliased_catalog_without_writes() {
    let (mut doc, _, _) = index_oracle_fixture();
    let root = doc.root().unwrap();
    let alias = doc.add(Object::Ref(root));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(alias));
    // A catalog alias hides the entire tree from the model, unlike an ordinary sibling branch.
    assert_model_scope_refused(&doc, &[]);
}

#[test]
fn page_index_oracle_refuses_aliased_top_pages_without_writes() {
    let (mut doc, _, tree) = index_oracle_fixture();
    let alias = doc.add(Object::Ref(tree));
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"Pages".to_vec(), Object::Ref(alias))).unwrap();
    assert_model_scope_refused(&doc, &[]);
}

#[test]
fn ordinary_indirect_kids_arrays_keep_model_indices_and_can_be_cleared() {
    let (mut original, pages, tree) = index_oracle_fixture();
    let kids = original.add(Object::Array(pages.iter().copied().map(Object::Ref).collect()));
    original.update_dict(tree, |d| d.set(b"Kids".to_vec(), Object::Ref(kids))).unwrap();
    assert_eq!(pdfcraft_model::pages(&original).into_iter().map(|page| page.obj).collect::<Vec<_>>(), pages);
    let kids_before = original.get(kids);
    for scope in [Some(0), None] {
        let mut doc = original.clone();
        let report = clear_comments(&mut doc, scope).unwrap();
        let changed_pages = if scope.is_none() { 2 } else { 1 };
        assert_eq!(report, ClearCommentsReport { comments_removed: changed_pages, popups_removed: 0, pages_changed: changed_pages });
        assert!(list(&doc, pages[0]).is_empty());
        assert_eq!(list(&doc, pages[1]).len(), usize::from(scope.is_some()));
        assert_eq!(doc.get(kids), kids_before, "the ordinary indirect tree array is not rewritten");
    }
}

#[test]
fn reduced_work_and_membership_budgets_refuse_before_writes() {
    let (mut original, pages) = fixture(2);
    for page in pages {
        set_list(&mut original, page, vec![annotation("Text").into(), annotation("Square").into()]);
    }
    let mut failed = 0;
    let mut succeeded = 0;
    for work in 0..150 {
        let mut doc = original.clone();
        let before = doc.read_state();
        match clear_with_limits(&mut doc, None, Limits { work, ..Limits::default() }) {
            Ok(report) => {
                assert_eq!(report.comments_removed, 4);
                succeeded += 1;
            }
            Err(_) => {
                assert!(before.matches(&doc), "budget {work} must not clear a prefix");
                failed += 1;
            }
        }
    }
    assert!(failed > 0 && succeeded > 0);
    for entries in [0, 1, 2, 3] {
        let mut doc = original.clone();
        let before = doc.read_state();
        assert!(clear_with_limits(&mut doc, None, Limits { entries, ..Limits::default() }).is_err());
        assert!(before.matches(&doc));
    }
}

#[test]
fn a_cached_alias_suffix_does_not_bypass_the_chain_limit() {
    let (mut doc, pages) = fixture(1);
    let comment = doc.add(annotation("Text"));
    let middle = doc.add(Object::Ref(comment));
    let outer = doc.add(Object::Ref(middle));
    // Reading the terminal first warms the resolver's memo before the longer chain is visited.
    set_list(&mut doc, pages[0], vec![comment.into(), outer.into()]);
    let before = doc.read_state();
    assert!(clear_with_limits(&mut doc, None, Limits { alias_depth: 2, ..Limits::default() }).is_err());
    assert!(before.matches(&doc));
}

#[test]
fn empty_and_repeated_clears_are_exact_noops_and_page_indices_are_strict() {
    let (mut doc, pages) = fixture(1);
    let before = doc.read_state();
    assert_eq!(clear_comments(&mut doc, None).unwrap(), ClearCommentsReport::default());
    assert!(before.matches(&doc));
    assert_eq!(clear_comments(&mut doc, Some(1)), Err(AnnotError::NoSuchPage(1)));
    assert!(before.matches(&doc));
    set_list(&mut doc, pages[0], vec![annotation("Text").into()]);
    clear_comments(&mut doc, None).unwrap();
    let already_dirty = doc.read_state();
    assert!(!clear_comments(&mut doc, Some(0)).unwrap().changed());
    assert!(already_dirty.matches(&doc));
    let (mut empty, _) = fixture(0);
    assert_eq!(clear_comments(&mut empty, Some(0)), Err(AnnotError::NoSuchPage(0)));
    assert_eq!(clear_comments(&mut empty, None).unwrap(), ClearCommentsReport::default());
}
