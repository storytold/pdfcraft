//! Acrobat JavaScript in the shell (egui_kittest): the console, Document JavaScripts, a button
//! script and Preferences ▸ JavaScript.

use craft::theme::variant::Tone;
use craft::ui::runtime::{env::WindowKey, toast};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
// PLAN: assumed `pdfcraft_ui_egui::JsDialog` with `DocumentScripts(DocJsDraft)` and `Console` (pdf-forms-w17)
use crate::support::AppHarness as _;
use pdfcraft_ui_egui::{JsDialog, PdfCraftApp};

/// A form with a text field and a push button whose Mouse Up script fills it in.
fn form() -> Vec<u8> {
    form_with_text_action("")
}

fn form_with_text_action(action: &str) -> Vec<u8> {
    let greeting = format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (greeting) /Rect [20 250 280 270] /P 3 0 R /F 4 {action} >>");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 300] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        &greeting,
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (hello) /Rect [20 200 120 230] /P 3 0 R /F 4 /A << /S /JavaScript /JS (getField\\('greeting'\\).value = 'Hello from JavaScript'; app.alert\\('Filled in'\\);) >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
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

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "form.pdf", None, form()).unwrap();
            app
        }
    });
    h.run_steps(4);
    h
}

fn value(h: &Harness<'static, PdfCraftApp>, name: &str) -> Vec<String> {
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    app.session.get(id).unwrap().form.iter().find(|f| f.name == name).unwrap().value.clone()
}

#[test]
fn console_runs_scripts_against_the_document() {
    let mut h = harness();
    assert!(h.execute("tools.js_console"));
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "JavaScript Console");
    h.state_mut().js_console.input = "getField('greeting').value = 'typed'; console.println(numFields); 6 * 7".into();
    h.get_by_label("Run").click();
    h.run_steps(2);
    assert_eq!(value(&h, "greeting"), ["typed"]);
    let log = h.state().js_console.log.clone();
    assert!(log.contains(&"2".to_string()) && log.contains(&"42".to_string()), "{log:?}");
    h.state_mut().js_console.input = "throw new Error('boom')".into();
    h.run_steps(4);
    h.get_by_label("Run").click();
    h.run_steps(2);
    assert!(h.state().js_console.log.last().unwrap().contains("boom"));
    h.run_steps(4);
    h.get_by_label("Clear").click();
    h.run_steps(2);
    assert!(h.state().js_console.log.is_empty(), "{:?}", h.state().js_console.log);
}

/// A button's script fills a field and its message takes the toast.
#[test]
fn button_scripts_run_and_alert() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    h.run_button_script(id, "hello", "getField('greeting').value = 'Hello from JavaScript'; app.alert('Filled in');");
    wait_for(&mut h, "Filled in");
    h.get_by_label("OK").click();
    wait_value(&mut h, "greeting", "Hello from JavaScript");
}

#[test]
fn text_widget_mouse_up_runs_its_menu_and_keeps_the_result() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
crate::support::install_runtime(&cc.egui_ctx);
{
        let mut app = crate::control::new_app();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.set_option(&egui::Context::default(), "left", "closed").unwrap();
        let bytes = form_with_text_action(
            r"/AA << /U << /S /JavaScript /JS (var choice = app.popUpMenuEx\({cName:'Choose test value',cReturn:'selected'}\); if \(choice\) getField\('greeting'\).value = choice;) >> >>",
        );
        app.open_bytes(&egui::Context::default(), "text-action.pdf", None, bytes).unwrap();
        app
    }
});
    crate::support::settle(&mut h);
    let point = {
        let app = h.state();
        let (index, id) = app.active_ids().unwrap();
        let doc = app.session.get(id).unwrap();
        let field = doc.form.iter().find(|field| field.name == "greeting").unwrap();
        assert!(field.widgets[0].mouse_up_script.is_some());
        pdfcraft_ui_egui::forms_ui::field_screen_rect(&app.views[index], &doc.info, field, 0).unwrap().center()
    };
    for _ in 0..2 {
        h.hover_at(point);
        h.run_steps(1);
        h.drag_at(point);
        h.run_steps(1);
        h.drop_at(point);
        wait_for(&mut h, "Choose test value");
        h.get_by_label("Choose test value").click();
        wait_value(&mut h, "greeting", "selected");
        h.run_steps(8);
    }
}

