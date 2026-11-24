//! Test that XLSX smooth line settings round-trip correctly.

use gridcraft_model::{Anchor, Chart, ChartKind, Color, LegendPos, Series, Sheet, Workbook};
use std::sync::Arc;

use crate::{read_xlsx, write_xlsx};

#[test]
fn smooth_line_setting_round_trips() {
    let mut wb = Workbook::default();
    let mut s = Sheet::new("Data");
    let base_series = Series {
        name: Some("Data!$A$1".into()),
        categories: Some("Data!$A$2:$A$5".into()),
        values: "Data!$B$2:$B$5".into(),
        bubble_sizes: None,
        color: Some(Color::Rgb(0x4472C4)),
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
    };
    s.charts.push(Chart {
        id: 1,
        kind: ChartKind::Line,
        anchor: Anchor { cell: gridcraft_core::CellRef::new(0, 3), width: 480.0, height: 300.0, ..Anchor::default() },
        title: Some("Straight Line".into()),
        series: vec![base_series.clone()],
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
    let mut smooth_series = base_series.clone();
    smooth_series.smooth = true;
    s.charts.push(Chart {
        id: 2,
        kind: ChartKind::ScatterLines,
        anchor: Anchor { cell: gridcraft_core::CellRef::new(15, 3), width: 480.0, height: 300.0, ..Anchor::default() },
        title: Some("Smooth Scatter".into()),
        series: vec![smooth_series],
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
    assert_eq!(back.sheets.len(), 1);
    let sh = &back.sheets[0];
    assert_eq!(sh.charts.len(), 2);
    assert!(!sh.charts[0].series[0].smooth, "straight line series should have smooth=false");
    assert!(sh.charts[1].series[0].smooth, "smooth scatter series should have smooth=true");
}
