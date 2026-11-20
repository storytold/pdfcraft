//! Explicit data binding (XFA 3.3, "The bind element", *The match property*, p. 176).
//!
//! - Subforms with `<bind match="none"/>` "will not bind to any node in the XFA Data DOM", and
//!   are "treated like a nameless subform by the data binding process" while scripts still
//!   reach them by name ("Transparent Nodes", p. 194). So what they hold binds in the
//!   enclosing data scope.
//! - Fields, exclusion groups and subforms with `<bind match="dataRef" ref="…"/>` bind to the
//!   data node the SOM expression `ref` names ("Explicit Data References", p. 199); a subform's
//!   children then bind by name under that node (p. 200).

use super::*;
use crate::layout::{Item, WidgetKind};
use crate::model::Template;

/// The widgets laid out on `page`, in order.
fn widgets(page: &Page) -> Vec<&Widget> {
    page.items.iter().filter_map(|i| if let Item::Widget(w) = i { Some(w.as_ref()) } else { None }).collect()
}

/// The value laid out in the field whose SOM ends with `tail`, on any page.
fn value_at(form: &Form, tail: &str) -> Option<String> {
    form.pages.iter().flat_map(|p| widgets(p)).find(|w| w.som.ends_with(tail)).and_then(|w| w.value.clone())
}

/// `data_path` of `som`, written `a[0].b[1]`.
fn path(tpl: &Template, som: &str) -> String {
    data_path(tpl, som).into_iter().map(|(n, i)| format!("{n}[{i}]")).collect::<Vec<_>>().join(".")
}

fn xdp(body: &str, data: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
{body}
</subform>
</template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form>{data}</form></xfa:data></xfa:datasets>
</xdp:xdp>"#
    )
}

const TEXT: &str = r#"w="2in" h="0.3in"><ui><textEdit/></ui>"#;

/// The case of #5483: section subforms marked `match="none"`, alone or inside a nameless
/// wrapper, with the answers flat under the form's own data group. Each field shows its answer,
/// and binds (so reads and writes) where the answer is.
#[test]
fn fields_in_a_match_none_subform_bind_in_the_enclosing_scope() {
    let body = format!(
        r#"<subform name="Section1" layout="tb"><bind match="none"/><field name="color" {TEXT}</field></subform>
<subform layout="tb"><subform name="Section2" layout="tb"><bind match="none"/><field name="pet" {TEXT}</field></subform></subform>
<subform name="Section3" layout="tb"><field name="town" {TEXT}</field></subform>"#
    );
    let xml = xdp(&body, "<color>Teal</color><pet>Biscuit</pet><Section3><town>Springfield</town></Section3>");
    let form = layout_xml(&xml).unwrap();
    assert_eq!(value_at(&form, "Section1[0].color[0]").as_deref(), Some("Teal"));
    assert_eq!(value_at(&form, "Section2[0].pet[0]").as_deref(), Some("Biscuit"));
    assert_eq!(value_at(&form, "Section3[0].town[0]").as_deref(), Some("Springfield"), "a subform that binds still opens its own scope");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].Section1[0].color[0]"), "form[0].color[0]");
    assert_eq!(path(&tpl, "form[0].#subform[0].Section2[0].pet[0]"), "form[0].pet[0]");
    assert_eq!(path(&tpl, "form[0].Section3[0].town[0]"), "form[0].Section3[0].town[0]");
    // Spelled out with the other values the attribute takes, a subform binds as before.
    let once = xdp(&format!(r#"<subform name="Section1" layout="tb"><bind match="once"/><field name="color" {TEXT}</field></subform>"#), "");
    let (tpl, _) = parse(&once).unwrap();
    assert_eq!(path(&tpl, "form[0].Section1[0].color[0]"), "form[0].Section1[0].color[0]");
}

/// Designer keeps the name when a subform is copied, so copies are told apart by their SOM
/// index. The binding walk steps into the copy the index names: here only the second copy's
/// `wrap` is `match="none"`, so only its field binds without a `wrap` group.
#[test]
fn copies_of_a_match_none_subform_are_told_apart_by_their_index() {
    let body = format!(
        r#"<subform name="group" layout="tb"><bind match="none"/><subform name="wrap" layout="tb"><field name="x" {TEXT}</field></subform></subform>
<subform name="group" layout="tb"><bind match="none"/><subform name="wrap" layout="tb"><bind match="none"/><field name="y" {TEXT}</field></subform></subform>"#
    );
    let xml = xdp(&body, "<wrap><x>first</x></wrap><y>second</y>");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].group[0].wrap[0].x[0]"), "form[0].wrap[0].x[0]");
    assert_eq!(path(&tpl, "form[0].group[1].wrap[0].y[0]"), "form[0].y[0]");
    let form = layout_xml(&xml).unwrap();
    assert_eq!(value_at(&form, "group[0].wrap[0].x[0]").as_deref(), Some("first"));
    assert_eq!(value_at(&form, "group[1].wrap[0].y[0]").as_deref(), Some("second"));
}

