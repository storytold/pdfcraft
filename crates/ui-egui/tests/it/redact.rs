//! Redact a PDF in the real shell (egui_kittest): the Redact tool, Redact pages, the apply
//! confirmation and Clear all.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn marks(h: &Harness<'static, PdfCraftApp>) -> usize {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().redaction_marks()
}

/// The Redact tool's box and Redact pages each add a mark, Clear all removes them (and undo brings
/// them back), and Redact all asks before applying them, which removes the fields under them.
#[test]
fn marking_applying_and_clearing() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.mark"));
    h.run_steps(2);
    h.get_by_label("Redact a PDF");
    // Draw a box in an empty part of the page.
    let (a, b) = (at(&h, 40.0, 280.0), at(&h, 200.0, 320.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(marks(&h), 1);
    // Redact pages ▸ current page.
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("Mark Page Range");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Clear all, then undo it.
    h.get_by_label("Clear all").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 0);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Redact all asks first; Apply removes the marks (and the page's fields under them).
    h.get_by_label("Redact all").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactApply));
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!((doc.redaction_marks(), doc.can_undo()), (0, Some("Apply redactions")));
    assert!(doc.form.is_empty(), "the whole page was redacted, fields included");
}

/// What a saved file holds: its redaction marks waiting to be applied, and its form fields
/// (whole-page marks cover them, so applying them removes the fields).
fn saved(path: &std::path::Path) -> (usize, usize) {
    let mut s = pdfcraft_engine::Session::new();
    let id = s.open("saved.pdf", None, std::sync::Arc::new(std::fs::read(path).unwrap()), None).unwrap();
    let doc = s.get(id).unwrap();
    (doc.redaction_marks(), doc.form.len())
}

