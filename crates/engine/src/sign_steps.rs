//! Signing in two steps, for a key that answers later (`sign.web-signing`). The browser's Web
//! Crypto signs asynchronously, so the web build asks it between the two steps; any signer outside
//! PdfCraft can answer the same way (the `sign_prepare` and `sign_finish` tools).
//!
//! [`Session::signing_request`] runs the signer with a key that only records what it is asked to
//! sign: the DER signed attributes of the CMS, which cover the document's digest.
//! [`Session::complete_signing`] checks the signature value it is handed against the certificate
//! and runs the signer again with a key that answers with that value, for exactly those bytes and
//! nothing else. What the bytes depend on is pinned between the two: the signing time is fixed in
//! the request, and an edit to the document in between refuses the second step. The file is the
//! one [`Session::sign`] writes with the same key (RSA PKCS #1 v1.5 and RFC 6979 ECDSA are
//! deterministic).

use std::sync::{Arc, Mutex};

use pdfcraft_sign::der::{self, Tlv, tag};
use pdfcraft_sign::keys::{ExternalKey, Scheme};
use pdfcraft_sign::{Certificate, DigestAlg, DigitalId, PrivateKey, PublicKey, SignError, SignOptions};

use crate::{DocId, EditError, Session};

/// What the key must sign for one signature, and what finishing it needs.
#[derive(Clone, Debug)]
pub struct SigningRequest {
    doc: DocId,
    generation: u64,
    opts: SignOptions,
    certificate: Certificate,
    chain: Vec<Certificate>,
    digest: DigestAlg,
    message: Vec<u8>,
}

impl SigningRequest {
    /// The document being signed.
    pub fn doc(&self) -> DocId {
        self.doc
    }

    /// The bytes to sign (the CMS signed attributes). The signer hashes them with
    /// [`digest`](Self::digest): RSA signs PKCS #1 v1.5, ECDSA over the curve the certificate names.
    pub fn message(&self) -> &[u8] {
        &self.message
    }

    /// The hash the signature uses: SHA-256, or SHA-384 for a P-384 key.
    pub fn digest(&self) -> DigestAlg {
        self.digest
    }

    /// The signer's certificate.
    pub fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    /// The signing time written into the signature (`/M`).
    pub fn date(&self) -> &str {
        &self.opts.date
    }
}

/// Records what it is asked to sign and answers with nothing (that file is thrown away).
#[derive(Default)]
struct Capture(Mutex<Option<(DigestAlg, Vec<u8>)>>);

impl ExternalKey for Capture {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let mut seen = self.0.lock().map_err(|_| SignError::Crypto("signing failed".into()))?;
        if seen.is_some() {
            return Err(SignError::Crypto("the signer asked for a second signature".into()));
        }
        *seen = Some((alg, msg.to_vec()));
        Ok(Vec::new())
    }
}

/// Answers with the signature value it was given, only for the bytes it was made for.
struct Replay {
    digest: DigestAlg,
    message: Vec<u8>,
    signature: Vec<u8>,
}

impl ExternalKey for Replay {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        if alg != self.digest || msg != self.message.as_slice() {
            return Err(SignError::Crypto("the bytes to sign changed between the two signing steps; sign the document again".into()));
        }
        Ok(self.signature.clone())
    }
}

/// The keys a signature can be made with: RSA, P-256 and P-384, as in one step.
fn signing_scheme(key: &PublicKey) -> Result<Scheme, EditError> {
    match key {
        PublicKey::Rsa { .. } => Ok(Scheme::RsaPkcs1),
        PublicKey::P256(_) | PublicKey::P384(_) => Ok(Scheme::Ecdsa),
        other => Err(EditError::Sign(format!("PdfCraft can't sign with a {} key (RSA, P-256 and P-384 only)", other.describe()))),
    }
}

/// An ECDSA signature as the DER `SEQUENCE { r, s }` a CMS holds: as given when it is DER, and
/// re-encoded when it is the raw `r ‖ s` Web Crypto, PKCS #11 tokens and HSMs return.
fn ecdsa_der(key: &PublicKey, sig: &[u8]) -> Vec<u8> {
    // By its structure: a raw value can start with the SEQUENCE tag by chance.
    let der = Tlv::parse_all(sig)
        .ok()
        .filter(|t| t.tag == tag::SEQUENCE)
        .and_then(|t| t.children().ok())
        .is_some_and(|parts| matches!(parts.as_slice(), [r, s] if r.tag == tag::INTEGER && s.tag == tag::INTEGER));
    if der {
        return sig.to_vec();
    }
    let field = match key {
        PublicKey::P256(_) => 32,
        PublicKey::P384(_) => 48,
        _ => return sig.to_vec(),
    };
    match (sig.len() == field * 2).then(|| sig.split_at(field)) {
        Some((r, s)) => der::seq(&[&der::uint(r), &der::uint(s)]),
        None => sig.to_vec(),
    }
}

