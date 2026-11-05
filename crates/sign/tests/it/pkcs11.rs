//! The Linux PKCS #11 module (`src/pkcs11.rs`) against a real module. The test bench is
//! SoftHSM 2 with the RSA key and certificate of `tests/data/rsa-aes.p12` imported under
//! `CKA_ID AABB` and the user PIN `1234` (the token is named `test`); a real Rutoken or
//! JaCarta works the same way. The tests are skipped unless `PDFCRAFT_PKCS11_MODULE` names
//! a module to read (the unit tests stay green on every machine and in CI).

use pdfcraft_sign::keys::{DigestAlg, Scheme};
use pdfcraft_sign::pkcs11;

/// `PDFCRAFT_PKCS11_MODULE`, or `None` (after a skip note) when the bench isn't set up.
fn bench() -> Option<String> {
    match std::env::var("PDFCRAFT_PKCS11_MODULE") {
        Ok(m) if !m.is_empty() => Some(m),
        _ => {
            eprintln!("skipped: PDFCRAFT_PKCS11_MODULE is not set (no PKCS #11 test bench)");
            None
        }
    }
}

#[test]
fn lists_the_token_identity() {
    let Some(_) = bench() else { return };
    let ids = pkcs11::list().expect("the module lists identities");
    let id = ids.iter().find(|id| id.certificate.subject.common_name() == Some("Test Signer RSA")).expect("the imported identity is listed");
    assert_eq!(id.certificate.subject.common_name(), Some("Test Signer RSA"));
}

#[test]
fn the_token_signs_and_the_certificate_verifies() {
    let Some(_) = bench() else { return };
    let ids = pkcs11::list().expect("the module lists identities");
    let id = ids.iter().find(|id| id.certificate.subject.common_name() == Some("Test Signer RSA")).expect("the imported identity is listed");
    let with_pin = pkcs11::find_with_pin(&pkcs11::reference(&id.certificate), Some("1234")).expect("signs in with the PIN");
    let msg = b"a message for the token to sign";
    let sig = with_pin.key.sign(DigestAlg::Sha256, msg).expect("the token signs");
    let digest = DigestAlg::Sha256.digest(&[msg]);
    let ok = with_pin.certificate.public_key.verify(Scheme::RsaPkcs1, DigestAlg::Sha256, &digest, &sig).expect("verifies");
    assert!(ok, "the token's signature matches the certificate's public key");
}

#[test]
fn find_by_reference_and_common_name() {
    let Some(_) = bench() else { return };
    let ids = pkcs11::list().expect("the module lists identities");
    let id = ids.iter().find(|id| id.certificate.subject.common_name() == Some("Test Signer RSA")).expect("the imported identity is listed");
    let reference = pkcs11::reference(&id.certificate);
    assert!(reference.starts_with("pkcs11:"));
    assert!(pkcs11::find(&reference).is_ok(), "found by its pkcs11: reference");
    assert!(pkcs11::find("Test Signer RSA").is_ok(), "found by common name");
    assert!(pkcs11::find("pkcs11:no-such-identity").is_err(), "an unknown reference is refused, not a panic");
}
