//! ClearScan page pipeline (stages 1–6).
//!
//! [`clearscan_image`] turns one page raster into a one-page PDF: deskew and binarize, connected
//! components, alignment with the recognised lines, centroid-aligned prototypes, Bézier outlines,
//! a Type 3 font with `/ToUnicode`, and a background the lettering has been painted out of.
//! Nothing here calls potrace, autotrace or any other copyleft tracer. The outlines are the
//! paths [`crate::vectorize`] already fitted, written as PDF operators by [`crate::synthesize_type3`].
//!
//! Lines passed to the recognizer callback are in the deskewed frame, the same frame as
//! [`PreprocessedPage`]. A simple Type 3 font holds 255 drawing glyphs; further shapes go into
//! more fonts on the same page, up to [`MAX_FONTS`].

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};

use crate::Ocr;
use crate::OcrError;
use crate::align::{AlignParams, MatchRole, ResolvedGlyph, align};
use crate::annotations::AnnotatedLine;
use crate::background::{
    ClearScanLayer, DerenderParams, PageRaster, content_stream, derender, embed_background, encoding_for, protected_regions, text_regions,
};
use crate::cluster::{ClusterParams, cluster};
use crate::components::{AnalyzeOptions, analyze_page};
use crate::grey::MAX_PIXELS;
use crate::preprocess::{PreprocessedOptions, PreprocessedPage, preprocess};
use crate::type3::{Type3Glyph, Type3Metrics, baseline_in_prototype, em_pixels, synthesize_type3};
use crate::vectorize::{VectorizeParams, vectorize};

/// Drawing glyphs in one simple Type 3 font. Matches the limit in [`crate::synthesize_type3`].
const PER_FONT: usize = 255;
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

/// Run the pipeline. `recognize` reads the deskewed page; its boxes must use that frame.
pub fn clearscan_image<R, P>(
    raster: ScanRaster<'_>,
    options: &ClearScanOptions,
    mut recognize: R,
    mut progress: P,
) -> Result<ClearScanOutput, OcrError>
where
    R: FnMut(&PreprocessedPage) -> Result<Vec<AnnotatedLine>, OcrError>,
    P: FnMut(ClearScanStage),
{
    let (buf, src_w, src_h, ppp) = prepare_raster(&raster, options, &mut progress)?;
    progress(ClearScanStage::Preprocess);
    let page = preprocess(&buf, src_w, src_h, raster.channels, &options.preprocess)?;
    progress(ClearScanStage::Recognize);
    let lines = recognize(&page)?;
    progress(ClearScanStage::Components);
    let components = analyze_page(&page, &options.analyze)?;
    progress(ClearScanStage::Align);
    let aligned = align(&components, &lines, &options.align);
    progress(ClearScanStage::Cluster);
    let clustered = cluster(&page.grey, &aligned.matched, &options.cluster);
    progress(ClearScanStage::Vectorize);
    let (masters, skipped_clusters) = masters_of(&clustered, &aligned.matched, &lines, options)?;
    progress(ClearScanStage::Compose);
    let raster = deskewed_raster(&buf, src_w, src_h, raster.channels, &page)?;
    compose(&raster, components.components(), &masters, &lines, ppp, options, skipped_clusters)
}

/// [`clearscan_image`] with [`Ocr::recognize_page`] as the recognizer.
pub fn clearscan_recognized<P>(ocr: &Ocr, raster: ScanRaster<'_>, options: &ClearScanOptions, progress: P) -> Result<ClearScanOutput, OcrError>
where
    P: FnMut(ClearScanStage),
{
    clearscan_image(raster, options, |page| ocr.recognize_page(page), progress)
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
    // The page keeps its user-unit size: fewer pixels, each covering more of the page.
    let ppp = points_per_pixel * f64::from(width) / f64::from(new_w);
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
    y_down: f64,
    origin_x: f64,
    text: String,
}

fn masters_of(
    clustered: &crate::cluster::Clusters,
    glyphs: &[ResolvedGlyph],
    lines: &[AnnotatedLine],
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
        let Some(first_index) = group.members().first().copied() else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        let Some(first) = glyphs.get(first_index) else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        let page_baseline = baseline_of(first, lines);
        let Some(baseline) = baseline_in_prototype(anchor[1], page_baseline, first.component.centroid[1]) else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        let advance = f64::from(prototype.binary().width());
        if !advance.is_finite() || advance < 0.0 {
            skipped = skipped.saturating_add(1);
            continue;
        }
        let outline = vectorize(prototype.binary(), &options.vectorize);
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
            shows.push(Occurrence { y_down: baseline_of(glyph, lines), origin_x, text: text.to_string() });
        }
        if shows.is_empty() {
            skipped = skipped.saturating_add(1);
            continue;
        }
        masters.push(Master { glyph: Type3Glyph { text: text.to_string(), outline, baseline, advance }, shows });
    }
    let cap = PER_FONT.saturating_mul(MAX_FONTS);
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
    match role {
        MatchRole::Character { source, .. } | MatchRole::Fragment { source, .. } => Some(source.line),
        MatchRole::Ligature { sources, .. } => sources.first().map(|source| source.line),
        MatchRole::Unmatched => None,
    }
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
    size: f64,
    y_down: f64,
    text: String,
}

