//! Connected components of a binarized page (ClearScan stage 2).
//!
//! [`analyze`] labels a [`Bitmap`] into blobs and classifies each one
//! as a text glyph, a rule or drawing, a photograph, or speckle noise. The labeler is the
//! classical two-pass algorithm (Rosenfeld & Pfaltz 1966, with the union-find of Hoshen &
//! Kopelman 1976): the first pass gives each ink pixel a provisional label from the neighbors
//! already visited and records which labels are the same blob; the second pass rewrites every
//! pixel with its blob's root. Border tracing (Suzuki–Abe) is the vectorizer's job, one stage
//! later — labeling needs the filled region, not its contour.
//!
//! Eight-connectivity is the default. Scanned strokes touch on diagonals often enough that
//! four-connectivity splits one letter into several blobs; four-connectivity stays available
//! for a page where that splitting is what you want.
//!
//! Classification is geometric, and the thresholds ride the page rather than a DPI value the
//! bitmap does not carry. The body size is the median height of the components that are
//! neither speckles, nor rules, nor page-sized blocks. A blob is a glyph when it is about
//! that tall and its ink is a stroke (the median horizontal run is well under the blob's long
//! side); a small compact mark is punctuation; a long thin blob is a rule; a dense block
//! covering a quarter of the page on both sides is a photograph.

use crate::OcrError;
use crate::binarize::Bitmap;
use crate::grey::MAX_PIXELS;
use crate::preprocess::PreprocessedPage;

/// How ink pixels join into a blob.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connectivity {
    /// Orthogonal neighbors only.
    Four,
    /// Orthogonal and diagonal neighbors. The default: scanned strokes meet on diagonals.
    #[default]
    Eight,
}

/// What a blob is for, judged from its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentKind {
    /// A speckle: too little ink, or far smaller than the text on the page.
    Noise,
    /// A text glyph, including punctuation and a fused ligature that has not been cut yet.
    Glyph,
    /// A rule, border, or other drawing. Kept on the page, not turned into a glyph.
    Graphic,
    /// A dense picture. Kept on the page as a raster.
    Photo,
}

/// The knobs of [`analyze`]. Defaults suit a page of body text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterParams {
    /// Fewer ink pixels than this is noise. A 2×2 speckle (4) falls under the default 6; a
    /// 3×3 period (9) does not.
    pub min_area: u32,
    /// Long side of a glyph, as a fraction of the body size. Thin stems still pass: the short
    /// side is not gated, so an "l" stays a glyph.
    pub min_glyph_side_ratio: f64,
    /// Long side of a glyph, as a multiple of the body size. Headings fit; a diagram does not.
    pub max_glyph_side_ratio: f64,
    /// `max(width, height) / min(width, height)` at or above this, on a blob at least twice
    /// the body size long, is a rule or a border.
    pub graphic_aspect: f64,
    /// A mark whose long side is at most this fraction of the body size is punctuation when
    /// it is compact enough ([`punct_min_density`](Self::punct_min_density)).
    pub punct_side_ratio: f64,
    /// Minimum ink fraction of the bounding box for a punctuation mark.
    pub punct_min_density: f64,
    /// Minimum ink fraction of the bounding box for a glyph. Sparse dither fails it.
    pub glyph_min_density: f64,
    /// Median horizontal run over the long side. Above this the blob is a filled block, not
    /// a stroke. A stem scores low because the run is its thickness and the long side is its
    /// height; a filled square scores 1.
    pub max_thickness: f64,
    /// A blob whose box covers at least this fraction of the page on **both** sides, and is
    /// at least [`photo_density`](Self::photo_density) ink, is a photograph.
    pub photo_side_fraction: f64,
    /// Minimum ink fraction for that photograph test, and for a large filled block.
    pub photo_density: f64,
    /// Bounding boxes larger than this many pixels are not stored as a dense mask. The area,
    /// box, centroid and kind are still recorded; the pixels stay in the page bitmap.
    pub max_mask_pixels: u64,
}

impl Default for FilterParams {
    fn default() -> Self {
        FilterParams {
            min_area: 6,
            min_glyph_side_ratio: 0.4,
            max_glyph_side_ratio: 4.0,
            graphic_aspect: 8.0,
            punct_side_ratio: 0.45,
            punct_min_density: 0.3,
            glyph_min_density: 0.08,
            max_thickness: 0.55,
            photo_side_fraction: 0.25,
            photo_density: 0.55,
            max_mask_pixels: 1_048_576,
        }
    }
}

/// Body size used when the page has no component a median can be taken from.
const DEFAULT_BODY: f64 = 16.0;

