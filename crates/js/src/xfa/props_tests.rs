//! Property objects as scripts see them: the spec's defaults, the template's values,
//! `getElement`, `oneOfChild`, `isPropertySpecified` and writes read back in the same run.

use std::sync::Arc;

use super::super::{XfaDoc, XfaEvent, XfaKind, XfaNode, XfaOutcome, run_xfa};
use super::{XfaProp, XfaProps};
use crate::Limits;

fn p(tag: &str, attrs: &[(&str, &str)], children: Vec<XfaProp>) -> XfaProp {
    XfaProp { tag: tag.into(), attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), text: None, children }
}

fn t(tag: &str, attrs: &[(&str, &str)], text: &str) -> XfaProp {
    XfaProp { text: Some(text.into()), ..p(tag, attrs, Vec::new()) }
}

fn node(name: &str, kind: XfaKind, attrs: &[(&str, &str)], elements: Vec<XfaProp>) -> XfaNode {
    XfaNode {
        name: name.into(),
        som: format!("form1[0].{name}[0]"),
        kind: Some(kind),
        presence: "visible".into(),
        props: Arc::new(XfaProps { attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), elements }),
        ..Default::default()
    }
}

/// form1 > { styled (a field with most properties written), plain (a field with none), label (a
/// draw with a text edit), rows (a repeating subform) }
fn form() -> XfaNode {
    let edge = |rgb: &str| p("edge", &[("thickness", "2pt")], vec![p("color", &[("value", rgb)], vec![])]);
    let styled = node(
        "styled",
        XfaKind::Field,
        &[("name", "styled"), ("w", "2in")],
        vec![
            p(
                "border",
                &[("presence", "hidden")],
                vec![
                    edge("255,0,0"),
                    edge("0,0,255"),
                    p("fill", &[], vec![p("color", &[("value", "1,2,3")], vec![]), p("linear", &[("type", "toTop")], vec![])]),
                ],
            ),
            p("ui", &[], vec![p("choiceList", &[("open", "always")], vec![p("border", &[], vec![edge("9,9,9")])])]),
            p(
                "validate",
                &[("nullTest", "error")],
                vec![
                    p("message", &[], vec![t("text", &[("name", "nullTest")], "Required"), t("text", &[("name", "scriptTest")], "Too big")]),
                    t("script", &[("contentType", "application/x-javascript")], "this.rawValue > 0"),
                ],
            ),
            p("font", &[("typeface", "Arial")], vec![]),
            p("assist", &[], vec![t("toolTip", &[], "Pick one")]),
            p("format", &[], vec![t("picture", &[], "num{z9}")]),
            p("value", &[], vec![p("decimal", &[], vec![])]),
            p("desc", &[], vec![t("text", &[("name", "vMandatory")], "1"), t("text", &[("name", "title")], "Title")]),
            p(
                "extras",
                &[],
                vec![
                    t("text", &[("name", "note")], "N"),
                    p("extras", &[("name", "arrayGroup")], vec![t("text", &[("name", "groupName")], "A")]),
                    p("extras", &[("name", "arrayGroup")], vec![t("text", &[("name", "groupName")], "B")]),
                ],
            ),
            p("bind", &[("ref", "$.a")], vec![]),
        ],
    );
    let plain = node("plain", XfaKind::Field, &[("name", "plain")], vec![]);
    let label = node("label", XfaKind::Draw, &[], vec![p("ui", &[], vec![p("textEdit", &[], vec![])])]);
    let rows = XfaNode {
        repeatable: true,
        occur_max: None,
        ..node(
            "rows",
            XfaKind::Subform,
            &[],
            vec![p("occur", &[("min", "0"), ("max", "-1")], vec![]), p("breakBefore", &[("targetType", "pageArea")], vec![])],
        )
    };
    XfaNode {
        name: "form1".into(),
        som: "form1[0]".into(),
        kind: Some(XfaKind::Subform),
        children: vec![styled, plain, label, rows],
        ..Default::default()
    }
}

fn go_on(root: &XfaNode, script: &str, target: &str) -> XfaOutcome {
    let ev = XfaEvent { activity: "click".into(), target: format!("form1[0].{target}[0]"), ..Default::default() };
    run_xfa(script, &ev, &XfaDoc::default(), root, Limits::default())
}

/// The script's result, failing the test on a script error.
fn eval(script: &str, target: &str) -> String {
    let o = go_on(&form(), script, target);
    assert_eq!(o.error, None, "{script}: {o:?}");
    o.result.unwrap_or_default()
}

/// Each property the template leaves out reads as the default XFA 3.3 ch. 17 gives it, and is
/// not specified.
#[test]
fn unspecified_properties_read_as_the_spec_defaults() {
    for (expr, want) in [
        ("this.border.presence", "hidden"),
        ("this.border.edge.presence", "visible"),
        ("this.border.edge.color.value", "0,0,0"),
        ("this.border.getElement('corner', 0).presence", "visible"),
        ("this.border.corner.color.value", "0,0,0"),
        ("this.border.fill.presence", "visible"),
        ("this.border.fill.color.value", "255,255,255"),
        ("this.border.fill.oneOfChild.className", "solid"),
        ("this.font.fill.color.value", "0,0,0"),
        ("this.font.typeface", "Courier"),
        ("this.font.weight", "normal"),
        ("this.para.hAlign", "left"),
        ("this.para.vAlign", "top"),
        ("this.ui.oneOfChild.className", "textEdit"),
        ("this.validate.nullTest", "disabled"),
        ("this.validate.formatTest", "warning"),
        ("this.validate.scriptTest", "error"),
        ("this.occur.min", "1"),
        ("this.occur.max", "1"),
        ("this.occur.initial", "1"),
        ("this.assist.toolTip.value", ""),
        ("this.format.picture.value", ""),
        ("String(this.isPropertySpecified('border'))", "false"),
        ("String(this.border.isPropertySpecified('presence'))", "false"),
        ("String(this.validate.isPropertySpecified('nullTest'))", "false"),
        // Attributes without a default here read as an empty value that never throws.
        ("this.border.edge.thickness", "0.5pt"),
        ("String(this.para.radixOffset)", ""),
        ("String(this.para.radixOffset == '0in')", "false"),
        ("String(this.para.radixOffset.isPropertySpecified('x'))", "false"),
        ("String(this.breakBefore.targetType) + this.font.posture.value", ""),
    ] {
        assert_eq!(eval(expr, "plain"), want, "{expr}");
    }
}

