//! Scan & OCR ▸ Enhance scanned file: the dialog (pages, background removal level, edge
//! shadows, despeckle) and the background run with its progress card. The scans are cleaned
//! on a worker thread ([`pdfcraft_engine::scan_clean`]); the result is applied as one undoable
//! step when it is done, unless the document changed meanwhile.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout};
use pdfcraft_engine::scan_clean::{HalftoneImage, HalftoneTuning, ScanCleanReport, ScanCleanSettings, ScanCleaned};
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
    /// The pages (0-based) the run was asked for; none means all.
    pub pages: Vec<usize>,
    pub progress: Arc<Mutex<ScanCleanProgress>>,
}

/// What a finished run restored, for the result dialog.
pub struct ScanCleanOutcome {
    pub doc: DocId,
    /// The document's edit generation right after: the sliders undo and redo the run only while
    /// nothing else has changed.
    pub generation: u64,
    /// The pages the run was asked for (none means all): the sliders repeat it on these, so the
    /// other scans it cleaned stay cleaned.
    pub requested: Vec<usize>,
    /// A slider has moved since the pages were last enhanced.
    pub pending: bool,
    /// The pages (0-based, in order) whose halftone scans were restored: the only ones the sliders
    /// ever touch.
    pub pages: Vec<usize>,
    pub halftone: Vec<HalftoneImage>,
    /// How each of those pages is tuned.
    pub tunings: Vec<(usize, HalftoneTuning)>,
    /// The sliders move every restored page, not just the one showing.
    pub all: bool,
    /// The view was taken to the first restored page.
    pub shown: bool,
}

/// The name of the restoring tool, as the result dialog says it.
const TOOL: &str = "Halftone Restore";

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

/// The tuning of `page`, or of the first restored page when `page` has none.
fn tuning_of(o: &ScanCleanOutcome, page: usize) -> HalftoneTuning {
    let find = |p: usize| o.tunings.iter().find(|(q, _)| *q == p).map(|(_, t)| *t);
    find(page).or_else(|| o.pages.first().and_then(|p| find(*p))).unwrap_or_default()
}

