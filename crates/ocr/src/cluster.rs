//! Glyph clustering and centroid-aligned master prototypes (ClearScan stage 3).
//!
//! One letter printed two hundred times should become one font glyph, not two hundred vector
//! paths. [`cluster`] groups the resolved glyphs of a page — [`cluster_pages`] the glyphs of
//! a whole document — into clusters of the same shape, then averages the aligned greyscale
//! of every instance into one master prototype: the sensor noise, paper grain and ink bleed
//! that differ between occurrences cancel out, and what remains is the letter as printed.
//!
//! **Pruning.** The OCR hypothesis is the first cut: only glyphs recognised as the same text
//! are ever compared (an 'e' never meets an 'a'). Within a text group, box size and ink area
//! are cheap gates, then a 16×16 similarity hash of the normalised mask (`signature` — a
//! perceptual hash), and only then the exact metric: the ink intersection over union of the
//! two masks placed with their centres of mass on one point (`ink_iou`). Instances that
//! hash equal are verified against their bucket's representative directly, so a page of
//! clean repeats costs no pairwise scanning at all.
//!
//! **Clustering.** The exact metric turns the instances into a graph — an edge where two
//! glyphs match — and its connected components (single-linkage hierarchical clustering cut
//! at the match threshold; DBSCAN with `min_samples` 1) are the clusters. The thresholds
//! err towards splitting: aligned by centre of mass, 'c' against 'e' is tipped out of
//! register by the crossbar's missing weight and the metric drops it; if a lookalike's
//! centroid were to coincide anyway, the ink-area gate catches it; and the text grouping
//! keeps 'm' from 'rn' however alike the ink.
//!
//! **Alignment.** Every instance is placed by its centre of mass (the raw moments M10/M00,
//! M01/M00 of [`Component::centroid`]), not by its bounding box: one speckle on the left
//! edge moves a box corner a whole pixel but the centroid by a fraction of one. The canvas
//! origin is the most extreme member's centroid-relative box edge, so each instance's pixel
//! lattice lands on the canvas grid, and the accumulation samples the page greyscale at
//! each fractional position (`sample`, bilinear): the instances' small centroid spread is
//! all the blur there is. Corner alignment would stack the same stroke a pixel apart and
//! average it into a smear.
//!
//! **Prototype.** The accumulated average is binarized with Otsu over its own histogram —
//! ink that was ink in every occurrence comes out far darker than paper that was speckled
//! in a few — and that pristine master bitmap is what the vectorizer traces next.

use std::collections::{HashMap, HashSet};

use crate::align::{MatchRole, ResolvedGlyph};
use crate::binarize::{Bitmap, otsu_threshold};
use crate::components::{Component, UnionFind, mask_plan};
use crate::grey::{GreyImage, MAX_PIXELS};

/// Side of the similarity-hash grid: a mask is normalised onto `16 × 16` sample points.
const SIGNATURE_SIDE: u32 = 16;
/// u64 words a signature packs its `16 × 16` bits into.
const SIGNATURE_WORDS: usize = (SIGNATURE_SIDE * SIGNATURE_SIDE / 64) as usize;

/// Sub-pixel slack when turning a member's centroid-relative box span into canvas pixel
/// ranges: it swallows the f64 residue of a span that starts or ends exactly on a pixel
/// boundary, and is seven orders of magnitude below anything printable.
const COVERAGE_FUZZ: f64 = 1e-9;

/// The knobs of [`cluster`]. Defaults suit body text at scanning resolution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterParams {
    /// Minimum ink intersection over union, centroid-aligned, for two glyphs to match.
    /// Strict: near-repeats pass, a fused crossbar does not.
    pub iou_min: f64,
    /// Box width and height may differ by at most this fraction of the larger. A heading's
    /// 'e' never joins the body text's.
    pub size_tolerance: f64,
    /// Ink area may differ by at most this fraction of the larger. A lookalike whose centre
    /// of mass happens to coincide ('c' in 'e', one crossbar of ink apart) slips past the
    /// exact metric's registration; this gate catches it.
    pub area_tolerance: f64,
    /// Similarity-hash bits (of 256) two glyphs may differ by before the exact metric runs.
    /// Purely a pruning knob: it only ever keeps the exact metric from running.
    pub signature_tolerance: u32,
    /// A prototype canvas larger than this many pixels is not built; the cluster stands
    /// without one.
    pub max_prototype_pixels: u64,
    /// Total pixels of prototype canvas across all clusters. Smaller canvases are built
    /// first, so a page of glyphs keeps its masters before a giant heading spends the budget.
    pub max_total_prototype_pixels: u64,
}

impl Default for ClusterParams {
    fn default() -> Self {
        ClusterParams {
            iou_min: 0.65,
            size_tolerance: 0.25,
            area_tolerance: 0.12,
            signature_tolerance: 96,
            max_prototype_pixels: 1 << 20,
            max_total_prototype_pixels: 1 << 25,
        }
    }
}

/// Why a glyph was left out of every cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// No OCR text to cluster by: the alignment could not match it to a character.
    NoText,
    /// The component's mask was not kept (its box was over the mask budget at labeling).
    NoMask,
}

/// A cluster's master glyph: the aligned average of its instances, and the pristine bitmap
/// cut from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Prototype {
    grey: GreyImage,
    binary: Bitmap,
    /// Where the instances' centres of mass converge, in prototype pixels: every instance
    /// was placed so its centroid sits exactly here. Later stages position the glyph by
    /// this anchor and measure how far its ink reaches from it.
    anchor: [f64; 2],
    members: u32,
}

impl Prototype {
    /// The averaged greyscale (0 = black ink, 255 = white paper): noise and grain have
    /// cancelled out of it.
    pub fn grey(&self) -> &GreyImage {
        &self.grey
    }

    /// The pristine master bitmap, Otsu-thresholded from [`Prototype::grey`].
    pub fn binary(&self) -> &Bitmap {
        &self.binary
    }

