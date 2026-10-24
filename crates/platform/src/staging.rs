//! Staging files for atomic saves: a new, unpredictably named file beside the target, written and
//! then renamed over it. Writing, closing, renaming and cleaning up stay with the caller.

use std::path::{Path, PathBuf};

/// How many staging names [`create_staging`] tries. A random 64-bit name is only taken if someone
/// put a file there on purpose, so running out means refusing, not trying harder.
pub const STAGING_ATTEMPTS: usize = 16;

/// Where the suffix goes in a staging name. Each caller keeps the form it has always used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingName {
    /// `.{name}.{suffix}.pdfcraft-tmp`, used by the automation tools.
    SuffixThenTag,
    /// `.{name}.pdfcraft-{suffix}.tmp`, used by the desktop app's save.
    TagThenSuffix,
}

/// Create a new, empty staging file in `dir` for the file `name`, one name per suffix. It is
/// opened with `create_new`, which fails if anything already has the name (a file, a hard link,
/// a symbolic link even when dangling, a folder), on Windows as everywhere else; such a name is
/// skipped, never opened, so a file planted at the staging path can't receive or redirect the
/// write. Pass [`staging_suffixes`]: fixed suffixes are for tests, and a predictable name can be
/// taken in advance to refuse every save.
pub fn create_staging(
    dir: &Path,
    name: &str,
    form: StagingName,
    suffixes: impl IntoIterator<Item = u64>,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    create_staging_as(dir, name, form, suffixes, None)
}

