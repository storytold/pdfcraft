//! Render tests for shape, line, fill and effect cases checked against PowerPoint's own exports
//! of synthetic "wild" slides (observed black-box; see `plan/powerpoint/tools/gen_wild.py`).

use super::{
    BezPath, Fill, Image, Line, MAX_DASHES, PathEl, Point, Rect, RenderOpts, ShapeKind, dash_with_caps, head_size, images, limit_dash_count, paint,
    render_slide, trim_ends,
};
use deckcraft_color::Rgba;
use deckcraft_model::style::ColorRef;
use deckcraft_model::{CustomPath, Geom, Presentation, Shape, ShapeId, Xfrm};
use kurbo::Shape as _;

fn deck_with(shapes: Vec<Shape>) -> Presentation {
    let mut p = Presentation::default();
    let s = std::sync::Arc::make_mut(&mut p.slides[0]);
    s.shapes = shapes;
    p
}

fn render(shapes: Vec<Shape>) -> Image {
    render_slide(&deck_with(shapes), 0, &RenderOpts { scale: 1.0, ..Default::default() })
}

fn solid(c: Rgba) -> Option<Fill> {
    Some(Fill::solid(ColorRef::rgb(c)))
}

fn no_line() -> Option<Line> {
    Some(Line { fill: Some(Fill::None), ..Default::default() })
}

fn close(px: [u8; 4], c: Rgba, tol: i32) -> bool {
    (px[0] as i32 - c.r as i32).abs() <= tol && (px[1] as i32 - c.g as i32).abs() <= tol && (px[2] as i32 - c.b as i32).abs() <= tol
}

const WHITE: Rgba = Rgba::WHITE;
const RED: Rgba = Rgba::rgb(255, 0, 0);

#[test]
fn custom_geometry_sub_paths_cut_holes_whichever_way_they_wind() {
    // Outer square clockwise, inner square also clockwise: PowerPoint still shows a hole.
    let d = "M 0 0 L 100 0 L 100 100 L 0 100 Z M 25 25 L 75 25 L 75 75 L 25 75 Z";
    let sh = Shape {
        id: ShapeId(10),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 200.0)),
        geom: Geom::Custom { paths: vec![CustomPath { w: 100.0, h: 100.0, d: d.into(), ..Default::default() }] },
        fill: solid(RED),
        line: no_line(),
        ..Default::default()
    };
    let img = render(vec![sh]);
    assert!(close(img.pixel(120, 120), RED, 2), "outer ring filled");
    assert!(close(img.pixel(200, 200), WHITE, 2), "inner square is a hole: {:?}", img.pixel(200, 200));
}

fn dotted_line(cap: deckcraft_model::style::LineCap, len: f64) -> Shape {
    Shape {
        id: ShapeId(11),
        xfrm: Some(Xfrm::new(100.0, 100.0, len, 0.0)),
        geom: Geom::preset("line"),
        line: Some(Line {
            fill: solid(RED),
            width: Some(10.0),
            dash: Some(deckcraft_model::style::Dash::Preset { name: "sysDot".into() }),
            cap: Some(cap),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn round_dots_are_separate_circles_with_the_caps_inside_each_dash() {
    use deckcraft_model::style::LineCap;
    // PowerPoint's "Round Dot": one width on, one off, round caps. The caps sit inside the dash, so
    // the dots stay apart (they used to grow into each other and draw a solid line).
    let img = render(vec![dotted_line(LineCap::Round, 200.0)]);
    assert!(close(img.pixel(100, 100), RED, 40), "a dot centred on the start {:?}", img.pixel(100, 100));
    assert!(close(img.pixel(120, 100), RED, 40), "the next dot one period on");
    assert!(close(img.pixel(110, 100), WHITE, 10), "a gap between dots {:?}", img.pixel(110, 100));
    // Flat caps: each dash is a width long, from the start.
    let img = render(vec![dotted_line(LineCap::Flat, 200.0)]);
    assert!(close(img.pixel(105, 100), RED, 40) && close(img.pixel(115, 100), WHITE, 10));
    // Square caps: dashes end flat; only the line's own start is squared off, half a width back.
    let img = render(vec![dotted_line(LineCap::Square, 200.0)]);
    assert!(close(img.pixel(97, 100), RED, 40), "square cap before the start {:?}", img.pixel(97, 100));
    assert!(close(img.pixel(105, 100), RED, 40) && close(img.pixel(115, 100), WHITE, 10), "dashes keep their length");
    assert!(close(img.pixel(122, 100), RED, 40) && close(img.pixel(118, 100), WHITE, 10), "no cap on the next dash");
    // The end: 200 pt is ten whole periods, so the line ends in a gap and gets no cap.
    assert!(close(img.pixel(302, 100), WHITE, 10), "{:?}", img.pixel(302, 100));
    // 205 pt ends half way through a dash: squared off past the end.
    let img = render(vec![dotted_line(LineCap::Square, 205.0)]);
    assert!(close(img.pixel(308, 100), RED, 40), "{:?}", img.pixel(308, 100));
}

#[test]
fn dash_lengths_make_room_for_round_and_square_caps() {
    assert_eq!(dash_with_caps(vec![4.0, 3.0], kurbo::Cap::Round, 1.0), vec![3.0, 4.0]);
    assert_eq!(dash_with_caps(vec![1.0, 1.0], kurbo::Cap::Round, 2.0), vec![0.002, 3.0]);
    assert_eq!(dash_with_caps(vec![1.0, 1.0], kurbo::Cap::Square, 2.0), vec![1.0, 1.0]);
    assert_eq!(dash_with_caps(vec![4.0, 3.0], kurbo::Cap::Butt, 1.0), vec![4.0, 3.0]);
    assert_eq!(dash_with_caps(vec![4.0, 3.0, 1.0], kurbo::Cap::Round, 1.0), vec![4.0, 3.0, 1.0]);
}

fn arrow_line(kind: &str, width: f64, c: Rgba) -> Shape {
    let end = deckcraft_model::style::LineEnd { kind: kind.into(), w: "med".into(), len: "med".into() };
    Shape {
        id: ShapeId(12),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 0.0)),
        geom: Geom::preset("line"),
        line: Some(Line { fill: solid(c), width: Some(width), tail: Some(end), ..Default::default() }),
        ..Default::default()
    }
}

#[test]
fn a_wide_line_stops_inside_its_arrowhead() {
    // 10 pt line, medium triangle (30 pt long, 30 wide) whose tip is the line's end at x = 300.
    let img = render(vec![arrow_line("triangle", 10.0, RED)]);
    assert!(close(img.pixel(297, 100), RED, 40), "the tip {:?}", img.pixel(297, 100));
    // 2 pt back from the tip the triangle is 2 pt wide: the line's corner must not show there.
    assert!(close(img.pixel(298, 104), WHITE, 10), "line corner past the head {:?}", img.pixel(298, 104));
}

#[test]
fn a_see_through_line_and_its_arrowhead_are_one_shape() {
    let half_red = Rgba::rgba(255, 0, 0, 128);
    let img = render(vec![arrow_line("triangle", 10.0, half_red)]);
    let line_only = img.pixel(200, 100);
    let head_over_line = img.pixel(285, 100);
    assert!(close(line_only, Rgba::rgb(255, 127, 127), 6), "{line_only:?}");
    assert!(close(head_over_line, Rgba::rgb(line_only[0], line_only[1], line_only[2]), 3), "no darker overlap: {head_over_line:?}");
}

#[test]
fn an_open_arrowheads_point_is_on_the_lines_end() {
    let img = render(vec![arrow_line("arrow", 8.0, RED)]);
    let reach = (290..320).rev().find(|x| img.pixel(*x, 100)[1] < 128);
    assert!(reach.is_some_and(|x| x <= 300), "point reaches {reach:?}");
    assert!(reach.is_some_and(|x| x >= 297), "point reaches {reach:?}");
}

#[test]
fn arrowheads_are_sized_for_at_least_a_two_point_line() {
    let e = deckcraft_model::style::LineEnd { kind: "triangle".into(), w: "sm".into(), len: "lg".into() };
    assert_eq!(head_size(&e, 0.5), (4.0, 10.0));
    assert_eq!(head_size(&e, 4.0), (8.0, 20.0));
}

#[test]
fn trimming_a_path_leaves_its_ends_off_by_arc_length() {
    let mut p = BezPath::new();
    p.move_to((0.0, 0.0));
    p.line_to((50.0, 0.0));
    p.line_to((50.0, 50.0));
    let t = trim_ends(&p, 10.0, 20.0);
    let els = t.elements();
    assert_eq!(els.first(), Some(&PathEl::MoveTo(Point::new(10.0, 0.0))));
    match els.last() {
        Some(PathEl::LineTo(q)) => assert!((q.x - 50.0).abs() < 1e-6 && (q.y - 30.0).abs() < 1e-6, "{q:?}"),
        other => panic!("{other:?}"),
    }
    // Trims longer than the path leave nothing; closed paths are untouched.
    assert!(trim_ends(&p, 60.0, 50.0).elements().is_empty());
    let r = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.1);
    assert_eq!(trim_ends(&r, 3.0, 3.0).elements().len(), r.elements().len());

    // Zero-length leading segments trimmed correctly without quadratic copy.
    let mut p0 = BezPath::new();
    p0.move_to((0.0, 0.0));
    p0.line_to((0.0, 0.0));
    p0.line_to((0.0, 0.0));
    p0.line_to((50.0, 0.0));
    let t0 = trim_ends(&p0, 10.0, 0.0);
    assert_eq!(t0.elements().first(), Some(&PathEl::MoveTo(Point::new(10.0, 0.0))));
}

fn radial(focus: [f64; 4]) -> Shape {
    use deckcraft_model::style::{Gradient, GradientShape, GradientStop};
    let stops = vec![GradientStop { pos: 0.0, color: ColorRef::rgb(WHITE) }, GradientStop { pos: 1.0, color: ColorRef::rgb(Rgba::BLACK) }];
    Shape {
        id: ShapeId(13),
        xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 200.0)),
        fill: Some(Fill::Gradient(Gradient {
            tile: [0.0; 4],
            stops,
            shape: GradientShape::Path { path: "circle".into(), focus },
            rotate_with_shape: true,
            flip: None,
        })),
        line: no_line(),
        ..Default::default()
    }
}

