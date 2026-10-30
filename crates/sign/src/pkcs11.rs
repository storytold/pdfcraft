//! Linux smartcard operations isolated in short-lived helper processes.
//!
//! Executables must call [`worker_if_requested`] before normal startup. Libraries and
//! test harnesses can select such an executable with the host-only configuration
//! `PDFCRAFT_PKCS11_HELPER`. Providers are never loaded in the calling process.

mod backend;
mod process;
mod worker;

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, Scheme};
use crate::{Certificate, DigitalId, SignError};
pub use process::{CancelOnDrop, Cancellation};
use std::sync::{Arc, Mutex};
pub use worker::worker_if_requested;
use worker::{Identity, Request};
use zeroize::Zeroizing;

/// The stable public certificate fingerprint, without device handles or credentials.
pub fn reference(certificate: &Certificate) -> String {
    backend::reference(certificate)
}

/// Enumerate signing certificates without authentication.
pub fn list() -> Result<Vec<DigitalId>, SignError> {
    records(false)?.into_iter().map(|record| signing_id(record, None)).collect()
}

/// A discovery-time signing selection. It holds only public certificate and token metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct SigningIdentity {
    record: Identity,
    certificate: Certificate,
}
impl SigningIdentity {
    pub fn certificate(&self) -> &Certificate {
        &self.certificate
    }
    pub fn token_label(&self) -> &str {
        self.record.label.as_deref().unwrap_or("")
    }
    pub fn token_serial(&self) -> &str {
        &self.record.serial
    }
    /// Bind a one-use PIN to this exact discovered card, never a freshly selected replacement.
    pub fn with_pin(&self, pin: Option<&str>) -> Result<DigitalId, SignError> {
        if pin.is_some_and(|p| p.len() > 256) {
            return Err(SignError::Malformed("the token PIN must not exceed 256 UTF-8 bytes".into()));
        }
        if self.record.serial.trim().is_empty() {
            return Err(SignError::TokenChanged);
        }
        signing_id(self.record.clone(), pin)
    }
}
/// Enumerate selections that remain bound to their discovery-time card when used later.
pub fn signing_identities() -> Result<Vec<SigningIdentity>, SignError> {
    records(false)?
        .into_iter()
        .map(|record| {
            let certificate = Certificate::parse(&record.certificate)?;
            Ok(SigningIdentity { record, certificate })
        })
        .collect()
}

fn records(decryption: bool) -> Result<Vec<Identity>, SignError> {
    let response = process::request(&Request::List { decryption }, None)?;
    serde_json::from_slice(&response).map_err(|_| SignError::Crypto("invalid smartcard discovery response".into()))
}

fn signing_id(record: Identity, pin: Option<&str>) -> Result<DigitalId, SignError> {
    let certificate = Certificate::parse(&record.certificate)?;
    let chain = record.chain.iter().map(|der| Certificate::parse(der)).collect::<Result<_, _>>()?;
    Ok(DigitalId {
        key: PrivateKey::external(
            certificate.public_key.clone(),
            Arc::new(RemoteKey {
                record: record.clone(),
                certificate: certificate.clone(),
                pin: Mutex::new(pin.map(|p| Zeroizing::new(p.to_owned()))),
            }),
        ),
        certificate,
        chain,
        friendly_name: record.label,
    })
}

/// Select a unique signing identity. The PIN is used once, at signing, never discovery.
pub fn find_with_pin(name: &str, pin: Option<&str>) -> Result<DigitalId, SignError> {
    if pin.is_some_and(|p| p.len() > 256) {
        return Err(SignError::Malformed("the token PIN must not exceed 256 UTF-8 bytes".into()));
    }
    let mut matching = records(false)?.into_iter().filter(|i| {
        Certificate::parse(&i.certificate).is_ok_and(|c| reference(&c) == name || c.subject.common_name() == Some(name.trim_start_matches("pkcs11:")))
    });
    let record = matching.next().ok_or_else(|| SignError::Crypto("the selected signing certificate is not present".into()))?;
    if matching.next().is_some() {
        return Err(SignError::Crypto("the signing certificate is ambiguous; leave only the intended token connected".into()));
    }
    signing_id(record, pin)
}

pub fn find(name: &str) -> Result<DigitalId, SignError> {
    find_with_pin(name, None)
}

struct RemoteKey {
    record: Identity,
    certificate: Certificate,
    pin: Mutex<Option<Zeroizing<String>>>,
}
impl ExternalKey for RemoteKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        self.sign_using(alg, msg, process::request)
    }
}
impl RemoteKey {
    fn sign_using(
        &self,
        alg: DigestAlg,
        msg: &[u8],
        request: impl FnOnce(&Request, Option<&str>) -> Result<Zeroizing<Vec<u8>>, SignError>,
    ) -> Result<Vec<u8>, SignError> {
        if msg.len() > 128 * 1024 {
            return Err(SignError::Malformed("smartcard signing messages must not exceed 128 KiB".into()));
        }
        let pin = self.pin.lock().map_err(|_| SignError::Crypto("the token PIN state is unavailable".into()))?.take();
        let signature = request(
            &Request::Sign { identity: self.record.clone(), algorithm: alg.oid().into(), message: msg.to_vec() },
            pin.as_deref().map(|p| p.as_str()),
        )?;
        // Verify again in the caller, independently of the provider process.
        let scheme = if matches!(self.certificate.public_key, crate::PublicKey::Rsa { .. }) { Scheme::RsaPkcs1 } else { Scheme::Ecdsa };
        if !self.certificate.public_key.verify(scheme, alg, &alg.digest(&[msg]), &signature)? {
            return Err(SignError::Crypto("the token signature does not match the selected certificate".into()));
        }
        Ok(signature.to_vec())
    }
}

