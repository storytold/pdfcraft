//! Create a PDF and Reduce File Size in the real shell.

use std::path::PathBuf;
use std::sync::Arc;

use crate::support::AppHarness as _;
use craft::host::{ManualJobs, Picked, ScriptedFiles};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_ui_egui::PdfCraftApp;

fn png() -> Vec<u8> {
    let img = craft_image_codecs::Image::from_u8(8, 4, craft_image_codecs::ChannelLayout::Rgb, vec![120u8; 8 * 4 * 3]).unwrap();
    craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap()
}

/// A harness whose app asks `files` for its picks and runs its background work when the test steps `jobs`.
fn scripted(
    files: Arc<ScriptedFiles>,
    jobs: Arc<ManualJobs>,
    size: egui::Vec2,
    setup: impl FnOnce(&mut PdfCraftApp) + 'static,
) -> Harness<'static, PdfCraftApp> {
    let host = crate::support::host_with(jobs, files);
    Harness::builder().with_size(size).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::test_app(host);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            setup(&mut app);
            app
        }
    })
}

#[test]
fn image_import_dialog_chooses_dpi_and_cancels() {
    use egui::{Key, Modifiers};
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.begin_image_import(&cc.egui_ctx, vec![("scan.png".into(), png())]);
            app
        }
    });
    h.run_steps(3);
    if let Ok(path) = std::env::var("PDFCRAFT_IMAGE_IMPORT_SHOT") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_label("Use 72 DPI (one point per pixel)").click();
    h.run_steps(1);
    h.get_by_label("Create").click();
    h.run_steps(3);
    assert!(h.state().image_import.is_none());
    let page = &h.state().session.docs()[0].info.pages[0];
    assert_eq!((page.width, page.height), (8.0, 4.0));
    let ctx = h.ctx.clone();
    h.state_mut().begin_image_import(&ctx, vec![("scan.png".into(), png())]);
    h.run_steps(2);
    h.get_by_label("Use custom DPI").click();
    h.run_steps(2);
    // The label and group also carry this name; type into the spin button.
    let field = h.get_by_role_and_label(Role::SpinButton, "DPI");
    field.focus();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_role_and_label(Role::SpinButton, "DPI").type_text("144");
    // Enter in the field is the dialog's default action, Create.
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(h.state().image_import.is_none(), "Enter creates the PDF");
    let page = &h.state().session.docs()[1].info.pages[0];
    assert_eq!((page.width, page.height), (4.0, 2.0));
    h.state_mut().begin_image_import(&ctx, vec![("scan.png".into(), png())]);
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().image_import.is_none());
    assert_eq!(h.state().session.docs().len(), 2);
}

#[test]
fn opening_images_and_text_converts_them_to_new_pdfs() {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app.open_bytes(&egui::Context::default(), "photo.png", Some("/tmp/photo.png".into()), png()).unwrap();
    app.open_bytes(&egui::Context::default(), "notes.txt", None, b"first line\nsecond line".to_vec()).unwrap();
    app.create_from_images(vec![("a.png".into(), png()), ("b.png".into(), png())]);
    assert!(app.execute(&egui::Context::default(), "create.blank"));
    let docs = app.session.docs();
    let summary: Vec<(String, usize, bool)> = docs.iter().map(|d| (d.name.clone(), d.info.pages.len(), d.dirty)).collect();
    assert_eq!(
        summary,
        [("photo.pdf".to_string(), 1, true), ("notes.pdf".into(), 1, true), ("Images.pdf".into(), 2, true), ("Untitled.pdf".into(), 1, true)]
    );
    assert!(docs[0].path.is_none(), "a converted file has no PDF path yet: Save asks where");
    assert!(app.open_bytes(&egui::Context::default(), "junk.png", None, b"\x89PNG\r\n\x1a\nnot really".to_vec()).is_err());
}

