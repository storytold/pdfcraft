//! Render the Print dialog with a synthetic ARCH D sheet, without accessing printers.
//! `cargo run -p pdfcraft-ui-egui --example print_dialog -- output.png`
use egui_kittest::Harness;
use pdfcraft_engine::print::{self, SizeMode};
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn main() -> Result<(), String> {
    let out = std::env::args().nth(1).ok_or("usage: print_dialog <output.png>")?;
    let mut app = PdfCraftApp::new();
    let bytes = app.session.create_blank(2592.0, 1728.0, 1).map_err(|e| e.to_string())?;
    app.open_bytes("Synthetic ARCH D.pdf", None, bytes.as_ref().clone()).map_err(|e| e.to_string())?;
    app.print_draft.paper = print::matching_paper((2592.0, 1728.0)).ok_or("ARCH D preset is missing")?;
    app.print_draft.size = SizeMode::Actual;
    app.dialog = Some(Dialog::Print);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(move |_cc| app);
    h.run_steps(8);
    h.render()?.save(out).map_err(|e| e.to_string())
}
