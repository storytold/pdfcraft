use super::{
    Ctx, Fields, Fill, Image, Opts, PathMorph, Point, RENDERER, Rect, Region, RenderOpts, Renderer, Rgba, ShapeState, SlideFields, TextMorph,
    crop_dest, dpi_for_scale, gif_frame, image_dpi, images, morph_path, morph_text, needs_filters, parse_path, pasteboard_region, place_slide,
    place_slide_with, render_blend, render_shapes, render_slide, render_slide_drawn, slide_extent, text_transform,
};
use deckcraft_model::chart::{ChartType, Trendline, TrendlineType};
use deckcraft_model::style::ColorRef;
use deckcraft_model::text::TextBody;
use deckcraft_model::{Chart, Geom, PhType, Placeholder, Presentation, Shape, ShapeId, ShapeKind, ShapeStyle, Table, Xfrm};

fn deck_with(shapes: Vec<Shape>) -> Presentation {
    let mut p = Presentation::default();
    let s = std::sync::Arc::make_mut(&mut p.slides[0]);
    s.shapes = shapes;
    p
}

#[test]
fn blank_slide_renders_white_background() {
    let p = Presentation::default();
    let img = render_slide(&p, 0, &RenderOpts { scale: 0.25, ..Default::default() });
    assert_eq!((img.width, img.height), (240, 135));
    assert_eq!(img.pixel(5, 5), [255, 255, 255, 255]);
}

#[test]
fn shape_fill_uses_theme_accent() {
    let sh = Shape {
        id: ShapeId(500),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    };
    let p = deck_with(vec![sh]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let accent = p.masters[0].theme.colors.get(deckcraft_color::SchemeSlot::Accent1);
    let px = img.pixel(200, 150);
    assert!((px[0] as i32 - accent.r as i32).abs() <= 2 && (px[2] as i32 - accent.b as i32).abs() <= 2, "{px:?} vs {accent:?}");
    assert_eq!(img.pixel(50, 50), [255, 255, 255, 255]);
}

/// #832: a quick style on an empty placeholder paints its fill. The style reference
/// (`s.style`) carries the fill, but the empty-placeholder gate skipped the fill whenever the
/// shape had no local fill, so the quick style only appeared once text was added. A
/// placeholder with no style at all still shows nothing but its dashed outline.
#[test]
fn a_quick_style_fills_an_empty_placeholder() {
    let placeholder = || Shape {
        id: ShapeId(910),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        ph: Some(Placeholder { kind: PhType::Body, idx: 1, ..Default::default() }),
        ..Default::default()
    };
    let p = deck_with(vec![Shape { style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)), ..placeholder() }]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, edit: true, ..Default::default() });
    let accent = p.masters[0].theme.colors.get(deckcraft_color::SchemeSlot::Accent1);
    // Lower right of the shape: the prompt text sits at the top of the text rect.
    let px = img.pixel(280, 185);
    assert!(
        (px[0] as i32 - accent.r as i32).abs() <= 2 && (px[1] as i32 - accent.g as i32).abs() <= 2 && (px[2] as i32 - accent.b as i32).abs() <= 2,
        "the quick style's fill is drawn on the empty placeholder: {px:?} vs {accent:?}"
    );
    // An unformatted empty placeholder stays empty: the gate is not simply gone.
    let p = deck_with(vec![placeholder()]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, edit: true, ..Default::default() });
    assert_eq!(img.pixel(280, 185), [255, 255, 255, 255], "an unformatted placeholder has no fill");
}

#[test]
fn render_shapes_draws_the_selection_on_a_transparent_canvas() {
    let sh = Shape {
        id: ShapeId(880),
        xfrm: Some(Xfrm::new(100.0, 80.0, 60.0, 40.0)),
        fill: Some(deckcraft_model::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0)))),
        line: Some(deckcraft_model::Line::none()),
        ..Default::default()
    };
    let p = deck_with(vec![sh]);
    let img = render_shapes(&p, &p.slides[0], &[ShapeId(880)], 1.0).expect("picture");
    let mid = img.pixel(img.width / 2, img.height / 2);
    assert!(mid[0] > 200 && mid[1] < 40 && mid[3] > 200, "{mid:?}");
    assert_eq!(img.pixel(0, 0)[3], 0, "{:?}", img.pixel(0, 0));
}

#[test]
fn ink_pressure_tapers_the_stroke_and_a_flat_half_stays_full_width() {
    let opts = RenderOpts { scale: 1.0, ..Default::default() };
    let tapered = render_slide(&ink_line(1.0, 0.12), 0, &opts);
    let flat = render_slide(&ink_line(0.5, 0.5), 0, &opts);
    let full = render_slide(&ink_line(1.0, 1.0), 0, &opts);
    let heavy = dark_in(&tapered, 50, 140, 70, 130);
    let light = dark_in(&tapered, 280, 360, 70, 130);
    assert!(heavy > light * 3, "a firm point covers more of the slide than a light one ({heavy} vs {light})");
    let (a, b) = (dark_in(&flat, 40, 370, 70, 130), dark_in(&full, 40, 370, 70, 130));
    assert!(a > 100 && (a as i32 - b as i32).unsigned_abs() * 10 < a as u32, "the old constant 0.5 still draws at the pen width ({a} vs {b})");
}

fn ink_line(left: f32, right: f32) -> Presentation {
    let points = (0..=40)
        .map(|i| {
            let pressure = if i < 20 { left } else { right };
            (40.0 + f64::from(i) * 8.0, 100.0, pressure)
        })
        .collect();
    let sh = Shape {
        id: ShapeId(7),
        xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)),
        kind: ShapeKind::Ink {
            strokes: vec![deckcraft_model::InkStroke { points, color: deckcraft_color::Rgba::rgb(0, 0, 0), width: 24.0, highlighter: false }],
        },
        ..Default::default()
    };
    deck_with(vec![sh])
}

fn dark_in(img: &Image, x0: u32, x1: u32, y0: u32, y1: u32) -> usize {
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if img.pixel(x, y)[0] < 200 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn text_draws_ink() {
    let mut sh = Shape { id: ShapeId(501), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), text_box: true, ..Default::default() };
    let mut t = TextBody::from_text("HELLO WORLD");
    t.paragraphs[0].runs[0].props.size = Some(120.0);
    sh.text = Some(t);
    let p = deck_with(vec![sh]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 0.5, ..Default::default() });
    let dark = img.pixels.as_chunks::<4>().0.iter().filter(|px| px[0] < 128).count();
    assert!(dark > 500, "dark pixels {dark}");
}

#[test]
fn every_preset_renders_without_panic() {
    let shapes: Vec<Shape> = deckcraft_geom::preset::CATALOG
        .iter()
        .enumerate()
        .map(|(i, pr)| Shape {
            id: ShapeId(1000 + i as u32),
            xfrm: Some(Xfrm::new((i % 16) as f64 * 60.0, (i / 16) as f64 * 55.0, 50.0, 45.0)),
            geom: Geom::preset(pr.name),
            style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent2)),
            ..Default::default()
        })
        .collect();
    let p = deck_with(shapes);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, edit: true, ..Default::default() });
    assert_eq!(img.width, 960);
}

#[test]
fn effects_tables_charts_render() {
    let mut sh = Shape {
        id: ShapeId(600),
        xfrm: Some(Xfrm::new(50.0, 50.0, 200.0, 100.0)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    };
    sh.effects = Some(deckcraft_model::Effects {
        outer_shadow: Some(deckcraft_model::style::Shadow {
            color: ColorRef::rgb(deckcraft_color::Rgba::BLACK),
            blur: 8.0,
            dist: 6.0,
            dir: 45.0,
            inner: false,
            sx: 1.0,
            sy: 1.0,
            kx: 0.0,
            ky: 0.0,
            align: String::new(),
            rotate_with_shape: false,
        }),
        glow: Some(deckcraft_model::style::Glow { color: ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 200, 0)), radius: 6.0 }),
        soft_edge: Some(4.0),
        ..Default::default()
    });
    let mut t = Table::new(3, 3, 300.0, 30.0);
    t.cell_mut(0, 0).unwrap().text = TextBody::from_text("Head");
    let tbl = Shape { id: ShapeId(601), xfrm: Some(Xfrm::new(300.0, 50.0, 300.0, 90.0)), kind: ShapeKind::Table(t), ..Default::default() };
    let mut charts = vec![];
    for (i, k) in [
        ChartType::Column,
        ChartType::Bar,
        ChartType::Line,
        ChartType::Pie,
        ChartType::Doughnut,
        ChartType::Area,
        ChartType::Scatter,
        ChartType::StackedColumn,
    ]
    .iter()
    .enumerate()
    {
        charts.push(Shape {
            id: ShapeId(700 + i as u32),
            xfrm: Some(Xfrm::new(i as f64 * 110.0, 300.0, 100.0, 100.0)),
            kind: ShapeKind::Chart(Box::new(Chart::sample(*k))),
            ..Default::default()
        });
    }
    let mut all = vec![sh, tbl];
    all.extend(charts);
    let p = deck_with(all);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    // Shadow darkens below-right of the shape (grey, past the glow), blurred by a third of its
    // 8 pt blur radius.
    let px = img.pixel(255, 155);
    assert!(px[0] < 240 && px[0] == px[2], "{px:?}");
}

#[test]
fn combo_chart_draws_columns_and_a_line_on_its_own_axis() {
    // Columns in the hundreds of thousands, a green line in the thousands on the secondary axis:
    // the line must sit up in the plot, not flat along the bottom of the primary scale.
    let mut c = Chart::sample(ChartType::Combo);
    c.title = None;
    c.legend = None;
    c.series.truncate(2);
    c.categories = vec!["A".into(), "B".into(), "C".into()];
    c.series[0].kind = Some(ChartType::Column);
    c.series[0].values = vec![Some(200_000.0), Some(300_000.0), Some(250_000.0)];
    c.series[1].kind = Some(ChartType::Line);
    c.series[1].values = vec![Some(7_000.0), Some(7_000.0), Some(7_000.0)];
    c.series[1].secondary_axis = true;
    c.series[1].line = Some(deckcraft_model::style::Line::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 176, 80)), 3.0));
    let sh = Shape { id: ShapeId(620), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
    let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let green = |x: u32, y0: u32, y1: u32| {
        (y0..y1).any(|y| {
            let p = img.pixel(x, y);
            p[1] > 120 && p[0] < 80 && p[2] < 140
        })
    };
    // The line is in the upper half (7 000 of an axis to 8 000), and columns fill the plot's bottom.
    assert!(green(480, 20, 270), "line not in the upper half");
    assert!(!green(480, 400, 500), "line along the bottom");
    let col = img.pixel(480, 480);
    assert!(col[0] < 250 || col[1] < 250 || col[2] < 250, "no column: {col:?}");
}

fn trend_chart(title: &str, kind: ChartType, ys: &[f64], trend: Trendline, y_max: f64) -> Chart {
    let mut c = Chart::sample(kind);
    c.title = Some(title.into());
    c.legend = None;
    c.categories = vec!["A".into(), "B".into(), "C".into(), "D".into()];
    c.series.truncate(1);
    c.series[0].name = "Data".into();
    c.series[0].values = ys.iter().copied().map(Some).collect();
    c.y_min = Some(0.0);
    c.y_max = Some(y_max);
    c.series[0].trendlines = vec![trend];
    c
}

fn trend_line(kind: TrendlineType) -> Trendline {
    Trendline { kind, disp_eq: true, disp_r_sqr: true, ..Default::default() }
}

/// Six charts, one trendline type each. `draw` drops the trendlines. `forecast` extends the linear fit.
fn trend_gallery(draw: bool, forecast: bool) -> Image {
    let mut linear = trend_line(TrendlineType::Linear);
    if forecast {
        linear.forward = Some(1.0);
        linear.backward = Some(0.5);
    }
    let mut poly = trend_line(TrendlineType::Poly);
    poly.order = Some(2);
    let mut ma = trend_line(TrendlineType::MovingAvg);
    ma.period = Some(2);
    let mut charts = vec![
        trend_chart("Linear", ChartType::Column, &[3.0, 5.0, 7.0, 9.0], linear, 12.0),
        trend_chart("Exponential", ChartType::LineMarkers, &[1.0, 2.0, 4.0, 8.0], trend_line(TrendlineType::Exp), 10.0),
        trend_chart(
            "Logarithmic",
            ChartType::LineMarkers,
            &[1.0_f64.ln(), 2.0_f64.ln(), 3.0_f64.ln(), 4.0_f64.ln()],
            trend_line(TrendlineType::Log),
            2.0,
        ),
        trend_chart("Polynomial", ChartType::LineMarkers, &[1.0, 4.0, 9.0, 16.0], poly, 18.0),
        trend_chart("Power", ChartType::LineMarkers, &[2.0, 2.0 * 2.0_f64.sqrt(), 2.0 * 3.0_f64.sqrt(), 4.0], trend_line(TrendlineType::Power), 6.0),
        trend_chart("Moving average", ChartType::LineMarkers, &[2.0, 8.0, 4.0, 10.0], ma, 12.0),
    ];
    if !draw {
        for c in &mut charts {
            c.series[0].trendlines.clear();
        }
    }
    let shapes = charts
        .into_iter()
        .enumerate()
        .map(|(i, c)| Shape {
            id: ShapeId(800 + i as u32),
            xfrm: Some(Xfrm::new(12.0 + (i % 3) as f64 * 316.0, 12.0 + (i / 3) as f64 * 264.0, 304.0, 252.0)),
            kind: ShapeKind::Chart(Box::new(c)),
            ..Default::default()
        })
        .collect();
    render_slide(&deck_with(shapes), 0, &RenderOpts { scale: 1.0, ..Default::default() })
}

#[test]
fn trendlines_draw_through_the_points_with_a_label() {
    let on = trend_gallery(true, false);
    let off = trend_gallery(false, false);
    assert_eq!((on.width, on.height), (960, 540));
    let dark = on
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(off.pixels.as_chunks::<4>().0)
        .filter(|(a, b)| a[0] < 50 && a[1] < 50 && a[2] < 50 && b[0] as i32 - a[0] as i32 > 20)
        .count();
    assert!(dark > 40, "dashed trendline ink {dark}");
    // A scatter fit uses the real x values and still paints the dash.
    let mut scatter = trend_chart("Scatter", ChartType::Scatter, &[2.0, 4.0, 6.0, 8.0], trend_line(TrendlineType::Linear), 10.0);
    scatter.series[0].x_values = (1..=4).map(|x| Some(x as f64)).collect();
    let sh = Shape {
        id: ShapeId(810),
        xfrm: Some(Xfrm::new(0.0, 0.0, 480.0, 360.0)),
        kind: ShapeKind::Chart(Box::new(scatter.clone())),
        ..Default::default()
    };
    let with = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    scatter.series[0].trendlines.clear();
    let sh =
        Shape { id: ShapeId(810), xfrm: Some(Xfrm::new(0.0, 0.0, 480.0, 360.0)), kind: ShapeKind::Chart(Box::new(scatter)), ..Default::default() };
    let without = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let scatter_dark = with
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(without.pixels.as_chunks::<4>().0)
        .filter(|(a, b)| a[0] < 50 && a[1] < 50 && a[2] < 50 && b[0] as i32 - a[0] as i32 > 20)
        .count();
    assert!(scatter_dark > 20, "scatter trendline ink {scatter_dark}");
    if std::env::var_os("DECKCRAFT_TREND_SHOT").is_some() {
        let png = trend_gallery(true, true).to_png();
        std::fs::write("/tmp/deckcraft-trendlines.png", &png).expect("write trendline screenshot");
    }
}

#[test]
fn chart_axis_titles_are_drawn() {
    let draw = |titles: (Option<String>, Option<String>)| {
        let mut c = Chart::sample(ChartType::Column);
        c.title = None;
        c.legend = None;
        c.axis_titles = titles;
        let sh = Shape { id: ShapeId(624), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let ink = |img: &Image, r: (u32, u32, u32, u32)| {
        (r.0..r.2).flat_map(|x| (r.1..r.3).map(move |y| (x, y))).filter(|(x, y)| img.pixel(*x, *y)[0] < 160).count()
    };
    let bare = draw((None, None));
    let titled = draw((Some("Month".into()), Some("Revenue".into())));
    // A turned "Revenue" down the left edge, "Month" along the bottom.
    let left = (38, 150, 62, 390);
    let bottom = (380, 490, 580, 520);
    assert!(ink(&titled, left) > ink(&bare, left) + 30, "value axis title");
    assert!(ink(&titled, bottom) > ink(&bare, bottom) + 30, "category axis title");
}

#[test]
fn column_series_draw_their_pattern_fill_and_no_fill_with_their_outline() {
    // Three column series, as PowerPoint draws them: a red-on-yellow pattern, no fill with a blue
    // outline, and no fill at all. None takes a palette colour; the legend keys match the bars.
    use deckcraft_model::style::{Fill, PatternFill};
    let rgb = |r, g, b| ColorRef::rgb(deckcraft_color::Rgba::rgb(r, g, b));
    let mut c = Chart::sample(ChartType::Column);
    c.title = None;
    c.series.truncate(3);
    c.categories = vec!["A".into()];
    for s in &mut c.series {
        s.values = vec![Some(10.0)];
    }
    c.series[0].fill = Some(Fill::Pattern(PatternFill { preset: "dkDnDiag".into(), fg: rgb(255, 0, 0), bg: rgb(255, 255, 0) }));
    c.series[1].fill = Some(Fill::None);
    c.series[1].line = Some(deckcraft_model::style::Line { width: Some(3.0), fill: Some(Fill::solid(rgb(0, 0, 255))), ..Default::default() });
    c.series[2].fill = Some(Fill::None);
    let sh =
        Shape { id: ShapeId(625), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c.clone())), ..Default::default() };
    let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let count =
        |f: &dyn Fn([u8; 4]) -> bool| (0..img.width).flat_map(|x| (0..img.height).map(move |y| (x, y))).filter(|(x, y)| f(img.pixel(*x, *y))).count();
    let red = count(&|p| p[0] > 220 && p[1] < 40 && p[2] < 40);
    let yellow = count(&|p| p[0] > 220 && p[1] > 220 && p[2] < 40);
    let blue = count(&|p| p[0] < 40 && p[1] < 40 && p[2] > 220);
    assert!(red > 1000 && yellow > 1000, "the pattern's two colours: {red} red, {yellow} yellow");
    assert!(blue > 300, "the empty series' outline: {blue}");
    // No palette colour: the theme's accents would show for any series drawn the old way.
    let p = deck_with(vec![]);
    let ctx = deckcraft_model::resolve::Ctx::for_slide(&p, &p.slides[0]).unwrap();
    for i in 0..3 {
        let a = crate::chart::series_color(&ctx, &c, i);
        let n = count(&|q| q[0].abs_diff(a.r) < 6 && q[1].abs_diff(a.g) < 6 && q[2].abs_diff(a.b) < 6);
        assert!(n < 50, "palette colour {i} drawn on {n} pixels");
    }
}

#[test]
fn column_points_take_their_own_fill_and_smooth_lines_curve() {
    // One column series where only the middle point is given a red c:dPt fill.
    let mut c = Chart::sample(ChartType::Column);
    c.title = None;
    c.legend = None;
    c.series.truncate(1);
    c.categories = vec!["A".into(), "B".into(), "C".into()];
    c.series[0].values = vec![Some(10.0), Some(10.0), Some(10.0)];
    c.series[0].point_fills = vec![(1, deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0))))];
    let sh = Shape { id: ShapeId(621), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
    let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let red = |x: u32| (300..500).any(|y| img.pixel(x, y)[..3] == [255, 0, 0]);
    assert!(red(480), "the middle column takes its point's fill");
    assert!(!red(200) && !red(760), "the others keep the series colour");

    // Two series, gap 150 %, no overlap: each bar is a band / 3.5 wide (PowerPoint's and
    // LibreOffice's geometry), not band / 2.5 split in two.
    let mut c = Chart::sample(ChartType::Column);
    c.title = None;
    c.legend = None;
    c.series.truncate(2);
    c.categories = vec!["A".into()];
    c.series[0].values = vec![Some(10.0)];
    c.series[1].values = vec![Some(10.0)];
    c.series[0].fill = Some(deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0))));
    (c.gap_width, c.overlap) = (1.5, 0.0);
    let sh = Shape { id: ShapeId(622), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
    let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let y = (0..540).rev().find(|y| (0..960).any(|x| img.pixel(x, *y)[..3] == [255, 0, 0])).unwrap() - 5;
    let reds = (0..960).filter(|x| img.pixel(*x, y)[..3] == [255, 0, 0]).count() as f64;
    // The plot is ~850 px wide with one category: a bar is ~240 px (it was ~170 when the band
    // was split as 1 + gap and then shared between the series).
    assert!((200.0..280.0).contains(&reds), "bar {reds} px");

    // A smoothed line through a peak overshoots no point but leaves the straight chord.
    let pts = [kurbo::Point::new(0.0, 0.0), kurbo::Point::new(10.0, 10.0), kurbo::Point::new(20.0, 0.0)];
    let p = crate::chart::smooth_path(&pts);
    assert_eq!(p.elements().len(), 3, "a cubic per segment");
    let mid = kurbo::ParamCurve::eval(&p.segments().next().unwrap(), 0.5);
    assert!(mid.y > mid.x, "the first half bows towards the peak instead of following the chord: {mid:?}");
}

#[test]
fn horizontal_bars_list_the_first_category_at_the_bottom_unless_reversed() {
    let draw = |reversed: bool| {
        let mut c = Chart::sample(ChartType::Bar);
        c.title = None;
        c.legend = None;
        c.series.truncate(1);
        c.categories = vec!["A".into(), "B".into(), "C".into()];
        c.series[0].values = vec![Some(10.0), Some(10.0), Some(10.0)];
        c.series[0].point_fills = vec![(0, deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0))))];
        c.cat_reversed = reversed;
        let sh = Shape { id: ShapeId(623), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
        let rows: Vec<u32> = (0..540).filter(|y| (0..960).any(|x| img.pixel(x, *y)[..3] == [255, 0, 0])).collect();
        (rows[0] + rows[rows.len() - 1]) / 2
    };
    assert!(draw(false) > 360, "first category at the bottom: {}", draw(false));
    assert!(draw(true) < 180, "reversed: first category at the top: {}", draw(true));
}

