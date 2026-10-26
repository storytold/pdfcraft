use crate::cache::{Cache, CacheKey, DecodedImageCache};
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
use hayro_syntax::object::Stream;
use hayro_syntax::page::Resources;
use hayro_syntax::xref::XRef;
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use std::borrow::Cow;
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
    static CONTENT_LEFT: core::cell::Cell<usize> = const { core::cell::Cell::new(MAX_PAGE_CONTENT) };
    // PdfCraft patch (18): whether this page's content budget ran out, reported as
    // `InterpreterWarning::ContentTruncated`.
    static CONTENT_TRUNCATED: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// PdfCraft patch: decoded content-stream bytes one page may hold at once: its contents, and
/// the forms, tiling patterns and Type 3 glyph procedures being painted. Each stream already
/// stops at `MAX_DECODED_STREAM`, but forms nest fifty deep and every level keeps its content
/// while the next one runs: a form painting itself around a stream that inflates to 256 MiB
/// would have held over 12 GB. A level gives its bytes back when it finishes, so content
/// painted again and again doesn't use the budget up. Ordinary pages hold a few megabytes.
pub(crate) const MAX_PAGE_CONTENT: usize = 512 << 20;

/// PdfCraft patch: a content stream decoded within the page's [`MAX_PAGE_CONTENT`]. The bytes
/// it decoded count against the budget until it is dropped; a stream without filters is
/// borrowed from the file and costs nothing.
pub(crate) struct DecodedContent<'a> {
    data: Cow<'a, [u8]>,
    held: usize,
}

impl AsRef<[u8]> for DecodedContent<'_> {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for DecodedContent<'_> {
    fn drop(&mut self) {
        let held = self.held;
        CONTENT_LEFT.with(|c| c.set(c.get().saturating_add(held).min(MAX_PAGE_CONTENT)));
    }
}

/// PdfCraft patch: decode a content stream within what is left of this page's
/// [`MAX_PAGE_CONTENT`]. `None` once the budget is spent (the content is skipped).
pub(crate) fn decode_content<'a>(stream: &Stream<'a>) -> Option<DecodedContent<'a>> {
    let left = CONTENT_LEFT.with(core::cell::Cell::get);
    if left == 0 {
        CONTENT_TRUNCATED.with(|truncated| truncated.set(true));
        warn!("page content budget exceeded");
        return None;
    }
    let data = stream.decoded_within(left).ok()?;
    let held = match &data {
        Cow::Owned(v) => v.len(),
        Cow::Borrowed(_) => 0,
    };
    charge_content(held);
    Some(DecodedContent { data, held })
}

/// PdfCraft patch: count `len` bytes held for the rest of the page (its own contents).
pub(crate) fn charge_content(len: usize) {
    CONTENT_LEFT.with(|c| c.set(c.get().saturating_sub(len)));
}

/// PdfCraft patch (18): whether content was skipped because the page's budget ran out.
pub(crate) fn content_was_truncated() -> bool {
    CONTENT_TRUNCATED.with(core::cell::Cell::get)
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
    /// PdfCraft patch: decoded images, shared by every render through this cache so a tile
    /// no longer decodes its page's images again.
    pub(crate) decoded_cache: DecodedImageCache,
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
            decoded_cache: DecodedImageCache::new(),
        }
    }

    /// PdfCraft patch (32): how many images were decoded through this cache (its misses, not
    /// counting images that are never cached), for diagnostics and tests.
    pub fn decoded_image_decodes(&self) -> usize {
        self.decoded_cache.decodes()
    }

    /// PdfCraft patch (48): how many `/ICCBased` profiles were read through this cache, for
    /// diagnostics and tests. A profile used again, by the same stream object or by a copy with
    /// the same dictionary and bytes, is read once.
    pub fn icc_profile_reads(&self) -> usize {
        self.object_cache.icc_profile_reads()
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
    /// PdfCraft patch: estimated bytes held by saved states, and for each `q` not saved because
    /// a limit was reached, the clip count to return to at its `Q`.
    saved_bytes: usize,
    skipped_saves: Vec<usize>,
    // PdfCraft patch (63).
    /// This is the content of a luminosity soft mask whose group colour space is `/DeviceCMYK`,
    /// so a `/DeviceCMYK` fill or stroke colour paints as its gray (ISO 32000-2 §11.5.3,
    /// §10.4.2.3) rather than through the CMYK profile used for display.
    pub(crate) luminosity_in_device_cmyk: bool,
}