#[test]
fn a_radial_gradient_off_centre_ends_on_the_circle_around_the_box() {
    // Focus a fifth of the way in from the top-left corner. PowerPoint's last colour is reached on
    // the circle through the box's corners, so the near corner is dark as well as the far one.
    let img = render(vec![radial([0.2, 0.2, 0.8, 0.8])]);
    assert!(img.pixel(181, 141)[0] > 240, "white at the focus {:?}", img.pixel(181, 141));
    assert!(img.pixel(101, 101)[0] < 60, "near corner dark {:?}", img.pixel(101, 101));
    assert!(img.pixel(498, 298)[0] < 60, "far corner dark {:?}", img.pixel(498, 298));
    // Straight above the focus the box's edge is well inside the circle: still light.
    assert!(img.pixel(181, 101)[0] > 150, "{:?}", img.pixel(181, 101));
    // A centred one is symmetric, dark only at the corners.
    let img = render(vec![radial([0.5, 0.5, 0.5, 0.5])]);
    assert!(img.pixel(300, 200)[0] > 240 && img.pixel(101, 101)[0] < 60);
    assert!((img.pixel(110, 200)[0] as i32 - img.pixel(490, 200)[0] as i32).abs() <= 2);
}

fn shadow(sx: f64, sy: f64, kx: f64, align: &str) -> deckcraft_model::style::Shadow {
    deckcraft_model::style::Shadow {
        color: ColorRef::rgb(Rgba::BLACK),
        blur: 0.0,
        dist: 0.0,
        dir: 0.0,
        inner: false,
        sx,
        sy,
        kx,
        ky: 0.0,
        align: align.into(),
        rotate_with_shape: false,
    }
}

fn with_effects(fill: Option<Fill>, line: Option<Line>, effects: deckcraft_model::Effects) -> Shape {
    Shape { id: ShapeId(14), xfrm: Some(Xfrm::new(100.0, 100.0, 100.0, 100.0)), fill, line, effects: Some(effects), ..Default::default() }
}

#[test]
fn a_perspective_shadow_lies_along_the_edge_its_alignment_names() {
    // Half height, skewed 45° so its top leans right, anchored at the bottom middle: the shadow's
    // bottom edge is the shape's (y = 200) and its top (y = 150) is 50 pt to the right.
    let fx = deckcraft_model::Effects { outer_shadow: Some(shadow(1.0, 0.5, -45.0, "b")), ..Default::default() };
    let img = render(vec![with_effects(solid(RED), no_line(), fx)]);
    assert!(close(img.pixel(210, 175), Rgba::BLACK, 30), "{:?}", img.pixel(210, 175));
    assert!(close(img.pixel(240, 155), Rgba::BLACK, 30), "{:?}", img.pixel(240, 155));
    assert!(close(img.pixel(240, 180), WHITE, 10), "{:?}", img.pixel(240, 180));
    assert!(close(img.pixel(205, 140), WHITE, 10), "nothing above the half height {:?}", img.pixel(205, 140));
    // Anchored bottom-right and half as big again: it grows up and to the left only.
    let fx = deckcraft_model::Effects { outer_shadow: Some(shadow(1.5, 1.5, 0.0, "br")), ..Default::default() };
    let img = render(vec![with_effects(solid(RED), no_line(), fx)]);
    assert!(close(img.pixel(70, 70), Rgba::BLACK, 30) && close(img.pixel(210, 210), WHITE, 10));
}

fn offset_shadow() -> deckcraft_model::Effects {
    let mut sh = shadow(1.0, 1.0, 0.0, "b");
    sh.dist = 20.0;
    sh.dir = 0.0;
    deckcraft_model::Effects { outer_shadow: Some(sh), ..Default::default() }
}

