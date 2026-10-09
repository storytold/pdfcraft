//! File ▸ Create ▸ PDF from Scanner in the real shell, against a fake network scanner.

use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_engine::scan::{Backend, Scanner, Source};
use pdfcraft_scan::fake::FakeEscl;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness(fake: &FakeEscl) -> Harness<'static, PdfCraftApp> {
    let (id, name) = (fake.id(), "PdfCraft Test Scanner".to_string());
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // Scan on this thread, so a click finishes within the test step.
        app.run_inline = true;
        app.scan_ui = Some(Default::default());
        let s = app.scan_ui.as_mut().unwrap();
        s.scanners = vec![Scanner { id: id.clone(), name, backend: Backend::Escl }];
        s.selected = Some(id);
        s.settings = pdfcraft_engine::scan::Preset::ColorDocument.settings();
        app.dialog = Some(Dialog::Scanner);
        app
    });
    h.run_steps(3);
    h
}

#[test]
fn scan_dialog_scans_a_page_into_a_new_pdf() {
    let fake = FakeEscl::start(0).unwrap();
    let mut h = harness(&fake);
    h.get_by_label("Create PDF from Scanner");
    if let Ok(path) = std::env::var("PDFCRAFT_SCAN_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_label("Scan").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none(), "one flatbed page is made into a PDF at once");
    let docs = h.state().session.docs();
    assert_eq!(docs.len(), 1);
    assert_eq!((docs[0].name.as_str(), docs[0].info.pages.len(), docs[0].dirty), ("Scan.pdf", 1, true));
    // Color Document is 200 dpi; the fake scanner offers 150 and 300, and its Letter bed is 612 × 792 pt.
    let page = &docs[0].info.pages[0];
    assert!((page.width - 612.0).abs() < 1.0 && (page.height - 792.0).abs() < 1.0, "{} × {}", page.width, page.height);
}

#[test]
fn scan_dialog_collects_several_pages_then_creates_one_pdf() {
    let fake = FakeEscl::start(0).unwrap();
    let mut h = harness(&fake);
    h.state_mut().scan_ui.as_mut().unwrap().prompt_more = true;
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, Some(Dialog::Scanner), "it asks for more pages");
    assert_eq!(h.state().scan_ui.as_ref().unwrap().pages.len(), 1);
    h.get_by_label("1 page scanned");
    h.get_by_label("Scan next page").click();
    h.run_steps(4);
    h.get_by_label("2 pages scanned");
    h.get_by_label("Create PDF").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    let docs = h.state().session.docs();
    assert_eq!((docs.len(), docs[0].info.pages.len()), (1, 2));
    assert!(h.state().scan_ui.as_ref().unwrap().pages.is_empty());
}

#[test]
fn scan_errors_are_shown_and_cancel_keeps_nothing() {
    let fake = FakeEscl::start(0).unwrap();
    let mut h = harness(&fake);
    // An empty document feeder.
    h.state_mut().scan_ui.as_mut().unwrap().settings.source = Source::Feeder;
    h.run_steps(2);
    h.get_by_label("Scan").click();
    h.run_steps(4);
    let error = h.state().scan_ui.as_ref().unwrap().error.clone().unwrap_or_default();
    assert!(error.contains("feeder is empty"), "{error}");
    assert_eq!(h.state().dialog, Some(Dialog::Scanner));
    assert!(h.state().session.docs().is_empty());
    // The feeder is loaded now: both sheets become one PDF.
    fake.load_feeder(2);
    h.get_by_label("Scan").click();
    h.run_steps(4);
    assert_eq!(h.state().session.docs()[0].info.pages.len(), 2);
    // Cancel closes without making a document.
    h.state_mut().dialog = Some(Dialog::Scanner);
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().session.docs().len(), 1);
}

#[test]
fn typed_addresses_become_network_scanners() {
    let mut app = PdfCraftApp::new();
    app.scan_ui = Some(Default::default());
    app.scan_ui.as_mut().unwrap().address = "192.168.1.20".into();
    app.add_scanner_address();
    let s = app.scan_ui.as_ref().unwrap();
    assert_eq!(s.selected.as_deref(), Some("escl:http://192.168.1.20/eSCL"));
    assert_eq!(s.scanners.len(), 1);
    app.scan_ui.as_mut().unwrap().address = "file:///etc/passwd".into();
    app.add_scanner_address();
    assert!(app.scan_ui.as_ref().unwrap().error.is_some());
    assert_eq!(app.scan_ui.as_ref().unwrap().scanners.len(), 1);
}
