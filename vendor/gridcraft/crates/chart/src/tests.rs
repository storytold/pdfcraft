use std::sync::Arc;

use gridcraft_core::{CellRef, Value};
use gridcraft_model::{Anchor, Chart, ChartKind, Color, LegendPos, Series, SparklineKind, Style, Workbook};

use crate::*;

mod budget;
mod data_table;

const ALL_KINDS: [ChartKind; 26] = [
    ChartKind::ColumnClustered,
    ChartKind::ColumnStacked,
    ChartKind::ColumnStacked100,
    ChartKind::BarClustered,
    ChartKind::BarStacked,
    ChartKind::BarStacked100,
    ChartKind::Line,
    ChartKind::LineMarkers,
    ChartKind::LineStacked,
    ChartKind::Pie,
    ChartKind::Doughnut,
    ChartKind::Area,
    ChartKind::AreaStacked,
    ChartKind::AreaStacked100,
    ChartKind::Scatter,
    ChartKind::ScatterLines,
    ChartKind::Bubble,
    ChartKind::Radar,
    ChartKind::Histogram,
    ChartKind::Waterfall,
    ChartKind::Funnel,
    ChartKind::Treemap,
    ChartKind::Sunburst,
    ChartKind::BoxWhisker,
    ChartKind::Stock,
    ChartKind::Combo,
];

fn chart(kind: ChartKind) -> Chart {
    Chart {
        id: 1,
        kind,
        anchor: Anchor::default(),
        title: Some("Quarterly Sales".into()),
        series: vec![],
        legend: LegendPos::Bottom,
        data_labels: true,
        data_table: None,
        gridlines: true,
        style: 0,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: Some("Quarter".into()),
        y_title: Some("Revenue".into()),
        source: None,
        by_rows: false,
        pivot: None,
    }
}

fn series(name: &str, values: &[f64], color: Rgba, kind: ChartKind) -> SeriesData {
    SeriesData {
        name: name.into(),
        values: values.iter().map(|v| Some(*v)).collect(),
        x: Some(values.iter().enumerate().map(|(i, _)| Some(i as f64 * 1.5 + 2.0)).collect()),
        sizes: Some(values.iter().map(|v| Some(v.abs() + 1.0)).collect()),
        color,
        kind,
        secondary: false,
        smooth: false,
        trendlines: Vec::new(),
        number_format: "$#,##0".into(),
    }
}

fn sample(kind: ChartKind) -> ChartData {
    let cats = ["Q1", "Q2", "Q3", "Q4", "Q5", "Q6"];
    let k2 = if kind == ChartKind::Combo { ChartKind::Line } else { kind };
    let k1 = if kind == ChartKind::Combo { ChartKind::ColumnClustered } else { kind };
    let mut b = series("East", &[38000.0, 30000.0, 45000.0, 61870.0, 52000.0, 41000.0], [200, 80, 40, 255], k2);
    b.secondary = kind == ChartKind::Combo;
    ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: cats.iter().map(ToString::to_string).collect(),
        series: vec![
            series("North", &[12000.0, -4000.0, 23000.0, 31000.0, 18000.0, 27000.0], [20, 96, 130, 255], k1),
            b,
            series("South", &[9000.0, 11000.0, 8000.0, 15000.0, 21000.0, 17000.0], [25, 107, 36, 255], k2),
        ],
    }
}

fn coords(p: &Prim) -> Vec<f32> {
    match p {
        Prim::Rect { x, y, w, h, .. } => vec![*x, *y, x + w, y + h],
        Prim::Line { pts, width, .. } => pts.iter().flat_map(|p| [p[0], p[1]]).chain([*width]).collect(),
        Prim::Polygon { pts, .. } => pts.iter().flat_map(|p| [p[0], p[1]]).collect(),
        Prim::Wedge { cx, cy, r_outer, r_inner, a0, a1, .. } => vec![*cx, *cy, *r_outer, *r_inner, *a0, *a1],
        Prim::Circle { cx, cy, r, .. } => vec![*cx, *cy, *r],
        Prim::Text { x, y, size, rotation, .. } => vec![*x, *y, *size, *rotation],
    }
}

fn assert_sane(prims: &[Prim], w: f32, h: f32) {
    let eps = 0.01;
    for p in prims {
        assert!(coords(p).iter().all(|v| v.is_finite()), "non-finite: {p:?}");
        let inside = |x: f32, y: f32| x >= -eps && x <= w + eps && y >= -eps && y <= h + eps;
        match p {
            Prim::Rect { x, y, w: rw, h: rh, .. } => assert!(inside(*x, *y) && inside(x + rw, y + rh), "{p:?}"),
            Prim::Line { pts, .. } | Prim::Polygon { pts, .. } => assert!(pts.iter().all(|q| inside(q[0], q[1])), "{p:?}"),
            Prim::Wedge { cx, cy, r_outer, .. } => assert!(inside(cx - r_outer, cy - r_outer) && inside(cx + r_outer, cy + r_outer), "{p:?}"),
            Prim::Circle { cx, cy, r, .. } => assert!(inside(cx - r, cy - r) && inside(cx + r, cy + r), "{p:?}"),
            Prim::Text { x, y, .. } => assert!(inside(*x, *y), "{p:?}"),
        }
    }
}

fn texts(prims: &[Prim]) -> Vec<&str> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn every_kind_renders_sanely() {
    let m = ApproxMeasure;
    for kind in ALL_KINDS {
        for legend in [LegendPos::Bottom, LegendPos::Top, LegendPos::Left, LegendPos::Right] {
            let mut c = chart(kind);
            c.legend = legend;
            let d = sample(kind);
            let (w, h) = (640.0, 400.0);
            let prims = render(&c, &d, w, h, &m);
            assert!(prims.len() > 3, "{kind:?}: too few prims");
            assert_sane(&prims, w, h);
            let t = texts(&prims);
            assert!(t.contains(&"Quarterly Sales"), "{kind:?}: no title");
            let entries = legend_entries(&c, &d);
            for (name, _, _) in &entries {
                assert!(t.contains(&name.as_str()), "{kind:?} {legend:?}: legend entry {name} missing");
            }
            let n_shapes = prims.iter().filter(|p| !matches!(p, Prim::Text { .. })).count();
            assert!(n_shapes > 2, "{kind:?}: nothing drawn");
        }
    }
}

#[test]
fn doughnut_draws_each_series_as_a_ring() {
    let mut c = chart(ChartKind::Doughnut);
    c.title = None;
    c.legend = LegendPos::None;
    c.data_labels = false;
    c.hole_size = 25;
    c.first_slice_angle = 0;
    let d = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: ["A", "B", "C"].iter().map(|s| s.to_string()).collect(),
        series: vec![
            series("Inner", &[1.0, 2.0, 3.0], [20, 96, 130, 255], ChartKind::Doughnut),
            series("Middle", &[3.0, 2.0, 1.0], [221, 132, 82, 255], ChartKind::Doughnut),
            series("Outer", &[1.0, 1.0, 1.0], [89, 161, 79, 255], ChartKind::Doughnut),
        ],
    };
    let prims = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
    let wedges: Vec<(f32, f32, f32)> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { r_outer, r_inner, a0, .. } => Some((*r_outer, *r_inner, *a0)),
            _ => None,
        })
        .collect();
    assert_eq!(wedges.len(), 9, "three rings of three slices");
    // Excel draws the first series as the innermost ring; the hole is 25% of the radius.
    assert!(wedges[0].0 < wedges[3].0 && wedges[3].0 < wedges[6].0, "{wedges:?}");
    assert!(wedges[0].1 > 0.0, "hole: {wedges:?}");
}

#[test]
fn doughnut_with_many_series_still_draws_the_outer_rings() {
    let mut c = chart(ChartKind::Doughnut);
    c.title = None;
    c.legend = LegendPos::None;
    c.data_labels = false;
    c.hole_size = 75;
    // The annulus is a few dozen pixels tall and there are far more series than pixels, so every
    // ring is thinner than 1 px. The chart used to draw nothing at all.
    let many = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: ["A", "B"].iter().map(|s| s.to_string()).collect(),
        series: (0..255).map(|i| series(&format!("S{i}"), &[1.0, 1.0], [20, 96, 130, 255], ChartKind::Doughnut)).collect(),
    };
    let single = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: ["A", "B"].iter().map(|s| s.to_string()).collect(),
        // Two series, so the title fallback (a lone series names the chart) does not shrink the plot.
        series: vec![
            series("Only", &[1.0, 1.0], [20, 96, 130, 255], ChartKind::Doughnut),
            series("Other", &[1.0, 1.0], [221, 132, 82, 255], ChartKind::Doughnut),
        ],
    };
    let (w, h) = (640.0, 400.0);
    let radii = |d: &ChartData| -> Vec<f32> {
        render(&c, d, w, h, &ApproxMeasure)
            .iter()
            .filter_map(|p| match p {
                Prim::Wedge { r_outer, .. } => Some(*r_outer),
                _ => None,
            })
            .collect()
    };
    let outer = radii(&many);
    assert!(!outer.is_empty(), "a sub-pixel ring must not suppress the whole chart");
    let mut distinct = outer.clone();
    distinct.sort_by(f32::total_cmp);
    distinct.dedup();
    assert!(distinct.len() > 8, "only {} rings drawn", distinct.len());
    assert!(distinct.len() < 255, "too many rings for the pixels available: {}", distinct.len());
    // The outermost ring still reaches the plot radius the single-ring chart uses.
    let r = radii(&single).into_iter().fold(0.0f32, f32::max);
    let drawn = distinct.last().copied().unwrap_or(0.0);
    assert!((drawn - r).abs() < 0.01, "outermost ring {drawn} of {r}");
}

