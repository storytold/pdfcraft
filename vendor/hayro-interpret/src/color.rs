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
    ColorProfile, DataColorSpace, Layout, ProfileText, RenderingIntent, Transform8BitExecutor,
    TransformF32Executor, TransformOptions,
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
    /// PdfCraft patch (35): `inner` drawn as the plate of one ink (see [`Plate`]).
    Plate(PlateSpace),
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

/// PdfCraft patch (74): how deeply colour spaces may nest. A legal file goes five levels at most:
/// a Pattern's underlying space, an Indexed base, a Separation's alternate, an ICC profile, and
/// that profile's alternate. Reading an object already being read is refused, which ends a cycle
/// but not a chain: 5,000 spaces each naming the next, 180 KB of file, overflowed the stack, and
/// that aborts the process.
const MAX_COLOR_SPACE_DEPTH: u32 = 32;

impl ColorSpaceType {
    fn new(object: Object<'_>, cache: &Cache, depth: u32) -> Option<Self> {
        if depth >= MAX_COLOR_SPACE_DEPTH {
            warn!("colour spaces nest past the limit of {MAX_COLOR_SPACE_DEPTH} levels");
            return None;
        }

        Self::new_inner(object, cache, depth)
    }

    fn new_inner(object: Object<'_>, cache: &Cache, depth: u32) -> Option<Self> {
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
                    // PdfCraft patch (74): what's kept is what the profile itself reads as. What
                    // it falls back to depends on how deep it was met, and kept with the profile
                    // a page could build a chain past the limit on spaces already read.
                    let read = || {
                        let data = icc_stream.raw_data();
                        let content = IccProfileKey::Content {
                            dict: icc_stream.cache_key(),
                            data: &data,
                        };
                        cache.get_or_insert_with(hash128(&content), || {
                            cache.count_icc_profile_read();
                            let decoded = icc_stream.decoded().ok()?;
                            Some(ICCProfile::new(&decoded, num_components).map(|icc| {
                                // TODO: For SVG and PNG we can assume that the output color space is
                                // sRGB. If we ever implement PDF-to-PDF, we probably want to
                                // let the user pass the native color type and don't make this optimization
                                // if it's not sRGB.
                                if icc.is_srgb() {
                                    Self::DeviceRgb
                                } else {
                                    Self::ICCBased(icc)
                                }
                            }))
                        })
                    };
                    let profile = match icc_stream_object(&color_array, &icc_stream) {
                        Some(object) => {
                            cache.get_or_insert_with(hash128(&IccProfileKey::Object(object)), read)
                        }
                        None => read(),
                    };
                    return profile?
                        .or_else(|| {
                            dict.get::<Object<'_>>(ALTERNATE)
                                .and_then(|o| Self::new(o, cache, depth + 1))
                        })
                        .or_else(|| match dict.get::<u8>(N) {
                            Some(1) => Some(Self::DeviceGray),
                            Some(3) => Some(Self::DeviceRgb),
                            Some(4) => Some(Self::DeviceCmyk),
                            _ => None,
                        });
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
                    return Some(Self::Indexed(Indexed::new(&color_array, cache, depth, false)?));
                }
                SEPARATION => {
                    return Some(Self::Separation(Separation::new(&color_array, cache, depth)?));
                }
                DEVICE_N => {
                    return Some(Self::DeviceN(DeviceN::new(&color_array, cache, depth)?));
                }
                PATTERN => {
                    // PdfCraft patch (72): the family is already read. Reading a name here too took
                    // the underlying space when it was a name or a reference.
                    let cs = iter
                        .next::<Object<'_>>()
                        .and_then(|o| ColorSpace::within(o, cache, depth + 1))
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
        Self::within(object, cache, 0)
    }

    /// A colour space named by another, `depth` levels down.
    fn within(object: Object<'_>, cache: &Cache, depth: u32) -> Option<Self> {
        Some(Self(Arc::new(ColorSpaceType::new(object, cache, depth)?)))
    }

    /// PdfCraft patch (80): as `new`, except that an `/Indexed` space with no lookup table is
    /// built (marked tableless) rather than unreadable, for the image and colour operators to
    /// handle it (see `Indexed::new`). Every other space is read as by `new`.
    pub(crate) fn new_allowing_tableless(object: Object<'_>, cache: &Cache) -> Option<Self> {
        if let Object::Array(array) = &object
            && array
                .flex_iter()
                .next::<Name<'_>>()
                .is_some_and(|family| matches!(family.deref(), INDEXED | I))
        {
            return Indexed::new(array, cache, 0, true)
                .map(|indexed| Self(Arc::new(ColorSpaceType::Indexed(indexed))));
        }
        Self::new(object, cache)
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
        match self.0.as_ref() {
            ColorSpaceType::Indexed(_) => true,
            // PdfCraft patch (35): a plate decodes its samples as its space does.
            ColorSpaceType::Plate(p) => p.inner.is_indexed(),
            _ => false,
        }
    }

    /// PdfCraft patch (80): whether this is an indexed colour space that broke ISO 32000-2
    /// §8.6.6.3 and was recovered (see `Indexed::new`).
    pub(crate) fn is_recovered_indexed(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::Indexed(i) if i.recovered && !i.tableless)
    }

    /// PdfCraft patch (80): whether this is an indexed colour space with no lookup table (see
    /// `Indexed::new`), which only `ColorSpace::new_allowing_tableless` builds.
    pub(crate) fn is_tableless_indexed(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::Indexed(i) if i.tableless)
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
            ColorSpaceType::Plate(p) => p.inner.default_decode_arr(n),
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
            ColorSpaceType::Plate(p) => p.inner.initial_color(),
        }
    }

    pub(crate) fn is_device_gray(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::DeviceGray)
    }

    /// Whether this is `DeviceRGB`, whose 8-bit samples are their own RGB values.
    pub(crate) fn is_device_rgb(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::DeviceRgb)
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
            ColorSpaceType::Plate(p) => p.inner.num_components(),
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
    /// On a plate (patch 35) device color keeps its device space: plates separate DeviceCMYK
    /// by its components, with nonzero overprint mode, and a DefaultCMYK profile would route it
    /// through the CMYK simulation (patch 36) instead.
    on_plate: bool,
    gray: OnceCell<Option<(ColorSpace, u128)>>,
    rgb: OnceCell<Option<(ColorSpace, u128)>>,
    cmyk: OnceCell<Option<(ColorSpace, u128)>>,
}

impl<'r, 'a> DefaultColorSpaces<'r, 'a> {
    pub(crate) fn new(resources: &'r Resources<'a>, cache: &Cache, on_plate: bool) -> Self {
        Self {
            resources,
            cache: cache.clone(),
            on_plate,
            gray: OnceCell::new(),
            rgb: OnceCell::new(),
            cmyk: OnceCell::new(),
        }
    }

    /// The default for `family`, with the cache key of the object it was read from. `None`
    /// when the resources have none, or theirs is not a colour space that can stand in for
    /// the device space, or on a plate: device colour is then drawn as it is.
    pub(crate) fn get(&self, family: DeviceFamily) -> Option<(ColorSpace, u128)> {
        if self.on_plate {
            return None;
        }
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
            ColorSpaceType::Plate(i) => i.convert_f32(input, output, manual_scale),
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
            ColorSpaceType::Plate(i) => i.supports_u8(),
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
            ColorSpaceType::Plate(i) => i.convert_u8(input, output),
        }
    }

    fn is_none(&self) -> bool {
        match self.0.as_ref() {
            ColorSpaceType::Separation(s) => s.is_none(),
            ColorSpaceType::DeviceN(d) => d.is_none(),
            ColorSpaceType::Plate(p) => p.inner.is_none(),
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
        Self::adapt(white_point, &CalRgb::D65_WHITEPOINT, xyz)
    }

    /// `xyz`, seen under `white_point`, Bradford-adapted to `destination`.
    fn adapt(white_point: &[f32; 3], destination: &[f32; 3], xyz: &[f32; 3]) -> [f32; 3] {
        let source_lms = CalRgb::matrix_product(&CalRgb::BRADFORD_SCALE_MATRIX, white_point);
        // A malformed but positive XYZ white can still yield a zero or
        // overflowing cone response. Leave it unadapted rather than divide by
        // zero or propagate a non-finite scale.
        if source_lms.iter().any(|v| !v.is_finite() || v.abs() <= f32::EPSILON) {
            return *xyz;
        }
        let destination_lms = CalRgb::matrix_product(&CalRgb::BRADFORD_SCALE_MATRIX, destination);
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

    /// PdfCraft patch: the L*, a* and b* of the colour `input`, clamped to L* 0..100 and the
    /// a*/b* /Range. `manual_scale` means the components arrive normalised to [0, 1] (an
    /// `/Indexed` palette over Lab looks its bytes up as `byte / 255`): they map linearly onto
    /// L* 0..100 and the /Range (ISO 32000-2 §8.6.6.3).
    fn components(&self, input: &[f32], manual_scale: bool) -> [f32; 3] {
        let [min_a, max_a, min_b, max_b] = self.range;
        let get = |k: usize| input.get(k).copied().unwrap_or(0.0);
        let (l, a, b) = if manual_scale {
            (
                clamp_component(get(0), 0.0, 1.0) * 100.0,
                min_a + clamp_component(get(1), 0.0, 1.0) * (max_a - min_a),
                min_b + clamp_component(get(2), 0.0, 1.0) * (max_b - min_b),
            )
        } else {
            (get(0), get(1), get(2))
        };
        [
            clamp_component(l, 0.0, 100.0),
            clamp_component(a, min_a, max_a),
            clamp_component(b, min_b, max_b),
        ]
    }

    /// The XYZ of L*a*b*, in the coordinates of the space's white point.
    fn xyz(&self, [l, a, b]: [f32; 3]) -> [f32; 3] {
        let [xw, yw, zw] = self.usable_white_point();
        let fy = (l + 16.0) / 116.0;
        let fx = fy + a / 500.0;
        let fz = fy - b / 200.0;
        [
            xw * Self::inverse_lab_f(fx),
            yw * Self::inverse_lab_f(fy),
            zw * Self::inverse_lab_f(fz),
        ]
    }

    /// PdfCraft patch (36): the color `input` as XYZ relative to [`PCS_WHITE`], relative
    /// colorimetric: the space's white point becomes the PCS white (Bradford-adapted).
    fn to_pcs(&self, input: &[f32], manual_scale: bool) -> [f32; 3] {
        let xyz = self.xyz(self.components(input, manual_scale));
        Self::adapt(&self.usable_white_point(), &PCS_WHITE, &xyz)
    }
}

impl ToRgb for Lab {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], manual_scale: bool) -> Option<()> {
        let white_point = self.usable_white_point();