/// The result window, shown while [`crate::Dialog::EnhanceScanResult`] is open: what was detected
/// and restored, and sliders that tune the restored pages: the one showing, or all of them. It is
/// a window to move about, not a modal, so the page stays in view and in reach.
pub(crate) fn show_result(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let active = app.active_ids();
    let Some(mut o) = app.scan_clean_result.take().filter(|o| active.map(|(_, id)| id) == Some(o.doc)) else {
        return close_result(app);
    };
    let Some((vi, _)) = active else { return close_result(app) };
    // The first restored page is the one to look at first.
    if !o.shown {
        o.shown = true;
        if let (Some(first), Some(v)) = (o.pages.first(), app.views.get_mut(vi)) {
            v.go_to_page(*first);
        }
    }
    let current = app.views.get(vi).map_or(0, |v| v.current);
    let busy = app.scan_clean_run.is_some();
    let t = Tokens::get(ctx);
    let (mut open, mut let_go, mut next, mut close) = (true, false, false, false);
    let (mut moved, mut dragging) = (false, false);
    egui::Window::new(tl!("Scan enhanced"))
        .id(egui::Id::unique("scan_clean_result_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .min_width(440.0)
        .max_width(440.0)
        .default_pos(egui::pos2(80.0, 110.0))
        .show(ctx, |ui| {
            widgets::begin_dialog(ui);
            // One sentence a format, with all the pages that have it.
            let mut formats: Vec<(&str, Vec<usize>)> = Vec::new();
            for h in &o.halftone {
                match formats.iter_mut().find(|(f, _)| *f == h.format) {
                    Some((_, pages)) => pages.extend(&h.pages),
                    None => formats.push((&h.format, h.pages.clone())),
                }
            }
            for (format, mut pages) in formats {
                pages.sort_unstable();
                pages.dedup();
                let pages: Vec<String> = pages.iter().map(|p| (p + 1).to_string()).collect();
                let text = crate::i18n::fmt(
                    &crate::i18n::trn(
                        crate::i18n::current(),
                        pages.len() as u64,
                        "A {format} image format was detected on page {pages} and was enhanced using the {tool} optimizing tool.",
                        "A {format} image format was detected on pages {pages} and was enhanced using the {tool} optimizing tool.",
                    ),
                    &[("format", format), ("pages", &pages.join(", ")), ("tool", TOOL)],
                );
                ui.label(text);
                ui.add_space(4.0);
            }
            ui.label(
                egui::RichText::new(tl!("Use the sliders below to customize the optimization further, otherwise, click 'Close'."))
                    .color(t.text_muted),
            );
            ui.add_space(8.0);
            // Where the sliders act: the page showing, or every restored page.
            let here = o.pages.contains(&current);
            let (mut one, mut every) = (!o.all, o.all);
            let a = ui.add(widgets::checkbox(&mut one, tl!("Apply to current page"))).changed();
            let b = ui.add(widgets::checkbox(&mut every, tl!("Apply to all"))).changed();
            if b {
                o.all = every;
            } else if a {
                o.all = !one;
            }
            if !o.all && !here {
                ui.label(egui::RichText::new(tl!("This page has no restored scan. Use 'Next page' to go to one.")).small().color(t.text_muted));
            }
            ui.add_space(6.0);
            // Moving a slider only picks the value; the pages are enhanced again once it is let go
            // (at once for a change from the keyboard).
            let mut tuning = tuning_of(&o, current);
            ui.add_enabled_ui(!busy && (o.all || here), |ui| {
                ui.spacing_mut().slider_width = 240.0;
                for (label, hint, value) in [
                    (tl!("Text weight"), tl!("How heavy the strokes are. Raise it if the text looks thin or broken."), &mut tuning.weight),
                    (tl!("Text contrast"), tl!("How dark the ink gets against the paper."), &mut tuning.contrast),
                    (tl!("Background clean-up"), tl!("How much of the paper's speckle is cleared to white."), &mut tuning.cleanup),
                ] {
                    ui.horizontal(|ui| {
                        let name = ui.label(label);
                        let r = ui.add(egui::Slider::new(value, 0..=100).trailing_fill(true)).labelled_by(name.id).on_hover_text(hint);
                        moved |= r.changed();
                        dragging |= r.dragged();
                    });
                }
            });
            if busy {
                ui.label(egui::RichText::new(tl!("Enhancing…")).small().color(t.text_muted));
            }
            if moved {
                let all = o.all;
                for (_, tn) in o.tunings.iter_mut().filter(|(p, _)| all || *p == current) {
                    *tn = tuning;
                }
                o.pending = true;
            }
            if o.pending && !dragging && !busy {
                o.pending = false;
                let_go = true;
            }
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = widgets::pill_button(ui, tl!("Close"), true).clicked();
                // Greyed out at the last restored page, or when there is no other.
                let more = o.pages.iter().any(|p| *p > current);
                next = ui.add_enabled_ui(more && !busy, |ui| widgets::pill_button(ui, tl!("Next page"), false)).inner.clicked();
            });
        });
    let escape = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) && !ctx.egui_wants_keyboard_input();
    if next
        && let Some(p) = o.pages.iter().find(|p| **p > current).copied()
        && let Some(v) = app.views.get_mut(vi)
    {
        v.go_to_page(p);
    }
    if !open || close || escape {
        return close_result(app);
    }
    app.scan_clean_result = Some(o);
    if let_go {
        app.retune_scan_clean();
    }
}

