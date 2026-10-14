//! Transparency flattening for the PostScript exporter (Wave 2): the shadow scene.
//!
//! PDF transparency — blend modes, soft masks, constant alpha — has no vector PostScript
//! equivalent, so regions that use it are rasterized through hayro's own `Device`
//! implementation ([`hayro::OffscreenRenderer`]): a flattened region composites exactly
//! as `page_render` composites it, by construction.
//!
//! vello_cpu's render context is immediate-mode — `render_to_buffer` asserts that no
//! layers are open — so a page cannot be rasterized mid-drawing and a long-lived scene
//! object cannot be asked for pixels. [`ShadowScene`] is therefore a *replay log*:
//! every draw the PostScript device emits is recorded as a closure over cloned paths,
//! paints and transforms (images are cloned by value — the vendor `Clone` patch — and
//! glyph outlines come from the scene's own cache, [`ShadowScene::outline_of`], keyed
//! like the rasterizer's). Recorded soft masks re-root onto the scene's pixel space
//! (`SoftMask::re_rooted`, the vendor patch): the interpreter captured them in device
//! page points (y up), while the replay renders in 300-dpi pixels (y down). Each
//! [`ShadowScene::flatten`] builds a fresh page-sized
//! renderer at 300 dpi, paints the white page base first (the same base `page_render`
//! starts from, and what makes the emitted raster opaque), replays the log through the
//! balanced prefix, pops any wrappers still open in reverse, finishes, and crops the
//! region. Recording is cheap — closures over cloned values — so pages with no
//! transparency never pay a render.
//!
//! Every flatten is bounded: the page and the region must each stay under
//! [`MAX_FLATTEN_AREA_PX`] and inside vello's u16 pixmap sides, at most
//! [`MAX_FLATTENS_PER_PAGE`] flattens happen per page, and a panic anywhere in the
//! replay poisons the scene — that flatten and every later one declines, so the caller
//! falls back to today's vector output plus its disclosure note, and the process never
//! crashes twice on the same page.
//!
//! One draw cannot be recorded at all: an image whose stream bytes are transient
//! (the interpreter hands inline images over with a borrow of the content window it
//! parsed them from, a lifetime no `'a`-bounded closure can capture). Such a draw
//! records as a *hole* — its device-space box, no closure — and any flatten whose
//! region intersects a hole declines: a raster either replays every draw it crops or
//! is not produced at all, never with a gap where an image was. XObject images (every
//! stream that borrows the document) record by value through the vendor's
//! `draw_retained_image` entry and are not holes.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use hayro::OffscreenRenderer;
use hayro::hayro_interpret::color::{AlphaColor, Color};
use hayro::hayro_interpret::font::OutlineGlyph;
use hayro::hayro_interpret::{BlendMode, CacheKey, ClipPath, Device, FillRule, Paint, PathDrawMode, SoftMask};
use hayro::vello_cpu::Pixmap;
use hayro::vello_cpu::color::palette::css::WHITE;
use kurbo::{Affine, BezPath, Rect};

/// The resolution flattened regions rasterize at. Fixed by the spec (no tool
/// options): deterministic tests, and transparent regions are photographs of
/// vectors, where 300 dpi is beyond what a printer resolves.
pub(crate) const FLATTEN_DPI: f64 = 300.0;

/// The pixel-area ceiling for one flatten (page or region): a hostile full-bleed
/// group declines (the caller flushes the vector and keeps today's note) instead of
/// allocating a raster that size. 100 Mpx is A3 at 300 dpi with margin.
pub(crate) const MAX_FLATTEN_AREA_PX: u64 = 100_000_000;

/// How many flattens one exported page may spend. Each is a full page render, so a
/// document of stacked groups cannot grind export to a halt; the rest decline.
pub(crate) const MAX_FLATTENS_PER_PAGE: u32 = 128;

/// The matrix that maps the device's y-up page space onto 300-dpi pixels: the page's
/// top-left corner becomes pixel (0, 0) and y runs down the page. This is the flip
/// `render_into` bakes in through `initial_transform(true)` (study its composition
/// there), adapted so the origin lands at `page`'s top-left corner whatever the
/// page's own coordinates: translate(0, H_px) · scale(s, −s) · translate(−x0, −y0).
pub(crate) fn page_matrix(page: Rect) -> Affine {
    let s = FLATTEN_DPI / 72.0;
    Affine::translate((0.0, page.height() * s)) * Affine::scale_non_uniform(s, -s) * Affine::translate((-page.x0, -page.y0))
}

/// How one log entry moves the offscreen renderer's wrapper (layer/clip) stack.
/// `flatten` tracks this while replaying so it can pop everything still open in
/// reverse before `finish` — vello's rasterizer asserts on open layers — and so a
/// pop with no matching push (hostile content) is skipped instead of reaching vello.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Balance {
    /// A draw or state change; no wrapper movement.
    Draw,
    /// `push_transparency_group`, opening one group layer.
    PushGroup,
    /// `pop_transparency_group`, closing the innermost group layer.
    PopGroup,
    /// `push_clip_path`, opening one clip.
    PushClip,
    /// `pop_clip_path`, closing the innermost clip.
    PopClip,
}