/// Open `name` and run Reduce File Size on it, saving to a place the script answers with.
fn reduce(name: &'static str, bytes: Vec<u8>) -> (Harness<'static, PdfCraftApp>, Arc<ScriptedFiles>, PathBuf) {
    let out = PathBuf::from("/reduced/reduced.pdf");
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![out.clone()])]));
    let jobs = Arc::new(ManualJobs::new());
    let mut h = scripted(files.clone(), jobs.clone(), egui::vec2(1200.0, 800.0), move |app| {
        // Keep metadata identical: changing ModDate can make a compact PDF one byte smaller.
        app.session = std::mem::take(&mut app.session).with_clock(|| 1_700_000_000);
        app.open_bytes(&egui::Context::default(), name, None, bytes).unwrap();
    });
    h.run_steps(2);
    assert!(h.execute("optimize.reduce"));
    h.run_steps(2);
    jobs.run_all();
    h.run_steps(3);
    assert!(h.state().optimize_run.is_none(), "the run finished");
    (h, files, out)
}

#[test]
fn reduce_file_size_writes_a_compact_copy() {
    // A photo-like picture, stored losslessly: Reduce recompresses it as JPEG.
    let (w, h) = (600u32, 400u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8 ^ (i & 7) as u8, (i / w * 255 / h) as u8, 128]).collect();
    let img = craft_image_codecs::Image::from_u8(w, h, craft_image_codecs::ChannelLayout::Rgb, px).unwrap();
    let photo = craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
    let (h, files, out) = reduce("photo.png", photo);
    let bytes = files.file(&out).expect("the copy is written");
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(bytes.len() < h.state().session.docs()[0].bytes.len(), "the copy is smaller");
    assert!(h.state().session.docs()[0].dirty, "the open document is unchanged");
    h.get_by_label_contains("% smaller");
}

#[test]
fn reduce_file_size_saves_nothing_when_the_file_is_already_small() {
    // A new text PDF is already compact: a copy would be no smaller (#490).
    let (h, files, out) = reduce("notes.txt", "lorem ipsum ".repeat(500).into_bytes());
    assert!(files.file(&out).is_none(), "no copy was written");
    h.get_by_label_contains("This file is already as small as it can be made");
}

#[test]
fn clipboard_images_and_text_become_new_pdfs() {
    use pdfcraft_ui_egui::Clip;
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
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
    let out = PathBuf::from("/optimized/optimized.pdf");
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![out.clone()])]));
    let jobs = Arc::new(ManualJobs::new());
    let mut h = scripted(files.clone(), jobs.clone(), egui::vec2(1200.0, 800.0), |app| {
        app.open_bytes(&egui::Context::default(), "notes.txt", None, "lorem ipsum ".repeat(500).into_bytes()).unwrap();
    });
    h.run_steps(4);
    assert!(h.execute("optimize.advanced"));
    h.run_steps(2);
    h.get_by_role_and_label(Role::Dialog, "PDF Optimizer");
    h.get_by_role_and_label(egui::accesskit::Role::Heading, "Color Images");
    h.get_by_role_and_label(Role::Heading, "Monochrome Images");
    // Monochrome Images ▸ Downsample to can be turned off.
    h.get_by_label("Downsample to").click();
    h.run_steps(2);
    h.get_by_label("Discard Objects").click();
    h.run_steps(2);
    h.get_by_label("Discard document tags").click();
    h.get_by_label("Discard User Data").click();
    h.run_steps(2);
    h.get_by_label("Discard document information and metadata").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    jobs.run_all();
    h.run_steps(4);
    assert!(files.file(&out).expect("the copy is saved").starts_with(b"%PDF-"));
    assert!(h.state().optimize_run.is_none(), "the run is over");
    assert!(h.query_by_role_and_label(Role::Dialog, "PDF Optimizer").is_none(), "the dialog closed");
}

