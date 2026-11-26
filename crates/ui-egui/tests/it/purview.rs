//! Microsoft account presentation in the real Purview dialog (egui_kittest).

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::Edit;
use pdfcraft_purview::{PurviewAccount, PurviewDocument, PurviewError, PurviewLabel, PurviewOpen, PurviewProvider, PurviewRights, PurviewStatus};
use pdfcraft_ui_egui::{PdfCraftApp, RecoveryStore};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

struct SignInGate {
    started: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

struct TestPurviewProvider {
    status: Mutex<PurviewStatus>,
    active: Mutex<Option<PurviewAccount>>,
    accounts: Mutex<Vec<PurviewAccount>>,
    inspect_document: Option<PurviewDocument>,
    inspect_sign_in_required: bool,
    sign_in_gate: Option<Arc<SignInGate>>,
    labels_gate: Option<Arc<SignInGate>>,
    labels_fail: bool,
    labels: Vec<PurviewLabel>,
    inspect_calls: Option<Arc<std::sync::atomic::AtomicUsize>>,
    deny_open: bool,
}

impl TestPurviewProvider {
    fn signed_out() -> Self {
        Self {
            status: Mutex::new(PurviewStatus::SignedOut),
            active: Mutex::new(None),
            accounts: Mutex::new(Vec::new()),
            inspect_document: None,
            inspect_sign_in_required: false,
            sign_in_gate: None,
            labels_gate: None,
            labels_fail: false,
            labels: Vec::new(),
            inspect_calls: None,
            deny_open: false,
        }
    }

    fn signed_out_for_pfile() -> Self {
        Self {
            status: Mutex::new(PurviewStatus::SignedOut),
            active: Mutex::new(None),
            accounts: Mutex::new(Vec::new()),
            inspect_document: None,
            inspect_sign_in_required: true,
            sign_in_gate: None,
            labels_gate: None,
            labels_fail: false,
            labels: Vec::new(),
            inspect_calls: None,
            deny_open: false,
        }
    }

    fn signed_in(account: PurviewAccount, accounts: Vec<PurviewAccount>) -> Self {
        Self {
            status: Mutex::new(PurviewStatus::Ready),
            active: Mutex::new(Some(account)),
            accounts: Mutex::new(accounts),
            inspect_document: None,
            inspect_sign_in_required: false,
            sign_in_gate: None,
            labels_gate: None,
            labels_fail: false,
            labels: Vec::new(),
            inspect_calls: None,
            deny_open: false,
        }
    }

