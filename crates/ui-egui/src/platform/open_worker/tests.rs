//! The open pipeline through the app: every open runs as a job on a `ManualJobs` queue, so a test
//! holds a document's preparation, looks at the window while it waits, and releases it with
//! `run_next`. Pickers are held `ScriptedFiles` picks answered as a user would, and prompts are
//! found on screen by their buttons.
//!
//! PLAN: the password dialog's labels ("Password", "Open", "Cancel") and the link prompt's "Allow"
//! are assumed.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use craft::automation::{ControlChannel, ControlRequest};
use craft::host::{AppInfo, Host, ManualJobs, Picked, ScriptedFiles, Wake};
use craft::platform::clipboard::{Clipboard, SystemClipboard};
use craft::platform::open::{FileIntake, IntakeSender};
use craft::platform::watch::FileWatch;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

use crate::canvas::PageDisplay;
use crate::{AppServices, PdfCraftApp};

fn fixture(pages: usize) -> Vec<u8> {
    let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    let kids = (0..pages).map(|page| format!("{} 0 R", 4 + 2 * page)).collect::<Vec<_>>().join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {pages} /MediaBox [0 0 200 300] >>"));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for page in 0..pages {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * page));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", page + 1);
        objects.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn encrypted_fixture() -> Vec<u8> {
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open("secret.pdf", None, Arc::new(fixture(2)), None).expect("open the synthetic PDF");
    session
        .apply(id, pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection { open_password: Some("tool-password".into()), ..Default::default() }))
        .expect("encrypt the synthetic PDF");
    session.save_bytes(id).expect("write the encrypted synthetic PDF").as_ref().clone()
}

