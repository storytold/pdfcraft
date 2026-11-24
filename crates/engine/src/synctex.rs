//! SyncTeX search for an open document: the `.synctex(.gz)` file beside the PDF maps a point on
//! a page to a LaTeX source line and back. Page positions here are display points, origin at the
//! top-left of the page as shown (after `/Rotate`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::time::SystemTime;

use pdfcraft_synctex::{SourcePos, SyncTex, UnknownSource};

use crate::Document;

/// Largest SyncTeX file read from disk (before unzipping, which the parser bounds itself).
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_SYNCTEX_FILE: u64 = 512 * 1024 * 1024;
/// Most inputs an [`SyncTexError::UnknownSource`] lists.
const MAX_LISTED_INPUTS: usize = 5;

/// Why a SyncTeX search found nothing.
#[derive(Debug)]
pub enum SyncTexError {
    NoFile { looked: Vec<PathBuf> },
    Unreadable(String),
    UnknownSource { wanted: PathBuf, inputs: Vec<PathBuf> },
    NothingHere,
    Parse(pdfcraft_synctex::Error),
}

impl std::fmt::Display for SyncTexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = |paths: &[PathBuf]| paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
        match self {
            Self::NoFile { looked } if looked.is_empty() => {
                write!(f, "this document has no file on disk, so there is no SyncTeX file beside it; pass `synctex` to name one")
            }
            Self::NoFile { looked } => write!(
                f,
                "no SyncTeX file found; looked for {}. Build the document with `-synctex=1` (or \\synctex=1), or pass `synctex` to name the file",
                list(looked)
            ),
            Self::Unreadable(why) => write!(f, "the SyncTeX file could not be read: {why}"),
            Self::UnknownSource { wanted, inputs } => {
                write!(f, "{} is not a source file of this document", wanted.display())?;
                if inputs.is_empty() { write!(f, "; the SyncTeX file lists no inputs") } else { write!(f, "; its inputs include {}", list(inputs)) }
            }
            Self::NothingHere => write!(f, "the SyncTeX file has no source text at that position"),
            Self::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SyncTexError {}

/// The parsed SyncTeX file last read, and what it was read from.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Cached {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    tex: Arc<SyncTex>,
}

/// On the web build nothing is read from disk, so there is no cache.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type Cache = Mutex<Option<Cached>>;

impl Document {
    /// The source position under `at` (display points) on `page` (0-based).
    pub fn synctex_inverse(&self, page: usize, at: [f64; 2], file: Option<&Path>) -> Result<SourcePos, SyncTexError> {
        let tex = self.synctex_file(file)?;
        let [x, y] = self.synctex_page_point(page, at)?;
        let sheet = u32::try_from(page.saturating_add(1)).map_err(|_| SyncTexError::NothingHere)?;
        tex.inverse(sheet, x, y).ok_or(SyncTexError::NothingHere)
    }

    /// The document TeX was run on: the first input the SyncTeX file names. `None` when it names
    /// no input.
    pub fn synctex_master(&self, file: Option<&Path>) -> Result<Option<PathBuf>, SyncTexError> {
        Ok(self.synctex_file(file)?.inputs().next().map(Path::to_path_buf))
    }

    /// Where `line` of `source` lands: its 0-based page and rectangles `[x0, y0, x1, y1]` in
    /// display points.
    pub fn synctex_forward(&self, source: &Path, line: u32, file: Option<&Path>) -> Result<(usize, Vec<[f64; 4]>), SyncTexError> {
        let tex = self.synctex_file(file)?;
        let found = match tex.forward(source, line) {
            Ok(Some(found)) => found,
            Ok(None) => return Err(SyncTexError::NothingHere),
            Err(UnknownSource) => {
                return Err(SyncTexError::UnknownSource {
                    wanted: source.to_path_buf(),
                    inputs: tex.inputs().take(MAX_LISTED_INPUTS).map(Path::to_path_buf).collect(),
                });
            }
        };
        let page = usize::try_from(found.sheet).ok().and_then(|s| s.checked_sub(1)).ok_or(SyncTexError::NothingHere)?;
        let rects = found.rects.iter().map(|r| self.synctex_rect_to_view(page, *r)).collect::<Result<Vec<_>, _>>()?;
        if rects.is_empty() {
            return Err(SyncTexError::NothingHere);
        }
        Ok((page, rects))
    }

