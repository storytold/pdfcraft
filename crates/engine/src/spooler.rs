//! Sending a print-ready PDF to a printer.
//!
//! macOS and Linux hand the PDF to CUPS (`lp`), which prints PDF itself. Windows has no such
//! spooler command, so there the print-ready PDF's pages are drawn to images and printed through
//! the operating system's own printing classes (`pdfcraft_print::spool::submit_images`), which
//! needs no PDF viewer and works with every printer driver.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfcraft_print::PrintError;
use pdfcraft_print::spool::Job;

use crate::export::{ExportSource, Exporter};

/// Resolution the pages are drawn at for a printer that is sent images.
pub const PRINT_DPI: f64 = 300.0;

/// The most pages one job rasterizes: a bound on temporary disk space, not a page limit worth
/// reaching (imposed sheets, not source pages, are counted).
pub const MAX_RASTER_PAGES: usize = 2000;

/// Send `pdf` (a print-ready PDF from `Session::print_pdf`) to the printer `job` names. Returns
/// the spooler's message (a job id on CUPS; empty on Windows, where the system takes the job
/// without one).
pub fn print_to_printer(pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    #[cfg(windows)]
    {
        windows_print(pdf, job)
    }
    #[cfg(not(windows))]
    {
        pdfcraft_print::spool::submit(pdf, job)
    }
}

#[cfg(windows)]
fn windows_print(pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    let dir = private_dir().map_err(|e| PrintError::Spool(format!("the print job could not be prepared: {e}")))?;
    let result = rasterize_pages(pdf, &dir).and_then(|_| pdfcraft_print::spool::submit_images(&dir, job));
    // The pages are the user's document: never leave them behind, whatever happened.
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Draw every page of `pdf` as `page-0001.png`, `page-0002.png`, … in `dir` at [`PRINT_DPI`].
/// Returns the files in page order.
pub fn rasterize_pages(pdf: &[u8], dir: &Path) -> Result<Vec<PathBuf>, PrintError> {
    let bytes = Arc::new(pdf.to_vec());
    let info =
        pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| PrintError::Spool(format!("the pages could not be read for printing: {e}")))?;
    let pages = info.pages.len();
    if pages == 0 {
        return Err(PrintError::NoPages);
    }
    if pages > MAX_RASTER_PAGES {
        return Err(PrintError::Spool(format!(
            "this job has {pages} sheets; print at most {MAX_RASTER_PAGES} at a time, or save the print-ready PDF"
        )));
    }
    let sizes = info.pages.iter().map(|p| (p.width, p.height)).collect();
    let mut exporter = Exporter::from_source(ExportSource { bytes, config: Default::default(), pages, sizes });
    let mut files = Vec::with_capacity(pages);
    for page in 0..pages {
        let png = exporter.png(page, PRINT_DPI).map_err(PrintError::Spool)?;
        let path = dir.join(format!("page-{:04}.png", page + 1));
        std::fs::write(&path, png).map_err(|e| PrintError::Spool(format!("the print job could not be prepared: {e}")))?;
        files.push(path);
    }
    Ok(files)
}

/// A new, empty folder in the user's temporary folder, with an unpredictable name. `create_dir`
/// (not `create_dir_all`) refuses a name that already exists, so a folder planted in advance is
/// never used.
#[cfg(any(windows, test))]
fn private_dir() -> std::io::Result<PathBuf> {
    use std::hash::BuildHasher;
    let state = std::hash::RandomState::new();
    let base = std::env::temp_dir();
    for i in 0u64..16 {
        let dir = base.join(format!("pdfcraft-print-{:016x}", state.hash_one(i)));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "every temporary folder name tried is taken"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dirs_are_new_and_distinct() {
        let a = private_dir().unwrap();
        let b = private_dir().unwrap();
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        let dir = private_dir().unwrap();
        assert!(rasterize_pages(b"not a pdf", &dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
