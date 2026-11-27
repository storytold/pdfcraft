//! ClearScan page pipeline (stages 1–6).
//!
//! [`clearscan_image`] turns one page raster into a one-page PDF: deskew and binarize, connected
//! components, alignment with the recognised lines, centroid-aligned prototypes, Bézier outlines,
//! a Type 3 font with `/ToUnicode`, and a background the redrawn lettering has been painted out of.
//! Nothing here calls potrace, autotrace or any other copyleft tracer. The outlines are the
//! paths [`crate::vectorize`] already fitted, written as PDF operators by [`crate::synthesize_type3`].
//!
//! Lines passed to the recognizer callback are in the deskewed frame, the same frame as
//! [`PreprocessedPage`]. A simple Type 3 font holds 255 drawing glyphs; further shapes go into
//! more fonts on the same page, up to [`MAX_FONTS`].

use std::collections::BTreeMap;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};

use crate::Ocr;
use crate::OcrError;
use crate::align::{AlignParams, MatchRole, ResolvedGlyph, align};
use crate::annotations::{AnnotatedLine, is_descender_char};
use crate::background::{
    ClearScanLayer, DerenderParams, InkRegion, PageRaster, content_stream, derender_with_page_mask, embed_background, encoding_for, protected_regions,
};
use crate::cluster::{ClusterParams, cluster};
use crate::components::{AnalyzeOptions, analyze_page};
use crate::grey::MAX_PIXELS;
use crate::scan_prep::{PreprocessedOptions, PreprocessedPage, preprocess_within};
use crate::type3::{Type3Glyph, Type3Metrics, baseline_in_prototype, em_pixels, synthesize_type3};
use crate::vectorize::{GlyphOutline, VectorizeParams, vectorize};

/// Drawing glyphs in one simple Type 3 font. Matches the limit in [`crate::synthesize_type3`].
const PER_FONT: usize = 255;
/// Masters placed in `F1`, leaving one charproc code for the explicit U+0020 space glyph.
const FIRST_FONT_MASTERS: usize = PER_FONT - 1;
/// How many Type 3 fonts one page will hold. Past this, further clusters are counted and left out.
pub const MAX_FONTS: usize = 16;
/// Downsample factors the pipeline accepts. `1` keeps the raster.
pub const MAX_DOWNSAMPLE: u32 = 8;
/// A coordinate or page side past this is not a page a reader can show.
const MAX_USER: f64 = 1.0e8;

/// Where [`clearscan_image`] is in the pipeline. The engine turns these into the tool's progress log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearScanStage {
    Downsample,
    Preprocess,
    Recognize,
    Components,
    Align,
    Cluster,
    Vectorize,
    Compose,
}

impl ClearScanStage {
    pub fn as_str(self) -> &'static str {
        match self {
            ClearScanStage::Downsample => "downsample",
            ClearScanStage::Preprocess => "preprocess",
            ClearScanStage::Recognize => "recognize",
            ClearScanStage::Components => "components",
            ClearScanStage::Align => "align",
            ClearScanStage::Cluster => "cluster",
            ClearScanStage::Vectorize => "vectorize",
            ClearScanStage::Compose => "compose",
        }
    }
}

/// The knobs of [`clearscan_image`]. Defaults are the stage defaults, at the raster's own resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearScanOptions {
    pub preprocess: PreprocessedOptions,
    pub analyze: AnalyzeOptions,
    pub align: AlignParams,
    pub cluster: ClusterParams,
    pub vectorize: VectorizeParams,
    pub derender: DerenderParams,
    /// Integer box downsample applied before the other stages, `1..=`[`MAX_DOWNSAMPLE`].
    pub downsample: u32,
}

impl Default for ClearScanOptions {
    fn default() -> Self {
        ClearScanOptions {
            preprocess: PreprocessedOptions::default(),
            analyze: AnalyzeOptions::default(),
            align: AlignParams::default(),
            cluster: ClusterParams::default(),
            vectorize: VectorizeParams::default(),
            derender: DerenderParams::default(),
            downsample: 1,
        }
    }
}

impl ClearScanOptions {
    /// Sauvola's `k` for a sensitivity of 0 (darkest ink only) … 100 (faint strokes). 50 is the
    /// paper default, `k` = 0.34. Values past 100 are clamped.
    pub fn with_sensitivity(mut self, sensitivity: u8) -> Self {
        self.preprocess.sauvola.k = sauvola_k(sensitivity);
        self
    }
}

/// Sauvola's `k` for `sensitivity` (0..=100). Higher sensitivity lowers `k`, so the local
/// threshold lets more grey through as ink. 50 maps to the paper's 0.34.
pub fn sauvola_k(sensitivity: u8) -> f32 {
    let step = f32::from(sensitivity.min(100));
    (0.5 - step * 0.0032).clamp(0.05, 0.5)
}

/// A page raster for [`clearscan_image`]. `pixels` holds `width` × `height` pixels of `channels`
/// (1, 3 or 4) bytes, row-major from the top-left. `points_per_pixel` is the user-unit size of
/// one pixel, before downsampling.
#[derive(Clone, Copy)]
pub struct ScanRaster<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    pub points_per_pixel: f64,
}

/// One Type 3 glyph placed on the page. `origin` is the baseline's left end in PDF user space (y up).
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphPlacement {
    pub text: String,
    pub code: u8,
    /// 1-based index of the page's Type 3 font (`F1`, `F2`, …).
    pub font: u8,
    pub origin: [f64; 2],
    pub size: f64,
}

/// The one-page PDF and what was placed on it.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearScanOutput {
    pub pdf: Vec<u8>,
    /// Glyph texts in reading order, with no added spaces.
    pub text: String,
    /// Glyphs drawn (one per recognised occurrence).
    pub glyphs: usize,
    /// Prototypes embedded. A repeated letter is one cluster.
    pub clusters: usize,
    /// Clusters left out (no prototype, text the font will not hold, or past [`MAX_FONTS`]).
    pub skipped_clusters: usize,
    /// Pixels whose samples changed when the lettering was painted out.
    pub erased: u64,
    /// Page size in user units.
    pub width: f64,
    pub height: f64,
    pub placements: Vec<GlyphPlacement>,
}

/// Target PDF page geometry used to map ClearScan's deskewed image and glyph layer back into the
/// original page's default user space, keeping the page's boxes, `/Rotate`, and `/UserUnit`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    /// Cropped page rectangle `[x0, y0, x1, y1]` in the target page's default user space.
    pub crop: [f64; 4],
    /// Upright cropped display height in PDF points (after `/Rotate` and `/UserUnit`).
    pub display_height: f64,
    /// Affine matrix `[a, b, c, d, e, f]` mapping upright display points (origin bottom-left,
    /// y-up) to the target page's default user space.
    pub view_matrix: [f64; 6],
}

