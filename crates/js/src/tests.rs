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
    DocInfo { file_name: "order.pdf".into(), num_pages: 3, page: 0, info: vec![("title".into(), "Order".into())], xfa: false }
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
fn printd_escapes_multibyte_chars() {
    let o = go(
        "event.value = [util.printd('yyyy \\\\é', new Date(2024, 0, 5)), util.printd('\\\\€ yyyy', new Date(2024, 0, 5)), \
         util.printd('\\\\y yyyy', new Date(2024, 0, 5))].join('|');",
        &Event::field("Calculate", "total", ""),
    );
    assert_eq!(o.error, None);
    assert_eq!(o.value, "2024 é|€ 2024|y 2024");
}

fn assert_printf_error(script: &str, message: &str) {
    let o = go(script, &Event::doc("Open"));
    assert!(o.error.as_deref().is_some_and(|e| e.starts_with(message)), "{script}: {:?}", o.error);
    let o = xfa::run_xfa(
        script,
        &xfa::XfaEvent { activity: "initialize".into(), target: "form1[0]".into(), ..Default::default() },
        &xfa::XfaDoc::default(),
        &xfa_model::form(),
        Limits::default(),
    );
    assert!(o.error.as_deref().is_some_and(|e| e.starts_with(message)), "XFA: {script}: {:?}", o.error);
}

#[test]
fn util_printf_rejects_large_width() {
    assert_printf_error("util.printf('%99999999999d', 1)", "TypeError: util.printf width or precision exceeds 4096");
}

#[test]
fn util_printf_rejects_large_precision() {
    assert_printf_error("util.printf('%.99999999999f', 1)", "TypeError: util.printf width or precision exceeds 4096");
}

#[test]
fn util_printf_rejects_overflowing_width() {
    assert_printf_error("util.printf('%9999999999999999999999999999999999999999d', 1)", "TypeError: util.printf width or precision exceeds 4096");
}

#[test]
fn util_printf_rejects_overflowing_precision() {
    assert_printf_error("util.printf('%.9999999999999999999999999999999999999999f', 1)", "TypeError: util.printf width or precision exceeds 4096");
}

#[test]
fn util_printf_rejects_limits_for_all_conversions() {
    for conv in ['d', 'f', 's', 'x'] {
        for spec in ["4097", ".4097", "99999999999", ".99999999999"] {
            assert_printf_error(&format!("util.printf('%{spec}{conv}', 1)"), "TypeError: util.printf width or precision exceeds 4096");
        }
    }
}

#[test]
fn util_printf_bounds_total_output() {
    for script in [
        "util.printf('%4096d'.repeat(300), 1)",
        "util.printf('%04096d'.repeat(300), -1)",
        "util.printf('aa'.repeat(524288) + 'a')",
        "util.printf('é'.repeat(524289))",
        "util.printf('%%%%'.repeat(524288) + '%%')",
        "util.printf('%s', 'aa'.repeat(524288) + 'a')",
        "util.printf('%s%s', 'aa'.repeat(524288), 'b')",
    ] {
        assert_printf_error(script, "TypeError: util.printf output exceeds 1048576 bytes");
    }
}

#[test]
fn util_printf_accepts_limits_and_preserves_formats() {
    let o = go(
        "event.value = [util.printf('%4096d', 1).length, util.printf('%04096d', -1).length, \
         util.printf('%.4096f', 1).length, util.printf('%.4096s', 'a'.repeat(4097)).length, \
         util.printf('%4096x', 255).length, util.printf('%.4096x', 255), \
         util.printf('%4096d'.repeat(255) + '%4095d', 1).length, util.printf('%4096d'.repeat(256), 1).length, \
         util.printf('é'.repeat(524288)).length, util.printf('a%%b:%+06d|%4s|%.2s|%x', 42, 'é', 'é界a', 255)].join('|');",
        &Event::field("Calculate", "total", ""),
    );
    assert_eq!(o.error, None);
    assert_eq!(o.value, "4096|4096|4098|4096|4096|FF|1048575|1048576|524288|a%b:+00042|   é|é界|FF");
}

fn assert_array_error(script: &str) {
    let o = go(script, &Event::field("Calculate", "total", ""));
    assert!(o.error.as_deref().is_some_and(|e| e.starts_with("TypeError: array length exceeds 1048576")), "{script}: {:?}", o.error);
    assert!(o.requests.is_empty() && o.changed.is_empty(), "{script}");
}

#[test]
fn reset_form_and_field_setters_refuse_huge_arrays() {
    for script in [
        "this.resetForm(new Array(4294967295))",
        "getField('total').value = new Array(4294967295)",
        "event.target.value = new Array(4294967295)",
        "getField('total').textColor = new Array(4294967295)",
        "getField('total').fillColor = new Array(4294967295)",
    ] {
        assert_array_error(script);
    }
}

#[test]
fn reset_form_and_field_setters_take_arrays_up_to_the_limit() {
    let o = go("this.resetForm(new Array(1 << 20)); getField('total').value = new Array(1 << 20);", &Event::field("Calculate", "total", ""));
    assert_eq!(o.error, None);
    assert!(matches!(&o.requests[..], [Request::Reset(names)] if names.len() == 1 << 20 && names.iter().all(String::is_empty)));
    assert_eq!(o.changed.iter().find(|f| f.name == "total").unwrap().value, [""]);
    assert_array_error("this.resetForm(new Array((1 << 20) + 1))");
    assert_array_error("getField('total').value = new Array((1 << 20) + 1)");
}

