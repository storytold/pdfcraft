//! Scan & OCR ▸ Recognize text: the Recognize Text dialog (pages, language, output, resolution)
//! and the background run with its progress. Recognition happens on a worker thread; the result
//! is applied as one undoable edit when it is done.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::ocr::{LANGUAGES, OcrPage, OcrSettings};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// Which pages to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrPages {
    All,
    Current,
    Range,
}

/// The dialog's choices (kept between runs, as in Acrobat).
#[derive(Clone, Debug, PartialEq)]
pub struct OcrDraft {
    pub pages: OcrPages,
    pub from: usize,
    pub to: usize,
    pub language: String,
    /// "Downsample to" resolution.
    pub dpi: u32,
    /// Also recognize pages that already have text (pictures on them; their text isn't added again).
    pub text_pages: bool,
}

impl Default for OcrDraft {
    fn default() -> Self {
        OcrDraft { pages: OcrPages::All, from: 1, to: 1, language: "en".into(), dpi: 300, text_pages: false }
    }
}

impl OcrDraft {
    /// The recognition settings these choices make.
    pub fn settings(&self) -> OcrSettings {
        OcrSettings { dpi: self.dpi as f32, language: self.language.clone(), skip_text_pages: !self.text_pages, ..Default::default() }
    }
}

/// Progress of a run: pages done, total, the result once finished, and a cancel request.
#[derive(Default)]
pub struct OcrProgress {
    pub done: usize,
    pub total: usize,
    pub result: Option<Result<Vec<OcrPage>, String>>,
    pub cancel: bool,
    /// The remaining-time estimate, kept by the UI.
    pub eta: Eta,
}

/// Recognize text in multiple files: files done, total, the page being read in the current file,
/// the summary once finished, and a cancel request.
#[derive(Default)]
pub struct BatchProgress {
    pub done: usize,
    pub total: usize,
    /// The page being read in the current file (0-based) and the file's pages; 0 before its first.
    pub page: usize,
    pub pages: usize,
    /// Pages read in the files finished so far.
    pub pages_done: usize,
    pub message: Option<String>,
    /// Stop: the file being read is not written, the ones finished stay.
    pub cancel: bool,
    /// The remaining-time estimate, kept by the UI.
    pub eta: Eta,
}

impl BatchProgress {
    /// The progress `recognize_file` reports for the current file; `false` once cancelled.
    pub(crate) fn page(p: &Mutex<BatchProgress>, page: usize, pages: usize) -> bool {
        p.lock()
            .map(|mut s| {
                (s.page, s.pages) = (page, pages);
                !s.cancel
            })
            // A poisoned lock means the UI can't show or cancel anything: carry on.
            .unwrap_or(true)
    }

    /// A file is done: its pages count as read.
    pub(crate) fn file_done(p: &Mutex<BatchProgress>) {
        if let Ok(mut s) = p.lock() {
            s.pages_done = s.pages_done.saturating_add(s.pages);
            (s.page, s.pages) = (0, 0);
        }
    }

    /// Pages read so far, and an estimate of the pages left: the current file's rest, and the
    /// average page count so far for each file still to come.
    fn pages_read_and_left(&self) -> (f64, f64) {
        let done = (self.pages_done + self.page) as f64;
        let files_seen = (self.done + 1) as f64;
        let average = (self.pages_done + self.pages) as f64 / files_seen;
        let after = self.total.saturating_sub(self.done + 1) as f64;
        (done, self.pages.saturating_sub(self.page) as f64 + after * average)
    }
}

/// A remaining-time estimate from the pace so far. It is timed with the UI's clock (egui's input
/// time, which the web build has too) from the first time it is asked, so the time the models
/// take to load doesn't count.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Eta {
    start: Option<(f64, f64)>,
}

impl Eta {
    /// Seconds left for `left` more units, `done` units being done at `now`. `None` until at least
    /// one unit and three seconds have passed since the first call: too early to say.
    pub fn update(&mut self, now: f64, done: f64, left: f64) -> Option<f64> {
        let (t0, d0) = *self.start.get_or_insert((now, done));
        let (elapsed, units) = (now - t0, done - d0);
        // NaN.max(0.0) is 0.0: check the count first, or a bad one reads as "almost done".
        if !left.is_finite() {
            return None;
        }
        let secs = elapsed / units * left.max(0.0);
        (elapsed >= 3.0 && units >= 1.0 && secs.is_finite()).then_some(secs)
    }
}

/// "less than a minute left", "about 6 min left", "about 2 h 5 min left".
pub(crate) fn eta_text(secs: f64) -> String {
    if !secs.is_finite() || secs < 60.0 {
        return tl!("less than a minute left").to_string();
    }
    // Saturating: a nonsense estimate shows a long time rather than wrapping.
    let mins = (secs / 60.0).ceil().min(f64::from(u32::MAX)) as u64;
    if mins < 120 {
        crate::i18n::fmt(tl!("about {n} min left"), &[("n", &mins.to_string())])
    } else {
        crate::i18n::fmt(tl!("about {h} h {m} min left"), &[("h", &(mins / 60).to_string()), ("m", &(mins % 60).to_string())])
    }
}

