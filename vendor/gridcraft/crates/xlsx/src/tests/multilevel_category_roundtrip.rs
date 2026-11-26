//! A two-column category range (year beside month) is written as a multi-level string reference
//! with Excel's level caches, and read back as the same reference.

use std::io::Read as _;
use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Anchor, Chart, ChartKind, LegendPos, Series, Sheet, Workbook};

use crate::{read_xlsx, write_xlsx};

const YEARS: [&str; 6] = ["2023", "", "", "2024", "", ""];
const MONTHS: [&str; 6] = ["окт", "ноя", "дек", "янв", "фев", "мар"];

fn workbook(categories: &str, values: &str) -> Workbook {
    let mut s = Sheet::new("Data");
    for (i, (y, m)) in YEARS.iter().zip(MONTHS).enumerate() {
        let r = i as u32 + 1;
        if !y.is_empty() {
            s.set_value(CellRef::new(r, 0), Value::from(*y));
        }
        s.set_value(CellRef::new(r, 1), Value::from(m));
        s.set_value(CellRef::new(r, 2), Value::Number(i as f64 + 1.0));
    }
    s.charts.push(Chart {
        id: 1,
        kind: ChartKind::Line,
        anchor: Anchor { cell: CellRef::new(0, 4), width: 480.0, height: 300.0, ..Anchor::default() },
        title: None,
        series: vec![Series {
            name: None,
            categories: Some(categories.into()),
            values: values.into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            smooth: false,
            trendlines: Vec::new(),
            quartiles: Default::default(),
        }],
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
    Workbook { sheets: vec![Arc::new(s)], ..Workbook::default() }
}

fn chart_xml(bytes: &[u8]) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut s = String::new();
    z.by_name("xl/charts/chart1.xml").unwrap().read_to_string(&mut s).unwrap();
    s
}

fn cat(xml: &str) -> &str {
    let a = xml.find("<c:cat>").expect("a <c:cat>");
    let b = xml.find("</c:cat>").expect("a </c:cat>");
    &xml[a..b]
}

#[test]
fn two_column_categories_are_written_as_levels_and_read_back() {
    let bytes = write_xlsx(&workbook("Data!$A$2:$B$7", "Data!$C$2:$C$7")).unwrap();
    let xml = chart_xml(&bytes);
    let c = cat(&xml);
    assert!(c.starts_with("<c:cat><c:multiLvlStrRef><c:f>Data!$A$2:$B$7</c:f><c:multiLvlStrCache><c:ptCount val=\"6\"/><c:lvl>"), "{c}");
    let lvls: Vec<&str> = c.split("<c:lvl>").skip(1).collect();
    assert_eq!(lvls.len(), 2, "{c}");
    // The innermost level first, every month; then the years, only where a group starts.
    assert!(lvls[0].contains("<c:pt idx=\"0\"><c:v>окт</c:v></c:pt>") && lvls[0].contains("<c:pt idx=\"5\"><c:v>мар</c:v></c:pt>"), "{c}");
    assert!(lvls[1].starts_with("<c:pt idx=\"0\"><c:v>2023</c:v></c:pt><c:pt idx=\"3\"><c:v>2024</c:v></c:pt></c:lvl>"), "{c}");
    assert!(!c.contains("<c:strRef>"));

    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheets[0].charts[0].series[0].categories.as_deref(), Some("Data!$A$2:$B$7"));
    assert_eq!(cat(&chart_xml(&write_xlsx(&back).unwrap())), c, "stable across saves");
}

#[test]
fn a_row_of_values_makes_the_levels_rows() {
    let mut wb = workbook("Data!$A$9:$F$10", "Data!$A$11:$F$11");
    let s = Arc::make_mut(&mut wb.sheets[0]);
    for (i, (y, m)) in YEARS.iter().zip(MONTHS).enumerate() {
        if !y.is_empty() {
            s.set_value(CellRef::new(8, i as u32), Value::from(*y));
        }
        s.set_value(CellRef::new(9, i as u32), Value::from(m));
        s.set_value(CellRef::new(10, i as u32), Value::Number(i as f64));
    }
    let xml = chart_xml(&write_xlsx(&wb).unwrap());
    let c = cat(&xml);
    assert!(c.contains("<c:multiLvlStrRef><c:f>Data!$A$9:$F$10</c:f><c:multiLvlStrCache><c:ptCount val=\"6\"/>"), "{c}");
    let lvls: Vec<&str> = c.split("<c:lvl>").skip(1).collect();
    assert_eq!(lvls.len(), 2);
    assert!(lvls[0].contains("<c:v>окт</c:v>") && lvls[1].contains("<c:pt idx=\"3\"><c:v>2024</c:v></c:pt>"), "{c}");
}

#[test]
fn a_single_column_of_categories_stays_a_plain_reference() {
    let xml = chart_xml(&write_xlsx(&workbook("Data!$B$2:$B$7", "Data!$C$2:$C$7")).unwrap());
    assert!(xml.contains("<c:cat><c:strRef><c:f>Data!$B$2:$B$7</c:f>"), "{xml}");
    assert!(!xml.contains("multiLvlStrRef"));
}
