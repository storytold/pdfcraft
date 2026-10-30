//! Physical geometry and local appearances for drawn Fill & Sign signatures.

use crate::create::{finite, num_array};
use crate::{AnnotError, Dict, Document, Meta, NewAnnotation, ObjRef, Object, Shape};

/// Compatibility defaults are display-only and never enter the stored model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignatureAppearanceUse {
    Edit,
    Display,
}

type Point = [f64; 2];
type Rect = [f64; 4];
const MAX_SIGNATURE_POINTS: usize = 100_000;
const MAX_SIGNATURE_STROKES: usize = 100_000;

fn invalid(message: &str) -> AnnotError {
    AnnotError::Invalid(message.into())
}

/// Canonical, noninherited model policy; no new UserUnit parser or clamp.
fn page_unit(doc: &Document, page: usize) -> Result<f64, AnnotError> {
    let obj = crate::lookup::page_ref(doc, page)?;
    let value = doc.get(obj);
    let dict = value.as_dict().ok_or_else(|| invalid("the signature page is invalid"))?;
    let unit = pdfcraft_model::Page { obj, dict: dict.clone() }.user_unit(doc);
    if !(unit.is_finite() && (1.0..=pdfcraft_model::MAX_USER_UNIT).contains(&unit)) {
        return Err(invalid("the signature page unit is invalid"));
    }
    Ok(unit)
}

/// Match the actual selected annotation-Rect sink, without promising exact f32 dimensions.
fn checked_rect(rect: Rect) -> Result<Rect, AnnotError> {
    if !finite(&rect) || !(rect[2] > rect[0] && rect[3] > rect[1]) {
        return Err(invalid("the signature rectangle must be finite and nonempty"));
    }
    let narrowed = rect.map(|v| v as f32);
    if !narrowed.iter().all(|v| v.is_finite()) || narrowed[2] <= narrowed[0] || narrowed[3] <= narrowed[1] {
        return Err(invalid("the signature rectangle cannot be represented at this position"));
    }
    Ok(rect)
}

/// Strict bounded parser: no odd coordinate loss, ignored bad points or recursive expansion.
pub(crate) fn read_strokes(doc: &Document, d: &Dict) -> Result<Vec<Vec<Point>>, AnnotError> {
    let object = d.get(b"InkList").ok_or_else(|| invalid("the signature has no InkList"))?;
    let object = doc.resolve(object);
    let list = object.as_array().ok_or_else(|| invalid("the signature InkList is not an array"))?;
    if list.len() > MAX_SIGNATURE_STROKES {
        return Err(invalid("the signature has too many strokes"));
    }
    let mut count = 0usize;
    let mut strokes = Vec::with_capacity(list.len());
    for stroke in list {
        let stroke = doc.resolve(stroke);
        let values = stroke.as_array().ok_or_else(|| invalid("a signature stroke is not an array"))?;
        if values.len() % 2 != 0 {
            return Err(invalid("a signature stroke has an unmatched coordinate"));
        }
        let length = values.len() / 2;
        count = count.checked_add(length).ok_or_else(|| invalid("the signature has too many points"))?;
        if count > MAX_SIGNATURE_POINTS {
            return Err(invalid("the signature has more than 100,000 points"));
        }
        let mut points = Vec::with_capacity(length);
        for pair in values.as_chunks::<2>().0 {
            let x = doc.resolve(&pair[0]).as_f64().ok_or_else(|| invalid("a signature coordinate is not a number"))?;
            let y = doc.resolve(&pair[1]).as_f64().ok_or_else(|| invalid("a signature coordinate is not a number"))?;
            if !finite(&[x, y]) {
                return Err(invalid("a signature coordinate is out of range"));
            }
            points.push([x, y]);
        }
        strokes.push(points);
    }
    stroke_hull(&strokes)?;
    Ok(strokes)
}

fn read_rect(doc: &Document, d: &Dict) -> Result<Rect, AnnotError> {
    let object = d.get(b"Rect").ok_or_else(|| invalid("the signature has no rectangle"))?;
    let object = doc.resolve(object);
    let values = object.as_array().filter(|v| v.len() == 4).ok_or_else(|| invalid("the signature rectangle is invalid"))?;
    let mut rect = [0.0; 4];
    for (out, value) in rect.iter_mut().zip(values) {
        *out = doc.resolve(value).as_f64().ok_or_else(|| invalid("a signature rectangle coordinate is not a number"))?;
    }
    checked_rect(rect)
}

