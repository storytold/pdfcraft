//! PivotTables: write → read round trips, package structure, hostile pivot parts.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

use gridcraft_core::date::serial_from_ymd;
use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::*;

use super::minimal;
use crate::xml;
use crate::{read_xlsx, write_xlsx};

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}
fn rr(a: &str) -> RangeRef {
    RangeRef::parse(a).unwrap()
}

fn date(y: i64, m: i64, d: i64) -> f64 {
    serial_from_ymd(gridcraft_core::DateSystem::D1900, y, m, d).unwrap()
}

fn field(name: &str) -> PivotField {
    PivotField { source_col: name.into(), ..PivotField::default() }
}

/// Source data on "Data" (A1:F9, also table "SalesTbl" and defined name "SalesData"), three
/// PivotTables on "Report" with a few hand-written report cells.
fn workbook() -> Workbook {
    let mut wb = Workbook::new();
    let date_style = wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new("m/d/yyyy"));
    let data = wb.sheet_mut(0).unwrap();
    data.name = "Data".into();
    let headers = ["Region", "Product", "Date", "Units", "Sales", "Channel"];
    for (c, h) in headers.iter().enumerate() {
        data.cells.set(CellRef::new(0, c as u32), Cell::value(Value::text(*h)));
    }
    let rows: [(&str, &str, f64, f64, f64, &str); 8] = [
        ("East", "A", date(2024, 1, 5), 3.0, 30.5, "Online"),
        ("West", "B", date(2024, 2, 9), 5.0, 50.0, "Store"),
        ("East", "B", date(2024, 3, 1), 2.0, 20.0, "Online"),
        ("North", "C", date(2025, 4, 12), 7.0, 70.25, "Store"),
        ("west", "A", date(2025, 5, 30), 1.0, 10.0, "Online"),
        ("East", "C", date(2025, 6, 2), 4.0, 40.0, "Phone"),
        ("North", "A", date(2024, 7, 7), 6.0, 60.0, "Online"),
        ("West", "C", date(2025, 12, 31), 8.0, 80.0, "Store"),
    ];
    for (r, (a, b, d, u, s, ch)) in rows.iter().enumerate() {
        let r = r as u32 + 1;
        data.cells.set(CellRef::new(r, 0), Cell::value(Value::text(*a)));
        data.cells.set(CellRef::new(r, 1), Cell::value(Value::text(*b)));
        data.cells.set(CellRef::new(r, 2), Cell { value: Value::number(*d), formula: None, style: date_style, py: None });
        data.cells.set(CellRef::new(r, 3), Cell::value(Value::number(*u)));
        data.cells.set(CellRef::new(r, 4), Cell::value(Value::number(*s)));
        data.cells.set(CellRef::new(r, 5), Cell::value(Value::text(*ch)));
    }
    data.tables.push(Table {
        id: 1,
        name: "SalesTbl".into(),
        range: rr("A1:F9"),
        header_row: true,
        totals_row: false,
        columns: headers
            .iter()
            .map(|h| TableColumn { name: (*h).into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None })
            .collect(),
        style: "TableStyleMedium2".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    wb.names.push(DefinedName { name: "SalesData".into(), scope: None, formula: "Data!$A$1:$F$9".into(), comment: String::new(), hidden: false });

    let mut report = Sheet::new("Report");
    // Pivot 1: tabular, page filter, custom and built-in value formats, collapsed item, Enable
    // show details off.
    let p1 = PivotTable {
        id: 1,
        name: "Sales by Region".into(),
        source: "Data!$A$1:$F$9".into(),
        anchor: at("A4"),
        rows: vec![PivotField { source_col: "Region".into(), sort: PivotSort::Asc, collapsed_items: vec!["East".into()], ..PivotField::default() }],
        columns: vec![PivotField { sort: PivotSort::None, ..field("Product") }],
        values: vec![
            PivotValue {
                source_col: "Sales".into(),
                func: PivotFunc::Sum,
                name: "Sum of Sales".into(),
                number_format: Some("#,##0.00".into()),
                ..PivotValue::default()
            },
            PivotValue {
                source_col: "Units".into(),
                func: PivotFunc::Count,
                name: "Count of Units".into(),
                number_format: Some("0.0\" u\"".into()),
                ..PivotValue::default()
            },
        ],
        filters: vec![PivotFilter { source_col: "Channel".into(), selected: Some(vec!["Online".into()]) }],
        layout: PivotLayout::Tabular,
        enable_show_details: false,
        grand_totals_rows: false,
        grand_totals_cols: true,
        subtotals: false,
        subtotals_top: false,
        style: "PivotStyleMedium9".into(),
        show_headers: false,
        last_range: Some(rr("A2:H12")),
        calculated_fields: vec![],
        data_index: Default::default(),
        values_on_rows: false,
        values_position: None,
    };
    // Pivot 2: table source, date grouping on two levels, show-values-as, multi-select page
    // filter and a hidden-items filter on a column field.
    let p2 = PivotTable {
        id: 2,
        name: "Trend".into(),
        source: "SalesTbl".into(),
        anchor: at("K4"),
        rows: vec![
            PivotField { source_col: "Date".into(), date_group: PivotDateGroup::Years, sort: PivotSort::None, collapsed_items: vec!["2024".into()] },
            PivotField { source_col: "Date".into(), date_group: PivotDateGroup::Months, sort: PivotSort::Asc, collapsed_items: vec![] },
        ],
        columns: vec![PivotField { sort: PivotSort::Desc, ..field("Product") }],
        values: vec![
            PivotValue {
                source_col: "Sales".into(),
                func: PivotFunc::Sum,
                name: "Share".into(),
                show_as: PivotShowAs::PercentOfGrandTotal,
                number_format: None,
            },
            PivotValue { source_col: "Sales".into(), func: PivotFunc::Max, name: "Rank".into(), show_as: PivotShowAs::Rank, number_format: None },
            PivotValue {
                source_col: "Sales".into(),
                func: PivotFunc::Average,
                name: "Running".into(),
                show_as: PivotShowAs::RunningTotal,
                number_format: None,
            },
            PivotValue {
                source_col: "Units".into(),
                func: PivotFunc::StdDevP,
                name: "Spread".into(),
                show_as: PivotShowAs::PercentOfColumnTotal,
                number_format: None,
            },
            PivotValue {
                source_col: "Units".into(),
                func: PivotFunc::VarP,
                name: "RowPct".into(),
                show_as: PivotShowAs::PercentOfRowTotal,
                number_format: None,
            },
        ],
        filters: vec![
            PivotFilter { source_col: "Region".into(), selected: Some(vec!["East".into(), "North".into()]) },
            PivotFilter { source_col: "Product".into(), selected: Some(vec!["A".into(), "B".into()]) },
        ],
        layout: PivotLayout::Outline,
        enable_show_details: true,
        grand_totals_rows: true,
        grand_totals_cols: false,
        subtotals: true,
        subtotals_top: true,
        style: "PivotStyleDark3".into(),
        show_headers: true,
        last_range: Some(rr("K2:Z30")),
        calculated_fields: vec![],
        data_index: Default::default(),
        values_on_rows: false,
        values_position: None,
    };
    // Pivot 3: defaults, defined-name source, compact.
    let p3 = PivotTable {
        id: 3,
        name: "PivotTable3".into(),
        source: "SalesData".into(),
        anchor: at("AB1"),
        rows: vec![field("Product")],
        values: vec![PivotValue { source_col: "Units".into(), func: PivotFunc::Product, name: "Product of Units".into(), ..PivotValue::default() }],
        filters: vec![PivotFilter { source_col: "Channel".into(), selected: None }],
        last_range: Some(rr("AB1:AC6")),
        ..PivotTable::default()
    };
    report.pivots = vec![p1, p2, p3];
    // Hand-written report cells (the engine's output stays plain cells).
    report.cells.set(at("A2"), Cell::value(Value::text("Channel")));
    report.cells.set(at("B2"), Cell::value(Value::text("Online")));
    report.cells.set(at("A5"), Cell::value(Value::text("East")));
    report.cells.set(at("B5"), Cell::value(Value::number(50.5)));
    wb.sheets.push(Arc::new(report));
    wb
}

fn unzip(bytes: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut m = HashMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        m.insert(f.name().to_string(), b);
    }
    m
}

