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
    /// or else leaves full screen. A Combine file shown large keeps Escape for itself.
    fn escape(&mut self, ctx: &egui::Context) {
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
            && view.find.is_some()
        {
<<<<<<< HEAD
            view.find = None;
        } else if self.full_screen() && self.combine_tab.preview.is_none() {
            self.set_full_screen(false);
=======
            return;
        }
        // The in-window menu bar holds the keys while one of its menus is open or it has the
        // keyboard (Alt, F10); a command shortcut has left it already (`logic`) and runs. The
        // chords wait meanwhile: the registered commands', Select all and the view's.
        let chords = !craft_menu::bar::is_open(ctx) && !craft_menu::bar::is_focused(ctx);
        // The pages' own chords (Select all, the view's) don't reach pages something covers: the
        // full-screen and update prompts, or the OCR Verify screen in their place (a dialog has
        // returned above, and the palette holds the keyboard).
        let page_chords = !self.covers_pages();
        if chords {
            self.registry_shortcuts(ctx);
        }
        // Esc closes one thing at a time: a Combine file shown large, an open menu, or the camera,
        // a selection or the find bar on the page go first, and full screen stays (#5302). With
        // the Esc leaves full screen preference off, only ⌘L / Ctrl+L leaves it.
        let page_takes_escape =
            egui::Popup::is_any_open(ctx) || self.active.and_then(|i| self.views.get(i)).is_some_and(|v| v.escape_closes_something());
        if self.full_screen
            && self.full_screen_escape_exits
            && self.combine_tab.preview.is_none()
            && !page_takes_escape
            // Taken: the Esc that leaves full screen closes nothing on the page as well.
            && self.page_keys.take(ctx, Key::Escape)
        {
            self.set_full_screen(ctx, false);
        }
        if let Some(i) = self.active {
            if page_chords && chords {
                self.select_all_keys(i, ctx);
            }
            self.tool_keys(i, ctx);
            let keys = self.page_keys;
            if self.quick_tool == QuickTool::EditText
                && let Some(doc) = self.session.get(self.views[i].id).filter(|d| d.allows_modification())
            {
                object_ui::nudge_keys(ctx, keys, &mut self.views[i], doc);
                object_ui::delete_keys(ctx, keys, &mut self.views[i]);
            } else if let Some(doc) = self.session.get(self.views[i].id).filter(|d| d.allows_annotation()) {
                comments::nudge_keys(ctx, keys, &mut self.views[i], &doc.info);
            }
            // A selected field moves with the arrows too (Prepare a form, #2608), before the view
            // turns the page with them.
            if self.is_preparing()
                && let Some(doc) = self.session.get(self.views[i].id).filter(|d| d.allows_modification())
            {
                prepare::nudge_keys(ctx, keys, &mut self.views[i], doc);
            }
            canvas::shortcuts(&mut self.views[i], ctx, keys, chords && page_chords, self.scroll_speed);
            self.clipboard_keys(i, ctx);
            self.image_pick_keys(i, ctx);
>>>>>>> upstream/main
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
