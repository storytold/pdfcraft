//! Scan & OCR ▸ From scanner (64-bit Windows TWAIN 2.x).
//!
//! Lists installed 64-bit TWAIN scanners (such as Kyocera KDSTWAIN), lets the user choose
//! resolution (default 600 DPI), color mode, paper source, native scanner dialog toggle, and
//! destination (new PDF or append to current PDF), then acquires pages on a dedicated STA worker
//! thread so the egui UI stays responsive.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use egui::{Align, Layout};
use pdfcraft_engine::{DocId, ImageResolution};
use pdfcraft_twain::{PaperSource, PixelMode, ScanBatch, ScanRequest, TwainDevice, TwainError};

use crate::PdfCraftApp;
use crate::notices::{self, ProgressKey, ProgressNotice};

const DPI_CHOICES: &[u16] = &[150, 200, 300, 400, 600, 1200];

/// Where scanned pages should be placed once acquired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanDestination {
    NewPdf,
    AppendCurrent,
}

/// Choices kept between scan runs.
#[derive(Clone, Debug)]
pub struct TwainScanDraft {
    pub open: bool,
    pub devices: Vec<TwainDevice>,
    pub selected: Option<String>,
    pub discovery_error: Option<String>,
    pub dpi: u16,
    pub pixel_mode: PixelMode,
    pub source: PaperSource,
    pub show_ui: bool,
    pub destination: ScanDestination,
    pub run_ocr_after: bool,
}

impl Default for TwainScanDraft {
    fn default() -> Self {
        Self {
            open: false,
            devices: Vec::new(),
            selected: None,
            discovery_error: None,
            dpi: 600,
            pixel_mode: PixelMode::Color,
            source: PaperSource::DriverDefault,
            show_ui: true,
            destination: ScanDestination::NewPdf,
            run_ocr_after: false,
        }
    }
}

/// Result of a background TWAIN scan worker.
pub struct TwainScanRun {
    pub destination: ScanDestination,
    pub target_doc: Option<DocId>,
    pub fallback_dpi: f64,
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Result<ScanBatch, TwainError>>,
}

/// Result of a background TWAIN device discovery worker.
pub struct TwainDiscoveryRun {
    pub rx: mpsc::Receiver<Result<Vec<TwainDevice>, TwainError>>,
}

/// Clean up redundant driver suffixes like ` (1)` or `(1)` for display when no other discovered
/// device shares the base name; the exact raw TWAIN `ProductName` is still passed to `TWAINDSM.dll`.
fn display_device_name(name: &str, all: &[TwainDevice]) -> String {
    let trimmed = name.strip_suffix(" (1)").or_else(|| name.strip_suffix("(1)")).map(str::trim_end);
    if let Some(base) = trimmed
        && !base.is_empty()
        && all.iter().filter(|d| d.name.starts_with(base)).count() == 1
    {
        return base.to_string();
    }
    name.to_string()
}

pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if !app.twain_scan_draft.open {
        return;
    }
    let mut open = true;
    let mut action = (false, false, false);
    egui::Window::new(tl!("Scan from scanner"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .fixed_size(egui::vec2(460.0, 0.0))
        .show(ui.ctx(), |ui| {
            action = body(ui, app);
        });
    let (go, cancel, refresh) = action;
    if !open || cancel || go {
        app.twain_scan_draft.open = false;
    }
    if refresh {
        app.start_twain_discovery();
    }
    if go {
        app.start_twain_scan();
    }
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> (bool, bool, bool) {
    let can_append =
        app.active_ids().and_then(|(_, id)| app.session.get(id)).is_some_and(|doc| doc.allows_assembly() && doc.read_only_reason.is_none());
    let scanning = app.twain_scan_run.is_some();
    let discovering = app.twain_discovery_run.is_some();
    let d = &mut app.twain_scan_draft;
    if !can_append && d.destination == ScanDestination::AppendCurrent {
        d.destination = ScanDestination::NewPdf;
    }

    ui.label(egui::RichText::new(tl!("Scan from scanner")).size(18.0).strong());
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Scans pages through your 64-bit TWAIN scanner driver into a new PDF or the active document.")).small().weak());
    ui.add_space(8.0);

    let mut refresh = false;
    let group_fill = ui.visuals().faint_bg_color;
    let group = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
        ui.label(egui::RichText::new(tl!(title)).size(13.0).strong());
        egui::Frame::new().fill(group_fill).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
        ui.add_space(8.0);
    };

    group(ui, "Scanner", &mut |ui| {
        ui.horizontal(|ui| {
            let raw_selected = d.selected.as_deref().or_else(|| d.devices.first().map(|dev| dev.name.as_str()));
            let current_label = raw_selected
                .map(|name| display_device_name(name, &d.devices))
                .unwrap_or_else(|| if discovering { tl!("Searching for scanners…").to_string() } else { tl!("No scanner selected").to_string() });
            let combo_w = (ui.available_width() - 120.0).clamp(160.0, 260.0);
            egui::ComboBox::from_id_salt("twain_scanner_select").width(combo_w).selected_text(current_label).show_ui(ui, |ui| {
                for dev in &d.devices {
                    let sel = d.selected.as_deref() == Some(dev.name.as_str());
                    let label = display_device_name(&dev.name, &d.devices);
                    if ui.selectable_label(sel, label).clicked() {
                        d.selected = Some(dev.name.clone());
                    }
                }
            });
            if ui.add_enabled(!discovering && !scanning, egui::Button::new(tl!("Refresh"))).clicked() {
                refresh = true;
            }
        });
        if let Some(err) = &d.discovery_error {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(err).small().color(ui.visuals().error_fg_color));
        }
    });

    group(ui, "Scanner settings", &mut |ui| {
        ui.horizontal(|ui| {
            ui.label(tl!("Resolution:"));
            egui::ComboBox::from_id_salt("twain_dpi_select").width(120.0).selected_text(format!("{} DPI", d.dpi)).show_ui(ui, |ui| {
                for &dpi in DPI_CHOICES {
                    ui.selectable_value(&mut d.dpi, dpi, format!("{dpi} DPI"));
                }
            });
            ui.add_space(12.0);
            ui.label(tl!("Color:"));
            let mode_text = match d.pixel_mode {
                PixelMode::Color => tl!("Color"),
                PixelMode::Grayscale => tl!("Grayscale"),
                PixelMode::BlackAndWhite => tl!("Black and white"),
            };
            egui::ComboBox::from_id_salt("twain_color_select").width(140.0).selected_text(mode_text).show_ui(ui, |ui| {
                ui.selectable_value(&mut d.pixel_mode, PixelMode::Color, tl!("Color"));
                ui.selectable_value(&mut d.pixel_mode, PixelMode::Grayscale, tl!("Grayscale"));
                ui.selectable_value(&mut d.pixel_mode, PixelMode::BlackAndWhite, tl!("Black and white"));
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(tl!("Source:"));
            let src_text = match d.source {
                PaperSource::DriverDefault => tl!("Scanner default"),
                PaperSource::Flatbed => tl!("Bed"),
                PaperSource::Feeder => tl!("Feeder"),
                PaperSource::FeederDuplex => tl!("Feeder (duplex)"),
            };
            egui::ComboBox::from_id_salt("twain_source_select").width(200.0).selected_text(src_text).show_ui(ui, |ui| {
                ui.selectable_value(&mut d.source, PaperSource::DriverDefault, tl!("Scanner default"));
                ui.selectable_value(&mut d.source, PaperSource::Flatbed, tl!("Bed"));
                ui.selectable_value(&mut d.source, PaperSource::Feeder, tl!("Feeder"));
                ui.selectable_value(&mut d.source, PaperSource::FeederDuplex, tl!("Feeder (duplex)"));
            });
        });
        ui.add_space(6.0);
        ui.checkbox(&mut d.show_ui, tl!("Show scanner driver dialog"));
    });

    group(ui, "Destination", &mut |ui| {
        ui.radio_value(&mut d.destination, ScanDestination::NewPdf, tl!("Create new PDF"));
        ui.add_enabled_ui(can_append, |ui| {
            ui.radio_value(&mut d.destination, ScanDestination::AppendCurrent, tl!("Append to current PDF"));
        });
    });

    let mut go = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let can_scan = !scanning && !discovering && (!d.devices.is_empty() || d.discovery_error.is_none());
            let btn_label = if scanning { tl!("Scanning…") } else { tl!("Scan") };
            if ui.add_enabled(can_scan, egui::Button::new(btn_label)).clicked() {
                go = true;
            }
            if ui.button(tl!("Cancel")).clicked() {
                cancel = true;
            }
        });
    });

    (go, cancel, refresh)
}

