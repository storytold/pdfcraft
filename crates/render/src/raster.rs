//! Background page rasterization.
//!
//! A `RenderPool` parses lazily once and shares that immutable parser between workers. Render
//! caches stay worker-local. Requests carry a caller tag that is echoed back.
//!
//! **Robustness:** every page render runs under `catch_unwind`. A panic inside the renderer on a
//! malformed page yields an error and permanently retires the pool's shared parser generation.
//! Workers discard its caches/results and fall back to private parsers. There is no shared
//! rebuild loop, and a timed-out shared initialization is never started again in that pool.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, sync_channel};

#[cfg(test)]
mod radial_softmask_tests;
mod work_queue;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use work_queue::{ResultReceiver, WorkQueue};

use hayro::hayro_interpret::font::{FontData, FontQuery, Glyph};
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use hayro::hayro_interpret::pattern::Pattern;
use hayro::hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, InterpreterWarning, Paint,
    PathDrawMode, SoftMask, TransformExt, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::page::Page;
use hayro::vello_cpu::color::AlphaColor;
use hayro::vello_cpu::color::palette::css::{TRANSPARENT, WHITE};
use hayro::{RenderCache, RenderSettings, render_into, render_size};

use crate::{Pixels, ReadingColors};

mod pool;

use pool::lock;
#[cfg(test)]
use pool::{BusyPublishGate, ParseGate, ParserState, PoolParser, RenderGate, Shared, same_request, worker, worker_settings};
pub use pool::{RenderPool, Retiring};

/// Monotonic-ish timer that is safe on wasm32 (where `std::time::Instant` panics).
#[derive(Clone, Copy)]
struct Stopwatch(#[cfg(not(target_arch = "wasm32"))] std::time::Instant);

impl Stopwatch {
    fn start() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        return Self(std::time::Instant::now());
        #[cfg(target_arch = "wasm32")]
        return Self();
    }
    fn millis(self) -> u32 {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.elapsed().as_millis() as u32;
        #[cfg(target_arch = "wasm32")]
        return 0;
    }
}

/// Per-document rendering configuration.
#[derive(Clone, Debug, Default)]
pub struct RenderConfig {
    /// User or owner password for encrypted documents.
    pub password: Option<Arc<str>>,
    /// Layer (optional content group) visibility overrides: (object number, generation, visible).
    pub layers: Arc<Vec<(i32, i32, bool)>>,
    /// View ▸ Hide all comments: markup annotations aren't drawn (fields and links still are).
    pub hide_comments: bool,
    /// Refuse a whole-page raster that would be reduced by the renderer's hard size caps.
    /// Tiled requests remain available for pages that exceed those caps. Both modes refuse
    /// a raster whose rounded integer dimensions exceed the final pixel-allocation budget.
    pub reject_oversize: bool,
    /// View ▸ Reading colours: solid text and line-art paints are remapped for readability and
    /// the page clears to the setting's background. Display only; the document never changes.
    pub reading_colors: ReadingColors,
    /// The page clears to transparent instead of white (or the reading background): for a page
    /// holding only what is laid over another raster, such as a comment while it is dragged.
    pub transparent: bool,
}

impl RenderConfig {
    fn settings(&self) -> InterpreterSettings {
        let standard = InterpreterSettings::default().font_resolver;
        InterpreterSettings {
            ocg_overrides: self.layers.clone(),
            hide_comments: self.hide_comments,
            font_resolver: Arc::new(move |query| japanese_fallback(query).or_else(|| standard(query))),
            ..InterpreterSettings::default()
        }
    }
}

