//! Format text's Paragraph spacing row edits the open paragraph's spacing.

use egui::Key;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_ui_egui::PdfCraftApp;

/// A one-page document showing "Page 1".
fn fixture() -> Vec<u8> {
    let body = "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The document open in Edit text with its paragraph in the editor.
fn editing() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture()).expect("fixture opens");
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    assert!(h.state().views[0].line_editor.is_some());
    h
}

fn paragraph_spacing(h: &Harness<'static, PdfCraftApp>) -> Option<f64> {
    h.state().views[0].line_editor.as_ref().map(|e| e.extras.paragraph_spacing)
}

/// The row is a working field, not a placeholder: a value typed in it becomes the paragraph's
/// spacing and is applied as an edit.
#[test]
fn the_paragraph_spacing_field_edits_the_paragraph() {
    let mut h = editing();
    h.get_by_label("FORMAT TEXT");
    let field = h.get_by_role_and_label(Role::SpinButton, "Paragraph spacing");
    assert!(!field.accesskit_node().is_disabled());
    assert_eq!(paragraph_spacing(&h), Some(0.0));
    field.click();
    h.run_steps(2);
    h.event(egui::Event::Text("12".into()));
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(4);
    assert_eq!(paragraph_spacing(&h), Some(12.0));
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Edit text"));
}
