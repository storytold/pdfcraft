//! Scanner acquisition dialog and background scanning workflow:
//! Detects scanners, displays scan options (Color mode, DPI, native UI),
//! runs the scan on a background thread, and creates a new PDF or appends
//! to the active document with optional OCR recognition.

use std::sync::{Arc, Mutex};
use egui::{Align, Layout};
use pdfcraft_platform::scanner::{self, ScanColorMode, ScanError, ScanOptions, ScanResolution, ScannerDevice};
use crate::theme::{self, Tokens};
use crate::{widgets, PdfCraftApp};

/// Where the scanned pages should land.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanTarget {
    #[default]
    NewDocument,
    AppendActive,
}

/// The scan dialog's choices and device state.
#[derive(Clone, Debug)]
pub struct ScanAcquireDraft {
    pub scanners: Vec<ScannerDevice>,
    pub selected_scanner: usize,
    pub options: ScanOptions,
    pub target: ScanTarget,
    pub recognize_text: bool,
    pub status_message: Option<String>,
}

impl Default for ScanAcquireDraft {
    fn default() -> Self {
        Self {
            scanners: Vec::new(),
            selected_scanner: 0,
            options: ScanOptions::default(),
            target: ScanTarget::NewDocument,
            recognize_text: false,
            status_message: None,
        }
    }
}

/// Status of a running background scan.
pub enum ScanState {
    Running,
    Finished(Result<Vec<u8>, ScanError>),
}

/// Progress shared between background scanner thread and UI.
pub struct ScanAcquireProgress {
    pub state: ScanState,
}

/// Active background scan run.
pub struct ScanAcquireRun {
    pub options: ScanOptions,
    pub target: ScanTarget,
    pub recognize_text: bool,
    pub progress: Arc<Mutex<ScanAcquireProgress>>,
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let has_active_doc = app.active_ids().is_some();
    let is_running = app.scan_acquire_run.is_some();
    let d = &mut app.scan_acquire_draft;

    ui.label(egui::RichText::new(tl!("Scan from Scanner")).font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!("Scan paper documents directly into PDF using connected physical or network scanners."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(8.0);

    let group = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
        ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(13.0)));
        egui::Frame::new()
            .fill(t.hover)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(ui);
            });
        ui.add_space(8.0);
    };

    // 1. Scanner Selection
    group(ui, "Scanner", &mut |ui| {
        ui.horizontal(|ui| {
            if d.scanners.is_empty() {
                ui.label(tl!("No scanners detected yet."));
            } else {
                let current_label = d
                    .scanners
                    .get(d.selected_scanner)
                    .map(|s| s.name.as_str())
                    .unwrap_or("Default Scanner");

                egui::ComboBox::from_id_salt("scanner_select")
                    .selected_text(current_label)
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (i, scanner) in d.scanners.iter().enumerate() {
                            ui.selectable_value(&mut d.selected_scanner, i, &scanner.name);
                        }
                    });
            }

            if ui.button(tl!("Detect Scanners")).clicked() {
                if let Ok(found) = scanner::list_scanners() {
                    d.scanners = found;
                    d.selected_scanner = 0;
                    if d.scanners.is_empty() {
                        d.status_message = Some(tl!("No scanners found.").to_string());
                    } else {
                        d.status_message = None;
                    }
                } else {
                    d.status_message = Some(tl!("Could not query scanner devices.").to_string());
                }
            }
        });

        if let Some(msg) = &d.status_message {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(msg).small().color(t.text_muted));
        }
    });

    // 2. Scan Options
    group(ui, "Options", &mut |ui| {
        #[cfg(target_os = "windows")]
        {
            ui.checkbox(&mut d.options.use_native_ui, tl!("Use system scan dialog (Preview & Scan)"));
            ui.add_space(6.0);
        }

        if !d.options.use_native_ui {
            ui.label(egui::RichText::new(tl!("Resolution:")).small().color(t.text_muted));
            ui.horizontal(|ui| {
                widgets::radio_value(ui, &mut d.options.resolution, ScanResolution::Dpi150, tl!("150 DPI"));
                widgets::radio_value(ui, &mut d.options.resolution, ScanResolution::Dpi300, tl!("300 DPI"));
                widgets::radio_value(ui, &mut d.options.resolution, ScanResolution::Dpi600, tl!("600 DPI"));
            });
            ui.add_space(6.0);

            ui.label(egui::RichText::new(tl!("Color Mode:")).small().color(t.text_muted));
            ui.horizontal(|ui| {
                widgets::radio_value(ui, &mut d.options.color_mode, ScanColorMode::Color, tl!("Color"));
                widgets::radio_value(ui, &mut d.options.color_mode, ScanColorMode::Grayscale, tl!("Grayscale"));
                widgets::radio_value(ui, &mut d.options.color_mode, ScanColorMode::BlackAndWhite, tl!("Black & White"));
            });
        }
    });

    // 3. Document destination & OCR
    group(ui, "Destination", &mut |ui| {
        widgets::radio_value(ui, &mut d.target, ScanTarget::NewDocument, tl!("Create new PDF document"));
        if has_active_doc {
            widgets::radio_value(ui, &mut d.target, ScanTarget::AppendActive, tl!("Append page to active document"));
        }
        ui.add_space(6.0);
        ui.checkbox(&mut d.recognize_text, tl!("Recognize text (OCR) after scan"));
    });

    if is_running {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Scanning in progress… Please wait."));
        });
    }

    ui.add_space(12.0);
    let mut go = false;
    let mut cancel = false;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
        if !is_running && widgets::pill_button(ui, tl!("Scan"), true).clicked() {
            go = true;
        }
    });

    (go, cancel)
}

