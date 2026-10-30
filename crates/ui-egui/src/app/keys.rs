use super::*;

impl PdfCraftApp {
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::Key;
        self.eraser_key_frame(ctx);
        let pasted = self.paste_key_released(ctx);
        // "Save changes?" is modal: its keys are its own (⌘D is Don't save there, not Document
        // properties; Escape cancels it rather than clearing a selection), and nothing may run
        // underneath it. They are read here, before the canvas can consume them.
        if self.close_request.is_some() {
            // These dialogs are drawn after the save prompt and own the keyboard while open.
            // Don't let Escape or Return resolve unsaved work underneath one.
            let covered = self.pending_link.is_some()
                || self.pending_remote_link.is_some()
                || self.pending_full_screen.is_some()
                || self.updates.open
                || self.dialog.is_some()
                || self.unlock_prompt.is_some();
            if !covered && let Some(choice) = dialogs::save_prompt_key(ctx, &mut self.save_prompt_button) {
                self.resolve_close(ctx, choice);
            }
            return;
        }
        // The other dialogs and prompts are modal too (⌘W closed the file behind Preferences,
        // #870).
        if self.modal_open() {
            return;
        }
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
            && view.auto_scroll.escape(ctx)
        {
            return;
        }
        // The in-window menu bar holds the keys while one of its menus is open or it has the
        // keyboard (Alt, F10); a command shortcut leaves it and runs.
        craft_menu::bar::leave_for_shortcut(ctx);
        if !craft_menu::bar::is_open(ctx) && !craft_menu::bar::is_focused(ctx) {
            self.registry_shortcuts(ctx);
        }
        // (Esc closes a Combine file shown large first.)
        if self.full_screen && self.combine_tab.preview.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.set_full_screen(ctx, false);
        }
        if let Some(i) = self.active {
            // Select all belongs to the document or page grid, unless a text field or
            // overlay owns the keyboard. Other canvas shortcuts keep their own handling.
            if self.dialog.is_none()
                && !self.palette_open
                && !ctx.egui_wants_keyboard_input()
                && ctx.input_mut(|input| input.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::A)))
            {
                if self.is_preparing() {
                    if let Some(doc) = self.session.get(self.views[i].id) {
                        prepare::select_all(&mut self.views[i], &doc.form);
                    }
                } else {
                    self.views[i].select_all();
                }
            }
            self.tool_keys(i, ctx);
            if self.quick_tool == QuickTool::EditText
                && let Some(doc) = self.session.get(self.views[i].id).filter(|d| d.allows_modification())
            {
                object_ui::nudge_keys(ctx, &mut self.views[i], doc);
                object_ui::delete_keys(ctx, &mut self.views[i]);
            } else if let Some(doc) = self.session.get(self.views[i].id).filter(|d| d.allows_annotation()) {
                comments::nudge_keys(ctx, &mut self.views[i], &doc.info);
            }
            // A selected field moves with the arrows too (Prepare a form, #2608), before the view
            // turns the page with them.
            if self.is_preparing()
                && let Some(doc) = self.session.get(self.views[i].id).filter(|d| d.allows_modification())
            {
                prepare::nudge_keys(ctx, &mut self.views[i], doc);
            }
            canvas::shortcuts(&mut self.views[i], ctx, self.scroll_speed);
            self.copy_key(i, ctx);
            self.paste_image_key(i, ctx, pasted);
            self.image_pick_keys(i, ctx);
        }
    }
}

use crate::comments::CommentTool;

/// How long E must be held for the Eraser to be only temporary (#3900).
const ERASER_HOLD_SECS: f64 = 0.35;
const DRAW: QuickTool = QuickTool::Comment(CommentTool::Ink);
const ERASER: QuickTool = QuickTool::Comment(CommentTool::Eraser);

impl PdfCraftApp {
    /// Make `tool` the active one and the one its quick-bar group shows, as choosing it from the
    /// bar or its command does.
    fn set_drawing_tool(&mut self, tool: CommentTool) {
        self.comment_prefs.group_tool[tool.group()] = tool;
        self.quick_tool = QuickTool::Comment(tool);
    }

