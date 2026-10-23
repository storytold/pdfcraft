use crate::{ColorReplacement, RenderCache, derive_settings};
use hayro_interpret::color::Color;
use hayro_interpret::encode::EncodedShadingPattern;
use hayro_interpret::font::Glyph;
use hayro_interpret::pattern::Pattern;
use hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Device, FillRule, GlyphDrawMode, ImageData, LumaData, MaskType,
    Paint, PathDrawMode, RgbData, SoftMask, StrokeProps,
};
use kurbo::{Affine, BezPath, Point, Rect, Shape, Vec2};
use pic_scale::{
    ImageSize, ImageStore, ImageStoreMut, PicScaleError, Resampling, ResamplingFunction, Scaler,
};
use std::collections::HashMap;
use std::mem::size_of;
use std::rc::Rc;
use std::sync::Arc;
use vello_cpu::color::DynamicColor;
use vello_cpu::color::palette::css::BLACK;
use vello_cpu::color::{AlphaColor, PremulRgba8, Srgb};
use vello_cpu::peniko::{ColorStop, Compose, Fill, Gradient, ImageQuality, ImageSampler, Mix};
use vello_cpu::{
    Image, ImageSource, Mask, PaintType, Pixmap, RenderContext, RenderSettings, peniko,
};

/// PdfCraft patch: largest image drawn, in pixels (2^28 ≈ 268 MP, well above real page images).
const MAX_IMAGE_PIXELS: u64 = 1 << 28;

/// PdfCraft patch: source axes allowed into CatmullRom planning. Unlike the final pixmap,
/// sources need not fit u16: a 65536-pixel strip can safely shrink to a 16-pixel image.
/// This resource cap follows the existing CCITT column limit; the filter-work estimate below
/// additionally bounds tables/scratch before entering the opaque PicScale planner.
const MAX_RESAMPLING_SOURCE_SIDE: u32 = 1 << 20;
const MAX_RESAMPLING_PLAN_BYTES: usize = 128 << 20;

/// PdfCraft patch: source, intermediate and RGBA byte lengths, independent of backend sides.
fn source_byte_len(width: u32, height: u32, channels: usize, max_pixels: u64) -> Option<usize> {
    if width == 0 || height == 0 || !matches!(channels, 1 | 3 | 4) {
        return None;
    }
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    if pixels > max_pixels {
        return None;
    }
    let pixels = usize::try_from(pixels).ok()?;
    let rgba_len = pixels.checked_mul(4)?;
    if rgba_len > isize::MAX as usize {
        return None;
    }
    pixels.checked_mul(channels)
}

/// PdfCraft patch: only final pixmaps/padded glyphs need to fit the backend's u16 sides.
fn image_byte_len(width: u32, height: u32, channels: usize, max_pixels: u64) -> Option<usize> {
    if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return None;
    }
    source_byte_len(width, height, channels, max_pixels)
}

/// PdfCraft patch: conservative CatmullRom planning work for one axis in PicScale 0.7.12.
/// Its four-tap kernel grows on downscaling. Include float/32-aligned i16 weights, temporary
/// float/f64/usize tap arrays and two copies of the bounds. This is a size estimate, not a
/// fallible reserve inside PicScale or a bound on total renderer memory.
fn filter_plan_bytes(source: u32, target: u32) -> Option<usize> {
    if source == target {
        return Some(0);
    }
    let (source, target) = (usize::try_from(source).ok()?, usize::try_from(target).ok()?);
    if target == 0 {
        return None;
    }
    let taps = source.checked_mul(4)?.div_ceil(target).max(4);
    let aligned_taps = taps.div_ceil(32).checked_mul(32)?;
    let float_weights = taps.checked_mul(target)?.checked_mul(size_of::<f32>())?;
    let fixed_weights = aligned_taps
        .checked_mul(target)?
        .checked_mul(size_of::<i16>())?;
    let tap_scratch = taps.checked_mul(size_of::<f32>() + size_of::<f64>() + size_of::<usize>())?;
    let bounds = target.checked_mul(4)?.checked_mul(size_of::<usize>())?;
    float_weights
        .checked_add(fixed_weights)?
        .checked_add(tap_scratch)?
        .checked_add(bounds)
}

/// PdfCraft patch: alpha-aware RGBA plans also need source-sized premultiplication scratch.
/// Bound the combined scratch before planning, including a conservative crossed intermediate
/// (or four rows for a short target) when both convolution axes change.
fn resampling_scratch_byte_len(
    source: (u32, u32),
    target: (u32, u32),
    channels: usize,
    max_pixels: u64,
) -> Option<usize> {
    if source == target {
        return Some(0);
    }
    let alpha = if channels == 4 {
        source_byte_len(source.0, source.1, channels, max_pixels)?
    } else {
        0
    };
    let filter = if source.0 != target.0 && source.1 != target.1 {
        source_byte_len(source.0, target.1.max(4), channels, max_pixels)?
    } else {
        0
    };
    let len = alpha.checked_add(filter)?;
    let limit = max_pixels.saturating_mul(4).min(isize::MAX as u64);
    if u64::try_from(len).ok()? > limit {
        return None;
    }
    Some(len)
}

/// PdfCraft patch: validate output, crossed intermediate and filter-work bounds before planning.
/// Source width × target height also bounds PicScale's smaller single-threaded four-row scratch.
/// Area averaging (`!interpolate`) has none of those: it streams source rows into the target, so
/// only the source and target buffers are bounded here. Its sums (one source row of columns and
/// a target row or two) are bounded by [`area_average`] itself, which knows the channel count
/// it averages (four for an image with a soft mask), against [`MAX_AREA_SCRATCH_BYTES`].
fn resampling_byte_len(
    source: (u32, u32),
    target: (u32, u32),
    channels: usize,
    data_len: usize,
    max_pixels: u64,
    interpolate: bool,
) -> Option<usize> {
    if source_byte_len(source.0, source.1, channels, max_pixels)? != data_len {
        return None;
    }
    let target_len = image_byte_len(target.0, target.1, channels, max_pixels)?;
    if !interpolate {
        return Some(target_len);
    }
    resampling_scratch_byte_len(source, target, channels, max_pixels)?;
    source_byte_len(source.0, target.1, channels, max_pixels)?;
    if source.0 > MAX_RESAMPLING_SOURCE_SIDE || source.1 > MAX_RESAMPLING_SOURCE_SIDE {
        return None;
    }
    let plan_bytes = filter_plan_bytes(source.0, target.0)?
        .checked_add(filter_plan_bytes(source.1, target.1)?)?;
    if plan_bytes > MAX_RESAMPLING_PLAN_BYTES {
        return None;
    }
    Some(target_len)
}

/// PdfCraft patch: image dimensions selected before allocating resampling buffers.
/// Exported for PdfCraft's small, metadata-only regression tests. `None` rejects invalid
/// sources/scales or an over-budget target/intermediate; the caller may retain a valid source.
/// `interpolate` selects the Catmull-Rom limits; area averaging (`false`) skips them.
#[doc(hidden)]
pub fn image_resampling_size(
    width: u32,
    height: u32,
    channels: usize,
    data_len: usize,
    x_scale: f32,
    y_scale: f32,
    max_pixels: u64,
    interpolate: bool,
) -> Option<(u32, u32)> {
    if !x_scale.is_finite() || !y_scale.is_finite() || x_scale <= 0.0 || y_scale <= 0.0 {
        return None;
    }
    let target = if x_scale < 1.0 || y_scale < 1.0 {
        let side = |size: u32, scale: f32| {
            (size as f32 * scale)
                .ceil()
                .max(1.0)
                .min((u16::MAX / 2) as f32) as u32
        };
        (side(width, x_scale), side(height, y_scale))
    } else {
        (width, height)
    };
    resampling_byte_len(
        (width, height),
        target,
        channels,
        data_len,
        max_pixels,
        interpolate,
    )?;
    Some(target)
}

/// PdfCraft patch: allocation failures in image conversion/resampling keep the renderer alive.
fn image_buffer(len: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).ok()?;
    Some(out)
}

/// PdfCraft patch: a zeroed buffer of `len` elements, or `None` if it cannot be allocated.
fn zeroed_buffer<T: Copy + Default>(len: usize) -> Option<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).ok()?;
    out.resize(len, T::default());
    Some(out)
}

/// PdfCraft patch: most bytes of `u64` sums [`area_average`] keeps besides its output: 128 MiB,
/// the budget of Catmull-Rom filter planning ([`MAX_RESAMPLING_PLAN_BYTES`]). Those sums take
/// 8 bytes per sample of one source row, so a one-row strip needs 8× its decoded size, and a
/// tiny Flate stream can decode to such a strip. Real images stay far below it: the budget is a
/// row of about 16 M samples (a 16 M-pixel gray or 4 M-pixel RGBA strip). A source over it
/// keeps its size, like any resize over its limit; one that wide cannot fit the backend's u16
/// sides, so the image is skipped with a warning.
const MAX_AREA_SCRATCH_BYTES: usize = 128 << 20;

/// PdfCraft patch: the bytes of `u64` sums [`area_average`] keeps for an `N`-channel source
/// `source_width` wide shrunk to `target_width`: the column sums of one source row, and one row
/// of target sums, or two when the height is enlarged. `None` past [`MAX_AREA_SCRATCH_BYTES`].
fn area_scratch_byte_len(
    source_width: u32,
    target_width: u32,
    channels: usize,
    enlarging: bool,
) -> Option<usize> {
    let columns = usize::try_from(source_width).ok()?.checked_mul(channels)?;
    let target_rows = 1 + usize::from(enlarging);
    let sums = usize::try_from(target_width)
        .ok()?
        .checked_mul(channels)?
        .checked_mul(target_rows)?;
    let len = columns.checked_add(sums)?.checked_mul(size_of::<u64>())?;
    (len <= MAX_AREA_SCRATCH_BYTES).then_some(len)
}

/// PdfCraft patch: resizes an `N`-channel image to `target` by exact area averaging (a box
/// filter): each target pixel is the mean of the source area it covers, weighted by coverage.
/// Images that don't ask for `/Interpolate` use it. Nearest-neighbour kept one sample in N, so a
/// shrunk 1-bit scan lost thin strokes (staff lines, serifs) and its edges turned jagged.
/// Averaging never blends anything outside a target pixel's own area, so whole-ratio shrinks of
/// blocks aligned to the target grid stay exact, flat colours (#624).
///
/// `add_row(y, weight, columns)` adds `weight ×` source row `y` (`source.0 × N` samples) into
/// `columns`. Rows are visited in order. When shrinking (or keeping) the height, each source row
/// is added once per target row it overlaps (at most twice): the source rows of one target row
/// are summed down the columns first (a plain multiply-add over contiguous samples), and only
/// then walked across once per *target* row, since walking every source row across cost about
/// 4× as much on a 10718 × 13701 scan. When enlarging the height, a source row covers several
/// target rows, so it is added and walked across once, and that row is reused for each of them:
/// adding it again per target row cost source width × target height, about nine billion samples
/// for a 1,100,000 × 1 strip drawn 16 × 8192. Either way the integer sums are the same. Memory is
/// one row of column sums and one or two target rows of sums besides the output, within
/// [`MAX_AREA_SCRATCH_BYTES`] (checked before anything is allocated or read). Coordinates are
/// integers in units where a source pixel is `target` long and a target pixel `source` long, so
/// every overlap is exact.
fn area_average<const N: usize>(
    source: (u32, u32),
    target: (u32, u32),
    mut add_row: impl FnMut(usize, u64, &mut [u64]) -> Option<()>,
) -> Option<Vec<u8>> {
    let ((sw, sh), (tw, th)) = (source, target);
    // Each target pixel's weights sum to `sw × sh`, so sums stay below 255 × MAX_IMAGE_PIXELS
    // and `total` is never zero.
    let total = u64::from(sw).checked_mul(u64::from(sh))?;
    if total == 0 || total > MAX_IMAGE_PIXELS || tw == 0 || th == 0 {
        return None;
    }
    let out_len = image_byte_len(tw, th, N, MAX_IMAGE_PIXELS)?;
    let enlarging = th > sh;
    area_scratch_byte_len(sw, tw, N, enlarging)?;
    let columns_len = (sw as usize).checked_mul(N)?;
    let sums_len = (tw as usize).checked_mul(N)?;
    let mut out = image_buffer(out_len)?;
    let mut columns = zeroed_buffer::<u64>(columns_len)?;
    let mut sums = zeroed_buffer::<u64>(sums_len)?;
    let (sh, th) = (u64::from(sh), u64::from(th));
    // Enlarging the height: the current source row walked across (each target column's sum,
    // at most 255 × `sw`), filled when that row is first reached.
    let mut row = if enlarging {
        Some(zeroed_buffer::<u64>(sums_len)?)
    } else {
        None
    };
    let (mut pos, mut sy, mut ty, mut row_ready) = (0u64, 0u64, 0u64, false);
    while sy < sh && ty < th {
        let (source_end, target_end) = ((sy + 1) * th, (ty + 1) * sh);
        let end = source_end.min(target_end);
        let weight = end - pos;
        // `sy < sh ≤ u32::MAX`, so it fits usize.
        match &mut row {
            Some(row) => {
                if !row_ready {
                    columns.fill(0);
                    add_row(sy as usize, 1, &mut columns)?;
                    area_average_row::<N>(&columns, sw, tw, row);
                    row_ready = true;
                }
                for (sum, value) in sums.iter_mut().zip(row.iter()) {
                    *sum += weight * value;
                }
            }
            None => add_row(sy as usize, weight, &mut columns)?,
        }
        pos = end;
        if end == target_end {
            if row.is_none() {
                area_average_row::<N>(&columns, sw, tw, &mut sums);
                columns.fill(0);
            }
            // A mean of samples, so at most 255.
            out.extend(
                sums.iter()
                    .map(|sum| ((sum + total / 2) / total).min(255) as u8),
            );
            if row.is_some() {
                sums.fill(0);
            }
            ty += 1;
        }
        if end == source_end {
            sy += 1;
            row_ready = false;
        }
    }
    (out.len() == out_len).then_some(out)
}

