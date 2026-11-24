//! `[0]!Name` ("this workbook") in chart references: read as the bare name, written back for
//! workbook-scoped names only.

use std::io::Read;
use std::sync::Arc;

use gridcraft_model::{Anchor, Chart, ChartKind, DefinedName, LegendPos, Series, Sheet, Workbook};

use crate::chart::chart_ref_from_file;
use crate::{read_xlsx, write_xlsx};

#[test]
fn this_book_prefix_is_dropped_only_in_front_of_names() {
    for (file, model) in [
        ("[0]!TaskNames", "TaskNames"),
        ("=[0]!TaskNames", "TaskNames"),
        ("  [0]!_Hidden ", "_Hidden"),
        ("([0]!a,[0]!b)", "(a,b)"),
        // External books keep their index: they go through externalLink parts.
        ("[1]Sheet1!$A$1:$A$5", "[1]Sheet1!$A$1:$A$5"),
        ("[2]!Rates", "[2]!Rates"),
        // Not a name after the prefix, or not at a name's start.
        ("[0]!$A$1", "[0]!$A$1"),
        ("Plan[[0]]", "Plan[[0]]"),
        // Inside a quoted sheet name or a string.
        ("'[0]!x'!$A$1", "'[0]!x'!$A$1"),
        ("\"[0]!x\"", "\"[0]!x\""),
        ("'It''s [0]!'!$A$1", "'It''s [0]!'!$A$1"),
        ("Sheet1!$B$2:$B$9", "Sheet1!$B$2:$B$9"),
        ("", ""),
        ("[0]!", "[0]!"),
        ("[0]!é", "é"),
    ] {
        assert_eq!(chart_ref_from_file(file), model, "{file}");
    }
}

fn series(name: &str, values: &str) -> Series {
    Series {
        name: Some(name.into()),
        categories: Some("Items".into()),
        values: values.into(),
        bubble_sizes: None,
        color: None,
        secondary: false,
        kind: None,
        smooth: false,
        trendlines: Vec::new(),
        quartiles: Default::default(),
    }
}

fn name(n: &str, scope: Option<usize>, formula: &str) -> DefinedName {
    DefinedName { name: n.into(), scope, formula: formula.into(), comment: String::new(), hidden: false }
}

