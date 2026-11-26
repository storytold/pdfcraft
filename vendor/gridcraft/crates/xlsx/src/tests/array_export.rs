//! Array evaluation must survive export even when a formula returns one value.

use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{Cell, DefinedName, Formula, Sheet, Table, TableColumn, TotalsFn, Workbook};

use crate::package::Package;
use crate::xml::El;
use crate::{read_xlsx, write_xlsx};

fn at(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn range(s: &str) -> RangeRef {
    RangeRef::parse(s).unwrap()
}

fn formula(text: &str, value: f64) -> Cell {
    let mut cell = Cell::formula(Formula::new(text));
    cell.value = Value::Number(value);
    cell
}

fn cell<'a>(sheet: &'a El, address: &str) -> &'a El {
    sheet.child("sheetData").unwrap().kids("row").flat_map(|r| r.kids("c")).find(|c| c.attr("r") == Some(address)).unwrap()
}

fn assert_scalar(sheet: &El, address: &str) {
    let c = cell(sheet, address);
    assert_eq!(c.attr("cm"), None, "scalar formula must not have dynamic metadata at {address}");
    let f = c.child("f").unwrap();
    assert_eq!(f.attr("t"), None);
    assert_eq!(f.attr("ref"), None);
}

fn assert_dynamic(sheet: &El, address: &str, reference: &str) {
    let c = cell(sheet, address);
    assert_eq!(c.attr("cm"), Some("1"), "array evaluation metadata at {address}");
    let f = c.child("f").unwrap();
    assert_eq!(f.attr("t"), Some("array"));
    assert_eq!(f.attr("ref"), Some(reference));
}

