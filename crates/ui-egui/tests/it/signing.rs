//! Use a certificate in the real shell: draw a signature, create a digital ID, sign, validate.

use egui::{Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::SignatureStatus;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp, QuickTool, RightPanel, SignStep};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length 45 >> stream
BT /F1 14 Tf 20 150 Td (Please sign below) Tj ET
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A fresh folder per call: tests run in parallel and must not remove each other's files.
fn dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("pdfcraft-signing-ui-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness(dir: std::path::PathBuf) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("contract.pdf", None, FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.export_dir_override = Some(dir.to_string_lossy().into_owned());
        app.save_override = Some(dir.join("contract_signed.pdf").to_string_lossy().into_owned());
        app
    });
    h.run_steps(6);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    pos2(r.left() + x / 300.0 * r.width(), r.top() + (200.0 - y) / 200.0 * r.height())
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
}

fn field<'a>(h: &'a Harness<'static, PdfCraftApp>, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_label(label).last().unwrap_or_else(|| panic!("no {label:?}"))
}

#[test]
fn ctrl_alt_q_types_an_email_without_quitting() {
    let mut h = harness(dir());
    h.state_mut().execute("sign.digital");
    let (a, b) = (at(&h, 40.0, 90.0), at(&h, 220.0, 40.0));
    drag(&mut h, a, b);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().step, SignStep::Configure);
    field(&h, "Email Address").click();
    h.run_steps(1);
    field(&h, "Email Address").type_text("test");
    h.run_steps(1);
    // The key and text events produced by Ctrl+Alt+Q on German QWERTZ.
    h.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Q,
        physical_key: Some(egui::Key::Q),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers { ctrl: true, command: true, alt: true, ..Default::default() },
    });
    h.input_mut().events.push(egui::Event::Text("@".into()));
    h.step();
    assert!(!h.output().viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::Close)));
    assert_eq!(h.state().sign_draft.as_ref().unwrap().new_id.email, "test@");
    field(&h, "Email Address").type_text("example.org");
    h.run_steps(1);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().new_id.email, "test@example.org");
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
}