#[test]
fn shadows_are_cast_by_what_is_drawn() {
    // An outline-only square: its shadow (20 pt to the right) is a frame, not a solid square.
    let outline = Some(Line::solid(ColorRef::rgb(RED), 4.0));
    let img = render(vec![with_effects(Some(Fill::None), outline.clone(), offset_shadow())]);
    assert!(close(img.pixel(220, 150), Rgba::BLACK, 30), "the frame's shadow {:?}", img.pixel(220, 150));
    assert!(close(img.pixel(160, 150), WHITE, 10), "no shadow inside the frame {:?}", img.pixel(160, 150));
    // A half see-through fill casts a half see-through shadow.
    let img = render(vec![with_effects(solid(Rgba::rgba(255, 0, 0, 128)), no_line(), offset_shadow())]);
    let px = img.pixel(210, 150);
    assert!(close(px, Rgba::rgb(127, 127, 127), 8), "half shadow {px:?}");
    // A line casts a shadow too.
    let mut ln = arrow_line("none", 6.0, RED);
    ln.effects = Some(offset_shadow());
    let img = render(vec![ln]);
    assert!(close(img.pixel(310, 100), Rgba::BLACK, 30), "line shadow {:?}", img.pixel(310, 100));
}

#[allow(clippy::unwrap_used)]
fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb(rgb))).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn picture_deck(line: Option<Line>) -> Presentation {
    let mut p = Presentation::default();
    let media = p.add_media("image1.png", "image/png", png(8, 8, [0, 0, 255])).expect("media");
    let sh = Shape {
        id: ShapeId(15),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        kind: ShapeKind::Picture { fill: deckcraft_model::style::PictureFill { media, ..Default::default() } },
        line,
        ..Default::default()
    };
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
    p
}

#[test]
fn a_pictures_border_lies_outside_the_picture() {
    let p = picture_deck(Some(Line::solid(ColorRef::rgb(RED), 8.0)));
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    let blue = Rgba::rgb(0, 0, 255);
    // The whole picture shows, up to its edge; the 8 pt border is all outside it.
    assert!(close(img.pixel(101, 150), blue, 30), "picture at its edge {:?}", img.pixel(101, 150));
    assert!(close(img.pixel(98, 150), RED, 30) && close(img.pixel(93, 150), RED, 30), "{:?}", img.pixel(93, 150));
    assert!(close(img.pixel(90, 150), WHITE, 10), "border no wider than the line {:?}", img.pixel(90, 150));
    // A dashed border keeps its dash lengths (a dash is 4 widths: 32 pt), drawn from the corner.
    let mut line = Line::solid(ColorRef::rgb(RED), 8.0);
    line.dash = Some(deckcraft_model::style::Dash::Preset { name: "dash".into() });
    let img = render_slide(&picture_deck(Some(line)), 0, &RenderOpts { scale: 1.0, ..Default::default() });
    assert!(close(img.pixel(120, 96), RED, 30) && close(img.pixel(140, 96), WHITE, 10), "{:?}", img.pixel(140, 96));
}

#[test]
fn children_filled_with_the_group_fill_show_one_fill_across_the_group() {
    use deckcraft_model::style::{Gradient, GradientShape, GradientStop};
    let stops = vec![GradientStop { pos: 0.0, color: ColorRef::rgb(RED) }, GradientStop { pos: 1.0, color: ColorRef::rgb(Rgba::rgb(0, 0, 255)) }];
    let grad = Fill::Gradient(Gradient {
        tile: [0.0; 4],
        stops,
        shape: GradientShape::Linear { angle: 0.0, scaled: false },
        rotate_with_shape: true,
        flip: None,
    });
    let child = |id: u32, x: f64| Shape {
        id: ShapeId(id),
        xfrm: Some(Xfrm::new(x, 0.0, 90.0, 100.0)),
        fill: Some(Fill::Group),
        line: no_line(),
        ..Default::default()
    };
    let g = Shape {
        id: ShapeId(16),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        fill: Some(grad),
        kind: ShapeKind::Group { children: vec![child(17, 0.0), child(18, 110.0)], child: Xfrm::new(0.0, 0.0, 200.0, 100.0) },
        ..Default::default()
    };
    let img = render(vec![g]);
    let (l, r) = (img.pixel(105, 150), img.pixel(295, 150));
    assert!(l[0] > 200 && l[2] < 60, "left end of the group's gradient {l:?}");
    assert!(r[2] > 200 && r[0] < 60, "right end {r:?}");
    assert!(close(img.pixel(200, 150), WHITE, 5), "the gap between the children stays empty");
}

#[test]
fn open_freeforms_take_arrowheads_even_when_filled() {
    // An open custom path with a fill and a tail arrow: PowerPoint draws the arrow.
    let end = deckcraft_model::style::LineEnd { kind: "triangle".into(), w: "med".into(), len: "med".into() };
    let sh = Shape {
        id: ShapeId(19),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        geom: Geom::Custom { paths: vec![CustomPath { w: 100.0, h: 100.0, d: "M 0 100 L 50 0 L 100 100".into(), ..Default::default() }] },
        fill: solid(Rgba::rgb(0, 200, 0)),
        line: Some(Line { fill: solid(RED), width: Some(4.0), tail: Some(end), ..Default::default() }),
        ..Default::default()
    };
    let img = render(vec![sh]);
    // The line ends at (300, 200) heading down-right at 45°; the 12 pt head's base corner reaches
    // (295.7, 187.3), 3.5 pt off the line's axis: outside the 4 pt line and outside the fill.
    assert!(close(img.pixel(294, 189), RED, 60), "arrowhead drawn {:?}", img.pixel(294, 189));
}

#[test]
fn a_corner_radial_centres_on_the_corner_through_its_tile() {
    // fillToRect on the top-left corner, tileRect reaching a whole box up and left: the circle is
    // centred on the corner and reaches the far corner (as PowerPoint's "from corner" presets).
    let mut sh = radial([0.0, 0.0, 1.0, 1.0]);
    if let Some(Fill::Gradient(g)) = &mut sh.fill {
        g.tile = [-1.0, -1.0, 0.0, 0.0];
    }
    let img = render(vec![sh]);
    assert!(img.pixel(102, 102)[0] > 240, "white at the corner {:?}", img.pixel(102, 102));
    let (top_right, bottom_left) = (img.pixel(498, 101)[0], img.pixel(101, 298)[0]);
    assert!(top_right < bottom_left, "farther is darker {top_right} {bottom_left}");
    assert!(bottom_left > 120, "the near edge stays light {bottom_left}");
    assert!(img.pixel(497, 297)[0] < 40, "dark at the far corner {:?}", img.pixel(497, 297));
}