impl Session {
    /// The first of two signing steps: what `certificate`'s key must sign to sign `doc` with
    /// `opts` (the signer's issuers in `chain`, as in a digital ID file). An empty `opts.date`
    /// takes the session clock, and the request keeps it for the second step.
    pub fn signing_request(
        &self,
        doc: DocId,
        certificate: Certificate,
        chain: Vec<Certificate>,
        mut opts: SignOptions,
    ) -> Result<SigningRequest, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        signing_scheme(&certificate.public_key)?;
        if opts.date.is_empty() {
            opts.date = self.signing_date();
        }
        let capture = Arc::new(Capture::default());
        let id = DigitalId { key: PrivateKey::external(certificate.public_key.clone(), capture.clone()), certificate, chain, friendly_name: None };
        pdfcraft_sign::sign(&editor.cos, &id, &opts)?;
        let asked = capture.0.lock().map_err(|_| EditError::Sign("signing failed".into()))?.take();
        let (digest, message) = asked.ok_or_else(|| EditError::Sign("the signer asked for no signature".into()))?;
        let DigitalId { certificate, chain, .. } = id;
        Ok(SigningRequest { doc, generation: d.edit_generation(), opts, certificate, chain, digest, message })
    }

    /// The second signing step: sign the request's document with `signature`, the value the key
    /// made over [`SigningRequest::message`]. Returns the signed file, as [`Session::sign`] does;
    /// the caller saves it and then calls [`Session::mark_signed`].
    ///
    /// Refused, writing nothing, when the document was edited or closed since the request, or
    /// when `signature` doesn't verify with the certificate's key (the wrong key, or another
    /// message). ECDSA may be DER or the raw `r ‖ s`; the file gets DER.
    pub fn complete_signing(&self, request: &SigningRequest, signature: &[u8]) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(request.doc).ok_or(EditError::NoDocument)?;
        if d.edit_generation() != request.generation {
            return Err(EditError::Sign("the document changed while it was being signed; sign it again".into()));
        }
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        let key = &request.certificate.public_key;
        let scheme = signing_scheme(key)?;
        let hash = request.digest.digest(&[&request.message]);
        if !key.verify(scheme, request.digest, &hash, signature).unwrap_or(false) {
            return Err(EditError::Sign("the signature doesn't match the certificate's key and the document's bytes to sign".into()));
        }
        let signature = if scheme == Scheme::Ecdsa { ecdsa_der(key, signature) } else { signature.to_vec() };
        let replay = Replay { digest: request.digest, message: request.message.clone(), signature };
        let id = DigitalId {
            key: PrivateKey::external(key.clone(), Arc::new(replay)),
            certificate: request.certificate.clone(),
            chain: request.chain.clone(),
            friendly_name: None,
        };
        Ok(Arc::new(pdfcraft_sign::sign(&editor.cos, &id, &request.opts)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edit, SignatureStatus};

    fn id(file: &str) -> DigitalId {
        let path = format!("{}/../sign/tests/data/{file}", env!("CARGO_MANIFEST_DIR"));
        pdfcraft_sign::pkcs12::open(&std::fs::read(path).unwrap(), "test").unwrap()
    }

    /// After the test certificates' start date (2026-10-02).
    fn session() -> (Session, DocId) {
        let mut s = Session::new().with_clock(|| 1_800_000_000);
        let doc = s.open("fixture.pdf", None, Arc::new(crate::tests::fixture(2)), None).unwrap();
        (s, doc)
    }

    fn visible() -> SignOptions {
        SignOptions { page: 1, rect: Some([20.0, 20.0, 180.0, 60.0]), reason: Some("Approved".into()), ..SignOptions::default() }
    }

    /// The raw `r ‖ s` of a DER ECDSA signature, as Web Crypto returns it.
    fn raw(der_sig: &[u8], field: usize) -> Vec<u8> {
        let parts = Tlv::parse_all(der_sig).unwrap().children().unwrap();
        parts.iter().flat_map(|i| std::iter::repeat_n(0u8, field - i.uint_bytes().len()).chain(i.uint_bytes().iter().copied())).collect()
    }

    #[test]
    fn two_rsa_steps_write_the_file_one_step_writes_and_it_validates() {
        let digital_id = id("rsa-aes.p12");
        let (mut s, doc) = session();
        let request = s.signing_request(doc, digital_id.certificate.clone(), digital_id.chain.clone(), visible()).unwrap();
        assert_eq!((request.doc(), request.digest(), request.date()), (doc, DigestAlg::Sha256, "D:20270115080000Z"));
        // What the browser's Web Crypto does with the key: RSASSA-PKCS1-v1_5 over the message.
        let signature = digital_id.key.sign(request.digest(), request.message()).unwrap();
        let signed = s.complete_signing(&request, &signature).unwrap();
        let one_step = s.sign(doc, &digital_id, SignOptions { date: request.date().into(), ..visible() }).unwrap();
        assert_eq!(signed, one_step, "the same file as signing in one step");
        s.mark_signed(doc, signed, None).unwrap();
        s.set_trusted_certificates(vec![digital_id.certificate.clone()]);
        let sig = &s.get(doc).unwrap().signatures[0];
        assert_eq!((sig.status, sig.page, sig.reason.as_deref()), (SignatureStatus::Valid, Some(1), Some("Approved")));
    }

    #[test]
    fn ecdsa_answers_as_der_or_raw_and_the_file_gets_der() {
        for (file, field, digest) in [("ec-p256.p12", 32, DigestAlg::Sha256), ("ec-p384.p12", 48, DigestAlg::Sha384)] {
            let digital_id = id(file);
            let (mut s, doc) = session();
            let request = s.signing_request(doc, digital_id.certificate.clone(), Vec::new(), SignOptions::default()).unwrap();
            assert_eq!(request.digest(), digest, "{file}");
            let der_sig = digital_id.key.sign(request.digest(), request.message()).unwrap();
            let from_der = s.complete_signing(&request, &der_sig).unwrap();
            let from_raw = s.complete_signing(&request, &raw(&der_sig, field)).unwrap();
            let one_step = s.sign(doc, &digital_id, SignOptions { date: request.date().into(), ..SignOptions::default() }).unwrap();
            assert_eq!((&from_der, &from_raw), (&one_step, &one_step), "{file}: deterministic ECDSA, DER in the file");
            s.mark_signed(doc, from_raw, None).unwrap();
            s.set_trusted_certificates(vec![digital_id.certificate.clone()]);
            assert_eq!(s.get(doc).unwrap().signatures[0].status, SignatureStatus::Valid, "{file}");
        }
    }

    #[test]
    fn an_edit_between_the_steps_refuses_the_second_and_writes_nothing() {
        let digital_id = id("rsa-aes.p12");
        let (mut s, doc) = session();
        let request = s.signing_request(doc, digital_id.certificate.clone(), Vec::new(), visible()).unwrap();
        let signature = digital_id.key.sign(request.digest(), request.message()).unwrap();
        s.apply(doc, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let err = s.complete_signing(&request, &signature).unwrap_err();
        assert!(err.to_string().contains("changed while it was being signed"), "{err}");
        assert!(s.get(doc).unwrap().signatures.is_empty());
        // A closed document has nothing to finish.
        s.close(doc);
        assert_eq!(s.complete_signing(&request, &signature).unwrap_err(), EditError::NoDocument);
    }

    #[test]
    fn a_signature_by_another_key_or_over_other_bytes_is_refused() {
        let digital_id = id("rsa-aes.p12");
        let (s, doc) = session();
        let request = s.signing_request(doc, digital_id.certificate.clone(), Vec::new(), visible()).unwrap();
        let mut other_bytes = request.message().to_vec();
        other_bytes.push(0);
        let other_key = id("chain.p12").key;
        for signature in [
            other_key.sign(request.digest(), request.message()).unwrap(),
            digital_id.key.sign(request.digest(), &other_bytes).unwrap(),
            Vec::new(),
            vec![0xFF; 4096],
        ] {
            let err = s.complete_signing(&request, &signature).unwrap_err();
            assert!(err.to_string().contains("doesn't match the certificate's key"), "{err}");
        }
    }

    #[test]
    fn the_answering_key_signs_only_the_bytes_it_was_given_for() {
        let replay = Replay { digest: DigestAlg::Sha256, message: b"signed attributes".to_vec(), signature: vec![7; 4] };
        assert_eq!(replay.sign(DigestAlg::Sha256, b"signed attributes").unwrap(), vec![7; 4]);
        assert!(replay.sign(DigestAlg::Sha256, b"other attributes").is_err());
        assert!(replay.sign(DigestAlg::Sha384, b"signed attributes").is_err());
        let capture = Capture::default();
        assert!(capture.sign(DigestAlg::Sha256, b"once").is_ok());
        assert!(capture.sign(DigestAlg::Sha256, b"twice").is_err(), "one signature per request");
    }

    #[test]
    fn a_raw_ecdsa_value_that_starts_like_der_is_still_re_encoded() {
        let key = id("ec-p256.p12").certificate.public_key;
        // r ‖ s whose first bytes read as a SEQUENCE header of the right length.
        let mut raw = vec![0x30, 62];
        raw.extend([0xAA; 62]);
        let (r, s) = raw.split_at(32);
        assert_eq!(ecdsa_der(&key, &raw), der::seq(&[&der::uint(r), &der::uint(s)]));
        let der_sig = der::seq(&[&der::uint(r), &der::uint(s)]);
        assert_eq!(ecdsa_der(&key, &der_sig), der_sig, "DER stays as it is");
    }

    #[test]
    fn a_key_pdfcraft_cant_sign_with_is_refused_before_anything_is_written() {
        let mut certificate = id("ec-p256.p12").certificate;
        certificate.public_key = PublicKey::Ed25519(vec![0; 32]);
        let (s, doc) = session();
        let err = s.signing_request(doc, certificate, Vec::new(), SignOptions::default()).unwrap_err();
        assert!(err.to_string().contains("can't sign with"), "{err}");
    }
}