/// A synthetic input file in the temp folder, removed on drop.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(case: &str, extension: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!("pdfcraft-open-tool-{}-{case}.{extension}", std::process::id()));
        std::fs::write(&path, bytes).expect("write the synthetic input");
        Self(path)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    fn name(&self) -> String {
        self.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The app in a window, with its held jobs and picks, the OS intake and the control channel.
struct Rig {
    h: Harness<'static, PdfCraftApp>,
    jobs: Arc<ManualJobs>,
    files: Arc<ScriptedFiles>,
    intake: IntakeSender,
    control: Sender<ControlRequest>,
}

impl Rig {
    /// The app with `existing.pdf` (three pages) open.
    fn new() -> Self {
        let jobs = Arc::new(ManualJobs::new());
        let files = Arc::new(ScriptedFiles::held());
        let (control, rx) = std::sync::mpsc::channel();
        let (intake, sender) = FileIntake::new(Wake::none());
        let parts = Mutex::new(Some((rx, intake)));
        let (host_jobs, host_files) = (jobs.clone(), files.clone());
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let (rx, intake) = parts.lock().expect("setup slot").take().expect("built once");
                let host = Host::builder(AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" })
                    .jobs(host_jobs.clone())
                    .files(host_files.clone())
                    .build();
                let clipboard = Clipboard::new(SystemClipboard::new());
                let services = AppServices::new(Wake::none(), clipboard, None, None, intake, FileWatch::new(Wake::none()).inline());
                let mut app = PdfCraftApp::with_host(Arc::new(host), services);
                app.set_option(&cc.egui_ctx, "language", "en").expect("language preference");
                app.attach_control(ControlChannel::new(rx), None);
                app.open_bytes(&egui::Context::default(), "existing.pdf", None, fixture(3)).expect("existing document");
                app
            }
        });
        h.run_steps(3);
        // The host's queue also carries the page text layers the window asks for; they finish first, so that the
        // oldest waiting job in a test is the open it holds.
        for _ in 0..2 {
            jobs.run_all();
            h.run_steps(2);
        }
        Self { h, jobs, files, intake: sender, control }
    }

    /// The app with no document open.
    fn empty() -> Self {
        let mut rig = Self::new();
        rig.h.state_mut().close_tab(0);
        rig.h.run_steps(2);
        rig
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let (request, reply) = ControlRequest::new(method, params);
        self.control.send(request).expect("the control channel is open");
        for _ in 0..5 {
            self.h.step();
            if let Ok(reply) = reply.try_recv() {
                return reply.get("result").cloned().unwrap_or(reply);
            }
        }
        panic!("{method} was not answered while a job was held");
    }

    /// Runs the next open, including any unrelated host jobs queued ahead of it, and shows its result.
    fn release(&mut self) {
        let finished = self.h.state().opens.counts().finished;
        for _ in 0..100 {
            assert!(self.jobs.run_next(), "an open job is waiting");
            if self.h.state().opens.counts().finished > finished {
                self.h.run_steps(2);
                return;
            }
        }
        panic!("the open job did not finish");
    }

    fn tabs(&self) -> usize {
        self.h.state().views.len()
    }

    fn has_doc(&self, name: &str) -> bool {
        self.h.state().session.docs().iter().any(|doc| doc.name == name)
    }

    fn active_name(&self) -> Option<String> {
        let app = self.h.state();
        app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|doc| doc.name.clone())
    }

    fn export_open(&self) -> bool {
        crate::export_ui::open_kind(self.h.state()).is_some()
    }

    /// Whether a notice on screen says `text`.
    fn notice_says(&self, text: &str) -> bool {
        use egui_kittest::kittest::NodeT as _;
        self.h.query_all_by_label_contains(text).any(|node| node.accesskit_node().role() == egui::accesskit::Role::Status)
    }

    fn password_dialog(&self) -> bool {
        self.h.query_all_by_label("Password").next().is_some()
    }

    fn press(&mut self, label: &str) {
        self.h.get_all_by_label(label).last().unwrap_or_else(|| panic!("no {label} button on screen")).click();
        self.h.run_steps(2);
    }

    /// Types `password` into the dialog and presses Open; the retry waits for `release`.
    fn enter_password(&mut self, password: &str) {
        self.h.get_by_role_and_label(egui::accesskit::Role::PasswordInput, "Password").focus();
        self.h.run_steps(1);
        self.h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        self.h.run_steps(1);
        self.h.get_by_role_and_label(egui::accesskit::Role::PasswordInput, "Password").type_text(password);
        self.h.run_steps(1);
        self.press("Open");
    }

    /// Picks the tool `command`, which asks for its file, and answers the picker with `file`.
    fn choose(&mut self, command: &str, file: &TempFile) {
        let ctx = self.h.ctx.clone();
        self.h.state_mut().run_command(&ctx, command);
        self.h.run_steps(2);
        assert!(self.files.answer(Picked::Paths(vec![file.0.clone()])).is_some(), "the picker is open");
        self.h.run_steps(2);
    }

    fn layout_of(&self, name: &str) -> Option<(PageDisplay, bool)> {
        let app = self.h.state();
        app.views.iter().find(|v| app.session.get(v.id).is_some_and(|d| d.name == name)).map(|v| (v.view.display, v.view.continuous))
    }
}

#[test]
fn open_prepare_keeps_control_and_existing_tabs_live_until_atomic_commit() {
    let mut rig = Rig::new();
    let file = TempFile::new("control", "pdf", &fixture(2));
    let reply = rig.request("ui.open", json!({ "path": file.path() }));
    assert_eq!(reply["pending"], true);
    assert_eq!(rig.jobs.waiting(), 1);

    let state = rig.request("ui.state", json!({}));
    assert_eq!(state["documents"].as_array().map(Vec::len), Some(1));
    assert_eq!(state["documents"][0]["name"], "existing.pdf");
    assert_eq!(state["pending_opens"].as_array().map(Vec::len), Some(1));
    rig.request("ui.command", json!({ "id": "page.insert_blank" }));
    let app = rig.h.state();
    assert_eq!(app.session.get(app.views[0].id).expect("existing tab").info.pages.len(), 4);
    assert_eq!(rig.tabs(), 1, "prepared state is not visible in the session");

    rig.release();
    let app = rig.h.state();
    assert_eq!(app.views.len(), 2, "one prepared document commits exactly once");
    assert_eq!(app.session.get(app.views[1].id).expect("new tab").info.pages.len(), 2);
    assert_eq!(app.active, Some(1));
}