#[test]
fn nonspilling_formulas_preserve_array_evaluation_and_metadata() {
    let mut wb = Workbook::new();
    let data = wb.sheet_mut(0).unwrap();
    data.name = "Data".into();
    for (address, value) in [("A1", 1.0), ("A2", 4.0), ("A3", 9.0), ("B1", 2.0), ("B2", 3.0), ("B3", 4.0)] {
        data.cells.set(at(address), Cell::value(Value::Number(value)));
    }
    let cases = [("B107", "SUM(Data!A1:A3*Data!B1:B3)", 50.0), ("B108", "SUM(SQRT(Data!A1:A3))", 6.0), ("B109", "SUM(ABS(Data!A1:A3))", 14.0)];
    let mut results = Sheet::new("Results");
    for (address, text, value) in cases {
        results.cells.set(at(address), formula(text, value));
    }
    results.cells.set(at("C107"), Cell::value(Value::text("neighbor")));
    assert!(results.spill_ranges.is_empty());
    wb.sheets.push(Arc::new(results));

    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet2.xml").unwrap().unwrap();
    for (address, text, _) in cases {
        assert_dynamic(&sheet, address, address);
        assert_eq!(cell(&sheet, address).child("f").unwrap().text, text);
    }
    assert_eq!(cell(&sheet, "C107").attr("cm"), None);

    let metadata = pkg.read_xml("xl/metadata.xml").unwrap().unwrap();
    let kind = metadata.path(&["metadataTypes", "metadataType"]).unwrap();
    assert_eq!(kind.attr("name"), Some("XLDAPR"));
    assert_eq!(kind.attr("cellMeta"), Some("1"));
    let dynamic = metadata.path(&["futureMetadata", "bk", "extLst", "ext", "dynamicArrayProperties"]).unwrap();
    assert_eq!(dynamic.attr("fDynamic"), Some("1"));
    assert_eq!(dynamic.attr("fCollapsed"), Some("0"));
    let record = metadata.path(&["cellMetadata", "bk", "rc"]).unwrap();
    assert_eq!((record.attr("t"), record.attr("v")), (Some("1"), Some("0")));
    let types = pkg.read_xml("[Content_Types].xml").unwrap().unwrap();
    assert!(types.kids("Override").any(|e| e.attr("PartName") == Some("/xl/metadata.xml")
        && e.attr("ContentType") == Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml")));
    assert!(pkg.rels("xl/workbook.xml").unwrap().iter().any(|r| r.kind == "sheetMetadata" && r.target == "xl/metadata.xml"));

    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let results = back.sheet(1).unwrap();
    for (address, text, value) in cases {
        let c = results.cell(at(address)).unwrap();
        let f = c.formula.as_ref().unwrap();
        assert_eq!(f.text, text);
        assert_eq!(f.array, None, "dynamic formula must not become legacy CSE");
        assert_eq!(c.value, Value::Number(value));
    }
    assert_eq!(results.value(at("C107")), Value::text("neighbor"));
}

#[test]
fn dynamic_spills_and_legacy_cse_keep_distinct_ranges() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    let mut legacy = Formula::new("ROW(A1:A2)");
    legacy.array = Some(range("A1:A2"));
    sh.cells.set(at("A1"), Cell::formula(legacy));
    sh.cells.set(at("A2"), Cell::value(Value::Number(2.0)));
    sh.cells.set(at("B1"), formula("SEQUENCE(3)", 1.0));
    sh.spill_ranges.insert(at("B1"), range("B1:B3"));
    sh.spill.insert(at("B2"), Value::Number(2.0));
    sh.spill.insert(at("B3"), Value::Number(3.0));
    sh.cells.set(at("C1"), formula("SUM(B1:B3)", 6.0));

    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    assert_eq!(cell(&sheet, "A1").attr("cm"), None);
    let legacy = cell(&sheet, "A1").child("f").unwrap();
    assert_eq!((legacy.attr("t"), legacy.attr("ref")), (Some("array"), Some("A1:A2")));
    assert_dynamic(&sheet, "B1", "B1:B3");
    assert_scalar(&sheet, "C1");
    assert_eq!(cell(&sheet, "B2").child("v").unwrap().text, "2");
    assert_eq!(cell(&sheet, "B2").attr("cm"), None);

    let (back, _) = read_xlsx(&bytes).unwrap();
    let sh = back.sheet(0).unwrap();
    assert_eq!(sh.cell(at("A1")).unwrap().formula.as_ref().unwrap().array, Some(range("A1:A2")));
    // A legacy array's cached values are dropped on read: the formula fills its range on recalc (#156).
    assert!(sh.cell(at("A2")).is_none());
    assert_eq!(sh.cell(at("B1")).unwrap().formula.as_ref().unwrap().array, None);
    assert!(sh.cell(at("B2")).is_none());
    assert!(sh.cell(at("B3")).is_none());
    assert_eq!(sh.value(at("C1")), Value::Number(6.0));
}

#[test]
fn scalar_formulas_constants_and_legacy_cse_do_not_require_dynamic_metadata() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.name = "Data".into();
    sh.cells.set(at("A1"), Cell::value(Value::Number(3.0)));
    for (address, text, value) in [("C1", "1+2", 3.0), ("C2", "Data!A1", 3.0), ("C3", "SUM(A1:A3)", 3.0), ("C4", "A1*2", 6.0)] {
        sh.cells.set(at(address), formula(text, value));
    }
    for legacy in [false, true] {
        if legacy {
            let mut f = Formula::new("SUM(A1:A2)");
            f.array = Some(range("B1"));
            wb.sheet_mut(0).unwrap().cells.set(at("B1"), Cell::formula(f));
        }
        let bytes = write_xlsx(&wb).unwrap();
        let mut pkg = Package::open(&bytes).unwrap();
        assert!(!pkg.has("xl/metadata.xml"));
        assert!(!pkg.rels("xl/workbook.xml").unwrap().iter().any(|r| r.kind == "sheetMetadata"));
        let types = pkg.read_xml("[Content_Types].xml").unwrap().unwrap();
        assert!(!types.kids("Override").any(|e| e.attr("PartName") == Some("/xl/metadata.xml")));
        let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
        assert_eq!(cell(&sheet, "A1").attr("cm"), None);
        for address in ["C1", "C2", "C3", "C4"] {
            assert_scalar(&sheet, address);
        }
    }
}

