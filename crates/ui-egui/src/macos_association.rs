//! Best-effort recovery of an existing per-file PDFCraft binding after a successful open/save.
//! Quarantined documents with redirected bindings can be refused by LaunchServices before the
//! app receives an open event (Apple feedback FB19468486). In-app Open remains reachable.
//! Never remove quarantine, set type-wide defaults, or create a new preference on an open.

use std::{io, path::Path};

const OPEN_WITH: &str = "com.apple.LaunchServices.OpenWith";
const QUARANTINE: &str = "com.apple.quarantine";
const BUNDLE_ID: &str = "ai.storyteller.pdfcraft";
const MAX_ATTRIBUTE: usize = 64 * 1024;

/// A bounded snapshot used only to decide whether native association recovery is eligible.
#[derive(Default, PartialEq, Eq)]
struct OpeningMetadata {
    association: Option<Vec<u8>>,
    quarantine: Option<Vec<u8>>,
    identity: Option<(u64, u64)>,
}

impl OpeningMetadata {
    /// Capture one regular file identity and only read a binding when quarantine exists.
    fn read(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt as _;
        let identity = match path.symlink_metadata() {
            Ok(meta) if meta.is_file() => (meta.dev(), meta.ino()),
            Ok(_) => return Ok(Self::default()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let quarantine = read_attribute(path, QUARANTINE)?;
        let association = if quarantine.is_none() { None } else { read_attribute(path, OPEN_WITH)? };
        let snapshot = Self { association, quarantine, identity: Some(identity) };
        let current = path.symlink_metadata()?;
        if !current.is_file() || (current.dev(), current.ino()) != identity {
            return Err(io::Error::other("the file changed while reading its opening settings"));
        }
        Ok(snapshot)
    }
}

/// Read a last-component-symlink-safe attribute with a strict allocation cap.
fn read_attribute(path: &Path, name: &str) -> io::Result<Option<Vec<u8>>> {
    read_attribute_with(|buffer| rustix::fs::lgetxattr(path, name, buffer))
}

/// Probe the length before allocating. A concurrently grown value fails safely; unreadable
/// attributes or values beyond the parser cap skip recovery without affecting open/save.
fn read_attribute_with(mut get: impl FnMut(&mut [u8]) -> rustix::io::Result<usize>) -> io::Result<Option<Vec<u8>>> {
    use rustix::io::Errno;
    let unreadable = |error| match error {
        Errno::RANGE => Err(io::Error::new(io::ErrorKind::InvalidData, "opening metadata exceeds 64 KiB or changed while reading")),
        _ => Ok(None),
    };
    let len = match get(&mut []) {
        Ok(len) if len <= MAX_ATTRIBUTE => len,
        Ok(_) => return unreadable(Errno::RANGE),
        Err(e) => return unreadable(e),
    };
    let mut bytes = vec![0; len];
    match get(bytes.as_mut_slice()) {
        Ok(len) if len <= bytes.len() => {
            bytes.truncate(len);
            Ok(Some(bytes.into_boxed_slice().into_vec()))
        }
        Ok(_) => unreadable(Errno::RANGE),
        Err(e) => unreadable(e),
    }
}

/// Accept only the supported native schema identifying this exact running bundle.
fn points_to_this_app(data: &[u8], app: &Path) -> bool {
    if data.len() > MAX_ATTRIBUTE {
        return false;
    }
    // Deserialize just the small native schema rather than retaining an arbitrary plist tree.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Binding {
        version: u32,
        path: String,
        // Apple's API sometimes writes only version/path (e.g. an unregistered local bundle).
        // The exact path still has to match the running bundle whose identity we checked.
        bundleidentifier: Option<String>,
    }
    let Ok(binding) = plist::from_bytes::<Binding>(data) else { return false };
    if binding.version != 0 || binding.bundleidentifier.as_deref().is_some_and(|id| id != BUNDLE_ID) || !Path::new(&binding.path).is_absolute() {
        return false;
    }
    // Exact native spelling is intentionally conservative. Never resolve an untrusted path:
    // canonicalize could contact a stale network mount and block the main thread.
    Path::new(&binding.path) == app
}

/// Queue bounded, deduplicated recovery work without filesystem I/O on the UI thread.
/// The framework's safe NSWorkspace binding is callable on a worker; its completion remains
/// asynchronous. A stalled volume consumes at most one worker and a bounded queue.
pub(crate) fn repair(path: &Path) {
    use objc2_foundation::NSBundle;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, OnceLock, PoisonError, mpsc};

