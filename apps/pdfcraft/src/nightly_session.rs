//! App-private Nightly session health, not telemetry or a Launcher protocol.
//! A live owner holds a separate persistent lease; secondary launches never rewrite its record.
//! Dropping the guard does not clear the marker. Only the app's controlled return path does.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

const RECORD: &str = "nightly-session.json";
const LEASE: &str = "nightly-session.lock";
const MAX_METADATA_BYTES: usize = 256;
/// Tried names for the staged record; an existing entry never gets opened, followed or removed.
const STAGE_ATTEMPTS: u32 = 64;

pub struct Session {
    _lease: File,
    path: PathBuf,
    own_record: Vec<u8>,
    previous_unclean: bool,
}

fn regular_or_missing(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(io::Error::other("Nightly session state must be a regular file, not an alias or directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Write `bytes` to `path` without ever following a link at the destination: stage a private
/// (`0600`) file under a fresh name in the same folder, then rename it over the name. A rename
/// replaces the link itself, never what it points at.
fn atomic_write_no_follow(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // A directory at the name cannot be replaced; a link can, and the rename replaces the link
    // itself rather than whatever it points at. The caller has already refused a planted alias.
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir()) {
        return Err(io::Error::other("Nightly session state must be a file, not a directory"));
    }
    let dir = path.parent().ok_or_else(|| io::Error::other("Nightly session state has no folder"))?;
    let name = path.file_name().ok_or_else(|| io::Error::other("Nightly session state has no name"))?;
    let mut staged = None;
    for attempt in 0..STAGE_ATTEMPTS {
        let candidate = dir.join(format!(".{}.stage-{}-{attempt}", name.to_string_lossy(), std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                staged = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let Some((staged, mut file)) = staged else {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "every staged Nightly session name is taken"));
    };
    let written = file.write_all(bytes).and_then(|()| file.flush()).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&staged, path) {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    // The directory entry itself must be durable, or a crash can lose the record's removal.
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The owner's session, shared by the app's exit hook (macOS) and the code after the event loop.
/// On macOS every quit ends the process inside AppKit's terminate, after eframe's `on_exit` and
/// before `eframe::run_native` could return, so the exit hook finishes it there; the return path
/// still runs for a start that created no window.
pub type Shared = Arc<Mutex<Option<Session>>>;

/// The controlled return ([`Session::finish`]) by whichever path comes first; later calls do nothing.
pub fn finish_once(shared: &Shared, unclean_notice_shown: bool) {
    let watch = shared.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(watch) = watch
        && let Err(error) = watch.finish(unclean_notice_shown)
    {
        log::warn!("Nightly session state was left intact: {error}");
    }
}

impl Session {
    /// The warning belongs to this window: a busy secondary has no health-record coverage,
    /// while Stable and a lease-owning Nightly window preserve their existing result.
    pub fn begin_with_health(root: Option<&Path>, version: &str, commit: Option<&str>) -> io::Result<(Option<Self>, bool)> {
        let watch = Self::begin(root, version, commit)?;
        let unavailable = root.is_some() && watch.is_none();
        Ok((watch, unavailable))
    }

    /// `None` is stable or another owner of this profile. Neither case touches the record.
    /// The supplied root must already be the validated private Nightly profile.
    pub fn begin(root: Option<&Path>, version: &str, commit: Option<&str>) -> io::Result<Option<Self>> {
        let Some(root) = root else { return Ok(None) };
        if version.is_empty() || version.len() > MAX_METADATA_BYTES || commit.is_some_and(|value| value.len() > MAX_METADATA_BYTES) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Nightly session provenance is missing or too large"));
        }
        let lease_path = root.join(LEASE);
        regular_or_missing(&lease_path)?;
        let lease = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(lease_path)?;
        match lease.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        let path = root.join(RECORD);
        let previous_unclean = regular_or_missing(&path)?;
        let own_record = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "channel": "nightly-v1",
            "version": version,
            "commit": commit.filter(|value| !value.is_empty()),
        }))?;
        atomic_write_no_follow(&path, &own_record)?;
        Ok(Some(Self { _lease: lease, path, own_record, previous_unclean }))
    }

    /// Whether an earlier session left its record behind: the caller shows the notice for it.
    pub fn previous_unclean(&self) -> bool {
        self.previous_unclean
    }

    /// Controlled return. If an earlier session's interruption was detected but no window showed
    /// it yet (a renderer failure before the app callback, including the OpenGL retry), the
    /// record is left for the next owner to report; otherwise this is [`Self::complete`].
    pub fn finish(self, unclean_notice_shown: bool) -> io::Result<()> {
        if self.previous_unclean && !unclean_notice_shown {
            return Ok(());
        }
        self.complete()
    }

    /// Controlled event-loop completion includes a handled startup error/renderer handoff, not only
    /// exit 0. Clear before releasing primary IPC so a successor cannot miss record ownership.
    /// An unwind, abort, forced exit or killed process never calls this method.
    pub fn complete(self) -> io::Result<()> {
        if !regular_or_missing(&self.path)? {
            return Ok(());
        }
        let mut current = Vec::new();
        File::open(&self.path)?.take(self.own_record.len() as u64 + 1).read_to_end(&mut current)?;
        if current != self.own_record {
            return Err(io::Error::other("Nightly session state changed; leaving it intact"));
        }
        std::fs::remove_file(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let path =
                    std::env::temp_dir().join(format!("pdfcraft-nightly-session-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create owned fixture: {error}"),
                }
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn busy_secondary_reports_unavailable_without_replacing_the_owner_record() {
        let fixture = Fixture::new();
        let (stable, warning) = Session::begin_with_health(None, "stable", None).unwrap();
        assert!(stable.is_none() && !warning);
        let (first, warning) = Session::begin_with_health(Some(&fixture.0), "nightly", Some("owned-commit")).unwrap();
        assert!(!warning);
        let first = first.unwrap();
        let record = std::fs::read(fixture.0.join(RECORD)).unwrap();
        let (secondary, warning) = Session::begin_with_health(Some(&fixture.0), "secondary", None).unwrap();
        assert!(secondary.is_none() && warning, "a secondary window cannot claim recorded exit/crash coverage");
        assert_eq!(std::fs::read(fixture.0.join(RECORD)).unwrap(), record);
        first.complete().unwrap();
        assert!(!fixture.0.join(RECORD).exists());
    }

    #[test]
    fn controlled_completion_is_clean_but_an_unfinished_owner_is_reported_next_time() {
        let fixture = Fixture::new();
        let first = Session::begin(Some(&fixture.0), "first", Some("recorded-commit")).unwrap().unwrap();
        assert!(!first.previous_unclean());
        let own = std::fs::read(fixture.0.join(RECORD)).unwrap();
        assert!(Session::begin(Some(&fixture.0), "secondary", None).unwrap().is_none());
        assert_eq!(std::fs::read(fixture.0.join(RECORD)).unwrap(), own, "a busy secondary must not replace the owner's record");
        first.complete().unwrap();
        assert!(!fixture.0.join(RECORD).exists());
        let unfinished = Session::begin(Some(&fixture.0), "second", None).unwrap().unwrap();
        assert!(!unfinished.previous_unclean());
        drop(unfinished); // Unwind/abnormal-path ownership release, not a controlled completion.
        let next = Session::begin(Some(&fixture.0), "third", None).unwrap().unwrap();
        assert!(next.previous_unclean());
        next.complete().unwrap();
        assert!(!fixture.0.join(RECORD).exists());
    }

    #[test]
    fn a_crash_is_reported_by_the_next_window_and_then_cleared() {
        let fixture = Fixture::new();
        // A window that never reached its controlled exit (a crash, a forced kill): it drops its
        // session instead of finishing it, so the record stays.
        drop(Session::begin(Some(&fixture.0), "crashed", None).unwrap().unwrap());
        let reporting = Session::begin(Some(&fixture.0), "reporting", None).unwrap().unwrap();
        assert!(reporting.previous_unclean(), "the next window must be told about the interrupted one");
        reporting.finish(true).unwrap();
        assert!(!fixture.0.join(RECORD).exists(), "the window that showed the notice clears the record");
        let clean = Session::begin(Some(&fixture.0), "clean", None).unwrap().unwrap();
        assert!(!clean.previous_unclean(), "a reported crash must not be reported again");
        clean.finish(true).unwrap();
        assert!(!fixture.0.join(RECORD).exists());
    }

    #[test]
    fn an_interruption_no_window_showed_survives_a_handled_startup_failure() {
        let fixture = Fixture::new();
        drop(Session::begin(Some(&fixture.0), "crashed", None).unwrap().unwrap());
        // Renderer initialization failed before the app callback: no notice was shown.
        let failed = Session::begin(Some(&fixture.0), "gpu-failed", None).unwrap().unwrap();
        assert!(failed.previous_unclean());
        failed.finish(false).unwrap();
        // The OpenGL retry (or the next start) still sees the interruption and shows it.
        let retry = Session::begin(Some(&fixture.0), "gl-retry", None).unwrap().unwrap();
        assert!(retry.previous_unclean());
        retry.finish(true).unwrap();
        assert!(!fixture.0.join(RECORD).exists(), "a session that showed the notice clears its record");
        // Without an earlier interruption, a handled failure still clears normally.
        let clean = Session::begin(Some(&fixture.0), "clean", None).unwrap().unwrap();
        assert!(!clean.previous_unclean());
        clean.finish(false).unwrap();
        assert!(!fixture.0.join(RECORD).exists());
    }

    #[test]
    fn the_exit_hook_and_the_return_path_finish_the_session_once() {
        let fixture = Fixture::new();
        let shared: Shared = Arc::new(Mutex::new(Session::begin(Some(&fixture.0), "quitting", None).unwrap()));
        // The exit hook (macOS quit: the only path that runs there).
        finish_once(&shared, false);
        assert!(!fixture.0.join(RECORD).exists(), "a quit that ran the exit hook is a clean exit");
        // Elsewhere the return path runs too, by then possibly after a successor took the profile.
        let successor = Session::begin(Some(&fixture.0), "successor", None).unwrap().unwrap();
        assert!(!successor.previous_unclean());
        let record = std::fs::read(fixture.0.join(RECORD)).unwrap();
        finish_once(&shared, false);
        assert_eq!(std::fs::read(fixture.0.join(RECORD)).unwrap(), record, "the second finish must not touch a successor's record");
        successor.complete().unwrap();
    }

    #[test]
    fn cleanup_never_removes_an_unknown_changed_record() {
        let fixture = Fixture::new();
        let watch = Session::begin(Some(&fixture.0), "nightly", None).unwrap().unwrap();
        std::fs::write(fixture.0.join(RECORD), b"replacement owned by someone else").unwrap();
        assert!(watch.complete().is_err());
        assert_eq!(std::fs::read(fixture.0.join(RECORD)).unwrap(), b"replacement owned by someone else");
    }

    #[test]
    fn provenance_is_bounded_and_never_collects_documents_paths_or_diagnostics() {
        let fixture = Fixture::new();
        assert!(Session::begin(Some(&fixture.0), &"x".repeat(MAX_METADATA_BYTES + 1), None).is_err());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
        assert!(Session::begin(None, "", None).unwrap().is_none(), "stable has no session-health side effects");
        let watch = Session::begin(Some(&fixture.0), "nightly", None).unwrap().unwrap();
        let data: serde_json::Value = serde_json::from_slice(&std::fs::read(fixture.0.join(RECORD)).unwrap()).unwrap();
        let fields = data.as_object().unwrap().keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(fields, ["schema_version", "channel", "version", "commit"].into_iter().collect());
        assert!(data["commit"].is_null(), "missing provenance must not be invented");
        watch.complete().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn reserved_state_aliases_do_not_modify_external_files() {
        let fixture = Fixture::new();
        let external = fixture.0.join("stable-document");
        std::fs::write(&external, b"stable bytes").unwrap();
        for name in [LEASE, RECORD] {
            let alias = fixture.0.join(name);
            std::os::unix::fs::symlink(&external, &alias).unwrap();
            assert!(Session::begin(Some(&fixture.0), "nightly", None).is_err());
            assert_eq!(std::fs::read(&external).unwrap(), b"stable bytes");
            std::fs::remove_file(alias).unwrap();
        }
    }

    #[test]
    #[cfg(unix)]
    fn the_staged_write_leaves_no_file_behind_and_keeps_its_mode_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let fixture = Fixture::new();
        let path = fixture.0.join("written.json");
        atomic_write_no_follow(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        atomic_write_no_follow(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second", "a rename replaces the name, not an owner");
        let staged = std::fs::read_dir(&fixture.0)
            .unwrap()
            .filter_map(|e| Some(e.ok()?.file_name()))
            .filter(|n| n.to_string_lossy().contains(".stage-"))
            .count();
        assert_eq!(staged, 0, "no staged file outlives the write");
    }
}