/// The knobs of [`analyze`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalyzeOptions {
    pub connectivity: Connectivity,
    pub filter: FilterParams,
    /// Provisional labels above this are refused. Each label is one blob that had not yet
    /// merged with a neighbor, so the cap bounds the union-find before a noisy page can
    /// allocate one label per ink pixel.
    pub max_components: u32,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        AnalyzeOptions { connectivity: Connectivity::Eight, filter: FilterParams::default(), max_components: 1_000_000 }
    }
}

/// One connected blob.
///
/// `bounds` is half-open `[left, top, right, bottom]` in bitmap pixels, the same convention
/// as the OCR boxes, so the two meet in [`align`](mod@crate::align) without a conversion. `mask`, when
/// present, is that box: its `(0, 0)` is `(left, top)` of the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub bounds: [u32; 4],
    pub area: u32,
    /// Center of mass in bitmap pixels, x right, y down. Sub-pixel: it is the mean of the
    /// ink coordinates, which is what a later clustering stage aligns on.
    pub centroid: [f64; 2],
    pub mask: Option<Bitmap>,
    /// Median length of the horizontal ink runs, in pixels. A blob with no stored mask (the
    /// box was over the per-mask cap, or the page-wide mask budget was already spent) uses its
    /// short side, which classes a filled block as filled.
    pub stroke_width: f64,
    pub kind: ComponentKind,
}

impl Component {
    /// `bounds` as a float box for intersection tests.
    pub fn rect(&self) -> [f32; 4] {
        [self.bounds[0] as f32, self.bounds[1] as f32, self.bounds[2] as f32, self.bounds[3] as f32]
    }

    pub fn width(&self) -> u32 {
        self.bounds[2].saturating_sub(self.bounds[0])
    }

    pub fn height(&self) -> u32 {
        self.bounds[3].saturating_sub(self.bounds[1])
    }
}

/// Every blob on one bitmap, in reading order (top, then left).
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentSet {
    width: u32,
    height: u32,
    body_size: f64,
    filter: FilterParams,
    components: Vec<Component>,
}

impl ComponentSet {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The median text height the classification was measured against.
    pub fn body_size(&self) -> f64 {
        self.body_size
    }

    pub fn filter(&self) -> &FilterParams {
        &self.filter
    }

    pub fn components(&self) -> &[Component] {
        &self.components
    }
}

/// Label and classify `bitmap`. An empty bitmap is an empty set. A canvas past
/// [`MAX_PIXELS`], or more provisional blobs than [`AnalyzeOptions::max_components`], is an
/// error naming the limit.
pub fn analyze(bitmap: &Bitmap, options: &AnalyzeOptions) -> Result<ComponentSet, OcrError> {
    let (width, height) = (bitmap.width(), bitmap.height());
    let pixels = pixel_len(bitmap)?;
    if pixels == 0 {
        return Ok(empty_set(width, height, options.filter));
    }
    let labels = label(bitmap, pixels, options)?;
    let count = labels.iter().copied().max().unwrap_or(0);
    let raws = accumulate(&labels, width, count);
    let mut masks = fill_masks(&labels, width, &raws, options.filter.max_mask_pixels);
    let body = body_size(&raws, width, height, &options.filter);
    let mut components = Vec::new();
    for (index, raw) in raws.iter().enumerate().skip(1) {
        if raw.area == 0 {
            continue;
        }
        let mask = masks.get_mut(index).and_then(Option::take);
        components.push(finish(raw, mask, width, height, body, &options.filter));
    }
    components.sort_by(|a, b| a.bounds[1].cmp(&b.bounds[1]).then(a.bounds[0].cmp(&b.bounds[0])).then(a.area.cmp(&b.area)));
    Ok(ComponentSet { width, height, body_size: body, filter: options.filter, components })
}

/// [`analyze`] the binarization of a preprocessed page. Boxes come back in the deskewed frame.
pub fn analyze_page(page: &PreprocessedPage, options: &AnalyzeOptions) -> Result<ComponentSet, OcrError> {
    analyze(&page.binary, options)
}

fn empty_set(width: u32, height: u32, filter: FilterParams) -> ComponentSet {
    ComponentSet { width, height, body_size: DEFAULT_BODY, filter, components: Vec::new() }
}

fn pixel_len(bitmap: &Bitmap) -> Result<usize, OcrError> {
    let pixels = u64::from(bitmap.width()).saturating_mul(u64::from(bitmap.height()));
    if pixels > MAX_PIXELS {
        return Err(OcrError::Components(format!("the bitmap has {pixels} pixels; at most {MAX_PIXELS} are segmented")));
    }
    let len = usize::try_from(pixels).unwrap_or(usize::MAX);
    if bitmap.ink().len() != len {
        return Err(OcrError::Components("the bitmap does not match its canvas".into()));
    }
    Ok(len)
}