/// One recorded operation: a closure that replays it onto a fresh renderer, plus how
/// it moves the wrapper stack. Closures must be replayable any number of times (the
/// log stays after a flatten, so later groups replay it again) and self-contained:
/// by-value arguments (`Image`, `SoftMask`) are cloned *inside* the closure, once
/// per replay.
struct Entry<'a> {
    op: Box<dyn FnMut(&mut OffscreenRenderer) + 'a>,
    balance: Balance,
}

/// One real-transparency group whose pop will flatten: the union of the device-space
/// boxes of every draw inside it (and inside its nested groups — `extend` widens
/// every open frame, so an outer group's pop flattens the union that covers its
/// inner groups' content).
#[derive(Clone, Copy)]
pub(crate) struct Frame {
    /// The union of the boxes of the draws recorded since the frame opened, in
    /// device page space.
    pub(crate) bbox: Rect,
    /// Whether no draw has reached the frame yet (`bbox` is still meaningless).
    pub(crate) empty: bool,
}

/// One flattened region: straight RGB rows (row 0 first — the top of the region) and
/// the matrix that places those pixels back onto the device page space.
pub(crate) struct FlattenRaster {
    /// The region's width in pixels.
    pub(crate) width: u32,
    /// The region's height in pixels.
    pub(crate) height: u32,
    /// `width × height` RGB triplets, row 0 (the region's top) first.
    pub(crate) rgb: Vec<u8>,
    /// Maps pixel space (row 0 first, y down) back onto device page space (y up):
    /// `translate(region.x0, region.y1) · scale(72/300, −72/300)`. The caller
    /// concatenates this with its transform and emits the raster through
    /// `images::opaque_raster`.
    pub(crate) place: Affine,
}

/// The shadow scene: every draw of one exported page, recorded as replay closures,
/// plus the page matrix that maps the device's y-up page space onto 300-dpi pixels
/// (y down, origin at the page's top-left corner).
pub(crate) struct ShadowScene<'a> {
    /// The page box in device page space; regions clip to it.
    page: Rect,
    /// [`page_matrix`] over `page`.
    matrix: Affine,
    /// The replay log, in recording order. Survives every flatten: a later
    /// overlapping group must composite over exactly what `page_render` composites
    /// over — the raster equals the draws' render, so the spec's "recorded back
    /// into the scene as an opaque image" holds by equivalence, without a
    /// second-generation raster.
    log: Vec<Entry<'a>>,
    /// Frames for the real-transparency groups currently open, outermost first.
    frames: Vec<Frame>,
    /// The frame closed by the most recent `pop_frame` — the region `flatten` crops.
    last_frame: Option<Frame>,
    /// Flattens spent on this page so far.
    flattens: u32,
    /// A panic escaped a replay once; every flatten from now on declines.
    poisoned: bool,
    /// Glyph outlines by the rasterizer's own key (`OutlineGlyph::identifier`'s
    /// cache key, font + glyph id) — the `Renderer::outline_cache` precedent.
    outline_cache: HashMap<u128, Rc<BezPath>>,
    /// Boxes of the draws that could not be recorded (transient-stream images): a
    /// flatten whose region intersects one declines (see [`ShadowScene::record_image_hole`]).
    holes: Vec<Rect>,
}

impl<'a> ShadowScene<'a> {
    /// A scene over one exported page (`page` in device page space — the same box
    /// the device's own DSC page bounding box uses).
    pub(crate) fn new(page: Rect) -> Self {
        Self {
            matrix: page_matrix(page),
            page,
            log: Vec::new(),
            frames: Vec::new(),
            last_frame: None,
            flattens: 0,
            poisoned: false,
            outline_cache: HashMap::new(),
            holes: Vec::new(),
        }
    }

    /// Record one replayable draw or state change. The closure drives the fresh
    /// `OffscreenRenderer`'s `Device` methods and applies `matrix` itself — the
    /// caller bakes the page matrix into the transforms it captures. The log replays
    /// at every flatten, so the closure must be callable repeatedly: clone by-value
    /// arguments (`Image`, `SoftMask`) inside, once per call.
    pub(crate) fn record(&mut self, op: impl FnMut(&mut OffscreenRenderer) + 'a) {
        self.log.push(Entry { op: Box::new(op), balance: Balance::Draw });
    }

    /// Record a `push_transparency_group` (page wrappers and real groups alike — the
    /// scene does not distinguish them; only `push_frame` opens a flattening frame).
    /// The mask re-roots onto the scene's pixel space inside the closure, once per
    /// replay: the interpreter captured it in device page points (y up), while the
    /// offscreen renderer draws in 300-dpi pixels (y down) — the same page matrix
    /// every recorded path bakes in (`SoftMask::re_rooted`, the vendor patch).
    pub(crate) fn record_push_group(&mut self, opacity: f32, mask: Option<SoftMask<'a>>, blend: BlendMode) {
        let matrix = self.matrix;
        self.log.push(Entry {
            op: Box::new(move |r| r.push_transparency_group(opacity, mask.clone().map(|m| m.re_rooted(matrix)), blend)),
            balance: Balance::PushGroup,
        });
    }

    /// Record a `set_soft_mask` (the interpreter re-announces the graphics state's
    /// mask before every draw). The mask re-roots onto the scene's pixel space at
    /// replay time exactly as [`ShadowScene::record_push_group`]'s does, so a draw
    /// masked outside any group composites through the same mask a later flatten of
    /// the region replays.
    pub(crate) fn record_soft_mask(&mut self, mask: Option<SoftMask<'a>>) {
        let matrix = self.matrix;
        self.log.push(Entry { op: Box::new(move |r| r.set_soft_mask(mask.clone().map(|m| m.re_rooted(matrix)))), balance: Balance::Draw });
    }

