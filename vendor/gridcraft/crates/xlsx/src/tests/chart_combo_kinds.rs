//! Importing a combination chart keeps each series' own group kind, the first group's included.

use gridcraft_model::ChartKind;

use crate::chart::read_chart;
use crate::xml::parse;

fn ser(i: usize) -> String {
    format!("<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/><c:val><c:numRef><c:f>Data!$A$1:$A$5</c:f></c:numRef></c:val></c:ser>")
}

const AX: &str = "<c:axId val=\"1\"/><c:axId val=\"2\"/>";

fn bar(sers: &str) -> String {
    format!("<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>{sers}{AX}</c:barChart>")
}

fn line(sers: &str) -> String {
    format!("<c:lineChart><c:grouping val=\"standard\"/>{sers}{AX}</c:lineChart>")
}

fn kinds(first: &str, second: &str) -> Vec<Option<ChartKind>> {
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">\
         <c:chart><c:plotArea>{first}{second}</c:plotArea></c:chart></c:chartSpace>"
    );
    let root = parse(xml.as_bytes()).unwrap();
    let ch = read_chart(&root).unwrap();
    assert_eq!(ch.kind, ChartKind::Combo);
    ch.series.iter().map(|s| s.kind).collect()
}

#[test]
fn first_group_with_two_columns_stays_columns() {
    let k = kinds(&bar(&(ser(0) + &ser(1))), &line(&ser(2)));
    assert_eq!(k[0], Some(ChartKind::ColumnClustered));
    assert_eq!(k[1], Some(ChartKind::ColumnClustered));
    assert!(matches!(k[2], Some(ChartKind::Line | ChartKind::LineMarkers)));
}

#[test]
fn line_first_group_stays_a_line() {
    let k = kinds(&line(&ser(0)), &bar(&ser(1)));
    assert!(matches!(k[0], Some(ChartKind::Line | ChartKind::LineMarkers)));
    assert_eq!(k[1], Some(ChartKind::ColumnClustered));
}

/// Outside a combo, a series keeps its own markers or lines where they differ from its chart's,
/// read and written. The chart's kind is its first series'.
#[test]
fn series_keep_their_own_markers_and_lines() {
    use gridcraft_model::{Chart, Series, Workbook};
    let no_marker = "<c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:marker><c:symbol val=\"none\"/></c:marker><c:val><c:numRef><c:f>Data!$A$1:$A$5</c:f></c:numRef></c:val></c:ser>";
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea>{}</c:plotArea></c:chart></c:chartSpace>",
        line(&(no_marker.to_string() + &ser(1)))
    );
    let ch = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    assert_eq!((ch.kind, ch.series.iter().map(|s| s.kind).collect::<Vec<_>>()), (ChartKind::Line, vec![None, Some(ChartKind::LineMarkers)]));
    let series = |kind| Series { values: "Sheet1!$A$1:$A$3".into(), kind, ..Series::default() };
    for (kind, kinds) in
        [(ChartKind::Line, vec![None, Some(ChartKind::LineMarkers)]), (ChartKind::ScatterLines, vec![Some(ChartKind::Scatter), None])]
    {
        let chart = Chart { kind, series: kinds.iter().map(|k| series(*k)).collect(), ..ch.clone() };
        let written = crate::chart::write_chart(&Workbook::new(), 0, &chart, "Book1");
        let back = read_chart(&parse(written.as_bytes()).unwrap()).unwrap();
        // XLSX has no chart-wide kind besides its series': each series is drawn as it was.
        let drawn = |c: &Chart| (0..c.series.len()).map(|i| c.series_kind(i)).collect::<Vec<_>>();
        assert_eq!(drawn(&back), drawn(&chart), "{kind:?}");
    }
}

