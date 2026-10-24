//! Reopen the files from the last session (Preferences ▸ Documents and view, #442): which files
//! were open when PdfCraft last closed, the page each showed, where on that page, and the active
//! tab. Local only, like
//! the recent-files list, and kept only while the preference is on.

use serde::Serialize;

use crate::PdfCraftApp;

/// More files than anyone keeps open. Settings are untrusted, so a longer list is cut here.
pub const MAX_FILES: usize = 64;

/// How far [`SessionFile::top`] may reach: a page above or below. The window's top is on the
/// current page or, when the page below shows more, on the one before it, so anything further is
/// not from us and is pulled back.
const MAX_TOP: f32 = 1.0;

/// The files open when PdfCraft last closed.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LastSession {
    pub files: Vec<SessionFile>,
    /// The path of the tab that was active.
    pub active: Option<String>,
}

/// One open file and where the reader was in it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionFile {
    /// Absolute, so a file opened from the command line by a relative path is found again.
    pub path: String,
    /// The page the tab showed (0-based).
    pub page: usize,
    /// Where the window's top was on that page, as a fraction of its height
    /// ([`crate::DocView::position_in_page`]). Settings written before #2310 have none: the top.
    pub top: f32,
}

/// `top` as something to scroll to: finite and within [`MAX_TOP`] of the page, else its top.
fn sane_top(top: f32) -> f32 {
    if top.is_finite() { top.clamp(-MAX_TOP, MAX_TOP) } else { 0.0 }
}

impl LastSession {
    /// Read what [`PdfCraftApp::persist`] wrote. Malformed entries are skipped and the list is
    /// capped at [`MAX_FILES`].
    pub(crate) fn from_json(v: &serde_json::Value) -> Self {
        let files = v["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|f| {
                        let path = f["path"].as_str().filter(|p| !p.is_empty())?.to_string();
                        // `go_to_page` clamps a page past the end.
                        let page = f["page"].as_u64().and_then(|p| usize::try_from(p).ok()).unwrap_or(0);
                        let top = sane_top(f["top"].as_f64().unwrap_or(0.0) as f32);
                        Some(SessionFile { path, page, top })
                    })
                    .take(MAX_FILES)
                    .collect()
            })
            .unwrap_or_default();
        Self { files, active: v["active"].as_str().map(str::to_string) }
    }
}

/// `path` made absolute against the current directory, without resolving links (and without
/// Windows' `\\?\` prefix, which `canonicalize` adds).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

impl PdfCraftApp {
    /// The files open now. Documents that were never saved to a file are left out (crash
    /// recovery keeps their unsaved work), and so is everything on the web, which has no paths.
    pub(crate) fn open_session(&self) -> LastSession {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = |v: &crate::DocView| self.session.get(v.id).and_then(|d| d.path.as_deref()).map(absolute);
            let file = |v: &crate::DocView| Some(SessionFile { path: path(v)?, page: v.current, top: sane_top(v.position_in_page()) });
            let files = self.views.iter().filter_map(file).take(MAX_FILES).collect();
            let active = self.active.and_then(|i| self.views.get(i)).and_then(path);
            LastSession { files, active }
        }
        #[cfg(target_arch = "wasm32")]
        LastSession::default()
    }

    /// What to save as the last session: the files open when the quit began, once the app is
    /// really quitting (unsaved tabs close one by one before that), else the files open now.
    pub(crate) fn session_to_save(&self) -> LastSession {
        match &self.quit_session {
            Some(s) if self.allow_quit => s.clone(),
            _ => self.open_session(),
        }
    }

    /// A quit is about to close its first tab: remember everything that is still open.
    pub(crate) fn note_quit_session(&mut self) {
        if self.quit_session.is_none() {
            self.quit_session = Some(self.open_session());
        }
    }

    /// The quit was cancelled: what is open from now on is the session again.
    pub(crate) fn forget_quit_session(&mut self) {
        self.quit_session = None;
    }

    /// At startup, with the preference on: reopen the last session's files at the pages they
    /// showed, scrolled to where they were on them, and bring its active tab forward. Files that are gone are skipped, and so are
    /// `skip` (files the app is about to open anyway, e.g. from the command line) and documents
    /// the Recovery dialog offers, so neither opens twice.
    ///
    /// Only one password prompt can wait at a time: the first encrypted file asks for its
    /// password and later ones stay closed (they are in the recent-files list).
    pub fn reopen_last_files(&mut self, skip: &[String]) {
        let last = std::mem::take(&mut self.last_session);
        #[cfg(not(target_arch = "wasm32"))]
        if self.reopen_last_session {
            let skip: Vec<String> =
                skip.iter().map(|p| absolute(p)).chain(self.recoverable.iter().filter_map(|m| m.path.as_deref().map(absolute))).collect();
            let mut active = None;
            let mut prompt = self.password_prompt.take();
            for f in &last.files {
                let open = |app: &Self| {
                    app.views.iter().any(|v| app.session.get(v.id).and_then(|d| d.path.as_deref()).map(absolute).as_deref() == Some(f.path.as_str()))
                };
                if skip.contains(&f.path) || !std::path::Path::new(&f.path).is_file() || open(self) {
                    continue;
                }
                let before = self.views.len();
                self.open_path(&f.path);
                if let Some(p) = self.password_prompt.take() {
                    prompt.get_or_insert(p);
                }
                if self.views.len() > before
                    && let Some(view) = self.views.last_mut()
                {
                    view.go_to_page(f.page);
                    // `go_to_page` clamped the page to the document. If the page is gone, so is the
                    // place on it, as when a tab is reloaded: the last page shows from its top.
                    let top = if view.current == f.page { f.top } else { 0.0 };
                    view.goto = Some((view.current, top));
                    if last.active.as_ref() == Some(&f.path) {
                        active = Some(self.views.len() - 1);
                    }
                }
            }
            self.password_prompt = prompt;
            if active.is_some() {
                self.active = active;
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (last, skip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_sessions_are_skipped_and_capped() {
        let v = serde_json::json!({
            "files": [{"path": 5}, {"path": ""}, "x", {"path": "/a.pdf", "page": -3, "top": "half"}, {"path": "/b.pdf", "page": 1e300, "top": 1e300}, {"path": "/c.pdf", "page": 7, "top": 0.25}, {"path": "/d.pdf", "top": -40}, {"path": "/e.pdf", "page": 2}],
            "active": 3,
        });
        let s = LastSession::from_json(&v);
        let pages: Vec<(&str, usize, f32)> = s.files.iter().map(|f| (f.path.as_str(), f.page, f.top)).collect();
        assert_eq!(pages, [("/a.pdf", 0, 0.0), ("/b.pdf", 0, 0.0), ("/c.pdf", 7, 0.25), ("/d.pdf", 0, -1.0), ("/e.pdf", 2, 0.0)]);
        assert_eq!(s.active, None);
        let many: Vec<serde_json::Value> = (0..1000).map(|i| serde_json::json!({"path": format!("/{i}.pdf")})).collect();
        assert_eq!(LastSession::from_json(&serde_json::json!({"files": many})).files.len(), MAX_FILES);
        assert_eq!(LastSession::from_json(&serde_json::json!("garbage")), LastSession::default());
    }
}
