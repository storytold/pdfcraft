//! A certificate-protected document's file is encrypted; the session reads a decrypted rewrite of
//! it. A saved revision and the version a signature covers are byte ranges of the file itself, so
//! they open from that file, and a repeat finds the tab already open.

use std::sync::Arc;

use pdfcraft_engine::sign::DigitalId;
use pdfcraft_engine::{Algorithm, CertificateProtection, CertificateRecipient, Edit, Session};
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::canvas::SnapshotKind;

fn identity() -> DigitalId {
    let p12 = std::fs::read(format!("{}/../sign/tests/data/rsa-aes.p12", env!("CARGO_MANIFEST_DIR"))).unwrap();
    pdfcraft_engine::sign::pkcs12::open(&p12, "test").unwrap()
}

/// One 200 x 300 page showing "Page 1".
fn one_page() -> Vec<u8> {
    let body = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// [`one_page`] encrypted for [`identity`] with every permission, as Save writes it.
fn protected() -> Arc<Vec<u8>> {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("plain.pdf", None, Arc::new(one_page()), None).unwrap();
    let protection = CertificateProtection {
        recipients: vec![CertificateRecipient { certificate: identity().certificate.raw, permissions: -1 }],
        algorithm: Algorithm::Aes256,
        encrypt_metadata: true,
        seed: s.new_seed().unwrap(),
    };
    s.apply(id, Edit::ProtectWithCertificates(protection)).unwrap();
    s.save_bytes(id).unwrap()
}

/// The open document is certificate-protected, from its own file.
fn certificate_protected(app: &PdfCraftApp, view: usize) -> bool {
    app.session.get(app.views[view].id).unwrap().security_summary().is_some_and(|s| s.method.contains("certificates"))
}

#[test]
fn viewing_the_signed_version_of_a_certificate_document_opens_that_revision() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.session.add_identity(identity());
    let first = protected();
    app.open_bytes("secret.pdf", None, first.to_vec()).unwrap();
    assert_eq!(app.views.len(), 1, "the identity opens it");
    let id = app.views[0].id;
    // A second revision, appended by an incremental save.
    app.session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Second".into() }).unwrap();
    let saved = app.session.save_bytes(id).unwrap();
    app.session.mark_saved(id, saved.clone(), None).unwrap();
    assert!(saved.starts_with(first.as_slice()), "the update is appended");

    // The version a signature over the first revision covers: the file as it was then.
    app.active = Some(0);
    app.view_signed_version(first.len(), 1);
    assert_eq!(app.views.len(), 2, "{:?}", app.toast);
    assert!(certificate_protected(&app, 1), "opened from the encrypted file, not the decrypted copy");
    assert_eq!(app.session.get(app.views[1].id).unwrap().info.title, None, "as it was before the update");

    // Saved revision 1 opens from the file too.
    app.active = Some(0);
    app.open_revision(1);
    assert_eq!(app.views.len(), 3, "{:?}", app.toast);
    assert!(app.views[2].snapshot.as_ref().is_some_and(|s| s.kind == SnapshotKind::Revision && s.revision == 1));
    assert!(certificate_protected(&app, 2));
    assert_eq!(app.session.get(app.views[2].id).unwrap().info.title, None);

    // Repeats find the tabs already open instead of opening identical ones.
    app.active = Some(0);
    app.open_revision(1);
    assert_eq!((app.views.len(), app.active), (3, Some(2)), "the revision tab is reused");
    app.active = Some(0);
    app.view_signed_version(first.len(), 1);
    assert_eq!((app.views.len(), app.active), (3, Some(1)), "the signed-version tab is reused");
}
