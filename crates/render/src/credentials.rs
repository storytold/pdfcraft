//! An authenticated, memory-only context over the original encrypted PDF bytes.
use crate::OpenError;
use hayro::hayro_syntax::{DecryptionError, DecryptionTarget, ExternalDecryptor, LoadPdfError, Pdf, object::ObjectIdentifier};
use pdfcraft_cos::{CosError, Document, SecurityHandler};
use std::sync::Arc;

/// A reusable document key bound to the original encryption parameters and file identity.
/// The encrypted source stays in memory; no decrypted PDF or token credential is stored.
#[derive(Clone)]
pub struct AuthenticatedDocument {
    source: Arc<Document>,
}
impl std::fmt::Debug for AuthenticatedDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticatedDocument").finish_non_exhaustive()
    }
}
impl PartialEq for AuthenticatedDocument {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
    }
}
impl AuthenticatedDocument {
    /// Retain an authenticated public-key context. No password, PIN or private key is retained.
    pub fn new(source: Document) -> Result<Self, CosError> {
        if source.security().is_none_or(|h| h.dict().filter != b"Adobe.PubSec") {
            return Err(CosError::Security("expected an authenticated certificate-encrypted document".into()));
        }
        Ok(Self { source: Arc::new(source) })
    }
    /// Reuse the credential only if encryption parameters and file identity still match.
    pub fn open(&self, bytes: Arc<Vec<u8>>) -> Result<Document, CosError> {
        let opened = self.source.open_with_same_credentials(bytes)?;
        if opened.security().is_none() {
            return Err(CosError::NeedsCertificate);
        }
        Ok(opened)
    }
    pub(crate) fn open_limited(&self, bytes: Arc<Vec<u8>>, limit: usize) -> Result<Document, CosError> {
        let opened = self.source.open_with_same_credentials_and_limit(bytes, limit)?;
        if opened.security().is_none() {
            return Err(CosError::NeedsCertificate);
        }
        Ok(opened)
    }
}

#[derive(Debug)]
struct ObjectDecryptor(Arc<SecurityHandler>);
impl ExternalDecryptor for ObjectDecryptor {
    fn decrypt(&self, id: ObjectIdentifier, data: &[u8], target: DecryptionTarget) -> Option<Vec<u8>> {
        let number = u32::try_from(id.obj_number).ok()?;
        let generation = u16::try_from(id.gen_number).ok()?;
        match target {
            DecryptionTarget::String => Some(self.0.decrypt_string(number, generation, data)),
            DecryptionTarget::Stream { metadata, embedded_file, filter } => {
                if filter.as_deref().is_some_and(|n| n != b"DefaultCryptFilter" && n != b"Identity") {
                    return None;
                }
                if metadata && !self.0.encrypts_metadata() {
                    return Some(data.to_vec());
                }
                let kind = match &filter {
                    Some(name) => pdfcraft_cos::CryptStreamKind::Named(name),
                    None if embedded_file => pdfcraft_cos::CryptStreamKind::EmbeddedFile,
                    None => pdfcraft_cos::CryptStreamKind::Normal,
                };
                Some(self.0.decrypt_stream(number, generation, data, kind))
            }
        }
    }
}

pub(crate) fn parse_pdf(bytes: Arc<Vec<u8>>, password: Option<&str>, authenticated: Option<&AuthenticatedDocument>) -> Result<Pdf, OpenError> {
    let result = if let Some(authenticated) = authenticated {
        let source = authenticated.open(bytes.clone()).map_err(|e| OpenError::Invalid(e.to_string()))?;
        let handler = source.security().ok_or_else(|| OpenError::Invalid("the authenticated document lost its encryption".into()))?;
        Pdf::new_with_external_decryptor(bytes, Arc::new(ObjectDecryptor(Arc::new(handler.clone()))))
    } else {
        Pdf::new_with_password(bytes, password.unwrap_or(""))
    };
    result.map_err(|error| match error {
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) if password.is_some() => OpenError::WrongPassword,
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => OpenError::NeedsPassword,
        LoadPdfError::Decryption(other) => OpenError::Unsupported(format!("encryption not supported: {other:?}")),
        LoadPdfError::Invalid => OpenError::Invalid("no readable page tree or cross-reference data was found".into()),
    })
}