/// Provisional labels, then roots, then dense ids starting at 1. Paper stays 0.
fn label(bitmap: &Bitmap, pixels: usize, options: &AnalyzeOptions) -> Result<Vec<u32>, OcrError> {
    let width = bitmap.width() as usize;
    let height = bitmap.height() as usize;
    let mut labels = vec![0_u32; pixels];
    let mut uf = UnionFind::new(options.max_components);
    let eight = options.connectivity == Connectivity::Eight;
    let ink = bitmap.ink();
    for y in 0..height {
        for x in 0..width {
            let Some(index) = y.checked_mul(width).and_then(|row| row.checked_add(x)) else {
                continue;
            };
            if ink.get(index).copied().unwrap_or(0) == 0 {
                continue;
            }
            let neighbors = neighbor_labels(&labels, width, height, x, y, eight);
            match first_label(&neighbors) {
                Some(primary) => {
                    for n in neighbors {
                        if n != 0 && n != primary {
                            uf.union(primary, n);
                        }
                    }
                    if let Some(slot) = labels.get_mut(index) {
                        *slot = primary;
                    }
                }
                None => {
                    let id = uf.new_label().ok_or_else(|| {
                        OcrError::Components(format!(
                            "the page has more than {} connected components; it is too noisy to segment",
                            options.max_components
                        ))
                    })?;
                    if let Some(slot) = labels.get_mut(index) {
                        *slot = id;
                    }
                }
            }
        }
    }
    densify(&mut labels, &mut uf);
    Ok(labels)
}

fn neighbor_labels(labels: &[u32], width: usize, height: usize, x: usize, y: usize, eight: bool) -> [u32; 4] {
    let (x, y) = (x as isize, y as isize);
    let west = label_at(labels, width, height, x - 1, y);
    if !eight {
        return [west, label_at(labels, width, height, x, y - 1), 0, 0];
    }
    [west, label_at(labels, width, height, x - 1, y - 1), label_at(labels, width, height, x, y - 1), label_at(labels, width, height, x + 1, y - 1)]
}

fn label_at(labels: &[u32], width: usize, height: usize, x: isize, y: isize) -> u32 {
    if x < 0 || y < 0 {
        return 0;
    }
    let (x, y) = (x as usize, y as usize);
    if x >= width || y >= height {
        return 0;
    }
    let Some(index) = y.checked_mul(width).and_then(|row| row.checked_add(x)) else {
        return 0;
    };
    labels.get(index).copied().unwrap_or(0)
}

fn first_label(labels: &[u32; 4]) -> Option<u32> {
    labels.iter().copied().find(|label| *label != 0)
}

/// Replace provisional labels with dense ids `1..=count`.
fn densify(labels: &mut [u32], uf: &mut UnionFind) {
    let mut map = vec![0_u32; uf.len()];
    let mut next = 1_u32;
    for label in labels.iter_mut() {
        if *label == 0 {
            continue;
        }
        let root = uf.find(*label);
        let Some(slot) = map.get_mut(root as usize) else {
            *label = 0;
            continue;
        };
        if *slot == 0 {
            *slot = next;
            next = next.saturating_add(1);
        }
        *label = *slot;
    }
}

struct Raw {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    area: u32,
    sum_x: u64,
    sum_y: u64,
}

impl Raw {
    fn fresh() -> Raw {
        Raw { left: u32::MAX, top: u32::MAX, right: 0, bottom: 0, area: 0, sum_x: 0, sum_y: 0 }
    }

    fn box_area(&self) -> u64 {
        u64::from(self.right.saturating_sub(self.left)).saturating_mul(u64::from(self.bottom.saturating_sub(self.top)))
    }
}

/// Which blobs get a dense mask. `per_cap` limits one box; `total` limits the sum. Smaller
/// boxes win ties broken by the lower index, and a box over `per_cap` is never stored.
/// Stage 3 reuses it to plan which glyph clusters get a prototype canvas.
pub(crate) fn mask_plan(areas: &[u64], per_cap: u64, total: u64) -> Vec<bool> {
    let mut order: Vec<usize> = (0..areas.len()).filter(|&index| areas.get(index).copied().unwrap_or(0) > 0).collect();
    order.sort_by(|&a, &b| {
        let area = |index| areas.get(index).copied().unwrap_or(u64::MAX);
        area(a).cmp(&area(b)).then(a.cmp(&b))
    });
    let mut keep = vec![false; areas.len()];
    let mut used = 0_u64;
    for index in order {
        let Some(area) = areas.get(index).copied() else {
            continue;
        };
        let Some(next) = used.checked_add(area) else {
            continue;
        };
        if area > per_cap || next > total {
            continue;
        }
        if let Some(flag) = keep.get_mut(index) {
            *flag = true;
        }
        used = next;
    }
    keep
}

