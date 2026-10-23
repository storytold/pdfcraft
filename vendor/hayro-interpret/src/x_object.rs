use crate::cache::{Cache, DecodedImageCache, DecodedImageKey};
use crate::color::{ColorComponents, ColorSpace, DefaultColorSpaces, DeviceFamily, ToRgb};
use crate::context::Context;
use crate::device::Device;
use crate::function::{Function, interpolate};
use crate::interpret::path::get_paint;
use crate::interpret::state::ActiveTransferFunction;
use crate::{BlendMode, CacheKey, ClipPath, Image, Paint, RasterImage, StencilImage};
use crate::{FillRule, InterpreterWarning, WarningSinkFn, interpret};
use crate::{ImageData, LumaData, RgbData};
use hayro_syntax::bit_reader::BitReader;
use hayro_syntax::content::TypedIter;
use hayro_syntax::object::Array;
use hayro_syntax::object::Dict;
use hayro_syntax::object::Name;
use hayro_syntax::object::Object;
use hayro_syntax::object::Stream;
use hayro_syntax::object::dict::keys::*;
use hayro_syntax::object::stream::{FilterResult, ImageColorSpace, ImageDecodeParams};
use hayro_syntax::page::Resources;
use kurbo::{Affine, Rect, Shape};
use smallvec::{SmallVec, smallvec};
use std::borrow::Cow;
use std::iter;
use std::ops::Deref;

pub(crate) enum XObject<'a> {
    FormXObject(FormXObject<'a>),
    ImageXObject(ImageXObject<'a>),
}

impl<'a> XObject<'a> {
    pub(crate) fn new(
        stream: &Stream<'a>,
        warning_sink: &WarningSinkFn,
        cache: &Cache,
        decoded_cache: Option<&DecodedImageCache>,
        transfer_function: Option<ActiveTransferFunction>,
        defaults: &DefaultColorSpaces<'_, '_>,
    ) -> Option<Self> {
        let dict = stream.dict();
        match dict.get::<Name<'_>>(SUBTYPE)?.deref() {
            IMAGE => Some(Self::ImageXObject(ImageXObject::new(
                stream,
                |_| None,
                warning_sink,
                cache,
                decoded_cache,
                false,
                transfer_function,
                Some(defaults),
            )?)),
            FORM => Some(Self::FormXObject(FormXObject::new(stream)?)),
            _ => None,
        }
    }
}

pub(crate) struct FormXObject<'a> {
    // PdfCraft patch: counted against the page's content budget while the form is alive.
    pub(crate) decoded: crate::context::DecodedContent<'a>,
    pub(crate) matrix: Affine,
    pub(crate) bbox: [f32; 4],
    is_transparency_group: bool,
    // PdfCraft patch: the group's /I and /K (ISO 32000-2 §11.6.6, both false by default).
    is_isolated: bool,
    is_knockout: bool,
    pub(crate) dict: Dict<'a>,
    resources: Option<Dict<'a>>,
}

impl<'a> FormXObject<'a> {
    pub(crate) fn new(stream: &Stream<'a>) -> Option<Self> {
        let dict = stream.dict();

        // PdfCraft patch: see `context::MAX_PAGE_CONTENT`.
        let decoded = crate::context::decode_content(stream)?;
        // PdfCraft patch: retain absence; present malformed dictionaries stay local-empty.
        let resources = dict.contains_key(RESOURCES).then(|| dict.get::<Dict<'_>>(RESOURCES).unwrap_or_default());

        let matrix = Affine::new(
            dict.get::<[f64; 6]>(MATRIX)
                .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        );
        let bbox = dict.get::<[f32; 4]>(BBOX)?;
        let group = dict.get::<Dict<'_>>(GROUP);
        let is_transparency_group = group.is_some();
        let is_isolated = group.as_ref().and_then(|g| g.get::<bool>(I)).unwrap_or(false);
        let is_knockout = group.as_ref().and_then(|g| g.get::<bool>(K)).unwrap_or(false);

        Some(Self {
            decoded,
            matrix,
            is_transparency_group,
            is_isolated,
            is_knockout,
            bbox,
            dict: dict.clone(),
            resources,
        })
    }
}

pub(crate) fn draw_xobject<'a>(
    x_object: &XObject<'a>,
    resources: &Resources<'a>,
    context: &mut Context<'a>,
    device: &mut impl Device<'a>,
) {
    match x_object {
        XObject::FormXObject(f) => draw_form_xobject(resources, f, context, device),
        XObject::ImageXObject(i) => {
            draw_image_xobject(i, context, device);
        }
    }
}

