//! Turning a stamp or a drawn signature about its middle.
//!
//! A drawing turns its points: plain PDF, the same in every viewer. PDF has no angle for a stamp,
//! so its appearance stays drawn upright and is turned by the form's `/Matrix`, and `/Rect` is the
//! turned box's upright hull. Viewers map the turned `/BBox` onto `/Rect` (ISO 32000-2 §12.5.5),
//! which is exactly that hull, so every viewer shows the stamp turned and unscaled.
//!
//! What PDF can't say is kept beside it: `/PCAngle` (degrees counterclockwise), `/PCSize` (the
//! upright box's width and height, for its proportions) and `/PCBaseMatrix` (the appearance's own
//! `/Matrix` from before it was turned, when it had one). The box's size on the page and its place
//! are always read from `/Rect` ([`own_box`]), so moving or resizing the stamp, in any program,
//! needs no change to these.

use super::create::{bounds, finite, normalize, num_array, rect_for};
use super::draw::redraw_or_restore;
use super::lookup::{annot_dict, annot_ref, drawn_upright};
use super::modify::{border_width_of, ink_strokes, touch, unlocked};
use super::types::{Meta, Shape, Style};
use super::{AnnotError, Dict, Document, ObjRef, Object};

const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// How far a stamp has been turned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Turn {
    /// Degrees counterclockwise: -180 (excluded) to 180, never 0.
    pub angle: f64,
    /// The upright box's width and height; only their proportions are used.
    pub size: [f64; 2],
    /// The appearance's `/Matrix` from before it was turned.
    pub base: [f64; 6],
}

fn numbers(doc: &Document, o: &Object) -> Option<Vec<f64>> {
    doc.resolve(o).as_array()?.iter().map(|v| doc.resolve(v).as_f64()).collect()
}

fn matrix_of(doc: &Document, o: &Object) -> Option<[f64; 6]> {
    let m: [f64; 6] = numbers(doc, o)?.try_into().ok()?;
    finite(&m).then_some(m)
}

