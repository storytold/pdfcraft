//! The Edit text tool on the canvas background: an area selection of the page's objects, and the keys of
//! what is selected (arrows move it, Delete removes it, Escape lets go).

use craft::behavior::press::PressState;
use craft::behavior::{CancelReason, Cursor, CursorIcon};
use craft::pages::ui::PlacedPage;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolClaims, ToolContext, ToolKey, ToolPath, ToolPointer};
use craft::ui::primitives::{Draw, PaintShape, StrokeWidth};
use pdfcraft_engine::Document;
use pdfcraft_render::DocInfo;

use crate::canvas::DocView;
use crate::object_ui;

/// Edit text's tool of the canvas: what the families' parts leave of a press, and the keys.
pub(crate) struct EditTextTool<'a> {
    view: &'a mut DocView,
    doc: &'a Document,
    info: &'a DocInfo,
    /// The pages shown this pass.
    placed: &'a [PlacedPage],
    /// The canvas background's press of this pass: the area selection's span is read from it.
    press: &'a PressState,
}

impl<'a> EditTextTool<'a> {
    /// The tool over `view`'s document `doc`, built each pass with the pages shown and the background press of
    /// the pass; the tool keeps nothing between passes.
    pub(crate) fn new(view: &'a mut DocView, doc: &'a Document, info: &'a DocInfo, placed: &'a [PlacedPage], press: &'a PressState) -> Self {
        Self { view, doc, info, placed, press }
    }

    /// Something is selected that an arrow moves or Delete removes.
    fn has_selection(&self) -> bool {
        self.view.session.objects.count() > 0 || self.view.session.image_selection.is_some() || self.view.session.content.selected.is_some()
    }

    /// Queue `edit` for the app to apply, when there is one.
    fn queue(&mut self, edit: Option<pdfcraft_engine::Edit>) {
        if edit.is_some() {
            self.view.session.pending_edit = edit;
        }
    }
}

impl CanvasTool for EditTextTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        if self.view.session.objects.input_blocked() {
            return ToolAction::Ignored;
        }
        let Some((page, xf)) = self.view.page_xforms().into_iter().find(|(_, xf)| xf.rect.contains(pointer.pos)) else {
            return ToolAction::Ignored;
        };
        // A press with Shift or ⌘ on an object toggles it; where none lies it starts an area selection
        // that adds to the selection.
        if object_ui::modified_press_at(self.view, self.doc, self.info, page, &xf, pointer.pos) {
            return ToolAction::Done;
        }
        object_ui::begin_marquee(self.view, page);
        ToolAction::Claimed
    }

    fn drag(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        if self.view.session.objects.input_blocked() {
            return;
        }
        if let Some(start) = self.press.phase.start() {
            object_ui::end_marquee(self.view, self.doc, self.info, start, pointer.mods, pointer.pos);
        }
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {}

    fn claims(&self) -> ToolClaims {
        if self.has_selection() { ToolClaims::NONE.deletes().nudges() } else { ToolClaims::NONE }
    }

    fn key(&mut self, key: &ToolKey, _cx: &mut ToolContext<'_>) -> bool {
        match key {
            ToolKey::Nudge { direction, mods, .. } => {
                if !self.has_selection() {
                    return false;
                }
                let edit = object_ui::nudge(self.view, self.doc, *direction, mods.shift);
                self.queue(edit);
                true
            }
            ToolKey::Delete { .. } => {
                if !self.has_selection() {
                    return false;
                }
                let edit = object_ui::delete(self.view)
                    .or_else(|| crate::edit_text_ui::delete_picture(self.view, self.placed))
                    .or_else(|| crate::content_ui::delete_selected(self.view));
                self.queue(edit);
                true
            }
            ToolKey::Cancel => {
                if let Some(image) = &mut self.view.session.image_selection
                    && image.crop.take().is_some()
                {
                    return true;
                }
                let had_objects = self.view.session.objects.count() > 0;
                self.view.session.objects.clear();
                let deselected = crate::content_ui::deselect(self.view);
                had_objects || deselected
            }
            _ => false,
        }
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        let (Some(rect), Some(style)) = (object_ui::marquee_rect(&self.press.phase), cx.style()) else { return };
        draw.stroke(&PaintShape::Rect(rect, egui::CornerRadius::ZERO), StrokeWidth::Thin, style.color(style.tokens.color.accent.solid));
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Select All command, the arrow keys and Delete")
    }

    fn cursor(&self, _cx: &ToolContext<'_>) -> Cursor {
        if object_ui::marquee_rect(&self.press.phase).is_some() { Cursor::System(CursorIcon::Crosshair) } else { Cursor::System(CursorIcon::Default) }
    }
}