pub(crate) fn draw_form_xobject<'a, 'b>(
    resources: &Resources<'a>,
    x_object: &'b FormXObject<'a>,
    context: &mut Context<'a>,
    device: &mut impl Device<'a>,
) {
    if !context.ocg_state.is_visible() {
        return;
    }

    if !context.begin_nested_interpretation() {
        return;
    }

    let has_oc = xobject_oc(&x_object.dict, context);
    if !context.ocg_state.is_visible() {
        if has_oc {
            context.ocg_state.end_marked_content();
        }
        context.end_nested_interpretation();
        return;
    }

    let iter = TypedIter::new(x_object.decoded.as_ref());

    context.path_mut().truncate(0);
    context.save_state();
    context.pre_concat_affine(x_object.matrix);
    context.push_root_transform();

    // PdfCraft patch: a transparency group that is neither isolated nor knockout composites
    // its elements with the backdrop already on the page (§11.4.7), so when it is entered at
    // full opacity with no soft mask and Normal blending there is nothing for a layer of its
    // own to add, and drawing straight onto the page is the exact result. A layer here was
    // wrong, not just slower: a blend mode inside the group (the `/BM /Multiply` of a highlight
    // appearance written by other editors) saw the layer's transparent backdrop instead of the
    // text, and the layer then covered the page as an opaque block (#1968). An isolated group
    // starts from a blank backdrop by definition, and a knockout group or one entered at partial
    // opacity, under a soft mask or with a blend mode keeps its layer, where the group's
    // elements still composite with a blank backdrop (an approximation of §11.4.7 for them).
    let neutral = !x_object.is_isolated && !x_object.is_knockout && {
        let state = &context.get().graphics_state;
        state.non_stroke_alpha >= 1.0
            && state.stroke_alpha >= 1.0
            && state.soft_mask.is_none()
            && state.blend_mode == BlendMode::Normal
    };
    // PdfCraft patch (39): a group drawn as an element of a knockout group replaces what that
    // group drew under its shape, the union of its own elements' shapes (#2971). The shape is
    // cleared by drawing the group's content once in the device's erase mode; the group then
    // composites over the cleared area in a layer of its own (also when it would otherwise be
    // drawn straight onto its parent, where its elements would knock each other out).
    let knockout_element = x_object.is_transparency_group && device.knockout_erase_wanted();
    if knockout_element {
        context.save_state();
        device.begin_knockout_erase();
        device.push_clip_path(&ClipPath {
            path: context.get().ctm
                * Rect::new(
                    x_object.bbox[0] as f64,
                    x_object.bbox[1] as f64,
                    x_object.bbox[2] as f64,
                    x_object.bbox[3] as f64,
                )
                .to_path(0.1),
            fill: FillRule::NonZero,
        });
        interpret(
            TypedIter::new(x_object.decoded.as_ref()),
            &Resources::for_form(x_object.resources.clone(), resources.clone()),
            context,
            device,
        );
        device.pop_clip_path();
        device.end_knockout_erase();
        context.restore_state(device);
        context.path_mut().truncate(0);
    }
    let own_layer = x_object.is_transparency_group && (!neutral || knockout_element);

    if own_layer {
        device.push_transparency_group(
            context.get().graphics_state.non_stroke_alpha,
            std::mem::take(&mut context.get_mut().graphics_state.soft_mask),
            std::mem::take(&mut context.get_mut().graphics_state.blend_mode),
        );

        context.get_mut().graphics_state.non_stroke_alpha = 1.0;
        context.get_mut().graphics_state.stroke_alpha = 1.0;
        // PdfCraft patch (39): its elements knock each other out (#2971).
        if x_object.is_knockout {
            device.set_knockout();
        }
    }

    device.set_soft_mask(context.get().graphics_state.soft_mask.clone());
    device.set_blend_mode(context.get().graphics_state.blend_mode);

    device.push_clip_path(&ClipPath {
        path: context.get().ctm
            * Rect::new(
                x_object.bbox[0] as f64,
                x_object.bbox[1] as f64,
                x_object.bbox[2] as f64,
                x_object.bbox[3] as f64,
            )
            .to_path(0.1),
        fill: FillRule::NonZero,
    });

    interpret(
        iter,
        &Resources::for_form(x_object.resources.clone(), resources.clone()),
        context,
        device,
    );

    device.pop_clip_path();

    if own_layer {
        device.pop_transparency_group();
    }

    context.pop_root_transform();
    context.restore_state(device);

    if has_oc {
        context.ocg_state.end_marked_content();
    }

    context.end_nested_interpretation();
}

pub(crate) fn draw_image_xobject<'a, 'b>(
    x_object: &ImageXObject<'b>,
    context: &mut Context<'a>,
    device: &mut impl Device<'a>,
) {
    if !context.ocg_state.is_visible() {
        return;
    }

    let has_oc = xobject_oc(x_object.stream.dict(), context);
    if !context.ocg_state.is_visible() {
        if has_oc {
            context.ocg_state.end_marked_content();
        }
        return;
    }

    let width = x_object.width as f64;
    let height = x_object.height as f64;

    context.save_state();
    context.pre_concat_affine(Affine::new([
        1.0 / width,
        0.0,
        0.0,
        -1.0 / height,
        0.0,
        1.0,
    ]));
    let transform = context.get().ctm;

    let has_alpha = x_object.has_mask();

    let mut soft_mask = std::mem::take(&mut context.get_mut().graphics_state.soft_mask);
    let blend_mode = std::mem::take(&mut context.get_mut().graphics_state.blend_mode);

    // If image has smask, the soft mask from the graphics state should be discarde.
    if has_alpha {
        soft_mask = None;
    }

    // PdfCraft patch (39): an image without a mask covers its whole placement rectangle, so in
    // a knockout group it replaces what the group drew there (#2971). A stencil's or a masked
    // image's shape is its mask; those composite as before (exact where the mask is opaque or
    // transparent, which is what masks mostly are). Only an image the device draws clears: one
    // that doesn't decode (over `MAX_IMAGE_PIXELS`, damaged data) or that it skips (a preview's
    // tiny image) replaces nothing. The device decides, at its own draw size (review on #2974:
    // a full-size decode here cost a second decode of every downscaled image).
    if !x_object.is_mask && !has_alpha && device.in_knockout_group() {
        device.knockout_image(&RasterImage(x_object.clone()), transform);
    }

    device.push_transparency_group(
        context.get().graphics_state.non_stroke_alpha,
        std::mem::take(&mut soft_mask),
        blend_mode,
    );

    device.set_soft_mask(None);
    device.set_blend_mode(BlendMode::default());
    // PdfCraft patch (46): the group just pushed applies the fill opacity, so a stencil mask's
    // paint (the fill colour) must not carry it again: at `ca 0.5` a mask was drawn at a
    // quarter of its opacity (#2975). The state is restored below.
    context.get_mut().graphics_state.non_stroke_alpha = 1.0;

    let image = if x_object.is_mask {
        Image::Stencil(StencilImage {
            // A pattern paint keeps the opacity it was set with: drop it too (patch 46).
            paint: match get_paint(context, false) {
                Paint::Pattern(pattern) => Paint::Pattern(Box::new(pattern.without_opacity())),
                paint => paint,
            },
            image_xobject: x_object.clone(),
        })
    } else {
        Image::Raster(RasterImage(x_object.clone()))
    };

    device.draw_image(image, transform);
    device.pop_transparency_group();

    context.restore_state(device);

    if has_oc {
        context.ocg_state.end_marked_content();
    }
}