#[test]
fn the_pdf_optimizer_dialog_picks_a_preset() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "notes.txt", None, b"lorem ipsum".to_vec()).unwrap();
            app
        }
    });
    h.run_steps(4);
    assert!(h.execute("optimize.advanced"));
    h.run_steps(2);
    h.get_by_value("Standard");
    assert!(h.query_all_by_value("150").next().is_some(), "Standard downsamples colour/gray images to 150 ppi");
    assert_eq!(h.get_by_label("Merge image fragments").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    h.get_by_value("Standard").click();
    h.run_steps(4);
    h.get_by_label("Mobile").click();
    h.run_steps(2);
    h.get_by_value("Mobile");
    assert!(h.query_all_by_value("96").next().is_some(), "Mobile downsamples colour/gray images to 96 ppi");
    // Changing an option makes the settings custom.
    h.get_by_label("Merge image fragments").click();
    h.run_steps(2);
    h.get_by_value("Custom");

    // Object streams need PDF 1.5 or later.
    h.get_by_label("Clean Up").click();
    h.run_steps(2);
    h.get_by_value("Retain existing").click();
    h.run_steps(4);
    h.get_by_label("PDF 1.4").click();
    h.run_steps(2);
    assert!(h.get_by_label("Compress document structure (object streams)").accesskit_node().is_disabled());
    h.get_by_value("PDF 1.4").click();
    h.run_steps(4);
    h.get_by_label("PDF 2.0").click();
    h.run_steps(2);
    assert!(!h.get_by_label("Compress document structure (object streams)").accesskit_node().is_disabled());
}

fn mixed_files(app: &PdfCraftApp) -> Vec<(String, Vec<u8>)> {
    let pdf = app.session.create_from_text("a", "from a pdf").unwrap();
    vec![
        ("notes.txt".into(), b"from text".to_vec()),
        ("report.docx".into(), b"PK\x03\x04".to_vec()),
        ("a.pdf".into(), pdf.to_vec()),
        ("photo.png".into(), png()),
    ]
}

#[test]
fn separate_batch_creates_one_pdf_per_source_without_replacing_existing_files() {
    use pdfcraft_ui_egui::FilePurpose;
    let dir = PathBuf::from("/batch");
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![dir.clone()])]).with_file(dir.join("notes.pdf"), b"keep this file".to_vec()));
    let jobs = Arc::new(ManualJobs::new());
    let mut h = scripted(files.clone(), jobs, egui::vec2(1000.0, 720.0), |_| {});
    h.run_steps(2);
    let mut sources = mixed_files(h.state());
    sources.push(("notes.txt".into(), b"another text file".to_vec()));
    let ctx_h = h.ctx.clone();
    h.state_mut().use_files(&ctx_h, FilePurpose::CreateMultipleSeparate, sources);
    h.run_steps(3);

    assert_eq!(files.file(&dir.join("notes.pdf")).unwrap(), b"keep this file");
    for name in ["notes (2).pdf", "notes (3).pdf", "photo.pdf"] {
        assert!(files.file(&dir.join(name)).unwrap_or_default().starts_with(b"%PDF-"), "{name}");
    }
    assert!(files.file(&dir.join("a (2).pdf")).is_none(), "an already-PDF input is skipped");
    assert!(files.file(&dir.join("report.pdf")).is_none(), "an invalid Office file is skipped");
    assert!(h.state().views.is_empty(), "batch conversion must not open PDF tabs");
}

#[test]
fn separate_batch_command_picks_files_and_writes_to_the_chosen_folder() {
    let source = PathBuf::from("/in/letter.txt");
    let dir = PathBuf::from("/out");
    let files = Arc::new(
        ScriptedFiles::answering([Picked::Paths(vec![source.clone()]), Picked::Paths(vec![dir.clone()])])
            .with_file(source, b"A separate PDF".to_vec()),
    );
    let jobs = Arc::new(ManualJobs::new());
    let mut h = scripted(files.clone(), jobs.clone(), egui::vec2(1000.0, 720.0), |_| {});
    assert!(h.execute("create.multiple_separate"));
    h.run_steps(4);
    jobs.run_all();
    h.run_steps(3);
    assert!(files.file(&dir.join("letter.pdf")).unwrap_or_default().starts_with(b"%PDF-"));
    assert!(h.state().views.is_empty());
}