/// Picking a file for a tool must carry the tool through native preparation; another file
/// opened while it is pending must not become the tool's source.
fn chosen_tool_runs_after_preparation(extension: &str, bytes: Vec<u8>) {
    let mut rig = Rig::empty();
    let file = TempFile::new("pending", extension, &bytes);
    rig.choose("export.text", &file);
    assert_eq!(rig.tabs(), 0);
    assert!(!rig.export_open(), "the tool cannot run before its document exists");
    let ctx_h = rig.h.ctx.clone();
    rig.h.state_mut().open_bytes(&ctx_h, "other.pdf", None, fixture(3)).expect("another document opens while preparation is held");
    let other = rig.h.state().active_ids().expect("the other document is active").1;
    rig.release();
    let app = rig.h.state();
    let source = app.active_ids().expect("the chosen file becomes active").1;
    assert_ne!(source, other);
    assert_eq!(app.views.len(), 2, "the chosen file commits once and retains the other document");
    assert_eq!(app.session.get(other).expect("the other document is retained").info.pages.len(), 3);
    assert!(!app.session.get(other).expect("the other document is retained").dirty);
    let chosen = app.session.get(source).expect("the tool's source exists");
    if extension == "pdf" {
        assert_eq!(chosen.path.as_deref(), Some(file.path().as_str()));
        assert_eq!(chosen.info.pages.len(), 2);
        assert_eq!(chosen.bytes.as_slice(), bytes.as_slice(), "the selected PDF's contents are retained");
    } else {
        assert!(chosen.dirty && chosen.path.is_none(), "conversion produces a new unsaved PDF");
        assert_eq!(chosen.info.pages.len(), 1);
    }
    assert!(rig.export_open(), "the chosen Export tool runs after its source commits");
    assert!(rig.h.state().session.docs().len() == 2);
}

#[test]
fn a_chosen_tool_runs_on_its_file_only_after_native_preparation_commits() {
    chosen_tool_runs_after_preparation("pdf", fixture(2));
}

#[test]
fn a_chosen_tool_runs_on_converted_text_only_after_native_preparation_commits() {
    chosen_tool_runs_after_preparation("txt", b"Synthetic text input for the selected export tool".to_vec());
}

#[test]
fn a_chosen_tool_survives_wrong_password_retry_and_runs_after_unlock() {
    let mut rig = Rig::empty();
    let file = TempFile::new("encrypted-retry", "pdf", &encrypted_fixture());
    rig.choose("export.text", &file);
    rig.release();
    assert!(rig.password_dialog());
    assert!(rig.tabs() == 0 && !rig.export_open());

    rig.enter_password("wrong-password");
    assert!(!rig.export_open(), "the tool waits for authentication to succeed");
    rig.release();
    assert!(rig.password_dialog(), "the wrong password is reported by the dialog coming back");
    assert!(rig.h.query_by_label_contains("Incorrect password").is_some());
    assert!(rig.tabs() == 0 && !rig.export_open());

    rig.enter_password("tool-password");
    assert!(!rig.export_open());
    rig.release();
    assert!(!rig.password_dialog());
    assert_eq!(rig.tabs(), 1);
    assert_eq!(rig.active_name(), Some(file.name()));
    assert!(rig.export_open(), "the chosen tool survives the wrong-password retry");
}