/// A page Edit should read now (see [`PdfCraftApp::scan_to_read`]).
struct ScanToRead {
    view: usize,
    doc: DocId,
    page: usize,
    /// The document's edit generation when it was judged.
    generation: u64,
}

pub struct OcrRun {
    pub doc: DocId,
    pub progress: Arc<Mutex<OcrProgress>>,
    /// Set for a run Edit started on a scanned page, not asked for: it shows no progress, says nothing
    /// when it found no words (a page with text of its own is skipped, and Edit must not make noise),
    /// and its result waits for Read mode and dialogs and is dropped if the document changed.
    pub automatic: Option<AutoOcr>,
}

/// What an Edit-started run needs to know to refuse a result that no longer fits its document.
#[derive(Clone, Copy, Debug)]
pub struct AutoOcr {
    /// The page it reads (0-based), when it was started.
    pub page: usize,
    /// The document's edit generation when it was started: a result is for those bytes only, and
    /// any later change (a page deleted or moved, say) may put a different page at that index.
    pub generation: u64,
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len().max(1));
    let available = pdfcraft_engine::ocr::available();
    let d = &mut app.ocr_draft;
    d.to = d.to.clamp(1, pages);
    d.from = d.from.clamp(1, d.to);
    ui.label(egui::RichText::new(tl!("Recognize Text")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let group = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
        ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(13.0)));
        egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
        ui.add_space(8.0);
    };
    group(ui, "Pages", &mut |ui| {
        widgets::radio_value(ui, &mut d.pages, OcrPages::All, tl!("All pages"));
        widgets::radio_value(ui, &mut d.pages, OcrPages::Current, tl!("Current page"));
        ui.horizontal(|ui| {
            widgets::radio_value(ui, &mut d.pages, OcrPages::Range, tl!("From"));
            let on = d.pages == OcrPages::Range;
            ui.add_enabled(on, egui::DragValue::new(&mut d.from).range(1..=pages));
            ui.label(tl!("to"));
            ui.add_enabled(on, egui::DragValue::new(&mut d.to).range(1..=pages));
        });
    });
    group(ui, "Settings", &mut |ui| settings_fields(ui, d));
    if !available {
        ui.label(
            egui::RichText::new(tl!("Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them."))
                .small()
                .color(t.text_muted),
        );
        ui.add_space(6.0);
    }
    ui.add_space(6.0);
    let (mut go, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(available, |ui| widgets::pill_button(ui, tl!("Recognize text"), true)).inner.clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (go, cancel)
}