/// A data group of the same name as a `match="none"` sibling doesn't belong to it: the data
/// index of a subform that binds counts only the same-named siblings that bind too.
#[test]
fn a_match_none_sibling_takes_no_data_index() {
    let body = format!(
        r#"<subform name="part" layout="tb"><bind match="none"/><field name="note" {TEXT}</field></subform>
<subform name="part" layout="tb"><field name="qty" {TEXT}</field></subform>"#
    );
    let xml = xdp(&body, "<note>n</note><part><qty>7</qty></part>");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].part[1].qty[0]"), "form[0].part[0].qty[0]");
    let form = layout_xml(&xml).unwrap();
    assert_eq!(value_at(&form, "part[0].note[0]").as_deref(), Some("n"));
    assert_eq!(value_at(&form, "part[1].qty[0]").as_deref(), Some("7"));
}

/// A repeating subform marked `match="none"` has no data group of its own to count, so it is
/// laid out as its `occur` says, as a nameless repeating subform is, however many groups of its
/// name the data holds.
#[test]
fn a_repeating_match_none_subform_takes_its_count_from_occur() {
    let body = format!(
        r#"<subform name="row" layout="tb"><bind match="none"/><occur min="1" max="-1" initial="2"/><field name="amount" {TEXT}</field></subform>"#
    );
    let xml = xdp(&body, "<row/><row/><row/><row/>");
    let form = layout_xml(&xml).unwrap();
    let rows = form.pages.iter().flat_map(|p| widgets(p)).filter(|w| w.som.ends_with(".amount[0]")).count();
    assert_eq!(rows, 2);
}

