//! Signing identities in the Windows Current User Personal ("My") certificate store.
//! Private keys remain in CNG; Windows may ask permission to use a key.

use std::sync::Arc;

use rustls_cng::key::{AlgorithmGroup, NCryptKey, SignaturePadding};
use rustls_cng::store::{CertStore, CertStoreType};

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey};
use crate::{Certificate, DigitalId, SignError};

struct WindowsKey {
    key: NCryptKey,
    public: PublicKey,
}

impl ExternalKey for WindowsKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let digest = alg.digest(&[msg]);
        // CNG adds the DigestInfo for PKCS #1 v1.5 when given PKCS1 padding.
        let padding = if matches!(self.public, PublicKey::Rsa { .. }) { SignaturePadding::Pkcs1 } else { SignaturePadding::None };
        let raw = self
            .key
            .sign(&digest, padding)
            .map_err(|e| SignError::Crypto(format!("the Windows certificate store didn't sign (key use may have been cancelled): {e}")))?;
        match &self.public {
            PublicKey::Rsa { .. } => Ok(raw),
            PublicKey::P256(_) => p256::ecdsa::Signature::from_slice(&raw)
                .map(|s| s.to_der().as_bytes().to_vec())
                .map_err(|e| SignError::Crypto(format!("CNG returned an invalid P-256 signature: {e}"))),
            PublicKey::P384(_) => p384::ecdsa::Signature::from_slice(&raw)
                .map(|s| s.to_der().as_bytes().to_vec())
                .map_err(|e| SignError::Crypto(format!("CNG returned an invalid P-384 signature: {e}"))),
            // `identities` lists only RSA, P-256 and P-384 store keys; anything else can't sign here.
            _ => Err(SignError::Unsupported("signing with this key type through the Windows certificate store".into())),
        }
    }
}

/// The `windows:<SHA-256 of the certificate DER>` reference kept for an identity.
pub fn reference(certificate: &Certificate) -> String {
    let digest = DigestAlg::Sha256.digest(&[&certificate.raw]);
    format!("windows:{}", digest.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Find an identity by its reference or subject common name.
pub fn find(reference_or_name: &str) -> Result<DigitalId, SignError> {
    identities()?
        .into_iter()
        .find(|id| {
            reference(&id.certificate) == reference_or_name
                || id.certificate.subject.common_name() == Some(reference_or_name.strip_prefix("windows:").unwrap_or(reference_or_name))
        })
        .ok_or_else(|| SignError::Crypto(format!("no Windows certificate store identity {reference_or_name}")))
}

/// List usable RSA, P-256 and P-384 CNG identities in Current User > Personal.
/// Certificates with unsupported keys or without an accessible private key are skipped.
pub fn identities() -> Result<Vec<DigitalId>, SignError> {
    let store = CertStore::open(CertStoreType::CurrentUser, "My")
        .map_err(|e| SignError::Crypto(format!("the Windows Current User Personal store couldn't be opened: {e}")))?;
    let contexts = store.find_all().map_err(|e| SignError::Crypto(format!("the Windows certificate store couldn't be searched: {e}")))?;
    let mut out = Vec::new();
    for context in contexts {
        let Ok(certificate) = Certificate::parse(context.as_der()) else { continue };
        // Enumeration must not open permission or PIN dialogs. Signing may prompt later.
        let Ok(mut key) = context.acquire_key(true) else { continue };
        let supported = match &certificate.public_key {
            PublicKey::Rsa { .. } => key.algorithm_group().is_ok_and(|a| a == AlgorithmGroup::Rsa),
            PublicKey::P256(_) => key.algorithm_group().is_ok_and(|a| a == AlgorithmGroup::Ecdsa) && key.bits().is_ok_and(|bits| bits == 256),
            PublicKey::P384(_) => key.algorithm_group().is_ok_and(|a| a == AlgorithmGroup::Ecdsa) && key.bits().is_ok_and(|bits| bits == 384),
            _ => false,
        };
        if !supported {
            continue;
        }
        key.set_silent(false);
        let public = certificate.public_key.clone();
        let key = PrivateKey::external(public.clone(), Arc::new(WindowsKey { key, public }));
        let friendly_name = Some(certificate.display_name());
        out.push(DigitalId { key, certificate, chain: Vec::new(), friendly_name });
    }
    Ok(out)
}