/// The recognition settings both Recognize Text dialogs show: the document language, the output,
/// the resolution pages are read at, and whether pages that have text are read too.
pub(crate) fn settings_fields(ui: &mut egui::Ui, d: &mut OcrDraft) {
    egui::Grid::new("ocr-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        let label = ui.label(tl!("Document language"));
        let name = LANGUAGES.iter().find(|l| l.0 == d.language).map_or("English", |l| l.1);
        craft_ui::ComboBox::from_id_salt("ocr-language")
            .selected_text(tl!(name))
            .width(220.0)
            .show_ui(ui, |ui| {
                for (code, name) in LANGUAGES {
                    ui.selectable_value(&mut d.language, (*code).to_string(), tl!(*name));
                }
            })
            .response
            .labelled_by(label.id);
        ui.end_row();
        let label = ui.label(tl!("Output"));
        craft_ui::ComboBox::from_id_salt("ocr-output")
            .selected_text(tl!("Searchable Image (Exact)"))
            .width(220.0)
            .show_ui(ui, |ui| {
                let _ = ui
                    .selectable_label(true, tl!("Searchable Image (Exact)"))
                    .on_hover_text(tl!("Adds invisible text over each word; the page image is not changed"));
            })
            .response
            .labelled_by(label.id);
        ui.end_row();
        let label = ui.label(tl!("Downsample to"));
        craft_ui::ComboBox::from_id_salt("ocr-dpi")
            .selected_text(format!("{} dpi", d.dpi))
            .width(220.0)
            .show_ui(ui, |ui| {
                for v in [600, 300, 150, 72] {
                    ui.selectable_value(&mut d.dpi, v, format!("{v} dpi"));
                }
            })
            .response
            .labelled_by(label.id);
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.add(crate::widgets::checkbox(&mut d.text_pages, tl!("Also recognize pages that already have text")))
        .on_hover_text(tl!("Pictures on those pages are recognized too; words already in the page's text are not added again"));
}

impl PdfCraftApp {
    /// Recognize text on the pages chosen in the dialog, in the background.
    pub fn start_ocr(&mut self) {
        let Some((vi, id)) = self.active_ids() else { return };
        if self.ocr_run.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        if let Some(why) = self.session.get(id).and_then(|d| d.read_only_reason.clone()) {
            self.notify(why);
            return;
        }
        let d = &self.ocr_draft;
        let pages: Vec<usize> = match d.pages {
            OcrPages::All => Vec::new(),
            OcrPages::Current => vec![self.views[vi].current],
            OcrPages::Range => (d.from.saturating_sub(1)..d.to).collect(),
        };
        let settings = d.settings();
        self.start_ocr_pages(id, &pages, settings, false);
    }

    /// Edit on a scanned page: like Acrobat's "Converting scanned page to Editable Text and
    /// Images", the page's words are recognized first, so that they open for editing (Edit text &
    /// images groups the recognised words into phrases). Runs once per page, for the page in view
    /// while Edit is on (its tab, or the Edit text & images or Add text tool). The page must be a
    /// picture (an image covering at least half of it) with no text of its own: comments and text
    /// boxes over it do not count as text, but real page text does, and such a page (a flyer with a
    /// background picture) is left alone, and is judged again only when the document changes. It
    /// is one undoable step, "Recognize text", and is not run again for that page after Undo.
    pub(crate) fn auto_ocr_scanned_page(&mut self) {
        let Some(scan) = self.scan_to_read() else { return };
        if !pdfcraft_engine::ocr::available() {
            self.views[scan.view].auto_ocr_skipped.insert(scan.page, scan.generation);
            return;
        }
        self.views[scan.view].auto_ocr_tried.insert(scan.page);
        // Only a page with no text of its own is read. Comments and text boxes over the picture do
        // not count as its text (a scan with a few comments is still a scan, as in Acrobat), but
        // a page with real text and a background picture is left alone.
        let mut settings = self.ocr_draft.settings();
        settings.skip_text_pages = true;
        settings.native_text_only = true;
        self.start_ocr_pages(scan.doc, &[scan.page], settings, true);
    }

    /// The page Edit should read now, if there is one: Edit is on outside Read mode with no dialog
    /// open, nothing else is being recognized, the document may be changed, the page in view has
    /// not been read before (nor been found no scan since the document last changed), and an image
    /// covers at least half of it. Needs no OCR models, so the rules can be tested without them;
    /// the page's own text is judged by the run itself.
    fn scan_to_read(&mut self) -> Option<ScanToRead> {
        // Edit is on when its panel is open (the Edit tab) or Edit text & images or Add text is picked,
        // the same test the page view uses for editing content.
        let editing = (self.left_open && self.left == crate::LeftPanel::Tool("edit"))
            || matches!(self.quick_tool, crate::QuickTool::EditText | crate::QuickTool::AddText);
        // Not in Read mode (which leaves the Edit tool picked but is not editing) and not beneath a
        // dialog or prompt: nothing may change the document under those. It is tried again once
        // they are gone.
        if !editing || self.mode == crate::Mode::Read || self.modal_open() || self.ocr_run.is_some() || self.ocr_batch.is_some() {
            return None;
        }
        let (view, doc) = self.active_ids()?;
        let page = self.views[view].current;
        if self.views[view].auto_ocr_tried.contains(&page) {
            return None;
        }
        let document = self.session.get(doc)?;
        if document.read_only_reason.is_some() || !document.allows_modification() {
            return None;
        }
        // A page found not to be a scan is judged again only once the document has changed, not
        // every frame (listing its images parses the page).
        let generation = document.edit_generation();
        if self.views[view].auto_ocr_skipped.get(&page) == Some(&generation) {
            return None;
        }
        let info = document.info.pages.get(page)?;
        let page_area = f64::from(info.width) * f64::from(info.height);
        let covered = document.page_images(page).iter().any(|i| {
            let [x0, y0, x1, y1] = i.visible_rect.unwrap_or(i.rect);
            page_area > 0.0 && ((x1 - x0) * (y1 - y0)).abs() >= 0.5 * page_area
        });
        if !covered {
            self.views[view].auto_ocr_skipped.insert(page, generation);
            return None;
        }
        Some(ScanToRead { view, doc, page, generation })
    }

    /// Recognize `pages` (0-based; empty = all) of document `id` in the background.
    fn start_ocr_pages(&mut self, id: DocId, pages: &[usize], settings: OcrSettings, automatic: bool) {
        // What an automatic run's result must still fit when it arrives (see `AutoOcr`).
        let auto = if automatic {
            pages.first().map(|&page| AutoOcr { page, generation: self.session.get(id).map_or(0, |d| d.edit_generation()) })
        } else {
            None
        };
        let Some(job) = self.session.ocr_job(id, pages, settings) else { return };
        let progress = Arc::new(Mutex::new(OcrProgress { total: job.pages.len(), ..Default::default() }));
        let p = progress.clone();
        // A failure is written on the worker thread: in the UI's language.
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            let result = recognize_guarded(|| {
                pdfcraft_engine::ocr::engine().map(|ocr| {
                    job.run(&ocr, |done, total| {
                        let Ok(mut s) = p.lock() else { return false };
                        s.done = done;
                        s.total = total;
                        !s.cancel
                    })
                })
            });
            if let Ok(mut s) = p.lock() {
                s.result = Some(result);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else {
            unstarted(&progress, std::thread::Builder::new().name("pdfcraft-ocr".into()).spawn(work), |s, e| s.result = Some(Err(e)));
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.ocr_run = Some(OcrRun { doc: id, progress, automatic: auto });
        self.poll_ocr();
    }

    /// Recognize text in each file, writing the searchable copies into a folder the user picks
    /// (the export folder override in tests), under the same names.
    pub fn ocr_files(&mut self, files: Vec<(String, Vec<u8>)>) {
        if self.ocr_batch.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        // The settings showing now, not when the folder arrives.
        let settings = self.ocr_draft.settings();
        #[cfg(not(target_arch = "wasm32"))]
        match self.export_dir_override.clone() {
            Some(d) => self.ocr_files_into(files, settings, d.into()),
            None => {
                let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Choose a folder for the searchable files").to_string());
                self.ask_one(crate::pickers::Ask::Folder(dialog), None, move |app, dir| app.ocr_files_into(files, settings, dir));
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.ocr_files_into(files, settings);
    }

    /// [`Self::ocr_files`] once the folder for the searchable files is known.
    fn ocr_files_into(&mut self, files: Vec<(String, Vec<u8>)>, settings: OcrSettings, #[cfg(not(target_arch = "wasm32"))] dir: std::path::PathBuf) {
        // Another batch may have started while the folder picker was open.
        if self.ocr_batch.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        let progress = Arc::new(Mutex::new(BatchProgress { total: files.len(), ..Default::default() }));
        let p = progress.clone();
        // The summary is written on the worker thread: draw it in the UI's language.
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            // A panic in the recognizer still ends the batch with a summary (#3919).
            #[cfg(not(target_arch = "wasm32"))]
            let summary = pdfcraft_engine::guard(|| recognize_files(files, &settings, &p, &dir));
            #[cfg(target_arch = "wasm32")]
            let summary = pdfcraft_engine::guard(|| recognize_files(files, &settings, &p));
            let summary = summary.unwrap_or_else(|panic| failed(&panic));
            if let Ok(mut s) = p.lock() {
                s.done = s.total;
                s.message = Some(summary);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else {
            unstarted(&progress, std::thread::Builder::new().name("pdfcraft-ocr-files".into()).spawn(work), |s, e| s.message = Some(e));
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.ocr_batch = Some(progress);
        self.poll_ocr();
    }
}

/// Recognize each file and write its searchable copy into `dir`; returns the summary to show.
fn recognize_files(
    files: Vec<(String, Vec<u8>)>,
    settings: &OcrSettings,
    p: &Mutex<BatchProgress>,
    #[cfg(not(target_arch = "wasm32"))] dir: &std::path::Path,
) -> String {
    let (mut ok, mut words, mut failed) = (0, 0, Vec::new());
    match pdfcraft_engine::ocr::engine() {
        Err(e) => failed.push(e),
        Ok(ocr) => {
            for (i, (name, bytes)) in files.into_iter().enumerate() {
                if let Ok(mut s) = p.lock() {
                    s.done = i;
                }
                if p.lock().is_ok_and(|s| s.cancel) {
                    break;
                }
                // A file stopped partway is not written: it would look recognized and isn't.
                let mut stopped = false;
                let r = pdfcraft_engine::ocr::recognize_file(&name, Arc::new(bytes), None, settings.clone(), &ocr, |page, pages| {
                    let go = BatchProgress::page(p, page, pages);
                    stopped |= !go;
                    go
                });
                if stopped {
                    break;
                }
                BatchProgress::file_done(p);
                let saved = r.and_then(|r| {
                    // A name the folder already has is kept; the new file gets a number (#1795).
                    #[cfg(not(target_arch = "wasm32"))]
                    pdfcraft_engine::batch::publish(&dir.join(file_name(&name)), &r.bytes)?;
                    #[cfg(target_arch = "wasm32")]
                    crate::editing::download(&name, &r.bytes)?;
                    Ok(r.words())
                });
                match saved {
                    Ok(n) => {
                        ok += 1;
                        words += n;
                    }
                    Err(e) => failed.push(format!("{name}: {}", e)),
                }
            }
        }
    }
    let done = if ok == 1 {
        crate::i18n::fmt(tl!("Recognized {w} words in 1 file"), &[("w", &words.to_string())])
    } else {
        crate::i18n::fmt(tl!("Recognized {w} words in {n} files"), &[("w", &words.to_string()), ("n", &ok.to_string())])
    };
    let mut msg = done;
    if !failed.is_empty() {
        msg.push_str(&crate::i18n::fmt(tl!("; failed: {list}"), &[("list", &failed.join("; "))]));
    }
    if p.lock().is_ok_and(|s| s.cancel) {
        msg = crate::i18n::fmt(tl!("Text recognition cancelled. {summary}"), &[("summary", &msg)]);
    }
    msg
}

impl PdfCraftApp {
    /// Stop a running recognition: in a document the pages read so far are kept; in files and
    /// folders the file being read is not written, and the ones finished stay.
    pub fn cancel_ocr(&mut self) {
        if let Some(r) = &self.ocr_run
            && let Ok(mut s) = r.progress.lock()
        {
            s.cancel = true;
        }
        if let Some(b) = &self.ocr_batch
            && let Ok(mut s) = b.lock()
        {
            s.cancel = true;
        }
    }

    /// The UI clock, for the remaining-time estimate.
    fn now(&self) -> f64 {
        self.ctx.as_ref().map_or(0.0, |ctx| ctx.input(|i| i.time))
    }

    /// Show `label` in the progress card, with Cancel, unless a cancel is under way.
    fn ocr_notice(&mut self, label: String, detail: Option<String>, fraction: f32, cancelling: bool) {
        let label = if cancelling { tl!("Cancelling…").to_string() } else { label };
        self.progress_notice = Some(crate::widgets::ProgressNotice {
            label,
            detail: if cancelling { None } else { detail },
            fraction,
            cancellable: !cancelling,
            job: crate::widgets::ProgressJob::Ocr,
        });
    }

    /// Take the progress card down if it is this job's.
    fn clear_ocr_notice(&mut self) {
        if self.progress_notice.as_ref().is_some_and(|p| p.job == crate::widgets::ProgressJob::Ocr) {
            self.progress_notice = None;
        }
    }

    /// Show progress; apply the result once the worker is done.
    pub(crate) fn poll_ocr(&mut self) {
        if let Some(b) = self.ocr_batch.clone() {
            let now = self.now();
            let state = b
                .lock()
                .map(|mut s| match s.message.take() {
                    Some(m) => Ok(m),
                    None => {
                        let (read, left) = s.pages_read_and_left();
                        let eta = if s.pages > 0 { s.eta.update(now, read, left) } else { None };
                        Err((s.done, s.total, s.page, s.pages, s.cancel, eta))
                    }
                })
                .map_err(|e| e.to_string());
            match state {
                Ok(Ok(m)) => {
                    self.ocr_batch = None;
                    self.clear_ocr_notice();
                    self.notify(m);
                }
                Ok(Err((done, total, page, pages, cancelling, eta))) => {
                    let total = total.max(1);
                    let label = crate::i18n::fmt(
                        tl!("Recognizing text… file {d} of {t}"),
                        &[("d", &(done + 1).min(total).to_string()), ("t", &total.to_string())],
                    );
                    let mut detail = Vec::new();
                    if pages > 0 {
                        detail
                            .push(crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(page + 1).min(pages).to_string()), ("n", &pages.to_string())]));
                    }
                    detail.extend(eta.map(eta_text));
                    let within = if pages > 0 { page as f32 / pages as f32 } else { 0.0 };
                    let fraction = (done as f32 + within) / total as f32;
                    self.ocr_notice(label, (!detail.is_empty()).then(|| detail.join(" · ")), fraction, cancelling);
                    if let Some(ctx) = &self.ctx {
                        ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    }
                }
                // The worker stopped while holding the lock: it will never report back (#3919).
                Err(e) => {
                    self.ocr_batch = None;
                    self.clear_ocr_notice();
                    self.notify(failed(&e));
                }
            }
        }
        let now = self.now();
        let Some(run) = self.ocr_run.as_ref() else { return };
        let (doc, progress, auto) = (run.doc, run.progress.clone(), run.automatic);
        let automatic = auto.is_some();
        let mut s = match progress.lock() {
            Ok(s) => s,
            Err(e) => {
                // As above: without this the run never clears and refuses every later one.
                let e = e.to_string();
                self.ocr_run = None;
                self.clear_ocr_notice();
                self.notify(failed(&e));
                return;
            }
        };
        // The result of a run Edit started waits while Read mode or a dialog is up: nothing may change
        // the document beneath them, and the run may have been started before they appeared. A run
        // the user asked for completes as before.
        if automatic && s.result.is_some() && (self.mode == crate::Mode::Read || self.modal_open()) {
            drop(s);
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        }
        let Some(result) = s.result.take() else {
            let total = s.total.max(1);
            let label =
                crate::i18n::fmt(tl!("Recognizing text… page {d} of {t}"), &[("d", &(s.done + 1).min(total).to_string()), ("t", &total.to_string())]);
            let (done, cancelling) = (s.done, s.cancel);
            let eta = s.eta.update(now, done as f64, total.saturating_sub(done) as f64);
            drop(s);
            // A run Edit started shows nothing.
            if !automatic {
                self.ocr_notice(label, eta.map(eta_text), done as f32 / total as f32, cancelling);
            }
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        };
        drop(s);
        self.ocr_run = None;
        if !automatic {
            self.clear_ocr_notice();
        }
        // The result is for the bytes the run started from. If the document has changed since (a
        // page deleted or moved, say) its page index may now name another page: drop it, and let
        // the page in view be judged again.
        if let Some(a) = auto
            && self.session.get(doc).map(|d| d.edit_generation()) != Some(a.generation)
        {
            if let Some(view) = self.views.iter_mut().find(|v| v.id == doc) {
                view.auto_ocr_tried.remove(&a.page);
            }
            return;
        }
        let found = match result {
            Ok(f) => f,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        let read = found.iter().filter(|p| p.skipped.is_none()).count();
        let skipped = found.len() - read;
        match self.session.apply_ocr(doc, &found) {
            Ok(words) => {
                if let Some(info) = self.session.get(doc).map(|d| d.info.clone())
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == doc)
                {
                    view.document_changed(&info);
                }
                // An automatic run that found nothing (a page that has text of its own) says nothing.
                if automatic && words == 0 {
                    return;
                }
                let words_part = if words == 1 {
                    crate::i18n::fmt(tl!("Recognized 1 word"), &[])
                } else {
                    crate::i18n::fmt(tl!("Recognized {n} words"), &[("n", &words.to_string())])
                };
                let pages_part = if read == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &read.to_string())]) };
                let mut msg = crate::i18n::fmt(tl!("{words} on {pages}"), &[("words", &words_part), ("pages", &pages_part)]);
                if skipped > 0 {
                    msg.push_str(&if skipped == 1 {
                        crate::i18n::fmt(tl!("; 1 page already had text"), &[])
                    } else {
                        crate::i18n::fmt(tl!("; {n} pages already had text"), &[("n", &skipped.to_string())])
                    });
                }
                self.notify(msg);
            }
            Err(e) => self.notify_error(e),
        }
    }
}