/// A real BS dictionary is authoritative. Only display may ignore an invalid BS type
/// and consult legacy Border, matching the existing appearance reader.
fn border_style(doc: &Document, d: &Dict, usage: SignatureAppearanceUse) -> Result<Option<Dict>, AnnotError> {
    match d.get(b"BS") {
        None => Ok(None),
        Some(value) => match doc.resolve(value).as_dict() {
            Some(bs) => Ok(Some(bs.clone())),
            None if usage == SignatureAppearanceUse::Display => Ok(None),
            None => Err(invalid("the signature border style is invalid")),
        },
    }
}

/// Resize is always strict, independently of the display-only appearance policy.
fn width_of(doc: &Document, d: &Dict) -> Result<f64, AnnotError> {
    appearance_width(doc, d, SignatureAppearanceUse::Edit)
}

fn appearance_width(doc: &Document, d: &Dict, usage: SignatureAppearanceUse) -> Result<f64, AnnotError> {
    let raw = if let Some(bs) = border_style(doc, d, usage)? {
        bs.get(b"W").cloned()
    } else if let Some(border) = d.get(b"Border") {
        let border = doc.resolve(border);
        match border.as_array() {
            Some(values) => values.get(2).cloned(),
            None if usage == SignatureAppearanceUse::Display => None,
            None => return Err(invalid("the signature border is invalid")),
        }
    } else {
        None
    };
    let width = match raw {
        None => 1.0,
        Some(value) => match doc.resolve(&value).as_f64() {
            Some(width) if width.is_finite() && width >= 0.0 => width,
            _ if usage == SignatureAppearanceUse::Display => 1.0,
            _ => return Err(invalid("the signature line width is invalid")),
        },
    };
    // A valid nonnegative finite raw width is never a malformed-style defaulting case.
    // Retain the existing cap and every derived sink check in both modes.
    if width >= 1e7 {
        return Err(invalid("the signature line width is out of range"));
    }
    Ok(width)
}

/// Full Catmull-Rom control hull, not only the raw vertex box.
fn stroke_hull<T: Copy + Into<f64>>(strokes: &[Vec<[T; 2]>]) -> Result<Rect, AnnotError> {
    if strokes.len() > MAX_SIGNATURE_STROKES {
        return Err(invalid("the signature has too many strokes"));
    }
    let mut count = 0usize;
    let mut hull: Option<Rect> = None;
    for stroke in strokes {
        count = count.checked_add(stroke.len()).ok_or_else(|| invalid("the signature has too many points"))?;
        if count > MAX_SIGNATURE_POINTS {
            return Err(invalid("the signature has more than 100,000 points"));
        }
        let mut add = |p: Point| -> Result<(), AnnotError> {
            if !finite(&p) {
                return Err(invalid("a signature curve control is out of range"));
            }
            let point = [p[0], p[1], p[0], p[1]];
            hull = Some(match hull {
                None => point,
                Some([x0, y0, x1, y1]) => [x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1])],
            });
            Ok(())
        };
        for p in stroke {
            add(p.map(Into::into))?;
        }
        // Same endpoint repetition and arithmetic order as smooth_segments, allocation-free.
        if stroke.len() > 2 {
            let last = stroke.last().copied().ok_or_else(|| invalid("the signature has no strokes"))?;
            let at = |i: usize| stroke.get(i).copied().unwrap_or(last).map(Into::into);
            for i in 0..stroke.len().saturating_sub(1) {
                let (p0, p1, p2, p3): (Point, Point, Point, Point) =
                    (at(i.saturating_sub(1)), at(i), at(i.saturating_add(1)), at(i.saturating_add(2)));
                add([p1[0] + (p2[0] - p0[0]) / 6.0, p1[1] + (p2[1] - p0[1]) / 6.0])?;
                add([p2[0] - (p3[0] - p1[0]) / 6.0, p2[1] - (p3[1] - p1[1]) / 6.0])?;
            }
        }
    }
    hull.ok_or_else(|| invalid("the signature has no strokes"))
}

fn has_dot<T: Copy + Into<f64>>(strokes: &[Vec<[T; 2]>]) -> bool {
    strokes.iter().any(|s| {
        s.first().is_some_and(|first| {
            let first: Point = first.map(Into::into);
            s.iter().all(|p| p.map(Into::into) == first)
        })
    })
}

