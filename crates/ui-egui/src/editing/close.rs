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
    /// Close this window (not the main one) once every dirty document in it is resolved.
    Window,
}

impl PdfCraftApp {
    /// Close a tab, asking first if it has unsaved changes.
    pub fn request_close_tab(&mut self, index: usize) {
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
    pub fn close_all(&mut self) {
        for i in (0..self.views.len()).rev() {
            if !self.has_unsaved_work(i) {
                self.close_tab(i);
            }
        }
        if self.first_dirty().is_some() {
            self.close_request = Some(CloseRequest::All);
        }
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
            CloseRequest::Quit | CloseRequest::All | CloseRequest::Window => match self.first_dirty() {
                Some(i) => i,
                None => {
                    match req {
                        CloseRequest::Quit => self.quit(ctx),
                        CloseRequest::Window => self.queue_window_op(crate::WindowOp::Close(self.current_window())),
                        _ => {}
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
        if !matches!(req, CloseRequest::Tab(_)) && self.close_request.is_none() {
            match self.first_dirty() {
                Some(_) => {
                    // A question about the next document is a new one.
                    self.save_prompt_seen = false;
                    self.close_request = Some(req);
                }
                None if req == CloseRequest::Quit => self.quit(ctx),
                None if req == CloseRequest::Window => self.queue_window_op(crate::WindowOp::Close(self.current_window())),
                None => {}
            }
        }
    }

    /// Request a normal application exit, preserving every unsaved document until answered.
    /// Returns false while the existing save/discard/cancel prompt needs a decision.
    pub fn request_quit(&mut self, ctx: &egui::Context) -> bool {
        // Asked from another window: the main window asks, with every window's tabs in it.
        if self.current_window() != crate::WindowId::ROOT {
            self.queue_window_op(crate::WindowOp::MergeAll);
            self.queue_window_op(crate::WindowOp::Quit);
            return false;
        }
        if self.any_unsaved_work() {
            // Every unsaved document is brought forward to save it, so they all come here first.
            self.merge_all_windows();
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
        // The main window's close is the application's, whichever window asked.
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    /// Publish whether any document has unsaved work, for the browser's reload warning (#812).
    pub(crate) fn sync_unsaved_flag(&self) {
        self.unsaved_flag.store(self.first_dirty().is_some(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Intercept the main window's close while documents, in any window, have unsaved changes
    /// (⌘Q is read per window, in `window_logic`).
    pub(crate) fn guard_quit(&mut self, ctx: &egui::Context) {
        // egui's default quit shortcut ignores extra Alt/Shift, including Ctrl+Alt+Q for '@'.
        ctx.options_mut(|o| o.quit_shortcuts.clear());
        self.sync_unsaved_flag();
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        // `any_unsaved_work` counts text typed into a field as an unsaved change.
        if !self.allow_quit && self.any_unsaved_work() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            // Every unsaved document is brought forward to save it, so they all come here first.
            self.merge_all_windows();
            self.close_request = Some(CloseRequest::Quit);
        } else {
            // A clean quit: nothing is left to recover, and the last session remembers every
            // window's files (the other windows go with the app).
            self.merge_all_windows();
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
