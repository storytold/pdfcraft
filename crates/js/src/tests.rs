use super::*;

fn fields() -> Vec<FieldState> {
    let mut agree = FieldState::new("agree", FieldType::CheckBox, vec![]);
    agree.options = vec![("Yes".into(), "Yes".into())];
    let mut size = FieldState::new("size", FieldType::ComboBox, vec!["m".into()]);
    size.options = vec![("s".into(), "Small".into()), ("m".into(), "Medium".into()), ("l".into(), "Large".into())];
    vec![
        FieldState::new("price", FieldType::Text, vec!["12.5".into()]),
        FieldState::new("qty", FieldType::Text, vec!["4".into()]),
        FieldState::new("total", FieldType::Text, vec![]),
        FieldState::new("zip", FieldType::Text, vec!["02134".into()]),
        agree,
        size,
    ]
}

fn doc() -> DocInfo {
    DocInfo { file_name: "order.pdf".into(), num_pages: 3, page: 0, info: vec![("title".into(), "Order".into())] }
}

fn go(script: &str, event: &Event) -> Outcome {
    run(script, event, &doc(), &fields(), &[], Limits::default())
}

#[test]
fn calculate_scripts_read_fields_and_set_the_value() {
    let o = go("event.value = this.getField('price').value * getField('qty').value;", &Event::field("Calculate", "total", ""));
    assert_eq!(o.error, None);
    assert_eq!(o.value, "50");
    assert!(o.rc);
}

#[test]
fn numbers_with_leading_zeros_stay_text() {
    let o = go("event.value = typeof getField('zip').value + ':' + typeof getField('qty').value;", &Event::field("Calculate", "total", ""));
    assert_eq!(o.value, "string:number");
}

#[test]
fn validate_scripts_reject_with_rc_and_alert() {
    let o = go("if (event.value > 10) { app.alert('Too many'); event.rc = false; }", &Event::field("Validate", "qty", "11"));
    assert!(!o.rc);
    assert_eq!(o.alerts, ["Too many"]);
    let o = go("if (event.value > 10) { event.rc = false; }", &Event::field("Validate", "qty", "3"));
    assert!(o.rc);
}

#[test]
fn keystroke_scripts_see_the_change() {
    let mut e = Event::field("Keystroke", "qty", "1");
    e.change = "x".into();
    e.will_commit = false;
    let o = go("event.rc = /^[0-9]*$/.test(event.change);", &e);
    assert!(!o.rc);
}

#[test]
fn scripts_change_other_fields_and_their_properties() {
    let o = go(
        "var t = getField('total'); t.value = 99; t.readonly = true; t.display = display.hidden; \
         getField('agree').checkThisBox(0, true); getField('size').value = 'l'; t.textColor = color.red;",
        &Event::field("Mouse Up", "agree", ""),
    );
    assert_eq!(o.error, None);
    let total = o.changed.iter().find(|f| f.name == "total").unwrap();
    assert_eq!(total.value, ["99"]);
    assert!(total.readonly);
    assert_eq!(total.display, DISPLAY_HIDDEN);
    assert_eq!(total.text_color.as_deref(), Some(&["RGB".to_string(), "1".into(), "0".into(), "0".into()][..]));
    assert_eq!(o.changed.iter().find(|f| f.name == "agree").unwrap().value, ["Yes"]);
    assert_eq!(o.changed.iter().find(|f| f.name == "size").unwrap().value, ["l"]);
}

#[test]
fn choice_and_check_box_object_model() {
    let o = go(
        "var s = getField('size'); event.value = [s.numItems, s.getItemAt(2, false), s.currentValueIndices, s.valueAsString, \
         getField('agree').value, getField('agree').isBoxChecked(0), s.type].join('|');",
        &Event::field("Calculate", "total", ""),
    );
    assert_eq!(o.value, "3|Large|1|m|Off|false|combobox");
}