    /// The alignment anchor in prototype pixels: the point every instance's centroid was
    /// placed on.
    pub fn anchor(&self) -> [f64; 2] {
        self.anchor
    }

    /// How many instances were averaged.
    pub fn members(&self) -> u32 {
        self.members
    }

    /// The master ink's half-open box `[left, top, right, bottom]` in prototype pixels, or
    /// `None` when the prototype holds no ink.
    pub fn ink_bounds(&self) -> Option<[u32; 4]> {
        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0_u32, 0_u32);
        for y in 0..self.binary.height() {
            for x in 0..self.binary.width() {
                if !self.binary.is_ink(i64::from(x), i64::from(y)) {
                    continue;
                }
                left = left.min(x);
                top = top.min(y);
                right = right.max(x.saturating_add(1));
                bottom = bottom.max(y.saturating_add(1));
            }
        }
        (right > left && bottom > top).then_some([left, top, right, bottom])
    }
}

/// One cluster: every occurrence the document holds of one shape, and its master prototype.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphCluster {
    text: String,
    /// Indices into the glyphs the caller passed (for [`cluster_pages`], into their
    /// concatenation, page by page), ascending.
    members: Vec<usize>,
    /// `None` when the canvas was over [`ClusterParams::max_prototype_pixels`] or the
    /// total budget: the cluster stands, without a master to vectorize.
    prototype: Option<Prototype>,
}

impl GlyphCluster {
    /// The OCR text every member was recognised as (the clustering key).
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn members(&self) -> &[usize] {
        &self.members
    }

    pub fn prototype(&self) -> Option<&Prototype> {
        self.prototype.as_ref()
    }
}

/// The clustering of a document: its glyph clusters, and the glyphs left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clusters {
    clusters: Vec<GlyphCluster>,
    skipped: Vec<(usize, SkipReason)>,
}

impl Clusters {
    /// The clusters, ordered by their first member (the document's reading order).
    pub fn clusters(&self) -> &[GlyphCluster] {
        &self.clusters
    }

    /// The glyphs no cluster took, with why, in input order.
    pub fn skipped(&self) -> &[(usize, SkipReason)] {
        &self.skipped
    }
}

/// Cluster one page's glyphs. See [`cluster_pages`] for the document-wide form.
pub fn cluster(grey: &GreyImage, glyphs: &[ResolvedGlyph], params: &ClusterParams) -> Clusters {
    cluster_pages(&[(grey, glyphs)], params)
}

/// Cluster the glyphs of every page together: the hundred 'e's of a document average into
/// one prototype however many pages they were printed on. `pages` holds each page's
/// greyscale (the frame its components were labeled in) and its aligned glyphs; cluster
/// members index the concatenation of the pages' glyphs, in page order.
pub fn cluster_pages(pages: &[(&GreyImage, &[ResolvedGlyph])], params: &ClusterParams) -> Clusters {
    let instances: Vec<&ResolvedGlyph> = pages.iter().flat_map(|(_, glyphs)| glyphs.iter()).collect();
    let page_of: Vec<&GreyImage> = pages.iter().flat_map(|(grey, glyphs)| std::iter::repeat_n(*grey, glyphs.len())).collect();
    let mut skipped = Vec::new();
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, glyph) in instances.iter().enumerate() {
        let Some(text) = role_text(&glyph.role) else {
            skipped.push((index, SkipReason::NoText));
            continue;
        };
        if glyph.component.mask.is_none() {
            skipped.push((index, SkipReason::NoMask));
            continue;
        }
        groups.entry(text).or_default().push(index);
    }
    let mut grouped: Vec<Vec<usize>> = Vec::new();
    for members in groups.values() {
        grouped.extend(group_clusters(&instances, members, params));
    }
    let plan = prototype_plan(&instances, &grouped, params);
    let mut clusters: Vec<GlyphCluster> = Vec::new();
    for (cluster_index, members) in grouped.into_iter().enumerate() {
        let text = members
            .first()
            .and_then(|&index| instances.get(index).map(|glyph| role_text(&glyph.role)))
            .unwrap_or_default()
            .unwrap_or_default()
            .to_string();
        let prototype = if plan.get(cluster_index).copied().unwrap_or(false) {
            prototype(&page_of, &instances, &members, params.max_prototype_pixels)
        } else {
            None
        };
        clusters.push(GlyphCluster { text, members, prototype });
    }
    clusters.sort_by(|a, b| a.members.first().cmp(&b.members.first()));
    Clusters { clusters, skipped }
}

/// The text a glyph was recognised as, when it is the key a cluster groups by.
fn role_text(role: &MatchRole) -> Option<&str> {
    match role {
        MatchRole::Character { text, .. } | MatchRole::Fragment { text, .. } | MatchRole::Ligature { text, .. } => Some(text.as_str()),
        MatchRole::Unmatched => None,
    }
    .filter(|text| !text.is_empty())
}

