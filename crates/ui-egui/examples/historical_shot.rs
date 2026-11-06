//! Inspect the optional OCR add-on dialog without opening a native window.
//! `cargo run -p pdfcraft-ui-egui --example historical_shot -- out.png [en|de] [historical|historical-handwriting]`
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let output = args.next().ok_or("expected screenshot path")?;
    let language = args.next().unwrap_or_else(|| "en".into());
    let recognizer = pdfcraft_engine::ocr::RecognizerChoice::from_id(&args.next().unwrap_or_else(|| "historical".into()))?;
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        let _ = app.set_option("language", &language);
        if let Ok(bytes) = app.session.create_from_text("Example", "Historical OCR add-on") {
            let _ = app.open_bytes("example.pdf", None, bytes.to_vec());
        }
        app.run_inline = true;
        app.ocr_draft.recognizer = recognizer;
        app
    });
    harness.run_steps(4);
    harness.state_mut().execute("ocr.recognize");
    harness.run_steps(20);
    harness.render()?.save(output).map_err(|error| error.to_string())
}