#[test]
fn util_printf_printd_and_printx() {
    let o = go(
        "event.value = [util.printf('%,0.2f', 1234567.891), util.printf('%05d|%s|%x', 42, 'hi', 255), util.printf('%,2.2f', 1234.5), \
         util.printd('mmm d, yyyy HH:MM', new Date(2024, 0, 5, 9, 7)), util.printd('dddd', new Date(2024, 0, 5)), \
         util.printx('(999) 999-9999', '5551234567'), util.printx('>AAA', 'abc')].join('|');",
        &Event::field("Calculate", "total", ""),
    );
    assert_eq!(o.error, None);
    assert_eq!(o.value, "1,234,567.89|00042|hi|FF|1.234,50|Jan 5, 2024 09:07|Friday|(555) 123-4567|ABC");
}

#[test]
fn document_requests_and_console() {
    let o = go(
        "console.println('hello ' + this.documentFileName + ' ' + numPages + ' ' + info.title); this.pageNum = 2; \
         this.resetForm(['qty']); this.print(); app.launchURL('https://example.org'); this.submitForm({cURL: 'https://example.org/f'});",
        &Event::field("Mouse Up", "agree", ""),
    );
    assert_eq!(o.error, None);
    assert_eq!(o.console, ["hello order.pdf 3 Order"]);
    assert_eq!(
        o.requests,
        [
            Request::GoToPage(2),
            Request::Reset(vec!["qty".into()]),
            Request::Print,
            Request::LaunchUrl("https://example.org".into()),
            Request::Submit("https://example.org/f".into())
        ]
    );
}

#[test]
fn document_level_functions_are_available() {
    let o = run(
        "event.value = double(getField('qty').value);",
        &Event::field("Calculate", "total", ""),
        &doc(),
        &fields(),
        &["function double(x) { return x * 2; }".into()],
        Limits::default(),
    );
    assert_eq!(o.value, "8");
}

#[test]
fn errors_and_runaway_scripts_are_contained() {
    let o = go("event.value = nosuch.thing;", &Event::field("Calculate", "total", "keep"));
    assert!(o.error.is_some());
    let o = run("while (true) {}", &Event::doc("Open"), &doc(), &fields(), &[], Limits { loop_iterations: 10_000, recursion: 64 });
    assert!(o.error.is_some(), "the loop limit stops it");
    let o = go("function f() { return f(); } f();", &Event::doc("Open"));
    assert!(o.error.is_some(), "the recursion limit stops it");
    let o = go("getField('nope').value", &Event::doc("Open"));
    assert!(o.error.as_deref().unwrap_or("").contains("null") || o.error.is_some());
}

#[test]
fn the_sandbox_has_no_host_access() {
    let o = go(
        "event.value = [typeof require, typeof process, typeof fetch, typeof XMLHttpRequest, typeof importScripts].join(',');",
        &Event::doc("Open"),
    );
    assert_eq!(o.value, "undefined,undefined,undefined,undefined,undefined");
}

/// Hostile nesting used to overflow the stack in boa's parser and compiler, aborting the app
/// (about 100 nested parentheses were enough on a 2 MiB stack).
#[test]
fn deeply_nested_scripts_fail_instead_of_crashing() {
    let n = 100_000;
    let hostile = [
        format!("event.value = {}1{};", "(".repeat(n), ")".repeat(n)),
        format!("var a = {}1{};", "[".repeat(n), "]".repeat(n)),
        format!("{}{}", "{".repeat(n), "}".repeat(n)),
        format!("var a = {}1;", "!".repeat(n)),
        format!("var a = {}1;", "- ".repeat(n)),
        format!("var a = {}1;", "1?1:".repeat(n)),
        format!("var f = {}1;", "a=>".repeat(n)),
    ];
    for s in &hostile {
        let o = go(s, &Event::doc("Open"));
        assert!(o.error.is_some(), "{}…", &s[..20]);
    }
    // A hostile document-level script is refused too, before the field script runs.
    let o = run("event.value = 'ran';", &Event::field("Calculate", "a", ""), &doc(), &fields(), &[hostile[0].clone()], Limits::default());
    assert!(o.error.is_some());
}

