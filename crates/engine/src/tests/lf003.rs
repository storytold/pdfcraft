//! LF-003: ResetForm must run custom AcroForm Calculate scripts after restoring /DV.
//! The fixture is built in memory from PdfCraft's own form authoring API.

use super::*;

fn form(all: bool, native: bool) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
    let text = pdfcraft_forms::NewField::Text { multiline: false };
    pdfcraft_forms::add_field(&mut doc, 0, [20.0, 240.0, 180.0, 260.0], &text, Some("price")).unwrap();
    pdfcraft_forms::add_field(&mut doc, 0, [20.0, 200.0, 180.0, 220.0], &text, Some("total")).unwrap();
    pdfcraft_forms::add_field(
        &mut doc,
        0,
        [20.0, 160.0, 180.0, 180.0],
        &pdfcraft_forms::NewField::Button { caption: "Reset".into() },
        Some("reset_button"),
    )
    .unwrap();

    pdfcraft_forms::set_value(&mut doc, "price", &FieldValue::Text("42".into())).unwrap();
    pdfcraft_forms::set_value(&mut doc, "total", &FieldValue::Text("84".into())).unwrap();
    pdfcraft_forms::set_props(&mut doc, "price", &FieldProps { default_value: Some(Some("17".into())), ..Default::default() }).unwrap();
    if all {
        pdfcraft_forms::set_props(&mut doc, "total", &FieldProps { default_value: Some(Some("999".into())), ..Default::default() }).unwrap();
    }
    if native {
        pdfcraft_forms::set_props(
            &mut doc,
            "total",
            &FieldProps { calculate: Some(form_scripts::Calculate::Notation("price * 2".into())), ..Default::default() },
        )
        .unwrap();
    } else {
        pdfcraft_forms::set_field_script(
            &mut doc,
            "total",
            "calculate",
            Some("console.println('lf003-calculate'); event.value = Number(getField('price').value) * 2;"),
        )
        .unwrap();
    }
    pdfcraft_forms::set_calculation_order(&mut doc, &["total".into()]).unwrap();
    let action = if all { "this.resetForm();" } else { "this.resetForm(['price']);" };
    pdfcraft_forms::set_field_actions(
        &mut doc,
        "reset_button",
        &[(pdfcraft_forms::Trigger::MouseUp, pdfcraft_forms::FieldAction::JavaScript(action.into()))],
    )
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut session = Session::new().with_clock(|| 1_700_000_000);
    let id = session.open("lf003.pdf", None, Arc::new(bytes), None).unwrap();
    (session, id)
}

fn value(session: &Session, id: DocId, name: &str) -> Vec<String> {
    session.get(id).unwrap().form.iter().find(|field| field.name == name).unwrap().value.clone()
}

fn with_document_script(bytes: Vec<u8>, script: &str) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    pdfcraft_forms::set_document_script(&mut doc, "init", Some(script)).unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

#[test]
fn reset_calculate_reads_defaults_despite_unguarded_document_initializer() {
    for all in [false, true] {
        let (mut session, id) = open(with_document_script(form(all, false), "getField('price').value = '0';"));
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
        let _ = session.take_js_output(id);

        session.apply(id, Edit::ResetForm { names: if all { None } else { Some(vec!["price".into()]) } }).unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));

        let saved = session.save_bytes(id).unwrap();
        let (reopened, reopened_id) = open(saved.as_ref().clone());
        assert_eq!((value(&reopened, reopened_id, "price"), value(&reopened, reopened_id, "total")), (vec!["17".into()], vec!["34".into()]));
        session.undo(id).unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
        session.redo(id).unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
    }
}

#[test]
fn button_reset_and_unchanged_default_are_not_overwritten_by_document_bootstrap() {
    for all in [false, true] {
        let (mut session, id) = open(with_document_script(form(all, false), "getField('price').value = '0';"));
        let action = if all { "this.resetForm();" } else { "this.resetForm(['price']);" };
        let outcome = session.run_javascript(id, action, Some("reset_button")).unwrap();
        assert!(outcome.error.is_none(), "{outcome:?}");
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
    }

    let mut doc = pdfcraft_cos::Document::open(Arc::new(form(false, false))).unwrap();
    pdfcraft_forms::set_value(&mut doc, "price", &FieldValue::Text("17".into())).unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let (mut session, id) = open(with_document_script(bytes, "getField('price').value = '0';"));
    session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
}

