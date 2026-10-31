use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

use crate::{PdfCraftApp, control::ControlClient, open_worker};

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

#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn harness() -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::default();
    let saved = slot.clone();
    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").expect("language preference");
        *saved.lock().expect("control-client slot") = Some(app.attach_control(&creation.egui_ctx));
        app.open_bytes("existing.pdf", None, fixture(3)).expect("existing document");
        app
    });
    harness.run_steps(3);
    (harness, slot.lock().expect("control-client slot").take().expect("control client"))
}

#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn request(harness: &mut Harness<'static, PdfCraftApp>, client: &ControlClient, method: &str, params: Value) -> Value {
    let reply = client.send(method, params);
    for _ in 0..5 {
        harness.step();
        if let Ok(result) = reply.try_recv() {
            return result.expect("control request succeeded");
        }
    }
    panic!("{method} was not answered during the held prepare phase");
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn open_prepare_keeps_control_and_existing_tabs_live_until_atomic_commit() {
    let (mut harness, client) = harness();
    let (gate, started, release, finished) = open_worker::prepare_gate();
    harness.state_mut().open_prepare_gate = Some(gate);

    let path = std::env::temp_dir().join(format!("pdfcraft-open-worker-{}.pdf", std::process::id()));
    std::fs::write(&path, fixture(2)).expect("write test PDF");
    let open_reply = request(&mut harness, &client, "ui.open", json!({ "path": path.to_string_lossy() }));
    assert_eq!(open_reply["pending"], true);
    started.recv_timeout(Duration::from_secs(5)).expect("worker reached prepare barrier");

    let state = request(&mut harness, &client, "ui.state", json!({}));
    assert_eq!(state["documents"].as_array().map(Vec::len), Some(1));
    assert_eq!(state["documents"][0]["name"], "existing.pdf");
    assert_eq!(state["pending_opens"].as_array().map(Vec::len), Some(1));
    request(&mut harness, &client, "ui.command", json!({ "id": "page.insert_blank" }));
    assert_eq!(harness.state().session.get(harness.state().views[0].id).expect("existing tab").info.pages.len(), 4);
    assert_eq!(harness.state().views.len(), 1, "prepared state is not visible in the session");

    release.send(()).expect("release prepare barrier");
    finished.recv_timeout(Duration::from_secs(5)).expect("worker finished preparation");
    harness.step();
    let app = harness.state();
    assert_eq!(app.views.len(), 2, "one prepared document commits exactly once");
    assert_eq!(app.session.get(app.views[1].id).expect("new tab").info.pages.len(), 2);
    assert_eq!(app.active, Some(1));
    let _ = std::fs::remove_file(path);
}

struct ToolOpenFile(std::path::PathBuf);

impl Drop for ToolOpenFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[allow(clippy::expect_used)]
fn tool_open_file(case: &str, extension: &str, bytes: &[u8]) -> ToolOpenFile {
    let path = std::env::temp_dir().join(format!("pdfcraft-open-tool-{}-{case}.{extension}", std::process::id()));
    std::fs::write(&path, bytes).expect("write the synthetic tool input");
    ToolOpenFile(path)
}

fn tool_open_harness() -> Harness<'static, PdfCraftApp> {
    let (mut harness, _) = harness();
    harness.state_mut().close_tab(0);
    harness.run_steps(2);
    harness
}

struct HeldToolOpen {
    started: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
    finished: std::sync::mpsc::Receiver<()>,
}

impl HeldToolOpen {
    fn install(harness: &mut Harness<'static, PdfCraftApp>) -> Self {
        let (gate, started, release, finished) = open_worker::prepare_gate();
        harness.state_mut().open_prepare_gate = Some(gate);
        Self { started, release, finished }
    }

    #[allow(clippy::expect_used)]
    fn wait_started(&self) {
        self.started.recv_timeout(Duration::from_secs(5)).expect("the native open reached its preparation barrier");
    }

    #[allow(clippy::expect_used)]
    fn complete(&self, harness: &mut Harness<'static, PdfCraftApp>) {
        self.release.send(()).expect("release the native preparation");
        self.finished.recv_timeout(Duration::from_secs(5)).expect("the native open returned its result");
        harness.run_steps(2);
    }
}

fn choose_export_tool(harness: &mut Harness<'static, PdfCraftApp>, file: &ToolOpenFile) {
    harness.state_mut().pick_override = Some(vec![file.0.to_string_lossy().into_owned()]);
    harness.state_mut().run_command("export.text");
    harness.run_steps(2);
}

