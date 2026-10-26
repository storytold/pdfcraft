//! Folder jobs: convert, recognize text in and reduce the size of many files and folders, one
//! file at a time, into an output folder. Shared by the desktop app (Scan & OCR ▸ Recognize text
//! in files and folders) and the `doc_batch` automation tool, so both find the same files, refuse
//! the same output folders and never replace an existing file.
//!
//! Discovery finishes before any output is made. Sources are never written: every result goes
//! under the output folder, at the input's path relative to the folder it was found in, and a
//! name that is taken gets a number (`scan (2).pdf`) instead of replacing it.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::{Session, ocr, optimize};

/// The most files and folders one job looks at, and the deepest it goes.
pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_DEPTH: usize = 64;
/// The largest input file read.
pub const MAX_INPUT: u64 = 512 * 1024 * 1024;

/// One thing done to each file, in the order given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Make a PDF from an image, text or other convertible file (first when present).
    Convert,
    /// Recognize text, so scanned pages become searchable.
    Ocr,
    /// Reduce File Size's image settings; a larger result keeps the original bytes.
    Optimize,
}

impl Step {
    /// The name the automation tools and the CLI use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Convert => "convert",
            Self::Ocr => "ocr",
            Self::Optimize => "optimize",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [Self::Convert, Self::Ocr, Self::Optimize].into_iter().find(|s| s.name() == name)
    }
}

/// What a step did to one file.
#[derive(Clone, Debug, PartialEq)]
pub enum StepLog {
    Ocr { words: usize, skipped_pages: usize },
    Size { step: Step, before: usize, after: usize },
}

/// The steps and their settings.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub steps: Vec<Step>,
    pub images: optimize::ImageSettings,
    pub mono: optimize::MonoSettings,
    pub ocr: ocr::OcrSettings,
}

impl Recipe {
    /// Recognize text only, with `settings`.
    pub fn ocr(settings: ocr::OcrSettings) -> Self {
        Self { steps: vec![Step::Ocr], images: optimize::ImageSettings::REDUCE, mono: optimize::MonoSettings::REDUCE, ocr: settings }
    }

    /// Whether the steps can read `path`: a PDF, or with Convert any convertible file.
    pub fn accepts(&self, path: &Path) -> bool {
        let extension = path.extension().and_then(std::ffi::OsStr::to_str).unwrap_or("").to_ascii_lowercase();
        extension == "pdf" || (self.steps.contains(&Step::Convert) && crate::CONVERTIBLE.contains(&extension.as_str()))
    }

    /// Run the steps on the file at `path`. `progress(page, pages)` is called while text is
    /// recognized; returning `false` stops the file with an error.
    pub fn process(&self, path: &Path, mut progress: impl FnMut(usize, usize) -> bool) -> Result<(Arc<Vec<u8>>, Vec<StepLog>), String> {
        // A fresh session per file bounds retained document state and never touches open tabs.
        let mut session = Session::new();
        let mut input = Vec::new();
        std::fs::File::open(path).map_err(|e| e.to_string())?.take(MAX_INPUT + 1).read_to_end(&mut input).map_err(|e| e.to_string())?;
        if input.len() as u64 > MAX_INPUT {
            return Err("input exceeds 512 MiB".into());
        }
        let name = path.file_name().and_then(std::ffi::OsStr::to_str).ok_or("input name is not UTF-8")?;
        let mut bytes = Arc::new(input);
        let mut log = Vec::new();
        for step in &self.steps {
            let before = bytes.len();
            match step {
                Step::Convert => bytes = session.convert_to_pdf(name, &bytes).map_err(|e| e.to_string())?.1,
                Step::Ocr => {
                    let engine = ocr::engine()?;
                    let result = ocr::recognize_file(name, bytes, None, self.ocr.clone(), &engine, &mut progress)?;
                    log.push(StepLog::Ocr { words: result.words(), skipped_pages: result.pages.iter().filter(|p| p.skipped.is_some()).count() });
                    bytes = result.bytes;
                    continue;
                }
                Step::Optimize => {
                    let id = session.open(name, None, bytes.clone(), None).map_err(|e| e.to_string())?;
                    let settings = optimize::Settings { color: self.images, gray: self.images, mono: self.mono, ..Default::default() };
                    let (candidate, _) = session.optimized_bytes(id, &settings, &[]).map_err(|e| e.to_string())?;
                    session.close(id);
                    if candidate.len() < bytes.len() {
                        bytes = candidate;
                    }
                }
            }
            log.push(StepLog::Size { step: *step, before, after: bytes.len() });
        }
        Ok((bytes, log))
    }
}

/// Whether `metadata` is a link (or, on Windows, a junction): never followed.
pub fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// `path` with `.` and `..` resolved by name. `None` if a `..` would climb above its start.
fn lexical(path: &Path) -> Option<PathBuf> {
    let mut parts: Vec<Component> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                _ => return None,
            },
            other => parts.push(other),
        }
    }
    Some(parts.iter().collect())
}

/// `path` made absolute through its existing ancestor, so an output folder that doesn't exist
/// yet compares with the inputs the same way it will once made.
pub fn absolute(path: &Path) -> Result<PathBuf, String> {
    let mut ancestor = path;
    let mut tail = Vec::new();
    while !ancestor.exists() {
        if ancestor.symlink_metadata().is_ok() {
            return Err("broken link in batch path".into());
        }
        tail.push(ancestor.file_name().ok_or("invalid batch path")?);
        ancestor = ancestor.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    }
    let mut path = ancestor.canonicalize().map_err(|e| e.to_string())?;
    path.extend(tail.iter().rev());
    lexical(&path).ok_or_else(|| "invalid batch path".into())
}

