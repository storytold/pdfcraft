//! Text recognition of scanned pages in the background, so their words can be selected, found and
//! copied as soon as they are read, as browsers do with scans. The recognised text is the view's
//! own: it becomes the page's text layer but is never added to the document, which stays
//! unchanged (Scan & OCR ▸ Recognize text still adds it, as an undoable edit, to save it).
//!
//! It starts once a page's text layer is read and found empty, and reads the pages that have
//! pictures, from the current page on; pages with text of their own are skipped. One document is
//! read at a time, and a page's words become its text layer as soon as that page is done. It needs
//! the recognition models, and can be turned off in Preferences.

use std::collections::VecDeque;
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
    /// It was stopped (Cancel), or cannot run here: a page's content changing doesn't start it
    /// again.
    stopped: bool,
    run: Option<Run>,
}

/// Pages looked at for pictures in one frame. Finding them parses each page's content, so a long
/// document is looked through a few pages at a time, never all at once on the UI thread.
const PAGES_PER_FRAME: usize = 8;
/// Pages with pictures handed to the worker at a time (capturing a job inspects their images).
const PAGES_PER_CHUNK: usize = 4;

/// A recognition in progress over the document's pages.
struct Run {
    /// Pages still to look at, from the current page on.
    queue: VecDeque<usize>,
    /// Pages with pictures found and not yet handed to a worker.
    pending: Vec<usize>,
    /// Pages in the document, and how many have been looked at and read (or passed over).
    total: usize,
    handled: usize,
    /// The worker reading the pages handed out last.
    chunk: Option<Chunk>,
}

/// One worker's pages. Dropping it (the view closes, its pages change, Cancel) asks the worker
/// to stop before its next page.
struct Chunk {
    shared: Arc<Mutex<Shared>>,
    pages: usize,
}

#[derive(Default)]
struct Shared {
    /// Pages of this chunk finished.
    done: usize,
    /// Pages read and not yet taken by the view.
    found: Vec<OcrPage>,
    finished: bool,
    cancel: bool,
}

impl Drop for Chunk {
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

    /// The page being read (1-based) and the pages in the document, while recognition runs.
    pub(crate) fn progress(&self) -> Option<(usize, usize)> {
        let run = self.run.as_ref()?;
        let reading = run.chunk.as_ref().and_then(|c| c.shared.lock().ok().map(|s| s.done)).unwrap_or(0);
        let total = run.total.max(1);
        Some((run.handled.saturating_add(reading).saturating_add(1).min(total), total))
    }

    /// Stop, and don't start again for these pages.
    pub(crate) fn stop(&mut self) {
        self.run = None;
        self.done = true;
        self.stopped = true;
    }

    /// One page's content changed (its text, pictures or added items): a run may be reading
    /// the old content, so it stops, and what its worker read goes with it. Recognition starts
    /// again for the pages not recognised yet, the changed one among them, unless it was stopped.
    pub(crate) fn content_changed(&mut self) {
        self.run = None;
        self.done = self.stopped;
    }
}

