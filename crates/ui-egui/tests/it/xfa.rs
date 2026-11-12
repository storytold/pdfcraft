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
fn a_dynamic_xfa_form_says_when_laying_it_out_took_its_usage_rights_off() {
    let xdp = pdfcraft_xfa::fixtures::template(2);
    let h = open(pdfcraft_xfa::fixtures::usage_rights_shell(&xdp, "", false));
    // The note starts its own sentence after the layout's.
    h.get_by_label_contains("laid out from its template: 2 pages, 11 fields. The form's usage rights were removed");
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

/// Clicking an XFA button in the app runs its click script: one adds a row, another shows its
/// message as a notice.
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

/// Every way the focus moves (a click, Tab, Shift+Tab, Escape) runs the field it left's exit
/// script and the field it reached's enter script, changed or not; typing a change and tabbing
/// out runs the exit once, not twice. A script's message shows as a notice.
#[test]
fn xfa_enter_and_exit_scripts_run_whenever_the_focus_moves() {
    use egui::{Key, Modifiers};
    let mut h = open(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::focus_template()));
    crate::support::settle(&mut h);
    let counts = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let v = |n: &str| doc.form.iter().find(|f| f.name == n).and_then(|f| f.value.first().cloned()).unwrap_or_default();
        (v("enters"), v("exits"))
    };
    let focus = |h: &Harness<'static, PdfCraftApp>| h.state().views[0].forms.focus.as_ref().map(|f| f.name.clone());
    let press = |h: &mut Harness<'static, PdfCraftApp>, modifiers, key| {
        h.key_press_modifiers(modifiers, key);
        h.run_steps(4);
    };
    click_field(&mut h, "probe");
    assert_eq!(counts(&h), ("1".into(), "0".into()), "a click in runs the enter");
    for _ in 0..2 {
        press(&mut h, Modifiers::NONE, Key::Tab);
        assert_eq!(focus(&h).as_deref(), Some("quiet"));
        press(&mut h, Modifiers::SHIFT, Key::Tab);
        assert_eq!(focus(&h).as_deref(), Some("probe"));
    }
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert_eq!(focus(&h), None);
    assert_eq!(counts(&h), ("3".into(), "3".into()), "three unchanged visits, three exits; console: {:?}", h.state().js_console.log);
    // A change typed in: tabbing out runs the exit once.
    click_field(&mut h, "probe");
    h.event(egui::Event::Text("typed".into()));
    h.run_steps(2);
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(counts(&h), ("4".into(), "4".into()));
    // Leaving `quiet` shows its message.
    assert_eq!(focus(&h).as_deref(), Some("quiet"));
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(h.query_by_label_contains("left quiet").is_some(), "console: {:?}", h.state().js_console.log);
}

/// An enter script that changes the field it enters (clearing a "Type here") shows in the editor,
/// so leaving the field keeps the script's value rather than writing the old one back; typing
/// after it is kept as usual. When the exit of the field left changed it first (`relay` puts the
/// "Type here" back), the editor follows both scripts and ends on the enter script's value.
#[test]
fn an_enter_script_that_changes_its_field_shows_in_the_editor() {
    use egui::{Key, Modifiers};
    let mut h = open(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::focus_template()));
    crate::support::settle(&mut h);
    let value = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name == "hint").and_then(|f| f.value.first().cloned()).unwrap_or_default()
    };
    assert_eq!(value(&h), "Type here");
    click_field(&mut h, "hint");
    assert_eq!(value(&h), "", "the enter script cleared it");
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.text.clone()).as_deref(), Some(""), "and the editor shows that");
    h.key_press_modifiers(Modifiers::NONE, Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h), "", "leaving it doesn't write \"Type here\" back");
    click_field(&mut h, "hint");
    h.event(egui::Event::Text("Ann".into()));
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::NONE, Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h), "Ann");
    // From `relay` back to `hint`: relay's exit puts "Type here" back over "Ann", then hint's enter
    // clears it. The editor follows both, so it holds the last value, and Tab keeps it.
    click_field(&mut h, "relay");
    click_field(&mut h, "hint");
    assert_eq!(value(&h), "", "relay's exit, then hint's enter");
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.text.clone()).as_deref(), Some(""), "the editor holds the enter script's value");
    h.key_press_modifiers(Modifiers::NONE, Key::Tab);
    h.run_steps(4);
    assert_eq!(value(&h), "");
}