/// A Japanese face from craft-fonts for a CID font of the Adobe-Japan1 collection that the PDF
/// doesn't embed (`HeiseiMin-W3`, `KozGoPro-Medium`, …). hayro's own substitutes for fonts that
/// aren't embedded are the Latin standard 14, so such text drew nothing. `None` for every other
/// font, and when PdfCraft was built without craft-fonts. Only Japanese: no other CJK face is
/// embedded whole.
fn japanese_fallback(query: &FontQuery) -> Option<(FontData, u32)> {
    let FontQuery::Fallback(f) = query else { return None };
    if f.character_collection.as_ref()?.family != CidFamily::AdobeJapan1 {
        return None;
    }
    let face = match japanese_face(f.post_script_name.as_deref().unwrap_or_default(), f.is_serif, f.is_bold || f.font_weight >= 600) {
        // BIZ UDMincho before the document face (Shippori Mincho): it covers half-width katakana
        // (U+FF61–U+FF9F), which Shippori Mincho lacks, and a character the substitute lacks is
        // drawn as .notdef.
        JapaneseFace::Mincho => pdfcraft_fonts::ui_japanese_fonts()
            .into_iter()
            .find(|c| c.family == "BIZ UDMincho" && c.style == "Regular")
            .or_else(pdfcraft_fonts::document_japanese_font),
        JapaneseFace::Gothic { bold } => {
            let faces = pdfcraft_fonts::ui_japanese_fonts();
            let style = if bold { "Bold" } else { "Regular" };
            faces.iter().find(|c| c.family == "BIZ UDPGothic" && c.style == style).or(faces.first()).copied()
        }
    }?;
    Some((Arc::new(face.bytes), 0))
}

#[derive(Debug, PartialEq)]
enum JapaneseFace {
    Mincho,
    Gothic { bold: bool },
}

/// Which kind of Japanese face stands in for the font named `name`: Mincho names (`HeiseiMin`,
/// `KozMin`, `Ryumin`, `MS-Mincho`) a serif Mincho; Gothic names (`…Gothic…`, `HeiseiKakuGo`,
/// `KozGo`, `…Maru…`) a sans Gothic; any other name by the font descriptor's serif flag.
fn japanese_face(name: &str, serif: bool, bold: bool) -> JapaneseFace {
    let name = name.to_ascii_lowercase();
    if name.contains("min") {
        JapaneseFace::Mincho
    } else if ["goth", "kakugo", "kozgo", "kaku", "maru"].iter().any(|k| name.contains(k)) || !serif {
        JapaneseFace::Gothic { bold }
    } else {
        JapaneseFace::Mincho
    }
}

/// Hard cap on a rendered side, to bound memory at extreme zoom levels (tiling arrives in M3.3).
pub const MAX_SIDE: f32 = 8192.0;
/// Hard cap on rendered pixels per page (~64 MP ≈ 256 MB RGBA).
pub const MAX_PIXELS: f32 = 64.0e6;

/// What a request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RequestKind {
    /// A raster of the page.
    #[default]
    Pixels,
    /// The page's text layer (glyphs with Unicode and boxes); `scale` is ignored.
    Text,
}

/// A region of a scaled page, in device pixels (for tiled rendering of large pages).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RenderRequest {
    pub page: usize,
    pub kind: RequestKind,
    /// Render only this region (device pixels at `scale`). `None` renders the whole page.
    pub tile: Option<Tile>,
    /// Device pixels per PDF point.
    pub scale: f32,
    /// Caller-defined tag, echoed back (used for zoom generations / thumbnail vs page).
    pub tag: u64,
    /// Low-detail render for previews (page backdrops, thumbnails): images covering only a
    /// few device pixels are skipped. Never set it for a final render (tiles, whole pages,
    /// print, export): previews may differ by those pixels.
    pub preview: bool,
}

/// The tag bit of a print-preview raster. Such requests always render the document's true
/// colours, ignoring `RenderConfig::reading_colors`: a preview's job is fidelity to the
/// printout, and the print itself — imposed from the document — is never recoloured.
pub const PRINT_TAG: u64 = 1 << 60;

/// The reading colours a request renders with: print previews ([`PRINT_TAG`]) ignore the
/// document's setting and render with reading colours off.
fn request_reading(config: &RenderConfig, req: &RenderRequest) -> ReadingColors {
    if req.tag & PRINT_TAG != 0 { ReadingColors::default() } else { config.reading_colors }
}

/// What a render that panicked or overran the watchdog is remembered by: its page and kind. Not
/// its scale, tile or tag: the canvas tags every zoom level and tile anew, so a wider key would
/// let one pathological page tie up a worker for the full watchdog limit per zoom and per tile
/// (exhausting worker replacements) and re-parse the document for every new request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RequestFailureKey {
    page: usize,
    kind: RequestKind,
}

impl From<RenderRequest> for RequestFailureKey {
    fn from(req: RenderRequest) -> Self {
        Self { page: req.page, kind: req.kind }
    }
}

