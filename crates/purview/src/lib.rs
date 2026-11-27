//! Native Microsoft Purview / MIP provider and its public policy types.

#[cfg(feature = "mip-auth")]
pub mod auth;
mod native;

pub use native::NativePurviewProvider;
pub use pdfcraft_purview_policy::{
    PurviewAccount, PurviewCheck, PurviewDocument, PurviewDocumentState, PurviewError, PurviewLabel, PurviewOpen, PurviewProvider, PurviewRights,
    PurviewSetupCheck, PurviewStatus, UnavailablePurviewProvider,
};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn native_provider_without_app_id_is_not_configured() {
        let provider = NativePurviewProvider::new(Some(""));
        assert_eq!(provider.status(), PurviewStatus::NotConfigured);
        assert!(matches!(provider.sign_in(None), Err(PurviewError::NotConfigured)));
        assert_eq!(provider.inspect("plain.pdf", &Arc::new(vec![0])).unwrap(), None);
        let setup = provider.setup_check().unwrap();
        assert!(matches!(setup.app_id, PurviewCheck::Failed(_)));
        assert_eq!(setup.public_client, PurviewCheck::NotChecked);
    }

    #[test]
    fn setup_check_reports_app_id_and_required_permissions() {
        let provider = NativePurviewProvider::new(Some("79a290f0-e7f0-4bd2-bdbe-8efab080402c"));
        let setup = provider.setup_check().unwrap();
        assert_eq!(setup.app_id, PurviewCheck::Passed);
        assert!(setup.redirect_uri.starts_with("http://localhost:"));
        assert!(setup.required_permissions.iter().any(|permission| permission.contains("user_impersonation")));
        assert!(setup.required_permissions.iter().any(|permission| permission.contains("UnifiedPolicy.User.Read")));
    }

    #[test]
    #[cfg(not(feature = "mip-sdk"))]
    fn ordinary_build_recognizes_mip_payloads_without_silently_opening_them_plain() {
        let provider = NativePurviewProvider::new(Some("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246"));
        let pfile = Arc::new(b"%PDF-1.7\n/EncryptedPayload\n/Subtype /MicrosoftIRMServices\nMicrosoftIRMServices Protected PDF".to_vec());
        assert_eq!(provider.status(), PurviewStatus::SdkUnavailable);
        assert!(matches!(provider.sign_in(None), Err(PurviewError::SdkUnavailable)));
        assert!(matches!(provider.inspect("protected.pdf", &pfile), Err(PurviewError::SdkUnavailable)));
        assert!(matches!(provider.inspect("protected.pfile", &Arc::new(b"pkg".to_vec())), Err(PurviewError::SdkUnavailable)));
        assert_eq!(provider.inspect("plain.pdf", &Arc::new(b"%PDF-1.7\nplain".to_vec())).unwrap(), None);
    }
}