fn xobject_oc(dict: &Dict<'_>, context: &mut Context<'_>) -> bool {
    let Some(oc_dict) = dict.get::<Dict<'_>>(OC) else {
        return false;
    };

    if let Some(oc_ref) = dict.get_ref(OC) {
        context.ocg_state.begin_ocg(&oc_dict, oc_ref.into());
    } else {
        context.ocg_state.begin_ocmd(&oc_dict);
    }

    true
}

#[derive(Clone)]
pub(crate) struct ImageXObject<'a> {
    width: u32,
    height: u32,
    color_space: Option<ColorSpace>,
    cache: Cache,
    /// PdfCraft patch: decoded pixels shared across renders (see `decoded_raster`); `None`
    /// for an image that is never cached (an inline image).
    decoded_cache: Option<DecodedImageCache>,
    interpolate: bool,
    is_mask: bool,
    is_stencil_mask: bool,
    stream: Stream<'a>,
    transfer_function: Option<ActiveTransferFunction>,
    warning_sink: WarningSinkFn,
    /// PdfCraft patch (38): the cache key of the default colour space drawn in place of the
    /// image's device space, if one is.
    default_space: Option<u128>,
}

impl<'a> ImageXObject<'a> {
    /// PdfCraft patch (38): an image in a device space is drawn in its default colour space
    /// from `defaults`. Masks have none to replace, and `resolve_cs` (an inline image's named
    /// colour space) substitutes its own.
    pub(crate) fn new(
        stream: &Stream<'a>,
        resolve_cs: impl FnOnce(&Name<'_>) -> Option<ColorSpace>,
        warning_sink: &WarningSinkFn,
        cache: &Cache,
        decoded_cache: Option<&DecodedImageCache>,
        mut is_mask: bool,
        transfer_function: Option<ActiveTransferFunction>,
        defaults: Option<&DefaultColorSpaces<'_, '_>>,
    ) -> Option<Self> {
        let dict = stream.dict();

        let is_stencil_mask = dict
            .get::<bool>(IM)
            .or_else(|| dict.get::<bool>(IMAGE_MASK))
            .unwrap_or(false);
        is_mask |= is_stencil_mask;

        let mut default_space = None;
        let image_cs = if is_mask {
            // Masks are always single-channel.
            Some(ColorSpace::device_gray())
        } else {
            let cs_obj = dict
                .get::<Object<'_>>(CS)
                .or_else(|| dict.get::<Object<'_>>(COLORSPACE));

            let default = cs_obj
                .as_ref()
                .and_then(DeviceFamily::of_object)
                .zip(defaults)
                .and_then(|(family, defaults)| defaults.get(family));
            if let Some((space, key)) = default {
                default_space = Some(key);
                Some(space)
            } else {
                cs_obj
                    .clone()
                    .and_then(|c| ColorSpace::new(c, cache))
                    // Inline images can also refer to color spaces by name.
                    .or_else(|| {
                        cs_obj
                            .and_then(|c| c.into_name())
                            .and_then(|n| resolve_cs(&n))
                    })
            }
        };

        let interpolate = dict
            .get::<bool>(I)
            .or_else(|| dict.get::<bool>(INTERPOLATE))
            .unwrap_or(false);

        let width = dict.get::<u32>(W).or_else(|| dict.get::<u32>(WIDTH))?;
        let height = dict.get::<u32>(H).or_else(|| dict.get::<u32>(HEIGHT))?;

        if width == 0 || height == 0 {
            return None;
        }

        Some(Self {
            width,
            cache: cache.clone(),
            decoded_cache: decoded_cache.cloned(),
            height,
            color_space: image_cs,
            warning_sink: warning_sink.clone(),
            transfer_function,
            interpolate,
            stream: stream.clone(),
            is_mask,
            is_stencil_mask,
            default_space,
        })
    }

    pub(crate) fn decoded_mask(&self, target_dimension: Option<(u32, u32)>) -> Option<DecodedMask> {
        if !self.is_mask {
            return None;
        }

        // PdfCraft patch: decode once per `DecodedImageKey`; tiles and scales share it.
        let Some(cache) = &self.decoded_cache else {
            return decode_mask(self, target_dimension);
        };
        cache.get_or_insert_with(
            self.decode_key(cache, true, target_dimension),
            MAX_DECODED_IMAGE_BYTES,
            decoded_mask_size,
            || decode_mask(self, target_dimension),
        )
    }

    pub(crate) fn decoded_raster(
        &self,
        target_dimension: Option<(u32, u32)>,
    ) -> Option<DecodedRaster> {
        if self.is_mask {
            return None;
        }

        // PdfCraft patch: decode once per `DecodedImageKey`; tiles and scales share it.
        let Some(cache) = &self.decoded_cache else {
            return decode_raster(self, target_dimension);
        };
        cache.get_or_insert_with(
            self.decode_key(cache, false, target_dimension),
            MAX_DECODED_IMAGE_BYTES,
            decoded_raster_size,
            || decode_raster(self, target_dimension),
        )
    }

    /// PdfCraft patch: the cache key of decoding this image (as a mask when `mask_output`) at
    /// `target_dimension`: every input of the decode (see [`DecodedImageKey`]). `None` when one
    /// of them can't be keyed cheaply (a transfer function): such a decode isn't cached.
    pub(crate) fn decode_key(
        &self,
        cache: &DecodedImageCache,
        mask_output: bool,
        target_dimension: Option<(u32, u32)>,
    ) -> Option<DecodedImageKey> {
        if self.transfer_function.is_some() {
            return None;
        }
        let dict = self.stream.dict();
        let bytes = dict.data();
        Some(DecodedImageKey {
            mask_output,
            dict_at: bytes.as_ptr() as usize,
            dict_len: bytes.len(),
            dict_hash: self.stream.cache_key(),
            obj: dict.obj_id().map(|id| (id.obj_number, id.gen_number)),
            is_mask: self.is_mask,
            is_stencil_mask: self.is_stencil_mask,
            interpolate: self.interpolate,
            // PdfCraft patch (38): the same image drawn in a default colour space or without one.
            default_space: self.default_space,
            // PdfCraft patch (32): see `decode_reads_target`.
            target: if self.decode_reads_target() {
                target_dimension
            } else {
                None
            },
            preview: cache.preview,
        })
    }

    /// PdfCraft patch (32): whether decoding this image depends on the size it is drawn at.
    /// Only the JPX filter decodes at a reduced resolution for a target size, in the image
    /// itself or in the soft mask or mask decoded with it; every other filter decodes the whole
    /// image whatever the target. Keyed by the target, a scan was decoded again at every zoom
    /// step and kept once per zoom level; without it, one decode serves them all.
    fn decode_reads_target(&self) -> bool {
        let dict = self.stream.dict();
        uses_jpx(&self.stream)
            || dict.get::<Stream<'_>>(SMASK).is_some_and(|s| uses_jpx(&s))
            || dict.get::<Stream<'_>>(MASK).is_some_and(|s| uses_jpx(&s))
    }

    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    pub(crate) fn stream(&self) -> &Stream<'a> {
        &self.stream
    }

    fn has_mask(&self) -> bool {
        let dict = self.stream.dict();

        dict.contains_key(SMASK_IN_DATA) || dict.contains_key(SMASK) || dict.contains_key(MASK)
    }
}

/// PdfCraft patch: budget for [`DecodedImageCache`] entries per interpreter cache: decoded
/// image pixels shared across tiles and scales. Past it the cache starts a fresh epoch.
const MAX_DECODED_IMAGE_BYTES: usize = 64 << 20;

/// PdfCraft patch: pixel bytes held by a decoded mask.
fn decoded_mask_size(mask: &DecodedMask) -> usize {
    mask.luma.width as usize * mask.luma.height as usize
}

/// PdfCraft patch (32): whether `stream` names the JPX filter, as a name or in an array, under
/// `/Filter` or its inline abbreviation `/F`. Errs towards `true`: a name the decoder would
/// ignore still counts, which only keeps the target in the key as before.
fn uses_jpx(stream: &Stream<'_>) -> bool {
    let dict = stream.dict();
    let is_jpx = |name: &Name<'_>| name.deref() == JPX_DECODE;
    [F, FILTER].into_iter().any(|key| {
        dict.get::<Name<'_>>(key).is_some_and(|name| is_jpx(&name))
            || dict
                .get::<Array<'_>>(key)
                .is_some_and(|filters| filters.iter::<Name<'_>>().any(|name| is_jpx(&name)))
    })
}

/// PdfCraft patch: pixel bytes held by a decoded raster image and its mask.
fn decoded_raster_size(raster: &DecodedRaster) -> usize {
    let image = match &raster.image {
        ImageData::Rgb(d) => d.width as usize * d.height as usize * 3,
        ImageData::Luma(d) => d.width as usize * d.height as usize,
    };
    let alpha = raster
        .alpha
        .as_ref()
        .map(|a| a.width as usize * a.height as usize)
        .unwrap_or(0);
    image.saturating_add(alpha)
}

#[derive(Clone)]
pub(crate) struct DecodedMask {
    pub(crate) luma: LumaData,
}

#[derive(Clone)]
pub(crate) struct DecodedRaster {
    pub(crate) image: ImageData,
    pub(crate) alpha: Option<LumaData>,
}

struct DecodeContext<'a> {
    decoded: FilterResult<'a>,
    width: u32,
    height: u32,
    scale_factors: (f32, f32),
    color_space: ColorSpace,
    bits_per_component: u8,
    decode_arr: SmallVec<[(f32, f32); 4]>,
}

fn decode_context<'a>(
    obj: &ImageXObject<'a>,
    target_dimension: Option<(u32, u32)>,
) -> Option<DecodeContext<'a>> {
    // PdfCraft patch: images and masks over `MAX_IMAGE_PIXELS` are not decoded at all (the
    // renderer skips them anyway): a fuzzed `/W 4294967295` stencil mask allocated gigabytes here.
    if !image_size_ok(obj.width, obj.height) {
        (obj.warning_sink)(InterpreterWarning::ImageDecodeFailure);
        return None;
    }
    let dict = obj.stream.dict();
    let dict_bpc = dict
        .get::<u8>(BPC)
        .or_else(|| dict.get::<u8>(BITS_PER_COMPONENT));
    let color_space = obj.color_space.clone();
    let is_indexed = obj.color_space.as_ref().is_some_and(|cs| cs.is_indexed());

    let decode_params = ImageDecodeParams {
        is_indexed,
        bpc: dict_bpc,
        num_components: color_space.as_ref().map(|c| c.num_components()),
        target_dimension,
        width: obj.width,
        height: obj.height,
    };

    let decoded = obj
        .stream
        .decoded_image(&decode_params)
        .map_err(|_| (obj.warning_sink)(InterpreterWarning::ImageDecodeFailure))
        .ok()?;

    let (mut scale_x, mut scale_y) = (1.0, 1.0);

    let (width, height) = decoded
        .image_data
        .as_ref()
        .map(|d| {
            scale_x = obj.width as f32 / d.width as f32;
            scale_y = obj.height as f32 / d.height as f32;

            (d.width, d.height)
        })
        .unwrap_or((obj.width, obj.height));

    let color_space = color_space
        .or_else(|| {
            decoded
                .image_data
                .as_ref()
                .map(|i| i.color_space)
                .and_then(|c| {
                    c.and_then(|c| match c {
                        ImageColorSpace::Gray => Some(ColorSpace::device_gray()),
                        ImageColorSpace::Rgb => Some(ColorSpace::device_rgb()),
                        ImageColorSpace::Cmyk => Some(ColorSpace::device_cmyk()),
                        ImageColorSpace::Unknown(_) => None,
                    })
                })
        })
        .unwrap_or(ColorSpace::device_gray());

    let fallback_bpc = if obj.is_stencil_mask { 1 } else { 8 };

    let bits_per_component = decoded
        .image_data
        .as_ref()
        .map(|i| i.bits_per_component)
        .or(dict_bpc)
        .unwrap_or(fallback_bpc);

    let decode_arr = dict
        .get::<Array<'_>>(D)
        .or_else(|| dict.get::<Array<'_>>(DECODE))
        .map(|a| a.iter::<(f32, f32)>().collect::<SmallVec<_>>())
        .unwrap_or(color_space.default_decode_arr(bits_per_component as f32));

    Some(DecodeContext {
        decoded,
        width,
        height,
        scale_factors: (scale_x, scale_y),
        color_space,
        bits_per_component,
        decode_arr,
    })
}

