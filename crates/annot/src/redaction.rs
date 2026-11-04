//! The one reading of a Redact annotation's area (ISO 32000-2 §12.5.6.23): what a preview shows
//! and what applying removes are both [`mark_areas`], so they can't disagree.
//!
//! The input is a dictionary already known to be a Redact annotation; telling Redact from other
//! subtypes, and whether a mark written in place can be applied, is the redaction collector's
//! business (`pdfcraft-redact`), not this module's.
//!
//! | `/QuadPoints`                                   | Areas, or refusal                                  |
//! |-------------------------------------------------|----------------------------------------------------|
//! | every group a usable quad                       | each quad's box; `/Rect` is not read                |
//! | some group unusable, or entries after the last  | the usable quads' boxes **and** a usable `/Rect`;   |
//! | whole group                                     | with no usable `/Rect` the whole mark is refused    |
//! | absent, empty, not an array, unresolvable       | a usable `/Rect`, else refused                      |
//! | more than [`MAX_QUADPOINT_VALUES`] entries      | refused, even with a usable `/Rect`                 |
//!
//! A quad is usable when its eight numbers are finite and its box has width and height; a
//! `/Rect` is usable when it is four finite numbers with width and height. Every chosen area is
//! then widened to the smallest `f32` rectangle containing it ([`pdfcraft_geom::rect_f32_cover`]),
//! so the screen (which draws in `f32`) and the removal (in `f64`) use the same numbers; an area
//! `f32` can't hold refuses the mark. Quads are never merged into one box: that would take the
//! gaps between marked lines with it.

use pdfcraft_cos::{Dict, Document};
use pdfcraft_model::numeric::{ArrayError, NumberReader, quad_groups, rect};

/// The most `/QuadPoints` entries one mark may have (8,192 whole quads).
pub const MAX_QUADPOINT_VALUES: usize = 65_536;

/// The checked areas of one Redact mark: exactly what a preview fills and what applying removes.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkAreas {
    /// The areas in user space, `[x0, y0, x1, y1]` with `x0 < x1`, `y0 < y1`, each exactly an
    /// `f32` rectangle: the usable quads' boxes in their order, then `/Rect` when it was used.
    pub rects: Vec<[f64; 4]>,
    /// `/Rect` is one of the areas (no quads, or quads that needed it).
    pub used_rect: bool,
    /// What was wrong with `/QuadPoints`, if anything.
    pub quad_issues: QuadIssues,
    /// Some area was widened to the nearest `f32` outside it (by less than one `f32` step per
    /// edge).
    pub widened: bool,
}

/// What was wrong with a mark's `/QuadPoints`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuadIssues {
    /// Groups of eight that are not a usable quad (a corner not a finite number, or no area).
    pub invalid_groups: usize,
    /// Entries after the last whole group of eight.
    pub trailing_values: usize,
    /// `/QuadPoints` is absent or an empty array.
    pub absent_or_empty: bool,
    /// `/QuadPoints` is not an array, or a reference that could not be followed.
    pub malformed_container: bool,
}

impl QuadIssues {
    /// Some quad could not be used, so `/Rect` had to be (or would have had to be) brought in.
    pub fn partial(&self) -> bool {
        self.invalid_groups > 0 || self.trailing_values > 0
    }
}

/// Why a mark has no area that can be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MarkAreaError {
    /// Neither a usable quad nor a usable `/Rect`.
    #[error("it has no readable area (neither usable QuadPoints nor a usable Rect)")]
    NoUsableArea,
    /// Some quads are usable but others are not, and there is no usable `/Rect` to cover them.
    #[error("some of its QuadPoints can't be read and it has no usable Rect to cover them")]
    QuadNeedsRect,
    /// `/QuadPoints` has more than [`MAX_QUADPOINT_VALUES`] entries.
    #[error("its QuadPoints have more than {MAX_QUADPOINT_VALUES} entries")]
    TooManyValues,
    #[error("there was not enough memory to read its area")]
    Allocation,
    /// An area too large to draw on screen exactly.
    #[error("its area is too large to show exactly")]
    UnrepresentableGeometry,
}

/// The area of the Redact annotation `annot` (see the module documentation for the rules).
pub fn mark_areas(doc: &Document, annot: &Dict) -> Result<MarkAreas, MarkAreaError> {
    let reader = NumberReader::new(doc);
    let mut issues = QuadIssues::default();
    let mut quads: Vec<[f64; 4]> = Vec::new();
    match annot.get(b"QuadPoints") {
        None => issues.absent_or_empty = true,
        Some(o) => match quad_groups(&reader, o, MAX_QUADPOINT_VALUES) {
            Ok(g) => {
                issues.absent_or_empty = g.groups.is_empty() && g.trailing == 0;
                issues.trailing_values = g.trailing;
                quads.try_reserve_exact(g.groups.len()).map_err(|_| MarkAreaError::Allocation)?;
                for group in &g.groups {
                    match group.as_ref().ok().and_then(pdfcraft_geom::finite_quad_bounds).filter(has_area) {
                        Some(b) => quads.push(b),
                        None => issues.invalid_groups = issues.invalid_groups.saturating_add(1),
                    }
                }
            }
            Err(ArrayError::TooLong { .. }) => return Err(MarkAreaError::TooManyValues),
            Err(ArrayError::Allocation) => return Err(MarkAreaError::Allocation),
            Err(ArrayError::NotArray | ArrayError::UnresolvedReference | ArrayError::WrongLength { .. } | ArrayError::InvalidEntry { .. }) => {
                issues.malformed_container = true;
            }
        },
    }
    let needs_rect = issues.partial() || quads.is_empty();
    let rect = if needs_rect { usable_rect(&reader, annot) } else { None };
    match (needs_rect, rect) {
        (false, _) => {}
        (true, Some(r)) => {
            quads.try_reserve_exact(1).map_err(|_| MarkAreaError::Allocation)?;
            quads.push(r);
        }
        (true, None) if !quads.is_empty() => return Err(MarkAreaError::QuadNeedsRect),
        (true, None) => return Err(MarkAreaError::NoUsableArea),
    }
    let mut widened = false;
    for area in &mut quads {
        let cover = pdfcraft_geom::rect_f32_cover(*area).ok_or(MarkAreaError::UnrepresentableGeometry)?;
        let cover = cover.map(f64::from);
        // Still an area once widened (it can only grow), but say so if it isn't.
        if !has_area(&cover) {
            return Err(MarkAreaError::UnrepresentableGeometry);
        }
        widened |= cover != *area;
        *area = cover;
    }
    Ok(MarkAreas { rects: quads, used_rect: needs_rect, quad_issues: issues, widened })
}

/// Where a mark is, for pointing at it when it can't be applied: its `/Rect` when that is four
/// finite numbers with an area `f32` can hold (widened to it), else `None`. Never an area that
/// applying would remove.
pub fn mark_locator(doc: &Document, annot: &Dict) -> Option<[f64; 4]> {
    let r = usable_rect(&NumberReader::new(doc), annot)?;
    pdfcraft_geom::rect_f32_cover(r).map(|c| c.map(f64::from))
}

/// `/Rect` when it is four finite numbers with width and height.
fn usable_rect(reader: &NumberReader<'_>, annot: &Dict) -> Option<[f64; 4]> {
    rect(reader, annot.get(b"Rect")?).ok().filter(has_area)
}

/// A normalized box with finite, positive width and height.
fn has_area(b: &[f64; 4]) -> bool {
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0
}

#[cfg(test)]
mod tests;
