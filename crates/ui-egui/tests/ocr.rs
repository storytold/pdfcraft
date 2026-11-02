//! Scan & OCR ▸ Recognize text in the real shell (egui_kittest): the dialog, the run and its
//! result (skipped without the OCR models: `cargo xtask models`).

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::{AddedText, Edit, Session, export};
use pdfcraft_ui_egui::PdfCraftApp;

/// A one-page PDF that is only a picture of a sentence.
fn scan() -> Vec<u8> {
    let mut s = Session::new();
    let text = s.create_from_text("t", "Recognize these scanned words").unwrap();
    let id = s.open("text.pdf", None, text, None).unwrap();
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
    s.create_from_images(&[("scan.png".into(), png)]).unwrap().to_vec()
}

#[test]
fn recognize_text_dialog_adds_searchable_text() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ocr.recognize"));
    // The scrollable dialog settles its size before pointer coordinates are read.
    h.run_steps(10);
    h.get_by_label("Document language");
    h.get_by_label("Output");
    h.get_by_label("Automatically rotate sideways and upside-down pages");
    h.get_by_label("Straighten skewed scans");
    assert!(h.state().ocr_draft.auto_rotate && h.state().ocr_draft.deskew);
    let expected = pdfcraft_engine::ocr::language_for_locale(sys_locale::get_locale().as_deref());
    assert_eq!(h.state().ocr_draft.language, expected, "OCR follows the system even when the interface is English");
    let label = pdfcraft_engine::ocr::LANGUAGES.iter().find(|(code, _)| *code == expected).unwrap().1;
    h.get_by_value(label).click();
    h.run_steps(2);
    for label in ["Nederlands", "Dansk", "Norsk bokmål", "Svenska", "Suomi", "Português", "Català", "繁體中文", "简体中文"] {
        h.get_by_label(label);
    }
    // The expanded language list scrolls when the popup reaches the window edge.
    h.get_by_label("简体中文").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("简体中文").click();
    h.run_steps(2);
    assert_eq!(h.state().ocr_draft.language, "zh-Hans", "the last language in the list can be chosen");
    h.get_by_value("简体中文").click();
    h.run_steps(2);
    h.get_by_label("Català").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("Català").click();
    h.run_steps(2);
    assert_eq!(h.state().ocr_draft.language, "ca");
    h.get_by_value("Català").click();
    h.run_steps(2);
    h.get_by_label("Italiano").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("Italiano").click();
    h.run_steps(2);
    assert_eq!(h.state().ocr_draft.language, "it");
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    h.get_by_label("Recognize text").click();
    h.run_steps(3);
    assert!(h.state().ocr_run.is_none(), "finished");
    let toast = h.state().toast.clone().unwrap().0;
    assert!(toast.starts_with("Recognized ") && toast.contains("on 1 page"), "{toast}");
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Recognize text"));
}

#[test]
fn historical_ocr_choice_requires_an_explicit_addon_download() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    h.state_mut().execute("ocr.recognize");
    h.run_steps(10);
    let language = h.state().ocr_draft.language.clone();
    let status = h.state_mut().ocr_addon.status.as_mut().unwrap();
    status.installed = false;
    status.supported = true;
    h.get_by_value("Modern print").click();
    h.run_steps(10);
    h.get_by_label("Historical print (Fraktur)").click();
    h.run_steps(10);
    assert_eq!(h.state().ocr_draft.recognizer, pdfcraft_engine::ocr::Recognizer::Historical);
    assert_eq!(h.state().ocr_draft.language, language, "style selection keeps the document language");
    h.get_by_label("Automatic (historical model)");
    h.get_by_label("Download historical OCR add-on (13 MB)");
    assert!(h.get_by_label("Recognize text").accesskit_node().is_disabled());
    assert!(h.state().ocr_run.is_none(), "choosing a style starts neither recognition nor download");
    h.get_by_value("Historical print (Fraktur)").click();
    h.run_steps(10);
    h.get_by_label("Historical handwriting (Kurrent)").click();
    h.run_steps(10);
    assert_eq!(h.state().ocr_draft.recognizer, pdfcraft_engine::ocr::Recognizer::HistoricalHandwriting);
    h.get_by_label("Handwriting requires upright horizontal lines in one text column. Columns, tables and marginal notes are not supported.");
    assert!(h.get_by_label("Recognize text").accesskit_node().is_disabled());
    h.get_by_value("Historical handwriting (Kurrent)").click();
    h.run_steps(10);
    h.get_by_label("Modern print").click();
    h.run_steps(10);
    assert_eq!(h.state().ocr_draft.recognizer, pdfcraft_engine::ocr::Recognizer::Modern);
}

