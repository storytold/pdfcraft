//! Export a PDF ▸ Image (PNG, JPEG, TIFF) and Text (Acrobat's Export a PDF tool, first formats).
//!
//! The export runs as a job and reports progress in the notice bar; the files go through the host's
//! file service, which downloads them on the web.

use std::path::PathBuf;
use std::sync::Arc;

use craft::host::queue::Priority;
use craft::host::{PickRequest, WriteIntent};
use craft::pages::PageRange;
use craft::pages::ui::range::{PageRangeChoices, PageRangeOptions};
use craft::ui::contract::Bind;
use craft::ui::dialog::{DialogResult, dialog_id};
use craft::ui::patterns::form_dialogs::{DialogFields, FieldValue, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::runtime::env::Label;
use craft::ui::value_drag::SliderScale;
use pdfcraft_engine::export::{ExportSource, Exporter, ImageFormat};

use super::job_count::JobCount;
use crate::PdfCraftApp;
use crate::notices::{self, ProgressKey, ProgressNotice};

/// The queue of exports, keyed by document, each answering with its summary line.
pub(crate) type ExportJobs = craft::host::queue::JobQueue<crate::DocId, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Image,
    Text,
    /// Export all images: the images pages use, as files.
    AllImages,
}

impl ExportKind {
    const ALL: [ExportKind; 3] = [ExportKind::Image, ExportKind::Text, ExportKind::AllImages];

    /// The dialog's key: the command that opens it. Each kind has its own so a result says which it answers.
    fn key(self) -> &'static str {
        match self {
            ExportKind::Image => "export.image",
            ExportKind::Text => "export.text",
            ExportKind::AllImages => "export.all_images",
        }
    }
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

const DPIS: [u32; 5] = [72, 96, 150, 300, 600];
const MIN_SIDES: [u32; 6] = [0, 16, 32, 64, 128, 256];
const DEFAULT_QUALITY: u8 = 85;

fn min_side_label(n: u32) -> String {
    if n == 0 { tl!("No limit").to_string() } else { crate::i18n::trn(crate::i18n::current(), u64::from(n), "{n} pixel", "{n} pixels") }
}

fn format_id(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg { .. } => "jpeg",
        ImageFormat::Tiff => "tiff",
    }
}

/// The dialog's rows for `kind` over a document of `count` pages.
fn params(kind: ExportKind, count: usize, current: usize) -> Vec<Param> {
    let mut rows = Vec::new();
    match kind {
        ExportKind::Image => {
            let dpis =
                DPIS.iter().map(|n| (n.to_string(), Label::from(crate::i18n::fmt(tl!("{dpi} pixels/inch"), &[("dpi", &n.to_string())])))).collect();
            rows.push(Param::new("dpi", tl!("Resolution"), ParamKind::choice(dpis, Some("150".into()))));
            let formats = ["png", "jpeg", "tiff"]
                .iter()
                .map(|id| {
                    let label = match *id {
                        "png" => ImageFormat::Png.label(),
                        "jpeg" => ImageFormat::Jpeg { quality: DEFAULT_QUALITY }.label(),
                        _ => ImageFormat::Tiff.label(),
                    };
                    (id.to_string(), Label::from(label))
                })
                .collect();
            rows.push(Param::new("format", tl!("Format"), ParamKind::choice(formats, Some("png".into()))));
            rows.push(
                Param::new("quality", tl!("Quality"), ParamKind::range(10.0, 100.0, f64::from(DEFAULT_QUALITY), SliderScale::Linear))
                    .decimals(0)
                    .visible_when("format", "jpeg"),
            );
        }
        ExportKind::AllImages => {
            let sides = MIN_SIDES.iter().map(|n| (n.to_string(), Label::from(min_side_label(*n)))).collect();
            rows.push(Param::new("min", tl!("Exclude images smaller than"), ParamKind::choice(sides, Some("0".into()))));
        }
        ExportKind::Text => {}
    }
    rows.push(PageRangeOptions::param("pages", tl!("Pages"), count, current, PageRangeChoices::Acrobat));
    rows
}

