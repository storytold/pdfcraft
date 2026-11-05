//! PdfCraft's cloud Open/Save/Share scaffolding.
//!
//! No concrete backend ships in this crate, or anywhere in this repository.
//! `craft-libs#160` is building a storytold-reviewable `CloudBackend`
//! reference implementation to replace the personal-fork `craft_ui::cloud`,
//! `craft_ui::share` and `craft_ui::presence` modules this file used to
//! depend on. Until that lands, this file defines its own small local
//! contract, trimmed to exactly what PdfCraft's UI calls below. Once
//! craft-libs#160 ships a trait with the same shape, wiring it in should be
//! a matter of calling [`CloudFiles::connect`] with it; nothing above this
//! seam should need to change.
//!
//! Nobody calls [`CloudFiles::connect`] today, so [`available`] is always
//! false. The Open/Save cloud menu items, the title bar's share button,
//! account badge and presence list all gate on it and so stay hidden
//! entirely, rather than shown and disabled: a greyed "Open from cloud…"
//! with nothing behind it would read as a broken feature, not a pending one.
//!
//! Every job answers on its own thread; [`CloudBackend::submit`] must return
//! before anything blocking runs, or it freezes the window that called it.
//! [`pump`] drains the channel the backend posts to on a later frame.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::Ui;

use crate::PdfCraftApp;

/// Extensions PdfCraft opens from the cloud. The same set the desktop file picker offers.
const OPENS: [&str; 3] = ["pdf", "fdf", "xfdf"];

/// The format every cloud save writes: PdfCraft saves PDFs, whatever extension a file was
/// opened under.
const SAVES_AS: &str = "pdf";

/// One entry in a cloud folder listing: a file or a folder a [`CloudBackend`] reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Opaque to PdfCraft; round-tripped back to the backend on open/save.
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    /// A conditional-write token, when the backend supports one.
    pub etag: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
}

/// The signed-in account shown in the title bar's badge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub display: String,
}

/// A cloud job in flight, for the busy spinner. Trimmed to what the title bar and the hidden
/// menu items read today; a real backend's job set will likely be larger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    Account,
    Open,
    Save,
}

/// What a [`CloudBackend`] reports failing, read by the account badge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    Unauthenticated,
    Other(String),
}

/// What the host asks a [`CloudBackend`] to do.
#[derive(Clone, Debug)]
pub enum Job {
    Account,
    SignIn,
    SignOut,
}

/// What a [`CloudBackend`] answers with, through [`Post::send`].
#[derive(Clone, Debug)]
pub enum Event {
    Account(Result<Account, Fault>),
}

/// Where a [`CloudBackend`] posts its answer once a [`Job`] finishes. A thin wrapper so a
/// backend crate never needs egui as a dependency of its own.
#[derive(Clone)]
pub struct Post {
    ctx: egui::Context,
    sender: Sender<Event>,
}

impl Post {
    pub fn send(&self, event: Event) {
        let _ = self.sender.send(event);
        self.ctx.request_repaint();
    }
}

/// The contract a cloud provider implements.
///
/// Pending craft-libs#160: once that crate's reference `CloudBackend` lands, this trait should
/// be able to go away in favour of it with no change to the call sites in this file.
pub trait CloudBackend: Send + Sync {
    fn submit(&self, job: Job, post: Post);
}

/// Open and Save against cloud storage, and the signed-in account. Local-only placeholder
/// (see the module doc): nothing populates [`CloudFiles::backend`] today.
#[derive(Default)]
pub struct CloudFiles {
    backend: Option<Arc<dyn CloudBackend>>,
    receiver: Option<Receiver<Event>>,
    pub account: Option<Account>,
    pub account_fault: Option<Fault>,
    pub busy: Option<Busy>,
    /// Shown in the account menu while a sign-in or save is in flight.
    pub status: Option<String>,
}

impl CloudFiles {
    pub fn new() -> CloudFiles {
        CloudFiles::default()
    }

    /// Wire in a backend. Once craft-libs#160 ships one, an app calls this the same way it
    /// would have called `CloudApp`'s old `connect`.
    pub fn connect(&mut self, backend: impl CloudBackend + 'static) {
        self.backend = Some(Arc::new(backend));
    }

