//! Restore last view settings when reopening documents (Acrobat's Preferences ▸ Documents
//! option; #1767): the page, zoom and page display (with two-page cover and RTL order) a file was
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
    /// Whether the earlier page of a two-page pair was shown on the right.
    pub rtl_pages: bool,
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
            rtl_pages: view.rtl_pages,
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
                let rtl_pages = e["rtl_pages"].as_bool().unwrap_or(false);
                Some(LastView { path, page, zoom, display, cover, rtl_pages })
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
        view.set_rtl_pages(last.rtl_pages);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::PageLayout;
    use serde_json::json;

    /// Open-view and closed-tab snapshots must serialize RTL page order independently
    /// of the saved normal/full-screen display, then restore it when that path reopens.
    #[test]
    fn remembered_rtl_page_order_survives_settings_and_reopen() {
        for rtl in [true, false] {
            for presenting in [false, true] {
                let path = std::env::temp_dir()
                    .join(format!("pdfcraft-last-view-rtl-{}-{rtl}-{presenting}.pdf", std::process::id()))
                    .to_string_lossy()
                    .into_owned();
                let bytes = include_bytes!("../../tests/data/form.pdf").to_vec();
                let mut app = PdfCraftApp::new();
                app.open_bytes("last-view.pdf", Some(path.clone()), bytes.clone()).unwrap();
                let view = &mut app.views[0];
                view.set_display(PageLayout::TwoUp, false);
                view.set_cover(true);
                view.set_rtl_pages(rtl);
                if presenting {
                    view.enter_presentation();
                }
                let saved_open = app.persist();
                app.close_tab(0);
                let saved_closed = app.persist();
                for (label, saved) in [("open", saved_open), ("closed", saved_closed)] {
                    let settings: serde_json::Value = serde_json::from_str(&saved).unwrap();
                    assert_eq!(settings["last_views"][0]["rtl_pages"].as_bool(), Some(rtl), "{label}, presenting={presenting}");
                    let mut reopened = PdfCraftApp::new();
                    reopened.restore(&saved);
                    reopened.open_bytes("last-view.pdf", Some(path.clone()), bytes.clone()).unwrap();
                    let view = &reopened.views[0];
                    assert_eq!(view.rtl_pages, rtl, "{label}, presenting={presenting}");
                    assert_eq!((view.layout, view.continuous, view.cover), (PageLayout::TwoUp, false, true));
                }
            }
        }
    }

    /// Legacy entries and hostile nonboolean values must explicitly restore LTR,
    /// including when the current view was already set to RTL before applying them.
    #[test]
    fn remembered_rtl_page_order_defaults_invalid_or_missing_values_to_ltr() {
        let path = std::env::temp_dir().join(format!("pdfcraft-last-view-rtl-default-{}.pdf", std::process::id())).to_string_lossy().into_owned();
        for rtl in [None, Some(json!(null)), Some(json!("true")), Some(json!(1)), Some(json!(false))] {
            let mut entry = json!({"path": path, "page": 0, "zoom": "fit-page", "display": "two-up", "cover": false});
            if let Some(value) = rtl {
                entry["rtl_pages"] = value;
            }
            let mut app = PdfCraftApp::new();
            app.restore_last_views(&json!([entry]));
            app.open_bytes("last-view.pdf", Some(path.clone()), include_bytes!("../../tests/data/form.pdf").to_vec()).unwrap();
            app.views[0].set_rtl_pages(true);
            app.apply_last_view(0);
            assert!(!app.views[0].rtl_pages, "legacy or invalid settings must actively restore the default");
        }
    }
}
