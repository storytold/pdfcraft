//! SOM names as scripts resolve them through unnamed subforms and areas ("Transparent Nodes",
//! XFA 3.3 pp. 94-95), and instance managers of a subform whose first instance is not number 0.

use super::super::{XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, XfaOutcome, run_xfa};
use crate::Limits;
use crate::formcalc::run_formcalc;

fn field(name: &str, som: &str, index: usize, value: &str) -> XfaNode {
    XfaNode { name: name.into(), som: som.into(), kind: Some(XfaKind::Field), value: value.into(), numeric: true, index, ..Default::default() }
}

/// An unnamed subform or area: it takes the SOM path of the container it is in.
fn unnamed(name: &str, kind: XfaKind, som: &str, children: Vec<XfaNode>) -> XfaNode {
    XfaNode { name: name.into(), som: som.into(), kind: Some(kind), children, ..Default::default() }
}

fn subform(name: &str, som: &str, index: usize, children: Vec<XfaNode>) -> XfaNode {
    XfaNode { name: name.into(), som: som.into(), kind: Some(XfaKind::Subform), index, children, ..Default::default() }
}

/// The shape of XFA 3.3 Example 3.12 (p. 95), with an area and a named sibling added:
/// form1 > { #subform > yes[0] = 1, #area > #subform > yes[1] = 2, yes[2] = 4, other > btn }
fn receipt() -> XfaNode {
    subform(
        "form1",
        "form1[0]",
        0,
        vec![
            unnamed("#subform", XfaKind::Subform, "form1[0]", vec![field("yes", "form1[0].yes[0]", 0, "1")]),
            unnamed(
                "#area",
                XfaKind::Area,
                "form1[0]",
                vec![unnamed("#subform", XfaKind::Subform, "form1[0]", vec![field("yes", "form1[0].yes[1]", 1, "2")])],
            ),
            field("yes", "form1[0].yes[2]", 2, "4"),
            subform("other", "form1[0].other[0]", 0, vec![field("btn", "form1[0].other[0].btn[0]", 0, "")]),
        ],
    )
}

fn js(root: &XfaNode, script: &str, target: &str) -> XfaOutcome {
    let ev = XfaEvent { activity: "click".into(), target: target.into(), ..Default::default() };
    run_xfa(script, &ev, &XfaDoc::default(), root, Limits::default())
}

fn fc(root: &XfaNode, script: &str, target: &str) -> XfaOutcome {
    let ev = XfaEvent { activity: "click".into(), target: target.into(), ..Default::default() };
    run_formcalc(script, &ev, &XfaDoc::default(), root, Limits::default())
}

/// What `script` gives from the button of [`receipt`], failing the test on a script error.
fn js_result(script: &str) -> String {
    let o = js(&receipt(), script, "form1[0].other[0].btn[0]");
    assert_eq!(o.error, None, "{script}: {o:?}");
    o.result.unwrap_or_default()
}

fn fc_result(script: &str) -> String {
    let o = fc(&receipt(), script, "form1[0].other[0].btn[0]");
    assert_eq!(o.error, None, "{script}: {o:?}");
    o.result.unwrap_or_default()
}

/// A name is found through the unnamed subforms and areas of the container it is asked of, and
/// `name[n]` is the n-th of them in document order: the number in its SOM path.
#[test]
fn javascript_names_see_through_unnamed_subforms_and_areas() {
    for (script, want) in [
        ("xfa.resolveNode('form1.yes').rawValue", "1"),
        ("xfa.resolveNode('form1.yes[1]').rawValue", "2"),
        ("xfa.resolveNode('xfa.form.form1.yes[2]').rawValue", "4"),
        ("xfa.resolveNode('form1.yes[1]').somExpression", "form1[0].yes[1]"),
        ("xfa.resolveNode('form1.yes[1]').index", "1"),
        ("String(xfa.resolveNode('form1.yes[3]'))", "null"),
        ("xfa.resolveNodes('form1.yes[*]').length", "3"),
        // A property read on the container, and a name from a script elsewhere in it.
        ("this.parent.parent.yes.rawValue", "1"),
        ("this.resolveNode('yes[2]').rawValue", "4"),
        ("yes.rawValue", "1"),
        ("xfa.resolveNode('form1.yes[2]').all.length", "3"),
        // The unnamed subform can still be named by its class, and its content from it.
        ("xfa.resolveNode('form1.#subform.yes').rawValue", "1"),
        ("xfa.resolveNode('form1.#area.yes').rawValue", "2"),
    ] {
        assert_eq!(js_result(script), want, "{script}");
    }
}

#[test]
fn formcalc_names_see_through_unnamed_subforms_and_areas() {
    for (script, want) in [
        ("form1.yes", "1"),
        ("form1.yes[1]", "2"),
        ("form1.yes[1].index", "1"),
        ("yes[2]", "4"),
        ("Sum(form1.yes[*])", "7"),
        ("Sum(yes[*])", "7"),
        ("Count(form1.yes[2].all)", "3"),
        ("xfa.resolveNode(\"form1.yes[1]\").rawValue", "2"),
    ] {
        assert_eq!(fc_result(script), want, "{script}");
    }
    // A value assigned by name reaches each object of the name in the scope.
    let o = fc(&receipt(), "form1.yes = 9", "form1[0].other[0].btn[0]");
    assert_eq!(o.error, None, "{o:?}");
    let set = |i: usize| XfaEffect::SetValue { som: format!("form1[0].yes[{i}]"), value: "9".into() };
    assert_eq!(o.effects, [set(0), set(1), set(2)]);
}

