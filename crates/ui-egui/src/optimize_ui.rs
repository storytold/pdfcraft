//! Optimize PDF ▸ Advanced optimization (Acrobat's PDF Optimizer): Images, Discard Objects,
//! Discard User Data and Clean Up panels. The result is saved as a copy, like Reduce File Size.
//!
//! The optimization (this dialog's, or Reduce File Size's fixed choices) runs on a worker thread
//! and shows its progress in a notice with a bar and a Cancel button; the copy is saved once it
//! is done.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout, RichText};
use pdfcraft_engine::optimize::{Compatibility, Compression, ImageSettings, Preset, QUALITIES, Settings};
use pdfcraft_engine::optimizer::{OptimizeStage, Optimized};
use pdfcraft_engine::{DocId, EditError, Hidden, OptimizeReport};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizeTab {
    Images,
    DiscardObjects,
    DiscardUserData,
    CleanUp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OptimizeDraft {
    pub tab: OptimizeTab,
    pub settings: Settings,
    /// Remove Hidden Information categories to discard.
    pub discard: Vec<Hidden>,
    /// "Audit space usage…" was pressed.
    pub audit: bool,
}

impl Default for OptimizeDraft {
    fn default() -> Self {
        Self { tab: OptimizeTab::Images, settings: Settings::default(), discard: Vec::new(), audit: false }
    }
}

/// Progress of a background optimization: the stage, the result once finished, and a cancel
/// request.
#[derive(Default)]
pub struct OptimizeProgress {
    pub stage: OptimizeStage,
    pub result: Option<Result<Optimized, EditError>>,
    pub cancel: bool,
}

/// Which command started an optimization: they save under different names, and only the
/// Optimizer's notice lists what it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizeKind {
    /// Reduce File Size (Acrobat's defaults).
    Reduce,
    /// Optimize PDF ▸ Advanced optimization (the dialog's choices).
    Advanced,
}

impl OptimizeKind {
    /// The suffix of the saved copy's name ("notes (reduced).pdf").
    pub(crate) fn suffix(self) -> &'static str {
        match self {
            OptimizeKind::Reduce => "reduced",
            OptimizeKind::Advanced => "optimized",
        }
    }
}

pub struct OptimizeRun {
    pub doc: DocId,
    pub kind: OptimizeKind,
    pub progress: Arc<Mutex<OptimizeProgress>>,
}

/// The notice's text for a stage.
fn stage_label(stage: OptimizeStage) -> String {
    match stage {
        OptimizeStage::Discarding => tl!("Optimizing… removing user data").to_string(),
        OptimizeStage::Fragments => tl!("Optimizing… merging image fragments").to_string(),
        OptimizeStage::Images { done, total } => crate::i18n::fmt(
            tl!("Optimizing… image {d} of {t}"),
            &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
        ),
        OptimizeStage::CleaningUp => tl!("Optimizing… cleaning up").to_string(),
        OptimizeStage::Merging => tl!("Optimizing… merging identical objects").to_string(),
        OptimizeStage::Writing => tl!("Optimizing… writing the copy").to_string(),
    }
}

/// What the saved notice adds after the size: images optimized and items discarded.
fn summary(r: &OptimizeReport) -> String {
    let o = &r.optimize;
    let lang = crate::i18n::current();
    let mut parts = Vec::new();
    let images = o.images_resampled + o.images_recompressed;
    if images > 0 {
        parts.push(crate::i18n::trn(lang, images as u64, "{n} image optimized", "{n} images optimized"));
    }
    let discarded: usize = r.discarded.iter().map(|(_, n)| n).sum();
    if discarded > 0 {
        parts.push(crate::i18n::trn(lang, discarded as u64, "{n} item discarded", "{n} items discarded"));
    }
    if parts.is_empty() { String::new() } else { format!("; {}", parts.join(", ")) }
}