fn fit_target(source: Rect, rect: Rect, margin: Rect) -> Result<Rect, AnnotError> {
    if !margin.iter().all(|v| v.is_finite() && *v >= 0.0) {
        return Err(invalid("the signature padding is invalid"));
    }
    let target = [rect[0] + margin[0], rect[1] + margin[1], rect[2] - margin[2], rect[3] - margin[3]];
    if !finite(&target)
        || target[2] < target[0]
        || target[3] < target[1]
        || (source[2] > source[0] && target[2] <= target[0])
        || (source[3] > source[1] && target[3] <= target[1])
    {
        return Err(invalid("this size cannot contain the drawn signature's stroke and padding; choose a larger size"));
    }
    Ok(target)
}

fn fit(strokes: &[Vec<Point>], source: Rect, rect: Rect, margin: Rect, width: f64) -> Result<Vec<Vec<Point>>, AnnotError> {
    let target = fit_target(source, rect, margin)?;
    let axis = |v: f64, a0: f64, a1: f64, b0: f64, b1: f64| {
        if a1 > a0 { b0 + (v - a0) / (a1 - a0) * (b1 - b0) } else { b0 + (b1 - b0) / 2.0 }
    };
    let scaled: Vec<Vec<Point>> = strokes
        .iter()
        .map(|s| {
            s.iter()
                .map(|p| [axis(p[0], source[0], source[2], target[0], target[2]), axis(p[1], source[1], source[3], target[1], target[3])])
                .collect()
        })
        .collect();
    let hull = stroke_hull(&scaled)?;
    let radius = width / 2.0;
    if !width.is_finite()
        || width < 0.0
        || hull[0] - radius < rect[0]
        || hull[1] - radius < rect[1]
        || hull[2] + radius > rect[2]
        || hull[3] + radius > rect[3]
    {
        return Err(invalid("the signature stroke cannot be represented inside this rectangle"));
    }
    Ok(scaled)
}

/// Target-aware desktop eligibility; no transformed stroke vectors need be allocated.
/// Preference dimension limits remain enforced by SignaturePlacementSize, not duplicated here.
pub fn validate_signature_size<T: Copy + Into<f64>>(strokes: &[Vec<[T; 2]>], width_physical: f64, size: [f64; 2]) -> Result<(), AnnotError> {
    if !(width_physical.is_finite() && (0.0..1e7).contains(&width_physical)) || !size.iter().all(|v| v.is_finite() && *v > 0.0 && *v < 1e7) {
        return Err(invalid("the drawn signature size or width is invalid"));
    }
    let source = stroke_hull(strokes)?;
    if width_physical == 0.0 && has_dot(strokes) {
        return Err(invalid("an isolated drawn dot needs a positive line width"));
    }
    let pad = width_physical / 2.0 + 1.0;
    fit_target(source, [0.0, 0.0, size[0], size[1]], [pad; 4])?;
    Ok(())
}

/// Physical points for width/padding only on NEW explicit placements.
/// The engine retains the outer COS-clone transaction, as for add_annotation.
pub fn add_sized_signature(doc: &mut Document, new: &NewAnnotation, rect: Rect, meta: &Meta) -> Result<usize, AnnotError> {
    let Shape::Signature { strokes } = &new.shape else {
        return Err(invalid("a sized drawn signature needs Signature strokes"));
    };
    let rect = checked_rect(rect)?;
    let unit = page_unit(doc, new.page)?;
    let physical = [(rect[2] - rect[0]) * unit, (rect[3] - rect[1]) * unit];
    validate_signature_size(strokes, new.style.width, physical)?;
    let width = new.style.width / unit;
    let pad = (new.style.width / 2.0 + 1.0) / unit;
    if !width.is_finite() || (new.style.width > 0.0 && width <= 0.0) || !(pad.is_finite() && pad > 0.0) {
        return Err(invalid("the signature metrics cannot be represented on this page"));
    }
    let scaled = fit(strokes, stroke_hull(strokes)?, rect, [pad; 4], width)?;
    let mut sized = new.clone();
    sized.shape = Shape::Signature { strokes: scaled };
    sized.style.width = width;
    crate::create::add_annotation_with_rect(doc, &sized, meta, Some(rect))
}

