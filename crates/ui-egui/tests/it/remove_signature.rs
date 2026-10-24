//! Remove Signature in the app: the Signatures panel's button opens the dialog, which warns and
//! saves a new file; the original is never written over, and a failed save leaves the document
//! as it was. Synthetic document, test digital ID (`crates/sign/tests/data`).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, SignStep};

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

fn dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("pdfcraft-remove-signature-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The app with the fixture signed and saved as `signed.pdf` in `dir`.
fn signed(dir: &std::path::Path) -> Harness<'static, PdfCraftApp> {
    let save = dir.join("signed.pdf").to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("contract.pdf", None, FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.save_override = Some(save);
        app
    });
    h.run_steps(6);
    let p12 = concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12");
    let id = pdfcraft_engine::sign::pkcs12::open(&std::fs::read(p12).unwrap(), "test").unwrap();
    let entry = h.state_mut().add_digital_id(p12, &id.certificate);
    h.state_mut().start_signing(0, Some([20.0, 20.0, 180.0, 60.0]), None, None);
    {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.selected = Some(entry);
        d.step = SignStep::SignAs;
        d.password = "test".into();
    }
    h.run_steps(2);
    h.get_by_label("Sign").click();
    h.run_steps(6);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().is_signed(), "the fixture is signed");
    h
}

fn open_dialog_from_the_panel(h: &mut Harness<'static, PdfCraftApp>) {
    h.get_by_label_contains("Signed by").click();
    h.run_steps(2);
    h.get_by_label("Remove signature…").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::RemoveSignature));
}

fn toast(h: &Harness<'static, PdfCraftApp>) -> String {
    h.state().toast.as_ref().map(|(t, _)| t.clone()).unwrap_or_default()
}

#[test]
fn the_panel_removes_a_signature_into_a_new_file_and_keeps_the_original() {
    let dir = dir();
    let mut h = signed(&dir);
    let original = std::fs::read(dir.join("signed.pdf")).unwrap();
    open_dialog_from_the_panel(&mut h);
    // The warning and the choice are shown before anything changes.
    h.get_by_label_contains("invalidates this signature in the version that results");
    h.get_by_label("Keep an empty signature field to sign again");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "nothing changed yet");

    let dest = dir.join("signed_unsigned.pdf");
    h.state_mut().save_override = Some(dest.to_string_lossy().into_owned());
    h.get_by_label("Remove and Save As…").click();
    h.run_steps(4);

    assert_eq!(std::fs::read(dir.join("signed.pdf")).unwrap(), original, "the original is untouched");
    let new = std::fs::read(&dest).expect("the new version is written");
    assert!(!new.windows(10).any(|w| w == b"/ByteRange"), "the only signature is gone from the new file, history included");
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.signatures.is_empty() && !doc.dirty);
    assert_eq!(doc.path.as_deref(), Some(dest.to_string_lossy().as_ref()), "the document now is the new file, ready to edit and sign");
    let msg = toast(&h);
    assert!(msg.contains("Removed the signature") && msg.contains("signed_unsigned.pdf") && msg.contains("checked to reopen"), "{msg}");
}

#[test]
fn the_original_file_is_never_the_destination() {
    let dir = dir();
    let mut h = signed(&dir);
    let original = std::fs::read(dir.join("signed.pdf")).unwrap();
    open_dialog_from_the_panel(&mut h);
    // save_override is still the signed file itself.
    h.get_by_label("Remove and Save As…").click();
    h.run_steps(4);
    assert!(toast(&h).contains("Choose another file"), "{}", toast(&h));
    assert_eq!(std::fs::read(dir.join("signed.pdf")).unwrap(), original);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.is_signed() && !doc.dirty, "nothing was removed");
}

// Case 8 — the new file can't be written: the original stays, nothing half-written is left, and
// the removal is undone so the document is as it was.
#[test]
fn a_failed_save_undoes_the_removal_and_leaves_no_file() {
    let dir = dir();
    let mut h = signed(&dir);
    let original = std::fs::read(dir.join("signed.pdf")).unwrap();
    open_dialog_from_the_panel(&mut h);
    let dest = dir.join("missing-folder").join("out.pdf");
    h.state_mut().save_override = Some(dest.to_string_lossy().into_owned());
    h.get_by_label("Remove and Save As…").click();
    h.run_steps(4);
    assert!(toast(&h).contains("Couldn't save"), "{}", toast(&h));
    assert!(!dest.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "only the original is in the folder: no temporary file left");
    assert_eq!(std::fs::read(dir.join("signed.pdf")).unwrap(), original);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.is_signed() && !doc.dirty, "the removal was undone");
}

#[test]
fn a_document_without_signatures_says_so_and_explains_page_content() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("plain.pdf", None, FIXTURE.to_vec()).expect("opens");
        app
    });
    h.run_steps(4);
    h.state_mut().run_command("sign.remove");
    h.run_steps(3);
    h.get_by_label("This document has no signature that can be removed.");
    h.get_by_label_contains("is not a field or an annotation");
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
}

// With "flatten Fill & Sign marks when saving" on, the save adds an edit of its own after the
// removal; a failed save must undo both, not just that one.
#[test]
fn a_failed_save_undoes_the_removal_even_after_flattening_marks() {
    let dir = dir();
    let mut h = signed(&dir);
    let id = h.state().views[0].id;
    let typed = pdfcraft_engine::Shape::TypedSignature { rect: [200.0, 20.0, 280.0, 50.0], contours: vec![vec![[0.1, 0.2], [0.5, 0.9], [0.9, 0.2]]] };
    let style = pdfcraft_engine::Style::default_for(&typed);
    let mark = pdfcraft_engine::NewAnnotation { page: 0, shape: typed, style, contents: String::new(), author: "Ada".into() };
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::AddAnnotation(mark)).unwrap();
    let before = h.state().session.get(id).unwrap().can_undo().map(str::to_owned);
    h.state_mut().flatten_fill_sign_on_save = true;
    h.run_steps(2);
    // The digital signature, not the visual one.
    h.state_mut().start_remove_signature(None);
    h.run_steps(2);
    assert!(h.state().remove_sig.as_ref().unwrap().items[0].field.is_some());
    let dest = dir.join("missing-folder").join("out.pdf");
    h.state_mut().save_override = Some(dest.to_string_lossy().into_owned());
    h.get_by_label("Remove and Save As…").click();
    h.run_steps(4);
    assert!(!dest.exists());
    let s = h.state();
    let doc = s.session.get(id).unwrap();
    assert!(doc.is_signed(), "the removal was undone, not only the flattening");
    assert_eq!(doc.can_undo().map(str::to_owned), before, "back to the state before the removal");
}

// From the panel row of a signature that can't be removed, the dialog shows why and asks for
// one that can, instead of saying none can.
#[test]
fn a_blocked_signature_asked_for_from_the_panel_asks_for_another() {
    let dir = dir();
    let mut h = signed(&dir);
    let id = h.state().views[0].id;
    let field = h.state().session.get(id).unwrap().signatures[0].field.clone();
    // Block it by hand, as a certification by another signature would.
    h.state_mut().start_remove_signature(Some(&field));
    {
        let d = h.state_mut().remove_sig.as_mut().unwrap();
        d.items.push(d.items[0].clone());
        d.items[0].blocked = Some(pdfcraft_engine::clear_signature::Blocker::LockedBy("Other".into()));
        d.selected = Some(0);
    }
    h.run_steps(3);
    h.get_by_label("Choose a signature that can be removed.");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none() && h.state().remove_sig.is_none());
}