/// The dialog's opening values: the last used ones.
fn fields_of(d: &ExportDraft) -> DialogFields {
    let mut fields = DialogFields::new();
    fields.set("dpi", FieldValue::Text((d.dpi as u32).to_string()));
    fields.set("format", FieldValue::Text(format_id(d.format).into()));
    if let ImageFormat::Jpeg { quality } = d.format {
        fields.set("quality", FieldValue::Number(f64::from(quality)));
    }
    fields.set("min", FieldValue::Text(d.min_side.to_string()));
    fields.set("pages", PageRangeOptions::field_value(&d.range));
    fields
}

/// Opens the Export dialog for `kind` over the active document.
pub(crate) fn open(app: &mut PdfCraftApp, ctx: &egui::Context, kind: ExportKind) {
    let (count, current) = match app.active_ids() {
        Some((i, id)) => (app.session.get(id).map_or(0, |d| d.info.pages.len()), app.views[i].current()),
        None => (0, 0),
    };
    let fields = fields_of(&app.export_draft);
    let dialog = HostedParamDialog::new(ctx, kind.key(), fields, move |fields| {
        let usable = PageRangeOptions::value(fields, "pages").is_some_and(|r| !r.pages(count, current, &[]).is_empty());
        let (title, note) = match kind {
            ExportKind::Image => {
                let f = match fields.text("format") {
                    Some("jpeg") => ImageFormat::Jpeg { quality: DEFAULT_QUALITY },
                    Some("tiff") => ImageFormat::Tiff,
                    _ => ImageFormat::Png,
                };
                (tl!("Export to Image"), crate::i18n::fmt(tl!("One {f} file per page, named after the document."), &[("f", f.label())]))
            }
            ExportKind::Text => (tl!("Export to Text"), tl!("Plain text in reading order; pages are separated by form feeds.").to_string()),
            ExportKind::AllImages => (
                tl!("Export All Images"),
                tl!("Each image once, named after the document and page. JPEG images are saved unchanged; others as PNG.").to_string(),
            ),
        };
        ParamDialog::new(kind.key(), title, params(kind, count, current))
            .values(Bind::bound(fields))
            .ok_label(tl!("Export").into())
            .ok_enabled(usable)
            .footer_note(note)
    });
    crate::dialogs::host::open(app, dialog);
}

/// The kind of Export dialog on the host, if one is open.
#[cfg(test)]
pub(crate) fn open_kind(app: &PdfCraftApp) -> Option<ExportKind> {
    ExportKind::ALL.into_iter().find(|k| app.dialogs.get_by_id::<HostedParamDialog>(dialog_id(k.key())).is_some())
}

/// Takes what the Export dialogs answered this pass: OK stores the values and starts the export.
pub(crate) fn pass(ctx: &egui::Context, app: &mut PdfCraftApp) {
    for kind in ExportKind::ALL {
        let Some(DialogResult::Ok(result)) = app.dialogs.take_by_id::<HostedParamDialog>(dialog_id(kind.key())) else { continue };
        let f = &result.fields;
        let d = &mut app.export_draft;
        if let Some(range) = PageRangeOptions::value(f, "pages") {
            d.range = range;
        }
        if let Some(dpi) = f.text("dpi").and_then(|t| t.parse::<f64>().ok()) {
            d.dpi = dpi;
        }
        if let Some(n) = f.text("min").and_then(|t| t.parse::<u32>().ok()) {
            d.min_side = n;
        }
        match f.text("format") {
            Some("jpeg") => {
                let quality = f.number("quality").unwrap_or(f64::from(DEFAULT_QUALITY)).clamp(10.0, 100.0).round() as u8;
                d.format = ImageFormat::Jpeg { quality };
            }
            Some("tiff") => d.format = ImageFormat::Tiff,
            Some(_) => d.format = ImageFormat::Png,
            None => {}
        }
        app.start_export(ctx, kind);
    }
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
    status: &'a JobCount,
    lang: crate::i18n::Lang,
}

