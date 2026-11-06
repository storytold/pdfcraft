use crate::color::{Color, ColorSpace, device_cmyk_to_gray};
use crate::context::{Context, path_as_rect};
use crate::device::Device;
use crate::util::{BezPathExt, Float32Ext};
use crate::{FillRule, Paint, PathDrawMode, StrokeProps};
use kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, Rect, Vec2};
use smallvec::smallvec;

pub(crate) fn fill_path<'a>(
    context: &mut Context<'a>,
    device: &mut impl Device<'a>,
    fill_rule: FillRule,
) {
    fill_path_impl(context, device, fill_rule, None);
    apply_pending_clip(context, device);

    context.path_mut().truncate(0);
}

pub(crate) fn stroke_path<'a>(context: &mut Context<'a>, device: &mut impl Device<'a>) {
    stroke_path_impl(context, device, None);
    apply_pending_clip(context, device);

    context.path_mut().truncate(0);
}

pub(crate) fn fill_stroke_path<'a>(
    context: &mut Context<'a>,
    device: &mut impl Device<'a>,
    fill_rule: FillRule,
) {
    fill_path_impl(context, device, fill_rule, None);
    stroke_path_impl(context, device, None);
    apply_pending_clip(context, device);

    context.path_mut().truncate(0);
}

/// PdfCraft patch: a clip set by `W` / `W*` takes effect once the path-painting operator that
/// ends the path has painted it, whichever operator that is (ISO 32000-2 §8.5.4). Upstream
/// applied it only on `n`, and otherwise left it pending for some later, unrelated `n`.
pub(crate) fn apply_pending_clip<'a>(context: &mut Context<'a>, device: &mut impl Device<'a>) {
    if let Some(rule) = context.clip_mut().take()
        && !context.path().elements().is_empty()
    {
        let ctm = context.get().ctm;
        // PdfCraft patch (65): a clip that encloses no area keeps the pixels a fill of it marks
        // (see `degenerate_fill_area`), as ISO 32000-2 §10.7.4 says of clipping: "the clipping
        // region consists of the set of pixels that would be included by a fill operation".
        match degenerate_fill_area(context.path(), ctm) {
            Some(area) => context.push_clip_path(area, FillRule::NonZero, device),
            None => {
                let clip_path = ctm * context.path().clone();
                context.push_clip_path(clip_path, rule, device);
            }
        }
    }
}

/// PdfCraft patch (65): the device-space area a fill marks for a path whose bounding box (user
/// space) has exactly no width or exactly no height, but not both, under `ctm`: one area per
/// subpath (see `degenerate_line_area`), kept as separate subpaths of one path, so a gap between
/// two subpaths on the same line stays unpainted (ISO 32000-2 §10.7.4 marks only the pixels the
/// shape touches). Every area winds the same way, so the non-zero rule paints their union. `None`
/// for a path with area, a path that is a point, or one none of whose subpaths is a line: those
/// paint as before.
pub(crate) fn degenerate_fill_area(path: &BezPath, ctm: Affine) -> Option<BezPath> {
    let bbox = path.fast_bounding_box();
    if (bbox.width() == 0.0) == (bbox.height() == 0.0) {
        return None;
    }
    let mut area = BezPath::new();
    let mut add = |subpath: &BezPath| {
        if subpath.elements().is_empty() {
            return;
        }
        let Some(mut corners) = degenerate_line_area(subpath.fast_bounding_box(), ctm) else {
            return;
        };
        // Shoelace: wind every area the same way, so overlapping ones add up under non-zero.
        let twice_area: f64 = (0..4)
            .map(|i| {
                let (p, q) = (corners[i], corners[(i + 1) % 4]);
                p.x * q.y - q.x * p.y
            })
            .sum();
        if twice_area < 0.0 {
            corners.reverse();
        }
        area.move_to(corners[0]);
        for corner in &corners[1..] {
            area.line_to(*corner);
        }
        area.close_path();
    };
    let mut subpath = BezPath::new();
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) {
            add(&subpath);
            subpath.truncate(0);
        }
        subpath.push(*el);
    }
    add(&subpath);
    (!area.elements().is_empty()).then_some(area)
}