/// Connected components of the matches within one text group.
fn group_clusters(instances: &[&ResolvedGlyph], members: &[usize], params: &ClusterParams) -> Vec<Vec<usize>> {
    // Instances with the same similarity hash bucket together; each member is verified
    // against its bucket's representative with the exact metric, so a hash collision never
    // merges two shapes. What fails joins the representatives for the pairwise pass instead.
    let mut buckets: HashMap<[u64; SIGNATURE_WORDS], Vec<usize>> = HashMap::new();
    for &index in members {
        if let Some(mask) = instances.get(index).and_then(|glyph| glyph.component.mask.as_ref()) {
            buckets.entry(signature(mask)).or_default().push(index);
        }
    }
    let mut bucket_list: Vec<Vec<usize>> = buckets.into_values().collect();
    bucket_list.sort_by(|a, b| a.first().cmp(&b.first()));
    let mut uf = UnionFind::with_len(instances.len().saturating_add(1));
    let mut representatives: Vec<usize> = Vec::new();
    for bucket in &bucket_list {
        let Some(&head) = bucket.first() else {
            continue;
        };
        representatives.push(head);
        for &member in bucket.iter().skip(1) {
            let pair = instances.get(head).zip(instances.get(member));
            if pair.is_some_and(|(head, member)| matches_shape(&head.component, &member.component, params)) {
                uf.union(label(head), label(member));
            } else {
                representatives.push(member);
            }
        }
    }
    pair_representatives(instances, &representatives, &mut uf, params);
    let mut by_root: HashMap<u32, Vec<usize>> = HashMap::new();
    for &member in members {
        by_root.entry(uf.find(label(member))).or_default().push(member);
    }
    let mut clusters: Vec<Vec<usize>> = by_root.into_values().collect();
    for cluster in &mut clusters {
        cluster.sort_unstable();
        cluster.dedup();
    }
    clusters.sort_by(|a, b| a.first().cmp(&b.first()));
    clusters
}

/// A usize as a union-find label. The structure reserves 0 for "no label" (in
/// [`crate::components`] it is the paper), so instances are labeled from 1. Instance counts
/// are memory-bound far below u32::MAX; anything past it saturates rather than wrap.
fn label(index: usize) -> u32 {
    u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX)
}

/// Match every pair of representatives, cheapest gate first: sizes, then the similarity
/// hash, then the exact centroid-aligned ink metric. The height sort lets the inner loop
/// stop at the first glyph too tall to match.
fn pair_representatives(instances: &[&ResolvedGlyph], representatives: &[usize], uf: &mut UnionFind, params: &ClusterParams) {
    let mut order: Vec<usize> = representatives.to_vec();
    // Height, then width, then index: a stable, total order.
    let height = |index: usize| instances.get(index).map(|glyph| glyph.component.height()).unwrap_or(0);
    let width = |index: usize| instances.get(index).map(|glyph| glyph.component.width()).unwrap_or(0);
    order.sort_by(|&a, &b| height(a).cmp(&height(b)).then(width(a).cmp(&width(b))).then(a.cmp(&b)));
    for position in 0..order.len() {
        let Some(&left) = order.get(position) else {
            continue;
        };
        let Some(left_component) = instances.get(left).map(|glyph| &glyph.component) else {
            continue;
        };
        for &right in order.iter().skip(position.saturating_add(1)) {
            let Some(right_component) = instances.get(right).map(|glyph| &glyph.component) else {
                continue;
            };
            let tolerance = params.size_tolerance.clamp(0.0, 16.0);
            if !within(left_component.height(), right_component.height(), tolerance) {
                break; // sorted by height: every later glyph is taller still
            }
            if !within(left_component.width(), right_component.width(), tolerance) {
                continue;
            }
            if !within(left_component.area, right_component.area, params.area_tolerance.clamp(0.0, 1.0)) {
                continue;
            }
            let masks = left_component.mask.as_ref().zip(right_component.mask.as_ref());
            let hashed =
                masks.is_some_and(|(left_mask, right_mask)| hamming(&signature(left_mask), &signature(right_mask)) <= params.signature_tolerance);
            if hashed && ink_iou(left_component, right_component) >= params.iou_min {
                uf.union(label(left), label(right));
            }
        }
    }
}

/// Whether two values are within `tolerance` of each other, as a fraction of the larger.
fn within(a: u32, b: u32, tolerance: f64) -> bool {
    let scale = f64::from(a.max(b));
    scale > 0.0 && (f64::from(a) - f64::from(b)).abs() / scale <= tolerance
}

/// The full match predicate: box size, ink area, and the exact centroid-aligned ink metric.
fn matches_shape(a: &Component, b: &Component, params: &ClusterParams) -> bool {
    let tolerance = params.size_tolerance.clamp(0.0, 16.0);
    within(a.height(), b.height(), tolerance)
        && within(a.width(), b.width(), tolerance)
        && within(a.area, b.area, params.area_tolerance.clamp(0.0, 1.0))
        && ink_iou(a, b) >= params.iou_min
}

/// The similarity hash of a mask: its ink sampled on a 16×16 grid over the bounding box,
/// packed into four u64. Coarse — a pruning key, never the decision.
fn signature(mask: &Bitmap) -> [u64; SIGNATURE_WORDS] {
    let mut words = [0_u64; SIGNATURE_WORDS];
    let (width, height) = (mask.width(), mask.height());
    if width == 0 || height == 0 {
        return words;
    }
    for cy in 0..SIGNATURE_SIDE {
        for cx in 0..SIGNATURE_SIDE {
            let x = ((f64::from(cx) + 0.5) * f64::from(width) / f64::from(SIGNATURE_SIDE)).floor() as i64;
            let y = ((f64::from(cy) + 0.5) * f64::from(height) / f64::from(SIGNATURE_SIDE)).floor() as i64;
            if !mask.is_ink(x, y) {
                continue;
            }
            let bit = cy * SIGNATURE_SIDE + cx;
            if let Some(word) = words.get_mut((bit / 64) as usize) {
                *word |= 1_u64 << (bit % 64);
            }
        }
    }
    words
}