fn image_row(ui: &mut egui::Ui, id: &str, title: &str, s: &mut ImageSettings) {
    ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut s.downsample, tl!("Bicubic downsampling to")));
        ui.add_enabled(s.downsample, egui::DragValue::new(&mut s.target_ppi).range(9.0..=2400.0).suffix(" ppi"));
        ui.label(tl!("for images above"));
        ui.add_enabled(s.downsample, egui::DragValue::new(&mut s.above_ppi).range(9.0..=2400.0).suffix(" ppi"));
    });
    s.above_ppi = s.above_ppi.max(s.target_ppi);
    ui.horizontal(|ui| {
        ui.label(tl!("Compression"));
        let label = match s.compression {
            Compression::Jpeg(_) => "JPEG",
            Compression::Flate => tl!("ZIP"),
            Compression::Retain => tl!("Retain existing"),
        };
        egui::ComboBox::from_id_salt((id, "compression")).selected_text(label).show_ui(ui, |ui| {
            let q = match s.compression {
                Compression::Jpeg(q) => q,
                _ => 60,
            };
            ui.selectable_value(&mut s.compression, Compression::Jpeg(q), "JPEG");
            ui.selectable_value(&mut s.compression, Compression::Flate, tl!("ZIP"));
            ui.selectable_value(&mut s.compression, Compression::Retain, tl!("Retain existing"));
        });
        if let Compression::Jpeg(q) = &mut s.compression {
            ui.label(tl!("Quality"));
            let name = QUALITIES.iter().min_by_key(|(_, v)| (*v as i32 - *q as i32).abs()).map_or(tl!("Medium"), |(n, _)| tl!(n));
            egui::ComboBox::from_id_salt((id, "quality")).selected_text(name).show_ui(ui, |ui| {
                for (n, v) in QUALITIES {
                    ui.selectable_value(q, v, tl!(n));
                }
            });
        }
    });
    ui.add_space(8.0);
}

fn discard_box(ui: &mut egui::Ui, list: &mut Vec<Hidden>, h: Hidden, label: &str) {
    let mut on = list.contains(&h);
    if ui.add(widgets::checkbox(&mut on, tl!(label))).changed() {
        if on {
            list.push(h);
        } else {
            list.retain(|x| *x != h);
        }
    }
}

