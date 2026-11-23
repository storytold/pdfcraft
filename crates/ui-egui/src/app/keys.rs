use super::*;

impl PdfCraftApp {
    /// The keys the app reads itself, after the runtime has fired the catalog's commands (V and H
    /// are the chords of `view.select_tool` and `view.hand_tool`; the held Space is the tool host's
    /// momentary Hand) and the tool host has taken its tools' keys.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        // Dialogs and prompts are modal: nothing runs underneath them (⌘W closed the file behind
        // Preferences, #870).
        if self.modal_open() || self.prompts.full_screen.is_some() {
            return;
        }
        // The window's Escape: the one no component took this pass (a dialog, a menu, a field, the
        // canvas's tool and sessions all take theirs first). Escape moves nothing, so the
        // direction does not matter.
        let cancel = craft::ui::runtime::keys::with_window_intents(ctx, false, |window| window.take_exact(&craft::behavior::Intent::Cancel));
        if cancel {
            self.escape(ctx);
        }
    }

    /// The app's one Escape arm, run once after the tool host has taken Escape: it closes Find,
    /// or else leaves full screen. Esc closes one thing at a time (#5302): the camera, a field, a
    /// selection and an open menu took theirs before this arm, so full screen stays while any of
    /// them is open. With Preferences ▸ Esc leaves full screen off, only ⌘L / Ctrl+L leaves it. A
    /// Combine file shown large keeps Escape for itself.
    fn escape(&mut self, ctx: &egui::Context) {
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
            && view.escape_closes_find()
        {
            view.find = None;
        } else if self.full_screen() && self.full_screen_escape_exits && self.combine_tab.preview.is_none() {
            self.set_full_screen(false);
        }
    }
}

impl PdfCraftApp {
    /// Select all (`edit.select_all`): the page grid's pages while the Pages panel has the
    /// keyboard, else the document's objects, fields or text for the tool in use. The runtime fires
    /// the command only when no modal or text field has the key.
    pub(crate) fn select_all(&mut self, i: usize, ctx: &egui::Context) {
        if self.views.get(i).is_none() {
            return;
        }
        let pages_owns_keyboard =
            self.right() == Some(RightPanel::Pages) && self.views[i].pages_focus_id.is_some_and(|id| ctx.memory(|m| m.has_focus(id)));
        let view = &self.views[i];
        // A paragraph or text box open for typing keeps ⌘A for its own text.
        let edit_objects = self.quick_tool == QuickTool::EditText && view.session.line_editor.is_none() && view.session.content.draft.is_none();
        if pages_owns_keyboard {
            self.views[i].select_all_pages();
        } else if let Some(doc) = self.session.get(view.id).filter(|d| edit_objects && d.allows_modification()) {
            object_ui::select_all(&mut self.views[i], doc);
        } else if self.is_preparing() {
            if let Some(doc) = self.session.get(self.views[i].id) {
                prepare::select_all(&mut self.views[i], &doc.form);
            }
        } else {
            self.views[i].select_all();
        }
    }
}
