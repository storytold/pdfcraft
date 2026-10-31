//! A table (`layout="table"`, its rows `layout="row"`) placed in positioned content repeats a
//! row subform once per instance, as a table in flowed content does: the template's `occur`
//! and the data's rows decide how many (XFA 3.3, "Tables", p. 327; "Repeating Elements using
//! Occurrence Limits", p. 263; Chapter 9, p. 336), and the data rows past `occur max` or the
//! layout's own limit are reported, not dropped silently.

use super::*;
use crate::layout::Item;
use crate::model::Rect;

/// Where the table sits.
#[derive(Clone, Copy, Debug)]
enum Place {
    /// At its own `x`/`y` in a positioned subform.
    Positioned,
    /// In a top-to-bottom subform at a position in a positioned subform, with a field after it.
    PositionedFlow,
    /// In the flowed body, with a field after it.
    Flowed,
}

/// A form with a table whose header row (`head`, shown once) and footer row (`foot`) frame a
/// repeating `item` row with `occur` attributes `occur`, and data holding `rows` items (and two
/// header rows, which the template shows once). Each row is 20pt tall.
fn table_form(place: Place, occur: &str, rows: usize) -> String {
    let items: String = (1..=rows).map(|i| format!("<item><qty>row {i}</qty></item>")).collect();
    let table = r#"<subform name="table" layout="table" columnWidths="200pt" POS><border><edge/></border>
     <subform name="head" layout="row"><field name="label" w="200pt" h="20pt"><ui><textEdit/></ui></field></subform>
     <subform name="item" layout="row"><occur OCCUR/><field name="qty" w="200pt" h="20pt"><ui><textEdit/></ui></field></subform>
     <subform name="foot" layout="row"><field name="total" w="200pt" h="20pt"><ui><textEdit/></ui></field></subform>
    </subform>"#
        .replace("OCCUR", occur);
    let after = r#"<field name="after" w="200pt" h="20pt"><ui><textEdit/></ui></field>"#;
    let (page, data) = match place {
        Place::Positioned => {
            (format!(r#"<subform name="page1" w="540pt" h="700pt">{}</subform>"#, table.replace("POS", r#"x="36pt" y="72pt""#)), "page1")
        }
        Place::PositionedFlow => (
            format!(
                r#"<subform name="page1" w="540pt" h="700pt"><subform name="wrap" layout="tb" w="200pt" x="36pt" y="72pt">{}{after}</subform></subform>"#,
                table.replace("POS", "")
            ),
            "page1><wrap",
        ),
        Place::Flowed => (format!(r#"<subform name="page1" layout="tb" w="540pt">{}{after}</subform>"#, table.replace("POS", "")), "page1"),
    };
    let close: String = data.split("><").collect::<Vec<_>>().iter().rev().map(|n| format!("</{n}>")).collect();
    format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="p"><occur max="-1"/><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
 {page}
</subform></template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><{data}><table><head><label>H</label></head><head><label>H2</label></head>{items}<foot><total>T</total></foot></table>{close}</form></xfa:data></xfa:datasets>
</xdp:xdp>"#
    )
}

/// A laid-out widget: (field name, SOM, value, rect).
type Laid = (String, String, Option<String>, Rect);

/// The laid-out widgets, in order.
fn laid_out(form: &Form) -> Vec<Laid> {
    form.pages
        .iter()
        .flat_map(|p| p.items.iter())
        .filter_map(|it| if let Item::Widget(w) = it { Some((w.name.clone(), w.som.clone(), w.value.clone(), w.rect)) } else { None })
        .collect()
}

fn named<'a>(all: &'a [Laid], field: &str) -> Vec<&'a Laid> {
    all.iter().filter(|w| w.1.ends_with(&format!(".{field}[0]"))).collect()
}

fn qty_values(all: &[Laid]) -> Vec<String> {
    named(all, "qty").iter().map(|w| w.2.clone().unwrap_or_default()).collect()
}

fn expected(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("row {i}")).collect()
}

fn cut_warning(form: &Form) -> Option<&String> {
    form.warnings.iter().find(|w| w.contains("rows; only the first"))
}

