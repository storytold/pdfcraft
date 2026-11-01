//! A callout's three parts: text box, knee line and end-point line (ISO 32000-2 Table 177).
//!
//! Acrobat lays a new knee line 12 pt out from the middle of the side facing the arrow, and lets
//! each part be dragged on its own (Acrobat DC Help, 11/28/2019, p. 357). The distances come from
//! black-box observation of Acrobat Pro DC with synthetic files, recorded on #4094.

use super::create::{bounds, finite, grow, normalize, num_array};
use super::draw::{draw_appearance, redraw_or_restore};
use super::lookup::annot_dict;
use super::modify::{border_width_of, refuse_foreign_text, touch};
use super::{AnnotError, Dict, Document, Meta, ObjRef, Object, appearance};

/// A side of a callout's text box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// Whether a knee line on this side is horizontal (it meets the left or right side). Acrobat
    /// lets a knee line be resized along its own direction only.
    pub fn horizontal(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }
}

/// A callout's knee line when Acrobat lays it out: 12 pt, out from the middle of the side. A new
/// callout's knee line, and one laid out again after its knee was dragged into the box or the side
/// it meets was moved, are 12 pt, and the knee can't come nearer the box than that.
pub const CALLOUT_KNEE_LINE: f64 = 12.0;

/// How far apart, in points, two positions of a callout's box or line may be and still count as the
/// same. The page hands a box back as `f32` values, which moves a coordinate such as 323.05 by a few
/// hundred-thousandths of a point (review on #4094).
const CALLOUT_SLACK: f64 = 0.01;

/// The side of the text box `rect` that faces `knee`: the one [`callout_attach`] attaches to.
pub fn callout_side(rect: [f64; 4], knee: [f64; 2]) -> Side {
    let [x0, _, x1, y1] = normalize(rect);
    if knee[0] < x0 {
        Side::Left
    } else if knee[0] > x1 {
        Side::Right
    } else if knee[1] > y1 {
        Side::Top
    } else {
        Side::Bottom
    }
}

/// Where a callout's leader line meets its text box: the middle of the side facing `knee`.
pub fn callout_attach(rect: [f64; 4], knee: [f64; 2]) -> [f64; 2] {
    let [x0, y0, x1, y1] = normalize(rect);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    match callout_side(rect, knee) {
        Side::Left => [x0, cy],
        Side::Right => [x1, cy],
        Side::Top => [cx, y1],
        Side::Bottom => [cx, y0],
    }
}

/// Where a knee goes on `side` of the text box `rect`: [`CALLOUT_KNEE_LINE`] out from the middle
/// of that side, so the knee line is level (left, right) or upright (top, bottom).
pub fn callout_knee_on(rect: [f64; 4], side: Side) -> [f64; 2] {
    let [x0, y0, x1, y1] = normalize(rect);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    match side {
        Side::Left => [x0 - CALLOUT_KNEE_LINE, cy],
        Side::Right => [x1 + CALLOUT_KNEE_LINE, cy],
        Side::Top => [cx, y1 + CALLOUT_KNEE_LINE],
        Side::Bottom => [cx, y0 - CALLOUT_KNEE_LINE],
    }
}

/// A new callout's knee: on the side of its text box `rect` that faces the arrow `point`.
pub fn callout_knee(rect: [f64; 4], point: [f64; 2]) -> [f64; 2] {
    callout_knee_on(rect, callout_side(rect, point))
}

/// Whether `end` lies on `side` of the text box `rect`: the leader line can stay where it meets it.
fn on_side(rect: [f64; 4], side: Side, end: [f64; 2]) -> bool {
    let [x0, y0, x1, y1] = normalize(rect);
    let near = |a: f64, b: f64| (a - b).abs() < CALLOUT_SLACK;
    let within = |v: f64, lo: f64, hi: f64| v > lo - CALLOUT_SLACK && v < hi + CALLOUT_SLACK;
    match side {
        Side::Left => near(end[0], x0) && within(end[1], y0, y1),
        Side::Right => near(end[0], x1) && within(end[1], y0, y1),
        Side::Top => near(end[1], y1) && within(end[0], x0, x1),
        Side::Bottom => near(end[1], y0) && within(end[0], x0, x1),
    }
}

/// A callout's leader line (`/CL`): four numbers (start, end) or six (start, knee, end). `None`
/// when it is missing, longer than the spec allows, or not entirely numeric.
pub(crate) fn callout_line(doc: &Document, d: &Dict) -> Option<Vec<[f64; 2]>> {
    let resolved = doc.resolve(d.get(b"CL")?);
    let array = resolved.as_array()?;
    // Table 177: four or six numbers. Anything else is not a line we can reshape.
    if !matches!(array.len(), 4 | 6) {
        return None;
    }
    // All or nothing: a non-number would shift every later number into the wrong point.
    let nums: Vec<f64> = array.iter().map(|item| doc.resolve(item).as_f64()).collect::<Option<_>>()?;
    finite(&nums).then(|| nums.as_chunks::<2>().0.to_vec())
}

