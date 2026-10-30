//! Desktop certificate protection and explicit, ephemeral smartcard authentication.
#![cfg(not(target_arch = "wasm32"))]
use crate::support;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use pdfcraft_engine::{
    Edit,
    sign::{Certificate, DigestAlg, Name, PrivateKey, Time, der},
};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, certificate_ui::CardEntry};

fn pdf() -> Vec<u8> {
    let stream = "BT /F1 18 Tf 20 150 Td (Certificate fixture) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", stream.len(), stream),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, object).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Root 1 0 R /Size 6 >>\nstartxref\n{xref}\n%%EOF").as_bytes());
    bytes
}
fn certificate() -> Vec<u8> {
    let key = PrivateKey::generate_rsa(2048).unwrap();
    let cert = Certificate::self_signed(&Name::build("Test PDF recipient", "", "", "", ""), &key, Time::from_unix(1_600_000_000), 30, &[1]).unwrap();
    let fields = der::Tlv::parse_all(&cert.raw).unwrap().children().unwrap();
    let tbs = fields[0].children().unwrap();
    let mut values: Vec<Vec<u8>> = tbs.iter().filter(|v| v.tag != der::tag::ctx(3)).map(|v| v.raw.to_vec()).collect();
    let usage = der::seq(&[&der::oid("2.5.29.15"), &der::octets(&der::tlv(der::tag::BIT_STRING, &[5, 0x20]))]);
    values.push(der::explicit(3, &der::seq(&[&usage])));
    let tbs = der::seq(&values.iter().map(Vec::as_slice).collect::<Vec<_>>());
    der::seq(&[&tbs, fields[1].raw, &der::bit_string(&key.sign(DigestAlg::Sha256, &tbs).unwrap())])
}
fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("fixture.pdf", None, pdf()).unwrap();
        app
    });
    h.run_steps(6);
    h
}
fn field<'a>(h: &'a Harness<'static, PdfCraftApp>, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_label(label).last().unwrap_or_else(|| panic!("no {label}"))
}
fn type_in(h: &mut Harness<'static, PdfCraftApp>, label: &str, text: &str) {
    field(h, label).click();
    h.run_steps(1);
    field(h, label).type_text(text);
    h.run_steps(1);
}
fn encrypted(cert: Vec<u8>) -> Vec<u8> {
    let mut session = pdfcraft_engine::Session::new();
    let id = session.open("fixture.pdf", None, std::sync::Arc::new(pdf()), None).unwrap();
    session
        .apply(
            id,
            Edit::ProtectCertificates(pdfcraft_engine::certificate_security::CertificateProtection {
                recipients: vec![pdfcraft_engine::certificate_security::CertificateRecipient { certificate: cert, permissions: -1 }],
                algorithm: pdfcraft_engine::Algorithm::Aes256,
                encrypt_metadata: true,
            }),
        )
        .unwrap();
    session.save_bytes(id).unwrap().as_ref().clone()
}

#[test]
fn certificate_dialog_adds_a_recipient_applies_and_undoes() {
    let mut h = harness();
    h.state_mut().execute("protect.certificate");
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::ProtectCertificates));
    assert!(h.get_by_label("Apply").accesskit_node().is_disabled());
    let cert = certificate();
    let path = std::env::temp_dir().join(format!("pdfcraft-ui-recipient-{}.der", std::process::id()));
    std::fs::write(&path, cert).unwrap();
    type_in(&mut h, "Certificate file (.der, .cer)", path.to_str().unwrap());
    h.run_steps(1);
    h.get_by_label("Add file").click();
    h.run_steps(2);
    let _ = std::fs::remove_file(path);
    assert!(!h.get_by_label("Apply").accesskit_node().is_disabled());
    if let Ok(path) = std::env::var("PDFCRAFT_CERTIFICATE_DIALOG_SHOT") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_label("Full control (including security changes)").click();
    h.run_steps(2);
    assert!(h.get_by_label("Apply").accesskit_node().is_disabled(), "at least one recipient must retain security control");
    h.get_by_label("Full control (including security changes)").click();
    h.run_steps(3);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(
        h.state().dialog.is_none(),
        "dialog={:?}, notice={:?}, error={:?}",
        h.state().dialog,
        h.state().toast,
        h.state().certificate_protect.error
    );
    let id = h.state().views[0].id;
    assert!(h.state().session.get(id).unwrap().info.encrypted);
    h.state_mut().execute("edit.undo");
    h.run_steps(2);
    assert!(!h.state().session.get(id).unwrap().info.encrypted);
}