impl PdfCraftApp {
    /// Open the scanner acquisition modal and refresh discovered devices.
    pub fn open_scanner_dialog(&mut self) {
        if self.scan_acquire_draft.scanners.is_empty() {
            if let Ok(devices) = scanner::list_scanners() {
                self.scan_acquire_draft.scanners = devices;
                self.scan_acquire_draft.selected_scanner = 0;
            }
        }
        self.dialog = Some(crate::Dialog::ScanAcquire);
    }

    /// Start scanner acquisition on a background thread.
    pub fn start_scan_acquire(&mut self) {
        if self.scan_acquire_run.is_some() {
            self.notify_tr("A scan is already in progress");
            return;
        }

        let draft = &mut self.scan_acquire_draft;
        let mut options = draft.options.clone();
        if let Some(dev) = draft.scanners.get(draft.selected_scanner) {
            options.device_id = Some(dev.id.clone());
        }

        let target = draft.target;
        let recognize_text = draft.recognize_text;

        let progress = Arc::new(Mutex::new(ScanAcquireProgress {
            state: ScanState::Running,
        }));

        let worker_progress = Arc::clone(&progress);
        let worker_options = options.clone();

        std::thread::spawn(move || {
            let result = scanner::acquire_scan(&worker_options);
            if let Ok(mut lock) = worker_progress.lock() {
                lock.state = ScanState::Finished(result);
            }
        });

        self.scan_acquire_run = Some(ScanAcquireRun {
            options,
            target,
            recognize_text,
            progress,
        });

        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// Check if the background scan completed and import the scanned image into a PDF.
    pub(crate) fn poll_scan_acquire(&mut self) {
        let Some(run) = &self.scan_acquire_run else { return };
        let state = match run.progress.lock() {
            Ok(guard) => match &guard.state {
                ScanState::Running => {
                    if let Some(ctx) = &self.ctx {
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    return;
                }
                ScanState::Finished(res) => res.clone(),
            },
            Err(_) => {
                self.scan_acquire_run = None;
                self.notify_tr("Scan failed: internal lock error");
                return;
            }
        };

        let target = run.target;
        let recognize_text = run.recognize_text;
        let resolution = pdfcraft_engine::ImageResolution::Dpi(run.options.resolution.as_dpi() as f64);
        self.scan_acquire_run = None;

        match state {
            Ok(bytes) => {
                let images = vec![("Scan.png".to_string(), bytes)];
                match target {
                    ScanTarget::NewDocument => {
                        self.create_from_images_with_resolution(images, resolution);
                        if recognize_text {
                            self.start_ocr();
                        }
                    }
                    ScanTarget::AppendActive => {
                        // Append as a new page in the active document
                        if let Some((_, doc_id)) = self.active_ids() {
                            let created = self.session.create_from_images_with_resolution(&images, resolution);
                            match created {
                                Ok(new_doc_id) => {
                                    // Combine pages from new document into the active document
                                    let active_page_count = self.session.get(doc_id).map_or(0, |d| d.info.pages.len());
                                    let _ = self.apply_edit(pdfcraft_engine::Edit::InsertPages {
                                        at: active_page_count,
                                        from_doc: new_doc_id,
                                        pages: vec![0],
                                    });
                                    let _ = self.session.close(new_doc_id);
                                    if recognize_text {
                                        self.start_ocr();
                                    }
                                    self.notify_tr("Scanned page appended to document");
                                }
                                Err(e) => self.notify(e.to_string()),
                            }
                        } else {
                            self.create_from_images_with_resolution(images, resolution);
                        }
                    }
                }
                self.dialog = None;
            }
            Err(ScanError::Cancelled) => {
                self.dialog = None;
            }
            Err(e) => {
                self.notify(format!("{e}"));
            }
        }
    }
}

