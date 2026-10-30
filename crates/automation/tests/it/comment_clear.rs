//! Contributor-original fixtures for scoped comment removal through the public tool table.

use super::{auto, fixture, json, ok, workdir};
use pdfcraft_cos::{Dict, Document, Object, SaveOptions, write_full};
use pdfcraft_engine::DocId;
use std::sync::Arc;

fn dict<const N: usize>(pairs: [(Vec<u8>, Object); N]) -> Dict {
    pairs.into_iter().collect()
}

fn source() -> Vec<u8> {
    let mut doc = Document::open(Arc::new(fixture(2))).unwrap();
    let pages = pdfcraft_model::pages(&doc);
    let note = doc.add(Object::Dict(dict([
        (b"Subtype".to_vec(), Object::name("Text")),
        (b"Rect".to_vec(), Object::Array(vec![10.into(), 10.into(), 30.into(), 30.into()])),
        (b"Contents".to_vec(), pdfcraft_cos::PdfString::text("First").into()),
    ])));
    let reply = doc.add(Object::Dict(dict([
        (b"Subtype".to_vec(), Object::name("Text")),
        (b"IRT".to_vec(), Object::Ref(note)),
        (b"Contents".to_vec(), pdfcraft_cos::PdfString::text("Reply").into()),
    ])));
    let popup = doc.add(Object::Dict(dict([(b"Subtype".to_vec(), Object::name("Popup")), (b"Parent".to_vec(), Object::Ref(note))])));
    let link = Object::Dict(dict([
        (b"Subtype".to_vec(), Object::name("Link")),
        (b"Rect".to_vec(), Object::Array(vec![40.into(), 10.into(), 90.into(), 30.into()])),
        (b"Dest".to_vec(), Object::Array(vec![Object::Ref(pages[1].obj), Object::name("Fit")])),
        // Misleading relationships on excluded types must not widen the removal graph.
        (b"IRT".to_vec(), Object::Ref(note)),
    ]));
    let widget = doc.add(Object::Dict(dict([
        (b"Subtype".to_vec(), Object::name("Widget")),
        (b"FT".to_vec(), Object::name("Btn")),
        (b"T".to_vec(), pdfcraft_cos::PdfString::text("Keep").into()),
        (b"Rect".to_vec(), Object::Array(vec![100.into(), 10.into(), 120.into(), 30.into()])),
        (b"V".to_vec(), Object::name("Off")),
    ])));
    doc.update_dict(pages[0].obj, |d| {
        d.set(b"Annots".to_vec(), Object::Array(vec![link, Object::Ref(reply), Object::Ref(popup), Object::Ref(widget), Object::Ref(note)]))
    })
    .unwrap();
    let second = doc.add(Object::Dict(dict([
        (b"Subtype".to_vec(), Object::name("Text")),
        (b"Rect".to_vec(), Object::Array(vec![10.into(), 10.into(), 30.into(), 30.into()])),
        (b"Contents".to_vec(), pdfcraft_cos::PdfString::text("Second").into()),
    ])));
    doc.update_dict(pages[1].obj, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(second)]))).unwrap();
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| {
        d.set(b"AcroForm".to_vec(), Object::Dict(dict([(b"Fields".to_vec(), Object::Array(vec![Object::Ref(widget)]))])));
        d.set(b"VendorComment".to_vec(), Object::Ref(note));
    })
    .unwrap();
    write_full(&doc, &SaveOptions::default()).unwrap()
}

fn working(a: &pdfcraft_automation::Automation, doc: u64) -> Vec<u8> {
    a.session().save_bytes(DocId(doc)).unwrap().as_ref().clone()
}

fn memberships(bytes: Vec<u8>) -> Vec<Vec<Object>> {
    let doc = Document::open(Arc::new(bytes)).unwrap();
    pdfcraft_model::pages(&doc)
        .iter()
        .map(|p| doc.get(p.obj).as_dict().unwrap().get(b"Annots").map(|o| doc.resolve(o).as_array().unwrap().clone()).unwrap_or_default())
        .collect()
}

