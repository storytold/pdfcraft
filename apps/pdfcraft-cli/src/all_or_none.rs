//! Write several files all or none: each is staged beside its target through `craft::atomic`
//! with the final rename held back, and only once every one is staged are they renamed into
//! place, so a part that cannot be written leaves every existing file as it was (#1793).

use std::cell::RefCell;
use std::fs::{File, Metadata, Permissions};
use std::io;
use std::path::{Path, PathBuf};

use craft::atomic::{AtomicFs, NewFilePermissions, RealFs, RenameRetry, retry_rename};

/// [`RealFs`], except that a rename is recorded instead of done: `(staging file, target)`.
/// The writer's one step after its rename, restoring hidden and system bits on Windows, is
/// repeated in `commit` after the real one.
#[derive(Default)]
struct HeldRenames(RefCell<Vec<(PathBuf, PathBuf)>>);

impl AtomicFs for HeldRenames {
    fn create_new(&self, path: &Path, permissions: NewFilePermissions) -> io::Result<File> {
        RealFs.create_new(path, permissions)
    }
    fn create_new_keeping(&self, path: &Path, permissions: NewFilePermissions, keep: Option<&Metadata>) -> io::Result<File> {
        RealFs.create_new_keeping(path, permissions, keep)
    }
    fn sync(&self, file: &File) -> io::Result<()> {
        RealFs.sync(file)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.0.borrow_mut().push((from.to_path_buf(), to.to_path_buf()));
        Ok(())
    }
    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()> {
        RealFs.hard_link(from, to)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        RealFs.remove(path)
    }
    fn sync_dir(&self, _: &Path) -> io::Result<()> {
        // Flushed after the real renames, in `commit`.
        Ok(())
    }
    fn set_permissions(&self, file: &File, perms: Permissions) -> io::Result<()> {
        RealFs.set_permissions(file, perms)
    }
    #[cfg(unix)]
    fn set_group(&self, file: &File, gid: u32) -> io::Result<()> {
        RealFs.set_group(file, gid)
    }
}

/// Write each `(path, bytes)`, replacing what is there, or write none of them. Every file is
/// written and flushed beside its target before any target is replaced; a failure while staging
/// removes the staged files and leaves every target as it was. Only a rename that fails after
/// earlier ones succeeded (a lock outlasting the retries) can leave some files replaced, and
/// the error says so.
pub(crate) fn write_all_or_none(files: &[(PathBuf, &[u8])]) -> Result<(), String> {
    let held = HeldRenames::default();
    let staged = files.iter().try_for_each(|(path, bytes)| stage(&held, path, bytes));
    let pending = held.0.into_inner();
    if let Err(e) = staged {
        discard(&pending);
        return Err(e);
    }
    commit(files, &pending)
}

/// Stage `bytes` for `path`. A folder (or other non-file) where the file goes, and on Windows a
/// read-only file, would only fail at the rename, after earlier files had replaced theirs, so
/// they are refused here.
fn stage(held: &HeldRenames, path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Ok(m) = std::fs::metadata(path) {
        if !m.is_file() {
            return Err(format!("{}: something other than a file is there, such as a folder", path.display()));
        }
        if cfg!(windows) && m.permissions().readonly() {
            return Err(format!("{}: the file is read-only", path.display()));
        }
    }
    craft::atomic::atomic_write_custom(held, RenameRetry::platform(), path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Rename every staged file over its target, in order. `files` names them as the caller gave
/// them, for the error.
fn commit(files: &[(PathBuf, &[u8])], pending: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    for (i, (from, to)) in pending.iter().enumerate() {
        #[cfg(windows)]
        let replaced = std::fs::metadata(to).ok().filter(Metadata::is_file);
        if let Err(e) = retry_rename(RenameRetry::platform(), || std::fs::rename(from, to)) {
            discard(pending.get(i..).unwrap_or_default());
            let named = files.get(i).map_or(to.as_path(), |(path, _)| path.as_path());
            let earlier = if i > 0 { "; the files before it were already replaced" } else { "" };
            return Err(format!("{}: {e}{earlier}", named.display()));
        }
        #[cfg(windows)]
        if let Some(replaced) = replaced {
            restore_kept_attributes(to, &replaced);
        }
    }
    // The renames are in place; a failed directory flush can't be undone, so it is best effort.
    let mut dirs: Vec<&Path> = pending.iter().filter_map(|(_, to)| to.parent()).collect();
    dirs.dedup();
    for dir in dirs {
        let _ = RealFs.sync_dir(if dir.as_os_str().is_empty() { Path::new(".") } else { dir });
    }
    Ok(())
}

/// What `craft::atomic` does after its own rename, which `HeldRenames` defers: a server that hides
/// dot-named files marks the stage hidden and keeps the mark through the rename, so the file gets
/// the replaced one's hidden and system bits back when they differ. Best effort: the bytes are in.
#[cfg(windows)]
fn restore_kept_attributes(target: &Path, replaced: &Metadata) {
    use std::os::windows::fs::MetadataExt;
    const KEPT: u32 = 0x2 | 0x4;
    let saved = std::fs::symlink_metadata(target).map(|m| m.file_attributes() & KEPT);
    if saved.is_ok_and(|bits| bits != replaced.file_attributes() & KEPT) {
        let _ = std::fs::set_permissions(target, replaced.permissions());
    }
}

fn discard(pending: &[(PathBuf, PathBuf)]) {
    for (staged, _) in pending {
        let _ = std::fs::remove_file(staged);
    }
}
