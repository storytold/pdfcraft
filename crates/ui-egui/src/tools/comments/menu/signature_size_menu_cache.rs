use std::cell::Cell;

use pdfcraft_engine::{DocId, Edit, NewAnnotation, Session, Shape, Style};

use super::{SignatureSizeMenuKey, signature_size_menu_source};

fn annotation(page: usize, signature: bool) -> Edit {
    let shape = if signature {
        pdfcraft_engine::typed_signature_shape([30.0, 200.0], "Synthetic User", 32.0, 0).unwrap()
    } else {
        Shape::Rectangle { rect: [30.0, 180.0, 100.0, 220.0] }
    };
    Edit::AddAnnotation(NewAnnotation { page, style: Style::default_for(&shape), shape, contents: String::new(), author: "Synthetic User".into() })
}

fn document(session: &mut Session, signature: bool) -> DocId {
    let bytes = session.create_blank(300.0, 400.0, 2).unwrap();
    let id = session.open_new("signature-menu.pdf", bytes).unwrap();
    for page in 0..2 {
        session.apply(id, annotation(page, signature)).unwrap();
        session.apply(id, annotation(page, !signature)).unwrap();
    }
    id
}

fn eligibility(ctx: &egui::Context, session: &Session, id: DocId, page: usize, annotation: usize, has_target: bool, queries: &Cell<usize>) -> bool {
    let doc = session.get(id).unwrap();
    let key = SignatureSizeMenuKey::new(doc, page, annotation);
    let mut eligible = false;
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
        eligible = signature_size_menu_source(ctx, key, has_target, || {
            queries.set(queries.get() + 1);
            doc.signature_placement_size(page, annotation).ok().flatten()
        })
        .is_some();
    });
    output.textures_delta.clear();
    eligible
}

#[test]
fn signature_size_menu_cache_skips_queries_without_saved_targets() {
    let mut session = Session::new();
    let id = document(&mut session, false);
    let ctx = egui::Context::default();
    let queries = Cell::new(0);
    for _ in 0..4 {
        assert!(!eligibility(&ctx, &session, id, 0, 0, false, &queries));
    }
    assert_eq!(queries.get(), 0, "an ordinary selected comment needs no signature query without saved targets");
    assert!(eligibility(&ctx, &session, id, 0, 1, true, &queries), "adding a saved target immediately permits a suitable source");
    assert_eq!(queries.get(), 1);
}

#[test]
fn signature_size_menu_cache_queries_unchanged_positive_negative_and_error_sources_once() {
    let mut session = Session::new();
    let id = document(&mut session, true);
    for (annotation, expected) in [(0, true), (1, false), (99, false)] {
        let ctx = egui::Context::default();
        let queries = Cell::new(0);
        for _ in 0..4 {
            assert_eq!(eligibility(&ctx, &session, id, 0, annotation, true, &queries), expected);
        }
        assert_eq!(queries.get(), 1, "source {annotation}: unchanged open-menu frames must reuse even a refusal");
    }
}

#[test]
fn signature_size_menu_cache_keys_document_page_and_annotation_and_replaces_one_slot() {
    let mut session = Session::new();
    let signature = document(&mut session, true);
    let ordinary = document(&mut session, false);
    assert_eq!(session.get(signature).unwrap().edit_generation(), session.get(ordinary).unwrap().edit_generation());
    let ctx = egui::Context::default();
    let queries = Cell::new(0);
    let sources = [(signature, 0, 0, true), (ordinary, 0, 0, false), (ordinary, 1, 0, false), (ordinary, 1, 1, true), (signature, 0, 0, true)];
    for (step, (doc, page, annotation, expected)) in sources.into_iter().enumerate() {
        for _ in 0..2 {
            assert_eq!(eligibility(&ctx, &session, doc, page, annotation, true, &queries), expected);
        }
        assert_eq!(queries.get(), step + 1, "replace the one slot on identity change, including when returning to an earlier key");
    }
}

#[test]
fn signature_size_menu_cache_invalidates_after_edit_undo_redo_and_index_reuse() {
    let mut session = Session::new();
    let id = document(&mut session, true);
    let ctx = egui::Context::default();
    let queries = Cell::new(0);
    for _ in 0..2 {
        assert!(eligibility(&ctx, &session, id, 0, 0, true, &queries));
    }
    assert_eq!(queries.get(), 1);
    session.apply(id, Edit::DeleteAnnotation { page: 0, index: 0 }).unwrap();
    for _ in 0..2 {
        assert!(!eligibility(&ctx, &session, id, 0, 0, true, &queries), "the ordinary annotation now occupies the removed signature's index");
    }
    assert_eq!(queries.get(), 2);
    session.undo(id).unwrap();
    for _ in 0..2 {
        assert!(eligibility(&ctx, &session, id, 0, 0, true, &queries));
    }
    assert_eq!(queries.get(), 3);
    session.redo(id).unwrap();
    for _ in 0..2 {
        assert!(!eligibility(&ctx, &session, id, 0, 0, true, &queries));
    }
    assert_eq!(queries.get(), 4);
}

#[test]
fn signature_size_menu_cache_invalidates_same_document_reload() {
    let mut session = Session::new();
    let id = document(&mut session, true);
    let replacement = document(&mut session, false);
    let bytes = session.save_bytes(replacement).unwrap();
    let ctx = egui::Context::default();
    let queries = Cell::new(0);
    for _ in 0..2 {
        assert!(eligibility(&ctx, &session, id, 0, 0, true, &queries));
    }
    assert_eq!(queries.get(), 1);
    let generation = session.get(id).unwrap().edit_generation();
    session.reload(id, bytes).unwrap();
    assert_eq!(session.get(id).unwrap().id, id);
    assert!(session.get(id).unwrap().edit_generation() > generation);
    for _ in 0..2 {
        assert!(!eligibility(&ctx, &session, id, 0, 0, true, &queries));
    }
    assert_eq!(queries.get(), 2, "reload must invalidate eligibility even while retaining the tab's document id");
}
