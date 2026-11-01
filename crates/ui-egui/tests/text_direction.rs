//! Focused direction controls acceptance, runnable without linking the full UI test suite.

use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

#[test]
fn explicit_direction_buttons_preserve_text_and_alignment_in_the_editor() {
    use pdfcraft_engine::TextDirection as D;
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    open_paragraph(&mut h);
    let ed = h.state().views[0].line_editor.as_ref().unwrap();
    let original = ed.text.clone();
    let alignment = ed.look.align;
    h.get_by_label("Right to left (RTL)").click();
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.as_ref().unwrap();
    assert_eq!(ed.look.direction, D::RightToLeft);
    assert_eq!(ed.look.align, alignment);
    assert_eq!(ed.text, original);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().text_blocks(0)[0].text_direction, D::RightToLeft);
    if let Some(path) = std::env::var_os("PDFCRAFT_DIRECTION_SNAPSHOT") {
        h.render().unwrap().save(std::path::Path::new(&path)).unwrap();
    }
    h.get_by_label("Left to right (LTR)").click();
    h.run_steps(3);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().look.direction, D::LeftToRight);
    h.get_by_label("Automatic text direction").click();
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.as_ref().unwrap();
    assert_eq!(ed.look.direction, D::Auto);
    assert_eq!(ed.look.align, alignment);
    assert_eq!(ed.text, original);
    h.key_press(Key::Escape);
    h.run_steps(3);
    // Format controls apply as undoable edits, like the existing font/alignment controls.
    for _ in 0..3 {
        if !h.state_mut().execute("edit.undo") {
            break;
        }
        h.run_steps(3);
    }
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.text_blocks(0)[0].text_direction, D::Auto);
    assert!(!doc.dirty, "undoing the format changes restores the original document");
}

fn harness(pages: usize, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut builder = Harness::builder().with_size(egui::vec2(1400.0, 900.0));
    if std::env::var_os("PDFCRAFT_ORGANIZE_SNAPSHOT").is_some() || std::env::var_os("PDFCRAFT_DIRECTION_SNAPSHOT").is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture(pages)).expect("fixture opens");
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

fn click_at(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(4);
}

fn open_paragraph(h: &mut Harness<'static, PdfCraftApp>) {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    click_at(h, at);
    assert!(h.state().views[0].line_editor.is_some(), "the paragraph opens for typing");
}