#[derive(Debug)]
pub struct RenderedPage {
    pub request: RenderRequest,
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major. Empty when `error` is set.
    pub rgba: Pixels,
    /// Why the page could not be rendered (renderer panic, empty page box, …).
    pub error: Option<String>,
    /// For `RequestKind::Text`.
    pub text: Option<Arc<crate::text::PageText>>,
    /// Non-fatal conditions that made the result partial or otherwise noteworthy.
    pub warnings: Vec<RenderWarning>,
    pub millis: u32,
}

/// A non-fatal condition reported while interpreting a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderWarning {
    /// Content was skipped after the per-page decoded-content budget was exhausted.
    ContentTruncated,
    /// A colour could not be computed (a colour space that cannot be read, or a Separation or
    /// DeviceN tint transform that cannot be evaluated) and was painted in a fallback colour.
    ColourFallback,
}

/// Counters for work performed by a renderer or render pool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Number of parser construction attempts.
    pub parser_builds: u64,
    /// Number of page interpretations for pixels or SVG, including tiled requests.
    pub page_interpretations: u64,
    /// Number of render requests accepted for processing.
    pub render_requests: u64,
    /// Number of requests that render only a tile.
    pub tile_requests: u64,
    /// Number of text-layer interpretations.
    pub text_interpretations: u64,
    /// Number of pixel rasterizations.
    pub rasterizations: u64,
}

#[derive(Default)]
struct AtomicRenderStats {
    parser_builds: AtomicU64,
    page_interpretations: AtomicU64,
    render_requests: AtomicU64,
    tile_requests: AtomicU64,
    text_interpretations: AtomicU64,
    rasterizations: AtomicU64,
}

