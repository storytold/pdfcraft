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
    Text,
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

    /// ⌘C / ⌘X / ⌘V copy, cut and paste text or images on canvas (unless a text field has focus).
    pub(super) fn clipboard_keys(&mut self, i: usize, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let pastes: Vec<_> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Paste(_) => Some(ClipboardPaste::Text),
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

        if copy
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
            let system_has_text = if !has_override_image && self.system_clipboard { crate::os_clipboard::has_text() } else { false };
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

    fn receive_pastes(&mut self, view: usize, pastes: Vec<ClipboardPaste>, images_allowed: bool) {
        let page = self.views.get(view).map_or(0, |view| view.current);
        for paste in pastes {
            match paste {
                ClipboardPaste::Text => {
                    // Text fields may own the paste; the canvas cache still observes its contents.
                    self.last_copied_image = None;
                    self.last_copied_file = None;
                }
                ClipboardPaste::Image(image) if images_allowed => {
                    self.paste_received_image(page, &image);
                }
                ClipboardPaste::Image(_) => {}
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
