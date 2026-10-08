use crate::cache::{Cache, CacheKey};
use crate::color::ColorSpace;
use crate::convert::convert_transform;
use crate::font::{Font, StandardFont};
use crate::interpret::state::{ClipType, State, TextStateFont};
use crate::ocg::OcgState;
use crate::util::{BezPathExt, Float64Ext};
use crate::{ClipPath, Device, FillRule, InterpreterSettings, StrokeProps};
use hayro_syntax::content::ops::Transform;
use hayro_syntax::object::Dict;
use hayro_syntax::object::Name;
use hayro_syntax::page::Resources;
use hayro_syntax::xref::XRef;
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Maximum nesting depth for interpreting `XObject`'s/patterns/streams.
pub(crate) const MAX_NESTED_INTERPRETATION_DEPTH: u32 = 50;
/// PdfCraft patch: maximum nesting for content that is painted recursively by the device
/// (tiling patterns, Type 3 glyphs). Either can refer back to itself through resources it
/// inherits, which recursed until the stack overflowed (found by fuzzing).
pub(crate) const MAX_PAINT_NESTING: u32 = 16;

/// PdfCraft patch: how many nested paints (a form inside a form, a Type 3 glyph or tiling
/// pattern painted from inside another one) one page may do. Nesting depth alone doesn't bound
/// the work: a glyph that shows ten glyphs of its own font, nested sixteen deep, is 10^16
/// paints (a fuzzed file hung). Ordinary pages stay far below this.
pub(crate) const MAX_NESTED_PAINTS: u32 = 50_000;

std::thread_local! {
    static NESTED_PAINTS: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
}

/// PdfCraft patch: count a paint at `depth` (top-level ones are free); `false` once this
/// page's budget is spent, and the paint is skipped.
pub(crate) fn take_nested_paint(depth: u32) -> bool {
    if depth < 2 {
        return true;
    }
    NESTED_PAINTS.with(|n| {
        let used = n.get();
        if used >= MAX_NESTED_PAINTS {
            return false;
        }
        n.set(used + 1);
        true
    })
}

/// A cache used by the interpreter.
///
/// Ideally, such a cache should be constructed once per PDF and then reused across
/// multiple interpreter invocations on the same document.
#[derive(Clone)]
pub struct InterpreterCache<'a> {
    pub(crate) font_cache: Rc<RefCell<HashMap<u128, Option<Font<'a>>>>>,
    pub(crate) object_cache: Cache,
}

impl<'a> Default for InterpreterCache<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> InterpreterCache<'a> {
    /// Create a new interpreter cache.
    pub fn new() -> Self {
        Self {
            font_cache: Rc::new(RefCell::new(HashMap::new())),
            object_cache: Cache::new(),
        }
    }
}

/// A per-page interpretation context that borrows shared data from an [`InterpreterCache`].
pub struct Context<'a> {
    states: Vec<State<'a>>,
    path: BezPath,
    sub_path_start: Point,
    last_point: Point,
    clip: Option<FillRule>,
    root_transforms: Vec<Affine>,
    bbox: Vec<Rect>,
    pub(crate) settings: InterpreterSettings,
    pub(crate) interpreter_cache: InterpreterCache<'a>,
    pub(crate) xref: &'a XRef,
    pub(crate) ocg_state: OcgState,
    nesting_depth: u32,
}

impl<'a> Context<'a> {
    /// Create a new context.
    pub fn new(
        initial_transform: Affine,
        bbox: Rect,
        cache: &InterpreterCache<'a>,
        xref: &'a XRef,
        settings: InterpreterSettings,
    ) -> Self {
        let state = State::new(initial_transform);
        // PdfCraft patch: a page (or other top-level content) starts with a fresh budget.
        NESTED_PAINTS.with(|n| n.set(0));

        Self::new_with(initial_transform, bbox, cache, xref, settings, state, 0)
    }