fn decode_mask(
    obj: &ImageXObject<'_>,
    target_dimension: Option<(u32, u32)>,
) -> Option<DecodedMask> {
    let ctx = decode_context(obj, target_dimension)?;
    let mut height = ctx.height;

    let data = decode_mask_bytes(
        ctx.decoded.data,
        ctx.width,
        &mut height,
        &ctx.color_space,
        ctx.bits_per_component,
        &ctx.decode_arr,
        // Note: The semantics between "normal" soft masks (i.e. masks defined in
        // the graphics state or via `Mask`/`SMask` are inverted compared to
        // stencil masks (defined via `ImageMask`). The former match the semantics
        // of normal alpha images, where 0 stands for invisible and MAX stands for
        // fully opaque. For stencil masks, it's the other way around: 1 means the
        // paint is visible, while 0 means it's invisible.
        obj.is_stencil_mask,
    )?;

    Some(DecodedMask {
        luma: LumaData {
            data,
            width: ctx.width,
            height,
            interpolate: obj.interpolate,
            scale_factors: ctx.scale_factors,
        },
    })
}

fn decode_raster(
    obj: &ImageXObject<'_>,
    target_dimension: Option<(u32, u32)>,
) -> Option<DecodedRaster> {
    let mut ctx = decode_context(obj, target_dimension)?;
    let mut height = ctx.height;

    let is_default_decode = ctx.decode_arr
        == ctx
            .color_space
            .default_decode_arr(ctx.bits_per_component as f32);
    let is_inverted_default_decode = ctx.decode_arr
        == ctx
            .color_space
            .inverted_default_decode_arr(ctx.bits_per_component as f32);

    let image_data = if ctx.bits_per_component == 8
        && ctx.color_space.supports_u8()
        && obj.transfer_function.is_none()
        && (is_default_decode || is_inverted_default_decode)
    {
        // This is actually the most common case, where the PDF is embedded
        // in such a way where we don't need to decode. In this case,
        // we can prevent the round-trip from f32 back to u8 and just return
        // the raw decoded data, which will already be in
        // RGB8/gray-scale with values between 0 and 255.
        fix_image_length(
            ctx.decoded.data.to_mut(),
            ctx.width,
            &mut height,
            0,
            &ctx.color_space,
        )?;

        if is_inverted_default_decode {
            for b in ctx.decoded.data.to_mut() {
                *b = 255 - *b;
            }
        }

        if ctx.color_space.is_device_gray() {
            Some(ImageData::Luma(LumaData {
                data: core::mem::take(&mut ctx.decoded.data).into_owned(),
                width: ctx.width,
                height,
                interpolate: obj.interpolate,
                scale_factors: ctx.scale_factors,
            }))
        } else {
            let mut output_buf = vec![0; ctx.width as usize * height as usize * 3];
            ctx.color_space
                .convert_u8(&ctx.decoded.data, &mut output_buf)?;

            Some(ImageData::Rgb(RgbData {
                data: output_buf,
                width: ctx.width,
                height,
                interpolate: obj.interpolate,
                scale_factors: ctx.scale_factors,
            }))
        }
    } else {
        let components = get_components(
            &ctx.decoded.data,
            ctx.width,
            height,
            &ctx.color_space,
            ctx.bits_per_component,
        )?;

        // PdfCraft patch (28): an image with few possible pixel values converts each of them once.
        let mut rgb_data = if let Some(rgb) = rgb_via_lookup(
            &components,
            ctx.width,
            height,
            ctx.scale_factors,
            &ctx.color_space,
            ctx.bits_per_component,
            &ctx.decode_arr,
            obj.interpolate,
        ) {
            height = rgb.height;
            Some(rgb)
        } else {
            let mut f32_data = apply_decode_array(
                &components,
                &ctx.color_space,
                ctx.bits_per_component,
                &ctx.decode_arr,
            )?;

            fix_image_length(&mut f32_data, ctx.width, &mut height, 0.0, &ctx.color_space)?;

            get_rgb_data(
                &f32_data,
                ctx.width,
                height,
                ctx.scale_factors,
                &ctx.color_space,
                obj.interpolate,
            )
        };

        if let Some(transfer_function) = &obj.transfer_function
            && let Some(rgb_data) = &mut rgb_data
        {
            let apply_single = |data: u8, function: &Function| {
                function
                    .eval(smallvec![data as f32 / 255.0])
                    .and_then(|v| v.first().copied())
                    .map(|v| (v * 255.0 + 0.5) as u8)
                    .unwrap_or(data)
            };

            match transfer_function {
                ActiveTransferFunction::Single(s) => {
                    for data in &mut rgb_data.data {
                        *data = apply_single(*data, s);
                    }
                }
                ActiveTransferFunction::Four(f) => {
                    for data in rgb_data.data.chunks_exact_mut(3) {
                        data[0] = apply_single(data[0], &f[0]);
                        data[1] = apply_single(data[1], &f[1]);
                        data[2] = apply_single(data[2], &f[2]);
                    }
                }
            }
        }

        rgb_data.map(ImageData::Rgb)
    };

    let mut image = image_data?;

    let alpha = if let Some((alpha, matte_rgb)) =
        resolve_matte(obj, &ctx.color_space, target_dimension)
        && alpha.width == ctx.width
        && alpha.height == height
    {
        unpremultiply(&mut image, &alpha.data, &matte_rgb);

        Some(alpha)
    } else {
        // Use flatten here, so in case the alpha channel is invalid we can still
        // return the main image (see PDFJS-19611).
        resolve_alpha(
            obj,
            &mut ctx.decoded,
            Some(&image),
            &ctx.color_space,
            ctx.bits_per_component,
            ctx.width,
            &mut height,
            ctx.scale_factors,
            target_dimension,
        )
        .flatten()
    };

    Some(DecodedRaster { image, alpha })
}

