//! Renaming an open document's file on disk (#2102, #2867): the checks a new name has to pass,
//! then the move, after which the document keeps its tab, undo history and everything else; only
//! its file name and path change.
//!
//! The rules are the same on every system, so a file renamed on one keeps working when the folder
//! is synced to another: no path separators or `< > : " | ? *`, no control characters, no Windows
//! device names (`CON`, `NUL`, `COM1`…), no name ending in a dot or a space, at most 255
//! bytes of UTF-8 and 255 UTF-16 code units. The file keeps its extension: a name typed without it gets it back.

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
    /// The file is marked read-only.
    ReadOnly,
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
            Self::ReadOnly => write!(f, "the file is read-only"),
            Self::Failed(why) => write!(f, "the file could not be renamed: {why}"),
        }
    }
}

impl std::error::Error for RenameError {}

/// Characters no file name may hold on Windows, refused everywhere.
const RESERVED_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Windows device names, refused as a name's stem on every system.
const DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
    "LPT6", "LPT7", "LPT8", "LPT9",
];
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

impl Session {
    /// Rename the file of document `id` to `typed` (see [`rename_target`]), in its own folder. The
    /// document must have a file and no unsaved changes. Returns the new path; the document keeps
    /// its undo history, only its name and path change.
    pub fn rename_file(&mut self, id: DocId, typed: &str) -> Result<String, RenameError> {
        let doc = self.get(id).ok_or_else(|| RenameError::Failed("the document is no longer open".into()))?;
        let current = PathBuf::from(doc.path.clone().ok_or(RenameError::NoFile)?);
        if doc.dirty {
            return Err(RenameError::Unsaved);
        }
        let Some(target) = rename_target(&current, typed)? else {
            return Ok(current.to_string_lossy().into_owned());
        };
        if std::fs::metadata(&current).is_ok_and(|m| m.permissions().readonly()) {
            return Err(RenameError::ReadOnly);
        }
        // `rename_target` checked that the name is free, but this move still replaces a file that
        // appeared in between (a sync client, another program): the window is small, and std has no
        // no-replace rename on every system.
        std::fs::rename(&current, &target).map_err(|e| {
            RenameError::Failed(match e.kind() {
                std::io::ErrorKind::PermissionDenied => "it is open in another program, or you don't have permission".into(),
                std::io::ErrorKind::NotFound => "it is no longer where it was opened from".into(),
                _ => e.to_string(),
            })
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

        let mut perms = std::fs::metadata(&cased).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&cased, perms.clone()).unwrap();
        assert_eq!(session.rename_file(id, "third"), Err(RenameError::ReadOnly));
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&cased, perms).unwrap();

        session.doc_mut(id).unwrap().dirty = true;
        assert_eq!(session.rename_file(id, "third"), Err(RenameError::Unsaved));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
