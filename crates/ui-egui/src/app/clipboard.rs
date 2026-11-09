use super::*;

/// Why nothing can be copied out of the document: its security doesn't allow it (Table 22,
/// bit 5; #568).
pub(crate) fn copy_refused() -> &'static str {
    tl!("The document's security settings don't allow copying text and images")
}

pub(crate) fn print_refused() -> &'static str {
    tl!("The document's security settings don't allow printing")
}

enum ClipboardPaste {
    Text(String),
    Image(std::sync::Arc<egui::ColorImage>),
}

impl PdfCraftApp {
    /// Whether text and images may be copied out of document `id`; says why not when they may
    /// not. Reading aloud and accessibility tools don't ask (assistive technology may read
    /// anything, §7.6.4.2).
    pub(crate) fn may_copy(&mut self, id: DocId) -> bool {
        if self.session.get(id).is_none_or(pdfcraft_engine::Document::allows_copying) {
            return true;
        }
        self.notify(copy_refused().to_string());
        false
    }

    /// ⌘C / ⌘X / ⌘V copy, cut and paste text, images or comments on canvas (unless a text field
    /// has focus). With no text selected, ⌘C copies the selected comments, each with its replies.
    pub(super) fn clipboard_keys(&mut self, i: usize, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let pastes: Vec<_> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Paste(text) => Some(ClipboardPaste::Text(text.clone())),
                    egui::Event::PasteImage(image) => Some(ClipboardPaste::Image(image.clone())),
                    _ => None,
                })
                .collect()
        });
        let free = self.dialog.is_none() && !self.palette_open && !ctx.egui_wants_keyboard_input() && !self.is_preparing();
        if !free || self.views.get(i).is_some_and(|v| v.forms.focus.is_some() || v.organize) {
            self.receive_pastes(i, pastes, false);
            return;
        }
        let copy = ctx.input(|input| input.events.iter().any(|e| matches!(e, egui::Event::Copy)))
            || ctx.input_mut(|input| input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::C)));
        let cut = ctx.input(|input| input.events.iter().any(|e| matches!(e, egui::Event::Cut)))
            || ctx.input_mut(|input| input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::X)));
        let paste = ctx.input_mut(|input| input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::V)));

        let text_selected = copy && self.views.get(i).and_then(|v| v.selected_text()).is_some_and(|t| !t.is_empty());
        if copy
            && (text_selected || !self.copy_selected_comments(i))
            && !self.copy_selected_text(i, ctx)
            && let Some(sel) = self.views.get(i).and_then(|v| v.image_selection.clone())
            && let Some(id) = self.views.get(i).map(|v| v.id)
            && self.may_copy(id)
        {
            self.copy_page_image(sel.page, sel.index);
        }
        if cut
            && let Some(sel) = self.views.get(i).and_then(|v| v.image_selection.clone())
            && let Some(id) = self.views.get(i).map(|v| v.id)
            && self.may_copy(id)
        {
            self.cut_page_image(sel.page, sel.index);
        }
        if !pastes.is_empty() {
            self.receive_pastes(i, pastes, true);
            return;
        }
        if paste {
            let has_override_image = matches!(self.clipboard_override, Some(crate::create_ui::Clip::Image { .. }));
            #[cfg(not(target_arch = "wasm32"))]
            let system_has_text =
                !has_override_image && self.system_clipboard && (self.own_clipboard_text.is_some() || crate::os_clipboard::has_text());
            #[cfg(target_arch = "wasm32")]
            let system_has_text = false;

            if !has_override_image && system_has_text {
                self.last_copied_image = None;
                self.last_copied_file = None;
                return;
            }

            let page = self.views.get(i).map_or(0, |v| v.current);
            self.paste_image(page, None);
        }
    }

    /// `on_canvas`: the paste is the canvas's (no text field, dialog or page grid has it), so
    /// pictures and copied comments land on the page.
    fn receive_pastes(&mut self, view: usize, pastes: Vec<ClipboardPaste>, on_canvas: bool) {
        let page = self.views.get(view).map_or(0, |view| view.current);
        for paste in pastes {
            match paste {
                ClipboardPaste::Text(text) => {
                    // Text fields may own the paste; the canvas cache still observes its contents.
                    self.last_copied_image = None;
                    self.last_copied_file = None;
                    if on_canvas {
                        self.paste_comment_text(view, text);
                    }
                }
                ClipboardPaste::Image(image) if on_canvas => {
                    self.paste_received_image(page, &image);
                }
                ClipboardPaste::Image(_) => {}
            }
        }
    }

    /// A text pasted onto the canvas: XFDF with comments is pasted as comments. Copied comments
    /// come from the system clipboard when it holds them, from the app's own only when it holds
    /// nothing legible (a word copied in another app since is that word, not the comments copied
    /// before it).
    fn paste_comment_text(&mut self, i: usize, text: String) {
        if pdfcraft_engine::is_xfdf_comments(&text) {
            self.paste_comments(i, Some(text), None);
        } else if text.trim().is_empty() && self.comment_clipboard.is_some() {
            self.paste_comments(i, None, None);
        }
    }

    /// Copy the selected comments of tab `i` as XFDF, each with its replies: into the app's
    /// clipboard (for ⌘V in any tab or window) and onto the system clipboard as text. False
    /// when nothing is selected.
    pub fn copy_selected_comments(&mut self, i: usize) -> bool {
        let Some(view) = self.views.get(i) else { return false };
        let keys = view.comments.selection();
        if keys.is_empty() {
            return false;
        }
        let id = view.id;
        match self.session.export_comments(id, &keys) {
            Ok(xfdf) => {
                if let Some(ctx) = &self.ctx {
                    ctx.copy_text(xfdf.clone());
                }
                self.comment_clipboard = Some(CommentClip { xfdf, count: keys.len() });
                true
            }
            Err(e) => {
                self.notify_error(e);
                false
            }
        }
    }

    /// Paste copied comments (`text`, XFDF from the system clipboard, or else the app's
    /// clipboard) into tab `i`. With `at` (a page and a user-space point, from the page's menu)
    /// the bundle is placed at once, centred there; otherwise it hangs on the pointer as one
    /// bundle until a click places it (`Gesture::Place`), so a copy never lands on its original
    /// and a bundle from a large page goes where it fits on a small one. The copies are selected
    /// once placed (see `apply_edit`).
    pub fn paste_comments(&mut self, i: usize, text: Option<String>, at: Option<(usize, [f64; 2])>) -> bool {
        let Some(xfdf) = text.or_else(|| self.comment_clipboard.as_ref().map(|c| c.xfdf.clone())) else {
            self.notify_tr("Copy comments first");
            return false;
        };
        let Some(view) = self.views.get(i) else { return false };
        let id = view.id;
        let Some(doc) = self.session.get(id) else { return false };
        if !doc.allows_annotation() {
            self.notify_tr("The document's security settings don't allow this change");
            return false;
        }
        let count = pdfcraft_engine::comment_count(&xfdf);
        let Some(bounds) = pdfcraft_engine::comments_bounds(&xfdf).filter(|_| count > 0) else {
            self.notify_tr("The clipboard holds no comments");
            return false;
        };
        match at {
            Some((page, [x, y])) => {
                let offset = [x - (bounds[0] + bounds[2]) / 2.0, y - (bounds[1] + bounds[3]) / 2.0];
                self.apply_edit(pdfcraft_engine::Edit::PasteComments { xfdf, page, offset: Some(offset) })
            }
            None => {
                let Some(view) = self.views.get_mut(i) else { return false };
                view.comments.clear_selection();
                let outlines = pdfcraft_engine::comment_outlines(&xfdf);
                view.comments.gesture = Some(comments::Gesture::Place { xfdf, bounds, count, outlines });
                self.notify_tr("Click where the copied comments go; Escape lets them go");
                true
            }
        }
    }

    /// Shared permission gate for copying selected page text from menus and keyboard events.
    /// After Select All of the document (#2639), pages whose text wasn't read for display are read
    /// off the UI thread ([`DocumentCopy`]), and the clipboard gets the text when they're done.
    pub(crate) fn copy_selected_text(&mut self, i: usize, ctx: &egui::Context) -> bool {
        // A newer copy wins: a whole-document copy still reading must not replace it later.
        self.document_copy = None;
        let Some(v) = self.views.get(i) else { return false };
        let id = v.id;
        if v.all_text && !v.pages_without_text().is_empty() {
            // Checked before any page is read: a document that refuses copying reads nothing.
            if !self.may_copy(id) {
                return false;
            }
            let Some(job) = self.views.get(i).and_then(|v| Some(DocumentCopyJob::of(v, self.session.get(id)?))) else { return false };
            self.document_copy = Some(DocumentCopy::start(id, job, ctx));
            return true;
        }
        let Some(text) = v.selected_text().filter(|text| !text.is_empty()) else { return false };
        if !self.may_copy(id) {
            return false;
        }
        ctx.copy_text(text);
        true
    }

    /// Puts an image or a file on the system clipboard through arboard (`put`). It replaces the
    /// text this window copied, so [`Self::own_clipboard_text`] is forgotten once `put` has
    /// succeeded; a failed write leaves that text there.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn put_on_system_clipboard(&mut self, put: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        #[cfg(test)]
        let put = || if crate::os_clipboard::writes_faked() { Ok(()) } else { put() };
        put()?;
        self.own_clipboard_text = None;
        Ok(())
    }

    /// Keeps [`Self::own_clipboard_text`] up to date at the end of a frame: the text this frame
    /// copied through egui (the last copy wins), while the window has focus. A copy made without
    /// focus may not take (Wayland refuses the claim) and is not remembered. A focus loss forgets
    /// it ([`Self::forget_own_clipboard_on_focus_loss`]).
    pub(crate) fn note_own_clipboard(&mut self, ctx: &egui::Context) {
        let copied = ctx.output(|o| {
            o.commands.iter().rev().find_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
        });
        if let Some(text) = copied {
            let focused = ctx.input(|i| i.focused);
            self.own_clipboard_text = Some(text).filter(|t| focused && !t.is_empty());
        }
    }

    /// Forgets [`Self::own_clipboard_text`] once the window has lost focus, when another app may
    /// have replaced it; before anything reads the pass's input. The focus-loss event counts, not
    /// only `focused`: a minimized or covered window runs no UI pass, so the loss and the focus
    /// regained can reach the same one.
    pub(crate) fn forget_own_clipboard_on_focus_loss(&mut self, raw: &egui::RawInput) {
        if !raw.focused || raw.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))) {
            self.own_clipboard_text = None;
        }
    }

    /// The clipboard text of a whole-document copy, once its pages are read; dropped with the
    /// document it was copied from, and by any later copy (a text field's, a comment's), which
    /// it must not replace. Run at the end of the frame, so it sees this frame's copies.
    pub(crate) fn finish_document_copy(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.document_copy else { return };
        let copied = ctx.output(|o| o.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_))));
        if copied || !self.views.iter().any(|v| v.id == c.doc) {
            self.document_copy = None;
            return;
        }
        match c.rx.try_recv() {
            Ok(text) => {
                self.document_copy = None;
                if !text.is_empty() {
                    ctx.copy_text(text);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.document_copy = None,
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    fn delivered() -> egui::Event {
        egui::Event::PasteImage(std::sync::Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::GREEN)))
    }

    fn key(pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key: egui::Key::V, physical_key: None, pressed, repeat: false, modifiers }
    }

    fn image_app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        app.clipboard_override = Some(crate::create_ui::Clip::Image { width: 1, height: 1, rgba: vec![255, 0, 0, 255] });
        app
    }

    fn frame(app: &mut PdfCraftApp, ctx: &egui::Context, time: f64, events: Vec<egui::Event>) -> usize {
        ctx.run_ui(egui::RawInput { time: Some(time), events, ..Default::default() }, |ui| app.clipboard_keys(0, ui.ctx()))
            .drop_without_applying_deltas();
        app.session.get(app.views[0].id).unwrap().page_images(0).len()
    }

    #[test]
    fn delivered_image_pastes_for_either_modifier_release_order() {
        for command_first in [true, false] {
            let mut app = image_app();
            let ctx = egui::Context::default();
            frame(&mut app, &ctx, 1.0, vec![egui::Event::ModifiersChanged(egui::Modifiers::COMMAND)]);
            let releases = if command_first {
                vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]
            } else {
                vec![key(false, egui::Modifiers::COMMAND), egui::Event::ModifiersChanged(egui::Modifiers::NONE)]
            };
            assert_eq!(frame(&mut app, &ctx, 1.1, releases), 0, "key release is not a delivered paste");
            assert_eq!(frame(&mut app, &ctx, 1.2, vec![delivered()]), 1, "asynchronous pixels identify the actual paste");
            assert_eq!(frame(&mut app, &ctx, 1.25, vec![key(false, egui::Modifiers::NONE)]), 1);
        }
    }

    #[test]
    fn clipboard_history_after_a_completed_paste_does_not_paste_on_open() {
        for explicit_press in [false, true] {
            let mut app = image_app();
            let ctx = egui::Context::default();
            let mut first = vec![egui::Event::ModifiersChanged(egui::Modifiers::COMMAND), delivered()];
            if explicit_press {
                first.push(key(true, egui::Modifiers::COMMAND));
            }
            first.push(key(false, egui::Modifiers::COMMAND));
            assert_eq!(frame(&mut app, &ctx, 1.0, first), 1);
            // The integration drops Windows-key identity; Win+V can leave only this release.
            assert_eq!(
                frame(&mut app, &ctx, 1.1, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]),
                1,
                "opening history must not paste"
            );
            assert_eq!(
                frame(&mut app, &ctx, 1.2, vec![delivered(), key(true, egui::Modifiers::COMMAND), key(false, egui::Modifiers::COMMAND)]),
                2,
                "choosing an item produces exactly one intentional paste"
            );
        }
    }

    #[test]
    fn clipboard_history_and_plain_v_after_an_unrelated_shortcut_do_nothing() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        let copy = egui::Event::Key { key: egui::Key::C, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        frame(&mut app, &ctx, 1.0, vec![egui::Event::ModifiersChanged(egui::Modifiers::COMMAND), copy]);
        assert_eq!(frame(&mut app, &ctx, 1.1, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]), 0);
        assert_eq!(frame(&mut app, &ctx, 1.2, vec![key(true, egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]), 0);
        assert_eq!(frame(&mut app, &ctx, 10.0, vec![key(false, egui::Modifiers::NONE)]), 0);
    }

    #[test]
    fn delivered_image_is_authoritative_and_not_duplicated_by_keys() {
        let mut app = image_app();
        app.system_clipboard = true;
        app.clipboard_override = Some(crate::create_ui::Clip::Text("newer OS contents".into()));
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![delivered(), key(true, egui::Modifiers::COMMAND), key(false, egui::Modifiers::COMMAND)]), 1);
        assert_eq!(app.last_copied_image.as_ref().unwrap().2, vec![0, 255, 0, 255]);
    }

    #[test]
    fn text_then_image_batch_pastes_image_and_keeps_its_cache() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![egui::Event::Paste("text".into()), delivered()]), 1);
        assert_eq!(app.last_copied_image.unwrap().2, vec![0, 255, 0, 255]);
    }

    #[test]
    fn image_then_text_batch_pastes_image_and_leaves_text_authoritative() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![delivered(), egui::Event::Paste("text".into())]), 1);
        assert!(app.last_copied_image.is_none());
        assert!(app.last_copied_file.is_none());
    }

    #[test]
    fn separate_delivered_images_in_one_batch_each_paste_once() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        assert_eq!(
            frame(&mut app, &ctx, 1.0, vec![delivered(), delivered(), key(true, egui::Modifiers::COMMAND), key(false, egui::Modifiers::COMMAND)]),
            2
        );
    }

    #[test]
    fn explicit_control_shortcut_still_pastes_once_without_an_image_event() {
        let mut app = image_app();
        let ctx = egui::Context::default();
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![key(true, egui::Modifiers::COMMAND)]), 1);
        assert_eq!(frame(&mut app, &ctx, 1.1, vec![key(false, egui::Modifiers::COMMAND)]), 1);
    }

    #[test]
    fn delivered_image_obeys_focus_modal_and_permission_guards() {
        let ctx = egui::Context::default();
        let mut app = image_app();
        app.palette_open = true;
        assert_eq!(frame(&mut app, &ctx, 1.0, vec![delivered()]), 0);
        app.palette_open = false;
        app.views[0].organize = true;
        assert_eq!(frame(&mut app, &ctx, 2.0, vec![delivered()]), 0);
        app.views[0].organize = false;
        app.apply_edit(pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection {
            permissions_password: Some("owner".into()),
            ..Default::default()
        }));
        let bytes = app.session.save_bytes(app.views[0].id).unwrap();
        let mut restricted = PdfCraftApp::new();
        restricted.open_bytes("restricted.pdf", None, bytes.as_ref().clone()).unwrap();
        assert!(!restricted.session.get(restricted.views[0].id).unwrap().allows_modification());
        assert_eq!(frame(&mut restricted, &ctx, 3.0, vec![delivered()]), 0);
    }

    #[test]
    fn review_an_image_less_release_is_silent_but_explicit_paste_explains() {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        app.toast = None;
        let ctx = egui::Context::default();
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| app.clipboard_keys(0, ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert!(app.toast.is_none());
        assert!(!app.paste_image(0, None));
        assert!(app.toast.is_some(), "an explicit menu action still explains why there is no image");
    }

    #[test]
    fn review_text_paste_release_is_silent_and_later_image_paste_still_works() {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        app.last_copied_image = Some((1, 1, vec![255, 0, 0, 255]));
        app.palette_open = true; // A text field owns the first event.
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput { events: vec![egui::Event::Paste("text".into())], ..Default::default() }, |ui| app.clipboard_keys(0, ui.ctx()))
            .drop_without_applying_deltas();
        assert!(app.last_copied_image.is_none());
        app.palette_open = false;
        app.toast = None;
        ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| app.clipboard_keys(0, ui.ctx()),
        )
        .drop_without_applying_deltas();
        assert!(app.toast.is_none(), "the text gesture's key release must be silent");
        app.clipboard_override = None;
        let delivered = egui::Event::PasteImage(std::sync::Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::GREEN)));
        ctx.run_ui(egui::RawInput { events: vec![delivered], ..Default::default() }, |ui| app.clipboard_keys(0, ui.ctx()))
            .drop_without_applying_deltas();
        let id = app.active_ids().unwrap().1;
        assert_eq!(app.session.get(id).unwrap().page_images(0).len(), 1, "a later real image shortcut still works");
    }

    #[test]
    fn review_failed_keyboard_cut_keeps_the_image_selection() {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.create_blank();
        app.views[0].image_selection = Some(crate::edit_text_ui::ImageSelection::at(0, 0));
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput { events: vec![egui::Event::Cut], ..Default::default() }, |ui| app.clipboard_keys(0, ui.ctx()))
            .drop_without_applying_deltas();
        assert!(app.views[0].image_selection.is_some());
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod own_clipboard_tests {
    use super::*;

    /// A frame that copies `copy` through egui (if any), with the window focused or not, ended
    /// as production ends it.
    fn frame(app: &mut PdfCraftApp, ctx: &egui::Context, focused: bool, copy: Option<&str>) {
        frame_with(app, ctx, egui::RawInput { focused, ..Default::default() }, copy);
    }

    /// A frame on `raw`, through the raw input hook first, as production runs it.
    fn frame_with(app: &mut PdfCraftApp, ctx: &egui::Context, mut raw: egui::RawInput, copy: Option<&str>) {
        app.prepare_raw_input(ctx, &mut raw);
        ctx.run_ui(raw, |ui| {
            if let Some(text) = copy {
                ui.ctx().copy_text(text.to_owned());
            }
            app.finish_render_frame(ui.ctx());
        })
        .drop_without_applying_deltas();
    }

    /// #6169: after the window copied text, a right-click's Paste Image check, Paste Image and
    /// Create ▸ Clipboard answer from that text. None of them opens the system clipboard:
    /// reading this window's own selection back waits for this thread, for good on Wayland.
    #[test]
    fn after_copying_text_nothing_reads_the_clipboard_back() {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = true;
        app.create_blank();
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, true, Some("copied words"));
        assert_eq!(app.own_clipboard_text.as_deref(), Some("copied words"));

        let opened = crate::os_clipboard::opens();
        assert!(!app.can_paste_image(), "text supersedes an image");
        assert!(!app.paste_image(0, None), "no image to paste");
        let docs = app.views.len();
        app.create_from_clipboard();
        assert_eq!(app.views.len(), docs + 1, "a new document from the copied text");
        assert_eq!(crate::os_clipboard::opens(), opened, "the system clipboard was opened");
    }

    /// Another app can replace the clipboard only while the window doesn't have focus: losing
    /// focus forgets the copied text. A later copy replaces it, and an empty one clears it.
    #[test]
    fn the_copied_text_lasts_until_the_window_loses_focus_or_a_later_copy() {
        let mut app = PdfCraftApp::new();
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, true, Some("first"));
        frame(&mut app, &ctx, true, None);
        assert_eq!(app.own_clipboard_text.as_deref(), Some("first"), "kept while focused");
        frame(&mut app, &ctx, true, Some("second"));
        assert_eq!(app.own_clipboard_text.as_deref(), Some("second"));
        frame(&mut app, &ctx, false, None);
        assert_eq!(app.own_clipboard_text, None, "forgotten once the window lost focus");
        frame(&mut app, &ctx, true, Some("third"));
        frame(&mut app, &ctx, true, Some(""));
        assert_eq!(app.own_clipboard_text, None, "an empty copy");
    }

    /// A minimized or covered window runs no UI pass, so a focus loss there reaches the next
    /// pass together with the focus regained: the copied text is forgotten before anything in
    /// that pass reads it.
    #[test]
    fn focus_lost_and_regained_between_passes_forgets_the_copied_text() {
        let mut app = PdfCraftApp::new();
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, true, Some("copied words"));
        let mut raw =
            egui::RawInput { focused: true, events: vec![egui::Event::WindowFocused(false), egui::Event::WindowFocused(true)], ..Default::default() };
        app.prepare_raw_input(&ctx, &mut raw);
        assert_eq!(app.own_clipboard_text, None, "forgotten before the pass reads its input");
    }

    /// A copy made while the window doesn't have focus, such as a whole-document copy finishing
    /// after the user switched away, may not reach the clipboard (Wayland refuses the claim), and
    /// another app may replace it before the window is back: it isn't remembered.
    #[test]
    fn a_copy_made_without_focus_is_not_remembered() {
        let mut app = PdfCraftApp::new();
        let ctx = egui::Context::default();
        frame(&mut app, &ctx, false, Some("late words"));
        assert_eq!(app.own_clipboard_text, None);
        frame(&mut app, &ctx, true, None);
        assert_eq!(app.own_clipboard_text, None, "nor once the window is back");
    }

    /// A focused blank document with one picture on page 1 that remembers copied text, whose
    /// clipboard writes on this thread succeed without touching the system clipboard.
    fn app_with_picture_and_copied_text() -> PdfCraftApp {
        crate::os_clipboard::fake_writes();
        let mut app = PdfCraftApp::new();
        app.system_clipboard = true;
        app.create_blank();
        let png = crate::create_ui::png_from_rgba(1, 1, &[255, 0, 0, 255]).unwrap();
        app.add_image("picture.png".into(), png);
        app.own_clipboard_text = Some("copied words".into());
        app
    }

    /// Copy Image puts the picture on the clipboard in place of the copied text.
    #[test]
    fn copy_image_forgets_the_copied_text() {
        let mut app = app_with_picture_and_copied_text();
        assert!(app.copy_page_image(0, 0));
        assert_eq!(app.own_clipboard_text, None);
    }

    /// Copy Image in Original Format puts a file on the clipboard in place of the copied text.
    #[test]
    fn copy_image_as_a_file_forgets_the_copied_text() {
        let mut app = app_with_picture_and_copied_text();
        app.copy_page_image_file(0, 0);
        assert!(app.last_copied_file.is_some(), "copied");
        assert_eq!(app.own_clipboard_text, None);
    }

    /// A snapshot puts its picture on the clipboard in place of the copied text.
    #[test]
    fn a_snapshot_forgets_the_copied_text() {
        let mut app = app_with_picture_and_copied_text();
        assert!(app.snapshot(0, 0, [0.0, 0.0, 20.0, 20.0]).is_ok());
        assert_eq!(app.own_clipboard_text, None);
    }

    /// A snapshot is on the clipboard at once, while egui puts the frame's copied text there
    /// after the frame: whichever comes first in the frame, the text is there last and is what
    /// the window remembers.
    #[test]
    fn text_copied_in_a_snapshots_frame_is_on_the_clipboard_last() {
        let mut app = app_with_picture_and_copied_text();
        let ctx = egui::Context::default();
        for snapshot_first in [true, false] {
            app.own_clipboard_text = None;
            ctx.run_ui(egui::RawInput::default(), |ui| {
                if snapshot_first {
                    assert!(app.snapshot(0, 0, [0.0, 0.0, 20.0, 20.0]).is_ok());
                }
                ui.ctx().copy_text("newer words".into());
                if !snapshot_first {
                    assert!(app.snapshot(0, 0, [0.0, 0.0, 20.0, 20.0]).is_ok());
                }
                app.finish_render_frame(ui.ctx());
            })
            .drop_without_applying_deltas();
            assert_eq!(app.own_clipboard_text.as_deref(), Some("newer words"), "snapshot first: {snapshot_first}");
        }
    }
}
