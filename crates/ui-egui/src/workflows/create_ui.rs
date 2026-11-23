//! Create a PDF (blank, from images, from text) and Reduce File Size (execution plan M10.2,
//! M11.1). Opening an image or a text file converts it to a new, unsaved PDF, as Acrobat does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft::host::{PickRequest, Picked, WriteIntent};
use craft::platform::clipboard::TEXT;
use craft::ui::contract::Bind;
use craft::ui::dialog::{DialogResult, dialog_id};
use craft::ui::patterns::form_dialogs::{DialogFields, FieldValue, HostedParamDialog, Param, ParamDialog, ParamKind};

use crate::PdfCraftApp;

/// The dialog key of the image import: the id its result is taken by.
const IMAGES_DIALOG: &str = "create.images";

/// The images staged for the resolution dialog; no document exists until the dialog confirms.
pub struct ImageImport {
    pub images: Vec<(String, Vec<u8>)>,
}

/// What Create ▸ Clipboard found on the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub enum Clip {
    /// RGBA pixels, row by row.
    Image {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    Text(String),
}

pub(crate) const MAX_IMAGE_SIDE: usize = 16384;
pub(crate) const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn png_from_rgba(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h) = (u32::try_from(width).map_err(|e| e.to_string())?, u32::try_from(height).map_err(|e| e.to_string())?);
    let expected = width.checked_mul(height).and_then(|p| p.checked_mul(4));
    if w == 0 || h == 0 || width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE || expected != Some(rgba.len()) || rgba.len() > MAX_IMAGE_BYTES {
        return Err("the clipboard image is empty, malformed or exceeds size limits".into());
    }
    let img = craft_image_codecs::Image::from_u8(w, h, craft_image_codecs::ChannelLayout::Rgba, rgba.to_vec()).map_err(|e| e.to_string())?;
    craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).map_err(|e| e.to_string())
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// The resolution dialog over `count` staged images. The values are the host's; `build` makes the dialog over them
/// each pass.
fn image_import_dialog(ctx: &egui::Context, count: usize) -> HostedParamDialog {
    let mut fields = DialogFields::new();
    fields.set("resolution", FieldValue::Text("image".into()));
    fields.set("dpi", FieldValue::Int(300));
    HostedParamDialog::new(ctx, IMAGES_DIALOG, fields, move |fields| {
        let options = vec![
            ("image".to_string(), tl!("Use image resolution").into()),
            ("72".to_string(), tl!("Use 72 DPI (one point per pixel)").into()),
            ("custom".to_string(), tl!("Use custom DPI").into()),
        ];
        let params = vec![
            Param::read_only_text("count", tl!("Images"), crate::i18n::fmt(tl!("Selected image files: {count}"), &[("count", &count.to_string())])),
            Param::new("resolution", tl!("Resolution"), ParamKind::radio(options, Some("image".to_string()))),
            Param::read_only_text("embedded", tl!("Image resolution"), tl!("Use each image's embedded DPI; use 72 DPI when it is absent.")),
            Param::new("dpi", tl!("DPI"), ParamKind::int(Some(1), Some(1200), 300)).enabled_when("resolution", FieldValue::Text("custom".into())),
        ];
        ParamDialog::new(IMAGES_DIALOG, tl!("Create PDF from Images"), params)
            .values(Bind::bound(fields))
            .ok_label(tl!("Create").into())
            .footer_note(tl!("DPI sets the printed page size without resampling the image."))
    })
}

/// Takes the image dialog's answer once it has finished: OK creates the document at the chosen resolution, Cancel
/// creates nothing. Either way the staged images are released.
pub(crate) fn pass(app: &mut PdfCraftApp) {
    let Some(result) = app.dialogs.take_by_id::<HostedParamDialog>(dialog_id(IMAGES_DIALOG)) else { return };
    let Some(draft) = app.image_import.take() else { return };
    let DialogResult::Ok(answer) = result else { return };
    let resolution = match answer.fields.text("resolution") {
        Some("72") => pdfcraft_engine::ImageResolution::Dpi(72.0),
        Some("custom") => pdfcraft_engine::ImageResolution::Dpi(answer.fields.int("dpi").unwrap_or(300) as f64),
        _ => pdfcraft_engine::ImageResolution::Embedded,
    };
    app.create_from_images_with_resolution(draft.images, resolution);
}