#[test]
fn pivots_round_trip() {
    let wb = workbook();
    let bytes = write_xlsx(&wb).unwrap();
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.iter().all(|w| !w.to_lowercase().contains("pivot")), "{:?}", report.warnings);
    let orig = &wb.sheet(1).unwrap().pivots;
    let read = &back.sheet(1).unwrap().pivots;
    assert_eq!(read.len(), 3);
    for (a, b) in orig.iter().zip(read.iter()) {
        assert_eq!(a, b, "pivot {} differs", a.name);
    }
    // Report cells stay as written.
    let s = back.sheet(1).unwrap();
    assert_eq!(s.value(at("B5")), Value::number(50.5));
    assert_eq!(s.value(at("A5")), Value::text("East"));
    // A second trip is stable.
    let (again, _) = read_xlsx(&write_xlsx(&back).unwrap()).unwrap();
    assert_eq!(&again.sheet(1).unwrap().pivots, read);
}

#[test]
fn pivot_package_structure() {
    let parts = unzip(&write_xlsx(&workbook()).unwrap());
    assert_pivot_content_types(&parts);
    assert_relationship_targets_exist(&parts);
    let cache_ids = assert_workbook_cache_relationships(&parts);
    assert_pivot_tables_reference_caches(&parts, &cache_ids);
    assert_sheet_pivot_relationships(&parts);
}

fn assert_pivot_content_types(parts: &HashMap<String, Vec<u8>>) {
    let ct = xml::parse(parts.get("[Content_Types].xml").unwrap()).unwrap();
    let overrides: HashMap<String, String> = ct
        .kids("Override")
        .map(|o| (o.attr("PartName").unwrap().trim_start_matches('/').to_string(), o.attr("ContentType").unwrap().to_string()))
        .collect();
    let defaults: HashSet<String> = ct.kids("Default").map(|d| d.attr("Extension").unwrap().to_string()).collect();
    for part in overrides.keys() {
        assert!(parts.contains_key(part), "override for missing part {part}");
    }
    for part in parts.keys().filter(|p| p.as_str() != "[Content_Types].xml") {
        let ext = part.rsplit('.').next().unwrap();
        assert!(overrides.contains_key(part) || defaults.contains(ext), "no content type for {part}");
    }
    let count = |content_type: &str| overrides.values().filter(|value| value.as_str() == content_type).count();
    assert_eq!(count(crate::pivot::CT_PIVOT_TABLE), 3);
    assert_eq!(count(crate::pivot::CT_PIVOT_CACHE_DEF), 3);
    assert_eq!(count(crate::pivot::CT_PIVOT_CACHE_REC), 3);
}

fn assert_relationship_targets_exist(parts: &HashMap<String, Vec<u8>>) {
    let mut kinds: Vec<String> = Vec::new();
    for (name, data) in parts.iter().filter(|(name, _)| name.ends_with(".rels")) {
        let dir = name.trim_end_matches(".rels").rsplit_once("/_rels/").map(|(d, _)| d).unwrap_or("");
        for relation in xml::parse(data).unwrap().kids("Relationship") {
            if relation.attr("TargetMode") == Some("External") {
                continue;
            }
            let target = crate::package::resolve(dir, relation.attr("Target").unwrap());
            assert!(parts.contains_key(&target), "{name} points at missing {target}");
            kinds.push(relation.attr("Type").unwrap().rsplit('/').next().unwrap().to_string());
        }
    }
    for kind in ["pivotTable", "pivotCacheDefinition", "pivotCacheRecords"] {
        assert!(kinds.iter().any(|found| found == kind), "no {kind} relationship");
    }
}

fn assert_workbook_cache_relationships(parts: &HashMap<String, Vec<u8>>) -> HashSet<String> {
    let workbook = xml::parse(parts.get("xl/workbook.xml").unwrap()).unwrap();
    let relationships = xml::parse(parts.get("xl/_rels/workbook.xml.rels").unwrap()).unwrap();
    let caches: Vec<&xml::El> = workbook.child("pivotCaches").unwrap().kids("pivotCache").collect();
    assert_eq!(caches.len(), 3);
    let mut ids = HashSet::new();
    for cache in &caches {
        assert!(ids.insert(cache.attr("cacheId").unwrap().to_string()));
        let rid = cache.attr("id").unwrap();
        let relation = relationships.kids("Relationship").find(|r| r.attr("Id") == Some(rid)).unwrap();
        assert!(relation.attr("Type").unwrap().ends_with("/pivotCacheDefinition"));
    }
    ids
}

fn assert_pivot_tables_reference_caches(parts: &HashMap<String, Vec<u8>>, ids: &HashSet<String>) {
    for table_number in 1..=3 {
        let table = xml::parse(parts.get(&format!("xl/pivotTables/pivotTable{table_number}.xml")).unwrap()).unwrap();
        assert!(ids.contains(table.attr("cacheId").unwrap()));
        let table_rels = xml::parse(parts.get(&format!("xl/pivotTables/_rels/pivotTable{table_number}.xml.rels")).unwrap()).unwrap();
        let cache_path = crate::package::resolve("xl/pivotTables", table_rels.kids("Relationship").next().unwrap().attr("Target").unwrap());
        let cache = xml::parse(parts.get(&cache_path).unwrap()).unwrap();
        assert_pivot_table_fields_match_cache(&table, &cache);
        assert_pivot_cache_records_match_fields(parts, &cache, &cache_path);
    }
}