        for (input, output) in input.chunks_exact(3).zip(output.chunks_exact_mut(3)) {
            // L*a*b* is relative to the space's own /WhitePoint. Map it to XYZ
            // in that white's coordinates, Bradford-adapt to D65 and convert to
            // sRGB (ISO 32000-2 §8.6.5.4).
            let xyz = self.xyz(self.components(input, manual_scale));

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
    /// `hival + 1` entries of the base's component count each; entries from `present` on are
    /// zeros and are drawn black (see `convert_f32`).
    values: Vec<Vec<f32>>,
    hival: u8,
    base: Box<ColorSpace>,
    /// PdfCraft patch (80): how many entries the lookup table holds in full (at most
    /// `hival + 1`). An index at or past it is drawn black.
    present: usize,
    /// PdfCraft patch (80): the space broke ISO 32000-2 §8.6.6.3 and was recovered (see
    /// `Indexed::new`).
    recovered: bool,
    /// PdfCraft patch (80): the space has no lookup table at all (see `Indexed::new`).
    tableless: bool,
}

impl Indexed {
    /// PdfCraft patch (80): ISO 32000-2 §8.6.6.3 says `hival` "shall be an integer" no greater
    /// than 255 and the lookup table "shall be m × (hival + 1) bytes long", but not how a reader
    /// recovers when a file breaks this. Most readers draw such a space, and so does this:
    /// - a real `hival` is truncated toward zero (upstream read it so too);
    /// - a `hival` past 255 is taken as 255;
    /// - a lookup table shorter than m × (hival + 1) bytes keeps the entries it holds in full,
    ///   and an index past them is drawn black, after conversion, whatever the base space (zero
    ///   bytes would be white in DeviceCMYK, Separation and DeviceN, and blue in Lab).
    ///
    /// Any of these sets `recovered`, which the drawing paths report. A `hival` below zero (once
    /// truncated), or not a number, has no reading: the space cannot be read, as upstream, and
    /// `has_unusable_hival` lets the image path report it.
    ///
    /// A lookup that is missing, or neither a string nor a stream that decodes, is no table at
    /// all. Upstream could not read such a space either. With `allow_tableless` it is built,
    /// marked `tableless`, so that the image path can skip the image with a warning and the
    /// colour operators can fall back as upstream; without it, it is `None` as upstream.
    fn new(array: &Array<'_>, cache: &Cache, depth: u32, allow_tableless: bool) -> Option<Self> {
        let mut iter = array.flex_iter();
        // Skip name
        let _ = iter.next::<Name<'_>>()?;
        let base_color_space = ColorSpace::within(iter.next::<Object<'_>>()?, cache, depth + 1)?;
        let (hival, hival_recovered) = indexed_hival(iter.next::<object::Number>()?)?;

        // At most 255 components (a `u8`), and at most 256 entries.
        let num_components = usize::from(base_color_space.num_components());
        let entries = usize::from(hival) + 1;

        // PdfCraft patch (80): read once and then by type, so a string lookup given by an
        // indirect reference is found (a failed stream read had already consumed the
        // reference).
        let data = iter
            .next::<Object<'_>>()
            .and_then(|o| indexed_lookup(o, cache));
        let tableless = data.is_none();
        if tableless && !allow_tableless {
            return None;
        }
        let data: &[u8] = data.as_deref().unwrap_or_default();
        let present = data
            .len()
            .checked_div(num_components)
            .unwrap_or(0)
            .min(entries);
        let mut bytes = data.iter().copied();
        let values: Vec<Vec<f32>> = (0..entries)
            .map(|entry| {
                (0..num_components)
                    .map(|_| {
                        if entry < present {
                            f32::from(bytes.next().unwrap_or(0)) / 255.0
                        } else {
                            0.0
                        }
                    })
                    .collect()
            })
            .collect();

        Some(Self {
            values,
            hival,
            base: Box::new(base_color_space),
            present,
            recovered: hival_recovered || present < entries,
            tableless,
        })
    }
}

/// PdfCraft patch (80): an Indexed space's `hival` (ISO 32000-2 §8.6.6.3) as drawn, and whether
/// it had to be recovered: a real is truncated toward zero and a value past 255 is taken as 255.
/// `None` for a value below zero once truncated, or one that is not finite.
fn indexed_hival(number: object::Number) -> Option<(u8, bool)> {
    let value = number.as_f64();
    let whole = value.trunc();
    // NaN fails this test too.
    if !(whole >= 0.0 && whole.is_finite()) {
        return None;
    }
    let clamped = whole.min(f64::from(u8::MAX));
    // `clamped` is a whole number in 0..=255, so the cast is exact.
    Some((clamped as u8, clamped != value))
}

/// PdfCraft patch (80): whether `object` is an `/Indexed` colour space array whose `hival` has no
/// reading (below zero, or not a number), which `Indexed::new` cannot recover. Reads the array's
/// first and third entries only; the lookup table is not touched.
pub(crate) fn has_unusable_hival(object: &Object<'_>) -> bool {
    let Object::Array(array) = object else {
        return false;
    };
    let mut iter = array.flex_iter();
    if !iter
        .next::<Name<'_>>()
        .is_some_and(|family| matches!(family.deref(), INDEXED | I))
    {
        return false;
    }
    let _base = iter.next::<Object<'_>>();
    iter.next::<object::Number>()
        .and_then(indexed_hival)
        .is_none()
}

/// PdfCraft patch (80): an Indexed space's lookup table, a byte string or a stream (ISO 32000-2
/// §8.6.6.3), with a stream's filters applied. `None` for any other object or a stream that
/// cannot be decoded. A stream that decodes to nothing although it holds data counts as one that
/// cannot be decoded: the Flate decoder recovers what it can from a broken stream, and here
/// recovered nothing. Each call is counted on `cache` (`Cache::indexed_lookup_reads`).
fn indexed_lookup<'a>(lookup: Object<'a>, cache: &Cache) -> Option<Cow<'a, [u8]>> {
    cache.count_indexed_lookup_read();
    match lookup {
        Object::Stream(s) => s
            .decoded()
            .ok()
            .filter(|d| !d.is_empty() || s.raw_data().is_empty()),
        Object::String(s) => Some(Cow::Owned(s.to_vec())),
        _ => None,
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

        self.base.convert_f32(&indexed, output, true)?;

        // PdfCraft patch (80): an index past the entries the table holds is drawn black, in
        // whatever space the base is (see `Indexed::new`).
        if self.present <= usize::from(self.hival) {
            for (input, rgb) in input.iter().copied().zip(output.chunks_exact_mut(3)) {
                let idx = (input.clamp(0.0, self.hival as f32) + 0.5) as usize;
                if idx >= self.present {
                    rgb.fill(0);
                }
            }
        }

        Some(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Separation {
    alternate_space: ColorSpace,
    /// PdfCraft patch (55): `None` when the tint transform cannot be read (see
    /// `eval_tint_transform`).
    tint_transform: Option<Function>,
    is_none_separation: bool,
    /// PdfCraft patch (35): the colorant the space paints, for separations.
    colorant: Colorant,
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
    fn new(array: &Array<'_>, cache: &Cache, depth: u32) -> Option<Self> {
        let mut iter = array.flex_iter();
        // Skip `/Separation`
        let _ = iter.next::<Name<'_>>()?;
        let name = iter.next::<Name<'_>>()?;
        let alternate_space = ColorSpace::within(iter.next::<Object<'_>>()?, cache, depth + 1)?;
        // PdfCraft patch (55): an unreadable tint transform keeps the space (see
        // `eval_tint_transform`).
        let tint_transform = iter.next::<Object<'_>>().and_then(|f| Function::new(&f));
        let is_none_separation = name.as_str() == "None";
        let colorant = Colorant::from_name(name.deref());
        // PdfCraft patch (66): `All` names every colourant of the output device at once
        // (ISO 32000-2 §8.6.6.4), so its tints never go through the alternate space.
        let is_all = name.as_str() == "All";

        Some(Self {
            alternate_space,
            tint_transform,
            is_none_separation,
            colorant,
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
    /// PdfCraft patch (35): the colorant of each component, for separations.
    colorants: Vec<Colorant>,
}

impl DeviceN {
    fn new(array: &Array<'_>, cache: &Cache, depth: u32) -> Option<Self> {
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
        let alternate_space = ColorSpace::within(iter.next::<Object<'_>>()?, cache, depth + 1)?;
        // PdfCraft patch (55): an unreadable tint transform keeps the space (see
        // `eval_tint_transform`).
        let tint_transform = iter.next::<Object<'_>>().and_then(|f| Function::new(&f));

        if num_components == 0 {
            return None;
        }

        // `All` names every colorant only in a Separation (ISO 32000-2 §8.6.6.5), so a DeviceN
        // component named `All` paints nothing in particular.
        let colorants = names
            .iter()
            .map(|n| match Colorant::from_name(n.deref()) {
                Colorant::All => Colorant::None,
                c => c,
            })
            .collect();

        Some(Self {
            alternate_space,
            num_components,
            tint_transform,
            is_none: all_none,
            colorants,
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
    /// PdfCraft patch (36): a digest of the profile's bytes, to tell a colour in a
    /// [`CmykSimulation`]'s own profile from one in another.
    digest: u128,
    /// PdfCraft patch (36): the transform into the separation hub (see [`Hub`]), made on first
    /// use; `None` when the profile doesn't convert to it.
    to_hub: OnceLock<Option<Arc<TransformF32Executor>>>,
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

        Self::new_from_src_profile(
            src_profile,
            is_srgb,
            is_lab,
            number_components,
            hash128(profile),
        )
    }

    fn new_from_src_profile(
        src_profile: ColorProfile,
        is_srgb: bool,
        is_lab: bool,
        number_components: usize,
        digest: u128,
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
            digest,
            to_hub: OnceLock::new(),
        })))
    }

    /// PdfCraft patch (36): `input` (whole colors of this profile's components) as XYZ
    /// relative to [`PCS_WHITE`], three values per color in `xyz`, through the separation hub
    /// (see [`Hub`]). L*a*b* components are scaled to the profile's encoding unless
    /// `manual_scale` says they already are (an Indexed palette's).
    fn to_pcs(&self, input: &[f32], manual_scale: bool, xyz: &mut [f32]) -> Option<()> {
        let transform = self
            .0
            .to_hub
            .get_or_init(|| {
                transform_f32(
                    &self.0.src_profile,
                    self.0.src_layout,
                    &HUB.profile,
                    Layout::Rgb,
                )
                .map(|(transform, _)| transform)
            })
            .as_ref()?;
        let n = self.0.number_components;
        let samples = input.len() / n.max(1);
        if samples > SIMULATION_CHUNK || xyz.len() != samples.checked_mul(3)? {
            return None;
        }
        // Values outside the encoding (or NaN) never reach the tables.
        let mut encoded = [0.0_f32; SIMULATION_CHUNK * 4];
        let encoded = encoded.get_mut(..samples.checked_mul(n)?)?;
        for (e, (k, v)) in encoded.iter_mut().zip(input.iter().enumerate()) {
            *e = match (self.is_lab() && !manual_scale, k % 3) {
                (true, 0) => unit(*v / 100.0),
                (true, _) => unit((*v + 128.0) / 255.0),
                (false, _) => unit(*v),
            };
        }
        transform.transform(encoded, xyz).ok()?;
        for color in xyz.chunks_exact_mut(3) {
            let pcs = HUB.to_pcs(&[color[0], color[1], color[2]]);
            color.copy_from_slice(&pcs);
        }
        Some(())
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
    /// PdfCraft patch (46): the same color, fully opaque (see `Pattern::without_opacity`). A
    /// fully transparent color stays so: on a plate it is the paint that leaves the plate as it
    /// is (`for_plate`), and making it opaque would knock the plate out.
    pub(crate) fn opaque(mut self) -> Self {
        if self.opacity != 0.0 {
            self.opacity = 1.0;
        }
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
    /// (§10.4.2.2). PdfCraft patch (104): an additive colour (`DeviceRGB`, `CalRGB`, `Lab`, a
    /// three-component ICCBased colour) is the ink that draws back to it through the CMYK
    /// profile. `None` for every other space, ICCBased CMYK included (it keeps its own
    /// profile), and the caller draws it as before.
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
            ColorSpaceType::DeviceRgb | ColorSpaceType::CalRgb(_) | ColorSpaceType::Lab(_) => {
                self.ink_from_srgb()?
            }
            ColorSpaceType::ICCBased(icc) if icc.0.number_components == 3 => {
                self.ink_from_srgb()?
            }
            _ => return None,
        };
        Some((tints, self.opacity.clamp(0.0, 1.0)))
    }

    fn ink_from_srgb(&self) -> Option<[f32; 4]> {
        // `to_rgba` turns a non-finite component into some finite colour, which is not the
        // colour's ink, so such a colour has no tints.
        if self.components.iter().any(|c| !c.is_finite()) {
            return None;
        }
        let [r, g, b, _] = self.to_rgba().components();
        crate::cmyk_inverse::srgb_to_cmyk([r, g, b])
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

    /// PdfCraft patch (35): the gray film of `tint` on a plate (see [`Plate`]).
    pub(crate) fn film(tint: f32, opacity: f32) -> Self {
        Self::new(
            ColorSpace::device_gray(),
            smallvec![1.0 - unit(tint)],
            opacity,
        )
    }

    /// PdfCraft patch (35): this colour as drawn on `plate`. `overprint` is the graphics
    /// state's overprint parameter for this paint (`OP` for strokes, `op` otherwise) and
    /// `overprint_mode` its `OPM` (ISO 32000-2 §8.6.7). A paint that leaves the plate as it is
    /// becomes fully transparent.
    pub(crate) fn for_plate(&self, plate: &Plate, overprint: bool, overprint_mode: u8) -> Self {
        let overprint = plate.overprint && overprint;
        let untouched = Self::film(0.0, 0.0);
        match self.color_space.ink_use(&plate.ink, plate.spots_as_process) {
            InkUse::Nothing => untouched,
            InkUse::Unspecified if overprint => untouched,
            InkUse::Unspecified => Self::film(0.0, self.opacity),
            InkUse::Paints => {
                let tint = self.color_space.ink_tint(
                    &self.components,
                    &plate.ink,
                    plate.simulation.as_ref(),
                    false,
                    plate.spots_as_process,
                );
                // Nonzero overprint mode: a DeviceCMYK component of 0 leaves its plate as it
                // is. It applies to colours set in DeviceCMYK itself, not to images, shadings
                // or colours converted from another space.
                if overprint
                    && overprint_mode == 1
                    && self.color_space.is_device_cmyk()
                    && tint <= 0.0
                {
                    untouched
                } else {
                    Self::film(tint, self.opacity)
                }
            }
        }
    }
}

/// PdfCraft patch (35): a printing ink, the unit of a separation: one of the four process
/// colorants or a spot colorant named by a `Separation` or `DeviceN` colour space (ISO 32000-2
/// §8.6.6.4).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ink {
    /// Process cyan.
    Cyan,
    /// Process magenta.
    Magenta,
    /// Process yellow.
    Yellow,
    /// Process black.
    Black,
    /// A spot colorant, by its name in the colour space.
    Spot(Arc<str>),
}

impl Ink {
    /// The four process inks, in plate order.
    pub const PROCESS: [Ink; 4] = [Ink::Cyan, Ink::Magenta, Ink::Yellow, Ink::Black];

    /// The ink's colorant name, as a `Separation` colour space spells it.
    pub fn name(&self) -> &str {
        match self {
            Self::Cyan => "Cyan",
            Self::Magenta => "Magenta",
            Self::Yellow => "Yellow",
            Self::Black => "Black",
            Self::Spot(name) => name,
        }
    }

    /// The ink's position in a CMYK colour, for a process ink.
    pub(crate) fn process_index(&self) -> Option<usize> {
        match self {
            Self::Cyan => Some(0),
            Self::Magenta => Some(1),
            Self::Yellow => Some(2),
            Self::Black => Some(3),
            Self::Spot(_) => None,
        }
    }
}

/// PdfCraft patch (35): what a `Separation` or `DeviceN` colorant name paints (ISO 32000-2
/// §8.6.6.4): an ink (the process names are the process inks), every ink (`All`), or nothing
/// (`None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Colorant {
    Ink(Ink),
    All,
    None,
}

impl Colorant {
    fn from_name(name: &[u8]) -> Self {
        match name {
            b"Cyan" => Self::Ink(Ink::Cyan),
            b"Magenta" => Self::Ink(Ink::Magenta),
            b"Yellow" => Self::Ink(Ink::Yellow),
            b"Black" => Self::Ink(Ink::Black),
            b"All" => Self::All,
            b"None" => Self::None,
            other => Self::Ink(Ink::Spot(Arc::from(
                String::from_utf8_lossy(other).as_ref(),
            ))),
        }
    }
}

/// PdfCraft patch (35): draw one ink's separation instead of the composite page. Every paint
/// draws the ink's tint as a gray film: white where the plate carries no ink, black where it
/// carries 100%. Set through [`InterpreterSettings::plate`](crate::InterpreterSettings::plate).
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    /// The ink whose separation is drawn.
    pub ink: Ink,
    /// Honour the overprint parameters `OP`, `op` and `OPM` (ISO 32000-2 §8.6.7) as a
    /// separating device does: a paint whose colour space doesn't specify this ink leaves the
    /// plate as it is. When `false`, every paint knocks out what is under it, as on a
    /// composite device.
    pub overprint: bool,
    /// PdfCraft patch (36): the printing condition process colour converts to, from the
    /// document's output intent. `None` converts by the ISO 32000-2 §10.4.2 device formulas.
    pub simulation: Option<CmykSimulation>,
    /// Craft patch (CMYK masks): spot colorants paint the process plates through their
    /// alternate space and tint transform, as a composite CMYK device converts them, so no spot
    /// ink has a plate of its own. A DeviceCMYK soft mask's luminosity is separated this way;
    /// separations leave it `false`.
    pub spots_as_process: bool,
}

impl Plate {
    /// Craft patch (CMYK masks): the tint, `0.0` (no ink) to `1.0` (solid), that `color` puts
    /// on this plate, ignoring overprint and opacity.
    pub fn tint(&self, color: &Color) -> f32 {
        match color.color_space.ink_use(&self.ink, self.spots_as_process) {
            InkUse::Paints => color.color_space.ink_tint(
                &color.components,
                &self.ink,
                self.simulation.as_ref(),
                false,
                self.spots_as_process,
            ),
            InkUse::Unspecified | InkUse::Nothing => 0.0,
        }
    }
}

/// PdfCraft patch (35): told about each spot ink a paint uses, with the ink's appearance: the
/// sRGB colour of that ink alone at a tint in `0.0..=1.0`, through the colour space's
/// alternate space and tint transform.
pub type InkSinkFn = Arc<dyn Fn(&Ink, &dyn Fn(f32) -> [u8; 3]) + Send + Sync>;

/// PdfCraft patch (35): how a colour space paints one ink's plate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InkUse {
    /// The space specifies the ink: its colours set the plate's tint.
    Paints,
    /// The space doesn't specify the ink. Overprinting leaves the plate alone; otherwise the
    /// paint knocks it out (tint 0).
    Unspecified,
    /// The space paints nothing at all (the `None` colorant).
    Nothing,
}

/// PdfCraft patch (35): DeviceCMYK to sRGB as pages are drawn (the bundled CGATS-compatible
/// profile), to compose separations back into a preview. `cmyk` holds whole pixels of four
/// bytes and `rgb` the same number of three-byte pixels; `None` when the lengths disagree or
/// the transform fails.
pub fn cmyk_to_srgb8(cmyk: &[u8], rgb: &mut [u8]) -> Option<()> {
    if cmyk.len() % 4 != 0 || (cmyk.len() / 4).checked_mul(3)? != rgb.len() {
        return None;
    }
    CMYK_TRANSFORM.convert_u8(cmyk, rgb)
}

/// A component as a tint: clamped to `0.0..=1.0`, NaN read as no ink.
fn unit(value: f32) -> f32 {
    clamp_component(value, 0.0, 1.0)
}

/// The DeviceCMYK colour of a DeviceRGB one by ISO 32000-2 §10.4.2.4, with black generation
/// and undercolour removal both the identity (`BG(k) = UCR(k) = k`). Those two functions are
/// device dependent; the identity is the simplest choice the formulas allow.
fn rgb_to_cmyk([r, g, b]: [f32; 3]) -> [f32; 4] {
    let (c, m, y) = (1.0 - unit(r), 1.0 - unit(g), 1.0 - unit(b));
    let k = c.min(m).min(y);
    [unit(c - k), unit(m - k), unit(y - k), unit(k)]
}

/// PdfCraft patch (36): colours converted per step through a [`CmykSimulation`], so that the
/// buffers stay on the stack whatever an image's size.
const SIMULATION_CHUNK: usize = 256;

/// PdfCraft patch (36): the printing condition separations simulate, from an output intent's
/// destination profile (ISO 32000-2 §14.11.5): a CMYK ICC profile with PCS-to-device (BToA)
/// tables. Set through [`Plate::simulation`]. Process colour in other spaces converts to the
/// profile's CMYK through those tables, instead of by the device formulas of §10.4.2:
///
/// - DeviceRGB as sRGB, as pages are drawn; CalRGB and CalGray through their sRGB colours;
/// - Lab and ICCBased colours (RGB, gray, L\*a\*b\*, and CMYK in another profile) through
///   CIE XYZ relative to the PCS white ([`PCS_WHITE`]);
/// - DeviceCMYK, ICCBased CMYK in this same profile, and DeviceGray stay as they are: gray is a
///   device colour, which a CMYK device prints with black ink alone (§10.4.2.2).
///
/// Conversions are relative colorimetric, the initial rendering intent (§8.6.5.8; the
/// graphics state's `ri` isn't read). A [`CmykConversion`] converts into the profile's CMYK:
/// [`CmykSimulation::new`] makes one with moxcms, and [`CmykSimulation::with_conversion`]
/// takes the caller's.
#[derive(Clone)]
pub struct CmykSimulation(Arc<SimulationRepr>);

struct SimulationRepr {
    digest: u128,
    conversion: Box<dyn CmykConversion>,
}

/// The conversion into a [`CmykSimulation`]'s CMYK, relative colorimetric. Each call converts
/// whole colors, at most 256 of them; CMYK components are `0.0..=1.0`, four per color. A
/// method returns `None` when it can't convert, and the color then separates by the device
/// formulas.
pub trait CmykConversion: Send + Sync {
    /// sRGB colors, gamma-encoded, three components in `0.0..=1.0` per color, to CMYK.
    fn from_srgb(&self, rgb: &[f32], cmyk: &mut [f32]) -> Option<()>;

    /// CIE XYZ colors relative to [`PCS_WHITE`] (the Y of white is 1), three components per
    /// color, to CMYK. The source's white has already been adapted to the PCS white.
    fn from_pcs_xyz(&self, xyz: &[f32], cmyk: &mut [f32]) -> Option<()>;

    /// CMYK bytes, four per pixel, to sRGB bytes, three per pixel, to show what the printing
    /// condition prints.
    fn to_srgb8(&self, cmyk: &[u8], rgb: &mut [u8]) -> Option<()> {
        let _ = (cmyk, rgb);
        None
    }

    /// The profile's description, to name the printing condition.
    fn description(&self) -> Option<&str> {
        None
    }

    /// Whether the conversions use the profile's perceptual tables, because it has no
    /// colorimetric ones.
    fn perceptual(&self) -> bool {
        false
    }
}

/// The D50 white of the ICC profile connection space (ICC.1 §7.2.16), the white that
/// [`CmykConversion::from_pcs_xyz`] takes its colors relative to.
pub const PCS_WHITE: [f32; 3] = [0.9642, 1.0, 0.8249];

impl Debug for CmykSimulation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CmykSimulation")
            .field("description", &self.description())
            .field("perceptual", &self.perceptual())
            .finish_non_exhaustive()
    }
}

/// Two simulations of the same profile are the same.
impl PartialEq for CmykSimulation {
    fn eq(&self, other: &Self) -> bool {
        self.0.digest == other.0.digest
    }
}

/// PdfCraft patch (36): why an ICC profile can't be simulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoSimulation {
    /// The profile doesn't parse.
    Unreadable,
    /// Its data colour space isn't CMYK.
    NotCmyk,
    /// It has no PCS-to-device (BToA) tables, or they don't convert.
    NoPcsToDevice,
}

