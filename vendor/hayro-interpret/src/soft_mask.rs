use crate::color::{CmykSimulation, Color, ColorComponents, ColorSpace, DeviceFamily, Ink, Plate, device_cmyk_to_gray};
use crate::context::{Context, InterpreterCache};
use crate::device::Device;
use crate::function::Function;
use crate::interpret::state::State;
use crate::util::hash128;
use crate::x_object::{FormXObject, draw_form_xobject};
use crate::{CacheKey, InterpreterSettings};
use hayro_syntax::object::Name;
use hayro_syntax::object::ObjectIdentifier;
use hayro_syntax::object::Stream;
use hayro_syntax::object::dict::keys::*;
use hayro_syntax::object::{Dict, Object};
use hayro_syntax::page::Resources;
use hayro_syntax::xref::XRef;
use kurbo::Affine;
use smallvec::smallvec;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::rc::Rc;

/// Type type of mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskType {
    /// A luminosity mask.
    Luminosity,
    /// An alpha mask.
    Alpha,
}

/// A transfer function to apply to the opacity values of a mask.
pub struct TransferFunction(Function);

impl TransferFunction {
    /// Apply the transfer function to byte opacity values.
    ///
    /// Inputs and outputs use the same rounded 8-bit samples as the renderer. A function
    /// that cannot evaluate a sample leaves it unchanged.
    pub fn apply_to(&self, values: &mut [u8]) {
        for value in values {
            if let Some(output) = self.0.eval(smallvec![f32::from(*value) / 255.0])
                .and_then(|v| v.first().copied())
                .filter(|output| output.is_finite())
            {
                *value = (output.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    }

    /// Apply the transfer function to the given value.
    ///
    /// The input value needs to be between 0 and 1 and the return value is
    /// guaranteed to be between 0 and 1.
    #[inline]
    pub fn apply(&self, val: f32) -> f32 {
        self.0
            .eval(smallvec![val])
            .and_then(|v| v.first().copied())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    }
}

struct Repr<'a> {
    cache_key: u128,
    obj_id: ObjectIdentifier,
    group: FormXObject<'a>,
    mask_type: MaskType,
    parent_resources: Resources<'a>,
    root_transform: Affine,
    bbox: kurbo::Rect,
    interpreter_cache: InterpreterCache<'a>,
    transfer_function: Option<TransferFunction>,
    settings: InterpreterSettings,
    background: Color,
    // PdfCraft patch (63).
    /// A luminosity mask whose group colour space is `/DeviceCMYK`.
    luminosity_in_device_cmyk: bool,
    xref: &'a XRef,
    nesting_depth: u32,
    /// Craft patch (CMYK masks): the group's `/CS` is DeviceCMYK.
    device_cmyk: bool,
    /// Craft patch (CMYK masks): the page plate's printing condition, for [`SoftMask::plate`].
    simulation: Option<CmykSimulation>,
}

impl Hash for Repr<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.obj_id.hash(state);
    }
}

/// A soft mask.
#[derive(Clone, Hash)]
pub struct SoftMask<'a>(Rc<Repr<'a>>);

impl Debug for SoftMask<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SoftMask({:?})", self.0.obj_id)
    }
}

impl PartialEq for SoftMask<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.obj_id == other.0.obj_id
    }
}

impl Eq for SoftMask<'_> {}

impl CacheKey for SoftMask<'_> {
    fn cache_key(&self) -> u128 {
        self.0.cache_key
    }
}