/// Run the pipeline. `recognize` reads the deskewed page; its boxes must use that frame.
pub fn clearscan_image<R, P>(raster: ScanRaster<'_>, options: &ClearScanOptions, recognize: R, progress: P) -> Result<ClearScanOutput, OcrError>
where
    R: FnMut(&PreprocessedPage) -> Result<Vec<AnnotatedLine>, OcrError>,
    P: FnMut(ClearScanStage),
{
    clearscan_inner(raster, options, None, recognize, progress)
}

/// Run the pipeline and wrap the output stream to place the deskewed page back into a target
/// PDF page whose geometry (boxes, `/Rotate`, `/UserUnit`) stays unchanged.
pub fn clearscan_image_with_geometry<R, P>(
    raster: ScanRaster<'_>,
    options: &ClearScanOptions,
    geometry: PageGeometry,
    recognize: R,
    progress: P,
) -> Result<ClearScanOutput, OcrError>
where
    R: FnMut(&PreprocessedPage) -> Result<Vec<AnnotatedLine>, OcrError>,
    P: FnMut(ClearScanStage),
{
    clearscan_inner(raster, options, Some(geometry), recognize, progress)
}

fn clearscan_inner<R, P>(
    raster: ScanRaster<'_>,
    options: &ClearScanOptions,
    geometry: Option<PageGeometry>,
    recognize: R,
    progress: P,
) -> Result<ClearScanOutput, OcrError>
where
    R: FnMut(&PreprocessedPage) -> Result<Vec<AnnotatedLine>, OcrError>,
    P: FnMut(ClearScanStage),
{
    clearscan_with_cap(raster, options, geometry, MAX_PIXELS, recognize, progress)
}

fn clearscan_with_cap<R, P>(
    raster: ScanRaster<'_>,
    options: &ClearScanOptions,
    geometry: Option<PageGeometry>,
    max_pixels: u64,
    mut recognize: R,
    mut progress: P,
) -> Result<ClearScanOutput, OcrError>
where
    R: FnMut(&PreprocessedPage) -> Result<Vec<AnnotatedLine>, OcrError>,
    P: FnMut(ClearScanStage),
{
    let (buf, src_w, src_h, source_ppp) = prepare_raster(&raster, options, &mut progress)?;
    progress(ClearScanStage::Preprocess);
    let page = preprocess_within(&buf, src_w, src_h, raster.channels, &options.preprocess, max_pixels)?;
    if !page.deskew_scale.is_finite() || page.deskew_scale <= 0.0 {
        return Err(clearscan_err("deskew scale is not valid"));
    }
    let deskew_ppp = source_ppp / page.deskew_scale;
    if !deskew_ppp.is_finite() || deskew_ppp <= 0.0 || deskew_ppp > MAX_USER {
        return Err(clearscan_err("deskewing made the page scale unusable"));
    }
    progress(ClearScanStage::Recognize);
    let mut lines = recognize(&page)?;
    progress(ClearScanStage::Components);
    let components = analyze_page(&page, &options.analyze)?;
    progress(ClearScanStage::Align);
    let mut aligned = align(&components, &lines, &options.align);
    // A split letter (the stem and the dot) would otherwise embed the character once per piece.
    assign_fragment_text(&mut aligned.matched);
    let refined_lines = refine_line_baselines(&mut lines, &aligned.matched);
    progress(ClearScanStage::Cluster);
    let clustered = cluster(&page.grey, &aligned.matched, &options.cluster);
    progress(ClearScanStage::Vectorize);
    let (masters, skipped_clusters) = masters_of(&clustered, &aligned.matched, &lines, &refined_lines, options)?;
    progress(ClearScanStage::Compose);
    let raster = deskewed_raster(&buf, src_w, src_h, raster.channels, &page)?;
    let frame = ComposeFrame { points_per_pixel: deskew_ppp, source_ppp, to_original: page.to_original, geometry, page_mask: &page.binary };
    compose(&raster, components.components(), &masters, &lines, frame, options, skipped_clusters)
}

/// [`clearscan_image`] with [`Ocr::recognize_page`] as the recognizer.
pub fn clearscan_recognized<P>(ocr: &Ocr, raster: ScanRaster<'_>, options: &ClearScanOptions, progress: P) -> Result<ClearScanOutput, OcrError>
where
    P: FnMut(ClearScanStage),
{
    clearscan_inner(raster, options, None, |page| ocr.recognize_page(page), progress)
}

/// [`clearscan_image_with_geometry`] with [`Ocr::recognize_page`] as the recognizer.
pub fn clearscan_recognized_with_geometry<P>(
    ocr: &Ocr,
    raster: ScanRaster<'_>,
    options: &ClearScanOptions,
    geometry: PageGeometry,
    progress: P,
) -> Result<ClearScanOutput, OcrError>
where
    P: FnMut(ClearScanStage),
{
    clearscan_inner(raster, options, Some(geometry), |page| ocr.recognize_page(page), progress)
}

fn clearscan_err(detail: impl Into<String>) -> OcrError {
    OcrError::ClearScan(detail.into())
}

fn prepare_raster<P>(raster: &ScanRaster<'_>, options: &ClearScanOptions, progress: &mut P) -> Result<(Vec<u8>, u32, u32, f64), OcrError>
where
    P: FnMut(ClearScanStage),
{
    let ScanRaster { pixels, width, height, channels, points_per_pixel } = *raster;
    if !matches!(channels, 1 | 3 | 4) {
        return Err(clearscan_err(format!("expected 1, 3 or 4 bytes per pixel, got {channels}")));
    }
    if width == 0 || height == 0 {
        return Err(OcrError::EmptyImage);
    }
    if !points_per_pixel.is_finite() || points_per_pixel <= 0.0 || points_per_pixel > MAX_USER {
        return Err(clearscan_err(format!("points per pixel must be a finite number in (0, {MAX_USER}], got {points_per_pixel}")));
    }
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(u64::from(channels)))
        .ok_or_else(|| clearscan_err("the image is too large"))?;
    if pixels.len() as u64 != expected {
        return Err(clearscan_err(format!("expected {expected} bytes for {width}×{height} at {channels} bytes per pixel, got {}", pixels.len())));
    }
    let pixels_n = expected / u64::from(channels);
    if pixels_n > MAX_PIXELS {
        return Err(clearscan_err(format!("the image has {pixels_n} pixels; at most {MAX_PIXELS} are read")));
    }
    let factor = options.downsample;
    if factor == 0 || factor > MAX_DOWNSAMPLE {
        return Err(clearscan_err(format!("downsample must be 1..={MAX_DOWNSAMPLE}, got {factor}")));
    }
    if factor == 1 {
        return Ok((pixels.to_vec(), width, height, points_per_pixel));
    }
    progress(ClearScanStage::Downsample);
    let (buf, new_w, new_h) = downsample(pixels, width, height, channels, factor)?;
    // Each downsampled pixel covers `factor` source pixels in both axes; avoid stretching
    // non-multiple dimensions by `width / new_w`.
    let ppp = points_per_pixel * f64::from(factor);
    if !ppp.is_finite() || ppp <= 0.0 || ppp > MAX_USER {
        return Err(clearscan_err("downsampling made the page scale unusable"));
    }
    Ok((buf, new_w, new_h, ppp))
}

fn downsample(pixels: &[u8], width: u32, height: u32, channels: u32, factor: u32) -> Result<(Vec<u8>, u32, u32), OcrError> {
    let new_w = width / factor;
    let new_h = height / factor;
    if new_w == 0 || new_h == 0 {
        return Err(clearscan_err(format!("downsampling {width}×{height} by {factor} leaves an empty image")));
    }
    let ch = usize::try_from(channels).unwrap_or(4);
    let out_len = usize::try_from(new_w)
        .ok()
        .and_then(|w| usize::try_from(new_h).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|n| n.checked_mul(ch))
        .ok_or_else(|| clearscan_err("the downsampled image is too large"))?;
    let mut out = vec![0_u8; out_len];
    let area = factor.saturating_mul(factor).max(1);
    for y in 0..new_h {
        for x in 0..new_w {
            let mut sum = [0_u32; 4];
            let x0 = x.saturating_mul(factor);
            let y0 = y.saturating_mul(factor);
            for row in 0..factor {
                let src_y = y0.saturating_add(row);
                for col in 0..factor {
                    let src_x = x0.saturating_add(col);
                    let index =
                        (u64::from(src_y).saturating_mul(u64::from(width)).saturating_add(u64::from(src_x))).saturating_mul(u64::from(channels));
                    let Some(start) = usize::try_from(index).ok() else { continue };
                    for c in 0..ch {
                        let sample = u32::from(pixels.get(start.saturating_add(c)).copied().unwrap_or(255));
                        if let Some(slot) = sum.get_mut(c) {
                            *slot = slot.saturating_add(sample);
                        }
                    }
                }
            }
            let dest = (u64::from(y).saturating_mul(u64::from(new_w)).saturating_add(u64::from(x))).saturating_mul(u64::from(channels));
            let Some(start) = usize::try_from(dest).ok() else { continue };
            for c in 0..ch {
                let average = sum.get(c).copied().unwrap_or(0) / area;
                if let Some(slot) = out.get_mut(start.saturating_add(c)) {
                    *slot = u8::try_from(average).unwrap_or(255);
                }
            }
        }
    }
    Ok((out, new_w, new_h))
}

struct Master {
    glyph: Type3Glyph,
    /// Page y of each occurrence's baseline, y down, and the occurrence's centroid.
    shows: Vec<Occurrence>,
}

struct Occurrence {
    line: Option<usize>,
    word: Option<usize>,
    y_down: f64,
    origin_x: f64,
    text: String,
    /// The piece the background loses when this occurrence is drawn.
    bounds: [u32; 4],
    mask: Option<crate::binarize::Bitmap>,
}

/// Refine each line's baseline from the actual binarized ink bottoms of non-descender
/// alphanumeric components on that line. Real `ocrs` slices the whole line box vertically, so
/// every `CharBox` on a line where every word has a descender shares the descender bottom; the
/// linked connected components still expose the true pixel baseline. Returns a per-line flag
/// indicating whether non-descender components anchored the baseline.
fn refine_line_baselines(lines: &mut [AnnotatedLine], glyphs: &[ResolvedGlyph]) -> Vec<bool> {
    let mut non_descenders: Vec<Vec<f64>> = vec![Vec::new(); lines.len()];
    let mut descenders: Vec<Vec<f64>> = vec![Vec::new(); lines.len()];
    for glyph in glyphs {
        let (line_idx, text) = match &glyph.role {
            MatchRole::Character { text, source } | MatchRole::Fragment { text, source } => (source.line, text.as_str()),
            MatchRole::Ligature { text, sources } => {
                let Some(source) = sources.first() else { continue };
                (source.line, text.as_str())
            }
            MatchRole::Unmatched => continue,
        };
        if text.is_empty() {
            continue;
        }
        let bottom = f64::from(glyph.component.bounds[3]);
        if !bottom.is_finite() {
            continue;
        }
        if text.chars().all(|c| c.is_alphanumeric() && !is_descender_char(c)) {
            if let Some(bucket) = non_descenders.get_mut(line_idx) {
                bucket.push(bottom);
            }
        } else if text.chars().any(is_descender_char)
            && let Some(bucket) = descenders.get_mut(line_idx)
        {
            bucket.push(bottom);
        }
    }
    let mut refined = vec![false; lines.len()];
    for (line_idx, line) in lines.iter_mut().enumerate() {
        let Some(mut bottoms) = non_descenders.get_mut(line_idx).map(std::mem::take) else { continue };
        let Some(true_baseline) = median(&mut bottoms) else { continue };
        let old_baseline = line.baseline;
        line.baseline = true_baseline as f32;
        let shift = old_baseline - line.baseline;
        if shift.is_finite() && shift > 0.0 {
            if line.cap_height > shift {
                line.cap_height -= shift;
            }
            if line.x_height > shift {
                line.x_height -= shift;
            }
        }
        let word_descent =
            line.words.iter().filter(|w| w.text.chars().any(is_descender_char)).map(|w| (w.rect[3] - line.baseline).max(0.0)).fold(0.0_f32, f32::max);
        let comp_descent = descenders.get(line_idx).into_iter().flatten().map(|&b| ((b - true_baseline).max(0.0)) as f32).fold(0.0_f32, f32::max);
        line.descent = word_descent.max(comp_descent);
        if let Some(slot) = refined.get_mut(line_idx) {
            *slot = true;
        }
    }
    refined
}

/// One character that labeling split into several blobs keeps its text on the largest piece.
/// The others are drawn with empty text, so search reads the letter once. Ties keep the
/// earlier glyph, which is the stable choice.
fn assign_fragment_text(glyphs: &mut [ResolvedGlyph]) {
    let mut groups: BTreeMap<(usize, usize, Option<usize>), Vec<usize>> = BTreeMap::new();
    for (index, glyph) in glyphs.iter().enumerate() {
        if let MatchRole::Fragment { source, .. } = &glyph.role {
            groups.entry((source.line, source.word, source.index)).or_default().push(index);
        }
    }
    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        let Some(primary) = members.iter().copied().max_by_key(|index| {
            let area = glyphs.get(*index).map(|glyph| glyph.component.area).unwrap_or(0);
            (area, std::cmp::Reverse(*index))
        }) else {
            continue;
        };
        for index in members {
            if *index == primary {
                continue;
            }
            if let Some(MatchRole::Fragment { text, .. }) = glyphs.get_mut(*index).map(|glyph| &mut glyph.role) {
                text.clear();
            }
        }
    }
}

fn masters_of(
    clustered: &crate::cluster::Clusters,
    glyphs: &[ResolvedGlyph],
    lines: &[AnnotatedLine],
    refined_lines: &[bool],
    options: &ClearScanOptions,
) -> Result<(Vec<Master>, usize), OcrError> {
    let mut masters = Vec::new();
    let mut skipped = 0_usize;
    for group in clustered.clusters() {
        let Some(prototype) = group.prototype() else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        let text = group.text();
        if text.chars().count() > 32 {
            skipped = skipped.saturating_add(1);
            continue;
        }
        let anchor = prototype.anchor();
        if !anchor[0].is_finite() || !anchor[1].is_finite() {
            skipped = skipped.saturating_add(1);
            continue;
        }
        let mut refined_baselines = Vec::new();
        let mut all_baselines = Vec::new();
        for &index in group.members() {
            let Some(glyph) = glyphs.get(index) else { continue };
            let page_baseline = baseline_of(glyph, lines);
            if let Some(b) = baseline_in_prototype(anchor[1], page_baseline, glyph.component.centroid[1]) {
                let is_refined = line_index(&glyph.role).and_then(|l| refined_lines.get(l).copied()).unwrap_or(false);
                if is_refined {
                    refined_baselines.push(b);
                }
                all_baselines.push(b);
            }
        }
        let Some(baseline) = median(&mut refined_baselines).or_else(|| median(&mut all_baselines)) else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        let advance = f64::from(prototype.binary().width());
        if !advance.is_finite() || advance < 0.0 {
            skipped = skipped.saturating_add(1);
            continue;
        }
        let vectorized = vectorize(prototype.binary(), &options.vectorize);
        // A truncated outline dropped contours or holes; drop the partial path so searchability
        // stays via an empty Type 3 glyph while `erase_regions` keeps the original scan ink.
        let outline = if vectorized.truncated() { GlyphOutline::default() } else { vectorized };
        let mut shows = Vec::new();
        for &index in group.members() {
            let Some(glyph) = glyphs.get(index) else { continue };
            let centroid = glyph.component.centroid;
            if !centroid[0].is_finite() || !centroid[1].is_finite() {
                continue;
            }
            let origin_x = centroid[0] - anchor[0];
            if !origin_x.is_finite() {
                continue;
            }
            let (line, word) = match word_ref(&glyph.role) {
                Some((l, w)) => (Some(l), Some(w)),
                None => (None, None),
            };
            let is_refined = line.and_then(|l| refined_lines.get(l).copied()).unwrap_or(false);
            // Refined lines snap every glyph to the line's measured baseline. On an unrefined
            // line (e.g. only descender glyphs with no baseline anchor), keep the glyph centred
            // on its scanned vertical centroid so it is never shifted down by a descender box.
            let y_down = if is_refined { baseline_of(glyph, lines) } else { centroid[1] + (baseline - anchor[1]) };
            if !y_down.is_finite() {
                continue;
            }
            shows.push(Occurrence {
                line,
                word,
                y_down,
                origin_x,
                text: text.to_string(),
                bounds: glyph.component.bounds,
                mask: glyph.component.mask.clone(),
            });
        }
        if shows.is_empty() {
            skipped = skipped.saturating_add(1);
            continue;
        }
        masters.push(Master { glyph: Type3Glyph { text: text.to_string(), outline, baseline, advance }, shows });
    }
    let cap = FIRST_FONT_MASTERS.saturating_add(PER_FONT.saturating_mul(MAX_FONTS.saturating_sub(1)));
    if masters.len() > cap {
        skipped = skipped.saturating_add(masters.len() - cap);
        masters.truncate(cap);
    }
    Ok((masters, skipped))
}

fn baseline_of(glyph: &ResolvedGlyph, lines: &[AnnotatedLine]) -> f64 {
    let Some(index) = line_index(&glyph.role) else {
        return f64::from(glyph.component.bounds[3]);
    };
    lines
        .get(index)
        .filter(|line| line.baseline.is_finite())
        .map(|line| f64::from(line.baseline))
        .unwrap_or_else(|| f64::from(glyph.component.bounds[3]))
}

fn line_index(role: &MatchRole) -> Option<usize> {
    word_ref(role).map(|(line, _)| line)
}

fn word_ref(role: &MatchRole) -> Option<(usize, usize)> {
    match role {
        MatchRole::Character { source, .. } | MatchRole::Fragment { source, .. } => Some((source.line, source.word)),
        MatchRole::Ligature { sources, .. } => sources.first().map(|s| (s.line, s.word)),
        MatchRole::Unmatched => None,
    }
}

/// Column-aware reading rank for each line in `lines` via recursive XY-cut (vertical gutter cut
/// before horizontal band cut, then baseline y and left x).
fn line_reading_ranks(lines: &[AnnotatedLine]) -> Vec<usize> {
    let rects: Vec<[f32; 4]> = lines.iter().map(AnnotatedLine::rect).collect();
    let indices: Vec<usize> = (0..lines.len()).collect();
    let mut ordered = Vec::with_capacity(lines.len());
    xy_order_lines(&indices, lines, &rects, 0, &mut ordered);
    let mut ranks = vec![0_usize; lines.len()];
    for (rank, &line_idx) in ordered.iter().enumerate() {
        if let Some(slot) = ranks.get_mut(line_idx) {
            *slot = rank;
        }
    }
    ranks
}

fn xy_order_lines(indices: &[usize], lines: &[AnnotatedLine], rects: &[[f32; 4]], depth: u32, out: &mut Vec<usize>) {
    if indices.len() <= 1 || depth >= 16 {
        let mut leaf = indices.to_vec();
        leaf.sort_by(|&a, &b| {
            let ba = lines.get(a).map_or(0.0, |l| l.baseline);
            let bb = lines.get(b).map_or(0.0, |l| l.baseline);
            let xa = rects.get(a).map_or(0.0, |r| r[0]);
            let xb = rects.get(b).map_or(0.0, |r| r[0]);
            ba.total_cmp(&bb).then_with(|| xa.total_cmp(&xb)).then_with(|| a.cmp(&b))
        });
        out.extend(leaf);
        return;
    }
    let mut by_x = indices.to_vec();
    by_x.sort_by(|&a, &b| {
        let xa = rects.get(a).map_or(0.0, |r| r[0]);
        let xb = rects.get(b).map_or(0.0, |r| r[0]);
        xa.total_cmp(&xb).then_with(|| a.cmp(&b))
    });
    let mut max_right = f32::NEG_INFINITY;
    for split in 1..by_x.len() {
        if let Some(&prev) = by_x.get(split - 1)
            && let Some(r) = rects.get(prev)
        {
            max_right = max_right.max(r[2]);
        }
        let next_left = by_x.get(split).and_then(|&idx| rects.get(idx)).map_or(f32::NEG_INFINITY, |r| r[0]);
        if max_right <= next_left {
            if let (Some(left), Some(right)) = (by_x.get(..split), by_x.get(split..)) {
                xy_order_lines(left, lines, rects, depth + 1, out);
                xy_order_lines(right, lines, rects, depth + 1, out);
            }
            return;
        }
    }
    let mut by_y = indices.to_vec();
    by_y.sort_by(|&a, &b| {
        let ya = rects.get(a).map_or(0.0, |r| r[1]);
        let yb = rects.get(b).map_or(0.0, |r| r[1]);
        ya.total_cmp(&yb).then_with(|| a.cmp(&b))
    });
    let mut max_bottom = f32::NEG_INFINITY;
    for split in 1..by_y.len() {
        if let Some(&prev) = by_y.get(split - 1)
            && let Some(r) = rects.get(prev)
        {
            max_bottom = max_bottom.max(r[3]);
        }
        let next_top = by_y.get(split).and_then(|&idx| rects.get(idx)).map_or(f32::NEG_INFINITY, |r| r[1]);
        if max_bottom <= next_top {
            if let (Some(top), Some(bottom)) = (by_y.get(..split), by_y.get(split..)) {
                xy_order_lines(top, lines, rects, depth + 1, out);
                xy_order_lines(bottom, lines, rects, depth + 1, out);
            }
            return;
        }
    }
    xy_order_lines(indices, lines, rects, 16, out);
}

fn page_metrics(lines: &[AnnotatedLine]) -> Type3Metrics {
    let mut caps = Vec::new();
    let mut xs = Vec::new();
    let mut ascents = Vec::new();
    let mut descents = Vec::new();
    for line in lines {
        push_positive(&mut caps, line.cap_height);
        push_positive(&mut xs, line.x_height);
        push_positive(&mut ascents, line.cap_height);
        if line.descent.is_finite() && line.descent >= 0.0 {
            descents.push(f64::from(line.descent));
        }
    }
    let cap = median(&mut caps).unwrap_or(1.0);
    Type3Metrics {
        ascent: median(&mut ascents).unwrap_or(cap).max(0.0),
        cap_height: cap.max(0.0),
        x_height: median(&mut xs).unwrap_or(cap * 0.5).max(0.0),
        descent: median(&mut descents).unwrap_or(0.0).max(0.0),
    }
}

fn push_positive(into: &mut Vec<f64>, value: f32) {
    if value.is_finite() && value > 0.0 {
        into.push(f64::from(value));
    }
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values.get(values.len() / 2).copied()
}

struct Placed {
    font: usize,
    code: u8,
    origin: [f64; 2],
    advance: f64,
    size: f64,
    color: [u8; 3],
    line: Option<usize>,
    word: Option<usize>,
    y_down: f64,
    text: String,
}

#[derive(Clone, Copy)]
struct SpaceGlyph {
    font: usize,
    code: u8,
    size: f64,
}

struct ComposeFrame<'a> {
    points_per_pixel: f64,
    source_ppp: f64,
    to_original: crate::deskew::Affine,
    geometry: Option<PageGeometry>,
    page_mask: &'a crate::binarize::Bitmap,
}

