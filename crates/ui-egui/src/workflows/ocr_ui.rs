//! Scan & OCR ▸ Recognize text: the Recognize Text dialog (pages, language, output, resolution)
//! and the background run with its progress. Recognition happens on a worker thread; the result
//! is applied as one undoable edit when it is done. Editable text and images is ClearScan: the
//! same worker pattern runs [`pdfcraft_engine::Session::clearscan_job`], then
//! [`pdfcraft_engine::Session::apply_clearscan`] (what [`pdfcraft_engine::Session::clearscan_document`]
//! does in one call).

use std::sync::{Arc, Mutex};

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::clearscan::{ClearScanBuilt, ClearScanError, ClearScanSettings};
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

/// What Recognize Text writes. Searchable Image (Exact) is the invisible text layer.
/// Editable text and images is ClearScan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OcrOutput {
    #[default]
    SearchableImage,
    EditableText,
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
    /// Output style. The draft holds it; there is no separate command.
    pub output: OcrOutput,
    /// ClearScan binarizer, 0–100. Shown only for [`OcrOutput::EditableText`].
    pub sensitivity: u8,
    /// ClearScan integer shrink after rendering, 1–8. Shown only for [`OcrOutput::EditableText`].
    pub downsample: u8,
}

impl Default for OcrDraft {
    fn default() -> Self {
        OcrDraft {
            pages: OcrPages::All,
            from: 1,
            to: 1,
            language: "en".into(),
            dpi: 300,
            text_pages: false,
            output: OcrOutput::SearchableImage,
            sensitivity: 50,
            downsample: 1,
        }
    }
}

impl OcrDraft {
    /// The recognition settings these choices make.
    pub fn settings(&self) -> OcrSettings {
        OcrSettings { dpi: self.dpi as f32, language: self.language.clone(), skip_text_pages: !self.text_pages, ..Default::default() }
    }

    /// ClearScan settings from the same pages, language, dpi and "already has text" choice,
    /// plus sensitivity and downsample.
    pub fn clearscan_settings(&self) -> ClearScanSettings {
        ClearScanSettings {
            dpi: self.dpi as f32,
            language: self.language.clone(),
            sensitivity: self.sensitivity.min(100),
            downsample: self.downsample.clamp(1, 8),
            skip_text_pages: !self.text_pages,
        }
    }
}

/// Progress of a run: pages done, total, the result once finished, and a cancel request.
#[derive(Default)]
pub struct OcrProgress {
    pub done: usize,
    pub total: usize,
    pub result: Option<Result<Vec<OcrPage>, String>>,
    pub cancel: bool,
}

/// Recognize text in multiple files: files done, total, and the summary once finished.
#[derive(Default)]
pub struct BatchProgress {
    pub done: usize,
    pub total: usize,
    pub message: Option<String>,
}

/// A page Edit should read now (see [`PdfCraftApp::scan_to_read`]).
struct ScanToRead {
    view: usize,
    doc: DocId,
    page: usize,
    /// The document's edit generation when it was judged.
    generation: u64,
}

/// Progress of a ClearScan run: the stage the pipeline last reported, the result once finished,
/// and a cancel request. Cancel stops before the next page; pages already built are still applied.
pub struct ClearScanRunProgress {
    pub done: usize,
    pub total: usize,
    pub stage: &'static str,
    pub(crate) result: Option<Result<ClearScanBuilt, ClearScanFail>>,
    pub cancel: bool,
}

impl Default for ClearScanRunProgress {
    fn default() -> Self {
        ClearScanRunProgress { done: 0, total: 0, stage: "render", result: None, cancel: false }
    }
}

/// Why a ClearScan run stopped before it could be applied.
pub(crate) enum ClearScanFail {
    /// The dialog's own missing-models message. Nothing is edited.
    NoModels,
    Message(String),
}

