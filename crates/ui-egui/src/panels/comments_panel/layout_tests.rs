//! Cached layout must agree with a fresh layout after document and UI changes.

use super::*;
use pdfcraft_engine::{DocId, Session};

/// One page with 120 independent comments, large enough to use the virtual list.
fn document(session: &mut Session) -> DocId {
    let refs: Vec<_> = (4..124).map(|i| format!("{i} 0 R")).collect();
    let mut bytes = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [{}] >> endobj\n",
        refs.join(" ")
    );
    for i in 4..124 {
        bytes.push_str(&format!("{i} 0 obj << /Type /Annot /Subtype /Text /Rect [20 20 40 40] /T (Ada) /Contents (Card {i}) >> endobj\n"));
    }
    bytes.push_str("trailer << /Root 1 0 R >>\n%%EOF\n");
    session.open_new("comments.pdf", std::sync::Arc::new(bytes.into_bytes())).unwrap()
}

/// Render the real panel with a short viewport so later cards are outside its clip rectangle.
fn height(ctx: &egui::Context, doc: &pdfcraft_engine::Document, view: &mut DocView, width: f32) -> f32 {
    let mut height = 0.0;
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 100.0))), ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| {
        let top = ui.cursor().min.y;
        show(ui, &Tokens::get(ctx), doc, view, &CommentPrefs::default(), true, &mut None);
        height = ui.cursor().min.y - top;
    });
    output.textures_delta.clear();
    height
}

/// A fresh context with the bundled interface fonts.
fn context() -> egui::Context {
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    ctx
}

/// Compare against the same document rendered with no prior height entries.
fn assert_fresh_height(ctx: &egui::Context, doc: &pdfcraft_engine::Document, view: &mut DocView, width: f32) {
    let fresh = context();
    let expected = height(&fresh, doc, view, width);
    let actual = height(ctx, doc, view, width);
    assert!((actual - expected).abs() < 0.5, "cached height {actual}, fresh height {expected}");
}

/// Replies change an off-screen card's height even when no card is selected. Undo and redo
/// must invalidate that measurement too, before a later card is revealed.
#[test]
fn offscreen_reply_edit_undo_redo_remeasure_the_list() {
    let mut session = Session::default();
    let id = document(&mut session);
    let doc = session.get(id).unwrap();
    let mut view = DocView::new(id, &doc.info, Default::default());
    let ctx = context();
    let original = height(&ctx, doc, &mut view, 300.0);
    assert_fresh_height(&ctx, doc, &mut view, 300.0);
    session
        .apply(id, Edit::ReplyToAnnotation { page: 0, index: 119, text: "A reply with multiple lines. ".repeat(20), author: "Bea".into() })
        .unwrap();
    let doc = session.get(id).unwrap();
    let expanded = height(&context(), doc, &mut view, 300.0);
    assert!(expanded > original + 100.0, "fixture must change the off-screen card's height");
    assert_fresh_height(&ctx, doc, &mut view, 300.0);
    session.undo(id).unwrap();
    assert_fresh_height(&ctx, session.get(id).unwrap(), &mut view, 300.0);
    session.redo(id).unwrap();
    assert_fresh_height(&ctx, session.get(id).unwrap(), &mut view, 300.0);
}

/// Two documents can have identical page/index pairs at the same edit generation and width.
#[test]
fn switching_documents_does_not_reuse_another_documents_heights() {
    let mut session = Session::default();
    let first = document(&mut session);
    let second = document(&mut session);
    for (id, text) in [(first, "short".into()), (second, "A much longer reply. ".repeat(30))] {
        session.apply(id, Edit::ReplyToAnnotation { page: 0, index: 119, text, author: "Bea".into() }).unwrap();
    }
    assert_eq!(session.get(first).unwrap().edit_generation(), session.get(second).unwrap().edit_generation());
    let ctx = context();
    for id in [first, second, first] {
        let doc = session.get(id).unwrap();
        let mut view = DocView::new(id, &doc.info, Default::default());
        assert_fresh_height(&ctx, doc, &mut view, 300.0);
    }
}

/// Selection opens the reply composer and expands the body; deselecting an off-screen card
/// must restore its compact height. Width changes must also remeasure wrapped replies.
#[test]
fn deselection_and_resize_remeasure_hidden_cards() {
    let mut session = Session::default();
    let id = document(&mut session);
    session.apply(id, Edit::ReplyToAnnotation { page: 0, index: 119, text: "Wrapped reply. ".repeat(30), author: "Bea".into() }).unwrap();
    let doc = session.get(id).unwrap();
    let mut view = DocView::new(id, &doc.info, Default::default());
    let ctx = context();
    view.comments.selected = Some((0, 119));
    height(&ctx, doc, &mut view, 300.0);
    view.comments.selected = None;
    assert_fresh_height(&ctx, doc, &mut view, 300.0);
    assert_fresh_height(&ctx, doc, &mut view, 210.0);
}
