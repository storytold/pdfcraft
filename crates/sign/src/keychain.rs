//! Digital IDs from the macOS Keychain: identities (a certificate with its private key) that
//! sign through the Security framework. The private key never leaves the Keychain; macOS may
//! ask the user to allow PdfCraft to use it.

use std::path::Path;
use std::sync::Arc;

use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
use security_framework::key::{Algorithm, SecKey};
use security_framework::os::macos::keychain::SecKeychain;

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey};
use crate::{Certificate, DigitalId, SignError};

struct KeychainKey {
    key: SecKey,
    rsa: bool,
}

impl ExternalKey for KeychainKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let a = match (self.rsa, alg) {
            (true, DigestAlg::Sha384) => Algorithm::RSASignatureMessagePKCS1v15SHA384,
            (true, DigestAlg::Sha512) => Algorithm::RSASignatureMessagePKCS1v15SHA512,
            (true, _) => Algorithm::RSASignatureMessagePKCS1v15SHA256,
            (false, DigestAlg::Sha384) => Algorithm::ECDSASignatureMessageX962SHA384,
            (false, DigestAlg::Sha512) => Algorithm::ECDSASignatureMessageX962SHA512,
            (false, _) => Algorithm::ECDSASignatureMessageX962SHA256,
        };
        self.key.create_signature(a, msg).map_err(|e| SignError::Crypto(format!("the Keychain didn't sign: {e}")))
    }
}

/// The `keychain:<SHA-256 of the certificate>` reference PdfCraft keeps for an identity.
pub fn reference(c: &Certificate) -> String {
    let d = DigestAlg::Sha256.digest(&[&c.raw]);
    format!("keychain:{}", d.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// The identity a reference (or a certificate common name) names.
pub fn find(reference_or_name: &str) -> Result<DigitalId, SignError> {
    identities(None)?
        .into_iter()
        .find(|id| {
            reference(&id.certificate) == reference_or_name
                || id.certificate.subject.common_name() == Some(reference_or_name.trim_start_matches("keychain:"))
        })
        .ok_or_else(|| SignError::Crypto(format!("no Keychain identity {reference_or_name}")))
}

/// The signing identities in the user's keychains (or only in the keychain file `keychain`).
/// Identities with keys PdfCraft can't use (other curves, Ed25519) are left out.
pub fn identities(keychain: Option<&Path>) -> Result<Vec<DigitalId>, SignError> {
    let mut search = ItemSearchOptions::new();
    search.class(ItemClass::identity()).load_refs(true).limit(Limit::All);
    let kc;
    if let Some(p) = keychain {
        kc = SecKeychain::open(p).map_err(|e| SignError::Crypto(format!("{}: {e}", p.display())))?;
        search.keychains(std::slice::from_ref(&kc));
    }
    let found = match search.search() {
        Ok(f) => f,
        // "The specified item could not be found in the keychain."
        Err(e) if e.code() == -25300 => return Ok(Vec::new()),
        Err(e) => return Err(SignError::Crypto(format!("the Keychain couldn't be searched: {e}"))),
    };
    let mut out = Vec::new();
    for r in found {
        let SearchResult::Ref(Reference::Identity(id)) = r else { continue };
        let (Ok(cert), Ok(key)) = (id.certificate(), id.private_key()) else { continue };
        let Ok(certificate) = Certificate::parse(&cert.to_der()) else { continue };
        let rsa = matches!(certificate.public_key, PublicKey::Rsa { .. });
        let key = PrivateKey::external(certificate.public_key.clone(), Arc::new(KeychainKey { key, rsa }));
        let friendly_name = Some(certificate.display_name());
        out.push(DigitalId { key, certificate, chain: Vec::new(), friendly_name });
    }
    Ok(out)
}