fn compose(
    raster: &PageRaster,
    components: &[crate::components::Component],
    masters: &[Master],
    lines: &[AnnotatedLine],
    frame: ComposeFrame<'_>,
    options: &ClearScanOptions,
    skipped_clusters: usize,
) -> Result<ClearScanOutput, OcrError> {
    let points_per_pixel = frame.points_per_pixel;
    let height_px = f64::from(raster.height());
    // Paint out only ink a placed outline replaces. An unmatched mark, a cluster with no
    // prototype, and a glyph whose origin cannot be shown stay in the scan.
    let text = erase_regions(masters, height_px, points_per_pixel);
    let protect = protected_regions(components);
    let cleaned = derender_with_page_mask(raster, &text, &protect, Some(frame.page_mask), &options.derender)?;
    let erased = cleaned.erased();
    let encoding = encoding_for(cleaned.raster());
    let metrics = page_metrics(lines);
    // The same em the Type 3 font is built with. `size / em` is one prototype pixel.
    let em = em_pixels(&metrics)?;
    let size = em * points_per_pixel;
    let mut placed = Vec::new();
    let mut doc = Document::new_empty();
    let mut font_ids = Vec::new();
    let mut space_glyph = None;
    if !masters.is_empty() {
        if !size.is_finite() || size <= 0.0 || size > MAX_USER {
            return Err(clearscan_err(format!("the font size must be a finite number in (0, {MAX_USER}], got {size}")));
        }
        let first_len = masters.len().min(FIRST_FONT_MASTERS);
        let first_chunk = masters.get(..first_len).unwrap_or(&[]);
        let rest = masters.get(first_len..).unwrap_or(&[]);
        let font_chunks = std::iter::once(first_chunk).chain(rest.chunks(PER_FONT));
        for (font_index, chunk) in font_chunks.enumerate() {
            let mut glyphs: Vec<Type3Glyph> = chunk.iter().map(|master| master.glyph.clone()).collect();
            let has_space = font_index == 0;
            if has_space {
                // Append an invisible U+0020 space charproc in F1 so the PDF content stream and
                // ToUnicode CMap carry explicit word spaces instead of relying on gap heuristics.
                glyphs.push(Type3Glyph { text: " ".into(), outline: GlyphOutline::default(), baseline: 0.0, advance: (em * 0.25).max(1.0) });
            }
            let written = synthesize_type3(&mut doc, &glyphs, &metrics)?;
            if has_space && let Some(&code) = written.codes.get(chunk.len()) {
                space_glyph = Some(SpaceGlyph { font: font_index, code, size });
            }
            for (glyph_index, master) in chunk.iter().enumerate() {
                let Some(&code) = written.codes.get(glyph_index) else { continue };
                let advance = master.glyph.advance * points_per_pixel;
                for show in &master.shows {
                    let Some(origin) = show_origin(show, height_px, points_per_pixel) else { continue };
                    let color = sample_ink_color(raster, show.bounds, show.mask.as_ref(), frame.page_mask);
                    placed.push(Placed {
                        font: font_index,
                        code,
                        origin,
                        advance,
                        size,
                        color,
                        line: show.line,
                        word: show.word,
                        y_down: show.y_down,
                        text: show.text.clone(),
                    });
                }
            }
            font_ids.push(written.id);
        }
    }
    let ranks = line_reading_ranks(lines);
    sort_placed(&mut placed, &ranks);
    let width = f64::from(cleaned.raster().width()) * points_per_pixel;
    let height = height_px * points_per_pixel;
    let placement = match frame.geometry {
        Some(geom) => Some((geom.crop, clearscan_to_user_matrix(&geom, frame.to_original, height_px, points_per_pixel, frame.source_ppp)?)),
        None => None,
    };
    let image = embed_background(&mut doc, &cleaned, encoding)?;
    let text_layer = TextLayer { fonts: &font_ids, placed: &placed, space_glyph };
    let layout = PageLayout { width, height, placement };
    let pdf = write_page(&mut doc, &image, &text_layer, layout)?;
    let text_out = joined_placed_text(&placed);
    let placements: Vec<GlyphPlacement> = placed
        .iter()
        .map(|show| GlyphPlacement {
            text: show.text.clone(),
            code: show.code,
            font: u8::try_from(show.font.saturating_add(1)).unwrap_or(u8::MAX),
            origin: show.origin,
            size: show.size,
        })
        .collect();
    let glyphs = placements.len();
    Ok(ClearScanOutput { pdf, text: text_out, glyphs, clusters: masters.len(), skipped_clusters, erased, width, height, placements })
}