    fn signed_in_with_document(account: PurviewAccount, accounts: Vec<PurviewAccount>, document: PurviewDocument) -> Self {
        Self {
            status: Mutex::new(PurviewStatus::Ready),
            active: Mutex::new(Some(account)),
            accounts: Mutex::new(accounts),
            inspect_document: Some(document),
            inspect_sign_in_required: false,
            sign_in_gate: None,
            labels_gate: None,
            labels_fail: false,
            labels: Vec::new(),
            inspect_calls: None,
            deny_open: false,
        }
    }
}

impl PurviewProvider for TestPurviewProvider {
    fn status(&self) -> PurviewStatus {
        *self.status.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn account(&self) -> Option<PurviewAccount> {
        self.active.lock().unwrap_or_else(|poison| poison.into_inner()).clone()
    }

    fn accounts(&self) -> Vec<PurviewAccount> {
        self.accounts.lock().unwrap_or_else(|poison| poison.into_inner()).clone()
    }

    fn switch_account(&self, account_id: &str) -> Result<(), PurviewError> {
        let account = self
            .accounts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
            .ok_or_else(|| PurviewError::Native("unknown test account".into()))?;
        *self.active.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(account);
        *self.status.lock().unwrap_or_else(|poison| poison.into_inner()) = PurviewStatus::Ready;
        Ok(())
    }

    fn sign_in(&self, _username_hint: Option<&str>) -> Result<(), PurviewError> {
        if let Some(gate) = &self.sign_in_gate {
            gate.started.send(()).unwrap();
            gate.release.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        }
        *self.status.lock().unwrap_or_else(|poison| poison.into_inner()) = PurviewStatus::Ready;
        Ok(())
    }

    fn sign_out(&self) -> Result<(), PurviewError> {
        let mut active = self.active.lock().unwrap_or_else(|poison| poison.into_inner());
        let mut accounts = self.accounts.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(account) = active.as_ref() {
            accounts.retain(|candidate| candidate.id != account.id);
        }
        *active = accounts.first().cloned();
        *self.status.lock().unwrap_or_else(|poison| poison.into_inner()) =
            if active.is_some() { PurviewStatus::Ready } else { PurviewStatus::SignedOut };
        Ok(())
    }

    fn inspect(&self, _name: &str, bytes: &Arc<Vec<u8>>) -> Result<Option<PurviewOpen>, PurviewError> {
        if let Some(calls) = &self.inspect_calls {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        if self.deny_open {
            return Err(PurviewError::NoAccess);
        }
        if self.inspect_sign_in_required {
            if self.status() != PurviewStatus::Ready {
                return Err(PurviewError::SignInRequired);
            }
            return Ok(Some(PurviewOpen {
                handle: 1,
                original: bytes.clone(),
                bytes: Arc::new(FIXTURE.to_vec()),
                document: PurviewDocument { protected: true, rights: PurviewRights { view: true, ..Default::default() }, ..Default::default() },
            }));
        }
        Ok(self.inspect_document.clone().map(|document| PurviewOpen { handle: 1, original: bytes.clone(), bytes: bytes.clone(), document }))
    }

    fn labels(&self) -> Result<Vec<PurviewLabel>, PurviewError> {
        if let Some(gate) = &self.labels_gate {
            gate.started.send(()).unwrap();
            gate.release.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        }
        if self.labels_fail { Err(PurviewError::Native("test label service unavailable".into())) } else { Ok(self.labels.clone()) }
    }

    fn apply_label(
        &self,
        _handle: u64,
        _original: &Arc<Vec<u8>>,
        _working_bytes: &Arc<Vec<u8>>,
        _label_id: &str,
        _justification: Option<&str>,
    ) -> Result<PurviewOpen, PurviewError> {
        Err(PurviewError::SdkUnavailable)
    }

    fn remove_label(
        &self,
        _handle: u64,
        _original: &Arc<Vec<u8>>,
        _working_bytes: &Arc<Vec<u8>>,
        _justification: Option<&str>,
    ) -> Result<PurviewOpen, PurviewError> {
        Err(PurviewError::SdkUnavailable)
    }

    fn remove_protection(&self, _handle: u64, _original: &Arc<Vec<u8>>, _working_bytes: &Arc<Vec<u8>>) -> Result<PurviewOpen, PurviewError> {
        Err(PurviewError::SdkUnavailable)
    }

    fn republish(&self, _handle: u64, _original: &Arc<Vec<u8>>, working_bytes: &Arc<Vec<u8>>) -> Result<Arc<Vec<u8>>, PurviewError> {
        Ok(working_bytes.clone())
    }

    fn close(&self, _handle: u64) {}
}

fn harness(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::inline_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.open_bytes(&cc.egui_ctx, "doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.set_option(&cc.egui_ctx, "dialog", "purview").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn harness_closed(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::inline_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.open_bytes(&cc.egui_ctx, "doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app
    });
    h.run_steps(4);
    h
}

fn harness_security(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::inline_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.open_bytes(&cc.egui_ctx, "doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.set_option(&cc.egui_ctx, "dialog", "security").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn harness_security_with_document(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::inline_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.open_bytes(&cc.egui_ctx, "protected.pdf", None, FIXTURE.to_vec()).unwrap();
        app.set_option(&cc.egui_ctx, "dialog", "security").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn harness_with_document(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::inline_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.open_bytes(&cc.egui_ctx, "protected.pdf", None, FIXTURE.to_vec()).unwrap();
        app.set_option(&cc.egui_ctx, "dialog", "purview").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn harness_pfile(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::inline_app();
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.open_bytes(
            &cc.egui_ctx,
            "protected.pfile",
            None,
            b"%PDF-1.7\n/EncryptedPayload\n/Subtype /MicrosoftIRMServices\nMicrosoftIRMServices Protected PDF".to_vec(),
        )
        .unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn signed_out_dialog_has_one_sign_in_action_and_no_email_field() {
    let h = harness(TestPurviewProvider::signed_out());
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Microsoft account");
    h.get_by_label("Sign in");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Sign out")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Microsoft 365 account")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Sensitivity labels")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Document capabilities")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Apply label")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove label")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove protection")).is_none());
}

#[test]
fn missing_storytold_app_id_names_the_actual_sign_in_prerequisite() {
    let provider = TestPurviewProvider::signed_out();
    *provider.status.lock().unwrap() = PurviewStatus::NotConfigured;
    let mut h = harness_closed(provider);
    assert!(h.state_mut().execute("purview.sign_in"));
    h.run_steps(1);
    assert!(
        h.state()
            .toast
            .as_ref()
            .is_some_and(|(message, _)| message.contains("approved Microsoft Entra application client ID") && message.contains("Azure CLI"))
    );
}

#[test]
fn opening_a_pfile_automatically_opens_the_signed_out_sign_in_gate() {
    let h = harness_pfile(TestPurviewProvider::signed_out_for_pfile());
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Microsoft account");
    h.get_by_label("Sign in");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Sensitivity labels")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Document capabilities")).is_none());
}