/// A message an initialize script shows on opening appears at once, and a print request from it
/// is refused (only a click may print).
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

/// On an encrypted dynamic XFA form that allows filling, the notice bar shows the layout note and
/// the security sentence together, with both the field-highlight and Security settings buttons.
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

/// Opening another document doesn't change the field highlight the document in front's scripts
/// read: the new one opens with the remembered Highlight fields in its own open context. Before,
/// the open set the session's value, so an edit of the first document handled before the next
/// frame read the new document's default as `app.runtimeHighlight`.
#[test]
fn opening_another_document_keeps_the_highlight_the_one_in_front_reads() {
    let script = |js: &str| format!(r#"<event activity="click"><script contentType="application/x-javascript">{js}</script></event>"#);
    let xdp = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <field name="look" w="1in" h="0.3in"><ui><button/></ui>{look}</field>
 <field name="seen" w="2in" h="0.3in"><ui><textEdit/></ui></field>
</subform>
</template>
</xdp:xdp>
"##,
        look = script("xfa.form.form.seen.rawValue = String(app.runtimeHighlight);"),
    );
    let mut h = open(pdfcraft_xfa::fixtures::shell(&xdp));
    let app = h.state_mut();
    // The document in front has Highlight fields on; the remembered setting is off.
    app.view_defaults.highlight_fields = false;
    app.views[0].highlight_fields = true;
    h.run_steps(2);
    let app = h.state_mut();
    let first = app.views[0].id;
    app.open_bytes("next.pdf", None, pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::template(1))).unwrap();
    // Before the next frame, the first document's script reads its own setting.
    app.session.apply(first, pdfcraft_engine::Edit::XfaEvent { som: "form[0].look[0]".into(), activity: "click".into() }).unwrap();
    let seen = app.session.get(first).unwrap().form.iter().find(|f| f.name == "seen").map(|f| f.value.clone()).unwrap_or_default();
    assert_eq!(seen, vec!["true".to_string()], "the first document's script reads its own Highlight fields");
}

/// A form that turns field highlighting on (`app.runtimeHighlight = true` in a `layout:ready`
/// script) gets it, and so does the next document, as with the Highlight fields button.
#[test]
fn a_form_that_turns_field_highlighting_on_gets_it() {
    let mut h = open(pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::layout_ready_template()));
    h.run_steps(4);
    assert!(h.state().views[0].highlight_fields, "the form's fields are highlighted");
    h.state_mut().open_bytes("next.pdf", None, pdfcraft_xfa::fixtures::shell(&pdfcraft_xfa::fixtures::template(1))).unwrap();
    h.run_steps(4);
    assert!(h.state().views[1].highlight_fields, "the next document opens highlighted");
}

/// A form with `k2` and `k3` calculated from `k1`, calculations the form only recommends
/// (`<calculate override="warning">`).
fn recommended_calculation() -> Vec<u8> {
    pdfcraft_xfa::fixtures::shell(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="s" layout="tb">
  <field name="k1" w="2in" h="0.3in"><ui><numericEdit/></ui><value><decimal>1</decimal></value></field>
  <field name="k2" w="2in" h="0.3in"><ui><numericEdit/></ui><calculate override="warning"><script contentType="application/x-javascript">k1.rawValue * 10</script></calculate></field>
  <field name="k3" w="2in" h="0.3in"><ui><numericEdit/></ui><calculate override="warning"><script contentType="application/x-javascript">k1.rawValue * 7</script></calculate></field>
 </subform>
</subform>
</template>
</xdp:xdp>
"##,
    )
}

fn field_name(h: &Harness<'static, PdfCraftApp>, field: &str) -> String {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name.ends_with(field)).unwrap().name.clone()
}

fn type_value(h: &mut Harness<'static, PdfCraftApp>, field: &str, value: &str) {
    let name = field_name(h, field);
    h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldValue { name, value: pdfcraft_engine::FieldValue::Text(value.into()) });
    h.run_steps(3);
}

/// The fields the calculation prompts are waiting to ask about, in order.
fn asked(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    h.state().pending_calc_overrides.iter().map(|(_, f, _)| f.rsplit('.').next().unwrap_or(f).to_string()).collect()
}

fn value(h: &Harness<'static, PdfCraftApp>, field: &str) -> String {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().form.iter().find(|f| f.name.ends_with(field)).unwrap().value.join("")
}

