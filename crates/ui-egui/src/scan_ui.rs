//! File ▸ Create ▸ PDF from Scanner (issue #312): choose a scanner and a preset, scan one or
//! more pages in the background, and open them as a new, unsaved PDF (optionally searchable).
//!
//! Finding scanners and scanning happen on worker threads; the dialog polls them each frame.
//! The scanning itself lives in `pdfcraft-scan`; this file is only the dialog and its state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Align, Layout};
use pdfcraft_engine::scan::{self, ColorMode, Paper, Preset, ScanError, ScanSettings, ScannedPage, Scanner, Source};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// How long a search waits for network scanners to announce themselves.
const SEARCH_WAIT: Duration = Duration::from_secs(3);
/// The resolutions the dialog offers (the scanner uses its closest).
const DPIS: [u32; 8] = [75, 100, 150, 200, 300, 400, 600, 1200];

type Shared<T> = Arc<Mutex<Option<T>>>;

/// A scan in progress.
pub struct ScanJob {
    result: Shared<Result<Vec<ScannedPage>, ScanError>>,
    cancel: Arc<AtomicBool>,
}

/// The dialog's state.
#[derive(Default)]
pub struct ScanUi {
    pub scanners: Vec<Scanner>,
    /// The id of the chosen scanner.
    pub selected: Option<String>,
    /// A network scanner typed by address.
    pub address: String,
    pub settings: ScanSettings,
    /// Ask for more pages after each flatbed scan.
    pub prompt_more: bool,
    /// Make the new PDF searchable once it is made.
    pub ocr: bool,
    /// Pages scanned so far, kept until the PDF is created.
    pub pages: Vec<ScannedPage>,
    /// Why the last search or scan failed.
    pub error: Option<String>,
    finding: Option<Shared<Vec<Scanner>>>,
    job: Option<ScanJob>,
}

impl ScanUi {
    pub fn is_scanning(&self) -> bool {
        self.job.is_some()
    }

    pub fn is_searching(&self) -> bool {
        self.finding.is_some()
    }
}

/// What the dialog asks the app to do.
#[derive(Default)]
pub(crate) struct ScanAction {
    pub scan: bool,
    pub search: bool,
    pub add_address: bool,
    pub finish: bool,
    pub stop: bool,
    pub close: bool,
}

fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, enabled: bool, value: &mut T, choices: &[(T, String)]) {
    let shown = choices.iter().find(|c| c.0 == *value).map_or(String::new(), |c| c.1.clone());
    ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt(id).selected_text(shown).width(260.0).show_ui(ui, |ui| {
            for (v, label) in choices {
                ui.selectable_value(value, *v, label);
            }
        });
    });
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> ScanAction {
    let ocr_available = pdfcraft_engine::ocr::available();
    let mut act = ScanAction::default();
    let Some(s) = app.scan_ui.as_mut() else {
        act.close = true;
        return act;
    };
    let busy = s.is_scanning();
    let started = !s.pages.is_empty();
    ui.label(egui::RichText::new(tl!("Create PDF from Scanner")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    egui::Grid::new("scan-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Scanner"));
        ui.horizontal(|ui| {
            let shown = s.selected.as_ref().and_then(|id| s.scanners.iter().find(|x| &x.id == id)).map_or_else(
                || if s.is_searching() { tl!("Searching…").to_string() } else { tl!("No scanner found").to_string() },
                |x| x.name.clone(),
            );
            ui.add_enabled_ui(!busy && !started, |ui| {
                egui::ComboBox::from_id_salt("scan-device").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                    for x in &s.scanners {
                        ui.selectable_value(&mut s.selected, Some(x.id.clone()), format!("{} ({})", x.name, tl!(x.backend.label())));
                    }
                });
            });
            if ui.add_enabled(!busy && !started && !s.is_searching(), egui::Button::new(tl!("Refresh"))).clicked() {
                act.search = true;
            }
        });
        ui.end_row();
        ui.label(tl!("Network address"));
        ui.horizontal(|ui| {
            ui.add_enabled(
                !busy && !started,
                egui::TextEdit::singleline(&mut s.address).hint_text(tl!("e.g. 192.168.1.20")).desired_width(180.0).background_color(t.field),
            );
            if ui.add_enabled(!busy && !started && !s.address.trim().is_empty(), egui::Button::new(tl!("Add"))).clicked() {
                act.add_address = true;
            }
        });
        ui.end_row();
        let open = !busy && !started;
        ui.label(tl!("Preset"));
        let mut preset = Preset::matching(&s.settings);
        let before = preset;
        ui.add_enabled_ui(open, |ui| {
            let shown = preset.map_or_else(|| tl!("Custom").to_string(), |p| tl!(p.label()).to_string());
            egui::ComboBox::from_id_salt("scan-preset").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                for p in Preset::ALL {
                    ui.selectable_value(&mut preset, Some(p), tl!(p.label()));
                }
            });
        });
        if preset != before
            && let Some(p) = preset
        {
            let keep = s.settings;
            s.settings = ScanSettings { source: keep.source, paper: keep.paper, ..p.settings() };
        }
        ui.end_row();
        ui.label(tl!("Color"));
        let modes = [ColorMode::BlackWhite, ColorMode::Gray, ColorMode::Color].map(|m| (m, tl!(m.label()).to_string()));
        combo(ui, "scan-color", open, &mut s.settings.color, &modes);
        ui.end_row();
        ui.label(tl!("Resolution"));
        let dpis: Vec<(u32, String)> = DPIS.iter().map(|d| (*d, format!("{d} dpi"))).collect();
        combo(ui, "scan-dpi", open, &mut s.settings.dpi, &dpis);
        ui.end_row();
        ui.label(tl!("Source"));
        let sources = [
            (Source::Flatbed, tl!("Flatbed").to_string()),
            (Source::Feeder, tl!("Document feeder").to_string()),
            (Source::FeederDuplex, tl!("Document feeder (both sides)").to_string()),
        ];
        combo(ui, "scan-source", open, &mut s.settings.source, &sources);
        ui.end_row();
        ui.label(tl!("Paper size"));
        let papers: Vec<(Paper, String)> = Paper::ALL.iter().map(|p| (*p, tl!(p.label()).to_string())).collect();
        combo(ui, "scan-paper", open, &mut s.settings.paper, &papers);
        ui.end_row();
    });
    ui.add_space(8.0);
    ui.add_enabled(!busy && !s.settings.source.is_feeder(), egui::Checkbox::new(&mut s.prompt_more, tl!("Ask to scan more pages")));
    ui.add_enabled(!busy && ocr_available, egui::Checkbox::new(&mut s.ocr, tl!("Make searchable (recognize text)")));
    if !ocr_available {
        ui.label(
            egui::RichText::new(tl!("Text recognition isn't installed: its models are missing (run `cargo xtask models`, or set PDFCRAFT_MODELS)."))
                .small()
                .color(t.text_muted),
        );
    }
    ui.add_space(6.0);
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Scanning…"));
        });
    } else if started {
        let n = s.pages.len().to_string();
        ui.label(if s.pages.len() == 1 { tl!("1 page scanned").to_string() } else { crate::i18n::fmt(tl!("{n} pages scanned"), &[("n", &n)]) });
    }
    if let Some(e) = &s.error {
        ui.label(egui::RichText::new(e.as_str()).color(t.text_muted));
    }
    ui.add_space(8.0);
    let can_scan = !busy && s.selected.is_some();
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if busy {
                if widgets::pill_button(ui, tl!("Stop"), false).clicked() {
                    act.stop = true;
                }
                return;
            }
            if started {
                if widgets::pill_button(ui, tl!("Create PDF"), true).clicked() {
                    act.finish = true;
                }
                if ui.add_enabled_ui(can_scan, |ui| widgets::pill_button(ui, tl!("Scan next page"), false)).inner.clicked() {
                    act.scan = true;
                }
            } else if ui.add_enabled_ui(can_scan, |ui| widgets::pill_button(ui, tl!("Scan"), true)).inner.clicked() {
                act.scan = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                act.close = true;
            }
        })
    });
    act
}

