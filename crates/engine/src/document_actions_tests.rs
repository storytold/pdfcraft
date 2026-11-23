//! Catalog JavaScript actions (#269, ported from #344): the `/OpenAction` runs once on opening
//! and again on Revert, `/AA /WS` before a save and `/AA /WP` before printing, all in the
//! document's retained realm.

use std::sync::Arc;

use crate::js::DocumentScriptEvent;
use crate::{DocId, Session};

/// A PDF of `objs`, numbered from 1, with a cross-reference table.
fn pdf_of(objs: &[String]) -> Vec<u8> {
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

/// A one-field form (`log`, a text field) whose catalog carries `catalog` (PDF syntax, e.g.
/// `/OpenAction 6 0 R`), with `extra` objects numbered from 6.
fn form(catalog: &str, extra: &[&str]) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    objs.extend(extra.iter().map(|s| s.to_string()));
    pdf_of(&objs)
}

fn log(s: &Session, id: DocId) -> String {
    s.get(id).unwrap().form.iter().find(|f| f.name == "log").unwrap().value.join(",")
}

/// The open action runs once on opening, after the document scripts and in their realm, and
/// sees `event.name` "Open"; its field change is applied.
#[test]
fn the_open_action_runs_once_on_opening_in_the_document_realm() {
    let bytes = form(
        "/OpenAction 6 0 R /Names << /JavaScript << /Names [(init) 7 0 R] >> >>",
        &[
            "<< /S /JavaScript /JS (opened++; getField\\('log'\\).value = event.type + '/' + event.name + '/' + opened;) >>",
            "<< /S /JavaScript /JS (var opened = 0;) >>",
        ],
    );
    let mut s = Session::new();
    let id = s.open("open.pdf", None, Arc::new(bytes), None).unwrap();
    assert_eq!(log(&s, id), "Doc/Open/1");
    assert!(s.take_js_output(id).errors.is_empty());
    // The realm is kept: a later script sees the count the open action left.
    assert_eq!(s.run_javascript(id, "opened;", None).unwrap().result.as_deref(), Some("1"));
}

/// An open action chain runs in order through `/Next` (a dictionary or an array), skipping
/// actions of other types, and a cycle in it runs each action once.
#[test]
fn an_open_action_chain_runs_in_order_and_a_cycle_runs_each_action_once() {
    let bytes = form(
        "/OpenAction 6 0 R",
        &[
            "<< /S /JavaScript /JS (var seen = ['a'];) /Next [7 0 R 8 0 R] >>",
            "<< /S /GoTo /D [3 0 R /Fit] /Next 9 0 R >>",
            "<< /S /JavaScript /JS (seen.push\\('c'\\); getField\\('log'\\).value = seen.join\\(''\\);) /Next 6 0 R >>",
            "<< /S /JavaScript /JS (seen.push\\('b'\\);) /Next 7 0 R >>",
        ],
    );
    let mut s = Session::new();
    let id = s.open("chain.pdf", None, Arc::new(bytes), None).unwrap();
    assert_eq!(log(&s, id), "abc");
}

/// What an open action asks for that needs a click (a URL, printing) is dropped; a page
/// change it asks for is kept.
#[test]
fn an_open_action_may_not_open_a_url_or_print() {
    let bytes = form("/OpenAction << /S /JavaScript /JS (app.launchURL\\('https://example.com'\\); this.print\\(\\); this.pageNum = 0;) >>", &[]);
    let mut s = Session::new();
    let id = s.open("asks.pdf", None, Arc::new(bytes), None).unwrap();
    let out = s.take_js_output(id);
    assert_eq!(out.requests, [crate::js::Request::GoToPage(0)], "{out:?}");
}

/// With JavaScript off the open action doesn't run.
#[test]
fn the_open_action_does_not_run_with_javascript_off() {
    let bytes = form("/OpenAction << /S /JavaScript /JS (getField\\('log'\\).value = 'ran';) >>", &[]);
    let mut s = Session::new();
    s.set_javascript(false);
    let id = s.open("off.pdf", None, Arc::new(bytes), None).unwrap();
    assert_eq!(log(&s, id), "");
    assert!(s.run_document_actions(id, DocumentScriptEvent::CatalogOpen).is_empty());
}

/// Revert reopens the saved file in a fresh realm, so the open action runs again from scratch.
#[test]
fn revert_runs_the_open_action_again_in_a_fresh_realm() {
    let bytes = form(
        "/OpenAction << /S /JavaScript /JS (if \\(typeof runs == 'undefined'\\) var runs = 0; runs++; getField\\('log'\\).value = 'run ' + runs;) >>",
        &[],
    );
    let mut s = Session::new();
    let id = s.open("revert.pdf", None, Arc::new(bytes), None).unwrap();
    assert_eq!(log(&s, id), "run 1");
    s.run_javascript(id, "getField('log').value = 'typed';", None).unwrap();
    s.revert(id).unwrap();
    assert_eq!(log(&s, id), "run 1", "a fresh realm counts from zero again");
}

