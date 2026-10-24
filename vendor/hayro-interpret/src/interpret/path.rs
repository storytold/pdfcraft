use crate::color::{Color, ColorSpace};
use crate::context::{Context, path_as_rect};
use crate::device::Device;
use crate::util::{BezPathExt, Float32Ext};
use crate::{FillRule, Paint, PathDrawMode, StrokeProps};
use kurbo::{BezPath, Cap, Join, PathEl, Point};
use smallvec::smallvec;
use std::borrow::Cow;

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
        let clip_path = context.get().ctm * context.path().clone();
        context.push_clip_path(clip_path, rule, device);
    }
}

pub(crate) fn close_path(context: &mut Context<'_>) {
    // This is necessary to prevent artifacts (see for example issue 157),
    // but it does cause some weird lines (maybe conflation artifacts) in
    // pdftc_900k_0907.
    //
    // PdfCraft patch (61): a path that is a single `m` is closed too. kurbo's `is_empty` counts
    // only segments, so upstream dropped the `h` of `x y m h`, and the single-point closed
    // subpath a round cap paints as a dot (ISO 32000-2 §8.5.3.2) became a trailing `m`, which
    // paints nothing.
    if context.path().elements().last() != Some(&PathEl::ClosePath)
        && !context.path().elements().is_empty()
    {
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

        match (
            (bbox.width() as f32).is_nearly_zero(),
            (bbox.height() as f32).is_nearly_zero(),
        ) {
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

    // PdfCraft patch (61): see `resolve_degenerate_subpaths`.
    let path = resolve_degenerate_subpaths(
        path.unwrap_or(context.path()),
        stroke_props.line_cap,
        stroke_props.line_width,
    );
    let path = path.as_ref();
    if path.elements().is_empty() {
        return;
    }
    let draw_mode = PathDrawMode::Stroke(stroke_props);

    if let Some(rect) = path_as_rect(path) {
        device.draw_rect(&rect, base_transform, &paint, &draw_mode);
    } else {
        device.draw_path(path, base_transform, &paint, &draw_mode);
    }
}

/// PdfCraft patch (61): `path` as `S` strokes it, by ISO 32000-2 §8.5.3.2: "If a subpath is
/// degenerate (consists of a single-point closed path or of two or more points at the same
/// coordinates), the S operator shall paint it only if round line caps have been specified,
/// producing a filled circle centred at the single point." With butt or projecting square caps
/// it "shall produce no output". So with round caps a degenerate subpath becomes a segment
/// centred on its point, a thousandth of the line width long (kurbo draws no cap on a segment of
/// length zero), and is left out for the other caps. The other subpaths, and zero-length dashes
/// of them, are not touched. Borrowed when nothing changes.
pub(crate) fn resolve_degenerate_subpaths(
    path: &BezPath,
    cap: Cap,
    line_width: f32,
) -> Cow<'_, BezPath> {
    let elements = path.elements();
    if !subpaths(elements).any(|sub| degenerate_point(sub).is_some()) {
        return Cow::Borrowed(path);
    }
    let mut out = BezPath::new();
    for sub in subpaths(elements) {
        match degenerate_point(sub) {
            Some(p) if cap == Cap::Round => {
                // A hairline (width 0) is widened by the device, so it keeps a length kurbo still
                // caps; an enormous width keeps the segment within half a unit.
                let half = (f64::from(line_width) * 5e-4).max(5e-5).min(0.5);
                out.move_to((p.x - half, p.y));
                out.line_to((p.x + half, p.y));
            }
            Some(_) => {}
            None => sub.iter().for_each(|el| out.push(*el)),
        }
    }
    Cow::Owned(out)
}

/// The subpaths of `elements`: each starts at a `MoveTo`, except perhaps the first.
fn subpaths(elements: &[PathEl]) -> impl Iterator<Item = &[PathEl]> {
    let mut rest = elements;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest
            .iter()
            .skip(1)
            .position(|el| matches!(el, PathEl::MoveTo(_)))
            .map_or(rest.len(), |i| i.saturating_add(1));
        let (sub, tail) = rest.split_at_checked(end).unwrap_or((rest, &[]));
        rest = tail;
        Some(sub)
    })
}

/// The point a degenerate subpath stays at: one that starts with a `MoveTo` and has at least one
/// segment or a `ClosePath`, every point of which (control points too) is the start point. A lone
/// `MoveTo` (a trailing `m`) is not one: it paints nothing whatever the caps.
fn degenerate_point(sub: &[PathEl]) -> Option<Point> {
    let (PathEl::MoveTo(start), rest) = sub.split_first()? else {
        return None;
    };
    let at = |p: &Point| p == start;
    let degenerate = !rest.is_empty()
        && rest.iter().all(|el| match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => at(p),
            PathEl::QuadTo(a, b) => at(a) && at(b),
            PathEl::CurveTo(a, b, c) => at(a) && at(b) && at(c),
            PathEl::ClosePath => true,
        });
    degenerate.then_some(*start)
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
        let color = Color::new(data.color_space, data.color, data.alpha);

        if let Some(tf) = &data.transfer_function {
            Paint::Color(Color::from_rgba(tf.apply(&color.to_rgba())))
        } else {
            Paint::Color(color)
        }
    }
}