/// What the template writes is what scripts read, and is specified.
#[test]
fn specified_properties_read_as_the_template_writes_them() {
    for (expr, want) in [
        ("this.border.presence", "hidden"),
        ("this.border.edge.color.value", "255,0,0"),
        ("this.border.edge.thickness", "2pt"),
        ("this.border.fill.color.value", "1,2,3"),
        ("this.border.fill.oneOfChild.className", "linear"),
        ("this.border.fill.oneOfChild.type", "toTop"),
        ("this.ui.oneOfChild.className", "choiceList"),
        ("this.ui.oneOfChild.open", "always"),
        ("this.validate.nullTest", "error"),
        ("this.validate.message.text.value", "Required"),
        ("this.validate.message.getElement('text', 1).value", "Too big"),
        ("this.validate.message.nodes.length", "2"),
        ("this.validate.message.nodes.item(1).name", "scriptTest"),
        ("this.validate.message.nodes.namedItem('text').value", "Required"),
        ("this.font.typeface", "Arial"),
        ("this.assist.toolTip.value", "Pick one"),
        ("this.format.picture.value", "num{z9}"),
        ("String(this.format.picture)", "num{z9}"),
        ("this.getElement('value', 0).oneOfChild.className", "decimal"),
        ("String(this.isPropertySpecified('border'))", "true"),
        ("String(this.isPropertySpecified('caption'))", "false"),
        ("String(this.isPropertySpecified('w'))", "true"),
        ("String(this.border.isPropertySpecified('presence'))", "true"),
        ("String(this.border.isPropertySpecified('break'))", "false"),
        ("String(this.border.isPropertySpecified('fill'))", "true"),
        ("String(this.border.isPropertySpecified('corner'))", "false"),
        ("this.getAttribute('w')", "2in"),
        ("this.getAttribute('name')", "styled"),
        ("this.getAttribute('presence')", "visible"),
        ("this.getAttribute('nothing')", ""),
        ("this.border.getAttribute('presence')", "hidden"),
        ("this.border.parent.className", "field"),
        ("this.border.edge.parent.className", "border"),
    ] {
        assert_eq!(eval(expr, "styled"), want, "{expr}");
    }
    assert_eq!(eval("this.occur.min + ',' + this.occur.max", "rows"), "0,-1");
    assert_eq!(eval("this.breakBefore.targetType", "rows"), "pageArea");
}

/// `getElement(name, index)` reaches each occurrence the template lists, up to what the spec
/// allows (four edges), and nothing beyond. An edge or corner past those listed reads as the
/// one drawn on that side (as readers draw two edges 0, 1, 0, 1, three 0, 1, 2, 1 and one on
/// every side), comes into being as a copy of it when reached, and a write then changes only
/// it; with none listed, each is a default one (edge p. 687, corner p. 663).
#[test]
fn get_element_reaches_occurrences_by_index() {
    assert_eq!(eval("this.border.getElement('edge', 0).color.value", "styled"), "255,0,0");
    assert_eq!(eval("this.border.getElement('edge', 1).color.value", "styled"), "0,0,255");
    assert_eq!(eval("this.border.getElement('edge').color.value", "styled"), "255,0,0");
    let edge3 = "var e = this.border.getElement('edge', 3); [e.color.value, e.thickness, e.presence, e.stroke, e.cap].join('|')";
    assert_eq!(eval(edge3, "styled"), "0,0,255|2pt|visible|solid|square");
    assert_eq!(eval("this.border.resolveNode('edge[2]').color.value", "styled"), "255,0,0");
    assert_eq!(eval("this.border.getElement('edge', 3).somExpression", "styled"), "form1[0].styled[0].border[0].edge[3]");
    let corner2 = "var c = this.border.getElement('corner', 2); \
                   [c.color.value, c.thickness, c.presence, c.stroke, c.radius, c.join, c.inverted].join('|')";
    assert_eq!(eval(corner2, "styled"), "0,0,0|0.5pt|visible|solid|0in|square|0");
    assert_eq!(eval("this.border.getElement('edge', 3).color.value", "plain"), "0,0,0");
    // A write to an edge past those listed changes only it.
    let all = "[0, 1, 2, 3].map(function (i) { return this.border.getElement('edge', i).color.value; }, this).join('|')";
    assert_eq!(eval(&format!("this.border.getElement('edge', 3).color.value = '9,9,9'; {all}"), "styled"), "255,0,0|0,0,255|255,0,0|9,9,9");
    assert_eq!(eval("this.border.getElement('edge', 2).thickness = '3pt'; this.border.getElement('edge', 0).thickness", "styled"), "2pt");
    // Reached, it is a copy: a later write to the edge it was drawn from leaves it.
    let copy = "var e = this.border.getElement('edge', 3); this.border.getElement('edge', 1).color.value = '5,5,5'; e.color.value";
    assert_eq!(eval(copy, "styled"), "0,0,255");
    // With no edge listed, each index is a default edge of its own.
    assert_eq!(eval("this.border.getElement('edge', 2).color.value = '1,1,1'; this.border.edge.color.value", "plain"), "0,0,0");
    // One edge listed is drawn on every side; three reuse the second for the fourth.
    let edges = |colors: &[&str]| {
        let listed = colors.iter().map(|c| p("edge", &[], vec![p("color", &[("value", c)], vec![])])).collect();
        let f = node("e", XfaKind::Field, &[], vec![p("border", &[], listed)]);
        let mut root = form();
        root.children.push(f);
        let o = go_on(&root, all, "e");
        assert_eq!(o.error, None, "{o:?}");
        o.result.unwrap_or_default()
    };
    assert_eq!(edges(&["1,1,1"]), "1,1,1|1,1,1|1,1,1|1,1,1");
    assert_eq!(edges(&["1,1,1", "2,2,2", "3,3,3"]), "1,1,1|2,2,2|3,3,3|2,2,2");
    for gone in ["this.border.getElement('edge', 4)", "this.border.getElement('edge', -1)", "this.border.getElement('nothing', 0)"] {
        assert_eq!(eval(&format!("String({gone})"), "styled"), "null", "{gone}");
    }
    assert_eq!(eval("this.getElement('border', 0).presence", "styled"), "hidden");
}

/// Each widget is the `ui`'s one-of child, by its class name.
#[test]
fn one_of_child_names_each_widget() {
    for widget in [
        "barcode",
        "button",
        "checkButton",
        "choiceList",
        "dateTimeEdit",
        "defaultUi",
        "imageEdit",
        "numericEdit",
        "passwordEdit",
        "signature",
        "textEdit",
    ] {
        let f = node("w", XfaKind::Field, &[], vec![p("ui", &[], vec![p("picture", &[], vec![]), p(widget, &[], vec![])])]);
        let root = XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), children: vec![f], ..Default::default() };
        let o = go_on(&root, &format!("this.ui.oneOfChild.className == '{widget}'"), "w");
        assert_eq!((o.error.as_deref(), o.result.as_deref()), (None, Some("true")), "{widget}");
    }
    assert_eq!(eval("this.ui.oneOfChild.className == 'choiceList'", "styled"), "true");
    assert_eq!(eval("this.ui.oneOfChild.className == 'choiceList'", "plain"), "false");
}