#[test]
fn cancelling_a_chosen_tools_picker_or_password_never_runs_it_on_another_document() {
    let mut rig = Rig::empty();
    let ctx = rig.h.ctx.clone();
    rig.h.state_mut().run_command(&ctx, "export.text");
    rig.h.run_steps(2);
    assert!(rig.files.answer(Picked::Cancelled).is_some());
    rig.h.run_steps(2);
    assert_eq!(rig.jobs.waiting(), 0);
    assert!(rig.tabs() == 0 && !rig.export_open());

    let file = TempFile::new("encrypted-cancel", "pdf", &encrypted_fixture());
    rig.choose("export.text", &file);
    rig.release();
    assert!(rig.password_dialog());
    rig.press("Cancel");
    let ctx_h = rig.h.ctx.clone();
    rig.h.state_mut().open_bytes(&ctx_h, "other.pdf", None, fixture(3)).expect("a different document opens after cancellation");
    rig.h.run_steps(3);
    assert!(!rig.password_dialog());
    assert_eq!(rig.tabs(), 1);
    assert_eq!(rig.active_name().as_deref(), Some("other.pdf"));
    assert!(!rig.export_open(), "cancelling discards the chosen tool's continuation");
}

#[test]
fn a_chosen_tool_does_not_run_when_native_preparation_rejects_the_file() {
    let mut rig = Rig::empty();
    let file = TempFile::new("invalid", "pdf", b"%PDF-1.7\nnot a PDF object graph");
    rig.choose("export.text", &file);
    rig.release();
    assert_eq!(rig.tabs(), 0);
    assert!(!rig.export_open() && !rig.password_dialog());
    // The notice's wording is the app's; only that an open failure is shown is asserted.
    assert!(rig.notice_says("Couldn't open"), "the open failure is reported");
}

#[test]
fn startup_intake_waits_for_local_authentication_and_keeps_its_own_background_password_prompt() {
    let mut rig = Rig::new();
    let ctx_h = rig.h.ctx.clone();
    rig.h.state_mut().open_bytes(&ctx_h, "local-secret.pdf", None, encrypted_fixture()).expect("the local PDF asks for its password");
    rig.h.run_steps(2);
    rig.intake.open_bytes("startup-secret.pdf".into(), Arc::from(encrypted_fixture()));
    rig.h.run_steps(2);
    assert_eq!(rig.tabs(), 1, "startup bytes wait while the local prompt is answered");
    assert!(rig.password_dialog());

    rig.enter_password("tool-password");
    rig.release();
    assert_eq!(rig.active_name().as_deref(), Some("local-secret.pdf"));
    // The startup file now asks for its own password, in the background.
    rig.h.run_steps(2);
    assert!(rig.password_dialog(), "the startup file has its own prompt");
    rig.enter_password("tool-password");
    rig.release();
    assert_eq!(rig.tabs(), 3);
    assert_eq!(rig.active_name().as_deref(), Some("local-secret.pdf"), "the startup document does not take the focus");
}

#[test]
fn startup_intake_waits_through_wrong_password_retry_and_cancel_preserves_the_existing_document() {
    let mut rig = Rig::new();
    let ctx_h = rig.h.ctx.clone();
    rig.h.state_mut().open_bytes(&ctx_h, "local-secret.pdf", None, encrypted_fixture()).expect("the local PDF asks for its password");
    rig.h.run_steps(2);
    rig.enter_password("wrong-password");
    rig.intake.open_bytes("startup.pdf".into(), Arc::from(fixture(1)));
    rig.h.run_steps(2);
    assert_eq!(rig.tabs(), 1, "startup bytes wait while the password is checked");

    rig.release();
    assert!(rig.password_dialog());
    assert!(rig.h.query_by_label_contains("Incorrect password").is_some());
    assert_eq!(rig.tabs(), 1, "startup bytes also wait for the user's retry or cancellation");
    rig.press("Cancel");
    rig.h.run_steps(2);
    assert_eq!(rig.tabs(), 2);
    assert_eq!(rig.active_name().as_deref(), Some("existing.pdf"), "cancelling the local open leaves the startup PDF in the background");
    assert!(!rig.has_doc("local-secret.pdf"));
    assert!(rig.has_doc("startup.pdf"));
}