    if !objc2::available!(macos = 12.0)
        || objc2::MainThreadMarker::new().is_none()
        || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return;
    }
    let bundle = NSBundle::mainBundle();
    if bundle.bundleIdentifier().is_none_or(|id| id.to_string() != BUNDLE_ID) {
        return;
    }
    let Some(app) = bundle.bundleURL().to_file_path() else { return };
    static QUEUE: OnceLock<Option<RepairQueue>> = OnceLock::new();
    let Some(queue) = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<RepairJob>(32);
        let pending = Arc::new(Mutex::new(HashSet::new()));
        std::thread::Builder::new()
            .name("pdfcraft-association".into())
            .spawn(move || {
                for job in rx {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| objc2::rc::autoreleasepool(|_| repair_job(job)))).is_err() {
                        log::warn!("PDF association recovery skipped after a worker error");
                    }
                }
            })
            .ok()?;
        Some(RepairQueue { tx, pending })
    }) else {
        return;
    };
    let path = path.to_path_buf();
    {
        let mut pending = queue.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending.len() >= 32 || !pending.insert(path.clone()) {
            return;
        }
    }
    let job = RepairJob { app, pending: Pending { path, set: queue.pending.clone() } };
    let _ = queue.tx.try_send(job); // A refused send drops the job and releases its pending entry.
}

/// One process-wide worker and the paths queued or awaiting native completion.
struct RepairQueue {
    tx: std::sync::mpsc::SyncSender<RepairJob>,
    pending: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>>,
}

/// The exact running bundle captured on the main thread and its bounded pending entry.
struct RepairJob {
    app: std::path::PathBuf,
    pending: Pending,
}

/// Release deduplication on every exit, including errors and native callback destruction.
struct Pending {
    path: std::path::PathBuf,
    set: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.set.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.path);
    }
}