/// script-errors C04a: the forms' reads through widgets, edges and validations compare as
/// strings and throw nothing, whether the template writes the property or not.
#[test]
fn widget_border_and_validate_reads_throw_no_type_error() {
    for target in ["styled", "plain", "label"] {
        let script = "var c = this.ui.oneOfChild.border.getElement('edge', 0).color.value; \
                      var n = this.validate.nullTest == 'error'; \
                      var w = this.ui.oneOfChild.className == 'choiceList'; \
                      c + '|' + n + '|' + w";
        let o = go_on(&form(), script, target);
        assert_eq!(o.error, None, "{target}: {o:?}");
        let want = match target {
            "styled" => "9,9,9|true|true",
            _ => "0,0,0|false|false",
        };
        assert_eq!(o.result.as_deref(), Some(want), "{target}");
    }
}

/// A script's writes are read back later in the same run, make the property specified, and
/// change nothing on the form (no effect comes back).
#[test]
fn writes_are_read_back_in_the_run_and_have_no_effect() {
    let script = "this.border.fill.color.value = '255,0,0'; \
                  this.validate.nullTest = 'error'; \
                  this.assist.toolTip.value = 'Now'; \
                  this.font.setAttribute('bold', 'weight'); \
                  [this.border.fill.color.value, this.validate.nullTest, this.assist.toolTip.value, this.font.weight, \
                   this.isPropertySpecified('border'), this.border.isPropertySpecified('fill'), this.validate.isPropertySpecified('nullTest'), \
                   this.font.getAttribute('weight')].join('|')";
    let o = go_on(&form(), script, "plain");
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("255,0,0|error|Now|bold|true|true|true|bold"));
    assert!(o.effects.is_empty(), "{o:?}");
    // Another object is untouched, and a fresh run starts from the template again.
    assert_eq!(eval("this.border.fill.color.value", "label"), "255,255,255");
    assert_eq!(eval("this.validate.nullTest", "plain"), "disabled");
}

/// form1 > { label (a field captioned "Name:"), rich (a field whose caption is rich text),
/// list (a choice list with shown and saved items), box (a check button with on and off values) }
fn caption_form() -> XfaNode {
    let caption =
        |content: XfaProp| p("caption", &[("reserve", "1in")], vec![p("value", &[], vec![content]), p("font", &[("weight", "bold")], vec![])]);
    let items = |attrs: &[(&str, &str)], tag: &str, entries: &[&str]| p("items", attrs, entries.iter().map(|e| t(tag, &[], e)).collect());
    let fields = vec![
        node("label", XfaKind::Field, &[], vec![caption(t("text", &[], "Name:"))]),
        node("rich", XfaKind::Field, &[], vec![caption(t("exData", &[("contentType", "text/html")], "Date of birth:"))]),
        node("list", XfaKind::Field, &[], vec![items(&[], "text", &["A", "B", "C"]), items(&[("save", "1")], "text", &["1", "2", "3"])]),
        node("box", XfaKind::Field, &[], vec![items(&[], "integer", &["1", "0"])]),
        node("bare", XfaKind::Field, &[], vec![]),
    ];
    XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), children: fields, ..Default::default() }
}

fn eval_on(root: &XfaNode, script: &str, target: &str) -> String {
    let o = go_on(root, script, target);
    assert_eq!(o.error, None, "{script}: {o:?}");
    o.result.unwrap_or_default()
}

/// A caption is a property object (XFA 3.3 p. 643): its value element holds the label, as text
/// or rich text, read by name, by class, through `resolveNode` and `oneOfChild`; the content it
/// doesn't hold reads as null (one of them, p. 598); its rich text reads as its words.
#[test]
fn a_caption_reads_its_label_as_text_or_rich_text() {
    let root = caption_form();
    for (expr, want) in [
        ("this.caption.value.text.value", "Name:"),
        ("this.caption.value['#text'].value", "Name:"),
        ("this.caption.value.resolveNode('#text').value", "Name:"),
        ("this.caption.value.oneOfChild.className", "text"),
        ("xfa.resolveNode('xfa.form.form1.label.caption.value.oneOfChild').value", "Name:"),
        ("String(this.caption.value.exData)", "null"),
        ("String(this.caption.value.resolveNode('#exData'))", "null"),
        ("this.caption.value.text.value.substr(0, 4) + this.caption.value.text.value.indexOf(':')", "Name4"),
        (
            "this.caption.reserve + '|' + this.caption.placement + '|' + this.caption.presence + '|' + this.caption.font.weight",
            "1in|left|visible|bold",
        ),
        ("this.caption.className + '|' + this.caption.value.className", "caption|value"),
    ] {
        assert_eq!(eval_on(&root, expr, "label"), want, "{expr}");
    }
    for (expr, want) in [
        ("this.caption.value.exData.value", "Date of birth:"),
        ("this.caption.value['#exData'].value.length", "14"),
        ("this.caption.value.resolveNode('#exData').value.indexOf(':')", "13"),
        ("this.caption.value.resolveNode('exData').value.replace(':', '* :')", "Date of birth* :"),
        ("this.caption.value.oneOfChild.className", "exData"),
        ("String(this.caption.value.text)", "null"),
        ("(this.caption.value.exData) ? this.caption.value.exData.value : this.caption.value.text.value", "Date of birth:"),
    ] {
        assert_eq!(eval_on(&root, expr, "rich"), want, "{expr}");
    }
    // Its methods to load and save rich text are not modelled yet: calling them changes nothing
    // and throws nothing.
    let o = go_on(&root, "this.caption.value.exData.loadXML('<body/>', false, true); typeof this.caption.value.exData.saveXML('pretty')", "rich");
    assert_eq!((o.error, o.result.as_deref(), o.effects.len()), (None, Some("string"), 0));
    // A caption the template doesn't give reads its defaults; a write reads back in the run.
    assert_eq!(eval_on(&root, "this.caption.placement + '|' + this.caption.value.oneOfChild.className", "bare"), "left|text");
    assert_eq!(eval_on(&root, "this.caption.value.text.value = 'Surname:'; this.caption.value.text.value", "label"), "Surname:");
}

/// A field's items are a property object (XFA 3.3 p. 758) whose entries `nodes` lists, each
/// reading as the list or check button shows it; a write reads back in the run.
#[test]
fn items_list_their_entries_as_the_list_shows_them() {
    let root = caption_form();
    for (expr, want) in [
        ("this.items.nodes.length", "3"),
        ("this.items.nodes.item(1).value", "B"),
        ("this.items.nodes.item(2).className", "text"),
        ("String(this.items.nodes.item.value)", "undefined"),
        ("this.getElement('items', 1).save + '|' + this.getElement('items', 1).nodes.item(0).value", "1|1"),
        // The first `<items>` is the shown list; the second is reached by index.
        ("typeof this.items.save + ':' + this.items.save", "string:0"),
        ("this.resolveNode('items[1]').nodes.item(1).value", "2"),
        ("this.resolveNodes('items[*]').length", "2"),
    ] {
        assert_eq!(eval_on(&root, expr, "list"), want, "{expr}");
    }
    assert_eq!(eval_on(&root, "this.items.nodes.item(0).value + '|' + this.items.nodes.item(1).value", "box"), "1|0");
    let script = "if (this.items.nodes.item(0).value == '1') this.items.nodes.item(0).value = 'On'; this.items.nodes.item(0).value";
    assert_eq!(eval_on(&root, script, "box"), "On");
    // A list without `<items>`: an empty items object, no error.
    assert_eq!(eval_on(&root, "this.items.nodes.length + '|' + this.resolveNodes('items').length", "bare"), "0|1");
}