impl CmykSimulation {
    /// The printing condition of the ICC profile `profile`, if it is a CMYK profile whose
    /// PCS-to-device tables convert, converted to with moxcms. A profile without colorimetric
    /// tables is used through its perceptual ones (tag 0, which every LUT-based ICC.1 profile
    /// has). Black point compensation isn't applied.
    pub fn new(profile: &[u8]) -> Result<Self, NoSimulation> {
        Ok(Self::with_conversion(
            profile,
            MoxcmsConversion::new(profile)?,
        ))
    }

    /// The printing condition of the CMYK ICC profile `profile`, converted to by `conversion`,
    /// which the caller made from that profile. ICCBased CMYK colors in `profile` itself stay
    /// as they are.
    pub fn with_conversion(profile: &[u8], conversion: impl CmykConversion + 'static) -> Self {
        Self(Arc::new(SimulationRepr {
            digest: hash128(profile),
            conversion: Box::new(conversion),
        }))
    }

    /// The profile's description; `None` when it has none.
    pub fn description(&self) -> Option<&str> {
        self.0.conversion.description()
    }

    /// Whether conversions use the profile's perceptual tables, because it has no
    /// colorimetric ones.
    pub fn perceptual(&self) -> bool {
        self.0.conversion.perceptual()
    }