#[test]
fn a_linear_gradient_spans_its_tile_rect() {
    use deckcraft_model::style::{Gradient, GradientShape, GradientStop};
    // Tile rect the left half of the box: the ramp ends half way and stays black after.
    let stops = vec![GradientStop { pos: 0.0, color: ColorRef::rgb(WHITE) }, GradientStop { pos: 1.0, color: ColorRef::rgb(Rgba::BLACK) }];
    let g = Gradient {
        stops,
        shape: GradientShape::Linear { angle: 0.0, scaled: false },
        rotate_with_shape: true,
        tile: [0.0, 0.0, 0.5, 0.0],
        flip: None,
    };
    let sh = Shape {
        id: ShapeId(20),
        xfrm: Some(Xfrm::new(100.0, 100.0, 400.0, 100.0)),
        fill: Some(Fill::Gradient(g)),
        line: no_line(),
        ..Default::default()
    };
    let img = render(vec![sh]);
    assert!(img.pixel(102, 150)[0] > 240);
    assert!(img.pixel(305, 150)[0] < 15 && img.pixel(495, 150)[0] < 15, "{:?}", img.pixel(305, 150));
}

#[test]
fn a_3d_pictures_border_stays_centred_on_its_edge() {
    let mut p = picture_deck(Some(Line::solid(ColorRef::rgb(RED), 8.0)));
    let s = std::sync::Arc::make_mut(&mut p.slides[0]);
    if let Some(sh) = s.shapes.first_mut() {
        sh.effects = Some(deckcraft_model::Effects {
            bevel: Some("circle".into()),
            raw3d: Some("<a:scene3d><a:camera prst=\"orthographicFront\"/></a:scene3d><a:sp3d><a:bevelT/></a:sp3d>".into()),
            ..Default::default()
        });
    }
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    assert!(close(img.pixel(102, 150), RED, 30), "covers the picture's edge {:?}", img.pixel(102, 150));
    assert!(close(img.pixel(94, 150), WHITE, 10), "reaches half the width out {:?}", img.pixel(94, 150));
}

#[allow(clippy::unwrap_used)]
fn half_red_half_blue() -> Vec<u8> {
    let img = image::RgbImage::from_fn(8, 4, |x, _| if x < 4 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
    let mut out = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn tiled(sx: f64, flip: &str, align: &str) -> Image {
    use deckcraft_model::style::{PictureFill, PictureMode};
    let mut p = Presentation::default();
    let media = p.add_media("t.png", "image/png", half_red_half_blue()).expect("media");
    let mode = PictureMode::Tile { tx: 0.0, ty: 0.0, sx, sy: 10.0, flip: flip.into(), align: align.into() };
    let sh = Shape {
        id: ShapeId(21),
        xfrm: Some(Xfrm::new(100.0, 100.0, 240.0, 60.0)),
        fill: Some(Fill::Picture(PictureFill { media, mode, ..Default::default() })),
        line: no_line(),
        ..Default::default()
    };
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
    render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() })
}

#[test]
fn tiled_pictures_flip_align_and_skip_non_positive_scales() {
    let blue = Rgba::rgb(0, 0, 255);
    // 8×4 px at 96 dpi scaled ten times: 60×30 pt tiles, red then blue, from the top-left.
    let img = tiled(10.0, "none", "tl");
    assert!(close(img.pixel(115, 115), RED, 30) && close(img.pixel(145, 115), blue, 30) && close(img.pixel(175, 115), RED, 30));
    // Flipped across x: every other tile mirrored, so the second starts blue.
    let img = tiled(10.0, "x", "tl");
    assert!(close(img.pixel(145, 115), blue, 30) && close(img.pixel(175, 115), blue, 30) && close(img.pixel(205, 115), RED, 30));
    // Aligned right: a tile ends at the box's right edge (340), so 310-340 is blue.
    let img = tiled(10.0, "none", "r");
    assert!(close(img.pixel(335, 130), blue, 30) && close(img.pixel(305, 130), RED, 30), "{:?}", img.pixel(305, 130));
    // A negative scale draws nothing, as in PowerPoint.
    let img = tiled(-10.0, "none", "tl");
    assert!(close(img.pixel(115, 115), WHITE, 5) && close(img.pixel(300, 150), WHITE, 5));
}

#[allow(clippy::unwrap_used)]
fn red_over_blue() -> Vec<u8> {
    let img = image::RgbImage::from_fn(4, 8, |_, y| if y < 4 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
    let mut out = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn a_tile_keeps_its_size_in_a_group_with_scaled_child_coordinates() {
    use deckcraft_model::style::{PictureFill, PictureMode};
    // A group's child space scales its shapes' boxes, not the tile: the same tiles show whether
    // the group's child space is 1600 times smaller (old decks write them so), 4 times larger,
    // the slide's own or stretched unevenly (PowerPoint draws all of them alike, measured on its
    // own export of a probe deck). Before, one tile covered hundreds of boxes and the fill was one
    // flat texel, and an uneven group stretched the tile.
    // Wide: 8x4 px = 6x3 pt, ten times = 60x30 pt tiles, red then blue across.
    // Tall: 4x8 px = 3x6 pt, ten times = 30x60 pt tiles, red over blue down.
    let blue = Rgba::rgb(0, 0, 255);
    let scales = [(1.0, 1.0), (1600.0, 1600.0), (0.25, 0.25), (2.0, 0.5), (0.5, 2.0), (1800.0, 1.0), (1.0, 1800.0)];
    for (kx, ky) in scales {
        for tall in [false, true] {
            let mut p = Presentation::default();
            let png = if tall { red_over_blue() } else { half_red_half_blue() };
            let media = p.add_media("t.png", "image/png", png).expect("media");
            let mode = PictureMode::Tile { tx: 0.0, ty: 0.0, sx: 10.0, sy: 10.0, flip: "none".into(), align: "tl".into() };
            let member = Shape {
                id: ShapeId(22),
                xfrm: Some(Xfrm::new(0.0, 0.0, 240.0 / kx, 60.0 / ky)),
                fill: Some(Fill::Picture(PictureFill { media, mode, ..Default::default() })),
                line: no_line(),
                ..Default::default()
            };
            let group = Shape {
                id: ShapeId(21),
                xfrm: Some(Xfrm::new(100.0, 100.0, 240.0, 60.0)),
                kind: ShapeKind::Group { children: vec![member], child: Xfrm::new(0.0, 0.0, 240.0 / kx, 60.0 / ky) },
                ..Default::default()
            };
            std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![group];
            let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
            // Along the tile's long side: first half red, second blue, then red again.
            let (a, b, c) = if tall { ((115, 115), (115, 145), (115, 175)) } else { ((115, 115), (145, 115), (175, 115)) };
            let (pa, pb, pc) = (img.pixel(a.0, a.1), img.pixel(b.0, b.1), img.pixel(c.0, c.1));
            assert!(
                close(pa, RED, 30) && close(pb, blue, 30) && (tall || close(pc, RED, 30)) && (!tall || close(pc, WHITE, 5) || close(pc, RED, 30)),
                "child space {kx} x {ky}, tall={tall}: {pa:?} {pb:?} {pc:?}"
            );
        }
    }
}

/// A slide with `shape` (built for a box of `w` x `h` child units) in a group at (100, 100) that is
/// 240 x 60 pt on the slide and scaled `kx` x `ky` against its child space, drawn at 1 px per point.
fn in_a_scaled_group(kx: f64, ky: f64, p: Presentation, shape: impl FnOnce(f64, f64) -> Shape) -> Image {
    let (w, h) = (240.0 / kx, 60.0 / ky);
    let group = Shape {
        id: ShapeId(21),
        xfrm: Some(Xfrm::new(100.0, 100.0, 240.0, 60.0)),
        kind: ShapeKind::Group { children: vec![shape(w, h)], child: Xfrm::new(0.0, 0.0, w, h) },
        ..Default::default()
    };
    let mut p = p;
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![group];
    render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() })
}