#[test]
fn guarded_conditional_and_target_document_initializers_leave_reset_values_coherent() {
    for script in [
        "if (event.name === 'Open') getField('price').value = '0';",
        "if (getField('price').value !== 17) getField('price').value = '0';",
        "getField('total').value = '0';",
    ] {
        let (mut session, id) = open(with_document_script(form(false, false), script));
        session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]), "{script}");
    }
    let (mut session, id) = open(with_document_script(form(false, true), "getField('price').value = '0';"));
    session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
}

#[test]
fn reset_bootstrap_keeps_document_helpers_and_real_calculate_effects() {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(form(false, false))).unwrap();
    pdfcraft_forms::add_field(&mut doc, 0, [20.0, 120.0, 180.0, 140.0], &pdfcraft_forms::NewField::Text { multiline: false }, Some("note")).unwrap();
    pdfcraft_forms::set_field_script(
        &mut doc,
        "total",
        "calculate",
        Some("getField('note').value = event.name + ':' + event.target.name + ':' + event.rc; event.value = totalForPrice(); console.println('calculate');"),
    )
    .unwrap();
    pdfcraft_forms::set_field_script(&mut doc, "total", "format", Some("console.println('format'); event.value = String(event.value) + '!';"))
        .unwrap();
    pdfcraft_forms::set_document_script(
        &mut doc,
        "init",
        Some("var factor = 2; function totalForPrice() { return Number(getField('price').value) * factor; } getField('price').value = '0'; this.resetForm(['price']); console.println('bootstrap');"),
    )
    .unwrap();
    let (mut session, id) = open(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
    let _ = session.take_js_output(id);
    session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
    assert_eq!(
        (value(&session, id, "price"), value(&session, id, "total"), value(&session, id, "note")),
        (vec!["17".into()], vec!["34".into()], vec!["Calculate:total:true".into()])
    );
    let output = session.take_js_output(id);
    assert_eq!(output.console, ["bootstrap", "calculate", "format"]);
    assert!(output.requests.is_empty(), "bootstrap reset requests are not applied or reported: {output:?}");
}

#[test]
fn targeted_reset_recalculates_custom_script_and_survives_history_and_save() {
    let (mut session, id) = open(form(false, false));
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
    let _ = session.take_js_output(id);

    session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
    assert_eq!(session.take_js_output(id).console, ["lf003-calculate"], "Calculate runs once");

    let saved = session.save_bytes(id).unwrap();
    let (reopened, reopened_id) = open(saved.as_ref().clone());
    assert_eq!((value(&reopened, reopened_id, "price"), value(&reopened, reopened_id, "total")), (vec!["17".into()], vec!["34".into()]));
    session.undo(id).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
    session.redo(id).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
}

#[test]
fn full_reset_recalculates_custom_script_after_restoring_total_default() {
    let (mut session, id) = open(form(true, false));
    assert_eq!(session.get(id).unwrap().form.iter().find(|field| field.name == "total").unwrap().default, ["999"]);
    let _ = session.take_js_output(id);

    session.apply(id, Edit::ResetForm { names: None }).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
    assert_eq!(session.take_js_output(id).console, ["lf003-calculate"], "Calculate runs once after the whole reset");
}

#[test]
fn reset_requested_by_a_button_script_recalculates_both_scopes() {
    for all in [false, true] {
        let (mut session, id) = open(form(all, false));
        let action = session
            .get(id)
            .unwrap()
            .field_actions("reset_button")
            .into_iter()
            .find_map(|(trigger, action)| match (trigger, action) {
                (pdfcraft_forms::Trigger::MouseUp, pdfcraft_forms::FieldAction::JavaScript(script)) => Some(script),
                _ => None,
            })
            .unwrap();
        let _ = session.take_js_output(id);
        let result = session.run_javascript(id, &action, Some("reset_button")).unwrap();
        assert!(result.error.is_none(), "{result:?}");
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
        assert_eq!(session.take_js_output(id).console, ["lf003-calculate"], "button reset recalculates once");
        assert_eq!(session.get(id).unwrap().can_undo(), Some("Run JavaScript"));
        session.undo(id).unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
        session.redo(id).unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
    }
}