#[test]
fn denied_open_shows_a_retry_without_actions_for_the_previous_document() {
    let account = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let mut provider = TestPurviewProvider::signed_in(account.clone(), vec![account]);
    provider.deny_open = true;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    provider.inspect_calls = Some(calls.clone());
    let mut h = harness_closed(provider);
    h.state_mut().purview_label_id = "previous-label".into();
    let ctx = h.ctx.clone();
    h.state_mut().open_bytes(&ctx, "protected.pfile", None, b"MicrosoftIRMServices Protected PDF".to_vec()).unwrap();
    h.run_steps(3);
    h.get_by_label("protected.pfile");
    h.get_by_label("Retry opening");
    assert!(h.query_by_label("Sensitivity labels").is_none());
    assert!(h.query_by_label("Apply label").is_none());
    assert!(h.query_by_label("Remove label").is_none());
    h.get_by_label("Retry opening").click();
    for _ in 0..200 {
        h.run_steps(1);
        if calls.load(std::sync::atomic::Ordering::SeqCst) >= 2
            && h.state().purview_signin_prompt.as_ref().is_some_and(|prompt| prompt.error.is_some())
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(calls.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    assert!(h.state().purview_signin_prompt.as_ref().is_some_and(|prompt| prompt.error.is_some()));
    assert_eq!(h.state().session.docs().len(), 1);
}

#[test]
fn opening_a_protected_file_keeps_its_prompt_when_the_account_dialog_is_already_open() {
    let mut h = harness_closed(TestPurviewProvider::signed_out_for_pfile());
    let ctx = h.ctx.clone();
    assert!(h.state_mut().execute(&ctx, "purview.apply_label"));
    h.run_steps(3);
    h.state_mut().open_bytes(&ctx, "protected.pfile", None, b"MicrosoftIRMServices Protected PDF".to_vec()).unwrap();
    h.run_steps(3);
    assert!(h.state().purview_signin_prompt.is_some());
    h.get_by_label("Sign in");
}

#[test]
fn a_second_protected_open_waits_for_the_first_sign_in_prompt() {
    let mut h = harness_pfile(TestPurviewProvider::signed_out_for_pfile());
    let ctx = h.ctx.clone();
    h.state_mut().open_bytes(&ctx, "second.pfile", None, b"MicrosoftIRMServices Protected PDF".to_vec()).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().purview_signin_prompt.as_ref().map(|prompt| prompt.name.as_str()), Some("protected.pfile"));
    h.key_press(egui::Key::Escape);
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().purview_signin_prompt.as_ref().is_some_and(|prompt| prompt.name == "second.pfile") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().purview_signin_prompt.as_ref().map(|prompt| prompt.name.as_str()), Some("second.pfile"));
}

#[test]
fn microsoft_sign_in_does_not_block_the_ui() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let mut provider = TestPurviewProvider::signed_out();
    provider.sign_in_gate = Some(Arc::new(SignInGate { started: started_tx, release: Mutex::new(release_rx) }));
    let mut h = harness(provider);
    h.get_by_label("Sign in").click();
    h.run_steps(1);
    started_rx.recv_timeout(std::time::Duration::from_secs(2)).expect("sign-in worker started");
    h.run_steps(2);
    h.get_by_label("Opening Microsoft sign-in");
    h.get_by_label("Close");

    release_tx.send(()).unwrap();
    for _ in 0..200 {
        h.run_steps(1);
        if h.query_by(|node| node.label().as_deref() == Some("Sensitivity labels")).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Sensitivity labels");
}