fn assert_pivot_table_fields_match_cache(table: &xml::El, cache: &xml::El) {
    let cache_fields = cache.child("cacheFields").unwrap();
    let field_count = cache_fields.kids("cacheField").count();
    assert_eq!(table.child("pivotFields").unwrap().kids("pivotField").count(), field_count);
    assert_eq!(cache_fields.attr_u32("count"), Some(field_count as u32));
    for (field_index, pivot_field) in table.child("pivotFields").unwrap().kids("pivotField").enumerate() {
        let cache_field = cache_fields.kids("cacheField").nth(field_index).unwrap();
        let item_count = cache_field
            .path(&["fieldGroup", "groupItems"])
            .map(|items| items.children.len())
            .unwrap_or_else(|| cache_field.child("sharedItems").map(|items| items.children.len()).unwrap_or(0));
        if let Some(items) = pivot_field.child("items") {
            assert_eq!(items.attr_u32("count"), Some(items.children.len() as u32));
            for item in items.kids("item") {
                if let Some(item_index) = item.attr_u32("x") {
                    assert!((item_index as usize) < item_count, "item x={item_index} out of {item_count} in field {field_index}");
                }
            }
        }
    }
}

fn assert_pivot_cache_records_match_fields(parts: &HashMap<String, Vec<u8>>, cache: &xml::El, cache_path: &str) {
    let Some(rid) = cache.attr("id") else { return };
    let rels_path = cache_path.replace("pivotCache/", "pivotCache/_rels/") + ".rels";
    let cache_rels = xml::parse(parts.get(&rels_path).unwrap()).unwrap();
    let relation = cache_rels.kids("Relationship").find(|r| r.attr("Id") == Some(rid)).unwrap();
    let records_path = crate::package::resolve("xl/pivotCache", relation.attr("Target").unwrap());
    let records = xml::parse(parts.get(&records_path).unwrap()).unwrap();
    let database_fields = cache.child("cacheFields").unwrap().kids("cacheField").filter(|f| f.attr("databaseField") != Some("0")).count();
    assert_eq!(records.attr_u32("count"), cache.attr_u32("recordCount"));
    assert_eq!(records.kids("r").count(), 8);
    for record in records.kids("r") {
        assert_eq!(record.children.len(), database_fields);
    }
}

fn assert_sheet_pivot_relationships(parts: &HashMap<String, Vec<u8>>) {
    let sheet_rels = xml::parse(parts.get("xl/worksheets/_rels/sheet2.xml.rels").unwrap()).unwrap();
    assert_eq!(sheet_rels.kids("Relationship").filter(|r| r.attr("Type").unwrap().ends_with("/pivotTable")).count(), 3);
}

#[test]
fn pivot_xml_details() {
    let parts = unzip(&write_xlsx(&workbook()).unwrap());
    let t1 = String::from_utf8(parts.get("xl/pivotTables/pivotTable1.xml").unwrap().clone()).unwrap();
    assert!(
        t1.contains("<location ref=\"A4:H12\" firstHeaderRow=\"1\" firstDataRow=\"3\" firstDataCol=\"1\" rowPageCount=\"1\" colPageCount=\"1\"/>"),
        "{t1}"
    );
    assert!(t1.contains("<field x=\"-2\"/>"));
    assert!(t1.contains("subtotal=\"count\""));
    assert!(t1.contains("numFmtId=\"4\""));
    assert!(t1.contains("axis=\"axisPage\""));
    assert!(t1.contains("<pivotTableStyleInfo name=\"PivotStyleMedium9\""));
    let c1 = String::from_utf8(parts.get("xl/pivotCache/pivotCacheDefinition1.xml").unwrap().clone()).unwrap();
    assert!(c1.contains("<worksheetSource ref=\"A1:F9\" sheet=\"Data\"/>"), "{c1}");
    assert!(c1.contains("refreshOnLoad=\"1\""));
    let c2 = String::from_utf8(parts.get("xl/pivotCache/pivotCacheDefinition2.xml").unwrap().clone()).unwrap();
    assert!(c2.contains("<worksheetSource name=\"SalesTbl\"/>"));
    assert!(c2.contains("groupBy=\"years\"") && c2.contains("groupBy=\"months\""));
    let t2 = String::from_utf8(parts.get("xl/pivotTables/pivotTable2.xml").unwrap().clone()).unwrap();
    assert!(t2.contains("pivotShowAs=\"rankDescending\""));
    assert!(t2.contains("showDataAs=\"runTotal\""));
    // Pivot 1 has no Grand Total column (`colGrandTotals`), pivot 2 no Grand Total row
    // (`rowGrandTotals`).
    assert!(t1.contains(" colGrandTotals=\"0\"") && !t1.contains("rowGrandTotals"), "{t1}");
    assert!(t2.contains(" rowGrandTotals=\"0\"") && !t2.contains("colGrandTotals"), "{t2}");
    // The custom value format is registered in the styles part.
    let styles = String::from_utf8(parts.get("xl/styles.xml").unwrap().clone()).unwrap();
    assert!(styles.contains("formatCode=\"0.0&quot; u&quot;\""));
}

#[test]
fn unresolvable_pivot_source_is_skipped() {
    let mut wb = workbook();
    let s = wb.sheet_mut(1).unwrap();
    s.pivots[0].source = "Nowhere!A1:B2".into();
    s.pivots.truncate(1);
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert!(back.sheet(1).unwrap().pivots.is_empty());
}

#[test]
fn large_cache_is_written_without_records() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    s.cells.set(at("A1"), Cell::value(Value::text("K")));
    s.cells.set(at("B1"), Cell::value(Value::text("V")));
    s.cells.set(at("C1"), Cell::value(Value::text("W")));
    // 700,000 records x 3 fields is over the records limit.
    for r in 1..=700_000u32 {
        s.cells.set(CellRef::new(r, 1), Cell::value(Value::number(r as f64)));
        s.cells.set(CellRef::new(r, 2), Cell::value(Value::number(0.5)));
        s.cells.set(CellRef::new(r, 0), Cell::value(Value::number((r % 3) as f64)));
    }
    s.pivots.push(PivotTable {
        id: 1,
        source: "Sheet1!$A$1:$C$700001".into(),
        anchor: at("E1"),
        rows: vec![field("K")],
        values: vec![PivotValue { source_col: "V".into(), name: "Sum of V".into(), ..PivotValue::default() }],
        last_range: Some(rr("E1:F5")),
        ..PivotTable::default()
    });
    let bytes = write_xlsx(&wb).unwrap();
    let parts = unzip(&bytes);
    assert!(!parts.keys().any(|k| k.contains("pivotCacheRecords")));
    let c = String::from_utf8(parts.get("xl/pivotCache/pivotCacheDefinition1.xml").unwrap().clone()).unwrap();
    assert!(c.contains("saveData=\"0\""));
    assert!(c.contains("<sharedItems containsSemiMixedTypes=\"0\" containsString=\"0\" containsNumber=\"1\" containsInteger=\"1\" minValue=\"0\" maxValue=\"2\" count=\"3\">"), "{}", &c[..c.len().min(2000)]);
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheet(0).unwrap().pivots.len(), 1);
}

