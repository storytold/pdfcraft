//! The PDF names of the fields and radio groups a dynamic form is laid out into stay with their
//! objects when the form is laid out again: a row added before a field, or a section shown before
//! it, gives the new objects new names and leaves every other name where it was.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object, PdfString};

use super::*;
use crate::fixtures::shell;
use crate::script::{Overrides, fields_by_som, rerender, set_overrides};

/// A form whose `row` subform repeats (`rows` rows in the data), each row holding radio group
/// `R` (options 1 and 2), followed by a `tax` section, hidden when `tax_hidden`, holding another
/// group `R` (options 1 and 2), and then group `R` of the form itself (options 1 and 0). Every
/// group is named `R`, so all but the first get a made-up name.
fn groups_form(rows: usize, tax_hidden: bool) -> String {
    let data: String = (0..rows).map(|_| "<row><R/></row>").collect();
    let presence = if tax_hidden { r#" presence="hidden""# } else { "" };
    let group = |a: &str, b: &str| {
        format!(
            r#"<exclGroup name="R" w="2in" h="0.3in" layout="lr-tb"><field name="a" w="0.5in" h="0.3in"><ui><checkButton shape="round"/></ui><items><integer>{a}</integer></items></field><field name="b" w="0.5in" h="0.3in"><ui><checkButton shape="round"/></ui><items><integer>{b}</integer></items></field></exclGroup>"#
        )
    };
    format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
 <subform name="row" layout="tb"><occur min="1" max="-1"/>{row}</subform>
 <subform name="tax" layout="tb"{presence}>{tax}</subform>
 {own}
</subform></template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form>{data}</form></xfa:data></xfa:datasets>
</xdp:xdp>"#,
        row = group("1", "2"),
        tax = group("1", "2"),
        own = group("1", "0"),
    )
}

/// The name of the generated field or radio group at SOM path `som`.
fn name_at(doc: &Document, som: &str) -> Option<String> {
    fields_by_som(doc).get(som).cloned()
}

/// Add rows to the `row` subform until there are `rows`, as an `addInstance` does, and lay the
/// form out again.
fn grow_rows(doc: &mut Document, rows: usize) {
    let tpl = crate::script::template_of(doc).unwrap();
    let parent = data_path(&tpl, "form[0]");
    write_data_ops(doc, &[DataOp::Instances { parent, name: "row".into(), count: rows }], None).unwrap();
    rerender(doc, &tpl).unwrap();
}

#[test]
fn a_row_added_before_a_radio_group_leaves_its_name_alone() {
    let mut doc = Document::open(Arc::new(shell(&groups_form(1, false)))).unwrap();
    render_into(&mut doc).unwrap();
    let own = "form[0].R[0]";
    let before = name_at(&doc, own).expect("the form's own group has a field");
    assert_eq!(name_at(&doc, "form[0].row[0].R[0]").as_deref(), Some("R"), "the first group keeps the template's name");
    assert_ne!(before, "R");

    grow_rows(&mut doc, 2);
    assert_eq!(name_at(&doc, own), Some(before.clone()), "the form's own group keeps its name after a row is added before it");
    assert_eq!(name_at(&doc, "form[0].row[0].R[0]").as_deref(), Some("R"));
    assert_eq!(name_at(&doc, "form[0].tax[0].R[0]"), Some("R_2".into()));
    let added = name_at(&doc, "form[0].row[1].R[0]").expect("the new row's group has a field");
    assert!(!["R", "R_2", before.as_str()].contains(&added.as_str()), "the new row's group takes a name of its own: {added}");

    // And again: every name stays where it was.
    let names = fields_by_som(&doc);
    grow_rows(&mut doc, 3);
    let again = fields_by_som(&doc);
    for (som, name) in &names {
        assert_eq!(again.get(som), Some(name), "{som} keeps its name");
    }
    let third = again.get("form[0].row[2].R[0]").expect("the third row's group has a field");
    assert_eq!(again.values().filter(|n| *n == third).count(), 1, "{third} names one object");
}

#[test]
fn a_section_shown_before_a_radio_group_leaves_its_name_alone() {
    let mut doc = Document::open(Arc::new(shell(&groups_form(1, true)))).unwrap();
    render_into(&mut doc).unwrap();
    let own = "form[0].R[0]";
    assert_eq!(name_at(&doc, own).as_deref(), Some("R_2"), "the hidden section's group has no field yet");
    assert_eq!(name_at(&doc, "form[0].tax[0].R[0]"), None);

    // A script shows the section: its group is laid out between the other two.
    let ov = Overrides { presence: [("form[0].tax[0]".to_string(), "visible".to_string())].into_iter().collect(), ..Default::default() };
    set_overrides(&mut doc, &ov).unwrap();
    let tpl = crate::script::template_of(&doc).unwrap();
    rerender(&mut doc, &tpl).unwrap();
    assert_eq!(name_at(&doc, own).as_deref(), Some("R_2"), "the form's own group keeps its name");
    let shown = name_at(&doc, "form[0].tax[0].R[0]").expect("the shown section's group has a field");
    assert!(!["R", "R_2"].contains(&shown.as_str()), "the shown group takes a name of its own: {shown}");
}