#[test]
fn protected_open_waits_for_an_in_progress_account_change() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let account = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let document = PurviewDocument { protected: true, rights: PurviewRights { view: true, ..Default::default() }, ..Default::default() };
    let mut provider = TestPurviewProvider::signed_in_with_document(account.clone(), vec![account], document);
    provider.sign_in_gate = Some(Arc::new(SignInGate { started: started_tx, release: Mutex::new(release_rx) }));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    provider.inspect_calls = Some(calls.clone());
    let mut h = harness_closed(provider);
    let ctx = h.ctx.clone();
    assert!(h.state_mut().execute(&ctx, "purview.sign_in"));
    started_rx.recv_timeout(std::time::Duration::from_secs(2)).expect("sign-in worker started");
    h.state_mut().open_bytes(&ctx, "protected.pdf", None, FIXTURE.to_vec()).unwrap();
    h.run_steps(3);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(h.state().session.docs().len(), 1);

    release_tx.send(()).unwrap();
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().session.docs().len() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(h.state().session.docs().len(), 2);
}

#[test]
fn successful_sign_in_retries_the_held_file_without_an_extra_close() {
    let mut h = harness_pfile(TestPurviewProvider::signed_out_for_pfile());
    h.get_by_label("Sign in").click();
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().session.docs().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.state().purview_signin_prompt.is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Microsoft account")).is_none());
    assert_eq!(h.state().session.docs().len(), 1);
}

#[test]
fn held_file_opens_while_label_policy_is_still_loading() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let mut provider = TestPurviewProvider::signed_out_for_pfile();
    provider.labels_gate = Some(Arc::new(SignInGate { started: started_tx, release: Mutex::new(release_rx) }));
    let mut h = harness_pfile(provider);
    h.get_by_label("Sign in").click();
    let mut labels_started = false;
    for _ in 0..200 {
        h.run_steps(1);
        if started_rx.try_recv().is_ok() {
            labels_started = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(labels_started, "label worker started");
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().session.docs().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().session.docs().len(), 1);
    release_tx.send(()).unwrap();
}

#[test]
fn escape_dismisses_the_pending_purview_sign_in() {
    let mut h = harness_pfile(TestPurviewProvider::signed_out_for_pfile());
    assert!(h.state().purview_signin_prompt.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by(|node| node.label().as_deref() == Some("Microsoft account")).is_none());
    assert!(h.state().purview_signin_prompt.is_none());
}

#[test]
fn cancelling_a_purview_recovery_puts_the_snapshot_back_on_offer() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-purview-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = RecoveryStore::new(&dir);
    let mut crashed = PdfCraftApp::new();
    crashed.enable_recovery(store.clone());
    crashed.open_bytes("Recovered.pdf", Some("/docs/Recovered.pdf".into()), FIXTURE.to_vec()).unwrap();
    assert!(crashed.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
    crashed.autosave_now();
    drop(crashed);

    let recoverable = store.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.enable_recovery(recoverable);
        app.session.set_purview_provider(Arc::new(TestPurviewProvider::signed_out_for_pfile()));
        app
    });
    h.run_steps(4);
    h.get_by_label("Recover unsaved documents?");
    h.get_by_label("Recover").click();
    crate::support::wait_until(&mut h, "Purview recovery prompt", |app| app.purview_signin_prompt.is_some());
    assert!(h.state().recoverable.is_empty());
    h.key_press(egui::Key::Escape);
    crate::support::wait_until(&mut h, "recovery returned to offer", |app| !app.recoverable.is_empty());
    assert!(h.state().purview_signin_prompt.is_none());
    assert_eq!(h.state().recoverable.len(), 1);
    drop(h);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn protect_menu_mip_entry_opens_signed_out_account_gate() {
    let mut h = harness_closed(TestPurviewProvider::signed_out());
    let ctx = h.ctx.clone();
    assert!(h.state_mut().execute(&ctx, "purview.apply_label"));
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Microsoft account");
    h.get_by_label("Sign in");
}

