//! Scan & OCR ▸ Recognize text in the real shell (egui_kittest): the dialog, the run and its
//! result (skipped without the OCR models: `cargo xtask models`).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::ocr::{OcrPage, OcrSettings, PlacedWord};
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
    h.run_steps(2);
    h.get_by_label("Document language");
    h.get_by_label("Output");
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
        let words = vec![PlacedWord {
            text: "Recognize".into(),
            origin: [72.0, ph - 100.0],
            across: [80.0, 0.0],
            up: [0.0, 14.0],
            confidence: None,
            source: "ocrs".into(),
        }];
        app.session.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    crate::support::settle(&mut h);
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

/// A page of real text, not a picture.
fn text_page() -> Vec<u8> {
    let s = Session::new();
    s.create_from_text("t", "This page already has its own text").unwrap().to_vec()
}

fn edit_harness(pdf: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, pdf).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    h
}

/// Like Acrobat's "Converting scanned page to Editable Text and Images": turning Edit on for a
/// scanned page recognizes its words first (one undoable "Recognize text" step), once. Undoing it
/// is not undone again by the next frame.
#[test]
fn editing_a_scanned_page_recognizes_its_text_first_and_only_once() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut h = edit_harness(scan());
    let id = h.state().active_ids().unwrap().1;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None, "nothing is recognized before Edit");
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(4);
    assert!(h.state().ocr_run.is_none(), "finished");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Recognize text"));
    // Undo takes the text layer away and Edit does not put it back.
    h.state_mut().undo();
    h.run_steps(4);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None);
    h.run_steps(4);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None, "recognized only once per page");
}

/// The Edit tab (its tool panel) turns Edit on without picking Edit text & images, and Acrobat's
/// conversion happens there too.
#[test]
fn the_edit_panel_recognizes_a_scanned_page_too() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut h = edit_harness(scan());
    let id = h.state().active_ids().unwrap().1;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None);
    h.state_mut().set_option("tool", "edit").unwrap();
    h.run_steps(4);
    assert!(h.state().ocr_run.is_none(), "finished");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Recognize text"));
}

/// A scan with only a comment (a text box) over it is still a scan: Edit reads the picture. The
/// comment's own words are not read into the layer, and it counts as no page text.
#[test]
fn editing_a_scan_with_a_comment_over_it_recognizes_the_picture() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let with_comment = {
        let mut s = Session::new();
        let id = s.open("scan.pdf", None, std::sync::Arc::new(scan()), None).unwrap();
        let (w, h) = {
            let p = &s.get(id).unwrap().info.pages[0];
            (p.width as f64, p.height as f64)
        };
        let shape = pdfcraft_engine::Shape::TextBox { rect: [w * 0.1, h * 0.75, w * 0.9, h * 0.85], font_size: 18.0 };
        let style = pdfcraft_engine::Style::default_for(&shape);
        let note = pdfcraft_engine::NewAnnotation { page: 0, shape, style, contents: "Invoice reference 4711".into(), author: "Test".into() };
        s.apply(id, Edit::AddAnnotation(note)).unwrap();
        s.save_bytes(id).unwrap().to_vec()
    };
    let mut h = edit_harness(with_comment);
    let id = h.state().active_ids().unwrap().1;
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(4);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Recognize text"));
}

/// A born-digital page with a full-page background picture and real text (a flyer, a slide
/// export) must not change just because Edit is turned on (maintainer review of #1609).
#[test]
fn editing_a_page_with_text_over_a_background_picture_changes_nothing() {
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
    let mut h = edit_harness(hybrid);
    let id = h.state().active_ids().unwrap().1;
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(6);
    assert!(h.state().ocr_run.is_none());
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None, "nothing was recognized or marked modified");
    assert!(!h.state().session.get(id).unwrap().dirty, "the file is not marked modified");
    // And Edit says nothing about recognizing: no progress, no "0 words ... already had text".
    let toast = h.state().toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(!toast.contains("ecogniz") && !toast.contains("already had text"), "{toast:?}");
}

/// A page that is not a scan when Edit looks at it is looked at again after the document changes:
/// adding a picture of text that covers it turns it into a scan, and Edit then reads it (and does
/// so once).
#[test]
fn a_page_that_becomes_a_scan_while_edit_is_on_is_recognized() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let png = {
        let mut s = Session::new();
        let text = s.create_from_text("t", "Recognize these scanned words").unwrap();
        let id = s.open("text.pdf", None, text, None).unwrap();
        export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap()
    };
    let blank = Session::new().create_blank(595.0, 842.0, 1).unwrap().to_vec();
    let mut h = edit_harness(blank);
    let id = h.state().active_ids().unwrap().1;
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(4);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None, "a blank page is no scan: nothing happened");
    // The same page, now with a picture of text over all of it.
    h.state_mut()
        .session
        .apply(id, Edit::AddImage { page: 0, rect: Some([0.0, 0.0, 595.0, 842.0]), name: "scan.png".into(), bytes: std::sync::Arc::new(png) })
        .unwrap();
    h.run_steps(6);
    assert!(h.state().ocr_run.is_none(), "finished");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Recognize text"));
}

/// Edit changes the document only while the user is editing: not in Read mode (which leaves the Edit
/// tool picked), and not beneath a dialog. The page is recognized once those are gone.
#[test]
fn edit_recognizes_a_scan_only_outside_read_mode_and_beneath_no_dialog() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let mut h = edit_harness(scan());
    let id = h.state().active_ids().unwrap().1;
    let undo = |h: &Harness<'static, PdfCraftApp>| -> Option<String> { h.state().session.get(id).unwrap().can_undo().map(String::from) };
    // Read mode with the Edit tool still picked.
    assert!(h.state_mut().execute("edit.edit_text"));
    h.state_mut().set_option("mode", "read").unwrap();
    h.run_steps(5);
    assert_eq!(undo(&h), None, "not in Read mode");
    // A dialog over the window.
    h.state_mut().set_option("mode", "edit").unwrap();
    h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::JsConsole);
    h.run_steps(5);
    assert_eq!(undo(&h), None, "not beneath a dialog");
    // Both gone: it is read now.
    h.state_mut().dialog = None;
    h.run_steps(5);
    assert_eq!(undo(&h).as_deref(), Some("Recognize text"));
}

