//! Export a PDF ▸ Image and Text from the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn export_dialogs_write_images_and_text() {
    // A `TempDir` is unique per run and removes itself at the end, panic or not.
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app.export_dir_override = Some(d.to_string_lossy().into_owned());
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("export.image"));
    h.run_steps(2);
    h.get_by_label("Export to Image");
    h.get_by_label("Export").click();
    h.run_steps(4);
    assert!(dir.path().join("doc_page_1.png").exists() && dir.path().join("doc_page_2.png").exists());
    h.get_by_label_contains("Exported 2 images");
    assert!(h.state_mut().execute("export.text"));
    h.run_steps(2);
    h.get_by_label("Export").click();
    h.run_steps(4);
    assert!(dir.path().join("doc.txt").exists());
    // Export all images: this document has none, and says so.
    assert!(h.state_mut().execute("export.all_images"));
    h.run_steps(2);
    h.get_by_label("Export All Images");
    h.get_by_label("Export").click();
    h.run_steps(4);
    h.get_by_label_contains("Exported 0 images");
}

/// A two-page PDF whose pages carry distinct ink: page 1 paints red, page 2 blue
/// (content lengths computed, never counted by hand — the repo fixture style).
fn two_ink_pages() -> Vec<u8> {
    let body = |ops: &str| -> Vec<u8> {
        let mut o = format!("<< /Length {} >> stream\n", ops.len()).into_bytes();
        o.extend_from_slice(ops.as_bytes());
        o.extend_from_slice(b"\nendstream endobj");
        o
    };
    let mut out = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n".to_vec();
    out.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 100] >> endobj\n");
    out.extend_from_slice(b"3 0 obj << /Type /Page /Parent 2 0 R /Contents 5 0 R >> endobj\n");
    out.extend_from_slice(b"4 0 obj << /Type /Page /Parent 2 0 R /Contents 6 0 R >> endobj\n");
    out.extend_from_slice(b"5 0 obj ");
    out.extend_from_slice(&body("1 0 0 rg 10 10 50 50 re f"));
    out.push(b'\n');
    out.extend_from_slice(b"6 0 obj ");
    out.extend_from_slice(&body("0 0 1 rg 140 40 50 50 re f"));
    out.push(b'\n');
    out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
    out
}

/// The EPS export takes the page on screen — not every page (which the exporter refuses
/// for EPS) — while the PostScript export keeps taking the whole document: `doc.eps`
/// written from page 2 of a two-page document holds exactly page 2's ink and one
/// `%%Page` block; `doc.ps` from the same state holds both.
#[test]
fn eps_export_takes_the_current_page_and_ps_all_pages() {
    // Same isolation as above: unique per run, removed when the test ends.
    let dir = tempfile::tempdir().unwrap();
    let eps = dir.path().join("doc.eps");
    let ps = dir.path().join("doc.ps");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, two_ink_pages()).unwrap();
        app
    });
    h.run_steps(4);
    // Page 2 on screen (set after the load, which resets the view state).
    if let Some(view) = h.state_mut().views.first_mut() {
        view.current = 1;
    }
    h.state_mut().save_override = Some(eps.to_string_lossy().into_owned());
    assert!(h.state_mut().execute("export.ps"));
    h.run_steps(4);
    let eps = String::from_utf8_lossy(&std::fs::read(dir.path().join("doc.eps")).expect("the EPS was written")).into_owned();
    assert!(eps.starts_with("%!PS-Adobe-3.0 EPSF-3.0"), "EPSF header: {eps}");
    assert!(eps.contains("%%Pages: 1"), "one page: {eps}");
    assert!(eps.contains("0 0 1 setrgbcolor"), "page 2's blue ink: {eps}");
    assert!(!eps.contains("1 0 0 setrgbcolor"), "page 1's red ink stays out: {eps}");

    // The same state exported as .ps still takes every page.
    h.state_mut().save_override = Some(ps.to_string_lossy().into_owned());
    h.run_steps(2);
    assert!(h.state_mut().execute("export.ps"));
    h.run_steps(4);
    let ps = String::from_utf8_lossy(&std::fs::read(dir.path().join("doc.ps")).expect("the PS was written")).into_owned();
    assert!(ps.starts_with("%!PS-Adobe-3.0\n"), "PS header: {ps}");
    assert!(ps.contains("%%Pages: 2"), "both pages: {ps}");
    assert!(ps.contains("1 0 0 setrgbcolor") && ps.contains("0 0 1 setrgbcolor"), "both inks: {ps}");
}