    /// A digest of the profile's bytes, for cache keys.
    pub(crate) fn digest(&self) -> u128 {
        self.0.digest
    }

    fn conversion(&self) -> &dyn CmykConversion {
        self.0.conversion.as_ref()
    }

    /// CMYK in this printing condition to sRGB, relative colorimetric, to show what it prints.
    /// As [`cmyk_to_srgb8`]; also `None` when the conversion can't show CMYK.
    pub fn cmyk_to_srgb8(&self, cmyk: &[u8], rgb: &mut [u8]) -> Option<()> {
        if cmyk.len() % 4 != 0 || (cmyk.len() / 4).checked_mul(3)? != rgb.len() {
            return None;
        }
        self.conversion().to_srgb8(cmyk, rgb)
    }
}

/// PdfCraft patch (36): the conversion [`CmykSimulation::new`] makes with moxcms. XYZ colors
/// pass through the separation hub (see [`Hub`]).
struct MoxcmsConversion {
    description: Option<String>,
    perceptual: bool,
    from_srgb: Arc<TransformF32Executor>,
    from_hub: Arc<TransformF32Executor>,
    to_srgb: Option<Arc<Transform8BitExecutor>>,
}

impl MoxcmsConversion {
    /// The longest description kept, in characters.
    const MAX_DESCRIPTION: usize = 160;

    fn new(profile: &[u8]) -> Result<Self, NoSimulation> {
        let parsed = ColorProfile::new_from_slice(profile).map_err(|_| NoSimulation::Unreadable)?;
        if parsed.color_space != DataColorSpace::Cmyk {
            return Err(NoSimulation::NotCmyk);
        }
        if parsed.lut_b_to_a_colorimetric.is_none() && parsed.lut_b_to_a_perceptual.is_none() {
            return Err(NoSimulation::NoPcsToDevice);
        }
        let (from_hub, perceptual) =
            transform_f32(&HUB.profile, Layout::Rgb, &parsed, Layout::Rgba)
                .ok_or(NoSimulation::NoPcsToDevice)?;
        let (from_srgb, _) = transform_f32(&SRGB, Layout::Rgb, &parsed, Layout::Rgba)
            .ok_or(NoSimulation::NoPcsToDevice)?;
        let to_srgb = transform_u8(&parsed, Layout::Rgba, &SRGB, Layout::Rgb);
        Ok(Self {
            description: profile_description(&parsed, Self::MAX_DESCRIPTION),
            perceptual,
            from_srgb,
            from_hub,
            to_srgb,
        })
    }
}

impl CmykConversion for MoxcmsConversion {
    fn from_srgb(&self, rgb: &[f32], cmyk: &mut [f32]) -> Option<()> {
        self.from_srgb.transform(rgb, cmyk).ok()
    }

    fn from_pcs_xyz(&self, xyz: &[f32], cmyk: &mut [f32]) -> Option<()> {
        let mut rgb = [0.0_f32; SIMULATION_CHUNK * 3];
        let rgb = rgb.get_mut(..xyz.len())?;
        for (out, xyz) in rgb.chunks_exact_mut(3).zip(xyz.chunks_exact(3)) {
            out.copy_from_slice(&HUB.from_pcs(&[xyz[0], xyz[1], xyz[2]]));
        }
        self.from_hub.transform(rgb, cmyk).ok()
    }