impl PdfCraftApp {
    /// Open the TWAIN scanner dialog and refresh available 64-bit scanners.
    pub fn open_twain_scan_dialog(&mut self) {
        self.twain_scan_draft.open = true;
        self.start_twain_discovery();
    }

    /// Query installed 64-bit TWAIN scanners on a background thread.
    pub fn start_twain_discovery(&mut self) {
        if self.twain_discovery_run.is_some() || self.twain_scan_run.is_some() {
            return;
        }
        self.twain_scan_draft.discovery_error = None;
        let (tx, rx) = mpsc::channel();
        let wake = self.wake.clone();
        let work = move || {
            let _ = tx.send(pdfcraft_twain::list_devices());
            wake.wake();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name("pdfcraft-twain-list".into()).spawn(work).is_err() {
            self.twain_scan_draft.discovery_error = Some("could not start scanner discovery thread".into());
            return;
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.twain_discovery_run = Some(TwainDiscoveryRun { rx });
    }

    /// Start acquiring pages from the selected TWAIN scanner on a dedicated STA thread.
    pub fn start_twain_scan(&mut self) {
        if self.twain_scan_run.is_some() {
            return;
        }
        let d = &self.twain_scan_draft;
        let req = ScanRequest {
            device_name: d.selected.clone().or_else(|| d.devices.first().map(|dev| dev.name.clone())),
            dpi: d.dpi,
            pixel_mode: d.pixel_mode,
            source: d.source,
            show_ui: d.show_ui,
        };
        let target_doc = self.active_ids().map(|(_, id)| id);
        let destination = if target_doc.is_some() { d.destination } else { ScanDestination::NewPdf };
        let fallback_dpi = f64::from(d.dpi.max(72));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        let wake = self.wake.clone();
        let work = move || {
            let _ = tx.send(pdfcraft_twain::acquire(&req, &worker_cancel));
            wake.wake();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name("pdfcraft-twain-scan".into()).spawn(work).is_err() {
            self.notify("could not start scanner thread");
            return;
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.twain_scan_run = Some(TwainScanRun { destination, target_doc, fallback_dpi, cancel, rx });
    }

    /// Signal an active TWAIN scan to stop.
    pub fn cancel_twain_scan(&mut self) {
        if let Some(run) = &self.twain_scan_run {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Poll TWAIN device discovery and page acquisition workers each frame.
    pub(crate) fn poll_twain_scan(&mut self, ctx: &egui::Context) {
        if let Some(run) = self.twain_discovery_run.as_ref() {
            match run.rx.try_recv() {
                Ok(res) => {
                    self.twain_discovery_run = None;
                    match res {
                        Ok(devs) => {
                            self.twain_scan_draft.discovery_error =
                                if devs.is_empty() { Some(tl!("No 64-bit TWAIN scanners were found.").to_string()) } else { None };
                            if self.twain_scan_draft.selected.as_ref().is_none_or(|sel| !devs.iter().any(|x| &x.name == sel)) {
                                self.twain_scan_draft.selected = devs.first().map(|x| x.name.clone());
                            }
                            self.twain_scan_draft.devices = devs;
                        }
                        Err(e) => {
                            self.twain_scan_draft.devices.clear();
                            self.twain_scan_draft.discovery_error = Some(e.to_string());
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.twain_discovery_run = None;
                    self.twain_scan_draft.discovery_error = Some("scanner discovery stopped unexpectedly".into());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(80));
                }
            }
        }

        let Some(run) = self.twain_scan_run.as_ref() else {
            return;
        };
        match run.rx.try_recv() {
            Ok(res) => {
                let destination = run.destination;
                let target_doc = run.target_doc;
                let fallback_dpi = run.fallback_dpi;
                self.twain_scan_run = None;
                notices::end_progress(self, ProgressKey::TwainScan);
                match res {
                    Ok(batch) => self.finish_twain_scan(ctx, batch, destination, target_doc, fallback_dpi),
                    Err(TwainError::Cancelled) => self.notify_tr("Scanning was stopped"),
                    Err(e) => self.notify(e.to_string()),
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.twain_scan_run = None;
                notices::end_progress(self, ProgressKey::TwainScan);
                self.notify_tr("Scanning was stopped");
            }
            Err(mpsc::TryRecvError::Empty) => {
                let cancelling = run.cancel.load(Ordering::Relaxed);
                notices::set_progress(
                    self,
                    ProgressKey::TwainScan,
                    ProgressNotice {
                        label: if cancelling { tl!("Cancelling…").to_string() } else { tl!("Scanning…").to_string() },
                        fraction: 0.0,
                        cancellable: !cancelling,
                    },
                );
                ctx.request_repaint_after(std::time::Duration::from_millis(80));
            }
        }
    }

    fn finish_twain_scan(
        &mut self,
        ctx: &egui::Context,
        batch: ScanBatch,
        destination: ScanDestination,
        target_doc: Option<DocId>,
        fallback_dpi: f64,
    ) {
        let incomplete_reason = batch.incomplete_reason;
        let pages = batch.pages;
        let count = pages.len();
        let dpi = pages.first().map(|p| p.dpi_x).filter(|d| d.is_finite() && (1.0..=1200.0).contains(d)).unwrap_or(fallback_dpi.clamp(1.0, 1200.0));
        let images: Vec<(String, Vec<u8>)> = pages.into_iter().enumerate().map(|(i, p)| (format!("scan-{}.bmp", i + 1), p.bmp_bytes)).collect();
        let bytes = match self.session.create_from_images_with_resolution(&images, ImageResolution::Dpi(dpi)) {
            Ok(b) => b,
            Err(e) => return self.notify(e.to_string()),
        };

        let open_new = |app: &mut Self, bytes: Arc<Vec<u8>>, suffix: Option<&str>| {
            if let Err(e) = app.open_bytes(ctx, "Scan.pdf", None, (*bytes).clone()) {
                app.notify(e);
            } else {
                let mut msg = crate::i18n::trn(crate::i18n::current(), count as u64, "Scanned {n} page", "Scanned {n} pages");
                if let Some(reason) = suffix {
                    msg = format!("{msg} ({reason})");
                }
                app.notify(msg);
            }
        };

        match destination {
            ScanDestination::NewPdf => open_new(self, bytes, incomplete_reason.as_deref()),
            ScanDestination::AppendCurrent => {
                let Some(doc_id) =
                    target_doc.filter(|id| self.session.get(*id).is_some_and(|doc| doc.allows_assembly() && doc.read_only_reason.is_none()))
                else {
                    open_new(self, bytes, incomplete_reason.as_deref());
                    return;
                };
                if let Some(idx) = self.views.iter().position(|v| v.id == doc_id) {
                    self.active = Some(idx);
                }
                let at = self.session.get(doc_id).map_or(0, |d| d.info.pages.len());
                if self.apply_edit(
                    ctx,
                    pdfcraft_engine::Edit::InsertPagesFrom {
                        name: "Scan.pdf".into(),
                        bytes: bytes.clone(),
                        pages: None,
                        at,
                        field_conflicts: pdfcraft_engine::FieldConflicts::Rename,
                    },
                ) {
                    let mut msg = crate::i18n::trn(crate::i18n::current(), count as u64, "Appended {n} scanned page", "Appended {n} scanned pages");
                    if let Some(reason) = incomplete_reason.as_deref() {
                        msg = format!("{msg} ({reason})");
                    }
                    self.notify(msg);
                } else {
                    // Keep the scanned pages by opening them as a new document if the target refused the edit.
                    open_new(self, bytes, incomplete_reason.as_deref());
                }
            }
        }
    }
}
