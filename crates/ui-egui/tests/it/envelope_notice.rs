//! Opening a CAdES envelope (`.pdf.p7m`, issue #1449) in the UI: the notice bar identifies the
//! signer and the read-only state, Extract makes an editable copy, and Document Properties lists
//! the envelope's details. A detached signature (`.p7s` + sibling PDF, issue #2129) gets the same
//! notice without the Extract button, and Properties names the signature file.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const FIXTURE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/cades-envelope.pdf.p7m"));
const SIGNED_PDF: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/openssl-signed.pdf"));
const SIGNED_P7S: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/openssl-signed.pdf.p7s"));

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("cades-envelope.pdf.p7m", None, FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(6);
    h
}

#[test]
fn envelope_notice_shows_the_signer_and_extract_opens_an_editable_copy() {
    let mut h = harness();

    // The command is registered where the File menu shows it.
    let spec = pdfcraft_engine::commands::command("file.extract_original").expect("registered");
    assert_eq!(spec.menu, Some("File"));

    // The notice bar: read-only, who signed, and that trust was left unchecked.
    h.get_by_label_contains("Opened from a CAdES signature envelope — read-only.");
    h.get_by_label_contains("Signed by PdfCraft Test on ");
    h.get_by_label_contains("the content matches what was signed");
    h.get_by_label_contains("trust was not checked");

    // Extract: the embedded PDF opens as a second, editable document, and the view follows it.
    h.get_by_label("Extract the original PDF…").click();
    h.run_steps(6);
    let s = h.state();
    assert_eq!(s.views.len(), 2, "the extracted copy is a new document");
    let copy = s.session.get(s.views[1].id).unwrap();
    assert!(copy.editable(), "the extracted copy is editable");
    assert!(copy.envelope.is_none(), "the copy is a plain document");
    assert_eq!(h.state().active, Some(1), "the view switched to the copy");
    assert_eq!(h.query_all_by_label_contains("Opened from a CAdES signature envelope").count(), 0, "no envelope bar over the editable copy");

    // Document Properties ▸ Advanced of the envelope document lists the envelope's details.
    h.state_mut().active = Some(0);
    h.run_steps(2);
    assert!(h.state_mut().execute("file.properties"));
    h.run_steps(2);
    h.get_by_label("Advanced").click();
    h.run_steps(2);
    h.get_by_label("Signer");
    h.get_by_label("PdfCraft Test");
    h.get_by_label("Signing time");
    let time = h.state().session.get(h.state().views[0].id).unwrap().envelope.as_ref().unwrap().signing_time.clone().unwrap();
    h.get_by_label(&time);
    h.get_by_label("Integrity");
    h.get_by_label("Matches what was signed");
}

/// The detached pair in a fresh temp folder, removed when the test ends. The `.p7s` opens by
/// its real path: a bytes-only open has no directory to look for the sibling document in.
struct TempPair {
    dir: std::path::PathBuf,
}

impl TempPair {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-detached-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("openssl-signed.pdf"), SIGNED_PDF).unwrap();
        std::fs::write(dir.join("openssl-signed.pdf.p7s"), SIGNED_P7S).unwrap();
        Self { dir }
    }
}

impl Drop for TempPair {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn detached_notice_has_no_extract_button() {
    let pair = TempPair::new();
    let path = pair.dir.join("openssl-signed.pdf.p7s").to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.open_path(&path);
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(6);

    // The document is the sibling beside the signature, and the engine flagged the pair.
    let (name, detached) = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let env = doc.envelope.as_ref().expect("a detached pair carries EnvelopeInfo");
        (doc.name.clone(), env.detached)
    };
    assert_eq!(name, "openssl-signed.pdf", "the document is the sibling, not the signature");
    assert!(detached, "opened from the sibling, not from inside an envelope");

    // The detached bar: the same signer and integrity copy, but no Extract button — the signed
    // PDF beside the .p7s is already the editable copy, so there is nothing to extract.
    h.get_by_label_contains("Opened from a detached CAdES signature — read-only.");
    h.get_by_label_contains("Signed by PdfCraft Test on ");
    h.get_by_label_contains("the content matches what was signed");
    h.get_by_label("Dismiss");
    assert_eq!(h.query_all_by_label_contains("Extract the original PDF").count(), 0, "no Extract on the detached bar");

    // Document Properties ▸ Advanced names the signature file the pair was opened from.
    assert!(h.state_mut().execute("file.properties"));
    h.run_steps(2);
    h.get_by_label("Advanced").click();
    h.run_steps(2);
    h.get_by_label("Detached signature file");
    h.get_by_label("openssl-signed.pdf.p7s");
    h.get_by_label("Signer");
    h.get_by_label("PdfCraft Test");
}