#[test]
fn glow_of_an_unfilled_shape_follows_its_line() {
    // A frame (no fill, thin line) with a strong glow: the glow lines its edge, the inside stays
    // the slide's white, as PowerPoint draws it.
    let glow = |fill: Option<Fill>| {
        let mut sh = Shape {
            id: ShapeId(610),
            xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 300.0)),
            fill,
            line: Some(deckcraft_model::style::Line::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 176, 240)), 0.25)),
            ..Default::default()
        };
        sh.effects = Some(deckcraft_model::Effects {
            glow: Some(deckcraft_model::style::Glow { color: ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 255)), radius: 8.0 }),
            ..Default::default()
        });
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let img = glow(Some(Fill::None));
    let inside = img.pixel(300, 250);
    assert!(inside[0] > 245 && inside[1] > 245, "{inside:?}");
    let edge = img.pixel(300, 97);
    assert!(edge[0] < 230, "{edge:?}");
    // A filled shape's glow is still the whole silhouette's (the fill covers its inside).
    let img = glow(Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 255, 255)))));
    assert!(img.pixel(300, 94)[0] < 240);
}

#[test]
fn glow_of_a_shape_with_no_fill_and_no_line_is_not_drawn() {
    // A text box (no fill, no line) with a glow in its shape properties: PowerPoint shows no halo
    // round the box (its text carries its own glow), so the box's edge and surroundings stay white.
    let mut sh = Shape { id: ShapeId(611), xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 300.0)), fill: Some(Fill::None), ..Default::default() };
    sh.effects = Some(deckcraft_model::Effects {
        glow: Some(deckcraft_model::style::Glow { color: ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 255, 0)), radius: 15.0 }),
        ..Default::default()
    });
    let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    for (x, y) in [(300, 250), (300, 100), (300, 95), (96, 250)] {
        let px = img.pixel(x, y);
        assert!(px[2] > 245, "({x}, {y}): {px:?}");
    }
}

#[test]
fn shadow_and_glow_keep_an_even_odd_shapes_hole() {
    // #1596: a donut is one even-odd subpath — its hole exists only under the
    // even-odd rule. The effects were cast from a merged silhouette filled
    // NonZero, so the shadow and the glow filled the hole solid. The hole's
    // centre must stay the slide's white in both, while the effects still draw.
    let donut = |effects: deckcraft_model::Effects| {
        let mut sh = Shape {
            id: ShapeId(612),
            xfrm: Some(Xfrm::new(50.0, 50.0, 200.0, 200.0)),
            geom: Geom::preset("donut"),
            fill: Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 160, 0)))),
            ..Default::default()
        };
        sh.effects = Some(effects);
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let img = donut(deckcraft_model::Effects {
        outer_shadow: Some(deckcraft_model::style::Shadow {
            color: ColorRef::rgb(deckcraft_color::Rgba::BLACK),
            blur: 8.0,
            dist: 0.0,
            dir: 45.0,
            inner: false,
            sx: 1.0,
            sy: 1.0,
            kx: 0.0,
            ky: 0.0,
            align: String::new(),
            rotate_with_shape: false,
        }),
        ..Default::default()
    });
    let hole = img.pixel(150, 150);
    assert!(hole[0] > 245 && hole[1] > 245 && hole[2] > 245, "shadow in the hole: {hole:?}");
    // The shadow still falls around the shape (just below its bottom edge).
    let below = img.pixel(150, 251);
    assert!(below[0] < 200 && below[0] == below[2], "no shadow below: {below:?}");

    let img = donut(deckcraft_model::Effects {
        glow: Some(deckcraft_model::style::Glow { color: ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0)), radius: 8.0 }),
        ..Default::default()
    });
    let hole = img.pixel(150, 150);
    assert!(hole[0] > 245 && hole[1] > 245 && hole[2] > 245, "glow in the hole: {hole:?}");
    // The ring itself is the shape's orange.
    let ring = img.pixel(150, 75);
    assert!(ring[0] > 200 && ring[1] > 100 && ring[2] < 100, "ring: {ring:?}");
}

#[test]
fn placeholder_prompts_only_in_edit_view() {
    let p = Presentation::default();
    let edit = render_slide(&p, 0, &RenderOpts { scale: 0.5, edit: true, ..Default::default() });
    let show = render_slide(&p, 0, &RenderOpts { scale: 0.5, ..Default::default() });
    let ink = |i: &Image| i.pixels.as_chunks::<4>().0.iter().filter(|px| px[0] < 200).count();
    assert!(ink(&edit) > 100);
    assert_eq!(ink(&show), 0);
}

#[test]
fn png_encodes() {
    let p = Presentation::default();
    let img = render_slide(&p, 0, &RenderOpts { scale: 0.1, ..Default::default() });
    let png = img.to_png();
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    assert_eq!(image_dpi(&png), None, "an untagged screenshot stays without a resolution");
    assert!(!img.to_jpeg(80).is_empty());
}

#[test]
fn a_retina_export_records_144_dpi_and_still_decodes() {
    assert_eq!(dpi_for_scale(2.0), 144.0);
    assert_eq!(dpi_for_scale(1.0), 72.0);
    assert_eq!(dpi_for_scale(f64::NAN), 72.0);
    let img = Image { width: 4, height: 2, pixels: [255, 0, 0, 255].repeat(8) };
    let png = img.to_png_at(144.0);
    assert_eq!(image_dpi(&png), Some((144.0, 144.0)));
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (4, 2));
    let again = images::stamp_png_dpi(&png, 72.0);
    assert_eq!(again.len(), png.len(), "a second pHYs chunk is not written");
    assert_eq!(image_dpi(&again), Some((144.0, 144.0)));
    let jpeg = img.to_jpeg_at(90, Some(144.0));
    assert_eq!(image_dpi(&jpeg), Some((144.0, 144.0)));
    assert_eq!(image_dpi(&img.to_jpeg(90)), None);
}

#[test]
fn crop_dest_math() {
    let r = crop_dest(Rect::new(0.0, 0.0, 100.0, 100.0), [0.5, 0.0, 0.0, 0.0]);
    assert!((r.x0 + 100.0).abs() < 1e-9 && (r.width() - 200.0).abs() < 1e-9);
    let r = crop_dest(Rect::new(0.0, 0.0, 100.0, 100.0), [f64::NAN, 2.0, -50.0, 0.0]);
    assert!(r.x0.is_finite() && r.y1.is_finite());
}

#[test]
fn custom_path_parse() {
    let p = parse_path("M 0 0 L 10 0 C 1 2 3 4 5 6 Q 1 1 2 2 Z garbage L x y");
    assert!(p.elements().len() >= 5);
    assert!(parse_path("").elements().is_empty());
}

#[test]
fn table_rows_of_zero_height_grow_to_fit_text() {
    // pandoc and python-pptx write `<a:tr h="0">`: the stored row height is only a minimum.
    let mut t = Table::new(3, 2, 300.0, 0.0);
    for (r, row) in [["Col A", "Col B"], ["1", "alpha"], ["2", "beta"]].iter().enumerate() {
        for (c, text) in row.iter().enumerate() {
            t.cell_mut(r, c).unwrap().text = TextBody::from_text(text);
        }
    }
    let tbl = Shape { id: ShapeId(610), xfrm: Some(Xfrm::new(100.0, 100.0, 300.0, 0.0)), kind: ShapeKind::Table(t.clone()), ..Default::default() };
    let p = deck_with(vec![tbl]);
    let ctx = deckcraft_model::resolve::Ctx::for_slide(&p, &p.slides[0]).unwrap();
    let hs = crate::table::row_heights(&ctx, &t, &deckcraft_text::NoFields);
    assert_eq!(hs.len(), 3);
    assert!(hs.iter().all(|h| *h > 10.0), "{hs:?}");
    let tops: Vec<f64> = crate::table::cell_rects(&t, &hs).iter().filter(|(_, c, _)| *c == 0).map(|(_, _, r)| r.y0).collect();
    assert!(tops.len() == 3 && tops.windows(2).all(|w| w[1] > w[0] + 10.0), "{tops:?}");
    // A taller stored height is kept.
    let mut tall = t;
    tall.rows[1].height = 200.0;
    assert_eq!(crate::table::row_heights(&ctx, &tall, &deckcraft_text::NoFields)[1], 200.0);
    // The PDF text layer gets every cell's text, rows one below the other.
    let placed = place_slide(&p, 0);
    let top = |s: &str| {
        let pt = placed.texts.iter().find(|pt| pt.body.text() == s).unwrap_or_else(|| panic!("no placed text {s:?}"));
        (pt.transform * kurbo::Point::new(0.0, pt.layout.lines[0].top)).y
    };
    assert!(top("Col A") < top("1") && top("1") < top("2"));
    assert!((top("alpha") - top("1")).abs() < 1.0 && (top("beta") - top("2")).abs() < 1.0);
    assert!(top("Col B") >= 100.0);
}

#[test]
fn blend_ends_match_the_slides_and_mixes_backdrops() {
    let sh = Shape {
        id: ShapeId(500),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    };
    let mut p = deck_with(vec![sh.clone()]);
    let mut dark = (*p.slides[0]).clone();
    dark.background =
        Some(deckcraft_model::Background::Fill { fill: deckcraft_model::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0))) });
    p.slides.push(std::sync::Arc::new(dark));
    let o = RenderOpts { scale: 0.5, ..Default::default() };
    let shapes = vec![(sh, false)];
    // At the ends the frame is the slide itself; the shape stays opaque throughout.
    assert_eq!(render_blend(&p, (0, 1), 1.0, &shapes, &[], &[], &o).pixels, render_slide(&p, 1, &o).pixels);
    let mid = render_blend(&p, (0, 1), 0.5, &shapes, &[], &[], &o);
    let px = mid.pixel(5, 5);
    assert!((px[0] as i32 - 128).abs() <= 3, "{px:?}");
    assert_eq!(mid.pixel(100, 75), render_slide(&p, 1, &o).pixel(100, 75));
    // Hostile input: bad indices and NaN never panic.
    assert_eq!(render_blend(&p, (0, 9), 0.5, &shapes, &[], &[], &o).width, 0);
    let _ = render_blend(&p, (0, 1), f64::NAN, &shapes, &[], &[], &o);
}

fn text_shape(id: u32, text: &str, x: f64, y: f64) -> Shape {
    let mut t = TextBody::from_text(text);
    for p in &mut t.paragraphs {
        for r in &mut p.runs {
            r.props.size = Some(40.0);
        }
    }
    Shape { id: ShapeId(id), xfrm: Some(Xfrm::new(x, y, 600.0, 120.0)), text_box: true, text: Some(t), ..Default::default() }
}

#[test]
fn morph_text_tokens_are_words_or_characters() {
    let p = Presentation::default();
    let rctx = Ctx::for_slide(&p, &p.slides[0]).unwrap();
    let s = text_shape(1, "Hello big  world\nHello", 0.0, 0.0);
    let l = deckcraft_text::layout(
        &rctx,
        &s,
        s.text.as_ref().unwrap(),
        &Opts { rect: Rect::new(0.0, 0.0, 600.0, 120.0), fields: &deckcraft_text::NoFields, prompt_color: None, no_shrink: false },
    );
    assert_eq!(morph_text::token_keys(&l, false), ["Hello", "big", "world", "Hello"]);
    assert_eq!(morph_text::token_keys(&l, true).concat(), "Hellobigworld".to_string() + "Hello");
    assert_eq!(morph_text::token_keys(&l, true).len(), 18);
}

#[test]
fn morph_text_ends_match_the_slides() {
    let from = Xfrm::new(100.0, 60.0, 600.0, 120.0);
    let to = Xfrm::new(200.0, 300.0, 600.0, 120.0);
    let mut p = deck_with(vec![text_shape(1, "Hello wide world", from.x, from.y)]);
    let mut b = (*p.slides[0]).clone();
    b.shapes = vec![text_shape(1, "world says Hello", to.x, to.y)];
    p.slides.push(std::sync::Arc::new(b));
    let o = RenderOpts { scale: 0.5, ..Default::default() };
    let frame = |t: f64| {
        let x = Xfrm::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t, 600.0, 120.0);
        let mut sb = text_shape(10, "world says Hello", 0.0, 0.0);
        let mut sa = text_shape(11, "Hello wide world", 0.0, 0.0);
        sb.xfrm = Some(x);
        sa.xfrm = Some(x);
        let tm = TextMorph { old: ShapeId(11), new: ShapeId(10), from, to, chars: false, t };
        render_blend(&p, (0, 1), t, &[(sb, false), (sa, true)], &[tm], &[], &o)
    };
    // Warm up: faces load lazily (a background scan runs), so the first render may still fall back.
    let _ = (render_slide(&p, 0, &o), render_slide(&p, 1, &o), frame(0.5));
    let close = |a: &Image, b: &Image| a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x.abs_diff(**y) > 8).count();
    let d0 = close(&frame(0.0), &render_slide(&p, 0, &o));
    assert!(d0 < 40, "start differs {d0}");
    let d1 = close(&frame(1.0), &render_slide(&p, 1, &o));
    assert!(d1 < 40, "end differs {d1}");
    // Halfway the shared words are on their way: ink between the two texts' rows.
    let mid = frame(0.5);
    let ink = (95..140).flat_map(|y| (0..480).map(move |x| (x, y))).filter(|&(x, y)| mid.pixel(x, y)[0] < 128).count();
    assert!(ink > 50, "ink {ink}");
}

/// A text box holding one run of `kind` (a field type, or plain text for `None`) saved as `saved`.
fn field_shape(kind: Option<&str>, saved: &str) -> Shape {
    let mut sh = text_shape(1, saved, 40.0, 40.0);
    if let (Some(k), Some(run)) = (kind, sh.text.as_mut().and_then(|t| t.paragraphs[0].runs.first_mut())) {
        run.kind = deckcraft_model::text::RunKind::Field { field: k.into() };
    }
    sh
}

#[test]
fn date_fields_show_the_date_in_their_format() {
    use deckcraft_text::datetime::DateTime;
    let fri = DateTime::new(2026, 10, 9, 20, 5, 7);
    let at = |p: &Presentation, now: DateTime| render_slide(p, 0, &RenderOpts { scale: 0.5, now: Some(now), ..Default::default() });
    let mut p = deck_with(vec![field_shape(Some("datetime1"), "1/1/2000")]);
    let _ = at(&p, fri); // faces load lazily
    // The field shows the date (it changes with the clock), not its saved text.
    let a = at(&p, fri);
    assert!(diff_px(&a, &at(&p, DateTime::new(2027, 3, 4, 0, 0, 0))) > 0);
    let shown = deck_with(vec![field_shape(None, "10/9/2026")]);
    assert_eq!(diff_px(&a, &at(&shown, fri)), 0, "datetime1 is M/D/YYYY");
    // Another slide's fixed date (a deck-wide Header & Footer text) doesn't replace it.
    p.header_footer.date_text = "Fixed on another slide".into();
    assert_eq!(diff_px(&a, &at(&p, fri)), 0);
    // Every type has its format; the run's language orders numeric dates.
    let f = Renderer::fields(&p, 0, &RenderOpts { now: Some(fri), ..Default::default() });
    assert_eq!(f.field_in("datetime4", Some("en-US")).as_deref(), Some("October 9, 2026"));
    assert_eq!(f.field_in("datetimeFigureOut", Some("en-GB")).as_deref(), Some("09/10/2026"));
    assert_eq!(f.field("datetime13").as_deref(), Some("8:05:07 PM"));
    assert_eq!(f.field("slidenum").as_deref(), Some("1"));
    // A date the app can't compute (no clock, unknown type, other-language names) keeps the saved text.
    assert_eq!(SlideFields { now: None, ..f }.field("datetime1"), None);
    let unknown = deck_with(vec![field_shape(Some("datetime99"), "Saved text")]);
    let plain = deck_with(vec![field_shape(None, "Saved text")]);
    assert_eq!(diff_px(&at(&unknown, fri), &at(&plain, fri)), 0);
    // An explicit override still wins.
    let o = RenderOpts { date_text: Some("Today".into()), now: Some(fri), ..Default::default() };
    assert_eq!(Renderer::fields(&p, 0, &o).field("datetime2").as_deref(), Some("Today"));
}

/// Channel values that differ noticeably between two images of the same size.
fn diff_px(a: &Image, b: &Image) -> usize {
    a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x.abs_diff(**y) > 64).count()
}

/// Two 8×8 frames, red then blue, 100 ms each.
fn two_frame_gif() -> Vec<u8> {
    use image::{Delay, Frame, Rgba, RgbaImage};
    let frame = |c: [u8; 4]| Frame::from_parts(RgbaImage::from_pixel(8, 8, Rgba(c)), 0, 0, Delay::from_numer_denom_ms(100, 1));
    let mut out = Vec::new();
    image::codecs::gif::GifEncoder::new(&mut out).encode_frames([frame([255, 0, 0, 255]), frame([0, 0, 255, 255])]).expect("gif");
    out
}

#[test]
fn animated_gif_shows_the_frame_for_its_time() {
    let mut p = Presentation::default();
    let media = p.add_media("a.gif", "image/gif", two_frame_gif()).expect("media");
    let fill = deckcraft_model::style::PictureFill { media, ..Default::default() };
    let sh = Shape { id: ShapeId(600), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Picture { fill }, ..Default::default() };
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
    let at = |t: Option<f64>| {
        let times: Vec<(ShapeId, f64)> = t.map(|t| vec![(ShapeId(600), t)]).unwrap_or_default();
        render_slide(&p, 0, &RenderOpts { scale: 0.1, gif_times: &times, ..Default::default() }).pixel(48, 27)
    };
    let red = |px: [u8; 4]| px[0] > 200 && px[2] < 60;
    let blue = |px: [u8; 4]| px[2] > 200 && px[0] < 60;
    assert!(red(at(None)), "no time: the first frame, as exports draw it");
    assert!(red(at(Some(0.05))));
    assert!(blue(at(Some(0.15))));
    assert!(red(at(Some(0.25))), "it loops");
    let bytes = p.media(media).map(|m| m.data.clone()).expect("media");
    let adj = Default::default();
    assert_eq!(gif_frame(&bytes, &adj, 0.15).map(|f| f.0), Some(1));
    assert_eq!(gif_frame(&bytes, &adj, f64::NAN).map(|f| f.0), Some(0));
    assert!(gif_frame(&std::sync::Arc::new(b"\x89PNG".to_vec()), &adj, 0.0).is_none());
}

#[test]
fn morph_path_ends_match_the_slides() {
    let from = Xfrm::new(100.0, 100.0, 200.0, 200.0);
    let to = Xfrm::new(500.0, 150.0, 300.0, 150.0);
    let form = |id: u32, g: &str, x: Xfrm| Shape {
        id: ShapeId(id),
        xfrm: Some(x),
        geom: Geom::preset(g),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    };
    let mut p = deck_with(vec![form(1, "ellipse", from)]);
    let mut b = (*p.slides[0]).clone();
    b.shapes = vec![form(1, "star5", to)];
    p.slides.push(std::sync::Arc::new(b));
    let o = RenderOpts { scale: 0.5, ..Default::default() };
    let frame = |t: f64| {
        let lerp = |a: f64, b: f64| a + (b - a) * t;
        let x = Xfrm::new(lerp(from.x, to.x), lerp(from.y, to.y), lerp(from.w, to.w), lerp(from.h, to.h));
        let pm = PathMorph { id: ShapeId(10), from: Geom::preset("ellipse"), from_box: from, to_box: to, t };
        render_blend(&p, (0, 1), t, &[(form(10, "star5", x), false)], &[], &[pm], &o)
    };
    // The morph splits the outlines into more curves, which flatten slightly differently: edge pixels
    // may differ a little, a different shape differs a lot.
    let differ = |a: &Image, b: &Image| a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x.abs_diff(**y) > 60).count();
    // At the start the star is still the circle, at the end the star.
    let d0 = differ(&frame(0.0), &render_slide(&p, 0, &o));
    assert!(d0 < 40, "start differs {d0}");
    let d1 = differ(&frame(1.0), &render_slide(&p, 1, &o));
    assert!(d1 < 40, "end differs {d1}");
    // Without the morph the start would show the star.
    let star = render_blend(&p, (0, 1), 0.0, &[(form(10, "star5", from), false)], &[], &[], &o);
    assert!(differ(&star, &render_slide(&p, 0, &o)) > 400);
    // Halfway the shape is filled around the middle of its box.
    let mid = frame(0.5);
    let c = (from.center().to_vec2() + to.center().to_vec2()) * 0.5 * 0.5;
    assert_ne!(mid.pixel(c.x as u32, c.y as u32), [255, 255, 255, 255]);
}

#[test]
fn morph_path_fades_a_fill_only_one_side_has() {
    let bx = Xfrm::new(100.0, 100.0, 200.0, 200.0);
    let form = |id: u32, g: &str| Shape {
        id: ShapeId(id),
        xfrm: Some(bx),
        geom: Geom::preset(g),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    };
    let mut p = deck_with(vec![form(1, "rect")]);
    let mut b = (*p.slides[0]).clone();
    b.shapes = vec![form(1, "line")];
    p.slides.push(std::sync::Arc::new(b));
    let o = RenderOpts { scale: 0.5, ..Default::default() };
    let at = |t: f64| {
        let pm = PathMorph { id: ShapeId(10), from: Geom::preset("rect"), from_box: bx, to_box: bx, t };
        // A point near the line but not on it: inside the shape all the way.
        render_blend(&p, (0, 1), t, &[(form(10, "line"), false)], &[], &[pm], &o).pixel(110, 90)
    };
    let (full, half, none) = (at(0.0), at(0.5), at(1.0));
    assert_eq!(none, [255, 255, 255, 255]);
    assert!(full[0] < 128, "{full:?}");
    // Halfway the fill is half there: between the full colour and the white slide.
    assert!(half[0] > full[0] + 40 && half[0] < 215, "{half:?} vs {full:?}");
}

#[test]
fn morph_path_aligns_once_per_transition() {
    let (from, to) = (Xfrm::new(0.0, 0.0, 200.0, 200.0), Xfrm::new(0.0, 0.0, 300.0, 150.0));
    let star = Shape { geom: Geom::preset("star5"), ..Default::default() };
    let pm = |t: f64| PathMorph { id: ShapeId(10), from: Geom::preset("ellipse"), from_box: from, to_box: to, t };
    let els = |(g, f): (deckcraft_geom::preset::Geometry, Vec<_>)| (g.paths.iter().map(|p| p.path.elements().to_vec()).collect::<Vec<_>>(), f);
    let mut cache = morph_path::Cache::default();
    for t in [0.0, 0.3, 0.7, 1.0] {
        let got = els(cache.geometry(&pm(t), &star, 250.0, 175.0));
        // The same as aligning afresh.
        assert_eq!(got, els(morph_path::Cache::default().geometry(&pm(t), &star, 250.0, 175.0)));
        cache.sweep();
    }
    assert_eq!(cache.len(), 1);
    // Another target outline is another morph.
    let _ = cache.geometry(&pm(0.5), &Shape { geom: Geom::preset("rect"), ..Default::default() }, 250.0, 175.0);
    assert_eq!(cache.len(), 2);
    // A frame without morphs ends the transition: the cache empties.
    cache.sweep();
    cache.sweep();
    assert_eq!(cache.len(), 0);
}