/// PdfCraft patch: [`area_average`] across one row: `sums` (`tw × N`) receives each target
/// column's `columns` weighted by how much of it each source column covers (summing to `sw`).
fn area_average_row<const N: usize>(columns: &[u64], sw: u32, tw: u32, sums: &mut [u64]) {
    sums.fill(0);
    let (columns, sums) = (columns.as_chunks::<N>().0, sums.as_chunks_mut::<N>().0);
    let (sw, tw) = (u64::from(sw), u64::from(tw));
    let (mut pos, mut sx, mut tx) = (0u64, 0usize, 0usize);
    while let (Some(column), Some(sum)) = (columns.get(sx), sums.get_mut(tx)) {
        let (source_end, target_end) = ((sx as u64 + 1) * tw, (tx as u64 + 1) * sw);
        let end = source_end.min(target_end);
        let weight = end - pos;
        for (sum, column) in sum.iter_mut().zip(column) {
            *sum += weight * column;
        }
        pos = end;
        if end == source_end {
            sx += 1;
        }
        if end == target_end {
            tx += 1;
        }
    }
}

/// PdfCraft patch: [`area_average`] of packed `N`-channel samples.
fn area_average_slice<const N: usize>(
    data: &[u8],
    source: (u32, u32),
    target: (u32, u32),
) -> Option<Vec<u8>> {
    let row_len = source_byte_len(source.0, 1, N, MAX_IMAGE_PIXELS)?;
    area_average::<N>(source, target, |y, weight, columns| {
        let start = y.checked_mul(row_len)?;
        let row = data.get(start..start.checked_add(row_len)?)?;
        for (column, sample) in columns.iter_mut().zip(row) {
            *column += weight * u64::from(*sample);
        }
        Some(())
    })
}

/// PdfCraft patch: area-averages a colour image and its same-sized soft mask straight into
/// premultiplied RGBA of `target` size. Averaging premultiplied samples keeps a transparent
/// pixel's colour out of its neighbours, and the full-size RGBA copy is never made.
fn area_premultiplied_rgba(
    color: &ImageData,
    alpha: &[u8],
    source: (u32, u32),
    target: (u32, u32),
) -> Option<Vec<u8>> {
    let (samples, channels) = match color {
        ImageData::Luma(luma) => (luma.data.as_slice(), 1),
        ImageData::Rgb(rgb) => (rgb.data.as_slice(), 3),
    };
    let width = source.0 as usize;
    area_average::<4>(source, target, |y, weight, columns| {
        let start = y.checked_mul(width)?;
        let end = start.checked_add(width)?;
        let colors = samples.get(start.checked_mul(channels)?..end.checked_mul(channels)?)?;
        let alphas = alpha.get(start..end)?;
        let pixels = columns.as_chunks_mut::<4>().0.iter_mut();
        for ((column, color), a) in pixels.zip(colors.chunks_exact(channels)).zip(alphas) {
            let [r, g, b] = match color {
                [v] => [*v; 3],
                [r, g, b] => [*r, *g, *b],
                _ => return None,
            };
            // Stencil masks and most soft masks are fully opaque or fully clear.
            let pixel = match a {
                255 => [r, g, b, 255],
                0 => continue,
                a => AlphaColor::from_rgba8(r, g, b, *a)
                    .premultiply()
                    .to_rgba8()
                    .to_u8_array(),
            };
            for (column, sample) in column.iter_mut().zip(pixel) {
                *column += weight * u64::from(sample);
            }
        }
        Some(())
    })
}

/// PdfCraft patch: most pieces one dashed stroke may be cut into. Stroke expansion keeps every
/// dash, so a pattern that is tiny next to its path asks for billions: a fuzzed `/D [[11] 0]` on
/// a line from x = 92234775807 allocated over 5 GB, and `[0.000001] 0 d` on a 20 pt line over
/// 10 GB. Measured in release builds: a stroke cut into 250,000 pieces renders in about 16 ms and
/// 75 MB at its peak (a million took 80 ms and 290 MB). Dotted and dashed lines in real documents
/// stay far below this.
const MAX_DASHES_PER_STROKE: f64 = 250_000.0;

/// PdfCraft patch: the dash pattern to stroke `path` with. Empty (a solid line) when the pattern
/// is invalid, or when it could cut the path into more than [`MAX_DASHES_PER_STROKE`] pieces.
/// ISO 32000-2 §8.4.3.6 requires nonnegative entries that are not all zero; a pattern whose
/// period isn't positive (`[-1 -1] 0 d`) made kurbo's search for the starting dash loop for ever.
fn usable_dash_pattern(path: &BezPath, dashes: &[f32]) -> Vec<f64> {
    if dashes.iter().any(|d| !d.is_finite() || *d < 0.0) {
        return Vec::new();
    }
    let sum: f64 = dashes.iter().map(|d| f64::from(*d)).sum();
    // kurbo repeats an odd-length pattern once, as SVG does, so a period covers it twice.
    let (period, per_period) = if dashes.len() % 2 == 1 {
        (2.0 * sum, dashes.len().saturating_mul(2))
    } else {
        (sum, dashes.len())
    };
    if !period.is_finite() || period <= 0.0 {
        return Vec::new();
    }
    let (length, restarts) = control_polygon_length(path);
    // kurbo restarts the pattern at every subpath and after every `ClosePath`, so each restart
    // can begin up to a whole period of pieces however short its segment is.
    let pieces = (length / period + restarts as f64) * per_period as f64;
    if !pieces.is_finite() || pieces > MAX_DASHES_PER_STROKE {
        return Vec::new();
    }
    dashes.iter().map(|d| f64::from(*d)).collect()
}

/// PdfCraft patch: the length of `path`'s control polygon, which a curve never exceeds, and how
/// many times kurbo restarts a dash pattern along it (each `MoveTo` and `ClosePath`).
fn control_polygon_length(path: &BezPath) -> (f64, usize) {
    let (mut length, mut restarts) = (0.0, 0usize);
    let (mut start, mut last) = (Point::ORIGIN, Point::ORIGIN);
    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) => {
                (start, last) = (p, p);
                restarts = restarts.saturating_add(1);
            }
            kurbo::PathEl::LineTo(p) => {
                length += last.distance(p);
                last = p;
            }
            kurbo::PathEl::QuadTo(c, p) => {
                length += last.distance(c) + c.distance(p);
                last = p;
            }
            kurbo::PathEl::CurveTo(c1, c2, p) => {
                length += last.distance(c1) + c1.distance(c2) + c2.distance(p);
                last = p;
            }
            kurbo::PathEl::ClosePath => {
                length += last.distance(start);
                last = start;
                restarts = restarts.saturating_add(1);
            }
        }
    }
    (length, restarts)
}

/// PdfCraft patch: how far, in user space, drawing may place geometry outside a path's exact
/// outline. vello expands strokes to a tolerance of at most 0.25 user units (`TOL / max(|a|, |d|,
/// 1)` in vello_common's `flatten::stroke`), and kurbo approximates round joins and caps to 1e-3.
/// Fills are flattened into chords, which stay inside the outline's bounds.
const DRAW_SLACK: f64 = 0.5;

/// PdfCraft patch: previews skip images below this target area in device pixels: redrawing
/// them at full detail costs a decode and a resample each for pixels nobody can see.
const PREVIEW_MIN_IMAGE_AREA: u32 = 16;

thread_local! {
    /// PdfCraft patch: whether [`may_paint_canvas`] may skip paths on this thread. Turned off
    /// only by PdfCraft's regression test, which renders each tile with and without skipping.
    static SKIP_OFFSCREEN_PATHS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// PdfCraft patch: exported for its regression test (see [`SKIP_OFFSCREEN_PATHS`]).
#[doc(hidden)]
pub fn set_skip_offscreen_paths(skip: bool) {
    SKIP_OFFSCREEN_PATHS.with(|s| s.set(skip));
}

/// PdfCraft patch: whether a device-space box can paint a pixel of a `width` × `height`
/// canvas. A box whose bounds miss the canvas is skipped without changing a pixel.
/// Bounds that aren't finite are drawn as before.
fn paints_canvas(device: Rect, width: u16, height: u16) -> bool {
    if !SKIP_OFFSCREEN_PATHS.with(std::cell::Cell::get) {
        return true;
    }
    let finite = [device.x0, device.y0, device.x1, device.y1]
        .iter()
        .all(|v| v.is_finite());
    !finite
        || (device.x0 < f64::from(width)
            && device.x1 > 0.0
            && device.y0 < f64::from(height)
            && device.y1 > 0.0)
}

/// PdfCraft patch: whether a path drawn with `transform` can paint a pixel of a `width` ×
/// `height` canvas. `reach` is how far its paint extends beyond the path, in user space: 0 for a
/// fill; for a stroke, half its width times the larger of its miter limit (kurbo draws a miter
/// only when its tip is within `limit × width / 2` of the vertex) and √2 (square caps). The box
/// of the path's points and control points (a curve stays inside their hull) grown by that and
/// [`DRAW_SLACK`], mapped to the device and grown by 2 pixels for antialiasing, contains
/// everything it paints: a path whose box misses the canvas is skipped without changing a pixel,
/// so a tile of a large page no longer strokes and fills every path on the page. Bounds that
/// aren't finite (an empty path) are drawn as before.
fn may_paint_canvas(
    path: &BezPath,
    transform: Affine,
    reach: f64,
    width: u16,
    height: u16,
) -> bool {
    let mut points = Rect::new(
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    let mut add = |p: Point| points = points.union_pt(p);
    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => add(p),
            kurbo::PathEl::QuadTo(c, p) => {
                add(c);
                add(p);
            }
            kurbo::PathEl::CurveTo(c1, c2, p) => {
                add(c1);
                add(c2);
                add(p);
            }
            kurbo::PathEl::ClosePath => {}
        }
    }
    let grow = reach + DRAW_SLACK;
    let device = transform
        .transform_rect_bbox(points.inflate(grow, grow))
        .inflate(2.0, 2.0);
    paints_canvas(device, width, height)
}

/// PdfCraft patch: whether a `rect` drawn with `transform` can paint a pixel of a `width` ×
/// `height` canvas. Used for images: decoded data can only shrink against the stream-size box
/// (subsampled `scale_factors`), so the box over-approximates what is drawn and skipping it
/// never changes a pixel, while a tile of an image-heavy page no longer decodes, resamples and
/// rasterizes every image on the page.
fn may_paint_rect(rect: Rect, transform: Affine, width: u16, height: u16) -> bool {
    let device = transform
        .transform_rect_bbox(rect.inflate(DRAW_SLACK, DRAW_SLACK))
        .inflate(2.0, 2.0);
    paints_canvas(device, width, height)
}