/// `<bind match="dataRef" ref="…"/>` on a field, an exclusion group or a subform binds it to
/// the node the reference names, whatever the object is called and wherever it sits: `$.` and a
/// bare name are relative to the parent's data node, `$record` is the form's record (the root
/// data group), `$data` the data itself, and `[n]` an index. A subform bound this way is the
/// data scope of its children, which bind by name under its node (XFA 3.3 pp. 199–200).
#[test]
fn explicit_data_references_bind_to_the_node_they_name() {
    let r#ref = |r: &str| format!(r#"<bind match="dataRef" ref="{r}"/>"#);
    let body = format!(
        r#"<subform name="page1" layout="tb"><bind match="none"/>
 <field name="a" {TEXT}{}</field>
 <field name="b" {TEXT}{}</field>
 <field name="c" {TEXT}{}</field>
 <field name="d" {TEXT}{}</field>
 <field name="e" {TEXT}{}</field>
 <subform name="anything" layout="tb">{}<field name="street" {TEXT}</field></subform>
 <exclGroup name="choice" layout="tb">{}
  <field name="yes" w="0.3in" h="0.3in"><ui><checkButton shape="round"/></ui><items><text>Y</text></items></field>
  <field name="no" w="0.3in" h="0.3in"><ui><checkButton shape="round"/></ui><items><text>N</text></items></field>
 </exclGroup>
</subform>"#,
        r#ref("$.person.surname"),
        r#ref("$record.person.first"),
        r#ref("$data.form.ids.id[1]"),
        r#ref("person.title"),
        r#ref("$.ids.id"),
        r#ref("$.person.address"),
        r#ref("$.person.agrees"),
    );
    let data = "<person><surname>Smith</surname><first>Ann</first><title>Dr</title><address><street>Main Rd</street></address><agrees>N</agrees></person>\
                <ids><id>A1</id><id>B2</id></ids>";
    let xml = xdp(&body, data);
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].page1[0].a[0]"), "form[0].person[0].surname[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].b[0]"), "form[0].person[0].first[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].c[0]"), "form[0].ids[0].id[1]");
    assert_eq!(path(&tpl, "form[0].page1[0].d[0]"), "form[0].person[0].title[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].e[0]"), "form[0].ids[0].id[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].anything[0].street[0]"), "form[0].person[0].address[0].street[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].choice[0]"), "form[0].person[0].agrees[0]");
    let form = layout_xml(&xml).unwrap();
    for (tail, want) in [("a[0]", "Smith"), ("b[0]", "Ann"), ("c[0]", "B2"), ("d[0]", "Dr"), ("e[0]", "A1"), ("street[0]", "Main Rd")] {
        assert_eq!(value_at(&form, &format!(".{tail}")).as_deref(), Some(want), "{tail}");
    }
    let on: Vec<&str> = form
        .pages
        .iter()
        .flat_map(|p| widgets(p))
        .filter(|w| w.value.is_some())
        .filter_map(|w| match &w.kind {
            WidgetKind::Radio { on, .. } => Some(on.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(on, ["N"], "the group takes the value its reference names");
}

/// A reference this binding can't follow (another DOM, or `..`, which searches the data) leaves
/// the object to bind by name, so its value still has somewhere to go.
#[test]
fn a_reference_that_cant_be_followed_binds_by_name() {
    let body = format!(
        r#"<field name="x" {TEXT}<bind match="dataRef" ref="$form.other"/></field>
<field name="y" {TEXT}<bind match="dataRef" ref="$..y"/></field>"#
    );
    let (tpl, _) = parse(&xdp(&body, "")).unwrap();
    assert_eq!(path(&tpl, "form[0].x[0]"), "form[0].x[0]");
    assert_eq!(path(&tpl, "form[0].y[0]"), "form[0].y[0]");
}

/// Designer copies of a subform keep its name, so the copies are told apart by their SOM index
/// (`group[2]`), and each copy may hold different objects bound to different data nodes. The
/// binding walk has to step into the copy the index names, not the first one of that name.
#[test]
fn explicit_bindings_follow_the_index_of_same_named_siblings() {
    let body = format!(
        r#"<subform name="page1" layout="tb"><bind match="none"/>
  <subform name="group" layout="tb"><bind match="none"/>
   <field name="altId" {TEXT}<bind match="dataRef" ref="$.person.altId"/></field>
  </subform>
  <subform name="group" layout="tb"><bind match="none"/>
   <field name="other" {TEXT}<bind match="dataRef" ref="$.person.other"/></field>
  </subform>
  <subform name="group" layout="tb"><bind match="none"/>
   <field name="value" {TEXT}<bind match="dataRef" ref="$.person.surname"/></field>
  </subform>
  <subform name="row" layout="tb"><bind match="none"/>
   <field name="amount" {TEXT}<bind match="dataRef" ref="$.income.line[0].amount"/></field>
  </subform>
  <subform name="row" layout="tb"><bind match="none"/>
   <field name="amount" {TEXT}<bind match="dataRef" ref="$.income.line[1].amount"/></field>
  </subform>
  <subform name="row" layout="tb"><bind match="none"/>
   <field name="amount" {TEXT}<bind match="dataRef" ref="$.income.line[2].amount"/></field>
  </subform>
 </subform>"#
    );
    let xml = xdp(
        &body,
        "<person><altId/><other/><surname>Smith</surname></person><income><line><amount>100</amount></line><line><amount>200</amount></line></income>",
    );
    let form = layout_xml(&xml).unwrap();
    assert_eq!(value_at(&form, "group[2].value[0]").as_deref(), Some("Smith"), "a field in the third copy binds by its own ref");
    let amounts: Vec<Option<String>> =
        form.pages.iter().flat_map(|p| widgets(p)).filter(|w| w.som.ends_with(".amount[0]")).map(|w| w.value.clone()).collect();
    assert_eq!(amounts, [Some("100".to_string()), Some("200".to_string()), None], "each copied row reads its own line");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].page1[0].group[2].value[0]"), "form[0].person[0].surname[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].row[1].amount[0]"), "form[0].income[0].line[1].amount[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].row[2].amount[0]"), "form[0].income[0].line[2].amount[0]");
}

/// A field on a master page binds by its reference too, and `$record` there is still the
/// form's record, not the page area it is drawn on: every page shows the one value.
#[test]
fn a_master_page_field_binds_by_its_reference() {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="1in" w="7.5in" h="9in"/><medium short="8.5in" long="11in"/>
  <field name="code" x="0.5in" y="0.2in" {TEXT}<bind match="dataRef" ref="$record.office.code"/></field>
 </pageArea></pageSet>
 <field name="body" {TEXT}</field>
</subform>
</template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><office><code>K9</code></office></form></xfa:data></xfa:datasets>
</xdp:xdp>"#
    );
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "front[0].code[0]"), "form[0].office[0].code[0]");
    assert_eq!(path(&tpl, "front[1].code[0]"), "form[0].office[0].code[0]");
    let form = layout_xml(&xml).unwrap();
    assert_eq!(value_at(&form, "code[0]").as_deref(), Some("K9"));
}