#[test]
fn text_transform_unmirrors_flips() {
    // Asymmetric text rect in a 100×50 box: flips move the rect, never mirror the text.
    let tr = Rect::new(10.0, 0.0, 40.0, 50.0);
    let at = |x: Xfrm, p: Point| text_transform(x.affine(), tr, 0.0) * p;
    let near = |a: Point, b: Point| (a - b).hypot() < 1e-9;
    let base = Xfrm::new(0.0, 0.0, 100.0, 50.0);
    // flipH: the rect moves to 60..90 and the text still runs left to right.
    let h = Xfrm { flip_h: true, ..base };
    assert!(near(at(h, Point::new(10.0, 0.0)), Point::new(60.0, 0.0)));
    assert!(near(at(h, Point::new(40.0, 50.0)), Point::new(90.0, 50.0)));
    // flipV: the text is turned 180° in its (vertically mirrored) rect.
    let v = Xfrm { flip_v: true, ..base };
    assert!(near(at(v, Point::new(10.0, 0.0)), Point::new(40.0, 50.0)));
    assert!(near(at(v, Point::new(40.0, 50.0)), Point::new(10.0, 0.0)));
    // Both flips: the same as a 180° turn.
    let hv = Xfrm { flip_h: true, flip_v: true, ..base };
    let turned = Xfrm { rot: 180.0, ..base };
    for p in [Point::new(10.0, 0.0), Point::new(40.0, 50.0), Point::new(25.0, 10.0)] {
        assert!(near(at(hv, p), at(turned, p)));
    }
    // Unflipped and hostile input: plain transform, nothing NaN from the helper itself.
    assert_eq!(text_transform(base.affine(), tr, 0.0), base.affine());
    let _ = text_transform(h.affine(), Rect::new(f64::NAN, 0.0, f64::INFINITY, 1.0), f64::NAN);
}

/// Slide with text box `x` holding left-aligned, asymmetric text.
fn flipped_text(x: Xfrm, group: Option<Xfrm>) -> Image {
    let mut s = text_shape(1, "Fly 42 RJ", 0.0, 0.0);
    s.xfrm = Some(x);
    let s = match group {
        Some(g) => Shape {
            id: ShapeId(2),
            xfrm: Some(g),
            kind: ShapeKind::Group { children: vec![s], child: Xfrm::new(0.0, 0.0, 600.0, 120.0) },
            ..Default::default()
        },
        None => s,
    };
    render_slide(&deck_with(vec![s]), 0, &RenderOpts { scale: 0.5, ..Default::default() })
}

fn differing(a: &Image, b: &Image) -> usize {
    a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x.abs_diff(**y) > 64).count()
}

#[test]
fn flipped_shapes_draw_readable_text() {
    let base = Xfrm::new(180.0, 200.0, 600.0, 120.0);
    let plain = flipped_text(base, None);
    // Warm up: faces load lazily, so the first render may still fall back.
    let plain = if differing(&plain, &flipped_text(base, None)) > 0 { flipped_text(base, None) } else { plain };
    let turned = flipped_text(Xfrm { rot: 180.0, ..base }, None);
    assert!(differing(&plain, &turned) > 200, "the comparisons below must mean something");
    // flipH: text as unflipped (the text box is symmetric). flipV and both: turned 180°.
    let d = differing(&plain, &flipped_text(Xfrm { flip_h: true, ..base }, None));
    assert!(d < 40, "flipH mirrored the text: {d}");
    let d = differing(&turned, &flipped_text(Xfrm { flip_v: true, ..base }, None));
    assert!(d < 40, "flipV mirrored the text: {d}");
    let d = differing(&turned, &flipped_text(Xfrm { flip_h: true, flip_v: true, ..base }, None));
    assert!(d < 40, "flipH+flipV: {d}");
    // Rotation then flipH: still the rotated text, not its mirror image.
    let rot = Xfrm { rot: 30.0, ..base };
    let d = differing(&flipped_text(rot, None), &flipped_text(Xfrm { flip_h: true, ..rot }, None));
    assert!(d < 40, "rotated flipH: {d}");
    // Groups: a flipped group's text isn't mirrored; a flipped child in a flipped group neither.
    let child = Xfrm::new(0.0, 0.0, 600.0, 120.0);
    let d = differing(&plain, &flipped_text(child, Some(Xfrm { flip_h: true, ..base })));
    assert!(d < 40, "group flipH: {d}");
    let d = differing(&plain, &flipped_text(Xfrm { flip_h: true, ..child }, Some(Xfrm { flip_h: true, ..base })));
    assert!(d < 40, "flipH child in flipH group: {d}");
    let d = differing(&turned, &flipped_text(child, Some(Xfrm { flip_v: true, ..base })));
    assert!(d < 40, "group flipV: {d}");
}

/// A text box with one run of `props` (40 pt).
fn run_deck(props: deckcraft_model::RunProps) -> Presentation {
    let mut sh = text_shape(1, "Linked text", 40.0, 40.0);
    if let Some(run) = sh.text.as_mut().and_then(|t| t.paragraphs[0].runs.first_mut()) {
        run.props = deckcraft_model::RunProps { size: Some(40.0), ..props };
    }
    deck_with(vec![sh])
}

#[test]
fn hyperlinks_draw_in_the_hyperlink_colour_underlined() {
    use deckcraft_model::RunProps;
    use deckcraft_model::text::{Action, Hyperlink};
    let o = RenderOpts { scale: 0.5, ..Default::default() };
    let px = |p: &Presentation| render_slide(p, 0, &o);
    let diff = |a: &Image, b: &Image| a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x.abs_diff(**y) > 64).count();
    let link = Some(Hyperlink {
        action: Action::Url { url: "https://example.org".into() },
        tooltip: String::new(),
        highlight_click: false,
        sound: None,
        stop_sound: false,
        zoom_return: None,
    });
    let hlink = Some(Fill::solid(ColorRef::scheme(deckcraft_color::SchemeSlot::Hlink)));
    let red = Some(Fill::solid(ColorRef::rgb(Rgba::rgb(200, 0, 0))));
    let _ = px(&run_deck(RunProps::default())); // faces load lazily
    let linked = px(&run_deck(RunProps { link: link.clone(), ..Default::default() }));
    assert!(diff(&linked, &px(&run_deck(RunProps::default()))) > 100, "a link doesn't look like plain text");
    // Exactly the theme's hyperlink colour, single underline.
    let styled = px(&run_deck(RunProps { fill: hlink, underline: Some("sng".into()), ..Default::default() }));
    assert_eq!(diff(&linked, &styled), 0);
    // A colour on the run itself wins; u="none" drops the underline.
    let own = px(&run_deck(RunProps { link: link.clone(), fill: red.clone(), underline: Some("none".into()), ..Default::default() }));
    assert_eq!(diff(&own, &px(&run_deck(RunProps { fill: red.clone(), ..Default::default() }))), 0);
    let own = px(&run_deck(RunProps { link, fill: red.clone(), ..Default::default() }));
    assert_eq!(diff(&own, &px(&run_deck(RunProps { fill: red, underline: Some("sng".into()), ..Default::default() }))), 0);
}

#[test]
fn a_linear_gradient_ramps_like_powerpoint() {
    // Black to white, left to right: PowerPoint draws 105 a quarter of the way and 186 half way
    // (linear-light mix, S-shaped), not 64 and 128.
    use deckcraft_model::style::{Fill, Gradient, GradientShape, GradientStop};
    let stop = |pos, c| GradientStop { pos, color: ColorRef::rgb(c) };
    let sh = Shape {
        id: ShapeId(700),
        xfrm: Some(Xfrm::new(0.0, 0.0, 800.0, 100.0)),
        fill: Some(Fill::Gradient(Gradient {
            tile: [0.0; 4],
            stops: vec![stop(0.0, deckcraft_color::Rgba::rgb(0, 0, 0)), stop(1.0, deckcraft_color::Rgba::rgb(255, 255, 255))],
            shape: GradientShape::Linear { angle: 0.0, scaled: false },
            rotate_with_shape: true,
            flip: None,
        })),
        line: Some(deckcraft_model::style::Line { fill: Some(Fill::None), ..Default::default() }),
        ..Default::default()
    };
    let p = deck_with(vec![sh]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    for (x, want) in [(200, 105), (400, 186), (600, 238)] {
        let got = img.pixel(x, 50)[0] as i32;
        assert!((got - want).abs() <= 4, "x {x}: {got} vs {want}");
    }
}

#[test]
fn a_gradient_with_a_stop_off_the_end_stays_linear() {
    // Stops at 20 % and 100 %: PowerPoint blends linearly in sRGB (96 at 50 %, 175 at 75 %).
    use deckcraft_model::style::{Fill, Gradient, GradientShape, GradientStop};
    let stop = |pos, c| GradientStop { pos, color: ColorRef::rgb(c) };
    let sh = Shape {
        id: ShapeId(701),
        xfrm: Some(Xfrm::new(0.0, 0.0, 800.0, 100.0)),
        fill: Some(Fill::Gradient(Gradient {
            tile: [0.0; 4],
            stops: vec![stop(0.2, deckcraft_color::Rgba::rgb(0, 0, 0)), stop(1.0, deckcraft_color::Rgba::rgb(255, 255, 255))],
            shape: GradientShape::Linear { angle: 0.0, scaled: false },
            rotate_with_shape: true,
            flip: None,
        })),
        line: Some(deckcraft_model::style::Line { fill: Some(Fill::None), ..Default::default() }),
        ..Default::default()
    };
    let p = deck_with(vec![sh]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    for (x, want) in [(400, 96), (600, 175)] {
        let got = img.pixel(x, 50)[0] as i32;
        assert!((got - want).abs() <= 4, "x {x}: {got} vs {want}");
    }
}

#[test]
fn a_shape_filled_with_the_background_shows_the_slide_background() {
    // `useBgFill`: PowerPoint paints the slide's background into the shape, laid out on the
    // slide. A white panel covers a black-to-grey background; a shape on it with the background
    // fill is a window onto that background.
    use deckcraft_model::style::{Fill, Gradient, GradientShape, GradientStop};
    let none = || Some(deckcraft_model::style::Line { fill: Some(Fill::None), ..Default::default() });
    let panel = Shape {
        id: ShapeId(710),
        xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)),
        fill: Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 255, 255)))),
        line: none(),
        ..Default::default()
    };
    let window = Shape {
        id: ShapeId(711),
        xfrm: Some(Xfrm::new(100.0, 100.0, 760.0, 340.0)),
        fill: Some(Fill::Background),
        line: none(),
        ..Default::default()
    };
    let mut p = deck_with(vec![panel, window]);
    let stop = |pos, v| GradientStop { pos, color: ColorRef::rgb(deckcraft_color::Rgba::rgb(v, v, v)) };
    std::sync::Arc::make_mut(&mut p.slides[0]).background = Some(deckcraft_model::Background::Fill {
        fill: Fill::Gradient(Gradient {
            tile: [0.0; 4],
            stops: vec![stop(0.0, 0), stop(0.5, 0), stop(1.0, 200)],
            shape: GradientShape::Linear { angle: 0.0, scaled: false },
            rotate_with_shape: true,
            flip: None,
        }),
    });
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    assert_eq!(img.pixel(50, 50)[0], 255, "the panel stays white outside the window");
    assert!(img.pixel(300, 270)[0] < 8, "the window shows the background's black half: {:?}", img.pixel(300, 270));
    // Laid out on the slide: three quarters across the slide is half way from black to grey.
    let v = img.pixel(720, 270)[0] as i32;
    assert!((v - 100).abs() <= 6, "{v}");
}

#[test]
fn a_background_is_laid_over_white() {
    // A background picture stretched into the bottom-right quarter (fillRect l=50% t=50%), and a
    // half-transparent colour: PowerPoint shows white around and under them, not transparency.
    use deckcraft_model::style::{Fill, PictureFill, PictureMode};
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([0, 0, 200]))).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let mut p = deck_with(vec![]);
    let media = p.add_media("bg.png", "image/png", png.into_inner()).expect("media");
    let pf = PictureFill { media, mode: PictureMode::Stretch { fill_rect: [0.5, 0.5, 0.0, 0.0] }, ..Default::default() };
    std::sync::Arc::make_mut(&mut p.slides[0]).background = Some(deckcraft_model::Background::Fill { fill: Fill::Picture(pf) });
    let img = render_slide(&p, 0, &RenderOpts { scale: 0.5, ..Default::default() });
    assert_eq!(img.pixel(20, 20), [255, 255, 255, 255], "white where the picture doesn't reach");
    let q = img.pixel(400, 230);
    assert!(q[2] > 190 && q[0] < 10 && q[3] == 255, "{q:?}");
    let see_through = ColorRef::rgb(deckcraft_color::Rgba { r: 0, g: 0, b: 0, a: 128 });
    std::sync::Arc::make_mut(&mut p.slides[0]).background = Some(deckcraft_model::Background::Fill { fill: Fill::solid(see_through) });
    let img = render_slide(&p, 0, &RenderOpts { scale: 0.5, ..Default::default() });
    let g = img.pixel(20, 20);
    assert!(g[3] == 255 && (g[0] as i32 - 127).abs() <= 3, "half-transparent black over white is grey: {g:?}");
}

#[test]
fn a_transparent_background_leaves_the_slide_background_out() {
    // #784: PNG export with a transparent background draws only what is on the slide. The
    // background (a solid colour here) and its white underlay are left out; the shapes, and a
    // shape filled with the background (`useBgFill`), still draw.
    use deckcraft_model::style::Fill;
    let blue = ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 200));
    let red = Shape {
        id: ShapeId(784),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        fill: Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(220, 0, 0)))),
        ..Default::default()
    };
    let bg_filled = Shape { id: ShapeId(785), xfrm: Some(Xfrm::new(500.0, 100.0, 200.0, 100.0)), fill: Some(Fill::Background), ..Default::default() };
    let mut p = deck_with(vec![red, bg_filled]);
    std::sync::Arc::make_mut(&mut p.slides[0]).background = Some(deckcraft_model::Background::Fill { fill: Fill::solid(blue) });
    let opaque = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    assert_eq!(opaque.pixel(20, 20), [0, 0, 200, 255], "without the option the background draws");
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, transparent_background: true, ..Default::default() });
    assert_eq!(img.pixel(20, 20), [0, 0, 0, 0], "no background, no white underlay");
    assert_eq!(img.pixel(200, 150), [220, 0, 0, 255], "the slide's shapes still draw");
    assert_eq!(img.pixel(600, 150), [0, 0, 200, 255], "a background-filled shape still shows the background");
    // The default theme background (a style reference) is left out too.
    let img = render_slide(&deck_with(vec![]), 0, &RenderOpts { scale: 0.25, transparent_background: true, ..Default::default() });
    assert!(img.pixels.as_chunks::<4>().0.iter().all(|px| px[3] == 0), "a blank slide is fully transparent");
}

#[test]
fn colour_glyphs_paint_their_layers() {
    // A COLRv0 test font (crates/render/tests/fixtures): U+E000 is a red square with a blue
    // square in its middle, U+E001 a square in the text colour.
    deckcraft_fonts::FontDb::global().add_font(include_bytes!("../tests/fixtures/colr-squares.ttf").to_vec());
    let mut sh = Shape { id: ShapeId(720), xfrm: Some(Xfrm::new(0.0, 0.0, 400.0, 200.0)), text_box: true, ..Default::default() };
    let mut t = TextBody::from_text("\u{E000} \u{E001}");
    let props = &mut t.paragraphs[0].runs[0].props;
    props.size = Some(72.0);
    props.font = Some("DeckCraft Colr Squares".into());
    props.fill = Some(deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 160, 0))));
    sh.text = Some(t);
    let p = deck_with(vec![sh]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let count = |f: &dyn Fn(&[u8; 4]) -> bool| img.pixels.as_chunks::<4>().0.iter().filter(|px| f(px)).count();
    let red = count(&|p| p[0] > 230 && p[1] < 30 && p[2] < 30);
    let blue = count(&|p| p[0] < 30 && p[1] < 30 && p[2] > 230);
    let green = count(&|p| p[0] < 30 && p[1] > 140 && p[1] < 180 && p[2] < 30);
    // 72 pt squares: the blue one is 40 % of the side (16 % of the area).
    assert!(red > 2500 && blue > 500, "red {red} blue {blue}");
    let share = blue as f64 / (red + blue) as f64;
    assert!((share - 0.16).abs() < 0.03, "blue share {share}");
    // The text-colour glyph is as big as the other square.
    assert!((green as f64 / (red + blue) as f64 - 1.0).abs() < 0.1, "green {green} vs {}", red + blue);
}

#[test]
fn a_text_gradient_starts_over_on_every_line() {
    // PowerPoint fills each line of gradient text with the whole gradient, spanning that line
    // (black at the top of every line to white at its bottom), not the text box.
    use deckcraft_model::style::{Fill, Gradient, GradientShape, GradientStop};
    let stop = |pos, v| GradientStop { pos, color: ColorRef::rgb(deckcraft_color::Rgba::rgb(v, v, v)) };
    let mut sh = Shape { id: ShapeId(730), xfrm: Some(Xfrm::new(0.0, 0.0, 600.0, 500.0)), text_box: true, ..Default::default() };
    let mut t = TextBody::from_text("HHHH\nHHHH");
    for p in &mut t.paragraphs {
        let props = &mut p.runs[0].props;
        props.size = Some(80.0);
        props.fill = Some(Fill::Gradient(Gradient {
            tile: [0.0; 4],
            stops: vec![stop(0.0, 0), stop(1.0, 255)],
            shape: GradientShape::Linear { angle: 90.0, scaled: false },
            rotate_with_shape: true,
            flip: None,
        }));
    }
    sh.text = Some(t);
    let p = deck_with(vec![sh]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    // Ink rows (any pixel not white), grouped into the two lines.
    let ink = |y: u32| (0..600).any(|x| img.pixel(x, y)[0] < 250);
    let mut bands: Vec<(u32, u32)> = vec![];
    for y in 0..500 {
        if ink(y) {
            match bands.last_mut() {
                Some(b) if b.1 + 1 == y => b.1 = y,
                _ => bands.push((y, y)),
            }
        }
    }
    assert_eq!(bands.len(), 2, "{bands:?}");
    // The darkest ink in the first rows of each line.
    let top = |b: (u32, u32)| (b.0..b.0 + 6).flat_map(|y| (0..600).map(move |x| (x, y))).map(|(x, y)| img.pixel(x, y)[0]).min().unwrap();
    let (a, b) = (top(bands[0]), top(bands[1]));
    assert!(a < 140 && (a as i32 - b as i32).abs() < 25, "line tops {a} and {b}: the second line starts dark again");
    // ...and each line ends light.
    let bottom = |b: (u32, u32)| (b.1 - 5..=b.1).flat_map(|y| (0..600).map(move |x| (x, y))).map(|(x, y)| img.pixel(x, y)[0]).min().unwrap();
    for band in bands {
        assert!(bottom(band) > 150, "line {band:?} ends at {}", bottom(band));
    }
}

/// A text box of big black "IIII" with `set` applied to its run.
fn effect_text(set: impl Fn(&mut deckcraft_model::text::RunProps)) -> Presentation {
    let mut sh = Shape { id: ShapeId(740), xfrm: Some(Xfrm::new(100.0, 100.0, 600.0, 300.0)), text_box: true, ..Default::default() };
    let mut t = TextBody::from_text("IIII");
    let props = &mut t.paragraphs[0].runs[0].props;
    props.size = Some(100.0);
    props.fill = Some(deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0))));
    set(props);
    sh.text = Some(t);
    deck_with(vec![sh])
}

/// Text whose glow comes only from its paragraph's `defRPr` needs the filters too: drawn on the
/// worker threads it panicked (vello_cpu has no filters there) and the slide came out blank.
#[test]
fn a_paragraph_default_text_glow_needs_filters() {
    let mut sh = Shape { id: ShapeId(741), xfrm: Some(Xfrm::new(100.0, 100.0, 600.0, 300.0)), text_box: true, ..Default::default() };
    let mut t = TextBody::from_text("IIII");
    let p = &mut t.paragraphs[0];
    p.runs[0].props.size = Some(100.0);
    p.runs[0].props.fill = Some(deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0))));
    p.def_props.glow = Some(deckcraft_model::style::Glow { color: ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 255, 0)), radius: 8.0 });
    sh.text = Some(t);
    assert_green_glow_drawn_with_threads(&deck_with(vec![sh]));
}

/// Text whose glow comes only from the presentation's default text style (`p:defaultTextStyle`)
/// needs the filters too, or the threaded render panics and the slide comes out blank.
#[test]
fn a_presentation_default_text_style_glow_needs_filters() {
    let mut deck = effect_text(|_| {});
    let level = deck.default_text_style.levels[0].as_mut().expect("the default text style sets level 1");
    level.run.glow = Some(deckcraft_model::style::Glow { color: ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 255, 0)), radius: 8.0 });
    assert_green_glow_drawn_with_threads(&deck);
}

/// Asserts that `deck`'s first slide needs filters and that, rendered with worker threads
/// requested, its text shows a green glow halo rather than a blank slide.
fn assert_green_glow_drawn_with_threads(deck: &Presentation) {
    assert!(needs_filters(deck, &deck.slides[0]));
    let img = render_slide(deck, 0, &RenderOpts { scale: 1.0, threads: 4, ..Default::default() });
    let halo = img.pixels.as_chunks::<4>().0.iter().filter(|p| p[1] > 150 && p[0] < 120 && p[2] < 120).count();
    assert!(halo > 300, "green halo pixels {halo}");
}

