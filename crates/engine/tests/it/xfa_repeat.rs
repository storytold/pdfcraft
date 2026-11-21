//! A dynamic XFA form whose button adds rows past fifty lays every row out and adds the pages
//! they need (XFA 3.3, "The occur element", p. 786: `max="-1"` sets no upper boundary).

use std::sync::Arc;

use pdfcraft_engine::Session;

/// A list whose `item` row (20pt, 36 to a page) repeats without bound, `data` rows in its
/// datasets, and a button that adds `add` rows in one click.
fn form(add: usize, data_rows: usize) -> Vec<u8> {
    let data: String = (1..=data_rows).map(|i| format!("<item><qty>row {i}</qty></item>")).collect();
    let datasets = if data_rows == 0 {
        String::new()
    } else {
        format!(
            r#"<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><page><list>{data}</list></page></form></xfa:data></xfa:datasets>"#
        )
    };
    let xdp = format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="p"><occur max="-1"/><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
 <subform name="page" layout="tb">
  <field name="more" w="100pt" h="20pt"><ui><button/></ui><caption><value><text>Add rows</text></value></caption>
   <event activity="click"><script contentType="application/x-javascript">for (var i = 0; i &lt; {add}; i++) list._item.addInstance(1);</script></event></field>
  <subform name="list" layout="tb">
   <subform name="item" layout="tb"><occur min="1" max="-1"/><field name="qty" w="200pt" h="20pt"><ui><textEdit/></ui></field></subform>
  </subform>
 </subform>
</subform></template>
{datasets}
</xdp:xdp>"#
    );
    pdfcraft_xfa::fixtures::shell(&xdp)
}

fn rows(s: &Session, id: pdfcraft_engine::DocId) -> usize {
    s.get(id).map_or(0, |d| d.form.iter().filter(|f| f.name.starts_with("qty")).count())
}

#[test]
fn a_button_adding_rows_past_fifty_lays_them_all_out() {
    let mut s = Session::new();
    let id = s.open("rows.pdf", None, Arc::new(form(59, 0)), None).expect("opens");
    assert_eq!(rows(&s, id), 1);
    assert_eq!(s.get(id).map(|d| d.info.pages.len()), Some(1));
    let o = s.run_javascript(id, "", Some("more")).expect("the click runs");
    assert_eq!(o.error, None);
    // The button row plus 60 rows of 20pt: 61 rows at 36 a page is two pages.
    assert_eq!(rows(&s, id), 60);
    assert_eq!(s.get(id).map(|d| d.info.pages.len()), Some(2));
    let out = s.take_js_output(id);
    assert!(!out.console.iter().any(|l| l.contains("only the first")), "no row is cut: {:?}", out.console);
}

/// A table (`layout="table"`) placed at its own position in a positioned subform, its `item` row
/// repeating with `occur max` `max`, and five rows in its data (XFA 3.3, "Tables", p. 327).
fn positioned_table(max: &str) -> Vec<u8> {
    let data: String = (1..=5).map(|i| format!("<item><qty>row {i}</qty></item>")).collect();
    let xdp = format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
 <subform name="page" w="540pt" h="700pt">
  <subform name="table" layout="table" columnWidths="200pt" x="36pt" y="72pt">
   <subform name="item" layout="row"><occur min="1" max="{max}"/><field name="qty" w="200pt" h="20pt"><ui><textEdit/></ui></field></subform>
  </subform>
 </subform>
</subform></template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><page><table>{data}</table></page></form></xfa:data></xfa:datasets>
</xdp:xdp>"#
    );
    pdfcraft_xfa::fixtures::shell(&xdp)
}

#[test]
fn a_table_in_a_positioned_subform_gets_a_row_per_data_row_up_to_its_max() {
    let mut s = Session::new();
    for (max, want) in [("-1", 5), ("3", 3)] {
        let id = s.open("table.pdf", None, Arc::new(positioned_table(max)), None).expect("opens");
        assert_eq!(rows(&s, id), want, "occur max {max}");
    }
}