#[test]
fn reset_in_mixed_batch_recalculates_after_the_reset() {
    let (mut session, id) = open(form(false, false));
    let _ = session.take_js_output(id);
    session
        .apply(
            id,
            Edit::Batch {
                label: "Write then reset".into(),
                edits: vec![
                    Edit::SetFieldValue { name: "price".into(), value: FieldValue::Text("43".into()) },
                    Edit::ResetForm { names: Some(vec!["price".into()]) },
                ],
            },
        )
        .unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
    assert_eq!(session.take_js_output(id).console, ["lf003-calculate", "lf003-calculate"]);
    assert_eq!(session.get(id).unwrap().can_undo(), Some("Write then reset"));
    session.undo(id).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));

    session
        .apply(
            id,
            Edit::Batch {
                label: "Reset then write".into(),
                edits: vec![
                    Edit::ResetForm { names: Some(vec!["price".into()]) },
                    Edit::SetFieldValue { name: "price".into(), value: FieldValue::Text("43".into()) },
                ],
            },
        )
        .unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["43".into()], vec!["86".into()]));
    session.undo(id).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
}

fn form_with_dependent(order: &[&str]) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(form(false, false))).unwrap();
    pdfcraft_forms::add_field(&mut doc, 0, [20.0, 120.0, 180.0, 140.0], &pdfcraft_forms::NewField::Text { multiline: false }, Some("tax")).unwrap();
    pdfcraft_forms::set_value(&mut doc, "tax", &FieldValue::Text("85".into())).unwrap();
    pdfcraft_forms::set_field_script(&mut doc, "tax", "calculate", Some("event.value = Number(getField('total').value) + 1;")).unwrap();
    pdfcraft_forms::set_calculation_order(&mut doc, &order.iter().map(|name| (*name).into()).collect::<Vec<_>>()).unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

#[test]
fn reset_obeys_both_calculation_orders() {
    for (order, expected_tax) in [(["total", "tax"], "35"), (["tax", "total"], "85")] {
        let (mut session, id) = open(with_document_script(form_with_dependent(&order), "getField('price').value = '0';"));
        let _ = session.take_js_output(id);
        session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
        assert_eq!(value(&session, id, "price"), ["17"]);
        assert_eq!(value(&session, id, "total"), ["34"]);
        assert_eq!(value(&session, id, "tax"), [expected_tax], "order {order:?}");
    }
}

#[test]
fn javascript_off_preserves_scripted_defaults_but_native_af_still_calculates() {
    for all in [false, true] {
        let (mut session, id) = open(with_document_script(form(all, false), "getField('price').value = '0';"));
        session.set_javascript(false);
        session.apply(id, Edit::ResetForm { names: if all { None } else { Some(vec!["price".into()]) } }).unwrap();
        assert_eq!(value(&session, id, "price"), ["17"]);
        assert_eq!(value(&session, id, "total"), [if all { "999" } else { "84" }]);
        assert!(session.take_js_output(id).console.is_empty());
    }
    let (mut session, id) = open(with_document_script(form(true, true), "getField('price').value = '0';"));
    session.set_javascript(false);
    session.apply(id, Edit::ResetForm { names: None }).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
}

#[test]
fn invalid_reset_target_leaves_document_and_history_unchanged() {
    let (mut session, id) = open(form(false, false));
    let before = session.save_bytes(id).unwrap();
    let err = session.apply(id, Edit::ResetForm { names: Some(vec!["missing".into()]) }).unwrap_err();
    assert!(err.to_string().contains("missing"), "{err}");
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
    assert_eq!(session.get(id).unwrap().can_undo(), None);
    assert_eq!(session.save_bytes(id).unwrap().as_ref(), before.as_ref());
}

#[test]
fn failing_calculate_reports_error_and_keeps_the_restored_values() {
    for all in [false, true] {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(form(all, false))).unwrap();
        pdfcraft_forms::set_field_script(&mut doc, "total", "calculate", Some("throw new Error('lf003 calculation failed');")).unwrap();
        let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
        let (mut session, id) = open(with_document_script(bytes, "getField('price').value = '0';"));
        let _ = session.take_js_output(id);
        session.apply(id, Edit::ResetForm { names: if all { None } else { Some(vec!["price".into()]) } }).unwrap();
        assert_eq!(value(&session, id, "price"), ["17"]);
        assert_eq!(value(&session, id, "total"), [if all { "999" } else { "84" }]);
        let output = session.take_js_output(id);
        assert!(output.errors.iter().any(|error| error.contains("lf003 calculation failed")), "{output:?}");
        session.undo(id).unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
    }
}