#[test]
fn doughnut_one_pixel_rings_skip_the_separator_stroke() {
    let mut c = chart(ChartKind::Doughnut);
    c.title = None;
    c.legend = LegendPos::None;
    c.data_labels = false;
    c.hole_size = 75;
    // 255 series over a 46-pixel annulus: every drawn ring is one pixel tall, narrower than the
    // 1.5-pixel white separator, which would otherwise paint over both neighbouring rings.
    let thin = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: ["A", "B"].iter().map(|s| s.to_string()).collect(),
        series: (0..255).map(|i| series(&format!("S{i}"), &[1.0, 1.0], [20, 96, 130, 255], ChartKind::Doughnut)).collect(),
    };
    let strokes: Vec<Option<(Rgba, f32)>> = render(&c, &thin, 640.0, 400.0, &ApproxMeasure)
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { stroke, .. } => Some(*stroke),
            _ => None,
        })
        .collect();
    assert!(!strokes.is_empty(), "the chart must still draw its rings");
    assert!(strokes.iter().all(Option::is_none), "a 1.5px stroke covers a 1px band: {strokes:?}");

    // A ring with room for the separator keeps it.
    let thick = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: ["A", "B"].iter().map(|s| s.to_string()).collect(),
        series: vec![
            series("Inner", &[1.0, 1.0], [20, 96, 130, 255], ChartKind::Doughnut),
            series("Outer", &[1.0, 1.0], [221, 132, 82, 255], ChartKind::Doughnut),
        ],
    };
    let strokes: Vec<Option<(Rgba, f32)>> = render(&c, &thick, 640.0, 400.0, &ApproxMeasure)
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { stroke, .. } => Some(*stroke),
            _ => None,
        })
        .collect();
    assert!(!strokes.is_empty());
    assert!(strokes.iter().all(Option::is_some), "wide rings keep their separators: {strokes:?}");
}

#[test]
fn doughnut_wedge_budget_bounds_many_series_and_categories() {
    let mut c = chart(ChartKind::Doughnut);
    c.title = None;
    c.legend = LegendPos::None;
    c.data_labels = false;
    c.hole_size = 75;
    c.first_slice_angle = 0;
    // 20,000 categories across 80 series would be hundreds of thousands of wedges: the per-series
    // limit does not bound the product. The first ten categories of every ring are the largest.
    let cats = 20_000usize;
    let values: Vec<f64> = (0..cats).map(|j| if j < 10 { 1000.0 } else { 1.0 }).collect();
    let d = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: (0..cats).map(|j| format!("C{j}")).collect(),
        series: (0..80)
            .map(|i| SeriesData { name: format!("S{i}"), values: values.iter().map(|v| Some(*v)).collect(), ..SeriesData::default() })
            .collect(),
    };
    let wedges: Vec<(f32, f32)> = render(&c, &d, 640.0, 400.0, &ApproxMeasure)
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { r_outer, a0, a1, .. } => Some((*r_outer, *a1 - *a0)),
            _ => None,
        })
        .collect();
    assert!(!wedges.is_empty(), "the innermost ring must still be drawn");
    let mut radii: Vec<f32> = wedges.iter().map(|(r, _)| *r).collect();
    radii.sort_by(f32::total_cmp);
    radii.dedup();
    assert!(radii.len() > 1, "every drawable ring is capped, not dropped");
    // A slice crossing 12 o'clock is split into two primitives, so each ring may add one.
    let budget = crate::render::MAX_WEDGES + radii.len();
    assert!(wedges.len() <= budget, "{} wedges for {} rings", wedges.len(), radii.len());

    // The first (innermost) ring keeps its largest slices and still fills the circle.
    let inner: Vec<f32> = wedges.iter().filter(|(r, _)| *r == radii[0]).map(|(_, a)| *a).collect();
    assert!(inner.len() < cats, "the ring was not capped: {}", inner.len());
    assert_eq!(inner.iter().filter(|a| **a > 0.1).count(), 10, "largest slices went missing");
    let cover: f32 = inner.iter().sum();
    assert!((cover - std::f32::consts::TAU).abs() < 0.01, "first ring covers {cover}");
}

#[test]
fn doughnut_first_slice_angle_rotates_the_start() {
    let mut c = chart(ChartKind::Doughnut);
    c.title = None;
    c.legend = LegendPos::None;
    c.first_slice_angle = 90;
    let d = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: ["A", "B"].iter().map(|s| s.to_string()).collect(),
        series: vec![series("Only", &[1.0, 1.0], [20, 96, 130, 255], ChartKind::Doughnut)],
    };
    let prims = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
    let first = prims.iter().find_map(|p| match p {
        Prim::Wedge { a0, .. } => Some(*a0),
        _ => None,
    });
    assert!(first.is_some_and(|a| (a - std::f32::consts::FRAC_PI_2).abs() < 0.01), "{first:?}");
}

/// Every slice must occupy exactly its angular share, even when the first slice angle pushes the
/// start past 12 o'clock (the split at TAU used to put whole slices over their neighbours).
#[test]
fn rotated_wedges_cover_each_slice_once_without_overlap() {
    const TAU: f32 = std::f32::consts::TAU;
    for kind in [ChartKind::Doughnut, ChartKind::Pie] {
        for angle in [180u16, 270] {
            let mut c = chart(kind);
            c.title = None;
            c.legend = LegendPos::None;
            c.data_labels = false;
            c.first_slice_angle = angle;
            let d = ChartData {
                category_levels: vec![],
                palette: vec![[230, 126, 34, 255], [39, 174, 96, 255], [142, 68, 173, 255]],
                categories: ["A", "B", "C", "D"].iter().map(|s| s.to_string()).collect(),
                series: vec![series("Only", &[1.0; 4], [20, 96, 130, 255], kind)],
            };
            let prims = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
            let wedges: Vec<(f32, f32)> = prims
                .iter()
                .filter_map(|p| match p {
                    Prim::Wedge { a0, a1, .. } => Some((*a0, *a1)),
                    _ => None,
                })
                .collect();
            assert!(wedges.len() >= 4, "{kind:?} at {angle}: {wedges:?}");
            for (a0, a1) in &wedges {
                assert!((0.0..TAU).contains(a0), "{kind:?} at {angle}: start {a0} outside [0, TAU)");
                assert!(*a1 > *a0 && *a1 <= TAU + 1e-4, "{kind:?} at {angle}: bad sweep {a0}..{a1}");
            }
            // Sample away from the boundaries: a correct four-slice chart covers every angle once.
            let steps = 1440;
            for k in 0..steps {
                let theta = (k as f32 + 0.5) * TAU / steps as f32;
                let cover = wedges.iter().filter(|(a0, a1)| theta >= *a0 && theta < *a1).count();
                assert_eq!(cover, 1, "{kind:?} at {angle}: {theta} covered {cover} times by {wedges:?}");
            }
        }
    }
}

/// A slice that crosses 12 o'clock is filled as two halves but outlined once: stroking each half
/// separately ran the white separator along the shared radial edge, straight through the slice.
#[test]
fn slice_crossing_twelve_has_no_internal_seam() {
    const TAU: f32 = std::f32::consts::TAU;
    let mut c = chart(ChartKind::Pie);
    c.title = None;
    c.legend = LegendPos::None;
    c.data_labels = false;
    c.first_slice_angle = 90;
    let a = [230, 126, 34, 255];
    let b = [39, 174, 96, 255];
    let d = ChartData {
        category_levels: vec![],
        palette: vec![a, b],
        categories: ["A", "B"].iter().map(|s| s.to_string()).collect(),
        series: vec![series("Only", &[1.0, 1.0], a, ChartKind::Pie)],
    };
    let (w, h) = (400u32, 400u32);
    let prims = render(&c, &d, w as f32, h as f32, &ApproxMeasure);
    // The second slice starts at 270° and wraps past 0°, so it is drawn as two unstroked halves.
    let mut halves = 0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut r = 0.0;
    for p in &prims {
        if let Prim::Wedge { cx: x, cy: y, r_outer, a0, a1, fill, stroke, .. } = p
            && *fill == b
        {
            assert!(stroke.is_none(), "a split half must not stroke its shared edge: {p:?}");
            assert!(*a0 == 0.0 || *a1 == TAU, "the crossing slice is split at 12 o'clock: {p:?}");
            halves += 1;
            (cx, cy, r) = (*x, *y, *r_outer);
        }
    }
    assert_eq!(halves, 2, "one slice crosses 12 o'clock");
    // Exactly one outline covers the whole crossing slice, drawn as line primitives.
    assert_eq!(prims.iter().filter(|p| matches!(p, Prim::Line { color, .. } if *color == [255, 255, 255, 255])).count(), 1, "{prims:?}");
    // No white pixel inside the slice along 12 o'clock, where the shared edge used to be stroked.
    let img = rasterize(&prims, w, h, [255, 255, 255, 255]);
    for t in [0.3f32, 0.5, 0.7] {
        assert_eq!(pixel(&img, w, cx as u32, (cy - r * t) as u32), b, "white seam at {t} of the slice");
    }
}

#[test]
fn doughnut_legend_names_categories_and_matches_every_ring() {
    let mut c = chart(ChartKind::Doughnut);
    c.title = None;
    c.data_labels = false;
    c.legend = LegendPos::Right;
    c.first_slice_angle = 180;
    let d = ChartData {
        category_levels: vec![],
        palette: vec![[230, 126, 34, 255], [39, 174, 96, 255], [142, 68, 173, 255]],
        categories: ["A", "B", "C", "D"].iter().map(|s| s.to_string()).collect(),
        series: vec![
            series("Outer", &[1.0; 4], [20, 96, 130, 255], ChartKind::Doughnut),
            series("Inner", &[1.0; 4], [221, 132, 82, 255], ChartKind::Doughnut),
        ],
    };
    let entries = legend_entries(&c, &d);
    assert_eq!(entries.iter().map(|(name, _, _)| name.as_str()).collect::<Vec<_>>(), ["A", "B", "C", "D"]);

    let start = f32::from(c.first_slice_angle) * std::f32::consts::TAU / 360.0;
    let prims = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
    let mut rings = Vec::new();
    for p in &prims {
        let Prim::Wedge { r_outer, a0, a1, fill, .. } = p else { continue };
        rings.push(*r_outer);
        let rel = ((a0 + a1) / 2.0 - start).rem_euclid(std::f32::consts::TAU);
        let cat = ((rel / (std::f32::consts::TAU / 4.0)) as usize).min(3);
        assert_eq!(*fill, entries[cat].1, "ring wedge at {a0}..{a1} does not match legend entry {}", entries[cat].0);
    }
    rings.sort_by(f32::total_cmp);
    rings.dedup();
    assert_eq!(rings.len(), 2, "two series must draw two rings");
}

#[test]
fn legend_counts() {
    let d = sample(ChartKind::ColumnClustered);
    assert_eq!(legend_entries(&chart(ChartKind::ColumnClustered), &d).len(), 3);
    assert_eq!(legend_entries(&chart(ChartKind::Pie), &d).len(), 6);
    assert_eq!(legend_entries(&chart(ChartKind::Waterfall), &d).len(), 3);
    assert_eq!(legend_entries(&chart(ChartKind::Histogram), &d).len(), 0);
    let mut c = chart(ChartKind::Line);
    c.legend = LegendPos::None;
    assert!(legend_entries(&c, &d).is_empty());
}