impl PdfCraftApp {
    /// Convert a non-PDF file (image, text, Office document) into a new tab. Returns `None` when
    /// `bytes` is not something Create understands (the caller then tries to open it as a PDF).
    /// An Office document blocks while LibreOffice runs: File ▸ Open converts it on the open
    /// worker instead ([`Self::open_converted_pdf`]), leaving this for startup and inline opens.
    pub(crate) fn open_converted(&mut self, name: &str, bytes: &[u8]) -> Option<Result<(), String>> {
        use pdfcraft_engine::SourceKind;
        if !matches!(pdfcraft_engine::source_kind(name, bytes), Some(SourceKind::Image | SourceKind::Text | SourceKind::Office)) {
            return None;
        }
        let created = self.session.convert_to_pdf(name, &Arc::new(bytes.to_vec())).map(|(_, pdf)| pdf);
        Some(self.open_converted_pdf(name, created.map_err(|e| e.to_string())))
    }

    /// Open `created`, the PDF converted from the file `name`, as a new, unsaved tab named after
    /// it (`Report.docx` → `Report.pdf`).
    pub(crate) fn open_converted_pdf(&mut self, name: &str, created: Result<Arc<Vec<u8>>, String>) -> Result<(), String> {
        created.and_then(|bytes| self.open_new_document(&format!("{}.pdf", stem(name)), bytes))
    }

    /// Open `bytes`, a PDF the app just made, as a new, unsaved tab called `name`.
    pub(crate) fn open_new_document(&mut self, name: &str, bytes: Arc<Vec<u8>>) -> Result<(), String> {
        let id = self.session.open_new(name, bytes).map_err(|e| e.to_string())?;
        // The reading-colours preference is app-wide: the newly created document starts with it.
        self.session.set_reading_colors(id, self.reading_colors);
        let info = &self.session.get(id).ok_or_else(|| tl!("the new document could not be opened").to_string())?.info;
        let mut view = crate::DocView::new(id, info, self.view_defaults, self.highlight_fields);
        // PLAN: assumed `full_screen() -> bool` (pdf-shell-w8)
        if self.full_screen() {
            view.enter_full_screen(self.full_screen_fill);
        }
        self.views.push(view);
        self.active = Some(self.views.len() - 1);
        Ok(())
    }

    /// The system clipboard's text when it has any, else its image: Create ▸ Clipboard takes what the user copied last,
    /// and a cached image never hides newer text.
    fn read_clipboard(&mut self) -> Option<Clip> {
        if let Ok(Some(flavor)) = self.clipboard.read(&[TEXT])
            && let Ok(text) = String::from_utf8(flavor.data)
            && !text.trim().is_empty()
        {
            return Some(Clip::Text(text));
        }
        let image = self.clipboard.read_image().ok().flatten()?;
        Some(Clip::Image { width: image.width as usize, height: image.height as usize, rgba: image.rgba })
    }

    /// Create ▸ Clipboard: a new document from the image (one page, its size) or the text on
    /// the clipboard, as Acrobat does.
    pub(crate) fn create_from_clipboard(&mut self) {
        match self.read_clipboard() {
            Some(c) => {
                if let Err(e) = self.create_from_clip(c) {
                    self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
                }
            }
            None => self.notify_tr("The clipboard has no image or text"),
        }
    }

    /// Create a new document from clipboard contents.
    pub fn create_from_clip(&mut self, clip: Clip) -> Result<(), String> {
        let created = match clip {
            Clip::Image { width, height, rgba } => {
                let png = png_from_rgba(width, height, &rgba)?;
                self.session.create_from_images(&[("Clipboard.png".into(), png)])
            }
            Clip::Text(t) => self.session.create_from_text("Clipboard", &t),
        };
        created.map_err(|e| e.to_string()).and_then(|bytes| self.open_new_document("Clipboard.pdf", bytes))
    }

    /// Create ▸ Blank page: a new untitled document on the system region's paper (#5644).
    pub(crate) fn create_blank(&mut self) {
        let (w, h) = self.blank_paper.points();
        let created = self.session.create_blank(w, h, 1).map_err(|e| e.to_string());
        if let Err(e) = created.and_then(|bytes| self.open_new_document("Untitled.pdf", bytes)) {
            self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
        }
    }