#[test]
fn invalid_real_calculate_write_rolls_back_the_entire_reset() {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(form(false, false))).unwrap();
    pdfcraft_forms::add_field(&mut doc, 0, [20.0, 120.0, 180.0, 140.0], &pdfcraft_forms::NewField::Text { multiline: false }, Some("note")).unwrap();
    pdfcraft_forms::set_props(&mut doc, "note", &FieldProps { max_len: Some(Some(3)), ..Default::default() }).unwrap();
    pdfcraft_forms::set_field_script(
        &mut doc,
        "total",
        "calculate",
        Some("getField('note').value = 'too long'; event.value = Number(getField('price').value) * 2;"),
    )
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let (mut session, id) = open(with_document_script(bytes, "getField('price').value = '0';"));
    let before = session.save_bytes(id).unwrap();
    let err = session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap_err();
    assert!(err.to_string().contains("note") && err.to_string().contains("3"), "{err}");
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
    assert_eq!(session.get(id).unwrap().can_undo(), None);
    assert_eq!(session.save_bytes(id).unwrap().as_ref(), before.as_ref());
}

#[test]
fn format_runs_once_and_reset_bootstrap_diagnostics_are_not_repeated() {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(form(false, false))).unwrap();
    pdfcraft_forms::set_document_script(&mut doc, "init", Some("console.println('doc-run'); app.alert('doc-alert'); this.print();")).unwrap();
    pdfcraft_forms::set_field_script(
        &mut doc,
        "total",
        "format",
        Some("console.println('format'); this.resetForm(['price']); event.value = String(event.value) + '!';"),
    )
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    for reset in [false, true] {
        let (mut session, id) = open(bytes.clone());
        let _ = session.take_js_output(id);
        if reset {
            session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
            assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
        } else {
            session.apply(id, Edit::SetFieldValue { name: "price".into(), value: FieldValue::Text("43".into()) }).unwrap();
            assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["43".into()], vec!["86".into()]));
        }
        let output = session.take_js_output(id);
        let expected: &[&str] = if reset { &["doc-run", "lf003-calculate", "format"] } else { &["doc-run", "lf003-calculate", "doc-run", "format"] };
        assert_eq!(output.console, expected);
        assert_eq!(output.alerts.len(), if reset { 1 } else { 2 }, "bootstrap is deduplicated only within the reset");
        assert!(output.alerts.iter().all(|alert| alert.text == "doc-alert"));
        if reset {
            assert_eq!(output.requests, [js::Request::Reset(vec!["price".into()])], "only the true Format request is logged");
        } else {
            assert_eq!(output.requests, [js::Request::Print, js::Request::Print, js::Request::Reset(vec!["price".into()])]);
        }
    }
}

#[test]
fn reset_obeys_form_filling_permissions_without_committing_a_partial_edit() {
    let (mut owner, id) = open(form(false, false));
    owner
        .apply(id, Edit::Protect(Protection { permissions_password: Some("owner".into()), changes: Changes::None, ..Protection::default() }))
        .unwrap();
    let protected = owner.save_bytes(id).unwrap();
    let mut reader = Session::new();
    let id = reader.open("restricted.pdf", None, protected.clone(), None).unwrap();
    assert_eq!(reader.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }), Err(EditError::NotPermitted("filling in form fields")));
    assert_eq!((value(&reader, id, "price"), value(&reader, id, "total")), (vec!["42".into()], vec!["84".into()]));
    assert_eq!(reader.get(id).unwrap().can_undo(), None);
    assert_eq!(reader.save_bytes(id).unwrap().as_ref(), protected.as_ref());
}

#[test]
fn reset_captured_document_event_reference_stays_coherent() {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(form(false, false))).unwrap();
    pdfcraft_forms::set_field_script(&mut doc, "total", "calculate", Some("event.value = savedName() + ':' + event.name;")).unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let (mut session, id) =
        open(with_document_script(bytes, "var savedEvent = event; savedEvent.name = 'Wrong'; function savedName() { return savedEvent.name; }"));
    session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["Calculate:Calculate".into()]));
}

