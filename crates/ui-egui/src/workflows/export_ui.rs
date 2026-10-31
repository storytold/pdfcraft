//! Export a PDF ▸ Image (PNG, JPEG, TIFF) and Text (Acrobat's Export a PDF tool, first formats).
//!
//! On the desktop the export runs on a worker thread and reports progress in the notice bar;
//! on the web it runs in place and downloads the files.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout};
use pdfcraft_engine::export::{ExportSource, Exporter, ImageFormat};

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

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, kind: ExportKind) -> (bool, bool) {
    let count = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.info.pages.len()).unwrap_or(0);
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
        *status.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((done, total, msg));
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
        let draft = self.export_draft.clone();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let start = move |app: &mut Self, dir: std::path::PathBuf, inline: bool| {
                app.start_export_into(
                    src,
                    kind,
                    draft,
                    stem,
                    dir,
                    inline,
                    #[cfg(test)]
                    Default::default(),
                );
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
            let pages = draft.range.pages(src.pages);
            let status: ExportStatus = Arc::new(Mutex::new(Some((0, pages.len(), None))));
            let msg = pdfcraft_engine::guard(|| {
                run(
                    ExportTask {
                        src,
                        kind,
                        dpi: draft.dpi,
                        format: draft.format,
                        min_side: draft.min_side,
                        pages,
                        stem,
                        status: &status,
                        lang: crate::i18n::current(),
                    },
                    |name, bytes| crate::editing::download(name, &bytes),
                )
            })
            .unwrap_or_else(|e| crate::i18n::fmt(tl!("Export stopped: {e}"), &[("e", &e)]));
            *status.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((0, 0, Some(msg)));
            self.export_status = Some(status);
        }
    }

    /// The desktop continuation after the destination is known.
    #[cfg(not(target_arch = "wasm32"))]
    #[cfg_attr(test, allow(clippy::too_many_arguments))]
    fn start_export_into(
        &mut self,
        src: ExportSource,
        kind: ExportKind,
        draft: ExportDraft,
        stem: String,
        dir: std::path::PathBuf,
        inline: bool,
        #[cfg(test)] fault: tests::Fault,
    ) {
        let pages = draft.range.pages(src.pages);
        let status: ExportStatus = Arc::new(Mutex::new(Some((0, pages.len(), None))));
        let st = status.clone();
        let shown = dir.display().to_string();
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            let msg = pdfcraft_engine::guard(|| {
                #[cfg(test)]
                if fault.panic || fault.poison {
                    let _held = fault.poison.then(|| st.lock().unwrap());
                    panic!("export worker fault");
                }
                let sink = |name: &str, bytes: Vec<u8>| {
                    crate::editing::write_atomically(&dir.join(name).to_string_lossy(), &bytes).map_err(|e| format!("{name}: {e}"))
                };
                let msg = run(
                    ExportTask { src, kind, dpi: draft.dpi, format: draft.format, min_side: draft.min_side, pages, stem, status: &st, lang },
                    sink,
                );
                crate::i18n::fmt(tl!("{msg} to {dir}"), &[("msg", &msg), ("dir", &shown)])
            })
            .unwrap_or_else(|e| crate::i18n::fmt(tl!("Export stopped: {e}"), &[("e", &e)]));
            // Completion must survive an unwind, including a poisoned progress lock.
            let mut s = st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let (done, total) = s.as_ref().map_or((0, 0), |(d, t, _)| (*d, *t));
            *s = Some((done.max(total), total, Some(msg)));
        };
        if inline {
            work();
        } else if let Err(e) = launch(
            work,
            #[cfg(test)]
            fault.spawn,
            #[cfg(test)]
            fault.finished,
        ) {
            let msg = crate::i18n::fmt(tl!("Export stopped: {e}"), &[("e", &e.to_string())]);
            let mut s = status.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let total = s.as_ref().map_or(0, |(_, total, _)| *total);
            *s = Some((total, total, Some(msg)));
        }
        self.export_status = Some(status);
    }

    /// Show export progress, and the result once it is done.
    pub(crate) fn poll_export(&mut self) {
        let Some(st) = self.export_status.clone() else { return };
        let snapshot = st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
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

#[cfg(not(target_arch = "wasm32"))]
fn launch(
    work: impl FnOnce() + Send + 'static,
    #[cfg(test)] fail: bool,
    #[cfg(test)] finished: Option<std::sync::mpsc::Sender<bool>>,
) -> std::io::Result<()> {
    #[cfg(test)]
    if fail {
        return Err(std::io::Error::other("export launch fault"));
    }
    #[cfg(test)]
    let work = tests::observe(work, finished);
    std::thread::Builder::new().name("pdfcraft-export".into()).spawn(work).map(|_| ())
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Sender};
    use std::time::Duration;

    #[derive(Default)]
    pub(super) struct Fault {
        pub panic: bool,
        pub poison: bool,
        pub spawn: bool,
        pub finished: Option<Sender<bool>>,
    }

    pub(super) fn observe(work: impl FnOnce(), finished: Option<Sender<bool>>) -> impl FnOnce() {
        move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            if let Some(finished) = finished {
                let _ = finished.send(result.is_ok());
            }
            if let Err(payload) = result {
                std::panic::resume_unwind(payload);
            }
        }
    }

    struct Folder(std::path::PathBuf, crate::i18n::Lang);
    impl Folder {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("pdfcraft-export-worker-{name}-{}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            let previous = crate::i18n::current();
            crate::i18n::set_current(crate::i18n::Lang::EN);
            Self(path, previous)
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            crate::i18n::set_current(self.1);
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn fixture() -> Vec<u8> {
        // Contributor-original pages. The text contract is Alpha, then Beta.
        let mut pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 /MediaBox [0 0 200 300] >> endobj\n3 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n".to_vec();
        for (page, text) in [(4, "Alpha"), (6, "Beta")] {
            let stream = format!("BT /F1 24 Tf 20 150 Td ({text}) Tj ET");
            pdf.extend_from_slice(format!("{page} 0 obj << /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >> endobj\n{} 0 obj << /Length {} >> stream\n{stream}\nendstream endobj\n", page + 1, page + 1, stream.len()).as_bytes());
        }
        pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
        pdf
    }

    fn app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("source.pdf", None, fixture()).unwrap();
        app
    }

    fn start(app: &mut PdfCraftApp, dir: &std::path::Path, fault: Fault) {
        let (_, id) = app.active_ids().unwrap();
        let src = app.session.get(id).unwrap().export_source();
        app.start_export_into(src, ExportKind::Text, app.export_draft.clone(), "source".into(), dir.into(), false, fault);
    }

    fn has_completion(app: &PdfCraftApp) -> bool {
        app.export_status
            .as_ref()
            .is_some_and(|st| st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().is_some_and(|(_, _, msg)| msg.is_some()))
    }

    #[derive(Debug, PartialEq)]
    struct SourceState {
        bytes: Arc<Vec<u8>>,
        dirty: bool,
        undo: Option<String>,
        redo: Option<String>,
    }

    fn source_state(app: &PdfCraftApp) -> SourceState {
        let (_, id) = app.active_ids().unwrap();
        let doc = app.session.get(id).unwrap();
        SourceState { bytes: doc.bytes.clone(), dirty: doc.dirty, undo: doc.can_undo().map(str::to_owned), redo: doc.can_redo().map(str::to_owned) }
    }

    #[test]
    fn native_launch_failure_completes_export() {
        let dir = Folder::new("launch");
        let mut app = app();
        let before_state = source_state(&app);
        let (_, id) = app.active_ids().unwrap();
        let before = app.session.get(id).unwrap().bytes.clone();
        start(&mut app, &dir.0, Fault { spawn: true, ..Default::default() });
        assert!(has_completion(&app), "launch error leaves export active without completion");
        app.poll_export();
        assert!(app.export_status.is_none());
        assert_eq!(app.toast.as_ref().unwrap().0, "Export stopped: export launch fault");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
        assert_eq!(app.session.get(id).unwrap().bytes, before);
        assert_eq!(source_state(&app), before_state);
    }

    #[test]
    fn native_worker_panic_completes_export_and_allows_exact_text_retry() {
        let dir = Folder::new("panic");
        let mut app = app();
        let before_state = source_state(&app);
        let (_, id) = app.active_ids().unwrap();
        let before = app.session.get(id).unwrap().bytes.clone();
        let (tx, rx) = mpsc::channel();
        start(&mut app, &dir.0, Fault { panic: true, finished: Some(tx), ..Default::default() });
        let contained = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(has_completion(&app), "native worker finished (contained={contained}) without publishing export completion");
        assert!(contained);
        app.poll_export();
        assert!(app.export_status.is_none());
        assert_eq!(app.toast.as_ref().unwrap().0, "Export stopped: export worker fault");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
        assert_eq!(app.session.get(id).unwrap().bytes, before);
        let (tx, rx) = mpsc::channel();
        start(&mut app, &dir.0, Fault { finished: Some(tx), ..Default::default() });
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        app.poll_export();
        assert!(app.export_status.is_none());
        assert_eq!(std::fs::read(dir.0.join("source.txt")).unwrap(), b"Alpha\n\x0cBeta\n");
        assert_eq!(app.toast.as_ref().unwrap().0, format!("Exported the text of 2 pages to {}", dir.0.display()));
        assert_eq!(app.session.get(id).unwrap().bytes, before);
        assert_eq!(source_state(&app), before_state);
    }

    #[test]
    fn native_panic_uses_captured_french_language() {
        let dir = Folder::new("french");
        let mut app = app();
        let before_state = source_state(&app);
        crate::i18n::set_current(crate::i18n::Lang::from_code("fr").unwrap());
        let (tx, rx) = mpsc::channel();
        start(&mut app, &dir.0, Fault { panic: true, finished: Some(tx), ..Default::default() });
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        assert!(has_completion(&app));
        app.poll_export();
        assert!(app.export_status.is_none());
        assert_eq!(app.toast.as_ref().unwrap().0, "Exportation interrompue : export worker fault");
        assert_eq!(source_state(&app), before_state);
    }

    #[test]
    fn native_panic_recovers_poisoned_export_progress() {
        let dir = Folder::new("poison");
        let mut app = app();
        let before_state = source_state(&app);
        let (tx, rx) = mpsc::channel();
        start(&mut app, &dir.0, Fault { poison: true, finished: Some(tx), ..Default::default() });
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        let status = app.export_status.as_ref().unwrap();
        assert!(status.is_poisoned());
        assert!(has_completion(&app));
        app.poll_export();
        assert!(app.export_status.is_none());
        assert_eq!(app.toast.as_ref().unwrap().0, "Export stopped: export worker fault");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
        assert_eq!(source_state(&app), before_state);
    }

    #[test]
    fn native_returned_export_error_keeps_its_existing_destination_message() {
        let dir = Folder::new("normal-error");
        let mut app = app();
        let before_state = source_state(&app);
        let blocked = dir.0.join("blocked");
        std::fs::write(&blocked, b"keep destination").unwrap();
        let (tx, rx) = mpsc::channel();
        start(&mut app, &blocked, Fault { finished: Some(tx), ..Default::default() });
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        app.poll_export();
        assert!(app.export_status.is_none());
        let error = std::fs::File::create(blocked.join("source.txt")).unwrap_err().to_string();
        assert_eq!(app.toast.as_ref().unwrap().0, format!("Export stopped: source.txt: {error} to {}", blocked.display()));
        assert_eq!(std::fs::read(&blocked).unwrap(), b"keep destination");
        assert_eq!(source_state(&app), before_state);
    }
}
