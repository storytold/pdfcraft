//! Owned corpus slice: combo charts (labels and trendlines), merged tables with diagonal
//! borders, and text columns, autofit, vertical text and a WordArt warp.
//!
//! A combo series can sit on the secondary value axis (`secondary_axis`, `y2_*`). That axis is
//! read from files; rebuilding a chart does not write it yet, so this test does not require it
//! to survive.

use std::sync::Arc;

use deckcraft_color::Rgba;
use deckcraft_geom::Xfrm;
use deckcraft_model::chart::{Chart, ChartType, Series, Trendline, TrendlineType};
use deckcraft_model::style::{ColorRef, Line};
use deckcraft_model::table::Table;
use deckcraft_model::text::{AutoFit, TextBody, TextDir};
use deckcraft_model::{Presentation, Shape, ShapeId, ShapeKind, Slide, defaults};

fn shape(p: &mut Presentation, name: &str, x: f64, y: f64, w: f64, h: f64) -> Shape {
    Shape { id: ShapeId(p.alloc_id().expect("id")), name: name.into(), xfrm: Some(Xfrm::new(x, y, w, h)), ..Default::default() }
}

fn round(p: &Presentation) -> Presentation {
    let bytes = deckcraft_pptx::export(p).expect("export");
    deckcraft_pptx::import(&bytes).expect("import")
}

fn by_name<'a>(s: &'a Slide, name: &str) -> &'a Shape {
    s.shapes.iter().find(|x| x.name == name).unwrap_or_else(|| panic!("no shape {name}"))
}

