//! Prepare a form: first-use download, cancellable background detection and one atomic edit.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use crate::{PdfCraftApp, widgets};
    use pdfcraft_engine::{
        DocId,
        form_detection::{self, Detection, Detector, Job, model},
    };
    use std::{
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
    };

    /// The desktop uses the pinned download. An embedding host/test can supply its own transport;
    /// the detector still verifies the installed file before executing it.
    pub type Installer = Arc<dyn Fn(&Path, &mut dyn FnMut(u64, u64) -> bool) -> Result<PathBuf, form_detection::DetectionError> + Send + Sync>;

    #[derive(Clone, Copy, Default)]
    enum Progress {
        #[default]
        Checking,
        Download(u64, u64),
        Loading,
        Detect(usize, usize),
    }
    enum Outcome {
        DownloadNeeded,
        Detected(Detection),
    }
    struct Running {
        receiver: mpsc::Receiver<Result<Outcome, String>>,
        cancel: Arc<AtomicBool>,
        progress: Arc<Mutex<Progress>>,
    }
    impl Drop for Running {
        fn drop(&mut self) {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
    struct Request {
        doc: DocId,
        job: Arc<Job>,
        error: Option<String>,
    }

    #[derive(Default)]
    pub struct State {
        /// Alternate application-data directory for embedding hosts and isolated tests.
        pub directory: Option<PathBuf>,
        pub installer: Option<Installer>,
        cache: Arc<Mutex<Option<Arc<Detector>>>>,
        request: Option<Request>,
        running: Option<Running>,
    }
    impl State {
        /// A request owns the modal; a cancelled worker may outlive it.
        pub(crate) fn is_open(&self) -> bool {
            self.request.is_some()
        }
        pub fn is_running(&self) -> bool {
            self.running.is_some()
        }
        pub fn is_prompting(&self) -> bool {
            self.request.is_some() && self.running.is_none()
        }
    }

    impl PdfCraftApp {
        pub fn detect_fields(&mut self) {
            if self.form_detection.is_running() {
                self.notify_tr("Form field detection is already running");
                return;
            }
            if !self.commit_form_typing() {
                return;
            }
            let Some((_, id)) = self.active_ids() else { return };
            if !self.session.get(id).is_some_and(|d| d.allows_modification()) {
                self.notify_tr("This document does not allow adding form fields");
                return;
            }
            match self.session.form_detection_job(id, &[]) {
                Ok(job) => {
                    self.form_detection.request = Some(Request { doc: id, job: Arc::new(job), error: None });
                    self.start_form_detection(false);
                }
                Err(e) => self.notify_error(e),
            }
        }

        fn start_form_detection(&mut self, download: bool) {
            let Some(request) = self.form_detection.request.as_ref() else { return };
            let job = request.job.clone();
            let directory = self.form_detection.directory.clone().map(Ok).unwrap_or_else(model::directory);
            let cache = self.form_detection.cache.clone();
            let installer = self.form_detection.installer.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let progress = Arc::new(Mutex::new(Progress::Checking));
            let (c, p) = (cancel.clone(), progress.clone());
            let (sender, receiver) = mpsc::channel();
            let ctx = self.ctx.clone();
            let work = move || {
                let result = pdfcraft_engine::guard(|| -> Result<Outcome, String> {
                    let dir = directory.map_err(|e| e.to_string())?;
                    let update = |value| -> bool {
                        if let Ok(mut progress) = p.lock() {
                            *progress = value;
                        }
                        !c.load(Ordering::Relaxed)
                    };
                    let mut cached = cache.lock().map_err(|_| "form detection stopped unexpectedly; restart PdfCraft".to_string())?;
                    if cached.is_none() {
                        if !model::available(&dir).map_err(|e| e.to_string())? {
                            if !download {
                                return Ok(Outcome::DownloadNeeded);
                            }
                            let mut report = |done, total| update(Progress::Download(done, total));
                            if let Some(install) = installer { install(&dir, &mut report) } else { model::install(&dir, &mut report) }
                                .map_err(|e| e.to_string())?;
                        }
                        if !update(Progress::Loading) {
                            return Err("form detection was cancelled".into());
                        }
                        *cached = Some(Arc::new(Detector::load(&dir.join(form_detection::MODEL_FILE)).map_err(|e| e.to_string())?));
                    }
                    let detector = cached.as_ref().cloned().ok_or("form detection could not be loaded")?;
                    drop(cached);
                    if c.load(Ordering::Relaxed) {
                        return Err("form detection was cancelled".into());
                    }
                    job.run(&detector, 0.3, |done, total| update(Progress::Detect(done, total))).map(Outcome::Detected).map_err(|e| e.to_string())
                })
                .unwrap_or_else(Err);
                let _ = sender.send(result);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            };
            self.form_detection.running = Some(Running { receiver, cancel, progress });
            if self.run_inline {
                work();
            } else if let Err(e) = std::thread::Builder::new().name("pdfcraft-form-detection".into()).spawn(work) {
                self.form_detection.running = None;
                if let Some(request) = self.form_detection.request.as_mut() {
                    request.error = Some(format!("could not start form detection: {e}"));
                }
            }
            self.poll_form_detection();
        }

        pub fn cancel_form_detection(&mut self) {
            if let Some(run) = &self.form_detection.running {
                run.cancel.store(true, Ordering::Relaxed);
            }
            self.form_detection.request = None;
        }

        pub(crate) fn poll_form_detection(&mut self) {
            if self.form_detection.running.is_none() {
                return;
            }
            if self.form_detection.request.as_ref().is_some_and(|r| self.session.get(r.doc).is_none()) {
                self.cancel_form_detection();
            }
            let Some(run) = self.form_detection.running.as_ref() else { return };
            let result = match run.receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some(ctx) = &self.ctx {
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("form detection stopped unexpectedly; please try again".into()),
            };
            let cancelled = run.cancel.load(Ordering::Relaxed);
            self.form_detection.running = None;
            if cancelled || self.form_detection.request.is_none() {
                return;
            }
            match result {
                Ok(Outcome::DownloadNeeded) => {}
                Err(error) => {
                    if let Some(request) = self.form_detection.request.as_mut() {
                        request.error = Some(error);
                    }
                }
                Ok(Outcome::Detected(found)) => {
                    let doc = self.form_detection.request.take().map(|r| r.doc);
                    match self.session.apply_detected_fields(found) {
                        Ok(names) => {
                            if let Some(doc) = doc {
                                self.form_detection_finished(doc, names.len());
                            }
                        }
                        Err(e) => self.notify_error(e),
                    }
                }
            }
        }
    }

    pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) -> bool {
        if !app.form_detection.is_open() {
            return false;
        }
        let (mut download, mut basic, mut cancel) = (false, false, false);
        let modal = widgets::show_modal(ctx, "form-detection", |ui| {
            ui.set_width(440.0);
            ui.heading(tl!("Find form fields"));
            ui.add_space(10.0);
            if let Some(run) = &app.form_detection.running {
                let progress = run.progress.lock().map(|p| *p).unwrap_or_default();
                match progress {
                    Progress::Checking => {
                        ui.spinner();
                        ui.label(tl!("Checking form detection…"));
                    }
                    Progress::Loading => {
                        ui.spinner();
                        ui.label(tl!("Preparing form detection…"));
                    }
                    Progress::Download(done, total) => {
                        ui.label(tl!("Downloading form detection…"));
                        let fraction = if total == 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 };
                        ui.add(egui::ProgressBar::new(fraction).text(format!(
                            "{:.1} / {:.1} MB",
                            done as f64 / 1_000_000.0,
                            total as f64 / 1_000_000.0
                        )));
                    }
                    Progress::Detect(done, total) => {
                        let page = done.saturating_add(1).min(total);
                        ui.label(crate::i18n::fmt(tl!("Finding fields… page {d} of {t}"), &[("d", &page.to_string()), ("t", &total.to_string())]));
                        ui.add(egui::ProgressBar::new(if total == 0 { 0.0 } else { page as f32 / total as f32 }));
                    }
                }
                ui.add_space(12.0);
                cancel = widgets::pill_button(ui, tl!("Cancel"), false).clicked();
            } else {
                ui.label(tl!("Download 121 MB to find text fields, checkboxes and signature areas automatically."));
                ui.add_space(6.0);
                ui.label(tl!("This is a one-time download from GitHub. Afterward, form detection works offline. Your PDFs stay on this device."));
                if let Some(error) = app.form_detection.request.as_ref().and_then(|r| r.error.as_ref()) {
                    ui.add_space(8.0);
                    ui.label(error);
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let retry = app.form_detection.request.as_ref().is_some_and(|r| r.error.is_some());
                    download = widgets::pill_button(ui, if retry { tl!("Try again") } else { tl!("Download and detect") }, true).clicked();
                    basic = widgets::pill_button(ui, tl!("Use basic detection"), false).clicked();
                    cancel = widgets::pill_button(ui, tl!("Cancel"), false).clicked();
                });
            }
        });
        if cancel || modal.should_close() {
            app.cancel_form_detection();
        } else if basic {
            if let Some(request) = app.form_detection.request.take() {
                app.detect_fields_basic(request.doc);
            }
        } else if download {
            app.start_form_detection(true);
        }
        true
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use egui_kittest::{
            Harness,
            kittest::{NodeT, Queryable},
        };

        #[test]
        fn detection_bar_tracks_the_displayed_page_through_the_last_page() {
            let progress = Arc::new(Mutex::new(Progress::Detect(0, 10)));
            let shown = progress.clone();
            let (_sender, receiver) = mpsc::channel();
            let mut h = Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_eframe(move |_cc| {
                let mut app = PdfCraftApp::new();
                app.set_option("language", "en").unwrap();
                let bytes = app.session.create_from_text("form", "Original form fixture").unwrap();
                app.open_bytes("form.pdf", None, bytes.to_vec()).unwrap();
                let doc = app.active_ids().unwrap().1;
                let job = app.session.form_detection_job(doc, &[]).unwrap();
                app.form_detection.request = Some(Request { doc, job: Arc::new(job), error: None });
                app.form_detection.running = Some(Running { receiver, cancel: Arc::new(AtomicBool::new(false)), progress: shown });
                app
            });
            for (done, percent) in [(0, 10.0), (9, 100.0), (10, 100.0)] {
                *progress.lock().unwrap() = Progress::Detect(done, 10);
                h.run_steps(2);
                let bar = h.get_by_role(egui::accesskit::Role::ProgressIndicator);
                assert_eq!(bar.accesskit_node().numeric_value(), Some(percent), "completed pages: {done}");
                assert!(bar.rect().width() >= 439.0, "the bar spans the dialog content");
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::dialog;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{Installer, State};

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub struct State;
#[cfg(target_arch = "wasm32")]
impl State {
    pub(crate) fn is_open(&self) -> bool {
        false
    }
    pub fn is_running(&self) -> bool {
        false
    }
    pub fn is_prompting(&self) -> bool {
        false
    }
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn dialog(_app: &mut crate::PdfCraftApp, _ctx: &egui::Context) -> bool {
    false
}
#[cfg(target_arch = "wasm32")]
impl crate::PdfCraftApp {
    pub fn detect_fields(&mut self) {
        if !self.commit_form_typing() {
            return;
        }
        if let Some((_, id)) = self.active_ids() {
            self.detect_fields_basic(id);
        }
    }
    pub(crate) fn poll_form_detection(&mut self) {}
    pub fn cancel_form_detection(&mut self) {}
}