/// Typing over a recommended calculation asks whether to keep the value: keeping it leaves the
/// field alone when the calculation's inputs change; using the calculated value (the default,
/// also what Escape chooses) gives the field its calculated value again.
#[test]
fn typing_over_a_recommended_calculation_asks_whether_to_keep_the_value() {
    let mut h = open(recommended_calculation());
    assert_eq!(value(&h, "k2"), "10");
    type_value(&mut h, "k2", "5");
    assert_eq!(asked(&h), ["k2"]);
    h.get_by_label("Replace the calculated value?");
    if let Ok(dir) = std::env::var("PDFCRAFT_XFA_SHOTS") {
        let _gpu = crate::gpu();
        h.render().unwrap().save(std::path::Path::new(&dir).join("calculation-override-prompt.png")).unwrap();
    }
    h.get_by_label("Keep my value").click();
    h.run_steps(3);
    assert!(asked(&h).is_empty());
    type_value(&mut h, "k1", "2");
    assert_eq!(value(&h, "k2"), "5", "kept over the calculation");

    let mut h = open(recommended_calculation());
    type_value(&mut h, "k2", "5");
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    assert_eq!((value(&h, "k2"), asked(&h).is_empty()), ("10".to_string(), true));
    type_value(&mut h, "k1", "3");
    assert_eq!(value(&h, "k2"), "30", "calculated again");

    let mut h = open(recommended_calculation());
    type_value(&mut h, "k2", "5");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert_eq!(value(&h, "k2"), "10");
}

/// Using the calculated value goes back to it in the document it was typed into, even when
/// another document is in front by then; and in that field only: an edit made before the
/// answer (as the other pane's can be, in a split view) stays.
#[test]
fn using_the_calculated_value_changes_that_field_only() {
    let mut h = open(recommended_calculation());
    type_value(&mut h, "k2", "5");
    assert_eq!(asked(&h), ["k2"]);
    h.state_mut().open_bytes("other.pdf", None, recommended_calculation()).unwrap();
    h.run_steps(4);
    assert_ne!(h.state().active, Some(0), "the other document is in front");
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    assert_eq!(value(&h, "k2"), "10", "the entry was undone in its own document");
    assert_eq!(h.state().active, Some(0), "its tab comes to the front, so what its page holds is applied to it");

    let mut h = open(recommended_calculation());
    type_value(&mut h, "k2", "5");
    // Another edit lands before the answer (as the other pane's can in a split view).
    type_value(&mut h, "k1", "2");
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    assert_eq!((value(&h, "k1"), value(&h, "k2")), ("2".to_string(), "20".to_string()), "the later edit stays; the field is calculated from it");
    assert!(asked(&h).is_empty());
}

/// While the question about a calculated field is shown, no editor stays open on the page: one
/// open on the field, holding the typed value, is closed under the question, so after the
/// answer nothing is left to type over the calculated value again.
#[test]
fn no_editor_stays_open_on_the_field_while_the_calculation_question_is_shown() {
    let mut h = open(recommended_calculation());
    // k2 is the second 0.3in row of the page, 0.5in in: its middle at (1.5in, 0.95in).
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let p = r.min + egui::vec2(r.width() * 108.0 / 612.0, r.height() * 68.4 / 792.0);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
    let k2 = field_name(&h, "k2");
    assert_eq!(h.state().views[0].forms.focus.as_ref().map(|f| f.name.clone()), Some(k2.clone()), "the editor is open on k2");
    let mut editor = h.state().views[0].forms.focus.clone().unwrap();
    type_value(&mut h, "k2", "5");
    assert_eq!(asked(&h), ["k2"]);
    // An editor open on the field again, holding the typed value.
    editor.text = "5".into();
    h.state_mut().views[0].forms.focus = Some(editor);
    h.run_steps(1);
    assert!(h.state().views[0].forms.focus.is_none(), "closed under the question");
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    assert_eq!(value(&h, "k2"), "10");
    assert!(h.state().views[0].forms.focus.is_none());
}

