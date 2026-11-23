use super::*;
use craft::docpath::names::safe_file_name;
#[cfg(not(target_arch = "wasm32"))]
use craft::host::queue::Priority;
use craft::host::{FileFilter, PickRequest, Picked};
#[cfg(not(target_arch = "wasm32"))]
use craft::platform::watch::FileStamp;
use craft::ui::patterns::form_dialogs::{PasswordDialog, PasswordPurpose};

/// Whether an open's reply asks for a password: the file is encrypted and none, or a wrong one,
/// was given.
#[cfg(not(target_arch = "wasm32"))]
fn asks_for_password(kind: &open_worker::ResultKind) -> bool {
    matches!(
        kind,
        open_worker::ResultKind::Prepared { result, .. }
            if matches!(**result, Err(pdfcraft_render::OpenError::NeedsPassword | pdfcraft_render::OpenError::WrongPassword))
    )
}

/// Whether two paths name the same file: equal, or the same once resolved on disk.
pub(crate) fn same_file(open: &str, path: &str) -> bool {
    craft::platform::open::same_file(std::path::Path::new(open), std::path::Path::new(path), None)
}

impl PdfCraftApp {
    /// Shows `panel` as the right panel, or closes the one showing for `None`.
    pub(crate) fn set_right(&mut self, panel: Option<RightPanel>) {
        use crate::side_panels::{PanelId, set_panel_visible};
        match panel {
            Some(panel) => set_panel_visible(self, PanelId::from(panel), true),
            None => {
                if let Some(open) = self.right() {
                    set_panel_visible(self, PanelId::from(open), false);
                }
            }
        }
    }

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
            let panel = match navigation {
                N::PageOnly => None,
                N::Bookmarks => Some(RightPanel::Bookmarks),
                N::Pages => Some(RightPanel::Pages),
                N::Attachments => Some(RightPanel::Attachments),
                N::Layers => Some(RightPanel::Layers),
            };
            if panel.is_some() {
                self.set_right(panel);
            }
        }
        let view = &mut self.views[index];
        if !self.always_default_view {
            match v.layout {
                // The view opened in Default page display already.
                L::Default => {}
                L::SinglePage => (view.view.display, view.view.continuous) = (canvas::PageDisplay::Single, false),
                L::SinglePageContinuous => (view.view.display, view.view.continuous) = (canvas::PageDisplay::Single, true),
                L::TwoUp => (view.view.display, view.view.continuous) = (canvas::PageDisplay::TwoUp, false),
                L::TwoUpContinuous => (view.view.display, view.view.continuous) = (canvas::PageDisplay::TwoUp, true),
                L::TwoUpCoverPage => {
                    (view.view.display, view.view.continuous, view.view.cover) = (canvas::PageDisplay::TwoUp, false, true);
                }
                L::TwoUpContinuousCoverPage => {
                    (view.view.display, view.view.continuous, view.view.cover) = (canvas::PageDisplay::TwoUp, true, true);
                }
            }
            match v.magnification {
                // The view opened at the default zoom already.
                M::Default => {}
                M::ActualSize => (view.view.zoom, view.view.fit) = (1.0, canvas::Fit::None),
                M::Percent(p) => (view.view.zoom, view.view.fit) = (f64::from(p) / 100.0, canvas::Fit::None),
                M::FitPage | M::FitVisible => view.view.fit = canvas::Fit::Page,
                M::FitWidth => view.view.fit = canvas::Fit::Width,
                M::FitHeight => view.view.fit = canvas::Fit::Height,
            }
        }
        // The opening destination (`/OpenAction`, named destinations resolved): its page, and
        // where on it and how large, as a bookmark or link to it would (ISO 32000-2 Table 149).
        // With the default view kept, only the page.
        let info = self.session.get(view.id).map(|d| &d.info);
        match info.and_then(|i| i.open_dest.filter(|(page, _)| *page < pages).map(|d| (i, d))) {
            Some((info, (page, dest))) if !self.always_default_view => view.go_to_dest(page, dest, info),
            Some((_, (page, _))) => {
                if page > 0 {
                    view.go_to_page(page);
                }
            }
            None if v.page > 0 && v.page < pages => view.go_to_page(v.page),
            None => {}
        }
    }

    /// Open a document and make it the active tab. Encrypted files raise the password prompt.
    pub fn open_bytes(&mut self, ctx: &egui::Context, name: &str, path: Option<String>, bytes: Vec<u8>) -> Result<(), String> {
        self.startup_superseded = true;
        self.open_bytes_with_focus(ctx, name, path, bytes, true)
    }

    pub(super) fn open_bytes_with_focus(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        path: Option<String>,
        bytes: Vec<u8>,
        activate: bool,
    ) -> Result<(), String> {
        // Files opened before the first frame (the ones named at startup) are drawn once, in the
        // interface language, rather than in English and again when the frame applies it.
        self.apply_status_captions(i18n::Lang::from_pref(&self.language));
        // Images and text files become new, unsaved PDFs (Create a PDF).
        let active = self.active;
        if let Some(r) = self.open_converted(name, &bytes) {
            if !activate {
                self.active = active;
            }
            return r;
        }
        self.try_open(ctx, name, path, std::sync::Arc::new(bytes), None, activate)
    }

    /// Hand `request` to the open queue; the result arrives later as a pending open, under the
    /// returned token.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn schedule_open(&mut self, request: OpenRequest) -> u64 {
        let token = self.next_open_token.wrapping_add(1);
        self.next_open_token = token;
        // A new document opens with the remembered Highlight fields, which its scripts read; the
        // session keeps the document in front's.
        let context = self.session.open_context().with_highlight(self.highlight_fields); // PLAN: `highlight_fields` on the app (pdf-canvas-w8)
        let job =
            open_worker::Job { context, name: request.name.clone(), path: request.path.clone(), password: request.password, input: request.input };
        self.opens.submit(token, request.priority, move |cancel| open_worker::run(job, cancel));
        self.pending_opens.push(PendingOpen {
            token,
            name: request.name,
            path: request.path,
            stamp: request.stamp,
            activate: request.activate,
            after_open: request.after_open,
            recovered: request.recovered,
            then: request.then,
            display_override: None,
            ready: None,
            reopen: false,
        });
        token
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn poll_pending_opens(&mut self, ctx: &egui::Context) {
        // The queue delivers in the order jobs finish; each result goes to its entry, so the list
        // keeps the order the files were asked for.
        while let Some(finished) = self.opens.try_recv() {
            if let Some(pending) = self.pending_opens.iter_mut().find(|pending| pending.token == finished.key) {
                pending.ready = Some(finished.result);
            }
        }
        let pending_opens = std::mem::take(&mut self.pending_opens);
        // A retry that is no longer waiting holds nothing back.
        if let Some(token) = self.password_retry
            && !pending_opens.iter().any(|pending| pending.token == token)
        {
            self.password_retry = None;
        }
        let flagged_retry =
            self.password_retry.is_some_and(|token| pending_opens.iter().any(|pending| pending.token == token && pending.display_override.is_some()));
        let mut remaining = pending_opens.into_iter();
        let mut waiting = Vec::new();
        while let Some(mut pending) = remaining.next() {
            // A tool, password prompt or file picker belongs to the document just admitted.
            // Keep later replies queued so another open cannot change its recipient.
            if self.modal_open() || self.picks_pending() {
                waiting.push(pending);
                waiting.extend(remaining);
                break;
            }
            let retry = self.password_retry == Some(pending.token);
            // Startup display overrides settle in request order, including a password retry.
            if flagged_retry && !retry && pending.display_override.is_some() {
                waiting.push(pending);
                continue;
            }
            let Some(kind) = pending.ready.take() else {
                waiting.push(pending);
                continue;
            };
            // While a closed prompt's password is being tried, a later file that asks for its own
            // password keeps its reply: its prompt would hide the retry's outcome. Others admit.
            if !retry && self.password_retry.is_some() && asks_for_password(&kind) {
                pending.ready = Some(kind);
                waiting.push(pending);
                continue;
            }
            if retry {
                self.password_retry = None;
            }
            self.finish_pending_open(ctx, pending, kind);
        }
        // A completion may schedule another open; preserve those requests as well as the
        // earlier waiting replies, without consuming a blocked receiver's result.
        waiting.append(&mut self.pending_opens);
        self.pending_opens = waiting;
        self.settle_launch_layout();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn finish_pending_open(&mut self, ctx: &egui::Context, pending: PendingOpen, kind: open_worker::ResultKind) {
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
                        self.execute(ctx, command);
                    }
                }
                Some(Err(error)) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error)]),
                None => {}
            },
            open_worker::ResultKind::Converted(created) => match self.open_converted_pdf(&pending.name, created) {
                Ok(()) => {
                    if let Some(command) = pending.then {
                        self.execute(ctx, command);
                    }
                }
                Err(error) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error)]),
            },
            open_worker::ResultKind::Prepared { bytes, result } => match *result {
                Ok(prepared) => match self.finish_open(ctx, &pending.name, pending.path.clone(), bytes.len(), prepared, pending.activate) {
                    Ok(id) => {
                        if let Some(path) = pending.path.as_deref() {
                            self.watch.known_as(id, std::path::Path::new(path), pending.stamp);
                            if pending.reopen {
                                self.forget_closed_tab(path);
                            }
                        }
                        if let Some(destination) = pending.after_open {
                            self.apply_remote_destination(destination);
                        }
                        if let Some((layout, continuous)) = pending.display_override
                            && let Some(view) = self.views.iter_mut().find(|view| view.id == id)
                        {
                            (view.view.display, view.view.continuous) = (layout, continuous);
                            self.launch_display_target = Some(id);
                        }
                        if let Some(meta) = &pending.recovered {
                            self.finish_recovery(meta);
                        }
                        if let Some(command) = pending.then {
                            self.execute(ctx, command);
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
                        error: password_error,
                        activate: pending.activate,
                        recovered: pending.recovered,
                        then: pending.then,
                        after_open: pending.after_open,
                        display_override: pending.display_override,
                    });
                }
                Err(error) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &pending.name), ("e", &error.to_string())]),
            },
        }
    }

    /// Open `bytes` on this thread: ask for a password if the document needs one, else add it
    /// as a tab (in front when `activate`).
    fn try_open(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        path: Option<String>,
        bytes: std::sync::Arc<Vec<u8>>,
        password: Option<&str>,
        activate: bool,
    ) -> Result<(), String> {
        use pdfcraft_render::OpenError;
        // A new document opens with the remembered Highlight fields, which its scripts read; the
        // session keeps the document in front's.
        let context = self.session.open_context().with_highlight(self.highlight_fields);
        let prepared = match context.prepare_open(name, path.clone(), bytes.clone(), password) {
            Ok(prepared) => prepared,
            Err(e @ (OpenError::NeedsPassword | OpenError::WrongPassword)) => {
                let error = matches!(e, OpenError::WrongPassword).then(|| "Incorrect password. Try again.".to_string());
                #[cfg(not(target_arch = "wasm32"))]
                let stamp = path.as_deref().and_then(|path| FileStamp::at(std::path::Path::new(path)));
                self.password_prompt = Some(PasswordPrompt {
                    name: name.to_string(),
                    path,
                    bytes,
                    #[cfg(not(target_arch = "wasm32"))]
                    stamp,
                    error,
                    activate,
                    recovered: None,
                    then: None,
                    after_open: None,
                    display_override: None,
                });
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };
        self.finish_open(ctx, name, path, bytes.len(), prepared, activate).map(|_| ())
    }

    /// Add a prepared document as a tab: its view, the panels it opens with, a portfolio's file list
    /// and initial document, the recent-files entry, and what its scripts said while it opened (at
    /// once in front, or when a background tab is first selected).
    fn finish_open(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        path: Option<String>,
        size: usize,
        prepared: pdfcraft_engine::PreparedDocument,
        activate: bool,
    ) -> Result<DocId, String> {
        let id = self.session.commit_open(prepared);
        self.password_prompt = None;
        // A tab opening in front leaves a portfolio's tab, whose file list gives the panel back:
        // the document a portfolio presents opens with the panels any other would.
        if activate {
            self.leave_portfolio();
        }
        let right_before = self.right();
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
        // A portfolio presents the document its `/D` names (ISO 32000-2 Table 153), kept by its
        // name-tree key, which an edit meanwhile can't shift as it can an index.
        let portfolio_initial =
            doc.info.portfolio.as_ref().and_then(|p| p.initial).and_then(|i| doc.info.attachments.get(i)).and_then(|a| match &a.source {
                pdfcraft_render::AttachmentSource::Document { key } => Some(key.clone()),
                pdfcraft_render::AttachmentSource::Annotation { .. } => None,
            });
        let opens_with = if activate && self.right().is_none() && !self.right_panel_closed && opening_mode != Mode::Read {
            if !doc.info.annotations.is_empty() {
                Some(RightPanel::Comments)
            // A full-screen document names its panel for outside full screen, and with
            // `/NonFullScreenPageMode /UseNone` (the default) no outline is visible (Table 147).
            } else if !doc.info.outline.is_empty() && !initial.full_screen {
                Some(RightPanel::Bookmarks)
            } else {
                None
            }
        } else {
            None
        };
        let view = DocView::new(id, &doc.info, self.view_defaults, self.highlight_fields); // PLAN: assumed signature (pdf-canvas-w8)
        if opens_with.is_some() {
            self.set_right(opens_with);
        }
        self.views.push(view);
        if activate {
            self.active = Some(self.views.len() - 1);
        }
        self.apply_initial_view(self.views.len() - 1, &initial);
        self.apply_viewer_preferences(self.views.len() - 1, &initial, activate);
        // Where the reader left off wins over the file's own initial view and the zoom its
        // viewer preferences open at (#1767).
        self.apply_last_view(self.views.len() - 1);
        // After all of them, so full screen shows one fitted page and leaving it comes back to them.
        if self.full_screen()
            && let Some(view) = self.views.last_mut()
        {
            view.enter_full_screen(self.full_screen_fill);
        }
        // Full screen is asked for, never taken: a document can't put the app in full screen on
        // its own (the prompt is `ask_full_screen`'s).
        if activate && initial.full_screen {
            self.ask_full_screen(id);
        }
        if activate {
            // A portfolio's tab shows its files from the start, in place of the panel there was
            // before it opened, whichever panel its page mode asked for.
            if opening_mode != Mode::Read && self.lists_files(id) {
                self.show_portfolio_files(id, right_before);
            }
            // A document opening in front of a portfolio that hasn't presented its initial
            // document yet (files opened together) keeps the front.
            self.portfolio_initial = portfolio_initial.map(|key| (id, key));
        }
        if activate
            && self.left_open()
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
            let left_open = self.left_open();
            self.select_mode(ctx, self.default_mode);
            if self.left_open() != left_open {
                self.set_left_open(left_open);
            }
        }
        if let Some(p) = path {
            #[cfg(target_os = "macos")]
            macos_association::repair(std::path::Path::new(&p));
            self.recent.retain(|r| r.path != p);
            self.recent_check.opened(&p);
            self.recent.insert(0, RecentFile { name: name.to_string(), path: p, pages, size });
            self.recent.truncate(self.recent_limit);
        }
        // What the form's scripts said while it opened (messages, errors) shows now, not with
        // the next edit.
        // Background documents keep their script output until their tab is selected: a print
        // or save request must never act on the current document instead.
        if activate {
            let out = self.session.take_js_output(id);
            self.handle_js(ctx, id, out);
        } else if let Some(on) = self.session.take_js_highlight(id) {
            // The field highlight a form asks for isn't about the current document: now.
            self.script_highlight(id, on);
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
            let target = self.pick_target(self.active_ids().map(|(_, id)| id));
            self.pick_first(PickRequest::open("attachment").title(tl!("Attach a file").to_string()), target, |app, ctx, path| {
                app.attach_file(ctx, &path)
            });
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Attaching files is not available on the web yet");
    }

    /// Attach the file at `path` to the active document (document level: the Attachments
    /// panel, not a paper-clip comment on a page).
    pub fn attach_file(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return;
            }
        };
        let mime = pdfcraft_engine::mime_for_name(&name).map(str::to_string);
        if self.apply_edit(ctx, pdfcraft_engine::Edit::AddAttachment { name, bytes, description: None, mime }) {
            self.set_right(Some(RightPanel::Attachments));
        }
    }

    /// Save an attachment to disk, or open a PDF attachment in a new tab (or go to the tab it is
    /// already open in). Only bytes that are a PDF open, whatever the file's name says; anything
    /// else asked to open is offered to save.
    pub fn attachment_action(&mut self, ctx: &egui::Context, doc: DocId, index: usize, open: bool) {
        let Some(d) = self.session.get(doc) else { return };
        let Some(att) = d.info.attachments.get(index).cloned() else { return };
        let data = pdfcraft_render::attachment_data(&d.bytes, d.password.as_deref(), &att);
        #[cfg(not(target_arch = "wasm32"))]
        let source = d.path.clone();
        match (data, open) {
            (Err(e), _) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]),
            (Ok(bytes), true) if looks_like_pdf(&bytes) => {
                // The tab name later names files written from the tab (Split, Extract pages).
                let name = safe_file_name(&att.name, "attachment");
                // Already open: the tab opened from this attachment of this document (its bytes
                // may have grown since, as a form laid out on open appends its layout).
                let origin = (doc, att.name.clone());
                let open_in = self.views.iter().position(|v| v.from_attachment.as_ref() == Some(&origin) && self.session.get(v.id).is_some());
                if let Some(i) = open_in {
                    self.active = Some(i);
                    return;
                }
                let before = self.views.len();
                match self.open_bytes(ctx, &name, None, bytes) {
                    // Opening bytes adds their tab at once (unless a password is asked for first).
                    Ok(()) if self.views.len() > before => {
                        if let Some(v) = self.views.last_mut() {
                            v.from_attachment = Some(origin);
                        }
                    }
                    Ok(()) => {}
                    Err(e) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]),
                }
            }
            (Ok(bytes), _) => {
                #[cfg(not(target_arch = "wasm32"))]
                self.pick_first(PickRequest::save("attachment").file_name(safe_file_name(&att.name, "attachment")), None, move |app, _, path| {
                    match app.host.files().write(&path, &bytes, craft::host::WriteIntent::SaveAs) {
                        Ok(taken_by_host) => {
                            // Windows: the file keeps the document's Mark-of-the-Web (#3990). Best
                            // effort: a volume without alternate data streams can't carry it.
                            if !taken_by_host {
                                let _ = pdfcraft_platform::zone::mark_from(&path, source.as_deref().map(std::path::Path::new));
                            }
                            app.notify_fmt("Saved {name}", &[("name", &path.display().to_string())])
                        }
                        Err(e) => app.notify_fmt("Couldn't save: {e}", &[("e", &e.to_string())]),
                    }
                });
                #[cfg(target_arch = "wasm32")]
                self.notify_fmt("Downloading attachments on the web arrives with M3.10 ({n} bytes ready)", &[("n", &bytes.len().to_string())]);
            }
        }
    }

    /// A PDF portfolio (ISO 32000-2 §7.11.6) shows its documents, not the cover sheet that its
    /// pages are. The document a portfolio that just opened presents first (`/D`) opens in a
    /// tab of its own, and a portfolio's tab shows its file list, the Attachments panel, even
    /// with the right panel closed, unless the portfolio keeps the list hidden (`/View /H`).
    /// Leaving the tab gives back the panel the list replaced, unless the reader changed panels
    /// meanwhile. Called every frame.
    pub(crate) fn present_portfolios(&mut self, ctx: &egui::Context) {
        // A prompt or picker over the window belongs to the document it was raised for (the
        // password of a file opened together with the portfolio, say): the initial document
        // waits for its answer, and opening it then would close the prompt. A file still
        // opening in the background (in the open worker, or a browser read of a drop or a
        // picked file) comes to the front when it arrives, so it is waited for too.
        // On the web a drop or pick arrives as bytes and opens at once, so nothing is in flight.
        #[cfg(not(target_arch = "wasm32"))]
        let busy = self.modal_open() || self.picks_pending() || self.pending_opens.iter().any(|p| p.activate);
        #[cfg(target_arch = "wasm32")]
        let busy = self.modal_open();
        if !busy
            && let Some((doc, key)) = self.portfolio_initial.take()
            && self.active_ids().is_some_and(|(_, id)| id == doc)
            && let Some(index) = self.portfolio_attachment(doc, &key)
        {
            self.open_portfolio_initial(ctx, doc, index);
            // A portfolio inside the portfolio waits until the reader opens it, so a nested
            // file can't open tab after tab on its own.
            self.portfolio_initial = None;
        }
        let active = self.active_ids().map(|(_, id)| id);
        if active == self.portfolio_seen {
            return;
        }
        self.leave_portfolio();
        if let Some(id) = active
            && self.mode != Mode::Read
            && self.right() != Some(RightPanel::Attachments)
            && self.lists_files(id)
        {
            self.show_portfolio_files(id, self.right());
        }
        self.portfolio_seen = active;
    }

    /// The document is a portfolio whose tab shows its file list: one that doesn't keep the
    /// list hidden (`/View /H`).
    fn lists_files(&self, id: DocId) -> bool {
        self.session.get(id).and_then(|d| d.info.portfolio.as_ref()).is_some_and(|p| p.view != pdfcraft_render::PortfolioView::Hidden)
    }

    /// Show portfolio `id`'s file list in place of `before`, the panel given back when its tab
    /// is left.
    fn show_portfolio_files(&mut self, id: DocId, before: Option<RightPanel>) {
        self.portfolio_seen = Some(id);
        self.portfolio_panel = Some(before);
        self.set_right(Some(RightPanel::Attachments));
    }

    /// Where in `doc`'s attachments now the file its name tree keys `key` is: an attachment
    /// deleted since the portfolio opened moves it, or it is gone.
    fn portfolio_attachment(&self, doc: DocId, key: &str) -> Option<usize> {
        self.session
            .get(doc)?
            .info
            .attachments
            .iter()
            .position(|a| matches!(&a.source, pdfcraft_render::AttachmentSource::Document { key: k } if k == key))
    }

    /// Open portfolio `doc`'s attachment `index` in a tab of its own when its bytes are a PDF,
    /// whatever its name. A file of another kind, or one that can't be read, stays in the list.
    fn open_portfolio_initial(&mut self, ctx: &egui::Context, doc: DocId, index: usize) {
        let Some(d) = self.session.get(doc) else { return };
        let Some(att) = d.info.attachments.get(index).cloned() else { return };
        let Ok(bytes) = pdfcraft_render::attachment_data(&d.bytes, d.password.as_deref(), &att) else { return };
        if pdfcraft_engine::source_kind(&att.name, &bytes) != Some(pdfcraft_engine::SourceKind::Pdf) {
            return;
        }
        if let Err(e) = self.open_bytes(ctx, &safe_file_name(&att.name, "attachment"), None, bytes) {
            self.notify_fmt("Couldn't open {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]);
        }
    }

    /// Leave a portfolio's tab: the panel its file list replaced comes back, unless the reader
    /// changed panels meanwhile. In Read mode, which put the list away and gives it back on
    /// leaving, it is the panel Read mode gives back that changes, and a panel the reader picked
    /// in Read mode stays.
    fn leave_portfolio(&mut self) {
        self.portfolio_seen = None;
        let Some(before) = self.portfolio_panel.take() else { return };
        if self.mode == Mode::Read {
            if self.right_before_read == Some(RightPanel::Attachments) {
                self.right_before_read = before;
            }
        } else if self.right() == Some(RightPanel::Attachments) {
            self.set_right(before);
        }
    }

    /// [`Self::attachment_action`] for the file of the attachment comment at (page, index).
    pub fn comment_attachment_action(&mut self, ctx: &egui::Context, doc: DocId, page: usize, index: usize, open: bool) {
        let source = pdfcraft_render::AttachmentSource::Annotation { page, index };
        // The menus offer the items only for a listed file, so a miss is a comment gone since.
        if let Some(i) = self.session.get(doc).and_then(|d| d.info.attachments.iter().position(|a| a.source == source)) {
            self.attachment_action(ctx, doc, i, open);
        }
    }

    /// Shows the password dialog for the open prompt, else the permissions prompt, and answers the
    /// prompt on the pass the dialog closes. Called by the window body every pass while either is
    /// set.
    pub(crate) fn password_pass(&mut self, ui: &mut egui::Ui) {
        let (purpose, error, unlock) = if let Some(prompt) = &self.password_prompt {
            (PasswordPurpose::open(prompt.name.clone()), prompt.error.clone(), false)
        } else if let Some(prompt) = &self.unlock_prompt {
            (PasswordPurpose::permissions(prompt.name.clone()), prompt.error.clone(), true)
        } else {
            return;
        };
        let mut dialog = PasswordDialog::new(purpose);
        if let Some(error) = error {
            dialog = dialog.error(craft::ui::runtime::env::Label::from(error));
        }
        // Open while a prompt waits: a wrong password sets the prompt again, which opens the dialog again.
        let mut open = true;
        let output = dialog.opened(craft::ui::contract::Bind::bound(&mut open)).show(ui);
        let password = match (output.answer, output.closed) {
            (Some(answer), _) => Some(answer.secret.expose().to_string()),
            (None, Some(_)) => None,
            (None, None) => return,
        };
        if unlock {
            self.submit_unlock(ui.ctx(), password);
        } else {
            self.submit_password(ui.ctx(), password);
        }
    }

    /// Answer the password prompt (`None` cancels).
    pub fn submit_password(&mut self, ctx: &egui::Context, password: Option<String>) {
        let Some(mut p) = self.password_prompt.take() else { return };
        let Some(pw) = password.map(zeroize::Zeroizing::new) else {
            // A cancelled recovery goes back on offer; its snapshot stays in the store and must
            // not attach itself to whatever document is unlocked next (#813).
            if let Some(meta) = p.recovered.take() {
                self.recovered.push(meta);
            }
            #[cfg(not(target_arch = "wasm32"))]
            self.settle_launch_layout();
            return;
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let token = self.schedule_open(OpenRequest {
                name: p.name.clone(),
                path: p.path.clone(),
                stamp: p.stamp,
                input: open_worker::Input::Bytes(p.bytes.clone()),
                password: Some(pw),
                activate: p.activate,
                after_open: p.after_open.clone(),
                recovered: p.recovered.clone(),
                then: p.then,
                priority: Priority::VISIBLE,
            });
            if let Some(pending) = self.pending_opens.iter_mut().find(|pending| pending.token == token) {
                pending.display_override = p.display_override;
            }
            // Replies behind it that ask for a password wait for its outcome (`poll_pending_opens`).
            self.password_retry = Some(token);
        }
        #[cfg(target_arch = "wasm32")]
        self.retry_inline(ctx, p, pw);
    }

    /// The web has no queue: the retry opens the held bytes at once.
    #[cfg(target_arch = "wasm32")]
    fn retry_inline(&mut self, ctx: &egui::Context, mut p: PasswordPrompt, pw: zeroize::Zeroizing<String>) {
        let after_open = p.after_open.clone();
        let opened = self.try_open(&p.name, p.path.take(), p.bytes.clone(), Some(&pw), p.activate);
        drop(pw);
        match opened {
            Err(e) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &p.name), ("e", &e.to_string())]),
            // A recovered encrypted document is open once its prompt is gone.
            Ok(()) if self.password_prompt.is_none() => {
                if let Some(destination) = after_open {
                    self.apply_remote_destination(destination);
                }
                if let Some((layout, continuous)) = p.display_override
                    && let Some(view) = self.views.last_mut()
                {
                    (view.view.display, view.view.continuous) = (layout, continuous);
                }
                if let Some(meta) = &p.recovered {
                    self.finish_recovery(meta);
                }
                if let Some(command) = p.then {
                    self.execute(ctx, command);
                }
            }
            // Wrong password: the new prompt is for the same bytes, so it keeps the snapshot
            // and the tool waiting for it.
            Ok(()) => {
                if let Some(prompt) = self.password_prompt.as_mut() {
                    prompt.recovered = p.recovered.take();
                    prompt.then = p.then;
                    prompt.after_open = after_open;
                    prompt.display_override = p.display_override;
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
    pub fn submit_unlock(&mut self, ctx: &egui::Context, password: Option<String>) {
        let Some(mut p) = self.unlock_prompt.take() else { return };
        let Some(mut pw) = password else { return };
        let unlocked = self.session.unlock(p.doc, &pw);
        zeroize::Zeroize::zeroize(&mut pw);
        match unlocked {
            // Carry on with what asked for the password, if its document is still the one shown.
            Ok(()) => match p.then {
                Some(command) if self.active_ids().is_some_and(|(_, id)| id == p.doc) => {
                    self.execute(ctx, command);
                }
                _ => self.notify_tr("Restrictions lifted"),
            },
            Err(pdfcraft_engine::EditError::WrongPassword) => {
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
        self.open_path_request(path, None, None)
    }

    /// Keep a chosen tool with its file until preparation and any password entry finish. The
    /// return value is the programmatic open contract (#1620): callers that must know whether
    /// the file opened, focused its tab, was handed to the background worker, or failed.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn open_path_then(&mut self, ctx: &egui::Context, path: &str, then: Option<&'static str>) -> OpenOutcome {
        let outcome = self.open_path_request(path, then, None);
        // A file that was open already is focused at once, and the tool it was opened for runs on it.
        if let (OpenOutcome::Focused, Some(command)) = (&outcome, then) {
            self.execute(ctx, command);
        }
        outcome
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open_path_request(&mut self, path: &str, then: Option<&'static str>, display_override: Option<(canvas::PageDisplay, bool)>) -> OpenOutcome {
        // `manual.pdf#nameddest=Intro`, as a help system starts a PDF reader (#3425): the file,
        // shown at that destination once open. A path that names a file is never split.
        match pdfcraft_engine::open_parameters::split(path, |p| std::path::Path::new(p).is_file()) {
            Some((file, destination)) => self.open_file_request(&file, Some(destination), then, display_override),
            None => self.open_file_request(path, None, then, display_override),
        }
    }

    /// Open the file `path`, shown at `after_open` once open. A file that waits for room in the
    /// worker's queue keeps its destination with it, as its path is the file alone (#3425).
    #[cfg(not(target_arch = "wasm32"))]
    fn open_file_request(
        &mut self,
        path: &str,
        after_open: Option<pdfcraft_render::RemoteDestination>,
        then: Option<&'static str>,
        display_override: Option<(canvas::PageDisplay, bool)>,
    ) -> OpenOutcome {
        // One file, one tab (#2166): a file that is open already comes to the front instead,
        // and the tool it was opened for runs on it. One still being opened is left to that.
        if let Some(i) = self.tab_showing(path) {
            self.startup_superseded = true;
            self.active = Some(i);
            if let Some((layout, continuous)) = display_override
                && let Some(view) = self.views.get_mut(i)
            {
                (view.view.display, view.view.continuous) = (layout, continuous);
                self.launch_display_target = Some(view.id);
            }
            if let Some(destination) = after_open {
                self.apply_destination_at(i, destination);
            }
            return OpenOutcome::Focused;
        }
        if let Some(pending) = self.pending_opens.iter_mut().find(|p| p.path.as_deref().is_some_and(|p| same_file(p, path))) {
            if display_override.is_some() {
                pending.display_override = display_override;
            }
            // Asked for another place before it finished opening: it opens at the latest one.
            if after_open.is_some() {
                pending.after_open = after_open;
            }
            return OpenOutcome::Pending;
        }
        // The same file named again while its password is being asked for joins that prompt
        // instead of queuing a second one; a display override, a place to open at or a tool it
        // brings is kept.
        if let Some(prompt) = self.password_prompt.as_mut().filter(|p| p.path.as_deref().is_some_and(|p| same_file(p, path))) {
            if display_override.is_some() {
                prompt.display_override = display_override;
            }
            if after_open.is_some() {
                prompt.after_open = after_open;
            }
            if then.is_some() {
                prompt.then = then;
            }
            return OpenOutcome::Pending;
        }
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
        // Taken before reading: a build that rewrites the file meanwhile still counts as a change.
        let stamp = FileStamp::at(std::path::Path::new(path));
        self.startup_superseded = true;
        let token = self.schedule_open(OpenRequest {
            name,
            path: Some(path.to_string()),
            stamp,
            input: open_worker::Input::Path(std::path::PathBuf::from(path)),
            password: None,
            activate: true,
            after_open,
            recovered: None,
            then,
            priority: Priority::NEAR,
        });
        if let Some(pending) = self.pending_opens.iter_mut().find(|pending| pending.token == token) {
            pending.display_override = display_override;
        }
        OpenOutcome::Pending
    }

    /// Reopen `path` from the last session: shown at `place` once open, and brought to the front only when `front`.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn reopen_path(&mut self, path: &str, place: pdfcraft_render::RemoteDestination, front: bool) {
        if !matches!(self.open_file_request(path, Some(place), None, None), OpenOutcome::Pending) {
            return;
        }
        if let Some(pending) = self.pending_opens.iter_mut().find(|p| p.path.as_deref().is_some_and(|p| same_file(p, path))) {
            pending.activate = front;
        }
    }

    /// Apply an explicit launch layout after the command-line display overrides settle.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_launch_layout(&mut self, value: &str) -> Result<(), String> {
        let (layout, continuous) = canvas::parse_display_token(value).ok_or("layout must be single, continuous, two-up or two-page")?;
        self.launch_layout = Some((layout, continuous));
        // The option may arrive before any flagged open; don't consume it before it has a target.
        if self.launch_display_target.is_some() {
            self.settle_launch_layout();
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn settle_launch_layout(&mut self) {
        if self.pending_opens.iter().any(|open| open.display_override.is_some())
            || self.password_prompt.as_ref().is_some_and(|prompt| prompt.display_override.is_some())
        {
            return;
        }
        // Follow the existing completion/activation order, not a guessed tab index or path.
        // Failed or cancelled opens cannot cause the layout to land on a restored document.
        if let Some((layout, continuous)) = self.launch_layout.take()
            && let Some(id) = self.launch_display_target
            && let Some(view) = self.views.iter_mut().find(|view| view.id == id)
        {
            (view.view.display, view.view.continuous) = (layout, continuous);
        }
    }

    /// Open one native file with a transient display override, including after a password prompt.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path_with_display(&mut self, path: &str, layout: canvas::PageDisplay, continuous: bool) {
        self.open_path_request(path, None, Some((layout, continuous)));
    }

    /// The File ▸ Open picker's file types, in order; the first, PDF, is the default. Images, text
    /// and (on the desktop, through LibreOffice) Office documents open as new, unsaved PDFs.
    /// No "All files" in the browser or on macOS, whose panel merges every type into one list
    /// rather than offering a choice.
    pub(crate) fn open_filters() -> Vec<FileFilter> {
        let mut filters = vec![FileFilter::new("PDF", &["pdf"]), FileFilter::new(tl!("Images").to_string(), &pdfcraft_engine::IMAGE_EXTENSIONS)];
        #[cfg(not(target_arch = "wasm32"))]
        filters.push(FileFilter::new(tl!("Office documents").to_string(), pdfcraft_engine::OFFICE_EXTENSIONS));
        filters.push(FileFilter::new(tl!("Text").to_string(), &pdfcraft_engine::TEXT_EXTENSIONS));
        #[cfg(not(any(target_arch = "wasm32", target_os = "macos")))]
        filters.push(FileFilter::new(tl!("All files").to_string(), &["*"]));
        filters
    }

    /// The File ▸ Open request: [`Self::open_filters`], one `filter` per type.
    fn open_request() -> PickRequest {
        Self::open_filters().iter().fold(PickRequest::open_many("document"), |request, filter| {
            let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
            request.filter(&filter.name, &extensions)
        })
    }

    pub fn open_dialog(&mut self) {
        self.startup_superseded = true;
        // The web answers `Picked::Contents`, the desktop `Picked::Paths`.
        self.pick_file(Self::open_request(), None, |app, ctx, picked| match picked {
            #[cfg(not(target_arch = "wasm32"))]
            Picked::Paths(paths) => {
                for path in paths {
                    app.open_path(&path.to_string_lossy());
                }
            }
            Picked::Contents(files) => {
                for (name, bytes) in files {
                    if let Err(error) = app.open_bytes(ctx, &name, None, bytes) {
                        app.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                    }
                }
            }
            _ => {}
        });
    }

    /// Opens what a picker answered and runs the command `then` on each opened document, as a tool
    /// chosen from Home does once its file is ready.
    pub fn open_picked_then(&mut self, ctx: &egui::Context, picked: Picked, then: &'static str) {
        self.startup_superseded = true;
        match picked {
            #[cfg(not(target_arch = "wasm32"))]
            Picked::Paths(paths) => {
                for path in paths {
                    self.open_path_then(ctx, &path.to_string_lossy(), Some(then));
                }
            }
            Picked::Contents(files) => {
                for (name, bytes) in files {
                    match self.open_bytes(ctx, &name, None, bytes) {
                        Ok(()) => {
                            self.execute(ctx, then);
                        }
                        Err(error) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]),
                    }
                }
            }
            _ => {}
        }
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.views.len() {
            return;
        }
        self.remember_view(index);
        let view = self.views.remove(index);
        let (id, place) = (view.id, view.place());
        // Another tab showing the same document (split view) keeps it open.
        if !self.views.iter().any(|v| v.id == id) {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(path) = self.session.get(id).and_then(|d| d.path.clone()) {
                if self.closed_tabs.len() == 10 {
                    self.closed_tabs.remove(0);
                }
                self.closed_tabs.push(path);
            }
            self.placed_signatures.remove(&id);
            self.forget_recovery(id);
            self.session.close(id);
        }
        self.active = match self.active {
            _ if self.views.is_empty() => None,
            Some(a) if a > index => Some(a - 1),
            // The tab after it in its strip comes forward (or the last).
            Some(a) if a == index => self.after_close(place, index),
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

/// Whether `bytes` are a PDF: a `%PDF-` header within the first 1024 bytes, as readers accept.
fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes.get(..1024).unwrap_or(bytes).windows(5).any(|w| w == b"%PDF-")
}