impl PdfCraftApp {
    /// Create ▸ PDF from Scanner: open the dialog and look for scanners.
    pub(crate) fn open_scan_dialog(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            self.notify_tr("Scanning isn't available in the web version");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.scan_ui.is_none() {
                self.scan_ui = Some(ScanUi::default());
            }
            self.dialog = Some(crate::Dialog::Scanner);
            self.search_scanners();
        }
    }

    /// Look for scanners in the background (network scanners announce themselves for a few
    /// seconds); the dialog fills in when the search is done.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn search_scanners(&mut self) {
        let inline = self.run_inline;
        let Some(s) = self.scan_ui.as_mut() else { return };
        if s.finding.is_some() {
            return;
        }
        let slot: Shared<Vec<Scanner>> = Arc::new(Mutex::new(None));
        s.finding = Some(slot.clone());
        let work = move || {
            let found = scan::scanners(SEARCH_WAIT);
            if let Ok(mut r) = slot.lock() {
                *r = Some(found);
            }
        };
        if inline {
            work();
        } else if std::thread::Builder::new().name("pdfcraft-find-scanners".into()).spawn(work).is_err()
            && let Some(s) = self.scan_ui.as_mut()
        {
            s.finding = None;
        }
        self.poll_scan();
    }

    #[cfg(target_arch = "wasm32")]
    pub fn search_scanners(&mut self) {}

    /// Use the scanner at the typed address (an `escl:` id).
    pub fn add_scanner_address(&mut self) {
        let Some(s) = self.scan_ui.as_mut() else { return };
        let typed = s.address.trim().to_string();
        match scan::escl::normalize_base(&typed) {
            Some(base) => {
                let id = format!("{}{base}", scan::Backend::Escl.prefix());
                if !s.scanners.iter().any(|x| x.id == id) {
                    s.scanners.push(Scanner { id: id.clone(), name: typed, backend: scan::Backend::Escl });
                }
                s.selected = Some(id);
                s.address.clear();
                s.error = None;
            }
            None => s.error = Some(tl!("That isn't a scanner address").to_string()),
        }
    }

    /// Scan one batch with the chosen scanner, in the background.
    pub fn start_scan(&mut self) {
        let inline = self.run_inline;
        let Some(s) = self.scan_ui.as_mut() else { return };
        let Some(id) = s.selected.clone() else { return };
        if s.job.is_some() {
            return;
        }
        s.error = None;
        let settings = s.settings;
        let (result, cancel): (Shared<Result<Vec<ScannedPage>, ScanError>>, _) = (Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
        s.job = Some(ScanJob { result: result.clone(), cancel: cancel.clone() });
        let work = move || {
            let r = scan::scan(&id, &settings, &cancel);
            if let Ok(mut slot) = result.lock() {
                *slot = Some(r);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if inline {
            work();
        } else if std::thread::Builder::new().name("pdfcraft-scan".into()).spawn(work).is_err()
            && let Some(s) = self.scan_ui.as_mut()
        {
            s.job = None;
            s.error = Some(tl!("Couldn't start scanning").to_string());
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (inline, work);
        }
        self.poll_scan();
    }

    /// Ask the running scan to stop; pages already scanned are kept.
    pub fn stop_scan(&mut self) {
        if let Some(j) = self.scan_ui.as_ref().and_then(|s| s.job.as_ref()) {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Collect the results of finished searches and scans.
    pub(crate) fn poll_scan(&mut self) {
        let Some(s) = self.scan_ui.as_mut() else { return };
        let found = s.finding.as_ref().and_then(|f| f.lock().ok().and_then(|mut r| r.take()));
        if let Some(found) = found {
            s.finding = None;
            let keep: Vec<Scanner> = s
                .scanners
                .iter()
                .filter(|x| !found.iter().any(|f| f.id == x.id) && x.backend == scan::Backend::Escl && s.selected.as_ref() == Some(&x.id))
                .cloned()
                .collect();
            s.scanners = found;
            s.scanners.extend(keep);
            if s.selected.as_ref().is_none_or(|id| !s.scanners.iter().any(|x| &x.id == id)) {
                s.selected = s.scanners.first().map(|x| x.id.clone());
            }
            if s.scanners.is_empty() {
                s.error = Some(tl!("No scanners were found. Check that the scanner is on and connected, or enter its network address.").to_string());
            } else {
                s.error = None;
            }
        }
        let done = s.job.as_ref().and_then(|j| j.result.lock().ok().and_then(|mut r| r.take()));
        let mut finish = false;
        if let Some(result) = done {
            s.job = None;
            let more = s.prompt_more && !s.settings.source.is_feeder();
            match result {
                Ok(pages) => {
                    s.pages.extend(pages);
                    s.pages.truncate(scan::MAX_PAGES);
                    finish = !more;
                }
                // Stopped or failed after some pages: keep them and let the user decide.
                Err(ScanError::Cancelled) => s.error = Some(tl!("Scanning was stopped").to_string()),
                Err(e) => s.error = Some(e.to_string()),
            }
        }
        if (s.is_scanning() || s.is_searching())
            && let Some(ctx) = &self.ctx
        {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if finish {
            self.finish_scan();
        }
    }

    /// Make the new document from the pages scanned so far and close the dialog.
    pub fn finish_scan(&mut self) {
        let Some(s) = self.scan_ui.as_mut() else { return };
        if s.pages.is_empty() {
            return;
        }
        let (pages, ocr) = (std::mem::take(&mut s.pages), s.ocr);
        let created = self.session.create_from_scan(&pages, "Scan").map_err(|e| e.to_string());
        match self.open_created_bytes("Scan.pdf", created) {
            Ok(()) => {
                self.dialog = None;
                if ocr && pdfcraft_engine::ocr::available() {
                    self.ocr_draft.pages = crate::ocr_ui::OcrPages::All;
                    self.start_ocr();
                }
            }
            Err(e) => {
                if let Some(s) = self.scan_ui.as_mut() {
                    s.pages = pages;
                    s.error = Some(crate::i18n::fmt(tl!("Couldn't create a PDF: {e}"), &[("e", &e)]));
                }
            }
        }
    }

    pub(crate) fn dismiss_scan_dialog(&mut self) {
        if let Some(s) = self.scan_ui.as_mut() {
            if let Some(j) = &s.job {
                j.cancel.store(true, Ordering::Relaxed);
            }
            // The pages are dropped with the dialog; the choices stay for next time.
            s.pages.clear();
            s.job = None;
            s.error = None;
        }
        self.dialog = None;
    }
}
