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
    objs.extend(objects.iter().map(ToString::to_string));
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
    // The notice offers the highlight toggle, like any form; highlights are on by default.
    h.get_by_label("Hide field highlights");
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
    // The highlight toggle (on by default, so it reads "Hide") is still in the notice.
    h.get_by_label("Hide field highlights");
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
    crate::support::settle(&mut h);
    let names = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().map(|f| f.name.clone()).collect::<Vec<_>>()
    };
    assert!(!names(&h).iter().any(|n| n == "amount_2"));
    click_field(&mut h, "addRow");
    assert!(names(&h).iter().any(|n| n == "amount_2"), "{:?}; script console: {:?}", names(&h), h.state().js_console.log);
    // The message box shows as a notice.
    click_field(&mut h, "hello");
    assert!(h.query_by_label_contains("Hello 2").is_some(), "script console: {:?}", h.state().js_console.log);
}

#[test]
fn messages_from_scripts_run_on_open_show_right_away() {
    let tpl = pdfcraft_xfa::fixtures::scripted_template()
        .replace("if (qty.rawValue === null) qty.rawValue = 2;", r#"xfa.host.messageBox("Welcome to the form"); xfa.host.print();"#);
    let mut h = open(pdfcraft_xfa::fixtures::shell(&tpl));
    h.run_steps(2);
    assert!(h.query_by_label_contains("Welcome to the form").is_some(), "script console: {:?}", h.state().js_console.log);
    assert_eq!(h.state().dialog, None, "an initialize script can't open the Print dialog");
    let id = h.state().views[0].id;
    assert!(h.state_mut().session.take_js_output(id).is_empty(), "nothing waits for the next edit");
}

/// A script the engine's own limits don't stop (its loops are in nested calls): it runs until
/// it is abandoned, which turns the form's scripts off.
const RUNAWAY: &str = "function f() { for (var j = 0; j &lt; 99999; j++) {} } for (var k = 0; k &lt; 5000; k++) f();";
const SCRIPTS_OFF: &str = "This form's scripts are turned off because one ran too long";

fn says_scripts_are_off(toast: &Option<(String, f64)>) -> bool {
    toast.as_ref().is_some_and(|t| t.0.contains(SCRIPTS_OFF))
}

#[test]
fn a_form_whose_scripts_were_turned_off_at_open_says_so() {
    let tpl = pdfcraft_xfa::fixtures::scripted_template().replace("if (qty.rawValue === null) qty.rawValue = 2;", RUNAWAY);
    let mut h = open(pdfcraft_xfa::fixtures::shell(&tpl));
    let id = h.state().views[0].id;
    assert!(h.state().session.get(id).unwrap().xfa_scripts_off(), "script console: {:?}", h.state().js_console.log);
    // A toast says so as it happens; the notice bar keeps saying so after the toast is gone.
    assert!(says_scripts_are_off(&h.state().toast), "{:?}", h.state().toast);
    h.state_mut().toast = None;
    h.run_steps(2);
    assert!(h.query_by_label_contains(SCRIPTS_OFF).is_some());
    // The bar leads to the console, where the script's error is.
    h.get_by_label("JavaScript Console").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::JsConsole));
    assert!(h.state().js_console.log.iter().any(|l| l.contains("abandoned")), "{:?}", h.state().js_console.log);
}

#[test]
fn scripts_turned_off_by_a_click_bring_a_closed_notice_bar_back() {
    let tpl = pdfcraft_xfa::fixtures::scripted_template().replace(r#"xfa.host.messageBox("Hello " + qty.rawValue);"#, RUNAWAY);
    let mut h = open(pdfcraft_xfa::fixtures::shell(&tpl));
    crate::support::settle(&mut h);
    // The form opens with its scripts on, and the user closes the notice bar.
    let id = h.state().views[0].id;
    assert!(!h.state().session.get(id).unwrap().xfa_scripts_off());
    assert!(h.query_by_label_contains(SCRIPTS_OFF).is_none() && !says_scripts_are_off(&h.state().toast));
    h.get_by_label("Dismiss").click();
    h.run_steps(2);
    assert!(h.state().views[0].notice_dismissed);
    // The button's script runs away: a toast says the scripts are off and the bar is back.
    click_field(&mut h, "hello");
    assert!(h.state().session.get(id).unwrap().xfa_scripts_off(), "script console: {:?}", h.state().js_console.log);
    assert!(says_scripts_are_off(&h.state().toast), "{:?}", h.state().toast);
    assert!(!h.state().views[0].notice_dismissed);
    h.state_mut().toast = None;
    h.run_steps(2);
    assert!(h.query_by_label_contains(SCRIPTS_OFF).is_some());
    // It is said once: closed again, the bar stays closed through what the user does next.
    h.get_by_label("Dismiss").click();
    h.run_steps(2);
    click_field(&mut h, "addRow");
    assert!(h.state().views[0].notice_dismissed);
    assert!(!says_scripts_are_off(&h.state().toast), "{:?}", h.state().toast);
}

#[test]
fn a_secured_dynamic_xfa_form_shows_its_layout_and_its_security() {
    // Changing and assembling withheld, filling allowed.
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::template(2)))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1036,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    let h = open(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
    h.get_by_label_contains("laid out from its template: 2 pages, 11 fields. This document is secured.");
    h.get_by_label("Hide field highlights");
    h.get_by_label("Security settings");
}