impl AtomicRenderStats {
    fn record_request(&self, req: RenderRequest) {
        self.render_requests.fetch_add(1, Ordering::Relaxed);
        if req.tile.is_some() {
            self.tile_requests.fetch_add(1, Ordering::Relaxed);
        }
        match req.kind {
            RequestKind::Pixels => {
                self.page_interpretations.fetch_add(1, Ordering::Relaxed);
                self.rasterizations.fetch_add(1, Ordering::Relaxed);
            }
            RequestKind::Text => {
                self.text_interpretations.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn snapshot(&self) -> RenderStats {
        RenderStats {
            parser_builds: self.parser_builds.load(Ordering::Relaxed),
            page_interpretations: self.page_interpretations.load(Ordering::Relaxed),
            render_requests: self.render_requests.load(Ordering::Relaxed),
            tile_requests: self.tile_requests.load(Ordering::Relaxed),
            text_interpretations: self.text_interpretations.load(Ordering::Relaxed),
            rasterizations: self.rasterizations.load(Ordering::Relaxed),
        }
    }
}

/// Clamp a requested scale so the output respects `MAX_SIDE` and `MAX_PIXELS`.
pub fn effective_scale(width_pt: f32, height_pt: f32, scale: f32) -> f32 {
    let (w, h) = (width_pt.max(1.0), height_pt.max(1.0));
    let by_side = MAX_SIDE / w.max(h);
    let by_area = (MAX_PIXELS / (w * h)).sqrt();
    // The caps always win: a floor here once let a 934-million-point-wide page (a fuzzed file)
    // render 9 million pixels wide. Only guard against a zero or non-finite scale.
    let capped = scale.min(by_side).min(by_area);
    if capped.is_finite() && capped > 0.0 { capped } else { by_side.min(by_area).max(f32::MIN_POSITIVE) }
}

/// Device pixels that cover `pt` points at `scale`: rounded up, as poppler's `pdftoppm` does, so
/// the last partial row or column of a page is drawn (anti-aliased against the background) rather
/// than cut off. Float noise up to 1/100 px is not rounded up, so 612 pt at 150 dpi stays 1275 px.
/// At least 1; saturates instead of overflowing.
pub fn device_pixels(pt: f32, scale: f32) -> u32 {
    let px = pt * scale - 0.01;
    if px.is_finite() { px.ceil().max(1.0) as u32 } else { 1 }
}

/// Render one page with a caller-owned parser and cache. Panics inside the renderer are caught
/// and reported as `Err((message, panicked))`.
type Output = (u32, u32, Pixels, Option<Arc<crate::text::PageText>>);

fn render_page<'a>(
    pdf: &'a Pdf,
    cache: &RenderCache<'a>,
    settings: &InterpreterSettings,
    config: &RenderConfig,
    req: RenderRequest,
) -> Result<Output, (String, bool)> {
    let reading = request_reading(config, &req);
    if req.kind == RequestKind::Text {
        return match catch_unwind(AssertUnwindSafe(|| crate::text::extract_page(pdf, req.page, settings))) {
            Ok(Some(t)) => Ok((0, 0, Pixels::default(), Some(Arc::new(t)))),
            Ok(None) => Err((format!("page {} does not exist", req.page + 1), false)),
            Err(panic) => Err((format!("text extraction crashed on page {}: {}", req.page + 1, panic_message(&panic)), true)),
        };
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let (page, w, h) = drawable_page(pdf, req.page)?;
        // Reading colours: the page clears to the setting's background, and solid text and
        // line-art paints are remapped (images and patterns never are). Off renders as before.
        let (bg_color, color_replacement) = match reading.params {
            Some(p) => (AlphaColor::from_rgb8(p.background[0], p.background[1], p.background[2]), Some(hayro::ColorReplacement::from(p))),
            None => (WHITE, None),
        };
        let bg_color = if config.transparent { TRANSPARENT } else { bg_color };
        let rs = match req.tile {
            // Tiles are bounded by construction, so the page itself may be arbitrarily large.
            Some(t) => {
                let scale = req.scale.clamp(0.01, 400.0);
                let (tw, th) = (t.w.clamp(1, 4096), t.h.clamp(1, 4096));
                RenderSettings {
                    x_scale: scale,
                    y_scale: scale,
                    width: Some(tw as u16),
                    height: Some(th as u16),
                    x_offset: t.x as f32,
                    y_offset: t.y as f32,
                    bg_color,
                    color_replacement,
                }
            }
            None => {
                let scale = effective_scale(w, h, req.scale);
                if config.reject_oversize && (!(req.scale.is_finite() && req.scale > 0.0) || req.scale > scale) {
                    return Err(format!(
                        "requested page {} raster scale {req_scale} exceeds renderer limits (maximum scale {scale})",
                        req.page + 1,
                        req_scale = req.scale
                    ));
                }
                // hayro floors the size when none is given, losing the partial edge pixels. The
                // scale keeps each side within MAX_SIDE (give or take float noise), so it fits u16.
                let side = |pt: f32| device_pixels(pt, scale).min(MAX_SIDE as u32) as u16;
                RenderSettings {
                    x_scale: scale,
                    y_scale: scale,
                    width: Some(side(w)),
                    height: Some(side(h)),
                    bg_color,
                    color_replacement,
                    ..Default::default()
                }
            }
        };
        // Rendered straight into a buffer of whole pixels, which the GUI takes over as is.
        let (w, h) = render_size(page, &rs);
        if u64::from(w).checked_mul(u64::from(h)).is_none_or(|pixels| pixels > MAX_PIXELS as u64) {
            return Err(format!("page {} raster size {w}x{h} exceeds renderer pixel limit", req.page + 1));
        }
        let mut pixels = Pixels::zeroed(w.into(), h.into()).ok_or_else(|| format!("page {} is too large to render", req.page + 1))?;
        // Previews render at low detail (tiny images are skipped); finals always draw all.
        let mut preview_settings = settings.clone();
        preview_settings.preview = req.preview;
        render_into(page, cache, &preview_settings, &rs, pixels.bytes_mut());
        Ok((w.into(), h.into(), pixels, None))
    }));
    match result {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err((e, false)),
        Err(panic) => Err((format!("renderer crashed on page {}: {}", req.page + 1, panic_message(&panic)), true)),
    }
}

/// Page `page` and its size in points, refusing a page box that is empty or not finite.
fn drawable_page(pdf: &Pdf, page: usize) -> Result<(&Page<'_>, f32, f32), String> {
    let p = pdf.pages().get(page).ok_or_else(|| format!("page {} does not exist", page.saturating_add(1)))?;
    let (w, h) = p.render_dimensions();
    if !(w.is_finite() && h.is_finite()) || w < 0.5 || h < 0.5 {
        return Err(format!("page {} has an empty or invalid page box ({w}×{h} pt)", page.saturating_add(1)));
    }
    Ok((p, w, h))
}

/// The most SVG text one page may become, by [`SvgSize`]'s estimate. hayro-svg shares only
/// glyphs, clips, masks and patterns, and writes out every other draw, so a page that draws one
/// form millions of times would otherwise grow the SVG until memory runs out.
const MAX_SVG_BYTES: u64 = 256 << 20;

