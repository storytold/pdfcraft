use super::*;
use crate::page_keys::PageKeys;

impl PdfCraftApp {
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::Key;
        // "Save changes?" is modal: its keys are its own (⌘D is Don't save there, not Document
        // properties; Escape cancels it rather than clearing a selection), and nothing may run
        // underneath it. They are read here, before the canvas can consume them.
        if self.close_request.is_some() {
            // These dialogs are drawn after the save prompt and own the keyboard while open.
            // Don't let Escape or Return resolve unsaved work underneath one.
            #[cfg(not(target_arch = "wasm32"))]
            if self.script_pending.as_ref().is_some_and(|pending| pending.has_prompt()) {
                return;
            }
            let covered = self.pending_link.is_some()
                || self.pending_remote_link.is_some()
                || self.pending_full_screen.is_some()
                || self.updates.open
                || self.dialog.is_some()
                || self.unlock_prompt.is_some();
            if !covered && let Some(choice) = dialogs::save_prompt_key(ctx, &mut self.save_prompt_button) {
                self.resolve_close(ctx, choice);
            }
            self.cancel_space_tap(ctx);
            return;
        }
        // The other dialogs and prompts are modal too (⌘W closed the file behind Preferences,
        // #870).
        if self.modal_open() {
            // A Space let go under the dialog is no tap: the pages never see it released. One
            // still held when it closes stays no tap until it is let go.
            self.cancel_space_tap(ctx);
            return;
        }
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
            && view.auto_scroll.escape(ctx)
        {
            self.cancel_space_tap(ctx);
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
        // (Esc closes a Combine file shown large first.)
        if self.full_screen && self.combine_tab.preview.is_none() && self.page_keys.pressed(ctx, Key::Escape) {
            self.set_full_screen(ctx, false);
        }
        if let Some(i) = self.active {
            if page_chords && chords {
                self.select_all_keys(i, ctx);
            }
            let space = self.tool_keys(i, ctx);
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
            canvas::shortcuts(&mut self.views[i], ctx, keys, chords && page_chords, self.scroll_speed, space);
            self.clipboard_keys(i, ctx);
            self.image_pick_keys(i, ctx);
        }
    }
}

impl PdfCraftApp {
    /// ⌘A selects all of the document or the page grid, unless a text field or overlay owns the
    /// keyboard.
    fn select_all_keys(&mut self, i: usize, ctx: &egui::Context) {
        use egui::Key;
        let pages_owns_keyboard =
            self.right == Some(RightPanel::Pages) && self.views[i].pages_focus_id.is_some_and(|id| ctx.memory(|m| m.has_focus(id)));
        if self.dialog.is_none()
            && !self.palette_open
            && (pages_owns_keyboard || !ctx.egui_wants_keyboard_input())
            && ctx.input_mut(|input| input.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::A)))
        {
            let view = &self.views[i];
            // A paragraph or text box open for typing keeps ⌘A for its own text.
            let edit_objects = self.quick_tool == QuickTool::EditText && view.line_editor.is_none() && view.content.draft.is_none();
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

    /// Space's tap is cancelled: something other than the pages had the keyboard. A Space still
    /// down stays no tap (`Some(false)`) until it is let go, so neither its repeats nor its
    /// release step the pages once they have the keyboard again.
    fn cancel_space_tap(&mut self, ctx: &egui::Context) {
        self.space_tap = PageKeys::held(ctx, egui::Key::Space).then_some(false);
    }

    /// The quick tools' keys, as in Acrobat: V selects, H pans, and holding Space pans until it
    /// is released. Plain letters, so not while a text field, a form field, a dialog, the palette
    /// or a menu has the keyboard.
    ///
    /// Space scrolls too, as in Acrobat (#7483): pressed and let go without a press of the pointer
    /// in between it goes a screen on, as Page Down does, and Shift+Space a screen back, as Page
    /// Up. Neither repeats while the key is held. Returns that step for the view: `Some(true)` on,
    /// `Some(false)` back. Full screen keeps Space and Shift+Space for its own page turning.
    fn tool_keys(&mut self, i: usize, ctx: &egui::Context) -> Option<bool> {
        use egui::Key;
        let keys = self.page_keys;
        let free = self.dialog.is_none() && !self.palette_open && !ctx.egui_wants_keyboard_input() && self.views[i].forms.focus.is_none();
        let free = free && !keys.over();
        // Only a Space the pages see go down and come back up is a tap: one let go while something
        // else had the keyboard is not.
        if !free {
            self.cancel_space_tap(ctx);
        }
        let (plain, space) = (ctx.input(|input| input.modifiers.is_none()), PageKeys::held(ctx, Key::Space));
        // Space going down starts a tap, once: the key's repeats while it is held start none.
        if free && plain && self.space_tap.is_none() && keys.pressed(ctx, Key::Space) {
            self.space_tap = Some(true);
        }
        // A press with the pointer while Space is down is the Hand's drag, not a tap.
        if self.space_tap.is_some() && ctx.input(|input| input.pointer.any_down() || input.pointer.any_pressed()) {
            self.space_tap = Some(false);
        }
        let mut step = None;
        if !space && self.space_tap.take() == Some(true) && free {
            step = Some(true);
        }
        if let Some(previous) = self.space_hand
            && !space
        {
            self.space_hand = None;
            if self.quick_tool == QuickTool::Hand {
                self.quick_tool = previous;
            }
        }
        if free && !self.views[i].presenting() {
            // Shift+Space starts no Hand, so it scrolls back at once. Judged by the press's own
            // modifiers, and taken so that nothing else reads it as a plain Space. Only its first
            // press steps: the key's repeats while it is held, then its release, find a tap that
            // isn't one (`Some(false)`), and a plain Space repeating after Shift is let go starts
            // none.
            let tap = &mut self.space_tap;
            keys.take_presses(ctx, |key, modifiers| {
                let back = key == Key::Space && modifiers.shift_only();
                if back {
                    if tap.is_none() {
                        step = Some(false);
                    }
                    *tap = Some(false);
                }
                back
            });
        }
        if !free || !plain {
            return step;
        }
        if keys.pressed(ctx, Key::V) {
            self.quick_tool = QuickTool::Select;
            self.space_hand = None;
        }
        if keys.pressed(ctx, Key::H) {
            self.quick_tool = QuickTool::Hand;
            self.space_hand = None;
        }
        if space && self.space_hand.is_none() && self.quick_tool != QuickTool::Hand {
            self.space_hand = Some(self.quick_tool);
            self.quick_tool = QuickTool::Hand;
        }
        step
    }
}
