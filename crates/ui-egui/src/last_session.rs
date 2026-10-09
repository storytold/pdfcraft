//! Reopen the files from the last session (Preferences ▸ Documents and view, #442): which files
//! were open when PdfCraft last closed, the page each showed and the active tab. Local only, like
//! the recent-files list, and kept only while the preference is on.

use serde::Serialize;

use crate::PdfCraftApp;

/// More files than anyone keeps open. Settings are untrusted, so a longer list is cut here.
pub const MAX_FILES: usize = 64;

/// The files open when PdfCraft last closed: the main window's, and the other windows'.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LastSession {
    pub files: Vec<SessionFile>,
    /// The path of the tab that was active.
    pub active: Option<String>,
    /// The main window's place on screen: x, y, width, height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<[f32; 4]>,
    /// The windows besides the main one. (Settings from before windows have none: the main
    /// window's `files` are the whole session.)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<LastWindow>,
}

/// One window besides the main one.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LastWindow {
    pub files: Vec<SessionFile>,
    pub active: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<[f32; 4]>,
}

/// One open file and where the reader was in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionFile {
    /// Absolute, so a file opened from the command line by a relative path is found again.
    pub path: String,
    /// The page the tab showed (0-based).
    pub page: usize,
}

impl LastSession {
    /// Read what [`PdfCraftApp::persist`] wrote. Malformed entries are skipped and the list is
    /// capped at [`MAX_FILES`].
    pub(crate) fn from_json(v: &serde_json::Value) -> Self {
        let windows = v["windows"]
            .as_array()
            .map(|windows| {
                windows
                    .iter()
                    .map(|w| LastWindow {
                        files: files_from(&w["files"]),
                        active: w["active"].as_str().map(str::to_string),
                        rect: rect_from(&w["rect"]),
                    })
                    .filter(|w| !w.files.is_empty())
                    // The main window is one of `MAX_WINDOWS`.
                    .take(crate::windows::MAX_WINDOWS.saturating_sub(1))
                    .collect()
            })
            .unwrap_or_default();
        Self { files: files_from(&v["files"]), active: v["active"].as_str().map(str::to_string), rect: rect_from(&v["rect"]), windows }
    }
}

fn files_from(v: &serde_json::Value) -> Vec<SessionFile> {
    v.as_array()
        .map(|files| {
            files
                .iter()
                .filter_map(|f| {
                    let path = f["path"].as_str().filter(|p| !p.is_empty())?.to_string();
                    // `go_to_page` clamps a page past the end.
                    let page = f["page"].as_u64().and_then(|p| usize::try_from(p).ok()).unwrap_or(0);
                    Some(SessionFile { path, page })
                })
                .take(MAX_FILES)
                .collect()
        })
        .unwrap_or_default()
}

/// A window rectangle from settings: four finite numbers (the size at least 100 points); anything
/// else is dropped. Whether it is on a screen is checked when the window opens.
fn rect_from(v: &serde_json::Value) -> Option<[f32; 4]> {
    let a = v.as_array()?;
    let n = |i: usize| a.get(i).and_then(serde_json::Value::as_f64).filter(|x| x.is_finite() && x.abs() < 1.0e6).map(|x| x as f32);
    let r = [n(0)?, n(1)?, n(2)?, n(3)?];
    (r[2] >= 100.0 && r[3] >= 100.0).then_some(r)
}

/// `path` made absolute against the current directory, without resolving links (and without
/// Windows' `\\?\` prefix, which `canonicalize` adds).
#[cfg(not(target_arch = "wasm32"))]
fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

impl PdfCraftApp {
    /// The tabs of window `id` without loading it: its views and the active one.
    fn window_tabs(&self, id: crate::WindowId) -> Option<(&[crate::DocView], Option<usize>)> {
        if id == self.current_window {
            Some((&self.views, self.active))
        } else if id == crate::WindowId::ROOT {
            Some((&self.root_state.views, self.root_state.active))
        } else {
            self.windows.iter().find(|w| w.id == id).map(|w| (w.state.views.as_slice(), w.state.active))
        }
    }