/// Picking a file for a tool must carry the tool through native preparation; another file
/// opened while it is pending must not become the tool's source.
#[allow(clippy::expect_used)]
fn chosen_tool_runs_after_preparation(extension: &str, bytes: Vec<u8>) {
    let mut harness = tool_open_harness();
    let held = HeldToolOpen::install(&mut harness);
    let file = tool_open_file("pending", extension, &bytes);
    choose_export_tool(&mut harness, &file);
    held.wait_started();
    assert!(harness.state().views.is_empty());
    assert!(harness.state().dialog.is_none(), "the tool cannot run before its document exists");
    harness.state_mut().open_bytes("other.pdf", None, fixture(3)).expect("another document opens while preparation is held");
    let other = harness.state().active_ids().expect("the other document is active").1;
    held.complete(&mut harness);
    let app = harness.state();
    let source = app.active_ids().expect("the chosen file becomes active").1;
    assert_ne!(source, other);
    assert_eq!(app.views.len(), 2, "the chosen file commits once and retains the other document");
    assert_eq!(app.session.get(other).expect("the other document is retained").info.pages.len(), 3);
    assert!(!app.session.get(other).expect("the other document is retained").dirty);
    let chosen = app.session.get(source).expect("the tool's source exists");
    if extension == "pdf" {
        assert_eq!(chosen.path.as_deref(), Some(file.0.to_string_lossy().as_ref()));
        assert_eq!(chosen.info.pages.len(), 2);
        assert_eq!(chosen.bytes.as_slice(), bytes.as_slice(), "the selected PDF's contents are retained");
    } else {
        assert!(chosen.dirty && chosen.path.is_none(), "conversion produces a new unsaved PDF");
        assert_eq!(chosen.info.pages.len(), 1);
        let mut renderer = pdfcraft_render::PageRenderer::new(chosen.bytes.clone(), Default::default());
        let output = renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, ..Default::default() });
        let text = output.text.expect("read the converted text").plain_text();
        assert_eq!(text.split_whitespace().collect::<Vec<_>>().join(" "), std::str::from_utf8(&bytes).expect("synthetic input is UTF-8"));
    }
    assert!(matches!(app.dialog, Some(crate::Dialog::Export(_))), "the chosen Export tool runs after its source commits");
    assert!(app.pending_opens.is_empty());
}

#[test]
fn a_chosen_tool_runs_on_its_file_only_after_native_preparation_commits() {
    chosen_tool_runs_after_preparation("pdf", fixture(2));
}

#[test]
fn a_chosen_tool_runs_on_converted_text_only_after_native_preparation_commits() {
    chosen_tool_runs_after_preparation("txt", b"Synthetic text input for the selected export tool".to_vec());
}

#[allow(clippy::expect_used)]
fn encrypted_tool_fixture() -> Vec<u8> {
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open("secret.pdf", None, Arc::new(fixture(2)), None).expect("open the synthetic PDF");
    session
        .apply(id, pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection { open_password: Some("tool-password".into()), ..Default::default() }))
        .expect("encrypt the synthetic PDF");
    session.save_bytes(id).expect("write the encrypted synthetic PDF").as_ref().clone()
}

#[test]
#[allow(clippy::expect_used)]
fn a_chosen_tool_survives_wrong_password_retry_and_runs_after_unlock() {
    let mut harness = tool_open_harness();
    let held = HeldToolOpen::install(&mut harness);
    let file = tool_open_file("encrypted-retry", "pdf", &encrypted_tool_fixture());
    choose_export_tool(&mut harness, &file);
    held.wait_started();
    held.complete(&mut harness);
    assert!(harness.state().password_prompt.is_some());
    assert!(harness.state().views.is_empty() && harness.state().dialog.is_none());

    harness.state_mut().submit_password(Some("wrong-password".into()));
    held.wait_started();
    assert!(harness.state().password_prompt.is_none(), "the submitted password is still being checked");
    held.complete(&mut harness);
    let prompt = harness.state().password_prompt.as_ref().expect("the wrong password is reported");
    assert!(prompt.error.is_some());
    assert_eq!(prompt.path.as_deref(), Some(file.0.to_string_lossy().as_ref()));
    assert!(harness.state().views.is_empty() && harness.state().dialog.is_none());

    harness.state_mut().submit_password(Some("tool-password".into()));
    held.wait_started();
    assert!(harness.state().dialog.is_none(), "the tool waits for authentication to succeed");
    held.complete(&mut harness);
    let app = harness.state();
    assert!(app.password_prompt.is_none() && app.pending_opens.is_empty());
    assert_eq!(app.views.len(), 1);
    let source = app.active_ids().expect("the unlocked document is active").1;
    assert_eq!(app.session.get(source).expect("the unlocked document exists").path.as_deref(), Some(file.0.to_string_lossy().as_ref()));
    assert!(matches!(app.dialog, Some(crate::Dialog::Export(_))), "the chosen tool survives the wrong-password retry");
}

#[test]
#[allow(clippy::expect_used)]
fn cancelling_a_chosen_tools_picker_or_password_never_runs_it_on_another_document() {
    let mut harness = tool_open_harness();
    harness.state_mut().pick_override = Some(Vec::new());
    harness.state_mut().run_command("export.text");
    harness.run_steps(2);
    assert!(harness.state().pending_opens.is_empty());
    assert!(harness.state().views.is_empty() && harness.state().dialog.is_none());

    let held = HeldToolOpen::install(&mut harness);
    let file = tool_open_file("encrypted-cancel", "pdf", &encrypted_tool_fixture());
    choose_export_tool(&mut harness, &file);
    held.wait_started();
    held.complete(&mut harness);
    assert!(harness.state().password_prompt.is_some());
    harness.state_mut().submit_password(None);
    harness.state_mut().open_bytes("other.pdf", None, fixture(3)).expect("a different document opens after cancellation");
    harness.run_steps(3);
    assert!(harness.state().password_prompt.is_none() && harness.state().pending_opens.is_empty());
    assert_eq!(harness.state().views.len(), 1);
    let id = harness.state().active_ids().expect("the other document is active").1;
    assert_eq!(harness.state().session.get(id).expect("the other document exists").name, "other.pdf");
    assert!(harness.state().dialog.is_none(), "cancelling discards the chosen tool's continuation");
}

#[test]
fn a_chosen_tool_does_not_run_when_native_preparation_rejects_the_file() {
    let mut harness = tool_open_harness();
    let held = HeldToolOpen::install(&mut harness);
    let file = tool_open_file("invalid", "pdf", b"%PDF-1.7\nnot a PDF object graph");
    choose_export_tool(&mut harness, &file);
    held.wait_started();
    held.complete(&mut harness);
    assert!(harness.state().views.is_empty() && harness.state().pending_opens.is_empty());
    assert!(harness.state().dialog.is_none() && harness.state().password_prompt.is_none());
    assert!(harness.state().toast.as_ref().is_some_and(|(message, _)| message.contains("Couldn't open")), "the open failure is reported");
}