/// PdfCraft patch: the whole-pixel device rectangle of a shrunk, axis-aligned image, as
/// `(width, height, x0, y0)`. `transform` maps source pixels to device pixels and `planned` is the
/// size [`image_resampling_size`] chose; `None` (draw as before) for rotated, skewed or flipped
/// images, non-finite geometry, or a snapped size that strays more than a pixel from `planned`
/// (so the memory budget that size passed still holds).
fn snap_minified_image(
    transform: &Affine,
    source: (u32, u32),
    planned: (u32, u32),
) -> Option<(u32, u32, f64, f64)> {
    let [a, b, c, d, e, f] = transform.as_coeffs();
    let tiny = 1e-4 * a.abs().max(d.abs());
    if !(a > 0.0 && d > 0.0 && b.abs() <= tiny && c.abs() <= tiny) {
        return None;
    }
    // Halves round up, not away from zero as `f64::round` does, so shifting the page by whole
    // pixels (a tile) shifts the rectangle without resizing it: with `round`, an image at
    // x = 0.5 snapped to 1..5 on the page but −1..4 in a tile starting at x = 1.
    let round = |v: f64| (v + 0.5).floor();
    let edge = |origin: f64, scale: f64, side: u32| {
        let (lo, hi) = (round(origin), round(origin + scale * f64::from(side)));
        (hi - lo >= 1.0 && lo.abs() < 1e6 && hi.abs() < 1e6).then_some((lo, hi))
    };
    let (x0, x1) = edge(e, a, source.0)?;
    let (y0, y1) = edge(f, d, source.1)?;
    // Bounded by the 1e6 check above, so the casts cannot saturate.
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    (w.abs_diff(planned.0) <= 1 && h.abs_diff(planned.1) <= 1).then_some((w, h, x0, y0))
}

/// PdfCraft patch (19): what `Renderer::set_paint` found out: the clip path to apply while
/// painting (if any), or that the paint touches no pixel of the canvas, so the caller skips
/// the fill or stroke (a mesh shading whose triangles all lie outside the path or canvas).
#[must_use]
enum PaintSetup {
    Clip(Option<BezPath>),
    Nothing,
}

/// PdfCraft patch: the transform that draws an image's final raster of `size` pixels. A snapped
/// image (`snapped` from [`snap_minified_image`]) whose raster has its snapped size is drawn 1:1
/// at the snapped corner. Any other raster is scaled by `additional` (source over raster size)
/// and drawn with the image's `transform`. That includes a resize that fell back to the source
/// (a failed allocation or resampler): drawn at the snapped corner, the source came out 1:1,
/// many times too large.
fn image_draw_transform(
    transform: Affine,
    additional: Affine,
    size: (u32, u32),
    snapped: Option<(u32, u32, f64, f64)>,
) -> Affine {
    match snapped {
        Some((w, h, x0, y0)) if (w, h) == size => Affine::translate((x0, y0)),
        _ => transform * additional,
    }
}

pub(crate) struct Renderer {
    pub(crate) ctx: RenderContext,
    /// PdfCraft patch: when set, solid text and line-art paint colours are remapped
    /// while rendering (the "reading colours" view setting).
    color_replacement: Option<ColorReplacement>,
    /// PdfCraft patch (17): while the page's annotations are being drawn, paints keep their
    /// colours — reading colours replace document colours, never annotation colours.
    in_annotations: bool,
    /// PdfCraft patch (22): one entry per transparency group pushed, whether it is a knockout
    /// group (`Device::set_knockout`).
    knockout: Vec<bool>,
    /// PdfCraft patch (22): inside a knockout erase pass (`Device::begin_knockout_erase`), and
    /// how many transparency groups were skipped in it (nothing is layered while erasing).
    erasing: bool,
    erase_groups: usize,
    pub(crate) inside_pattern: bool,
    /// Soft masks drawn during this render. PdfCraft note: it belongs to one `Renderer` (one
    /// render, every nested renderer starting empty) and so to one `preview` setting: a mask
    /// drawn for a preview, which may leave out tiny images, never reaches a final render.
    pub(crate) soft_mask_cache: HashMap<u128, Mask>,
    pub(crate) outline_cache: Rc<std::cell::RefCell<HashMap<u128, Rc<BezPath>>>>,
    pub(crate) cur_mask: Option<Mask>,
    pub(crate) in_type3_glyph: bool,
    pub(crate) scaler: Scaler,
    /// PdfCraft patch: low-detail preview render (see `InterpreterSettings::preview`).
    pub(crate) preview: bool,
    // TODO: Remove this once vello_cpu bug with non-transparent images is fixed
    image_transparency_stack: Vec<bool>,
}

#[derive(Clone, Copy)]
enum ImagePixelFormat {
    Luma,
    Rgb,
    Rgba,
}

impl Renderer {
    /// PdfCraft patch (22): whether the innermost transparency group is a knockout group.
    fn in_knockout(&self) -> bool {
        self.knockout.last().copied().unwrap_or(false)
    }

    pub(crate) fn new(
        width: u16,
        height: u16,
        settings: RenderSettings,
        cache: &RenderCache<'_>,
        color_replacement: Option<ColorReplacement>,
        preview: bool,
    ) -> Self {
        Self {
            ctx: RenderContext::new_with(width, height, settings),
            color_replacement,
            in_annotations: false,
            knockout: Vec::new(),
            erasing: false,
            erase_groups: 0,
            inside_pattern: false,
            soft_mask_cache: HashMap::default(),
            outline_cache: cache.outline_cache.clone(),
            cur_mask: None,
            in_type3_glyph: false,
            scaler: Scaler::new(ResamplingFunction::CatmullRom),
            preview,
            image_transparency_stack: Vec::new(),
        }
    }

    /// PdfCraft patch: returns the stroke's reach for [`may_paint_canvas`].
    fn set_stroke_properties(
        &mut self,
        stroke_props: &StrokeProps,
        is_text: bool,
        path: &BezPath,
    ) -> f64 {
        let threshold = if is_text { 0.25 } else { 1.0 };

        // Best-effort attempt to ensure a line width of at least 1.0, as required by the PDF
        // specification. If we are stroking text, we reduce the threshold as it will otherwise
        // lead to very bold-looking text at low resolutions.
        let min_factor = max_factor(self.ctx.transform());
        let mut line_width = stroke_props.line_width.max(0.01);
        let transformed_width = line_width * min_factor;

        // Only enforce line width if not inside of pattern or type 3 glyph.
        if transformed_width < threshold && !self.inside_pattern && !self.in_type3_glyph {
            line_width /= transformed_width;
            line_width *= threshold;
        }

        // PdfCraft patch: a stroke far wider than the canvas looks the same as one a few canvases
        // wide, but its geometry grows with the width: a fuzzed `/LW 9223372036854775807`
        // allocated 10 GB in stroke expansion.
        let widest = 4.0 * (f32::from(self.ctx.width()) + f32::from(self.ctx.height()));
        if min_factor.is_finite() && min_factor > 0.0 && line_width * min_factor > widest {
            line_width = widest / min_factor;
        }

        let stroke = kurbo::Stroke {
            width: line_width as f64,
            join: stroke_props.line_join,
            miter_limit: stroke_props.miter_limit as f64,
            start_cap: stroke_props.line_cap,
            end_cap: stroke_props.line_cap,
            // PdfCraft patch: see `usable_dash_pattern`.
            dash_pattern: usable_dash_pattern(path, &stroke_props.dash_array).into(),
            dash_offset: if stroke_props.dash_offset.is_finite() {
                stroke_props.dash_offset as f64
            } else {
                0.0
            },
        };

        let reach = stroke_reach(&stroke);
        self.ctx.set_stroke(stroke);
        reach
    }

    fn draw_image_with_alpha_mask(&mut self, image_data: ImageData, mut alpha_data: LumaData) {
        // PdfCraft patch: mismatched masks need a dummy RGB staging image. Validate and
        // reserve it before creating/pushing any mask, so failure skips the whole image.
        let Some(len) = source_byte_len(alpha_data.width, alpha_data.height, 3, MAX_IMAGE_PIXELS)
        else {
            log::warn!(
                "Image alpha-mask staging dimensions exceeded the buffer limit; image skipped"
            );
            return;
        };
        let Some(mut dummy_rgb) = image_buffer(len) else {
            log::warn!("Image alpha-mask staging buffer could not be allocated; image skipped");
            return;
        };
        dummy_rgb.resize(len, 0);
        let mask = {
            let transform = *self.ctx.transform()
                * Affine::scale_non_uniform(
                    image_data.width() as f64 / alpha_data.width as f64,
                    image_data.height() as f64 / alpha_data.height as f64,
                );
            // PdfCraft patch (#1649): a mask that differs in size from its image carries the
            // shape on its own (Word saves text as a tiny black image with a glyph-bitmap mask).
            // Shrunk by nearest-neighbour it keeps one sample in N and drops every stroke thinner
            // than N mask pixels, so a shrunk mask is filtered whatever its `/Interpolate`, which
            // only asks for smoothing when the mask is enlarged.
            let (x, y) = x_y_advances(&transform);
            if x.length() < 1.0 || y.length() < 1.0 {
                alpha_data.interpolate = true;
            }
            let mut renderer = Self {
                ctx: RenderContext::new_with(
                    self.ctx.width(),
                    self.ctx.height(),
                    derive_settings(self.ctx.render_settings()),
                ),
                // PdfCraft patch: this renders an image's alpha mask, which must keep its
                // original values, so it is never remapped by the reading-colours setting.
                color_replacement: None,
                in_annotations: false,
                knockout: Vec::new(),
                erasing: false,
                erase_groups: 0,
                inside_pattern: false,
                soft_mask_cache: HashMap::default(),
                outline_cache: self.outline_cache.clone(),
                cur_mask: None,
                in_type3_glyph: false,
                scaler: self.scaler,
                preview: self.preview,
                image_transparency_stack: Vec::new(),
            };
            let mut mask_pix = Pixmap::new(self.ctx.width(), self.ctx.height());
            let rgb_data = ImageData::Rgb(RgbData {
                data: dummy_rgb,
                width: alpha_data.width,
                height: alpha_data.height,
                interpolate: alpha_data.interpolate,
                scale_factors: alpha_data.scale_factors,
            });
            renderer.ctx.set_transform(transform);
            // Note that there is a circle between `draw_image` and `draw_image_with_alpha_mask`,
            // but `draw_image_with_alpha_mask` is only called if the dimensions or interpolate
            // values between alpha_data and rgb_data don't match, which they do here.
            renderer.draw_image(rgb_data, Some(alpha_data));
            renderer.ctx.flush();
            let mut resources = vello_cpu::Resources::default();
            renderer.ctx.render_to_pixmap(&mut resources, &mut mask_pix);
            Mask::new_alpha(&mask_pix)
        };

        self.ctx.push_mask_layer(mask);
        self.image_transparency_stack.push(true);
        self.draw_image(image_data, None);
        self.image_transparency_stack.pop();
        self.ctx.pop_layer();
    }

    fn resize_image_data(
        &self,
        data: Vec<u8>,
        src_width: u32,
        src_height: u32,
        new_width: u32,
        new_height: u32,
        pixel_format: ImagePixelFormat,
        interpolate: bool,
    ) -> (Vec<u8>, u32, u32) {
        // PdfCraft patch (#624): `/Interpolate` governs the pre-minification pass too. Using
        // Catmull-Rom here for an image that did not request interpolation blends hard edges
        // across neighbouring pixels. Such an image is area-averaged instead, which only mixes
        // what falls inside each target pixel. PDF's default for `/Interpolate` is false.
        if !interpolate {
            let source = (src_width, src_height);
            let target = (new_width, new_height);
            let resized = match pixel_format {
                ImagePixelFormat::Luma => area_average_slice::<1>(&data, source, target),
                ImagePixelFormat::Rgb => area_average_slice::<3>(&data, source, target),
                ImagePixelFormat::Rgba => area_average_slice::<4>(&data, source, target),
            };
            return match resized {
                Some(out) => (out, new_width, new_height),
                None => {
                    log::warn!(
                        "Image area averaging failed or exceeded its buffer limit; falling back to source data"
                    );
                    (data, src_width, src_height)
                }
            };
        }
        let scaler = &self.scaler;
        let resized = match pixel_format {
            ImagePixelFormat::Luma => self.resize_image_data_impl::<1>(
                scaler,
                data,
                src_width,
                src_height,
                new_width,
                new_height,
                |scaler, source_size, target_size| {
                    scaler.plan_planar_resampling(source_size, target_size)
                },
            ),
            ImagePixelFormat::Rgb => self.resize_image_data_impl::<3>(
                scaler,
                data,
                src_width,
                src_height,
                new_width,
                new_height,
                |scaler, source_size, target_size| {
                    scaler.plan_rgb_resampling(source_size, target_size)
                },
            ),
            ImagePixelFormat::Rgba => self.resize_image_data_impl::<4>(
                scaler,
                data,
                src_width,
                src_height,
                new_width,
                new_height,
                |scaler, source_size, target_size| {
                    scaler.plan_rgba_resampling(source_size, target_size, true)
                },
            ),
        };
        match resized {
            Ok(out) => (out, new_width, new_height),
            Err(original) => {
                log::warn!(
                    "Image resampling failed or exceeded its buffer limit; falling back to source data"
                );
                (original, src_width, src_height)
            }
        }
    }