#[test]
fn nice_axis() {
    let s = nice_scale(0.0, 61870.0, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 70000.0, 10000.0));
    let s = nice_scale(0.0, 100.0, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 120.0, 20.0));
    let s = nice_scale(-4000.0, 31000.0, true);
    assert!(s.min < -4000.0 && s.min >= -10000.0 && s.max >= 31000.0);
    // All zero.
    let s = nice_scale(0.0, 0.0, true);
    assert_eq!((s.min, s.max), (0.0, 1.0));
    // Far from zero: no forced zero.
    let s = nice_scale(50.0, 55.0, false);
    assert!(s.min >= 45.0 && s.max <= 60.0, "{s:?}");
    // Close to zero: zero included.
    assert_eq!(nice_scale(10.0, 90.0, false).min, 0.0);
    // Negative only.
    let s = nice_scale(-80.0, -10.0, false);
    assert_eq!(s.max, 0.0);
    assert!(s.min <= -80.0);
    for (lo, hi) in [(0.0, 1e-300), (-1e308, 1e308), (f64::NAN, 5.0), (0.001, 0.0013), (3.0, 3.0), (-7.0, -7.0), (1e15, 1e15 + 1.0)] {
        let s = nice_scale(lo, hi, false);
        assert!(s.min.is_finite() && s.max.is_finite() && s.step.is_finite() && s.max > s.min && s.step > 0.0, "{lo} {hi} {s:?}");
        let n = s.ticks().len();
        assert!((2..=51).contains(&n));
    }
    // Ticks count in 4..=10 for typical data.
    for hi in [1.0, 7.0, 13.0, 99.0, 250.0, 1234.0, 61870.0, 999999.0] {
        let n = nice_scale(0.0, hi, true).ticks().len();
        assert!((4..=10).contains(&n), "{hi}: {n}");
    }
}

#[test]
fn axis_labels_use_number_format() {
    assert_eq!(format_number(40000.0, "$#,##0", Some(10000.0)), "$40,000");
    assert_eq!(format_number(0.30000000000000004, "General", Some(0.1)), "0.3");
    assert_eq!(format_number(1e-17, "General", Some(0.2)), "0");
    let c = chart(ChartKind::ColumnClustered);
    let prims = render(&c, &sample(ChartKind::ColumnClustered), 640.0, 400.0, &ApproxMeasure);
    let t = texts(&prims);
    assert!(t.contains(&"$70,000"), "{t:?}");
    assert!(t.contains(&"$40,000"));
}

#[test]
fn resolve_from_workbook() {
    let mut wb = Workbook::new();
    let cur = wb.styles.intern(Style { num_fmt: gridcraft_model::NumFmt(Arc::from("$#,##0")), ..Style::default() });
    {
        let s = wb.sheet_mut(0).unwrap();
        s.name = "Sales".into();
        s.set_value(CellRef::new(0, 1), Value::from("Revenue"));
        for (i, (q, v)) in [("Q1", 100.0), ("Q2", 250.0), ("Q3", 175.0)].iter().enumerate() {
            let r = i as u32 + 1;
            s.set_value(CellRef::new(r, 0), Value::from(*q));
            s.set_value(CellRef::new(r, 1), Value::Number(*v));
            s.set_style(CellRef::new(r, 1), cur);
        }
        s.set_value(CellRef::new(2, 2), Value::Number(5.0));
    }
    let mut c = chart(ChartKind::ColumnClustered);
    c.series = vec![
        Series {
            name: Some("Sales!$B$1".into()),
            categories: Some("Sales!$A$2:$A$4".into()),
            values: "Sales!$B$2:$B$4".into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
        },
        Series {
            name: None,
            categories: None,
            values: "=Sales!$C$2:$C$4".into(),
            bubble_sizes: None,
            color: Some(Color::rgb(1, 2, 3)),
            secondary: true,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
        },
        Series {
            name: Some("Literal".into()),
            categories: None,
            values: "{1,2,3,4}".into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
        },
    ];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, vec!["Q1", "Q2", "Q3"]);
    assert_eq!(d.series.len(), 3);
    let s0 = &d.series[0];
    assert_eq!(s0.name, "Revenue");
    assert_eq!(s0.values, vec![Some(100.0), Some(250.0), Some(175.0)]);
    assert_eq!(s0.number_format, "$#,##0");
    assert_eq!(s0.color, series_color(&wb.theme, 0));
    let t = wb.theme.colors[4];
    assert_eq!(s0.color, [(t >> 16) as u8, (t >> 8) as u8, t as u8, 255]);
    let s1 = &d.series[1];
    assert_eq!(s1.name, "Series2");
    assert_eq!(s1.values, vec![None, Some(5.0), None]);
    assert_eq!(s1.color, [1, 2, 3, 255]);
    assert!(s1.secondary);
    assert_eq!(d.series[2].name, "Literal");
    assert_eq!(d.series[2].values.len(), 4);

    // Default categories.
    c.series.remove(0);
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, vec!["1", "2", "3", "4"]);

    // Combo kinds and scatter X values.
    let mut c = chart(ChartKind::Combo);
    c.series = vec![
        Series {
            name: None,
            categories: None,
            values: "Sales!B2:B4".into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
        },
        Series {
            name: None,
            categories: None,
            values: "Sales!B2:B4".into(),
            bubble_sizes: None,
            color: None,
            secondary: true,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
        },
    ];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.series[0].kind, ChartKind::ColumnClustered);
    assert_eq!(d.series[1].kind, ChartKind::Line);
    let mut c = chart(ChartKind::Bubble);
    c.series = vec![Series {
        name: None,
        categories: Some("Sales!B2:B4".into()),
        values: "Sales!B2:B4".into(),
        bubble_sizes: Some("{1,2,3}".into()),
        color: None,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
    }];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.series[0].x.as_ref().map(|x| x.len()), Some(3));
    assert_eq!(d.series[0].sizes, Some(vec![Some(1.0), Some(2.0), Some(3.0)]));

    // Bad formulas never panic.
    let mut c = chart(ChartKind::Line);
    c.series = vec![Series {
        name: Some("=Nope!A1".into()),
        categories: Some("((".into()),
        values: "Missing!A1:A3".into(),
        bubble_sizes: Some("".into()),
        color: None,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
    }];
    let d = resolve(&wb, 7, &c);
    assert_eq!(d.series.len(), 1);
    let _ = render(&c, &d, 300.0, 200.0, &ApproxMeasure);
}

#[test]
fn palette_beyond_six_series() {
    let theme = gridcraft_model::Theme::default();
    let p = default_palette(&theme, 14);
    assert_eq!(p.len(), 14);
    assert_ne!(p[0], p[6]);
    assert_ne!(p[6], p[12]);
}

#[test]
fn scatter_with_text_x_values_numbers_the_points() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    for (i, (label, v)) in [("North", 3.0), ("South", 5.0), ("East", 4.0)].iter().enumerate() {
        s.set_value(CellRef::new(i as u32, 0), Value::from(*label));
        s.set_value(CellRef::new(i as u32, 1), Value::Number(*v));
        s.set_value(CellRef::new(i as u32, 2), Value::Number(i as f64 * 10.0));
    }
    for kind in [ChartKind::Scatter, ChartKind::ScatterLines, ChartKind::Bubble] {
        let mut c = chart(kind);
        c.series = vec![Series { categories: Some("Sheet1!$A$1:$A$3".into()), values: "Sheet1!$B$1:$B$3".into(), ..Default::default() }];
        let d = resolve(&wb, 0, &c);
        assert_eq!(d.series[0].x, None, "{kind:?}");
        let prims = render(&c, &d, 400.0, 300.0, &ApproxMeasure);
        let empty =
            render(&c, &ChartData { category_levels: vec![], palette: vec![], categories: vec![], series: vec![] }, 400.0, 300.0, &ApproxMeasure);
        assert!(prims.len() > empty.len() + 2, "{kind:?} draws its points");
        // Numeric X values are kept.
        c.series[0].categories = Some("Sheet1!$C$1:$C$3".into());
        assert_eq!(resolve(&wb, 0, &c).series[0].x, Some(vec![Some(0.0), Some(10.0), Some(20.0)]));
    }
}

fn pixel(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

#[test]
fn rasterize_bar_center_has_bar_color() {
    let mut c = chart(ChartKind::ColumnClustered);
    c.data_labels = false;
    let d = sample(ChartKind::ColumnClustered);
    let (w, h) = (640u32, 400u32);
    let prims = render(&c, &d, w as f32, h as f32, &ApproxMeasure);
    let img = rasterize(&prims, w, h, [255, 255, 255, 255]);
    assert_eq!(img.len(), (w * h * 4) as usize);
    // Find the bars of the East series (tallest) and check their centres.
    let east = d.series[1].color;
    let bars: Vec<_> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Rect { x, y, w, h, fill: Some(f), .. } if *f == east && *w > 10.0 => Some((*x, *y, *w, *h)),
            _ => None,
        })
        .collect();
    assert_eq!(bars.len(), 6);
    for (x, y, bw, bh) in bars {
        let px = pixel(&img, w, (x + bw / 2.0) as u32, (y + bh / 2.0) as u32);
        assert_eq!(px, east);
    }
    // Primitive-level checks.
    let img = rasterize(&[Prim::Circle { cx: 10.0, cy: 10.0, r: 5.0, fill: Some([255, 0, 0, 255]), stroke: None }], 20, 20, [0, 0, 0, 0]);
    assert_eq!(pixel(&img, 20, 10, 10), [255, 0, 0, 255]);
    assert_eq!(pixel(&img, 20, 0, 0)[3], 0);
    let wedge = Prim::Wedge {
        cx: 50.0,
        cy: 50.0,
        r_outer: 40.0,
        r_inner: 20.0,
        a0: 0.0,
        a1: std::f32::consts::FRAC_PI_2,
        fill: [0, 0, 255, 255],
        stroke: None,
    };
    let img = rasterize(&[wedge], 100, 100, [255, 255, 255, 255]);
    assert_eq!(pixel(&img, 100, 79, 30), [0, 0, 255, 255]); // upper-right quadrant ring
    assert_eq!(pixel(&img, 100, 50, 50), [255, 255, 255, 255]); // hole
    assert_eq!(pixel(&img, 100, 25, 70), [255, 255, 255, 255]); // other quadrant
    assert!(rasterize(&[], 0, 10, [0; 4]).is_empty());
    assert!(rasterize(&[], 100_000, 100_000, [0; 4]).is_empty());
}

