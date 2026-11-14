//! Acrobat JavaScript in the shell (egui_kittest): the console, Document JavaScripts, a button
//! script and Preferences ▸ JavaScript.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, form()).unwrap();
        app
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
    assert!(h.state_mut().execute("tools.js_console"));
    h.run_steps(2);
    h.get_by_label("JavaScript Console");
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
    assert!(h.state().js_console.log.is_empty(), "{:?} {:?}", h.state().js_console.log, h.state().dialog);
}

/// A button's script fills a field and its message takes the toast.
#[test]
fn button_scripts_run_and_alert() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    h.state_mut().run_button_script(id, "hello", "getField('greeting').value = 'Hello from JavaScript'; app.alert('Filled in');");
    wait_for(&mut h, "Filled in");
    h.get_by_label("OK").click();
    wait_value(&mut h, "greeting", "Hello from JavaScript");
}

#[test]
fn text_widget_mouse_up_runs_its_menu_and_keeps_the_result() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("left", "closed").unwrap();
        let bytes = form_with_text_action(
            r"/AA << /U << /S /JavaScript /JS (var choice = app.popUpMenuEx\({cName:'Choose test value',cReturn:'selected'}\); if \(choice\) getField\('greeting'\).value = choice;) >> >>",
        );
        app.open_bytes("text-action.pdf", None, bytes).unwrap();
        app
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

/// One run's messages are all shown, in order (#4294): the first takes the toast with its title
/// and icon (#4293), the next takes it when the first is gone, and a notice shown meanwhile
/// doesn't lose the ones still waiting. Messages past the queue's room are counted and reported
/// once the rest have been shown.
#[test]
fn every_message_a_script_shows_takes_the_toast_in_turn() {
    use pdfcraft_engine::js::{MessageButtons, MessageIcon};
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    let out = h
        .state_mut()
        .session
        .run_javascript(id, "app.alert('First', 1, 0, 'Order form'); app.alert('Second', 3); app.alert('Third');", None)
        .unwrap();
    h.state_mut().handle_js(id, pdfcraft_engine::js::JsOutput { alerts: out.alerts, ..Default::default() });
    h.run_steps(2);
    let shown = |h: &Harness<'static, PdfCraftApp>| h.state().toast.clone().map(|t| t.0);
    assert_eq!(shown(&h).as_deref(), Some("First"));
    let m = h.state().toast_script.clone().expect("the toast shows the script's message");
    assert_eq!((m.title.as_deref(), m.icon, m.buttons), (Some("Order form"), MessageIcon::Warning, MessageButtons::Ok));
    assert!(h.query_by_label("Order form").is_some(), "the title is drawn");
    assert_eq!(h.state().script_messages.len(), 2);
    // The first is gone: the second takes its place, with its own icon and no title.
    h.state_mut().toast = None;
    h.run_steps(2);
    assert_eq!(shown(&h).as_deref(), Some("Second"));
    assert_eq!(h.state().toast_script.as_ref().map(|m| (m.title.clone(), m.icon)), Some((None, MessageIcon::Status)));
    // A notice of the app's own replaces it; the third still follows.
    h.state_mut().notify("Saved");
    assert_eq!(h.state().toast_script, None);
    h.state_mut().toast = None;
    h.run_steps(2);
    assert_eq!(shown(&h).as_deref(), Some("Third"));
    assert!(h.state().script_messages.is_empty());
    // A notice the same run raises first (its submit request) isn't replaced: the message
    // follows it.
    h.state_mut().toast = None;
    h.run_steps(2);
    let out = h.state_mut().session.run_javascript(id, "this.submitForm('https://example.com/form'); app.alert('Fourth');", None).unwrap();
    h.state_mut().handle_js(id, pdfcraft_engine::js::JsOutput { alerts: out.alerts, requests: out.requests, ..Default::default() });
    h.run_steps(2);
    let notice = shown(&h).unwrap_or_default();
    assert!(notice.contains("submitted"), "the submit notice stays: {notice}");
    assert_eq!(h.state().script_messages.len(), 1);
    h.state_mut().toast = None;
    h.run_steps(2);
    assert_eq!(shown(&h).as_deref(), Some("Fourth"));
    // Past the 100 the queue holds, messages are counted, and the toast says how many once the
    // rest have been shown.
    h.state_mut().toast = None;
    h.run_steps(2);
    let many: Vec<_> = (0..105).map(|i| pdfcraft_engine::js::Message::plain(format!("Message {i}"))).collect();
    h.state_mut().show_script_messages(many);
    assert_eq!(shown(&h).as_deref(), Some("Message 0"));
    for i in 1..100 {
        assert!(h.state_mut().next_script_message());
        assert_eq!(shown(&h), Some(format!("Message {i}")));
    }
    assert!(h.state_mut().next_script_message(), "the notice comes after the last message");
    assert_eq!(shown(&h).as_deref(), Some("5 more messages from the form's scripts were left out."));
    assert_eq!(h.state().toast_script, None, "an app notice, not a script message");
    assert!(!h.state_mut().next_script_message(), "said once");
    // Another notice replacing it before its time doesn't lose the count: once that notice has
    // been shown its time, the count is said again, and then, its own time over, not again.
    h.state_mut().notify("Saved");
    assert_eq!(shown(&h).as_deref(), Some("Saved"));
    let left_out = "5 more messages from the form's scripts were left out.";
    let mut said_again = false;
    for _ in 0..600 {
        h.run_steps(1);
        said_again |= shown(&h).as_deref() == Some(left_out);
        if said_again && shown(&h).is_none() {
            break;
        }
    }
    assert!(said_again, "the count is said again after the notice that replaced it");
    assert_eq!(shown(&h), None, "and once only");
    assert!(!h.state_mut().next_script_message());
}

