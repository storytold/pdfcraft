//! Microsoft Purview / MIP policy types and provider contract.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PurviewStatus {
    Ready,
    NotConfigured,
    SignedOut,
    SigningIn,
    SdkUnavailable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PurviewAccount {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub principal_name: String,
}

impl PurviewAccount {
    pub fn label(&self) -> String {
        let display = self.display_name.trim();
        let email = self.email.trim();
        let principal = self.principal_name.trim();
        let secondary = if !email.is_empty() {
            email
        } else if !principal.is_empty() {
            principal
        } else {
            ""
        };
        if !display.is_empty() && !secondary.is_empty() && display != secondary {
            format!("{display} - {secondary}")
        } else if !display.is_empty() {
            display.to_owned()
        } else if !secondary.is_empty() {
            secondary.to_owned()
        } else {
            "Microsoft account".to_owned()
        }
    }

    pub fn initials(&self) -> String {
        let source = if self.display_name.trim().is_empty() { self.email.trim() } else { self.display_name.trim() };
        let mut out = String::new();
        for word in source.split_whitespace().take(2) {
            if let Some(first) = word.chars().next() {
                out.extend(first.to_uppercase());
            }
        }
        if out.is_empty() { "M".to_owned() } else { out }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurviewLabel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tooltip: String,
    pub color: Option<String>,
    pub parent_id: Option<String>,
    pub sensitivity: i32,
    pub active: bool,
    pub protection: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PurviewRights {
    pub owner: bool,
    pub view: bool,
    pub edit: bool,
    pub save: bool,
    pub comment: bool,
    pub extract: bool,
    pub export: bool,
    pub print: bool,
    pub forward: bool,
    pub reply: bool,
    pub reply_all: bool,
    pub view_rights: bool,
    pub change_rights: bool,
    pub object_model: bool,
    pub document_edit: bool,
}

impl PurviewRights {
    pub fn unrestricted(&self) -> bool {
        self.owner
    }

    pub fn can_view(&self) -> bool {
        self.owner || self.view
    }

    pub fn can_save(&self) -> bool {
        self.owner || self.save || self.edit
    }

    pub fn can_copy(&self) -> bool {
        self.owner || self.extract
    }

    pub fn can_print(&self) -> bool {
        self.owner || self.print
    }

    pub fn can_edit(&self) -> bool {
        self.owner || self.document_edit
    }

    pub fn can_comment(&self) -> bool {
        self.owner || self.comment || self.document_edit
    }

    pub fn can_export(&self) -> bool {
        self.owner || self.export
    }

    pub fn can_change_security(&self) -> bool {
        self.owner || self.export || self.change_rights
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PurviewDocument {
    pub protected: bool,
    pub label: Option<PurviewLabel>,
    pub rights: PurviewRights,
    pub owner: Option<String>,
    pub content_id: Option<String>,
    pub labeling_required: bool,
    pub default_label_id: Option<String>,
    pub downgrade_justification_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurviewOpen {
    pub handle: u64,
    pub original: Arc<Vec<u8>>,
    pub bytes: Arc<Vec<u8>>,
    pub document: PurviewDocument,
}

#[derive(Clone)]
pub struct PurviewDocumentState {
    pub handle: u64,
    pub original: Arc<Vec<u8>>,
    pub document: PurviewDocument,
    pub provider: Arc<dyn PurviewProvider>,
}

#[derive(Debug, thiserror::Error)]
pub enum PurviewError {
    #[error("Microsoft Purview sign-in is not configured: this build has no approved Storytold Entra application client ID")]
    NotConfigured,
    #[error("the Microsoft Purview MIP SDK is unavailable")]
    SdkUnavailable,
    #[error("Microsoft Entra sign-in is required")]
    SignInRequired,
    #[error("the signed-in account does not have permission to open this protected document")]
    NoAccess,
    #[error("removing or changing protection requires Export or Owner rights")]
    ExportRequired,
    #[error("the protection label policy requires justification")]
    JustificationRequired,
    #[error("{0}")]
    Native(String),
}

pub trait PurviewProvider: Send + Sync {
    fn status(&self) -> PurviewStatus;
    fn account(&self) -> Option<PurviewAccount>;
    fn accounts(&self) -> Vec<PurviewAccount>;
    fn switch_account(&self, account_id: &str) -> Result<(), PurviewError>;
    fn sign_in(&self, username_hint: Option<&str>) -> Result<(), PurviewError>;
    fn reauthenticate_active(&self, username_hint: Option<&str>) -> Result<(), PurviewError> {
        self.sign_in(username_hint)
    }
    fn sign_out(&self) -> Result<(), PurviewError>;
    fn inspect(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<Option<PurviewOpen>, PurviewError>;
    fn labels(&self) -> Result<Vec<PurviewLabel>, PurviewError>;
    fn apply_label(
        &self,
        handle: u64,
        original: &Arc<Vec<u8>>,
        working_bytes: &Arc<Vec<u8>>,
        label_id: &str,
        justification: Option<&str>,
    ) -> Result<PurviewOpen, PurviewError>;
    fn remove_label(
        &self,
        handle: u64,
        original: &Arc<Vec<u8>>,
        working_bytes: &Arc<Vec<u8>>,
        justification: Option<&str>,
    ) -> Result<PurviewOpen, PurviewError>;
    fn remove_protection(&self, handle: u64, original: &Arc<Vec<u8>>, working_bytes: &Arc<Vec<u8>>) -> Result<PurviewOpen, PurviewError>;
    fn republish(&self, handle: u64, original: &Arc<Vec<u8>>, working_bytes: &Arc<Vec<u8>>) -> Result<Arc<Vec<u8>>, PurviewError>;
    fn close(&self, handle: u64);
}

#[derive(Default)]
pub struct UnavailablePurviewProvider;

impl PurviewProvider for UnavailablePurviewProvider {
    fn status(&self) -> PurviewStatus {
        PurviewStatus::SdkUnavailable
    }

    fn account(&self) -> Option<PurviewAccount> {
        None
    }

    fn accounts(&self) -> Vec<PurviewAccount> {
        Vec::new()
    }

    fn switch_account(&self, _account_id: &str) -> Result<(), PurviewError> {
        Err(PurviewError::SdkUnavailable)
    }

    fn sign_in(&self, _username_hint: Option<&str>) -> Result<(), PurviewError> {
        Err(PurviewError::SdkUnavailable)
    }

    fn sign_out(&self) -> Result<(), PurviewError> {
        Ok(())
    }

    fn inspect(&self, _name: &str, _bytes: &Arc<Vec<u8>>) -> Result<Option<PurviewOpen>, PurviewError> {
        Ok(None)
    }

    fn labels(&self) -> Result<Vec<PurviewLabel>, PurviewError> {
        Err(PurviewError::SdkUnavailable)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rights_map_to_pdf_operations_conservatively() {
        let view_only = PurviewRights { view: true, ..PurviewRights::default() };
        assert!(!view_only.can_copy());
        assert!(!view_only.can_print());
        assert!(view_only.can_view());
        assert!(!view_only.can_edit());
        assert!(!view_only.can_save());

        let extract = PurviewRights { owner: false, view: true, extract: true, ..PurviewRights::default() };
        assert!(extract.can_copy());
        assert!(!extract.can_export(), "copy rights do not authorize an unprotected document export");
        assert!(PurviewRights { export: true, ..extract }.can_export());

        let owner = PurviewRights { owner: true, ..PurviewRights::default() };
        assert!(owner.can_copy());
        assert!(owner.can_view());
        assert!(owner.can_print());
        assert!(owner.can_edit());
        assert!(owner.can_save());
        assert!(owner.can_export());
        assert!(owner.can_change_security());

        let save_only = PurviewRights { save: true, view: true, ..PurviewRights::default() };
        assert!(save_only.can_save());
        assert!(!save_only.can_edit());

        let sdk_edit = PurviewRights { edit: true, view: true, ..PurviewRights::default() };
        assert!(sdk_edit.can_save(), "the SDK EDIT right saves the protected document");
        assert!(!sdk_edit.can_edit(), "the SDK EDIT right does not grant DOCEDIT");
        assert!(!sdk_edit.can_comment());

        let doc_edit = PurviewRights { document_edit: true, view: true, ..PurviewRights::default() };
        assert!(doc_edit.can_edit());
        assert!(doc_edit.can_comment());
        assert!(!doc_edit.can_save(), "DOCEDIT alone is not the SDK Save right");
    }

    #[test]
    fn unavailable_provider_does_not_claim_mip_support() {
        let provider = UnavailablePurviewProvider;
        assert_eq!(provider.status(), PurviewStatus::SdkUnavailable);
        assert!(matches!(provider.labels(), Err(PurviewError::SdkUnavailable)));
    }
}
