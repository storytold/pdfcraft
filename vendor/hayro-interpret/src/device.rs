use crate::cmyk_group::CmykGroup;
use crate::font::Glyph;
use crate::soft_mask::SoftMask;
use crate::{BlendMode, ClipPath, Image};
use crate::{GlyphDrawMode, Paint, PathDrawMode};
use kurbo::{Affine, BezPath, Rect, Shape};

/// A trait for a device that can be used to process PDF drawing instructions.
pub trait Device<'a> {
    /// Set the properties for future stroking operations.
    /// Set a soft mask to be used for future drawing instructions.
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'a>>);
    /// Set the blend mode that should be used for rendering operations.
    fn set_blend_mode(&mut self, blend_mode: BlendMode);
    /// Draw a path.
    fn draw_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &Paint<'a>,
        draw_mode: &PathDrawMode,
    );
    /// Push a new clip path to the clip stack.
    fn push_clip_path(&mut self, clip_path: &ClipPath);
    /// Push a new transparency group to the blend stack.
    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'a>>,
        blend_mode: BlendMode,
    );
    /// Draw a glyph.
    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        // TODO: Move this into outline glyph.
        draw_mode: &GlyphDrawMode,
    );
    /// Draw an image.
    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine);
    /// Pop the last clip path from the clip stack.
    fn pop_clip_path(&mut self);
    /// Pop the last transparency group from the blend stack.
    fn pop_transparency_group(&mut self);
    /// Draw a rectangle directly, without going through the general path pipeline.
    fn draw_rect(
        &mut self,
        rect: &Rect,
        transform: Affine,
        paint: &Paint<'a>,
        draw_mode: &PathDrawMode,
    ) {
        self.draw_path(&rect.to_path(0.1), transform, paint, draw_mode);
    }
    /// Called at the beginning of a marked content sequence (BMC/BDC).
    ///
    /// The tag is the marked content tag (e.g. b"P", b"Span"). The mcid is
    /// the marked content identifier from the properties dict, if present.
    fn begin_marked_content(&mut self, _tag: &[u8], _mcid: Option<i32>) {}
    /// PdfCraft patch (25): the `/ActualText` of the marked content sequence that just began
    /// (ISO 32000-2 §14.9.4), as the text string's raw bytes. Called right after
    /// `begin_marked_content`; the text replaces everything drawn up to the matching
    /// `end_marked_content`.
    fn actual_text(&mut self, _text: &[u8]) {}
    /// Called at the end of a marked content sequence (EMC).
    fn end_marked_content(&mut self) {}
    /// PdfCraft patch (29): the page's annotations are about to be drawn (until
    /// `end_annotations`), as page content finishes. Devices can use this to treat their
    /// paints as annotation colours rather than document colours — the reading-colours
    /// setting replaces document colours only, never the colours a comment was given.
    fn begin_annotations(&mut self) {}
    /// PdfCraft patch (29): the page's annotations are done; what follows is page content.
    fn end_annotations(&mut self) {}
    /// PdfCraft patch (39): the transparency group just pushed is a knockout group (`/K true`):
    /// each of its elements composites with the group's initial backdrop, not with the
    /// elements drawn before it in the group (ISO 32000-2, knockout groups), until the
    /// matching `pop_transparency_group`.
    fn set_knockout(&mut self) {}
    /// PdfCraft patch (39): an element whose shape is `shape` (device space) is about to be
    /// drawn into a transparency group of its own. In a knockout group (`set_knockout`) the
    /// element replaces what the group drew under its shape, so a device clears that first;
    /// the element's layer then composites over the cleared area as usual.
    fn knockout_shape(&mut self, _shape: &BezPath) {}
    /// PdfCraft patch (39): whether `knockout_shape` would clear anything now, that is whether
    /// the innermost transparency group is a knockout group. Lets the interpreter skip work it
    /// only needs for the clear.
    fn in_knockout_group(&mut self) -> bool {
        false
    }
    /// PdfCraft patch (39): `image`, an unmasked raster image, is about to be drawn with
    /// `transform` in a knockout group. A device that will draw it (it isn't skipped and it
    /// decodes) clears what the group drew under its placement rectangle first, so the image
    /// replaces it. Decided by the device, at the size it draws the image, so the draw reuses
    /// that decode.
    fn knockout_image(&mut self, _image: &crate::RasterImage<'_>, _transform: Affine) {}
    /// PdfCraft patch (39): whether a transparency group drawn now is an element of a knockout
    /// group that the device wants cleared under the group's shape first. If so the
    /// interpreter draws the group's content once between `begin_knockout_erase` and
    /// `end_knockout_erase`, in which every paint clears what is under its shape instead of
    /// painting, and then draws the group as usual, in a layer of its own.
    fn knockout_erase_wanted(&mut self) -> bool {
        false
    }
    /// PdfCraft patch (39): see `knockout_erase_wanted`.
    fn begin_knockout_erase(&mut self) {}
    /// PdfCraft patch (39): see `knockout_erase_wanted`.
    fn end_knockout_erase(&mut self) {}
    /// PdfCraft patch (67): the knockout group just pushed (`set_knockout`) is drawn once per
    /// blend mode its elements use, each pass into a layer of its own composited with that
    /// mode. In this pass only elements whose blend mode is `mode` paint; every other element
    /// clears what is under its shape, as it would replace it. Each pixel then holds the last
    /// element covering it in exactly one pass, which composites it with the group's backdrop
    /// in that element's own blend mode (#4886).
    fn begin_knockout_pass(&mut self, _mode: BlendMode) {}
    /// PdfCraft patch (67): ends the pass `begin_knockout_pass` started, before the layer is
    /// popped. Returns the other blend modes the pass's elements used, each once. A device
    /// that doesn't track them returns none, and the group is drawn in one Normal pass.
    fn end_knockout_pass(&mut self) -> Vec<BlendMode> {
        Vec::new()
    }
    /// PdfCraft patch (70): draw the contents of an isolated transparency group that blends
    /// in `DeviceCMYK` (ISO 32000-2 §11.3.4), already inside the layer it is composited
    /// through. Return `true` when the device drew them; on `false` (the default) the
    /// interpreter draws the contents into the device as for any other group.
    fn draw_isolated_cmyk_group(&mut self, _group: &CmykGroup<'a, '_>) -> bool {
        false
    }
    /// PdfCraft patch (70): an isolated transparency group that names a blending colour space
    /// other than `DeviceCMYK` is about to be drawn. A device drawing an enclosing group in CMYK
    /// can no longer draw that group right, since this one blends in its own space.
    fn group_blends_outside_device_cmyk(&mut self) {}
}

/// A device that discards all drawing operations.
pub struct DummyDevice;

impl Device<'_> for DummyDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'_>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'_>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'_>>, _: BlendMode) {}
    fn draw_glyph(
        &mut self,
        _: &Glyph<'_>,
        _: Affine,
        _: Affine,
        _: &Paint<'_>,
        _: &GlyphDrawMode,
    ) {
    }
    fn draw_image(&mut self, _: Image<'_, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}