/// Drop the result, and the window with it if it is still the open dialog.
fn close_result(app: &mut PdfCraftApp) {
    app.scan_clean_result = None;
    if app.dialog == Some(crate::Dialog::EnhanceScanResult) {
        app.dialog = None;
    }
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
        let Some((vi, _)) = self.active_ids() else { return };
        let d = &self.scan_clean_draft;
        let pages: Vec<usize> = match d.pages {
            OcrPages::All => Vec::new(),
            OcrPages::Current => self.views.get(vi).map(|v| vec![v.current]).unwrap_or_default(),
            OcrPages::Range => (d.from.saturating_sub(1)..d.to).collect(),
        };
        self.start_scan_clean_on(pages, Vec::new());
    }

    /// Enhance again the pages of the last run (the result dialog's sliders): its enhancement is
    /// undone and done again with the new settings. Nothing happens if the document has changed.
    pub fn retune_scan_clean(&mut self) {
        let Some(o) = self.scan_clean_result.as_ref() else { return };
        let (doc, pages, generation, tunings) = (o.doc, o.requested.clone(), o.generation, o.tunings.clone());
        if self.scan_clean_run.is_some() || self.active_ids().map(|(_, id)| id) != Some(doc) {
            return;
        }
        if self.session.get(doc).map(|d| d.edit_generation()) != Some(generation) {
            self.notify_tr("The document changed since it was enhanced: undo that step, then enhance again to use the sliders");
            return;
        }
        if let Err(e) = self.session.undo(doc) {
            return self.notify(e.to_string());
        }
        for v in self.views.iter_mut().filter(|v| v.id == doc) {
            v.invalidate_content();
        }
        if self.start_scan_clean_on(pages, tunings) {
            return;
        }
        // No new run: the enhancement that was undone for it comes back, and the sliders stay usable.
        if self.session.redo(doc).is_ok() {
            for v in self.views.iter_mut().filter(|v| v.id == doc) {
                v.invalidate_content();
            }
            let generation = self.session.get(doc).map(|d| d.edit_generation());
            if let (Some(o), Some(g)) = (self.scan_clean_result.as_mut(), generation) {
                o.generation = g;
            }
        }
    }

    /// Whether a run started.
    fn start_scan_clean_on(&mut self, pages: Vec<usize>, tunings: Vec<(usize, HalftoneTuning)>) -> bool {
        let Some((_, id)) = self.active_ids() else { return false };
        if self.scan_clean_run.is_some() {
            self.notify_tr("Scanned pages are already being enhanced");
            return false;
        }
        // Both jobs share the progress card and capture the same document state.
        if self.optimize_run.is_some() {
            self.notify_tr("Optimization is already running");
            return false;
        }
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return false;
        }
        let job = match self.session.scan_clean_job(id, &pages, self.scan_clean_draft.settings) {
            Ok(job) => job.with_page_tunings(tunings),
            Err(e) => {
                self.notify(e.to_string());
                return false;
            }
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
        self.scan_clean_run = Some(ScanCleanRun { doc: id, pages, progress });
        self.poll_scan_clean();
        true
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
        let (doc, requested, progress) = (run.doc, run.pages.clone(), run.progress.clone());
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
                // The page rasters, tiles and thumbnails show the old pixels at the same scale.
                for v in self.views.iter_mut().filter(|v| v.id == doc) {
                    v.invalidate_content();
                }
                let msg = summary(&report);
                self.notify(msg);
                // A restored halftone scan opens the result dialog, with the sliders to tune it.
                if report.halftone.is_empty() {
                    self.scan_clean_result = None;
                    if self.dialog == Some(crate::Dialog::EnhanceScanResult) {
                        self.dialog = None;
                    }
                } else if let Some(generation) = self.session.get(doc).map(|d| d.edit_generation()) {
                    let mut pages: Vec<usize> = report.halftone.iter().flat_map(|h| h.pages.iter().copied()).collect();
                    pages.sort_unstable();
                    pages.dedup();
                    // Running again keeps what was tuned; a first run starts every page at the dialog's values.
                    let before = self.scan_clean_result.take().filter(|o| o.doc == doc);
                    let default = self.scan_clean_draft.settings.halftone;
                    let tunings = pages
                        .iter()
                        .map(|p| (*p, before.as_ref().and_then(|o| o.tunings.iter().find(|(q, _)| q == p)).map_or(default, |(_, t)| *t)))
                        .collect();
                    let (all, shown) = before.map_or((false, false), |o| (o.all, o.shown));
                    self.scan_clean_result =
                        Some(ScanCleanOutcome { doc, generation, requested, pending: false, pages, halftone: report.halftone, tunings, all, shown });
                    if self.dialog.is_none() || self.dialog == Some(crate::Dialog::EnhanceScanResult) {
                        self.dialog = Some(crate::Dialog::EnhanceScanResult);
                    }
                }
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
            ..Default::default()
        };
        assert_eq!(summary(&r), "Cleaned up the scans on 2 pages. 1 page is not a scan. 1 image was left as it was (page 4: it is CMYK).");
        assert_eq!(summary(&ScanCleanReport::default()), "No scanned pages were cleaned up.");
    }
}
