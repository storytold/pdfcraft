//! Reload a document when its file changes on disk (#431): a LaTeX or Typst build rewriting the
//! PDF that is open shows its new version without reopening it.
//!
//! Cheap by design. One worker thread looks at each open file's metadata (no reading) when there
//! may be something new to see: when the system says something changed in the file's folder
//! (FSEvents on macOS, inotify on Linux, ReadDirectoryChangesW on Windows), and every [`IDLE`]
//! for a file it can't say that about: one on a network or FUSE volume, which another computer can
//! change without the system seeing it, one whose folder is missing or couldn't be watched, and
//! every file on other systems. So while nothing changes the worker doesn't wake at all, and it
//! blocks without waking while no open document has a file. A changed file is read only once it
//! has stopped changing ([`SETTLE`] apart) and ends like a whole PDF: its last KiB is read first,
//! so a file still being written costs one small read. The new bytes go to the UI thread, which is
//! woken for them: a clean document is reloaded in place, keeping its tab, page, zoom and scroll;
//! one with unsaved changes gets a notice instead. PdfCraft's own saves are recognised by the
//! file's stamp after writing, so they never reload.

use std::collections::HashMap;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use pdfcraft_engine::DocId;

use crate::PdfCraftApp;

/// How often a file is looked at when the system can't say it changed (see the module docs).
pub const IDLE: Duration = Duration::from_millis(250);
/// How long a changed file must stay the same before it is read: writers pause between chunks.
pub const SETTLE: Duration = Duration::from_millis(40);
/// How much of the end of a changed file is read to see whether it is a whole PDF.
const TAIL: usize = 1024;

/// One version of a file, told apart without reading it: its length and modification time, and
/// what the platform adds (Unix: device, inode and change time; Windows: creation and last-write
/// times in 100 ns ticks). The modification time alone is coarse on FAT and some network
/// drives, and a rebuild can keep the length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    extra: [u64; 4],
}

impl Stamp {
    fn of(meta: &Metadata) -> Self {
        #[cfg(unix)]
        let extra = {
            use std::os::unix::fs::MetadataExt;
            [meta.dev(), meta.ino(), meta.ctime().cast_unsigned(), meta.ctime_nsec().cast_unsigned()]
        };
        #[cfg(windows)]
        let extra = {
            use std::os::windows::fs::MetadataExt;
            [meta.creation_time(), meta.last_write_time(), 0, 0]
        };
        #[cfg(not(any(unix, windows)))]
        let extra = [0; 4];
        Self { len: meta.len(), modified: meta.modified().ok(), extra }
    }

    /// Only Unix change-time differs: metadata may have changed without a content edit.
    fn metadata_only_candidate(self, other: Self) -> bool {
        #[cfg(unix)]
        {
            self.len == other.len && self.modified == other.modified && self.extra.get(..2) == other.extra.get(..2)
        }
        #[cfg(not(unix))]
        {
            let _ = other;
            false
        }
    }

    /// The file at `path` now; `None` when it is missing, not a file or can't be read.
    pub fn at(path: &str) -> Option<Self> {
        std::fs::metadata(path).ok().filter(Metadata::is_file).map(|m| Self::of(&m))
    }
}

/// A file to watch, as the UI thread sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Watched {
    pub doc: DocId,
    pub path: String,
    /// The version PdfCraft last read or wrote (`None`: the first one the watcher sees).
    pub known: Option<Stamp>,
    /// Which reading or writing of the file by PdfCraft this is for (an open, a save, Save As):
    /// a version the watcher found for an earlier one is older than the document is now.
    pub epoch: u64,
}

/// A new version of a watched file, read and ready to open.
pub struct Changed {
    pub doc: DocId,
    /// The file it was read from, and the epoch it was watched for (see [`Watched`]).
    pub path: String,
    pub epoch: u64,
    pub stamp: Stamp,
    pub bytes: Arc<Vec<u8>>,
}

/// The watcher's record of one file.
struct Entry {
    doc: DocId,
    path: String,
    epoch: u64,
    known: Option<Stamp>,
    /// A changed version seen once, waiting to be seen unchanged again.
    pending: Option<Stamp>,
    /// The system reports changes to this file ([`Notifications::follow`]), so it isn't looked at
    /// every [`IDLE`].
    notified: bool,
}

/// Looks at the watched files: on the worker thread, or on the UI thread in tests.
#[derive(Default)]
pub struct Watcher {
    files: Vec<Entry>,
}

impl Watcher {
    /// Watch `list` from now on. A file watched already keeps its pending change; a version the
    /// UI thread knows (it just read or saved the file) replaces the watcher's own.
    fn set(&mut self, list: Vec<Watched>) {
        let old = std::mem::take(&mut self.files);
        self.files = list
            .into_iter()
            .map(|w| {
                let prev = old.iter().find(|e| e.doc == w.doc && e.path == w.path);
                Entry {
                    known: w.known.or(prev.and_then(|e| e.known)),
                    pending: prev.and_then(|e| e.pending),
                    notified: false,
                    doc: w.doc,
                    path: w.path,
                    epoch: w.epoch,
                }
            })
            .collect();
    }

