//! XFA forms (#60): not read yet, so the document says so instead of silently showing a
//! placeholder page or fields whose values Acrobat would override.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// A one-page PDF with a proper xref; `acroform` is the catalog's /AcroForm (or empty), `extra`
/// more catalog entries, `objects` extra objects numbered from 5.
fn pdf(acroform: &str, extra: &str, objects: &[&str]) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {acroform} {extra} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R >>".into(),
        "<< /Length 0 >>\nstream\n\nendstream".into(),
    ];
    objs.extend(objects.iter().map(|o| o.to_string()));
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

const XFA_PACKET: &str = "<< /Length 52 >>\nstream\n<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"></xdp:xdp>\nendstream";

fn open(bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("form.pdf", None, bytes).expect("opens");
        app
    });
    h.run_steps(4);
    h
}

fn xfa(h: &Harness<'static, PdfCraftApp>) -> Option<pdfcraft_render::Xfa> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.xfa
}

#[test]
fn a_dynamic_xfa_form_says_its_page_is_a_placeholder() {
    // No fields, the form lives in the XFA packets (an array of name/stream pairs here).
    let h = open(pdf("/AcroForm << /Fields [] /XFA [(template) 5 0 R] >>", "/NeedsRendering true", &[XFA_PACKET]));
    assert_eq!(xfa(&h), Some(pdfcraft_render::Xfa::Dynamic));
    h.get_by_label_contains("dynamic XFA form");
    // Without /NeedsRendering, no fields still means nothing to fill but the placeholder.
    let h = open(pdf("/AcroForm << /Fields [] /XFA 5 0 R >>", "", &[XFA_PACKET]));
    assert_eq!(xfa(&h), Some(pdfcraft_render::Xfa::Dynamic));
}

#[test]
fn a_static_xfa_form_can_be_filled_but_warns_about_its_xfa_data() {
    let field = "<< /FT /Tx /T (name) /Rect [20 20 200 40] /Type /Annot /Subtype /Widget /P 3 0 R >>";
    let h = open(pdf("/AcroForm << /Fields [6 0 R] /XFA 5 0 R >>", "", &[XFA_PACKET, field]));
    assert_eq!(xfa(&h), Some(pdfcraft_render::Xfa::Static));
    h.get_by_label_contains("also contains XFA data");
    // The fields can still be highlighted from the notice.
    h.get_by_label("Highlight fields");
}

#[test]
fn ordinary_forms_and_documents_have_no_xfa_notice() {
    let field = "<< /FT /Tx /T (name) /Rect [20 20 200 40] /Type /Annot /Subtype /Widget /P 3 0 R >>";
    let h = open(pdf("/AcroForm << /Fields [5 0 R] >>", "", &[field]));
    assert_eq!(xfa(&h), None);
    assert!(h.query_by_label_contains("XFA").is_none());
    h.get_by_label_contains("interactive form fields");
    // A malformed /XFA (a number) still counts as XFA; it never crashes the open.
    let h = open(pdf("/AcroForm << /Fields [] /XFA 42 >>", "", &[]));
    assert_eq!(xfa(&h), Some(pdfcraft_render::Xfa::Dynamic));
}
