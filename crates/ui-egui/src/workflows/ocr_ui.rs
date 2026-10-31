//! Scan & OCR ▸ Recognize text: the Recognize Text dialog (pages, language, output, resolution)
//! and the background run with its progress. Recognition happens on a worker thread; the result
//! is applied as one undoable edit when it is done.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::ocr::{EngineChoice, LANGUAGES, MergeStrategy, OcrPage, OcrSettings, OutputMode};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// Which pages to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrPages {
    All,
    Current,
    Range,
}

/// The dialog's choices (kept between runs, as in Acrobat). The Recognize Text dialog, the OCR
/// Verify screen and the batch options all read and write this one draft, and it is what the
/// preferences persist: language, output mode, the four preprocessing flags, engine, strategy
/// and the three skip flags — never review edits, verified flags or the selection.
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
    /// Which engine reads the pages.
    pub engine: EngineChoice,
    /// How two engines' readings combine.
    pub strategy: MergeStrategy,
    /// Preprocessing flags, all off by default.
    pub auto_rotate: bool,
    pub deskew: bool,
    pub denoise: bool,
    pub binarize: bool,
    /// What Accept produces.
    pub output_mode: OutputMode,
    /// Batch: leave files that already contain text alone.
    pub skip_text_files: bool,
    /// Batch: leave pages that already contain text alone.
    pub skip_text_pages: bool,
    /// Batch: read everything, overriding both skips.
    pub force_ocr: bool,
    /// Batch: where the searchable copies go (None: ask each time). Remembered from the last
    /// batch's folder pick, so a repeated export into the same place does not re-ask.
    pub output_folder: Option<String>,
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
            engine: EngineChoice::default(),
            strategy: MergeStrategy::default(),
            auto_rotate: false,
            deskew: false,
            denoise: false,
            binarize: false,
            output_mode: OutputMode::default(),
            skip_text_files: false,
            skip_text_pages: true,
            force_ocr: false,
            output_folder: None,
        }
    }
}

impl OcrDraft {
    /// The recognition settings these choices make.
    pub fn settings(&self) -> OcrSettings {
        OcrSettings {
            dpi: self.dpi as f32,
            language: self.language.clone(),
            engine: self.engine,
            strategy: self.strategy,
            auto_rotate: self.auto_rotate,
            deskew: self.deskew,
            denoise: self.denoise,
            binarize: self.binarize,
            output_mode: self.output_mode,
            skip_text_files: self.skip_text_files,
            skip_text_pages: self.skip_text_pages && !self.text_pages,
            force_ocr: self.force_ocr,
            ..Default::default()
        }
    }

    /// Restore from the preferences JSON (`ocr` object); unknown or malformed values keep the
    /// defaults, like every other restored setting.
    pub fn apply_pref(&mut self, v: &serde_json::Value) {
        if let Some(l) = v["language"].as_str().filter(|l| LANGUAGES.iter().any(|x| x.0 == *l)) {
            self.language = l.to_string();
        }
        if let Some(e) = v["engine"].as_str() {
            self.engine = EngineChoice::parse(e);
        }
        if let Some(s) = v["strategy"].as_str() {
            self.strategy = match s.trim().to_ascii_lowercase().as_str() {
                "confidence" | "confidence-weighted" | "confidence_weighted" => MergeStrategy::ConfidenceWeighted,
                "rover" | "rover-vote" | "vote" => MergeStrategy::RoverVote,
                "primary" | "primary-only" => MergeStrategy::PrimaryOnly,
                _ => MergeStrategy::PrimaryOnly,
            };
        } else if self.engine == EngineChoice::Auto {
            // No strategy was saved with "Automatic": it reads the way Automatic reads by
            // default, not as "Primary only" (which would never bring in the second engine).
            self.strategy = self.engine.default_strategy();
        }
        if let Some(m) = v["output_mode"].as_str() {
            self.output_mode = OutputMode::parse(m);
        }
        for (key, flag) in
            [("auto_rotate", &mut self.auto_rotate), ("deskew", &mut self.deskew), ("denoise", &mut self.denoise), ("binarize", &mut self.binarize)]
        {
            if let Some(on) = v[key].as_bool() {
                *flag = on;
            }
        }
        for (key, flag) in
            [("skip_text_files", &mut self.skip_text_files), ("skip_text_pages", &mut self.skip_text_pages), ("force_ocr", &mut self.force_ocr)]
        {
            if let Some(on) = v[key].as_bool() {
                *flag = on;
            }
        }
        if let Some(f) = v["output_folder"].as_str().filter(|f| !f.is_empty()) {
            self.output_folder = Some(f.to_string());
        }
    }

