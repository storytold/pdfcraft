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
    let mode_now = ctx.get().text_state.render_mode;
    let render_mode = match (glyph, mode_now) {
        (Glyph::Type3(_), TextRenderingMode::Invisible) => TextRenderingMode::Invisible,
        (Glyph::Type3(_), TextRenderingMode::Clip) => TextRenderingMode::Clip,
        // PdfCraft patch (64, extended): modes 4 to 6 render *and* clip. A Type 3 glyph adds
        // nothing to the clipping path, so the clip this text object builds is unknown -- and
        // dispatching it as `Fill` means `clip_glyph` never sees it, so nothing marked it. A
        // space in another font later in the same object would then leave `ET` installing an
        // empty clip and hiding the rest of the page (noted in review).
        (
            Glyph::Type3(_),
            TextRenderingMode::FillAndClip | TextRenderingMode::StrokeAndClip | TextRenderingMode::FillAndStrokeAndClip,
        ) => {
            ctx.get_mut().text_state.clip_unsupported = true;
            TextRenderingMode::Fill
        }
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
    // PdfCraft patch: this glyph is shown in a clipping mode, so the text object clips at `ET`
    // whether or not the glyph has an outline to clip to.
    context.get_mut().text_state.clip_shown = true;
    match glyph {
        // PdfCraft patch (64, extended): `try_outline`, not `outline`. A glyph the font has no
        // outline for came back as an empty path, the same answer a space gives, so a clip mode
        // installed an empty clip for it and nothing painted after `ET` -- the page lost content
        // over a glyph id the font simply does not have (noted in review). An outline that
        // resolved clips, even when it is empty; one that did not is unsupported, and `ET` then
        // leaves what follows unclipped.
        Glyph::Outline(o) => match o.try_outline() {
            Some(outline) => {
                let outline = transform * outline;
                if outline.segments().next().is_some() {
                    context.get_mut().text_state.clip_paths.extend(outline);
                }
            }
            None => {
                warn!("no outline for a glyph shown in a clipping text rendering mode");
                context.get_mut().text_state.clip_unsupported = true;
                return;
            }
        },
        Glyph::Type3(_) => {
            warn!("text rendering mode clip not implemented for shape glyphs");
            // PdfCraft patch (64, extended): a Type 3 glyph adds nothing to the clipping path in
            // modes 4 to 7 (ISO 32000-2 §9.3.6), so the clip this glyph belongs to cannot be
            // built. Marked unsupported rather than left empty: `ET` then leaves what follows
            // unclipped, which is the "no clip" this case wants. Treating it as an empty clip --
            // which is right for a glyph that simply has no outline -- would hide the rest of
            // the page instead.
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
