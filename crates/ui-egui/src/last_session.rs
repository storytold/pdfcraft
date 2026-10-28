//! Reopen the files from the last session (Preferences ▸ Documents and view, #442): which files
//! were open when PdfCraft last closed, the page each showed and the active tab. Local only, like
//! the recent-files list, and kept only while the preference is on.

use serde::Serialize;

use crate::PdfCraftApp;

/// More files than anyone keeps open. Settings are untrusted, so a longer list is cut here.
pub const MAX_FILES: usize = 64;

/// The files open when PdfCraft last closed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LastSession {
    pub files: Vec<SessionFile>,
    /// The path of the tab that was active.
    pub active: Option<String>,
}

/// One open file and where the reader was in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionFile {
    /// Absolute, so a file opened from the command line by a relative path is found again.
    pub path: String,
    /// The page the tab showed (0-based).
    pub page: usize,
    /// Vertical scroll offset in points, remembering where the reader was (#3555).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_y: Option<u32>,
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
                        let scroll_y = f["scroll_y"].as_u64().and_then(|y| u32::try_from(y).ok());
                        Some(SessionFile { path, page, scroll_y })
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
fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

impl PdfCraftApp {
    /// The files open now. Documents that were never saved to a file are left out (crash
    /// recovery keeps their unsaved work), and so is everything on the web, which has no paths.
    pub(crate) fn open_session(&self) -> LastSession {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = |v: &crate::DocView| self.session.get(v.id).and_then(|d| d.path.as_deref()).map(absolute);
            let files = self
                .views
                .iter()
                .filter_map(|v| {
                    let p = path(v)?;
                    let y = (v.last_scroll.y > 1.0 && v.last_scroll.y.is_finite()).then_some(v.last_scroll.y as u32);
                    Some(SessionFile { path: p, page: v.current, scroll_y: y })
                })
                .take(MAX_FILES)
                .collect();
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
    /// showed and bring its active tab forward. Files that are gone are skipped, and so are
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
                    if let Some(y) = f.scroll_y {
                        let h = view.page_heights.get(f.page).copied().unwrap_or(800.0).max(1.0);
                        view.goto = Some((f.page, (y as f32 / h).clamp(0.0, 1.0)));
                    }
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
            "files": [{"path": 5}, {"path": ""}, "x", {"path": "/a.pdf", "page": -3}, {"path": "/b.pdf", "page": 1e300}, {"path": "/c.pdf", "page": 7}],
            "active": 3,
        });
        let s = LastSession::from_json(&v);
        let pages: Vec<(&str, usize)> = s.files.iter().map(|f| (f.path.as_str(), f.page)).collect();
        assert_eq!(pages, [("/a.pdf", 0), ("/b.pdf", 0), ("/c.pdf", 7)]);
        assert_eq!(s.active, None);
        let many: Vec<serde_json::Value> = (0..1000).map(|i| serde_json::json!({"path": format!("/{i}.pdf")})).collect();
        assert_eq!(LastSession::from_json(&serde_json::json!({"files": many})).files.len(), MAX_FILES);
        assert_eq!(LastSession::from_json(&serde_json::json!("garbage")), LastSession::default());
    }

    #[test]
    fn scroll_position_is_preserved_in_session() {
        let v = serde_json::json!({
            "files": [{"path": "/a.pdf", "page": 2, "scroll_y": 1420}],
            "active": "/a.pdf",
        });
        let s = LastSession::from_json(&v);
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].scroll_y, Some(1420));
        assert_eq!(s.files[0].page, 2);
    }
}
