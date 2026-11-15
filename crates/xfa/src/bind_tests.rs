//! Subforms with `<bind match="none"/>`: they "will not bind to any node in the XFA Data DOM"
//! (XFA 3.3, "The bind element", *The match property*, p. 176), and are "treated like a
//! nameless subform by the data binding process" while scripts still reach them by name
//! ("Transparent Nodes", p. 194). So what they hold binds in the enclosing data scope.

use super::*;
use crate::layout::Item;
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
