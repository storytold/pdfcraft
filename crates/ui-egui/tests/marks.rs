//! Header & footer, watermark and background dialogs in the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::MarkKind;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, FIXTURE.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn marks(h: &Harness<'static, PdfCraftApp>) -> Vec<MarkKind> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().marks.clone()
}

#[test]
fn header_and_footer_dialog_inserts_tokens_and_applies_to_a_subset() {
    let mut h = harness();
    assert!(!h.state_mut().execute("edit.header_footer.remove"), "nothing to remove yet");
    assert!(h.state_mut().execute("edit.header_footer"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Marks(MarkKind::HeaderFooter)));
    // The center footer box takes the page-number token.
    h.state_mut().marks_draft.focused_box = 4;
    h.get_by_label("Insert Page Number").click();
    h.run_steps(2);
    assert_eq!(h.state().marks_draft.hf.text[4], "<<1>>");
    h.state_mut().marks_draft.range.subset = pdfcraft_ui_egui::marks::Subset::Odd;
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(marks(&h), [MarkKind::HeaderFooter]);
    // Update and Remove are now available.
    assert!(h.state_mut().execute("edit.header_footer.update"));
    h.run_steps(2);
    h.get_by_label("Update Header and Footer");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("edit.header_footer.remove"));
    assert!(marks(&h).is_empty());
}

#[test]
fn watermark_and_background_dialogs() {
    let mut h = harness();
    h.state_mut().execute("edit.watermark");
    h.run_steps(2);
    assert!(h.query_by(|n| n.label().as_deref() == Some("OK") && n.is_disabled()).is_some(), "OK waits for text");
    h.state_mut().marks_draft.wm.text = "DRAFT".into();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    h.state_mut().execute("edit.background");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h).len(), 2);
}

#[test]
fn backgrounds_and_watermarks_from_a_file() {
    let mut h = harness();
    h.state_mut().execute("edit.background");
    h.run_steps(2);
    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label("No file chosen");
    assert!(h.query_by(|n| n.label().as_deref() == Some("OK") && n.is_disabled()).is_some(), "OK waits for a file");
    // (Browse… opens the system picker; the test supplies the file directly.)
    h.state_mut().marks_draft.file = Some(("letterhead.pdf".into(), std::sync::Arc::new(FIXTURE.to_vec())));
    h.run_steps(2);
    h.get_by_label("Page number");
    h.get_by_label("Scale relative to target page");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), vec![MarkKind::Background]);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Add background"));
}