#[test]
fn text_glow_and_shadow_are_drawn() {
    let count = |img: &Image, f: &dyn Fn(&[u8; 4]) -> bool| img.pixels.as_chunks::<4>().0.iter().filter(|px| f(px)).count();
    let green = |p: &[u8; 4]| p[1] > 150 && p[0] < 120 && p[2] < 120;
    let plain = effect_text(|_| {});
    assert!(!needs_filters(&plain, &plain.slides[0]));
    let img = render_slide(&plain, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    assert_eq!(count(&img, &green), 0);
    // Glow: a green halo hugging the letters, needing the blur filter.
    let glow =
        effect_text(|p| p.glow = Some(deckcraft_model::style::Glow { color: ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 255, 0)), radius: 8.0 }));
    assert!(needs_filters(&glow, &glow.slides[0]), "text glow blurs");
    let img = render_slide(&glow, 0, &RenderOpts { scale: 1.0, threads: 4, ..Default::default() });
    let halo = count(&img, &green);
    assert!(halo > 300, "green halo pixels {halo}");
    // Shadow: grey ink below and right of the letters, where the plain text has none.
    let shadow = effect_text(|p| {
        p.shadow = Some(deckcraft_model::style::Shadow {
            color: ColorRef::rgb(deckcraft_color::Rgba { r: 0, g: 0, b: 0, a: 128 }),
            blur: 2.0,
            dist: 12.0,
            dir: 45.0,
            inner: false,
            sx: 1.0,
            sy: 1.0,
            kx: 0.0,
            ky: 0.0,
            align: String::new(),
            rotate_with_shape: false,
        })
    });
    let img = render_slide(&shadow, 0, &RenderOpts { scale: 1.0, threads: 4, ..Default::default() });
    let base = render_slide(&plain, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let grey = |p: &[u8; 4]| p[0] > 60 && p[0] < 230 && p[0] == p[1] && p[1] == p[2];
    assert!(count(&img, &grey) > count(&base, &grey) + 300, "a grey shadow appears");
}

#[test]
fn pattern_fills_draw_powerpoints_tiles() {
    // dkVert at one output pixel per pattern cell (0.75 pt): columns 11001100… across, as
    // PowerPoint's export draws it; pct50 is a checkerboard.
    use deckcraft_model::style::{Fill, PatternFill};
    let rgb = |v| ColorRef::rgb(deckcraft_color::Rgba::rgb(v, v, v));
    let shape = |id, x: f64, preset: &str| Shape {
        id: ShapeId(id),
        xfrm: Some(Xfrm::new(x, 0.0, 60.0, 60.0)),
        fill: Some(Fill::Pattern(PatternFill { preset: preset.into(), fg: rgb(0), bg: rgb(255) })),
        line: Some(deckcraft_model::style::Line { fill: Some(Fill::None), ..Default::default() }),
        ..Default::default()
    };
    let p = deck_with(vec![shape(750, 0.0, "dkVert"), shape(751, 120.0, "pct50")]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 4.0 / 3.0, ..Default::default() });
    let dark = |x: u32, y: u32| img.pixel(x, y)[0] < 128;
    let row: Vec<bool> = (8..16).map(|x| dark(x, 20)).collect();
    assert_eq!(row, [true, true, false, false, true, true, false, false], "dkVert columns");
    assert!((8..16).all(|y| dark(8, y) == dark(8, 20)), "dkVert is vertical");
    // pct50 starts at slide x = 120 pt = 160 px.
    for (x, y) in [(168, 8), (169, 9), (170, 12)] {
        assert_ne!(dark(x, y), dark(x + 1, y), "pct50 alternates across at {x},{y}");
        assert_eq!(dark(x, y), dark(x + 1, y + 1), "pct50 repeats diagonally at {x},{y}");
    }
}

#[test]
fn compound_lines_draw_their_bands() {
    // A 24 pt outline on a rectangle, as PowerPoint's export draws each kind (top edge, outside
    // first): double 8/8/8, thin-thick 4.8/4.8/14.4, triple 4/4/8/4/4.
    use deckcraft_model::style::{Compound, Fill, Line};
    let runs = |c: Compound| {
        let sh = Shape {
            id: ShapeId(760),
            xfrm: Some(Xfrm::new(100.0, 100.0, 300.0, 200.0)),
            fill: Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 255, 255)))),
            line: Some(Line {
                fill: Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0)))),
                width: Some(24.0),
                compound: Some(c),
                ..Default::default()
            }),
            ..Default::default()
        };
        let p = deck_with(vec![sh]);
        let img = render_slide(&p, 0, &RenderOpts { scale: 2.0, ..Default::default() });
        // Down the middle of the top edge (y 100 pt), from 80 pt to 120 pt: lengths of dark and
        // light runs in points.
        let mut out: Vec<(bool, f64)> = vec![];
        for y in 160..240 {
            let dark = img.pixel(500, y)[0] < 128;
            match out.last_mut() {
                Some((d, n)) if *d == dark => *n += 0.5,
                _ => out.push((dark, 0.5)),
            }
        }
        out.into_iter().filter(|(d, _)| *d).map(|(_, n)| n).collect::<Vec<f64>>()
    };
    let close = |got: Vec<f64>, want: &[f64]| got.len() == want.len() && got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1.0);
    assert!(close(runs(Compound::Single), &[24.0]));
    let d = runs(Compound::Double);
    assert!(close(d.clone(), &[8.0, 8.0]), "{d:?}");
    let t = runs(Compound::ThinThick);
    assert!(close(t.clone(), &[4.8, 14.4]), "{t:?}");
    let k = runs(Compound::ThickThin);
    assert!(close(k.clone(), &[14.4, 4.8]), "{k:?}");
    let tri = runs(Compound::Triple);
    assert!(close(tri.clone(), &[4.0, 8.0, 4.0]), "{tri:?}");
}

#[test]
fn rect_path_gradients_draw_x_shaped_contours() {
    // path="rect" from the centre, black to grey to white (linear): PowerPoint's colour position is
    // the larger axis share, so 20 pt right of centre in a 280x200 box is 1/7 and 20 pt down 1/5.
    use deckcraft_model::style::{Fill, Gradient, GradientShape, GradientStop};
    let stop = |pos, v| GradientStop { pos, color: ColorRef::rgb(deckcraft_color::Rgba::rgb(v, v, v)) };
    let sh = Shape {
        id: ShapeId(770),
        xfrm: Some(Xfrm::new(0.0, 0.0, 280.0, 200.0)),
        fill: Some(Fill::Gradient(Gradient {
            tile: [0.0; 4],
            stops: vec![stop(0.0, 0), stop(0.5, 128), stop(1.0, 255)],
            shape: GradientShape::Path { path: "rect".into(), focus: [0.5, 0.5, 0.5, 0.5] },
            rotate_with_shape: true,
            flip: None,
        })),
        line: Some(deckcraft_model::style::Line { fill: Some(Fill::None), ..Default::default() }),
        ..Default::default()
    };
    let p = deck_with(vec![sh]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let v = |x: u32, y: u32| img.pixel(x, y)[0] as f64;
    assert!((v(160, 100) - 255.0 / 7.0).abs() < 6.0, "{}", v(160, 100));
    assert!((v(140, 120) - 51.0).abs() < 6.0, "{}", v(140, 120));
    // Along the diagonal to a corner the colour follows the vertical share.
    assert!((v(168, 120) - 51.0).abs() < 6.0, "{}", v(168, 120));
}

#[test]
fn soft_edges_fade_to_the_outline() {
    // An ellipse with a 12 pt soft edge: transparent at its outline, half opaque about 10 pt
    // inside, opaque further in, as in PowerPoint's export (the fade follows the curve, not the
    // bounding box).
    let sh = Shape {
        id: ShapeId(780),
        xfrm: Some(Xfrm::new(100.0, 100.0, 300.0, 200.0)),
        geom: Geom::Preset { name: "ellipse".into(), adj: vec![] },
        fill: Some(deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0)))),
        line: Some(deckcraft_model::style::Line { fill: Some(deckcraft_model::style::Fill::None), ..Default::default() }),
        effects: Some(deckcraft_model::Effects { soft_edge: Some(12.0), ..Default::default() }),
        ..Default::default()
    };
    let p = deck_with(vec![sh]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let alpha = |x: u32, y: u32| 1.0 - img.pixel(x, y)[0] as f64 / 255.0;
    // Top of the ellipse is at y = 100, x = 250.
    assert!(alpha(250, 101) < 0.08, "edge {}", alpha(250, 101));
    let mid = alpha(250, 110);
    assert!((0.3..0.7).contains(&mid), "0.85 r inside: {mid}");
    assert!(alpha(250, 130) > 0.97, "inside {}", alpha(250, 130));
    // Near the ends of the bounding box's top edge, outside the ellipse, nothing is drawn.
    assert!(alpha(110, 105) < 0.01);
}

#[test]
fn a_file_defined_table_style_draws_its_parts() {
    // Red table background (tblBg), see-through white on banded rows, a 4 pt blue left border and
    // no inside lines: drawn from the definition, not DeckCraft's own look for the style.
    use deckcraft_model::style::{Fill, Line};
    use deckcraft_model::table::{TableBorder, TablePart, TablePartStyle, TableStyleDef};
    let rgb = |r, g, b| ColorRef::rgb(deckcraft_color::Rgba::rgb(r, g, b));
    let mut t = Table::new(4, 2, 400.0, 50.0);
    t.style = "{ABCDEF01-2345-4678-9ABC-DEF012345678}".into();
    t.first_row = false;
    t.band_row = true;
    let line = |c: ColorRef, w| TableBorder::Line { line: Line { fill: Some(Fill::solid(c)), width: Some(w), ..Default::default() } };
    let mut whole = TablePartStyle { fill: Some(Fill::None), ..Default::default() };
    whole.borders[0] = Some(line(rgb(0, 0, 255), 4.0));
    whole.borders[4] = Some(TableBorder::Line { line: Line { fill: Some(Fill::None), ..Default::default() } });
    let see_through = ColorRef::rgb(deckcraft_color::Rgba { r: 255, g: 255, b: 255, a: 51 });
    let band = TablePartStyle { fill: Some(Fill::solid(see_through)), ..Default::default() };
    let def = TableStyleDef {
        id: t.style.clone(),
        name: "Probe".into(),
        parts: vec![(TablePart::WholeTbl, whole), (TablePart::Band1H, band)],
        background: Some(TablePartStyle { fill: Some(Fill::solid(rgb(255, 0, 0))), ..Default::default() }),
        raw: String::new(),
    };
    let tbl = Shape { id: ShapeId(790), xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)), kind: ShapeKind::Table(t), ..Default::default() };
    let mut p = deck_with(vec![tbl]);
    p.table_styles.push(def);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let px = |x, y| img.pixel(x, y);
    // Row 1 (band1H): red with 20 % white over it; row 2 (band2H, undefined): plain red.
    let banded = px(300, 125);
    assert!(banded[0] > 250 && (40..65).contains(&banded[1]), "{banded:?}");
    assert_eq!(px(300, 175)[..3], [255, 0, 0]);
    // No line between the rows; a blue left border.
    let edge = px(300, 150);
    assert!(edge[0] > 240, "no inside line: {edge:?}");
    let left = px(100, 175);
    assert!(left[2] > 200 && left[0] < 60, "{left:?}");
}

#[test]
fn a_table_fill_shows_behind_cells_without_their_own() {
    // `a:tblPr` solidFill (tdf#127237): cells with no fill show the table's blue.
    use deckcraft_model::style::Fill;
    let mut t = Table::new(2, 2, 200.0, 50.0);
    t.style = "none".into();
    t.background = Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 112, 192))));
    for r in 0..2 {
        for c in 0..2 {
            t.cell_mut(r, c).unwrap().fill = Some(Fill::None);
        }
    }
    let tbl = Shape { id: ShapeId(795), xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)), kind: ShapeKind::Table(t), ..Default::default() };
    let p = deck_with(vec![tbl]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    assert_eq!(img.pixel(150, 160)[..3], [0, 112, 192]);
    assert_eq!(img.pixel(50, 50)[..3], [255, 255, 255]);
}

#[test]
fn table_body_text_is_dark_on_the_tinted_cells_of_a_dark_theme() {
    // The default style (medium2) tints body cells towards white, so their text must be the
    // theme's dark colour, as PowerPoint's medium styles set it (dk1): inheriting the text colour
    // leaves it light, and unreadable, on a dark theme such as Nocturne.
    let fill_and_darkest = |theme: &str| {
        let mut t = Table::new(2, 2, 200.0, 40.0);
        for (r, c) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            t.cell_mut(r, c).unwrap().text = TextBody::from_text("Body");
        }
        let tbl = Shape { id: ShapeId(796), xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 80.0)), kind: ShapeKind::Table(t), ..Default::default() };
        let mut p = deck_with(vec![tbl]);
        let th = deckcraft_model::theme::builtin_themes().into_iter().find(|th| th.name == theme).unwrap();
        std::sync::Arc::make_mut(&mut p.masters[0]).theme = th;
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        let lum = |x, y| {
            let px = img.pixel(x, y);
            deckcraft_color::Rgba::rgb(px[0], px[1], px[2]).luminance()
        };
        // Body cell (1, 1) covers (200, 140)–(300, 180); its top-left corner is clear of the text
        // (cell margins) and of the white grid line.
        let fill = lum(203, 143);
        let darkest = (143..177).flat_map(|y| (203..297).map(move |x| (x, y))).map(|(x, y)| lum(x, y)).fold(1.0_f64, f64::min);
        (fill, darkest)
    };
    for theme in ["Nocturne", "Harbor"] {
        let (fill, darkest) = fill_and_darkest(theme);
        assert!(fill > 0.4, "{theme}: the body cell's fill is a light tint, luminance {fill}");
        assert!(
            darkest < 0.15 && darkest < fill / 3.0,
            "{theme}: darkest pixel in the body cell has luminance {darkest} on a fill of {fill}: its text is not dark"
        );
    }
}

#[test]
fn table_light_and_dark_presets_give_their_own_header_and_body_looks() {
    // light1, light2 and dark1 each style the header and banded body cells their own way.
    use deckcraft_color::{ColorTransform, Rgba, SchemeSlot};
    use deckcraft_model::style::ColorRef;
    let p = Presentation::default();
    let ctx = Ctx::for_slide(&p, &p.slides[0]).unwrap();
    let a = ctx.color(&ColorRef::scheme(SchemeSlot::Accent1), None);
    let look = |style: &str, r: usize| {
        let mut t = Table::new(3, 1, 100.0, 90.0);
        t.style = style.to_string();
        t.first_row = true;
        t.band_row = true;
        crate::table::look_with_def(&ctx, None, &t, r, 0)
    };

    // light1: header has no fill, bold text and an accent border; banded body rows get a light tint.
    let h = look("light1", 0);
    assert_eq!((h.fill, h.bold, h.border), (None, true, Some((a, 1.0))));
    let b = look("light1", 1);
    assert_eq!(b.fill, Some(deckcraft_color::apply(a, &[ColorTransform::Tint(20000)])));
    assert!(b.text.is_some() && !b.bold);

    // light2: accent-filled header with white bold text; body has no fill.
    let h = look("light2", 0);
    assert_eq!((h.fill, h.text, h.bold, h.border), (Some(a), Some(Rgba::WHITE), true, Some((a, 1.0))));
    let b = look("light2", 1);
    assert_eq!((b.fill, b.text, b.bold), (None, None, false));

    // dark1: black header, shaded accent body (darker when banded), white text and white borders.
    let h = look("dark1", 0);
    assert_eq!((h.fill, h.text, h.bold, h.border), (Some(Rgba::BLACK), Some(Rgba::WHITE), true, Some((Rgba::WHITE, 1.0))));
    let b = look("dark1", 1);
    assert_eq!(b.fill, Some(deckcraft_color::apply(a, &[ColorTransform::Shade(60000)])));
    assert_eq!(b.text, Some(Rgba::WHITE));
}

/// A white 280x200 pt rectangle at (20, 20) with `effects`, rendered at 1 px/pt.
fn effect_box(effects: deckcraft_model::Effects) -> Image {
    use deckcraft_model::style::{Fill, Line};
    let sh = Shape {
        id: ShapeId(800),
        xfrm: Some(Xfrm::new(20.0, 20.0, 280.0, 200.0)),
        fill: Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 255, 255)))),
        line: Some(Line { fill: Some(Fill::None), ..Default::default() }),
        effects: Some(effects),
        ..Default::default()
    };
    render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
}

fn shadow(blur: f64, dist: f64, dir: f64) -> deckcraft_model::style::Shadow {
    deckcraft_model::style::Shadow {
        color: ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0)),
        blur,
        dist,
        dir,
        inner: false,
        sx: 1.0,
        sy: 1.0,
        kx: 0.0,
        ky: 0.0,
        align: String::new(),
        rotate_with_shape: false,
    }
}

#[test]
fn shadows_and_glows_spread_like_powerpoints() {
    let red = |img: &Image, x: u32, y: u32| img.pixel(x, y)[0] as i32;
    // Outer shadow, 9 pt blur, no offset: PowerPoint's export reads 144, 204, 238 at 1, 3 and 5
    // px outside the left edge.
    let img = effect_box(deckcraft_model::Effects { outer_shadow: Some(shadow(9.0, 0.0, 0.0)), ..Default::default() });
    for (d, want) in [(1, 144), (3, 204), (5, 238)] {
        assert!((red(&img, 20 - d, 120) - want).abs() <= 6, "outer {d}: {}", red(&img, 20 - d, 120));
    }
    // Inner shadow pointing down-right (45 degrees): it falls inside along the bottom and right
    // edges, not the top and left.
    let mut sh = shadow(0.0, 10.0, 45.0);
    sh.inner = true;
    let img = effect_box(deckcraft_model::Effects { inner_shadow: Some(sh), ..Default::default() });
    assert!(red(&img, 296, 120) < 20, "right edge {}", red(&img, 296, 120));
    assert!(red(&img, 160, 216) < 20, "bottom edge {}", red(&img, 160, 216));
    assert!(red(&img, 24, 120) > 250 && red(&img, 160, 24) > 250, "top and left stay clear");
    // Inner shadow with a 9 pt blur and no offset: the same profile as the outer one, inside.
    let mut sh = shadow(9.0, 0.0, 0.0);
    sh.inner = true;
    let img = effect_box(deckcraft_model::Effects { inner_shadow: Some(sh), ..Default::default() });
    for (d, want) in [(0, 144), (2, 204), (4, 238)] {
        assert!((red(&img, 20 + d, 120) - want).abs() <= 6, "inner {d}: {}", red(&img, 20 + d, 120));
    }
    // Glow of radius 10 pt: nearly solid 1 px out, about half strength 5 px out, gone by 10.
    let glow = deckcraft_model::style::Glow { color: ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0)), radius: 10.0 };
    let img = effect_box(deckcraft_model::Effects { glow: Some(glow), ..Default::default() });
    assert!(red(&img, 19, 120) < 20);
    assert!((60..170).contains(&red(&img, 15, 120)), "{}", red(&img, 15, 120));
    assert!(red(&img, 10, 120) > 245);
}

/// A 400 × 200 text box at (100, 100) holding `text` at 40 pt, warped by `warp`.
fn wordart(warp: Option<&str>, text: &str) -> Presentation {
    let mut t = TextBody::from_text(text);
    t.paragraphs[0].runs[0].props.size = Some(40.0);
    t.paragraphs[0].props.align = Some(deckcraft_model::text::Align::Center);
    t.body.warp = warp.map(String::from);
    deck_with(vec![Shape { id: ShapeId(700), xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)), text: Some(t), ..Default::default() }])
}

/// Ink box (x0, y0, x1, y1) of dark pixels, and the top ink row of each column.
fn ink(img: &Image) -> ((u32, u32, u32, u32), Vec<Option<u32>>) {
    let mut b = (u32::MAX, u32::MAX, 0, 0);
    let mut tops = vec![None; img.width as usize];
    for y in 0..img.height {
        for x in 0..img.width {
            if img.pixel(x, y)[0] < 128 {
                b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
                if tops[x as usize].is_none() {
                    tops[x as usize] = Some(y);
                }
            }
        }
    }
    (b, tops)
}

#[test]
fn wordart_arch_up_lifts_the_middle_of_the_line() {
    let o = RenderOpts { scale: 1.0, ..Default::default() };
    let flat = render_slide(&wordart(None, "ARCHED WORDART"), 0, &o);
    let arched = render_slide(&wordart(Some("textArchUp"), "ARCHED WORDART"), 0, &o);
    assert!(diff_px(&flat, &arched) > 1000);
    let ((fx0, _, fx1, _), ftops) = ink(&flat);
    let ((ax0, ay0, ax1, _), atops) = ink(&arched);
    let top_near = |tops: &[Option<u32>], x: u32| (x.saturating_sub(15)..x + 15).filter_map(|i| tops.get(i as usize).copied().flatten()).min();
    // Flat text: the ends and the middle stand on one line.
    let (fl, fm) = (top_near(&ftops, fx0 + 20).unwrap(), top_near(&ftops, (fx0 + fx1) / 2).unwrap());
    assert!(fl.abs_diff(fm) < 4, "flat {fl} {fm}");
    // Arched: the line follows the top of the ellipse inscribed in the box, so the middle is
    // at the box top and the ends sit clearly lower.
    let (al, am, ar) = (top_near(&atops, ax0 + 20).unwrap(), top_near(&atops, (ax0 + ax1) / 2).unwrap(), top_near(&atops, ax1 - 20).unwrap());
    assert!(al > am + 10 && ar > am + 10, "arched {al} {am} {ar}");
    // At the box top (y = 100), give or take the font: Linux's fallback face reaches 79.
    assert!((70..=110).contains(&ay0), "arch top {ay0}");
}

