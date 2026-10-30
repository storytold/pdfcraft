use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::{DocId, Edit, NewAnnotation, Shape, SignaturePlacementSize, Style};

use crate::PdfCraftApp;
use crate::fill_sign::{SavedSig, SignatureSizeRequest};

fn add_document(app: &mut PdfCraftApp) -> DocId {
    let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
    app.open_bytes("signature-request.pdf", None, bytes.as_ref().clone()).unwrap();
    let id = app.views[app.active.unwrap()].id;
    for signature in [true, true, false] {
        let shape = if signature {
            pdfcraft_engine::typed_signature_shape([30.0, 200.0], "Synthetic User", 32.0, 0).unwrap()
        } else {
            Shape::Rectangle { rect: [30.0, 180.0, 100.0, 220.0] }
        };
        app.session
            .apply(
                id,
                Edit::AddAnnotation(NewAnnotation {
                    page: 0,
                    style: Style::default_for(&shape),
                    shape,
                    contents: String::new(),
                    author: String::new(),
                }),
            )
            .unwrap();
    }
    app.views[app.active.unwrap()].comments.selected = Some((0, 0));
    id
}

fn app() -> (PdfCraftApp, DocId) {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.signature = Some(SavedSig::Typed("Synthetic User".into()));
    app.signature_placement_size = Some(SignaturePlacementSize::new(70.0, 16.0).unwrap());
    let id = add_document(&mut app);
    (app, id)
}

fn paint_menu(ui: &mut egui::Ui, app: &mut PdfCraftApp) {
    let signature = app.signature.as_ref();
    let initials = app.initials.as_ref();
    let view = &mut app.views[app.active.unwrap()];
    let doc = app.session.get(view.id).unwrap();
    crate::comments::signature_size_items(ui, view, doc, signature, initials);
}

fn cache_menu(ctx: &egui::Context, app: &mut PdfCraftApp) {
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| paint_menu(ui, app));
    output.textures_delta.clear();
}

#[derive(Debug, PartialEq)]
struct DocumentState {
    id: DocId,
    bytes: Vec<u8>,
    dirty: bool,
    undo: Option<String>,
    redo: Option<String>,
}

fn documents(app: &PdfCraftApp) -> Vec<DocumentState> {
    app.session
        .docs()
        .iter()
        .map(|doc| DocumentState {
            id: doc.id,
            bytes: app.session.save_bytes(doc.id).unwrap().as_ref().clone(),
            dirty: doc.dirty,
            undo: doc.can_undo().map(str::to_owned),
            redo: doc.can_redo().map(str::to_owned),
        })
        .collect()
}

#[test]
fn signature_size_request_rejects_stale_identity_and_removed_target_and_always_clears_pending() {
    for change in 0..5 {
        let (mut app, id) = app();
        let request = SignatureSizeRequest { doc: id, page: 0, annotation: 0, initials: false };
        let ctx = egui::Context::default();
        cache_menu(&ctx, &mut app);
        match change {
            0 => {
                add_document(&mut app);
            }
            1 => app.views[0].comments.selected = Some((0, 1)),
            2 => app.signature = None,
            3 => app.views[0].comments.selected = None,
            _ => app.active = None,
        }
        let before = documents(&app);
        let preferences = (app.signature_placement_size, app.initials_placement_size);
        assert!(!app.dispatch_signature_size_request(request), "stale request case {change} must be refused");
        assert_eq!((app.signature_placement_size, app.initials_placement_size), preferences);
        assert_eq!(documents(&app), before);
        assert!(app.pending_signature_size_request.is_none(), "early refusal must not leave a request that affects a later command");
    }
}

#[test]
fn signature_size_request_command_freshly_reads_resized_source_and_refuses_deleted_source_after_menu_cache() {
    let (mut app, id) = app();
    let ctx = egui::Context::default();
    cache_menu(&ctx, &mut app);
    app.session.apply(id, Edit::ResizeAnnotation { page: 0, index: 0, rect: [30.0, 180.0, 100.0, 197.0] }).unwrap();
    let size = app.session.get(id).unwrap().signature_placement_size(0, 0).unwrap().unwrap();
    let before = documents(&app);
    assert!(app.execute("sign.fill.signature.size"));
    assert_eq!(app.signature_placement_size, Some(size), "execution reads the current geometry rather than a cached placement size");
    assert_eq!(documents(&app), before);
    assert!(app.pending_signature_size_request.is_none());

    cache_menu(&ctx, &mut app);
    // Remove both suitable sources so the ordinary rectangle reuses the cached source index.
    for _ in 0..2 {
        app.session.apply(id, Edit::DeleteAnnotation { page: 0, index: 0 }).unwrap();
    }
    let before = documents(&app);
    assert!(!app.execute("sign.fill.signature.size"));
    assert_eq!(app.signature_placement_size, Some(size));
    assert_eq!(documents(&app), before);
    assert!(app.pending_signature_size_request.is_none());
}