impl<'a> SoftMask<'a> {
    pub(crate) fn new(
        dict: &Dict<'a>,
        context: &Context<'a>,
        parent_resources: Resources<'a>,
    ) -> Option<Self> {
        // TODO: With this setup, if there is a luminosity mask and alpha mask pointing to the
        // same xobject, the ID will be the same.
        let obj_id = dict.get_ref(G)?.into();
        let group_stream = dict.get::<Stream<'_>>(G)?;
        let group = FormXObject::new(&group_stream)?;
        // PdfCraft patch: the group's /CS is optional (ISO 32000-2 §11.6.5.2; Chrome/Skia omit
        // it). Upstream returned None here, silently dropping the whole soft mask. Fall back to
        // DeviceRGB, the blending space our renderer composites in.
        let cs_object = group
            .dict
            .get::<Dict<'_>>(GROUP)
            .and_then(|g| g.get::<Object<'_>>(CS));
        // PdfCraft patch (63): the group is composited in `/DeviceCMYK` itself. An ICC-based
        // or calibrated group space keeps its own conversion.
        let group_in_device_cmyk = cs_object
            .as_ref()
            .and_then(DeviceFamily::of_object)
            .is_some_and(|family| family == DeviceFamily::Cmyk);
        let cs = cs_object
            .and_then(|o| ColorSpace::new(o, &context.interpreter_cache.object_cache))
            .unwrap_or_else(ColorSpace::device_rgb);
        let device_cmyk = cs.is_device_cmyk();
        let transfer_function = dict
            .get::<Object<'_>>(TR)
            .and_then(|o| Function::new(&o))
            .map(TransferFunction);
        let (mask_type, background) = match dict.get::<Name<'_>>(S)?.deref() {
            LUMINOSITY => {
                let color = dict
                    .get::<ColorComponents>(BC)
                    .map(|c| {
                        // PdfCraft patch (63): a DeviceCMYK backdrop's luminosity is its gray
                        // (ISO 32000-2 §11.5.3), as for the group's content.
                        if group_in_device_cmyk && c.len() == 4 {
                            Color::new(
                                ColorSpace::device_gray(),
                                smallvec![device_cmyk_to_gray(&c)],
                                1.0,
                            )
                        } else {
                            Color::new(cs, c, 1.0)
                        }
                    })
                    .unwrap_or(Color::new(ColorSpace::device_gray(), smallvec![0.0], 1.0));

                (MaskType::Luminosity, color)
            }
            ALPHA => (
                MaskType::Alpha,
                // Background color attribute should only be used with luminosity masks.
                Color::new(ColorSpace::device_gray(), smallvec![0.0], 1.0),
            ),
            _ => return None,
        };
        let nesting_depth = context.nesting_depth() + 1;
        let root_transform = context.get().ctm;
        let bbox = context.bbox();
        let resources_key = resources_cache_key(&parent_resources);
        let simulation = context
            .settings
            .plate
            .as_ref()
            .and_then(|p| p.simulation.clone());
        let cache_key = hash128(&(
            simulation.as_ref().map(|s| s.digest()),
            dict.cache_key(),
            root_transform.cache_key(),
            [bbox.x0.to_bits(), bbox.y0.to_bits(), bbox.x1.to_bits(), bbox.y1.to_bits()],
            resources_key,
            nesting_depth,
        ));

        Some(Self(Rc::new(Repr {
            cache_key,
            obj_id,
            group,
            mask_type,
            root_transform,
            transfer_function,
            bbox: context.bbox(),
            interpreter_cache: context.interpreter_cache.clone(),
            // PdfCraft patch (35): a mask's luminosity comes from its group's colours, never
            // from the page's plate, and the inks it uses are never printed.
            settings: InterpreterSettings {
                plate: None,
                ink_sink: None,
                ..context.settings.clone()
            },
            xref: context.xref,
            background,
            luminosity_in_device_cmyk: group_in_device_cmyk && mask_type == MaskType::Luminosity,
            parent_resources,
            nesting_depth,
            device_cmyk,
            simulation,
        })))
    }

    /// Craft patch (CMYK masks): whether the mask group's color space is DeviceCMYK. A
    /// luminosity mask then takes its values from the group's CMYK by the ISO 32000-2 §10.4.2.2
    /// formula, `1 - min(1, 0.3c + 0.59m + 0.11y + k)`, rather than from its colors as drawn:
    /// drawn through a CMYK profile, K 100% is not black.
    pub fn is_device_cmyk(&self) -> bool {
        self.0.device_cmyk
    }

    /// Craft patch (CMYK masks): the plate of process `ink` that the mask group composites on a
    /// CMYK device, for [`Self::interpret_plate`] and [`Plate::tint`] of the backdrop. Spot
    /// colorants paint through their alternate space, and nothing overprints.
    pub fn plate(&self, ink: Ink) -> Plate {
        Plate {
            ink,
            overprint: false,
            simulation: self.0.simulation.clone(),
            spots_as_process: true,
        }
    }