/// A `[*]` in a reference binds "one more instance of the object matching the `*`" (XFA 3.3,
/// "Explicit Data References", p. 199): objects whose reference reads the same, in the same
/// data scope, take the instances in turn, in the order the form holds them. Here three
/// copies of a row (not one repeating subform) read lines 0, 1 and 2; a reference that reads
/// differently counts on its own.
#[test]
fn copies_with_the_same_star_reference_take_its_instances_in_turn() {
    let row = |n: &str| {
        format!(
            r#"<subform name="row" layout="tb"><bind match="none"/><field name="amount{n}" {TEXT}<bind match="dataRef" ref="$.income.line[*].amount"/></field></subform>"#
        )
    };
    let body = format!(
        r#"<subform name="page1" layout="tb"><bind match="none"/>{}{}{}<field name="code" {TEXT}<bind match="dataRef" ref="$.codes.code[*]"/></field></subform>"#,
        row("A"),
        row("B"),
        row("C")
    );
    let data = "<income><line><amount>100</amount></line><line><amount>200</amount></line><line><amount>300</amount></line></income><codes><code>X</code></codes>";
    let xml = xdp(&body, data);
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].page1[0].row[0].amountA[0]"), "form[0].income[0].line[0].amount[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].row[1].amountB[0]"), "form[0].income[0].line[1].amount[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].row[2].amountC[0]"), "form[0].income[0].line[2].amount[0]");
    assert_eq!(path(&tpl, "form[0].page1[0].code[0]"), "form[0].codes[0].code[0]");
    let form = layout_xml(&xml).unwrap();
    assert_eq!(value_at(&form, "amountA[0]").as_deref(), Some("100"));
    assert_eq!(value_at(&form, "amountB[0]").as_deref(), Some("200"));
    assert_eq!(value_at(&form, "amountC[0]").as_deref(), Some("300"));
    assert_eq!(value_at(&form, "code[0]").as_deref(), Some("X"));
}