/// Draw the dialog; returns (ok, cancel).
pub(crate) fn body(ui: &mut egui::Ui, d: &mut OptimizeDraft, t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("PDF Optimizer")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(tl!("Settings"));
        let name = |p: Preset| match p {
            Preset::Standard => tl!("Standard"),
            Preset::Mobile => tl!("Mobile"),
        };
        let current = Preset::of(&d.settings);
        egui::ComboBox::from_id_salt("optimize-preset").selected_text(current.map_or(tl!("Custom"), name)).show_ui(ui, |ui| {
            for p in Preset::ALL {
                if ui.selectable_label(current == Some(p), name(p)).clicked() {
                    d.settings = p.settings();
                }
            }
        });
        ui.add_space(8.0);
        ui.label(tl!("Compatibility"));
        let compat_label = |c: Compatibility| match c {
            Compatibility::Retain => tl!("Retain existing").to_string(),
            _ => format!("PDF {}", c.name()),
        };
        egui::ComboBox::from_id_salt("optimize-compatibility").selected_text(compat_label(d.settings.compatibility)).show_ui(ui, |ui| {
            for c in Compatibility::ALL {
                ui.selectable_value(&mut d.settings.compatibility, c, compat_label(c));
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for (tab, label) in [
            (OptimizeTab::Images, tl!("Images")),
            (OptimizeTab::DiscardObjects, tl!("Discard Objects")),
            (OptimizeTab::DiscardUserData, tl!("Discard User Data")),
            (OptimizeTab::CleanUp, tl!("Clean Up")),
        ] {
            if widgets::mode_tab(ui, label, d.tab == tab).clicked() {
                d.tab = tab;
            }
        }
    });
    ui.separator();
    ui.add_space(6.0);
    let s = &mut d.settings;
    match d.tab {
        OptimizeTab::Images => {
            ui.add(widgets::checkbox(&mut s.merge_fragments, tl!("Merge image fragments")));
            image_row(ui, "color", "Color Images", &mut s.color);
            image_row(ui, "gray", "Grayscale Images", &mut s.gray);
            ui.label(
                egui::RichText::new(tl!("Each image is measured where pages draw it; an image is replaced only if the result is smaller."))
                    .small()
                    .color(t.text_muted),
            );
        }
        OptimizeTab::DiscardObjects => {
            discard_box(ui, &mut d.discard, Hidden::LinksActionsScripts, "Discard all links, actions and JavaScript");
            ui.add(widgets::checkbox(&mut s.discard_alternate_images, tl!("Discard alternate images")));
            ui.add(widgets::checkbox(&mut s.discard_thumbnails, tl!("Discard embedded page thumbnails")));
            ui.add(widgets::checkbox(&mut s.discard_tags, tl!("Discard document tags")));
            ui.add(widgets::checkbox(&mut s.discard_print_settings, tl!("Discard embedded print settings")));
            discard_box(ui, &mut d.discard, Hidden::Bookmarks, "Discard bookmarks");
            discard_box(ui, &mut d.discard, Hidden::FormFields, "Flatten form fields");
            discard_box(ui, &mut d.discard, Hidden::HiddenLayers, "Discard hidden layer content");
        }
        OptimizeTab::DiscardUserData => {
            discard_box(ui, &mut d.discard, Hidden::Comments, "Discard all comments, forms and multimedia");
            discard_box(ui, &mut d.discard, Hidden::Metadata, "Discard document information and metadata");
            discard_box(ui, &mut d.discard, Hidden::Attachments, "Discard all object data (file attachments)");
            discard_box(ui, &mut d.discard, Hidden::PrivateData, "Discard private data of other applications");
            discard_box(ui, &mut d.discard, Hidden::HiddenText, "Discard hidden text");
        }
        OptimizeTab::CleanUp => {
            ui.add(widgets::checkbox(&mut s.flate_unencoded, tl!("Use Flate to encode streams that are not encoded")));
            ui.add(widgets::checkbox(&mut s.lzw_to_flate, tl!("In streams that use LZW encoding, use Flate instead")));
            ui.add(widgets::checkbox(&mut s.remove_invalid_links, tl!("Remove invalid links and bookmarks")));
            ui.add(widgets::checkbox(&mut s.remove_unreferenced_dests, tl!("Remove unreferenced named destinations")));
            let mut object_streams = s.effective_object_streams();
            if ui
                .add_enabled(
                    s.compatibility != Compatibility::Pdf1_4,
                    widgets::checkbox(&mut object_streams, tl!("Compress document structure (object streams)")),
                )
                .changed()
            {
                s.object_streams = object_streams;
            }
            ui.add_enabled(false, widgets::checkbox(&mut true, tl!("Remove unused objects and merge identical ones")));
        }
    }
    ui.add_space(12.0);
    let (mut ok, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Audit space usage…"), false).clicked() {
            d.audit = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                ok = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    (ok, cancel)
}

/// Audit Space Usage: how the file's bytes are spent now, by kind of content (not what
/// optimizing would save).
pub(crate) fn audit_body(ui: &mut egui::Ui, rows: &[pdfcraft_engine::optimize::SpaceUse], t: &Tokens) -> bool {
    ui.label(RichText::new(tl!("Space Audit")).font(crate::theme::semibold(18.0)));
    ui.add_space(4.0);
    audit_table(ui, rows, t);
    ui.add_space(12.0);
    let mut ok = false;
    ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), |ui| ok = widgets::pill_button(ui, tl!("OK"), true).clicked()));
    ok
}

