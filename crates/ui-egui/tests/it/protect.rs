//! Protect Using Password in the real shell (egui_kittest).

use crate::support::AppHarness as _;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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

fn open_password_pair(h: &mut Harness<'static, PdfCraftApp>, password: &str) {
    type_into(h, "Type Password", password);
    type_into(h, "Re-type Password", password);
}

fn advanced_options(h: &mut Harness<'static, PdfCraftApp>) {
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Advanced Options").click();
    h.run_steps(2);
}

fn apply_enabled(h: &Harness<'static, PdfCraftApp>) -> bool {
    h.query_by(|n| n.label().as_deref() == Some("Apply") && !n.is_disabled()).is_some()
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
    advanced_options(&mut h);
    assert!(h.query_by_label("Printing allowed").is_none(), "no restrictions without a permissions password");
    h.get_by_label("Restrict editing and printing of the document").click();
    h.run_steps(2);
    h.get_by_label("Printing allowed");
    open_password_pair(&mut h, "Open-Sesame-42");
    h.get_by_label_contains("Type a permissions password");
    assert!(!apply_enabled(&h));
    type_into(&mut h, "Change Permissions Password", "Boss-Key-7");
    type_into(&mut h, "Re-type Permissions Password", "Boss-Key");
    h.get_by_label_contains("permissions passwords don't match");
    assert!(!apply_enabled(&h));
    type_into(&mut h, "Re-type Permissions Password", "-7");
    assert!(apply_enabled(&h), "Apply waits for a matching second password");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.query_by_label("Apply").is_none(), "Apply closes the dialog");
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

/// With 40-bit RC4 the options the guide says need 128-bit encryption are unavailable:
/// low-resolution printing, inserting/deleting/rotating pages, filling in forms and signing, and
/// screen-reader access (which follows copying there). A choice made before is put back to the
/// stricter "None".
#[test]
fn forty_bit_rc4_offers_only_what_it_can_keep() {
    use egui::accesskit::{Role, Toggled};

    let mut h = harness();
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    h.get_by_label("Editing").click();
    h.run_steps(2);
    advanced_options(&mut h);
    let open = |h: &mut Harness<'static, PdfCraftApp>, caption: &str| {
        h.get_by_role_and_label(Role::ComboBox, caption).click();
        h.run_steps(2);
    };
    let close = |h: &mut Harness<'static, PdfCraftApp>| {
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
    };
    let pick = |h: &mut Harness<'static, PdfCraftApp>, caption: &str, item: &str| {
        h.get_by_role_and_label(Role::ComboBox, caption).click();
        h.run_steps(2);
        h.get_by(|n| n.label().as_deref() == Some(item) && n.role() != Role::ComboBox).click_accesskit();
        h.run_steps(2);
    };
    let disabled = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.query_all_by(|n| n.label().as_deref() == Some(label) && n.role() != Role::ComboBox && n.is_disabled()).next().is_some()
    };
    let value = |h: &Harness<'static, PdfCraftApp>, caption: &str| h.get_by_role_and_label(Role::ComboBox, caption).value();
    let screen_readers = "Enable text access for screen reader devices for the visually impaired";
    let ticked = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.get_by(|n| n.label().as_deref() == Some(label) && n.role() == Role::CheckBox).accesskit_node().toggled() == Some(Toggled::True)
    };
    // 256-bit AES: every choice is available; pick the ones 40-bit RC4 can't keep.
    open(&mut h, "Printing allowed");
    assert!(!disabled(&h, "Low Resolution (150 dpi)"));
    close(&mut h);
    pick(&mut h, "Printing allowed", "Low Resolution (150 dpi)");
    open(&mut h, "Changes allowed");
    assert!(!disabled(&h, "Inserting, deleting, and rotating pages"));
    close(&mut h);
    pick(&mut h, "Changes allowed", "Filling in form fields and signing existing signature fields");
    assert!(!disabled(&h, screen_readers));
    assert_eq!(value(&h, "Printing allowed").as_deref(), Some("Low Resolution (150 dpi)"));
    assert_eq!(value(&h, "Changes allowed").as_deref(), Some("Filling in form fields and signing existing signature fields"));
    assert!(!ticked(&h, "Enable copying of text, images, and other content") && ticked(&h, screen_readers));
    pick(&mut h, "Compatibility", "Acrobat 3.0 and later (40-bit RC4)");
    h.run_steps(2);
    assert_eq!(value(&h, "Printing allowed").as_deref(), Some("None"), "back to the stricter choice");
    assert_eq!(value(&h, "Changes allowed").as_deref(), Some("None"), "back to the stricter choice");
    assert!(!ticked(&h, screen_readers), "screen readers follow copying");
    assert!(disabled(&h, screen_readers));
    open(&mut h, "Printing allowed");
    assert!(disabled(&h, "Low Resolution (150 dpi)"), "low-resolution printing is unavailable with 40-bit RC4");
    assert!(!disabled(&h, "High Resolution"));
    close(&mut h);
    open(&mut h, "Changes allowed");
    for label in ["Inserting, deleting, and rotating pages", "Filling in form fields and signing existing signature fields"] {
        assert!(disabled(&h, label), "{label} is unavailable with 40-bit RC4");
    }
    for label in ["Commenting, filling in form fields, and signing existing signature fields", "Any except extracting pages"] {
        assert!(!disabled(&h, label), "{label} stays available");
    }
    close(&mut h);
    // Copying allowed: screen readers come with it.
    h.get_by_label("Enable copying of text, images, and other content").click();
    h.run_steps(2);
    assert!(ticked(&h, screen_readers));
}