#[test]
fn signature_size_menu_cache_actual_items_update_across_frames_source_changes_and_target_changes() {
    let (app, id) = app();
    let mut h = Harness::builder().with_size(egui::vec2(600.0, 300.0)).build_ui_state(paint_menu, app);
    h.run_steps(4);
    assert!(h.query_by_label("Use this size for saved signature").is_some());
    assert!(h.query_by_label("Use this size for saved initials").is_none());
    h.state_mut().views[0].comments.selected = Some((0, 2));
    h.run_steps(3);
    assert!(h.query_by_label("Use this size for saved signature").is_none(), "changing selection invalidates the previous positive result");
    h.state_mut().views[0].comments.selected = Some((0, 0));
    h.run_steps(3);
    assert!(h.query_by_label("Use this size for saved signature").is_some());
    for _ in 0..2 {
        h.state_mut().session.apply(id, Edit::DeleteAnnotation { page: 0, index: 0 }).unwrap();
    }
    h.run_steps(3);
    assert!(h.query_by_label("Use this size for saved signature").is_none(), "an ordinary source at the same index is not eligible");
    h.state_mut().session.undo(id).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("Use this size for saved signature").is_some());
    h.state_mut().signature = None;
    h.run_steps(3);
    assert!(h.query_by_label("Use this size for saved signature").is_none());
    h.state_mut().initials = Some(SavedSig::Typed("SU".into()));
    h.run_steps(3);
    h.get_by_label("Use this size for saved initials").click();
    h.run_steps(2);
    let request = h.state_mut().views[0].comments.signature_size_request.take().unwrap();
    assert_eq!(request, SignatureSizeRequest { doc: id, page: 0, annotation: 0, initials: true });
    let before = documents(h.state());
    assert!(h.state_mut().dispatch_signature_size_request(request));
    assert!(h.state().initials_placement_size.is_some());
    assert_eq!(documents(h.state()), before);
    assert!(h.state().pending_signature_size_request.is_none());
}

fn drawn_target() -> SavedSig {
    SavedSig::Drawn(vec![vec![[0.0, 0.5], [0.5, 0.5], [1.0, 0.0]]])
}

#[test]
fn signature_size_command_checks_the_actual_drawn_target_and_keeps_typed_initials_independent() {
    // The selected source is a typed placed appearance. Eligibility follows the saved target.
    let (mut app, id) = app();
    app.signature = Some(drawn_target());
    let command = pdfcraft_engine::commands::command("sign.fill.signature.size").unwrap();
    let valid_source_size = app.session.get(id).unwrap().signature_placement_size(0, 0).unwrap().unwrap();
    assert!(app.command_enabled(command), "the normal typed appearance size fits the saved drawn target");
    let before = documents(&app);
    assert!(app.execute("sign.fill.signature.size"));
    assert_eq!(app.signature_placement_size, Some(valid_source_size));
    assert_eq!(documents(&app), before);

    // Shrink the same selected typed source to a size that the drawn strokes cannot occupy.
    app.session.apply(id, Edit::ResizeAnnotation { page: 0, index: 0, rect: [30.0, 180.0, 32.0, 182.0] }).unwrap();
    let too_small = app.session.get(id).unwrap().signature_placement_size(0, 0).unwrap().unwrap();
    let before = documents(&app);
    let old_preferences = (app.signature_placement_size, app.initials_placement_size);
    assert!(!app.command_enabled(command));
    assert!(!app.execute("sign.fill.signature.size"));
    assert_eq!((app.signature_placement_size, app.initials_placement_size), old_preferences);
    assert_eq!(documents(&app), before);
    assert!(app.pending_signature_size_request.is_none());
    assert!(too_small.width() < valid_source_size.width());

    // A typed initials target remains eligible for this same small source and updates only its slot.
    app.initials = Some(SavedSig::Typed("SU".into()));
    let before = documents(&app);
    let old_signature_size = app.signature_placement_size;
    let request = SignatureSizeRequest { doc: id, page: 0, annotation: 0, initials: true };
    assert!(app.dispatch_signature_size_request(request));
    assert_eq!(app.initials_placement_size, Some(too_small));
    assert_eq!(app.signature_placement_size, old_signature_size);
    assert_eq!(documents(&app), before);
}

#[test]
fn signature_size_menu_checks_drawn_signature_and_typed_initials_independently() {
    let (mut app, id) = app();
    app.signature = Some(drawn_target());
    app.initials = Some(SavedSig::Typed("SU".into()));
    app.session.apply(id, Edit::ResizeAnnotation { page: 0, index: 0, rect: [30.0, 180.0, 32.0, 182.0] }).unwrap();

    let mut h = Harness::builder().with_size(egui::vec2(600.0, 300.0)).build_ui_state(paint_menu, app);
    h.run_steps(4);
    assert!(h.query_by_label("Use this size for saved signature").is_some());
    assert!(h.query_by_label("Use this size for saved initials").is_some());
    assert!(h.get_by_label("Use this size for saved signature").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Use this size for saved initials").accesskit_node().is_disabled());
    // The invalid signature action is disabled. Clicking it must not enqueue a request.
    h.get_by_label("Use this size for saved signature").click();
    h.run_steps(2);
    assert!(h.state().views[0].comments.signature_size_request.is_none());
    // The separate typed initials action remains available.
    h.get_by_label("Use this size for saved initials").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.signature_size_request, Some(SignatureSizeRequest { doc: id, page: 0, annotation: 0, initials: true }));
}