/// The audit's explanation and its table: bytes and share of the file per kind of content.
/// Shared by the Optimizer's Audit Space Usage and Properties ▸ Space Audit.
pub(crate) fn audit_table(ui: &mut egui::Ui, rows: &[pdfcraft_engine::optimize::SpaceUse], t: &Tokens) {
    use egui::{TextWrapMode, WidgetText};
    ui.label(RichText::new(tl!("What takes up the space in this file, by kind of content.")).color(t.text_muted));
    ui.add_space(8.0);
    // Every cell is laid out first so each column can be exactly as wide as its widest cell. A
    // right-to-left layout in a grid's last column takes the rest of the window instead, which
    // drew the percentages at the dialog's far edge, away from their rows.
    let text = |s: RichText| WidgetText::from(s).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
    let header = |s: &str| text(RichText::new(s).color(t.text_muted));
    let mut table = vec![[header(tl!("Description")), header(tl!("Bytes")), header(tl!("Percentage"))]];
    for r in rows.iter().filter(|r| r.bytes > 0) {
        // "Patterns" also names the redaction search patterns ("模式"); the audit's PDF
        // graphics objects need their own key.
        let category = if r.category == pdfcraft_engine::optimize::SpaceCategory::Patterns {
            tl!("Patterns (graphics objects)")
        } else {
            tl!(r.category.label())
        };
        table.push([text(RichText::new(category)), text(RichText::new(r.bytes.to_string())), text(RichText::new(format!("{:.2}%", r.percent)))]);
    }
    let total: u64 = rows.iter().map(|r| r.bytes).sum();
    table.push([
        text(RichText::new(tl!("Total")).strong()),
        text(RichText::new(total.to_string()).strong()),
        text(RichText::new("100.00%").strong()),
    ]);
    let gap = 24.0;
    let width = |column: usize| table.iter().filter_map(|row| row.get(column)).map(|g| g.size().x).fold(0.0, f32::max);
    // The description takes what the numbers leave, so the striped rows span the dialog.
    let numbers = width(1) + width(2) + 2.0 * gap;
    let widths = [width(0).max(ui.available_width() - numbers), width(1), width(2)];
    let height = table.iter().flatten().map(|g| g.size().y).fold(ui.spacing().interact_size.y, f32::max);
    // No minimum column width: egui's default widened a narrow Bytes column past `widths` and
    // pushed the table beyond the dialog's padding.
    egui::Grid::new("space-audit").num_columns(3).striped(true).spacing([gap, 4.0]).min_col_width(0.0).show(ui, |ui| {
        for row in table {
            for (column, (cell, width)) in row.into_iter().zip(widths).enumerate() {
                // Numbers line up on their last digit.
                let layout = if column == 0 { Layout::left_to_right(Align::Center) } else { Layout::right_to_left(Align::Center) };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
                ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout)).label(cell);
            }
            ui.end_row();
        }
    });
}

impl PdfCraftApp {
    /// Measure the active document's space use, unless `space_audit` already describes it as
    /// it is now. With no document the audit is empty.
    pub(crate) fn refresh_space_audit(&mut self) {
        let Some((_, id)) = self.active_ids() else {
            self.space_audit.clear();
            self.space_audit_of = None;
            return;
        };
        let Some(doc) = self.session.get(id) else { return };
        let key = (id, doc.edit_generation());
        if self.space_audit_of != Some(key) {
            self.space_audit = doc.audit_space();
            self.space_audit_of = Some(key);
        }
    }

    /// Optimize PDF with the dialog's choices on a worker thread; the copy is saved when done.
    pub fn optimize_with_draft(&mut self) {
        let (settings, discard) = (self.optimize_draft.settings.clone(), self.optimize_draft.discard.clone());
        self.start_optimize(OptimizeKind::Advanced, &settings, &discard);
    }

