//! End-to-end bulk Field Properties through the same tools used by CLI and MCP.

use pdfcraft_automation::{Automation, Content, ToolError};
use pdfcraft_engine::{DocId, Edit, FieldFont, FieldProps, FieldValue, NewField, Session, field_flags};
use serde_json::{Value, json};

fn call(a: &mut Automation, args: Value) -> Value {
    match a.call("form_set_props", &args).unwrap().remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn tool(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap().remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn setup(test: &str) -> (Automation, u64, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-form-bulk-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    let bytes = s.create_blank(300.0, 400.0, 2).unwrap();
    let id = s.open("form.pdf", None, bytes, None).unwrap();
    for (name, page, x) in [("alpha", 0, 30.0), ("beta", 1, 40.0)] {
        s.apply(id, Edit::AddField { page, rect: [x, 300.0, x + 180.0, 324.0], kind: NewField::Text { multiline: false }, name: Some(name.into()) })
            .unwrap();
        s.apply(id, Edit::SetFieldValue { name: name.into(), value: FieldValue::Text(name.into()) }).unwrap();
        let mut look = s.get(id).unwrap().field_look(name).unwrap();
        look.fill = Some(if name == "alpha" { [1.0, 0.9, 0.8] } else { [0.8, 0.9, 1.0] });
        look.font = if name == "alpha" { FieldFont::Courier } else { FieldFont::Times };
        s.apply(
            id,
            Edit::SetFieldProps {
                name: name.into(),
                props: Box::new(FieldProps { look: Some(look), tooltip: Some(name.into()), ..Default::default() }),
            },
        )
        .unwrap();
    }
    std::fs::write(dir.join("form.pdf"), s.save_bytes(id).unwrap().as_slice()).unwrap();
    // Saves stamp /ModDate from the clock: pin it, or two saves a second apart differ.
    let mut a = Automation::new().with_root(&dir).unwrap().with_clock(|| 1_700_000_000);
    let id = tool(&mut a, "doc_open", json!({"path": "form.pdf"}))["doc"].as_u64().unwrap();
    (a, id, dir)
}

fn bytes(a: &Automation, id: u64) -> Vec<u8> {
    a.session().save_bytes(DocId(id)).unwrap().as_ref().clone()
}

#[test]
fn bulk_properties_are_one_undo_step_preserve_other_looks_and_survive_save() {
    let (mut a, id, dir) = setup("roundtrip");
    let before = bytes(&a, id);
    let original =
        [a.session().get(DocId(id)).unwrap().field_look("alpha").unwrap(), a.session().get(DocId(id)).unwrap().field_look("beta").unwrap()];
    let out = call(&mut a, json!({"doc": id, "fields": ["alpha", "beta"], "required": true, "appearance": {"width": 4, "border": "#FF0000"}}));
    assert_eq!(out["fields"], json!(["alpha", "beta"]));
    let doc = a.session().get(DocId(id)).unwrap();
    assert_eq!(doc.can_undo(), Some("Change field properties"));
    for (name, look) in [("alpha", original[0]), ("beta", original[1])] {
        let changed = doc.field_look(name).unwrap();
        assert_eq!(changed.width, 4.0);
        assert_eq!(changed.border, Some([1.0, 0.0, 0.0]));
        assert_eq!((changed.fill, changed.font, changed.text), (look.fill, look.font, look.text));
        let f = doc.form.iter().find(|f| f.name == name).unwrap();
        assert!(f.has(field_flags::REQUIRED));
        assert_eq!(f.value, [name]);
        assert_eq!(f.tooltip.as_deref(), Some(name));
    }
    tool(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(bytes(&a, id), before);
    tool(&mut a, "edit_redo", json!({"doc": id}));
    tool(&mut a, "doc_save", json!({"doc": id, "path": "saved.pdf"}));
    let reopened = tool(&mut a, "doc_open", json!({"path": "saved.pdf"}))["doc"].as_u64().unwrap();
    let doc = a.session().get(DocId(reopened)).unwrap();
    assert_eq!(doc.field_look("beta").unwrap().font, original[1].font);
    assert_eq!(doc.field_look("beta").unwrap().width, 4.0);
    assert_eq!(doc.form.iter().find(|f| f.name == "beta").unwrap().value, ["beta"]);
    let image = a.call("page_render", &json!({"doc": reopened, "page": 2, "dpi": 72})).unwrap();
    assert!(image.iter().any(|c| matches!(c, Content::Png { width: 300, height: 400, .. })));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bulk_property_arguments_are_strict_and_fail_without_any_edit() {
    let (mut a, id, dir) = setup("invalid");
    let before = bytes(&a, id);
    let cases = [
        json!({"doc": id, "required": true}),
        json!({"doc": id, "field": "alpha", "fields": ["beta"], "required": true}),
        json!({"doc": id, "fields": [], "required": true}),
        json!({"doc": id, "fields": ["alpha", "alpha"], "required": true}),
        json!({"doc": id, "fields": ["alpha", 7], "required": true}),
        json!({"doc": id, "fields": ["alpha", "beta"], "name": "same"}),
        json!({"doc": id, "fields": ["alpha", "beta"], "rotation": 90}),
        json!({"doc": id, "fields": ["alpha", "beta"], "rect": [0, 0, 20, 20]}),
        json!({"doc": id, "fields": ["alpha", "beta"]}),
        json!({"doc": id, "fields": ["alpha", "beta"], "required": "yes"}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"style": "invalid"}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"width": -1}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"width": "thick"}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"style": 3}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"font": true}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "appearance": {"unknown": true}}),
        json!({"doc": id, "fields": ["alpha", "beta"], "requred": true}),
        json!({"doc": id, "fields": vec!["alpha"; 1001], "required": true}),
    ];
    for args in cases {
        assert!(matches!(a.call("form_set_props", &args), Err(ToolError::InvalidArgs(_))), "{args}");
        assert_eq!(bytes(&a, id), before, "{args}");
    }
    assert!(matches!(a.call("form_set_props", &json!({"doc": id, "fields": ["alpha", "missing"], "required": true})), Err(ToolError::Failed(_))));
    assert_eq!(bytes(&a, id), before);
    assert!(a.session().get(DocId(id)).unwrap().can_undo().is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_locked_last_field_rolls_back_earlier_changes_and_unlocks_with_the_batch() {
    let (mut a, id, dir) = setup("locked");
    call(&mut a, json!({"doc": id, "field": "beta", "locked": true}));
    let before = bytes(&a, id);
    assert!(matches!(
        a.call("form_set_props", &json!({"doc": id, "fields": ["alpha", "beta"], "tooltip": "Shared help"})),
        Err(ToolError::Failed(_))
    ));
    assert_eq!(bytes(&a, id), before);
    assert_eq!(a.session().get(DocId(id)).unwrap().can_undo(), Some("Change field properties"));
    call(&mut a, json!({"doc": id, "fields": ["alpha", "beta"], "locked": false, "tooltip": "Shared help"}));
    assert!(a.session().get(DocId(id)).unwrap().form.iter().all(|f| !f.locked() && f.tooltip.as_deref() == Some("Shared help")));
    tool(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(bytes(&a, id), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bulk_options_apply_per_field_and_refuse_an_invalid_comb_atomically() {
    let (mut a, id, dir) = setup("options");
    let before = bytes(&a, id);
    assert!(matches!(a.call("form_set_props", &json!({"doc": id, "fields": ["alpha", "beta"], "flags": {"comb": true}})), Err(ToolError::Failed(_))));
    assert_eq!(bytes(&a, id), before);
    call(&mut a, json!({"doc": id, "fields": ["alpha", "beta"], "max_length": 20, "align": "center", "flags": {"comb": true, "spell_check": false}}));
    let doc = a.session().get(DocId(id)).unwrap();
    assert!(doc.form.iter().all(|f| f.max_len == Some(20) && f.quadding == 1 && f.has(field_flags::COMB) && f.has(field_flags::DO_NOT_SPELL_CHECK)));
    assert_eq!(doc.form.iter().map(|f| f.value[0].as_str()).collect::<Vec<_>>(), ["alpha", "beta"]);
    tool(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(bytes(&a, id), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn button_export_edit_is_one_undo_step_and_survives_save() {
    let (mut a, id, dir) = setup("button-export");
    tool(&mut a, "form_add_field", json!({"doc": id, "page": 1, "type": "checkbox", "name": "agree", "rect": [20, 20, 40, 40]}));
    tool(&mut a, "form_fill", json!({"doc": id, "values": {"agree": true}}));
    let before = bytes(&a, id);
    call(&mut a, json!({"doc": id, "field": "agree", "export_value": "Accepted", "widget": 1}));
    let f = tool(&mut a, "form_fields", json!({"doc": id}));
    let check = f["fields"].as_array().unwrap().iter().find(|f| f["name"] == "agree").unwrap();
    assert_eq!(check["export_values"], json!(["Accepted"]));
    assert_eq!(check["value"], true);
    tool(&mut a, "edit_undo", json!({"doc": id}));
    assert_eq!(bytes(&a, id), before);
    tool(&mut a, "edit_redo", json!({"doc": id}));
    tool(&mut a, "doc_save", json!({"doc": id, "path": "saved.pdf"}));
    let reopened = tool(&mut a, "doc_open", json!({"path": "saved.pdf"}))["doc"].as_u64().unwrap();
    let f = tool(&mut a, "form_fields", json!({"doc": reopened}));
    let check = f["fields"].as_array().unwrap().iter().find(|f| f["name"] == "agree").unwrap();
    assert_eq!(check["export_values"], json!(["Accepted"]));
    assert_eq!(check["value"], true);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn form_fill_surfaces_calculation_alerts_and_requests() {
    let (mut a, id, dir) = setup("calc-alerts");
    tool(
        &mut a,
        "form_set_script",
        json!({
            "doc": id,
            "field": "beta",
            "event": "calculate",
            "script": "if (this.getField(\"alpha\").value == \"trigger\") app.alert(\"Alert triggered\");"
        }),
    );
    let normal = tool(&mut a, "form_fill", json!({"doc": id, "values": {"alpha": "normal"}}));
    assert!(normal.get("alerts").is_none());
    let triggered = tool(&mut a, "form_fill", json!({"doc": id, "values": {"alpha": "trigger"}}));
    assert_eq!(triggered["alerts"][0]["message"], "Alert triggered");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn form_fill_multiple_alerts_and_viewer_requests() {
    let (mut a, id, dir) = setup("multi-alerts");
    tool(
        &mut a,
        "form_set_script",
        json!({
            "doc": id,
            "field": "beta",
            "event": "calculate",
            "script": "if (this.getField(\"alpha\").value == \"both\") { app.alert(\"Alert 1\"); app.alert(\"Alert 2\"); app.beep(0); }"
        }),
    );

    // No output when condition not met
    let no_out = tool(&mut a, "form_fill", json!({"doc": id, "values": {"alpha": "quiet"}}));
    assert!(no_out.get("alerts").is_none());
    assert!(no_out.get("requests").is_none());

    // Both alerts and requests captured
    let res = tool(&mut a, "form_fill", json!({"doc": id, "values": {"alpha": "both"}}));
    let alerts = res["alerts"].as_array().expect("alerts array");
    assert_eq!(alerts.len(), 2);
    assert_eq!(alerts[0]["message"], "Alert 1");
    assert_eq!(alerts[1]["message"], "Alert 2");

    let requests = res["requests"].as_array().expect("requests array");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["beep"], true);

    // Repeated invocation: output is taken, so subsequent edits do not duplicate old alerts
    let second = tool(&mut a, "form_fill", json!({"doc": id, "values": {"alpha": "quiet"}}));
    assert!(second.get("alerts").is_none());
    assert!(second.get("requests").is_none());

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn form_fill_two_open_documents_do_not_leak_alerts() {
    let (mut a, id1, dir) = setup("doc-leak-1");
    let id2 = tool(&mut a, "doc_open", json!({"path": "form.pdf"}))["doc"].as_u64().unwrap();
    assert_ne!(id1, id2);

    // Set script on doc 1 only
    tool(
        &mut a,
        "form_set_script",
        json!({
            "doc": id1,
            "field": "beta",
            "event": "calculate",
            "script": "if (this.getField(\"alpha\").value == \"leak_test\") app.alert(\"Doc1 Alert\");"
        }),
    );

    // Fill in Doc 2: should have NO alerts
    let out2 = tool(&mut a, "form_fill", json!({"doc": id2, "values": {"alpha": "leak_test"}}));
    assert!(out2.get("alerts").is_none(), "Doc 2 must not receive Doc 1 scripts or alerts");

    // Fill in Doc 1: receives Doc1 Alert
    let out1 = tool(&mut a, "form_fill", json!({"doc": id1, "values": {"alpha": "leak_test"}}));
    assert_eq!(out1["alerts"][0]["message"], "Doc1 Alert");

    // Further edit in Doc 2 still has no alerts
    let out2_again = tool(&mut a, "form_fill", json!({"doc": id2, "values": {"alpha": "leak_test_again"}}));
    assert!(out2_again.get("alerts").is_none());

    // Close doc 1 with pending state, doc 2 remains functional
    tool(&mut a, "doc_close", json!({"doc": id1, "discard_changes": true}));
    let out2_after = tool(&mut a, "form_fill", json!({"doc": id2, "values": {"alpha": "final"}}));
    assert!(out2_after.get("alerts").is_none());

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn form_fill_undo_redo_does_not_replay_alert_side_effects() {
    let (mut a, id, dir) = setup("undo-alert");
    tool(
        &mut a,
        "form_set_script",
        json!({
            "doc": id,
            "field": "beta",
            "event": "calculate",
            "script": "if (this.getField(\"alpha\").value == \"warn\") app.alert(\"Warning message\");"
        }),
    );

    let res = tool(&mut a, "form_fill", json!({"doc": id, "values": {"alpha": "warn"}}));
    assert_eq!(res["alerts"][0]["message"], "Warning message");

    // Undo should restore previous values without re-triggering alert
    let undo_res = tool(&mut a, "edit_undo", json!({"doc": id}));
    assert!(undo_res.get("alerts").is_none(), "Undo must not replay alerts");

    // Redo should restore snapshot without re-triggering alert
    let redo_res = tool(&mut a, "edit_redo", json!({"doc": id}));
    assert!(redo_res.get("alerts").is_none(), "Redo must not replay alerts");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn form_fill_invalid_script_does_not_panic_or_break_fill() {
    let (mut a, id, dir) = setup("invalid-script");
    tool(
        &mut a,
        "form_set_script",
        json!({
            "doc": id,
            "field": "beta",
            "event": "calculate",
            "script": "syntax error !!! @#$%"
        }),
    );

    // Filling form with syntax error in calculation script does not crash or panic
    let res = tool(&mut a, "form_fill", json!({"doc": id, "values": {"alpha": "test"}}));
    assert_eq!(res["filled"], 1);
    let errors = res["errors"].as_array().expect("errors array");
    assert!(!errors.is_empty(), "script errors must be captured and reported in apply output");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn form_fill_surfaces_script_errors_and_console_diagnostics() {
    let (mut a, id1, dir) = setup("diagnostics-test-1");
    let id2 = tool(&mut a, "doc_open", json!({"path": "form.pdf"}))["doc"].as_u64().unwrap();
    assert_ne!(id1, id2);

    // Set script on doc 1 that emits alerts, viewer requests, console output, and throws an error
    tool(
        &mut a,
        "form_set_script",
        json!({
            "doc": id1,
            "field": "beta",
            "event": "calculate",
            "script": "if (this.getField(\"alpha\").value == \"trigger\") { console.println(\"calc diagnostic 1\"); console.println(\"calc diagnostic 2\"); app.alert(\"Alert diag\"); app.beep(0); throw new Error(\"calc failed intentionally\"); }"
        }),
    );

    // Doc 2 edit: completely clean, no diagnostics from doc 1 leaked
    let res2 = tool(&mut a, "form_fill", json!({"doc": id2, "values": {"alpha": "trigger"}}));
    assert!(res2.get("alerts").is_none());
    assert!(res2.get("requests").is_none());
    assert!(res2.get("console").is_none());
    assert!(res2.get("errors").is_none());

    // Doc 1 edit: surfaces alerts, requests, console, and errors
    let res1 = tool(&mut a, "form_fill", json!({"doc": id1, "values": {"alpha": "trigger"}}));
    let alerts = res1["alerts"].as_array().expect("alerts array");
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["message"], "Alert diag");

    let reqs = res1["requests"].as_array().expect("requests array");
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0]["beep"], true);

    let console = res1["console"].as_array().expect("console array");
    assert_eq!(console.len(), 2);
    assert_eq!(console[0], "calc diagnostic 1");
    assert_eq!(console[1], "calc diagnostic 2");

    let errors = res1["errors"].as_array().expect("errors array");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].as_str().unwrap().contains("calc failed intentionally"));

    // Repeated call: diagnostics are drained, so subsequent clean fill has none
    let res1_clean = tool(&mut a, "form_fill", json!({"doc": id1, "values": {"alpha": "other"}}));
    assert!(res1_clean.get("alerts").is_none());
    assert!(res1_clean.get("requests").is_none());
    assert!(res1_clean.get("console").is_none());
    assert!(res1_clean.get("errors").is_none());

    // Undo: does NOT replay side effects or diagnostics
    let undo_res = tool(&mut a, "edit_undo", json!({"doc": id1}));
    assert!(undo_res.get("alerts").is_none());
    assert!(undo_res.get("requests").is_none());
    assert!(undo_res.get("console").is_none());
    assert!(undo_res.get("errors").is_none());

    // Redo: does NOT replay side effects or diagnostics
    let redo_res = tool(&mut a, "edit_redo", json!({"doc": id1}));
    assert!(redo_res.get("alerts").is_none());
    assert!(redo_res.get("requests").is_none());
    assert!(redo_res.get("console").is_none());
    assert!(redo_res.get("errors").is_none());

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn form_fill_surfaces_document_level_script_errors_and_isolates_documents() {
    let (mut a, id1, dir) = setup("doc-script-err-1");
    let id2 = tool(&mut a, "doc_open", json!({"path": "form.pdf"}))["doc"].as_u64().unwrap();
    assert_ne!(id1, id2);

    // 1. Add a genuine failing document-level script to doc 1
    tool(
        &mut a,
        "js_set_document_script",
        json!({
            "doc": id1,
            "name": "init_fail",
            "script": "throw new Error(\"DocumentLevelInitFailure\");"
        }),
    );

    // 2. Add a field-level script to doc 1 that also has console lines, alerts, and field errors
    tool(
        &mut a,
        "form_set_script",
        json!({
            "doc": id1,
            "field": "beta",
            "event": "calculate",
            "script": "console.println(\"beta calc\"); app.alert(\"Calc alert\"); throw new Error(\"FieldLevelCalcFailure\");"
        }),
    );

    // 3. Edit in doc 2: must have zero errors or diagnostics (strict cross-document isolation)
    let res2 = tool(&mut a, "form_fill", json!({"doc": id2, "values": {"alpha": "test"}}));
    assert!(res2.get("errors").is_none());
    assert!(res2.get("doc_errors").is_none());
    assert!(res2.get("console").is_none());
    assert!(res2.get("alerts").is_none());

    // 4. Edit in doc 1: surfaces both doc_errors and field-level errors
    let res1 = tool(&mut a, "form_fill", json!({"doc": id1, "values": {"alpha": "trigger"}}));
    let doc_errs = res1["doc_errors"].as_array().expect("doc_errors array");
    assert_eq!(doc_errs.len(), 1);
    assert!(doc_errs[0].as_str().unwrap().contains("DocumentLevelInitFailure"));

    let errs = res1["errors"].as_array().expect("errors array");
    assert!(errs.iter().any(|e| e.as_str().unwrap().contains("DocumentLevelInitFailure")));
    assert!(errs.iter().any(|e| e.as_str().unwrap().contains("FieldLevelCalcFailure")));

    let console = res1["console"].as_array().expect("console array");
    assert_eq!(console[0], "beta calc");

    let alerts = res1["alerts"].as_array().expect("alerts array");
    assert_eq!(alerts[0]["message"], "Calc alert");

    // 5. Remove the failing document script; subsequent fill with clean condition has no doc_errors or errors
    tool(&mut a, "js_set_document_script", json!({ "doc": id1, "name": "init_fail" }));
    // Also set field script with condition so "other" doesn't trigger field error
    tool(
        &mut a,
        "form_set_script",
        json!({
            "doc": id1,
            "field": "beta",
            "event": "calculate",
            "script": "if (this.getField(\"alpha\").value == \"trigger\") { console.println(\"beta calc\"); app.alert(\"Calc alert\"); throw new Error(\"FieldLevelCalcFailure\"); }"
        }),
    );
    let res1_clean = tool(&mut a, "form_fill", json!({"doc": id1, "values": {"alpha": "other"}}));
    assert!(res1_clean.get("doc_errors").is_none(), "doc_errors must be empty after document script removed");
    assert!(res1_clean.get("errors").is_none(), "errors must be empty when condition not met");
    assert!(res1_clean.get("alerts").is_none());
    assert!(res1_clean.get("console").is_none());

    // 6. Undo/Redo does not replay diagnostics
    let undo = tool(&mut a, "edit_undo", json!({"doc": id1}));
    assert!(undo.get("doc_errors").is_none());
    assert!(undo.get("errors").is_none());
    let redo = tool(&mut a, "edit_redo", json!({"doc": id1}));
    assert!(redo.get("doc_errors").is_none());
    assert!(redo.get("errors").is_none());

    std::fs::remove_dir_all(dir).unwrap();
}
