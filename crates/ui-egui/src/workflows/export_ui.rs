//! Export a PDF ▸ Image (PNG, JPEG, TIFF) and Text (Acrobat's Export a PDF tool, first formats).
//!
//! On the desktop the export runs on a worker thread and reports progress in the notice bar;
//! on the web it runs in place and downloads the files.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use egui::{Align, Layout};
use pdfcraft_engine::{
    DocId,
    export::{ExportSource, Exporter, ImageFormat, SizeEstimate},
};

use crate::marks_ui::PageRange;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Image,
    Text,
    /// Export all images: the images pages use, as files.
    AllImages,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportDraft {
    pub dpi: f64,
    pub range: PageRange,
    pub format: ImageFormat,
    /// Export all images ▸ skip images with fewer pixels than this on their shorter side.
    pub min_side: u32,
}

impl Default for ExportDraft {
    fn default() -> Self {
        Self { dpi: 150.0, range: PageRange::default(), format: ImageFormat::Png, min_side: 0 }
    }
}

/// Progress of a background export: (done, total, final message once finished).
pub type ExportStatus = Arc<Mutex<Option<(usize, usize, Option<String>)>>>;

/// What the size estimate was made for: a new document state, page range, resolution or format
/// starts a new one.
#[derive(Clone, PartialEq, Eq)]
struct SizeKey {
    /// The document whose pages are measured.
    doc: DocId,
    /// The document's edit generation, so an undo or an edit measures again.
    generation: u64,
    /// The 0-based pages the export would write.
    pages: Vec<usize>,
    /// The resolution's bits, so the key compares exactly without a float.
    dpi: u64,
    /// The file format (and JPEG quality) the pages would be written in.
    format: ImageFormat,
}

/// One running size estimate: the settings it was started for, its worker's result, and the flag
/// that stops it.
struct SizeJob {
    /// The settings this estimate was started for; a result is only kept while it still matches.
    key: SizeKey,
    /// The worker's result, once it has one.
    receiver: mpsc::Receiver<Result<SizeEstimate, String>>,
    /// Set when this estimate is no longer wanted (a new key, or the dialog closed); the worker
    /// reads it before starting and between the pages it draws.
    cancel: Arc<AtomicBool>,
}

impl SizeJob {
    /// Whether its worker has exited: a worker drops its sender when it returns, which the
    /// receiver reports as disconnected (a worker that finished normally sends first).
    fn finished(&self) -> bool {
        matches!(self.receiver.try_recv(), Ok(_) | Err(mpsc::TryRecvError::Disconnected))
    }
}