    pub(crate) fn new_with(
        initial_transform: Affine,
        bbox: Rect,
        cache: &InterpreterCache<'a>,
        xref: &'a XRef,
        settings: InterpreterSettings,
        state: State<'a>,
        nesting_depth: u32,
    ) -> Self {
        let mut ocg_state = {
            let root_ref = xref.root_id();
            xref.get::<Dict<'_>>(root_ref)
                .map(|catalog| OcgState::from_catalog(&catalog))
                .unwrap_or_default()
        };
        // PdfCraft patch: apply viewer layer toggles.
        for (num, generation, visible) in settings.ocg_overrides.iter() {
            ocg_state.set_active(hayro_syntax::object::ObjectIdentifier::new(*num, *generation), *visible);
        }

        Self {
            states: vec![state],
            settings,
            xref,
            root_transforms: vec![initial_transform],
            last_point: Point::default(),
            sub_path_start: Point::default(),
            clip: None,
            bbox: vec![bbox],
            path: BezPath::new(),
            interpreter_cache: cache.clone(),
            ocg_state,
            nesting_depth,
        }
    }

    pub(crate) fn save_state(&mut self) {
        let Some(cur) = self.states.last().cloned() else {
            warn!("attempted to save state without existing state");
            return;
        };

        self.states.push(cur);
    }

    pub(crate) fn bbox(&self) -> Rect {
        self.bbox.last().copied().unwrap_or_else(|| {
            warn!("failed to get a bbox");

            Rect::new(0.0, 0.0, 1.0, 1.0)
        })
    }

    fn push_bbox(&mut self, bbox: Rect) {
        let new = self.bbox().intersect(bbox);
        self.bbox.push(new);
    }

    pub(crate) fn push_clip_path(
        &mut self,
        clip_path: BezPath,
        fill: FillRule,
        device: &mut impl Device<'a>,
    ) {
        if let Some(clip_rect) = path_as_rect(&clip_path) {
            let cur_bbox = self.bbox();

            // If the clip path is a rect and completely covers the current bbox, don't emit it.
            if cur_bbox
                .min_x()
                .is_nearly_greater_or_equal(clip_rect.min_x())
                && cur_bbox
                    .min_y()
                    .is_nearly_greater_or_equal(clip_rect.min_y())
                && cur_bbox.max_x().is_nearly_less_or_equal(clip_rect.max_x())
                && cur_bbox.max_y().is_nearly_less_or_equal(clip_rect.max_y())
            {
                self.get_mut().clips.push(ClipType::Dummy);
                return;
            }
        }

        let bbox = clip_path.bounding_box();
        device.push_clip_path(&ClipPath {
            path: clip_path,
            fill,
        });
        self.push_bbox(bbox);
        self.get_mut().clips.push(ClipType::Real);
    }

    pub(crate) fn pop_clip_path(&mut self, device: &mut impl Device<'a>) {
        if let Some(ClipType::Real) = self.get_mut().clips.pop() {
            device.pop_clip_path();
            self.pop_bbox();
        }
    }

    fn pop_bbox(&mut self) {
        self.bbox.pop();
    }

    pub(crate) fn push_root_transform(&mut self) {
        self.root_transforms.push(self.get().ctm);
    }

    pub(crate) fn pop_root_transform(&mut self) {
        self.root_transforms.pop();
    }

    pub(crate) fn root_transform(&self) -> Affine {
        self.root_transforms
            .last()
            .copied()
            .unwrap_or(Affine::IDENTITY)
    }

    pub(crate) fn restore_state(&mut self, device: &mut impl Device<'a>) {
        let Some(target_clips) = self
            .states
            .get(self.states.len().saturating_sub(2))
            .map(|s| s.clips.len())
        else {
            warn!("underflowed graphics state");
            return;
        };

        while self.get().clips.len() > target_clips {
            self.pop_clip_path(device);
        }

        // The first state should never be popped.
        if self.states.len() > 1 {
            self.states.pop();
        }

        device.set_soft_mask(
            self.states
                .last()
                .and_then(|l| l.graphics_state.soft_mask.clone()),
        );
    }

    pub(crate) fn path(&self) -> &BezPath {
        &self.path
    }

    pub(crate) fn path_mut(&mut self) -> &mut BezPath {
        &mut self.path
    }

    pub(crate) fn sub_path_start(&self) -> &Point {
        &self.sub_path_start
    }

    pub(crate) fn sub_path_start_mut(&mut self) -> &mut Point {
        &mut self.sub_path_start
    }