/// A card as the window shows it: its title, its text and its tone.
type Shown = (Option<String>, String, Tone);

/// The cards on screen, oldest first. A notice with no message puts its one line in the text.
fn cards(h: &Harness<'static, PdfCraftApp>) -> Vec<Shown> {
    toast::cards(&h.ctx, &WindowKey::main())
        .into_iter()
        .map(|card| {
            let title = card.toast.title.as_ref().map(|t| t.as_str().to_owned());
            let message = card.toast.message.as_str().to_owned();
            if message.is_empty() { (None, title.unwrap_or_default(), card.toast.tone) } else { (title, message, card.toast.tone) }
        })
        .collect()
}

fn texts(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    cards(h).into_iter().map(|c| c.1).collect()
}

/// The user's close action on every card, then the passes that act on it.
fn dismiss_all(h: &mut Harness<'static, PdfCraftApp>) {
    // Programmatic removal emits no close event; use the user's dismiss control. The cards shown now, one at a
    // time: cards are as wide as their text, so the next card's control moves once one closes.
    let shown = h.query_all_by(|node| !node.is_hidden() && node.label().as_deref() == Some("Dismiss")).count();
    for _ in 0..shown {
        if let Some(button) = h.query_all_by(|node| !node.is_hidden() && node.label().as_deref() == Some("Dismiss")).next() {
            button.click();
        }
        h.run_steps(3);
    }
}

/// One run's messages are all shown, in order (#4294): the first is a card with its title and tone
/// (#4293), the next is posted when the first ends, and a notice raised meanwhile doesn't lose the
/// ones still waiting. Messages past the queue's room are counted and reported once the rest have
/// been shown, and a count the card limit dropped is said again.
#[test]
fn every_message_a_script_shows_takes_the_toast_in_turn() {
    let mut h = harness();
    dismiss_all(&mut h);
    let id = h.state().active_ids().unwrap().1;
    let out = h
        .state_mut()
        .session
        .run_javascript(id, "app.alert('First', 1, 0, 'Order form'); app.alert('Second', 3); app.alert('Third');", None)
        .unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().handle_js(&ctx_h, id, pdfcraft_engine::js::JsOutput { alerts: out.alerts, ..Default::default() });
    h.run_steps(2);
    assert_eq!(cards(&h), [(Some("Order form".to_owned()), "First".to_owned(), Tone::Warn)]);
    assert_eq!(h.state().script_messages.len(), 2, "the others wait");
    // The first has ended: the second is posted, with its own tone and no title.
    dismiss_all(&mut h);
    assert_eq!(cards(&h), [(None, "Second".to_owned(), Tone::Info)]);
    dismiss_all(&mut h);
    assert_eq!(texts(&h), ["Third"]);
    dismiss_all(&mut h);
    assert!(cards(&h).is_empty());
    assert!(h.state().script_messages.is_empty());
    // A notice the same run raises first (its submit request) comes before the message.
    let out = h.state_mut().session.run_javascript(id, "this.submitForm('https://example.com/form'); app.alert('Fourth');", None).unwrap();
    let ctx_h = h.ctx.clone();
    h.state_mut().handle_js(&ctx_h, id, pdfcraft_engine::js::JsOutput { alerts: out.alerts, requests: out.requests, ..Default::default() });
    h.run_steps(2);
    let shown = texts(&h);
    assert!(shown.len() == 2 && shown[0].contains("submitted") && shown[1] == "Fourth", "{shown:?}");
    dismiss_all(&mut h);
    // Past the 100 the queue holds, messages are counted, and a notice says how many once the rest
    // have been shown.
    let many: Vec<_> = (0..105).map(|i| pdfcraft_engine::js::Message::plain(format!("Message {i}"))).collect();
    h.state_mut().show_script_messages(many);
    h.run_steps(2);
    assert_eq!(texts(&h), ["Message 0"]);
    for i in 1..100 {
        dismiss_all(&mut h);
        assert_eq!(texts(&h), [format!("Message {i}")]);
    }
    let left_out = "5 more messages from the form's scripts were left out.";
    dismiss_all(&mut h);
    assert_eq!(texts(&h), [left_out], "the count comes after the last message");
    // Three more notices push the count card out before it was shown its time: it is said again, once.
    for n in 1..=3 {
        h.state_mut().notify(format!("Saved {n}"));
    }
    h.run_steps(2);
    assert_eq!(texts(&h).iter().filter(|t| *t == left_out).count(), 1, "said again after it was dropped: {:?}", texts(&h));
    dismiss_all(&mut h);
    assert!(!texts(&h).iter().any(|t| t == left_out), "and once only: {:?}", texts(&h));
}

