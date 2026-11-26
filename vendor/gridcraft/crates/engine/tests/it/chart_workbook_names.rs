//! Chart series that point at workbook-scoped defined names. Excel writes these as `[0]!Name`
//! in `<c:f>`; they used to read back verbatim and evaluate to `#NAME?`, so a chart
//! driven by dynamic (OFFSET) names opened blank with a single `#NAME?` category.

use std::io::{Read, Write};

use gridcraft_engine::{Session, io};
use gridcraft_model::Workbook;
use serde_json::json;

const TASKS: usize = 6;

/// A plan table, OFFSET names over its structured references (and names built on names; one
/// scoped to the sheet), and a stacked bar chart whose series name those names.
fn plan() -> Workbook {
    let mut s = Session::new();
    s.new_workbook();
    let mut rows = vec![json!(["Task", "Start", "Done", "Left"])];
    for i in 0..TASKS {
        rows.push(json!([format!("Task {}", i + 1), (45_200 + 7 * i).to_string(), (i + 1).to_string(), (TASKS - i).to_string()]));
    }
    s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    let range = format!("A1:D{}", TASKS + 1);
    s.execute("insert.table", json!({"range": range, "header": true, "name": "Plan"})).unwrap();
    for (name, refers, scope) in [
        ("TaskNames", "=OFFSET(Plan[[#Headers],[Task]],1,0,COUNTA(Plan[Task]),1)", "Workbook"),
        ("TaskStart", "=OFFSET(TaskNames,0,1)", "Workbook"),
        ("TaskDone", "=OFFSET(TaskNames,0,2)", "Workbook"),
        ("TaskLeft", "=OFFSET(TaskNames,0,3)", "Sheet1"),
        ("LeftTitle", "=Sheet1!$D$1", "Workbook"),
        ("DoneTitle", "=Sheet1!$C$1", "Sheet1"),
    ] {
        s.execute("formulas.defineName", json!({"name": name, "refersTo": refers, "scope": scope})).unwrap();
    }
    s.execute("insert.chart", json!({"range": range, "type": "bar", "subtype": "stacked"})).unwrap();
    let mut wb = Workbook::clone(&s.doc().unwrap().wb);
    let chart = &mut wb.sheet_mut(0).unwrap().charts[0];
    assert_eq!(chart.series.len(), 3);
    for (ser, (name, values)) in
        chart.series.iter_mut().zip([("Sheet1!$B$1", "TaskStart"), ("Sheet1!DoneTitle", "TaskDone"), ("LeftTitle", "TaskLeft")])
    {
        ser.name = Some(name.into());
        ser.categories = Some("TaskNames".into());
        ser.values = values.into();
    }
    wb
}

fn chart_xml(bytes: &[u8]) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut f = z.by_name("xl/charts/chart1.xml").unwrap();
    let mut xml = String::new();
    f.read_to_string(&mut xml).unwrap();
    xml
}

/// The package with `edit` applied to its chart part.
fn edit_chart(bytes: &[u8], edit: impl Fn(String) -> String) -> Vec<u8> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().to_string();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        if name == "xl/charts/chart1.xml" {
            data = edit(String::from_utf8(data).unwrap()).into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(&data).unwrap();
    }
    out.finish().unwrap().into_inner()
}

fn reopen(bytes: &[u8]) -> Workbook {
    io::open_bytes("plan.xlsx", bytes).unwrap().0
}

#[test]
fn workbook_names_in_chart_series_resolve_after_save_and_reopen() {
    let bytes = io::save_bytes(&plan(), "plan.xlsx").unwrap();
    // Qualified, as Excel needs them: it draws a chart with a bare name in `<c:f>` blank.
    let xml = chart_xml(&bytes);
    for f in ["[0]!TaskNames", "[0]!TaskStart", "[0]!TaskDone", "Sheet1!TaskLeft", "[0]!LeftTitle"] {
        assert!(xml.contains(&format!("<c:f>{f}</c:f>")), "{f} missing from {xml}");
    }
    assert!(!xml.contains("<c:f>Task"), "a bare name in {xml}");
    // Series names that are defined names stay references, qualified or not.
    assert!(xml.contains("<c:tx><c:strRef><c:f>Sheet1!DoneTitle</c:f>"), "{xml}");
    // A series name that is a defined name stays a reference, not literal text.
    assert!(!xml.contains("<c:v>LeftTitle</c:v>"));

    let wb = reopen(&bytes);
    let chart = &wb.sheet(0).unwrap().charts[0];
    assert_eq!(chart.series[0].categories.as_deref(), Some("TaskNames"));
    assert_eq!(chart.series[0].values, "TaskStart");
    assert_eq!(chart.series[2].values, "Sheet1!TaskLeft");
    assert_eq!(chart.series[2].name.as_deref(), Some("LeftTitle"));

    let data = gridcraft_chart::resolve(&wb, 0, chart);
    let tasks: Vec<String> = (1..=TASKS).map(|i| format!("Task {i}")).collect();
    assert_eq!(data.categories, tasks);
    let names: Vec<&str> = data.series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Start", "Done", "Left"]);
    let start: Vec<Option<f64>> = (0..TASKS).map(|i| Some((45_200 + 7 * i) as f64)).collect();
    assert_eq!(data.series[0].values, start);
    let left: Vec<Option<f64>> = (0..TASKS).map(|i| Some((TASKS - i) as f64)).collect();
    assert_eq!(data.series[2].values, left);

    // A second save and reopen gives the same chart data.
    let again = reopen(&io::save_bytes(&wb, "plan.xlsx").unwrap());
    assert_eq!(gridcraft_chart::resolve(&again, 0, &again.sheet(0).unwrap().charts[0]), data);
}

#[test]
fn a_missing_workbook_name_resolves_to_no_values_without_panicking() {
    let bytes = io::save_bytes(&plan(), "plan.xlsx").unwrap();
    let bytes = edit_chart(&bytes, |x| x.replace("Sheet1!TaskLeft", "[0]!NoSuchName"));
    let wb = reopen(&bytes);
    let chart = &wb.sheet(0).unwrap().charts[0];
    assert_eq!(chart.series[2].values, "NoSuchName");
    let data = gridcraft_chart::resolve(&wb, 0, chart);
    assert!(data.series[2].values.iter().all(Option::is_none), "{:?}", data.series[2].values);
    // The other series still plot.
    assert_eq!(data.series[0].values.len(), TASKS);
}

#[test]
fn external_workbook_refs_in_chart_series_are_kept_as_they_were() {
    let bytes = io::save_bytes(&plan(), "plan.xlsx").unwrap();
    let bytes = edit_chart(&bytes, |x| x.replace("[0]!TaskDone", "[1]Sheet1!$C$2:$C$7"));
    let wb = reopen(&bytes);
    assert_eq!(wb.sheet(0).unwrap().charts[0].series[1].values, "[1]Sheet1!$C$2:$C$7");
}