#[test]
fn drawing_a_signature_creating_an_id_signing_and_trusting() {
    let dir = dir();
    let mut h = harness(dir.clone());
    assert!(h.state_mut().execute("sign.digital"));
    assert_eq!(h.state().quick_tool, QuickTool::SignArea { certify: false });
    let (a, b) = (at(&h, 40.0, 90.0), at(&h, 220.0, 40.0));
    drag(&mut h, a, b);
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    assert_eq!(h.state().sign_draft.as_ref().map(|d| d.step), Some(SignStep::Configure), "no IDs yet: configure one");

    // Create a new digital ID (P-256 keeps the test fast).
    h.get_by_label("Configure a Digital ID for Signing");
    field(&h, "Name").click();
    h.run_steps(1);
    field(&h, "Name").type_text("Grace Hopper");
    h.state_mut().sign_draft.as_mut().unwrap().new_id.key = 3;
    for (label, text) in [("Password", "secret12"), ("Confirm Password", "secret12")] {
        field(&h, label).click();
        h.run_steps(1);
        field(&h, label).type_text(text);
        h.run_steps(1);
    }
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert_eq!(h.state().digital_ids.len(), 1);
    assert!(std::path::Path::new(&h.state().digital_ids[0].path).exists(), "saved as a .p12");
    h.get_by_label("Grace Hopper");
    h.get_by_label("Continue").click();
    h.run_steps(2);
    h.get_by_label("Sign as \"Grace Hopper\"");

    // A wrong password says so.
    field(&h, "Digital ID password").click();
    h.run_steps(1);
    field(&h, "Digital ID password").type_text("nope");
    h.get_by_label("Sign").click();
    h.run_steps(3);
    h.get_by_label("The password is incorrect.");
    h.state_mut().sign_draft.as_mut().unwrap().password = "secret12".into();
    field(&h, "Reason").click();
    h.run_steps(1);
    field(&h, "Reason").type_text("I approve");
    h.get_by_label("Sign").click();
    h.run_steps(6);

    let s = h.state();
    assert_eq!(s.dialog, None);
    assert!(dir.join("contract_signed.pdf").exists());
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.name, "contract_signed.pdf");
    let sig = doc.signatures.iter().find(|x| x.signed).expect("signed");
    assert_eq!((sig.status, sig.reason.as_deref(), sig.page), (SignatureStatus::Unknown, Some("I approve"), Some(0)));
    assert_eq!(s.right, Some(RightPanel::Signatures));
    h.get_by_label("At least one signature has problems.");

    // Signatures panel: expand, trust the signer → valid.
    h.get_by_label_contains("Signed by Grace Hopper").click();
    h.run_steps(2);
    h.get_by_label("Signature validity is UNKNOWN:");
    // Seed a find so the trust path proves it invalidates the view: trust rebuilds
    // the display copy, and the next frame must draw from it without any zoom change.
    h.state_mut().views[0].find = Some(pdfcraft_ui_egui::canvas::Find { matches: vec![(0, 0..5)], ..Default::default() });
    h.get_by_label("Add to trusted certificates").click();
    h.run_steps(3);
    h.get_by_label("Signed and all signatures are valid.");
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().signatures[0].status, SignatureStatus::Valid);
    assert!(s.views[0].find.as_ref().is_some_and(|f| f.matches.is_empty()), "trust invalidates the view without zooming");
    assert!(!s.session.get(s.views[0].id).unwrap().dirty, "trust alone is not an edit");
    // Show certificate: the Certificate Viewer.
    h.get_by_label("Show certificate…").click();
    h.run_steps(3);
    h.get_by_label("Certificate Viewer");
    assert!(h.get_all_by_label_contains("CN=Grace Hopper").count() >= 2, "issued to and by (self-signed)");
    h.run_steps(2);
    h.get_by_label("Details").click();
    h.run_steps(2);
    h.get_by_label("SHA-256 digest");
    h.run_steps(2);
    h.get_by_label("Trust").click();
    h.run_steps(2);

    h.get_by_label("This certificate is in your list of trusted certificates.");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    // View signed version opens the bytes the signature covers as a new document.
    h.get_by_label("View signed version").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.views.len(), 2);
    let v = s.session.get(s.views[1].id).unwrap();
    assert!(v.name.contains("signed version") && v.signatures.iter().any(|x| x.signed));
    assert!(s.views[0].snapshot.is_none());
    assert!(s.views[1].snapshot.as_ref().is_some_and(|snap| snap.kind == pdfcraft_ui_egui::canvas::SnapshotKind::SignedVersion));
    // A repeat click reuses the snapshot tab instead of opening another identical one.
    h.get_by_label("View signed version").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!((s.views.len(), s.active), (2, Some(1)));
    // A repeat from the original tab reuses it too.
    h.state_mut().active = Some(0);
    h.run_steps(1);
    h.get_by_label("View signed version").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!((s.views.len(), s.active), (2, Some(1)));
    // The snapshot tab says what it is.
    h.get_by_label_contains("You are viewing the signed version of");
    // Closing a clean snapshot closes the tab with no save prompt. The bar's own
    // Close is addressed from its message, because the tab chrome has a Close button
    // with the same label.
    let label = h.get_by_label_contains("You are viewing the signed version of");
    let label_row = label.rect().y_range();
    let mut node = label;
    let mut close = None;
    for _ in 0..4 {
        if let Some(pill) = node.query_by_label("Close") {
            close = Some(pill);
            break;
        }
        let Some(up) = node.parent() else { break };
        node = up;
    }
    let close = close.expect("the notice bar holds its own Close");
    // The bar's Close sits in the bar's row and the tab chrome's sits above it. Without
    // this the walk could reach the chrome's button, which closes the same tab, and the
    // test would pass with the bar's own button never pressed.
    assert!(label_row.contains(close.rect().center().y), "the Close found is the bar's own");
    close.click();
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1);
    assert!(h.state().close_request.is_none());
    // Closing a dirty snapshot asks about saving instead of closing at once.
    h.get_by_label("View signed version").click();
    h.run_steps(3);
    let id = h.state().views[1].id;
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Dirty".into() }).unwrap();
    h.state_mut().views[1].close_requested = true;
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 2);
    h.get_by_label("Save changes to “contract_signed (signed version).pdf” before closing?");
    h.get_by_label("Don't save").click();
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1);
    // Trusted certificates and the ID list persist.
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert_eq!((fresh.digital_ids.len(), fresh.session.trusted_certificates().len()), (1, 1));
}