fn decode_mask_bytes(
    mut decoded_data: Cow<'_, [u8]>,
    width: u32,
    height: &mut u32,
    color_space: &ColorSpace,
    bits_per_component: u8,
    decode_arr: &[(f32, f32)],
    invert: bool,
) -> Option<Vec<u8>> {
    let default_decode = color_space.default_decode_arr(bits_per_component as f32);
    let inverted_default = color_space.inverted_default_decode_arr(bits_per_component as f32);
    let fast_path = bits_per_component == 8
        && (decode_arr == default_decode.as_slice() || decode_arr == inverted_default.as_slice());

    let mut data = if fast_path {
        let should_invert = invert ^ (decode_arr == inverted_default.as_slice());
        if should_invert {
            for b in decoded_data.to_mut() {
                *b = 255 - *b;
            }
        }

        decoded_data.into_owned()
    } else {
        let components = get_components(
            &decoded_data,
            width,
            *height,
            color_space,
            bits_per_component,
        )?;

        let f32_data =
            apply_decode_array(&components, color_space, bits_per_component, decode_arr)?;

        if invert {
            f32_data
                .iter()
                .map(|alpha| ((1.0 - *alpha) * 255.0 + 0.5) as u8)
                .collect()
        } else {
            f32_data
                .iter()
                .map(|alpha| (*alpha * 255.0 + 0.5) as u8)
                .collect()
        }
    };

    fix_image_length(&mut data, width, height, 0, color_space)?;

    Some(data)
}