fn compose(
    raster: &PageRaster,
    components: &[crate::components::Component],
    masters: &[Master],
    lines: &[AnnotatedLine],
    points_per_pixel: f64,
    options: &ClearScanOptions,
    skipped_clusters: usize,
) -> Result<ClearScanOutput, OcrError> {
    let text = text_regions(components);
    let protect = protected_regions(components);
    let cleaned = derender(raster, &text, &protect, &options.derender)?;
    let erased = cleaned.erased();
    let encoding = encoding_for(cleaned.raster());
    let height_px = f64::from(cleaned.raster().height());
    let metrics = page_metrics(lines);
    // The same em the Type 3 font is built with. `size / em` is one prototype pixel.
    let em = em_pixels(&metrics)?;
    let size = em * points_per_pixel;
    let mut placed = Vec::new();
    let mut doc = Document::new_empty();
    let mut font_ids = Vec::new();
    if !masters.is_empty() {
        if !size.is_finite() || size <= 0.0 || size > MAX_USER {
            return Err(clearscan_err(format!("the font size must be a finite number in (0, {MAX_USER}], got {size}")));
        }
        for (font_index, chunk) in masters.chunks(PER_FONT).enumerate() {
            let glyphs: Vec<Type3Glyph> = chunk.iter().map(|master| master.glyph.clone()).collect();
            let written = synthesize_type3(&mut doc, &glyphs, &metrics)?;
            for (glyph_index, master) in chunk.iter().enumerate() {
                let Some(&code) = written.codes.get(glyph_index) else { continue };
                for show in &master.shows {
                    let origin_x = show.origin_x * points_per_pixel;
                    let origin_y = (height_px - show.y_down) * points_per_pixel;
                    if !origin_x.is_finite() || !origin_y.is_finite() || origin_x.abs() > MAX_USER || origin_y.abs() > MAX_USER {
                        continue;
                    }
                    placed.push(Placed { font: font_index, code, origin: [origin_x, origin_y], size, y_down: show.y_down, text: show.text.clone() });
                }
            }
            font_ids.push(written.id);
        }
    }
    placed.sort_by(|a, b| {
        a.y_down
            .partial_cmp(&b.y_down)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.origin[0].partial_cmp(&b.origin[0]).unwrap_or(std::cmp::Ordering::Equal))
    });
    let width = f64::from(cleaned.raster().width()) * points_per_pixel;
    let height = height_px * points_per_pixel;
    let image = embed_background(&mut doc, &cleaned, encoding)?;
    let pdf = write_page(&mut doc, &image, &font_ids, &placed, width, height)?;
    let text_out: String = placed.iter().map(|show| show.text.as_str()).collect();
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

fn deskewed_raster(pixels: &[u8], width: u32, height: u32, channels: u32, page: &PreprocessedPage) -> Result<PageRaster, OcrError> {
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

fn write_page(
    doc: &mut Document,
    image: &crate::background::BackgroundImage,
    fonts: &[pdfcraft_cos::ObjRef],
    placed: &[Placed],
    width: f64,
    height: f64,
) -> Result<Vec<u8>, OcrError> {
    let image_only =
        ClearScanLayer { image_name: "Im0", image_width: width, image_height: height, image_origin: [0.0, 0.0], font_name: "F1", glyphs: &[] };
    let mut content = content_stream(&image_only)?;
    if !placed.is_empty() {
        content.extend(text_operators(placed));
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
    if !fonts.is_empty() {
        let mut font_dict = Dict::new();
        for (index, id) in fonts.iter().enumerate() {
            let name = font_name(index)?;
            font_dict.set(name.into_bytes(), Object::Ref(*id));
        }
        resources.set(b"Font".to_vec(), Object::Dict(font_dict));
    }
    let mut page = Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(width), Object::Real(height)]));
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

/// The text block from the background writer, for a page that switches fonts. That writer
/// names one font; past 255 shapes the page uses `F1`, `F2`, and so on, so the block is
/// written here. The operators stay `q 0 g BT 0 Tr`, then `Tf`, `Tm`, and `Tj`.
fn text_operators(placed: &[Placed]) -> Vec<u8> {
    let mut out = String::from("q\n0 g\nBT\n0 Tr\n");
    let mut current_font: Option<usize> = None;
    let mut current_size: Option<f64> = None;
    for glyph in placed {
        let switched = current_font != Some(glyph.font) || current_size.is_none_or(|size| (size - glyph.size).abs() > 1.0e-4);
        if switched {
            let name = format!("F{}", glyph.font.saturating_add(1));
            out.push('/');
            out.push_str(&name);
            out.push(' ');
            out.push_str(&crate::pdf_num(glyph.size));
            out.push_str(" Tf\n");
            current_font = Some(glyph.font);
            current_size = Some(glyph.size);
        }
        out.push_str(&crate::pdf_num(1.0));
        out.push(' ');
        out.push_str(&crate::pdf_num(0.0));
        out.push(' ');
        out.push_str(&crate::pdf_num(0.0));
        out.push(' ');
        out.push_str(&crate::pdf_num(1.0));
        out.push(' ');
        out.push_str(&crate::pdf_num(glyph.origin[0]));
        out.push(' ');
        out.push_str(&crate::pdf_num(glyph.origin[1]));
        out.push_str(" Tm\n<");
        out.push_str(&format!("{:02X}", glyph.code));
        out.push_str("> Tj\n");
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
        ClearScanOptions { downsample, preprocess: crate::preprocess::PreprocessedOptions { max_skew_degrees, ..base.preprocess }, ..base }
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
        // Deskew adds a sampling margin. At downsample 2 each of those pixels is 2 user units.
        assert!((out.width - f64::from(width)).abs() < 16.0, "width {}", out.width);
        assert!(out.text.contains('H'), "{:?}", out.text);
    }
}