fn accumulate(labels: &[u32], width: u32, count: u32) -> Vec<Raw> {
    let mut raws = Vec::with_capacity(count as usize + 1);
    raws.push(Raw::fresh());
    for _ in 0..count {
        raws.push(Raw::fresh());
    }
    let width_us = width as usize;
    if width_us == 0 {
        return raws;
    }
    for (index, &id) in labels.iter().enumerate() {
        if id == 0 {
            continue;
        }
        let Some(raw) = raws.get_mut(id as usize) else {
            continue;
        };
        let x = (index % width_us) as u32;
        let y = (index / width_us) as u32;
        raw.left = raw.left.min(x);
        raw.top = raw.top.min(y);
        raw.right = raw.right.max(x.saturating_add(1));
        raw.bottom = raw.bottom.max(y.saturating_add(1));
        raw.area = raw.area.saturating_add(1);
        raw.sum_x = raw.sum_x.saturating_add(u64::from(x));
        raw.sum_y = raw.sum_y.saturating_add(u64::from(y));
    }
    raws
}

fn fill_masks(labels: &[u32], width: u32, raws: &[Raw], max_mask_pixels: u64) -> Vec<Option<Bitmap>> {
    // Small boxes first, so a page of glyphs keeps its masks when a few drawings are large.
    // The total is capped at a page: past that, further masks are omitted and classification
    // treats the blob as filled.
    let areas: Vec<u64> = raws.iter().map(Raw::box_area).collect();
    let keep = mask_plan(&areas, max_mask_pixels, MAX_PIXELS);
    let mut builds = raws
        .iter()
        .enumerate()
        .map(|(index, raw)| keep.get(index).copied().unwrap_or(false).then(|| MaskBuild::start(raw, max_mask_pixels)).flatten())
        .collect::<Vec<_>>();
    let width_us = width as usize;
    if width_us == 0 {
        return vec![None; raws.len()];
    }
    for (index, &id) in labels.iter().enumerate() {
        if id == 0 {
            continue;
        }
        let Some(build) = builds.get_mut(id as usize).and_then(|slot| slot.as_mut()) else {
            continue;
        };
        let x = (index % width_us) as u32;
        let y = (index / width_us) as u32;
        let local_x = x.saturating_sub(build.origin_x);
        let local_y = y.saturating_sub(build.origin_y);
        let Some(local) = (local_y as usize).checked_mul(build.width as usize).and_then(|row| row.checked_add(local_x as usize)) else {
            continue;
        };
        if let Some(flag) = build.ink.get_mut(local) {
            *flag = 1;
        }
    }
    builds.into_iter().map(|build| build.and_then(MaskBuild::finish)).collect()
}

struct MaskBuild {
    width: u32,
    height: u32,
    origin_x: u32,
    origin_y: u32,
    ink: Vec<u8>,
}

impl MaskBuild {
    fn start(raw: &Raw, max_mask_pixels: u64) -> Option<MaskBuild> {
        let width = raw.right.saturating_sub(raw.left);
        let height = raw.bottom.saturating_sub(raw.top);
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        if pixels == 0 || pixels > max_mask_pixels || pixels > MAX_PIXELS {
            return None;
        }
        let len = usize::try_from(pixels).ok()?;
        Some(MaskBuild { width, height, origin_x: raw.left, origin_y: raw.top, ink: vec![0; len] })
    }

    fn finish(self) -> Option<Bitmap> {
        Bitmap::from_ink(self.width, self.height, self.ink)
    }
}

fn body_size(raws: &[Raw], page_w: u32, page_h: u32, filter: &FilterParams) -> f64 {
    let mut heights = Vec::new();
    for raw in raws.iter().skip(1) {
        if raw.area < filter.min_area {
            continue;
        }
        let (w, h) = (raw.right.saturating_sub(raw.left), raw.bottom.saturating_sub(raw.top));
        if w == 0 || h == 0 {
            continue;
        }
        let (long, short) = (w.max(h) as f64, w.min(h) as f64);
        if short > 0.0 && long / short >= filter.graphic_aspect {
            continue;
        }
        if is_page_block(w, h, page_w, page_h, filter.photo_side_fraction) {
            continue;
        }
        heights.push(h as f64);
    }
    median(&mut heights).unwrap_or(DEFAULT_BODY)
}

fn is_page_block(w: u32, h: u32, page_w: u32, page_h: u32, fraction: f64) -> bool {
    page_w > 0 && page_h > 0 && w as f64 >= fraction * page_w as f64 && h as f64 >= fraction * page_h as f64
}

