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

fn clipped_query_fixture(query: &str) -> Vec<u8> {
    let form = format!("BT /F1 10 Tf 200 200 Td ({query}) Tj ET");
    let page = format!("BT /F1 10 Tf 20 20 Td ({query}) Tj ET /Fm Do");
    let widths = "600 ".repeat(95);
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Fm 6 0 R >> >> >>"
            .into(),
        format!("<< /Length {} >>\nstream\n{page}\nendstream", page.len()),
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 126 /Widths [{widths}] /FontDescriptor << /Type /FontDescriptor /FontName /Courier /Flags 33 /FontBBox [-23 -250 715 805] /ItalicAngle 0 /Ascent 629 /Descent -157 /CapHeight 562 /StemV 51 >> >>"
        ),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 50 50] /Matrix [1 0 0 1 0 0] /Resources << /Font << /F1 5 0 R >> >> /Length {} >>\nstream\n{form}\nendstream",
            form.len()
        ),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![];
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
    for n in offsets {
        out.extend_from_slice(format!("{n:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn query_redaction_includes_clipped_copies_in_the_saved_document() {
    use pdfcraft_ui_egui::RedactSearchMode;
    for mode in [RedactSearchMode::Phrase, RedactSearchMode::Words, RedactSearchMode::Patterns] {
        let query = if mode == RedactSearchMode::Patterns { "x@y.test" } else { "QUERY" };
        let bytes = clipped_query_fixture(query);
        let mut app = PdfCraftApp::new();
        app.open_bytes("clipped.pdf", None, bytes).unwrap();
        app.redact_search.mode = mode;
        app.redact_search.pattern = pdfcraft_engine::RedactPattern::Email;
        app.redact_search.text = query.into();
        app.redact_search.words = query.into();
        assert_eq!(app.redact_search(), 2, "{mode:?}");
        let id = app.views[0].id;
        assert_eq!(app.session.get(id).unwrap().redaction_marks(), 2);
        app.session.apply(id, pdfcraft_engine::Edit::ApplyRedactions { pages: None }).unwrap();
        assert_eq!(app.session.get(id).unwrap().redaction_marks(), 0);
        let saved = app.session.save_full_bytes(id).unwrap();
        // Full save retains only reachable objects. Inspect decoded content, not visual extraction.
        let parsed = pdfcraft_cos::Document::open(saved.clone()).unwrap();
        for (_, object) in parsed.scan_objects() {
            if let pdfcraft_cos::Object::Stream(stream) = object.as_ref() {
                let decoded = stream.decoded_strict().unwrap();
                assert!(!decoded.windows(query.len()).any(|s| s == query.as_bytes()), "{mode:?}");
            }
        }
        app.open_bytes("saved.pdf", None, saved.as_ref().clone()).unwrap();
        assert_eq!(app.session.get(app.views.last().unwrap().id).unwrap().redaction_marks(), 0);
    }
}
