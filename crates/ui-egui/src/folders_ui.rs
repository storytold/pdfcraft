//! Pinned folders on the Home view: the newest PDFs in folders the user chose (where a scanner
//! saves, or a cloud-synced folder such as iCloud Drive, OneDrive or Dropbox), one click away.
//!
//! Local only: a pinned folder is read through the file system like any other path, never
//! through a storage provider's API, and nothing about it leaves the machine. Folders are
//! listed on a worker thread and at most every few seconds while Home shows, so a slow, huge or
//! offline synced folder never blocks the frame.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use crate::PdfCraftApp;

/// At most this many folders can be pinned.
pub const MAX_PINNED: usize = 8;
/// Home shows this many of a folder's newest PDFs until "Show more" is clicked.
pub const SHOWN: usize = 8;
/// A folder's listing keeps at most this many PDFs (its newest).
pub const MAX_LISTED: usize = 100;
/// Directory entries read per folder, so a huge folder can't stall the scan.
const MAX_ENTRIES: usize = 20_000;
/// While Home shows, the folders are listed again at most this often (seconds).
const RESCAN_SECS: f64 = 5.0;
/// Longest folder path accepted, from the picker or from settings.
const MAX_PATH_CHARS: usize = 4096;

/// A PDF in a pinned folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderFile {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// What a pinned folder held when it was last listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderListing {
    pub folder: String,
    /// Newest first (files without a modified time last), at most [`MAX_LISTED`].
    pub files: Vec<FolderFile>,
    /// How many PDFs the folder holds; can be more than `files.len()`.
    pub total: usize,
    /// The folder couldn't be read: moved or deleted, no permission, or its drive or cloud sync
    /// is offline. It stays pinned and is listed again later.
    pub unavailable: bool,
}

/// The pinned folders and their latest listings.
#[derive(Debug, Default)]
pub struct PinnedFolders {
    /// Absolute paths, in the order they were pinned.
    pub folders: Vec<String>,
    /// The latest finished listing of each folder.
    pub listings: Vec<FolderListing>,
    /// Folders whose "Show more" is open.
    pub expanded: Vec<String>,
    /// A listing in progress: the worker fills the slot.
    scan: Option<Arc<Mutex<Option<Vec<FolderListing>>>>>,
    /// When (egui time) the last listing started.
    last_scan: Option<f64>,
    /// The folders changed since the last listing started: list them on the next frame.
    stale: bool,
}

impl PinnedFolders {
    /// The latest listing of `folder`, if one has finished since it was pinned.
    pub fn listing(&self, folder: &str) -> Option<&FolderListing> {
        self.listings.iter().find(|l| l.folder == folder)
    }

    /// Restore pinned folders from settings: absolute paths only, without duplicates, at most
    /// [`MAX_PINNED`]. Settings are untrusted, so anything else is ignored. A folder that is
    /// missing right now is kept (a synced drive may be offline) and shown as unavailable.
    pub(crate) fn restore(&mut self, value: &serde_json::Value) {
        let Some(items) = value.as_array() else { return };
        let mut folders = Vec::new();
        for folder in items.iter().filter_map(serde_json::Value::as_str).filter_map(clean) {
            if folders.len() >= MAX_PINNED {
                break;
            }
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        self.folders = folders;
        self.listings.clear();
        self.expanded.clear();
        self.stale = true;
    }
}

/// `path` as a folder to pin: trimmed, without a trailing separator, and absolute; `None` if it
/// can't be one.
fn clean(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || path.chars().count() > MAX_PATH_CHARS || path.contains('\0') {
        return None;
    }
    let trimmed = path.trim_end_matches(['/', '\\']);
    // The file system root ("/") keeps its separator, as does a Windows drive root ("C:\").
    let path = if trimmed.is_empty() || trimmed.ends_with(':') { path } else { trimmed };
    std::path::Path::new(path).is_absolute().then(|| path.to_string())
}

/// The PDFs directly inside `folder` (not its subfolders, and not hidden files), newest first.
pub fn list(folder: &str) -> FolderListing {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return FolderListing { folder: folder.to_string(), files, total: 0, unavailable: true };
    };
    for entry in entries.flatten().take(MAX_ENTRIES) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name.starts_with('.') || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
            continue;
        }
        // Follows links, like opening the file does.
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_file() {
            files.push(FolderFile { name, path: path.to_string_lossy().into_owned(), size: meta.len(), modified: meta.modified().ok() });
        }
    }
    let total = files.len();
    // Newest first; `None` sorts before any time, so files without one come last.
    files.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
    files.truncate(MAX_LISTED);
    FolderListing { folder: folder.to_string(), files, total, unavailable: false }
}

