//! Lists the Windows Personal store and, with `PDFCRAFT_WINSTORE_SIGN=<out.pdf>` and
//! `PDFCRAFT_WINSTORE_ID=<part of the subject>`, signs a one-page PDF with that identity and
//! validates it. Runs only with `PDFCRAFT_WINSTORE_TEST=1`: it uses the real store, and a
//! desktopID certificate asks for approval in the remoteID app.

#![cfg(target_os = "windows")]

use std::sync::Arc;

use pdfcraft_sign::{SignOptions, TrustStore, windows};

#[test]
fn windows_store_identities_sign() {
    if std::env::var("PDFCRAFT_WINSTORE_TEST").is_err() {
        return;
    }
    let listing = windows::list().expect("list");
    for id in &listing.ids {
        println!("usable: {} | {}", id.certificate.display_name(), windows::reference(&id.certificate));
    }
    for u in &listing.unusable {
        println!("unusable: {} | {}", u.subject, u.reason);
    }
    let (Ok(out), Ok(want)) = (std::env::var("PDFCRAFT_WINSTORE_SIGN"), std::env::var("PDFCRAFT_WINSTORE_ID")) else { return };
    let id = listing.ids.iter().find(|id| id.certificate.display_name().contains(&want)).expect("an identity matching PDFCRAFT_WINSTORE_ID");
    let id = windows::find(&windows::reference(&id.certificate)).expect("find");
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
    let doc = pdfcraft_cos::Document::open(Arc::new(pdf.to_vec())).expect("fixture");
    let started = std::time::Instant::now();
    let signed = pdfcraft_sign::sign(&doc, &id, &SignOptions { page: 0, ..Default::default() }).expect("sign");
    println!("signed in {:?}", started.elapsed());
    std::fs::write(&out, &signed).expect("write");
    let doc = pdfcraft_cos::Document::open(Arc::new(signed.clone())).expect("reload");
    for s in pdfcraft_sign::signatures(&doc, &signed, &TrustStore::default()) {
        println!("validation: {:?} | {:?} | chain {}", s.status, s.signer, s.chain.len());
    }
}