#[test]
fn a_tile_a_group_passes_to_its_members_keeps_its_size_inside_a_scaled_group() {
    use deckcraft_model::style::{PictureFill, PictureMode};
    // The group that owns the tiled fill sits inside another group whose child space is scaled: the
    // members that take it (`grpFill`) get tiles of 60 x 30 pt, as with no outer group.
    let blue = Rgba::rgb(0, 0, 255);
    for (kx, ky) in [(1.0, 1.0), (0.25, 0.25), (1600.0, 1600.0), (2.0, 0.5)] {
        let mut p = Presentation::default();
        let media = p.add_media("t.png", "image/png", half_red_half_blue()).expect("media");
        let mode = PictureMode::Tile { tx: 0.0, ty: 0.0, sx: 10.0, sy: 10.0, flip: "none".into(), align: "tl".into() };
        let img = in_a_scaled_group(kx, ky, p, |w, h| {
            let member =
                Shape { id: ShapeId(23), xfrm: Some(Xfrm::new(0.0, 0.0, w, h)), fill: Some(Fill::Group), line: no_line(), ..Default::default() };
            Shape {
                id: ShapeId(22),
                xfrm: Some(Xfrm::new(0.0, 0.0, w, h)),
                fill: Some(Fill::Picture(PictureFill { media, mode, ..Default::default() })),
                kind: ShapeKind::Group { children: vec![member], child: Xfrm::new(0.0, 0.0, w, h) },
                ..Default::default()
            }
        });
        let (a, b, c) = (img.pixel(115, 115), img.pixel(145, 115), img.pixel(175, 115));
        assert!(close(a, RED, 30) && close(b, blue, 30) && close(c, RED, 30), "outer group {kx} x {ky}: {a:?} {b:?} {c:?}");
    }
}

#[test]
fn a_picture_shapes_own_tile_keeps_its_size_inside_a_scaled_group() {
    use deckcraft_model::style::{PictureFill, PictureMode};
    // A picture (`p:pic`) whose blip fill is a tile rather than a stretch.
    let blue = Rgba::rgb(0, 0, 255);
    for (kx, ky) in [(1.0, 1.0), (0.25, 0.25), (1600.0, 1600.0), (2.0, 0.5)] {
        let mut p = Presentation::default();
        let media = p.add_media("t.png", "image/png", half_red_half_blue()).expect("media");
        let mode = PictureMode::Tile { tx: 0.0, ty: 0.0, sx: 10.0, sy: 10.0, flip: "none".into(), align: "tl".into() };
        let img = in_a_scaled_group(kx, ky, p, |w, h| Shape {
            id: ShapeId(22),
            xfrm: Some(Xfrm::new(0.0, 0.0, w, h)),
            kind: ShapeKind::Picture { fill: PictureFill { media, mode, ..Default::default() } },
            line: no_line(),
            ..Default::default()
        });
        let (a, b, c) = (img.pixel(115, 115), img.pixel(145, 115), img.pixel(175, 115));
        assert!(close(a, RED, 30) && close(b, blue, 30) && close(c, RED, 30), "group {kx} x {ky}: {a:?} {b:?} {c:?}");
    }
}

#[test]
fn a_tiled_pictures_reflection_keeps_the_tile_size_inside_a_scaled_group() {
    use deckcraft_model::style::{PictureFill, PictureMode, Reflection};
    // The reflection mirrors the picture's pixels, so its tiles must be the face's, 60 x 30 pt,
    // whatever the group's child scale: just below the box it shows the same colours.
    let reflected = |kx: f64, ky: f64| {
        let mut p = Presentation::default();
        let media = p.add_media("t.png", "image/png", half_red_half_blue()).expect("media");
        let mode = PictureMode::Tile { tx: 0.0, ty: 0.0, sx: 10.0, sy: 10.0, flip: "none".into(), align: "tl".into() };
        let rf = Reflection { blur: 0.0, start_alpha: 1.0, end_alpha: 0.0, end_pos: 0.5, dist: 0.0, ..Default::default() };
        let img = in_a_scaled_group(kx, ky, p, |w, h| Shape {
            id: ShapeId(22),
            xfrm: Some(Xfrm::new(0.0, 0.0, w, h)),
            kind: ShapeKind::Picture { fill: PictureFill { media, mode, ..Default::default() } },
            line: no_line(),
            effects: Some(deckcraft_model::Effects { reflection: Some(rf), ..Default::default() }),
            ..Default::default()
        });
        [img.pixel(115, 163), img.pixel(145, 163), img.pixel(175, 163)]
    };
    let plain = reflected(1.0, 1.0);
    assert!(plain[0][0] > plain[0][2] && plain[1][2] > plain[1][0] && plain[2][0] > plain[2][2], "red, blue, red in the reflection: {plain:?}");
    for (kx, ky) in [(0.25, 0.25), (1600.0, 1600.0), (2.0, 0.5)] {
        let got = reflected(kx, ky);
        for (g, e) in got.iter().zip(&plain) {
            assert!(close(*g, Rgba::rgb(e[0], e[1], e[2]), 40), "child scale {kx} x {ky}: {got:?} against {plain:?}");
        }
    }
}