#[test]
fn wordart_plain_stretches_the_ink_over_the_text_rect() {
    let img = render_slide(&wordart(Some("textPlain"), "PLAIN"), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let ((x0, y0, x1, y1), _) = ink(&img);
    for (got, want) in [(x0, 100), (y0, 100), (x1, 499), (y1, 299)] {
        assert!(got.abs_diff(want) <= 2, "ink box {:?}", (x0, y0, x1, y1));
    }
}

#[test]
fn unknown_or_absent_warps_draw_flat_text() {
    let o = RenderOpts { scale: 0.5, ..Default::default() };
    // Warm the font database first (other tests load fonts concurrently).
    let _ = render_slide(&wordart(None, "FLAT"), 0, &o);
    let flat = render_slide(&wordart(None, "FLAT"), 0, &o);
    for w in ["textNoShape", "bogus", ""] {
        assert_eq!(diff_px(&flat, &render_slide(&wordart(Some(w), "FLAT"), 0, &o)), 0, "{w}");
    }
}

#[test]
fn every_warp_preset_renders_text_inside_the_slide() {
    let o = RenderOpts { scale: 0.5, ..Default::default() };
    for name in
        ["textArchUp", "textArchDown", "textCircle", "textButton", "textWave1", "textRingInside", "textCirclePour", "textDeflateInflateDeflate"]
    {
        let mut p = wordart(Some(name), "One line\nTwo line\nThree");
        if let Some(b) = std::sync::Arc::make_mut(&mut p.slides[0]).shapes[0].text.as_mut() {
            b.body.warp_adj = vec![f64::NAN, -1e12];
        }
        let img = render_slide(&p, 0, &o);
        let ((x0, y0, x1, y1), _) = ink(&img);
        assert!(x0 <= x1 && y0 <= y1, "{name}: no ink");
    }
}

/// A placeable WMF, 2 × 1 inch (2880 × 1440 twips): a solid red brush and a rectangle over the
/// left half, nothing on the right.
fn half_red_wmf() -> Vec<u8> {
    fn rec(func: u16, params: &[i16]) -> Vec<u8> {
        let mut v = ((3 + params.len()) as u32).to_le_bytes().to_vec();
        v.extend(func.to_le_bytes());
        for p in params {
            v.extend(p.to_le_bytes());
        }
        v
    }
    // CREATEBRUSHINDIRECT (solid, COLORREF 0x000000FF = red), SELECTOBJECT 0, RECTANGLE (bottom,
    // right, top, left).
    let body = [rec(0x02FC, &[0, 0xFF, 0, 0]), rec(0x012D, &[0]), rec(0x041B, &[1440, 1440, 0, 0]), rec(0, &[])].concat();
    let mut f = 0x9AC6_CDD7u32.to_le_bytes().to_vec();
    f.extend(0u16.to_le_bytes());
    for v in [0i16, 0, 2880, 1440] {
        f.extend(v.to_le_bytes());
    }
    f.extend(1440u16.to_le_bytes());
    f.extend(0u32.to_le_bytes());
    f.extend(0u16.to_le_bytes());
    let total = (f.len() + 18 + body.len()) / 2;
    for v in [1u16, 9, 0x300] {
        f.extend(v.to_le_bytes());
    }
    f.extend((total as u32).to_le_bytes());
    f.extend(16u16.to_le_bytes());
    f.extend(0u32.to_le_bytes());
    f.extend(0u16.to_le_bytes());
    f.extend(body);
    f
}

#[test]
fn metafile_pictures_draw_their_vectors() {
    let mut p = Presentation::default();
    let media = p.add_media("image1.wmf", "image/x-wmf", half_red_wmf()).expect("media");
    let fill = deckcraft_model::style::PictureFill { media, ..Default::default() };
    let sh = Shape { id: ShapeId(600), xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)), kind: ShapeKind::Picture { fill }, ..Default::default() };
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
    for scale in [0.5, 1.0, 3.0] {
        let img = render_slide(&p, 0, &RenderOpts { scale, ..Default::default() });
        let at = |x: f64, y: f64| img.pixel((x * scale) as u32, (y * scale) as u32);
        let left = at(200.0, 200.0);
        assert!(left[0] > 200 && left[1] < 60 && left[2] < 60, "left half red at {scale}: {left:?}");
        let right = at(400.0, 200.0);
        assert!(right[0] > 200 && right[1] > 200 && right[2] > 200, "right half empty at {scale}: {right:?}");
    }
}

#[test]
fn broken_metafiles_draw_nothing_and_never_panic() {
    let wmf = half_red_wmf();
    let mut cases: Vec<Vec<u8>> = (0..wmf.len()).step_by(3).map(|n| wmf[..n].to_vec()).collect();
    cases.push(vec![1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
    let mut huge = wmf.clone();
    huge[10] = 0xFF;
    huge[11] = 0x7F;
    cases.push(huge);
    for bytes in cases {
        let mut p = Presentation::default();
        let media = p.add_media("image1.emf", "image/x-emf", bytes).expect("media");
        let fill = deckcraft_model::style::PictureFill { media, ..Default::default() };
        let sh = Shape { id: ShapeId(600), xfrm: Some(Xfrm::new(0.0, 0.0, 400.0, 200.0)), kind: ShapeKind::Picture { fill }, ..Default::default() };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
        let _ = render_slide(&p, 0, &RenderOpts { scale: 0.25, ..Default::default() });
    }
}

#[test]
fn an_object_deckcraft_doesnt_model_shows_its_preview_picture() {
    // An embedded OLE object (or an unknown graphic) draws its preview stretched over its frame,
    // as PowerPoint does; with no preview it draws nothing.
    let png = {
        let mut out = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 2, image::Rgb([200, 20, 20])))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    };
    for with_preview in [true, false] {
        let mut p = Presentation::default();
        let media = p.add_media("image1.png", "image/png", png.clone()).expect("media");
        let sh = Shape {
            id: ShapeId(600),
            xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)),
            kind: ShapeKind::Opaque { xml: String::new(), preview: with_preview.then_some(media), label: "Package".into(), parts: vec![] },
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        for (x, y) in [(110, 110), (300, 200), (490, 290)] {
            let px = img.pixel(x, y);
            let red = px[0] > 180 && px[1] < 60 && px[2] < 60;
            assert_eq!(red, with_preview, "({x}, {y}) {px:?}");
        }
        assert!(img.pixel(520, 200)[1] > 200, "nothing outside the frame");
    }
}

/// #270: a filled shape half off the slide's left edge, and one wholly below it.
fn off_slide_deck() -> Presentation {
    let half = Shape {
        id: ShapeId(2700),
        xfrm: Some(Xfrm::new(-100.0, 100.0, 200.0, 100.0)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    };
    let below = Shape {
        id: ShapeId(2701),
        xfrm: Some(Xfrm::new(400.0, 600.0, 100.0, 60.0)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent2)),
        ..Default::default()
    };
    deck_with(vec![half, below])
}

fn near(px: [u8; 4], c: deckcraft_color::Rgba) -> bool {
    (px[0] as i32 - c.r as i32).abs() <= 3 && (px[1] as i32 - c.g as i32).abs() <= 3 && (px[2] as i32 - c.b as i32).abs() <= 3 && px[3] == 255
}

#[test]
fn shows_exports_and_thumbnails_clip_to_the_slide() {
    let p = off_slide_deck();
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    assert_eq!((img.width, img.height), (960, 540));
    let a1 = p.masters[0].theme.colors.get(deckcraft_color::SchemeSlot::Accent1);
    // The on-slide half is there, nothing else is drawn outside the slide.
    assert!(near(img.pixel(50, 150), a1), "{:?}", img.pixel(50, 150));
    assert_eq!(img.pixel(150, 150), [255, 255, 255, 255]);
}

#[test]
fn the_editor_pasteboard_draws_shapes_off_the_slide() {
    let p = off_slide_deck();
    let extent = slide_extent(&p, 0);
    let region = pasteboard_region((960, 540), 1.0, extent, Region { x: -300, y: -200, w: 1560, h: 1000 });
    assert!(region.x <= -100 && region.y == 0 && region.y as i64 + region.h as i64 >= 660, "{region:?}");
    let opts = RenderOpts { scale: 1.0, edit: true, offset: region.offset(), size: Some(region.size()), ..Default::default() };
    let (img, drawn) = render_slide_drawn(&p, 0, &opts);
    assert_eq!((img.width, img.height), region.size());
    let at = |x: i32, y: i32| img.pixel((x - region.x) as u32, (y - region.y) as u32);
    let a1 = p.masters[0].theme.colors.get(deckcraft_color::SchemeSlot::Accent1);
    let a2 = p.masters[0].theme.colors.get(deckcraft_color::SchemeSlot::Accent2);
    // Both halves of the straddling shape, at full strength, and the shape below the slide.
    assert!(near(at(-50, 150), a1), "{:?}", at(-50, 150));
    assert!(near(at(50, 150), a1), "{:?}", at(50, 150));
    assert!(near(at(450, 630), a2), "{:?}", at(450, 630));
    // The pasteboard itself stays clear (the UI's own colour shows through); the slide is white.
    assert_eq!(at(-50, 50), [0, 0, 0, 0]);
    assert_eq!(at(200, 300), [255, 255, 255, 255]);
    // The drawing's reach, in slide space.
    assert!(drawn.x0 <= -100.0 && drawn.y1 >= 660.0 && drawn.x1 >= 960.0, "{drawn:?}");
}

#[test]
fn animation_state_skips_layout_and_master_shapes_sharing_a_slide_shape_id() {
    let mut p = Presentation::default();
    let bar = |x| Shape {
        id: ShapeId(77),
        xfrm: Some(Xfrm::new(x, 400.0, 100.0, 60.0)),
        fill: Some(Fill::solid(ColorRef::rgb(Rgba::rgb(200, 0, 0)))),
        ..Default::default()
    };
    let lid = p.slides[0].layout;
    let m = std::sync::Arc::make_mut(&mut p.masters[0]);
    m.shapes.push(bar(100.0));
    m.layouts.iter_mut().find(|l| l.id == lid).expect("layout").shapes.push(bar(400.0));
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes.push(bar(700.0));
    let hide = |_: ShapeId| ShapeState { visible: false, ..Default::default() };
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, state: Some(&hide), ..Default::default() });
    for x in [150, 450] {
        assert_eq!(img.pixel(x, 430)[..3], [200, 0, 0], "the backdrop bar at x={x} stays");
    }
    assert_ne!(img.pixel(750, 430)[..3], [200, 0, 0], "the slide's own shape with that id is hidden");
}

#[test]
fn master_graphics_off_the_slide_show_on_the_pasteboard() {
    let mut p = Presentation::default();
    let m = std::sync::Arc::make_mut(&mut p.masters[0]);
    m.shapes.push(Shape {
        id: ShapeId(2702),
        xfrm: Some(Xfrm::new(-150.0, 300.0, 100.0, 60.0)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent3)),
        ..Default::default()
    });
    let region = pasteboard_region((960, 540), 1.0, slide_extent(&p, 0), Region { x: -300, y: -100, w: 1500, h: 800 });
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, edit: true, offset: region.offset(), size: Some(region.size()), ..Default::default() });
    let a3 = p.masters[0].theme.colors.get(deckcraft_color::SchemeSlot::Accent3);
    assert!(near(img.pixel((-100 - region.x) as u32, (330 - region.y) as u32), a3));
}

#[test]
fn text_spilling_out_of_its_box_below_the_slide_is_reported() {
    let mut sh = Shape { id: ShapeId(2703), xfrm: Some(Xfrm::new(100.0, 500.0, 400.0, 20.0)), ..Default::default() };
    let mut t = TextBody::from_text("one\ntwo\nthree\nfour\nfive");
    for para in &mut t.paragraphs {
        for r in &mut para.runs {
            r.props.size = Some(40.0);
        }
    }
    sh.text = Some(t);
    let p = deck_with(vec![sh]);
    // The box estimate doesn't see the text below the slide; the render does.
    assert!(slide_extent(&p, 0).y1 <= 540.0 + 4.0);
    let (_, drawn) = render_slide_drawn(&p, 0, &RenderOpts { scale: 0.5, ..Default::default() });
    assert!(drawn.y1 > 600.0, "{drawn:?}");
    // A slide with nothing off it reaches just the slide.
    let (_, drawn) = render_slide_drawn(&Presentation::default(), 0, &RenderOpts { scale: 0.5, edit: true, ..Default::default() });
    assert_eq!(drawn, Rect::new(0.0, 0.0, 960.0, 540.0));
}

/// A placeable WMF (1440 units per inch, bounds `w` × `h`) holding `records` (function, parameter
/// bytes).
fn wmf_with(w: i16, h: i16, records: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (func, params) in records {
        let mut p = params.clone();
        if p.len() % 2 == 1 {
            p.push(0);
        }
        body.extend(((6 + p.len()) as u32 / 2).to_le_bytes());
        body.extend(func.to_le_bytes());
        body.extend(p);
    }
    body.extend([3, 0, 0, 0, 0, 0]);
    let mut f = 0x9AC6_CDD7u32.to_le_bytes().to_vec();
    f.extend(0u16.to_le_bytes());
    for v in [0i16, 0, w, h] {
        f.extend(v.to_le_bytes());
    }
    f.extend(1440u16.to_le_bytes());
    f.extend(0u32.to_le_bytes());
    f.extend(0u16.to_le_bytes());
    let total = (f.len() + 18 + body.len()) / 2;
    for v in [1u16, 9, 0x300] {
        f.extend(v.to_le_bytes());
    }
    f.extend((total as u32).to_le_bytes());
    f.extend(16u16.to_le_bytes());
    f.extend(0u32.to_le_bytes());
    f.extend(0u16.to_le_bytes());
    f.extend(body);
    f
}

fn i16s(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| (*x as i16).to_le_bytes()).collect()
}

/// CREATEFONTINDIRECT (height, escapement, face) then SELECTOBJECT of it (handle 0), transparent
/// background, and the alignment `ta`.
fn font_records(height: i32, esc: i32, face: &str, ta: i32) -> Vec<(u16, Vec<u8>)> {
    let mut f = i16s(&[height, 0, esc, esc, 400]);
    f.extend([0, 0, 0, if face.contains("dings") { 2 } else { 0 }, 0, 0, 0, 0]);
    f.extend(face.as_bytes());
    f.push(0);
    vec![(0x02FB, f), (0x012D, i16s(&[0])), (0x0102, i16s(&[1])), (0x012E, i16s(&[ta]))]
}

/// EXTTEXTOUT at (x, y) with an optional dx array.
fn text_record(x: i32, y: i32, s: &str, dx: &[i32]) -> (u16, Vec<u8>) {
    let mut v = i16s(&[y, x, s.len() as i32, 0]);
    v.extend(s.as_bytes());
    if s.len() % 2 == 1 {
        v.push(0);
    }
    v.extend(i16s(dx));
    (0x0A32, v)
}

/// Bounding box (x0, y0, x1, y1) of the pixels at least half opaque and darker than mid-grey.
fn mf_ink(img: &image::RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 128 && (p[0] as u32 + p[1] as u32 + p[2] as u32) < 384 {
            b = Some(match b {
                None => (x, y, x, y),
                Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x), e.max(y)),
            });
        }
    }
    b
}

#[test]
fn metafile_dashed_pen_leaves_gaps() {
    // CREATEPENINDIRECT PS_DASH | PS_ENDCAP_FLAT, 40 units (4 px) wide, black, then a line across
    // the picture at y = 72 px: 12 px dashes with 4 px gaps.
    let mut pen = i16s(&[0x0201, 40, 0]);
    pen.extend(0u32.to_le_bytes());
    let recs = vec![(0x02FA, pen), (0x012D, i16s(&[0])), (0x0214, i16s(&[720, 100])), (0x0213, i16s(&[720, 2780]))];
    let img = crate::metafile::rasterize(&wmf_with(2880, 1440, &recs), 288).expect("raster");
    let ink: Vec<bool> = (20..260).map(|x| img.get_pixel(x, 72)[3] > 128).collect();
    assert!(ink.iter().filter(|b| **b).count() > 120, "dashes are drawn");
    assert!(ink.iter().filter(|b| !**b).count() > 30, "with gaps between them");
}

#[test]
fn metafile_skewed_bitmap_is_drawn_with_its_transform() {
    use craft_metafile::write::{Image as MetafileImage, Op, emf};
    use kurbo::{Affine, Rect};

    let image = std::sync::Arc::new(MetafileImage { width: 2, height: 2, rgba: [255, 0, 0, 255].repeat(4) });
    let xf = Affine::translate((25.0, 25.0)) * Affine::new([1.0, 0.25, 0.2, 1.0, 0.0, 0.0]) * Affine::scale(24.0);
    let bytes = emf(&[Op::Image { image, xf, opacity: 1.0 }], Rect::new(0.0, 0.0, 100.0, 100.0), "DeckCraft", "skewed bitmap").expect("EMF");
    let img = crate::metafile::rasterize(&bytes, 160).expect("raster");
    let red: Vec<_> = img.enumerate_pixels().filter(|(_, _, p)| p[0] > 180 && p[1] < 80 && p[3] > 128).collect();
    assert!(red.len() > 100, "the transformed image is visible: {} red pixels", red.len());
    let (min_x, max_x) = red.iter().fold((u32::MAX, 0), |(lo, hi), (x, _, _)| (lo.min(*x), hi.max(*x)));
    let (min_y, max_y) = red.iter().fold((u32::MAX, 0), |(lo, hi), (_, y, _)| (lo.min(*y), hi.max(*y)));
    assert!(max_x - min_x > 25 && max_y - min_y > 25, "the image keeps both transformed axes: x={min_x}..{max_x}, y={min_y}..{max_y}");
}

#[test]
fn metafile_clip_path_and_nondefault_pen_geometry_are_applied() {
    use craft_metafile::write::{Op, Pen, emf};
    use craft_metafile::{Cap, Join};
    use kurbo::{BezPath, Rect, Shape as _};

    let mut triangle = BezPath::new();
    triangle.move_to((10.0, 10.0));
    triangle.line_to((90.0, 10.0));
    triangle.line_to((50.0, 90.0));
    triangle.close_path();

    let mut bevel = BezPath::new();
    bevel.move_to((15.0, 35.0));
    bevel.line_to((50.0, 60.0));
    bevel.line_to((85.0, 35.0));
    let mut miter = BezPath::new();
    miter.move_to((15.0, 45.0));
    miter.line_to((50.0, 70.0));
    miter.line_to((85.0, 45.0));
    let pen = |join| Pen { rgb: [0, 0, 0], width: 5.0, cap: Cap::Square, join, miter: 4.0, dash: None };
    let ops = [
        Op::Clip { path: triangle, even_odd: false },
        Op::Fill { path: Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.1), even_odd: false, rgb: [255, 0, 0] },
        Op::Stroke { path: bevel, pen: pen(Join::Bevel) },
        Op::Stroke { path: miter, pen: pen(Join::Miter) },
        Op::Unclip,
    ];
    let bytes = emf(&ops, Rect::new(0.0, 0.0, 100.0, 100.0), "DeckCraft", "clipped strokes").expect("EMF");
    let img = crate::metafile::rasterize(&bytes, 160).expect("raster");
    assert!(img.get_pixel(80, 40)[0] > 180, "inside the clip path is filled");
    assert_eq!(img.get_pixel(8, 80)[3], 0, "outside the clip path remains transparent");
    assert!(img.pixels().any(|p| p[3] > 128 && p[0] < 80), "square capped bevel and miter strokes are rendered");
}

#[test]
fn metafile_text_ink_lands_at_its_reference_point() {
    // 2 × 1 inch (144 × 72 pt) drawn 288 px wide: 2 px per point. A 36-point "HH", top-left
    // aligned at (72, 18) pt.
    let mut recs = font_records(-720, 0, "Arial", 0);
    recs.push(text_record(1440, 360, "HH", &[]));
    let img = crate::metafile::rasterize(&wmf_with(2880, 1440, &recs), 288).expect("raster");
    let (x0, y0, x1, y1) = mf_ink(&img).expect("text ink");
    assert!((144..156).contains(&x0), "left edge at the reference point: {x0}");
    assert!(x1 > x0 + 40 && x1 < 288, "two capitals wide: {x1}");
    // The cell's top is at 36 px; a capital starts below the cell top and sits on the baseline.
    assert!((38..72).contains(&y0), "cap top below the cell top: {y0}");
    assert!((88..112).contains(&y1), "baseline one ascent below the top: {y1}");
}

#[test]
fn metafile_text_centre_baseline_and_escapement() {
    // TA_CENTER | TA_BASELINE at (72, 36) pt: the ink is centred on 144 px and sits on 72 px.
    let mut recs = font_records(-720, 0, "Arial", 24 | 6);
    recs.push(text_record(1440, 720, "HH", &[]));
    let img = crate::metafile::rasterize(&wmf_with(2880, 1440, &recs), 288).expect("raster");
    let (x0, _, x1, y1) = mf_ink(&img).expect("ink");
    assert!(((x0 + x1) as i32 / 2 - 144).abs() <= 4, "centred: {x0}..{x1}");
    assert!((69..=74).contains(&y1), "on the baseline: {y1}");
    // Escapement 90°: the string runs up the picture.
    let mut recs = font_records(-360, 900, "Arial", 24);
    recs.push(text_record(1440, 1300, "HHHH", &[]));
    let img = crate::metafile::rasterize(&wmf_with(2880, 1440, &recs), 288).expect("raster");
    let (x0, y0, x1, y1) = mf_ink(&img).expect("ink");
    assert!(y1 - y0 > 2 * (x1 - x0), "a vertical run: {x0} {y0} {x1} {y1}");
    assert!(x1 <= 146 && y1 <= 132, "drawn up and left of the baseline origin (144, 130): {x1} {y1}");
}

#[test]
fn metafile_text_follows_its_dx_array() {
    // "HH" with the second H a full inch (144 px) after the first: two clumps of ink, a gap between.
    let mut recs = font_records(-360, 0, "Arial", 24);
    recs.push(text_record(360, 720, "HH", &[1440, 100]));
    let img = crate::metafile::rasterize(&wmf_with(2880, 1440, &recs), 288).expect("raster");
    let col_has_ink = |x: u32| (0..img.height()).any(|y| img.get_pixel(x, y)[3] > 128);
    assert!((36..56).any(col_has_ink), "first H at 36 px");
    assert!((180..200).any(col_has_ink), "second H one inch later");
    assert!(!(70..170).any(col_has_ink), "nothing between them");
}

#[test]
fn metafile_symbol_text_draws_a_bullet_not_a_box() {
    // Webdings "n" is a filled circle (installed, or mapped to a black circle where it is not), as
    // is Wingdings "l" in a symbol font that is not installed anywhere.
    for (face, s) in [("Webdings", "n"), ("Wingdings Not Installed", "l")] {
        let mut recs = font_records(-1080, 0, face, 0);
        recs.push(text_record(360, 0, s, &[]));
        let img = crate::metafile::rasterize(&wmf_with(2880, 1440, &recs), 288).expect("raster");
        let (x0, y0, x1, y1) = mf_ink(&img).expect("ink");
        assert!(x1 - x0 > 10 && y1 - y0 > 10, "{face}: a sizeable mark: {x0} {y0} {x1} {y1}");
        let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
        assert!(img.get_pixel(cx, cy)[3] > 200, "{face}: filled in the middle");
        for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            assert!(img.get_pixel(x, y)[3] < 128, "{face}: round, not a box: corner ({x}, {y})");
        }
    }
}