    pub(crate) fn last_point(&self) -> &Point {
        &self.last_point
    }

    pub(crate) fn last_point_mut(&mut self) -> &mut Point {
        &mut self.last_point
    }

    pub(crate) fn clip_mut(&mut self) -> &mut Option<FillRule> {
        &mut self.clip
    }

    pub(crate) fn get(&self) -> &State<'a> {
        self.states.last().unwrap()
    }

    pub(crate) fn get_mut(&mut self) -> &mut State<'a> {
        self.states.last_mut().unwrap()
    }

    pub(crate) fn pre_concat_transform(&mut self, transform: Transform) {
        self.pre_concat_affine(convert_transform(transform));
    }

    pub(crate) fn pre_concat_affine(&mut self, transform: Affine) {
        self.get_mut().ctm *= transform;
    }

    pub(crate) fn get_color_space(
        &mut self,
        resources: &Resources<'_>,
        name: &Name<'_>,
    ) -> Option<ColorSpace> {
        let cs_object = resources.get_color_space(name)?;
        self.interpreter_cache
            .object_cache
            .get_or_insert_with(cs_object.cache_key(), || {
                ColorSpace::new(cs_object.clone(), &self.interpreter_cache.object_cache)
            })
    }

    pub(crate) fn stroke_props(&self) -> StrokeProps {
        self.get().graphics_state.stroke_props.clone()
    }

    pub(crate) fn num_states(&self) -> usize {
        self.states.len()
    }

    pub(crate) fn nesting_depth(&self) -> u32 {
        self.nesting_depth
    }

    pub(crate) fn begin_nested_interpretation(&mut self) -> bool {
        if self.nesting_depth >= MAX_NESTED_INTERPRETATION_DEPTH {
            warn!("interpreter nesting depth exceeded");

            return false;
        }
        if !take_nested_paint(self.nesting_depth + 1) {
            warn!("nested paint budget exceeded");

            return false;
        }

        self.nesting_depth += 1;

        true
    }

    pub(crate) fn end_nested_interpretation(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
    }
    pub(crate) fn resolve_font(&mut self, font_dict: &Dict<'a>) -> Option<TextStateFont<'a>> {
        let cache_key = font_dict.cache_key();

        let resolved = {
            let mut font_cache = self.interpreter_cache.font_cache.borrow_mut();
            font_cache
                .entry(cache_key)
                .or_insert_with(|| {
                    Font::new(
                        font_dict,
                        &self.settings.font_resolver,
                        &self.settings.cmap_resolver,
                    )
                })
                .clone()
        };

        if let Some(resolved) = resolved {
            Some(TextStateFont::Font(resolved))
        } else {
            Font::new_standard(StandardFont::Helvetica, &self.settings.font_resolver)
                .map(TextStateFont::Fallback)
        }
    }
}

pub(crate) fn path_as_rect(path: &BezPath) -> Option<Rect> {
    // One MoveTo, three LineTo, one ClosePath
    if path.elements().len() != 5 {
        return None;
    }

    let bbox = path.fast_bounding_box();
    let (min_x, min_y, max_x, max_y) = (bbox.min_x(), bbox.min_y(), bbox.max_x(), bbox.max_y());
    let mut corners = [false; 4];

    let mut check_point = |p: Point| {
        corners[0] |= p.x.is_nearly_equal(min_x) && p.y.is_nearly_equal(min_y);
        corners[1] |= p.x.is_nearly_equal(min_x) && p.y.is_nearly_equal(max_y);
        corners[2] |= p.x.is_nearly_equal(max_x) && p.y.is_nearly_equal(min_y);
        corners[3] |= p.x.is_nearly_equal(max_x) && p.y.is_nearly_equal(max_y);
    };

    for (idx, el) in path.elements().iter().enumerate() {
        match el {
            PathEl::MoveTo(p) => {
                if idx != 0 {
                    return None;
                }

                check_point(*p);
            }
            PathEl::LineTo(l) => check_point(*l),
            PathEl::QuadTo(_, _) => return None,
            PathEl::CurveTo(_, _, _) => return None,
            PathEl::ClosePath => {}
        }
    }

    if corners[0] && corners[1] && corners[2] && corners[3] {
        Some(bbox)
    } else {
        None
    }
}