#[test]
fn reset_recalculates_when_document_event_is_configurably_nonwritable() {
    let (mut session, id) = open(with_document_script(
        form(false, false),
        "Object.defineProperty(this, 'event', { value: 'blocked', writable: false, configurable: true });",
    ));
    session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
    assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["17".into()], vec!["34".into()]));
}

#[test]
fn unrestorable_reset_event_returns_an_error_without_committing_the_document() {
    for initializer in [
        "Object.defineProperty(this, 'event', { value: 'blocked', writable: false, configurable: false });",
        "Object.defineProperty(this, 'event', { get() { return 'blocked'; }, configurable: false });",
        "Object.freeze(event);",
    ] {
        let (mut session, id) = open(with_document_script(form(false, false), initializer));
        let before = session.save_bytes(id).unwrap();
        let error = session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap_err();
        assert!(error.to_string().contains("reset event could not be restored"), "{initializer}: {error}");
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
        assert_eq!(session.save_bytes(id).unwrap().as_ref(), before.as_ref());
        assert_eq!(session.get(id).unwrap().can_undo(), None);
    }
}

#[test]
fn reset_event_protection_does_not_leak_to_a_later_batch_field_edit() {
    for all in [false, true] {
        let (mut session, id) = open(with_document_script(form(all, false), "this.print();"));
        let _ = session.take_js_output(id);
        session
            .apply(
                id,
                Edit::Batch {
                    label: "Reset then write".into(),
                    edits: vec![
                        Edit::ResetForm { names: if all { None } else { Some(vec!["price".into()]) } },
                        Edit::SetFieldValue { name: "price".into(), value: FieldValue::Text("43".into()) },
                    ],
                },
            )
            .unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["43".into()], vec!["86".into()]));
        // This fixture has a Calculate action and no Format action: only the later edit prints.
        assert_eq!(session.take_js_output(id).requests, [js::Request::Print]);
        session.undo(id).unwrap();
        assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
    }
}

fn normal_appearance_text(bytes: Arc<Vec<u8>>, name: &str) -> String {
    let doc = pdfcraft_cos::Document::open(bytes).unwrap();
    let all = pdfcraft_forms::fields(&doc);
    let field = all.iter().find(|f| f.name == name).unwrap();
    let widget = doc.get(field.widgets.first().unwrap().obj);
    let ap = doc.resolve(widget.as_dict().unwrap().get(b"AP").unwrap());
    let normal = doc.resolve(ap.as_dict().unwrap().get(b"N").unwrap());
    let pdfcraft_cos::Object::Stream(stream) = normal.as_ref() else { panic!("normal appearance must be a stream") };
    // These synthetic fixtures use a simple standard-14 font and ASCII Tj strings.
    let text: Vec<u8> = pdfcraft_content::parse(&stream.decoded().unwrap())
        .ops
        .into_iter()
        .filter(|op| op.is("Tj"))
        .flat_map(|op| op.operands.first().unwrap().as_string().unwrap().bytes.clone())
        .collect();
    String::from_utf8(text).unwrap()
}

#[test]
fn reset_lexical_event_failure_preserves_bytes_history_dirty_and_generation() {
    for (initializer, dependent) in [
        ("let event = { value: '0', name: 'Wrong' };", false),
        ("const event = { value: '0', name: 'Wrong' };", false),
        ("let event = 7;", false),
        ("let event;", false),
        ("throw new Error('before initialization'); let event;", false),
        ("let event = {value:'0'}; function totalForPrice() { event.value = '34'; return event.value; }", false),
        ("const event = globalThis.event.name === 'Format' ? {} : globalThis.event;", false),
        ("const event = globalThis.event.targetName === 'tax' ? {} : globalThis.event;", true),
    ] {
        for dirty in [false, true] {
            let bytes = if dependent { form_with_dependent(&["total", "tax"]) } else { form(false, false) };
            let mut cos = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
            pdfcraft_forms::set_field_script(&mut cos, "total", "format", Some("event.value = String(event.value) + '!';")).unwrap();
            let (mut session, id) = open(with_document_script(pdfcraft_cos::write_full(&cos, &Default::default()).unwrap(), initializer));
            if dirty {
                session
                    .apply(
                        id,
                        Edit::SetFieldProps {
                            name: "price".into(),
                            props: Box::new(FieldProps { tooltip: Some("edited".into()), ..Default::default() }),
                        },
                    )
                    .unwrap();
            }
            let before = session.save_bytes(id).unwrap();
            let doc = session.get(id).unwrap();
            let state = (doc.dirty, doc.generation, doc.can_undo().map(str::to_owned), doc.can_redo().map(str::to_owned));
            let error = session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap_err();
            assert!(error.to_string().contains("reset event could not be restored"), "{initializer}: {error}");
            assert_eq!(session.save_bytes(id).unwrap().as_ref(), before.as_ref());
            let doc = session.get(id).unwrap();
            assert_eq!((doc.dirty, doc.generation, doc.can_undo().map(str::to_owned), doc.can_redo().map(str::to_owned)), state, "{initializer}");
            assert_eq!((value(&session, id, "price"), value(&session, id, "total")), (vec!["42".into()], vec!["84".into()]));
        }
    }
}