/// A combo chart is written with one group per kind its series are drawn as, and read back in
/// the series' own order.
#[test]
fn combo_series_keep_their_kinds_and_order() {
    use gridcraft_model::{Chart, Series, Workbook};
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea>{}{}</c:plotArea></c:chart></c:chartSpace>",
        bar(&ser(0)),
        line(&ser(1))
    );
    let base = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    for kinds in [
        vec![ChartKind::ColumnClustered, ChartKind::Area],
        vec![ChartKind::ColumnStacked, ChartKind::LineMarkers],
        vec![ChartKind::LineMarkers, ChartKind::ColumnClustered],
    ] {
        let series: Vec<Series> = kinds
            .iter()
            .enumerate()
            .map(|(i, k)| Series { values: format!("Sheet1!${}$1:${}$3", ["B", "C"][i], ["B", "C"][i]), kind: Some(*k), ..Series::default() })
            .collect();
        let chart = Chart { kind: ChartKind::Combo, series, ..base.clone() };
        let back = read_chart(&parse(crate::chart::write_chart(&Workbook::new(), 0, &chart, "Book1").as_bytes()).unwrap()).unwrap();
        let drawn = |c: &Chart| (0..c.series.len()).map(|i| (c.series_kind(i), c.series[i].values.clone())).collect::<Vec<_>>();
        assert_eq!(drawn(&back), drawn(&chart), "{kinds:?}");
    }
    // Groups that can't share one pair of axes are drawn in the first one's kind: a pie beside
    // columns as columns, its axes all present. (A combination's bars are drawn as columns.)
    {
        let (kinds, want) = (vec![ChartKind::Pie, ChartKind::ColumnClustered], ChartKind::ColumnClustered);
        let series: Vec<Series> = kinds.iter().map(|k| Series { values: "Sheet1!$B$1:$B$3".into(), kind: Some(*k), ..Series::default() }).collect();
        let chart = Chart { kind: ChartKind::Combo, series, ..base.clone() };
        let xml = crate::chart::write_chart(&Workbook::new(), 0, &chart, "Book1");
        let back = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
        assert!((0..2).all(|i| back.series_kind(i) == want), "{kinds:?}: {xml}");
        assert!(back.series.iter().all(|s| !s.secondary), "{kinds:?}");
        assert!(xml.contains("<c:valAx>"), "{kinds:?}: {xml}");
    }
}

/// A chart of one kind with a series on the secondary axis keeps it, and reads back as that
/// kind, not a combination.
#[test]
fn one_kind_on_two_axes_stays_one_kind() {
    use gridcraft_model::{Chart, Series, Workbook};
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea>{}</c:plotArea></c:chart></c:chartSpace>",
        bar(&ser(0))
    );
    let base = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    let series = vec![
        Series { values: "Sheet1!$B$1:$B$3".into(), ..Series::default() },
        Series { values: "Sheet1!$C$1:$C$3".into(), secondary: true, ..Series::default() },
    ];
    let chart = Chart { kind: ChartKind::ColumnClustered, series, ..base };
    let back = read_chart(&parse(crate::chart::write_chart(&Workbook::new(), 0, &chart, "Book1").as_bytes()).unwrap()).unwrap();
    assert_eq!(back.kind, ChartKind::ColumnClustered);
    assert_eq!(back.series.iter().map(|s| (s.kind, s.secondary)).collect::<Vec<_>>(), vec![(None, false), (None, true)]);
}

/// A stock chart with volume: the stock group of 4 series stays a stock group beside the
/// volume columns.
#[test]
fn stock_with_volume_keeps_its_stock_group() {
    use gridcraft_model::{Chart, Series, Workbook};
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea>{}</c:plotArea></c:chart></c:chartSpace>",
        bar(&ser(0))
    );
    let base = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    let mut series = vec![Series { values: "Sheet1!$B$1:$B$3".into(), kind: Some(ChartKind::ColumnClustered), ..Series::default() }];
    series.extend((0..4).map(|_| Series { values: "Sheet1!$C$1:$C$3".into(), kind: Some(ChartKind::Stock), secondary: true, ..Series::default() }));
    let chart = Chart { kind: ChartKind::Combo, series, ..base };
    let written = crate::chart::write_chart(&Workbook::new(), 0, &chart, "Book1");
    assert!(written.contains("<c:stockChart>"), "{written}");
    let back = read_chart(&parse(written.as_bytes()).unwrap()).unwrap();
    assert_eq!((1..5).map(|i| back.series_kind(i)).collect::<Vec<_>>(), vec![ChartKind::Stock; 4]);
}

