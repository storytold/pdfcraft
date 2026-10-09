//! XFA forms (#60): a dynamic form is laid out from its template and says so; a static one takes
//! its values from the XFA data; one whose template can't be read says that instead of silently
//! showing a placeholder page.

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
        app.set_option("language", "en").unwrap();
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
fn a_dynamic_xfa_form_is_laid_out_and_filled() {
    let h = open(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::template(2)));
    assert_eq!(xfa(&h), Some(pdfcraft_render::Xfa::Dynamic));
    h.get_by_label_contains("laid out from its template: 2 pages, 11 fields");
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 2);
    assert!(doc.form.iter().any(|f| f.name == "familyName"));
    // The notice offers to highlight the fields, like any form.
    h.get_by_label("Highlight fields");
}

#[test]
fn a_dynamic_xfa_form_says_its_page_is_a_placeholder() {
    // No fields, the form lives in the XFA packets (an array of name/stream pairs here), but
    // the packets hold no template: the placeholder page stays, with a notice.
    let h = open(pdf("/AcroForm << /Fields [] /XFA [(template) 5 0 R] >>", "/NeedsRendering true", &[XFA_PACKET]));
    assert_eq!(xfa(&h), Some(pdfcraft_render::Xfa::Dynamic));
    h.get_by_label_contains("dynamic XFA form");
    // Without /NeedsRendering, no fields still means nothing to fill but the placeholder.
    let h = open(pdf("/AcroForm << /Fields [] /XFA 5 0 R >>", "", &[XFA_PACKET]));
    assert_eq!(xfa(&h), Some(pdfcraft_render::Xfa::Dynamic));
}

#[test]
fn a_static_xfa_form_is_filled_from_its_data_and_says_so() {
    let field = "<< /FT /Tx /T (name) /Rect [20 20 200 40] /Type /Annot /Subtype /Widget /P 3 0 R >>";
    let h = open(pdf("/AcroForm << /Fields [6 0 R] /XFA 5 0 R >>", "", &[XFA_PACKET, field]));
    assert_eq!(xfa(&h), Some(pdfcraft_render::Xfa::Static));
    h.get_by_label_contains("XFA data are kept in step");
    // Values saved in the datasets by another viewer show up in the fields.
    let h = open(pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Ada</name><agree>1</agree></page1></form1>"));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.form.iter().find(|f| f.name == "form1[0].page1[0].name[0]").unwrap().value, vec!["Ada".to_string()]);
    assert_eq!(doc.form.iter().find(|f| f.name == "form1[0].page1[0].agree[0]").unwrap().value, vec!["1".to_string()]);
    assert!(!doc.dirty);
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

/// Click the centre of a field's widget.
fn click_field(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let p = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let f = doc.form.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no field {name}"));
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&s.views[0], &doc.info, f, 0).expect("on screen").center()
    };
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
}

#[test]
fn xfa_buttons_run_their_scripts_in_the_app() {
    let mut h = open(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::scripted_template()));
    for _ in 0..40 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let names = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().map(|f| f.name.clone()).collect::<Vec<_>>()
    };
    assert!(!names(&h).iter().any(|n| n == "amount_2"));
    click_field(&mut h, "addRow");
    assert!(names(&h).iter().any(|n| n == "amount_2"), "{:?}", names(&h));
    // The message box shows as a notice.
    click_field(&mut h, "hello");
    h.get_by_label_contains("Hello 2");
}

#[test]
fn messages_from_scripts_run_on_open_show_right_away() {
    let tpl = pdfcraft_xfa::fixtures::scripted_template()
        .replace("if (qty.rawValue === null) qty.rawValue = 2;", r#"xfa.host.messageBox("Welcome to the form"); xfa.host.print();"#);
    let mut h = open(pdfcraft_xfa::fixtures::shell(&tpl));
    h.run_steps(2);
    h.get_by_label_contains("Welcome to the form");
    assert_eq!(h.state().dialog, None, "an initialize script can't open the Print dialog");
    let id = h.state().views[0].id;
    assert!(h.state_mut().session.take_js_output(id).is_empty(), "nothing waits for the next edit");
}