#[test]
fn certifying_without_a_visible_signature() {
    let dir = dir().join("certify");
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness(dir.clone());
    // An ID from a file.
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    assert!(h.state_mut().execute("sign.certify_invisible"));
    h.run_steps(2);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        assert_eq!((d.rect, d.certify), (None, Some(2)));
        d.new_id.create = false;
        d.new_id.file = p12.into();
        d.new_id.file_password = "test".into();
    }
    h.run_steps(1);
    h.get_by_label("Continue").click();
    h.run_steps(2);
    assert_eq!(h.state().digital_ids.len(), 1);
    // The dialog shrinks onto this step. egui centers an area from the previous frame's size,
    // so give the buttons a frame to land before clicking.
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(2);
    h.get_by_label("Certify as \"Test Signer EC\"");
    h.get_by_label("Permitted actions after certifying");
    h.state_mut().sign_draft.as_mut().unwrap().password = "test".into();
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.sign_draft.as_ref().and_then(|d| d.error.clone()), None);
    let doc = s.session.get(s.views[0].id).unwrap();
    let sig = doc.signatures.iter().find(|x| x.signed).unwrap();
    assert_eq!((sig.certify, sig.visible), (Some(2), false));
    h.get_by_label_contains("Certified by Test Signer EC");
}

#[test]
fn save_as_keeps_a_signed_document_incremental() {
    // #2165: Save As to a new file rewrites unsigned documents in full; a signed one is appended
    // to, or its signature would no longer cover the file.
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let digital_id = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap();
    let mut s = pdfcraft_engine::Session::new();
    let id = s.open("contract.pdf", None, std::sync::Arc::new(FIXTURE.to_vec()), None).unwrap();
    let opts = pdfcraft_engine::SignOptions { page: 0, rect: Some([20.0, 20.0, 180.0, 60.0]), ..Default::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    let out = dir().join("copy.pdf");
    let (bytes, cert) = (signed.to_vec(), digital_id.certificate.clone());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("contract.pdf", None, bytes).expect("opens");
        app.session.set_trusted_certificates(vec![cert]);
        app
    });
    h.run_steps(4);
    let status = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().signatures.iter().find(|x| x.signed).map(|x| x.status)
    };
    assert_eq!(status(&h), Some(SignatureStatus::Valid));
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::As), "a signed document can be saved as a copy");
    let saved = std::fs::read(&out).expect("file written");
    assert!(saved.starts_with(&signed), "the signed revision is kept as it was");
    assert_eq!(status(&h), Some(SignatureStatus::Valid));
}

/// The fixture with an empty signature field across the bottom of the page.
const WITH_FIELD: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Annots [6 0 R] >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Sig /T (Approver) /Rect [20 20 280 80] /P 3 0 R /F 4 >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn clicking_an_empty_signature_field_signs_it() {
    let dir = dir().join("field");
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness(dir.clone());
    h.state_mut().open_bytes("form.pdf", None, WITH_FIELD.to_vec()).unwrap();
    h.state_mut().active = Some(1);
    h.run_steps(6);
    let p = {
        let r = h.state().views[1].page_screen_rect(0).expect("on screen");
        pos2(r.left() + 150.0 / 300.0 * r.width(), r.top() + (200.0 - 50.0) / 200.0 * r.height())
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.field.clone()).as_deref(), Some("Approver"));
    let entry = {
        let id = pdfcraft_engine::sign::pkcs12::open(
            &std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap(),
            "test",
        )
        .unwrap();
        h.state_mut().add_digital_id(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12"), &id.certificate)
    };
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.selected = Some(entry);
        d.step = SignStep::SignAs;
        d.password = "test".into();
    }
    h.run_steps(2);
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    let sig = s.session.get(s.views[1].id).unwrap().signatures.iter().find(|x| x.field == "Approver").cloned().unwrap();
    assert!(sig.signed && sig.visible);
}