#[test]
fn opening_a_fresh_label_dialog_does_not_reuse_an_old_selection_or_reason() {
    let account = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let mut h = harness_closed(TestPurviewProvider::signed_in(account.clone(), vec![account]));
    h.state_mut().purview_label_id = "old-label".into();
    h.state_mut().purview_justification = "old document reason".into();
    let ctx = h.ctx.clone();
    assert!(h.state_mut().execute(&ctx, "purview.apply_label"));
    h.run_steps(3);
    assert!(h.state().purview_label_id.is_empty());
    assert!(h.state().purview_justification.is_empty());
}

#[test]
fn security_properties_offer_sign_in_then_sensitivity_label_action() {
    let h = harness_security(TestPurviewProvider::signed_out());
    h.get_by_label("Sign in to Microsoft Purview…");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Apply sensitivity label / Microsoft Purview…")).is_none());

    let ada = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let h = harness_security(TestPurviewProvider::signed_in(
        ada,
        vec![PurviewAccount {
            id: "ada".into(),
            display_name: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
            principal_name: "ada@example.com".into(),
        }],
    ));
    h.get_by_label("Apply sensitivity label / Microsoft Purview…");
}

#[test]
fn security_properties_do_not_claim_a_protected_document_is_unrestricted() {
    let account = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let document = PurviewDocument { protected: true, rights: PurviewRights { view: true, ..Default::default() }, ..Default::default() };
    let h = harness_security_with_document(TestPurviewProvider::signed_in_with_document(account.clone(), vec![account.clone()], document.clone()));
    h.get_by_label("Managed by Microsoft Purview");
    assert!(
        h.query_all_by(|node| { node.value().as_deref() == Some("Changing labels or protection on protected documents is not available yet.") })
            .next()
            .is_some()
    );
    assert!(h.query_by(|node| node.label().as_deref() == Some("None — everything is allowed")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("No security")).is_none());
}

#[test]
fn security_properties_report_distinct_protected_capabilities() {
    let account = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let document = PurviewDocument {
        protected: true,
        rights: PurviewRights { view: true, save: true, change_rights: true, ..Default::default() },
        ..Default::default()
    };
    let h = harness_security_with_document(TestPurviewProvider::signed_in_with_document(account.clone(), vec![account], document));
    for (name, expected) in [("MIP edit", "Not allowed"), ("MIP save", "Allowed"), ("MIP export", "Not allowed"), ("MIP change security", "Allowed")]
    {
        assert!(
            h.query_all_by(|node| node.label().as_deref() == Some(name) && node.value().as_deref() == Some(expected)).next().is_some(),
            "{name} should be {expected}"
        );
    }
    assert!(h.query_by_label("Remove protection").is_none(), "Change Rights does not authorize removal");

    let document = PurviewDocument { rights: PurviewRights { export: true, ..document.rights.clone() }, ..document };
    let h = harness_security_with_document(TestPurviewProvider::signed_in_with_document(account, vec![], document));
    h.get_by_label("Remove protection");
}

#[test]
fn label_only_security_properties_do_not_report_irm_restrictions() {
    let account = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let label = PurviewLabel {
        id: "label".into(),
        name: "Internal".into(),
        description: String::new(),
        tooltip: String::new(),
        color: None,
        parent_id: None,
        sensitivity: 0,
        active: true,
        protection: false,
    };
    let document = PurviewDocument { label: Some(label), rights: PurviewRights { view: true, ..Default::default() }, ..Default::default() };
    let h = harness_security_with_document(TestPurviewProvider::signed_in_with_document(account.clone(), vec![account], document));
    h.get_by_label("Change sensitivity label / Microsoft Purview…");
    assert!(h.query_by_label("MIP edit").is_none());
    assert!(h.query_by_label("MIP save").is_none());
    assert!(h.query_by_label("MIP print").is_none());
    assert!(h.query_by_label("MIP copy/extract").is_none());
}

#[test]
fn signed_in_dialog_shows_display_name_and_email_and_replaces_sign_in_with_sign_out() {
    let ada = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let h = harness(TestPurviewProvider::signed_in(
        ada,
        vec![PurviewAccount {
            id: "ada".into(),
            display_name: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
            principal_name: "ada@example.com".into(),
        }],
    ));
    h.get_by_label("Ada Lovelace");
    h.get_by_label("ada@example.com");
    h.get_by_label("Sign out");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Sign in")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Microsoft 365 account")).is_none());
    h.get_by_label("No sensitivity labels are available to this account or tenant policy.");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove label")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove protection")).is_none());
}