/// "Recognize text failed: {e}", in the current language.
/// No worker thread: end the run with the failure, written into its progress by `report`,
/// instead of waiting for a result that never comes and refusing every later run (#1819).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn unstarted<P, T>(p: &Mutex<P>, spawned: std::io::Result<T>, report: impl FnOnce(&mut P, String)) {
    if let Err(e) = spawned
        && let Ok(mut s) = p.lock()
    {
        report(&mut s, failed(&e.to_string()));
    }
}

/// The last part of a file's name, so a name with folders in it still lands in the chosen folder.
#[cfg(not(target_arch = "wasm32"))]
fn file_name(name: &str) -> &std::ffi::OsStr {
    std::path::Path::new(name).file_name().unwrap_or(std::ffi::OsStr::new("document.pdf"))
}

fn failed(e: &str) -> String {
    crate::i18n::fmt(tl!("{label} failed: {e}"), &[("label", tl!("Recognize text")), ("e", e)])
}

/// `recognize`, with a panic in it (a damaged model, a page the recognizer can't take) turned into
/// the run's error: the run has to end either way, or it refuses every later one (#3919).
fn recognize_guarded<T>(recognize: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    pdfcraft_engine::guard(recognize).unwrap_or_else(|panic| Err(failed(&panic)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dialog, LeftPanel, Mode, QuickTool};
    use pdfcraft_engine::{Edit, Session};

    /// A one-page PDF that is one picture covering the page.
    fn scan() -> Vec<u8> {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([250, 250, 250, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        Session::new().create_from_images(&[("scan.png".into(), png.into_inner())]).unwrap().to_vec()
    }

    fn app_with(pdf: Vec<u8>) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.open_bytes("doc.pdf", None, pdf).unwrap();
        app.quick_tool = QuickTool::EditText;
        app
    }

    /// #1819: a recognition whose worker can't start ends with the reason, so the next one isn't
    /// refused as already running.
    #[test]
    fn a_recognition_whose_worker_cannot_start_ends() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let mut app = app_with(scan());
        let doc = app.active_ids().unwrap().1;
        let progress = Arc::new(Mutex::new(OcrProgress::default()));
        unstarted(&progress, Ok(()), |s: &mut OcrProgress, e| s.result = Some(Err(e)));
        assert!(progress.lock().unwrap().result.is_none());
        unstarted::<_, ()>(&progress, Err(std::io::Error::other("no threads left")), |s, e| s.result = Some(Err(e)));
        app.ocr_run = Some(OcrRun { doc, progress, automatic: None });
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "the run ended");
        assert_eq!(app.toast.clone().unwrap().0, "Recognize text failed: no threads left");
        let batch = Arc::new(Mutex::new(BatchProgress::default()));
        unstarted::<_, ()>(&batch, Err(std::io::Error::other("no threads left")), |s, e| s.message = Some(e));
        app.ocr_batch = Some(batch);
        app.poll_ocr();
        assert!(app.ocr_batch.is_none(), "the batch ended");
    }

    /// #1795: a searchable copy never replaces a file the folder already has, nor leaves it.
    #[test]
    fn recognized_files_keep_the_files_already_in_the_folder() {
        if !pdfcraft_engine::ocr::available() {
            eprintln!("skipped: OCR models not installed");
            return;
        }
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("scan.pdf"), b"keep me").unwrap();
        let p = Mutex::new(BatchProgress::default());
        let summary = recognize_files(vec![("scan.pdf".into(), scan()), ("../up.pdf".into(), scan())], &OcrSettings::default(), &p, dir.path());
        assert!(summary.ends_with("in 2 files"), "{summary}");
        assert_eq!(std::fs::read(dir.path().join("scan.pdf")).unwrap(), b"keep me");
        assert!(dir.path().join("scan (2).pdf").exists() && dir.path().join("up.pdf").exists());
    }

    fn found(app: &mut PdfCraftApp) -> Option<usize> {
        app.scan_to_read().map(|s| s.page)
    }

    /// These rules need no OCR models (the run itself judges the page's own text), so they are
    /// tested everywhere, unlike the tests that recognize something.
    #[test]
    fn edit_reads_a_page_that_is_a_picture_and_no_other() {
        assert_eq!(found(&mut app_with(scan())), Some(0));
        let text = Session::new().create_from_text("t", "Real text, no picture").unwrap().to_vec();
        assert_eq!(found(&mut app_with(text)), None, "a page of text is no scan");
        let blank = Session::new().create_blank(595.0, 842.0, 1).unwrap().to_vec();
        assert_eq!(found(&mut app_with(blank)), None, "nor is a blank page");
    }

    #[test]
    fn only_edit_being_on_starts_it() {
        let mut app = app_with(scan());
        app.quick_tool = QuickTool::Select;
        assert_eq!(found(&mut app), None, "another tool");
        // The Edit tab's panel counts, as for the page view, with no tool picked.
        app.left = LeftPanel::Tool("edit");
        app.left_open = true;
        assert_eq!(found(&mut app), Some(0), "the Edit panel");
        app.left_open = false;
        assert_eq!(found(&mut app), None, "the Edit panel closed");
    }

    #[test]
    fn it_waits_for_read_mode_and_dialogs_to_end() {
        let mut app = app_with(scan());
        app.mode = Mode::Read;
        assert_eq!(found(&mut app), None, "Read mode");
        app.mode = Mode::Edit;
        app.dialog = Some(Dialog::JsConsole);
        assert_eq!(found(&mut app), None, "beneath a dialog");
        app.dialog = None;
        assert_eq!(found(&mut app), Some(0), "tried again once they are gone");
    }

    #[test]
    fn a_page_is_read_once_and_a_page_found_no_scan_is_judged_again_after_a_change() {
        let mut app = app_with(scan());
        app.views[0].auto_ocr_tried.insert(0);
        assert_eq!(found(&mut app), None, "a page already read is not read again (Undo sticks)");

        // A blank page is no scan; it is judged again only once the document changes.
        let blank = Session::new().create_blank(595.0, 842.0, 1).unwrap().to_vec();
        let mut app = app_with(blank);
        assert_eq!(found(&mut app), None);
        assert!(app.views[0].auto_ocr_skipped.contains_key(&0), "remembered with the document's generation");
        let id = app.active_ids().unwrap().1;
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([250, 250, 250, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let add = Edit::AddImage { page: 0, rect: Some([0.0, 0.0, 595.0, 842.0]), name: "scan.png".into(), bytes: Arc::new(png.into_inner()) };
        app.session.apply(id, add).unwrap();
        assert_eq!(found(&mut app), Some(0), "a picture over the page makes it a scan");
    }
    /// A finished run Edit started is applied only once Read mode and dialogs are gone; one the user
    /// asked for is applied at once. The result is made up, so no OCR models are needed.
    #[test]
    fn the_result_of_an_automatic_run_waits_for_read_mode_and_dialogs() {
        let word = || pdfcraft_engine::ocr::PlacedWord { text: "scanned".into(), origin: [72.0, 700.0], across: [60.0, 0.0], up: [0.0, 12.0] };
        let finished = |app: &PdfCraftApp, automatic: bool| {
            let generation = app.session.get(app.active_ids().unwrap().1).unwrap().edit_generation();
            let page = pdfcraft_engine::ocr::OcrPage { page: 0, words: vec![word()], already_on_page: 0, skipped: None };
            let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), ..Default::default() }));
            OcrRun { doc: app.active_ids().unwrap().1, progress, automatic: automatic.then_some(AutoOcr { page: 0, generation }) }
        };
        let label = |app: &PdfCraftApp| app.session.get(app.active_ids().unwrap().1).unwrap().can_undo().map(String::from);

        let mut app = app_with(scan());
        app.ocr_run = Some(finished(&app, true));
        app.mode = Mode::Read;
        app.poll_ocr();
        assert!(app.ocr_run.is_some() && label(&app).is_none(), "waits in Read mode");
        app.mode = Mode::Edit;
        app.dialog = Some(Dialog::JsConsole);
        app.poll_ocr();
        assert!(app.ocr_run.is_some() && label(&app).is_none(), "waits beneath a dialog");
        app.dialog = None;
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "taken once they are gone");
        assert_eq!(label(&app).as_deref(), Some("Recognize text"));

        // A run the user asked for is not held back.
        let mut app = app_with(scan());
        app.ocr_run = Some(finished(&app, false));
        app.mode = Mode::Read;
        app.poll_ocr();
        assert_eq!(label(&app).as_deref(), Some("Recognize text"));
    }
    /// A run's result is for the document as it was when the run started. Once the document has
    /// changed, a result of Edit's is dropped, not applied to whatever page now has its index, and
    /// the page may be judged again.
    #[test]
    fn the_result_of_an_automatic_run_is_dropped_once_the_document_has_changed() {
        let mut app = app_with(scan());
        let id = app.active_ids().unwrap().1;
        let generation = app.session.get(id).unwrap().edit_generation();
        let page = pdfcraft_engine::ocr::OcrPage {
            page: 0,
            words: vec![pdfcraft_engine::ocr::PlacedWord { text: "scanned".into(), origin: [72.0, 700.0], across: [60.0, 0.0], up: [0.0, 12.0] }],
            already_on_page: 0,
            skipped: None,
        };
        let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), ..Default::default() }));
        app.ocr_run = Some(OcrRun { doc: id, progress, automatic: Some(AutoOcr { page: 0, generation }) });
        app.views[0].auto_ocr_tried.insert(0);
        // The document changes while the run is out.
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([250, 250, 250, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        app.session
            .apply(id, Edit::AddImage { page: 0, rect: Some([10.0, 10.0, 50.0, 50.0]), name: "p.png".into(), bytes: Arc::new(png.into_inner()) })
            .unwrap();
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "the run is over");
        assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Add image"), "its result was not applied");
        assert!(!app.views[0].auto_ocr_tried.contains(&0), "the page may be judged again");
    }

    #[test]
    fn a_panicking_recognizer_ends_the_run_with_an_error() {
        let error = recognize_guarded(|| -> Result<(), String> { panic!("the model file is damaged") }).unwrap_err();
        assert!(error.starts_with("Recognize text failed") && error.contains("the model file is damaged"), "{error}");
        assert_eq!(recognize_guarded(|| Ok::<_, String>(7)), Ok(7));
    }

    /// Poison `m` the way a worker that panics while it holds the lock does.
    fn poison<T: Send + 'static>(m: &Arc<Mutex<T>>) {
        let worker = m.clone();
        let _ = std::thread::spawn(move || {
            let _held = worker.lock();
            panic!("the worker died");
        })
        .join();
        assert!(m.is_poisoned());
    }

    #[test]
    fn a_run_whose_worker_died_ends_instead_of_waiting_forever() {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let progress = Arc::new(Mutex::new(OcrProgress { total: 3, ..Default::default() }));
        poison(&progress);
        app.ocr_run = Some(OcrRun { doc: DocId(1), progress, automatic: None });
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "the next run isn't refused as already running");
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert!(toast.starts_with("Recognize text failed"), "{toast}");

        let batch = Arc::new(Mutex::new(BatchProgress { total: 2, ..Default::default() }));
        poison(&batch);
        app.ocr_batch = Some(batch);
        app.poll_ocr();
        assert!(app.ocr_batch.is_none());
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert!(toast.starts_with("Recognize text failed"), "{toast}");
    }

    #[test]
    fn the_estimate_waits_for_a_page_and_three_seconds_then_follows_the_pace() {
        let mut eta = Eta::default();
        assert_eq!(eta.update(10.0, 0.0, 10.0), None, "the first look only starts the clock");
        assert_eq!(eta.update(12.0, 1.0, 9.0), None, "two seconds is too early to say");
        assert_eq!(eta.update(14.0, 1.0, 9.0), Some(36.0), "4 s for 1 page, 9 to go");
        assert_eq!(eta.update(20.0, 5.0, 5.0), Some(10.0), "10 s for 5 pages, 5 to go");
        assert_eq!(eta.update(30.0, 5.0, f64::NAN), None, "no nonsense from a bad count");
    }

    #[test]
    fn the_estimate_reads_in_minutes_and_hours() {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        assert_eq!(eta_text(20.0), "less than a minute left");
        assert_eq!(eta_text(61.0), "about 2 min left");
        assert_eq!(eta_text(119.0 * 60.0), "about 119 min left");
        assert_eq!(eta_text(125.0 * 60.0), "about 2 h 5 min left");
        assert_eq!(eta_text(f64::INFINITY), "less than a minute left");
        assert!(eta_text(1e30).ends_with("min left"), "a huge estimate doesn't wrap or panic");
    }

    #[test]
    fn files_still_to_come_are_estimated_at_the_average_so_far() {
        // File 3 of 5 at page 2 of 10, after two files of 4 and 6 pages.
        let p = BatchProgress { done: 2, total: 5, page: 2, pages: 10, pages_done: 10, ..Default::default() };
        let (read, left) = p.pages_read_and_left();
        assert_eq!(read, 12.0);
        // 8 more in this file, and two files at (10 + 10) / 3 pages.
        assert!((left - (8.0 + 2.0 * 20.0 / 3.0)).abs() < 1e-9, "{left}");
    }

    /// The progress card of a batch: file, page and Cancel; Cancel asks the worker to stop and
    /// the card says so until it has.
    #[test]
    fn a_batch_shows_its_file_and_page_and_cancel_stops_it() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let batch = Arc::new(Mutex::new(BatchProgress { done: 1, total: 4, page: 2, pages: 10, pages_done: 7, ..Default::default() }));
        let worker = batch.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.ocr_batch = Some(batch);
            app
        });
        h.run_steps(4);
        h.get_by_label("Recognizing text… file 2 of 4");
        h.get_by_label_contains("Page 3 of 10");
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert!(worker.lock().unwrap().cancel, "the worker is told to stop");
        h.get_by_label("Cancelling…");
        assert!(h.query_by_label("Cancel").is_none(), "no second Cancel while it stops");
        // The worker finishes with its summary: the card goes and the summary shows.
        worker.lock().unwrap().message = Some("Text recognition cancelled. Recognized 9 words in 1 file".into());
        h.run_steps(3);
        assert!(h.state().ocr_batch.is_none() && h.state().progress_notice.is_none());
        assert!(h.state().toast.as_ref().is_some_and(|t| t.0.starts_with("Text recognition cancelled.")));
    }
}