#[test]
fn the_fill_and_sign_signature_or_an_image_shows_in_a_digital_signature() {
    use pdfcraft_ui_egui::SignGraphic;
    use pdfcraft_ui_egui::fill_sign::SavedSig;
    let dir = dir().join("graphic");
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness(dir.clone());
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let cert = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap().certificate;
    let sign_as = |h: &mut Harness<'static, PdfCraftApp>, rect: [f64; 4]| {
        h.state_mut().start_signing(0, Some(rect), None, None);
        let entry = h.state_mut().add_digital_id(p12, &cert);
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        (d.selected, d.step, d.password) = (Some(entry), SignStep::SignAs, "test".into());
        h.run_steps(2);
    };

    // Without a saved Fill & Sign signature there is nothing to choose.
    sign_as(&mut h, [20.0, 110.0, 280.0, 140.0]);
    h.get_by_label("Graphic");
    assert!(h.get_by_label("Fill & Sign signature").accesskit_node().is_disabled(), "disabled without one");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    // The saved signature (typed here) takes the name's place.
    h.state_mut().signature = Some(SavedSig::Typed("Grace Hopper".into()));
    sign_as(&mut h, [20.0, 110.0, 280.0, 140.0]);
    h.get_by_label("Fill & Sign signature").click();
    h.run_steps(2);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().graphic, SignGraphic::FillSign);
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.sign_draft.as_ref().and_then(|d| d.error.clone()), None);
    let sig = s.session.get(s.views[0].id).unwrap().signatures.iter().find(|x| x.signed).cloned().unwrap();
    assert!(sig.visible && sig.status == SignatureStatus::Unknown, "{:?}", sig.details);
    // Remembered for the next signature, and across a restart.
    assert_eq!(s.sign_graphic, SignGraphic::FillSign);
    sign_as(&mut h, [20.0, 20.0, 280.0, 60.0]);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().graphic, SignGraphic::FillSign);

    // An image (the picker's result is set directly: the file dialog is native).
    let image = {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(60, 20, image::Rgba([200, 0, 0, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        pdfcraft_engine::SignatureImage::from_bytes(png.get_ref()).unwrap()
    };
    h.state_mut().sign_draft.as_mut().unwrap().graphic = SignGraphic::Image(image.clone());
    h.run_steps(2);
    h.get_by_label("Choose image…");
    h.get_by_label("Sign").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.sign_draft.as_ref().and_then(|d| d.error.clone()), None);
    let signed = std::fs::read(dir.join("contract_signed.pdf")).unwrap();
    assert!(signed.windows(6).any(|w| w == b"/Image"), "the image is embedded");
    assert_eq!(s.session.get(s.views[0].id).unwrap().signatures.iter().filter(|x| x.signed).count(), 2);
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&s.persist());
    assert_eq!(fresh.sign_graphic, SignGraphic::Image(image));
    // A saved choice that can't be read back falls back to the name.
    fresh.restore(r#"{"sign_graphic": {"Image": "not base64"}}"#);
    assert_eq!(fresh.sign_graphic, SignGraphic::Name);
}

/// The decoded text of every visible signature appearance (Helvetica forms) in a signed file.
fn stamped(path: &std::path::Path) -> String {
    use pdfcraft_cos::{Document, ObjRef, Object};
    let doc = Document::open(std::sync::Arc::new(std::fs::read(path).unwrap())).unwrap();
    let mut text = String::new();
    for num in doc.object_numbers() {
        if let Object::Stream(s) = &*doc.get(ObjRef { num, generation: 0 })
            && s.dict.name(b"Subtype") == Some(b"Form")
        {
            let content = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
            if content.contains("/Helv") {
                text.push_str(&content);
            }
        }
    }
    text
}

/// A signature signed in German is stamped with German labels. In Simplified Chinese, whose
/// labels the signature's font can't draw, it is stamped with English ones and the dialog says
/// so under the preview: signing neither fails nor writes `?` for them.
#[test]
fn the_signature_labels_follow_the_interface_language() {
    use pdfcraft_ui_egui::i18n::{Lang, tr};
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let cert = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap().certificate;
    let note = "The labels are in English: the signature's font can't draw them in this language.";
    for (code, labels, in_english) in [("de", ["(Digital", "(Grund:", "(Datum:"], false), ("zh-hans", ["(Digitally", "(Reason:", "(Date:"], true)] {
        let dir = dir().join(code);
        std::fs::create_dir_all(&dir).unwrap();
        let mut h = harness(dir.clone());
        let lang = Lang::from_code(code).unwrap();
        h.state_mut().set_option("language", code).unwrap();
        h.state_mut().start_signing(0, Some([20.0, 110.0, 280.0, 180.0]), None, None);
        let entry = h.state_mut().add_digital_id(p12, &cert);
        {
            let d = h.state_mut().sign_draft.as_mut().unwrap();
            (d.selected, d.step, d.password, d.reason) = (Some(entry), SignStep::SignAs, "test".into(), "OK".into());
            d.appearance.reason = true;
        }
        h.run_steps(3);
        assert_eq!(h.query_all_by_label(tr(lang, note)).count(), usize::from(in_english), "{code}: the note under the preview");
        field(&h, tr(lang, "Sign")).click();
        h.run_steps(6);
        assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.error.clone()), None, "{code}");
        let content = stamped(&dir.join("contract_signed.pdf"));
        for label in labels {
            assert!(content.contains(label), "{code}: {label}: {content}");
        }
        assert!(!content.contains('?'), "{code}: {content}");
        assert_eq!(content.contains("Digitally") || content.contains("Reason"), in_english, "{code}: {content}");
    }
}