/// The most SVG one pixel of an image or shading raster becomes: a PNG of 4-byte pixels with
/// each row's filter byte is at most 5 bytes a pixel and base64 makes 4 of every 3, and an image
/// whose soft mask has another size also writes the mask as a PNG.
const PIXEL_BYTES: u64 = 8;

/// [`SvgSize`]'s estimate of the SVG `page` becomes, stopping soon after it passes
/// [`MAX_SVG_BYTES`].
fn svg_size(page: &Page<'_>, w: f32, h: f32, settings: &InterpreterSettings) -> u64 {
    let stop = Arc::new(AtomicBool::new(false));
    let settings = InterpreterSettings { cancelled: Some(stop.clone()), ..settings.clone() };
    let cache = InterpreterCache::new();
    let mut ctx =
        Context::new(page.initial_transform(true).to_kurbo(), kurbo::Rect::new(0.0, 0.0, w.into(), h.into()), &cache, page.xref(), settings);
    let mut size = SvgSize { bytes: 0, stop, seen: HashSet::new(), masked: false, blended: false };
    interpret_page(page, &mut ctx, &mut size);
    size.bytes
}

/// A device that only adds up about how much SVG each draw becomes: an element with its paint
/// and transform about 128 bytes, each path segment at most 64, each image pixel
/// [`PIXEL_BYTES`], and the group hayro-svg wraps each path and glyph in while a soft mask or
/// blend mode is set. A shading fill adds the raster hayro-svg draws of its whole box, unclipped.
/// Tiling cells, Type 3 glyphs and soft masks are visited once per key hayro-svg writes them
/// under, as it does. Past [`MAX_SVG_BYTES`] it sets `stop`, the interpreter's cancel flag.
struct SvgSize {
    bytes: u64,
    stop: Arc<AtomicBool>,
    seen: HashSet<u128>,
    masked: bool,
    blended: bool,
}

