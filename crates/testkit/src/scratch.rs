//! Scratch folders that remove themselves when the test that made them ends (#6028).
//!
//! The workspace's test helpers made `temp_dir()/pdfcraft-<what>-<pid>` folders and never removed
//! them. nextest runs every test in a process of its own, so each run left a folder per test, and
//! Windows never clears `%TEMP%`: one machine held 22,390 of them, 1.35 GB, after six days.
//!
//! [`ScratchDir`] and [`ScratchFile`] are what such helpers hand out instead. Both dereference to
//! their [`Path`], so code written for the `PathBuf` the helpers used to return (`dir.join(..)`,
//! `&dir`, `path.display()`, `std::fs::write(&path, ..)`) keeps compiling, and dropping them, at
//! the end of the test, pass or fail, removes the folder with everything in it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A new, empty folder under the system temporary directory, removed with its contents when
/// dropped.
///
/// Bind it to a variable that lives as long as the folder is needed. Used as a temporary, as in
/// `ScratchDir::new("x")?.join("a.pdf")`, it is dropped, and the folder removed, at the end of
/// that statement.
#[derive(Debug)]
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// A folder named `pdfcraft-<name>-<process id>-<n>`. `name` says which test made it, in a
    /// listing of the temporary directory; the process id and a count kept per process make it a
    /// folder of its own, so tests running at the same time, as threads or as processes, never
    /// share one.
    pub fn new(name: &str) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pdfcraft-{name}-{}-{n}", std::process::id()));
        // Process ids come round again, so a run that was killed before its drops ran can have
        // left a folder of this name: start empty, or fail rather than hand out its contents.
        match std::fs::remove_dir_all(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        std::fs::create_dir_all(&path)?;
        Ok(ScratchDir { path })
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

/// For APIs taking `impl Into<PathBuf>`, as `&Path` does: the folder still belongs to the
/// [`ScratchDir`], which must outlive the copy.
impl From<&ScratchDir> for PathBuf {
    fn from(dir: &ScratchDir) -> PathBuf {
        dir.path.clone()
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best effort: a file another process still holds open can stop Windows from removing
        // it, and a test must not fail over its own clean-up.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The path of one file, `file`, in a [`ScratchDir`] of its own: for helpers that hand out file
/// paths rather than folders. The file isn't created; the folder, with whatever the test wrote
/// there, is removed when this is dropped.
#[derive(Debug)]
pub struct ScratchFile {
    path: PathBuf,
    /// The file's folder, removed when this is dropped.
    dir: ScratchDir,
}

impl ScratchFile {
    /// `file` in a new [`ScratchDir::new`]`(name)`.
    pub fn new(name: &str, file: &str) -> std::io::Result<Self> {
        let dir = ScratchDir::new(name)?;
        Ok(ScratchFile { path: dir.join(file), dir })
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The folder the file is in.
    pub fn dir(&self) -> &ScratchDir {
        &self.dir
    }
}

impl std::ops::Deref for ScratchFile {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchFile {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

/// For APIs taking `impl Into<PathBuf>`: the file's folder still belongs to the [`ScratchFile`].
impl From<&ScratchFile> for PathBuf {
    fn from(file: &ScratchFile) -> PathBuf {
        file.path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scratch_dir_is_removed_with_its_contents_when_dropped() {
        let dir = ScratchDir::new("testkit-scratch").unwrap();
        let path = dir.to_path_buf();
        assert!(path.is_dir(), "{path:?} exists while the test holds it");
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0, "and starts empty");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested").join("a.pdf"), b"%PDF").unwrap();
        drop(dir);
        assert!(!path.exists(), "{path:?} is gone, contents and all");
    }

    #[test]
    fn scratch_dirs_with_the_same_name_are_separate_folders() {
        let (a, b) = (ScratchDir::new("testkit-same").unwrap(), ScratchDir::new("testkit-same").unwrap());
        assert_ne!(a.path(), b.path());
        std::fs::write(a.join("x"), b"a").unwrap();
        drop(b);
        assert_eq!(std::fs::read(a.join("x")).unwrap(), b"a", "dropping one leaves the other's files");
    }

    #[test]
    fn a_scratch_file_takes_its_folder_with_it() {
        let file = ScratchFile::new("testkit-file", "out.pdf").unwrap();
        assert_eq!(file.file_name().and_then(|n| n.to_str()), Some("out.pdf"));
        assert_eq!(file.parent(), Some(file.dir().path()));
        std::fs::write(&file, b"%PDF").unwrap();
        let dir = file.dir().to_path_buf();
        drop(file);
        assert!(!dir.exists(), "{dir:?} went with the file");
    }
}