    fn to_srgb8(&self, cmyk: &[u8], rgb: &mut [u8]) -> Option<()> {
        self.to_srgb.as_ref()?.transform(cmyk, rgb).ok()
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn perceptual(&self) -> bool {
        self.perceptual
    }
}

/// PdfCraft patch (36): a profile's description, without control characters and at most `max`
/// characters long; `None` when it has none.
fn profile_description(profile: &ColorProfile, max: usize) -> Option<String> {
    let text = match profile.description.as_ref()? {
        ProfileText::PlainString(s) => s.as_str(),
        ProfileText::Localizable(list) => {
            &list
                .iter()
                .find(|s| s.language == "en")
                .or(list.first())?
                .value
        }
        ProfileText::Description(d) if d.ascii_string.trim().is_empty() => &d.unicode_string,
        ProfileText::Description(d) => &d.ascii_string,
    };
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

/// PdfCraft patch (36): a transform from `source` to `destination`, relative colorimetric, or
/// through the perceptual tables (`true`) where a profile lacks colorimetric ones.
fn transform_f32(
    source: &ColorProfile,
    source_layout: Layout,
    destination: &ColorProfile,
    destination_layout: Layout,
) -> Option<(Arc<TransformF32Executor>, bool)> {
    [
        RenderingIntent::RelativeColorimetric,
        RenderingIntent::Perceptual,
    ]
    .into_iter()
    .find_map(|rendering_intent| {
        let options = TransformOptions {
            rendering_intent,
            ..TransformOptions::default()
        };
        source
            .create_transform_f32(source_layout, destination, destination_layout, options)
            .ok()
            .map(|t| (t, rendering_intent == RenderingIntent::Perceptual))
    })
}

/// PdfCraft patch (36): as [`transform_f32`], for bytes.
fn transform_u8(
    source: &ColorProfile,
    source_layout: Layout,
    destination: &ColorProfile,
    destination_layout: Layout,
) -> Option<Arc<Transform8BitExecutor>> {
    [
        RenderingIntent::RelativeColorimetric,
        RenderingIntent::Perceptual,
    ]
    .into_iter()
    .find_map(|rendering_intent| {
        let options = TransformOptions {
            rendering_intent,
            ..TransformOptions::default()
        };
        source
            .create_transform_8bit(source_layout, destination, destination_layout, options)
            .ok()
    })
}

static SRGB: LazyLock<ColorProfile> = LazyLock::new(ColorProfile::new_srgb);

/// PdfCraft patch (36): the separation hub, the RGB that ICCBased colors leave their profile
/// in, and that [`MoxcmsConversion`] takes PCS XYZ to its CMYK through: ROMM RGB (ProPhoto)
/// primaries, D50 white, gamma 1.8. Its
/// gamut holds practically every surface colour, so nothing printable clips on the way, and
/// paper white and black land on exact grid points of the conversion's tables. (moxcms
/// converts CMYK to CMYK and gray to CMYK only through an intermediate.)
struct Hub {
    profile: ColorProfile,
    /// Linear hub RGB to XYZ, row-major.
    to_xyz: [f32; 9],
    /// XYZ to linear hub RGB, row-major.
    from_xyz: [f32; 9],
    /// The XYZ of hub white, `(1, 1, 1)`.
    white: [f32; 3],
}

impl Hub {
    const GAMMA: f32 = 1.8;

    fn new() -> Self {
        let profile = ColorProfile::new_pro_photo_rgb();
        let to_xyz = profile.rgb_to_xyz_matrix();
        let from_xyz = to_xyz.inverse().v;
        let white = to_xyz.v.map(|row| (row[0] + row[1] + row[2]) as f32);
        let flat = |m: [[f64; 3]; 3]| {
            let mut out = [0.0_f32; 9];
            for (out, v) in out.iter_mut().zip(m.iter().flatten()) {
                *out = *v as f32;
            }
            out
        };
        Self {
            profile,
            to_xyz: flat(to_xyz.v),
            from_xyz: flat(from_xyz),
            white,
        }
    }

    /// XYZ relative to [`PCS_WHITE`] as hub RGB, clipped to its gamut. The PCS white is scaled
    /// onto the hub's own D50 white, so that paper white lands on hub white exactly.
    fn from_pcs(&self, xyz: &[f32; 3]) -> [f32; 3] {
        let xyz = [0, 1, 2].map(|k| xyz[k] * self.white[k] / PCS_WHITE[k]);
        CalRgb::matrix_product(&self.from_xyz, &xyz).map(|v| unit(v).powf(1.0 / Self::GAMMA))
    }

    /// Hub RGB as XYZ relative to [`PCS_WHITE`]; the inverse of [`Hub::from_pcs`] in gamut.
    fn to_pcs(&self, rgb: &[f32; 3]) -> [f32; 3] {
        let linear = rgb.map(|v| unit(v).powf(Self::GAMMA));
        let xyz = CalRgb::matrix_product(&self.to_xyz, &linear);
        [0, 1, 2].map(|k| xyz[k] * PCS_WHITE[k] / self.white[k])
    }
}

static HUB: LazyLock<Hub> = LazyLock::new(Hub::new);

impl ColorSpace {
    /// PdfCraft patch (35): this space drawn as `plate` (for images and shadings, whose
    /// colours are converted sample by sample), converting process colour through its
    /// `simulation` (patch 36).
    pub(crate) fn plate(&self, plate: &Plate) -> Self {
        Self(Arc::new(ColorSpaceType::Plate(PlateSpace {
            inner: self.clone(),
            ink: plate.ink.clone(),
            simulation: plate.simulation.clone(),
            spots_as_process: plate.spots_as_process,
        })))
    }

    /// PdfCraft patch (36): whether `simulation` converts this space's colours (see
    /// [`CmykSimulation`]).
    fn simulated_by(&self, simulation: &CmykSimulation) -> bool {
        match self.0.as_ref() {
            ColorSpaceType::DeviceRgb
            | ColorSpaceType::CalRgb(_)
            | ColorSpaceType::CalGray(_)
            | ColorSpaceType::Lab(_) => true,
            ColorSpaceType::ICCBased(icc) => {
                icc.0.number_components != 4 || icc.0.digest != simulation.0.digest
            }
            _ => false,
        }
    }

    /// PdfCraft patch (36): the colours `input` (whole colours of this space's components;
    /// `manual_scale` as for [`ToRgb::convert_f32`]) in `simulation`'s CMYK, four values per
    /// colour in `cmyk`. `None` when the space isn't [`simulated_by`](Self::simulated_by)
    /// `simulation`, the lengths disagree, or a conversion fails.
    fn simulate_cmyk(
        &self,
        simulation: &CmykSimulation,
        input: &[f32],
        manual_scale: bool,
        cmyk: &mut [f32],
    ) -> Option<()> {
        let n = usize::from(self.num_components()).max(1);
        let colours = input.len() / n;
        if cmyk.len() != colours.checked_mul(4)? {
            return None;
        }
        let conversion = simulation.conversion();
        // sRGB or PCS XYZ, by the space.
        let mut rgb = [0.0_f32; SIMULATION_CHUNK * 3];
        let mut srgb = [0_u8; SIMULATION_CHUNK * 3];
        for (input, cmyk) in input
            .chunks(SIMULATION_CHUNK * n)
            .zip(cmyk.chunks_mut(SIMULATION_CHUNK * 4))
        {
            let count = cmyk.len() / 4;
            let input = input.get(..count.checked_mul(n)?)?;
            let rgb = rgb.get_mut(..count * 3)?;
            match self.0.as_ref() {
                ColorSpaceType::DeviceRgb => {
                    for (out, v) in rgb.iter_mut().zip(input) {
                        *out = unit(*v);
                    }
                    conversion.from_srgb(rgb, cmyk)?;
                }
                ColorSpaceType::CalRgb(_) | ColorSpaceType::CalGray(_) => {
                    let srgb = srgb.get_mut(..count * 3)?;
                    self.convert_f32(input, srgb, manual_scale)?;
                    for (out, v) in rgb.iter_mut().zip(srgb.iter()) {
                        *out = u8_to_f32(*v);
                    }
                    conversion.from_srgb(rgb, cmyk)?;
                }
                ColorSpaceType::Lab(lab) => {
                    for (out, colour) in rgb.chunks_exact_mut(3).zip(input.chunks_exact(3)) {
                        out.copy_from_slice(&lab.to_pcs(colour, manual_scale));
                    }
                    conversion.from_pcs_xyz(rgb, cmyk)?;
                }
                ColorSpaceType::ICCBased(icc) if self.simulated_by(simulation) => {
                    icc.to_pcs(input, manual_scale, rgb)?;
                    conversion.from_pcs_xyz(rgb, cmyk)?;
                }
                _ => return None,
            }
        }
        Some(())
    }

    /// PdfCraft patch (35): DeviceCMYK itself (`CalCMYK` reads as it too), where the nonzero
    /// overprint mode applies. Craft patch (flatten): public.
    pub fn is_device_cmyk(&self) -> bool {
        matches!(self.0.as_ref(), ColorSpaceType::DeviceCmyk)
    }

    /// PdfCraft patch (35): spaces whose colours are converted to sRGB before separation
    /// (DeviceRGB is separated from its components directly, without a conversion).
    fn separates_through_srgb(&self) -> bool {
        match self.0.as_ref() {
            ColorSpaceType::CalRgb(_) | ColorSpaceType::Lab(_) => true,
            ColorSpaceType::ICCBased(icc) => icc.0.number_components == 3,
            _ => false,
        }
    }

    /// Craft patch (CMYK masks): the alternate space of a `Separation` or `DeviceN` space that
    /// names a spot ink; `None` for other spaces, and for colorants that are all process inks,
    /// `All` or `None`.
    fn spot_alternate_space(&self) -> Option<&ColorSpace> {
        let is_spot = |c: &Colorant| matches!(c, Colorant::Ink(Ink::Spot(_)));
        match self.0.as_ref() {
            ColorSpaceType::Separation(s) if is_spot(&s.colorant) => Some(&s.alternate_space),
            ColorSpaceType::DeviceN(d) if !d.is_none && d.colorants.iter().any(is_spot) => {
                Some(&d.alternate_space)
            }
            _ => None,
        }
    }

    /// Craft patch (CMYK masks): a `Separation` or `DeviceN` color in its alternate space,
    /// through the tint transform; the components unchanged for other spaces.
    fn tint_transform(&self, components: &[f32]) -> ColorComponents {
        let (function, alternate) = match self.0.as_ref() {
            ColorSpaceType::Separation(s) => (&s.tint_transform, &s.alternate_space),
            ColorSpaceType::DeviceN(d) => (&d.tint_transform, &d.alternate_space),
            _ => return components.iter().copied().collect(),
        };
        count_tint_transform_runs(1);
        function
            .as_ref()
            .and_then(|f| f.eval(components.iter().copied().collect()))
            .unwrap_or(alternate.initial_color())
    }

    /// PdfCraft patch (35): how the space paints `ink`'s plate. Gray, RGB, CIE-based and ICC
    /// spaces are process colour: they specify the four process inks and no spot ink. With
    /// `spots_as_process` (Craft patch, CMYK masks), a space naming a spot ink paints as its
    /// alternate space.
    pub(crate) fn ink_use(&self, ink: &Ink, spots_as_process: bool) -> InkUse {
        if spots_as_process && let Some(alternate) = self.spot_alternate_space() {
            return alternate.ink_use(ink, false);
        }
        match self.0.as_ref() {
            ColorSpaceType::DeviceCmyk
            | ColorSpaceType::DeviceGray
            | ColorSpaceType::DeviceRgb
            | ColorSpaceType::ICCBased(_)
            | ColorSpaceType::CalGray(_)
            | ColorSpaceType::CalRgb(_)
            | ColorSpaceType::Lab(_) => {
                if ink.process_index().is_some() {
                    InkUse::Paints
                } else {
                    InkUse::Unspecified
                }
            }
            ColorSpaceType::Indexed(i) => i.base.ink_use(ink, spots_as_process),
            ColorSpaceType::Separation(s) => match &s.colorant {
                Colorant::None => InkUse::Nothing,
                Colorant::All => InkUse::Paints,
                Colorant::Ink(i) if i == ink => InkUse::Paints,
                Colorant::Ink(_) => InkUse::Unspecified,
            },
            ColorSpaceType::DeviceN(d) => {
                if d.is_none {
                    InkUse::Nothing
                } else if d
                    .colorants
                    .iter()
                    .any(|c| matches!(c, Colorant::Ink(i) if i == ink))
                {
                    InkUse::Paints
                } else {
                    InkUse::Unspecified
                }
            }
            // A pattern is separated through what it paints.
            ColorSpaceType::Pattern(_) => InkUse::Unspecified,
            ColorSpaceType::Plate(p) => p.inner.ink_use(ink, spots_as_process),
        }
    }

    /// PdfCraft patch (35): the tint, `0.0` (no ink) to `1.0` (solid), that the colour
    /// `components` puts on `ink`'s plate; `0.0` when the space doesn't specify the ink.
    /// Process colour converts through `simulation` when there is one (patch 36).
    /// `manual_scale` as for [`ToRgb::convert_f32`] (components normalised to `0.0..=1.0`, as
    /// an Indexed palette gives them). `spots_as_process` as for [`Self::ink_use`].
    pub(crate) fn ink_tint(
        &self,
        components: &[f32],
        ink: &Ink,
        simulation: Option<&CmykSimulation>,
        manual_scale: bool,
        spots_as_process: bool,
    ) -> f32 {
        if spots_as_process && let Some(alternate) = self.spot_alternate_space() {
            let alternate_components = self.tint_transform(components);
            return alternate.ink_tint(&alternate_components, ink, simulation, false, false);
        }
        let tint = match self.0.as_ref() {
            ColorSpaceType::Separation(s) => match &s.colorant {
                Colorant::All => components.first().copied().unwrap_or(0.0),
                Colorant::Ink(i) if i == ink => components.first().copied().unwrap_or(0.0),
                _ => 0.0,
            },
            ColorSpaceType::DeviceN(d) => d
                .colorants
                .iter()
                .position(|c| matches!(c, Colorant::Ink(i) if i == ink))
                .and_then(|k| components.get(k))
                .copied()
                .unwrap_or(0.0),
            ColorSpaceType::Indexed(i) => {
                let index = components.first().copied().unwrap_or(0.0);
                let index = (clamp_component(index, 0.0, f32::from(i.hival)) + 0.5) as usize;
                match i.values.get(index) {
                    Some(values) => {
                        i.base
                            .ink_tint(values, ink, simulation, true, spots_as_process)
                    }
                    None => 0.0,
                }
            }
            ColorSpaceType::Pattern(_) => 0.0,
            ColorSpaceType::Plate(p) => {
                p.inner
                    .ink_tint(components, ink, simulation, manual_scale, spots_as_process)
            }
            _ => match (
                self.process_cmyk(components, simulation, manual_scale),
                ink.process_index(),
            ) {
                (Some(cmyk), Some(k)) => cmyk.get(k).copied().unwrap_or(0.0),
                _ => 0.0,
            },
        };
        unit(tint)
    }

    /// PdfCraft patch (35): a process colour as DeviceCMYK: CMYK spaces (DeviceCMYK, ICC with
    /// four components) as they are, gray as black only (`k = 1 − gray`, §10.4.2.2), and RGB,
    /// CIE-based and three-component ICC spaces through sRGB by [`rgb_to_cmyk`]. Patch 36:
    /// the spaces `simulation` converts go through it instead, and only fall back to those
    /// formulas if the conversion fails.
    fn process_cmyk(
        &self,
        components: &[f32],
        simulation: Option<&CmykSimulation>,
        manual_scale: bool,
    ) -> Option<[f32; 4]> {
        if let Some(simulation) = simulation.filter(|s| self.simulated_by(s)) {
            let n = usize::from(self.num_components()).max(1);
            let mut colour: ColorComponents = components.iter().copied().take(n).collect();
            colour.resize(n, 0.0);
            let mut cmyk = [0.0; 4];
            if self
                .simulate_cmyk(simulation, &colour, manual_scale, &mut cmyk)
                .is_some()
            {
                return Some(cmyk.map(unit));
            }
        }
        let get = |k: usize| unit(components.get(k).copied().unwrap_or(0.0));
        match self.0.as_ref() {
            ColorSpaceType::DeviceCmyk => Some([get(0), get(1), get(2), get(3)]),
            ColorSpaceType::ICCBased(icc) if icc.0.number_components == 4 => {
                Some([get(0), get(1), get(2), get(3)])
            }
            ColorSpaceType::ICCBased(icc) if icc.0.number_components == 1 => {
                Some([0.0, 0.0, 0.0, 1.0 - get(0)])
            }
            ColorSpaceType::DeviceGray | ColorSpaceType::CalGray(_) => {
                Some([0.0, 0.0, 0.0, 1.0 - get(0)])
            }
            ColorSpaceType::DeviceRgb => Some(rgb_to_cmyk([get(0), get(1), get(2)])),
            _ if self.separates_through_srgb() => {
                let mut rgb = [0_u8; 3];
                self.convert_sample(components, &mut rgb, manual_scale)?;
                Some(rgb_to_cmyk(rgb.map(u8_to_f32)))
            }
            _ => None,
        }
    }

    /// PdfCraft patch (35): tell `sink` about every spot ink the space names, with the ink's
    /// appearance (the space's colour with only that component set).
    pub(crate) fn report_spots(&self, sink: &InkSinkFn) {
        match self.0.as_ref() {
            ColorSpaceType::Separation(s) => {
                if let Colorant::Ink(ink @ Ink::Spot(_)) = &s.colorant {
                    sink(ink, &|tint| self.appearance(&[tint]));
                }
            }
            ColorSpaceType::DeviceN(d) => {
                for (k, colorant) in d.colorants.iter().enumerate() {
                    if let Colorant::Ink(ink @ Ink::Spot(_)) = colorant {
                        sink(ink, &|tint| {
                            let mut components: ColorComponents = smallvec![0.0; d.colorants.len()];
                            if let Some(c) = components.get_mut(k) {
                                *c = tint;
                            }
                            self.appearance(&components)
                        });
                    }
                }
            }
            ColorSpaceType::Indexed(i) => i.base.report_spots(sink),
            ColorSpaceType::Pattern(cs) => cs.report_spots(sink),
            ColorSpaceType::Plate(p) => p.inner.report_spots(sink),
            _ => {}
        }
    }

    /// The sRGB colour of `components` (the `None` colorant's invisibility aside).
    fn appearance(&self, components: &[f32]) -> [u8; 3] {
        let [r, g, b, _] = self.to_rgba(components, 1.0, false).to_rgba8();
        [r, g, b]
    }
}

/// PdfCraft patch (35): a colour space drawn as one ink's plate: each colour becomes the gray
/// film of that ink's tint (see [`Plate`]).
#[derive(Debug, Clone)]
pub(crate) struct PlateSpace {
    inner: ColorSpace,
    ink: Ink,
    /// Patch 36: the printing condition process colour converts to, if any.
    simulation: Option<CmykSimulation>,
    /// Craft patch (CMYK masks): see [`Plate::spots_as_process`].
    spots_as_process: bool,
}

impl PlateSpace {
    fn samples(&self) -> usize {
        usize::from(self.inner.num_components()).max(1)
    }

    /// The film of the tint the colour `sample` puts on the plate.
    fn film(&self, sample: &[f32], manual_scale: bool) -> u8 {
        let tint = self
            .inner
            .ink_tint(
                sample,
                &self.ink,
                self.simulation.as_ref(),
                manual_scale,
                self.spots_as_process,
            );
        f32_to_u8(1.0 - tint)
    }

    /// PdfCraft patch (36): the inner space's colours `input` converted through the
    /// simulation, as film; `None` when the space isn't simulated or a conversion fails.
    fn film_through_simulation(
        &self,
        input: &[f32],
        output: &mut [u8],
        manual_scale: bool,
    ) -> Option<()> {
        let simulation = self
            .simulation
            .as_ref()
            .filter(|s| self.inner.simulated_by(s))?;
        let Some(k) = self.ink.process_index() else {
            output.fill(255);
            return Some(());
        };
        let n = self.samples();
        let mut cmyk = [0.0_f32; SIMULATION_CHUNK * 4];
        for (input, rgb) in input
            .chunks(SIMULATION_CHUNK * n)
            .zip(output.chunks_mut(SIMULATION_CHUNK * 3))
        {
            let count = (input.len() / n).min(rgb.len() / 3);
            let cmyk = cmyk.get_mut(..count * 4)?;
            self.inner
                .simulate_cmyk(simulation, input.get(..count * n)?, manual_scale, cmyk)?;
            for (px, colour) in rgb.chunks_exact_mut(3).zip(cmyk.chunks_exact(4)) {
                px.fill(f32_to_u8(1.0 - unit(colour.get(k).copied().unwrap_or(0.0))));
            }
        }
        Some(())
    }

    /// The film of every entry of an Indexed inner space's palette, worked out once for an
    /// image's `samples` rather than once per sample; `None` for other spaces, and when there
    /// are no more samples than palette entries.
    fn palette_films(&self, samples: usize) -> Option<Vec<u8>> {
        let ColorSpaceType::Indexed(indexed) = self.inner.0.as_ref() else {
            return None;
        };
        if samples <= usize::from(indexed.hival) + 1 {
            return None;
        }
        Some(
            (0..=indexed.hival)
                .map(|index| self.film(&[f32::from(index)], false))
                .collect(),
        )
    }

    /// Turn sRGB pixels, converted from the inner space, into film in place.
    fn film_from_srgb(&self, output: &mut [u8]) {
        let Some(k) = self.ink.process_index() else {
            output.fill(255);
            return;
        };
        for rgb in output.chunks_exact_mut(3) {
            let cmyk = rgb_to_cmyk([u8_to_f32(rgb[0]), u8_to_f32(rgb[1]), u8_to_f32(rgb[2])]);
            let film = f32_to_u8(1.0 - cmyk.get(k).copied().unwrap_or(0.0));
            rgb.copy_from_slice(&[film; 3]);
        }
    }
}

impl ToRgb for PlateSpace {
    fn convert_f32(&self, input: &[f32], output: &mut [u8], manual_scale: bool) -> Option<()> {
        if self
            .film_through_simulation(input, output, manual_scale)
            .is_some()
        {
            return Some(());
        }
        if self.inner.separates_through_srgb() {
            self.inner.convert_f32(input, output, manual_scale)?;
            self.film_from_srgb(output);
            return Some(());
        }
        if let Some(films) = self.palette_films(input.len()) {
            // As `ink_tint` reads an Indexed sample: the index, clamped and rounded.
            let hival = films.len().saturating_sub(1) as f32;
            for (index, rgb) in input.iter().zip(output.chunks_exact_mut(3)) {
                let index = (clamp_component(*index, 0.0, hival) + 0.5) as usize;
                rgb.fill(films.get(index).copied().unwrap_or(255));
            }
            return Some(());
        }
        for (sample, rgb) in input
            .chunks_exact(self.samples())
            .zip(output.chunks_exact_mut(3))
        {
            rgb.fill(self.film(sample, manual_scale));
        }
        Some(())
    }

    // The spaces that take bytes (device and ICC spaces) read them as component × 255.
    fn supports_u8(&self) -> bool {
        self.inner.supports_u8()
    }

    fn convert_u8(&self, input: &[u8], output: &mut [u8]) -> Option<()> {
        if self
            .simulation
            .as_ref()
            .is_some_and(|s| self.inner.simulated_by(s))
        {
            // Bytes are the components' encoding scaled to 0–255 (an ICC L*a*b* colour's too,
            // so they arrive as `manual_scale` components).
            let n = self.samples();
            let mut floats = [0.0_f32; SIMULATION_CHUNK * 4];
            let mut done = Some(());
            for (bytes, rgb) in input
                .chunks(SIMULATION_CHUNK * n)
                .zip(output.chunks_mut(SIMULATION_CHUNK * 3))
            {
                let floats = floats.get_mut(..bytes.len())?;
                for (f, b) in floats.iter_mut().zip(bytes) {
                    *f = u8_to_f32(*b);
                }
                done = done.and(self.film_through_simulation(floats, rgb, true));
            }
            if done.is_some() {
                return Some(());
            }
        }
        if self.inner.separates_through_srgb() {
            self.inner.convert_u8(input, output)?;
            self.film_from_srgb(output);
            return Some(());
        }
        let mut sample: ColorComponents = SmallVec::new();
        for (bytes, rgb) in input
            .chunks_exact(self.samples())
            .zip(output.chunks_exact_mut(3))
        {
            sample.clear();
            sample.extend(bytes.iter().copied().map(u8_to_f32));
            rgb.fill(self.film(&sample, false));
        }
        Some(())
    }

    fn is_none(&self) -> bool {
        self.inner.is_none()
    }
}

/// The CMYK profile pages are drawn with.
pub(crate) const CMYK_PROFILE: &[u8] = include_bytes!("../assets/CGATS001Compat-v2-micro.icc");

static CMYK_TRANSFORM: LazyLock<ICCProfile> =
    LazyLock::new(|| ICCProfile::new(CMYK_PROFILE, 4).unwrap());

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

/// Craft patch (flatten): the inks a colour paints, for devices that keep colours as inks
/// instead of RGB (see [`Color::ink_paint`]).
#[derive(Clone, Debug, PartialEq)]
pub enum InkPaint {
    /// Tints `0.0..=1.0` of the listed inks; other inks are unspecified. Process colour lists
    /// all four process inks.
    Tints(Vec<(Ink, f32)>),
    /// The same tint on every plate (the `All` colorant).
    All(f32),
    /// No ink (the `None` colorant).
    None,
}

/// Craft patch (flatten): how a spot ink looks at 100%, from its colour space's alternate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpotAlternate {
    /// The alternate space is DeviceCMYK: the tint transform's CMYK.
    Cmyk([f32; 4]),
    /// Any other alternate space: the sRGB appearance.
    Srgb([u8; 3]),
}

impl Color {
    /// Craft patch (flatten): the opacity (`ca` or `CA`) the colour paints with.
    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    /// Craft patch (flatten): whether the colour was set in DeviceCMYK itself, where the
    /// nonzero overprint mode applies (ISO 32000-2 §8.6.7).
    pub fn is_device_cmyk(&self) -> bool {
        self.color_space.is_device_cmyk()
    }

