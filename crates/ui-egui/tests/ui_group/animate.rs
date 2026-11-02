//! LaTeX `animate` PDFs (#169): every field is a push button holding a frame and a Screen
//! annotation drives the player. Nothing there can be filled in, so the form-fields bar stays
//! away, and the Screen annotation is not a comment.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// A one-page PDF whose page has `/Annots [6 0 R …]` for each of `annots` (objects from 6 on)
/// and whose AcroForm lists the widgets among them.
fn pdf(annots: &[&str]) -> Vec<u8> {
    let refs: Vec<String> = (0..annots.len()).map(|i| format!("{} 0 R", i + 6)).collect();
    let fields: Vec<String> = annots.iter().enumerate().filter(|(_, a)| a.contains("/Widget")).map(|(i, _)| format!("{} 0 R", i + 6)).collect();
    let mut objs: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{}] >> >>", fields.join(" ")),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Annots [{}] >>", refs.join(" ")),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
        "<< /Type /XObject /Subtype /Form /BBox [0 0 200 100] /Length 0 >>\nstream\n\nendstream".into(),
    ];
    objs.extend(annots.iter().map(|o| o.to_string()));
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

fn open(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("animate.pdf", None, bytes).expect("opens");
        app
    });
    h.run_steps(4);
    h
}

/// A frame of the player: a read-only push button (`/Ff 65537`), hidden unless `shown`.
fn frame(name: &str, shown: bool) -> String {
    let hidden = if shown { 4 } else { 6 };
    format!("<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65537 /T ({name}) /F {hidden} /Rect [50 50 250 150] /P 3 0 R /AP << /N 5 0 R >> >>")
}

const SCREEN: &str = "<< /Type /Annot /Subtype /Screen /Rect [50 50 250 150] /P 3 0 R \
     /AA << /PO << /S /JavaScript /JS (app.setInterval\\('a0_gotoNext\\(\\)', 500\\);) >> >> >>";

#[test]
fn push_buttons_alone_show_no_form_bar_and_a_screen_annotation_is_not_a_comment() {
    let (a, b, c) = (frame("0.0", true), frame("0.1", false), frame("0.2", false));
    let h = open(pdf(&[&a, &b, &c, SCREEN]));
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert_eq!(doc.info.fields.len(), 3, "the frames are still fields");
        assert!(doc.info.fields.iter().all(|f| f.kind == pdfcraft_render::FieldKind::PushButton));
        assert!(doc.info.annotations.is_empty(), "no comments: {:?}", doc.info.annotations);
    }
    assert!(h.query_by_label_contains("interactive form fields").is_none(), "nothing to fill in, so no form bar");
    assert!(h.query_by_label("Highlight fields").is_none());
}

#[test]
fn a_fillable_field_beside_push_buttons_still_shows_the_form_bar() {
    let button = frame("0.0", true);
    let text = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 160 200 180] /P 3 0 R >>";
    let h = open(pdf(&[&button, text, SCREEN]));
    h.get_by_label_contains("This document contains 2 interactive form fields.");
    h.get_by_label("Highlight fields");
}