#[test]
fn os_store_identities_are_not_persisted() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    for path in ["windows:Store signer", "keychain:Store signer", "file-id.p12"] {
        app.digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
            path: path.into(),
            name: "Signer".into(),
            issuer: "Test issuer".into(),
            email: String::new(),
            expires: "2030.01.01".into(),
            unusable: None,
        });
    }
    let saved = app.persist();
    let settings: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(settings["digital_ids"].as_array().unwrap().len(), 1);
    assert_eq!(settings["digital_ids"][0]["path"], "file-id.p12");
    let mut restored = PdfCraftApp::new();
    restored.set_option("language", "en").unwrap();
    restored.restore(&saved);
    assert_eq!(restored.digital_ids.len(), 1);
    assert_eq!(restored.digital_ids[0].path, "file-id.p12");
}

#[test]
fn windows_store_identity_does_not_ask_for_a_file_password() {
    let mut h = harness(dir());
    h.state_mut().start_signing(0, None, None, None);
    // A synthetic picker entry exercises the dialog without a real Windows identity.
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: "windows:No Such Signer".into(),
        name: "Store signer".into(),
        issuer: "Test issuer".into(),
        email: String::new(),
        expires: "2030.01.01".into(),
        unusable: None,
    });
    let draft = h.state_mut().sign_draft.as_mut().unwrap();
    draft.step = SignStep::Choose;
    draft.selected = Some(0);
    h.run_steps(3);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    h.get_by_label("Sign as \"Store signer\"");
    assert_eq!(h.query_all_by_label("Digital ID password").count(), 0);
    h.get_by_label("The key is in the Windows certificate store, which may ask to allow PdfCraft to use it.");
    h.get_by_label("Sign").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Sign));
    assert!(h.state().sign_draft.as_ref().unwrap().error.as_ref().unwrap().contains("Windows certificate store"));
}

/// Certificate names are as long as their owners made them: the picker cuts both lines of a
/// row with "…" instead of painting them past the dialog's edge.
#[test]
fn long_digital_id_names_stay_inside_the_picker() {
    let mut h = harness(dir());
    h.state_mut().start_signing(0, None, None, None);
    let long = "a certificate subject much longer than the picker is wide ".repeat(4);
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: "windows:No Such Signer".into(),
        name: long.clone(),
        issuer: long,
        email: String::new(),
        expires: "2030.01.01".into(),
        unusable: None,
    });
    h.state_mut().sign_draft.as_mut().unwrap().step = SignStep::Choose;
    h.run_steps(3);
    let right = h.get_by_label("Continue").rect().right();
    let lines: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text.contains("much longer") => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 2, "the name and the details line");
    for line in lines {
        assert!(line.galley.elided, "{}", line.galley.job.text);
        assert!(
            line.pos.x + line.galley.size().x <= right,
            "the line ends at {} but the dialog's buttons end at {right}",
            line.pos.x + line.galley.size().x
        );
    }
}