#[test]
fn metafile_opaque_text_fills_its_cell() {
    // OPAQUE background in red behind a space.
    let mut recs = font_records(-720, 0, "Arial", 0);
    recs.push((0x0102, i16s(&[2])));
    recs.push((0x0201, 0x0000_00FFu32.to_le_bytes().to_vec()));
    recs.push(text_record(1440, 360, " ", &[]));
    let img = crate::metafile::rasterize(&wmf_with(2880, 1440, &recs), 288).expect("raster");
    let p = img.get_pixel(148, 60);
    assert!(p[0] > 200 && p[1] < 60 && p[3] > 200, "cell filled red: {p:?}");
    assert!(img.get_pixel(140, 60)[3] < 50, "nothing left of the cell");
}

#[test]
fn metafile_pattern_brush_tiles_and_averages_when_tiny() {
    // A 1-bit 8 × 8 checker (palette white, black), a null pen and a one-inch square.
    let mut dib: Vec<u8> = [40u32, 8, 8].iter().flat_map(|v| v.to_le_bytes()).collect();
    dib.extend(1u16.to_le_bytes());
    dib.extend(1u16.to_le_bytes());
    dib.extend([0u8; 24]);
    dib.extend([255, 255, 255, 0, 0, 0, 0, 0]);
    for r in 0..8 {
        dib.extend([if r % 2 == 0 { 0x55 } else { 0xAA }, 0, 0, 0]);
    }
    let mut brush = i16s(&[5, 0]);
    brush.extend(dib);
    let recs = vec![(0x0142, brush), (0x012D, i16s(&[0])), (0x012D, i16s(&[0x8008])), (0x041B, i16s(&[1440, 1440, 0, 0]))];
    let bytes = wmf_with(1440, 1440, &recs);
    // 4 px per point: a 0.75 pt cell is 3 px.
    let img = crate::metafile::rasterize(&bytes, 288).expect("raster");
    let (a, b) = (img.get_pixel(1, 1), img.get_pixel(4, 1));
    assert!(a[3] > 200 && b[3] > 200);
    assert!((a[0] as i32 - b[0] as i32).abs() > 200, "neighbouring cells alternate: {a:?} {b:?}");
    // Far below one pixel per cell: the average grey.
    let img = crate::metafile::rasterize(&bytes, 32).expect("raster");
    let p = img.get_pixel(16, 16);
    assert!((100..156).contains(&p[0]) && p[3] > 200, "average grey: {p:?}");
}

/// A 400 x 200 group at (100, 100) holding a dashed 4 pt rectangle and a 3 pt line with an
/// arrowhead, placed in the group's child coordinates by `to_child` (slide point to child point).
fn outlined_group(child: Xfrm, to_child: impl Fn(f64, f64) -> (f64, f64)) -> Image {
    use deckcraft_model::style::{Dash, LineEnd};
    let ink = |w: f64| deckcraft_model::style::Line { dash: None, ..deckcraft_model::style::Line::solid(ColorRef::rgb(Rgba::rgb(200, 0, 0)), w) };
    let place = |x0: f64, y0: f64, x1: f64, y1: f64| {
        let ((a, b), (c, d)) = (to_child(x0, y0), to_child(x1, y1));
        Xfrm::new(a, b, c - a, d - b)
    };
    let rect = Shape {
        id: ShapeId(11),
        xfrm: Some(place(140.0, 120.0, 300.0, 200.0)),
        geom: Geom::preset("rect"),
        line: Some(deckcraft_model::style::Line { dash: Some(Dash::Preset { name: "dash".into() }), ..ink(4.0) }),
        ..Default::default()
    };
    let end = LineEnd { kind: "triangle".into(), w: "lg".into(), len: "lg".into() };
    let line = Shape {
        id: ShapeId(12),
        xfrm: Some(Xfrm { flip_v: true, ..place(150.0, 170.0, 450.0, 260.0) }),
        geom: Geom::preset("line"),
        line: Some(deckcraft_model::style::Line { tail: Some(end), ..ink(3.0) }),
        ..Default::default()
    };
    let pts: Vec<(f64, f64, f32)> = [(160.0, 240.0), (260.0, 285.0)].iter().map(|(x, y)| to_child(*x, *y)).map(|(x, y)| (x, y, 1.0)).collect();
    let ink = Shape {
        id: ShapeId(13),
        xfrm: Some(place(100.0, 100.0, 500.0, 300.0)),
        kind: ShapeKind::Ink {
            strokes: vec![deckcraft_model::InkStroke { points: pts, color: Rgba::rgb(200, 0, 0), width: 5.0, highlighter: false }],
        },
        ..Default::default()
    };
    let group = Shape {
        id: ShapeId(10),
        xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)),
        kind: ShapeKind::Group { children: vec![rect, line, ink], child },
        ..Default::default()
    };
    render_slide(&deck_with(vec![group]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
}

fn inked(img: &Image) -> usize {
    img.pixels.as_chunks::<4>().0.iter().filter(|px| px[1] < 200).count()
}

#[test]
fn group_child_scale_does_not_scale_line_widths() {
    // Children laid out 1:1 with the slide are the reference.
    let reference = outlined_group(Xfrm::new(100.0, 100.0, 400.0, 200.0), |x, y| (x, y));
    let drawn = inked(&reference);
    assert!((500..20_000).contains(&drawn), "the reference draws thin lines: {drawn}");
    // The same drawing in a small legacy child space (scale 100), and one scaled unevenly
    // (100 across, 50 down): same outlines, same line widths, dashes and arrowhead.
    let uniform = outlined_group(Xfrm::new(1.0, 1.0, 4.0, 2.0), |x, y| (1.0 + (x - 100.0) / 100.0, 1.0 + (y - 100.0) / 100.0));
    let uneven = outlined_group(Xfrm::new(1.0, 1.0, 4.0, 4.0), |x, y| (1.0 + (x - 100.0) / 100.0, 1.0 + (y - 100.0) / 50.0));
    for (name, img) in [("uniform", &uniform), ("uneven", &uneven)] {
        let (n, d) = (inked(img), differing(&reference, img));
        assert!(n.abs_diff(drawn) * 20 < drawn, "{name}: {n} inked pixels against {drawn}");
        assert!(d * 20 < drawn, "{name}: {d} differing values against {drawn} inked pixels");
    }
}

/// #2067: a legend entry with its own text colour, size and weight (`c:legendEntry/c:txPr`) draws
/// its name so; the other entries keep the legend's.
#[test]
fn a_legend_entry_draws_its_own_text_colour_and_size() {
    let draw = |entry: Option<deckcraft_model::chart::LegendEntry>| {
        let mut c = Chart::sample(ChartType::Line);
        c.title = None;
        c.legend = Some("r".into());
        c.text_size = Some(10.0);
        c.text_color = Some(ColorRef::rgb(deckcraft_color::Rgba::rgb(128, 128, 128)));
        c.legend_entries = entry.into_iter().collect();
        let sh = Shape { id: ShapeId(634), xfrm: Some(Xfrm::new(0.0, 0.0, 480.0, 300.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let green = |img: &Image| {
        (0..480u32)
            .flat_map(|x| (0..300u32).map(move |y| (x, y)))
            .filter(|(x, y)| {
                let p = img.pixel(*x, *y);
                p[1] > 150 && p[0] < 80 && p[2] < 80
            })
            .count()
    };
    let entry = |size: Option<f64>| deckcraft_model::chart::LegendEntry {
        idx: 0,
        size,
        bold: Some(true),
        color: Some(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 200, 0))),
    };
    let none = green(&draw(None));
    let own = green(&draw(Some(entry(None))));
    let big = green(&draw(Some(entry(Some(20.0)))));
    assert_eq!(none, 0, "no green in a chart without the entry");
    assert!(own > 40, "the first entry's name is drawn in its own colour ({own})");
    assert!(big > own * 2, "and at its own size ({own} at 10 pt, {big} at 20 pt)");
}

#[test]
fn a_picture_and_an_embedded_objects_preview_draw_their_fill_behind_transparent_pixels() {
    // PowerPoint draws a picture's own fill behind its transparent pixels (and an old equation
    // object's fallback picture on its coloured box). Here the left half of the image is clear.
    let png = {
        let img = image::RgbaImage::from_fn(8, 4, |x, _| if x < 4 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([20, 20, 160, 255]) });
        let mut out = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let orange = Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 153, 0)));
    for (object, fill) in [(false, None), (false, Some(orange.clone())), (true, Some(orange.clone()))] {
        let mut p = Presentation::default();
        let media = p.add_media("half.png", "image/png", png.clone()).expect("media");
        let kind = if object {
            ShapeKind::Opaque { xml: String::new(), preview: Some(media), label: "Equation.3".into(), parts: vec![] }
        } else {
            ShapeKind::Picture { fill: deckcraft_model::style::PictureFill { media, ..Default::default() } }
        };
        let sh = Shape { id: ShapeId(630), xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)), kind, fill: fill.clone(), ..Default::default() };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        let (clear, opaque) = (img.pixel(200, 200), img.pixel(400, 200));
        assert!(opaque[2] > 140 && opaque[0] < 40, "the picture's own pixels ({object}): {opaque:?}");
        if fill.is_some() {
            assert_eq!(clear[..3], [255, 153, 0], "the fill behind the clear half ({object})");
            // A fill-colour emphasis recolours that fill, as it does any shape's.
            let tint = |_: ShapeId| ShapeState { tint: Some(deckcraft_color::Rgba::rgb(0, 160, 60)), ..Default::default() };
            let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, state: Some(&tint), ..Default::default() });
            assert_eq!(img.pixel(200, 200)[..3], [0, 160, 60], "the tinted fill behind the clear half ({object})");
        } else {
            assert!(clear[..3].iter().all(|v| *v > 240), "no fill: the slide shows through: {clear:?}");
        }
    }
}

#[test]
fn a_picture_in_a_placeholder_shows_the_layout_placeholders_fill_behind_transparent_pixels() {
    // #2068: a slide picture with an empty `p:spPr` in a layout picture placeholder that sets a
    // fill shows that fill behind its clear pixels. Its own fill (none included) and its own
    // style still decide when it has them. The left half of the image is clear.
    let png = {
        let img = image::RgbaImage::from_fn(8, 4, |x, _| if x < 4 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([20, 20, 160, 255]) });
        let mut out = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let orange = Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 153, 0)));
    let ph = || Some(Placeholder { kind: PhType::Picture, idx: 11, ..Default::default() });
    let style = ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1);
    // (layout fill, picture's own fill, picture's own style, the clear half shows orange)
    let cases = [
        (Some(orange.clone()), None, None, true),
        (None, None, None, false),
        (Some(orange.clone()), Some(Fill::None), None, false),
        (Some(orange.clone()), None, Some(style), false),
    ];
    for (i, (layout_fill, own_fill, own_style, shows)) in cases.into_iter().enumerate() {
        let mut p = Presentation::default();
        let media = p.add_media("half.png", "image/png", png.clone()).expect("media");
        let lid = p.slides[0].layout;
        let m = std::sync::Arc::make_mut(&mut p.masters[0]);
        let layout = m.layouts.iter_mut().find(|l| l.id == lid).expect("layout");
        layout.shapes.push(Shape {
            id: ShapeId(2),
            ph: ph(),
            xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)),
            fill: layout_fill,
            ..Default::default()
        });
        let pic = Shape {
            id: ShapeId(3),
            ph: ph(),
            kind: ShapeKind::Picture { fill: deckcraft_model::style::PictureFill { media, ..Default::default() } },
            fill: own_fill,
            style: own_style,
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![pic];
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        let (clear, opaque) = (img.pixel(200, 200), img.pixel(400, 200));
        assert!(opaque[2] > 140 && opaque[0] < 40, "case {i}: the picture's own pixels: {opaque:?}");
        if shows {
            assert_eq!(clear[..3], [255, 153, 0], "case {i}: the layout's fill behind the clear half");
        } else {
            assert!(clear[..3].iter().all(|v| *v > 240), "case {i}: the slide shows through: {clear:?}");
        }
    }
}

#[test]
fn a_picture_casts_its_shadow_from_its_pixels() {
    // PowerPoint shades only where a picture paints: a logo on a transparent background casts the
    // logo's shadow, not its frame's. Here the left half of the image is transparent.
    let png = {
        let img = image::RgbaImage::from_fn(8, 4, |x, _| if x < 4 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([20, 160, 20, 255]) });
        let mut out = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    };
    let shadow = deckcraft_model::style::Shadow {
        color: ColorRef::rgb(deckcraft_color::Rgba::BLACK),
        blur: 0.0,
        dist: 20.0,
        dir: 90.0,
        inner: false,
        sx: 1.0,
        sy: 1.0,
        kx: 0.0,
        ky: 0.0,
        align: String::new(),
        rotate_with_shape: false,
    };
    for as_fill in [false, true] {
        let mut p = Presentation::default();
        let media = p.add_media("logo.png", "image/png", png.clone()).expect("media");
        let pf = deckcraft_model::style::PictureFill { media, ..Default::default() };
        let mut sh = Shape { id: ShapeId(600), xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)), ..Default::default() };
        if as_fill {
            sh.fill = Some(Fill::Picture(pf));
            sh.line = Some(deckcraft_model::style::Line { fill: Some(Fill::None), ..Default::default() });
        } else {
            sh.kind = ShapeKind::Picture { fill: pf };
        }
        sh.effects = Some(deckcraft_model::Effects { outer_shadow: Some(shadow.clone()), ..Default::default() });
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        // Below the picture, 10 pt down: shadow under the opaque right half only.
        let left = img.pixel(200, 310);
        let right = img.pixel(400, 310);
        assert!(left[0] > 240, "no shadow under the transparent half ({as_fill}): {left:?}");
        assert!(right[0] < 40, "shadow under the opaque half ({as_fill}): {right:?}");
        // The transparent half shows the slide, not a shadow behind it.
        assert!(img.pixel(200, 200)[0] > 240, "{as_fill}");
    }
}

#[test]
fn text_in_a_scaled_group_keeps_its_point_size() {
    // A group's child coordinates scale its shapes' boxes, never their type: a 24 pt word in a
    // group whose child space is 1/1600 of its size (as old decks write them) or 4 times it is
    // 24 pt on the slide, and inside its box (it used to grow until the slide turned black).
    let draw = |scale: f64| {
        let mut p = Presentation::default();
        let mut body = TextBody::from_text("Hello");
        body.paragraphs[0].runs[0].props.size = Some(24.0);
        body.paragraphs[0].runs[0].props.fill = Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::BLACK)));
        let child = Shape { id: ShapeId(601), xfrm: Some(Xfrm::new(0.0, 0.0, 400.0 / scale, 200.0 / scale)), text: Some(body), ..Default::default() };
        let g = Shape {
            id: ShapeId(600),
            xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)),
            kind: ShapeKind::Group { children: vec![child], child: Xfrm::new(0.0, 0.0, 400.0 / scale, 200.0 / scale) },
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![g];
        render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let ((_, y0, _, y1), _) = ink(&draw(1.0));
    let plain = y1 - y0;
    assert!(plain > 10 && plain < 30, "24 pt word ink height {plain}");
    for scale in [1600.0, 4.0, 0.25] {
        let ((x0, sy0, x1, sy1), _) = ink(&draw(scale));
        assert!((sy1 - sy0).abs_diff(plain) <= 1, "scale {scale}: ink height {} vs {plain}", sy1 - sy0);
        assert!(x0 >= 100 && x1 <= 500 && sy0 >= 100 && sy1 <= 300, "scale {scale}: ink inside the box");
    }
}

#[test]
fn rounded_corners_in_a_stretched_group_stay_round() {
    // #1083: a group that stretches its child space 4x across makes a rounded rectangle wider on
    // the slide, but its corners stay circular, like a plain rounded rectangle of that size.
    let round = |id: u32, w: f64, h: f64| Shape {
        id: ShapeId(id),
        xfrm: Some(Xfrm::new(0.0, 0.0, w, h)),
        geom: Geom::Preset { name: "roundRect".into(), adj: vec![16667.0] },
        fill: Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::BLACK))),
        line: Some(deckcraft_model::style::Line { fill: Some(Fill::None), ..Default::default() }),
        ..Default::default()
    };
    let render = |sh: Shape| {
        let mut p = Presentation::default();
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
        render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let mut plain = round(700, 400.0, 100.0);
    plain.xfrm = Some(Xfrm::new(100.0, 100.0, 400.0, 100.0));
    let plain = render(plain);
    let grouped = render(Shape {
        id: ShapeId(701),
        xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 100.0)),
        kind: ShapeKind::Group { children: vec![round(702, 100.0, 100.0)], child: Xfrm::new(0.0, 0.0, 100.0, 100.0) },
        ..Default::default()
    });
    for (x, y) in [(120, 102), (102, 120), (480, 102), (498, 180), (120, 198)] {
        assert_eq!(plain.pixel(x, y), grouped.pixel(x, y), "pixel ({x},{y}) of a roundRect in a stretched group");
    }
}

#[test]
fn a_fill_overlay_blends_over_the_fill() {
    // `a:fillOverlay blend="mult"`: a 50% grey multiplied over a light fill darkens it; "screen"
    // lightens a dark one; the overlay stays inside the shape.
    use deckcraft_model::style::FillOverlay;
    let draw = |base: [u8; 3], blend: &str| {
        let mut p = Presentation::default();
        let mut sh = Shape {
            id: ShapeId(600),
            xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
            fill: Some(Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(base[0], base[1], base[2])))),
            line: Some(deckcraft_model::style::Line { fill: Some(Fill::None), ..Default::default() }),
            ..Default::default()
        };
        sh.effects = Some(deckcraft_model::Effects {
            fill_overlay: Some(FillOverlay { blend: blend.into(), fill: Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(128, 128, 128))) }),
            ..Default::default()
        });
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        (img.pixel(200, 150), img.pixel(50, 50))
    };
    let (mult, outside) = draw([200, 100, 50], "mult");
    assert!((mult[0] as i32 - 100).abs() <= 3 && (mult[1] as i32 - 50).abs() <= 3 && (mult[2] as i32 - 25).abs() <= 3, "{mult:?}");
    assert_eq!(&outside[..3], &[255, 255, 255], "nothing outside the shape");
    let (screen, _) = draw([0, 0, 0], "screen");
    assert!((screen[0] as i32 - 128).abs() <= 3, "{screen:?}");
    let (over, _) = draw([200, 100, 50], "over");
    assert!((over[0] as i32 - 128).abs() <= 3 && (over[1] as i32 - 128).abs() <= 3, "{over:?}");
}

#[test]
fn dotted_and_wavy_underlines_draw_their_pattern() {
    // Underlined spaces only, so the underline is the only ink: a red line under "      ".
    let row_runs = |u: &str| {
        let mut sh = Shape { id: ShapeId(760), xfrm: Some(Xfrm::new(0.0, 0.0, 900.0, 300.0)), text_box: true, ..Default::default() };
        let mut t = TextBody::from_text("            ");
        let props = &mut t.paragraphs[0].runs[0].props;
        props.size = Some(96.0);
        props.underline = Some(u.into());
        props.fill = Some(deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0))));
        sh.text = Some(t);
        let p = deck_with(vec![sh]);
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        let red = |x: u32, y: u32| {
            let px = img.pixel(x, y);
            px[0] > 150 && px[1] < 120
        };
        // The ink's rows, and the number of separate red stretches along the most inked row.
        let rows: Vec<u32> = (0..300).filter(|&y| (0..900).any(|x| red(x, y))).collect();
        let best = rows.iter().copied().max_by_key(|&y| (0..900).filter(|&x| red(x, y)).count()).unwrap_or(0);
        let stretches = (1..900).filter(|&x| red(x, best) && !red(x - 1, best)).count();
        (rows.len(), stretches)
    };
    let (sng_rows, sng_runs) = row_runs("sng");
    assert_eq!(sng_runs, 1, "a single line is one stretch");
    let (_, dotted_runs) = row_runs("dotted");
    assert!(dotted_runs > 10, "dots: {dotted_runs}");
    let (wavy_rows, _) = row_runs("wavy");
    assert!(wavy_rows > sng_rows, "a wave spans more rows ({wavy_rows}) than a single line ({sng_rows})");
}

#[test]
fn east_asian_vertical_text_stands_ideographs_upright() {
    // "一" is a horizontal stroke. In eaVert text it stays horizontal (upright) while the line runs
    // down the shape; in vert it lies on its side like Latin text.
    let ink = |dir: deckcraft_model::text::TextDir| {
        let mut sh = Shape { id: ShapeId(770), xfrm: Some(Xfrm::new(100.0, 50.0, 200.0, 400.0)), text_box: true, ..Default::default() };
        let mut t = TextBody::from_text("一");
        t.body.vert = Some(dir);
        let props = &mut t.paragraphs[0].runs[0].props;
        props.size = Some(80.0);
        props.fill = Some(deckcraft_model::style::Fill::solid(ColorRef::rgb(deckcraft_color::Rgba::rgb(0, 0, 0))));
        sh.text = Some(t);
        let p = deck_with(vec![sh]);
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..540 {
            for x in 0..960 {
                if img.pixel(x, y)[0] < 128 {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                }
            }
        }
        assert!(x1 >= x0 && y1 >= y0, "no ink for {dir:?}");
        (x1 - x0, y1 - y0)
    };
    let (w, h) = ink(deckcraft_model::text::TextDir::EaVertical);
    assert!(w > h * 3, "eaVert: upright stroke {w}x{h}");
    let (w, h) = ink(deckcraft_model::text::TextDir::Vertical);
    assert!(h > w * 3, "vert: stroke on its side {w}x{h}");
}