#[test]
#[allow(clippy::expect_used)]
fn startup_intake_waits_for_local_authentication_and_keeps_its_own_background_password_prompt() {
    let (mut harness, _) = harness();
    let existing = harness.state().active_ids().expect("the existing document is active").1;
    let original = harness.state().session.get(existing).expect("the existing document is open").bytes.clone();
    let encrypted = encrypted_tool_fixture();
    harness.state_mut().open_bytes("local-secret.pdf", None, encrypted.clone()).expect("the local PDF asks for its password");
    let held = HeldToolOpen::install(&mut harness);
    harness.state_mut().submit_password(Some("tool-password".into()));
    held.wait_started();
    harness.state().startup_inbox.lock().expect("startup inbox").push(("startup-secret.pdf".into(), encrypted.clone()));
    harness.run_steps(2);
    assert_eq!(harness.state().startup_inbox.lock().expect("startup inbox").len(), 1, "startup bytes stay queued during local authentication");
    assert!(harness.state().password_prompt.is_none(), "no unrelated prompt is created during the held open");
    assert_eq!(harness.state().active_ids().map(|(_, id)| id), Some(existing));

    held.complete(&mut harness);
    let local = harness.state().active_ids().expect("the authenticated local document is active").1;
    assert_ne!(local, existing);
    assert_eq!(harness.state().session.get(local).expect("the local document is open").name, "local-secret.pdf");
    assert_eq!(harness.state().views.len(), 2);
    assert!(harness.state().startup_inbox.lock().expect("startup inbox").is_empty());
    let prompt = harness.state().password_prompt.as_ref().expect("the startup PDF retains its own password prompt after local completion");
    assert_eq!(prompt.name, "startup-secret.pdf");
    assert_eq!(prompt.bytes.as_slice(), encrypted.as_slice());
    assert!(!prompt.activate, "unlocking a late startup PDF must keep it in the background");

    harness.state_mut().submit_password(Some("tool-password".into()));
    held.wait_started();
    assert_eq!(harness.state().active_ids().map(|(_, id)| id), Some(local));
    held.complete(&mut harness);
    let app = harness.state();
    assert!(app.password_prompt.is_none() && app.pending_opens.is_empty());
    assert_eq!(app.views.len(), 3);
    assert_eq!(app.active_ids().map(|(_, id)| id), Some(local), "startup authentication does not steal the local document's focus");
    let startup = app.session.docs().iter().find(|doc| doc.name == "startup-secret.pdf").expect("the startup document is available");
    assert_ne!(startup.id, local);
    assert_eq!(startup.info.pages.len(), 2);
    assert_eq!(app.session.get(existing).expect("the existing document remains open").bytes, original);
}

#[test]
#[allow(clippy::expect_used)]
fn startup_intake_waits_through_wrong_password_retry_and_cancel_preserves_the_existing_document() {
    let (mut harness, _) = harness();
    let existing = harness.state().active_ids().expect("the existing document is active").1;
    let original = harness.state().session.get(existing).expect("the existing document is open").bytes.clone();
    harness.state_mut().open_bytes("local-secret.pdf", None, encrypted_tool_fixture()).expect("the local PDF asks for its password");
    let held = HeldToolOpen::install(&mut harness);
    harness.state_mut().submit_password(Some("wrong-password".into()));
    held.wait_started();
    harness.state().startup_inbox.lock().expect("startup inbox").push(("startup.pdf".into(), fixture(1)));
    harness.run_steps(2);
    assert_eq!(harness.state().startup_inbox.lock().expect("startup inbox").len(), 1, "startup bytes wait while the password is checked");
    assert_eq!(harness.state().active_ids().map(|(_, id)| id), Some(existing));

    held.complete(&mut harness);
    let prompt = harness.state().password_prompt.as_ref().expect("the local wrong-password prompt is retained");
    assert_eq!(prompt.name, "local-secret.pdf");
    assert!(prompt.error.is_some());
    assert_eq!(harness.state().startup_inbox.lock().expect("startup inbox").len(), 1, "startup bytes also wait for the user's retry or cancellation");
    assert_eq!(harness.state().views.len(), 1);
    harness.state_mut().submit_password(None);
    harness.run_steps(2);
    let app = harness.state();
    assert!(app.password_prompt.is_none() && app.pending_opens.is_empty());
    assert!(app.startup_inbox.lock().expect("startup inbox").is_empty());
    assert_eq!(app.views.len(), 2);
    assert_eq!(app.active_ids().map(|(_, id)| id), Some(existing), "cancelling the local open leaves the startup PDF in the background");
    assert!(!app.session.docs().iter().any(|doc| doc.name == "local-secret.pdf"));
    assert_eq!(app.session.docs().iter().find(|doc| doc.name == "startup.pdf").expect("the startup document is available").info.pages.len(), 1);
    assert_eq!(app.session.get(existing).expect("the existing document remains open").bytes, original);
}

