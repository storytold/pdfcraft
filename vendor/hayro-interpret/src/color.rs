//! PDF colors and color spaces.

use crate::cache::{Cache, CacheKey};
use crate::function::Function;
use crate::util::hash128;
use hayro_syntax::object;
use hayro_syntax::object::Array;
use hayro_syntax::object::Dict;
use hayro_syntax::object::Name;
use hayro_syntax::object::Object;
use hayro_syntax::object::Stream;
use hayro_syntax::object::dict::keys::*;
use hayro_syntax::page::Resources;
use moxcms::{
    ColorProfile, DataColorSpace, Layout, Transform8BitExecutor, TransformF32Executor,
    TransformOptions,
};
use smallvec::{SmallVec, ToSmallVec, smallvec};
use std::borrow::Cow;
use std::cell::OnceCell;
use std::fmt::{Debug, Formatter};
use std::ops::Deref;
use std::sync::{Arc, LazyLock, OnceLock};

/// A storage for the components of colors.
pub type ColorComponents = SmallVec<[f32; 4]>;

std::thread_local! {
    // PdfCraft patch (34): see `tint_transform_runs`.
    static TINT_TRANSFORM_RUNS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
    // PdfCraft patch (55): see `note_color_fallback`.
    static COLOR_FALLBACK: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// PdfCraft patch (55): record that a colour could not be computed and was painted in a
/// fallback colour, reported as `InterpreterWarning::ColorFallback` when the content stream
/// being interpreted ends. Per thread, like the page's other budgets: one page is interpreted
/// on one thread.
pub(crate) fn note_color_fallback() {
    COLOR_FALLBACK.with(|fell_back| fell_back.set(true));
}

/// PdfCraft patch (55): whether a colour fell back since the last call (or since the page
/// started), clearing it, so each fallback is reported once.
pub(crate) fn take_color_fallback() -> bool {
    COLOR_FALLBACK.with(|fell_back| fell_back.replace(false))
}

/// PdfCraft patch (34): how many colours this thread has run through a Separation or DeviceN
/// tint transform, for diagnostics and tests. An image converted through patch (28)'s lookup
/// table runs it once per possible sample value, not once per pixel. Per thread, so a caller
/// counting its own render is not disturbed by renders on other threads.
pub fn tint_transform_runs() -> u64 {
    TINT_TRANSFORM_RUNS.with(core::cell::Cell::get)
}

/// PdfCraft patch (34): count `colours` runs of a tint transform (called once per batch).
fn count_tint_transform_runs(colours: usize) {
    let colours = u64::try_from(colours).unwrap_or(u64::MAX);
    TINT_TRANSFORM_RUNS.with(|runs| runs.set(runs.get().saturating_add(colours)));
}

/// An RGB color with an alpha channel.
#[derive(Debug, Copy, Clone)]
pub struct AlphaColor {
    components: [f32; 4],
}

impl AlphaColor {
    /// A black color.
    pub const BLACK: Self = Self::new([0., 0., 0., 1.]);

    /// A transparent color.
    pub const TRANSPARENT: Self = Self::new([0., 0., 0., 0.]);

    /// A white color.
    pub const WHITE: Self = Self::new([1., 1., 1., 1.]);

    /// Create a new color from the given components.
    pub const fn new(components: [f32; 4]) -> Self {
        Self { components }
    }

    /// Create a new color from RGB8 values.
    pub const fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        let components = [u8_to_f32(r), u8_to_f32(g), u8_to_f32(b), 1.];
        Self::new(components)
    }

    /// Return the color as premulitplied RGBF32.
    pub fn premultiplied(&self) -> [f32; 4] {
        [
            self.components[0] * self.components[3],
            self.components[1] * self.components[3],
            self.components[2] * self.components[3],
            self.components[3],
        ]
    }

    /// Create a new color from RGBA8 values.
    pub const fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        let components = [u8_to_f32(r), u8_to_f32(g), u8_to_f32(b), u8_to_f32(a)];
        Self::new(components)
    }

    /// Return the color as RGBA8.
    pub fn to_rgba8(&self) -> [u8; 4] {
        [
            (self.components[0] * 255.0 + 0.5) as u8,
            (self.components[1] * 255.0 + 0.5) as u8,
            (self.components[2] * 255.0 + 0.5) as u8,
            (self.components[3] * 255.0 + 0.5) as u8,
        ]
    }

    /// Return the components of the color as RGBF32.
    pub fn components(&self) -> [f32; 4] {
        self.components
    }
}

const fn u8_to_f32(x: u8) -> f32 {
    x as f32 * (1.0 / 255.0)
}

#[derive(Debug, Clone)]
pub(crate) enum ColorSpaceType {
    DeviceCmyk,
    DeviceGray,
    DeviceRgb,
    Pattern(ColorSpace),
    Indexed(Indexed),
    ICCBased(ICCProfile),
    CalGray(CalGray),
    CalRgb(CalRgb),
    Lab(Lab),
    Separation(Separation),
    DeviceN(DeviceN),
}

/// PdfCraft patch (48): what the colour space read from an `/ICCBased` profile stream is cached
/// by. hayro keyed it by the stream dictionary's bytes alone, so two different profiles with the
/// same `/N` and `/Length` shared whichever was read first, and colours in the second were
/// drawn through the first.
#[derive(Hash)]
enum IccProfileKey<'a> {
    /// The stream object a space names, looked up without reading the stream: an object holds
    /// one profile for as long as the document, and so the cache, lives.
    Object(object::ObjectIdentifier),
    /// What the space is built from: the stream dictionary's key (its bytes, the old key:
    /// `/N`, `/Alternate`, the filters) and the stream's own bytes, decrypted but not decoded,
    /// which decode to the profile. Copies of a profile share it, and a lookup never decodes:
    /// decoding waits for a miss.
    Content { dict: u128, data: &'a [u8] },
}

/// PdfCraft patch (48): the object of the `/ICCBased` profile `stream` that `array` names, or
/// `None` for a stream written directly in the array (its dictionary lies within the array's
/// bytes). Such a stream has no object of its own: its dictionary reports the object that
/// holds the array, which every other stream written there reports too.
fn icc_stream_object(array: &Array<'_>, stream: &Stream<'_>) -> Option<object::ObjectIdentifier> {
    let dict = stream.dict();
    if array.data().as_ptr_range().contains(&dict.data().as_ptr()) {
        return None;
    }
    dict.obj_id()
}

impl ColorSpaceType {
    fn new(object: Object<'_>, cache: &Cache) -> Option<Self> {
        Self::new_inner(object, cache)
    }