/// A stock series too few for a stock group is saved as a line in the line group already on its
/// axes: the stacked lines after it stay stacked.
#[test]
fn a_lone_stock_series_does_not_unstack_the_lines_beside_it() {
    use ChartKind::*;
    let chart = chart_of(Combo, &[(Some(Stock), false), (Some(LineStacked), false), (Some(LineStacked), false)]);
    let (xml, back) = round_trip(&chart);
    assert_eq!(xml.matches("<c:lineChart>").count(), 1, "{xml}");
    assert!(xml.contains("<c:grouping val=\"stacked\"/>"), "{xml}");
    assert_eq!((back.series_kind(1), back.series_kind(2)), (LineStacked, LineStacked), "{xml}");
}

/// The primary axes are the pair whose value axis is on the left, whichever group the file
/// lists first.
#[test]
fn a_secondary_scatter_listed_first_stays_secondary() {
    let sc = |s: &str, a: u32, b: u32| {
        format!("<c:scatterChart><c:scatterStyle val=\"lineMarker\"/>{s}<c:axId val=\"{a}\"/><c:axId val=\"{b}\"/></c:scatterChart>")
    };
    let ax =
        |id: u32, cross: u32, pos: &str| format!("<c:valAx><c:axId val=\"{id}\"/><c:axPos val=\"{pos}\"/><c:crossAx val=\"{cross}\"/></c:valAx>");
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea>{}{}{}{}{}{}</c:plotArea></c:chart></c:chartSpace>",
        sc(&ser(0), 30, 40),
        sc(&ser(1), 10, 20),
        ax(10, 20, "b"),
        ax(20, 10, "l"),
        ax(30, 40, "b"),
        ax(40, 30, "r")
    );
    let ch = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    assert_eq!(ch.series.iter().map(|s| s.secondary).collect::<Vec<_>>(), vec![true, false]);
}

/// A chart of `kind` whose series have `kinds` (and secondary flags).
fn chart_of(kind: ChartKind, kinds: &[(Option<ChartKind>, bool)]) -> gridcraft_model::Chart {
    use gridcraft_model::{Chart, Series};
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea>{}</c:plotArea></c:chart></c:chartSpace>",
        bar(&ser(0))
    );
    let base = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    let series = kinds
        .iter()
        .enumerate()
        .map(|(i, (k, secondary))| Series { values: format!("Sheet1!$B${0}:$B${0}", i + 1), kind: *k, secondary: *secondary, ..Series::default() })
        .collect();
    Chart { kind, series, ..base }
}

/// The chart written to XLSX, and read back.
fn round_trip(chart: &gridcraft_model::Chart) -> (String, gridcraft_model::Chart) {
    let xml = crate::chart::write_chart(&gridcraft_model::Workbook::new(), 0, chart, "Book1");
    let back = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    (xml, back)
}

/// What a chart draws: each series' values, kind and axis.
fn drawn(c: &gridcraft_model::Chart) -> Vec<(String, ChartKind, bool)> {
    let any_primary = c.series.iter().any(|s| !s.secondary);
    (0..c.series.len()).map(|i| c.series_kind(i)).zip(&c.series).map(|(k, s)| (s.values.clone(), k, s.secondary && any_primary)).collect()
}

/// A chart written to XLSX and read back draws the same series, kinds and axes.
fn assert_round_trips(kind: ChartKind, kinds: &[(Option<ChartKind>, bool)]) -> String {
    let chart = chart_of(kind, kinds);
    let (xml, back) = round_trip(&chart);
    assert_eq!(drawn(&back), drawn(&chart), "{kinds:?}: {xml}");
    xml
}

/// Pies and doughnuts have no axes: a series marked secondary (left from a combo before the
/// type changed) stays in the one group, so Excel draws one pie, not two on top of each other.
#[test]
fn pies_and_doughnuts_are_one_group_whatever_the_secondary_flags() {
    for (kind, tag) in [(ChartKind::Pie, "<c:pieChart>"), (ChartKind::Doughnut, "<c:doughnutChart>")] {
        let (xml, back) = round_trip(&chart_of(kind, &[(None, false), (None, true)]));
        assert_eq!(xml.matches(tag).count(), 1, "{xml}");
        assert!(!xml.contains("<c:valAx>"), "{xml}");
        assert_eq!((back.kind, back.series.len()), (kind, 2));
    }
}

