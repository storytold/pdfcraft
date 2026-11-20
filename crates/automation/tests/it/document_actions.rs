//! A form's document actions and pointer/focus scripts through the tool table (#269, ported
//! from #344): the open action runs on doc_open, Will Save and Did Save on doc_save, Will
//! Print and Did Print on doc_print, Will Close on doc_close, and js_event runs a widget's
//! pointer and focus scripts, all in one realm.

use pdfcraft_automation::{Automation, Content};
use serde_json::{Value, json};

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(value) => value,
        other => panic!("expected JSON: {other:?}"),
    }
}

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

/// `log` records what ran; `hot` has Mouse Enter and On Blur scripts. The open action counts
/// in a variable the Will Save and Will Print actions and `hot`'s scripts read.
fn form() -> Vec<u8> {
    pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /OpenAction 6 0 R /AA << /WS 7 0 R /WP 8 0 R >> /AcroForm << /Fields [4 0 R 9 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 9 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        "<< /S /JavaScript /JS (var n = 1; getField\\('log'\\).value = 'open' + n;) >>".into(),
        "<< /S /JavaScript /JS (n++; getField\\('log'\\).value += ' save' + n; console.println\\('saving'\\);) >>".into(),
        "<< /S /JavaScript /JS (n++; getField\\('log'\\).value += ' print' + n;) >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (hot) /V () /Rect [20 200 180 220] /P 3 0 R /F 4 /AA << /E << /S /JavaScript /JS (n++; getField\\('log'\\).value += ' ' + event.name + n;) >> /Bl << /S /JavaScript /JS (this.print\\(\\); getField\\('log'\\).value += ' ' + event.name;) >> >> >>".into(),
    ])
}

fn log(a: &mut Automation, doc: u64) -> Value {
    let f = tool(a, "form_fields", json!({ "doc": doc }));
    f["fields"].as_array().unwrap().iter().find(|f| f["name"] == "log").map(|f| f["value"].clone()).unwrap()
}