    /// The media box of `page` (0-based).
    fn synctex_media(&self, page: usize) -> Result<[f64; 4], SyncTexError> {
        self.page_boxes().get(page).map(|b| b[0]).ok_or(SyncTexError::NothingHere)
    }

    /// A display point to a SyncTeX point (from the top-left of the media box, in user space).
    fn synctex_page_point(&self, page: usize, at: [f64; 2]) -> Result<[f64; 2], SyncTexError> {
        let media = self.synctex_media(page)?;
        let user = self.measurement_to_user(page, at).map_err(|_| SyncTexError::NothingHere)?;
        Ok([user[0] - media[0], media[3] - user[1]])
    }

    /// A SyncTeX rectangle to display points, from its four corners, normalised.
    fn synctex_rect_to_view(&self, page: usize, r: [f64; 4]) -> Result<[f64; 4], SyncTexError> {
        let media = self.synctex_media(page)?;
        let mut out = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
            let [vx, vy] = self.measurement_to_view(page, [media[0] + x, media[3] - y]).map_err(|_| SyncTexError::NothingHere)?;
            out = [out[0].min(vx), out[1].min(vy), out[2].max(vx), out[3].max(vy)];
        }
        Ok(out)
    }

    /// Where a SyncTeX file for this document would sit, first choice first: `<stem>.synctex.gz`,
    /// then `<stem>.synctex`, beside the PDF. Empty for a document with no file on disk. A caller
    /// that confines file access checks these before it lets a lookup read one.
    pub fn synctex_beside(&self) -> Vec<PathBuf> {
        let Some(pdf) = self.path.as_deref().map(Path::new) else { return Vec::new() };
        vec![pdf.with_extension("synctex.gz"), pdf.with_extension("synctex")]
    }

    /// The parsed SyncTeX file for this document: `file` if named, else `<stem>.synctex.gz`, then
    /// `<stem>.synctex` beside the PDF. Read again whenever its size or time changes.
    #[cfg(not(target_arch = "wasm32"))]
    fn synctex_file(&self, file: Option<&Path>) -> Result<Arc<SyncTex>, SyncTexError> {
        let candidates: Vec<PathBuf> = match file {
            Some(f) => vec![f.to_path_buf()],
            None => self.synctex_beside(),
        };
        if candidates.is_empty() {
            return Err(SyncTexError::NoFile { looked: Vec::new() });
        }
        let found = candidates.iter().find_map(|p| std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| (p, m)));
        let Some((path, meta)) = found else {
            return Err(SyncTexError::NoFile { looked: candidates });
        };
        let (len, modified) = (meta.len(), meta.modified().ok());
        // A poisoned lock only means another thread died holding it; the cache is rebuilt anyway.
        let mut cache = self.synctex.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cache.as_ref().filter(|c| c.path == *path && c.len == len && c.modified == modified) {
            return Ok(c.tex.clone());
        }
        if len > MAX_SYNCTEX_FILE {
            return Err(SyncTexError::Unreadable(format!("{} is larger than {} MiB", path.display(), MAX_SYNCTEX_FILE >> 20)));
        }
        let bytes = std::fs::read(path).map_err(|e| SyncTexError::Unreadable(format!("{}: {e}", path.display())))?;
        let base = path.parent().unwrap_or(Path::new(""));
        let tex = Arc::new(SyncTex::parse(&bytes, base).map_err(SyncTexError::Parse)?);
        *cache = Some(Cached { path: path.clone(), len, modified, tex: tex.clone() });
        Ok(tex)
    }

    /// The web build has no files to read.
    #[cfg(target_arch = "wasm32")]
    fn synctex_file(&self, _: Option<&Path>) -> Result<Arc<SyncTex>, SyncTexError> {
        Err(SyncTexError::NoFile { looked: Vec::new() })
    }
}