    /// Run an optimization of the active document on a worker thread (inline in tests and on
    /// the web), showing its progress; the copy is saved when it is done.
    pub(crate) fn start_optimize(&mut self, kind: OptimizeKind, settings: &Settings, discard: &[Hidden]) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        if self.optimize_run.is_some() {
            self.notify_tr("Optimization is already running");
            return;
        }
        let job = match self.session.optimize_job(id, settings, discard) {
            Ok(job) => job,
            Err(e) => return self.save_optimized(id, kind, Err(e)),
        };
        let progress = Arc::new(Mutex::new(OptimizeProgress::default()));
        let p = progress.clone();
        let work = move || {
            let result = job.run(|stage| {
                let Ok(mut s) = p.lock() else { return false };
                s.stage = stage;
                !s.cancel
            });
            if let Ok(mut s) = p.lock() {
                s.result = Some(result);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else if let Err(e) = std::thread::Builder::new().name("pdfcraft-optimize".into()).spawn(work) {
            // No worker: report the failure instead of waiting for a result that never comes.
            if let Ok(mut s) = progress.lock() {
                s.result = Some(Err(EditError::Optimize(e.to_string())));
            }
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.optimize_run = Some(OptimizeRun { doc: id, kind, progress });
        self.poll_optimize();
    }

    /// Stop a running optimization (nothing is saved).
    pub fn cancel_optimize(&mut self) {
        if let Some(r) = &self.optimize_run
            && let Ok(mut s) = r.progress.lock()
        {
            s.cancel = true;
        }
    }

    /// Show progress; save the copy once the worker is done.
    pub(crate) fn poll_optimize(&mut self) {
        let Some(run) = self.optimize_run.as_ref() else { return };
        let (doc, kind, progress) = (run.doc, run.kind, run.progress.clone());
        let Ok(mut s) = progress.lock() else {
            // The worker panicked while holding the lock: it will never report back.
            self.optimize_run = None;
            self.progress_notice = None;
            self.notify_tr("Couldn't optimize the file");
            return;
        };
        let Some(result) = s.result.take() else {
            let cancelling = s.cancel;
            let stage = s.stage;
            drop(s);
            let label = if cancelling { tl!("Cancelling…").to_string() } else { stage_label(stage) };
            self.progress_notice = Some(crate::widgets::ProgressNotice { label, fraction: stage.fraction(), cancellable: !cancelling });
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        };
        drop(s);
        self.optimize_run = None;
        self.progress_notice = None;
        match result {
            Err(EditError::Cancelled) => self.notify_tr("Optimization cancelled"),
            result => {
                let result = result.map(|(b, r)| {
                    let detail = if kind == OptimizeKind::Advanced { summary(&r) } else { String::new() };
                    (b, detail)
                });
                self.save_optimized(doc, kind, result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn the_progress_card_shows_the_stage_and_cancels() {
        let progress = Arc::new(Mutex::new(OptimizeProgress { stage: OptimizeStage::Images { done: 3, total: 10 }, ..Default::default() }));
        let p = progress.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("notes.txt", None, b"hello".to_vec()).unwrap();
            let (_, doc) = app.active_ids().unwrap();
            app.optimize_run = Some(OptimizeRun { doc, kind: OptimizeKind::Advanced, progress: p });
            app.notify("Saved notes.pdf");
            app
        });
        // 1.5 s (kittest steps a quarter second): the bar has eased in, the toast still shows.
        h.run_steps(6);
        if let Ok(path) = std::env::var("PDFCRAFT_OPTIMIZE_PROGRESS_SHOT") {
            h.render().unwrap().save(path).unwrap();
        }
        h.get_by_label("Optimizing… image 4 of 10");
        let shown = h.state().progress_notice.clone().unwrap();
        assert!((shown.fraction - 0.275).abs() < 1e-4, "{shown:?}");
        assert!(shown.cancellable);

        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(progress.lock().unwrap().cancel, "the worker is asked to stop");
        h.get_by_label("Cancelling…");
        assert!(h.query_by_label("Cancel").is_none(), "Cancel is not offered twice");

        // The worker stops and reports back.
        progress.lock().unwrap().result = Some(Err(EditError::Cancelled));
        h.run_steps(2);
        assert!(h.state().optimize_run.is_none() && h.state().progress_notice.is_none());
        assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("Optimization cancelled"));
    }

    #[test]
    fn a_second_run_is_refused_while_one_is_going() {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("notes.txt", None, b"hello".to_vec()).unwrap();
        let (_, doc) = app.active_ids().unwrap();
        app.optimize_run = Some(OptimizeRun { doc, kind: OptimizeKind::Reduce, progress: Arc::default() });
        app.optimize_with_draft();
        assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Optimization is already running"));
    }
}