#[test]
fn ordinary_nesting_still_runs() {
    let n = 40;
    let s = format!("event.value = {}1{} + [[[2]]][0][0][0] + (1 ? 2 : 3);", "(".repeat(n), ")".repeat(n));
    let o = go(&s, &Event::field("Calculate", "a", ""));
    assert_eq!(o.error, None);
    assert_eq!(o.value, "5");
    // Brackets in strings and comments don't count.
    let s = format!("// {}\nevent.value = '{}'.length;", "(".repeat(1000), "[".repeat(1000));
    assert_eq!(go(&s, &Event::field("Calculate", "a", "")).value, "1000");
}

mod xfa_model {
    use crate::Limits;
    use crate::xfa::*;

    fn field(name: &str, som: &str, value: &str, numeric: bool) -> XfaNode {
        XfaNode {
            name: name.into(),
            som: som.into(),
            kind: Some(XfaKind::Field),
            value: value.into(),
            numeric,
            presence: "visible".into(),
            ..Default::default()
        }
    }

    /// form1 > page1 > { qty, price, total, details (subform, hidden) > note, table > row[0..2] > { what, amount }, go (button) }
    fn form() -> XfaNode {
        let row = |i: usize| XfaNode {
            name: "row".into(),
            som: format!("form1[0].page1[0].table[0].row[{i}]"),
            kind: Some(XfaKind::Subform),
            repeatable: true,
            occur_min: 1,
            occur_max: None,
            index: i,
            children: vec![
                field("what", &format!("form1[0].page1[0].table[0].row[{i}].what[0]"), if i == 0 { "Rent" } else { "" }, false),
                field("amount", &format!("form1[0].page1[0].table[0].row[{i}].amount[0]"), if i == 0 { "10" } else { "5" }, true),
            ],
            ..Default::default()
        };
        XfaNode {
            name: "form1".into(),
            som: "form1[0]".into(),
            kind: Some(XfaKind::Subform),
            children: vec![XfaNode {
                name: "page1".into(),
                som: "form1[0].page1[0]".into(),
                kind: Some(XfaKind::Subform),
                children: vec![
                    field("qty", "form1[0].page1[0].qty[0]", "4", true),
                    field("price", "form1[0].page1[0].price[0]", "2.5", true),
                    field("total", "form1[0].page1[0].total[0]", "", true),
                    XfaNode {
                        name: "details".into(),
                        som: "form1[0].page1[0].details[0]".into(),
                        kind: Some(XfaKind::Subform),
                        presence: "hidden".into(),
                        children: vec![field("note", "form1[0].page1[0].details[0].note[0]", "", false)],
                        ..Default::default()
                    },
                    XfaNode {
                        name: "table".into(),
                        som: "form1[0].page1[0].table[0]".into(),
                        kind: Some(XfaKind::Subform),
                        children: vec![row(0), row(1)],
                        ..Default::default()
                    },
                    field("go", "form1[0].page1[0].go[0]", "", false),
                ],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn go(script: &str, target: &str, activity: &str) -> XfaOutcome {
        let ev = XfaEvent { activity: activity.into(), target: target.into(), ..Default::default() };
        let doc = XfaDoc { file_name: "f.pdf".into(), page: 1, page_count: 3 };
        run_xfa(script, &ev, &doc, &form(), Limits::default())
    }

    #[test]
    fn calculate_scripts_see_siblings_by_name_and_yield_their_last_expression() {
        let o = go("qty.rawValue * price.rawValue", "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("10"));
        let o = go("this.rawValue = xfa.form.form1.page1.qty.rawValue + 1;", "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.effects, vec![XfaEffect::SetValue { som: "form1[0].page1[0].total[0]".into(), value: "5".into() }]);
    }

    #[test]
    fn values_read_as_numbers_or_strings_and_null_when_empty() {
        let o = go(
            "typeof qty.rawValue + ':' + typeof this.rawValue + ':' + (total.rawValue === null) + ':' + typeof table.row.what.rawValue",
            "form1[0].page1[0].price[0]",
            "click",
        );
        assert_eq!(o.result.as_deref(), Some("number:number:true:string"));
    }

    #[test]
    fn navigation_parent_resolve_node_and_nodes() {
        let o = go(
            "this.parent.name + '/' + this.parent.parent.className + '/' + this.resolveNode('qty').rawValue + '/' + xfa.resolveNode('$form.form1.page1.table.row[1].amount').rawValue + '/' + this.parent.nodes.length + '/' + xfa.resolveNodes('form1.page1.table.row[*]').length + '/' + xfa.resolveNode('$form..amount').somExpression",
            "form1[0].page1[0].price[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("page1/subform/4/5/6/2/form1[0].page1[0].table[0].row[0].amount[0]"));
    }

    #[test]
    fn presence_and_access_changes_are_effects() {
        let o = go("details.presence = 'visible'; details.note.access = 'readOnly'; this.presence = 'nonsense';", "form1[0].page1[0].go[0]", "click");
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(
            o.effects,
            vec![
                XfaEffect::SetPresence { som: "form1[0].page1[0].details[0]".into(), presence: "visible".into() },
                XfaEffect::SetAccess { som: "form1[0].page1[0].details[0].note[0]".into(), access: "readOnly".into() },
            ]
        );
    }

    #[test]
    fn instance_managers_add_remove_and_count() {
        let o = go(
            "var n = table._row.count; var r = table._row.addInstance(1); r.what.rawValue = 'New'; n + '/' + table._row.count + '/' + r.somExpression + '/' + r.index",
            "form1[0].page1[0].go[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("2/3/form1[0].page1[0].table[0].row[2]/2"));
        assert_eq!(
            o.effects,
            vec![
                XfaEffect::AddInstance { som: "form1[0].page1[0].table[0].row[2]".into() },
                XfaEffect::SetValue { som: "form1[0].page1[0].table[0].row[2].what[0]".into(), value: "New".into() },
            ]
        );
        // Through the instance manager property, from inside a row; removing renumbers.
        let o = go(
            "this.parent.instanceManager.removeInstance(0); xfa.resolveNode('table.row[0]').amount.rawValue",
            "form1[0].page1[0].table[0].row[1].amount[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("5"), "the second row is now the first");
        assert_eq!(o.effects, vec![XfaEffect::RemoveInstance { som: "form1[0].page1[0].table[0].row[0]".into() }]);
        // setInstances, and the floor of one instance.
        let o = go(
            "table._row.setInstances(4); var a = table._row.count; table._row.setInstances(0); a + '/' + table._row.count",
            "form1[0].page1[0].go[0]",
            "click",
        );
        assert_eq!(o.result.as_deref(), Some("4/1"));
    }

    #[test]
    fn host_layout_event_and_app_calls_become_effects() {
        let o = go(
            "xfa.host.messageBox('Hi', 'T', 3, 1); xfa.host.resetData('qty, price'); xfa.host.print(1, '0', '2', 0, 0, 0, 0, 0); app.execMenuItem('SaveAs'); app.launchURL('https://x.test'); xfa.host.setFocus('qty'); xfa.form.recalculate(1); console.println('p' + xfa.layout.page(this) + '/' + xfa.layout.pageCount() + '/' + xfa.host.currentPage + '/' + xfa.event.name); this.border.fill.color.value = '255,0,0'; this.fillColor = 'x'; util.printf('%d', 7)",
            "form1[0].page1[0].go[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(
            o.effects,
            vec![
                XfaEffect::MessageBox("Hi".into()),
                XfaEffect::ResetData(vec!["qty".into(), "price".into()]),
                XfaEffect::Print,
                XfaEffect::SaveAs,
                XfaEffect::LaunchUrl("https://x.test".into()),
                XfaEffect::SetFocus("form1[0].page1[0].qty[0]".into()),
                XfaEffect::Recalculate,
            ]
        );
        assert_eq!(o.console, vec!["p2/3/1/click".to_string()]);
        assert_eq!(o.result.as_deref(), Some("7"));
    }

    #[test]
    fn field_names_never_shadow_javascript_builtins() {
        let mut f = form();
        f.children[0].children.push(field("Date", "form1[0].page1[0].Date[0]", "x", false));
        f.children[0].children.push(field("eval", "form1[0].page1[0].eval[0]", "y", false));
        let ev = XfaEvent { activity: "click".into(), target: "form1[0].page1[0].go[0]".into(), ..Default::default() };
        let o = run_xfa(
            "typeof new Date().getTime() + ':' + this.parent.Date.rawValue + ':' + this.parent.eval.rawValue",
            &ev,
            &XfaDoc::default(),
            &f,
            Limits::default(),
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("number:x:y"));
    }

    #[test]
    fn validate_scripts_give_a_verdict_and_errors_are_reported() {
        let o = go("this.rawValue === null || this.rawValue <= 3", "form1[0].page1[0].qty[0]", "validate");
        assert_eq!(o.result_bool, Some(false));
        let o = go("nosuch.rawValue = 1", "form1[0].page1[0].qty[0]", "click");
        assert!(o.error.is_some() && o.effects.is_empty());
        let o = go("while (true) {}", "form1[0].page1[0].qty[0]", "click");
        assert!(o.error.is_some(), "the loop limit stops it");
        let o = go(&"(".repeat(200), "form1[0].page1[0].qty[0]", "click");
        assert!(o.error.is_some());
        // Adding instances is capped; a runaway loop can't blow the tree up.
        let o = go("for (var i = 0; i < 5000; i++) table._row.addInstance(1); table._row.count", "form1[0].page1[0].go[0]", "click");
        assert_eq!(o.result.as_deref(), Some("1002"));
    }

    #[test]
    fn values_set_again_merge_and_hostile_loops_are_capped() {
        // A calculate that sets its own value in a loop leaves one effect: the last value.
        let o = go("for (var i = 0; i < 50000; i++) this.rawValue = i;", "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.effects, vec![XfaEffect::SetValue { som: "form1[0].page1[0].total[0]".into(), value: "49999".into() }], "{:?}", o.error);
        // Order-dependent effects in between keep the values on either side.
        let o = go("qty.rawValue = 1; xfa.host.resetData(); qty.rawValue = 2; qty.rawValue = 3;", "form1[0].page1[0]", "click");
        assert_eq!(o.effects.len(), 3, "{:?}", o.effects);
        assert_eq!(o.effects.last(), Some(&XfaEffect::SetValue { som: "form1[0].page1[0].qty[0]".into(), value: "3".into() }));
        // Message boxes and console lines in a loop stop at their caps, and say so.
        let o = go("for (var i = 0; i < 100000; i++) { xfa.host.messageBox('m' + i); console.println('c' + i); }", "form1[0].page1[0]", "initialize");
        let alerts = o.effects.iter().filter(|e| matches!(e, XfaEffect::MessageBox(_))).count();
        assert_eq!(alerts, MAX_ALERTS);
        assert_eq!(o.console.len(), MAX_CONSOLE);
        assert!(o.notes.iter().any(|n| n.contains("messages")) && o.notes.iter().any(|n| n.contains("console")), "{:?}", o.notes);
        // Distinct effects stop at the effect cap.
        let o = go("for (var i = 0; i < 30000; i++) xfa.host.beep();", "form1[0].page1[0]", "click");
        assert_eq!(o.effects.len(), MAX_EFFECTS);
        assert!(o.notes.iter().any(|n| n.contains("changes")), "{:?}", o.notes);
        // Long messages are cut.
        let o = go("xfa.host.messageBox(new Array(100000).join('x'));", "form1[0].page1[0]", "click");
        assert!(matches!(&o.effects[0], XfaEffect::MessageBox(m) if m.chars().count() <= 4_097));
    }
}