fn tool_runs_after_background_open(extension: &str, bytes: Vec<u8>, pages: usize) {
    let mut rig = Rig::empty();
    let file = TempFile::new("home", extension, &bytes);
    rig.choose("export.text", &file);
    assert!(rig.h.state().active.is_none());
    assert!(!rig.export_open(), "the tool waits for its document");
    rig.release();
    let app = rig.h.state();
    assert_eq!(app.views.len(), 1, "the chosen document opens once");
    assert_eq!(app.session.get(app.views[0].id).expect("chosen document").info.pages.len(), pages);
    assert!(rig.export_open(), "the chosen tool runs after preparation");
    rig.h.run_steps(2);
    assert_eq!(rig.tabs(), 1);
    assert_eq!(std::fs::read(&file.0).expect("read unchanged input"), bytes);
}

#[test]
fn tool_from_home_runs_after_pdf_preparation() {
    tool_runs_after_background_open("pdf", fixture(2), 2);
}

#[test]
fn tool_from_home_runs_after_text_conversion() {
    tool_runs_after_background_open("txt", b"Synthetic document for a chosen tool".to_vec(), 1);
}

#[test]
fn a_startup_document_waits_for_an_active_document_to_prepare() {
    let mut rig = Rig::empty();
    let file = TempFile::new("startup-order", "pdf", &fixture(2));
    rig.h.state_mut().open_path(&file.path());
    rig.intake.open_bytes("startup.pdf".into(), Arc::from(fixture(1)));
    rig.h.run_steps(2);
    assert_eq!(rig.tabs(), 0, "startup bytes wait for the pending active open");
    rig.release();
    rig.h.run_steps(2);
    assert_eq!(rig.tabs(), 2);
    assert_eq!(rig.active_name(), Some(file.name()), "the chosen document keeps the focus");
}

#[test]
fn tool_modal_keeps_picked_recipient_when_other_open_is_ready() {
    // Both results ready in one frame, then one after the other.
    for together in [true, false] {
        let mut rig = Rig::empty();
        let (a, b) = (TempFile::new("modal-a", "pdf", &fixture(2)), TempFile::new("modal-b", "pdf", &fixture(1)));
        rig.choose("export.text", &a);
        rig.h.state_mut().open_path(&b.path());
        if together {
            rig.jobs.run_all();
            rig.h.run_steps(2);
        } else {
            rig.release();
            rig.release();
        }
        assert!(rig.export_open(), "the tool's dialog is on screen for the picked document");
        assert_eq!(rig.tabs(), 1, "the other file waits while the tool modal is open");
        assert_eq!(rig.active_name(), Some(a.name()));
        rig.press("Cancel");
        rig.h.run_steps(2);
        assert_eq!(rig.tabs(), 2, "the other file is admitted once, after Cancel");
        assert_eq!(rig.h.state().session.docs().iter().filter(|doc| doc.name == b.name()).count(), 1);
        assert!(!rig.export_open(), "the tool does not run again");
    }
}

#[test]
fn tool_password_prompt_retains_other_ready_open_through_retry() {
    let mut rig = Rig::empty();
    let (a, b) = (TempFile::new("retain-a", "pdf", &encrypted_fixture()), TempFile::new("retain-b", "pdf", &fixture(1)));
    rig.choose("export.text", &a);
    rig.h.state_mut().open_path(&b.path());
    rig.jobs.run_all();
    rig.h.run_steps(2);
    assert!(rig.password_dialog());
    assert_eq!(rig.tabs(), 0, "the other open cannot replace the visible prompt");

    rig.enter_password("wrong-password");
    rig.release();
    assert!(rig.password_dialog(), "the prompt for the picked file comes back with its error");
    assert!(!rig.has_doc(&a.name()));

    rig.enter_password("tool-password");
    rig.release();
    assert!(rig.has_doc(&a.name()));
    assert!(rig.export_open(), "the picked document gets the tool");
    rig.press("Cancel");
    rig.h.run_steps(2);
    assert_eq!(rig.h.state().session.docs().iter().filter(|doc| doc.name == b.name()).count(), 1, "the other tab is admitted once");
}

