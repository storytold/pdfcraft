//! Protect Using Password in the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{Changes, Printing};
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

fn open_password_pair(h: &mut Harness<'static, PdfCraftApp>, password: &str) {
    type_into(h, "Type Password", password);
    type_into(h, "Re-type Password", password);
}

/// Advanced Options sets an open password and a permissions password together, as the guide's
/// Password Security settings do ("setting both types of passwords is often beneficial"): the
/// saved file needs the open password to open, opens restricted with it, and the permissions
/// password lifts the restrictions.
#[test]
fn an_open_password_and_a_permissions_password_together() {
    let mut h = harness();
    h.state_mut().execute("protect.password");
    h.run_steps(2);
    h.get_by_label("Viewing");
    h.get_by_label("Advanced Options ⌄").click();
    h.run_steps(2);
    assert!(h.query_by_label("Printing allowed").is_none(), "no restrictions without a permissions password");
    h.get_by_label("Restrict editing and printing of the document").click();
    h.run_steps(2);
    h.get_by_label("Printing allowed");
    open_password_pair(&mut h, "Open-Sesame-42");
    h.get_by_label_contains("Type a permissions password");
    assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some());
    type_into(&mut h, "Change Permissions Password", "Boss-Key-7");
    type_into(&mut h, "Re-type Permissions Password", "Boss-Key-8");
    h.get_by_label_contains("permissions passwords don't match");
    h.state_mut().protect_draft.second_confirm = "Boss-Key-7".into();
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
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
    use egui::accesskit::Role;

    let mut h = harness();
    h.state_mut().execute("protect.password");
    h.run_steps(2);
    h.get_by_label("Editing").click();
    h.run_steps(2);
    h.get_by_label("Advanced Options ⌄").click();
    h.run_steps(2);
    let combo = |h: &mut Harness<'static, PdfCraftApp>, value: &str| {
        h.query_all_by_role(Role::ComboBox).find(|n| n.value().as_deref() == Some(value)).expect(value).click();
        h.run_steps(2);
    };
    let disabled = |h: &Harness<'static, PdfCraftApp>, label: &str| h.query_by(|n| n.label().as_deref() == Some(label) && n.is_disabled()).is_some();
    // 256-bit AES: every choice is available; pick the ones 40-bit RC4 can't keep.
    combo(&mut h, "High Resolution");
    assert!(!disabled(&h, "Low Resolution (150 dpi)"));
    h.get_by_label("Low Resolution (150 dpi)").click();
    h.run_steps(2);
    combo(&mut h, "None");
    assert!(!disabled(&h, "Inserting, deleting, and rotating pages"));
    h.get_by_label("Filling in form fields and signing existing signature fields").click();
    h.run_steps(2);
    assert!(!disabled(&h, "Enable text access for screen reader devices for the visually impaired"));
    let p = h.state().protect_draft.protection.clone();
    assert_eq!((p.printing, p.changes, p.copy, p.accessibility), (Printing::Low, Changes::FillSign, false, true));
    combo(&mut h, "Acrobat X and later (256-bit AES)");
    h.get_by_label("Acrobat 3.0 and later (40-bit RC4)").click();
    h.run_steps(3);
    let p = h.state().protect_draft.protection.clone();
    assert_eq!(
        (p.printing, p.changes, p.accessibility),
        (Printing::None, Changes::None, false),
        "back to the stricter choice; screen readers follow copying"
    );
    assert!(disabled(&h, "Enable text access for screen reader devices for the visually impaired"));
    // Printing and changes both say "None" now: the first list is printing, the second changes.
    let open = |h: &mut Harness<'static, PdfCraftApp>, nth: usize| {
        h.query_all_by_role(Role::ComboBox).filter(|n| n.value().as_deref() == Some("None")).nth(nth).expect("the list").click();
        h.run_steps(2);
    };
    open(&mut h, 0);
    assert!(disabled(&h, "Low Resolution (150 dpi)"), "low-resolution printing is unavailable with 40-bit RC4");
    assert!(!disabled(&h, "High Resolution"));
    open(&mut h, 0);
    open(&mut h, 1);
    for label in ["Inserting, deleting, and rotating pages", "Filling in form fields and signing existing signature fields"] {
        assert!(disabled(&h, label), "{label} is unavailable with 40-bit RC4");
    }
    for label in ["Commenting, filling in form fields, and signing existing signature fields", "Any except extracting pages"] {
        assert!(!disabled(&h, label), "{label} stays available");
    }
    open(&mut h, 1);
    // Copying allowed: screen readers come with it.
    h.get_by_label("Enable copying of text, images, and other content").click();
    h.run_steps(2);
    assert!(h.state().protect_draft.protection.accessibility);
}

/// A document that already has security asks before its security is changed ("If you receive a
/// prompt, click Yes to change the security"): No leaves it as it is, Yes goes on to the settings.
#[test]
fn changing_existing_security_asks_first() {
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("secured.pdf", None, protected.clone()).unwrap();
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    assert!(h.state().session.get(id).unwrap().security_summary().is_some());
    h.state_mut().execute("protect.password");
    h.run_steps(2);
    h.get_by_label_contains("already has security settings");
    assert!(h.query_by_label("Type Password").is_none(), "the settings wait for the answer");
    h.get_by_label("No").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    assert!(!h.state().session.get(id).unwrap().dirty, "nothing changed");
    h.state_mut().execute("protect.password");
    h.run_steps(2);
    h.get_by_label("Yes").click();
    h.run_steps(4);
    assert!(!h.state().protect_draft.ask_change);
    open_password_pair(&mut h, "New-Secret-9");
    assert_eq!(h.state().protect_draft.password, "New-Secret-9", "typed into the settings");
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None, "{:?} / {:?}", h.state().protect_draft.problem(), h.state().protect_draft.ask_change);
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
    h.state_mut().execute("protect.password");
    h.run_steps(2);
    assert!(h.query_by_label_contains("already has security settings").is_none());
    h.get_by_label("Type Password");
}