    fn new_inner(object: Object<'_>, cache: &Cache) -> Option<Self> {
        if let Object::Name(name) = object {
            return Self::new_from_name(&name);
        } else if let Object::Array(color_array) = object {
            let mut iter = color_array.flex_iter();
            let name = iter.next::<Name<'_>>()?;

            match name.deref() {
                ICC_BASED => {
                    let icc_stream = iter.next::<Stream<'_>>()?;
                    let dict = icc_stream.dict();
                    let num_components = dict.get::<usize>(N)?;

                    // PdfCraft patch (48): cached by what the profile is read from (see
                    // `IccProfileKey`), not by the stream dictionary's bytes alone.
                    let read = || {
                        let data = icc_stream.raw_data();
                        let content = IccProfileKey::Content {
                            dict: icc_stream.cache_key(),
                            data: &data,
                        };
                        cache.get_or_insert_with(hash128(&content), || {
                            cache.count_icc_profile_read();
                            if let Some(decoded) = icc_stream.decoded().ok().as_ref() {
                                ICCProfile::new(decoded, num_components)
                                    .map(|icc| {
                                        // TODO: For SVG and PNG we can assume that the output color space is
                                        // sRGB. If we ever implement PDF-to-PDF, we probably want to
                                        // let the user pass the native color type and don't make this optimization
                                        // if it's not sRGB.
                                        if icc.is_srgb() {
                                            Self::DeviceRgb
                                        } else {
                                            Self::ICCBased(icc)
                                        }
                                    })
                                    .or_else(|| {
                                        dict.get::<Object<'_>>(ALTERNATE)
                                            .and_then(|o| Self::new(o, cache))
                                    })
                                    .or_else(|| match dict.get::<u8>(N) {
                                        Some(1) => Some(Self::DeviceGray),
                                        Some(3) => Some(Self::DeviceRgb),
                                        Some(4) => Some(Self::DeviceCmyk),
                                        _ => None,
                                    })
                            } else {
                                None
                            }
                        })
                    };
                    return match icc_stream_object(&color_array, &icc_stream) {
                        Some(object) => {
                            cache.get_or_insert_with(hash128(&IccProfileKey::Object(object)), read)
                        }
                        None => read(),
                    };
                }
                CALCMYK => return Some(Self::DeviceCmyk),
                CALGRAY => {
                    let cal_dict = iter.next::<Dict<'_>>()?;
                    return Some(Self::CalGray(CalGray::new(&cal_dict)?));
                }
                CALRGB => {
                    let cal_dict = iter.next::<Dict<'_>>()?;
                    return Some(Self::CalRgb(CalRgb::new(&cal_dict)?));
                }
                DEVICE_RGB | RGB => return Some(Self::DeviceRgb),
                DEVICE_GRAY | G => return Some(Self::DeviceGray),
                DEVICE_CMYK | CMYK => return Some(Self::DeviceCmyk),
                LAB => {
                    let lab_dict = iter.next::<Dict<'_>>()?;
                    return Some(Self::Lab(Lab::new(&lab_dict)?));
                }
                INDEXED | I => {
                    return Some(Self::Indexed(Indexed::new(&color_array, cache)?));
                }
                SEPARATION => {
                    return Some(Self::Separation(Separation::new(&color_array, cache)?));
                }
                DEVICE_N => {
                    return Some(Self::DeviceN(DeviceN::new(&color_array, cache)?));
                }
                PATTERN => {
                    let _ = iter.next::<Name<'_>>();
                    let cs = iter
                        .next::<Object<'_>>()
                        .and_then(|o| ColorSpace::new(o, cache))
                        .unwrap_or(ColorSpace::device_rgb());
                    return Some(Self::Pattern(cs));
                }
                _ => {
                    warn!("unsupported color space: {}", name.as_str());
                    return None;
                }
            }
        }

        None
    }

    fn new_from_name(name: &Name<'_>) -> Option<Self> {
        match name.deref() {
            DEVICE_RGB | RGB => Some(Self::DeviceRgb),
            DEVICE_GRAY | G => Some(Self::DeviceGray),
            DEVICE_CMYK | CMYK => Some(Self::DeviceCmyk),
            CALCMYK => Some(Self::DeviceCmyk),
            PATTERN => Some(Self::Pattern(ColorSpace::device_rgb())),
            _ => None,
        }
    }
}

/// A PDF color space.
#[derive(Debug, Clone)]
pub struct ColorSpace(Arc<ColorSpaceType>);

impl ColorSpace {
    /// Create a new color space from the given object.
    pub(crate) fn new(object: Object<'_>, cache: &Cache) -> Option<Self> {
        Some(Self(Arc::new(ColorSpaceType::new(object, cache)?)))
    }

    /// Create a new color space from the name.
    pub(crate) fn new_from_name(name: &Name<'_>) -> Option<Self> {
        ColorSpaceType::new_from_name(name).map(|c| Self(Arc::new(c)))
    }

    /// Return the device gray color space.
    pub(crate) fn device_gray() -> Self {
        Self(Arc::new(ColorSpaceType::DeviceGray))
    }

    /// Return the device RGB color space.
    pub(crate) fn device_rgb() -> Self {
        Self(Arc::new(ColorSpaceType::DeviceRgb))
    }

    /// Return the device CMYK color space.
    pub(crate) fn device_cmyk() -> Self {
        Self(Arc::new(ColorSpaceType::DeviceCmyk))
    }

    /// Return the pattern color space.
    pub(crate) fn pattern() -> Self {
        Self(Arc::new(ColorSpaceType::Pattern(Self::device_gray())))
    }

    pub(crate) fn pattern_cs(&self) -> Option<Self> {
        match self.0.as_ref() {
            ColorSpaceType::Pattern(cs) => Some(cs.clone()),
            _ => None,
        }
    }