    /// Craft patch (CMYK masks): interpret the mask's contents as the film of `plate`'s ink
    /// (see [`Self::plate`]).
    pub fn interpret_plate(&self, device: &mut impl Device<'a>, plate: Plate) {
        let state = State::new(self.0.root_transform);
        let settings = InterpreterSettings {
            plate: Some(plate),
            ..self.0.settings.clone()
        };
        let mut ctx = Context::new_with(
            self.0.root_transform,
            self.0.bbox,
            &self.0.interpreter_cache,
            self.0.xref,
            settings,
            state,
            self.0.nesting_depth,
        );
        draw_form_xobject(&self.0.parent_resources, &self.0.group, &mut ctx, device);
    }

    /// Interpret the contents of the mask into the given device.
    pub fn interpret(&self, device: &mut impl Device<'a>) {
        let state = State::new(self.0.root_transform);
        let mut ctx = Context::new_with(
            self.0.root_transform,
            self.0.bbox,
            &self.0.interpreter_cache,
            self.0.xref,
            self.0.settings.clone(),
            state,
            self.0.nesting_depth,
        );
        ctx.luminosity_in_device_cmyk = self.0.luminosity_in_device_cmyk;
        draw_form_xobject(&self.0.parent_resources, &self.0.group, &mut ctx, device);
    }

    /// Return the object identifier of the mask.
    ///
    /// This can be used as a unique identifier for caching purposes.
    pub fn id(&self) -> ObjectIdentifier {
        self.0.obj_id
    }

    /// Return the underlying mask type.
    pub fn mask_type(&self) -> MaskType {
        self.0.mask_type
    }

    /// The background color against which the mask should be composited.
    pub fn background_color(&self) -> Color {
        self.0.background.clone()
    }

    /// Return the transfer function that should be used for the mask.
    pub fn transfer_function(&self) -> Option<&TransferFunction> {
        self.0.transfer_function.as_ref()
    }
}

fn resources_cache_key(resources: &Resources<'_>) -> u128 {
    let local = (
        resources.ext_g_states.obj_id(),
        resources.fonts.obj_id(),
        resources.properties.obj_id(),
        resources.color_spaces.obj_id(),
        resources.x_objects.obj_id(),
        resources.patterns.obj_id(),
        resources.shadings.obj_id(),
    );
    hash128(&(local, resources.parent().map(resources_cache_key)))
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use hayro_syntax::object::FromBytes;

    fn transfer(bytes: &[u8]) -> TransferFunction {
        let dict = Dict::from_bytes(bytes).expect("synthetic function dictionary");
        TransferFunction(Function::new(&Object::Dict(dict)).expect("synthetic function"))
    }

    #[test]
    fn byte_transfer_rounds_near_equal_endpoints() {
        let function = transfer(b"<< /FunctionType 2 /Domain [0 1] /C0 [0.5001] /C1 [0.5] /N 1 >>");
        assert!(function.apply(0.0) > function.apply(1.0));
        let mut ends = [0, 255];
        function.apply_to(&mut ends);
        assert_eq!(ends, [128, 128], "sub-byte differences do not invert a byte mask");
    }

    #[test]
    fn byte_transfer_preserves_identity_and_maps_inversion() {
        for (function, expected) in [
            (b"<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>".as_slice(), [0, 1, 127, 128, 254, 255]),
            (b"<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>".as_slice(), [255, 254, 128, 127, 1, 0]),
        ] {
            let mut values = [0, 1, 127, 128, 254, 255];
            transfer(function).apply_to(&mut values);
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn byte_transfer_keeps_a_sample_when_the_function_has_no_output() {
        let function = transfer(b"<< /FunctionType 2 /Domain [0 1] /C0 [] /C1 [] /N 1 >>");
        let mut values = [0, 17, 127, 255];
        function.apply_to(&mut values);
        assert_eq!(values, [0, 17, 127, 255]);
    }

    #[test]
    fn byte_transfer_keeps_samples_when_malformed_functions_are_not_finite() {
        // Fractional powers of a negative domain produce NaN; finite coefficients can
        // also overflow during interpolation. Neither is a byte opacity value.
        for bytes in [
            b"<< /FunctionType 2 /Domain [-1 -0.5] /C0 [0] /C1 [1] /N 0.5 >>".as_slice(),
            b"<< /FunctionType 2 /Domain [0 1] /C0 [-300000000000000000000000000000000000000] /C1 [300000000000000000000000000000000000000] /N 1 >>",
        ] {
            let mut values = [0, 17, 127, 255];
            transfer(bytes).apply_to(&mut values);
            assert_eq!(values, [0, 17, 127, 255]);
        }
    }
}