/// A dictionary's four numbers at `key` as a rectangle with some area.
fn box_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<[f64; 4]> {
    let r: [f64; 4] = numbers(doc, d.get(key)?)?.try_into().ok()?;
    let r = normalize(r);
    (finite(&r) && r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// The turn a stamp's dictionary records, or `None` for an upright stamp or values that don't
/// make one: such a stamp is treated as upright, and turning it starts from what it shows.
pub(crate) fn turn_of(doc: &Document, d: &Dict) -> Option<Turn> {
    let angle = d.get(b"PCAngle").and_then(|a| doc.resolve(a).as_f64()).filter(|a| a.is_finite()).map(wrap).filter(|a| *a != 0.0)?;
    let size: [f64; 2] = numbers(doc, d.get(b"PCSize")?)?.try_into().ok()?;
    if !(finite(&size) && size[0] > 0.0 && size[1] > 0.0) {
        return None;
    }
    Some(Turn { angle, size, base: d.get(b"PCBaseMatrix").and_then(|m| matrix_of(doc, m)).unwrap_or(IDENTITY) })
}

/// `degrees` within -180 (excluded) to 180; a whisker from upright is upright.
fn wrap(degrees: f64) -> f64 {
    let a = 180.0 - (180.0 - degrees).rem_euclid(360.0);
    if a.abs() < 1e-6 { 0.0 } else { a }
}

/// Sine and cosine of `degrees`, exact on quarter turns.
fn sin_cos(degrees: f64) -> (f64, f64) {
    match degrees.rem_euclid(360.0) {
        0.0 => (0.0, 1.0),
        90.0 => (1.0, 0.0),
        180.0 => (0.0, -1.0),
        270.0 => (-1.0, 0.0),
        d => d.to_radians().sin_cos(),
    }
}

/// `m` then `n`, as PDF multiplies matrices.
fn then(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

/// The upright box around `b` transformed by `m`.
fn hull(m: [f64; 6], b: [f64; 4]) -> Option<[f64; 4]> {
    bounds(
        [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]].into_iter().map(|[x, y]| [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]),
    )
}

/// The `/Matrix` that draws a form with `/BBox` `bbox` and base matrix `base` in the box `own`,
/// turned `angle` degrees counterclockwise about its middle. `None` for a form with no area.
fn turned_matrix(base: [f64; 6], bbox: [f64; 4], own: [f64; 4], angle: f64) -> Option<[f64; 6]> {
    let from = hull(base, bbox)?;
    let (sx, sy) = ((own[2] - own[0]) / (from[2] - from[0]), (own[3] - own[1]) / (from[3] - from[1]));
    if !(sx.is_finite() && sy.is_finite()) {
        return None;
    }
    let fit = [sx, 0.0, 0.0, sy, own[0] - from[0] * sx, own[1] - from[1] * sy];
    let (sin, cos) = sin_cos(angle);
    let (cx, cy) = ((own[0] + own[2]) / 2.0, (own[1] + own[3]) / 2.0);
    let turn = [cos, sin, -sin, cos, cx - cos * cx + sin * cy, cy - sin * cx - cos * cy];
    let m = then(then(base, fit), turn);
    finite(&m).then_some(m)
}

/// The upright box of a stamp turned by `angle`, whose `/Rect` is `rect`: of `size`'s proportions,
/// the largest whose hull fits `rect`, in its middle.
pub(crate) fn own_box(rect: [f64; 4], angle: f64, size: [f64; 2]) -> [f64; 4] {
    let (sin, cos) = sin_cos(angle);
    let (sin, cos) = (sin.abs(), cos.abs());
    let scale = ((rect[2] - rect[0]) / (cos * size[0] + sin * size[1])).min((rect[3] - rect[1]) / (sin * size[0] + cos * size[1]));
    let (w, h) = (size[0] * scale / 2.0, size[1] * scale / 2.0);
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    [cx - w, cy - h, cx + w, cy + h]
}

/// `rect` as a turned stamp whose hull is now `old` can fill it: the same shape, in its middle.
/// Stretching the hull would shear the turned appearance.
pub(crate) fn same_shape(old: [f64; 4], rect: [f64; 4]) -> [f64; 4] {
    let (w, h) = (old[2] - old[0], old[3] - old[1]);
    let scale = ((rect[2] - rect[0]) / w).min((rect[3] - rect[1]) / h);
    let (w, h) = (w * scale / 2.0, h * scale / 2.0);
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    [cx - w, cy - h, cx + w, cy + h]
}

/// `/Rect` of a stamp, when it has one with some area.
pub(crate) fn rect_of(doc: &Document, d: &Dict) -> Option<[f64; 4]> {
    box_of(doc, d, b"Rect")
}

/// The stamp's dictionary with its own box as `/Rect`, for drawing its appearance upright.
pub(crate) fn upright_dict(doc: &Document, d: &Dict, turn: &Turn) -> Option<Dict> {
    let mut upright = d.clone();
    upright.set(b"Rect".to_vec(), num_array(&own_box(rect_of(doc, d)?, turn.angle, turn.size)));
    Some(upright)
}

/// Turn a freshly drawn upright appearance of the stamp `d` (see [`upright_dict`]) by `turn`.
pub(crate) fn turn_form(doc: &Document, d: &Dict, turn: &Turn, form: &mut Dict) -> Option<()> {
    let bbox = box_of(doc, form, b"BBox")?;
    // A picture's orientation was set when it was placed, and only the old appearance knows it.
    let base = if d.contains(b"PCPicture") { turn.base } else { form.get(b"Matrix").and_then(|m| matrix_of(doc, m)).unwrap_or(IDENTITY) };
    let own = own_box(rect_of(doc, d)?, turn.angle, turn.size);
    form.set(b"Matrix".to_vec(), num_array(&turned_matrix(base, bbox, own, turn.angle)?));
    Some(())
}

/// Turn a stamp or a drawing `degrees` counterclockwise about its middle. A stamp that comes back
/// upright is as it was before it was first turned. Other comments, locked ones and ones kept
/// upright on the page (NoRotate) are refused.
pub fn rotate_annotation(doc: &mut Document, page: usize, index: usize, degrees: f64, meta: &Meta) -> Result<(), AnnotError> {
    if !finite(&[degrees]) {
        return Err(AnnotError::Invalid("invalid angle".into()));
    }
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let d = annot_dict(doc, r);
    if drawn_upright(doc, &d) {
        return Err(AnnotError::Invalid("the comment is kept upright on the page, so it can't be rotated".into()));
    }
    match d.name(b"Subtype") {
        Some(b"Stamp") => turn_stamp(doc, r, &d, degrees, meta),
        Some(b"Ink") => turn_ink(doc, r, d, degrees, meta),
        other => Err(AnnotError::Invalid(format!("{} comments can't be rotated", String::from_utf8_lossy(other.unwrap_or_default())))),
    }
}

fn turn_stamp(doc: &mut Document, r: ObjRef, d: &Dict, degrees: f64, meta: &Meta) -> Result<(), AnnotError> {
    let invalid = |why: &str| AnnotError::Invalid(why.into());
    let rect = rect_of(doc, d).ok_or_else(|| invalid("the stamp has no rectangle"))?;
    let ap = d.get(b"AP").map(|ap| doc.resolve(ap)).and_then(|ap| ap.as_dict().cloned()).ok_or_else(|| invalid("the stamp has no appearance"))?;
    let normal = ap.get(b"N").map(|n| doc.resolve(n));
    let Some(Object::Stream(form)) = normal.as_deref() else {
        return Err(invalid("the stamp's normal appearance is not a stream"));
    };
    let mut form = form.clone();
    let bbox = box_of(doc, &form.dict, b"BBox").ok_or_else(|| invalid("the stamp's appearance has no box"))?;
    let turn = turn_of(doc, d);
    let (base, own, was) = match &turn {
        Some(t) => (t.base, own_box(rect, t.angle, t.size), t.angle),
        None => (form.dict.get(b"Matrix").and_then(|m| matrix_of(doc, m)).unwrap_or(IDENTITY), rect, 0.0),
    };
    let angle = wrap(was + degrees);
    let (matrix, shown) = if angle == 0.0 {
        (base, own)
    } else {
        let m = turned_matrix(base, bbox, own, angle).ok_or_else(|| invalid("the stamp's appearance has no area"))?;
        (m, hull(m, bbox).ok_or_else(|| invalid("the stamp's appearance has no area"))?)
    };
    if !finite(&shown) || shown[2] <= shown[0] || shown[3] <= shown[1] {
        return Err(invalid("the turned stamp doesn't fit the page's coordinates"));
    }
    if matrix == IDENTITY {
        form.dict.remove(b"Matrix");
    } else {
        form.dict.set(b"Matrix".to_vec(), num_array(&matrix));
    }
    // A new appearance: another stamp may share this one (a copy, a repeated signature). The old
    // down and rollover appearances would show it upright on press or hover.
    let form = doc.add(Object::Stream(form));
    let mut ap = ap;
    ap.remove(b"D");
    ap.remove(b"R");
    ap.set(b"N".to_vec(), Object::Ref(form));
    doc.update_dict(r, |d| {
        d.set(b"AP".to_vec(), Object::Dict(ap));
        d.set(b"Rect".to_vec(), num_array(&shown));
        d.remove(b"PCBaseMatrix");
        if angle == 0.0 {
            d.remove(b"PCAngle");
            d.remove(b"PCSize");
        } else {
            d.set(b"PCAngle".to_vec(), Object::Real(angle));
            d.set(b"PCSize".to_vec(), num_array(&[own[2] - own[0], own[3] - own[1]]));
            if base != IDENTITY {
                d.set(b"PCBaseMatrix".to_vec(), num_array(&base));
            }
        }
        touch(d, meta);
    })?;
    Ok(())
}

/// Turn a drawing's strokes about the middle of their box; `/Rect` follows them, with room for the
/// line's width as when it was drawn, and the appearance is redrawn.
fn turn_ink(doc: &mut Document, r: ObjRef, d: Dict, degrees: f64, meta: &Meta) -> Result<(), AnnotError> {
    let strokes = ink_strokes(doc, &d);
    let b = bounds(strokes.iter().flatten().copied()).ok_or_else(|| AnnotError::Invalid("the drawing has no strokes".into()))?;
    let (sin, cos) = sin_cos(degrees);
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let strokes: Vec<Vec<[f64; 2]>> = strokes
        .iter()
        .map(|s| s.iter().map(|p| [cx + cos * (p[0] - cx) - sin * (p[1] - cy), cy + sin * (p[0] - cx) + cos * (p[1] - cy)]).collect())
        .collect();
    // Checks the points too: finite, and some to draw.
    let style = Style { width: border_width_of(&d), ..Style::default() };
    let rect = rect_for(&Shape::Ink { strokes: strokes.clone() }, &style)?;
    let before = d;
    doc.update_dict(r, |d| {
        d.set(b"InkList".to_vec(), Object::Array(strokes.iter().map(|s| num_array(&s.concat())).collect()));
        d.set(b"Rect".to_vec(), num_array(&rect));
        touch(d, meta);
    })?;
    redraw_or_restore(doc, r, before)
}