/// A document that already has security asks before its security is changed ("If you receive a
/// prompt, click Yes to change the security"): No leaves it as it is, Yes goes on to the settings.
#[test]
fn changing_existing_security_asks_first() {
    use egui::accesskit::Role;

    let mut cos = pdfcraft_cos::Document::open(std::sync::Arc::new(FIXTURE.to_vec())).unwrap();
    cos.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "boss",
        permissions: -1,
        encrypt_metadata: true,
        seed: [3; 32],
    })
    .unwrap();
    let protected = pdfcraft_cos::write_full(&cos, &Default::default()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes(&egui::Context::default(), "secured.pdf", None, protected.clone()).unwrap();
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    assert!(h.state().session.get(id).unwrap().security_summary().is_some());
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    h.get_by_label_contains("already has security settings");
    assert!(h.query_by_label("Type Password").is_none(), "the settings wait for the answer");
    h.get_by_role_and_label(Role::Button, "No").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("already has security settings").is_none());
    assert!(h.query_by_label("Type Password").is_none(), "No leaves the settings closed");
    assert!(!h.state().session.get(id).unwrap().dirty, "nothing changed");
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Yes").click();
    h.run_steps(4);
    open_password_pair(&mut h, "New-Secret-9");
    assert!(apply_enabled(&h), "typed into the settings");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.query_by_label("Apply").is_none(), "Apply closes the dialog");
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.security_summary().is_some_and(|s| s.pending), "{:?}", doc.security_summary());
    let bytes = h.state().session.save_bytes(id).unwrap();
    assert!(pdfcraft_cos::Document::open_with_password(bytes.clone(), None).is_err());
    assert!(pdfcraft_cos::Document::open_with_password(bytes, Some("New-Secret-9")).is_ok());
}

/// A document without security goes straight to the settings.
#[test]
fn an_unprotected_document_is_not_asked() {
    let mut h = harness();
    assert!(h.execute("protect.password"));
    h.run_steps(2);
    assert!(h.query_by_label_contains("already has security settings").is_none());
    assert!(h.query_all_by_label("Type Password").next().is_some(), "the settings show at once");
}
