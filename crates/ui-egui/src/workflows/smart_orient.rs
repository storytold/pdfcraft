//! Scan & OCR ▸ Smart Page Orient: preview suggested quarter turns, then apply lossless `/Rotate`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::Ui;
use pdfcraft_engine::orient_pages::{OrientSource, OrientSuggestion};

use crate::theme;
use crate::{PdfCraftApp, widgets};

/// One row in the preview table (user may override the suggested turn).
#[derive(Clone, Debug)]
pub struct OrientRow {
    pub suggestion: OrientSuggestion,
    /// Include this page when applying.
    pub include: bool,
    /// Manual override in degrees clockwise (0/90/180/270).
    pub degrees_cw: i32,
}

#[derive(Default)]
pub struct SmartOrientDraft {
    pub rows: Vec<OrientRow>,
    pub ready: bool,
    pub error: Option<String>,
    /// Progress while assessing: (done, total).
    pub progress: (usize, usize),
}

impl SmartOrientDraft {
    pub fn clear(&mut self) {
        self.rows.clear();
        self.ready = false;
        self.error = None;
        self.progress = (0, 0);
    }
}

pub(crate) enum OrientScanMsg {
    Progress(usize, usize),
    Done(Result<Vec<OrientSuggestion>, String>),
}

/// Background assessment for the Smart Page Orient dialog.
pub(crate) struct SmartOrientScan {
    pub doc: pdfcraft_engine::DocId,
    rx: std::sync::mpsc::Receiver<OrientScanMsg>,
    cancel: Arc<AtomicBool>,
}