#[test]
fn installed_historical_ocr_runs_from_the_dialog() {
    if !pdfcraft_engine::ocr::available() || !pdfcraft_engine::ocr_addons::status().installed {
        eprintln!("skipped: standard OCR and optional historical models are required");
        return;
    }
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app.ocr_draft.recognizer = pdfcraft_engine::ocr::Recognizer::Historical;
        app.ocr_draft.auto_rotate = false;
        app.ocr_draft.deskew = false;
        app
    });
    h.run_steps(4);
    h.state_mut().execute("ocr.recognize");
    h.run_steps(10);
    h.get_by_label("Historical OCR add-on installed");
    h.get_by_label("Remove add-on");
    assert!(!h.get_by_label("Recognize text").accesskit_node().is_disabled());
    h.get_by_label("Recognize text").click();
    h.run_steps(3);
    assert!(h.state().ocr_run.is_none());
    let toast = h.state().toast.clone().unwrap().0;
    assert!(toast.starts_with("Recognized ") && !toast.starts_with("Recognized 0 "), "{toast}");
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Recognize text"));
}

/// Opt-in network smoke test. The caller must provide a fresh, isolated add-on
/// root; the ordinary test suite never downloads or removes installed resources.
#[test]
#[ignore = "explicit network test; set PDFCRAFT_OCR_ADDONS to a fresh isolated directory"]
fn historical_ocr_download_and_remove_buttons_manage_the_real_addon() {
    let root = std::env::var_os("PDFCRAFT_OCR_ADDONS").expect("an explicit isolated add-on root is required");
    let directory = std::path::Path::new(&root).join("historical-v1");
    assert!(directory.is_absolute() && !directory.exists(), "the test must not touch an existing add-on");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app.ocr_draft.recognizer = pdfcraft_engine::ocr::Recognizer::Historical;
        app
    });
    h.run_steps(4);
    h.state_mut().execute("ocr.recognize");
    h.run_steps(10);
    h.get_by_label("Download historical OCR add-on (13 MB)").click();
    h.run_steps(10);
    assert!(h.state().ocr_addon.status.as_ref().unwrap().installed, "{:?}", h.state().ocr_addon.message);
    h.get_by_label("Historical OCR add-on installed");
    h.get_by_label("Remove add-on").click();
    h.run_steps(10);
    assert!(!h.state().ocr_addon.status.as_ref().unwrap().installed);
    assert!(!directory.exists());
}

#[test]
fn scan_settings_preserve_corrections_and_existing_text_choice() {
    let mut draft = PdfCraftApp::new().ocr_draft;
    assert!(draft.settings().skip_text_pages);
    draft.text_pages = true;
    draft.language = "fr".into();
    draft.auto_rotate = false;
    draft.deskew = false;
    draft.auto_crop = true;
    draft.crop_margin = 7.0;
    let settings = draft.settings();
    assert!(!settings.skip_text_pages);
    assert_eq!(settings.language, "fr");
    assert!(!settings.auto_rotate && !settings.deskew);
    assert!(settings.auto_crop);
    assert_eq!(settings.crop_margin, 7.0);
    draft.text_pages = false;
    draft.recognize = false;
    assert!(!draft.settings().skip_text_pages, "enhancement retains existing OCR pages");
    assert!(!draft.settings().recognize);
}

#[test]
fn enhance_scan_dialog_offers_correction_without_recognition() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ocr.enhance"));
    h.run_steps(10);
    h.get_by_label("Correct scans");
    assert!(!h.state().ocr_draft.recognize);
    assert!(h.state().ocr_draft.auto_crop);
    assert_eq!(h.state().ocr_draft.language, pdfcraft_engine::ocr::language_for_locale(sys_locale::get_locale().as_deref()));
    h.get_by_label("Safety margin (pt)");
    h.get_by_label("Crop scanner borders to the paper edge").click();
    h.run_steps(2);
    assert!(!h.state().ocr_draft.auto_crop);
    h.get_by_label("Add searchable text").click();
    h.run_steps(2);
    assert!(h.state().ocr_draft.recognize);
}

