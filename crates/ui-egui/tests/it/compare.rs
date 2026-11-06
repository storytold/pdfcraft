//! Compare files in the real shell (egui_kittest): the dialog, the Compare panel, marks and
//! the report.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Session;
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

#[test]
fn compare_two_versions() {
    let s = Session::new();
    let v1 = s.create_from_text("t", "Delivery within five days. Returns accepted.").unwrap().to_vec();
    let v2 = s.create_from_text("t", "Delivery within three days. Returns accepted for a week.").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("v1.pdf", None, v1.clone()).unwrap();
        app.open_bytes("v2.pdf", None, v2.clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("doc.compare"));
    h.run_steps(2);
    h.get_by_label("Compare Files");
    h.get_by_label("Compare").click();
    h.run_steps(3);
    assert_eq!(h.state().right, Some(RightPanel::Compare));
    assert_eq!(h.state().toast.clone().unwrap().0, "2 differences found");
    h.get_by_label("2 Replaced");
    h.get_by_label("\"five\" → \"three\"").click();
    h.run_steps(2);
    assert_eq!(h.state().compare.as_ref().unwrap().selected, Some(0));
    let id = h.state().active_ids().unwrap().1;
    let i = h.state().active_ids().unwrap().0;
    assert_eq!(h.state().views[i].compare_marks.len(), 2);
    // Side by side: the old file's half shades its side of the changes and follows the new
    // file's zoom; "New file only" ends it.
    assert!(h.state().compare.as_ref().unwrap().side_by_side, "side by side on a wide window");
    let old_i = h.state().views.iter().position(|v| v.id == h.state().compare.as_ref().unwrap().old).unwrap();
    assert_eq!(h.state().views[old_i].compare_marks.len(), 2, "replaced words shaded on the old file too");
    assert!(h.state().views[old_i].flash.is_some(), "the old half jumped to the change");
    h.state_mut().views[i].zoom = 1.5;
    h.state_mut().views[i].fit = pdfcraft_ui_egui::canvas::Fit::None;
    h.run_steps(3);
    assert_eq!((h.state().views[old_i].zoom, h.state().views[old_i].fit), (1.5, pdfcraft_ui_egui::canvas::Fit::None), "the old half follows");
    h.get_by_label("New file only").click();
    h.run_steps(2);
    assert!(!h.state().compare.as_ref().unwrap().side_by_side);
    h.get_by_label("Side by side").click();
    h.run_steps(2);
    assert!(h.state().compare.as_ref().unwrap().side_by_side);
    h.get_by_label("New file only").click();
    h.run_steps(2);
    h.get_by_label("Mark as comments").click();
    h.run_steps(3);
    assert_eq!(h.state().session.get(id).unwrap().info.annotations.len(), 2);
    h.get_by_label("Report…").click();
    h.run_steps(3);
    assert_eq!(h.state().session.get(h.state().active_ids().unwrap().1).unwrap().name, "Compare Report.pdf");
}

/// Two versions compared side by side (old `v1.pdf` left, new `v2.pdf` right).
fn side_by_side() -> Harness<'static, PdfCraftApp> {
    let s = Session::new();
    let v1 = s.create_from_text("t", "Delivery within five days. Returns accepted.").unwrap().to_vec();
    let v2 = s.create_from_text("t", "Delivery within three days. Returns accepted for a week.").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("v1.pdf", None, v1.clone()).unwrap();
        app.open_bytes("v2.pdf", None, v2.clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("doc.compare"));
    h.run_steps(2);
    h.get_by_label("Compare").click();
    h.run_steps(3);
    assert!(h.state().compare.as_ref().unwrap().side_by_side);
    h
}

/// #3915: a half that closes its own tab while it draws (its snapshot bar's Close sets
/// `close_requested`) must not leave the frame indexing the closed tab. Either half.
#[test]
fn closing_either_half_of_a_side_by_side_comparison_keeps_the_app_running() {
    for (half, name) in [(1, "v2.pdf"), (0, "v1.pdf")] {
        let mut h = side_by_side();
        let c = h.state().compare.as_ref().unwrap();
        let id = if half == 1 { c.new } else { c.old };
        let i = h.state().views.iter().position(|v| v.id == id).unwrap();
        h.state_mut().views[i].close_requested = true;
        h.run_steps(3);
        let s = h.state();
        assert_eq!(s.views.len(), 1, "the {name} tab closed");
        assert!(s.views.iter().all(|v| v.id != id));
    }
}

