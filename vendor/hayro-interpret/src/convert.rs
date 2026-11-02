use hayro_syntax::content::ops::{LineCap, LineJoin, Transform};

pub(crate) fn convert_transform(t: Transform) -> kurbo::Affine {
    kurbo::Affine::new([
        t.0.as_f64(),
        t.1.as_f64(),
        t.2.as_f64(),
        t.3.as_f64(),
        t.4.as_f64(),
        t.5.as_f64(),
    ])
}

pub(crate) fn convert_line_cap(lc: LineCap) -> kurbo::Cap {
    match lc.0.as_i64() {
        0 => kurbo::Cap::Butt,
        1 => kurbo::Cap::Round,
        2 => kurbo::Cap::Square,
        _ => kurbo::Cap::Butt,
    }
}

pub(crate) fn convert_line_join(lc: LineJoin) -> kurbo::Join {
    match lc.0.as_i64() {
        0 => kurbo::Join::Miter,
        1 => kurbo::Join::Round,
        2 => kurbo::Join::Bevel,
        _ => kurbo::Join::Miter,
    }
}

/// PdfCraft patch (61): the line width to keep for a `w` operand or an ExtGState `/LW`. ISO
/// 32000-2 §8.4.3.2 makes 0 "the thinnest line that can be rendered at device resolution" but
/// says nothing of a negative width; it is drawn as 0, as is one that isn't finite, so no device
/// sees a width below zero.
pub(crate) fn convert_line_width(width: f32) -> f32 {
    if width.is_finite() && width > 0.0 {
        width
    } else {
        0.0
    }
}

/// PdfCraft patch (61): the dash array to keep for a `d` operator or an ExtGState `/D`. ISO
/// 32000-2 §8.4.3.6 requires numbers that "shall be nonnegative and not all zero". A pattern that
/// breaks that (all zero, a negative or a non-finite entry) is kept as `[]`, a solid line, rather
/// than cut into dashes the zero-entry workaround below would make a hundredth of a point long.
pub(crate) fn convert_dash_array(
    dashes: impl Iterator<Item = f32>,
) -> smallvec::SmallVec<[f32; 4]> {
    let dashes: smallvec::SmallVec<[f32; 4]> = dashes.collect();
    if dashes.iter().any(|d| !d.is_finite() || *d < 0.0) || dashes.iter().all(|d| *d == 0.0) {
        return smallvec::SmallVec::new();
    }
    // kurbo apparently cannot properly deal with dashes that are exactly 0.
    dashes
        .into_iter()
        .map(|d| if d == 0.0 { 0.01 } else { d })
        .collect()
}
