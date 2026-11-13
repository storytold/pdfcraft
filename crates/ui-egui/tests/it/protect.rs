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

/// Advanced Options lists the guide's five compatibility levels, each with the PDF version it
/// needs, and offers "All document contents except metadata" from Acrobat 6.0 on (Acrobat 9 guide:
/// "This option is available when you select a compatibility to Acrobat 6.0 and later"): the
/// levels whose encryption has crypt filters, where ISO 32000-2 Table 21 `EncryptMetadata` means
/// something. Acrobat 6.0 writes 128-bit RC4 through a crypt filter (`/V 4`, `/CFM /V2`).
#[test]
fn acrobat_6_and_later_can_leave_the_metadata_unencrypted() {
    use egui::accesskit::Role;

    let mut h = harness();
    h.state_mut().execute("protect.password");
    h.run_steps(2);
    h.get_by_label("Advanced Options ⌄").click();
    h.run_steps(2);
    let pick = |h: &mut Harness<'static, PdfCraftApp>, from: &str, to: &str| {
        h.query_all_by_role(Role::ComboBox).find(|n| n.value().as_deref() == Some(from)).expect(from).click();
        h.run_steps(2);
        h.get_by_label(to).click();
        h.run_steps(2);
    };
    const X: &str = "Acrobat X and later (PDF 1.7, 256-bit AES)";
    const A6: &str = "Acrobat 6.0 and later (PDF 1.5, 128-bit RC4)";
    const EXCEPT: &str = "All document contents except metadata";
    let disabled = |h: &Harness<'static, PdfCraftApp>| h.query_by(|n| n.label().as_deref() == Some(EXCEPT) && n.is_disabled()).is_some();
    // The list, in the guide's order.
    h.query_all_by_role(Role::ComboBox).find(|n| n.value().as_deref() == Some(X)).expect(X).click();
    h.run_steps(2);
    let levels = [
        X,
        "Acrobat 7.0 and later (PDF 1.6, 128-bit AES)",
        A6,
        "Acrobat 5.0 and later (PDF 1.4, 128-bit RC4)",
        "Acrobat 3.0 and later (PDF 1.3, 40-bit RC4)",
    ];
    let tops: Vec<f32> = levels.iter().map(|l| h.get_by_label(l).rect().top()).collect();
    assert!(tops.windows(2).all(|w| w[0] < w[1]), "in the guide's order: {tops:?}");
    h.get_by_label(X).click();
    h.run_steps(2);
    assert!(!disabled(&h), "Acrobat X");
    for (level, offered) in levels.iter().zip([true, true, true, false, false]).skip(1) {
        pick(&mut h, X, level);
        assert_eq!(!disabled(&h), offered, "{level}");
        pick(&mut h, level, X);
    }
    pick(&mut h, X, A6);
    h.get_by_label(EXCEPT).click();
    h.run_steps(2);
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