#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn assert_tool_runs_after_background_open(extension: &str, bytes: Vec<u8>, pages: usize) {
    let (mut harness, _client) = harness();
    harness.state_mut().close_tab(0);
    let (gate, started, release, finished) = open_worker::prepare_gate();
    harness.state_mut().open_prepare_gate = Some(gate);
    let path = std::env::temp_dir().join(format!("pdfcraft-tool-worker-{}.{extension}", std::process::id()));
    std::fs::write(&path, &bytes).expect("write picked document");
    harness.state_mut().pick_override = Some(vec![path.to_string_lossy().into_owned()]);
    harness.state_mut().run_command("export.text");
    harness.step();
    started.recv_timeout(Duration::from_secs(5)).expect("the picked document reached preparation");
    assert!(harness.state().active.is_none());
    assert!(harness.state().dialog.is_none(), "the tool waits for its document");
    release.send(()).expect("release picked document preparation");
    finished.recv_timeout(Duration::from_secs(5)).expect("picked document prepared");
    harness.step();
    let app = harness.state();
    assert_eq!(app.views.len(), 1, "the chosen document opens once");
    assert_eq!(app.session.get(app.views[0].id).expect("chosen document").info.pages.len(), pages);
    assert!(matches!(app.dialog, Some(crate::Dialog::Export(_))), "the chosen tool runs after preparation: {:?}", app.dialog);
    harness.run_steps(2);
    assert_eq!(harness.state().views.len(), 1);
    assert_eq!(std::fs::read(&path).expect("read unchanged input"), bytes);
    let _ = std::fs::remove_file(path);
}

#[test]
fn tool_from_home_runs_after_pdf_preparation() {
    assert_tool_runs_after_background_open("pdf", fixture(2), 2);
}