    fn resize_image_data_impl<const N: usize>(
        &self,
        scaler: &Scaler,
        data: Vec<u8>,
        src_width: u32,
        src_height: u32,
        new_width: u32,
        new_height: u32,
        plan: impl FnOnce(
            &Scaler,
            ImageSize,
            ImageSize,
        ) -> Result<Arc<Resampling<u8, N>>, PicScaleError>,
    ) -> Result<Vec<u8>, Vec<u8>> {
        // PdfCraft patch: validate before constructing a plan or allocating its output/scratch.
        // Returning the owned input preserves both its pixels and dimensions on any failure.
        let resized = (|| {
            let len = resampling_byte_len(
                (src_width, src_height),
                (new_width, new_height),
                N,
                data.len(),
                MAX_IMAGE_PIXELS,
                true,
            )?;
            let source_size = ImageSize::new(src_width as usize, src_height as usize);
            let target_size = ImageSize::new(new_width as usize, new_height as usize);
            let src = ImageStore::<u8, N>::from_slice(&data, source_size.width, source_size.height)
                .ok()?;
            let plan = plan(scaler, source_size, target_size).ok()?;
            let scratch_len = plan.scratch_size();
            let scratch_bound = resampling_scratch_byte_len(
                (src_width, src_height),
                (new_width, new_height),
                N,
                MAX_IMAGE_PIXELS,
            )?;
            if scratch_len > scratch_bound {
                return None;
            }
            let mut out = image_buffer(len)?;
            out.resize(len, 0);
            let mut dst =
                ImageStoreMut::<u8, N>::from_slice(&mut out, target_size.width, target_size.height)
                    .ok()?;
            let mut scratch = image_buffer(scratch_len)?;
            scratch.resize(scratch_len, 0);
            plan.resample_with_scratch(&src, &mut dst, &mut scratch)
                .ok()?;
            Some(out)
        })();
        match resized {
            Some(out) => Ok(out),
            None => Err(data),
        }
    }

    /// The size an image of `width` x `height` samples is decoded at to be drawn with
    /// `transform`, or `None` if `draw_image` skips it: off the canvas (PdfCraft patch: see
    /// `may_paint_rect`; checked before decoding, so a tile no longer decodes every image on its
    /// page), or, in a preview, covering only a few device pixels (PdfCraft patch; final renders
    /// always draw them). The tiny-image test measures axis lengths: the size returned maps a
    /// point, so it includes the translation and only suits the decode hint.
    fn image_draw_size(&self, width: u32, height: u32, transform: Affine) -> Option<(u32, u32)> {
        if !may_paint_rect(
            Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            transform,
            self.ctx.width(),
            self.ctx.height(),
        ) {
            return None;
        }
        let target_width = (transform * Point::new(width as f64, 0.0))
            .to_vec2()
            .length()
            .ceil() as u32;
        let target_height = (transform * Point::new(0.0, height as f64))
            .to_vec2()
            .length()
            .ceil() as u32;
        if self.preview {
            let c = transform.as_coeffs();
            let tiny_w = (f64::from(width) * (c[0] * c[0] + c[1] * c[1]).sqrt()).ceil() as u32;
            let tiny_h = (f64::from(height) * (c[2] * c[2] + c[3] * c[3]).sqrt()).ceil() as u32;
            if tiny_w.saturating_mul(tiny_h) < PREVIEW_MIN_IMAGE_AREA {
                return None;
            }
        }
        Some((target_width, target_height))
    }

    fn draw_image(&mut self, image_data: ImageData, alpha_data: Option<LumaData>) {
        let cur_transform = *self.ctx.transform();
        let (x_scale, y_scale) = {
            let (x, y) = x_y_advances(&cur_transform);
            (x.length() as f32, y.length() as f32)
        };
        // PdfCraft patch: do not send non-finite or collapsed geometry to the rasterizer.
        if !cur_transform.as_coeffs().iter().all(|v| v.is_finite())
            || !x_scale.is_finite()
            || !y_scale.is_finite()
            || x_scale <= 0.0
            || y_scale <= 0.0
        {
            log::warn!("Image has an invalid transform and was skipped");
            return;
        }
        let (source_width, source_height) = (image_data.width(), image_data.height());
        let channels = match &image_data {
            ImageData::Luma(_) => 1,
            ImageData::Rgb(_) => 3,
        };
        let data_len = match &image_data {
            ImageData::Luma(a) => a.data.len(),
            ImageData::Rgb(a) => a.data.len(),
        };
        if source_byte_len(source_width, source_height, channels, MAX_IMAGE_PIXELS)
            != Some(data_len)
        {
            log::warn!("Image has invalid or over-limit source dimensions/data and was skipped");
            return;
        }
        let interpolate = image_data.interpolate();
        if let Some(a) = &alpha_data {
            if source_byte_len(a.width, a.height, 1, MAX_IMAGE_PIXELS) != Some(a.data.len()) {
                log::warn!("Image has invalid or over-limit alpha dimensions/data and was skipped");
                return;
            }
            if a.width != source_width || a.height != source_height || a.interpolate != interpolate
            {
                if let Some(alpha) = alpha_data {
                    self.draw_image_with_alpha_mask(image_data, alpha);
                }
                return;
            }
        }
        let mut quality = if interpolate {
            ImageQuality::Medium
        } else {
            ImageQuality::Low
        };
        let has_alpha = alpha_data.is_some();
        let mut may_have_transparency = has_alpha || self.force_images_may_have_transparency();
        let requested_resize = x_scale < 1.0 || y_scale < 1.0;
        let (new_width, new_height) = match image_resampling_size(
            source_width,
            source_height,
            channels,
            data_len,
            x_scale,
            y_scale,
            MAX_IMAGE_PIXELS,
            interpolate,
        ) {
            Some(target) => target,
            None => {
                if image_byte_len(source_width, source_height, 4, MAX_IMAGE_PIXELS).is_none() {
                    log::warn!(
                        "Image resampling exceeded its buffer limit and the original resolution cannot fit the backend; image skipped"
                    );
                    return;
                }
                log::warn!(
                    "Image resampling exceeded its buffer limit; retaining the original resolution"
                );
                (source_width, source_height)
            }
        };
        // PdfCraft patch (#624): High is bicubic and would blend the nearest pre-resize of an image
        // that did not request interpolation, so only interpolated glyph images are promoted.
        if requested_resize && self.in_type3_glyph && interpolate {
            quality = ImageQuality::High;
        }
        // PdfCraft patch: an axis-aligned image that is shrunk lands on whole device pixels. It
        // is shrunk once to exactly its snapped device rectangle and drawn 1:1 at the snapped
        // corner. Otherwise the shrunk raster (rounded up to a whole size) is sampled a second
        // time at a fractional scale and offset, which blurs scans and photos (interpolated) or
        // drops and doubles columns of the averaged result (not interpolated).
        let snapped = if requested_resize
            && !self.in_type3_glyph
            && (new_width, new_height) != (source_width, source_height)
        {
            snap_minified_image(&cur_transform, (source_width, source_height), (new_width, new_height))
        } else {
            None
        };
        let (new_width, new_height) = snapped.map_or((new_width, new_height), |s| (s.0, s.1));
        let needs_resize = (new_width, new_height) != (source_width, source_height);

        // PdfCraft patch (#624): a minified image with a soft mask that is not interpolated is
        // area-averaged straight into premultiplied target RGBA. That skips the full-resolution
        // RGBA copy below and the premultiply pass after it.
        let direct = match &alpha_data {
            Some(alpha) if needs_resize && !interpolate => area_premultiplied_rgba(
                &image_data,
                &alpha.data,
                (source_width, source_height),
                (new_width, new_height),
            ),
            _ => None,
        };
        let premultiplied = direct.is_some();
        // Preserve the single-channel/RGB fast paths. With alpha, expand directly to RGBA;
        // every source and alpha length was validated before zipping or allocating.
        let (data, format) = match (direct, image_data, alpha_data) {
            (Some(rgba), _, _) => (rgba, ImagePixelFormat::Rgba),
            (None, ImageData::Luma(luma), None) => (luma.data, ImagePixelFormat::Luma),
            (None, ImageData::Rgb(rgb), None) => (rgb.data, ImagePixelFormat::Rgb),
            (None, image, Some(alpha)) => {
                let Some(len) = source_byte_len(source_width, source_height, 4, MAX_IMAGE_PIXELS)
                else {
                    return;
                };
                let Some(mut rgba) = image_buffer(len) else {
                    log::warn!(
                        "Image RGBA buffer could not be allocated and the image was skipped"
                    );
                    return;
                };
                match image {
                    ImageData::Luma(luma) => {
                        rgba.extend(
                            luma.data
                                .iter()
                                .zip(alpha.data)
                                .flat_map(|(g, a)| [*g, *g, *g, a]),
                        );
                    }
                    ImageData::Rgb(rgb) => {
                        rgba.extend(
                            rgb.data
                                .as_chunks::<3>()
                                .0
                                .iter()
                                .zip(alpha.data)
                                .flat_map(|([r, g, b], a)| [*r, *g, *b, a]),
                        );
                    }
                }
                (rgba, ImagePixelFormat::Rgba)
            }
        };
        let (data, mut img_width, mut img_height) = if premultiplied {
            (data, new_width, new_height)
        } else if needs_resize {
            self.resize_image_data(
                data,
                source_width,
                source_height,
                new_width,
                new_height,
                format,
                interpolate,
            )
        } else {
            (data, source_width, source_height)
        };
        if image_byte_len(img_width, img_height, 4, MAX_IMAGE_PIXELS).is_none() {
            log::warn!(
                "Image resampling failed and the original resolution cannot fit the backend; image skipped"
            );
            return;
        }
        let mut additional_transform = Affine::scale_non_uniform(
            source_width as f64 / img_width as f64,
            source_height as f64 / img_height as f64,
        );
        let mut rgba_data = if matches!(format, ImagePixelFormat::Rgba) {
            data
        } else {
            let Some(len) = image_byte_len(img_width, img_height, 4, MAX_IMAGE_PIXELS) else {
                return;
            };
            let Some(mut rgba) = image_buffer(len) else {
                log::warn!("Image RGBA buffer could not be allocated and the image was skipped");
                return;
            };
            match format {
                ImagePixelFormat::Luma => rgba.extend(data.iter().flat_map(|g| [*g, *g, *g, 255])),
                ImagePixelFormat::Rgb => rgba.extend(
                    data.as_chunks::<3>()
                        .0
                        .iter()
                        .flat_map(|[r, g, b]| [*r, *g, *b, 255]),
                ),
                ImagePixelFormat::Rgba => {} // Handled above.
            }
            rgba
        };
        if has_alpha && !premultiplied {
            let (chunks, _) = rgba_data.as_chunks_mut::<4>();
            for chunk in chunks {
                let [r, g, b, a] = *chunk;
                *chunk = AlphaColor::from_rgba8(r, g, b, a)
                    .premultiply()
                    .to_rgba8()
                    .to_u8_array();
            }
        }
        // Type 3 glyphs get a transparent two-pixel frame for interpolation. Check the padded
        // RGBA allocation and u16 dimensions too, before narrowing or growing either side.
        if self.in_type3_glyph {
            let (Some(width), Some(height)) = (img_width.checked_add(4), img_height.checked_add(4))
            else {
                return;
            };
            let Some(len) = image_byte_len(width, height, 4, MAX_IMAGE_PIXELS) else {
                log::warn!(
                    "Image glyph padding exceeded its buffer limit and the image was skipped"
                );
                return;
            };
            let Some(mut padded) = image_buffer(len) else {
                log::warn!("Image glyph padding could not be allocated and the image was skipped");
                return;
            };
            let row_len = width as usize * 4; // Checked RGBA length above bounds every row.
            padded.resize(row_len * 2, 0);
            for row in rgba_data.chunks_exact(img_width as usize * 4) {
                padded.extend([0; 8]);
                padded.extend_from_slice(row);
                padded.extend([0; 8]);
            }
            padded.resize(len, 0);
            (img_width, img_height) = (width, height);
            additional_transform *= Affine::translate((-2.0, -2.0));
            may_have_transparency = true;
            rgba_data = padded;
        }
        let pixmap = Pixmap::from_parts_with_opacity(
            bytemuck::cast_vec(rgba_data),
            img_width as u16,
            img_height as u16,
            may_have_transparency,
        );
        let draw_transform = image_draw_transform(
            cur_transform,
            additional_transform,
            (img_width, img_height),
            snapped,
        );
        self.draw_pixmap(Arc::new(pixmap), quality, draw_transform);
    }

