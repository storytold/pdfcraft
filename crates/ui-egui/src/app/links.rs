use super::*;

impl PdfCraftApp {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn open_remote_link(&mut self, source: DocId, file: String, destination: pdfcraft_render::RemoteDestination) {
        if self.pending_remote_link.is_some() || self.pending_link.is_some() {
            return;
        }
        let Some(source_doc) = self.session.get(source) else { return };
        let source_path = source_doc.path.clone();
        let path = match pdfcraft_engine::resolve_remote_document_path(source_path.as_deref().map(std::path::Path::new), &file) {
            Ok(path) => path,
            Err(e) => {
                self.notify_fmt("Couldn't follow linked PDF: {reason}", &[("reason", &e.to_string())]);
                return;
            }
        };
        if pdfcraft_engine::remote_document_path_is_absolute(&file) {
            self.pending_remote_link = Some(PendingRemoteLink { source, path, destination });
            return;
        }
        self.open_resolved_remote_link(path, destination);
    }

    /// Take a pending request exactly once. Closing its source invalidates the request.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolve_pending_remote_link(&mut self, open: bool) {
        if let Some(pending) = self.pending_remote_link.take()
            && open
            && self.session.get(pending.source).is_some()
        {
            self.open_resolved_remote_link(pending.path, pending.destination);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open_resolved_remote_link(&mut self, path: std::path::PathBuf, destination: pdfcraft_render::RemoteDestination) {
        let path = path.to_string_lossy().into_owned();
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        let stamp = file_watch::Stamp::at(&path);
        self.startup_superseded = true;
        let result = self.schedule_open(OpenRequest {
            name: name.clone(),
            path: Some(path.clone()),
            stamp,
            input: open_worker::Input::RemotePath(std::path::PathBuf::from(path)),
            password: None,
            activate: true,
            after_open: Some(destination),
            recovered: None,
            then: None,
        });
        if let Err(error) = result {
            self.notify_fmt("Couldn't follow linked PDF: {reason}", &[("reason", &error)]);
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn open_remote_link(&mut self, _source: DocId, _file: String, _destination: pdfcraft_render::RemoteDestination) {
        self.notify_tr("Local cross-PDF links require the desktop app");
    }

    #[cfg(target_arch = "wasm32")]
    pub fn resolve_pending_remote_link(&mut self, _open: bool) {
        self.pending_remote_link = None;
    }

    pub(super) fn apply_remote_destination(&mut self, destination: pdfcraft_render::RemoteDestination) {
        if let Some(index) = self.views.len().checked_sub(1) {
            self.apply_destination_at(index, destination);
        }
    }

    /// Show `destination` in the tab at `index`: a document just opened from a link or with open
    /// parameters (#3425), or one already open that such a link or launch names. The latest request
    /// for a document wins over a named destination still being looked up.
    pub(super) fn apply_destination_at(&mut self, index: usize, destination: pdfcraft_render::RemoteDestination) {
        let Some(id) = self.views.get(index).map(|view| view.id) else { return };
        #[cfg(not(target_arch = "wasm32"))]
        self.pending_destinations.retain(|pending| pending.doc != id);
        let destination = match destination {
            pdfcraft_render::RemoteDestination::Named(name) => {
                let Some(doc) = self.session.get(id) else { return };
                let (bytes, password) = (doc.bytes.clone(), doc.password.clone());
                // Live, the name tree is read off the UI thread; startup and tests stay inline.
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ctx) = self.ctx.clone().filter(|_| !self.run_inline) {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    let (work_bytes, work_password, work_name) = (bytes.clone(), password.clone(), name.clone());
                    let work = move || {
                        let _ = sender.send(pdfcraft_render::named_destination(work_bytes, work_password.as_deref(), &work_name));
                        ctx.request_repaint();
                    };
                    if std::thread::Builder::new().name("pdfcraft-named-destination".into()).spawn(work).is_ok() {
                        self.pending_destinations.push(PendingDestination { doc: id, name, receiver });
                        return;
                    }
                }
                found_destination(pdfcraft_render::named_destination(bytes, password.as_deref(), &name), name)
            }
            other => other,
        };
        self.show_destination_at(index, destination);
    }

    /// Show each named destination whose lookup has finished, in its document's tab.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn poll_pending_destinations(&mut self) {
        for pending in std::mem::take(&mut self.pending_destinations) {
            let found = match pending.receiver.try_recv() {
                Ok(found) => found,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.pending_destinations.push(pending);
                    continue;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
            };
            if let Some(index) = self.views.iter().position(|view| view.id == pending.doc) {
                self.show_destination_at(index, found_destination(found, pending.name));
            }
        }
    }

    fn show_destination_at(&mut self, index: usize, destination: pdfcraft_render::RemoteDestination) {
        let Some(view) = self.views.get_mut(index) else { return };
        let Some(doc) = self.session.get(view.id) else { return };
        let warning = match destination {
            pdfcraft_render::RemoteDestination::Default => None,
            pdfcraft_render::RemoteDestination::Named(_) => Some("The linked PDF opened, but its named destination was not applied."),
            pdfcraft_render::RemoteDestination::Page { page, view: dest } if page < doc.info.pages.len() => {
                view.go_to_dest(page, dest, &doc.info);
                None
            }
            pdfcraft_render::RemoteDestination::Page { .. } => Some("The linked PDF opened, but its page destination was out of range."),
        };
        if let Some(warning) = warning {
            self.notify_tr(warning);
        }
    }

    /// Open a web link in the system browser (a new `noopener` tab on the web). Only for
    /// PdfCraft's own links; an address that came from a document goes through
    /// [`Self::request_document_url`].
    pub fn open_url(&mut self, url: &str) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(ctx) = &self.ctx {
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
        }
        // eframe's web runner opens new tabs with `window.open(url, "_blank")` and no features,
        // which hands the opened page `window.opener` and lets it navigate this tab (#5084).
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(window) = web_sys::window() {
                // With `noopener` the browser answers `null` instead of the new window, and a
                // blocked pop-up is `null` or an error: there is nothing to keep or retry.
                let _ = window.open_with_url_and_target_and_features(url, "_blank", "noopener,noreferrer");
            }
        }
        self.last_opened_url = Some(url.to_string());
    }

    /// A document asks to open `url` (a link, a button's URI action or `app.launchURL`). Web and
    /// email addresses wait for the user to allow them; anything else is refused with a notice
    /// (#90, #91). While one request is waiting, further ones are dropped, so a script can't
    /// queue up a stream of dialogs.
    pub fn request_document_url(&mut self, url: &str, origin: LinkOrigin) {
        self.request_url(url, origin, None);
    }

    /// The form of document `id` asks to be submitted to `url` (a submit button's action, or a
    /// script's `submitForm`, which `button` doesn't name). PdfCraft never sends form data. An
    /// email address (`mailto:`) is offered as a new message in the user's email app, as an email
    /// link is, after saving the document for them to attach (#7693); any other address is only
    /// explained.
    pub fn request_form_submit(&mut self, id: DocId, url: &str, origin: LinkOrigin, button: Option<&str>) {
        if pdfcraft_engine::links::is_mail_address(url) {
            self.request_url(url, origin, Some(id));
            return;
        }
        match button {
            Some(name) => self.notify_fmt(
                "{name} submits the form to {url}; PdfCraft doesn't send form data. Save the document to keep your entries.",
                &[("name", name), ("url", url)],
            ),
            None => self.notify_fmt(
                "The form asks to be submitted to {u}; PdfCraft doesn't send form data. Save the document to keep your entries.",
                &[("u", url)],
            ),
        }
    }

    fn request_url(&mut self, url: &str, origin: LinkOrigin, submit: Option<DocId>) {
        match pdfcraft_engine::links::document_url(url) {
            Ok(url) => {
                if self.pending_link.is_none() && self.pending_remote_link.is_none() {
                    self.pending_link = Some(PendingLink { url, origin, submit });
                }
            }
            Err(e) => {
                use pdfcraft_engine::links::BlockedLink;
                let template = if matches!(e, BlockedLink::MailFile | BlockedLink::MailEncodedWord) {
                    "{who} in this document tried to open an address PdfCraft won't open: {e}."
                } else {
                    "{who} in this document tried to open an address PdfCraft won't open: {e}. Only web (http, https) and email (mailto) links open from documents."
                };
                self.notify_fmt(template, &[("who", tl!(origin.noun())), ("e", &e.to_string())]);
            }
        }
    }

    /// The user's answer to [`Self::pending_link`]: open it, or drop it.
    pub fn resolve_pending_link(&mut self, open: bool) {
        if let Some(p) = self.pending_link.take()
            && open
        {
            self.open_url(&p.url);
            if let Some(id) = p.submit {
                self.form_email_opened(id, None);
            }
        }
    }

    /// The answer "Save and open email app" to a form's email submission
    /// ([`PendingLink::submit`]): save the document, then open the email, only once it is written
    /// (which may be on a later frame, when the user chooses where). A failed or cancelled save
    /// opens nothing.
    pub fn save_and_resolve_form_email(&mut self) {
        let Some(id) = self.pending_link.as_ref().and_then(|p| p.submit) else { return };
        let Some(p) = self.pending_link.take() else { return };
        let Some(index) = self.asking_tab(id) else { return };
        self.save_then(index, crate::SaveTarget::InPlace, move |app, written| {
            app.open_url(&p.url);
            app.form_email_opened(id, Some(written));
        });
    }

    /// Whether document `id` should be saved before its form is emailed: it has unsaved work (in
    /// any of its tabs), or no file of its own to attach yet: none, an email app's temporary copy,
    /// or a path read back from recovery, where Save asks where to keep it.
    pub(crate) fn form_email_needs_save(&self, id: DocId) -> bool {
        let Some(doc) = self.session.get(id) else { return false };
        let no_file = doc.path.as_deref().is_none_or(crate::editing::is_mail_temp_copy) || self.recovered_paths.contains(&id);
        doc.dirty || no_file || self.views.iter().enumerate().any(|(i, v)| v.id == id && self.has_unsaved_typing(i))
    }

    /// Say what to attach once a form's email has opened. The file isn't attached for the user:
    /// an email address that attaches a local file is what
    /// [`pdfcraft_engine::links::document_url`] refuses. `saved`: the name of the file the document
    /// was just saved to for it, which a web download after Apply redactions names differently
    /// from the document.
    fn form_email_opened(&mut self, id: DocId, saved: Option<&str>) {
        if let Some(written) = saved {
            self.notify_fmt("Attach {name} to the new email before you send it.", &[("name", written)]);
            return;
        }
        let Some(name) = self.session.get(id).map(|d| d.name.clone()) else { return };
        if self.form_email_needs_save(id) {
            self.notify_fmt("Save {name}, then attach it to the new email before you send it.", &[("name", &name)]);
        } else {
            self.notify_fmt("Attach {name} to the new email before you send it.", &[("name", &name)]);
        }
    }
}

/// A named destination's lookup result as a destination to show: its page, or the name again when
/// the document doesn't define it, which shows a warning.
fn found_destination(found: Option<(usize, pdfcraft_render::DestView)>, name: String) -> pdfcraft_render::RemoteDestination {
    match found {
        Some((page, view)) => pdfcraft_render::RemoteDestination::Page { page, view },
        None => pdfcraft_render::RemoteDestination::Named(name),
    }
}

#[cfg(test)]
mod form_email_tests {
    use crate::PdfCraftApp;

    /// A web save after Apply redactions delivers `form_Redacted.pdf` while the document keeps
    /// its name: the notice names the file delivered, so the unredacted original isn't the one
    /// the user is told to attach.
    #[test]
    fn the_attach_notice_names_the_file_written_not_the_document() {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../../tests/data/form.pdf").to_vec()).unwrap();
        let id = app.views[0].id;
        app.form_email_opened(id, Some("form_Redacted.pdf"));
        let said = app.toast.clone().map(|t| t.0).unwrap_or_default();
        assert!(said.contains("Attach form_Redacted.pdf to the new email"), "{said}");
        assert_eq!(app.session.get(id).unwrap().name, "form.pdf", "the document keeps its name");
    }
}