/// Saving with redaction marks that aren't applied asks first (#4741): saved as they are, the
/// marks only outline what they cover, and the text and fields under them stay in the file.
/// Cancel writes nothing; Save with marks writes them as marks and doesn't ask again until the
/// document changes; Redact all and save applies them, then writes the file without what they
/// covered.
#[test]
fn saving_with_redaction_marks_not_applied_asks_first() {
    let mut h = harness();
    let dir = std::env::temp_dir().join(format!("pdfcraft-redact-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.pdf");
    let _ = std::fs::remove_file(&out);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    let fields = h.state().session.get(h.state().views[0].id).unwrap().form.len();
    assert!(fields > 0, "the fixture has form fields");
    // Mark the whole page.
    let mark_page = |h: &mut Harness<'static, PdfCraftApp>| {
        h.state_mut().execute("redact.pages");
        h.run_steps(2);
        h.get_by_label("OK").click();
        h.run_steps(3);
    };
    mark_page(&mut h);
    assert_eq!(marks(&h), 1);
    // Save asks; Cancel writes nothing.
    h.state_mut().execute("file.save");
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactBeforeSave));
    h.get_by_label("Redact all and save");
    assert!(!out.exists(), "nothing is written while the question is open");
    if let Ok(shots) = std::env::var("PDFCRAFT_SHOTS") {
        let _gpu = crate::gpu();
        for theme in ["light", "dark"] {
            h.state_mut().set_option("theme", theme).unwrap();
            h.run_steps(3);
            h.render().unwrap().save(format!("{shots}/redact-before-save-{theme}.png")).unwrap();
        }
        h.state_mut().set_option("theme", "light").unwrap();
        h.run_steps(2);
    }
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None);
    assert!(!out.exists(), "a cancelled save writes nothing");
    assert!(h.state().pending_redact_save.is_none());
    // Save with marks writes them as marks: the fields under them are still there.
    h.state_mut().execute("file.save");
    h.run_steps(3);
    h.get_by_label("Save with marks").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(saved(&out), (1, fields), "the mark is saved unapplied, with what it covers");
    // Saving the same document again doesn't ask again.
    std::fs::remove_file(&out).unwrap();
    h.state_mut().execute("file.save");
    h.run_steps(3);
    assert_eq!(h.state().dialog, None, "the marks were kept on purpose and nothing changed since");
    assert_eq!(saved(&out), (1, fields));
    // Another mark changes the document: Save asks again, and Redact all and save applies the
    // marks before writing.
    mark_page(&mut h);
    assert_eq!(marks(&h), 2);
    h.state_mut().execute("file.save");
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactBeforeSave));
    h.get_by_label("Redact all and save").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(marks(&h), 0);
    assert_eq!(saved(&out), (0, 0), "the saved file holds neither the marks nor what they covered");
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(!doc.dirty, "and the document is saved");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save with marks holds for each document until it changes: keeping a second document's marks
/// doesn't make the first ask again, and neither do later saves of either, though each write moves
/// its document's generation on.
#[test]
fn save_with_marks_holds_for_each_document_until_it_changes() {
    let mut h = harness();
    let dir = std::env::temp_dir().join(format!("pdfcraft-redact-each-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.pdf"), dir.join("b.pdf"));
    let save = |h: &mut Harness<'static, PdfCraftApp>, view: usize, out: &std::path::Path| {
        h.state_mut().active = Some(view);
        h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
        h.run_steps(2);
        h.state_mut().execute("file.save");
        h.run_steps(3);
    };
    let mark_and_keep = |h: &mut Harness<'static, PdfCraftApp>, view: usize, out: &std::path::Path| {
        h.state_mut().active = Some(view);
        h.run_steps(2);
        h.state_mut().execute("redact.pages");
        h.run_steps(2);
        h.get_by_label("OK").click();
        h.run_steps(3);
        save(h, view, out);
        assert_eq!(h.state().dialog, Some(Dialog::RedactBeforeSave), "view {view} asks first");
        h.get_by_label("Save with marks").click();
        h.run_steps(4);
        assert_eq!(h.state().dialog, None);
    };
    mark_and_keep(&mut h, 0, &a);
    h.state_mut().open_bytes("other.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
    crate::support::settle(&mut h);
    mark_and_keep(&mut h, 1, &b);
    for (view, out) in [(0, &a), (1, &b), (0, &a), (1, &b)] {
        save(&mut h, view, out);
        assert_eq!(h.state().dialog, None, "view {view}: its marks were kept and nothing changed since");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A redaction mark added while the Save As picker is open (on Windows the window still takes
/// input then) is asked about before anything is written to the place chosen there: Save with
/// marks writes it there with the marks, without asking again or opening another picker; Redact
/// all and save applies them and writes it there; Cancel writes nothing.
#[test]
fn a_mark_added_while_the_save_as_picker_is_open_is_asked_about_first() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-redact-picker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let elsewhere = dir.join("elsewhere.pdf");
    let marks_in = |path: &std::path::Path| {
        let mut s = pdfcraft_engine::Session::new();
        let id = s.open("saved.pdf", None, std::sync::Arc::new(std::fs::read(path).unwrap()), None).unwrap();
        s.get(id).unwrap().redaction_marks()
    };
    for (answer, out, marks) in
        [("Save with marks", "kept.pdf", Some(1)), ("Redact all and save", "applied.pdf", Some(0)), ("Cancel", "cancelled.pdf", None)]
    {
        let out = dir.join(out);
        let mut h = harness();
        h.state_mut().pick_override = Some(vec![out.to_string_lossy().into_owned()]);
        h.state_mut().execute("file.save_as");
        // The picker has answered, but no frame has taken the answer yet: the mark comes first.
        let mark = h.state().redact_prefs.mark(0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T");
        assert!(h.state_mut().apply_edit(mark));
        // A second picker would write here instead.
        h.state_mut().pick_override = Some(vec![elsewhere.to_string_lossy().into_owned()]);
        h.run_steps(3);
        assert_eq!(h.state().dialog, Some(Dialog::RedactBeforeSave), "{answer}: asked first");
        assert!(!out.exists(), "{answer}: nothing is written before the answer");
        h.get_by_label(answer).click();
        h.run_steps(4);
        assert_eq!(h.state().dialog, None, "{answer}: asked once");
        assert!(!elsewhere.exists(), "{answer}: no second picker");
        assert_eq!(out.exists().then(|| marks_in(&out)), marks, "{answer}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Redaction codes picked in Redaction Tool Properties become the marks' overlay text, in the
/// code set's order; turning codes off gives back the custom text, which was kept meanwhile.
#[test]
fn redaction_codes_become_the_overlay_text() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    h.get_by_label("Redaction Tool Properties");
    h.get_by_label("Use overlay text").click();
    h.run_steps(2);
    h.state_mut().redact_prefs.overlay = "CONFIDENTIAL".into();
    h.get_by_label("Redaction code:").click();
    h.run_steps(2);
    h.get_by_label("(b)(7)(C)").click();
    h.run_steps(2);
    h.get_by_label("(b)(6)").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    let prefs = &h.state().redact_prefs;
    assert_eq!(prefs.overlay_text(), "(b)(6), (b)(7)(C)", "codes in the set's order");
    let pdfcraft_engine::Edit::AddAnnotation(a) = prefs.mark(0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert!(matches!(a.shape, pdfcraft_engine::Shape::Redact { ref overlay, .. } if overlay == "(b)(6), (b)(7)(C)"));
    h.state_mut().redact_prefs.use_code = false;
    assert_eq!(h.state().redact_prefs.overlay_text(), "CONFIDENTIAL", "custom text is kept while codes are used");
}

/// The Redaction Tool Properties are the next run's too, like every other tool's defaults
/// (Acrobat's "Make Properties Default" without the box): fill, overlay text or codes, the
/// text's look, and the mark's outline colour and opacity all come back from the settings.
#[test]
fn redaction_properties_survive_a_restart() {
    let mut h = harness();
    {
        let p = &mut h.state_mut().redact_prefs;
        p.fill = None;
        p.use_overlay = true;
        p.use_code = true;
        p.code_set = pdfcraft_engine::REDACTION_CODE_SETS[0];
        p.codes = vec!["(b)(6)", "(b)(7)(C)"];
        p.overlay = "CONFIDENTIAL".into();
        p.look.font = pdfcraft_engine::OverlayFont::Courier;
        p.look.size = 12.0;
        p.look.color = [0.0, 0.0, 1.0];
        p.look.align = 2;
        p.look.repeat = true;
        p.outline = [0.0, 0.5, 0.0];
        p.hover_fill = [1.0, 1.0, 0.0];
        p.opacity = 0.4;
    }
    // The defaults are what OK with "Make properties default" ticked leaves: go through the
    // dialog.
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(6);
    h.get_by_label("Make properties default");
    h.get_by_label("OK").click();
    h.run_steps(2);
    let json = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&json);
    let p = &fresh.redact_prefs;
    assert_eq!(p.fill, None, "no colour is a saved choice, not a missing one");
    assert_eq!(p.overlay_text(), "(b)(6), (b)(7)(C)");
    assert_eq!(p.overlay, "CONFIDENTIAL");
    assert_eq!(p.look.font, pdfcraft_engine::OverlayFont::Courier);
    assert_eq!((p.look.size, p.look.color, p.look.align, p.look.repeat), (12.0, [0.0, 0.0, 1.0], 2, true));
    assert_eq!((p.outline, p.hover_fill, p.opacity), ([0.0, 0.5, 0.0], [1.0, 1.0, 0.0], 0.4));
    // The next mark takes the outline and opacity.
    let pdfcraft_engine::Edit::AddAnnotation(a) = p.mark(0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert_eq!((a.style.color, a.style.opacity, a.style.fill), ([0.0, 0.5, 0.0], 0.4, None));
    // Settings are untrusted: a bad colour, an unknown code set and a code outside the set are
    // dropped, the rest kept.
    let mut odd = PdfCraftApp::new();
    odd.restore(
        r#"{"redact_prefs":{"outline":[2,0,0],"opacity":0.7,"code_set":"nope","codes":["(b)(6)","bogus"],"use_overlay":true,"use_code":true}}"#,
    );
    let p = &odd.redact_prefs;
    assert_eq!((p.outline, p.opacity), ([0.89, 0.13, 0.13], 0.7));
    assert_eq!(p.code_set.id, pdfcraft_engine::REDACTION_CODE_SETS[0].id);
    assert_eq!(p.overlay_text(), "(b)(6)");
}

/// "Make properties default" off: OK keeps the values for this run only, the saved defaults
/// stay what they were; Cancel puts back what the dialog opened with.
#[test]
fn redaction_properties_for_this_run_only_and_cancel() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(6);
    h.get_by_label("Redaction mark appearance");
    h.get_by_label("Outline colour:");
    h.get_by_label("Fill colour:");
    h.get_by_label("Opacity:");
    h.get_by_label("Make properties default").click();
    h.run_steps(3);
    assert!(!h.state().redact_prefs.remember);
    h.state_mut().redact_prefs.outline = [0.0, 0.0, 1.0];
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().redact_prefs.outline, [0.0, 0.0, 1.0], "used for this run");
    assert_eq!(h.state().redact_defaults.outline, [0.89, 0.13, 0.13], "not made the default");
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&h.state().persist());
    assert_eq!(fresh.redact_prefs.outline, [0.89, 0.13, 0.13], "the next run starts from the saved defaults");
    // Cancel discards the edits made while the dialog was open.
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    h.state_mut().redact_prefs.opacity = 0.2;
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().redact_prefs.opacity, 1.0, "cancelled");
    assert_eq!(h.state().redact_prefs.outline, [0.0, 0.0, 1.0], "the run's values stay");
}

#[test]
fn redacting_an_imported_word_list() {
    let memo = pdfcraft_engine::Session::new().create_from_text("memo", "Call Ada today\nAda is private\nPublic line").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("memo.pdf", None, memo.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.run_steps(2);
    h.get_by_label("Multiple words or phrases");
    let list = b"  Ada \n\nprivate\nAda\nabsent\n".to_vec();
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::RedactWords, vec![("names.txt".into(), list)]);
    let d = &h.state().redact_search;
    assert_eq!((d.mode, d.words.as_str()), (pdfcraft_ui_egui::RedactSearchMode::Words, "Ada\nprivate\nabsent"));
    h.run_steps(2);
    h.get_by_label("3 word(s) or phrase(s)");
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.found), (3, Some(3)));
    let too_big = vec![b'a'; 2 << 20];
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::RedactWords, vec![("huge.txt".into(), too_big)]);
    assert_eq!(h.state().redact_search.words, "Ada\nprivate\nabsent", "an oversized list changes nothing");
}