/// How many bits two signatures differ by.
fn hamming(a: &[u64; SIGNATURE_WORDS], b: &[u64; SIGNATURE_WORDS]) -> u32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// Ink intersection over union of two components, each placed with its (rounded) centre of
/// mass on one point. Centroid placement is robust where a box corner is not: a speckle on
/// an edge moves the centroid by a fraction of a pixel, the corner by a whole one.
fn ink_iou(a: &Component, b: &Component) -> f64 {
    let (Some(mask_a), Some(mask_b)) = (a.mask.as_ref(), b.mask.as_ref()) else {
        return 0.0;
    };
    let place = |component: &Component, mask: &Bitmap| -> HashSet<(i32, i32)> {
        let [cx, cy] = component.centroid;
        // A capacity hint only; a hand-built component could claim any area, so it is capped
        // rather than trusted with the allocation.
        let mut offsets = HashSet::with_capacity(u64::from(component.area).min(1 << 16) as usize);
        for y in 0..mask.height() {
            for x in 0..mask.width() {
                if !mask.is_ink(i64::from(x), i64::from(y)) {
                    continue;
                }
                let page_x = f64::from(component.bounds[0].saturating_add(x));
                let page_y = f64::from(component.bounds[1].saturating_add(y));
                offsets.insert(((page_x - cx).round() as i32, (page_y - cy).round() as i32));
            }
        }
        offsets
    };
    let set_a = place(a, mask_a);
    let mut intersection = 0_u64;
    for offset in place(b, mask_b) {
        if set_a.contains(&offset) {
            intersection = intersection.saturating_add(1);
        }
    }
    let union = u64::from(a.area) + u64::from(b.area) - intersection;
    if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
}

/// Which clusters get a prototype canvas: the same small-first plan the mask budget uses.
fn prototype_plan(instances: &[&ResolvedGlyph], grouped: &[Vec<usize>], params: &ClusterParams) -> Vec<bool> {
    let areas: Vec<u64> = grouped
        .iter()
        .map(|members| canvas_extents(instances, members).map(|(_, _, width, height)| u64::from(width) * u64::from(height)).unwrap_or(u64::MAX))
        .collect();
    mask_plan(&areas, params.max_prototype_pixels, params.max_total_prototype_pixels)
}

/// The prototype canvas, in centroid-relative pixels: the union of the members' boxes each
/// placed so its centroid sits at the origin, returned as `(min_x, min_y, width, height)`.
fn canvas_extents(instances: &[&ResolvedGlyph], members: &[usize]) -> Option<(f64, f64, u32, u32)> {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut seen = false;
    for &index in members {
        let component = &instances.get(index)?.component;
        let [cx, cy] = component.centroid;
        if !cx.is_finite() || !cy.is_finite() {
            continue;
        }
        let [left, top, right, bottom] = component.bounds;
        min_x = min_x.min(f64::from(left) - cx);
        min_y = min_y.min(f64::from(top) - cy);
        max_x = max_x.max(f64::from(right) - cx);
        max_y = max_y.max(f64::from(bottom) - cy);
        seen = true;
    }
    if !seen {
        return None;
    }
    let width = (max_x - min_x).ceil().max(1.0);
    let height = (max_y - min_y).ceil().max(1.0);
    if !width.is_finite() || !height.is_finite() || width > u32::MAX as f64 || height > u32::MAX as f64 {
        return None;
    }
    Some((min_x, min_y, width as u32, height as u32))
}

/// Average every member's greyscale, aligned by centroid, and threshold the average.
fn prototype(page_of: &[&GreyImage], instances: &[&ResolvedGlyph], members: &[usize], cap: u64) -> Option<Prototype> {
    let (min_x, min_y, width, height) = canvas_extents(instances, members)?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > cap || pixels > MAX_PIXELS {
        return None;
    }
    let len = usize::try_from(pixels).ok()?;
    let mut sums = vec![0.0_f32; len];
    let mut counts = vec![0_u32; len];
    let width_us = width as usize;
    for &index in members {
        let component = &instances.get(index)?.component;
        let [cx, cy] = component.centroid;
        let [left, top, right, bottom] = component.bounds;
        if !cx.is_finite() || !cy.is_finite() {
            continue;
        }
        let grey = page_of.get(index).copied()?;
        // Canvas pixel x covers centroid-relative [min_x + x, min_x + x + 1) and is sampled
        // at its left edge — where the instances' pixel lattices sit, each anchored at its
        // own centroid — and the member contributes where that point lies in its box. The
        // epsilon absorbs the f64 fuzz of "exactly on the boundary": without it a span
        // starting 6e-15 past an edge ceils to the next pixel and leaves a whole column of
        // the canvas uncovered. Past the edge it only ever samples one row of paper.
        let x_lo = ((f64::from(left) - cx - min_x - COVERAGE_FUZZ).ceil().clamp(0.0, f64::from(width))) as u32;
        let x_hi = ((f64::from(right) - cx - min_x + COVERAGE_FUZZ).ceil().clamp(0.0, f64::from(width))) as u32;
        let y_lo = ((f64::from(top) - cy - min_y - COVERAGE_FUZZ).ceil().clamp(0.0, f64::from(height))) as u32;
        let y_hi = ((f64::from(bottom) - cy - min_y + COVERAGE_FUZZ).ceil().clamp(0.0, f64::from(height))) as u32;
        for y in y_lo..y_hi {
            for x in x_lo..x_hi {
                let page_x = cx + min_x + f64::from(x);
                let page_y = cy + min_y + f64::from(y);
                let darkness = 255.0 - sample(grey, page_x, page_y);
                let cell = y as usize * width_us + x as usize;
                if let (Some(sum), Some(count)) = (sums.get_mut(cell), counts.get_mut(cell)) {
                    *sum += darkness;
                    *count = count.saturating_add(1);
                }
            }
        }
    }
    let averaged: Vec<u8> = sums
        .iter()
        .zip(counts.iter())
        .map(|(&sum, &count)| if count == 0 { 255 } else { (255.0 - sum / count as f32).round().clamp(0.0, 255.0) as u8 })
        .collect();
    let grey = GreyImage::new(width, height, averaged)?;
    // Otsu over the averaged histogram: consistent ink is dark, speckled paper is not. A
    // single-level average (nothing sampled at all) falls back to the midpoint.
    let threshold = otsu_threshold(&grey).unwrap_or(128);
    let ink: Vec<u8> = grey.data().iter().map(|&g| u8::from(g < threshold)).collect();
    Some(Prototype {
        binary: Bitmap::from_ink(width, height, ink)?,
        grey,
        anchor: [-min_x, -min_y],
        members: u32::try_from(members.len()).unwrap_or(u32::MAX),
    })
}