#[test]
fn rasterize_does_not_panic_on_negative_rect_extents() {
    let red = [255, 0, 0, 255];
    for (w, h) in [(-10.0, 10.0), (10.0, -10.0), (-10.0, -10.0), (f32::NAN, f32::NAN)] {
        let rect = Prim::Rect { x: 5.0, y: 5.0, w, h, fill: Some(red), stroke: Some((red, 2.0)), radius: 0.0 };
        let img = rasterize(&[rect], 20, 20, [0, 0, 0, 0]);
        assert_eq!(img.len(), 20 * 20 * 4);
    }
}

#[test]
fn pie_starts_at_twelve_clockwise() {
    let c = chart(ChartKind::Pie);
    let d = sample(ChartKind::Pie);
    let prims = render(&c, &d, 400.0, 400.0, &ApproxMeasure);
    let wedges: Vec<_> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { a0, a1, r_inner, stroke, .. } => Some((*a0, *a1, *r_inner, *stroke)),
            _ => None,
        })
        .collect();
    assert_eq!(wedges.len(), 6);
    assert_eq!(wedges[0].0, 0.0);
    assert!(wedges.windows(2).all(|w| w[1].0 >= w[0].1 - 1e-4));
    assert!((wedges[5].1 - std::f32::consts::TAU).abs() < 1e-3);
    assert!(wedges.iter().all(|w| w.2 == 0.0 && w.3.map(|s| s.0) == Some([255; 4])));
    let mut c = chart(ChartKind::Doughnut);
    c.hole_size = 50;
    let mut d = sample(ChartKind::Doughnut);
    d.series.truncate(1);
    let prims = render(&c, &d, 400.0, 400.0, &ApproxMeasure);
    let ok = prims.iter().any(|p| matches!(p, Prim::Wedge { r_outer, r_inner, .. } if (r_inner / r_outer - 0.5).abs() < 1e-3));
    assert!(ok);
}

#[test]
fn negative_values_put_axis_below_zero() {
    let c = chart(ChartKind::ColumnClustered);
    let d = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: vec![],
        series: vec![series("N", &[-5.0, 3.0, -2.0], [1, 2, 3, 255], ChartKind::ColumnClustered)],
    };
    let prims = render(&c, &d, 400.0, 300.0, &ApproxMeasure);
    let t = texts(&prims);
    assert!(t.iter().any(|s| s.starts_with('-') || s.starts_with("($") || s.starts_with("-$")), "{t:?}");
    assert!(t.contains(&"1") && t.contains(&"3"), "default categories: {t:?}");
}

#[test]
fn edge_cases_never_panic() {
    let m = ApproxMeasure;
    let empty = ChartData::default();
    let zeros = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: vec!["a".into(), "b".into()],
        series: vec![series("z", &[0.0, 0.0], [0, 0, 0, 255], ChartKind::Line)],
    };
    let weird = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: vec![String::new(); 3],
        series: vec![SeriesData {
            name: "w".repeat(500),
            values: vec![Some(f64::NAN), Some(f64::INFINITY), None, Some(-1e308), Some(1e308)],
            x: Some(vec![Some(f64::NAN)]),
            sizes: Some(vec![Some(-1.0), Some(0.0)]),
            ..SeriesData::default()
        }],
    };
    for kind in ALL_KINDS {
        let c = chart(kind);
        for d in [&empty, &zeros, &weird] {
            for (w, h) in [(0.0, 0.0), (-5.0, 100.0), (f32::NAN, 10.0), (1.0, 1.0), (20.0, 15.0), (640.0, 400.0), (5000.0, 30.0), (1e6, 1e6)] {
                let p = render(&c, d, w, h, &m);
                if !(w > 0.0 && h > 0.0) || w.is_nan() {
                    assert!(p.is_empty());
                } else {
                    assert_sane(&p, w, h);
                }
            }
        }
    }
}

#[test]
fn thousands_of_points_are_decimated() {
    let n = 50_000;
    let vals: Vec<f64> = (0..n).map(|i| ((i as f64) * 0.01).sin() * 100.0 + i as f64 * 0.001).collect();
    let mut c = chart(ChartKind::Line);
    c.data_labels = true;
    let d = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: vec![],
        series: vec![series("big", &vals, [9, 9, 9, 255], ChartKind::Line)],
    };
    let prims = render(&c, &d, 800.0, 400.0, &ApproxMeasure);
    let max_line = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Line { pts, .. } => Some(pts.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    assert!(max_line > 10 && max_line <= 4 * 2000, "{max_line}");
    assert!(prims.len() < 10_000);
    for kind in [ChartKind::Scatter, ChartKind::Histogram, ChartKind::AreaStacked, ChartKind::BoxWhisker] {
        let p = render(&chart(kind), &d, 800.0, 400.0, &ApproxMeasure);
        assert_sane(&p, 800.0, 400.0);
    }
}

#[test]
fn smooth_line_adds_interpolated_points_and_keeps_endpoints() {
    let m = ApproxMeasure;
    let cats = ["a", "b", "c", "d"].map(String::from).to_vec();
    let line_pts = |smooth: bool, m: &ApproxMeasure| -> usize {
        let mut s = series("L", &[1.0, 5.0, 2.0, 6.0], [10, 20, 30, 255], ChartKind::Line);
        s.smooth = smooth;
        let d = ChartData { category_levels: vec![], palette: vec![], categories: cats.clone(), series: vec![s] };
        render(&chart(ChartKind::Line), &d, 600.0, 400.0, m)
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, .. } => Some(pts.len()),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    };
    let straight = line_pts(false, &m);
    let curved = line_pts(true, &m);
    assert_eq!(straight, 4, "straight polyline keeps one point per datum");
    assert!(curved > straight, "smooth line interpolates extra points: {curved} vs {straight}");
    // Rendering either way must stay finite and inside the canvas.
    let mut s = series("L", &[1.0, 5.0, 2.0, 6.0], [10, 20, 30, 255], ChartKind::Line);
    s.smooth = true;
    let d = ChartData { category_levels: vec![], palette: vec![], categories: cats, series: vec![s] };
    assert_sane(&render(&chart(ChartKind::Line), &d, 600.0, 400.0, &m), 600.0, 400.0);
}

#[test]
fn smooth_line_splits_at_gaps() {
    let m = ApproxMeasure;
    let mut s = series("L", &[1.0, 5.0, 2.0, 6.0], [10, 20, 30, 255], ChartKind::Line);
    s.smooth = true;
    s.values[1] = None; // gap between two data points
    let d = ChartData { category_levels: vec![], palette: vec![], categories: vec!["a".into(), "b".into(), "c".into(), "d".into()], series: vec![s] };
    let prims = render(&chart(ChartKind::Line), &d, 600.0, 400.0, &m);
    // Two separate runs (before/after the gap) → at least two line primitives.
    let lines = prims.iter().filter(|p| matches!(p, Prim::Line { .. })).count();
    assert!(lines >= 2, "a gap must split the smoothed line into runs, got {lines}");
    assert_sane(&prims, 600.0, 400.0);
}

/// Small deterministic PRNG (xorshift) for the fuzz test.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
}

#[test]
fn fuzz_random_data_and_sizes() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let m = ApproxMeasure;
    for iter in 0..400 {
        let kind = ALL_KINDS[(r.next() % ALL_KINDS.len() as u64) as usize];
        let nser = (r.next() % 6) as usize;
        let npts = (r.next() % 40) as usize;
        let mag = 10f64.powi((r.next() % 30) as i32 - 10);
        let val = |r: &mut Rng| -> Option<f64> {
            match r.next() % 12 {
                0 => None,
                1 => Some(f64::NAN),
                2 => Some(0.0),
                3 => Some(-r.f() * mag),
                _ => Some(r.f() * mag),
            }
        };
        let series: Vec<SeriesData> = (0..nser)
            .map(|i| SeriesData {
                name: format!("S{i}"),
                values: (0..npts).map(|_| val(&mut r)).collect(),
                x: if r.next().is_multiple_of(2) { Some((0..npts).map(|_| val(&mut r)).collect()) } else { None },
                sizes: if r.next().is_multiple_of(2) { Some((0..npts).map(|_| val(&mut r)).collect()) } else { None },
                color: [(r.next() % 256) as u8, 100, 50, 255],
                kind: ALL_KINDS[(r.next() % ALL_KINDS.len() as u64) as usize],
                secondary: r.next().is_multiple_of(3),
                smooth: r.next().is_multiple_of(4),
                trendlines: (0..r.next() % 3)
                    .map(|_| TrendlineData {
                        spec: Trendline {
                            kind: [
                                TrendlineKind::Linear,
                                TrendlineKind::Exponential,
                                TrendlineKind::Logarithmic,
                                TrendlineKind::Polynomial((r.next() % 8) as u8),
                                TrendlineKind::Power,
                                TrendlineKind::MovingAverage((r.next() % 6) as u32),
                            ][(r.next() % 6) as usize],
                            forward: [0.0, 1.5, -2.0, f64::NAN, 1e300][(r.next() % 5) as usize],
                            backward: [0.0, 0.5, f64::INFINITY][(r.next() % 3) as usize],
                            intercept: [None, Some(0.0), Some(-5.0), Some(f64::NAN)][(r.next() % 4) as usize],
                            display_equation: r.next().is_multiple_of(2),
                            display_r_squared: r.next().is_multiple_of(2),
                            width: [None, Some(0.0), Some(f32::NAN), Some(3.0)][(r.next() % 4) as usize],
                            ..Trendline::default()
                        },
                        color: [0, 0, 0, 255],
                    })
                    .collect(),
                number_format: ["General", "$#,##0", "0.00%", "yyyy-mm-dd", "#,##0.0;[Red]-#,##0.0", "@"][(r.next() % 6) as usize].into(),
            })
            .collect();
        let ncat = (r.next() % 50) as usize;
        let d = ChartData {
            category_levels: vec![],
            palette: vec![],
            categories: (0..ncat).map(|i| "c".repeat(i % 7) + &i.to_string()).collect(),
            series,
        };
        let mut c = chart(kind);
        c.legend = [LegendPos::None, LegendPos::Bottom, LegendPos::Top, LegendPos::Left, LegendPos::Right][(r.next() % 5) as usize];
        c.data_labels = r.next().is_multiple_of(2);
        c.gridlines = r.next().is_multiple_of(2);
        if r.next().is_multiple_of(3) {
            c.title = None;
        }
        let w = [0.0, -3.0, 1.0, 12.0, 80.0, 320.0, 1200.0][(r.next() % 7) as usize] as f32 + r.f() as f32 * 10.0;
        let h = [0.0, 2.0, 30.0, 200.0, 900.0][(r.next() % 5) as usize] as f32 + r.f() as f32 * 10.0;
        let p = render(&c, &d, w, h, &m);
        if w >= 1.0 && h >= 1.0 {
            assert_sane(&p, w, h);
        }
        if iter % 20 == 0 && w >= 1.0 && h >= 1.0 && w < 2000.0 && h < 2000.0 {
            let img = rasterize(&p, w as u32, h as u32, [255; 4]);
            assert_eq!(img.len(), (w as u32 * h as u32 * 4) as usize);
        }
        let vals: Vec<Option<f64>> = (0..npts).map(|_| val(&mut r)).collect();
        for k in [SparklineKind::Line, SparklineKind::Column, SparklineKind::WinLoss] {
            let sp = render_sparkline(k, &vals, [0, 0, 0, 255], r.next().is_multiple_of(2), w, h);
            if w >= 1.0 && h >= 1.0 {
                assert_sane(&sp, w.max(1.0), h.max(1.0));
            }
        }
    }
}