    /// Return `true` if the current color space is the pattern color space.
    pub(crate) fn is_pattern(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::Pattern(_))
    }

    /// Return `true` if the current color space is an indexed color space.
    pub(crate) fn is_indexed(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::Indexed(_))
    }

    /// Get the default decode array for the color space.
    pub(crate) fn default_decode_arr(&self, n: f32) -> SmallVec<[(f32, f32); 4]> {
        match self.0.as_ref() {
            ColorSpaceType::DeviceCmyk => smallvec![(0.0, 1.0), (0.0, 1.0), (0.0, 1.0), (0.0, 1.0)],
            ColorSpaceType::DeviceGray => smallvec![(0.0, 1.0)],
            ColorSpaceType::DeviceRgb => smallvec![(0.0, 1.0), (0.0, 1.0), (0.0, 1.0)],
            ColorSpaceType::ICCBased(i) => smallvec![(0.0, 1.0); i.0.number_components],
            ColorSpaceType::CalGray(_) => smallvec![(0.0, 1.0)],
            ColorSpaceType::CalRgb(_) => smallvec![(0.0, 1.0), (0.0, 1.0), (0.0, 1.0)],
            ColorSpaceType::Lab(l) => smallvec![
                (0.0, 100.0),
                (l.range[0], l.range[1]),
                (l.range[2], l.range[3]),
            ],
            ColorSpaceType::Indexed(_) => smallvec![(0.0, 2.0_f32.powf(n) - 1.0)],
            ColorSpaceType::Separation(_) => smallvec![(0.0, 1.0)],
            ColorSpaceType::DeviceN(d) => smallvec![(0.0, 1.0); d.num_components as usize],
            // Not a valid image color space.
            ColorSpaceType::Pattern(_) => smallvec![(0.0, 1.0)],
        }
    }

    pub(crate) fn inverted_default_decode_arr(&self, n: f32) -> SmallVec<[(f32, f32); 4]> {
        self.default_decode_arr(n)
            .iter()
            .map(|(min, max)| (*max, *min))
            .collect()
    }

    /// Get the initial color of the color space.
    pub(crate) fn initial_color(&self) -> ColorComponents {
        match self.0.as_ref() {
            ColorSpaceType::DeviceCmyk => smallvec![0.0, 0.0, 0.0, 1.0],
            ColorSpaceType::DeviceGray => smallvec![0.0],
            ColorSpaceType::DeviceRgb => smallvec![0.0, 0.0, 0.0],
            ColorSpaceType::ICCBased(icc) => match icc.0.number_components {
                1 => smallvec![0.0],
                3 => smallvec![0.0, 0.0, 0.0],
                4 => smallvec![0.0, 0.0, 0.0, 1.0],
                _ => unreachable!(),
            },
            ColorSpaceType::CalGray(_) => smallvec![0.0],
            ColorSpaceType::CalRgb(_) => smallvec![0.0, 0.0, 0.0],
            ColorSpaceType::Lab(_) => smallvec![0.0, 0.0, 0.0],
            ColorSpaceType::Indexed(_) => smallvec![0.0],
            ColorSpaceType::Separation(_) => smallvec![1.0],
            ColorSpaceType::Pattern(c) => c.initial_color(),
            ColorSpaceType::DeviceN(d) => smallvec![1.0; d.num_components as usize],
        }
    }

    pub(crate) fn is_device_gray(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::DeviceGray)
    }

    // PdfCraft patch (63).
    /// Whether this is `/DeviceCMYK` itself, not an ICC-based or default colour space that
    /// stands in for it.
    pub(crate) fn is_device_cmyk(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::DeviceCmyk)
    }

    /// Get the number of components of the color space.
    pub(crate) fn num_components(&self) -> u8 {
        match self.0.as_ref() {
            ColorSpaceType::DeviceCmyk => 4,
            ColorSpaceType::DeviceGray => 1,
            ColorSpaceType::DeviceRgb => 3,
            ColorSpaceType::ICCBased(icc) => icc.0.number_components as u8,
            ColorSpaceType::CalGray(_) => 1,
            ColorSpaceType::CalRgb(_) => 3,
            ColorSpaceType::Lab(_) => 3,
            ColorSpaceType::Indexed(_) => 1,
            ColorSpaceType::Separation(_) => 1,
            ColorSpaceType::Pattern(p) => p.num_components(),
            ColorSpaceType::DeviceN(d) => d.num_components,
        }
    }

    /// Turn the given component values and opacity into an RGBA color.
    pub fn to_rgba(&self, c: &[f32], opacity: f32, manual_scale: bool) -> AlphaColor {
        self.to_alpha_color(c, opacity, manual_scale)
            .unwrap_or(AlphaColor::BLACK)
    }

    /// PdfCraft patch (38): whether this space can be the default colour space of `family`
    /// (ISO 32000-2 §8.6.5.6): it has as many components as the device space and is not a
    /// Lab, Indexed or Pattern space. Nor is a device space: a default that names one is
    /// ignored, so substitution never recurses.
    fn can_stand_in_for(&self, family: DeviceFamily) -> bool {
        !matches!(
            self.0.as_ref(),
            ColorSpaceType::DeviceGray
                | ColorSpaceType::DeviceRgb
                | ColorSpaceType::DeviceCmyk
                | ColorSpaceType::Lab(_)
                | ColorSpaceType::Indexed(_)
                | ColorSpaceType::Pattern(_)
        ) && self.num_components() == family.num_components()
    }
}

/// PdfCraft patch (38): a device colour space (ISO 32000-2 §8.6.4), which the resources'
/// default colour space for it replaces wherever content selects it (§8.6.5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceFamily {
    Gray,
    Rgb,
    Cmyk,
}

impl DeviceFamily {
    /// The device space a colour space operand names: `/DeviceGray`, `/DeviceRGB` or
    /// `/DeviceCMYK` (or an abbreviation read as one), alone or at the head of an array. `None`
    /// for any other space, including the ICC-based and calibrated ones drawn as device colour
    /// here (an sRGB profile, `/CalCMYK`): they are not device spaces.
    pub(crate) fn of_object(object: &Object<'_>) -> Option<Self> {
        match object {
            Object::Name(name) => Self::of_name(name),
            Object::Array(array) => array
                .flex_iter()
                .next::<Name<'_>>()
                .and_then(|name| Self::of_name(&name)),
            _ => None,
        }
    }

    /// The device space `name` names, as [`DeviceFamily::of_object`].
    pub(crate) fn of_name(name: &Name<'_>) -> Option<Self> {
        match name.deref() {
            DEVICE_GRAY | G => Some(Self::Gray),
            DEVICE_RGB | RGB => Some(Self::Rgb),
            DEVICE_CMYK | CMYK => Some(Self::Cmyk),
            _ => None,
        }
    }

    /// The device space itself.
    fn space(self) -> ColorSpace {
        match self {
            Self::Gray => ColorSpace::device_gray(),
            Self::Rgb => ColorSpace::device_rgb(),
            Self::Cmyk => ColorSpace::device_cmyk(),
        }
    }

    fn num_components(self) -> u8 {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }

    /// The `/ColorSpace` resource that holds the default for this space.
    fn default_key(self) -> &'static [u8] {
        match self {
            Self::Gray => DEFAULT_GRAY,
            Self::Rgb => DEFAULT_RGB,
            Self::Cmyk => DEFAULT_CMYK,
        }
    }
}

/// PdfCraft patch (38): the default colour spaces (`/DefaultGray`, `/DefaultRGB` and
/// `/DefaultCMYK`, ISO 32000-2 §8.6.5.6) of the resources content is drawn with, each read the
/// first time the content selects its device space. Like any named resource, a default comes
/// from those resources' `/ColorSpace` or from the resources they inherit: a form XObject
/// without one of its own draws in its page's.
pub(crate) struct DefaultColorSpaces<'r, 'a> {
    resources: &'r Resources<'a>,
    cache: Cache,
    gray: OnceCell<Option<(ColorSpace, u128)>>,
    rgb: OnceCell<Option<(ColorSpace, u128)>>,
    cmyk: OnceCell<Option<(ColorSpace, u128)>>,
}

impl<'r, 'a> DefaultColorSpaces<'r, 'a> {
    pub(crate) fn new(resources: &'r Resources<'a>, cache: &Cache) -> Self {
        Self {
            resources,
            cache: cache.clone(),
            gray: OnceCell::new(),
            rgb: OnceCell::new(),
            cmyk: OnceCell::new(),
        }
    }

    /// The default for `family`, with the cache key of the object it was read from. `None`
    /// when the resources have none, or theirs is not a colour space that can stand in for
    /// the device space: device colour is then drawn as it is.
    pub(crate) fn get(&self, family: DeviceFamily) -> Option<(ColorSpace, u128)> {
        let found = match family {
            DeviceFamily::Gray => &self.gray,
            DeviceFamily::Rgb => &self.rgb,
            DeviceFamily::Cmyk => &self.cmyk,
        };
        found
            .get_or_init(|| {
                let object = self
                    .resources
                    .get_color_space(&Name::new_unescaped(family.default_key()))?;
                let key = object.cache_key();
                let space = self
                    .cache
                    .get_or_insert_with(key, || ColorSpace::new(object.clone(), &self.cache))?;
                space.can_stand_in_for(family).then_some((space, key))
            })
            .clone()
    }