    fn push_clip_path_inner(&mut self, clip_path: &BezPath, fill: FillRule) {
        let old_transform = *self.ctx.transform();

        self.ctx.set_fill_rule(convert_fill_rule(fill));
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.push_clip_path(clip_path);

        self.ctx.set_transform(old_transform);
    }

    fn draw_pixmap(&mut self, pixmap: Arc<Pixmap>, quality: ImageQuality, transform: Affine) {
        let (width, height) = (pixmap.width(), pixmap.height());
        let image = Image {
            image: ImageSource::Pixmap(pixmap),
            sampler: ImageSampler {
                x_extend: peniko::Extend::Pad,
                y_extend: peniko::Extend::Pad,
                quality,
                alpha: 1.0,
            },
        };

        self.ctx.set_transform(transform);
        self.ctx.set_paint(image);
        self.ctx
            .fill_rect(&Rect::new(0.0, 0.0, width as f64, height as f64));
    }

    /// PdfCraft patch: remaps a solid paint colour through `color_replacement`, if one is
    /// set. `Paint::Pattern` (images, shadings, tiling patterns) always passes through
    /// untouched.
    fn replaced<'a>(&self, paint: &Paint<'a>, is_text: bool) -> Paint<'a> {
        let Some(replacement) = self.color_replacement else {
            return paint.clone();
        };
        // PdfCraft patch (17): annotations keep the colours they were given; reading colours
        // replace document colours only (issue #1548).
        if self.in_annotations {
            return paint.clone();
        }
        match paint {
            Paint::Color(c) => {
                let mapped = replacement.map(c.to_rgba().to_rgba8(), is_text);
                let rgba = hayro_interpret::color::AlphaColor::from_rgba8(
                    mapped[0], mapped[1], mapped[2], mapped[3],
                );
                Paint::Color(Color::from_rgba(rgba))
            }
            other => other.clone(),
        }
    }

    #[must_use]
    fn set_paint(&mut self, paint: &Paint<'_>, path: &BezPath, is_stroke: bool) -> PaintSetup {
        // PdfCraft patch (22): in a knockout erase pass every paint clears what is under its
        // shape (a transparent fill with `Compose::Copy`, weighted by coverage).
        if self.erasing {
            self.ctx.set_blend_mode(knockout_blend_mode());
            self.ctx.set_paint(AlphaColor::<Srgb>::new([0.0, 0.0, 0.0, 0.0]));
            // A shading's `/BBox` clips what it paints, so it clips what it clears too.
            return PaintSetup::Clip(match paint {
                Paint::Pattern(p) => match p.as_ref() {
                    Pattern::Shading(s) => s.shading.clip_path.clone(),
                    _ => None,
                },
                _ => None,
            });
        }
        let mut paint_transform = Affine::IDENTITY;
        let mut clip_path = None;
        // PdfCraft patch (19): `Extend::Repeat` tiles the texture over the path; a mesh texture
        // covers its triangles alone and must not repeat outside them.
        let mut extend = peniko::Extend::Repeat;

        let paint: PaintType = match paint.clone() {
            Paint::Color(c) => {
                let c = c.to_rgba().to_rgba8();
                AlphaColor::from_rgba8(c[0], c[1], c[2], c[3]).into()
            }
            Paint::Pattern(p) => {
                let path_transform = self.ctx.transform();

                match *p {
                    Pattern::Shading(s) => {
                        clip_path = s.shading.clip_path.clone();
                        let mut bbox = (*path_transform * path.clone()).bounding_box();

                        if is_stroke {
                            // Try to account for stroke in bbox.
                            let (a1, a2) = x_y_advances(path_transform);
                            let factor = a1.length().max(a2.length()) * self.ctx.stroke().width;
                            bbox = bbox.inflate(factor, factor);
                        }

                        bbox = bbox.intersect(Rect::new(
                            0.0,
                            0.0,
                            self.ctx.width() as f64,
                            self.ctx.height() as f64,
                        ));

                        // PdfCraft patch: mesh shadings are sampled only inside `bbox` (and a
                        // pixel around it: texels at a fractional edge look one pixel further).
                        let encoded = s.encode_within(Some(bbox.inflate(1.0, 1.0)));

                        // PdfCraft patch (19): a mesh paints only where its triangles are. The
                        // texture below used to cover the whole path (the canvas, for `sh`)
                        // and was sampled pixel by pixel: 310 one-triangle meshes on a plan
                        // cost 310 full tiles per tile, and the watchdog gave the page up. It
                        // now covers the triangles alone (a pixel around them, which stays
                        // transparent and is what `Extend::Pad` repeats), and a mesh with no
                        // triangle on the canvas paints nothing at all.
                        if let Some(extent) = encoded.extent {
                            // An empty extent (no triangle in the area) is `Rect::ZERO`, which
                            // sits at the canvas origin: test it before intersecting, or a
                            // 1 × 1 transparent texture would still be painted over the whole
                            // path (310 of them cost 230 ms per tile).
                            if extent.width() <= 0.0 || extent.height() <= 0.0 {
                                return PaintSetup::Nothing;
                            }
                            bbox = bbox.intersect(extent.inflate(1.0, 1.0));
                            if bbox.width() <= 0.0 || bbox.height() <= 0.0 {
                                return PaintSetup::Nothing;
                            }
                            extend = peniko::Extend::Pad;
                        }

                        // PdfCraft patch (backport of hayro 0.8): axial and radial shadings
                        // become native gradients instead of a texture sampled pixel by pixel.
                        // Upstream also shifts by half a pixel for vello_cpu 0.3; 0.0.8 already
                        // evaluates gradients at pixel centres, like the texture below.
                        // PdfCraft patch (#2128): vello_cpu 0.0.8 copies an opaque gradient
                        // over fully covered tiles without applying the mask or blend mode, and
                        // a gradient can't be flagged transparent like the texture below, so
                        // under a soft mask or blend mode the shading is sampled instead.
                        if !self.force_images_may_have_transparency()
                            && let Some(native) =
                                encoded.as_svg_gradient(bbox, NATIVE_GRADIENT_TOLERANCE)
                        {
                            self.ctx
                                .set_paint_transform(path_transform.inverse() * native.transform);
                            self.ctx.set_paint(native_gradient(native));
                            return PaintSetup::Clip(clip_path);
                        }

                        let (image, width, height, transform, may_have_transparency) =
                            render_shading_texture(bbox, &encoded);
                        let may_have_transparency =
                            may_have_transparency || self.force_images_may_have_transparency();
                        paint_transform = path_transform.inverse() * transform;

                        let pixmap = Pixmap::from_parts_with_opacity(
                            image,
                            width as u16,
                            height as u16,
                            may_have_transparency,
                        );

                        let image = Image {
                            image: ImageSource::Pixmap(Arc::new(pixmap)),
                            sampler: ImageSampler {
                                x_extend: extend,
                                y_extend: extend,
                                quality: ImageQuality::Medium,
                                alpha: 1.0,
                            },
                        };

                        PaintType::Image(image)
                    }
                    Pattern::Tiling(t) => {
                        const MAX_PIXMAP_SIZE: f32 = 3000.0;
                        // TODO: Raise this limit and perform downsampling if reached
                        // (see pdftc_100k_0138.pdf).
                        const MIN_PIXMAP_SIZE: f32 = 1.0;

                        let bbox = t.bbox;
                        let max_x_scale = MAX_PIXMAP_SIZE / bbox.width() as f32;
                        let min_x_scale = MIN_PIXMAP_SIZE / bbox.width() as f32;
                        let max_y_scale = MAX_PIXMAP_SIZE / bbox.height() as f32;
                        let min_y_scale = MIN_PIXMAP_SIZE / bbox.height() as f32;

                        let (mut xs, mut ys) = {
                            let (x, y) = x_y_advances(&(t.matrix));
                            (x.length() as f32, y.length() as f32)
                        };
                        xs = xs.max(min_x_scale).min(max_x_scale);
                        ys = ys.max(min_y_scale).min(max_y_scale);
                        // PdfCraft patch (20): a step too large for a cell pixmap, where what is
                        // painted can only show the first cell (PowerPoint writes `/XStep
                        // 2147480100` for a picture fill drawn once, #3349): the cell's `/BBox` is
                        // drawn at full scale with a transparent border and padded, not tiled.
                        // Shrinking the cell to fit the step left a picture under one pixel.
                        // How far a stroke's paint reaches past the path, miters included, in
                        // device pixels: the most the transform stretches any direction (a skew
                        // stretches some further than either axis), times the reach.
                        let reach = if is_stroke {
                            path_transform.spectral_norm() * stroke_reach(self.ctx.stroke())
                        } else {
                            0.0
                        };
                        if usable_cell_scale(xs, ys)
                            && (xs * t.x_step.abs() > MAX_PIXMAP_SIZE || ys * t.y_step.abs() > MAX_PIXMAP_SIZE)
                            && shows_one_cell(&t.matrix, path_transform, path, reach, bbox, t.x_step, t.y_step)
                        {
                            const BORDER: f64 = 1.0;
                            let pix_width = ((bbox.width() as f32 * xs).ceil() as u16).saturating_add(2);
                            let pix_height = ((bbox.height() as f32 * ys).ceil() as u16).saturating_add(2);
                            let mut renderer = Self {
                                ctx: RenderContext::new_with(
                                    pix_width,
                                    pix_height,
                                    derive_settings(self.ctx.render_settings()),
                                ),
                                color_replacement: None,
                                in_annotations: false,
                                knockout: Vec::new(),
                                erasing: false,
                                erase_groups: 0,
                                cur_mask: None,
                                inside_pattern: true,
                                soft_mask_cache: HashMap::default(),
                                outline_cache: self.outline_cache.clone(),
                                in_type3_glyph: false,
                                scaler: self.scaler,
                                preview: self.preview,
                                image_transparency_stack: Vec::new(),
                            };
                            let initial_transform = Affine::translate((BORDER, BORDER))
                                * Affine::scale_non_uniform(xs as f64, ys as f64)
                                * Affine::translate((-bbox.x0, -bbox.y0));
                            t.interpret(&mut renderer, initial_transform, is_stroke);
                            let mut pix = Pixmap::new(pix_width, pix_height);
                            renderer.ctx.flush();
                            let mut resources = vello_cpu::Resources::default();
                            renderer.ctx.render_to_pixmap(&mut resources, &mut pix);
                            paint_transform = path_transform.inverse() * t.matrix * initial_transform.inverse();
                            let image = Image {
                                image: ImageSource::Pixmap(Arc::new(pix)),
                                sampler: ImageSampler {
                                    x_extend: peniko::Extend::Pad,
                                    y_extend: peniko::Extend::Pad,
                                    quality: ImageQuality::Medium,
                                    alpha: 1.0,
                                },
                            };
                            self.ctx.set_paint_transform(paint_transform);
                            self.ctx.set_paint(PaintType::Image(image));
                            return PaintSetup::Clip(clip_path);
                        }
                        // PdfCraft patch: the cell pixmap is `step × scale` pixels, which only the
                        // u16 cast below bounded (a fuzzed /XStep far beyond /BBox gave 65535² cells).
                        xs = tiling_cell_scale(xs, t.x_step, MAX_PIXMAP_SIZE);
                        ys = tiling_cell_scale(ys, t.y_step, MAX_PIXMAP_SIZE);

                        let x_step = xs * t.x_step;
                        let y_step = ys * t.y_step;

                        let scaled_width = bbox.width() as f32 * xs;
                        let scaled_height = bbox.height() as f32 * ys;
                        let pix_width = x_step.abs().round() as u16;
                        let pix_height = y_step.abs().round() as u16;

                        let mut renderer = Self {
                            ctx: RenderContext::new_with(
                                pix_width,
                                pix_height,
                                derive_settings(self.ctx.render_settings()),
                            ),
                            // PdfCraft patch: a pattern cell is painted as one artwork, so its
                            // content is never remapped by the reading-colours setting.
                            color_replacement: None,
                            in_annotations: false,
                            knockout: Vec::new(),
                            erasing: false,
                            erase_groups: 0,
                            cur_mask: None,
                            inside_pattern: true,
                            soft_mask_cache: HashMap::default(),
                            outline_cache: self.outline_cache.clone(),
                            in_type3_glyph: false,
                            scaler: self.scaler,
                            preview: self.preview,
                            image_transparency_stack: Vec::new(),
                        };
                        let mut initial_transform = Affine::scale_non_uniform(xs as f64, ys as f64)
                            * Affine::translate((-bbox.x0, -bbox.y0));
                        t.interpret(&mut renderer, initial_transform, is_stroke);
                        let mut pix = Pixmap::new(pix_width, pix_height);
                        renderer.ctx.flush();
                        let mut resources = vello_cpu::Resources::default();
                        renderer.ctx.render_to_pixmap(&mut resources, &mut pix);

                        // TODO: Fix these
                        if x_step < 0.0 {
                            initial_transform *=
                                Affine::new([-1.0, 0.0, 0.0, 1.0, scaled_width as f64, 0.0]);
                        }

                        if y_step < 0.0 {
                            initial_transform *=
                                Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, scaled_height as f64]);
                        }

                        paint_transform =
                            path_transform.inverse() * t.matrix * initial_transform.inverse();

                        let image = Image {
                            image: ImageSource::Pixmap(Arc::new(pix)),
                            sampler: ImageSampler {
                                x_extend: peniko::Extend::Repeat,
                                y_extend: peniko::Extend::Repeat,
                                quality: ImageQuality::Medium,
                                alpha: 1.0,
                            },
                        };

                        PaintType::Image(image)
                    }
                }
            }
        };

        self.ctx.set_paint_transform(paint_transform);
        self.ctx.set_paint(paint);

        PaintSetup::Clip(clip_path)
    }

    fn stroke_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &Paint<'_>,
        stroke_props: &StrokeProps,
        is_text: bool,
    ) {
        self.ctx.set_transform(transform);
        let reach = self.set_stroke_properties(stroke_props, is_text, path);
        // PdfCraft patch: see `may_paint_canvas`.
        if !may_paint_canvas(path, transform, reach, self.ctx.width(), self.ctx.height()) {
            return;
        }

        // PdfCraft patch: apply the reading-colours mapping before painting.
        let paint = self.replaced(paint, is_text);
        let PaintSetup::Clip(clip_path) = self.set_paint(&paint, path, true) else { return };
        if let Some(clip_path) = clip_path.as_ref() {
            self.push_clip_path_inner(clip_path, FillRule::NonZero);
        }
        self.ctx.stroke_path(path);
        if clip_path.is_some() {
            self.ctx.pop_clip_path();
        }
    }

    fn fill_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &Paint<'_>,
        fill_rule: FillRule,
        // PdfCraft patch: distinguishes glyph fills from plain line art for the
        // reading-colours mapping.
        is_text: bool,
    ) {
        self.ctx.set_fill_rule(convert_fill_rule(fill_rule));
        self.ctx.set_transform(transform);
        // PdfCraft patch: see `may_paint_canvas`.
        if !may_paint_canvas(path, transform, 0.0, self.ctx.width(), self.ctx.height()) {
            return;
        }

        // PdfCraft patch: apply the reading-colours mapping before painting.
        let paint = self.replaced(paint, is_text);
        let PaintSetup::Clip(clip_path) = self.set_paint(&paint, path, false) else { return };
        if let Some(clip_path) = clip_path.as_ref() {
            self.push_clip_path_inner(clip_path, fill_rule);
        }

        self.ctx.fill_path(path);

        if clip_path.is_some() {
            self.ctx.pop_clip_path();
        }
    }

    fn fill_glyph<'a>(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
    ) {
        match glyph {
            Glyph::Outline(o) => {
                let base_outline = self.cached_outline(o);

                self.fill_path(
                    base_outline.as_ref(),
                    transform * glyph_transform,
                    paint,
                    FillRule::NonZero,
                    // PdfCraft patch: glyph outlines are text for the reading-colours mapping.
                    true,
                );
            }
            Glyph::Type3(s) => {
                // PdfCraft patch: a type 3 glyph's content stream keeps painting as line art
                // (its internal ops reach `fill_path`/`stroke_path` with `is_text: false`), so
                // reading colours remap it only when line-art replacement is on.
                self.in_type3_glyph = true;
                s.interpret(self, transform, glyph_transform, paint);
                self.in_type3_glyph = false;
            }
        }
    }

    fn stroke_glyph<'a>(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        stroke_props: &StrokeProps,
    ) {
        match glyph {
            Glyph::Outline(o) => {
                let base_outline = self.cached_outline(o);

                self.stroke_path(
                    &(glyph_transform * base_outline.as_ref().clone()),
                    transform,
                    paint,
                    stroke_props,
                    true,
                );
            }
            Glyph::Type3(s) => {
                s.interpret(self, transform, glyph_transform, paint);
            }
        }
    }

    fn cached_outline(&self, glyph: &hayro_interpret::font::OutlineGlyph) -> Rc<BezPath> {
        let id = glyph.identifier().cache_key();

        if let Some(path) = self.outline_cache.borrow().get(&id) {
            return path.clone();
        }

        let path = Rc::new(glyph.outline());
        self.outline_cache.borrow_mut().insert(id, path.clone());
        path
    }

    fn force_images_may_have_transparency(&self) -> bool {
        self.cur_mask.is_some()
            || self
                .image_transparency_stack
                .last()
                .copied()
                .unwrap_or(false)
            || self.ctx.blend_mode() != peniko::BlendMode::default()
    }
}