#[test]
fn document_scripts_and_preferences() {
    let mut h = harness();
    assert!(h.execute("tools.document_js"));
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Document JavaScripts");
    match h.state_mut().js_dialog.as_mut() {
        Some(JsDialog::DocumentScripts(draft)) => {
            draft.name = "init".into();
            draft.script = "function hi() { return 'hi'; }".into();
        }
        other => panic!("the Document JavaScripts dialog is not open: {other:?}"),
    }
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Document JavaScripts").get_by_label("Save").click();
    h.run_steps(2);
    let id = h.state().active_ids().unwrap().1;
    assert_eq!(h.state().session.get(id).unwrap().document_scripts()[0].0, "init");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.state_mut().open_preferences();
    h.run_steps(2);
    h.get_by_label("Enable Acrobat JavaScript").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("Enable Acrobat JavaScript").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert!(!h.state().session.javascript());
    let saved = h.state().settings_snapshot();
    let mut fresh = crate::control::new_app();
    fresh.set_option(&egui::Context::default(), "language", "en").unwrap();
    fresh.apply_settings(&saved);
    assert!(!fresh.session.javascript(), "the preference is remembered");
}

#[test]
fn merge_data_files_into_a_spreadsheet() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-merge-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("report.csv");
    // PLAN: assumed `crate::support::host_with_files(ScriptedFiles) -> Arc<Host>` (pdf-chrome-w9) and `PdfCraftApp::with_host`
    let files = std::sync::Arc::new(craft::host::ScriptedFiles::answering([craft::host::Picked::Paths(vec![out.clone()])]));
    let host = crate::support::host_with_files(files.clone());
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::support::test_app(host);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app
        }
    });
    h.run_steps(2);
    h.state_mut().merge_data_files(vec![("form.pdf".into(), form())]);
    h.run_steps(4);
    assert_eq!(files.file(&out).unwrap(), b"greeting\n\n");
    assert!(texts(&h).iter().any(|t| t.starts_with("Merged 1 file")), "{:?}", texts(&h));
    dismiss_all(&mut h);
    h.state_mut().merge_data_files(vec![("x.fdf".into(), b"junk".to_vec())]);
    h.run_steps(4);
    assert!(texts(&h).iter().any(|t| t.starts_with("x.fdf:")), "{:?}", texts(&h));
}

#[test]
fn prepare_a_form_detects_fields_on_a_paper_form() {
    let paper = pdfcraft_engine::Session::new().create_from_text("t", "Name: ____________________\n\nPhone: ____________________").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "paper.pdf", None, paper.clone()).unwrap();
            app
        }
    });
    h.run_steps(4);
    assert!(h.execute("form.prepare"));
    h.run_steps(3);
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    let names: Vec<String> = app.session.get(id).unwrap().form.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["Name", "Phone"]);
    assert!(texts(&h).iter().any(|t| t == "Detected 2 form fields"), "{:?}", texts(&h));
}

