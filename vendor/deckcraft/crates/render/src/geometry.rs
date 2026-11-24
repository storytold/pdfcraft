use super::*;

pub(crate) fn color(c: Rgba, alpha: f64) -> peniko::Color {
    let a = (c.a as f64 * alpha.clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u8;
    peniko::Color::from_rgba8(c.r, c.g, c.b, a)
}

pub(crate) fn shade(c: Rgba, m: FillMode) -> Rgba {
    match m {
        FillMode::Lighten => c.lerp(Rgba::WHITE.with_alpha(c.a), 0.4),
        FillMode::LightenLess => c.lerp(Rgba::WHITE.with_alpha(c.a), 0.2),
        FillMode::Darken => c.lerp(Rgba::BLACK.with_alpha(c.a), 0.4),
        FillMode::DarkenLess => c.lerp(Rgba::BLACK.with_alpha(c.a), 0.2),
        _ => c,
    }
}

/// Geometry of a shape at its size.
pub fn shape_geometry(shape: &Shape, w: f64, h: f64) -> Geometry {
    match &shape.geom {
        Geom::Preset { name, adj } => preset::build(name, w, h, adj).or_else(|| preset::build("rect", w, h, &[])).unwrap_or(Geometry {
            paths: vec![],
            text_rect: Rect::new(0.0, 0.0, w, h),
            handles: vec![],
            sites: vec![],
        }),
        Geom::Custom { paths } => {
            let mut out = vec![];
            for p in paths {
                let (pw, ph) = (if p.w > 0.0 { p.w } else { w.max(1e-9) }, if p.h > 0.0 { p.h } else { h.max(1e-9) });
                let mut bp = parse_path(&p.d);
                bp.apply_affine(Affine::scale_non_uniform(w / pw, h / ph));
                // PowerPoint fills each custom path's sub-paths alternately (even-odd): a sub-path
                // inside another cuts a hole whichever way it winds, as in icons and outlined text.
                out.push(SubPath { path: bp, fill: p.fill, stroke: p.stroke, even_odd: true });
            }
            Geometry { paths: out, text_rect: Rect::new(0.0, 0.0, w, h), handles: vec![], sites: vec![] }
        }
    }
}

/// Parse our custom path syntax (`M x y L x y C … Q … Z`).
pub fn parse_path(d: &str) -> BezPath {
    let mut bp = BezPath::new();
    let toks: Vec<&str> = d.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).collect();
    let mut i = 0;
    let num = |i: usize| toks.get(i).and_then(|t| t.parse::<f64>().ok()).filter(|v| v.is_finite());
    let mut started = false;
    while i < toks.len() {
        let t = toks.get(i).copied().unwrap_or("");
        match t {
            "M" => {
                if let (Some(x), Some(y)) = (num(i + 1), num(i + 2)) {
                    bp.move_to((x, y));
                    started = true;
                }
                i += 3;
            }
            "L" => {
                if let (Some(x), Some(y)) = (num(i + 1), num(i + 2)) {
                    if !started {
                        bp.move_to((x, y));
                        started = true;
                    } else {
                        bp.line_to((x, y));
                    }
                }
                i += 3;
            }
            "C" => {
                if let (Some(a), Some(b), Some(c), Some(d2), Some(e), Some(f)) =
                    (num(i + 1), num(i + 2), num(i + 3), num(i + 4), num(i + 5), num(i + 6))
                    && started
                {
                    bp.curve_to((a, b), (c, d2), (e, f));
                }
                i += 7;
            }
            "Q" => {
                if let (Some(a), Some(b), Some(c), Some(d2)) = (num(i + 1), num(i + 2), num(i + 3), num(i + 4))
                    && started
                {
                    bp.quad_to((a, b), (c, d2));
                }
                i += 5;
            }
            "Z" | "z" => {
                if started {
                    bp.close_path();
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    bp
}

/// The transform a 3-D camera applies to shape `s` of size `w` x `h` (approximated by the affine
/// map that fits it best), as drawn; `None` for no camera, and for groups, tables and charts.
pub fn camera_turn(rctx: &Ctx, s: &Shape, w: f64, h: f64) -> Option<Affine> {
    if matches!(s.kind, ShapeKind::Group { .. } | ShapeKind::Table(_) | ShapeKind::Chart(_)) {
        return None;
    }
    let (e, _) = resolve::effects(rctx, s);
    e.as_ref().and_then(|e| e.raw3d.as_deref()).and_then(camera::from_raw).and_then(|c| camera::affine(&c, w, h))
}

/// The rectangle a picture's image occupies given its crop (fractions; negative = padding).
pub fn crop_dest(bounds: Rect, crop: [f64; 4]) -> Rect {
    let [l, t, r, b] = crop.map(|v| if v.is_finite() { v.clamp(-10.0, 0.99) } else { 0.0 });
    let vis_w = (1.0 - l - r).max(1e-6);
    let vis_h = (1.0 - t - b).max(1e-6);
    let fw = bounds.width() / vis_w;
    let fh = bounds.height() / vis_h;
    Rect::new(bounds.x0 - l * fw, bounds.y0 - t * fh, bounds.x0 - l * fw + fw, bounds.y0 - t * fh + fh)
}

/// Whether a shape's border is drawn outside its outline: a picture's is, unless the picture is
/// drawn in 3-D (a bevel seen through a 3-D scene), where PowerPoint centres it on the edge as on
/// any shape (observed: an 18 pt border covers 9 pt of the picture then).
pub(crate) fn border_outside(is_pic: bool, effects: Option<&Effects>) -> bool {
    is_pic && !effects.is_some_and(|e| e.bevel.is_some() && e.raw3d.as_deref().is_some_and(|r| r.contains("scene3d")))
}

/// The line that draws `line` (`lw` wide) outside a picture: twice as wide, so the half outside
/// the outline is the border, with dashes kept their length. `None` for compound lines, whose
/// bands would not survive the doubling (they stay centred on the outline).
pub(crate) fn outside_line(line: &Line, lw: f64) -> Option<Line> {
    if line.compound.is_some_and(|c| c != deckcraft_model::style::Compound::Single) {
        return None;
    }
    let dash = line.dash.as_ref().map(|d| {
        let p = d.pattern();
        deckcraft_model::style::Dash::Custom {
            pattern: p.chunks(2).map(|c| (c.first().copied().unwrap_or(0.0) / 2.0, c.get(1).copied().unwrap_or(0.0) / 2.0)).collect(),
        }
    });
    Some(Line { width: Some(lw * 2.0), dash: dash.or_else(|| line.dash.clone()), ..line.clone() })
}

/// A shape's outline as it casts a shadow: its line, that line's placeholder colour and opacity,
/// and the space it is stroked in (see `Renderer::line_space`).
pub(crate) struct Caster<'a> {
    pub(crate) line: std::borrow::Cow<'a, Line>,
    pub(crate) ph: Option<Rgba>,
    pub(crate) alpha: f64,
    pub(crate) base: Affine,
    pub(crate) rel: Affine,
}

/// How opaque a fill is overall (`None` when nothing is filled): a solid colour's alpha, a
/// gradient's mean stop alpha, otherwise opaque.
pub(crate) fn fill_opacity(rctx: &Ctx, fill: &Option<Fill>, ph: Option<Rgba>) -> Option<f64> {
    match fill {
        None | Some(Fill::None) => None,
        Some(Fill::Solid { color: c }) => Some(rctx.color(c, ph).a as f64 / 255.0),
        Some(Fill::Gradient(g)) if !g.stops.is_empty() => {
            Some(g.stops.iter().map(|s| rctx.color(&s.color, ph).a as f64 / 255.0).sum::<f64>() / g.stops.len() as f64)
        }
        Some(_) => Some(1.0),
    }
}

/// A shadow's own scale and skew (`sx`, `sy`, `kx`, `ky`) in the shape's space, about the point
/// of the shape's box its alignment names (`algn`: `b` is the middle of the bottom edge, `tl` the
/// top-left corner…), as PowerPoint applies them: skewed, then scaled. A perspective shadow
/// (`algn="b"`, `sy` 23%, `kx` -20°) so lies flat along the shape's bottom edge, not around its
/// middle.
pub(crate) fn shadow_local(sh: &deckcraft_model::style::Shadow, w: f64, h: f64) -> Affine {
    let (ax, ay) = match sh.align.as_str() {
        "tl" => (0.0, 0.0),
        "t" => (0.5, 0.0),
        "tr" => (1.0, 0.0),
        "l" => (0.0, 0.5),
        "r" => (1.0, 0.5),
        "bl" => (0.0, 1.0),
        "b" => (0.5, 1.0),
        "br" => (1.0, 1.0),
        _ => (0.5, 0.5),
    };
    let a = Vec2::new(w * ax, h * ay);
    let (tx, ty) = (sh.kx.to_radians().tan(), sh.ky.to_radians().tan());
    let k = Affine::new([sh.sx, sh.sy * ty, sh.sx * tx, sh.sy, 0.0, 0.0]);
    if !k.is_finite() {
        return Affine::IDENTITY;
    }
    Affine::translate(a) * k * Affine::translate(-a)
}

pub(crate) fn pre_translate(m: Affine, d: Vec2) -> Affine {
    Affine::translate(d) * m
}

pub(crate) fn scale_of(m: Affine) -> f64 {
    m.determinant().abs().sqrt()
}

pub(crate) fn blur(sigma: f64) -> Filter {
    Filter::from_primitive(FilterPrimitive::GaussianBlur { std_deviation: sigma.clamp(0.0, 500.0) as f32, edge_mode: EdgeMode::None })
}

/// The widest a shape may reach on screen, in pixels, and still be drawn: ten thousand screens
/// across, far past any real deck, and still cheap for vello to flatten (#1165).
const MAX_DEVICE_EXTENT: f64 = 1e7;

/// Whether a shape's geometry can go to vello. Geometry far beyond any screen (a size at the EMU
/// limit, group scales compounding, custom-geometry guides) makes vello's flattener size a buffer
/// by its on-screen length, and the allocation aborts the process (#1165). Each path is checked
/// for finite points first: a bounding box drops NaN, and a union with the frame would hide it.
/// The extent is the control box, which always holds the curve: the tight bounding box can miss
/// a curve's extremum when finding it overflows.
pub(crate) fn drawable(geo: &Geometry, m: Affine, w: f64, h: f64) -> bool {
    if !geo.paths.iter().all(|p| p.path.is_finite()) {
        return false;
    }
    let dev = m.transform_rect_bbox(geo.outline().control_box().union(Rect::new(0.0, 0.0, w, h)));
    dev.is_finite() && dev.width().max(dev.height()) <= MAX_DEVICE_EXTENT
}