fn sort_placed(placed: &mut [Placed], ranks: &[usize]) {
    placed.sort_by(|a, b| match (a.line, b.line) {
        (Some(la), Some(lb)) => {
            let ra = ranks.get(la).copied().unwrap_or(usize::MAX);
            let rb = ranks.get(lb).copied().unwrap_or(usize::MAX);
            ra.cmp(&rb)
                .then_with(|| a.word.cmp(&b.word))
                .then_with(|| a.origin[0].partial_cmp(&b.origin[0]).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.y_down.partial_cmp(&b.y_down).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a
            .y_down
            .partial_cmp(&b.y_down)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.origin[0].partial_cmp(&b.origin[0]).unwrap_or(std::cmp::Ordering::Equal)),
    });
}

/// Reconstruct plain text from sorted placements, inserting a space between words on the same
/// line and a newline between lines. Unmatched glyphs (`line: None`) never collapse into one word.
fn joined_placed_text(placed: &[Placed]) -> String {
    let mut out = String::new();
    let mut prev: Option<(Option<usize>, Option<usize>, f64)> = None;
    for show in placed {
        if show.text.is_empty() {
            continue;
        }
        if let Some((prev_line, prev_word, prev_y)) = prev {
            match (prev_line, show.line) {
                (Some(pl), Some(sl)) => {
                    if sl != pl {
                        out.push('\n');
                    } else if show.word != prev_word {
                        out.push(' ');
                    }
                }
                (Some(_), None) | (None, Some(_)) => out.push('\n'),
                (None, None) => {
                    if (show.y_down - prev_y).abs() > 2.0 {
                        out.push('\n');
                    } else {
                        out.push(' ');
                    }
                }
            }
        }
        out.push_str(&show.text);
        prev = Some((show.line, show.word, show.y_down));
    }
    out
}

/// The baseline origin in user space, when the page can show it. [`compose`] places a glyph
/// and paints out its ink only when this is `Some`, so a skipped placement stays visible.
fn show_origin(show: &Occurrence, height_px: f64, points_per_pixel: f64) -> Option<[f64; 2]> {
    let origin_x = show.origin_x * points_per_pixel;
    let origin_y = (height_px - show.y_down) * points_per_pixel;
    if origin_x.is_finite() && origin_y.is_finite() && origin_x.abs() <= MAX_USER && origin_y.abs() <= MAX_USER {
        Some([origin_x, origin_y])
    } else {
        None
    }
}

/// Ink regions of the occurrences that will be drawn. A master with no outline (including one
/// whose truncated outline was cleared) is not a replacement, so its scan ink stays.
fn erase_regions<'a>(masters: &'a [Master], height_px: f64, points_per_pixel: f64) -> Vec<InkRegion<'a>> {
    let mut regions = Vec::new();
    for master in masters {
        if master.glyph.outline.contours().is_empty() || master.glyph.outline.truncated() {
            continue;
        }
        for show in &master.shows {
            if show_origin(show, height_px, points_per_pixel).is_none() {
                continue;
            }
            regions.push(InkRegion { bounds: show.bounds, mask: show.mask.as_ref() });
        }
    }
    regions
}

fn deskewed_raster(pixels: &[u8], width: u32, height: u32, channels: u32, page: &PreprocessedPage) -> Result<PageRaster, OcrError> {
    if !matches!(channels, 1 | 3 | 4) {
        return Err(clearscan_err(format!("expected 1, 3 or 4 bytes per pixel, got {channels}")));
    }
    let dest_w = page.grey.width();
    let dest_h = page.grey.height();
    if channels == 1 {
        return PageRaster::from_pixels(dest_w, dest_h, 1, page.grey.data());
    }
    let stored = 3_u32;
    let len = u64::from(dest_w)
        .checked_mul(u64::from(dest_h))
        .and_then(|n| n.checked_mul(u64::from(stored)))
        .ok_or_else(|| clearscan_err("the deskewed image is too large"))?;
    let Some(out_len) = usize::try_from(len).ok() else {
        return Err(clearscan_err("the deskewed image is too large"));
    };
    if page.angle_radians == 0.0 && dest_w == width && dest_h == height && page.to_original == crate::deskew::Affine::IDENTITY {
        let mut data = Vec::with_capacity(out_len);
        if channels == 4 {
            for &[r, g, b, a] in pixels.as_chunks::<4>().0 {
                let fade = u8::MAX - a;
                data.push(r.saturating_add(fade));
                data.push(g.saturating_add(fade));
                data.push(b.saturating_add(fade));
            }
        } else {
            for &[r, g, b] in pixels.as_chunks::<3>().0 {
                data.extend_from_slice(&[r, g, b]);
            }
        }
        return PageRaster::from_pixels(dest_w, dest_h, stored, &data);
    }
    let mut data = vec![255_u8; out_len];
    for y in 0..dest_h {
        for x in 0..dest_w {
            let (sx, sy) = page.to_original.apply(f64::from(x), f64::from(y));
            let rgb = sample_rgb(pixels, width, height, channels, sx, sy);
            let index = u64::from(y).saturating_mul(u64::from(dest_w)).saturating_add(u64::from(x)).saturating_mul(u64::from(stored));
            let Some(start) = usize::try_from(index).ok() else { continue };
            for (offset, channel) in rgb.iter().enumerate() {
                if let Some(slot) = data.get_mut(start.saturating_add(offset)) {
                    *slot = *channel;
                }
            }
        }
    }
    PageRaster::from_pixels(dest_w, dest_h, stored, &data)
}

/// Bilinear sample. RGBA from the renderer is premultiplied, so a clear pixel becomes white.
fn sample_rgb(pixels: &[u8], width: u32, height: u32, channels: u32, x: f64, y: f64) -> [u8; 3] {
    if !x.is_finite() || !y.is_finite() || width == 0 || height == 0 {
        return [255; 3];
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let (fx, fy) = (x - x0, y - y0);
    let at = |px: f64, py: f64| pixel(pixels, width, height, channels, px, py);
    let mix = |a: [f64; 3], b: [f64; 3], t: f64| [a[0] * (1.0 - t) + b[0] * t, a[1] * (1.0 - t) + b[1] * t, a[2] * (1.0 - t) + b[2] * t];
    let top = mix(at(x0, y0), at(x0 + 1.0, y0), fx);
    let bottom = mix(at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0), fx);
    let rgb = mix(top, bottom, fy);
    [byte(rgb[0]), byte(rgb[1]), byte(rgb[2])]
}

fn pixel(pixels: &[u8], width: u32, height: u32, channels: u32, x: f64, y: f64) -> [f64; 3] {
    if x < 0.0 || y < 0.0 || x >= f64::from(width) || y >= f64::from(height) {
        return [255.0; 3];
    }
    let (x, y) = (x as u32, y as u32);
    let index = u64::from(y).saturating_mul(u64::from(width)).saturating_add(u64::from(x)).saturating_mul(u64::from(channels));
    let Some(start) = usize::try_from(index).ok() else { return [255.0; 3] };
    let channel = |offset: usize| f64::from(pixels.get(start.saturating_add(offset)).copied().unwrap_or(255));
    match channels {
        1 => {
            let g = channel(0);
            [g, g, g]
        }
        4 => {
            // Premultiplied over white: colour + (1 − alpha) × white.
            let a = channel(3);
            [channel(0) + 255.0 - a, channel(1) + 255.0 - a, channel(2) + 255.0 - a]
        }
        _ => [channel(0), channel(1), channel(2)],
    }
}

fn byte(value: f64) -> u8 {
    if value.is_finite() { value.round().clamp(0.0, 255.0) as u8 } else { 255 }
}

fn clearscan_to_user_matrix(
    geom: &PageGeometry,
    to_original: crate::deskew::Affine,
    deskewed_height_px: f64,
    deskew_ppp: f64,
    source_ppp: f64,
) -> Result<[f64; 6], OcrError> {
    let [cx0, cy0, cx1, cy1] = geom.crop;
    let cw = cx1 - cx0;
    let ch = cy1 - cy0;
    if !cx0.is_finite()
        || !cy0.is_finite()
        || !cw.is_finite()
        || !ch.is_finite()
        || cw <= 0.0
        || ch <= 0.0
        || !geom.display_height.is_finite()
        || geom.display_height <= 0.0
        || !deskewed_height_px.is_finite()
        || deskewed_height_px <= 0.0
        || !deskew_ppp.is_finite()
        || deskew_ppp <= 0.0
        || !source_ppp.is_finite()
        || source_ppp <= 0.0
        || geom.view_matrix.iter().any(|v| !v.is_finite())
    {
        return Err(clearscan_err("target page geometry is not valid"));
    }
    let scale = source_ppp / deskew_ppp;
    let ex = to_original.e + 0.5 - 0.5 * to_original.a - 0.5 * to_original.c;
    let fy = to_original.f + 0.5 - 0.5 * to_original.b - 0.5 * to_original.d;
    let m_disp = [
        to_original.a * scale,
        -to_original.b * scale,
        -to_original.c * scale,
        to_original.d * scale,
        source_ppp * (to_original.c * deskewed_height_px + ex),
        geom.display_height - source_ppp * (to_original.d * deskewed_height_px + fy),
    ];
    let v = geom.view_matrix;
    let m_user = [
        m_disp[0] * v[0] + m_disp[1] * v[2],
        m_disp[0] * v[1] + m_disp[1] * v[3],
        m_disp[2] * v[0] + m_disp[3] * v[2],
        m_disp[2] * v[1] + m_disp[3] * v[3],
        m_disp[4] * v[0] + m_disp[5] * v[2] + v[4],
        m_disp[4] * v[1] + m_disp[5] * v[3] + v[5],
    ];
    if m_user.iter().any(|x| !x.is_finite()) {
        return Err(clearscan_err("target page transform is not finite"));
    }
    Ok(m_user)
}

struct TextLayer<'a> {
    fonts: &'a [pdfcraft_cos::ObjRef],
    placed: &'a [Placed],
    space_glyph: Option<SpaceGlyph>,
}

struct PageLayout {
    width: f64,
    height: f64,
    placement: Option<([f64; 4], [f64; 6])>,
}

