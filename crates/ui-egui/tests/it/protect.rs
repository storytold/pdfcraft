//! Protect Using Password in the real shell (egui_kittest).

use crate::support::AppHarness as _;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, FIXTURE.to_vec()).unwrap();
            app
        }
    });
    h.run_steps(4);
    h
}

/// Types into the masked field named `label` (its field row carries the same name).
fn type_into(h: &mut Harness<'static, PdfCraftApp>, label: &str, text: &str) {
    use egui::accesskit::Role;
    h.get_by(|n| n.label().as_deref() == Some(label) && matches!(n.role(), Role::PasswordInput | Role::TextInput)).click();
    h.run_steps(2);
    h.get_by(|n| n.label().as_deref() == Some(label) && matches!(n.role(), Role::PasswordInput | Role::TextInput)).type_text(text);
    h.run_steps(2);
}

fn open_password_pair(h: &mut Harness<'static, PdfCraftApp>, password: &str) {
    type_into(h, "Type Password", password);
    type_into(h, "Re-type Password", password);
}

#[test]
fn protecting_for_viewing_requires_matching_passwords_and_saves_encrypted() {
    let mut h = harness();
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    h.get_by_label("Viewing");
    type_into(&mut h, "Type Password", "Open-Sesame-42");
    type_into(&mut h, "Re-type Password", "Open-Sesame-4");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some(), "Apply is disabled while the passwords differ");
    type_into(&mut h, "Re-type Password", "2");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && !n.is_disabled()).is_some(), "Apply is enabled once they match");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.query_by_label("Apply").is_none(), "Apply closes the dialog");
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
    assert!(h.execute("protect.remove"));
    h.undo();
    h.undo();
    assert!(!h.state().session.get(id).unwrap().info.encrypted);
}

#[test]
fn editing_restrictions_via_advanced_options() {
    let mut h = harness();
    h.execute("protect.password");
    h.run_steps(2);
    h.get_by_label("Editing").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Advanced Options").click();
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
    use craft::host::{Picked, ScriptedFiles};
    use pdfcraft_engine::sign::pkcs12;
    let me = sign_data("rsa-aes.p12");
    let ada = pkcs12::open(&std::fs::read(sign_data("chain.p12")).unwrap(), "test").unwrap();
    // Ada's certificate, as a file the picker hands over.
    let ada_cer = std::env::temp_dir().join(format!("pdfcraft-ada-{}.cer", std::process::id()));
    std::fs::write(&ada_cer, &ada.certificate.raw).unwrap();
    let files = ScriptedFiles::answering([Picked::Paths(vec![ada_cer.clone()])]);
    let host = crate::support::host_with_files(files);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::test_app(host);
        app.set_option(&cc.egui_ctx, "language", "en").unwrap();
        app.open_bytes(&cc.egui_ctx, "doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    // The person's digital ID file is listed (as Sign with a Digital ID lists it).
    let mine = pkcs12::open(&std::fs::read(&me).unwrap(), "test").unwrap();
    h.state_mut().add_digital_id(&me, &mine.certificate);
    assert!(h.execute("protect.certificate"));
    h.run_steps(2);
    assert!(h.query_all_by_label("Encrypt with Certificate").next().is_some());
    h.get_by_label("No recipients yet.");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some(), "Apply waits for a recipient");
    // A wrong password adds nobody and says why.
    type_into(&mut h, "Password", "nope");
    h.get_by_label("Add").click();
    h.run_steps(3);
    h.get_by_label("The password is incorrect.");
    h.get_by_label("No recipients yet.");
    type_into(&mut h, "Password", "test");
    h.get_by_label("Add").click();
    h.run_steps(3);
    h.get_by_label("Test Signer RSA (you)");
    // Another recipient, from a certificate file, with the default restrictions (no changes).
    h.get_by_label("Add certificate file…").click();
    h.run_steps(4);
    h.get_by_label("Ada Lovelace");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.query_all_by_label("Encrypt with Certificate").next().is_none(), "the dialog closes");
    let _ = std::fs::remove_file(&ada_cer);
    let id = h.state().views[0].id;
    let summary = h.state().session.get(id).unwrap().security_summary().unwrap();
    assert!(summary.pending && summary.method.contains("certificates (2 recipients)"), "{summary:?}");
    let bytes = h.state().session.save_bytes(id).unwrap();

    // Opening it: the prompt asks for a digital ID; one that is not a recipient doesn't open it.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        crate::support::inline_app()
    });
    h.set_option("language", "en").unwrap();
    let ctx = h.ctx.clone();
    h.state_mut().open_bytes(&ctx, "secret.pdf", None, bytes.to_vec()).unwrap();
    h.run_steps(2);
    assert!(h.state().views.is_empty());
    let prompt = h.state_mut().password_prompt.as_mut().expect("a digital ID is asked for");
    assert!(prompt.digital_id.is_some());
    assert!(
        prompt.error.as_deref().is_some_and(|e| e.contains("no digital ID") && e.contains("serial")),
        "no ID yet, and the recipients are named: {:?}",
        prompt.error
    );
    assert!(h.query_all_by_label("Digital ID required").next().is_some());
    prompt_with(&mut h, &sign_data("ec-p256.p12"));
    assert!(
        crate::support::step_until(&mut h, |h| h
            .state()
            .password_prompt
            .as_ref()
            .is_some_and(|p| p.error.as_deref().is_some_and(|e| e.contains("none of your digital IDs")))),
        "asked again: {:?}",
        h.state().password_prompt.as_ref().map(|p| p.error.clone())
    );
    prompt_with(&mut h, &sign_data("chain.p12"));
    assert!(crate::support::step_until(&mut h, |h| h.state().password_prompt.is_none() && h.state().views.len() == 1), "Ada opens it");
    let opened = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(opened.permissions().is_some_and(|p| !p.modify()), "with her restrictions");
    // The author opens it with everything.
    let mut s = pdfcraft_engine::Session::new();
    assert!(s.open("p.pdf", None, bytes.clone(), None).is_err());
    s.set_identities(vec![mine]);
    let mine_open = s.open("p.pdf", None, bytes, None).unwrap();
    assert!(s.get(mine_open).unwrap().permissions().is_none_or(|p| p.print() && p.modify()));
}

