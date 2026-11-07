use super::PdfCraftApp;
use super::save::SaveTarget;

/// What the user was doing when we asked whether to save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseRequest {
    /// Close this document's tab. By id, not tab index: tabs can close and shift while a save
    /// panel is open.
    Tab(pdfcraft_engine::DocId),
    /// Quit the application once every dirty document is resolved.
    Quit,
    /// File ▸ Close all: like Quit, but the application stays open.
    All,
}

impl PdfCraftApp {
    /// Close a tab, asking first if it has unsaved changes. A document under OCR review stays
    /// open until the review is accepted or rejected.
    pub fn request_close_tab(&mut self, index: usize) {
        if self.hold_close_for_ocr_review(index) {
            return;
        }
        // Text typed into a field counts as an unsaved change.
        if self.has_unsaved_work(index)
            && let Some(id) = self.views.get(index).map(|v| v.id)
        {
            self.close_request = Some(CloseRequest::Tab(id));
        } else {
            self.close_tab(index);
        }
    }

    /// File ▸ Close all: clean documents close at once; each one with unsaved changes asks.
    /// While a document is under OCR review, nothing closes: the review comes forward first.
    pub fn close_all(&mut self) {
        if self.hold_for_any_ocr_review() {
            return;
        }
        for i in (0..self.views.len()).rev() {
            if !self.has_unsaved_work(i) {
                self.close_tab(i);
            }
        }
        if self.first_dirty().is_some() {
            self.close_request = Some(CloseRequest::All);
        }
    }

    /// A document whose OCR review is still open (a run, or a reading not yet accepted or
    /// rejected) does not close under it: closing would leave the review with nothing to apply
    /// to (#6485). The document comes to the front with its review, and a notice says what to
    /// do. Whether the close was held.
    pub(crate) fn hold_close_for_ocr_review(&mut self, index: usize) -> bool {
        let Some(id) = self.views.get(index).map(|v| v.id) else { return false };
        if !self.ocr_verify.holds_reading_of(id) {
            return false;
        }
        self.active = Some(index);
        self.ocr_verify.open = true;
        self.notify_tr("Accept or reject the OCR review before closing this document");
        true
    }

    /// Close all and Quit close every document, so a document under OCR review holds them too
    /// ([`Self::hold_close_for_ocr_review`]), even when it has no unsaved changes. Whether they
    /// were held.
    fn hold_for_any_ocr_review(&mut self) -> bool {
        let reviewed = self.views.iter().position(|v| self.ocr_verify.holds_reading_of(v.id));
        reviewed.is_some_and(|i| self.hold_close_for_ocr_review(i))
    }

