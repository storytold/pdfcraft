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
            show_glyph(ctx, device, &glyph, glyph_transform);
        }

        ctx.get_mut().text_state.apply_code_advance(code, adv);
    }
}

pub(crate) fn next_line(ctx: &mut Context<'_>, tx: f64, ty: f64) {
    let new_matrix = ctx.get_mut().text_state.text_line_matrix * Affine::translate((tx, ty));
    ctx.get_mut().text_state.text_line_matrix = new_matrix;
    ctx.get_mut().text_state.text_matrix = new_matrix;
}

pub(crate) fn show_glyph<'a>(
    ctx: &mut Context<'a>,
    device: &mut impl Device<'a>,
    glyph: &Glyph<'a>,
    glyph_transform: Affine,
) {
    if !ctx.ocg_state.is_visible() {
        return;
    }

    device.set_soft_mask(ctx.get().graphics_state.soft_mask.clone());
    device.set_blend_mode(ctx.get().graphics_state.blend_mode);
    let stroke_props = ctx.stroke_props();

    // PdfCraft patch: text in a Type 3 font is rendered from its glyph descriptions in every mode
    // but 3 and 7 (ISO 32000-2 §9.3.6). Stroking ran the glyph procedure a second time, so
    // `2 Tr` made bitmap fonts look bold.
    // PdfCraft patch (64): in modes 3 and 7 the text "shall not be rendered" (§9.3.6); `7 Tr`
    // painted the glyphs in the fill colour. It is still reported as clipping text, but in
    // modes 4 to 7 "nothing shall be added to the clipping path" (`clip_glyph`).
    let render_mode = match (glyph, ctx.get().text_state.render_mode) {
        (Glyph::Type3(_), TextRenderingMode::Invisible) => TextRenderingMode::Invisible,
        (Glyph::Type3(_), TextRenderingMode::Clip) => TextRenderingMode::Clip,
        (Glyph::Type3(_), _) => TextRenderingMode::Fill,
        (_, mode) => mode,
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
            // PdfCraft patch: report clipping text separately from invisible OCR text.
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
    match glyph {
        Glyph::Outline(o) => {
            let outline = transform * o.outline();
            let has_outline = outline.segments().next().is_some();

            if has_outline {
                context.get_mut().text_state.clip_paths.extend(outline);
            }
        }
        // PdfCraft patch (64): a Type 3 glyph adds nothing to the clipping path in modes 4 to 7
        // (ISO 32000-2 §9.3.6), so text shown only in a Type 3 font leaves no clip at `ET`.
        Glyph::Type3(_) => {}
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
