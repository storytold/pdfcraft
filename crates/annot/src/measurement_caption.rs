//! PdfCraft distance captions: standard Line offsets with a horizontal appearance.
use crate::create::num_array;
use crate::draw::redraw_or_restore;
use crate::lookup::{annot_dict, annot_ref, page_ref};
use crate::modify::{touch, unlocked};
use crate::{AnnotError, Meta, appearance};
use pdfcraft_cos::{Dict, Document, Object};

const MAX_OFFSET: f64 = 14_400.0;

pub(crate) struct Caption {
    pub baseline: [f64; 2],
    pub width: f64,
    pub offset: [f64; 2],
    pub along: [f64; 2],
}
impl Caption {
    pub fn rect(&self) -> [f64; 4] {
        [self.baseline[0], self.baseline[1] - 2.5, self.baseline[0] + self.width, self.baseline[1] + 8.0]
    }
}

fn numbers<const N: usize>(d: &Dict, key: &[u8]) -> Option<[f64; N]> {
    let a = d.get(key)?.as_array()?;
    if a.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (target, item) in out.iter_mut().zip(a) {
        *target = item.as_f64().filter(|v| v.is_finite() && v.abs() <= 1e9)?;
    }
    Some(out)
}

/// Shared by the appearance, hit testing and editing. Foreign Line captions take their existing
/// path. Until moved, PdfCraft's caption remains exactly at its old Rect-relative baseline.
/// ISO 32000-2 Table 178 /CO offsets the normal caption position along and across the line;
/// /CP and /LL, /LLE, /LLO are read but never changed. The AP deliberately stays horizontal:
/// another viewer regenerating it may instead draw the caption along the line.
pub(crate) fn layout(d: &Dict) -> Option<Caption> {
    if d.name(b"Subtype") != Some(b"Line")
        || d.name(b"IT") != Some(b"LineDimension")
        || !d.contains(b"Measure")
        || !matches!(d.get(b"Cap"), Some(Object::Bool(true)))
    {
        return None;
    }
    let string = d.get(b"PCMeasureValue")?.as_string()?;
    // At most 256 UTF-16 surrogate pairs plus the BOM; cap before decoding or allocating.
    if string.bytes.len() > 1026 {
        return None;
    }
    let value = string.to_text();
    if value.chars().count() > 256 {
        return None;
    }
    let width = appearance::text_width(&value, 10.0);
    let (mid, along) = appearance::measurement_line_frame(d, numbers::<4>(d, b"L")?)?;
    let normal = [-along[1], along[0]];
    let base = if d.name(b"CP") == Some(b"Top") { appearance::border_width(d) * 0.5 + 2.0 } else { -3.5 };
    let origin = [mid[0] + normal[0] * base, mid[1] + normal[1] * base];
    let (baseline, offset) = if d.contains(b"CO") {
        let co = numbers::<2>(d, b"CO")?;
        if co.iter().any(|v| v.abs() > MAX_OFFSET) {
            return None;
        }
        ([origin[0] + along[0] * co[0] + normal[0] * co[1] - width * 0.5, origin[1] + along[1] * co[0] + normal[1] * co[1]], co)
    } else {
        let r = numbers::<4>(d, b"Rect")?;
        // External edits can reverse Rect corners; use the same normalized top as the AP.
        let baseline = [(r[0] + r[2] - width) * 0.5, r[1].max(r[3]) - 12.0];
        let centre = [baseline[0] + width * 0.5, baseline[1]];
        let delta = [centre[0] - origin[0], centre[1] - origin[1]];
        (baseline, [delta[0] * along[0] + delta[1] * along[1], delta[0] * normal[0] + delta[1] * normal[1]])
    };
    (baseline.iter().all(|v| v.is_finite() && v.abs() <= 1e9) && offset.iter().all(|v| v.is_finite())).then_some(Caption {
        baseline,
        width,
        offset,
        along,
    })
}

/// Rebuilt bounds for a moved PdfCraft distance readout. All appearance rebuilds and caption
/// moves share this, so restyling cannot clip the caption or retain a vacated frame. An absent
/// or malformed /CO keeps the existing legacy drawing path unchanged.
pub(crate) fn bounds(d: &Dict) -> Option<[f64; 4]> {
    if !d.contains(b"CO") {
        return None;
    }
    let rect = layout(d)?.rect();
    let line = appearance::measurement_line_bounds(d, numbers::<4>(d, b"L")?)?;
    Some([line[0].min(rect[0] - 1.0), line[1].min(rect[1] - 1.0), line[2].max(rect[2] + 1.0), line[3].max(rect[3] + 1.0)])
}

/// The horizontal numeric readout's user-space bounds, only for a PdfCraft distance measurement.
pub fn measurement_caption_rect(doc: &Document, page: usize, index: usize) -> Option<[f64; 4]> {
    let p = doc.get(page_ref(doc, page).ok()?);
    let list = doc.resolve(p.as_dict()?.get(b"Annots")?);
    let a = doc.resolve(list.as_array()?.get(index)?);
    Some(layout(a.as_dict()?)?.rect())
}

