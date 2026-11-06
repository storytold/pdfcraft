//! Text recognition of scanned pages in the background, so their words can be selected, found and
//! copied as soon as they are read, as browsers do with scans. The recognised text is the view's
//! own: it becomes the page's text layer but is never added to the document, which stays
//! unchanged (Scan & OCR ▸ Recognize text still adds it, as an undoable edit, to save it).
//!
//! It starts once a page's text layer is read and found empty, and reads the pages that have
//! pictures, from the current page on; pages with text of their own are skipped. One document is
//! read at a time, and a page's words become its text layer as soon as that page is done. It needs
//! the recognition models, and can be turned off in Preferences.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use pdfcraft_engine::ocr::{EngineChoice, OcrPage, OcrSettings};

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
    /// Pages read whose pictures held no words (a photo): not read again when another page's
    /// content changes, only when their own does. Those with words are the view's `ocr_texts`.
    read_empty: HashSet<usize>,
    /// Workers of this view's runs stopped before they finished.
    retiring: Retiring,
}

/// Workers of recognitions stopped before they finished: each still reads the page it is on.
/// None starts until they are done, so edits in a row (dragging a picture on a scan) never leave
/// several reading at once, each with a page rendered at 300 dpi; they ran out of memory.
#[derive(Default)]
pub(crate) struct Retiring(Vec<Arc<Mutex<Shared>>>);

impl Retiring {
    /// Whether one is still reading. Those that are done are forgotten.
    fn still_reading(&mut self) -> bool {
        // A worker that panicked marks its chunk finished all the same (`Finished`).
        self.0.retain(|s| !s.lock().unwrap_or_else(std::sync::PoisonError::into_inner).finished);
        !self.0.is_empty()
    }

    /// Take over `ocr`'s workers, its running one included: its view is closing or being
    /// replaced, and they are waited for here.
    pub(crate) fn take_from(&mut self, ocr: &mut AutoOcr) {
        ocr.retire();
        self.0.append(&mut ocr.retiring.0);
    }
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

/// Marks its chunk finished when dropped: when the worker returns, and when it unwinds from a
/// panic (a page that crashes the renderer or the recogniser), so the view never waits for a
/// chunk that will not finish.
struct Finished(Arc<Mutex<Shared>>);

impl Drop for Finished {
    fn drop(&mut self) {
        // Taken while unwinding, the lock isn't poisoned by it; one poisoned before still holds
        // the state the view needs to see.
        let mut s = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        s.finished = true;
    }
}

/// A worker's work on one chunk: `read` reads its pages, reporting progress (`false` back means
/// stop) and handing over each page as soon as it is read.
fn run_chunk(shared: &Arc<Mutex<Shared>>, read: impl FnOnce(&mut dyn FnMut(usize, usize) -> bool, &mut dyn FnMut(OcrPage))) {
    let _finished = Finished(shared.clone());
    let mut progress = |done: usize, _total: usize| {
        let Ok(mut s) = shared.lock() else { return false };
        s.done = done;
        !s.cancel
    };
    let mut found = |page: OcrPage| {
        if let Ok(mut s) = shared.lock() {
            s.found.push(page);
        }
    };
    read(&mut progress, &mut found);
}

impl AutoOcr {
    /// Forget everything: the pages may have moved, so it starts again if a page is still empty.
    /// Workers still reading are waited for all the same.
    pub(crate) fn reset(&mut self) {
        self.retire();
        let retiring = std::mem::take(&mut self.retiring);
        *self = AutoOcr { retiring, ..AutoOcr::default() };
    }

    /// End the run. Dropping its chunk asks the worker to stop; one still reading its page is
    /// kept, to be waited for.
    fn retire(&mut self) {
        if let Some(chunk) = self.run.take().and_then(|run| run.chunk) {
            self.retiring.0.push(chunk.shared.clone());
        }
    }

    /// This view replaces `old`'s (its document was reloaded from a changed file): `old`'s
    /// workers, still reading the old version, are waited for before this one reads.
    pub(crate) fn take_over(&mut self, old: &mut AutoOcr) {
        self.retiring.take_from(old);
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
        self.retire();
        self.done = true;
        self.stopped = true;
    }