/// PdfCraft patch (65): the corners of the device-space area a fill marks for one subpath whose
/// bounding box `bbox` (user space) has exactly no width or exactly no height, but not both,
/// under `ctm`. ISO 32000-2 §10.7.4 paints "any pixel whose half-open square region intersects
/// the shape, no matter how small the intersection is", and its example says "A zero-width or
/// zero-height rectangle paints a line 1 pixel wide". Such a path encloses no area, so the
/// rasteriser marks nothing: the fill used to fall back to a thinnest-line stroke along the box
/// (half-covering two rows when the line fell on a pixel boundary), and a clip of it hid
/// everything painted after it. A box with area, however thin, is not one of these: scaled up it
/// can cover many pixels.
///
/// When the line stays within one device row or column (counting a line less than 1/256 of a
/// pixel across as lying exactly along the axis, at its middle), the area is the run of whole
/// pixels it touches, by the half-open rule: from the floor of its low end to the ceiling of its
/// high end, at least one pixel. Otherwise (a rotated or skewed `ctm` that carries it across
/// rows and columns) it is a band one device pixel wide centred on the line, like the thinnest
/// line of §8.4.3.2, so each row or column is marked only where the line runs through it. `None`
/// for a box with area, a point (both sides zero), a non-finite box, or a `ctm` that collapses
/// the line to a point.
fn degenerate_line_area(bbox: Rect, ctm: Affine) -> Option<[Point; 4]> {
    let flat_x = bbox.width() == 0.0;
    let flat_y = bbox.height() == 0.0;
    if flat_x == flat_y {
        return None;
    }
    let a = ctm * Point::new(bbox.x0, bbox.y0);
    let b = ctm * Point::new(bbox.x1, bbox.y1);
    if ![a.x, a.y, b.x, b.y].iter().all(|v| v.is_finite()) {
        return None;
    }
    let along = b - a;
    let length = along.hypot();
    if !length.is_finite() || length <= 0.0 {
        return None;
    }
    let device = Rect::from_points(a, b);
    let (x0, x1) = touched_pixels(device.x0, device.x1);
    let (y0, y1) = touched_pixels(device.y0, device.y1);
    if x1 - x0 <= 1.0 || y1 - y0 <= 1.0 {
        return Some([Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)]);
    }
    let half = Vec2::new(-along.y, along.x) * (0.5 / length);
    Some([a + half, b + half, b - half, a - half])
}

/// PdfCraft patch (65): the device pixel boundaries `(first, end)` of the pixels a span `lo..=hi`
/// (`lo <= hi`, finite) touches along one axis by the half-open rule, at least one pixel. A span
/// under 1/256 of a pixel is taken at its middle, so a line along a device axis that a CTM's
/// rounding tilts slightly still marks one row or column.
fn touched_pixels(lo: f64, hi: f64) -> (f64, f64) {
    let (lo, hi) = if ((hi - lo) as f32).is_nearly_zero() {
        let mid = lo + (hi - lo) / 2.0;
        (mid, mid)
    } else {
        (lo, hi)
    };
    let first = lo.floor();
    (first, hi.ceil().max(first + 1.0))
}

pub(crate) fn close_path(context: &mut Context<'_>) {
    // This is necessary to prevent artifacts (see for example issue 157),
    // but it does cause some weird lines (maybe conflation artifacts) in
    // pdftc_900k_0907.
    if context.path().elements().last() != Some(&PathEl::ClosePath) && !context.path().is_empty() {
        context.path_mut().close_path();

        *(context.last_point_mut()) = *context.sub_path_start();
    }
}

