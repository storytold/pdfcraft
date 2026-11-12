//! OCR boxes lined up with connected components (ClearScan stage 2).
//!
//! A glyph's box is tight on the ink. The recognizer's box is the character cell, often a
//! little larger and sometimes a little shifted, so a pure intersection-over-union test misses
//! a thin stem sitting inside a padded cell. [`iou`] is the score that is recorded; a pair
//! also matches when most of the blob sits inside the cell, or most of the cell sits inside
//! the blob (one component swallowing several character boxes — ink bleed).
//!
//! Touching letters come out of labeling as one blob. [`TouchingStrategy::Ligature`] keeps
//! that blob and records every character it covers, in left-to-right order, so a later stage
//! can build one multi-character glyph. [`TouchingStrategy::VerticalCut`] slices the blob on
//! the boundaries between those character boxes. When the boxes do not separate — a single
//! word box around "fi", or two cells that overlap by more than half — the slice falls back
//! to the lightest column of the horizontal projection, which on a bridged pair is the bridge.

use crate::annotations::{AnnotatedLine, CharBox};
use crate::components::{Component, ComponentKind, ComponentSet, component_from_pixels};

/// Intersection over union of two half-open boxes `[left, top, right, bottom]`.
///
/// A non-finite, inverted, or empty box scores 0. Identical boxes score 1.
pub fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let inter = intersection_area(a, b);
    let union = box_area(a) + box_area(b) - inter;
    if union <= 0.0 || !union.is_finite() { 0.0 } else { (inter / union).clamp(0.0, 1.0) }
}

fn intersection_area(a: [f32; 4], b: [f32; 4]) -> f32 {
    if a.iter().any(|value| !value.is_finite()) || b.iter().any(|value| !value.is_finite()) {
        return 0.0;
    }
    let width = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let height = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    width * height
}

fn box_area(rect: [f32; 4]) -> f32 {
    if rect.iter().any(|value| !value.is_finite()) {
        return 0.0;
    }
    let width = rect[2] - rect[0];
    let height = rect[3] - rect[1];
    if width <= 0.0 || height <= 0.0 { 0.0 } else { width * height }
}

/// What to do with one blob that covers several characters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TouchingStrategy {
    /// Keep the blob whole and record the combined character string.
    #[default]
    Ligature,
    /// Slice the blob on vertical lines at the character boundaries.
    VerticalCut,
}

/// The knobs of [`align`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlignParams {
    /// Minimum intersection-over-union for a match on its own.
    pub min_iou: f32,
    /// A blob also matches when at least this fraction of its box lies inside the character.
    pub min_component_coverage: f32,
    /// A blob also matches when at least this fraction of the character box lies inside it.
    pub min_char_coverage: f32,
    pub touching: TouchingStrategy,
}

impl Default for AlignParams {
    fn default() -> Self {
        AlignParams { min_iou: 0.15, min_component_coverage: 0.5, min_char_coverage: 0.5, touching: TouchingStrategy::Ligature }
    }
}

/// Where a matched box came from in the annotated lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharRef {
    pub line: usize,
    pub word: usize,
    /// Index into the word's characters, or `None` when the box is the whole word.
    pub index: Option<usize>,
}

/// How a blob relates to the text that claimed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchRole {
    /// One blob, one character.
    Character { text: String, source: CharRef },
    /// One blob, several characters (ink bleed), or one word box holding several characters
    /// that was kept whole. `text` is left to right.
    Ligature { text: String, sources: Vec<CharRef> },
    /// One character, one piece of a glyph the labeling split (the stem and the tittle).
    Fragment { text: String, source: CharRef },
    /// A piece a cut produced and no character box claimed.
    Unmatched,
}

/// A glyph after alignment. A vertical cut owns its piece; otherwise `component` is the blob
/// labeling found.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedGlyph {
    /// Index into [`ComponentSet::components`] the ink came from.
    pub source: usize,
    pub component: Component,
    pub role: MatchRole,
    /// Best intersection-over-union against the boxes in `role`.
    pub iou: f32,
}