    /// Look at every file once: the new versions found, and whether a file is still changing
    /// (look again after [`SETTLE`] rather than [`IDLE`]).
    fn tick(&mut self) -> (Vec<Changed>, bool) {
        let mut found = Vec::new();
        let mut settling = false;
        for e in &mut self.files {
            // Missing (deleted, or being replaced) or unreadable: the document stays as it is.
            let Some(now) = Stamp::at(&e.path) else {
                e.pending = None;
                continue;
            };
            let known = *e.known.get_or_insert(now);
            if now == known {
                e.pending = None;
                continue;
            }
            if e.pending != Some(now) {
                // Changed since the last look: read it once it stops changing.
                e.pending = Some(now);
                settling = true;
                continue;
            }
            e.pending = None;
            match read_whole_pdf(&e.path, now) {
                Version::Ready(bytes) => {
                    e.known = Some(now);
                    found.push(Changed { doc: e.doc, path: e.path.clone(), epoch: e.epoch, stamp: now, bytes });
                }
                // Not a whole PDF in this version (a build that failed or paused): wait for the
                // next one.
                Version::Incomplete => e.known = Some(now),
                // Changed again while being read, or held by its writer: look again soon.
                Version::Busy => settling = true,
            }
        }
        (found, settling)
    }
}

/// Verify a metadata-only candidate without allocating a second PDF. The stamp must stay
/// fixed throughout; changed bytes, a replacement or any read failure still count as a change.
fn contents_match(path: &str, stamp: Stamp, expected: &[u8]) -> bool {
    let Ok(mut file) = File::open(path) else { return false };
    if !file.metadata().is_ok_and(|m| Stamp::of(&m) == stamp) {
        return false;
    }
    let mut buffer = [0u8; 8192];
    for chunk in expected.chunks(buffer.len()) {
        let Some(out) = buffer.get_mut(..chunk.len()) else { return false };
        if file.read_exact(out).is_err() || out != chunk {
            return false;
        }
    }
    let mut tail = [0];
    matches!(file.read(&mut tail), Ok(0)) && file.metadata().is_ok_and(|m| Stamp::of(&m) == stamp) && Stamp::at(path) == Some(stamp)
}

/// What a changed file turned out to be.
enum Version {
    /// A whole PDF, read.
    Ready(Arc<Vec<u8>>),
    /// Not a whole PDF: a build that failed or paused.
    Incomplete,
    /// Changing, replaced or held by its writer: look again.
    Busy,
}

/// Read `path` if it is still the version `stamp` and ends like a whole PDF (`%%EOF` in its last
/// KiB, read first). The whole file is then read into a buffer of exactly its size.
fn read_whole_pdf(path: &str, stamp: Stamp) -> Version {
    // Windows refuses to open a file its writer holds exclusively: not ready yet.
    let Ok(mut file) = File::open(path) else { return Version::Busy };
    let Ok(meta) = file.metadata() else { return Version::Busy };
    if Stamp::of(&meta) != stamp {
        return Version::Busy;
    }
    let len = meta.len();
    let mut end = [0u8; TAIL];
    let tail = usize::try_from(len).map_or(TAIL, |l| l.min(TAIL));
    let (Some(end), Ok(back)) = (end.get_mut(..tail), i64::try_from(tail)) else { return Version::Incomplete };
    if file.seek(SeekFrom::End(-back)).is_err() || file.read_exact(end).is_err() {
        return Version::Busy;
    }
    if !end.windows(5).any(|w| w == b"%%EOF") {
        return Version::Incomplete;
    }
    let mut bytes = Vec::new();
    // Too big to hold: the open version stays.
    let Ok(size) = usize::try_from(len) else { return Version::Incomplete };
    if bytes.try_reserve_exact(size).is_err() {
        return Version::Incomplete;
    }
    if file.seek(SeekFrom::Start(0)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return Version::Busy;
    }
    // Still that version, under that name: a writer may have replaced the file meanwhile.
    if bytes.len() != size || Stamp::at(path) != Some(stamp) {
        return Version::Busy;
    }
    Version::Ready(Arc::new(bytes))
}

/// What wakes the worker thread.
enum Wake {
    /// The files to watch now, from the UI thread.
    Files(Vec<Watched>),
    /// The system says something changed in the folder of a watched file, or that it lost track
    /// of what changed: look.
    Notified,
    /// The app is gone (see [`FileWatch`]'s `Drop`).
    Stop,
}