    /// `page`'s content changed (its text, pictures or added items): a run may be reading the
    /// old content, so it stops, and what its worker read goes with it. Recognition starts again
    /// for the pages not recognised yet, `page` among them, unless it was stopped.
    pub(crate) fn content_changed(&mut self, page: usize) {
        self.read_empty.remove(&page);
        self.retire();
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
        for page in found {
            let Some(info) = pages.get(page.page) else { continue };
            let text = Arc::new(page.view_text(info));
            if text.is_empty() {
                // Read, and no words in its pictures: not read again. One skipped (it has text of
                // its own, or could not be read) is looked at again.
                if page.skipped.is_none() {
                    self.auto_ocr.read_empty.insert(page.page);
                }
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
        // Turned off in Preferences, or while Recognize text runs (asked for, or Edit's on a scan),
        // so the two never read at once: runs end where they are, and start again from the pages
        // not recognised yet afterwards (`stop` would keep them from starting again).
        if !self.auto_ocr || self.ocr_run.is_some() {
            for view in &mut self.views {
                // What its worker has read already is kept.
                if let Some(doc) = self.session.get(view.id) {
                    view.take_recognised(&doc.info.pages);
                }
                view.auto_ocr.retire();
            }
        }
        let mut busy = self.auto_ocr_closed.still_reading();
        for index in 0..self.views.len() {
            self.step_auto_ocr(index);
            busy |= self.views.get_mut(index).is_some_and(|v| v.auto_ocr.running() || v.auto_ocr.retiring.still_reading());
        }
        if busy {
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
        // The built-in engine's models are looked for once per document (a cached answer, never a
        // wait); without them the bar offers Recognize text. Tesseract is never asked: its probe
        // is a process of its own, and the background reads with the built-in engine only.
        if pdfcraft_engine::ocr::engine_unavailable_reason(EngineChoice::Ocrs).is_some() {
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
            // A page read already, before another page's content changed, is not read again.
            let read = view.ocr_texts.contains_key(&page) || view.auto_ocr.read_empty.contains(&page);
            if read || doc.page_images(page).is_empty() {
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
        // Comments are not part of the scan, as for Edit: they don't count as its text and aren't
        // read. A comment edit keeps the page's recognised text, so their words would outlive them.
        let settings = OcrSettings {
            language: self.ocr_draft.language.clone(),
            skip_text_pages: true,
            native_text_only: true,
            engine: EngineChoice::Ocrs,
            ..OcrSettings::default()
        };
        let Some(job) = self.session.ocr_job(view.id, &pages, settings) else {
            view.auto_ocr.stop();
            return;
        };
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker = shared.clone();
        let work = move || {
            run_chunk(&worker, |progress, found| {
                if let Ok(engines) = pdfcraft_engine::ocr::recognizers(&job.settings) {
                    job.run_each(&engines, progress, found);
                }
            });
        };
        if self.run_inline {
            work();
        } else if std::thread::Builder::new().name("pdfcraft-auto-ocr".into()).spawn(work).is_err() {
            // No worker took the pages, so there is none to wait for.
            view.auto_ocr.stop();
            return;
        }
        run.chunk = Some(Chunk { shared, pages: pages.len() });
    }
}

/// The progress of a background recognition, at the bottom of the document area, with Cancel.
/// Not drawn while a panel or dialog covers the page area (`unobstructed` false); the flag is
/// taken here, not tested by the caller, to keep `document_area` within clippy's complexity limit.
pub(crate) fn progress_bar(ui: &egui::Ui, viewport: egui::Rect, view: &mut DocView, unobstructed: bool) {
    let Some((page, total)) = view.auto_ocr.progress().filter(|_| unobstructed) else { return };
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

    /// A document of `pages` pages that are each only a picture, as a scan's are.
    fn scan_pdf(pages: usize) -> Vec<u8> {
        let images: Vec<(String, Vec<u8>)> = (0..pages).map(|p| (format!("{p}.png"), png())).collect();
        Session::new().create_from_images(&images).unwrap().to_vec()
    }

    /// An app with [`scan_pdf`] open.
    fn scan(pages: usize) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.open_bytes("scan.pdf", None, scan_pdf(pages)).unwrap();
        app
    }

    /// A run whose worker is still reading page 0, and that worker's state.
    fn reading() -> (Arc<Mutex<Shared>>, Run) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let chunk = Chunk { shared: shared.clone(), pages: 1 };
        (shared, Run { queue: VecDeque::new(), pending: Vec::new(), total: 1, handled: 0, chunk: Some(chunk) })
    }

    /// Whether recognition of view `index` started, or found it cannot run here (no models).
    fn started(app: &PdfCraftApp, index: usize) -> bool {
        let ocr = &app.views[index].auto_ocr;
        ocr.running() || ocr.done
    }

    /// What recognition read on `page`: one word.
    fn read(page: usize, word: &str) -> OcrPage {
        let word = PlacedWord { text: word.into(), origin: [2.0, 2.0], across: [4.0, 0.0], up: [0.0, 2.0], confidence: None, source: String::new() };
        OcrPage { page, words: vec![word], already_on_page: 0, skipped: None, notes: Vec::new() }
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

    /// Edits in a row on a scan (dragging a picture across it) each stopped the running worker and
    /// started another while the first still read its page: several read at once, each with a
    /// page rendered at 300 dpi, until memory ran out. A worker still reading is waited for.
    #[test]
    fn a_page_edit_waits_for_the_worker_still_reading() {
        let mut app = scan(1);
        let (shared, run) = reading();
        app.views[0].auto_ocr = AutoOcr { wanted: true, run: Some(run), ..Default::default() };
        let picture = Edit::AddImage { page: 0, rect: Some([1.0, 1.0, 4.0, 4.0]), name: "stamp.png".into(), bytes: Arc::new(png()) };
        assert!(app.apply_edit(picture));
        app.poll_auto_ocr();
        assert!(shared.lock().unwrap().cancel, "the worker is asked to stop");
        assert!(!started(&app, 0), "recognition started again beside the worker still reading");
        shared.lock().unwrap().finished = true;
        app.poll_auto_ocr();
        assert!(started(&app, 0), "recognition starts again once that worker is done");
    }

    /// A document reloaded from its changed file gets a fresh view: the old view's worker, still
    /// reading the old version, is waited for before the new one is read (CodeRabbit on #3860).
    #[test]
    fn a_reloaded_document_waits_for_the_old_version_s_worker() {
        let mut app = scan(1);
        let (shared, run) = reading();
        app.views[0].auto_ocr = AutoOcr { wanted: true, run: Some(run), ..Default::default() };
        let info = app.session.get(app.views[0].id).unwrap().info.clone();
        app.views[0].document_replaced(&info);
        // The new version's text layer comes back empty too.
        app.views[0].auto_ocr.wanted = true;
        app.poll_auto_ocr();
        assert!(shared.lock().unwrap().cancel, "the old version's worker is asked to stop");
        assert!(!started(&app, 0), "the new version was read beside the old one");
        shared.lock().unwrap().finished = true;
        app.poll_auto_ocr();
        assert!(started(&app, 0), "it is read once the old version's worker is done");
    }

    /// A closed tab's worker, still reading a page, is waited for before another document is read
    /// (CodeRabbit on #3860).
    #[test]
    fn a_closed_tab_s_worker_is_waited_for() {
        let mut app = scan(1);
        app.open_bytes("other.pdf", None, scan_pdf(1)).unwrap();
        assert_eq!(app.views.len(), 2);
        let (shared, run) = reading();
        app.views[0].auto_ocr = AutoOcr { wanted: true, run: Some(run), ..Default::default() };
        app.views[1].auto_ocr.wanted = true;
        app.close_tab(0);
        app.poll_auto_ocr();
        assert!(shared.lock().unwrap().cancel, "the closed tab's worker is asked to stop");
        assert!(!started(&app, 0), "another document was read beside it");
        shared.lock().unwrap().finished = true;
        app.poll_auto_ocr();
        assert!(started(&app, 0), "it is read once that worker is done");
    }

    /// Turning background recognition off in Preferences ends a run where it is: its worker stops
    /// and its progress bar goes. Turned on again, it starts again, unlike after Cancel
    /// (CodeRabbit on #3860).
    #[test]
    fn turning_background_recognition_off_pauses_it() {
        let mut app = scan(1);
        let (shared, run) = reading();
        app.views[0].auto_ocr = AutoOcr { wanted: true, run: Some(run), ..Default::default() };
        app.auto_ocr = false;
        app.poll_auto_ocr();
        assert!(shared.lock().unwrap().cancel, "the worker is asked to stop");
        assert_eq!(app.views[0].auto_ocr_progress(), None, "still running, with its progress bar");
        shared.lock().unwrap().finished = true;
        app.poll_auto_ocr();
        assert!(!started(&app, 0), "started again while turned off");
        app.auto_ocr = true;
        app.poll_auto_ocr();
        assert!(started(&app, 0), "turned on again, it starts again");
    }

    /// While Recognize text runs (asked for, or Edit's on a scan), background recognition pauses,
    /// so the two never read at once, and starts again once that run is over (CodeRabbit on
    /// #3860).
    #[test]
    fn recognize_text_pauses_background_recognition() {
        let mut app = scan(1);
        let (shared, run) = reading();
        app.views[0].auto_ocr = AutoOcr { wanted: true, run: Some(run), ..Default::default() };
        let doc = app.views[0].id;
        let guard = app.session.ocr_guard(doc).unwrap();
        app.ocr_run = Some(crate::ocr_ui::OcrRun { doc, progress: Default::default(), guard, automatic: None, editable: false });
        app.poll_auto_ocr();
        assert!(shared.lock().unwrap().cancel, "the background worker is asked to stop");
        assert!(!app.views[0].auto_ocr.running(), "background recognition kept reading beside Recognize text");
        shared.lock().unwrap().finished = true;
        app.poll_auto_ocr();
        assert!(!started(&app, 0), "started beside Recognize text");
        app.ocr_run = None;
        app.poll_auto_ocr();
        assert!(started(&app, 0), "it starts again once Recognize text is over");
    }

    /// Cancel holds through a content edit: recognition doesn't start again on its own.
    #[test]
    fn a_stopped_recognition_stays_stopped_when_a_page_changes() {
        let mut ocr = AutoOcr { wanted: true, done: true, ..Default::default() };
        ocr.content_changed(0);
        assert!(!ocr.done, "a finished recognition starts again for the changed page");
        ocr.stop();
        ocr.content_changed(0);
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

    /// Pictures read and found to hold no words (photos) are not read again whenever another
    /// page's content changes, only when their own does; a page that could not be read is tried
    /// again (CodeRabbit on #3860).
    #[test]
    fn pages_read_with_no_words_are_not_read_again() {
        let mut app = scan(3);
        app.run_inline = true;
        let empty = |page| OcrPage { page, words: Vec::new(), already_on_page: 0, skipped: None, notes: Vec::new() };
        let failed = OcrPage { skipped: Some("the page could not be drawn".into()), ..empty(1) };
        let shared = Shared { found: vec![empty(0), failed, empty(2)], finished: true, ..Default::default() };
        let chunk = Chunk { shared: Arc::new(Mutex::new(shared)), pages: 3 };
        app.views[0].auto_ocr.run = Some(Run { queue: VecDeque::new(), pending: Vec::new(), total: 3, handled: 0, chunk: Some(chunk) });
        let pages = app.session.get(app.views[0].id).unwrap().info.pages.clone();
        app.views[0].take_recognised(&pages);
        // Page 2's content changes: recognition starts again.
        app.views[0].content_page_changed(2);
        app.views[0].auto_ocr.run = Some(Run { queue: (0..3).collect(), pending: Vec::new(), total: 3, handled: 0, chunk: None });
        app.step_auto_ocr(0);
        let run = app.views[0].auto_ocr.run.as_ref().expect("reading");
        assert_eq!(run.chunk.as_ref().map(|c| c.pages), Some(2), "page 2, whose content changed, and page 1, not read, are read");
        assert_eq!(run.handled, 1, "page 0, read with no words, is passed over");
    }

    /// A page that crashes the renderer or the recogniser panics the worker reading it: its chunk
    /// still finishes, keeping the pages read before, so the view stops waiting and its progress
    /// bar goes (CodeRabbit on #3860).
    #[test]
    fn a_chunk_finishes_when_its_worker_panics() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker = shared.clone();
        let ran = std::thread::spawn(move || {
            run_chunk(&worker, |progress, found| {
                assert!(progress(0, 2));
                found(read(0, "Read"));
                panic!("a page crashed the recogniser");
            });
        })
        .join();
        assert!(ran.is_err(), "the worker panicked");
        let mut app = scan(2);
        app.auto_ocr = false;
        let run = Run { queue: VecDeque::new(), pending: Vec::new(), total: 2, handled: 0, chunk: Some(Chunk { shared, pages: 2 }) };
        app.views[0].auto_ocr.run = Some(run);
        app.poll_auto_ocr();
        let view = &app.views[0];
        assert_eq!(view.auto_ocr_progress(), None, "the view still waits for the worker");
        assert_eq!(view.page_plain_text(0).as_deref(), Some("Read"), "the page read before the panic is kept");
    }
}
