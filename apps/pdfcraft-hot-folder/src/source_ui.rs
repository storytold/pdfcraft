//! Source settings and explicit asynchronous authentication. Passwords never enter task JSON.
use crate::i18n::Language;
use pdfcraft_engine::{hot_folder as hf, hot_folder_sources as source};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

enum Job {
    Password(String, Zeroizing<String>),
    Forget(String),
    Status(String),
    Begin(source::OutlookSource),
    Poll(source::OutlookSource, source::DeviceSignIn),
    Folders(source::OutlookSource, Option<String>),
}
enum Event {
    Saved,
    Forgotten,
    Status(bool),
    Begun(source::OutlookSource, source::DeviceSignIn),
    Polled(source::SignInStatus),
    Folders(Vec<source::MailFolder>),
}
struct SignIn {
    source: source::OutlookSource,
    code: source::DeviceSignIn,
    next: Instant,
}
#[derive(Default)]
pub struct SourceUi {
    password: String,
    pending: Option<Job>,
    receiver: Option<Receiver<Result<Event, String>>>,
    sign_in: Option<SignIn>,
    pub error: Option<String>,
    message: Option<&'static str>,
    folders: Vec<source::MailFolder>,
}
impl Drop for SourceUi {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}
impl SourceUi {
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }
    pub fn begin_pending(&mut self, config: &Path) -> Result<(), String> {
        let Some(job) = self.pending.take() else { return Ok(()) };
        let store = source::CredentialStore::for_config(config, None)?;
        self.spawn(job, store);
        Ok(())
    }
    fn spawn(&mut self, job: Job, store: source::CredentialStore) {
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.error = None;
        self.message = None;
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                Job::Password(id, password) => source::save_ftp_password(&store, &id, &password).map(|_| Event::Saved),
                Job::Forget(id) => source::forget_credentials(&store, &id).map(|_| Event::Forgotten),
                Job::Status(id) => source::credential_status(&store, &id).map(Event::Status),
                Job::Begin(settings) => source::outlook_begin_sign_in(&store, &settings).map(|code| Event::Begun(settings, code)),
                Job::Poll(settings, code) => source::outlook_poll_sign_in(&store, &settings, &code).map(Event::Polled),
                Job::Folders(settings, parent) => source::outlook_folders(&store, &settings, parent.as_deref(), &|| false).map(Event::Folders),
            }))
            .unwrap_or_else(|_| Err("source authentication failed safely".into()));
            let _ = tx.send(result);
        });
    }
    pub fn tick(&mut self, config: &Path) {
        let result = self.receiver.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(value) => Some(value),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("source worker stopped unexpectedly".into())),
        });
        if let Some(result) = result {
            self.receiver = None;
            match result {
                Err(error) => {
                    self.error = Some(error);
                    self.sign_in = None;
                }
                Ok(Event::Saved) => self.message = Some("Credentials saved"),
                Ok(Event::Forgotten) => {
                    self.message = Some("Credentials removed");
                    self.sign_in = None;
                }
                Ok(Event::Status(found)) => self.message = Some(if found { "Credentials available" } else { "Sign-in required" }),
                Ok(Event::Begun(settings, code)) => {
                    self.sign_in = Some(SignIn { next: Instant::now() + Duration::from_secs(code.interval_seconds.max(1)), source: settings, code });
                }
                Ok(Event::Polled(source::SignInStatus::Pending { retry_after_seconds })) => {
                    if let Some(sign) = &mut self.sign_in {
                        sign.next = Instant::now() + Duration::from_secs(retry_after_seconds.max(1));
                    }
                }
                Ok(Event::Polled(status)) => {
                    self.sign_in = None;
                    self.message = Some(match status {
                        source::SignInStatus::Authorized => "Signed in",
                        source::SignInStatus::Expired => "Sign-in expired",
                        _ => "Sign-in declined",
                    });
                }
                Ok(Event::Folders(folders)) => self.folders = folders,
            }
        }
        if self.receiver.is_none()
            && let Some(sign) = &self.sign_in
            && Instant::now() >= sign.next
        {
            let job = Job::Poll(sign.source.clone(), sign.code.clone());
            match source::CredentialStore::for_config(config, None) {
                Ok(store) => self.spawn(job, store),
                Err(error) => {
                    self.error = Some(error);
                    self.sign_in = None;
                }
            }
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, task: &mut hf::Task, lang: Language) {
        let t = |key| lang.t(key);
        ui.heading(t("Source"));
        let mut kind = match &task.source {
            source::SourceConfig::Local => 0,
            source::SourceConfig::Ftp(_) => 1,
            source::SourceConfig::Outlook(_) => 2,
            source::SourceConfig::Project(_) => 3,
        };
        let old = kind;
        let names = ["Local or network folder", "FTP / FTPS", "Outlook attachments", "PDFCraft project"];
        egui::ComboBox::from_id_salt("source-kind").selected_text(t(names.get(kind).copied().unwrap_or(names[0]))).show_ui(ui, |ui| {
            for (index, name) in names.iter().enumerate() {
                ui.selectable_value(&mut kind, index, t(name));
            }
        });
        if kind != old {
            task.source = match kind {
                1 => source::SourceConfig::Ftp(source::FtpSource { credential_id: format!("{}-ftp", task.id), ..Default::default() }),
                2 => source::SourceConfig::Outlook(source::OutlookSource { credential_id: format!("{}-mail", task.id), ..Default::default() }),
                3 => source::SourceConfig::Project(Default::default()),
                _ => source::SourceConfig::Local,
            };
            task.originals = hf::Originals::Keep;
            self.folders.clear();
        }
        ui.add_enabled_ui(self.receiver.is_none(), |ui| match &mut task.source {
            source::SourceConfig::Local => {}
            source::SourceConfig::Ftp(settings) => {
                line(ui, t("Server"), &mut settings.host);
                ui.horizontal(|ui| {
                    ui.label(t("Port"));
                    ui.add(egui::DragValue::new(&mut settings.port).range(1..=65535));
                });
                ui.horizontal_wrapped(|ui| {
                    crate::widgets::radio_value(ui, &mut settings.security, source::FtpSecurity::ExplicitTls, t("FTP with TLS"));
                    crate::widgets::radio_value(ui, &mut settings.security, source::FtpSecurity::Plain, t("Unencrypted FTP"));
                });
                line(ui, t("Username"), &mut settings.username);
                line(ui, t("Server folder"), &mut settings.folder);
                ui.add(crate::widgets::checkbox(&mut settings.recursive, t("Include subfolders")));
                line(ui, t("Credential ID"), &mut settings.credential_id);
                ui.label(t("Password"));
                ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                if ui.button(t("Save password securely")).clicked() {
                    self.pending = Some(Job::Password(settings.credential_id.clone(), Zeroizing::new(std::mem::take(&mut self.password))));
                }
                self.credential_buttons(ui, &settings.credential_id, lang);
                action(ui, &mut settings.action, t("Archive server folder"), lang);
            }
            source::SourceConfig::Outlook(settings) => {
                line(ui, t("Application ID (public client)"), &mut settings.client_id);
                line(ui, t("Tenant"), &mut settings.tenant);
                line(ui, t("Credential ID"), &mut settings.credential_id);
                ui.weak(t("Use your own Microsoft application registration with public-client device sign-in enabled."));
                ui.horizontal_wrapped(|ui| {
                    if ui.button(t("Sign in to Outlook")).clicked() {
                        self.pending = Some(Job::Begin(settings.clone()));
                    }
                    if ui.button(t("Load mail folders")).clicked() {
                        self.pending = Some(Job::Folders(settings.clone(), None));
                    }
                });
                self.credential_buttons(ui, &settings.credential_id, lang);
                line(ui, t("Mail folder ID"), &mut settings.folder_id);
                for folder in &self.folders {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(settings.folder_id == folder.id, &folder.name).clicked() {
                            settings.folder_id = folder.id.clone();
                        }
                        if folder.child_count > 0 && ui.button(t("Open subfolders")).clicked() {
                            self.pending = Some(Job::Folders(settings.clone(), Some(folder.id.clone())));
                        }
                    });
                }
                action(ui, &mut settings.action, t("Archive mail folder ID"), lang);
                ui.weak(t("Mail move/delete affects the whole message and waits for all supported attachments. Enable PDF and images."));
            }
            source::SourceConfig::Project(settings) => {
                let mut path = settings.manifest.to_string_lossy().into_owned();
                line(ui, t("Project manifest"), &mut path);
                settings.manifest = path.into();
                ui.horizontal_wrapped(|ui| {
                    if ui.button(t("Choose project…")).clicked()
                        && let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file()
                    {
                        settings.manifest = path;
                    }
                    if ui.button(t("Create project from files…")).clicked() {
                        match create_project() {
                            Ok(Some(path)) => settings.manifest = path,
                            Ok(None) => {}
                            Err(error) => self.error = Some(error),
                        }
                    }
                });
                ui.weak(t("Open PDFCraft manifests contain an ordered list of PDF/image files. Proprietary ABBYY projects are not supported."));
                action(ui, &mut settings.action, t("Archive folder"), lang);
            }
        });
        if self.receiver.is_some() {
            ui.spinner();
            ui.label(t("Connecting…"));
        }
        if let Some(sign) = &self.sign_in {
            ui.label(t("Open the sign-in page and enter this code:"));
            ui.hyperlink_to(&sign.code.verification_uri, &sign.code.verification_uri);
            ui.monospace(&sign.code.user_code);
        }
        if let Some(message) = self.message {
            ui.label(t(message));
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::DARK_RED, t("Error details"));
            ui.label(error);
        }
    }
    fn credential_buttons(&mut self, ui: &mut egui::Ui, id: &str, lang: Language) {
        ui.horizontal_wrapped(|ui| {
            if ui.button(lang.t("Check credentials")).clicked() {
                self.pending = Some(Job::Status(id.into()));
            }
            if ui.button(lang.t("Remove credentials")).clicked() {
                self.pending = Some(Job::Forget(id.into()));
            }
        });
        ui.weak(lang.t("Credentials stay in the protected service store and are never exported with tasks."));
    }
}
fn line(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.text_edit_singleline(value);
}
fn action(ui: &mut egui::Ui, value: &mut source::SourceAction, destination_label: &str, lang: Language) {
    let t = |key| lang.t(key);
    ui.label(t("After processing originals"));
    let mut kind = match value {
        source::SourceAction::Keep => 0,
        source::SourceAction::Move { .. } => 1,
        source::SourceAction::Delete => 2,
    };
    let old = kind;
    ui.horizontal_wrapped(|ui| {
        for (index, name) in ["Keep", "Move after success", "Delete after success"].iter().enumerate() {
            crate::widgets::radio_value(ui, &mut kind, index, t(name));
        }
    });
    if old != kind {
        *value = match kind {
            1 => source::SourceAction::Move { destination: String::new() },
            2 => source::SourceAction::Delete,
            _ => source::SourceAction::Keep,
        };
    }
    if let source::SourceAction::Move { destination } = value {
        line(ui, destination_label, destination);
    }
}
fn create_project() -> Result<Option<std::path::PathBuf>, String> {
    let Some(files) = rfd::FileDialog::new().add_filter("PDF / images", &["pdf", "png", "jpg", "jpeg", "tif", "tiff"]).pick_files() else {
        return Ok(None);
    };
    let Some(first) = files.first().and_then(|p| p.parent()) else { return Ok(None) };
    let Some(path) = rfd::FileDialog::new().set_directory(first).set_file_name("hot-folder-project.json").add_filter("JSON", &["json"]).save_file()
    else {
        return Ok(None);
    };
    let parent = path.parent().ok_or("project needs a parent folder")?;
    let relative = files
        .iter()
        .map(|file| {
            file.strip_prefix(parent)
                .map(Path::to_path_buf)
                .map_err(|_| "save the project in a folder containing all selected source files".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    source::write_project_manifest(&path, &source::ProjectManifest { files: relative, ..Default::default() }, None)?;
    Ok(Some(path))
}
