//! SOM numbering through unnamed subforms and areas (XFA 3.3 ch. 3, "Transparent Nodes",
//! pp. 94-95, Example 3.12): the numbers the template's objects are given, and that everything
//! that names an object by its SOM path (the layout's widgets, the live form scripts run on,
//! template lookups, the paths counted as shared) numbers it the same way.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;

use super::*;
use crate::data::{DataOp, som_is_shared, som_made_once, template_field, template_object};
use crate::model::{Node, SomIndices, Template};
use crate::numbering::MAX_SOM_INDEX;
use crate::script::{FormNode, MAX_REACHED, NodeKind, Overrides, apply_overrides_split, form_tree, take_owners_looked};

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
    // Without one the template repeats, the nearest subform below the number is it.
    let once = xdp(&format!("<subform name=\"row\">{}</subform><subform name=\"row\">{}</subform>", field("x"), field("y")), "");
    assert!(template_field(&template(&once), "form1[0].row[2].y[0]").is_some());
}

/// The form keeps property changes for the objects the template makes once: a path written as
/// the live form writes it, from the root subform, each segment one object that occurs once.
/// Not inside a subform that may repeat or at a number its instances take, not where two
/// objects have the path, not on page-area content, and not at a path written another way. A
/// name holding a `.` is one segment; a name holding a bracket anywhere in the template leaves
/// no path one object's.
#[test]
fn a_path_names_an_object_made_once_where_nothing_repeats_or_shares_it() {
    let unnamed = r#"<field w="100pt" h="12pt"><ui><textEdit/></ui></field>"#;
    let body = format!(
        "{a}{dotted}<subform name=\"once\">{b}<subform>{c}</subform></subform>         <subform name=\"row\" layout=\"tb\"><occur min=\"1\" max=\"-1\"/>{x}</subform><subform name=\"row\">{y}</subform>         <subform name=\"two\">{unnamed}{unnamed}</subform><subform name=\"pair\">{d}{d}</subform>         <subform><occur min=\"2\" max=\"2\"/>{twice}</subform>",
        a = field("a"),
        dotted = field("CheckList20.10"),
        b = field("b"),
        c = field("c"),
        x = field("x"),
        y = field("y"),
        d = field("d"),
        twice = field("twice"),
    );
    let tpl = template(&xdp(&body, ""));
    for som in [
        "form1[0]",
        "form1[0].a[0]",
        "form1[0].CheckList20.10[0]",
        "form1[0].once[0]",
        "form1[0].once[0].b[0]",
        "form1[0].once[0].c[0]",
        "form1[0].two[0]",
        "form1[0].pair[0].d[0]",
        "form1[0].pair[0].d[1]",
    ] {
        assert!(som_made_once(&tpl, som), "{som} is one object's, made once");
    }
    for som in [
        // A subform that may repeat, what it holds, and the numbers its instances take.
        "form1[0].row[0]",
        "form1[0].row[0].x[0]",
        "form1[0].row[1]",
        "form1[0].row[1].y[0]",
        "form1[0].row[2].x[0]",
        // Two objects' path, and the content of an unnamed subform made twice.
        "form1[0].two[0].field[0]",
        "form1[0].twice[0]",
        "form1[0].twice[1]",
        // A page area, and what the template doesn't have.
        "p[0]",
        "form1[0].nothing[0]",
        "form1[0].pair[0].d[2]",
        "form1[1].a[0]",
        "other[0].a[0]",
        // Not written as the live form writes paths.
        "",
        ".",
        "form1",
        "form1.a",
        "form1[0].a",
        "form1[0].a[00]",
        "form1[0].a[+0]",
        "form1[0].a[0].",
        "form1[0]..a[0]",
        "form1[0].a[0]x",
        "form1[0]a[0]",
        "form1[0].[0]",
        "form1[0].#field[0]",
        "form1[0].a[0][0]",
        "form1[0].a[99999999999999999999999]",
        "xfa[0].form[0].form1[0].a[0]",
    ] {
        assert!(!som_made_once(&tpl, som), "{som:?} is not one object's the template makes once");
    }
    // `a[0].b` as a name: `form1[0].a[0].b[0]` would be it and the `b` of subform `a` alike.
    let bracketed = template(&xdp(&format!("{}<subform name=\"a\">{}</subform>{}", field("a[0].b"), field("b"), field("c")), ""));
    for som in ["form1[0]", "form1[0].c[0]", "form1[0].a[0].b[0]", "form1[0].a[0]"] {
        assert!(!som_made_once(&bracketed, som), "{som}");
    }
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

/// Review r5 (D2): asking whether a path is shared looks at few owners, however many objects
/// of the template share a name. Counted, not timed.
#[test]
fn asking_who_owns_a_path_looks_at_few_owners_however_many_share_a_name() {
    let unnamed = r#"<field w="100pt" h="12pt"><ui><textEdit/></ui></field>"#;
    // 5,000 unnamed fields, each `field[0]`: the second one found settles it, and a path
    // through them, which hold nothing, looks at one.
    let tpl = template(&xdp(&unnamed.repeat(5_000), ""));
    assert!(!som_is_shared(&tpl, "form1[0].none[0]"));
    let _ = take_owners_looked();
    assert!(som_is_shared(&tpl, "form1[0].field[0]"));
    let looked = take_owners_looked();
    assert!(looked <= 2, "{looked}");
    assert!(!som_is_shared(&tpl, "form1[0].field[0].x[0]"), "a field holds nothing");
    let looked = take_owners_looked();
    assert!(looked <= 1, "{looked}");
    // 300 same-named subforms that may repeat: the number 299 may be an instance of any of
    // them, and a path through it is followed into no more than the bound allows.
    let rows = format!("<subform name=\"row\" layout=\"tb\"><occur max=\"-1\"/>{}</subform>", field("c")).repeat(300);
    let tpl = template(&xdp(&rows, ""));
    assert!(!som_is_shared(&tpl, "form1[0].row[0].c[0]"), "the first instance of the first");
    let _ = take_owners_looked();
    assert!(som_is_shared(&tpl, "form1[0].row[299].c[0]"));
    let looked = take_owners_looked();
    assert!(looked <= MAX_REACHED + 1, "{looked}");
}

/// The bound on how many objects a path is followed into takes nothing from a form within it:
/// with as many same-named subforms that may repeat as the bound, a path only the last of them
/// holds is one object's. With one more, the path counts as shared (nothing is kept at it).
#[test]
fn a_path_is_followed_into_as_many_objects_as_the_bound_allows() {
    let row = |inner: &str| format!("<subform name=\"row\" layout=\"tb\"><occur max=\"-1\"/>{inner}</subform>");
    for (rows, shared) in [(9, false), (MAX_REACHED, false), (MAX_REACHED + 1, true)] {
        let body = format!("{}{}", row(&field("c")).repeat(rows - 1), row(&format!("{}{}", field("c"), field("only"))));
        let tpl = template(&xdp(&body, ""));
        let last = rows - 1;
        assert_eq!(som_is_shared(&tpl, &format!("form1[0].row[{last}].only[0]")), shared, "{rows} rows");
        assert!(som_is_shared(&tpl, &format!("form1[0].row[{last}].c[0]")), "{rows} rows");
        assert!(!som_is_shared(&tpl, "form1[0].row[0].c[0]"), "{rows} rows");
    }
}

/// The owners of a name are found by their numbers wherever they stand: an unnamed field after
/// two fields named `field` is `field[0]` with the first of them, and a subform named `field`
/// is looked into past an unnamed field that has its number.
#[test]
fn owners_are_found_by_number_wherever_they_stand() {
    let unnamed = r#"<field w="100pt" h="12pt"><ui><textEdit/></ui></field>"#;
    let tpl = template(&xdp(&format!("{}{}{unnamed}", field("field"), field("field")), ""));
    assert!(som_is_shared(&tpl, "form1[0].field[0]"), "the first named so and the unnamed one");
    assert!(!som_is_shared(&tpl, "form1[0].field[1]"));
    for (inside, shared) in [(1, false), (2, true)] {
        let tpl = template(&xdp(&format!("{unnamed}<subform name=\"field\">{}</subform>", unnamed.repeat(inside)), ""));
        assert!(som_is_shared(&tpl, "form1[0].field[0]"), "the unnamed field and the subform");
        assert_eq!(som_is_shared(&tpl, "form1[0].field[0].field[0]"), shared, "{inside} in the subform");
    }
}

/// Page areas of one name: the number after the name counts the pages laid out, not the
/// template's areas, so what they hold is looked up as one container's. A name two of them
/// hold is shared and a name one holds is not, at the cost of a look-up or two, however many
/// areas have the name (137 here, as many as in one of the forms looked at).
#[test]
fn same_named_page_areas_are_looked_up_as_one() {
    let area = |inner: &str| {
        format!(r#"<pageArea name="p"><contentArea x="36pt" y="36pt" w="540pt" h="700pt"/><medium short="612pt" long="792pt"/>{inner}</pageArea>"#)
    };
    let xml = format!(
        r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form1" layout="tb"><pageSet>{}{}</pageSet>{}</subform></template>"#,
        area(&field("n")).repeat(136),
        area(&format!("{}{}", field("n"), field("only"))),
        field("x")
    );
    let tpl = template(&xml);
    assert_eq!(tpl.root.page_set.as_ref().map(|ps| ps.areas.len()), Some(137));
    assert!(som_is_shared(&tpl, "p[0]"), "one path for every area of the name");
    let _ = take_owners_looked();
    assert!(som_is_shared(&tpl, "p[0].n[0]"));
    assert!(!som_is_shared(&tpl, "p[0].only[0]"));
    let looked = take_owners_looked();
    assert!(looked <= 3, "{looked}");
    assert!(!som_is_shared(&tpl, "form1[0].x[0]"));
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

/// A subform and an area of one name in one container are numbered apart, and only the area
/// is no data scope: the data path of the subform's field keeps the subform's segment, so a
/// value typed into the field is written where the layout and the live form read it. (Looked up
/// by name alone, the first of the name wherever it stood, the subform's segment was dropped
/// when the area came first among the direct children.)
#[test]
fn a_subform_named_as_an_area_keeps_its_place_in_the_data_path() {
    let body =
        format!("<subform layout=\"tb\"><subform name=\"a\" layout=\"tb\">{}</subform></subform><area name=\"a\">{}</area>", field("b"), field("c"));
    let tpl = template(&xdp(&body, ""));
    assert_eq!(root_numbers(&tpl), ["a[0]", "a[1]"]);
    let path = data_path(&tpl, "form1[0].a[0].b[0]");
    assert_eq!(path, [("form1".to_string(), 0), ("a".to_string(), 0), ("b".to_string(), 0)]);
    // What the area holds binds under the area's parent.
    assert_eq!(data_path(&tpl, "form1[0].a[1].c[0]"), [("form1".to_string(), 0), ("c".to_string(), 0)]);
    let mut data = DataNode::default();
    data.apply(&DataOp::Value { path, text: "typed".into() });
    let tree = form_tree(&tpl, Some(&data), &Overrides::default());
    let mut scripts = Vec::new();
    live_fields(&tree.root, &mut scripts);
    let shown = pairs(&[("form1[0].a[0].b[0]", "typed"), ("form1[0].a[1].c[0]", "")]);
    assert_eq!(scripts, shown);
    assert_eq!(widgets(&layout(&tpl, Some(&data)).unwrap()), shown);
}

/// A repeating subform's second instance and a same-named area after the subform are both
/// `row[1]`. What the rest of the path names tells them apart: the `amount` only the subform
/// holds binds in the second row, not under the area's parent as the area's `note` does, so the
/// second row shows its own data and takes what is typed in it. The segment alone, with nothing
/// after it, is the row's: it is how a row is added or removed and how the instances nested in
/// it are counted.
#[test]
fn a_rows_field_is_not_bound_as_a_same_named_areas() {
    let body = format!(
        "<subform name=\"row\" layout=\"tb\"><occur min=\"1\" max=\"-1\"/>{}</subform><area name=\"row\">{}</area>",
        field("amount"),
        field("note")
    );
    let xml = xdp(&body, "<row><amount>1</amount></row><row><amount>2</amount></row>");
    let tpl = template(&xml);
    assert_eq!(root_numbers(&tpl), ["row[0]", "row[1]"]);
    let seg = |name: &str, index: usize| (name.to_string(), index);
    assert_eq!(data_path(&tpl, "form1[0].row[1].amount[0]"), [seg("form1", 0), seg("row", 1), seg("amount", 0)]);
    assert_eq!(data_path(&tpl, "form1[0].row[2].amount[0]"), [seg("form1", 0), seg("row", 2), seg("amount", 0)]);
    assert_eq!(data_path(&tpl, "form1[0].row[1].note[0]"), [seg("form1", 0), seg("note", 0)], "the area's content binds under its parent");
    assert_eq!(data_path(&tpl, "form1[0].row[1]"), [seg("form1", 0), seg("row", 1)], "the segment alone is the row's");
    assert!(template_field(&tpl, "form1[0].row[1].amount[0]").is_some() && template_field(&tpl, "form1[0].row[1].note[0]").is_some());
    let shown = widgets(&layout_xml(&xml).unwrap());
    for (som, value) in [("form1[0].row[0].amount[0]", "1"), ("form1[0].row[1].amount[0]", "2")] {
        assert!(shown.contains(&(som.to_string(), value.to_string())), "{som}: {shown:?}");
    }
}

/// The instances nested in a repeating subform's second instance are counted under that
/// instance's data, also when an area after the subform has its name and so its number: each
/// row shows as many lines as its own record holds.
#[test]
fn a_rows_nested_instances_are_counted_in_the_row_with_a_same_named_area_after_it() {
    let line = format!("<subform name=\"line\" layout=\"tb\"><occur min=\"1\" max=\"-1\"/>{}</subform>", field("qty"));
    let body = format!(
        "<subform name=\"row\" layout=\"tb\"><occur min=\"1\" max=\"-1\"/>{}{line}</subform><area name=\"row\">{}</area>",
        field("amount"),
        field("note")
    );
    let data = "<row><amount>1</amount><line><qty>a</qty></line></row>\
                <row><amount>2</amount><line><qty>b</qty></line><line><qty>c</qty></line></row>";
    let shown = widgets(&layout_xml(&xdp(&body, data)).unwrap());
    let lines: Vec<_> = shown.iter().filter(|(som, _)| som.ends_with(".qty[0]")).cloned().collect();
    let want = pairs(&[("form1[0].row[0].line[0].qty[0]", "a"), ("form1[0].row[1].line[0].qty[0]", "b"), ("form1[0].row[1].line[1].qty[0]", "c")]);
    assert_eq!(lines, want);
}

/// An object's number is its data node's, and the data keeps 10,000 same-named nodes under one
/// node. The numbers count through the instances of unnamed subforms, so two lines of template
/// number an object past that: it is shown, and a value typed in it is not kept, which the
/// write says by the object's path.
#[test]
fn a_value_for_an_object_numbered_past_the_datas_limit_is_reported_not_kept() {
    let body = format!(
        "<subform><occur min=\"101\" max=\"101\" initial=\"101\"/><subform><occur min=\"100\" max=\"100\" initial=\"100\"/>{0}</subform></subform>{0}",
        field("a")
    );
    let tpl = template(&xdp(&body, ""));
    assert_eq!(root_numbers(&tpl), ["a[0]", "a[10100]"]);
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::fixtures::static_shell("<form1/>"))).unwrap();
    let typed = |som: &str| FieldDatum { obj: pdfcraft_cos::ObjRef::new(1000, 0), name: som.into(), data: FieldData::Text("v".into()) };
    let w = write_datasets(&mut doc, &[typed("form1[0].a[10100]")]).unwrap();
    assert!(w.warnings.iter().any(|m| m.contains("repeats too often") && m.contains("a[10100]")), "{w:?}");
    assert_eq!(crate::script::data_of(&doc).map(|d| d.count(&[("form1".to_string(), 0)], "a")), Some(0), "nothing is made for it");
    // One within the limit is kept, with the nodes before it.
    let w = write_datasets(&mut doc, &[typed("form1[0].a[100]")]).unwrap();
    assert!(w.written && w.warnings.is_empty(), "{w:?}");
    assert_eq!(crate::script::data_of(&doc).map(|d| d.count(&[("form1".to_string(), 0)], "a")), Some(101));
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
    // `field` is also what an unnamed field is called, and `group` an unnamed exclusion group.
    prop_oneof![6 => Just(None), 8 => Just(Some("a")), 4 => Just(Some("b")), 2 => Just(Some("field")), 1 => Just(Some("group"))]
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

/// The model of `top` under the root subform: its live objects in document order, each
/// subform of `more` (by the generated subform's id) with that many instances beyond those
/// its template starts with, as when the data holds them.
fn model(top: &[Gen], more: &HashMap<usize, usize>) -> Vec<Live> {
    let mut out = Vec::new();
    live(top, "form1[0]", &mut Counts::new(), more, &mut out);
    out
}

/// The live objects of `nodes` in the scope of the named container at `parent`, written from
/// the specification rather than from the implementation: an unnamed subform or area is
/// "removed and its children adopted" by its container (XFA 3.3 p. 94), so the same-named
/// objects of the scope count up in document order, in `counts`. What the specification leaves
/// open is modelled as the crate documents it: an unnamed object is number 0; a named subform's
/// later instances count on from its number, which only it takes in the scope; an unnamed
/// subform made more than once names its content the same in every instance, and what follows
/// is numbered past all of them.
fn live(nodes: &[Gen], parent: &str, counts: &mut Counts, more: &HashMap<usize, usize>, out: &mut Vec<Live>) {
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
                let instances = occ.instances() + more.get(&g.id).copied().unwrap_or(0);
                for k in 0..instances {
                    let som = format!("{parent}.{n}[{}]", first + k);
                    out.push(Live { som: som.clone(), kind: Kind::Subform, id, named });
                    live(kids, &som, &mut Counts::new(), more, out);
                }
            }
            (What::Subform(occ, kids), None) if occ.instances() > 1 => {
                let before = counts.clone();
                live(kids, parent, counts, more, out);
                for _ in 1..occ.instances() {
                    live(kids, parent, &mut before.clone(), more, out);
                }
                for (name, n) in counts.iter_mut() {
                    let was = before.get(name).copied().unwrap_or(0);
                    *n = was + (*n - was) * occ.instances();
                }
            }
            (What::Subform(_, kids), None) | (What::Area(kids), None) => live(kids, parent, counts, more, out),
            (What::Area(kids), Some(n)) => {
                let som = format!("{parent}.{n}[{}]", take(counts, n));
                out.push(Live { som: som.clone(), kind: Kind::Area, id, named });
                live(kids, &som, &mut Counts::new(), more, out);
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

/// The objects of the root subform's own content in `tree` that have a path of their own.
fn owned_content(tree: &crate::script::LiveForm) -> Vec<(String, Kind)> {
    let content = FormNode { children: tree.root.children.iter().filter(|c| c.som.starts_with("form1[0]")).cloned().collect(), ..tree.root.clone() };
    let mut got = Vec::new();
    owned(&content, false, &mut got);
    got
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

/// How many of the model's objects have each path.
fn times(want: &[Live]) -> HashMap<&str, usize> {
    let mut times = HashMap::new();
    for l in want {
        *times.entry(l.som.as_str()).or_insert(0) += 1;
    }
    times
}

/// Whether `som` is below a path several objects have: which of them is meant can't be told.
fn below_several(times: &HashMap<&str, usize>, som: &str) -> bool {
    som.match_indices('.').any(|(at, _)| times.get(&som[..at]).is_some_and(|&n| n > 1))
}

/// The container's path, the name and the number of a model path's last segment.
fn last_segment(som: &str) -> (&str, &str, usize) {
    let (parent, last) = som.rsplit_once('.').unwrap_or(("", som));
    let (name, number) = last.split_once('[').unwrap_or((last, "0]"));
    (parent, name, number.trim_end_matches(']').parse().unwrap_or(0))
}

/// The paths of the model's areas.
fn areas(want: &[Live]) -> HashSet<&str> {
    want.iter().filter(|l| l.kind == Kind::Area).map(|l| l.som.as_str()).collect()
}

/// The data path of the model's object at `som`: each named container on the way with its
/// number, an object taking the data node of its number, without the areas (`areas`, by
/// path): an area is no data scope, and what it holds binds under its container.
fn model_data_path(som: &str, areas: &HashSet<&str>) -> DataPath {
    let mut out = DataPath::new();
    let mut from = 0;
    for end in som.match_indices('.').map(|(at, _)| at).chain([som.len()]) {
        if !areas.contains(&som[..end]) {
            let (_, name, number) = last_segment(&som[from..end]);
            out.push((name.to_string(), number));
        }
        from = end + 1;
    }
    out
}

/// Whether the data can't tell an instance of the subform at `at` from another object: a
/// named one that takes its number or a later one in its SOM scope, or binds a data node of
/// its name with such a number under the same data node (what a named area holds is numbered
/// apart and binds beside what its container holds).
fn contested(want: &[Live], areas: &HashSet<&str>, at: &Live) -> bool {
    let (scope, name, number) = last_segment(&at.som);
    let path = model_data_path(&at.som, areas);
    let data_scope = &path[..path.len().saturating_sub(1)];
    want.iter().filter(|l| l.named && l.id != at.id).any(|l| {
        let (s, n, k) = last_segment(&l.som);
        let in_scope = s == scope && n == name && k >= number;
        let other = model_data_path(&l.som, areas);
        let in_data = l.kind != Kind::Area && other.split_last().is_some_and(|((n, k), s)| s == data_scope && n == name && *k >= number);
        in_scope || in_data
    })
}

/// Every named subform of `nodes`, by id, with whether it occurs once by the template.
fn named_subforms(nodes: &[Gen], out: &mut Vec<(usize, bool)>) {
    for g in nodes {
        match &g.what {
            What::Subform(occ, kids) => {
                if g.name.is_some() {
                    out.push((g.id, *occ == Occ::Once));
                }
                named_subforms(kids, out);
            }
            What::Area(kids) => named_subforms(kids, out),
            _ => {}
        }
    }
}

/// What [`more_instances`] gathers.
struct Pick<'m> {
    base: &'m [Live],
    times: HashMap<&'m str, usize>,
    more: HashMap<usize, usize>,
}

/// The generated subforms the data of the third property gives more instances than their
/// templates start with, and how many more: named ones the model (`base`, without any) has at
/// paths of their own that the template lets repeat (`max="-1"`).
fn more_instances(top: &[Gen], base: &[Live]) -> HashMap<usize, usize> {
    fn walk(nodes: &[Gen], p: &mut Pick) {
        for g in nodes {
            match &g.what {
                What::Subform(occ, kids) => {
                    let base = p.base;
                    let at: Vec<&Live> = base.iter().filter(|l| l.id == Some(g.id) && l.kind == Kind::Subform).collect();
                    let own =
                        g.name.is_some() && !at.is_empty() && at.iter().all(|l| p.times[l.som.as_str()] == 1 && !below_several(&p.times, &l.som));
                    if own && matches!(occ, Occ::Repeats | Occ::TwiceOrMore) {
                        p.more.insert(g.id, 1 + g.id % 2);
                    }
                    walk(kids, p);
                }
                What::Area(kids) => walk(kids, p),
                _ => {}
            }
        }
    }
    let mut p = Pick { base, times: times(base), more: HashMap::new() };
    walk(top, &mut p);
    p.more
}

/// The data node at `path` below `root`, made where missing with empty nodes of its name
/// before it.
fn data_at<'a>(root: &'a mut DataNode, path: &[(String, usize)]) -> &'a mut DataNode {
    let mut node = root;
    for (name, number) in path {
        let have = node.children.iter().filter(|c| &c.name == name).count();
        for _ in have..=*number {
            node.children.push(DataNode { name: name.clone(), ..DataNode::default() });
        }
        node = node.children.iter_mut().filter(|c| &c.name == name).nth(*number).expect("made above");
    }
    node
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
        let want = model(&top, &HashMap::new());

        // The live form: the same objects at the same paths, in document order.
        let tree = form_tree(&tpl, None, &Overrides::default());
        prop_assert!(!tree.truncated);
        prop_assert_eq!(&owned_content(&tree), &want.iter().map(|l| (l.som.clone(), l.kind)).collect::<Vec<_>>(), "the live form of {}", body);

        // The layout: a widget for each field and button, at the same path.
        let form = layout(&tpl, None).unwrap();
        let laid = sorted(widgets(&form).into_iter().map(|(som, _)| som).collect());
        let fields = sorted(want.iter().filter(|l| matches!(l.kind, Kind::Field | Kind::Button)).map(|l| l.som.clone()).collect());
        prop_assert_eq!(laid, fields, "the widgets of {}", body);

        let times = times(&want);
        let once_only = named_subforms_occur_once(&top);
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
            if let (Some(id), true, false) = (l.id, l.named, several || below_several(&times, &l.som)) {
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
        let want = model(&top, &HashMap::new());
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

    /// Data written from the specification's reading alone, with more rows than the template
    /// starts with (of subforms that repeat): the live form
    /// and the layout make the instances the model has, each at the model's path, and a field
    /// that has a path and a data node of its own shows the value written for it. Left to the
    /// property above: a template where the data can't tell the instances of a subform that
    /// occurs more than once, or may, from another object of its name ([`contested`]).
    #[test]
    fn data_makes_the_models_instances_and_each_field_shows_its_own_value(top in generated()) {
        let base = model(&top, &HashMap::new());
        let base_areas = areas(&base);
        let mut subforms = Vec::new();
        named_subforms(&top, &mut subforms);
        let unclear = |id: usize| base.iter().filter(|l| l.id == Some(id) && l.kind == Kind::Subform).any(|l| contested(&base, &base_areas, l));
        if subforms.iter().any(|&(id, once)| !once && unclear(id)) {
            return Ok(());
        }
        let mut body = String::new();
        render(&top, &mut body);
        let tpl = template(&xdp(&body, ""));
        let more = more_instances(&top, &base);
        let want = model(&top, &more);
        let times = times(&want);
        let areas = areas(&want);

        // A value for each named field (an unnamed one binds no data of its own), and a node
        // for each instance of the subforms given more.
        let mut data = DataNode::default();
        let mut written: Vec<(usize, DataPath)> = Vec::new();
        for (k, l) in want.iter().enumerate() {
            let path = model_data_path(&l.som, &areas);
            match l.kind {
                Kind::Field if l.named => data_at(&mut data, &path).text = format!("v{k}"),
                Kind::Subform if l.id.is_some_and(|id| more.contains_key(&id)) => {
                    data_at(&mut data, &path);
                }
                _ => continue,
            }
            written.push((k, path));
        }
        let ov = Overrides::default();

        let tree = form_tree(&tpl, Some(&data), &ov);
        prop_assert!(!tree.truncated);
        prop_assert_eq!(&owned_content(&tree), &want.iter().map(|l| (l.som.clone(), l.kind)).collect::<Vec<_>>(), "the live form of {} with {:?}", body, data);
        let mut scripts = Vec::new();
        live_fields(&tree.root, &mut scripts);
        let applied = apply_overrides_split(&tpl, &ov);
        let form = crate::layout::layout_with_overrides(&applied.template, Some(&data), &ov).unwrap();
        let laid = widgets(&form);
        let fields = sorted(want.iter().filter(|l| matches!(l.kind, Kind::Field | Kind::Button)).map(|l| l.som.clone()).collect());
        prop_assert_eq!(sorted(laid.iter().map(|(som, _)| som.clone()).collect()), fields, "the widgets of {}", body);

        for (k, path) in &written {
            let l = &want[*k];
            // Its own: one object at the path, and no node written at, above or below its data
            // node but those of the subforms it is in (what an area holds binds beside what
            // its container holds).
            let apart = |other: &DataPath| !(other.starts_with(path) || path.starts_with(other));
            let around = |j: usize| want[j].kind == Kind::Subform && l.som.strip_prefix(want[j].som.as_str()).is_some_and(|rest| rest.starts_with('.'));
            let own = l.kind == Kind::Field
                && times[l.som.as_str()] == 1
                && !below_several(&times, &l.som)
                && written.iter().all(|(j, other)| j == k || around(*j) || apart(other));
            if !own {
                continue;
            }
            prop_assert_eq!(&data_path(&tpl, &l.som), path, "{} in {}", &l.som, body);
            let shown = |all: &[(String, String)]| all.iter().filter(|(som, _)| *som == l.som).map(|(_, v)| v.clone()).collect::<Vec<_>>();
            prop_assert_eq!(shown(&scripts), [format!("v{k}")], "scripts read {} in {}", &l.som, body);
            prop_assert_eq!(shown(&laid), [format!("v{k}")], "the layout shows {} in {}", &l.som, body);
        }
    }
}