    /// The files open now, in every window. Documents that were never saved to a file are left
    /// out (crash recovery keeps their unsaved work), and so is everything on the web, which has
    /// no paths.
    pub(crate) fn open_session(&self) -> LastSession {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let window = |id: crate::WindowId| {
                let (views, active) = self.window_tabs(id).unwrap_or((&[], None));
                let path = |v: &crate::DocView| self.session.get(v.id).and_then(|d| d.path.as_deref()).map(absolute);
                let files: Vec<SessionFile> =
                    views.iter().filter_map(|v| Some(SessionFile { path: path(v)?, page: v.current })).take(MAX_FILES).collect();
                let active = active.and_then(|i| views.get(i)).and_then(path);
                (files, active)
            };
            let rect_of = |id: crate::WindowId| {
                let r = if id == crate::WindowId::ROOT { self.root_rect } else { self.windows.iter().find(|w| w.id == id).and_then(|w| w.last_rect) };
                r.filter(|r| r.is_finite()).map(|r| [r.min.x, r.min.y, r.width(), r.height()])
            };
            let (files, active) = window(crate::WindowId::ROOT);
            let windows = self
                .windows
                .iter()
                .map(|w| {
                    let (files, active) = window(w.id);
                    LastWindow { files, active, rect: rect_of(w.id) }
                })
                .filter(|w| !w.files.is_empty())
                .take(crate::windows::MAX_WINDOWS.saturating_sub(1))
                .collect();
            LastSession { files, active, rect: rect_of(crate::WindowId::ROOT), windows }
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
                    if last.active.as_ref() == Some(&f.path) {
                        active = Some(self.views.len() - 1);
                    }
                }
            }
            self.password_prompt = prompt;
            if active.is_some() {
                self.active = active;
            }
            self.reopen_other_windows(&last, &skip);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (last, skip);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl PdfCraftApp {
    /// The windows besides the main one: each opens its files, and a file that is open in
    /// another window already becomes a second view of it (the document opens once).
    fn reopen_other_windows(&mut self, last: &LastSession, skip: &[String]) {
        let monitor = self.ctx.as_ref().and_then(|c| c.input(|i| i.viewport().monitor_size));
        for w in last.windows.iter().take(crate::windows::MAX_WINDOWS.saturating_sub(1)) {
            if self.windows.len() + 1 >= crate::windows::MAX_WINDOWS {
                break;
            }
            let id = crate::WindowId(self.next_window_id);
            self.next_window_id = self.next_window_id.saturating_add(1);
            let geometry = w.rect.and_then(|r| plausible_rect(r, monitor));
            self.windows.push(crate::windows::WindowSlot { id, state: Default::default(), geometry, last_rect: None });
            self.with_window(id, |a| {
                let mut active = None;
                for f in &w.files {
                    if skip.contains(&f.path) || !std::path::Path::new(&f.path).is_file() {
                        continue;
                    }
                    if a.open_file_here(&f.path, f.page) && w.active.as_ref() == Some(&f.path) {
                        active = Some(a.views.len().saturating_sub(1));
                    }
                }
                if active.is_some() {
                    a.active = active;
                }
            });
        }
        // A window whose files are all gone is not left empty.
        self.windows.retain(|w| !w.state.views.is_empty());
        self.assign_view_numbers();
    }

    /// Show the file at `path` in this window: a second view when another window has the
    /// document open, else open it. False when nothing could be shown.
    fn open_file_here(&mut self, path: &str, page: usize) -> bool {
        let shown = |a: &Self, p: &str| a.session.docs().iter().find(|d| d.path.as_deref().map(absolute).as_deref() == Some(p)).map(|d| d.id);
        let before = self.views.len();
        if let Some(doc) = self.window_ids().into_iter().find_map(|w| self.with_window(w, |a| shown(a, path)).flatten()) {
            if self.views.iter().any(|v| v.id == doc) {
                return false;
            }
            let Some(d) = self.session.get(doc) else { return false };
            let mut view = crate::DocView::new(doc, &d.info, self.view_defaults);
            view.seen_generation = d.edit_generation();
            view.seen_display_generation = d.display_generation();
            self.views.push(view);
        } else {
            self.open_path(path);
        }
        if self.views.len() > before
            && let Some(view) = self.views.last_mut()
        {
            view.go_to_page(page);
            return true;
        }
        false
    }
}