impl Drop for SizeJob {
    /// Dropping the job is cancellation: nothing will read what it measures.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The export dialog's size estimate: at most one worker running, and no stale result after the
/// settings, the document or the page range change.
#[derive(Default)]
pub(crate) struct SizeDraft {
    key: Option<SizeKey>,
    /// The key the previous frame had. A job starts only once the settings settle, so dragging the
    /// quality slider does not start a worker per frame.
    settled: Option<SizeKey>,
    job: Option<SizeJob>,
    /// Cancelled estimates whose worker has not exited yet. A render cannot be interrupted, so a
    /// new worker waits for these: closing and reopening the dialog then cannot stack renders.
    retiring: Vec<SizeJob>,
    result: Option<Result<SizeEstimate, String>>,
}

impl SizeDraft {
    /// Stop measuring and forget the last estimate (the dialog closed, or another document is up).
    pub(crate) fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
            self.retiring.push(job);
        }
        self.key = None;
        self.settled = None;
        self.result = None;
        self.prune();
    }

    /// Forget the cancelled workers that have exited.
    fn prune(&mut self) {
        self.retiring.retain(|job| !job.finished());
    }

    /// Keep `key`'s estimate up to date: start a worker once the settings have settled, drop the
    /// result of a job whose key has changed, and take the finished estimate when it arrives.
    fn update(&mut self, key: SizeKey, source: impl FnOnce() -> ExportSource, ctx: &egui::Context, inline: bool) {
        self.prune();
        if self.key.as_ref() != Some(&key) {
            if let Some(job) = self.job.take() {
                job.cancel.store(true, Ordering::Relaxed);
                self.retiring.push(job);
            }
            self.key = Some(key.clone());
            self.settled = None;
            self.result = None;
        }
        if let Some(job) = &self.job {
            let received = match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("the size estimate stopped unexpectedly".to_string())),
            };
            if let Some(result) = received {
                if job.key == key && !job.cancel.load(Ordering::Relaxed) {
                    self.result = Some(result);
                }
                self.job = None;
            }
        }
        if self.job.is_some() || self.result.is_some() {
            return;
        }
        if !self.retiring.is_empty() {
            // A cancelled estimate is still winding down. Start nothing until it exits, and keep
            // the frame coming so this notices when it does.
            ctx.request_repaint();
            return;
        }
        if self.settled.as_ref() != Some(&key) {
            self.settled = Some(key);
            // The settling frame starts no worker, so ask for the next frame itself: the estimate
            // must not wait for the user to move the mouse.
            ctx.request_repaint();
            return;
        }
        let source = source();
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (pages, dpi, format) = (key.pages.clone(), f64::from_bits(key.dpi), key.format);
        self.job = Some(SizeJob { key, receiver, cancel: cancel.clone() });
        let ctx = ctx.clone();
        let work = move || {
            // The dialog may have closed, or the settings changed, while this worker waited to
            // start: there is nothing to measure for any more.
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let result = Exporter::from_source(source).estimate_cancellable(&pages, dpi, format, &cancel);
            let _ = sender.send(result);
            ctx.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if !inline {
            if let Err(error) = std::thread::Builder::new().name("export-size".into()).spawn(work) {
                self.job = None;
                self.result = Some(Err(error.to_string()));
            }
            return;
        }
        let _ = inline;
        work();
    }
}

/// An estimated size in the unit that keeps it short: an estimate is approximate, so it keeps one
/// decimal at most.
fn approx_size(bytes: u64) -> String {
    let (mb, kb) = (bytes as f64 / (1u64 << 20) as f64, bytes as f64 / (1u64 << 10) as f64);
    if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else if kb >= 10.0 {
        format!("{kb:.0} KB")
    } else if kb >= 1.0 {
        format!("{kb:.1} KB")
    } else {
        crate::panels::byte_count(bytes as usize)
    }
}

/// The size the files will take, that it is being measured, or why it is unknown.
fn size_line(ui: &mut egui::Ui, size: &SizeDraft, t: &Tokens) {
    match &size.result {
        Some(Ok(estimate)) => {
            let text = crate::i18n::fmt(tl!("Estimated size: about {size}"), &[("size", &approx_size(estimate.total()))]);
            ui.label(egui::RichText::new(text).color(t.text_muted))
                .on_hover_text(tl!("Measured on a sample of the pages at this resolution and format; the files may differ."));
        }
        Some(Err(error)) => {
            let text = crate::i18n::fmt(tl!("No size estimate: {e}"), &[("e", error)]);
            ui.label(egui::RichText::new(text).small().color(t.text_faint));
        }
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new(tl!("Estimating size…")).color(t.text_faint));
            });
        }
    }
}