/// The last part of `folder`'s path, for headings and messages.
pub fn folder_name(folder: &str) -> String {
    std::path::Path::new(folder).file_name().map_or_else(|| folder.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Where `folder` is, in words, so two folders with the same name can be told apart without a
/// long path: "iCloud Drive", "OneDrive · Contoso", "Documents › Work", or the parent path with
/// the home folder as "~". The full path is shown on hover.
pub fn location(folder: &str) -> String {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(std::path::PathBuf::from);
    location_in(folder, home.as_deref())
}

/// [`location`] with the home folder given (tests).
pub fn location_in(folder: &str, home: Option<&std::path::Path>) -> String {
    use std::path::Path;
    let parent = Path::new(folder).parent().unwrap_or_else(|| Path::new(folder));
    let Some(rel) = home.and_then(|h| parent.strip_prefix(h).ok()) else {
        return parent.to_string_lossy().into_owned();
    };
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let (place, rest) = match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => return "~".to_string(),
        // macOS keeps iCloud Drive and File Provider cloud drives under ~/Library.
        ["Library", "Mobile Documents", "com~apple~CloudDocs", ..] => ("iCloud Drive".to_string(), 3),
        ["Library", "CloudStorage", drive, ..] => (cloud_drive(drive), 3),
        _ => (String::new(), 0),
    };
    let rest: Vec<&str> = parts.iter().skip(rest).map(String::as_str).collect();
    match (place.is_empty(), rest.is_empty()) {
        (true, _) => rest.join(" › "),
        (false, true) => place,
        (false, false) => format!("{place} › {}", rest.join(" › ")),
    }
}

/// A `~/Library/CloudStorage` folder name ("OneDrive-Contoso", "GoogleDrive-me@example.com",
/// "Dropbox") as the drive's name and, shortened, its account.
fn cloud_drive(folder: &str) -> String {
    let (provider, account) = folder.split_once('-').unwrap_or((folder, ""));
    let (provider, account) = match (provider, account.split_once('-')) {
        ("OneDrive", Some(("SharedLibraries", org))) => ("SharePoint", org),
        ("GoogleDrive", _) => ("Google Drive", account),
        _ => (provider, account),
    };
    // Business accounts are named after the organisation with its spaces removed, which can be
    // very long: its first part is enough to tell accounts apart.
    let account = account.split('-').next().unwrap_or(account);
    let account: String = if account.chars().count() > 32 { account.chars().take(31).chain(['…']).collect() } else { account.to_string() };
    if account.is_empty() { provider.to_string() } else { format!("{provider} · {account}") }
}

impl PdfCraftApp {
    /// File ▸ Pin folder to Home…: choose a folder whose newest PDFs Home lists.
    pub fn pin_folder_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Pin a folder to Home").to_string());
            self.ask_one(crate::pickers::Ask::Folder(dialog), None, |app, dir| {
                app.pin_folder(&dir.to_string_lossy());
            });
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Pinned folders need the desktop app");
    }

    /// Pin `folder` (an absolute path) to Home. Returns false, after telling the user, when it
    /// can't be pinned. Pinning a folder again changes nothing.
    pub fn pin_folder(&mut self, folder: &str) -> bool {
        let Some(folder) = clean(folder) else {
            self.notify_tr("Choose a folder on this computer to pin");
            return false;
        };
        let name = folder_name(&folder);
        if self.pinned.folders.contains(&folder) {
            self.notify_fmt("{folder} is already pinned to Home", &[("folder", &name)]);
            return true;
        }
        if self.pinned.folders.len() >= MAX_PINNED {
            self.notify_fmt("You can pin up to {n} folders. Unpin one first.", &[("n", &MAX_PINNED.to_string())]);
            return false;
        }
        self.pinned.folders.push(folder);
        self.pinned.stale = true;
        self.notify_fmt("Pinned {folder} to Home", &[("folder", &name)]);
        true
    }

    /// Remove `folder` from Home. Its files are not touched.
    pub fn unpin_folder(&mut self, folder: &str) {
        self.pinned.folders.retain(|f| f != folder);
        self.pinned.listings.retain(|l| l.folder != folder);
        self.pinned.expanded.retain(|f| f != folder);
    }

    /// List the pinned folders again if the last listing is older than a few seconds or the
    /// folders changed (called while Home shows; `now` is egui time). The listing runs on a
    /// worker thread, inline in tests ([`PdfCraftApp::run_inline`]).
    pub fn refresh_pinned(&mut self, now: f64) {
        self.poll_pinned();
        if self.pinned.scan.is_some() {
            return;
        }
        if self.pinned.folders.is_empty() {
            self.pinned.listings.clear();
            return;
        }
        // `now < last` means the clock restarted (a new context in tests): list again.
        let due = self.pinned.stale || self.pinned.last_scan.is_none_or(|last| now < last || now - last >= RESCAN_SECS);
        if !due {
            return;
        }
        self.pinned.stale = false;
        self.pinned.last_scan = Some(now);
        let folders = self.pinned.folders.clone();
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        let ctx = self.ctx.clone();
        let work = move || {
            let listings: Vec<FolderListing> = folders.iter().map(|f| list(f)).collect();
            *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(listings);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        if self.run_inline {
            work();
        } else if std::thread::Builder::new().name("pdfcraft-folders".into()).spawn(work).is_err() {
            // No worker: try again when the next listing is due.
            return;
        }
        self.pinned.scan = Some(slot);
        self.poll_pinned();
    }

    /// Take a finished listing, if there is one.
    fn poll_pinned(&mut self) {
        let Some(slot) = &self.pinned.scan else { return };
        let done = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        match done {
            Some(listings) => {
                self.pinned.scan = None;
                // A folder unpinned while it was being listed stays gone.
                let folders = &self.pinned.folders;
                self.pinned.listings = listings.into_iter().filter(|l| folders.contains(&l.folder)).collect();
            }
            // The worker ended without a result (it panicked): list again when due.
            None if Arc::strong_count(slot) == 1 => self.pinned.scan = None,
            None => {}
        }
    }
}

/// The Home view's Folders section (desktop only: the web build has no folders to read).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn section(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    use egui::{Align2, CornerRadius, Rect, Sense, vec2};

    use crate::theme::{self, Tokens};
    use crate::{icons, widgets};

    let t = Tokens::get(ui.ctx());
    app.refresh_pinned(ui.input(|i| i.time));
    let mut pin = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Folders")).font(theme::semibold(17.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            pin = widgets::ghost_button(ui, "folder-plus", tl!("Pin a folder…")).clicked();
        });
    });
    ui.add_space(4.0);
    if app.pinned.folders.is_empty() {
        ui.label(
            egui::RichText::new(tl!("Pin a folder to list its newest PDFs here, such as where your scanner or a synced cloud drive saves them."))
                .color(t.text_muted),
        );
    }
    let mut open = None;
    let mut unpin = None;
    let mut toggle = None;
    for folder in &app.pinned.folders {
        let name = folder_name(folder);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (icon, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            icons::paint(ui, icon, "folder", 20.0, t.icon);
            ui.label(egui::RichText::new(crate::bidi::visual(&name)).font(theme::semibold(14.0))).on_hover_text(folder);
            ui.add_space(4.0);
            // Leave room for Unpin: a long location is cut short, never the button.
            let room = (ui.available_width() - 110.0).max(0.0);
            ui.scope(|ui| {
                ui.set_max_width(room);
                let place = egui::RichText::new(crate::bidi::visual(&location(folder))).font(theme::regular(12.0)).color(t.text_faint);
                ui.add(egui::Label::new(place).truncate()).on_hover_text(folder);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let resp = widgets::ghost_button(ui, "x", tl!("Unpin"));
                // Every folder has an Unpin button: say which one it is.
                let label = crate::i18n::fmt(tl!("Unpin {folder}"), &[("folder", &name)]);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
                if resp.clicked() {
                    unpin = Some(folder.clone());
                }
            });
        });
        let note = |ui: &mut egui::Ui, text: &str| {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(text).color(t.text_muted));
        };
        let Some(listing) = app.pinned.listing(folder) else {
            note(ui, tl!("Looking for PDFs…"));
            continue;
        };
        if listing.unavailable {
            note(ui, tl!("This folder isn't available right now. It may have moved, or its drive or cloud sync is offline."));
            continue;
        }
        if listing.files.is_empty() {
            note(ui, tl!("No PDFs in this folder yet"));
            continue;
        }
        let expanded = app.pinned.expanded.contains(folder);
        let shown = if expanded { listing.files.len() } else { listing.files.len().min(SHOWN) };
        for f in listing.files.iter().take(shown) {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &f.name));
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(8), t.hover);
            }
            icons::paint(
                ui,
                Rect::from_min_size(rect.min + vec2(32.0, 9.0), vec2(20.0, 20.0)),
                "file-text",
                18.0,
                egui::Color32::from_rgb(0xE0, 0x3E, 0x3E),
            );
            ui.painter().text(rect.min + vec2(62.0, 19.0), Align2::LEFT_CENTER, crate::bidi::visual(&f.name), theme::medium(13.0), t.text);
            let size = crate::panels::human_size(usize::try_from(f.size).unwrap_or(usize::MAX));
            let when = f.modified.map(crate::combine_ui::ago).unwrap_or_default();
            let detail = if when.is_empty() { size } else { format!("{when}  ·  {size}") };
            ui.painter().text(rect.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, detail, theme::regular(12.0), t.text_muted);
            if resp.on_hover_text(&f.path).clicked() {
                open = Some(f.path.clone());
            }
        }
        let more = listing.files.len().saturating_sub(shown);
        let capped = listing.total > listing.files.len();
        if more > 0 || (expanded && (listing.files.len() > SHOWN || capped)) {
            ui.horizontal(|ui| {
                ui.add_space(30.0);
                let label = if expanded { tl!("Show less").to_string() } else { crate::i18n::fmt(tl!("Show {n} more"), &[("n", &more.to_string())]) };
                if (more > 0 || listing.files.len() > SHOWN) && ui.link(label).clicked() {
                    toggle = Some(folder.clone());
                }
                if expanded && capped {
                    let n = listing.files.len().to_string();
                    let text = crate::i18n::fmt(tl!("The newest {n} of {total} PDFs"), &[("n", &n), ("total", &listing.total.to_string())]);
                    ui.label(egui::RichText::new(text).color(t.text_faint));
                }
            });
        }
    }
    if pin {
        app.pin_folder_dialog();
    }
    if let Some(folder) = unpin {
        app.unpin_folder(&folder);
    }
    if let Some(folder) = toggle {
        if app.pinned.expanded.contains(&folder) {
            app.pinned.expanded.retain(|f| *f != folder);
        } else {
            app.pinned.expanded.push(folder);
        }
    }
    if let Some(path) = open {
        app.open_recent(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_from_settings_must_be_absolute_unique_and_few() {
        let mut p = PinnedFolders::default();
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        let a = format!("{root}scans");
        let many: Vec<String> = (0..20).map(|i| format!("{root}f{i}")).collect();
        let mut items = vec![
            serde_json::json!(5),
            serde_json::json!("relative/folder"),
            serde_json::json!(""),
            serde_json::json!(format!("{a}/")),
            serde_json::json!(a.clone()),
            serde_json::json!("x".repeat(MAX_PATH_CHARS + 1)),
        ];
        items.extend(many.iter().map(|f| serde_json::json!(f)));
        p.restore(&serde_json::Value::Array(items));
        assert_eq!(p.folders.len(), MAX_PINNED);
        assert_eq!(p.folders.first(), Some(&a), "trailing separator trimmed, duplicate dropped");
        assert!(p.folders.iter().all(|f| std::path::Path::new(f).is_absolute()));
        p.restore(&serde_json::json!("not a list"));
        assert_eq!(p.folders.len(), MAX_PINNED, "garbage keeps what is there");
    }

    #[test]
    fn the_root_keeps_its_separator() {
        if cfg!(windows) {
            assert_eq!(clean("C:\\").as_deref(), Some("C:\\"));
        } else {
            assert_eq!(clean("/").as_deref(), Some("/"));
            assert_eq!(clean("  /tmp/scans//  ").as_deref(), Some("/tmp/scans"));
        }
        assert_eq!(clean("a\0b"), None);
    }

    #[cfg(unix)]
    #[test]
    fn locations_are_said_in_words() {
        let home = Some(std::path::Path::new("/Users/me"));
        let at = |p: &str| location_in(p, home);
        assert_eq!(at("/Users/me/Library/Mobile Documents/com~apple~CloudDocs/Scans"), "iCloud Drive");
        assert_eq!(at("/Users/me/Library/Mobile Documents/com~apple~CloudDocs/Personal/Scans"), "iCloud Drive › Personal");
        assert_eq!(at("/Users/me/Library/CloudStorage/OneDrive-ContosoLtd-salesandservice/Scans"), "OneDrive · ContosoLtd");
        assert_eq!(at("/Users/me/Library/CloudStorage/OneDrive-Personal/Scans"), "OneDrive · Personal");
        assert_eq!(at("/Users/me/Library/CloudStorage/OneDrive-SharedLibraries-Contoso/Team/Scans"), "SharePoint · Contoso › Team");
        assert_eq!(at("/Users/me/Library/CloudStorage/GoogleDrive-me@example.com/My Drive/Scans"), "Google Drive · me@example.com › My Drive");
        assert_eq!(at("/Users/me/Library/CloudStorage/Dropbox/Scans"), "Dropbox");
        assert_eq!(at("/Users/me/Documents/Work/Scans"), "Documents › Work");
        assert_eq!(at("/Users/me/Scans"), "~");
        assert_eq!(at("/Volumes/NAS/Scans"), "/Volumes/NAS");
        assert_eq!(location_in("/Users/me/Documents/Scans", None), "/Users/me/Documents");
        let long = format!("/Users/me/Library/CloudStorage/OneDrive-{}/Scans", "x".repeat(50));
        assert_eq!(at(&long).chars().count(), "OneDrive · ".chars().count() + 32);
    }

    #[test]
    fn a_missing_folder_lists_as_unavailable() {
        let missing = std::env::temp_dir().join(format!("pdfcraft-no-such-folder-{}", std::process::id()));
        let l = list(&missing.to_string_lossy());
        assert!(l.unavailable);
        assert!(l.files.is_empty());
    }
}