/// A repeating subform whose reference ends in `[*]` lays out one instance per node the
/// reference matches, instance `k` bound to the `k`-th; its fields bind by name under it.
#[test]
fn a_repeating_subform_bound_with_a_star_reference_repeats_with_the_data() {
    let body = format!(
        r#"<subform name="page1" layout="tb"><bind match="none"/>
 <subform name="item" layout="tb"><occur min="0" max="-1"/><bind match="dataRef" ref="$.income.line[*]"/><field name="amount" {TEXT}</field></subform>
</subform>"#
    );
    let data = "<income><line><amount>100</amount></line><line><amount>200</amount></line><line><amount>300</amount></line></income>";
    let xml = xdp(&body, data);
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].page1[0].item[2].amount[0]"), "form[0].income[0].line[2].amount[0]");
    let form = layout_xml(&xml).unwrap();
    let amounts: Vec<Option<String>> =
        form.pages.iter().flat_map(|p| widgets(p)).filter(|w| w.som.ends_with(".amount[0]")).map(|w| w.value.clone()).collect();
    assert_eq!(amounts, [Some("100".to_string()), Some("200".to_string()), Some("300".to_string())]);
}

/// Objects take the instances of a `[*]` reference in turn only within one data scope: the
/// same reference in two repeated rows (each its own scope) starts again in each.
#[test]
fn star_references_count_within_their_data_scope() {
    let body = format!(
        r#"<subform name="row" layout="tb"><occur min="1" max="-1"/>
 <field name="first" {TEXT}<bind match="dataRef" ref="$.v[*]"/></field>
 <field name="second" {TEXT}<bind match="dataRef" ref="$.v[*]"/></field>
</subform>"#
    );
    let xml = xdp(&body, "<row><v>a</v><v>b</v></row><row><v>c</v><v>d</v></row>");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].row[1].first[0]"), "form[0].row[1].v[0]");
    assert_eq!(path(&tpl, "form[0].row[1].second[0]"), "form[0].row[1].v[1]");
    let form = layout_xml(&xml).unwrap();
    let values: Vec<Option<String>> = form.pages.iter().flat_map(|p| widgets(p)).map(|w| w.value.clone()).collect();
    assert_eq!(values, ["a", "b", "c", "d"].map(|v| Some(v.to_string())));
}

/// What a `match="none"` subform holds is adopted by the enclosing data scope (XFA 3.3,
/// "Transparent Nodes", p. 193), so same-named objects from different `match="none"` subforms,
/// and from the instances of a repeating one, take successive data nodes instead of sharing one.
#[test]
fn same_named_fields_adopted_from_match_none_subforms_take_successive_nodes() {
    let body = format!(
        r#"<subform name="first" layout="tb"><bind match="none"/><field name="amount" {TEXT}</field></subform>
<subform name="second" layout="tb"><bind match="none"/><field name="amount" {TEXT}</field></subform>
<subform name="row" layout="tb"><bind match="none"/><occur min="1" max="-1" initial="2"/><field name="amount" {TEXT}</field></subform>
<field name="amount" {TEXT}</field>"#
    );
    let xml = xdp(&body, "<amount>1</amount><amount>2</amount><amount>3</amount><amount>4</amount><amount>5</amount>");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].first[0].amount[0]"), "form[0].amount[0]");
    assert_eq!(path(&tpl, "form[0].second[0].amount[0]"), "form[0].amount[1]");
    assert_eq!(path(&tpl, "form[0].row[0].amount[0]"), "form[0].amount[2]");
    assert_eq!(path(&tpl, "form[0].row[1].amount[0]"), "form[0].amount[3]");
    assert_eq!(path(&tpl, "form[0].amount[0]"), "form[0].amount[4]");
    let form = layout_xml(&xml).unwrap();
    let values: Vec<Option<String>> = form.pages.iter().flat_map(|p| widgets(p)).map(|w| w.value.clone()).collect();
    assert_eq!(values, ["1", "2", "3", "4", "5"].map(|v| Some(v.to_string())));
}