/// A callout's text box: `/Rect` inset by `/RD`. `None` without both, or when the inset leaves
/// less than a point of box.
pub(crate) fn callout_text_box(doc: &Document, d: &Dict) -> Option<[f64; 4]> {
    let nums = |key: &[u8]| -> Option<Vec<f64>> {
        let resolved = doc.resolve(d.get(key)?);
        let array = resolved.as_array()?;
        if array.len() != 4 {
            return None;
        }
        array.iter().map(|item| doc.resolve(item).as_f64()).collect()
    };
    let r = nums(b"Rect").filter(|v| finite(v))?;
    let rd = nums(b"RD").filter(|v| finite(v) && v.iter().all(|x| *x >= 0.0))?;
    let r = normalize([r[0], r[1], r[2], r[3]]);
    let tb = [r[0] + rd[0], r[1] + rd[1], r[2] - rd[2], r[3] - rd[3]];
    (tb[2] - tb[0] >= 1.0 && tb[3] - tb[1] >= 1.0).then_some(tb)
}

/// Store `rect` as a FreeText annotation's text box (or a square's or oval's rectangle). For a
/// callout, `lead` (its arrow point and optional knee) replaces the leader line's own points.
/// The arrow point never moves with the box, and the line follows the box as Acrobat was seen to:
/// the box moved without changing size, the knee and the end come along; the box resized with the
/// side the line meets left where it was, the line stays; otherwise the line meets the middle of
/// the side facing the arrow, the knee [`CALLOUT_KNEE_LINE`] out from it. Acrobat also moves the
/// box next to the arrow then; the box stays where it was put.
///
/// With `lead`, the end stays where it is while it is on the side facing the knee, and otherwise
/// meets the middle of that side.
pub(crate) fn place_text_box(
    doc: &mut Document,
    r: ObjRef,
    rect: [f64; 4],
    lead: Option<&[[f64; 2]]>,
    meta: &Meta,
    new_text: bool,
) -> Result<(), AnnotError> {
    let rect = normalize(rect);
    if !finite(&rect) || rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return Err(AnnotError::Invalid("invalid rectangle (too small)".into()));
    }
    let d = annot_dict(doc, r);
    if !new_text {
        refuse_foreign_text(doc, &d)?;
    }
    let old_line = callout_line(doc, &d);
    let old_box = callout_text_box(doc, &d);
    let old_end = old_line.as_deref().and_then(<[[f64; 2]]>::last).copied();
    let line: Option<Vec<[f64; 2]>> = match (lead, old_line.as_deref()) {
        (Some(lead @ [.., toward]), _) => {
            let side = callout_side(rect, *toward);
            let end = old_end.filter(|end| on_side(rect, side, *end)).unwrap_or_else(|| callout_attach(rect, *toward));
            Some(lead.iter().copied().chain([end]).collect())
        }
        (Some(_), _) => None,
        (None, Some(old @ [start, .., end])) => {
            let (start, end) = (*start, *end);
            let knee = old.get(1).filter(|_| old.len() == 3).copied();
            let toward = knee.unwrap_or(start);
            Some(match old_box {
                // Moved, not resized: the knee and the end come along. The arrow stays.
                Some(b)
                    if ((rect[2] - rect[0]) - (b[2] - b[0])).abs() < CALLOUT_SLACK && ((rect[3] - rect[1]) - (b[3] - b[1])).abs() < CALLOUT_SLACK =>
                {
                    let (dx, dy) = (rect[0] - b[0], rect[1] - b[1]);
                    let shift = |p: [f64; 2]| [p[0] + dx, p[1] + dy];
                    [start].into_iter().chain(knee.map(shift)).chain([shift(end)]).collect()
                }
                // Resized, the side the line meets still under its end: nothing changes.
                Some(b) if on_side(rect, callout_side(b, toward), end) => old.to_vec(),
                // That side moved: laid out again, from the side facing the arrow point.
                Some(_) => {
                    let new_knee = callout_knee(rect, start);
                    let end = callout_attach(rect, new_knee);
                    [start].into_iter().chain(knee.map(|_| new_knee)).chain([end]).collect()
                }
                None => [start].into_iter().chain(knee).chain([callout_attach(rect, toward)]).collect(),
            })
        }
        (None, _) => None,
    };
    let (outer, cl) = match line.as_deref() {
        Some(line @ [lead @ .., _]) => {
            // The end is on the box, so only the arrow point and the knee need room around them.
            let width = border_width_of(&d);
            let pad = width / 2.0 + appearance::arrow_size(width) + 1.0;
            let lead_box = grow(bounds(lead.iter().copied()).unwrap_or_default(), pad);
            ([rect[0].min(lead_box[0]), rect[1].min(lead_box[1]), rect[2].max(lead_box[2]), rect[3].max(lead_box[3])], Some(line.concat()))
        }
        _ => (rect, None),
    };
    doc.update_dict(r, |d| {
        d.set(b"Rect".to_vec(), num_array(&outer));
        match &cl {
            Some(nums) => {
                d.set(b"CL".to_vec(), num_array(nums));
                d.set(b"RD".to_vec(), num_array(&[rect[0] - outer[0], rect[1] - outer[1], outer[2] - rect[2], outer[3] - rect[3]]));
            }
            None => {
                d.remove(b"RD");
            }
        }
        touch(d, meta);
    })?;
    let before = d;
    if new_text {
        if let Err(e) = draw_appearance(doc, r) {
            doc.set(r, Object::Dict(before));
            return Err(e);
        }
        Ok(())
    } else {
        redraw_or_restore(doc, r, before)
    }
}