/// Two versions whose differences are of both kinds: a replaced word (text) and a rectangle
/// drawn far below the text (visual only, no text change explains it).
fn text_and_visual_versions() -> (Vec<u8>, Vec<u8>) {
    let mut s = Session::new();
    let v1 = s.create_from_text("t", "Delivery within five days. Returns accepted.").unwrap().to_vec();
    let text = s.create_from_text("t", "Delivery within three days. Returns accepted.").unwrap();
    let id = s.open("v2.pdf", None, text, None).unwrap();
    let square = pdfcraft_engine::Shape::Rectangle { rect: [100.0, 80.0, 300.0, 200.0] };
    let style = pdfcraft_engine::Style::default_for(&square);
    let new = pdfcraft_engine::NewAnnotation { page: 0, shape: square, style, contents: String::new(), author: "Ada".into() };
    s.apply(id, pdfcraft_engine::Edit::AddAnnotation(new)).unwrap();
    (v1, s.save_bytes(id).unwrap().to_vec())
}

/// The Compare panel's Show: Text / Visual filter, on both halves: Text takes the text changes
/// out of the list and the shading, Visual takes the visual differences (counted, never listed)
/// out of the shading, and each brings them back.
#[test]
fn compare_filters_hide_text_or_visual_changes() {
    let (v1, v2) = text_and_visual_versions();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("v1.pdf", None, v1.clone()).unwrap();
        app.open_bytes("v2.pdf", None, v2.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().compare_old = h.state().views.first().map(|v| v.id);
    h.state_mut().run_compare();
    h.run_steps(3);
    let new_i = h.state().active_ids().unwrap().0;
    let old_i = h.state().views.iter().position(|v| v.id == h.state().compare.as_ref().unwrap().old).unwrap();
    // Visual differences are shaded orange (compare_ui::VISUAL), text changes in their kind's colour.
    let visual = egui::Color32::from_rgb(0xF0, 0x8C, 0x1A);
    let kinds = |h: &Harness<'_, PdfCraftApp>, i: usize| {
        let marks = &h.state().views[i].compare_marks;
        (marks.iter().filter(|m| m.2 != visual).count(), marks.iter().filter(|m| m.2 == visual).count())
    };
    let (text_new, visual_new) = kinds(&h, new_i);
    let (text_old, visual_old) = kinds(&h, old_i);
    assert!(text_new > 0 && visual_new > 0, "the fixture has both kinds on the new file: {text_new} text, {visual_new} visual");
    assert!(text_old > 0 && visual_old > 0, "and on the old file: {text_old} text, {visual_old} visual");
    h.get_by_label("\"five\" → \"three\"");

    h.get_by_label("Text").click();
    h.run_steps(2);
    assert!(h.query_by_label("\"five\" → \"three\"").is_none(), "text changes leave the list");
    assert_eq!(kinds(&h, new_i), (0, visual_new), "and the new file's shading");
    assert_eq!(kinds(&h, old_i), (0, visual_old), "and the old file's");

    h.get_by_label("Visual").click();
    h.run_steps(2);
    assert_eq!(kinds(&h, new_i), (0, 0));
    assert_eq!(kinds(&h, old_i), (0, 0));

    h.get_by_label("Text").click();
    h.run_steps(2);
    h.get_by_label("\"five\" → \"three\"");
    assert_eq!(kinds(&h, new_i), (text_new, 0), "text back, visual still hidden");
    assert_eq!(kinds(&h, old_i), (text_old, 0));
}

