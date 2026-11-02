//! Protect Using Password in the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn type_into(h: &mut Harness<'static, PdfCraftApp>, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.run_steps(2);
    h.get_by_label(label).type_text(text);
    h.run_steps(2);
}

#[test]
fn protecting_for_viewing_requires_matching_passwords_and_saves_encrypted() {
    let mut h = harness();
    assert!(h.state_mut().execute("protect.password"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Protect));
    h.get_by_label("Viewing");
    type_into(&mut h, "Type Password", "Open-Sesame-42");
    type_into(&mut h, "Re-type Password", "Open-Sesame-4");
    h.get_by_label_contains("don't match");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some(), "Apply is disabled");
    type_into(&mut h, "Re-type Password", "2");
    h.get_by_label("Strength: Strong");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    let id = h.state().views[0].id;
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.info.encrypted && doc.dirty);
    assert!(doc.security_summary().unwrap().pending);
    // What Save writes needs the password.
    let bytes = h.state().session.save_bytes(id).unwrap();
    let mut s = pdfcraft_engine::Session::new();
    assert!(s.open("p.pdf", None, bytes.clone(), None).is_err());
    assert!(s.open("p.pdf", None, bytes, Some("Open-Sesame-42")).is_ok());
    // Remove security is now offered, and undo also takes the protection back.
    assert!(h.state_mut().execute("protect.remove"));
    h.state_mut().undo();
    h.state_mut().undo();
    assert!(!h.state().session.get(id).unwrap().info.encrypted);
}

#[test]
fn editing_restrictions_via_advanced_options() {
    let mut h = harness();
    h.state_mut().execute("protect.password");
    h.run_steps(2);
    h.get_by_label("Editing").click();
    h.run_steps(2);
    h.get_by_label("Advanced Options ⌄").click();
    h.run_steps(2);
    h.get_by_label("Enable copying of text, images, and other content").click();
    h.run_steps(1);
    type_into(&mut h, "Type Password", "boss");
    type_into(&mut h, "Re-type Password", "boss");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    let id = h.state().views[0].id;
    let sec = h.state().session.get(id).unwrap().security_summary().unwrap();
    assert!(sec.permissions.copy() && sec.permissions.print() && !sec.permissions.modify() && !sec.permissions.annotate());
    // Remove security is disabled for nobody here: this session set it.
    assert!(h.state().session.get(id).unwrap().allows_security_change());
}

fn sign_data(name: &str) -> String {
    format!("{}/../sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn encrypting_with_certificates_and_opening_with_a_digital_id() {
    use pdfcraft_engine::sign::pkcs12;
    let me = sign_data("rsa-aes.p12");
    let ada = pkcs12::open(&std::fs::read(sign_data("chain.p12")).unwrap(), "test").unwrap();
    let mut h = harness();
    // The person's digital ID file is listed (as Sign with a Digital ID lists it).
    let mine = pkcs12::open(&std::fs::read(&me).unwrap(), "test").unwrap();
    h.state_mut().add_digital_id(&me, &mine.certificate);
    assert!(h.state_mut().execute("protect.certificate"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::EncryptCertificate));
    h.get_by_label("No recipients yet.");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some(), "Apply waits for a recipient");
    // A wrong password adds nobody and says why.
    h.state_mut().certificate_draft.own_password = "nope".into();
    h.get_by_label("Add").click();
    h.run_steps(2);
    h.get_by_label("The password is incorrect.");
    assert!(h.state().certificate_draft.recipients.is_empty());
    h.state_mut().certificate_draft.own_password = "test".into();
    h.get_by_label("Add").click();
    h.run_steps(2);
    h.get_by_label("Test Signer RSA (you)");
    // Another recipient, as a certificate file gives it, with the restrictions below.
    h.state_mut().certificate_draft.add(pdfcraft_ui_egui::Recipient::of(&ada.certificate, false));
    h.state_mut().certificate_draft.protection.printing = pdfcraft_engine::Printing::None;
    h.run_steps(2);
    h.get_by_label("Ada Lovelace");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    let id = h.state().views[0].id;
    let summary = h.state().session.get(id).unwrap().security_summary().unwrap();
    assert!(summary.pending && summary.method.contains("certificates (2 recipients)"), "{summary:?}");
    let bytes = h.state().session.save_bytes(id).unwrap();

    // Opening it: the prompt asks for a digital ID; one that is not a recipient doesn't open it.
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("secret.pdf", None, bytes.to_vec()).unwrap();
    assert!(app.views.is_empty());
    let prompt = app.password_prompt.as_mut().expect("a digital ID is asked for");
    assert!(prompt.digital_id.is_some() && prompt.error.is_none());
    prompt.digital_id = Some(sign_data("ec-p256.p12"));
    app.submit_password(Some("test".into()));
    let prompt = app.password_prompt.as_mut().expect("asked again");
    assert_eq!(prompt.error.as_deref(), Some("None of your digital IDs can open this document."));
    prompt.digital_id = Some(sign_data("chain.p12"));
    app.submit_password(Some("test".into()));
    assert!(app.password_prompt.is_none() && app.views.len() == 1, "Ada opens it");
    let opened = app.session.get(app.views[0].id).unwrap();
    assert!(opened.permissions().is_some_and(|p| !p.print()), "with her restrictions");
    // The author opens it with everything.
    let mut s = pdfcraft_engine::Session::new();
    assert!(s.open("p.pdf", None, bytes.clone(), None).is_err());
    s.set_identities(vec![mine]);
    let mine_open = s.open("p.pdf", None, bytes, None).unwrap();
    assert!(s.get(mine_open).unwrap().permissions().is_none_or(|p| p.print() && p.modify()));
}