#[test]
fn sparklines() {
    let v = [Some(1.0), Some(-2.0), None, Some(4.0), Some(0.0), Some(3.0)];
    let line = render_sparkline(SparklineKind::Line, &v, [1, 2, 3, 255], true, 60.0, 16.0);
    assert_eq!(line.iter().filter(|p| matches!(p, Prim::Line { .. })).count(), 2);
    assert_eq!(line.iter().filter(|p| matches!(p, Prim::Circle { .. })).count(), 5);
    assert_sane(&line, 60.0, 16.0);
    let col = render_sparkline(SparklineKind::Column, &v, [1, 2, 3, 255], false, 60.0, 16.0);
    assert_eq!(col.len(), 5);
    assert_sane(&col, 60.0, 16.0);
    let wl = render_sparkline(SparklineKind::WinLoss, &v, [1, 2, 3, 255], false, 60.0, 16.0);
    assert_eq!(wl.len(), 4);
    assert!(render_sparkline(SparklineKind::Line, &v, [0; 4], false, 0.0, 10.0).is_empty());
    assert!(render_sparkline(SparklineKind::Line, &[], [0; 4], false, 10.0, 10.0).is_empty());
}

// ---------------------------------------------------------------- trendlines

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * (1.0 + b.abs())
}

fn fit(kind: TrendlineKind, pts: &[(f64, f64)]) -> crate::trend::TrendFit {
    crate::trend::fit_trendline(&Trendline { kind, ..Trendline::default() }, pts).expect("fits")
}

#[test]
fn linear_trendline_matches_least_squares() {
    // Slope 0.6, intercept 2.2, R² = 1 - 2.4 / 6.
    let f = fit(TrendlineKind::Linear, &[(1.0, 2.0), (2.0, 4.0), (3.0, 5.0), (4.0, 4.0), (5.0, 5.0)]);
    assert!(close(f.coef[0], 2.2) && close(f.coef[1], 0.6), "{:?}", f.coef);
    assert!(close(f.r2.unwrap(), 0.6));
    assert_eq!(f.equation(), "y = 0.6x + 2.2");
    assert_eq!(f.r_squared(), "R² = 0.6");
    assert!(close(f.eval(10.0).unwrap(), 8.2));
    let down = fit(TrendlineKind::Linear, &[(0.0, 1.0), (1.0, -1.0), (2.0, -3.0)]);
    assert_eq!(down.equation(), "y = -2x + 1");
}

#[test]
fn set_intercept_fits_through_it() {
    // Through the origin: slope Σxy / Σx² = 30 / 14; R² against the uncentred total.
    let pts = [(1.0, 2.0), (2.0, 5.0), (3.0, 6.0)];
    let f = crate::trend::fit_trendline(&Trendline { intercept: Some(0.0), ..Trendline::default() }, &pts).unwrap();
    assert!(close(f.coef[0], 0.0) && close(f.coef[1], 15.0 / 7.0), "{:?}", f.coef);
    let ss_res: f64 = pts.iter().map(|(x, y)| (y - 15.0 / 7.0 * x).powi(2)).sum();
    assert!(close(f.r2.unwrap(), 1.0 - ss_res / 65.0));
    let q = crate::trend::fit_trendline(
        &Trendline { kind: TrendlineKind::Polynomial(2), intercept: Some(2.0), ..Trendline::default() },
        &[(1.0, 0.0), (2.0, 0.0), (3.0, 2.0), (4.0, 6.0)],
    )
    .unwrap();
    assert!(close(q.coef[0], 2.0) && close(q.coef[1], -3.0) && close(q.coef[2], 1.0), "{:?}", q.coef);
}

#[test]
fn transformed_trendlines_recover_their_curves() {
    let xs: [f64; 6] = [0.5, 1.0, 2.0, 3.0, 4.5, 6.0];
    let e = fit(TrendlineKind::Exponential, &xs.map(|x| (x, 3.0 * (0.5 * x).exp())));
    assert!(close(e.coef[0].exp(), 3.0) && close(e.coef[1], 0.5));
    assert_eq!(e.equation(), "y = 3e^(0.5x)");
    let l = fit(TrendlineKind::Logarithmic, &xs.map(|x| (x, 2.0 * x.ln() + 1.0)));
    assert!(close(l.coef[1], 2.0) && close(l.coef[0], 1.0));
    assert_eq!(l.equation(), "y = 2ln(x) + 1");
    let p = fit(TrendlineKind::Power, &xs.map(|x| (x, 4.0 * x.powf(1.5))));
    assert!(close(p.coef[0].exp(), 4.0) && close(p.coef[1], 1.5));
    assert_eq!(p.equation(), "y = 4x^1.5");
    assert!(close(p.r2.unwrap(), 1.0));
    assert_eq!(p.eval(-1.0), None, "power is undefined left of zero");
    // Points the transform can't take are skipped, as Excel documents.
    let skip = fit(TrendlineKind::Exponential, &[(1.0, 2.0f64.exp()), (2.0, 0.0), (3.0, -4.0), (4.0, 8.0f64.exp())]);
    assert!(close(skip.coef[1], 2.0) && close(skip.coef[0], 0.0), "{:?}", skip.coef);
    assert!(
        crate::trend::fit_trendline(&Trendline { kind: TrendlineKind::Power, ..Trendline::default() }, &[(-1.0, 1.0), (0.0, 2.0), (3.0, 3.0)])
            .is_none()
    );
}

#[test]
fn polynomial_trendlines_are_stable() {
    let q = fit(TrendlineKind::Polynomial(2), &[0.0, 1.0, 2.0, 3.0, 4.0].map(|x| (x, x * x - 3.0 * x + 2.0)));
    assert!(close(q.coef[0], 2.0) && close(q.coef[1], -3.0) && close(q.coef[2], 1.0), "{:?}", q.coef);
    assert_eq!(q.equation(), "y = x² - 3x + 2");
    // Year-valued x would make the raw normal equations hopelessly ill-conditioned.
    let curve = |x: f64| {
        let t = x - 2000.0;
        0.001 * t.powi(6) - 0.02 * t.powi(5) + 0.1 * t.powi(3) - t + 7.0
    };
    let years: Vec<(f64, f64)> = (2001..=2012).map(|x| (f64::from(x), curve(f64::from(x)))).collect();
    let six = fit(TrendlineKind::Polynomial(6), &years);
    for x in [2001.0, 2006.5, 2012.0, 2014.0] {
        let (got, want) = (six.eval(x).unwrap(), curve(x));
        assert!((got - want).abs() <= 1e-6 * (1.0 + want.abs()), "{x}: {got} vs {want}");
    }
    assert!(six.r2.unwrap() > 0.999_999);
}

#[test]
fn degenerate_trendlines_fit_nothing() {
    let lin = |pts: &[(f64, f64)]| crate::trend::fit_trendline(&Trendline::default(), pts);
    assert!(lin(&[]).is_none());
    assert!(lin(&[(1.0, 1.0)]).is_none());
    assert!(lin(&[(2.0, 1.0), (2.0, 5.0), (2.0, 3.0)]).is_none(), "all x equal is singular");
    assert!(lin(&[(f64::NAN, 1.0), (1.0, f64::INFINITY), (2.0, 3.0)]).is_none());
    assert!(
        crate::trend::fit_trendline(&Trendline { kind: TrendlineKind::Polynomial(3), ..Trendline::default() }, &[(1.0, 1.0), (2.0, 4.0), (3.0, 9.0)])
            .is_none()
    );
    assert!(
        crate::trend::fit_trendline(&Trendline { kind: TrendlineKind::MovingAverage(2), ..Trendline::default() }, &[(1.0, 1.0), (2.0, 2.0)])
            .is_none()
    );
    let flat = lin(&[(1.0, 4.0), (2.0, 4.0), (3.0, 4.0)]).unwrap();
    assert_eq!((flat.equation(), flat.r2), ("y = 4".to_string(), None));
    assert!(lin(&[(0.0, 1e300), (1.0, -1e300), (2.0, 1e300)]).is_none_or(|f| f.coef.iter().all(|c| c.is_finite())));
}

#[test]
fn moving_average_and_labels() {
    let pts = [(1.0, 1.0), (2.0, 2.0), (3.0, 6.0), (4.0, f64::NAN), (5.0, 4.0)];
    assert_eq!(crate::trend::moving_average(&pts, 2), vec![(2.0, 1.5), (3.0, 4.0), (5.0, 5.0)]);
    assert_eq!(crate::trend::moving_average(&pts, 3), vec![(3.0, 3.0), (5.0, 4.0)]);
    assert!(crate::trend::moving_average(&pts, 9).is_empty());
    assert_eq!(crate::trend::num_text(0.000012345), "1.2345E-05");
    assert_eq!(crate::trend::num_text(-0.00001), "-1E-05");
    assert_eq!(crate::trend::num_text(1234.56789), "1234.5679");
    assert_eq!(crate::trend::auto_name(TrendlineKind::MovingAverage(3), "Sales"), "3 per. Mov. Avg. (Sales)");
    assert_eq!(crate::trend::auto_name(TrendlineKind::Exponential, "Sales"), "Expon. (Sales)");
}

fn lines_of(prims: &[Prim], color: Rgba) -> Vec<&Vec<[f32; 2]>> {
    prims
        .iter()
        .filter_map(|p| match p {
            Prim::Line { pts, color: c, .. } if *c == color => Some(pts),
            _ => None,
        })
        .collect()
}

