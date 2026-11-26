//! Chart trendlines (`<c:trendline>`) round-trip through XLSX in every chart group that has them.

use std::io::Read;
use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Anchor, Chart, ChartKind, Color, LegendPos, LineDash, Series, Sheet, Trendline, TrendlineKind, Workbook};

use crate::{read_xlsx, write_xlsx};

fn all_trendlines() -> Vec<Trendline> {
    vec![
        Trendline {
            kind: TrendlineKind::Linear,
            forward: 2.0,
            backward: 0.5,
            display_equation: true,
            display_r_squared: true,
            ..Trendline::default()
        },
        Trendline { kind: TrendlineKind::Exponential, name: Some("Growth & decay".into()), intercept: Some(1.5), ..Trendline::default() },
        Trendline {
            kind: TrendlineKind::Logarithmic,
            color: Some(Color::Rgb(0xC00000)),
            width: Some(2.25),
            dash: Some(LineDash::SysDot),
            ..Trendline::default()
        },
        Trendline { kind: TrendlineKind::Polynomial(2), ..Trendline::default() },
        Trendline { kind: TrendlineKind::Polynomial(6), intercept: Some(-3.0), display_equation: true, ..Trendline::default() },
        Trendline { kind: TrendlineKind::Power, display_r_squared: true, dash: Some(LineDash::LgDashDot), ..Trendline::default() },
        Trendline { kind: TrendlineKind::MovingAverage(4), ..Trendline::default() },
    ]
}

fn chart(id: u32, kind: ChartKind, trendlines: Vec<Trendline>) -> Chart {
    Chart {
        id,
        kind,
        anchor: Anchor { cell: CellRef::new(id * 16, 3), width: 480.0, height: 300.0, ..Anchor::default() },
        title: None,
        series: vec![Series {
            name: Some("Data!$B$1".into()),
            categories: Some("Data!$A$2:$A$6".into()),
            values: "Data!$B$2:$B$6".into(),
            bubble_sizes: None,
            color: Some(Color::Rgb(0x4472C4)),
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines,
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
    }
}

const KINDS: [ChartKind; 8] = [
    ChartKind::Line,
    ChartKind::LineMarkers,
    ChartKind::ColumnClustered,
    ChartKind::BarClustered,
    ChartKind::Area,
    ChartKind::Scatter,
    ChartKind::ScatterLines,
    ChartKind::Bubble,
];

fn workbook() -> Workbook {
    let mut s = Sheet::new("Data");
    s.set_value(CellRef::new(0, 1), Value::Text("Sales".into()));
    for r in 1..=5u32 {
        s.set_value(CellRef::new(r, 0), Value::Number(f64::from(r)));
        s.set_value(CellRef::new(r, 1), Value::Number(f64::from(r * r)));
    }
    for (i, k) in KINDS.iter().enumerate() {
        s.charts.push(chart(i as u32 + 1, *k, all_trendlines()));
    }
    Workbook { sheets: vec![Arc::new(s)], ..Workbook::default() }
}

#[test]
fn every_trendline_kind_round_trips_in_every_chart_group() {
    let bytes = write_xlsx(&workbook()).unwrap();
    let (back, _rep) = read_xlsx(&bytes).unwrap();
    let charts = &back.sheets[0].charts;
    assert_eq!(charts.len(), KINDS.len());
    for (c, k) in charts.iter().zip(KINDS) {
        assert_eq!(c.kind, k);
        assert_eq!(c.series[0].trendlines, all_trendlines(), "{k:?}");
    }
    // A second save writes the same parts (nothing drifts on the way round).
    let again = write_xlsx(&back).unwrap();
    assert_eq!(chart_parts(&again), chart_parts(&bytes));
}

fn chart_parts(bytes: &[u8]) -> Vec<String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut names: Vec<String> =
        (0..z.len()).map(|i| z.name_for_index(i).unwrap().to_string()).filter(|n| n.starts_with("xl/charts/chart")).collect();
    names.sort();
    names
        .iter()
        .map(|n| {
            let mut s = String::new();
            z.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        })
        .collect()
}

#[test]
fn trendlines_follow_the_series_schema_sequence() {
    let parts = chart_parts(&write_xlsx(&workbook()).unwrap());
    assert_eq!(parts.len(), KINDS.len());
    for (xml, k) in parts.iter().zip(KINDS) {
        let at = |tag: &str| xml.find(tag).unwrap_or_else(|| panic!("{k:?}: no {tag}"));
        let first = at("<c:trendline>");
        // After spPr / marker / invertIfNegative, before the data references.
        let data = if matches!(k, ChartKind::Scatter | ChartKind::ScatterLines | ChartKind::Bubble) { "<c:xVal>" } else { "<c:cat>" };
        assert!(at("<c:spPr>") < first && first < at(data), "{k:?}");
        if let Some(m) = xml.find("<c:marker>") {
            assert!(m < first, "{k:?}: marker after trendline");
        }
        if let Some(m) = xml.find("<c:invertIfNegative") {
            assert!(m < first, "{k:?}: invertIfNegative after trendline");
        }
        assert_eq!(xml.matches("<c:trendline>").count(), all_trendlines().len(), "{k:?}");
    }
    // Children in CT_Trendline order.
    let xml = &parts[0];
    let lin = &xml[xml.find("<c:trendline>").unwrap()..];
    let lin = &lin[..lin.find("</c:trendline>").unwrap()];
    let pos: Vec<usize> = [
        "<c:trendlineType val=\"linear\"/>",
        "<c:forward val=\"2\"/>",
        "<c:backward val=\"0.5\"/>",
        "<c:dispRSqr val=\"1\"/>",
        "<c:dispEq val=\"1\"/>",
        "<c:trendlineLbl>",
    ]
    .iter()
    .map(|t| lin.find(t).unwrap_or_else(|| panic!("missing {t}")))
    .collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{lin}");
    let exp = &xml[xml.find("<c:name>Growth &amp; decay</c:name>").expect("escaped name")..];
    assert!(exp.find("<c:trendlineType val=\"exp\"/>").unwrap() < exp.find("<c:intercept val=\"1.5\"/>").unwrap());
}

#[test]
fn excel_style_trendlines_are_read() {
    // As Excel writes them: a theme-coloured dashed line, defaults left out, and an unknown
    // type that is skipped rather than failing the chart.
    let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:plotArea><c:lineChart><c:grouping val="standard"/><c:ser><c:idx val="0"/><c:order val="0"/><c:marker><c:symbol val="none"/></c:marker><c:trendline><c:spPr><a:ln w="19050" cap="rnd"><a:solidFill><a:schemeClr val="accent2"/></a:solidFill><a:prstDash val="sysDot"/></a:ln></c:spPr><c:trendlineType val="poly"/><c:order val="3"/><c:dispRSqr val="0"/><c:dispEq/></c:trendline><c:trendline><c:trendlineType val="movingAvg"/><c:period val="1"/></c:trendline><c:trendline><c:trendlineType val="cubicSpline"/></c:trendline><c:val><c:numRef><c:f>Data!$B$2:$B$6</c:f></c:numRef></c:val></c:ser><c:axId val="1"/><c:axId val="2"/></c:lineChart></c:plotArea></c:chart></c:chartSpace>"#;
    let root = crate::xml::parse(xml.as_bytes()).unwrap();
    let c = crate::chart::read_chart(&root).unwrap();
    let t = &c.series[0].trendlines;
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].kind, TrendlineKind::Polynomial(3));
    assert!(t[0].display_equation && !t[0].display_r_squared, "an empty CT_Boolean means true");
    assert_eq!((t[0].width, t[0].dash), (Some(1.5), Some(LineDash::SysDot)));
    assert_eq!(t[0].color, Some(Color::Theme(5, 0)));
    assert_eq!(t[1].kind, TrendlineKind::MovingAverage(2), "period clamps to 2..255");
}