#[test]
fn open_will_save_will_print_and_field_events_share_the_document_realm() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-document-actions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("form.pdf"), form()).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap().with_clock(|| 1_700_000_000);
    let doc = tool(&mut a, "doc_open", json!({ "path": "form.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(log(&mut a, doc), "open1");

    let entered = tool(&mut a, "js_event", json!({ "doc": doc, "field": "hot", "event": "mouse_enter" }));
    assert_eq!(entered["changed"], true, "{entered}");
    assert_eq!(log(&mut a, doc), "open1 Mouse Enter2");
    // Losing the focus happens unasked: its print request is dropped and noted.
    let blurred = tool(&mut a, "js_event", json!({ "doc": doc, "field": "hot", "event": "on_blur" }));
    assert_eq!(blurred["requests"], json!([]), "{blurred}");
    assert_eq!(log(&mut a, doc), "open1 Mouse Enter2 Blur");
    let none = tool(&mut a, "js_event", json!({ "doc": doc, "field": "log", "event": "mouse_down" }));
    assert_eq!(none["changed"], false, "no script: {none}");
    assert!(a.call("js_event", &json!({ "doc": doc, "field": "hot", "event": "keystroke" })).is_err());
    assert!(a.call("js_event", &json!({ "doc": doc, "field": "hot", "event": "mouse_up", "widget": 2 })).is_err());

    // Saved in place (incrementally): object numbers stay, so the realm is kept. A full
    // rewrite starts a fresh realm for the next event.
    let saved = tool(&mut a, "doc_save", json!({ "doc": doc }));
    assert_eq!(saved["incremental"], true, "{saved}");
    std::fs::copy(dir.join("form.pdf"), dir.join("saved.pdf")).unwrap();
    assert_eq!(saved["will_save"]["console"], json!(["saving"]), "{saved}");
    assert_eq!(log(&mut a, doc), "open1 Mouse Enter2 Blur save3");

    let printed = tool(&mut a, "doc_print", json!({ "doc": doc, "path": "printed.pdf" }));
    assert!(printed.get("will_print").is_none(), "nothing to report: {printed}");
    assert_eq!(log(&mut a, doc), "open1 Mouse Enter2 Blur save3 print4");
    // A print with nowhere to go fails before Will Print runs.
    assert!(a.call("doc_print", &json!({ "doc": doc })).is_err());
    assert_eq!(log(&mut a, doc), "open1 Mouse Enter2 Blur save3 print4");

    // What Will Save changed is in the saved file.
    tool(&mut a, "js_enabled", json!({ "enabled": false }));
    let reopened = tool(&mut a, "doc_open", json!({ "path": "saved.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(log(&mut a, reopened), "open1 Mouse Enter2 Blur save3");
}

/// A form whose open action sets `n`, and whose Did Save, Did Print and Will Close actions
/// count on from it into `log` and the console (`aa` is the catalog's `/AA`, `extra` objects
/// are numbered from 6).
fn after_form(aa: &str, extra: &[&str]) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        format!(
            "<< /Type /Catalog /Pages 2 0 R /OpenAction << /S /JavaScript /JS (var n = 0;) >> /AA {aa} /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>"
        ),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
    ];
    objs.extend(extra.iter().map(|s| s.to_string()));
    pdf_of(&objs)
}

/// Did Save runs after doc_save has written the file and leaves what it changes unsaved; Did
/// Print runs after doc_print; Will Close runs on doc_close, only when the close goes ahead,
/// and can't keep the document open or ask for what needs a click.
#[test]
fn did_save_did_print_and_will_close_run_after_saving_printing_and_before_closing() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-after-actions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let js = |s: &str| format!("<< /S /JavaScript /JS ({s}) >>");
    let aa = format!(
        "<< /DS {} /DP {} /WC {} >>",
        js("n++; getField\\('log'\\).value += 'saved' + n + ';'; console.println\\('did save'\\);"),
        js("n++; getField\\('log'\\).value += 'printed' + n + ';'; console.println\\('did print'\\);"),
        js("getField\\('log'\\).value += 'closing;'; console.println\\('will close ' + n\\); this.print\\(\\);"),
    );
    std::fs::write(dir.join("after.pdf"), after_form(&aa, &[])).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap().with_clock(|| 1_700_000_000);
    let doc = tool(&mut a, "doc_open", json!({ "path": "after.pdf" }))["doc"].as_u64().unwrap();

    let saved = tool(&mut a, "doc_save", json!({ "doc": doc }));
    assert_eq!(saved["did_save"]["console"], json!(["did save"]), "{saved}");
    assert!(saved.get("will_save").is_none(), "no Will Save action: {saved}");
    assert_eq!(saved["document"]["dirty"], true, "Did Save's change came after the save: {saved}");
    assert_eq!(log(&mut a, doc), "saved1;");
    std::fs::copy(dir.join("after.pdf"), dir.join("saved.pdf")).unwrap();

    let printed = tool(&mut a, "doc_print", json!({ "doc": doc, "path": "printed.pdf" }));
    assert_eq!(printed["did_print"]["console"], json!(["did print"]), "{printed}");
    assert_eq!(log(&mut a, doc), "saved1;printed2;");
    // A print with nowhere to go fails before anything prints: Did Print doesn't run.
    assert!(a.call("doc_print", &json!({ "doc": doc })).is_err());
    assert_eq!(log(&mut a, doc), "saved1;printed2;");

    // A close refused for the unsaved change doesn't close: Will Close doesn't run.
    assert!(a.call("doc_close", &json!({ "doc": doc })).is_err());
    assert_eq!(log(&mut a, doc), "saved1;printed2;");
    let closed = tool(&mut a, "doc_close", json!({ "doc": doc, "discard_changes": true }));
    assert_eq!(closed["will_close"]["console"][0], "will close 2", "{closed}");
    assert_eq!(closed["will_close"]["requests"], json!([]), "an unasked script may not print: {closed}");
    assert!(closed["will_close"]["console"][1].as_str().is_some_and(|l| l.contains("Print requested")), "and is told so: {closed}");
    assert!(a.call("form_fields", &json!({ "doc": doc })).is_err(), "closed all the same");

    // The saved file has nothing Did Save changed.
    tool(&mut a, "js_enabled", json!({ "enabled": false }));
    let reopened = tool(&mut a, "doc_open", json!({ "path": "saved.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(log(&mut a, reopened).as_str().unwrap_or_default(), "", "Did Save's change is not in the file");
    // With JavaScript off none of them runs.
    let saved = tool(&mut a, "doc_save", json!({ "doc": reopened, "path": "off.pdf" }));
    assert!(saved.get("did_save").is_none(), "{saved}");
    let closed = tool(&mut a, "doc_close", json!({ "doc": reopened }));
    assert!(closed.get("will_close").is_none(), "{closed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Malformed Did Save, Did Print and Will Close entries never fail the save, print or close:
/// entries of the wrong type run nothing, and a `/Next` chain that loops back through indirect
/// objects runs each action once.
#[test]
fn malformed_did_save_did_print_and_will_close_entries_never_fail_the_tools() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-after-actions-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let add = |v: &str, next: &str| format!("<< /S /JavaScript /JS (getField\\('log'\\).value += '{v}';) /Next {next} >>");
    std::fs::write(dir.join("types.pdf"), after_form("<< /DS (x) /DP 7 /WC /JavaScript >>", &[])).unwrap();
    std::fs::write(dir.join("not-a-dict.pdf"), after_form("[6 0 R]", &[&add("x", "6 0 R")])).unwrap();
    std::fs::write(
        dir.join("cycle.pdf"),
        after_form("<< /DS 6 0 R /DP [7 0 R 7 0 R 99 0 R] /WC 6 0 R >>", &[&add("a", "7 0 R"), &add("b", "[6 0 R 7 0 R]")]),
    )
    .unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    for (file, after) in [("types.pdf", ""), ("not-a-dict.pdf", ""), ("cycle.pdf", "abba")] {
        let doc = tool(&mut a, "doc_open", json!({ "path": file }))["doc"].as_u64().unwrap();
        let saved = tool(&mut a, "doc_save", json!({ "doc": doc, "path": format!("saved-{file}") }));
        assert!(saved.get("did_save").is_none(), "{file}: {saved}");
        let printed = tool(&mut a, "doc_print", json!({ "doc": doc, "path": format!("printed-{file}") }));
        assert!(printed.get("did_print").is_none(), "{file}: {printed}");
        assert_eq!(log(&mut a, doc).as_str().unwrap_or_default(), after, "{file}");
        let closed = tool(&mut a, "doc_close", json!({ "doc": doc, "discard_changes": true }));
        assert_eq!(closed["closed"], doc, "{file}: {closed}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Of two fields sharing a name, `field_id` (from form_fields) picks the one js_event runs; by
/// name alone it is the first.
#[test]
fn js_event_runs_the_field_its_field_id_names() {
    let enter = |text: &str| format!("/AA << /E << /S /JavaScript /JS (getField\\('log'\\).value += '{text}';) >> >>");
    let pdf = pdf_of(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 6 0 R 7 0 R] >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (log) /V () /Rect [20 250 180 270] /P 3 0 R /F 4 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V () /Rect [20 200 180 220] /P 3 0 R /F 4 {} >>", enter("first;")),
        format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /V () /Rect [20 150 180 170] /P 3 0 R /F 4 {} >>", enter("second;")),
    ]);
    let dir = std::env::temp_dir().join(format!("pdfcraft-js-event-dup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dup.pdf"), pdf).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = tool(&mut a, "doc_open", json!({ "path": "dup.pdf" }))["doc"].as_u64().unwrap();
    let fields = tool(&mut a, "form_fields", json!({ "doc": doc }));
    let ids: Vec<u64> = fields["fields"].as_array().unwrap().iter().filter(|f| f["name"] == "dup").map(|f| f["field_id"].as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{fields}");
    tool(&mut a, "js_event", json!({ "doc": doc, "field": "dup", "field_id": ids[1], "event": "mouse_enter" }));
    assert_eq!(log(&mut a, doc), "second;");
    tool(&mut a, "js_event", json!({ "doc": doc, "field": "dup", "event": "mouse_enter" }));
    assert_eq!(log(&mut a, doc), "second;first;");
    assert!(
        a.call("js_event", &json!({ "doc": doc, "field": "log", "field_id": ids[1], "event": "mouse_enter" })).is_err(),
        "the id is another field's"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