/// [`create_staging`], giving the new file the mode and group of the file `keep` describes on
/// Unix. It is created readable and writable by its owner only and then given them, so its mode
/// is never more open than that of the file it will replace, not even while it is still empty (a
/// handle opened then would keep reading what is written later). The owner is the saver's.
fn create_staging_as(
    dir: &Path,
    name: &str,
    form: StagingName,
    suffixes: impl IntoIterator<Item = u64>,
    keep: Option<&std::fs::Metadata>,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    // At most 128 bytes of the target's name, cut between characters, so the staging name fits
    // the 255-byte (Linux, macOS) and 255-unit (Windows) limits however long that name is.
    let mut stem = String::new();
    for c in name.chars() {
        if stem.len() + c.len_utf8() > 128 {
            break;
        }
        stem.push(c);
    }
    for suffix in suffixes.into_iter().take(STAGING_ATTEMPTS) {
        let tmp = dir.join(match form {
            StagingName::SuffixThenTag => format!(".{stem}.{suffix:016x}.pdfcraft-tmp"),
            StagingName::TagThenSuffix => format!(".{stem}.pdfcraft-{suffix:016x}.tmp"),
        });
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if keep.is_some() {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&tmp) {
            Ok(file) => {
                if let Some(keep) = keep {
                    // A volume that can't change modes (FAT, some network shares) leaves the file
                    // owner-only: stricter than the file it replaces, never looser.
                    let _ = file.set_permissions(kept_permissions(&file, keep));
                }
                return Ok((tmp, file));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            // Windows reports a folder at the name as "access denied"; it is taken all the same.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && tmp.symlink_metadata().is_ok() => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "every temporary file name tried is taken"))
}

/// The permissions a staging file gets to replace the file `keep` describes. The mode's group bits
/// are meant for that file's group, so the staging file joins it first. Where it can't (the saver
/// isn't a member), group and others get nothing: someone the group bits shut out would
/// otherwise read the file as one of the others.
#[cfg(unix)]
fn kept_permissions(file: &std::fs::File, keep: &std::fs::Metadata) -> std::fs::Permissions {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let mut mode = keep.permissions();
    let joined = file.metadata().is_ok_and(|m| m.gid() == keep.gid()) || std::os::unix::fs::fchown(file, None, Some(keep.gid())).is_ok();
    if !joined {
        mode.set_mode(mode.mode() & !0o077);
    }
    mode
}

#[cfg(not(unix))]
fn kept_permissions(_: &std::fs::File, keep: &std::fs::Metadata) -> std::fs::Permissions {
    keep.permissions()
}

/// Unpredictable staging-name suffixes, so a name can't be planted in advance. Each is the
/// standard library's keyed hash of a counter (`RandomState::hash_one`, currently SipHash-1-3),
/// keyed from the operating system's random source. That is all a staging name needs, but it is
/// not a CSPRNG: don't use these where a value must stay secret or unguessable after it has been
/// seen, such as a token or a key.
pub fn staging_suffixes() -> impl Iterator<Item = u64> {
    use std::hash::BuildHasher;
    let state = std::hash::RandomState::new();
    (0u64..).map(move |i| state.hash_one(i))
}

/// Write `bytes` to `path` through a staging file beside it (see [`create_staging`]; `form`
/// names it as the caller always has), flushed to the disk and then renamed over the target: a
/// crash, a full disk or a refused flush never leaves a half-written file where a good one was,
/// and the target is untouched until the new file is complete. The target's folder must exist;
/// a folder at the target is refused by the rename. Flushing is the staging file's own
/// (`sync_all`); parent-directory durability after a power loss is not promised. On Unix a file
/// that is replaced keeps its permissions and group, so a private file stays private; a new file gets the
/// usual mode for a new file. On Windows and macOS a replaced file keeps its creation date, where
/// the volume stores one.
/// On macOS, accessible extended attributes of an existing regular destination are copied
/// before flushing; a copy failure refuses the save rather than losing metadata (#2658).
pub fn write_atomically(path: &Path, bytes: &[u8], form: StagingName) -> std::io::Result<()> {
    write_atomically_with(path, bytes, form, staging_suffixes(), std::fs::File::sync_all)
}

/// [`write_atomically`] with the staging-name suffixes and the flush given, so tests can plant
/// names and refuse flushes. A failure at any step removes the staging file and leaves the
/// target as it was. A read-only target is refused before anything is written.
pub fn write_atomically_with(
    path: &Path,
    bytes: &[u8],
    form: StagingName,
    suffixes: impl IntoIterator<Item = u64>,
    sync_all: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write as _;
    // A read-only file is not replaced. Windows refuses the rename anyway, but on Unix a rename
    // ignores the target's permissions: the file would be overwritten without a word, and come
    // out writable (#2658).
    if std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().readonly()) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the file is read-only; save a copy with Save As"));
    }
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().and_then(std::ffi::OsStr::to_str).unwrap_or("save");
    // A file that is replaced keeps its permissions (#1790) and group. Unix only: Windows is as before.
    #[cfg(unix)]
    let keep = std::fs::metadata(path).ok();
    #[cfg(not(unix))]
    let keep = None;
    // A saved file is the same document: it keeps its creation date (#2658).
    #[cfg(any(windows, target_os = "macos"))]
    let created = std::fs::metadata(path).ok().filter(|m| m.is_file()).and_then(|m| m.created().ok());
    let (tmp, file) = create_staging_as(dir, name, form, suffixes, keep.as_ref())?;
    let result = (|| {
        // Closed at the end of the block, before the rename (required on Windows).
        {
            let mut file = file;
            file.write_all(bytes)?;
            #[cfg(target_os = "macos")]
            preserve_target_xattrs(path, &file).map_err(|e| {
                std::io::Error::new(e.kind(), format!("could not preserve the file's extended attributes; save a copy with Save As: {e}"))
            })?;
            #[cfg(any(windows, target_os = "macos"))]
            keep_created(&file, created);
            sync_staged(&file, sync_all)?;
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Copy accessible macOS attributes to the staging descriptor before publication. The kernel
/// hides compression bookkeeping unless SHOWCOMPRESSION is requested; leave that option off.
/// Provenance is managed by macOS for the new file, not writable document metadata.
#[cfg(target_os = "macos")]
fn preserve_target_xattrs(path: &Path, staged: &std::fs::File) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags, XattrFlags, fgetxattr, flistxattr, fsetxattr, open};
    use rustix::io::Errno;

    // A new destination has nothing to retain. Replacing a symlink has always replaced the
    // link, not its referent: do not open it to read metadata from a different file. NONBLOCK
    // also prevents a FIFO at the destination from hanging the metadata read.
    let source = match open(path, OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK, Mode::empty()) {
        Ok(fd) => std::fs::File::from(fd),
        Err(Errno::NOENT | Errno::LOOP) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !source.metadata()?.is_file() {
        return Ok(());
    }

    // Metadata is untrusted too: bound both allocations and total copying, including forks.
    const MAX_NAMES_BYTES: usize = 64 * 1024;
    const MAX_ATTRIBUTES: usize = 1024;
    const MAX_VALUE_BYTES: usize = 16 * 1024 * 1024;
    const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
    let mut names = vec![0u8; MAX_NAMES_BYTES];
    let len = match flistxattr(&source, names.as_mut_slice()) {
        Ok(len) => len,
        Err(Errno::NOTSUP) => return Ok(()), // No attributes exist on this filesystem.
        Err(Errno::RANGE) => return Err(std::io::Error::other("extended attribute name list exceeds 64 KiB")),
        Err(e) => return Err(e.into()),
    };
    let names = names.get(..len).ok_or_else(|| std::io::Error::other("extended attribute name list exceeds 64 KiB"))?;
    let mut total = 0usize;
    for (index, name) in names.split(|b| *b == 0).filter(|name| !name.is_empty()).enumerate() {
        if index >= MAX_ATTRIBUTES {
            return Err(std::io::Error::other("extended attribute count exceeds 1024"));
        }
        if name == b"com.apple.provenance" {
            continue;
        }
        let name = std::ffi::CString::new(name).map_err(|_| std::io::Error::other("invalid extended attribute name"))?;
        let size = fgetxattr(&source, &name, &mut [] as &mut [u8])?;
        if size > MAX_VALUE_BYTES || size > MAX_TOTAL_BYTES.saturating_sub(total) {
            return Err(std::io::Error::other("extended attributes exceed the 16 MiB per-value or 64 MiB total limit"));
        }
        // Resource forks may return a prefix instead of ERANGE on growth; check the size again.
        let value = read_xattr_value(size, |buffer| Ok(fgetxattr(&source, &name, buffer)?))?;
        fsetxattr(staged, &name, &value, XattrFlags::empty())?;
        total = total.saturating_add(value.len());
    }
    Ok(())
}

/// Read an already size-bounded attribute, refusing a partial result or a changed post-read size.
#[cfg(target_os = "macos")]
fn read_xattr_value(size: usize, mut read: impl FnMut(&mut [u8]) -> std::io::Result<usize>) -> std::io::Result<Vec<u8>> {
    let mut value = vec![0u8; size.max(1)];
    let len = read(value.as_mut_slice())?;
    let after = read(&mut [])?;
    if len != size || after != size {
        return Err(std::io::Error::other("extended attribute changed size while saving"));
    }
    value.truncate(len);
    Ok(value)
}

/// Give the staging file the creation date of the file it replaces, where the system keeps one
/// that a program can set (Windows, macOS). Best effort: a volume that can't store it leaves the
/// new file's own date, as before.
#[cfg(any(windows, target_os = "macos"))]
fn keep_created(file: &std::fs::File, created: Option<std::time::SystemTime>) {
    if let Some(created) = created {
        #[cfg(target_os = "macos")]
        use std::os::macos::fs::FileTimesExt as _;
        #[cfg(windows)]
        use std::os::windows::fs::FileTimesExt as _;
        let _ = file.set_times(std::fs::FileTimes::new().set_created(created));
    }
}

/// Flush the staging file to the disk before it replaces the target. On macOS `sync_all` asks
/// for a full flush (`F_FULLFSYNC`), which SMB and FUSE volumes, such as a mounted Cryptomator
/// vault, refuse as unsupported: there a plain `fsync` is the strongest flush the volume offers.
/// Any other failure still fails the write.
fn sync_staged(f: &std::fs::File, sync_all: impl FnOnce(&std::fs::File) -> std::io::Result<()>) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use rustix::io::Errno;
        match sync_all(f) {
            Err(e) if matches!(Errno::from_io_error(&e), Some(Errno::NOTSUP | Errno::NOTTY)) => Ok(rustix::fs::fsync(f)?),
            other => other,
        }
    }
    #[cfg(not(target_os = "macos"))]
    sync_all(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty folder for one test.
    fn test_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-platform-staging-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_name(p: &Path) -> String {
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// The callers' tests plant files at these exact names; if a name changed, those tests would
    /// plant where nothing writes and pass without testing anything.
    #[test]
    fn each_form_keeps_its_exact_name() {
        let dir = test_dir("forms");
        let (tools, _) = create_staging(&dir, "out.pdf", StagingName::SuffixThenTag, [0xab]).unwrap();
        assert_eq!(file_name(&tools), ".out.pdf.00000000000000ab.pdfcraft-tmp");
        let (desktop, _) = create_staging(&dir, "out.pdf", StagingName::TagThenSuffix, [0xab]).unwrap();
        assert_eq!(file_name(&desktop), ".out.pdf.pdfcraft-00000000000000ab.tmp");
        assert_eq!(std::fs::read(&tools).unwrap(), b"", "a new, empty file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_name_is_cut_to_128_bytes_between_characters() {
        let dir = test_dir("long");
        // One byte, then four-byte characters: the 128-byte limit falls inside the 32nd one.
        let name = format!("a{}.pdf", "\u{1F600}".repeat(40));
        let (tmp, _) = create_staging(&dir, &name, StagingName::SuffixThenTag, [1]).unwrap();
        assert_eq!(file_name(&tmp), format!(".a{}.0000000000000001.pdfcraft-tmp", "\u{1F600}".repeat(31)));
        // One-byte characters: exactly 128 of them are kept.
        let (tmp, _) = create_staging(&dir, &"x".repeat(200), StagingName::SuffixThenTag, [2]).unwrap();
        assert_eq!(file_name(&tmp), format!(".{}.0000000000000002.pdfcraft-tmp", "x".repeat(128)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #2658: a read-only file is refused before anything is staged, on Unix (where a rename would
    /// replace it) as on Windows (where the rename fails).
    #[test]
    fn a_read_only_target_is_refused_and_kept() {
        let dir = test_dir("readonly");
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&target, perms.clone()).unwrap();
        let staged = std::cell::Cell::new(false);
        let e = write_atomically_with(&target, b"NEW", StagingName::TagThenSuffix, [7], |_: &std::fs::File| {
            staged.set(true);
            Ok(())
        })
        .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
        assert!(!staged.get(), "nothing was staged");
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
        assert!(std::fs::metadata(&target).unwrap().permissions().readonly(), "still read-only");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no staging file is left");
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&target, perms).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_out_of_suffixes_refuses_and_creates_nothing() {
        let dir = test_dir("empty");
        let e = create_staging(&dir, "out.pdf", StagingName::TagThenSuffix, []).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_first_sixteen_names_are_tried() {
        let dir = test_dir("cap");
        let name = |s: u64| dir.join(format!(".out.pdf.{s:016x}.pdfcraft-tmp"));
        for s in 1..=STAGING_ATTEMPTS as u64 {
            std::fs::write(name(s), "PLANTED").unwrap();
        }
        assert!(create_staging(&dir, "out.pdf", StagingName::SuffixThenTag, 1..).is_err());
        assert!(!name(STAGING_ATTEMPTS as u64 + 1).exists(), "a 17th name was tried");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `write_atomically`: the target holds the new bytes only once they are complete; a refused
    /// flush leaves the old file and removes the staging file; a missing folder writes nothing.
    #[test]
    fn an_atomic_write_replaces_the_target_only_when_complete() {
        let dir = test_dir("write");
        let target = dir.join("out.pdf");
        std::fs::write(&target, b"OLD").unwrap();
        let names = |dir: &Path| {
            let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| file_name(&e.unwrap().path())).collect();
            v.sort();
            v
        };
        // The staging file is written and flushed before the target changes.
        let mut seen_old = false;
        write_atomically_with(&target, b"NEW", StagingName::SuffixThenTag, [7], |f| {
            seen_old = std::fs::read(&target).unwrap() == b"OLD" && f.metadata().unwrap().len() == 3;
            Ok(())
        })
        .unwrap();
        assert!(seen_old, "flushed before publication, with the target still the old file");
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert_eq!(names(&dir), ["out.pdf"], "no staging file is left");
        // A refused flush keeps the target and cleans up.
        let e = write_atomically_with(&target, b"NEWER", StagingName::TagThenSuffix, [8], |_| Err(std::io::Error::other("refused"))).unwrap_err();
        assert!(e.to_string().contains("refused"));
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert_eq!(names(&dir), ["out.pdf"]);
        // A folder that does not exist: nothing is written anywhere.
        assert!(write_atomically(&dir.join("missing").join("out.pdf"), b"X", StagingName::SuffixThenTag).is_err());
        assert_eq!(names(&dir), ["out.pdf"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn atomic_saves_keep_finder_tags_and_binary_extended_attributes() {
        use rustix::fs::{XattrFlags, fgetxattr, setxattr};
        use std::os::macos::fs::FileTimesExt as _;
        let dir = test_dir("xattrs");
        let target = dir.join("out.pdf");
        let attributes: &[(&str, &[u8])] = &[
            ("com.apple.metadata:_kMDItemUserTags", b"<plist version=\"1.0\"><array><string>PdfCraft\n6</string></array></plist>"),
            ("com.pdfcraft.test.binary", b"\0\xff\x80original\0"),
            ("com.pdfcraft.test.empty", b""),
        ];
        std::fs::write(&target, b"OLD").unwrap();
        for (name, value) in attributes {
            setxattr(&target, *name, value, XattrFlags::empty()).unwrap();
        }
        let created = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_934_245);
        let original = std::fs::OpenOptions::new().write(true).open(&target).unwrap();
        original.set_times(std::fs::FileTimes::new().set_created(created)).unwrap();
        assert_eq!(original.metadata().unwrap().created().unwrap(), created);
        drop(original);
        // Both desktop and automation name forms, including a second save of the replacement.
        for form in [StagingName::TagThenSuffix, StagingName::SuffixThenTag] {
            write_atomically_with(&target, b"NEW", form, [7], |staged| {
                assert_eq!(staged.metadata().unwrap().created().unwrap(), created, "creation date must survive attribute copying before flush");
                for (name, value) in attributes {
                    let mut bytes = [0u8; 256];
                    let len = fgetxattr(staged, *name, &mut bytes[..]).unwrap();
                    assert_eq!(&bytes[..len], *value, "metadata must be present before flush: {name}");
                }
                Ok(())
            })
            .unwrap();
            let saved = std::fs::File::open(&target).unwrap();
            assert_eq!(saved.metadata().unwrap().created().unwrap(), created);
            for (name, value) in attributes {
                let mut bytes = [0u8; 256];
                let len = fgetxattr(&saved, *name, &mut bytes[..]).unwrap();
                assert_eq!(&bytes[..len], *value);
            }
            assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_failed_flush_keeps_the_original_extended_attributes() {
        use rustix::fs::{XattrFlags, getxattr, setxattr};
        let dir = test_dir("xattrs-flush");
        let target = dir.join("out.pdf");
        std::fs::write(&target, b"OLD").unwrap();
        setxattr(&target, "com.pdfcraft.test", b"original", XattrFlags::empty()).unwrap();
        let e = write_atomically_with(&target, b"NEW", StagingName::TagThenSuffix, [9], |_| Err(std::io::Error::other("flush refused"))).unwrap_err();
        assert!(e.to_string().contains("flush refused"));
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
        let mut value = [0u8; 64];
        let len = getxattr(&target, "com.pdfcraft.test", &mut value[..]).unwrap();
        assert_eq!(&value[..len], b"original");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Model an HFS-style partial read while using real macOS metadata and the atomic writer.
    /// The shared read validator must reject growth before the staging file can be published.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_growing_resource_fork_refuses_publication_and_keeps_original() {
        use rustix::fs::{XattrFlags, fgetxattr, fsetxattr, getxattr, setxattr};
        let dir = test_dir("xattrs-growing-fork");
        let target = dir.join("out.pdf");
        let name = "com.apple.ResourceFork";
        for form in [StagingName::TagThenSuffix, StagingName::SuffixThenTag] {
            std::fs::write(&target, b"ORIGINAL PDF").unwrap();
            setxattr(&target, name, b"ABC", XattrFlags::empty()).unwrap();
            let source = std::fs::File::open(&target).unwrap();
            let result = write_atomically_with(&target, b"REPLACEMENT PDF", form, [23], |staged| {
                let size = fgetxattr(&source, name, &mut [] as &mut [u8])?;
                let value = read_xattr_value(size, |buffer| {
                    if buffer.is_empty() {
                        return Ok(fgetxattr(&source, name, buffer)?);
                    }
                    // HFS can return the prefix that fits instead of ERANGE. APFS need not
                    // have that behaviour: model the kernel response, not a timing race.
                    setxattr(&target, name, b"ABCD", XattrFlags::empty())?;
                    buffer.get_mut(..3).unwrap().copy_from_slice(b"ABC");
                    Ok(3)
                })?;
                fsetxattr(staged, name, &value, XattrFlags::empty())?;
                Ok(())
            });
            let error = result.expect_err("a short prefix of the grown fork must not be published");
            assert!(error.to_string().contains("changed size"), "{error}");
            assert_eq!(std::fs::read(&target).unwrap(), b"ORIGINAL PDF");
            let mut bytes = [0u8; 8];
            let len = getxattr(&target, name, &mut bytes[..]).unwrap();
            assert_eq!(&bytes[..len], b"ABCD", "the original's grown fork must remain intact");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "staging files are removed on refusal");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Stable empty/binary values are accepted; short reads, growth and failed rechecks are refused.
    #[cfg(target_os = "macos")]
    #[test]
    fn attribute_reads_require_a_complete_value_and_stable_size() {
        for bytes in [&b""[..], &b"\0\xffB"[..]] {
            let value = read_xattr_value(bytes.len(), |buffer| {
                if !buffer.is_empty() {
                    buffer.get_mut(..bytes.len()).unwrap().copy_from_slice(bytes);
                }
                Ok(bytes.len())
            })
            .unwrap();
            assert_eq!(value, bytes);
        }
        for (before, read_len, after) in [(3, 2, 3), (3, 3, 4), (0, 1, 1), (3, 2, 2)] {
            let result = read_xattr_value(before, |buffer| Ok(if buffer.is_empty() { after } else { read_len }));
            assert!(result.unwrap_err().to_string().contains("changed size"));
        }
        let result = read_xattr_value(3, |buffer| {
            if buffer.is_empty() { Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "size recheck refused")) } else { Ok(3) }
        });
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn oversized_extended_attributes_refuse_the_save_without_replacing_the_target() {
        use rustix::fs::{XattrFlags, getxattr, setxattr};
        let dir = test_dir("xattrs-limit");
        let target = dir.join("out.pdf");
        std::fs::write(&target, b"OLD").unwrap();
        // A resource fork can exceed the ordinary attribute limit on macOS. No fixture file
        // is committed; only the OS metadata of this temporary synthetic file holds the bytes.
        let fork = vec![42u8; 16 * 1024 * 1024 + 1];
        setxattr(&target, "com.apple.ResourceFork", &fork, XattrFlags::empty()).unwrap();
        let mut flushed = false;
        let e = write_atomically_with(&target, b"NEW", StagingName::TagThenSuffix, [10], |_| {
            flushed = true;
            Ok(())
        })
        .unwrap_err();
        assert!(e.to_string().contains("extended attributes"), "{e}");
        assert!(e.to_string().contains("Save As"), "{e}");
        assert!(!flushed, "metadata failure must stop before flush and rename");
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
        let size = getxattr(&target, "com.apple.ResourceFork", &mut [] as &mut [u8]).unwrap();
        assert_eq!(size, fork.len());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn replacing_a_symlink_does_not_copy_attributes_from_its_referent() {
        use rustix::fs::{XattrFlags, getxattr, setxattr};
        let dir = test_dir("xattrs-symlink");
        let outside = test_dir("xattrs-outside").join("private.pdf");
        std::fs::write(&outside, b"PRIVATE").unwrap();
        setxattr(&outside, "com.pdfcraft.test", b"private metadata", XattrFlags::empty()).unwrap();
        let target = dir.join("out.pdf");
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        write_atomically(&target, b"NEW", StagingName::TagThenSuffix).unwrap();
        assert_eq!(std::fs::read(&outside).unwrap(), b"PRIVATE");
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert!(!target.symlink_metadata().unwrap().file_type().is_symlink());
        let mut bytes = [0u8; 64];
        assert_eq!(getxattr(&target, "com.pdfcraft.test", &mut bytes[..]).unwrap_err(), rustix::io::Errno::NOATTR);
        let len = getxattr(&outside, "com.pdfcraft.test", &mut bytes[..]).unwrap();
        assert_eq!(&bytes[..len], b"private metadata");
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(outside.parent().unwrap()).unwrap();
    }

    /// #1790: a replaced file keeps its permissions, and its staging file already has them while
    /// it is written; a new file gets the same mode as any other new file.
    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = test_dir("modes");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let target = dir.join("out.pdf");
        for keep in [0o600, 0o640, 0o664] {
            std::fs::write(&target, b"OLD").unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(keep)).unwrap();
            let mut staged = 0;
            write_atomically_with(&target, b"NEW", StagingName::SuffixThenTag, [keep as u64], |f| {
                staged = f.metadata()?.permissions().mode() & 0o777;
                Ok(())
            })
            .unwrap();
            assert_eq!(staged, keep, "the staging file has the target's mode before it is flushed");
            assert_eq!(mode(&target), keep, "the saved file keeps mode {keep:o}");
            assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        }
        let fresh = dir.join("new.pdf");
        write_atomically(&fresh, b"NEW", StagingName::TagThenSuffix).unwrap();
        let plain = dir.join("plain");
        std::fs::write(&plain, b"").unwrap();
        assert_eq!(mode(&fresh), mode(&plain), "a new file is not made private");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A replaced file keeps its group as well as its mode, so group permissions never reach a
    /// group that couldn't read the file before (the saver's own group, by default).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_replaced_file_keeps_its_group() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let dir = test_dir("group");
        let target = dir.join("out.pdf");
        std::fs::write(&target, b"OLD").unwrap();
        let created = std::fs::metadata(&target).unwrap().gid();
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let groups = status.lines().find_map(|l| l.strip_prefix("Groups:")).unwrap_or_default();
        let Some(other) = groups.split_whitespace().filter_map(|g| g.parse::<u32>().ok()).find(|&g| g != created) else {
            eprintln!("skipped: the test needs a second group");
            return;
        };
        std::os::unix::fs::chown(&target, None, Some(other)).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        let mut staged = (0, 0);
        write_atomically_with(&target, b"NEW", StagingName::SuffixThenTag, [7], |f| {
            let m = f.metadata()?;
            staged = (m.gid(), m.permissions().mode() & 0o777);
            Ok(())
        })
        .unwrap();
        assert_eq!(staged, (other, 0o640), "the staging file has the target's group before it is flushed");
        let m = std::fs::metadata(&target).unwrap();
        assert_eq!((m.gid(), m.permissions().mode() & 0o777), (other, 0o640));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #2658: a save replaces the file, but the document is the same one: it keeps its creation
    /// date where the system lets a program set it.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_replaced_file_keeps_its_creation_date() {
        #[cfg(target_os = "macos")]
        use std::os::macos::fs::FileTimesExt as _;
        #[cfg(windows)]
        use std::os::windows::fs::FileTimesExt as _;
        let dir = test_dir("created");
        let target = dir.join("doc.pdf");
        std::fs::write(&target, b"OLD").unwrap();
        // 2020-01-02 03:04:05 UTC.
        let then = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_934_245);
        let file = std::fs::OpenOptions::new().write(true).open(&target).unwrap();
        file.set_times(std::fs::FileTimes::new().set_created(then)).unwrap();
        drop(file);
        assert_eq!(std::fs::metadata(&target).unwrap().created().unwrap(), then, "the fixture's date is set");
        write_atomically(&target, b"NEW", StagingName::SuffixThenTag).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert_eq!(std::fs::metadata(&target).unwrap().created().unwrap(), then, "the saved file keeps its creation date");
        // A new file gets a date of its own.
        let fresh = dir.join("new.pdf");
        write_atomically(&fresh, b"NEW", StagingName::SuffixThenTag).unwrap();
        assert!(std::fs::metadata(&fresh).unwrap().created().unwrap() > then);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
