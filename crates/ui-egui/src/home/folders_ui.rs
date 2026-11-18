//! Pinned folders on the Home view: the newest PDFs in folders the user chose (where a scanner
//! saves, or a cloud-synced folder such as iCloud Drive, OneDrive or Dropbox), one click away.
//!
//! Local only: a pinned folder is read through the file system like any other path, never
//! through a storage provider's API, and nothing about it leaves the machine. Folders are
//! listed on the job queue and at most every few seconds while Home shows, so a slow, huge or
//! offline synced folder never blocks the frame.

use std::time::SystemTime;

use craft::host::PickRequest;
use craft::host::queue::Priority;
use craft::i18n::AgeStyle;
use craft::ui::runtime::env::Env;
use craft::ui::shell::{FolderListing as ShellListing, PinnedFile, PinnedFolder};

use crate::PdfCraftApp;

/// At most this many folders can be pinned.
pub const MAX_PINNED: usize = 8;
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
    /// Folders folded down to their heading (#5295); kept in settings.
    pub collapsed: Vec<String>,
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
        self.collapsed.clear();
        self.stale = true;
    }

    /// Restore which folders are collapsed, after [`Self::restore`]: only folders that are
    /// pinned, each once, so untrusted settings can't grow the list.
    pub(crate) fn restore_collapsed(&mut self, value: &serde_json::Value) {
        let Some(items) = value.as_array() else { return };
        self.collapsed.clear();
        for folder in items.iter().filter_map(serde_json::Value::as_str) {
            if self.folders.iter().any(|f| f == folder) && !self.collapsed.iter().any(|f| f == folder) {
                self.collapsed.push(folder.to_string());
            }
        }
    }

    /// Fold pinned `folder` down to its heading (`collapsed`) or open it again. Returns false,
    /// changing nothing, when `folder` isn't pinned.
    pub(crate) fn set_collapsed(&mut self, folder: &str, collapsed: bool) -> bool {
        if !self.folders.iter().any(|f| f == folder) {
            return false;
        }
        self.collapsed.retain(|f| f != folder);
        if collapsed {
            self.collapsed.push(folder.to_string());
        }
        true
    }

    /// Fold `folder` if it shows its files, open it if it is folded.
    pub(crate) fn toggle_collapsed(&mut self, folder: &str) {
        let collapsed = self.collapsed.iter().any(|f| f == folder);
        self.set_collapsed(folder, !collapsed);
    }

    /// Show all of pinned `folder`'s files if it shows the first few, the first few if it shows all.
    pub(crate) fn toggle_expanded(&mut self, folder: &str) {
        if !self.folders.iter().any(|f| f == folder) {
            return;
        }
        if self.expanded.iter().any(|f| f == folder) {
            self.expanded.retain(|f| f != folder);
        } else {
            self.expanded.push(folder.to_string());
        }
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
        self.pick_first(PickRequest::folder("pin-folder").title(tl!("Pin a folder to Home").to_string()), None, |app, _, dir| {
            app.pin_folder(&dir.to_string_lossy());
        });
        // The web has no folder path to pin.
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
        self.pinned.collapsed.retain(|f| f != folder);
    }

    /// `ui.set folder-collapsed <folder>=on|off` (CodeRabbit on #5341): the control channel's
    /// way to fold a pinned folder down to its heading or open it again, as its chevron does.
    /// `ui.state` lists the pinned folders and which are collapsed. The folder is named by its
    /// path as pinned; the last `=` splits it from the state, so a path may hold `=` too.
    pub(crate) fn set_folder_collapsed(&mut self, value: &str) -> Result<(), String> {
        let (folder, state) = value.rsplit_once('=').ok_or("folder-collapsed must be <folder>=on or <folder>=off")?;
        let collapsed = match state {
            "on" => true,
            "off" => false,
            _ => return Err("folder-collapsed must be <folder>=on or <folder>=off".into()),
        };
        if self.pinned.set_collapsed(folder, collapsed) {
            Ok(())
        } else {
            Err(format!("folder-collapsed: {folder} is not a pinned folder (ui.state lists them)"))
        }
    }

    /// The pinned folders as the start screen lists them: named apart where two share a name,
    /// with what the last listing found. `env` words the age of each file.
    pub(crate) fn pinned_folders(&self, env: &Env) -> Vec<PinnedFolder> {
        self.pinned
            .folders
            .iter()
            .map(|folder| {
                let base = folder_name(folder);
                let shared = self.pinned.folders.iter().any(|f| f != folder && folder_name(f) == base);
                let name = if shared { format!("{base} ({})", location(folder)) } else { base };
                let listing = match self.pinned.listing(folder) {
                    None => ShellListing::Pending,
                    Some(l) if l.unavailable => ShellListing::Unavailable,
                    Some(l) => ShellListing::Listed { files: l.files.iter().take(MAX_LISTED).map(|f| pinned_file(f, env)).collect(), total: l.total },
                };
                PinnedFolder::new(folder.as_str(), name)
                    .listing(listing)
                    .folded(self.pinned.collapsed.contains(folder))
                    .expanded(self.pinned.expanded.contains(folder))
            })
            .collect()
    }

    /// List the pinned folders again if the last listing is older than a few seconds or the
    /// folders changed (called while Home shows; `now` is egui time). The listing runs on the
    /// app's job queue; a job that delivers nothing is made up for by the next due listing.
    pub fn refresh_pinned(&mut self, now: f64) {
        self.poll_pinned();
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
        // PLAN: assumed field `pinned_jobs: JobQueue<(), Vec<FolderListing>>` on PdfCraftApp (pdf-hub-w1).
        // A panicking listing delivers nothing; the next due listing makes up for it.
        self.pinned_jobs.submit((), Priority::BACKGROUND, move |_| {
            pdfcraft_engine::guard(|| folders.iter().map(|f| list(f)).collect::<Vec<FolderListing>>()).unwrap_or_default()
        });
    }

    /// Take the newest finished listing, if there is one.
    fn poll_pinned(&mut self) {
        let mut latest = None;
        while let Some(done) = self.pinned_jobs.try_recv() {
            latest = Some(done.result);
        }
        if let Some(listings) = latest {
            // A folder unpinned while it was being listed stays gone.
            let folders = &self.pinned.folders;
            self.pinned.listings = listings.into_iter().filter(|l| folders.contains(&l.folder)).collect();
        }
    }
}

/// `file` as a row of a pinned folder: its name, with its age and size under it.
fn pinned_file(file: &FolderFile, env: &Env) -> PinnedFile {
    let size = crate::panels::human_size(usize::try_from(file.size).unwrap_or(usize::MAX));
    let modified = file.modified.and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs());
    let note = match modified {
        Some(seconds) => format!("{} · {size}", env.relative_age(seconds, AgeStyle::Short)),
        None => size,
    };
    PinnedFile::new(file.path.as_str(), file.name.as_str()).note(note)
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
