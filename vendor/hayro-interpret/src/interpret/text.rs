use crate::GlyphDrawMode;
use crate::context::Context;
use crate::device::Device;
use crate::font::Glyph;
use crate::interpret::path::get_paint;
use crate::interpret::state::TextStateFont;
use hayro_syntax::object;
use hayro_syntax::page::Resources;
use kurbo::Affine;

pub(crate) fn show_text_string<'a>(
    ctx: &mut Context<'a>,
    device: &mut impl Device<'a>,
    resources: &Resources<'a>,
    text: &object::String<'_>,
) {
    show_text_run(ctx, |ctx, pass| show_string_pass(ctx, device, resources, text, pass));
}

pub(crate) fn show_text_array<'a>(
    ctx: &mut Context<'a>,
    device: &mut impl Device<'a>,
    resources: &Resources<'a>,
    text: &object::Array<'_>,
) {
    show_text_run(ctx, |ctx, pass| {
        for item in text.iter::<object::Object<'_>>() {
            match item {
                object::Object::Number(number) => ctx.get_mut().text_state.apply_adjustment(number.as_f32()),
                object::Object::String(text) => show_string_pass(ctx, device, resources, &text, pass),
                _ => {}
            }
        }
    });
}

#[derive(Clone, Copy)]
enum TextPass {
    All,
    Fill,
    Stroke,
}

/// Fill the entire text-showing operation before stroking it, including all strings in TJ.
/// Replaying the source avoids an input-sized vector of positioned glyphs. Only the text
/// matrix advances while reading these operands; clipping paths from the first pass remain.
fn show_text_run<'a>(ctx: &mut Context<'a>, mut draw: impl FnMut(&mut Context<'a>, TextPass)) {
    if matches!(
        ctx.get().text_state.render_mode,
        TextRenderingMode::FillStroke | TextRenderingMode::FillAndStrokeAndClip
    ) {
        let start = ctx.get().text_state.text_matrix;
        draw(ctx, TextPass::Fill);
        let end = ctx.get().text_state.text_matrix;
        ctx.get_mut().text_state.text_matrix = start;
        draw(ctx, TextPass::Stroke);
        ctx.get_mut().text_state.text_matrix = end;
    } else {
        draw(ctx, TextPass::All);
    }
}

fn show_string_pass<'a>(
    ctx: &mut Context<'a>,
    device: &mut impl Device<'a>,
    resources: &Resources<'a>,
    text: &object::String<'_>,
    pass: TextPass,
) {
    let Some(font) = ctx.get().text_state.font.clone() else {
        warn!("tried to show text without active font");

        return;
    };

    let bytes = text.as_bytes();

    // In case we have a fallback font (which occurs if either no font was set at all
    // in the content stream, or an invalid one), we only want to show the glyphs
    // using Helvetica if the bytes are actually valid ASCII.
    let show_glyphs = matches!(font, TextStateFont::Font(_))
        || (matches!(font, TextStateFont::Fallback(_)) && bytes.is_ascii());

    let mut cur_idx = 0;

    while cur_idx < bytes.len() {
        let (code, adv) = font.read_code(bytes, cur_idx);
        cur_idx += adv;

        if show_glyphs {
            let (glyph, glyph_transform) = font.get_glyph(
                font.map_code(code),
                code,
                ctx,
                resources,
                font.origin_displacement(code),
            );
            show_glyph(ctx, device, &glyph, glyph_transform, pass);
        }

        ctx.get_mut().text_state.apply_code_advance(code, adv);
    }
}

pub(crate) fn next_line(ctx: &mut Context<'_>, tx: f64, ty: f64) {
    let new_matrix = ctx.get_mut().text_state.text_line_matrix * Affine::translate((tx, ty));
    ctx.get_mut().text_state.text_line_matrix = new_matrix;
    ctx.get_mut().text_state.text_matrix = new_matrix;
}