/// The items reads come from the list state (`HNode::items`) once one is set: the hook for
/// PdfCraft's choice-list methods (`addItem`, `clearItems`; the Series 1 work, not on main yet).
/// Nothing sets it in the product yet, so this test sets it directly.
#[test]
fn items_read_the_list_state_once_one_is_set() {
    use super::{ItemList, content, list_entries, occurrences};
    let doc = XfaDoc::default();
    let mut h = super::super::XHost::new(caption_form(), "form1[0].list[0]", &doc);
    let list = h.current;
    assert_eq!(list_entries(&h, list, 0).len(), 3, "as the template lists them");
    let entries = vec![("text".to_string(), "One".to_string()), ("text".to_string(), "Two".to_string())];
    if let Some(n) = h.nodes.get_mut(list) {
        n.items = Some(vec![ItemList { entries }]);
    }
    assert_eq!(list_entries(&h, list, 0).len(), 2);
    assert_eq!(content(&h, list, "items#0/text#1"), "Two");
    assert_eq!(occurrences(&h, list, "", "items"), 1);
    // A list that had no `<items>` and got one entry.
    let mut h = super::super::XHost::new(caption_form(), "form1[0].bare[0]", &doc);
    let bare = h.current;
    assert_eq!(list_entries(&h, bare, 0).len(), 0);
    if let Some(n) = h.nodes.get_mut(bare) {
        n.items = Some(vec![ItemList { entries: vec![("text".to_string(), "X".to_string())] }]);
    }
    assert_eq!((list_entries(&h, bare, 0).len(), content(&h, bare, "items#0/text#0")), (1, "X".to_string()));
}

/// A script can't grow the run past its limits: paths deeper than [`super::MAX_PATH`] take
/// anything (a template nested deeper than the parser keeps), and one property keeps at most
/// [`super::MAX_WRITTEN_ATTRS`] written attributes, with a note.
#[test]
fn deep_paths_and_many_writes_stay_bounded() {
    let mut deep = p("x", &[], Vec::new());
    for _ in 0..20 {
        deep = p("x", &[], vec![deep]);
    }
    let f = node("deep", XfaKind::Field, &[], vec![deep]);
    let root = XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), children: vec![f], ..Default::default() };
    let script = "var o = this.getElement('x', 0); var tags = []; for (var i = 0; i < 12; i++) { tags.push(String(o.className)); o.y = 1; o = o.x; } tags.join(',')";
    let o = go_on(&root, script, "deep");
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("x,x,x,x,x,x,x,x,,,,"));
    let script = "for (var i = 0; i < 1000; i++) { this.font.setAttribute('v' + i, 'a' + i); } this.font.getAttribute('a63') + '|' + this.font.getAttribute('a64')";
    let o = go_on(&form(), script, "plain");
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("v63|"));
    assert!(o.notes.iter().any(|n| n.contains("936 more")), "{o:?}");
}

/// The other reads forms make: children by name (`desc.vMandatory`, a message's `scriptTest`
/// text), properties by class (`ui.#textEdit`), `resolveNode` from a property, `somExpression`,
/// `nodes.namedItem` by name, and a node's `mandatory` and validation messages, with their
/// defaults where the template has none.
#[test]
fn named_children_class_names_and_validation_members_read_as_forms_use_them() {
    for (expr, want) in [
        ("this.desc.vMandatory.value", "1"),
        ("this.desc.nodes.namedItem('title').value", "Title"),
        // A name written in the run is the one `namedItem` finds.
        (
            "this.desc.nodes.namedItem('title').setAttribute('heading', 'name'), String(this.desc.nodes.namedItem('title')) + '|' + this.desc.nodes.namedItem('heading').value",
            "null|Title",
        ),
        ("this.extras.nodes.namedItem('note').value", "N"),
        ("this.extras.note.value", "N"),
        ("this.bind.ref", "$.a"),
        ("this.validate.message.scriptTest.value", "Too big"),
        ("this.validate.message.nullTest.value", "Required"),
        ("this.ui['#choiceList'].open", "always"),
        ("this.ui.resolveNode('#choiceList').className", "choiceList"),
        ("this.ui.resolveNode('#choiceList.border.edge').color.value", "9,9,9"),
        ("this.border.resolveNodes('edge[*]').length", "2"),
        ("String(this.ui.resolveNode('#textEdit'))", "null"),
        ("this.ui.nodes.item(0).className", "choiceList"),
        ("this.border.edge.somExpression", "form1[0].styled[0].border[0].edge[0]"),
        ("this.mandatory", "error"),
        ("this.validationMessage", "Too big"),
        ("this.mandatoryMessage", "Required"),
        ("this.formatMessage", ""),
        ("this.validate.script.value", "this.rawValue > 0"),
        // SOM expressions from a node that run into its properties.
        ("this.resolveNode('ui.#choiceList').className", "choiceList"),
        ("this.resolveNode('border.edge[1]').color.value", "0,0,255"),
        ("xfa.resolveNode('xfa.form.form1.styled.border.edge[0].color').value", "255,0,0"),
        ("xfa.form.form1.resolveNode('styled.ui.#choiceList.border.edge').color.value", "9,9,9"),
        ("this.resolveNodes('border.edge[*]').length", "2"),
        ("String(this.resolveNode('nothing.edge'))", "null"),
        ("String(this.resolveNode('ui.#textEdit'))", "null"),
        ("this.resolveNodes('extras.arrayGroup[*].groupName').length", "2"),
        ("this.resolveNodes('extras.arrayGroup[*].groupName').item(1).value", "B"),
        ("this.resolveNode('ui.*.border.edge').color.value", "9,9,9"),
        ("this.fillColor + '|' + this.borderColor + '|' + this.fontColor", "1,2,3|255,0,0|0,0,0"),
        ("this.fillColor = '255,0,0'; this.fillColor + '|' + this.border.fill.color.value", "255,0,0|255,0,0"),
        ("this.desc.rawValue", "1"),
        ("this.validate.message.text.rawValue", "Required"),
    ] {
        assert_eq!(eval(expr, "styled"), want, "{expr}");
    }
    for (expr, want) in [
        ("this.mandatory", "disabled"),
        ("this.validationMessage", ""),
        ("this.validationMessage.split('x').length", "1"),
        ("this.validate.script.value", ""),
        ("this.desc.rawValue", ""),
        ("this.fillColor + '|' + this.borderColor + '|' + this.fontColor", "255,255,255|0,0,0|0,0,0"),
        ("this.margin.leftInset", "0in"),
        ("this.break.after", "auto"),
        ("this.ui.textEdit.comb.numberOfCells", "0"),
        ("this.ui['#textEdit'].border.fill.color.value", "255,255,255"),
        ("this.bind.ref", ""),
        ("String(this.desc.vMandatory.value)", ""),
        ("this.border.fill.oneOfChild.color.value", ""),
    ] {
        assert_eq!(eval(expr, "plain"), want, "{expr}");
    }
    // A gradient without a type runs to the right (XFA 3.3 p. 770).
    let f = node("g", XfaKind::Field, &[], vec![p("border", &[], vec![p("fill", &[], vec![p("linear", &[], vec![])])])]);
    let root = XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), children: vec![f], ..Default::default() };
    let o = go_on(&root, "this.border.fill.oneOfChild.type + '|' + this.border.fill.oneOfChild.color.value", "g");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("toRight|0,0,0")));
    // Writes are read back in the run, and only a known nullTest is taken.
    let o = go_on(
        &form(),
        "this.mandatory = 'error'; this.validationMessage = 'Bad'; var a = this.mandatory + this.validate.nullTest + this.validationMessage; this.mandatory = 'nonsense'; a + this.mandatory",
        "plain",
    );
    assert_eq!((o.error, o.result.as_deref(), o.effects.len()), (None, Some("errorerrorBaderror"), 0));
}

