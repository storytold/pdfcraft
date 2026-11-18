//! A script button with no convertible characters must not rewrite hidden source text.
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_render::{PageRenderer, RenderRequest};
use pdfcraft_ui_egui::PdfCraftApp;
use std::sync::Arc;

fn hidden_text() -> Vec<u8> {
    let body = "BT /F1 24 Tf 3 Tr 40 150 Td (QQQ) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn pixels(bytes: Arc<Vec<u8>>) -> Vec<u8> {
    let mut renderer = PageRenderer::new(bytes, Default::default());
    let rendered = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(rendered.error.is_none(), "{:?}", rendered.error);
    rendered.rgba.to_vec()
}

#[test]
fn unsupported_script_buttons_leave_hidden_text_bytes_history_and_saved_pixels_unchanged() {
    for button in ["Superscript", "Subscript"] {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("hidden.pdf", None, hidden_text()).unwrap();
            app
        });
        h.run_steps(4);
        assert!(h.state_mut().execute("edit.edit_text"));
        h.run_steps(3);
        let id = h.state().views[0].id;
        let block = h.state().session.get(id).unwrap().text_blocks(0).remove(0);
        assert_eq!(block.text, "QQQ");
        let page = h.state().views[0].page_screen_rect(0).unwrap();
        let x = (block.rect[0] + block.rect[2]) as f32 / 2.0;
        let y = (block.rect[1] + block.rect[3]) as f32 / 2.0;
        let at = egui::pos2(page.left() + x / 300.0 * page.width(), page.top() + (300.0 - y) / 300.0 * page.height());
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(3);
        let editor = h.state_mut().views[0].line_editor.as_mut().expect("hidden text editor opens");
        editor.selection = Some(1..2);
        editor.focus = true;
        h.run_steps(1);
        let before = h.state().session.get(id).unwrap().bytes.clone();
        assert!(h.state().session.get(id).unwrap().can_undo().is_none());
        h.get_by_label(button).click();
        h.run_steps(4);
        let saved = h.state_mut().session.save_bytes(id).unwrap();
        assert_eq!(pixels(saved.clone()), pixels(before.clone()), "{button} must preserve hidden text after saving");
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.bytes.as_ref(), before.as_ref(), "no conversion must leave source bytes untouched");
        assert!(doc.can_undo().is_none(), "no conversion must not create an edit");
        let mut reopened = PdfCraftApp::new();
        reopened.open_bytes("reopened.pdf", None, saved.as_ref().clone()).unwrap();
        assert_eq!(reopened.session.get(reopened.views[0].id).unwrap().text_blocks(0)[0].text, "QQQ");
    }
}
