//! Scan & OCR ▸ Recognize text in the real shell (egui_kittest): the dialog, the run and its
//! result (skipped without the OCR models: `cargo xtask models`).

use std::sync::Arc;

use crate::support::AppHarness as _;
use craft::host::{InlineJobs, Picked, ScriptedFiles};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::ocr::{OcrPage, OcrSettings, PlacedWord};
use pdfcraft_engine::{AddedText, Edit, Session, export};
use pdfcraft_ui_egui::PdfCraftApp;

/// The app on a host whose jobs run at once and whose pickers answer from `files`.
fn app_on(files: Arc<ScriptedFiles>) -> PdfCraftApp {
    crate::support::test_app(crate::support::host_with(Arc::new(InlineJobs), files))
}

/// Sets a field of the open dialog as the user editing that control would.
fn set_field(h: &mut Harness<'_, PdfCraftApp>, field: &str, value: serde_json::Value) {
    let ctx = h.ctx.clone();
    // PLAN: assumed `OpenDialog::id` and `DialogHost::open_dialogs`/`set_json_field` as listed in the framework's `DialogHost`.
    let id = h.state().dialogs.open_dialogs().first().expect("the dialog is open").id;
    h.state_mut().dialogs.set_json_field(&ctx, id, field, value).unwrap();
    h.run_steps(2);
}

/// Presses OK of the open Recognize Text dialog.
fn confirm_recognize(h: &mut Harness<'_, PdfCraftApp>) {
    let ctx = h.ctx.clone();
    assert!(h.state_mut().dialogs.confirm::<craft::ui::patterns::form_dialogs::HostedParamDialog>(&ctx), "OK was enabled");
    h.run_steps(3);
}

/// The text of the notice on screen.
fn notice(h: &Harness<'_, PdfCraftApp>) -> String {
    // PLAN: assumed `support::shown_notice(&Harness) -> Option<String>` (pdf-chrome-w9).
    crate::support::shown_notice(h).unwrap_or_default()
}

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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_on(Arc::new(ScriptedFiles::new()));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "scan.pdf", None, scan()).unwrap();
            app
        }
    });
    h.run_steps(4);
    assert!(h.execute("ocr.recognize"));
    h.run_steps(2);
    h.get_by_label("Document language");
    h.get_by_label("Output");
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    confirm_recognize(&mut h);
    h.run_steps(3);
    assert!(h.state().ocr_run.is_none(), "finished");
    let toast = notice(&h);
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
    // The folder pick is answered by the script; the copies are written through the host's files.
    let files = Arc::new(ScriptedFiles::answering([Picked::Paths(vec![dir.clone()])]));
    let service = files.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_on(service);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(2);
    let ctx_h = h.ctx.clone();
    h.state_mut().use_files(&ctx_h, pdfcraft_ui_egui::FilePurpose::Ocr, vec![("one.pdf".into(), scan()), ("two.pdf".into(), scan())]);
    h.run_steps(4);
    assert!(h.state().ocr_batch.is_none(), "finished");
    let toast = notice(&h);
    assert!(toast.starts_with("Recognized ") && toast.ends_with("in 2 files"), "{toast}");
    for name in ["one.pdf", "two.pdf"] {
        let bytes = files.file(&dir.join(name)).expect("the copy was written");
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_on(Arc::new(ScriptedFiles::new()));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "hybrid.pdf", None, hybrid).unwrap();
            app
        }
    });
    h.run_steps(4);
    assert!(h.execute("ocr.recognize"));
    h.run_steps(2);
    assert!(h.state().ocr_draft.settings().skip_text_pages, "pages with text are skipped by default");
    // The draft takes the dialog's values when it is confirmed.
    set_field(&mut h, "skip_text_pages", serde_json::json!(false));
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    confirm_recognize(&mut h);
    h.run_steps(3);
    assert!(!h.state().ocr_draft.settings().skip_text_pages, "the confirmed choice reached the draft");
    assert!(h.state().ocr_run.is_none(), "finished");
    let toast = notice(&h);
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "scan.pdf", None, scan()).unwrap();
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
            app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
            assert!(app.execute(&egui::Context::default(), "edit.edit_text"));
            app
        }
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
    let opened = |h: &Harness<'_, PdfCraftApp>| h.state().views[0].session.line_editor.as_ref().map(|e| e.text.clone());
    assert_eq!(opened(&h).as_deref(), Some("Recognize"));
    assert!(h.state().views[0].session.image_selection.is_none(), "the scan under the word isn't selected");
    h.hover_at(r.min + egui::vec2(300.0, 400.0) * k);
    h.run_steps(4);
    assert_eq!(opened(&h).as_deref(), Some("Recognize"), "moving the pointer away keeps the word open");
}