#[test]
fn tool_picker_keeps_picked_recipient_when_other_open_is_ready() {
    for cancel in [true, false] {
        let mut rig = Rig::new();
        let inserted = TempFile::new("picker-insert", "pdf", &fixture(1));
        let other = TempFile::new("picker-other", "pdf", &fixture(1));
        // The tool's second picker is out while another file finishes opening.
        rig.h.state_mut().pick_files(crate::FilePurpose::InsertPages, true);
        rig.h.run_steps(2);
        rig.h.state_mut().open_path(&other.path());
        rig.release();
        assert_eq!(rig.tabs(), 1, "a ready open waits while the picker is open");
        let answer = if cancel { Picked::Cancelled } else { Picked::Paths(vec![inserted.0.clone()]) };
        assert!(rig.files.answer(answer).is_some());
        rig.h.run_steps(3);
        assert_eq!(rig.tabs(), 2, "the other file is admitted once");
        let app = rig.h.state();
        let pages = app.session.get(app.views[0].id).expect("the picked document").info.pages.len();
        assert_eq!(pages, if cancel { 3 } else { 4 }, "the picker's answer went to the document it was asked for");
    }
}

/// A tool chosen for a file opens with it whether the file is a PDF, text, or locked.
#[test]
fn inline_tool_open_keeps_its_command_for_pdf_conversion_and_password_retry() {
    for (extension, bytes, locked) in
        [("pdf", fixture(2), false), ("txt", b"Synthetic text for the tool".to_vec(), false), ("pdf", encrypted_fixture(), true)]
    {
        let mut rig = Rig::empty();
        let file = TempFile::new("table", extension, &bytes);
        rig.choose("export.text", &file);
        rig.release();
        if locked {
            rig.enter_password("tool-password");
            rig.release();
        }
        assert_eq!(rig.tabs(), 1, "{extension} locked={locked}");
        assert!(rig.export_open(), "{extension} locked={locked}");
    }
}