#[test]
fn line_chart_draws_trendline_forecast_and_labels() {
    let m = ApproxMeasure;
    let tc: Rgba = [1, 2, 3, 255];
    let mut s = series("Sales", &[9.0, 7.0, 5.0, 3.0], [10, 20, 30, 255], ChartKind::Line);
    s.trendlines =
        vec![TrendlineData { spec: Trendline { forward: 1.0, display_equation: true, display_r_squared: true, ..Trendline::default() }, color: tc }];
    let d =
        ChartData { category_levels: vec![], palette: vec![], categories: ["a", "b", "c", "d"].map(String::from).to_vec(), series: vec![s.clone()] };
    let mut c = chart(ChartKind::Line);
    c.legend = LegendPos::None; // keeps legend swatches out of the line counts
    let prims = render(&c, &d, 600.0, 400.0, &m);
    assert_sane(&prims, 600.0, 400.0);
    let data = lines_of(&prims, [10, 20, 30, 255]);
    let trend = lines_of(&prims, tc);
    assert_eq!((data.len(), trend.len()), (1, 1));
    let (d0, dn) = (data[0][0], data[0][3]);
    let (t0, tn) = (trend[0][0], *trend[0].last().unwrap());
    // Exact data: the trendline starts on the first point and runs through the last,
    // then on one more category (the forecast slot).
    assert!((t0[0] - d0[0]).abs() < 0.01 && (t0[1] - d0[1]).abs() < 0.01, "{t0:?} vs {d0:?}");
    let step = (dn[0] - d0[0]) / 3.0;
    assert!((tn[0] - (dn[0] + step)).abs() < 0.01, "forecast end {tn:?}, last point {dn:?}");
    assert!(tn[1] > dn[1], "keeps falling past the data");
    let t = texts(&prims);
    assert!(t.contains(&"y = -2x + 11") && t.contains(&"R² = 1"), "{t:?}");
    assert!(legend_entries(&chart(ChartKind::Line), &d).iter().any(|(n, col, sw)| n == "Linear (Sales)" && *col == tc && *sw == Swatch::Line));
    // Not on chart kinds Excel doesn't offer trendlines on.
    let pie = render(&chart(ChartKind::Pie), &d, 600.0, 400.0, &m);
    assert!(lines_of(&pie, tc).is_empty());
    // One point: nothing to fit, nothing drawn.
    s.values.truncate(1);
    let one = ChartData { category_levels: vec![], palette: vec![], categories: vec!["a".into()], series: vec![s] };
    assert!(lines_of(&render(&c, &one, 600.0, 400.0, &m), tc).is_empty());
}

#[test]
fn scatter_trendlines_use_x_values_and_stay_in_the_plot() {
    let m = ApproxMeasure;
    let xs = [1.0, 2.0, 4.0, 8.0];
    let mut s = series("P", &xs.map(|x| 4.0 * x * x), [10, 20, 30, 255], ChartKind::Scatter);
    s.x = Some(xs.iter().map(|x| Some(*x)).collect());
    let colors: [Rgba; 3] = [[1, 0, 0, 255], [2, 0, 0, 255], [3, 0, 0, 255]];
    s.trendlines = vec![
        TrendlineData {
            spec: Trendline { kind: TrendlineKind::Power, backward: 0.5, display_equation: true, ..Trendline::default() },
            color: colors[0],
        },
        // A steep exponential leaves the plot box and is clipped to it.
        TrendlineData { spec: Trendline { kind: TrendlineKind::Exponential, forward: 30.0, ..Trendline::default() }, color: colors[1] },
        TrendlineData { spec: Trendline { kind: TrendlineKind::MovingAverage(2), ..Trendline::default() }, color: colors[2] },
    ];
    let d = ChartData { category_levels: vec![], palette: vec![], categories: vec![], series: vec![s] };
    let mut c = chart(ChartKind::Scatter);
    c.legend = LegendPos::None;
    let prims = render(&c, &d, 600.0, 400.0, &m);
    assert_sane(&prims, 600.0, 400.0);
    for c in colors {
        assert!(!lines_of(&prims, c).is_empty(), "{c:?} not drawn");
    }
    assert_eq!(lines_of(&prims, colors[2])[0].len(), 3, "moving average: one point per full window");
    assert!(texts(&prims).contains(&"y = 4x^2"));
}

#[test]
fn treemap_and_histogram_shapes() {
    let d = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        series: vec![series("t", &[6.0, 6.0, 4.0, 3.0], [10, 20, 30, 255], ChartKind::Treemap)],
    };
    let mut c = chart(ChartKind::Treemap);
    c.title = None;
    c.legend = LegendPos::None;
    let prims = render(&c, &d, 600.0, 400.0, &ApproxMeasure);
    let rects: Vec<f32> = prims
        .iter()
        .skip(1)
        .filter_map(|p| match p {
            Prim::Rect { w, h, .. } => Some(w * h),
            _ => None,
        })
        .collect();
    assert_eq!(rects.len(), 4);
    // Areas proportional to values.
    assert!((rects[0] / rects[3] - 2.0).abs() < 0.05, "{rects:?}");

    let vals: Vec<f64> = (0..100).map(|i| i as f64).collect();
    let d = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: vec![],
        series: vec![series("h", &vals, [10, 20, 30, 255], ChartKind::Histogram)],
    };
    let prims = render(&chart(ChartKind::Histogram), &d, 600.0, 400.0, &ApproxMeasure);
    let bins = prims.iter().filter(|p| matches!(p, Prim::Rect { fill: Some([10, 20, 30, 255]), .. })).count();
    assert_eq!(bins, 10);
}

#[test]
fn histogram_boundary_values_match_interval_labels() {
    let heights = |vals: &[f64]| -> Vec<f32> {
        let d = ChartData {
            category_levels: vec![],
            palette: vec![],
            categories: vec![],
            series: vec![series("h", vals, [10, 20, 30, 255], ChartKind::Histogram)],
        };
        let prims = render(&chart(ChartKind::Histogram), &d, 600.0, 400.0, &ApproxMeasure);
        prims.iter().filter_map(|p| if let Prim::Rect { h, fill: Some([10, 20, 30, 255]), .. } = p { Some(*h) } else { None }).collect()
    };
    // Labels are "[0, 7.5]" and "(7.5, 15.0]": 7.5 belongs to the first bar, so the counts are 2 and 1.
    let h = heights(&[0.0, 7.5, 15.0]);
    assert_eq!(h.len(), 2);
    assert!((h[0] / h[1] - 2.0).abs() < 0.01, "{h:?}");
    let h = heights(&[-15.0, -7.5, 0.0]);
    assert!((h[0] / h[1] - 2.0).abs() < 0.01, "{h:?}");
    // A value off the boundary still goes right: [0, 7.5] holds 1, (7.5, 15.0] holds 2.
    let h = heights(&[0.0, 7.75, 15.0]);
    assert!((h[1] / h[0] - 2.0).abs() < 0.01, "{h:?}");
    // Five values, three bins [0,5] (5,10] (10,15]: counts 3, 1, 1.
    let h = heights(&[0.0, 5.0, 5.0, 10.0, 15.0]);
    assert_eq!(h.len(), 3);
    assert!((h[0] / h[1] - 3.0).abs() < 0.01 && (h[1] - h[2]).abs() < 0.01, "{h:?}");
}

#[test]
fn union_series_references() {
    let mut wb = Workbook::new();
    {
        let s = wb.sheet_mut(0).unwrap();
        for (r, (l, v)) in [("a", 1.0), ("b", 2.0), ("Total", 3.0), ("d", 4.0)].iter().enumerate() {
            s.set_value(CellRef::new(r as u32, 0), Value::from(*l));
            s.set_value(CellRef::new(r as u32, 1), Value::Number(*v));
        }
    }
    let mut c = chart(ChartKind::ColumnClustered);
    c.series = vec![Series {
        name: Some("\"Sum of X\"".into()),
        categories: Some("(Sheet1!$A$1:$A$2,Sheet1!$A$4)".into()),
        values: "(Sheet1!$B$1:$B$2,Sheet1!$B$4)".into(),
        ..Series::default()
    }];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.series[0].values, vec![Some(1.0), Some(2.0), Some(4.0)]);
    assert_eq!(d.series[0].name, "Sum of X");
    assert_eq!(d.categories, vec!["a", "b", "d"]);
    // Deeply nested parentheses from a file are bounded, not a crash.
    c.series[0].values = format!("{}Sheet1!$B$1,Sheet1!$B$2{}", "(".repeat(5000), ")".repeat(5000));
    let _ = resolve(&wb, 0, &c);
}

#[test]
fn box_and_stock_charts_honour_the_gridlines_flag() {
    // Horizontal full-width lines in the gridline colour are the value-axis gridlines.
    let grid: Rgba = [217, 217, 217, 255];
    let is_grid = |p: &Prim| match p {
        Prim::Line { color, pts, .. } => *color == grid && pts.len() == 2 && pts[0][1] == pts[1][1] && pts[1][0] - pts[0][0] > 200.0,
        _ => false,
    };
    let grids = |prims: &[Prim]| prims.iter().filter(|p| is_grid(p)).count();
    for kind in [ChartKind::BoxWhisker, ChartKind::Stock, ChartKind::ColumnClustered] {
        let mut d = sample(kind);
        d.series.push(series("Close", &[10000.0, 12000.0, 9000.0, 14000.0, 20000.0, 16000.0], [90, 90, 90, 255], kind));
        let mut c = chart(kind);
        let on = render(&c, &d, 500.0, 350.0, &ApproxMeasure);
        c.gridlines = false;
        let off = render(&c, &d, 500.0, 350.0, &ApproxMeasure);
        assert!(grids(&on) > 0, "{kind:?}: gridlines on must draw lines");
        assert_eq!(grids(&off), 0, "{kind:?}: gridlines off must hide them");
    }
}

#[test]
fn funnel_value_labels_follow_the_data_labels_flag() {
    let data = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: vec!["a".into(), "b".into(), "c".into()],
        series: vec![series("S", &[11111.0, 22222.0, 33333.0], [20, 96, 130, 255], ChartKind::Funnel)],
    };
    let mut c = chart(ChartKind::Funnel);
    c.data_labels = false;
    let off = render(&c, &data, 500.0, 350.0, &ApproxMeasure);
    c.data_labels = true;
    let on = render(&c, &data, 500.0, 350.0, &ApproxMeasure);
    let values = ["$11,111", "$22,222", "$33,333"];
    for v in values {
        assert!(texts(&on).contains(&v), "{v} missing with labels on");
        assert!(!texts(&off).contains(&v), "{v} drawn with labels off");
    }
    for cat in ["a", "b", "c"] {
        assert!(texts(&off).contains(&cat));
        assert!(texts(&on).contains(&cat));
    }
}