    /// The colour space content that selects `family` draws in: its default, else the device
    /// space itself.
    pub(crate) fn device(&self, family: DeviceFamily) -> ColorSpace {
        self.get(family)
            .map_or_else(|| family.space(), |(space, _)| space)
    }
}

impl ToRgb for ColorSpace {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], manual_scale: bool) -> Option<()> {
        match self.0.as_ref() {
            ColorSpaceType::DeviceCmyk => {
                if input.len() == 4 {
                    let converted = [
                        f32_to_u8(input[0]),
                        f32_to_u8(input[1]),
                        f32_to_u8(input[2]),
                        f32_to_u8(input[3]),
                    ];
                    CMYK_TRANSFORM.convert_u8(&converted, output)
                } else {
                    let converted = input.iter().copied().map(f32_to_u8).collect::<Vec<_>>();
                    CMYK_TRANSFORM.convert_u8(&converted, output)
                }
            }
            ColorSpaceType::DeviceGray => {
                for (gray, output) in input.iter().zip(output.chunks_exact_mut(3)) {
                    let gray = f32_to_u8(*gray);
                    output.copy_from_slice(&[gray, gray, gray]);
                }

                Some(())
            }
            ColorSpaceType::DeviceRgb => {
                for (input, output) in input.iter().copied().zip(output) {
                    *output = f32_to_u8(input);
                }

                Some(())
            }
            ColorSpaceType::Pattern(i) => i.convert_f32(input, output, manual_scale),
            ColorSpaceType::Indexed(i) => i.convert_f32(input, output, manual_scale),
            ColorSpaceType::ICCBased(i) => i.convert_f32(input, output, manual_scale),
            ColorSpaceType::CalGray(i) => i.convert_f32(input, output, manual_scale),
            ColorSpaceType::CalRgb(i) => i.convert_f32(input, output, manual_scale),
            ColorSpaceType::Lab(i) => i.convert_f32(input, output, manual_scale),
            ColorSpaceType::Separation(i) => i.convert_f32(input, output, manual_scale),
            ColorSpaceType::DeviceN(i) => i.convert_f32(input, output, manual_scale),
        }
    }

    fn supports_u8(&self) -> bool {
        match self.0.as_ref() {
            ColorSpaceType::DeviceCmyk => true,
            ColorSpaceType::DeviceGray => true,
            ColorSpaceType::DeviceRgb => true,
            ColorSpaceType::Pattern(i) => i.supports_u8(),
            ColorSpaceType::Indexed(i) => i.supports_u8(),
            ColorSpaceType::ICCBased(i) => i.supports_u8(),
            ColorSpaceType::CalGray(i) => i.supports_u8(),
            ColorSpaceType::CalRgb(i) => i.supports_u8(),
            ColorSpaceType::Lab(i) => i.supports_u8(),
            ColorSpaceType::Separation(i) => i.supports_u8(),
            ColorSpaceType::DeviceN(i) => i.supports_u8(),
        }
    }

    fn convert_u8(&self, input: &[u8], output: &mut [u8]) -> Option<()> {
        match self.0.as_ref() {
            ColorSpaceType::DeviceCmyk => CMYK_TRANSFORM.convert_u8(input, output),
            ColorSpaceType::DeviceGray => {
                for (input, output) in input.iter().zip(output.chunks_exact_mut(3)) {
                    output.copy_from_slice(&[*input, *input, *input]);
                }

                Some(())
            }
            ColorSpaceType::DeviceRgb => {
                for (input, output) in input.iter().zip(output.iter_mut()) {
                    *output = *input;
                }

                Some(())
            }
            ColorSpaceType::Pattern(i) => i.convert_u8(input, output),
            ColorSpaceType::Indexed(i) => i.convert_u8(input, output),
            ColorSpaceType::ICCBased(i) => i.convert_u8(input, output),
            ColorSpaceType::CalGray(i) => i.convert_u8(input, output),
            ColorSpaceType::CalRgb(i) => i.convert_u8(input, output),
            ColorSpaceType::Lab(i) => i.convert_u8(input, output),
            ColorSpaceType::Separation(i) => i.convert_u8(input, output),
            ColorSpaceType::DeviceN(i) => i.convert_u8(input, output),
        }
    }

    fn is_none(&self) -> bool {
        match self.0.as_ref() {
            ColorSpaceType::Separation(s) => s.is_none(),
            ColorSpaceType::DeviceN(d) => d.is_none(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CalGray {
    white_point: [f32; 3],
    black_point: [f32; 3],
    gamma: f32,
}

// See <https://github.com/mozilla/pdf.js/blob/06f44916c8936b92f464d337fe3a0a6b2b78d5b4/src/core/colorspace.js#L752>
impl CalGray {
    fn new(dict: &Dict<'_>) -> Option<Self> {
        let white_point = dict.get::<[f32; 3]>(WHITE_POINT).unwrap_or([1.0, 1.0, 1.0]);
        let black_point = dict.get::<[f32; 3]>(BLACK_POINT).unwrap_or([0.0, 0.0, 0.0]);
        let gamma = dict.get::<f32>(GAMMA).unwrap_or(1.0);

        Some(Self {
            white_point,
            black_point,
            gamma,
        })
    }
}

impl ToRgb for CalGray {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], _: bool) -> Option<()> {
        for (input, output) in input.iter().copied().zip(output.chunks_exact_mut(3)) {
            let g = self.gamma;
            let (_xw, yw, _zw) = {
                let wp = self.white_point;
                (wp[0], wp[1], wp[2])
            };
            let (_xb, _yb, _zb) = {
                let bp = self.black_point;
                (bp[0], bp[1], bp[2])
            };

            let a = input;
            let ag = a.powf(g);
            let l = yw * ag;
            let val = (0.0_f32.max(295.8 * l.powf(0.333_333_34) - 40.8) + 0.5) as u8;

            output.copy_from_slice(&[val, val, val]);
        }

        Some(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CalRgb {
    white_point: [f32; 3],
    black_point: [f32; 3],
    matrix: [f32; 9],
    gamma: [f32; 3],
}

// See <https://github.com/mozilla/pdf.js/blob/06f44916c8936b92f464d337fe3a0a6b2b78d5b4/src/core/colorspace.js#L846>
// Completely copied from there without really understanding the logic, but we get the same results as Firefox
// which should be good enough (and by viewing the `calrgb.pdf` test file in different viewers you will
// see that in many cases each viewer does whatever it wants, even Acrobat), so this is good enough for us.
impl CalRgb {
    fn new(dict: &Dict<'_>) -> Option<Self> {
        let white_point = dict.get::<[f32; 3]>(WHITE_POINT).unwrap_or([1.0, 1.0, 1.0]);
        let black_point = dict.get::<[f32; 3]>(BLACK_POINT).unwrap_or([0.0, 0.0, 0.0]);
        let matrix = dict
            .get::<[f32; 9]>(MATRIX)
            .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        let gamma = dict.get::<[f32; 3]>(GAMMA).unwrap_or([1.0, 1.0, 1.0]);

        Some(Self {
            white_point,
            black_point,
            matrix,
            gamma,
        })
    }

    const BRADFORD_SCALE_MATRIX: [f32; 9] = [
        0.8951, 0.2664, -0.1614, -0.7502, 1.7135, 0.0367, 0.0389, -0.0685, 1.0296,
    ];

    const BRADFORD_SCALE_INVERSE_MATRIX: [f32; 9] = [
        0.9869929, -0.1470543, 0.1599627, 0.4323053, 0.5183603, 0.0492912, -0.0085287, 0.0400428,
        0.9684867,
    ];

    const SRGB_D65_XYZ_TO_RGB_MATRIX: [f32; 9] = [
        3.2404542, -1.5371385, -0.4985314, -0.969_266, 1.8760108, 0.0415560, 0.0556434, -0.2040259,
        1.0572252,
    ];

    const FLAT_WHITEPOINT: [f32; 3] = [1.0, 1.0, 1.0];
    const D65_WHITEPOINT: [f32; 3] = [0.95047, 1.0, 1.08883];

    fn decode_l_constant() -> f32 {
        ((8.0_f32 + 16.0) / 116.0).powi(3) / 8.0
    }

    fn srgb_transfer_function(color: f32) -> f32 {
        if color <= 0.0031308 {
            (12.92 * color).clamp(0.0, 1.0)
        } else if color >= 0.99554525 {
            1.0
        } else {
            ((1.0 + 0.055) * color.powf(1.0 / 2.4) - 0.055).clamp(0.0, 1.0)
        }
    }

    fn matrix_product(a: &[f32; 9], b: &[f32; 3]) -> [f32; 3] {
        [
            a[0] * b[0] + a[1] * b[1] + a[2] * b[2],
            a[3] * b[0] + a[4] * b[1] + a[5] * b[2],
            a[6] * b[0] + a[7] * b[1] + a[8] * b[2],
        ]
    }

    fn to_flat(source_white_point: &[f32; 3], lms: &[f32; 3]) -> [f32; 3] {
        [
            lms[0] / source_white_point[0],
            lms[1] / source_white_point[1],
            lms[2] / source_white_point[2],
        ]
    }

    fn to_d65(source_white_point: &[f32; 3], lms: &[f32; 3]) -> [f32; 3] {
        [
            lms[0] * Self::D65_WHITEPOINT[0] / source_white_point[0],
            lms[1] * Self::D65_WHITEPOINT[1] / source_white_point[1],
            lms[2] * Self::D65_WHITEPOINT[2] / source_white_point[2],
        ]
    }

    fn decode_l(l: f32) -> f32 {
        if l < 0.0 {
            -Self::decode_l(-l)
        } else if l > 8.0 {
            ((l + 16.0) / 116.0).powi(3)
        } else {
            l * Self::decode_l_constant()
        }
    }

    fn compensate_black_point(source_bp: &[f32; 3], xyz_flat: &[f32; 3]) -> [f32; 3] {
        if source_bp == &[0.0, 0.0, 0.0] {
            return *xyz_flat;
        }

        let zero_decode_l = Self::decode_l(0.0);

        let mut out = [0.0; 3];
        for i in 0..3 {
            let src = Self::decode_l(source_bp[i]);
            let scale = (1.0 - zero_decode_l) / (1.0 - src);
            let offset = 1.0 - scale;
            out[i] = xyz_flat[i] * scale + offset;
        }

        out
    }

    fn normalize_white_point_to_flat(
        &self,
        source_white_point: &[f32; 3],
        xyz: &[f32; 3],
    ) -> [f32; 3] {
        if source_white_point[0] == 1.0 && source_white_point[2] == 1.0 {
            return *xyz;
        }
        let lms = Self::matrix_product(&Self::BRADFORD_SCALE_MATRIX, xyz);
        let lms_flat = Self::to_flat(source_white_point, &lms);
        Self::matrix_product(&Self::BRADFORD_SCALE_INVERSE_MATRIX, &lms_flat)
    }

    fn normalize_white_point_to_d65(source_white_point: &[f32; 3], xyz: &[f32; 3]) -> [f32; 3] {
        let lms = Self::matrix_product(&Self::BRADFORD_SCALE_MATRIX, xyz);
        let lms_d65 = Self::to_d65(source_white_point, &lms);
        Self::matrix_product(&Self::BRADFORD_SCALE_INVERSE_MATRIX, &lms_d65)
    }
}

impl ToRgb for CalRgb {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], _: bool) -> Option<()> {
        for (input, output) in input.chunks_exact(3).zip(output.chunks_exact_mut(3)) {
            let input = [
                input[0].clamp(0.0, 1.0),
                input[1].clamp(0.0, 1.0),
                input[2].clamp(0.0, 1.0),
            ];

            let [r, g, b] = input;
            let [gr, gg, gb] = self.gamma;
            let [agr, bgg, cgb] = [
                if r == 1.0 { 1.0 } else { r.powf(gr) },
                if g == 1.0 { 1.0 } else { g.powf(gg) },
                if b == 1.0 { 1.0 } else { b.powf(gb) },
            ];

            let m = &self.matrix;
            let x = m[0] * agr + m[3] * bgg + m[6] * cgb;
            let y = m[1] * agr + m[4] * bgg + m[7] * cgb;
            let z = m[2] * agr + m[5] * bgg + m[8] * cgb;
            let xyz = [x, y, z];

            let xyz_flat = self.normalize_white_point_to_flat(&self.white_point, &xyz);
            let xyz_black = Self::compensate_black_point(&self.black_point, &xyz_flat);
            let xyz_d65 = Self::normalize_white_point_to_d65(&Self::FLAT_WHITEPOINT, &xyz_black);
            let srgb_xyz = Self::matrix_product(&Self::SRGB_D65_XYZ_TO_RGB_MATRIX, &xyz_d65);

            output.copy_from_slice(&[
                (Self::srgb_transfer_function(srgb_xyz[0]) * 255.0 + 0.5) as u8,
                (Self::srgb_transfer_function(srgb_xyz[1]) * 255.0 + 0.5) as u8,
                (Self::srgb_transfer_function(srgb_xyz[2]) * 255.0 + 0.5) as u8,
            ]);
        }

        Some(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Lab {
    range: [f32; 4],
    white_point: [f32; 3],
}

impl Lab {
    fn new(dict: &Dict<'_>) -> Option<Self> {
        let white_point = dict.get::<[f32; 3]>(WHITE_POINT).unwrap_or([1.0, 1.0, 1.0]);
        let range = dict
            .get::<[f32; 4]>(RANGE)
            .unwrap_or([-100.0, 100.0, -100.0, 100.0]);

        Some(Self { range, white_point })
    }

    /// The white point the space's L\*a\*b\* values are relative to (ISO 32000-2
    /// §8.6.5.4). A missing or degenerate entry falls back to a flat white point
    /// so hostile input cannot break the conversion.
    fn usable_white_point(&self) -> [f32; 3] {
        let white_point = self.white_point;
        if white_point
            .iter()
            .all(|component| component.is_finite() && *component > 0.0)
        {
            white_point
        } else {
            [1.0, 1.0, 1.0]
        }
    }

    /// Bradford adaptation scales cone responses, so both white points must
    /// be expressed in LMS before taking their ratios. CalRGB's separate
    /// flat-white/black-point pipeline retains its existing normalization.
    fn adapt_to_d65(white_point: &[f32; 3], xyz: &[f32; 3]) -> [f32; 3] {
        let source_lms = CalRgb::matrix_product(&CalRgb::BRADFORD_SCALE_MATRIX, white_point);
        // A malformed but positive XYZ white can still yield a zero or
        // overflowing cone response. Leave it unadapted rather than divide by
        // zero or propagate a non-finite scale.
        if source_lms.iter().any(|v| !v.is_finite() || v.abs() <= f32::EPSILON) {
            return *xyz;
        }
        let destination_lms = CalRgb::matrix_product(&CalRgb::BRADFORD_SCALE_MATRIX, &CalRgb::D65_WHITEPOINT);
        let lms = CalRgb::matrix_product(&CalRgb::BRADFORD_SCALE_MATRIX, xyz);
        let adapted = [
            lms[0] * destination_lms[0] / source_lms[0],
            lms[1] * destination_lms[1] / source_lms[1],
            lms[2] * destination_lms[2] / source_lms[2],
        ];
        CalRgb::matrix_product(&CalRgb::BRADFORD_SCALE_INVERSE_MATRIX, &adapted)
    }

    /// Inverse of the CIE f(t) helper that maps relative fX/fY/fZ to absolute
    /// XYZ as a ratio of the white point.
    fn inverse_lab_f(t: f32) -> f32 {
        const DELTA: f32 = 6.0 / 29.0;
        if t > DELTA {
            t * t * t
        } else {
            3.0 * DELTA * DELTA * (t - 4.0 / 29.0)
        }
    }
}

impl ToRgb for Lab {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], manual_scale: bool) -> Option<()> {
        let [min_a, max_a, min_b, max_b] = self.range;
        let white_point = self.usable_white_point();
        let [xw, yw, zw] = white_point;

        for (input, output) in input.chunks_exact(3).zip(output.chunks_exact_mut(3)) {
            // PdfCraft patch: `manual_scale` means the components arrive
            // normalised to [0, 1] (an `/Indexed` palette over Lab looks its
            // bytes up as `byte / 255`). Map them linearly onto L* 0..100 and
            // the a*/b* /Range (ISO 32000-2 §8.6.6.3) before converting.
            let (l, a, b) = if manual_scale {
                (
                    clamp_component(input[0], 0.0, 1.0) * 100.0,
                    min_a + clamp_component(input[1], 0.0, 1.0) * (max_a - min_a),
                    min_b + clamp_component(input[2], 0.0, 1.0) * (max_b - min_b),
                )
            } else {
                (input[0], input[1], input[2])
            };
            let l = clamp_component(l, 0.0, 100.0);
            let a = clamp_component(a, min_a, max_a);
            let b = clamp_component(b, min_b, max_b);

            // L*a*b* is relative to the space's own /WhitePoint. Map it to XYZ
            // in that white's coordinates, Bradford-adapt to D65 and convert to
            // sRGB (ISO 32000-2 §8.6.5.4).
            let fy = (l + 16.0) / 116.0;
            let fx = fy + a / 500.0;
            let fz = fy - b / 200.0;
            let xyz = [
                xw * Self::inverse_lab_f(fx),
                yw * Self::inverse_lab_f(fy),
                zw * Self::inverse_lab_f(fz),
            ];

            let xyz_d65 = Self::adapt_to_d65(&white_point, &xyz);
            let srgb = CalRgb::matrix_product(&CalRgb::SRGB_D65_XYZ_TO_RGB_MATRIX, &xyz_d65);

            output.copy_from_slice(&[
                f32_to_u8(CalRgb::srgb_transfer_function(srgb[0])),
                f32_to_u8(CalRgb::srgb_transfer_function(srgb[1])),
                f32_to_u8(CalRgb::srgb_transfer_function(srgb[2])),
            ]);
        }

        Some(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Indexed {
    values: Vec<Vec<f32>>,
    hival: u8,
    base: Box<ColorSpace>,
}

impl Indexed {
    fn new(array: &Array<'_>, cache: &Cache) -> Option<Self> {
        let mut iter = array.flex_iter();
        // Skip name
        let _ = iter.next::<Name<'_>>()?;
        let base_color_space = ColorSpace::new(iter.next::<Object<'_>>()?, cache)?;
        let hival = iter.next::<u32>()?.min(u8::MAX as u32) as u8;

        let values = {
            let data = iter
                .next::<Stream<'_>>()
                .and_then(|s| s.decoded().ok())
                .or_else(|| {
                    iter.next::<object::String<'_>>()
                        .map(|s| Cow::Owned(s.to_vec()))
                })?;

            let num_components = base_color_space.num_components();

            let mut byte_iter = data.iter().copied();

            let mut vals = vec![];
            for _ in 0..=hival {
                let mut temp = vec![];

                for _ in 0..num_components {
                    temp.push(byte_iter.next()? as f32 / 255.0);
                }

                vals.push(temp);
            }

            vals
        };

        Some(Self {
            values,
            hival,
            base: Box::new(base_color_space),
        })
    }
}

impl ToRgb for Indexed {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], _: bool) -> Option<()> {
        let mut indexed = vec![0.0; input.len() * self.base.num_components() as usize];

        for (input, output) in input
            .iter()
            .copied()
            .zip(indexed.chunks_exact_mut(self.base.num_components() as usize))
        {
            let idx = (input.clamp(0.0, self.hival as f32) + 0.5) as usize;
            output.copy_from_slice(&self.values[idx]);
        }

        self.base.convert_f32(&indexed, output, true)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Separation {
    alternate_space: ColorSpace,
    /// PdfCraft patch (55): `None` when the tint transform cannot be read (see
    /// `eval_tint_transform`).
    tint_transform: Option<Function>,
    is_none_separation: bool,
    /// PdfCraft patch (66): the colourant is the special name `All`.
    is_all: bool,
}

/// PdfCraft patch (55): `tint_transform` run on the tints of one colour, or, when it cannot be
/// read or evaluated, the initial colour of `alternate_space` (black in every device space), with
/// the fallback recorded. hayro already fell back this way when evaluation failed, silently; a
/// transform it could not read made the whole colour space unreadable, and the space's
/// paints were drawn in DeviceGray from their first tint instead (white for a full tint).
fn eval_tint_transform(
    tint_transform: Option<&Function>,
    alternate_space: &ColorSpace,
    tints: &[f32],
) -> ColorComponents {
    tint_transform
        .and_then(|f| f.eval(tints.to_smallvec()))
        .unwrap_or_else(|| {
            note_color_fallback();
            alternate_space.initial_color()
        })
}

impl Separation {
    fn new(array: &Array<'_>, cache: &Cache) -> Option<Self> {
        let mut iter = array.flex_iter();
        // Skip `/Separation`
        let _ = iter.next::<Name<'_>>()?;
        let name = iter.next::<Name<'_>>()?;
        let alternate_space = ColorSpace::new(iter.next::<Object<'_>>()?, cache)?;
        // PdfCraft patch (55): an unreadable tint transform keeps the space (see
        // `eval_tint_transform`).
        let tint_transform = iter.next::<Object<'_>>().and_then(|f| Function::new(&f));
        let is_none_separation = name.as_str() == "None";
        // PdfCraft patch (66): `All` names every colourant of the output device at once
        // (ISO 32000-2 §8.6.6.4), so its tints never go through the alternate space.
        let is_all = name.as_str() == "All";

        Some(Self {
            alternate_space,
            tint_transform,
            is_none_separation,
            is_all,
        })
    }
}

impl ToRgb for Separation {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], _: bool) -> Option<()> {
        if self.is_all {
            // PdfCraft patch (66): §8.6.6.4: the tint applies to every colourant, and on an
            // additive device it is complemented first, so tint t is the gray 1 − t on each of
            // R, G and B whatever the alternate space and tint transform say.
            for (tint, rgb) in input.iter().zip(output.chunks_exact_mut(3)) {
                // `clamp` keeps NaN, and `f32_to_u8`'s cast saturates it to 0.
                let gray = f32_to_u8(1.0 - tint.clamp(0.0, 1.0));
                rgb.copy_from_slice(&[gray, gray, gray]);
            }
            return Some(());
        }
        count_tint_transform_runs(input.len());
        // PdfCraft patch (55): see `eval_tint_transform`.
        let evaluated = input
            .iter()
            .flat_map(|n| {
                eval_tint_transform(
                    self.tint_transform.as_ref(),
                    &self.alternate_space,
                    &[*n],
                )
            })
            .collect::<Vec<_>>();
        self.alternate_space.convert_f32(&evaluated, output, false)
    }

    fn is_none(&self) -> bool {
        self.is_none_separation
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DeviceN {
    alternate_space: ColorSpace,
    num_components: u8,
    /// PdfCraft patch (55): `None` when the tint transform cannot be read (see
    /// `eval_tint_transform`).
    tint_transform: Option<Function>,
    is_none: bool,
}

impl DeviceN {
    fn new(array: &Array<'_>, cache: &Cache) -> Option<Self> {
        let mut iter = array.flex_iter();
        // Skip `/DeviceN`
        let _ = iter.next::<Name<'_>>()?;
        // Skip `Name`.
        let names = iter
            .next::<Array<'_>>()?
            .iter::<Name<'_>>()
            .collect::<Vec<_>>();
        let num_components = u8::try_from(names.len()).ok()?;
        let all_none = names.iter().all(|n| n.as_str() == "None");
        let alternate_space = ColorSpace::new(iter.next::<Object<'_>>()?, cache)?;
        // PdfCraft patch (55): an unreadable tint transform keeps the space (see
        // `eval_tint_transform`).
        let tint_transform = iter.next::<Object<'_>>().and_then(|f| Function::new(&f));

        if num_components == 0 {
            return None;
        }

        Some(Self {
            alternate_space,
            num_components,
            tint_transform,
            is_none: all_none,
        })
    }
}

impl ToRgb for DeviceN {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], _: bool) -> Option<()> {
        let colours = input.len().checked_div(usize::from(self.num_components));
        count_tint_transform_runs(colours.unwrap_or(0));
        // PdfCraft patch (55): see `eval_tint_transform`.
        let evaluated = input
            .chunks_exact(self.num_components as usize)
            .flat_map(|n| {
                eval_tint_transform(self.tint_transform.as_ref(), &self.alternate_space, n)
            })
            .collect::<Vec<_>>();
        self.alternate_space.convert_f32(&evaluated, output, false)
    }

    fn is_none(&self) -> bool {
        self.is_none
    }
}

struct ICCColorRepr {
    src_profile: ColorProfile,
    src_layout: Layout,
    number_components: usize,
    is_srgb: bool,
    is_lab: bool,
    transform_u8: Arc<Transform8BitExecutor>,
    transform_f32: OnceLock<Arc<TransformF32Executor>>,
}

#[derive(Clone)]
pub(crate) struct ICCProfile(Arc<ICCColorRepr>);

impl Debug for ICCProfile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "ICCColor {{..}}")
    }
}

impl ICCProfile {
    fn new(profile: &[u8], number_components: usize) -> Option<Self> {
        let src_profile = ColorProfile::new_from_slice(profile).ok()?;

        const SRGB_MARKER: &[u8] = b"sRGB";

        let is_srgb = profile
            .get(52..56)
            .map(|device_model| device_model == SRGB_MARKER)
            .unwrap_or(false);
        let is_lab = src_profile.color_space == DataColorSpace::Lab;

        Self::new_from_src_profile(src_profile, is_srgb, is_lab, number_components)
    }

    fn new_from_src_profile(
        src_profile: ColorProfile,
        is_srgb: bool,
        is_lab: bool,
        number_components: usize,
    ) -> Option<Self> {
        let src_layout = match number_components {
            1 => Layout::Gray,
            3 => Layout::Rgb,
            4 => Layout::Rgba,
            _ => {
                warn!("unsupported number of components {number_components} for ICC profile");

                return None;
            }
        };

        let dest_profile = ColorProfile::new_srgb();
        let transform_u8 = src_profile
            .clone()
            .create_transform_8bit(
                src_layout,
                &dest_profile,
                Layout::Rgb,
                TransformOptions::default(),
            )
            .ok()?;

        Some(Self(Arc::new(ICCColorRepr {
            src_profile,
            src_layout,
            number_components,
            is_srgb,
            is_lab,
            transform_u8,
            transform_f32: OnceLock::new(),
        })))
    }

    fn is_srgb(&self) -> bool {
        self.0.is_srgb
    }

    fn is_lab(&self) -> bool {
        self.0.is_lab
    }

    fn transform_u8(&self) -> &Arc<Transform8BitExecutor> {
        &self.0.transform_u8
    }

    fn transform_f32(&self) -> &Arc<TransformF32Executor> {
        // From my benchmarking, creating the f32 transforms is usually much
        // more expensive than u8. Therefore, we only create it lazily when
        // really needed.
        self.0.transform_f32.get_or_init(|| {
            let dest_profile = ColorProfile::new_srgb();
            self.0
                .src_profile
                .clone()
                .create_transform_f32(
                    self.0.src_layout,
                    &dest_profile,
                    Layout::Rgb,
                    TransformOptions::default(),
                )
                // Since the u8 version was valid, hopefully this should never panic?
                .unwrap()
        })
    }
}

impl ToRgb for ICCProfile {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], _: bool) -> Option<()> {
        let mut temp = vec![0.0_f32; output.len()];

        if self.is_lab() {
            // moxcms expects normalized values.
            let scaled = input
                .chunks_exact(3)
                .flat_map(|i| {
                    [
                        i[0] * (1.0 / 100.0),
                        (i[1] + 128.0) * (1.0 / 255.0),
                        (i[2] + 128.0) * (1.0 / 255.0),
                    ]
                })
                .collect::<Vec<_>>();
            self.transform_f32().transform(&scaled, &mut temp).ok()?;
        } else {
            self.transform_f32().transform(input, &mut temp).ok()?;
        };

        for (input, output) in temp.iter().zip(output.iter_mut()) {
            *output = (input * 255.0 + 0.5) as u8;
        }

        Some(())
    }

    fn supports_u8(&self) -> bool {
        true
    }

    fn convert_u8(&self, input: &[u8], output: &mut [u8]) -> Option<()> {
        if self.is_srgb() {
            output.copy_from_slice(input);
        } else {
            self.transform_u8().transform(input, output).ok()?;
        }

        Some(())
    }
}

#[inline(always)]
fn f32_to_u8(val: f32) -> u8 {
    (val * 255.0 + 0.5) as u8
}

// PdfCraft patch (63).
/// The `/DeviceCMYK` to `/DeviceGray` conversion of ISO 32000-2 §10.4.2.3,
/// `gray = 1.0 - min(1.0, 0.3 c + 0.59 m + 0.11 y + k)`, which §11.5.3 also gives for the
/// luminosity of a soft mask whose group is in `/DeviceCMYK`. A missing component counts as
/// zero, and each component is clamped to `[0, 1]` first (`NaN` and infinities read as zero).
pub(crate) fn device_cmyk_to_gray(cmyk: &[f32]) -> f32 {
    let component = |i: usize| clamp_component(cmyk.get(i).copied().unwrap_or(0.0), 0.0, 1.0);
    let ink = 0.3 * component(0) + 0.59 * component(1) + 0.11 * component(2) + component(3);
    // Every component is in [0, 1], so `ink` is not negative and the result is in [0, 1].
    1.0 - ink.min(1.0)
}

/// Clamp an input-derived colour component to `[lo, hi]` without panicking on
/// NaN or on a reversed range. Non-finite values map to zero.
fn clamp_component(value: f32, lo: f32, hi: f32) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    value.max(lo).min(hi)
}

#[derive(Debug, Clone)]
/// A color.
pub struct Color {
    color_space: ColorSpace,
    components: ColorComponents,
    opacity: f32,
}

impl Color {
    /// PdfCraft patch (46): the same colour, fully opaque (see `Pattern::without_opacity`).
    pub(crate) fn opaque(mut self) -> Self {
        self.opacity = 1.0;
        self
    }

    pub(crate) fn new(color_space: ColorSpace, components: ColorComponents, opacity: f32) -> Self {
        Self {
            color_space,
            components,
            opacity,
        }
    }

    /// Return the color as an RGBA color.
    pub fn to_rgba(&self) -> AlphaColor {
        self.color_space
            .to_rgba(&self.components, self.opacity, false)
    }

    /// PdfCraft patch (70): the colour as process ink tints `[C, M, Y, K]` (0 = no ink) and
    /// its opacity, for blending in a `DeviceCMYK` transparency group (ISO 32000-2 §11.3.4).
    ///
    /// `DeviceCMYK` gives its components. `DeviceGray` is black ink alone, `K = 1 − gray`
    /// (§10.4.2.2). `None` for every other space, ICCBased CMYK included: such a colour has no
    /// tints without a colour conversion this crate does not make (an ICCBased colour keeps
    /// its own profile), and the caller draws it as before.
    pub fn ink_tints(&self) -> Option<([f32; 4], f32)> {
        let c = |i: usize| {
            self.components
                .get(i)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0)
        };
        let tints = match self.color_space.0.as_ref() {
            ColorSpaceType::DeviceCmyk => [c(0), c(1), c(2), c(3)],
            ColorSpaceType::DeviceGray => [0.0, 0.0, 0.0, 1.0 - c(0)],
            _ => return None,
        };
        Some((tints, self.opacity.clamp(0.0, 1.0)))
    }

    /// Create a color from RGBA.
    pub fn from_rgba(rgba: AlphaColor) -> Self {
        let c = rgba.components();
        Self {
            color_space: ColorSpace::device_rgb(),
            components: smallvec![c[0], c[1], c[2]],
            opacity: c[3],
        }
    }
}