#[test]
fn recognize_text_in_multiple_files_writes_searchable_copies() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.run_inline = true;
    app.export_dir_override = Some(dir.to_string_lossy().into_owned());
    app.use_files(pdfcraft_ui_egui::FilePurpose::Ocr, vec![("one.pdf".into(), scan()), ("two.pdf".into(), scan())]);
    assert!(app.ocr_batch.is_none(), "finished");
    let toast = app.toast.clone().unwrap().0;
    assert!(toast.starts_with("Recognized ") && toast.ends_with("in 2 files"), "{toast}");
    for name in ["one.pdf", "two.pdf"] {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        assert!(bytes.len() > scan().len(), "an incremental update was appended");
    }
}

/// #1028: "Also recognize pages that already have text" reads a picture on a page that has its own
/// text, without adding that text a second time.
#[test]
fn recognize_text_can_read_pictures_on_pages_with_text() {
    let hybrid = {
        let mut s = Session::new();
        let id = s.open("scan.pdf", None, std::sync::Arc::new(scan()), None).unwrap();
        let (w, h) = {
            let p = &s.get(id).unwrap().info.pages[0];
            (p.width as f64, p.height as f64)
        };
        let note =
            AddedText { rect: [w * 0.1, h * 0.75, w * 0.9, h * 0.85], text: "Invoice reference 4711".into(), size: 18.0, ..Default::default() };
        s.apply(id, Edit::AddText { page: 0, text: note }).unwrap();
        s.save_bytes(id).unwrap().to_vec()
    };
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("hybrid.pdf", None, hybrid).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ocr.recognize"));
    h.run_steps(2);
    assert!(h.state().ocr_draft.settings().skip_text_pages, "pages with text are skipped by default");
    h.get_by_label("Also recognize pages that already have text").click();
    h.run_steps(2);
    assert!(!h.state().ocr_draft.settings().skip_text_pages);
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    h.get_by_label("Recognize text").click();
    h.run_steps(3);
    assert!(h.state().ocr_run.is_none(), "finished");
    let toast = h.state().toast.clone().unwrap().0;
    assert!(toast.starts_with("Recognized ") && toast.ends_with("on 1 page"), "{toast}");
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    let doc = app.session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Recognize text"));
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
    let text = r.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
    let text = text.text.unwrap().plain_text().to_lowercase();
    assert_eq!((text.matches("invoice").count(), text.matches("scanned").count()), (1, 1), "{text}");
}

/// Edit text & images on a scanned page: an OCR word (invisible text over the scan) opens for
/// editing on a click, stays open when the pointer leaves it, and its editor hides the scanned
/// word instead of showing it through.
#[test]
fn an_ocr_word_opens_for_editing_and_stays_open() {
    use pdfcraft_engine::ocr::PlacedWord;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        let id = app.views[0].id;
        let ph = app.session.get(id).unwrap().info.pages[0].height as f64;
        let words = vec![PlacedWord { text: "Recognize".into(), origin: [72.0, ph - 100.0], across: [80.0, 0.0], up: [0.0, 14.0] }];
        app.session.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (r, k) = {
        let app = h.state();
        let d = app.session.get(app.views[0].id).unwrap();
        assert!(d.text_blocks(0).first().is_some_and(|b| b.invisible));
        let r = app.views[0].page_screen_rect(0).unwrap();
        (r, r.width() / d.info.pages[0].width)
    };
    // The word, 100 pt from the top of the page.
    let p = r.min + egui::vec2(100.0, 93.0) * k;
    h.hover_at(p);
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(4);
    let opened = |h: &Harness<'_, PdfCraftApp>| h.state().views[0].line_editor.as_ref().map(|e| e.text.clone());
    assert_eq!(opened(&h).as_deref(), Some("Recognize"));
    assert!(h.state().views[0].image_selection.is_none(), "the scan under the word isn't selected");
    h.hover_at(r.min + egui::vec2(300.0, 400.0) * k);
    h.run_steps(4);
    assert_eq!(opened(&h).as_deref(), Some("Recognize"), "moving the pointer away keeps the word open");
}