/// PdfCraft patch: most graphics states saved at once. Every `q` copies the current state, dash
/// array and clip list included, so without a limit short operators multiply one large state.
const MAX_SAVED_STATES: usize = 4096;
/// PdfCraft patch: most bytes all saved states may hold together.
const MAX_SAVED_STATE_BYTES: usize = 64 << 20;

/// Roughly what a saved copy of `state` holds.
fn state_bytes(state: &State<'_>) -> usize {
    size_of::<State<'_>>()
        .saturating_add(
            state
                .graphics_state
                .stroke_props
                .dash_array
                .len()
                .saturating_mul(size_of::<f32>()),
        )
        .saturating_add(state.clips.len().saturating_mul(size_of::<ClipType>()))
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
        // PdfCraft patch: a page (or other top-level content) starts with fresh budgets.
        NESTED_PAINTS.with(|n| n.set(0));
        CONTENT_LEFT.with(|c| c.set(MAX_PAGE_CONTENT));
        CONTENT_TRUNCATED.with(|truncated| truncated.set(false));
        // PdfCraft patch (55): a fallback left over from another page is not this page's.
        crate::color::take_color_fallback();
        crate::encode::reset_mesh_budget();

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
            saved_bytes: 0,
            skipped_saves: Vec::new(),
            luminosity_in_device_cmyk: false,
        }
    }

    pub(crate) fn save_state(&mut self) {
        let Some(cur) = self.states.last() else {
            warn!("attempted to save state without existing state");
            return;
        };

        // PdfCraft patch: past either limit the `q` is recorded but nothing is copied. Its `Q`
        // still removes the clips added since; other changes made in between are kept.
        let bytes = state_bytes(cur);
        if !self.skipped_saves.is_empty()
            || self.states.len() >= MAX_SAVED_STATES
            || self.saved_bytes.saturating_add(bytes) > MAX_SAVED_STATE_BYTES
        {
            if self.skipped_saves.is_empty() {
                warn!("too many saved graphics states; further saves are not kept");
            }
            self.skipped_saves.push(cur.clips.len());
            return;
        }

        let cur = cur.clone();
        self.saved_bytes = self.saved_bytes.saturating_add(bytes);
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
        // PdfCraft patch: a save that was not kept is the most recent one.
        if let Some(target_clips) = self.skipped_saves.pop() {
            while self.get().clips.len() > target_clips {
                self.pop_clip_path(device);
            }
            return;
        }

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
            // PdfCraft patch: the state now on top is the copy that was charged when saved.
            if let Some(top) = self.states.last() {
                self.saved_bytes = self.saved_bytes.saturating_sub(state_bytes(top));
            }
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
        // PdfCraft patch: saves not kept still count, so callers restore back past them.
        self.states.len().saturating_add(self.skipped_saves.len())
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

    // PdfCraft patch: the points have to trace the rectangle, not merely reach its corners. The
    // length check above admits one MoveTo and four LineTo as well as the intended one MoveTo,
    // three LineTo and one ClosePath, and asking only that each corner of the bounding box was
    // visited says nothing about the order or the steps between: a five-point polyline whose
    // ends reached every corner of its own bounding box was painted as that box, and a clip of
    // that shape was discarded. Four distinct points, each step along one axis, the last point
    // back at the first: that is a rectangle and nothing else is.
    let mut points: Vec<Point> = Vec::with_capacity(5);
    let mut closed = false;
    for (idx, el) in path.elements().iter().enumerate() {
        match el {
            PathEl::MoveTo(p) => {
                if idx != 0 {
                    return None;
                }
                points.push(*p);
            }
            PathEl::LineTo(p) => points.push(*p),
            PathEl::QuadTo(_, _) => return None,
            PathEl::CurveTo(_, _, _) => return None,
            // A subpath that ends and starts again is more than one shape.
            PathEl::ClosePath => {
                if idx + 1 != path.elements().len() {
                    return None;
                }
                closed = true;
            }
        }
    }

    // Exactly equal, not nearly: `is_nearly_equal` allows a 256th of a path unit, and the matrix
    // is applied after this, so a `cm` of 10,000 turns that into tens of device units -- a path
    // that only nearly closes would be painted as the rectangle, with a side it does not have.
    // A rectangle's coordinates are equal in the content stream, not approximately equal, so
    // nothing that is one loses the fast path by this; a path that misses takes the general route
    // and is painted as written (#3327 review).
    let same = |a: Point, b: Point| a.x == b.x && a.y == b.y;
    // `m l l l l`: the fourth line goes back to the start, which `m l l l h` does with ClosePath.
    if !closed {
        if points.len() != 5 || !same(points[4], points[0]) {
            return None;
        }
        points.pop();
    }
    if points.len() != 4 {
        return None;
    }
    // Distinct, so a degenerate path is not one, and each step along one axis only: the four
    // points of an axis-aligned rectangle, in order round it.
    for (i, p) in points.iter().enumerate() {
        let q = points[(i + 1) % 4];
        let along_x = p.y == q.y && p.x != q.x;
        let along_y = p.x == q.x && p.y != q.y;
        if !(along_x || along_y) {
            return None;
        }
        if points.iter().skip(i + 1).any(|o| same(*p, *o)) {
            return None;
        }
    }

    Some(path.fast_bounding_box())
}