/// The worker thread: watches the newest list the UI thread sent and hands back new versions,
/// waking the UI for them. It looks at the files when the system says one may have changed, every
/// [`IDLE`] while one of them can't be followed that way, and [`SETTLE`] after a look that found
/// a file still changing; otherwise it sleeps. `wake` is its own inbox, for the system's
/// notifications. It ends on [`Wake::Stop`], or when its inbox is gone.
fn run(inbox: mpsc::Receiver<Wake>, wake: mpsc::Sender<Wake>, out: Arc<Mutex<Vec<Changed>>>, looks: Arc<AtomicU64>, ctx: egui::Context) {
    let mut watcher = Watcher::default();
    let mut notifications = Notifications::new(wake);
    // When the files are to be looked at next; `None`: only when something says so.
    let mut due: Option<Instant> = None;
    // The last look, and whether it found a file still changing.
    let mut last: Option<Instant> = None;
    let mut settling = false;
    loop {
        let first = match due {
            None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(at) => inbox.recv_timeout(at.saturating_duration_since(Instant::now())),
        };
        let (mut listed, mut notified) = (false, false);
        for message in std::iter::once(first).chain(std::iter::from_fn(|| inbox.try_recv().ok().map(Ok))) {
            match message {
                Ok(Wake::Files(list)) => {
                    watcher.set(list);
                    listed = true;
                }
                Ok(Wake::Notified) => notified = true,
                Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        let now = Instant::now();
        if listed {
            // Stop following folders no open file is in any more, and follow the new ones.
            notifications.follow(&mut watcher.files);
            if watcher.files.is_empty() {
                (due, settling) = (None, false);
                continue;
            }
            // A file not seen before is stamped at once.
            due = Some(now);
        } else if notified && !settling {
            // No sooner than SETTLE after the last look: a folder that never stops changing costs
            // one look per SETTLE, not one per change. A file still settling is looked at when
            // its SETTLE is up anyway.
            let at = last.map_or(now, |l| (l + SETTLE).max(now));
            due = Some(due.map_or(at, |d| d.min(at)));
        }
        let Some(at) = due else { continue };
        if at > now || watcher.files.is_empty() {
            continue;
        }
        looks.fetch_add(1, Ordering::Relaxed);
        last = Some(now);
        // Changes from here on ring again; the ones before are what this look sees.
        notifications.heard();
        let (found, still) = watcher.tick();
        settling = still;
        if !found.is_empty() {
            out.lock().unwrap_or_else(PoisonError::into_inner).extend(found);
            ctx.request_repaint();
        }
        // A file that appeared or went, or a folder made anew, changes what is followed.
        notifications.follow(&mut watcher.files);
        due = if settling {
            Some(now + SETTLE)
        } else if watcher.files.iter().any(|e| !e.notified) {
            Some(now + IDLE)
        } else {
            None
        };
    }
}

use notifications::Notifications;

/// The system's file change notifications, for the folders of the watched files: FSEvents on
/// macOS, inotify on Linux, ReadDirectoryChangesW on Windows (through `notify`).
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
mod notifications {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use notify::Watcher as _;

    use super::{Entry, IDLE, Wake};

    pub(super) struct Notifications {
        /// `None`: the system gave none, and every file is looked at every [`IDLE`].
        watcher: Option<notify::RecommendedWatcher>,
        /// The folders followed, each with what it was when following started: inotify and
        /// ReadDirectoryChangesW follow a folder, not its name, so one deleted and made again
        /// is followed anew.
        folders: HashMap<PathBuf, Option<[u64; 2]>>,
        /// Set by the first notification since the worker last looked: a burst of them sends one
        /// [`Wake::Notified`], not one each.
        rung: Arc<AtomicBool>,
    }

    impl Notifications {
        pub(super) fn new(wake: mpsc::Sender<Wake>) -> Self {
            let rung = Arc::new(AtomicBool::new(false));
            let bell = rung.clone();
            // Any event (or an error: some were lost) is a reason to look; the stamps tell what
            // changed, so a change is never missed for a path spelled differently.
            let watcher = notify::recommended_watcher(move |_: notify::Result<notify::Event>| {
                if !bell.swap(true, Ordering::AcqRel) {
                    let _ = wake.send(Wake::Notified);
                }
            });
            if let Err(e) = &watcher {
                log::warn!("no file change notifications ({e}): open files are looked at every {} ms", IDLE.as_millis());
            }
            Self { watcher: watcher.ok(), folders: HashMap::new(), rung }
        }

        /// The worker is about to look: the next notification wakes it again.
        pub(super) fn heard(&self) {
            self.rung.store(false, Ordering::Release);
        }

        /// Follow the folders of `files`, and only those, and mark the files whose changes are
        /// notified: every folder the file is in is followed, and its volume reports every change.
        pub(super) fn follow(&mut self, files: &mut [Entry]) {
            let Some(watcher) = self.watcher.as_mut() else {
                files.iter_mut().for_each(|e| e.notified = false);
                return;
            };
            let per_file: Vec<Vec<PathBuf>> = files.iter().map(|e| folders_of(Path::new(&e.path))).collect();
            let mut wanted: HashMap<&Path, Option<[u64; 2]>> = HashMap::new();
            for folder in per_file.iter().flatten() {
                wanted.entry(folder.as_path()).or_insert_with(|| identity(folder));
            }
            // Folders no file is in any more, or made anew since they were followed.
            self.folders.retain(|folder, was| {
                let keep = wanted.get(folder.as_path()).is_some_and(|now| now == was);
                if !keep {
                    let _ = watcher.unwatch(folder);
                }
                keep
            });
            for (folder, id) in wanted {
                if !self.folders.contains_key(folder) && watcher.watch(folder, notify::RecursiveMode::NonRecursive).is_ok() {
                    self.folders.insert(folder.to_path_buf(), id);
                }
            }
            for (e, folders) in files.iter_mut().zip(&per_file) {
                e.notified = !folders.is_empty() && folders.iter().all(|f| self.folders.contains_key(f) && reports_changes(f));
            }
        }
    }

    /// The folders whose notifications cover a change to `path`: its own folder, and, when it is
    /// a link, the folder of the file it leads to, which is what a build rewrites. Real paths, as
    /// notifications name them; none when the folder is missing.
    fn folders_of(path: &Path) -> Vec<PathBuf> {
        let own = path.parent().filter(|p| !p.as_os_str().is_empty()).and_then(|p| std::fs::canonicalize(p).ok());
        let target = std::fs::canonicalize(path).ok().and_then(|p| p.parent().map(Path::to_path_buf));
        let mut out: Vec<PathBuf> = own.into_iter().collect();
        if let Some(target) = target.filter(|t| !out.contains(t)) {
            out.push(target);
        }
        out
    }

    /// Tells a folder from one made again under its name.
    fn identity(folder: &Path) -> Option<[u64; 2]> {
        let meta = std::fs::metadata(folder).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some([meta.dev(), meta.ino()])
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            Some([meta.creation_time(), 0])
        }
    }

    /// Whether the system is told of every change to the files in `folder`: not on a network
    /// share or a FUSE volume, whose files another computer (or the FUSE server) can change
    /// without going through this one, so their changes are looked for every [`IDLE`].
    #[cfg(target_os = "macos")]
    pub(super) fn reports_changes(folder: &Path) -> bool {
        // `MNT_LOCAL` in <sys/mount.h>: "filesystem is stored locally". SMB, AFP, NFS, WebDAV and
        // FUSE volumes (unless mounted `local`) don't set it.
        const MNT_LOCAL: u32 = 0x0000_1000;
        rustix::fs::statfs(folder).is_ok_and(|s| s.f_flags & MNT_LOCAL != 0)
    }

    /// Whether the system is told of every change to the files in `folder`: not on a network
    /// file system or a FUSE volume, whose files another computer (or the FUSE server) can change
    /// without going through this one, so their changes are looked for every [`IDLE`].
    #[cfg(target_os = "linux")]
    pub(super) fn reports_changes(folder: &Path) -> bool {
        // statfs(2) `f_type`s, from <linux/magic.h> (and Lustre's): NFS, SMB, CIFS, SMB2, FUSE,
        // 9P (WSL drives, VM shares), Ceph, AFS (two), Coda, NCP, OCFS2, GFS2, VirtualBox
        // shared folders, Lustre.
        const ELSEWHERE: &[u32] = &[
            0x6969,
            0x517B,
            0xFF53_4D42,
            0xFE53_4D42,
            0x6573_5546,
            0x0102_1997,
            0x00C3_6400,
            0x5346_414F,
            0x6B41_4653,
            0x7375_7245,
            0x564C,
            0x7461_636F,
            0x0116_1970,
            0x786F_4256,
            0x0BD0_0BD0,
        ];
        // `f_type` is a signed word on most architectures; the magic numbers are 32 bits, and
        // the ones past `i32::MAX` (CIFS, SMB2) arrive sign-extended.
        rustix::fs::statfs(folder).is_ok_and(|s| !ELSEWHERE.contains(&(s.f_type as u32)))
    }

    /// Whether the system is told of every change to the files in `folder`. On Windows
    /// ReadDirectoryChangesW reports changes on network shares too.
    #[cfg(windows)]
    pub(super) fn reports_changes(_: &Path) -> bool {
        true
    }
}

/// No file change notifications on this system: every file is looked at every [`IDLE`].
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod notifications {
    use std::sync::mpsc;

    use super::{Entry, Wake};

    pub(super) struct Notifications;

    impl Notifications {
        pub(super) fn new(_: mpsc::Sender<Wake>) -> Self {
            Self
        }

        pub(super) fn heard(&self) {}

        pub(super) fn follow(&mut self, files: &mut [Entry]) {
            files.iter_mut().for_each(|e| e.notified = false);
        }
    }
}

/// The app's side of the watcher.
#[derive(Default)]
pub struct FileWatch {
    /// The worker's inbox (`None` until a document with a file is open).
    inbox: Option<mpsc::Sender<Wake>>,
    found: Arc<Mutex<Vec<Changed>>>,
    /// How many times the worker has looked at the files.
    looks: Arc<AtomicU64>,
    /// What the watcher was last told.
    sent: Vec<Watched>,
    /// In tests ([`PdfCraftApp::run_inline`]) there is no thread: each frame looks once.
    inline: Watcher,
    /// The version of each document's file that PdfCraft last read or wrote.
    known: HashMap<DocId, Stamp>,
    /// The version each document came from (its open, reload or last save). Unlike `known`,
    /// a version found on disk and not applied (the document had unsaved changes) is not
    /// recorded here, so Save can tell that the file is not the one the document came from.
    basis: HashMap<DocId, Stamp>,
    /// How many times PdfCraft has read or written each document's file: a version the watcher
    /// found before the latest one is older than the document in memory, and is not applied
    /// (#1802).
    epoch: HashMap<DocId, u64>,
    /// The notice bar's Reload was pressed for this document.
    pub reload_request: Option<DocId>,
}

impl Drop for FileWatch {
    /// The worker's own notifications keep its inbox open, so it is told to end.
    fn drop(&mut self) {
        if let Some(inbox) = self.inbox.take() {
            let _ = inbox.send(Wake::Stop);
        }
    }
}

impl FileWatch {
    /// How many times the worker has looked at the files.
    #[cfg(test)]
    fn looks(&self) -> u64 {
        self.looks.load(Ordering::Relaxed)
    }

    /// PdfCraft read or wrote `path` for `doc`: that version is not a change, and anything the
    /// watcher found before it is stale.
    pub fn known_now(&mut self, doc: DocId, path: &str) {
        self.known_as(doc, Stamp::at(path));
    }

    /// Remember `stamp`, read before the file was, for `doc`.
    pub fn known_as(&mut self, doc: DocId, stamp: Option<Stamp>) {
        if let Some(stamp) = stamp {
            self.known.insert(doc, stamp);
            self.basis.insert(doc, stamp);
        }
        self.bump(doc);
    }

    /// Whether the file at `path` is another version than the one `doc` came from: something
    /// else wrote it since the document was opened, reloaded or last saved (#2166). A file
    /// that is missing or can't be looked at, or a document whose version was never recorded,
    /// is not a difference.
    pub fn disk_differs(&self, doc: DocId, path: &str, saved: Option<Arc<Vec<u8>>>) -> bool {
        match (Stamp::at(path), self.basis.get(&doc)) {
            (Some(now), Some(basis)) => {
                now != *basis && !(now.metadata_only_candidate(*basis) && saved.is_some_and(|bytes| contents_match(path, now, &bytes)))
            }
            _ => false,
        }
    }

    fn bump(&mut self, doc: DocId) {
        let epoch = self.epoch.entry(doc).or_default();
        *epoch = epoch.wrapping_add(1);
    }

    fn epoch(&self, doc: DocId) -> u64 {
        self.epoch.get(&doc).copied().unwrap_or(0)
    }
}

impl PdfCraftApp {
    /// The documents to watch: every open one with a file, while the preference is on.
    fn watched(&self) -> impl Iterator<Item = (DocId, &str, Option<Stamp>, u64)> {
        self.views.iter().filter(|_| self.reload_changed_files).filter_map(|v| {
            let path = self.session.get(v.id)?.path.as_deref()?;
            Some((v.id, path, self.watch.known.get(&v.id).copied(), self.watch.epoch(v.id)))
        })
    }

    /// Every frame: tell the watcher about files opened, closed or saved (without allocating
    /// when nothing changed), and reload what changed on disk.
    pub(crate) fn watch_files(&mut self) {
        if let Some(id) = self.watch.reload_request.take() {
            self.reload_from_disk(id);
        }
        let mut sent = self.watch.sent.iter();
        let unchanged = self
            .watched()
            .all(|(doc, path, known, epoch)| sent.next().is_some_and(|w| w.doc == doc && w.path == path && w.known == known && w.epoch == epoch))
            && sent.next().is_none();
        if !unchanged {
            let list: Vec<Watched> = self.watched().map(|(doc, path, known, epoch)| Watched { doc, path: path.to_owned(), known, epoch }).collect();
            let open: Vec<DocId> = self.views.iter().map(|v| v.id).collect();
            self.watch.known.retain(|doc, _| open.contains(doc));
            self.watch.basis.retain(|doc, _| open.contains(doc));
            self.watch.epoch.retain(|doc, _| open.contains(doc));
            if self.send_to_watcher(list.clone()) {
                self.watch.sent = list;
            }
        }
        if self.run_inline {
            let (found, _) = self.watch.inline.tick();
            self.watch.found.lock().unwrap_or_else(PoisonError::into_inner).extend(found);
        }
        let found = std::mem::take(&mut *self.watch.found.lock().unwrap_or_else(PoisonError::into_inner));
        for c in found {
            self.file_changed(c);
        }
    }

    /// Hand `list` to the watcher, starting it when needed. `false`: not delivered (no context
    /// yet, or the worker couldn't start or ended); the next frame tries again.
    fn send_to_watcher(&mut self, list: Vec<Watched>) -> bool {
        if self.run_inline {
            self.watch.inline.set(list);
            return true;
        }
        if self.watch.inbox.is_none() {
            if list.is_empty() {
                return true;
            }
            let Some(ctx) = self.ctx.clone() else { return false };
            let (tx, rx) = mpsc::channel();
            let (wake, out, looks) = (tx.clone(), self.watch.found.clone(), self.watch.looks.clone());
            if std::thread::Builder::new().name("pdfcraft-watch".into()).spawn(move || run(rx, wake, out, looks, ctx)).is_err() {
                return false;
            }
            self.watch.inbox = Some(tx);
        }
        let delivered = self.watch.inbox.as_ref().is_some_and(|tx| tx.send(Wake::Files(list)).is_ok());
        if !delivered {
            // The worker ended (it panicked): start a new one next frame.
            self.watch.inbox = None;
        }
        delivered
    }

    /// A watched file has a new version: reload its document, or ask first when it has unsaved
    /// changes.
    fn file_changed(&mut self, c: Changed) {
        let Some(i) = self.views.iter().position(|v| v.id == c.doc) else { return };
        // Found for an earlier reading or writing of the file, or for another file than the
        // document's now (a save, Save As or a reopen came between): older than the document in
        // memory, so it is not applied over it (#1802). The watcher was told the newer version.
        let same_file = self.session.get(c.doc).and_then(|d| d.path.as_deref()) == Some(c.path.as_str());
        if !same_file || self.watch.epoch(c.doc) != c.epoch {
            return;
        }
        // PdfCraft wrote this version itself (a save), or it was handled already.
        if self.watch.known.get(&c.doc) == Some(&c.stamp) {
            return;
        }
        // LaunchServices recovery (and Finder metadata edits) changes ctime. The worker
        // already read the stable file: identical saved bytes are not a document change.
        if self.session.get(c.doc).is_some_and(|d| d.saved_bytes().as_slice() == c.bytes.as_slice()) {
            self.watch.known_as(c.doc, Some(c.stamp));
            return;
        }
        self.watch.known.insert(c.doc, c.stamp);
        if self.has_unsaved_work(i) {
            if let Some(view) = self.views.get_mut(i) {
                view.disk_changed = true;
            }
            return;
        }
        self.reload_view(i, c.bytes);
    }

    /// The notice bar's Reload: read the file now and replace the document, unsaved changes and
    /// all.
    fn reload_from_disk(&mut self, doc: DocId) {
        let Some(i) = self.views.iter().position(|v| v.id == doc) else { return };
        let Some(path) = self.session.get(doc).and_then(|d| d.path.clone()) else { return };
        let stamp = Stamp::at(&path);
        match std::fs::read(&path) {
            Ok(bytes) => {
                self.watch.known_as(doc, stamp);
                self.reload_view(i, Arc::new(bytes));
            }
            Err(e) => {
                let name = self.session.get(doc).map(|d| d.name.clone()).unwrap_or_default();
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
    }

    /// Open `bytes` in place of tab `i`'s document, keeping the tab and the reader's place. A
    /// version that can't be opened leaves the document as it was, and says so.
    fn reload_view(&mut self, i: usize, bytes: Arc<Vec<u8>>) {
        let Some(id) = self.views.get(i).map(|v| v.id) else { return };
        match self.session.reload(id, bytes) {
            Ok(()) => {
                // Whatever was unsaved is gone, so there is nothing left to recover.
                self.forget_recovery(id);
                if let (Some(view), Some(doc)) = (self.views.get_mut(i), self.session.get(id)) {
                    view.document_replaced(&doc.info);
                }
                // What the form's scripts said while it opened, as on opening.
                let out = self.session.take_js_output(id);
                self.handle_js(id, out);
            }
            Err(e) => {
                let name = self.session.get(id).map(|d| d.name.clone()).unwrap_or_default();
                self.notify_fmt("Couldn't reload {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use egui_kittest::Harness;

    use super::*;
    use crate::SaveTarget;

    /// An `n`-page PDF with a proper xref table.
    fn fixture(n: usize) -> Vec<u8> {
        let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
        for _ in 0..n {
            objs.push("<< /Type /Page /Parent 2 0 R >>".into());
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// A fresh temporary folder with `thesis.pdf` in it, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(test: &str, bytes: &[u8]) -> (Self, String) {
            let dir = std::env::temp_dir().join(format!("pdfcraft-watch-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("thesis.pdf").to_string_lossy().into_owned();
            std::fs::write(&path, bytes).unwrap();
            (Self(dir), path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Rewrite `path` as a build does, with a later modification time.
    fn rebuild(path: &str, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(60)).unwrap();
    }

    fn harness(path: &str) -> Harness<'static, PdfCraftApp> {
        let path = path.to_owned();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.run_inline = true;
            app.open_path(&path);
            app
        });
        h.run_steps(4);
        h
    }

    /// Version B of the open file, found by the watcher but not yet applied: queued as the worker
    /// thread queues what it finds between frames.
    fn find_new_version(app: &mut PdfCraftApp, path: &str, bytes: &[u8]) -> Vec<Changed> {
        rebuild(path, bytes);
        // One look sees the change, the next sees it settled and reads it.
        let (mut found, _) = app.watch.inline.tick();
        found.extend(app.watch.inline.tick().0);
        assert_eq!(found.len(), 1, "the watcher found the new version");
        found
    }

    fn doc<'a>(h: &'a Harness<'static, PdfCraftApp>) -> &'a pdfcraft_engine::Document {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap()
    }

    /// Association recovery changes ctime without replacing PDF contents. It must neither
    /// discard undo history nor make the next Save ask to overwrite our own metadata change.
    #[cfg(target_os = "macos")]
    #[test]
    fn association_metadata_does_not_reload_or_block_save() {
        let (_dir, path) = TempDir::new("association-metadata", &fixture(2));
        let mut h = harness(&path);
        let app = h.state_mut();
        app.views[0].select_pages(&[0]);
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        xattr::set(&path, "com.apple.quarantine", b"2082;1234;Preview;").unwrap();
        // Exercise Save before the worker has consumed the metadata change.
        assert!(h.state_mut().save_active(SaveTarget::InPlace), "ctime alone must not cause an overwrite prompt");
        xattr::set(&path, "com.apple.quarantine", b"2082;5678;Preview;").unwrap();
        h.run_steps(4);
        assert!(doc(&h).can_undo().is_some(), "metadata-only events must not reload and clear history");
        assert!(!h.state().views[0].disk_changed);
    }

    /// The repair parser's 64 KiB cap must not override the shared writer's larger limit.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_large_binding_does_not_block_a_document_save() {
        let (_dir, path) = TempDir::new("large-binding-save", &fixture(2));
        xattr::set(&path, "com.apple.LaunchServices.OpenWith", &vec![b'x'; 65537]).unwrap();
        xattr::set(&path, "com.apple.quarantine", b"0082;1234;Preview;").unwrap();
        let mut h = harness(&path);
        let app = h.state_mut();
        app.views[0].select_pages(&[0]);
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        assert!(app.save_active(SaveTarget::InPlace), "the document save uses upstream preservation limits");
        assert_eq!(xattr::get(&path, "com.apple.LaunchServices.OpenWith").unwrap().unwrap().len(), 65537);
        assert_eq!(xattr::get(&path, "com.apple.quarantine").unwrap(), Some(b"0082;1234;Preview;".to_vec()));
    }

    /// Retaining mtime and length must not hide an actual external edit behind metadata noise.
    #[cfg(unix)]
    #[test]
    fn same_length_and_mtime_content_edits_still_require_overwrite_confirmation() {
        let (_dir, path) = TempDir::new("ctime-content", &fixture(2));
        let mut h = harness(&path);
        let old = std::fs::metadata(&path).unwrap().modified().unwrap();
        let changed = String::from_utf8(fixture(2)).unwrap().replace("200 300", "201 300");
        std::fs::write(&path, changed).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        let app = h.state_mut();
        app.views[0].select_pages(&[0]);
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        assert!(!app.save_active(SaveTarget::InPlace));
        assert!(matches!(app.dialog, Some(crate::Dialog::OverwriteChanged)));
    }

    /// #1802: the watcher found version B of the file, then the user saved version C over it
    /// before B was applied. B is older than the document now: it is not applied, the undo history
    /// stays, and C stays on disk.
    #[test]
    fn a_version_found_before_a_save_is_not_applied_over_it() {
        let (_dir, path) = TempDir::new("stale-save", &fixture(2));
        let mut h = harness(&path);
        let stale = find_new_version(h.state_mut(), &path, &fixture(3));
        let app = h.state_mut();
        app.views[0].select_pages(&[0]);
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        // B is on disk, not the A the document came from: Save asks first (#2166), and the
        // user saves C over it.
        assert!(!app.save_active(SaveTarget::InPlace), "asks before writing over B");
        app.overwrite_changed_file();
        assert!(!doc(&h).dirty, "saved version C");
        let app = h.state_mut();
        app.watch.found.lock().unwrap().extend(stale);
        h.run_steps(3);
        let d = doc(&h);
        assert_eq!(d.info.pages.len(), 2, "the stale version was not applied over the saved one");
        assert!(d.can_undo().is_some() && !d.dirty, "the saved document and its history stay");
        assert_eq!(Stamp::at(&path), h.state().watch.known.get(&h.state().views[0].id).copied(), "C is still on disk");
    }

    /// #1802: version B found under the old path, then Save As to another file: B is not applied
    /// to the document under its new path.
    #[test]
    fn a_version_found_before_save_as_is_not_applied_to_the_new_file() {
        let (dir, path) = TempDir::new("stale-save-as", &fixture(2));
        let mut h = harness(&path);
        let stale = find_new_version(h.state_mut(), &path, &fixture(3));
        let new_path = dir.0.join("copy.pdf").to_string_lossy().into_owned();
        let app = h.state_mut();
        app.views[0].select_pages(&[0]);
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        app.save_override = Some(new_path.clone());
        assert!(app.save_active(SaveTarget::As), "saved as the new file");
        app.watch.found.lock().unwrap().extend(stale);
        h.run_steps(3);
        let d = doc(&h);
        assert_eq!(d.path.as_deref(), Some(new_path.as_str()));
        assert_eq!(d.info.pages.len(), 2, "the old file's version was not applied to the new one");
        assert!(d.can_undo().is_some() && !d.dirty, "the saved document and its history stay");
    }

    /// Dropping the app's side tells the worker to end: its own notifications keep its inbox open.
    #[test]
    fn dropping_the_app_side_tells_the_worker_to_end() {
        let (tx, rx) = mpsc::channel();
        let mut watch = FileWatch::default();
        watch.inbox = Some(tx);
        drop(watch);
        assert!(matches!(rx.try_recv(), Ok(Wake::Stop)));
    }

    /// The worker thread on its own, with the system's change notifications, as the app runs it.
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    mod worker {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Arc, Mutex, mpsc};
        use std::thread::JoinHandle;
        use std::time::{Duration, Instant};

        use pdfcraft_engine::DocId;

        use super::super::{Changed, IDLE, Stamp, Wake, Watched, notifications, run};
        use super::{TempDir, fixture, rebuild};

        /// Long enough for a change notification on a busy machine; a poll every [`IDLE`] looks four
        /// times in it.
        const QUIET: Duration = Duration::from_secs(1);

        struct Worker {
            inbox: mpsc::Sender<Wake>,
            found: Arc<Mutex<Vec<Changed>>>,
            looks: Arc<AtomicU64>,
            thread: Option<JoinHandle<()>>,
        }

        impl Worker {
            fn start() -> Self {
                let (inbox, rx) = mpsc::channel();
                let (found, looks) = (Arc::<Mutex<Vec<Changed>>>::default(), Arc::<AtomicU64>::default());
                let (wake, out, counter) = (inbox.clone(), found.clone(), looks.clone());
                let thread = std::thread::spawn(move || run(rx, wake, out, counter, egui::Context::default()));
                Self { inbox, found, looks, thread: Some(thread) }
            }

            fn watch(&self, path: &str, known: Option<Stamp>) {
                let file = Watched { doc: DocId(1), path: path.to_owned(), known, epoch: 0 };
                self.inbox.send(Wake::Files(vec![file])).unwrap();
            }

            fn looks(&self) -> u64 {
                self.looks.load(Ordering::Relaxed)
            }

            /// How many looks the worker takes in `QUIET`, after any late notification of the
            /// test's own writing has come in.
            fn looks_while_quiet(&self) -> u64 {
                std::thread::sleep(Duration::from_millis(300));
                let before = self.looks();
                std::thread::sleep(QUIET);
                self.looks() - before
            }

            fn until(&self, what: &str, done: impl Fn(&Self) -> bool) {
                let end = Instant::now() + Duration::from_secs(20);
                while !done(self) {
                    assert!(Instant::now() < end, "timed out waiting until {what}");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }

            fn next_version(&self) -> Changed {
                self.until("a new version is found", |w| !w.found.lock().unwrap().is_empty());
                self.found.lock().unwrap().remove(0)
            }
        }

        impl Drop for Worker {
            fn drop(&mut self) {
                let _ = self.inbox.send(Wake::Stop);
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
            }
        }

        /// The worker doesn't look at an unchanged file at all: it waits for the system to say its
        /// folder changed, and then reads the new version as before.
        #[test]
        fn the_worker_rests_until_the_system_says_the_file_changed() {
            let (_dir, path) = TempDir::new("notified", &fixture(2));
            let worker = Worker::start();
            worker.watch(&path, Stamp::at(&path));
            worker.until("the file is first looked at", |w| w.looks() >= 1);
            let quiet = worker.looks_while_quiet();
            assert!(quiet <= 1, "{quiet} looks at an unchanged file in {QUIET:?} (every {IDLE:?} would be 4)");
            let before = worker.looks();
            rebuild(&path, &fixture(3));
            let found = worker.next_version();
            assert_eq!((found.path.as_str(), found.bytes.as_slice()), (path.as_str(), fixture(3).as_slice()));
            assert_eq!(Some(found.stamp), Stamp::at(&path));
            let looks = worker.looks() - before;
            assert!(looks <= 8, "a few looks for one build, not a poll: {looks}");
        }

        /// A file the system can't report on (here: its folder is gone, as a clean build leaves
        /// it) is looked at every IDLE, as before. Once it is back its folder is followed and the
        /// looking stops.
        #[test]
        fn a_file_without_notifications_is_looked_at_until_its_folder_can_be_followed() {
            let (dir, _) = TempDir::new("unfollowed", &fixture(1));
            let folder = dir.0.join("build");
            let path = folder.join("thesis.pdf").to_string_lossy().into_owned();
            let worker = Worker::start();
            // The document came from a version that is gone now.
            worker.watch(&path, Some(Stamp { len: 1, modified: None, extra: [0; 4] }));
            let polled = worker.looks_while_quiet();
            assert!(polled >= 2, "looked at every {IDLE:?} with no folder to follow: {polled} looks in {QUIET:?}");
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(&path, fixture(3)).unwrap();
            assert_eq!(worker.next_version().bytes.as_slice(), fixture(3).as_slice());
            let quiet = worker.looks_while_quiet();
            assert!(quiet <= 1, "the folder is followed now: {quiet} looks in {QUIET:?}");
        }

        /// The worker ends when told to, with its notifications.
        #[test]
        fn a_stopped_worker_ends() {
            let (_dir, path) = TempDir::new("stop", &fixture(1));
            let mut worker = Worker::start();
            worker.watch(&path, Stamp::at(&path));
            worker.until("the file is first looked at", |w| w.looks() >= 1);
            worker.inbox.send(Wake::Stop).unwrap();
            let thread = worker.thread.take().unwrap();
            let end = Instant::now() + Duration::from_secs(20);
            while !thread.is_finished() {
                assert!(Instant::now() < end, "the worker didn't end");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// A local folder reports its changes (a temporary folder is local), and a missing one
        /// can't be asked.
        #[test]
        fn a_local_folder_reports_its_changes() {
            let (dir, _) = TempDir::new("local", &fixture(1));
            let folder = std::fs::canonicalize(&dir.0).unwrap();
            assert!(notifications::reports_changes(&folder));
            #[cfg(unix)]
            assert!(!notifications::reports_changes(&folder.join("missing")));
        }
    }

    /// The app reloads a rebuilt file from the system's notification, its worker resting until
    /// then.
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    #[test]
    fn a_rebuilt_file_reloads_from_the_systems_notification() {
        let (_dir, path) = TempDir::new("app-notified", &fixture(2));
        let mut h = harness(&path);
        // Opened on the test's thread; from here the worker thread watches, as in the app.
        h.state_mut().run_inline = false;
        h.state_mut().watch.sent.clear();
        let id = h.state().views[0].id;
        let end = std::time::Instant::now() + Duration::from_secs(20);
        while h.state().watch.looks() == 0 {
            assert!(std::time::Instant::now() < end, "the worker never looked");
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        let before = h.state().watch.looks();
        std::thread::sleep(Duration::from_secs(1));
        let quiet = h.state().watch.looks() - before;
        assert!(quiet <= 1, "{quiet} looks at an unchanged file in a second");
        rebuild(&path, &fixture(3));
        while doc(&h).info.pages.len() != 3 {
            assert!(std::time::Instant::now() < end + Duration::from_secs(10), "the rebuilt file never reloaded");
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!((h.state().views.len(), h.state().views[0].id), (1, id), "in the same tab");
    }
}