/// One edit that types over two recommended calculations (a batch of values) asks about each
/// field in turn (XFA 3.3 p. 642: the warning is about the form object the user set). Using the
/// calculated value for the first gives that field its calculated value and leaves the second
/// field's entry, and the edit's other value, as typed; the second is asked about next.
#[test]
fn each_calculation_one_edit_types_over_is_asked_about_in_turn() {
    let mut h = open(recommended_calculation());
    let text = |v: &str| pdfcraft_engine::FieldValue::Text(v.into());
    let edits = vec![
        pdfcraft_engine::Edit::SetFieldValue { name: field_name(&h, "k1"), value: text("2") },
        pdfcraft_engine::Edit::SetFieldValue { name: field_name(&h, "k2"), value: text("5") },
        pdfcraft_engine::Edit::SetFieldValue { name: field_name(&h, "k3"), value: text("6") },
    ];
    h.state_mut().apply_edit(pdfcraft_engine::Edit::Batch { label: "Fill in form".into(), edits });
    h.run_steps(3);
    assert_eq!(asked(&h), ["k2", "k3"]);
    assert_eq!(["k1", "k2", "k3"].map(|f| value(&h, f)), ["2", "5", "6"]);
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    assert_eq!(["k1", "k2", "k3"].map(|f| value(&h, f)), ["2", "20", "6"], "the first field only");
    assert_eq!(asked(&h), ["k3"], "the second field is asked about next");
    h.get_by_label("Replace the calculated value?");
    h.get_by_label("Keep my value").click();
    h.run_steps(3);
    assert!(asked(&h).is_empty());
    type_value(&mut h, "k1", "3");
    assert_eq!(["k2", "k3"].map(|f| value(&h, f)), ["30", "6"], "the first calculated again, the second kept");
}

/// A new text box that can't be applied: it names a page the document doesn't have (as a box
/// left open on a page removed since would), so adding its text is refused.
fn unappliable_box() -> pdfcraft_ui_egui::TextDraft {
    pdfcraft_ui_egui::TextDraft {
        page: 99,
        index: None,
        rect: [20.0, 600.0, 220.0, 620.0],
        text: "Typed".into(),
        style: Default::default(),
        focus: false,
        selection: None,
        typing_from: None,
    }
}

/// "Use calculated value" while a box open on the page can't be applied changes nothing and
/// keeps the question: it waits, not shown, while the box stays open, and is asked again once
/// the box is cancelled; answered then, the field takes its calculated value. The note says the
/// value stays for now, not to use Undo (which takes back the last edit, not this entry).
#[test]
fn the_calculation_question_waits_for_a_box_that_cant_be_applied() {
    let mut h = open(recommended_calculation());
    type_value(&mut h, "k2", "5");
    h.state_mut().views[0].content.draft = Some(unappliable_box());
    h.run_steps(1);
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    assert_eq!(value(&h, "k2"), "5");
    assert_eq!(asked(&h), ["k2"], "the question is kept");
    assert!(h.query_by_label("Replace the calculated value?").is_none(), "not shown while the box is open");
    let toast = h.state().toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(toast.contains("stays for now") && !toast.contains("Undo"), "{toast}");
    h.state_mut().views[0].content.draft = None;
    h.run_steps(3);
    h.get_by_label("Replace the calculated value?");
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    assert_eq!(value(&h, "k2"), "10");
    assert!(asked(&h).is_empty());
}

/// A question about a value no longer kept over its calculation (a reset took it back while
/// the question waited, or before it was answered) needs no answer: it isn't asked again, and
/// answering it changes nothing and reports nothing.
#[test]
fn a_question_a_reset_answered_is_dropped() {
    let mut h = open(recommended_calculation());
    type_value(&mut h, "k2", "5");
    h.state_mut().views[0].content.draft = Some(unappliable_box());
    h.run_steps(1);
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    h.state_mut().apply_edit(pdfcraft_engine::Edit::ResetForm { names: None });
    h.run_steps(3);
    assert_eq!(value(&h, "k2"), "10");
    h.state_mut().views[0].content.draft = None;
    h.run_steps(3);
    assert!(asked(&h).is_empty());
    assert!(h.query_by_label("Replace the calculated value?").is_none());

    let mut h = open(recommended_calculation());
    type_value(&mut h, "k2", "5");
    h.state_mut().apply_edit(pdfcraft_engine::Edit::ResetForm { names: None });
    h.run_steps(3);
    h.state_mut().toast = None;
    h.get_by_label("Use calculated value").click();
    h.run_steps(3);
    assert_eq!(value(&h, "k2"), "10");
    assert!(asked(&h).is_empty());
    assert!(h.state().toast.is_none(), "{:?}", h.state().toast);
}