#[test]
fn pie_points_follow_workbook_theme() {
    let mut wb = Workbook::new();
    wb.theme.colors[4] = 0x4F6D8A;
    wb.theme.colors[5] = 0x8AA1B8;
    let mut c = chart(ChartKind::Pie);
    c.series = vec![Series {
        name: None,
        categories: None,
        values: "{5,3}".into(),
        bubble_sizes: None,
        color: None,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
    }];
    let d = resolve(&wb, 0, &c);
    let expected = vec![series_color(&wb.theme, 0), series_color(&wb.theme, 1)];
    assert_eq!(expected, vec![[0x4F, 0x6D, 0x8A, 0xFF], [0x8A, 0xA1, 0xB8, 0xFF]]);
    let legend: Vec<Rgba> = legend_entries(&c, &d).iter().map(|e| e.1).collect();
    assert_eq!(legend, expected);
    let fills: Vec<Rgba> = render(&c, &d, 480.0, 288.0, &ApproxMeasure)
        .iter()
        .filter_map(|p| match p {
            Prim::Wedge { fill, .. } => Some(*fill),
            _ => None,
        })
        .collect();
    assert_eq!(fills, expected);
    for kind in [ChartKind::Doughnut, ChartKind::Treemap, ChartKind::Sunburst] {
        c.kind = kind;
        let legend: Vec<Rgba> = legend_entries(&c, &d).iter().map(|e| e.1).collect();
        assert_eq!(legend, expected, "{kind:?}");
    }
    // Past the 36-colour cycle the points keep following the workbook theme.
    c.kind = ChartKind::Pie;
    c.series[0].values = format!("{{{}}}", ["1"; 40].join(","));
    let d = resolve(&wb, 0, &c);
    let legend: Vec<Rgba> = legend_entries(&c, &d).iter().map(|e| e.1).collect();
    assert_eq!(legend.len(), 40);
    assert_eq!(legend.get(37), Some(&series_color(&wb.theme, 37)));
    assert_ne!(series_color(&wb.theme, 37), series_color(&gridcraft_model::Theme::default(), 37));
}

#[test]
fn area_charts_draw_source_value_labels_only_when_enabled() {
    for kind in [ChartKind::Area, ChartKind::AreaStacked, ChartKind::AreaStacked100] {
        let d = sample(kind);
        let mut c = chart(kind);
        c.data_labels = false;
        let off = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
        c.data_labels = true;
        let on = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
        let count = |p: &[Prim], v: &str| texts(p).iter().filter(|t| **t == v).count();
        // One label per point: 3 series x 6 categories.
        assert_eq!(texts(&on).len(), texts(&off).len() + 18, "{kind:?}");
        for v in ["$12,000", "$38,000", "$9,000", "$27,000"] {
            assert_eq!(count(&on, v), count(&off, v) + 1, "{kind:?}: {v}");
        }
        // Stacked areas label the source values, not cumulative totals (12,000 + 38,000).
        assert_eq!(count(&on, "$50,000"), count(&off, "$50,000"), "{kind:?}: cumulative total shown");
        let polys = |p: &[Prim]| p.iter().filter(|p| matches!(p, Prim::Polygon { .. })).cloned().collect::<Vec<_>>();
        assert_eq!(polys(&on), polys(&off));
        // The first series' labels are drawn after every area, so the next area can't cover them.
        let last_poly = on.iter().rposition(|p| matches!(p, Prim::Polygon { .. }));
        let first_label = on.iter().position(|p| matches!(p, Prim::Text { text, .. } if text == "$12,000"));
        assert!(first_label > last_poly, "{kind:?}: label drawn under a later area");
    }
}

#[test]
fn drawing_and_resolving_agree_on_the_combo_plot_kind() {
    // Resolved series carry `Chart::series_kind`, the kind drawing uses: for every chart kind,
    // series kind (or none) and series index it is never a combo, and outside a combo it is the
    // chart's kind or one of its family.
    let wb = Workbook::new();
    let series_kinds = std::iter::once(None).chain(ALL_KINDS.iter().map(|k| Some(*k)));
    for ck in ALL_KINDS {
        for sk in series_kinds.clone() {
            for i in 0..3 {
                let mut c = chart(ck);
                c.series = (0..3).map(|j| Series { kind: if j == i { sk } else { None }, ..Default::default() }).collect();
                let resolved: Vec<ChartKind> = resolve(&wb, 0, &c).series.iter().map(|s| s.kind).collect();
                assert_eq!(resolved, c.series_kinds(), "{ck:?} {sk:?} {i}");
                if ck != ChartKind::Combo {
                    assert!(gridcraft_model::same_family(ck, resolved[i]), "{ck:?} {sk:?} {i}");
                } else {
                    assert_ne!(resolved[i], ChartKind::Combo);
                }
            }
        }
    }
}

#[test]
fn ticks_of_a_scale_off_the_step_grid_start_at_its_min() {
    let s = Scale { min: 3.0, max: 7.0, step: 4.0 };
    assert_eq!(s.ticks(), vec![3.0, 7.0]);
    // Aligned scales still snap float error away.
    let s = Scale { min: 0.0, max: 1.0, step: 0.1 };
    assert!((s.ticks()[3] - 0.3).abs() < 1e-12 && s.ticks()[0] == 0.0);
}

#[test]
fn hundred_percent_stacked_areas_fill_each_category_to_its_total() {
    let d = ChartData {
        category_levels: vec![],
        palette: vec![],
        categories: ["Q1", "Q2", "Q3", "Q4"].iter().map(|s| s.to_string()).collect(),
        series: vec![
            series("North", &[10.0, 30.0, 0.0, 5.0], [20, 96, 130, 255], ChartKind::AreaStacked100),
            series("South", &[30.0, 10.0, 0.0, 15.0], [25, 107, 36, 255], ChartKind::AreaStacked100),
        ],
    };
    let mut c = chart(ChartKind::AreaStacked100);
    c.data_labels = false;
    let p = render(&c, &d, 640.0, 400.0, &ApproxMeasure);
    let t = texts(&p);
    assert!(t.contains(&"100%") && t.contains(&"0%"), "{t:?}");
    assert!(!t.contains(&"40"), "{t:?}");
    let polys: Vec<&Vec<[f32; 2]>> = p
        .iter()
        .filter_map(|p| match p {
            Prim::Polygon { pts, .. } => Some(pts),
            _ => None,
        })
        .collect();
    let (Some(north), Some(south)) = (polys.first(), polys.get(1)) else { panic!("two areas expected: {polys:?}") };
    // Each polygon is its top edge left to right, then its base right to left.
    let top = |poly: &[[f32; 2]], j: usize| poly.get(j).map(|p| p[1]).unwrap_or(f32::NAN);
    let base = |poly: &[[f32; 2]], j: usize| poly.get(poly.len() - 1 - j).map(|p| p[1]).unwrap_or(f32::NAN);
    let (y0, y100) = (base(north, 0), top(south, 0));
    assert!(y0 > y100, "{y0} {y100}");
    let frac = |y: f32| (y0 - y) / (y0 - y100);
    for (j, share) in [(0, 0.25), (1, 0.75), (3, 0.25)] {
        assert!((frac(top(north, j)) - share).abs() < 1e-3, "Q{}: North reaches {}", j + 1, frac(top(north, j)));
        assert!((frac(top(south, j)) - 1.0).abs() < 1e-3, "Q{}: South reaches {}", j + 1, frac(top(south, j)));
    }
    // A category whose total is zero stays on the baseline.
    assert!(frac(top(south, 2)).abs() < 1e-3, "Q3: South reaches {}", frac(top(south, 2)));
}

#[test]
fn two_level_categories_resolve_and_draw() {
    let years = ["2023", "", "", "2024", "", ""];
    let months = ["Oct", "Nov", "Dec", "Jan", "Feb", "Mar"];
    let mut wb = Workbook::new();
    {
        let s = wb.sheet_mut(0).unwrap();
        s.name = "Sales".into();
        for (i, (y, m)) in years.iter().zip(months).enumerate() {
            // Year beside month beside the values, points down the rows…
            let r = i as u32 + 1;
            if !y.is_empty() {
                s.set_value(CellRef::new(r, 0), Value::from(*y));
                s.set_value(CellRef::new(10, i as u32), Value::from(*y));
            }
            s.set_value(CellRef::new(r, 1), Value::from(m));
            s.set_value(CellRef::new(r, 2), Value::Number(10.0 * (i as f64 + 1.0)));
            // …and the same table in rows, points along the columns.
            s.set_value(CellRef::new(11, i as u32), Value::from(m));
            s.set_value(CellRef::new(12, i as u32), Value::Number(10.0 * (i as f64 + 1.0)));
        }
    }
    let mut c = chart(ChartKind::Line);
    c.series = vec![Series { categories: Some("Sales!$A$2:$B$7".into()), values: "Sales!$C$2:$C$7".into(), ..Default::default() }];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, months);
    assert_eq!(d.category_levels, vec![years.map(String::from).to_vec()]);

    let texts = |prims: &[Prim]| -> Vec<(String, f32, f32)> {
        prims.iter().filter_map(|p| if let Prim::Text { text, x, y, .. } = p { Some((text.clone(), *x, *y)) } else { None }).collect()
    };
    let prims = render(&c, &d, 600.0, 300.0, &ApproxMeasure);
    assert_sane(&prims, 600.0, 300.0);
    let t = texts(&prims);
    let at = |t: &[(String, f32, f32)], s: &str| {
        t.iter().find(|(x, _, _)| x == s).map(|(_, x, y)| (*x, *y)).unwrap_or_else(|| panic!("no {s:?} in {t:?}"))
    };
    // Six evenly spaced month labels across the plot, not twelve slots for a two-column range.
    let xs: Vec<f32> = months.iter().map(|m| at(&t, m).0).collect();
    let step = xs[1] - xs[0];
    assert!(step > 40.0 && xs.windows(2).all(|w| (w[1] - w[0] - step).abs() < 0.5), "{xs:?}");
    // The years sit in a row below the months, each under its own group.
    let (x23, y23) = at(&t, "2023");
    let (x24, y24) = at(&t, "2024");
    assert!(y23 > at(&t, "Oct").1 && (y23 - y24).abs() < 0.01, "{t:?}");
    assert!(x23 < at(&t, "Dec").0 && x24 > at(&t, "Dec").0 && x24 < at(&t, "Mar").0, "{t:?}");

    // Bars: the levels stand in a column left of the labels.
    c.kind = ChartKind::BarClustered;
    let prims = render(&c, &d, 600.0, 300.0, &ApproxMeasure);
    assert_sane(&prims, 600.0, 300.0);
    let t = texts(&prims);
    let (x23, y23) = at(&t, "2023");
    let (x_oct, y_oct) = at(&t, "Oct");
    // Categories run up the axis: the group's name sits left of its labels, between Oct and Dec.
    assert!(x23 < x_oct && y23 < y_oct && y23 > at(&t, "Dec").1, "{t:?}");
    let (x24, y24) = at(&t, "2024");
    assert!((x23 - x24).abs() < 0.01 && y24 < y23, "same column, 2024 above: {t:?}");

    // A row of values: the levels are rows.
    c.series[0].categories = Some("Sales!$A$11:$F$12".into());
    c.series[0].values = "Sales!$A$13:$F$13".into();
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, months);
    assert_eq!(d.category_levels, vec![years.map(String::from).to_vec()]);

    // One column of categories: a plain axis, as before.
    c.series[0].categories = Some("Sales!$B$2:$B$7".into());
    c.series[0].values = "Sales!$C$2:$C$7".into();
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.categories, months);
    assert!(d.category_levels.is_empty());
}