/// Writes a screenshot of the Compare panel when PDFCRAFT_SHOT is set (for review).
#[test]
fn compare_panel_screenshot() {
    let Ok(out) = std::env::var("PDFCRAFT_SHOT") else { return };
    let _gpu = crate::gpu();
    let s = Session::new();
    let v1 = s.create_from_text("t", "Delivery within five days. Returns accepted.").unwrap().to_vec();
    let v2 = s.create_from_text("t", "Delivery within three days. Returns accepted for a week.").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("v1.pdf", None, v1.clone()).unwrap();
        app.open_bytes("v2.pdf", None, v2.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().compare_old = h.state().views.first().map(|v| v.id);
    h.state_mut().run_compare();
    for _ in 0..20 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    h.render().unwrap().save(out).unwrap();
}

#[test]
fn pdfa_dialog_verifies_and_converts() {
    let doc = Session::new().create_from_text("t", "Keep forever").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("keep.pdf", None, doc.clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("standards.pdfa"));
    h.run_steps(2);
    h.get_by_label("Declared conformance: none");
    assert!(h.query_by_label("Remove PDF/A identification").is_none(), "nothing to remove yet");
    h.get_by_label("Verify").click();
    h.run_steps(3);
    h.get_by_label("The document has no XMP metadata");
    h.get_by_label("Save as PDF/A").click();
    h.run_steps(3);
    h.get_by_label("Declared conformance: PDF/A-2b");
    let issues = h.state().pdfa.issues.clone().unwrap();
    assert!(issues.iter().all(|i| !i.fixable), "{issues:?}");
    let id = h.state().active_ids().unwrap().1;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Save as PDF/A-2b"));
    h.get_by_label("Remove PDF/A identification").click();
    h.run_steps(3);
    h.get_by_label("Declared conformance: none");
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Remove PDF/A identification"));
}

/// A one-page PDF whose XMP declares the given `pdfaid`/`pdfuaid` attributes.
fn declaring(ids: &str) -> Vec<u8> {
    declaring_with(ids, false)
}

/// [`declaring`], with a text field on its page when `field` is set.
fn declaring_with(ids: &str, field: bool) -> Vec<u8> {
    let xmp = format!(
        "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" \
         xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\" {ids}/></rdf:RDF>"
    );
    let (form, annots) = if field { (" /AcroForm << /Fields [5 0 R] >>", " /Annots [5 0 R]") } else { ("", "") };
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Metadata 4 0 R{form} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << >>{annots} >>"),
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len()),
    ];
    if field {
        objects.push("<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 200 180 220] /P 3 0 R /F 4 >>".to_string());
    }
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

#[test]
fn pdfa_dialog_shows_a_declared_pdfua() {
    // The dialog read only the PDF/A identification, so a PDF/UA-1 document showed "none" (#1309).
    for (ids, shown) in [
        ("pdfuaid:part=\"1\"", "Declared conformance: PDF/UA-1"),
        ("pdfaid:part=\"2\" pdfaid:conformance=\"B\" pdfuaid:part=\"1\"", "Declared conformance: PDF/A-2b, PDF/UA-1"),
    ] {
        let doc = declaring(ids);
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("ua.pdf", None, doc.clone()).unwrap();
            app
        });
        h.run_steps(4);
        assert!(h.state_mut().execute("standards.pdfa"));
        h.run_steps(2);
        h.get_by_label(shown);
    }
}

fn open_declaring(ids: &str, settings: Option<String>) -> Harness<'static, PdfCraftApp> {
    open_pdf(declaring(ids), settings)
}

fn open_pdf(doc: Vec<u8>, settings: Option<String>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        if let Some(s) = &settings {
            app.restore(s);
        }
        app.set_option("language", "en").unwrap();
        app.open_bytes("archive.pdf", None, doc.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

const PDFA_NOTICE: &str = "This document declares that it conforms to";

/// #5930: a document that declares PDF/A says so above its pages, naming the part, as Acrobat's
/// bar does. Verify opens Standards ▸ PDF/A with the document verified against the part it
/// declares, and the bar goes once the declaration is removed and comes back on Undo.
#[test]
fn a_document_that_declares_pdfa_says_so_above_its_pages() {
    let mut h = open_declaring("pdfaid:part=\"2\" pdfaid:conformance=\"B\"", None);
    h.get_by_label("This document declares that it conforms to PDF/A-2b. Changes can break its conformance.");
    assert!(h.state().dialog.is_none());
    h.get_by_label("Verify").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::PdfA));
    assert!(h.state().pdfa.issues.is_some(), "the dialog opens verified");
    h.get_by_label("Remove PDF/A identification").click();
    h.run_steps(3);
    h.state_mut().dialog = None;
    h.run_steps(3);
    assert!(h.query_by_label_contains(PDFA_NOTICE).is_none(), "nothing is declared any more");
    h.state_mut().undo();
    h.run_steps(3);
    h.get_by_label_contains("PDF/A-2b");

    // PDF/A-3 is verified as 3b, even after the dialog was left at 2b: this file passes the 3b
    // identification check and fails the 2b one, so the two results differ.
    let mut h = open_declaring("pdfaid:part=\"3\" pdfaid:conformance=\"B\"", None);
    h.get_by_label_contains("conforms to PDF/A-3b.");
    h.get_by_label("Verify").click();
    h.run_steps(3);
    assert_eq!(h.state().pdfa.level, pdfcraft_engine::pdfa::Level::A3b);
    let doc = h.state().session.get(h.state().active_ids().unwrap().1).unwrap();
    let (as_3b, as_2b) = (doc.pdfa_verify(pdfcraft_engine::pdfa::Level::A3b), doc.pdfa_verify(pdfcraft_engine::pdfa::Level::A2b));
    assert_ne!(as_3b, as_2b);
    assert_eq!(h.state().pdfa.issues, Some(as_3b));

    // PDF/UA alone is not PDF/A: no bar.
    let h = open_declaring("pdfuaid:part=\"1\"", None);
    assert!(h.query_by_label_contains(PDFA_NOTICE).is_none());
}