/// The lining-up of one page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageAlignment {
    pub matched: Vec<ResolvedGlyph>,
    /// Glyph blobs no character box claimed.
    pub unmatched_glyphs: Vec<usize>,
    /// Character and word boxes no glyph claimed.
    pub unmatched_text: Vec<CharRef>,
}

struct InkBox {
    rect: [f32; 4],
    text: String,
    source: CharRef,
}

struct Links {
    by_component: Vec<Vec<(usize, f32)>>,
    by_box: Vec<Vec<usize>>,
}

struct Slot {
    rect: [f32; 4],
    text: String,
    source: CharRef,
}

/// Line `lines` up with `components`. Only [`ComponentKind::Glyph`] blobs are candidates;
/// speckles, rules and photographs stay out of the text even when a box covers them.
pub fn align(components: &ComponentSet, lines: &[AnnotatedLine], params: &AlignParams) -> PageAlignment {
    let boxes = collect_boxes(lines);
    let links = link_glyphs(components, &boxes, params);
    let mut matched = Vec::new();
    let mut unmatched_glyphs = Vec::new();
    for (index, component) in components.components().iter().enumerate() {
        if component.kind != ComponentKind::Glyph {
            continue;
        }
        let Some(of_component) = links.by_component.get(index) else {
            continue;
        };
        if of_component.is_empty() {
            unmatched_glyphs.push(index);
            continue;
        }
        matched.extend(resolve(index, component, of_component, &boxes, &links, components, params));
    }
    let unmatched_text = boxes
        .iter()
        .enumerate()
        .filter(|(index, _)| links.by_box.get(*index).map(Vec::is_empty).unwrap_or(true))
        .map(|(_, ink)| ink.source)
        .collect();
    PageAlignment { matched, unmatched_glyphs, unmatched_text }
}

fn collect_boxes(lines: &[AnnotatedLine]) -> Vec<InkBox> {
    let mut boxes = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        for (word_index, word) in line.words.iter().enumerate() {
            if word.chars.is_empty() {
                push_box(&mut boxes, word.rect, word.text.clone(), CharRef { line: line_index, word: word_index, index: None });
                continue;
            }
            for (char_index, ch) in word.chars.iter().enumerate() {
                push_char(&mut boxes, ch, CharRef { line: line_index, word: word_index, index: Some(char_index) });
            }
        }
    }
    boxes
}

fn push_char(boxes: &mut Vec<InkBox>, ch: &CharBox, source: CharRef) {
    push_box(boxes, ch.rect, ch.char.to_string(), source);
}

fn push_box(boxes: &mut Vec<InkBox>, rect: [f32; 4], text: String, source: CharRef) {
    if text.is_empty() || box_area(rect) <= 0.0 {
        return;
    }
    boxes.push(InkBox { rect, text, source });
}

fn link_glyphs(components: &ComponentSet, boxes: &[InkBox], params: &AlignParams) -> Links {
    let mut links = Links { by_component: vec![Vec::new(); components.components().len()], by_box: vec![Vec::new(); boxes.len()] };
    for (index, component) in components.components().iter().enumerate() {
        if component.kind != ComponentKind::Glyph {
            continue;
        }
        for (box_index, ink) in boxes.iter().enumerate() {
            let Some(score) = match_score(component, ink.rect, params) else {
                continue;
            };
            if let Some(of_component) = links.by_component.get_mut(index) {
                of_component.push((box_index, score));
            }
            if let Some(of_box) = links.by_box.get_mut(box_index) {
                of_box.push(index);
            }
        }
    }
    links
}

fn match_score(component: &Component, rect: [f32; 4], params: &AlignParams) -> Option<f32> {
    let blob = component.rect();
    let inter = intersection_area(blob, rect);
    let blob_area = box_area(blob);
    let char_area = box_area(rect);
    if inter <= 0.0 || blob_area <= 0.0 || char_area <= 0.0 {
        return None;
    }
    let score = iou(blob, rect);
    let covers_blob = inter / blob_area >= params.min_component_coverage;
    let covers_char = inter / char_area >= params.min_char_coverage;
    (score >= params.min_iou || covers_blob || covers_char).then_some(score)
}