#[test]
fn tool_from_home_runs_after_text_conversion() {
    assert_tool_runs_after_background_open("txt", b"Synthetic document for a chosen tool".to_vec(), 1);
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn a_startup_document_waits_for_an_active_document_to_prepare() {
    let (mut harness, _client) = harness();
    harness.state_mut().close_tab(0);
    let (gate, started, release, finished) = open_worker::prepare_gate();
    harness.state_mut().open_prepare_gate = Some(gate);
    let path = std::env::temp_dir().join(format!("pdfcraft-startup-worker-{}.pdf", std::process::id()));
    std::fs::write(&path, fixture(2)).expect("write chosen document");
    harness.state_mut().open_path(&path.to_string_lossy());
    started.recv_timeout(Duration::from_secs(5)).expect("chosen document reached preparation");
    harness.state().startup_inbox.lock().expect("startup inbox").push(("startup.pdf".into(), fixture(1)));
    harness.step();
    assert_eq!(harness.state().startup_inbox.lock().expect("startup inbox").len(), 1, "startup bytes wait for the pending active open");
    assert!(harness.state().views.is_empty());
    release.send(()).expect("release chosen document preparation");
    finished.recv_timeout(Duration::from_secs(5)).expect("chosen document prepared");
    harness.step();
    assert_eq!(harness.state().views.len(), 2);
    assert_eq!(harness.state().active, Some(0), "the chosen document keeps focus");
    let _ = std::fs::remove_file(path);
}

#[test]
#[allow(clippy::expect_used)]
fn tool_modal_keeps_picked_recipient_when_other_open_is_ready() {
    for same_batch in [true, false] {
        let timing = if same_batch { "batch" } else { "later" };
        let original_a = fixture(2);
        let original_b = fixture(1);
        let picked = tool_open_file(&format!("recipient-{timing}-a"), "pdf", &original_a);
        let other = tool_open_file(&format!("recipient-{timing}-b"), "pdf", &original_b);
        let mut harness = tool_open_harness();
        harness.state_mut().pick_override = Some(vec![picked.0.to_string_lossy().into_owned()]);
        let held = HeldToolOpen::install(&mut harness);
        harness.state_mut().run_command("export.text");
        harness.step();
        held.started.recv_timeout(Duration::from_secs(5)).expect("picked A reached prepare barrier");
        harness.state_mut().open_path(other.0.to_string_lossy().as_ref());
        held.release.send(()).expect("release picked A");
        held.finished.recv_timeout(Duration::from_secs(5)).expect("picked A is ready");
        held.started.recv_timeout(Duration::from_secs(5)).expect("ordinary B reached prepare barrier");
        if same_batch {
            held.release.send(()).expect("release ordinary B before A admission");
            held.finished.recv_timeout(Duration::from_secs(5)).expect("ordinary B is also ready");
        }
        harness.step();
        assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
        let active = harness.state().active.expect("active Export recipient");
        let doc = harness.state().session.get(harness.state().views[active].id).expect("Export recipient document");
        assert_eq!(doc.path.as_deref(), Some(picked.0.to_string_lossy().as_ref()), "Export must retain picked A in {timing} completion timing");
        assert_eq!(doc.info.pages.len(), 2);
        assert!(!doc.dirty && doc.can_undo().is_none() && doc.can_redo().is_none());
        assert_eq!(doc.saved_bytes().as_slice(), original_a.as_slice());
        assert_eq!(harness.state().views.len(), 1);
        assert_eq!(harness.state().pending_opens.len(), 1);
        if !same_batch {
            held.release.send(()).expect("release ordinary B after A Export opens");
            held.finished.recv_timeout(Duration::from_secs(5)).expect("ordinary B is now ready");
        }
        harness.run_steps(2);
        assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
        assert_eq!(harness.state().active, Some(active));
        assert_eq!(harness.state().views.len(), 1, "ready B stays unadmitted while A's modal is open");
        assert_eq!(harness.state().pending_opens.len(), 1);
        assert_eq!(harness.state().pending_opens[0].path.as_deref(), Some(other.0.to_string_lossy().as_ref()));
        // Use the actual modal's Cancel button, rather than clearing its state directly.
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        assert!(harness.state().dialog.is_none() && harness.state().pending_opens.is_empty());
        assert_eq!(harness.state().views.len(), 2, "cancel releases B's retained result exactly once");
        let active = harness.state().active.expect("ordinary B activates after cancel");
        let doc = harness.state().session.get(harness.state().views[active].id).expect("ordinary B document");
        assert_eq!(doc.path.as_deref(), Some(other.0.to_string_lossy().as_ref()));
        assert_eq!(doc.info.pages.len(), 1);
        assert!(!doc.dirty && doc.can_undo().is_none() && doc.can_redo().is_none());
        assert_eq!(doc.saved_bytes().as_slice(), original_b.as_slice());
        harness.run_steps(2);
        assert_eq!(harness.state().views.len(), 2);
        assert!(harness.state().dialog.is_none(), "A's tool does not run again after B admission");
        for (file, original) in [(&picked, &original_a), (&other, &original_b)] {
            assert_eq!(std::fs::read(&file.0).expect("read unchanged modal fixture"), *original);
        }
    }
}

#[test]
#[allow(clippy::expect_used)]
fn tool_password_prompt_retains_other_ready_open_through_retry() {
    let protected = encrypted_tool_fixture();
    let picked = tool_open_file("password-recipient-a", "pdf", &protected);
    let other = tool_open_file("password-recipient-b", "pdf", &fixture(2));
    let mut harness = tool_open_harness();
    harness.state_mut().pick_override = Some(vec![picked.0.to_string_lossy().into_owned()]);
    let held = HeldToolOpen::install(&mut harness);
    harness.state_mut().run_command("export.text");
    harness.step();
    held.started.recv_timeout(Duration::from_secs(5)).expect("encrypted A reached prepare barrier");
    harness.state_mut().open_path(other.0.to_string_lossy().as_ref());
    held.release.send(()).expect("release encrypted A");
    held.finished.recv_timeout(Duration::from_secs(5)).expect("encrypted A is ready");
    held.started.recv_timeout(Duration::from_secs(5)).expect("ordinary B reached prepare barrier");
    held.release.send(()).expect("release ordinary B");
    held.finished.recv_timeout(Duration::from_secs(5)).expect("ordinary B is ready before prompt admission");
    harness.step();
    let prompt = harness.state().password_prompt.as_ref().expect("picked A password prompt");
    assert_eq!(prompt.path.as_deref(), Some(picked.0.to_string_lossy().as_ref()));
    assert_eq!(prompt.then, Some("export.text"));
    harness.run_steps(2);
    assert!(harness.state().views.is_empty());
    assert_eq!(harness.state().pending_opens.len(), 1, "B cannot replace A's visible password prompt");
    harness.state_mut().submit_password(Some("wrong-password".into()));
    held.wait_started();
    held.complete(&mut harness);
    // During the retry gap no modal exists, so ordinary B may admit. A still owns its
    // continuation and its wrong-password prompt, and success must reactivate A.
    assert_eq!(harness.state().views.len(), 1);
    let prompt = harness.state().password_prompt.as_ref().expect("picked A wrong-password prompt");
    assert_eq!(prompt.path.as_deref(), Some(picked.0.to_string_lossy().as_ref()));
    assert!(prompt.error.is_some());
    assert_eq!(prompt.then, Some("export.text"));
    assert!(harness.state().dialog.is_none());
    harness.state_mut().submit_password(Some("tool-password".into()));
    held.wait_started();
    held.complete(&mut harness);
    assert!(harness.state().password_prompt.is_none() && harness.state().pending_opens.is_empty());
    assert_eq!(harness.state().views.len(), 2);
    assert!(matches!(harness.state().dialog, Some(crate::Dialog::Export(_))));
    let active = harness.state().active.expect("A reactivates for Export");
    let doc = harness.state().session.get(harness.state().views[active].id).expect("A Export recipient");
    assert_eq!(doc.path.as_deref(), Some(picked.0.to_string_lossy().as_ref()));
    assert_eq!(doc.info.pages.len(), 2);
    assert!(!doc.dirty && doc.can_undo().is_none());
    harness.run_steps(2);
    assert_eq!(harness.state().active, Some(active));
    harness.get_by_label("Cancel").click();
    harness.run_steps(2);
    assert!(harness.state().dialog.is_none() && harness.state().pending_opens.is_empty());
    assert_eq!(harness.state().views.len(), 2);
    assert_eq!(std::fs::read(&picked.0).expect("read original encrypted A"), protected.as_slice());
}

#[test]
#[allow(clippy::expect_used)]
fn tool_picker_keeps_picked_recipient_when_other_open_is_ready() {
    for cancel in [false, true] {
        let case = if cancel { "picker-cancel" } else { "picker-insert" };
        let original_a = fixture(2);
        let original_b = fixture(1);
        let original_c = fixture(1);
        let picked = tool_open_file(&format!("{case}-a"), "pdf", &original_a);
        let other = tool_open_file(&format!("{case}-b"), "pdf", &original_b);
        let insert = tool_open_file(&format!("{case}-c"), "pdf", &original_c);
        let mut harness = tool_open_harness();
        harness.state_mut().pick_override = Some(vec![picked.0.to_string_lossy().into_owned()]);
        let held = HeldToolOpen::install(&mut harness);
        harness.state_mut().run_command("page.insert");
        harness.step();
        held.started.recv_timeout(Duration::from_secs(5)).expect("picked A reached prepare barrier");
        assert!(harness.state().views.is_empty());
        assert_eq!(harness.state().pending_opens.len(), 1);
        harness.state_mut().open_path(other.0.to_string_lossy().as_ref());
        // A's first picker was consumed above. Its real Insert pages continuation must
        // now queue this second picker, bound to A, when the prepared document commits.
        harness.state_mut().pick_override = Some(if cancel { Vec::new() } else { vec![insert.0.to_string_lossy().into_owned()] });
        held.release.send(()).expect("release picked A");
        held.finished.recv_timeout(Duration::from_secs(5)).expect("picked A reply is ready");
        held.started.recv_timeout(Duration::from_secs(5)).expect("ordinary B reached prepare barrier");
        held.release.send(()).expect("release ordinary B");
        held.finished.recv_timeout(Duration::from_secs(5)).expect("ordinary B reply is also ready");
        assert_eq!(harness.state().pending_opens.len(), 2);
        let files = PickerFiles {
            picked: &picked,
            other: &other,
            insert: &insert,
            original_a: &original_a,
            original_b: &original_b,
            original_c: &original_c,
        };
        let a_id = assert_picker_keeps_a_visible(&mut harness, &files);
        assert_picker_completion_releases_b(&mut harness, a_id, cancel, &files);
    }
}

struct PickerFiles<'a> {
    picked: &'a ToolOpenFile,
    other: &'a ToolOpenFile,
    insert: &'a ToolOpenFile,
    original_a: &'a [u8],
    original_b: &'a [u8],
    original_c: &'a [u8],
}