/// Bilinear sample of the page greyscale at a fractional position. Outside the page there
/// is paper: an instance at the page edge fails soft to white rather than reach past it.
fn sample(grey: &GreyImage, x: f64, y: f64) -> f32 {
    if !x.is_finite() || !y.is_finite() {
        return 255.0;
    }
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
    let at = |xi: f64, yi: f64| f32::from(grey.get(xi as i64, yi as i64));
    let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
    let bottom = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
    top + (bottom - top) * fy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::align::CharRef;
    use crate::components::{AnalyzeOptions, ComponentKind, ComponentSet, analyze};

    // -- synthetic glyphs ------------------------------------------------------

    /// A drawn pattern and its extent: `pattern(x, y)` is ink inside `width × height`.
    #[derive(Clone, Copy)]
    struct Shape {
        pattern: fn(u32, u32) -> bool,
        width: u32,
        height: u32,
    }

    /// A blocky 'e', 16 × 20, stroke 3: left stem, top and bottom bars to x = 13, a crossbar.
    fn is_e(x: u32, y: u32) -> bool {
        x < 3 || (y < 3 && x < 13) || ((8..11).contains(&y) && (3..13).contains(&x)) || (y >= 17 && x < 13)
    }

    /// A blocky 'c': the 'e' without the crossbar. Same box; 30 pixels less ink.
    fn is_c(x: u32, y: u32) -> bool {
        x < 3 || (y < 3 && x < 13) || (y >= 17 && x < 13)
    }

    /// A blocky 'm', 24 × 20: three stems joined by a crossbar.
    fn is_m(x: u32, y: u32) -> bool {
        x < 4 || (10..14).contains(&x) || x >= 20 || ((4..8).contains(&y) && (4..24).contains(&x))
    }

    /// A blocky 'rn', 24 × 20: two stems under one long arch. Almost an 'm' to the eye.
    fn is_rn(x: u32, y: u32) -> bool {
        !(4..20).contains(&x) || (y < 5 && (4..24).contains(&x))
    }

    const E: Shape = Shape { pattern: is_e, width: 16, height: 20 };
    const C: Shape = Shape { pattern: is_c, width: 16, height: 20 };
    const M: Shape = Shape { pattern: is_m, width: 24, height: 20 };
    const RN: Shape = Shape { pattern: is_rn, width: 24, height: 20 };

    /// Draw `shapes` as clean instances (ink grey 30 on paper 235) on a grid far enough
    /// apart that nothing touches. Returns (ink flags, grey, width, height).
    fn page(shapes: &[Shape]) -> (Vec<u8>, Vec<u8>, u32, u32) {
        let per_row = 8_u32;
        let rows = u32::try_from(shapes.len().div_ceil(per_row as usize)).unwrap_or(1);
        let (width, height) = (40 + per_row * 48, 40 + rows * 64);
        let mut ink = vec![0_u8; (width * height) as usize];
        let mut grey = vec![235_u8; (width * height) as usize];
        for (i, shape) in shapes.iter().enumerate() {
            let i = i as u32;
            let (x0, y0) = (40 + (i % per_row) * 48, 40 + (i / per_row) * 64);
            for y in 0..shape.height {
                for x in 0..shape.width {
                    if !(shape.pattern)(x, y) {
                        continue;
                    }
                    let index = ((y0 + y) * width + x0 + x) as usize;
                    if let Some(flag) = ink.get_mut(index) {
                        *flag = 1;
                    }
                    if let Some(value) = grey.get_mut(index) {
                        *value = 30;
                    }
                }
            }
        }
        (ink, grey, width, height)
    }

    /// Every glyph-kind component of an analyzed page, in reading order — the drawn
    /// instances, noise excluded — wrapped as recognised 'e's.
    fn glyphs_of(set: &ComponentSet) -> Vec<ResolvedGlyph> {
        set.components()
            .iter()
            .filter(|component| component.kind == ComponentKind::Glyph)
            .map(|component| ResolvedGlyph {
                source: 0,
                component: component.clone(),
                role: MatchRole::Character { text: "e".into(), source: CharRef { line: 0, word: 0, index: Some(0) } },
                iou: 1.0,
            })
            .collect()
    }

    fn analyzed(ink: Vec<u8>, width: u32, height: u32) -> ComponentSet {
        let bitmap = Bitmap::from_ink(width, height, ink).unwrap();
        analyze(&bitmap, &AnalyzeOptions::default()).unwrap()
    }

    /// The pristine pattern's centroid: the point every instance is placed on.
    fn centroid(shape: &Shape) -> [f64; 2] {
        let mut sums = [0.0_f64, 0.0];
        let mut area = 0.0_f64;
        for y in 0..shape.height {
            for x in 0..shape.width {
                if (shape.pattern)(x, y) {
                    sums[0] += f64::from(x);
                    sums[1] += f64::from(y);
                    area += 1.0;
                }
            }
        }
        [sums[0] / area, sums[1] / area]
    }

    /// Where a pattern pixel lands in prototype pixels, measured from the anchor.
    fn at(anchor: [f64; 2], center: [f64; 2], x: u32, y: u32) -> (i64, i64) {
        ((anchor[0] + f64::from(x) - center[0]).round() as i64, (anchor[1] + f64::from(y) - center[1]).round() as i64)
    }

    /// The ink pixels of a prototype as centroid-relative offsets.
    fn master(prototype: &Prototype) -> HashSet<(i32, i32)> {
        let mut ink = HashSet::new();
        for y in 0..prototype.binary().height() {
            for x in 0..prototype.binary().width() {
                if prototype.binary().is_ink(i64::from(x), i64::from(y)) {
                    ink.insert((x as i32, y as i32));
                }
            }
        }
        ink
    }

    fn pattern_pixels(shape: &Shape, anchor: [f64; 2], center: [f64; 2]) -> HashSet<(i32, i32)> {
        let mut pixels = HashSet::new();
        for y in 0..shape.height {
            for x in 0..shape.width {
                if (shape.pattern)(x, y) {
                    let (px, py) = at(anchor, center, x, y);
                    pixels.insert((px as i32, py as i32));
                }
            }
        }
        pixels
    }

    // -- the pipeline ----------------------------------------------------------

    #[test]
    fn repeated_clean_glyphs_form_one_cluster_whose_master_is_the_pattern() {
        let (ink, grey, width, height) = page(&[E, E, E, E, E, E, E, E, E]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        assert_eq!(glyphs.len(), 9);
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert!(clustered.skipped().is_empty(), "{:?}", clustered.skipped());
        assert_eq!(clustered.clusters().len(), 1);
        let only = &clustered.clusters()[0];
        assert_eq!(only.text(), "e");
        assert_eq!(only.members(), &(0..9).collect::<Vec<_>>());
        let prototype = only.prototype().unwrap();
        assert_eq!(prototype.members(), 9);
        // Aligned exactly, clean instances average to exactly the printed pattern.
        let center = centroid(&E);
        let expected = pattern_pixels(&E, prototype.anchor(), center);
        let actual = master(prototype);
        let missing: Vec<_> = expected.difference(&actual).collect();
        let extra: Vec<_> = actual.difference(&expected).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "master differs from the pattern: missing {missing:?}, extra {extra:?}, anchor {:?} (center {center:?}, {}×{})",
            prototype.anchor(),
            prototype.grey().width(),
            prototype.grey().height()
        );
    }

    /// The degraded scan of the noise test: nine 'e's, each with its own salt.
    fn degraded_page() -> (Vec<u8>, GreyImage, u32, u32) {
        let per_row = 3_u32;
        let (width, height) = (40 + per_row * 48, 40 + 3 * 64);
        let mut ink = vec![0_u8; (width * height) as usize];
        let mut grey = vec![235_u8; (width * height) as usize];
        let hash = |x: i64, y: i64, i: u32| ((x as u64).wrapping_mul(31) + (y as u64).wrapping_mul(17) + u64::from(i) * 101) % 23;
        let near_ink = |x: u32, y: u32| {
            (-1..=1_i32).any(|dy| {
                (-1..=1_i32).any(|dx| {
                    let (nx, ny) = (i64::from(x) + i64::from(dx), i64::from(y) + i64::from(dy));
                    nx >= 0 && ny >= 0 && nx < 16 && ny < 20 && is_e(nx as u32, ny as u32)
                })
            })
        };
        for i in 0..9_u32 {
            let (x0, y0) = (40 + (i % per_row) * 48, 40 + (i / per_row) * 64);
            // One instance gets a speck welded to the left edge: its box corner moves a
            // whole pixel, its centroid a fraction of one.
            if i == 0 {
                let index = ((y0 + 5) * width + x0 - 1) as usize;
                if let (Some(flag), Some(value)) = (ink.get_mut(index), grey.get_mut(index)) {
                    *flag = 1;
                    *value = 60;
                }
            }
            for y in 0..20_u32 {
                for x in 0..16_u32 {
                    let index = ((y0 + y) * width + x0 + x) as usize;
                    if is_e(x, y) && hash(x as i64, y as i64, i) != 5 {
                        if let (Some(flag), Some(value)) = (ink.get_mut(index), grey.get_mut(index)) {
                            *flag = 1; // ink, unless this instance drops it (hash 5)
                            *value = 30;
                        }
                        continue;
                    }
                    // Salt: bleed attached to the ink (never on the stem's left edge, where
                    // the welded speck already is), and interior specks — noise components
                    // of their own, darker than ink in the grey but seen by the average.
                    let attached = x >= 4 && near_ink(x, y) && hash(x as i64, y as i64, i) == 0;
                    let speck = !near_ink(x, y) && hash(x as i64, y as i64, i) == 7;
                    if (attached || speck)
                        && let (Some(flag), Some(value)) = (ink.get_mut(index), grey.get_mut(index))
                    {
                        *flag = 1;
                        *value = 60;
                    }
                }
            }
        }
        let grey_image = GreyImage::new(width, height, grey).unwrap();
        (ink, grey_image, width, height)
    }

    #[test]
    fn noise_cancels_and_edges_stay_sharp_under_centroid_alignment() {
        let (ink, grey, width, height) = degraded_page();
        let glyphs = glyphs_of(&analyzed(ink.clone(), width, height));
        assert_eq!(glyphs.len(), 9, "specks stay noise, not glyphs");
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert!(clustered.skipped().is_empty());
        assert_eq!(clustered.clusters().len(), 1);
        let prototype = clustered.clusters()[0].prototype().unwrap();
        let center = centroid(&E);

        // Noise cancellation: deep paper (≥ 2 px from any ink) is paper in the master even
        // where instances were speckled.
        let mut speckled = 0;
        for y in 13..16_u32 {
            for x in 6..13_u32 {
                let (px, py) = at(prototype.anchor(), center, x, y);
                assert!(!prototype.binary().is_ink(px, py), "deep paper ({x},{y}) became ink");
                if glyphs.iter().any(|glyph| {
                    let [l, t, _, _] = glyph.component.bounds;
                    let index = ((t + y) * width + l + x) as usize;
                    ink.get(index).copied().unwrap_or(0) == 1
                }) {
                    speckled += 1;
                }
            }
        }
        assert!(speckled > 0, "the fixture must speckle the deep paper it asserts on");
        let paper = at(prototype.anchor(), center, 8, 14);
        let paper_index = paper.1 as usize * prototype.grey().width() as usize + paper.0 as usize;
        assert!(*prototype.grey().data().get(paper_index).unwrap_or(&0) > 200, "speckled paper averages back to paper");

        // Sharpness: row 14 crosses only the left stem, 3 px wide. Corner alignment would
        // stack the speck-shifted instance a pixel apart and smear the stem to 4 px; centre
        // of mass keeps the printed width at the printed position.
        let (stem_x, row_y) = at(prototype.anchor(), center, 0, 14);
        let mut runs: Vec<(i64, u32)> = Vec::new();
        let mut run = 0_u32;
        for x in 0..prototype.binary().width() {
            if prototype.binary().is_ink(i64::from(x), row_y) {
                run += 1;
            } else if run > 0 {
                runs.push((i64::from(x) - i64::from(run), run));
                run = 0;
            }
        }
        if run > 0 {
            runs.push((i64::from(prototype.binary().width()) - i64::from(run), run));
        }
        assert_eq!(runs, vec![(stem_x, 3)], "the stem stays 3 px at the printed position");
        let core = at(prototype.anchor(), center, 1, 14);
        let core_index = core.1 as usize * prototype.grey().width() as usize + core.0 as usize;
        assert!(*prototype.grey().data().get(core_index).unwrap_or(&255) < 80, "the ink core averaged to paper");
    }

    #[test]
    fn c_and_e_share_a_box_but_not_ink_and_never_cluster() {
        let (ink, grey, width, height) = page(&[E, C, E, C]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        // The adversary: the reader called them all 'e'. The shapes must disagree: aligned
        // by centre of mass the crossbar's missing weight tips 'c' out of register (the
        // exact metric drops it), and the crossbar alone is a fifth of the ink (the area
        // gate drops it too). Two independent gates, one verdict.
        let (e, c) = (&glyphs[0].component, &glyphs[1].component);
        assert_eq!((e.width(), e.height()), (c.width(), c.height()), "the fixture keeps the boxes alike");
        let scored = ink_iou(e, c);
        assert!(scored < ClusterParams::default().iou_min, "IoU {scored} must fall under the threshold");
        assert!(!within(e.area, c.area, ClusterParams::default().area_tolerance), "the ink areas must also disagree");
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert_eq!(clustered.clusters().len(), 2, "{:?}", clustered.clusters().iter().map(|c| c.members().to_vec()).collect::<Vec<_>>());
        assert_eq!(clustered.clusters()[0].members(), &[0, 2]);
        assert_eq!(clustered.clusters()[1].members(), &[1, 3]);
    }

    #[test]
    fn rn_and_m_stay_apart_under_one_reading() {
        let (ink, grey, width, height) = page(&[M, RN]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let mut glyphs = glyphs_of(&analyzed(ink, width, height));
        for glyph in &mut glyphs {
            glyph.role = MatchRole::Character { text: "m".into(), source: CharRef { line: 0, word: 0, index: Some(0) } };
        }
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert_eq!(clustered.clusters().len(), 2, "both read as 'm'; the shapes must not merge");
        assert!(clustered.clusters().iter().all(|cluster| cluster.prototype().is_some()));
    }

    #[test]
    fn a_heading_sized_glyph_is_not_the_body_texts() {
        let big = Shape { pattern: |x, y| is_e(x / 2, y / 2), width: 32, height: 40 };
        let (ink, grey, width, height) = page(&[E, E, big, big]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert_eq!(clustered.clusters().len(), 2);
        assert_eq!(clustered.clusters()[0].members(), &[0, 1]);
        assert_eq!(clustered.clusters()[1].members(), &[2, 3]);
    }

    #[test]
    fn slight_ink_bleed_still_averages_together() {
        // The same 'e' with one column of bleed: a pixel of size drift must not split a letter.
        let bleed = Shape { pattern: |x, y| is_e(x, y) || (x == 16 && (4..8).contains(&y)), width: 17, height: 20 };
        let (ink, grey, width, height) = page(&[E, bleed, E]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert_eq!(clustered.clusters().len(), 1);
        assert_eq!(clustered.clusters()[0].members().len(), 3);
    }

    #[test]
    fn a_split_glyphs_pieces_cluster_separately() {
        // An 'i' the labeling split: a 3 × 20 stem and a 3 × 3 dot, both read as 'i'.
        let stem = Shape { pattern: |x, y| x < 3 && y < 20, width: 3, height: 20 };
        let dot = Shape { pattern: |x, y| x < 3 && y < 3, width: 3, height: 3 };
        let (ink, grey, width, height) = page(&[stem, dot, stem, dot]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let mut glyphs = glyphs_of(&analyzed(ink, width, height));
        for glyph in &mut glyphs {
            glyph.role = MatchRole::Fragment { text: "i".into(), source: CharRef { line: 0, word: 0, index: Some(0) } };
        }
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert_eq!(clustered.clusters().len(), 2, "stems with stems, dots with dots");
        assert_eq!(clustered.clusters()[0].members(), &[0, 2]);
        assert_eq!(clustered.clusters()[1].members(), &[1, 3]);
    }

    #[test]
    fn a_singleton_occurrence_still_gets_a_prototype() {
        let (ink, grey, width, height) = page(&[E]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert_eq!(clustered.clusters().len(), 1);
        let prototype = clustered.clusters()[0].prototype().unwrap();
        assert_eq!(prototype.members(), 1);
        assert_eq!(prototype.binary().ink_count(), u64::from(glyphs[0].component.area));
        assert_eq!(prototype.ink_bounds(), Some([0, 0, 13, 20]), "the box is tight on ink: 13, not the 16-wide cell");
    }

    #[test]
    fn cluster_pages_averages_across_pages() {
        let (ink, grey, width, height) = page(&[E, E]);
        let first = GreyImage::new(width, height, grey).unwrap();
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        let (ink, grey, width, height) = page(&[E, E, E]);
        let second = GreyImage::new(width, height, grey).unwrap();
        let more = glyphs_of(&analyzed(ink, width, height));
        let clustered = cluster_pages(&[(&first, &glyphs), (&second, &more)], &ClusterParams::default());
        assert_eq!(clustered.clusters().len(), 1);
        assert_eq!(clustered.clusters()[0].members(), &[0, 1, 2, 3, 4]);
        assert_eq!(clustered.clusters()[0].prototype().unwrap().members(), 5);
    }

    // -- intake and limits -----------------------------------------------------

    #[test]
    fn glyphs_without_text_or_mask_are_skipped_not_dropped_silently() {
        let (ink, grey, width, height) = page(&[E, E]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let mut glyphs = glyphs_of(&analyzed(ink, width, height));
        glyphs[0].role = MatchRole::Unmatched;
        glyphs[1].component.mask = None;
        let clustered = cluster(&grey, &glyphs, &ClusterParams::default());
        assert_eq!(clustered.skipped(), &[(0, SkipReason::NoText), (1, SkipReason::NoMask)]);
        assert!(clustered.clusters().is_empty());
    }

    #[test]
    fn an_over_cap_canvas_keeps_its_cluster_but_not_its_prototype() {
        let (ink, grey, width, height) = page(&[E, M]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        // 'e' spans about 16 × 20 = 320 canvas pixels, 'm' about 24 × 20 = 480.
        let params = ClusterParams { max_prototype_pixels: 350, ..ClusterParams::default() };
        let clustered = cluster(&grey, &glyphs, &params);
        assert_eq!(clustered.clusters().len(), 2);
        assert!(clustered.clusters()[0].prototype().is_some(), "the smaller canvas fits the cap");
        assert!(clustered.clusters()[1].prototype().is_none(), "the larger canvas is over the cap");
        // The total budget spends the same way: small canvases first.
        let params = ClusterParams { max_total_prototype_pixels: 400, ..ClusterParams::default() };
        let clustered = cluster(&grey, &glyphs, &params);
        assert!(clustered.clusters()[0].prototype().is_some());
        assert!(clustered.clusters()[1].prototype().is_none());
    }

    #[test]
    fn a_page_with_no_greyscale_fails_soft_to_an_empty_master() {
        let mut ink = vec![0_u8; 16 * 20];
        for y in 0..20_u32 {
            for x in 0..16_u32 {
                if is_e(x, y) {
                    ink[(y * 16 + x) as usize] = 1;
                }
            }
        }
        let mask = Bitmap::from_ink(16, 20, ink).unwrap();
        let component = Component {
            bounds: [0, 0, 16, 20],
            area: mask.ink_count() as u32,
            centroid: [3.3, 20.6],
            mask: Some(mask),
            stroke_width: 3.0,
            kind: ComponentKind::Glyph,
        };
        let glyphs = vec![ResolvedGlyph {
            source: 0,
            component,
            role: MatchRole::Character { text: "e".into(), source: CharRef { line: 0, word: 0, index: Some(0) } },
            iou: 1.0,
        }];
        let empty = GreyImage::new(0, 0, Vec::new()).unwrap();
        let clustered = cluster(&empty, &glyphs, &ClusterParams::default());
        assert_eq!(clustered.clusters().len(), 1);
        let prototype = clustered.clusters()[0].prototype().unwrap();
        assert_eq!(prototype.binary().ink_count(), 0, "a white page averages to white");
        assert!(prototype.ink_bounds().is_none());
    }

    #[test]
    fn no_glyphs_is_no_clusters() {
        let grey = GreyImage::new(4, 4, vec![255; 16]).unwrap();
        assert_eq!(cluster(&grey, &[], &ClusterParams::default()), Clusters::default());
    }

    #[test]
    fn clustering_is_deterministic() {
        let (ink, grey, width, height) = page(&[E, C, E, M, C]);
        let grey = GreyImage::new(width, height, grey).unwrap();
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        let once = cluster(&grey, &glyphs, &ClusterParams::default());
        let again = cluster(&grey, &glyphs, &ClusterParams::default());
        assert_eq!(once, again);
    }

    // -- the pieces ------------------------------------------------------------

    #[test]
    fn the_similarity_hash_is_stable_and_coarse() {
        let draw = |shape: &Shape| {
            let mut ink = vec![0_u8; (shape.width * shape.height) as usize];
            for y in 0..shape.height {
                for x in 0..shape.width {
                    if (shape.pattern)(x, y) {
                        ink[(y * shape.width + x) as usize] = 1;
                    }
                }
            }
            Bitmap::from_ink(shape.width, shape.height, ink).unwrap()
        };
        let e = signature(&draw(&E));
        assert_eq!(e, signature(&draw(&E)), "the same mask hashes the same");
        assert!(e.iter().any(|word| *word != 0), "an inked mask sets bits");
        assert_eq!(signature(&Bitmap::from_ink(0, 0, Vec::new()).unwrap()), [0; SIGNATURE_WORDS]);
        let c = signature(&draw(&C));
        assert!(hamming(&e, &c) > 0 && hamming(&e, &c) <= 256, "c and e hash apart but within the pruning bound");
    }

    #[test]
    fn centroid_aligned_ink_iou_scores_identity_and_difference() {
        let (ink, _, width, height) = page(&[E, E, C]);
        let glyphs = glyphs_of(&analyzed(ink, width, height));
        assert_eq!(ink_iou(&glyphs[0].component, &glyphs[1].component), 1.0, "the same shape at different places scores 1");
        // 'c' inside 'e': every c-pixel overlaps, but the missing crossbar moves c's centre
        // of mass, so centroid alignment interleaves the two shapes instead of nesting them.
        let scored = ink_iou(&glyphs[0].component, &glyphs[2].component);
        assert!(scored > 0.3 && scored < ClusterParams::default().iou_min, "c vs e scores {scored}: lookalike, not match");
        // Masks gone (over the mask budget) never match anything.
        let mut bare = glyphs[0].clone();
        bare.component.mask = None;
        assert_eq!(ink_iou(&bare.component, &glyphs[1].component), 0.0);
    }
}
