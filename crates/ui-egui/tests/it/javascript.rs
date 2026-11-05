//! Acrobat JavaScript in the shell (egui_kittest): the console, Document JavaScripts, a button
//! script and Preferences ▸ JavaScript.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// A form with a text field and a push button whose Mouse Up script fills it in.
fn form() -> Vec<u8> {
    form_with("getField\\('greeting'\\).value = 'Hello from JavaScript'; app.alert\\('Filled in'\\);")
}

/// [`form`], the button's Mouse Up script being `script`, as written in a PDF literal string.
fn form_with(script: &str) -> Vec<u8> {
    let button = format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (hello) /Rect [20 200 120 230] /P 3 0 R /F 4 /A << /S /JavaScript /JS ({script}) >> >>"
    );
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 300] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (greeting) /Rect [20 250 280 270] /P 3 0 R /F 4 >>",
        button.as_str(),
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
    harness_of(form())
}

fn harness_of(pdf: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, pdf).unwrap();
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

/// A script past the engine's limit, which only a document can hold, has a note where its box would
/// be. Nothing the dialog does with it writes it back, and Clear script is the way to replace it.
#[test]
fn a_document_script_too_long_to_edit_is_left_as_it_is() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    let big = "x".repeat(pdfcraft_engine::js::MAX_SCRIPT_BYTES + 1);
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::SetDocumentScript { name: "big".into(), script: Some(big.clone()) }).unwrap();
    let bytes = |h: &Harness<'static, PdfCraftApp>| h.state().session.get(id).unwrap().bytes.clone();
    let before = bytes(&h);
    assert!(h.state_mut().execute("tools.document_js"));
    h.run_steps(2);
    h.get_by_label("big").click();
    h.run_steps(2);
    assert_eq!(h.state().doc_js.script, big);
    assert!(h.query_by_label_contains("too long").is_some(), "the note is shown");
    assert!(h.query_by_label("JavaScript").is_none(), "in place of the box");
    h.get_by_label("Save").click();
    h.run_steps(2);
    assert_eq!(bytes(&h), before, "saving a script it can't show writes nothing");
    assert_eq!(h.state().doc_js.script, big);
    h.get_by_label("Clear script").click();
    h.run_steps(2);
    assert_eq!(h.state().doc_js.script, "");
    h.get_by_label("JavaScript");
    assert_eq!(bytes(&h), before, "clearing the box is not saving it");
}

/// A field's script past the engine's limit: Edit, Update and OK change nothing.
#[test]
fn a_field_script_too_long_to_edit_is_left_as_it_is() {
    let big = format!("var a = '{}';", "x".repeat(pdfcraft_engine::js::MAX_SCRIPT_BYTES));
    let mut h = harness_of(form_with(&big));
    let id = h.state().active_ids().unwrap().1;
    let before = h.state().session.get(id).unwrap().bytes.clone();
    h.state_mut().open_field_props("hello", 0);
    h.state_mut().field_props.as_mut().unwrap().tab = pdfcraft_ui_egui::prepare::FieldTab::Actions;
    h.run_steps(2);
    // The last "Edit": the menu bar has one too.
    h.get_all_by_label("Edit").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(h.state().field_props.as_ref().unwrap().new_action.text, big);
    assert!(h.query_by_label_contains("too long").is_some(), "the note is shown");
    assert!(h.query_by_label("JavaScript").is_none(), "in place of the box");
    h.get_by_label("Update").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert!(h.state().field_props.is_none(), "the dialog closed");
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.bytes, before, "OK writes nothing");
    assert_eq!(doc.undo_label(), None, "and adds no undo step");
    assert!(!doc.dirty);
}

/// A paste past the limit never gets as far as the field: the box refuses it and says why, and Update
/// and OK write what the box still holds.
#[test]
fn a_paste_past_the_limit_never_reaches_the_field() {
    let mut h = harness_of(form_with("go\\(\\);"));
    let id = h.state().active_ids().unwrap().1;
    let before = h.state().session.get(id).unwrap().bytes.clone();
    h.state_mut().open_field_props("hello", 0);
    h.state_mut().field_props.as_mut().unwrap().tab = pdfcraft_ui_egui::prepare::FieldTab::Actions;
    h.run_steps(2);
    h.get_all_by_label("Edit").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(h.state().field_props.as_ref().unwrap().new_action.text, "go();");
    h.get_by_role_and_label(egui::accesskit::Role::MultilineTextInput, "Script:").focus();
    h.run_steps(2);
    h.event(egui::Event::Paste("y".repeat(pdfcraft_engine::js::MAX_SCRIPT_BYTES)));
    h.run_steps(2);
    assert!(h.state().field_props.as_ref().unwrap().new_action.text == "go();", "the box still holds the script");
    assert!(h.query_by_label_contains("Not added").is_some(), "the line under the box says why");
    assert!(h.query_by_label_contains("too long").is_none(), "no note");
    h.get_by_label("Update").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert!(h.state().field_props.is_none(), "the dialog closed");
    let doc = h.state().session.get(id).unwrap();
    assert!(doc.bytes == before, "OK writes nothing");
    assert_eq!(doc.undo_label(), None, "and adds no undo step");
}