#[test]
fn reset_lexical_alias_preserves_calculate_format_and_save() {
    for all in [false, true] {
        let mut cos = pdfcraft_cos::Document::open(Arc::new(form(all, false))).unwrap();
        pdfcraft_forms::set_field_script(&mut cos, "total", "format", Some("event.value = String(event.value) + '!';")).unwrap();
        let (mut session, id) = open(with_document_script(
            pdfcraft_cos::write_full(&cos, &Default::default()).unwrap(),
            "const event = globalThis.event; var saved = event; function savedName() { return saved.name; }",
        ));
        session.apply(id, Edit::ResetForm { names: if all { None } else { Some(vec!["price".into()]) } }).unwrap();
        let saved = session.save_bytes(id).unwrap();
        assert_eq!(normal_appearance_text(saved.clone(), "total"), "34!");
        let (reopened, id) = open(saved.as_ref().clone());
        assert_eq!((value(&reopened, id, "price"), value(&reopened, id, "total")), (vec!["17".into()], vec!["34".into()]));
        assert_eq!(normal_appearance_text(reopened.save_bytes(id).unwrap(), "total"), "34!");
    }
}

struct CountScripts<'a> {
    inner: &'a mut dyn pdfcraft_forms::Scripts,
    events: Vec<(pdfcraft_forms::FieldEvent, String)>,
    english: Vec<String>,
}

impl pdfcraft_forms::Scripts for CountScripts<'_> {
    fn run(
        &mut self,
        event: pdfcraft_forms::FieldEvent,
        script: &str,
        target: &pdfcraft_forms::Field,
        value: &str,
        fields: &[pdfcraft_forms::Field],
    ) -> pdfcraft_forms::ScriptResult {
        self.events.push((event, target.name.clone()));
        self.inner.run(event, script, target, value, fields)
    }

    fn rerun_with_english_dates(
        &mut self,
        event: pdfcraft_forms::FieldEvent,
        script: &str,
        target: &pdfcraft_forms::Field,
        value: &str,
        fields: &[pdfcraft_forms::Field],
    ) -> Option<pdfcraft_forms::ScriptResult> {
        let result = self.inner.rerun_with_english_dates(event, script, target, value, fields);
        self.english.push(result.as_ref().map(|r| r.value.clone()).unwrap_or_default());
        result
    }
}