fn show_glyph<'a>(
    ctx: &mut Context<'a>,
    device: &mut impl Device<'a>,
    glyph: &Glyph<'a>,
    glyph_transform: Affine,
    pass: TextPass,
) {
    if !ctx.ocg_state.is_visible() {
        return;
    }

    // Type 3 procedures paint themselves and ignore fill/stroke modes. Keep the existing
    // single-paint behavior when an outline run requires a second, stroke-only pass.
    if matches!((glyph, pass), (Glyph::Type3(_), TextPass::Stroke)) {
        return;
    }

    device.set_soft_mask(ctx.get().graphics_state.soft_mask.clone());
    device.set_blend_mode(ctx.get().graphics_state.blend_mode);
    // Craft patch (flatten): tell the device the overprint parameters.
    crate::interpret::path::set_overprint(ctx, device);
    let stroke_props = ctx.stroke_props();

    // PdfCraft patch: only mode 3 has any effect on text in a Type 3 font (ISO 32000-1
    // 9.3.6). Stroking ran the glyph procedure a second time, so `2 Tr` made bitmap fonts
    // look bold, and the clip modes have no outline to clip with.
    let render_mode = match (glyph, ctx.get().text_state.render_mode, pass) {
        (Glyph::Type3(_), TextRenderingMode::Invisible, _) => TextRenderingMode::Invisible,
        (Glyph::Type3(_), _, _) => TextRenderingMode::Fill,
        (_, TextRenderingMode::FillAndStrokeAndClip, TextPass::Fill) => TextRenderingMode::FillAndClip,
        (_, _, TextPass::Fill) => TextRenderingMode::Fill,
        (_, _, TextPass::Stroke) => TextRenderingMode::Stroke,
        (_, mode, TextPass::All) => mode,
    };

    match render_mode {
        TextRenderingMode::Fill => {
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, false),
                &GlyphDrawMode::Fill,
            );
        }
        TextRenderingMode::Stroke => {
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, true),
                &GlyphDrawMode::Stroke(stroke_props),
            );
        }
        TextRenderingMode::FillStroke => {
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, false),
                &GlyphDrawMode::Fill,
            );
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, true),
                &GlyphDrawMode::Stroke(stroke_props),
            );
        }
        TextRenderingMode::Invisible => {
            // Still call draw_glyph for invisible text, so that it can
            // for example be used for text extraction.
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, false),
                &GlyphDrawMode::Invisible,
            );
        }
        TextRenderingMode::Clip => {
            clip_glyph(ctx, glyph, glyph_transform);
            // PdfCraft patch (#3116): report clipping text separately from invisible OCR text.
            // Extraction filters must retain text that is filled through its
            // clipping path without rendering devices drawing it directly.
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, false),
                &GlyphDrawMode::Clip,
            );
        }
        TextRenderingMode::FillAndClip => {
            clip_glyph(ctx, glyph, glyph_transform);
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, false),
                &GlyphDrawMode::Fill,
            );
        }
        TextRenderingMode::StrokeAndClip => {
            clip_glyph(ctx, glyph, glyph_transform);
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, true),
                &GlyphDrawMode::Stroke(stroke_props),
            );
        }
        TextRenderingMode::FillAndStrokeAndClip => {
            clip_glyph(ctx, glyph, glyph_transform);
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, false),
                &GlyphDrawMode::Fill,
            );
            device.draw_glyph(
                glyph,
                ctx.get().ctm,
                glyph_transform,
                &get_paint(ctx, true),
                &GlyphDrawMode::Stroke(stroke_props),
            );
        }
    }
}

pub(crate) fn clip_glyph(context: &mut Context<'_>, glyph: &Glyph<'_>, transform: Affine) {
    // PdfCraft patch: this glyph is shown in a clipping mode, so the text object clips at `ET`
    // whether or not the glyph has an outline to clip to.
    context.get_mut().text_state.clip_shown = true;
    match glyph {
        Glyph::Outline(o) => {
            let outline = transform * o.outline();
            let has_outline = outline.segments().next().is_some();

            if has_outline {
                context.get_mut().text_state.clip_paths.extend(outline);
            }
        }
        Glyph::Type3(_) => {
            warn!("text rendering mode clip not implemented for shape glyphs");
            // PdfCraft patch: the clip this glyph belongs to cannot be built, so `ET` leaves what
            // follows unclipped rather than applying an empty clip and hiding it.
            context.get_mut().text_state.clip_unsupported = true;
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum TextRenderingMode {
    #[default]
    Fill,
    Stroke,
    FillStroke,
    Invisible,
    FillAndClip,
    StrokeAndClip,
    FillAndStrokeAndClip,
    Clip,
}
