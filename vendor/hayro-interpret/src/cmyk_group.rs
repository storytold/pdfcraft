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
//!
//! PdfCraft patch (71): the page group too (#4887). A page whose `/Group` blends in
//! `DeviceCMYK` composites its contents in CMYK (§11.3.4, §11.4.7), and so do the
//! non-isolated and knockout groups on it, which blend in their parent's space (Table 145). The
//! renderer draws the page group from a transparent backdrop of its own and puts it on the
//! paper afterwards (hayro patch (18)), so the page's contents go to the device the same way as
//! an isolated group's. Only a page that reaches a blend mode other than `Normal` is offered
//! ([`page_blends_in_device_cmyk`]): every press-ready page carries a CMYK page group, and one
//! that blends nothing would pay for the passes for no visible difference.

use crate::InterpreterSettings;
use crate::context::{Context, InterpreterCache};
use crate::device::Device;
use crate::interpret::interpret_content;
use crate::interpret::state::{ClipType, State};
use crate::x_object::{FormXObject, interpret_form_contents};
use hayro_syntax::object::dict::keys::{BM, CS, EXT_G_STATE, FONT, GROUP, PATTERN, RESOURCES, SUBTYPE, XOBJECT};
use hayro_syntax::object::{Array, Dict, Name, Stream};
use hayro_syntax::page::{Page, Resources};
use hayro_syntax::xref::XRef;
use kurbo::{Affine, Rect, Shape};
use std::ops::Deref;

/// An isolated, non-knockout transparency group whose blending colour space is `DeviceCMYK`,
/// captured where it is painted (PdfCraft patch (70); see the module documentation), or the
/// contents of a page whose page group blends in `DeviceCMYK` (patch (71)).
pub struct CmykGroup<'a, 'b> {
    source: Source<'a, 'b>,
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
    /// The page's nested paints when it was captured, which each pass of a page starts from
    /// (see `context::nested_paints_used`).
    nested_paints: u32,
}

impl<'a, 'b> CmykGroup<'a, 'b> {
    pub(crate) fn new(form: &'b FormXObject<'a>, resources: &'b Resources<'a>, context: &Context<'a>) -> Self {
        Self::with_source(Source::Form { form, resources }, context)
    }

    /// The contents of `page` (not its annotations), drawn from the start of the page (patch (71)).
    pub(crate) fn for_page(page: &'b Page<'a>, context: &Context<'a>) -> Self {
        Self::with_source(Source::Page(page), context)
    }

    fn with_source(source: Source<'a, 'b>, context: &Context<'a>) -> Self {
        let mut state = context.get().clone();
        // The clips in force belong to the device the group is composited onto; contents
        // interpreted on their own push and pop only the clips they set themselves.
        state.clips = Vec::<ClipType>::new();
        Self {
            source,
            state,
            root_transform: context.root_transform(),
            bbox: context.bbox(),
            interpreter_cache: context.interpreter_cache.clone(),
            settings: context.settings.clone(),
            xref: context.xref,
            nesting_depth: context.nesting_depth(),
            nested_paints: crate::context::nested_paints_used(),
        }
    }

    /// The device-space box the group can paint: its `/BBox` under the current transform,
    /// within the area being drawn (all of it, for a page).
    pub fn device_bbox(&self) -> Rect {
        let Source::Form { form, .. } = &self.source else {
            return self.bbox;
        };
        let [x0, y0, x1, y1] = form.bbox;
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
        match &self.source {
            Source::Form { form, resources } => interpret_form_contents(resources, form, &mut context, device),
            Source::Page(page) => {
                crate::context::set_nested_paints_used(self.nested_paints);
                interpret_content(&mut page.content_reader(), page.resources(), &mut context, device);
            }
        }
    }
}

/// What a [`CmykGroup`] draws.
enum Source<'a, 'b> {
    /// An isolated group's form, with the resources it is drawn with.
    Form { form: &'b FormXObject<'a>, resources: &'b Resources<'a> },
    /// A page's own contents.
    Page(&'b Page<'a>),
}

/// PdfCraft patch (71): whether `page`'s contents are offered to the device to blend in
/// `DeviceCMYK`: its page group says so in its `/CS`, and its contents can set a blend mode
/// other than `Normal` (see the module documentation). Public for PdfCraft's tests: this
/// crate's own don't build outside the workspace it is patched into.
pub fn page_blends_in_device_cmyk(page: &Page<'_>) -> bool {
    blends_in_device_cmyk(page.raw()) && reaches_a_blend_mode(page.resources())
}

/// How many resource entries [`reaches_a_blend_mode`] reads before it gives up and answers yes,
/// and how deeply it follows forms, patterns and Type 3 fonts inside one another. Answering
/// yes only costs the device its passes, which fall back by themselves when they must.
const MAX_RESOURCE_ENTRIES: usize = 4096;
const MAX_RESOURCE_DEPTH: u32 = 32;

