//! Open and Save against cloud storage: craft-ui's real cloud feature
//! (storytold/craft-libs#160), not a local placeholder.
//!
//! Nobody connects a backend here. [`craft_ui::cloud::CloudFiles::connect`] is never called
//! anywhere in this repository, so [`craft_ui::cloud::available`] stays false and every surface
//! that reaches these pages stays hidden: see `commands.rs`, `menus.rs`, `hotbar.rs`,
//! `dialogs/palette.rs` and `automation/pages.rs` for the gates, all of which read this module's
//! re-exported [`available`] rather than assuming anything about whether a service exists.
//!
//! What this module adds on top of the re-exported contract is only what craft-ui asks an
//! application to supply itself: [`PdfCraftApp`]'s [`CloudApp`] implementation, its
//! [`DocumentKind`] (mirrored from the real File ▸ Open picker, not invented separately), and its
//! icon and Save a Copy formats.
//!
//! e-sign has its own shared contract too (storytold/craft-libs#661, merged): see `esign.rs`'s
//! module doc for its real feature's module and why nothing connects a backend to it either.

use egui::{Color32, Painter, Rect, Ui};

pub use craft_ui::cloud::*;

/// What PdfCraft opens, mirrored from [`crate::app::files::PdfCraftApp::open_filters`] (the real
/// File ▸ Open picker) so the cloud Open page can never drift from what local Open actually
/// accepts. `fdf` and `xfdf` are deliberately absent: those belong to the form-data import
/// dialog, not general document open.
fn open_extensions() -> &'static [&'static str] {
    static LIST: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        let mut extensions = vec!["pdf"];
        extensions.extend(pdfcraft_engine::IMAGE_EXTENSIONS.iter().copied());
        // Office documents convert through LibreOffice, desktop only (`open_filters`'s own gate).
        #[cfg(not(target_arch = "wasm32"))]
        extensions.extend(pdfcraft_engine::OFFICE_EXTENSIONS.iter().copied());
        extensions.extend(pdfcraft_engine::TEXT_EXTENSIONS.iter().copied());
        extensions
    })
}

/// Content types for the same formats, read when a provider hands back a correct type and no
/// extension at all. Matched by prefix (`DocumentKind::mimes`'s own contract), so a profiled type
/// such as `application/pdf; version=1.7` still matches `application/pdf`.
const MIMES: &[&str] = &[
    "application/pdf",
    "image/png",
    "image/jpeg",
    "image/tiff",
    "image/gif",
    "image/bmp",
    "image/jp2",
    "image/x-pcx",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml",
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.presentationml",
    "application/vnd.oasis.opendocument",
    "application/rtf",
    "text/plain",
];

/// What one of PdfCraft's documents is, as the cloud pages need to know. One value, computed
/// once: see [`open_extensions`] for why it is not a hand-duplicated list.
fn kind() -> DocumentKind {
    DocumentKind { extensions: open_extensions(), mimes: MIMES, noun: "document", default_extension: "pdf" }
}

/// PdfCraft's one Save a Copy format. Routed through [`tl!`] (F15 in the review): `CopyFormat`
/// takes `Into<String>`, so there is no reason for this label to be the one cloud string that
/// skips the catalog.
pub fn copies() -> Vec<CopyFormat> {
    vec![CopyFormat::new("pdf", tl!("Save a Copy as PDF (.pdf)…"))]
}

/// Draws whichever cloud page `app.cloud_page` names, in place of the normal main pane content.
/// Called from `panes.rs`'s `main_pane`, ahead of Home and the document view: the Open and Save
/// As pages are a Backstage-style full-pane view, the same way they replace the main pane in
/// every other Craft app that has wired this feature.
pub fn page_ui(app: &mut crate::PdfCraftApp, ui: &mut Ui) {
    let page = app.cloud_page;
    match page {
        Some(crate::CloudPage::Open) => open_page(app, ui),
        Some(crate::CloudPage::SaveAs) if save_page(app, ui) => {
            app.cloud_page = None;
        }
        Some(crate::CloudPage::SaveAs) | None => {}
    }
}