impl<'a> Device<'a> for Renderer {
    fn draw_image(&mut self, image: hayro_interpret::Image<'a, '_>, mut transform: Affine) {
        // PdfCraft patch (22): an erase pass paints nothing; an unmasked image's rectangle was
        // cleared by `knockout_shape`, and a masked image's shape is left alone.
        if self.erasing {
            return;
        }
        let Some((target_width, target_height)) =
            self.image_draw_size(image.width(), image.height(), transform)
        else {
            return;
        };
        self.ctx.set_paint_transform(Affine::IDENTITY);
        self.ctx.set_aliasing_threshold(Some(1));

        match image {
            hayro_interpret::Image::Stencil(s) => {
                s.with_stencil(
                    |stencil, paint| {
                        transform *= Affine::scale_non_uniform(
                            stencil.scale_factors.0 as f64,
                            stencil.scale_factors.1 as f64,
                        );

                        // PdfCraft patch: a stencil mask is line art in disguise, so its solid
                        // paint goes through the reading-colours mapping (line-art gate) like
                        // every other solid fill; baked in raw, its ink would stay at its
                        // original colour and vanish into a mapped background.
                        let paint = &self.replaced(paint, false);
                        match paint {
                            Paint::Color(c) => {
                                let color = c.to_rgba().to_rgba8();
                                let (rgb_bytes, alpha) = (
                                    stencil
                                        .data
                                        .iter()
                                        .flat_map(|_| [color[0], color[1], color[2]])
                                        .collect::<Vec<u8>>(),
                                    color[3],
                                );

                                let blend_mode = self.ctx.blend_mode();
                                let push_layer =
                                    alpha != 255 || blend_mode != peniko::BlendMode::default();
                                self.ctx.set_transform(transform);
                                if push_layer {
                                    self.ctx.push_layer(
                                        None,
                                        Some(blend_mode),
                                        Some(alpha as f32 / 255.0),
                                        None,
                                        None,
                                    );
                                }
                                let old_rule = *self.ctx.fill_rule();
                                self.ctx.set_fill_rule(Fill::NonZero);

                                let rgb_data = ImageData::Rgb(RgbData {
                                    data: rgb_bytes,
                                    width: stencil.width,
                                    height: stencil.height,
                                    interpolate: stencil.interpolate,
                                    scale_factors: stencil.scale_factors,
                                });
                                self.draw_image(rgb_data, Some(stencil));

                                if push_layer {
                                    self.ctx.pop_layer();
                                }

                                self.ctx.set_fill_rule(old_rule);
                            }
                            Paint::Pattern(_) => {
                                let (width, height) = (self.ctx.width(), self.ctx.height());
                                let stencil_rect = Rect::new(
                                    0.0,
                                    0.0,
                                    stencil.width as f64,
                                    stencil.height as f64,
                                );
                                let mask_pix = {
                                    let rgb_bytes = ImageData::Rgb(RgbData {
                                        data: vec![
                                            255;
                                            stencil.width as usize
                                                * stencil.height as usize
                                                * 3
                                        ],
                                        width: stencil.width,
                                        height: stencil.height,
                                        interpolate: stencil.interpolate,
                                        scale_factors: stencil.scale_factors,
                                    });
                                    let mut sub_renderer = Self {
                                        ctx: RenderContext::new_with(
                                            width,
                                            height,
                                            derive_settings(self.ctx.render_settings()),
                                        ),
                                        // PdfCraft patch: a stencil mask must keep its original
                                        // coverage values, so its content is never remapped by
                                        // the reading-colours setting.
                                        color_replacement: None,
                                        in_annotations: false,
                                        knockout: Vec::new(),
                                        erasing: false,
                                        erase_groups: 0,
                                        inside_pattern: false,
                                        soft_mask_cache: HashMap::default(),
                                        outline_cache: self.outline_cache.clone(),
                                        cur_mask: None,
                                        in_type3_glyph: false,
                                        scaler: self.scaler,
                                        preview: self.preview,
                                        image_transparency_stack: Vec::new(),
                                    };
                                    let mut sub_pix = Pixmap::new(width, height);
                                    sub_renderer.ctx.set_transform(transform);
                                    sub_renderer.draw_image(rgb_bytes, Some(stencil));
                                    sub_renderer.ctx.flush();
                                    let mut resources = vello_cpu::Resources::default();
                                    sub_renderer
                                        .ctx
                                        .render_to_pixmap(&mut resources, &mut sub_pix);
                                    sub_pix
                                };

                                self.ctx.push_layer(
                                    None,
                                    Some(self.ctx.blend_mode()),
                                    None,
                                    Some(Mask::new_luminance(&mask_pix)),
                                    None,
                                );
                                self.ctx.set_transform(transform);

                                // PdfCraft patch: a stencil mask is painted with the fill
                                // (non-stroking) paint; `true` drew an uncoloured tiling pattern
                                // in its stroke colour (black by default) (#2975).
                                if let PaintSetup::Clip(clip_path) =
                                    self.set_paint(paint, &stencil_rect.to_path(0.1), false)
                                {
                                    if let Some(clip_path) = clip_path.as_ref() {
                                        self.push_clip_path_inner(clip_path, FillRule::NonZero);
                                    }
                                    self.ctx.fill_rect(&stencil_rect);
                                    if clip_path.is_some() {
                                        self.ctx.pop_clip_path();
                                    }
                                }

                                self.ctx.pop_layer();
                            }
                        };
                    },
                    Some((target_width, target_height)),
                );
            }
            hayro_interpret::Image::Raster(r) => {
                r.with_rgba(
                    |image, alpha| {
                        let (sx, sy) = image.scale_factors();
                        transform *= Affine::scale_non_uniform(sx as f64, sy as f64);
                        self.ctx.set_transform(transform);
                        self.draw_image(image, alpha);
                    },
                    Some((target_width, target_height)),
                );
            }
        }

        self.ctx.set_aliasing_threshold(None);
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        self.push_clip_path_inner(&clip_path.path, clip_path.fill);
    }

    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'_>>,
        blend_mode: BlendMode,
    ) {
        // PdfCraft patch (22): nothing is layered in a knockout erase pass.
        if self.erasing {
            self.erase_groups += 1;
            return;
        }
        let settings = *self.ctx.render_settings();
        let force_images_may_have_transparency = mask.is_some()
            || blend_mode != BlendMode::Normal
            || self
                .image_transparency_stack
                .last()
                .copied()
                .unwrap_or(false);
        self.ctx.push_layer(
            None,
            Some(convert_blend_mode(blend_mode)),
            Some(opacity),
            // TODO: Deduplicate
            mask.map(|m| {
                let width = self.ctx.width();
                let height = self.ctx.height();

                self.soft_mask_cache
                    .entry(m.cache_key())
                    .or_insert_with(|| draw_soft_mask(&m, settings, width, height, self.preview))
                    .clone()
            }),
            None,
        );
        self.image_transparency_stack
            .push(force_images_may_have_transparency);
        self.knockout.push(false);
    }

    fn pop_clip_path(&mut self) {
        self.ctx.pop_clip_path();
    }

    fn pop_transparency_group(&mut self) {
        if self.erasing && self.erase_groups > 0 {
            self.erase_groups -= 1;
            return;
        }
        self.image_transparency_stack.pop();
        self.knockout.pop();
        self.ctx.pop_layer();
    }

    /// PdfCraft patch (22): the group just pushed is a knockout group. Its elements are drawn
    /// with `Compose::Copy`, which (weighted by coverage) replaces what earlier elements drew
    /// where the element's shape is, so each element composites with the group's initial
    /// backdrop (transparent: an isolated group, and the non-isolated approximation every
    /// group layer here already makes) instead of with the elements before it. Against a
    /// transparent backdrop the element's own blend mode has nothing to blend with, so the
    /// mix is Normal. This covers paths, strokes, glyph outlines and shadings. Images and
    /// nested groups are drawn into a layer of their own, which can't be copied (it covers the
    /// whole canvas, transparent where nothing was drawn): what is under their shape is cleared
    /// first (`knockout_shape`, the knockout erase pass) and the layer composites over it.
    fn set_knockout(&mut self) {
        if let Some(top) = self.knockout.last_mut() {
            *top = true;
            self.ctx.set_blend_mode(knockout_blend_mode());
        }
    }

    /// PdfCraft patch (22): in a knockout group, clear what the group drew under `shape` (a
    /// transparent fill with `Compose::Copy`, weighted by coverage) before an element drawn into
    /// a layer of its own composites there, so the element replaces it.
    fn knockout_erase_wanted(&mut self) -> bool {
        self.in_knockout() && !self.erasing
    }

    fn begin_knockout_erase(&mut self) {
        self.erasing = true;
        self.erase_groups = 0;
    }

    fn end_knockout_erase(&mut self) {
        self.erasing = false;
        self.erase_groups = 0;
        self.ctx.set_blend_mode(knockout_blend_mode());
    }

    fn in_knockout_group(&mut self) -> bool {
        self.in_knockout()
    }

    fn knockout_image(&mut self, image: &hayro_interpret::RasterImage<'_>, transform: Affine) {
        if !self.in_knockout() {
            return;
        }
        // Only an image `draw_image` will draw, decoded at the size it draws it (the same
        // decode, so a cached decode serves both).
        let Some(size) = self.image_draw_size(image.width(), image.height(), transform) else {
            return;
        };
        let mut decodes = false;
        image.with_rgba(|_, _| decodes = true, Some(size));
        if decodes {
            let rect = Rect::new(0.0, 0.0, f64::from(image.width()), f64::from(image.height()));
            self.knockout_shape(&(transform * rect.to_path(0.1)));
        }
    }

    fn knockout_shape(&mut self, shape: &BezPath) {
        if !self.in_knockout() {
            return;
        }
        let old_transform = *self.ctx.transform();
        let old_rule = *self.ctx.fill_rule();
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_fill_rule(Fill::NonZero);
        self.ctx.set_blend_mode(knockout_blend_mode());
        self.ctx.set_paint(AlphaColor::<Srgb>::new([0.0, 0.0, 0.0, 0.0]));
        self.ctx.fill_path(shape);
        self.ctx.set_fill_rule(old_rule);
        self.ctx.set_transform(old_transform);
    }

    fn set_soft_mask(&mut self, mask: Option<SoftMask<'_>>) {
        let settings = *self.ctx.render_settings();
        self.cur_mask = mask.map(|m| {
            let width = self.ctx.width();
            let height = self.ctx.height();

            self.soft_mask_cache
                .entry(m.cache_key())
                .or_insert_with(|| draw_soft_mask(&m, settings, width, height, self.preview))
                .clone()
        });
        if let Some(mask) = self.cur_mask.clone() {
            self.ctx.set_mask(mask);
        } else {
            self.ctx.reset_mask();
        }
    }

    fn draw_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &Paint<'_>,
        draw_mode: &PathDrawMode,
    ) {
        match draw_mode {
            PathDrawMode::Fill(f) => {
                Self::fill_path(self, path, transform, paint, *f, false);
            }
            PathDrawMode::Stroke(s) => {
                Self::stroke_path(self, path, transform, paint, s, false);
            }
        }
    }

    /// PdfCraft patch (17): the reading-colours mapping is suspended for annotations.
    fn begin_annotations(&mut self) {
        self.in_annotations = true;
    }

    /// PdfCraft patch (17): back to document content; reading colours apply again.
    fn end_annotations(&mut self) {
        self.in_annotations = false;
    }

    fn draw_rect(
        &mut self,
        rect: &Rect,
        transform: Affine,
        paint: &Paint<'_>,
        draw_mode: &PathDrawMode,
    ) {
        let path = rect.to_path(0.1);
        match draw_mode {
            PathDrawMode::Fill(fill_rule) => {
                self.ctx.set_fill_rule(convert_fill_rule(*fill_rule));
                self.ctx.set_transform(transform);
                // PdfCraft patch: see `may_paint_canvas`.
                if !may_paint_canvas(&path, transform, 0.0, self.ctx.width(), self.ctx.height()) {
                    return;
                }

                // PdfCraft patch: apply the reading-colours mapping before painting.
                let paint = self.replaced(paint, false);
                let PaintSetup::Clip(clip_path) = self.set_paint(&paint, &path, false) else { return };
                if let Some(clip_path) = clip_path.as_ref() {
                    self.push_clip_path_inner(clip_path, *fill_rule);
                }

                self.ctx.fill_rect(rect);

                if clip_path.is_some() {
                    self.ctx.pop_clip_path();
                }
            }
            PathDrawMode::Stroke(s) => {
                Self::stroke_path(self, &path, transform, paint, s, false);
            }
        }
    }

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        draw_mode: &GlyphDrawMode,
    ) {
        match draw_mode {
            GlyphDrawMode::Fill => {
                Self::fill_glyph(self, glyph, transform, glyph_transform, paint);
            }
            GlyphDrawMode::Stroke(s) => {
                Self::stroke_glyph(self, glyph, transform, glyph_transform, paint, s);
            }
            GlyphDrawMode::Invisible | GlyphDrawMode::Clip => {
                // PdfCraft patch: neither mode paints; clipping was installed by the interpreter.
            }
        }
    }

    fn set_blend_mode(&mut self, blend_mode: BlendMode) {
        // PdfCraft patch (22): inside a knockout group every element replaces what it covers.
        let mode = if self.in_knockout() {
            knockout_blend_mode()
        } else {
            convert_blend_mode(blend_mode)
        };
        self.ctx.set_blend_mode(mode);
    }
}