/// The data index counts the siblings before an object by where they are, so a `match="none"`
/// sibling after a repeating subform doesn't shift that subform's instances.
#[test]
fn a_match_none_sibling_after_a_repeating_subform_leaves_its_instances_alone() {
    let body = format!(
        r#"<subform name="row" layout="tb"><occur min="1" max="-1"/><field name="qty" {TEXT}</field></subform>
<subform name="row" layout="tb"><bind match="none"/><field name="note" {TEXT}</field></subform>"#
    );
    let xml = xdp(&body, "<row><qty>a</qty></row><row><qty>b</qty></row><row><qty>c</qty></row>");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].row[2].qty[0]"), "form[0].row[2].qty[0]");
}

/// A container bound by a `[*]` reference is the data scope of what it holds: the second such
/// container binds the second instance, and a `[*]` reference inside it counts from that
/// instance's own first node again.
#[test]
fn star_references_inside_star_bound_containers_count_in_their_own_instance() {
    let inner = |n: &str| {
        format!(
            r#"<subform name="{n}" layout="tb"><bind match="dataRef" ref="$.item[*]"/><field name="v{n}" {TEXT}<bind match="dataRef" ref="$.v[*]"/></field></subform>"#
        )
    };
    let xml = xdp(&format!("{}{}", inner("a"), inner("b")), "<item><v>a0</v><v>a1</v></item><item><v>b0</v><v>b1</v></item>");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].b[0].vb[0]"), "form[0].item[1].v[0]");
    let form = layout_xml(&xml).unwrap();
    assert_eq!(value_at(&form, "vb[0]").as_deref(), Some("b0"));
}