/// #1447 item 1: Excel counts booleans and errors as strings in `<sharedItems>`; writing
/// `containsString="0"` for them makes a file Excel can't open.
#[test]
fn boolean_and_error_cache_flags_match_excel() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    let na = || Value::Error(gridcraft_core::CellError::NA);
    let cols: [(&str, [Value; 3]); 7] = [
        ("K", [Value::text("a"), Value::text("b"), Value::text("a")]),
        ("BoolOnly", [Value::Bool(true), Value::Bool(false), Value::Bool(true)]),
        ("NumBool", [Value::number(1.0), Value::Bool(true), Value::number(2.5)]),
        ("ErrOnly", [na(), na(), na()]),
        ("NumErr", [Value::number(1.0), na(), Value::number(3.0)]),
        ("TextBool", [Value::text("x"), Value::Bool(true), Value::text("y")]),
        ("BoolBlank", [Value::Bool(true), Value::Empty, Value::Bool(false)]),
    ];
    for (c, (h, vals)) in cols.iter().enumerate() {
        s.cells.set(CellRef::new(0, c as u32), Cell::value(Value::text(*h)));
        for (r, v) in vals.iter().enumerate() {
            if !matches!(v, Value::Empty) {
                s.cells.set(CellRef::new(r as u32 + 1, c as u32), Cell::value(v.clone()));
            }
        }
    }
    s.pivots.push(PivotTable {
        id: 1,
        source: "Sheet1!$A$1:$G$4".into(),
        anchor: at("J1"),
        rows: vec![field("K")],
        values: vec![PivotValue { source_col: "NumBool".into(), name: "Sum of NumBool".into(), ..PivotValue::default() }],
        last_range: Some(rr("J1:K4")),
        ..PivotTable::default()
    });
    let bytes = write_xlsx(&wb).unwrap();
    let parts = unzip(&bytes);
    let c = String::from_utf8(parts.get("xl/pivotCache/pivotCacheDefinition1.xml").unwrap().clone()).unwrap();
    let start = |name: &str| -> String {
        let i = c.find(&format!("<cacheField name=\"{name}\"")).unwrap();
        let j = i + c[i..].find("<sharedItems").unwrap();
        let k = j + c[j..].find('>').unwrap();
        c[j..=k].trim_end_matches('>').trim_end_matches('/').to_string()
    };
    let flags = |name: &str| -> String {
        let t = start(name);
        // Drop the trailing count attribute, which depends on itemization.
        match t.find(" count=") {
            Some(i) => t[..i].to_string(),
            None => t,
        }
    };
    assert_eq!(flags("BoolOnly"), "<sharedItems");
    assert_eq!(flags("NumBool"), "<sharedItems containsMixedTypes=\"1\" containsNumber=\"1\" minValue=\"1\" maxValue=\"2.5\"");
    assert_eq!(flags("ErrOnly"), "<sharedItems");
    assert_eq!(flags("NumErr"), "<sharedItems containsMixedTypes=\"1\" containsNumber=\"1\" containsInteger=\"1\" minValue=\"1\" maxValue=\"3\"");
    assert_eq!(flags("TextBool"), "<sharedItems");
    assert_eq!(flags("BoolBlank"), "<sharedItems containsBlank=\"1\"");
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheet(0).unwrap().pivots.len(), 1);
    assert!(!report.warnings.iter().any(|w| w.to_lowercase().contains("pivot")), "{:?}", report.warnings);
}

const PT_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable" Target="../pivotTables/pivotTable1.xml"/></Relationships>"#;
const PT_CACHE_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheDefinition" Target="../pivotCache/pivotCacheDefinition1.xml"/></Relationships>"#;

fn with_pivot(table: &str, cache: Option<&str>) -> Vec<u8> {
    let mut extra: Vec<(&str, &str)> = vec![
        ("xl/worksheets/_rels/sheet1.xml.rels", PT_RELS),
        ("xl/pivotTables/pivotTable1.xml", table),
        ("xl/pivotTables/_rels/pivotTable1.xml.rels", PT_CACHE_RELS),
    ];
    if let Some(c) = cache {
        extra.push(("xl/pivotCache/pivotCacheDefinition1.xml", c));
    }
    minimal("<sheetData/>", &extra, "", "")
}

const GOOD_CACHE: &str = r#"<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cacheSource type="worksheet"><worksheetSource ref="A1:B3"/></cacheSource><cacheFields count="2"><cacheField name="K"><sharedItems><s v="x"/><n v="2"/><m/><b v="1"/><d v="2024-01-01T00:00:00"/></sharedItems></cacheField><cacheField name="V"><sharedItems/><fieldGroup base="0"><rangePr startNum="0" endNum="10" groupInterval="5"/><groupItems><s v="0-4"/></groupItems></fieldGroup></cacheField></cacheFields></pivotCacheDefinition>"#;