#[test]
fn a_huge_forecast_in_a_file_is_read_clamped() {
    let t = |f: &str, b: &str| {
        format!(
            r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:barChart><c:barDir val="col"/><c:ser><c:idx val="0"/><c:order val="0"/><c:trendline><c:trendlineType val="linear"/><c:forward val="{f}"/><c:backward val="{b}"/></c:trendline><c:val><c:numRef><c:f>Data!$B$2:$B$6</c:f></c:numRef></c:val></c:ser><c:axId val="1"/><c:axId val="2"/></c:barChart></c:plotArea></c:chart></c:chartSpace>"#
        )
    };
    let read = |f: &str, b: &str| {
        let root = crate::xml::parse(t(f, b).as_bytes()).unwrap();
        crate::chart::read_chart(&root).unwrap().series[0].trendlines[0].clone()
    };
    let big = read("1000000", "1e300");
    assert_eq!((big.forward, big.backward), (Trendline::MAX_FORECAST, Trendline::MAX_FORECAST));
    let small = read("2.5", "-3");
    assert_eq!((small.forward, small.backward), (2.5, 0.0), "ordinary values are kept");
}

#[test]
fn trendline_metadata_pairing_skips_unknown_trendline_elements() {
    let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
      <c:chart><c:plotArea><c:barChart><c:barDir val="col"/>
        <c:ser><c:idx val="0"/><c:order val="0"/>
          <c:trendline>
            <c:spPr><a:ln w="12700"><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></a:ln></c:spPr>
            <c:trendlineType val="notAType"/>
            <c:dispEq val="1"/>
          </c:trendline>
          <c:trendline>
            <c:spPr><a:ln w="25400"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:prstDash val="sysDot"/></a:ln></c:spPr>
            <c:trendlineType val="linear"/>
            <c:forward val="2.5"/>
            <c:dispRSqr val="1"/>
          </c:trendline>
          <c:val><c:numRef><c:f>Data!$B$2:$B$6</c:f></c:numRef></c:val>
        </c:ser>
        <c:axId val="1"/><c:axId val="2"/>
      </c:barChart></c:plotArea></c:chart>
    </c:chartSpace>"#;
    let root = crate::xml::parse(xml.as_bytes()).unwrap();
    let t = &crate::chart::read_chart(&root).unwrap().series[0].trendlines;
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].kind, TrendlineKind::Linear);
    assert_eq!(t[0].forward, 2.5);
    assert!(!t[0].display_equation);
    assert!(t[0].display_r_squared);
    assert_eq!(t[0].color, Some(Color::Rgb(0xFF0000)));
    assert_eq!(t[0].width, Some(2.0));
    assert_eq!(t[0].dash, Some(LineDash::SysDot));
}