fn write_page(doc: &mut Document, image: &crate::background::BackgroundImage, text: &TextLayer<'_>, layout: PageLayout) -> Result<Vec<u8>, OcrError> {
    let image_only = ClearScanLayer {
        image_name: "Im0",
        image_width: layout.width,
        image_height: layout.height,
        image_origin: [0.0, 0.0],
        font_name: "F1",
        glyphs: &[],
    };
    let mut content = match layout.placement {
        Some(([cx0, cy0, cx1, cy1], [a, b, c, d, e, f])) => {
            let cw = cx1 - cx0;
            let ch = cy1 - cy0;
            format!("q\n{cx0:.6} {cy0:.6} {cw:.6} {ch:.6} re W n\n{a:.6} {b:.6} {c:.6} {d:.6} {e:.6} {f:.6} cm\n").into_bytes()
        }
        None => Vec::new(),
    };
    content.extend(content_stream(&image_only)?);
    if !text.placed.is_empty() {
        content.extend(text_operators(text.placed, text.space_glyph));
    }
    if layout.placement.is_some() {
        content.extend_from_slice(b"\nQ\n");
    }
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), &content)));
    let pages = {
        let root = doc.trailer().reference(b"Root").ok_or_else(|| clearscan_err("the new document has no catalog"))?;
        let catalog = doc.dict(&Object::Ref(root)).ok_or_else(|| clearscan_err("the new document has no catalog"))?;
        catalog.reference(b"Pages").ok_or_else(|| clearscan_err("the new document has no page tree"))?
    };
    let mut xobjects = Dict::new();
    xobjects.set(b"Im0".to_vec(), Object::Ref(image.id));
    let mut resources = Dict::new();
    resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
    if !text.fonts.is_empty() {
        let mut font_dict = Dict::new();
        for (index, id) in text.fonts.iter().enumerate() {
            let name = font_name(index)?;
            font_dict.set(name.into_bytes(), Object::Ref(*id));
        }
        resources.set(b"Font".to_vec(), Object::Dict(font_dict));
    }
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(layout.width), Object::Real(layout.height)]));
    page.set(b"Contents".to_vec(), Object::Ref(contents));
    page.set(b"Resources".to_vec(), Object::Dict(resources));
    let page = doc.add(Object::Dict(page));
    doc.update_dict(pages, |dict| {
        dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        dict.set(b"Count".to_vec(), Object::Int(1));
    })
    .map_err(|error| clearscan_err(error.to_string()))?;
    write_full(doc, &SaveOptions::default()).map_err(|error| clearscan_err(error.to_string()))
}

fn font_name(index: usize) -> Result<String, OcrError> {
    let number = index.saturating_add(1);
    if number > MAX_FONTS {
        return Err(clearscan_err(format!("a page holds at most {MAX_FONTS} Type 3 fonts")));
    }
    Ok(format!("F{number}"))
}