#[test]
fn editing_a_page_that_has_text_does_not_recognize_anything() {
    let mut h = edit_harness(text_page());
    let id = h.state().active_ids().unwrap().1;
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(6);
    assert!(h.state().ocr_run.is_none());
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None);
}

/// Only the Edit tool recognizes: viewing a scan, or any other tool, leaves the file as it is.
#[test]
fn only_the_edit_tool_recognizes_a_scanned_page() {
    let mut h = edit_harness(scan());
    let id = h.state().active_ids().unwrap().1;
    h.run_steps(6);
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None);
}

/// A placed word the review can hold (its box sits where a real one would).
fn placed(text: &str, confidence: f32) -> PlacedWord {
    PlacedWord { text: text.into(), origin: [40.0, 40.0], across: [50.0, 0.0], up: [0.0, 12.0], confidence: Some(confidence), source: "ocrs".into() }
}

/// The OCR Verify screen (Scan & OCR ▸ Correct recognized text) in the real shell: every
/// control is live, the keyboard walks the uncertain words, the inspector removes and restores
/// through clicks, and Accept applies the review as one undoable step. The recognition is
/// loaded directly, so this runs without the OCR models.
#[test]
fn ocr_verify_screen_controls_keyboard_and_accept() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ocr.correct"));
    h.run_steps(3);
    // The screen owns the window and its controls are all present (no dead controls).
    h.get_by_label("Run OCR");
    h.get_by_label("Reject");
    h.get_by_label("Next uncertain");
    h.get_by_label("Previous uncertain");
    h.get_by_label("Page verified");
    h.get_by_label("Exit");
    use pdfcraft_ui_egui::Phase;
    assert_eq!(h.state().ocr_verify.phase, Phase::Idle);
    // The guards hold in the real UI: Reject outside ReviewReady is ignored, not acted on.
    h.get_by_label("Reject").click();
    h.run_steps(2);
    assert_eq!(h.state().ocr_verify.phase, Phase::Idle);
    // A recognition result, as the worker would deliver it.
    {
        let app = h.state_mut();
        let id = app.active_ids().unwrap().1;
        let job = app.session.ocr_job(id, &[0], OcrSettings::default()).unwrap();
        let page =
            OcrPage { already_on_page: 0, page: 0, words: vec![placed("Helo", 40.0), placed("wrold", 95.0)], skipped: None, notes: Vec::new() };
        app.ocr_verify.load(id, job.guard.clone(), vec![page]);
    }
    h.run_steps(3);
    assert_eq!(h.state().ocr_verify.phase, Phase::ReviewReady);
    // F8 walks to the Low-band word; the inspector shows it.
    h.key_press(egui::Key::F8);
    h.run_steps(2);
    assert_eq!(h.state().ocr_verify.selected, Some((0, 0)));
    h.get_by_label("Remove").click();
    h.run_steps(2);
    assert!(h.state().ocr_verify.is_removed((0, 0)), "Remove strikes the word out");
    h.get_by_label("Restore").click();
    h.run_steps(2);
    assert!(!h.state().ocr_verify.is_removed((0, 0)), "Restore brings it back");
    // Ctrl+T marks the page reviewed (the checkbox flips).
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::T);
    h.run_steps(2);
    assert!(h.state().ocr_verify.review.verified_pages.contains(&0), "Ctrl+T marks the page verified");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::T);
    h.run_steps(2);
    assert!(!h.state().ocr_verify.review.verified_pages.contains(&0));
    // Accept applies the whole review as ONE undoable step on the open document.
    h.get_by_label("Accept").click();
    h.run_steps(3);
    assert_eq!(h.state().ocr_verify.phase, Phase::Idle, "a successful accept ends the review");
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Recognize text"));
    let toast = app.toast.clone().map(|t| t.0).unwrap_or_default();
    assert!(toast.contains("one undoable step"), "{toast}");
}

/// Ctrl+Tab moves the pane focus ring, and F8 with nothing to review is a no-op: the walk
/// never invents a selection. The recognition is loaded directly (no models needed).
#[test]
fn ocr_verify_screen_focus_ring_and_empty_walk() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ocr.correct"));
    h.run_steps(3);
    {
        let app = h.state_mut();
        let id = app.active_ids().unwrap().1;
        let job = app.session.ocr_job(id, &[0], OcrSettings::default()).unwrap();
        let page = OcrPage { already_on_page: 0, page: 0, words: vec![placed("Fine", 95.0)], skipped: None, notes: Vec::new() };
        app.ocr_verify.load(id, job.guard.clone(), vec![page]);
    }
    h.run_steps(3);
    // Nothing asks for review: F8 selects nothing instead of wrapping into an error.
    h.key_press(egui::Key::F8);
    h.run_steps(2);
    assert_eq!(h.state().ocr_verify.selected, None);
    // Ctrl+Tab cycles the four panes; four steps come back to the start.
    let start = h.state().ocr_verify.focus;
    let mut moved = false;
    for _ in 0..4 {
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Tab);
        h.run_steps(2);
        moved |= h.state().ocr_verify.focus != start;
    }
    assert!(moved, "Ctrl+Tab moves the focus");
    assert_eq!(h.state().ocr_verify.focus, start, "the ring returns after the four panes");
}