pub struct ClearScanRun {
    pub doc: DocId,
    pub progress: Arc<Mutex<ClearScanRunProgress>>,
    /// The document's edit generation when the run started. A result is for those bytes only.
    pub generation: u64,
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
    d.sensitivity = d.sensitivity.min(100);
    d.downsample = d.downsample.clamp(1, 8);
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
    group(ui, "Settings", &mut |ui| {
        egui::Grid::new("ocr-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tl!("Document language"));
            let name = LANGUAGES.iter().find(|l| l.0 == d.language).map_or("English", |l| l.1);
            craft_ui::ComboBox::from_id_salt("ocr-language").selected_text(tl!(name)).width(220.0).show_ui(ui, |ui| {
                for (code, name) in LANGUAGES {
                    ui.selectable_value(&mut d.language, (*code).to_string(), tl!(*name));
                }
            });
            ui.end_row();
            ui.label(tl!("Output"));
            let output = craft_ui::ComboBox::from_id_salt("ocr-output").selected_text(output_label(d.output)).width(220.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut d.output, OcrOutput::SearchableImage, tl!("Searchable Image (Exact)"))
                    .on_hover_text(tl!("Adds invisible text over each word; the page image is not changed"));
                ui.selectable_value(&mut d.output, OcrOutput::EditableText, tl!("Editable text and images"))
                    .on_hover_text(tl!("Replaces the lettering with outlines. Photographs and rules stay, and the page image changes."));
            });
            output.response.on_hover_text(output_hover(d.output));
            ui.end_row();
            if d.output == OcrOutput::EditableText {
                ui.label(tl!("Sensitivity"));
                ui.add(egui::DragValue::new(&mut d.sensitivity).range(0..=100));
                ui.end_row();
                ui.label(tl!("Downsample"));
                ui.add(egui::DragValue::new(&mut d.downsample).range(1..=8));
                ui.end_row();
            }
            ui.label(tl!("Downsample to"));
            craft_ui::ComboBox::from_id_salt("ocr-dpi").selected_text(format!("{} dpi", d.dpi)).width(220.0).show_ui(ui, |ui| {
                for v in [600, 300, 150, 72] {
                    ui.selectable_value(&mut d.dpi, v, format!("{v} dpi"));
                }
            });
            ui.end_row();
        });
        ui.add_space(4.0);
        ui.add(crate::widgets::checkbox(&mut d.text_pages, tl!("Also recognize pages that already have text")))
            .on_hover_text(tl!("Pictures on those pages are recognized too; words already in the page's text are not added again"));
    });
    if !available {
        ui.label(egui::RichText::new(models_missing()).small().color(t.text_muted));
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

impl PdfCraftApp {
    /// Recognize text on the pages chosen in the dialog, in the background.
    pub fn start_ocr(&mut self) {
        let Some((vi, id)) = self.active_ids() else { return };
        if self.ocr_run.is_some() || self.clearscan_run.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        if self.optimize_run.is_some() {
            self.notify_tr("Optimization is already running");
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
        if d.output == OcrOutput::EditableText {
            let settings = d.clearscan_settings();
            self.start_clearscan(id, &pages, settings);
            return;
        }
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
        if !editing
            || self.mode == crate::Mode::Read
            || self.modal_open()
            || self.ocr_run.is_some()
            || self.ocr_batch.is_some()
            || self.clearscan_run.is_some()
        {
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
            std::thread::Builder::new().name("pdfcraft-ocr".into()).spawn(work).ok();
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.ocr_run = Some(OcrRun { doc: id, progress, automatic: auto });
        self.poll_ocr();
    }

    /// Editable text and images: ClearScan on a worker, then one "ClearScan" undo step.
    /// This is [`pdfcraft_engine::Session::clearscan_document`] split so the window stays responsive.
    fn start_clearscan(&mut self, id: DocId, pages: &[usize], settings: ClearScanSettings) {
        if !pdfcraft_engine::ocr::available() {
            self.notify(models_missing());
            return;
        }
        let job = match self.session.clearscan_job(id, pages, settings) {
            Ok(job) => job,
            Err(error) => {
                self.notify_clearscan_error(error);
                return;
            }
        };
        let progress = Arc::new(Mutex::new(ClearScanRunProgress { total: job.pages.len(), ..Default::default() }));
        let p = progress.clone();
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            let result = clearscan_guarded(|| {
                job.run(|event| {
                    let Ok(mut state) = p.lock() else { return false };
                    state.done = event.done;
                    state.total = event.total;
                    state.stage = event.stage;
                    !state.cancel
                })
            });
            if let Ok(mut state) = p.lock() {
                state.result = Some(result);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else if let Err(error) = std::thread::Builder::new().name("pdfcraft-clearscan".into()).spawn(work)
            && let Ok(mut state) = progress.lock()
        {
            state.result = Some(Err(ClearScanFail::Message(error.to_string())));
        }
        #[cfg(target_arch = "wasm32")]
        work();
        let generation = self.session.get(id).map_or(0, |document| document.edit_generation());
        self.clearscan_run = Some(ClearScanRun { doc: id, progress, generation });
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
            std::thread::Builder::new().name("pdfcraft-ocr-files".into()).spawn(work).ok();
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
                let r = pdfcraft_engine::ocr::recognize_file(&name, Arc::new(bytes), None, settings.clone(), &ocr, |_, _| true);
                let saved = r.and_then(|r| {
                    #[cfg(not(target_arch = "wasm32"))]
                    crate::editing::write_atomically(&dir.join(&name).to_string_lossy(), &r.bytes).map_err(|e| e.to_string())?;
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
    msg
}

impl PdfCraftApp {
    /// Stop a running recognition (the pages read so far are kept).
    pub fn cancel_ocr(&mut self) {
        if let Some(r) = &self.ocr_run
            && let Ok(mut s) = r.progress.lock()
        {
            s.cancel = true;
        }
    }

    /// Stop ClearScan before the next page. Pages already built are still applied.
    pub fn cancel_clearscan(&mut self) {
        if let Some(run) = &self.clearscan_run
            && let Ok(mut state) = run.progress.lock()
        {
            state.cancel = true;
        }
    }

    /// Show progress; apply the result once the worker is done.
    pub(crate) fn poll_ocr(&mut self) {
        self.poll_clearscan();
        if let Some(b) = self.ocr_batch.clone() {
            let msg = b.lock().map(|mut s| s.message.take().ok_or((s.done, s.total))).map_err(|e| e.to_string());
            match msg {
                Ok(Ok(m)) => {
                    self.ocr_batch = None;
                    self.notify(m);
                }
                Ok(Err((done, total))) => {
                    let m = crate::i18n::fmt(
                        tl!("Recognizing text… file {d} of {t}"),
                        &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
                    );
                    if self.toast.as_ref().is_none_or(|t| t.0 != m) {
                        self.notify(m);
                    }
                    if let Some(ctx) = &self.ctx {
                        ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    }
                }
                // The worker stopped while holding the lock: it will never report back (#3919).
                Err(e) => {
                    self.ocr_batch = None;
                    self.notify(failed(&e));
                }
            }
        }
        let Some(run) = self.ocr_run.as_ref() else { return };
        let (doc, progress, auto) = (run.doc, run.progress.clone(), run.automatic);
        let automatic = auto.is_some();
        let mut s = match progress.lock() {
            Ok(s) => s,
            Err(e) => {
                // As above: without this the run never clears and refuses every later one.
                let e = e.to_string();
                self.ocr_run = None;
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
            let msg = crate::i18n::fmt(
                tl!("Recognizing text… page {d} of {t}"),
                &[("d", &(s.done + 1).min(s.total.max(1)).to_string()), ("t", &s.total.max(1).to_string())],
            );
            drop(s);
            if !automatic && self.toast.as_ref().is_none_or(|t| t.0 != msg) {
                self.notify(msg);
            }
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        };
        drop(s);
        self.ocr_run = None;
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

    /// Show the ClearScan stage; apply pages already built once the worker is done, including
    /// when the run was cancelled.
    fn poll_clearscan(&mut self) {
        let Some(run) = self.clearscan_run.as_ref() else { return };
        let (doc, progress, generation) = (run.doc, run.progress.clone(), run.generation);
        let mut state = match progress.lock() {
            Ok(state) => state,
            Err(error) => {
                self.clearscan_run = None;
                self.clear_progress(crate::widgets::ProgressJob::ClearScan);
                self.notify(failed_clearscan(&error.to_string()));
                return;
            }
        };
        let Some(result) = state.result.take() else {
            let cancelling = state.cancel;
            let (done, total, stage) = (state.done, state.total, state.stage);
            drop(state);
            let label = if cancelling { tl!("Cancelling…").to_string() } else { clearscan_progress_label(stage, done, total) };
            self.progress_job = crate::widgets::ProgressJob::ClearScan;
            self.progress_notice =
                Some(crate::widgets::ProgressNotice { label, fraction: clearscan_fraction(done, total, stage), cancellable: !cancelling });
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        };
        drop(state);
        self.clearscan_run = None;
        self.clear_progress(crate::widgets::ProgressJob::ClearScan);
        let built = match result {
            Ok(built) => built,
            Err(ClearScanFail::NoModels) => {
                self.notify(models_missing());
                return;
            }
            Err(ClearScanFail::Message(error)) => {
                self.notify(error);
                return;
            }
        };
        if self.session.get(doc).map(|document| document.edit_generation()) != Some(generation) {
            self.notify_tr("The document changed while ClearScan ran, so its result was not applied");
            return;
        }
        let report = built.report;
        match self.session.apply_clearscan(doc, built.built) {
            Ok(()) => {
                if let Some(info) = self.session.get(doc).map(|d| d.info.clone())
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == doc)
                {
                    view.document_changed(&info);
                }
                self.notify(clearscan_done_message(&report));
            }
            Err(error) => self.notify_clearscan_error(error),
        }
    }

    fn notify_clearscan_error(&mut self, error: ClearScanError) {
        match error {
            ClearScanError::NoModels => self.notify(models_missing()),
            other => self.notify_error(other),
        }
    }
}

/// "Recognize text failed: {e}", in the current language.
fn failed(e: &str) -> String {
    crate::i18n::fmt(tl!("{label} failed: {e}"), &[("label", tl!("Recognize text")), ("e", e)])
}

/// `recognize`, with a panic in it (a damaged model, a page the recognizer can't take) turned into
/// the run's error: the run has to end either way, or it refuses every later one (#3919).
fn recognize_guarded<T>(recognize: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    pdfcraft_engine::guard(recognize).unwrap_or_else(|panic| Err(failed(&panic)))
}

fn clearscan_guarded(run: impl FnOnce() -> Result<ClearScanBuilt, ClearScanError>) -> Result<ClearScanBuilt, ClearScanFail> {
    match pdfcraft_engine::guard(run) {
        Ok(Ok(built)) => Ok(built),
        Ok(Err(ClearScanError::NoModels)) => Err(ClearScanFail::NoModels),
        Ok(Err(error)) => Err(ClearScanFail::Message(error.to_string())),
        Err(panic) => Err(ClearScanFail::Message(failed_clearscan(&panic))),
    }
}

/// The message the Recognize Text dialog shows when the models are missing.
fn models_missing() -> String {
    tl!("Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them.")
        .to_string()
}

fn output_label(output: OcrOutput) -> &'static str {
    match output {
        OcrOutput::SearchableImage => tl!("Searchable Image (Exact)"),
        OcrOutput::EditableText => tl!("Editable text and images"),
    }
}

fn output_hover(output: OcrOutput) -> &'static str {
    match output {
        OcrOutput::SearchableImage => tl!("Adds invisible text over each word; the page image is not changed"),
        OcrOutput::EditableText => tl!("Replaces the lettering with outlines. Photographs and rules stay, and the page image changes."),
    }
}

fn clearscan_stage_name(stage: &str) -> String {
    match stage {
        "render" => tl!("Rendering"),
        "downsample" => tl!("Downsampling"),
        "preprocess" => tl!("Preparing the page"),
        "recognize" => tl!("Recognizing letters"),
        "components" => tl!("Finding strokes"),
        "align" => tl!("Straightening"),
        "cluster" => tl!("Grouping letters"),
        "vectorize" => tl!("Drawing outlines"),
        "compose" => tl!("Composing the page"),
        "skipped" => tl!("Skipping a page that already has text"),
        "done" => tl!("Finishing the page"),
        _ => tl!("Working"),
    }
    .to_string()
}

fn clearscan_progress_label(stage: &str, done: usize, total: usize) -> String {
    let total = total.max(1);
    let page = if stage == "done" { done.max(1).min(total) } else { done.saturating_add(1).min(total) };
    crate::i18n::fmt(
        tl!("ClearScan… {stage}, page {d} of {t}"),
        &[("stage", &clearscan_stage_name(stage)), ("d", &page.to_string()), ("t", &total.to_string())],
    )
}

fn clearscan_fraction(done: usize, total: usize, stage: &str) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let into = match stage {
        "done" => 0.0,
        "render" => 0.1,
        "downsample" => 0.2,
        "preprocess" => 0.3,
        "recognize" => 0.45,
        "components" => 0.55,
        "align" => 0.65,
        "cluster" => 0.75,
        "vectorize" => 0.85,
        "compose" | "skipped" => 0.95,
        _ => 0.0,
    };
    let fraction = (done as f32 + into) / total as f32;
    if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 }
}

/// "ClearScan failed: {e}", in the current language.
fn failed_clearscan(error: &str) -> String {
    crate::i18n::fmt(tl!("{label} failed: {e}"), &[("label", tl!("ClearScan")), ("e", error)])
}

fn clearscan_done_message(report: &pdfcraft_engine::clearscan::ClearScanReport) -> String {
    let replaced = report.pages.iter().filter(|page| page.skipped.is_none() && page.error.is_none()).count();
    let skipped = report.pages.iter().filter(|page| page.skipped.is_some()).count();
    let failed: Vec<&str> = report.pages.iter().filter_map(|page| page.error.as_deref()).collect();
    let mut message = if report.cancelled && replaced == 0 {
        tl!("ClearScan cancelled").to_string()
    } else if report.cancelled && replaced == 1 {
        tl!("ClearScan stopped; 1 page was updated").to_string()
    } else if report.cancelled {
        crate::i18n::fmt(tl!("ClearScan stopped; {n} pages were updated"), &[("n", &replaced.to_string())])
    } else if replaced == 0 && failed.is_empty() {
        tl!("ClearScan left the pages unchanged").to_string()
    } else if replaced == 1 {
        tl!("ClearScan replaced the lettering on 1 page").to_string()
    } else {
        crate::i18n::fmt(tl!("ClearScan replaced the lettering on {n} pages"), &[("n", &replaced.to_string())])
    };
    if skipped > 0 {
        message.push_str(&if skipped == 1 {
            crate::i18n::fmt(tl!("; 1 page already had text"), &[])
        } else {
            crate::i18n::fmt(tl!("; {n} pages already had text"), &[("n", &skipped.to_string())])
        });
    }
    if !failed.is_empty() {
        message.push_str(&crate::i18n::fmt(tl!("; failed: {list}"), &[("list", &failed.join("; "))]));
    }
    message
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
            let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), cancel: false }));
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
        let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), cancel: false }));
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
    fn editable_output_defaults_to_the_clearscan_paper_settings() {
        let draft = OcrDraft::default();
        assert_eq!(draft.output, OcrOutput::SearchableImage);
        assert_eq!((draft.sensitivity, draft.downsample), (50, 1));
        let settings = draft.clearscan_settings();
        assert_eq!((settings.dpi, settings.sensitivity, settings.downsample, settings.skip_text_pages), (300.0, 50, 1, true));
        let mut asked = draft.clone();
        asked.text_pages = true;
        asked.language = "de".into();
        asked.dpi = 150;
        let settings = asked.clearscan_settings();
        assert_eq!(settings.language, "de");
        assert_eq!(settings.dpi, 150.0);
        assert!(!settings.skip_text_pages);
    }

    #[test]
    fn missing_models_show_the_dialog_message_and_do_not_edit() {
        if pdfcraft_engine::ocr::available() {
            return;
        }
        let mut app = app_with(scan());
        app.set_option("language", "en").unwrap();
        app.ocr_draft.output = OcrOutput::EditableText;
        let id = app.active_ids().unwrap().1;
        app.start_ocr();
        assert!(app.clearscan_run.is_none() && app.ocr_run.is_none());
        assert_eq!(app.session.get(id).unwrap().can_undo(), None);
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert!(toast.contains("model files are missing"), "{toast}");
    }

    #[test]
    fn clearscan_progress_names_the_stage_and_cancel_keeps_pages_already_built() {
        let mut app = app_with(scan());
        app.set_option("language", "en").unwrap();
        let id = app.active_ids().unwrap().1;
        let progress = Arc::new(Mutex::new(ClearScanRunProgress { done: 0, total: 2, stage: "vectorize", ..Default::default() }));
        let generation = app.session.get(id).unwrap().edit_generation();
        app.clearscan_run = Some(ClearScanRun { doc: id, progress: progress.clone(), generation });
        app.poll_ocr();
        let notice = app.progress_notice.clone().unwrap();
        assert_eq!(notice.label, "ClearScan… Drawing outlines, page 1 of 2");
        assert!(app.progress_job == crate::widgets::ProgressJob::ClearScan);
        assert!(notice.cancellable);
        app.cancel_clearscan();
        app.poll_ocr();
        assert!(progress.lock().unwrap().cancel);
        assert_eq!(app.progress_notice.as_ref().map(|n| n.label.as_str()), Some("Cancelling…"));
        assert!(!app.progress_notice.as_ref().unwrap().cancellable);

        let replacement = pdfcraft_engine::Session::new().create_blank(200.0, 200.0, 1).unwrap().to_vec();
        let report = pdfcraft_engine::clearscan::ClearScanReport {
            cancelled: true,
            pages: vec![pdfcraft_engine::clearscan::ClearScanPage {
                page: 0,
                text: "H".into(),
                glyphs: 1,
                clusters: 1,
                erased: 1,
                bytes: replacement.len(),
                skipped: None,
                error: None,
            }],
        };
        progress.lock().unwrap().result = Some(Ok(ClearScanBuilt { report, built: vec![(0, replacement)] }));
        app.poll_ocr();
        assert!(app.clearscan_run.is_none() && app.progress_notice.is_none());
        assert_eq!(app.session.get(id).unwrap().can_undo(), Some("ClearScan"));
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert_eq!(toast, "ClearScan stopped; 1 page was updated");
        app.undo();
        assert_eq!(app.session.get(id).unwrap().can_undo(), None);
    }

    #[test]
    fn a_clearscan_run_that_finds_no_models_edits_nothing() {
        let mut app = app_with(scan());
        app.set_option("language", "en").unwrap();
        let id = app.active_ids().unwrap().1;
        let progress = Arc::new(Mutex::new(ClearScanRunProgress { result: Some(Err(ClearScanFail::NoModels)), ..Default::default() }));
        let generation = app.session.get(id).unwrap().edit_generation();
        app.clearscan_run = Some(ClearScanRun { doc: id, progress, generation });
        app.progress_job = crate::widgets::ProgressJob::ClearScan;
        app.progress_notice = Some(crate::widgets::ProgressNotice { label: "ClearScan…".into(), fraction: 0.2, cancellable: true });
        app.poll_ocr();
        assert!(app.clearscan_run.is_none() && app.progress_notice.is_none());
        assert_eq!(app.session.get(id).unwrap().can_undo(), None);
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert!(toast.contains("model files are missing"), "{toast}");
    }

    #[test]
    fn recognize_text_waits_while_optimization_is_running() {
        let mut app = app_with(scan());
        app.set_option("language", "en").unwrap();
        app.ocr_draft.output = OcrOutput::EditableText;
        let id = app.active_ids().unwrap().1;
        app.optimize_run =
            Some(super::super::optimize_ui::OptimizeRun { doc: id, kind: super::super::optimize_ui::OptimizeKind::Reduce, progress: Arc::default() });
        app.start_ocr();
        assert!(app.clearscan_run.is_none() && app.ocr_run.is_none());
        assert_eq!(app.toast.as_ref().map(|toast| toast.0.as_str()), Some("Optimization is already running"));
    }

    #[test]
    fn a_clearscan_result_is_dropped_when_the_document_changed() {
        let mut app = app_with(scan());
        app.set_option("language", "en").unwrap();
        let id = app.active_ids().unwrap().1;
        let generation = app.session.get(id).unwrap().edit_generation();
        let replacement = Session::new().create_blank(200.0, 200.0, 1).unwrap().to_vec();
        let progress = Arc::new(Mutex::new(ClearScanRunProgress {
            result: Some(Ok(ClearScanBuilt {
                report: pdfcraft_engine::clearscan::ClearScanReport { cancelled: false, pages: Vec::new() },
                built: vec![(0, replacement)],
            })),
            ..Default::default()
        }));
        app.clearscan_run = Some(ClearScanRun { doc: id, progress, generation });
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([250, 250, 250, 255])))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        app.session
            .apply(id, Edit::AddImage { page: 0, rect: Some([10.0, 10.0, 50.0, 50.0]), name: "p.png".into(), bytes: Arc::new(png.into_inner()) })
            .unwrap();
        app.poll_ocr();
        assert!(app.clearscan_run.is_none() && app.progress_notice.is_none());
        assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Add image"));
        let toast = app.toast.take().map(|toast| toast.0).unwrap_or_default();
        assert!(toast.contains("document changed"), "{toast}");
    }
}