/// Will Save runs in the document realm before the bytes are written, and what it changes is in
/// the saved file; a failing Will Save is reported and the save still has bytes to write.
#[test]
fn will_save_changes_what_is_saved_and_a_failing_one_does_not_stop_the_save() {
    let bytes = form(
        "/Names << /JavaScript << /Names [(init) 6 0 R] >> >> /AA << /WS << /S /JavaScript /JS (saves++; getField\\('log'\\).value = event.name + ' ' + saves;) >> >>",
        &["<< /S /JavaScript /JS (var saves = 0;) >>"],
    );
    let mut s = Session::new();
    let id = s.open("save.pdf", None, Arc::new(bytes), None).unwrap();
    let out = s.run_document_actions(id, DocumentScriptEvent::WillSave);
    assert!(out.errors.is_empty(), "{out:?}");
    assert_eq!(log(&s, id), "WillSave 1");
    let saved = s.save_bytes(id).unwrap();
    let mut reopened = Session::new();
    reopened.set_javascript(false);
    let again = reopened.open("saved.pdf", None, saved, None).unwrap();
    assert_eq!(log(&reopened, again), "WillSave 1");

    let failing = form("/AA << /WS << /S /JavaScript /JS (throw new Error\\('no'\\);) >> >>", &[]);
    let id = s.open("fail.pdf", None, Arc::new(failing), None).unwrap();
    let out = s.run_document_actions(id, DocumentScriptEvent::WillSave);
    assert!(out.errors.iter().any(|e| e.contains("no")), "{out:?}");
    assert!(s.save_bytes(id).is_ok());
}

/// Will Print runs before printing, in the same realm, and sees `event.name` "WillPrint".
#[test]
fn will_print_runs_in_the_document_realm() {
    let bytes = form("/AA << /WP << /S /JavaScript /JS (getField\\('log'\\).value = event.name;) >> >>", &[]);
    let mut s = Session::new();
    let id = s.open("print.pdf", None, Arc::new(bytes), None).unwrap();
    assert!(s.run_document_actions(id, DocumentScriptEvent::WillSave).is_empty(), "no Will Save action: nothing runs");
    assert_eq!(log(&s, id), "");
    s.run_document_actions(id, DocumentScriptEvent::WillPrint);
    assert_eq!(log(&s, id), "WillPrint");
}

/// A form with a text field `log` and a text field `hot` whose widget has JavaScript for every
/// pointer and focus trigger, each appending `event.name` to `log`.
fn triggers_form() -> Vec<u8> {
    let add =
        || "<< /S /JavaScript /JS (var l = getField\\('log'\\); l.value = l.value + event.name + ':' + event.target.name + ';';) >>".to_string();
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (hot) /V () /Rect [20 200 180 220] /P 3 0 R /F 4 /AA << /E {} /X {} /D {} /U {} /Fo {} /Bl {} >> >>",
            add(),
            add(),
            "<< /S /JavaScript /JS (getField\\('log'\\).value += 'D;'; this.print\\(\\);) >>",
            add(),
            add(),
            "<< /S /JavaScript /JS (getField\\('log'\\).value += 'Bl;'; this.print\\(\\);) >>",
        ),
    ];
    pdf_of(&objs)
}

/// Pointer and focus triggers run the widget's JavaScript with the event names Acrobat uses,
/// each as one undoable edit; a field without such a script runs nothing.
#[test]
fn pointer_and_focus_triggers_run_the_widget_scripts() {
    use crate::js::FieldFocus;
    use pdfcraft_forms::Trigger;
    let mut s = Session::new();
    let id = s.open("hot.pdf", None, Arc::new(triggers_form()), None).unwrap();
    assert!(s.field_trigger(id, "hot", 0, Trigger::MouseEnter).unwrap());
    assert!(s.field_focus(id, "hot", 0, FieldFocus::Enter).unwrap());
    assert!(s.field_trigger(id, "hot", 0, Trigger::MouseUp).unwrap());
    assert!(s.field_trigger(id, "hot", 0, Trigger::MouseExit).unwrap());
    assert_eq!(log(&s, id), "Mouse Enter:hot;Focus:hot;Mouse Up:hot;Mouse Exit:hot;");
    assert_eq!(s.get(id).unwrap().can_undo(), Some("Run JavaScript"));
    assert!(!s.field_trigger(id, "log", 0, Trigger::MouseEnter).unwrap(), "no script: nothing runs");
    assert!(!s.field_trigger(id, "hot", 5, Trigger::MouseEnter).unwrap(), "no such widget: nothing runs");
    assert!(s.field_trigger(id, "missing", 0, Trigger::MouseEnter).is_err());
    s.set_javascript(false);
    assert!(!s.field_trigger(id, "hot", 0, Trigger::MouseEnter).unwrap(), "JavaScript off");
}