/// Why a found path is left alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    LinkOrSpecial,
    Unsupported,
    Stopped,
}

impl SkipReason {
    pub fn text(self) -> &'static str {
        match self {
            Self::LinkOrSpecial => "link or special file",
            Self::Unsupported => "unsupported input extension",
            Self::Stopped => "stopped after error",
        }
    }
}

/// The files a job will read, each with its path relative to the folder it was found in (a file
/// named directly: its own name), and what was found and left alone.
#[derive(Debug, Default)]
pub struct Discovery {
    pub files: BTreeMap<PathBuf, PathBuf>,
    pub skipped: Vec<(PathBuf, SkipReason)>,
    seen: HashSet<PathBuf>,
    visited: usize,
}

impl Discovery {
    /// Check the inputs against `out` and find the files under them: the files named, and the
    /// files in the folders named (and their subfolders when `recursive`). Links are not
    /// followed and the output folder's tree is never read. An output folder that equals or
    /// contains an input folder is refused; one inside an input folder is left out of it.
    pub fn find(roots: &[PathBuf], out: &Path, recursive: bool) -> Result<Self, String> {
        let mut found = Self::default();
        for root in roots {
            if root.is_dir() && root.starts_with(out) {
                return Err("the output folder must not contain an input folder or equal it".into());
            }
            let relative = if root.is_dir() { PathBuf::new() } else { PathBuf::from(root.file_name().ok_or("input has no name")?) };
            found.walk(root, &relative, out, recursive, 0)?;
        }
        Ok(found)
    }

    fn walk(&mut self, path: &Path, relative: &Path, out: &Path, recursive: bool, depth: usize) -> Result<(), String> {
        self.visited += 1;
        if self.visited > MAX_ENTRIES || depth > MAX_DEPTH {
            return Err("batch discovery exceeds 10000 entries or 64 folder levels".into());
        }
        if depth > 0 && path.starts_with(out) {
            return Ok(());
        }
        let metadata = path.symlink_metadata().map_err(|e| format!("{}: {e}", path.display()))?;
        if linked(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
            self.skipped.push((path.to_path_buf(), SkipReason::LinkOrSpecial));
        } else if (metadata.is_file() || depth == 0 || recursive) && self.seen.insert(path.to_path_buf()) {
            if metadata.is_file() {
                self.files.insert(path.to_path_buf(), relative.to_path_buf());
            } else if depth == 0 || recursive {
                // Bound the list before sorting it; never collect an unbounded folder listing.
                let mut entries = Vec::new();
                for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                    if entries.len() >= MAX_ENTRIES.saturating_sub(self.visited) {
                        return Err("batch discovery exceeds 10000 entries".into());
                    }
                    entries.push(entry.map_err(|e| e.to_string())?.path());
                }
                entries.sort();
                for child in entries {
                    let name = child.file_name().ok_or("entry has no name")?;
                    self.walk(&child, &relative.join(name), out, recursive, depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

/// Where a found file's result goes: its relative path under `out`, as a PDF.
pub fn output_path(out: &Path, relative: &Path) -> PathBuf {
    out.join(relative).with_extension("pdf")
}

/// Write `bytes` at `target` atomically without replacing any existing name, even when another
/// writer wins a race: a taken name gets a number. Returns the path written.
pub fn publish(target: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = target.parent().ok_or("output has no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    temp.write_all(bytes).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    let stem = target.file_stem().ok_or("output has no stem")?.to_string_lossy();
    for n in 1..=10_000 {
        let candidate = if n == 1 { target.to_path_buf() } else { dir.join(format!("{stem} ({n}).pdf")) };
        match temp.persist_noclobber(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists || candidate.symlink_metadata().is_ok() => temp = e.file,
            Err(e) => return Err(e.error.to_string()),
        }
    }
    Err("all 10000 output names are occupied".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_limits_fail_before_reading_another_entry() {
        let mut discovery = Discovery { visited: MAX_ENTRIES, ..Default::default() };
        assert!(discovery.walk(Path::new("absent"), Path::new(""), Path::new("output"), true, 0).unwrap_err().contains("10000"));
        discovery.visited = 0;
        assert!(discovery.walk(Path::new("absent"), Path::new(""), Path::new("output"), true, MAX_DEPTH + 1).unwrap_err().contains("64"));
    }

    #[test]
    fn an_output_folder_that_is_or_holds_an_input_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let scans = dir.path().join("scans");
        std::fs::create_dir_all(&scans).unwrap();
        for out in [scans.clone(), dir.path().to_path_buf()] {
            assert!(Discovery::find(std::slice::from_ref(&scans), &out, true).unwrap_err().contains("output folder"));
        }
        // A folder inside the input is allowed, and its tree is never read back.
        let inside = scans.join("ocr");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("done.pdf"), b"%PDF").unwrap();
        std::fs::write(scans.join("a.pdf"), b"%PDF").unwrap();
        let found = Discovery::find(std::slice::from_ref(&scans), &inside, true).unwrap();
        assert_eq!(found.files.into_values().collect::<Vec<_>>(), vec![PathBuf::from("a.pdf")]);
    }

    #[test]
    fn publish_numbers_a_taken_name_instead_of_replacing_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("scan.pdf");
        std::fs::write(&target, b"original").unwrap();
        let written = publish(&target, b"new").unwrap();
        assert_eq!(written, dir.path().join("scan (2).pdf"));
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
    }
}
