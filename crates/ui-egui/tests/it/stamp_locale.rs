//! The canvas passes the effective interface/date language when it places a dynamic stamp.

use std::sync::Arc;

use egui_kittest::Harness;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

fn assert_saved_by_line(bytes: Arc<Vec<u8>>, expected: &str) {
    let pdf = pdfcraft_cos::Document::open(bytes).unwrap();
    let page = pdfcraft_model::pages(&pdf).remove(0);
    let annotations = pdf.resolve(page.dict.get(b"Annots").unwrap());
    assert_eq!(annotations.as_array().unwrap().len(), 1);
    let annotation = pdf.resolve(annotations.as_array().unwrap().first().unwrap());
    let dict = annotation.as_dict().unwrap();
    assert_eq!(dict.get(b"PCByLine").unwrap().as_string().unwrap().to_text(), expected);
    // The drawn appearance must contain the full accented text, not only the private metadata.
    let appearance = pdf.resolve(dict.get(b"AP").unwrap());
    let normal = pdf.resolve(appearance.as_dict().unwrap().get(b"N").unwrap());
    let pdfcraft_cos::Object::Stream(stream) = normal.as_ref() else { panic!("stamp has an appearance stream") };
    let contents = stream.decoded().unwrap();
    let text = pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(expected));
    assert!(contents.windows(text.len()).any(|bytes| bytes == text.as_slice()));
}

#[test]
fn placing_dynamic_stamps_follows_interface_language_and_explicit_date_language() {
    for (language, date_language, expected) in [
        ("pt-br", "auto", "Por Ana às 21:30, 14/10/2026"),
        ("pt-br", "en", "By Ana at 9:30 pm, Oct 14, 2026"),
        ("en", "pt-br", "Por Ana às 21:30, 14/10/2026"),
    ] {
        let mut harness = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.session = std::mem::take(&mut app.session).with_wall_clock(|| 1_792_024_200, || -3 * 3600);
            app.set_option("language", language).unwrap();
            app.set_option("date-language", date_language).unwrap();
            app.comment_prefs.author = "Ana".into();
            let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
            app.open_bytes("synthetic-stamp.pdf", None, bytes.as_ref().clone()).unwrap();
            app.set_option("zoom", "100").unwrap();
            app
        });
        harness.run_steps(6);
        harness.state_mut().quick_tool = QuickTool::Stamp(pdfcraft_engine::StampKind::DynApproved);
        let page = harness.state().views[0].page_screen_rect(0).unwrap();
        let at = page.min + egui::vec2(page.width() * 0.5, page.height() * 0.65);
        harness.hover_at(at);
        harness.run_steps(1);
        harness.drag_at(at);
        harness.run_steps(1);
        harness.drop_at(at);
        harness.run_steps(4);
        let app = harness.state();
        assert_eq!(app.quick_tool, QuickTool::Select);
        let doc = app.views[0].id;
        assert_saved_by_line(app.session.save_bytes(doc).unwrap(), expected);
    }
}