impl DocView {
    /// Pages read in the background, as text layers of the pages they were read on.
    fn take_recognised(&mut self, pages: &[pdfcraft_render::PageInfo]) {
        let Some(chunk) = self.auto_ocr.run.as_ref().and_then(|r| r.chunk.as_ref()) else { return };
        let (found, finished) = match chunk.shared.lock() {
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
        if finished && let Some(run) = self.auto_ocr.run.as_mut() {
            run.handled = run.handled.saturating_add(run.chunk.take().map_or(0, |c| c.pages));
        }
    }
}

impl PdfCraftApp {
    /// Hand finished pages to their views, keep a recognition going, and start one for a
    /// document that needs it.
    pub(crate) fn poll_auto_ocr(&mut self) {
        let mut running = false;
        for index in 0..self.views.len() {
            self.step_auto_ocr(index);
            running |= self.views.get(index).is_some_and(|v| v.auto_ocr.running());
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
        // The web build has no threads to read in; it reads nothing on its own.
        if cfg!(target_arch = "wasm32") {
            view.auto_ocr.stop();
            return;
        }
        // Models are looked for once per document; without them the bar offers Recognize text.
        if !pdfcraft_engine::ocr::available() {
            view.auto_ocr.done = true;
            return;
        }
        let Some(doc) = self.session.get(view.id) else { return };
        // From the current page on: the pages the reader is looking at first.
        let total = doc.info.pages.len();
        let queue = (0..total).map(|k| (view.current + k) % total.max(1)).collect();
        view.auto_ocr.run = Some(Run { queue, pending: Vec::new(), total, handled: 0, chunk: None });
        self.step_auto_ocr(index);
    }

    /// One frame of a view's recognition: take what its worker has read, look through a few more
    /// pages for pictures, and hand the next ones to a worker when it is free.
    fn step_auto_ocr(&mut self, index: usize) {
        let Some(view) = self.views.get_mut(index) else { return };
        let Some(doc) = self.session.get(view.id) else { return };
        view.take_recognised(&doc.info.pages);
        let Some(run) = view.auto_ocr.run.as_mut() else { return };
        if run.chunk.is_some() {
            return;
        }
        for _ in 0..PAGES_PER_FRAME {
            if run.pending.len() >= PAGES_PER_CHUNK {
                break;
            }
            let Some(page) = run.queue.pop_front() else { break };
            // A page recognised already, before another page's content changed, is not read again.
            if view.ocr_texts.contains_key(&page) || doc.page_images(page).is_empty() {
                run.handled = run.handled.saturating_add(1);
            } else {
                run.pending.push(page);
            }
        }
        let ready = run.pending.len() >= PAGES_PER_CHUNK || (run.queue.is_empty() && !run.pending.is_empty());
        if !ready {
            if run.queue.is_empty() {
                view.auto_ocr.run = None;
                view.auto_ocr.done = true;
            }
            return;
        }
        let pages = std::mem::take(&mut run.pending);
        let settings = OcrSettings { language: self.ocr_draft.language.clone(), skip_text_pages: true, ..OcrSettings::default() };
        let Some(job) = self.session.ocr_job(view.id, &pages, settings) else {
            view.auto_ocr.stop();
            return;
        };
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker = shared.clone();
        let work = move || {
            if let Ok(ocr) = pdfcraft_engine::ocr::engine() {
                let progress = |done: usize, _total: usize| {
                    let Ok(mut s) = worker.lock() else { return false };
                    s.done = done;
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
        run.chunk = Some(Chunk { shared, pages: pages.len() });
        if self.run_inline {
            work();
        } else if std::thread::Builder::new().name("pdfcraft-auto-ocr".into()).spawn(work).is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_engine::ocr::PlacedWord;
    use pdfcraft_engine::{Edit, Session};

    fn png() -> Vec<u8> {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 200, 200, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.into_inner()
    }

    /// An app with a document of `pages` pages that are each only a picture, as a scan's are.
    fn scan(pages: usize) -> PdfCraftApp {
        let images: Vec<(String, Vec<u8>)> = (0..pages).map(|p| (format!("{p}.png"), png())).collect();
        let pdf = Session::new().create_from_images(&images).unwrap();
        let mut app = PdfCraftApp::new();
        app.open_bytes("scan.pdf", None, pdf.to_vec()).unwrap();
        app
    }

    /// What recognition read on `page`: one word.
    fn read(page: usize, word: &str) -> OcrPage {
        let word = PlacedWord { text: word.into(), origin: [2.0, 2.0], across: [4.0, 0.0], up: [0.0, 2.0] };
        OcrPage { page, words: vec![word], already_on_page: 0, skipped: None }
    }

    /// A recognition running when a page's content changes may be reading its old picture: what
    /// it read comes back neither as the page's recognised text nor as its text layer (CodeRabbit
    /// on #3860).
    #[test]
    fn words_read_before_a_content_edit_do_not_come_back() {
        let mut app = scan(1);
        // Only the run planted here: none starts after the edit.
        app.auto_ocr = false;
        let shared = Shared { found: vec![read(0, "Old")], finished: true, ..Default::default() };
        let chunk = Chunk { shared: Arc::new(Mutex::new(shared)), pages: 1 };
        let run = Run { queue: VecDeque::new(), pending: Vec::new(), total: 1, handled: 0, chunk: Some(chunk) };
        app.views[0].auto_ocr = AutoOcr { wanted: true, run: Some(run), ..Default::default() };
        let picture = Edit::AddImage { page: 0, rect: Some([1.0, 1.0, 4.0, 4.0]), name: "stamp.png".into(), bytes: Arc::new(png()) };
        assert!(app.apply_edit(picture));
        app.poll_auto_ocr();
        let view = &app.views[0];
        assert!(view.ocr_texts.is_empty(), "kept as the page's recognised text");
        assert_eq!(view.page_plain_text(0), None, "made the page's text layer");
        assert!(!view.auto_ocr.done, "recognition starts again, for the page as it is now");
    }

    /// Cancel holds through a content edit: recognition doesn't start again on its own.
    #[test]
    fn a_stopped_recognition_stays_stopped_when_a_page_changes() {
        let mut ocr = AutoOcr { wanted: true, done: true, ..Default::default() };
        ocr.content_changed();
        assert!(!ocr.done, "a finished recognition starts again for the changed page");
        ocr.stop();
        ocr.content_changed();
        assert!(ocr.done, "one that was stopped does not");
    }

    /// Starting again after a page's content changed, recognition reads that page and passes
    /// over the pages it recognised before, instead of reading the whole document again.
    #[test]
    fn pages_recognised_already_are_not_read_again() {
        let mut app = scan(3);
        app.run_inline = true;
        // Pages 0 and 2 were recognised; page 1's content changed since.
        let pages = app.session.get(app.views[0].id).unwrap().info.pages.clone();
        for page in [0, 2] {
            app.views[0].ocr_texts.insert(page, Arc::new(read(page, "Kept").view_text(&pages[page])));
        }
        app.views[0].auto_ocr.run = Some(Run { queue: (0..3).collect(), pending: Vec::new(), total: 3, handled: 0, chunk: None });
        app.step_auto_ocr(0);
        let run = app.views[0].auto_ocr.run.as_ref().expect("reading");
        assert_eq!(run.chunk.as_ref().map(|c| c.pages), Some(1), "only the changed page is handed to the worker");
        assert_eq!(run.handled, 2, "the recognised pages are passed over");
    }
}
