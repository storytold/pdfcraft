//! PdfCraft's half of Open and Save against cloud storage.
//!
//! The pages themselves are `craft_ui::cloud`. Every Craft app offers the same
//! ones. What is here is what only PdfCraft can answer. A document is a PDF.
//! These extensions and content types are what that means. Bytes become one
//! through [`PdfCraftApp::open_bytes`] and go back through
//! `session.save_full_bytes`.
//!
//! PdfCraft holds no provider credential and speaks no provider protocol.
//! Every call goes to the CollabCraft service. That service is the single
//! place a provider is configured and permissions are decided. Replacing one
//! provider with another changes nothing here.
//!
//! Nothing in this file opens a socket either. The transport half is a
//! [`CloudBackend`] the host application supplies. Every CollabCraft call
//! blocks. The pages state a [`Job`]. The host runs it on a worker thread.
//! The answer arrives on a channel that [`pump`] drains on a later frame.
//!
//! ## PDF semantics
//!
//! PdfCraft opens, views, annotates and form-fills PDFs. It does not host
//! real-time collaborative text editing. The share dialog lets a user grant
//! View or Edit access; "Edit" here means annotation and form filling, which
//! the provider enforces. No claim is made that two people can edit the
//! underlying PDF structure simultaneously.

use egui::{Color32, Painter, Rect, Ui};

// Re-exported so a host application can name these types without depending on
// the crates they come from. Tests need them for the same reason.
pub use craft_cloud::{Capability, Entry, EntryKind};
pub use craft_ui::cloud::{
    Account, Busy, CloudApp, CloudBackend, CloudFiles, Conflict, CopyFormat, Destination, DocumentKind, Event, Fault, Folder, Job, Link, Origin,
    Post, Source, UNAVAILABLE, available, browse, can_converse, conflict_modal, destination_name, has_target, link, load_shared, open_entry,
    open_page, origin, parent_of, prefers_cloud, progress, reload_active, save, save_as, save_page, share_button, shares, sign_in, sign_out, start,
    work_together,
};

use crate::PdfCraftApp;
use crate::theme::Tokens;

/// Extensions PdfCraft opens. The same set the desktop file picker offers.
const OPENS: [&str; 3] = ["pdf", "fdf", "xfdf"];

/// Content types PdfCraft opens, matched against the start of what the
/// provider reports. Neither extension nor MIME is reliable alone.
const MIMES: [&str; 3] = ["application/pdf", "application/vnd.adobe.xfdf", "application/vnd.adobe.xdf"];

/// What a PdfCraft document is, as a cloud file list needs to know.
pub const DOCUMENT: DocumentKind = DocumentKind { extensions: &OPENS, mimes: &MIMES, noun: "PDF", default_extension: "pdf" };

/// The format every cloud save writes.
///
/// PdfCraft saves PDFs. A file opened from the cloud under any extension goes
/// back as a PDF — with annotations, form data and any other changes. FDF and
/// XFDF import forms data but are not saved back as such.
const SAVES_AS: &str = "pdf";

/// Save a Copy formats offered under the share.
///
/// PdfCraft can export images, but those are destructive one-way conversions,
/// not copies. The only meaningful cloud copy is the PDF itself.
const COPIES: [(&str, &str); 1] = [("pdf", "Save a Copy as PDF (.pdf)\u{2026}")];

/// Whether PdfCraft can open this entry.
pub fn opens(entry: &Entry) -> bool {
    craft_ui::cloud::opens(entry, DOCUMENT)
}

/// A typed name as a PDF file name. Strips folder separators; adds `.pdf`
/// when no recognised extension is present.
pub fn pdf_name(typed: &str) -> String {
    craft_ui::cloud::document_name(typed, DOCUMENT)
}

/// The whole of the cloud state, for the control channel and for tests.
pub fn status(app: &PdfCraftApp) -> serde_json::Value {
    craft_ui::cloud::report(app)
}

/// The Share menu's export half.
pub fn copies() -> Vec<CopyFormat> {
    COPIES.iter().map(|(id, label)| CopyFormat::new(*id, *label)).collect()
}