/// Whether contents drawn with `resources` can set a blend mode other than `Normal` (or its
/// synonym `Compatible`): an extended graphics state in them, or in the resources of a form,
/// tiling pattern or Type 3 font in them, names one. Only `gs` sets a blend mode, and it can only
/// name an extended graphics state of the resources in force.
fn reaches_a_blend_mode(resources: &Resources<'_>) -> bool {
    let mut budget = MAX_RESOURCE_ENTRIES;
    let mut level = Some(resources);
    while let Some(r) = level {
        if entries_reach_a_blend_mode(&r.ext_g_states, &r.x_objects, &r.patterns, &r.fonts, &mut budget, 0) {
            return true;
        }
        level = r.parent();
    }
    false
}

/// [`reaches_a_blend_mode`] for a resource dictionary as written.
fn dict_reaches_a_blend_mode(resources: &Dict<'_>, budget: &mut usize, depth: u32) -> bool {
    let kind = |key: &[u8]| resources.get::<Dict<'_>>(key).unwrap_or_else(Dict::empty);
    entries_reach_a_blend_mode(&kind(EXT_G_STATE), &kind(XOBJECT), &kind(PATTERN), &kind(FONT), budget, depth)
}

fn entries_reach_a_blend_mode(
    ext_g_states: &Dict<'_>,
    x_objects: &Dict<'_>,
    patterns: &Dict<'_>,
    fonts: &Dict<'_>,
    budget: &mut usize,
    depth: u32,
) -> bool {
    if depth > MAX_RESOURCE_DEPTH {
        return true;
    }
    let nested =
        |dict: &Dict<'_>, budget: &mut usize| dict.get::<Dict<'_>>(RESOURCES).is_some_and(|r| dict_reaches_a_blend_mode(&r, budget, depth + 1));
    for key in ext_g_states.keys() {
        if spent(budget) || ext_g_states.get::<Dict<'_>>(key.deref()).is_some_and(|gs| sets_a_blend_mode(&gs)) {
            return true;
        }
    }
    for key in x_objects.keys() {
        if spent(budget) {
            return true;
        }
        if let Some(form) = x_objects.get::<Stream<'_>>(key.deref())
            && form.dict().get::<Name<'_>>(SUBTYPE).is_some_and(|s| s.deref() == b"Form")
            && nested(form.dict(), budget)
        {
            return true;
        }
    }
    for key in patterns.keys() {
        if spent(budget) {
            return true;
        }
        // A tiling pattern is a stream with resources of its own; a shading pattern a dictionary
        // whose `/ExtGState` applies while it paints.
        if let Some(tiling) = patterns.get::<Stream<'_>>(key.deref()) {
            if nested(tiling.dict(), budget) {
                return true;
            }
        } else if let Some(shading) = patterns.get::<Dict<'_>>(key.deref())
            && shading.get::<Dict<'_>>(EXT_G_STATE).is_some_and(|gs| sets_a_blend_mode(&gs))
        {
            return true;
        }
    }
    for key in fonts.keys() {
        if spent(budget) {
            return true;
        }
        if let Some(font) = fonts.get::<Dict<'_>>(key.deref())
            && font.get::<Name<'_>>(SUBTYPE).is_some_and(|s| s.deref() == b"Type3")
            && nested(&font, budget)
        {
            return true;
        }
    }
    false
}

/// Take one entry from `budget`; `true` when there was none left.
fn spent(budget: &mut usize) -> bool {
    match budget.checked_sub(1) {
        Some(left) => {
            *budget = left;
            false
        }
        None => true,
    }
}

/// Whether extended graphics state `gs` sets a blend mode other than `Normal`: by name, or as an
/// array of names to choose from (ISO 32000-2 §11.6.3).
fn sets_a_blend_mode(gs: &Dict<'_>) -> bool {
    let blends = |name: &Name<'_>| !matches!(name.deref(), b"Normal" | b"Compatible");
    if let Some(name) = gs.get::<Name<'_>>(BM) {
        return blends(&name);
    }
    gs.get::<Array<'_>>(BM).is_some_and(|names| names.iter::<Name<'_>>().any(|n| blends(&n)))
}

/// Whether `form`'s transparency group blends in `DeviceCMYK`: it says so in its `/CS`.
pub(crate) fn blends_in_device_cmyk(form: &Dict<'_>) -> bool {
    form.get::<Dict<'_>>(GROUP).and_then(|g| g.get::<Name<'_>>(CS)).is_some_and(|cs| matches!(cs.deref(), b"DeviceCMYK" | b"CMYK"))
}

/// Whether `form`'s transparency group names a blending colour space of its own (`/CS`).
pub(crate) fn names_a_blending_space(form: &Dict<'_>) -> bool {
    form.get::<Dict<'_>>(GROUP).is_some_and(|g| g.contains_key(CS))
}
