//! Opening a CAdES envelope (`.pdf.p7m`, issue #1449) in the UI: the notice bar identifies the
//! signer and the read-only state, Extract makes an editable copy, and Document Properties lists
//! the envelope's details.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const FIXTURE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/cades-envelope.pdf.p7m"));

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