/// A validation message set through the node is the message's text of that name, read back
/// both ways; `rawValue` set on a property reads back where it is read from.
#[test]
fn messages_and_raw_values_written_read_back_from_their_property() {
    for (target, want) in [("plain", "Bad|Bad|Soon"), ("styled", "Bad|Bad|Soon")] {
        let script = "this.validationMessage = 'Bad'; this.mandatoryMessage = 'Soon'; \
                      [this.validationMessage, this.validate.message.scriptTest.value, this.validate.message.nullTest.value].join('|')";
        let o = go_on(&form(), script, target);
        assert_eq!((o.error, o.result.as_deref()), (None, Some(want)), "{target}");
    }
    assert_eq!(eval("this.validate.message.scriptTest.value = 'Mine'; this.validationMessage", "styled"), "Mine");
    for (expr, want) in [
        ("this.desc.rawValue = 'Z'; this.desc.rawValue + this.desc.vMandatory.value", "ZZ"),
        ("this.border.fill.color.rawValue = '5,5,5'; this.border.fill.color.value + '|' + this.border.fill.color.rawValue", "5,5,5|5,5,5"),
        ("this.assist.toolTip.rawValue = 'T'; this.assist.toolTip.value", "T"),
    ] {
        assert_eq!(eval(expr, "styled"), want, "{expr}");
    }
}

/// A child named like a property comes first, for every property name, as XFA SOM resolves
/// name clashes in favour of the explicitly named object; `#name` reaches the property (XFA
/// 3.3 p. 99).
#[test]
fn a_child_named_like_a_property_comes_first() {
    let mut parent = node("box", XfaKind::Subform, &[], vec![p("border", &[("presence", "hidden")], vec![]), p("occur", &[("min", "2")], vec![])]);
    parent.children = ["format", "desc", "border", "occur", "mandatory"]
        .iter()
        .map(|n| XfaNode { som: format!("form1[0].box[0].{n}[0]"), ..node(n, XfaKind::Field, &[], vec![]) })
        .collect();
    let root = XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), children: vec![parent], ..Default::default() };
    let script = "[this.format.className, this.desc.className, this.border.className, this.occur.className, this.mandatory.className, \
                  this['#format'].className, this['#border'].presence, this['#occur'].min].join('|')";
    let o = go_on(&root, script, "box");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("field|field|field|field|field|format|hidden|2")));
}

/// `resolveNodes` that runs through every instance into a property gives each instance's.
#[test]
fn resolve_nodes_through_instances_gives_each_instances_property() {
    let row = |i: usize, rgb: &str| XfaNode {
        som: format!("form1[0].row[{i}]"),
        index: i,
        repeatable: true,
        ..node("row", XfaKind::Subform, &[], vec![p("border", &[], vec![p("fill", &[], vec![p("color", &[("value", rgb)], vec![])])])])
    };
    let root = XfaNode {
        name: "form1".into(),
        som: "form1[0]".into(),
        kind: Some(XfaKind::Subform),
        children: vec![row(0, "1,1,1"), row(1, "2,2,2")],
        ..Default::default()
    };
    let script = "var l = xfa.resolveNodes('xfa.form.form1.row[*].border.fill.color'); l.length + '|' + l.item(0).value + '|' + l.item(1).value";
    let o = go_on(&root, script, "row");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("2|1,1,1|2,2,2")));
}

/// A child named as a property comes first for each instance on its own: with a field `border`
/// in the first row and only the border property in the second, `row[*].border` lists the
/// field and then the second row's border.
#[test]
fn resolve_nodes_takes_a_child_over_a_property_instance_by_instance() {
    let row = |i: usize, children: Vec<XfaNode>| XfaNode {
        som: format!("form1[0].row[{i}]"),
        index: i,
        repeatable: true,
        children,
        ..node("row", XfaKind::Subform, &[], vec![p("border", &[("presence", "hidden")], vec![])])
    };
    let field = XfaNode { som: "form1[0].row[0].border[0]".into(), ..node("border", XfaKind::Field, &[], vec![]) };
    let root = XfaNode {
        name: "form1".into(),
        som: "form1[0]".into(),
        kind: Some(XfaKind::Subform),
        children: vec![row(0, vec![field]), row(1, Vec::new())],
        ..Default::default()
    };
    let script = "var l = xfa.resolveNodes('xfa.form.form1.row[*].border'); \
                  [l.length, l.item(0).className, l.item(1).className, l.item(1).presence, l.namedItem('border').className].join('|')";
    let o = go_on(&root, script, "row");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("2|field|border|hidden|field")));
}

/// A child named as a property shadows that property for the rest of the expression too
/// (`row.border.edge[0]` with a field `border`): the rest is resolved from the child, and when the
/// child has no such property the expression names nothing; it does not fall back to the parent's
/// property. `#border` names the property by class (XFA 3.3 p. 96), so it still reaches it.
#[test]
fn a_child_named_as_a_property_shadows_it_for_the_rest_of_the_expression() {
    let edges = |presence: &str| vec![p("edge", &[("presence", presence)], vec![])];
    let field =
        XfaNode { som: "form1[0].row[0].border[0]".into(), ..node("border", XfaKind::Field, &[], vec![p("border", &[], edges("invisible"))]) };
    let row = XfaNode {
        som: "form1[0].row[0]".into(),
        children: vec![field],
        ..node("row", XfaKind::Subform, &[], vec![p("border", &[], edges("hidden"))])
    };
    let root = XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), children: vec![row], ..Default::default() };
    let script = "var r = function (s) { var o = xfa.resolveNode('xfa.form.form1.row.' + s); return o === null ? 'null' : o.presence; }; \
                  [r('border.border.edge[0]'), r('border.edge[0]'), r('#border.edge[0]')].join('|')";
    let o = go_on(&root, script, "row");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("invisible|null|hidden")));
}