fn assert_picker_keeps_a_visible(harness: &mut Harness<'static, PdfCraftApp>, files: &PickerFiles<'_>) -> pdfcraft_engine::DocId {
    // Poll admission separately from consuming the picker callback: this reproduces
    // the same ordering as a UI frame, with both worker replies already available.
    harness.state_mut().poll_pending_opens();
    assert_eq!(harness.state().views.len(), 1, "ready B must wait for A's Insert pages picker");
    assert_eq!(harness.state().active, Some(0));
    assert!(harness.state().dialog.is_none() && harness.state().password_prompt.is_none());
    let a_id = harness.state().views[0].id;
    let doc = harness.state().session.get(a_id);
    assert!(doc.is_some(), "picked A before picker completion");
    if let Some(doc) = doc {
        assert_eq!(doc.path.as_deref(), Some(files.picked.0.to_string_lossy().as_ref()));
        assert_eq!(doc.info.pages.len(), 2);
        assert!(!doc.dirty && doc.can_undo().is_none() && doc.can_redo().is_none());
    }
    assert_eq!(harness.state().pending_opens.len(), 1);
    assert_eq!(harness.state().pending_opens[0].path.as_deref(), Some(files.other.0.to_string_lossy().as_ref()));
    harness.state_mut().poll_pending_opens();
    assert_eq!(harness.state().views.len(), 1, "B remains held until the queued picker result is consumed");
    assert_eq!(harness.state().active, Some(0));
    a_id
}

fn assert_picker_completion_releases_b(
    harness: &mut Harness<'static, PdfCraftApp>,
    a_id: pdfcraft_engine::DocId,
    cancel: bool,
    files: &PickerFiles<'_>,
) {
    // Consume the actual native-picker callback (or its empty-path cancellation).
    harness.state_mut().process_picked();
    assert_document_a_after_picker(harness, a_id, cancel);
    assert_eq!(harness.state().views.len(), 1);
    assert_eq!(harness.state().pending_opens.len(), 1);
    assert_eq!(harness.state().active, Some(0));

    harness.step();
    assert!(harness.state().pending_opens.is_empty());
    assert_eq!(harness.state().views.len(), 2, "picker completion releases B exactly once");
    let active = harness.state().active;
    assert!(active.is_some(), "B activates after picker completion");
    if let Some(active) = active {
        assert_document_b_after_picker(harness, active, files);
    }
    harness.run_steps(2);
    assert!(harness.state().pending_opens.is_empty() && harness.state().dialog.is_none());
    assert_eq!(harness.state().views.len(), 2);
    assert_eq!(harness.state().active, active);
    assert_document_a_after_picker(harness, a_id, cancel);
    assert_a_saved_source(harness, a_id, files.original_a);
    assert_picker_files_unchanged(files);
}

fn assert_document_a_after_picker(harness: &Harness<'static, PdfCraftApp>, a_id: pdfcraft_engine::DocId, cancel: bool) {
    let expected_pages = if cancel { 2 } else { 3 };
    let doc = harness.state().session.get(a_id);
    assert!(doc.is_some(), "A after picker completion");
    if let Some(doc) = doc {
        assert_eq!(doc.info.pages.len(), expected_pages);
        assert_eq!(doc.dirty, !cancel);
        assert_eq!(doc.can_undo().is_some(), !cancel);
        assert!(doc.can_redo().is_none());
    }
}

fn assert_document_b_after_picker(harness: &Harness<'static, PdfCraftApp>, active: usize, files: &PickerFiles<'_>) {
    let doc = harness.state().session.get(harness.state().views[active].id);
    assert!(doc.is_some(), "ordinary B document");
    if let Some(doc) = doc {
        assert_eq!(doc.path.as_deref(), Some(files.other.0.to_string_lossy().as_ref()));
        assert_eq!(doc.info.pages.len(), 1);
        assert!(!doc.dirty && doc.can_undo().is_none() && doc.can_redo().is_none());
    }
}

fn assert_a_saved_source(harness: &Harness<'static, PdfCraftApp>, a_id: pdfcraft_engine::DocId, original: &[u8]) {
    let doc = harness.state().session.get(a_id);
    assert!(doc.is_some(), "A retains exactly one insertion or cancellation");
    if let Some(doc) = doc {
        assert_eq!(doc.saved_bytes().as_slice(), original, "insertion preserves A's saved source");
    }
}