#[test]
fn an_upright_tile_in_a_turned_shape_keeps_its_size_in_points_at_any_zoom() {
    use deckcraft_model::style::{PictureFill, PictureMode};
    // `rotWithShape="0"` draws the tiles on the slide's axes, in device pixels: they were sized in
    // pixels, so zooming (or a group's child scale) changed their size on the slide. PowerPoint's
    // export of the same shapes shows 60x30 pt tiles at every zoom.
    let draw = |scale: f64, group: f64| {
        let mut p = Presentation::default();
        let media = p.add_media("t.png", "image/png", half_red_half_blue()).expect("media");
        let mode = PictureMode::Tile { tx: 0.0, ty: 0.0, sx: 10.0, sy: 10.0, flip: "none".into(), align: "tl".into() };
        let fill = Fill::Picture(PictureFill { media, mode, upright: true, ..Default::default() });
        let shape = Shape {
            id: ShapeId(22),
            xfrm: Some(Xfrm { rot: 20.0, ..Xfrm::new(0.0, 0.0, 240.0 / group, 120.0 / group) }),
            fill: Some(fill),
            line: no_line(),
            ..Default::default()
        };
        let shape = if group == 1.0 {
            Shape { xfrm: Some(Xfrm { rot: 20.0, ..Xfrm::new(100.0, 100.0, 240.0, 120.0) }), ..shape }
        } else {
            Shape {
                id: ShapeId(21),
                xfrm: Some(Xfrm::new(100.0, 100.0, 240.0, 120.0)),
                kind: ShapeKind::Group { children: vec![shape], child: Xfrm::new(0.0, 0.0, 240.0 / group, 120.0 / group) },
                ..Default::default()
            }
        };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![shape];
        render_slide(&p, 0, &RenderOpts { scale, ..Default::default() })
    };
    for group in [1.0, 0.25] {
        let (one, two) = (draw(1.0, group), draw(2.0, group));
        let (mut same, mut total) = (0, 0);
        for y in (120..220).step_by(7) {
            for x in (120..320).step_by(9) {
                let (a, b) = (one.pixel(x, y), two.pixel(2 * x, 2 * y));
                if close(a, WHITE, 5) && close(b, WHITE, 5) {
                    continue; // outside the turned rectangle
                }
                total += 1;
                same += usize::from(close(a, Rgba::rgb(b[0], b[1], b[2]), 40));
            }
        }
        assert!(total > 40 && same * 10 >= total * 9, "group scale {group}: {same} of {total} samples agree between zoom 1 and 2");
    }
}

#[test]
fn a_tiles_first_copy_sits_on_the_point_its_alignment_names() {
    let b = Rect::new(0.0, 0.0, 100.0, 50.0);
    assert_eq!(paint::tile_origin(b, "tl", (20.0, 10.0), (0.0, 0.0)), Point::new(0.0, 0.0));
    assert_eq!(paint::tile_origin(b, "ctr", (20.0, 10.0), (0.0, 0.0)), Point::new(40.0, 20.0));
    assert_eq!(paint::tile_origin(b, "br", (20.0, 10.0), (5.0, -5.0)), Point::new(85.0, 35.0));
    assert_eq!(paint::tile_origin(b, "nonsense", (20.0, 10.0), (f64::NAN, 1.0)), Point::new(0.0, 1.0));
}

/// `png` with a pHYs chunk saying `dpi` inserted after its header.
fn with_dpi(png: &[u8], dpi: f64) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for b in data {
            c ^= *b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    let ppm = (dpi / 0.0254).round() as u32;
    let mut chunk = b"pHYs".to_vec();
    chunk.extend_from_slice(&ppm.to_be_bytes());
    chunk.extend_from_slice(&ppm.to_be_bytes());
    chunk.push(1);
    let mut out = png[..33].to_vec(); // signature (8) + IHDR chunk (25)
    out.extend_from_slice(&9u32.to_be_bytes());
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc32(&chunk).to_be_bytes());
    out.extend_from_slice(&png[33..]);
    out
}

#[test]
fn picture_resolution_sizes_tiles() {
    let png = half_red_half_blue();
    assert_eq!(images::dpi(&png), None);
    let p48 = with_dpi(&png, 48.0);
    let d = images::dpi(&p48).unwrap_or_default();
    assert!((d.0 - 48.0).abs() < 0.01 && (d.1 - 48.0).abs() < 0.01, "{d:?}");
    // 8 × 4 px at 48 dpi is as big as 16 × 8 at 96.
    assert_eq!(images::natural_size(&p48), Some((16.0, 8.0)));
    assert_eq!(images::natural_size(&png), Some((8.0, 4.0)));
    // JFIF density in dots per inch.
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
    jpeg.extend_from_slice(b"JFIF\0\x01\x01\x01\x00\x48\x00\x48");
    assert_eq!(images::dpi(&jpeg), Some((72.0, 72.0)));
}

#[test]
fn gradient_lines_and_their_heads_take_the_gradient() {
    use deckcraft_model::style::{Gradient, GradientShape, GradientStop};
    let blue = Rgba::rgb(0, 0, 255);
    let grad = |angle: f64| {
        Fill::Gradient(Gradient {
            stops: vec![GradientStop { pos: 0.0, color: ColorRef::rgb(RED) }, GradientStop { pos: 1.0, color: ColorRef::rgb(blue) }],
            shape: GradientShape::Linear { angle, scaled: false },
            rotate_with_shape: true,
            tile: [0.0; 4],
            flip: None,
        })
    };
    // Left to right along a 20 pt line: red at its start, blue at its end, and the arrowhead at
    // the end blue too.
    let mut sh = arrow_line("triangle", 20.0, RED);
    if let Some(l) = &mut sh.line {
        l.fill = Some(grad(0.0));
    }
    let img = render(vec![sh]);
    let (start, mid, head) = (img.pixel(102, 100), img.pixel(200, 100), img.pixel(296, 100));
    assert!(start[0] > 200 && start[2] < 60, "{start:?}");
    assert!(mid[0] > 60 && mid[2] > 60, "{mid:?}");
    assert!(head[2] > 200 && head[0] < 60, "head {head:?}");
    // Top to bottom across a horizontal line: the gradient runs over the line's width.
    let mut sh = arrow_line("none", 20.0, RED);
    if let Some(l) = &mut sh.line {
        l.fill = Some(grad(90.0));
    }
    let img = render(vec![sh]);
    let (top, bottom) = (img.pixel(200, 91), img.pixel(200, 109));
    assert!(top[0] > 200 && top[2] < 60 && bottom[2] > 200 && bottom[0] < 60, "{top:?} {bottom:?}");
}

#[test]
fn a_groups_shadow_is_cast_by_its_children() {
    let child = |id: u32, x: f64| Shape {
        id: ShapeId(id),
        xfrm: Some(Xfrm::new(x, 0.0, 80.0, 100.0)),
        fill: solid(RED),
        line: no_line(),
        ..Default::default()
    };
    let g = Shape {
        id: ShapeId(22),
        xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)),
        kind: ShapeKind::Group { children: vec![child(23, 0.0), child(24, 120.0)], child: Xfrm::new(0.0, 0.0, 200.0, 100.0) },
        effects: Some(offset_shadow()),
        ..Default::default()
    };
    let img = render(vec![g]);
    // 20 pt to the right of each child: the shadow; in the gap only the left child's shadow shows.
    assert!(close(img.pixel(190, 150), Rgba::BLACK, 30), "{:?}", img.pixel(190, 150));
    assert!(close(img.pixel(210, 150), WHITE, 10), "{:?}", img.pixel(210, 150));
    assert!(close(img.pixel(310, 150), Rgba::BLACK, 30), "{:?}", img.pixel(310, 150));
    assert!(close(img.pixel(150, 150), RED, 10), "children drawn over it");
}

