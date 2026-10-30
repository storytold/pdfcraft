//! Occurrence audit across layout strategies. XFA 3.3 ch. 7 pp. 263–268 and
//! ch. 8 pp. 327–330: merge counts apply before layout; positioned objects keep
//! their offsets, row cells use successive columns, and extents include all instances.

use crate::layout::{Form, Item, Widget};
use crate::{layout, parse, parse_datasets};

fn widgets(form: &Form) -> Vec<&Widget> {
    form.pages.iter().flat_map(|p| &p.items).filter_map(|i| if let Item::Widget(w) = i { Some(w.as_ref()) } else { None }).collect()
}

fn render(body: &str, data: Option<&str>) -> Form {
    let (tpl, _) = parse(&format!(r#"<template><subform name="form" layout="tb" w="300pt">{body}</subform></template>"#)).unwrap();
    let data = data.map(|xml| {
        parse_datasets(&format!(r#"<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data>{xml}</xfa:data></xfa:datasets>"#))
            .unwrap()
    });
    layout(&tpl, data.as_ref()).unwrap()
}

fn counts_in(mode: &str) {
    for fixed in [false, true] {
        for (initial, minimum, records, expected) in [
            (0, 0, None, 0),
            (1, 0, None, 1),
            (3, 0, None, 3),
            (3, 0, Some(0), 0),
            (3, 0, Some(1), 1),
            (0, 0, Some(3), 3),
            (0, 0, Some(4), 3),
            (3, 2, Some(1), 2),
        ] {
            let height = if fixed { r#" h="100pt""# } else { "" };
            let child_mode = if mode == "table" { "row" } else { "tb" };
            let repeated = format!(
                r#"<subform name="rep" layout="{child_mode}" w="60pt"><occur min="{minimum}" initial="{initial}" max="3"/>
                <field name="entry" w="60pt" h="12pt"><ui><textEdit/></ui><value><text>default</text></value></field></subform>"#
            );
            let box_xml = if mode == "area" {
                format!(r#"<area name="box">{repeated}</area>"#)
            } else {
                format!(r#"<subform name="box" layout="{mode}" w="300pt" columnWidths="60pt 60pt 60pt"{height}>{repeated}</subform>"#)
            };
            let body = if mode == "row" {
                format!(r#"<subform name="table" layout="table" columnWidths="60pt 60pt 60pt">{box_xml}</subform>"#)
            } else {
                box_xml
            };
            let data = records.map(|n| {
                let rows = (0..n).map(|i| format!("<rep><entry>record {i}</entry></rep>")).collect::<String>();
                let container = if mode == "area" { rows } else { format!("<box>{rows}</box>") };
                let container = if mode == "row" { format!("<table>{container}</table>") } else { container };
                format!("<form>{container}</form>")
            });
            let form = render(&body, data.as_deref());
            let fields = widgets(&form);
            assert_eq!(fields.len(), expected, "{mode}, fixed={fixed}, initial={initial}, min={minimum}, records={records:?}");
            let mut soms = std::collections::HashSet::new();
            for (i, field) in fields.iter().enumerate() {
                assert!(field.som.ends_with(&format!("rep[{i}].entry[0]")), "{}", field.som);
                assert!(soms.insert(&field.som), "duplicate identity");
                let value = if records.is_some_and(|n| i < n) { format!("record {i}") } else { "default".into() };
                assert_eq!(field.value.as_deref(), Some(value.as_str()), "{mode}: {}", field.som);
            }
        }
    }
}

#[test]
fn positioned_counts_match_blank_and_data_merges() {
    counts_in("position");
}
#[test]
fn area_counts_match_blank_and_data_merges() {
    counts_in("area");
}
#[test]
fn vertical_counts_match_blank_and_data_merges() {
    counts_in("tb");
}
#[test]
fn horizontal_counts_match_blank_and_data_merges() {
    counts_in("lr-tb");
    counts_in("rl-tb");
}
#[test]
fn table_counts_match_blank_and_data_merges() {
    counts_in("table");
}
#[test]
fn cell_counts_match_blank_and_data_merges() {
    counts_in("row");
}

/// The later instance can be the largest. The following horizontal and vertical
/// siblings expose both dimensions, independent of drawing all the missing widgets.
#[test]
fn positioned_instances_contribute_their_maximum_extents() {
    for area in [false, true] {
        for reverse in [false, true] {
            let repeated = r#"<subform name="rep" layout="tb" x="7pt" y="11pt"><occur min="0" max="3"/>
                <subform name="wide" layout="tb" w="140pt"><occur min="0" initial="0" max="1"/>
                    <field name="extra" w="140pt" h="60pt"><ui><textEdit/></ui></field>
                </subform><field name="entry" w="40pt" h="10pt"><ui><textEdit/></ui></field></subform>"#;
            let container = if area {
                format!(r#"<area name="box">{repeated}</area>"#)
            } else {
                format!(r#"<subform name="box" layout="position">{repeated}</subform>"#)
            };
            let body = format!(
                r#"<subform name="line" layout="lr-tb" w="300pt">{container}<field name="beside" w="10pt" h="10pt"><ui><textEdit/></ui></field></subform>
                <field name="below" w="10pt" h="10pt"><ui><textEdit/></ui></field>"#
            );
            let rows = if reverse { "<rep><wide/></rep><rep/>" } else { "<rep/><rep><wide/></rep>" };
            let data = if area { format!("<form><line>{rows}</line></form>") } else { format!("<form><line><box>{rows}</box></line></form>") };
            let form = render(&body, Some(&data));
            let fields = widgets(&form);
            let beside = fields.iter().find(|f| f.name == "beside").unwrap();
            let below = fields.iter().find(|f| f.name == "below").unwrap();
            assert!((beside.rect.x - 36.0 - 147.0).abs() < 0.01, "area={area} reverse={reverse}: {beside:?}");
            assert!((below.rect.y - 36.0 - 81.0).abs() < 0.01, "area={area} reverse={reverse}: {below:?}");
            assert_eq!(fields.iter().filter(|f| f.som.ends_with("entry[0]")).count(), 2);
        }
    }
}

/// Each repeated cell consumes its own span and contributes its own height. A later
/// ordinary cell must follow all instances, rather than share a column with one.
#[test]
fn repeated_cells_use_distinct_columns_spans_and_data() {
    for fixed in [false, true] {
        let height = if fixed { r#" h="100pt""# } else { "" };
        let body = format!(
            r#"<subform name="table" layout="table" w="300pt" columnWidths="40pt 50pt 60pt 70pt 80pt"{height}>
            <subform name="row" layout="row">
                <subform name="cell" layout="tb" colSpan="2"><occur min="0" max="3"/>
                    <field name="entry"><ui><textEdit multiLine="1"/></ui></field>
                </subform><field name="last" h="10pt"><ui><textEdit/></ui></field>
            </subform></subform><field name="after" w="100pt" h="10pt"><ui><textEdit/></ui></field>"#
        );
        let text = "line1\nline2\nline3\nline4\nline5\nline6\nline7";
        let form = render(
            &body,
            Some(&format!("<form><table><row><cell><entry>short</entry></cell><cell><entry>{text}</entry></cell></row></table></form>")),
        );
        let fields = widgets(&form);
        assert_eq!(fields.len(), 4, "fixed={fixed}");
        assert_eq!(fields[0].value.as_deref(), Some("short"));
        assert_eq!(fields[1].value.as_deref(), Some(text));
        for (field, x, w) in [(fields[0], 36.0, 90.0), (fields[1], 126.0, 130.0), (fields[2], 256.0, 80.0)] {
            assert!((field.rect.x - x).abs() < 0.01 && (field.rect.w - w).abs() < 0.01, "{field:?}");
        }
        assert!(fields[1].rect.h > fields[0].rect.h + 20.0, "{fields:?}");
        assert!((fields[2].rect.bottom() - fields[1].rect.bottom()).abs() < 0.01);
        let bottom = if fixed { 136.0 } else { fields[1].rect.bottom() };
        assert!((fields[3].rect.y - bottom).abs() < 0.01);
    }
}

#[test]
fn positioned_empty_repeats_are_still_bounded() {
    let level = r#"<subform layout="position" w="1pt" h="1pt"><occur initial="50" max="-1"/>"#;
    let xml = format!("<template><subform>{}{}</subform></template>", level.repeat(4), "</subform>".repeat(4));
    assert!(matches!(crate::layout_xml(&xml), Err(crate::XfaError::TooLarge(_))));
}

#[test]
fn inferred_table_columns_include_repeated_cell_spans() {
    for fixed in ["", r#" h="100pt""#] {
        for span in [1, 2] {
            let body = format!(
                r#"<subform name="table" layout="table" w="300pt"{fixed}><subform name="row" layout="row">
            <subform name="cell" layout="tb" colSpan="{span}"><occur min="0" initial="2" max="3"/>
                <field name="entry" h="20pt"><ui><textEdit/></ui></field></subform>
            <field name="last" h="20pt"><ui><textEdit/></ui></field></subform></subform>"#
            );
            let form = render(&body, None);
            let fields = widgets(&form);
            assert_eq!(fields.len(), 3);
            let unit = 300.0 / f64::from(2 * span + 1);
            for (i, field) in fields.iter().enumerate() {
                let w = if i < 2 { unit * f64::from(span) } else { unit };
                assert!(
                    (field.rect.x - 36.0 - i as f64 * unit * f64::from(span)).abs() < 0.01 && (field.rect.w - w).abs() < 0.01,
                    "{fixed}: {field:?}"
                );
            }
        }
    }
}

#[test]
fn inferred_columns_scan_each_rows_own_data() {
    let body = r#"<subform name="table" layout="table" w="300pt"><subform name="row" layout="row"><occur min="0" max="3"/>
        <subform name="cell" layout="tb"><occur min="0" max="3"/><field name="entry" h="20pt"><ui><textEdit/></ui></field></subform>
        <field name="last" h="20pt"><ui><textEdit/></ui></field></subform></subform>"#;
    let form = render(body, Some("<form><table><row/><row><cell/><cell/></row></table></form>"));
    let fields = widgets(&form);
    assert_eq!(fields.len(), 4);
    assert!(fields.iter().all(|f| (f.rect.w - 100.0).abs() < 0.01));
    assert_eq!(fields[0].rect.y, 36.0);
    assert_eq!(fields[3].rect.y, 56.0);
    assert_eq!(fields[3].rect.x, 236.0);
}

/// Tables may also contain ordinary flowed sections. Their children are not cells
/// and must not narrow the columns used by actual rows.
#[test]
fn inferred_columns_ignore_non_row_sections() {
    for fixed in ["", r#" h="100pt""#] {
        let body = format!(
            r#"<subform name="table" layout="table" w="300pt"{fixed}>
            <subform name="intro" layout="tb">
                <field name="one" w="40pt" h="10pt"><ui><textEdit/></ui></field>
                <field name="two" w="40pt" h="10pt"><ui><textEdit/></ui></field>
                <field name="three" w="40pt" h="10pt"><ui><textEdit/></ui></field></subform>
            <subform name="row" layout="row">
                <field name="left" h="20pt"><ui><textEdit/></ui></field>
                <field name="right" h="20pt"><ui><textEdit/></ui></field></subform></subform>"#
        );
        let form = render(&body, None);
        let fields = widgets(&form);
        assert_eq!(fields.len(), 5);
        for (field, x) in [(fields[3], 36.0), (fields[4], 186.0)] {
            assert_eq!((field.rect.x, field.rect.w), (x, 150.0), "{fixed}: {field:?}");
        }
    }
}

#[test]
fn inferred_column_allocation_is_bounded() {
    let cell = r#"<subform colSpan="64" layout="tb"><occur initial="50" max="-1"/></subform>"#;
    let xml = format!(r#"<template><subform layout="table"><subform layout="row">{}</subform></subform></template>"#, cell.repeat(126));
    assert!(matches!(crate::layout_xml(&xml), Err(crate::XfaError::TooLarge(_))));
}

#[test]
fn truncated_positioned_and_cell_instances_are_reported() {
    for mode in ["position", "row"] {
        let box_xml = format!(
            r#"<subform name="box" layout="{mode}" w="300pt"><subform name="rep" layout="tb">
            <occur min="0" max="2"/><field name="entry" w="30pt" h="10pt"><ui><textEdit/></ui></field></subform></subform>"#
        );
        let (body, data) = if mode == "row" {
            (format!(r#"<subform name="table" layout="table">{box_xml}</subform>"#), "<form><table><box><rep/><rep/><rep/></box></table></form>")
        } else {
            (box_xml, "<form><box><rep/><rep/><rep/></box></form>")
        };
        let form = render(&body, Some(data));
        assert_eq!(widgets(&form).len(), 2);
        assert!(form.warnings.iter().any(|w| w.contains("data holds 3 rows; only the first 2")), "{mode}: {:?}", form.warnings);
    }
}

#[test]
fn page_master_occurrences_have_independent_values() {
    for count in [0, 1, 3] {
        let body = format!(
            r#"<pageSet><pageArea name="master"><contentArea x="36pt" y="80pt" w="300pt" h="600pt"/>
            <subform name="rep" layout="tb" x="20pt" y="10pt" w="60pt"><occur min="0" initial="{count}" max="3"/>
                <field name="entry" w="60pt" h="12pt"><ui><textEdit/></ui></field></subform></pageArea></pageSet>"#
        );
        let form = render(&body, None);
        assert_eq!(widgets(&form).len(), count);
        let records = (0..count).map(|i| format!("<rep><entry>master {i}</entry></rep>")).collect::<String>();
        let form = render(&body, Some(&format!("<master>{records}</master>")));
        let fields = widgets(&form);
        assert_eq!(fields.len(), count);
        for (i, field) in fields.iter().enumerate() {
            assert_eq!(field.som, format!("master[0].rep[{i}].entry[0]"));
            assert_eq!(field.value.as_deref(), Some(format!("master {i}").as_str()));
            assert_eq!((field.rect.x, field.rect.y), (20.0, 10.0));
        }
    }
}

#[test]
fn positioned_repeats_keep_anchors_and_presence() {
    for (presence, visible, occupied) in [("visible", true, true), ("invisible", false, true), ("hidden", false, false)] {
        let body = format!(
            r#"<subform name="box" layout="position" w="300pt">
            <subform name="rep" layout="tb" w="40pt" h="20pt" x="90pt" y="50pt" anchorType="bottomRight" presence="{presence}">
                <occur min="0" initial="3" max="3"/><field name="entry" w="40pt" h="20pt"><ui><textEdit/></ui></field>
            </subform></subform><field name="after" w="40pt" h="10pt"><ui><textEdit/></ui></field>"#
        );
        let form = render(&body, None);
        let fields = widgets(&form);
        assert_eq!(fields.len(), if visible { 4 } else { 1 });
        for field in fields.iter().filter(|f| f.name != "after") {
            assert_eq!((field.rect.x, field.rect.y), (86.0, 66.0));
        }
        assert_eq!(fields.last().unwrap().rect.y, if occupied { 86.0 } else { 36.0 });
    }
}
