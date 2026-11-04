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
        let Some(index) = self.views.len().checked_sub(1) else { return };
        let Some(doc) = self.session.get(self.views[index].id) else { return };
        let warning = match destination {
            pdfcraft_render::RemoteDestination::Default => None,
            pdfcraft_render::RemoteDestination::Named(_) => Some("The linked PDF opened, but its named destination was not applied."),
            pdfcraft_render::RemoteDestination::Page { page, view } if page < doc.info.pages.len() => {
                self.views[index].go_to_dest(page, view, &doc.info);
                None
            }
            pdfcraft_render::RemoteDestination::Page { .. } => Some("The linked PDF opened, but its page destination was out of range."),
        };
        if let Some(warning) = warning {
            self.notify_tr(warning);
        }
    }

    /// Open a web link in the system browser (a new tab on the web). Only for PdfCraft's own
    /// links; an address that came from a document goes through [`Self::request_document_url`].
    pub fn open_url(&mut self, url: &str) {
        if let Some(ctx) = &self.ctx {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        self.last_opened_url = Some(url.to_string());
    }

    /// A document asks to open `url` (a link, a button's URI action or `app.launchURL`). Web and
    /// email addresses wait for the user to allow them; anything else is refused with a notice
    /// (#90, #91). While one request is waiting, further ones are dropped, so a script can't
    /// queue up a stream of dialogs.
    pub fn request_document_url(&mut self, url: &str, origin: LinkOrigin) {
        match pdfcraft_engine::links::document_url(url) {
            Ok(url) => {
                if self.pending_link.is_none() && self.pending_remote_link.is_none() {
                    self.pending_link = Some(PendingLink { url, origin });
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
        }
    }
}