/// PdfCraft patch (backport of hayro 0.8): largest per-channel error allowed when approximating
/// a shading function by gradient stops.
const NATIVE_GRADIENT_TOLERANCE: f32 = 0.01;

/// PdfCraft patch (backport of hayro 0.8): a `vello_cpu` gradient for an approximated shading.
fn native_gradient(native: hayro_interpret::gradient::SvgGradient) -> PaintType {
    use hayro_interpret::gradient::SvgGradientKind;

    let stops = native
        .stops
        .iter()
        .map(|stop| ColorStop {
            offset: stop.offset,
            color: DynamicColor::from_alpha_color(AlphaColor::<Srgb>::new(stop.color)),
        })
        .collect::<Vec<_>>();

    match native.kind {
        SvgGradientKind::Linear { start, end } => Gradient::new_linear(start, end),
        SvgGradientKind::Radial {
            start_center,
            start_radius,
            end_center,
            end_radius,
        } => Gradient::new_two_point_radial(start_center, start_radius, end_center, end_radius),
    }
    .with_extend(peniko::Extend::Pad)
    .with_stops(stops.as_slice())
    .into()
}

// TODO: Deduplicate with hayro-svg?
fn render_shading_texture(
    path_bbox: Rect,
    shading_pattern: &EncodedShadingPattern,
) -> (Vec<PremulRgba8>, u32, u32, Affine, bool) {
    let base_width = (path_bbox.width() as f32).max(1.0);
    let base_height = (path_bbox.height() as f32).max(1.0);

    let width = (base_width).ceil() as u32;
    let height = (base_height).ceil() as u32;

    let (x_advance, y_advance) = x_y_advances(&shading_pattern.base_transform);

    let mut buf = vec![PremulRgba8::from_u32(0); width as usize * height as usize];
    let mut start_point = shading_pattern.base_transform
        * Affine::translate((0.5, 0.5))
        * Point::new(path_bbox.x0, path_bbox.y0);
    let mut may_have_transparency = false;

    for row in buf.chunks_exact_mut(width as usize) {
        let mut point = start_point;

        for pixel in row {
            let sample = shading_pattern.sample(point);
            *pixel = AlphaColor::<Srgb>::new(sample).premultiply().to_rgba8();
            may_have_transparency |= pixel.a != 255;

            point += x_advance;
        }

        start_point += y_advance;
    }

    (
        buf,
        width,
        height,
        Affine::translate((path_bbox.x0, path_bbox.y0)),
        may_have_transparency,
    )
}

fn draw_soft_mask(mask: &SoftMask<'_>, settings: RenderSettings, width: u16, height: u16, preview: bool) -> Mask {
    let mut renderer = Renderer {
        ctx: RenderContext::new_with(width, height, derive_settings(&settings)),
        // PdfCraft patch: a soft mask must keep its original luminance values, so its
        // content is never remapped by the reading-colours setting.
        color_replacement: None,
        in_annotations: false,
        knockout: Vec::new(),
        erasing: false,
        erase_groups: 0,
        inside_pattern: false,
        cur_mask: None,
        soft_mask_cache: HashMap::default(),
        outline_cache: Rc::new(std::cell::RefCell::new(HashMap::new())),
        in_type3_glyph: false,
        scaler: Scaler::new(ResamplingFunction::CatmullRom),
        preview,
        image_transparency_stack: Vec::new(),
    };

    let bg_color = mask.background_color().to_rgba();
    let apply_bg = bg_color.to_rgba8() != BLACK.to_rgba8().to_u8_array();

    if apply_bg {
        renderer
            .ctx
            .set_paint(AlphaColor::<Srgb>::new(bg_color.components()));
        renderer
            .ctx
            .fill_rect(&Rect::new(0.0, 0.0, width as f64, height as f64));
        renderer.ctx.push_layer(None, None, None, None, None);
    }

    mask.interpret(&mut renderer);

    if apply_bg {
        renderer.ctx.pop_layer();
    }

    let mut pix = Pixmap::new(width, height);
    renderer.ctx.flush();
    let mut resources = vello_cpu::Resources::default();
    renderer.ctx.render_to_pixmap(&mut resources, &mut pix);

    let mut rendered_mask = match mask.mask_type() {
        MaskType::Luminosity => Mask::new_luminance(&pix),
        MaskType::Alpha => Mask::new_alpha(&pix),
    };

    if let Some(transfer_function) = mask.transfer_function() {
        let mut map = Vec::new();

        for y in 0..rendered_mask.height() {
            for x in 0..rendered_mask.width() {
                map.push(
                    (transfer_function.apply(rendered_mask.sample(x, y) as f32 / 255.0) * 255.0
                        + 0.5) as u8,
                );
            }
        }

        rendered_mask = Mask::from_parts(map, rendered_mask.width(), rendered_mask.height());
    }

    rendered_mask
}

pub(crate) fn max_factor(transform: &Affine) -> f32 {
    let scale_skew_transform = {
        let c = transform.as_coeffs();
        Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
    };

    let x_advance = scale_skew_transform * Point::new(1.0, 0.0);
    let y_advance = scale_skew_transform * Point::new(0.0, 1.0);

    x_advance
        .to_vec2()
        .length()
        .max(y_advance.to_vec2().length()) as f32
}

pub(crate) fn x_y_advances(transform: &Affine) -> (Vec2, Vec2) {
    let scale_skew_transform = {
        let c = transform.as_coeffs();
        Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
    };

    let x_advance = scale_skew_transform * Point::new(1.0, 0.0);
    let y_advance = scale_skew_transform * Point::new(0.0, 1.0);

    (
        Vec2::new(x_advance.x, x_advance.y),
        Vec2::new(y_advance.x, y_advance.y),
    )
}