/// A scan of "Recognize these scanned words" with its OCR layer, open in Edit text at 100%, and the
/// words' boxes on the screen.
fn phrase_on_a_scan() -> (Harness<'static, PdfCraftApp>, Vec<egui::Rect>) {
    let mut s = Session::new();
    let text = s.create_from_text("t", "Recognize these scanned words").unwrap();
    let id = s.open("text.pdf", None, text, None).unwrap();
    let (source_width, source_height, ink) = {
        let d = s.get(id).unwrap();
        let line = d.text_lines(0).remove(0);
        (d.info.pages[0].width as f64, d.info.pages[0].height as f64, line.ink)
    };
    assert_eq!(ink.len(), 4, "a box per word");
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
    let scan = s.create_from_images(&[("scan.png".into(), png)]).unwrap().to_vec();
    let boxes = Arc::new(ink);
    let placed = boxes.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes(&egui::Context::default(), "scan.pdf", None, scan.clone()).unwrap();
        let id = app.views[0].id;
        let (w, h) = (app.session.get(id).unwrap().info.pages[0].width as f64, app.session.get(id).unwrap().info.pages[0].height as f64);
        let (kx, ky) = (w / source_width, h / source_height);
        let words = ["Recognize", "these", "scanned", "words"]
            .iter()
            .zip(placed.iter())
            .map(|(word, b)| PlacedWord {
                text: (*word).into(),
                origin: [b[0] * kx, b[1] * ky],
                across: [(b[2] - b[0]) * kx, 0.0],
                // Tight around the ink, as a recognised box is: the run's box has room for
                // ascenders and descenders any word may not have.
                up: [0.0, (b[3] - b[1]) * ky * 0.7],
                confidence: None,
                source: "ocrs".into(),
            })
            .collect();
        app.session.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
        app.set_option(&egui::Context::default(), "zoom", "100").unwrap();
        assert!(app.execute(&egui::Context::default(), "edit.edit_text"));
        app
    });
    crate::support::settle(&mut h);
    let screen = {
        let app = h.state();
        let d = app.session.get(app.views[0].id).unwrap();
        let block = d.text_blocks(0).remove(0);
        assert!(block.invisible);
        let r = app.views[0].page_screen_rect(0).unwrap();
        let page = &d.info.pages[0];
        let k = r.width() / page.width;
        d.text_lines(0)
            .iter()
            .flat_map(|line| line.ink.clone())
            .map(|b| {
                let top = r.min + egui::vec2(b[0] as f32, page.height - b[3] as f32) * k;
                let bottom = r.min + egui::vec2(b[2] as f32, page.height - b[1] as f32) * k;
                egui::Rect::from_min_max(top, bottom)
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(screen.len(), 4);
    (h, screen)
}

/// Click into the phrase and let the editor open.
fn open_phrase(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(4);
    assert!(h.state().views[0].session.line_editor.is_some(), "the phrase opened");
}

/// Apply the open phrase, as ⌘Enter does, and let the page render again.
fn apply(h: &mut Harness<'static, PdfCraftApp>) {
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    crate::support::settle(h);
}

/// The share of `area`'s pixels that differ visibly between two frames of the window.
fn changed(a: &image::RgbaImage, b: &image::RgbaImage, area: egui::Rect, ppp: f32) -> f64 {
    let (mut differ, mut all) = (0usize, 0usize);
    for y in (area.top() * ppp) as u32..(area.bottom() * ppp) as u32 {
        for x in (area.left() * ppp) as u32..(area.right() * ppp) as u32 {
            let (p, q) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
            differ += usize::from(p.iter().zip(q).any(|(u, v)| u.abs_diff(v) > 48));
            all += 1;
        }
    }
    differ as f64 / all.max(1) as f64
}

/// How much of `area` is dark, as ink is.
fn inked(image: &image::RgbaImage, area: egui::Rect, ppp: f32) -> f64 {
    let (mut dark, mut all) = (0usize, 0usize);
    for y in (area.top() * ppp) as u32..(area.bottom() * ppp) as u32 {
        for x in (area.left() * ppp) as u32..(area.right() * ppp) as u32 {
            dark += usize::from(image.get_pixel(x, y).0.iter().take(3).all(|c| *c < 120));
            all += 1;
        }
    }
    dark as f64 / all.max(1) as f64
}

fn snapshot_to(image: &image::RgbaImage, name: &str) {
    if let Some(dir) = std::env::var_os("PDFCRAFT_OCR_PREVIEW_DIR") {
        image.save(std::path::Path::new(&dir).join(name)).unwrap();
    }
}

/// Deleting a word while the phrase is open looks as it will once applied: that word is covered,
/// and the words around it are still the scan's pixels, not redrawn in a stand-in font (#3313).
#[test]
fn deleting_a_word_while_typing_leaves_the_scan_showing_around_it() {
    let (mut h, words) = phrase_on_a_scan();
    open_phrase(&mut h, words[1].center());
    let before = h.render().unwrap();
    snapshot_to(&before, "1-open.png");
    let ppp = h.ctx.pixels_per_point();
    assert!(inked(&before, words[3], ppp) > 0.05, "the last word is on the scan");

    h.key_press(egui::Key::End);
    for _ in 0.."words".len() {
        h.key_press(egui::Key::Backspace);
    }
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.line_editor.as_ref().unwrap().text.trim_end(), "Recognize these scanned");
    let typing = h.render().unwrap();
    snapshot_to(&typing, "2-deleted.png");
    assert!(inked(&typing, words[3], ppp) < 0.005, "the deleted word is covered");
    for (word, name) in words.iter().zip(["Recognize", "these", "scanned"]) {
        assert!(changed(&before, &typing, *word, ppp) < 0.02, "{name} is still the scan's pixels");
    }

    // Applied, the page looks the same.
    apply(&mut h);
    h.run_steps(4);
    let applied = h.render().unwrap();
    snapshot_to(&applied, "3-applied.png");
    let app = h.state();
    let blocks = app.session.get(app.views[0].id).unwrap().text_blocks(0);
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Recognize these scanned"]);
    assert!(blocks[0].invisible, "the kept words are still the layer's");
}

/// Backspacing over a word in the middle of the phrase leaves a space either side of it. The words
/// around it stay the scan's pixels, while typing and once applied.
#[test]
fn backspacing_over_a_middle_word_leaves_the_scan_showing_around_it() {
    let (mut h, words) = phrase_on_a_scan();
    open_phrase(&mut h, words[1].center());
    let before = h.render().unwrap();
    let ppp = h.ctx.pixels_per_point();
    let text = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].session.line_editor.as_ref().unwrap().text.clone();
    // To just after "scanned", then back over it.
    h.key_press(egui::Key::End);
    for _ in 0.." words".len() {
        h.key_press(egui::Key::ArrowLeft);
    }
    for _ in 0.."scanned".len() {
        h.key_press(egui::Key::Backspace);
    }
    h.run_steps(3);
    assert_eq!(text(&h), "Recognize these  words", "both spaces are still there");
    let typing = h.render().unwrap();
    snapshot_to(&typing, "5-backspaced.png");
    assert!(inked(&typing, words[2], ppp) < 0.005, "the deleted word is covered");
    for (word, name) in [(words[0], "Recognize"), (words[1], "these"), (words[3], "words")] {
        assert!(changed(&before, &typing, word, ppp) < 0.02, "{name} is still the scan's pixels while typing");
    }

    apply(&mut h);
    h.run_steps(4);
    let app = h.state();
    let blocks = app.session.get(app.views[0].id).unwrap().text_blocks(0);
    assert!(blocks.iter().all(|b| b.invisible), "nothing was redrawn: {:?}", blocks.iter().map(|b| &b.text).collect::<Vec<_>>());
    assert!(blocks.iter().all(|b| !b.text.contains("scanned")), "and the word is gone");
}