impl CloudApp for crate::PdfCraftApp {
    fn cloud(&self) -> &CloudFiles {
        &self.cloud
    }

    fn cloud_mut(&mut self) -> &mut CloudFiles {
        &mut self.cloud
    }

    fn kind(&self) -> DocumentKind {
        kind()
    }

    fn active(&self) -> Option<u64> {
        let index = self.active?;
        self.views.get(index).map(|view| view.id.0)
    }

    fn title(&self, uid: u64) -> String {
        self.session.get(pdfcraft_engine::DocId(uid)).map(|doc| doc.display_name()).unwrap_or_default()
    }

    fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Option<u64>, String> {
        // The cloud page that asked for this open is still showing until here: closing it only on
        // success, from inside the one path that can mean a cloud open just completed, is what
        // keeps the page from getting stuck open or dismissing itself on an unrelated local open
        // (the tab-count heuristic the earlier local stand-in used, which neither distinguished).
        crate::PdfCraftApp::open_bytes(self, name, None, bytes)?;
        self.cloud_page = None;
        Ok(self.active.and_then(|index| self.views.get(index)).map(|view| view.id.0))
    }

    fn encode(&mut self, uid: u64, _into: Destination<'_>) -> Result<Vec<u8>, String> {
        let id = pdfcraft_engine::DocId(uid);
        // Pending typed edits land first, the same guard a local Save takes (`save_doc_to`): a
        // cloud save with no UI step of its own to flush them would otherwise drop whatever the
        // user was still typing.
        self.commit_typing_in(id);
        // Always a clean, full copy: a document opened from the cloud under one name and saved
        // under another (Save As, or a save that landed in a different account) must not carry
        // revisions that retain deleted pages or replaced text, the same reasoning `save_doc_to`
        // applies to every Save As.
        let bytes = self.session.save_full_bytes(id).map_err(|e| e.to_string())?;
        self.session.check_save(id, &bytes).map_err(|e| e.to_string())?;
        Ok((*bytes).clone())
    }

    fn close(&mut self, uid: u64) {
        if let Some(index) = self.views.iter().position(|view| view.id.0 == uid) {
            self.request_close_tab(index);
        }
    }

    fn sharing(&mut self) -> &mut craft_ui::share::ShareDialog {
        &mut self.sharing
    }

    fn presence(&mut self) -> &mut craft_ui::presence::PresenceBar {
        &mut self.presence
    }

    fn paint_icon(&self, painter: &Painter, rect: Rect, kind: craft_cloud::EntryKind, colour: Color32) {
        let name = match kind {
            craft_cloud::EntryKind::Folder => "folder",
            craft_cloud::EntryKind::File => "file-text",
        };
        crate::widgets::icons::paint_on(painter, rect, name, colour);
    }

    fn open_url(&self, url: &str) {
        // Mirrors `app/links.rs`'s own `open_url`, minus the `last_opened_url` bookkeeping that
        // method keeps: `CloudApp::open_url` takes `&self`, and that field is for debugging what
        // PdfCraft's own help links last opened, not something a cloud page's link needs.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ctx) = &self.ctx {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(window) = web_sys::window() {
            let _ = window.open_with_url_and_target_and_features(url, "_blank", "noopener,noreferrer");
        }
    }

    fn say(&mut self, text: String) {
        // PdfCraft has no separate modal message box; this reaches the user through the same
        // toast every other notice in the app uses.
        self.notify(text);
    }

    fn toast(&mut self, text: String) {
        self.notify(text);
    }

    fn app_name(&self) -> &str {
        "PdfCraft"
    }

    fn tr(&self, text: &str) -> String {
        crate::i18n::tr(crate::i18n::Lang::from_pref(&self.language), text).to_owned()
    }
}

#[cfg(test)]
#[path = "cloud_tests.rs"]
mod tests;