/// `keep` reads as the template writes it, else with its defaults (XFA 3.3 pp. 762-763):
/// `intact` is `contentArea` for a draw and a positioned or row subform, `none` for a field and
/// a flowing subform; `next` and `previous` are `none`. A write reads back in the run.
#[test]
fn keep_reads_its_context_dependent_defaults_and_writes_read_back() {
    let kept = node("kept", XfaKind::Field, &[], vec![p("keep", &[("intact", "pageArea"), ("next", "contentArea")], vec![])]);
    let flowing = node("flowing", XfaKind::Subform, &[("layout", "tb")], vec![]);
    let row = node("arow", XfaKind::Subform, &[("layout", "row")], vec![]);
    let mut root = form();
    root.children.extend([kept, flowing, row]);
    let read = |target: &str, script: &str| {
        let o = go_on(&root, script, target);
        assert_eq!(o.error, None, "{target}: {o:?}");
        o.result.unwrap_or_default()
    };
    let all = "[this.keep.intact, this.keep.next, this.keep.previous, this.keep.isPropertySpecified('intact')].join('|')";
    for (target, want) in [
        ("plain", "none|none|none|false"),
        ("label", "contentArea|none|none|false"),
        ("rows", "contentArea|none|none|false"),
        ("flowing", "none|none|none|false"),
        ("arow", "contentArea|none|none|false"),
        ("kept", "pageArea|contentArea|none|true"),
    ] {
        assert_eq!(read(target, all), want, "{target}");
    }
    assert_eq!(
        read("plain", "this.keep.intact = 'contentArea'; this.keep.intact + '|' + this.keep.isPropertySpecified('intact')"),
        "contentArea|true"
    );
}

/// A single unnamed message text is every validation's message (XFA 3.3 p. 782), and setting
/// a test's message overwrites it, as readers do; with several texts and none named for the
/// test, a text of that name is added. A test's text that doesn't exist can't be read: readers
/// raise an error, which a script can catch.
#[test]
fn a_single_unnamed_message_text_serves_every_validation() {
    let f = node("one", XfaKind::Field, &[], vec![p("validate", &[], vec![p("message", &[], vec![t("text", &[], "Fix it")])])]);
    let two = node(
        "two",
        XfaKind::Field,
        &[],
        vec![p("validate", &[], vec![p("message", &[], vec![t("text", &[("name", "nullTest")], "N"), t("text", &[], "U")])])],
    );
    let mut root = form();
    root.children.extend([f, two]);
    let o = go_on(&root, "[this.validationMessage, this.formatMessage, this.mandatoryMessage].join('|')", "one");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("Fix it|Fix it|Fix it")));
    let script = "this.validationMessage = 'New'; \
                  [this.validationMessage, this.formatMessage, this.mandatoryMessage, this.validate.message.nodes.length, this.validate.message.text.value].join('|')";
    let o = go_on(&root, script, "one");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("New|New|New|1|New")));
    let o = go_on(&root, "try { this.validate.message.scriptTest.value } catch (e) { 'caught ' + (e instanceof TypeError) }", "one");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("caught true")));
    let o = go_on(
        &root,
        "this.validationMessage = 'S'; [this.validationMessage, this.mandatoryMessage, this.validate.message.scriptTest.value].join('|')",
        "two",
    );
    assert_eq!((o.error, o.result.as_deref()), (None, Some("S|N|S")));
    // A name a script gives the text counts: it no longer serves every test.
    let o = go_on(&root, "this.validate.message.text.name = 'scriptTest'; this.validationMessage + '|' + this.mandatoryMessage", "one");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("Fix it|")));
}

/// A write to a key that names a child sets that child's value, as a read gets the child.
#[test]
fn a_write_to_a_named_child_sets_its_value() {
    let f = node("named", XfaKind::Field, &[], vec![p("extras", &[("id", "attr")], vec![t("text", &[("name", "id")], "X")])]);
    let mut root = form();
    root.children.push(f);
    let o = go_on(&root, "this.extras.id = 'Y'; this.extras.id.value + '|' + this.extras.getAttribute('id')", "named");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("Y|attr")));
}

/// A property's child of a name comes before an attribute or a property of that name, as for
/// nodes (XFA 3.3 p. 99), and a node's child named like a member not modelled yet comes before
/// the member.
#[test]
fn named_children_come_before_attributes_and_unmodelled_members() {
    let f = node(
        "named",
        XfaKind::Field,
        &[],
        vec![
            p("extras", &[("id", "attr")], vec![t("text", &[("name", "id")], "X")]),
            p("validate", &[], vec![p("message", &[], vec![t("text", &[("name", "scriptTest")], "A"), t("text", &[("name", "text")], "B")])]),
        ],
    );
    let mut parent = node("box", XfaKind::Subform, &[], vec![]);
    parent.children = ["items", "event", "locale"]
        .iter()
        .map(|n| XfaNode { som: format!("form1[0].box[0].{n}[0]"), ..node(n, XfaKind::Field, &[], vec![]) })
        .collect();
    let mut root = form();
    root.children.extend([f, parent]);
    let o = go_on(&root, "this.extras.id.value + '|' + this.extras.getAttribute('id') + '|' + this.validate.message.text.value", "named");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("X|attr|B")));
    let o = go_on(&root, "[this.items.className, this.event.className, this.locale.className].join('|')", "box");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("field|field|field")));
    // Without such a child the name reaches the property: a field with no `<items>` reads an
    // items object with no entries.
    let o = go_on(&root, "this.items.className + '|' + this.items.nodes.length", "plain");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("items|0")));
}

/// `borderColor` set colours all four edges, the ones not listed made first as copies of the
/// edge drawn there. Read, it is edge 0's.
#[test]
fn border_color_colours_all_four_edges() {
    let all = "[0, 1, 2, 3].map(function (i) { return this.border.getElement('edge', i).color.value; }, this).concat([this.borderColor]).join('|')";
    for target in ["styled", "plain"] {
        assert_eq!(eval(&format!("this.borderColor = '7,7,7'; {all}"), target), "7,7,7|7,7,7|7,7,7|7,7,7|7,7,7", "{target}");
    }
    // The edges made are copies of those drawn there: only their colour changed.
    assert_eq!(eval("this.borderColor = '7,7,7'; this.border.getElement('edge', 3).thickness", "styled"), "2pt");
}