/// A space typed after the last word is no change to show or to apply: the line stays the scan,
/// while typing and after, with nothing to undo.
#[test]
fn a_space_after_the_last_word_leaves_the_scan_alone() {
    let (mut h, words) = phrase_on_a_scan();
    open_phrase(&mut h, words[1].center());
    let before = h.render().unwrap();
    let ppp = h.ctx.pixels_per_point();
    let bytes = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    h.key_press(egui::Key::End);
    h.event(egui::Event::Text(" ".into()));
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.line_editor.as_ref().unwrap().text, "Recognize these scanned words ");
    let typing = h.render().unwrap();
    for (word, name) in words.iter().zip(["Recognize", "these", "scanned", "words"]) {
        assert!(changed(&before, &typing, *word, ppp) < 0.02, "{name} is still the scan's pixels");
    }
    apply(&mut h);
    h.run_steps(4);
    let app = h.state();
    assert!(app.views[0].session.line_editor.is_none(), "the editor closed");
    assert_eq!(app.session.get(app.views[0].id).unwrap().bytes, bytes, "nothing was written");
}

/// A replaced word is drawn while typing, where Apply will draw it, and the rest stays the scan.
#[test]
fn replacing_a_word_while_typing_draws_only_that_word() {
    let (mut h, words) = phrase_on_a_scan();
    open_phrase(&mut h, words[1].center());
    let before = h.render().unwrap();
    let ppp = h.ctx.pixels_per_point();
    h.key_press(egui::Key::End);
    for _ in 0.."words".len() {
        h.key_press(egui::Key::Backspace);
    }
    h.event(egui::Event::Text("birds".into()));
    h.run_steps(3);
    assert_eq!(h.state().views[0].session.line_editor.as_ref().unwrap().text, "Recognize these scanned birds");
    let typing = h.render().unwrap();
    snapshot_to(&typing, "4-replaced.png");
    assert!(changed(&before, &typing, words[3], ppp) > 0.05, "the replaced word is different");
    assert!(inked(&typing, words[3], ppp) > 0.05, "and drawn in ink");
    for (word, name) in words.iter().zip(["Recognize", "these", "scanned"]) {
        assert!(changed(&before, &typing, *word, ppp) < 0.02, "{name} is still the scan's pixels");
    }
}