#[test]
fn startup_files_wait_for_absolute_link_consent_and_preparation() {
    let target = TempFile::new("link-target", "pdf", &fixture(2));
    let escaped_target = target.path().replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
    let source = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Annots [4 0 R] >> endobj\n\
         4 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 200 300] /Border [0 0 0] /A << /S /GoToR /F ({escaped_target}) /D [0 /Fit] >> >> endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
    );
    let mut rig = Rig::empty();
    let ctx_h = rig.h.ctx.clone();
    rig.h.state_mut().open_bytes(&ctx_h, "source.pdf", None, source.into_bytes()).expect("the source document");
    rig.h.run_steps(3);
    let page = rig.h.state().views[0].page_screen_rect(0).expect("the page is on screen");
    rig.h.event(egui::Event::PointerMoved(page.center()));
    for pressed in [true, false] {
        rig.h.event(egui::Event::PointerButton {
            pos: page.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    rig.h.run_steps(3);
    assert!(rig.h.query_by_label("Allow").is_some(), "the link asks before the file is touched");

    rig.intake.open_bytes("startup.pdf".into(), Arc::from(fixture(1)));
    rig.h.run_steps(2);
    assert_eq!(rig.tabs(), 1, "startup bytes cannot replace the consent prompt");
    // The prompt defaults to Cancel, so it is approved with its own button.
    rig.press("Allow");
    assert_eq!(rig.tabs(), 1, "the approved file is still being prepared");
    rig.release();
    rig.h.run_steps(3);
    assert_eq!(rig.tabs(), 3, "the linked file opens, then the startup file");
    assert!(rig.has_doc(&target.name()));
}

#[test]
fn launch_layout_waits_for_cli_opens_and_preserves_restored_tabs() {
    // The explicit `--layout` is set before the flagged opens, or after them.
    for layout_first in [true, false] {
        let mut rig = Rig::new();
        let restored = rig.layout_of("existing.pdf").expect("the restored tab");
        let (a, b) = (TempFile::new("layout-a", "pdf", &fixture(1)), TempFile::new("layout-b", "pdf", &fixture(1)));
        if layout_first {
            rig.h.state_mut().set_launch_layout("two-page").expect("a layout");
        }
        rig.h.state_mut().open_path_with_display(&a.path(), PageDisplay::Single, false);
        rig.h.state_mut().open_path_with_display(&b.path(), PageDisplay::Single, false);
        if !layout_first {
            rig.h.state_mut().set_launch_layout("two-page").expect("a layout");
        }
        rig.release();
        assert_eq!(rig.layout_of("existing.pdf"), Some(restored), "the restored tab is not targeted");
        assert_ne!(rig.layout_of(&b.name()), Some((PageDisplay::TwoUp, false)), "the layout waits for the last flagged open");
        rig.release();
        assert_eq!(rig.layout_of(&b.name()), Some((PageDisplay::TwoUp, false)), "the last settled open gets the layout");
        assert_eq!(rig.layout_of(&a.name()), Some((PageDisplay::Single, false)));
        assert_eq!(rig.layout_of("existing.pdf"), Some(restored));
        // A later open does not inherit it.
        let c = TempFile::new("layout-c", "pdf", &fixture(1));
        rig.h.state_mut().open_path(&c.path());
        rig.release();
        assert_ne!(rig.layout_of(&c.name()), Some((PageDisplay::TwoUp, false)));
    }
}

#[test]
fn launch_layout_failed_or_cancelled_opens_do_not_target_existing_documents() {
    let mut rig = Rig::new();
    let restored = rig.layout_of("existing.pdf").expect("the restored tab");
    rig.h.state_mut().set_launch_layout("two-up").expect("a layout");
    // A read that fails.
    let missing = std::env::temp_dir().join(format!("pdfcraft-missing-{}.pdf", std::process::id()));
    rig.h.state_mut().open_path_with_display(&missing.to_string_lossy(), PageDisplay::Single, false);
    rig.release();
    assert_eq!(rig.layout_of("existing.pdf"), Some(restored));
    // A password prompt that is cancelled.
    rig.h.state_mut().set_launch_layout("two-up").expect("a layout");
    let locked = TempFile::new("layout-locked", "pdf", &encrypted_fixture());
    rig.h.state_mut().open_path_with_display(&locked.path(), PageDisplay::Single, false);
    rig.release();
    assert!(rig.password_dialog());
    rig.press("Cancel");
    assert_eq!(rig.layout_of("existing.pdf"), Some(restored), "a cancelled open never targets an existing document");
    // A later open is not affected.
    let next = TempFile::new("layout-next", "pdf", &fixture(1));
    rig.h.state_mut().open_path(&next.path());
    rig.release();
    assert_ne!(rig.layout_of(&next.name()), Some((PageDisplay::TwoUp, false)));
}

#[test]
fn flagged_startup_opens_wait_for_password_ownership_before_the_first_frame() {
    let mut rig = Rig::empty();
    rig.h.state_mut().set_launch_layout("two-page").expect("a layout");
    let locked = TempFile::new("flag-locked", "pdf", &encrypted_fixture());
    let plain = TempFile::new("flag-plain", "pdf", &fixture(1));
    rig.h.state_mut().open_path_with_display(&locked.path(), PageDisplay::Single, false);
    // Naming the locked file again joins its open instead of queuing a second one.
    rig.h.state_mut().open_path_with_display(&locked.path(), PageDisplay::Single, false);
    rig.h.state_mut().open_path_with_display(&plain.path(), PageDisplay::Single, false);
    assert_eq!(rig.h.state().pending_opens.len(), 2);
    rig.release();
    assert!(rig.password_dialog());
    rig.release();
    assert_eq!(rig.tabs(), 0, "the flagged open waits for the password's owner");

    rig.enter_password("wrong-password");
    rig.release();
    assert!(rig.h.query_by_label_contains("Incorrect password").is_some());
    rig.enter_password("tool-password");
    rig.release();
    rig.h.run_steps(2);
    assert_eq!(rig.tabs(), 2);
    assert!(!rig.password_dialog(), "no second prompt appears");
    assert_eq!(rig.layout_of(&locked.name()), Some((PageDisplay::Single, false)));
    assert_eq!(rig.layout_of(&plain.name()), Some((PageDisplay::TwoUp, false)), "the layout lands on the last settled open");
}

#[test]
fn a_new_file_from_the_os_brings_the_window_forward_before_it_is_prepared() {
    let mut rig = Rig::new();
    let file = TempFile::new("os-open", "pdf", &fixture(2));
    rig.intake.open(vec![file.0.clone()]);
    rig.h.step();
    let commands: Vec<egui::ViewportCommand> = rig.h.output().viewport_output.values().flat_map(|output| output.commands.clone()).collect();
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)), "{commands:?}");
    assert!(commands.contains(&egui::ViewportCommand::Focus), "{commands:?}");
    assert_eq!(rig.tabs(), 1, "the file is still being prepared");
    rig.release();
    assert_eq!(rig.tabs(), 2);
    assert_eq!(rig.active_name(), Some(file.name()), "the new document is the selected tab");
}