#[test]
fn hostile_pivot_parts_warn_not_panic() {
    let tables = [
        "garbage <<<",
        r#"<notAPivot/>"#,
        r#"<pivotTableDefinition name="P" cacheId="1"/>"#,
        r#"<pivotTableDefinition name="P" cacheId="1"><location ref="nonsense"/></pivotTableDefinition>"#,
        r#"<pivotTableDefinition name="" cacheId="9"><location ref="$D$3:$F$9"/><pivotFields count="1"><pivotField axis="axisRow" sortType="descending"><items><item x="99" h="1" sd="0"/><item x="-4"/><item t="default"/></items></pivotField></pivotFields><rowFields><field x="0"/><field x="4294967295"/><field x="-2"/><field x="77"/></rowFields><colFields><field x="1"/></colFields><pageFields><pageField fld="0" item="9999"/><pageField fld="-1"/><pageField fld="55"/></pageFields><dataFields><dataField fld="1" subtotal="bogus" showDataAs="index" numFmtId="99999"/><dataField fld="12"/><dataField name="E" fld="0"><extLst><ext><dataField pivotShowAs="percentOfParent"/></ext></extLst></dataField></dataFields></pivotTableDefinition>"#,
    ];
    let caches = [
        None,
        Some("garbage"),
        Some(r#"<pivotCacheDefinition><cacheSource type="external"/></pivotCacheDefinition>"#),
        Some(r#"<pivotCacheDefinition><cacheSource type="worksheet"><worksheetSource ref="zz"/></cacheSource></pivotCacheDefinition>"#),
        Some(r#"<pivotCacheDefinition><cacheSource type="worksheet"><worksheetSource ref="A1:B2"/></cacheSource></pivotCacheDefinition>"#),
        Some(r#"<somethingElse/>"#),
        Some(GOOD_CACHE),
    ];
    for t in &tables {
        for c in &caches {
            let bytes = with_pivot(t, *c);
            let (wb, report) = read_xlsx(&bytes).unwrap();
            let pivots = &wb.sheet(0).unwrap().pivots;
            if pivots.is_empty() {
                assert!(!report.warnings.is_empty(), "no warning for table {t} / cache {c:?}");
            }
        }
    }
    // The fully hostile table against a good cache still yields a pivot, with warnings.
    let (wb, report) = read_xlsx(&with_pivot(tables[4], Some(GOOD_CACHE))).unwrap();
    let pt = &wb.sheet(0).unwrap().pivots[0];
    assert_eq!(pt.anchor, at("D3"));
    assert_eq!(pt.source, "Data!$A$1:$B$3");
    assert_eq!(pt.rows.len(), 1);
    assert_eq!(pt.rows[0].sort, PivotSort::Desc);
    assert!(pt.values.iter().any(|v| v.func == PivotFunc::Sum && v.source_col == "V"));
    assert!(report.warnings.iter().any(|w| w.contains("bogus")), "{:?}", report.warnings);
    assert!(report.warnings.iter().any(|w| w.contains("grouping")), "{:?}", report.warnings);
    assert!(report.warnings.iter().any(|w| w.contains("index")), "{:?}", report.warnings);
}

/// #2159: a field repeated on the axes or as a page field is used once (each use walks the
/// field's items and copied its labels), and the lists are capped.
#[test]
fn repeated_pivot_fields_are_used_once() {
    let items: String = (0..2_000).map(|_| r#"<item x="0" sd="0"/>"#).collect();
    let rows = r#"<field x="0"/>"#.repeat(20_000);
    let pages = r#"<pageField fld="0"/>"#.repeat(20_000);
    let table = format!(
        r#"<pivotTableDefinition name="P" cacheId="1"><location ref="D5:F9"/><pivotFields count="1"><pivotField axis="axisRow"><items>{items}</items></pivotField></pivotFields><rowFields>{rows}<field x="-2"/></rowFields><colFields>{rows}</colFields><pageFields>{pages}</pageFields></pivotTableDefinition>"#
    );
    let (wb, report) = read_xlsx(&with_pivot(&table, Some(GOOD_CACHE))).unwrap();
    let pt = &wb.sheet(0).unwrap().pivots[0];
    assert_eq!(pt.rows.len(), 1, "{:?}", report.warnings);
    assert!(pt.columns.is_empty());
    assert_eq!(pt.rows[0].collapsed_items.len(), 2_000);
    assert_eq!(pt.filters.iter().filter(|f| f.source_col == "K").count(), 1);
    // One page field: the table's range starts two rows above its location.
    assert_eq!(pt.last_range, Some(rr("D3:F9")));
}

/// #2159: a field list cut at the 16,384-entry cap is reported, so an incomplete PivotTable can be
/// told apart from a complete one.
#[test]
fn capped_pivot_field_lists_warn() {
    let fields = r#"<pivotField/>"#.repeat(16_385);
    let table = format!(
        r#"<pivotTableDefinition name="P" cacheId="1"><location ref="D5:F9"/><pivotFields count="16385">{fields}</pivotFields></pivotTableDefinition>"#
    );
    let (_, report) = read_xlsx(&with_pivot(&table, Some(GOOD_CACHE))).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("field lists past 16384 entries")), "{:?}", report.warnings);
    let table = r#"<pivotTableDefinition name="P" cacheId="1"><location ref="D5:F9"/><pivotFields count="1"><pivotField/></pivotFields></pivotTableDefinition>"#;
    let (_, report) = read_xlsx(&with_pivot(table, Some(GOOD_CACHE))).unwrap();
    assert!(!report.warnings.iter().any(|w| w.contains("field lists past")), "{:?}", report.warnings);
}

/// #2159: a cache cut at 16,384 fields is reported too, as the table field lists are.
#[test]
fn capped_cache_fields_warn() {
    let fields = r#"<cacheField name="F"><sharedItems/></cacheField>"#.repeat(16_385);
    let cache = format!(
        r#"<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cacheSource type="worksheet"><worksheetSource ref="A1:B3"/></cacheSource><cacheFields count="16385">{fields}</cacheFields></pivotCacheDefinition>"#
    );
    let table = r#"<pivotTableDefinition name="P" cacheId="1"><location ref="D5:F9"/><pivotFields count="1"><pivotField/></pivotFields></pivotTableDefinition>"#;
    let (_, report) = read_xlsx(&with_pivot(table, Some(&cache))).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("cache fields past 16384")), "{:?}", report.warnings);
    let (_, report) = read_xlsx(&with_pivot(table, Some(GOOD_CACHE))).unwrap();
    assert!(!report.warnings.iter().any(|w| w.contains("cache fields past")), "{:?}", report.warnings);
}

/// #2159: axis filters and calculated fields are deduplicated through a set of lower-cased names;
/// the first of names equal but for case still wins, as with the scan it replaced.
#[test]
fn axis_filters_and_calculated_fields_dedupe_ignoring_case() {
    let cache = r#"<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cacheSource type="worksheet"><worksheetSource ref="A1:B3"/></cacheSource><cacheFields count="4"><cacheField name="K"><sharedItems><s v="a"/><s v="b"/></sharedItems></cacheField><cacheField name="k"><sharedItems><s v="a"/><s v="b"/></sharedItems></cacheField><cacheField name="Price" formula="1" databaseField="0"/><cacheField name="PRICE" formula="2" databaseField="0"/></cacheFields></pivotCacheDefinition>"#;
    let table = r#"<pivotTableDefinition name="P" cacheId="1"><location ref="D5:F9"/><pivotFields count="2"><pivotField axis="axisRow"><items><item x="0"/><item x="1" h="1"/></items></pivotField><pivotField axis="axisCol"><items><item x="0" h="1"/><item x="1"/></items></pivotField></pivotFields><rowFields><field x="0"/></rowFields><colFields><field x="1"/></colFields></pivotTableDefinition>"#;
    let (wb, report) = read_xlsx(&with_pivot(table, Some(cache))).unwrap();
    let pt = &wb.sheet(0).unwrap().pivots[0];
    assert_eq!((pt.rows.len(), pt.columns.len()), (1, 1), "{:?}", report.warnings);
    assert_eq!(pt.filters, vec![PivotFilter { source_col: "K".into(), selected: Some(vec!["a".into()]) }]);
    assert_eq!(pt.calculated_fields, vec![PivotCalcField { name: "Price".into(), formula: "=1".into() }]);
}

#[test]
fn cache_found_by_workbook_cache_id() {
    let rels_extra = r#"<Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheDefinition" Target="pivotCache/pivotCacheDefinition1.xml"/>"#;
    let table = r#"<pivotTableDefinition name="ById" cacheId="7"><location ref="C1:D4"/><pivotFields><pivotField axis="axisRow"><items><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField dataField="1"/></pivotFields><rowFields><field x="0"/></rowFields><dataFields><dataField name="Total" fld="1" subtotal="average"/></dataFields></pivotTableDefinition>"#;
    let bytes = minimal(
        "<sheetData/>",
        &[
            ("xl/worksheets/_rels/sheet1.xml.rels", PT_RELS),
            ("xl/pivotTables/pivotTable1.xml", table),
            ("xl/pivotCache/pivotCacheDefinition1.xml", GOOD_CACHE),
        ],
        rels_extra,
        "",
    );
    // `minimal` puts wb_extra before <sheets>; pivotCaches belongs after, but readers locate it by name.
    let bytes2 = {
        let mut parts = unzip(&bytes);
        let wbx = String::from_utf8(parts.remove("xl/workbook.xml").unwrap()).unwrap();
        let wbx = wbx.replace("</workbook>", r#"<pivotCaches><pivotCache cacheId="7" r:id="rId9"/></pivotCaches></workbook>"#);
        let mut list: Vec<(String, Vec<u8>)> = parts.into_iter().collect();
        list.push(("xl/workbook.xml".into(), wbx.into_bytes()));
        let refs: Vec<(&str, &[u8])> = list.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        super::make_zip(&refs)
    };
    let (wb, report) = read_xlsx(&bytes2).unwrap();
    let pts = &wb.sheet(0).unwrap().pivots;
    assert_eq!(pts.len(), 1, "{:?}", report.warnings);
    assert_eq!(pts[0].name, "ById");
    assert_eq!(pts[0].values[0].func, PivotFunc::Average);
    assert_eq!(pts[0].rows[0].source_col, "K");
}

#[test]
fn calculated_fields_round_trip() {
    let mut wb = workbook();
    {
        let p = wb.sheet_mut(1).unwrap().pivots.get_mut(0).unwrap();
        p.calculated_fields.push(PivotCalcField { name: "Price".into(), formula: "=Sales/Units".into() });
        p.calculated_fields.push(PivotCalcField { name: "Unit Bonus".into(), formula: "='Price'*0.1".into() });
        p.values.push(PivotValue { source_col: "Price".into(), name: "Sum of Price".into(), ..PivotValue::default() });
    }
    let bytes = write_xlsx(&wb).unwrap();
    let parts = unzip(&bytes);
    let c1 = String::from_utf8(parts.get("xl/pivotCache/pivotCacheDefinition1.xml").unwrap().clone()).unwrap();
    assert!(c1.contains("<cacheField name=\"Price\" numFmtId=\"0\" formula=\"Sales/Units\" databaseField=\"0\">"), "{c1}");
    assert!(c1.contains("formula=\"&apos;Price&apos;*0.1\"") || c1.contains("formula=\"'Price'*0.1\""), "{c1}");
    let t1 = String::from_utf8(parts.get("xl/pivotTables/pivotTable1.xml").unwrap().clone()).unwrap();
    assert!(t1.contains("<dataField name=\"Sum of Price\" fld=\"6\""), "{t1}");
    assert!(t1.contains("dataField=\"1\" dragToRow=\"0\""), "{t1}");
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.iter().all(|w| !w.to_lowercase().contains("pivot")), "{:?}", report.warnings);
    let orig = &wb.sheet(1).unwrap().pivots;
    let read = &back.sheet(1).unwrap().pivots;
    for (a, b) in orig.iter().zip(read.iter()) {
        assert_eq!(a, b, "pivot {} differs", a.name);
    }
}

/// The `x` attributes of an axis list (`rowFields`/`colFields`) in a pivotTable part.
fn axis_xs(table: &str, tag: &str) -> Vec<i64> {
    let Some(start) = table.find(&format!("<{tag} ")) else { return vec![] };
    let end = table[start..].find(&format!("</{tag}>")).map_or(table.len(), |e| start + e);
    table[start..end].split("<field x=\"").skip(1).filter_map(|s| s.split('"').next()?.parse().ok()).collect()
}

#[test]
fn values_on_rows_round_trip() {
    let mut wb = workbook();
    {
        let p = wb.sheet_mut(1).unwrap().pivots.get_mut(1).unwrap();
        p.values_on_rows = true;
        p.values_position = Some(1);
    }
    let bytes = write_xlsx(&wb).unwrap();
    let parts = unzip(&bytes);
    let t2 = String::from_utf8(parts.get("xl/pivotTables/pivotTable2.xml").unwrap().clone()).unwrap();
    assert!(t2.contains(" dataOnRows=\"1\""), "{t2}");
    // Σ Values (-2) between the two row fields; Columns keep only Product.
    let rows = axis_xs(&t2, "rowFields");
    assert_eq!(rows.len(), 3, "{t2}");
    assert_eq!(rows[1], -2, "{t2}");
    assert!(!axis_xs(&t2, "colFields").contains(&-2), "{t2}");
    assert!(t2.contains("firstDataRow=\"2\" firstDataCol=\"3\""), "{t2}");
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.iter().all(|w| !w.to_lowercase().contains("pivot")), "{:?}", report.warnings);
    assert_eq!(back.sheet(1).unwrap().pivots, wb.sheet(1).unwrap().pivots);

    // After the last row field.
    wb.sheet_mut(1).unwrap().pivots[1].values_position = None;
    let bytes = write_xlsx(&wb).unwrap();
    let t2 = String::from_utf8(unzip(&bytes).get("xl/pivotTables/pivotTable2.xml").unwrap().clone()).unwrap();
    assert_eq!(axis_xs(&t2, "rowFields").last(), Some(&-2), "{t2}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheet(1).unwrap().pivots, wb.sheet(1).unwrap().pivots);

    // Σ Values in Columns is written after the last column field, its only place in the layout.
    {
        let p = wb.sheet_mut(1).unwrap().pivots.get_mut(1).unwrap();
        p.values_on_rows = false;
        p.values_position = Some(0);
    }
    let bytes = write_xlsx(&wb).unwrap();
    let t2 = String::from_utf8(unzip(&bytes).get("xl/pivotTables/pivotTable2.xml").unwrap().clone()).unwrap();
    assert!(!t2.contains("dataOnRows"), "{t2}");
    assert_eq!(axis_xs(&t2, "colFields").last(), Some(&-2), "{t2}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheet(1).unwrap().pivots[1].values_position, None);
}

#[test]
fn values_ahead_of_column_fields_read_last() {
    let table = r#"<pivotTableDefinition name="ValuesFirst" cacheId="1"><location ref="C1:F4"/><pivotFields><pivotField axis="axisCol"><items><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField dataField="1"/></pivotFields><colFields count="2"><field x="-2"/><field x="0"/></colFields><dataFields><dataField name="Sum of V" fld="1"/><dataField name="Count of V" fld="1" subtotal="count"/></dataFields></pivotTableDefinition>"#;
    let (wb, _) = read_xlsx(&with_pivot(table, Some(GOOD_CACHE))).unwrap();
    let pt = &wb.sheet(0).unwrap().pivots[0];
    assert_eq!((pt.columns.len(), pt.values.len()), (1, 2));
    assert!(!pt.values_on_rows);
    assert_eq!(pt.values_position, None);
}

// As Excel saves them: `rowGrandTotals="0"` drops the Grand Total row under the report (leaving
// Excel's "On for Rows Only"), `colGrandTotals="0"` the Grand Total column at its right ("On for
// Columns Only"). LibreOffice's Pivot2_Row_Subtotals.xlsx (`rowGrandTotals="0"`) shows no Grand
// Total row; ClosedXML's TemplateWithTableSourcePivotTables.xlsx pivotTable5 (`colGrandTotals="0"`,
// row and column fields) shows a Grand Total row and no Grand Total column.
#[test]
fn grand_totals_read_in_excel_direction() {
    let table = |attr: &str| {
        format!(
            r#"<pivotTableDefinition name="GT" cacheId="1" {attr}><location ref="C1:F4"/><pivotFields><pivotField axis="axisRow"><items><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField dataField="1"/></pivotFields><rowFields><field x="0"/></rowFields><dataFields><dataField name="Sum of V" fld="1"/></dataFields></pivotTableDefinition>"#
        )
    };
    for (attr, rows, cols) in [("rowGrandTotals=\"0\"", true, false), ("colGrandTotals=\"0\"", false, true), ("", true, true)] {
        let (wb, _) = read_xlsx(&with_pivot(&table(attr), Some(GOOD_CACHE))).unwrap();
        let pt = &wb.sheet(0).unwrap().pivots[0];
        assert_eq!((pt.grand_totals_rows, pt.grand_totals_cols), (rows, cols), "{attr}");
    }
}

// `enableDrill="0"` is PivotTable Options › Data › Enable show details turned off; without the
// attribute the option is on.
#[test]
fn enable_show_details_reads_and_writes_enable_drill() {
    let table = |attr: &str| {
        format!(
            r#"<pivotTableDefinition name="Drill" cacheId="1" {attr}><location ref="C1:F4"/><pivotFields><pivotField axis="axisRow"><items><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField dataField="1"/></pivotFields><rowFields><field x="0"/></rowFields><dataFields><dataField name="Sum of V" fld="1"/></dataFields></pivotTableDefinition>"#
        )
    };
    for (attr, on) in [("enableDrill=\"0\"", false), ("enableDrill=\"1\"", true), ("", true)] {
        let (wb, _) = read_xlsx(&with_pivot(&table(attr), Some(GOOD_CACHE))).unwrap();
        assert_eq!(wb.sheet(0).unwrap().pivots[0].enable_show_details, on, "{attr}");
        let parts = unzip(&write_xlsx(&wb).unwrap());
        let written = String::from_utf8(parts.get("xl/pivotTables/pivotTable1.xml").unwrap().clone()).unwrap();
        assert_eq!(written.contains(" enableDrill=\"0\""), !on, "{attr}: {written}");
    }
}

#[test]
fn data_on_rows_with_one_value_field_reads_without_warning() {
    let table = r#"<pivotTableDefinition name="OneValue" cacheId="1" dataOnRows="1"><location ref="C1:D4"/><pivotFields><pivotField axis="axisRow"><items><item x="0"/><item x="1"/><item t="default"/></items></pivotField><pivotField dataField="1"/></pivotFields><rowFields><field x="0"/></rowFields><dataFields><dataField name="Total" fld="1"/></dataFields></pivotTableDefinition>"#;
    let (wb, report) = read_xlsx(&with_pivot(table, Some(GOOD_CACHE))).unwrap();
    assert!(report.warnings.iter().all(|w| !w.contains("values on rows")), "{:?}", report.warnings);
    let pt = &wb.sheet(0).unwrap().pivots[0];
    assert!(pt.values_on_rows);
    assert_eq!(pt.values_position, None);
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert!(back.sheet(0).unwrap().pivots[0].values_on_rows);
}

const SIMPLE_TABLE: &str = r#"<pivotTableDefinition name="Sales"><location ref="C1:D4"/><pivotFields><pivotField axis="axisRow"><items><item x="0"/><item t="default"/></items></pivotField><pivotField dataField="1"/></pivotFields><rowFields><field x="0"/></rowFields><dataFields><dataField name="Total" fld="1"/></dataFields></pivotTableDefinition>"#;
const CACHE_PART: &str = "xl/pivotCache/pivotCacheDefinition1.xml";

/// Rewrites the uncompressed size a zip's central directory declares for `name`.
fn declare_size(mut zip: Vec<u8>, name: &str, size: u32) -> Vec<u8> {
    let at = (0..zip.len().saturating_sub(46))
        .find(|&i| {
            zip[i..i + 4] == [0x50, 0x4b, 0x01, 0x02] && {
                let len = u16::from_le_bytes([zip[i + 28], zip[i + 29]]) as usize;
                zip.get(i + 46..i + 46 + len) == Some(name.as_bytes())
            }
        })
        .unwrap();
    zip[at + 24..at + 28].copy_from_slice(&size.to_le_bytes());
    zip
}

#[test]
fn unreadable_pivot_cache_warns() {
    // Control: a readable cache gives a PivotTable and no cache warning.
    let (wb, report) = read_xlsx(&with_pivot(SIMPLE_TABLE, Some(GOOD_CACHE))).unwrap();
    assert_eq!(wb.sheet(0).unwrap().pivots.len(), 1, "{:?}", report.warnings);
    assert!(!report.warnings.iter().any(|w| w.contains("PivotTable's cache")), "{:?}", report.warnings);
    // Malformed and missing caches: the workbook opens and the lost PivotTable is reported.
    let lost = format!("a PivotTable's cache {CACHE_PART} is missing or could not be read; kept as plain cells");
    for cache in [Some("garbage <<<"), None] {
        let (wb, report) = read_xlsx(&with_pivot(SIMPLE_TABLE, cache)).unwrap();
        assert!(wb.sheet(0).unwrap().pivots.is_empty());
        assert!(report.warnings.contains(&lost), "{cache:?}: {:?}", report.warnings);
    }
}

#[test]
fn oversized_pivot_cache_warns() {
    // The cache declares more than the per-part limit, so it is refused before it inflates.
    let declared = u32::try_from(crate::package::MAX_PART + 1).unwrap();
    let bytes = declare_size(with_pivot(SIMPLE_TABLE, Some(GOOD_CACHE)), CACHE_PART, declared);
    let (wb, report) = read_xlsx(&bytes).unwrap();
    assert_eq!(wb.sheet(0).unwrap().name, "Data");
    assert!(wb.sheet(0).unwrap().pivots.is_empty());
    let limit = crate::package::MAX_PART;
    let lost = format!("a PivotTable's cache is too large to read ({CACHE_PART} expands to more than {limit} bytes); kept as plain cells");
    assert!(report.warnings.contains(&lost), "{:?}", report.warnings);
}

/// A one-sheet-of-data workbook with a pivot on "Report": `columns` are the source columns
/// (header, values), `pt` the pivot built on `Data!A1:<last>`.
fn small_pivot(columns: &[(&str, Vec<Value>)], pt: impl FnOnce(&mut PivotTable)) -> HashMap<String, String> {
    let mut wb = Workbook::new();
    let data = wb.sheet_mut(0).unwrap();
    data.name = "Data".into();
    let mut rows = 0;
    for (c, (h, vals)) in columns.iter().enumerate() {
        data.cells.set(CellRef::new(0, c as u32), Cell::value(Value::text(*h)));
        for (r, v) in vals.iter().enumerate() {
            if *v != Value::Empty {
                data.cells.set(CellRef::new(r as u32 + 1, c as u32), Cell::value(v.clone()));
            }
        }
        rows = rows.max(vals.len());
    }
    let last = format!("{}{}", gridcraft_core::col_to_letters(columns.len() as u32 - 1), rows + 1);
    let mut table = PivotTable { id: 1, name: "PT".into(), source: format!("Data!$A$1:{last}"), anchor: at("A3"), ..PivotTable::default() };
    pt(&mut table);
    let mut report = Sheet::new("Report");
    report.pivots = vec![table];
    wb.sheets.push(Arc::new(report));
    unzip(&write_xlsx(&wb).unwrap()).into_iter().map(|(k, v)| (k, String::from_utf8_lossy(&v).into_owned())).collect()
}

/// #1447: two value fields and no column field: Excel 365 adds a `Values` row unless told not to.
#[test]
fn two_value_fields_without_columns_hide_the_values_row() {
    let cols = [
        ("Region", vec![Value::text("East"), Value::text("West"), Value::text("East")]),
        ("Sales", vec![Value::number(10.0), Value::number(20.0), Value::number(5.0)]),
        ("Qty", vec![Value::number(1.0), Value::number(2.0), Value::number(3.0)]),
    ];
    let value = |c: &str| PivotValue { source_col: c.into(), name: format!("Sum of {c}"), ..PivotValue::default() };
    let two = small_pivot(&cols, |p| {
        p.rows = vec![field("Region")];
        p.values = vec![value("Sales"), value("Qty")];
    });
    let t = &two["xl/pivotTables/pivotTable1.xml"];
    assert!(t.contains("firstHeaderRow=\"0\" firstDataRow=\"1\""), "{t}");
    assert!(t.contains("<x14:pivotTableDefinition hideValuesRow=\"1\"/>"), "{t}");
    let one = small_pivot(&cols, |p| {
        p.rows = vec![field("Region")];
        p.values = vec![value("Sales")];
    });
    let t = &one["xl/pivotTables/pivotTable1.xml"];
    assert!(t.contains("firstHeaderRow=\"1\""), "{t}");
    assert!(!t.contains("hideValuesRow"), "{t}");
}

/// The "Trend" pivot of [`workbook`] with `values` as its value fields, written; returns the file
/// and its PivotTable and cache parts.
fn trend_with(values: impl FnOnce(&PivotValue) -> Vec<PivotValue>) -> (Vec<u8>, String, String) {
    let mut wb = workbook();
    let pivot = wb.sheet_mut(1).unwrap().pivots.iter_mut().find(|p| p.name == "Trend").unwrap();
    pivot.values = values(&pivot.values[0]);
    let bytes = write_xlsx(&wb).unwrap();
    let parts = unzip(&bytes);
    let (path, table) = parts
        .iter()
        .filter(|(path, _)| path.starts_with("xl/pivotTables/pivotTable"))
        .map(|(path, xml)| (path.clone(), String::from_utf8_lossy(xml).into_owned()))
        .find(|(_, xml)| xml.contains("name=\"Trend\""))
        .unwrap();
    let n = path.trim_start_matches("xl/pivotTables/pivotTable").trim_end_matches(".xml");
    let cache_part = format!("xl/pivotCache/pivotCacheDefinition{n}.xml");
    let rels = String::from_utf8_lossy(&parts[&format!("xl/pivotTables/_rels/pivotTable{n}.xml.rels")]).into_owned();
    assert!(rels.contains(&format!("../pivotCache/pivotCacheDefinition{n}.xml")), "{rels}");
    let cache = String::from_utf8_lossy(&parts[&cache_part]).into_owned();
    (bytes, table, cache)
}

fn trend_values(bytes: &[u8]) -> Vec<PivotValue> {
    let (back, report) = read_xlsx(bytes).unwrap();
    assert!(report.warnings.iter().all(|w| !w.to_lowercase().contains("pivot")), "{:?}", report.warnings);
    back.sheet(1).unwrap().pivots.iter().find(|p| p.name == "Trend").unwrap().values.clone()
}

const GC_DISTINCT: &str = "<ext uri=\"{80B1AD42-0C9F-45FF-B109-D57FB0F4F0B4}\" xmlns:gc=\"https://github.com/storytold/gridcraft/ns/pivot/2026\"><gc:summary func=\"distinctCount\"/></ext>";

#[test]
fn distinct_count_round_trips_through_a_gridcraft_extension() {
    // The file format's subtotal list has no distinct count: the field is written as Count, which
    // other readers use, with a GridCraft marker that brings Distinct Count back on open.
    let (bytes, table, cache) =
        trend_with(|v| vec![PivotValue { func: PivotFunc::DistinctCount, name: "Distinct Count of Sales".into(), ..v.clone() }]);
    assert!(table.contains("<dataField name=\"Distinct Count of Sales\" fld=\"4\" subtotal=\"count\""), "{table}");
    assert!(table.contains(&format!("<extLst>{GC_DISTINCT}</extLst></dataField>")), "{table}");
    // A refresh on open would recompute it as Count in Excel, over the values on the sheet.
    assert!(!cache.contains("refreshOnLoad"), "{cache}");
    let values = trend_values(&bytes);
    assert_eq!((values[0].func, values[0].name.as_str()), (PivotFunc::DistinctCount, "Distinct Count of Sales"));
}

#[test]
fn rank_and_distinct_count_write_one_extension_each() {
    let (bytes, table, _) = trend_with(|v| {
        vec![PivotValue { func: PivotFunc::DistinctCount, show_as: PivotShowAs::Rank, name: "Rank of Distinct Sales".into(), ..v.clone() }]
    });
    let rank = "<ext uri=\"{E15A36E0-9728-4e99-A89B-3F7291B0FE68}\" xmlns:x14=\"http://schemas.microsoft.com/office/spreadsheetml/2009/9/main\"><x14:dataField pivotShowAs=\"rankDescending\"/></ext>";
    assert!(table.contains(&format!("<extLst>{rank}{GC_DISTINCT}</extLst></dataField>")), "{table}");
    let values = trend_values(&bytes);
    assert_eq!((values[0].func, values[0].show_as), (PivotFunc::DistinctCount, PivotShowAs::Rank));
}

#[test]
fn count_without_the_gridcraft_extension_reads_as_count() {
    // Plain `subtotal="count"`, as Excel writes it, stays Count; so does a stale marker on a field
    // whose function was changed to something else.
    let (bytes, table, cache) = trend_with(|v| vec![PivotValue { func: PivotFunc::Count, name: "Count of Sales".into(), ..v.clone() }]);
    assert!(!table.contains("extLst"), "{table}");
    assert!(cache.contains("refreshOnLoad=\"1\""), "{cache}");
    assert_eq!(trend_values(&bytes)[0].func, PivotFunc::Count);
    let marked = |subtotal: &str| {
        format!(
            r#"<pivotTableDefinition name="P" cacheId="1"><location ref="C1:D4"/><pivotFields><pivotField/><pivotField dataField="1"/></pivotFields><dataFields><dataField name="V" fld="1"{subtotal}><extLst>{GC_DISTINCT}</extLst></dataField></dataFields></pivotTableDefinition>"#
        )
    };
    let func = |table: &str| read_xlsx(&with_pivot(table, Some(GOOD_CACHE))).unwrap().0.sheet(0).unwrap().pivots[0].values[0].func;
    assert_eq!(func(&marked(" subtotal=\"count\"")), PivotFunc::DistinctCount);
    assert_eq!(func(&marked("")), PivotFunc::Sum);
    assert_eq!(func(&marked(" subtotal=\"max\"")), PivotFunc::Max);
}
