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