#[test]
fn placed_shapes_give_the_click_boxes_of_a_slide_in_drawing_order() {
    use deckcraft_model::text::{Action, Hyperlink};
    let link =
        Hyperlink { action: Action::NextSlide, tooltip: String::new(), highlight_click: false, sound: None, stop_sound: false, zoom_return: None };
    let rect = |id: u32, x: f64, click: Option<Hyperlink>| Shape {
        id: ShapeId(id),
        xfrm: Some(Xfrm::new(x, 50.0, 100.0, 40.0)),
        click,
        ..Default::default()
    };
    let group = Shape {
        id: ShapeId(10),
        xfrm: Some(Xfrm::new(300.0, 50.0, 200.0, 40.0)),
        click: Some(link.clone()),
        kind: ShapeKind::Group { children: vec![rect(11, 0.0, None), rect(12, 100.0, None)], child: Xfrm::new(0.0, 0.0, 200.0, 40.0) },
        ..Default::default()
    };
    let hidden = Shape { hidden: true, ..rect(13, 0.0, Some(link.clone())) };
    let p = deck_with(vec![rect(1, 0.0, None), rect(2, 150.0, Some(link.clone())), hidden, group]);
    let placed = place_slide(&p, 0);
    // Back to front, a group before its children; the hidden shape is not there.
    let ids: Vec<u32> = placed.shapes.iter().map(|s| s.id.0).collect();
    assert_eq!(ids, vec![1, 2, 10, 11, 12]);
    // Only the shapes with an action carry a link.
    let linked: Vec<u32> = placed.shapes.iter().filter(|s| s.link.is_some()).map(|s| s.id.0).collect();
    assert_eq!(linked, vec![2, 10]);
    // The children know their group, whose box covers them.
    let group = &placed.shapes[2];
    assert!(group.is_group && group.parent.is_none());
    for child in &placed.shapes[3..] {
        assert_eq!(child.parent, Some(2));
        assert!(child.rect.x0 >= group.rect.x0 - 0.01 && child.rect.x1 <= group.rect.x1 + 0.01);
    }
    // Shape actions come out as links owned by their shape, not as text links.
    assert!(placed.links.iter().all(|l| !l.text));
    assert_eq!(placed.links.iter().map(|l| l.shape.0).collect::<Vec<_>>(), vec![2, 10]);
}

#[test]
fn a_text_hyperlink_belongs_to_the_shape_that_holds_the_text() {
    use deckcraft_model::text::{Action, Hyperlink};
    let mut body = TextBody::from_text("click here");
    for r in &mut body.paragraphs[0].runs {
        r.props.link = Some(Hyperlink {
            action: Action::FirstSlide,
            tooltip: String::new(),
            highlight_click: false,
            sound: None,
            stop_sound: false,
            zoom_return: None,
        });
    }
    let sh = Shape { id: ShapeId(7), xfrm: Some(Xfrm::new(100.0, 100.0, 300.0, 60.0)), text: Some(body), ..Default::default() };
    let placed = place_slide(&deck_with(vec![sh]), 0);
    assert!(placed.links.iter().any(|l| l.text && l.shape == ShapeId(7)));
    assert!(placed.shapes.iter().all(|s| s.link.is_none()), "a text link is not the shape's own action");
}

#[test]
fn metafile_intersect_clip_rect_hides_the_rest_of_the_picture() {
    let rec = |typ: u32, payload: &[u8]| {
        let mut v = typ.to_le_bytes().to_vec();
        v.extend(((8 + payload.len()) as u32).to_le_bytes());
        v.extend(payload);
        v
    };
    let rect = |l: i32, t: i32, r: i32, b: i32| [l, t, r, b].into_iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
    let mut body = rec(30, &rect(0, 0, 50, 50));
    body.extend(rec(43, &rect(0, 0, 100, 50)));
    let eof = rec(14, &[0; 12]);
    let total = 88 + body.len() + eof.len();
    let mut file = vec![0u8; 88];
    file[0..4].copy_from_slice(&1u32.to_le_bytes());
    file[4..8].copy_from_slice(&88u32.to_le_bytes());
    file[8..24].copy_from_slice(&rect(0, 0, 100, 50));
    file[40..44].copy_from_slice(&0x464D_4520u32.to_le_bytes());
    file[44..48].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    file[48..52].copy_from_slice(&(total as u32).to_le_bytes());
    file[52..56].copy_from_slice(&2u32.to_le_bytes());
    file[56..58].copy_from_slice(&1u16.to_le_bytes());
    file.extend(body);
    file.extend(eof);
    let img = crate::metafile::rasterize(&file, 100).expect("raster");
    let opaque = |x: u32, y: u32| img.get_pixel(x, y)[3] > 200;
    assert!(opaque(20, 25), "the clipped half is drawn");
    assert!(!opaque(80, 25), "past the clip the picture stays clear, pixel {:?}", img.get_pixel(80, 25));
}

#[test]
fn tiny_chart_does_not_blank_the_slide() {
    // A chart a few points wide used to panic in `f64::clamp` (gutter min above max), and the
    // panic guard then blanked the whole slide. Render directly, without the guard.
    let red = Shape {
        id: ShapeId(800),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    };
    let mut shapes = vec![red];
    for (i, k) in [ChartType::Column, ChartType::Bar, ChartType::Line, ChartType::Combo, ChartType::Scatter].iter().enumerate() {
        shapes.push(Shape {
            id: ShapeId(801 + i as u32),
            xfrm: Some(Xfrm::new(500.0 + i as f64 * 5.0, 300.0, 0.9, 0.9)),
            kind: ShapeKind::Chart(Box::new(Chart::sample(*k))),
            ..Default::default()
        });
    }
    let p = deck_with(shapes);
    let img = RENDERER.with(|r| r.borrow_mut().slide(&p, &p.slides[0], 0, &RenderOpts { scale: 1.0, ..Default::default() }));
    let px = img.pixel(200, 150);
    assert_ne!(px, [255, 255, 255, 255], "the other shape must still draw");
    assert_eq!(crate::chart::clamp_soft(5.0, 10.5, 0.276), 0.276);
}

/// Shapes with depth under turned cameras (an `extrusionH` and, on two, an `extrusionClr`).
fn extruded_deck() -> Presentation {
    let mk = |id: u32, x: f64, preset: &str, raw: &str| Shape {
        id: ShapeId(id),
        xfrm: Some(Xfrm::new(x, 170.0, 170.0, 130.0)),
        geom: Geom::preset(preset),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        effects: Some(deckcraft_model::Effects { raw3d: Some(raw.into()), ..Default::default() }),
        ..Default::default()
    };
    deck_with(vec![
        mk(
            700,
            40.0,
            "rect",
            r#"<a:scene3d><a:camera prst="isometricOffAxis2Left"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="381000"/>"#,
        ),
        mk(
            701,
            280.0,
            "ellipse",
            r#"<a:scene3d><a:camera prst="isometricTopUp"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="254000" contourW="63500"><a:extrusionClr><a:srgbClr val="C00000"/></a:extrusionClr><a:contourClr><a:srgbClr val="FFC000"/></a:contourClr></a:sp3d>"#,
        ),
        mk(
            702,
            520.0,
            "star5",
            r#"<a:scene3d><a:camera prst="perspectiveContrastingLeftFacing"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="508000"><a:extrusionClr><a:schemeClr val="accent2"/></a:extrusionClr></a:sp3d>"#,
        ),
        mk(
            703,
            760.0,
            "rect",
            r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="381000" contourW="76200" prstMaterial="metal"><a:contourClr><a:srgbClr val="00B050"/></a:contourClr></a:sp3d>"#,
        ),
    ])
}

