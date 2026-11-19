//! Microsoft account presentation in the real Purview dialog (egui_kittest).

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_purview::{PurviewAccount, PurviewDocument, PurviewError, PurviewLabel, PurviewOpen, PurviewProvider, PurviewRights, PurviewStatus};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, PropsTab};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

struct TestPurviewProvider {
    status: Mutex<PurviewStatus>,
    active: Mutex<Option<PurviewAccount>>,
    accounts: Mutex<Vec<PurviewAccount>>,
    inspect_document: Option<PurviewDocument>,
    inspect_sign_in_required: bool,
}

impl TestPurviewProvider {
    fn signed_out() -> Self {
        Self {
            status: Mutex::new(PurviewStatus::SignedOut),
            active: Mutex::new(None),
            accounts: Mutex::new(Vec::new()),
            inspect_document: None,
            inspect_sign_in_required: false,
        }
    }

    fn signed_out_for_pfile() -> Self {
        Self {
            status: Mutex::new(PurviewStatus::SignedOut),
            active: Mutex::new(None),
            accounts: Mutex::new(Vec::new()),
            inspect_document: None,
            inspect_sign_in_required: true,
        }
    }

    fn signed_in(account: PurviewAccount, accounts: Vec<PurviewAccount>) -> Self {
        Self {
            status: Mutex::new(PurviewStatus::Ready),
            active: Mutex::new(Some(account)),
            accounts: Mutex::new(accounts),
            inspect_document: None,
            inspect_sign_in_required: false,
        }
    }

    fn signed_in_with_document(account: PurviewAccount, accounts: Vec<PurviewAccount>, document: PurviewDocument) -> Self {
        Self {
            status: Mutex::new(PurviewStatus::Ready),
            active: Mutex::new(Some(account)),
            accounts: Mutex::new(accounts),
            inspect_document: Some(document),
            inspect_sign_in_required: false,
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
            return Err(PurviewError::SignInRequired);
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
    let h = harness(TestPurviewProvider::signed_in(ada.clone(), vec![ada.clone(), grace]));
    h.get_by_label("Ada Lovelace");
    h.get_by_label("Add account");
    h.get_by_label("Sign out");
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
fn removable_label_and_protection_actions_require_change_security_rights() {
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
    let h = harness_with_document(TestPurviewProvider::signed_in_with_document(ada.clone(), vec![ada], document));
    h.get_by_label("Remove label");
    h.get_by_label("Remove protection");
}