/// Advance bounded UI frames until the synthetic prompt label becomes available.
fn wait_for(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    while std::time::Instant::now() < deadline {
        h.run_steps(1);
        if h.query_by_label(label).is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("script dialog did not show {label}");
}
/// Advance bounded UI frames until the worker completion updates the expected synthetic field value.
fn wait_value(h: &mut Harness<'static, PdfCraftApp>, name: &str, expected: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    while std::time::Instant::now() < deadline {
        h.run_steps(1);
        if value(h, name) == [expected] {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(value(h, name), [expected]);
}
#[test]
/// Every native script prompt identifies its document, even with misleading or empty titles.
fn native_script_prompts_show_an_app_owned_caption() {
    for (script, body, title) in [
        ("app.response({cQuestion:'Type a value',cTitle:'PdfCraft security',bPassword:true});", "Type a value", Some("PdfCraft security")),
        ("app.response({cQuestion:'Empty title prompt',cTitle:''});", "Empty title prompt", None),
        ("app.alert({cMsg:'Script message',cTitle:'PdfCraft security'});", "Script message", Some("PdfCraft security")),
        ("app.alert({cMsg:'Untitled alert',cTitle:''});", "Untitled alert", None),
        ("app.popUpMenuEx({cName:'Pick a value',cReturn:'yes'});", "Pick a value", None),
    ] {
        let mut h = harness();
        let id = h.state().active_ids().unwrap().1;
        h.run_button_script(id, "hello", script);
        wait_for(&mut h, body);
        assert!(h.query_by_role_and_label(egui::accesskit::Role::Dialog, "This document's script is asking").is_some(), "{script}");
        assert_eq!(
            h.query_by_label("This request comes from the document, not PdfCraft. Only enter a password if you trust this document.").is_some(),
            script.contains("bPassword:true"),
            "{script}"
        );
        assert!(h.query_by_label("Document: form.pdf").is_some(), "{script}");
        if let Some(title) = title {
            assert!(h.query_by_label(title).is_some(), "{script}");
        }
        h.get_by_label("Cancel").click();
        h.run_steps(2);
    }
}

#[test]
fn native_script_prompt_uses_the_german_catalog_when_available() {
    use pdfcraft_ui_egui::i18n::{Lang, tr};

    let Some(german) = Lang::from_code("de") else { return };
    let source = "This document's script is asking";
    let heading = "Das Skript dieses Dokuments fragt";
    // Older catalog pins can still build the application with the English fallback.
    if tr(german, source) == source && std::env::var_os("PDFCRAFT_REQUIRE_SCRIPT_PROMPT_TRANSLATIONS").is_none() {
        return;
    }
    assert_eq!(tr(german, source), heading);
    let mut h = harness();
    h.set_option("language", "de").unwrap();
    let id = h.state().active_ids().unwrap().1;
    h.run_button_script(id, "hello", "app.response({cQuestion:'Test',bPassword:true});");
    wait_for(&mut h, heading);
    assert!(h.query_by_label("Dokument: form.pdf").is_some());
    // The warning in the catalog's words, which translators may reword, and never the English.
    let warning = "This request comes from the document, not PdfCraft. Only enter a password if you trust this document.";
    assert_ne!(tr(german, warning), warning, "the German catalog translates the password warning");
    assert!(h.query_by_label(tr(german, warning)).is_some());
}

#[test]
/// A real UI menu followed by input resumes once and does not leak the menu click into the next modal.
fn native_button_popup_and_response_resume_without_replaying_the_script() {
    for typed in [false, true] {
        let mut h = harness();
        let id = h.state().active_ids().unwrap().1;
        h.run_button_script(id,"hello","var calls=typeof calls==='number'?calls:0;calls++;var choice=app.popUpMenuEx({cName:'Morning',cReturn:'08:00'});var extra=app.response({cTitle:'Manual time',cQuestion:'Suffix',cDefault:'x'});getField('greeting').value=choice+'/'+extra+'/'+calls;");
        wait_for(&mut h, "Morning");
        h.get_by_label("Morning").click();
        wait_for(&mut h, "Suffix");
        if typed {
            h.get_by(|n| n.role() == egui::accesskit::Role::TextInput && n.label().as_deref() == Some("Answer") && n.value().as_deref() == Some("x"))
                .focus();
            h.run_steps(2);
            h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
            h.run_steps(1);
            h.get_by(|n| n.role() == egui::accesskit::Role::TextInput && n.is_focused()).type_text("typed answer");
            h.run_steps(2);
        }
        if let Ok(path) = std::env::var("PDFCRAFT_COMPAT_SCREENSHOT") {
            let image = h.render().unwrap();
            let crop = image::imageops::crop_imm(&image, 400, 220, 600, 460).to_image();
            crop.save(path).unwrap();
        }
        h.get_by_label("OK").click();
        wait_value(&mut h, "greeting", if typed { "08:00/typed answer/1" } else { "08:00/x/1" });
    }
}
#[test]
/// Cancelling a native menu returns null to the same invocation without duplicating preceding work.
fn native_popup_cancellation_returns_null_and_preserves_the_single_invocation() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    let script = "var calls=typeof calls==='number'?calls:0;calls++;var choice=app.popUpMenuEx({cName:'Choose',cReturn:'yes'});getField('greeting').value=String(choice)+'/'+calls;";
    h.run_button_script(id, "hello", script);
    wait_for(&mut h, "Choose");
    h.get_by_label("Cancel").click();
    wait_value(&mut h, "greeting", "null/1");
    h.run_button_script(id, "hello", script);
    wait_for(&mut h, "Choose");
    h.get_by_label("Cancel").click();
    wait_value(&mut h, "greeting", "null/2");
}

/// A nested menu exposes only enabled leaves as valid replies to the waiting script.
#[test]
fn native_popup_submenu_keeps_disabled_items_unselectable() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    let script = "var calls=typeof calls==='number'?calls:0; calls++; \
                  var choice=app.popUpMenuEx({cName:'Group',oSubMenu:[{cName:'Disabled',bEnabled:false}, \
                  {cName:'Enabled',cReturn:'chosen'}]}); getField('greeting').value=choice+'/'+calls;";
    h.run_button_script(id, "hello", script);
    wait_for(&mut h, "Group");
    h.get_by_label("Group").click();
    wait_for(&mut h, "Enabled");
    // The modal recentres while the submenu expands. Settle the layout before sending pointer input.
    h.run_steps(30);
    assert!(h.query_by(|node| node.label().as_deref() == Some("Disabled") && node.is_disabled()).is_some());
    h.get_by_label("Enabled").click();
    wait_value(&mut h, "greeting", "chosen/1");
}