#[test]
fn legacy_index_match_formulas_remain_scalar_after_xlsx_roundtrip() {
    let source = super::minimal(
        r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c><c r="B1"><v>0.1</v></c><c r="D1" t="inlineStr"><is><t>c</t></is></c><c r="E1" t="inlineStr"><is><t>x</t></is></c><c r="F1"><f>INDEX($B$1:$B$3,MATCH(D1,$A$1:$A$3,0))</f><v>0.7</v></c><c r="F3" t="str"><f>IF(E1=&quot;&quot;,&quot;&quot;,INDEX($B$1:$B$3,MATCH(D1,$A$1:$A$3,0)))</f><v>0.7</v></c><c r="F5"><f>VLOOKUP(D1,$A$1:$B$3,2,FALSE)</f><v>0.7</v></c><c r="F6"><f>SUM($B$1:$B$3)</f><v>1.2</v></c><c r="F7"><f>MATCH(D1,$A$1:$A$3,0)</f><v>3</v></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>b</t></is></c><c r="B2"><v>0.4</v></c><c r="F2"><f>IFERROR(INDEX($B$1:$B$3,MATCH(D1,$A$1:$A$3,0)),0)</f><v>0.7</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>c</t></is></c><c r="B3"><v>0.7</v></c></row></sheetData>"#,
        &[],
        "",
        "",
    );
    let (opened, _) = read_xlsx(&source).unwrap();
    let saved = write_xlsx(&opened).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();

    for address in ["F1", "F2", "F3", "F5", "F6", "F7"] {
        assert_scalar(&sheet, address);
    }
}

