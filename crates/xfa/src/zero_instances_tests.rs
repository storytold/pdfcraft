//! Synthetic occurrence regressions (XFA 3.3, chapter 7, “Repeating Elements using
//! Occurrence Limits”). No source document or viewer assets are used here.

use crate::layout::{Form, Item, Widget};
use crate::{layout, parse, parse_datasets};

/// Collect interactive geometry from every laid-out page for visibility and spacing assertions.
fn widgets(form: &Form) -> Vec<&Widget> {
    form.pages
        .iter()
        .flat_map(|p| &p.items)
        .filter_map(|item| match item {
            Item::Widget(widget) => Some(widget.as_ref()),
            _ => None,
        })
        .collect()
}

/// Lay out a synthetic body with optional datasets under a single top-to-bottom root.
fn render(body: &str, data: Option<&str>) -> Form {
    let (template, _) = parse(&format!(r#"<template><subform name="form" layout="tb" w="300pt">{body}</subform></template>"#)).unwrap();
    let data = data.map(|xml| {
        parse_datasets(&format!(r#"<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data>{xml}</xfa:data></xfa:datasets>"#))
            .unwrap()
    });
    layout(&template, data.as_ref()).unwrap()
}

/// Zero-instance children consume no space in positioned, flowing, and table containers.
#[test]
fn zero_instances_are_absent_in_every_container_layout() {
    for mode in ["position", "tb", "lr-tb", "rl-tb", "table"] {
        for height in ["", " h=\"80pt\""] {
            let child_mode = if mode == "table" { "row" } else { "tb" };
            let body = format!(
                r#"
                <subform name="box" layout="{mode}" w="300pt"{height}>
                    <subform name="optional" layout="{child_mode}" w="120pt" h="30pt">
                        <occur min="0" initial="0" max="1"/>
                        <field name="absent" w="100pt" h="20pt"><ui><textEdit/></ui></field>
                    </subform>
                    <field name="after" w="100pt" h="20pt"><ui><textEdit/></ui></field>
                </subform>
                <field name="following" w="100pt" h="20pt"><ui><textEdit/></ui></field>"#
            );
            let form = render(&body, None);
            let fields = widgets(&form);
            assert_eq!(fields.len(), 2, "mode={mode}, height={height}: {fields:?}");
            assert!(fields[0].som.ends_with("after[0]"));
            if height.is_empty() {
                assert!((fields[1].rect.y - fields[0].rect.y - 20.0).abs() < 0.01, "zero instance must take no height: {fields:?}");
            }
        }
    }
}

/// Blank merges use initial counts; populated datasets instead obey matching records and bounds.
#[test]
fn occurrence_initial_is_for_blank_forms_and_minimum_for_data_merges() {
    let body = r#"<subform name="row" layout="tb" w="100pt" h="20pt">
        <occur min="0" initial="2" max="3"/>
        <field name="value" w="100pt" h="20pt"><ui><textEdit/></ui></field>
    </subform>"#;
    assert_eq!(widgets(&render(body, None)).len(), 2);
    assert_eq!(widgets(&render(body, Some(""))).len(), 2, "an empty data packet is a blank merge");
    assert_eq!(widgets(&render(body, Some("<form/>"))).len(), 0, "data contains no row");
    assert_eq!(widgets(&render(body, Some("<form><row/></form>"))).len(), 1);
    assert_eq!(widgets(&render(body, Some("<form><row/><row/><row/><row/></form>"))).len(), 3);
    let required = body.replace("min=\"0\"", "min=\"2\"");
    assert_eq!(widgets(&render(&required, Some("<form><row/></form>"))).len(), 2);
}

/// An optional singleton requires matching data; omitted max=0 prohibits instances.
#[test]
fn optional_singleton_is_instantiated_when_its_data_exists() {
    let body = r#"<subform name="row" layout="tb"><occur min="0" initial="0" max="1"/>
        <field name="value" w="100pt" h="20pt"><ui><textEdit/></ui></field>
    </subform>"#;
    assert_eq!(widgets(&render(body, None)).len(), 0);
    assert_eq!(widgets(&render(body, Some("<form><row><value>present</value></row></form>"))).len(), 1);
    let prohibited = body.replace(" max=\"1\"", "");
    assert_eq!(widgets(&render(&prohibited, Some("<form><row/></form>"))).len(), 0, "omitted max defaults to min=0");
}

/// Break directives inside absent instances must neither advance the cursor nor create pages.
#[test]
fn zero_instances_take_no_height_and_do_not_trigger_page_breaks() {
    let body = r#"<field name="before" w="100pt" h="20pt"><ui><textEdit/></ui></field>
        <subform name="row" layout="tb"><occur min="0" initial="0" max="-1"/>
            <breakBefore targetType="pageArea"/><breakAfter targetType="pageArea"/>
            <field name="absent" w="100pt" h="200pt"><ui><textEdit/></ui></field>
        </subform>
        <field name="after" w="100pt" h="20pt"><ui><textEdit/></ui></field>"#;
    let form = render(body, None);
    assert_eq!(form.pages.len(), 1);
    let fields = widgets(&form);
    assert_eq!(fields.len(), 2);
    assert!((fields[1].rect.y - fields[0].rect.y - 20.0).abs() < 0.01);
}

/// A table cell counts its data under its row, not under the enclosing table.
#[test]
fn an_optional_subform_cell_uses_the_rows_data_scope() {
    let body = r#"<subform name="table" layout="table" columnWidths="100pt">
        <subform name="row" layout="row">
            <subform name="cell" layout="tb"><occur min="0" initial="0" max="1"/>
                <field name="value" w="100pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>
        </subform>
    </subform>"#;
    assert_eq!(widgets(&render(body, Some("<form><table><row/></table></form>"))).len(), 0);
    let form = render(body, Some("<form><table><row><cell><value>present</value></cell></row></table></form>"));
    let fields = widgets(&form);
    assert_eq!(fields.len(), 1);
    assert!(fields[0].rect.h >= 20.0, "the row must measure its present cell: {:?}", fields[0]);
}

/// Build a section whose optional child makes it cross the full-page-height threshold.
fn shrinking_section(mode: &str, constraints: &str, height: &str) -> String {
    format!(
        r#"
        <field name="intro" w="300pt" h="300pt"><ui><textEdit/></ui></field>
        <subform name="section" layout="{mode}" w="300pt" {height}>
            {constraints}
            <field name="first" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <field name="second" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <field name="third" w="300pt" h="180pt"><ui><textEdit/></ui></field>
            <subform name="optional" w="300pt" h="240pt"><occur min="0" initial="0" max="1"/>
                <field name="extra" w="300pt" h="240pt"><ui><textEdit/></ui></field>
            </subform>
        </subform>"#
    )
}

/// Locate the first section field to distinguish splitting from moving the entire section.
fn first_field_page(form: &Form) -> usize {
    form.pages.iter().position(|page| page.items.iter().any(|item| matches!(item, Item::Widget(field) if field.som.ends_with("first[0]")))).unwrap()
}

/// Shrinking intrinsic horizontal flow below a page height must not introduce an implicit keep.
#[test]
fn removing_the_last_optional_row_does_not_push_a_flowing_section_to_the_next_page() {
    for mode in ["lr-tb", "rl-tb"] {
        let body = shrinking_section(mode, "", "");
        let populated = render(&body, Some("<form><section><optional/></section></form>"));
        let empty = render(&body, Some("<form><section/></form>"));
        assert_eq!(first_field_page(&populated), 0, "a section taller than the page starts in the space left");
        assert_eq!(first_field_page(&empty), 0, "shrinking below a full page must not move the whole section: {mode}");
        assert_eq!(empty.pages.len(), 2);
        for field in widgets(&empty) {
            assert!(field.rect.y >= 36.0 && field.rect.bottom() <= 756.01, "a field is kept whole at the break: {field:?}");
        }
    }
}

/// Compatibility with public-form pagination: keep hints do not make a growable
/// horizontal group indivisible. Assert placement and content, not only page counts.
#[test]
fn growable_horizontal_keep_hints_do_not_add_pages_after_optional_row_removal() {
    for mode in ["lr-tb", "rl-tb"] {
        for constraint in ["", r#"<keep intact="contentArea"/>"#, r#"<keep intact="pageArea"/>"#] {
            let body =
                format!("{}<field name=\"footer\" w=\"300pt\" h=\"300pt\"><ui><textEdit/></ui></field>", shrinking_section(mode, constraint, ""));
            for (data, count) in [("<form><section/></form>", 5), ("<form><section><optional/></section></form>", 6)] {
                let form = render(&body, Some(data));
                assert_eq!(first_field_page(&form), 0, "{mode}/{constraint}: start in the space left");
                assert_eq!(form.pages.len(), 2, "{mode}/{constraint}: keep must not add a page");
                let fields = widgets(&form);
                assert_eq!(fields.len(), count);
                for field in &fields {
                    assert!(field.rect.y >= 36.0 && field.rect.bottom() <= 756.01, "{field:?}");
                }
                for page in &form.pages {
                    let fields: Vec<_> = page.items.iter().filter_map(|item| if let Item::Widget(f) = item { Some(f) } else { None }).collect();
                    for pair in fields.windows(2) {
                        assert!(pair[0].rect.bottom() <= pair[1].rect.y + 0.01, "fields must not overlap: {pair:?}");
                    }
                }
            }
        }
    }
}

/// Splitting must use the next content area's origin, not force an extra physical page.
#[test]
fn horizontal_keep_hints_split_between_content_areas_without_losing_geometry() {
    for mode in ["lr-tb", "rl-tb"] {
        for intact in ["contentArea", "pageArea"] {
            let body = format!(
                r#"
                <pageSet><pageArea name="body">
                    <contentArea x="10pt" y="10pt" w="300pt" h="60pt"/>
                    <contentArea x="330pt" y="10pt" w="300pt" h="60pt"/>
                    <medium short="640pt" long="80pt"/>
                </pageArea></pageSet>
                <field name="intro" w="300pt" h="20pt"><ui><textEdit/></ui></field>
                <subform name="section" layout="{mode}" w="300pt"><keep intact="{intact}"/>
                    <field name="first" w="300pt" h="20pt"><ui><textEdit/></ui></field>
                    <field name="second" w="300pt" h="20pt"><ui><textEdit/></ui></field>
                    <field name="third" w="300pt" h="20pt"><ui><textEdit/></ui></field>
                </subform>
                <field name="footer" w="300pt" h="20pt"><ui><textEdit/></ui></field>"#
            );
            let form = render(&body, None);
            assert_eq!(form.pages.len(), 1, "{mode}/{intact}");
            let fields = widgets(&form);
            assert_eq!(fields.len(), 5);
            for (field, (x, y)) in fields.iter().zip([(10.0, 10.0), (10.0, 30.0), (10.0, 50.0), (330.0, 10.0), (330.0, 30.0)]) {
                assert!((field.rect.x - x).abs() < 0.01 && (field.rect.y - y).abs() < 0.01, "{mode}/{intact}: {field:?}");
                assert!((field.rect.h - 20.0).abs() < 0.01);
            }
        }
    }
}

/// A break between whole leaves keeps the enclosing flowed section on both areas.
#[test]
fn horizontal_leaf_breaks_preserve_parent_borders_and_fills() {
    for mode in ["lr-tb", "rl-tb"] {
        for columns in [false, true] {
            for intact in ["contentArea", "pageArea"] {
                let second = if columns { r#"<contentArea x="330pt" y="20pt" w="300pt" h="60pt"/>"# } else { "" };
                let body = format!(
                    r#"<pageSet><pageArea><contentArea x="10pt" y="10pt" w="300pt" h="60pt"/>{second}
                        <medium short="640pt" long="100pt"/></pageArea></pageSet>
                    <subform name="box" layout="tb" w="290pt">
                        <border><edge thickness="1pt"/><fill><color value="0,0,255"/></fill></border>
                        <field name="intro" w="290pt" h="40pt"><ui><textEdit/></ui></field>
                        <subform name="section" layout="{mode}" w="290pt"><keep intact="{intact}"/>
                            <field name="first" w="290pt" h="20pt"><ui><textEdit/></ui></field>
                            <field name="second" w="290pt" h="20pt"><ui><textEdit/></ui></field>
                        </subform>
                    </subform>"#
                );
                let form = render(&body, None);
                assert_eq!(form.pages.len(), if columns { 1 } else { 2 });
                let fields = widgets(&form);
                assert_eq!(fields.len(), 3);
                assert_eq!((fields[1].rect.x, fields[1].rect.y), (10.0, 50.0), "first leaf stays on the first area");
                let (last_x, last_y) = if columns { (330.0, 20.0) } else { (10.0, 10.0) };
                assert_eq!((fields[2].rect.x, fields[2].rect.y), (last_x, last_y));
                let fills: Vec<_> = form
                    .pages
                    .iter()
                    .flat_map(|p| &p.items)
                    .filter_map(|item| if let Item::Fill { rect, color } = item { (*color == [0.0, 0.0, 1.0]).then_some(*rect) } else { None })
                    .collect();
                assert_eq!(fills.len(), 2, "{mode}/{columns}/{intact}: {:?}", form.warnings);
                assert_eq!((fills[0].x, fills[0].y, fills[0].h), (10.0, 10.0, 60.0));
                assert_eq!((fills[1].x, fills[1].y, fills[1].h), (last_x, last_y, 20.0));
                assert_eq!(form.pages.iter().flat_map(|p| &p.items).filter(|i| matches!(i, Item::Line { .. })).count(), 8);
                for page in &form.pages {
                    for (index, item) in page.items.iter().enumerate() {
                        if let Item::Widget(w) = item {
                            assert!(
                                page.items[..index].iter().any(|i| matches!(i, Item::Fill { rect, .. }
                                if rect.x <= w.rect.x && rect.y <= w.rect.y && rect.bottom() >= w.rect.bottom())),
                                "fill must be under its field"
                            );
                        }
                    }
                }
                assert!(form.warnings.is_empty(), "{:?}", form.warnings);
            }
        }
    }
}

/// Wrapping the same full-width leaves in a growable horizontal group must retain the
/// existing nested-frame margins, draw order and column origins of top-to-bottom flow.
#[test]
fn horizontal_leaf_breaks_repeat_nested_margins_without_empty_frames() {
    for intro in [20, 59] {
        let fields: String = [20, 45, 20, 20]
            .iter()
            .enumerate()
            .map(|(i, h)| format!(r#"<field name="f{i}" w="270pt" h="{h}pt"><ui><textEdit/></ui></field>"#))
            .collect();
        let body = |mode| {
            format!(
                r#"
            <pageSet><pageArea><contentArea x="10pt" y="10pt" w="300pt" h="60pt"/>
                <contentArea x="330pt" y="20pt" w="300pt" h="50pt"/>
                <medium short="640pt" long="100pt"/></pageArea></pageSet>
            <field name="intro" w="300pt" h="{intro}pt"><ui><textEdit/></ui></field>
            <subform name="outer" layout="tb" w="290pt"><margin leftInset="5pt" topInset="5pt" bottomInset="3pt"/>
                <border><edge/><fill><color value="0,0,255"/></fill></border>
                <subform name="inner" layout="tb" w="280pt"><margin leftInset="10pt" topInset="7pt" bottomInset="4pt"/>
                    <border><edge/><fill><color value="0,255,0"/></fill></border>
                    <subform name="section" layout="{mode}" w="270pt"><keep intact="contentArea"/>{fields}</subform>
                </subform>
            </subform>
            <field name="footer" w="300pt" h="10pt"><ui><textEdit/></ui></field>"#
            )
        };
        let expected = render(&body("tb"), None);
        assert!(expected.warnings.is_empty(), "{:?}", expected.warnings);
        for mode in ["lr-tb", "rl-tb"] {
            let actual = render(&body(mode), None);
            assert_eq!(actual, expected, "{mode}, intro={intro}");
        }
    }
}

/// The next row may contain overlapping vertical extents. Reserve the whole connected
/// band before repeating parent margins, not just its first or shortest field.
#[test]
fn horizontal_leaf_breaks_reserve_overlapping_fields_before_top_margins() {
    for mode in ["lr-tb", "rl-tb"] {
        let body = format!(
            r#"
            <pageSet><pageArea><contentArea x="10pt" y="10pt" w="300pt" h="60pt"/>
                <contentArea x="330pt" y="20pt" w="300pt" h="50pt"/>
                <medium short="640pt" long="100pt"/></pageArea></pageSet>
            <field name="intro" w="300pt" h="40pt"><ui><textEdit/></ui></field>
            <subform name="box" layout="tb" w="300pt"><margin topInset="10pt"/>
                <border><edge/><fill><color value="0,0,255"/></fill></border>
                <subform layout="{mode}" w="300pt"><keep intact="contentArea"/>
                    <subform layout="position" w="300pt" h="45pt">
                        <field name="a" w="100pt" h="20pt"><ui><textEdit/></ui></field>
                        <field name="b" x="120pt" y="15pt" w="100pt" h="30pt"><ui><textEdit/></ui></field>
                    </subform>
                </subform>
            </subform>"#
        );
        let form = render(&body, None);
        assert_eq!(form.pages.len(), 1);
        let fields = widgets(&form);
        assert_eq!(fields.len(), 3);
        assert_eq!((fields[1].rect.x, fields[1].rect.y), (330.0, 25.0));
        assert_eq!((fields[2].rect.x, fields[2].rect.y, fields[2].rect.bottom()), (450.0, 40.0, 70.0));
        let fills: Vec<_> = form.pages[0].items.iter().filter_map(|i| if let Item::Fill { rect, .. } = i { Some(rect) } else { None }).collect();
        assert_eq!(fills.len(), 1, "no empty frame left in the first column");
        assert_eq!((fills[0].x, fills[0].y, fills[0].bottom()), (330.0, 20.0, 70.0));
        assert!(form.warnings.is_empty(), "{:?}", form.warnings);
    }
}

/// Fields, table rows and their frame borders retain their inset when flow changes columns.
#[test]
fn flowed_fields_table_rows_and_borders_follow_the_content_area_origin() {
    for layout in ["tb", "table"] {
        let fields: String = ["first", "second", "third"]
            .iter()
            .map(|name| {
                let field = format!(r#"<field name="{name}" w="280pt" h="20pt"><ui><textEdit/></ui></field>"#);
                if layout == "table" { format!(r#"<subform layout="row">{field}</subform>"#) } else { field }
            })
            .collect();
        let body = format!(
            r#"
            <pageSet><pageArea><contentArea x="10pt" y="10pt" w="300pt" h="60pt"/>
                <contentArea x="330pt" y="10pt" w="300pt" h="60pt"/>
                <medium short="640pt" long="80pt"/></pageArea></pageSet>
            <field name="intro" w="300pt" h="40pt"><ui><textEdit/></ui></field>
            <subform name="box" layout="{layout}" w="290pt" columnWidths="280pt">
                <margin leftInset="5pt"/>
                <border><fill><color value="0,0,255"/></fill></border>{fields}
            </subform>
            <field name="footer" w="300pt" h="20pt"><ui><textEdit/></ui></field>"#
        );
        let form = render(&body, None);
        assert_eq!(form.pages.len(), 1, "{layout}");
        let fields = widgets(&form);
        assert_eq!(fields.len(), 5);
        for (field, (x, y)) in fields.iter().zip([(10.0, 10.0), (15.0, 50.0), (335.0, 10.0), (335.0, 30.0), (330.0, 50.0)]) {
            assert!((field.rect.x - x).abs() < 0.01 && (field.rect.y - y).abs() < 0.01, "{layout}: {field:?}");
        }
        let fills: Vec<_> = form.pages[0]
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Fill { rect, color } if *color == [0.0, 0.0, 1.0] => Some(rect),
                _ => None,
            })
            .collect();
        assert_eq!(fills.len(), 2, "{layout}: {fills:?}");
        assert!(fills.iter().any(|r| (r.x - 10.0).abs() < 0.01 && (r.y - 50.0).abs() < 0.01));
        assert!(fills.iter().any(|r| (r.x - 330.0).abs() < 0.01 && (r.y - 10.0).abs() < 0.01));
    }
}

/// Repeated table leaders follow their rows into another column; later content can return
/// to the first column of a new page without retaining the previous column's offset.
#[test]
fn repeated_headers_and_following_fields_use_each_content_areas_origin() {
    let body = r#"<pageSet><pageArea>
        <contentArea x="10pt" y="10pt" w="300pt" h="60pt"/>
        <contentArea x="330pt" y="10pt" w="300pt" h="60pt"/>
        <medium short="640pt" long="80pt"/></pageArea></pageSet>
        <field name="intro" w="300pt" h="40pt"><ui><textEdit/></ui></field>
        <subform name="table" layout="table" columnWidths="280pt" w="290pt"><margin leftInset="5pt"/>
            <overflow leader="header"/>
            <subform name="header" layout="row"><field name="heading" w="280pt" h="10pt"><ui><textEdit/></ui></field></subform>
            <subform name="row" layout="row"><occur min="2" max="2"/>
                <field name="value" w="280pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>
        </subform>
        <field name="footer" w="300pt" h="20pt"><ui><textEdit/></ui></field>"#;
    let form = render(body, None);
    assert_eq!(form.pages.len(), 2);
    let fields = widgets(&form);
    assert_eq!(fields.len(), 6);
    for (field, (x, y)) in fields.iter().zip([(10.0, 10.0), (15.0, 50.0), (335.0, 10.0), (335.0, 20.0), (335.0, 40.0), (10.0, 10.0)]) {
        assert!((field.rect.x - x).abs() < 0.01 && (field.rect.y - y).abs() < 0.01, "{field:?}");
    }
    assert!(matches!(form.pages[1].items.last(), Some(Item::Widget(f)) if f.name == "footer"));
}

/// Fixed-height horizontal sections still move as a unit, with or without keep hints.
#[test]
fn fixed_height_horizontal_sections_still_move_whole() {
    for mode in ["lr-tb", "rl-tb"] {
        for constraint in ["", r#"<keep intact="contentArea"/>"#, r#"<keep intact="pageArea"/>"#] {
            let form = render(&shrinking_section(mode, constraint, r#"h="540pt""#), Some("<form><section/></form>"));
            assert_eq!(first_field_page(&form), 1);
        }
    }
}

/// Repeated rows must measure optional cells in each row's data scope, including
/// tables measured inside positioned containers and drawn with a fixed height.
#[test]
fn repeated_table_rows_measure_each_optional_cell() {
    for wrapper in ["tb", "position"] {
        for height in ["", " h=\"100pt\""] {
            let body = format!(
                r#"<subform name="box" layout="{wrapper}">
                <subform name="table" layout="table" columnWidths="100pt"{height}>
                    <subform name="row" layout="row"><occur min="0" max="-1"/>
                        <subform name="cell" layout="tb"><occur min="0" initial="0" max="1"/>
                            <field name="value" w="100pt" h="30pt"><ui><textEdit/></ui></field>
                        </subform>
                    </subform>
                </subform>
                <field name="after" w="100pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>"#
            );
            let form = render(
                &body,
                Some(
                    "<form><box><table><row/><row><cell><value>second</value></cell></row><row><cell><value>third</value></cell></row></table></box></form>",
                ),
            );
            let fields = widgets(&form);
            let cells: Vec<_> = fields.iter().filter(|f| f.som.ends_with("value[0]")).collect();
            assert_eq!(cells.len(), 2, "wrapper={wrapper}, height={height}: {fields:?}");
            assert!(cells[0].rect.h >= 30.0 && cells[1].rect.h >= 30.0);
            assert!(cells[1].rect.y >= cells[0].rect.bottom() - 0.01);
            if wrapper == "tb" {
                let after = fields.iter().find(|f| f.som.ends_with("after[0]")).unwrap();
                assert!(after.rect.y >= cells[1].rect.bottom() - 0.01);
            }
        }
    }
}

/// Later optional cells drive page breaks and leave room below repeated headers.
#[test]
fn optional_table_cells_paginate_with_repeated_headers() {
    let body = r#"<pageSet><pageArea name="body">
        <contentArea x="10pt" y="10pt" w="300pt" h="70pt"/>
        <medium short="320pt" long="100pt"/>
        </pageArea></pageSet>
        <subform name="table" layout="table" columnWidths="100pt"><overflow leader="header"/>
            <subform name="header" layout="row"><field name="heading" h="20pt"><ui><textEdit/></ui></field></subform>
            <subform name="row" layout="row"><occur min="0" max="-1"/>
                <subform name="cell" layout="tb"><occur min="0" initial="0" max="1"/>
                    <field name="value" w="100pt" h="40pt"><ui><textEdit/></ui></field>
                </subform>
            </subform>
        </subform>"#;
    let form = render(
        body,
        Some("<form><table><row/><row><cell><value>second</value></cell></row><row><cell><value>third</value></cell></row></table></form>"),
    );
    assert_eq!(form.pages.len(), 2);
    for page in &form.pages {
        let fields: Vec<_> = page
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Widget(w) => Some(w),
                _ => None,
            })
            .collect();
        assert_eq!(fields.len(), 2, "one header and one present cell: {fields:?}");
        assert!(fields[0].som.ends_with("heading[0]"));
        assert!(fields[1].rect.y >= fields[0].rect.bottom() - 0.01);
        assert!(fields[1].rect.bottom() <= 80.01);
    }
}

/// Each repeated non-row container measures its own optional children, including
/// both data orders, nested parents, fixed drawing boxes and horizontal packing.
#[test]
fn repeated_flowing_subforms_measure_their_own_instances() {
    for outer in ["tb", "table", "lr-tb", "rl-tb"] {
        for mode in ["tb", "lr-tb", "rl-tb"] {
            for height in ["", " h=\"120pt\""] {
                for rows in ["<row/><row><opt/></row>", "<row><opt/></row><row/>"] {
                    let body = format!(
                        r#"
                    <subform name="parent" layout="tb" w="300pt"><occur min="0" max="-1"/>
                        <subform name="box" layout="{outer}" w="300pt"{height}>
                            <subform name="row" layout="{mode}" w="300pt"><occur min="0" max="-1"/>
                                <subform name="opt" layout="tb" w="300pt"><occur min="0" initial="0" max="1"/>
                                    <field name="value" w="300pt" h="30pt"><ui><textEdit/></ui></field>
                                </subform>
                                <field name="tail" w="300pt" h="10pt"><ui><textEdit/></ui></field>
                            </subform>
                        </subform>
                        <field name="after" w="300pt" h="10pt"><ui><textEdit/></ui></field>
                    </subform>"#
                    );
                    let reverse = if rows.starts_with("<row/>") { "<row><opt/></row><row/>" } else { "<row/><row><opt/></row>" };
                    let data = format!("<form><parent><box>{rows}</box></parent><parent><box>{reverse}</box></parent></form>");
                    let form = render(&body, Some(&data));
                    let fields = widgets(&form);
                    assert_eq!(fields.len(), 8);
                    for parent in 0..2 {
                        let scoped: Vec<_> = fields.iter().filter(|f| f.som.contains(&format!("parent[{parent}]"))).collect();
                        let tails: Vec<_> = scoped.iter().filter(|f| f.som.ends_with("tail[0]")).collect();
                        let value = scoped.iter().find(|f| f.som.ends_with("value[0]")).unwrap();
                        let after = scoped.iter().find(|f| f.som.ends_with("after[0]")).unwrap();
                        let present = usize::from(value.som.contains("row[1]"));
                        let start = if present == 0 { value.rect.y } else { tails[0].rect.y };
                        assert!(
                            (tails[0].rect.y - start - if present == 0 { 30.0 } else { 0.0 }).abs() < 0.01,
                            "{outer}/{mode}/{height}/{rows}: {scoped:?}"
                        );
                        assert!((tails[1].rect.y - start - 40.0).abs() < 0.01, "{outer}/{mode}/{height}/{rows}: {scoped:?}");
                        assert!(
                            (after.rect.y - start - if height.is_empty() { 50.0 } else { 120.0 }).abs() < 0.01,
                            "{outer}/{mode}/{height}/{rows}: {scoped:?}"
                        );
                    }
                }
            }
        }
    }
}

/// A later, taller horizontal instance splits at leaf boundaries using its actual height.
#[test]
fn repeated_horizontal_subforms_paginate_by_instance_height() {
    for mode in ["lr-tb", "rl-tb"] {
        let body = format!(
            r#"<pageSet><pageArea name="body">
            <contentArea x="10pt" y="10pt" w="300pt" h="60pt"/>
            <medium short="320pt" long="80pt"/>
            </pageArea></pageSet>
            <subform name="row" layout="{mode}" w="300pt"><occur min="0" max="-1"/>
                <keep intact="contentArea"/>
                <subform name="opt" layout="tb" w="300pt"><occur min="0" initial="0" max="1"/>
                    <field name="value" w="300pt" h="40pt"><ui><textEdit/></ui></field>
                </subform>
                <field name="tail" w="300pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>"#
        );
        let form = render(&body, Some("<form><row/><row><opt/></row></form>"));
        assert_eq!(form.pages.len(), 2, "{mode}");
        let fields = widgets(&form);
        assert_eq!(fields.len(), 3);
        assert!((fields[1].rect.y - 30.0).abs() < 0.01);
        assert!((fields[2].rect.y - 10.0).abs() < 0.01);
        assert_eq!(form.pages[0].items.iter().filter(|item| matches!(item, Item::Widget(_))).count(), 2);
        assert_eq!(form.pages[1].items.iter().filter(|item| matches!(item, Item::Widget(_))).count(), 1);
        assert!(fields.iter().all(|f| f.rect.bottom() <= 70.01));
    }
}

/// Same-named fields use their own data for growth, even under a positioned parent.
#[test]
fn same_named_growing_fields_measure_their_own_values() {
    for parent in ["tb", "position"] {
        let body = format!(
            r#"<subform name="box" layout="{parent}" w="300pt">
            <subform name="content" layout="tb" w="100pt">
                <field name="text" w="100pt"><ui><textEdit multiLine="1"/></ui></field>
                <field name="text" w="100pt"><ui><textEdit multiLine="1"/></ui></field>
                <field name="after" w="100pt" h="10pt"><ui><textEdit/></ui></field>
            </subform></subform>"#
        );
        let form = render(
            &body,
            Some(
                "<form><box><content><text>short</text><text>one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen</text></content></box></form>",
            ),
        );
        let fields = widgets(&form);
        assert_eq!(fields.len(), 3);
        assert!(fields[1].rect.h > fields[0].rect.h + 15.0, "{parent}: {fields:?}");
        assert!((fields[1].rect.y - fields[0].rect.bottom()).abs() < 0.01);
        assert!((fields[2].rect.y - fields[1].rect.bottom()).abs() < 0.01);
    }
}

/// An absent overflow leader must stay absent on every continuation page.
#[test]
fn zero_instance_overflow_leaders_stay_absent() {
    let body = r#"<pageSet><pageArea name="body">
        <contentArea x="10pt" y="10pt" w="300pt" h="70pt"/>
        <medium short="320pt" long="100pt"/></pageArea></pageSet>
        <subform name="table" layout="table" columnWidths="100pt"><overflow leader="header"/>
            <subform name="header" layout="row"><occur min="0" initial="0" max="1"/>
                <field name="heading" h="20pt"><ui><textEdit/></ui></field>
            </subform>
            <subform name="row" layout="row"><occur min="0" max="-1"/>
                <field name="value" h="40pt"><ui><textEdit/></ui></field>
            </subform>
        </subform>"#;
    for header in ["", "<header/>"] {
        let data = format!("<form><table>{header}<row/><row/><row/></table></form>");
        let form = render(body, Some(&data));
        assert_eq!(form.pages.len(), 3);
        for page in &form.pages {
            let fields: Vec<_> = page.items.iter().filter_map(|i| if let Item::Widget(w) = i { Some(w) } else { None }).collect();
            assert_eq!(fields.len(), if header.is_empty() { 1 } else { 2 });
            assert_eq!(fields.iter().any(|f| f.som.contains("header[")), !header.is_empty());
            assert!(fields.iter().all(|f| f.rect.bottom() <= 80.01));
        }
    }
}

/// Unnamed containers use occurrence defaults even when unrelated data is present.
#[test]
fn unnamed_subforms_keep_initial_instances_in_layout_and_runtime() {
    for initial in [0, 1, 2] {
        let body = format!(
            r#"<subform layout="tb" w="100pt"><occur min="0" initial="{initial}" max="2"/>
            <field name="value" w="100pt" h="20pt"><ui><textEdit/></ui></field></subform>"#
        );
        let (tpl, _) = parse(&format!(r#"<template><subform name="form" layout="tb">{body}</subform></template>"#)).unwrap();
        let data = parse_datasets(
            r#"<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><unrelated/></form></xfa:data></xfa:datasets>"#,
        )
        .unwrap();
        // Check runtime independently of geometry: scripts must see exactly the visible instances.
        let live = crate::form_tree(&tpl, Some(&data), &Default::default());
        let mut pending = vec![&live.root];
        let mut values = 0;
        while let Some(n) = pending.pop() {
            values += usize::from(n.name == "value");
            pending.extend(&n.children);
        }
        assert_eq!(values, initial);
        let form = render(&body, Some("<form><unrelated/></form>"));
        assert_eq!(widgets(&form).len(), initial);
    }
}

/// A partial data document cannot reduce a repeating subform below its required minimum (#4744).
#[test]
fn data_with_fewer_groups_than_occur_min_still_gives_min_instances() {
    let body = r#"<subform name="rep" layout="tb"><occur min="2" max="2"/>
        <field name="k" w="100pt" h="20pt"><ui><textEdit/></ui></field>
    </subform>"#;
    let form = render(body, Some("<form><rep><k>first</k></rep></form>"));
    let fields = widgets(&form);
    assert_eq!(fields.len(), 2);
    assert!(fields[0].som.ends_with("rep[0].k[0]"));
    assert!(fields[1].som.ends_with("rep[1].k[0]"));
    assert!((fields[1].rect.y - fields[0].rect.y - 20.0).abs() < 0.01);
}

/// Initial data groups retain empty descendants through transparent areas and unnamed wrappers,
/// and survive an ordinary PDF save/reopen without manufacturing data for the wrappers.
#[test]
fn initial_empty_groups_below_areas_and_unnamed_wrappers_survive_reopen() {
    use std::sync::Arc;

    use pdfcraft_cos::{Document, SaveOptions, write_incremental};

    let xdp = r#"<template><subform name="form" layout="tb">
        <area name="panel"><subform layout="tb">
            <subform name="outer" layout="tb"><occur min="0" initial="2" max="3"/>
                <subform name="empty"><occur min="0" initial="2" max="3"/></subform>
                <subform name="absent"><occur min="0" initial="0" max="1"/></subform>
                <field name="k" w="100pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>
        </subform></area>
    </subform></template>"#;
    let mut doc = Document::open(Arc::new(crate::fixtures::shell(xdp))).unwrap();
    crate::render_into(&mut doc).unwrap();
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(bytes)).unwrap();
    for document in [&doc, &reopened] {
        let data = crate::data_of(document).expect("the blank merge records initially created groups");
        let root = vec![("form".to_string(), 0)];
        assert_eq!(data.count(&root, "panel"), 0, "an area does not establish a data scope");
        assert_eq!(data.count(&root, "#subform"), 0);
        assert_eq!(data.count(&root, "outer"), 2);
        for i in 0..2 {
            let parent = vec![("form".to_string(), 0), ("outer".to_string(), i)];
            assert_eq!(data.count(&parent, "empty"), 2, "empty descendants still represent instances");
            assert_eq!(data.count(&parent, "absent"), 0);
        }
        let (tpl, _) = parse(xdp).unwrap();
        let form = layout(&tpl, Some(&data)).unwrap();
        assert_eq!(widgets(&form).len(), 2, "materializing data must preserve the blank layout");
    }
}

/// Recording body groups must not turn initially present master-page subforms into absent ones.
/// Master-page SOM scopes follow the runtime's existing mapping outside the body root.
#[test]
fn initial_groups_in_named_and_unnamed_page_master_scopes_are_preserved() {
    use std::sync::Arc;

    use pdfcraft_cos::Document;

    for area_name in ["", " name=\"master\""] {
        let xdp = format!(
            r#"<template><subform name="form" layout="tb">
                <pageSet><pageArea{area_name}>
                    <medium short="612pt" long="792pt"/>
                    <contentArea x="36pt" y="100pt" w="540pt" h="650pt"/>
                    <area name="header"><subform name="rep" layout="tb">
                        <occur min="0" initial="2" max="3"/>
                        <field name="k" w="100pt" h="20pt"><ui><textEdit/></ui></field>
                    </subform></area>
                </pageArea></pageSet>
                <field name="body" w="100pt" h="20pt"><ui><textEdit/></ui></field>
            </subform></template>"#
        );
        let mut doc = Document::open(Arc::new(crate::fixtures::shell(&xdp))).unwrap();
        crate::render_into(&mut doc).unwrap();
        let data = crate::data_of(&doc).expect("the blank merge preserves master-page groups as well as body groups");
        let parent = if area_name.is_empty() { vec![("header".to_string(), 0)] } else { vec![("master".to_string(), 0), ("header".to_string(), 0)] };
        assert_eq!(data.count(&parent, "rep"), 2, "master scope: {area_name:?}");
        let (tpl, _) = parse(&xdp).unwrap();
        let live = crate::form_tree(&tpl, Some(&data), &crate::Overrides::default());
        let master = live.root.children.iter().find(|n| n.name == if area_name.is_empty() { "#pageArea" } else { "master" }).unwrap();
        let header = master.children.iter().find(|n| n.name == "header").unwrap();
        assert_eq!(header.children.iter().filter(|n| n.name == "rep").count(), 2, "script snapshots retain master-page instances");
    }
}

/// An unnamed body root leaves named area segments in the existing resolver's data paths.
#[test]
fn initial_groups_below_an_unnamed_root_use_the_runtime_data_scope() {
    use std::sync::Arc;

    use pdfcraft_cos::{Document, SaveOptions, write_incremental};

    let xml = r#"<template><subform layout="tb">
        <area name="panel"><subform layout="tb">
            <subform name="outer" layout="tb"><occur min="0" initial="2" max="3"/>
                <area name="inside"><subform name="empty"><occur min="0" initial="2" max="3"/></subform></area>
                <field name="k" w="100pt" h="20pt"><ui><textEdit/></ui></field>
            </subform>
        </subform></area>
    </subform></template>"#;
    let mut doc = Document::open(Arc::new(crate::fixtures::shell(xml))).unwrap();
    crate::render_into(&mut doc).unwrap();
    let reopened = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let (tpl, _) = parse(xml).unwrap();
    for document in [&doc, &reopened] {
        let data = crate::data_of(document).unwrap();
        let panel = crate::data_path(&tpl, "panel[0]");
        assert_eq!(data.count(&panel, "outer"), 2);
        for i in 0..2 {
            let parent = crate::data_path(&tpl, &format!("panel[0].outer[{i}].inside[0]"));
            assert_eq!(data.count(&parent, "empty"), 2);
        }
        assert_eq!(widgets(&layout(&tpl, Some(&data)).unwrap()).len(), 2);
        let live = crate::form_tree(&tpl, Some(&data), &Default::default());
        let panel = live.root.children.iter().find(|n| n.name == "panel").unwrap();
        let unnamed = panel.children.first().unwrap();
        assert_eq!(unnamed.children.iter().filter(|n| n.name == "outer").count(), 2);
    }
}