    /// Record the `pop_transparency_group` closing the innermost group still open.
    pub(crate) fn record_pop_group(&mut self) {
        self.log.push(Entry { op: Box::new(|r| r.pop_transparency_group()), balance: Balance::PopGroup });
    }

    /// Record a `push_clip_path`. The clip arrives in device page space, like every
    /// path the device emits; the page matrix is applied at record time, so the
    /// replayed clip is in pixel space.
    pub(crate) fn record_push_clip(&mut self, clip: &ClipPath) {
        let mut path = clip.path.clone();
        path.apply_affine(self.matrix);
        let mapped = ClipPath { path, fill: clip.fill };
        self.log.push(Entry { op: Box::new(move |r| r.push_clip_path(&mapped)), balance: Balance::PushClip });
    }

    /// Record the `pop_clip_path` closing the innermost clip still open.
    pub(crate) fn record_pop_clip(&mut self) {
        self.log.push(Entry { op: Box::new(|r| r.pop_clip_path()), balance: Balance::PopClip });
    }

    /// Record one image draw the replay log cannot carry: the interpreter hands such
    /// an image over as `Image<'a, '_>` whose stream bytes borrow something shorter
    /// than `'a` (an inline image borrows the content window it was parsed from), and
    /// no `'a`-bounded closure can capture it. `bbox` is the draw's device-space box
    /// (the unit rect × its transform); [`ShadowScene::flatten`] declines whenever a
    /// region intersects a hole, so the raster either replays every draw it crops or
    /// is not produced at all — never a gap where an image was.
    pub(crate) fn record_image_hole(&mut self, bbox: Rect) {
        self.holes.push(bbox);
    }

    /// A real-transparency group opens: starts at an empty box, widened by
    /// [`ShadowScene::extend`] as its draws record.
    pub(crate) fn push_frame(&mut self) {
        self.frames.push(Frame { bbox: Rect::ZERO, empty: true });
    }

    /// Close the innermost frame. `None` when no frame is open — the device's own
    /// group stack already guards balance, so an unbalanced pop never reaches here.
    /// The returned frame (also remembered as the region of the next `flatten`) is
    /// the union that covers everything recorded since the matching
    /// `push_transparency_group`, nested groups included.
    pub(crate) fn pop_frame(&mut self) -> Option<Frame> {
        let frame = self.frames.pop()?;
        self.last_frame = Some(frame);
        Some(frame)
    }

    /// Widen **every** open frame's bbox to cover `bbox`, a draw's device-space box:
    /// paths their control-point bbox × transform grown by the stroke's reach (half
    /// the line width times the larger of the miter limit and √2 — the rasterizer's
    /// own `may_paint_canvas` formula — plus 0.5 user units of flattening slack
    /// before the transform and 2 pixels of antialiasing after it); images the unit
    /// rect × transform; glyphs the cached outline's bbox × their total transform.
    /// The first box an empty frame sees becomes its bbox outright (never a union
    /// with the origin).
    pub(crate) fn extend(&mut self, bbox: Rect) {
        for frame in &mut self.frames {
            if frame.empty {
                frame.bbox = bbox;
                frame.empty = false;
            } else {
                frame.bbox = frame.bbox.union(bbox);
            }
        }
    }