fn plain(t: &TextBody) -> String {
    t.paragraphs.iter().map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn trend(kind: TrendlineType) -> Trendline {
    Trendline { kind, ..Default::default() }
}

fn corpus() -> Presentation {
    let mut p = defaults::new_presentation(None);
    p.props.title = "Chart and table corpus".into();
    let mut s = {
        let layout = defaults::layout_of_kind(&p, deckcraft_model::LayoutType::Blank).unwrap_or_default();
        defaults::new_slide(&mut p, layout).expect("slide")
    };

    let chart = Chart {
        kind: ChartType::Combo,
        title: Some("Spend and clicks".into()),
        categories: vec!["W1".into(), "W2".into(), "W3".into(), "W4".into()],
        data_labels: true,
        y2_max: Some(8000.0),
        y2_format: Some("0.0".into()),
        series: vec![
            Series {
                name: "Spend".into(),
                values: vec![Some(200.0), Some(310.0), Some(280.0), Some(360.0)],
                kind: Some(ChartType::Column),
                trendlines: vec![
                    Trendline { kind: TrendlineType::Poly, name: Some("Cubic".into()), order: Some(3), ..Default::default() },
                    trend(TrendlineType::Exp),
                ],
                ..Default::default()
            },
            Series {
                name: "Clicks".into(),
                values: vec![Some(6000.0), Some(7400.0), Some(7100.0), Some(7900.0)],
                kind: Some(ChartType::LineMarkers),
                secondary_axis: true,
                data_labels: Some(false),
                trendlines: vec![
                    Trendline {
                        kind: TrendlineType::Linear,
                        name: Some("Fit & forecast".into()),
                        forward: Some(1.0),
                        backward: Some(0.5),
                        intercept: Some(0.0),
                        disp_eq: true,
                        disp_r_sqr: true,
                        ..Default::default()
                    },
                    trend(TrendlineType::Log),
                    trend(TrendlineType::Power),
                    Trendline { kind: TrendlineType::MovingAvg, period: Some(2), ..Default::default() },
                ],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut ch = shape(&mut p, "Combo", 40.0, 40.0, 420.0, 260.0);
    ch.kind = ShapeKind::Chart(Box::new(chart));
    s.shapes.push(ch);

    let mut table = Table::new(3, 3, 360.0, 28.0);
    if let Some(cell) = table.cell_mut(0, 0) {
        cell.text = TextBody::from_text("Merged");
    }
    assert!(table.merge(0, 0, 1, 1));
    if let Some(cell) = table.cell_mut(0, 2) {
        cell.text = TextBody::from_text("Diag");
        cell.diag_down = Some(Line::solid(ColorRef::rgb(Rgba::rgb(180, 40, 40)), 1.5));
        cell.diag_up = Some(Line::solid(ColorRef::rgb(Rgba::rgb(20, 90, 160)), 2.0));
    }
    let mut ts = shape(&mut p, "Merged table", 40.0, 320.0, 360.0, 84.0);
    ts.kind = ShapeKind::Table(table);
    s.shapes.push(ts);

    let mut columns = TextBody::from_text("Two columns\nof vertical text");
    columns.body.columns = Some(3);
    columns.body.col_spacing = Some(18.0);
    columns.body.autofit = Some(AutoFit::Shrink { font_scale: Some(0.85), line_reduction: 0.2 });
    columns.body.vert = Some(TextDir::Vertical270);
    let mut col = shape(&mut p, "Columns", 480.0, 40.0, 200.0, 160.0);
    col.text = Some(columns);
    s.shapes.push(col);

    let mut warp = TextBody::from_text("Arch");
    warp.body.warp = Some("textArchUp".into());
    warp.body.warp_adj = vec![5_000_000.0];
    warp.body.autofit = Some(AutoFit::Shape);
    let mut word = shape(&mut p, "Warp", 480.0, 220.0, 200.0, 80.0);
    word.text = Some(warp);
    s.shapes.push(word);

    p.slides.push(Arc::new(s));
    p
}

fn chart_of(s: &Slide) -> &Chart {
    let ShapeKind::Chart(c) = &by_name(s, "Combo").kind else { panic!("not a chart") };
    c
}

#[test]
fn chart_table_and_text_corpus_round_trips() {
    let p = corpus();
    let q = round(&p);
    let slide = &q.slides[1];
    let src = chart_of(&p.slides[1]);
    let got = chart_of(slide);
    assert_eq!(got.kind, ChartType::Combo);
    assert_eq!(got.title.as_deref(), Some("Spend and clicks"));
    assert_eq!(got.categories, src.categories);
    assert!(got.data_labels);
    assert_eq!(got.series.len(), 2);
    for (a, b) in src.series.iter().zip(got.series.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.values, b.values);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.data_labels, b.data_labels);
        assert_eq!(a.trendlines, b.trendlines);
    }

    let twice = round(&q);
    let again = chart_of(&twice.slides[1]);
    for (a, b) in got.series.iter().zip(again.series.iter()) {
        assert_eq!(a.trendlines, b.trendlines);
        assert_eq!(a.data_labels, b.data_labels);
    }

    merged_table_survives(slide);
    text_columns_and_warp_survive(slide);
}

fn merged_table_survives(slide: &Slide) {
    let ShapeKind::Table(t) = &by_name(slide, "Merged table").kind else { panic!("not a table") };
    let anchor = t.cell(0, 0).expect("anchor");
    assert_eq!((anchor.grid_span, anchor.row_span), (2, 2));
    assert!(!anchor.h_merge && !anchor.v_merge);
    assert_eq!(plain(&anchor.text), "Merged");
    assert!(t.cell(0, 1).is_some_and(|c| c.h_merge && !c.v_merge));
    assert!(t.cell(1, 0).is_some_and(|c| !c.h_merge && c.v_merge));
    assert!(t.cell(1, 1).is_some_and(|c| c.h_merge && c.v_merge));
    let diag = t.cell(0, 2).expect("diag");
    assert_eq!(diag.diag_down, Some(Line::solid(ColorRef::rgb(Rgba::rgb(180, 40, 40)), 1.5)));
    assert_eq!(diag.diag_up, Some(Line::solid(ColorRef::rgb(Rgba::rgb(20, 90, 160)), 2.0)));
    assert_eq!(plain(&diag.text), "Diag");
}

fn text_columns_and_warp_survive(slide: &Slide) {
    let cols = by_name(slide, "Columns").text.as_ref().expect("columns");
    assert_eq!(plain(cols), "Two columns\nof vertical text");
    assert_eq!(cols.body.columns, Some(3));
    assert_eq!(cols.body.col_spacing, Some(18.0));
    assert_eq!(cols.body.autofit, Some(AutoFit::Shrink { font_scale: Some(0.85), line_reduction: 0.2 }));
    assert_eq!(cols.body.vert, Some(TextDir::Vertical270));

    let warp = by_name(slide, "Warp").text.as_ref().expect("warp");
    assert_eq!(plain(warp), "Arch");
    assert_eq!(warp.body.warp.as_deref(), Some("textArchUp"));
    assert_eq!(warp.body.warp_adj, vec![5_000_000.0]);
    assert_eq!(warp.body.autofit, Some(AutoFit::Shape));
}