fn resolve_alpha(
    obj: &ImageXObject<'_>,
    decoded: &mut FilterResult<'_>,
    image_data: Option<&ImageData>,
    color_space: &ColorSpace,
    bits_per_component: u8,
    width: u32,
    height: &mut u32,
    scale_factors: (f32, f32),
    target_dimension: Option<(u32, u32)>,
) -> Option<Option<LumaData>> {
    let dict = obj.stream.dict();

    let alpha = if let Some(1) = dict.get::<u8>(SMASK_IN_DATA) {
        let smask_data = decoded.image_data.as_mut().and_then(|i| i.alpha.take());

        if let Some(mut data) = smask_data {
            fix_image_length(&mut data, width, height, 0, &ColorSpace::device_gray())?;

            Some(LumaData {
                data,
                width,
                height: *height,
                interpolate: obj.interpolate,
                scale_factors,
            })
        } else {
            None
        }
        // Note: `SMASK` field takes precedence over `MASK`, so order matters here.
    } else if let Some(s_mask) = dict
        .get::<Stream<'_>>(SMASK)
        .or_else(|| dict.get::<Stream<'_>>(MASK))
    {
        let obj = ImageXObject::new(&s_mask, |_| None, &obj.warning_sink, &obj.cache, obj.decoded_cache.as_ref(), true, None, None)?;

        decode_mask(&obj, target_dimension).map(|decoded| decoded.luma)
    } else if let Some(color_key_mask) = dict.get::<SmallVec<[u16; 4]>>(MASK) {
        let mut mask_data = vec![];

        // TODO: Make this less ugly.
        let raw_data = match image_data {
            Some(ImageData::Luma(d)) => &d.data,
            _ => decoded.data.as_ref(),
        };

        let components = get_components(raw_data, width, *height, color_space, bits_per_component)?;

        for pixel in components.chunks_exact(color_space.num_components() as usize) {
            let mut mask_val = 0;

            for (component, min_max) in pixel.iter().zip(color_key_mask.chunks_exact(2)) {
                if *component > min_max[1] || *component < min_max[0] {
                    mask_val = 255;
                }
            }

            mask_data.push(mask_val);
        }

        fix_image_length(&mut mask_data, width, height, 0, &ColorSpace::device_gray())?;

        Some(LumaData {
            data: mask_data,
            width,
            height: *height,
            interpolate: obj.interpolate,
            scale_factors,
        })
    } else {
        None
    };

    Some(alpha)
}

fn resolve_matte(
    obj: &ImageXObject<'_>,
    color_space: &ColorSpace,
    target_dimension: Option<(u32, u32)>,
) -> Option<(LumaData, [u8; 3])> {
    let dict = obj.stream.dict();
    let s_mask = dict.get::<Stream<'_>>(SMASK)?;
    let matte = s_mask.dict().get::<ColorComponents>(MATTE)?;

    if matte.len() != color_space.num_components() as usize {
        return None;
    }

    // In theory, matte needs to be applied in the image's original color space,
    // but we always do it in RGB for now.
    let mut matte_rgb = [0_u8; 3];
    color_space.convert_f32(&matte, &mut matte_rgb, false);

    let mask_obj = ImageXObject::new(&s_mask, |_| None, &obj.warning_sink, &obj.cache, obj.decoded_cache.as_ref(), true, None, None)?;
    let alpha = decode_mask(&mask_obj, target_dimension)?.luma;

    Some((alpha, matte_rgb))
}

fn unpremultiply(image: &mut ImageData, alpha: &[u8], matte_rgb: &[u8]) {
    match image {
        ImageData::Rgb(rgb) => {
            for (pixel, &a) in rgb.data.chunks_exact_mut(3).zip(alpha.iter()) {
                if a == 0 {
                    continue;
                }
                let inv_alpha = 255.0 / a as f32;
                for (c, &m) in pixel.iter_mut().zip(matte_rgb.iter()) {
                    let m = m as f32;
                    *c = (m + (*c as f32 - m) * inv_alpha) as u8;
                }
            }
        }
        ImageData::Luma(luma) => {
            let m = matte_rgb[0] as f32;
            for (c, &a) in luma.data.iter_mut().zip(alpha.iter()) {
                if a == 0 {
                    continue;
                }
                let inv_alpha = 255.0 / a as f32;
                *c = (m + (*c as f32 - m) * inv_alpha) as u8;
            }
        }
    }
}

fn get_rgb_data(
    decoded: &[f32],
    width: u32,
    height: u32,
    scale_factors: (f32, f32),
    cs: &ColorSpace,
    interpolate: bool,
) -> Option<RgbData> {
    // To prevent a panic when calling the `chunks` method.
    if cs.num_components() == 0 {
        return None;
    }

    let mut output = vec![0; width as usize * height as usize * 3];
    cs.convert_f32(decoded, &mut output, false);

    Some(RgbData {
        data: output,
        width,
        height,
        interpolate,
        scale_factors,
    })
}

impl CacheKey for ImageXObject<'_> {
    fn cache_key(&self) -> u128 {
        self.stream.cache_key()
    }
}