#[test]
fn document_scripts_and_preferences() {
    let mut h = harness();
    assert!(h.state_mut().execute("tools.document_js"));
    h.run_steps(2);
    h.get_by_label("Document JavaScripts");
    h.state_mut().doc_js.name = "init".into();
    h.state_mut().doc_js.script = "function hi() { return 'hi'; }".into();
    h.get_by_label("Save").click();
    h.run_steps(2);
    let id = h.state().active_ids().unwrap().1;
    assert_eq!(h.state().session.get(id).unwrap().document_scripts()[0].0, "init");
    h.state_mut().dialog = Some(Dialog::Preferences);
    h.run_steps(2);
    // Below the fold in a 900-point window: the preferences scroll to it.
    h.get_by_label("Enable Acrobat JavaScript").scroll_to_me();
    h.run_steps(4);
    h.get_by_label("Enable Acrobat JavaScript").click();
    h.run_steps(2);
    assert!(!h.state().session.javascript());
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert!(!fresh.session.javascript(), "the preference is remembered");
}

#[test]
fn merge_data_files_into_a_spreadsheet() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-merge-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("report.csv");
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.save_override = Some(out.to_string_lossy().into_owned());
    app.merge_data_files(vec![("form.pdf".into(), form())]);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "greeting\n\n");
    assert!(app.toast.clone().unwrap().0.starts_with("Merged 1 file"));
    app.merge_data_files(vec![("x.fdf".into(), b"junk".to_vec())]);
    assert!(app.toast.clone().unwrap().0.starts_with("x.fdf:"));
}

#[test]
fn prepare_a_form_detects_fields_on_a_paper_form() {
    let paper = pdfcraft_engine::Session::new().create_from_text("t", "Name: ____________________\n\nPhone: ____________________").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("paper.pdf", None, paper.clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("form.prepare"));
    h.run_steps(3);
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    let names: Vec<String> = app.session.get(id).unwrap().form.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["Name", "Phone"]);
    assert_eq!(app.toast.clone().unwrap().0, "Detected 2 form fields");
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
        h.state_mut().run_button_script(id, "hello", script);
        wait_for(&mut h, body);
        assert!(h.query_by_label("This document's script is asking").is_some(), "{script}");
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
    h.state_mut().set_option("language", "de").unwrap();
    let id = h.state().active_ids().unwrap().1;
    h.state_mut().run_button_script(id, "hello", "app.response({cQuestion:'Test',bPassword:true});");
    wait_for(&mut h, heading);
    assert!(h.query_by_label("Dokument: form.pdf").is_some());
    assert!(h.query_by_label("Diese Aufforderung stammt aus dem Dokument und wird von dessen Skript angezeigt. Gib ein Passwort nur ein, wenn du diesem Dokument vertraust.").is_some());
}