#[test]
fn shared_legacy_index_match_formulas_remain_scalar_after_xlsx_roundtrip() {
    let mut body = String::from("<sheetData>");
    for row in 16..=39 {
        let formula = if row == 16 {
            format!(r#"<f t="shared" ref="F16:F39" si="0">IF(E{row}="","",IFERROR(INDEX($B$1:$B$3,MATCH(D{row},$A$1:$A$3,0)),0))</f>"#)
        } else {
            r#"<f t="shared" si="0"/>"#.to_string()
        };
        body.push_str(&format!(r#"<row r="{row}"><c r="F{row}">{formula}<v>0.7</v></c></row>"#));
    }
    body.push_str("</sheetData>");
    let source = super::minimal(&body, &[], "", "");
    let (opened, _) = read_xlsx(&source).unwrap();
    let saved = write_xlsx(&opened).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();

    for row in 16..=39 {
        assert_scalar(&sheet, &format!("F{row}"));
    }
}

#[test]
fn calculated_table_column_keeps_formula_and_cell_value() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.cells.set(at("A2"), Cell::value(Value::Number(3.0)));
    sh.cells.set(at("B2"), formula("A2*2", 6.0));
    sh.tables.push(Table {
        id: 1,
        name: "Data".into(),
        range: range("A1:B2"),
        header_row: true,
        totals_row: false,
        columns: vec![
            TableColumn { name: "Input".into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None },
            TableColumn {
                name: "Output".into(),
                totals: TotalsFn::None,
                totals_label: None,
                formula: Some("A2*2".into()),
                unique_name: None,
                xml: None,
            },
        ],
        style: "TableStyleLight1".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    assert_scalar(&sheet, "B2");
    assert_eq!(cell(&sheet, "B1").attr("cm"), None, "table headers are text");
    let table = pkg.read_xml("xl/tables/table1.xml").unwrap().unwrap();
    let column = table.child("tableColumns").unwrap().kids("tableColumn").nth(1).unwrap();
    assert_eq!(column.child("calculatedColumnFormula").unwrap().text, "A2*2");
    let (back, _) = read_xlsx(&bytes).unwrap();
    let sh = back.sheet(0).unwrap();
    assert_eq!(sh.tables[0].columns[1].formula.as_deref(), Some("A2*2"));
    assert_eq!(sh.cell(at("B2")).unwrap().formula.as_ref().unwrap().array, None);
    assert_eq!(sh.value(at("B2")), Value::Number(6.0));
}

#[test]
fn this_row_references_are_written_in_long_form() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.cells.set(at("A2"), Cell::value(Value::Number(3.0)));
    sh.cells.set(at("B2"), formula("Data[@Input]*2", 6.0));
    sh.tables.push(Table {
        id: 1,
        name: "Data".into(),
        range: range("A1:B2"),
        header_row: true,
        totals_row: false,
        columns: vec![
            TableColumn { name: "Input".into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None },
            TableColumn {
                name: "Output".into(),
                totals: TotalsFn::None,
                totals_label: None,
                formula: Some("[@Input]*2".into()),
                unique_name: None,
                xml: None,
            },
        ],
        style: "TableStyleLight1".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    assert_eq!(cell(&sheet, "B2").child("f").unwrap().text, "Data[[#This Row],[Input]]*2");
    let table = pkg.read_xml("xl/tables/table1.xml").unwrap().unwrap();
    let column = table.child("tableColumns").unwrap().kids("tableColumn").nth(1).unwrap();
    assert_eq!(column.child("calculatedColumnFormula").unwrap().text, "[[#This Row],[Input]]*2");
    let (back, _) = read_xlsx(&bytes).unwrap();
    let sh = back.sheet(0).unwrap();
    assert_eq!(sh.tables[0].columns[1].formula.as_deref(), Some("[@Input]*2"));
    assert_eq!(sh.cell(at("B2")).unwrap().formula.as_ref().unwrap().text, "Data[@Input]*2");
}

/// Compare both cell and formula metadata, collecting all boundaries in the group on failure.
fn check_formula_metadata(mut wb: Workbook, sheet_index: usize, cases: &[(&str, bool)]) {
    for (i, (text, _)) in cases.iter().enumerate() {
        let f = Formula::new(text);
        assert!(f.expr().is_some(), "test formula must parse: {text}");
        wb.sheet_mut(sheet_index).unwrap().cells.set(CellRef::new(i as u32 + 100, 5), Cell::formula(f));
    }
    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml(&format!("xl/worksheets/sheet{}.xml", sheet_index + 1)).unwrap().unwrap();
    let mut mismatches = vec![];
    for (i, (text, dynamic)) in cases.iter().enumerate() {
        let address = CellRef::new(i as u32 + 100, 5).to_string();
        let c = cell(&sheet, &address);
        let f = c.child("f").unwrap();
        let expected = if *dynamic { (Some("1"), Some("array"), Some(address.as_str())) } else { (None, None, None) };
        let actual = (c.attr("cm"), f.attr("t"), f.attr("ref"));
        if actual != expected {
            mismatches.push(format!("{text}: expected {expected:?}, got {actual:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn implicit_intersection_metadata_respects_function_argument_context() {
    check_formula_metadata(
        Workbook::new(),
        0,
        &[
            ("SQRT(A1:A3)", true),
            ("ABS(A1:A3)", true),
            ("SUM(A1:A3*B1:B3)", true),
            ("SUM(SQRT(A1:A3))", true),
            ("SUM(A1:A3)", false),
            ("SUMPRODUCT(A1:A3*B1:B3)", false),
            ("SUMPRODUCT(SQRT(A1:A3))", false),
            // An `@` that legacy Excel implies is dropped (the reader puts it back), and so is one
            // of a range an edit made one cell (`SQRT(@A1:A1)` after deleting rows under
            // `SQRT(@A1:A3)`). An `@` of one value stays as `_xlfn.SINGLE` in a legacy formula, as
            // Excel writes it; any other `@` makes the formula a dynamic-array one.
            ("SQRT(@A1:A3)", false),
            ("@A1:A3*2", false),
            ("@(A1:A3*2)", true),
            ("SQRT(@A1)", false),
            ("SQRT(@A1:A1)", false),
            ("@SUM(A1:A3)", false),
            ("SUMPRODUCT(@ROW(A1:A3))", true),
            ("SUMPRODUCT(@SQRT(A1:A3))", true),
            // Legacy Excel intersects IFERROR's and IFNA's arguments inside SUMPRODUCT; dynamic-array
            // Excel evaluates them as arrays.
            ("SUMPRODUCT(IFERROR(VALUE(A1:A3),0))", true),
            ("SUMPRODUCT(IFNA(A1:A3,0))", true),
            ("SQRT(INDIRECT(\"A1\"))", false),
            ("SUM(INDIRECT(\"A1:A3\"))", false),
            ("SQRT(INDIRECT(\"A1:A3\"))", true),
            ("SUM(INDIRECT(A1))", false),
            ("SQRT(INDIRECT(A1))", true),
            ("INDEX(A1:B3,1,1)", false),
            ("INDEX(A1:A3,1)", false),
            ("INDEX(A1:A3,MATCH(D1,A1:A3,0))", false),
            ("IFERROR(INDEX(A1:A3,MATCH(D1,A1:A3,0)),0)", false),
            ("IF(E1=\"\",\"\",INDEX(A1:A3,MATCH(D1,A1:A3,0)))", false),
            ("INDEX(A1:B3,MATCH(D1,A1:A3,0),MATCH(E1,A1:B1,0))", false),
            ("INDEX(A1:B3,MATCH(D1,A1:A3,0))", true),
            ("SUM(INDEX(A1:B3,0,1))", false),
            ("SQRT(INDEX(A1:B3,0,1))", true),
            ("SUM(ROW(A1:A3))", true),
            ("SUMPRODUCT(ROW(A1:A3))", false),
            ("OFFSET(A1,0,0,1,1)*2", false),
            ("SUM(OFFSET(A1,0,0,3,1)*2)", true),
            ("SUM(OFFSET(A1,0,0,3,1))", false),
            ("LET(x,A1:A3,SUM(x))", false),
            ("LET(x,A1:A3,SUM(x*2))", true),
            ("XLOOKUP(1,A1:A3,B1:B3)", false),
            ("XLOOKUP(1,A1:A3,B1:C3)", true),
            ("FORECAST.LINEAR(4,A1:A3,B1:B3)", false),
            ("FORECAST.ETS(4,A1:A3,B1:B3,1,1,1)", false),
            ("FORECAST.ETS(C1:C3,A1:A3,B1:B3)", true),
            ("FORECAST.ETS.CONFINT(4,A1:A3,B1:B3,0.95)", false),
            ("FORECAST.ETS.SEASONALITY(A1:A3,B1:B3)", false),
            ("FORECAST.ETS.STAT(A1:A3,B1:B3,1)", false),
        ],
    );
}

#[test]
fn implicit_intersection_metadata_resolves_names_in_formula_sheet_scope() {
    let mut wb = Workbook::new();
    wb.sheets.push(Arc::new(Sheet::new("Results")));
    for (name, scope, text) in [
        ("Rate", None, "2"),
        ("Inputs", None, "Sheet1!$A$1:$A$3"),
        ("Single", None, "Sheet1!$A$1"),
        ("Alias", None, "Inputs"),
        ("Scoped", None, "Sheet1!$A$1:$A$3"),
        ("Scoped", Some(0), "2"),
        ("LocalInputs", Some(1), "Results!$A$1:$A$3"),
    ] {
        wb.names.push(DefinedName { name: name.into(), scope, formula: text.into(), comment: String::new(), hidden: false });
    }
    check_formula_metadata(
        wb.clone(),
        0,
        &[
            ("Rate", false),
            ("SQRT(Rate)", false),
            ("Single*2", false),
            ("Inputs", true),
            ("SUM(Inputs)", false),
            ("SUM(Inputs*2)", true),
            ("SUM(SQRT(Alias))", true),
            ("SQRT(Scoped)", false),
        ],
    );
    check_formula_metadata(wb, 1, &[("SQRT(Scoped)", true), ("SUM(LocalInputs)", false), ("LocalInputs*2", true)]);
}

#[test]
fn cyclic_names_preserve_array_evaluation_without_unbounded_export_recursion() {
    let mut wb = Workbook::new();
    for (name, formula) in [("First", "Second"), ("Second", "First")] {
        wb.names.push(DefinedName { name: name.into(), scope: None, formula: formula.into(), comment: String::new(), hidden: false });
    }
    check_formula_metadata(wb, 0, &[("First", true), ("SUM(First)", true)]);
}

#[test]
fn implicit_intersection_metadata_distinguishes_table_rows_and_columns() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.tables.push(Table {
        id: 1,
        name: "Data".into(),
        range: range("A1:B3"),
        header_row: true,
        totals_row: false,
        columns: vec![
            TableColumn { name: "Input".into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None },
            TableColumn {
                name: "Output".into(),
                totals: TotalsFn::None,
                totals_label: None,
                formula: Some("[@Input]*2".into()),
                unique_name: None,
                xml: None,
            },
        ],
        style: "TableStyleLight1".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    sh.cells.set(at("A2"), Cell::value(Value::Number(3.0)));
    sh.cells.set(at("B2"), formula("[@Input]*2", 6.0));
    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    assert_scalar(&sheet, "B2");
    check_formula_metadata(
        wb,
        0,
        &[("Data[@Input]*2", false), ("SUM(Data[Input])", false), ("SUM(Data[Input]*2)", true), ("SQRT(Data[Input])", true)],
    );
}

/// How `export` saved a formula, as comparable text.
fn export_text(wb: &Workbook, at: CellRef, text: &str, names: &mut crate::array_formula::NameCache) -> String {
    use crate::array_formula::{Export, export};
    match export(wb, 0, at, &Formula::new(text), crate::FunctionInfo::default(), names) {
        Export::Unchanged => "unchanged".into(),
        Export::Dynamic => "dynamic".into(),
        Export::Legacy(e) => format!("legacy {}", gridcraft_formula::print(&e)),
    }
}

#[test]
fn defined_names_are_analysed_once_for_many_formulas() {
    use crate::array_formula::NameCache;
    let mut wb = Workbook::new();
    // A large definition: many operations, ending in a range.
    let big = (1..=500).map(|i| format!("Sheet1!$A${i}")).collect::<Vec<_>>().join("+");
    for (name, text) in [("Big", format!("({big})*Sheet1!$B$1:$B$3")), ("Rng", "Sheet1!$A$1:$A$3".to_string()), ("Alias", "Rng".to_string())] {
        wb.names.push(DefinedName { name: name.into(), scope: None, formula: text, comment: String::new(), hidden: false });
    }
    let texts = ["SUM(Big)", "SQRT(Big)", "Big*2", "SQRT(Alias)", "SUM(Alias)", "Alias", "SUM(Rng*2)", "Missing*2"];
    let mut shared = NameCache::default();
    for i in 0..400u32 {
        for text in texts {
            let at = CellRef::new(i + 10, 5);
            let with_shared = export_text(&wb, at, text, &mut shared);
            let fresh = export_text(&wb, at, text, &mut NameCache::default());
            assert_eq!(with_shared, fresh, "{text}");
        }
    }
    assert!(gridcraft_formula::parse(&wb.names[0].formula).is_ok(), "the large name must parse");
    // 3 names, each analysed once for all 3200 formulas (a fresh cache analyses a name per formula).
    assert_eq!(shared.analyses, 3);
}

#[test]
fn shared_name_analysis_keeps_each_formulas_limits_and_position() {
    use crate::array_formula::NameCache;
    let mut wb = Workbook::new();
    let terms = |n: usize| (1..=n).map(|i| format!("Sheet1!$A${i}")).collect::<Vec<_>>().join("+");
    // Twenty names of about a thousand nodes each: a formula using all of them needs more than
    // its 16,384-node budget, one using a few does not.
    for i in 0..20 {
        let text = format!("({})*Sheet1!$B$1:$B$3", terms(500));
        wb.names.push(DefinedName { name: format!("Big_{i}"), scope: None, formula: text, comment: String::new(), hidden: false });
    }
    for (name, text) in [
        ("Cyc1", "Cyc2".to_string()),
        ("Cyc2", "Cyc1".to_string()),
        ("Deep", format!("{}1{}", "(".repeat(100), ")".repeat(100))),
        ("Tbl", "Table1[Col]".to_string()),
    ] {
        wb.names.push(DefinedName { name: name.into(), scope: None, formula: text, comment: String::new(), hidden: false });
    }
    assert!(wb.names.iter().all(|n| gridcraft_formula::parse(&n.formula).is_ok()), "every name must parse");
    let uses = |n: usize| (0..n).map(|i| format!("SQRT(Big_{i})")).collect::<Vec<_>>().join("+");
    let mut texts: Vec<String> = [1, 5, 10, 15, 17, 20].iter().map(|n| uses(*n)).collect();
    texts.extend(["SQRT(Big_3)", "Cyc1", "SUM(Cyc1)", "SUM(Deep)", "SQRT(Tbl)", "((((((((SQRT(Deep))))))))"].map(String::from));
    // The limit is really reached: a few uses analyse fully, all twenty stop at the budget.
    let probe = |n: usize| export_text(&wb, CellRef::new(5, 5), &uses(n), &mut NameCache::default());
    assert_eq!(probe(1), "dynamic");
    assert_eq!(probe(20), "unchanged");
    let mut shared = NameCache::default();
    for round in 0..3 {
        for text in &texts {
            for at in [CellRef::new(5, 5), CellRef::new(7, 9)] {
                let with_shared = export_text(&wb, at, text, &mut shared);
                let fresh = export_text(&wb, at, text, &mut NameCache::default());
                assert_eq!(with_shared, fresh, "round {round}: {}", &text[..text.len().min(60)]);
            }
        }
    }
}

/// A name defined as another name reads the same from the shared cache in either order: after
/// the other name (whose cost and uncertainty the formula already paid) and before it (a hit
/// records the other name too, so the formula does not pay for it twice).
#[test]
fn shared_name_analysis_of_names_defined_by_names_matches_fresh_in_both_orders() {
    use crate::array_formula::NameCache;
    let mut wb = Workbook::new();
    let terms = (1..=500).map(|i| format!("Sheet1!$A${i}")).collect::<Vec<_>>().join("+");
    // `Unsure` is outside the established legacy semantics (ISOMITTED), and `Alias` is it by name.
    let mut defs = vec![
        ("Unsure".to_string(), "IF(ISOMITTED(Sheet1!$C$1),Sheet1!$A$1:$A$3,Sheet1!$B$1:$B$3)".to_string()),
        ("Alias".to_string(), "Unsure".to_string()),
    ];
    for i in 0..10 {
        defs.push((format!("Big_{i}"), format!("({terms})*Sheet1!$B$1:$B$3")));
        defs.push((format!("Via_{i}"), format!("Big_{i}")));
    }
    for (name, formula) in defs {
        wb.names.push(DefinedName { name, scope: None, formula, comment: String::new(), hidden: false });
    }
    // Ten names of about a thousand nodes, each reached twice: within the budget only if the
    // second reference is free.
    let both = |first: &str, second: &str| (0..10).map(|i| format!("SQRT({first}_{i})+SQRT({second}_{i})")).collect::<Vec<_>>().join("+");
    let orders = [
        ["Unsure+SQRT(Alias)", "SQRT(Alias)+Unsure", "SQRT(Alias)+SQRT(Sheet1!$A$1:$A$3)", "@SQRT(Alias)"].map(String::from).to_vec(),
        ["SQRT(Alias)", "SQRT(Alias)+Unsure", "Unsure+SQRT(Alias)", "SQRT(Alias)+SQRT(Sheet1!$A$1:$A$3)", "@SQRT(Alias)"].map(String::from).to_vec(),
        vec![both("Big", "Via"), both("Via", "Big"), "SQRT(Via_0)".to_string(), both("Via", "Big")],
        vec!["SQRT(Via_0)".to_string(), both("Via", "Big"), both("Big", "Via"), both("Via", "Big")],
    ];
    let at = CellRef::new(5, 5);
    assert_eq!(export_text(&wb, at, &both("Via", "Big"), &mut NameCache::default()), "dynamic", "fresh fits the budget");
    for texts in orders {
        let mut shared = NameCache::default();
        for text in &texts {
            let with_shared = export_text(&wb, at, text, &mut shared);
            let fresh = export_text(&wb, at, text, &mut NameCache::default());
            assert_eq!(with_shared, fresh, "{}", &text[..text.len().min(60)]);
        }
        // Reading: one pass over the formulas, one row each, against each formula read alone.
        let read = |cells: &[(CellRef, &String)], one_pass: bool| -> Vec<String> {
            let mut wb = wb.clone();
            for (c, text) in cells {
                Arc::make_mut(&mut wb.sheets[0]).cells.set(*c, Cell::formula(Formula::new(text)));
            }
            let all: Vec<CellRef> = cells.iter().map(|(c, _)| *c).collect();
            if one_pass {
                crate::array_formula::apply_legacy(&mut wb, vec![(0, all.clone())], crate::FunctionInfo::default());
            } else {
                for c in &all {
                    crate::array_formula::apply_legacy(&mut wb, vec![(0, vec![*c])], crate::FunctionInfo::default());
                }
            }
            all.iter().map(|c| gridcraft_formula::print(&wb.sheets[0].cells.get(*c).unwrap().formula.as_ref().unwrap().expr_arc().unwrap())).collect()
        };
        let cells: Vec<(CellRef, &String)> = texts.iter().enumerate().map(|(i, t)| (CellRef::new(i as u32 + 5, 5), t)).collect();
        assert_eq!(read(&cells, true), read(&cells, false));
    }
}

#[test]
fn long_flat_chains_are_scalar_formulas() {
    // `=A1+A2+…` nests one level per operator. Past the scan's depth limit of 64, a plain sum
    // of 66 cells was written as a dynamic-array formula; Excel writes it as a plain formula.
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    let refs = (1..=1500).map(|r| format!("A{r}")).collect::<Vec<_>>().join("+");
    let mixed = (1..=1000).map(|r| format!("A{r}*2")).collect::<Vec<_>>().join("-");
    let literals = vec!["1"; 4096].join("&");
    let ranges = format!("{refs}+A1:A3");
    sh.cells.set(at("B1"), formula(&refs, 0.0));
    sh.cells.set(at("B2"), formula(&mixed, 0.0));
    sh.cells.set(at("B3"), formula(&literals, 0.0));
    sh.cells.set(at("B4"), formula(&ranges, 0.0));
    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    for address in ["B1", "B2", "B3"] {
        assert_scalar(&sheet, address);
    }
    // A range anywhere in the chain still needs array evaluation.
    assert_dynamic(&sheet, "B4", "B4");
    assert_eq!(cell(&sheet, "B1").child("f").unwrap().text, refs);
}

#[test]
fn spilled_text_is_written_as_a_cached_string_not_a_shared_string() {
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    let mut anchor = Cell::formula(Formula::new("SORT({\"b\";\"a\";\"c\"})"));
    anchor.value = Value::text("a");
    sh.cells.set(at("K4"), anchor);
    sh.spill_ranges.insert(at("K4"), range("K4:K6"));
    sh.spill.insert(at("K5"), Value::text("b"));
    sh.spill.insert(at("K6"), Value::text("c"));

    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    for (address, text) in [("K4", "a"), ("K5", "b"), ("K6", "c")] {
        let c = cell(&sheet, address);
        assert_eq!(c.attr("t"), Some("str"), "{address}");
        assert_eq!(c.child("v").unwrap().text, text, "{address}");
    }
}