impl SmartOrientScan {
    pub fn start(
        doc: pdfcraft_engine::DocId,
        bytes: Arc<Vec<u8>>,
        password: Option<String>,
        pages: Vec<usize>,
        ctx: &egui::Context,
    ) -> SmartOrientScan {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cancel_w = Arc::clone(&cancel);
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("smart-page-orient".into()).spawn(move || {
                let mut session = pdfcraft_engine::Session::new();
                let result = match session.open("orient.pdf", None, bytes, password.as_deref()) {
                    Ok(id) => session
                        .assess_orientation(id, &pages, &|| cancel_w.load(Ordering::Relaxed), &|done, total| {
                            let _ = tx.send(OrientScanMsg::Progress(done, total));
                            ctx.request_repaint();
                        })
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(OrientScanMsg::Done(result));
                ctx.request_repaint();
            });
            if let Err(e) = spawned {
                log::warn!("Smart Page Orient scan: {e}");
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (&pages, &ctx, password, bytes);
            let _ = tx.send(OrientScanMsg::Done(Err("Smart Page Orient runs on the desktop build".into())));
        }
        SmartOrientScan { doc, rx, cancel }
    }

    pub fn poll(&self) -> Option<OrientScanMsg> {
        self.rx.try_recv().ok()
    }
}

impl Drop for SmartOrientScan {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Dialog body. Returns `(apply, close)`.
pub fn body(ui: &mut Ui, app: &mut PdfCraftApp) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Smart Page Orient")).font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.label(tl!(
        "Detect each page's upright orientation (0°, 90°, 180°, 270°) with OCR when available, then apply lossless page rotation. Intentionally landscape pages stay landscape when their text already reads upright."
    ));
    ui.add_space(8.0);

    if app.smart_orient_scan.is_none()
        && !app.smart_orient_draft.ready
        && app.smart_orient_draft.error.is_none()
        && let Some((i, id)) = app.active_ids()
        && let Some(doc) = app.session.get(id)
    {
        let pages: Vec<usize> = {
            let v = &app.views[i];
            if v.selected.is_empty() { (0..doc.info.pages.len()).collect() } else { v.selected.iter().copied().collect() }
        };
        app.smart_orient_draft.clear();
        app.smart_orient_scan = Some(SmartOrientScan::start(id, doc.bytes.clone(), doc.password.clone(), pages, ui.ctx()));
    }

    let mut finished: Option<Result<Vec<OrientSuggestion>, String>> = None;
    if let Some(scan) = &app.smart_orient_scan {
        while let Some(msg) = scan.poll() {
            match msg {
                OrientScanMsg::Progress(d, t) => app.smart_orient_draft.progress = (d, t),
                OrientScanMsg::Done(r) => finished = Some(r),
            }
        }
    }
    if let Some(r) = finished {
        app.smart_orient_scan = None;
        match r {
            Ok(suggestions) => {
                app.smart_orient_draft.rows = suggestions
                    .into_iter()
                    .map(|s| {
                        let degrees_cw = s.degrees_cw;
                        let include = degrees_cw != 0 && !s.low_confidence;
                        OrientRow { suggestion: s, include, degrees_cw }
                    })
                    .collect();
                app.smart_orient_draft.ready = true;
            }
            Err(e) => {
                app.smart_orient_draft.error = Some(e);
                app.smart_orient_draft.ready = true;
            }
        }
    }

    if let Some(err) = &app.smart_orient_draft.error {
        ui.colored_label(ui.visuals().error_fg_color, err);
    } else if !app.smart_orient_draft.ready {
        let (d, t) = app.smart_orient_draft.progress;
        if t > 0 {
            ui.label(format!("{} {d}/{t}", tl!("Analysing pages…")));
            ui.add(egui::ProgressBar::new(d as f32 / t.max(1) as f32).desired_width(280.0));
        } else {
            ui.label(tl!("Analysing pages…"));
            ui.spinner();
        }
    } else {
        let need = app.smart_orient_draft.rows.iter().filter(|r| r.degrees_cw != 0).count();
        let low = app.smart_orient_draft.rows.iter().filter(|r| r.suggestion.low_confidence && r.degrees_cw != 0).count();
        ui.label(format!(
            "{} · {}",
            crate::i18n::fmt(tl!("{n} page(s) need a turn"), &[("n", &need.to_string())]),
            crate::i18n::fmt(tl!("{n} low confidence"), &[("n", &low.to_string())])
        ));
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("smart-orient-grid").striped(true).num_columns(5).show(ui, |ui| {
                ui.label(tl!("Apply"));
                ui.label(tl!("Page"));
                ui.label(tl!("Turn"));
                ui.label(tl!("Confidence"));
                ui.label(tl!("Notes"));
                ui.end_row();
                for row in &mut app.smart_orient_draft.rows {
                    ui.add(widgets::checkbox(&mut row.include, ""));
                    ui.label(format!("{}", row.suggestion.page.saturating_add(1)));
                    let label = match row.degrees_cw {
                        90 => "90°",
                        180 => "180°",
                        270 => "270°",
                        _ => "0°",
                    };
                    craft_ui::ComboBox::from_id_salt(format!("orient-{}", row.suggestion.page)).selected_text(label).width(72.0).show_ui(ui, |ui| {
                        for (d, name) in [(0, "0°"), (90, "90°"), (180, "180°"), (270, "270°")] {
                            if ui.selectable_label(row.degrees_cw == d, name).clicked() {
                                row.degrees_cw = d;
                                row.include = d != 0;
                            }
                        }
                    });
                    let pct = (row.suggestion.confidence * 100.0).round() as i32;
                    let src = match row.suggestion.source {
                        OrientSource::Ocr => "OCR",
                        OrientSource::Pixels => "pixels",
                    };
                    ui.label(format!("{pct}% ({src})"));
                    let mut note = row.suggestion.note.clone().unwrap_or_default();
                    if row.suggestion.low_confidence && row.degrees_cw != 0 {
                        if !note.is_empty() {
                            note.push_str(" · ");
                        }
                        note.push_str(tl!("review"));
                    }
                    ui.label(note);
                    ui.end_row();
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button(tl!("Select all corrections")).clicked() {
                for row in &mut app.smart_orient_draft.rows {
                    row.include = row.degrees_cw != 0;
                }
            }
            if ui.button(tl!("Clear selection")).clicked() {
                for row in &mut app.smart_orient_draft.rows {
                    row.include = false;
                }
            }
        });
    }

    ui.add_space(12.0);
    let mut apply = false;
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can = app.smart_orient_draft.ready
                && app.smart_orient_draft.error.is_none()
                && app.smart_orient_draft.rows.iter().any(|r| r.include && r.degrees_cw != 0);
            if ui.add_enabled(can, egui::Button::new(tl!("Apply corrections")).min_size([120.0, 28.0].into())).clicked() {
                apply = true;
                close = true;
            }
            if ui.button(tl!("Cancel")).clicked() {
                close = true;
            }
        });
    });
    (apply, close)
}

impl PdfCraftApp {
    /// Apply the Smart Page Orient draft as one undoable step.
    pub fn apply_smart_orient(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        if let Some(scan) = &self.smart_orient_scan
            && scan.doc != id
        {
            self.notify_tr("The document changed; open Smart Page Orient again");
            self.smart_orient_draft.clear();
            return;
        }
        let suggestions: Vec<OrientSuggestion> = self
            .smart_orient_draft
            .rows
            .iter()
            .filter(|r| r.include && r.degrees_cw != 0)
            .map(|r| {
                let mut s = r.suggestion.clone();
                s.degrees_cw = r.degrees_cw;
                s.low_confidence = false;
                s
            })
            .collect();
        match self.session.apply_orientation(id, &suggestions) {
            Ok(report) => {
                self.notify(format!("{} {}", tl!("Rotated"), crate::i18n::fmt(tl!("{n} page(s)"), &[("n", &report.rotated.to_string())])));
            }
            Err(e) => self.notify(e.to_string()),
        }
        self.smart_orient_draft.clear();
        self.smart_orient_scan = None;
    }
}