fn resolve(
    index: usize,
    component: &Component,
    of_component: &[(usize, f32)],
    boxes: &[InkBox],
    links: &Links,
    set: &ComponentSet,
    params: &AlignParams,
) -> Vec<ResolvedGlyph> {
    if !is_fused(of_component, boxes) {
        return vec![resolve_one(index, component, of_component, boxes, links)];
    }
    match params.touching {
        TouchingStrategy::Ligature => vec![ligature(index, component, of_component, boxes)],
        TouchingStrategy::VerticalCut => cut(index, component, of_component, boxes, set),
    }
}

/// One blob over several characters, or one box whose text is already several characters.
fn is_fused(of_component: &[(usize, f32)], boxes: &[InkBox]) -> bool {
    of_component.len() >= 2 || piece_target(of_component, boxes) >= 2
}

fn piece_target(of_component: &[(usize, f32)], boxes: &[InkBox]) -> usize {
    of_component.iter().map(|(index, _)| boxes.get(*index).map(|ink| ink.text.chars().count()).unwrap_or(0)).sum()
}

fn resolve_one(index: usize, component: &Component, of_component: &[(usize, f32)], boxes: &[InkBox], links: &Links) -> ResolvedGlyph {
    let (box_index, score) = of_component.first().copied().unwrap_or((0, 0.0));
    let text = boxes.get(box_index).map(|ink| ink.text.clone()).unwrap_or_default();
    let source = boxes.get(box_index).map(|ink| ink.source).unwrap_or(CharRef { line: 0, word: 0, index: None });
    let owners = links.by_box.get(box_index).map(Vec::len).unwrap_or(0);
    let role = if owners >= 2 { MatchRole::Fragment { text, source } } else { MatchRole::Character { text, source } };
    ResolvedGlyph { source: index, component: component.clone(), role, iou: score }
}

fn ligature(index: usize, component: &Component, of_component: &[(usize, f32)], boxes: &[InkBox]) -> ResolvedGlyph {
    let ordered = ordered(of_component, boxes);
    let text = joined_text(&ordered, boxes);
    let sources = sources_of(&ordered, boxes);
    let score = ordered.iter().map(|(_, score)| *score).fold(0.0_f32, f32::max);
    ResolvedGlyph { source: index, component: component.clone(), role: MatchRole::Ligature { text, sources }, iou: score }
}

fn cut(index: usize, component: &Component, of_component: &[(usize, f32)], boxes: &[InkBox], set: &ComponentSet) -> Vec<ResolvedGlyph> {
    let ordered = ordered(of_component, boxes);
    let rects: Vec<[f32; 4]> = ordered.iter().filter_map(|(box_index, _)| boxes.get(*box_index).map(|ink| ink.rect)).collect();
    let mut cuts = boundary_cuts(component, &rects);
    let wanted = piece_target(&ordered, boxes);
    if cuts.len().saturating_add(1) < wanted {
        cuts = valley_cuts(component, wanted, &cuts);
    }
    let pieces = split(component, &cuts, set);
    if pieces.len() < 2 {
        return vec![ligature(index, component, of_component, boxes)];
    }
    let slots = expand_slots(&ordered, boxes);
    let mut resolved = if slots_overlap(&slots) { assign_in_order(index, pieces, &slots) } else { assign_by_iou(index, pieces, &slots) };
    resolved.sort_by(|a, b| a.component.bounds[0].cmp(&b.component.bounds[0]).then(a.component.bounds[1].cmp(&b.component.bounds[1])));
    resolved
}

fn ordered(of_component: &[(usize, f32)], boxes: &[InkBox]) -> Vec<(usize, f32)> {
    let mut links = of_component.to_vec();
    links.sort_by(|a, b| {
        let left = |index: usize| boxes.get(index).map(|ink| ink.rect[0]).unwrap_or(0.0);
        let top = |index: usize| boxes.get(index).map(|ink| ink.rect[1]).unwrap_or(0.0);
        left(a.0).total_cmp(&left(b.0)).then(top(a.0).total_cmp(&top(b.0))).then(a.0.cmp(&b.0))
    });
    links
}