#[must_use]
fn fix_image_length<T: Copy>(
    image: &mut Vec<T>,
    width: u32,
    height: &mut u32,
    filler: T,
    cs: &ColorSpace,
) -> Option<()> {
    let row_len = width as usize * cs.num_components() as usize;

    if (row_len * *height as usize) <= image.len() {
        // Too much data (or just the right amount), truncate it.
        image.truncate(row_len * *height as usize);
    } else {
        // Too little data, adapt the height and pad.
        *height = image.len().div_ceil(row_len) as u32;

        if !image.len().is_multiple_of(row_len) {
            image.extend(iter::repeat_n(filler, row_len - (image.len() % row_len)));
        }
    }

    if width == 0 || *height == 0 {
        None
    } else {
        Some(())
    }
}

fn get_components(
    data: &[u8],
    width: u32,
    height: u32,
    color_space: &ColorSpace,
    bits_per_component: u8,
) -> Option<Vec<u16>> {
    let result = match bits_per_component {
        1..8 | 9..16 => {
            let mut buf = vec![];
            let bpc = bits_per_component;
            let mut reader = BitReader::new(data);

            for _ in 0..height {
                for _ in 0..width {
                    for _ in 0..color_space.num_components() {
                        // See `stream_ccit_not_enough_data`, some images seemingly don't have
                        // enough data, so we just pad with zeroes in this case.
                        let next = reader.read(bpc).unwrap_or(0) as u16;

                        buf.push(next);
                    }
                }

                reader.align();
            }

            buf
        }
        8 => data.iter().map(|v| *v as u16).collect(),
        16 => data
            .chunks(2)
            .map(|v| u16::from_be_bytes([v[0], v[1]]))
            .collect(),
        _ => {
            warn!("unsupported bits per component: {bits_per_component}");
            return None;
        }
    };

    Some(result)
}

fn apply_decode_array(
    components: &[u16],
    color_space: &ColorSpace,
    bits_per_component: u8,
    decode: &[(f32, f32)],
) -> Option<Vec<f32>> {
    let mut decoded_arr = vec![];

    for pixel in components.chunks(color_space.num_components() as usize) {
        for (component, range) in pixel.iter().zip(decode) {
            decoded_arr.push(decode_sample(*component, bits_per_component, *range));
        }
    }

    Some(decoded_arr)
}

