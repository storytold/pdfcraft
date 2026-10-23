//! Lists the PKCS #11 device identities on this machine and, with
//! `PDFCRAFT_PKCS11_SIGN=<out.pdf>`, signs a one-page PDF with the first one and validates it.
//! Runs only with `PDFCRAFT_PKCS11_TEST=1`: it loads the real modules (desktopID may ask for
//! approval on the phone).

#![cfg(not(target_arch = "wasm32"))]

use pdfcraft_sign::{SignOptions, TrustStore, pkcs11};

#[test]
fn token_identities_sign() {
    if std::env::var("PDFCRAFT_PKCS11_TEST").is_err() {
        return;
    }
    let ids = pkcs11::identities(&[]);
    for id in &ids {
        println!("{} | {} | {} | pin: {}", id.reference, id.token, id.certificate.display_name(), id.needs_pin);
    }
    let Ok(out) = std::env::var("PDFCRAFT_PKCS11_SIGN") else { return };
    let first = ids.first().expect("a token identity");
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(pdf.to_vec())).expect("fixture");
    let id = pkcs11::find(&first.reference, None, &[]).expect("find");
    let t = std::time::Instant::now();
    let signed = pdfcraft_sign::sign(&doc, &id, &SignOptions { page: 0, ..Default::default() }).expect("sign");
    println!("signed in {:?}", t.elapsed());
    std::fs::write(&out, &signed).expect("write");
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(signed.clone())).expect("reload");
    for s in pdfcraft_sign::signatures(&doc, &signed, &TrustStore::default()) {
        println!("validation: {:?} | {:?}", s.status, s.signer);
    }
}
