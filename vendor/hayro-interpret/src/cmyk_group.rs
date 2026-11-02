//! PdfCraft patch (70): an isolated transparency group that blends in `DeviceCMYK`.
//!
//! ISO 32000-2 §11.3.4: a group's elements blend in the group's colour space, and the
//! components of a subtractive space are complemented before the blend function and
//! complemented back after it. The renderer composites in RGB, which gives other colours
//! wherever a blend mode meets CMYK artwork: `0 1 0 0 k` (magenta) under `0 0 0 1 k` (black)
//! with `/BM /Difference` is green when blended in CMYK (`|(1,0,1,1) − (1,1,1,0)|`
//! complemented back is `1 0 1 0`) and magenta when blended in RGB.
//!
//! An *isolated* group starts from a transparent backdrop of its own (§11.4.7), so its
//! interior can be computed apart from the page and handed back as finished pixels. A device
//! that can blend in CMYK takes such a group through [`Device::draw_isolated_cmyk_group`] and
//! interprets it with [`CmykGroup::interpret`] as often as it needs; every other device, and a
//! device that declines, gets the group's contents as before.

use crate::InterpreterSettings;
use crate::context::{Context, InterpreterCache};
use crate::device::Device;
use crate::interpret::state::{ClipType, State};
use crate::x_object::{FormXObject, interpret_form_contents};
use hayro_syntax::object::dict::keys::{CS, GROUP};
use hayro_syntax::object::{Dict, Name};
use hayro_syntax::page::Resources;
use hayro_syntax::xref::XRef;
use kurbo::{Affine, Rect, Shape};
use std::ops::Deref;

/// An isolated, non-knockout transparency group whose blending colour space is `DeviceCMYK`,
/// captured where it is painted (PdfCraft patch (70); see the module documentation).
pub struct CmykGroup<'a, 'b> {
    form: &'b FormXObject<'a>,
    resources: &'b Resources<'a>,
    /// The graphics state the group's contents start with: the state at `Do`, after the
    /// form's `/Matrix`, with the group's own opacity, soft mask and blend mode already taken
    /// by the layer the group is composited through.
    state: State<'a>,
    root_transform: Affine,
    bbox: Rect,
    interpreter_cache: InterpreterCache<'a>,
    settings: InterpreterSettings,
    xref: &'a XRef,
    nesting_depth: u32,
}

impl<'a, 'b> CmykGroup<'a, 'b> {
    pub(crate) fn new(form: &'b FormXObject<'a>, resources: &'b Resources<'a>, context: &Context<'a>) -> Self {
        let mut state = context.get().clone();
        // The clips in force belong to the device the group is composited onto; contents
        // interpreted on their own push and pop only the clips they set themselves.
        state.clips = Vec::<ClipType>::new();
        Self {
            form,
            resources,
            state,
            root_transform: context.root_transform(),
            bbox: context.bbox(),
            interpreter_cache: context.interpreter_cache.clone(),
            settings: context.settings.clone(),
            xref: context.xref,
            nesting_depth: context.nesting_depth(),
        }
    }

    /// The device-space box the group can paint: its `/BBox` under the current transform,
    /// within the area being drawn.
    pub fn device_bbox(&self) -> Rect {
        let [x0, y0, x1, y1] = self.form.bbox;
        let user = Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64);
        let device = (self.state.ctm * user.to_path(0.1)).bounding_box();
        device.intersect(self.bbox)
    }

    /// Interpret the group's contents into `device`, with device space moved by `offset`
    /// (so a device covering only [`Self::device_bbox`] can start at its corner).
    pub fn interpret(&self, device: &mut impl Device<'a>, offset: (f64, f64)) {
        let shift = Affine::translate((-offset.0, -offset.1));
        let mut state = self.state.clone();
        state.ctm = shift * state.ctm;
        let mut context = Context::new_with(
            shift * self.root_transform,
            shift.transform_rect_bbox(self.bbox),
            &self.interpreter_cache,
            self.xref,
            self.settings.clone(),
            state,
            self.nesting_depth,
        );
        interpret_form_contents(self.resources, self.form, &mut context, device);
    }
}

/// Whether `form`'s transparency group blends in `DeviceCMYK`: it says so in its `/CS`.
pub(crate) fn blends_in_device_cmyk(form: &Dict<'_>) -> bool {
    form.get::<Dict<'_>>(GROUP).and_then(|g| g.get::<Name<'_>>(CS)).is_some_and(|cs| matches!(cs.deref(), b"DeviceCMYK" | b"CMYK"))
}

/// Whether `form`'s transparency group names a blending colour space of its own (`/CS`).
pub(crate) fn names_a_blending_space(form: &Dict<'_>) -> bool {
    form.get::<Dict<'_>>(GROUP).is_some_and(|g| g.contains_key(CS))
}