    /// Craft patch (flatten): the inks the colour paints, as plate mode separates it:
    /// `Separation` and `DeviceN` colorants as they are, and process colour as CMYK, converted
    /// through `simulation` where plate mode converts it. `None` for a pattern space.
    pub fn ink_paint(&self, simulation: Option<&CmykSimulation>) -> Option<InkPaint> {
        self.color_space
            .ink_paint(&self.components, simulation, false)
    }

    /// Craft patch (flatten): how spot `ink` of this colour's space looks at 100%; `None` when
    /// the space doesn't name it.
    pub fn spot_alternate(&self, ink: &Ink) -> Option<SpotAlternate> {
        self.color_space.spot_alternate(ink)
    }
}

impl ColorSpace {
    /// Craft patch (flatten): see [`Color::ink_paint`].
    fn ink_paint(
        &self,
        components: &[f32],
        simulation: Option<&CmykSimulation>,
        manual_scale: bool,
    ) -> Option<InkPaint> {
        let get = |k: usize| unit(components.get(k).copied().unwrap_or(0.0));
        Some(match self.0.as_ref() {
            ColorSpaceType::Separation(s) => match &s.colorant {
                Colorant::None => InkPaint::None,
                Colorant::All => InkPaint::All(get(0)),
                Colorant::Ink(ink) => InkPaint::Tints(vec![(ink.clone(), get(0))]),
            },
            ColorSpaceType::DeviceN(d) => {
                let tints: Vec<_> = d
                    .colorants
                    .iter()
                    .enumerate()
                    .filter_map(|(k, c)| match c {
                        Colorant::Ink(ink) => Some((ink.clone(), get(k))),
                        _ => None,
                    })
                    .collect();
                if d.is_none || tints.is_empty() {
                    InkPaint::None
                } else {
                    InkPaint::Tints(tints)
                }
            }
            ColorSpaceType::Indexed(i) => {
                let index = components.first().copied().unwrap_or(0.0);
                let index = (clamp_component(index, 0.0, f32::from(i.hival)) + 0.5) as usize;
                return i.base.ink_paint(i.values.get(index)?, simulation, true);
            }
            ColorSpaceType::Pattern(_) => return None,
            ColorSpaceType::Plate(p) => {
                return p.inner.ink_paint(components, simulation, manual_scale);
            }
            _ => {
                let cmyk = self.process_cmyk(components, simulation, manual_scale)?;
                InkPaint::Tints(Ink::PROCESS.into_iter().zip(cmyk).collect())
            }
        })
    }