fn joined_text(links: &[(usize, f32)], boxes: &[InkBox]) -> String {
    let mut text = String::new();
    for &(index, _) in links {
        if let Some(ink) = boxes.get(index) {
            text.push_str(&ink.text);
        }
    }
    text
}

fn sources_of(links: &[(usize, f32)], boxes: &[InkBox]) -> Vec<CharRef> {
    links.iter().filter_map(|(index, _)| boxes.get(*index).map(|ink| ink.source)).collect()
}

fn expand_slots(links: &[(usize, f32)], boxes: &[InkBox]) -> Vec<Slot> {
    let mut slots = Vec::new();
    for &(index, _) in links {
        let Some(ink) = boxes.get(index) else {
            continue;
        };
        let chars: Vec<char> = ink.text.chars().collect();
        if chars.len() <= 1 {
            slots.push(Slot { rect: ink.rect, text: ink.text.clone(), source: ink.source });
            continue;
        }
        for ch in chars {
            slots.push(Slot { rect: ink.rect, text: ch.to_string(), source: ink.source });
        }
    }
    slots
}

/// Character cells that share most of their width cannot host a boundary cut; the projection
/// valley does that job, and the pieces are handed out left to right.
fn slots_overlap(slots: &[Slot]) -> bool {
    slots.iter().enumerate().any(|(left_index, left)| {
        slots.iter().skip(left_index.saturating_add(1)).any(|right| {
            let overlap = (left.rect[2].min(right.rect[2]) - left.rect[0].max(right.rect[0])).max(0.0);
            let narrower = (left.rect[2] - left.rect[0]).min(right.rect[2] - right.rect[0]).max(0.0);
            narrower > 0.0 && overlap > narrower * 0.5
        })
    })
}