    /// Replay the log into a fresh page-sized renderer (white page fill first), pop
    /// everything still open in reverse — an enclosing group's params apply, which is
    /// correct for the final page and disclosed-consistent when a decline flushes —
    /// then crop the region of the frame closed by the most recent `pop_frame`,
    /// intersected with the page, at 300 dpi.
    ///
    /// An empty region is a `width: 0, height: 0` raster (the caller emits nothing).
    /// `None` means *declined*: the scene is poisoned, the page or region exceeds
    /// [`MAX_FLATTEN_AREA_PX`] or vello's u16 pixmap sides, [`MAX_FLATTENS_PER_PAGE`]
    /// is spent, no frame was ever popped, the region touches a recorded image hole
    /// ([`ShadowScene::record_image_hole`]), or the replay panicked (which poisons the
    /// scene) — the caller flushes the withheld vector and keeps today's note.
    pub(crate) fn flatten(&mut self) -> Option<FlattenRaster> {
        if self.poisoned || self.flattens >= MAX_FLATTENS_PER_PAGE {
            return None;
        }
        // The page must rasterize at all: finite, positive, inside vello's u16
        // pixmap sides and the area cap.
        let scale = FLATTEN_DPI / 72.0;
        let (wpx, hpx) = (self.page.width() * scale, self.page.height() * scale);
        if !self.page.is_finite() || !wpx.is_finite() || !hpx.is_finite() {
            return None;
        }
        let side_max = f64::from(u16::MAX);
        if !(1.0..=side_max).contains(&wpx) || !(1.0..=side_max).contains(&hpx) {
            return None;
        }
        // Floored, whole, and within u16 by the range check above, so the casts are
        // exact; the u16 product cannot overflow u64, but checked stays cheap.
        let (pw, ph) = (wpx.floor() as u16, hpx.floor() as u16);
        let page_area = u64::from(pw).checked_mul(u64::from(ph))?;
        if page_area > MAX_FLATTEN_AREA_PX {
            return None;
        }

        let frame = self.last_frame?;

        // The region: the frame's box clipped to the page, snapped out to whole
        // pixels through the page matrix. A non-finite box declines.
        if !frame.bbox.is_finite() {
            return None;
        }
        let region = frame.bbox.intersect(self.page);
        // A region that touches a hole (an image draw the log cannot carry) declines
        // before any budget is spent: its replay could not paint that image, so the
        // raster would show a gap — the caller falls back to the vector instead.
        // Touching edges count (inclusive): the region snaps out to whole pixels,
        // so a draw exactly on its boundary reaches the snapped pixel.
        if self.holes.iter().any(|hole| boxes_touch(*hole, region)) {
            return None;
        }
        self.flattens = self.flattens.saturating_add(1);
        let px = self.matrix.transform_rect_bbox(region);
        let (x0, x1) = (px.x0.floor().clamp(0.0, f64::from(pw)), px.x1.ceil().clamp(0.0, f64::from(pw)));
        let (y0, y1) = (px.y0.floor().clamp(0.0, f64::from(ph)), px.y1.ceil().clamp(0.0, f64::from(ph)));
        if ![x0, x1, y0, y1].iter().all(|v| v.is_finite()) {
            return None;
        }
        // Both differences are whole and clamped inside [0, pw/ph], so the casts are
        // exact and bounded by u16::MAX.
        let (w, h) = ((x1 - x0) as u64, (y1 - y0) as u64);
        let area = w.checked_mul(h)?;
        if w > u64::from(u16::MAX) || h > u64::from(u16::MAX) || area > MAX_FLATTEN_AREA_PX {
            return None;
        }
        let place = placement(region);
        if w == 0 || h == 0 {
            return Some(FlattenRaster { width: 0, height: 0, rgb: Vec::new(), place });
        }
        let (x0u, y0u, wu, hu) = (x0 as u32, y0 as u32, w as u32, h as u32);

        // The render, under the panic guard: one panic poisons the scene forever.
        let rendered = catch_unwind(AssertUnwindSafe(|| {
            let mut renderer = OffscreenRenderer::new(pw, ph);
            // vello's WHITE — the same base `render_into` clears its pixmap with —
            // carried into hayro's own color type for the `Device` call.
            let w = WHITE.to_rgba8();
            let white = Paint::Color(Color::from_rgba(AlphaColor::from_rgba8(w.r, w.g, w.b, w.a)));
            renderer.draw_rect(&Rect::new(0.0, 0.0, f64::from(pw), f64::from(ph)), Affine::IDENTITY, &white, &PathDrawMode::Fill(FillRule::NonZero));
            let mut open: Vec<Balance> = Vec::new();
            for entry in self.log.iter_mut() {
                let replays = match entry.balance {
                    Balance::PushGroup | Balance::PushClip => {
                        open.push(entry.balance);
                        true
                    }
                    Balance::PopGroup | Balance::PopClip => open.pop().is_some(),
                    Balance::Draw => true,
                };
                if replays {
                    (entry.op)(&mut renderer);
                }
            }
            // Wrappers still open (the page clip and group, any form's bbox clip)
            // close in reverse so `finish` meets vello's balanced-layers assert.
            for wrapper in open.into_iter().rev() {
                match wrapper {
                    Balance::PushGroup => renderer.pop_transparency_group(),
                    Balance::PushClip => renderer.pop_clip_path(),
                    _ => {}
                }
            }
            let pixmap = renderer.finish();
            crop(&pixmap, x0u, y0u, wu, hu).map(|rgb| FlattenRaster { width: wu, height: hu, rgb, place })
        }));
        match rendered {
            Ok(Some(raster)) => Some(raster),
            // A checked rail inside the crop tripped (unreachable behind the caps):
            // decline without poisoning.
            Ok(None) => None,
            Err(_) => {
                self.poisoned = true;
                None
            }
        }
    }

    /// The glyph's outline, cached by the rasterizer's own key (font + glyph id, the
    /// `Renderer::outline_cache` precedent). What the flattener records is exactly
    /// what the raster renderer paints: this path × the glyph's total transform.
    pub(crate) fn outline_of(&mut self, glyph: &OutlineGlyph) -> Rc<BezPath> {
        let key = glyph.identifier().cache_key();
        if let Some(path) = self.outline_cache.get(&key) {
            return Rc::clone(path);
        }
        let path = Rc::new(glyph.outline());
        self.outline_cache.insert(key, Rc::clone(&path));
        path
    }
}