#[test]
fn two_signed_in_accounts_have_an_account_picker() {
    let ada = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let grace = PurviewAccount {
        id: "grace".into(),
        display_name: "Grace Hopper".into(),
        email: "grace@example.com".into(),
        principal_name: "grace@example.com".into(),
    };
    let mut h = harness(TestPurviewProvider::signed_in(ada.clone(), vec![ada.clone(), grace]));
    h.get_by_label("Ada Lovelace");
    h.get_by_label("Add account");
    h.get_by_label("Sign out");
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Microsoft account").focus();
    h.run_steps(1);
    assert!(h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Microsoft account").is_focused());
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    h.get_by_label("Grace Hopper - grace@example.com").focus();
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    h.get_by_label("Grace Hopper");
    h.get_by_label("grace@example.com");
}

#[test]
fn switching_accounts_discards_old_labels_when_refresh_fails() {
    let ada = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let grace = PurviewAccount { id: "grace".into(), display_name: "Grace".into(), email: "grace@example.com".into(), principal_name: String::new() };
    let mut provider = TestPurviewProvider::signed_in(ada.clone(), vec![ada, grace]);
    provider.labels_fail = true;
    let mut h = harness(provider);
    h.state_mut().purview_labels = vec![PurviewLabel {
        id: "old-label".into(),
        name: "Old account label".into(),
        description: String::new(),
        tooltip: String::new(),
        color: None,
        parent_id: None,
        sensitivity: 0,
        active: true,
        protection: false,
    }];
    h.run_steps(3);
    h.get_by_label("Old account label - Label only").click();
    h.run_steps(2);
    assert_eq!(h.state().purview_label_id, "old-label");
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Microsoft account").click();
    h.run_steps(2);
    h.get_by_label("Grace - grace@example.com").click();
    for _ in 0..200 {
        h.run_steps(1);
        if h.query_by_label("test label service unavailable").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.state().purview_labels.is_empty());
    assert!(h.state().purview_label_id.is_empty());
    assert!(h.query_by_label("Old account label - Label only").is_none());
    assert!(h.query_by_label("No sensitivity labels are available to this account or tenant policy.").is_none());
    h.get_by_label("test label service unavailable");
}

#[test]
fn signing_out_loads_the_remaining_accounts_labels() {
    let ada = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let grace = PurviewAccount { id: "grace".into(), display_name: "Grace".into(), email: "grace@example.com".into(), principal_name: String::new() };
    let label = |id: &str, name: &str| PurviewLabel {
        id: id.into(),
        name: name.into(),
        description: String::new(),
        tooltip: String::new(),
        color: None,
        parent_id: None,
        sensitivity: 0,
        active: true,
        protection: false,
    };
    let mut provider = TestPurviewProvider::signed_in(ada.clone(), vec![ada, grace]);
    provider.labels = vec![label("grace-label", "Grace label")];
    let mut h = harness(provider);
    h.state_mut().purview_labels = vec![label("ada-label", "Ada label")];
    h.run_steps(3);
    let reason = "Justification (required for policy downgrades or removals)";
    h.get_by(|node| {
        node.label().as_deref() == Some(reason) && matches!(node.role(), egui::accesskit::Role::TextInput | egui::accesskit::Role::MultilineTextInput)
    })
    .click();
    h.run_steps(1);
    h.get_by(|node| {
        node.label().as_deref() == Some(reason) && matches!(node.role(), egui::accesskit::Role::TextInput | egui::accesskit::Role::MultilineTextInput)
    })
    .type_text("old account reason");
    h.run_steps(2);
    assert_eq!(h.state().purview_justification, "old account reason");
    h.get_by_label("Sign out").click();
    for _ in 0..200 {
        h.run_steps(1);
        if h.query_by_label("Grace label - Label only").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("Grace");
    h.get_by_label("Grace label - Label only");
    assert!(h.query_by_label("Ada label - Label only").is_none());
    assert!(h.state().purview_justification.is_empty());
}

#[test]
fn refreshing_labels_does_not_block_the_dialog_or_allow_account_changes() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let account = PurviewAccount { id: "ada".into(), display_name: "Ada".into(), email: "ada@example.com".into(), principal_name: String::new() };
    let mut provider = TestPurviewProvider::signed_in(account.clone(), vec![account]);
    provider.labels_gate = Some(Arc::new(SignInGate { started: started_tx, release: Mutex::new(release_rx) }));
    let mut h = harness(provider);
    h.get_by_label("Refresh labels").click();
    h.run_steps(1);
    started_rx.recv_timeout(std::time::Duration::from_secs(2)).expect("label worker started");
    h.run_steps(2);
    h.get_by_label("Loading sensitivity labels…");
    h.get_by_label("Close");
    assert!(h.query_by_label("Sign out").is_none());
    assert!(h.query_by_label("Add account").is_none());

    release_tx.send(()).unwrap();
    for _ in 0..200 {
        h.run_steps(1);
        if h.query_by_label("No sensitivity labels are available to this account or tenant policy.").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.get_by_label("No sensitivity labels are available to this account or tenant policy.");
    h.get_by_label("Sign out");
}

#[test]
fn sensitivity_label_can_be_selected_by_keyboard() {
    let ada = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let mut h = harness(TestPurviewProvider::signed_in(ada.clone(), vec![ada]));
    h.state_mut().purview_labels = vec![PurviewLabel {
        id: "confidential".into(),
        name: "Confidential".into(),
        description: String::new(),
        tooltip: String::new(),
        color: None,
        parent_id: None,
        sensitivity: 0,
        active: true,
        protection: false,
    }];
    h.run_steps(3);
    let label = "Confidential - Label only";
    h.get_by_role_and_label(egui::accesskit::Role::RadioButton, label).focus();
    h.run_steps(1);
    assert!(h.get_by_role_and_label(egui::accesskit::Role::RadioButton, label).is_focused());
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert_eq!(h.state().purview_label_id, "confidential");
    assert_eq!(h.get_by_role_and_label(egui::accesskit::Role::RadioButton, label).accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    h.get_by_label("Apply label");
}

#[test]
fn labeled_document_shows_owner_label_and_dynamic_capabilities() {
    let ada = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let document = PurviewDocument {
        protected: true,
        label: Some(PurviewLabel {
            id: "confidential".into(),
            name: "Confidential".into(),
            description: "Internal project information".into(),
            tooltip: "Use only inside Storytold".into(),
            color: Some("#356FA5".into()),
            parent_id: None,
            sensitivity: 2,
            active: true,
            protection: true,
        }),
        rights: PurviewRights {
            owner: false,
            view: true,
            edit: true,
            save: true,
            comment: true,
            extract: false,
            export: false,
            print: true,
            forward: false,
            reply: false,
            reply_all: false,
            view_rights: false,
            change_rights: false,
            object_model: false,
            document_edit: true,
        },
        owner: Some("Grace Hopper".into()),
        content_id: Some("policy-123".into()),
        labeling_required: false,
        default_label_id: None,
        downgrade_justification_required: true,
    };
    let h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada], document));
    h.get_by_label("Protected");
    h.get_by_label("Confidential");
    h.get_by_label("Grace Hopper");
    h.get_by_label("policy-123");
    h.get_by_label("Document capabilities");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove label")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove protection")).is_none());
}