    /// Craft patch (flatten): see [`Color::spot_alternate`].
    pub fn spot_alternate(&self, ink: &Ink) -> Option<SpotAlternate> {
        let (alternate, transform, components) = match self.0.as_ref() {
            ColorSpaceType::Separation(s) => {
                if s.colorant != Colorant::Ink(ink.clone()) {
                    return None;
                }
                (&s.alternate_space, &s.tint_transform, smallvec![1.0])
            }
            ColorSpaceType::DeviceN(d) => {
                let k = d
                    .colorants
                    .iter()
                    .position(|c| matches!(c, Colorant::Ink(i) if i == ink))?;
                let mut components: ColorComponents = smallvec![0.0; d.colorants.len()];
                *components.get_mut(k)? = 1.0;
                (&d.alternate_space, &d.tint_transform, components)
            }
            ColorSpaceType::Indexed(i) => return i.base.spot_alternate(ink),
            ColorSpaceType::Pattern(cs) => return cs.spot_alternate(ink),
            ColorSpaceType::Plate(p) => return p.inner.spot_alternate(ink),
            _ => return None,
        };
        if alternate.is_device_cmyk()
            && let Some(cmyk) = transform.as_ref().and_then(|f| f.eval(components.clone()))
            && let [c, m, y, k] = cmyk[..]
        {
            return Some(SpotAlternate::Cmyk([c, m, y, k].map(unit)));
        }
        Some(SpotAlternate::Srgb(self.appearance(&components)))
    }
}

/// Craft patch (flatten): the components a device that keeps colours as inks stores a space's
/// colours in, as plate mode separates them (see [`ColorSpace::ink_values`]).
#[derive(Clone, Debug, PartialEq)]
pub enum InkSpace {
    /// DeviceGray: one gray level, 1 = white.
    Gray,
    /// The four process tints. `converted` when the colours are converted into CMYK (RGB, Lab,
    /// CIE-based and ICC spaces other than a CMYK profile the simulation leaves as it is).
    Cmyk {
        /// Whether the colours are converted into CMYK.
        converted: bool,
    },
    /// Tints of these inks: `Separation` and `DeviceN` colorants, `None` colorants left out.
    Inks(Vec<Ink>),
    /// One tint for every ink (the `All` colorant).
    All,
    /// No ink (the `None` colorant, or a pattern space).
    None,
}

impl InkSpace {
    /// Values per colour.
    pub fn len(&self) -> usize {
        match self {
            Self::Gray | Self::All => 1,
            Self::Cmyk { .. } => 4,
            Self::Inks(inks) => inks.len(),
            Self::None => 0,
        }
    }

