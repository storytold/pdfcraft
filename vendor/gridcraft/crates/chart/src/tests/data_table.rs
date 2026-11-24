use super::*;
use gridcraft_model::ChartDataTable;

#[test]
fn combo_table_keys_use_each_series_index() {
    let mut c = chart(ChartKind::Combo);
    c.legend = LegendPos::None;
    c.data_labels = false;
    c.data_table = Some(ChartDataTable::default());
    let mut d = sample(c.kind);
    // The public renderer also accepts Combo series: the first defaults to columns, later ones to lines.
    for s in &mut d.series {
        s.kind = ChartKind::Combo;
    }
    let p = render(&c, &d, 900.0, 500.0, &ApproxMeasure);
    assert_sane(&p, 900.0, 500.0);
    let header_y = p
        .iter()
        .find_map(|p| match p {
            Prim::Text { text, y, .. } if text == "Q1" => Some(*y),
            _ => None,
        })
        .unwrap();
    for (i, s) in d.series.iter().enumerate() {
        let boxes = p.iter().filter(|p| matches!(p, Prim::Rect { y, fill: Some(f), .. } if *f == s.color && *y > header_y)).count();
        let lines =
            p.iter().filter(|p| matches!(p, Prim::Line { pts, color, .. } if *color == s.color && pts.iter().all(|p| p[1] > header_y))).count();
        assert_eq!(boxes, usize::from(i == 0), "series {i}: column key");
        assert_eq!(lines, usize::from(i > 0), "series {i}: line key");
    }
}

#[test]
fn tables_draw_source_values_for_every_supported_chart_kind() {
    for kind in ALL_KINDS {
        let mut c = chart(kind);
        c.legend = LegendPos::None;
        c.data_labels = false;
        c.data_table = Some(ChartDataTable::default());
        let supported = c.supports_data_table();
        let d = sample(kind);
        let prims = render(&c, &d, 900.0, 500.0, &ApproxMeasure);
        assert_sane(&prims, 900.0, 500.0);
        let labels = texts(&prims);
        // Source amounts, including on 100% stacked charts, not shares or running totals.
        if supported {
            for value in ["$12,000", "-$4,000", "$38,000", "$61,870"] {
                assert!(labels.contains(&value), "{kind:?}: missing {value}");
            }
        } else {
            c.data_table = None;
            assert_eq!(prims, render(&c, &d, 900.0, 500.0, &ApproxMeasure));
        }
    }
}

#[test]
fn table_columns_align_with_plot_and_keep_missing_values_blank() {
    let mut c = chart(ChartKind::ColumnClustered);
    c.title = Some(String::new());
    c.x_title = None;
    c.y_title = None;
    c.legend = LegendPos::None;
    c.data_labels = false;
    c.data_table = Some(ChartDataTable::default());
    let color = [1, 2, 3, 255];
    let mut s = series("Rate", &[0.25, 0.0, 0.75], color, c.kind);
    s.values[1] = None;
    s.number_format = "0.0%".into();
    let d = ChartData { categories: ["Jan", "Feb", "Mar"].map(String::from).to_vec(), series: vec![s], ..Default::default() };
    let p = render(&c, &d, 600.0, 400.0, &ApproxMeasure);
    let at = |text: &str| -> (f32, f32) {
        p.iter()
            .find_map(|p| match p {
                Prim::Text { text: t, x, y, .. } if t == text => Some((*x, *y)),
                _ => None,
            })
            .unwrap()
    };
    let (x1, y1) = at("25.0%");
    let (x3, y3) = at("75.0%");
    let header_y = at("Jan").1;
    assert!(y1 > header_y && (y1 - y3).abs() < 0.01);
    assert_eq!(at("Jan").0, x1);
    assert_eq!(at("Mar").0, x3);
    assert_eq!(texts(&p).iter().filter(|t| **t == "Jan").count(), 1, "the table replaces category-axis labels");
    let centers: Vec<f32> = p
        .iter()
        .filter_map(|p| match p {
            Prim::Rect { x, y, w, fill: Some(f), .. } if *f == color && *y < header_y => Some(x + w / 2.0),
            _ => None,
        })
        .collect();
    assert_eq!(centers.len(), 2);
    assert!((centers[0] - x1).abs() < 0.01 && (centers[1] - x3).abs() < 0.01);
    let cells: Vec<&str> = p
        .iter()
        .filter_map(|p| match p {
            Prim::Text { y, text, .. } if (*y - y1).abs() < 0.01 => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(cells, ["Rate", "25.0%", "75.0%"], "blank cells are not zeroes");
    assert!(
        p.iter().any(|p| matches!(p, Prim::Rect { y, fill: Some(f), .. } if *f == color && *y > header_y)),
        "legend key independent of hidden chart legend"
    );
    c.data_table = Some(ChartDataTable { show_keys: false, ..Default::default() });
    let off = render(&c, &d, 600.0, 400.0, &ApproxMeasure);
    assert!(!off.iter().any(|p| matches!(p, Prim::Rect { y, fill: Some(f), .. } if *f == color && *y > header_y)));
}

#[test]
fn table_borders_are_independent_and_multilevel_headers_are_kept() {
    let mut c = chart(ChartKind::Line);
    c.legend = LegendPos::None;
    c.data_labels = false;
    let mut d = sample(c.kind);
    d.category_levels = vec![vec!["2025".into(), "".into(), "".into(), "2026".into(), "".into(), "".into()]];
    for (h, v, outline, want) in [(false, false, false, 0), (true, false, false, 4), (false, true, false, 6), (false, false, true, 4)] {
        c.data_table = Some(ChartDataTable { horizontal_border: h, vertical_border: v, outline, show_keys: false });
        let p = render(&c, &d, 900.0, 600.0, &ApproxMeasure);
        let year_y = p
            .iter()
            .find_map(|p| match p {
                Prim::Text { text, y, .. } if text == "2025" => Some(*y),
                _ => None,
            })
            .unwrap();
        assert!(texts(&p).contains(&"2026"));
        let lines = p.iter().filter(|p| matches!(p, Prim::Line { pts, .. } if pts.iter().all(|p| p[1] > year_y - 9.0))).count();
        assert_eq!(lines, want, "{h} {v} {outline}");
        assert_sane(&p, 900.0, 600.0);
    }
}

#[test]
fn dense_and_tiny_tables_are_bounded_and_explicit() {
    let mut c = chart(ChartKind::ColumnClustered);
    c.data_table = Some(ChartDataTable::default());
    let mut d = sample(c.kind);
    d.categories = (0..20_000).map(|i| i.to_string()).collect();
    let p = render(&c, &d, 600.0, 400.0, &ApproxMeasure);
    assert!(texts(&p).contains(&"Data table needs more space"));
    assert_sane(&p, 600.0, 400.0);
    d.categories.clear();
    for (w, h) in [(1.0, 1.0), (10.0, 10.0), (40.0, 35.0), (200.0, 100.0)] {
        assert_sane(&render(&c, &d, w, h, &ApproxMeasure), w, h);
    }
}