#[test]
fn protected_label_actions_follow_owner_rights_and_keep_account_changes_gated() {
    let ada = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let document = PurviewDocument {
        protected: true,
        label: Some(PurviewLabel {
            id: "confidential".into(),
            name: "Confidential".into(),
            description: "Internal project information".into(),
            tooltip: "Use only inside Storytold".into(),
            color: None,
            parent_id: None,
            sensitivity: 2,
            active: true,
            protection: true,
        }),
        rights: PurviewRights {
            owner: true,
            view: true,
            edit: true,
            save: true,
            comment: true,
            extract: true,
            export: true,
            print: true,
            forward: false,
            reply: false,
            reply_all: false,
            view_rights: true,
            change_rights: true,
            object_model: false,
            document_edit: true,
        },
        owner: Some("Ada Lovelace".into()),
        content_id: Some("policy-456".into()),
        labeling_required: false,
        default_label_id: None,
        downgrade_justification_required: false,
    };
    let h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada.clone()], document.clone()));
    h.get_by_label("Renew Microsoft sign-in");
    h.get_by_label("Remove label");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Add account")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Sign out")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove protection")).is_none());

    let label_only = PurviewDocument {
        protected: false,
        rights: PurviewRights { view: true, ..Default::default() },
        default_label_id: Some("other-default".into()),
        ..document
    };
    let h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada], label_only));
    h.get_by_label("Remove label");
    h.get_by_label("Add account");
    h.get_by_label("Sign out");
    h.get_by_label("A default label is configured but is not applied automatically.");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove protection")).is_none());
}

