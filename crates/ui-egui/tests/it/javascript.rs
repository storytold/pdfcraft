//! Acrobat JavaScript in the shell (egui_kittest): the console, Document JavaScripts, a button
//! script and Preferences ▸ JavaScript.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// A form with a text field and a push button whose Mouse Up script fills it in.
fn form() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 300] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (greeting) /Rect [20 250 280 270] /P 3 0 R /F 4 >>",
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
    h.run_steps(2);
    assert_eq!(value(&h, "greeting"), ["Hello from JavaScript"]);
    assert_eq!(h.state().toast.clone().unwrap().0, "Filled in");
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
    h.state_mut().run_button_script(id, "hello", "app.alert('First', 1, 0, 'Order form'); app.alert('Second', 3); app.alert('Third');");
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
    h.state_mut().run_button_script(id, "hello", "this.submitForm('https://example.com/form'); app.alert('Fourth');");
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
