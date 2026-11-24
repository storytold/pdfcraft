//! Literal chart categories that contain a comma, a semicolon or a quote stay one category on save.

use gridcraft_model::{Anchor, Chart, ChartKind, LegendPos, Series, Sheet, Workbook};
use std::sync::Arc;

use crate::{read_xlsx, write_xlsx};

#[test]
fn literal_category_with_separators_survives_save() {
    // (typed array constant, what reading the saved file gives back)
    let cats = [
        (r#"{"a,b"}"#, r#"{"a,b"}"#),
        (r#"{"a;b"}"#, r#"{"a;b"}"#),
        (r#"{"red","blue"}"#, r#"{"red","blue"}"#),
        (r#"{"say ""hi"", ok"}"#, r#"{"say ""hi"", ok"}"#),
        (r#"{"a,b","c;d","e"}"#, r#"{"a,b","c;d","e"}"#),
        (r#"{ "Jan" , "Feb, Mar" ; Apr }"#, r#"{"Jan","Feb, Mar","Apr"}"#),
    ];
    let mut wb = Workbook::default();
    let mut s = Sheet::new("Data");
    let series: Vec<Series> = cats
        .iter()
        .map(|(c, _)| Series {
            name: Some("North, South".into()),
            categories: Some((*c).into()),
            values: "{1,2,3}".into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
        })
        .collect();
    s.charts.push(Chart {
        id: 1,
        kind: ChartKind::ColumnClustered,
        anchor: Anchor { cell: gridcraft_core::CellRef::new(0, 3), width: 480.0, height: 300.0, ..Anchor::default() },
        title: None,
        series,
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
    wb.sheets = vec![Arc::new(s)];
    let bytes = write_xlsx(&wb).unwrap();
    let (back, _rep) = read_xlsx(&bytes).unwrap();
    let saved = &back.sheets[0].charts[0].series;
    for (i, (_, want)) in cats.iter().enumerate() {
        assert_eq!(saved[i].categories.as_deref(), Some(*want));
        assert_eq!(saved[i].name.as_deref(), Some("North, South"));
    }
}