    /// File ▸ Revert (after the confirmation).
    pub fn revert_active(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        match self.session.revert(id) {
            Ok(()) => {
                if let Some(i) = self.active
                    && let Some(d) = self.session.get(id)
                {
                    let view = &mut self.views[i];
                    view.document_changed(&d.info);
                    // Revert discards this source's uncommitted typing as well as model edits.
                    // Otherwise a later Save could write a rejected draft back into the file.
                    view.forms.focus = None;
                    view.forms.committed = None;
                    view.forms.unbaked.clear();
                    view.pending_edit = None;
                }
                self.notify_tr("Reverted to the last saved version");
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The first tab with unsaved changes.
    pub fn first_dirty(&self) -> Option<usize> {
        (0..self.views.len()).find(|&i| self.has_unsaved_work(i))
    }

    /// Answer the save prompt: `Some(true)` save, `Some(false)` discard, `None` cancel.
    pub fn resolve_close(&mut self, ctx: &egui::Context, choice: Option<bool>) {
        let Some(req) = self.close_request.take() else { return };
        // The next question (another dirty document) starts on Save again.
        self.save_prompt_button = Default::default();
        let index = match req {
            CloseRequest::Tab(id) => match self.views.iter().position(|v| v.id == id) {
                Some(i) => i,
                None => return, // already closed
            },
            CloseRequest::Quit | CloseRequest::All => match self.first_dirty() {
                Some(i) => i,
                None => {
                    if req == CloseRequest::Quit {
                        self.quit(ctx);
                    }
                    return;
                }
            },
        };
        // Quitting closes the unsaved tabs one by one (and brings each forward to save it):
        // remember what was open, and which tab was active, before the first one goes (#442).
        if req == CloseRequest::Quit {
            match choice {
                Some(_) => self.note_quit_session(),
                None => self.forget_quit_session(),
            }
        }
        match choice {
            None => {} // cancelled: nothing closes
            Some(false) => self.close_and_continue(ctx, index, req),
            Some(true) => {
                let Some(id) = self.views.get(index).map(|v| v.id) else { return };
                // Bring the document forward: what's typed in its form is committed and saved
                // with it, and a field that rejects the value is shown where the user can fix it.
                self.active = Some(index);
                // Close only once the save has actually been written, which may be on a later
                // frame when the user has to choose where. A failed or cancelled save keeps the
                // document open.
                let ctx = ctx.clone();
                self.save_then(index, SaveTarget::InPlace, move |app| {
                    if let Some(i) = app.views.iter().position(|v| v.id == id) {
                        app.close_and_continue(&ctx, i, req);
                    }
                });
            }
        }
    }

    /// Close tab `index` for a close request, then ask about the next dirty document, or quit.
    pub(super) fn close_and_continue(&mut self, ctx: &egui::Context, index: usize, req: CloseRequest) {
        self.close_tab(index);
        // A newer prompt wins: the user may have started another close while a save was pending,
        // and seen its question, which keeps counting as seen.
        if (req == CloseRequest::Quit || req == CloseRequest::All) && self.close_request.is_none() {
            match self.first_dirty() {
                Some(_) => {
                    // A question about the next document is a new one.
                    self.save_prompt_seen = false;
                    self.close_request = Some(req);
                }
                None if req == CloseRequest::Quit => self.quit(ctx),
                None => {}
            }
        }
    }

    /// Request a normal application exit, preserving every unsaved document until answered.
    /// Returns false while the existing save/discard/cancel prompt needs a decision, or while an
    /// OCR review is still to be accepted or rejected.
    pub(crate) fn request_quit(&mut self, ctx: &egui::Context) -> bool {
        if self.hold_for_any_ocr_review() {
            ctx.request_repaint();
            false
        } else if self.first_dirty().is_some() {
            self.close_request = Some(CloseRequest::Quit);
            ctx.request_repaint();
            false
        } else {
            self.shutdown_recovery();
            self.quit(ctx);
            true
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Publish whether any document has unsaved work, for the browser's reload warning (#812).
    pub(crate) fn sync_unsaved_flag(&self) {
        self.unsaved_flag.store(self.first_dirty().is_some(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Intercept window close while documents have unsaved changes or an OCR review is open.
    pub(crate) fn guard_quit(&mut self, ctx: &egui::Context) {
        // egui's default quit shortcut ignores extra Alt/Shift, including Ctrl+Alt+Q for '@'.
        ctx.options_mut(|o| o.quit_shortcuts.clear());
        let shortcut = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key: egui::Key::Q, pressed: true, modifiers, .. }
                    if modifiers.matches_exact(egui::Modifiers::COMMAND))
            })
        });
        if shortcut {
            self.request_quit(ctx);
        }
        self.sync_unsaved_flag();
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if !self.allow_quit && self.hold_for_any_ocr_review() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        } else if !self.allow_quit && self.first_dirty().is_some() {
            // `first_dirty` counts text typed into a field as an unsaved change.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_request = Some(CloseRequest::Quit);
        } else {
            // A clean quit: nothing is left to recover.
            self.shutdown_recovery();
        }
    }

    /// Ask for the user's attention when the "save changes?" question opens while PdfCraft is in
    /// the background (Quit in the Dock menu, logging out, or an agent quitting), and withdraw the
    /// request once the user comes back or the question is answered.
    ///
    /// The question holds up the quit the user asked for, so the request is critical, as AppKit
    /// makes it for a modal panel that an inactive app presents: the Dock icon bounces until
    /// PdfCraft is active, Windows flashes the taskbar button until the window is in front, and
    /// X11 sets the urgency hint, which stays until it is reset. A question that opens in front
    /// asks nothing, and neither does leaving it open: the user has seen it. Each document asked
    /// about when quitting is a new question, so one that opens in the background asks even after
    /// a question the user saw; a request that still stands carries over to it.
    pub(crate) fn attend_save_prompt(&mut self, ctx: &egui::Context) {
        let open = self.close_request.is_some();
        let focused = ctx.input(|i| i.focused);
        if self.save_prompt_attention {
            if focused || !open {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Reset));
                self.save_prompt_attention = false;
            }
        } else if open && !self.save_prompt_seen && !focused {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
            self.save_prompt_attention = true;
        }
        self.save_prompt_seen = open;
    }
}
