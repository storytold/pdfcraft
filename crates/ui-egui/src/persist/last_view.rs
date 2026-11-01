//! Restore last view settings when reopening documents (Acrobat's Preferences ▸ Documents
//! option; #1767): the page, zoom and page display (with the two-page cover setting) a file was
//! last shown with, remembered by its path when its tab is closed or settings are saved, and
//! applied when the file is opened again. Desktop only: a document without a file has nothing to be found by.

use serde::Serialize;

use crate::PdfCraftApp;
use crate::canvas::{self, ViewDefaults};

/// How many files' views are remembered (the most recently closed or saved first).
const MAX_REMEMBERED: usize = 100;

/// How a file was last shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LastView {
    /// Absolute, as `last_session` keys files.
    pub path: String,
    /// 0-based.
    pub page: usize,
    /// As settings name it: `fit-width`, `fit-page` or a percentage, kept to its fraction
    /// (`62.5%`; read back by `ViewDefaults::with_zoom`).
    pub zoom: String,
    /// The page display token (`canvas::display_token`).
    pub display: String,
    /// Whether two-page view showed a cover page (the first page on its own).
    pub cover: bool,
}

/// As `last_session` keys files (and the web has no paths to remember).
#[cfg(not(target_arch = "wasm32"))]
fn absolute(path: &str) -> String {
    std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

#[cfg(target_arch = "wasm32")]
fn absolute(path: &str) -> String {
    path.to_string()
}

impl PdfCraftApp {
    /// How view `index` shows its document now; `None` for a document without a file. In full
    /// screen, the display it will go back to, at the page reached.
    fn view_to_remember(&self, index: usize) -> Option<LastView> {
        let view = self.views.get(index)?;
        let path = self.session.get(view.id)?.path.as_deref()?;
        let shown = view.normal_display();
        // A fixed zoom keeps its fraction (62.5%, not 63%): `zoom_name` rounds to whole percents.
        let zoom = match shown.fit {
            canvas::Fit::None => format!("{}%", shown.zoom * 100.0),
            fit => ViewDefaults { fit, zoom: shown.zoom, ..ViewDefaults::default() }.zoom_name(),
        };
        Some(LastView {
            path: absolute(path),
            page: view.current,
            zoom,
            display: canvas::display_token(shown.layout, shown.continuous).to_string(),
            cover: shown.cover,
        })
    }

    /// Remember how view `index` shows its document (when its tab closes).
    pub(crate) fn remember_view(&mut self, index: usize) {
        if let Some(v) = self.view_to_remember(index) {
            self.last_views.retain(|l| l.path != v.path);
            self.last_views.insert(0, v);
            self.last_views.truncate(MAX_REMEMBERED);
        }
    }

    /// The remembered views as settings store them: the open views first, as they are now
    /// (so quitting remembers them without closing their tabs), then the rest.
    pub(crate) fn last_views_to_save(&self) -> Vec<LastView> {
        let mut out: Vec<LastView> = (0..self.views.len()).filter_map(|i| self.view_to_remember(i)).collect();
        for l in &self.last_views {
            if !out.iter().any(|o| o.path == l.path) {
                out.push(l.clone());
            }
        }
        out.truncate(MAX_REMEMBERED);
        out
    }

    /// Settings are untrusted: an entry without a usable path, zoom or display is dropped, and
    /// the list is capped.
    pub(crate) fn restore_last_views(&mut self, v: &serde_json::Value) {
        let Some(list) = v.as_array() else { return };
        self.last_views = list
            .iter()
            .take(MAX_REMEMBERED)
            .filter_map(|e| {
                let path = e["path"].as_str().filter(|p| !p.is_empty())?.to_string();
                let zoom = e["zoom"].as_str().filter(|z| ViewDefaults::default().with_zoom(z).is_some())?.to_string();
                let display = e["display"].as_str().filter(|d| canvas::parse_display_token(d).is_some())?.to_string();
                let page = usize::try_from(e["page"].as_u64().unwrap_or(0)).unwrap_or(0);
                let cover = e["cover"].as_bool().unwrap_or(false);
                Some(LastView { path, page, zoom, display, cover })
            })
            .collect();
    }

    /// Show view `index` as its document's file was last shown, when the preference is on and
    /// the file is remembered. Applied after the document's own initial view, which it
    /// overrides: where the reader left off is what they asked to come back to.
    pub(crate) fn apply_last_view(&mut self, index: usize) {
        if !self.restore_last_view {
            return;
        }
        let Some(view) = self.views.get(index) else { return };
        let Some(doc) = self.session.get(view.id) else { return };
        let Some(path) = doc.path.as_deref().map(absolute) else { return };
        let pages = doc.info.pages.len();
        let Some(last) = self.last_views.iter().find(|l| l.path == path).cloned() else { return };
        let Some(view) = self.views.get_mut(index) else { return };
        if let Some((layout, continuous)) = canvas::parse_display_token(&last.display) {
            view.set_display(layout, continuous);
        }
        view.set_cover(last.cover);
        if let Some(defaults) = ViewDefaults::default().with_zoom(&last.zoom) {
            match defaults.fit {
                canvas::Fit::None => view.set_zoom(defaults.zoom),
                fit => view.set_fit(fit),
            }
        }
        if last.page < pages {
            view.go_to_page(last.page);
        }
    }
}