/// A corner past those a border lists reads as the corner drawn there: one serves all four,
/// two alternate, three reuse the second for the fourth, as for edges.
#[test]
fn corners_not_listed_read_as_the_corner_drawn_there() {
    let corners = |listed: &[&str]| {
        let kids = listed.iter().map(|r| p("corner", &[("radius", r)], vec![])).collect();
        let f = node("c", XfaKind::Field, &[], vec![p("border", &[], kids)]);
        let mut root = form();
        root.children.push(f);
        let o = go_on(&root, "[0, 1, 2, 3].map(function (i) { return this.border.getElement('corner', i).radius; }, this).join('|')", "c");
        assert_eq!(o.error, None, "{o:?}");
        o.result.unwrap_or_default()
    };
    assert_eq!(corners(&["1pt"]), "1pt|1pt|1pt|1pt");
    assert_eq!(corners(&["1pt", "2pt"]), "1pt|2pt|1pt|2pt");
    assert_eq!(corners(&["1pt", "2pt", "3pt"]), "1pt|2pt|3pt|2pt");
}

/// A border's presence reads `hidden` when the object has no `<border>` element, as readers
/// read it, and `visible` when it has one that doesn't say (XFA 3.3 p. 629).
#[test]
fn border_presence_reads_hidden_without_a_border_element() {
    let f = node("bordered", XfaKind::Field, &[], vec![p("border", &[], vec![])]);
    let mut root = form();
    root.children.push(f);
    for (target, want) in [("plain", "hidden"), ("label", "hidden"), ("bordered", "visible"), ("styled", "hidden")] {
        let o = go_on(&root, "this.border.presence", target);
        assert_eq!((o.error, o.result.as_deref()), (None, Some(want)), "{target}");
    }
}

/// A border has four sides (edges and corners occur at most four times, XFA 3.3 p. 629): an
/// index past them is no element, from a script or from inside the host, and nothing is made
/// for it.
#[test]
fn edges_and_corners_past_the_fourth_are_none() {
    for script in [
        "String(this.border.getElement('edge', 7))",
        "String(this.border.resolveNode('edge[7]'))",
        "String(this.border.resolveNode('corner[5]'))",
        "String(xfa.resolveNode('xfa.form.form1.styled.border.edge[4]'))",
    ] {
        assert_eq!(eval(script, "styled"), "null", "{script}");
    }
    let doc = XfaDoc::default();
    let mut h = super::super::XHost::new(form(), "form1[0].styled[0]", &doc);
    let styled = h.current;
    h.instantiate(styled, "border#0/edge#7");
    h.instantiate(styled, "border#0/corner#5/color#0");
    h.instantiate(styled, "border#0/edge#3");
    let made: Vec<String> = h.prop_writes.get(&styled).map(|m| m.keys().cloned().collect()).unwrap_or_default();
    assert!(made.iter().all(|p| !p.contains("edge#7") && !p.contains("corner#5")), "{made:?}");
    assert!(made.iter().any(|p| p.starts_with("border#0/edge#3")), "a side within the four is made: {made:?}");
}

/// An edge not listed mirrors the drawn one with what a script made on it, not only what the
/// template lists: a colour set on the one listed edge shows on all four sides.
#[test]
fn edges_not_listed_mirror_what_a_script_made_on_the_drawn_one() {
    let f = node("one", XfaKind::Field, &[], vec![p("border", &[], vec![p("edge", &[], vec![])])]);
    let mut root = form();
    root.children.push(f);
    let script = "this.border.getElement('edge', 0).color.value = '255,0,0'; \
                  [0, 1, 2, 3].map(function (i) { return this.border.getElement('edge', i).color.value; }, this).join('|')";
    let o = go_on(&root, script, "one");
    assert_eq!((o.error, o.result.as_deref()), (None, Some("255,0,0|255,0,0|255,0,0|255,0,0")));
}

/// A calculation is a property object (XFA 3.3 pp. 641-642): `override` reads `disabled` where
/// the template has no `<calculate>` and `error` where it has one that doesn't say, else as
/// written; its script and message read; a write reads back in the run.
#[test]
fn a_calculation_reads_its_override_script_and_message() {
    let mut root = form();
    root.children.push(node(
        "calc",
        XfaKind::Field,
        &[],
        vec![p(
            "calculate",
            &[("override", "disabled")],
            vec![p("message", &[], vec![t("text", &[], "Computed")]), t("script", &[("contentType", "application/x-javascript")], "a + b")],
        )],
    ));
    root.children.push(node("sum", XfaKind::Field, &[], vec![p("calculate", &[], vec![t("script", &[], "1")])]));
    for (target, expr, want) in [
        ("calc", "this.calculate.override", "disabled"),
        ("calc", "this.calculate.script.value + '|' + this.calculate.message.text.value", "a + b|Computed"),
        ("calc", "this.calculate.className + '|' + this.calculate.isPropertySpecified('override')", "calculate|true"),
        ("calc", "this.calculate.override = 'warning'; this.calculate.override", "warning"),
        ("sum", "this.calculate.override", "error"),
        ("plain", "this.calculate.override + '|' + this.calculate.isPropertySpecified('override')", "disabled|false"),
        ("plain", "this['#calculate'].className + '|' + this.getElement('calculate').override", "calculate|disabled"),
    ] {
        assert_eq!(eval_on(&root, expr, target), want, "{target}: {expr}");
    }
}

/// A fill type the fill doesn't have (a pattern on a solid fill) can't be read: the read
/// throws an error a script catches, and the fill keeps its type; the type it has reads.
#[test]
fn a_fill_type_the_fill_does_not_have_throws_and_leaves_the_fill_as_it_was() {
    let mut root = form();
    root.children.push(node(
        "hatched",
        XfaKind::Field,
        &[],
        vec![p("border", &[], vec![p("fill", &[], vec![p("pattern", &[("type", "crossHatch")], vec![])])])],
    ));
    root.children.push(node(
        "solid",
        XfaKind::Field,
        &[],
        vec![p("border", &[], vec![p("fill", &[], vec![p("color", &[("value", "1,2,3")], vec![])])])],
    ));
    for (target, expr) in
        [("solid", "this.border.fill.pattern.type = 'crossHatch'"), ("plain", "this.border.fill.pattern"), ("styled", "this.border.fill.radial")]
    {
        let o = go_on(&root, expr, target);
        let e = o.error.unwrap_or_default();
        assert!(e.contains("GeneralError") && e.contains("operation failed"), "{target}: {expr}: {e}");
    }
    let caught = "var r; try { this.border.fill.pattern.type = 'crossHatch'; r = 'no error'; } catch (e) { r = e.name; } \
                  r + '|' + this.border.fill.oneOfChild.className + '|' + this.border.fill.color.value";
    assert_eq!(eval_on(&root, caught, "solid"), "GeneralError|solid|1,2,3");
    assert_eq!(eval_on(&root, "this.border.fill.pattern.type = 'horizontal'; this.border.fill.pattern.type", "hatched"), "horizontal");
    assert_eq!(eval_on(&root, "this.border.fill.oneOfChild.className + '|' + this.border.fill.linear.type", "styled"), "linear|toTop");
}

