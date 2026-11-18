use super::*;

impl PdfCraftApp {
    /// The keys the app reads itself, after the runtime has fired the catalog's commands (V and H
    /// are the chords of `view.select_tool` and `view.hand_tool`; the held Space is the tool host's
    /// momentary Hand) and the tool host has taken its tools' keys.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        // Dialogs and prompts are modal: nothing runs underneath them (⌘W closed the file behind
        // Preferences, #870).
        if self.modal_open() {
            return;
        }
        // PLAN: Escape is read raw here until the plan names the signal for "the tool host did not take Escape" (a declined-key output or a catalog `app.escape` command); it then calls `escape` from that signal.
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.escape(ctx);
        }
    }

    /// The app's one Escape arm, run once after the tool host has taken Escape: it closes Find,
    /// or else leaves full screen. A Combine file shown large keeps Escape for itself.
    fn escape(&mut self, ctx: &egui::Context) {
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
            && view.find.is_some()
        {
            view.find = None;
        } else if self.full_screen() && self.combine_tab.preview.is_none() {
            self.set_full_screen(ctx, false);
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
            self.right == Some(RightPanel::Pages) && self.views[i].pages_focus_id.is_some_and(|id| ctx.memory(|m| m.has_focus(id)));
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