fn diagnostic_form(format: bool, multiple: bool, bootstrap: &str) -> Vec<u8> {
    let bytes = if multiple { form_with_dependent(&["total", "tax"]) } else { form(false, false) };
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    for (name, expression) in [("total", "Number(getField('price').value) * 2"), ("tax", "Number(getField('total').value) + 1")]
        .into_iter()
        .take(if multiple { 2 } else { 1 })
    {
        pdfcraft_forms::set_field_script(
            &mut doc,
            name,
            "calculate",
            Some(&format!("console.println('same'); app.alert('same'); event.value = {expression};")),
        )
        .unwrap();
        if format {
            pdfcraft_forms::set_field_script(
                &mut doc,
                name,
                "format",
                Some("console.println('same'); app.alert('same'); event.value = String(event.value) + '!';"),
            )
            .unwrap();
        }
    }
    pdfcraft_forms::set_document_script(&mut doc, "init", Some(bootstrap)).unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

fn counted_reset(doc: &mut pdfcraft_cos::Document, runner: &mut js::JsRunner) -> (Vec<(pdfcraft_forms::FieldEvent, String)>, Vec<String>) {
    let mut reset = runner.for_reset();
    let mut counted = CountScripts { inner: &mut reset, events: Vec::new(), english: Vec::new() };
    pdfcraft_forms::reset_with(doc, Some(&["price".into()]), &mut counted).unwrap();
    (counted.events, counted.english)
}

#[test]
fn reset_bootstrap_diagnostics_keep_real_duplicates_and_event_counts() {
    use pdfcraft_forms::FieldEvent::{Calculate, Format};
    for format in [false, true] {
        for multiple in [false, true] {
            let bytes = diagnostic_form(format, multiple, "console.println('same'); app.alert('same');");
            let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
            let mut runner = js::JsRunner::new(&doc, "diagnostics.pdf");
            let (events, english) = counted_reset(&mut doc, &mut runner);
            let calculated = if multiple { 2 } else { 1 };
            assert_eq!(events.iter().filter(|(e, _)| *e == Calculate).count(), calculated);
            assert_eq!(events.iter().filter(|(e, _)| *e == Format).count(), if format { calculated } else { 0 });
            assert!(english.is_empty());
            let expected = 1 + events.len();
            assert_eq!(runner.output.console, vec!["same"; expected], "format={format}, multiple={multiple}");
            assert_eq!(runner.output.alerts, vec![js::Message::plain("same"); expected]);
            assert!(runner.output.errors.is_empty());
            let saved = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
            assert_eq!(normal_appearance_text(saved.clone(), "total"), if format { "34!" } else { "34" });
            let (reopened, id) = open(saved.as_ref().clone());
            assert_eq!(value(&reopened, id, "total"), ["34"]);
            assert_eq!(normal_appearance_text(reopened.save_bytes(id).unwrap(), "total"), if format { "34!" } else { "34" });
        }
    }
}

#[test]
fn reset_bootstrap_diagnostics_keep_new_messages_and_occurrences() {
    for (bootstrap, multiple, expected) in [
        ("console.println('bootstrap:' + event.target.name); app.alert({cMsg:'same', cTitle:event.target.name});", true, 6),
        ("var n = event.name === 'Format' ? 3 : 2; for (var i=0; i<n; i++) { console.println('same'); app.alert('same'); }", false, 5),
    ] {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(diagnostic_form(true, multiple, bootstrap))).unwrap();
        let mut runner = js::JsRunner::new(&doc, "new-diagnostics.pdf");
        counted_reset(&mut doc, &mut runner);
        assert_eq!(runner.output.console.len(), expected, "{bootstrap}: {:?}", runner.output.console);
        assert_eq!(runner.output.alerts.len(), expected);
        if multiple {
            assert_eq!(runner.output.console, ["bootstrap:total", "same", "same", "bootstrap:tax", "same", "same"]);
            assert_eq!(runner.output.alerts.iter().filter_map(|a| a.title.as_deref()).collect::<Vec<_>>(), ["total", "tax"]);
        }
    }
}

#[test]
fn reset_bootstrap_diagnostics_do_not_leak_between_resets_or_into_ordinary_edits() {
    for ordinary in [false, true] {
        let (mut session, id) = open(diagnostic_form(true, false, "console.println('same'); app.alert('same');"));
        let _ = session.take_js_output(id);
        session
            .apply(
                id,
                Edit::Batch {
                    label: "Reset diagnostics".into(),
                    edits: vec![
                        Edit::ResetForm { names: Some(vec!["price".into()]) },
                        if ordinary {
                            Edit::SetFieldValue { name: "price".into(), value: FieldValue::Text("43".into()) }
                        } else {
                            Edit::ResetForm { names: Some(vec!["price".into()]) }
                        },
                    ],
                },
            )
            .unwrap();
        // Reset #1: one bootstrap, Calculate, Format. Reset #2: bootstrap and Calculate
        // (unchanged result); an ordinary edit keeps both of its bootstrap invocations.
        let expected = if ordinary { 7 } else { 5 };
        let output = session.take_js_output(id);
        assert_eq!(output.console, vec!["same"; expected]);
        assert_eq!(output.alerts, vec![js::Message::plain("same"); expected]);
        let expected_total = if ordinary { "86!" } else { "34!" };
        assert_eq!(normal_appearance_text(session.save_bytes(id).unwrap(), "total"), expected_total);
        let _ = session.take_js_output(id);
        session.apply(id, Edit::SetFieldValue { name: "price".into(), value: FieldValue::Text("42".into()) }).unwrap();
        let _ = session.take_js_output(id);
        session.apply(id, Edit::ResetForm { names: Some(vec!["price".into()]) }).unwrap();
        assert_eq!(session.take_js_output(id).console, ["same", "same", "same"], "a subsequent ResetForm has its own diagnostic scope");
    }
}

