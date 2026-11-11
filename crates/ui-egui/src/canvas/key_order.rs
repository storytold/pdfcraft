//! One pass gives the pages' keys to whoever holds them.
//!
//! Each page module read its keys for itself, wherever its code happened to run: some before
//! anything was drawn (`app/keys.rs`), some after the pages. So nothing said who got a key, and
//! the panels drawn in between read keys the pages had already acted on. This pass reads the
//! pages' keys for the active view once a pass, in `document_area`: after the panels have had
//! theirs, and before the pages are drawn, while last pass's page positions are still there for
//! the nudges.

use pdfcraft_engine::Document;

use super::DocView;
use crate::QuickTool;
use crate::page_keys::PageKeys;
use crate::scroll_speed::ScrollSpeed;

/// What the pass needs besides the view.
pub(crate) struct KeyCx<'a> {
    pub(crate) ctx: &'a egui::Context,
    pub(crate) keys: PageKeys,
    pub(crate) doc: &'a Document,
    /// The quick tool.
    pub(crate) tool: QuickTool,
    /// Prepare a form is open: fields are edited, not filled.
    pub(crate) preparing: bool,
    pub(crate) speed: ScrollSpeed,
}

/// The pages' keys for `view`, the active one.
pub(super) fn run(view: &mut DocView, cx: &KeyCx<'_>) {
    let KeyCx { ctx, keys, doc, tool, preparing, speed } = *cx;
    if tool == QuickTool::EditText && doc.allows_modification() {
        crate::object_ui::nudge_keys(ctx, keys, view, doc);
        crate::object_ui::delete_keys(ctx, keys, view);
    } else if doc.allows_annotation() {
        crate::comments::nudge_keys(ctx, keys, view, &doc.info);
    }
    // A selected field moves with the arrows too (Prepare a form, #2608), before the view turns
    // the page with them.
    if preparing && doc.allows_modification() {
        crate::prepare::nudge_keys(ctx, keys, view, doc);
    }
    super::navigation::view_plain_keys(view, ctx, keys, speed);
    // The picked image: Escape deselects it, or takes back a click whose picture isn't known yet.
    if !ctx.egui_wants_keyboard_input() && keys.pressed(ctx, egui::Key::Escape) {
        view.picked_image = None;
        view.pending_pick = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PdfCraftApp;

    /// Escape takes back a click whose picture isn't known yet: it isn't selected when it is.
    #[test]
    fn escape_cancels_a_click_on_a_page_still_being_read() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("notes.txt", None, b"hello".to_vec()).unwrap();
        let (i, id) = app.active_ids().unwrap();
        app.views[i].pending_pick = Some((0, [1.0, 1.0], 0));
        let escape = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let Some(doc) = app.session.get(id) else { panic!("the document is open") };
        let view = &mut app.views[i];
        egui::Context::default()
            .run_ui(egui::RawInput { events: vec![escape], ..Default::default() }, |ui| {
                let cx = KeyCx {
                    ctx: ui.ctx(),
                    keys: PageKeys::new(false),
                    doc,
                    tool: QuickTool::Select,
                    preparing: false,
                    speed: ScrollSpeed::default(),
                };
                run(view, &cx);
            })
            .drop_without_applying_deltas();
        assert_eq!(app.views[i].pending_pick, None);
    }
}
