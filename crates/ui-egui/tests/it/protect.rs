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

/// Advanced Options lists the guide's five compatibility levels, each with the PDF version it
/// needs, and offers "All document contents except metadata" from Acrobat 6.0 on (Acrobat 9 guide:
/// "This option is available when you select a compatibility to Acrobat 6.0 and later"): the
/// levels whose encryption has crypt filters, where ISO 32000-2 Table 21 `EncryptMetadata` means
/// something. Acrobat 6.0 writes 128-bit RC4 through a crypt filter (`/V 4`, `/CFM /V2`).
#[test]
fn acrobat_6_and_later_can_leave_the_metadata_unencrypted() {
    use egui::accesskit::Role;

    let mut h = harness();
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Advanced Options").click();
    h.run_steps(2);
    let open = |h: &mut Harness<'static, PdfCraftApp>, caption: &str| {
        h.get_by_role_and_label(Role::ComboBox, caption).click();
        h.run_steps(2);
    };
    let item = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.query_all_by(|n| n.label().as_deref() == Some(label) && n.role() != Role::ComboBox).next().map(|n| n.rect())
    };
    let pick = |h: &mut Harness<'static, PdfCraftApp>, caption: &str, to: &str| {
        h.get_by_role_and_label(Role::ComboBox, caption).click();
        h.run_steps(2);
        h.get_by(|n| n.label().as_deref() == Some(to) && n.role() != Role::ComboBox).click_accesskit();
        h.run_steps(2);
    };
    const X: &str = "Acrobat X and later (PDF 1.7, 256-bit AES)";
    const A6: &str = "Acrobat 6.0 and later (PDF 1.5, 128-bit RC4)";
    const EXCEPT: &str = "All document contents except metadata";
    // Whether the Encrypt list offers EXCEPT.
    let offered = |h: &mut Harness<'static, PdfCraftApp>| {
        h.get_by_role_and_label(Role::ComboBox, "Encrypt").click();
        h.run_steps(2);
        let on = h.query_all_by(|n| n.label().as_deref() == Some(EXCEPT) && n.role() != Role::ComboBox && !n.is_disabled()).next().is_some();
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        on
    };
    // The list, in the guide's order.
    let levels = [
        X,
        "Acrobat 7.0 and later (PDF 1.6, 128-bit AES)",
        A6,
        "Acrobat 5.0 and later (PDF 1.4, 128-bit RC4)",
        "Acrobat 3.0 and later (PDF 1.3, 40-bit RC4)",
    ];
    open(&mut h, "Compatibility");
    let tops: Vec<f32> = levels.iter().map(|l| item(&h, l).expect(l).top()).collect();
    assert!(tops.windows(2).all(|w| w[0] < w[1]), "in the guide's order: {tops:?}");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(offered(&mut h), "Acrobat X");
    for (level, wanted) in levels.iter().zip([true, true, true, false, false]).skip(1) {
        pick(&mut h, "Compatibility", level);
        assert_eq!(offered(&mut h), wanted, "{level}");
    }
    pick(&mut h, "Compatibility", A6);
    pick(&mut h, "Encrypt", EXCEPT);
    type_into(&mut h, "Type Password", "Open-Sesame-42");
    type_into(&mut h, "Re-type Password", "Open-Sesame-42");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    let id = h.state().views[0].id;
    let bytes = h.state().session.save_bytes(id).unwrap();
    let raw = String::from_utf8_lossy(&bytes);
    for entry in ["/V 4", "/R 4", "/CFM /V2", "/EncryptMetadata false"] {
        assert!(raw.contains(entry), "{entry} missing");
    }
    // The fixture is PDF 1.7 already: the version is never lowered.
    assert!(raw.starts_with("%PDF-1.7"));
    let mut s = pdfcraft_engine::Session::new();
    assert!(s.open("p.pdf", None, bytes, Some("Open-Sesame-42")).is_ok());
}