impl SvgSize {
    fn element(&mut self, extra: u64) {
        self.bytes = self.bytes.saturating_add(128).saturating_add(extra);
        if self.bytes > MAX_SVG_BYTES {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn add_path(&mut self, path: &kurbo::BezPath) {
        self.element(64u64.saturating_mul(path.elements().len() as u64));
    }

    /// Whether `key` is new, so what hayro-svg writes once for it is still to count.
    fn first(&mut self, key: u128) -> bool {
        !self.stop.load(Ordering::Relaxed) && self.seen.insert(key)
    }

    /// A pattern paint of `path` drawn with `transform`, keyed and sized as hayro-svg's
    /// `write_paint` and `render_shading_texture` do.
    fn paint<'a>(&mut self, paint: &Paint<'a>, path: &kurbo::BezPath, transform: kurbo::Affine, line_width: Option<f32>) {
        use kurbo::Shape as _;
        let Paint::Pattern(pattern) = paint else { return };
        match pattern.as_ref() {
            Pattern::Shading(s) => {
                let mut b = path.bounding_box();
                if let Some(w) = line_width {
                    b = b.inflate(f64::from(w), f64::from(w));
                }
                let b = (transform * b.to_path(0.0)).bounding_box();
                let bits = |lo: f64, hi: f64| (u128::from(lo.to_bits()) << 64) | u128::from(hi.to_bits());
                if self.first((s.cache_key(), (bits(b.x0, b.x1), bits(b.y0, b.y1))).cache_key()) {
                    // A side that is not finite is past any budget (the cast saturates).
                    let side = |v: f64| if v.is_finite() { v.max(1.0).ceil() } else { f64::MAX };
                    self.element((side(b.width()) * side(b.height()) * PIXEL_BYTES as f64) as u64);
                }
            }
            Pattern::Tiling(t) => {
                if self.first((t.cache_key(), transform.inverse().cache_key()).cache_key()) {
                    let _ = t.interpret(self, kurbo::Affine::translate((-t.bbox.x0, -t.bbox.y0)), false);
                }
            }
        }
    }

    /// The group hayro-svg's `with_group` wraps a path or glyph in.
    fn group(&mut self) {
        if self.masked || self.blended {
            self.element(0);
        }
    }

    fn mask(&mut self, mask: Option<SoftMask<'_>>) {
        if let Some(mask) = mask
            && self.first(mask.cache_key())
        {
            mask.interpret(self);
        }
    }
}

impl<'a> Device<'a> for SvgSize {
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'a>>) {
        self.masked = mask.is_some();
        self.mask(mask);
    }
    fn set_blend_mode(&mut self, mode: BlendMode) {
        self.blended = mode != BlendMode::Normal;
    }
    fn draw_path(&mut self, path: &kurbo::BezPath, transform: kurbo::Affine, paint: &Paint<'a>, mode: &PathDrawMode) {
        self.group();
        self.add_path(path);
        let line_width = match mode {
            PathDrawMode::Stroke(s) => Some(s.line_width),
            PathDrawMode::Fill(_) => None,
        };
        self.paint(paint, path, transform, line_width);
    }
    fn push_clip_path(&mut self, clip: &ClipPath) {
        self.add_path(&clip.path);
    }
    fn push_transparency_group(&mut self, _: f32, mask: Option<SoftMask<'a>>, _: BlendMode) {
        self.element(0);
        self.mask(mask);
    }
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: kurbo::Affine, glyph_transform: kurbo::Affine, paint: &Paint<'a>, mode: &GlyphDrawMode) {
        // hayro-svg writes the group even around a glyph it then leaves out.
        self.group();
        self.element(0);
        let line_width = match mode {
            GlyphDrawMode::Invisible | GlyphDrawMode::Clip => return,
            GlyphDrawMode::Stroke(s) => Some(s.line_width),
            GlyphDrawMode::Fill => None,
        };
        match glyph {
            // hayro-svg sizes a glyph's paint from its outline before the glyph transform.
            Glyph::Outline(o) if matches!(paint, Paint::Pattern(_)) => self.paint(paint, &o.outline(), transform, line_width),
            Glyph::Outline(_) => {}
            Glyph::Type3(t) => {
                let key = ((t.cache_key(), transform.cache_key()), (glyph_transform.cache_key(), paint.cache_key())).cache_key();
                if self.first(key) {
                    t.interpret(self, transform, glyph_transform, paint);
                }
            }
        }
    }
    fn draw_image(&mut self, image: Image<'a, '_>, transform: kurbo::Affine) {
        let (w, h) = (image.width(), image.height());
        self.element(PIXEL_BYTES.saturating_mul(u64::from(w)).saturating_mul(u64::from(h)));
        // hayro-svg draws an image mask filled with a pattern as its box path filled with that
        // paint, in a group masked by the image.
        if let Image::Stencil(s) = &image
            && !self.stop.load(Ordering::Relaxed)
        {
            use kurbo::Shape as _;
            let rect = kurbo::Rect::new(0.0, 0.0, f64::from(w), f64::from(h)).to_path(0.1);
            // Only the paint is wanted, so ask for the smallest decode the codec allows.
            s.with_stencil(
                |_, paint| {
                    if matches!(paint, Paint::Pattern(_)) {
                        self.element(0);
                        self.add_path(&rect);
                        self.paint(paint, &rect, transform, None);
                    }
                },
                Some((1, 1)),
            );
        }
    }
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

fn finish(req: RenderRequest, start: Stopwatch, r: Result<Output, (String, bool)>, warnings: Vec<RenderWarning>) -> RenderedPage {
    let millis = start.millis();
    match r {
        Ok((width, height, rgba, text)) => RenderedPage { request: req, width, height, rgba, error: None, text, warnings, millis },
        Err((e, _)) => RenderedPage { request: req, width: 0, height: 0, rgba: Pixels::default(), error: Some(e), text: None, warnings, millis },
    }
}

fn warning_settings(settings: &InterpreterSettings, warnings: Arc<Mutex<Vec<RenderWarning>>>) -> InterpreterSettings {
    let target = warnings;
    InterpreterSettings {
        warning_sink: Arc::new(move |warning| {
            // Nested forms, Type 3 glyphs and patterns are interpreted again and each reports the
            // exhausted budget, or the colours that fell back: once per page is enough.
            let warning = match warning {
                InterpreterWarning::ContentTruncated => RenderWarning::ContentTruncated,
                InterpreterWarning::ColorFallback => RenderWarning::ColourFallback,
                InterpreterWarning::UnsupportedFont | InterpreterWarning::ImageDecodeFailure => return,
            };
            let mut warnings = lock(&target);
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }),
        ..settings.clone()
    }
}

fn take_warnings(warnings: &Mutex<Vec<RenderWarning>>) -> Vec<RenderWarning> {
    std::mem::take(&mut *lock(warnings))
}