/// Excel draws a chart blank when a `<c:f>` holds a bare name (observed in Excel for Windows,
/// Microsoft 365, with no repair prompt); `[0]!Name` and `Sheet!Name` both plot.
#[test]
fn chart_names_are_written_qualified() {
    let mut wb = Workbook::default();
    let mut s = Sheet::new("Data");
    s.charts.push(Chart {
        id: 1,
        kind: ChartKind::BarStacked,
        anchor: Anchor { cell: gridcraft_core::CellRef::new(0, 3), width: 480.0, height: 300.0, ..Anchor::default() },
        title: None,
        series: vec![
            series("Title", "Amounts"),
            series("Data!$B$1", "Local"),
            series("\"Plain\"", "Data!$B$2:$B$4"),
            series("'My Data'!Other", "'My Data'!Other"),
            series("Literal", "NoSuchName"),
            series("Gone!Thing", "Gone!Thing"),
        ],
        legend: LegendPos::Bottom,
        data_labels: false,
        data_table: None,
        gridlines: true,
        style: 2,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: None,
        y_title: None,
        source: None,
        by_rows: false,
        pivot: None,
    });
    wb.sheets = vec![Arc::new(s), Arc::new(Sheet::new("My Data"))];
    wb.names = vec![
        name("Items", None, "Data!$A$2:$A$4"),
        name("Amounts", None, "OFFSET(Items,0,1)"),
        name("Title", None, "Data!$B$1"),
        name("Local", Some(0), "Data!$C$2:$C$4"),
        name("Other", Some(1), "'My Data'!$A$1:$A$3"),
    ];
    let bytes = write_xlsx(&wb).unwrap();
    let mut xml = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().by_name("xl/charts/chart1.xml").unwrap().read_to_string(&mut xml).unwrap();
    for f in [
        // Workbook-scoped: `[0]!`, as Excel writes it.
        "<c:f>[0]!Items</c:f>",
        "<c:f>[0]!Amounts</c:f>",
        "<c:tx><c:strRef><c:f>[0]!Title</c:f>",
        // Sheet-scoped: qualified with its sheet.
        "<c:f>Data!Local</c:f>",
        "<c:tx><c:strRef><c:f>'My Data'!Other</c:f>",
        "<c:val><c:numRef><c:f>'My Data'!Other</c:f>",
        // Undefined: `[0]!`, as a file that referred to it had.
        "<c:f>[0]!NoSuchName</c:f>",
        // Literal series names stay literal.
        "<c:tx><c:v>Plain</c:v></c:tx>",
        "<c:tx><c:v>Literal</c:v></c:tx>",
        // An unknown sheet is kept as written, never re-qualified.
        "<c:f>Gone!Thing</c:f>",
    ] {
        assert!(xml.contains(f), "{f} missing from {xml}");
    }
    for bare in ["<c:f>Local</c:f>", "<c:f>Items</c:f>", "<c:f>Amounts</c:f>", "<c:f>Other</c:f>", "<c:f>NoSuchName</c:f>"] {
        assert!(!xml.contains(bare), "bare {bare} in {xml}");
    }

    let (back, _) = read_xlsx(&bytes).unwrap();
    let got: Vec<(Option<&str>, Option<&str>, &str)> =
        back.sheets[0].charts[0].series.iter().map(|s| (s.name.as_deref(), s.categories.as_deref(), s.values.as_str())).collect();
    assert_eq!(
        got,
        [
            (Some("Title"), Some("Items"), "Amounts"),
            (Some("Data!$B$1"), Some("Items"), "Data!Local"),
            (Some("Plain"), Some("Items"), "Data!$B$2:$B$4"),
            (Some("'My Data'!Other"), Some("Items"), "'My Data'!Other"),
            (Some("Literal"), Some("Items"), "NoSuchName"),
            (Some("Gone!Thing"), Some("Items"), "Gone!Thing"),
        ]
    );
    // A second save writes the same references.
    let mut again = String::new();
    let bytes2 = write_xlsx(&back).unwrap();
    zip::ZipArchive::new(std::io::Cursor::new(&bytes2)).unwrap().by_name("xl/charts/chart1.xml").unwrap().read_to_string(&mut again).unwrap();
    let refs = |x: &str| x.split("<c:f>").skip(1).filter_map(|p| p.split_once("</c:f>").map(|(f, _)| f.to_string())).collect::<Vec<_>>();
    assert_eq!(refs(&again), refs(&xml));
}

/// A quoted sheet name keeps its outer spaces: `' Sales '!Data` is not `Sales!Data`, so the
/// chart doesn't switch to the other sheet's name on save.
#[test]
fn chart_names_keep_spaces_around_a_quoted_sheet_name() {
    let mut wb = Workbook::default();
    let mut s = Sheet::new("Sales");
    s.charts.push(Chart {
        id: 1,
        kind: ChartKind::BarStacked,
        anchor: Anchor { cell: gridcraft_core::CellRef::new(0, 3), width: 480.0, height: 300.0, ..Anchor::default() },
        title: None,
        series: vec![series("' Sales '!Label", "' Sales '!Data")],
        legend: LegendPos::Bottom,
        data_labels: false,
        data_table: None,
        gridlines: true,
        style: 2,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: None,
        y_title: None,
        source: None,
        by_rows: false,
        pivot: None,
    });
    wb.sheets = vec![Arc::new(s), Arc::new(Sheet::new(" Sales "))];
    wb.names = vec![
        name("Items", None, "Sales!$A$2:$A$4"),
        name("Data", Some(0), "Sales!$B$2:$B$4"),
        name("Data", Some(1), "' Sales '!$B$2:$B$4"),
        name("Label", Some(0), "Sales!$B$1"),
        name("Label", Some(1), "' Sales '!$B$1"),
    ];
    let bytes = write_xlsx(&wb).unwrap();
    let mut xml = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().by_name("xl/charts/chart1.xml").unwrap().read_to_string(&mut xml).unwrap();
    assert!(xml.contains("<c:val><c:numRef><c:f>' Sales '!Data</c:f>"), "{xml}");
    assert!(xml.contains("<c:tx><c:strRef><c:f>' Sales '!Label</c:f>"), "{xml}");
}