fn assert_picker_files_unchanged(files: &PickerFiles<'_>) {
    for (file, original) in [(files.picked, files.original_a), (files.other, files.original_b), (files.insert, files.original_c)] {
        let bytes = std::fs::read(&file.0);
        assert!(bytes.is_ok(), "read unchanged original picker fixture: {bytes:?}");
        if let Ok(bytes) = bytes {
            assert_eq!(bytes, original);
        }
    }
}

#[test]
#[allow(clippy::expect_used, clippy::unwrap_used)]
fn inline_tool_open_keeps_its_command_for_pdf_conversion_and_password_retry() {
    for (name, bytes, encrypted) in
        [("plain.pdf", fixture(1), false), ("converted.txt", b"Text to export".to_vec(), false), ("locked.pdf", encrypted_tool_fixture(), true)]
    {
        let path = std::env::temp_dir().join(format!("pdfcraft-inline-tool-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write selected file");
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").expect("language preference");
        assert!(app.ctx.is_none(), "this exercises the no-context inline path");
        app.open_path_then(&path.to_string_lossy(), Some("export.text"));
        if encrypted {
            assert!(app.dialog.is_none());
            assert_eq!(app.password_prompt.as_ref().expect("password prompt").then, Some("export.text"));
            app.submit_password(Some("wrong".into()));
            assert!(app.password_prompt.as_ref().expect("wrong-password prompt").error.is_some());
            assert!(app.dialog.is_none());
            app.submit_password(Some("tool-password".into()));
        }
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.active, Some(0));
        assert!(matches!(app.dialog, Some(crate::Dialog::Export(_))), "the inline {name} retains its export command");
        let _ = std::fs::remove_file(path);
    }
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn startup_files_wait_for_absolute_link_consent_and_preparation() {
    let (mut h, _) = harness();
    let source = h.state().views[0].id;
    let path = std::env::temp_dir().join(format!("pdfcraft-open-consented-startup-{}.pdf", std::process::id()));
    std::fs::write(&path, fixture(2)).expect("write linked PDF");
    h.state_mut().open_remote_link(source, path.to_string_lossy().into_owned(), pdfcraft_render::RemoteDestination::Default);
    h.state().startup_inbox.lock().expect("startup queue").push(("startup.pdf".into(), fixture(1)));
    h.step();
    assert!(h.state().pending_remote_link.is_some());
    assert_eq!(h.state().startup_inbox.lock().expect("startup queue").len(), 1);
    assert_eq!(h.state().views.len(), 1, "startup cannot replace the consent interaction");

    let (gate, started, release, finished) = open_worker::prepare_gate();
    h.state_mut().open_prepare_gate = Some(gate);
    h.state_mut().resolve_pending_remote_link(true);
    started.recv_timeout(Duration::from_secs(5)).expect("worker reached prepare barrier");
    h.step();
    let queued = h.state().startup_inbox.lock().expect("startup queue").len();
    let visible = h.state().views.len();
    // Always release before asserting: a failure must not strand the worker at the barrier.
    release.send(()).expect("release prepare barrier");
    finished.recv_timeout(Duration::from_secs(5)).expect("worker finished preparation");
    assert_eq!(queued, 1, "startup bytes wait while the approved linked PDF prepares");
    assert_eq!(visible, 1);
    h.step();
    let app = h.state();
    assert_eq!(app.views.len(), 3);
    assert_eq!(app.session.get(app.views[1].id).expect("linked document").path.as_deref(), path.to_str());
    assert_eq!(app.session.get(app.views[2].id).expect("startup document").name, "startup.pdf");
    assert_eq!(app.active, Some(1), "the user's approved navigation keeps focus");
    let _ = std::fs::remove_file(path);
}

#[test]
fn launch_layout_waits_for_cli_opens_and_preserves_restored_tabs() {
    use crate::canvas::PageLayout;
    // Controlled worker replies exercise both completion orders without scheduling races.
    for reverse in [false, true] {
        for count in [1, 2] {
            for restored in [false, true] {
                let mut app = PdfCraftApp::new();
                app.set_option("default-layout", "continuous").unwrap();
                if restored {
                    app.open_bytes("restored.pdf", None, fixture(1)).unwrap();
                }
                let offset = usize::from(restored);
                let mut replies = Vec::new();
                for token in 0..count {
                    let name = format!("cli-{token}.pdf");
                    let bytes = Arc::new(fixture(2));
                    let prepared = app.session.prepare_open(&name, None, bytes.clone(), None);
                    let (sender, receiver) = std::sync::mpsc::channel();
                    app.pending_opens.push(crate::PendingOpen {
                        token,
                        receiver,
                        name,
                        path: None,
                        stamp: None,
                        activate: true,
                        after_open: None,
                        recovered: None,
                        then: None,
                        display_override: Some((PageLayout::TwoUp, false)),
                    });
                    replies.push((
                        sender,
                        open_worker::ResultMessage { token, kind: open_worker::ResultKind::Prepared { bytes, result: Box::new(prepared) } },
                    ));
                }
                app.set_launch_layout("single").unwrap();
                if restored {
                    assert!(app.views[0].continuous(), "pending --layout must not target the restored tab");
                }
                if reverse {
                    replies.reverse();
                }
                for (index, (sender, reply)) in replies.into_iter().enumerate() {
                    let name = format!("cli-{}.pdf", reply.token);
                    sender.send(reply).unwrap();
                    app.poll_pending_opens();
                    let view = &app.views[index + offset];
                    assert_eq!(view.layout, if index + 1 == count as usize { PageLayout::Single } else { PageLayout::TwoUp });
                    assert!(!view.continuous());
                    assert_eq!(app.session.get(view.id).unwrap().name, name);
                }
                assert_eq!(app.active, Some(count as usize + offset - 1));
                if restored {
                    assert_eq!(app.views[0].layout, PageLayout::Single);
                    assert!(app.views[0].continuous());
                }
                assert_eq!(app.view_defaults.layout, PageLayout::Single);
                assert!(app.view_defaults.continuous);
                assert_eq!(serde_json::from_str::<Value>(&app.persist()).unwrap()["default_layout"], "continuous");
                // Ordinary later opens cannot inherit either launch override.
                app.open_bytes("later.pdf", None, fixture(1)).unwrap();
                assert!(app.views.last().unwrap().continuous());
            }
        }
    }
}

#[test]
fn launch_layout_failed_or_cancelled_opens_do_not_target_existing_documents() {
    use crate::canvas::PageLayout;
    for cancel in [false, true] {
        let mut app = PdfCraftApp::new();
        app.set_option("default-layout", "continuous").unwrap();
        app.open_bytes("restored.pdf", None, fixture(1)).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        app.pending_opens.push(crate::PendingOpen {
            token: 0,
            receiver,
            name: "cli.pdf".into(),
            path: None,
            stamp: None,
            activate: true,
            after_open: None,
            recovered: None,
            then: None,
            display_override: Some((PageLayout::TwoUp, false)),
        });
        app.set_launch_layout("single").unwrap();
        assert!(app.set_launch_layout("invalid").is_err());
        let kind = if cancel {
            open_worker::ResultKind::Prepared { bytes: Arc::new(fixture(1)), result: Box::new(Err(pdfcraft_render::OpenError::NeedsPassword)) }
        } else {
            open_worker::ResultKind::ReadFailed("synthetic read failure".into())
        };
        sender.send(open_worker::ResultMessage { token: 0, kind }).unwrap();
        app.poll_pending_opens();
        if cancel {
            app.submit_password(None);
        }
        assert!(app.launch_layout.is_none());
        assert_eq!(app.views.len(), 1);
        assert!(app.views[0].continuous());
        app.open_bytes("later.pdf", None, fixture(1)).unwrap();
        assert!(app.views[1].continuous());
    }
}

#[test]
fn flagged_startup_opens_wait_for_password_ownership_before_the_first_frame() {
    use crate::canvas::PageLayout;
    let locked = tool_open_file("launch-locked", "pdf", &encrypted_tool_fixture());
    let second = tool_open_file("launch-second", "pdf", &fixture(2));
    let mut app = PdfCraftApp::new();
    app.set_option("default-layout", "continuous").unwrap();
    app.open_path_with_display(locked.0.to_str().unwrap(), PageLayout::TwoUp, false);
    let bytes = app.password_prompt.as_ref().unwrap().bytes.clone();
    app.open_path_with_display(locked.0.to_str().unwrap(), PageLayout::TwoUp, false);
    assert!(app.pending_opens.is_empty(), "repeating the locked file keeps its original prompt");
    app.open_path_with_display(second.0.to_str().unwrap(), PageLayout::TwoUp, false);
    assert_eq!(app.pending_opens.len(), 1);
    assert!(Arc::ptr_eq(&app.password_prompt.as_ref().unwrap().bytes, &bytes));
    app.set_launch_layout("single").unwrap();
    app.submit_password(Some("wrong".into()));
    assert!(app.password_prompt.as_ref().unwrap().error.is_some());
    app.submit_password(Some("tool-password".into()));
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.views[0].layout, PageLayout::TwoUp);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app.pending_opens.is_empty() {
        assert!(std::time::Instant::now() < deadline);
        app.poll_pending_opens();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.views.len(), 2);
    assert_eq!(app.views[1].layout, PageLayout::Single);
    assert!(!app.views[1].continuous());
    assert!(app.view_defaults.continuous);
}

