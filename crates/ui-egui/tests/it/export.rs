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
    let dir = std::env::temp_dir().join(format!("pdfcraft-export-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.clone();
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
    assert!(dir.join("doc_page_1.png").exists() && dir.join("doc_page_2.png").exists());
    h.get_by_label_contains("Exported 2 images");
    assert!(h.state_mut().execute("export.text"));
    h.run_steps(2);
    h.get_by_label("Export").click();
    h.run_steps(4);
    assert!(dir.join("doc.txt").exists());
    // Export all images: this document has none, and says so.
    assert!(h.state_mut().execute("export.all_images"));
    h.run_steps(2);
    h.get_by_label("Export All Images");
    h.get_by_label("Export").click();
    h.run_steps(4);
    h.get_by_label_contains("Exported 0 images");
}

/// Drawn as header, body, heading; tagged as heading then body, with the header an artifact.
fn tagged_fixture() -> Vec<u8> {
    let content = "/Artifact BMC BT /F1 12 Tf 20 180 Td (Running header) Tj ET EMC \
                   /P << /MCID 0 >> BDC BT /F1 12 Tf 20 150 Td (Body text) Tj ET EMC \
                   /H1 << /MCID 1 >> BDC BT /F1 12 Tf 20 120 Td (Heading) Tj ET EMC";
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /StructTreeRoot /K 6 0 R >> endobj
6 0 obj << /S /Document /Pg 3 0 R /K [<< /S /H1 /K 1 >> << /S /P /K 0 >>] >> endobj
7 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes()
}

#[test]
fn export_text_from_tags_writes_tag_order_and_refuses_untagged_files() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-export-tagged-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let run = |name: &'static str, bytes: Vec<u8>| {
        let d = dir.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes(name, None, bytes.clone()).unwrap();
            app.export_dir_override = Some(d.to_string_lossy().into_owned());
            app
        });
        h.run_steps(4);
        assert!(h.state_mut().execute("export.tagged_text"));
        h.run_steps(2);
        h.get_by_label("Export Text from Tags");
        // Tag order doesn't follow pages: there is no page range to choose.
        assert!(h.query_by_label("Pages").is_none());
        h.get_by_label("Export").click();
        h.run_steps(4);
        h
    };
    let h = run("tagged.pdf", tagged_fixture());
    h.get_by_label_contains("Exported the text of the document's tags");
    let text = std::fs::read_to_string(dir.join("tagged.txt")).unwrap();
    let (heading, body) = (text.find("Heading").expect("the heading is exported"), text.find("Body").expect("the body is exported"));
    assert!(heading < body && !text.contains("Running"), "{text:?}");
    drop(h);
    let h = run("plain.pdf", FIXTURE.to_vec());
    h.get_by_label_contains("the document has no tags");
    assert!(!dir.join("plain.txt").exists(), "nothing is written for an untagged file");
}