#[test]
fn multiple_files_open_as_one_document_in_the_page_grid() {
    let path = PathBuf::from("/saved/combined.pdf");
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![path.clone()])]));
    let jobs = Arc::new(ManualJobs::new());
    let mut h = scripted(files.clone(), jobs, egui::vec2(1000.0, 720.0), |app| {
        let sources = mixed_files(app);
        app.use_files(&egui::Context::default(), pdfcraft_ui_egui::FilePurpose::CreateMultiple, sources);
    });
    h.run_steps(4);
    {
        let app = h.state();
        assert_eq!(app.views.len(), 1);
        assert!(app.views[0].organize, "the pages are shown as a grid");
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(doc.name, "Combined.pdf");
        assert!(doc.dirty && doc.path.is_none(), "unsaved until the user saves it");
        assert_eq!(doc.info.pages.len(), 3, "the Word file can't be converted and is left out");
        assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["notes", "a", "photo"]);
    }
    h.get_by_label_contains("left out: report.docx");
    h.get_by_label("Insert a file before page 2");
    if let Ok(path) = std::env::var("PDFCRAFT_CREATE_MULTIPLE_SHOT") {
        let _gpu = crate::gpu();
        h.run_steps(20);
        h.render().unwrap().save(path).unwrap();
    }
    // Remove the middle page and save what is left.
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.get_by_label("Delete pages (Delete)").click();
    h.run_steps(3);
    h.get_by_label("Save pages").click();
    h.run_steps(4);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert!(!doc.dirty && doc.info.pages.len() == 2);
    assert!(files.file(&path).unwrap_or_default().starts_with(b"%PDF-"));
}

#[test]
fn multiple_files_that_cannot_be_converted_open_nothing() {
    let files = Arc::new(ScriptedFiles::new());
    let jobs = Arc::new(ManualJobs::new());
    let mut h = scripted(files, jobs, egui::vec2(1000.0, 720.0), |_| {});
    h.run_steps(2);
    let ctx_h = h.ctx.clone();
    h.state_mut().use_files(&ctx_h, pdfcraft_ui_egui::FilePurpose::CreateMultiple, vec![("report.docx".into(), b"PK\x03\x04".to_vec())]);
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    // PLAN: assumed `support::shown_notice(&Harness<PdfCraftApp>) -> Option<String>` (pdf-chrome-w9)
    assert!(crate::support::shown_notice(&h).is_some(), "the user is told");
}

#[test]
fn multiple_files_past_a_thousand_are_all_used() {
    let mut app = crate::control::new_app();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    let pdf = app.session.create_from_text("a", "from a pdf").unwrap();
    let files: Vec<(String, Vec<u8>)> = (0..1001).map(|i| (format!("f{i}.pdf"), pdf.to_vec())).collect();
    app.use_files(&egui::Context::default(), pdfcraft_ui_egui::FilePurpose::CreateMultiple, files);
    assert_eq!(app.views.len(), 1);
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 1001, "every picked file is used, in order");
}

/// Create ▸ Blank page uses the system region's paper, not always US Letter (#5644): the app
/// starts with the system's, and a blank page is on A4 and on Letter as the paper says
/// (CodeRabbit on #5651: a Letter host alone would not tell a hard-coded Letter apart).
#[test]
fn a_blank_page_is_on_the_system_regions_paper() {
    use pdfcraft_ui_egui::paper::Paper;
    assert_eq!(crate::control::new_app().blank_paper, Paper::system());
    for paper in [Paper::A4, Paper::Letter] {
        let mut app = crate::control::new_app();
        app.blank_paper = paper;
        assert!(app.execute(&egui::Context::default(), "create.blank"));
        let docs = app.session.docs();
        let crop = docs.last().and_then(|d| d.info.pages.first()).map(|p| p.crop).unwrap();
        let (w, h) = paper.points();
        assert!(
            ((crop[2] - crop[0]) as f64 - w).abs() < 0.01 && ((crop[3] - crop[1]) as f64 - h).abs() < 0.01,
            "{paper:?}: {crop:?} is not {w} × {h}"
        );
    }
}
