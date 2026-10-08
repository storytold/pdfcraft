//! Create a PDF and Reduce File Size in the real shell.

use pdfcraft_ui_egui::PdfCraftApp;

fn png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 8, 4);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[120u8; 8 * 4 * 3]).unwrap();
    }
    out
}

#[test]
fn opening_images_and_text_converts_them_to_new_pdfs() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("photo.png", Some("/tmp/photo.png".into()), png()).unwrap();
    app.open_bytes("notes.txt", None, b"first line\nsecond line".to_vec()).unwrap();
    app.create_from_images(vec![("a.png".into(), png()), ("b.png".into(), png())]);
    assert!(app.execute("create.blank"));
    let docs = app.session.docs();
    let summary: Vec<(String, usize, bool)> = docs.iter().map(|d| (d.name.clone(), d.info.pages.len(), d.dirty)).collect();
    assert_eq!(
        summary,
        [("photo.pdf".to_string(), 1, true), ("notes.pdf".into(), 1, true), ("Images.pdf".into(), 2, true), ("Untitled.pdf".into(), 1, true)]
    );
    assert!(docs[0].path.is_none(), "a converted file has no PDF path yet: Save asks where");
    assert!(app.open_bytes("junk.png", None, b"\x89PNG\r\n\x1a\nnot really".to_vec()).is_err());
}

#[test]
fn reduce_file_size_writes_a_compact_copy() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-reduce-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("reduced.pdf");
    let mut app = PdfCraftApp::new();
    app.open_bytes("notes.txt", None, "lorem ipsum ".repeat(500).into_bytes()).unwrap();
    app.save_override = Some(out.to_string_lossy().into_owned());
    assert!(app.execute("optimize.reduce"));
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(app.session.docs()[0].dirty, "the open document is unchanged");
}

#[test]
fn clipboard_images_and_text_become_new_pdfs() {
    use pdfcraft_ui_egui::Clip;
    let mut app = PdfCraftApp::new();
    app.create_from_clip(Clip::Image { width: 40, height: 20, rgba: [10u8, 20, 30, 255].repeat(40 * 20) }).unwrap();
    app.create_from_clip(Clip::Text("Pasted\nlines".into())).unwrap();
    let docs = app.session.docs();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0].name, "Clipboard.pdf");
    // The image page takes the image's size (one pixel per point at 72 dpi).
    let p = &docs[0].info.pages[0];
    assert!((p.width / p.height - 2.0).abs() < 0.01, "{} × {}", p.width, p.height);
    assert!(app.create_from_clip(Clip::Image { width: 4, height: 4, rgba: vec![0; 3] }).is_err(), "malformed images are refused");
}

#[test]
fn the_pdf_optimizer_dialog_saves_an_optimized_copy() {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    let dir = std::env::temp_dir().join(format!("pdfcraft-optimizer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("optimized.pdf");
    let out2 = out.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("notes.txt", None, "lorem ipsum ".repeat(500).into_bytes()).unwrap();
        app.save_override = Some(out2.to_string_lossy().into_owned());
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("optimize.advanced"));
    h.run_steps(2);
    h.get_by_label("PDF Optimizer");
    h.get_by_label("Color Images");
    h.get_by_label("Discard Objects").click();
    h.run_steps(2);
    h.get_by_label("Discard document tags").click();
    h.get_by_label("Discard User Data").click();
    h.run_steps(2);
    h.get_by_label("Discard document information and metadata").click();
    h.run_steps(1);
    {
        let d = &h.state().optimize_draft;
        assert!(d.settings.discard_tags && d.discard == vec![pdfcraft_engine::Hidden::Metadata]);
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF-"));
    assert_eq!(h.state().dialog, None);
}
