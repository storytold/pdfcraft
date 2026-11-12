//! Renaming an open document's file on disk (#2102, #2867): the checks a new name has to pass,
//! then the move, after which the document keeps its tab, undo history and everything else; only
//! its file name and path change.
//!
//! The rules are the same on every system, so a file renamed on one keeps working when the folder
//! is synced to another: no path separators or `< > : " | ? *`, no control characters, no Windows
//! device names (`CON`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9` and the superscript-digit forms),
//! no name ending in a dot or a space, at most 255 bytes of UTF-8 and 255 UTF-16 code units, and
//! a full path within this system's limit. The file keeps its extension: a name typed without it gets it back.

use std::path::{Path, PathBuf};

use crate::{DocId, Session};

/// Why a file could not be renamed. The messages say what to do instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameError {
    /// The document was never saved, so there is no file to rename.
    NoFile,
    /// The document has unsaved changes: save them first (the file on disk is not this version).
    Unsaved,
    /// The new name is empty.
    Empty,
    /// The new name can't be used as a file name; the reason says why.
    Invalid(String),
    /// Another file already has that name.
    Exists(String),
    /// The operating system refused (in use by another program, no permission, …).
    Failed(String),
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFile => write!(f, "the document has no file yet: save it first"),
            Self::Unsaved => write!(f, "the document has unsaved changes: save them before renaming"),
            Self::Empty => write!(f, "the name is empty"),
            Self::Invalid(why) => write!(f, "{why}"),
            Self::Exists(name) => write!(f, "a file named {name} already exists in that folder"),
            Self::Failed(why) => write!(f, "the file could not be renamed: {why}"),
        }
    }
}

impl std::error::Error for RenameError {}

/// Characters no file name may hold on Windows, refused everywhere.
const RESERVED_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Windows device names, refused as a name's stem on every system.
const DEVICE_NAMES: [&str; 33] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM0",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT0",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
    "CONIN$",
    "CONOUT$",
    "CLOCK$",
];
/// The longest full path this system takes, where it can be known: Windows' `MAX_PATH` (260 with
/// the closing NUL, in UTF-16 units; a `\\?\` path is exempt), Linux's `PATH_MAX` (4096 bytes) and
/// macOS's (1024 bytes). Elsewhere there is no limit to check.
#[cfg(windows)]
const MAX_PATH_LEN: Option<usize> = Some(259);
#[cfg(target_os = "macos")]
const MAX_PATH_LEN: Option<usize> = Some(1023);
#[cfg(target_os = "linux")]
const MAX_PATH_LEN: Option<usize> = Some(4095);
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
const MAX_PATH_LEN: Option<usize> = None;

/// The length of `path` in the unit this system limits it in.
fn path_len(path: &Path) -> usize {
    #[cfg(windows)]
    {
        path.as_os_str().encode_wide().count()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().len()
    }
}
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
/// The longest file name the common file systems take: 255 bytes of UTF-8 (ext4, APFS) and 255
/// UTF-16 code units (NTFS), so a name is checked against both, never by its characters.
const MAX_NAME_LEN: usize = 255;

/// The file name `typed` gives the document now at `current`: trimmed, checked, with
/// `current`'s extension added back when it was left out.
pub fn new_file_name(current: &Path, typed: &str) -> Result<String, RenameError> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err(RenameError::Empty);
    }
    if let Some(c) = typed.chars().find(|c| RESERVED_CHARS.contains(c)) {
        return Err(RenameError::Invalid(format!("a file name can't contain {c}")));
    }
    if typed.chars().any(char::is_control) {
        return Err(RenameError::Invalid("a file name can't contain control characters".into()));
    }
    if typed.ends_with('.') {
        return Err(RenameError::Invalid("a file name can't end with a dot".into()));
    }
    let ext = current.extension().map(|e| e.to_string_lossy().into_owned());
    let name = match &ext {
        Some(ext) if !typed.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) => format!("{typed}.{ext}"),
        _ => typed.to_owned(),
    };
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    if DEVICE_NAMES.iter().any(|d| d.eq_ignore_ascii_case(stem)) {
        return Err(RenameError::Invalid(format!("{stem} is a name Windows reserves for a device")));
    }
    if name.len() > MAX_NAME_LEN || name.encode_utf16().count() > MAX_NAME_LEN {
        return Err(RenameError::Invalid(format!("a file name can be at most {MAX_NAME_LEN} bytes")));
    }
    if let Some(limit) = MAX_PATH_LEN {
        let full = current.with_file_name(&name);
        // Windows' `\\?\` prefix lifts the limit.
        let extended = cfg!(windows) && full.to_string_lossy().starts_with(r"\\?\");
        if !extended && path_len(&full) > limit {
            return Err(RenameError::Invalid(format!("the full path would be over this system's limit of {limit} characters: use a shorter name")));
        }
    }
    Ok(name)
}

