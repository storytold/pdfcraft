//! Text recognition of scanned pages in the background, so their words can be selected, found and
//! copied as soon as they are read, as browsers do with scans. The recognised text is the view's
//! own: it becomes the page's text layer but is never added to the document, which stays
//! unchanged (Scan & OCR ▸ Recognize text still adds it, as an undoable edit, to save it).
//!
//! It starts once a page's text layer is read and found empty, and reads the pages that have
//! pictures, from the current page on; pages with text of their own are skipped. One document is
//! read at a time, and a page's words become its text layer as soon as that page is done. It needs
//! the recognition models, and can be turned off in Preferences.

use std::sync::{Arc, Mutex};

use pdfcraft_engine::ocr::{OcrPage, OcrSettings};

use crate::PdfCraftApp;
use crate::canvas::DocView;

/// A view's background recognition.
#[derive(Default)]
pub(crate) struct AutoOcr {
    /// A text layer of this document came back empty: its pages may be scans.
    pub(crate) wanted: bool,
    /// Recognition ran (or was stopped, or cannot run) for the pages as they are now.
    pub(crate) done: bool,
    run: Option<Run>,
}

/// A recognition in progress. Dropping it (the view closes, its pages change, Cancel) asks the
/// worker to stop before its next page.
struct Run {
    shared: Arc<Mutex<Shared>>,
}

#[derive(Default)]
struct Shared {
    /// Pages finished and the pages to read.
    done: usize,
    total: usize,
    /// Pages read and not yet taken by the view.
    found: Vec<OcrPage>,
    finished: bool,
    cancel: bool,
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Ok(mut s) = self.shared.lock() {
            s.cancel = true;
        }
    }
}

impl AutoOcr {
    /// Forget everything: the pages may have moved, so it starts again if a page is still empty.
    pub(crate) fn reset(&mut self) {
        *self = AutoOcr::default();
    }

    pub(crate) fn running(&self) -> bool {
        self.run.is_some()
    }

    /// The page being read (1-based) and the pages to read, while recognition runs.
    pub(crate) fn progress(&self) -> Option<(usize, usize)> {
        let s = self.run.as_ref()?.shared.lock().ok()?;
        let total = s.total.max(1);
        Some(((s.done + 1).min(total), total))
    }

    /// Stop, and don't start again for these pages.
    pub(crate) fn stop(&mut self) {
        self.run = None;
        self.done = true;
    }
}

impl DocView {
    /// Pages read in the background, as text layers of the pages they were read on.
    fn take_recognised(&mut self, pages: &[pdfcraft_render::PageInfo]) {
        let Some(run) = self.auto_ocr.run.as_ref() else { return };
        let (found, finished) = match run.shared.lock() {
            Ok(mut s) => (std::mem::take(&mut s.found), s.finished),
            // A worker that panicked holding the lock has nothing more to give.
            Err(_) => (Vec::new(), true),
        };
        for page in found.into_iter().filter(|p| !p.words.is_empty()) {
            let Some(info) = pages.get(page.page) else { continue };
            let text = Arc::new(page.view_text(info));
            if text.is_empty() {
                continue;
            }
            self.ocr_texts.insert(page.page, text.clone());
            // Only a page still without text of its own takes it.
            if self.texts.get(&page.page).is_none_or(|t| t.is_empty()) {
                self.texts.insert(page.page, text);
                self.refresh_find_page(page.page);
            }
        }
        if finished {
            self.auto_ocr.run = None;
            self.auto_ocr.done = true;
        }
    }
}

impl PdfCraftApp {
    /// Hand finished pages to their views and start recognising a document that needs it.
    pub(crate) fn poll_auto_ocr(&mut self) {
        let mut running = false;
        for view in &mut self.views {
            if let Some(doc) = self.session.get(view.id) {
                view.take_recognised(&doc.info.pages);
            }
            running |= view.auto_ocr.running();
        }
        if running {
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        }
        // One recognition at a time, and none beside a Recognize text run.
        if !self.auto_ocr || self.ocr_run.is_some() {
            return;
        }
        let Some(index) = self.views.iter().position(|v| v.auto_ocr.wanted && !v.auto_ocr.done) else { return };
        self.start_auto_ocr(index);
    }

    fn start_auto_ocr(&mut self, index: usize) {
        let Some(view) = self.views.get_mut(index) else { return };
        // Models are looked for once per document; without them the bar offers Recognize text.
        if !pdfcraft_engine::ocr::available() {
            view.auto_ocr.done = true;
            return;
        }
        let Some(doc) = self.session.get(view.id) else { return };
        // Pages with pictures, from the current page on: those the reader is looking at first.
        let count = doc.info.pages.len();
        let pages: Vec<usize> = (0..count).map(|k| (view.current + k) % count.max(1)).filter(|&p| !doc.page_images(p).is_empty()).collect();
        let settings = OcrSettings { language: self.ocr_draft.language.clone(), skip_text_pages: true, ..OcrSettings::default() };
        let job = if pages.is_empty() { None } else { self.session.ocr_job(view.id, &pages, settings) };
        let Some(job) = job else {
            view.auto_ocr.done = true;
            return;
        };
        let shared = Arc::new(Mutex::new(Shared { total: job.pages.len(), ..Default::default() }));
        let worker = shared.clone();
        let work = move || {
            if let Ok(ocr) = pdfcraft_engine::ocr::engine() {
                let progress = |done: usize, total: usize| {
                    let Ok(mut s) = worker.lock() else { return false };
                    (s.done, s.total) = (done, total);
                    !s.cancel
                };
                job.run_each(&ocr, progress, |page| {
                    if let Ok(mut s) = worker.lock() {
                        s.found.push(page);
                    }
                });
            }
            if let Ok(mut s) = worker.lock() {
                s.finished = true;
            }
        };
        view.auto_ocr.run = Some(Run { shared });
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else if std::thread::Builder::new().name("pdfcraft-auto-ocr".into()).spawn(work).is_err() {
            view.auto_ocr.stop();
        }
        // The web build has no threads to read in; it reads nothing on its own.
        #[cfg(target_arch = "wasm32")]
        {
            drop(work);
            view.auto_ocr.stop();
        }
    }
}

/// The progress of a background recognition, at the bottom of the document area, with Cancel.
pub(crate) fn progress_bar(ui: &egui::Ui, viewport: egui::Rect, view: &mut DocView) {
    let Some((page, total)) = view.auto_ocr.progress() else { return };
    let message = crate::i18n::fmt(tl!("Recognizing text… page {d} of {t}"), &[("d", &page.to_string()), ("t", &total.to_string())]);
    let mut cancel = false;
    egui::Area::new(egui::Id::unique(("auto-ocr-progress", view.id.0)))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(viewport.center().x, viewport.bottom() - 16.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(message);
                    cancel = ui.button(tl!("Cancel")).clicked();
                });
            });
        });
    if cancel {
        view.auto_ocr.stop();
    }
}
