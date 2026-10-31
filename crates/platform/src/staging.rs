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
/// handle opened then would keep reading what is written later). The owner is the saver's. On
/// Windows it is created with that file's [`KEPT_ATTRIBUTES`].
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
        // Given as the file is created, so a hidden target's staging file is never visible.
        #[cfg(windows)]
        if let Some(keep) = keep {
            use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
            options.attributes(keep.file_attributes() & KEPT_ATTRIBUTES);
        }
        match options.open(&tmp) {
            Ok(file) => {
                #[cfg(unix)]
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
    #[cfg(not(any(unix, windows)))]
    let _ = keep;
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "every temporary file name tried is taken"))
}

/// The Windows attributes a replaced file passes on to its staging file (#4593):
/// `FILE_ATTRIBUTE_HIDDEN` and `FILE_ATTRIBUTE_SYSTEM`, the two that say who should see the
/// file. A read-only file is refused before anything is staged, and the archive bit is the
/// system's to set on a changed file; the other attributes describe how the volume stores it.
#[cfg(windows)]
const KEPT_ATTRIBUTES: u32 = 0x2 | 0x4;

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
/// usual mode for a new file. On Windows a replaced file stays hidden or system if it was, and
/// plain if it was plain. On Windows and macOS a replaced file keeps its creation date, where
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
    // A file that is replaced keeps its permissions (#1790) and group on Unix, and its hidden and
    // system attributes on Windows (#4593). A save replaces a symbolic link, not the file it
    // points to, so on Windows the attributes are the link's own.
    #[cfg(unix)]
    let keep = std::fs::metadata(path).ok();
    #[cfg(windows)]
    let keep = std::fs::symlink_metadata(path).ok();
    #[cfg(not(any(unix, windows)))]
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
mod tests;