/// A click's Mouse Down may ask to print; losing the focus, which happens unasked, may not.
#[test]
fn only_a_click_trigger_may_print() {
    use crate::js::{FieldFocus, Request};
    use pdfcraft_forms::Trigger;
    let mut s = Session::new();
    let id = s.open("hot.pdf", None, Arc::new(triggers_form()), None).unwrap();
    s.field_trigger(id, "hot", 0, Trigger::MouseDown).unwrap();
    assert_eq!(s.take_js_output(id).requests, [Request::Print]);
    s.field_focus(id, "hot", 0, FieldFocus::Exit).unwrap();
    let out = s.take_js_output(id);
    assert!(out.requests.is_empty(), "{out:?}");
    assert_eq!(log(&s, id), "D;Bl;");
}

/// `setItems` and `setAction` are written to the document: the combo box gets its new items
/// (pairs where text and export value differ), the installed scripts run on their triggers, and
/// both survive a save (#269, from #344).
#[test]
fn set_items_and_set_action_are_written_to_the_document() {
    use pdfcraft_forms::Trigger;
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (size) /Opt [(Small) (Large)] /V (Large) /Rect [20 200 180 220] /P 3 0 R /F 4 >>"
            .into(),
    ];
    let mut s = Session::new();
    let id = s.open("items.pdf", None, Arc::new(pdf_of(&objs)), None).unwrap();
    let out = s
        .run_javascript(
            id,
            "var f = getField('size'); f.setItems([['Medium', 'm'], 'Large']); \
             f.setAction('MouseEnter', \"getField('log').value = 'entered ' + event.target.value;\");",
            None,
        )
        .unwrap();
    assert_eq!(out.error, None, "{out:?}");
    let size = s.get(id).unwrap().form.iter().find(|f| f.name == "size").unwrap().clone();
    assert_eq!(size.options, [("m".to_string(), "Medium".to_string()), ("Large".into(), "Large".into())]);
    assert_eq!(size.value, ["Large"], "the value is still an item: it stays");
    assert!(s.field_trigger(id, "size", 0, Trigger::MouseEnter).unwrap());
    assert_eq!(log(&s, id), "entered Large");
    s.undo(id).unwrap();
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().form.iter().find(|f| f.name == "size").unwrap().options[0].1, "Small", "undone");
    s.redo(id).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let mut again = Session::new();
    let reopened = again.open("saved.pdf", None, saved, None).unwrap();
    let size = again.get(reopened).unwrap().form.iter().find(|f| f.name == "size").unwrap().clone();
    assert_eq!(size.options[0], ("m".to_string(), "Medium".to_string()));
    assert!(again.field_trigger(reopened, "size", 0, Trigger::MouseEnter).unwrap(), "the installed script is in the file");
}

/// A check box's Mouse Up, run after the click stored the new value, reads that value and can
/// show or hide another field by it (#247).
#[test]
fn a_check_box_mouse_up_reads_the_value_its_click_stored() {
    use crate::{Edit, FieldValue};
    use pdfcraft_forms::Trigger;
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 7 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (strike) /V /Off /AS /Off /Rect [20 150 50 180] /P 3 0 R /F 4 /MK << /CA (4) >> /AP << /N << /Yes 8 0 R /Off 8 0 R >> >> /A << /S /JavaScript /JS (getField\\('note'\\).display = event.target.value == 'Off' ? display.hidden : display.visible; getField\\('log'\\).value += 'up:' + event.target.value + ';';) >> >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (note) /V (struck) /Rect [60 150 180 180] /P 3 0 R /F 6 >>".into(),
        "<< /Type /XObject /Subtype /Form /BBox [0 0 30 30] /Length 0 >>\nstream\n\nendstream".into(),
    ];
    let mut s = Session::new();
    let id = s.open("strike.pdf", None, Arc::new(pdf_of(&objs)), None).unwrap();
    s.apply(id, Edit::SetFieldValue { name: "strike".into(), value: FieldValue::Text("Yes".into()) }).unwrap();
    let ran = s.field_trigger(id, "strike", 0, Trigger::MouseUp);
    let out = s.take_js_output(id);
    assert!(ran.unwrap(), "{out:?}");
    assert_eq!(log(&s, id), "up:Yes;");
    let note = |s: &Session| s.get(id).unwrap().form.iter().find(|f| f.name == "note").unwrap().widgets[0].hidden;
    assert!(!note(&s), "shown by the script");
    s.apply(id, Edit::SetFieldValue { name: "strike".into(), value: FieldValue::Check(false) }).unwrap();
    assert!(s.field_trigger(id, "strike", 0, Trigger::MouseUp).unwrap());
    assert_eq!(log(&s, id), "up:Yes;up:Off;");
    assert!(note(&s), "hidden again");
}