/// A public key-management identity. No provider handles or PINs are retained.
pub struct RsaDecryptionIdentity {
    record: Identity,
    certificate: Certificate,
}
impl std::fmt::Debug for RsaDecryptionIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RsaDecryptionIdentity")
            .field("reference", &reference(&self.certificate))
            .field("token_label", &self.record.label)
            .finish_non_exhaustive()
    }
}
impl RsaDecryptionIdentity {
    pub fn certificate(&self) -> &Certificate {
        &self.certificate
    }
    pub fn token_label(&self) -> &str {
        self.record.label.as_deref().unwrap_or("")
    }
    pub fn token_serial(&self) -> &str {
        &self.record.serial
    }
    /// Perform one RSA operation in a helper. Errors never trigger an automatic retry.
    pub fn decrypt_key(&self, encrypted: &[u8], pin: Option<&str>) -> Result<Zeroizing<Vec<u8>>, SignError> {
        let crate::PublicKey::Rsa { n, .. } = &self.certificate.public_key else {
            return Err(SignError::Unsupported("recipient decryption requires RSA".into()));
        };
        if encrypted.len() != n.len() || encrypted.len() > 1024 {
            return Err(SignError::Malformed("recipient RSA ciphertext has an invalid length".into()));
        }
        let key = process::request(&Request::Decrypt { identity: self.record.clone(), ciphertext: encrypted.to_vec() }, pin)?;
        if !matches!(key.len(), 16 | 24 | 32) {
            return Err(SignError::Crypto("the token returned an invalid CMS content key length".into()));
        }
        Ok(key)
    }
}

/// Enumerate RSA key-management certificates, including expired certificates for old files.
pub fn decryption_identities() -> Result<Vec<RsaDecryptionIdentity>, SignError> {
    records(true)?
        .into_iter()
        .map(|record| {
            let certificate = Certificate::parse(&record.certificate)?;
            Ok(RsaDecryptionIdentity { record, certificate })
        })
        .collect()
}

/// Resolve an exact fingerprint without authenticating or guessing between tokens.
pub fn find_decryption_identity(name: &str) -> Result<RsaDecryptionIdentity, SignError> {
    let fingerprint = name
        .strip_prefix("pkcs11:")
        .filter(|f| f.len() == 64 && f.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| SignError::Malformed("expected pkcs11:<64 hexadecimal SHA-256 characters> from encryption_token_ids".into()))?;
    let mut matching =
        decryption_identities()?.into_iter().filter(|i| reference(i.certificate()).get(7..).is_some_and(|f| f.eq_ignore_ascii_case(fingerprint)));
    let identity =
        matching.next().ok_or_else(|| SignError::Crypto("the selected encryption identity is not present; refresh encryption_token_ids".into()))?;
    if matching.next().is_some() {
        return Err(SignError::Crypto("the certificate is present on multiple token identities; leave only the intended token connected".into()));
    }
    Ok(identity)
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    fn selection(serial: &str) -> SigningIdentity {
        let id = crate::pkcs12::open(include_bytes!("../tests/data/ec-p256.p12"), "test").unwrap();
        SigningIdentity {
            record: Identity {
                certificate: id.certificate.raw.clone(),
                chain: Vec::new(),
                label: Some("Selected card".into()),
                serial: serial.into(),
            },
            certificate: id.certificate,
        }
    }
    #[test]
    fn a_bound_identity_keeps_its_discovery_record_and_one_use_pin() {
        let selected = selection("original-A");
        let id = selected.with_pin(Some("disposable-pin")).unwrap();
        assert_eq!(id.certificate.raw, selected.record.certificate);
        let remote = RemoteKey {
            record: selected.record.clone(),
            certificate: selected.certificate.clone(),
            pin: Mutex::new(Some(Zeroizing::new("disposable-pin".into()))),
        };
        let failure = remote.sign_using(DigestAlg::Sha256, b"CMS attributes", |request, pin| {
            let Request::Sign { identity, .. } = request else { panic!("expected sign request") };
            assert_eq!(identity.serial, "original-A");
            assert_eq!(identity.certificate, selected.record.certificate);
            assert_eq!(pin, Some("disposable-pin"));
            Err(SignError::TokenChanged)
        });
        assert!(matches!(failure, Err(SignError::TokenChanged)));
        assert!(remote.pin.lock().unwrap().is_none(), "a rejected operation consumes its PIN");
        assert_ne!(selected, selection("replacement-B"));
        assert!(matches!(selected.with_pin(Some(&"x".repeat(257))), Err(SignError::Malformed(_))));
    }
    #[test]
    fn missing_token_serial_cannot_accept_a_pin() {
        for serial in ["", "   "] {
            assert!(matches!(selection(serial).with_pin(Some("disposable-pin")), Err(SignError::TokenChanged)));
            assert!(matches!(selection(serial).with_pin(None), Err(SignError::TokenChanged)));
        }
    }
}