fn run(task: ExportTask<'_>, mut sink: impl FnMut(&str, Vec<u8>) -> Result<(), String>) -> String {
    let ExportTask { src, kind, dpi, format, min_side, pages, stem, status, lang } = task;
    // Runs on a worker thread: messages are drawn in the UI's language.
    crate::i18n::set_current(lang);
    let total = pages.len();
    let set = |done: usize| status.set(done, total);
    if kind == ExportKind::AllImages {
        set(0);
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
            set(0);
            // Pages are drawn several at once and handed over in order: the progress counts the
            // pages written so far.
            let mut done = 0;
            let result = ex.images(&pages, dpi, format, |p, img| {
                sink(&format!("{stem}_page_{}.{}", p + 1, format.extension()), img)?;
                done += 1;
                set(done);
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
            set(0);
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
    pub(crate) fn start_export(&mut self, ctx: &egui::Context, kind: ExportKind) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing(ctx) {
            return;
        }
        let Some((view, id)) = self.active_ids() else { return };
        if !self.may_copy(id) {
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let src = doc.export_source();
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let current = self.views[view].current();
        let pages = self.export_draft.range.pages(src.pages, current, &[]);
        let dpi = self.export_draft.dpi;
        let format = self.export_draft.format;
        let min_side = self.export_draft.min_side;
        let task = move |app: &mut Self, dir: PathBuf| app.submit_export(id, ExportTask_ { src, kind, dpi, format, min_side, pages, stem }, dir);
        // The web has no folder to pick: the files download.
        #[cfg(target_arch = "wasm32")]
        task(self, PathBuf::new());
        #[cfg(not(target_arch = "wasm32"))]
        self.pick_first(PickRequest::folder("export.folder").title(tl!("Choose a folder for the exported files")), None, move |app, _, dir| {
            task(app, dir)
        });
    }

    /// Submits the export of `task` into `dir` (empty on the web).
    fn submit_export(&mut self, id: crate::DocId, task: ExportTask_, dir: PathBuf) {
        let ExportTask_ { src, kind, dpi, format, min_side, pages, stem } = task;
        let count = Arc::new(JobCount::default());
        let job_count = count.clone();
        let host = self.host.clone(); // PLAN: assumed `PdfCraftApp::host: Arc<Host>` (pdf-hub-w1)

        let lang = crate::i18n::current();
        let shown = dir.display().to_string();
        self.export_jobs.submit(id, Priority::VISIBLE, move |_| {
            let sink = |name: &str, bytes: Vec<u8>| {
                host.files().write(&dir.join(name), &bytes, WriteIntent::SaveAs).map(|_| ()).map_err(|e| format!("{name}: {e}"))
            };
            let msg = run(ExportTask { src, kind, dpi, format, min_side, pages, stem, status: &job_count, lang }, sink);

            if shown.is_empty() { msg } else { crate::i18n::fmt(tl!("{msg} to {dir}"), &[("msg", &msg), ("dir", &shown)]) }
        });
        self.export_status = Some(count);
    }

    /// Show export progress, and the result once it is done.
    pub(crate) fn poll_export(&mut self) {
        // Without lanes the owner runs the job, one per pass.
        #[cfg(target_arch = "wasm32")]
        self.export_jobs.run_inline(1);
        let Some(count) = self.export_status.clone() else { return };
        if let Some(finished) = self.export_jobs.try_recv() {
            self.export_status = None;
            notices::end_progress(self, ProgressKey::Export);
            self.notify(finished.result);
            return;
        }
        let Some((done, total)) = count.take_change() else { return };
        if total > 1 {
            let label = crate::i18n::fmt(tl!("Exporting… {d} of {t}"), &[("d", &done.to_string()), ("t", &total.to_string())]);
            notices::set_progress(self, ProgressKey::Export, ProgressNotice { label, fraction: done as f32 / total as f32, cancellable: false });
        }
    }
}

/// What the folder pick carries to the job: the export without its status and language.
struct ExportTask_ {
    src: ExportSource,
    kind: ExportKind,
    dpi: f64,
    format: ImageFormat,
    min_side: u32,
    pages: Vec<usize>,
    stem: String,
}