    /// The end of a held E (#3900), settled every frame, even under a dialog, so that the
    /// moment the key comes up is the moment it is read. A hold of [`ERASER_HOLD_SECS`] or more
    /// gives the Draw tool back; a tap leaves the Eraser. The hold is over, with nothing given
    /// back, once the user has chosen another tool. Space's temporary Hand may sit on top of the
    /// Eraser meanwhile: it then returns to Draw when Space is let go.
    pub(super) fn eraser_key_frame(&mut self, ctx: &egui::Context) {
        let Some(since) = self.eraser_key else { return };
        let on_top = self.quick_tool == QuickTool::Hand && self.space_hand == Some(ERASER);
        if self.quick_tool != ERASER && !on_top {
            self.eraser_key = None;
            return;
        }
        if ctx.input(|input| input.key_down(egui::Key::E)) {
            return;
        }
        self.eraser_key = None;
        if ctx.input(|input| input.time) - since >= ERASER_HOLD_SECS {
            if on_top {
                // The Hand gives Draw back when Space is let go, and the bar shows it already.
                self.space_hand = Some(DRAW);
                self.comment_prefs.group_tool[CommentTool::Ink.group()] = CommentTool::Ink;
            } else {
                self.set_drawing_tool(CommentTool::Ink);
            }
        }
    }

    /// The quick tools' keys, as in Acrobat: V selects, H pans, and holding Space pans until it
    /// is released. Plain letters, so not while a text field, a form field, a dialog or the
    /// palette has the keyboard.
    fn tool_keys(&mut self, i: usize, ctx: &egui::Context) {
        use egui::Key;
        let free = self.dialog.is_none() && !self.palette_open && !ctx.egui_wants_keyboard_input() && self.views[i].forms.focus.is_none();
        let (plain, space) = ctx.input(|input| (input.modifiers.is_none(), input.key_down(Key::Space)));
        if let Some(previous) = self.space_hand
            && !space
        {
            self.space_hand = None;
            if self.quick_tool == QuickTool::Hand {
                self.quick_tool = previous;
            }
        }
        if !free || !plain {
            return;
        }
        // E and B swap the Draw tool and the Eraser (#3900): a tap of E switches to the Eraser and
        // stays, another tap of E or a tap of B goes back; holding E is the Eraser only while held.
        // Only from one of the two, so E means nothing to any other tool. The press itself must be
        // plain too: a Shift+E let go within the frame leaves the frame's modifiers empty. Presses
        // are taken in the order they came, as a frame can hold more than one.
        let presses: Vec<Key> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key: key @ (Key::E | Key::B), pressed: true, repeat: false, modifiers, .. } if modifiers.is_none() => {
                        Some(*key)
                    }
                    _ => None,
                })
                .collect()
        });
        for key in presses {
            match (key, self.quick_tool) {
                (Key::E, DRAW) => {
                    self.set_drawing_tool(CommentTool::Eraser);
                    // A hold is timed only while E is still down: a tap whose release came in the
                    // same frame is over, and the next frame may come much later.
                    self.eraser_key = ctx.input(|input| input.key_down(Key::E)).then(|| ctx.input(|input| input.time));
                }
                (Key::E | Key::B, ERASER) => {
                    self.set_drawing_tool(CommentTool::Ink);
                    self.eraser_key = None;
                }
                _ => {}
            }
        }
        if ctx.input(|input| input.key_pressed(Key::V)) {
            self.quick_tool = QuickTool::Select;
            self.space_hand = None;
        }
        if ctx.input(|input| input.key_pressed(Key::H)) {
            self.quick_tool = QuickTool::Hand;
            self.space_hand = None;
        }
        if space && self.space_hand.is_none() && self.quick_tool != QuickTool::Hand {
            self.space_hand = Some(self.quick_tool);
            self.quick_tool = QuickTool::Hand;
        }
    }
}