#[test]
fn renewing_sign_in_revalidates_rights_without_losing_protected_edits() {
    let account = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let document = PurviewDocument { protected: true, rights: PurviewRights { owner: true, view: true, ..Default::default() }, ..Default::default() };
    let mut h = harness_with_document(TestPurviewProvider::signed_in_with_document(account.clone(), vec![account], document));
    let id = h.state().session.docs()[0].id;
    h.state_mut().session.apply(id, Edit::SetInfo { key: "Title".into(), value: "unsaved".into() }).unwrap();
    h.get_by_label("Renew Microsoft sign-in").click();
    crate::support::wait_until(&mut h, "renewed Purview sign-in", |app| app.toast.as_ref().is_some_and(|(message, _)| message == "Signed in"));
    assert_eq!(h.state().session.get(id).unwrap().info_value("Title").as_deref(), Some("unsaved"));
    assert!(h.state().session.get(id).unwrap().purview().unwrap().rights.owner);
}

#[test]
fn protected_pages_cannot_be_copied_or_combined_without_export_right() {
    let ada = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let document =
        PurviewDocument { protected: true, rights: PurviewRights { view: true, extract: true, ..Default::default() }, ..Default::default() };
    let mut h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada.clone()], document.clone()));
    let ctx = h.ctx.clone();
    h.state_mut().set_option(&ctx, "dialog", "none").unwrap();
    h.state_mut().copy_pages(&ctx, false);
    assert!(h.state().page_clipboard.is_none());
    h.state_mut().open_bytes(&ctx, "another.pdf", None, FIXTURE.to_vec()).unwrap();
    h.state_mut().execute(&ctx, "page.combine");
    h.run_steps(3);
    h.get_by_label("Add open documents").click();
    h.run_steps(2);
    h.get_by_label("All open documents").click();
    h.run_steps(3);
    assert!(h.state().combine_parts_shown().is_empty());
    assert!(crate::support::shown_notice(&h).is_some_and(|message| message.contains("exporting unprotected content")));

    let allowed = PurviewDocument { rights: PurviewRights { export: true, ..document.rights }, ..document };
    let mut h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada], allowed));
    let ctx = h.ctx.clone();
    h.state_mut().set_option(&ctx, "dialog", "none").unwrap();
    h.state_mut().copy_pages(&ctx, false);
    assert!(h.state().page_clipboard.is_some());
}

#[test]
fn protected_attachments_cannot_open_as_unprotected_documents_without_export_right() {
    let ada = PurviewAccount {
        id: "ada".into(),
        display_name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        principal_name: "ada@example.com".into(),
    };
    let document = PurviewDocument {
        protected: true,
        rights: PurviewRights { view: true, document_edit: true, extract: true, ..Default::default() },
        ..Default::default()
    };
    let mut h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada], document));
    let id = h.state().session.docs()[0].id;
    h.state_mut()
        .session
        .apply(
            id,
            pdfcraft_engine::Edit::AddAttachment {
                name: "child.pdf".into(),
                bytes: FIXTURE.to_vec(),
                description: None,
                mime: Some("application/pdf".into()),
            },
        )
        .unwrap();
    assert_eq!(h.state().session.get(id).unwrap().info.attachments.len(), 1);
    let ctx = h.ctx.clone();
    h.state_mut().attachment_action(&ctx, id, 0, true);
    h.run_steps(2);
    assert_eq!(h.state().session.docs().len(), 1);
    assert!(crate::support::shown_notice(&h).is_some_and(|message| message.contains("exporting unprotected content")));
}
