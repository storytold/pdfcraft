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
    s.session.get(s.views[0].id).unwrap().marks().to_vec()
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
    h.get_by_label("Offset from centre");
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

/// Three pages: a white backdrop of its own (as browsers write), a page-sized picture (a scan)
/// and bare paper.
fn backdrop_fixture() -> Vec<u8> {
    let stream = |data: &str| format!("<< /Length {} >> stream\n{data}\nendstream", data.len());
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] /Resources << /XObject << /Im0 9 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>".into(),
        stream("q 1 1 1 rg 0 0 300 400 re f Q 0 g 20 20 50 50 re f"),
        stream("q 300 0 0 400 0 0 cm /Im0 Do Q"),
        stream("0 g 20 20 50 50 re f"),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >> stream\n\u{80}\nendstream"
            .into(),
    ];
    let mut out = String::from("%PDF-1.7\n");
    for (i, o) in objs.iter().enumerate() {
        out.push_str(&format!("{} 0 obj {o} endobj\n", i + 1));
    }
    out.push_str("trailer << /Root 1 0 R >>\n%%EOF");
    out.into_bytes()
}

#[test]
fn the_background_dialog_offers_to_replace_a_white_backdrop_and_warns_about_scans() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, backdrop_fixture()).unwrap();
        app
    });
    h.run_steps(4);
    let backdrop = |h: &Harness<'static, PdfCraftApp>, page| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().page_backdrop(page)
    };
    assert_eq!(backdrop(&h, 0), Some(pdfcraft_engine::Backdrop::Fill));
    h.state_mut().execute("edit.background");
    h.run_steps(3);
    let replace = "This page paints its own white backdrop, which would hide the background. Replace it";
    h.get_by_label("A picture covers this page (a scan?), so the background won't show there.");
    assert!(h.state().marks_draft.bg.replace_backdrop, "on by default");
    // Left unticked, the backdrop stays and still hides the background.
    h.get_by_label(replace).click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(backdrop(&h, 0), Some(pdfcraft_engine::Backdrop::Fill));
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(2);
    // Ticked, it is replaced; the scan is reported, not touched.
    h.state_mut().execute("edit.background");
    h.run_steps(3);
    h.get_by_label(replace).click();
    h.run_steps(2);
    assert!(h.state().marks_draft.bg.replace_backdrop);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(backdrop(&h, 0), None);
    assert_eq!(backdrop(&h, 1), Some(pdfcraft_engine::Backdrop::Image));
    // Nothing left to replace: the offer is gone.
    h.state_mut().execute("edit.background.update");
    h.run_steps(3);
    assert!(h.query_by_label(replace).is_none());
}
