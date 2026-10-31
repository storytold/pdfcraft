use crate::color::{Color, ColorSpace, InkUse, Plate, device_cmyk_to_gray};
use crate::context::{Context, path_as_rect};
use crate::device::Device;
use crate::interpret::state::PaintData;
use crate::pattern::Pattern;
use crate::util::{BezPathExt, Float32Ext};
use crate::{FillRule, Paint, PathDrawMode, StrokeProps};
use kurbo::{BezPath, Cap, Join, PathEl};
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
        let clip_path = context.get().ctm * context.path().clone();
        context.push_clip_path(clip_path, rule, device);
    }
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

    // PdfCraft patch (53): report spot inks, and draw separations.
    if let Some(sink) = &context.settings.ink_sink {
        data.color_space.report_spots(sink);
        if let Some(Pattern::Shading(s)) = &data.pattern {
            s.shading.color_space.report_spots(sink);
        }
    }
    if let Some(plate) = &context.settings.plate {
        let state = &context.get().graphics_state;
        let overprint = if is_stroke {
            state.overprint_stroke
        } else {
            state.overprint_fill
        };
        return plate_paint(context, data, plate, overprint, state.overprint_mode);
    }

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

/// PdfCraft patch (53): the paint `data` as drawn on `plate`: a colour becomes its film, a
/// shading is converted through the plate, and a paint that leaves the plate as it is (an
/// overprinting paint that doesn't specify the plate's ink, or the `None` colorant) becomes
/// transparent. Transfer functions are left out: they act on device colorants, which the
/// film doesn't model.
fn plate_paint<'a>(
    context: &Context<'a>,
    data: PaintData<'a>,
    plate: &Plate,
    overprint: bool,
    overprint_mode: u8,
) -> Paint<'a> {
    let untouched = || Paint::Color(Color::film(0.0, 0.0));
    if !(data.color_space.is_pattern() || data.pattern.is_some()) {
        let color = Color::new(data.color_space, data.color, data.alpha);
        return Paint::Color(color.for_plate(plate, overprint, overprint_mode));
    }
    let Some(mut pattern) = data.pattern else {
        // Pattern was likely invalid, use transparent paint.
        return untouched();
    };
    match &mut pattern {
        Pattern::Shading(s) => {
            match s.shading.color_space.ink_use(&plate.ink) {
                InkUse::Nothing => return untouched(),
                InkUse::Unspecified if plate.overprint && overprint => return untouched(),
                InkUse::Paints | InkUse::Unspecified => {}
            }
            std::sync::Arc::make_mut(&mut s.shading).for_plate(plate);
        }
        Pattern::Tiling(t) => t.for_plate(plate, overprint, overprint_mode),
    }
    pattern.pre_concat_transform(context.root_transform());
    Paint::Pattern(Box::new(pattern))
}
