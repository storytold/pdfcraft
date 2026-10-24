//! File ▸ Open next / previous PDF in folder (#4267): step through the PDFs beside the open
//! document, for reading a folder of invoices or scans one after another.

#[cfg(not(target_arch = "wasm32"))]
use std::cmp::Ordering;

use crate::PdfCraftApp;
#[cfg(not(target_arch = "wasm32"))]
use crate::combine_ui::natural;

/// Of the file names in a folder, the PDF after `current` (before it when `forward` is false):
/// names ending in `.pdf` in any case, not hidden ones, in natural order ("scan2" before
/// "scan10"). `current` need not be among them, so a file renamed or deleted since it was opened
/// still has its place. `None` at either end.
#[cfg(not(target_arch = "wasm32"))]
fn pdf_beside<'a>(names: &'a [String], current: &str, forward: bool) -> Option<&'a str> {
    // Natural order alone ties names that differ only in case; the bytes settle those.
    let order = |a: &str, b: &str| natural(a, b).then_with(|| a.cmp(b));
    let pdfs = names.iter().map(String::as_str).filter(|n| is_listed_pdf(n));
    if forward {
        pdfs.filter(|n| order(n, current) == Ordering::Greater).min_by(|a, b| order(a, b))
    } else {
        pdfs.filter(|n| order(n, current) == Ordering::Less).max_by(|a, b| order(a, b))
    }
}

/// A PDF the step visits: `.pdf` in any case, with a name before it, and not hidden (macOS
/// leaves `._name.pdf` resource files beside PDFs on some drives, and they are not PDFs).
#[cfg(not(target_arch = "wasm32"))]
fn is_listed_pdf(name: &str) -> bool {
    !name.starts_with('.') && name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && ext.eq_ignore_ascii_case("pdf"))
}

/// [`pdf_beside`] for the file at `path`, reading its folder once. Folders named like a PDF are
/// left out; a link to a PDF counts. Names that aren't valid Unicode are skipped.
#[cfg(not(target_arch = "wasm32"))]
fn pdf_beside_on_disk(path: &std::path::Path, forward: bool) -> std::io::Result<Option<std::path::PathBuf>> {
    let Some(current) = path.file_name() else { return Ok(None) };
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        if is_listed_pdf(&name) && entry.path().is_file() {
            names.push(name);
        }
    }
    Ok(pdf_beside(&names, &current.to_string_lossy(), forward).map(|name| dir.join(name)))
}

/// A folder scan on its way: the file the step started from, so a notice can name the folder,
/// and the worker's answer.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct FolderStep {
    path: String,
    forward: bool,
    receiver: std::sync::mpsc::Receiver<std::io::Result<Option<std::path::PathBuf>>>,
}

impl PdfCraftApp {
    /// A folder scan is still on its way (File ▸ Open next / previous PDF in folder).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn folder_step_pending(&self) -> bool {
        self.folder_step.is_some()
    }

    /// Open the PDF after the active document's file in its folder (before it when `forward` is
    /// false). It opens like any other file, in its own tab, so the document shown keeps its tab
    /// and its unsaved changes; a file already open comes to the front. The folder is read on a
    /// worker, so a slow network drive never holds the frame (inline in tests, like the opens).
    pub(crate) fn open_pdf_beside(&mut self, forward: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.folder_step.is_some() {
                return;
            }
            let Some(path) = self.active_ids().and_then(|(_, id)| self.session.get(id)).and_then(|d| d.path.clone()) else {
                self.notify_tr("This document isn't in a folder yet: save it first");
                return;
            };
            let inline = self.ctx.is_none() || self.run_inline;
            let (sender, receiver) = std::sync::mpsc::channel();
            let scan = path.clone();
            let ctx = self.ctx.clone();
            let work = move || {
                let _ = sender.send(pdf_beside_on_disk(std::path::Path::new(&scan), forward));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            };
            if inline {
                work();
            } else if let Err(e) = std::thread::Builder::new().name("pdfcraft-folder-step".into()).spawn(work) {
                let folder = std::path::Path::new(&path).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &folder), ("e", &e.to_string())]);
                return;
            }
            self.folder_step = Some(FolderStep { path, forward, receiver });
            // A worker's answer waits for the next frame; an inline one is already here.
            if inline {
                self.poll_folder_step();
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = forward;
    }

    /// Act on a finished folder scan, if there is one.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn poll_folder_step(&mut self) {
        let Some(step) = &self.folder_step else { return };
        let done = match step.receiver.try_recv() {
            Ok(done) => done,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            // The worker ended without an answer (it panicked): stop waiting for it.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.folder_step = None;
                return;
            }
        };
        let Some(FolderStep { path, forward, .. }) = self.folder_step.take() else { return };
        match done {
            Ok(Some(next)) => {
                self.open_path(&next.to_string_lossy());
            }
            Ok(None) if forward => self.notify_tr("This is the last PDF in its folder"),
            Ok(None) => self.notify_tr("This is the first PDF in its folder"),
            Err(e) => {
                let folder = std::path::Path::new(&path).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &folder), ("e", &e.to_string())]);
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_next_pdf_is_the_one_after_in_natural_order() {
        let folder = names(&["scan10.pdf", "scan2.pdf", "scan1.pdf", "notes.txt", "scan3.pdf.txt"]);
        assert_eq!(pdf_beside(&folder, "scan1.pdf", true), Some("scan2.pdf"));
        assert_eq!(pdf_beside(&folder, "scan2.pdf", true), Some("scan10.pdf"));
        assert_eq!(pdf_beside(&folder, "scan10.pdf", true), None);
        assert_eq!(pdf_beside(&folder, "scan10.pdf", false), Some("scan2.pdf"));
        assert_eq!(pdf_beside(&folder, "scan2.pdf", false), Some("scan1.pdf"));
        assert_eq!(pdf_beside(&folder, "scan1.pdf", false), None);
    }

    #[test]
    fn only_visible_pdfs_count_in_any_case() {
        let folder = names(&["a.pdf", "B.PDF", "c.Pdf", ".d.pdf", "._a.pdf", ".pdf", "e.pdfx"]);
        assert_eq!(pdf_beside(&folder, "a.pdf", true), Some("B.PDF"));
        assert_eq!(pdf_beside(&folder, "B.PDF", true), Some("c.Pdf"));
        assert_eq!(pdf_beside(&folder, "c.Pdf", true), None);
        assert_eq!(pdf_beside(&folder, "a.pdf", false), None);
    }

    #[test]
    fn a_file_gone_from_the_folder_keeps_its_place() {
        let folder = names(&["a.pdf", "c.pdf"]);
        assert_eq!(pdf_beside(&folder, "b.pdf", true), Some("c.pdf"));
        assert_eq!(pdf_beside(&folder, "b.pdf", false), Some("a.pdf"));
        assert_eq!(pdf_beside(&[], "b.pdf", true), None);
    }

    #[test]
    fn names_differing_only_in_case_are_both_visited() {
        let folder = names(&["Report.pdf", "report.pdf"]);
        assert_eq!(pdf_beside(&folder, "Report.pdf", true), Some("report.pdf"));
        assert_eq!(pdf_beside(&folder, "report.pdf", false), Some("Report.pdf"));
    }
}