/// The path the document now at `current` moves to when renamed to `typed`: the same folder,
/// [`new_file_name`]. `Ok(None)` when nothing would change.
pub fn rename_target(current: &Path, typed: &str) -> Result<Option<PathBuf>, RenameError> {
    let name = new_file_name(current, typed)?;
    let target = current.with_file_name(&name);
    if target == current {
        return Ok(None);
    }
    // A name that only changes letter case is the same file on Windows and macOS: let it through.
    let same_file = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if target.exists() && !same_file(current, &target) {
        return Err(RenameError::Exists(name));
    }
    Ok(Some(target))
}

/// Whether `a` and `b` are one file: a rename that only changes letter case on a case-insensitive
/// file system.
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// Move `old` to `new`, never replacing a file that already has that name (`AlreadyExists`).
///
/// `std::fs::rename` replaces its target on every system and std has no no-replace rename, so the
/// move is a hard link (which fails atomically when `new` exists) and then the removal of `old`;
/// if `old` can't be removed the new name is taken away again, so a failure never leaves both. Where
/// hard links are not available (FAT/exFAT, some network shares, or any other error that means the
/// link can't be made) it falls back to checking and then renaming: a file created in between is
/// replaced, a window that remains only there. The same file under another letter case is plainly
/// renamed (nothing to replace).
fn move_no_replace(old: &Path, new: &Path) -> std::io::Result<()> {
    if same_file(old, new) {
        return std::fs::rename(old, new);
    }
    match std::fs::hard_link(old, new) {
        Ok(()) => std::fs::remove_file(old).inspect_err(|_| {
            let _ = std::fs::remove_file(new);
        }),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::NotFound) => Err(e),
        Err(_) => {
            if new.symlink_metadata().is_ok() {
                return Err(std::io::ErrorKind::AlreadyExists.into());
            }
            std::fs::rename(old, new)
        }
    }
}