/// A saved window rectangle, if it can be shown: finite, not tiny or huge, and (when the screen
/// is known) starting on it.
fn plausible_rect(r: [f32; 4], monitor: Option<egui::Vec2>) -> Option<egui::Rect> {
    let rect = egui::Rect::from_min_size(egui::pos2(r[0], r[1]), egui::vec2(r[2], r[3]));
    let sane = rect.is_finite() && (100.0..=20_000.0).contains(&rect.width()) && (100.0..=20_000.0).contains(&rect.height());
    let on_screen = monitor.is_none_or(|m| rect.min.x >= -50.0 && rect.min.y >= -50.0 && rect.min.x < m.x && rect.min.y < m.y);
    (sane && on_screen).then_some(rect)
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
    fn settings_from_before_windows_are_one_window() {
        let v = serde_json::json!({"files": [{"path": "/a.pdf", "page": 2}], "active": "/a.pdf"});
        let s = LastSession::from_json(&v);
        assert_eq!(s.files.len(), 1);
        assert!(s.windows.is_empty() && s.rect.is_none());
        // And nothing new is written for one window: older versions read it as before.
        let written = serde_json::to_value(&s).unwrap();
        assert!(written.get("windows").is_none() && written.get("rect").is_none());
    }

    #[test]
    fn windows_are_read_and_written() {
        let v = serde_json::json!({
            "files": [{"path": "/a.pdf"}],
            "rect": [10.0, 20.0, 800.0, 600.0],
            "windows": [{"files": [{"path": "/a.pdf", "page": 3}, {"path": "/b.pdf"}], "active": "/b.pdf", "rect": [50, 60, 900, 700]}, {"files": []}],
        });
        let s = LastSession::from_json(&v);
        assert_eq!(s.rect, Some([10.0, 20.0, 800.0, 600.0]));
        assert_eq!(s.windows.len(), 1, "a window without files is dropped");
        assert_eq!(s.windows[0].files.len(), 2);
        assert_eq!(s.windows[0].active.as_deref(), Some("/b.pdf"));
        let again = LastSession::from_json(&serde_json::to_value(&s).unwrap());
        assert_eq!(again, s);
    }

    #[test]
    fn odd_rectangles_never_panic_and_are_dropped() {
        for bad in [
            serde_json::json!([f64::NAN, 0, 800, 600]),
            serde_json::json!([0, 0, 1e300, 600]),
            serde_json::json!([0, 0, 5, 5]),
            serde_json::json!([0, 0]),
            serde_json::json!("x"),
            serde_json::json!([null, 0, 800, 600]),
        ] {
            assert_eq!(rect_from(&bad), None, "{bad}");
        }
        assert!(plausible_rect([5000.0, 0.0, 800.0, 600.0], Some(egui::vec2(1920.0, 1080.0))).is_none(), "off the screen");
        assert!(plausible_rect([100.0, 100.0, 800.0, 600.0], Some(egui::vec2(1920.0, 1080.0))).is_some());
    }

    #[test]
    fn ten_thousand_windows_are_capped() {
        let windows: Vec<serde_json::Value> = (0..10_000).map(|i| serde_json::json!({"files": [{"path": format!("/{i}.pdf")}]})).collect();
        let s = LastSession::from_json(&serde_json::json!({"windows": windows}));
        assert_eq!(s.windows.len(), crate::windows::MAX_WINDOWS - 1);
    }
}