#[test]
/// A real UI menu followed by input resumes once and does not leak the menu click into the next modal.
fn native_button_popup_and_response_resume_without_replaying_the_script() {
    for typed in [false, true] {
        let mut h = harness();
        let id = h.state().active_ids().unwrap().1;
        h.state_mut().run_button_script(id,"hello","var calls=typeof calls==='number'?calls:0;calls++;var choice=app.popUpMenuEx({cName:'Morning',cReturn:'08:00'});var extra=app.response({cTitle:'Manual time',cQuestion:'Suffix',cDefault:'x'});getField('greeting').value=choice+'/'+extra+'/'+calls;");
        wait_for(&mut h, "Morning");
        h.get_by_label("Morning").click();
        wait_for(&mut h, "Suffix");
        if typed {
            h.get_by(|n| n.role() == egui::accesskit::Role::TextInput && n.value().as_deref() == Some("x")).focus();
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
    h.state_mut().run_button_script(id, "hello", script);
    wait_for(&mut h, "Choose");
    h.get_by_label("Cancel").click();
    wait_value(&mut h, "greeting", "null/1");
    h.state_mut().run_button_script(id, "hello", script);
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
    h.state_mut().run_button_script(id, "hello", script);
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
    h.state_mut().run_button_script(id, "hello", "app.popUpMenuEx({cName:'Wait',cReturn:'yes'});getField('greeting').value='stale';");
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
    h.state_mut().run_button_script(id, "hello", "getField('greeting').value=String(event.willCommit);");
    wait_value(&mut h, "greeting", "false");
}

/// A popup owns keyboard shortcuts; Undo must not edit the document below it.
#[test]
fn native_popup_keeps_undo_shortcuts_out_of_the_document() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    h.state_mut()
        .session
        .apply(id, pdfcraft_engine::Edit::SetFieldValue { name: "greeting".into(), value: pdfcraft_engine::FieldValue::Text("current".into()) })
        .unwrap();
    h.state_mut().run_button_script(id, "hello", "app.popUpMenuEx({cName:'Wait',cReturn:'yes'});");
    wait_for(&mut h, "Wait");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(3);
    assert_eq!(value(&h, "greeting"), ["current"]);
    assert!(h.query_by_label("Wait").is_some());
    h.get_by_label("Cancel").click();
    h.run_steps(5);
}

/// A script response stays clickable while another modal remains open underneath it.
#[test]
fn native_script_prompt_is_above_an_existing_dialog() {
    let _gpu = crate::gpu();
    for open_first in [true, false] {
        let mut h = harness();
        let id = h.state().active_ids().unwrap().1;
        if open_first {
            h.state_mut().dialog = Some(Dialog::JsConsole);
            h.run_steps(3);
        }
        h.state_mut().run_button_script(
            id,
            "hello",
            "getField('greeting').value=app.response({cTitle:'Top prompt',cQuestion:'Answer here',cDefault:'visible'});",
        );
        wait_for(&mut h, "Answer here");
        if !open_first {
            h.state_mut().dialog = Some(Dialog::JsConsole);
        }
        h.run_steps(4);
        let expected = egui::Id::unique(("document-script-prompt", id.0, 1u32));
        assert_eq!(h.ctx.memory(|m| m.top_modal_layer()).map(|l| l.id), Some(expected), "the prompt owns modal input, open_first={open_first}");
        if let Ok(path) = std::env::var("PDFCRAFT_REVIEW4_MODAL_SCREENSHOT") {
            h.render().unwrap().save(path).unwrap();
        }
        h.get_by_label("OK").click();
        wait_value(&mut h, "greeting", "visible");
        assert_eq!(h.state().dialog, Some(Dialog::JsConsole));
        h.state_mut().run_button_script(id, "hello", "getField('greeting').value=String(app.response({cQuestion:'Cancel here'}));");
        wait_for(&mut h, "Cancel here");
        h.key_press(egui::Key::Escape);
        wait_value(&mut h, "greeting", "null");
        assert_eq!(h.state().dialog, Some(Dialog::JsConsole));
    }
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("duplicate-buttons.pdf", None, bytes).unwrap();
        app
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
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    assert_eq!(captions(&h), [Some("idle".into()), Some("idle".into())]);
    assert!(h.state_mut().execute("edit.redo"));
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
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("images.pdf", None, bytes).unwrap();
        app.save_override = Some(path.to_string_lossy().into_owned());
        app
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
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(2);
    assert_eq!(icons(&mut h), [false, false]);
    assert!(h.state_mut().execute("edit.redo"));
    h.run_steps(2);
    assert_eq!(icons(&mut h), [false, true]);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