/// The move made below a folder, without following a link (Linux only; see
/// [`Session::rename_file_in`]).
#[cfg(target_os = "linux")]
mod rooted {
    use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags, linkat, openat, renameat, renameat_with, statat, unlinkat};
    use rustix::io::Errno;
    use std::io::{Error, ErrorKind, Result};
    use std::os::fd::OwnedFd;
    use std::path::{Component, Path};

    fn refused(what: &str) -> Error {
        Error::other(format!("the move can't be kept inside the allowed folder: {what}"))
    }

    fn open_dir(parent: &OwnedFd, name: &std::ffi::OsStr) -> Result<OwnedFd> {
        openat(parent, name, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()).map_err(|e| match e {
            Errno::LOOP | Errno::NOTDIR => refused("a folder on the way is a link"),
            e => e.into(),
        })
    }

    /// `path` relative to `root`: as written, or else with its folder's links resolved (the file
    /// itself is never resolved). Names below `root` only; `..` and the like are refused.
    fn below(root: &Path, path: &Path) -> Result<std::path::PathBuf> {
        let rel = match path.strip_prefix(root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => {
                let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Err(refused("no folder")) };
                let dir = dir.canonicalize()?;
                dir.strip_prefix(root.canonicalize()?).map_err(|_| refused("the file is outside it"))?.join(name)
            }
        };
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(refused("an odd name on the way"));
        }
        Ok(rel)
    }

    /// Same as [`super::move_no_replace`], with `old` and `new` (one folder, below `root`) reached
    /// from an open `root` by names opened without following links.
    pub(super) fn move_no_replace(root: &Path, old: &Path, new: &Path) -> Result<()> {
        let (old, new) = (below(root, old)?, below(root, new)?);
        let (Some(folder), Some(from), Some(to)) = (old.parent(), old.file_name(), new.file_name()) else { return Err(refused("no file name")) };
        if new.parent() != Some(folder) {
            return Err(refused("the new name is in another folder"));
        }
        let mut dir = openat(rustix::fs::CWD, root, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?;
        for part in folder.components() {
            dir = open_dir(&dir, part.as_os_str())?;
        }
        let kept = |e: Errno| -> Error { e.into() };
        match renameat_with(&dir, from, &dir, to, RenameFlags::NOREPLACE) {
            Ok(()) => Ok(()),
            Err(Errno::EXIST) => {
                // The same file under another letter case (case-insensitive folders) is renamed.
                let at = |n: &std::ffi::OsStr| statat(&dir, n, AtFlags::SYMLINK_NOFOLLOW);
                match (at(from), at(to)) {
                    (Ok(a), Ok(b))
                        if (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino)
                            && from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase() =>
                    {
                        renameat(&dir, from, &dir, to).map_err(kept)
                    }
                    _ => Err(ErrorKind::AlreadyExists.into()),
                }
            }
            // No renameat2 / NOREPLACE here: a hard link refuses a taken name atomically, then the
            // old name goes (and the new one is taken away again if it can't).
            Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => match linkat(&dir, from, &dir, to, AtFlags::empty()) {
                Ok(()) => unlinkat(&dir, from, AtFlags::empty()).map_err(|e| {
                    let _ = unlinkat(&dir, to, AtFlags::empty());
                    kept(e)
                }),
                Err(e @ (Errno::EXIST | Errno::NOENT)) => Err(e.into()),
                Err(_) => {
                    if statat(&dir, to, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
                        return Err(ErrorKind::AlreadyExists.into());
                    }
                    renameat(&dir, from, &dir, to).map_err(kept)
                }
            },
            Err(e) => Err(e.into()),
        }
    }
}

impl Session {
    /// Rename the file of document `id` to `typed` (see [`rename_target`]), in its own folder. The
    /// document must have a file and no unsaved changes. Returns the new path; the document keeps
    /// its undo history, only its name and path change.
    pub fn rename_file(&mut self, id: DocId, typed: &str) -> Result<String, RenameError> {
        self.rename_file_in(id, typed, None)
    }