/// A widget action's `/Next` chain runs too, each JavaScript action in order (depth first), past
/// an action of another type at its head.
#[test]
fn a_widget_actions_next_chain_runs_each_script_in_order() {
    use pdfcraft_forms::Trigger;
    let js = |text: &str, next: &str| format!("<< /S /JavaScript /JS (getField\\('log'\\).value += '{text}';) {next} >>");
    let chain = format!("<< /S /URI /URI (https://example.com/) /Next [{} {}] >>", js("a;", &format!("/Next {}", js("b;", ""))), js("c;", ""));
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (hot) /V () /Rect [20 200 180 220] /P 3 0 R /F 4 /AA << /E {chain} >> >>"),
    ];
    let mut s = Session::new();
    let id = s.open("chain.pdf", None, Arc::new(pdf_of(&objs)), None).unwrap();
    assert!(s.field_trigger(id, "hot", 0, Trigger::MouseEnter).unwrap());
    assert_eq!(log(&s, id), "a;b;c;");
}

/// `setAction` on a field that shares its name with another installs the script on that field,
/// the one the script's object stands for, not the first of the name.
#[test]
fn set_action_installs_on_the_field_the_object_stands_for() {
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V () /Rect [20 200 180 220] /P 3 0 R /F 4 >>".into(),
    ];
    let mut s = Session::new();
    let id = s.open("dup.pdf", None, Arc::new(pdf_of(&objs)), None).unwrap();
    let second = crate::js::field_object_id(pdfcraft_cos::ObjRef::new(6, 0));
    let out =
        s.run_javascript_answers_target(id, "event.target.setAction('Format', \"event.value = 'x';\");", Some("dup"), Some(second), &[]).unwrap();
    assert_eq!(out.error, None, "{out:?}");
    let cos = s.get(id).unwrap().editor.as_ref().unwrap().cos.clone();
    let formats: Vec<(u32, bool)> = pdfcraft_forms::fields(&cos).iter().map(|f| (f.obj.num, f.actions.scripts.format.is_some())).collect();
    assert_eq!(formats, [(4, false), (6, true)]);
}

/// Of two fields sharing a name, the one the pointer is on runs its own widget script.
#[test]
fn a_trigger_on_a_field_sharing_a_name_runs_that_fields_script() {
    use pdfcraft_forms::Trigger;
    let enter = |text: &str| format!("/AA << /E << /S /JavaScript /JS (getField\\('log'\\).value += '{text}';) >> >>");
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>"
            .to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 7 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V () /Rect [20 200 180 220] /P 3 0 R /F 4 {} >>", enter("first;")),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V () /Rect [20 150 180 170] /P 3 0 R /F 4 {} >>", enter("second;")),
    ];
    let mut s = Session::new();
    let id = s.open("dup.pdf", None, Arc::new(pdf_of(&objs)), None).unwrap();
    let second = crate::js::field_object_id(pdfcraft_cos::ObjRef::new(7, 0));
    assert!(s.field_trigger_target(id, "dup", Some(second), 0, Trigger::MouseEnter).unwrap());
    assert_eq!(log(&s, id), "second;");
    assert!(s.field_trigger(id, "dup", 0, Trigger::MouseEnter).unwrap(), "by name alone, the first");
    assert_eq!(log(&s, id), "second;first;");
}

/// Focus too: of two fields sharing a name, the one that gained focus runs its On Focus script.
#[test]
fn focus_on_a_field_sharing_a_name_runs_that_fields_script() {
    use crate::js::FieldFocus;
    let focus = |text: &str| format!("/AA << /Fo << /S /JavaScript /JS (getField\\('log'\\).value += '{text}';) >> >>");
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>"
            .to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 7 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V () /Rect [20 200 180 220] /P 3 0 R /F 4 {} >>", focus("first;")),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V () /Rect [20 150 180 170] /P 3 0 R /F 4 {} >>", focus("second;")),
    ];
    let mut s = Session::new();
    let id = s.open("dup.pdf", None, Arc::new(pdf_of(&objs)), None).unwrap();
    let second = crate::js::field_object_id(pdfcraft_cos::ObjRef::new(7, 0));
    assert!(s.field_focus_target(id, "dup", Some(second), 0, FieldFocus::Enter).unwrap());
    assert_eq!(log(&s, id), "second;");
}
