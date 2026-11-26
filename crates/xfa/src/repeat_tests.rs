//! Repeating subforms with many instances: every row the data holds is laid out, up to the
//! template's `occur max` and the layout's instance limit, past which the rows left out are
//! reported (XFA 3.3, "The occur element", p. 786; Chapter 9, p. 336).

use super::*;
use crate::layout::MAX_INSTANCES;
use crate::model::Occur;

/// A form with a flowed list (`table` false) or a table (`table` true) whose `item` row
/// repeats with `occur max`, and data holding `rows` rows, each 20pt tall: 36 to a page.
fn rows_form(max: &str, rows: usize, table: bool) -> String {
    let (list, row) = if table { (r#"layout="table" columnWidths="200pt""#, r#"layout="row""#) } else { (r#"layout="tb""#, r#"layout="tb""#) };
    let data: String = (1..=rows).map(|i| format!("<item><qty>row {i}</qty></item>")).collect();
    format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="p"><occur max="-1"/><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
 <subform name="list" {list}>
  <subform name="item" {row}><occur min="1" max="{max}"/><field name="qty" w="200pt" h="20pt"><ui><textEdit/></ui></field></subform>
 </subform>
</subform></template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><list>{data}</list></form></xfa:data></xfa:datasets>
</xdp:xdp>"#
    )
}

fn qty_widgets(pages: &[Page]) -> Vec<&Widget> {
    pages.iter().flat_map(|p| p.items.iter()).filter_map(|it| if let Item::Widget(w) = it { Some(w.as_ref()) } else { None }).collect()
}

/// The values of the laid-out `qty` fields, in order.
fn values(form: &Form) -> Vec<String> {
    qty_widgets(&form.pages).iter().map(|w| w.value.clone().unwrap_or_default()).collect()
}

fn expected(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("row {i}")).collect()
}

fn cut_warning(form: &Form) -> Option<&String> {
    form.warnings.iter().find(|w| w.contains("rows; only the first"))
}

#[test]
fn every_data_row_past_fifty_is_laid_out_and_paginated() {
    for table in [false, true] {
        for (rows, pages) in [(60, 2), (300, 9)] {
            let form = layout_xml(&rows_form("-1", rows, table)).unwrap();
            assert_eq!(values(&form), expected(rows), "table {table}, {rows} rows");
            assert_eq!(form.pages.len(), pages, "table {table}, {rows} rows at 36 a page");
            assert_eq!(cut_warning(&form), None, "nothing is cut: {:?}", form.warnings);
            // Rows stay inside the content area of their page.
            for w in qty_widgets(&form.pages) {
                assert!(w.rect.y >= 36.0 - 0.01 && w.rect.bottom() <= 756.0 + 0.01, "{} at {:?}", w.name, w.rect);
            }
        }
    }
}

#[test]
fn occur_max_still_stops_the_rows_and_says_so() {
    for table in [false, true] {
        let form = layout_xml(&rows_form("5", 60, table)).unwrap();
        assert_eq!(values(&form), expected(5), "table {table}");
        let w = cut_warning(&form).unwrap_or_else(|| panic!("the 55 rows past occur max are reported: {:?}", form.warnings));
        assert!(w.contains("holds 60 rows") && w.contains("first 5") && !w.contains("the most"), "occur max is the form's own rule: {w}");
    }
}

#[test]
fn rows_past_the_instance_limit_are_cut_with_a_warning() {
    for table in [false, true] {
        // Exactly at the limit: all laid out, nothing reported.
        let form = layout_xml(&rows_form("-1", MAX_INSTANCES, table)).unwrap();
        assert_eq!(values(&form).len(), MAX_INSTANCES, "table {table}");
        assert_eq!(cut_warning(&form), None, "{:?}", form.warnings);
        // One past it: the last row is left out, and the warning says so.
        let form = layout_xml(&rows_form("-1", MAX_INSTANCES + 1, table)).unwrap();
        assert_eq!(values(&form), expected(MAX_INSTANCES), "table {table}");
        let w = cut_warning(&form).unwrap_or_else(|| panic!("rows past the limit are reported: {:?}", form.warnings));
        assert!(w.contains(&format!("holds {} rows", MAX_INSTANCES + 1)) && w.contains(&format!("first {MAX_INSTANCES}")), "{w}");
        assert!(w.contains(&format!("{MAX_INSTANCES} is the most one repeating subform lays out")), "the limit is named: {w}");
    }
}

#[test]
fn an_initial_count_lays_out_whole_up_to_the_limit() {
    // `initial` comes from the template, not the data: past fifty it lays out whole, and a
    // hostile count stops at the limit (the parser reads it up to there).
    let tpl = |initial: &str| {
        rows_form("-1", 0, false).replace(r#"<occur min="1" max="-1"/>"#, &format!(r#"<occur min="1" initial="{initial}" max="-1"/>"#))
    };
    let form = layout_xml(&tpl("300")).unwrap();
    assert_eq!(qty_widgets(&form.pages).len(), 300);
    assert_eq!(form.pages.len(), 9);
    let form = layout_xml(&tpl("1000000000")).unwrap();
    assert_eq!(qty_widgets(&form.pages).len(), MAX_INSTANCES);
    assert_eq!(crate::layout::instance_count(&Occur { initial: usize::MAX, ..Occur::default() }, 0), 1, "max defaults to 1");
    let unbounded = Occur { initial: usize::MAX, max: None, ..Occur::default() };
    assert_eq!(crate::layout::instance_count(&unbounded, 0), MAX_INSTANCES);
    assert_eq!(crate::layout::instance_count(&unbounded, usize::MAX), MAX_INSTANCES);
}

#[test]
fn rows_cut_in_a_positioned_container_are_reported_too() {
    // The list sits in a positioned subform, so it is drawn rather than flowed: its `tb`,
    // `lr-tb` and `rl-tb` repeats are cut at the same bounds and must say so as a flowed one does.
    for layout in ["tb", "lr-tb", "rl-tb"] {
        let xml = rows_form("5", 60, false)
            .replace(r#"<subform name="form" layout="tb">"#, r#"<subform name="form" layout="tb"><subform name="page" w="540pt" h="720pt">"#)
            .replace("</subform></template>", "</subform></subform></template>")
            .replace(r#"<subform name="list" layout="tb">"#, &format!(r#"<subform name="list" x="0pt" y="0pt" w="540pt" layout="{layout}">"#))
            .replace("<xfa:data><form><list>", "<xfa:data><form><page><list>")
            .replace("</list></form></xfa:data>", "</list></page></form></xfa:data>");
        let form = layout_xml(&xml).unwrap();
        assert_eq!(values(&form), expected(5), "{layout}");
        let w = cut_warning(&form).unwrap_or_else(|| panic!("{layout}: the rows past occur max are reported: {:?}", form.warnings));
        assert!(w.contains("holds 60 rows") && w.contains("first 5"), "{layout}: {w}");
    }
}
