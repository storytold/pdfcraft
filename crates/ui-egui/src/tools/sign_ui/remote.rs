//! The GUI holds an immutable operation while authorization happens in the provider browser.
use std::path::PathBuf;

use pdfcraft_engine::{CompletedSignature, DocId, PendingSignature, SignOptions, sign};
use pdfcraft_platform::signing_browser::{SigningExchange, installed_browser_transport};

use super::{PdfCraftApp, SignGraphic};

pub(crate) struct RemoteSignature {
    pending: Option<PendingSignature>,
    completed: Option<CompletedSignature>,
    browser: Option<Box<dyn SigningExchange>>,
    path: PathBuf,
    graphic: SignGraphic,
    error: Option<String>,
}

impl Drop for RemoteSignature {
    fn drop(&mut self) {
        if let Some(browser) = &self.browser {
            browser.cancel();
        }
    }
}

impl PdfCraftApp {
    pub(super) fn begin_id_austria(&mut self, doc: DocId, opts: SignOptions, graphic: SignGraphic, path: PathBuf) -> Result<(), String> {
        if self.remote_signature.is_some() {
            return Err("Finish or cancel the current ID Austria operation first.".into());
        }
        if path.symlink_metadata().is_ok() {
            return Err("Choose a new output file; the destination already exists.".into());
        }
        // Fail before asking for authorization when the destination folder cannot be written.
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
        drop(tempfile::NamedTempFile::new_in(parent).map_err(|e| format!("Could not prepare the output folder: {e}"))?);
        let pending = self.session.prepare_external_signature(doc, None, opts, sign::SignatureProfile::CmsDetached).map_err(|e| e.to_string())?;
        let xml = sign::security_layer::cms_request(pending.prepared()).map_err(|e| e.to_string())?;
        let transport = self.remote_signing_transport.clone().unwrap_or_else(installed_browser_transport);
        let browser = transport(xml)?;
        self.remote_signature = Some(RemoteSignature { pending: Some(pending), completed: None, browser: Some(browser), path, graphic, error: None });
        Ok(())
    }

    pub(crate) fn poll_id_austria(&mut self, ctx: &egui::Context) {
        let Some(mut job) = self.remote_signature.take() else { return };
        if let Some(response) = job.browser.as_ref().and_then(|browser| browser.poll()) {
            job.browser = None;
            let result = response.and_then(|xml| sign::security_layer::cms_response(&xml).map_err(|e| e.to_string())).and_then(|cms| {
                let pending = job.pending.take().ok_or("The prepared document is no longer available.")?;
                self.session.complete_external_signature(pending, &cms).map_err(|e| e.to_string())
            });
            match result {
                Ok(completed) => job.completed = Some(completed),
                Err(e) => {
                    job.pending = None;
                    job.error = Some(e);
                }
            }
        }
        let mut open = true;
        let mut cancel = false;
        let mut save = job.completed.is_some() && job.error.is_none();
        let mut choose_path = false;
        egui::Window::new(tl!("ID Austria signature"))
            .id(egui::Id::unique("id-austria-operation"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                if job.browser.is_some() {
                    ui.spinner();
                    ui.label(tl!("Approve signing in the provider browser. This window stays responsive."));
                }
                if let Some(error) = &job.error {
                    ui.label(tl!(error));
                    if job.completed.is_some() {
                        ui.label(tl!("The checked signature is retained. Choose a new output path or retry saving without authorizing again."));
                        if ui.button(tl!("Retry saving")).clicked() {
                            save = true;
                        }
                        if ui.button(tl!("Choose another output file…")).clicked() {
                            choose_path = true;
                        }
                    }
                }
                if ui.button(tl!("Cancel")).clicked() {
                    cancel = true;
                }
            });
        if !open || cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return;
        }
        if save {
            match self.save_remote_signature(&job) {
                Ok(()) => return,
                Err(e) => job.error = Some(e),
            }
        }
        self.remote_signature = Some(job);
        if choose_path {
            let dialog = rfd::AsyncFileDialog::new().set_file_name("signed.pdf");
            let expected = self.remote_signature.as_ref().and_then(|j| j.completed.as_ref()).map(|c| c.bytes().clone());
            self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, path| {
                if let Some(job) = app.remote_signature.as_mut()
                    && job.completed.as_ref().zip(expected.as_ref()).is_some_and(|(c, b)| std::sync::Arc::ptr_eq(c.bytes(), b))
                {
                    job.path = path;
                    job.error = None;
                }
            });
        }
        if self.remote_signature.as_ref().is_some_and(|job| job.browser.is_some()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn save_remote_signature(&mut self, job: &RemoteSignature) -> Result<(), String> {
        let completed = job.completed.as_ref().ok_or("Authorization is not complete.")?;
        self.session.check_completed_signature(completed).map_err(|e| e.to_string())?;
        pdfcraft_platform::staging::write_new_atomically(&job.path, completed.bytes()).map_err(|e| format!("Could not save: {e}"))?;
        let doc = completed.doc();
        self.session.mark_signed(doc, completed.bytes().clone(), Some(job.path.to_string_lossy().into_owned())).map_err(|e| e.to_string())?;
        self.watch.known_now(doc, &job.path.to_string_lossy());
        #[cfg(target_os = "macos")]
        crate::macos_association::repair(&job.path);
        self.sign_graphic = job.graphic.clone();
        if let Some(view) = self.views.iter_mut().find(|v| v.id == doc) {
            view.invalidate_content();
        }
        self.right = Some(crate::RightPanel::Signatures);
        self.notify_fmt("Signed and saved to {path}", &[("path", &job.path.display().to_string())]);
        Ok(())
    }
}