/// PdfCraft patch (70): `input`, 8-bit `DeviceCMYK` pixels (four bytes each), converted to
/// 8-bit sRGB in `output` (three bytes each) through the CMYK profile pages are drawn with,
/// so a colour blended in CMYK lands where the same colour painted directly does.
pub fn cmyk_to_srgb8(input: &[u8], output: &mut [u8]) -> Option<()> {
    if input.len() % 4 != 0 || output.len() != input.len() / 4 * 3 {
        return None;
    }
    CMYK_TRANSFORM.convert_u8(input, output)
}

static CMYK_TRANSFORM: LazyLock<ICCProfile> = LazyLock::new(|| {
    ICCProfile::new(include_bytes!("../assets/CGATS001Compat-v2-micro.icc"), 4).unwrap()
});

pub(crate) trait ToRgb {
    fn convert_sample(&self, input: &[f32], output: &mut [u8], manual_scale: bool) -> Option<()> {
        // We prefer using the u8 variant for single samples, which is especially
        // important for ICC profiles to avoid constructing the (more expensive)
        // f32 variant.
        if self.supports_u8() {
            let converted = input
                .iter()
                .copied()
                .map(f32_to_u8)
                .collect::<SmallVec<[u8; 4]>>();

            if self.convert_u8(&converted, output).is_some() {
                return Some(());
            }
        }

        self.convert_f32(input, output, manual_scale)
    }