#[test]
/// An intervening document edit prevents an older pending script result from replacing current values.
fn native_popup_completion_is_discarded_after_the_document_is_edited() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    h.run_button_script(id, "hello", "app.popUpMenuEx({cName:'Wait',cReturn:'yes'});getField('greeting').value='stale';");
    wait_for(&mut h, "Wait");
    h.state_mut()
        .session
        .apply(id, pdfcraft_engine::Edit::SetFieldValue { name: "greeting".into(), value: pdfcraft_engine::FieldValue::Text("current".into()) })
        .unwrap();
    h.run_steps(5);
    assert!(h.query_by_label("Wait").is_none(), "the stale prompt must be dismissed");
    assert_eq!(value(&h, "greeting"), ["current"]);
    // The document's worker remains reserved until the abandoned prompt exits.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let out = h.state_mut().session.run_javascript(id, "var afterEdit='kept';", None).unwrap();
        let Some(error) = out.error else { break };
        assert!(out.execution_refused && error.contains("another script is active"), "{error}");
        assert!(std::time::Instant::now() < deadline, "the abandoned worker did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state_mut().session.run_javascript(id, "afterEdit", None).unwrap().result.as_deref(), Some("kept"));
}

/// Native Mouse Up must expose the same willCommit flag as the headless button path.
#[test]
fn native_button_events_match_headless_will_commit() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    let headless = h.state_mut().session.run_javascript(id, "String(event.willCommit);", Some("hello")).unwrap();
    assert_eq!(headless.result.as_deref(), Some("false"));
    h.run_button_script(id, "hello", "getField('greeting').value=String(event.willCommit);");
    wait_value(&mut h, "greeting", "false");
}