/// Called only after ordinary annotation locking checks and bounded signature recognition.
pub(crate) fn resize_signature(doc: &mut Document, page: usize, r: ObjRef, rect: Rect, meta: &Meta) -> Result<(), AnnotError> {
    let rect = checked_rect(rect)?;
    let unit = page_unit(doc, page)?;
    if (rect[2] - rect[0]) * unit < 1.0 || (rect[3] - rect[1]) * unit < 1.0 {
        return Err(invalid("the drawn signature rectangle must be at least one physical point on each axis"));
    }
    let before = crate::lookup::annot_dict(doc, r);
    let old = read_rect(doc, &before)?;
    let strokes = read_strokes(doc, &before)?;
    let source = stroke_hull(&strokes)?;
    let width = width_of(doc, &before)?;
    if width == 0.0 && has_dot(&strokes) {
        return Err(invalid("an isolated drawn dot needs a positive line width"));
    }
    let margin = [source[0] - old[0], source[1] - old[1], old[2] - source[2], old[3] - source[3]];
    if !margin.iter().all(|m| m.is_finite() && *m >= width / 2.0) {
        return Err(invalid("the signature's stored stroke does not fit its rectangle"));
    }
    // Preserve true stored RAW metrics; never divide width/margins by UserUnit again.
    let scaled = fit(&strokes, source, rect, margin, width)?;
    doc.update_dict(r, |d| {
        d.set(b"InkList".to_vec(), Object::Array(scaled.iter().map(|s| num_array(&s.concat())).collect()));
        d.set(b"Rect".to_vec(), num_array(&rect));
        crate::modify::touch(d, meta);
    })?;
    crate::draw::redraw_or_restore(doc, r, before)
}

/// Build-only normalization, never persisted into the model or cached with page context.
/// Caller has used bounded Signature-subject recognition, not PCFillSign alone.
pub(crate) fn normalized_appearance(doc: &Document, d: &Dict, usage: SignatureAppearanceUse) -> Result<Dict, AnnotError> {
    let rect = read_rect(doc, d)?;
    let strokes = read_strokes(doc, d)?;
    let width = appearance_width(doc, d, usage)?;
    if width == 0.0 && has_dot(&strokes) {
        return Err(invalid("an isolated drawn dot needs a positive line width"));
    }
    let hull = stroke_hull(&strokes)?;
    let radius = width / 2.0;
    // Existing imported Ink may intentionally use Rect as a clipping viewport, or
    // have an exactly bounded spline whose control hull is larger than its visible ink.
    // Only display retains that viewport; committed edits still require safe containment.
    // Strict parsing, normalized metrics and computed f32 operand checks remain below.
    if usage == SignatureAppearanceUse::Edit
        && (hull[0] - radius < rect[0] || hull[1] - radius < rect[1] || hull[2] + radius > rect[2] || hull[3] + radius > rect[3])
    {
        return Err(invalid("the signature's stored stroke does not fit its rectangle"));
    }
    let longest = (rect[2] - rect[0]).max(rect[3] - rect[1]);
    let scale = 1000.0 / longest;
    if !(longest.is_finite() && longest > 0.0 && scale.is_finite() && scale > 0.0) {
        return Err(invalid("the signature appearance scale is invalid"));
    }
    let bbox = [0.0, 0.0, (rect[2] - rect[0]) * scale, (rect[3] - rect[1]) * scale];
    if !bbox.iter().all(|v| v.is_finite() && (*v as f32).is_finite()) || bbox[2] as f32 <= 0.0 || bbox[3] as f32 <= 0.0 {
        return Err(invalid("the signature appearance box cannot be represented"));
    }
    let local: Vec<Vec<Point>> = strokes.iter().map(|s| s.iter().map(|p| [(p[0] - rect[0]) * scale, (p[1] - rect[1]) * scale]).collect()).collect();
    let local_width = width * scale;
    if !local_width.is_finite()
        || !(local_width as f32).is_finite()
        || (width > 0.0 && local_width as f32 <= 0.0)
        || !local.iter().flatten().all(|p| p.iter().all(|v| v.is_finite() && (*v as f32).is_finite()))
    {
        return Err(invalid("the signature appearance metrics cannot be represented"));
    }
    let mut out = d.clone();
    if let Some(color) = d.get(b"C") {
        let color = doc.resolve(color);
        let values = color.as_array().filter(|a| matches!(a.len(), 0 | 1 | 3 | 4)).ok_or_else(|| invalid("the signature color is invalid"))?;
        let mut resolved = Vec::with_capacity(values.len());
        for value in values {
            let value = doc.resolve(value).as_f64().filter(|v| v.is_finite()).ok_or_else(|| invalid("the signature color is invalid"))?;
            resolved.push(Object::Real(value));
        }
        out.set(b"C".to_vec(), Object::Array(resolved));
    }
    if let Some(opacity) = d.get(b"CA") {
        let opacity = match doc.resolve(opacity).as_f64() {
            // The reader historically clamps an overflowing decimal opacity. Keep
            // that display behavior, emitting only the finite clamped value.
            Some(value) if usage == SignatureAppearanceUse::Display && !value.is_nan() => value.clamp(0.0, 1.0),
            Some(value) if value.is_finite() => value,
            None if usage == SignatureAppearanceUse::Display => 1.0,
            _ => return Err(invalid("the signature opacity is invalid")),
        };
        out.set(b"CA".to_vec(), Object::Real(opacity));
    }
    out.set(b"Rect".to_vec(), num_array(&bbox));
    out.set(b"InkList".to_vec(), Object::Array(local.iter().map(|s| num_array(&s.concat())).collect()));
    // Literal bounded subject lets the pure appearance builder select Signature-specific
    // precise numeric emission and dot handling even when the stored subject was indirect.
    out.set(b"Subj".to_vec(), crate::PdfString::text("Signature"));
    let mut border = border_style(doc, d, usage)?.unwrap_or_default();
    border.set(b"W".to_vec(), Object::Real(local_width));
    normalize_dash(doc, d, &mut border, scale, usage)?;
    out.set(b"BS".to_vec(), Object::Dict(border));
    out.remove(b"Border");
    Ok(out)
}