/// Whether two boxes touch at all (a shared edge counts): the inclusive overlap
/// test `Rect::intersects` of newer kurbo versions.
fn boxes_touch(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// Crop `w × h` pixels at (`x0`, `y0`) out of a finished page pixmap into straight
/// RGB rows, row 0 (the top of the region) first. The white page base makes every
/// alpha 255, so the premultiplied RGBA rows copy as straight RGB. `None` only if a
/// checked rail trips — unreachable behind the caps, but never a panic.
fn crop(pixmap: &Pixmap, x0: u32, y0: u32, w: u32, h: u32) -> Option<Vec<u8>> {
    let stride = u32::from(pixmap.width()).checked_mul(4)?;
    let row_bytes = w.checked_mul(4)?;
    let len = w.checked_mul(h)?.checked_mul(3)?;
    let mut rgb = Vec::new();
    // The caps already bounded the region; reserving can still fail on a full disk,
    // and that must decline, not abort.
    rgb.try_reserve_exact(len as usize).ok()?;
    let data = pixmap.data_as_u8_slice();
    for row in 0..h {
        let y = y0.checked_add(row)?;
        let start = y.checked_mul(stride)?.checked_add(x0.checked_mul(4)?)?;
        let end = start.checked_add(row_bytes)?;
        let rgba = data.get(start as usize..end as usize)?;
        for &[r, g, b, _] in rgba.as_chunks::<4>().0 {
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    Some(rgb)
}

/// The matrix mapping a flattened raster's pixels (row 0 first, x right, y down)
/// back onto the device page space it was cropped from: the region's top-left
/// corner, 72/300 pt per pixel, y flipped back up.
fn placement(region: Rect) -> Affine {
    Affine::translate((region.x0, region.y1)) * Affine::scale_non_uniform(72.0 / FLATTEN_DPI, -72.0 / FLATTEN_DPI)
}

#[cfg(test)]
mod tests {
    use hayro::OffscreenRenderer;
    use hayro::hayro_interpret::color::{AlphaColor, Color};
    use hayro::hayro_interpret::font::Glyph;
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::{
        BlendMode, ClipPath, Context, Device, FillRule, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask,
        interpret_page,
    };
    use kurbo::{Affine, BezPath, Rect};

    /// A `<< … >> stream … endstream` object body, in the hand-written style of
    /// `crates/ps-export/src/lib.rs`'s tests (length computed, not counted by hand).
    fn stream_obj(dict: &str, body: &[u8]) -> Vec<u8> {
        let mut obj = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        obj.extend_from_slice(body);
        obj.extend_from_slice(b"\nendstream");
        obj
    }

    /// Assemble a PDF from object bodies (numbered from 1), computing the xref like
    /// `crates/ps-export/src/lib.rs`'s tests.
    fn assemble(objs: Vec<Vec<u8>>) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// A one-page PDF drawing two image XObjects: a 2×2 `/DeviceRGB` raster and a 2×2
    /// `/ImageMask` stencil (the replay log must capture both `Image` variants by value).
    fn image_pdf() -> Vec<u8> {
        // Two rows of one byte each: a 2-wide row uses the top 2 bits. Samples 0,0,1,1:
        // under `/Decode [0 1]` the zeros paint, so the left column of each row is ink.
        let mask: [u8; 2] = [0x30, 0x30];
        let raster: [u8; 12] = [
            255, 0, 0, 255, 0, 0, //
            0, 255, 0, 0, 255, 0,
        ];
        assemble(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 50] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R /Msk0 6 0 R >> >> >>".to_vec(),
            stream_obj("", b"q 10 0 0 10 10 10 cm /Im0 Do Q\nq 10 0 0 10 50 10 cm /Msk0 Do Q"),
            stream_obj("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8", &raster),
            stream_obj("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ImageMask true /BitsPerComponent 1 /Decode [0 1]", &mask),
        ])
    }

    /// A `DummyDevice` that proves every `draw_image` argument can be captured by value:
    /// each received `Image` is cloned in place (the compile-time assertion this test
    /// exists for) and the clone's dimensions are compared with the original's.
    #[derive(Default)]
    struct CloningDevice {
        /// `(variant, original width, cloned width)` per received image.
        images: Vec<(&'static str, u32, u32)>,
    }

    impl Device<'_> for CloningDevice {
        fn set_soft_mask(&mut self, _: Option<SoftMask<'_>>) {}
        fn set_blend_mode(&mut self, _: BlendMode) {}
        fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'_>, _: &PathDrawMode) {}
        fn push_clip_path(&mut self, _: &ClipPath) {}
        fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'_>>, _: BlendMode) {}
        fn draw_glyph(&mut self, _: &Glyph<'_>, _: Affine, _: Affine, _: &Paint<'_>, _: &GlyphDrawMode) {}
        fn draw_image(&mut self, image: Image<'_, '_>, _: Affine) {
            // The replay log captures images by value: `Image` must be `Clone`.
            let cloned = image.clone();
            let variant = match &image {
                Image::Stencil(_) => "stencil",
                Image::Raster(_) => "raster",
            };
            self.images.push((variant, image.width(), cloned.width()));
        }
        fn pop_clip_path(&mut self) {}
        fn pop_transparency_group(&mut self) {}
    }

    /// The offscreen renderer is the same `Device` hayro's `page_render` drives, so paint
    /// must flow through it: a white rect over the transparent 4×4 context leaves an
    /// opaque white corner pixel (RGBA 255,255,255,255) in the finished pixmap.
    #[test]
    fn offscreen_renderer_paints_a_rect() {
        let mut renderer = OffscreenRenderer::new(4, 4);
        let white = Paint::Color(Color::from_rgba(AlphaColor::new([1.0, 1.0, 1.0, 1.0])));
        renderer.draw_rect(&Rect::new(0.0, 0.0, 4.0, 4.0), Affine::IDENTITY, &white, &PathDrawMode::Fill(FillRule::NonZero));
        let pixmap = renderer.finish();
        let corner = pixmap.sample(0, 0);
        assert_eq!(
            [corner.r, corner.g, corner.b, corner.a],
            [255, 255, 255, 255],
            "the white fill must reach the pixmap (the context starts transparent)"
        );
    }

    /// Both `Image` variants reach `draw_image` and are cloneable by value, and each
    /// clone reports the same dimensions as its original — the vendor `Clone` patch the
    /// flattener's replay log depends on.
    #[test]
    fn replay_images_are_cloneable() {
        let pdf = Pdf::new(image_pdf()).expect("fixture parses");
        let page = pdf.pages().first().expect("fixture has a page");
        let cache = InterpreterCache::new();
        let mut state = Context::new(Affine::IDENTITY, Rect::new(0.0, 0.0, 100.0, 50.0), &cache, page.xref(), InterpreterSettings::default());
        let mut device = CloningDevice::default();
        interpret_page(page, &mut state, &mut device);

        assert_eq!(device.images.len(), 2, "both XObjects must reach draw_image: {:?}", device.images);
        assert_eq!(device.images[0], ("raster", 2, 2), "the raster image clones with its dimensions");
        assert_eq!(device.images[1], ("stencil", 2, 2), "the stencil image clones with its dimensions");
    }

    // ------------------------------------------------------ the shadow scene

    use super::{FLATTEN_DPI, MAX_FLATTEN_AREA_PX, MAX_FLATTENS_PER_PAGE, ShadowScene, page_matrix};
    use kurbo::Point;
    use kurbo::Shape;

    /// The page matrix anchors pixel (0, 0) at the page's top-left corner and runs y
    /// down the page — the flip `render_into` bakes in through
    /// `initial_transform(true)`, here composed over an offset page so the origin
    /// subtraction is exercised too.
    #[test]
    fn page_matrix_flips_the_page() {
        let page = Rect::new(10.0, 20.0, 110.0, 220.0);
        let matrix = page_matrix(page);
        let s = FLATTEN_DPI / 72.0;
        let top_left = matrix * Point::new(10.0, 220.0);
        assert!(top_left.x.abs() < 1e-9 && top_left.y.abs() < 1e-9, "page top-left → pixel origin: {top_left:?}");
        let bottom_left = matrix * Point::new(10.0, 20.0);
        assert!(bottom_left.x.abs() < 1e-9 && (bottom_left.y - 200.0 * s).abs() < 1e-9, "page bottom-left → last row: {bottom_left:?}");
        let top_right = matrix * Point::new(110.0, 220.0);
        assert!((top_right.x - 100.0 * s).abs() < 1e-9 && top_right.y.abs() < 1e-9, "page top-right → first row's right edge: {top_right:?}");
    }

    /// Scene replay is pixel-for-pixel what rendering the same draws directly
    /// produces: the white page base, the page wrapper (clip + full-opacity group,
    /// still open at flatten time), a half-opacity group holding one red rect, popped
    /// — the flatten raster must equal the direct render over the frame's region, and
    /// the region's interior must be the red-over-white blend vello composites.
    #[test]
    fn replay_matches_direct_render() {
        let page = Rect::new(0.0, 0.0, 100.0, 100.0);
        let matrix = page_matrix(page);
        let s = FLATTEN_DPI / 72.0;
        let side = (100.0 * s).floor() as u16;

        let red = Paint::Color(Color::from_rgba(AlphaColor::new([1.0, 0.0, 0.0, 1.0])));
        let w = hayro::vello_cpu::color::palette::css::WHITE.to_rgba8();
        let white = Paint::Color(Color::from_rgba(AlphaColor::from_rgba8(w.r, w.g, w.b, w.a)));
        let rect = Rect::new(20.0, 20.0, 80.0, 80.0);
        let mut page_clip = page.to_path(0.1);
        page_clip.apply_affine(matrix);

        // The scene: the page wrapper stays open (as on a real export page), the
        // real-transparency group opens a frame, draws, and pops.
        let mut scene = ShadowScene::new(page);
        scene.record_push_clip(&ClipPath { path: page.to_path(0.1), fill: FillRule::NonZero });
        scene.record_push_group(1.0, None, BlendMode::Normal);
        scene.record_push_group(0.5, None, BlendMode::Normal);
        scene.push_frame();
        let scene_red = red.clone();
        scene.record(move |r| r.draw_rect(&rect, matrix, &scene_red, &PathDrawMode::Fill(FillRule::NonZero)));
        scene.extend(rect);
        scene.record_pop_group();
        let frame = scene.pop_frame().expect("a frame was pushed");
        assert!(!frame.empty, "the rect's box reached the frame");
        let raster = scene.flatten().expect("a 100×100 pt page is far under every cap");

        // The expected crop: the frame's box ∩ the page, on the pixel grid.
        let region = frame.bbox.intersect(page);
        assert_eq!((region.x0, region.y0, region.x1, region.y1), (20.0, 20.0, 80.0, 80.0));
        let px = matrix.transform_rect_bbox(region);
        let (x0, y0) = (px.x0.floor() as u32, px.y0.floor() as u32);
        let (x1, y1) = (px.x1.ceil() as u32, px.y1.ceil() as u32);
        assert_eq!(raster.width, x1 - x0, "pixel-grid width of the region");
        assert_eq!(raster.height, y1 - y0, "pixel-grid height of the region");
        assert_eq!(raster.rgb.len(), ((raster.width * raster.height * 3) as usize), "one straight-RGB triplet per pixel");

        // The direct render: the same draws through a fresh OffscreenRenderer, no
        // scene — including popping the wrappers the scene pops at flatten time.
        let mut direct = OffscreenRenderer::new(side, side);
        direct.draw_rect(&Rect::new(0.0, 0.0, f64::from(side), f64::from(side)), Affine::IDENTITY, &white, &PathDrawMode::Fill(FillRule::NonZero));
        direct.push_clip_path(&ClipPath { path: page_clip, fill: FillRule::NonZero });
        direct.push_transparency_group(1.0, None, BlendMode::Normal);
        direct.push_transparency_group(0.5, None, BlendMode::Normal);
        direct.draw_rect(&rect, matrix, &red, &PathDrawMode::Fill(FillRule::NonZero));
        direct.pop_transparency_group();
        direct.pop_transparency_group();
        direct.pop_clip_path();
        let pixmap = direct.finish();

        for j in 0..raster.height {
            for i in 0..raster.width {
                let o = ((j * raster.width + i) * 3) as usize;
                let got = [raster.rgb[o], raster.rgb[o + 1], raster.rgb[o + 2]];
                let p = pixmap.sample((x0 + i) as u16, (y0 + j) as u16);
                assert_eq!(got, [p.r, p.g, p.b], "pixel ({i}, {j}) of the region must match the direct render");
            }
        }

        // The interior is the half-red-over-white blend (the loop above already
        // proves it equals vello's own compositing; the ±1 absorbs its rounding).
        let mid = &raster.rgb[((raster.height / 2 * raster.width + raster.width / 2) * 3) as usize..];
        assert_eq!(mid[0], 255, "the red channel stays saturated at half opacity: {}", mid[0]);
        assert!(
            (i32::from(mid[1]) - 128).abs() <= 1 && (i32::from(mid[2]) - 128).abs() <= 1,
            "green/blue carry the white half: {}/{}",
            mid[1],
            mid[2]
        );

        // `place` maps the raster's top-left pixel back onto the region's top-left in
        // device space, running y down the raster = y up the page, 72/300 pt per pixel.
        let tl = raster.place * Point::new(0.0, 0.0);
        assert!((tl.x - region.x0).abs() < 1e-9 && (tl.y - region.y1).abs() < 1e-9, "raster row 0 starts at the region's top: {tl:?}");
        let br = raster.place * Point::new(f64::from(raster.width), f64::from(raster.height));
        let want = (region.x0 + f64::from(raster.width) * 72.0 / FLATTEN_DPI, region.y1 - f64::from(raster.height) * 72.0 / FLATTEN_DPI);
        assert!((br.x - want.0).abs() < 1e-9 && (br.y - want.1).abs() < 1e-9, "the far corner lands one 72/300-pt step per pixel away: {br:?}");
    }

    /// A page whose 300-dpi rasterization would exceed the area cap is declined
    /// before any renderer is built: 2500×2500 pt → 10416² px ≈ 108.5 Mpx, over
    /// `MAX_FLATTEN_AREA_PX` even though the group's own region would fit.
    #[test]
    fn area_cap_declines() {
        let page_side = 2500.0;
        let px = page_side * FLATTEN_DPI / 72.0;
        assert!(px * px > MAX_FLATTEN_AREA_PX as f64, "the fixture page must really exceed the cap: {px}²");

        let mut scene = ShadowScene::new(Rect::new(0.0, 0.0, page_side, page_side));
        scene.record_push_group(0.5, None, BlendMode::Normal);
        scene.push_frame();
        scene.extend(Rect::new(100.0, 100.0, 2400.0, 2400.0));
        scene.record_pop_group();
        scene.pop_frame();
        assert!(scene.flatten().is_none(), "the oversized page declines without rendering");
    }

    /// A frame with no draws — and a frame whose box falls entirely outside the
    /// page — yield an empty raster (width 0, height 0), not a decline: the caller
    /// emits nothing and keeps the disclosure note.
    #[test]
    fn empty_region_yields_empty_raster() {
        let page = Rect::new(0.0, 0.0, 100.0, 100.0);

        let mut scene = ShadowScene::new(page);
        scene.push_frame();
        let frame = scene.pop_frame().expect("a frame was pushed");
        assert!(frame.empty, "no draw reached the frame");
        let raster = scene.flatten().expect("an empty region is not a decline");
        assert_eq!((raster.width, raster.height), (0, 0), "no pixels, no bytes");
        assert!(raster.rgb.is_empty());

        let mut scene = ShadowScene::new(page);
        scene.record_push_group(0.5, None, BlendMode::Normal);
        scene.push_frame();
        scene.record(|_| {});
        scene.extend(Rect::new(200.0, 200.0, 300.0, 300.0));
        scene.record_pop_group();
        scene.pop_frame();
        let raster = scene.flatten().expect("an off-page region is empty, not declined");
        assert_eq!((raster.width, raster.height), (0, 0), "nothing of the box is on the page");
        assert!(raster.rgb.is_empty());
    }

    /// A panic inside a recorded closure poisons the scene: this flatten declines,
    /// and every later one declines too — never crash twice.
    #[test]
    fn poisoned_scene_declines() {
        let mut scene = ShadowScene::new(Rect::new(0.0, 0.0, 100.0, 100.0));
        scene.record_push_group(0.5, None, BlendMode::Normal);
        scene.push_frame();
        scene.record(|_: &mut OffscreenRenderer| panic!("poison: a replayed draw panicked"));
        scene.extend(Rect::new(10.0, 10.0, 90.0, 90.0));
        scene.record_pop_group();
        scene.pop_frame();
        assert!(scene.flatten().is_none(), "the panicking replay is caught and declined");

        // A later group with a healthy log must still decline: the scene is poisoned.
        scene.record(|r| r.set_blend_mode(BlendMode::Normal));
        scene.record_push_group(0.5, None, BlendMode::Normal);
        scene.push_frame();
        scene.extend(Rect::new(0.0, 0.0, 50.0, 50.0));
        scene.record_pop_group();
        scene.pop_frame();
        assert!(scene.flatten().is_none(), "a poisoned scene declines forever");
    }

    /// The per-page flatten budget is enforced — and one scene survives being
    /// flattened repeatedly, its log replayed again each time.
    #[test]
    fn flatten_budget_declines() {
        let mut scene = ShadowScene::new(Rect::new(0.0, 0.0, 10.0, 10.0));
        for _ in 0..MAX_FLATTENS_PER_PAGE {
            scene.push_frame();
            scene.extend(Rect::new(1.0, 1.0, 9.0, 9.0));
            scene.pop_frame();
            assert!(scene.flatten().is_some(), "within the budget a tiny page flattens");
        }
        scene.push_frame();
        scene.extend(Rect::new(1.0, 1.0, 9.0, 9.0));
        scene.pop_frame();
        assert!(scene.flatten().is_none(), "the budget is spent");
    }

    /// A hole (an image draw the log cannot carry — a transient stream) declines only
    /// the flattens whose region touches it; a region clear of holes still flattens,
    /// and the decline spends no budget (the check runs before the render).
    #[test]
    fn image_hole_declines_only_intersecting_flattens() {
        let mut scene = ShadowScene::new(Rect::new(0.0, 0.0, 100.0, 100.0));
        scene.record_image_hole(Rect::new(40.0, 40.0, 60.0, 60.0));

        scene.push_frame();
        scene.extend(Rect::new(10.0, 10.0, 50.0, 50.0));
        scene.pop_frame().expect("a frame was pushed");
        assert!(scene.flatten().is_none(), "the region overlaps the hole: declined");

        scene.push_frame();
        scene.extend(Rect::new(10.0, 10.0, 30.0, 30.0));
        scene.pop_frame().expect("a frame was pushed");
        let raster = scene.flatten().expect("a region clear of the hole flattens");
        assert!(raster.width > 0 && raster.height > 0, "the clear region renders: {}×{}", raster.width, raster.height);

        // The decline spent no budget: a small page still flattens afterwards.
        scene.push_frame();
        scene.extend(Rect::new(0.0, 0.0, 20.0, 20.0));
        scene.pop_frame().expect("a frame was pushed");
        assert!(scene.flatten().is_some(), "the hole decline burned no flatten budget");
    }

    /// The log survives a flatten and replays with content the next time: the second
    /// flatten — with nothing new recorded — still composites the first's draws, so
    /// recorded closures run twice with identical pixels (the reason `record` takes
    /// `FnMut`; clearing the log after a successful flatten would fail here). The
    /// draw sits inside a balanced recorded clip pair, so a recorded pop replays too,
    /// not only the still-open wrappers `flatten` unwinds.
    #[test]
    fn log_replays_with_content_after_a_flatten() {
        let page = Rect::new(0.0, 0.0, 20.0, 20.0);
        let matrix = page_matrix(page);
        let rect = Rect::new(4.0, 4.0, 12.0, 12.0);
        let blue = Paint::Color(Color::from_rgba(AlphaColor::new([0.0, 0.0, 1.0, 1.0])));

        let mut scene = ShadowScene::new(page);
        scene.push_frame();
        // A balanced clip pair around the draw — the shape of a form XObject's
        // /BBox clip wrapping its content.
        scene.record_push_clip(&ClipPath { path: page.to_path(0.1), fill: FillRule::NonZero });
        scene.record(move |r| r.draw_rect(&rect, matrix, &blue, &PathDrawMode::Fill(FillRule::NonZero)));
        scene.extend(rect);
        scene.record_pop_clip();
        scene.pop_frame().expect("a frame was pushed");
        let first = scene.flatten().expect("a 20×20 pt page is far under every cap");
        assert!(first.width > 0 && first.height > 0, "the region is the draw's box");
        // The region is exactly the rect, so its centre pixel is blue over white.
        let mid = &first.rgb[((first.height / 2 * first.width + first.width / 2) * 3) as usize..];
        assert_eq!([mid[0], mid[1], mid[2]], [0, 0, 255], "the draw's ink is in the first raster");

        // Nothing new recorded: the second flatten can only come from the surviving
        // log, replayed over the same region.
        scene.push_frame();
        scene.extend(rect);
        scene.pop_frame().expect("a frame was pushed");
        let second = scene.flatten().expect("the budget is not spent");
        assert_eq!((second.width, second.height), (first.width, first.height), "the same region, the same pixel grid");
        let mid = &second.rgb[((second.height / 2 * second.width + second.width / 2) * 3) as usize..];
        assert_eq!([mid[0], mid[1], mid[2]], [0, 0, 255], "the same draw replays into the second raster");
        assert_eq!(second.rgb, first.rgb, "the replay is byte-identical: closures run twice with the same result");
    }
}