/// A real pointer click must retain the second same-named button through native completion.
#[test]
fn clicking_duplicate_named_buttons_targets_the_clicked_object() {
    let _gpu = crate::gpu();
    use pdfcraft_cos::{Dict, ObjRef, Object, PdfString};
    use std::sync::Arc;
    let mut cos = pdfcraft_cos::Document::open(Arc::new(form())).unwrap();
    let first = ObjRef::new(5, 0);
    let action = |caption: &str| {
        let mut a = Dict::new();
        a.set(b"S".to_vec(), Object::name("JavaScript"));
        a.set(b"JS".to_vec(), PdfString::text(&format!("event.target.buttonSetCaption('{caption}');")));
        Object::Dict(a)
    };
    cos.update_dict(first, |d| {
        d.set(b"A".to_vec(), action("first"));
        let mut mk = Dict::new();
        mk.set(b"CA".to_vec(), PdfString::text("idle"));
        d.set(b"MK".to_vec(), Object::Dict(mk));
    })
    .unwrap();
    let mut second = cos.get(first).as_dict().unwrap().clone();
    second.set(b"Rect".to_vec(), Object::Array(vec![160.into(), 200.into(), 280.into(), 230.into()]));
    second.set(b"A".to_vec(), action("second"));
    let second = cos.add(Object::Dict(second));
    let root = cos.root().unwrap();
    cos.update_dict(root, |d| {
        let mut af = d.get(b"AcroForm").unwrap().as_dict().unwrap().clone();
        let mut refs = af.get(b"Fields").unwrap().as_array().unwrap().clone();
        refs.push(second.into());
        af.set(b"Fields".to_vec(), Object::Array(refs));
        d.set(b"AcroForm".to_vec(), Object::Dict(af));
    })
    .unwrap();
    cos.update_dict(ObjRef::new(3, 0), |d| {
        let mut a = d.get(b"Annots").unwrap().as_array().unwrap().clone();
        a.push(second.into());
        d.set(b"Annots".to_vec(), Object::Array(a));
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&cos, &Default::default()).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::new_app();
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "duplicate-buttons.pdf", None, bytes).unwrap();
            app
        }
    });
    h.run_steps(6);
    let captions = |h: &Harness<'_, PdfCraftApp>| {
        let id = h.state().active_ids().unwrap().1;
        h.state()
            .session
            .get(id)
            .unwrap()
            .form
            .iter()
            .filter(|f| f.name == "hello")
            .map(|f| f.widgets.first().and_then(|w| w.caption.clone()))
            .collect::<Vec<_>>()
    };
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let at = r.min + egui::vec2(r.width() * 220.0 / 300.0, r.height() * 85.0 / 300.0);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    while std::time::Instant::now() < deadline {
        h.run_steps(2);
        if captions(&h).iter().any(|s| s.as_deref() == Some("second")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(captions(&h), [Some("idle".into()), Some("second".into())]);
    assert!(h.execute("edit.undo"));
    h.run_steps(3);
    assert_eq!(captions(&h), [Some("idle".into()), Some("idle".into())]);
    assert!(h.execute("edit.redo"));
    h.run_steps(3);
    assert_eq!(captions(&h), [Some("idle".into()), Some("second".into())]);
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let at = r.min + egui::vec2(r.width() * 70.0 / 300.0, r.height() * 85.0 / 300.0);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    while std::time::Instant::now() < deadline {
        h.run_steps(2);
        if captions(&h).first().and_then(|s| s.as_deref()) == Some("first") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(captions(&h), [Some("first".into()), Some("second".into())]);
    let id = h.state().active_ids().unwrap().1;
    let bytes = h.state().session.save_bytes(id).unwrap();
    let mut reopened = pdfcraft_engine::Session::new();
    reopened.set_javascript(false);
    let reopened_id = reopened.open("saved-buttons.pdf", None, bytes, None).unwrap();
    assert_eq!(
        reopened
            .get(reopened_id)
            .unwrap()
            .form
            .iter()
            .filter(|f| f.name == "hello")
            .map(|f| f.widgets.first().and_then(|w| w.caption.clone()))
            .collect::<Vec<_>>(),
        [Some("first".into()), Some("second".into())]
    );
    if let Ok(path) = std::env::var("PDFCRAFT_REVIEW12_BUTTON_SCREENSHOT") {
        // Let the asynchronous page raster catch up with the final accepted script edit.
        for _ in 0..60 {
            h.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.render().unwrap().save(path).unwrap();
    }
}

/// The real image-button click and picker callback must preserve the second duplicate's identity.
#[test]
fn clicking_duplicate_image_buttons_imports_only_into_the_clicked_field() {
    let _gpu = crate::gpu();
    let mut session = pdfcraft_engine::Session::new();
    let bytes = session.create_blank(300.0, 300.0, 1).unwrap();
    let id = session.open("images.pdf", None, bytes, None).unwrap();
    for (name, x) in [("first", 20.0), ("second", 160.0)] {
        session
            .apply(
                id,
                pdfcraft_engine::Edit::AddField {
                    page: 0,
                    rect: [x, 200.0, x + 100.0, 230.0],
                    kind: pdfcraft_engine::NewField::Image,
                    name: Some(name.into()),
                },
            )
            .unwrap();
    }
    let mut cos = pdfcraft_cos::Document::open(session.save_bytes(id).unwrap()).unwrap();
    for field in session.get(id).unwrap().form.iter() {
        cos.update_dict(field.obj, |d| d.set(b"T".to_vec(), pdfcraft_cos::PdfString::text("same"))).unwrap();
    }
    let bytes = pdfcraft_cos::write_full(&cos, &Default::default()).unwrap();
    let dir = std::env::temp_dir().join(format!("pdfcraft-exact-image-click-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("synthetic.png");
    let img = craft_image_codecs::Image::from_u8(2, 2, craft_image_codecs::ChannelLayout::Rgb, vec![120u8; 12]).unwrap();
    let png = craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
    std::fs::write(&path, png).unwrap();
    // The image picker answers with the PNG.
    let files = std::sync::Arc::new(craft::host::ScriptedFiles::answering([craft::host::Picked::Paths(vec![path.clone()])]));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::control::app_with_files(files);
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "images.pdf", None, bytes).unwrap();
            app
        }
    });
    h.run_steps(6);
    let rect = h.state().views[0].page_screen_rect(0).unwrap();
    let at = rect.min + egui::vec2(rect.width() * 210.0 / 300.0, rect.height() * 85.0 / 300.0);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    let icons = |h: &mut Harness<'_, PdfCraftApp>| {
        let id = h.state().active_ids().unwrap().1;
        let cos = pdfcraft_cos::Document::open(h.state_mut().session.save_bytes(id).unwrap()).unwrap();
        let root = cos.root().unwrap();
        let form = cos.get(root).as_dict().unwrap().get(b"AcroForm").map(|v| cos.resolve(v)).unwrap();
        form.as_dict()
            .unwrap()
            .get(b"Fields")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                cos.resolve(r)
                    .as_dict()
                    .and_then(|d| d.get(b"MK").map(|v| cos.resolve(v)))
                    .is_some_and(|m| m.as_dict().is_some_and(|d| d.contains(b"I")))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(icons(&mut h), [false, true]);
    assert!(h.execute("edit.undo"));
    h.run_steps(2);
    assert_eq!(icons(&mut h), [false, false]);
    assert!(h.execute("edit.redo"));
    h.run_steps(2);
    assert_eq!(icons(&mut h), [false, true]);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