/// Answers the digital ID prompt with the file `id` and the password "test".
fn prompt_with(h: &mut Harness<'static, PdfCraftApp>, id: &str) {
    let ctx = h.ctx.clone();
    if let Some(prompt) = h.state_mut().password_prompt.as_mut() {
        prompt.digital_id = Some(id.to_string());
    }
    h.state_mut().submit_password(&ctx, Some("test".into()));
    h.run_steps(2);
}

/// Advanced Options sets an open password and a permissions password together, as the guide's
/// Password Security settings do ("setting both types of passwords is often beneficial"): the
/// saved file needs the open password to open, opens restricted with it, and the permissions
/// password lifts the restrictions.
#[test]
fn an_open_password_and_a_permissions_password_together() {
    let mut h = harness();
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    h.get_by_label("Viewing");
    open_password_pair(&mut h, "Open-Sesame-42");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Advanced Options").click();
    h.run_steps(2);
    assert!(h.query_by_label("Printing allowed").is_none(), "no restrictions without a permissions password");
    h.get_by_label("Restrict editing and printing of the document").click();
    h.run_steps(4);
    h.get_by_label("Printing allowed");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some());
    type_into(&mut h, "Change Permissions Password", "Boss-Key-7");
    type_into(&mut h, "Re-type Permissions Password", "Boss-Key-");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some());
    type_into(&mut h, "Re-type Permissions Password", "7");
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.query_by_label("Apply").is_none());
    let id = h.state().views[0].id;
    let bytes = h.state().session.save_bytes(id).unwrap();
    let auth = |pw: Option<&str>| pdfcraft_cos::Document::open_with_password(bytes.clone(), pw).ok().and_then(|d| d.security().map(|s| s.auth()));
    assert_eq!(auth(None), None, "the open password is needed");
    assert_eq!(auth(Some("Open-Sesame-42")), Some(pdfcraft_cos::Auth::User));
    assert_eq!(auth(Some("Boss-Key-7")), Some(pdfcraft_cos::Auth::Owner));
    let user = pdfcraft_cos::Document::open_with_password(bytes.clone(), Some("Open-Sesame-42")).unwrap();
    let p = user.permissions().unwrap();
    assert!(p.print() && !p.modify() && !p.copy(), "the defaults restrict changes and copying: {:#x}", p.bits);
}

/// Setting an editing password can also require an open password via Advanced Options (#7401).
/// This prevents the open password from defaulting to blank, blocking unauthorized tools from
/// decrypting without the open password.
#[test]
fn editing_protection_can_require_an_open_password_too() {
    let mut h = harness();
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    h.get_by_label("Editing").click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Advanced Options").click();
    h.run_steps(2);
    // Editing restrictions are enabled.
    h.get_by_label("Printing allowed");
    h.get_by_label("Require a password to open the document").click();
    h.run_steps(2);
    type_into(&mut h, "Type Password", "Boss-Key-7");
    type_into(&mut h, "Re-type Password", "Boss-Key-7");
    type_into(&mut h, "Document Open Password", "Open-Sesame-42");
    type_into(&mut h, "Re-type Document Open Password", "Open-Sesame-42");
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.query_by_label("Apply").is_none());
    let id = h.state().views[0].id;
    let bytes = h.state().session.save_bytes(id).unwrap();
    let auth = |pw: Option<&str>| pdfcraft_cos::Document::open_with_password(bytes.clone(), pw).ok().and_then(|d| d.security().map(|s| s.auth()));
    assert_eq!(auth(None), None, "the open password is required and not blank");
    assert_eq!(auth(Some("Open-Sesame-42")), Some(pdfcraft_cos::Auth::User));
    assert_eq!(auth(Some("Boss-Key-7")), Some(pdfcraft_cos::Auth::Owner));
}

/// A password that reduces to empty under SASLprep (e.g. soft hyphens U+00AD) is rejected by
/// validation with Apply disabled, for both Viewing and Editing roles.
#[test]
fn effective_empty_password_is_rejected_for_both_viewing_and_editing() {
    // 1. Viewing role
    let mut h = harness();
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    h.get_by_label("Viewing");
    type_into(&mut h, "Type Password", "\u{00AD}");
    type_into(&mut h, "Re-type Password", "\u{00AD}");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some(), "Apply is disabled for Viewing role");

    // 2. Editing role
    h.get_by_label("Editing").click();
    h.run_steps(2);
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some(), "Apply is disabled for Editing role");
}
