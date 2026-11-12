//! SOM numbering through unnamed subforms and areas (XFA 3.3 ch. 3, "Transparent Nodes",
//! pp. 94-95, Example 3.12): the numbers the template's objects are given, and that everything
//! that names an object by its SOM path (the layout's widgets, the live form scripts run on,
//! template lookups, the paths counted as shared) numbers it the same way.

use std::collections::HashMap;

use proptest::prelude::*;

use super::*;
use crate::data::{DataOp, som_is_shared, template_field, template_object};
use crate::model::{Node, SomIndices, Template};
use crate::numbering::MAX_SOM_INDEX;
use crate::script::{FormNode, NodeKind, Overrides, form_tree};

/// An XDP whose root subform `form1` holds `body`, with `data` as the content of its data.
fn xdp(body: &str, data: &str) -> String {
    format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form1" layout="tb">
 <pageSet><pageArea name="p"><occur max="-1"/><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
{body}
</subform></template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form1>{data}</form1></xfa:data></xfa:datasets>
</xdp:xdp>"#
    )
}

fn template(xml: &str) -> Template {
    parse(xml).unwrap().0
}

fn field(name: &str) -> String {
    format!(r#"<field name="{name}" w="100pt" h="12pt"><ui><textEdit/></ui></field>"#)
}

/// The SOM path and value of every laid-out field, in page order.
fn widgets(form: &Form) -> Vec<(String, String)> {
    let items = form.pages.iter().flat_map(|p| p.items.iter());
    items.filter_map(|it| if let Item::Widget(w) = it { Some((w.som.clone(), w.value.clone().unwrap_or_default())) } else { None }).collect()
}

/// The SOM path and value of every field of the live form, in document order.
fn live_fields(n: &FormNode, out: &mut Vec<(String, String)>) {
    if n.kind == Some(NodeKind::Field) {
        out.push((n.som.clone(), n.value.clone()));
    }
    for c in &n.children {
        live_fields(c, out);
    }
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// The number of each named child of the root, with its name, in document order (unnamed
/// containers looked through).
fn numbers(nodes: &[Node], out: &mut Vec<String>) {
    for n in nodes {
        match (&n.common().name, n) {
            (Some(name), _) => out.push(format!("{name}[{}]", n.common().som_index)),
            (None, Node::Subform(sf)) => numbers(&sf.children, out),
            (None, Node::Area(a)) => numbers(&a.children, out),
            (None, _) => out.push(format!("#[{}]", n.common().som_index)),
        }
    }
}

fn root_numbers(tpl: &Template) -> Vec<String> {
    let mut out = Vec::new();
    numbers(&tpl.root.children, &mut out);
    out
}

/// XFA 3.3 Example 3.12 (p. 95): two unnamed subforms of `Receipt` each hold a `Description`
/// and a `Total_Price`, and a third `Total_Price` follows them. The fields are
/// `Receipt.Description[0]` and `[1]`, `Receipt.Total_Price[0]`, `[1]` and `[2]`, and each
/// takes its own data value.
#[test]
fn same_named_fields_in_unnamed_subforms_count_up_through_them() {
    let detail = format!("<subform layout=\"tb\">{}{}</subform>", field("Description"), field("Total_Price"));
    let body = format!("<subform name=\"Receipt\" layout=\"tb\">{detail}{detail}{}</subform>", field("Total_Price"));
    let data = "<Receipt><Description>d0</Description><Description>d1</Description>\
                <Total_Price>t0</Total_Price><Total_Price>t1</Total_Price><Total_Price>t2</Total_Price></Receipt>";
    let xml = xdp(&body, data);
    let want = pairs(&[
        ("form1[0].Receipt[0].Description[0]", "d0"),
        ("form1[0].Receipt[0].Total_Price[0]", "t0"),
        ("form1[0].Receipt[0].Description[1]", "d1"),
        ("form1[0].Receipt[0].Total_Price[1]", "t1"),
        ("form1[0].Receipt[0].Total_Price[2]", "t2"),
    ]);
    assert_eq!(widgets(&layout_xml(&xml).unwrap()), want, "the laid-out fields");

    let tpl = template(&xml);
    let mut live = Vec::new();
    live_fields(&form_tree(&tpl, parse_datasets(&xml).as_ref(), &Overrides::default()).root, &mut live);
    assert_eq!(live, want, "the fields scripts see");
    for (som, _) in &want {
        assert!(!som_is_shared(&tpl, som), "{som} is one field's");
        assert!(template_field(&tpl, som).is_some(), "{som} is a field of the template");
    }
    assert!(template_field(&tpl, "form1[0].Receipt[0].Description[2]").is_none(), "there are two");
}

/// Each child's number is how many same-named children came before it; an unnamed one is 0, and
/// so is anything past the last.
#[test]
fn som_indices_count_earlier_same_named_siblings() {
    let draw = |name: Option<&str>| match name {
        Some(n) => format!(r#"<draw name="{n}" w="10pt" h="10pt"/>"#),
        None => r#"<draw w="10pt" h="10pt"/>"#.to_string(),
    };
    let body: String = [Some("a"), Some("b"), Some("a"), None, Some("a"), Some("b")].into_iter().map(draw).collect();
    let tpl = template(&xdp(&body, ""));
    let children = &tpl.root.children;
    let sibs = SomIndices::of(children);
    assert_eq!((0..=children.len()).map(|i| sibs.at(i)).collect::<Vec<_>>(), [0, 0, 1, 0, 2, 1, 0]);
}

/// Unnamed areas are looked through as unnamed subforms are, nested ones too; a named subform,
/// a named area and an exclusion group each start numbering again for what they hold, and are
/// themselves numbered in the scope they are in.
#[test]
fn a_named_container_starts_numbering_again_and_an_unnamed_one_does_not() {
    let radio =
        |name: &str| format!(r#"<field name="{name}" w="20pt" h="12pt"><ui><checkButton shape="round"/></ui><items><text>1</text></items></field>"#);
    let body = format!(
        "<subform layout=\"tb\">{yes}</subform>\
         <area><subform layout=\"tb\"><area>{yes}</area></subform></area>\
         {yes}\
         <area name=\"box\">{yes}<subform>{yes}</subform></area>\
         <subform name=\"box\" layout=\"tb\">{yes}</subform>\
         <exclGroup name=\"yes\" layout=\"tb\">{r}{r}</exclGroup>\
         <subform><exclGroup name=\"yes\" layout=\"tb\">{r}</exclGroup></subform>",
        yes = field("yes"),
        r = radio("yes"),
    );
    let xml = xdp(&body, "");
    let tpl = template(&xml);
    assert_eq!(root_numbers(&tpl), ["yes[0]", "yes[1]", "yes[2]", "box[0]", "box[1]", "yes[3]", "yes[4]"]);
    let soms: Vec<String> = widgets(&layout_xml(&xml).unwrap()).into_iter().map(|(som, _)| som).collect();
    let want = [
        "form1[0].yes[0]",
        "form1[0].yes[1]",
        "form1[0].yes[2]",
        "form1[0].box[0].yes[0]",
        "form1[0].box[0].yes[1]",
        "form1[0].box[1].yes[0]",
        "form1[0].yes[3].yes[0]",
        "form1[0].yes[3].yes[1]",
        "form1[0].yes[4].yes[0]",
    ];
    assert_eq!(soms, want);
    for som in want {
        assert!(!som_is_shared(&tpl, som), "{som} is one object's");
    }
}

/// The content of each page area is a scope of its own, numbered through its unnamed subforms.
#[test]
fn each_page_area_numbers_its_own_content() {
    let xml = format!(
        r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form1" layout="tb">
 <pageSet>
  <pageArea name="first"><contentArea x="36pt" y="36pt" w="540pt" h="700pt"/><medium short="612pt" long="792pt"/>
   <subform x="36pt" y="740pt">{pg}</subform>{pg}</pageArea>
  <pageArea name="rest"><contentArea x="36pt" y="36pt" w="540pt" h="700pt"/><medium short="612pt" long="792pt"/>{pg}</pageArea>
 </pageSet>
 {pg}
</subform></template>"#,
        pg = field("pg")
    );
    let tpl = template(&xml);
    let areas = &tpl.root.page_set.as_ref().unwrap().areas;
    let of = |nodes: &[Node]| {
        let mut out = Vec::new();
        numbers(nodes, &mut out);
        out
    };
    assert_eq!(of(&areas[0].items), ["pg[0]", "pg[1]"]);
    assert_eq!(of(&areas[1].items), ["pg[0]"]);
    assert_eq!(root_numbers(&tpl), ["pg[0]"]);
    let soms: Vec<String> = widgets(&layout_xml(&xml).unwrap()).into_iter().map(|(som, _)| som).collect();
    assert!(soms.contains(&"first[0].pg[1]".to_string()), "{soms:?}");
}

/// An unnamed subform the template makes twice: each instance names its content as the first
/// does, so those paths are shared, and what follows is numbered past both.
#[test]
fn what_follows_an_unnamed_subform_made_twice_is_numbered_past_both() {
    let body = format!("<subform layout=\"tb\"><occur min=\"2\" max=\"2\" initial=\"2\"/>{a}</subform>{a}", a = field("a"));
    let xml = xdp(&body, "");
    let tpl = template(&xml);
    assert_eq!(root_numbers(&tpl), ["a[0]", "a[2]"]);
    let soms: Vec<String> = widgets(&layout_xml(&xml).unwrap()).into_iter().map(|(som, _)| som).collect();
    assert_eq!(soms, ["form1[0].a[0]", "form1[0].a[0]", "form1[0].a[2]"]);
    assert!(som_is_shared(&tpl, "form1[0].a[0]"), "two fields have it");
    assert!(!som_is_shared(&tpl, "form1[0].a[2]"));
    assert!(template_field(&tpl, "form1[0].a[2]").is_some());
    assert!(template_field(&tpl, "form1[0].a[1]").is_none(), "no field is numbered 1");
}

/// A repeating subform's later instances count on from its number. A same-named subform after
/// it keeps the number the template gives it, which an instance may have too: that path counts
/// as shared, and a number past both is an instance of the one that repeats.
#[test]
fn a_number_past_a_repeating_subform_names_its_instance() {
    let body = format!(
        "<subform name=\"row\" layout=\"tb\"><occur min=\"1\" max=\"-1\"/>{}</subform><subform name=\"row\" layout=\"tb\">{}</subform>",
        field("x"),
        field("y")
    );
    let xml = xdp(&body, "<row><x>1</x></row><row><x>2</x></row><row><x>3</x></row>");
    let tpl = template(&xml);
    assert_eq!(root_numbers(&tpl), ["row[0]", "row[1]"]);
    assert!(template_field(&tpl, "form1[0].row[0].x[0]").is_some());
    assert!(template_field(&tpl, "form1[0].row[1].y[0]").is_some(), "the subform numbered 1 exactly");
    assert!(template_field(&tpl, "form1[0].row[2].x[0]").is_some(), "the third instance of the one that repeats");
    assert!(template_field(&tpl, "form1[0].row[2].y[0]").is_none());
    assert!(!som_is_shared(&tpl, "form1[0].row[0]"));
    assert!(som_is_shared(&tpl, "form1[0].row[1]"), "the second instance and the subform after it");
    // Without one the template repeats, the nearest subform below the number is it (a script
    // may have made it repeat).
    let once = xdp(&format!("<subform name=\"row\">{}</subform><subform name=\"row\">{}</subform>", field("x"), field("y")), "");
    assert!(template_field(&template(&once), "form1[0].row[2].y[0]").is_some());
}

/// Unnamed fields and exclusion groups have no number of their own: each is `field[0]` or
/// `group[0]` of its container, a path two of them share.
#[test]
fn unnamed_fields_share_the_path_of_their_container() {
    let unnamed = r#"<field w="100pt" h="12pt"><ui><textEdit/></ui></field>"#;
    let xml = xdp(&format!("<subform name=\"one\">{unnamed}</subform><subform name=\"two\">{unnamed}<subform>{unnamed}</subform></subform>"), "");
    let tpl = template(&xml);
    let soms: Vec<String> = widgets(&layout_xml(&xml).unwrap()).into_iter().map(|(som, _)| som).collect();
    assert_eq!(soms, ["form1[0].one[0].field[0]", "form1[0].two[0].field[0]", "form1[0].two[0].field[0]"]);
    assert!(!som_is_shared(&tpl, "form1[0].one[0].field[0]"));
    assert!(som_is_shared(&tpl, "form1[0].two[0].field[0]"));
}

/// An unnamed exclusion group is `group[n]` of its container to the layout, the live form and
/// the Tab order alike, so its buttons are told from the fields beside it.
#[test]
fn the_buttons_of_an_unnamed_group_are_named_in_it() {
    let radio =
        |name: &str| format!(r#"<field name="{name}" w="20pt" h="12pt"><ui><checkButton shape="round"/></ui><items><text>1</text></items></field>"#);
    // The group is written after the field `x` and stands above it: Tab reaches its buttons first.
    let body = format!(
        "<subform name=\"s\" w=\"300pt\" h=\"100pt\">         <field name=\"x\" y=\"60pt\" w=\"100pt\" h=\"12pt\"><ui><textEdit/></ui></field>         <exclGroup layout=\"tb\" y=\"0pt\" w=\"100pt\" h=\"30pt\">{}{}</exclGroup></subform>",
        radio("x"),
        radio("y"),
    );
    let xml = xdp(&body, "<s><x>typed</x></s>");
    let form = layout_xml(&xml).unwrap();
    let laid = widgets(&form);
    let soms: Vec<&str> = laid.iter().map(|(som, _)| som.as_str()).collect();
    assert_eq!(soms, ["form1[0].s[0].group[0].x[0]", "form1[0].s[0].group[0].y[0]", "form1[0].s[0].x[0]"], "in Tab order");
    assert_eq!(laid.last().map(|(_, v)| v.as_str()), Some("typed"), "the field's value is its own");

    let tpl = template(&xml);
    let mut live = Vec::new();
    live_fields(&form_tree(&tpl, parse_datasets(&xml).as_ref(), &Overrides::default()).root, &mut live);
    let mut seen: Vec<&str> = live.iter().map(|(som, _)| som.as_str()).collect();
    seen.sort_unstable();
    let mut want = soms.clone();
    want.sort_unstable();
    assert_eq!(seen, want, "the fields scripts see");
    for som in soms {
        assert!(!som_is_shared(&tpl, som), "{som} is one field's");
    }
}

/// Instance counts multiply up through nested unnamed subforms, so a few lines of template
/// reach any number: the numbers stop at [`MAX_SOM_INDEX`] instead of overflowing, and so do
/// the sums made from them.
#[test]
fn hostile_instance_counts_stop_at_the_largest_number() {
    let mut body = field("a");
    for _ in 0..12 {
        body = format!("<subform><occur min=\"1000\" max=\"1000\" initial=\"1000\"/>{body}</subform>");
    }
    body.push_str(&field("a"));
    body.push_str("<subform name=\"a\"><occur min=\"2\" max=\"-1\" initial=\"2\"/></subform>");
    let tpl = template(&xdp(&body, ""));
    assert_eq!(root_numbers(&tpl), ["a[0]".to_string(), format!("a[{MAX_SOM_INDEX}]"), format!("a[{MAX_SOM_INDEX}]")]);
    // Looked up and counted without overflow, whatever the answer.
    let som = format!("form1[0].a[{MAX_SOM_INDEX}]");
    let _ = (som_is_shared(&tpl, &som), template_object(&tpl, &som), template_object(&tpl, "form1[0].a[4294967295]"));
}

// --- Generated templates -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Occ {
    Once,
    /// `max="-1"`: one instance until data or a script makes more.
    Repeats,
    /// Exactly two.
    Twice,
    /// Two to start with, and as many as wanted.
    TwiceOrMore,
}

impl Occ {
    fn xml(self) -> &'static str {
        match self {
            Occ::Once => "",
            Occ::Repeats => r#"<occur min="1" max="-1"/>"#,
            Occ::Twice => r#"<occur min="2" max="2" initial="2"/>"#,
            Occ::TwiceOrMore => r#"<occur min="1" max="-1" initial="2"/>"#,
        }
    }

    /// How many instances a form without data has.
    fn instances(self) -> usize {
        if matches!(self, Occ::Twice | Occ::TwiceOrMore) { 2 } else { 1 }
    }
}

#[derive(Clone, Debug)]
enum What {
    Field,
    Draw,
    /// An exclusion group and the names of its buttons.
    Group(Vec<Option<&'static str>>),
    Subform(Occ, Vec<Gen>),
    Area(Vec<Gen>),
}

/// One generated template object. `id` is its place in document order, written as its `id`
/// attribute so the object a lookup finds can be told.
#[derive(Clone, Debug)]
struct Gen {
    id: usize,
    name: Option<&'static str>,
    what: What,
}

fn name() -> impl Strategy<Value = Option<&'static str>> {
    // `field` is also what an unnamed field is called.
    prop_oneof![3 => Just(None), 4 => Just(Some("a")), 2 => Just(Some("b")), 1 => Just(Some("field"))]
}

fn occ() -> impl Strategy<Value = Occ> {
    prop_oneof![4 => Just(Occ::Once), 2 => Just(Occ::Repeats), 1 => Just(Occ::Twice), 1 => Just(Occ::TwiceOrMore)]
}

fn generated() -> impl Strategy<Value = Vec<Gen>> {
    let leaf = prop_oneof![
        4 => name().prop_map(|name| Gen { id: 0, name, what: What::Field }),
        1 => name().prop_map(|name| Gen { id: 0, name, what: What::Draw }),
        1 => (name(), prop::collection::vec(name(), 1..3)).prop_map(|(name, buttons)| Gen { id: 0, name, what: What::Group(buttons) }),
    ];
    let node = leaf.prop_recursive(3, 20, 3, |inner| {
        prop_oneof![
            3 => (name(), occ(), prop::collection::vec(inner.clone(), 0..4)).prop_map(|(name, occ, kids)| Gen { id: 0, name, what: What::Subform(occ, kids) }),
            1 => (name(), prop::collection::vec(inner, 0..4)).prop_map(|(name, kids)| Gen { id: 0, name, what: What::Area(kids) }),
        ]
    });
    prop::collection::vec(node, 1..5).prop_map(|mut top| {
        fn number(nodes: &mut [Gen], positioned: bool, next: &mut usize) {
            for g in nodes {
                g.id = *next;
                *next += 1;
                match &mut g.what {
                    What::Subform(occ, kids) => {
                        // Positioned content shows one instance of a subform, however many
                        // its `occur` or the data ask for, while the live form has them all:
                        // a matter of layout, not of numbering, so an area holds no subform
                        // that repeats here.
                        if positioned {
                            *occ = Occ::Once;
                        }
                        number(kids, false, next);
                    }
                    What::Area(kids) => number(kids, true, next),
                    _ => {}
                }
            }
        }
        number(&mut top, false, &mut 0);
        top
    })
}

fn render(nodes: &[Gen], out: &mut String) {
    for g in nodes {
        let attrs = match g.name {
            Some(n) => format!(r#"name="{n}" id="n{}""#, g.id),
            None => format!(r#"id="n{}""#, g.id),
        };
        match &g.what {
            What::Field => out.push_str(&format!(r#"<field {attrs} w="60pt" h="10pt"><ui><textEdit/></ui></field>"#)),
            What::Draw => out.push_str(&format!(r#"<draw {attrs} w="60pt" h="10pt"><value><text>t</text></value></draw>"#)),
            What::Group(buttons) => {
                out.push_str(&format!(r#"<exclGroup {attrs} layout="tb">"#));
                for b in buttons {
                    let name = b.map(|n| format!(r#" name="{n}""#)).unwrap_or_default();
                    out.push_str(&format!(
                        r#"<field{name} w="20pt" h="10pt"><ui><checkButton shape="round"/></ui><items><text>1</text></items></field>"#
                    ));
                }
                out.push_str("</exclGroup>");
            }
            What::Subform(occ, kids) => {
                out.push_str(&format!(r#"<subform {attrs} layout="tb">{}"#, occ.xml()));
                render(kids, out);
                out.push_str("</subform>");
            }
            What::Area(kids) => {
                out.push_str(&format!("<area {attrs}>"));
                render(kids, out);
                out.push_str("</area>");
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Field,
    Button,
    Draw,
    Group,
    Subform,
    Area,
}

/// A live object that has a SOM path of its own, as the specification's reading gives it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Live {
    som: String,
    kind: Kind,
    /// The generated object it is an instance of (a button has none).
    id: Option<usize>,
    named: bool,
}

type Counts = HashMap<&'static str, usize>;

fn take(counts: &mut Counts, name: &'static str) -> usize {
    let n = counts.entry(name).or_insert(0);
    *n += 1;
    *n - 1
}

/// The live objects of `nodes` in the scope of the named container at `parent`, written from
/// the specification rather than from the implementation: an unnamed subform or area is
/// "removed and its children adopted" by its container (XFA 3.3 p. 94), so the same-named
/// objects of the scope count up in document order, in `counts`. What the specification leaves
/// open is modelled as the crate documents it: an unnamed object is number 0; a named subform's
/// later instances count on from its number, which only it takes in the scope; an unnamed
/// subform made more than once names its content the same in every instance, and what follows
/// is numbered past all of them.
fn live(nodes: &[Gen], parent: &str, counts: &mut Counts, out: &mut Vec<Live>) {
    for g in nodes {
        let id = Some(g.id);
        let named = g.name.is_some();
        match (&g.what, g.name) {
            (What::Field, Some(n)) => out.push(Live { som: format!("{parent}.{n}[{}]", take(counts, n)), kind: Kind::Field, id, named }),
            (What::Field, None) => out.push(Live { som: format!("{parent}.field[0]"), kind: Kind::Field, id, named }),
            (What::Draw, Some(n)) => out.push(Live { som: format!("{parent}.{n}[{}]", take(counts, n)), kind: Kind::Draw, id, named }),
            // An unnamed draw has no path of its own.
            (What::Draw, None) => {}
            (What::Group(buttons), name) => {
                let som = match name {
                    Some(n) => format!("{parent}.{n}[{}]", take(counts, n)),
                    None => format!("{parent}.group[0]"),
                };
                out.push(Live { som: som.clone(), kind: Kind::Group, id, named });
                let mut own = Counts::new();
                for b in buttons {
                    let som = match b {
                        Some(n) => format!("{som}.{n}[{}]", take(&mut own, n)),
                        None => format!("{som}.field[0]"),
                    };
                    out.push(Live { som, kind: Kind::Button, id: None, named: b.is_some() });
                }
            }
            (What::Subform(occ, kids), Some(n)) => {
                let first = take(counts, n);
                for k in 0..occ.instances() {
                    let som = format!("{parent}.{n}[{}]", first + k);
                    out.push(Live { som: som.clone(), kind: Kind::Subform, id, named });
                    live(kids, &som, &mut Counts::new(), out);
                }
            }
            (What::Subform(occ, kids), None) if occ.instances() > 1 => {
                let before = counts.clone();
                live(kids, parent, counts, out);
                for _ in 1..occ.instances() {
                    live(kids, parent, &mut before.clone(), out);
                }
                for (name, n) in counts.iter_mut() {
                    let was = before.get(name).copied().unwrap_or(0);
                    *n = was + (*n - was) * occ.instances();
                }
            }
            (What::Subform(_, kids), None) | (What::Area(kids), None) => live(kids, parent, counts, out),
            (What::Area(kids), Some(n)) => {
                let som = format!("{parent}.{n}[{}]", take(counts, n));
                out.push(Live { som: som.clone(), kind: Kind::Area, id, named });
                live(kids, &som, &mut Counts::new(), out);
            }
        }
    }
}

/// The live form's objects that have a path of their own (an unnamed subform, area or draw has
/// its container's), in document order.
fn owned(n: &FormNode, in_group: bool, out: &mut Vec<(String, Kind)>) {
    for c in &n.children {
        if c.som != n.som {
            let kind = match c.kind {
                Some(NodeKind::Field) if in_group => Kind::Button,
                Some(NodeKind::Field) => Kind::Field,
                Some(NodeKind::Draw) => Kind::Draw,
                Some(NodeKind::ExclGroup) => Kind::Group,
                Some(NodeKind::Subform) => Kind::Subform,
                _ => Kind::Area,
            };
            out.push((c.som.clone(), kind));
        }
        owned(c, c.kind == Some(NodeKind::ExclGroup), out);
    }
}

/// Whether every named subform of `nodes` occurs once, so no path is shared with an instance
/// the form doesn't have yet.
fn named_subforms_occur_once(nodes: &[Gen]) -> bool {
    nodes.iter().all(|g| match &g.what {
        What::Subform(occ, kids) => (g.name.is_none() || *occ == Occ::Once) && named_subforms_occur_once(kids),
        What::Area(kids) => named_subforms_occur_once(kids),
        _ => true,
    })
}

fn sorted<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort();
    v
}

proptest! {
    // A failing case is printed with its template; no seed file is written beside the source.
    #![proptest_config(ProptestConfig { cases: 192, failure_persistence: None, ..ProptestConfig::default() })]

    /// Over generated templates, the script side and the form side name every object alike,
    /// and as the specification's reading does: the live form scripts run on, the laid-out
    /// widgets, template lookups by path, and the paths counted as shared.
    #[test]
    fn every_side_numbers_generated_templates_alike(top in generated()) {
        let mut body = String::new();
        render(&top, &mut body);
        let xml = xdp(&body, "");
        let tpl = template(&xml);
        let mut want = Vec::new();
        live(&top, "form1[0]", &mut Counts::new(), &mut want);

        // The live form: the same objects at the same paths, in document order.
        let tree = form_tree(&tpl, None, &Overrides::default());
        prop_assert!(!tree.truncated);
        let content = FormNode { children: tree.root.children.iter().filter(|c| c.som.starts_with("form1[0]")).cloned().collect(), ..tree.root.clone() };
        let mut got = Vec::new();
        owned(&content, false, &mut got);
        prop_assert_eq!(&got, &want.iter().map(|l| (l.som.clone(), l.kind)).collect::<Vec<_>>(), "the live form of {}", body);

        // The layout: a widget for each field and button, at the same path.
        let form = layout(&tpl, None).unwrap();
        let laid = sorted(widgets(&form).into_iter().map(|(som, _)| som).collect());
        let fields = sorted(want.iter().filter(|l| matches!(l.kind, Kind::Field | Kind::Button)).map(|l| l.som.clone()).collect());
        prop_assert_eq!(laid, fields, "the widgets of {}", body);

        let mut times: HashMap<&str, usize> = HashMap::new();
        for l in &want {
            *times.entry(l.som.as_str()).or_insert(0) += 1;
        }
        let once_only = named_subforms_occur_once(&top);
        // Paths below a path several objects have: which of them is meant can't be told.
        let below_several = |som: &str| som.match_indices('.').any(|(at, _)| times.get(&som[..at]).is_some_and(|&n| n > 1));
        for l in &want {
            let several = times[l.som.as_str()] > 1;
            // A path several live objects have counts as shared; with no subform that may get
            // more instances, no other path does.
            let shared = som_is_shared(&tpl, &l.som);
            prop_assert!(shared || !several, "{} has {} objects but is not shared, in {}", l.som, times[l.som.as_str()], body);
            if once_only {
                prop_assert_eq!(shared, several, "{} in {}", &l.som, body);
            }
            // A path one named object has finds that object in the template.
            if let (Some(id), true, false) = (l.id, l.named, several || below_several(&l.som)) {
                let found = template_object(&tpl, &l.som).and_then(|n| n.common().id.clone());
                prop_assert_eq!(found, Some(format!("n{id}")), "{} in {}", &l.som, body);
            }
        }
    }

    /// With data at every field's path, the layout and the live form give each field the same
    /// value at the same path, and make the same instances of every subform.
    #[test]
    fn layout_and_live_form_read_the_same_data(top in generated()) {
        let mut body = String::new();
        render(&top, &mut body);
        let tpl = template(&xdp(&body, ""));
        let mut want = Vec::new();
        live(&top, "form1[0]", &mut Counts::new(), &mut want);
        let mut data = DataNode::default();
        for (k, l) in want.iter().filter(|l| l.kind == Kind::Field).enumerate() {
            data.apply(&DataOp::Value { path: data_path(&tpl, &l.som), text: format!("v{k}") });
        }
        let tree = form_tree(&tpl, Some(&data), &Overrides::default());
        prop_assert!(!tree.truncated);
        let mut scripts = Vec::new();
        live_fields(&tree.root, &mut scripts);
        let form = layout(&tpl, Some(&data)).unwrap();
        // A button's widget holds its group's state, not a value of its own: paths only.
        let radio = |som: &str| want.iter().any(|l| l.kind == Kind::Button && l.som == som);
        let value = |(som, v): (String, String)| if radio(&som) { (som, String::new()) } else { (som, v) };
        prop_assert_eq!(
            sorted(widgets(&form).into_iter().map(value).collect::<Vec<_>>()),
            sorted(scripts.into_iter().map(value).collect::<Vec<_>>()),
            "{}", body
        );
    }
}