#[test]
fn a_repeated_waiting_open_keeps_the_tool_it_brings() {
    let mut rig = Rig::empty();
    let file = TempFile::new("repeat", "pdf", &fixture(2));
    let ctx = rig.h.ctx.clone();
    rig.h.state_mut().open_path_then(&ctx, &file.path(), Some("export.text"));
    rig.h.state_mut().open_path_then(&ctx, &file.path(), None);
    assert_eq!(rig.jobs.waiting(), 1, "the file opens once");
    rig.release();
    assert_eq!(rig.tabs(), 1);
    assert!(rig.export_open(), "the second open keeps the tool the first one brought");
}

/// File ▸ Open of a Word document: LibreOffice runs as a queue job, never on the frame thread,
/// and the result opens as a new, unsaved PDF. Without LibreOffice the user is told it is needed,
/// rather than that the file is a broken PDF.
#[test]
fn file_open_converts_an_office_document_on_the_worker() {
    let mut rig = Rig::empty();
    let file = TempFile::new("office", "docx", &synthetic_docx());
    rig.choose("file.open", &file);
    assert_eq!(rig.tabs(), 0, "nothing opens before the conversion finishes");
    rig.release();
    let app = rig.h.state();
    if let Some(view) = app.views.first() {
        let doc = app.session.get(view.id).expect("the converted document");
        // Named after the Word file: `<stem>.docx` opens as `<stem>.pdf`.
        assert_eq!(doc.name, format!("pdfcraft-open-tool-{}-office.pdf", std::process::id()));
        assert!(doc.path.is_none(), "the converted PDF is new and unsaved, not the .docx on disk");
    } else {
        assert!(rig.notice_says("LibreOffice, which isn't installed"), "the failure names the missing dependency");
    }
}

fn synthetic_docx() -> Vec<u8> {
    let mut writer = craft::zip::Writer::new();
    {
        for (name, body) in [
            (
                "[Content_Types].xml",
                "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>",
            ),
            (
                "word/document.xml",
                "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>Synthetic</w:t></w:r></w:p></w:body></w:document>",
            ),
        ] {
            writer.add(name, body.as_bytes(), craft::zip::Compression::Deflated).expect("add zip entry");
        }
    }
    writer.finish().expect("finish zip")
}
