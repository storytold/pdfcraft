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

pub struct OcrRun {
    pub doc: DocId,
    /// The document's edit generation when the pages were captured. The result only fits that
    /// state: after an edit, undo or reload, page numbers and positions may no longer match, so a
    /// stale result is dropped rather than applied to whatever now has that index (see `poll_ocr`).
    pub generation: u64,
    pub progress: Arc<Mutex<OcrProgress>>,
    /// Set for a run Edit started on a scanned page, not asked for: it shows no progress, says nothing
    /// when it found no words (a page with text of its own is skipped, and Edit must not make noise),
    /// and its result waits for Read mode and dialogs.
    pub automatic: Option<AutoOcr>,
}

/// What an Edit-started run needs beyond `OcrRun::generation` to let the page it read be judged
/// again once a stale result is dropped.
#[derive(Clone, Copy, Debug)]
pub struct AutoOcr {
    /// The page it reads (0-based), when it was started.
    pub page: usize,
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len().max(1));
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
            craft_ui::ComboBox::from_id_salt("ocr-output").selected_text(tl!("Searchable Image (Exact)")).width(220.0).show_ui(ui, |ui| {
                let _ = ui
                    .selectable_label(true, tl!("Searchable Image (Exact)"))
                    .on_hover_text(tl!("Adds invisible text over each word; the page image is not changed"));
            });
            ui.end_row();
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
    let available = pdfcraft_engine::ocr::available_for(&d.language);
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
        // The document's state now: the result only fits pages captured at this generation,
        // whether the run was asked for or Edit started it (see the generation check in `poll_ocr`).
        let generation = self.session.get(id).map_or(0, |d| d.edit_generation());
        // Automatic runs always target exactly one page (see `auto_ocr_scanned_page`).
        let auto = automatic.then(|| pages.first().map(|&page| AutoOcr { page })).flatten();
        let Some(job) = self.session.ocr_job(id, pages, settings) else { return };
        let progress = Arc::new(Mutex::new(OcrProgress { total: job.pages.len(), ..Default::default() }));
        let p = progress.clone();
        // A failure is written on the worker thread: in the UI's language.
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            let result = recognize_guarded(|| {
                pdfcraft_engine::ocr::engine_for(&job.settings.language).map(|ocr| {
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
        } else if let Err(e) = self.spawn_ocr_worker("pdfcraft-ocr", work) {
            // No worker: report the failure instead of waiting for a result that never comes.
            self.notify_error(e);
            return;
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.ocr_run = Some(OcrRun { doc: id, generation, progress, automatic: auto });
        self.poll_ocr();
    }

    /// Start a recognition worker thread. `fail_spawn` (tests) makes it fail as under resource pressure.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_ocr_worker(&self, name: &str, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
        if self.fail_spawn {
            return Err(std::io::Error::other("could not start a background thread"));
        }
        std::thread::Builder::new().name(name.into()).spawn(work).map(drop)
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
        } else if let Err(e) = self.spawn_ocr_worker("pdfcraft-ocr-files", work) {
            // No worker: report the failure instead of waiting for a result that never comes.
            self.notify_error(e);
            return;
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
    match pdfcraft_engine::ocr::engine_for(&settings.language) {
        Err(e) => failed.push(e),
        Ok(ocr) => {
            for (i, (name, bytes)) in files.into_iter().enumerate() {
                if let Ok(mut s) = p.lock() {
                    s.done = i;
                }
                let r = pdfcraft_engine::ocr::recognize_file(&name, Arc::new(bytes), None, settings.clone(), &ocr, |_, _| true);
                let saved = r.and_then(|r| {
                    // Never replaces a file: a taken name (the source itself, run in place, or an
                    // earlier run's output) gets a number such as " (2)" instead (#572).
                    #[cfg(not(target_arch = "wasm32"))]
                    pdfcraft_platform::staging::publish(&dir.join(&name), &r.bytes).map_err(|e| e.to_string())?;
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

    /// Show progress; apply the result once the worker is done.
    pub(crate) fn poll_ocr(&mut self) {
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
        let (doc, generation, progress, auto) = (run.doc, run.generation, run.progress.clone(), run.automatic);
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
        // The pages were captured at `generation`. After an edit, undo or reload, those page
        // numbers and positions no longer match, so the words would land on the wrong pages: the
        // result is dropped, not applied to whatever now has that index. A run Edit started says
        // nothing and lets the page in view be judged again; one the user asked for says so.
        if self.session.get(doc).is_some_and(|d| d.edit_generation() != generation) {
            if let Some(a) = auto {
                if let Some(view) = self.views.iter_mut().find(|v| v.id == doc) {
                    view.auto_ocr_tried.remove(&a.page);
                }
            } else {
                self.notify_tr("The document changed while text was being recognized, so the result was discarded. Run Recognize text again.");
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
            OcrRun { doc: app.active_ids().unwrap().1, generation, progress, automatic: automatic.then_some(AutoOcr { page: 0 }) }
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
        app.ocr_run = Some(OcrRun { doc: id, generation, progress, automatic: Some(AutoOcr { page: 0 }) });
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
        app.ocr_run = Some(OcrRun { doc: DocId(1), generation: 0, progress, automatic: None });
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

    /// An app with a one-page document and a finished recognition run captured at that state, not
    /// started by Edit.
    fn app_with_finished_run() -> Option<(PdfCraftApp, DocId)> {
        let mut app = app_with(Session::new().create_from_text("t", "Some text").ok()?.to_vec());
        let id = app.active_ids()?.1;
        let generation = app.session.get(id)?.edit_generation();
        let word = pdfcraft_engine::ocr::PlacedWord { text: "scanned".into(), origin: [50.0, 700.0], across: [40.0, 0.0], up: [0.0, 12.0] };
        let page = pdfcraft_engine::ocr::OcrPage { page: 0, words: vec![word], already_on_page: 0, skipped: None };
        let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), cancel: false }));
        app.ocr_run = Some(OcrRun { doc: id, generation, progress, automatic: None });
        Some((app, id))
    }

    /// #1801: an unchanged document takes the result as one "Recognize text" edit.
    #[test]
    fn result_is_applied_when_the_document_is_unchanged() {
        let (mut app, id) = app_with_finished_run().expect("app with a run");
        app.poll_ocr();
        assert!(app.ocr_run.is_none());
        assert_eq!(app.session.get(id).and_then(|d| d.can_undo()), Some("Recognize text"));
    }

    /// #1801: an edit made while recognition ran changes the pages; the old result is refused, and
    /// (unlike a run Edit started silently retrying) the user is told why nothing was added.
    #[test]
    fn result_is_refused_when_the_document_changed_meanwhile() {
        let (mut app, id) = app_with_finished_run().expect("app with a run");
        let note = pdfcraft_engine::AddedText { rect: [50.0, 600.0, 300.0, 640.0], text: "Added meanwhile".into(), size: 12.0, ..Default::default() };
        app.session.apply(id, Edit::AddText { page: 0, text: note }).expect("edit");
        let before = app.session.get(id).and_then(|d| d.can_undo().map(str::to_owned));
        app.poll_ocr();
        assert!(app.ocr_run.is_none());
        assert_eq!(app.session.get(id).and_then(|d| d.can_undo().map(str::to_owned)), before, "nothing was added");
        let toast = app.toast.clone().expect("toast").0;
        assert!(toast.contains("changed while text was being recognized"), "{toast}");
    }
}