fn finish(raw: &Raw, mask: Option<Bitmap>, page_w: u32, page_h: u32, body: f64, filter: &FilterParams) -> Component {
    let stroke = mask.as_ref().map(stroke_width).unwrap_or_else(|| raw.right.saturating_sub(raw.left).min(raw.bottom.saturating_sub(raw.top)) as f64);
    let kind = classify(raw, stroke, page_w, page_h, body, filter);
    let area = raw.area.max(1);
    Component {
        bounds: [raw.left, raw.top, raw.right, raw.bottom],
        area: raw.area,
        centroid: [raw.sum_x as f64 / area as f64, raw.sum_y as f64 / area as f64],
        mask,
        stroke_width: stroke,
        kind,
    }
}

fn classify(raw: &Raw, stroke: f64, page_w: u32, page_h: u32, body: f64, filter: &FilterParams) -> ComponentKind {
    let w = raw.right.saturating_sub(raw.left);
    let h = raw.bottom.saturating_sub(raw.top);
    if w == 0 || h == 0 || raw.area < filter.min_area {
        return ComponentKind::Noise;
    }
    let density = raw.area as f64 / (f64::from(w) * f64::from(h));
    if is_page_block(w, h, page_w, page_h, filter.photo_side_fraction) && density >= filter.photo_density {
        return ComponentKind::Photo;
    }
    let long = f64::from(w.max(h));
    let short = f64::from(w.min(h));
    let aspect = if short > 0.0 { long / short } else { f64::MAX };
    let body = if body.is_finite() && body > 0.0 { body } else { DEFAULT_BODY };
    if aspect >= filter.graphic_aspect && long >= body * 2.0 {
        return ComponentKind::Graphic;
    }
    if long > body * filter.max_glyph_side_ratio {
        return if density >= filter.photo_density { ComponentKind::Photo } else { ComponentKind::Graphic };
    }
    let punct_cap = (body * filter.punct_side_ratio).max(3.0);
    if long <= punct_cap && density >= filter.punct_min_density {
        return ComponentKind::Glyph;
    }
    let thickness = if long > 0.0 { stroke / long } else { 1.0 };
    let glyph = long >= body * filter.min_glyph_side_ratio
        && density >= filter.glyph_min_density
        && thickness <= filter.max_thickness
        && aspect < filter.graphic_aspect;
    if glyph {
        return ComponentKind::Glyph;
    }
    if long < body * filter.min_glyph_side_ratio {
        return ComponentKind::Noise;
    }
    ComponentKind::Graphic
}

/// Median horizontal ink-run length. Runs longer than 255 count as 255, which is already past
/// any stroke a glyph has.
fn stroke_width(mask: &Bitmap) -> f64 {
    let mut hist = [0_u32; 256];
    let mut total = 0_u32;
    let width = mask.width() as usize;
    if width == 0 {
        return 0.0;
    }
    let ink = mask.ink();
    for row in ink.chunks(width) {
        total = total.saturating_add(record_runs(row, &mut hist));
    }
    median_hist(&hist, total)
}

fn record_runs(row: &[u8], hist: &mut [u32; 256]) -> u32 {
    let mut run = 0_u32;
    let mut runs = 0_u32;
    for &flag in row {
        if flag == 1 {
            run = run.saturating_add(1);
            continue;
        }
        if run > 0 {
            push_run(hist, run);
            runs = runs.saturating_add(1);
            run = 0;
        }
    }
    if run > 0 {
        push_run(hist, run);
        runs = runs.saturating_add(1);
    }
    runs
}

fn push_run(hist: &mut [u32; 256], run: u32) {
    let bin = run.min(255) as usize;
    if let Some(count) = hist.get_mut(bin) {
        *count = count.saturating_add(1);
    }
}

fn median_hist(hist: &[u32; 256], total: u32) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let target_lo = (total - 1) / 2;
    let target_hi = total / 2;
    let mut seen = 0_u32;
    let mut lo = None;
    let mut hi = None;
    for (len, &count) in hist.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let next = seen.saturating_add(count);
        if lo.is_none() && next > target_lo {
            lo = Some(len as f64);
        }
        if hi.is_none() && next > target_hi {
            hi = Some(len as f64);
        }
        seen = next;
        if hi.is_some() {
            break;
        }
    }
    match (lo, hi) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        (Some(a), None) => a,
        _ => 0.0,
    }
}

fn median(values: &mut Vec<f64>) -> Option<f64> {
    values.retain(|value| value.is_finite());
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let mid = values.len() / 2;
    let upper = values.get(mid).copied()?;
    if values.len() % 2 == 1 { Some(upper) } else { values.get(mid - 1).map(|lower| (lower + upper) / 2.0) }
}