fn boundary_cuts(component: &Component, rects: &[[f32; 4]]) -> Vec<u32> {
    let mut cuts = Vec::new();
    for pair in rects.windows(2) {
        let Some((left, right)) = pair.first().copied().zip(pair.get(1).copied()) else {
            continue;
        };
        if vertical_overlap(left, right) <= 0.0 {
            continue;
        }
        let Some(at) = boundary_between(left, right) else {
            continue;
        };
        if at > component.bounds[0] && at < component.bounds[2] {
            cuts.push(at);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

fn vertical_overlap(left: [f32; 4], right: [f32; 4]) -> f32 {
    left[3].min(right[3]) - left[1].max(right[1])
}

/// The x of the cut, or `None` when the cells overlap too much for a boundary to mean anything.
fn boundary_between(left: [f32; 4], right: [f32; 4]) -> Option<u32> {
    let overlap = left[2].min(right[2]) - left[0].max(right[0]);
    let narrower = (left[2] - left[0]).min(right[2] - right[0]).max(0.0);
    let x = if left[2] <= right[0] {
        (left[2] + right[0]) * 0.5
    } else if narrower > 0.0 && overlap > narrower * 0.5 {
        return None;
    } else if overlap > 0.0 {
        (left[0].max(right[0]) + left[2].min(right[2])) * 0.5
    } else {
        return None;
    };
    finite_u32(x)
}

fn finite_u32(value: f32) -> Option<u32> {
    if !value.is_finite() || value < 0.0 || value >= u32::MAX as f32 { None } else { Some(value.round() as u32) }
}

fn valley_cuts(component: &Component, pieces: usize, existing: &[u32]) -> Vec<u32> {
    let mut cuts = existing.to_vec();
    let Some(mask) = component.mask.as_ref() else {
        return cuts;
    };
    let columns = column_ink(mask);
    while cuts.len().saturating_add(1) < pieces {
        let Some(local) = lowest_column(&columns, component.bounds[0], &cuts) else {
            break;
        };
        cuts.push(component.bounds[0].saturating_add(local));
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

fn column_ink(mask: &crate::binarize::Bitmap) -> Vec<u32> {
    let mut columns = vec![0_u32; mask.width() as usize];
    for x in 0..mask.width() {
        let mut ink = 0_u32;
        for y in 0..mask.height() {
            if mask.is_ink(i64::from(x), i64::from(y)) {
                ink = ink.saturating_add(1);
            }
        }
        if let Some(slot) = columns.get_mut(x as usize) {
            *slot = ink;
        }
    }
    columns
}

fn lowest_column(columns: &[u32], origin: u32, cuts: &[u32]) -> Option<u32> {
    let mut best: Option<(u32, u32)> = None;
    for local in 1..columns.len() as u32 {
        let absolute = origin.saturating_add(local);
        if cuts.iter().any(|&cut| cut.abs_diff(absolute) < 2) {
            continue;
        }
        let ink = columns.get(local as usize).copied().unwrap_or(u32::MAX);
        if best.is_none_or(|(best_ink, _)| ink < best_ink) {
            best = Some((ink, local));
        }
    }
    best.map(|(_, local)| local)
}

fn split(component: &Component, cuts: &[u32], set: &ComponentSet) -> Vec<Component> {
    let cuts = interior_cuts(component, cuts);
    if cuts.is_empty() {
        return Vec::new();
    }
    let mut bands = vec![Vec::new(); cuts.len().saturating_add(1)];
    for (x, y) in pixels_of(component) {
        let band = cuts.iter().position(|&cut| x < cut).unwrap_or(cuts.len());
        if let Some(slot) = bands.get_mut(band) {
            slot.push((x, y));
        }
    }
    bands
        .into_iter()
        .filter_map(|pixels| component_from_pixels(&pixels, set.width(), set.height(), set.body_size(), set.filter()))
        .filter(|piece| piece.kind == ComponentKind::Glyph && piece.area > 0)
        .collect()
}

fn interior_cuts(component: &Component, cuts: &[u32]) -> Vec<u32> {
    let mut cuts: Vec<u32> = cuts.iter().copied().filter(|&cut| cut > component.bounds[0] && cut < component.bounds[2]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

fn pixels_of(component: &Component) -> Vec<(u32, u32)> {
    let Some(mask) = &component.mask else {
        return Vec::new();
    };
    let mut pixels = Vec::new();
    for y in 0..mask.height() {
        for x in 0..mask.width() {
            if mask.is_ink(i64::from(x), i64::from(y)) {
                pixels.push((component.bounds[0].saturating_add(x), component.bounds[1].saturating_add(y)));
            }
        }
    }
    pixels
}

fn assign_in_order(source: usize, mut pieces: Vec<Component>, slots: &[Slot]) -> Vec<ResolvedGlyph> {
    pieces.sort_by(|a, b| a.bounds[0].cmp(&b.bounds[0]).then(a.bounds[1].cmp(&b.bounds[1])));
    let shared = pieces.len() != slots.len();
    pieces
        .into_iter()
        .enumerate()
        .map(|(index, component)| {
            let slot = slots.get(index).or(slots.last());
            let score = slot.map(|slot| iou(component.rect(), slot.rect)).unwrap_or(0.0);
            let role = slot.map(|slot| role_for(shared, &slot.text, slot.source)).unwrap_or(MatchRole::Unmatched);
            ResolvedGlyph { source, component, role, iou: score }
        })
        .collect()
}

fn role_for(shared: bool, text: &str, source: CharRef) -> MatchRole {
    if shared { MatchRole::Fragment { text: text.to_string(), source } } else { MatchRole::Character { text: text.to_string(), source } }
}

fn assign_by_iou(source: usize, pieces: Vec<Component>, slots: &[Slot]) -> Vec<ResolvedGlyph> {
    let assignment = greedy(pieces.as_slice(), slots);
    let mut per_slot = vec![0_u32; slots.len()];
    for (slot_index, _) in assignment.iter().flatten() {
        if let Some(count) = per_slot.get_mut(*slot_index) {
            *count = count.saturating_add(1);
        }
    }
    pieces
        .into_iter()
        .enumerate()
        .map(|(index, component)| {
            let Some((slot_index, score)) = assignment.get(index).copied().flatten() else {
                return ResolvedGlyph { source, component, role: MatchRole::Unmatched, iou: 0.0 };
            };
            let Some(slot) = slots.get(slot_index) else {
                return ResolvedGlyph { source, component, role: MatchRole::Unmatched, iou: score };
            };
            let shared = per_slot.get(slot_index).copied().unwrap_or(0) > 1;
            ResolvedGlyph { source, component, role: role_for(shared, &slot.text, slot.source), iou: score }
        })
        .collect()
}

/// Unique pairs, highest intersection-over-union first. A piece left over takes its best slot
/// anyway, and [`assign_by_iou`] marks that slot's pieces as fragments.
fn greedy(pieces: &[Component], slots: &[Slot]) -> Vec<Option<(usize, f32)>> {
    let mut pairs = Vec::new();
    for (piece_index, piece) in pieces.iter().enumerate() {
        for (slot_index, slot) in slots.iter().enumerate() {
            pairs.push((iou(piece.rect(), slot.rect), piece_index, slot_index));
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut assignment = vec![None; pieces.len()];
    let mut used = vec![false; slots.len()];
    for (score, piece_index, slot_index) in pairs {
        let Some(slot_free) = used.get(slot_index).copied() else {
            continue;
        };
        if slot_free || assignment.get(piece_index).and_then(|slot| *slot).is_some() {
            continue;
        }
        if let Some(slot) = assignment.get_mut(piece_index) {
            *slot = Some((slot_index, score));
        }
        if let Some(flag) = used.get_mut(slot_index) {
            *flag = true;
        }
    }
    for (piece_index, piece) in pieces.iter().enumerate() {
        if assignment.get(piece_index).and_then(|slot| *slot).is_some() {
            continue;
        }
        let best = best_slot(piece, slots);
        if let Some(slot) = assignment.get_mut(piece_index) {
            *slot = best;
        }
    }
    assignment
}

fn best_slot(piece: &Component, slots: &[Slot]) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (index, slot) in slots.iter().enumerate() {
        let score = iou(piece.rect(), slot.rect);
        if best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((index, score));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::{AnnotatedWord, CharBox};
    use crate::binarize::Bitmap;
    use crate::components::{AnalyzeOptions, ComponentKind, FilterParams, analyze};

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

    fn line(words: Vec<AnnotatedWord>) -> AnnotatedLine {
        AnnotatedLine { words, ..AnnotatedLine::default() }
    }

    fn word(text: &str, rect: [f32; 4], chars: Vec<CharBox>) -> AnnotatedWord {
        AnnotatedWord { text: text.into(), rect, chars }
    }

    fn ch(c: char, rect: [f32; 4]) -> CharBox {
        CharBox { char: c, rect }
    }

    /// Two stems joined by a two-pixel bridge. One blob, area 106, on an 80×80 page so the
    /// blob is a glyph and not a page-sized photograph.
    fn bridged() -> crate::components::ComponentSet {
        let image = bitmap(80, 80, |x, y| {
            let left = (4..7).contains(&x) && (4..20).contains(&y);
            let right = (12..15).contains(&x) && (4..20).contains(&y);
            let bridge = (7..12).contains(&x) && (10..12).contains(&y);
            left || right || bridge
        });
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        assert_eq!(set.components().len(), 1);
        assert_eq!(set.components()[0].kind, ComponentKind::Glyph);
        assert_eq!(set.components()[0].area, 106);
        set
    }

    fn touching_lines() -> Vec<AnnotatedLine> {
        // 'i' is listed first and drawn on the right, so a joiner that trusts vec order yields "if".
        vec![line(vec![word("if", [4.0, 4.0, 15.0, 20.0], vec![ch('i', [12.0, 4.0, 15.0, 20.0]), ch('f', [4.0, 4.0, 8.0, 20.0])])])]
    }

    #[test]
    fn iou_is_one_a_third_and_zero_and_hostile_boxes_score_zero() {
        let full = [0.0, 0.0, 10.0, 10.0];
        assert_eq!(iou(full, full), 1.0);
        assert_eq!(iou(full, [10.0, 0.0, 20.0, 10.0]), 0.0);
        let partial = iou(full, [5.0, 0.0, 15.0, 10.0]);
        assert!((partial - 1.0 / 3.0).abs() < 1e-5, "{partial}");
        assert_eq!(iou([5.0, 5.0, 1.0, 1.0], full), 0.0);
        assert_eq!(iou(full, [0.0, 0.0, 10.0, f32::NAN]), 0.0);
        assert_eq!(iou(full, [0.0, f32::INFINITY, 10.0, 10.0]), 0.0);
    }

    #[test]
    fn a_stem_matches_its_cell_and_a_padded_cell_by_containment() {
        let image = bitmap(80, 80, |x, y| (10..13).contains(&x) && (10..30).contains(&y));
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        assert_eq!(set.components()[0].kind, ComponentKind::Glyph);
        let exact = [10.0, 10.0, 13.0, 30.0];
        let aligned = align(&set, &[line(vec![word("l", exact, vec![ch('l', exact)])])], &AlignParams::default());
        assert_eq!(aligned.matched.len(), 1);
        assert_eq!(aligned.matched[0].iou, 1.0);
        assert_eq!(aligned.matched[0].role, MatchRole::Character { text: "l".into(), source: CharRef { line: 0, word: 0, index: Some(0) } });
        let padded = [7.0, 7.0, 20.0, 34.0];
        let aligned = align(&set, &[line(vec![word("l", padded, vec![ch('l', padded)])])], &AlignParams::default());
        assert_eq!(aligned.matched.len(), 1);
        assert!(aligned.matched[0].iou < 1.0 && aligned.matched[0].iou > 0.1, "{}", aligned.matched[0].iou);
        assert!(matches!(aligned.matched[0].role, MatchRole::Character { ref text, .. } if text == "l"));
    }

    #[test]
    fn a_speckle_inside_a_cell_is_not_a_character_and_an_empty_cell_stays_unmatched() {
        let image = bitmap(80, 80, |x, y| ((10..13).contains(&x) && (10..30).contains(&y)) || (x == 40 && y == 12));
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        assert_eq!(set.components().iter().filter(|component| component.kind == ComponentKind::Noise).count(), 1);
        let cell = [8.0, 8.0, 50.0, 32.0];
        let elsewhere = [60.0, 0.0, 70.0, 10.0];
        let nan = [0.0, 0.0, 10.0, f32::NAN];
        let lines = vec![line(vec![word("l?", cell, vec![ch('l', cell), ch('?', elsewhere), ch('?', nan)])])];
        let aligned = align(&set, &lines, &AlignParams::default());
        assert_eq!(aligned.matched.len(), 1);
        assert_eq!(aligned.matched[0].component.area, 60);
        assert!(aligned.unmatched_glyphs.is_empty());
        assert_eq!(aligned.unmatched_text.len(), 1);
        assert_eq!(aligned.unmatched_text[0].index, Some(1));
    }

    #[test]
    fn a_rule_under_a_character_box_is_not_text() {
        let image = bitmap(100, 40, |x, y| (10..70).contains(&x) && (20..22).contains(&y));
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        assert_eq!(set.components()[0].kind, ComponentKind::Graphic);
        let cell = [10.0, 18.0, 70.0, 24.0];
        let aligned = align(&set, &[line(vec![word("-", cell, vec![ch('-', cell)])])], &AlignParams::default());
        assert!(aligned.matched.is_empty());
        assert_eq!(aligned.unmatched_text.len(), 1);
    }

    #[test]
    fn touching_letters_stay_one_ligature_in_reading_order() {
        let set = bridged();
        let aligned = align(&set, &touching_lines(), &AlignParams::default());
        assert_eq!(aligned.matched.len(), 1);
        let glyph = &aligned.matched[0];
        assert_eq!(glyph.component.area, 106);
        assert!(glyph.iou > 0.3, "{}", glyph.iou);
        assert_eq!(
            glyph.role,
            MatchRole::Ligature {
                text: "fi".into(),
                sources: vec![CharRef { line: 0, word: 0, index: Some(1) }, CharRef { line: 0, word: 0, index: Some(0) }],
            }
        );
    }

    #[test]
    fn touching_letters_split_on_the_character_boundary_without_losing_ink() {
        let set = bridged();
        let params = AlignParams { touching: TouchingStrategy::VerticalCut, ..AlignParams::default() };
        let aligned = align(&set, &touching_lines(), &params);
        assert_eq!(aligned.matched.len(), 2, "{:?}", roles(&aligned));
        let texts: Vec<_> = aligned.matched.iter().map(|glyph| role_text(&glyph.role)).collect();
        assert_eq!(texts, ["f", "i"]);
        assert!(aligned.matched.iter().all(|glyph| matches!(glyph.role, MatchRole::Character { .. })));
        let areas: Vec<u32> = aligned.matched.iter().map(|glyph| glyph.component.area).collect();
        assert_eq!(areas, [54, 52], "the midpoint cut keeps the bridge");
        assert!(aligned.matched[0].component.bounds[2] <= 10);
        assert!(aligned.matched[1].component.bounds[0] >= 10);
        assert_eq!(areas.iter().sum::<u32>(), 106);
    }

    #[test]
    fn a_word_box_splits_on_the_projection_valley_between_the_stems() {
        let set = bridged();
        let params = AlignParams { touching: TouchingStrategy::VerticalCut, ..AlignParams::default() };
        let lines = vec![line(vec![word("fi", [4.0, 4.0, 15.0, 20.0], Vec::new())])];
        let aligned = align(&set, &lines, &params);
        assert_eq!(aligned.matched.len(), 2, "{:?}", roles(&aligned));
        let texts: Vec<_> = aligned.matched.iter().map(|glyph| role_text(&glyph.role)).collect();
        assert_eq!(texts, ["f", "i"]);
        let areas: Vec<u32> = aligned.matched.iter().map(|glyph| glyph.component.area).collect();
        assert_eq!(areas, [48, 58], "the valley is the bridge, so the left stem is whole");
        assert_eq!(areas.iter().sum::<u32>(), 106);
        assert!(aligned.matched.iter().all(|glyph| matches!(glyph.role, MatchRole::Character { source: CharRef { index: None, .. }, .. })));
    }

    #[test]
    fn a_stem_and_a_dot_inside_one_cell_are_fragments() {
        let image = bitmap(80, 40, |x, y| {
            let letter = (10..26).contains(&x)
                && (10..30).contains(&y)
                && ((10..13).contains(&x) || (23..26).contains(&x) || ((19..22).contains(&y) && (13..23).contains(&x)));
            let dot = (50..53).contains(&x) && (6..9).contains(&y);
            let stem = (50..53).contains(&x) && (14..30).contains(&y);
            letter || dot || stem
        });
        let set = analyze(&image, &AnalyzeOptions::default()).unwrap();
        assert_eq!(set.components().iter().filter(|component| component.kind == ComponentKind::Glyph).count(), 3);
        let cell = [48.0, 4.0, 58.0, 32.0];
        let aligned = align(&set, &[line(vec![word("i", cell, vec![ch('i', cell)])])], &AlignParams::default());
        assert_eq!(aligned.matched.len(), 2, "{:?}", roles(&aligned));
        assert!(
            aligned
                .matched
                .iter()
                .all(|glyph| glyph.role == MatchRole::Fragment { text: "i".into(), source: CharRef { line: 0, word: 0, index: Some(0) } })
        );
        let mut areas: Vec<u32> = aligned.matched.iter().map(|glyph| glyph.component.area).collect();
        areas.sort_unstable();
        assert_eq!(areas, [9, 48]);
        assert_eq!(aligned.unmatched_glyphs.len(), 1);
    }

    #[test]
    fn an_empty_page_aligns_to_nothing() {
        let image = Bitmap::from_ink(8, 8, vec![0; 64]).unwrap();
        let set = analyze(&image, &AnalyzeOptions { filter: FilterParams::default(), ..AnalyzeOptions::default() }).unwrap();
        let aligned = align(&set, &[], &AlignParams::default());
        assert!(aligned.matched.is_empty() && aligned.unmatched_glyphs.is_empty() && aligned.unmatched_text.is_empty());
    }

    fn roles(aligned: &PageAlignment) -> Vec<&MatchRole> {
        aligned.matched.iter().map(|glyph| &glyph.role).collect()
    }

    fn role_text(role: &MatchRole) -> &str {
        match role {
            MatchRole::Character { text, .. } | MatchRole::Fragment { text, .. } => text,
            MatchRole::Ligature { text, .. } => text,
            MatchRole::Unmatched => "",
        }
    }
}