fn row(index: usize, repeatable: bool, what: &str, children: Vec<XfaNode>) -> XfaNode {
    let som = format!("form1[0].row[{index}]");
    let mut kids = vec![field("what", &format!("{som}.what[0]"), 0, what)];
    kids.extend(children);
    XfaNode { repeatable, occur_min: 1, occur_max: if repeatable { None } else { Some(1) }, ..subform("row", &som, index, kids) }
}

/// form1 > { #subform > row[0] (once), #subform > { row[1], row[2] } (repeating), go }: the
/// repeating subform's first instance is `row[1]`, after the `row` before it in the scope.
fn rows() -> XfaNode {
    subform(
        "form1",
        "form1[0]",
        0,
        vec![
            unnamed("#subform", XfaKind::Subform, "form1[0]", vec![row(0, false, "10", vec![])]),
            unnamed("#subform", XfaKind::Subform, "form1[0]", vec![row(1, true, "20", vec![]), row(2, true, "30", vec![])]),
            field("go", "form1[0].go[0]", 0, ""),
        ],
    )
}

/// A new instance is numbered one past the last, and a removed one's followers move down one:
/// the numbers go on from the first instance's, which is not 0 here. `instanceIndex` is the
/// place among the instances, whatever they are numbered from.
#[test]
fn instances_are_numbered_on_from_the_first_whatever_its_number() {
    let o = js(
        &rows(),
        "var m = xfa.resolveNode('form1.row[1]').instanceManager; var r = m.addInstance(1); \
         [r.somExpression, r.index, r.instanceIndex, m.count, r.what.somExpression, xfa.resolveNodes('form1.row[*]').length].join('/')",
        "form1[0].go[0]",
    );
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("form1[0].row[3]/3/2/3/form1[0].row[3].what[0]/4"));
    assert_eq!(o.effects, [XfaEffect::AddInstance { som: "form1[0].row[3]".into() }]);

    let o = js(
        &rows(),
        "var m = xfa.resolveNode('form1.row[1]').instanceManager; m.removeInstance(0); var r = xfa.resolveNode('form1.row[1]'); \
         [r.what.rawValue, r.index, r.instanceIndex, r.what.somExpression, m.count].join('/')",
        "form1[0].go[0]",
    );
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("30/1/0/form1[0].row[1].what[0]/1"), "the instance after the removed one takes its number");
    assert_eq!(o.effects, [XfaEffect::RemoveInstance { som: "form1[0].row[1]".into() }]);

    // FormCalc counts and numbers the same instances.
    let o = fc(
        &rows(),
        "form1.row[1].instanceManager.addInstance(1)\nConcat(form1.row[3].index, \"/\", form1.row[3].instanceIndex, \"/\", form1.row[2].instanceIndex)",
        "form1[0].go[0]",
    );
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("3/2/1"));
    assert_eq!(o.effects, [XfaEffect::AddInstance { som: "form1[0].row[3]".into() }]);
}

/// `_name` is the manager of the container the subform is a child of: an unnamed subform of
/// the one the name is asked of, here.
#[test]
fn an_instance_manager_is_found_through_an_unnamed_subform() {
    let root = subform(
        "form1",
        "form1[0]",
        0,
        vec![
            unnamed("#subform", XfaKind::Subform, "form1[0]", vec![row(0, true, "10", vec![]), row(1, true, "20", vec![])]),
            field("go", "form1[0].go[0]", 0, ""),
        ],
    );
    let o = js(&root, "var n = this.parent._row.count; var r = _row.addInstance(1); n + '/' + _row.count + '/' + r.somExpression", "form1[0].go[0]");
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("2/3/form1[0].row[2]"));
    assert_eq!(o.effects, [XfaEffect::AddInstance { som: "form1[0].row[2]".into() }]);
    let o = fc(&root, "form1._row.addInstance(1)\nform1._row.count", "form1[0].go[0]");
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("3"));
    let o = fc(&root, "form1._row.count = 4\nform1._row.count", "form1[0].go[0]");
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("4"));
}

/// A copy of a subform takes the first instance of each repeating subform in it, which is told
/// by the order: here the first `item` that repeats is `item[1]`, after one that occurs once.
#[test]
fn a_copy_takes_the_first_instance_of_a_nested_subform_numbered_past_zero() {
    let item = |index: usize, repeatable: bool| XfaNode {
        repeatable,
        occur_min: 1,
        occur_max: if repeatable { None } else { Some(1) },
        ..subform("item", &format!("form1[0].row[0].item[{index}]"), index, vec![])
    };
    let root = subform(
        "form1",
        "form1[0]",
        0,
        vec![row(0, true, "10", vec![item(0, false), item(1, true), item(2, true)]), field("go", "form1[0].go[0]", 0, "")],
    );
    let o = js(
        &root,
        "var r = _row.addInstance(1); var all = r.resolveNodes('item[*]'); [all.length, all.item(0).somExpression, all.item(1).somExpression].join('/')",
        "form1[0].go[0]",
    );
    assert_eq!(o.error, None, "{o:?}");
    assert_eq!(o.result.as_deref(), Some("2/form1[0].row[1].item[0]/form1[0].row[1].item[1]"));
}
