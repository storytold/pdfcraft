use super::*;

/// Whether `a` and `b` name the same file: equal, or equal once made absolute against the
/// current directory (links are not resolved, as in `last_session`).
pub(crate) fn same_file(a: &str, b: &str) -> bool {
    a == b || matches!((std::path::absolute(a), std::path::absolute(b)), (Ok(x), Ok(y)) if x == y)
}

impl PdfCraftApp {
    /// Open a document the way it asks to be opened: navigation panel, layout, magnification,
    /// page and the place on it (Document Properties ▸ Initial View, `/OpenAction`). With Always
    /// use my default zoom and page layout on, the layout and magnification stay at the defaults
    /// the view opened with, and the view goes to the opening page's top.
    fn apply_initial_view(&mut self, index: usize, v: &pdfcraft_engine::InitialView) {
        use pdfcraft_engine::{InitialLayout as L, Magnification as M, Navigation as N};
        let pages = self.session.get(self.views[index].id).map_or(0, |d| d.info.pages.len());
        // A right panel the user closed stays closed, whatever the document asks for (#2884).
        // A document that asks for full screen gets the panel it names for outside it
        // (`/NonFullScreenPageMode`, Table 147), there when the prompt is declined or full
        // screen is left.
        let navigation = if v.full_screen { v.non_full_screen } else { v.navigation };
        if self.active == Some(index) && !self.right_panel_closed {
            match navigation {
                N::PageOnly => {}
                N::Bookmarks => self.right = Some(RightPanel::Bookmarks),
                N::Pages => self.right = Some(RightPanel::Pages),
                N::Attachments => self.right = Some(RightPanel::Attachments),
                N::Layers => self.right = Some(RightPanel::Layers),
            }
        }
        let view = &mut self.views[index];
        if !self.always_default_view {
            match v.layout {
                // The view opened in Default page display already.
                L::Default => {}
                L::SinglePage => (view.layout, view.continuous) = (canvas::PageLayout::Single, false),
                L::SinglePageContinuous => (view.layout, view.continuous) = (canvas::PageLayout::Single, true),
                L::TwoUp => (view.layout, view.continuous) = (canvas::PageLayout::TwoUp, false),
                L::TwoUpContinuous => (view.layout, view.continuous) = (canvas::PageLayout::TwoUp, true),
                L::TwoUpCoverPage => {
                    (view.layout, view.continuous, view.cover) = (canvas::PageLayout::TwoUp, false, true);
                }
                L::TwoUpContinuousCoverPage => {
                    (view.layout, view.continuous, view.cover) = (canvas::PageLayout::TwoUp, true, true);
                }
            }
            match v.magnification {
                // The view opened at the default zoom already.
                M::Default => {}
                M::ActualSize => view.set_zoom(1.0),
                M::Percent(p) => view.set_zoom((p / 100.0) as f32),
                M::FitPage | M::FitVisible => view.fit = canvas::Fit::Page,
                M::FitWidth => view.fit = canvas::Fit::Width,
                M::FitHeight => view.fit = canvas::Fit::Height,
            }
        }
        // The opening destination (`/OpenAction`, named destinations resolved): its page, and
        // where on it and how large, as a bookmark or link to it would (ISO 32000-2 Table 149).
        // With the default view kept, only the page.
        let info = self.session.get(view.id).map(|d| &d.info);
        match info.and_then(|i| i.open_dest.filter(|(page, _)| *page < pages).map(|d| (i, d))) {
            Some((info, (page, dest))) if !self.always_default_view => view.go_to_dest(page, dest, info),
            Some((_, (page, _))) if page > 0 => view.go_to_page(page),
            None if v.page > 0 && v.page < pages => view.go_to_page(v.page),
            _ => {}
        }
    }

    /// Open a document and make it the active tab. Encrypted files raise the password prompt.
    pub fn open_bytes(&mut self, name: &str, path: Option<String>, bytes: Vec<u8>) -> Result<(), String> {
        self.startup_superseded = true;
        self.open_bytes_with_focus(name, path, bytes, true)
    }