#[test]
fn patterns_between_whole_pixel_cells_blend_neighbouring_cells() {
    use deckcraft_model::style::PatternFill;
    // narVert (alternate cells) at 0.75 output pixels per cell: PowerPoint's export reads
    // 255, 128, 0 repeating across; drawing the nearest cell gave only black and white. Bilinear
    // sampling gives about 213, 128, 42 (closer on average over all presets, not exact).
    let g = |v: u8| ColorRef::rgb(Rgba::rgb(v, v, v));
    let sh = Shape {
        id: ShapeId(25),
        xfrm: Some(Xfrm::new(0.0, 0.0, 60.0, 60.0)),
        fill: Some(Fill::Pattern(PatternFill { preset: "narVert".into(), fg: g(0), bg: g(255) })),
        line: no_line(),
        ..Default::default()
    };
    let img = render(vec![sh]);
    let row: Vec<u8> = (12..24).map(|x| img.pixel(x, 20)[0]).collect();
    assert!(row.iter().any(|v| (100..=156).contains(v)), "a blended column: {row:?}");
    assert!(row.iter().any(|v| *v < 60) && row.iter().any(|v| *v > 200), "{row:?}");
    // The pattern repeats every 6 px (8 cells).
    assert!(row.iter().zip(row.iter().skip(6)).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 2), "{row:?}");
}

