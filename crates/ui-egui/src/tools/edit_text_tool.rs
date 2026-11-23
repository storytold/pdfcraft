//! The Edit text tool on the canvas background: an area selection of the page's objects, and the keys of
//! what is selected (arrows move it, Delete removes it, Escape lets go).

use craft::behavior::press::PressState;
use craft::behavior::{CancelReason, Cursor, CursorIcon};
use craft::pages::ui::PlacedPage;
use craft::ui::canvas::{CanvasTool, ToolAction, ToolClaims, ToolContext, ToolKey, ToolPath, ToolPointer};
use craft::ui::primitives::Draw;
use pdfcraft_engine::Document;
use pdfcraft_render::DocInfo;

use crate::canvas::{PageGeometry, PageSession};
use crate::object_ui;

/// Edit text's tool of the canvas: what the families' parts leave of a press, and the keys.
pub(crate) struct EditTextTool<'a> {
    session: &'a mut PageSession,
    doc: &'a Document,
    info: &'a DocInfo,
    /// The pages shown this pass.
    placed: &'a [PlacedPage],
    /// The canvas background's press of this pass: the area selection's span is read from it.
    press: &'a PressState,
    /// The page sizes and rotation the pages are placed with.
    geo: PageGeometry<'a>,
    /// A pointer button is down this pass.
    button_held: bool,
}

impl<'a> EditTextTool<'a> {
    /// The tool over the page session of document `doc`, built each pass with the pages shown and the background
    /// press of the pass; the tool keeps nothing between passes.
    pub(crate) fn new(
        session: &'a mut PageSession,
        doc: &'a Document,
        info: &'a DocInfo,
        placed: &'a [PlacedPage],
        press: &'a PressState,
        geo: PageGeometry<'a>,
    ) -> Self {
        Self { session, doc, info, placed, press, geo, button_held: false }
    }

    /// The tool with a pointer button down this pass (`held`): Delete waits for it to come up.
    #[must_use]
    pub(crate) fn button_held(mut self, held: bool) -> Self {
        self.button_held = held;
        self
    }

    /// Something is selected that an arrow moves or Delete removes.
    fn has_selection(&self) -> bool {
        self.session.objects.count() > 0 || self.session.image_selection.is_some() || self.session.content.selected.is_some()
    }

    /// Queue `edit` for the app to apply, when there is one.
    fn queue(&mut self, edit: Option<pdfcraft_engine::Edit>) {
        if edit.is_some() {
            self.session.pending_edit = edit;
        }
    }
}

impl CanvasTool for EditTextTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        if self.session.objects.input_blocked() {
            return ToolAction::Ignored;
        }
        let Some((page, xf)) = self.session.page_xforms(self.geo).into_iter().find(|(_, xf)| xf.rect.contains(pointer.pos)) else {
            return ToolAction::Ignored;
        };
        // A press with Shift or ⌘ on an object toggles it; where none lies it starts an area selection
        // that adds to the selection.
        if (pointer.mods.shift || pointer.mods.cmd || pointer.mods.ctrl)
            && object_ui::modified_press_at(self.session, self.doc, self.info, page, &xf, pointer.pos)
        {
            return ToolAction::Done;
        }
        object_ui::begin_marquee(self.session, pointer.mods);
        ToolAction::Claimed
    }

    fn drag(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        if self.session.objects.input_blocked() {
            return;
        }
        if let Some(start) = self.press.phase.start() {
            object_ui::end_marquee(self.session, self.doc, self.info, start, pointer.mods, pointer.pos, self.geo);
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
                let edit = object_ui::nudge(self.session, self.doc, *direction, mods.shift, self.geo);
                self.queue(edit);
                true
            }
            ToolKey::Delete { .. } => {
                if !self.has_selection() {
                    return false;
                }
                // Delete waits for the button to come up: the press held would then name another item, whose
                // drag or click would follow. The key is still the tool's.
                if self.button_held {
                    return true;
                }
                let edit = object_ui::delete(self.session)
                    .or_else(|| crate::edit_text_ui::delete_picture(self.session, self.placed, self.geo.viewport))
                    .or_else(|| crate::content_ui::delete_selected(self.session));
                self.queue(edit);
                true
            }
            ToolKey::Cancel => {
                if let Some(image) = &mut self.session.image_selection
                    && image.crop.take().is_some()
                {
                    return true;
                }
                let had_objects = self.session.objects.count() > 0;
                self.session.objects.clear();
                let deselected = crate::content_ui::deselect(self.session);
                had_objects || deselected
            }
            _ => false,
        }
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        let Some(rect) = object_ui::marquee_rect(&self.press.phase) else { return };
        crate::prepare::tool::marquee_fill(draw, cx, rect);
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Select All command, the arrow keys and Delete")
    }

    fn cursor(&self, _cx: &ToolContext<'_>) -> Cursor {
        if object_ui::marquee_rect(&self.press.phase).is_some() { Cursor::System(CursorIcon::Crosshair) } else { Cursor::System(CursorIcon::Default) }
    }
}