    pub(super) fn open_bytes_with_focus(&mut self, name: &str, path: Option<String>, bytes: Vec<u8>, activate: bool) -> Result<(), String> {
        // Images and text files become new, unsaved PDFs (Create a PDF).
        let active = self.active;
        if let Some(r) = self.open_converted(name, &bytes) {
            if !activate {
                self.active = active;
            }
            return r;
        }
        self.try_open(name, path, std::sync::Arc::new(bytes), None, activate)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn schedule_open(&mut self, request: OpenRequest) -> Result<u64, String> {
        if self.open_worker.is_none() {
            self.open_worker = Some(
                open_worker::OpenWorker::new(
                    #[cfg(test)]
                    self.open_prepare_gate.clone(),
                )
                .map_err(|e| format!("couldn't start the document-open worker: {e}"))?,
            );
        }
        let token = self.next_open_token.wrapping_add(1);
        let worker = self.open_worker.as_ref().ok_or("the document-open worker is unavailable")?;
        if let Some(ctx) = &self.ctx {
            worker.set_context(ctx);
        }
        let receiver =
            worker.submit(token, self.session.open_context(), request.name.clone(), request.path.clone(), request.password, request.input)?;
        self.next_open_token = token;
        self.pending_opens.push(PendingOpen {
            token,
            receiver,
            name: request.name,
            path: request.path,
            stamp: request.stamp,
            activate: request.activate,
            after_open: request.after_open,
            recovered: request.recovered,
            then: request.then,
        });
        Ok(token)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn poll_pending_opens(&mut self) {
        let pending_opens = std::mem::take(&mut self.pending_opens);
        let mut remaining = pending_opens.into_iter();
        let mut waiting = Vec::new();
        while let Some(pending) = remaining.next() {
            // A tool, password prompt or file picker belongs to the document just admitted.
            // Keep later replies queued so another open cannot change its recipient.
            if self.modal_open() || self.pickers.pending() {
                waiting.push(pending);
                waiting.extend(remaining);
                break;
            }
            match pending.receiver.try_recv() {
                Ok(result) if result.token == pending.token => self.finish_pending_open(pending, result.kind),
                Ok(_) => waiting.push(pending),
                Err(std::sync::mpsc::TryRecvError::Empty) => waiting.push(pending),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.notify_fmt("Couldn't open {name}: the background worker stopped", &[("name", &pending.name)]);
                }
            }
        }
        // A completion may schedule another open; preserve those requests as well as the
        // earlier waiting replies, without consuming a blocked receiver's result.
        waiting.append(&mut self.pending_opens);
        self.pending_opens = waiting;
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn finish_pending_open(&mut self, pending: PendingOpen, kind: open_worker::ResultKind) {
        match kind {
            open_worker::ResultKind::ReadFailed(reason) => {
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &pending.name), ("e", &reason)]);
            }
            open_worker::ResultKind::RemoteFailed(reason) => {
                self.notify_fmt("Couldn't follow linked PDF: {reason}", &[("reason", &reason)]);
            }
            open_worker::ResultKind::RecoveryFailed(reason) => {
                self.notify_fmt("Couldn't recover {name}: {e}", &[("name", &pending.name), ("e", &reason)]);
            }
            open_worker::ResultKind::Failed(reason) => {
                self.notify_fmt("Couldn't open {name}: {reason}", &[("name", &pending.name), ("reason", &reason)]);
            }
            open_worker::ResultKind::Conversion(bytes) => match self.open_converted(&pending.name, &bytes) {
                Some(Ok(())) => {
                    if let Some(command) = pending.then {
                        self.execute(command);
                    }
                }
                Some(Err(error)) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error)]),
                None => {}
            },
            open_worker::ResultKind::Prepared { bytes, result } => match *result {
                Ok(prepared) => match self.finish_open(&pending.name, pending.path.clone(), bytes.len(), prepared, pending.activate) {
                    Ok(id) => {
                        if pending.path.is_some() {
                            self.watch.known_as(id, pending.stamp);
                        }
                        if let Some(destination) = pending.after_open {
                            self.apply_remote_destination(destination);
                        }
                        if let Some(meta) = &pending.recovered {
                            self.finish_recovery(meta);
                        }
                        if let Some(command) = pending.then {
                            self.execute(command);
                        }
                    }
                    Err(error) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error)]),
                },
                Err(error @ (pdfcraft_render::OpenError::NeedsPassword | pdfcraft_render::OpenError::WrongPassword)) => {
                    let password_error =
                        matches!(error, pdfcraft_render::OpenError::WrongPassword).then(|| "Incorrect password. Try again.".to_string());
                    self.password_prompt = Some(PasswordPrompt {
                        name: pending.name,
                        path: pending.path,
                        bytes,
                        stamp: pending.stamp,
                        input: String::new(),
                        error: password_error,
                        activate: pending.activate,
                        recovered: pending.recovered,
                        then: pending.then,
                        after_open: pending.after_open,
                    });
                }
                Err(error) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error.to_string())]),
            },
        }
    }

    fn try_open(
        &mut self,
        name: &str,
        path: Option<String>,
        bytes: std::sync::Arc<Vec<u8>>,
        password: Option<&str>,
        activate: bool,
    ) -> Result<(), String> {
        use pdfcraft_render::OpenError;
        let prepared = match self.session.prepare_open(name, path.clone(), bytes.clone(), password) {
            Ok(prepared) => prepared,
            Err(e @ (OpenError::NeedsPassword | OpenError::WrongPassword)) => {
                let error = matches!(e, OpenError::WrongPassword).then(|| "Incorrect password. Try again.".to_string());
                #[cfg(not(target_arch = "wasm32"))]
                let stamp = path.as_deref().and_then(file_watch::Stamp::at);
                self.password_prompt = Some(PasswordPrompt {
                    name: name.to_string(),
                    path,
                    bytes,
                    #[cfg(not(target_arch = "wasm32"))]
                    stamp,
                    input: String::new(),
                    error,
                    activate,
                    recovered: None,
                    then: None,
                    after_open: None,
                });
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };
        self.finish_open(name, path, bytes.len(), prepared, activate).map(|_| ())
    }

    fn finish_open(
        &mut self,
        name: &str,
        path: Option<String>,
        size: usize,
        prepared: pdfcraft_engine::PreparedDocument,
        activate: bool,
    ) -> Result<DocId, String> {
        let id = self.session.commit_open(prepared);
        self.password_prompt = None;
        // The reading-colours preference is app-wide: a newly opened document starts with it.
        // Idempotent: the engine returns `false` when the document has the setting already.
        self.session.set_reading_colors(id, self.reading_colors);
        let doc = self.session.get(id).ok_or("the document could not be opened")?;
        let pages = doc.info.pages.len();
        // Acrobat opens straight to the Comments panel when a document has comments.
        // Read mode opens with the panes closed; the rail still opens them (#932). A right panel
        // the user closed stays closed (#1590).
        let opening_mode = self.mode_override.unwrap_or(self.default_mode);
        let initial = doc.initial_view();
        if activate && self.right.is_none() && !self.right_panel_closed && opening_mode != Mode::Read {
            self.right = if !doc.info.annotations.is_empty() {
                Some(RightPanel::Comments)
            // A full-screen document names its panel for outside full screen, and with
            // `/NonFullScreenPageMode /UseNone` (the default) no outline is visible (Table 147).
            } else if !doc.info.outline.is_empty() && !initial.full_screen {
                Some(RightPanel::Bookmarks)
            } else {
                None
            };
        }
        let view = DocView::new(id, &doc.info, self.view_defaults);
        self.views.push(view);
        if activate {
            self.active = Some(self.views.len() - 1);
        }
        self.apply_initial_view(self.views.len() - 1, &initial);
        // Where the reader left off wins over the file's own initial view (#1767).
        self.apply_last_view(self.views.len() - 1);
        // After both, so full screen shows one fitted page and leaving it comes back to them.
        if self.full_screen
            && let Some(view) = self.views.last_mut()
        {
            view.enter_presentation();
        }
        // Full screen is asked for, never taken: a document can't put the app in full screen on
        // its own (the prompt is `dialogs::full_screen_prompt`).
        if activate && initial.full_screen {
            self.pending_full_screen = Some(id);
        }
        if activate
            && self.left_open
            && self.left == LeftPanel::Tool("organize")
            && let Some(view) = self.views.last_mut()
        {
            view.open_organize();
        }
        // The document shows, not the expanded All tools view over it (whichever workspace opens).
        if activate {
            self.all_tools_full = false;
        }
        if activate && let Some(mode) = self.mode_override {
            // Explicit mode options do not reset independent --tool / --left choices.
            self.mode = mode;
        } else if activate && self.mode != self.default_mode {
            // Switch workspace like the mode bar, but a left panel the user (or `--left closed`)
            // closed stays closed, and an unchanged mode keeps the tool panel the user chose.
            let left_open = self.left_open;
            self.select_mode(self.default_mode);
            self.left_open = left_open;
        }
        if let Some(p) = path {
            #[cfg(target_os = "macos")]
            macos_association::repair(std::path::Path::new(&p));
            self.recent.retain(|r| r.path != p);
            self.recent.insert(0, RecentFile { name: name.to_string(), path: p, pages, size });
            self.recent.truncate(12);
        }
        // What the form's scripts said while it opened (messages, errors) shows now, not with
        // the next edit.
        // Background documents keep their script output until their tab is selected: a print
        // or save request must never act on the current document instead.
        if activate {
            let out = self.session.take_js_output(id);
            self.handle_js(id, out);
        }
        Ok(id)
    }

    /// Edit ▸ Attach a file…: choose a file, then attach it to the active document and show
    /// the Attachments panel with it.
    pub fn attach_file_dialog(&mut self) {
        if self.active_ids().is_none() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Attach a file").to_string());
            let target = self.active_ids().map(|(_, id)| id);
            self.ask_one(pickers::Ask::File(dialog), target, |app, path| app.attach_file(&path));
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Attaching files is not available on the web yet");
    }

    /// Attach the file at `path` to the active document (document level: the Attachments
    /// panel, not a paper-clip comment on a page).
    pub fn attach_file(&mut self, path: &std::path::Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return;
            }
        };
        let mime = pdfcraft_engine::mime_for_name(&name).map(str::to_string);
        if self.apply_edit(pdfcraft_engine::Edit::AddAttachment { name, bytes, description: None, mime }) {
            self.choose_right_panel(Some(RightPanel::Attachments));
        }
    }

    /// Save an attachment to disk, or open a PDF attachment in a new tab.
    pub fn attachment_action(&mut self, doc: DocId, index: usize, open: bool) {
        let Some(d) = self.session.get(doc) else { return };
        let Some(att) = d.info.attachments.get(index).cloned() else { return };
        let data = pdfcraft_render::attachment_data(&d.bytes, d.password.as_deref(), &att);
        match (data, open) {
            (Err(e), _) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]),
            (Ok(bytes), true) => {
                // The tab name later names files written from the tab (Split, Extract pages).
                if let Err(e) = self.open_bytes(&crate::files::attachment_file_name(&att.name), None, bytes) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]);
                }
            }
            (Ok(bytes), false) => {
                #[cfg(not(target_arch = "wasm32"))]
                self.ask_one(
                    pickers::Ask::Save(rfd::AsyncFileDialog::new().set_file_name(crate::files::attachment_file_name(&att.name))),
                    None,
                    move |app, path| match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                        Ok(()) => app.notify_fmt("Saved {name}", &[("name", &path.display().to_string())]),
                        Err(e) => app.notify_fmt("Couldn't save: {e}", &[("e", &e.to_string())]),
                    },
                );
                #[cfg(target_arch = "wasm32")]
                self.notify_fmt("Downloading attachments on the web arrives with M3.10 ({n} bytes ready)", &[("n", &bytes.len().to_string())]);
            }
        }
    }

    /// [`Self::attachment_action`] for the file of the attachment comment at (page, index).
    pub fn comment_attachment_action(&mut self, doc: DocId, page: usize, index: usize, open: bool) {
        let source = pdfcraft_render::AttachmentSource::Annotation { page, index };
        // The menus offer the items only for a listed file, so a miss is a comment gone since.
        if let Some(i) = self.session.get(doc).and_then(|d| d.info.attachments.iter().position(|a| a.source == source)) {
            self.attachment_action(doc, i, open);
        }
    }

    /// Answer the password prompt (`None` cancels).
    pub fn submit_password(&mut self, password: Option<String>) {
        let Some(p) = self.password_prompt.take() else { return };
        let Some(pw) = password else {
            // A cancelled recovery goes back on offer; its snapshot stays in the store and must
            // not attach itself to whatever document is unlocked next (#813).
            if let Some(meta) = p.recovered {
                self.recoverable.push(meta);
            }
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.ctx.is_some() && !self.run_inline {
            let result = self.schedule_open(OpenRequest {
                name: p.name.clone(),
                path: p.path.clone(),
                stamp: p.stamp,
                input: open_worker::Input::Bytes(p.bytes.clone()),
                password: Some(pw),
                activate: p.activate,
                after_open: p.after_open.clone(),
                recovered: p.recovered.clone(),
                then: p.then,
            });
            if let Err(error) = result {
                let mut prompt = p;
                prompt.error = Some(error);
                self.password_prompt = Some(prompt);
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let (path_for_watch, stamp_for_watch) = (p.path.clone(), p.stamp);
        let after_open = p.after_open.clone();
        match self.try_open(&p.name, p.path, p.bytes, Some(&pw), p.activate) {
            Err(e) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &p.name), ("e", &e.to_string())]),
            // A recovered encrypted document is open once its prompt is gone.
            Ok(()) if self.password_prompt.is_none() => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(path) = path_for_watch.as_deref()
                    && let Some(id) = self
                        .views
                        .iter()
                        .rev()
                        .find_map(|view| self.session.get(view.id).and_then(|doc| (doc.path.as_deref() == Some(path)).then_some(view.id)))
                {
                    self.watch.known_as(id, stamp_for_watch);
                }
                if let Some(destination) = after_open {
                    self.apply_remote_destination(destination);
                }
                if let Some(meta) = &p.recovered {
                    self.finish_recovery(meta);
                }
                if let Some(command) = p.then {
                    self.execute(command);
                }
            }
            // Wrong password: the new prompt is for the same bytes, so it keeps the snapshot
            // and the tool waiting for it.
            Ok(()) => {
                if let Some(prompt) = self.password_prompt.as_mut() {
                    prompt.recovered = p.recovered;
                    prompt.then = p.then;
                    prompt.after_open = after_open;
                }
            }
        }
    }

    /// Ask for the active document's permissions password, then run `then` (if any).
    pub(crate) fn request_unlock(&mut self, then: Option<&'static str>) {
        let Some(doc) = self.active_ids().and_then(|(_, id)| self.session.get(id)) else { return };
        self.unlock_prompt = Some(UnlockPrompt::new(doc, then));
    }

    /// Answer the permissions password prompt (`None` cancels).
    pub fn submit_unlock(&mut self, password: Option<String>) {
        let Some(mut p) = self.unlock_prompt.take() else { return };
        let Some(pw) = password else { return };
        match self.session.unlock(p.doc, &pw) {
            // Carry on with what asked for the password, if its document is still the one shown.
            Ok(()) => match p.then {
                Some(command) if self.active_ids().is_some_and(|(_, id)| id == p.doc) => {
                    self.execute(command);
                }
                _ => self.notify_tr("Restrictions lifted"),
            },
            Err(pdfcraft_engine::EditError::WrongPassword) => {
                p.input.clear();
                p.error = Some("Incorrect password. Try again.".to_string());
                self.unlock_prompt = Some(p);
            }
            Err(e) => self.notify_fmt("Couldn't unlock {name}: {e}", &[("name", &p.name), ("e", &e.to_string())]),
        }
    }

    /// The tab already showing the file at `path` (the same path, or one that names the same
    /// file once made absolute).
    pub(crate) fn tab_showing(&self, path: &str) -> Option<usize> {
        self.views.iter().position(|v| self.session.get(v.id).and_then(|d| d.path.as_deref()).is_some_and(|p| same_file(p, path)))
    }

    /// Open a file from a recent list: focus the tab already showing it, else open it (File ▸
    /// Open Recent and the Home view's list share this).
    pub fn open_recent(&mut self, path: &str) {
        if let Some(i) = self.tab_showing(path) {
            self.active = Some(i);
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            self.open_path(path);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = path;
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path(&mut self, path: &str) -> OpenOutcome {
        self.open_path_then(path, None)
    }

    /// Keep a chosen tool with its file until preparation and any password entry finish. The
    /// return value is the programmatic open contract (#1620): callers that must know whether
    /// the file opened, focused its tab, was handed to the background worker, or failed.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn open_path_then(&mut self, path: &str, then: Option<&'static str>) -> OpenOutcome {
        // One file, one tab (#2166): a file that is open already comes to the front instead,
        // and the tool it was opened for runs on it. One still being opened is left to that.
        if let Some(i) = self.tab_showing(path) {
            self.startup_superseded = true;
            self.active = Some(i);
            if let Some(command) = then {
                self.execute(command);
            }
            return OpenOutcome::Focused;
        }
        if self.pending_opens.iter().any(|p| p.path.as_deref().is_some_and(|p| same_file(p, path))) {
            return OpenOutcome::Pending;
        }
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
        // Taken before reading: a build that rewrites the file meanwhile still counts as a change.
        let stamp = file_watch::Stamp::at(path);
        // Before the first frame there is no interaction to keep responsive; this also preserves
        // deterministic inline startup and test flows. Once live, reading and preparation use
        // the bounded worker and commit on the UI thread.
        if self.ctx.is_none() || self.run_inline {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let before = self.views.len();
                    if let Err(error) = self.open_bytes(&name, Some(path.to_string()), bytes) {
                        self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                        return OpenOutcome::Failed;
                    }
                    if let Some(prompt) = self.password_prompt.as_mut().filter(|p| p.path.as_deref() == Some(path)) {
                        // Encrypted: the tool runs once the password is in.
                        prompt.then = then;
                    } else if self.views.len() > before {
                        if let Some(doc) = self.views.last().map(|view| view.id) {
                            self.watch.known_as(doc, stamp);
                        }
                        if let Some(command) = then {
                            self.execute(command);
                        }
                    } else {
                        // Open reported success but added no tab and raised no prompt; the
                        // control channel answers this as a failure to open.
                        return OpenOutcome::Failed;
                    }
                    OpenOutcome::Opened
                }
                Err(error) => {
                    self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &error.to_string())]);
                    OpenOutcome::Failed
                }
            }
        } else {
            self.startup_superseded = true;
            match self.schedule_open(OpenRequest {
                name: name.clone(),
                path: Some(path.to_string()),
                stamp,
                input: open_worker::Input::Path(std::path::PathBuf::from(path)),
                password: None,
                activate: true,
                after_open: None,
                recovered: None,
                then,
            }) {
                Ok(_) => OpenOutcome::Pending,
                Err(error) => {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                    OpenOutcome::Failed
                }
            }
        }
    }

    /// The File ▸ Open picker: PDFs, and images and text that become PDFs.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn open_picker() -> rfd::AsyncFileDialog {
        rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).add_filter(tl!("Images and text (converted to PDF)"), &create_ui::CONVERTIBLE)
    }

    pub fn open_dialog(&mut self) {
        self.startup_superseded = true;
        #[cfg(not(target_arch = "wasm32"))]
        self.pick(
            pickers::PickFor::Open,
            rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).add_filter(tl!("Images and text (converted to PDF)"), &create_ui::CONVERTIBLE),
            true,
        );
        // Browsers pick files asynchronously; the bytes arrive through `inbox`.
        #[cfg(target_arch = "wasm32")]
        {
            let inbox = self.inbox.clone();
            let ctx = self.ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(h) = rfd::AsyncFileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .add_filter(tl!("Images and text"), &create_ui::CONVERTIBLE)
                    .pick_file()
                    .await
                {
                    let bytes = h.read().await;
                    if let Ok(mut q) = inbox.lock() {
                        q.push((h.file_name(), bytes));
                    }
                    // The read may finish while the app is idle: wake it to open the file.
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                }
            });
        }
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.views.len() {
            return;
        }
        self.remember_view(index);
        let id = self.views.remove(index).id;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = self.session.get(id).and_then(|d| d.path.clone()) {
            if self.closed_tabs.len() == 10 {
                self.closed_tabs.remove(0);
            }
            self.closed_tabs.push(path);
        }
        self.forget_recovery(id);
        self.session.close(id);
        self.active = match self.active {
            _ if self.views.is_empty() => None,
            Some(a) if a > index => Some(a - 1),
            Some(a) if a >= self.views.len() => Some(self.views.len() - 1),
            other => other,
        };
    }

    /// Moves the tab at `from` so it sits at `to` (#1537). The same document stays active
    /// wherever it lands; an index out of range changes nothing.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.views.len() || to >= self.views.len() || from == to {
            return;
        }
        let active = self.active_ids().map(|(_, id)| id);
        let view = self.views.remove(from);
        self.views.insert(to, view);
        if let Some(id) = active {
            self.active = self.views.iter().position(|v| v.id == id);
        }
    }
}