    fn submit(&mut self, job: Job, ctx: &egui::Context) {
        let Some(backend) = self.backend.clone() else { return };
        let (sender, receiver) = channel();
        self.receiver = Some(receiver);
        backend.submit(job, Post { ctx: ctx.clone(), sender });
    }
}

/// Whether a cloud backend is connected. The Open/Save cloud menu items and the title bar's
/// share button, account badge and presence list all stay hidden while this is false.
pub fn available(app: &PdfCraftApp) -> bool {
    app.cloud.backend.is_some()
}

/// Whether PdfCraft can open this entry.
pub fn opens(entry: &Entry) -> bool {
    entry.kind == EntryKind::Folder || OPENS.iter().any(|ext| entry.name.to_ascii_lowercase().ends_with(&format!(".{ext}")))
}

/// A typed name as a PDF file name. Strips folder separators; adds `.pdf` when no recognised
/// extension is present.
pub fn pdf_name(typed: &str) -> String {
    let name = typed.trim().replace(['/', '\\'], " ");
    let name = if name.is_empty() { "Untitled".to_owned() } else { name };
    let has_extension =
        OPENS.iter().any(|ext| name.to_ascii_lowercase().ends_with(&format!(".{ext}"))) || name.to_ascii_lowercase().ends_with(".pdf");
    if has_extension { name } else { format!("{name}.{SAVES_AS}") }
}

/// The document a cloud save or share request is about: its id and, if the document was opened
/// from the cloud, the entry it came from.
pub struct Origin {
    pub entry: Entry,
}

/// The entry the active document was opened from, if any. Nothing populates this today: no
/// cloud open has ever happened without a backend.
pub fn origin(_app: &PdfCraftApp) -> Option<&Origin> {
    None
}

/// Whether the connected backend supports sharing. Always false: nothing is connected.
pub fn shares(_app: &PdfCraftApp) -> bool {
    false
}

/// "Save a Copy as" format offered under the share button.
pub struct CopyFormat {
    pub id: &'static str,
    pub label: &'static str,
}

/// The Share menu's export half.
pub fn copies() -> Vec<CopyFormat> {
    vec![CopyFormat { id: "pdf", label: "Save a Copy as PDF (.pdf)\u{2026}" }]
}

/// The share button, drawn only when [`shares`] is true (so never today). Kept so wiring a real
/// backend back in is a matter of this function drawing something, not call sites changing.
pub fn share_button(_app: &mut PdfCraftApp, _ui: &mut Ui, _copies: &[CopyFormat]) -> Option<&'static str> {
    None
}

/// The title-bar account badge and menu. Draws nothing while no backend is connected.
pub fn who(app: &mut PdfCraftApp, ui: &mut Ui) {
    if !available(app) {
        return;
    }
    if let Some(account) = &app.cloud.account {
        ui.label(&account.display);
    }
}

/// Ask the connected backend to sign in. A no-op while none is connected.
pub fn sign_in(app: &mut PdfCraftApp) {
    if !available(app) {
        return;
    }
    app.cloud.status = Some("Finishing signing in\u{2026}".to_owned());
    let ctx = app.ctx.clone();
    if let Some(ctx) = ctx {
        app.cloud.submit(Job::SignIn, &ctx);
    }
}

/// Drain cloud worker events from the background thread. A no-op while no backend is connected,
/// since nothing ever submits a job in that state.
pub fn pump(app: &mut PdfCraftApp, _ctx: &egui::Context) {
    let Some(receiver) = &app.cloud.receiver else { return };
    while let Ok(event) = receiver.try_recv() {
        match event {
            Event::Account(Ok(account)) => {
                app.cloud.account = Some(account);
                app.cloud.account_fault = None;
                app.cloud.status = None;
            }
            Event::Account(Err(fault)) => {
                app.cloud.account = None;
                app.cloud.account_fault = Some(fault);
                app.cloud.status = None;
            }
        }
    }
}

#[cfg(test)]
#[path = "cloud_tests.rs"]
mod tests;