pub(crate) fn fill_path_impl<'a>(
    context: &mut Context<'a>,
    device: &mut impl Device<'a>,
    fill_rule: FillRule,
    path: Option<&BezPath>,
) {
    if !context.ocg_state.is_visible() {
        return;
    }

    let base_transform = context.get().ctm;

    let paint = get_paint(context, false);
    device.set_soft_mask(context.get().graphics_state.soft_mask.clone());
    device.set_blend_mode(context.get().graphics_state.blend_mode);

    let mut draw = |path: &BezPath| {
        // pdf.js issue 4260: Replace zero-sized paths with a small stroke instead.
        let bbox = path.fast_bounding_box();

        // PdfCraft patch (65): see `degenerate_fill_area`. Painted with the non-zero rule: the
        // area is one simple shape, whatever rule the path was filled with.
        if let Some(area) = degenerate_fill_area(path, base_transform) {
            let draw_mode = PathDrawMode::Fill(FillRule::NonZero);
            if let Some(rect) = path_as_rect(&area) {
                device.draw_rect(&rect, Affine::IDENTITY, &paint, &draw_mode);
            } else {
                device.draw_path(&area, Affine::IDENTITY, &paint, &draw_mode);
            }
            return;
        }

        // PdfCraft patch (65): only a box with exactly no width or height (by now a point) is
        // stroked. A box with area, however thin, is filled as the shape it is: a "nearly zero"
        // side scaled up by the CTM can cover many device pixels.
        match (bbox.width() == 0.0, bbox.height() == 0.0) {
            (false, false) => {
                let draw_mode = PathDrawMode::Fill(fill_rule);
                if let Some(rect) = path_as_rect(path) {
                    device.draw_rect(&rect, base_transform, &paint, &draw_mode);
                } else {
                    device.draw_path(path, base_transform, &paint, &draw_mode);
                }
            }
            _ => {
                let mut path = BezPath::new();
                path.move_to((bbox.x0, bbox.y0));
                path.line_to((bbox.x1, bbox.y1));

                let stroke_props = StrokeProps {
                    // TODO: Make dependent on transform?
                    line_width: 0.001,
                    line_join: Join::Bevel,
                    line_cap: Cap::Butt,
                    ..Default::default()
                };

                device.draw_path(
                    &path,
                    base_transform,
                    &paint,
                    &PathDrawMode::Stroke(stroke_props),
                );
            }
        };
    };

    match path {
        None => draw(context.path()),
        Some(path) => draw(path),
    };
}

pub(crate) fn stroke_path_impl<'a>(
    context: &mut Context<'a>,
    device: &mut impl Device<'a>,
    path: Option<&BezPath>,
) {
    if !context.ocg_state.is_visible() {
        return;
    }

    let base_transform = context.get().ctm;

    let stroke_props = context.stroke_props();
    device.set_soft_mask(context.get().graphics_state.soft_mask.clone());
    device.set_blend_mode(context.get().graphics_state.blend_mode);
    let paint = get_paint(context, true);

    let path = path.unwrap_or(context.path());
    let draw_mode = PathDrawMode::Stroke(stroke_props);

    if let Some(rect) = path_as_rect(path) {
        device.draw_rect(&rect, base_transform, &paint, &draw_mode);
    } else {
        device.draw_path(path, base_transform, &paint, &draw_mode);
    }
}

pub(crate) fn get_paint<'a>(context: &Context<'a>, is_stroke: bool) -> Paint<'a> {
    let data = if is_stroke {
        context.get().stroke_data()
    } else {
        context.get().non_stroke_data()
    };

    if data.color_space.is_pattern() || data.pattern.is_some() {
        if let Some(mut pattern) = data.pattern {
            if let Some(tf) = &data.transfer_function {
                pattern.set_transfer_function(tf.clone());
            }

            // PdfCraft patch: `CA`/`ca` can be set by a `gs` after the pattern colour operator, so use the alpha current at paint time.
            pattern.set_alpha(data.alpha);
            pattern.pre_concat_transform(context.root_transform());

            Paint::Pattern(Box::new(pattern))
        } else {
            // Pattern was likely invalid, use transparent paint.
            Paint::Color(Color::new(ColorSpace::device_gray(), smallvec![0.0], 0.0))
        }
    } else {
        // PdfCraft patch (63): see `Context::luminosity_in_device_cmyk`.
        let color = if context.luminosity_in_device_cmyk && data.color_space.is_device_cmyk() {
            let gray = device_cmyk_to_gray(&data.color);
            Color::new(ColorSpace::device_gray(), smallvec![gray], data.alpha)
        } else {
            Color::new(data.color_space, data.color, data.alpha)
        };

        if let Some(tf) = &data.transfer_function {
            Paint::Color(Color::from_rgba(tf.apply(&color.to_rgba())))
        } else {
            Paint::Color(color)
        }
    }
}