    /// [`rename_file`](Self::rename_file) for an automation caller confined to the write folder
    /// `root`: on Linux the move itself is made below that folder, with no link followed on the way
    /// (the folder is opened, each name down to the file's folder is opened with `O_NOFOLLOW`, and
    /// the file is renamed with `renameat` there), so a folder swapped for a link between the
    /// caller's check and the move can't carry the file out of `root`. Where that can't be done the
    /// rename is refused rather than made unconfined.
    ///
    /// macOS and Windows keep the check-then-rename model the repo documents for confined paths
    /// (the caller's check, then a plain rename); `root` is ignored there, as it is when `None`.
    pub fn rename_file_in(&mut self, id: DocId, typed: &str, root: Option<&Path>) -> Result<String, RenameError> {
        #[cfg(not(target_os = "linux"))]
        let _ = root;
        let doc = self.get(id).ok_or_else(|| RenameError::Failed("the document is no longer open".into()))?;
        let current = PathBuf::from(doc.path.clone().ok_or(RenameError::NoFile)?);
        if doc.dirty {
            return Err(RenameError::Unsaved);
        }
        let Some(target) = rename_target(&current, typed)? else {
            return Ok(current.to_string_lossy().into_owned());
        };
        // Whether the file itself is marked read-only is not checked: renaming needs write access
        // to the folder, not to the file, and the system says so when it refuses.
        // `rename_target` checked that the name is free, but a file may appear under it before the
        // move (a sync client, another program): `move_no_replace` refuses to replace it.
        #[cfg(target_os = "linux")]
        let moved = match root {
            Some(root) => rooted::move_no_replace(root, &current, &target),
            None => move_no_replace(&current, &target),
        };
        #[cfg(not(target_os = "linux"))]
        let moved = move_no_replace(&current, &target);
        moved.map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                RenameError::Exists(target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            }
            std::io::ErrorKind::PermissionDenied => RenameError::Failed("it is open in another program, or you don't have permission".into()),
            std::io::ErrorKind::NotFound => RenameError::Failed("it is no longer where it was opened from".into()),
            _ => RenameError::Failed(e.to_string()),
        })?;
        let path = target.to_string_lossy().into_owned();
        if let Ok(doc) = self.doc_mut(id) {
            doc.name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
            doc.path = Some(path.clone());
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(typed: &str) -> Result<String, RenameError> {
        new_file_name(Path::new("/docs/report.pdf"), typed)
    }

    #[test]
    fn the_extension_comes_back_when_left_out() {
        assert_eq!(name("Budget 2026"), Ok("Budget 2026.pdf".into()));
        assert_eq!(name("Budget.PDF"), Ok("Budget.PDF".into()));
        assert_eq!(name("  spaced  "), Ok("spaced.pdf".into()), "surrounding spaces are trimmed");
        assert_eq!(name("notes.txt"), Ok("notes.txt.pdf".into()), "another extension is part of the name");
        assert_eq!(name("年度 report"), Ok("年度 report.pdf".into()));
    }

    #[test]
    fn names_windows_or_other_systems_refuse_are_refused() {
        assert_eq!(name(""), Err(RenameError::Empty));
        assert_eq!(name("   "), Err(RenameError::Empty));
        for bad in ["a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "tab\there"] {
            assert!(matches!(name(bad), Err(RenameError::Invalid(_))), "{bad:?}");
        }
        for device in ["CON", "con", "Nul", "COM1", "lpt9", "aux.notes", "PRN "] {
            assert!(matches!(name(device), Err(RenameError::Invalid(_))), "{device:?}");
        }
        assert!(name("CONSOLE").is_ok(), "only the exact device names are reserved");
        assert!(matches!(name("trailing."), Err(RenameError::Invalid(_))));
        // A trailing space goes with the trim, which leaves the dot at the end to be refused.
        assert!(matches!(name("a. "), Err(RenameError::Invalid(_))));
        assert!(matches!(name("report.pdf."), Err(RenameError::Invalid(_))));
        assert!(matches!(name(&"x".repeat(252)), Err(RenameError::Invalid(_))), "252 + .pdf is over 255");
        assert!(name(&"x".repeat(251)).is_ok());
    }

    #[test]
    fn renaming_moves_the_file_and_refuses_what_it_must() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("first.pdf");
        let mut session = Session::new();
        let bytes = session.create_blank(200.0, 300.0, 1).unwrap();
        std::fs::write(&file, bytes.as_ref()).unwrap();
        let id = session.open(file.to_string_lossy().into_owned(), Some(file.to_string_lossy().into_owned()), bytes.clone(), None).unwrap();
        std::fs::write(dir.join("taken.pdf"), b"other").unwrap();

        assert_eq!(session.rename_file(id, "taken"), Err(RenameError::Exists("taken.pdf".into())));
        assert_eq!(session.rename_file(id, "first"), Ok(file.to_string_lossy().into_owned()), "the same name changes nothing");

        let renamed = session.rename_file(id, "second").unwrap();
        assert!(!file.exists() && Path::new(&renamed).exists());
        let doc = session.get(id).unwrap();
        assert_eq!(doc.name, "second.pdf");
        assert_eq!(doc.path.as_deref(), Some(renamed.as_str()));

        // A case-only rename is the same file where the file system ignores case.
        let cased = session.rename_file(id, "Second").unwrap();
        assert!(Path::new(&cased).exists());
        assert_eq!(session.get(id).unwrap().name, "Second.pdf");

        // A read-only file in a writable folder renames, as it does in the file manager.
        let mut perms = std::fs::metadata(&cased).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&cased, perms.clone()).unwrap();
        let third = session.rename_file(id, "third");
        let moved = third.clone().unwrap_or_else(|_| cased.clone());
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&moved, perms).unwrap();
        assert!(third.is_ok(), "{third:?}");
        session.rename_file(id, "second").unwrap();

        session.doc_mut(id).unwrap().dirty = true;
        assert_eq!(session.rename_file(id, "third"), Err(RenameError::Unsaved));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_appears_under_the_new_name_is_never_replaced() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-rename-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (old, new) = (dir.join("old.pdf"), dir.join("new.pdf"));
        std::fs::write(&old, b"mine").unwrap();
        // Created after rename_target's check, before the move.
        std::fs::write(&new, b"theirs").unwrap();
        let err = move_no_replace(&old, &new).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&new).unwrap(), b"theirs", "the other file is untouched");
        assert_eq!(std::fs::read(&old).unwrap(), b"mine", "and nothing was moved");
        std::fs::remove_file(&new).unwrap();
        move_no_replace(&old, &new).unwrap();
        assert!(!old.exists(), "the old name is gone");
        assert_eq!(std::fs::read(&new).unwrap(), b"mine");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "one name is left");
        // The same file under another letter case is renamed, not refused.
        move_no_replace(&new, &dir.join("NEW.pdf")).unwrap();
        assert_eq!(std::fs::read(dir.join("NEW.pdf")).unwrap(), b"mine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn more_device_names_and_a_path_too_long_are_refused() {
        for device in ["COM0", "lpt0", "COM\u{b9}", "com\u{b2}.txt", "LPT\u{b3}"] {
            assert!(matches!(name(device), Err(RenameError::Invalid(_))), "{device:?}");
        }
        assert!(name("COM10").is_ok());
        if let Some(limit) = MAX_PATH_LEN {
            let folder = Path::new(if cfg!(windows) { "C:\\docs" } else { "/docs" });
            let mut dir = folder.to_path_buf();
            while path_len(&dir) < limit - 60 {
                dir.push("d".repeat(50));
            }
            let current = dir.join("report.pdf");
            assert!(new_file_name(&current, "short").is_ok());
            let long = "x".repeat(limit - path_len(&dir) - ".pdf".len());
            assert!(matches!(new_file_name(&current, &long), Err(RenameError::Invalid(_))), "one past the limit");
            assert!(new_file_name(&current, &long[1..]).is_ok(), "exactly at the limit");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_folder_swapped_for_a_link_out_of_the_root_is_refused_on_linux() {
        let base = std::env::temp_dir().join(format!("pdfcraft-rename-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let (inside_file, outside_file) = (root.join("sub/a.pdf"), outside.join("a.pdf"));
        std::fs::write(&inside_file, b"in").unwrap();
        std::fs::write(&outside_file, b"out").unwrap();
        let root = root.canonicalize().unwrap();
        // Plain move below the root works.
        rooted::move_no_replace(&root, &root.join("sub/a.pdf"), &root.join("sub/b.pdf")).unwrap();
        assert!(root.join("sub/b.pdf").exists() && !inside_file.exists());
        // A taken name is refused, nothing replaced.
        std::fs::write(root.join("sub/c.pdf"), b"c").unwrap();
        let err = rooted::move_no_replace(&root, &root.join("sub/b.pdf"), &root.join("sub/c.pdf")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        // `sub` is swapped for a link to a folder outside the root after the caller's check.
        std::fs::remove_dir_all(root.join("sub")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("sub")).unwrap();
        let err = rooted::move_no_replace(&root, &root.join("sub/a.pdf"), &root.join("sub/z.pdf")).unwrap_err();
        assert!(err.to_string().contains("allowed folder"), "{err}");
        assert!(outside_file.exists() && !outside.join("z.pdf").exists(), "nothing moved outside");
        // A file outside the root is refused too.
        assert!(rooted::move_no_replace(&root, &outside_file, &outside.join("y.pdf")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
