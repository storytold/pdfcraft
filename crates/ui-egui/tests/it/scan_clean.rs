//! Scan & OCR ▸ Enhance scanned file in the real shell (egui_kittest): the dialog, the run and
//! its report, and that the page keeps its comment while its scan is cleaned in place.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{Edit, NewAnnotation, NoteIcon, Session, Shape, Style};
use pdfcraft_ui_egui::PdfCraftApp;

/// A two-page PDF: page 1 a grey scan with a darker gutter and text bars (and a sticky note),
/// page 2 the same scan again.
fn scan() -> Vec<u8> {
    let img = image::GrayImage::from_fn(300, 400, |x, y| {
        let light = 0.55 + 0.45 * (x as f32 / 60.0).min(1.0);
        let ink = if (60..360).contains(&y) && (y - 60) % 40 < 6 && (60..240).contains(&x) { 0.12 } else { 0.78 };
        image::Luma([(ink * light * 255.0) as u8])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut s = Session::new();
    let bytes = s.create_from_images(&[("a.png".into(), png.clone()), ("b.png".into(), png)]).unwrap();
    let id = s.open("scan.pdf", None, bytes, None).unwrap();
    let shape = Shape::Note { at: [20.0, 20.0], icon: NoteIcon::Comment };
    let note = NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: "Keep me".into(), author: "Tester".into() };
    s.apply(id, Edit::AddAnnotation(note)).unwrap();
    s.save_bytes(id).unwrap().to_vec()
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn enhance_scanned_file_cleans_the_chosen_pages_and_says_what_it_did() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    assert!(h.state().session.get(id).unwrap().comment_props(0, 0).is_some());
    assert!(h.state_mut().execute("ocr.enhance"));
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Background removal").count(), 2, "the label and the slider it names");
    h.get_by_label("Edge shadow removal");
    h.get_by_label("Despeckle");
    h.get_by_role_and_label(egui::accesskit::Role::RadioButton, "Current page").click();
    h.run_steps(1);
    h.get_by_label("Enhance").click();
    h.run_steps(3);
    let app = h.state();
    assert!(app.scan_clean_run.is_none() && app.dialog.is_none(), "finished and closed");
    let toast = app.toast.clone().unwrap().0;
    assert_eq!(toast, "Cleaned up the scan on 1 page.");
    let doc = app.session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Enhance scanned file"));
    assert_eq!(doc.info.pages.len(), 2);
    assert_eq!(doc.comment_props(0, 0).map(|p| p.author), Some("Tester".to_string()), "the sticky note stays");
}

#[test]
fn nothing_to_enhance_is_said_not_done() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    h.state_mut().scan_clean_draft.settings = pdfcraft_engine::scan_clean::ScanCleanSettings { level: 0, edge_shadows: false, despeckle: false };
    assert!(h.state_mut().execute("ocr.enhance"));
    h.run_steps(2);
    // With every filter off the button is disabled; starting anyway changes nothing.
    h.state_mut().start_scan_clean();
    h.run_steps(2);
    let app = h.state();
    assert!(app.toast.clone().unwrap().0.starts_with("No scanned pages were cleaned up."), "{:?}", app.toast);
    assert_eq!(app.session.get(id).unwrap().can_undo(), None);
}