    fn convert_f32(&self, input: &[f32], output: &mut [u8], manual_scale: bool) -> Option<()>;
    fn supports_u8(&self) -> bool {
        false
    }
    fn convert_u8(&self, _: &[u8], _: &mut [u8]) -> Option<()> {
        unimplemented!();
    }
    fn is_none(&self) -> bool {
        false
    }
    fn to_alpha_color(
        &self,
        input: &[f32],
        mut opacity: f32,
        manual_scale: bool,
    ) -> Option<AlphaColor> {
        let mut output = [0; 3];
        self.convert_sample(input, &mut output, manual_scale)?;

        // For separation color spaces:
        // "The special colourant name None shall not produce any visible output.
        // Painting operations in a Separation space with this colourant name
        // shall have no effect on the current page."
        if self.is_none() {
            opacity = 0.0;
        }

        Some(AlphaColor::from_rgba8(
            output[0],
            output[1],
            output[2],
            (opacity * 255.0 + 0.5) as u8,
        ))
    }
}

#[cfg(test)]
mod device_cmyk_gray_tests {
    use super::device_cmyk_to_gray;

    fn assert_close(cmyk: &[f32], expected: f32) {
        let got = device_cmyk_to_gray(cmyk);
        assert!(
            (got - expected).abs() < 1e-5,
            "{cmyk:?}: gray {got}, expected {expected}"
        );
    }

    #[test]
    fn each_ink_darkens_by_its_iso_weight() {
        assert_close(&[0.0, 0.0, 0.0, 0.0], 1.0);
        assert_close(&[0.0, 0.0, 0.0, 1.0], 0.0);
        assert_close(&[0.0, 0.0, 0.0, 0.5], 0.5);
        assert_close(&[1.0, 0.0, 0.0, 0.0], 0.7);
        assert_close(&[0.0, 1.0, 0.0, 0.0], 0.41);
        assert_close(&[0.0, 0.0, 1.0, 0.0], 0.89);
        assert_close(&[0.5, 0.5, 0.5, 0.25], 0.25);
    }

    #[test]
    fn ink_past_full_coverage_is_black_and_bad_components_read_as_zero() {
        assert_close(&[1.0, 1.0, 1.0, 1.0], 0.0);
        assert_close(&[0.0, 0.0, 0.0, 1.5], 0.0);
        assert_close(&[2.0, -1.0, f32::NAN, f32::INFINITY], 0.7);
        assert_close(&[], 1.0);
        assert_close(&[0.0, 0.0], 1.0);
    }
}