/// Header, the rows and footer one under another, 20pt apart, from the header's top.
fn assert_stacked(all: &[Laid], rows: usize, what: &str) {
    let head = named(all, "label");
    assert_eq!(head.len(), 1, "{what}: the header row is shown once: {head:?}");
    let top = head[0].3.y;
    let qty = named(all, "qty");
    assert_eq!(qty.len(), rows, "{what}");
    for (i, w) in qty.iter().enumerate() {
        assert!((w.3.y - (top + 20.0 * (i as f64 + 1.0))).abs() < 0.01, "{what}: row {i} at {:?}, header at {top}", w.3);
    }
    let foot = named(all, "total");
    assert_eq!(foot.len(), 1, "{what}: the footer row is shown once");
    assert!((foot[0].3.y - (top + 20.0 * (rows as f64 + 1.0))).abs() < 0.01, "{what}: footer at {:?} after {rows} rows", foot[0].3);
}

#[test]
fn a_table_in_positioned_content_gets_a_row_per_data_row() {
    for place in [Place::Positioned, Place::PositionedFlow] {
        let form = layout_xml(&table_form(place, r#"min="1" max="-1""#, 5)).unwrap();
        let all = laid_out(&form);
        assert_eq!(qty_values(&all), expected(5), "{place:?}");
        let soms: Vec<&str> = named(&all, "qty").iter().map(|w| w.1.as_str()).collect();
        let prefix = if matches!(place, Place::Positioned) { "form[0].page1[0].table[0]" } else { "form[0].page1[0].wrap[0].table[0]" };
        let want: Vec<String> = (0..5).map(|i| format!("{prefix}.item[{i}].qty[0]")).collect();
        assert_eq!(soms, want, "{place:?}: each row with its own SOM index");
        assert_stacked(&all, 5, &format!("{place:?}"));
        assert_eq!(named(&all, "label")[0].2.as_deref(), Some("H"), "{place:?}: the header takes the first header data row");
        assert!(cut_warning(&form).is_none(), "{place:?}: {:?}", form.warnings);
    }
}

#[test]
fn a_table_in_positioned_content_is_measured_with_its_rows() {
    // What follows the table in a top-to-bottom container comes after the footer, not after one
    // row: the table is measured with every instance.
    let form = layout_xml(&table_form(Place::PositionedFlow, r#"min="1" max="-1""#, 5)).unwrap();
    let all = laid_out(&form);
    let foot = named(&all, "total")[0].3;
    let after = named(&all, "after")[0].3;
    assert!((after.y - foot.bottom()).abs() < 0.01, "after {after:?}, footer {foot:?}");
}

#[test]
fn occur_max_bounds_a_positioned_table_and_the_rows_cut_are_reported() {
    for place in [Place::Positioned, Place::PositionedFlow, Place::Flowed] {
        let form = layout_xml(&table_form(place, r#"min="1" max="3""#, 5)).unwrap();
        let all = laid_out(&form);
        assert_eq!(qty_values(&all), expected(3), "{place:?}");
        assert_stacked(&all, 3, &format!("{place:?}"));
        let warning = cut_warning(&form).unwrap_or_else(|| panic!("{place:?}: {:?}", form.warnings));
        assert!(warning.contains("item") && warning.contains("5 rows") && warning.contains("first 3"), "{place:?}: {warning}");
    }
}

#[test]
fn without_data_rows_a_positioned_table_takes_its_initial_count() {
    let form = layout_xml(&table_form(Place::Positioned, r#"min="1" max="-1" initial="2""#, 0)).unwrap();
    let all = laid_out(&form);
    assert_stacked(&all, 2, "initial 2");
    assert!(cut_warning(&form).is_none(), "{:?}", form.warnings);
}

#[test]
fn a_positioned_table_lays_out_the_rows_a_flowed_one_does() {
    // One rule for both: the same rows, values, spacing and warnings, wherever the table sits.
    for (occur, rows) in [(r#"min="1" max="-1""#, 5), (r#"min="2" max="4""#, 7), (r#"min="1" max="-1""#, 0), (r#"min="3""#, 1)] {
        let flowed = layout_xml(&table_form(Place::Flowed, occur, rows)).unwrap();
        let positioned = layout_xml(&table_form(Place::Positioned, occur, rows)).unwrap();
        let (f, p) = (laid_out(&flowed), laid_out(&positioned));
        assert_eq!(qty_values(&p), qty_values(&f), "{occur} with {rows} rows");
        let offsets = |all: &[Laid]| -> Vec<f64> {
            let top = named(all, "label").first().map_or(0.0, |w| w.3.y);
            all.iter().filter(|w| w.0 != "after").map(|w| w.3.y - top).collect()
        };
        assert_eq!(offsets(&p), offsets(&f), "{occur} with {rows} rows");
        assert_eq!(cut_warning(&positioned).is_some(), cut_warning(&flowed).is_some(), "{occur} with {rows} rows");
    }
}

#[test]
fn hostile_occur_values_and_data_floods_lay_a_positioned_table_out_within_bounds() {
    for occur in [
        r#"min="-3" max="-7" initial="99999999999999999999""#,
        r#"min="abc" max="1e9" initial="-1""#,
        r#"min="999999" max="999999" initial="999999""#,
        r#"max="0""#,
        r#"min="0" max="0" initial="0""#,
    ] {
        for rows in [0, 4] {
            let form = layout_xml(&table_form(Place::Positioned, occur, rows)).unwrap();
            let flowed = layout_xml(&table_form(Place::Flowed, occur, rows)).unwrap();
            assert_eq!(qty_values(&laid_out(&form)).len(), qty_values(&laid_out(&flowed)).len(), "{occur} with {rows} rows");
        }
    }
    // More data rows than the layout lays out: bounded, and said.
    let rows = 1_200;
    let form = layout_xml(&table_form(Place::Positioned, r#"min="1" max="-1""#, rows)).unwrap();
    let n = qty_values(&laid_out(&form)).len();
    assert!((1..rows).contains(&n), "{n} of {rows} rows");
    let warning = cut_warning(&form).unwrap_or_else(|| panic!("{:?}", form.warnings));
    assert!(warning.contains(&format!("{rows} rows")) && warning.contains(&format!("first {n}")), "{warning}");
}

/// A repeating positioned `entry` subform, each instance holding a positioned table whose `item`
/// rows repeat, with a field after the entries; the data gives entry `i` `rows[i]` rows.
fn nested_form(place: Place, rows: &[usize]) -> String {
    let entries: String = rows
        .iter()
        .enumerate()
        .map(|(e, n)| {
            let items: String = (1..=*n).map(|i| format!("<item><qty>{e}.{i}</qty></item>")).collect();
            format!("<entry><table>{items}</table></entry>")
        })
        .collect();
    let entry = r#"<subform name="entry" w="200pt"><occur min="1" max="-1"/>
     <subform name="table" layout="table" columnWidths="200pt" x="0pt" y="0pt">
      <subform name="item" layout="row"><occur min="1" max="-1"/><field name="qty" w="200pt" h="20pt"><ui><textEdit/></ui></field></subform>
     </subform></subform>"#;
    let after = r#"<field name="after" w="200pt" h="20pt"><ui><textEdit/></ui></field>"#;
    let (page, open, close) = match place {
        Place::PositionedFlow => (
            format!(
                r#"<subform name="page1" w="540pt" h="700pt"><subform name="wrap" layout="tb" w="200pt" x="36pt" y="72pt">{entry}{after}</subform></subform>"#
            ),
            "<page1><wrap>",
            "</wrap></page1>",
        ),
        _ => (format!(r#"<subform name="page1" layout="tb" w="540pt">{entry}{after}</subform>"#), "<page1>", "</page1>"),
    };
    format!(
        r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="p"><occur max="-1"/><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
 {page}
</subform></template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form>{open}{entries}{close}</form></xfa:data></xfa:datasets>
</xdp:xdp>"#
    )
}

#[test]
fn each_instance_of_a_repeating_container_is_measured_with_its_own_rows() {
    // Instances of one repeating subform hold different numbers of rows in the data (XFA 3.3,
    // "Tables", p. 327; Chapter 9, p. 336): each takes the height of its own rows, so the rows
    // of one never run into the next instance or what follows.
    for place in [Place::Flowed, Place::PositionedFlow] {
        for rows in [[1, 5], [5, 1], [2, 3]] {
            let what = format!("{place:?} {rows:?}");
            let form = layout_xml(&nested_form(place, &rows)).unwrap();
            let all = laid_out(&form);
            let qty = named(&all, "qty");
            let want: Vec<String> = rows.iter().enumerate().flat_map(|(e, n)| (1..=*n).map(move |i| format!("{e}.{i}"))).collect();
            assert_eq!(qty.iter().map(|w| w.2.clone().unwrap_or_default()).collect::<Vec<_>>(), want, "{what}");
            let top = qty[0].3.y;
            for (i, w) in qty.iter().enumerate() {
                assert!((w.3.y - (top + 20.0 * i as f64)).abs() < 0.01, "{what}: row {i} at {:?}, first at {top}", w.3);
            }
            let after = named(&all, "after")[0].3;
            let last = qty[qty.len() - 1].3;
            assert!((after.y - last.bottom()).abs() < 0.01, "{what}: after {after:?}, last row {last:?}");
        }
    }
}

#[test]
fn each_row_instance_takes_the_height_of_its_own_growing_text() {
    // A repeating row whose cell grows with its text (a multi-line field without a height) is
    // as tall as its own instance's value (XFA 3.3, "Tables", p. 327; "Growable Containers",
    // p. 271): a long value makes its row taller and pushes the rows after it down, wherever
    // the table sits.
    let long = "a long value that wraps over several lines of the cell ".repeat(6);
    for place in [Place::Positioned, Place::PositionedFlow, Place::Flowed] {
        let xml = table_form(place, r#"min="1" max="-1""#, 3)
            .replace(
                r#"<field name="qty" w="200pt" h="20pt"><ui><textEdit/></ui></field>"#,
                r#"<field name="qty" w="200pt" minH="20pt"><ui><textEdit multiLine="1"/></ui></field>"#,
            )
            .replace("<qty>row 2</qty>", &format!("<qty>{long}</qty>"));
        let form = layout_xml(&xml).unwrap();
        let all = laid_out(&form);
        let qty = named(&all, "qty");
        assert_eq!(qty.len(), 3, "{place:?}");
        let (short, tall) = (qty[0].3, qty[1].3);
        assert!(tall.h > short.h + 20.0, "{place:?}: the long row {tall:?} is no taller than a short one {short:?}");
        for pair in qty.windows(2) {
            let (a, b) = (pair[0].3, pair[1].3);
            assert!((b.y - a.bottom()).abs() < 0.01, "{place:?}: row at {b:?} after {a:?}");
        }
        let foot = named(&all, "total")[0].3;
        assert!((foot.y - qty[2].3.bottom()).abs() < 0.01, "{place:?}: footer {foot:?} after {:?}", qty[2].3);
        if let Some(after) = named(&all, "after").first() {
            assert!((after.3.y - foot.bottom()).abs() < 0.01, "{place:?}: after {:?}, footer {foot:?}", after.3);
        }
    }
}

/// A multi-line `qty` text field 100pt wide that grows with its text, with `access` and
/// template value `value`.
fn qty_cell(access: &str, value: &str) -> String {
    format!(
        r#"<field name="qty" w="100pt" minH="20pt" access="{access}"><ui><textEdit multiLine="1"/></ui><value><text>{value}</text></value></field>"#
    )
}

/// [`table_form`] with two same-named cells per `item` row, `qty[0]` and `qty[1]` (`first` and
/// `second`), in two 100pt columns, and the data of row `i` holding `data[i]`.
fn same_name_cells_form(place: Place, first: &str, second: &str, data: [&str; 3]) -> String {
    let mut xml = table_form(place, r#"min="1" max="-1""#, 3)
        .replace(r#"columnWidths="200pt""#, r#"columnWidths="100pt 100pt""#)
        .replace(r#"<field name="qty" w="200pt" h="20pt"><ui><textEdit/></ui></field>"#, &format!("{first}{second}"));
    for (i, d) in data.iter().enumerate() {
        xml = xml.replace(&format!("<item><qty>row {}</qty></item>", i + 1), &format!("<item>{d}</item>"));
    }
    xml
}

#[test]
fn a_row_is_as_tall_as_its_tallest_same_named_cell() {
    // Each cell of a row is measured under its own SOM index among its same-named siblings,
    // the index it is drawn and bound at (XFA 3.3, "Tables", p. 327; "Growable Containers",
    // p. 271; Chapter 9, p. 336): a short `qty[0]` beside a long `qty[1]` makes the row as tall
    // as the long one, and the rows and content after it start below it.
    let long = "a long value that wraps over several lines of the cell".repeat(6);
    let cell = qty_cell("open", "");
    for place in [Place::Positioned, Place::PositionedFlow, Place::Flowed] {
        let data = ["<qty>s1</qty><qty>t1</qty>", &format!("<qty>s2</qty><qty>{long}</qty>"), "<qty>s3</qty><qty>t3</qty>"];
        let form = layout_xml(&same_name_cells_form(place, &cell, &cell, data)).unwrap();
        let all = laid_out(&form);
        let first = named(&all, "qty");
        let second: Vec<&Laid> = all.iter().filter(|w| w.1.ends_with(".qty[1]")).collect();
        assert_eq!(first.iter().map(|w| w.2.clone().unwrap_or_default()).collect::<Vec<_>>(), ["s1", "s2", "s3"], "{place:?}");
        assert_eq!(second.len(), 3, "{place:?}");
        assert_eq!(second[1].2.as_deref().map(str::trim), Some(long.trim()), "{place:?}");
        let tall = second[1].3;
        assert!(tall.h > 60.0, "{place:?}: the long cell {tall:?} did not grow");
        // Both cells of the row take its height, and the next row starts below it.
        assert!((first[1].3.h - tall.h).abs() < 0.01, "{place:?}: qty[0] {:?} beside qty[1] {tall:?}", first[1].3);
        for cells in [&first, &second] {
            for pair in cells.windows(2) {
                let (a, b) = (pair[0].3, pair[1].3);
                assert!((b.y - a.bottom()).abs() < 0.01, "{place:?}: row at {b:?} after {a:?}");
            }
        }
        let foot = named(&all, "total")[0].3;
        assert!((foot.y - second[2].3.bottom()).abs() < 0.01, "{place:?}: footer {foot:?} after {:?}", second[2].3);
        if let Some(after) = named(&all, "after").first() {
            assert!((after.3.y - foot.bottom()).abs() < 0.01, "{place:?}: after {:?}, footer {foot:?}", after.3);
        }
    }
}

#[test]
fn a_long_non_interactive_same_named_cell_stays_in_its_row() {
    // A non-interactive `qty[1]` is drawn as text, not as a widget; measured under its own SOM
    // index (where the data holds nothing, so its long template value is what shows), each row
    // holds its lines, and none run into the row after it or the footer.
    let long = "longword ".repeat(12);
    for place in [Place::Positioned, Place::PositionedFlow, Place::Flowed] {
        let data = ["<qty>s1</qty>", "<qty>s2</qty>", "<qty>s3</qty>"];
        let form = layout_xml(&same_name_cells_form(place, &qty_cell("open", ""), &qty_cell("nonInteractive", &long), data)).unwrap();
        let all = laid_out(&form);
        let rows: Vec<Rect> = named(&all, "qty").iter().map(|w| w.3).collect();
        assert_eq!(rows.len(), 3, "{place:?}");
        let lines: Vec<f64> = form
            .pages
            .iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|it| if let Item::Text(s) = it { Some(s) } else { None })
            .filter(|s| s.text.contains("longword"))
            .map(|s| s.baseline)
            .collect();
        assert!(lines.len() >= 6, "{place:?}: {lines:?}");
        let per_row: Vec<usize> = rows.iter().map(|r| lines.iter().filter(|&&b| b > r.y && b <= r.bottom()).count()).collect();
        assert_eq!(per_row.iter().sum::<usize>(), lines.len(), "{place:?}: lines {lines:?} outside rows {rows:?}");
        assert!(per_row.iter().all(|&n| n == per_row[0] && n > 1), "{place:?}: lines per row {per_row:?}");
        let foot = named(&all, "total")[0].3;
        assert!((foot.y - rows[2].bottom()).abs() < 0.01, "{place:?}: footer {foot:?} after {:?}", rows[2]);
    }
}