/// The Export dialog's body: the format and resolution (and page range) controls, the estimate of
/// what the files will take, and the Export button. Returns (export clicked, cancel clicked).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, kind: ExportKind) -> (bool, bool) {
    let active = app.active_ids();
    let count = active.and_then(|(_, id)| app.session.get(id)).map(|d| d.info.pages.len()).unwrap_or(0);
    let d = &mut app.export_draft;
    ui.label(
        egui::RichText::new(match kind {
            ExportKind::Image => tl!("Export to Image"),
            ExportKind::Text => tl!("Export to Text"),
            ExportKind::AllImages => tl!("Export All Images"),
        })
        .font(theme::semibold(18.0)),
    );
    ui.add_space(8.0);
    if kind == ExportKind::Image {
        ui.horizontal(|ui| {
            ui.label(tl!("Resolution"));
            craft_ui::ComboBox::from_id_salt("export-dpi")
                .selected_text(crate::i18n::fmt(tl!("{dpi} pixels/inch"), &[("dpi", &d.dpi.to_string())]))
                .show_ui(ui, |ui| {
                    for dpi in [72.0, 96.0, 150.0, 300.0, 600.0] {
                        ui.selectable_value(&mut d.dpi, dpi, crate::i18n::fmt(tl!("{dpi} pixels/inch"), &[("dpi", &dpi.to_string())]));
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Format"));
            let mut quality = match d.format {
                ImageFormat::Jpeg { quality } => quality,
                _ => 85,
            };
            craft_ui::ComboBox::from_id_salt("export-format").selected_text(d.format.label()).show_ui(ui, |ui| {
                for f in [ImageFormat::Png, ImageFormat::Jpeg { quality }, ImageFormat::Tiff] {
                    let on = std::mem::discriminant(&d.format) == std::mem::discriminant(&f);
                    if ui.selectable_label(on, f.label()).clicked() {
                        d.format = f;
                    }
                }
            });
            if let ImageFormat::Jpeg { .. } = d.format {
                ui.label(tl!("Quality"));
                if ui.add(egui::Slider::new(&mut quality, 10..=100)).changed() {
                    d.format = ImageFormat::Jpeg { quality };
                }
            }
        });
        ui.label(
            egui::RichText::new(crate::i18n::fmt(tl!("One {f} file per page, named after the document."), &[("f", d.format.label())]))
                .small()
                .color(t.text_faint),
        );
        // What the files will take, measured on a sample of the pages by a worker (#2819).
        let (pages, dpi, format) = (d.range.pages(count), d.dpi, d.format);
        if !pages.is_empty()
            && let Some((_, id)) = active
            && let Some(doc) = app.session.get(id)
        {
            let key = SizeKey { doc: id, generation: doc.edit_generation(), pages, dpi: dpi.to_bits(), format };
            let size = &mut app.export_size;
            size.update(key, || doc.export_source(), ui.ctx(), app.run_inline);
            size_line(ui, &app.export_size, t);
        }
    } else if kind == ExportKind::AllImages {
        ui.horizontal(|ui| {
            ui.label(tl!("Exclude images smaller than"));
            let label = move |n: u32| {
                if n == 0 { tl!("No limit").to_string() } else { crate::i18n::trn(crate::i18n::current(), u64::from(n), "{n} pixel", "{n} pixels") }
            };
            craft_ui::ComboBox::from_id_salt("export-min").selected_text(label(d.min_side)).show_ui(ui, |ui| {
                for n in [0, 16, 32, 64, 128, 256] {
                    ui.selectable_value(&mut d.min_side, n, label(n));
                }
            });
        });
        ui.label(
            egui::RichText::new(tl!("Each image once, named after the document and page. JPEG images are saved unchanged; others as PNG."))
                .small()
                .color(t.text_faint),
        );
    } else {
        ui.label(egui::RichText::new(tl!("Plain text in reading order; pages are separated by form feeds.")).small().color(t.text_faint));
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Pages")).font(theme::semibold(12.5)));
    d.range.ui(ui, count);
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let ok = !d.range.pages(count).is_empty();
        if ui.add_enabled_ui(ok, |ui| widgets::pill_button(ui, tl!("Export"), true)).inner.clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

/// Write the files (`name`, bytes) produced for `pages`, reporting progress.
struct ExportTask<'a> {
    src: ExportSource,
    kind: ExportKind,
    dpi: f64,
    format: ImageFormat,
    min_side: u32,
    pages: Vec<usize>,
    stem: String,
    status: &'a ExportStatus,
    lang: crate::i18n::Lang,
}

fn run(task: ExportTask<'_>, mut sink: impl FnMut(&str, Vec<u8>) -> Result<(), String>) -> String {
    let ExportTask { src, kind, dpi, format, min_side, pages, stem, status, lang } = task;
    // May run on a worker thread: messages are drawn in the UI's language.
    crate::i18n::set_current(lang);
    let total = pages.len();
    let set = |done: usize, msg: Option<String>| {
        if let Ok(mut s) = status.lock() {
            *s = Some((done, total, msg));
        }
    };
    if kind == ExportKind::AllImages {
        set(0, None);
        let out = match pdfcraft_engine::export::extract_images(&src, &pages, min_side) {
            Ok(o) => o,
            Err(e) => return crate::i18n::fmt(tl!("Export stopped: {e}"), &[("e", &e.to_string())]),
        };
        for (k, img) in out.images.iter().enumerate() {
            if let Err(e) = sink(&pdfcraft_engine::export::image_file_name(&stem, img, k + 1, src.pages), img.data.clone()) {
                return crate::i18n::fmt(tl!("Export stopped: {e}"), &[("e", &e.to_string())]);
            }
        }
        let n = out.images.len();
        let mut msg = if n == 1 {
            crate::i18n::fmt(tl!("Exported 1 image"), &[])
        } else {
            crate::i18n::fmt(tl!("Exported {n} images"), &[("n", &n.to_string())])
        };
        if !out.skipped.is_empty() {
            msg.push_str(&crate::i18n::fmt(
                tl!(" ({s} not exported: {first})"),
                &[("s", &out.skipped.len().to_string()), ("first", &out.skipped[0].2)],
            ));
        }
        return msg;
    }
    let mut ex = Exporter::from_source(src);
    match kind {
        // Handled above.
        ExportKind::AllImages => String::new(),
        ExportKind::Image => {
            set(0, None);
            // Pages are drawn several at once and handed over in order: the progress counts the
            // pages written so far.
            let mut done = 0;
            let result = ex.images(&pages, dpi, format, |p, img| {
                sink(&format!("{stem}_page_{}.{}", p + 1, format.extension()), img)?;
                done += 1;
                set(done, None);
                Ok(())
            });
            if let Err(e) = result {
                return crate::i18n::fmt(tl!("Export stopped: {e}"), &[("e", &e.to_string())]);
            }
            let mut msg = if total == 1 {
                crate::i18n::fmt(tl!("Exported 1 image"), &[])
            } else {
                crate::i18n::fmt(tl!("Exported {n} images"), &[("n", &total.to_string())])
            };
            // Pages too large for the renderer at the chosen resolution are drawn at the most it
            // allows: say so, rather than leave a smaller image unexplained.
            let lowered: Vec<f64> = pages.iter().map(|p| ex.dpi_used(*p, dpi)).filter(|used| *used < dpi.clamp(18.0, 1200.0) - 0.5).collect();
            if let Some(lowest) = lowered.iter().copied().reduce(f64::min) {
                msg.push_str(&crate::i18n::fmt(
                    tl!(" ({n} at {dpi} dpi, the largest size they can be drawn at)"),
                    &[("n", &lowered.len().to_string()), ("dpi", &format!("{}", lowest.floor()))],
                ));
            }
            msg
        }
        ExportKind::Text => {
            set(0, None);
            match ex.text_of(&pages).and_then(|text| sink(&format!("{stem}.txt"), text.into_bytes())) {
                Ok(()) => {
                    if total == 1 {
                        crate::i18n::fmt(tl!("Exported the text of 1 page"), &[])
                    } else {
                        crate::i18n::fmt(tl!("Exported the text of {n} pages"), &[("n", &total.to_string())])
                    }
                }
                Err(e) => crate::i18n::fmt(tl!("Export stopped: {e}"), &[("e", &e.to_string())]),
            }
        }
    }
}

impl PdfCraftApp {
    /// Start exporting the active document with the dialog's settings.
    pub(crate) fn start_export(&mut self, kind: ExportKind) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        if !self.may_copy(id) {
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let src = doc.export_source();
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let pages = self.export_draft.range.pages(src.pages);
        let dpi = self.export_draft.dpi;
        let format = self.export_draft.format;
        let min_side = self.export_draft.min_side;
        let status: ExportStatus = Arc::new(Mutex::new(Some((0, pages.len(), None))));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let start = move |app: &mut Self, dir: std::path::PathBuf, inline: bool| {
                let st = status.clone();
                let shown = dir.display().to_string();
                let lang = crate::i18n::current();
                let work = move || {
                    let sink = |name: &str, bytes: Vec<u8>| {
                        crate::editing::write_atomically(&dir.join(name).to_string_lossy(), &bytes).map_err(|e| format!("{name}: {e}"))
                    };
                    let msg = run(ExportTask { src, kind, dpi, format, min_side, pages, stem, status: &st, lang }, sink);
                    if let Ok(mut s) = st.lock() {
                        let (done, total) = s.as_ref().map_or((0, 0), |(d, t, _)| (*d, *t));
                        let done_msg = crate::i18n::fmt(tl!("{msg} to {dir}"), &[("msg", &msg), ("dir", &shown)]);
                        *s = Some((done.max(total), total, Some(done_msg)));
                    }
                };
                if inline {
                    work(); // tests and automation: synchronous
                } else {
                    std::thread::Builder::new().name("pdfcraft-export".into()).spawn(work).ok();
                }
                app.export_status = Some(status);
            };
            match self.export_dir_override.clone() {
                Some(d) => start(self, d.into(), true),
                None => {
                    let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Choose a folder for the exported files").to_string());
                    self.ask_one(crate::pickers::Ask::Folder(dialog), None, move |app, dir| start(app, dir, false));
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let msg =
                run(ExportTask { src, kind, dpi, format, min_side, pages, stem, status: &status, lang: crate::i18n::current() }, |name, bytes| {
                    crate::editing::download(name, &bytes)
                });
            if let Ok(mut s) = status.lock() {
                *s = Some((0, 0, Some(msg)));
            }
            self.export_status = Some(status);
        }
    }

    /// Show export progress, and the result once it is done.
    pub(crate) fn poll_export(&mut self) {
        let Some(st) = self.export_status.clone() else { return };
        let snapshot = st.lock().ok().and_then(|s| s.clone());
        match snapshot {
            Some((_, _, Some(msg))) => {
                self.export_status = None;
                self.notify(msg);
            }
            Some((done, total, None)) if total > 1 => {
                let m = crate::i18n::fmt(tl!("Exporting… {d} of {t}"), &[("d", &done.to_string()), ("t", &total.to_string())]);
                self.notify(m);
            }
            _ => {}
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings for edit `generation`, asking to measure page 0.
    fn key(generation: u64) -> SizeKey {
        SizeKey { doc: DocId(1), generation, pages: vec![0], dpi: 150.0f64.to_bits(), format: ImageFormat::Png }
    }

    /// A source with no pages: measuring it fails at once, which is enough to see a worker start.
    fn source() -> ExportSource {
        ExportSource { bytes: Arc::new(Vec::new()), config: Default::default(), pages: 0, sizes: Vec::new() }
    }

    /// A render cannot be interrupted, so a cancelled estimate keeps its slot until its worker
    /// exits. Closing and reopening the dialog then cannot stack renders.
    #[test]
    fn a_cancelled_worker_keeps_its_slot_until_it_exits() {
        let ctx = egui::Context::default();
        let mut draft = SizeDraft::default();
        // A worker that is still rendering: its sender is alive, so it has not exited.
        let (sender, receiver) = mpsc::channel();
        draft.key = Some(key(1));
        draft.job = Some(SizeJob { key: key(1), receiver, cancel: Arc::new(AtomicBool::new(false)) });

        // New settings cancel it, but nothing starts while it winds down.
        let mut started = 0;
        draft.update(
            key(2),
            || {
                started += 1;
                source()
            },
            &ctx,
            true,
        );
        assert_eq!(started, 0, "no worker while a cancelled one is still rendering");
        assert_eq!(draft.retiring.len(), 1, "the cancelled worker is still accounted for");

        // The worker exits (drops its sender). The new estimate settles, then starts.
        drop(sender);
        draft.update(
            key(2),
            || {
                started += 1;
                source()
            },
            &ctx,
            true,
        );
        assert_eq!(started, 0, "the frame after settling only settles");
        draft.update(
            key(2),
            || {
                started += 1;
                source()
            },
            &ctx,
            true,
        );
        assert_eq!(started, 1, "the estimate starts once the previous worker has exited");
        assert!(draft.retiring.is_empty(), "the exited worker is forgotten");
    }
}
