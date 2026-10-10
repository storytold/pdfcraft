//! Signing identities in the Windows Current User Personal ("My") certificate store.
//! Private keys remain in CNG; Windows may ask permission to use a key.

use std::sync::Arc;

use rustls_cng::cert::CertContext;
use rustls_cng::key::{AlgorithmGroup, SignaturePadding};
use rustls_cng::store::{CertStore, CertStoreType};

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey, StoreKey};
use crate::{Certificate, DigitalId, SignError};

struct WindowsKey {
    context: CertContext,
    public: PublicKey,
}

impl ExternalKey for WindowsKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let digest = alg.digest(&[msg]);
        // CNG adds the DigestInfo for PKCS #1 v1.5 when given PKCS1 padding.
        let padding = if matches!(self.public, PublicKey::Rsa { .. }) { SignaturePadding::Pkcs1 } else { SignaturePadding::None };
        // A key handle acquired silently can never prompt: smart cards and tokens then refuse
        // to sign (NTE_SILENT_CONTEXT, 0x80090022) instead of asking for the PIN. Acquire the
        // signing handle without the silent flag so the provider can show its own dialog.
        let key = self.context.acquire_key(false).map_err(|e| {
            SignError::Crypto(format!(
                "the Windows certificate store key couldn't be opened for signing (is the smart card or token connected?): {e}"
            ))
        })?;
        let raw = key
            .sign(&digest, padding)
            .map_err(|e| SignError::Crypto(format!("the Windows certificate store didn't sign (key use may have been cancelled): {e}")))?;
        self.public.store_signature_der(&raw)
    }
}

/// The `windows:<SHA-256 of the certificate DER>` reference kept for an identity.
pub fn reference(certificate: &Certificate) -> String {
    let digest = DigestAlg::Sha256.digest(&[&certificate.raw]);
    format!("windows:{}", digest.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Find an identity by its reference or subject common name. Its `chain` holds the issuers
/// Windows finds for the certificate, so a signature embeds them as it does for a file's.
pub fn find(reference_or_name: &str) -> Result<DigitalId, SignError> {
    let (context, mut id) = entries()?
        .into_iter()
        .find(|(_, id)| {
            reference(&id.certificate) == reference_or_name
                || id.certificate.subject.common_name() == Some(reference_or_name.strip_prefix("windows:").unwrap_or(reference_or_name))
        })
        .ok_or_else(|| SignError::Crypto(format!("no Windows certificate store identity {reference_or_name}")))?;
    id.chain = issuers(&context, &id.certificate);
    Ok(id)
}

/// The certificates Windows chains `certificate` to, without the root. Best effort: an
/// identity signs without them too.
fn issuers(context: &CertContext, certificate: &Certificate) -> Vec<Certificate> {
    let Ok(chain) = context.as_chain_der() else { return Vec::new() };
    chain.iter().filter(|der| **der != certificate.raw).filter_map(|der| Certificate::parse(der).ok()).collect()
}

/// List usable RSA, P-256 and P-384 CNG identities in Current User > Personal.
/// Certificates with unsupported keys or without an accessible private key are skipped.
pub fn identities() -> Result<Vec<DigitalId>, SignError> {
    Ok(entries()?.into_iter().map(|(_, id)| id).collect())
}

/// Each usable identity with the store's certificate context it came from.
fn entries() -> Result<Vec<(CertContext, DigitalId)>, SignError> {
    let store = CertStore::open(CertStoreType::CurrentUser, "My")
        .map_err(|e| SignError::Crypto(format!("the Windows Current User Personal store couldn't be opened: {e}")))?;
    let contexts = store.find_all().map_err(|e| SignError::Crypto(format!("the Windows certificate store couldn't be searched: {e}")))?;
    let mut out = Vec::new();
    for context in contexts {
        let Ok(certificate) = Certificate::parse(context.as_der()) else { continue };
        // Enumeration must not open permission or PIN dialogs. Signing may prompt later.
        let Ok(key) = context.acquire_key(true) else { continue };
        // P-521, brainpool and Ed25519 certificates parse (to verify with) but aren't signed with.
        let supported = match certificate.public_key.store_signing_key() {
            Some(StoreKey::Rsa) => key.algorithm_group().is_ok_and(|a| a == AlgorithmGroup::Rsa),
            Some(StoreKey::Ecdsa(want)) => {
                key.algorithm_group().is_ok_and(|a| a == AlgorithmGroup::Ecdsa) && key.bits().is_ok_and(|bits| bits == want)
            }
            None => false,
        };
        if !supported {
            continue;
        }
        // The silent handle was only for the checks above; `sign` acquires its own.
        drop(key);
        let public = certificate.public_key.clone();
        let key = PrivateKey::external(public.clone(), Arc::new(WindowsKey { context: context.clone(), public }));
        let friendly_name = Some(certificate.display_name());
        out.push((context, DigitalId { key, certificate, chain: Vec::new(), friendly_name }));
    }
    Ok(out)
}
