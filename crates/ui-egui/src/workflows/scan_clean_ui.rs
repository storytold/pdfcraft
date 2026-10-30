//! Scan & OCR ▸ Enhance scanned file: the dialog (pages, background removal level, edge
//! shadows, despeckle) and the background run with its progress card. The scans are cleaned
//! on a worker thread ([`pdfcraft_engine::scan_clean`]); the result is applied as one undoable
//! step when it is done, unless the document changed meanwhile.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout};
use pdfcraft_engine::scan_clean::{ScanCleanReport, ScanCleanSettings, ScanCleaned};
use pdfcraft_engine::{DocId, EditError};

use crate::ocr_ui::OcrPages;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The dialog's choices (kept between runs).
#[derive(Clone, Debug, PartialEq)]
pub struct ScanCleanDraft {
    pub pages: OcrPages,
    pub from: usize,
    pub to: usize,
    pub settings: ScanCleanSettings,
}

impl Default for ScanCleanDraft {
    fn default() -> Self {
        ScanCleanDraft { pages: OcrPages::All, from: 1, to: 1, settings: ScanCleanSettings::default() }
    }
}

/// Progress of a run: images done, total, the result once finished, and a cancel request.
#[derive(Default)]
pub struct ScanCleanProgress {
    pub done: usize,
    pub total: usize,
    pub result: Option<Result<ScanCleaned, EditError>>,
    pub cancel: bool,
}

pub struct ScanCleanRun {
    pub doc: DocId,
    pub progress: Arc<Mutex<ScanCleanProgress>>,
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len().max(1));
    let d = &mut app.scan_clean_draft;
    d.to = d.to.clamp(1, pages);
    d.from = d.from.clamp(1, d.to);
    ui.label(egui::RichText::new(tl!("Enhance Scanned File")).font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!("Cleans up the scanned pictures in place: recognized text, comments and links on the pages stay as they are."))
            .small()
            .color(t.text_muted),
    );
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
            let from = widgets::radio_value(ui, &mut d.pages, OcrPages::Range, tl!("From"));
            let on = d.pages == OcrPages::Range;
            ui.add_enabled(on, egui::DragValue::new(&mut d.from).range(1..=pages)).labelled_by(from.id);
            let to = ui.label(tl!("to"));
            ui.add_enabled(on, egui::DragValue::new(&mut d.to).range(1..=pages)).labelled_by(to.id);
        });
    });
    group(ui, "Filters", &mut |ui| {
        ui.horizontal(|ui| {
            let label = ui.label(tl!("Background removal"));
            ui.spacing_mut().slider_width = 220.0;
            ui.add(egui::Slider::new(&mut d.settings.level, 0..=pdfcraft_engine::camera_clean::MAX_LEVEL).trailing_fill(true))
                .labelled_by(label.id)
                .on_hover_text(tl!("Evens out the paper so a grey or yellowed background turns white, and darkens faint strokes. 0 is off."));
        });
        ui.add(widgets::checkbox(&mut d.settings.edge_shadows, tl!("Edge shadow removal")))
            .on_hover_text(tl!("Lightens shadows along the edges of the scan, such as a book's gutter or a copier's dark border"));
        ui.add(widgets::checkbox(&mut d.settings.despeckle, tl!("Despeckle")))
            .on_hover_text(tl!("Removes isolated dark specks such as dust and scanner noise. The only filter black and white scans take."));
    });
    ui.add_space(6.0);
    let (mut go, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ready = d.settings.changes_something();
            if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Enhance"), true)).inner.clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (go, cancel)
}

/// What a finished run says: what was cleaned, what was not a scan, and what was left and why.
fn summary(r: &ScanCleanReport) -> String {
    let lang = crate::i18n::current();
    let mut out = if r.pages.is_empty() {
        tl!("No scanned pages were cleaned up.").to_string()
    } else {
        crate::i18n::trn(lang, r.pages.len() as u64, "Cleaned up the scan on {n} page.", "Cleaned up the scans on {n} pages.")
    };
    if !r.not_scanned.is_empty() {
        out.push(' ');
        out.push_str(&crate::i18n::trn(lang, r.not_scanned.len() as u64, "{n} page is not a scan.", "{n} pages are not scans."));
    }
    if let Some(first) = r.skipped.first() {
        out.push(' ');
        let page = first.pages.first().map_or_else(String::new, |p| (p + 1).to_string());
        // The reason is the engine's own sentence, in English.
        out.push_str(&crate::i18n::fmt(
            &crate::i18n::trn(
                lang,
                r.skipped.len() as u64,
                "{n} image was left as it was (page {p}: {why}).",
                "{n} images were left as they were (page {p}: {why}, …).",
            ),
            &[("p", &page), ("why", &first.reason)],
        ));
    }
    out
}