/// A name the file holds for a generated field is kept only when the layout could have given it:
/// a field doesn't take another field's name, or a name in another shape, from the file.
#[test]
fn a_kept_name_must_be_one_the_layout_could_give() {
    let mut doc = Document::open(Arc::new(shell(&groups_form(1, false)))).unwrap();
    render_into(&mut doc).unwrap();
    let tpl = crate::script::template_of(&doc).unwrap();
    for (bad, why) in [("Other", "another base"), ("R_02", "a leading zero"), ("R_1", "below 2"), ("R_", "no number")] {
        let (_, obj) = crate::script::field_objects_by_som(&doc).get("form[0].R[0]").cloned().unwrap();
        doc.update_dict(obj, |d| d.set(b"T".to_vec(), Object::String(PdfString::text(bad)))).unwrap();
        rerender(&mut doc, &tpl).unwrap();
        let name = name_at(&doc, "form[0].R[0]").unwrap();
        assert_ne!(name, bad, "{why}");
        assert!(name.starts_with("R_"), "{why}: {name}");
    }
    // A name of the right shape that a file saved by an earlier build holds is kept.
    let (_, obj) = crate::script::field_objects_by_som(&doc).get("form[0].R[0]").cloned().unwrap();
    doc.update_dict(obj, |d| d.set(b"T".to_vec(), Object::String(PdfString::text("R_7")))).unwrap();
    rerender(&mut doc, &tpl).unwrap();
    assert_eq!(name_at(&doc, "form[0].R[0]").as_deref(), Some("R_7"));
    assert_eq!(name_at(&doc, "form[0].tax[0].R[0]").as_deref(), Some("R_2"));
}

/// Unnamed radio groups take the made-up name `group`, `group_2`, …: the group after a repeating
/// row keeps its name when rows are added before it, as each group keeps its own selection when
/// rows are added in Acrobat.
#[test]
fn unnamed_radio_groups_keep_their_names_when_rows_are_added_before_them() {
    let group = |p: &str| {
        format!(
            r#"<exclGroup layout="lr-tb" w="7in"><field name="{p}A" w="2in" h="0.3in"><ui><checkButton shape="round"/></ui><items><integer>1</integer></items></field><field name="{p}B" w="2in" h="0.3in"><ui><checkButton shape="round"/></ui><items><integer>2</integer></items></field></exclGroup>"#
        )
    };
    let xdp = format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
 {top}<subform name="row" layout="tb"><occur min="1" max="-1" initial="1"/>{row}</subform>{bottom}
</subform></template>
</xdp:xdp>"#,
        top = group("Top"),
        row = group("Row"),
        bottom = group("Bottom"),
    );
    let mut doc = Document::open(Arc::new(shell(&xdp))).unwrap();
    render_into(&mut doc).unwrap();
    // An unnamed group has no SOM step of its own: the top and bottom groups both name `form[0]`.
    let before = radio_groups(&doc);
    assert_eq!(
        before,
        [("group".to_string(), "form[0]".to_string()), ("group_2".into(), "form[0].row[0]".into()), ("group_3".into(), "form[0]".into())]
    );
    grow_rows(&mut doc, 3);
    let after = radio_groups(&doc);
    assert_eq!(
        after,
        [
            ("group".to_string(), "form[0]".to_string()),
            ("group_2".into(), "form[0].row[0]".into()),
            ("group_4".into(), "form[0].row[1]".into()),
            ("group_5".into(), "form[0].row[2]".into()),
            ("group_3".into(), "form[0]".into()),
        ],
        "the bottom group keeps group_3; the added rows' groups take new names"
    );
}

/// The radio groups of the AcroForm, in the order the layout wrote them: (name, SOM path).
fn radio_groups(doc: &Document) -> Vec<(String, String)> {
    let root = doc.root().unwrap();
    let catalog = doc.get(root);
    let acro = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap());
    let text = |d: &pdfcraft_cos::Dict, key: &[u8]| d.get(key).and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text()));
    fields
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| {
            let f = doc.resolve(f);
            let d = f.as_dict()?;
            d.contains(b"Kids").then(|| Some((text(d, b"T")?, text(d, crate::pdf::SOM_KEY)?)))?
        })
        .collect()
}
