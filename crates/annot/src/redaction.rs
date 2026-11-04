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

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
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

/// How a Redact mark is drawn in a display-only copy of the document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shown<'a> {
    /// The outline of each area applying removes.
    Ready(&'a [[f64; 4]]),
    /// A mark that can't be applied: a warning outline at its locator, or nothing on the page
    /// when it has none.
    Refused(Option<[f64; 4]>),
}

/// The annotation flags a display copy clears so a mark is drawn where it is, at its size, and
/// at all: Invisible, Hidden, NoZoom, NoRotate, NoView (ISO 32000-2 Table 167).
const DISPLAY_CLEARED_FLAGS: i64 = 1 | 2 | 8 | 16 | 32;

/// What an outline appearance draws: the areas, the stroke colour and a dash pattern.
type Outline<'a> = (&'a [[f64; 4]], [f64; 3], &'static str);

/// What a display copy replaces or drops of a mark's dictionary, so they are never copied: the
/// appearance and its state, the quads it draws, optional-content membership, flags and `/Rect`.
const DISPLAY_REPLACED_KEYS: [&[u8]; 6] = [b"QuadPoints", b"AP", b"AS", b"OC", b"F", b"Rect"];

/// Draw the Redact marks `marks` (each its `/Annots` slot and how it is shown) of page `page`
/// (object `page_obj`) of `display`, a display-only copy, exactly as shown: each mark's normal
/// appearance becomes the outline of its checked areas (or a warning locator), its `/Rect` and
/// the appearance's `/BBox` the union of them (placed with no scaling, in the page's own user
/// space); its other appearances, `/AS`, `/OC` and the flags that would hide, scale or turn it
/// are dropped. The page is read once: its `/Annots` is copied only when a mark written in place
/// or listed through a chain has to be replaced in it. The marks' dictionaries in the document the
/// copy was made from are not touched; this is a view, never saved.
pub fn show_page_in_display(display: &mut Document, page: usize, page_obj: ObjRef, marks: &[(usize, Shown<'_>)]) -> Result<(), crate::AnnotError> {
    if marks.is_empty() {
        return Ok(());
    }
    let page_held = display.get(page_obj);
    let list_held;
    let list: &[Object] = match page_held.as_dict().and_then(|d| d.get(b"Annots")) {
        Some(Object::Array(a)) => a,
        Some(r @ Object::Ref(_)) => {
            list_held = display.resolve(r);
            list_held.as_array().map_or(&[], Vec::as_slice)
        }
        _ => &[],
    };
    let mut replaced: Vec<(usize, ObjRef)> = Vec::new();
    for &(index, shown) in marks {
        let listed = list.get(index).ok_or(crate::AnnotError::NoSuchAnnotation { page, index })?;
        let entry_held;
        let src = match listed {
            Object::Dict(d) => d,
            Object::Ref(_) => {
                entry_held = display.resolve(listed);
                // A stream object is drawn from its dictionary; anything else that can't be read as an
                // annotation is an error (the view is then unavailable), never left drawing itself.
                match &*entry_held {
                    Object::Dict(d) => d,
                    Object::Stream(s) => &s.dict,
                    _ => return Err(crate::AnnotError::NoSuchAnnotation { page, index }),
                }
            }
            _ => return Err(crate::AnnotError::NoSuchAnnotation { page, index }),
        };
        let mut d = Dict::default();
        for (key, value) in src.iter().filter(|(key, _)| !DISPLAY_REPLACED_KEYS.contains(&key.as_slice())) {
            d.set(key.clone(), value.clone());
        }
        let flags = src.int(b"F").unwrap_or(0) & !DISPLAY_CLEARED_FLAGS;
        let locator;
        let drawn: Option<Outline<'_>> = match shown {
            Shown::Ready(areas) => Some((areas, display_stroke(display, src), "")),
            Shown::Refused(Some(r)) => {
                locator = [r];
                Some((&locator, [0.84, 0.53, 0.0], "[3 2] 0 d\n"))
            }
            Shown::Refused(None) => None,
        };
        match drawn.and_then(|(areas, rgb, dash)| outline(areas, rgb, dash)) {
            Some((bbox, content)) => {
                let mut sd = Dict::default();
                sd.set(b"Type".to_vec(), Object::Name(b"XObject".to_vec()));
                sd.set(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()));
                sd.set(b"BBox".to_vec(), reals(&bbox));
                sd.set(b"Resources".to_vec(), Object::Dict(Dict::default()));
                let stream = display.add(Object::Stream(pdfcraft_cos::Stream::from_raw(sd, content.into_bytes())));
                let mut ap = Dict::default();
                ap.set(b"N".to_vec(), Object::Ref(stream));
                d.set(b"AP".to_vec(), Object::Dict(ap));
                d.set(b"Rect".to_vec(), reals(&bbox));
                d.set(b"F".to_vec(), Object::Int(flags));
            }
            // Nothing safe to draw: the mark is hidden on the page (the Comments panel lists it).
            None => d.set(b"F".to_vec(), Object::Int(flags | 2)),
        }
        match listed {
            // The listed object is the dictionary: the copy's object is replaced.
            Object::Ref(r) if display.get(*r).as_dict().is_some() => display.set(*r, Object::Dict(d)),
            // Written in place, or listed through a chain: the copy's list gets the drawn dictionary.
            _ => {
                replaced.try_reserve(1).map_err(|_| crate::AnnotError::Unsupported("too many redaction marks to draw".into()))?;
                replaced.push((index, display.add(Object::Dict(d))));
            }
        }
    }
    if !replaced.is_empty() {
        let mut copy = list.to_vec();
        #[cfg(test)]
        tests::count_list_copy();
        for (index, r) in replaced {
            if let Some(slot) = copy.get_mut(index) {
                *slot = Object::Ref(r);
            }
        }
        crate::set_annots(display, page_obj, copy)?;
    }
    Ok(())
}

/// A mark's outline colour: its `/C` as RGB, clamped, or the red PdfCraft draws marks in.
fn display_stroke(doc: &Document, d: &Dict) -> [f64; 3] {
    d.get(b"C")
        .and_then(|c| pdfcraft_model::numeric::device_color(&NumberReader::new(doc), c).ok())
        .and_then(|c| c.rgb())
        .map_or([0.89, 0.13, 0.13], |c| c.map(|v| v.clamp(0.0, 1.0)))
}

/// The box around `areas`, widened by the stroke so it isn't clipped, and the content stroking
/// each area apart. `None` when there is nothing to draw or the box isn't finite.
fn outline(areas: &[[f64; 4]], rgb: [f64; 3], dash: &str) -> Option<([f64; 4], String)> {
    let first = areas.first()?;
    let union = areas.iter().fold(*first, |u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]);
    let bbox = [union[0] - 1.0, union[1] - 1.0, union[2] + 1.0, union[3] + 1.0];
    if !bbox.iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut c = format!("q {} {} {} RG 1 w {dash}", rgb[0], rgb[1], rgb[2]);
    for r in areas {
        c.push_str(&format!(
            "{} {} {} {} re S
",
            r[0],
            r[1],
            r[2] - r[0],
            r[3] - r[1]
        ));
    }
    c.push_str(
        "Q
",
    );
    Some((bbox, c))
}

fn reals(v: &[f64; 4]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

#[cfg(test)]
mod tests;