/// Rebuild a blob from absolute ink coordinates. Used when a fused glyph is cut into letters.
pub(crate) fn component_from_pixels(pixels: &[(u32, u32)], page_w: u32, page_h: u32, body: f64, filter: &FilterParams) -> Option<Component> {
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0_u32, 0_u32);
    let (mut sum_x, mut sum_y) = (0_u64, 0_u64);
    for &(x, y) in pixels {
        left = left.min(x);
        top = top.min(y);
        right = right.max(x.saturating_add(1));
        bottom = bottom.max(y.saturating_add(1));
        sum_x = sum_x.saturating_add(u64::from(x));
        sum_y = sum_y.saturating_add(u64::from(y));
    }
    let area = u32::try_from(pixels.len()).ok()?;
    if area == 0 {
        return None;
    }
    let raw = Raw { left, top, right, bottom, area, sum_x, sum_y };
    let mask = mask_from_pixels(pixels, &raw, filter.max_mask_pixels);
    Some(finish(&raw, mask, page_w, page_h, body, filter))
}

fn mask_from_pixels(pixels: &[(u32, u32)], raw: &Raw, max_mask_pixels: u64) -> Option<Bitmap> {
    let build = MaskBuild::start(raw, max_mask_pixels)?;
    let mut build = build;
    for &(x, y) in pixels {
        let local_x = x.saturating_sub(build.origin_x);
        let local_y = y.saturating_sub(build.origin_y);
        let Some(local) = (local_y as usize).checked_mul(build.width as usize).and_then(|row| row.checked_add(local_x as usize)) else {
            continue;
        };
        if let Some(flag) = build.ink.get_mut(local) {
            *flag = 1;
        }
    }
    build.finish()
}

pub(crate) struct UnionFind {
    parent: Vec<u32>,
    rank: Vec<u8>,
    cap: u32,
}

impl UnionFind {
    fn new(max_components: u32) -> UnionFind {
        UnionFind { parent: vec![0], rank: vec![0], cap: max_components.saturating_add(1).max(1) }
    }

    /// Every index `0..len` its own root, for clustering a known set of items (no new
    /// labels are minted; [`UnionFind::find`] and [`UnionFind::union`] are all there is).
    pub(crate) fn with_len(len: usize) -> UnionFind {
        let len = len.min(u32::MAX as usize);
        UnionFind { parent: (0..len as u32).collect(), rank: vec![0; len], cap: len.saturating_add(1) as u32 }
    }

    fn len(&self) -> usize {
        self.parent.len()
    }

    fn new_label(&mut self) -> Option<u32> {
        let id = u32::try_from(self.parent.len()).ok()?;
        if id >= self.cap {
            return None;
        }
        self.parent.push(id);
        self.rank.push(0);
        Some(id)
    }

    pub(crate) fn find(&mut self, start: u32) -> u32 {
        let mut root = start;
        let limit = self.parent.len();
        for _ in 0..limit {
            let Some(&parent) = self.parent.get(root as usize) else {
                return 0;
            };
            if parent == root {
                break;
            }
            root = parent;
        }
        let mut current = start;
        for _ in 0..limit {
            if current == root {
                break;
            }
            let Some(slot) = self.parent.get_mut(current as usize) else {
                break;
            };
            let next = *slot;
            *slot = root;
            if next == current {
                break;
            }
            current = next;
        }
        root
    }