/// The host part of the service address, for the account menu's first line.
///
/// Read from the environment; nothing is compiled in. The service is
/// deployment-specific. Somebody with accounts on two deployments needs to
/// know which one a PDF is about to land in.
fn service_host() -> String {
    ["PDFCRAFT_CLOUD_URL", "CRAFT_CLOUD_URL", "COLLABCRAFT_URL"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .find(|value| !value.is_empty())
        .map(|value| value.trim_start_matches("https://").trim_start_matches("http://").to_owned())
        .unwrap_or_default()
}

/// The format a cloud save should be written in, given its destination.
///
/// PdfCraft only writes PDF. A file opened from the cloud under any other
/// extension still goes back as a PDF. The only readable output from this
/// application is a PDF, and so that is the only format offered here. A
/// destination that explicitly asks for something else is refused with a
/// message the user can act on.
fn writes_as(into: Destination<'_>) -> Result<&'static str, String> {
    let ext = into.extension();
    if ext == "pdf" || ext.is_empty() {
        return Ok(SAVES_AS);
    }
    Err(format!(
        "PdfCraft saves PDFs. It cannot write a .{ext} file to the cloud. \
         Use Save As to create a new PDF in the cloud."
    ))
}

impl CloudApp for PdfCraftApp {
    fn cloud(&self) -> &CloudFiles {
        &self.cloud
    }

    fn cloud_mut(&mut self) -> &mut CloudFiles {
        &mut self.cloud
    }

    fn kind(&self) -> DocumentKind {
        DOCUMENT
    }

    fn active(&self) -> Option<u64> {
        self.active_ids().map(|(_, id)| id.0)
    }

    fn title(&self, uid: u64) -> String {
        let id = pdfcraft_engine::DocId(uid);
        self.session.get(id).map(|d| d.display_name()).unwrap_or_default()
    }

    /// Open PDF bytes from the cloud.
    ///
    /// Returns `Ok(None)` when the document is password-protected and a prompt
    /// is shown; the cloud page waits for the next event rather than assuming
    /// success.
    fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Option<u64>, String> {
        // Record which document ids are already open before the call.
        let before: std::collections::HashSet<u64> = self.views.iter().map(|v| v.id.0).collect();
        // `PdfCraftApp::open_bytes` opens with focus and returns only errors.
        // Password-protected documents go into a prompt without raising an error.
        self.open_bytes(name, None, bytes)?;
        // The newly opened document, if it became active immediately.
        let after = self.active_ids().map(|(_, id)| id.0).filter(|uid| !before.contains(uid));
        Ok(after)
    }

    /// Encode the document's current state as PDF bytes for the given destination.
    fn encode(&mut self, uid: u64, into: Destination<'_>) -> Result<Vec<u8>, String> {
        let id = pdfcraft_engine::DocId(uid);
        writes_as(into)?;
        let bytes = self.session.save_full_bytes(id).map_err(|e| e.to_string())?;
        Ok(bytes.as_ref().clone())
    }

    fn close(&mut self, uid: u64) {
        let id = pdfcraft_engine::DocId(uid);
        if let Some(i) = self.views.iter().position(|v| v.id == id) {
            self.close_tab(i);
        }
    }

    fn ready_to_save(&mut self) -> bool {
        // A form field's open editor holds text that is not in the document yet.
        self.commit_form_typing()
    }

    fn sharing(&mut self) -> &mut craft_ui::share::ShareDialog {
        &mut self.share
    }

    fn presence(&mut self) -> &mut craft_ui::presence::PresenceBar {
        &mut self.presence
    }

    fn paint_icon(&self, painter: &Painter, rect: Rect, kind: EntryKind, colour: Color32) {
        let name = if kind == EntryKind::Folder { "folder" } else { "file-text" };
        crate::icons::paint_on(painter, rect, name, colour);
    }

    fn open_url(&self, url: &str) {
        if let Some(ctx) = &self.ctx {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }

    fn say(&mut self, text: String) {
        self.notify(text);
    }

    fn toast(&mut self, text: String) {
        self.notify(text);
    }

    fn go_to_save_as(&mut self) {
        // The conflict modal calls this when the user chooses "Save as a copy".
        // PdfCraft has no backstage, so we set cloud_page to SaveAs which renders
        // save_page in the CentralPanel on the next frame.
        self.cloud_page = Some(crate::CloudPage::SaveAs);
    }

    fn app_name(&self) -> &str {
        "PdfCraft"
    }

    fn service(&self) -> String {
        service_host()
    }

    fn save_note(&self) -> Option<String> {
        Some("The PDF is saved in the folder above. Annotations and form data are preserved.".to_owned())
    }

    fn style(&self, ui: &Ui) -> craft_ui::cloud::CloudStyle {
        let t = Tokens::get(ui.ctx());
        craft_ui::cloud::CloudStyle {
            surface: t.chrome,
            border: t.border,
            text: t.text,
            dim: t.text_muted,
            hover: t.hover,
            soft: t.accent.gamma_multiply(0.14),
            family: egui::FontFamily::Proportional,
            bold: egui::FontFamily::Name("semibold".into()),
        }
    }
}

/// The title-bar account badge and menu (`craft_ui::cloud::who`), plus the browser
/// login popup on Sign in while the click's user activation still holds.
pub fn who(app: &mut PdfCraftApp, ui: &mut Ui) {
    craft_ui::cloud::who(app, ui);
    #[cfg(target_arch = "wasm32")]
    if app.cloud_mut().take_sign_in_from_menu() && app.cloud.account.is_none() {
        let ctx = ui.ctx();
        let popup_retry = egui::Id::new("pdfcraft:popup_retry");
        ctx.memory_mut(|m| m.data.remove::<bool>(popup_retry));
        crate::web_auth::offer_login_popup(ctx);
    }
}

/// Drain cloud worker events from the background thread.
pub fn pump(app: &mut PdfCraftApp, ctx: &egui::Context) {
    craft_ui::cloud::pump(app, ctx);
}

#[cfg(test)]
#[path = "cloud_tests.rs"]
mod tests;