/// A signed index in a reference (`[+1]`) is relative to the object's own instance, which this
/// binding doesn't follow: the object binds by name rather than to an absolute instance.
#[test]
fn a_signed_index_in_a_reference_is_not_read_as_an_absolute_one() {
    let xml = xdp(&format!(r#"<field name="x" {TEXT}<bind match="dataRef" ref="$.line[+1].amount"/></field>"#), "");
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].x[0]"), "form[0].x[0]");
}

/// The root subform binds by its own reference too, and what it holds binds under that node.
#[test]
fn the_root_subform_binds_by_its_reference() {
    let xml = xdp(&format!(r#"<field name="x" {TEXT}</field>"#), "")
        .replace(r#"<subform name="form" layout="tb">"#, r#"<subform name="form" layout="tb"><bind match="dataRef" ref="$data.actual"/>"#);
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].x[0]"), "actual[0].x[0]");
}

/// An object bound by a reference it follows takes no node named after it, so a same-named
/// object after it that binds by name takes the first such node.
#[test]
fn a_sibling_bound_by_its_reference_takes_no_data_index() {
    let body = format!(
        r#"<field name="x" {TEXT}<bind match="dataRef" ref="$.other"/></field>
<field name="x" {TEXT}</field>
<field name="x" {TEXT}<bind match="dataRef" ref="$form.elsewhere"/></field>
<field name="x" {TEXT}</field>"#
    );
    let (tpl, _) = parse(&xdp(&body, "")).unwrap();
    assert_eq!(path(&tpl, "form[0].x[1]"), "form[0].x[0]");
    // A reference that can't be followed binds by name, so it does take a node.
    assert_eq!(path(&tpl, "form[0].x[2]"), "form[0].x[1]");
    assert_eq!(path(&tpl, "form[0].x[3]"), "form[0].x[2]");
}

/// `[*]` numbering settles however deeply containers bound by `[*]` nest, up to the nesting
/// limit: here two chains of ten, each level bound by an absolute reference of its own, so the
/// second chain takes the second instance at every level.
#[test]
fn star_numbering_settles_through_deep_nesting() {
    let chain = |c: &str| {
        let mut s = format!(r#"<field name="v{c}" {TEXT}<bind match="dataRef" ref="$.v[*]"/></field>"#);
        for level in (0..10).rev() {
            s = format!(r#"<subform name="{c}{level}" layout="tb"><bind match="dataRef" ref="$data.l{level}[*]"/>{s}</subform>"#);
        }
        s
    };
    let xml = xdp(&format!("{}{}", chain("a"), chain("b")), "");
    let (tpl, _) = parse(&xml).unwrap();
    let som = format!("form[0].{}.vb[0]", (0..10).map(|l| format!("b{l}[0]")).collect::<Vec<_>>().join("."));
    // Each level binds by an absolute reference, so each settles in a pass of its own.
    let want = "l9[1].v[0]";
    assert_eq!(path(&tpl, &som), want);
}

/// Resolving a data path through `match="none"` sections looks at the siblings on the way, not
/// at everything they hold: scripts resolve one for each field they touch, and a form with
/// hundreds of such sections ran out of its script time when each lookup walked them all.
#[test]
fn data_paths_through_many_match_none_sections_stay_quick() {
    let section = |i: usize| {
        let fields: String = (0..40).map(|f| format!(r#"<field name="f{f}" {TEXT}</field>"#)).collect();
        format!(
            r#"<subform name="s{i}" layout="tb"><bind match="none"/><subform name="mid" layout="tb"><bind match="none"/><subform name="inner" layout="tb"><bind match="none"/>{fields}</subform></subform></subform>"#
        )
    };
    let body: String = (0..1000).map(section).collect();
    let (tpl, _) = parse(&xdp(&body, "")).unwrap();
    let started = std::time::Instant::now();
    let mut last = String::new();
    for i in 0..1000 {
        for f in [0, 13, 26, 39] {
            last = path(&tpl, &format!("form[0].s{i}[0].mid[0].inner[0].f{f}[0]"));
        }
    }
    assert_eq!(last, "form[0].f39[999]", "each section's field takes the next node of its name");
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "took {:?}", started.elapsed());
}

/// The root comes first in the form's order, so its own `[*]` reference takes the first
/// instance and an object inside it with the same reference takes the next.
#[test]
fn the_root_takes_the_first_instance_of_its_star_reference() {
    let xml =
        xdp(&format!(r#"<subform name="inner" layout="tb"><bind match="dataRef" ref="$data.item[*]"/><field name="x" {TEXT}</field></subform>"#), "")
            .replace(r#"<subform name="form" layout="tb">"#, r#"<subform name="form" layout="tb"><bind match="dataRef" ref="$data.item[*]"/>"#);
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].inner[0].x[0]"), "item[1].x[0]");
}

/// `$record` is the root subform's data node: when the root binds by a reference, the node
/// that reference names, not a group named after the root.
#[test]
fn record_is_the_node_the_root_binds_to() {
    let xml = xdp(&format!(r#"<field name="x" {TEXT}<bind match="dataRef" ref="$record.code"/></field>"#), "")
        .replace(r#"<subform name="form" layout="tb">"#, r#"<subform name="form" layout="tb"><bind match="dataRef" ref="$data.actual"/>"#);
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].x[0]"), "actual[0].code[0]");
}

/// Master-page objects with the same `[*]` reference take its instances in turn, as content
/// objects do.
#[test]
fn master_page_objects_take_the_instances_of_a_star_reference_in_turn() {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="1in" w="7.5in" h="9in"/><medium short="8.5in" long="11in"/>
  <field name="a" x="0.5in" y="0.2in" {TEXT}<bind match="dataRef" ref="$record.codes.code[*]"/></field>
  <field name="b" x="3in" y="0.2in" {TEXT}<bind match="dataRef" ref="$record.codes.code[*]"/></field>
 </pageArea></pageSet>
 <field name="body" {TEXT}</field>
</subform>
</template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><codes><code>K1</code><code>K2</code></codes></form></xfa:data></xfa:datasets>
</xdp:xdp>"#
    );
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "front[0].a[0]"), "form[0].codes[0].code[0]");
    assert_eq!(path(&tpl, "front[0].b[0]"), "form[0].codes[0].code[1]");
}

/// A repeating subform after a `match="none"` sibling that holds an object of its name starts
/// at the data node after that object's, and lays out only the instances the data has left.
#[test]
fn a_repeating_subform_after_an_adopted_namesake_counts_only_the_rest() {
    let body = format!(
        r#"<subform name="wrap" layout="tb"><bind match="none"/><subform name="row" layout="tb"><field name="qty" {TEXT}</field></subform></subform>
<subform name="row" layout="tb"><occur min="1" max="-1"/><field name="qty" {TEXT}</field></subform>"#
    );
    let xml = xdp(&body, "<row><qty>a</qty></row><row><qty>b</qty></row><row><qty>c</qty></row>");
    let form = layout_xml(&xml).unwrap();
    let values: Vec<Option<String>> = form.pages.iter().flat_map(|p| widgets(p)).map(|w| w.value.clone()).collect();
    assert_eq!(values, ["a", "b", "c"].map(|v| Some(v.to_string())), "no empty fourth row");
}

/// The root's own reference is resolved once: what the root holds binds under the node it
/// names, and `$record` is that same node, even when the reference starts from `$record`.
#[test]
fn a_root_reference_from_record_is_resolved_once() {
    let body = format!(r#"<field name="x" {TEXT}</field><field name="y" {TEXT}<bind match="dataRef" ref="$record.code"/></field>"#);
    let xml = xdp(&body, "")
        .replace(r#"<subform name="form" layout="tb">"#, r#"<subform name="form" layout="tb"><bind match="dataRef" ref="$record.actual"/>"#);
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].x[0]"), "form[0].actual[0].x[0]");
    assert_eq!(path(&tpl, "form[0].y[0]"), "form[0].actual[0].code[0]");
}

/// The root's own `[*]` reference counts against the nodes it matches before the root binds:
/// a child whose absolute reference matches the same nodes takes the next instance.
#[test]
fn a_root_star_reference_from_record_takes_the_first_instance() {
    let body =
        format!(r#"<subform name="inner" layout="tb"><bind match="dataRef" ref="$data.form.row[*]"/><field name="x" {TEXT}</field></subform>"#);
    let xml = xdp(&body, "")
        .replace(r#"<subform name="form" layout="tb">"#, r#"<subform name="form" layout="tb"><bind match="dataRef" ref="$record.row[*]"/>"#);
    let (tpl, _) = parse(&xml).unwrap();
    assert_eq!(path(&tpl, "form[0].inner[0].x[0]"), "form[0].row[1].x[0]");
}

/// A repeating subform bound by a reference that names one node (no `[*]`) has that one node:
/// one instance when the data has it, however many groups the data holds of the subform's name.
#[test]
fn a_repeating_subform_bound_to_one_node_lays_out_once() {
    let body = format!(
        r#"<subform name="row" layout="tb"><occur min="0" max="-1"/><bind match="dataRef" ref="$record.actual[0]"/><field name="qty" {TEXT}</field></subform>"#
    );
    let xml = xdp(&body, "<row/><row/><row/><actual><qty>7</qty></actual>");
    let form = layout_xml(&xml).unwrap();
    let values: Vec<Option<String>> = form.pages.iter().flat_map(|p| widgets(p)).map(|w| w.value.clone()).collect();
    assert_eq!(values, [Some("7".to_string())]);
}
