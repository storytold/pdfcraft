//! Certificate encryption and authentication. Token PINs belong to the caller, never history.

use crate::EditError;
use pdfcraft_cos::{Algorithm, Document};

/// One recipient's public X.509 certificate and ISO 32000-2 public-key permission bits.
#[derive(Clone, Debug, PartialEq)]
pub struct CertificateRecipient {
    /// DER certificate bytes, never a private key.
    pub certificate: Vec<u8>,
    /// Bit 2 grants security control. Other rights follow Table 24; -1 grants every right.
    pub permissions: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CertificateProtection {
    pub recipients: Vec<CertificateRecipient>,
    pub algorithm: Algorithm,
    pub encrypt_metadata: bool,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn protect(doc: &mut Document, protection: &CertificateProtection, cx: &mut crate::edit::EditCtx) -> Result<(), EditError> {
    use crate::sign::{Certificate, Time, envelope};
    use pdfcraft_cos::SecurityHandler;
    use zeroize::Zeroizing;
    let bad = |m: &str| EditError::Protection(m.into());
    if protection.recipients.is_empty() || protection.recipients.len() > 64 {
        return Err(bad("certificate encryption requires 1 to 64 recipients"));
    }
    if !matches!(protection.algorithm, Algorithm::Aes128 | Algorithm::Aes256) {
        return Err(bad("certificate encryption supports aes-128 or aes-256"));
    }
    if !protection.recipients.iter().any(|r| r.permissions & 2 != 0) {
        return Err(bad("at least one recipient must have security-control permission (bit 2)"));
    }
    let mut certificates = Vec::with_capacity(protection.recipients.len());
    let mut seen = std::collections::HashSet::new();
    let mut identifiers = std::collections::HashSet::new();
    for recipient in &protection.recipients {
        if recipient.certificate.len() > 64 * 1024 {
            return Err(bad("a recipient certificate must not exceed 64 KiB"));
        }
        let cert = Certificate::parse(&recipient.certificate)?;
        if !seen.insert(cert.fingerprint()) {
            return Err(bad("each recipient certificate must occur only once"));
        }
        if !identifiers.insert((cert.issuer.raw.clone(), cert.serial.clone())) {
            return Err(bad("recipient certificates must have distinct issuer/serial identifiers"));
        }
        certificates.push(cert);
    }
    let entropy = Zeroizing::new(cx.entropy()?);
    let seed: [u8; 20] = entropy.get(..20).and_then(|b| b.try_into().ok()).ok_or_else(|| bad("could not create the encryption seed"))?;
    let seed = Zeroizing::new(seed);
    let file_entropy = cx.entropy()?;
    let file_id = file_entropy.get(..16).and_then(|b| b.try_into().ok()).ok_or_else(|| bad("could not create the file identity"))?;
    let at = Time::from_unix(cx.now.unwrap_or_else(|| chrono::Utc::now().timestamp()));
    let envelopes = certificates
        .iter()
        .zip(&protection.recipients)
        .map(|(cert, recipient)| envelope::seal(cert, &envelope::RecipientPayload::new(*seed, recipient.permissions), at))
        .collect::<Result<Vec<_>, _>>()?;
    let handler =
        SecurityHandler::create_public_key(protection.algorithm, envelopes, &seed, protection.encrypt_metadata).map_err(|e| bad(&e.to_string()))?;
    doc.set_public_key_encryption(handler, file_id).map_err(|e| bad(&e.to_string()))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn protect(_: &mut Document, _: &CertificateProtection, _: &mut crate::edit::EditCtx) -> Result<(), EditError> {
    Err(EditError::Protection("certificate encryption requires the desktop or native CLI".into()))
}

/// Resolve exactly one matching envelope before invoking a private-key operation. Malformed,
/// missing and ambiguous recipients fail without asking for a PIN; failures never retry.
#[cfg(not(target_arch = "wasm32"))]
pub fn authenticate(
    params: &pdfcraft_cos::EncryptDict,
    certificate: &crate::sign::Certificate,
    decrypt: impl FnOnce(&[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, crate::sign::SignError>,
) -> Result<pdfcraft_cos::SecurityHandler, pdfcraft_cos::CosError> {
    use crate::sign::envelope;
    let bad = |m: String| pdfcraft_cos::CosError::Security(m);
    pdfcraft_cos::SecurityHandler::validate_public_key(params).map_err(|e| bad(e.to_string()))?;
    let recipients = &params.public_key.as_ref().ok_or_else(|| bad("expected certificate security".into()))?.recipients;
    let mut selected = None;
    for bytes in recipients {
        if envelope::matches_recipient(bytes, certificate).map_err(|e| bad(e.to_string()))? {
            if selected.is_some() {
                return Err(bad("multiple envelopes match the selected certificate; no PIN was attempted".into()));
            }
            selected = Some(bytes);
        }
    }
    let bytes = selected.ok_or_else(|| bad("this PDF has no recipient matching the selected certificate".into()))?;
    let payload = envelope::open(bytes, certificate, decrypt).map_err(|e| bad(e.to_string()))?;
    pdfcraft_cos::SecurityHandler::open_public_key(params.clone(), payload.seed(), payload.permissions).map_err(|e| bad(e.to_string()))
}

/// An opening-only resolver. Neither the token identity nor its borrowed PIN enters a session.
#[cfg(target_os = "linux")]
pub struct TokenResolver<'a> {
    pub identity: &'a crate::sign::pkcs11::RsaDecryptionIdentity,
    pub pin: Option<&'a str>,
}
#[cfg(target_os = "linux")]
impl pdfcraft_cos::SecurityResolver for TokenResolver<'_> {
    fn authenticate(&self, params: &pdfcraft_cos::EncryptDict, _: &[u8]) -> Result<pdfcraft_cos::SecurityHandler, pdfcraft_cos::CosError> {
        authenticate(params, self.identity.certificate(), |key| self.identity.decrypt_key(key, self.pin))
    }
}