/// A border that lists more than four edges or corners still gives scripts four of each, as
/// the layout draws the first four: the fifth and later are out of reach by index, by
/// `resolveNodes`, and in `nodes`.
#[test]
fn a_border_listing_more_than_four_edges_gives_scripts_four() {
    let six = |tag: &str| (0..6).map(|i| p(tag, &[("thickness", &format!("{i}pt"))], vec![])).collect::<Vec<_>>();
    let mut kids = six("edge");
    kids.extend(six("corner"));
    let mut root = form();
    root.children.push(node("six", XfaKind::Field, &[], vec![p("border", &[], kids)]));
    for (expr, want) in [
        ("String(this.border.getElement('edge', 5))", "null"),
        ("String(this.border.getElement('edge', 4))", "null"),
        ("this.border.getElement('edge', 3).thickness", "3pt"),
        ("this.border.resolveNodes('edge[*]').length", "4"),
        ("this.border.resolveNodes('corner[*]').length", "4"),
        ("String(this.border.resolveNode('edge[5]'))", "null"),
        ("String(xfa.resolveNode('xfa.form.form1.six.border.edge[5]'))", "null"),
        ("this.border.nodes.length", "8"),
    ] {
        assert_eq!(eval_on(&root, expr, "six"), want, "{expr}");
    }
}

/// Copying the edges a script reaches past those a border lists is charged to its own budget,
/// not to the run's text budget: reaching thousands of sides leaves room for the script's
/// writes, and the sides past the copy budget still read as the side drawn there.
#[test]
fn reaching_unlisted_edges_leaves_the_write_budget_to_writes() {
    let wide = "x".repeat(40 << 10);
    let rows: Vec<XfaNode> = (0..1_000)
        .map(|i| XfaNode {
            som: format!("form1[0].row[{i}]"),
            index: i,
            repeatable: true,
            ..node("row", XfaKind::Field, &[], vec![p("border", &[], vec![p("edge", &[("thickness", "2pt"), ("note", wide.as_str())], vec![])])])
        })
        .collect();
    let mut root = form();
    root.children.extend(rows);
    let script = "var l = xfa.resolveNodes('xfa.form.form1.row[*].border.edge[3]'); \
                  var t = l.item(999).thickness; \
                  xfa.resolveNode('xfa.form.form1.styled').rawValue = 'kept'; l.length + '|' + t";
    let o = go_on(&root, script, "styled");
    assert_eq!((o.error.as_deref(), o.result.as_deref()), (None, Some("1000|2pt")), "{:?}", o.notes);
    assert!(o.effects.iter().any(|e| matches!(e, crate::xfa::XfaEffect::SetValue { value, .. } if value == "kept")), "{:?}", o.notes);
    assert!(o.notes.iter().all(|n| !n.contains("MiB")), "{:?}", o.notes);
}

/// `name in fill` is false for a fill type the fill doesn't have, whose read throws, and true
/// for what a read answers.
#[test]
fn in_says_which_fill_types_can_be_read() {
    let mut root = form();
    root.children.push(node(
        "solid",
        XfaKind::Field,
        &[],
        vec![p("border", &[], vec![p("fill", &[], vec![p("color", &[("value", "1,2,3")], vec![])])])],
    ));
    let expr = "['pattern', 'linear', '#radial', 'color', 'solid', 'presence', 'oneOfChild'].map(function (k) { return k in this.border.fill; }, this).join('|')";
    assert_eq!(eval_on(&root, expr, "solid"), "false|false|false|true|true|true|true");
    assert_eq!(eval_on(&root, "('linear' in this.border.fill) + '|' + ('pattern' in this.border.fill)", "styled"), "true|false");
}

/// `value.#text` on a draw stays unresolved: the property objects don't carry a draw's content,
/// so it reads as null rather than as an empty text until a node's value is its value element.
/// On a field its value element's content type is reached, as before.
#[test]
fn a_draws_value_text_stays_unresolved() {
    let mut root = form();
    root.children.push(node("words", XfaKind::Draw, &[], vec![p("value", &[], vec![p("text", &[], vec![])])]));
    assert_eq!(eval_on(&root, "String(this.resolveNode('value.#text'))", "words"), "null");
    assert_eq!(eval_on(&root, "this.resolveNode('value.#decimal').className", "styled"), "decimal");
}

/// A node with a child named like a member read from its properties reads that child, and a
/// write to the member doesn't reach the property either.
#[test]
fn a_member_shadowed_by_a_child_is_not_written_to_the_property() {
    let mut shadow =
        node("shadow", XfaKind::Subform, &[], vec![p("border", &[], vec![p("fill", &[], vec![p("color", &[("value", "4,4,4")], vec![])])])]);
    shadow.children = ["fillColor", "mandatory"]
        .iter()
        .map(|n| XfaNode { som: format!("form1[0].shadow[0].{n}[0]"), ..node(n, XfaKind::Field, &[], vec![]) })
        .collect();
    let mut root = form();
    root.children.push(shadow);
    let script = "this.fillColor = '9,9,9'; this.mandatory = 'error'; \
                  [this.fillColor.className, this.border.fill.color.value, this.validate.nullTest].join('|')";
    assert_eq!(eval_on(&root, script, "shadow"), "field|4,4,4|disabled");
}

/// A SOM expression into the properties of many instances gives at most
/// [`super::MAX_RESOLVED`] of them.
#[test]
fn resolving_into_many_instances_stops_at_the_limit() {
    let rows: Vec<XfaNode> = (0..5_000)
        .map(|i| XfaNode {
            som: format!("form1[0].row[{i}]"),
            index: i,
            repeatable: true,
            ..node("row", XfaKind::Subform, &[], vec![p("border", &[], vec![])])
        })
        .collect();
    let mut root = form();
    root.children.extend(rows);
    assert_eq!(eval_on(&root, "xfa.resolveNodes('xfa.form.form1.row[*].border').length", "styled"), super::MAX_RESOLVED.to_string());
}

/// An `assist`'s `speak` reads its text and attributes.
#[test]
fn assist_speak_reads_its_text_and_attributes() {
    let mut root = form();
    root.children.push(node(
        "spoken",
        XfaKind::Field,
        &[],
        vec![p("assist", &[], vec![t("speak", &[("priority", "toolTip"), ("disable", "1")], "Say this")])],
    ));
    let expr = "[this.assist.speak.value, this.assist.speak.priority, this.assist.speak.disable, this.assist.speak.className].join('|')";
    assert_eq!(eval_on(&root, expr, "spoken"), "Say this|toolTip|1|speak");
    assert_eq!(eval_on(&root, "this.assist.speak.value + '|' + this.assist.isPropertySpecified('speak')", "plain"), "|false");
}