fn convert_fill_rule(fill_rule: FillRule) -> Fill {
    match fill_rule {
        FillRule::NonZero => Fill::NonZero,
        FillRule::EvenOdd => Fill::EvenOdd,
    }
}

/// PdfCraft patch (22): how an element of a knockout group is drawn (see `set_knockout`).
fn knockout_blend_mode() -> peniko::BlendMode {
    peniko::BlendMode::new(Mix::Normal, Compose::Copy)
}

fn convert_blend_mode(blend_mode: BlendMode) -> peniko::BlendMode {
    let mix = match blend_mode {
        BlendMode::Normal => Mix::Normal,
        BlendMode::Multiply => Mix::Multiply,
        BlendMode::Screen => Mix::Screen,
        BlendMode::Overlay => Mix::Overlay,
        BlendMode::Darken => Mix::Darken,
        BlendMode::Lighten => Mix::Lighten,
        BlendMode::ColorDodge => Mix::ColorDodge,
        BlendMode::ColorBurn => Mix::ColorBurn,
        BlendMode::HardLight => Mix::HardLight,
        BlendMode::SoftLight => Mix::SoftLight,
        BlendMode::Difference => Mix::Difference,
        BlendMode::Exclusion => Mix::Exclusion,
        BlendMode::Hue => Mix::Hue,
        BlendMode::Saturation => Mix::Saturation,
        BlendMode::Color => Mix::Color,
        BlendMode::Luminosity => Mix::Luminosity,
    };

    peniko::BlendMode::new(mix, Compose::SrcOver)
}

/// PdfCraft patch: how far a stroke's paint reaches past its path, in user units: half the width
/// times the larger of the miter limit and √2 (a miter join or a square cap). A miter limit that
/// isn't finite leaves miters unbounded, so the reach is too.
fn stroke_reach(stroke: &kurbo::Stroke) -> f64 {
    let miter = stroke.miter_limit.abs();
    stroke.width / 2.0
        * if miter.is_finite() {
            miter.max(std::f64::consts::SQRT_2)
        } else {
            f64::INFINITY
        }
}

/// PdfCraft patch (20): whether a cell's scale can draw it at full size: finite and positive. A
/// `/BBox` too thin to divide by (1e-40 wide passes the zero-area check) gives an infinite scale,
/// which would send non-finite geometry to the cell renderer; such a cell keeps the old path.
pub fn usable_cell_scale(xs: f32, ys: f32) -> bool {
    xs.is_finite() && ys.is_finite() && xs > 0.0 && ys > 0.0
}

/// PdfCraft patch (20): whether `path` (drawn with `path_transform`, its paint reaching `reach`
/// device pixels past it), mapped into the space of a tiling pattern with matrix `matrix`, can
/// only meet the first cell's `/BBox`: every other cell's content lies a whole step or more
/// away. False when the matrix can't be inverted or the bounds aren't finite.
pub(crate) fn shows_one_cell(matrix: &Affine, path_transform: &Affine, path: &BezPath, reach: f64, bbox: Rect, x_step: f32, y_step: f32) -> bool {
    if matrix.determinant().abs() < 1e-12 || !matrix.determinant().is_finite() {
        return false;
    }
    let device = (*path_transform * path.clone()).bounding_box().inflate(reach, reach);
    let bounds = (matrix.inverse() * device.to_path(0.1)).bounding_box();
    let (sx, sy) = (f64::from(x_step.abs()), f64::from(y_step.abs()));
    let finite = [bounds.x0, bounds.y0, bounds.x1, bounds.y1, sx, sy].iter().all(|v| v.is_finite());
    // Cell k's content spans `bbox + k × step`; with k = ±1 out of reach, so is every other.
    finite
        && bounds.x1 < bbox.x0 + sx
        && bounds.x0 > bbox.x1 - sx
        && bounds.y1 < bbox.y0 + sy
        && bounds.y0 > bbox.y1 - sy
}

/// PdfCraft patch: the scale for a tiling cell, lowered so that the cell pixmap (`|step| ×
/// scale` pixels a side) stays within `max_pixels`. Geometry is unchanged (the pattern transform
/// is derived from the scale); an over-large step only renders at lower resolution.
pub fn tiling_cell_scale(scale: f32, step: f32, max_pixels: f32) -> f32 {
    let step = step.abs();
    if step.is_finite() && step > 0.0 && scale * step > max_pixels {
        max_pixels / step
    } else {
        scale
    }
}

// PdfCraft patch: unit tests for area averaging and the snapping of minified images.
#[cfg(test)]
mod area_average_tests {
    use super::{
        MAX_AREA_SCRATCH_BYTES, area_average, area_scratch_byte_len, image_draw_transform,
        snap_minified_image,
    };
    use kurbo::{Affine, Rect};

    /// A snapped image is drawn 1:1 at its snapped corner only when its raster has the snapped
    /// size. A resize that falls back to the source (a failed allocation or resampler) keeps the
    /// image's own transform: placed at the snapped corner, an 8 × 8 source meant for 4 × 4
    /// device pixels covered 8 × 8.
    #[test]
    fn a_resize_that_falls_back_keeps_the_image_transform() {
        let transform = Affine::new([0.5, 0.0, 0.0, 0.5, 1.0, 1.0]);
        let snapped = snap_minified_image(&transform, (8, 8), (4, 4));
        assert_eq!(snapped, Some((4, 4, 1.0, 1.0)));
        let shrunk = image_draw_transform(transform, Affine::scale(2.0), (4, 4), snapped);
        assert_eq!(shrunk, Affine::translate((1.0, 1.0)));
        let fallback = image_draw_transform(transform, Affine::IDENTITY, (8, 8), snapped);
        let device = fallback.transform_rect_bbox(Rect::new(0.0, 0.0, 8.0, 8.0));
        let size = "the source keeps the image's size";
        assert_eq!(device, Rect::new(1.0, 1.0, 5.0, 5.0), "{size}");
        let unsnapped = image_draw_transform(transform, Affine::scale(2.0), (4, 4), None);
        assert_eq!(unsnapped, transform * Affine::scale(2.0));
    }

    /// Every `u64` sum `area_average` keeps besides its output counts against one budget: the
    /// column sums of a source row and the target row (two when enlarging the height). A cap on
    /// the column sums alone let them reach 256 MiB, twice the Catmull-Rom planning budget, for a
    /// one-row strip a tiny Flate stream decodes to. A strip over the budget is refused before
    /// anything is allocated or a row is read.
    #[test]
    fn area_average_refuses_strips_over_its_scratch_budget() {
        let samples = (MAX_AREA_SCRATCH_BYTES / 8) as u32;
        let budget = Some(MAX_AREA_SCRATCH_BYTES);
        assert_eq!(area_scratch_byte_len(samples - 1, 1, 1, false), budget);
        assert_eq!(area_scratch_byte_len(samples - 2, 1, 1, true), budget);
        assert_eq!(area_scratch_byte_len(samples, 1, 1, false), None);
        assert_eq!(area_scratch_byte_len(samples - 1, 1, 1, true), None);
        assert_eq!(area_scratch_byte_len(samples / 4 - 1, 1, 4, false), budget);
        assert_eq!(area_scratch_byte_len(samples / 4, 1, 4, false), None);
        assert_eq!(area_scratch_byte_len(u32::MAX, u32::MAX, 4, true), None);

        let mut reads = 0;
        let mut read = |_: usize, _: u64, _: &mut [u64]| {
            reads += 1;
            Some(())
        };
        assert_eq!(area_average::<1>((samples, 1), (16, 1), &mut read), None);
        assert_eq!(
            area_average::<4>((samples / 4, 1), (16, 1), &mut read),
            None
        );
        assert_eq!(area_average::<1>((samples - 1, 1), (1, 2), &mut read), None);
        assert_eq!(reads, 0, "a row was read past the budget");
    }

    /// An image shrunk across but enlarged down (a long strip drawn as a tall, narrow box) must
    /// read each source row a bounded number of times. Repeating a source row once per target
    /// row it covers walks the whole source width again for every target row: a validated
    /// 1,100,000 × 1 image drawn 16 × 8192 visited about nine billion samples per pass, which
    /// stalls the render (and the browser's inline renderer). A row straddling a target-row
    /// boundary is read twice when shrinking, so two is the most allowed.
    #[test]
    fn area_average_reads_each_source_row_at_most_twice() {
        for (source, target) in [((1024, 1), (4, 1024)), ((1024, 3), (4, 1000)), ((1024, 1024), (4, 4))] {
            let mut reads = vec![0usize; source.1 as usize];
            let out = area_average::<1>(source, target, |y, _, _| {
                *reads.get_mut(y)? += 1;
                Some(())
            });
            assert!(out.is_some(), "{source:?} → {target:?}");
            let most = reads.iter().max().copied().unwrap_or(0);
            assert!(most <= 2, "{source:?} → {target:?}: a source row was read {most} times");
        }
    }

    /// Tiles are rendered with the page shifted by whole device pixels, so a shrunk image must
    /// snap to the same rectangle, shifted by the same amount, whatever the tile's origin.
    /// `f64::round`, used before, rounds halves away from zero: an 8-column image drawn 4 pixels
    /// wide at x = 0.5 snapped to 1..5 in the full page, but to −1..4 (five columns) in a tile
    /// starting at x = 1, so the tile averaged different source areas and seams showed at tile
    /// edges.
    #[test]
    fn snapped_minified_image_is_invariant_under_whole_pixel_shifts() {
        let snap = |x: f64, y: f64| {
            snap_minified_image(&Affine::new([0.5, 0.0, 0.0, 0.5, x, y]), (8, 8), (4, 4))
        };
        for origin in [0.5, 1.5, 2.5, 0.25, 0.75, 3.0] {
            for shift in [1.0, 2.0, 3.0] {
                let page = snap(origin, origin);
                let tile = snap(origin - shift, origin - shift);
                let expected = page.map(|(w, h, x0, y0)| (w, h, x0 - shift, y0 - shift));
                assert_eq!(tile, expected, "origin {origin}, tile shifted by {shift}");
            }
        }
    }

    /// The overlap of source span `s` and target span `t` along one axis, in units where a
    /// source pixel is `target` long and a target pixel `source` long.
    fn overlap(s: u64, t: u64, source: u64, target: u64) -> u64 {
        let lo = (s * target).max(t * source);
        let hi = ((s + 1) * target).min((t + 1) * source);
        hi.saturating_sub(lo)
    }

    /// Both ways through [`area_average`] (shrinking the height, and enlarging it, which reuses
    /// each source row) give each target pixel the coverage-weighted mean of the source, computed
    /// here pixel by pixel, for every size up to 6 × 6 each way.
    #[test]
    fn area_average_matches_a_pixel_by_pixel_reference() {
        const N: usize = 3;
        for (sw, sh, tw, th) in (1..=6u32).flat_map(|sw| {
            (1..=6u32).flat_map(move |sh| {
                (1..=6u32).flat_map(move |tw| (1..=6u32).map(move |th| (sw, sh, tw, th)))
            })
        }) {
            let samples: Vec<u8> = (0..sw * sh * N as u32)
                .map(|i| (i * 37 % 251) as u8)
                .collect();
            let row_len = sw as usize * N;
            let got = area_average::<N>((sw, sh), (tw, th), |y, weight, columns| {
                let row = samples.get(y * row_len..(y + 1) * row_len)?;
                for (column, sample) in columns.iter_mut().zip(row) {
                    *column += weight * u64::from(*sample);
                }
                Some(())
            });
            let total = u64::from(sw * sh);
            let mut want = Vec::new();
            for ty in 0..u64::from(th) {
                for tx in 0..u64::from(tw) {
                    for c in 0..N {
                        let mut sum = 0;
                        for sy in 0..u64::from(sh) {
                            for sx in 0..u64::from(sw) {
                                let weight = overlap(sx, tx, sw.into(), tw.into())
                                    * overlap(sy, ty, sh.into(), th.into());
                                let i = (sy * u64::from(sw) + sx) as usize * N + c;
                                sum += weight * u64::from(samples[i]);
                            }
                        }
                        want.push(((sum + total / 2) / total) as u8);
                    }
                }
            }
            assert_eq!(got, Some(want), "{sw} × {sh} → {tw} × {th}");
        }
    }
}