/// #1928: a new file handed over by the OS (Finder, or another launch) raises the window at once,
/// before it is prepared, and is the selected tab once it opens.
#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn a_new_file_from_the_os_brings_the_window_forward_before_it_is_prepared() {
    let (mut harness, _client) = harness();
    let (gate, started, release, finished) = open_worker::prepare_gate();
    harness.state_mut().open_prepare_gate = Some(gate);
    let path = std::env::temp_dir().join(format!("pdfcraft-os-open-worker-{}.pdf", std::process::id()));
    std::fs::write(&path, fixture(2)).expect("write the handed-over PDF");
    let mut queue = vec![crate::OsEvent::Open(vec![path.to_string_lossy().into_owned()])];
    harness.state_mut().os_events = Some(Box::new(move || std::mem::take(&mut queue)));
    harness.step();
    started.recv_timeout(Duration::from_secs(5)).expect("the handed-over PDF reached preparation");
    let commands: Vec<_> = harness.output().viewport_output.values().flat_map(|v| v.commands.clone()).collect();
    let visible = harness.state().views.len();
    // Always release before asserting: a failure must not strand the worker at the barrier.
    release.send(()).expect("release the handed-over PDF");
    finished.recv_timeout(Duration::from_secs(5)).expect("the handed-over PDF prepared");
    assert!(commands.contains(&egui::ViewportCommand::Minimized(false)), "{commands:?}");
    assert!(commands.contains(&egui::ViewportCommand::Focus), "{commands:?}");
    assert_eq!(visible, 1, "the window comes forward while the file is still preparing");
    harness.step();
    let app = harness.state();
    assert_eq!(app.views.len(), 2);
    let active = app.active.expect("a selected document");
    let doc = app.session.get(app.views[active].id).expect("the handed-over document");
    assert_eq!(doc.path.as_deref(), Some(path.to_string_lossy().as_ref()), "the new file is the selected tab");
    assert_eq!(doc.info.pages.len(), 2);
    let _ = std::fs::remove_file(path);
}