/// Read one identity-checked snapshot immediately before dispatch. No second xattr pass:
/// the native API has no compare-and-set, so a post-dispatch external change can still race it.
fn repair_job(job: RepairJob) {
    use block2::RcBlock;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSError, NSURL};
    let metadata = match OpeningMetadata::read(&job.pending.path) {
        Ok(metadata) => metadata,
        Err(_) => {
            log::warn!("PDF association recovery skipped: could not read opening metadata");
            return;
        }
    };
    if metadata.quarantine.is_none() || !metadata.association.as_deref().is_some_and(|data| points_to_this_app(data, &job.app)) {
        return;
    }
    let (Some(app_url), Some(file_url)) = (NSURL::from_file_path(&job.app), NSURL::from_file_path(&job.pending.path)) else { return };
    let pending = job.pending;
    let completion = RcBlock::new(move |error: *mut NSError| {
        let _ = &pending;
        if error.is_null() {
            log::info!("PDF association recovery completed");
        } else {
            log::warn!("PDF association recovery failed; document open/save remains successful");
        }
    });
    NSWorkspace::sharedWorkspace().setDefaultApplicationAtURL_toOpenFileAtURL_completionHandler(&app_url, &file_url, Some(&completion));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// Create an isolated scratch directory for parallel metadata tests.
    fn directory() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("pdfcraft-association-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        p
    }

    /// Build either native plist encoding with controlled identity fields.
    fn binding(app: &Path, id: &str, version: u32, binary: bool) -> Vec<u8> {
        let value = plist::Value::Dictionary(
            [
                ("version".to_string(), plist::Value::Integer(version.into())),
                ("path".to_string(), plist::Value::String(app.to_string_lossy().into_owned())),
                ("bundleidentifier".to_string(), plist::Value::String(id.into())),
            ]
            .into_iter()
            .collect(),
        );
        let mut bytes = Vec::new();
        if binary {
            value.to_writer_binary(&mut bytes).unwrap();
        } else {
            value.to_writer_xml(&mut bytes).unwrap();
        }
        bytes
    }

    /// Reject foreign apps, unsupported schema versions, malformed data and relative paths.
    #[test]
    fn requires_matching_identifier_path_and_native_schema() {
        let app = directory();
        let other = directory();
        for binary in [false, true] {
            assert!(points_to_this_app(&binding(&app, BUNDLE_ID, 0, binary), &app));
            assert!(!points_to_this_app(&binding(&app, "com.apple.Preview", 0, binary), &app));
            assert!(!points_to_this_app(&binding(&other, BUNDLE_ID, 0, binary), &app));
            assert!(!points_to_this_app(&binding(&app, BUNDLE_ID, 1, binary), &app));
        }
        let mut native = plist::from_bytes::<plist::Value>(&binding(&app, BUNDLE_ID, 0, true)).unwrap();
        native.as_dictionary_mut().unwrap().remove("bundleidentifier");
        let mut bytes = Vec::new();
        native.to_writer_binary(&mut bytes).unwrap();
        assert!(points_to_this_app(&bytes, &app), "Apple's path-only binding still identifies this exact bundle");
        assert!(!points_to_this_app(&bytes, &other));
        assert!(!points_to_this_app(b"not a plist", &app));
        assert!(!points_to_this_app(&vec![0; MAX_ATTRIBUTE + 1], &app));
        assert!(!points_to_this_app(&binding(Path::new("relative.app"), BUNDLE_ID, 0, true), &app));
        std::fs::remove_dir_all(app).unwrap();
        std::fs::remove_dir_all(other).unwrap();
    }

    /// Preserve destination values verbatim without interpreting them as new preferences.
    #[test]
    fn atomic_save_preserves_own_other_and_malformed_bindings_and_quarantine() {
        let dir = directory();
        let file = dir.join("file.pdf");
        for data in [binding(&dir, BUNDLE_ID, 0, true), binding(&dir, "com.apple.Preview", 0, true), b"malformed".to_vec()] {
            std::fs::write(&file, b"before").unwrap();
            xattr::set(&file, OPEN_WITH, &data).unwrap();
            xattr::set(&file, QUARANTINE, b"0082;1234;Preview;").unwrap();
            crate::editing::write_atomically(file.to_str().unwrap(), b"after").unwrap();
            assert_eq!(std::fs::read(&file).unwrap(), b"after");
            assert_eq!(xattr::get(&file, OPEN_WITH).unwrap(), Some(data));
            assert_eq!(xattr::get(&file, QUARANTINE).unwrap(), Some(b"0082;1234;Preview;".to_vec()));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A new Save As destination must not inherit a source association or quarantine.
    #[test]
    fn new_destination_does_not_acquire_an_association() {
        let dir = directory();
        let file = dir.join("new.pdf");
        crate::editing::write_atomically(file.to_str().unwrap(), b"pdf bytes").unwrap();
        assert!(xattr::get(&file, OPEN_WITH).unwrap().is_none());
        assert!(xattr::get(&file, QUARANTINE).unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The repair parser cap must not narrow upstream's larger preservation limits.
    #[test]
    fn oversized_binding_skips_repair_but_is_preserved_by_the_shared_writer() {
        let dir = directory();
        let file = dir.join("out.pdf");
        std::fs::write(&file, b"original PDF").unwrap();
        xattr::set(&file, OPEN_WITH, &vec![b'x'; MAX_ATTRIBUTE + 1]).unwrap();
        xattr::set(&file, QUARANTINE, b"0082;1234;Preview;").unwrap();
        assert!(OpeningMetadata::read(&file).is_err());
        crate::editing::write_atomically(file.to_str().unwrap(), b"new PDF").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new PDF");
        assert_eq!(xattr::get(&file, OPEN_WITH).unwrap().unwrap().len(), MAX_ATTRIBUTE + 1);
        assert_eq!(xattr::get(&file, QUARANTINE).unwrap(), Some(b"0082;1234;Preview;".to_vec()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Snapshots distinguish replaced inodes even with identical attribute bytes.
    #[test]
    fn preflight_refuses_a_replaced_file_even_with_identical_attributes() {
        let dir = directory();
        let file = dir.join("out.pdf");
        let replacement = dir.join("replacement.pdf");
        let prepare = |path: &Path| {
            std::fs::write(path, b"PDF").unwrap();
            xattr::set(path, OPEN_WITH, &binding(&dir, BUNDLE_ID, 0, true)).unwrap();
            xattr::set(path, QUARANTINE, b"0082;1234;Preview;").unwrap();
        };
        prepare(&file);
        let metadata = OpeningMetadata::read(&file).unwrap();
        assert!(OpeningMetadata::read(&file).unwrap() == metadata);
        prepare(&replacement);
        std::fs::rename(replacement, &file).unwrap();
        assert!(OpeningMetadata::read(&file).unwrap() != metadata, "the file successfully opened is no longer at this path");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Snapshots distinguish changed quarantine and ignore last-component symlinks.
    #[test]
    fn preflight_refuses_changed_quarantine_and_symbolic_links() {
        let dir = directory();
        let file = dir.join("out.pdf");
        std::fs::write(&file, b"PDF").unwrap();
        xattr::set(&file, OPEN_WITH, &binding(&dir, BUNDLE_ID, 0, true)).unwrap();
        xattr::set(&file, QUARANTINE, b"0082;1234;Preview;").unwrap();
        let metadata = OpeningMetadata::read(&file).unwrap();
        xattr::set(&file, QUARANTINE, b"0082;5678;another app;").unwrap();
        assert!(OpeningMetadata::read(&file).unwrap() != metadata);
        let link = dir.join("link.pdf");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        let linked = OpeningMetadata::read(&link).unwrap();
        assert!(linked.association.is_none() && linked.quarantine.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Do not resolve aliases or mount paths supplied in untrusted binding metadata.
    #[test]
    fn untrusted_bundle_aliases_are_skipped_without_resolving_them() {
        let dir = directory();
        let app = dir.join("PdfCraft.app");
        std::fs::create_dir(&app).unwrap();
        let alias = dir.join("alias.app");
        std::os::unix::fs::symlink(&app, &alias).unwrap();
        assert!(!points_to_this_app(&binding(&alias, BUNDLE_ID, 0, true), &app));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Check a real captured native binding and the packaged application identity.
    #[test]
    fn native_binding_and_packaging_identity_match() {
        // Captured via Apple's API on a synthetic PDF on macOS 27.0.1. Contains only the
        // public installed bundle path, identifier and schema version; no document data.
        let hex = "62706c6973743030d30102030405065776657273696f6e54706174685f101062756e646c656964656e74696669657210005f101a2f4170706c69636174696f6e732f50646643726166742e6170705f101761692e73746f727974656c6c65722e7064666372616674080f171c2f314e0000000000000101000000000000000700000000000000000000000000000068";
        let captured: Vec<u8> =
            hex.as_bytes().as_chunks::<2>().0.iter().map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect();
        assert!(points_to_this_app(&captured, Path::new("/Applications/PdfCraft.app")));
        let package = plist::from_bytes::<plist::Value>(include_bytes!("../../../packaging/macos/Info.plist.in")).unwrap();
        assert_eq!(package.as_dictionary().unwrap().get("CFBundleIdentifier").unwrap().as_string(), Some(BUNDLE_ID));
    }

    /// Exercise arbitrary and damaged metadata through the strict native parser.
    #[test]
    fn malformed_plists_and_truncated_native_schemas_do_not_panic() {
        let app = Path::new("/Applications/PdfCraft.app");
        for binary in [false, true] {
            let native = binding(app, BUNDLE_ID, 0, binary);
            for end in 0..native.len() {
                let _ = points_to_this_app(native.get(..end).unwrap(), app);
            }
            for index in 0..native.len() {
                for byte in [0, 0xff, b'<', b'&'] {
                    let mut mutated = native.clone();
                    *mutated.get_mut(index).unwrap() = byte;
                    let _ = points_to_this_app(&mutated, app);
                }
            }
        }
        let mut state = 0x125a_9430_8c97_ae13u64;
        for len in 0..1024 {
            let data: Vec<_> = (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect();
            let _ = points_to_this_app(&data, app);
        }
    }

    /// Length probing allocates only the value, rejects oversize before a second call,
    /// and skips recovery if the value grows between the probe and read.
    #[test]
    fn attribute_probes_bound_allocation_and_detect_growth() {
        let mut calls = 0;
        let small = read_attribute_with(|out| {
            calls += 1;
            if calls == 1 {
                assert!(out.is_empty());
                return Ok(3);
            }
            assert_eq!(out.len(), 3);
            out.copy_from_slice(b"abc");
            Ok(3)
        })
        .unwrap()
        .unwrap();
        assert_eq!(small, b"abc");
        assert_eq!(small.capacity(), 3);
        assert!(read_attribute_with(|_| Ok(MAX_ATTRIBUTE + 1)).is_err());
        let mut calls = 0;
        assert!(
            read_attribute_with(|_| {
                calls += 1;
                if calls == 1 { Ok(3) } else { Err(rustix::io::Errno::RANGE) }
            })
            .is_err()
        );
    }

    /// Volumes may expose no readable attributes with errors beyond ENOTSUP.
    #[test]
    fn unreadable_attributes_are_best_effort() {
        use rustix::io::Errno;
        for errno in [Errno::NOSYS, Errno::INVAL, Errno::PERM, Errno::ACCESS, Errno::IO] {
            assert_eq!(read_attribute_with(|_| Err(errno)).unwrap(), None);
        }
    }

    /// Skip unavailable attributes while retaining the hard limit on known oversized values.
    #[test]
    fn unsupported_attribute_volumes_are_distinct_from_read_failures() {
        use rustix::io::Errno;
        for errno in [Errno::NOATTR, Errno::NOTSUP, Errno::OPNOTSUPP] {
            assert_eq!(read_attribute_with(|_| Err(errno)).unwrap(), None, "{errno}");
        }
        assert!(read_attribute_with(|_| Err(Errno::RANGE)).is_err());
    }
}