/// PdfCraft patch tests (ported from craft-libs): the graphics-state stack is bounded by count
/// and by bytes, and every `q` still pairs with its `Q`.
#[cfg(test)]
mod saved_state_tests {
    use super::*;
    use crate::device::DummyDevice;
    use hayro_syntax::Pdf;

    fn pdf() -> Pdf {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Count 0 /Kids [] >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 3\n0000000000 65535 f \n");
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        Pdf::new(out).expect("synthetic pdf")
    }

    fn triangle() -> BezPath {
        let mut path = BezPath::new();
        path.move_to((10.0, 10.0));
        path.line_to((90.0, 10.0));
        path.line_to((50.0, 90.0));
        path.close_path();
        path
    }

    /// Saves `saves` more times, checks a clip added after the last save goes at its `Q`, and unwinds.
    fn save_and_unwind(ctx: &mut Context<'_>, saves: usize) {
        let before = ctx.num_states();
        for _ in 0..saves {
            ctx.save_state();
        }
        assert!(ctx.states.len() <= MAX_SAVED_STATES);
        assert!(ctx.saved_bytes <= MAX_SAVED_STATE_BYTES);
        assert_eq!(ctx.num_states(), before + saves, "every q still counts");

        let mut device = DummyDevice;
        let clips = ctx.get().clips.len();
        ctx.push_clip_path(triangle(), FillRule::NonZero, &mut device);
        assert_eq!(ctx.get().clips.len(), clips + 1);
        ctx.restore_state(&mut device);
        assert_eq!(ctx.get().clips.len(), clips);

        while ctx.num_states() > 1 {
            ctx.restore_state(&mut device);
        }
        assert_eq!(ctx.states.len(), 1);
        assert!(ctx.skipped_saves.is_empty());
        assert_eq!(ctx.saved_bytes, 0);
    }

    #[test]
    fn repeated_saves_stay_bounded_and_balanced() {
        let pdf = pdf();
        let cache = InterpreterCache::new();
        let bbox = Rect::new(0.0, 0.0, 100.0, 100.0);
        let settings = InterpreterSettings::default();
        let mut ctx = Context::new(Affine::IDENTITY, bbox, &cache, pdf.xref(), settings);

        // A small state reaches the count limit.
        save_and_unwind(&mut ctx, 20_000);

        // A large one (a 64 Ki-entry dash array, 256 KiB a copy) reaches the byte limit first.
        ctx.get_mut().graphics_state.stroke_props.dash_array = (0..1 << 16).map(|_| 2.0).collect();
        ctx.save_state();
        assert_eq!(ctx.states.len(), 2);
        while ctx.skipped_saves.is_empty() {
            ctx.save_state();
        }
        assert!(ctx.states.len() < MAX_SAVED_STATES);
        save_and_unwind(&mut ctx, 1000);

        // Once unwound, a kept save restores what changed after it.
        let mut device = DummyDevice;
        ctx.save_state();
        ctx.pre_concat_affine(Affine::scale(2.0));
        ctx.restore_state(&mut device);
        assert_eq!(ctx.get().ctm, Affine::IDENTITY);
    }
}