#[test]
fn a_huge_trendline_forecast_does_not_collapse_the_columns() {
    let m = ApproxMeasure;
    let tc: Rgba = [1, 2, 3, 255];
    let col: Rgba = [10, 20, 30, 255];
    let mut s = series("Sales", &[1.0, 3.0, 2.0, 5.0, 4.0], col, ChartKind::ColumnClustered);
    s.trendlines = vec![TrendlineData { spec: Trendline { forward: 1e6, backward: 1e6, ..Trendline::default() }, color: tc }];
    let d = ChartData { palette: vec![], category_levels: vec![], categories: ["a", "b", "c", "d", "e"].map(String::from).to_vec(), series: vec![s] };
    let mut c = chart(ChartKind::ColumnClustered);
    c.legend = LegendPos::None;
    let prims = render(&c, &d, 600.0, 400.0, &m);
    assert_sane(&prims, 600.0, 400.0);
    let widths: Vec<f32> =
        prims.iter().filter_map(|p| if let Prim::Rect { w, fill: Some(f), .. } = p { (*f == col).then_some(*w) } else { None }).collect();
    assert_eq!(widths.len(), 5);
    assert!(widths.iter().all(|w| *w > 4.0), "columns stay visible: {widths:?}");
    let trend = lines_of(&prims, tc);
    assert_eq!(trend.len(), 1);
    assert!(trend[0].len() >= 20, "the trendline is sampled over the plot, not the forecast: {}", trend[0].len());
}

#[test]
fn box_whisker_draws_each_series_quartile_method() {
    use gridcraft_model::QuartileMethod;
    let mut c = chart(ChartKind::BoxWhisker);
    c.legend = LegendPos::None;
    c.series = vec![Series::default(), Series { quartiles: QuartileMethod::Inclusive, ..Default::default() }];
    let color = [12, 34, 56, 255];
    let d = ChartData {
        series: vec![series("A", &[1., 2., 3., 4., 5., 6.], color, c.kind), series("B", &[1., 2., 3., 4., 5., 6.], color, c.kind)],
        ..Default::default()
    };
    let prims = render(&c, &d, 640., 400., &ApproxMeasure);
    let heights: Vec<f32> = prims
        .iter()
        .filter_map(|p| match p {
            Prim::Rect { h, fill, .. } if *fill == Some(color) => Some(*h),
            _ => None,
        })
        .collect();
    assert_eq!(heights.len(), 2);
    // The boxes span 3.5 and 2.5 data units on the same value axis.
    assert!((heights[0] / heights[1] - 1.4).abs() < 0.001);
}

#[test]
fn a_series_named_by_a_name_on_a_sheet_with_an_apostrophe_resolves() {
    // #2012: `'Bob''s'!Title` kept the doubled quote, so the sheet wasn't found and the series
    // was named by the literal text.
    let mut wb = Workbook::new();
    {
        let s = wb.sheet_mut(0).unwrap();
        s.name = "Bob's".into();
        s.set_value(CellRef::new(0, 0), Value::from("Takings"));
        s.set_value(CellRef::new(1, 0), Value::Number(4.0));
        s.set_value(CellRef::new(2, 0), Value::Number(9.0));
    }
    wb.names.push(gridcraft_model::DefinedName {
        name: "Title".into(),
        scope: Some(0),
        formula: "'Bob''s'!$A$1".into(),
        comment: String::new(),
        hidden: false,
    });
    let mut c = chart(ChartKind::ColumnClustered);
    c.series = vec![Series {
        name: Some("'Bob''s'!Title".into()),
        categories: None,
        values: "'Bob''s'!$A$2:$A$3".into(),
        bubble_sizes: None,
        color: None,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
    }];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.series.first().map(|s| s.name.as_str()), Some("Takings"));
}

#[test]
fn a_series_named_by_a_name_on_a_sheet_with_a_bang_resolves() {
    // `'a!b'!Title`: the sheet name holds a `!`, so the evaluator must split the parsed name at
    // the last one, not the first.
    let mut wb = Workbook::new();
    {
        let s = wb.sheet_mut(0).unwrap();
        s.name = "a!b".into();
        s.set_value(CellRef::new(0, 0), Value::from("Takings"));
        s.set_value(CellRef::new(1, 0), Value::Number(4.0));
        s.set_value(CellRef::new(2, 0), Value::Number(9.0));
    }
    wb.names.push(gridcraft_model::DefinedName {
        name: "Title".into(),
        scope: Some(0),
        formula: "'a!b'!$A$1".into(),
        comment: String::new(),
        hidden: false,
    });
    let mut c = chart(ChartKind::ColumnClustered);
    c.series = vec![Series {
        name: Some("'a!b'!Title".into()),
        categories: None,
        values: "'a!b'!$A$2:$A$3".into(),
        bubble_sizes: None,
        color: None,
        secondary: false,
        kind: None,
        quartiles: Default::default(),
        smooth: false,
        trendlines: Vec::new(),
    }];
    let d = resolve(&wb, 0, &c);
    assert_eq!(d.series.first().map(|s| s.name.as_str()), Some("Takings"));
}

#[test]
fn volatile_series_formulas_are_detected() {
    let wb = Workbook::new();
    let with = |values: &str, name: Option<&str>| {
        let mut c = chart(ChartKind::Line);
        c.series.push(Series {
            name: name.map(Into::into),
            categories: None,
            values: values.into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            smooth: false,
            quartiles: Default::default(),
            trendlines: vec![],
        });
        c
    };
    assert!(has_volatile_series(&wb, 0, &with("=NOW()", None)));
    assert!(has_volatile_series(&wb, 0, &with("=SUM(A1:A3)*rand ()", None)));
    assert!(has_volatile_series(&wb, 0, &with("Sheet1!$B$2:$B$9", Some("=TODAY()"))));
    assert!(has_volatile_series(&wb, 0, &with("=RANDARRAY(5)", None)));
    assert!(has_volatile_series(&wb, 0, &with("=_xlfn.RANDARRAY(5)", None)));
    assert!(!has_volatile_series(&wb, 0, &with("Sheet1!$B$2:$B$9", Some("Sheet1!$B$1"))));
    assert!(!has_volatile_series(&wb, 0, &with("=MYRAND(A1)", None)));
    assert!(!has_volatile_series(&wb, 0, &with("=Sheet1!$NOW", None)));
    assert!(!has_volatile_series(&wb, 0, &chart(ChartKind::Line)));
}

#[test]
fn volatile_defined_names_are_followed() {
    let with = |values: &str| {
        let mut c = chart(ChartKind::Line);
        c.series.push(Series {
            name: None,
            categories: None,
            values: values.into(),
            bubble_sizes: None,
            color: None,
            secondary: false,
            kind: None,
            smooth: false,
            quartiles: Default::default(),
            trendlines: vec![],
        });
        c
    };
    let name = |name: &str, scope: Option<usize>, formula: &str| gridcraft_model::DefinedName {
        name: name.into(),
        scope,
        formula: formula.into(),
        comment: String::new(),
        hidden: false,
    };
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().name = "Bob's".into();
    wb.names.push(name("FreshClock", None, "NOW()"));
    wb.names.push(name("Later", None, "FreshClock+1"));
    wb.names.push(name("Dice", Some(0), "RANDBETWEEN(1,6)"));
    wb.names.push(name("Rates", None, "'Bob''s'!$A$1:$A$3"));
    wb.names.push(name("Loop", None, "Loop+1"));
    wb.names.push(name("Spin", None, "Spin+NOW()"));
    wb.names.push(name("Roll", None, "LAMBDA(n,n*RAND())"));
    // A workbook-scoped name, a name used inside another, and a sheet-scoped one, bare or qualified.
    assert!(has_volatile_series(&wb, 0, &with("=FreshClock")));
    assert!(has_volatile_series(&wb, 0, &with("=SUM(Later, 1)")));
    assert!(has_volatile_series(&wb, 0, &with("=Dice*2")));
    assert!(has_volatile_series(&wb, 0, &with("='Bob''s'!Dice")));
    assert!(has_volatile_series(&wb, 0, &with("=Roll(3)")));
    // A self-referencing name ends (it evaluates to an error, not a changing value); one that
    // also calls NOW is volatile.
    assert!(!has_volatile_series(&wb, 0, &with("=Loop")));
    assert!(has_volatile_series(&wb, 0, &with("=Spin")));
    // Many series over the same stable name stay cacheable: each name is followed once.
    let mut many = with("=Rates");
    let first = many.series[0].clone();
    many.series.extend(std::iter::repeat_n(first, 100));
    assert!(!has_volatile_series(&wb, 0, &many));
    // Names nested deeper than the limit are not known to be stable.
    for i in 0..10 {
        wb.names.push(name(&format!("Deep{i}"), None, &format!("Deep{}", i + 1)));
    }
    assert!(has_volatile_series(&wb, 0, &with("=Deep0")));
    // A name that refers to cells, a name inside a string and a name of another scope are stable.
    assert!(!has_volatile_series(&wb, 0, &with("=Rates")));
    assert!(!has_volatile_series(&wb, 0, &with("=\"FreshClock\"")));
    // Dice belongs to sheet 0: seen from another sheet it isn't defined.
    wb.sheets.push(Arc::new(gridcraft_model::Sheet::new("Other")));
    assert!(!has_volatile_series(&wb, 1, &with("=Dice")));
    assert!(has_volatile_series(&wb, 1, &with("=FreshClock")));
}