    pub(crate) fn union(&mut self, a: u32, b: u32) {
        let a = self.find(a);
        let b = self.find(b);
        if a == 0 || b == 0 || a == b {
            return;
        }
        let rank_a = self.rank.get(a as usize).copied().unwrap_or(0);
        let rank_b = self.rank.get(b as usize).copied().unwrap_or(0);
        let (child, parent) = if rank_a < rank_b { (a, b) } else { (b, a) };
        if let Some(slot) = self.parent.get_mut(child as usize) {
            *slot = parent;
        }
        if rank_a == rank_b
            && let Some(slot) = self.rank.get_mut(parent as usize)
        {
            *slot = slot.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(width: u32, height: u32, ink_at: impl Fn(u32, u32) -> bool) -> Bitmap {
        let mut ink = vec![0_u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                if ink_at(x, y) {
                    ink[(y * width + x) as usize] = 1;
                }
            }
        }
        Bitmap::from_ink(width, height, ink).unwrap()
    }

    fn paint(ink: &mut [u8], width: u32, x0: u32, y0: u32, w: u32, h: u32) {
        for y in y0..y0.saturating_add(h) {
            for x in x0..x0.saturating_add(w) {
                let index = (y * width + x) as usize;
                if let Some(flag) = ink.get_mut(index) {
                    *flag = 1;
                }
            }
        }
    }

    /// An H of 16×20 with a 3px stroke. Area 150, median run 3.
    fn paint_h(ink: &mut [u8], width: u32, x0: u32, y0: u32) {
        paint(ink, width, x0, y0, 3, 20);
        paint(ink, width, x0 + 13, y0, 3, 20);
        paint(ink, width, x0 + 3, y0 + 9, 10, 3);
    }

    #[test]
    fn from_ink_rejects_a_buffer_that_is_not_the_canvas() {
        assert!(Bitmap::from_ink(2, 2, vec![1, 0, 1]).is_none());
        assert!(Bitmap::from_ink(0, 0, Vec::new()).is_some());
        assert!(Bitmap::from_ink(u32::MAX, 2, Vec::new()).is_none());
        let normalized = Bitmap::from_ink(1, 2, vec![0, 7]).unwrap();
        assert_eq!(normalized.ink(), &[0, 1]);
    }

    #[test]
    fn eight_connectivity_joins_a_diagonal_that_four_connectivity_splits() {
        // The five ink pixels of an X meet only at corners.
        let image = bitmap(3, 3, |x, y| (x + y) % 2 == 0);
        let eight = analyze(&image, &AnalyzeOptions::default()).unwrap();
        assert_eq!(eight.components().len(), 1);
        assert_eq!(eight.components()[0].area, 5);
        assert_eq!(eight.components()[0].bounds, [0, 0, 3, 3]);
        assert_eq!(eight.components()[0].centroid, [1.0, 1.0]);
        let four = analyze(&image, &AnalyzeOptions { connectivity: Connectivity::Four, ..AnalyzeOptions::default() }).unwrap();
        assert_eq!(four.components().len(), 5);
        assert!(four.components().iter().all(|component| component.area == 1));
    }

    #[test]
    fn a_u_shape_merges_its_two_provisional_labels() {
        // The arms are labeled before the scan reaches the join.
        let image = bitmap(3, 3, |x, y| y == 2 || x != 1);
        let set = analyze(&image, &AnalyzeOptions { connectivity: Connectivity::Four, ..AnalyzeOptions::default() }).unwrap();
        assert_eq!(set.components().len(), 1);
        assert_eq!(set.components()[0].area, 7);
    }

    #[test]
    fn a_rectangle_keeps_its_interior_its_centroid_and_its_stroke() {
        let image = bitmap(30, 20, |x, y| (5..15).contains(&x) && (7..13).contains(&y));
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        assert_eq!(set.components().len(), 1);
        let blob = &set.components()[0];
        assert_eq!(blob.bounds, [5, 7, 15, 13]);
        assert_eq!(blob.area, 60);
        assert_eq!(blob.centroid, [9.5, 9.5]);
        assert_eq!(blob.stroke_width, 10.0);
        let mask = blob.mask.as_ref().unwrap();
        assert_eq!((mask.width(), mask.height()), (10, 6));
        assert_eq!(mask.ink_count(), 60);
        assert!(mask.ink().iter().all(|&flag| flag == 1));
    }

    #[test]
    fn an_l_has_the_center_of_mass_of_its_pixels() {
        // (0,0) (0,1) (0,2) (1,2) (2,2). Sums (3, 7) over 5 pixels.
        let image = bitmap(4, 4, |x, y| (x == 0 && y < 3) || (y == 2 && x < 3));
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        let blob = &set.components()[0];
        assert_eq!(blob.area, 5);
        assert!((blob.centroid[0] - 0.6).abs() < 1e-9, "{}", blob.centroid[0]);
        assert!((blob.centroid[1] - 1.4).abs() < 1e-9, "{}", blob.centroid[1]);
    }

    #[test]
    fn classification_separates_glyphs_speckles_rules_and_photos() {
        let (width, height) = (200_u32, 160_u32);
        let mut ink = vec![0_u8; (width * height) as usize];
        for x0 in [10_u32, 40, 70] {
            paint_h(&mut ink, width, x0, 10);
        }
        paint(&mut ink, width, 110, 22, 4, 4);
        ink[(10 * width + 180) as usize] = 1;
        paint(&mut ink, width, 190, 12, 2, 1);
        paint(&mut ink, width, 120, 36, 70, 80);
        paint(&mut ink, width, 5, 150, 190, 2);
        let image = Bitmap::from_ink(width, height, ink).unwrap();
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        assert!((set.body_size() - 20.0).abs() < 1e-6, "body {}", set.body_size());
        let kind = |wanted| set.components().iter().filter(|component| component.kind == wanted).count();
        assert_eq!(kind(ComponentKind::Glyph), 4, "three letters and a period: {:?}", kinds(&set));
        assert_eq!(kind(ComponentKind::Noise), 2, "{:?}", kinds(&set));
        assert_eq!(kind(ComponentKind::Graphic), 1, "{:?}", kinds(&set));
        assert_eq!(kind(ComponentKind::Photo), 1, "{:?}", kinds(&set));
        let glyphs: Vec<_> = set.components().iter().filter(|component| component.kind == ComponentKind::Glyph).collect();
        assert!(glyphs.iter().all(|component| component.area == 150 || component.area == 16));
        let letter = glyphs.iter().find(|component| component.area == 150).unwrap();
        assert_eq!(letter.stroke_width, 3.0);
        let mask = letter.mask.as_ref().unwrap();
        assert!(!mask.is_ink(4, 2), "the counter of the H stays paper");
        assert!(mask.is_ink(0, 0) && mask.is_ink(14, 10));
        let rule = set.components().iter().find(|component| component.kind == ComponentKind::Graphic).unwrap();
        assert!(rule.width() > 100 && rule.height() <= 2, "{rule:?}");
        let photo = set.components().iter().find(|component| component.kind == ComponentKind::Photo).unwrap();
        assert_eq!(photo.area, 70 * 80);
        // Reading order: the top row (speckles and letters) comes before the photo and the rule.
        assert_eq!(set.components().first().map(|component| component.bounds[1]), Some(10));
        assert!(set.components().last().unwrap().kind == ComponentKind::Graphic);
    }

    fn kinds(set: &ComponentSet) -> Vec<ComponentKind> {
        set.components().iter().map(|component| component.kind).collect()
    }

    #[test]
    fn mask_budget_keeps_small_boxes_until_the_total_is_spent() {
        // Index 0 is the unused label. 40 fits, 50 would pass the per-box cap but not the
        // remaining total, 200 is over the per-box cap on its own.
        let plan = mask_plan(&[0, 50, 40, 200], 100, 80);
        assert_eq!(plan, vec![false, false, true, false]);
        let both = mask_plan(&[0, 64, 64], 100, 100);
        assert_eq!(both, vec![false, true, false]);
    }

    #[test]
    fn a_mask_is_omitted_once_the_box_exceeds_the_cap_and_the_kind_stands() {
        let image = bitmap(40, 40, |x, y| (10..30).contains(&x) && (10..30).contains(&y));
        let options = AnalyzeOptions { filter: FilterParams { max_mask_pixels: 10, ..FilterParams::default() }, ..AnalyzeOptions::default() };
        let set = analyze(&image, &options).unwrap();
        assert_eq!(set.components().len(), 1);
        let blob = &set.components()[0];
        assert!(blob.mask.is_none());
        assert_eq!(blob.area, 400);
        assert_eq!(blob.kind, ComponentKind::Photo);
    }

    #[test]
    fn a_vertical_stem_is_a_glyph_and_its_stroke_is_its_thickness() {
        let image = bitmap(20, 30, |x, y| (4..7).contains(&x) && (5..25).contains(&y));
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        let blob = &set.components()[0];
        assert_eq!(blob.kind, ComponentKind::Glyph);
        assert_eq!(blob.stroke_width, 3.0);
        assert_eq!(blob.area, 60);
    }

    #[test]
    fn empty_and_blank_bitmaps_have_no_components() {
        let empty = Bitmap::from_ink(0, 0, Vec::new()).unwrap();
        assert!(analyze(&empty, &AnalyzeOptions::default()).unwrap().components().is_empty());
        let blank = Bitmap::from_ink(30, 20, vec![0; 600]).unwrap();
        let blank = analyze(&blank, &AnalyzeOptions::default()).unwrap();
        assert!(blank.components().is_empty());
        assert_eq!(blank.body_size(), DEFAULT_BODY);
    }

    #[test]
    fn too_many_blobs_is_an_error_not_a_panic() {
        // Ink on every other pixel in both axes: nothing touches, even on a diagonal.
        let image = bitmap(8, 8, |x, y| x % 2 == 0 && y % 2 == 0);
        let options = AnalyzeOptions { max_components: 4, ..AnalyzeOptions::default() };
        let message = analyze(&image, &options).unwrap_err().to_string();
        assert!(message.contains("too noisy") && message.contains("4"), "{message}");
        assert_eq!(analyze(&image, &AnalyzeOptions::default()).unwrap().components().len(), 16);
    }

    #[test]
    fn analyze_page_reads_the_binarized_scan() {
        let pixels = vec![250_u8; 80 * 48];
        let page = crate::preprocess::preprocess(&pixels, 80, 48, 1, &crate::PreprocessedOptions::default()).unwrap();
        let set = analyze_page(&page, &AnalyzeOptions::default()).unwrap();
        assert!(set.components().is_empty());
        assert_eq!(set.width(), page.binary.width());
    }
}