#[test]
fn a_bevelled_shape_is_lit_from_the_upper_left() {
    let base = Rgba::rgb(91, 155, 213);
    let bevelled = |raw: &str| Shape {
        id: ShapeId(26),
        xfrm: Some(Xfrm::new(100.0, 100.0, 130.0, 130.0)),
        fill: solid(base),
        line: no_line(),
        effects: Some(deckcraft_model::Effects { bevel: Some("circle".into()), raw3d: Some(raw.into()), ..Default::default() }),
        ..Default::default()
    };
    let raw = r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d><a:bevelT w="152400" h="76200"/></a:sp3d>"#;
    let img = render(vec![bevelled(raw)]);
    // PowerPoint: face (99, 166, 226); top of the band saturates, its bottom and right darken.
    let face = img.pixel(165, 165);
    assert!(close(face, Rgba::rgb(99, 166, 226), 8), "face {face:?}");
    let (top, bottom, right) = (img.pixel(165, 104)[2], img.pixel(165, 226)[2], img.pixel(226, 165)[2]);
    assert!(top > face[2] && bottom < face[2] && right < face[2], "top {top} bottom {bottom} right {right} face {}", face[2]);
    // Without a 3-D scene PowerPoint draws it flat.
    let img = render(vec![bevelled(r#"<a:sp3d><a:bevelT w="152400" h="76200"/></a:sp3d>"#)]);
    assert!(close(img.pixel(165, 104), base, 2) && close(img.pixel(165, 165), base, 2));
}

#[test]
fn table_borders_take_their_dashes_and_diagonals_are_drawn() {
    let mut t = deckcraft_model::Table::new(1, 1, 200.0, 100.0);
    t.style = "none".into();
    let mut dashed = Line::solid(ColorRef::rgb(RED), 4.0);
    dashed.dash = Some(deckcraft_model::style::Dash::Preset { name: "dash".into() });
    if let Some(c) = t.rows.first_mut().and_then(|r| r.cells.first_mut()) {
        c.borders[2] = Some(dashed);
        c.diag_down = Some(Line::solid(ColorRef::rgb(Rgba::rgb(0, 0, 255)), 4.0));
    }
    let sh = Shape { id: ShapeId(27), xfrm: Some(Xfrm::new(100.0, 100.0, 200.0, 100.0)), kind: ShapeKind::Table(t), ..Default::default() };
    let img = render(vec![sh]);
    // Dashes of 4 widths (16 pt) on, 3 (12 pt) off along the top edge, from its start.
    assert!(close(img.pixel(108, 100), RED, 40) && close(img.pixel(122, 100), WHITE, 30) && close(img.pixel(135, 100), RED, 40));
    // The diagonal from top-left to bottom-right passes through the middle.
    assert!(close(img.pixel(200, 150), Rgba::rgb(0, 0, 255), 40), "{:?}", img.pixel(200, 150));
}

#[test]
fn a_large_pictures_shadow_is_whole() {
    // A 400 px picture shown at 240 pt: its shadow was recoloured in the picture's pixel space and
    // covered only part of it.
    let mut counts = vec![];
    for side in [8u32, 400] {
        let mut p = Presentation::default();
        let media = p.add_media("image1.png", "image/png", png(side, side * 3 / 4, [0, 0, 255])).expect("media");
        let mut fx = offset_shadow();
        if let Some(o) = &mut fx.outer_shadow {
            o.blur = 8.0;
        }
        let sh = Shape {
            id: ShapeId(15),
            xfrm: Some(Xfrm { rot: 20.0, ..Xfrm::new(360.0, 300.0, 240.0, 160.0) }),
            kind: ShapeKind::Picture { fill: deckcraft_model::style::PictureFill { media, ..Default::default() } },
            effects: Some(fx),
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        counts.push(
            (0..img.height)
                .flat_map(|y| (0..img.width).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    let q = img.pixel(*x, *y);
                    q[0] < 200 && q[2] < 200
                })
                .count(),
        );
    }
    assert!(counts[1] > 3000 && (counts[0] as i64 - counts[1] as i64).abs() < 300, "{counts:?}");
}

#[test]
fn a_gradient_not_rotating_with_its_shape_keeps_to_the_slide() {
    use deckcraft_model::style::{Gradient, GradientShape, GradientStop};
    let blue = Rgba::rgb(0, 0, 255);
    for rotate_with_shape in [true, false] {
        let g = Gradient {
            stops: vec![GradientStop { pos: 0.0, color: ColorRef::rgb(RED) }, GradientStop { pos: 1.0, color: ColorRef::rgb(blue) }],
            shape: GradientShape::Linear { angle: 0.0, scaled: false },
            rotate_with_shape,
            tile: [0.0; 4],
            flip: None,
        };
        // 200 × 100 turned a quarter: it covers 150..250 across and 50..250 down.
        let sh = Shape {
            id: ShapeId(28),
            xfrm: Some(Xfrm { rot: 90.0, ..Xfrm::new(100.0, 100.0, 200.0, 100.0) }),
            fill: Some(Fill::Gradient(g)),
            line: no_line(),
            ..Default::default()
        };
        let img = render(vec![sh]);
        let (left, top) = (img.pixel(152, 150), img.pixel(200, 52));
        if rotate_with_shape {
            assert!(top[0] > 200 && top[2] < 60, "turned with the shape: red at the top {top:?}");
        } else {
            assert!(left[0] > 200 && left[2] < 60, "upright: red at the left {left:?}");
            assert!(img.pixel(248, 150)[2] > 200, "blue at the right");
        }
    }
}

#[test]
fn stars_and_right_arrows_have_sharp_corners_by_default() {
    let shape = |name: &str, join: Option<deckcraft_model::style::LineJoin>| Shape {
        id: ShapeId(29),
        xfrm: Some(Xfrm::new(100.0, 100.0, 100.0, 100.0)),
        geom: Geom::preset(name),
        fill: solid(WHITE),
        line: Some(Line { join, ..Line::solid(ColorRef::rgb(RED), 10.0) }),
        ..Default::default()
    };
    // A star5's top point mitred reaches about 16 pt above the box; rounded only 5.
    let img = render(vec![shape("star5", None)]);
    assert!(close(img.pixel(150, 90), RED, 40), "mitred point {:?}", img.pixel(150, 90));
    let img = render(vec![shape("star5", Some(deckcraft_model::style::LineJoin::Round))]);
    assert!(close(img.pixel(150, 90), WHITE, 10), "an explicit round join stays round");
    // A rectangle keeps PowerPoint's round default: its corner is cut round.
    let img = render(vec![shape("rect", None)]);
    assert!(close(img.pixel(95, 95), WHITE, 30), "{:?}", img.pixel(95, 95));
    assert!(deckcraft_geom::preset::mitred_by_default("rightArrow") && !deckcraft_geom::preset::mitred_by_default("leftArrow"));
}

#[test]
fn an_upright_picture_fill_stays_upright_in_a_turned_shape() {
    use deckcraft_model::style::PictureFill;
    let blue = Rgba::rgb(0, 0, 255);
    for upright in [false, true] {
        let mut p = Presentation::default();
        let media = p.add_media("t.png", "image/png", half_red_half_blue()).expect("media");
        // Half red (left) half blue picture in a 200 × 200 square turned a quarter (and so
        // covering the same square).
        let sh = Shape {
            id: ShapeId(30),
            xfrm: Some(Xfrm { rot: 90.0, ..Xfrm::new(100.0, 100.0, 200.0, 200.0) }),
            fill: Some(Fill::Picture(PictureFill { media, upright, ..Default::default() })),
            line: no_line(),
            ..Default::default()
        };
        std::sync::Arc::make_mut(&mut p.slides[0]).shapes = vec![sh];
        let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
        let (left, top) = (img.pixel(110, 200), img.pixel(200, 110));
        if upright {
            assert!(close(left, RED, 30) && close(img.pixel(290, 200), blue, 30), "upright: red left {left:?}");
        } else {
            assert!(close(top, RED, 30), "turned with the shape: red at the top {top:?}");
        }
    }
}

#[test]
fn a_picture_reflects_its_pixels() {
    let mut p = picture_deck(None);
    let s = std::sync::Arc::make_mut(&mut p.slides[0]);
    if let Some(sh) = s.shapes.first_mut() {
        let rf = deckcraft_model::style::Reflection { blur: 0.0, start_alpha: 1.0, end_alpha: 0.0, end_pos: 0.5, dist: 0.0, ..Default::default() };
        sh.effects = Some(deckcraft_model::Effects { reflection: Some(rf), ..Default::default() });
    }
    let img = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    // The blue picture spans 100..200 down; just below it, its reflection tints the page blue.
    let below = img.pixel(200, 204);
    assert!(below[2] > 200 && below[0] < 200, "{below:?}");
    assert!(close(img.pixel(200, 290), WHITE, 10), "faded out by half its height");
}

#[test]
fn a_3d_shapes_face_is_lit_even_without_a_bevel() {
    let base = Rgba::rgb(237, 125, 49);
    let shape = |raw: &str| Shape {
        id: ShapeId(31),
        xfrm: Some(Xfrm::new(100.0, 100.0, 150.0, 100.0)),
        fill: solid(base),
        line: no_line(),
        effects: Some(deckcraft_model::Effects { raw3d: Some(raw.into()), ..Default::default() }),
        ..Default::default()
    };
    // PowerPoint shows this face as about (251, 135, 56).
    let img = render(vec![shape(r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d/>"#)]);
    assert!(close(img.pixel(175, 150), Rgba::rgb(248, 136, 60), 8), "{:?}", img.pixel(175, 150));
    // No shape properties in 3-D: flat.
    let img = render(vec![shape(r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d>"#)]);
    assert!(close(img.pixel(175, 150), base, 1));
}

#[test]
fn a_tiny_dash_on_a_huge_shape_draws_without_running_out_of_memory() {
    // Found by fuzzing the pptx round trip: a 1 EMU `custDash` on a 2147483647 EMU square asked
    // the stroker for hundreds of thousands of dashes and aborted on a 7.5 GB allocation.
    let sh = Shape {
        id: ShapeId(12),
        xfrm: Some(Xfrm::new(0.0, 0.0, 169_093.0, 169_093.0)),
        geom: Geom::preset("rect"),
        line: Some(Line {
            fill: solid(RED),
            width: Some(0.0001),
            dash: Some(deckcraft_model::style::Dash::Custom { pattern: vec![(1.0, 1.0)] }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let img = render_slide(&deck_with(vec![sh]), 0, &RenderOpts { scale: 0.25, ..Default::default() });
    assert!(img.width > 0);
    // The pattern is scaled up only past the limit; ordinary lines keep theirs.
    let mut p = BezPath::new();
    p.move_to((0.0, 0.0));
    p.line_to((100.0, 0.0));
    assert_eq!(limit_dash_count(&p, vec![1.0, 1.0]), vec![1.0, 1.0]);
    p.line_to((4.0e6, 0.0));
    let pat = limit_dash_count(&p, vec![1.0, 1.0]);
    assert!((4.0e6 / (pat[0] + pat[1]) - MAX_DASHES).abs() < 1.0, "{pat:?}");
    // A custom pattern of 1,000 dashes counts each of them: the limit is on dashes drawn.
    let pat = limit_dash_count(&p, vec![1.0; 2000]);
    let period: f64 = pat.iter().sum();
    assert!((4.0e6 / period * 1000.0 - MAX_DASHES).abs() < 1.0, "{}", period);
}

/// A bar chart with 20,000 points, each with its own fill, and a sparse scatter of the same size:
/// the per-point fills and the scatter's "has any x" are looked up, not searched per point.
#[test]
fn many_point_fills_and_sparse_scatter_x_render() {
    use deckcraft_model::ShapeKind;
    use deckcraft_model::chart::{Chart, ChartType};
    use deckcraft_model::style::Fill;
    let n = 20_000;
    let chart = |kind: ChartType| {
        let mut c = Chart::sample(kind);
        c.series.truncate(1);
        c.categories = (0..n).map(|i| i.to_string()).collect();
        c.series[0].values = (0..n).map(|i| Some(i as f64)).collect();
        c
    };
    let mut bars = chart(ChartType::Column);
    bars.series[0].point_fills = (0..n).rev().map(|i| (i, Fill::solid(ColorRef::rgb(Rgba::rgb(200, 0, 0))))).collect();
    let mut scatter = chart(ChartType::Scatter);
    scatter.series[0].x_values = (0..n).map(|i| (i + 1 == n).then_some(1.0)).collect();
    let shape = |id: u32, c: Chart| Shape {
        id: ShapeId(id),
        xfrm: Some(Xfrm::new(10.0, 10.0, 400.0, 300.0)),
        kind: ShapeKind::Chart(Box::new(c)),
        ..Default::default()
    };
    let img = render(vec![shape(2, bars), shape(3, scatter)]);
    assert!(img.width > 0);
}