/// Move only a PdfCraft distance readout by the relative vector `[dx, dy]` in the PDF page's
/// native, unrotated user space: positive x is right and positive y is up. These units equal
/// points only when /UserUnit is 1; this is not an absolute position or a displayed-page delta.
/// Automation's `comment_edit.caption_move` takes displayed page points (right/down) and
/// converts once with `pdfcraft_engine::Document::object_move_offset` before reaching here.
/// Both delta components and the resulting line-frame /CO components must be finite and
/// within +/-14400 native units. Locked annotations and anything other than a PdfCraft
/// distance readout return an error; perimeter and area labels are not supported.
/// /L, /Measure and leader geometry remain untouched; /CO (ISO 32000-2 Table 178) and the
/// appearance persist the move. PdfCraft keeps the caption horizontal; another viewer
/// regenerating the appearance may draw it along the line instead.
pub fn move_measurement_caption(doc: &mut Document, page: usize, index: usize, dx: f64, dy: f64, meta: &Meta) -> Result<(), AnnotError> {
    if [dx, dy].iter().any(|v| !v.is_finite() || v.abs() > MAX_OFFSET) {
        return Err(AnnotError::Invalid("caption offset must be finite and within 14400 points".into()));
    }
    let (_, r) = annot_ref(doc, page, index)?;
    unlocked(doc, r)?;
    let before = annot_dict(doc, r);
    let caption = layout(&before).ok_or_else(|| AnnotError::Invalid("only PdfCraft distance measurements have a movable readout".into()))?;
    let [ux, uy] = caption.along;
    let co = [caption.offset[0] + ux * dx + uy * dy, caption.offset[1] - uy * dx + ux * dy];
    if co.iter().any(|v| !v.is_finite() || v.abs() > MAX_OFFSET) {
        return Err(AnnotError::Invalid("caption offset must be within 14400 points".into()));
    }
    let mut next = before.clone();
    next.set(b"CO".to_vec(), num_array(&co));
    let rect = bounds(&next).ok_or_else(|| AnnotError::Invalid("invalid measurement caption bounds".into()))?;
    next.set(b"Rect".to_vec(), num_array(&rect));
    touch(&mut next, meta);
    doc.update_dict(r, |d| *d = next)?;
    redraw_or_restore(doc, r, before)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn distance() -> Dict {
        let mut d = Dict::new();
        for (key, value) in [(b"Subtype".as_slice(), "Line"), (b"IT", "LineDimension"), (b"CP", "Top")] {
            d.set(key.to_vec(), Object::name(value));
        }
        d.set(b"Measure".to_vec(), Object::Dict(Dict::new()));
        d.set(b"PCMeasureValue".to_vec(), pdfcraft_cos::PdfString::text("1.00 in"));
        d.set(b"Cap".to_vec(), Object::Bool(true));
        d.set(b"L".to_vec(), num_array(&[10.0, 20.0, 70.0, 100.0]));
        d.set(b"Rect".to_vec(), num_array(&[0.0, 10.0, 80.0, 130.0]));
        d
    }
    #[test]
    fn caption_offsets_follow_the_line_frame_without_rotating_the_readout() {
        let mut d = distance();
        d.set(b"LL".to_vec(), Object::Real(12.0));
        d.set(b"LLO".to_vec(), Object::Real(4.0));
        d.set(b"LLE".to_vec(), Object::Real(3.0));
        let before = layout(&d).unwrap();
        assert!((before.baseline[1] - 118.0).abs() < 1e-9, "unmoved legacy baseline");
        d.set(b"CO".to_vec(), num_array(&before.offset));
        let initialized = layout(&d).unwrap();
        for (a, b) in initialized.baseline.into_iter().zip(before.baseline) {
            assert!((a - b).abs() < 1e-9, "first move has no jump");
        }
        d.set(b"CO".to_vec(), num_array(&[before.offset[0] + 10.0, before.offset[1] + 20.0]));
        let moved = layout(&d).unwrap();
        // /L direction (0.6,0.8), perpendicular (-0.8,0.6): /CO (10,20) -> (-10,20).
        assert!((moved.baseline[0] - before.baseline[0] + 10.0).abs() < 1e-9);
        assert!((moved.baseline[1] - before.baseline[1] - 20.0).abs() < 1e-9);
        assert_eq!(moved.width, before.width);
        assert_eq!(moved.rect()[3] - moved.rect()[1], 10.5);
        // Expanding the enclosing Rect must never move a caption with an explicit offset.
        d.set(b"Rect".to_vec(), num_array(&[-100.0, -100.0, 300.0, 300.0]));
        assert_eq!(layout(&d).unwrap().baseline, moved.baseline);
    }
    #[test]
    fn distance_path_bounds_include_displaced_leaders_and_arrow_margins() {
        let mut d = distance();
        d.set(b"LL".to_vec(), Object::Real(12.0));
        d.set(b"LLO".to_vec(), Object::Real(4.0));
        d.set(b"LLE".to_vec(), Object::Real(3.0));
        d.set(b"LE".to_vec(), Object::Array(vec![Object::name("OpenArrow"), Object::name("OpenArrow")]));
        let rect = appearance::measurement_line_bounds(&d, numbers::<4>(&d, b"L").unwrap()).unwrap();
        // Direction (0.6,0.8): leaders run from offset 4 through 12+4+3. Their extrema
        // are [-5.2,22.4,66.8,111.4], plus the creator's width-1 ending margin of 10.5.
        for (actual, expected) in rect.into_iter().zip([-15.7, 11.9, 77.3, 121.9]) {
            assert!((actual - expected).abs() < 1e-9, "{rect:?}");
        }
    }

    #[test]
    fn foreign_and_malformed_measurement_captions_have_no_drag_target() {
        let mut d = distance();
        d.remove(b"PCMeasureValue");
        assert!(layout(&d).is_none());
        let mut d = distance();
        d.set(b"CO".to_vec(), num_array(&[f64::NAN, 0.0]));
        assert!(layout(&d).is_none());
        d.set(b"CO".to_vec(), num_array(&[14_401.0, 0.0]));
        assert!(layout(&d).is_none());
        d.remove(b"CO");
        d.set(b"L".to_vec(), num_array(&[10.0, 20.0, 10.0, 20.0]));
        assert!(layout(&d).is_none());
    }
}