/// A store certificate that can't sign is shown with its reason (so the user learns why an
/// identity Acrobat lists is missing, issue #179) but is never selected or chosen.
#[test]
fn an_unusable_id_is_shown_with_its_reason_and_cannot_be_chosen() {
    let mut h = harness(dir());
    // Not a `windows:` path: those are replaced by the real store listing when signing starts.
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: "store-test:broken".into(),
        name: "Broken signer".into(),
        issuer: String::new(),
        email: String::new(),
        expires: String::new(),
        unusable: Some("its private key can't be opened: the key is missing from its key store (Windows error 0x80090016)".into()),
    });
    h.state_mut().start_signing(0, None, None, None);
    h.run_steps(3);
    let draft = h.state().sign_draft.as_ref().unwrap();
    assert_eq!(draft.step, SignStep::Choose, "shown, with its reason, rather than skipped to Configure");
    assert_eq!(draft.selected, None);
    h.get_by_label("Broken signer").click();
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().selected, None);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().step, SignStep::Choose);
    // A usable ID after it is the one selected to begin with.
    h.state_mut().digital_ids.push(pdfcraft_ui_egui::DigitalIdEntry {
        path: "store-test:fine".into(),
        name: "Working signer".into(),
        issuer: "Test issuer".into(),
        email: String::new(),
        expires: "2030.01.01".into(),
        unusable: None,
    });
    h.state_mut().start_signing(0, None, None, None);
    h.run_steps(3);
    assert_eq!(h.state().sign_draft.as_ref().unwrap().selected, Some(1));
}

/// Issue #1396: an imported digital ID can be removed from Sign with a Digital ID, after a
/// confirmation, and its file stays where it was.
#[test]
fn an_imported_digital_id_can_be_removed() {
    let dir = dir().join("remove");
    std::fs::create_dir_all(&dir).unwrap();
    let p12 = dir.join("signer.p12");
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12"), &p12).unwrap();
    let mut h = harness(dir.clone());
    assert!(h.state_mut().execute("sign.certify_invisible"));
    h.run_steps(2);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.new_id.create = false;
        d.new_id.file = p12.to_string_lossy().into_owned();
        d.new_id.file_password = "test".into();
    }
    h.run_steps(1);
    h.get_by_label("Continue").click();
    h.run_steps(4);
    assert_eq!(h.state().digital_ids.len(), 1);
    assert_eq!(h.state().sign_draft.as_ref().map(|d| d.step), Some(SignStep::Choose));

    // Remove asks first, and Cancel keeps the ID. The dialog changes size with the question;
    // egui centres it from the previous frame's size, so let it settle before each click.
    h.get_by_label("Remove").click();
    h.run_steps(4);
    h.get_by_label("Remove \"Test Signer EC\" from this list? The file itself is not deleted.");
    // The question's Cancel comes before the dialog's own.
    h.get_all_by_label("Cancel").next().unwrap().click();
    h.run_steps(4);
    assert!(h.query_by_label_contains("from this list?").is_none());
    assert_eq!(h.state().digital_ids.len(), 1);

    h.get_by_label("Remove").click();
    h.run_steps(4);
    h.get_by_label("Remove").click();
    h.run_steps(3);
    assert!(h.state().digital_ids.is_empty());
    assert!(p12.exists(), "the file itself is not deleted");
    // With no IDs left the dialog offers to configure one.
    assert_eq!(h.state().sign_draft.as_ref().map(|d| d.step), Some(SignStep::Configure));
    h.get_by_label("Configure a Digital ID for Signing");
}

/// Sign-and-save must use the same destination metadata policy as Save As.
#[cfg(target_os = "macos")]
#[test]
fn signing_preserves_destination_opening_attributes() {
    let dir = dir();
    let dest = dir.join("contract_signed.pdf");
    std::fs::write(&dest, FIXTURE).unwrap();
    xattr::set(&dest, "com.apple.LaunchServices.OpenWith", b"existing binding").unwrap();
    xattr::set(&dest, "com.apple.quarantine", b"0082;1234;Preview;").unwrap();
    let mut h = harness(dir.clone());
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let cert = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap().certificate;
    h.state_mut().start_signing(0, None, None, None);
    let entry = h.state_mut().add_digital_id(p12, &cert);
    let d = h.state_mut().sign_draft.as_mut().unwrap();
    (d.selected, d.step, d.password) = (Some(entry), SignStep::SignAs, "test".into());
    h.run_steps(2);
    h.get_by_label("Sign").click();
    h.run_steps(6);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().signatures.iter().any(|s| s.signed));
    assert_eq!(xattr::get(&dest, "com.apple.LaunchServices.OpenWith").unwrap(), Some(b"existing binding".to_vec()));
    assert_eq!(xattr::get(&dest, "com.apple.quarantine").unwrap(), Some(b"0082;1234;Preview;".to_vec()));
    let _ = std::fs::remove_dir_all(dir);
}