/// The console has no Cancel: a paste past the limit must leave what was typed, which can still be run.
#[test]
fn a_paste_past_the_limit_leaves_the_console_s_input() {
    let mut h = harness();
    assert!(h.state_mut().execute("tools.js_console"));
    h.run_steps(2);
    h.get_by_label("JavaScript").focus();
    h.run_steps(2);
    h.event(egui::Event::Text("typed".into()));
    h.run_steps(2);
    assert_eq!(h.state().js_console.input, "typed");
    h.event(egui::Event::Paste("y".repeat(pdfcraft_engine::js::MAX_SCRIPT_BYTES)));
    h.run_steps(2);
    assert!(h.state().js_console.input == "typed", "still what was typed");
    assert!(h.query_by_label_contains("Not added").is_some(), "the line under the box says why");
    h.get_by_label("JavaScript");
}

/// The Actions tab says what PdfCraft does with an action. Only the Mouse Up of a push button runs;
/// the rest is saved for other viewers, which the user could not otherwise tell.
#[test]
fn the_actions_tab_says_which_actions_pdfcraft_runs() {
    use pdfcraft_engine::FieldTrigger;
    let hint = "PdfCraft won't run this action, because it only runs Mouse Up actions on buttons. It's still saved in the PDF, so other PDF apps can run it.";
    let mut h = harness_of(form_with("event.value = 1;"));
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("hello", 0);
    h.run_steps(2);
    h.get_by_label("Actions").click();
    h.run_steps(2);
    assert_eq!(h.state().field_props.as_ref().unwrap().new_action.trigger, FieldTrigger::MouseUp);
    assert!(h.query_by_label(hint).is_none(), "the Mouse Up of a push button runs");
    h.state_mut().field_props.as_mut().unwrap().new_action.trigger = FieldTrigger::OnBlur;
    h.run_steps(2);
    h.get_by_label(hint);
}

/// On any other kind of field, Mouse Up is saved and doesn't run either.
#[test]
fn the_actions_tab_of_a_text_field_says_its_actions_do_not_run() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app
    });
    crate::support::settle(&mut h);
    h.state_mut().execute("form.prepare");
    h.run_steps(2);
    h.state_mut().open_field_props("city", 0);
    h.run_steps(2);
    h.get_by_label("Actions").click();
    h.run_steps(2);
    h.get_by_label("PdfCraft won't run this action, because it only runs Mouse Up actions on buttons. It's still saved in the PDF, so other PDF apps can run it.");
    // The script box takes the dialog's whole width, under the pickers, and its label names it.
    let script = h.get_by_role_and_label(egui::accesskit::Role::MultilineTextInput, "Script:");
    let width = script.rect().width();
    assert!(width > 400.0, "wider than the 340-point column it used to sit in: {width}");
    let height = script.rect().height();
    assert!(height > 100.0, "eight lines, not three: {height}");
}

fn sized(size: egui::Vec2) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(size).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, form_with("go\\(\\);")).unwrap();
        app
    });
    h.run_steps(8);
    h
}