fn sample_ink_color(raster: &PageRaster, bounds: [u32; 4], mask: Option<&crate::binarize::Bitmap>, page_mask: &crate::binarize::Bitmap) -> [u8; 3] {
    let channels = raster.channels();
    if channels != 1 && channels != 3 {
        return [0, 0, 0];
    }
    let [left, top, right, bottom] = bounds;
    let right = right.min(raster.width());
    let bottom = bottom.min(raster.height());
    if right <= left || bottom <= top {
        return [0, 0, 0];
    }
    let matched = mask.filter(|m| m.width() == right - left && m.height() == bottom - top);
    if channels == 1 {
        let mut greys = Vec::new();
        for y in top..bottom {
            for x in left..right {
                let is_ink = match matched {
                    Some(m) => m.is_ink(i64::from(x - left), i64::from(y - top)),
                    None => page_mask.is_ink(i64::from(x), i64::from(y)),
                };
                if !is_ink {
                    continue;
                }
                if let Some(&g) = raster.pixel(x, y).and_then(|px| px.first()) {
                    greys.push(g);
                }
            }
        }
        if greys.is_empty() {
            return [0, 0, 0];
        }
        greys.sort_unstable();
        let g = greys.get(greys.len() / 2).copied().unwrap_or(0);
        return if g <= 96 { [0, 0, 0] } else { [g, g, g] };
    }
    let mut rs = Vec::new();
    let mut gs = Vec::new();
    let mut bs = Vec::new();
    for y in top..bottom {
        for x in left..right {
            let is_ink = match matched {
                Some(m) => m.is_ink(i64::from(x - left), i64::from(y - top)),
                None => page_mask.is_ink(i64::from(x), i64::from(y)),
            };
            if !is_ink {
                continue;
            }
            if let Some(&[r, g, b]) = raster.pixel(x, y).and_then(|px| px.first_chunk::<3>()) {
                rs.push(r);
                gs.push(g);
                bs.push(b);
            }
        }
    }
    if rs.is_empty() {
        return [0, 0, 0];
    }
    rs.sort_unstable();
    gs.sort_unstable();
    bs.sort_unstable();
    let mid = rs.len() / 2;
    let r = rs.get(mid).copied().unwrap_or(0);
    let g = gs.get(mid).copied().unwrap_or(0);
    let b = bs.get(mid).copied().unwrap_or(0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    // Dark neutral ink stays crisp black (`0 g`).
    if max <= 96 && max.saturating_sub(min) <= 24 { [0, 0, 0] } else { [r, g, b] }
}

/// The text block from the background writer, for a page that switches fonts. That writer
/// names one font; past 254 shapes the page uses `F1`, `F2`, and so on, so the block is
/// written here. The operators start `q 0 g BT 0 Tr`, switching fill colour when an
/// occurrence has coloured or grey ink, then `Tf`, `Tm`, and `Tj` (with explicit U+0020 space
/// glyphs emitted between distinct words on a line).
fn text_operators(placed: &[Placed], space_glyph: Option<SpaceGlyph>) -> Vec<u8> {
    let mut out = String::from("q\n0 g\nBT\n0 Tr\n");
    let mut current_font: Option<usize> = None;
    let mut current_size: Option<f64> = None;
    let mut current_color = [0_u8, 0, 0];
    let mut prev_word: Option<(usize, usize, f64, f64)> = None;
    let emit_font = |out: &mut String, font: usize, size: f64, current_font: &mut Option<usize>, current_size: &mut Option<f64>| {
        let switched = *current_font != Some(font) || current_size.is_none_or(|s| (s - size).abs() > 1.0e-4);
        if switched {
            let name = format!("F{}", font.saturating_add(1));
            out.push('/');
            out.push_str(&name);
            out.push(' ');
            out.push_str(&crate::pdf_num(size));
            out.push_str(" Tf\n");
            *current_font = Some(font);
            *current_size = Some(size);
        }
    };
    let emit_show = |out: &mut String, x: f64, y: f64, code: u8| {
        out.push_str("1 0 0 1 ");
        out.push_str(&crate::pdf_num(x));
        out.push(' ');
        out.push_str(&crate::pdf_num(y));
        out.push_str(" Tm\n<");
        out.push_str(&format!("{code:02X}"));
        out.push_str("> Tj\n");
    };
    for glyph in placed {
        if let (Some(line), Some(word)) = (glyph.line, glyph.word) {
            if let Some((pl, pw, prev_end_x, prev_y)) = prev_word
                && pl == line
                && pw != word
                && let Some(space) = space_glyph
            {
                let sx = prev_end_x.min(glyph.origin[0]);
                emit_font(&mut out, space.font, space.size, &mut current_font, &mut current_size);
                emit_show(&mut out, sx, prev_y, space.code);
            }
            prev_word = Some((line, word, glyph.origin[0] + glyph.advance, glyph.origin[1]));
        } else {
            prev_word = None;
        }
        if glyph.color != current_color {
            if glyph.color == [0, 0, 0] {
                out.push_str("0 g\n");
            } else if glyph.color[0] == glyph.color[1] && glyph.color[1] == glyph.color[2] {
                out.push_str(&crate::pdf_num(f64::from(glyph.color[0]) / 255.0));
                out.push_str(" g\n");
            } else {
                out.push_str(&crate::pdf_num(f64::from(glyph.color[0]) / 255.0));
                out.push(' ');
                out.push_str(&crate::pdf_num(f64::from(glyph.color[1]) / 255.0));
                out.push(' ');
                out.push_str(&crate::pdf_num(f64::from(glyph.color[2]) / 255.0));
                out.push_str(" rg\n");
            }
            current_color = glyph.color;
        }
        emit_font(&mut out, glyph.font, glyph.size, &mut current_font, &mut current_size);
        emit_show(&mut out, glyph.origin[0], glyph.origin[1], glyph.code);
    }
    out.push_str("ET\nQ\n");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::{AnnotatedWord, CharBox};
    use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
    use std::sync::Arc;

    fn no_progress(_: ClearScanStage) {}

    fn tuned(downsample: u32, max_skew_degrees: f64) -> ClearScanOptions {
        let base = ClearScanOptions::default().with_sensitivity(50);
        ClearScanOptions { downsample, preprocess: crate::scan_prep::PreprocessedOptions { max_skew_degrees, ..base.preprocess }, ..base }
    }

    fn lines_for_left_ink(page: &PreprocessedPage) -> Result<Vec<AnnotatedLine>, OcrError> {
        let binary = &page.binary;
        let mid = binary.width() / 2;
        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0_u32, 0_u32);
        let mut any = false;
        for y in 0..binary.height() {
            for x in 0..mid {
                if binary.is_ink(i64::from(x), i64::from(y)) {
                    any = true;
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x.saturating_add(1));
                    bottom = bottom.max(y.saturating_add(1));
                }
            }
        }
        if !any {
            return Ok(Vec::new());
        }
        let rect = [left as f32, top as f32, right as f32, bottom as f32];
        let height = rect[3] - rect[1];
        Ok(vec![AnnotatedLine {
            words: vec![AnnotatedWord { text: "H".into(), rect, chars: vec![CharBox { char: 'H', rect }] }],
            baseline: rect[3],
            x_height: height * 0.5,
            cap_height: height,
            descent: 0.0,
        }])
    }

    /// A stem on the left, a grey fringe, and a filled photograph on the right. Paper carries a
    /// little grain so the raw scan does not compress to nothing.
    fn synthetic_scan() -> (Vec<u8>, u32, u32) {
        let (width, height) = (180_u32, 140_u32);
        let mut data = vec![250_u8; (width * height) as usize];
        let put = |data: &mut [u8], x: u32, y: u32, value: u8| {
            let index = y as usize * width as usize + x as usize;
            if let Some(slot) = data.get_mut(index) {
                *slot = value;
            }
        };
        for y in 40..68 {
            for x in 30..34 {
                put(&mut data, x, y, 0);
            }
        }
        for y in 39..69 {
            for x in 29..35 {
                let index = y as usize * width as usize + x as usize;
                if data.get(index).copied() == Some(250) {
                    put(&mut data, x, y, 180);
                }
            }
        }
        for y in 20..70 {
            for x in 120..170 {
                put(&mut data, x, y, 20);
            }
        }
        let mut noise = 0x1234_5678_u32;
        for y in 0..height {
            for x in 0..width {
                let index = y as usize * width as usize + x as usize;
                if data.get(index).copied() == Some(250) {
                    noise = noise.wrapping_mul(1664525).wrapping_add(1013904223);
                    put(&mut data, x, y, 247 + ((noise >> 24) % 7) as u8);
                }
            }
        }
        (data, width, height)
    }

    #[test]
    fn sensitivity_50_is_the_paper_default_and_higher_keeps_fainter_ink() {
        assert!((sauvola_k(50) - 0.34).abs() < 1.0e-5, "{}", sauvola_k(50));
        assert!(sauvola_k(0) > sauvola_k(50));
        assert!(sauvola_k(50) > sauvola_k(100));
        assert!((sauvola_k(200) - sauvola_k(100)).abs() < 1.0e-6);
    }

    #[test]
    fn hostile_rasters_are_errors() {
        let options = ClearScanOptions::default();
        let refuse = |pixels: &[u8], w, h, ch, ppp, options: &ClearScanOptions| {
            clearscan_image(ScanRaster { pixels, width: w, height: h, channels: ch, points_per_pixel: ppp }, options, |_| Ok(Vec::new()), no_progress)
                .unwrap_err()
        };
        assert!(matches!(refuse(&[], 0, 0, 1, 1.0, &options), OcrError::EmptyImage));
        let mismatch = refuse(&[0, 0], 2, 2, 1, 1.0, &options);
        assert!(mismatch.to_string().contains("expected"), "{mismatch}");
        let scale = refuse(&[255; 16], 4, 4, 1, f64::NAN, &options);
        assert!(scale.to_string().contains("points per pixel"), "{scale}");
        let mut bad = options.clone();
        bad.downsample = 9;
        let factor = refuse(&[255; 16], 4, 4, 1, 1.0, &bad);
        assert!(factor.to_string().contains("downsample"), "{factor}");
        bad.downsample = 8;
        let empty = refuse(&[255; 4], 2, 2, 1, 1.0, &bad);
        assert!(empty.to_string().contains("empty"), "{empty}");
        let huge = refuse(&[0], u32::MAX, u32::MAX, 1, 1.0, &options);
        assert!(huge.to_string().contains("too large") || huge.to_string().contains("expected"), "{huge}");
        assert!(refuse(&[0, 0, 0], 1, 1, 2, 1.0, &options).to_string().contains("bytes per pixel"));
    }

    #[test]
    fn a_synthetic_scan_becomes_searchable_vectors_smaller_than_the_bitmap() {
        let (pixels, width, height) = synthetic_scan();
        // A degree is enough to exercise deskew without swinging the photograph across the stem.
        let options = tuned(1, 1.0);
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &options,
            lines_for_left_ink,
            no_progress,
        )
        .expect("clearscan");
        assert!(out.pdf.len() < pixels.len() / 2, "pdf {} bytes, raw {}", out.pdf.len(), pixels.len());
        assert!(out.text.contains('H'), "text {:?}", out.text);
        assert_eq!(out.glyphs, 1, "one stem, {:?}", out.placements);
        assert!(out.erased > 0, "the stem was painted out of the scan");
        assert!(out.width > 0.0 && out.height > 0.0);

        let mut renderer = PageRenderer::new(Arc::new(out.pdf.clone()), RenderConfig::default());
        let text = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..RenderRequest::default() });
        assert!(text.error.is_none(), "{:?}", text.error);
        let found: String = text.text.iter().flat_map(|page| page.glyphs.iter().map(|glyph| glyph.text.as_str())).collect();
        assert!(found.contains('H'), "extracted {found:?}");
        let pixels_page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..RenderRequest::default() });
        assert!(pixels_page.error.is_none(), "{:?}", pixels_page.error);
        let dark = pixels_page.rgba.chunks(4).filter(|px| px.first().copied().unwrap_or(255) < 80).count();
        // The photograph is 50×50. A page that kept only the stem would be a few hundred dark pixels.
        assert!(dark > 800, "the photograph should stay, dark pixels {dark}");

        let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
        let path = std::env::temp_dir().join(format!("pdfcraft-clearscan-{}.pdf", std::process::id()));
        std::fs::write(&path, &out.pdf).expect("write");
        let checked = qpdf.command().arg("--check").arg(&path).output().expect("qpdf");
        let _ = std::fs::remove_file(&path);
        assert!(checked.status.success(), "qpdf --check\n{}", String::from_utf8_lossy(&checked.stderr));
    }

    #[test]
    fn a_stem_and_a_dot_are_searchable_once() {
        let (width, height) = (160_u32, 80_u32);
        // The dot sits clear of the stem, so labeling keeps two blobs inside one character box.
        let pixels = marked_page(width, height, &[[40, 12, 43, 15], [40, 24, 43, 44]]);
        let out =
            clearscan_image(ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 }, &tuned(1, 1.0), lines_i, no_progress)
                .expect("dotted i");
        assert_eq!(out.text, "i", "placements {:?}", out.placements);
        assert_eq!(out.glyphs, 2, "the dot is drawn, without its own letter: {:?}", out.placements);
        assert_eq!(out.placements.iter().filter(|show| show.text == "i").count(), 1);

        let mut renderer = PageRenderer::new(Arc::new(out.pdf.clone()), RenderConfig::default());
        let text = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..RenderRequest::default() });
        assert!(text.error.is_none(), "{:?}", text.error);
        let found: String = text.text.iter().flat_map(|page| page.glyphs.iter().map(|glyph| glyph.text.as_str())).collect();
        assert_eq!(found, "i", "extracted {found:?}");
    }

    #[test]
    fn an_unclaimed_mark_stays_in_the_scan() {
        let (width, height) = (180_u32, 140_u32);
        // The recognizer claims the left stem only. The stem on the right is the same kind of
        // glyph, and no character box owns it.
        let pixels = marked_page(width, height, &[[30, 40, 34, 68], [140, 40, 144, 68]]);
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &tuned(1, 1.0),
            lines_for_left_ink,
            no_progress,
        )
        .expect("unclaimed mark");
        assert!(out.text.contains('H'), "{:?}", out.text);
        let mut renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..RenderRequest::default() });
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        let page_width = rendered.width.max(1);
        let dark_right = rendered
            .rgba
            .chunks(4)
            .enumerate()
            .filter(|(index, pixel)| {
                let x = (*index as u32) % page_width;
                x >= page_width.saturating_mul(2) / 3 && pixel.first().copied().unwrap_or(255) < 80
            })
            .count();
        assert!(dark_right > 40, "the unclaimed block should stay, dark pixels on the right {dark_right}");
    }

    /// Black rectangles on a lightly grained page. Each mark is `[left, top, right, bottom)`.
    fn marked_page(width: u32, height: u32, marks: &[[u32; 4]]) -> Vec<u8> {
        let len = (width as usize).saturating_mul(height as usize);
        let mut data = vec![250_u8; len];
        let put = |data: &mut [u8], x: u32, y: u32, value: u8| {
            let index = y as usize * width as usize + x as usize;
            if let Some(slot) = data.get_mut(index) {
                *slot = value;
            }
        };
        for mark in marks {
            let [left, top, right, bottom] = *mark;
            for y in top..bottom {
                for x in left..right {
                    put(&mut data, x, y, 0);
                }
            }
        }
        let mut noise = 0x1234_5678_u32;
        for y in 0..height {
            for x in 0..width {
                let index = y as usize * width as usize + x as usize;
                if data.get(index).copied() == Some(250) {
                    noise = noise.wrapping_mul(1664525).wrapping_add(1013904223);
                    put(&mut data, x, y, 247 + ((noise >> 24) % 7) as u8);
                }
            }
        }
        data
    }

    fn lines_i(page: &PreprocessedPage) -> Result<Vec<AnnotatedLine>, OcrError> {
        let binary = &page.binary;
        let mid = binary.width() / 2;
        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0_u32, 0_u32);
        let mut any = false;
        for y in 0..binary.height() {
            for x in 0..mid {
                if binary.is_ink(i64::from(x), i64::from(y)) {
                    any = true;
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x.saturating_add(1));
                    bottom = bottom.max(y.saturating_add(1));
                }
            }
        }
        if !any {
            return Ok(Vec::new());
        }
        let rect = [left as f32, top as f32, right as f32, bottom as f32];
        let height = rect[3] - rect[1];
        Ok(vec![AnnotatedLine {
            words: vec![AnnotatedWord { text: "i".into(), rect, chars: vec![CharBox { char: 'i', rect }] }],
            baseline: rect[3],
            x_height: height * 0.5,
            cap_height: height,
            descent: 0.0,
        }])
    }

    #[test]
    fn downsample_keeps_the_page_size_and_the_letter() {
        let (pixels, width, height) = synthetic_scan();
        let options = tuned(2, 1.0);
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &options,
            lines_for_left_ink,
            no_progress,
        )
        .expect("downsampled");
        // Deskew adds a sampling margin (when tilted). At downsample 2 each of those pixels is 2 user units.
        assert!((out.width - f64::from(width)).abs() < 16.0, "width {}", out.width);
        assert!(out.text.contains('H'), "{:?}", out.text);
    }

    #[test]
    fn capped_rectification_carries_scale_through_compose() {
        let (width, height) = (320_u32, 200_u32);
        let mut pixels = vec![248_u8; (width * height) as usize];
        // Draw tilted word-like bars at ~5° so estimate_skew finds >= 0.1° and rotation expansion
        // exceeds the 64,000 px test cap (= 320 * 200).
        let angle = 5.0_f64.to_radians();
        let (sin, cos) = angle.sin_cos();
        let mut put_block = |x0: i64, y0: i64, bw: i64, bh: i64| {
            for dy in 0..bh {
                for dx in 0..bw {
                    let rx = (f64::from(x0 as i32 + dx as i32) - 160.0) * cos - (f64::from(y0 as i32 + dy as i32) - 100.0) * sin + 160.0;
                    let ry = (f64::from(x0 as i32 + dx as i32) - 160.0) * sin + (f64::from(y0 as i32 + dy as i32) - 100.0) * cos + 100.0;
                    let ix = rx.round() as i64;
                    let iy = ry.round() as i64;
                    if ix >= 0 && iy >= 0 && ix < i64::from(width) && iy < i64::from(height) {
                        pixels[iy as usize * width as usize + ix as usize] = 10;
                    }
                }
            }
        };
        for r in 0..5_i64 {
            for c in 0..6_i64 {
                if (r, c) == (0, 0) {
                    // Draw a single connected 'H' on the top-left side.
                    put_block(30, 30, 4, 18);
                    put_block(44, 30, 4, 18);
                    put_block(30, 37, 18, 4);
                } else {
                    put_block(30 + c * 46, 30 + r * 30, 26, 8);
                }
            }
        }
        let geom = PageGeometry {
            crop: [0.0, 0.0, f64::from(width), f64::from(height)],
            display_height: f64::from(height),
            view_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        };
        let options = tuned(1, 10.0);
        let cap = u64::from(width) * u64::from(height);
        let out = clearscan_with_cap(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &options,
            Some(geom),
            cap,
            |page| {
                assert!(page.deskew_scale < 1.0, "expected capped scale < 1.0, got {}", page.deskew_scale);
                lines_for_left_ink(page)
            },
            no_progress,
        )
        .expect("capped rectification should succeed without exceeding pixel cap");
        assert!(out.width >= f64::from(width) && out.height >= f64::from(height), "out {}x{}", out.width, out.height);
        assert!(out.text.contains('H'), "{:?}", out.text);
        let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(out.pdf)).expect("valid pdf");
        assert!(doc.trailer().reference(b"Root").is_some());
    }

    #[test]
    fn truncated_outline_keeps_scan_ink_and_searchability() {
        let (width, height) = (180_u32, 140_u32);
        let mut pixels = vec![250_u8; (width * height) as usize];
        // A hollow ring has two contours (outer + hole); max_contours = 1 truncates the hole.
        for y in 40..68_usize {
            for x in 30..54_usize {
                if !(35..49).contains(&x) || !(45..63).contains(&y) {
                    pixels[y * width as usize + x] = 0;
                }
            }
        }
        let mut options = tuned(1, 1.0);
        options.vectorize.max_contours = 1;
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &options,
            lines_for_left_ink,
            no_progress,
        )
        .expect("truncated outline should succeed");
        assert_eq!(out.erased, 0, "truncated outline must not erase original scan ink");
        assert_eq!(out.text, "H", "searchable text must remain present");

        let mut renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let text = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..RenderRequest::default() });
        assert!(text.error.is_none(), "{:?}", text.error);
        let found: String = text.text.iter().flat_map(|page| page.glyphs.iter().map(|glyph| glyph.text.as_str())).collect();
        assert!(found.contains('H'), "extracted {found:?}");

        let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..RenderRequest::default() });
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        let sample = |x: u32, y: u32| -> u8 {
            let idx = (y as usize * rendered.width as usize + x as usize) * 4;
            rendered.rgba[idx]
        };
        assert!(sample(42, 42) < 80, "stroke ink must stay visible, got {}", sample(42, 42));
        assert!(sample(42, 54) > 200, "truncated outer must not fill the hole, got {}", sample(42, 54));
    }

    #[test]
    fn cluster_baseline_uses_median_across_members_not_just_first() {
        let (width, height) = (180_u32, 160_u32);
        // Three identical stems ('I') at y = 20..40, 60..80, 100..120.
        let pixels = marked_page(width, height, &[[30, 20, 36, 40], [30, 60, 36, 80], [30, 100, 36, 120]]);
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &tuned(1, 1.0),
            |_| {
                let line = |top: f32, bottom: f32, baseline: f32| AnnotatedLine {
                    words: vec![AnnotatedWord {
                        text: "I".into(),
                        rect: [30.0, top, 36.0, bottom],
                        chars: vec![CharBox { char: 'I', rect: [30.0, top, 36.0, bottom] }],
                    }],
                    baseline,
                    x_height: 10.0,
                    cap_height: 20.0,
                    descent: 0.0,
                };
                // First line has a descender-skewed baseline (+10 px below the stem bottom);
                // lines 2 and 3 sit on the true baseline (bottom).
                Ok(vec![line(20.0, 40.0, 50.0), line(60.0, 80.0, 80.0), line(100.0, 120.0, 120.0)])
            },
            no_progress,
        )
        .expect("median cluster baseline");
        assert_eq!(out.clusters, 1, "all three stems cluster into one master");

        let mut renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..RenderRequest::default() });
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        let sample = |x: u32, y: u32| -> u8 {
            let idx = (y as usize * rendered.width as usize + x as usize) * 4;
            rendered.rgba[idx]
        };
        // With the median baseline across members, lines 2 and 3 draw at their true vertical
        // positions (centres at y = 70 and y = 110), rather than being shifted up by 10 px.
        assert!(sample(33, 75) < 80, "second stem lower half must stay dark at y=75, got {}", sample(33, 75));
        assert!(sample(33, 115) < 80, "third stem lower half must stay dark at y=115, got {}", sample(33, 115));
    }

    #[test]
    fn two_columns_with_near_equal_baselines_do_not_interleave() {
        let (width, height) = (220_u32, 140_u32);
        // Left column: [20..26, 20..40] ('A') and [20..26, 70..90] ('B').
        // Right column: [140..146, 21..41] ('C') and [140..146, 69..89] ('D').
        let pixels = marked_page(width, height, &[[20, 20, 26, 40], [20, 70, 26, 90], [140, 21, 146, 41], [140, 69, 146, 89]]);
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &tuned(1, 1.0),
            |_| {
                let mk = |ch: char, l: f32, t: f32, r: f32, b: f32| AnnotatedLine {
                    words: vec![AnnotatedWord { text: ch.to_string(), rect: [l, t, r, b], chars: vec![CharBox { char: ch, rect: [l, t, r, b] }] }],
                    baseline: b,
                    x_height: (b - t) * 0.5,
                    cap_height: b - t,
                    descent: 0.0,
                };
                // Recognizer returns lines interleaved by y across the two columns.
                Ok(vec![
                    mk('A', 20.0, 20.0, 26.0, 40.0),
                    mk('C', 140.0, 21.0, 146.0, 41.0),
                    mk('D', 140.0, 69.0, 146.0, 89.0),
                    mk('B', 20.0, 70.0, 26.0, 90.0),
                ])
            },
            no_progress,
        )
        .expect("two columns");
        let order: String = out.placements.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(order, "ABCD", "left column must read before right column without interleaving, got {order:?}");
    }

    #[test]
    fn coloured_lettering_keeps_its_ink_colour_when_redrawn() {
        let (width, height) = (180_u32, 140_u32);
        let mut pixels = vec![250_u8; (width * height * 3) as usize];
        // Draw a red stem [30..36, 40..68] on the left so lines_for_left_ink recognizes 'H'.
        for y in 40..68_usize {
            for x in 30..36_usize {
                let idx = (y * width as usize + x) * 3;
                pixels[idx..idx + 3].copy_from_slice(&[200, 20, 20]);
            }
        }
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 3, points_per_pixel: 1.0 },
            &tuned(1, 1.0),
            lines_for_left_ink,
            no_progress,
        )
        .expect("coloured lettering");
        assert_eq!(out.text, "H");
        let mut renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..RenderRequest::default() });
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        let idx = (54_usize * rendered.width as usize + 33) * 4;
        let [r, g, b, _] = rendered.rgba[idx..idx + 4] else { panic!("pixel") };
        assert!(r > 150 && g < 60 && b < 60, "expected red glyph fill, got rgb=({r}, {g}, {b})");
    }

    #[test]
    fn word_spaces_and_line_breaks_appear_in_output_text_and_extracted_page_text() {
        let (width, height) = (200_u32, 140_u32);
        // Line 0: word 0 ("HI" at x=20..24, 28..32) and word 1 ("TO" at x=70..74, 78..82), y=20..40.
        // Line 1: word 0 ("GO" at x=20..24, 28..32), y=80..100.
        let pixels = marked_page(
            width,
            height,
            &[[20, 20, 24, 40], [28, 20, 32, 40], [70, 20, 74, 40], [78, 20, 82, 40], [20, 80, 24, 100], [28, 80, 32, 100]],
        );
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &tuned(1, 1.0),
            |_| {
                let word = |w: &str, c0: (char, f32, f32), c1: (char, f32, f32), t: f32, b: f32| AnnotatedWord {
                    text: w.into(),
                    rect: [c0.1, t, c1.2, b],
                    chars: vec![CharBox { char: c0.0, rect: [c0.1, t, c0.2, b] }, CharBox { char: c1.0, rect: [c1.1, t, c1.2, b] }],
                };
                Ok(vec![
                    AnnotatedLine {
                        words: vec![
                            word("HI", ('H', 20.0, 24.0), ('I', 28.0, 32.0), 20.0, 40.0),
                            word("TO", ('T', 70.0, 74.0), ('O', 78.0, 82.0), 20.0, 40.0),
                        ],
                        baseline: 40.0,
                        x_height: 10.0,
                        cap_height: 20.0,
                        descent: 0.0,
                    },
                    AnnotatedLine {
                        words: vec![word("GO", ('G', 20.0, 24.0), ('O', 28.0, 32.0), 80.0, 100.0)],
                        baseline: 100.0,
                        x_height: 10.0,
                        cap_height: 20.0,
                        descent: 0.0,
                    },
                ])
            },
            no_progress,
        )
        .expect("word spaces and line breaks");
        assert_eq!(out.text, "HI TO\nGO");

        let mut renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let text = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..RenderRequest::default() });
        assert!(text.error.is_none(), "{:?}", text.error);
        let plain = text.text.as_ref().map(|page| page.plain_text()).unwrap_or_default();
        assert_eq!(plain, "HI TO\nGO");
    }

    #[test]
    fn pdf_text_stream_and_tounicode_contain_explicit_u0020_spaces() {
        let (width, height) = (180_u32, 120_u32);
        // Tightly spaced words ("HI" at x=20..24, 26..30 and "TO" at x=32..36, 38..42 — only a 2 px gap).
        let pixels = marked_page(width, height, &[[20, 20, 24, 40], [26, 20, 30, 40], [32, 20, 36, 40], [38, 20, 42, 40]]);
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &tuned(1, 1.0),
            |_| {
                let word = |w: &str, c0: (char, f32, f32), c1: (char, f32, f32)| AnnotatedWord {
                    text: w.into(),
                    rect: [c0.1, 20.0, c1.2, 40.0],
                    chars: vec![CharBox { char: c0.0, rect: [c0.1, 20.0, c0.2, 40.0] }, CharBox { char: c1.0, rect: [c1.1, 20.0, c1.2, 40.0] }],
                };
                Ok(vec![AnnotatedLine {
                    words: vec![word("HI", ('H', 20.0, 24.0), ('I', 26.0, 30.0)), word("TO", ('T', 32.0, 36.0), ('O', 38.0, 42.0))],
                    baseline: 40.0,
                    x_height: 10.0,
                    cap_height: 20.0,
                    descent: 0.0,
                }])
            },
            no_progress,
        )
        .expect("explicit pdf space");

        // Verify F1's /ToUnicode CMap maps a glyph code to <0020> and the page stream emits it.
        let cos = Document::open(Arc::new(out.pdf.clone())).expect("open pdf");
        let root = cos.trailer().reference(b"Root").expect("root");
        let pages_ref = cos.dict(&Object::Ref(root)).expect("catalog").reference(b"Pages").expect("pages");
        let pages_dict = cos.dict(&Object::Ref(pages_ref)).expect("pages dict");
        let kids = pages_dict.get(b"Kids").expect("kids");
        let first_page_ref = cos.resolve(kids).as_array().expect("kids array").first().and_then(Object::as_ref).expect("first page ref");
        let page = cos.dict(&Object::Ref(first_page_ref)).expect("page dict");
        let res = cos.dict(page.get(b"Resources").expect("resources")).expect("res dict");
        let fonts = cos.dict(res.get(b"Font").expect("font dict")).expect("fonts");
        let f1 = cos.dict(fonts.get(b"F1").expect("F1")).expect("F1 dict");
        let to_unicode = f1.get(b"ToUnicode").expect("ToUnicode");
        let cmap_stream = match cos.resolve(to_unicode).as_ref() {
            Object::Stream(s) => String::from_utf8_lossy(&s.decoded().expect("decode cmap")).into_owned(),
            other => panic!("expected ToUnicode stream, got {other:?}"),
        };
        let space_line = cmap_stream.lines().find(|l| l.contains("<0020>")).expect("ToUnicode CMap must contain <0020>");
        let space_hex = space_line.split_whitespace().next().expect("space code token");
        let contents_stream = match cos.resolve(page.get(b"Contents").expect("Contents")).as_ref() {
            Object::Stream(s) => String::from_utf8_lossy(&s.decoded().expect("decode contents")).into_owned(),
            other => panic!("expected Contents stream, got {other:?}"),
        };
        assert!(contents_stream.contains(&format!("{space_hex} Tj")), "content stream missing explicit space {space_hex} Tj:\n{contents_stream}");

        let mut renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let text = renderer.render(RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, ..RenderRequest::default() });
        assert!(text.error.is_none(), "{:?}", text.error);
        let plain = text.text.as_ref().map(|page| page.plain_text()).unwrap_or_default();
        assert_eq!(plain, "HI TO", "tightly spaced words must extract with a single explicit space");
    }

    #[test]
    fn grey_lettering_on_1ch_scan_keeps_its_grey_tone() {
        let (width, height) = (180_u32, 140_u32);
        let mut pixels = vec![250_u8; (width * height) as usize];
        // Draw a medium-grey stem [30..36, 40..68] (luma 140 on 250 paper).
        for y in 40..68_usize {
            for x in 30..36_usize {
                pixels[y * width as usize + x] = 140;
            }
        }
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &tuned(1, 1.0),
            lines_for_left_ink,
            no_progress,
        )
        .expect("grey 1ch lettering");
        assert_eq!(out.text, "H");
        let mut renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..RenderRequest::default() });
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        let idx = (54_usize * rendered.width as usize + 33) * 4;
        let g = rendered.rgba[idx];
        assert!((115..=165).contains(&g), "expected medium grey (~140) glyph fill, got {g}");
    }

    #[test]
    fn descender_line_with_whole_line_char_boxes_keeps_true_baseline_on_both_lines() {
        let (width, height) = (220_u32, 160_u32);
        // Line 0 ("gap"): 'g' [20..26, 25..55] (descender), 'a' [34..40, 25..40], 'p' [48..54, 25..55] (descender).
        // Line 1 ("cat"): 'c' [20..26, 85..100], 'a' [34..40, 85..100], 't' [48..54, 80..100].
        // Real ocrs slices the whole line box vertically, so every CharBox on line 0 gets bottom = 55!
        let pixels = marked_page(
            width,
            height,
            &[[20, 25, 26, 55], [34, 25, 40, 40], [48, 25, 54, 55], [20, 85, 26, 100], [34, 85, 40, 100], [48, 80, 54, 100]],
        );
        let out = clearscan_image(
            ScanRaster { pixels: &pixels, width, height, channels: 1, points_per_pixel: 1.0 },
            &tuned(1, 1.0),
            |_| {
                let line0 = AnnotatedLine::measure(vec![AnnotatedWord {
                    text: "gap".into(),
                    rect: [20.0, 25.0, 54.0, 55.0],
                    chars: vec![
                        CharBox { char: 'g', rect: [20.0, 25.0, 26.0, 55.0] },
                        CharBox { char: 'a', rect: [34.0, 25.0, 40.0, 55.0] },
                        CharBox { char: 'p', rect: [48.0, 25.0, 54.0, 55.0] },
                    ],
                }]);
                // Without component-based baseline refinement, line0.baseline is 55.0 (descender bottom).
                assert_eq!(line0.baseline, 55.0);
                let line1 = AnnotatedLine::measure(vec![AnnotatedWord {
                    text: "cat".into(),
                    rect: [20.0, 80.0, 54.0, 100.0],
                    chars: vec![
                        CharBox { char: 'c', rect: [20.0, 80.0, 26.0, 100.0] },
                        CharBox { char: 'a', rect: [34.0, 80.0, 40.0, 100.0] },
                        CharBox { char: 't', rect: [48.0, 80.0, 54.0, 100.0] },
                    ],
                }]);
                Ok(vec![line0, line1])
            },
            no_progress,
        )
        .expect("descender line baseline refinement");

        let mut renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let rendered = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 1.0, ..RenderRequest::default() });
        assert!(rendered.error.is_none(), "{:?}", rendered.error);
        let sample = |x: u32, y: u32| -> u8 {
            let idx = (y as usize * rendered.width as usize + x as usize) * 4;
            rendered.rgba[idx]
        };
        // Line 0 ('a' at y=25..40): must stay dark inside 25..40 and white at y=48 (NOT shifted down by 15 px).
        assert!(sample(37, 32) < 80, "line 0 'a' must remain at y=32, got {}", sample(37, 32));
        assert!(sample(37, 48) > 200, "line 0 'a' must not drop to y=48, got {}", sample(37, 48));
        // Line 0 ('g' at y=25..55): both top and descender tail must stay dark.
        assert!(sample(23, 32) < 80, "line 0 'g' body must stay dark at y=32, got {}", sample(23, 32));
        assert!(sample(23, 50) < 80, "line 0 'g' tail must stay dark at y=50, got {}", sample(23, 50));
        // Line 1 ('a' at y=85..100): must stay dark at y=92 and white at y=108.
        assert!(sample(37, 92) < 80, "line 1 'a' must remain at y=92, got {}", sample(37, 92));
        assert!(sample(37, 108) > 200, "line 1 'a' must not drop to y=108, got {}", sample(37, 108));
    }

    #[test]
    fn real_ocrs_descender_line_keeps_true_baseline() {
        let Ok(ocr) = Ocr::find() else { return };
        // Build a simple Helvetica PDF with line 0 ("gap dog peg") where every word has a descender,
        // and line 1 ("cat hat mat") with no descenders.
        let mut doc = Document::new_empty();
        let root = doc.trailer().reference(b"Root").expect("root");
        let pages = doc.dict(&Object::Ref(root)).expect("cat").reference(b"Pages").expect("pages");
        let font = doc.add(Object::Dict({
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("Font"));
            d.set(b"Subtype".to_vec(), Object::name("Type1"));
            d.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
            d
        }));
        let mut font_res = Dict::new();
        font_res.set(b"F1".to_vec(), Object::Ref(font));
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(font_res));
        let stream = doc.add(Object::Stream(Stream::flate(
            Dict::new(),
            b"1 1 1 rg 0 0 320 120 re f 0 0 0 rg BT /F1 18 Tf 24 84 Td (gap dog peg) Tj 0 -36 Td (cat hat mat) Tj ET\n",
        )));
        let mut page = Dict::new();
        page.set(b"Type".to_vec(), Object::name("Page"));
        page.set(b"Parent".to_vec(), Object::Ref(pages));
        page.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(320), Object::Int(120)]));
        page.set(b"Resources".to_vec(), Object::Dict(res));
        page.set(b"Contents".to_vec(), Object::Ref(stream));
        let page_ref = doc.add(Object::Dict(page));
        doc.update_dict(pages, |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .expect("set pages");
        let src_pdf = Arc::new(write_full(&doc, &SaveOptions::default()).expect("write src"));
        let mut src_renderer = PageRenderer::new(src_pdf, RenderConfig::default());
        let before = src_renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 3.0, ..RenderRequest::default() });
        assert!(before.error.is_none(), "{:?}", before.error);

        let out = clearscan_recognized(
            &ocr,
            ScanRaster { pixels: &before.rgba, width: before.width, height: before.height, channels: 4, points_per_pixel: 1.0 / 3.0 },
            &tuned(1, 1.0),
            no_progress,
        )
        .expect("real ocrs clearscan");
        assert!(out.glyphs >= 6, "expected recognized glyphs, got {}", out.glyphs);

        let mut after_renderer = PageRenderer::new(Arc::new(out.pdf), RenderConfig::default());
        let after = after_renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, scale: 3.0, ..RenderRequest::default() });
        assert!(after.error.is_none(), "{:?}", after.error);

        // Compare vertical ink centroids of the top half (line 0: "gap dog peg") and bottom half
        // (line 1: "cat hat mat"): both lines must stay within 2 px of their original vertical centroid.
        let band_cy = |img: &pdfcraft_render::RenderedPage, y0: u32, y1: u32| -> (usize, f64) {
            let mut count = 0_usize;
            let mut sum_y = 0.0_f64;
            for y in y0..y1.min(img.height) {
                for x in 0..img.width {
                    let idx = (y as usize * img.width as usize + x as usize) * 4;
                    if img.rgba[idx] < 128 {
                        count += 1;
                        sum_y += f64::from(y);
                    }
                }
            }
            (count, if count > 0 { sum_y / count as f64 } else { 0.0 })
        };
        let mid_y = before.height / 2;
        let (b0_n, b0_cy) = band_cy(&before, 0, mid_y);
        let (a0_n, a0_cy) = band_cy(&after, 0, mid_y);
        let (b1_n, b1_cy) = band_cy(&before, mid_y, before.height);
        let (a1_n, a1_cy) = band_cy(&after, mid_y, after.height);
        assert!(b0_n > 100 && a0_n > 100, "line 0 ink missing: before={b0_n}, after={a0_n}");
        assert!(b1_n > 100 && a1_n > 100, "line 1 ink missing: before={b1_n}, after={a1_n}");
        assert!((a0_cy - b0_cy).abs() <= 2.0, "descender line 0 shifted vertically: before={b0_cy:.2}, after={a0_cy:.2}");
        assert!((a1_cy - b1_cy).abs() <= 2.0, "normal line 1 shifted vertically: before={b1_cy:.2}, after={a1_cy:.2}");
    }

    #[test]
    fn unmatched_placements_sort_by_y_then_x_and_do_not_merge_into_one_word() {
        let mk = |line: Option<usize>, word: Option<usize>, x: f64, y_down: f64, text: &str| Placed {
            font: 0,
            code: 1,
            origin: [x, 100.0 - y_down],
            advance: 6.0,
            size: 12.0,
            color: [0, 0, 0],
            line,
            word,
            y_down,
            text: text.into(),
        };
        let mut placed = vec![
            mk(None, None, 10.0, 80.0, "Z"),
            mk(None, None, 50.0, 20.0, "X"),
            mk(None, None, 60.0, 20.0, "Y"),
            mk(Some(0), Some(0), 10.0, 20.0, "A"),
        ];
        sort_placed(&mut placed, &[0]);
        let seq: Vec<&str> = placed.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(seq, vec!["A", "X", "Y", "Z"]);
        assert_eq!(joined_placed_text(&placed), "A\nX Y\nZ");
    }

    #[test]
    fn more_than_254_masters_splits_into_f2_while_keeping_space_in_f1() {
        let raster = PageRaster::from_pixels(64, 64, 1, &vec![250_u8; 64 * 64]).expect("raster");
        let page_mask = crate::binarize::Bitmap::from_ink(64, 64, vec![0_u8; 64 * 64]).expect("mask");
        let lines = vec![AnnotatedLine { words: Vec::new(), baseline: 20.0, x_height: 8.0, cap_height: 12.0, descent: 2.0 }];
        let masters: Vec<Master> = (0..255_usize)
            .map(|i| Master {
                glyph: Type3Glyph { text: "a".into(), outline: GlyphOutline::default(), baseline: 10.0, advance: 4.0 },
                shows: vec![Occurrence {
                    line: Some(0),
                    word: Some(i),
                    y_down: 20.0,
                    origin_x: (i % 10) as f64 * 4.0,
                    text: "a".into(),
                    bounds: [0, 0, 2, 2],
                    mask: None,
                }],
            })
            .collect();
        let frame = ComposeFrame {
            points_per_pixel: 1.0,
            source_ppp: 1.0,
            to_original: crate::deskew::Affine::IDENTITY,
            geometry: None,
            page_mask: &page_mask,
        };
        let out = compose(&raster, &[], &masters, &lines, frame, &ClearScanOptions::default(), 0).expect("255 masters split across F1 and F2");
        assert_eq!(out.clusters, 255);
        assert_eq!(out.placements.iter().filter(|p| p.font == 1).count(), 254);
        assert_eq!(out.placements.iter().filter(|p| p.font == 2).count(), 1);
        let cos = Document::open(Arc::new(out.pdf)).expect("open pdf");
        let root = cos.trailer().reference(b"Root").expect("root");
        let pages_ref = cos.dict(&Object::Ref(root)).expect("catalog").reference(b"Pages").expect("pages");
        let pages_dict = cos.dict(&Object::Ref(pages_ref)).expect("pages dict");
        let kids = pages_dict.get(b"Kids").expect("kids");
        let first_page_ref = cos.resolve(kids).as_array().expect("kids array").first().and_then(Object::as_ref).expect("page ref");
        let page = cos.dict(&Object::Ref(first_page_ref)).expect("page dict");
        let res = cos.dict(page.get(b"Resources").expect("resources")).expect("res");
        let fonts = cos.dict(res.get(b"Font").expect("font")).expect("fonts");
        let f1 = cos.dict(fonts.get(b"F1").expect("F1")).expect("F1 dict");
        let f2 = cos.dict(fonts.get(b"F2").expect("F2")).expect("F2 dict");
        assert_eq!(f1.get(b"LastChar").and_then(Object::as_int), Some(255), "F1 holds 254 masters + space = 255");
        assert_eq!(f2.get(b"LastChar").and_then(Object::as_int), Some(1), "F2 holds the 255th master at code 1");
    }
}