#[test]
fn certificate_open_prompt_cancel_keeps_unsaved_work_and_hides_pin() {
    let mut h = harness();
    let id = h.state().views[0].id;
    h.state_mut().apply_edit(Edit::SetInfo { key: "Title".into(), value: "Keep my edit".into() });
    let cert = certificate();
    let path = std::env::temp_dir().join(format!("pdfcraft-ui-protected-{}.pdf", std::process::id()));
    std::fs::write(&path, encrypted(cert.clone())).unwrap();
    h.state_mut().open_path(path.to_str().unwrap());
    assert!(support::step_until(&mut h, |h| h.state().certificate_prompt.is_some()));
    assert_eq!(h.state().certificate_prompt.as_ref().unwrap().pending.path.as_deref(), path.to_str());
    let prompt = h.state_mut().certificate_prompt.as_mut().unwrap();
    prompt.cards.entries = vec![CardEntry {
        reference: "pkcs11:unavailable".into(),
        name: "Test PDF recipient".into(),
        token_label: "Test card".into(),
        token_serial: "test".into(),
        certificate: cert,
    }];
    h.run_steps(2);
    h.get_by_label("Test PDF recipient · Test card").click();
    h.run_steps(1);
    type_in(&mut h, "Token PIN", "1234");
    h.run_steps(1);
    assert_eq!(h.state().certificate_prompt.as_ref().unwrap().pin.value(), Some("1234"));
    assert!(!format!("{:?}", h.state().certificate_prompt.as_ref().unwrap().pin).contains("1234"));
    if let Ok(path) = std::env::var("PDFCRAFT_CARD_PROMPT_SHOT") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().certificate_prompt.is_none());
    assert_eq!(h.state().views.len(), 1);
    assert!(h.state().session.get(id).unwrap().dirty);
    assert_eq!(h.state().session.get(id).unwrap().info.title.as_deref(), Some("Keep my edit"));
    let _ = std::fs::remove_file(path);
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires disposable SoftHSM, PDFCRAFT_TEST_TOKEN_REF, and PDFCRAFT_TEST_ENCRYPTED_PDF; test PIN is 1234"]
fn certificate_open_dialog_uses_softhsm_and_clears_pin_after_each_attempt() {
    assert!(std::env::var("PDFCRAFT_PKCS11_MODULE").unwrap().to_lowercase().contains("softhsm"));
    assert!(std::env::var("SOFTHSM2_CONF").is_ok());
    let reference = std::env::var("PDFCRAFT_TEST_TOKEN_REF").unwrap();
    let bytes = std::fs::read(std::env::var("PDFCRAFT_TEST_ENCRYPTED_PDF").unwrap()).unwrap();
    let mut h = harness();
    h.state_mut().open_bytes("card.pdf", None, bytes).unwrap();
    h.run_steps(2);
    h.get_by_label("Refresh cards").click();
    assert!(support::step_until(&mut h, |h| h.state().certificate_prompt.as_ref().is_some_and(|p| p
        .cards
        .entries
        .iter()
        .any(|e| e.reference == reference))));
    h.run_steps(3);
    let entry = h.state().certificate_prompt.as_ref().unwrap().cards.entries.iter().find(|e| e.reference == reference).unwrap();
    let label = format!("{} · {}", entry.name, entry.token_label);
    h.get_by_label(&label).click();
    h.run_steps(2);
    assert_eq!(h.state().certificate_prompt.as_ref().unwrap().cards.selected.as_deref(), Some(reference.as_str()));
    type_in(&mut h, "Token PIN", "wrong-test-pin");
    h.run_steps(1);
    h.get_by_label("Open with card").click();
    assert!(support::step_until(&mut h, |h| h.state().certificate_prompt.as_ref().is_some_and(|p| p.error.is_some())));
    h.run_steps(3);
    assert!(h.state().certificate_prompt.as_ref().unwrap().pin.value().is_none());
    assert_eq!(h.state().views.len(), 1);
    type_in(&mut h, "Token PIN", "1234");
    h.run_steps(1);
    h.get_by_label("Open with card").click();
    assert!(
        support::step_until(&mut h, |h| h.state().certificate_prompt.is_none()),
        "card error: {:?}",
        h.state().certificate_prompt.as_ref().and_then(|p| p.error.as_ref())
    );
    assert_eq!(h.state().views.len(), 2);
    let doc = h.state().session.get(h.state().views[1].id).unwrap();
    assert!(doc.info.encrypted && !doc.words().is_empty());
}