#[test]
fn contours_band_the_outline_in_their_colour() {
    let img = render_slide(&extruded_deck(), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let n = |x0: u32, x1: u32, f: &dyn Fn([u8; 4]) -> bool| {
        let mut k = 0;
        for y in 0..img.height {
            for x in x0..x1.min(img.width) {
                let i = ((y * img.width + x) * 4) as usize;
                if img.pixels.get(i..i + 4).and_then(|p| <[u8; 4]>::try_from(p).ok()).is_some_and(f) {
                    k += 1;
                }
            }
        }
        k
    };
    // The ellipse's yellow contour and the flat rectangle's green one.
    assert!(n(260, 480, &|p| p[0] > 200 && (150..230).contains(&p[1]) && p[2] < 80) > 200);
    assert!(n(740, 960, &|p| p[0] < 80 && p[1] > 120 && p[2] < 120) > 300);
}

#[test]
fn extrusions_show_their_sides_under_a_turned_camera() {
    let p = extruded_deck();
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    if let Ok(path) = std::env::var("DECKCRAFT_PNG_OUT") {
        let _ = std::fs::write(path, img.to_png());
    }
    let count = |x0: u32, x1: u32, f: &dyn Fn([u8; 4]) -> bool| {
        (x0..x1).flat_map(|x| (100..400).map(move |y| (x, y))).filter(|(x, y)| f(img.pixel(*x, *y))).count()
    };
    // The red sides of the ellipse; the dark accent-1 sides of the cube; none for a front camera.
    assert!(count(250, 500, &|c| c[0] > 100 && c[1] < 40 && c[2] < 40) > 300, "red extrusion");
    let flat = render_slide(&deck_with(vec![]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let ink = |img: &Image, x0: u32, x1: u32| {
        (x0..x1).flat_map(|x| (100..400).map(move |y| (x, y))).filter(|(x, y)| img.pixel(*x, *y) != [255, 255, 255, 255]).count()
    };
    let mut no_depth = extruded_deck();
    for sh in &mut std::sync::Arc::make_mut(&mut no_depth.slides[0]).shapes {
        if let Some(fx) = sh.effects.as_mut() {
            fx.raw3d = fx.raw3d.as_ref().map(|r| r.replace(" extrusionH=", " x="));
        }
    }
    let thin = render_slide(&no_depth, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    assert!(ink(&img, 0, 260) > ink(&thin, 0, 260) + 1000, "the cube's sides add ink");
    assert_eq!(ink(&img, 740, 960), ink(&thin, 740, 960), "a front camera shows no sides");
    assert_eq!(ink(&flat, 0, 960), 0);
}

#[test]
fn group_child_scale_does_not_scale_3d_depth_contours_or_bevels() {
    // A legacy group child space (here 100 points per unit) scales the shapes' boxes, but
    // extrusion depths, contour widths and bevels are points: a 1 pt contour on a small extruded
    // cube in such a group was drawn hundreds of points wide, a band across the whole slide.
    let mut deck = extruded_deck();
    let bevelled = Shape {
        id: ShapeId(704),
        xfrm: Some(Xfrm::new(40.0, 340.0, 170.0, 130.0)),
        geom: Geom::preset("rect"),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        effects: Some(deckcraft_model::Effects {
            raw3d: Some(r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d><a:bevelT w="254000" h="254000"/></a:sp3d>"#.into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    std::sync::Arc::make_mut(&mut deck.slides[0]).shapes.push(bevelled);
    let shapes = deck.slides[0].shapes.clone();
    let grouped = |k: f64| {
        let children = shapes
            .iter()
            .map(|s| {
                let x = s.xfrm.unwrap_or_default();
                Shape { xfrm: Some(Xfrm::new(x.x / k, x.y / k, x.w / k, x.h / k)), ..s.clone() }
            })
            .collect();
        let group = Shape {
            id: ShapeId(10),
            xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)),
            kind: ShapeKind::Group { children, child: Xfrm::new(0.0, 0.0, 960.0 / k, 540.0 / k) },
            ..Default::default()
        };
        render_slide(&deck_with(vec![group]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let reference = grouped(1.0);
    let ink = |img: &Image| img.pixels.as_chunks::<4>().0.iter().filter(|p| **p != [255, 255, 255, 255]).count();
    let drawn = ink(&reference);
    assert!(drawn > 20_000, "the reference draws the shapes: {drawn}");
    let scaled = grouped(100.0);
    let (n, d) = (ink(&scaled), differing(&reference, &scaled));
    assert!(n.abs_diff(drawn) * 20 < drawn, "{n} inked pixels against {drawn}");
    assert!(d * 20 < drawn, "{d} differing values against {drawn} inked pixels");
}

#[test]
fn a_turned_camera_in_a_one_to_one_group_draws_its_3d_sizes_as_outside_one() {
    // The camera's turn is part of the shape's transform; its foreshortening is not a group scale,
    // so a 1:1 group must not change extrusion depths, contours or bevels under a turned camera.
    let mut deck = extruded_deck();
    let bevelled = Shape {
        id: ShapeId(705),
        xfrm: Some(Xfrm::new(40.0, 340.0, 170.0, 130.0)),
        geom: Geom::preset("rect"),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        effects: Some(deckcraft_model::Effects {
            raw3d: Some(r#"<a:scene3d><a:camera prst="isometricOffAxis1Right"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="254000" contourW="38100"><a:bevelT w="254000" h="254000"/></a:sp3d>"#.into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    std::sync::Arc::make_mut(&mut deck.slides[0]).shapes.push(bevelled);
    let opts = RenderOpts { scale: 1.0, ..Default::default() };
    let alone = render_slide(&deck, 0, &opts);
    let group = Shape {
        id: ShapeId(10),
        xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)),
        kind: ShapeKind::Group { children: deck.slides[0].shapes.clone(), child: Xfrm::new(0.0, 0.0, 960.0, 540.0) },
        ..Default::default()
    };
    let grouped = render_slide(&deck_with(vec![group]), 0, &opts);
    let d = differing(&alone, &grouped);
    assert!(d < 200, "{d} values differ between the shapes alone and in a 1:1 group");
}

#[test]
fn table_rows_below_the_slide_are_not_laid_out_and_the_slide_looks_the_same() {
    // A fuzzed deck's 200 x 200 table of long text took 70 s to draw, most of it off the slide.
    let table = |rows: usize| {
        let mut t = Table::new(rows, 2, 400.0, 30.0);
        for r in 0..rows {
            for c in 0..2 {
                t.cell_mut(r, c).unwrap().text = TextBody::from_text(&format!("Row {r} col {c}"));
            }
        }
        Shape { id: ShapeId(611), xfrm: Some(Xfrm::new(50.0, 50.0, 400.0, 30.0 * rows as f64)), kind: ShapeKind::Table(t), ..Default::default() }
    };
    let opts = RenderOpts { scale: 0.5, ..Default::default() };
    crate::table::TEXT_CELLS_DRAWN.with(|n| n.set(0));
    let big = render_slide(&deck_with(vec![table(500)]), 0, &opts);
    let drawn = crate::table::TEXT_CELLS_DRAWN.with(|n| n.get());
    assert!(drawn > 2 * 10 && drawn < 2 * 40, "{drawn} cells drawn");
    // 30 rows already run past the slide's bottom: the same picture.
    let small = render_slide(&deck_with(vec![table(30)]), 0, &opts);
    assert_eq!((big.width, big.height), (small.width, small.height));
    let diff = (0..big.height).flat_map(|y| (0..big.width).map(move |x| (x, y))).filter(|&(x, y)| big.pixel(x, y) != small.pixel(x, y)).count();
    assert_eq!(diff, 0);
}

#[test]
fn office_2016_charts_draw_their_own_layouts() {
    let chart = |kind: ChartType, vals: &[f64], subtotals: Vec<usize>| {
        let c = Chart {
            kind,
            title: None,
            legend: None,
            gridlines: false,
            subtotals,
            categories: (0..vals.len()).map(|i| format!("C{i}")).collect(),
            series: vec![deckcraft_model::chart::Series { values: vals.iter().map(|v| Some(*v)).collect(), ..Default::default() }],
            ..Default::default()
        };
        let sh = Shape { id: ShapeId(621), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let ink = |img: &crate::Image, x: u32, y: u32| {
        let p = img.pixel(x, y);
        p[0] < 235 || p[1] < 235 || p[2] < 235
    };
    // Waterfall: the decrease floats (nothing under it near the axis), the total stands on it.
    let img = chart(ChartType::Waterfall, &[100.0, 40.0, -25.0, 115.0], vec![3]);
    let plot_bottom = (300..530).rev().find(|&y| ink(&img, 120, y)).unwrap_or(500);
    let band = |i: f64| (80.0 + (900.0 - 80.0) * (i + 0.5) / 4.0) as u32;
    assert!(!ink(&img, band(2.0), plot_bottom - 20), "the decrease bar reaches the axis");
    assert!(ink(&img, band(3.0), plot_bottom - 20), "the total bar doesn't stand on the axis");
    assert!(ink(&img, band(1.0), 100), "the increase bar is missing");
    // Funnel, treemap and sunburst fill the middle of the chart.
    for kind in [ChartType::Funnel, ChartType::Treemap, ChartType::Sunburst] {
        let img = chart(kind, &[50.0, 30.0, 20.0], vec![]);
        let covered = (0..540).step_by(10).flat_map(|y| (0..960).step_by(10).map(move |x| (x, y))).filter(|&(x, y)| ink(&img, x, y)).count();
        assert!(covered > 1000, "{kind:?}: {covered} samples inked");
    }
}

/// Histograms draw touching columns of their bins, Pareto charts add the cumulative line, and
/// box and whisker charts draw a box per group and series.
#[test]
fn histogram_pareto_and_box_charts_draw() {
    let shot = |c: Chart| {
        let sh = Shape { id: ShapeId(622), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let inked = |img: &crate::Image| {
        (0..540)
            .step_by(6)
            .flat_map(|y| (0..960).step_by(6).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = img.pixel(x, y);
                p[0] < 200 || p[1] < 200 || p[2] < 200
            })
            .count()
    };
    let mut empty = Chart::sample(ChartType::Histogram);
    empty.series[0].values.clear();
    let blank = inked(&shot(empty));
    for kind in [ChartType::Histogram, ChartType::Pareto, ChartType::BoxWhisker] {
        let img = shot(Chart::sample(kind));
        if let Ok(dir) = std::env::var("DECKCRAFT_CHART_SHOTS") {
            let _ = std::fs::write(format!("{dir}/{kind:?}.png"), img.to_png());
        }
        let n = inked(&img);
        assert!(n > blank + 400, "{kind:?}: {n} inked samples (blank {blank})");
    }
}

/// Multi-level treemaps and sunbursts draw their groups: the treemap's group banners and the
/// sunburst's inner ring of groups, coloured by top-level group.
#[test]
fn multilevel_treemap_and_sunburst_draw_their_groups() {
    for kind in [ChartType::Treemap, ChartType::Sunburst] {
        let c = Chart::sample(kind);
        let sh = Shape { id: ShapeId(623), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
        if let Ok(dir) = std::env::var("DECKCRAFT_CHART_SHOTS") {
            let _ = std::fs::write(format!("{dir}/{kind:?}-levels.png"), img.to_png());
        }
        // Distinct colours: one hue per top-level group plus lighter children.
        let mut seen: Vec<[u8; 3]> = Vec::new();
        for y in (60..520).step_by(8) {
            for x in (40..920).step_by(8) {
                let p = img.pixel(x, y);
                let c = [p[0] / 16, p[1] / 16, p[2] / 16];
                if c != [15, 15, 15] && !seen.contains(&c) {
                    seen.push(c);
                }
            }
        }
        assert!(seen.len() >= 6, "{kind:?}: {} colours", seen.len());
    }
}

/// A region map without drawable geography falls back to coloured tiles on the value scale.
#[test]
fn region_map_draws_tiles_on_its_colour_scale() {
    let names = ["Brest", "Vitebsk", "Gomel", "Grodno", "Minsk", "Mogilev", "Minsk City"];
    let vals = [3.1, 2.2, 2.9, 4.8, 6.0, 1.4, 9.5];
    let c = Chart {
        kind: ChartType::RegionMap,
        title: Some("Sales by region".into()),
        categories: names.iter().map(|s| s.to_string()).collect(),
        series: vec![deckcraft_model::chart::Series { name: "Sales".into(), values: vals.iter().map(|v| Some(*v)).collect(), ..Default::default() }],
        data_labels: true,
        ..Default::default()
    };
    let sh = Shape { id: ShapeId(624), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
    let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    if let Ok(dir) = std::env::var("DECKCRAFT_CHART_SHOTS") {
        let _ = std::fs::write(format!("{dir}/RegionMap-tiles.png"), img.to_png());
    }
    let mut seen: Vec<[u8; 3]> = Vec::new();
    for y in (60..520).step_by(6) {
        for x in (40..920).step_by(6) {
            let p = img.pixel(x, y);
            let c = [p[0] / 16, p[1] / 16, p[2] / 16];
            if c != [15, 15, 15] && !seen.contains(&c) {
                seen.push(c);
            }
        }
    }
    assert!(seen.len() >= 5, "{} colours", seen.len());
}

/// Radar charts are polar, stock charts draw high-low lines (and up/down bars), and a combo
/// chart draws its line series over its columns.
#[test]
fn radar_stock_and_combo_draw_their_own_layouts() {
    let mut ohlc = Chart::sample(ChartType::Stock);
    ohlc.series.insert(
        0,
        deckcraft_model::chart::Series {
            name: "Open".into(),
            values: [44.0, 25.0, 38.0, 50.0, 35.0].iter().map(|v| Some(*v)).collect(),
            ..Default::default()
        },
    );
    ohlc.up_down_bars = true;
    let charts = [
        ("Radar", Chart::sample(ChartType::Radar)),
        ("FilledRadar", Chart::sample(ChartType::FilledRadar)),
        ("Stock", Chart::sample(ChartType::Stock)),
        ("StockOHLC", ohlc),
        ("Combo", Chart::sample(ChartType::Combo)),
    ];
    for (name, c) in charts {
        let sh = Shape {
            id: ShapeId(625),
            xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)),
            kind: ShapeKind::Chart(Box::new(c.clone())),
            ..Default::default()
        };
        let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() });
        if let Ok(dir) = std::env::var("DECKCRAFT_CHART_SHOTS") {
            let _ = std::fs::write(format!("{dir}/{name}.png"), img.to_png());
        }
        let ink = |x: u32, y: u32| img.pixel(x, y)[..3] != [255, 255, 255];
        match name {
            // The radar's top spoke runs straight up from the centre; a line chart has nothing there.
            "Radar" | "FilledRadar" => assert!((150..260).any(|y| ink(480, y)), "{name}"),
            // No series lines: well right of the first category there is only gridline grey.
            "Stock" | "StockOHLC" => {
                let coloured = (60..480).step_by(2).flat_map(|y| (40..920).step_by(4).map(move |x| (x, y))).any(|(x, y)| {
                    let p = img.pixel(x, y);
                    p[2] > p[0].saturating_add(60)
                });
                assert!(!coloured || name == "Stock", "{name}: series drawn as lines");
            }
            _ => {}
        }
    }
}

#[test]
fn charts_in_degenerate_frames_draw_without_panicking() {
    // A chart frame squashed to no width (cx="0" in a generated file) made the value labels'
    // gutter clamp panic (its upper bound, a share of the width, fell below its lower bound).
    // Every chart type and legend position must survive any frame size.
    let p = Presentation::default();
    let rctx = Ctx::for_slide(&p, &p.slides[0]).unwrap();
    let kinds = [
        ChartType::Column,
        ChartType::StackedColumn,
        ChartType::PercentColumn,
        ChartType::Bar,
        ChartType::StackedBar,
        ChartType::PercentBar,
        ChartType::Line,
        ChartType::LineMarkers,
        ChartType::StackedLine,
        ChartType::Area,
        ChartType::StackedArea,
        ChartType::Pie,
        ChartType::Doughnut,
        ChartType::Scatter,
        ChartType::Bubble,
        ChartType::Radar,
        ChartType::FilledRadar,
        ChartType::Stock,
        ChartType::Surface,
        ChartType::Histogram,
        ChartType::Pareto,
        ChartType::BoxWhisker,
        ChartType::Waterfall,
        ChartType::Funnel,
        ChartType::Treemap,
        ChartType::Sunburst,
        ChartType::Combo,
    ];
    let mut ctx = vello_cpu::RenderContext::new(64, 64);
    for k in kinds {
        for (w, h) in [(0.0, 403.0), (403.0, 0.0), (0.0, 0.0), (2.0, 400.0), (400.0, 2.0), (12.0, 12.0)] {
            for legend in [None, Some("b"), Some("t"), Some("r"), Some("l"), Some("tr")] {
                let mut c = Chart::sample(k);
                c.legend = legend.map(String::from);
                crate::chart::draw(&mut ctx, &rctx, &c, kurbo::Affine::IDENTITY, w, h);
            }
        }
    }
}

/// Renders the EMF at `$DECKCRAFT_EMF_IN` to `$DECKCRAFT_EMF_PNG` (a look-at helper; does nothing
/// without them).
#[test]
fn render_an_emf_file_to_look_at() {
    let (Ok(src), Ok(out)) = (std::env::var("DECKCRAFT_EMF_IN"), std::env::var("DECKCRAFT_EMF_PNG")) else { return };
    let Ok(bytes) = std::fs::read(&src) else { return };
    let mut p = Presentation::default();
    let media = p.add_media("image1.emf", "image/x-emf", bytes).expect("media");
    let fill = deckcraft_model::style::PictureFill { media, ..Default::default() };
    let sh = Shape { id: ShapeId(600), xfrm: Some(Xfrm::new(40.0, 40.0, 840.0, 400.0)), kind: ShapeKind::Picture { fill }, ..Default::default() };
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let _ = std::fs::write(out, img.to_png());
}

#[test]
fn single_series_column_chart_varies_colours_by_point() {
    let render = |vary: bool| {
        let mut c = Chart::sample(ChartType::Column);
        c.series.truncate(1);
        c.vary_colors = vary;
        let sh =
            Shape { id: ShapeId(800), xfrm: Some(Xfrm::new(50.0, 50.0, 400.0, 300.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() }).pixels
    };
    assert_ne!(render(true), render(false));
}

/// A 1024×1024 PNG of 2 px wide black and white vertical stripes.
fn fine_stripes_png() -> Vec<u8> {
    let img =
        image::RgbaImage::from_fn(1024, 1024, |x, _| if (x / 2) % 2 == 0 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

#[test]
fn a_cropped_picture_is_decoded_at_the_resolution_of_the_whole_image() {
    // The top-left sixteenth of the image in a 64 pt box: 64 image pixels over 64 output pixels.
    // Decoding at the box's size instead of the whole image's blurs the stripes to grey (#762).
    let mut p = Presentation::default();
    let media = p.add_media("stripes.png", "image/png", fine_stripes_png()).expect("media");
    let fill = deckcraft_model::style::PictureFill { media, crop: [0.0, 0.0, 15.0 / 16.0, 15.0 / 16.0], ..Default::default() };
    let sh = Shape { id: ShapeId(600), xfrm: Some(Xfrm::new(100.0, 100.0, 64.0, 64.0)), kind: ShapeKind::Picture { fill }, ..Default::default() };
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let (mut sharp, mut total) = (0, 0);
    for y in 110..154 {
        for x in 110..154 {
            let v = img.pixel(x, y)[0];
            total += 1;
            if !(48..=208).contains(&v) {
                sharp += 1;
            }
        }
    }
    assert!(sharp * 10 >= total * 9, "the stripes stay sharp: {sharp} of {total} pixels black or white");
}

#[test]
fn empty_bullet_is_gray_only_while_editing_and_never_in_exports() {
    use deckcraft_model::text::{Bullet, Paragraph};
    let mut para = Paragraph::default();
    para.props.bullet = Some(Bullet::Char { char: "•".into() });
    para.props.margin_left = Some(24.0);
    para.props.indent = Some(-24.0);
    para.end_props.size = Some(32.0);
    let sh = Shape {
        id: ShapeId(901),
        xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 120.0)),
        text: Some(TextBody { paragraphs: vec![para], ..Default::default() }),
        ..Default::default()
    };
    let p = deck_with(vec![sh]);
    let render = |edit, editing_text| render_slide(&p, 0, &RenderOpts { edit, editing_text, ..Default::default() });
    let plain = render(false, None);
    assert_eq!(plain.pixels, render(true, None).pixels);
    assert_eq!(plain.pixels, render(false, Some(ShapeId(901))).pixels, "export ignores editor markers");
    assert_eq!(plain.pixels, render(true, Some(ShapeId(902))).pixels, "other shapes stay empty");
    let editing = render(true, Some(ShapeId(901)));
    let changed: Vec<_> = editing.pixels.as_chunks::<4>().0.iter().zip(plain.pixels.as_chunks::<4>().0).filter(|(a, b)| a != b).collect();
    assert!(!changed.is_empty(), "an empty bullet must be visible");
    assert!(changed.iter().any(|(px, _)| px[0] == 179 && px[1] == 179 && px[2] == 179));
    assert!(changed.iter().all(|(px, _)| px[0] >= 179 && px[0] == px[1] && px[1] == px[2]));
}

#[test]
fn placed_text_holds_the_field_value_not_the_saved_placeholder() {
    let mut p = deck_with(vec![field_shape(Some("slidenum"), "<#>")]);
    let second = p.slides[0].clone();
    p.slides.push(second);
    let placed = place_slide(&p, 1);
    assert_eq!(placed.texts.len(), 1);
    assert_eq!(placed.texts[0].body.text(), "2");
    // The layout was drawn from the same value, so the offsets agree with the body.
    assert_eq!(placed.texts[0].layout.runs.iter().map(|r| r.chars.1).max(), Some(1));
}

#[test]
fn placed_text_takes_its_date_from_the_given_render_options() {
    use deckcraft_text::datetime::DateTime;
    let p = deck_with(vec![field_shape(Some("datetime1"), "1/1/2000")]);
    let at = |now: DateTime| place_slide_with(&p, 0, &RenderOpts { now: Some(now), ..Default::default() });
    assert_eq!(at(DateTime::new(2026, 10, 9, 20, 5, 7)).texts[0].body.text(), "10/9/2026");
    assert_eq!(at(DateTime::new(2027, 3, 4, 0, 0, 0)).texts[0].body.text(), "3/4/2027");
}

/// A chart that has no text colour of its own takes PowerPoint's default, the theme's text colour
/// at 65 % plus 35 %: light on Nocturne's dark slides (it used to be a fixed grey, near-invisible
/// there), 595959 grey on Harbor's light ones.
#[test]
fn chart_text_follows_the_theme_when_it_has_no_colour_of_its_own() {
    let draw = |theme: &str, title: Option<&str>| {
        let mut c = Chart::sample(ChartType::Column);
        c.title = title.map(String::from);
        c.legend = None;
        c.data_labels = false;
        let sh = Shape { id: ShapeId(630), xfrm: Some(Xfrm::new(0.0, 0.0, 960.0, 540.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        let mut p = deck_with(vec![sh]);
        let t = deckcraft_model::theme::builtin_themes().into_iter().find(|t| t.name == theme).expect("theme");
        std::sync::Arc::make_mut(&mut p.masters[0]).theme = t;
        render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let lum = |px: [u8; 4]| (px[0] as u32 * 299 + px[1] as u32 * 587 + px[2] as u32 * 114) / 1000;
    // Pixels in the title band that pass `f`.
    let count = |img: &Image, f: &dyn Fn(u32) -> bool| {
        (200..760u32).flat_map(|x| (4..70u32).map(move |y| (x, y))).filter(|(x, y)| f(lum(img.pixel(*x, *y)))).count()
    };
    let (dark_bare, dark_titled) = (draw("Nocturne", None), draw("Nocturne", Some("Quarterly revenue")));
    assert!(lum(dark_bare.pixel(5, 5)) < 60, "Nocturne's slide is dark: {:?}", dark_bare.pixel(5, 5));
    let light_ink = count(&dark_titled, &|l| l > 200) as i64 - count(&dark_bare, &|l| l > 200) as i64;
    assert!(light_ink > 100, "the title is light on a dark theme (light pixels added: {light_ink})");
    let (light_bare, light_titled) = (draw("Harbor", None), draw("Harbor", Some("Quarterly revenue")));
    assert!(lum(light_bare.pixel(5, 5)) > 200, "Harbor's slide is light: {:?}", light_bare.pixel(5, 5));
    // 595959 has luminance 89: dark, but not black.
    let dark_ink = count(&light_titled, &|l| l < 120) as i64 - count(&light_bare, &|l| l < 120) as i64;
    assert!(dark_ink > 100, "the title is dark on a light theme (dark pixels added: {dark_ink})");
    let black_ink = count(&light_titled, &|l| l < 40) as i64 - count(&light_bare, &|l| l < 40) as i64;
    assert!(black_ink < 20, "the title is PowerPoint's 595959 grey, not black (black pixels added: {black_ink})");
}

/// Pie data labels sit on the slices in white; a colour given to the chart's text (chart.options
/// textColor, Font Color on the chart) colours them too, as it does the title and legend.
#[test]
fn pie_data_labels_take_the_chart_text_colour_when_it_has_one() {
    let draw = |colour: Option<ColorRef>| {
        let mut c = Chart::sample(ChartType::Pie);
        c.title = None;
        c.legend = None;
        c.data_labels = true;
        c.text_color = colour;
        let sh = Shape { id: ShapeId(632), xfrm: Some(Xfrm::new(0.0, 0.0, 400.0, 400.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let count = |img: &Image, f: &dyn Fn([u8; 4]) -> bool| {
        (0..400u32).flat_map(|x| (0..400u32).map(move |y| (x, y))).filter(|(x, y)| f(img.pixel(*x, *y))).count()
    };
    let red = |p: [u8; 4]| p[0] > 200 && p[1] < 60 && p[2] < 60;
    let white = |p: [u8; 4]| p[0] > 245 && p[1] > 245 && p[2] > 245;
    let (plain, coloured) = (draw(None), draw(Some(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0)))));
    assert_eq!(count(&plain, &red), 0, "no red without a text colour");
    assert!(count(&coloured, &red) > 30, "the labels take the chart's red: {} red pixels", count(&coloured, &red));
    assert!(count(&plain, &white) > count(&coloured, &white), "the labels were white before");
}

/// #2066: a line series that asks for value labels (`c:dLbls/c:showVal`, chart-wide or its own)
/// shows each point's value above it; with the chart's text red, the labels add red ink there.
/// Area series still draw none (their labels belong inside the area, not on a point).
#[test]
fn line_chart_value_labels_are_drawn_above_their_points() {
    let draw = |kind: ChartType, labels: bool, own: bool| {
        let mut c = Chart::sample(kind);
        c.title = None;
        c.legend = None;
        c.text_color = Some(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 0)));
        if own {
            c.series[0].data_labels = Some(labels);
        } else {
            c.data_labels = labels;
        }
        let sh = Shape { id: ShapeId(633), xfrm: Some(Xfrm::new(0.0, 0.0, 400.0, 300.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
        render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
    };
    let red = |img: &Image| {
        (0..400u32)
            .flat_map(|x| (0..300u32).map(move |y| (x, y)))
            .filter(|(x, y)| {
                let p = img.pixel(*x, *y);
                p[0] > 200 && p[1] < 60 && p[2] < 60
            })
            .count()
    };
    for kind in [ChartType::Line, ChartType::LineMarkers] {
        let (off, on) = (red(&draw(kind, false, false)), red(&draw(kind, true, false)));
        assert!(on > off + 100, "{kind:?}: labels add red ink ({off} → {on})");
        let own = red(&draw(kind, true, true));
        assert!(own > off + 30, "{kind:?}: a series' own showVal shows its labels ({off} → {own})");
    }
    // Area series keep none, and scatter and bubble series (drawn by the same code) are unchanged.
    for kind in [ChartType::Area, ChartType::Scatter, ChartType::Bubble] {
        let (off, on) = (red(&draw(kind, false, false)), red(&draw(kind, true, false)));
        assert_eq!(on, off, "{kind:?}: no point labels");
    }
}

/// The magenta pixels the labels add to a line chart of two series, only the second labelled,
/// with the chart's text magenta (no series, gridline or axis line is): its axis labels are
/// magenta too, so they are what the unlabelled chart has.
fn line_label_ink(kind: ChartType, first: f64, second: f64) -> (Image, Vec<(u32, u32)>) {
    let off: std::collections::HashSet<_> = magenta_pixels(&line_label_render(kind, first, second, false)).into_iter().collect();
    let img = line_label_render(kind, first, second, true);
    let added = magenta_pixels(&img).into_iter().filter(|p| !off.contains(p)).collect();
    (img, added)
}

fn line_label_render(kind: ChartType, first: f64, second: f64, labels: bool) -> Image {
    let mut c = Chart::sample(ChartType::Line);
    c.kind = kind;
    c.title = None;
    c.legend = None;
    c.series.truncate(2);
    c.series[0].values = vec![Some(first); 4];
    c.series[1].values = vec![Some(second); 4];
    c.series[1].data_labels = Some(labels);
    c.text_color = Some(ColorRef::rgb(deckcraft_color::Rgba::rgb(255, 0, 255)));
    let sh = Shape { id: ShapeId(634), xfrm: Some(Xfrm::new(0.0, 0.0, 400.0, 300.0)), kind: ShapeKind::Chart(Box::new(c)), ..Default::default() };
    render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 1.0, ..Default::default() })
}

/// Magenta ink on white, its anti-aliased edges included.
fn is_magenta(p: [u8; 4]) -> bool {
    p[0] > 200 && p[2] > 200 && p[1] < 200
}

fn magenta_pixels(img: &Image) -> Vec<(u32, u32)> {
    (0..img.width).flat_map(|x| (0..img.height).map(move |y| (x, y))).filter(|(x, y)| is_magenta(img.pixel(*x, *y))).collect()
}

/// #2066: in a stacked line chart a point's label is its own value, not the running total it is
/// drawn at. The second series' 5s, stacked on 100s, are labels as wide as an unstacked series of
/// 5s, and far narrower than one of 105s (the width: the pixel columns the label ink covers).
#[test]
fn a_stacked_line_labels_each_point_with_its_own_value() {
    let width = |kind, second| line_label_ink(kind, 100.0, second).1.iter().map(|(x, _)| *x).collect::<std::collections::HashSet<_>>().len();
    let (stacked, fives, totals) = (width(ChartType::StackedLine, 5.0), width(ChartType::Line, 5.0), width(ChartType::Line, 105.0));
    assert!(stacked > 0 && fives > 0, "labels are drawn ({stacked}, {fives})");
    assert!(stacked.abs_diff(fives) <= 4, "stacked 5s are labelled \"5\" ({stacked} columns vs {fives} for 5s)");
    assert!(stacked * 2 < totals, "not with the total \"105\" ({stacked} columns vs {totals} for 105s)");
}

/// #2066: a line label sits just above its point: all its ink is above the series' line, and
/// its lowest row is close to it.
#[test]
fn a_line_label_sits_just_above_its_point() {
    let (img, ink) = line_label_ink(ChartType::Line, 20.0, 50.0);
    assert!(!ink.is_empty(), "labels are drawn");
    let cols: std::collections::HashSet<u32> = ink.iter().map(|(x, _)| *x).collect();
    let (first, last) = (cols.iter().min().copied().unwrap(), cols.iter().max().copied().unwrap());
    // The labelled series' flat line at 50: the topmost coloured pixel in the plot's columns
    // between the labels (the other line is lower, at 20; the axis labels lie outside them).
    let line_top = (first..last)
        .filter(|x| !cols.iter().any(|c| c.abs_diff(*x) < 4))
        .filter_map(|x| {
            (0..img.height).find(|y| {
                let p = img.pixel(x, *y);
                let (hi, lo) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
                hi - lo > 80 && !is_magenta(p)
            })
        })
        .min()
        .unwrap();
    let lowest = ink.iter().map(|(_, y)| *y).max().unwrap();
    assert!(lowest < line_top, "the labels are above the line (lowest ink row {lowest}, line at {line_top})");
    assert!(line_top - lowest < 20, "and just above it (lowest ink row {lowest}, line at {line_top})");
}

/// #1165: an ellipse sized at the EMU limit, crossing the slide, made vello's flattener allocate a
/// buffer from its on-screen length and abort. It is skipped; the rest of the slide still draws.
#[test]
fn a_shape_far_larger_than_any_screen_is_skipped_not_flattened() {
    let huge = Shape {
        id: ShapeId(501),
        geom: Geom::preset("ellipse"),
        xfrm: Some(Xfrm::new(-5e17, -5e17, 1e18, 1e18)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent2)),
        ..Default::default()
    };
    let normal = Shape {
        id: ShapeId(502),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        style: Some(ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
        ..Default::default()
    };
    let p = deck_with(vec![huge, normal]);
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let accent = p.masters[0].theme.colors.get(deckcraft_color::SchemeSlot::Accent1);
    let px = img.pixel(200, 150);
    assert!((px[0] as i32 - accent.r as i32).abs() <= 2, "{px:?} vs {accent:?}");
    assert_eq!(img.pixel(50, 50), [255, 255, 255, 255], "the huge ellipse is not drawn");
}

/// #1165: a path with a NaN point is not drawn, even when the shape's frame is finite and small
/// (a bounding box, or a union with the frame, would hide the NaN).
#[test]
fn a_shape_whose_outline_is_not_finite_is_not_drawable() {
    let geo = |pts: &[(f64, f64)]| {
        let mut path = kurbo::BezPath::new();
        path.move_to(pts[0]);
        for p in &pts[1..] {
            path.line_to(*p);
        }
        deckcraft_geom::preset::Geometry {
            paths: vec![deckcraft_geom::preset::SubPath { path, fill: deckcraft_geom::preset::FillMode::Norm, stroke: true, even_odd: false }],
            text_rect: kurbo::Rect::ZERO,
            handles: vec![],
            sites: vec![],
        }
    };
    let m = kurbo::Affine::IDENTITY;
    assert!(crate::geometry::drawable(&geo(&[(0.0, 0.0), (100.0, 50.0)]), m, 100.0, 50.0));
    assert!(!crate::geometry::drawable(&geo(&[(0.0, 0.0), (f64::NAN, 50.0)]), m, 100.0, 50.0));
    assert!(!crate::geometry::drawable(&geo(&[(0.0, 0.0), (f64::INFINITY, 50.0)]), m, 100.0, 50.0));
    assert!(!crate::geometry::drawable(&geo(&[(0.0, 0.0), (1e18, 50.0)]), m, 100.0, 50.0));
    // A quadratic whose extremum overflows kurbo's bounding box: its control point still counts.
    let mut path = kurbo::BezPath::new();
    path.move_to((0.0, 0.0));
    path.quad_to((1e308, 0.0), (0.0, 0.0));
    let mut quad = geo(&[(0.0, 0.0), (1.0, 1.0)]);
    quad.paths[0].path = path;
    assert!(!crate::geometry::drawable(&quad, m, 100.0, 50.0));
}

/// A WMF without a placeable header (the frame comes from its window records) holding `records`
/// (function, parameter bytes).
fn wmf_bare(records: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut max_words = 3u32;
    for (func, params) in records {
        let words = (6 + params.len() as u32).div_ceil(2);
        max_words = max_words.max(words);
        body.extend(words.to_le_bytes());
        body.extend(func.to_le_bytes());
        body.extend(params);
        if params.len() % 2 == 1 {
            body.push(0);
        }
    }
    body.extend([3, 0, 0, 0, 0, 0]);
    let mut f = Vec::new();
    for v in [1u16, 9, 0x300] {
        f.extend(v.to_le_bytes());
    }
    f.extend((((18 + body.len()) / 2) as u32).to_le_bytes());
    f.extend(1u16.to_le_bytes());
    f.extend(max_words.to_le_bytes());
    f.extend(0u16.to_le_bytes());
    f.extend(body);
    f
}

#[test]
fn metafile_scaled_viewport_places_each_sub_picture_in_its_half() {
    // #2069: an 800 × 400 frame drawn as two sub-pictures. Each saves the DC, halves the viewport
    // width with SCALEVIEWPORTEXT, sets its own 400 × 400 window (the second shifted right by its
    // window origin), draws a circle and restores the DC: red on the left, blue on the right.
    let sub = |origin_x: i32, colorref: u32| {
        let mut brush = 0u16.to_le_bytes().to_vec();
        brush.extend(colorref.to_le_bytes());
        brush.extend(0u16.to_le_bytes());
        vec![
            (0x001E, vec![]),                    // SAVEDC
            (0x0103, i16s(&[8])),                // SETMAPMODE MM_ANISOTROPIC
            (0x0412, i16s(&[1, 1, 2, 1])),       // SCALEVIEWPORTEXT y 1/1, x 1/2
            (0x020B, i16s(&[0, origin_x])),      // SETWINDOWORG
            (0x020C, i16s(&[400, 400])),         // SETWINDOWEXT
            (0x02FC, brush),                     // CREATEBRUSHINDIRECT
            (0x012D, i16s(&[0])),                // SELECTOBJECT
            (0x0418, i16s(&[380, 380, 20, 20])), // ELLIPSE
            (0x0127, i16s(&[-1])),               // RESTOREDC
            (0x01F0, i16s(&[0])),                // DELETEOBJECT
        ]
    };
    let mut recs = vec![(0x020B, i16s(&[0, 0])), (0x020C, i16s(&[400, 800]))];
    recs.extend(sub(0, 0x0000_00FF));
    recs.extend(sub(-400, 0x00FF_0000));
    let img = crate::metafile::rasterize(&wmf_bare(&recs), 200).expect("raster");
    assert_eq!(img.dimensions(), (200, 100));
    let red = |p: &image::Rgba<u8>| p[0] > 200 && p[1] < 60 && p[2] < 60 && p[3] > 200;
    let blue = |p: &image::Rgba<u8>| p[2] > 200 && p[0] < 60 && p[1] < 60 && p[3] > 200;
    assert!(red(img.get_pixel(50, 50)), "red circle in the left half: {:?}", img.get_pixel(50, 50));
    assert!(blue(img.get_pixel(150, 50)), "blue circle in the right half: {:?}", img.get_pixel(150, 50));
    assert!(img.get_pixel(100, 50)[3] < 128, "the circles don't meet in the middle: {:?}", img.get_pixel(100, 50));
}
