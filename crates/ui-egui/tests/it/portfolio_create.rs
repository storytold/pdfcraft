//! Create a PDF ▸ PDF portfolio in the real shell: the dialog bundles the picked files into a
//! new portfolio tab, and a date it can't read keeps the dialog open with the reason.

use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::PdfCraftApp;

/// The app with the portfolio dialog open on a PDF and a Word file.
fn harness() -> Harness<'static, PdfCraftApp> {
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF".to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.begin_portfolio(vec![("Complaint.pdf".into(), pdf.clone(), None), ("Lease.docx".into(), b"PK not really".to_vec(), None)]);
        app
    });
    h.run_steps(3);
    h
}

#[test]
fn the_portfolio_dialog_bundles_the_files_into_a_new_portfolio() {
    let mut h = harness();
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::CreatePortfolio));
    if let Some(d) = h.state_mut().portfolio_draft.as_mut() {
        d.title = "Smith v. Jones".into();
        d.rows[1].description = "Signed lease".into();
        d.rows[0].date = "March 1, 2026".into();
    }
    h.run_steps(1);
    h.get_by_label("Create").click();
    h.run_steps(3);
    assert!(h.state().portfolio_draft.is_none());
    assert_eq!(h.state().dialog, None);
    let docs = h.state().session.docs();
    assert_eq!(docs.len(), 1);
    let info = &docs[0].info;
    assert_eq!(docs[0].name, "Smith v. Jones.pdf");
    assert!(info.portfolio.is_some(), "not a portfolio");
    let names: Vec<&str> = info.attachments.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"Complaint.pdf") && names.contains(&"Lease.docx"), "{names:?}");
}

#[test]
fn a_date_it_cannot_read_keeps_the_dialog_open_with_the_reason() {
    let mut h = harness();
    if let Some(d) = h.state_mut().portfolio_draft.as_mut() {
        d.rows[0].date = "someday".into();
    }
    h.run_steps(1);
    h.get_by_label("Create").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::CreatePortfolio));
    let error = h.state().portfolio_draft.as_ref().and_then(|d| d.error.clone()).unwrap_or_default();
    assert!(error.contains("Complaint.pdf") && error.contains("someday"), "{error}");
    assert!(h.state().session.docs().is_empty());
}