    /// The preferences JSON for `persist`.
    pub fn to_pref(&self) -> serde_json::Value {
        serde_json::json!({
            "language": self.language,
            "engine": self.engine.name(),
            "strategy": match self.strategy {
                MergeStrategy::PrimaryOnly => "primary",
                MergeStrategy::ConfidenceWeighted => "confidence",
                MergeStrategy::RoverVote => "rover",
            },
            "output_mode": self.output_mode.name(),
            "auto_rotate": self.auto_rotate,
            "deskew": self.deskew,
            "denoise": self.denoise,
            "binarize": self.binarize,
            "skip_text_files": self.skip_text_files,
            "skip_text_pages": self.skip_text_pages,
            "force_ocr": self.force_ocr,
            "output_folder": self.output_folder,
        })
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

/// Recognize text in multiple files: files done, total, and the summary once finished. Cancel
/// is asked for between files: the file in flight finishes, the rest are counted as not run.
#[derive(Default)]
pub struct BatchProgress {
    pub done: usize,
    pub total: usize,
    pub message: Option<String>,
    pub cancel: bool,
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
    /// What the document looked like when the job started: a result is applied only while it
    /// still does (the dialog closes on Go, so the user can edit, or redact, while this runs).
    pub guard: pdfcraft_engine::ocr::OcrGuard,
    /// Set for a run Edit started on a scanned page, not asked for: it shows no progress, says nothing
    /// when it found no words (a page with text of its own is skipped, and Edit must not make noise),
    /// and its result waits for Read mode and dialogs and is dropped if the document changed.
    pub automatic: Option<AutoOcr>,
    /// The dialog promised an editable document: the result goes to the OCR Verify screen
    /// (whose Accept produces the editable document), not to an invisible layer.
    pub editable: bool,
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

/// The Tesseract probe runs in the background (the interface never waits on a process): while
/// it runs, an engine choice reads as "still checking", so look again soon.
pub(crate) fn keep_asking_while_probing(ctx: &egui::Context) {
    if pdfcraft_engine::ocr::engine_probe_running() {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
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
            ui.label(tl!("Engine"));
            let engine_name = match d.engine {
                EngineChoice::Auto => tl!("Automatic"),
                EngineChoice::Ocrs => tl!("Built-in"),
                EngineChoice::Tesseract => tl!("Tesseract"),
            };
            craft_ui::ComboBox::from_id_salt("ocr-engine").selected_text(engine_name).width(220.0).show_ui(ui, |ui| {
                let choices: [(EngineChoice, &str); 3] =
                    [(EngineChoice::Auto, "Automatic"), (EngineChoice::Ocrs, "Built-in"), (EngineChoice::Tesseract, "Tesseract")];
                for (choice, name) in choices {
                    ui.selectable_value(&mut d.engine, choice, tl!(name));
                }
            });
            ui.end_row();
            ui.label(tl!("Combine engines"));
            let strategy_name = match d.strategy {
                MergeStrategy::PrimaryOnly => tl!("Primary only"),
                MergeStrategy::ConfidenceWeighted => tl!("Confidence weighted"),
                MergeStrategy::RoverVote => tl!("ROVER vote"),
            };
            craft_ui::ComboBox::from_id_salt("ocr-strategy").selected_text(strategy_name).width(220.0).show_ui(ui, |ui| {
                let choices: [(MergeStrategy, &str); 3] = [
                    (MergeStrategy::PrimaryOnly, "Primary only"),
                    (MergeStrategy::ConfidenceWeighted, "Confidence weighted"),
                    (MergeStrategy::RoverVote, "ROVER vote"),
                ];
                for (choice, name) in choices {
                    ui.selectable_value(&mut d.strategy, choice, tl!(name));
                }
            });
            ui.end_row();
            ui.label(tl!("Output"));
            let mode_name = match d.output_mode {
                OutputMode::Searchable => tl!("Searchable Image (Exact)"),
                OutputMode::EditableText => tl!("Editable text"),
            };
            craft_ui::ComboBox::from_id_salt("ocr-output").selected_text(mode_name).width(220.0).show_ui(ui, |ui| {
                let s = ui
                    .selectable_label(d.output_mode == OutputMode::Searchable, tl!("Searchable Image (Exact)"))
                    .on_hover_text(tl!("Adds invisible text over each word; the page image is not changed"));
                if s.clicked() {
                    d.output_mode = OutputMode::Searchable;
                }
                let e = ui
                    .selectable_label(d.output_mode == OutputMode::EditableText, tl!("Editable text"))
                    .on_hover_text(tl!("Writes a new document of visible text; the scan is not changed"));
                if e.clicked() {
                    d.output_mode = OutputMode::EditableText;
                }
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
        ui.add(crate::widgets::checkbox(&mut d.auto_rotate, tl!("Straighten pages scanned sideways or upside down")));
        ui.add(crate::widgets::checkbox(&mut d.deskew, tl!("Straighten skewed text lines")));
        ui.add(crate::widgets::checkbox(&mut d.denoise, tl!("Remove scan speckle")));
        ui.add(crate::widgets::checkbox(&mut d.binarize, tl!("Black and white (binarize)")));
    });
    group(ui, "Skip", &mut |ui| {
        ui.add(crate::widgets::checkbox(&mut d.skip_text_pages, tl!("Skip pages that already have text")))
            .on_hover_text(tl!("Pages with renderable text are left alone and reported as skipped"));
        ui.add(crate::widgets::checkbox(&mut d.skip_text_files, tl!("Skip files that already have text")))
            .on_hover_text(tl!("In multiple files: a file with any text is its own skipped bucket, and nothing is written for it"));
        ui.add(crate::widgets::checkbox(&mut d.force_ocr, tl!("Force OCR (override skip options)")))
            .on_hover_text(tl!("Reads pages and files even where the skip options would leave them alone"));
    });
    group(ui, "Output folder", &mut |ui| {
        ui.horizontal(|ui| match &d.output_folder {
            Some(f) => {
                ui.monospace(f).on_hover_text(tl!("The searchable copies are written here under the source names"));
                if ui.small_button(tl!("Clear")).clicked() {
                    d.output_folder = None;
                }
            }
            None => {
                ui.label(tl!("Automatic")).on_hover_text(tl!("A folder is asked for when a multiple-file run starts"));
            }
        });
    });
    keep_asking_while_probing(ui.ctx());
    if let Some(why) = pdfcraft_engine::ocr::engine_unavailable_reason(d.engine) {
        // Said in the interface's language: the built-in engine's missing models get the
        // notice users know; Tesseract's reasons are catalog keys of their own.
        let text = match d.engine {
            pdfcraft_engine::ocr::EngineChoice::Tesseract => crate::i18n::t(&why).to_string(),
            _ => tl!("Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them.")
                .to_string(),
        };
        ui.label(egui::RichText::new(text).small().color(t.text_muted));
        ui.add_space(6.0);
    }
    ui.add_space(6.0);
    let (mut go, mut cancel) = (false, false);
    let can_run = pdfcraft_engine::ocr::engine_unavailable_reason(d.engine).is_none();
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(can_run, |ui| widgets::pill_button(ui, tl!("Recognize text"), true)).inner.clicked() {
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
        if pdfcraft_engine::ocr::engine_unavailable_reason(pdfcraft_engine::ocr::EngineChoice::Ocrs).is_some() {
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
        let Some(job) = self.session.ocr_job(id, pages, settings.clone()) else { return };
        let guard = job.guard.clone();
        let editable = settings.output_mode == OutputMode::EditableText;
        let progress = Arc::new(Mutex::new(OcrProgress { total: job.pages.len(), ..Default::default() }));
        let p = progress.clone();
        // A failure is written on the worker thread: in the UI's language.
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            let result = recognize_guarded(|| {
                pdfcraft_engine::ocr::recognizers(&settings).map(|r| {
                    job.run(&r, |done, total| {
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
        self.ocr_run = Some(OcrRun { doc: id, progress, guard, automatic: auto, editable });
        self.poll_ocr();
    }

    /// Recognize text in each file, writing the searchable copies into a folder the user picks
    /// (the export folder override in tests), under the same names.
    pub fn ocr_files(&mut self, files: Vec<(String, Vec<u8>)>) {
        if self.ocr_batch.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        // The settings showing now, not when the folder arrives. A remembered folder (the last
        // batch's pick, persisted with the preferences) is reused; otherwise the user is asked.
        let settings = self.ocr_draft.settings();
        #[cfg(not(target_arch = "wasm32"))]
        match self.export_dir_override.clone().or_else(|| self.ocr_draft.output_folder.clone()) {
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
        // The folder is remembered (the next batch reuses it; the dialog's Clear un-remembers).
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ocr_draft.output_folder = Some(dir.to_string_lossy().into_owned());
        }
        // Two inputs of one file name (from different folders) would write the same output
        // (or download under the same name): refused before anything is read.
        let twice = pdfcraft_engine::ocr::duplicate_output_names(files.iter().map(|(name, _)| name.as_str()));
        if !twice.is_empty() {
            self.notify_error(crate::i18n::fmt(
                tl!("Several files have the same name, so their results would overwrite each other; nothing was written: {list}"),
                &[("list", &twice.join(", "))],
            ));
            return;
        }
        // Collision pre-flight: a searchable copy never silently overwrites. Every target is
        // checked before the first page is read; a collision stops the batch here.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let hits: Vec<String> = files
                .iter()
                .filter_map(|(name, _)| {
                    let target = dir.join(name);
                    target.exists().then(|| target.to_string_lossy().into_owned())
                })
                .take(5)
                .collect();
            if !hits.is_empty() {
                self.notify_error(crate::i18n::fmt(
                    tl!("The output folder already holds files of these names; nothing was written: {list}"),
                    &[("list", &hits.join(", "))],
                ));
                return;
            }
        }
        // The engines are built (and named if they cannot be) before anything starts.
        let recognizers = match pdfcraft_engine::ocr::recognizers(&settings) {
            Ok(r) => r,
            Err(why) => {
                self.notify_error(why);
                return;
            }
        };
        let progress = Arc::new(Mutex::new(BatchProgress { total: files.len(), ..Default::default() }));
        let p = progress.clone();
        // The summary is written on the worker thread: draw it in the UI's language.
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            #[cfg(not(target_arch = "wasm32"))]
            let write = move |name: &str, r: &pdfcraft_engine::ocr::FileResult| {
                crate::editing::write_atomically(&dir.join(name).to_string_lossy(), &r.bytes).map_err(|e| e.to_string())
            };
            #[cfg(target_arch = "wasm32")]
            let write = move |name: &str, r: &pdfcraft_engine::ocr::FileResult| crate::editing::download(name, &r.bytes);
            // A panic in the recognizer still ends the batch with a summary (#3919).
            let msg = pdfcraft_engine::guard(|| run_batch_files(files, settings, &recognizers, &p, write)).unwrap_or_else(|panic| failed(&panic));
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

    /// Ask the running batch to stop: the file in flight finishes, the rest are counted as not
    /// run.
    pub fn cancel_ocr_batch(&mut self) {
        if let Some(b) = &self.ocr_batch
            && let Ok(mut s) = b.lock()
        {
            s.cancel = true;
        }
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
        let (doc, progress, auto, guard, editable) = (run.doc, run.progress.clone(), run.automatic, run.guard.clone(), run.editable);
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
        // The dialog's "Editable text" choice is honored: the recognition is not written as an
        // invisible layer here. It opens in the OCR Verify screen, whose Accept produces the
        // editable document (a NEW file) the choice promises.
        if editable && !automatic {
            self.ocr_verify.load(doc, guard, found);
            self.ocr_verify.open = true;
            self.notify_tr("Review the recognition, then Accept to write the editable document");
            return;
        }
        let read = found.iter().filter(|p| p.skipped.is_none()).count();
        let skipped = found.len() - read;
        // The guard refuses a result that no longer fits its document (the user edited, or
        // redacted, while the worker was out); its reason says so.
        match self.session.apply_ocr_checked(doc, &found, &guard) {
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
                // The pages' own notes (the editable output's unrenderable-character warning,
                // for one) are part of the result: said here, not dropped.
                for note in found.iter().flat_map(|p| p.notes.iter()) {
                    msg.push_str("; ");
                    msg.push_str(note);
                }
                self.notify(msg);
            }
            Err(e) => self.notify_error(e),
        }
    }
}

/// The batch's worker: read each file, hand the searchable copy to `write` (the output folder,
/// or the web's download), count the buckets, and compose the summary. A file the skip-files
/// option passes over is its own bucket and `write` is never called for it. A cancel is asked
/// for between files: the file in flight finishes, the rest are counted as not run.
pub(crate) fn run_batch_files(
    files: Vec<(String, Vec<u8>)>,
    settings: OcrSettings,
    recognizers: &pdfcraft_engine::ocr::Recognizers,
    progress: &Mutex<BatchProgress>,
    mut write: impl FnMut(&str, &pdfcraft_engine::ocr::FileResult) -> Result<(), String>,
) -> String {
    let total = files.len();
    let (mut ok, mut skipped, mut words, mut failed, mut notes) = (0usize, 0usize, 0usize, Vec::new(), Vec::new());
    let mut not_run: Option<usize> = None;
    for (i, (name, bytes)) in files.into_iter().enumerate() {
        if progress.lock().map(|s| s.cancel).unwrap_or(false) {
            not_run = Some(total - i);
            break;
        }
        if let Ok(mut s) = progress.lock() {
            s.done = i;
        }
        let r = pdfcraft_engine::ocr::recognize_file(&name, Arc::new(bytes), None, settings.clone(), recognizers, |_, _| true);
        match r {
            Ok(res) if res.skipped() => skipped += 1,
            Ok(res) => match write(&name, &res) {
                Ok(()) => {
                    ok += 1;
                    words += res.words();
                    // The batch's low-confidence note: at or under the settings' threshold (60 by
                    // default), on 1-based sorted pages.
                    if let Some((count, pages)) = pdfcraft_engine::ocr::low_confidence_pages(&res.pages, settings.low_confidence_threshold) {
                        let list = pages.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ");
                        notes.push(crate::i18n::fmt(
                            tl!("{name}: {n} low-confidence word(s) on pages {pages}"),
                            &[("name", &name), ("n", &count.to_string()), ("pages", &list)],
                        ));
                    }
                }
                Err(e) => failed.push(format!("{name}: {e}")),
            },
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    batch_summary(ok, words, skipped, &failed, &notes, not_run)
}

/// The batch summary: what was read, the skipped files as their own bucket, the failures, the
/// low-confidence notes and (on a cancel) how many files were not run.
fn batch_summary(ok: usize, words: usize, skipped: usize, failed: &[String], notes: &[String], not_run: Option<usize>) -> String {
    let mut msg = if ok == 1 {
        crate::i18n::fmt(tl!("Recognized {w} words in 1 file"), &[("w", &words.to_string())])
    } else {
        crate::i18n::fmt(tl!("Recognized {w} words in {n} files"), &[("w", &words.to_string()), ("n", &ok.to_string())])
    };
    if skipped > 0 {
        msg.push_str(&crate::i18n::fmt(tl!("; {n} files already had text"), &[("n", &skipped.to_string())]));
    }
    if !failed.is_empty() {
        msg.push_str(&crate::i18n::fmt(tl!("; failed: {list}"), &[("list", &failed.join("; "))]));
    }
    if let Some(rest) = not_run {
        msg.push_str(&crate::i18n::fmt(tl!("; cancelled — {n} files were not run"), &[("n", &rest.to_string())]));
    }
    for note in notes {
        msg.push_str("; ");
        msg.push_str(note);
    }
    msg
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
    use pdfcraft_engine::ocr::{Recognizer, Recognizers};
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
        let word = || pdfcraft_engine::ocr::PlacedWord {
            text: "scanned".into(),
            origin: [72.0, 700.0],
            across: [60.0, 0.0],
            up: [0.0, 12.0],
            confidence: None,
            source: "ocrs".into(),
        };
        let finished = |app: &PdfCraftApp, automatic: bool| {
            let generation = app.session.get(app.active_ids().unwrap().1).unwrap().edit_generation();
            let page = pdfcraft_engine::ocr::OcrPage { page: 0, words: vec![word()], already_on_page: 0, skipped: None, notes: Vec::new() };
            let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), cancel: false }));
            let guard = app.session.ocr_guard(app.active_ids().unwrap().1).unwrap();
            OcrRun {
                doc: app.active_ids().unwrap().1,
                progress,
                guard,
                automatic: automatic.then_some(AutoOcr { page: 0, generation }),
                editable: false,
            }
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
            words: vec![pdfcraft_engine::ocr::PlacedWord {
                text: "scanned".into(),
                origin: [72.0, 700.0],
                across: [60.0, 0.0],
                up: [0.0, 12.0],
                confidence: None,
                source: "ocrs".into(),
            }],
            already_on_page: 0,
            skipped: None,
            notes: Vec::new(),
        };
        let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), cancel: false }));
        let guard = app.session.ocr_guard(id).unwrap();
        app.ocr_run = Some(OcrRun { doc: id, progress, guard, automatic: Some(AutoOcr { page: 0, generation }), editable: false });
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
        let pdf = pdfcraft_engine::Session::new().create_from_text("t", "guard fixture").unwrap();
        app.open_bytes("a.pdf", None, pdf.to_vec()).unwrap();
        let id = app.active_ids().unwrap().1;
        let guard = app.session.ocr_guard(id).unwrap();
        let progress = Arc::new(Mutex::new(OcrProgress { total: 3, ..Default::default() }));
        poison(&progress);
        app.ocr_run = Some(OcrRun { doc: id, progress, guard, automatic: None, editable: false });
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

    /// A fake engine: one confident word per call, no filesystem, no models needed.
    struct Fake {
        confidence: f32,
    }

    impl Recognizer for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn available(&self) -> bool {
            true
        }
        fn languages(&self) -> Vec<(String, String)> {
            vec![("en".into(), "English".into())]
        }
        fn recognize(
            &self,
            _image: &pdfcraft_ocr::OcrImage,
            _options: &pdfcraft_ocr::RecognizeOptions,
        ) -> Result<Vec<pdfcraft_ocr::Line>, pdfcraft_engine::ocr::OcrError> {
            Ok(vec![pdfcraft_ocr::Line {
                words: vec![pdfcraft_ocr::Word::new("word", [1.0, 1.0, 9.0, 9.0], "fake").with_confidence(self.confidence)],
            }])
        }
    }

    /// A batch over two text files (the engine never runs on them: they are skipped files):
    /// the summary counts them in their own bucket, nothing is written, and a cancel before
    /// the first file leaves the rest not run.
    #[test]
    fn batch_counts_skipped_files_and_honours_a_cancel() {
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "Already typed here").expect("fixture");
        let files = vec![("a.pdf".into(), pdf.to_vec()), ("b.pdf".into(), pdf.to_vec())];
        let progress = Mutex::new(BatchProgress { total: files.len(), cancel: true, ..Default::default() });
        let recognizers = Recognizers { primary: Arc::new(Fake { confidence: 95.0 }), secondary: None };
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = writes.clone();
        let msg = run_batch_files(files, OcrSettings { skip_text_files: true, ..Default::default() }, &recognizers, &progress, move |_, _| {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing is written for skipped files, and the cancel came first");
        assert!(msg.contains("cancelled — 2 files were not run"), "{msg}");
        // Without the cancel, the skipped bucket is named and still nothing is written.
        let files = vec![("a.pdf".into(), pdf.to_vec()), ("b.pdf".into(), pdf.to_vec())];
        let progress = Mutex::new(BatchProgress { total: files.len(), ..Default::default() });
        let w = writes.clone();
        let msg = run_batch_files(files, OcrSettings { skip_text_files: true, ..Default::default() }, &recognizers, &progress, move |_, _| {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(msg.contains("; 2 files already had text"), "{msg}");
        assert!(msg.contains("Recognized 0 words in 0 files"), "{msg}");
    }

    /// A batch over scanned files: the words are counted, and a low-confidence reading lands
    /// in the summary as a note with its 1-based sorted pages.
    #[test]
    fn batch_notes_low_confidence_pages() {
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "fake scan").expect("fixture");
        let files = vec![("scan.pdf".into(), pdf.to_vec()), ("clean.pdf".into(), pdf.to_vec())];
        let progress = Mutex::new(BatchProgress { total: files.len(), ..Default::default() });
        let recognizers = Recognizers { primary: Arc::new(Fake { confidence: 40.0 }), secondary: None };
        // The fixtures are text PDFs standing in for scans; the default skip-text-pages option
        // would (correctly) leave them alone, so the batch here reads everything.
        let settings = OcrSettings { skip_text_pages: false, ..Default::default() };
        let msg = run_batch_files(files, settings.clone(), &recognizers, &progress, |_, _| Ok(()));
        assert!(msg.contains("Recognized 2 words in 2 files"), "{msg}");
        assert!(msg.contains("scan.pdf: 1 low-confidence word(s) on pages 1"), "{msg}");
        // The write failing is a per-file failure, named in the summary.
        let files = vec![("scan.pdf".into(), pdf.to_vec())];
        let progress = Mutex::new(BatchProgress { total: files.len(), ..Default::default() });
        let msg = run_batch_files(files, settings, &recognizers, &progress, |_, _| Err("disk full".into()));
        assert!(msg.contains("; failed: scan.pdf: disk full"), "{msg}");
    }

    /// The batch note follows the settings' low-confidence threshold (60 by default), not a
    /// number of its own: a 40% word is flagged under 60 and under 40, and not under 30.
    #[test]
    fn the_batch_note_uses_the_settings_threshold() {
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "fake scan").expect("fixture");
        let recognizers = Recognizers { primary: Arc::new(Fake { confidence: 40.0 }), secondary: None };
        let run = |threshold: f32| {
            let files = vec![("scan.pdf".into(), pdf.to_vec())];
            let progress = Mutex::new(BatchProgress { total: files.len(), ..Default::default() });
            let settings = OcrSettings { skip_text_pages: false, low_confidence_threshold: threshold, ..Default::default() };
            run_batch_files(files, settings, &recognizers, &progress, |_, _| Ok(()))
        };
        assert!(run(OcrSettings::default().low_confidence_threshold).contains("1 low-confidence word(s)"), "the default flags a 40% word");
        assert!(run(40.0).contains("1 low-confidence word(s)"), "at the threshold counts");
        assert!(!run(30.0).contains("low-confidence"), "a lower threshold leaves the word alone: {}", run(30.0));
    }

    /// The remembered output folder survives the preferences round trip; an absent or empty
    /// value keeps the default (ask each time).
    #[test]
    fn the_output_folder_is_persisted_with_the_draft() {
        let mut d = OcrDraft::default();
        assert_eq!(d.output_folder, None, "by default the folder is asked for");
        d.output_folder = Some("out".into());
        let mut r = OcrDraft::default();
        r.apply_pref(&d.to_pref());
        assert_eq!(r.output_folder.as_deref(), Some("out"));
        let mut e = OcrDraft::default();
        e.apply_pref(&serde_json::json!({}));
        assert_eq!(e.output_folder, None, "an absent value keeps the default");
        e.output_folder = Some("kept".into());
        e.apply_pref(&serde_json::json!({ "output_folder": "" }));
        assert_eq!(e.output_folder.as_deref(), Some("kept"), "an empty value is not a folder");
    }

    /// "Automatic" saved without a strategy reads the way Automatic reads by default (the
    /// second engine joins when the first is unsure); an explicit choice always stands.
    #[test]
    fn an_automatic_engine_without_a_saved_strategy_is_confidence_weighted() {
        let mut d = OcrDraft::default();
        d.apply_pref(&serde_json::json!({ "engine": "auto" }));
        assert_eq!((d.engine, d.strategy), (EngineChoice::Auto, MergeStrategy::ConfidenceWeighted));
        let mut named = OcrDraft::default();
        named.apply_pref(&serde_json::json!({ "engine": "tesseract" }));
        assert_eq!(named.strategy, MergeStrategy::PrimaryOnly, "a named engine reads alone");
        let mut explicit = OcrDraft::default();
        explicit.apply_pref(&serde_json::json!({ "engine": "auto", "strategy": "primary" }));
        assert_eq!((explicit.engine, explicit.strategy), (EngineChoice::Auto, MergeStrategy::PrimaryOnly));
    }

    /// Two inputs of one file name (from different folders, or differing only in case) would
    /// write the same output: the batch stops before anything is read or written.
    #[test]
    fn inputs_that_share_a_file_name_are_refused_before_anything_is_read() {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-same-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture");
        app.ocr_draft.output_folder = Some(dir.to_string_lossy().into_owned());
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "scanned words here").expect("fixture");
        app.ocr_files(vec![("scan.pdf".into(), pdf.to_vec()), ("other.pdf".into(), pdf.to_vec()), ("Scan.PDF".into(), pdf.to_vec())]);
        assert!(app.ocr_batch.is_none(), "the batch never starts");
        let toast = app.toast.clone().map(|t| t.0).unwrap_or_default();
        assert!(toast.contains("same name") && toast.contains("Scan.PDF"), "{toast}");
        assert_eq!(std::fs::read_dir(&dir).expect("fixture").count(), 0, "nothing was written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A remembered output folder is used without asking, is remembered again after the run,
    /// and its collision pre-flight still stops the batch before the first page is read.
    #[test]
    fn a_remembered_output_folder_is_used_and_still_preflights_collisions() {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-folder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture");
        std::fs::write(dir.join("a.pdf"), b"would be overwritten").expect("fixture");
        app.ocr_draft.output_folder = Some(dir.to_string_lossy().into_owned());
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "scanned words here").expect("fixture");
        app.ocr_files(vec![("a.pdf".into(), pdf.to_vec()), ("b.pdf".into(), pdf.to_vec())]);
        assert!(app.ocr_batch.is_none(), "the collision stops the batch before anything is read");
        let toast = app.toast.clone().map(|t| t.0).unwrap_or_default();
        assert!(toast.contains("already holds files of these names"), "{toast}");
        assert!(toast.contains("a.pdf"), "the colliding names are in the error: {toast}");
        assert_eq!(app.ocr_draft.output_folder.as_deref(), Some(dir.to_string_lossy().as_ref()), "the folder stays remembered for the next batch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A result that arrives after the document changed (here: a page turned while the worker was
    /// reading; the same applies to a redaction applied meanwhile) is refused with the reason and
    /// writes nothing, so words read before a redaction cannot come back as invisible text.
    #[test]
    fn a_result_that_arrives_after_the_document_changed_is_refused() {
        let mut app = PdfCraftApp::new();
        let pdf = pdfcraft_engine::Session::new().create_from_text("t", "Typed text").expect("fixture");
        app.open_bytes("a.pdf", None, pdf.to_vec()).expect("opens");
        let id = app.active_ids().expect("a document").1;
        let guard = app.session.ocr_guard(id).expect("a guard");
        app.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).expect("an edit while the worker runs");
        let after_edit = app.session.get(id).and_then(|d| d.can_undo().map(str::to_owned));
        let word = pdfcraft_engine::ocr::PlacedWord {
            text: "123-45-6789".into(),
            origin: [40.0, 40.0],
            across: [50.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(95.0),
            source: "fake".into(),
        };
        let page = OcrPage { page: 0, words: vec![word], already_on_page: 0, skipped: None, notes: Vec::new() };
        let progress = Arc::new(Mutex::new(OcrProgress { total: 1, done: 1, result: Some(Ok(vec![page])), cancel: false }));
        app.ocr_run = Some(OcrRun { doc: id, progress, guard, automatic: None, editable: false });
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "the run is over");
        let toast = app.toast.clone().map(|t| t.0).unwrap_or_default();
        assert!(toast.contains("changed since recognition started"), "{toast}");
        assert_eq!(
            app.session.get(id).and_then(|d| d.can_undo().map(str::to_owned)),
            after_edit,
            "nothing was written: the last undoable step is still the edit"
        );
    }

    /// A page's notes survive to the result the user sees: the editable output's
    /// unrenderable-character warning ("N characters had no font … appear as ?") is said in
    /// the run's summary, not dropped. The result is fabricated, so no models are needed.
    #[test]
    fn the_result_summary_names_the_substituted_characters() {
        let mut app = app_with(scan());
        let id = app.active_ids().unwrap().1;
        let word = pdfcraft_engine::ocr::PlacedWord {
            text: "scanned".into(),
            origin: [72.0, 700.0],
            across: [60.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(95.0),
            source: "ocrs".into(),
        };
        let page = pdfcraft_engine::ocr::OcrPage {
            page: 0,
            words: vec![word],
            already_on_page: 0,
            skipped: None,
            notes: vec!["3 characters had no font that could show them and appear as ? in the output".into()],
        };
        let guard = app.session.ocr_guard(id).unwrap();
        let progress = Arc::new(Mutex::new(OcrProgress { total: 1, done: 1, result: Some(Ok(vec![page])), cancel: false }));
        app.ocr_run = Some(OcrRun { doc: id, progress, guard, automatic: None, editable: false });
        app.poll_ocr();
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert!(toast.contains("3 characters had no font"), "{toast}");
    }

    /// The dialog's "Editable text" output is honored: a run that promised an editable document
    /// does not quietly write the invisible searchable layer — its result opens in the OCR
    /// Verify screen, whose Accept produces the editable document. The result is fabricated,
    /// so no models are needed.
    #[test]
    fn an_editable_output_run_opens_the_review_instead_of_applying() {
        let mut app = app_with(scan());
        app.run_inline = true;
        let id = app.active_ids().unwrap().1;
        let word = pdfcraft_engine::ocr::PlacedWord {
            text: "scanned".into(),
            origin: [72.0, 700.0],
            across: [60.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(95.0),
            source: "ocrs".into(),
        };
        let page = OcrPage { page: 0, words: vec![word], already_on_page: 0, skipped: None, notes: Vec::new() };
        let guard = app.session.ocr_guard(id).unwrap();
        let progress = Arc::new(Mutex::new(OcrProgress { total: 1, done: 1, result: Some(Ok(vec![page])), cancel: false }));
        app.ocr_draft.output_mode = pdfcraft_engine::ocr::OutputMode::EditableText;
        app.ocr_run = Some(OcrRun { doc: id, progress, guard, automatic: None, editable: true });
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "the run is over");
        assert_eq!(app.session.get(id).unwrap().can_undo(), None, "no invisible layer was written");
        assert!(app.ocr_verify.open, "the review opens, to accept as an editable document");
        assert_eq!(app.ocr_verify.phase, crate::workflows::ocr_verify::Phase::ReviewReady);
    }
}