/// Drag the corner of the only script box to `to`.
fn drag_the_corner(h: &mut Harness<'static, PdfCraftApp>, to: egui::Pos2) {
    let corner = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().right_bottom() - egui::vec2(3.0, 3.0);
    h.hover_at(corner);
    h.run_steps(2);
    h.drag_at(corner);
    h.run_steps(1);
    for i in 1..=6 {
        h.hover_at(corner + (to - corner) * (i as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
}

/// Field Properties' card stays as tall as it has been, so the tabs don't jump (see below). Going by
/// the card, the box shrank for good: to three lines in a window with room for more, and it could not
/// be dragged back. And a card too tall for the window once stayed so, its title off the screen.
#[test]
fn the_action_box_is_sized_by_the_room_the_dialog_really_has() {
    let height = |h: &Harness<'static, PdfCraftApp>| h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().height();
    let open = |h: &mut Harness<'static, PdfCraftApp>| {
        h.state_mut().open_field_props("greeting", 0);
        h.state_mut().field_props.as_mut().unwrap().tab = pdfcraft_ui_egui::prepare::FieldTab::Actions;
        h.run_steps(6);
    };
    // A window a little too short for the eight lines.
    let mut h = sized(egui::vec2(1000.0, 700.0));
    open(&mut h);
    assert!(height(&h) > 110.0, "what fits, not three lines: {}", height(&h));
    let mut h = sized(egui::vec2(1000.0, 640.0));
    open(&mut h);
    let title = h.get_by_label("Text Field Properties").rect();
    assert!(title.top() >= 0.0, "the title stays on the screen: {title:?}");
    assert!(h.get_by_label("OK").rect().bottom() <= 640.0, "and so do the buttons");
    // Dragged as tall as it goes, shorter and taller again, it follows the corner: the card, still as
    // tall as it was, has no room to give, but the dialog does.
    let mut h = sized(egui::vec2(1000.0, 900.0));
    open(&mut h);
    let corner = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().right_bottom();
    drag_the_corner(&mut h, egui::pos2(corner.x - 3.0, 895.0));
    let tallest = height(&h);
    let corner = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().right_bottom();
    drag_the_corner(&mut h, corner - egui::vec2(3.0, 80.0));
    let shorter = height(&h);
    assert!(shorter < tallest - 40.0, "{tallest} then {shorter}");
    let corner = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().right_bottom();
    drag_the_corner(&mut h, corner + egui::vec2(-3.0, 80.0));
    assert!(height(&h) > shorter + 40.0, "{shorter} then {}", height(&h));
    // Dragged as short as it goes, the text area is no taller than the box: its own four rows would
    // scroll a box this short for nothing.
    let corner = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().right_bottom();
    drag_the_corner(&mut h, egui::pos2(corner.x - 3.0, 100.0));
    assert!(height(&h) < 70.0, "as short as the box: {}", height(&h));
}

/// Field Properties keeps its height as tabs are picked, so a shorter one doesn't move the tabs out
/// from under the pointer. Sizing the card by what each tab holds moved them by 100 points a click.
#[test]
fn the_tabs_of_field_properties_stay_put() {
    use pdfcraft_ui_egui::prepare::FieldTab as T;
    let mut h = sized(egui::vec2(1200.0, 1000.0));
    h.state_mut().open_field_props("greeting", 0);
    h.run_steps(6);
    let tabs_at = |h: &mut Harness<'static, PdfCraftApp>, tab: T| {
        h.state_mut().field_props.as_mut().unwrap().tab = tab;
        h.run_steps(6);
        h.get_by_label("General").rect().top()
    };
    let tallest = tabs_at(&mut h, T::Actions);
    let ok = h.get_by_label("OK").rect().top();
    for tab in [T::General, T::Position, T::Format, T::Validate, T::Calculate] {
        let y = tabs_at(&mut h, tab);
        assert!((y - tallest).abs() < 1.0, "{tab:?}: the tabs moved from {tallest} to {y}");
        let y = h.get_by_label("OK").rect().top();
        assert!((y - ok).abs() < 1.0, "{tab:?}: OK moved from {ok} to {y}");
    }
    // In a window too short for the tallest tab they move no more than its room for the box does.
    let mut h = sized(egui::vec2(1200.0, 600.0));
    h.state_mut().open_field_props("greeting", 0);
    h.run_steps(6);
    let tallest = tabs_at(&mut h, T::Actions);
    for tab in [T::General, T::Position, T::Format] {
        let y = tabs_at(&mut h, tab);
        assert!((y - tallest).abs() < 25.0, "{tab:?}: in a short window the tabs moved from {tallest} to {y}");
    }
}

/// A box dragged tall and reopened in a short window measures what it is, not the part still on the
/// screen: that made it collapse, and stay small in a tall window too.
#[test]
fn a_tall_box_reopened_in_a_short_window_fits_it() {
    let mut h = sized(egui::vec2(1400.0, 1000.0));
    assert!(h.state_mut().execute("tools.document_js"));
    h.run_steps(3);
    h.state_mut().doc_js.script = "a\nb\nc\n".into();
    h.run_steps(3);
    let corner = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().right_bottom();
    drag_the_corner(&mut h, egui::pos2(corner.x - 3.0, 990.0));
    h.get_by_label("Close").click();
    h.run_steps(3);
    h.set_size(egui::vec2(1400.0, 500.0));
    h.run_steps(3);
    assert!(h.state_mut().execute("tools.document_js"));
    h.run_steps(8);
    let reopened = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().height();
    assert!(reopened > 150.0, "the room there is, not three lines: {reopened}");
}

/// ⌘Enter goes by the key's own modifiers. Behind typing in a slow frame the box holds the key for a
/// frame, by when ⌘ may be up: the script then didn't run.
#[test]
fn cmd_enter_behind_typing_runs_the_script_even_with_cmd_up_by_then() {
    let cmd = egui::Modifiers { alt: false, ctrl: false, shift: false, mac_cmd: true, command: true };
    let enter = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: cmd };
    let mut h = harness();
    assert!(h.state_mut().execute("tools.js_console"));
    h.run_steps(2);
    h.get_by_label("JavaScript").focus();
    h.run_steps(2);
    h.state_mut().js_console.input = "6 * 7".into();
    h.run_steps(2);
    h.input_mut().events.extend([egui::Event::ModifiersChanged(cmd), egui::Event::Text("0".into()), enter(true)]);
    h.step();
    h.input_mut().events.extend([enter(false), egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
    h.step();
    h.run_steps(3);
    let console = &h.state().js_console;
    assert_eq!(console.input, "06 * 7");
    assert!(console.log.iter().any(|l| l.contains("42")), "it ran: {:?}", console.log);
}
