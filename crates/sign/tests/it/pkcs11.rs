//! PKCS #11 tokens and smart cards.
//!
//! The hardware tests are ignored by default and take the card's PIN from
//! `PDFCRAFT_PKCS11_PIN`; they never run with a PIN someone typed for something else, and never
//! retry a wrong one (a card locks itself after a few attempts). Without a card, the module is
//! still loaded and its failure reported rather than crashing.

use pdfcraft_sign::{Modification, Status, TrustStore, signatures};

use crate::pdf::{fixture, open, opts};

/// The modules the tests use: `PDFCRAFT_PKCS11_MODULES` when set, else the usual middleware.
fn modules() -> Vec<std::path::PathBuf> {
    let found = pdfcraft_sign::pkcs11::default_modules();
    assert!(!found.is_empty(), "no PKCS#11 module found; set PDFCRAFT_PKCS11_MODULES");
    found
}

fn pin() -> Option<zeroize::Zeroizing<String>> {
    std::env::var("PDFCRAFT_PKCS11_PIN").ok().filter(|p| !p.is_empty()).map(zeroize::Zeroizing::new)
}

/// A module path that doesn't exist is reported as a problem and nothing else: no panic, and no
/// quiet success that would leave the Sign dialog empty with no explanation.
#[test]
fn a_missing_module_is_reported_not_fatal() {
    let listing = pdfcraft_sign::pkcs11::identities(&["/nonexistent/pkcs11.so".into()], None);
    assert!(listing.identities.is_empty());
    assert_eq!(listing.problems.len(), 1);
    assert!(listing.problems[0].contains("pkcs11.so"), "{:?}", listing.problems);
    let found = pdfcraft_sign::pkcs11::find("pkcs11:00", &["/nonexistent/pkcs11.so".into()], None);
    assert!(found.is_err_and(|e| e.to_string().contains("no PKCS#11 identity")));
}

/// A real module loads, and its listing is well-formed. Needs a module but no card, so it runs on
/// a machine with the middleware installed.
#[test]
#[ignore = "loads the installed PKCS#11 middleware; run with --ignored"]
fn the_installed_modules_load_and_list_cleanly() {
    let listing = pdfcraft_sign::pkcs11::identities(&modules(), None);
    for id in &listing.identities {
        assert!(id.reference().starts_with("pkcs11:"));
        assert_eq!(id.reference(), pdfcraft_sign::pkcs11::reference(&id.certificate));
    }
    for unused in &listing.unusable {
        assert!(!unused.reason.is_empty());
        assert!(!unused.subject.is_empty());
    }
    // A machine with a card inserted has identities; one without has none. Neither is an error.
    eprintln!(
        "{} identities, {} unusable, {} problems: {:?}",
        listing.identities.len(),
        listing.unusable.len(),
        listing.problems.len(),
        listing.problems
    );
    for id in &listing.identities {
        eprintln!("  {} — {} (needs PIN: {})", id.reference(), id.certificate.display_name(), id.needs_pin());
    }
}

/// Sign a whole document with the card in the reader and validate the result. Ignored by
/// default: it needs a card, its middleware, and `PDFCRAFT_PKCS11_PIN`. On an Italian CNS it
/// signs with the first certificate the card offers that may sign documents (the "DS User
/// Certificate" one, not the authentication certificate, which key usage keeps out).
#[test]
#[ignore = "signs with the card in the reader; needs PDFCRAFT_PKCS11_PIN, run with --ignored"]
fn a_card_signs_and_the_signature_validates() {
    let modules = modules();
    // Running every ignored test without a card at hand skips this one, with a line saying so.
    let Some(pin) = pin() else {
        eprintln!("PDFCRAFT_PKCS11_PIN not set; skipping the card signature");
        return;
    };
    let listing = pdfcraft_sign::pkcs11::identities(&modules, Some(&pin));
    assert!(listing.problems.is_empty(), "{:?}", listing.problems);
    // The first certificate with a key worth signing with: an Italian CNS also carries an
    // RSA-1024 authentication certificate, which the card allows signing with but PdfCraft
    // reports as weak.
    let identities = listing.identities;
    let Some(identity) = identities.iter().find(|i| i.certificate.public_key.weakness().is_none()).cloned().or_else(|| identities.into_iter().next())
    else {
        // No card (or none that may sign): nothing to check, and no PIN was ever sent.
        eprintln!("no card identity; skipping");
        return;
    };
    // A card that needs its PIN refuses cleanly before anything is hashed or written.
    if identity.needs_pin() {
        assert!(identity.clone().digital_id(None).is_err_and(|e| e.to_string().contains("PIN")));
    }
    let certificate = identity.certificate.clone();
    let id = identity.digital_id(Some(pin)).expect("a signing key");
    assert!(id.key.is_external());
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).expect("the card signs the document");
    let validated = signatures(&open(&signed), &signed, &TrustStore { certs: vec![certificate.clone()] });
    let signature = validated.iter().find(|s| s.signed).expect("the signature is found");
    assert_eq!(signature.status, Status::Valid, "{:?}", signature.details);
    assert_eq!(signature.modification, Modification::None);
    // The signer is the card's certificate, not a substitute.
    assert_eq!(signature.certificate.as_ref().map(|c| c.raw.clone()), Some(certificate.raw));
}
