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
        OcrSettings { dpi: self.dpi as f32, language: self.language.clone(), skip_text_pages: !self.text_pages }
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

pub struct OcrRun {
    pub doc: DocId,
    /// The document's edit generation when the pages were captured; the result only fits that state.
    pub generation: u64,
    pub progress: Arc<Mutex<OcrProgress>>,
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
    group(ui, "Settings", &mut |ui| {
        egui::Grid::new("ocr-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tl!("Document language"));
            let name = LANGUAGES.iter().find(|l| l.0 == d.language).map_or("English", |l| l.1);
            egui::ComboBox::from_id_salt("ocr-language").selected_text(tl!(name)).width(220.0).show_ui(ui, |ui| {
                for (code, name) in LANGUAGES {
                    ui.selectable_value(&mut d.language, (*code).to_string(), tl!(*name));
                }
            });
            ui.end_row();
            ui.label(tl!("Output"));
            egui::ComboBox::from_id_salt("ocr-output").selected_text(tl!("Searchable Image (Exact)")).width(220.0).show_ui(ui, |ui| {
                let _ = ui
                    .selectable_label(true, tl!("Searchable Image (Exact)"))
                    .on_hover_text(tl!("Adds invisible text over each word; the page image is not changed"));
            });
            ui.end_row();
            ui.label(tl!("Downsample to"));
            egui::ComboBox::from_id_salt("ocr-dpi").selected_text(format!("{} dpi", d.dpi)).width(220.0).show_ui(ui, |ui| {
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

pub(crate) fn toggle(ui: &mut egui::Ui, app: &mut PdfCraftApp) {
    let mut enabled = app.auto_ocr;
    if ui
        .add(widgets::checkbox(&mut enabled, tl!("Recognize scanned text automatically")))
        .on_hover_text(tl!("Ask whether to recognize the current page or all pages when editing a scan."))
        .changed()
    {
        app.set_auto_ocr(enabled);
    }
}

pub(crate) fn scan_prompt_body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> (Option<bool>, bool) {
    ui.label(egui::RichText::new(tl!("Scanned text detected")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(tl!("Recognize text on this page or throughout the document?"));
    ui.add_space(8.0);
    toggle(ui, app);
    ui.add_space(8.0);
    let mut all = None;
    let mut cancel = false;
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Current page"), true).clicked() {
            all = Some(false);
        }
        if widgets::pill_button(ui, tl!("All pages"), false).clicked() {
            all = Some(true);
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (all, cancel)
}

impl PdfCraftApp {
    pub(crate) fn set_auto_ocr(&mut self, enabled: bool) {
        if enabled && !self.auto_ocr {
            for view in &mut self.views {
                view.ocr_prompted.clear();
                view.ocr_checked.clear();
            }
        }
        self.auto_ocr = enabled;
    }

    /// The scan-detection approach from #1609, with a page-choice prompt before any edit.
    pub(crate) fn prompt_scan_ocr(&mut self) {
        let editing = (self.left_open && self.left == crate::LeftPanel::Tool("edit"))
            || matches!(self.quick_tool, crate::QuickTool::EditText | crate::QuickTool::AddText);
        if !self.auto_ocr
            || !editing
            || self.mode == crate::Mode::Read
            || self.modal_open()
            || self.palette_open
            || self.ocr_run.is_some()
            || self.ocr_batch.is_some()
        {
            return;
        }
        let Some((view, doc)) = self.active_ids() else { return };
        let page = self.views[view].current;
        if self.views[view].ocr_prompted.contains(&page)
            || self.views[view].line_editor.is_some()
            || self.views[view].pending_edit.is_some()
            || self.views[view].snapshot.is_some()
        {
            return;
        }
        let Some(document) = self.session.get(doc) else { return };
        let generation = document.edit_generation();
        if document.read_only_reason.is_some() || !document.allows_modification() || self.views[view].ocr_checked.get(&page) == Some(&generation) {
            return;
        }
        self.views[view].ocr_checked.insert(page, generation);
        let Some(info) = document.info.pages.get(page) else { return };
        let page_area = f64::from(info.width) * f64::from(info.height);
        if !page_area.is_finite() || page_area <= 0.0 || !document.text_blocks(page).is_empty() {
            return;
        }
        let scanned = document.page_images(page).iter().any(|image| {
            let [x0, y0, x1, y1] = image.visible_rect.unwrap_or(image.rect);
            let area = (x1 - x0).abs() * (y1 - y0).abs();
            image.width > 1 && image.height > 1 && area.is_finite() && area >= 0.5 * page_area
        });
        if scanned && pdfcraft_engine::ocr::available() {
            self.views[view].ocr_prompted.insert(page);
            self.dialog = Some(crate::Dialog::RecognizeScan { doc, page, generation });
        }
    }

    pub(crate) fn recognize_scan(&mut self, doc: DocId, page: usize, generation: u64, all: bool) {
        if self.active_ids().map(|(_, id)| id) != Some(doc) || self.session.get(doc).is_none_or(|d| d.edit_generation() != generation) {
            self.notify_tr("The document changed while text was being recognized, so the result was discarded. Run Recognize text again.");
            return;
        }
        let mut settings = self.ocr_draft.settings();
        settings.skip_text_pages = true;
        if all {
            if let Some((view, _)) = self.active_ids()
                && let Some(document) = self.session.get(doc)
            {
                self.views[view].ocr_prompted.extend(0..document.info.pages.len());
            }
            self.start_ocr_pages(doc, &[], settings);
        } else {
            self.start_ocr_pages(doc, &[page], settings);
        }
    }

    /// Recognize text on the pages chosen in the dialog, in the background.
    pub fn start_ocr(&mut self) {
        let Some((vi, id)) = self.active_ids() else { return };
        let d = &self.ocr_draft;
        let pages: Vec<usize> = match d.pages {
            OcrPages::All => Vec::new(),
            OcrPages::Current => vec![self.views[vi].current],
            OcrPages::Range => (d.from.saturating_sub(1)..d.to).collect(),
        };
        let settings = d.settings();
        self.start_ocr_pages(id, &pages, settings);
    }

    fn start_ocr_pages(&mut self, id: DocId, pages: &[usize], settings: OcrSettings) {
        if self.ocr_run.is_some() || self.ocr_batch.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        let Some(document) = self.session.get(id) else { return };
        if let Some(why) = document.read_only_reason.clone() {
            self.notify(why);
            return;
        }
        if !document.allows_modification() {
            self.notify_tr("This document does not allow editing");
            return;
        }
        let Some(job) = self.session.ocr_job(id, pages, settings) else { return };
        let generation = self.session.get(id).map_or(0, |d| d.edit_generation());
        let progress = Arc::new(Mutex::new(OcrProgress { total: job.pages.len(), ..Default::default() }));
        let p = progress.clone();
        let work = move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pdfcraft_engine::ocr::engine().map(|ocr| {
                    job.run(&ocr, |done, total| {
                        let Ok(mut s) = p.lock() else { return false };
                        s.done = done;
                        s.total = total;
                        !s.cancel
                    })
                })
            }))
            .unwrap_or_else(|_| Err("Text recognition failed; the document was not changed.".into()));
            if let Ok(mut s) = p.lock() {
                s.result = Some(result);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else {
            if let Err(error) = std::thread::Builder::new().name("pdfcraft-ocr".into()).spawn(work) {
                self.notify_error(error);
                return;
            }
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.ocr_run = Some(OcrRun { doc: id, generation, progress });
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
            if let Ok(mut s) = p.lock() {
                s.done = s.total;
                s.message = Some(msg);
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
            let msg = b.lock().ok().map(|mut s| s.message.take().ok_or((s.done, s.total)));
            match msg {
                Some(Ok(m)) => {
                    self.ocr_batch = None;
                    self.notify(m);
                }
                Some(Err((done, total))) => {
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
                None => self.ocr_batch = None,
            }
        }
        let Some(run) = self.ocr_run.as_ref() else { return };
        let (doc, generation, progress) = (run.doc, run.generation, run.progress.clone());
        let Ok(mut s) = progress.lock() else { return };
        let Some(result) = s.result.take() else {
            let msg = crate::i18n::fmt(
                tl!("Recognizing text… page {d} of {t}"),
                &[("d", &(s.done + 1).min(s.total.max(1)).to_string()), ("t", &s.total.max(1).to_string())],
            );
            drop(s);
            if self.toast.as_ref().is_none_or(|t| t.0 != msg) {
                self.notify(msg);
            }
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        };
        drop(s);
        self.ocr_run = None;
        let found = match result {
            Ok(f) => f,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        // The pages were captured at `generation`; after any edit, undo or reload the page numbers
        // and positions no longer match, so the words would land on the wrong pages (or bring back
        // text that was removed meanwhile).
        if self.session.get(doc).is_some_and(|d| d.edit_generation() != generation) {
            self.notify_tr("The document changed while text was being recognized, so the result was discarded. Run Recognize text again.");
            return;
        }
        let read = found.iter().filter(|p| p.skipped.is_none()).count();
        let skipped = found.len() - read;
        match self.session.apply_ocr(doc, &found) {
            Ok(words) => {
                if let Some(info) = self.session.get(doc).map(|d| d.info.clone())
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == doc)
                {
                    view.document_changed(&info);
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

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_engine::ocr::PlacedWord;
    use pdfcraft_engine::{AddedText, Edit, Session};

    /// An app with a one-page document and a finished recognition run captured at that state.
    fn app_with_finished_run() -> Option<(PdfCraftApp, DocId)> {
        let s = Session::new();
        let bytes = s.create_from_text("t", "Some text").ok()?;
        let mut app = PdfCraftApp::new();
        app.open_bytes("t.pdf", None, bytes.to_vec()).ok()?;
        let id = app.active_ids()?.1;
        let generation = app.session.get(id)?.edit_generation();
        let word = PlacedWord { text: "scanned".into(), origin: [50.0, 700.0], across: [40.0, 0.0], up: [0.0, 12.0] };
        let page = OcrPage { page: 0, words: vec![word], already_on_page: 0, skipped: None };
        let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), cancel: false }));
        app.ocr_run = Some(OcrRun { doc: id, generation, progress });
        Some((app, id))
    }

    /// An unchanged document takes the result as one "Recognize text" edit.
    #[test]
    fn result_is_applied_when_the_document_is_unchanged() {
        let (mut app, id) = app_with_finished_run().expect("app with a run");
        app.poll_ocr();
        assert!(app.ocr_run.is_none());
        assert_eq!(app.session.get(id).and_then(|d| d.can_undo()), Some("Recognize text"));
    }

    /// An edit made while recognition ran changes the pages; the old result is refused, not applied.
    #[test]
    fn result_is_refused_when_the_document_changed_meanwhile() {
        let (mut app, id) = app_with_finished_run().expect("app with a run");
        let note = AddedText { rect: [50.0, 600.0, 300.0, 640.0], text: "Added meanwhile".into(), size: 12.0, ..Default::default() };
        app.session.apply(id, Edit::AddText { page: 0, text: note }).expect("edit");
        let before = app.session.get(id).and_then(|d| d.can_undo().map(str::to_owned));
        app.poll_ocr();
        assert!(app.ocr_run.is_none());
        assert_eq!(app.session.get(id).and_then(|d| d.can_undo().map(str::to_owned)), before, "nothing was added");
        let toast = app.toast.clone().expect("toast").0;
        assert!(toast.contains("changed while text was being recognized"), "{toast}");
    }
}