    /// Create ▸ Images: several images, one page each, in one new document.
    pub(crate) fn create_from_images_dialog(&mut self) {
        let request = PickRequest::open_many("create.images")
            .title(tl!("Choose images"))
            .filter(tl!("Images"), &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"]);
        self.pick_file(request, None, move |app, ctx, picked| {
            let images = match picked {
                // The browser hands over the bytes with the names.
                Picked::Contents(files) => files,
                Picked::Paths(paths) => {
                    let mut images = Vec::new();
                    for f in paths {
                        match app.host.files().read(&f) {
                            Ok(b) => images.push((f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), b)),
                            Err(e) => {
                                app.notify_fmt("Couldn't read {name}: {e}", &[("name", &f.display().to_string()), ("e", &e.to_string())]);
                                return;
                            }
                        }
                    }
                    images
                }
                Picked::Cancelled | Picked::Unavailable(_) => return,
            };
            app.begin_image_import(ctx, images);
        });
    }

    /// One new document from images (tests and automation call this directly).
    pub fn create_from_images(&mut self, images: Vec<(String, Vec<u8>)>) {
        self.create_from_images_with_resolution(images, pdfcraft_engine::ImageResolution::Embedded);
    }

    /// Stage selected images for the DPI chooser; no document is created until confirmed.
    // PLAN: `apps/pdfcraft/src/main.rs:536` must pass its `egui::Context` (the owner of that caller)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn begin_image_import_paths(&mut self, ctx: &egui::Context, paths: &[String]) -> Result<(), String> {
        let images = paths
            .iter()
            .map(|p| {
                let path = std::path::Path::new(p);
                let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                Ok((name, bytes))
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.begin_image_import(ctx, images);
        Ok(())
    }

    /// Stage selected images for the DPI chooser; no document is created until confirmed.
    pub fn begin_image_import(&mut self, ctx: &egui::Context, images: Vec<(String, Vec<u8>)>) {
        if images.is_empty() {
            return;
        }
        let dialog = image_import_dialog(ctx, images.len());
        self.image_import = Some(ImageImport { images });
        crate::dialogs::host::open(self, dialog);
    }

    /// Create directly at the requested resolution (also used by UI tests).
    pub fn create_from_images_with_resolution(&mut self, images: Vec<(String, Vec<u8>)>, resolution: pdfcraft_engine::ImageResolution) {
        if images.is_empty() {
            return;
        }
        let name =
            if let Some((name, _)) = images.first().filter(|_| images.len() == 1) { format!("{}.pdf", stem(name)) } else { "Images.pdf".to_string() };
        let created = self.session.create_from_images_with_resolution(&images, resolution).map_err(|e| e.to_string());
        if let Err(e) = created.and_then(|bytes| self.open_new_document(&name, bytes)) {
            self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
        }
    }

    /// Reduce File Size: write a compacted copy with images downsampled (the open document is
    /// unchanged). It asks where to save first, so a heavy file can finish by itself, then runs in
    /// the background with a progress bar, like the PDF Optimizer.
    pub(crate) fn reduce_file_size(&mut self, ctx: &egui::Context) {
        use crate::optimize_ui::OptimizeKind::Reduce;
        let settings = pdfcraft_engine::optimize::Settings::default();
        // What can't run says so now, not after asking where to save.
        if !self.commit_form_typing(ctx) {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        if self.optimize_run.is_some() {
            self.notify_tr("Optimization is already running");
            return;
        }
        if let Err(e) = self.session.optimize_job(id, &settings, &[]) {
            return self.save_optimized(id, Reduce, Err(e), None);
        }
        let Some(doc) = self.session.get(id) else { return };
        let name = optimized_name(&doc.name, Reduce);
        let folder = doc.path.as_deref().and_then(|p| Path::new(p).parent()).map(Path::to_path_buf);
        let mut request = PickRequest::save("reduce").file_name(name).filter("PDF", &["pdf"]);
        if let Some(folder) = folder {
            request = request.default_directory(folder);
        }
        // With the target, a document closed, switched from or edited meanwhile isn't compressed.
        let target = self.pick_target(Some(id));
        self.pick_file(request, target, move |app, ctx, picked| {
            if let Some(path) = picked.paths().into_iter().next() {
                app.run_optimize(ctx, id, Reduce, &settings, &[], Some(path.to_string_lossy().into_owned()));
            }
        });
    }

    /// Save an optimized copy (Reduce File Size, Optimize PDF) to `destination`, or where the user
    /// now chooses, reporting the saving. A Reduce File Size copy that isn't smaller is not saved
    /// (#490).
    pub(crate) fn save_optimized(
        &mut self,
        id: pdfcraft_engine::DocId,
        kind: crate::optimize_ui::OptimizeKind,
        result: Result<(Arc<Vec<u8>>, String), pdfcraft_engine::EditError>,
        destination: Option<String>,
    ) {
        let Some(doc) = self.session.get(id) else { return };
        let (before, name) = (doc.bytes.len(), optimized_name(&doc.name, kind));
        let folder = doc.path.as_deref().and_then(|p| Path::new(p).parent()).map(Path::to_path_buf);
        let (bytes, detail) = match result {
            Ok(r) => r,
            Err(e) => {
                self.notify_fmt("Couldn't optimize the file: {e}", &[("e", &e.to_string())]);
                return;
            }
        };
        let after = bytes.len();
        if kind == crate::optimize_ui::OptimizeKind::Reduce && after >= before {
            self.notify_fmt(
                "This file is already as small as it can be made ({size}). No copy was saved.",
                &[("size", &crate::panels::human_size(before))],
            );
            return;
        }
        let saved = move |app: &mut PdfCraftApp, place: String| {
            let (b, a) = (crate::panels::human_size(before), crate::panels::human_size(after));
            if after < before {
                let pct = 100.0 * (1.0 - after as f64 / before.max(1) as f64);
                app.notify_fmt(
                    "Saved {place}: {before} → {after} ({pct}% smaller){detail}",
                    &[("place", &place), ("before", &b), ("after", &a), ("pct", &format!("{pct:.0}")), ("detail", &detail)],
                );
            } else {
                // Optimize PDF still saves: what it discards may matter more than the size.
                app.notify_fmt(
                    "Saved {place}: {before} → {after} (not smaller){detail}",
                    &[("place", &place), ("before", &b), ("after", &a), ("detail", &detail)],
                );
            }
        };
        match destination {
            Some(path) => self.write_optimized(path, name, folder, bytes, saved),
            None => self.ask_where_to_save_optimized(name, folder, bytes, saved),
        }
    }

    /// Write an optimized copy to `path`. The copy can have taken minutes: a place that has gone bad since doesn't
    /// lose it, so a failed write asks again.
    fn write_optimized(
        &mut self,
        path: String,
        name: String,
        folder: Option<PathBuf>,
        bytes: Arc<Vec<u8>>,
        saved: impl FnOnce(&mut Self, String) + 'static,
    ) {
        match self.host.files().write(Path::new(&path), &bytes, WriteIntent::SaveAs) {
            Ok(_) => saved(self, path),
            Err(e) => {
                self.notify_fmt("Couldn't write {path}: {e}", &[("path", &path), ("e", &e.to_string())]);
                self.ask_where_to_save_optimized(name, folder, bytes, saved);
            }
        }
    }

    /// Ask where an optimized copy goes and write it there, asking again until it is written or
    /// the question is cancelled.
    fn ask_where_to_save_optimized(
        &mut self,
        name: String,
        folder: Option<PathBuf>,
        bytes: Arc<Vec<u8>>,
        saved: impl FnOnce(&mut Self, String) + 'static,
    ) {
        let mut request = PickRequest::save("reduce").file_name(name.clone()).filter("PDF", &["pdf"]);
        if let Some(folder) = &folder {
            request = request.default_directory(folder.clone());
        }
        self.pick_file(request, None, move |app, _, picked| {
            if let Some(path) = picked.paths().into_iter().next() {
                app.write_optimized(path.to_string_lossy().into_owned(), name, folder, bytes, saved);
            }
        });
    }
}

/// The suggested name of an optimized copy of `document` ("notes (reduced).pdf").
fn optimized_name(document: &str, kind: crate::optimize_ui::OptimizeKind) -> String {
    format!("{} ({}).pdf", stem(document), kind.suffix())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use craft::host::{AppInfo, FileError, Files, Host, ManualJobs, PickId, PickRequest, Picked, ScriptedFiles, WriteIntent};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use crate::PdfCraftApp;

    /// A one-page document showing `text`, with an unencoded content stream long enough that
    /// Reduce File Size makes a smaller copy (it saves none otherwise).
    fn text_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 12 Tf 20 250 Td ({text}) Tj ET\n{}", "q Q\n".repeat(500));
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 300] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// An app on `files` and `jobs` with `notes.pdf` open, in a harness.
    fn reducing(files: Arc<dyn Files>, jobs: Arc<ManualJobs>, source: Vec<u8>) -> Harness<'static, PdfCraftApp> {
        let host = Arc::new(Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).files(files).jobs(jobs).build());
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = PdfCraftApp::for_unit_tests_on(host);
                app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                app.session = std::mem::take(&mut app.session).with_clock(|| 1_700_000_000);
                app.open_bytes(&egui::Context::default(), "notes.pdf", None, source).unwrap();
                app
            }
        });
        h.run_steps(2);
        h
    }

    fn reopened_text(bytes: Vec<u8>) -> String {
        let mut again = pdfcraft_engine::Session::new();
        let id = again.open("notes (reduced).pdf", None, Arc::new(bytes), None).unwrap();
        pdfcraft_engine::export::Exporter::new(again.get(id).unwrap()).text(0).unwrap()
    }

    /// Files whose first `fails` writes are refused, as a place that has gone bad.
    struct FailingWrites {
        inner: Arc<ScriptedFiles>,
        fails: Mutex<usize>,
    }

    impl Files for FailingWrites {
        fn pick(&self, request: PickRequest) -> PickId {
            self.inner.pick(request)
        }
        fn take_answer(&self, id: PickId) -> Option<Picked> {
            self.inner.take_answer(id)
        }
        fn forget(&self, id: PickId) {
            self.inner.forget(id);
        }
        fn pick_pending(&self) -> bool {
            self.inner.pick_pending()
        }
        fn read(&self, path: &Path) -> Result<Vec<u8>, FileError> {
            self.inner.read(path)
        }
        fn write(&self, path: &Path, bytes: &[u8], intent: WriteIntent) -> Result<bool, FileError> {
            let mut fails = self.fails.lock().unwrap_or_else(|e| e.into_inner());
            if *fails > 0 {
                *fails -= 1;
                return Err(FileError::PermissionDenied(path.to_path_buf()));
            }
            drop(fails);
            self.inner.write(path, bytes, intent)
        }
        fn last_folder(&self, purpose: &str) -> Option<PathBuf> {
            self.inner.last_folder(purpose)
        }
        fn save(&self, request: PickRequest, bytes: Vec<u8>) -> PickId {
            self.inner.save(request, bytes)
        }
        fn can_pick(&self) -> bool {
            self.inner.can_pick()
        }
        fn browser_saves(&self) -> bool {
            self.inner.browser_saves()
        }
    }

    #[test]
    fn reduce_file_size_asks_where_to_save_before_it_compresses() {
        let out = PathBuf::from("/reduced/notes (reduced).pdf");
        let files = Arc::new(ScriptedFiles::held());
        let jobs = Arc::new(ManualJobs::new());
        let mut h = reducing(files.clone(), jobs.clone(), text_pdf("Minutes of the harbour board"));
        // The page's own work (its pictures and text) settles first, so the queue below holds only the reduction's.
        while jobs.run_all() > 0 {
            h.run_steps(2);
        }
        let ctx_h = h.ctx.clone();
        h.state_mut().reduce_file_size(&ctx_h);
        h.run_steps(2);
        assert!(h.state().optimize_run.is_none() && jobs.waiting() == 0, "nothing runs until the answer is in");
        assert_eq!(files.open(), 1, "the question is out");
        assert!(files.file(&out).is_none(), "no copy is written before the answer");
        files.answer(Picked::Paths(vec![out.clone()]));
        h.run_steps(2);
        jobs.run_all();
        h.run_steps(3);
        assert_eq!(files.requests().len(), 1, "no second question");
        let written = files.file(&out).expect("the copy is written where the user chose");
        let text = reopened_text(written);
        assert!(text.contains("Minutes of the harbour board"), "the copy reopens with its text: {text:?}");
    }

    /// The answer is for the document it was asked about.
    #[test]
    fn switching_documents_while_reduce_file_size_asks_compresses_nothing() {
        let out = PathBuf::from("/reduced/notes (reduced).pdf");
        let files = Arc::new(ScriptedFiles::held());
        let jobs = Arc::new(ManualJobs::new());
        let mut h = reducing(files.clone(), jobs.clone(), text_pdf("Minutes of the harbour board"));
        let ctx_h = h.ctx.clone();
        h.state_mut().reduce_file_size(&ctx_h);
        h.run_steps(2);
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, "other.pdf", None, text_pdf("Another document")).unwrap();
        files.answer(Picked::Paths(vec![out.clone()]));
        h.run_steps(3);
        jobs.run_all();
        h.run_steps(2);
        assert!(h.state().optimize_run.is_none() && files.file(&out).is_none(), "nothing runs, and nothing is written");
    }

    /// Minutes of work aren't lost because the place chosen first has gone bad.
    #[test]
    fn a_reduced_copy_asks_until_it_can_be_written() {
        let good = PathBuf::from("/good/notes (reduced).pdf");
        let inner = Arc::new(ScriptedFiles::answering([
            Picked::Paths(vec![PathBuf::from("/gone1/notes (reduced).pdf")]),
            Picked::Paths(vec![PathBuf::from("/gone2/notes (reduced).pdf")]),
            Picked::Paths(vec![good.clone()]),
        ]));
        let files = Arc::new(FailingWrites { inner: inner.clone(), fails: Mutex::new(2) });
        let jobs = Arc::new(ManualJobs::new());
        let mut h = reducing(files, jobs.clone(), text_pdf("Minutes of the harbour board"));
        let ctx_h = h.ctx.clone();
        h.state_mut().reduce_file_size(&ctx_h);
        h.run_steps(2);
        jobs.run_all();
        h.run_steps(8);
        assert_eq!(inner.requests().len(), 3, "each unwritable place raises a new question");
        assert!(inner.file(&good).is_some(), "the copy lands at the third place");
    }

    /// A chosen existing file stays as it was when no copy is made.
    #[test]
    fn a_reduced_copy_that_isnt_smaller_leaves_the_chosen_file_alone() {
        let out = PathBuf::from("/out/notes (reduced).pdf");
        let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![out.clone()])]).with_file(out.clone(), b"keep this file".to_vec()));
        let jobs = Arc::new(ManualJobs::new());
        let mut h = reducing(files.clone(), jobs.clone(), text_pdf("Minutes"));
        // A copy of a plain text file is no smaller.
        let ctx_h = h.ctx.clone();
        h.state_mut().open_bytes(&ctx_h, "plain.txt", None, "lorem ipsum ".repeat(500).into_bytes()).unwrap();
        h.run_steps(2);
        let ctx_h = h.ctx.clone();
        h.state_mut().reduce_file_size(&ctx_h);
        h.run_steps(2);
        jobs.run_all();
        h.run_steps(3);
        assert_eq!(files.file(&out).unwrap(), b"keep this file");
        h.get_by_label_contains("No copy was saved");
    }

    #[test]
    fn reduce_file_size_doesnt_ask_while_a_run_is_going() {
        let out = PathBuf::from("/out/notes (reduced).pdf");
        let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![out])]));
        let jobs = Arc::new(ManualJobs::new());
        let mut h = reducing(files.clone(), jobs.clone(), text_pdf("Minutes of the harbour board"));
        let ctx_h = h.ctx.clone();
        h.state_mut().reduce_file_size(&ctx_h);
        h.run_steps(2);
        assert!(h.state().optimize_run.is_some(), "the first run is waiting for its job");
        let ctx_h = h.ctx.clone();
        h.state_mut().reduce_file_size(&ctx_h);
        h.run_steps(2);
        assert_eq!(files.requests().len(), 1, "no second question");
        h.get_by_role_and_label(egui::accesskit::Role::Status, "Optimization is already running");
    }
}