#[test]
fn redaction_search_can_match_case_and_whole_words() {
    let memo = pdfcraft_engine::Session::new().create_from_text("memo", "Ada ada Adaline\nAda beta betamax").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("memo.pdf", None, memo.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.state_mut().redact_search.text = "Ada".into();
    h.run_steps(2);
    h.get_by_label("Whole words only").click();
    h.get_by_label("Case-sensitive").click();
    h.run_steps(2);
    assert!(h.state().redact_search.whole_words && h.state().redact_search.case_sensitive);
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.found), (2, Some(2)));
}

#[test]
fn removing_hidden_information_and_sanitizing() {
    let mut h = harness();
    h.state_mut().execute("protect.remove_hidden");
    h.run_steps(2);
    h.get_by_label("Remove hidden information");
    {
        let d = &h.state().hidden_draft;
        let fields = d.found.iter().find(|f| f.0 == pdfcraft_engine::Hidden::FormFields).unwrap();
        assert_eq!((fields.1, fields.2), (5, true), "five fields, checked");
    }
    h.get_by_label("Remove").click();
    h.run_steps(3);
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert!(doc.form.is_empty());
        assert_eq!(doc.can_undo(), Some("Remove hidden information"));
    }
    h.state_mut().execute("redact.sanitize");
    h.run_steps(2);
    h.get_by_label("Sanitize").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Sanitize document"));
}