impl PdfCraftApp {
    /// Clean up the scans on the pages chosen in the dialog, in the background.
    pub fn start_scan_clean(&mut self) {
        let Some((vi, id)) = self.active_ids() else { return };
        if self.scan_clean_run.is_some() {
            self.notify_tr("Scanned pages are already being enhanced");
            return;
        }
        // Both jobs share the progress card and capture the same document state.
        if self.optimize_run.is_some() {
            self.notify_tr("Optimization is already running");
            return;
        }
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let d = &self.scan_clean_draft;
        let pages: Vec<usize> = match d.pages {
            OcrPages::All => Vec::new(),
            OcrPages::Current => self.views.get(vi).map(|v| vec![v.current]).unwrap_or_default(),
            OcrPages::Range => (d.from.saturating_sub(1)..d.to).collect(),
        };
        let job = match self.session.scan_clean_job(id, &pages, d.settings) {
            Ok(job) => job,
            Err(e) => return self.notify(e.to_string()),
        };
        let progress = Arc::new(Mutex::new(ScanCleanProgress { total: job.total(), ..Default::default() }));
        let p = progress.clone();
        let work = move || {
            let result = job.run(|done, total| {
                let Ok(mut s) = p.lock() else { return false };
                s.done = done;
                s.total = total;
                !s.cancel
            });
            if let Ok(mut s) = p.lock() {
                s.result = Some(result);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else if let Err(e) = std::thread::Builder::new().name("pdfcraft-scan-clean".into()).spawn(work) {
            // No worker: report the failure instead of waiting for a result that never comes.
            if let Ok(mut s) = progress.lock() {
                s.result = Some(Err(EditError::Invalid(e.to_string())));
            }
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.scan_clean_run = Some(ScanCleanRun { doc: id, progress });
        self.poll_scan_clean();
    }

    /// Stop a running clean-up (nothing changes).
    pub fn cancel_scan_clean(&mut self) {
        if let Some(r) = &self.scan_clean_run
            && let Ok(mut s) = r.progress.lock()
        {
            s.cancel = true;
        }
    }

    /// Show progress; apply the result once the worker is done.
    pub(crate) fn poll_scan_clean(&mut self) {
        let Some(run) = self.scan_clean_run.as_ref() else { return };
        let (doc, progress) = (run.doc, run.progress.clone());
        let Ok(mut s) = progress.lock() else {
            // The worker panicked while holding the lock: it will never report back.
            self.scan_clean_run = None;
            self.progress_notice = None;
            self.notify_tr("Couldn't enhance the scanned pages");
            return;
        };
        let Some(result) = s.result.take() else {
            let (cancelling, done, total) = (s.cancel, s.done, s.total);
            drop(s);
            let label = if cancelling {
                tl!("Cancelling…").to_string()
            } else {
                crate::i18n::fmt(
                    tl!("Enhancing scans… image {d} of {t}"),
                    &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
                )
            };
            let fraction = if total == 0 { 1.0 } else { done.min(total) as f32 / total as f32 };
            self.progress_notice = Some(crate::widgets::ProgressNotice { label, fraction, cancellable: !cancelling });
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        };
        drop(s);
        self.scan_clean_run = None;
        self.progress_notice = None;
        match result.and_then(|cleaned| self.session.apply_scan_clean(doc, cleaned)) {
            Ok(report) => {
                let msg = summary(&report);
                self.notify(msg);
            }
            Err(EditError::Cancelled) => self.notify_tr("Enhancing the scanned pages was cancelled"),
            Err(e) => self.notify(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_engine::scan_clean::SkippedImage;

    #[test]
    fn the_summary_says_what_was_done_and_what_was_left() {
        let r = ScanCleanReport {
            pages: vec![0, 2],
            images: 1,
            not_scanned: vec![1],
            skipped: vec![SkippedImage { pages: vec![3], name: "Im0".into(), reason: "it is CMYK".into() }],
        };
        assert_eq!(summary(&r), "Cleaned up the scans on 2 pages. 1 page is not a scan. 1 image was left as it was (page 4: it is CMYK).");
        assert_eq!(summary(&ScanCleanReport::default()), "No scanned pages were cleaned up.");
    }
}