#[test]
fn reset_bootstrap_diagnostics_preserve_document_and_real_script_errors() {
    let bytes = diagnostic_form(true, true, "console.println('bootstrap'); throw new Error('shared error');");
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    pdfcraft_forms::set_calculation_order(&mut doc, &["tax".into(), "total".into()]).unwrap();
    pdfcraft_forms::set_field_script(&mut doc, "tax", "calculate", Some("console.println('same'); throw new Error('shared error');")).unwrap();
    pdfcraft_forms::set_field_script(&mut doc, "total", "format", Some("console.println('same'); throw new Error('shared error');")).unwrap();
    let mut runner = js::JsRunner::new(&doc, "errors.pdf");
    let (events, english) = counted_reset(&mut doc, &mut runner);
    assert_eq!(events.len(), 3);
    assert!(english.is_empty());
    assert_eq!(runner.output.console, ["bootstrap", "same", "same", "same"]);
    assert_eq!(runner.output.doc_errors.len(), 1);
    assert_eq!(runner.output.errors.len(), 3, "two real failures remain distinct from the document failure");
    assert!(runner.output.errors.iter().all(|e| e.contains("shared error")));
    assert_eq!(normal_appearance_text(Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()), "total"), "34");
}

#[test]
fn reset_bootstrap_diagnostics_preserve_the_actual_english_format_rerun() {
    let bytes = diagnostic_form(true, false, "console.println('bootstrap'); app.alert('bootstrap');");
    let mut doc = pdfcraft_cos::Document::open(Arc::new(bytes)).unwrap();
    // Literal characters unavailable to this fixture's Helvetica force an English retry,
    // without changing process-wide date names while the other tests run.
    pdfcraft_forms::set_field_script(
        &mut doc,
        "total",
        "format",
        Some("console.println('format'); app.alert('format'); event.value=util.printd('yyyy年mm月dd日', new Date(2026,9,18));"),
    )
    .unwrap();
    let mut runner = js::JsRunner::new(&doc, "english.pdf");
    let (events, english) = counted_reset(&mut doc, &mut runner);
    assert_eq!(events.len(), 2, "one Calculate and the first Format");
    assert_eq!(english, ["2026年10月18日"], "the unlogged English run still executes the real Format");
    assert_eq!(runner.output.console, ["bootstrap", "same", "format"]);
    assert_eq!(runner.output.alerts, [js::Message::plain("bootstrap"), js::Message::plain("same"), js::Message::plain("format")]);
    assert!(runner.output.errors.is_empty());
    assert_eq!(normal_appearance_text(Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()), "total"), "2026?10?18?");
}

#[test]
fn reset_bootstrap_diagnostics_do_not_hide_real_messages_after_console_clear() {
    let mut doc = pdfcraft_cos::Document::open(Arc::new(diagnostic_form(true, false, "console.println('same'); app.alert('same');"))).unwrap();
    pdfcraft_forms::set_field_script(
        &mut doc,
        "total",
        "format",
        Some("console.clear(); console.println('same'); app.alert('same'); event.value=String(event.value)+'!';"),
    )
    .unwrap();
    let mut runner = js::JsRunner::new(&doc, "clear.pdf");
    let (events, english) = counted_reset(&mut doc, &mut runner);
    assert_eq!(events.len(), 2);
    assert!(english.is_empty());
    assert_eq!(runner.output.console, ["same", "same", "same"]);
    assert_eq!(runner.output.alerts, [js::Message::plain("same"), js::Message::plain("same"), js::Message::plain("same")]);
    assert_eq!(normal_appearance_text(Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()), "total"), "34!");
}