/// A combination chart's pies and doughnuts are drawn as columns, on axes: on the secondary
/// axis beside other secondary series, with primary axes always written, and a combo of only
/// pies and doughnuts is one group of columns.
#[test]
fn combo_pies_are_columns_on_their_own_axes() {
    use ChartKind::*;
    let xml = assert_round_trips(Combo, &[(Some(Pie), true), (Some(ColumnClustered), true)]);
    assert!(xml.contains("<c:axId val=\"1\"/>") && !xml.contains("<c:axId val=\"3\"/>"), "{xml}");
    let xml = assert_round_trips(Combo, &[(Some(ColumnClustered), false), (Some(Line), true), (Some(Pie), true)]);
    assert_eq!(xml.matches("<c:barChart>").count(), 2, "{xml}");
    let xml = assert_round_trips(Combo, &[(Some(Pie), false), (Some(Doughnut), false)]);
    assert!(!xml.contains("<c:pieChart>") && !xml.contains("<c:doughnutChart>"), "{xml}");
    assert_eq!(xml.matches("<c:barChart>").count(), 1, "{xml}");
}

/// Excel keeps one group per type on a pair of axes: clustered and stacked columns (lines and
/// stacked lines, areas and stacked areas) on the same axes are written as one group, drawn
/// alike before saving and after reopening.
#[test]
fn one_group_per_type_on_a_pair_of_axes() {
    use ChartKind::*;
    for (kinds, tag) in [
        (vec![(Some(ColumnStacked), false), (Some(Pie), false), (Some(Line), false)], "<c:barChart>"),
        (vec![(Some(Pie), false), (Some(ColumnStacked), false), (Some(Line), false)], "<c:barChart>"),
        (vec![(Some(ColumnClustered), false), (Some(ColumnStacked), false)], "<c:barChart>"),
        (vec![(Some(ColumnStacked), false), (Some(ColumnClustered), false), (Some(Line), false)], "<c:barChart>"),
        (vec![(Some(Line), false), (Some(LineStacked), false), (Some(ColumnClustered), false)], "<c:lineChart>"),
        (vec![(Some(LineStacked), false), (Some(LineMarkers), false), (Some(ColumnClustered), false)], "<c:lineChart>"),
        (vec![(Some(Area), false), (Some(AreaStacked), false), (Some(ColumnClustered), false)], "<c:areaChart>"),
    ] {
        let xml = assert_round_trips(Combo, &kinds);
        assert_eq!(xml.matches(tag).count(), 1, "{kinds:?}: {xml}");
    }
    // A stacked column after a clustered one is drawn clustered, as it is saved.
    let chart = chart_of(Combo, &[(Some(ColumnClustered), false), (Some(ColumnStacked), false)]);
    assert_eq!((chart.series_kind(0), chart.series_kind(1)), (ColumnClustered, ColumnClustered));
    // On separate axes each keeps its own grouping.
    let xml = assert_round_trips(Combo, &[(Some(ColumnClustered), false), (Some(ColumnStacked), true), (Some(Line), false)]);
    assert_eq!(xml.matches("<c:barChart>").count(), 2, "{xml}");
}

/// A merged group writes its series in their own order: a bubble series drawn as a column joins
/// the columns where it stands.
#[test]
fn a_merged_group_keeps_its_series_in_order() {
    use ChartKind::*;
    let (xml, back) = round_trip(&chart_of(Combo, &[(Some(ColumnClustered), false), (Some(Bubble), false), (Some(ColumnClustered), false)]));
    assert_eq!(back.series.iter().map(|s| s.values.as_str()).collect::<Vec<_>>(), vec!["Sheet1!$B$1:$B$1", "Sheet1!$B$2:$B$2", "Sheet1!$B$3:$B$3"]);
    let orders: Vec<&str> = xml.match_indices("<c:order val=\"").map(|(i, m)| &xml[i + m.len()..i + m.len() + 1]).collect();
    assert_eq!(orders, vec!["0", "1", "2"], "{xml}");
}

/// A markers-only scatter series drawn among lines (a combo on category axes) keeps its
/// markers.
#[test]
fn a_scatter_series_among_lines_keeps_its_markers() {
    use ChartKind::*;
    let (xml, back) = round_trip(&chart_of(Combo, &[(Some(ColumnClustered), false), (Some(Scatter), false)]));
    assert_eq!(back.series_kind(1), LineMarkers, "{xml}");
}