    /// Whether colours have no values (the space paints nothing).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ColorSpace {
    /// Craft patch (flatten): the components per colour.
    pub fn components(&self) -> usize {
        usize::from(self.num_components())
    }

    /// Craft patch (flatten): how [`Self::ink_values`] stores this space's colours. An Indexed
    /// space stores its base space's.
    pub fn ink_space(&self, simulation: Option<&CmykSimulation>) -> InkSpace {
        match self.0.as_ref() {
            ColorSpaceType::DeviceGray => InkSpace::Gray,
            ColorSpaceType::DeviceCmyk => InkSpace::Cmyk { converted: false },
            ColorSpaceType::ICCBased(icc) if icc.0.number_components == 4 => InkSpace::Cmyk {
                converted: simulation.is_some_and(|s| self.simulated_by(s)),
            },
            ColorSpaceType::Indexed(i) => i.base.ink_space(simulation),
            ColorSpaceType::Separation(s) => match &s.colorant {
                Colorant::Ink(ink) => InkSpace::Inks(vec![ink.clone()]),
                Colorant::All => InkSpace::All,
                Colorant::None => InkSpace::None,
            },
            ColorSpaceType::DeviceN(d) => {
                let inks: Vec<Ink> = d
                    .colorants
                    .iter()
                    .filter_map(|c| match c {
                        Colorant::Ink(ink) => Some(ink.clone()),
                        _ => None,
                    })
                    .collect();
                if d.is_none || inks.is_empty() {
                    InkSpace::None
                } else {
                    InkSpace::Inks(inks)
                }
            }
            ColorSpaceType::Pattern(_) => InkSpace::None,
            ColorSpaceType::Plate(p) => p.inner.ink_space(simulation),
            _ => InkSpace::Cmyk { converted: true },
        }
    }

    /// Craft patch (flatten): the colours `colors` (whole colours of [`Self::components`]
    /// values each) appended to `out` as [`Self::ink_space`] values, `0.0..=1.0`. Process
    /// colour converts as [`Color::ink_paint`] does, in batches.
    pub fn ink_values(&self, colors: &[f32], simulation: Option<&CmykSimulation>, out: &mut Vec<f32>) {
        self.ink_values_scaled(colors, simulation, false, out);
    }

    fn ink_values_scaled(
        &self,
        colors: &[f32],
        simulation: Option<&CmykSimulation>,
        manual_scale: bool,
        out: &mut Vec<f32>,
    ) {
        let n = usize::from(self.num_components()).max(1);
        let colors = &colors[..colors.len() / n * n];
        match self.0.as_ref() {
            ColorSpaceType::Indexed(i) => {
                let width = i.base.ink_space(simulation).len();
                let mut table = Vec::with_capacity((usize::from(i.hival) + 1) * width);
                for values in &i.values {
                    let start = table.len();
                    i.base.ink_values_scaled(values, simulation, true, &mut table);
                    table.resize(start + width, 0.0);
                }
                for index in colors {
                    let index = (clamp_component(*index, 0.0, f32::from(i.hival)) + 0.5) as usize;
                    match table.get(index * width..(index + 1) * width) {
                        Some(values) => out.extend_from_slice(values),
                        None => out.extend(std::iter::repeat_n(0.0, width)),
                    }
                }
            }
            ColorSpaceType::Plate(p) => p.inner.ink_values_scaled(colors, simulation, manual_scale, out),
            ColorSpaceType::Pattern(_) => {}
            ColorSpaceType::DeviceGray => out.extend(colors.iter().map(|v| unit(*v))),
            ColorSpaceType::Separation(s) => {
                if s.colorant != Colorant::None {
                    out.extend(colors.iter().map(|v| unit(*v)));
                }
            }
            ColorSpaceType::DeviceN(d) => {
                if d.is_none {
                    return;
                }
                for colour in colors.chunks_exact(n) {
                    for (c, v) in d.colorants.iter().zip(colour) {
                        if matches!(c, Colorant::Ink(_)) {
                            out.push(unit(*v));
                        }
                    }
                }
            }
            _ => {
                let start = out.len();
                out.resize(start + colors.len() / n * 4, 0.0);
                if let Some(simulation) = simulation.filter(|s| self.simulated_by(s))
                    && self
                        .simulate_cmyk(simulation, colors, manual_scale, &mut out[start..])
                        .is_some()
                {
                    for v in &mut out[start..] {
                        *v = unit(*v);
                    }
                    return;
                }
                if self.separates_through_srgb() {
                    let mut rgb = vec![0_u8; colors.len() / n * 3];
                    if self.convert_f32(colors, &mut rgb, manual_scale).is_some() {
                        for (cmyk, rgb) in out[start..].chunks_exact_mut(4).zip(rgb.chunks_exact(3)) {
                            cmyk.copy_from_slice(&rgb_to_cmyk([rgb[0], rgb[1], rgb[2]].map(u8_to_f32)));
                        }
                        return;
                    }
                }
                for (cmyk, colour) in out[start..].chunks_exact_mut(4).zip(colors.chunks_exact(n)) {
                    if let Some(c) = self.process_cmyk(colour, None, manual_scale) {
                        cmyk.copy_from_slice(&c);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod device_color_compatibility_tests {
    use super::ColorSpace;

    #[test]
    fn device_rgb_rounds_components_and_opacity_to_bytes() {
        let space = ColorSpace::device_rgb();
        assert_eq!(space.to_rgba(&[0.0, 0.5, 1.0], 0.5, false).to_rgba8(), [0, 128, 255, 128]);
        assert_eq!(space.to_rgba(&[-1.0, 2.0, f32::NAN], 1.0, false).to_rgba8(), [0, 255, 0, 255]);
        assert_eq!(space.to_rgba(&[0.5], 1.0, false).to_rgba8(), [128, 0, 0, 255]);
    }

    #[test]
    fn device_gray_uses_the_same_rounding_and_missing_component_default() {
        let space = ColorSpace::device_gray();
        assert_eq!(space.to_rgba(&[0.5], 0.5, false).to_rgba8(), [128, 128, 128, 128]);
        assert_eq!(space.to_rgba(&[], 1.0, false).to_rgba8(), [0, 0, 0, 255]);
    }
}

#[cfg(test)]
mod nesting_tests {
    use super::{Cache, ColorSpace, MAX_COLOR_SPACE_DEPTH};
    use hayro_syntax::Pdf;
    use hayro_syntax::object::{Object, ObjectIdentifier};

    /// A PDF whose object 3 starts a chain of `levels` `/Indexed` spaces, each naming the next
    /// as its base through an indirect reference, ending in `/DeviceGray`.
    fn indexed_chain(levels: usize) -> Pdf {
        let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), "<< /Type /Pages /Count 0 /Kids [] >>".to_string()];
        for i in 0..levels {
            objects.push(format!("[/Indexed {} 0 R 0 <00>]", i + 4));
        }
        objects.push("/DeviceGray".to_string());
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        Pdf::new(out).expect("synthetic pdf")
    }

    fn chain_space(levels: usize) -> Option<ColorSpace> {
        let pdf = indexed_chain(levels);
        let object = pdf.xref().get::<Object<'_>>(ObjectIdentifier::new(3, 0)).expect("chain start");
        ColorSpace::new(object, &Cache::new())
    }

    #[test]
    fn nested_color_spaces_stop_at_the_depth_limit() {
        let deepest = (MAX_COLOR_SPACE_DEPTH as usize) - 1;
        assert!(chain_space(1).is_some());
        assert!(chain_space(deepest).is_some(), "the base at the deepest allowed level resolves");
        assert!(chain_space(deepest + 1).is_none());
    }

    #[test]
    fn a_long_indirect_chain_fails_without_exhausting_the_stack() {
        assert!(chain_space(100_000).is_none());
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