/// Only raw malformed pattern syntax may use the historic solid display default.
fn invalid_raw_dash(usage: SignatureAppearanceUse) -> Result<Option<Vec<f64>>, AnnotError> {
    match usage {
        SignatureAppearanceUse::Display => Ok(None),
        SignatureAppearanceUse::Edit => Err(invalid("the signature dash pattern is invalid")),
    }
}

fn raw_dash(doc: &Document, d: &Dict, usage: SignatureAppearanceUse) -> Result<Option<Vec<f64>>, AnnotError> {
    let raw = if let Some(bs) = border_style(doc, d, usage)? {
        let dashed = bs.get(b"S").is_some_and(|s| doc.resolve(s).as_name() == Some(b"D"));
        if !dashed {
            return Ok(None);
        }
        match bs.get(b"D") {
            None => Object::Array(vec![Object::Real(3.0)]),
            Some(value) => match doc.resolve(value).as_ref() {
                Object::Null => Object::Array(vec![Object::Real(3.0)]), // A null dictionary value is absent.
                value => value.clone(),
            },
        }
    } else if let Some(border) = d.get(b"Border") {
        let border = doc.resolve(border);
        let Some(values) = border.as_array() else {
            return invalid_raw_dash(usage);
        };
        match values.get(3) {
            None => return Ok(None),
            Some(value) => (*doc.resolve(value)).clone(),
        }
    } else {
        return Ok(None);
    };
    let Some(values) = raw.as_array().filter(|a| a.len() <= 64) else {
        return invalid_raw_dash(usage);
    };
    if values.is_empty() {
        return Ok(None); // The valid empty dash array specifies a solid line.
    }
    let mut pattern = Vec::with_capacity(values.len());
    let mut positive = false;
    for value in values {
        let Some(value) = doc.resolve(value).as_f64().filter(|v| v.is_finite() && *v >= 0.0) else {
            return invalid_raw_dash(usage);
        };
        positive |= value > 0.0;
        pattern.push(value);
    }
    if !positive {
        return invalid_raw_dash(usage);
    }
    Ok(Some(pattern))
}

fn normalize_dash(doc: &Document, d: &Dict, out: &mut Dict, scale: f64, usage: SignatureAppearanceUse) -> Result<(), AnnotError> {
    let Some(raw) = raw_dash(doc, d, usage)? else {
        out.set(b"S".to_vec(), Object::name("S"));
        out.remove(b"D");
        return Ok(());
    };
    // Raw validation completed before scaling. Never convert a derived failure to solid.
    let mut dash = Vec::with_capacity(raw.len());
    for value in raw {
        let local = value * scale;
        if !(local.is_finite() && (local as f32).is_finite()) || (value > 0.0 && local as f32 <= 0.0) {
            return Err(invalid("the signature dash length cannot be represented"));
        }
        dash.push(Object::Real(local));
    }
    out.set(b"S".to_vec(), Object::name("D"));
    out.set(b"D".to_vec(), Object::Array(dash));
    Ok(())
}
