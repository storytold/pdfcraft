//! Microsoft account presentation in the real Purview dialog (egui_kittest).

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_purview::{PurviewAccount, PurviewDocument, PurviewError, PurviewLabel, PurviewOpen, PurviewProvider, PurviewRights, PurviewStatus};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, PropsTab};

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
        *self.status.lock().unwrap_or_else(|poison| poison.into_inner()) = PurviewStatus::SignedOut;
        *self.active.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
        self.accounts.lock().unwrap_or_else(|poison| poison.into_inner()).clear();
        Ok(())
    }

    fn inspect(&self, _name: &str, bytes: &Arc<Vec<u8>>) -> Result<Option<PurviewOpen>, PurviewError> {
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
        Ok(Vec::new())
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.dialog = Some(Dialog::Purview);
        app
    });
    h.run_steps(4);
    h
}

fn harness_closed(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app
    });
    h.run_steps(4);
    h
}

fn harness_security(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.dialog = Some(Dialog::Properties(PropsTab::Security));
        app
    });
    h.run_steps(4);
    h
}

fn harness_security_with_document(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.open_bytes("protected.pdf", None, FIXTURE.to_vec()).unwrap();
        app.dialog = Some(Dialog::Properties(PropsTab::Security));
        app
    });
    h.run_steps(4);
    h
}

fn harness_with_document(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.open_bytes("protected.pdf", None, FIXTURE.to_vec()).unwrap();
        app.dialog = Some(Dialog::Purview);
        app
    });
    h.run_steps(4);
    h
}

fn harness_pfile(provider: TestPurviewProvider) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.session.set_purview_provider(Arc::new(provider));
        app.open_bytes(
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
    h.get_by_label("Microsoft account");
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
fn opening_a_pfile_automatically_opens_the_signed_out_sign_in_gate() {
    let h = harness_pfile(TestPurviewProvider::signed_out_for_pfile());
    h.get_by_label("Microsoft account");
    h.get_by_label("Sign in");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Sensitivity labels")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Document capabilities")).is_none());
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
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().session.docs().len(), 1);
}

#[test]
fn escape_dismisses_the_pending_purview_sign_in() {
    let mut h = harness_pfile(TestPurviewProvider::signed_out_for_pfile());
    assert!(h.state().purview_signin_prompt.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert!(h.state().purview_signin_prompt.is_none());
}

#[test]
fn protect_menu_mip_entry_opens_signed_out_account_gate() {
    let mut h = harness_closed(TestPurviewProvider::signed_out());
    assert!(h.state_mut().execute("purview.apply_label"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Purview));
    h.get_by_label("Microsoft account");
    h.get_by_label("Sign in");
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
    let h = harness_security_with_document(TestPurviewProvider::signed_in_with_document(account.clone(), vec![account], document));
    h.get_by_label("Managed by Microsoft Purview");
    assert!(h.query_by(|node| node.label().as_deref() == Some("None — everything is allowed")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("No security")).is_none());
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
fn protected_label_actions_are_unavailable_even_with_owner_rights() {
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
    h.get_by_label("Changing labels or protection on protected documents is not available yet.");
    h.get_by_label("Close all Microsoft Purview-protected documents before changing accounts.");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Add account")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Sign out")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove label")).is_none());
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove protection")).is_none());

    let label_only = PurviewDocument { protected: false, default_label_id: Some("other-default".into()), ..document };
    let h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada], label_only));
    h.get_by_label("Remove label");
    h.get_by_label("Add account");
    h.get_by_label("Sign out");
    h.get_by_label("A default label is configured but is not applied automatically.");
    assert!(h.query_by(|node| node.label().as_deref() == Some("Remove protection")).is_none());
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
    h.state_mut().dialog = None;
    h.state_mut().copy_pages(false);
    assert!(h.state().page_clipboard.is_none());
    h.state_mut().open_bytes("another.pdf", None, FIXTURE.to_vec()).unwrap();
    h.state_mut().execute("page.combine");
    h.run_steps(3);
    h.get_by_label("Add open documents").click();
    h.run_steps(2);
    h.get_by_label("All open documents").click();
    h.run_steps(3);
    assert!(h.state().combine_parts_shown().is_empty());
    assert!(h.state().toast.as_ref().is_some_and(|(message, _)| message.contains("exporting unprotected content")));

    let allowed = PurviewDocument { rights: PurviewRights { export: true, ..document.rights }, ..document };
    let mut h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada], allowed));
    h.state_mut().dialog = None;
    h.state_mut().copy_pages(false);
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
        rights: PurviewRights { view: true, edit: true, extract: true, ..Default::default() },
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
    h.state_mut().attachment_action(id, 0, true);
    assert_eq!(h.state().session.docs().len(), 1);
    assert!(h.state().toast.as_ref().is_some_and(|(message, _)| message.contains("exporting unprotected content")));
}