#[test]
fn reset_form_and_field_setters_still_read_short_arrays() {
    let mut f = fields();
    f.push(FieldState::new("pick", FieldType::ListBox, vec![]));
    let o = run(
        "this.resetForm(['a', 'b']); getField('pick').value = ['x', 'y', 'z']; getField('total').value = ['x', 'y', 'z'];",
        &Event::field("Calculate", "total", ""),
        &doc(),
        &f,
        &[],
        Limits::default(),
    );
    assert_eq!(o.error, None);
    assert_eq!(o.requests, [Request::Reset(vec!["a".into(), "b".into()])]);
    assert_eq!(o.changed.iter().find(|f| f.name == "pick").unwrap().value, ["x", "y", "z"]);
    assert_eq!(o.changed.iter().find(|f| f.name == "total").unwrap().value, ["x"]);
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

/// Bytes of text a run handed back in its messages, console lines and requests.
fn kept_bytes(o: &Outcome) -> usize {
    let req = |r: &Request| match r {
        Request::Reset(names) => names.iter().map(String::len).sum(),
        Request::Submit(u) | Request::LaunchUrl(u) | Request::Focus(u) => u.len(),
        _ => 0,
    };
    o.console.iter().chain(&o.alerts).map(String::len).sum::<usize>() + o.requests.iter().map(req).sum::<usize>()
}

#[test]
fn exec_menu_item_requests_are_capped_like_other_requests() {
    let o = go("for (var i = 0; i < 12000; i++) app.execMenuItem('Print');", &Event::field("Mouse Up", "agree", ""));
    assert_eq!(o.error, None);
    assert_eq!(o.requests.len(), crate::xfa::MAX_EFFECTS);
}

#[test]
fn console_alerts_and_requests_are_capped_in_count_and_bytes() {
    let o = go(
        "var s = 'x'; while (s.length < 8192) s += s; \
         for (var i = 0; i < 1500; i++) { console.println(s); app.alert(s); } \
         var a = []; for (var i = 0; i < 2000; i++) a.push(s); \
         for (var i = 0; i < 5; i++) this.resetForm(a); \
         for (var i = 0; i < 12000; i++) app.launchURL('u');",
        &Event::field("Mouse Up", "agree", ""),
    );
    assert_eq!(o.error, None);
    assert!(o.console.len() <= crate::xfa::MAX_CONSOLE + 1, "{} console lines", o.console.len());
    assert!(o.alerts.len() <= crate::xfa::MAX_ALERTS, "{} alerts", o.alerts.len());
    assert!(o.requests.len() <= crate::xfa::MAX_EFFECTS, "{} requests", o.requests.len());
    assert!(o.console.iter().chain(&o.alerts).all(|m| m.chars().count() <= 4_097));
    assert!(kept_bytes(&o) <= crate::xfa::MAX_PAYLOAD, "{} bytes kept", kept_bytes(&o));
    assert!(o.console.last().is_some_and(|l| l.contains("left out")), "{:?}", o.console.last());
}

#[test]
fn printd_reads_a_long_literal_run_in_linear_work() {
    crate::PRINTD_SCANNED.store(0, std::sync::atomic::Ordering::Relaxed);
    let o = go("event.value = util.printd(new Array(20001).join('x'), new Date(2024, 0, 5)).length;", &Event::field("Calculate", "total", ""));
    assert_eq!(o.error, None);
    assert_eq!(o.value, "20000");
    // Other tests format short dates at the same time; they add a few dozen bytes at most.
    let scanned = crate::PRINTD_SCANNED.load(std::sync::atomic::Ordering::Relaxed);
    assert!(scanned < 40_000, "printd scanned {scanned} bytes for a 20000-byte format");
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

/// #3940: a document script that throws after defining a helper (here calling an Acrobat method
/// PdfCraft lacks) still leaves the helper, and the later scripts, to the field script.
#[test]
fn a_failing_document_script_does_not_stop_the_field_script() {
    let docs = [
        "function double(x) { return x * 2; }\nthis.getField('qty').buttonSetIcon(null);\nvar after = 1;".to_string(),
        "function plus(x) { return x + 1; }".to_string(),
        "this is not javascript".to_string(),
    ];
    let o = run(
        "event.value = plus(double(getField('qty').value));",
        &Event::field("Calculate", "total", ""),
        &doc(),
        &fields(),
        &docs,
        Limits::default(),
    );
    assert_eq!((o.error.as_deref(), o.value.as_str()), (None, "9"));
    assert_eq!(o.doc_errors.len(), 2, "{:?}", o.doc_errors);
    assert!(o.doc_errors[0].starts_with("document script 1: TypeError"), "{:?}", o.doc_errors);
    assert!(o.doc_errors[1].starts_with("document script 3: SyntaxError"), "{:?}", o.doc_errors);
    // The failing script stopped where it failed.
    let o = run("event.value = typeof after;", &Event::field("Calculate", "total", ""), &doc(), &fields(), &docs, Limits::default());
    assert_eq!(o.value, "undefined");
    // Working document scripts report nothing.
    assert!(go("event.value = 1;", &Event::field("Calculate", "total", "")).doc_errors.is_empty());
}

/// A document script refused before running (too long, or nested too deeply to run safely) is
/// skipped like one that throws: never run, reported in its place, the rest still run.
#[test]
fn a_document_script_too_big_to_run_is_skipped_and_reported() {
    let docs = [
        "this.getField('qty').buttonSetIcon(null);".to_string(),
        format!("var big = 1{};", "+1".repeat(MAX_SCRIPT_BYTES / 2)),
        "function double(x) { return x * 2; }".to_string(),
        format!("var deep = {}1{};", "(".repeat(10_000), ")".repeat(10_000)),
    ];
    let o = run(
        "event.value = double(getField('qty').value) + typeof big;",
        &Event::field("Calculate", "total", ""),
        &doc(),
        &fields(),
        &docs,
        Limits::default(),
    );
    assert_eq!((o.error.as_deref(), o.value.as_str()), (None, "8undefined"));
    let places: Vec<&str> = o.doc_errors.iter().map(|e| e.split(':').next().unwrap_or_default()).collect();
    assert_eq!(places, ["document script 1", "document script 2", "document script 4"], "{:?}", o.doc_errors);
    assert!(o.doc_errors[1].contains("too long") && o.doc_errors[2].contains("nested too deeply"), "{:?}", o.doc_errors);
    // The field script's own limits still refuse it outright.
    let o = run(&format!("1{}", "+1".repeat(MAX_SCRIPT_BYTES)), &Event::field("Calculate", "total", ""), &doc(), &fields(), &docs, Limits::default());
    assert!(o.error.is_some_and(|e| e.contains("too long")) && o.doc_errors.is_empty());
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
    // A hostile document-level script is refused too: it never runs, and is reported, while
    // the field script still does (#3940).
    let o = run("event.value = 'ran';", &Event::field("Calculate", "a", ""), &doc(), &fields(), &[hostile[0].clone()], Limits::default());
    assert_eq!((o.error, o.value.as_str()), (None, "ran"));
    assert!(o.doc_errors.len() == 1 && o.doc_errors[0].contains("nested too deeply"), "{:?}", o.doc_errors);
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

/// Code built from strings at run time would reach the parser without the size and nesting
/// checks the script itself passed, so `eval` and the `Function` constructors refuse it.
#[test]
fn scripts_cannot_compile_code_from_strings() {
    for s in [
        "event.value = eval('1 + 1');",
        "event.value = (0, eval)('1 + 1');",
        "event.value = new Function('return 2')();",
        "event.value = Function('a', 'return a')(2);",
        "event.value = (function* () {}).constructor('yield 2')().next().value;",
        "event.value = new Array(1001).join('(') + '1' + new Array(1001).join(')'); event.value = eval(event.value);",
    ] {
        let o = go(s, &Event::field("Calculate", "a", ""));
        assert!(o.error.as_deref().is_some_and(|e| e.contains("compile code from strings")), "{s}: {o:?}");
    }
}

mod xfa_model {
    use crate::Limits;
    use crate::xfa::*;

    pub(super) fn field(name: &str, som: &str, value: &str, numeric: bool) -> XfaNode {
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
    pub(super) fn form() -> XfaNode {
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

    /// Run the JavaScript `script` for `activity` on the test form, from the node at `target`.
    fn go(script: &str, target: &str, activity: &str) -> XfaOutcome {
        let ev = XfaEvent { activity: activity.into(), target: target.into(), ..Default::default() };
        let doc = XfaDoc { file_name: "f.pdf".into(), page: 1, page_count: 3 };
        run_xfa(script, &ev, &doc, &form(), Limits::default())
    }

    /// A calculate script finds its siblings by name and yields its last expression, and a
    /// value it sets comes back as an effect.
    #[test]
    fn calculate_scripts_see_siblings_by_name_and_yield_their_last_expression() {
        let o = go("qty.rawValue * price.rawValue", "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("10"));
        let o = go("this.rawValue = xfa.form.form1.page1.qty.rawValue + 1;", "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.effects, vec![XfaEffect::SetValue { som: "form1[0].page1[0].total[0]".into(), value: "5".into() }]);
    }

    /// A long value written again and again stops at the run's byte budget, and the write that
    /// does not fit is left out of the field the script reads as well as of the effect.
    #[test]
    fn a_value_past_the_byte_budget_is_left_out_of_reads_and_effects_alike() {
        let o = go(
            "var s = 'x'; while (s.length < 65536) s += s; \
             for (var i = 0; i < 300; i++) details.note.rawValue = ('00000' + i).slice(-6) + s.slice(6); \
             details.note.rawValue.slice(0, 6)",
            "form1[0].page1[0].total[0]",
            "calculate",
        );
        assert_eq!(o.error, None, "{o:?}");
        let set: Vec<&str> = o
            .effects
            .iter()
            .filter_map(|e| if let XfaEffect::SetValue { value, .. } = e { Some(value.get(..6).unwrap_or(value)) } else { None })
            .collect();
        assert_eq!(set.len(), 1);
        assert_eq!(o.result.as_deref(), set.first().copied());
    }

    /// The script runs through `eval`, so code built from strings at run time is held to the
    /// script's own size and nesting checks instead of being refused outright.
    #[test]
    fn code_built_from_strings_is_checked_like_the_script() {
        let total = "form1[0].page1[0].total[0]";
        assert_eq!(go("eval('qty.rawValue * 2')", total, "calculate").result.as_deref(), Some("8"));
        assert_eq!(go("new Function('return 3')()", total, "calculate").result.as_deref(), Some("3"));
        for s in [
            "eval(new Array(1001).join('(') + '1' + new Array(1001).join(')'))",
            "new Function(new Array(1001).join('[') + new Array(1001).join(']'))",
        ] {
            let o = go(s, total, "calculate");
            assert!(o.error.as_deref().is_some_and(|e| e.contains("nested too deeply")), "{s}: {o:?}");
        }
    }

    /// A form whose one field `d` holds the ISO date 2026-10-20, shown as 20261020.
    pub(super) fn dated_form() -> XfaNode {
        let d = XfaNode { formatted: Some("20261020".into()), ..field("d", "form1[0].page1[0].d[0]", "2026-10-20", false) };
        XfaNode {
            name: "form1".into(),
            som: "form1[0]".into(),
            kind: Some(XfaKind::Subform),
            children: vec![XfaNode {
                name: "page1".into(),
                som: "form1[0].page1[0]".into(),
                kind: Some(XfaKind::Subform),
                children: vec![d],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// `formattedValue` reads the value as the field's picture shows it, and `rawValue` the
    /// value itself; once the script sets the value, `formattedValue` gives that value.
    #[test]
    fn formatted_value_reads_the_shown_value_until_the_script_sets_one() {
        let ev = XfaEvent { activity: "calculate".into(), target: "form1[0].page1[0].d[0]".into(), ..Default::default() };
        let doc = XfaDoc::default();
        let run = |script: &str| run_xfa(script, &ev, &doc, &dated_form(), Limits::default());
        let o = run("this.formattedValue + ' ' + this.rawValue");
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("20261020 2026-10-20"));
        let o = run("this.rawValue = '2026-10-21'; this.formattedValue");
        assert_eq!(o.result.as_deref(), Some("2026-10-21"));
    }

    /// Long values a script sets, and long results, are cut where they are stored, with a note.
    #[test]
    fn long_values_and_results_are_cut_where_they_are_stored() {
        // The value kept for later reads is the one the effect saves.
        let o = go("details.note.rawValue = new Array(100001).join('x'); details.note.rawValue.length", "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("65536"));
        assert!(o.notes.iter().any(|n| n.contains("cut")), "{:?}", o.notes);
        // A calculate's result is cut like a value set, and noted.
        let o = go("new Array(100001).join('x')", "form1[0].page1[0].details[0].note[0]", "calculate");
        assert_eq!(o.result.map(|r| r.chars().count()), Some(65_536));
        assert!(o.notes.iter().any(|n| n.contains("result")), "{:?}", o.notes);
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

    /// Past the byte budget, adding or removing a row is left out of the snapshot as well as the
    /// effects, so the script never counts rows the form will not get.
    #[test]
    fn rows_changed_past_the_byte_budget_are_left_out_of_the_snapshot_too() {
        let o = go(
            "var s = 'x'; while (s.length < 4096) s += s; for (var i = 0; i < 9000; i++) xfa.host.gotoURL(s); \
             table._row.removeInstance(0); var added = table._row.addInstance(1); (added === null) + ':' + table._row.count",
            "form1[0].page1[0].go[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert!(o.notes.iter().any(|n| n.contains("MiB")), "{:?}", o.notes);
        assert!(!o.effects.iter().any(|e| matches!(e, XfaEffect::AddInstance { .. } | XfaEffect::RemoveInstance { .. })));
        assert_eq!(o.result.as_deref(), Some("true:2"));
    }

    /// Past the effect cap, presence and access set on a new object are left out of the snapshot
    /// as well as the effects; one set again on an object that already has its effect still lands.
    #[test]
    fn presence_and_access_past_the_effect_cap_are_left_out_of_the_snapshot_too() {
        let read = "qty.presence + ':' + qty.access + ':' + price.presence";
        let before = go(read, "form1[0].page1[0]", "click").result;
        let o = go(
            &format!(
                "price.presence = 'invisible'; for (var i = 0; i < 20000; i++) xfa.host.beep(); qty.presence = 'hidden'; qty.access = 'readOnly'; price.presence = 'hidden'; {read}"
            ),
            "form1[0].page1[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.effects.len(), MAX_EFFECTS);
        let unchanged = before.as_deref().and_then(|b| b.rsplit_once(':')).map(|(qty, _)| format!("{qty}:hidden"));
        assert_eq!(o.result, unchanged);
        assert!(o.effects.contains(&XfaEffect::SetPresence { som: "form1[0].page1[0].price[0]".into(), presence: "hidden".into() }));
        assert!(!o.effects.iter().any(|e| matches!(e, XfaEffect::SetPresence { som, .. } | XfaEffect::SetAccess { som, .. } if som.contains("qty"))));
    }

    /// Past the effect cap, a value written to a field with no effect yet is left out of reads as
    /// well as the effects; a field written again before the cap still takes its new value.
    #[test]
    fn a_value_past_the_effect_cap_is_left_out_of_reads_and_effects_alike() {
        let o = go(
            "price.rawValue = 1; for (var i = 0; i < 20000; i++) xfa.host.beep(); qty.rawValue = 9; price.rawValue = 3; qty.rawValue + ':' + price.rawValue",
            "form1[0].page1[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.effects.len(), MAX_EFFECTS);
        assert_eq!(o.result.as_deref(), Some("4:3"));
        assert!(!o.effects.iter().any(|e| matches!(e, XfaEffect::SetValue { som, .. } if som.contains("qty"))));
    }

    #[test]
    fn urls_and_reset_lists_are_clipped_and_deduplicated() {
        let o = go(
            "var s = 'x'; while (s.length < 65536) s += s; for (var i = 0; i < 600; i++) xfa.host.gotoURL(s + i); \
             var r = 'a,'; while (r.length < 262144) r += r; xfa.host.resetData(r);",
            "form1[0].page1[0]",
            "initialize",
        );
        assert_eq!(o.error, None);
        let urls: Vec<&String> = o.effects.iter().filter_map(|e| if let XfaEffect::LaunchUrl(u) = e { Some(u) } else { None }).collect();
        assert_eq!(urls.len(), 600);
        assert!(urls.iter().all(|u| u.chars().count() <= 4_097));
        let names: usize = o.effects.iter().map(|e| if let XfaEffect::ResetData(n) = e { n.len() } else { 0 }).sum();
        assert_eq!(names, 1);
    }

    #[test]
    fn a_slow_engine_start_does_not_count_against_the_script() {
        let ms = std::time::Duration::from_millis;
        // On a busy machine the engine can take longer to start than a script may run: the
        // script still runs (abandoning it would turn the form's scripts off).
        let o = run_within_limits("test", ms(10_000), ms(500), move |started| {
            std::thread::sleep(ms(1_500));
            started();
            XfaOutcome { result: Some("ran".into()), ..Default::default() }
        });
        assert!(!o.abandoned, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("ran"));
        // The script's own time counts from then.
        let o = run_within_limits("test", ms(10_000), ms(200), move |started| {
            started();
            std::thread::sleep(ms(2_000));
            XfaOutcome::default()
        });
        assert!(o.abandoned && o.error.as_deref().is_some_and(|e| e.contains("ran longer")), "{o:?}");
        // Start-up has a limit of its own, so nobody waits forever for an engine.
        let o = run_within_limits("test", ms(200), ms(10_000), move |started| {
            std::thread::sleep(ms(2_000));
            started();
            XfaOutcome::default()
        });
        assert!(o.abandoned && o.error.as_deref().is_some_and(|e| e.contains("to start")), "{o:?}");
        // A script that finishes without saying it started (refused at once) isn't abandoned.
        let o = run_within_limits("test", ms(10_000), ms(200), |_| XfaOutcome { error: Some("refused".into()), ..Default::default() });
        assert!(!o.abandoned && o.error.as_deref() == Some("refused"), "{o:?}");
    }
}

mod formcalc {
    use super::xfa_model::form;
    use crate::Limits;
    use crate::formcalc::run_formcalc;
    use crate::xfa::*;

    fn go(script: &str, target: &str, activity: &str) -> XfaOutcome {
        let ev = XfaEvent { activity: activity.into(), target: target.into(), ..Default::default() };
        let doc = XfaDoc { file_name: "f.pdf".into(), page: 1, page_count: 3 };
        run_formcalc(script, &ev, &doc, &form(), Limits::default())
    }

    /// The value of a script run as a calculate on `total`.
    fn calc(script: &str) -> String {
        let o = go(script, "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.error, None, "{script}");
        o.result.unwrap_or_default()
    }

    /// A negative value that rounds to zero under the picture prints without a minus sign.
    #[test]
    fn format_does_not_print_negative_zero() {
        assert_eq!(calc(r#"Format("9.9", -0.04)"#), "0.0");
        assert_eq!(calc(r#"Format("99", -0.2)"#), "00");
        assert_eq!(calc(r#"Format("9.9", -1.5)"#), "-1.5");
    }

    /// Assigning one long value to many fields at once stops at the run's byte budget and is
    /// charged per copy, and a reset list names each field once.
    #[test]
    fn bulk_assignments_and_reset_lists_are_bounded() {
        let f = |i: usize| super::xfa_model::field("f", &format!("form1[0].page1[0].f[{i}]"), "", false);
        let root = XfaNode {
            name: "form1".into(),
            som: "form1[0]".into(),
            kind: Some(XfaKind::Subform),
            children: vec![XfaNode {
                name: "page1".into(),
                som: "form1[0].page1[0]".into(),
                kind: Some(XfaKind::Subform),
                children: (0..1300).map(f).collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let ev = XfaEvent { activity: "initialize".into(), target: "form1[0]".into(), ..Default::default() };
        let run = |script: &str| run_formcalc(script, &ev, &XfaDoc::default(), &root, Limits::default());
        let o = run("var s = \"x\"\nwhile (Len(s) < 65536) do s = Concat(s, s) endwhile\npage1.f = s");
        assert_eq!(o.error, None, "{o:?}");
        let kept: usize = o.effects.iter().map(|e| if let XfaEffect::SetValue { value, .. } = e { value.len() } else { 0 }).sum();
        assert!(kept <= MAX_PAYLOAD, "{kept} bytes of values kept");
        assert!(o.notes.iter().any(|n| n.contains("left out")), "{:?}", o.notes);
        // 1300 copies of a 1 MiB value are more work than a script may do.
        let o = run("var s = \"x\"\nwhile (Len(s) < 1048576) do s = Concat(s, s) endwhile\npage1.f = s");
        assert_eq!(o.error.as_deref(), Some("the script ran too long"), "{:?}", o.notes);
        let o = run("var r = \"a,\"\nwhile (Len(r) < 262144) do r = Concat(r, r) endwhile\n$host.resetData(r)\n$host.resetData(r)");
        assert_eq!(o.error, None, "{o:?}");
        let names: usize = o.effects.iter().map(|e| if let XfaEffect::ResetData(n) = e { n.len() } else { 0 }).sum();
        assert_eq!(names, 2);
    }

    /// FormCalc presence and access past the effect cap are left out of the snapshot too.
    #[test]
    fn formcalc_presence_and_access_past_the_effect_cap_are_left_out_of_the_snapshot_too() {
        let o = go(
            "for i = 1 upto 20000 do $host.beep() endfor\nqty.presence = \"hidden\"\nqty.access = \"readOnly\"\nConcat(qty.presence, \":\", qty.access)",
            "form1[0].page1[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.effects.len(), MAX_EFFECTS);
        let before = go("Concat(qty.presence, \":\", qty.access)", "form1[0].page1[0]", "click");
        assert_eq!(o.result, before.result);
    }

    /// FormCalc reads `formattedValue` as the field's picture shows it too, until the script
    /// sets the value.
    #[test]
    fn formcalc_formatted_value_reads_the_shown_value_until_the_script_sets_one() {
        let ev = XfaEvent { activity: "calculate".into(), target: "form1[0].page1[0].d[0]".into(), ..Default::default() };
        let doc = XfaDoc::default();
        let run = |script: &str| run_formcalc(script, &ev, &doc, &super::xfa_model::dated_form(), Limits::default());
        let o = run("Concat($.formattedValue, \" \", $.rawValue)");
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("20261020 2026-10-20"));
        let o = run("$.rawValue = \"2026-10-21\"\n$.formattedValue");
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("2026-10-21"));
    }

    /// FormCalc arithmetic, string conversion and comparisons follow FormCalc's own rules.
    #[test]
    fn arithmetic_strings_and_comparisons_follow_formcalc_rules() {
        assert_eq!(calc("1 + 2 * 3"), "7");
        assert_eq!(calc("(1 + 2) * 3"), "9");
        assert_eq!(calc("10 / 4"), "2.5");
        assert_eq!(calc("-3 + +4"), "1");
        assert_eq!(calc(r#""12" + 3"#), "15");
        assert_eq!(calc(r#""abc" + 3"#), "3");
        assert_eq!(calc(r#"Concat("a", "b", 1.50)"#), "ab1.5");
        assert_eq!(calc(r##""say ""hi""""##), "say \"hi\"");
        assert_eq!(calc("1 < 2 and 2 <= 2"), "1");
        assert_eq!(calc("1 == 1.0 & 3 <> 4"), "1");
        assert_eq!(calc(r#""a" lt "b" | 0"#), "1");
        assert_eq!(calc("not 0"), "1");
        assert_eq!(calc("!0 + !1"), "1");
        assert_eq!(calc("table.row[0]..amount"), "10");
        assert_eq!(calc(r#""abc" == "ABC""#), "0");
        // Logical operands are numbers: text that is not a number is false.
        assert_eq!(calc(r#"if ("abc") then 1 else 0 endif"#), "0");
        assert_eq!(calc(r#"if ("2") then 1 else 0 endif"#), "1");
        assert_eq!(calc("null + 1"), "1");
        assert_eq!(calc("null == null"), "1");
        assert_eq!(calc("1 == null"), "0");
        assert_eq!(calc("1 <> null"), "1");
        assert!(go("1 / 0", "form1[0].page1[0].total[0]", "calculate").error.is_some());
        // Comments and newlines inside brackets.
        assert_eq!(calc("; a comment\n// another\nSum(1,\n 2,\n 3)"), "6");
    }

    #[test]
    fn date_pictures_render_the_weekday_letters() {
        assert_eq!(calc(r#"Num2Date(45350, DateFmt(4))"#), "Thursday, February 29, 2024");
        assert_eq!(calc(r#"Num2Date(45350, "EEE, YYYY-MM-DD")"#), "Thu, 2024-02-29");
        assert_eq!(calc(r#"Num2Date(45350, "EEEE")"#), "Thursday");
        assert_eq!(calc(r#"Num2Date(45350, "E")"#), "5");
        assert_eq!(calc(r#"Num2Date(45351, "EEEE")"#), "Friday");
        assert_eq!(calc(r#"Num2Date(45352, "EEEE")"#), "Saturday");
        assert_eq!(calc(r#"Num2Date(45353, "EEEE")"#), "Sunday");
        assert_eq!(calc(r#"Num2Date(1, "EEEE")"#), "Monday");
        assert_eq!(calc(r#"Format("EEEE, YYYY-MM-DD", 45350)"#), "Thursday, 2024-02-29");
        // Other letters are still literal text.
        assert_eq!(calc(r#"Num2Date(45350, "YYYY-MM-DD x")"#), "2024-02-29 x");
    }

    /// Accepted day numbers past chrono's own date range (about year 262143) keep the dates the
    /// pre-chrono algorithm gave, on both sides of the epoch, through Num2Date and Format.
    #[test]
    fn date_numbers_beyond_chronos_range_keep_their_dates() {
        assert_eq!(calc(r#"Num2Date(99000000, "YYYY-MM-DD")"#), "272952-10-16");
        assert_eq!(calc(r#"Format("YYYY-MM-DD", 99000000)"#), "272952-10-16");
        assert_eq!(calc(r#"Num2Date(99999999, "YYYY-MM-DD")"#), "275690-09-11");
        assert_eq!(calc(r#"Num2Date(-99000000, "YYYY-MM-DD")"#), "-269153-03-16");
        assert_eq!(calc(r#"Num2Date(-99999999, "YYYY-MM-DD")"#), "-271891-04-20");
    }

    #[test]
    fn str_and_format_round_exact_ties_away_from_zero() {
        assert_eq!(calc("Str(2.5, 3, 0)"), "  3");
        assert_eq!(calc("Str(0.5, 3, 0)"), "  1");
        assert_eq!(calc("Str(-2.5, 4, 0)"), "  -3");
        assert_eq!(calc("Str(1.125, 6, 2)"), "  1.13");
        assert_eq!(calc(r#"Format("9", 2.5)"#), "3");
        assert_eq!(calc(r#"Format("9.99", 0.125)"#), "0.13");
        assert_eq!(calc(r#"Format("9.9", 0.25)"#), "0.3");
        assert_eq!(calc(r#"Format("9.9", -0.25)"#), "-0.3");
        assert_eq!(calc(r#"Format("9.99", 2.344)"#), "2.34");
    }

    #[test]
    fn round_keeps_huge_values_finite() {
        assert_eq!(calc("Round(1e300, 12) == 1e300"), "1");
        assert_eq!(calc("Round(1.2345, 2)"), "1.23");
    }

    #[test]
    fn built_in_functions_cover_the_common_set() {
        assert_numeric_and_logical_builtins();
        assert_string_builtins();
        assert_format_and_date_builtins();
        assert_financial_and_unit_builtins();
        assert_builtin_errors();
    }

    fn assert_numeric_and_logical_builtins() {
        assert_eq!(calc("Abs(-2.5)"), "2.5");
        assert_eq!(calc(r#"Decode("a%2Fb", "url")"#), "a/b");
        assert_eq!(calc(r#"Decode("a%+fb", "url")"#), "a%+fb");
        assert_eq!(calc("Ceil(2.1)"), "3");
        assert_eq!(calc("Floor(-2.1)"), "-3");
        assert_eq!(calc("Mod(7, 3)"), "1");
        assert_eq!(calc("Round(2.345, 2)"), "2.35");
        assert_eq!(calc("Sum(1, 2, 3)"), "6");
        assert_eq!(calc("Avg(1, 2, 3, null)"), "2");
        assert_eq!(calc("Count(1, null, 3)"), "2");
        assert_eq!(calc("Max(1, 9, 3)"), "9");
        assert_eq!(calc("Min(4, 2, 3)"), "2");
        assert_eq!(calc(r#"Choose(2, "a", "b", "c")"#), "b");
        assert_eq!(calc(r#"Oneof(3, 1, 2, 3)"#), "1");
        assert_eq!(calc(r#"Within(5, 1, 10)"#), "1");
        assert_eq!(calc(r#"HasValue("")"#), "0");
        assert_eq!(calc(r#"HasValue("x")"#), "1");
    }

    fn assert_string_builtins() {
        assert_eq!(calc(r#"At("abcd", "c")"#), "3");
        assert_eq!(calc(r#"Left("hello", 2)"#), "he");
        assert_eq!(calc(r#"Right("hello", 2)"#), "lo");
        assert_eq!(calc(r#"Len("héllo")"#), "5");
        assert_eq!(calc(r#"Lower("ABC")"#), "abc");
        assert_eq!(calc(r#"Upper("abc")"#), "ABC");
        assert_eq!(calc(r#"Ltrim("  a ")"#), "a ");
        assert_eq!(calc(r#"Rtrim("  a ")"#), "  a");
        assert_eq!(calc(r#"Space(3)"#), "   ");
        assert_eq!(calc(r#"Str(3.14159, 6, 2)"#), "  3.14");
        assert_eq!(calc(r#"Substr("abcdef", 2, 3)"#), "bcd");
        assert_eq!(calc(r#"Stuff("abcdef", 2, 3, "XY")"#), "aXYef");
        assert_eq!(calc(r#"Replace("a-b-c", "-", "+")"#), "a+b+c");
        assert_eq!(calc(r#"WordNum(42)"#), "Forty-Two");
        assert_eq!(calc(r#"WordNum(1.999, 2)"#), "Two Dollars And 00 Cents");
        assert_eq!(calc(r#"WordNum(1.01, 2)"#), "One Dollar And 01 Cent");
        assert_eq!(calc(r#"Encode("a b&c", "url")"#), "a%20b%26c");
        assert_eq!(calc(r#"Decode("a%20b", "url")"#), "a b");
    }

    fn assert_format_and_date_builtins() {
        assert_eq!(calc(r#"Format("zzz,zz9.99", 1234.5)"#), "1,234.50");
        assert_eq!(calc(r#"Format("$z,zz9.99", -5)"#), "-$5.00");
        assert_eq!(calc(r#"Format("MM/DD/YYYY", "2024-02-29")"#), "02/29/2024");
        assert_eq!(calc(r#"Parse("zzz,zz9.99", "1,234.50")"#), "1234.5");
        assert_eq!(calc(r#"Parse("MMM D, YYYY", "Mar 15, 1996")"#), "1996-03-15");
        assert_eq!(calc(r#"Date2Num("1900-01-01")"#), "1");
        assert_eq!(calc(r#"Date2Num("2024-02-29")"#), "45350");
        assert_eq!(calc(r#"Date2Num("29/02/2024", "DD/MM/YYYY")"#), "45350");
        assert_eq!(calc(r#"Num2Date(45350, "YYYY-MM-DD")"#), "2024-02-29");
        assert_eq!(calc(r#"Num2Date(Date2Num("2024-02-29") + 1, "MMMM D, YYYY")"#), "March 1, 2024");
        assert_eq!(calc(r#"Time2Num("01:00:00")"#), "3600000");
        assert_eq!(calc(r#"IsoTime2Num("12:30:45Z")"#), "45045000");
        assert_eq!(calc(r#"IsoTime2Num("12:30:45.250")"#), "45045250");
        assert_eq!(calc(r#"IsoTime2Num("T12:30:45")"#), "45045000");
        assert_eq!(calc(r#"Num2Time(3661000)"#), "01:01:01");
    }

    fn assert_financial_and_unit_builtins() {
        assert_eq!(calc("Round(Pmt(10000, 0.01, 12), 2)"), "888.49");
        assert_eq!(calc("Round(FV(100, 0.01, 12), 2)"), "1268.25");
        assert_eq!(calc("Round(PV(100, 0.01, 12), 2)"), "1125.51");
        assert_eq!(calc(r#"UnitValue("2in", "mm")"#), "50.8");
        assert_eq!(calc(r#"UnitType("2.5cm")"#), "cm");
        assert_eq!(calc(r#"UnitValue("72points", "in")"#), "1");
        assert_eq!(calc(r#"UnitValue("25.4millimeters", "inches")"#), "1");
        assert_eq!(calc(r#"UnitValue("1in", "points")"#), "72");
    }

    fn assert_builtin_errors() {
        assert!(go(r#"Get("http://example.com")"#, "form1[0].page1[0].total[0]", "calculate").error.unwrap().contains("network"));
        assert!(go("Nope(1)", "form1[0].page1[0].total[0]", "calculate").error.unwrap().contains("unknown function"));
    }

    #[test]
    fn format_nine_pads_the_integer_part() {
        assert_eq!(calc(r#"Format("99999", 42)"#), "00042");
        assert_eq!(calc(r#"Format("9999", 5)"#), "0005");
        assert_eq!(calc(r#"Format("999.99", 4.5)"#), "004.50");
        assert_eq!(calc(r#"Format("zzz9", 42)"#), "42");
        assert_eq!(calc(r#"Format("99", 12345)"#), "12345");
    }

    #[test]
    fn date2num_rejects_days_past_the_end_of_the_month() {
        assert_eq!(calc(r#"Date2Num("2023-02-30")"#), "0");
        assert_eq!(calc(r#"Date2Num("31/04/2024", "DD/MM/YYYY")"#), "0");
        assert_eq!(calc(r#"Date2Num("2023-02-29")"#), "0");
        assert_eq!(calc(r#"Date2Num("1900-02-29")"#), "0");
        assert_eq!(calc(r#"Parse("YYYY-MM-DD", "2023-02-30") == null"#), "1");
        // Valid month ends still work.
        assert_eq!(calc(r#"Date2Num("2024-02-29")"#), "45350");
        assert_eq!(calc(r#"Date2Num("2000-02-29") <> 0"#), "1");
        assert_eq!(calc(r#"Date2Num("2024-04-30") <> 0"#), "1");
        assert_eq!(calc(r#"Date2Num("2024-12-31") <> 0"#), "1");
    }

    #[test]
    fn control_flow_variables_and_functions_work() {
        assert_eq!(calc("var x = 3\nif (x > 2) then 10 elseif (x > 1) then 20 else 30 endif"), "10");
        // An assignment is an expression: its value is the script's result.
        assert_eq!(calc("qty = 7"), "7");
        assert_eq!(calc("var y = 8"), "8");
        assert_eq!(calc("var x = 1\nif (x > 2) then 10 elseif (x > 0) then 20 else 30 endif"), "20");
        assert_eq!(calc("var i = 0\nvar s = 0\nwhile (i < 5) do\n i = i + 1\n if (i == 3) then continue endif\n s = s + i\nendwhile\ns"), "12");
        assert_eq!(calc("var s = 0\nfor i = 1 upto 10 step 3 do s = s + i endfor\ns"), "22");
        assert_eq!(calc("var s = 0\nfor i = 5 downto 1 do\n if (i == 2) then break endif\n s = s + i\nendfor\ns"), "12");
        assert_eq!(
            calc(
                r#"var s = ""
foreach w in ("a", "b", "c") do s = Concat(s, w) endfor
s"#
            ),
            "abc"
        );
        assert_eq!(calc("func sq(n) do n * n endfunc\nsq(4) + sq(3)"), "25");
        assert_eq!(calc("func Sq(n) do n * n endfunc\nSQ(4)"), "16");
        // `exit` inside a function ends the whole script.
        let o = go("func f() do exit endfunc\nf()\n$host.messageBox(\"still ran\")\n5", "form1[0].page1[0].go[0]", "click");
        assert_eq!(o.error, None);
        assert!(o.effects.is_empty(), "{:?}", o.effects);
        // Rows can be walked as objects.
        assert_eq!(calc("var s = 0\nforeach r in (table.row[*]) do s = s + r.amount endfor\ns"), "15");
        // A long sum is not deep nesting.
        let long = format!("1{}", " + 1".repeat(500));
        let ev = XfaEvent { activity: "calculate".into(), target: "form1[0].page1[0].total[0]".into(), ..Default::default() };
        let o = run_formcalc(&long, &ev, &XfaDoc::default(), &form(), Limits { loop_iterations: 1000, recursion: 64 });
        assert_eq!(o.result.as_deref(), Some("501"), "{o:?}");
        // Null is an empty result with no truth value, as in JavaScript.
        let o = go("if (0) then 5 endif", "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.result.as_deref(), Some(""));
        assert_eq!(o.result_bool, None);
        assert_eq!(calc("func f(n) do if (n < 2) then return 1 endif\n return n * f(n - 1) endfunc\nf(5)"), "120");
        assert_eq!(calc("IF (1) THEN 5 ENDIF"), "5");
        let o = go(r#"throw "stop here""#, "form1[0].page1[0].total[0]", "calculate");
        assert_eq!(o.error.as_deref(), Some("stop here"));
        let o = go("if (1) then 1", "form1[0].page1[0].total[0]", "calculate");
        assert!(o.error.unwrap().contains("endif"));
    }

    #[test]
    fn form_objects_resolve_assign_and_record_effects() {
        assert_formcalc_reads_fields_and_rows();
        assert_formcalc_reads_runtime_properties();
        assert_formcalc_records_primary_effects();
        assert_formcalc_updates_instances_and_records_requests();
    }

    fn assert_formcalc_reads_fields_and_rows() {
        // Fields deref to their values; `$` is the current field.
        assert_eq!(calc("qty * price"), "10");
        assert_eq!(calc("$.name"), "total");
        assert_eq!(calc("$form.form1.page1.qty + 1"), "5");
        assert_eq!(calc("xfa.form.form1.page1.qty"), "4");
        assert_eq!(calc("$record.page1.qty"), "4");
        assert_eq!(calc("Sum(table.row[*].amount)"), "15");
        assert_eq!(calc("Count(table.row[*])"), "2");
        assert_eq!(calc("Count(table.row[*].what)"), "1");
        // Lists hold each object once, however they were reached.
        assert_eq!(calc("Count((table.row[*].parent..row).parent..row)"), "2");
        assert_eq!(calc("Count(xfa.resolveNodes(\"table.row[*].parent.row[*].parent.row[*]\"))"), "2");
        // Bare names in a row see every sibling row, and `[-1]`/`[+1]` are relative to the row
        // the script runs in.
        let in_row = |s: &str| {
            let o = go(s, "form1[0].page1[0].table[0].row[1].amount[0]", "calculate");
            assert_eq!(o.error, None, "{s}");
            o.result.unwrap_or_default()
        };
        assert_eq!(in_row("Count(row[*])"), "2");
        assert_eq!(in_row("Sum(row[*].amount)"), "15");
        assert_eq!(in_row("row[1].amount"), "5");
        assert_eq!(in_row("row[-1].amount"), "10");
        let past = go("row[+1].amount", "form1[0].page1[0].table[0].row[1].amount[0]", "calculate");
        assert!(past.error.unwrap().contains("no object"));
        assert_eq!(in_row("Exists(row[-5])"), "0");
        let o = go("row[+1].amount", "form1[0].page1[0].table[0].row[0].amount[0]", "calculate");
        assert_eq!(o.result.as_deref(), Some("5"));
        assert_eq!(calc("table.row[1].amount"), "5");
        assert_eq!(calc("table.row.amount"), "10");
        assert_eq!(calc("$form..amount[1]"), "5");
        assert_eq!(calc("table._row.count"), "2");
        assert_eq!(calc("Exists(qty) & not Exists(nothing)"), "1");
        assert_eq!(calc("HasValue(total)"), "0");
    }

    fn assert_formcalc_reads_runtime_properties() {
        assert_eq!(calc("$host.numPages"), "3");
        assert_eq!(calc("$layout.pageCount()"), "3");
        assert_eq!(calc("$event.name"), "calculate");
        assert_eq!(calc("xfa.resolveNode(\"page1.price\").rawValue"), "2.5");
        assert_eq!(calc("$.border.fill.color.value"), "");
        assert!(go("nothing + 1", "form1[0].page1[0].total[0]", "calculate").error.unwrap().contains("unknown name"));
    }

    fn assert_formcalc_records_primary_effects() {
        // Assignment to a field (by name and by path), a property and a variable.
        let o = go(
            r#"page1.qty = 6
xfa.form.form1.page1.qty = 7
details.presence = "visible"
price.access = "readOnly"
$.border.fill.color.value = "255,0,0"
$host.messageBox(Concat("Total ", qty * price))
$host.setFocus("qty")
table._row.addInstance(1)
table._row.count"#,
            "form1[0].page1[0].go[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("3"));
        assert_eq!(
            o.effects,
            vec![
                XfaEffect::SetValue { som: "form1[0].page1[0].qty[0]".into(), value: "7".into() },
                XfaEffect::SetPresence { som: "form1[0].page1[0].details[0]".into(), presence: "visible".into() },
                XfaEffect::SetAccess { som: "form1[0].page1[0].price[0]".into(), access: "readOnly".into() },
                XfaEffect::MessageBox("Total 17.5".into()),
                XfaEffect::SetFocus("form1[0].page1[0].qty[0]".into()),
                XfaEffect::AddInstance { som: "form1[0].page1[0].table[0].row[2]".into() },
            ]
        );
    }

    fn assert_formcalc_updates_instances_and_records_requests() {
        let o = go(
            "table._row.removeInstance(0)\ntable._row.setInstances(1)\ntable._row.count = 0\ntable._row.count",
            "form1[0].page1[0].go[0]",
            "click",
        );
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.result.as_deref(), Some("1"));
        // Validate scripts give a truth value; a value the script sets wins over the result.
        let o = go("$ == null or $ <= 100", "form1[0].page1[0].qty[0]", "validate");
        assert_eq!(o.result_bool, Some(true));
        let o = go("$ > 100", "form1[0].page1[0].qty[0]", "validate");
        assert_eq!(o.result_bool, Some(false));
        // Reset, print and URLs are recorded as requests, not performed.
        let o = go(
            r#"$host.resetData("qty, price")
$host.print()
$host.gotoURL("https://example.org")"#,
            "form1[0].page1[0].go[0]",
            "click",
        );
        assert_eq!(
            o.effects,
            vec![XfaEffect::ResetData(vec!["qty".into(), "price".into()]), XfaEffect::Print, XfaEffect::LaunchUrl("https://example.org".into()),]
        );
    }

    #[test]
    fn formcalc_values_results_and_errors_stay_bounded() {
        let total = "form1[0].page1[0].total[0]";
        let fast = Limits { loop_iterations: 10_000, recursion: 32 };
        let ev = XfaEvent { activity: "calculate".into(), target: total.into(), ..Default::default() };
        let doc = XfaDoc::default();
        let run = |s: &str| run_formcalc(s, &ev, &doc, &form(), fast);
        // A long value is cut where it is stored, so it reads back as saved.
        let o = run("details.note = Space(100000)\nLen(details.note)");
        assert_eq!(o.error, None);
        assert_eq!(o.result.as_deref(), Some("65536"));
        // A calculate's result is cut as values are, with a note.
        let o = run("Space(100000)");
        assert_eq!(o.result.map(|r| r.chars().count()), Some(65_536));
        assert!(o.notes.iter().any(|n| n.contains("result")), "{:?}", o.notes);
        // A thrown value is the error message: cut like one.
        let e = run("throw Space(100000)").error.unwrap();
        assert!(e.chars().count() <= 4_097, "{}", e.len());
        // Copying and comparing long strings costs steps, so it ends without a clock (wasm).
        let e = run("var s = Space(1000000)\nfor i = 1 upto 3000 do s endfor").error;
        assert!(e.is_some_and(|e| e.contains("too long")));
        let e = run("var s = Space(1000000)\nfor i = 1 upto 3000 do s == \"x\" endfor").error;
        assert!(e.is_some_and(|e| e.contains("too long")));
        // Each period IPmt and PPmt walk costs a step.
        let e = run("for i = 1 upto 600 do IPmt(1e9, 0.01, 1, 1, 10000) endfor").error;
        assert!(e.is_some_and(|e| e.contains("too long")));
        // Exists answers no for a name that isn't there, but a limit inside it still stops the script.
        assert_eq!(run("Exists(nothing.here)").result.as_deref(), Some("0"));
        assert_eq!(run("Exists(qty)").result.as_deref(), Some("1"));
        assert!(run("func f(n) do Exists(f(n + 1)) endfunc\nf(1)").error.unwrap().contains("nested"));
        // An elseif condition's chain counts from zero, not from the last statement of the arm before.
        let script = format!("if 0 then\nvar x = 1{}\nelseif 1{} then\n2\nendif", "+1".repeat(900), "+1".repeat(200));
        assert_eq!(run(&script).result.as_deref(), Some("2"));
    }

    #[test]
    fn hostile_formcalc_scripts_end_with_an_error_and_bounded_output() {
        assert_hostile_control_flow_is_bounded();
        assert_hostile_numeric_inputs_are_safe();
        assert_hostile_outputs_are_bounded();
        assert_hostile_nesting_is_bounded();
        assert_hostile_deadlines_and_parse_errors();
        assert_hostile_instance_growth_is_bounded();
        assert_hostile_message_count_is_bounded();
        assert_hostile_small_stack_is_safe();
    }

    fn assert_hostile_control_flow_is_bounded() {
        assert!(run_hostile_formcalc("while (1) do endwhile").error.unwrap().contains("looped"));
        assert!(run_hostile_formcalc("for i = 1 upto 1000000 do endfor").error.unwrap().contains("looped"));
        assert!(run_hostile_formcalc("func f(n) do f(n + 1) endfunc\nf(1)").error.unwrap().contains("nested"));
        assert!(run_hostile_formcalc(&format!("{}1{}", "(".repeat(500), ")".repeat(500))).error.is_some());
        // A 120k-term chain is refused at parse time (its tree would be as deep as it is long).
        assert!(run_hostile_formcalc(&format!("1{}", "+1".repeat(120_000))).error.unwrap().contains("too long"));
        assert!(run_hostile_formcalc(&format!("$form{}", ".a".repeat(120_000))).error.unwrap().contains("too long"));
        assert!(run_hostile_formcalc("nothing.x = 1").error.unwrap().contains("unknown name"));
        assert!(run_hostile_formcalc("$form.nothing = 1").error.unwrap().contains("no nothing"));
        // Values go into fields only: a subform or the form itself is refused, nothing is recorded.
        let o = run_hostile_formcalc("details = \"junk\"");
        assert!(o.error.unwrap().contains("not a field"));
        assert!(o.effects.is_empty());
        assert!(run_hostile_formcalc("$form = \"junk\"").error.unwrap().contains("not a field"));
    }

    fn assert_hostile_numeric_inputs_are_safe() {
        // Hostile numbers into the string functions.
        assert_eq!(run_hostile_formcalc(r#"Stuff("abc", 2, 1e300, "X")"#).result.as_deref(), Some("aX"));
        assert_eq!(run_hostile_formcalc(r#"Stuff("abc", -1e300, 1, "X")"#).result.as_deref(), Some("Xbc"));
        assert_eq!(run_hostile_formcalc(r#"Substr("abc", 1e300, 1e300)"#).result.as_deref(), Some(""));
        assert_eq!(run_hostile_formcalc(r#"Left("abc", -1e300)"#).result.as_deref(), Some(""));
        assert!(run_hostile_formcalc(r#"Str(1e300, 1e300, 1e300)"#).error.is_none());
        assert!(run_hostile_formcalc(r#"Round(1e300, 1e300)"#).error.is_none());
        assert!(run_hostile_formcalc(r#"Choose(1e300, 1)"#).error.is_none());
    }

    fn assert_hostile_outputs_are_bounded() {
        // Strings can't grow past the cap through any function, and messages and values are clipped.
        assert!(
            run_hostile_formcalc("var s = Space(1000000)\nfor i = 1 upto 10 do s = Encode(Replace(s, \" \", \"&\"), \"html\") endfor\nLen(s)")
                .error
                .unwrap()
                .contains("too long")
        );
        assert!(run_hostile_formcalc("Upper(Concat(Space(600000), Space(600000)))").error.unwrap().contains("too long"));
        let o = run_hostile_formcalc("$host.messageBox(Space(100000))");
        assert!(matches!(o.effects.as_slice(), [XfaEffect::MessageBox(m)] if m.chars().count() <= 4100), "{:?}", o.effects.len());
        let o = run_hostile_formcalc("qty = Space(100000)");
        assert!(matches!(o.effects.as_slice(), [XfaEffect::SetValue { value, .. }] if value.chars().count() == 65_536), "{:?}", o.effects.len());
        assert!(o.notes.iter().any(|n| n.contains("cut")), "{:?}", o.notes);
    }

    fn assert_hostile_nesting_is_bounded() {
        // A comment of dashes is not deep nesting.
        assert_eq!(run_hostile_formcalc(&format!("; {}\n1 + 1", "-".repeat(70))).result.as_deref(), Some("2"));
        // Parentheses don't reset the chain count: a tree is never deeper than the cap.
        let mut nested = "1+1+1+1+1+1+1+1+1+1".to_string();
        for _ in 0..30 {
            nested = format!("({nested}{})", "+1".repeat(40));
        }
        assert!(run_hostile_formcalc(&nested).error.unwrap().contains("too long"));
        assert_eq!(run_hostile_formcalc("(1+2)*(3+4) + (5+6)").result.as_deref(), Some("32"));
        // Nested statements inside recursion count against the depth limit.
        let deep = format!("func f(n) do\n{}return f(n - 1)\n{}endfunc\nf(1000)", "if (n > 0) then\n".repeat(20), "endif\n".repeat(20));
        assert!(run_hostile_formcalc(&deep).error.unwrap().contains("nested"));
    }

    fn assert_hostile_deadlines_and_parse_errors() {
        let total = "form1[0].page1[0].total[0]";
        let ev = XfaEvent { activity: "calculate".into(), target: total.into(), ..Default::default() };
        let doc = XfaDoc::default();
        // A deadline already passed stops the script at its first look at the clock.
        let o = crate::formcalc::run_formcalc_at(
            "for i = 1 upto 100000 do Eval(\"1\") endfor",
            &ev,
            &doc,
            form(),
            Limits { loop_iterations: u64::MAX, recursion: 32 },
            Some(std::time::Duration::ZERO),
        );
        assert!(o.error.unwrap().contains("too long"));
        assert!(run_hostile_formcalc(&format!("{}1{}", "-".repeat(5000), "")).error.is_some());
        assert!(run_hostile_formcalc("var s = \"x\"\nfor i = 1 upto 100 do s = Concat(s, s) endfor\nLen(s)").error.unwrap().contains("too long"));
        assert!(run_hostile_formcalc("Space(1e12)").error.is_none());
        assert!(run_hostile_formcalc("Replace(Space(100000), \" \", Space(100000))").error.unwrap().contains("too long"));
        assert_eq!(run_hostile_formcalc(r#"Format("YYYY-MM-DD", 1e300)"#).error, None);
        assert_eq!(run_hostile_formcalc(r#"Date2Num("99999999999999999999-01-01", "YYYYYYYYYYYYYYYYYYYY-MM-DD")"#).error, None);
        assert_eq!(run_hostile_formcalc(r#"Time2Num("99999999999999999999:0:0")"#).error, None);
        assert_eq!(run_hostile_formcalc(r#"Num2Date(1e300, "YYYY")"#).result.as_deref(), Some(""));
        assert!(run_hostile_formcalc("for i = 1 upto 1000 do nothing endfor").error.unwrap().contains("unknown name"));
        assert!(run_hostile_formcalc("\"unterminated").error.unwrap().contains("unterminated"));
        assert!(run_hostile_formcalc("1 ) 2").error.is_some());
        assert!(run_hostile_formcalc("Eval(\"Eval(\"\"Eval(\"\"\"\"1\"\"\"\")\"\")\")").error.is_none());
    }

    fn assert_hostile_instance_growth_is_bounded() {
        // Adding rows forever stops at the instance cap with the effects it managed.
        let o = run_hostile_formcalc("for i = 1 upto 100000 do table._row.addInstance(1) endfor\ntable._row.count");
        assert!(o.error.unwrap().contains("looped"));
        assert!(o.effects.iter().filter(|e| matches!(e, XfaEffect::AddInstance { .. })).count() <= 1000, "{}", o.effects.len());
        let o = run_hostile_formcalc("for i = 1 upto 5000 do table._row.addInstance(1) endfor\ntable._row.count");
        assert_eq!(o.error, None);
        assert_eq!(o.result.as_deref(), Some("1002"));
    }

    fn assert_hostile_message_count_is_bounded() {
        // Message boxes past the cap are dropped and noted.
        let o = run_hostile_formcalc("for i = 1 upto 500 do $host.messageBox(i) endfor");
        assert_eq!(o.effects.iter().filter(|e| matches!(e, XfaEffect::MessageBox(_))).count(), 100);
        assert!(!o.notes.is_empty());
    }

    fn assert_hostile_small_stack_is_safe() {
        let ev = XfaEvent { activity: "calculate".into(), target: "form1[0].page1[0].total[0]".into(), ..Default::default() };
        let doc = XfaDoc::default();
        // On a small stack (the wasm build's), the limits stop the script before the stack does.
        let small = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                let fast = Limits { loop_iterations: 10_000, recursion: 256 };
                let ev = XfaEvent { activity: "click".into(), target: "form1[0].page1[0].go[0]".into(), ..Default::default() };
                let doc = XfaDoc::default();
                let run = |s: &str| run_formcalc(s, &ev, &doc, &form(), fast);
                assert!(run("func f(n) do f(n + 1) endfunc\nf(1)").error.unwrap().contains("nested"));
                let deep = format!("func f(n) do\n{}return f(n - 1)\n{}endfunc\nf(1000)", "if (n > 0) then\n".repeat(20), "endif\n".repeat(20));
                assert!(run(&deep).error.unwrap().contains("nested"));
                let mut nested = "1".to_string();
                for _ in 0..30 {
                    nested = format!("({nested}{})", "+1".repeat(40));
                }
                assert!(run(&nested).error.is_some());
                let chain = format!("1{}", "+1".repeat(999));
                assert_eq!(run(&chain).result.as_deref(), Some("1000"));
                let dots = format!("$form{}", ".parent".repeat(60));
                assert!(run(&dots).error.unwrap().contains("no object"));
                let dots = format!("${}", ".parent".repeat(3));
                assert_eq!(run(&dots).result.as_deref(), Some(""));
            })
            .unwrap();
        small.join().expect("no stack overflow on a 1 MiB stack");
        // Abandoned when it runs past the deadline.
        let o = crate::formcalc::run_formcalc_within(
            "while (1) do endwhile",
            &ev,
            &doc,
            form(),
            Limits { loop_iterations: u64::MAX, recursion: 32 },
            std::time::Duration::from_millis(200),
        );
        assert!(o.abandoned || o.error.is_some(), "{o:?}");
    }

    fn run_hostile_formcalc(script: &str) -> XfaOutcome {
        let ev = XfaEvent { activity: "calculate".into(), target: "form1[0].page1[0].total[0]".into(), ..Default::default() };
        let doc = XfaDoc::default();
        let fast = Limits { loop_iterations: 10_000, recursion: 32 };
        run_formcalc(script, &ev, &doc, &form(), fast)
    }
}

mod session {
    use super::xfa_model::form;
    use crate::Limits;
    use crate::xfa::*;
    use crate::xfa_session::{Script, XfaSession};

    /// A JavaScript or FormCalc script for `activity` on `target`, against the test form with
    /// default limits and a 3 s timeout.
    fn script(source: &str, formcalc: bool, target: &str, activity: &str) -> Script {
        Script {
            source: source.into(),
            formcalc,
            event: XfaEvent { activity: activity.into(), target: target.into(), ..Default::default() },
            root: form(),
            limits: Limits::default(),
            timeout: std::time::Duration::from_secs(3),
        }
    }

    /// JavaScript and FormCalc scripts run in turn in one session, each on the form the caller
    /// sends: nothing an earlier script changed or defined leaks into the next, a failing script
    /// doesn't end the session, and hundreds of scripts run on its one thread.
    #[test]
    fn scripts_of_one_event_run_on_one_thread_each_on_the_form_the_caller_sends() {
        let mut s = XfaSession::new(XfaDoc { page_count: 3, ..Default::default() });
        assert_eq!(s.dead(), None);
        // JavaScript, then FormCalc, on the same thread.
        let o = s.run(script("qty.rawValue = 10; qty.rawValue", false, "form1[0].page1[0].total[0]", "calculate"));
        assert_eq!(o.error, None, "{o:?}");
        assert_eq!(o.effects.len(), 1);
        assert_eq!(o.result.as_deref(), Some("10"));
        // The next script sees the form the caller sends, not what the last script did on
        // its copy: nothing a script changed leaks past the caller.
        let o = s.run(script("qty * price", true, "form1[0].page1[0].total[0]", "calculate"));
        assert_eq!(o.result.as_deref(), Some("10"), "{o:?}");
        let mut with = script("qty * price", true, "form1[0].page1[0].total[0]", "calculate");
        with.root.children[0].children[0].value = "10".into();
        assert_eq!(s.run(with).result.as_deref(), Some("25"));
        // Nor do variables, globals, prototype changes or kept objects.
        s.run(script("leak = 5; String.prototype.trim = function () { return 'P' }; kept = qty; 1", false, "form1[0].page1[0].qty[0]", "calculate"));
        let o = s.run(script("typeof leak + ':' + ' x '.trim() + ':' + typeof kept", false, "form1[0].page1[0].qty[0]", "calculate"));
        assert_eq!(o.result.as_deref(), Some("undefined:x:undefined"));
        s.run(script("Object.freeze(globalThis); 1", false, "form1[0].page1[0].qty[0]", "calculate"));
        let o = s.run(script("xfa.host.numPages", false, "form1[0].page1[0].qty[0]", "calculate"));
        assert_eq!(o.result.as_deref(), Some("3"), "{o:?}");
        // `this` and `event` follow each script's target.
        let o = s.run(script("this.name + ':' + xfa.event.name", false, "form1[0].page1[0].price[0]", "change"));
        assert_eq!(o.result.as_deref(), Some("price:change"));
        // A failing script doesn't end the session.
        let o = s.run(script("throw new Error('no')", false, "form1[0].page1[0].go[0]", "click"));
        assert!(o.error.is_some() && !o.abandoned);
        assert_eq!(s.run(script("1 + 1", true, "form1[0].page1[0].go[0]", "click")).result.as_deref(), Some("2"));
        // Hundreds of scripts on the one thread.
        let started = std::time::Instant::now();
        for i in 0..300 {
            let o = s.run(script(&format!("{i} + qty.rawValue"), i % 2 == 0, "form1[0].page1[0].total[0]", "calculate"));
            assert_eq!(o.error, None, "{o:?}");
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    }

    /// A JavaScript or FormCalc script whose engine takes longer to be ready than the script's
    /// time limit still runs and returns its result, and the session stays alive.
    #[test]
    fn a_scripts_time_starts_when_its_context_is_ready() {
        // The engine made slow to be ready (300 ms) under a 50 ms script limit: the script is
        // not abandoned, its time starting only when the engine says it is ready.
        use crate::xfa_session::START_DELAY_MS;
        use std::sync::atomic::Ordering::Relaxed;
        let mut s = XfaSession::new(XfaDoc::default());
        START_DELAY_MS.store(300, Relaxed);
        let mut quick = script("1", false, "form1[0].page1[0].go[0]", "click");
        quick.timeout = std::time::Duration::from_millis(50);
        let js = s.run(quick);
        let mut quick = script("1", true, "form1[0].page1[0].go[0]", "click");
        quick.timeout = std::time::Duration::from_millis(50);
        let fc = s.run(quick);
        START_DELAY_MS.store(0, Relaxed);
        assert!(!js.abandoned && js.result.as_deref() == Some("1"), "{js:?}");
        assert!(!fc.abandoned && fc.result.as_deref() == Some("1"), "{fc:?}");
        assert_eq!(s.dead(), None);
    }

    /// An endless JavaScript loop past its 200 ms limit is abandoned within 2 s and leaves the
    /// session dead, so nothing more runs in it; an endless FormCalc loop is abandoned or
    /// reported as running too long.
    #[test]
    fn a_script_past_its_time_is_abandoned_and_the_session_is_dead_after_it() {
        let mut s = XfaSession::new(XfaDoc::default());
        let mut slow = script("while (true) {}", false, "form1[0].page1[0].go[0]", "click");
        slow.limits = Limits { loop_iterations: u64::MAX, recursion: 32 };
        slow.timeout = std::time::Duration::from_millis(200);
        let started = std::time::Instant::now();
        let o = s.run(slow);
        assert!(o.abandoned, "{o:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(s.dead().is_some_and(|w| w.contains("abandoned")));
        let o = s.run(script("1", false, "form1[0].page1[0].go[0]", "click"));
        assert!(o.abandoned && o.result.is_none(), "nothing more runs: {o:?}");
        // FormCalc past its time too (it stops itself at the deadline and is reported).
        let mut s = XfaSession::new(XfaDoc::default());
        let mut slow = script("while (1) do endwhile", true, "form1[0].page1[0].go[0]", "click");
        slow.limits = Limits { loop_iterations: u64::MAX, recursion: 32 };
        slow.timeout = std::time::Duration::from_millis(200);
        let o = s.run(slow);
        assert!(o.abandoned || o.error.as_deref().is_some_and(|e| e.contains("too long")), "{o:?}");
    }
}

#[test]
fn shared_field_snapshots_isolate_writes_errors_and_script_globals() {
    let initial = Arc::new(fields());
    let before = initial.as_ref().clone();
    let event = Event::field("Calculate", "total", "");
    let write = run_shared(
        "var scriptLocal = 1; getField('qty').value = 9; getField('agree').checkThisBox(0, true); event.value = getField('qty').value;",
        &event,
        &doc(),
        Arc::clone(&initial),
        &[],
        Limits::default(),
    );
    assert_eq!(write.error, None);
    assert_eq!(write.value, "9");
    assert_eq!(write.changed.iter().find(|f| f.name == "qty").unwrap().value, ["9"]);
    assert_eq!(write.changed.iter().find(|f| f.name == "agree").unwrap().value, ["Yes"]);
    assert_eq!(*initial, before, "script writes cannot mutate the caller's snapshot");
    let failed = run_shared("getField('qty').value = 99; throw new Error('rejected');", &event, &doc(), Arc::clone(&initial), &[], Limits::default());
    assert!(failed.error.is_some());
    assert_eq!(*initial, before, "an exception cannot leak partial writes into the initial snapshot");
    let read = run_shared(
        "event.value = typeof scriptLocal + ':' + getField('qty').value + ':' + getField('agree').isBoxChecked(0);",
        &event,
        &doc(),
        initial,
        &[],
        Limits::default(),
    );
    assert_eq!(read.error, None);
    assert_eq!(read.value, "undefined:4:false");
    assert!(read.changed.is_empty());
}

mod scand {
    use super::*;
    use crate::xfa::{XfaDoc, XfaEvent, run_xfa};

    /// Runs `util.scand(format, text)` for each case and checks the date it gives, written as
    /// `yyyy-mm-dd HH:MM:ss`, or `null`.
    fn assert_scand(cases: &[(&str, &str, &str)]) {
        let list: Vec<String> = cases.iter().map(|(f, t, _)| format!("['{f}', '{t}']")).collect();
        let script = format!(
            "event.value = [{}].map(function (c) {{ var d = util.scand(c[0], c[1]); \
             return d === null ? 'null' : util.printd('yyyy-mm-dd HH:MM:ss', d); }}).join('|');",
            list.join(", ")
        );
        let o = go(&script, &Event::field("Calculate", "total", ""));
        assert_eq!(o.error, None);
        let got: Vec<&str> = o.value.split('|').collect();
        assert_eq!(got.len(), cases.len());
        for ((f, t, want), got) in cases.iter().zip(got) {
            assert_eq!(got, *want, "util.scand('{f}', '{t}')");
        }
    }

    // The cases below are another viewer's answers, observed black-box with a synthetic file.

    /// Any separator may come between fields, whether the format has that one, another or none,
    /// but a text with a space or a letter before or after the date reads nothing.
    #[test]
    fn scand_allows_separators_between_fields_but_nothing_before_or_after_them() {
        assert_scand(&[
            ("yyyymmdd", "20261018", "2026-10-18 00:00:00"),
            ("yyyymmdd", "2026-10-18", "2026-10-18 00:00:00"),
            ("yyyymmdd", "2026 10 18", "2026-10-18 00:00:00"),
            ("mm/dd/yyyy", "10/18/2026", "2026-10-18 00:00:00"),
            ("mm/dd/yyyy", "10-18-2026", "2026-10-18 00:00:00"),
            ("yyyymmdd", " 20261018 ", "null"),
            ("yyyymmdd", "20261018x", "null"),
            ("yyyymmdd", "x20261018", "null"),
        ]);
    }

    /// Digits typed together fill the joined fields with exactly as many digits as each field has
    /// letters, so a run of any other length reads nothing.
    #[test]
    fn scand_splits_digits_typed_together_by_the_formats_letter_counts() {
        assert_scand(&[
            ("yyyymmdd", "261018", "null"),
            ("yyyymmdd", "2026118", "null"),
            ("yyyymmdd", "202610181", "null"),
            ("yymmdd", "20261018", "null"),
            ("yyyymdd", "2026118", "2026-01-18 00:00:00"),
            ("yyyymmd", "2026118", "2026-11-08 00:00:00"),
            ("yyyymd", "2026118", "null"),
            ("yyyymd", "202618", "2026-01-08 00:00:00"),
            ("yyyymd", "20261018", "null"),
            ("yymd", "2618", "2026-01-08 00:00:00"),
            ("yymd", "261018", "null"),
        ]);
    }

    /// A month past 12, a day past the month's end and February 29 outside a leap year are
    /// refused, while a month or day of 0 is read as 1.
    #[test]
    fn scand_refuses_impossible_dates_and_reads_a_zero_month_or_day_as_1() {
        assert_scand(&[
            ("yyyymmdd", "20261318", "null"),
            ("yyyymmdd", "20260230", "null"),
            ("yyyymmdd", "20260229", "null"),
            ("yyyymmdd", "20240229", "2024-02-29 00:00:00"),
            ("mm/dd/yyyy", "13/45/2026", "null"),
            ("yyyymmdd", "20260018", "2026-01-18 00:00:00"),
            ("yyyymmdd", "20261000", "2026-10-01 00:00:00"),
        ]);
    }

    /// A field typed by itself may have one or two digits, but a `yyyy` year needs four; `yy` reads
    /// as 1950–2049; month names may be full or three letters in any case; weekday names are read.
    #[test]
    fn scand_reads_lone_fields_two_digit_years_and_month_and_weekday_names() {
        assert_scand(&[
            ("mm/dd/yyyy", "1/5/2026", "2026-01-05 00:00:00"),
            ("m/d/yyyy", "1/5/2026", "2026-01-05 00:00:00"),
            ("m/d/yyyy", "01/05/2026", "2026-01-05 00:00:00"),
            ("m/d/yy", "1/5/26", "2026-01-05 00:00:00"),
            ("mm/dd/yyyy", "10/18/26", "null"),
            ("yyyy/mm/dd", "26/10/18", "null"),
            ("yymmdd", "261018", "2026-10-18 00:00:00"),
            ("yymmdd", "491018", "2049-10-18 00:00:00"),
            ("yymmdd", "501018", "1950-10-18 00:00:00"),
            ("mmm d, yyyy", "Oct 18, 2026", "2026-10-18 00:00:00"),
            ("mmm d, yyyy", "October 18, 2026", "2026-10-18 00:00:00"),
            ("mmm d, yyyy", "oct 18, 2026", "2026-10-18 00:00:00"),
            ("mmmm d, yyyy", "Oct 18, 2026", "2026-10-18 00:00:00"),
            ("mmmm d, yyyy", "October 18, 2026", "2026-10-18 00:00:00"),
            ("dd-mmm-yyyy", "18-Oct-2026", "2026-10-18 00:00:00"),
            ("dddd, mmmm d, yyyy", "Sunday, October 18, 2026", "2026-10-18 00:00:00"),
        ]);
    }

    /// A time is read along with a date, but a format with only time fields reads nothing.
    #[test]
    fn scand_reads_a_time_only_with_a_date() {
        assert_scand(&[
            ("mm/dd/yyyy HH:MM:ss", "10/18/2026 21:07:05", "2026-10-18 21:07:05"),
            ("HH:MM", "09:07", "null"),
            ("HH:MM", "9:07", "null"),
            ("h:MM tt", "9:07 pm", "null"),
            ("h:MM tt", "9:07 am", "null"),
            ("h:MM tt", "12:07 am", "null"),
            ("h:MM tt", "9:07pm", "null"),
        ]);
    }

    /// An empty text reads as today at noon, and a text that is not a string (a number, or none
    /// at all) reads nothing.
    #[test]
    fn scand_reads_only_text_and_an_empty_text_is_today_at_noon() {
        let o = go(
            "var d = util.scand('yyyymmdd', ''), n = new Date(); \
             event.value = [d.toDateString() == n.toDateString(), d.getHours(), d.getMinutes(), d.getSeconds(), \
             util.scand('yyyymmdd', 20261018), util.scand(), util.scand('yyyymmdd')].join('|');",
            &Event::field("Calculate", "total", ""),
        );
        assert_eq!(o.error, None);
        assert_eq!(o.value, "true|12|0|0|||");
    }

    // Not observed: the choices made where the other viewer wasn't asked.

    /// A year left out is this year, `h:MM tt` reads a 12-hour time and refuses hours it cannot
    /// have, and a letter the format writes must be typed there.
    #[test]
    fn scand_takes_this_year_when_none_is_given_and_reads_12_hour_times_and_written_letters() {
        assert_scand(&[
            ("mm/dd/yyyy h:MM tt", "10/18/2026 9:07 pm", "2026-10-18 21:07:00"),
            ("mm/dd/yyyy h:MM tt", "10/18/2026 12:07 AM", "2026-10-18 00:07:00"),
            ("mm/dd/yyyy h:MM tt", "10/18/2026 13:07 pm", "null"),
            ("mm/dd/yyyy HH:MM", "10/18/2026 24:00", "null"),
            ("yyyy-mm-ddTHH:MM", "2026-10-18T21:07", "2026-10-18 21:07:00"),
            ("yyyy-mm-ddTHH:MM", "2026-10-18X21:07", "null"),
        ]);
        let o = go(
            "var d = util.scand('mm/dd', '10/18'); event.value = [d.getFullYear() == new Date().getFullYear(), d.getMonth(), d.getDate()].join('|');",
            &Event::field("Calculate", "total", ""),
        );
        assert_eq!(o.error, None);
        assert_eq!(o.value, "true|9|18");
    }

    /// A format of 50,000 literal letters and one of 100,000 fields, with texts to match, are read
    /// in linear time. The limit is generous so a loaded runner passes (it also covers starting
    /// the engine and building the strings); a quadratic scan of inputs this size takes minutes.
    #[test]
    fn scand_reads_long_formats_and_texts_in_linear_time() {
        // Scripts can pass anything; native code isn't bounded by the loop limit.
        let started = std::time::Instant::now();
        let o = go(
            "var x = new Array(50001).join('x'), md = new Array(50001).join('md'), ones = new Array(100000).join('1 ') + '1'; \
             event.value = [util.scand(x + 'yyyy', x + '2026') !== null, util.scand('yyyy' + md, '2026 ' + ones) !== null].join('|');",
            &Event::field("Calculate", "total", ""),
        );
        assert_eq!(o.error, None);
        assert_eq!(o.value, "true|true");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    }

    /// The check an XFA form runs on leaving a date field tries six formats with `util.scand` and
    /// writes the first that reads the text back as `yyyymmdd`, or reports an invalid date.
    #[test]
    fn xfa_scripts_check_typed_dates_with_util_scand() {
        // The check a form runs when a date field is left: the first of six formats that reads
        // the typed text, written back as yyyymmdd; or an invalid date. Results as another viewer gives them.
        let check = |typed: &str| {
            let script = format!(
                "var six = ['yyyymmdd', 'yymmdd', 'yyyymdd', 'yyyymmd', 'yyyymd', 'yymd'], out = 'invalid'; \
                 for (var i = 0; i < six.length; i++) {{ var d = util.scand(six[i], '{typed}'); \
                 if (d !== null) {{ out = util.printd('yyyymmdd', d); break; }} }} out"
            );
            let event = XfaEvent { activity: "exit".into(), target: "form1[0].page1[0].total[0]".into(), ..Default::default() };
            run_xfa(&script, &event, &XfaDoc::default(), &super::xfa_model::form(), Limits::default())
        };
        for (typed, want) in [
            ("20261018", "20261018"),
            ("261018", "20261018"),
            ("2026118", "20260118"),
            ("202618", "20260108"),
            ("2618", "20260108"),
            ("2026-10-18", "20261018"),
            ("20261318", "invalid"),
            ("20260230", "invalid"),
        ] {
            let o = check(typed);
            assert_eq!(o.error, None, "{typed}: {o:?}");
            assert_eq!(o.result.as_deref(), Some(want), "{typed}");
        }
    }
}

mod menu_items {
    use super::*;
    use crate::xfa::{XfaDoc, XfaEffect, XfaEvent, run_xfa};

    /// Every side panel has a fixed id in automation output.
    #[test]
    fn each_panel_has_a_fixed_id_for_automation() {
        let ids = [Panel::Bookmarks, Panel::Pages, Panel::Attachments, Panel::Signatures, Panel::Layers].map(Panel::id);
        assert_eq!(ids, ["bookmarks", "pages", "attachments", "signatures", "layers"]);
    }

    /// Each side panel item, Print and SaveAs make their request, in order; an unknown item
    /// makes none and is not an error.
    #[test]
    fn exec_menu_item_shows_side_panels_prints_and_saves() {
        let o = go(
            "app.execMenuItem('ShowHideFileAttachment'); app.execMenuItem('ShowHideBookmarks'); app.execMenuItem('ShowHideThumbnails'); \
             app.execMenuItem('ShowHideSignatures'); app.execMenuItem('ShowHideOptCont'); app.execMenuItem('Print'); \
             app.execMenuItem('SaveAs'); app.execMenuItem('NoSuchItem');",
            &Event::field("Mouse Up", "total", ""),
        );
        assert_eq!(o.error, None);
        assert_eq!(
            o.requests,
            [
                Request::ShowPanel(Panel::Attachments),
                Request::ShowPanel(Panel::Bookmarks),
                Request::ShowPanel(Panel::Pages),
                Request::ShowPanel(Panel::Signatures),
                Request::ShowPanel(Panel::Layers),
                Request::Print,
                Request::SaveAs,
            ]
        );
    }

    /// `app.execMenuItem` works in XFA scripts too: a click shows the attachments panel.
    #[test]
    fn xfa_scripts_show_the_attachments_panel() {
        let event = XfaEvent { activity: "click".into(), target: "form1[0].page1[0].go[0]".into(), ..Default::default() };
        let o = run_xfa("app.execMenuItem('ShowHideFileAttachment')", &event, &XfaDoc::default(), &super::xfa_model::form(), Limits::default());
        assert_eq!(o.error, None, "{o:?}");
        assert!(o.effects.contains(&XfaEffect::ShowPanel(Panel::Attachments)), "{:?}", o.effects);
    }
}

/// The check XFA forms run on opening: without both globals, or with a version below 2.8, it
/// alerts that the viewer can't show the form.
const XFA_CHECK: &str = "if (typeof(xfa_installed) == 'undefined' || typeof(xfa_version) == 'undefined' || xfa_version < 2.8) app.alert('old viewer'); \
                         event.value = typeof xfa_installed + ' ' + typeof xfa_version + ' ' + xfa_version;";

/// An XFA form sees `xfa_installed` (true) and `xfa_version` (3.5), so the check passes.
#[test]
fn an_xfa_form_sees_the_xfa_plugin_installed() {
    let xfa = DocInfo { xfa: true, ..doc() };
    let o = run(XFA_CHECK, &Event::doc("Open"), &xfa, &fields(), &[], Limits::default());
    assert_eq!(o.error, None);
    assert_eq!(o.value, "boolean number 3.5");
    assert!(o.alerts.is_empty(), "{:?}", o.alerts);
    let o = run("event.value = String(xfa_installed);", &Event::doc("Open"), &xfa, &fields(), &[], Limits::default());
    assert_eq!(o.value, "true");
}

/// A form without XFA sees `xfa_version` but no `xfa_installed`, so the check alerts.
#[test]
fn a_plain_form_sees_the_xfa_version_but_not_installed() {
    let o = go(XFA_CHECK, &Event::doc("Open"));
    assert_eq!(o.error, None);
    assert_eq!(o.value, "undefined number 3.5");
    assert_eq!(o.alerts, ["old viewer"]);
}