/// A single-threaded renderer over one parsed document (used by the CLI and tests).
pub struct PageRenderer {
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    pdf: Option<Pdf>,
    settings: InterpreterSettings,
    stats: RenderStats,
}

impl PageRenderer {
    pub fn new(bytes: Arc<Vec<u8>>, config: RenderConfig) -> Self {
        let pdf = parse(&bytes, config.password.as_deref());
        let settings = config.settings();
        Self { bytes, config, pdf, settings, stats: RenderStats { parser_builds: 1, ..RenderStats::default() } }
    }

    pub fn page_count(&self) -> usize {
        self.pdf.as_ref().map(|p| p.pages().len()).unwrap_or(0)
    }

    /// Snapshot the work counters collected by this renderer.
    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    /// Extract one page for statistics, with semantic exclusions. Never rasterizes or panics.
    pub fn filtered_text(&mut self, page: usize, filter: crate::text::TextFilter) -> Result<crate::PageText, String> {
        let pdf = self.pdf.as_ref().ok_or("the document could not be parsed")?;
        self.stats.text_interpretations = self.stats.text_interpretations.saturating_add(1);
        catch_unwind(AssertUnwindSafe(|| crate::text::extract_page_filtered(pdf, page, &self.settings, filter)))
            .map_err(|e| format!("text extraction failed: {}", panic_message(&e)))?
    }

    /// One page as a standalone SVG document over white, sized like `render` from the crop box
    /// and rotation. Text is drawn as glyph outlines, so it is not selectable. A page whose SVG
    /// would pass about [`MAX_SVG_BYTES`] is an error. Never panics.
    pub fn svg(&mut self, page: usize) -> Result<String, String> {
        let pdf = self.pdf.as_ref().ok_or("the document could not be parsed")?;
        self.stats.page_interpretations = self.stats.page_interpretations.saturating_add(1);
        let r = catch_unwind(AssertUnwindSafe(|| {
            let (p, w, h) = drawable_page(pdf, page)?;
            // hayro-svg holds the whole document in one String, so size it before writing it.
            if svg_size(p, w, h, &self.settings) > MAX_SVG_BYTES {
                return Err(format!("too complex to export as SVG (over {} MiB); export it as PNG or JPEG instead", MAX_SVG_BYTES >> 20));
            }
            self.stats.page_interpretations = self.stats.page_interpretations.saturating_add(1);
            let white = hayro_svg::SvgRenderSettings { bg_color: [255, 255, 255, 255] };
            Ok(hayro_svg::convert(p, &hayro_svg::RenderCache::new(), &self.settings, &white))
        }));
        r.unwrap_or_else(|panic| {
            // A fresh parser, as `render` takes after a crash.
            self.pdf = parse(&self.bytes, self.config.password.as_deref());
            self.stats.parser_builds = self.stats.parser_builds.saturating_add(1);
            Err(format!("SVG export crashed on page {}: {}", page.saturating_add(1), panic_message(&panic)))
        })
    }

    /// Render one page. Never panics.
    pub fn render(&mut self, req: RenderRequest) -> RenderedPage {
        let start = Stopwatch::start();
        if self.pdf.is_some() {
            self.stats.render_requests += 1;
            if req.tile.is_some() {
                self.stats.tile_requests += 1;
            }
            match req.kind {
                RequestKind::Pixels => {
                    self.stats.page_interpretations += 1;
                    self.stats.rasterizations += 1;
                }
                RequestKind::Text => self.stats.text_interpretations += 1,
            }
        }
        let Some(pdf) = self.pdf.as_ref() else {
            return finish(req, start, Err(("the document could not be parsed".into(), false)), Vec::new());
        };
        let cache = RenderCache::new();
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let settings = warning_settings(&self.settings, warnings.clone());
        let r = render_page(pdf, &cache, &settings, &self.config, req);
        let warnings = take_warnings(&warnings);
        if matches!(r, Err((_, true))) {
            self.pdf = parse(&self.bytes, self.config.password.as_deref());
            self.stats.parser_builds += 1;
        }
        finish(req, start, r, warnings)
    }
}

fn parse(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Option<Pdf> {
    catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes.clone(), password.unwrap_or("")).ok())).ok().flatten()
}

pub(crate) fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(ToString::to_string).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into())
}

#[cfg(test)]
mod tests;