/// A combination chart is drawn on category axes, whichever series comes first: a scatter or
/// bubble series never turns the axes into value axes and the other series into scatter series.
#[test]
fn a_combo_stays_on_category_axes() {
    use ChartKind::*;
    for kinds in [
        vec![(Some(Scatter), false), (Some(ColumnClustered), false)],
        vec![(Some(ScatterLines), false), (Some(Line), false), (Some(ColumnStacked), false)],
        vec![(Some(Bubble), false), (Some(Line), false)],
        vec![(Some(Line), false), (Some(Scatter), true), (Some(ColumnClustered), true)],
        vec![(Some(Radar), false), (Some(ColumnClustered), false)],
    ] {
        let xml = assert_round_trips(Combo, &kinds);
        assert!(!xml.contains("<c:scatterChart>") && !xml.contains("<c:bubbleChart>"), "{kinds:?}: {xml}");
        assert!(xml.contains("<c:catAx>"), "{kinds:?}: {xml}");
    }
}

/// A combination chart's chartex series (histogram, waterfall, funnel, treemap, sunburst, box &
/// whisker) are drawn and saved as clustered columns, sharing the first column's grouping.
#[test]
fn combo_chartex_series_are_columns() {
    use ChartKind::*;
    for k in [Histogram, Waterfall, Funnel, Treemap, Sunburst, BoxWhisker] {
        let xml = assert_round_trips(Combo, &[(Some(ColumnStacked), false), (Some(k), false), (None, false)]);
        assert_eq!(xml.matches("<c:barChart>").count(), 1, "{k:?}: {xml}");
        assert_round_trips(Combo, &[(Some(k), false), (Some(ColumnStacked), false), (Some(Line), false)]);
        let chart = chart_of(Combo, &[(Some(Bubble), false), (Some(k), false)]);
        assert_eq!(round_trip(&chart).1.supports_data_table(), chart.supports_data_table(), "{k:?}");
    }
}

/// A chart without series keeps a group of its kind, so it is read back as that kind rather
/// than dropped (an empty combo as clustered columns, an empty stock chart as a line chart: a
/// stock group must hold series).
#[test]
fn a_chart_without_series_survives_a_round_trip() {
    use ChartKind::*;
    let kinds = [
        ColumnClustered,
        ColumnStacked100,
        BarStacked,
        Line,
        LineMarkers,
        LineStacked,
        Pie,
        Doughnut,
        Area,
        AreaStacked,
        Scatter,
        ScatterLines,
        Bubble,
        Radar,
        Stock,
        Combo,
    ];
    for kind in kinds {
        let (xml, back) = round_trip(&chart_of(kind, &[]));
        let want = match kind {
            Combo => ColumnClustered,
            Stock => Line,
            k => k,
        };
        assert_eq!((back.kind, back.series.len()), (want, 0), "{kind:?}: {xml}");
    }
}

/// The primary axes are found in one pass over the value axes, however many groups a file has:
/// eight times the groups is about eight times the work, not sixty-four.
#[test]
fn many_groups_are_read_in_linear_time() {
    // Every group has a series and its own value axis on the right, so no group is the primary
    // pair and every group's value axis is looked up among all the others.
    let visits = |n: usize| {
        let groups: String = (0..n)
            .map(|i| {
                format!(
                    "<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>{}<c:axId val=\"{}\"/><c:axId val=\"{}\"/></c:barChart>",
                    ser(i),
                    2 * i + 1,
                    2 * i + 2
                )
            })
            .collect();
        let axes: String = (0..n).map(|i| format!("<c:valAx><c:axId val=\"{}\"/><c:axPos val=\"r\"/></c:valAx>", 2 * i + 2)).collect();
        let xml = format!(
            "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart><c:plotArea>{groups}{axes}</c:plotArea></c:chart></c:chartSpace>"
        );
        let doc = parse(xml.as_bytes()).unwrap();
        crate::chart::VAL_AXIS_VISITS.with(|v| v.set(0));
        let ch = read_chart(&doc).unwrap();
        assert_eq!(ch.series.len(), n);
        crate::chart::VAL_AXIS_VISITS.with(|v| v.get())
    };
    let (small, large) = (visits(300), visits(2_400));
    assert!(small >= 300, "every value axis is looked at: {small}");
    assert!(large < 16 * small, "eight times the groups took {large} visits against {small}");
}

