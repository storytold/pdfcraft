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
#[ignore = "requires a disposable SoftHSM token with the documented test identity"]
fn lists_the_token_identity() {
    let Some(_) = bench() else { return };
    let ids = pkcs11::list().expect("the module lists identities");
    let id = ids.iter().find(|id| id.certificate.subject.common_name() == Some("Test Signer RSA")).expect("the imported identity is listed");
    assert_eq!(id.certificate.subject.common_name(), Some("Test Signer RSA"));
}

#[test]
#[ignore = "requires a disposable SoftHSM token with the documented test identity"]
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
#[ignore = "requires a disposable SoftHSM token with the documented test identity"]
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

/// Provision unique signing identities, including per-use authentication. Only disposable
/// test credentials are passed to the external fixture provisioning tools.
#[test]
#[ignore = "requires SoftHSM utilities, PDFCRAFT_PKCS11_HELPER, and a disposable token store"]
fn isolated_worker_signs_rsa_and_ecdsa_without_loading_the_provider_in_the_parent() {
    use pdfcraft_sign::{Certificate, Name, Time, pkcs12};
    use std::process::Command;
    let module = std::env::var("PDFCRAFT_PKCS11_MODULE").unwrap();
    let util = std::env::var("PDFCRAFT_SOFTHSM_UTIL").unwrap();
    let nonce = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let label = format!("worker-{}", &nonce[nonce.len().saturating_sub(20)..]);
    let dir = std::env::temp_dir().join(format!("pdfcraft-worker-test-{nonce}"));
    std::fs::create_dir_all(&dir).unwrap();
    let output = Command::new(util)
        .args(["--module", &module, "--init-token", "--free", "--label", &label, "--so-pin", "12345678", "--pin", "1234"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    for (index, fixture, scheme) in [(1, "rsa-aes.p12", Scheme::RsaPkcs1), (2, "ec-p256.p12", Scheme::Ecdsa), (3, "ec-p384.p12", Scheme::Ecdsa)] {
        let source = std::fs::read(format!("{}/tests/data/{fixture}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let id = pkcs12::open(&source, "test").unwrap();
        let name = Name::build(&format!("Worker {nonce} {index}"), "", "", "", "");
        let certificate =
            Certificate::self_signed(&name, &id.key, Time { year: 2026, month: 10, day: 1, hour: 0, minute: 0, second: 0 }, 5, &[index]).unwrap();
        let key = dir.join("key.der");
        let cert = dir.join("cert.der");
        std::fs::write(&key, id.key.pkcs8()).unwrap();
        std::fs::write(&cert, &certificate.raw).unwrap();
        for (path, kind) in [(&key, "privkey"), (&cert, "cert")] {
            let mut command = Command::new("pkcs11-tool");
            command.args([
                "--module",
                &module,
                "--token-label",
                &label,
                "--login",
                "--pin",
                "1234",
                "--write-object",
                path.to_str().unwrap(),
                "--type",
                kind,
                "--id",
                &format!("{index:02x}"),
            ]);
            if kind == "privkey" {
                command.args(["--usage-sign", "--always-auth"]);
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "import: {}", String::from_utf8_lossy(&output.stderr));
        }
        let reference = pkcs11::reference(&certificate);
        let remote = pkcs11::find_with_pin(&reference, Some("1234")).unwrap();
        let algorithm = remote.key.preferred_digest();
        let message = b"PDF CMS signed attributes through an isolated provider";
        let signature = remote.key.sign(algorithm, message).unwrap();
        assert!(certificate.public_key.verify(scheme, algorithm, &algorithm.digest(&[message]), &signature).unwrap());
        assert!(remote.key.sign(algorithm, message).is_err(), "the consumed PIN is not retained for another signature");
    }
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libsofthsm2.so"), "discovery and signing must never load the provider into the caller");
    std::fs::remove_dir_all(dir).unwrap();
}