fn edit_harness(pdf: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = app_on(Arc::new(ScriptedFiles::new()));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, pdf).unwrap();
            app
        }
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
    assert!(h.execute("edit.edit_text"));
    h.run_steps(4);
    assert!(h.state().ocr_run.is_none(), "finished");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Recognize text"));
    // Undo takes the text layer away and Edit does not put it back.
    let ctx = h.ctx.clone();
    h.state_mut().undo(&ctx);
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
    h.set_option("tool", "edit").unwrap();
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
    assert!(h.execute("edit.edit_text"));
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
    assert!(h.execute("edit.edit_text"));
    h.run_steps(6);
    assert!(h.state().ocr_run.is_none());
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), None, "nothing was recognized or marked modified");
    assert!(!h.state().session.get(id).unwrap().dirty, "the file is not marked modified");
    // And Edit says nothing about recognizing: no progress, no "0 words ... already had text".
    let toast = notice(&h);
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
    assert!(h.execute("edit.edit_text"));
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
    assert!(h.execute("edit.edit_text"));
    h.set_option("mode", "read").unwrap();
    h.run_steps(5);
    assert_eq!(undo(&h), None, "not in Read mode");
    // A dialog over the window.
    h.set_option("mode", "edit").unwrap();
    assert!(h.execute("tools.js_console"));
    h.run_steps(5);
    assert_eq!(undo(&h), None, "not beneath a dialog");
    // Both gone: it is read now.
    h.key_press(egui::Key::Escape);
    h.run_steps(5);
    assert_eq!(undo(&h).as_deref(), Some("Recognize text"));
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.open_bytes(&egui::Context::default(), "scan.pdf", None, scan()).unwrap();
            app
        }
    });
    h.run_steps(4);
    assert!(h.execute("ocr.correct"));
    h.run_steps(3);
    // The screen's controls are all present (no dead controls).
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
    let toast = notice(&h);
    assert!(toast.contains("one undoable step"), "{toast}");
}

/// F8 with nothing to review is a no-op: the walk never invents a selection. The recognition is
/// loaded directly (no models needed).
#[test]
fn ocr_verify_screen_empty_walk() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.open_bytes(&egui::Context::default(), "scan.pdf", None, scan()).unwrap();
            app
        }
    });
    h.run_steps(4);
    assert!(h.execute("ocr.correct"));
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
}
