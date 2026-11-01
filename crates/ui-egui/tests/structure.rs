//! Standards ▸ Validate structure in the real shell (egui_kittest): the dialog lists where the
//! document departs from the Arlington PDF Model, and a finding on a page turns to that page.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// Two pages; the second has a link annotation whose highlighting mode (/H) isn't one of N, I,
/// O or P.
fn document() -> Vec<u8> {
    with_link("<< /Type /Annot /Subtype /Link /Rect [10 10 50 30] /H /Glow /PdfCraftNote 1 >>")
}

/// Two pages; the second has the link annotation `link`.
fn with_link(link: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Annots [5 0 R] >>",
        link,
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

fn harness(language: &'static str) -> Harness<'static, PdfCraftApp> {
    harness_of(language, document())
}

fn harness_of(language: &'static str, doc: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", language).unwrap();
        app.open_bytes("structure.pdf", None, doc.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn validate_structure_lists_findings_and_turns_to_their_page() {
    let mut h = harness("en");
    assert!(h.state_mut().execute("standards.structure"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Structure));
    h.get_by_label("Errors (1)");
    h.get_by_label("Information (1)");
    // Information starts hidden: the key the model doesn't define is counted, not listed.
    assert!(h.query_by_label("PdfCraftNote is not an entry of AnnotLink in the model").is_none());
    h.get_by_label("H in AnnotLink is /Glow, which is not one of /N, /I, /O, /P").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None, "the dialog makes way for the page");
    let i = h.state().active_ids().unwrap().0;
    assert_eq!(h.state().views[i].current, 1, "turned to the second page");
    assert_eq!(h.state().views[i].flash.map(|(p, r, _)| (p, r)), Some((1, [10.0, 10.0, 50.0, 30.0])), "the link is flashed");

    // The command shows the same report again while the document is unchanged.
    let checked = h.state().structure_report().unwrap().objects;
    assert!(h.state_mut().execute("standards.structure"));
    h.run_steps(2);
    assert_eq!(h.state().structure_report().unwrap().objects, checked);
    h.get_by_label("Information (1)").click();
    h.run_steps(2);
    h.get_by_label("PdfCraftNote is not an entry of AnnotLink in the model");
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
}

/// Without errors, the dialog says the structure follows the model only if every rule was
/// evaluated: the model has rules this validator can't evaluate (`InNameTree`), and those are left
/// open.
#[test]
fn without_errors_the_dialog_claims_only_the_rules_it_evaluated() {
    let mut h = harness_of("en", with_link("<< /Type /Annot /Subtype /Link /Rect [10 10 50 30] /H /I >>"));
    assert!(h.state_mut().execute("standards.structure"));
    h.run_steps(2);
    h.get_by_label("Errors (0)");
    assert!(!h.state().structure_report().unwrap().not_checked.is_empty());
    h.get_by_label("No errors in the rules that were evaluated.");
    assert!(h.query_by_label("No errors: the structure follows the Arlington PDF Model.").is_none());
}

/// The findings read in the interface's language, with the document's own names left as they are.
#[test]
fn findings_are_said_in_spanish() {
    let mut h = harness("es");
    assert!(h.state_mut().execute("standards.structure"));
    h.run_steps(2);
    h.get_by_label("Validar estructura");
    h.get_by_label("Errores (1)");
    h.get_by_label("H en AnnotLink es /Glow, que no es uno de /N, /I, /O, /P");
}