#[test]
fn comment_clear_page_and_all_preserve_other_annotations_and_round_trip() {
    let dir = workdir("comment-clear-round-trip");
    let source = source();
    std::fs::write(dir.join("source.pdf"), &source).unwrap();
    let before = memberships(source.clone());
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"source.pdf"}))["doc"].as_u64().unwrap();
    let fields = ok(&mut a, "doc_info", json!({"doc":doc}))["fields"].clone();
    assert_eq!(fields.as_array().unwrap().len(), 1, "the fixture has one valid merged field/widget");
    let out = ok(&mut a, "comment_clear", json!({"doc":doc,"scope":"page","page":1}));
    assert_eq!(out["changed"], true);
    let page_only = working(&a, doc);
    assert!(page_only.starts_with(&source), "membership edits retain the original revision");
    let after = memberships(page_only.clone());
    assert_eq!(after[0], vec![before[0][0].clone(), before[0][3].clone()]);
    assert_eq!(after[1], before[1]);
    assert_eq!(ok(&mut a, "comment_list", json!({"doc":doc}))["count"], 1);
    assert_eq!(ok(&mut a, "doc_info", json!({"doc":doc}))["fields"], fields);
    // Removed membership does not free the object retained by vendor data.
    let old = Document::open(Arc::new(source.clone())).unwrap();
    let new = Document::open(Arc::new(page_only)).unwrap();
    let root = old.root().unwrap();
    let note = old.get(root).as_dict().unwrap().reference(b"VendorComment").unwrap();
    assert_eq!(*old.get(note), *new.get(note));
    ok(&mut a, "edit_undo", json!({"doc":doc}));
    assert_eq!(memberships(working(&a, doc)), before, "one Undo restores the whole page");
    ok(&mut a, "edit_redo", json!({"doc":doc}));
    ok(&mut a, "comment_clear", json!({"doc":doc,"scope":"all"}));
    assert_eq!(ok(&mut a, "comment_list", json!({"doc":doc}))["count"], 0);
    ok(&mut a, "edit_undo", json!({"doc":doc}));
    assert_eq!(memberships(working(&a, doc)), after, "one Undo restores the document scope");
    ok(&mut a, "edit_redo", json!({"doc":doc}));
    ok(&mut a, "doc_save", json!({"doc":doc,"path":"cleared.pdf"}));
    let reopened = ok(&mut a, "doc_open", json!({"path":"cleared.pdf"}))["doc"].as_u64().unwrap();
    assert_eq!(ok(&mut a, "comment_list", json!({"doc":reopened}))["count"], 0);
    assert_eq!(memberships(working(&a, reopened))[0].len(), 2);
    assert_eq!(std::fs::read(dir.join("source.pdf")).unwrap(), source);
}

#[test]
fn comment_clear_rejects_ambiguous_scopes_and_preserves_empty_history() {
    let dir = workdir("comment-clear-arguments");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_set_info", json!({"doc":doc,"key":"Title","value":"temporary"}));
    ok(&mut a, "edit_undo", json!({"doc":doc}));
    let before = working(&a, doc);
    let generation = a.session().get(DocId(doc)).unwrap().edit_generation();
    let history = ok(&mut a, "doc_list", json!({}));
    for mut args in [
        json!({}),
        json!({"scope":"page"}),
        json!({"scope":"ALL"}),
        json!({"scope":"page","page":0}),
        json!({"scope":"page","page":-1}),
        json!({"scope":"page","page":1.5}),
        json!({"scope":"page","page":4}),
        json!({"scope":"page","page":18446744073709551615u64}),
        json!({"scope":"all","page":1}),
        json!({"scope":"all","page":null}),
        json!({"scope":"all","pages":[1]}),
    ] {
        args["doc"] = json!(doc);
        assert!(a.call("comment_clear", &args).is_err(), "{args}");
        assert_eq!(working(&a, doc), before);
    }
    for args in [json!({"doc":doc,"scope":"page","page":1}), json!({"doc":doc,"scope":"all"})] {
        let out = ok(&mut a, "comment_clear", args);
        assert_eq!(out["changed"], false);
        assert_eq!(out["dirty"], false);
        assert_eq!(a.session().get(DocId(doc)).unwrap().edit_generation(), generation);
        assert_eq!(working(&a, doc), before);
        assert_eq!(ok(&mut a, "doc_list", json!({})), history, "no-op keeps Redo");
    }
    ok(&mut a, "edit_redo", json!({"doc":doc}));
    assert_eq!(ok(&mut a, "doc_info", json!({"doc":doc}))["title"], "temporary");
}

#[test]
fn comment_clear_refuses_page_tree_aliases_before_editing_a_different_page() {
    let dir = workdir("comment-clear-page-index");
    let mut source = Document::open(Arc::new(source())).unwrap();
    let pages = pdfcraft_model::pages(&source);
    let top = source.get(source.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let kids = source.add(Object::Array(vec![Object::Ref(pages[0].obj)]));
    let alias = source.add(Object::Ref(kids));
    let branch = source.add(Object::Dict(dict([
        (b"Type".to_vec(), Object::name("Pages")),
        (b"Parent".to_vec(), Object::Ref(top)),
        (b"Kids".to_vec(), Object::Ref(alias)),
        (b"Count".to_vec(), Object::Int(1)),
    ])));
    source.update_dict(pages[0].obj, |d| d.set(b"Parent".to_vec(), Object::Ref(branch))).unwrap();
    source.update_dict(top, |d| d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(branch), Object::Ref(pages[1].obj)]))).unwrap();
    assert_eq!(pdfcraft_model::pages(&source).iter().map(|p| p.obj).collect::<Vec<_>>(), vec![pages[1].obj]);
    let original = write_full(&source, &SaveOptions::default()).unwrap();
    std::fs::write(dir.join("aliased.pdf"), &original).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({"path":"aliased.pdf"}))["doc"].as_u64().unwrap();
    let before = working(&a, doc);
    let history = ok(&mut a, "doc_list", json!({}));
    let generation = a.session().get(DocId(doc)).unwrap().edit_generation();
    for args in [json!({"doc":doc,"scope":"page","page":1}), json!({"doc":doc,"scope":"all"})] {
        let error = a.call("comment_clear", &args).unwrap_err();
        assert!(error.to_string().contains("alias"), "{error}");
        assert_eq!(working(&a, doc), before);
        assert_eq!(a.session().get(DocId(doc)).unwrap().edit_generation(), generation);
        assert_eq!(ok(&mut a, "doc_list", json!({})), history);
    }
    assert_eq!(std::fs::read(dir.join("aliased.pdf")).unwrap(), original);
}