/// #5930: Don't show again turns the PDF/A notice off in every document, kept in the settings,
/// and Preferences ▸ General (or `--pdfa-notice`) turns it back on.
#[test]
fn the_pdfa_notice_can_be_turned_off_for_good() {
    let declares = "pdfaid:part=\"2\" pdfaid:conformance=\"U\"";
    let mut h = open_declaring(declares, None);
    assert!(h.state().show_pdfa_notice, "on by default");
    h.get_by_label_contains(PDFA_NOTICE);
    h.get_by_label("Don't show again").click();
    h.run_steps(2);
    assert!(h.query_by_label_contains(PDFA_NOTICE).is_none());
    assert!(!h.state().show_pdfa_notice);
    let saved = h.state().persist();
    let mut h = open_declaring(declares, Some(saved));
    assert!(h.query_by_label_contains(PDFA_NOTICE).is_none(), "remembered across a restart");
    h.state_mut().set_option("dialog", "preferences").unwrap();
    h.run_steps(2);
    let label = "Show a notice when a document declares PDF/A conformance";
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, label).scroll_to_me();
    h.run_steps(4);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, label).click();
    h.run_steps(2);
    assert!(h.state().show_pdfa_notice);
    h.state_mut().dialog = None;
    h.run_steps(2);
    h.get_by_label_contains("conforms to PDF/A-2u.");
    let app = h.state_mut();
    app.set_option("pdfa-notice", "false").unwrap();
    assert!(!app.show_pdfa_notice);
    assert_eq!(app.set_option("pdfa-notice", "maybe"), Err("pdfa-notice must be true or false".into()));
}

/// #5930 review: a PDF/A form says both, as a secured form does, and keeps Highlight fields
/// next to Verify and Don't show again.
#[test]
fn a_pdfa_form_says_what_it_declares_after_its_form_notice() {
    let h = open_pdf(declaring_with("pdfaid:part=\"2\" pdfaid:conformance=\"B\"", true), None);
    h.get_by_label(
        "This document contains 1 interactive form fields. This document declares that it conforms to PDF/A-2b. Changes can break its conformance.",
    );
    assert!(h.query_by_label("Highlight fields").or(h.query_by_label("Hide field highlights")).is_some(), "the form's button stays");
    h.get_by_label("Verify");
    h.get_by_label("Don't show again");
}

/// #5930 review: Standards ▸ PDF/A works on the active tab, so the old half of a side-by-side
/// comparison says what it declares but offers no Verify that would check the other file.
#[test]
fn the_old_half_of_a_comparison_offers_no_verify() {
    let old = declaring("pdfaid:part=\"2\" pdfaid:conformance=\"B\"");
    let new = Session::new().create_from_text("t", "Kept for ten years").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("old.pdf", None, old.clone()).unwrap();
        app.open_bytes("new.pdf", None, new.clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("doc.compare"));
    h.run_steps(2);
    h.get_by_label("Compare").click();
    h.run_steps(3);
    assert!(h.state().compare.as_ref().unwrap().side_by_side);
    h.get_by_label_contains("conforms to PDF/A-2b.");
    h.get_by_label("Don't show again");
    assert!(h.query_by_label("Verify").is_none(), "the old file isn't the one the dialog would verify");
}

#[test]
fn export_to_word_html_and_rtf() {
    let doc = Session::new().create_from_text("t", "Exported words").unwrap().to_vec();
    let dir = std::env::temp_dir().join(format!("pdfcraft-office-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("e.pdf", None, doc).unwrap();
    for ext in ["docx", "pptx", "html", "rtf"] {
        let out = dir.join(format!("e.{ext}"));
        app.save_override = Some(out.to_string_lossy().into_owned());
        assert!(app.execute(&format!("export.{ext}")));
        assert!(std::fs::metadata(&out).unwrap().len() > 40, "{ext}");
    }
    assert!(std::fs::read_to_string(dir.join("e.html")).unwrap().contains("Exported words"));
}
