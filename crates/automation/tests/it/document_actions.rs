//! A form's document actions and pointer/focus scripts through the tool table (#269, ported
//! from #344): the open action runs on doc_open, Will Save on doc_save, Will Print on
//! doc_print, and js_event runs a widget's pointer and focus scripts, all in one realm.

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
