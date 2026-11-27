//! Reopen the files from the last session (Preferences ▸ Documents and view, #442): which files
//! were open when PdfCraft last closed, the page each showed and the active tab. Local only, like
//! the recent-files list, and kept only while the preference is on.

use craft::platform::session::{self, MAX_DOCUMENTS, Session, SessionDocument};

use crate::PdfCraftApp;

impl PdfCraftApp {
    /// The files open now. Documents that were never saved to a file are left out (crash
    /// recovery keeps their unsaved work), and so is everything on the web, which has no paths.
    pub(crate) fn open_session(&self) -> Session {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // PLAN: assumed `DocView::current(&self) -> usize` (pdf-canvas-w11)
            let path = |v: &crate::DocView| self.session.get(v.id).and_then(|d| d.path.as_deref()).map(session::absolute);
            let documents = self
                .first_views()
                .filter_map(|v| Some(SessionDocument { path: path(v)?, window: None, view: serde_json::json!({ "page": v.current() }) }))
                .take(MAX_DOCUMENTS)
                .collect();
            let active = self.active.and_then(|i| self.views.get(i)).and_then(path);
            Session { documents, active }
        }
        #[cfg(target_arch = "wasm32")]
        Session::default()
    }

    /// What to save as the last session: the files open when the quit began, once the app is
    /// really quitting (unsaved tabs close one by one before that), else the files open now.
    pub(crate) fn session_to_save(&self) -> Session {
        self.session_keeper.to_save(self.quitting(), self.open_session())
    }

    /// At startup, with the preference on: reopen the last session's files at the pages they
    /// showed and bring its active tab forward. Files that are gone are skipped, and so are
    /// `skip` (files the app is about to open anyway, e.g. from the command line) and documents
    /// the Recovery dialog offers, so neither opens twice.
    ///
    /// Only one password prompt can wait at a time: the first encrypted file asks for its
    /// password and later ones stay closed (in the recent-files list, if there is one).
    pub fn reopen_last_files(&mut self, skip: &[String]) {
        let last = std::mem::take(&mut self.last_session);
        #[cfg(not(target_arch = "wasm32"))]
        if self.reopen_last_session {
            let skip: Vec<String> = skip
                .iter()
                .cloned()
                .chain(self.recovered.iter().filter_map(|m| m.meta.original.as_ref().map(|p| p.to_string_lossy().into_owned())))
                .collect();
            let is_open = |path: &str| {
                self.views.iter().any(|v| self.session.get(v.id).and_then(|d| d.path.as_deref()).map(session::absolute).as_deref() == Some(path))
            };
            // The opens are queued: each takes the page it showed with it, and only the session's active file comes
            // to the front.
            for document in last.to_reopen(&skip, is_open) {
                let page = document.view["page"].as_u64().and_then(|p| usize::try_from(p).ok()).unwrap_or(0);
                let place = pdfcraft_render::RemoteDestination::Page { page, view: pdfcraft_render::DestView::Top };
                let front = last.active.as_deref().is_none_or(|active| active == document.path);
                self.reopen_path(&document.path, place, front);
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (last, skip);
    }
}