/// An empty group of another kind before the groups with series (as another writer may leave)
/// sets neither the chart's axes nor which series are secondary.
#[test]
fn an_empty_group_sets_neither_axes_nor_secondary() {
    let ax = |tag: &str, id: u32, cross: u32, pos: &str, title: &str| {
        format!(
            "<c:{tag}><c:axId val=\"{id}\"/><c:axPos val=\"{pos}\"/><c:title><c:tx><c:rich><a:p><a:r><a:t>{title}</a:t></a:r></a:p></c:rich></c:tx></c:title><c:crossAx val=\"{cross}\"/></c:{tag}>"
        )
    };
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><c:chart><c:plotArea>\
         <c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>{AX}</c:barChart>\
         <c:scatterChart><c:scatterStyle val=\"lineMarker\"/>{}<c:axId val=\"3\"/><c:axId val=\"4\"/></c:scatterChart>\
         {}{}{}{}</c:plotArea></c:chart></c:chartSpace>",
        ser(0),
        ax("catAx", 1, 2, "b", "BARCAT"),
        ax("valAx", 2, 1, "l", "BARVAL"),
        ax("valAx", 3, 4, "b", "SCX"),
        ax("valAx", 4, 3, "l", "SCY"),
    );
    let ch = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    assert_eq!(ch.kind, ChartKind::ScatterLines);
    assert!(!ch.series[0].secondary);
    assert_eq!((ch.x_title.as_deref(), ch.y_title.as_deref()), (Some("SCX"), Some("SCY")));
}

/// An axis element (`tag` with id, crossing axis, side, title, and whether it is deleted).
fn axis(tag: &str, id: u32, cross: u32, pos: &str, title: Option<&str>, deleted: bool) -> String {
    let title = title.map_or(String::new(), |t| format!("<c:title><c:tx><c:rich><a:p><a:r><a:t>{t}</a:t></a:r></a:p></c:rich></c:tx></c:title>"));
    format!(
        "<c:{tag}><c:axId val=\"{id}\"/><c:delete val=\"{}\"/><c:axPos val=\"{pos}\"/>{title}<c:crossAx val=\"{cross}\"/></c:{tag}>",
        u8::from(deleted)
    )
}

fn titles_of(plot: &str) -> (Option<String>, Option<String>) {
    let xml = format!(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><c:chart><c:plotArea>{plot}</c:plotArea></c:chart></c:chartSpace>"
    );
    let ch = read_chart(&parse(xml.as_bytes()).unwrap()).unwrap();
    (ch.x_title, ch.y_title)
}

/// Axis titles come from shown axes before deleted ones, and the primary pair's group says
/// whether they are a scatter's two value axes.
#[test]
fn titles_come_from_shown_axes_of_the_primary_pair() {
    let some = |x: &str, y: &str| (Some(x.to_string()), Some(y.to_string()));
    // Series only on axes 3/4 (whose category axis is deleted), beside an empty group on 1/2.
    let plot = format!(
        "<c:barChart><c:barDir val=\"col\"/><c:grouping val=\"clustered\"/>{AX}</c:barChart>\
         <c:lineChart><c:grouping val=\"standard\"/>{}<c:axId val=\"3\"/><c:axId val=\"4\"/></c:lineChart>{}{}{}{}",
        ser(0),
        axis("catAx", 1, 2, "b", Some("PCAT"), false),
        axis("valAx", 2, 1, "l", Some("PVAL"), false),
        axis("catAx", 3, 4, "b", None, true),
        axis("valAx", 4, 3, "r", Some("SVAL"), false),
    );
    assert_eq!(titles_of(&plot), some("PCAT", "SVAL"));
    // A secondary line group before the primary scatter group.
    let plot = format!(
        "<c:lineChart><c:grouping val=\"standard\"/>{}<c:axId val=\"3\"/><c:axId val=\"4\"/></c:lineChart>\
         <c:scatterChart><c:scatterStyle val=\"lineMarker\"/>{}{AX}</c:scatterChart>{}{}{}{}",
        ser(0),
        ser(1),
        axis("catAx", 3, 4, "b", Some("SCAT"), true),
        axis("valAx", 4, 3, "r", Some("SVAL"), false),
        axis("valAx", 1, 2, "b", Some("X"), false),
        axis("valAx", 2, 1, "l", Some("Y"), false),
    );
    assert_eq!(titles_of(&plot), some("X", "Y"));
}