/// One sample mapped through its `/Decode` range (ISO 32000-2 §8.9.5.2). PdfCraft patch (28):
/// shared by [`apply_decode_array`] and [`rgb_via_lookup`], so both decode alike.
fn decode_sample(sample: u16, bits_per_component: u8, (d_min, d_max): (f32, f32)) -> f32 {
    interpolate(
        sample as f32,
        0.0,
        2.0_f32.powi(bits_per_component as i32) - 1.0,
        d_min,
        d_max,
    )
}

/// PdfCraft patch (28): the most bits a pixel's samples may hold together for
/// [`rgb_via_lookup`] (a table of 65,536 colours).
const MAX_LOOKUP_BITS: usize = 16;

/// PdfCraft patch (28): the RGB of an image whose pixels hold at most [`MAX_LOOKUP_BITS`] bits
/// of samples, converted once per possible pixel value rather than once per pixel, at the
/// height `fix_image_length` gives. `None` when the table would not pay off (the image has no
/// more pixels than it has entries) or does not apply; the caller then converts every pixel.
///
/// The colour of a pixel depends only on its own samples, so the result is the per-pixel one,
/// except where a tint transform gives the wrong number of components. The per-pixel batch then
/// shifts the colours after that pixel. The table converts each entry on its own
/// ([`convert_each`]), so a wrong-sized entry changes only itself, and a value the image never
/// uses cannot change the colour of one it does.
/// A Separation or DeviceN tint transform is a PDF function, often a PostScript calculator
/// program: run once per pixel, an 8-bit spot-colour background of 3304 × 4105 took four
/// seconds and 430 MB to draw; looked up from 256 entries it takes milliseconds.
#[allow(clippy::too_many_arguments)]
fn rgb_via_lookup(
    components: &[u16],
    width: u32,
    height: u32,
    scale_factors: (f32, f32),
    cs: &ColorSpace,
    bits_per_component: u8,
    decode: &[(f32, f32)],
    interpolate: bool,
) -> Option<RgbData> {
    let n = cs.num_components() as usize;
    let bpc = usize::from(bits_per_component);
    let bits = bpc.checked_mul(n)?;
    // `apply_decode_array` decodes only the samples `/Decode` has a range for: leave a short
    // array to it, and every sample size it doesn't read.
    if n == 0 || bits == 0 || bits > MAX_LOOKUP_BITS || bpc > 16 || decode.len() < n {
        return None;
    }
    // The length rules of `fix_image_length`: extra samples are dropped; missing ones shorten
    // the image to the rows begun, the last row padded with decoded zeros.
    let row_len = usize::try_from(width).ok()?.checked_mul(n)?;
    let wanted = row_len.checked_mul(usize::try_from(height).ok()?)?;
    let (height, used) = if wanted <= components.len() {
        (height, wanted)
    } else {
        (u32::try_from(components.len().div_ceil(row_len)).ok()?, components.len())
    };
    let pixels = usize::try_from(width).ok()?.checked_mul(usize::try_from(height).ok()?)?;
    let total = pixels.checked_mul(n)?;
    let entries = 1usize << bits;
    if pixels <= entries {
        return None;
    }

    // Every possible pixel value, its samples in order, decoded as `apply_decode_array` would.
    let mask = (1usize << bpc) - 1;
    let mut table_input = Vec::with_capacity(entries * n);
    for code in 0..entries {
        for (k, range) in decode.iter().take(n).enumerate() {
            let sample = ((code >> (bpc * (n - 1 - k))) & mask) as u16;
            table_input.push(decode_sample(sample, bits_per_component, *range));
        }
    }
    let mut table = vec![0; entries * 3];
    convert_each(cs, &table_input, n, &mut table);

    let mut data = vec![0; pixels.checked_mul(3)?];
    let whole = used / n;
    let samples = components.get(..whole * n)?;
    for (pixel, rgb) in samples.chunks_exact(n).zip(data.chunks_exact_mut(3)) {
        let code = pixel.iter().fold(0usize, |code, s| (code << bpc) | (usize::from(*s) & mask));
        if let Some(colour) = table.get(code * 3..code * 3 + 3) {
            rgb.copy_from_slice(colour);
        }
    }
    // The pixels past the samples present (at most a row): converted as before, padding and all.
    if whole < pixels {
        let tail: Vec<f32> = (whole * n..total)
            .map(|i| match (components.get(i), decode.get(i % n)) {
                (Some(s), Some(range)) if i < used => decode_sample(*s, bits_per_component, *range),
                _ => 0.0,
            })
            .collect();
        let _ = cs.convert_f32(&tail, data.get_mut(whole * 3..)?, false);
    }

    Some(RgbData {
        data,
        width,
        height,
        interpolate,
        scale_factors,
    })
}

/// PdfCraft patch (28): `input`, `n` decoded samples a pixel, converted into `output` (RGB) one
/// pixel at a time. A Separation or DeviceN tint transform may give more or fewer components
/// than its alternate space takes (a Type 4 program's count is only warned about): converted as
/// one batch, such a result shifts every pixel after it; converted alone, it affects only its
/// own. A failed conversion leaves black, as in `get_rgb_data`.
fn convert_each(cs: &ColorSpace, input: &[f32], n: usize, output: &mut [u8]) {
    if n == 0 {
        return;
    }
    for (pixel, rgb) in input.chunks_exact(n).zip(output.chunks_exact_mut(3)) {
        let _ = cs.convert_f32(pixel, rgb, false);
    }
}

/// PdfCraft patch: whether an image or mask of `width` × `height` may be decoded (at most
/// 2^28 pixels, the renderer's `MAX_IMAGE_PIXELS`).
pub fn image_size_ok(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 1 << 28
}
